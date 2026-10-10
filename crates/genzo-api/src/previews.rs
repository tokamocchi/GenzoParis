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
//! - 作り直しを予約したら、thumbs.db に作り直し待ちの印を付け、作って保存したときに外す。印が残っていれば
//!   （作り直しの前に終了した・異常終了した）、次の起動で作り直す（[`regenerate_pending`]）。

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
use crate::library::Refreshed;
use crate::types::{JobKind, JobResult, RegenerateReport, SourceInfo, ThumbnailData, VariantIssue};

/// 現像結果から作る L0 の保存形式の版（`CacheFormat::format_version`。JPEG の品質などを変えたら上げる）。
pub const L0_FORMAT_VERSION: u32 = 1;
/// 現像結果から作る L1 の保存形式の版。
pub const L1_FORMAT_VERSION: u32 = 1;

/// ワーカーが元ファイルから作った L0（埋め込み JPEG・画像そのもの・動画の代表フレーム）のキャッシュキー
/// （16 進数 64 文字）。キャッシュの世代（カタログの [`genzo_catalog::Catalog::cache_generation`]）・
/// 元ファイル・リビジョンから決まる（現像設定には依存しない）。世代を入れるのは、復元の後に再利用される
/// ID で、前の写真と同じキー（UI の `genzo://thumb/{id}?rev=`）にならないため。
pub(crate) fn source_thumb_rev(generation: &str, file_id: FileId, revision: u32) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"genzo.thumb.source.v2\0");
    h.update(generation.as_bytes());
    h.update(b"\0");
    h.update(&file_id.get().to_le_bytes());
    h.update(&revision.to_le_bytes());
    h.update(&CacheSpec::L0_THUMBNAIL.long_edge.to_le_bytes());
    h.update(&[CacheSpec::L0_THUMBNAIL.quality]);
    h.finalize().to_hex().to_string()
}

/// 現像結果から作る L0 / L1 のキャッシュキー（4.1 節。16 進数 64 文字）。4.1 節のキー（元ファイル・
/// リビジョン・設定・寸法・形式）に、キャッシュの世代を加えたもの（[`source_thumb_rev`] と同じ理由）。
///
/// 外部データ（`render_deps`）は、保存された値ではなく、`raw_decoder`（RAW なら今の RAW デコーダの識別子。
/// RAW 以外は `None`）だけを入れた値で置き換えて求める（写真を展開せずに求めるため。`deps` の doc）。
pub(crate) fn rendered_key(
    generation: &str,
    file_id: FileId,
    revision: u32,
    settings: &DevelopSettings,
    raw_decoder: Option<&str>,
    kind: CacheKind,
) -> String {
    let (spec, version) = match kind {
        CacheKind::L1Preview => (CacheSpec::L1_PREVIEW, L1_FORMAT_VERSION),
        _ => (CacheSpec::L0_THUMBNAIL, L0_FORMAT_VERSION),
    };
    let settings = crate::deps::with_render_deps(
        settings,
        &genzo_model::RenderDeps {
            raw_decoder: raw_decoder.unwrap_or_default().to_owned(),
            ..Default::default()
        },
    );
    let key = CacheKey::for_settings(
        file_id,
        revision,
        &settings,
        (spec.long_edge, 0),
        RenderQuality::Final,
        CacheFormat {
            kind,
            color_space: CacheColorSpace::DisplayP3,
            format_version: version,
        },
    )
    .hex();
    let mut h = blake3::Hasher::new();
    h.update(b"genzo.cache.rendered.v1\0");
    h.update(generation.as_bytes());
    h.update(b"\0");
    h.update(key.as_bytes());
    h.finalize().to_hex().to_string()
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
        is_master: s.is_master,
        variant_name: s.variant_name,
    })
}

/// 展開した入力（段階 A0）と、撮影情報。
pub(crate) struct LoadedSource {
    pub source: PhotoSource,
    pub metadata: PhotoMetadata,
    pub info: SourceInfo,
    /// 展開したファイル（ディスクと照合した後の情報。リビジョンはこちらを使う）。
    pub file: VariantFile,
    /// 実際に使う外部データ（RAW のカメラ行列・デコーダ。RAW 以外は空。`deps` の doc）。
    pub render_deps: genzo_model::RenderDeps,
}

/// variant の主となるファイルをディスクと照合する（3.3 節。サイズ・更新日時が変わっていれば、内容を
/// 確かめ、変わっていればメタデータを読み直してリビジョンを上げる。見つからなかったファイルが戻って
/// いれば `status = ok` に戻す）。照合した後の情報を返す。見つからなければ `None`（初めて見つからなく
/// なったときは `status = missing` にして警告する）。
///
/// 内容の変化を検知したら、カタログの変更を知らせ、L0 / L1 の作り直しを予約する（エンジンの A1・ガイドの
/// キャッシュはリビジョンをキーに含むので、古い内容のものは使われなくなる）。
pub(crate) fn refresh_variant_file(
    inner: &Arc<Inner>,
    file: &VariantFile,
    lane: Lane,
    token: &CancellationToken,
) -> Result<Option<VariantFile>, ApiError> {
    let path = file.path.clone().ok_or_else(|| ApiError::FileAccess {
        path: PathBuf::from(&file.name),
        message: "ボリュームのマウント先が分かりません".to_owned(),
    })?;
    let rec = inner.with_catalog(|c| c.file(file.file_id))?;
    match crate::library::refresh_file(inner, &rec, &path, lane, token)? {
        Refreshed::Unchanged => Ok(Some(file.clone())),
        Refreshed::Missing => {
            if rec.status != FileStatus::Missing {
                mark_missing(inner, file.file_id, file.variant_id, &path);
            }
            Ok(None)
        }
        Refreshed::Restored | Refreshed::Changed => {
            crate::library::after_files_changed(inner, &[rec.asset_id])?;
            Ok(Some(
                inner.with_catalog_api(|c| variant_file(c, file.variant_id))?,
            ))
        }
    }
}

/// 元ファイルをワーカーで展開する（RAW は `DecodeRaw`、それ以外は `DecodeImage`。SEC-05）。
///
/// 展開の前にファイルをディスクと照合する（[`refresh_variant_file`]）。ファイルが見つからなければ
/// `status = missing` にして [`ApiError::FileAccess`]。
pub(crate) fn load_source(
    inner: &Arc<Inner>,
    file: &VariantFile,
    lane: Lane,
    token: &CancellationToken,
) -> Result<LoadedSource, ApiError> {
    let Some(file) = refresh_variant_file(inner, file, lane, token)? else {
        return Err(ApiError::FileAccess {
            path: file
                .path
                .clone()
                .unwrap_or_else(|| PathBuf::from(&file.name)),
            message: "ファイルが見つかりません".to_owned(),
        });
    };
    let path = file.path.clone().ok_or_else(|| ApiError::FileAccess {
        path: PathBuf::from(&file.name),
        message: "ボリュームのマウント先が分かりません".to_owned(),
    })?;
    let pool = inner.pool()?;
    let id = SourceId::new(file.file_id, file.revision);
    let loaded = if pair_class(&file.name) == PairClass::Raw {
        let frame = pool.decode_raw(lane, &path, token)?;
        let render_deps = crate::deps::actual_render_deps(
            frame.image.cam_xyz.as_ref(),
            frame.cam_xyz_source,
            &frame.image.metadata,
            frame.decoder_id.as_deref(),
        );
        let info = SourceInfo {
            is_raw: true,
            width: frame.image.width,
            height: frame.image.height,
            decoder: frame.decoder_id.clone(),
            as_shot_white_balance: as_shot_white_balance(&frame.image),
        };
        let metadata = frame.image.metadata.clone();
        LoadedSource {
            source: PhotoSource::new(id, SourceImage::Raw(Arc::new(frame.image)))?,
            metadata,
            info,
            file,
            render_deps,
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
                as_shot_white_balance: None,
            },
            file,
            render_deps: genzo_model::RenderDeps::default(),
        }
    };
    Ok(loaded)
}

/// 撮影時の WB を色温度と tint で表す（[`SourceInfo::as_shot_white_balance`]）。現像（genzo-pipeline の
/// センサー処理）と同じく、RAW のカメラ行列、なければ BT.2020 とみなす行列を使う。
pub(crate) fn as_shot_white_balance(
    image: &genzo_raw::RawImage,
) -> Option<crate::types::AsShotWhiteBalance> {
    let cam_xyz = image
        .cam_xyz
        .map(|m| genzo_color::Mat3::from_rows(m.map(|r| r.map(f64::from))))
        .unwrap_or(genzo_pipeline::sensor::FALLBACK_CAM_XYZ);
    let wb = image.as_shot_wb;
    let t = genzo_color::white_balance::temperature_tint_from_wb_multipliers(
        &cam_xyz,
        [f64::from(wb[0]), f64::from(wb[1]), f64::from(wb[2])],
    )
    .ok()?;
    let (temperature_k, tint) = (t.temperature_k as f32, t.tint as f32);
    let in_range = genzo_model::develop::ranges::TEMPERATURE_K.contains(temperature_k)
        && genzo_model::develop::ranges::TINT.contains(tint);
    in_range.then_some(crate::types::AsShotWhiteBalance {
        temperature_k,
        tint,
    })
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

/// [`regenerate_one`] の結果。
#[derive(Debug, Default)]
pub(crate) struct Regenerated {
    /// 作って保存したか（作る必要がなかった（動画以外でファイルが見つからない・古い世代になった）なら
    /// `false`）。
    pub stored: bool,
    /// 現像の警告（表示用の説明。警告のイベントでも知らせた）。
    pub warnings: Vec<String>,
}

/// 1 つの variant の L0 / L1 を作り直す。
pub(crate) fn regenerate_one(
    inner: &Arc<Inner>,
    variant_id: VariantId,
    token: &CancellationToken,
) -> Result<Regenerated, ApiError> {
    let warnings_out = std::cell::RefCell::new(Vec::new());
    let stored = match regenerate_inner(inner, variant_id, token, &warnings_out) {
        Ok(stored) => stored,
        Err(e) => {
            // 取り消し（終了など）なら印を残して次の起動で作り直す。それ以外の失敗（読めないファイル
            // など）は警告で知らせ、印は外す（起動のたびに同じ失敗を繰り返さないため）。
            if !e.is_cancelled() {
                clear_pending(inner, variant_id);
            }
            return Err(e);
        }
    };
    let warnings = warnings_out.into_inner();
    if stored {
        crate::render::emit_render_warnings(inner, variant_id, "プレビューの作成", &warnings);
    }
    Ok(Regenerated { stored, warnings })
}

fn regenerate_inner(
    inner: &Arc<Inner>,
    variant_id: VariantId,
    token: &CancellationToken,
    warnings_out: &std::cell::RefCell<Vec<String>>,
) -> Result<bool, ApiError> {
    let (file, state) = inner
        .with_catalog_api(|c| Ok((variant_file(c, variant_id)?, c.develop_state(variant_id)?)))?;
    if file.path.is_none() {
        clear_pending(inner, variant_id);
        return Ok(false);
    }
    // ディスクと照合する（他のアプリで上書きされた内容・見つからなかったファイルが戻ったことを反映する）。
    let Some(file) = refresh_variant_file(inner, &file, Lane::Batch, token)? else {
        // 見つからないファイルは作れない（戻ったら、ファイルの確認で作り直しを予約し直す）。
        clear_pending(inner, variant_id);
        return Ok(false);
    };
    let Some(path) = file.path.clone() else {
        clear_pending(inner, variant_id);
        return Ok(false);
    };
    if file.kind == AssetKind::Video {
        // 動画は現像しない。ワーカーの代表フレームを L0 にする（VID-02）。
        let t =
            inner
                .pool()?
                .video_thumbnail(Lane::Batch, &path, CacheSpec::L0_THUMBNAIL, token)?;
        let rev = source_thumb_rev(&inner.cache_generation, file.file_id, file.revision);
        return store_if_current(inner, &file, None, &rev, &t.jpeg, None);
    }
    let settings = state.settings;
    // 現像中の写真なら、展開済みの入力を使う（展開し直さない）。
    let (source, file, deps) =
        match crate::develop::session_source(inner, variant_id, file.revision) {
            Some((source, deps)) => (source, file, deps),
            None => {
                let loaded = load_source(inner, &file, Lane::Batch, token)?;
                (loaded.source, loaded.file, loaded.render_deps)
            }
        };
    if token.is_cancelled() {
        return Err(ApiError::Cancelled);
    }
    let request = PreviewRequest::new(RenderQuality::Final, CacheSpec::L1_PREVIEW.long_edge)
        .with_output(OutputTarget::CacheDisplayP3);
    // 保存された設定の外部データを、実際に使う値に置き換えて描く（`deps` の doc）。
    let effective = crate::deps::with_render_deps(&settings, &deps);
    let rendered = inner.render_preview(&source, &effective, &request, Some(variant_id))?;
    *warnings_out.borrow_mut() = rendered.warnings;
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
    let g = &inner.cache_generation;
    let dec = inner.raw_decoder_for(&file.name);
    let l0_key = rendered_key(
        g,
        file.file_id,
        file.revision,
        &settings,
        dec,
        CacheKind::L0Thumb,
    );
    let l1_key = rendered_key(
        g,
        file.file_id,
        file.revision,
        &settings,
        dec,
        CacheKind::L1Preview,
    );
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
            // 今の設定で作ったので、作り直し待ちの印を外す（同じカタログのロックの中なので、この後に
            // 設定を保存すれば、印は付け直される）。
            cache.thumbs.clear_regen_pending(file.variant_id)?;
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

/// 作り直し待ちの印を外す（失敗はログだけ）。
fn clear_pending(inner: &Inner, variant_id: VariantId) {
    if let Err(e) = inner.with_cache(|c| c.thumbs.clear_regen_pending(variant_id)) {
        tracing::warn!(error = %e, "作り直し待ちの印を外せない");
    }
}

/// 設定を保存した variant の L0 / L1 を、バックグラウンド（P3）で作り直す。同じ variant の古い要求は
/// 置き換える（「最新の 1 件だけ」）。
///
/// 投入する前に、thumbs.db に作り直し待ちの印を付ける（作り直しの依頼は終了・異常終了で失われるため。
/// 印は作り直して保存したときに外し、残っていれば次の起動で作り直す。[`regenerate_pending`]）。
pub(crate) fn schedule_regenerate(inner: &Arc<Inner>, variant_id: VariantId) {
    if let Err(e) = inner.with_cache(|c| c.thumbs.mark_regen_pending(&[variant_id])) {
        tracing::warn!(error = %e, "作り直し待ちの印を付けられない");
    }
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

/// 起動時に、前回作り直せなかった（作り直し待ちの印が残っている）variant の L0 / L1 を作り直すジョブを
/// 投入する。なければ何もしない。
pub(crate) fn regenerate_pending(inner: &Arc<Inner>) {
    match inner.with_cache(|c| c.thumbs.regen_pending()) {
        Ok(ids) if !ids.is_empty() => {
            spawn_regenerate(inner, ids);
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "作り直し待ちの印を読めない"),
    }
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
                    Ok(r) if r.stored => {
                        report.rendered += 1;
                        if !r.warnings.is_empty() {
                            report.warnings.push(VariantIssue {
                                variant_id: v,
                                reason: r.warnings.join(" / "),
                            });
                        }
                    }
                    Ok(_) => report.skipped += 1,
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
                &inner.cache_generation,
                f.file_id,
                f.revision,
                &s,
                inner.raw_decoder_for(&f.name),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 撮影時の WB の色温度・tint は、カスタムの WB に入れると撮影時の係数に戻る値（DEV-03。指摘 F31）。
    #[test]
    fn as_shot_white_balance_round_trips_through_custom() {
        let image = genzo_raw::RawImage {
            width: 2,
            height: 2,
            cfa: genzo_raw::CfaPattern::RGGB,
            data: vec![0; 4],
            black_level: [0.0; 4],
            white_level: 1000.0,
            as_shot_wb: [2.0, 1.0, 1.5, 1.0],
            cam_xyz: Some([
                [0.7424, -0.2329, -0.0466],
                [-0.4598, 1.2471, 0.2347],
                [-0.0715, 0.1505, 0.6066],
            ]),
            metadata: Default::default(),
        };
        let t = as_shot_white_balance(&image).expect("範囲内");
        let cam = genzo_color::Mat3::from_rows(image.cam_xyz.unwrap().map(|r| r.map(f64::from)));
        let back = genzo_pipeline::sensor::wb::resolve_multipliers(
            &genzo_model::WhiteBalance::Custom {
                temperature_k: t.temperature_k,
                tint: t.tint,
            },
            image.as_shot_wb,
            &cam,
        )
        .unwrap();
        for (a, b) in back.iter().zip(image.as_shot_wb) {
            assert!((a - b).abs() < 1e-3, "{back:?} / {t:?}");
        }
    }

    /// L0 / L1 のキーは、RAW デコーダの識別子で変わり（LibRaw を更新したら古い色のキャッシュを使わない）、
    /// 保存された設定の render_deps（記録した値。空のこともある）には左右されない（K1）。キャッシュの世代
    /// でも変わる（F07）。
    #[test]
    fn rendered_keys_follow_the_decoder_and_generation() {
        let file = FileId::new(1);
        let s = DevelopSettings::default();
        let k = |g: &str, s: &DevelopSettings, dec: Option<&str>| {
            rendered_key(g, file, 1, s, dec, CacheKind::L1Preview)
        };
        let base = k("g1", &s, Some("libraw-0.21.2"));
        assert_ne!(base, k("g1", &s, Some("libraw-0.22.0")), "デコーダ");
        assert_ne!(base, k("g1", &s, None), "RAW 以外");
        assert_ne!(base, k("g2", &s, Some("libraw-0.21.2")), "世代");
        let mut recorded = s.clone();
        recorded.render_deps.raw_decoder = "libraw-0.20.0".into();
        recorded.render_deps.camera_profile = genzo_model::DataRef::new("x", "ab".repeat(32));
        assert_eq!(
            base,
            k("g1", &recorded, Some("libraw-0.21.2")),
            "記録した値には左右されない"
        );
        assert_eq!(base.len(), 64);
        assert_ne!(
            source_thumb_rev("g1", file, 1),
            source_thumb_rev("g2", file, 1)
        );
    }
}
