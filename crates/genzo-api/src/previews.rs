//! サムネイル（L0）と標準プレビュー（L1）（04 の 4 章・4.1 節、PRV-01・PRV-02・PRV-04）と、
//! 元ファイルの展開（段階 A0）。
//!
//! - 取り込みの直後は、ワーカーが作る B5 の JPEG（RAW は埋め込み JPEG、それ以外は画像そのもの、動画は
//!   代表フレーム）を L0 として thumbs.db に保存する（PRV-01）。キャッシュキーは元ファイルとリビジョン
//!   から作る（[`source_thumb_rev`]）。
//! - 現像設定が変わったら、バックグラウンド（P3）で現像結果から L0（長辺 320）と L1（長辺 2560。
//!   ファイルのキャッシュ、上限容量の LRU）を作り直す（PRV-02）。B5（Display P3・IEC 61966-2-1・ICC 付きの
//!   JPEG）は、genzo-pipeline のステージ 17b のキャッシュの出力（`OutputTarget::CacheDisplayP3`）から作る。
//!   L1 は 8bit に量子化して genzo-media の JPEG（Display P3 の ICC 付き）に、L0 は 16bit に量子化して
//!   genzo-media の `render_cache_jpeg`（リニアな値の面積平均で長辺 320 に縮小）にする。
//! - **古い世代の結果で新しいキャッシュを上書きしない**（4.1 節）: 作り始めたときの現像設定のハッシュと
//!   ファイルのリビジョンを覚えておき、保存の直前に（カタログのロックの中で）今の値と比べる。違えば
//!   捨てる。保存はカタログのロックを持ったまま行うので、比べてから保存するまでの間に設定は変わらない。
//! - L1 のキャッシュキーは現在の設定から求めるので、設定が変わると古い L1 は使われなくなり、上限容量の
//!   LRU で消える（4.1 節「回収」）。

use std::path::PathBuf;
use std::sync::Arc;

use genzo_catalog::{Catalog, PairClass, pair_class};
use genzo_jobs::{CancellationToken, Priority};
use genzo_media::jpeg::encode_jpeg;
use genzo_media::{
    CacheSpec, DynRgbImage, RgbImage8, RgbImage16, cache_icc_profile, render_cache_jpeg,
};
use genzo_model::{
    AssetKind, CacheColorSpace, CacheFormat, CacheKey, CacheKind, DevelopSettings, FileId,
    FileStatus, Orientation, PhotoMetadata, RenderQuality, VariantId,
};
use genzo_pipeline::finish::output::{Dither, quantize_u8, quantize_u16};
use genzo_pipeline::{
    ColorContract, OutputTarget, PhotoSource, PreviewRequest, RgbImage, SourceId, SourceImage,
};
use genzo_worker::Lane;

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::{Event, WarningCode};
use crate::jobs::spawn_job;
use crate::types::{JobKind, JobResult, RegenerateReport, SourceInfo, ThumbnailData, VariantIssue};

/// 現像結果から作る L0 の保存形式の版（`CacheFormat::format_version`。JPEG の品質などを変えたら上げる）。
pub const L0_FORMAT_VERSION: u32 = 1;
/// 現像結果から作る L1 の保存形式の版。
pub const L1_FORMAT_VERSION: u32 = 1;

/// ワーカーが元ファイルから作った L0（埋め込み JPEG・画像そのもの・動画の代表フレーム）のキャッシュキー
/// （16 進数 64 文字）。元ファイルとリビジョンから決まる（現像設定には依存しない）。
pub(crate) fn source_thumb_rev(file_id: FileId, revision: u32) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"genzo.thumb.source.v1\0");
    h.update(&file_id.get().to_le_bytes());
    h.update(&revision.to_le_bytes());
    h.update(&CacheSpec::L0_THUMBNAIL.long_edge.to_le_bytes());
    h.update(&[CacheSpec::L0_THUMBNAIL.quality]);
    h.finalize().to_hex().to_string()
}

/// 現像結果から作る L0 / L1 のキャッシュキー（4.1 節。16 進数 64 文字）。
pub(crate) fn rendered_key(
    file_id: FileId,
    revision: u32,
    settings: &DevelopSettings,
    kind: CacheKind,
) -> String {
    let (spec, version) = match kind {
        CacheKind::L1Preview => (CacheSpec::L1_PREVIEW, L1_FORMAT_VERSION),
        _ => (CacheSpec::L0_THUMBNAIL, L0_FORMAT_VERSION),
    };
    CacheKey::for_settings(
        file_id,
        revision,
        settings,
        (spec.long_edge, 0),
        RenderQuality::Final,
        CacheFormat {
            kind,
            color_space: CacheColorSpace::DisplayP3,
            format_version: version,
        },
    )
    .hex()
}

/// variant の主となるファイルの情報。
#[derive(Debug, Clone)]
pub(crate) struct VariantFile {
    pub variant_id: VariantId,
    pub asset_id: genzo_model::AssetId,
    pub kind: AssetKind,
    pub file_id: FileId,
    pub revision: u32,
    pub name: String,
    pub path: Option<PathBuf>,
    pub status: FileStatus,
    pub is_master: bool,
    pub variant_name: Option<String>,
}

/// variant の主となるファイルの情報を読む。
pub(crate) fn variant_file(cat: &Catalog, variant_id: VariantId) -> Result<VariantFile, ApiError> {
    let s = cat
        .variant_summaries(&[variant_id])?
        .pop()
        .ok_or_else(|| ApiError::NotFound(format!("variant {variant_id}")))?;
    let loc = cat.file_location(s.file_id)?;
    Ok(VariantFile {
        variant_id,
        asset_id: s.asset_id,
        kind: s.kind,
        file_id: s.file_id,
        revision: s.file_revision,
        name: s.file_name,
        path: loc.absolute_path(),
        status: s.file_status,
        is_master: s.is_master,
        variant_name: s.variant_name,
    })
}

/// 展開した入力（段階 A0）と、撮影情報。
pub(crate) struct LoadedSource {
    pub source: PhotoSource,
    pub metadata: PhotoMetadata,
    pub info: SourceInfo,
}

/// 元ファイルをワーカーで展開する（RAW は `DecodeRaw`、それ以外は `DecodeImage`。SEC-05）。
///
/// ファイルが見つからなければ `status = missing` にして [`ApiError::FileAccess`]。
pub(crate) fn load_source(
    inner: &Inner,
    file: &VariantFile,
    lane: Lane,
    token: &CancellationToken,
) -> Result<LoadedSource, ApiError> {
    let path = file.path.clone().ok_or_else(|| ApiError::FileAccess {
        path: PathBuf::from(&file.name),
        message: "ボリュームのマウント先が分かりません".to_owned(),
    })?;
    if !path.is_file() {
        mark_missing(inner, file.file_id, file.variant_id, &path);
        return Err(ApiError::FileAccess {
            path,
            message: "ファイルが見つかりません".to_owned(),
        });
    }
    let pool = inner.pool()?;
    let id = SourceId::new(file.file_id, file.revision);
    let loaded = if pair_class(&file.name) == PairClass::Raw {
        let frame = pool.decode_raw(lane, &path, token)?;
        let info = SourceInfo {
            is_raw: true,
            width: frame.image.width,
            height: frame.image.height,
            decoder: frame.decoder_id.clone(),
        };
        let metadata = frame.image.metadata.clone();
        LoadedSource {
            source: PhotoSource::new(id, SourceImage::Raw(Arc::new(frame.image)))?,
            metadata,
            info,
        }
    } else {
        let img = pool.decode_image(lane, &path, token)?;
        let (w, h) = (img.width(), img.height());
        let metadata = img.info.metadata.clone();
        let image = RgbImage::from_vec(w, h, ColorContract::B2Working, img.pixels)?;
        LoadedSource {
            source: PhotoSource::new(
                id,
                SourceImage::Working {
                    image: Arc::new(image),
                    orientation: metadata.orientation,
                },
            )?,
            metadata,
            info: SourceInfo {
                is_raw: false,
                width: w,
                height: h,
                decoder: None,
            },
        }
    };
    Ok(loaded)
}

/// ファイルを `status = missing` にして、警告とカタログの変更を知らせる（3.3 節）。
pub(crate) fn mark_missing(
    inner: &Inner,
    file_id: FileId,
    variant_id: VariantId,
    path: &std::path::Path,
) {
    let r = inner.with_catalog(|c| {
        c.set_file_status(
            file_id,
            FileStatus::Missing,
            Some("ファイルが見つかりません"),
        )
    });
    if let Err(e) = r {
        tracing::warn!(error = %e, "ファイルの状態を記録できない");
        return;
    }
    inner.events.warn(
        WarningCode::FileMissing,
        format!("ファイルが見つかりません: {}", path.display()),
        Some(variant_id),
        Some(path.to_path_buf()),
    );
    inner.events.emit(Event::CatalogChanged {
        change: crate::events::CatalogChange::FileStatus,
        variant_ids: vec![variant_id],
        all: false,
    });
}

/// 1 つの variant の L0 / L1 を作り直す。作って保存したら `true`、作る必要がなかった（動画以外で
/// ファイルが見つからない・古い世代になった）なら `false`。
pub(crate) fn regenerate_one(
    inner: &Inner,
    variant_id: VariantId,
    token: &CancellationToken,
) -> Result<bool, ApiError> {
    let (file, state) = inner
        .with_catalog_api(|c| Ok((variant_file(c, variant_id)?, c.develop_state(variant_id)?)))?;
    let Some(path) = file.path.clone() else {
        return Ok(false);
    };
    if file.status == FileStatus::Missing {
        return Ok(false);
    }
    if file.kind == AssetKind::Video {
        // 動画は現像しない。ワーカーの代表フレームを L0 にする（VID-02）。
        let t =
            inner
                .pool()?
                .video_thumbnail(Lane::Batch, &path, CacheSpec::L0_THUMBNAIL, token)?;
        let rev = source_thumb_rev(file.file_id, file.revision);
        return store_if_current(inner, &file, None, &rev, &t.jpeg, None);
    }
    let settings = state.settings;
    // 現像中の写真なら、展開済みの入力を使う（展開し直さない）。
    let source = match crate::develop::session_source(inner, variant_id, file.revision) {
        Some(s) => s,
        None => load_source(inner, &file, Lane::Batch, token)?.source,
    };
    if token.is_cancelled() {
        return Err(ApiError::Cancelled);
    }
    let request = PreviewRequest::new(RenderQuality::Final, CacheSpec::L1_PREVIEW.long_edge)
        .with_output(OutputTarget::CacheDisplayP3);
    let rendered = inner.render_preview(&source, &settings, &request, Some(variant_id))?;
    let out = rendered
        .output
        .ok_or_else(|| ApiError::Internal("キャッシュの出力がない".to_owned()))?;
    let (w, h) = out.dimensions();
    let icc = cache_icc_profile()?;
    // L1: ステージ 17b のキャッシュの出力（B5）をそのまま 8bit にして、ICC 付きの JPEG にする
    // （長辺は A1 の長辺 = L1 の長辺以下なので縮小しない）。
    let l1 = if w.max(h) <= CacheSpec::L1_PREVIEW.long_edge {
        let q8 = quantize_u8(&out, (0, 0), Dither::None)?;
        encode_jpeg(
            &RgbImage8::from_raw(w, h, q8)?,
            CacheSpec::L1_PREVIEW.quality,
            Some(icc.as_bytes()),
            None,
        )?
    } else {
        let q = quantize_u16(&out, (0, 0), Dither::None)?;
        let img = DynRgbImage::Rgb16(RgbImage16::from_raw(w, h, q)?);
        render_cache_jpeg(&img, &icc, Orientation::Normal, CacheSpec::L1_PREVIEW)?.bytes
    };
    // L0: 16bit にしてから、genzo-media でリニアな値の面積平均で長辺 320 に縮小する（PRV-01 と同じ方法）。
    let q = quantize_u16(&out, (0, 0), Dither::None)?;
    let img = DynRgbImage::Rgb16(RgbImage16::from_raw(w, h, q)?);
    let l0 = render_cache_jpeg(&img, &icc, Orientation::Normal, CacheSpec::L0_THUMBNAIL)?;
    let l0_key = rendered_key(file.file_id, file.revision, &settings, CacheKind::L0Thumb);
    let l1_key = rendered_key(file.file_id, file.revision, &settings, CacheKind::L1Preview);
    store_if_current(
        inner,
        &file,
        Some(&state.develop_hash),
        &l0_key,
        &l0.bytes,
        Some((&l1_key, &l1)),
    )
}

/// 作り始めたときの設定・リビジョンが今と同じなら保存する（4.1 節）。
fn store_if_current(
    inner: &Inner,
    file: &VariantFile,
    develop_hash: Option<&str>,
    l0_key: &str,
    l0: &[u8],
    l1: Option<(&str, &[u8])>,
) -> Result<bool, ApiError> {
    let stored = inner.with_catalog_api(|c| {
        let current = c.file(file.file_id)?;
        if current.revision != file.revision {
            return Ok(false);
        }
        if let Some(hash) = develop_hash
            && c.develop_state(file.variant_id)?.develop_hash != hash
        {
            return Ok(false);
        }
        inner.with_cache(|cache| {
            cache.thumbs.put(file.variant_id, l0_key, l0)?;
            if let Some((key, bytes)) = l1 {
                cache.previews.put(key, Some(file.variant_id), bytes)?;
            }
            Ok(())
        })?;
        Ok(true)
    })?;
    if stored {
        inner.events.emit(Event::ThumbnailUpdated {
            variant_id: file.variant_id,
            rev: l0_key.to_owned(),
        });
    }
    Ok(stored)
}

/// 設定を保存した variant の L0 / L1 を、バックグラウンド（P3）で作り直す。同じ variant の古い要求は
/// 置き換える（「最新の 1 件だけ」）。
pub(crate) fn schedule_regenerate(inner: &Arc<Inner>, variant_id: VariantId) {
    let weak = Arc::downgrade(inner);
    let _ = inner.background.submit_latest(
        format!("regenerate-{variant_id}"),
        Priority::P3,
        move |ctx| {
            let Some(inner) = weak.upgrade() else {
                return Ok(());
            };
            if let Err(e) = regenerate_one(&inner, variant_id, ctx.token())
                && !e.is_cancelled()
            {
                inner.events.warn(
                    WarningCode::PreviewFailed,
                    format!("プレビューを作れませんでした: {e}"),
                    Some(variant_id),
                    None,
                );
            }
            Ok(())
        },
    );
}

/// 複数の variant の L0 / L1 を作り直すジョブを投入する。
pub(crate) fn spawn_regenerate(inner: &Arc<Inner>, variant_ids: Vec<VariantId>) -> u64 {
    spawn_job(
        inner,
        JobKind::RegeneratePreviews,
        "プレビューの作り直し",
        move |ctx| {
            let mut report = RegenerateReport::default();
            let total = variant_ids.len() as u64;
            for (i, &v) in variant_ids.iter().enumerate() {
                if ctx.is_cancelled() {
                    return Err(ApiError::Cancelled);
                }
                match regenerate_one(ctx.inner, v, ctx.token()) {
                    Ok(true) => report.rendered += 1,
                    Ok(false) => report.skipped += 1,
                    Err(e) if e.is_cancelled() => return Err(e),
                    Err(e) => report.failed.push(VariantIssue {
                        variant_id: v,
                        reason: e.to_string(),
                    }),
                }
                ctx.progress(i as u64 + 1, total);
            }
            Ok(JobResult::RegeneratePreviews(report))
        },
    )
}

impl Core {
    /// サムネイル（L0）の JPEG と、キャッシュキーのハッシュ（UI の `genzo://thumb/{variant_id}?rev={rev}`）。
    /// まだなければ `None`（[`Core::regenerate_previews`] で作れる）。
    pub fn thumbnail(&self, variant_id: VariantId) -> Result<Option<ThumbnailData>, ApiError> {
        self.inner.check_open()?;
        let t = self.inner.with_cache(|c| c.thumbs.get(variant_id))?;
        Ok(t.map(|t| ThumbnailData {
            variant_id,
            rev: t.cache_key,
            jpeg: t.jpeg,
        }))
    }

    /// 現在の現像設定の標準プレビュー（L1）のファイル（なければ `None`）。最後に使った日時を記録する
    /// （LRU）。
    pub fn preview_path(&self, variant_id: VariantId) -> Result<Option<PathBuf>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let key = inner.with_catalog_api(|c| {
            let f = variant_file(c, variant_id)?;
            let s = c.develop_settings(variant_id)?;
            Ok(rendered_key(
                f.file_id,
                f.revision,
                &s,
                CacheKind::L1Preview,
            ))
        })?;
        inner.with_cache(|c| c.previews.get(&key))
    }

    /// サムネイル（L0）と標準プレビュー（L1）を作り直すジョブを投入する（P3）。ジョブの ID を返す。
    pub fn regenerate_previews(&self, variant_ids: &[VariantId]) -> Result<u64, ApiError> {
        self.inner.check_open()?;
        Ok(spawn_regenerate(&self.inner, variant_ids.to_vec()))
    }
}
