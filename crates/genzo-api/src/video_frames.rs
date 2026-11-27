//! 動画のフレームの取り出し: スクラブ表示（VID-05）と、動画からの静止画の切り出し（VID-07）。
//!
//! どちらも、利用者の ffmpeg（同梱しない。動画の再生用のファイルと同じ道具。`playback` の `ffmpeg_tools`）で、
//! 指定した時刻のフレームを 1 枚取り出す（genzo-media の `FfmpegTools::extract_frame_png`。動画のサムネイルと
//! 同じ引数で、入力は読むだけ。DATA-01）。ffmpeg がなければ使えない（理由を返す）。
//!
//! # スクラブ表示（[`Core::video_scrub_frame`]）
//!
//! - 動画の長さを [`SCRUB_FRAMES`] 等分した、それぞれの区間の真ん中のフレーム（長辺 320 の JPEG。sRGB と
//!   みなす。サムネイルと同じ）。画面は、グリッドの動画のサムネイルの上でカーソルを左右に動かした位置の番号の
//!   フレームを出す。
//! - 作ったフレームは **メモリに覚える**（[`MAX_SCRUB_CACHE_FRAMES`] 枚まで。古いものから捨てる）。ディスクには
//!   保存しない（アプリを開き直すと、カーソルを載せたときに作り直す）。
//! - 1 度に 1 枚だけ作る（ffmpeg のプロセスを並べない）。作っている間のほかの要求は、待たずに
//!   [`ApiError::Cancelled`]（画面は、次にカーソルが動いたときに頼み直す）。
//!
//! # 静止画の切り出し（[`Core::extract_video_frame`]）
//!
//! - 指定した時刻のフレームを、元の解像度のまま JPEG（品質 [`STILL_JPEG_QUALITY`]。sRGB の ICC 付き）にして、
//!   動画と同じフォルダに `<動画の名前>-frame-<時分秒ミリ秒>.jpg` で保存し（同じ名前があれば連番を付ける。
//!   上書きしない）、カタログに登録する（P3 のジョブ。結果は [`JobResult::VideoFrame`]）。
//! - 撮影日時は「動画の撮影日時 ＋ その時刻」（動画の撮影日時が分かるとき）。位置・レンズなどは書かない。
//! - **色**: 動画の色を変換しない（サムネイルと同じく、BT.709 の動画を sRGB とみなす。Log・HDR の動画は、
//!   記録のままの眠い色・明るさで切り出される。LUT の適用は VID-06 で、まだない）。
//! - ffmpeg が書いた PNG は本体で読む（ffmpeg のエンコーダーが作ったもので、元の動画のバイト列ではない）。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

use chrono::TimeDelta;
use genzo_media::{
    CacheSpec, DynRgbImage, ExifData, FfmpegTools, MediaError, ProtectedFiles, WriteOutcome,
    decode_image_bytes, render_cache_jpeg,
};
use genzo_model::{
    ConflictPolicy, ExportFormat, ExportSettings, ExportSize, FileId, OutputColorSpace, VariantId,
};
use genzo_worker::Lane;
use parking_lot::Mutex;

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::jobs::{JobCtx, spawn_job};
use crate::previews::{VariantFile, refresh_variant_file};
use crate::types::{JobKind, JobResult, VideoFrameReport};

/// スクラブ表示のフレームの数（動画の長さを等分する。**仮置き**: 10）。
pub const SCRUB_FRAMES: u32 = 10;

/// スクラブ表示のフレームの大きさと品質（サムネイルと同じ長辺 320。品質は少し下げる。**仮置き**）。
pub const SCRUB_SPEC: CacheSpec = CacheSpec {
    long_edge: 320,
    quality: 72,
};

/// メモリに覚えるスクラブ表示のフレームの数の上限（**仮置き**: 400。1 枚 20 KB として約 8 MB）。
pub const MAX_SCRUB_CACHE_FRAMES: usize = 400;

/// 切り出す静止画の JPEG の品質（**仮置き**: 95）。
pub const STILL_JPEG_QUALITY: u8 = 95;

/// スクラブ表示のフレームの鍵（ファイル・リビジョン・フレームの番号）。
type ScrubKey = (FileId, u32, u32);

/// スクラブ表示のフレームのキャッシュ（新しいものを後ろに置く）。
#[derive(Default)]
pub(crate) struct ScrubCache {
    frames: VecDeque<(ScrubKey, Arc<Vec<u8>>)>,
}

impl ScrubCache {
    fn get(&mut self, key: ScrubKey) -> Option<Arc<Vec<u8>>> {
        let at = self.frames.iter().position(|(k, _)| *k == key)?;
        // 使ったものを後ろへ（古いものから捨てるため）。
        let entry = self.frames.remove(at)?;
        let jpeg = Arc::clone(&entry.1);
        self.frames.push_back(entry);
        Some(jpeg)
    }

    fn put(&mut self, key: ScrubKey, jpeg: Arc<Vec<u8>>) {
        self.frames.retain(|(k, _)| *k != key);
        self.frames.push_back((key, jpeg));
        while self.frames.len() > MAX_SCRUB_CACHE_FRAMES {
            self.frames.pop_front();
        }
    }
}

/// スクラブ表示の `index` 番目のフレームの時刻（秒）。長さが分からなければ、先頭（`index` が 0 のときだけ）。
pub(crate) fn scrub_position_s(duration_s: Option<f64>, index: u32) -> Option<f64> {
    if index >= SCRUB_FRAMES {
        return None;
    }
    match duration_s.filter(|d| d.is_finite() && *d > 0.0) {
        Some(d) => Some(d * (f64::from(index) + 0.5) / f64::from(SCRUB_FRAMES)),
        None => (index == 0).then_some(0.0),
    }
}

/// 切り出す静止画のファイル名（`<動画の名前（拡張子の前）>-frame-<時><分><秒><ミリ秒>.jpg`）。
pub(crate) fn still_file_name(video_name: &str, position_ms: u64) -> String {
    let stem = Path::new(video_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("video");
    let (h, m, s, ms) = (
        position_ms / 3_600_000,
        position_ms / 60_000 % 60,
        position_ms / 1000 % 60,
        position_ms % 1000,
    );
    format!("{stem}-frame-{h:02}{m:02}{s:02}{ms:03}.jpg")
}

fn media_err(e: MediaError) -> ApiError {
    ApiError::from(e)
}

/// ffmpeg・ffprobe（なければ、使えない理由）。
fn tools(inner: &Inner) -> Result<FfmpegTools, ApiError> {
    crate::playback::ffmpeg_tools(&inner.config)
        .filter(FfmpegTools::is_available)
        .ok_or_else(|| {
            ApiError::InvalidArgument(
                "ffmpeg・ffprobe が見つからないため、動画のフレームを取り出せません（FFmpeg を入れて、見つからなければ設定で場所を指定してください）"
                    .to_owned(),
            )
        })
}

/// 動画のファイル（ディスクと照らし合わせた後。見つからなければ [`ApiError::FileAccess`]）。
fn located_video(
    inner: &Arc<Inner>,
    variant_id: VariantId,
) -> Result<(VariantFile, std::path::PathBuf), ApiError> {
    let file = crate::playback::video_file(inner, variant_id)?;
    let token = genzo_jobs::CancellationToken::new();
    let missing = |f: &VariantFile| ApiError::FileAccess {
        path: f
            .path
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from(&f.name)),
        message: "ファイルが見つかりません".to_owned(),
    };
    let Some(file) = refresh_variant_file(inner, &file, Lane::Batch, &token)? else {
        return Err(missing(&file));
    };
    let path = file.path.clone().ok_or_else(|| missing(&file))?;
    Ok((file, path))
}

/// `position_s` 秒のフレーム（8bit の RGB）。その位置にフレームがなければ [`ApiError::InvalidArgument`]。
fn frame_at(
    tools: &FfmpegTools,
    path: &Path,
    probe: &genzo_media::VideoProbe,
    position_s: f64,
) -> Result<DynRgbImage, ApiError> {
    let png = tools
        .extract_frame_png(path, probe, position_s)
        .map_err(media_err)?;
    if png.is_empty() {
        return Err(ApiError::InvalidArgument(format!(
            "動画の {position_s:.3} 秒の位置にフレームがありません"
        )));
    }
    Ok(decode_image_bytes(&png).map_err(media_err)?.pixels)
}

fn run_extract(
    ctx: &JobCtx<'_>,
    variant_id: VariantId,
    position_s: f64,
) -> Result<VideoFrameReport, ApiError> {
    let inner = ctx.inner;
    let tools = tools(inner)?;
    let (file, path) = located_video(inner, variant_id)?;
    let probe = tools.probe(&path).map_err(media_err)?;
    let position_ms = (position_s * 1000.0).round() as u64;
    let pixels = frame_at(&tools, &path, &probe, position_s)?;
    if ctx.is_cancelled() {
        return Err(ApiError::Cancelled);
    }
    let dest = path
        .parent()
        .ok_or_else(|| ApiError::Internal("動画のフォルダがありません".to_owned()))?;
    // 撮影日時: 動画の撮影日時 ＋ 切り出した時刻。
    let capture = inner.with_catalog(|c| c.capture_time(file.asset_id))?;
    let mut exif = ExifData::default().with_capture_time(&capture);
    if let Some(t) = exif.datetime_original {
        exif.datetime_original = t.checked_add_signed(TimeDelta::milliseconds(position_ms as i64));
    }
    // 原本の保護（6.4 節）: 元の動画と、同じフォルダにあるカタログのファイルは上書きしない。
    let mut protected = ProtectedFiles::new();
    protected.extend([path.clone()]);
    protected.extend(crate::export::catalog_files_in(inner, dest)?);
    let settings = ExportSettings {
        format: ExportFormat::Jpeg {
            quality: STILL_JPEG_QUALITY,
        },
        color_space: OutputColorSpace::Srgb,
        size: ExportSize::Original,
        on_conflict: ConflictPolicy::Sequence,
        remove_keywords: true,
        ..ExportSettings::default()
    };
    let (width, height) = pixels.dimensions();
    let written = match genzo_media::export_image_cancellable(
        &pixels,
        &settings,
        Some(&exif),
        &dest.join(still_file_name(&file.name, position_ms)),
        &protected,
        &|| ctx.is_cancelled(),
    )
    .map_err(media_err)?
    {
        WriteOutcome::Written { path, .. } => path,
        WriteOutcome::Skipped { existing } => {
            return Err(ApiError::Internal(format!(
                "同じ名前のファイルがあるため、保存しませんでした: {}",
                existing.display()
            )));
        }
    };
    let mut report = VideoFrameReport {
        source: variant_id,
        position_ms,
        width,
        height,
        path: written.clone(),
        registered: None,
        registration_error: None,
    };
    match crate::composite::register_written(ctx, &written) {
        Ok((v, problem)) => {
            report.registered = Some(v);
            report.registration_error = problem;
        }
        Err(e) if e.is_cancelled() => {
            // 登録の前に取り消された: 書いたファイルを消す。
            let _ = std::fs::remove_file(&written);
            return Err(e);
        }
        // ファイルは残す（取り込み直せる）。
        Err(e) => report.registration_error = Some(e.to_string()),
    }
    Ok(report)
}

impl Core {
    /// スクラブ表示（VID-05）の `index` 番目（0 から [`SCRUB_FRAMES`] − 1）のフレーム（長辺 320 の JPEG。
    /// モジュールの doc）。
    ///
    /// エラー: 動画でない・番号が範囲の外・長さの分からない動画の 1 番目以降なら [`ApiError::InvalidArgument`]、
    /// ffmpeg がなければ [`ApiError::InvalidArgument`]、ほかのフレームを作っている間は [`ApiError::Cancelled`]。
    pub fn video_scrub_frame(
        &self,
        variant_id: VariantId,
        index: u32,
    ) -> Result<Arc<Vec<u8>>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if index >= SCRUB_FRAMES {
            return Err(ApiError::InvalidArgument(format!(
                "スクラブ表示のフレームの番号は 0〜{} です",
                SCRUB_FRAMES - 1
            )));
        }
        let file = crate::playback::video_file(inner, variant_id)?;
        let key = (file.file_id, file.revision, index);
        if let Some(jpeg) = inner.scrub.cache.lock().get(key) {
            return Ok(jpeg);
        }
        // 1 度に 1 枚だけ作る（待たない）。
        let Some(_one) = inner.scrub.busy.try_lock() else {
            return Err(ApiError::Cancelled);
        };
        let tools = tools(inner)?;
        let (file, path) = located_video(inner, variant_id)?;
        let probe = tools.probe(&path).map_err(media_err)?;
        let position = scrub_position_s(probe.metadata.duration_s, index).ok_or_else(|| {
            ApiError::InvalidArgument(
                "動画の長さが分からないため、先頭のフレームだけを出せます".to_owned(),
            )
        })?;
        let pixels = frame_at(&tools, &path, &probe, position)?;
        let srgb = genzo_color::IccProfile::standard(genzo_color::StandardProfile::Srgb)
            .map_err(|e| ApiError::Internal(format!("sRGB のプロファイルを作れません: {e}")))?;
        let jpeg = Arc::new(
            render_cache_jpeg(&pixels, &srgb, genzo_model::Orientation::Normal, SCRUB_SPEC)
                .map_err(media_err)?
                .bytes,
        );
        inner
            .scrub
            .cache
            .lock()
            .put((file.file_id, file.revision, index), Arc::clone(&jpeg));
        Ok(jpeg)
    }

    /// 動画の `position_seconds` 秒のフレームを静止画（JPEG）として切り出し、動画と同じフォルダに保存して
    /// カタログに登録する（VID-07。P3 のジョブ。ジョブの ID を返す。結果は [`JobResult::VideoFrame`]。モジュールの
    /// doc）。
    ///
    /// エラー（ジョブを始める前）: 動画でない・時刻が負・有限でないなら [`ApiError::InvalidArgument`]、ffmpeg が
    /// なければ [`ApiError::InvalidArgument`]。
    pub fn extract_video_frame(
        &self,
        variant_id: VariantId,
        position_seconds: f64,
    ) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if !position_seconds.is_finite() || !(0.0..=86_400.0 * 7.0).contains(&position_seconds) {
            return Err(ApiError::InvalidArgument(
                "切り出す時刻は 0 秒以上で指定してください".to_owned(),
            ));
        }
        crate::playback::video_file(inner, variant_id)?;
        tools(inner)?;
        Ok(spawn_job(
            inner,
            JobKind::VideoFrame,
            "動画からの静止画の切り出し",
            move |ctx| run_extract(ctx, variant_id, position_seconds).map(JobResult::VideoFrame),
        ))
    }
}

/// スクラブ表示の状態（[`Inner`] の `scrub`）。
#[derive(Default)]
pub(crate) struct ScrubState {
    pub(crate) cache: Mutex<ScrubCache>,
    /// フレームを作っている間のロック（1 度に 1 枚）。
    pub(crate) busy: Mutex<()>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_positions_are_the_middles_of_equal_parts() {
        assert_eq!(scrub_position_s(Some(100.0), 0), Some(5.0));
        assert_eq!(scrub_position_s(Some(100.0), 9), Some(95.0));
        assert_eq!(scrub_position_s(Some(100.0), SCRUB_FRAMES), None);
        // 長さが分からなければ、先頭だけ。
        assert_eq!(scrub_position_s(None, 0), Some(0.0));
        assert_eq!(scrub_position_s(None, 1), None);
        assert_eq!(scrub_position_s(Some(f64::NAN), 0), Some(0.0));
        assert_eq!(scrub_position_s(Some(0.0), 3), None);
    }

    #[test]
    fn still_names_carry_the_position() {
        assert_eq!(
            still_file_name("DJI_0012.MP4", 83_456),
            "DJI_0012-frame-000123456.jpg"
        );
        assert_eq!(
            still_file_name("旅 01.mov", 3_723_004),
            "旅 01-frame-010203004.jpg"
        );
        assert_eq!(still_file_name(".mp4", 0), ".mp4-frame-000000000.jpg");
        assert_eq!(still_file_name("", 0), "video-frame-000000000.jpg");
    }

    #[test]
    fn scrub_cache_forgets_the_least_recently_used() {
        let mut c = ScrubCache::default();
        let key = |n: u32| (FileId::new(1), 1, n);
        for n in 0..MAX_SCRUB_CACHE_FRAMES as u32 {
            c.put(key(n), Arc::new(vec![n as u8]));
        }
        // 0 番を使ってから 1 枚足すと、いちばん古い 1 番が消える。
        assert!(c.get(key(0)).is_some());
        c.put(key(9999), Arc::new(vec![1]));
        assert!(c.get(key(0)).is_some());
        assert!(c.get(key(1)).is_none());
        assert!(c.get(key(9999)).is_some());
        assert_eq!(c.frames.len(), MAX_SCRUB_CACHE_FRAMES);
    }
}
