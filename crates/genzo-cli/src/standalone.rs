//! カタログを使わないコマンド: `render`（1 つのファイルを現像して書き出す）と `info`（メタデータ）。
//!
//! 信頼できない入力の解析と展開は、カタログを使うコマンドと同じく **ワーカープロセス** で行う
//! （04 の 1.1 節・1.2 節、SEC-05）。ワーカーは本体の実行ファイル自身を `__worker` 付きで 1 つ起動する。
//! 現像は本体で行う（GPU が使えれば GPU 版、GPU の側の失敗なら CPU 版。2.4 節）。書き出しは
//! genzo-media の安全な書き出し（6.4 節。入力のファイルは保護の対象にし、上書きしない）。
//!
//! genzo-api の [`genzo_api::Core`] はカタログが前提なので、ここではワーカー（genzo-worker）・
//! パイプライン（genzo-pipeline）・GPU（genzo-gpu）・書き出し（genzo-media）を直接使う
//! （04 の 1.4 節からの逸脱。lib.rs の doc）。手順は genzo-api の書き出し（`export.rs`）・展開
//! （`previews.rs` の `load_source`）と同じ。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use genzo_api::{ApiError, AssetKind, DevelopSettings, RenderBackend};
use genzo_gpu::{ExportBackend, GpuContext, GpuContextOptions, GpuRenderer};
use genzo_jobs::CancellationToken;
use genzo_media::{DynRgbImage, ExifData, ProtectedFiles, RgbImage8, RgbImage16, WriteOutcome};
use genzo_model::{FileId, OutputColorSpace, PhotoMetadata};
use genzo_pipeline::{
    ColorContract, Engine, EngineConfig, ExportOptions, ExportPixels, NeverCancel, PhotoSource,
    RgbImage, SourceId, SourceImage,
};
use genzo_worker::{WORKER_SUBCOMMAND, WorkerClient, WorkerConfig};
use serde::Serialize;
use serde_json::json;

use crate::Status;
use crate::args::{FormatArg, GlobalArgs, GpuArg, RenderArgs};
use crate::develop::{merged_settings, read_json_input};
use crate::error::{CliError, CliResult};
use crate::export::{backend_text, export_settings, format_text};
use crate::library::{shutter_text, video_items};
use crate::output::{Output, color_space_text, json_value, key_values};

/// 本体の実行ファイル自身をワーカーとして起動する設定（`genzo __worker`）。
pub fn worker_config() -> CliResult<WorkerConfig> {
    let exe = std::env::current_exe()
        .map_err(|e| CliError::other(format!("本体の実行ファイルの場所が分かりません: {e}")))?;
    Ok(WorkerConfig {
        executable: Some(exe),
        args: vec![WORKER_SUBCOMMAND.into()],
        ..WorkerConfig::default()
    })
}

/// 1 つのワーカー（drop すると終了させる）。
pub struct Worker {
    client: Option<WorkerClient>,
}

impl Worker {
    /// 起動する。
    pub fn spawn() -> CliResult<Self> {
        Ok(Self {
            client: Some(WorkerClient::spawn(worker_config()?)?),
        })
    }

    /// ワーカー。
    pub fn client(&mut self) -> &mut WorkerClient {
        self.client.as_mut().expect("終了の前は常に Some")
    }

    /// 終了を依頼して待つ（結果は受け取った後なので、失敗は警告にする）。
    pub fn shutdown(mut self, out: Output) {
        if let Some(c) = self.client.take()
            && let Err(e) = c.shutdown()
        {
            out.warn(format!("ワーカーを正常に終了できませんでした: {e}"));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(c) = self.client.take()
            && let Err(e) = c.shutdown()
        {
            tracing::warn!(error = %e, "ワーカーを終了できない");
        }
    }
}

/// 入力の種類（拡張子で決める。genzo-api の取り込みと同じ拡張子）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// RAW（[`genzo_api::RAW_EXTENSIONS`]）。
    Raw,
    /// RAW 以外の写真（[`genzo_api::IMAGE_EXTENSIONS`]）。
    Image,
    /// 動画（[`genzo_api::VIDEO_EXTENSIONS`]）。
    Video,
}

/// 入力の種類を拡張子で決める（対象外の拡張子はエラー）。
pub fn input_kind(path: &Path) -> CliResult<InputKind> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if genzo_api::RAW_EXTENSIONS.contains(&ext.as_str()) {
        Ok(InputKind::Raw)
    } else if genzo_api::IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        Ok(InputKind::Image)
    } else if genzo_api::VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Ok(InputKind::Video)
    } else {
        Err(CliError::input(format!(
            "対応していない拡張子です（{}）。RAW: {}、画像: {}、動画: {}",
            path.display(),
            genzo_api::RAW_EXTENSIONS.join(", "),
            genzo_api::IMAGE_EXTENSIONS.join(", "),
            genzo_api::VIDEO_EXTENSIONS.join(", ")
        )))
    }
}

/// 動画を現像の入力にしたときの説明。
const VIDEO_NOT_DEVELOPED: &str =
    "動画は現像できません（動画はカタログでの管理だけ。EXP-05 は v1）";

/// 入力のファイルがあるか確かめる。
pub fn ensure_file(path: &Path) -> CliResult<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(CliError::Api(ApiError::NotFound(format!(
            "ファイル {}",
            path.display()
        ))))
    }
}

/// 展開した入力。
pub struct Loaded {
    /// 現像の入力（段階 A0）。
    pub source: PhotoSource,
    /// 撮影情報。
    pub metadata: PhotoMetadata,
    /// RAW か。
    pub is_raw: bool,
    /// 幅と高さ（向きを反映する前）。
    pub size: (u32, u32),
    /// RAW のデコーダの識別子。
    pub decoder: Option<String>,
}

/// カタログを使わない入力の ID（キャッシュのキーにだけ使う）。
pub const STANDALONE_SOURCE_ID: SourceId = SourceId::new(FileId::new(1), 1);

/// ワーカーで展開する（RAW は `DecodeRaw`、それ以外は `DecodeImage`。genzo-api の `load_source` と同じ）。
pub fn load(client: &mut WorkerClient, path: &Path, kind: InputKind) -> CliResult<Loaded> {
    let cancel = CancellationToken::new();
    match kind {
        InputKind::Raw => {
            let frame = client.decode_raw(path, &cancel)?;
            let size = (frame.image.width, frame.image.height);
            let metadata = frame.image.metadata.clone();
            Ok(Loaded {
                source: PhotoSource::new(
                    STANDALONE_SOURCE_ID,
                    SourceImage::Raw(Arc::new(frame.image)),
                )?,
                metadata,
                is_raw: true,
                size,
                decoder: frame.decoder_id,
            })
        }
        InputKind::Image => {
            let img = client.decode_image(path, &cancel)?;
            let (w, h) = (img.width(), img.height());
            let metadata = img.info.metadata.clone();
            let image = RgbImage::from_vec(w, h, ColorContract::B2Working, img.pixels)?;
            Ok(Loaded {
                source: PhotoSource::new(
                    STANDALONE_SOURCE_ID,
                    SourceImage::Working {
                        image: Arc::new(image),
                        orientation: metadata.orientation,
                    },
                )?,
                metadata,
                is_raw: false,
                size: (w, h),
                decoder: None,
            })
        }
        InputKind::Video => Err(CliError::input(VIDEO_NOT_DEVELOPED)),
    }
}

/// GPU（`--gpu off`・GPU がない・初期化できない場合は `None`。初期化の失敗は警告を出して CPU 版にする）。
pub fn gpu_renderer(gpu: GpuArg, out: Output) -> Option<GpuRenderer> {
    if gpu == GpuArg::Off {
        return None;
    }
    let options = GpuContextOptions::from_env();
    match GpuContext::new(&options).and_then(|c| c.map(GpuRenderer::new).transpose()) {
        Ok(r) => r,
        Err(e) => {
            out.warn(format!("GPU を初期化できないため、CPU 版で処理します: {e}"));
            None
        }
    }
}

/// `render` の結果（JSON）。
#[derive(Debug, Serialize)]
struct RenderOutput {
    input: PathBuf,
    /// 書き出したファイル（スキップした場合は `None`）。
    output: Option<PathBuf>,
    replaced: bool,
    /// スキップした場合の、既にあったファイル。
    skipped_existing: Option<PathBuf>,
    backend: RenderBackend,
    width: u32,
    height: u32,
    source_width: u32,
    source_height: u32,
    is_raw: bool,
    decoder: Option<String>,
    export: genzo_model::ExportSettings,
    settings: DevelopSettings,
    /// 時間（ミリ秒。参考値）: 展開（ワーカー・転送を含む）、現像、エンコードと書き込み。
    timings_ms: serde_json::Value,
}

/// `render`。
pub fn render(g: &GlobalArgs, out: Output, a: RenderArgs) -> CliResult<Status> {
    let kind = input_kind(&a.file)?;
    ensure_file(&a.file)?;
    if kind == InputKind::Video {
        return Err(CliError::input(VIDEO_NOT_DEVELOPED));
    }
    let format = a
        .opts
        .format
        .or_else(|| FormatArg::from_extension(&a.out))
        .ok_or_else(|| {
            CliError::Usage(
                "--format を指定するか、--out の拡張子を jpg / tif / png にしてください".to_owned(),
            )
        })?;
    let color_space = a
        .opts
        .color_space
        .map_or(OutputColorSpace::Srgb, OutputColorSpace::from);
    let export = export_settings(&a.opts, format, color_space)?;
    if a.out.is_dir() {
        return Err(CliError::Usage(format!(
            "--out には書き出すファイルのパスを指定してください（{} はフォルダです）",
            a.out.display()
        )));
    }
    let settings = match &a.settings {
        Some(src) => merged_settings(&DevelopSettings::default(), &read_json_input(src)?, false)?,
        None => DevelopSettings::default(),
    };

    let t0 = Instant::now();
    let mut worker = Worker::spawn()?;
    let loaded = load(worker.client(), &a.file, kind)?;
    worker.shutdown(out);
    let t_decode = t0.elapsed();

    let t1 = Instant::now();
    let engine = Engine::new(EngineConfig::default())?;
    let gpu = gpu_renderer(g.gpu, out);
    let outcome = genzo_gpu::export_with_fallback(
        gpu.as_ref(),
        &engine,
        &loaded.source,
        &settings,
        &export,
        &ExportOptions::default(),
        &NeverCancel,
    )?;
    let backend = match &outcome.backend {
        ExportBackend::Gpu => RenderBackend::Gpu,
        ExportBackend::Cpu { gpu_error } => {
            if let Some(e) = gpu_error {
                out.warn(format!(
                    "GPU の処理に失敗したため、CPU 版で処理しました: {e}"
                ));
            }
            RenderBackend::Cpu
        }
    };
    let image = outcome.image;
    let (w, h) = (image.width, image.height);
    let pixels = match image.pixels {
        ExportPixels::Rgb8(v) => DynRgbImage::Rgb8(RgbImage8::from_raw(w, h, v)?),
        ExportPixels::Rgb16(v) => DynRgbImage::Rgb16(RgbImage16::from_raw(w, h, v)?),
    };
    let t_render = t1.elapsed();

    let t2 = Instant::now();
    if let Some(parent) = a.out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| CliError::io(parent, e))?;
    }
    let exif = ExifData::from_photo_metadata(&loaded.metadata);
    // 原本の保護（6.4 節）: 入力のファイルへは書き出さない（パスとファイルの同一性で照合する）。
    let mut protected = ProtectedFiles::new();
    protected.insert(&a.file);
    let written = genzo_media::export_image(&pixels, &export, Some(&exif), &a.out, &protected)?;
    let t_write = t2.elapsed();

    let (output, replaced, skipped_existing) = match written {
        WriteOutcome::Written { path, replaced } => (Some(path), replaced, None),
        WriteOutcome::Skipped { existing } => (None, false, Some(existing)),
    };
    let result = RenderOutput {
        input: a.file.clone(),
        output,
        replaced,
        skipped_existing,
        backend,
        width: w,
        height: h,
        source_width: loaded.size.0,
        source_height: loaded.size.1,
        is_raw: loaded.is_raw,
        decoder: loaded.decoder.clone(),
        export,
        settings,
        timings_ms: json!({
            "decode": t_decode.as_secs_f64() * 1e3,
            "render": t_render.as_secs_f64() * 1e3,
            "encode_and_write": t_write.as_secs_f64() * 1e3,
        }),
    };
    if out.json {
        out.print_json(&result);
    } else {
        match (&result.output, &result.skipped_existing) {
            (Some(p), _) => out.line(format!(
                "{} を書き出しました: {}（{} × {}、{}、{}{}）",
                format_text(export.format),
                p.display(),
                w,
                h,
                color_space_text(export.color_space),
                backend_text(backend),
                if replaced { "、上書き" } else { "" }
            )),
            (None, Some(e)) => out.line(format!(
                "同じ名前のファイルがあるため、書き出しませんでした: {}",
                e.display()
            )),
            (None, None) => {}
        }
    }
    Ok(Status::Success)
}

/// `info`。
pub fn info(out: Output, file: &Path) -> CliResult<Status> {
    let kind = input_kind(file)?;
    ensure_file(file)?;
    let mut worker = Worker::spawn()?;
    let cancel = CancellationToken::new();
    let (value, lines) = match kind {
        InputKind::Raw | InputKind::Image => {
            let probe = worker.client().probe_photo(file, &cancel)?;
            let m = &probe.metadata;
            let opt = |o: Option<String>| o.unwrap_or_else(|| "—".to_owned());
            let lines = vec![
                ("ファイル", file.display().to_string()),
                ("種別", "写真".to_owned()),
                ("形式", format!("{:?}", probe.format).to_lowercase()),
                ("カメラ", opt(m.camera_name())),
                ("レンズ", opt(m.lens.clone())),
                (
                    "寸法",
                    match (m.width, m.height) {
                        (Some(w), Some(h)) => format!("{w} × {h}（向き {}）", m.orientation as u8),
                        _ => "—".to_owned(),
                    },
                ),
                ("ISO", opt(m.iso.map(|x| x.to_string()))),
                ("絞り", opt(m.aperture.map(|x| format!("F{x:.1}")))),
                (
                    "シャッター速度",
                    opt(m.shutter_s.map(|x| shutter_text(f64::from(x)))),
                ),
                ("焦点距離", opt(m.focal_mm.map(|x| format!("{x:.0} mm")))),
                (
                    "撮影日時",
                    opt(m.capture.datetime.clone().map(|d| match &m.capture.offset {
                        Some(o) => format!("{d}（{o}）"),
                        None => d,
                    })),
                ),
                (
                    "GPS",
                    opt(m.gps.map(|g| format!("{:.6}, {:.6}", g.lat, g.lon))),
                ),
                (
                    "色",
                    opt(probe.profile.as_ref().map(|p| {
                        if p.embedded {
                            "埋め込みの ICC プロファイル".to_owned()
                        } else {
                            "sRGB とみなした".to_owned()
                        }
                    })),
                ),
            ];
            let value = json!({
                "file": file,
                "kind": AssetKind::Photo,
                "probe": json_value(&probe),
            });
            (value, lines)
        }
        InputKind::Video => {
            let probe = worker.client().probe_video(file, &cancel)?;
            let m = &probe.metadata;
            let mut lines = vec![
                ("ファイル", file.display().to_string()),
                ("種別", "動画".to_owned()),
                (
                    "寸法",
                    match (m.width, m.height) {
                        (Some(w), Some(h)) => format!("{w} × {h}"),
                        _ => "—".to_owned(),
                    },
                ),
            ];
            lines.extend(video_items(m));
            lines.push((
                "画素の形式",
                probe.pix_fmt.clone().unwrap_or_else(|| "—".to_owned()),
            ));
            lines.push(("回転", format!("{}°", probe.rotation_deg)));
            let value = json!({
                "file": file,
                "kind": AssetKind::Video,
                "probe": json_value(&probe),
            });
            (value, lines)
        }
    };
    worker.shutdown(out);
    if out.json {
        out.print_json(&value);
    } else {
        out.line(key_values(&lines).trim_end());
    }
    Ok(Status::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_kinds_by_extension() {
        assert_eq!(input_kind(Path::new("a/DSC1.ARW")).unwrap(), InputKind::Raw);
        assert_eq!(input_kind(Path::new("x.dng")).unwrap(), InputKind::Raw);
        assert_eq!(input_kind(Path::new("x.JPG")).unwrap(), InputKind::Image);
        assert_eq!(input_kind(Path::new("x.tiff")).unwrap(), InputKind::Image);
        assert_eq!(input_kind(Path::new("x.MP4")).unwrap(), InputKind::Video);
        assert!(input_kind(Path::new("x.xmp")).is_err());
        assert!(input_kind(Path::new("noext")).is_err());
    }
}
