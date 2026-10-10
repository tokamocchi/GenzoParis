//! `bench`: 計測（02 の MAINT-07、05 の 1.8 節のルール、PoC-3 の測定の準備）。
//!
//! genzo-testkit の [`Bench`] で測り、[`BenchRecorder`] で JSON（`<記録先>/<名前>.json`）に追記して、前回の
//! 結果（同じ名前・条件・入力・現像設定・環境）と比べる。報告する値は平均・95 パーセンタイル・最大
//! （[`genzo_testkit::bench::DurationStats`]）、目標を超えた回数。95 パーセンタイルが前回より 10% 以上
//! 悪化したもの（genzo-testkit の既定。仮置き）があれば終了コード 1。目標に届かないだけなら 0
//! （このコンテナの llvmpipe など、実機でない環境の数値で失敗にしないため）。
//!
//! # 計測する項目
//!
//! | 名前 | 内容（PoC-3 の測定方法） | 種類・条件 | 目標 |
//! |---|---|---|---|
//! | `cli_preview_stage_c` | 露光量のドラッグ（段階 C。A1・ガイドはキャッシュ）(1) | 応答時間・ウォーム | 33ms（PERF-01） |
//! | `cli_preview_wb_drag_draft` | WB のドラッグ（段階 A1 の簡易処理 → C）(1) | 応答時間・ウォーム | 1/15 秒（PERF-01b の 15fps） |
//! | `cli_preview_wb_release_final` | WB を離した後の最終品質の描画（A1 の最終品質 → ガイド → C）(2) | 応答時間・コールドとウォーム | 0.5 秒（PERF-01b） |
//! | `cli_export_jpeg` | 書き出しの時間の内訳: `decode`（ワーカーでの展開と転送）・`render`（現像）・`encode`・`write` (3) | 一括処理・コールド | 3 秒（PERF-10） |
//!
//! - プレビューは長辺 [`PREVIEW_LONG_EDGE`]（2560。1.8 節「プレビューは 2560 × 1707 で固定」）。入力の長辺が
//!   それより短いと、プレビューは入力の寸法になり（拡大しない）1.8 節の解像度にならないので、警告を出し、
//!   実際のプレビューの寸法を `notes.preview` に記録する（寸法の違う結果どうしは比べない）。
//! - コールドは、毎回エンジン（と GPU 版）のキャッシュを捨ててから測る。プロセスの起動直後（OS の
//!   ファイルキャッシュを含む）の時間ではない（genzo-testkit の bench の doc）。
//! - **測るのは呼び出しから結果が戻るまで** で、1.8 節の応答時間の定義（入力イベントから画面への表示まで）
//!   ではない。UI での計測は PoC-1・PoC-3 で行う（記録の `notes.scope = call_to_result`）。
//! - 展開と転送は、ワーカーの側の時間を受け取る仕組みがないため、1 つの内訳（`decode`）にまとめる。
//! - 入力: `--file`（ワーカーで展開）、または `--synthetic WxH`（合成の RAW。ワーカーが LibRaw を使える
//!   なら合成 DNG をワーカーで展開し、使えなければメモリの上の RAW を使う。`notes.input` で区別する）。
//!   どちらもなければ α7 IV の寸法（[`DEFAULT_SYNTHETIC_SIZE`]）の合成の RAW。
//! - GPU は `--gpu auto` で使えれば使う（GPU の失敗は CPU 版に切り替えず、エラーにする。処理した側の
//!   混ざった結果を記録しないため）。書き出しは genzo-gpu の [`genzo_gpu::export_with_fallback`]（失敗したら
//!   CPU 版。処理した側は `notes.backend`）。GPU の API（Vulkan・Metal・DX12 など）は `notes.gpu_backend`
//!   （API が違う結果どうしは比べない）、GPU の名前とドライバーは環境の情報に記録する。
//! - 終了コード 1（悪化）にするのは、**今回の回数が 1.8 節の規則を満たす** 計測が前回より悪化した場合
//!   だけ（[`regression_fails`]）。`--runs` で回数を減らした試しの計測は、比べた結果を出すが終了コードには
//!   しない（少ない回数の 95 パーセンタイルは最大値とほぼ同じで、ばらつきで悪化と判定されやすいため）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use genzo_gpu::{ExportBackend, GpuPreviewOptions, GpuRenderer};
use genzo_media::{DynRgbImage, ExifData, ProtectedFiles, RgbImage8, RgbImage16};
use genzo_model::{
    ConflictPolicy, CurvePoint, DevelopSettings, ExportSettings, RenderQuality, WhiteBalance,
};
use genzo_pipeline::{
    Engine, EngineConfig, ExportOptions, ExportPixels, NeverCancel, OutputTarget, PhotoSource,
    PreviewRequest, SourceImage,
};
use genzo_raw::{CfaPattern, RawImage};
use genzo_testkit::bayer::{BayerParams, mosaic, write_dng};
use genzo_testkit::bench::{Bench, BenchRecorder, BenchReport, BenchResult, PhaseTimer};
use genzo_testkit::image::FloatImage;
use genzo_testkit::{EnvironmentInfo, InputRef};
use serde_json::json;

use crate::Status;
use crate::args::{BenchArgs, BenchTarget, GlobalArgs};
use crate::error::{CliError, CliResult};
use crate::output::{Output, json_value};
use crate::standalone::{
    InputKind, STANDALONE_SOURCE_ID, Worker, ensure_file, gpu_renderer, input_kind, load,
};

/// プレビューの長辺（05 の 1.8 節「プレビューは 2560 × 1707 で固定」。02 の PERF-01）。
pub const PREVIEW_LONG_EDGE: u32 = 2560;

/// 合成の RAW の既定の寸法（α7 IV。04 の 2.2 節）。
pub const DEFAULT_SYNTHETIC_SIZE: (u32, u32) = (7008, 4672);

/// 段階 C（露光量のドラッグ）の目標（PERF-01: 1 フレーム 33ms 以内）。
pub const STAGE_C_TARGET: Duration = Duration::from_millis(33);

/// WB のドラッグ（簡易処理）の目標（PERF-01b: 15fps 以上 = 1 フレーム 1/15 秒以内）。
pub const WB_DRAG_TARGET: Duration = Duration::from_nanos(1_000_000_000 / 15);

/// WB を離した後の最終品質の目標（PERF-01b: 0.5 秒以内）。
pub const WB_RELEASE_TARGET: Duration = Duration::from_millis(500);

/// 書き出しの目標（PERF-10: α7 IV のフル解像度の JPEG で 1 枚あたり 3 秒以内）。
pub const EXPORT_TARGET: Duration = Duration::from_secs(3);

/// ドラッグを模す WB の色温度の範囲（K）と刻み（**仮置き**: 4000〜7000 K を 150 K ずつ。毎回 A1 の
/// キャッシュに当たらない値にするため。値そのものに意味はない）。
const WB_SWEEP: (f32, f32, f32) = (4000.0, 7000.0, 150.0);

/// 計測で使う現像設定（いろいろなステージが有効になる設定。genzo-pipeline の計測と同じ考え方）。
pub fn bench_settings() -> DevelopSettings {
    let mut s = DevelopSettings {
        exposure_ev: 0.5,
        contrast: 30.0,
        ..Default::default()
    };
    s.tone.highlights = -70.0;
    s.tone.shadows = 70.0;
    s.tone.whites = 20.0;
    s.tone.blacks = -20.0;
    s.color.saturation = 30.0;
    s.color.vibrance = 30.0;
    s.tone_curve.luma = vec![
        CurvePoint::new(0.0, 0.0),
        CurvePoint::new(0.25, 0.2),
        CurvePoint::new(0.75, 0.82),
        CurvePoint::new(1.0, 1.0),
    ];
    s
}

/// 計測用の合成のシーン（カメラ RGB。整数の演算と定数の表だけで作る。OS の数学ライブラリによらない）。
///
/// 横に 4 つの区画: グレーの段階 / 細い線 / 色のパッチ / 明暗差。
pub fn synthetic_scene(w: u32, h: u32) -> CliResult<FloatImage> {
    const GRAYS: [f32; 8] = [0.004, 0.01, 0.025, 0.06, 0.12, 0.25, 0.5, 0.9];
    const PATCHES: [[f32; 3]; 6] = [
        [0.60, 0.08, 0.05],
        [0.05, 0.50, 0.07],
        [0.04, 0.06, 0.55],
        [0.70, 0.62, 0.05],
        [0.30, 0.22, 0.16],
        [0.18, 0.18, 0.18],
    ];
    FloatImage::from_rgb_fn(w, h, |x, y| match (x * 4 / w).min(3) {
        0 => [GRAYS[(y * 8 / h).min(7) as usize]; 3],
        1 => {
            if x % 7 == 0 || y % 9 == 0 || (x + y) % 13 == 0 {
                [0.8, 0.75, 0.7]
            } else {
                [0.03, 0.035, 0.04]
            }
        }
        2 => PATCHES[((x / 11 + y / 11 * 5) % 6) as usize],
        _ => {
            let (cx, cy) = (x % 40, y % 40);
            if (10..30).contains(&cx) && (10..30).contains(&cy) {
                [0.9, 0.88, 0.86]
            } else {
                [0.006, 0.005, 0.004]
            }
        }
    })
    .map_err(CliError::other)
}

/// 計測用の合成の RAW（RGGB・典型的な形のカメラ行列。実在の機種の値ではない）。
pub fn synthetic_raw(w: u32, h: u32) -> CliResult<RawImage> {
    let scene = synthetic_scene(w, h)?;
    mosaic(
        &scene,
        &BayerParams {
            cfa: CfaPattern::RGGB,
            cam_xyz: Some([
                [0.70, -0.20, -0.06],
                [-0.50, 1.30, 0.23],
                [-0.07, 0.13, 0.65],
            ]),
            ..Default::default()
        },
    )
    .map_err(CliError::other)
}

/// 計測の入力。
struct BenchInput {
    /// ワーカーで展開するファイル（なければメモリの上の RAW）。
    file: Option<(PathBuf, InputKind)>,
    /// メモリの上の入力（ファイルがない場合）。
    memory: Option<PhotoSource>,
    /// 記録する入力の ID とハッシュ。
    input: InputRef,
    /// 入力の種類（`file` / `worker_dng` / `in_memory`）。
    how: &'static str,
    /// 合成の DNG を置く一時フォルダ（drop で消す）。
    _temp: Option<tempfile::TempDir>,
}

impl BenchInput {
    fn prepare(a: &BenchArgs, worker: &mut Worker) -> CliResult<Self> {
        if let Some(file) = &a.file {
            ensure_file(file)?;
            let kind = input_kind(file)?;
            if kind == InputKind::Video {
                return Err(CliError::Usage(
                    "動画は計測の入力にできません（写真のファイルを指定してください）".to_owned(),
                ));
            }
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let input = InputRef::from_file(format!("file:{name}"), file)
                .map_err(|e| CliError::io(file, e))?;
            return Ok(Self {
                file: Some((file.clone(), kind)),
                memory: None,
                input,
                how: "file",
                _temp: None,
            });
        }
        let (w, h) = a.synthetic.unwrap_or(DEFAULT_SYNTHETIC_SIZE);
        let raw = synthetic_raw(w, h)?;
        let params = format!("cli_synthetic_scene v1 {w}x{h} cfa=rggb");
        let input = InputRef::from_bytes("synthetic:cli_scene", params.as_bytes());
        // ワーカーが LibRaw を使えるなら、合成 DNG をワーカーで展開する（展開と転送も測る）。
        let raw_supported = worker.client().ping()?.raw_decoder.is_some();
        if raw_supported {
            let temp = tempfile::tempdir().map_err(|e| CliError::io(std::env::temp_dir(), e))?;
            let path = temp.path().join("synthetic.dng");
            write_dng(&path, &raw).map_err(CliError::other)?;
            return Ok(Self {
                file: Some((path, InputKind::Raw)),
                memory: None,
                input,
                how: "worker_dng",
                _temp: Some(temp),
            });
        }
        let source = PhotoSource::new(STANDALONE_SOURCE_ID, SourceImage::Raw(Arc::new(raw)))?;
        Ok(Self {
            file: None,
            memory: Some(source),
            input,
            how: "in_memory",
            _temp: None,
        })
    }

    /// 入力を展開する（ファイルならワーカーで。RAW のデコーダの識別子も返す）。
    fn load(&self, worker: &mut Worker) -> CliResult<(PhotoSource, Option<String>)> {
        match (&self.file, &self.memory) {
            (Some((path, kind)), _) => {
                let l = load(worker.client(), path, *kind)?;
                Ok((l.source, l.decoder))
            }
            (None, Some(s)) => Ok((s.clone(), None)),
            (None, None) => Err(CliError::other("計測の入力がありません")),
        }
    }
}

/// 計測の共通の設定（環境・入力・条件）。
fn base(name: &str, kind_latency: bool, ctx: &Ctx) -> Bench {
    let b = if kind_latency {
        Bench::latency(name)
    } else {
        Bench::batch(name)
    };
    let b = match ctx.runs {
        Some(n) => b.iterations(n),
        None => b,
    };
    let b = b
        .environment(ctx.env.clone())
        .input(ctx.input.clone())
        .note("input", ctx.how)
        .note("resolution", format!("{}x{}", ctx.size.0, ctx.size.1))
        .note("backend", if ctx.gpu.is_some() { "gpu" } else { "cpu" })
        .note("source", if ctx.is_raw { "raw" } else { "image" })
        .note("scope", "call_to_result");
    match &ctx.gpu {
        Some(g) => b.note("gpu_backend", g.context().summary().backend.clone()),
        None => b,
    }
}

/// プレビューの寸法（genzo-pipeline の [`genzo_pipeline::preview_size`] で長辺を [`PREVIEW_LONG_EDGE`] に
/// 縮める。拡大はしない）。向きと切り抜きは考えない（記録と、1.8 節の解像度との比較にだけ使う）。
pub fn preview_size((w, h): (u32, u32)) -> (u32, u32) {
    genzo_pipeline::preview_size(w, h, PREVIEW_LONG_EDGE)
}

/// 終了コードを 1（悪化）にするか: 前回より悪化し、今回の回数が 1.8 節の規則を満たす計測があるか。
pub fn regression_fails(reports: &[BenchReport]) -> bool {
    reports
        .iter()
        .any(|r| r.regressed() && r.result.meets_iteration_rule)
}

/// 計測の文脈。
struct Ctx {
    runs: Option<u32>,
    env: EnvironmentInfo,
    input: InputRef,
    how: &'static str,
    size: (u32, u32),
    /// RAW か（RAW 以外は簡易処理がなく、WB のドラッグも最終品質で描く）。
    is_raw: bool,
    gpu: Option<GpuRenderer>,
}

/// プレビューを 1 回描く（GPU があれば GPU 版。結果は読み出さず、ヒストグラムだけ求める）。
fn render_preview(
    engine: &Engine,
    gpu: Option<&GpuRenderer>,
    source: &PhotoSource,
    settings: &DevelopSettings,
    quality: RenderQuality,
) -> CliResult<()> {
    let request = PreviewRequest::new(quality, PREVIEW_LONG_EDGE)
        .with_output(OutputTarget::assumed_srgb_display()?)
        .with_histogram();
    match gpu {
        Some(g) => {
            let options = GpuPreviewOptions {
                download_b3: false,
                download_output: false,
                gpu_draft_a1: true,
            };
            g.render_preview(engine, source, settings, &request, &options)?;
        }
        None => {
            engine.render_preview_with(source, settings, &request)?;
        }
    }
    Ok(())
}

/// 計測の中で起きたエラーを覚えておく（計測の関数はエラーを返せないため）。
fn keep_err(slot: &mut Option<CliError>, r: CliResult<()>) {
    if let Err(e) = r
        && slot.is_none()
    {
        *slot = Some(e);
    }
}

fn next_wb(k: &mut f32) -> WhiteBalance {
    *k = if *k + WB_SWEEP.2 > WB_SWEEP.1 {
        WB_SWEEP.0
    } else {
        *k + WB_SWEEP.2
    };
    WhiteBalance::Custom {
        temperature_k: *k,
        tint: 0.0,
    }
}

fn bench_preview(ctx: &Ctx, source: &PhotoSource) -> CliResult<Vec<BenchResult>> {
    let engine = Engine::new(EngineConfig::default())?;
    let gpu = ctx.gpu.as_ref();
    let clear = || {
        engine.clear_caches();
        if let Some(g) = gpu {
            g.clear_caches();
        }
    };
    let mut results = Vec::new();
    let mut err: Option<CliError> = None;

    let preview = preview_size(ctx.size);
    let preview_note = format!("{}x{}", preview.0, preview.1);
    let preview_bench = |name: &str| base(name, true, ctx).note("preview", preview_note.clone());

    // (1) 露光量のドラッグ（段階 C）。最初に 1 回描いて A1・ガイドをキャッシュに入れる。
    let mut s = bench_settings();
    render_preview(&engine, gpu, source, &s, RenderQuality::Final)?;
    let mut ev = 0.0f32;
    let r = preview_bench("cli_preview_stage_c")
        .target(STAGE_C_TARGET)
        .develop_settings(&s)
        .run_warm(|| {
            ev = if ev > 1.0 { 0.0 } else { ev + 0.1 };
            s.exposure_ev = ev;
            keep_err(
                &mut err,
                render_preview(&engine, gpu, source, &s, RenderQuality::Final),
            );
        })?;
    results.push(r);

    // (1) WB のドラッグ（簡易処理）。
    let mut s = bench_settings();
    let mut k = WB_SWEEP.0;
    let r = preview_bench("cli_preview_wb_drag_draft")
        .target(WB_DRAG_TARGET)
        .develop_settings(&s)
        .run_warm(|| {
            s.white_balance = next_wb(&mut k);
            keep_err(
                &mut err,
                render_preview(&engine, gpu, source, &s, RenderQuality::Draft),
            );
        })?;
    results.push(r);

    // (2) WB を離した後の最終品質（コールド: 毎回キャッシュを捨てる / ウォーム: WB を変えながら）。
    let mut s = bench_settings();
    let r = preview_bench("cli_preview_wb_release_final")
        .target(WB_RELEASE_TARGET)
        .develop_settings(&s)
        .run_cold(clear, || {
            s.white_balance = next_wb(&mut k);
            keep_err(
                &mut err,
                render_preview(&engine, gpu, source, &s, RenderQuality::Final),
            );
        })?;
    results.push(r);
    let mut s = bench_settings();
    let r = preview_bench("cli_preview_wb_release_final")
        .target(WB_RELEASE_TARGET)
        .develop_settings(&s)
        .run_warm(|| {
            s.white_balance = next_wb(&mut k);
            keep_err(
                &mut err,
                render_preview(&engine, gpu, source, &s, RenderQuality::Final),
            );
        })?;
    results.push(r);
    match err {
        Some(e) => Err(e),
        None => Ok(results),
    }
}

/// 書き出しを 1 回行う（内訳を記録する）。使った側（GPU / CPU）を返す。
#[allow(clippy::too_many_arguments)]
fn export_once(
    t: &mut PhaseTimer,
    input: &BenchInput,
    worker: &mut Worker,
    engine: &Engine,
    gpu: Option<&GpuRenderer>,
    settings: &DevelopSettings,
    export: &ExportSettings,
    dest: &Path,
    protected: &ProtectedFiles,
) -> CliResult<&'static str> {
    let start = Instant::now();
    let (source, _) = input.load(worker)?;
    if input.file.is_some() {
        t.record("decode", start.elapsed());
    }
    let start = Instant::now();
    let outcome = genzo_gpu::export_with_fallback(
        gpu,
        engine,
        &source,
        settings,
        export,
        &ExportOptions::default(),
        &NeverCancel,
    )?;
    let backend = match outcome.backend {
        ExportBackend::Gpu => "gpu",
        ExportBackend::Cpu { .. } => "cpu",
    };
    let image = outcome.image;
    let (w, h) = (image.width, image.height);
    let pixels = match image.pixels {
        ExportPixels::Rgb8(v) => DynRgbImage::Rgb8(RgbImage8::from_raw(w, h, v)?),
        ExportPixels::Rgb16(v) => DynRgbImage::Rgb16(RgbImage16::from_raw(w, h, v)?),
    };
    t.record("render", start.elapsed());
    let start = Instant::now();
    let exif = ExifData::from_photo_metadata(&source_metadata(&source));
    let bytes = genzo_media::encode_export(&pixels, export, Some(&exif))?;
    t.record("encode", start.elapsed());
    let start = Instant::now();
    let path = dest.join(format!("bench.{}", export.format.extension()));
    genzo_media::write_atomically(&path, ConflictPolicy::Overwrite, protected, |f| {
        f.write_all(&bytes)
            .map_err(|e| genzo_media::MediaError::Io {
                path: Some(path.clone()),
                source: e,
            })
    })?;
    t.record("write", start.elapsed());
    Ok(backend)
}

/// 入力の撮影情報（RAW 以外は既定の値。計測では Exif の中身は時間にほとんど影響しない）。
fn source_metadata(source: &PhotoSource) -> genzo_model::PhotoMetadata {
    match &source.image {
        SourceImage::Raw(r) => r.metadata.clone(),
        SourceImage::Working { .. } => genzo_model::PhotoMetadata::default(),
    }
}

fn bench_export(ctx: &Ctx, input: &BenchInput, worker: &mut Worker) -> CliResult<Vec<BenchResult>> {
    let engine = Engine::new(EngineConfig::default())?;
    let gpu = ctx.gpu.as_ref();
    let settings = bench_settings();
    let export = ExportSettings {
        on_conflict: ConflictPolicy::Overwrite,
        ..ExportSettings::default()
    };
    let dest = tempfile::tempdir().map_err(|e| CliError::io(std::env::temp_dir(), e))?;
    let mut protected = ProtectedFiles::new();
    if let Some((p, _)) = &input.file {
        protected.insert(p);
    }
    let mut err: Option<CliError> = None;
    let mut backends: Vec<&'static str> = Vec::new();
    let r = base("cli_export_jpeg", false, ctx)
        .target(EXPORT_TARGET)
        .develop_settings(&settings)
        .note("format", "jpeg_q90_srgb")
        .run_cold_phased(
            || {
                engine.clear_caches();
                if let Some(g) = gpu {
                    g.clear_caches();
                }
            },
            |t| match export_once(
                t,
                input,
                worker,
                &engine,
                gpu,
                &settings,
                &export,
                dest.path(),
                &protected,
            ) {
                Ok(b) => backends.push(b),
                Err(e) => keep_err(&mut err, Err(e)),
            },
        )?;
    if let Some(e) = err {
        return Err(e);
    }
    let mut r = r;
    // GPU から CPU 版に切り替えた回があれば、処理した側を記録に残す（比べる相手を分けるため）。
    if backends.contains(&"cpu") && ctx.gpu.is_some() {
        r.notes
            .insert("backend".to_owned(), "gpu_with_cpu_fallback".to_owned());
    }
    Ok(vec![r])
}

/// `bench`。
pub fn run(g: &GlobalArgs, out: Output, a: BenchArgs) -> CliResult<Status> {
    let mut worker = Worker::spawn()?;
    let input = BenchInput::prepare(&a, &mut worker)?;
    out.progress("入力を展開しています…");
    let (source, decoder) = input.load(&mut worker)?;
    let size = source.image.sensor_size();
    let gpu = gpu_renderer(g.gpu, out);
    if let Some(r) = &gpu {
        r.compile_all()?;
    }
    let mut env = EnvironmentInfo::detect().with_library("genzo", env!("CARGO_PKG_VERSION"));
    if let Some(r) = &gpu {
        let s = r.context().summary();
        env = env.with_gpu(
            s.name.clone(),
            Some(format!("{} {}", s.driver, s.driver_info).trim().to_owned()),
        );
        if s.is_software {
            out.warn("GPU はソフトウェアの実装です。時間は性能の判断に使えません");
        }
    }
    if let Some(d) = &decoder {
        env = env.with_library("raw_decoder", d.clone());
    }
    if a.target == BenchTarget::Preview {
        let p = preview_size(size);
        if p.0.max(p.1) < PREVIEW_LONG_EDGE {
            out.warn(format!(
                "入力（{} × {}）の長辺が {PREVIEW_LONG_EDGE} 未満のため、プレビューは {} × {} になり、1.8 節の解像度（2560 × 1707）ではありません",
                size.0, size.1, p.0, p.1
            ));
        }
    }
    let ctx = Ctx {
        runs: a.runs,
        env,
        input: input.input.clone(),
        how: input.how,
        size,
        is_raw: matches!(source.image, SourceImage::Raw(_)),
        gpu,
    };
    out.progress(format!(
        "計測しています（{}、{} × {}、{}）…",
        match a.target {
            BenchTarget::Preview => "プレビュー",
            BenchTarget::Export => "書き出し",
        },
        size.0,
        size.1,
        if ctx.gpu.is_some() { "GPU" } else { "CPU" }
    ));
    let results = match a.target {
        BenchTarget::Preview => bench_preview(&ctx, &source)?,
        BenchTarget::Export => bench_export(&ctx, &input, &mut worker)?,
    };
    worker.shutdown(out);
    let recorder = match &a.out {
        Some(d) => BenchRecorder::new(d.clone()),
        None => BenchRecorder::from_env(),
    };
    let mut reports: Vec<BenchReport> = Vec::new();
    for r in &results {
        reports.push(recorder.record(r)?);
    }
    let regressed = regression_fails(&reports);
    if reports
        .iter()
        .any(|r| r.regressed() && !r.result.meets_iteration_rule)
    {
        out.warn(
            "回数が 1.8 節の規則に足りない計測で悪化を検出しましたが、終了コードには反映しません（--runs を省くと規則の回数で測ります）",
        );
    }
    if out.json {
        let items: Vec<serde_json::Value> = reports
            .iter()
            .map(|r| {
                json!({
                    "name": r.result.name,
                    "path": r.path,
                    "passed": r.passed(),
                    "target_met": r.result.target_met(),
                    "regressed": r.regressed(),
                    "result": json_value(&r.result),
                    "comparison": r.comparison,
                })
            })
            .collect();
        out.print_json(&json!({ "reports": items, "regressed": regressed }));
    } else {
        for r in &reports {
            out.line(r.to_string());
            out.line("");
        }
        out.line(
            "※ 時間は呼び出しから結果が戻るまでで、入力から画面への表示までではありません（05 の 1.8 節）",
        );
    }
    Ok(if regressed {
        Status::Failure
    } else {
        Status::Success
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_follow_the_requirements() {
        assert_eq!(STAGE_C_TARGET.as_millis(), 33);
        // 15fps = 66.67ms。
        assert_eq!(WB_DRAG_TARGET.as_micros(), 66_666);
        assert_eq!(WB_RELEASE_TARGET.as_millis(), 500);
        assert_eq!(EXPORT_TARGET.as_secs(), 3);
        assert_eq!(PREVIEW_LONG_EDGE, genzo_pipeline::DEFAULT_PREVIEW_LONG_EDGE);
    }

    #[test]
    fn synthetic_raw_has_the_requested_size() {
        let raw = synthetic_raw(64, 48).unwrap();
        assert_eq!((raw.width, raw.height), (64, 48));
        raw.validate().unwrap();
    }

    #[test]
    fn preview_size_follows_the_long_edge_rule() {
        // α7 IV（7008 × 4672）は 1.8 節の 2560 × 1707（3:2）になる。
        assert_eq!(preview_size(DEFAULT_SYNTHETIC_SIZE), (2560, 1707));
        assert_eq!(preview_size((4672, 7008)), (1707, 2560), "縦位置");
        assert_eq!(preview_size((640, 426)), (640, 426), "拡大しない");
        assert_eq!(preview_size((2560, 1707)), (2560, 1707));
    }

    fn report(samples_ms: &[u64], previous_p95: Option<f64>) -> BenchReport {
        let samples: Vec<Duration> = samples_ms
            .iter()
            .map(|&v| Duration::from_millis(v))
            .collect();
        let result = Bench::batch("x")
            .environment(EnvironmentInfo::detect())
            .from_samples(genzo_testkit::bench::Condition::Cold, &samples, &[])
            .unwrap();
        let comparison = previous_p95.map(|p| {
            let mut previous = result.clone();
            previous.stats.p95_ms = p;
            genzo_testkit::bench::compare_results(
                &previous,
                &result,
                &genzo_testkit::bench::RegressionPolicy::default(),
            )
        });
        BenchReport {
            path: PathBuf::from("x.json"),
            result,
            comparison,
        }
    }

    #[test]
    fn only_regressions_with_enough_runs_fail() {
        // 5 回（一括処理の規則）で前回の 95 パーセンタイル 10 ms → 今回 50 ms は悪化で失敗。
        let enough = report(&[50, 50, 50, 50, 50], Some(10.0));
        assert!(enough.regressed() && enough.result.meets_iteration_rule);
        assert!(regression_fails(std::slice::from_ref(&enough)));
        // 2 回（規則に足りない）の悪化は終了コードにしない。
        let few = report(&[50, 50], Some(10.0));
        assert!(few.regressed() && !few.result.meets_iteration_rule);
        assert!(!regression_fails(std::slice::from_ref(&few)));
        // 悪化していなければ失敗にしない。比べる相手がない場合も同じ。
        assert!(!regression_fails(&[report(&[10; 5], Some(10.0))]));
        assert!(!regression_fails(&[report(&[10; 5], None)]));
        assert!(regression_fails(&[few, enough]));
    }

    #[test]
    fn wb_sweep_wraps_around() {
        let mut k = WB_SWEEP.1 - 10.0;
        let WhiteBalance::Custom { temperature_k, .. } = next_wb(&mut k) else {
            panic!()
        };
        assert_eq!(temperature_k, WB_SWEEP.0);
        let WhiteBalance::Custom { temperature_k, .. } = next_wb(&mut k) else {
            panic!()
        };
        assert_eq!(temperature_k, WB_SWEEP.0 + WB_SWEEP.2);
    }
}
