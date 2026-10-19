//! 比較明・比較暗合成（DEV-34。docs/composite_design.md、決定事項ログ No.177〜180）。
//!
//! 三脚で固定して撮った複数の写真（花火・星の軌跡・光跡）を、画素ごとの最大・最小で 1 枚にする。Lightroom Classic から
//! Photoshop へ「レイヤーとして開く」で渡し、描画モードを「比較（明）」にするのと同じく、**各写真に現像の設定をかけた
//! 後の画像** で合成する（方式 A）。
//!
//! 1. 写真を撮影日時の順に並べる（分からないものは最後、同じなら variant の ID の順。動画は除く）。
//! 2. 1 枚ずつ、書き出しと同じ経路（バッチ用ワーカーで展開 → [`Inner::render_export`]）で、**フル解像度・16bit・
//!    ディザなし**・合成の設定の色空間で現像し、累積のバッファ（genzo-pipeline の [`Compositor`]）に重ねる。持つのは
//!    累積のバッファ 1 枚と、現像した 1 枚だけ（枚数によらない。SCL-05）。
//! 3. 8bit の形式なら、合成の後で 1 回だけ量子化する（書き出しの仕上げと同じ三角分布のディザ）。ディザをかけた画像を
//!    比較明で重ねると、プラス側の雑音ばかりが選ばれて明るく偏るため。
//! 4. 書き出しと同じ安全な手順で書く（04 の 6.4 節。合成の入力・書き出し先のカタログのファイルは上書きしない。一時
//!    ファイルに書いて確定し、上書きしない名前の変更で公開する。同じ名前は連番）。出力シャープ・透かしはかけない。
//!    Exif と説明のメタデータは 1 枚目の写真の値、キーワードは書かない。
//! 5. カタログに登録し（取り込みと同じ手順。メタデータの読み取りはワーカー）、設定に従って元の写真とスタックに
//!    まとめて一番上にする。
//!
//! **失敗の扱い**: 1 枚でも現像に失敗したら（寸法が 1 枚目と違うときを含む）、何も書かずに全体を失敗にする
//! （[`CompositeReport::failure`]。花火の 1 枚が黙って抜けた結果を残さない）。登録・スタックの失敗では、書いたファイルは
//! 消さない（理由を結果に入れる）。
//!
//! **取り消し**: カタログに登録するまでは、取り消したら何も残さない。写真ごと（現像はタイルごと）、エンコードの前、
//! 一時ファイルに書き終えて公開する前（genzo-media の `export_image_cancellable`）、公開した後・メタデータを読んでいる
//! 間・登録の直前に確かめ、公開した後なら書いたファイルを消す（このジョブが作った保存先のフォルダも、空なら消す）。
//! エンコードそのものは途中で止めない。登録をコミットした後は取り消さない（スタックにまとめるところまで進める）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use genzo_catalog::{Catalog, FileFacts, ImportTags, RegisterOptions, VariantSummary};
use genzo_media::{DynRgbImage, ExifData, ProtectedFiles, RgbImage8, RgbImage16, WriteOutcome};
use genzo_model::{
    AssetKind, CompositeSettings, ConflictPolicy, DevelopSettings, ExportFormat, ExportSettings,
    ExportSize, FileStatus, VariantId,
};
use genzo_pipeline::finish::output::{Dither, requantize_u16_to_u8};
use genzo_pipeline::{Compositor, ExportPixels, compositor_bytes};
use genzo_worker::Lane;

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::CatalogChange;
use crate::import::{
    ProbeTarget, ThumbTask, after_import, announce_changes, excluded_dirs, make_thumbnails,
    probe_request, register_requests, tally_outcomes,
};
use crate::jobs::{JobCtx, spawn_job};
use crate::paths::absolute_lexical;
use crate::previews::{load_source, variant_file};
use crate::types::{
    CompositeCheck, CompositeFailure, CompositeReport, CompositeWarning, ImportReport, JobKind,
    JobResult, RenderBackend,
};

/// 合成する写真（撮影日時の順）と、除いた動画の数。
struct Sources {
    photos: Vec<VariantSummary>,
    skipped_videos: u64,
}

/// 指定した写真を撮影日時の順に並べる（同じ variant は 1 回だけ。動画は除く）。
fn ordered_sources(c: &Catalog, variant_ids: &[VariantId]) -> Result<Sources, ApiError> {
    let mut seen = HashSet::new();
    let ids: Vec<VariantId> = variant_ids
        .iter()
        .copied()
        .filter(|v| seen.insert(*v))
        .collect();
    let summaries = c.variant_summaries(&ids)?;
    if let Some(missing) = ids
        .iter()
        .find(|v| !summaries.iter().any(|s| s.variant_id == **v))
    {
        return Err(ApiError::NotFound(format!("variant {missing}")));
    }
    let (mut photos, videos): (Vec<_>, Vec<_>) = summaries
        .into_iter()
        .partition(|s| s.kind == AssetKind::Photo);
    photos.sort_by(|a, b| {
        let (ta, tb) = (a.capture.utc_db_string(), b.capture.utc_db_string());
        let t = match (&ta, &tb) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        t.then(a.variant_id.cmp(&b.variant_id))
    });
    Ok(Sources {
        photos,
        skipped_videos: videos.len() as u64,
    })
}

/// 合成できない理由（写真の数・見つからない写真）。
fn problems(sources: &Sources, missing: &[VariantId]) -> Vec<String> {
    let mut out = Vec::new();
    let n = sources.photos.len();
    if n < CompositeSettings::MIN_SOURCES {
        out.push(format!(
            "合成するには {} 枚以上の写真を選んでください（選んだ写真: {n} 枚{}）",
            CompositeSettings::MIN_SOURCES,
            if sources.skipped_videos > 0 {
                format!("。動画 {} 本は除きました", sources.skipped_videos)
            } else {
                String::new()
            }
        ));
    }
    if n > CompositeSettings::MAX_SOURCES {
        out.push(format!(
            "一度に合成できるのは {} 枚までです（選んだ写真: {n} 枚）",
            CompositeSettings::MAX_SOURCES
        ));
    }
    if !missing.is_empty() {
        out.push(format!(
            "見つからない（または読めなかった）写真が {} 枚あります。再リンクするか、選択から外してください",
            missing.len()
        ));
    }
    out
}

/// 幾何（切り抜き・角度・レンズ補正）の設定。
fn geometry_of(d: &DevelopSettings) -> (&genzo_model::Geometry, &genzo_model::LensCorrection) {
    (&d.geometry, &d.lens)
}

/// 幾何と外部データを除いた現像の設定（ほかの写真と比べる）。
fn non_geometry(d: &DevelopSettings) -> DevelopSettings {
    DevelopSettings {
        geometry: genzo_model::Geometry::default(),
        lens: genzo_model::LensCorrection::default(),
        render_deps: genzo_model::RenderDeps::default(),
        ..d.clone()
    }
}

/// 向きを適用した、元の写真の寸法（分からなければ `None`）。
fn oriented_size(s: &VariantSummary) -> Option<(u32, u32)> {
    let (w, h) = (s.width?, s.height?);
    Some(if s.orientation.swaps_dimensions() {
        (h, w)
    } else {
        (w, h)
    })
}

/// 保存するファイルの名前（`<1 枚目の元のファイル名>-<モード>-<枚数>.<拡張子>`。決定事項ログ No.179）。
fn output_file_name(first: &VariantSummary, settings: &CompositeSettings, count: usize) -> String {
    let stem = Path::new(&first.file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("composite");
    format!(
        "{stem}-{}-{count}.{}",
        settings.mode.file_label(),
        settings.format.extension()
    )
}

/// 1 枚目の写真のフォルダ（既定の保存先）。
fn default_dest(c: &Catalog, first: &VariantSummary) -> Result<Option<PathBuf>, ApiError> {
    Ok(c.file_location(first.file_id)?
        .absolute_path()
        .and_then(|p| p.parent().map(Path::to_path_buf)))
}

/// 書き出しの設定（合成の結果を書くとき）。キーワードは書かず、仕上げはかけない。
fn write_settings(settings: &CompositeSettings) -> ExportSettings {
    ExportSettings {
        format: settings.format,
        color_space: settings.color_space,
        size: ExportSize::Original,
        remove_gps: settings.remove_gps,
        on_conflict: ConflictPolicy::Sequence,
        remove_keywords: true,
        ..ExportSettings::default()
    }
}

fn check(
    inner: &Inner,
    variant_ids: &[VariantId],
    settings: &CompositeSettings,
    dest_dir: Option<&Path>,
) -> Result<CompositeCheck, ApiError> {
    inner.with_catalog_api(|c| {
        let sources = ordered_sources(c, variant_ids)?;
        let mut out = CompositeCheck {
            sources: sources.photos.iter().map(|s| s.variant_id).collect(),
            skipped_videos: sources.skipped_videos,
            ..CompositeCheck::default()
        };
        out.missing = sources
            .photos
            .iter()
            .filter(|s| s.file_status != FileStatus::Ok)
            .map(|s| s.variant_id)
            .collect();
        if let Some(first) = sources.photos.first() {
            let first_size = oriented_size(first);
            let first_develop = c.develop_settings(first.variant_id)?;
            let first_other = non_geometry(&first_develop);
            for s in &sources.photos[1..] {
                if oriented_size(s) != first_size {
                    out.size_differs.push(s.variant_id);
                }
                let d = c.develop_settings(s.variant_id)?;
                if geometry_of(&d) != geometry_of(&first_develop) {
                    out.geometry_differs.push(s.variant_id);
                }
                if non_geometry(&d) != first_other {
                    out.settings_differ.push(s.variant_id);
                }
            }
            if let Some((w, h)) = first_size {
                out.estimated_buffer_bytes = compositor_bytes(settings.mode, w, h);
            }
            out.stacks = c.stacks_of_variants(&out.sources)?.len() as u64;
            out.dest_dir = match dest_dir {
                Some(d) => Some(absolute_lexical(d)?),
                None => default_dest(c, first)?,
            };
            out.file_name = Some(output_file_name(first, settings, sources.photos.len()));
        }
        out.problems = problems(&sources, &out.missing);
        Ok(out)
    })
}

/// 合成の途中の失敗（どの写真か）。
fn fail(report: &mut CompositeReport, variant_id: Option<VariantId>, e: &ApiError) {
    report.failure = Some(CompositeFailure {
        variant_id,
        error: e.info(),
    });
}

/// 写真を 1 枚現像する（16bit・ディザなし・フル解像度）。警告を結果に入れる。
fn render_one(
    ctx: &JobCtx<'_>,
    variant_id: VariantId,
    render_settings: &ExportSettings,
    report: &mut CompositeReport,
) -> Result<(u32, u32, Vec<u16>, crate::previews::LoadedSource), ApiError> {
    let inner = ctx.inner;
    let (file, develop) = inner.with_catalog_api(|c| {
        Ok((
            variant_file(c, variant_id)?,
            c.develop_settings(variant_id)?,
        ))
    })?;
    let loaded = load_source(inner, &file, Lane::Batch, ctx.token())?;
    // 保存された設定の外部データを、実際に使う値に置き換えて描く（書き出しと同じ。`deps` の doc）。
    let effective = crate::deps::with_render_deps(&inner.engine, &develop, &loaded.render_deps);
    if crate::deps::recorded_deps_differ(&develop.render_deps, &effective.render_deps) {
        inner.events.warn(
            crate::events::WarningCode::RenderDepsChanged,
            crate::deps::deps_changed_message(&develop.render_deps, &effective.render_deps),
            Some(variant_id),
            None,
        );
    }
    let token = ctx.token().clone();
    let control = move || token.is_cancelled();
    let (image, backend) = inner.render_export(
        &loaded.source,
        &effective,
        render_settings,
        &control,
        variant_id,
    )?;
    if backend == RenderBackend::Cpu {
        report.cpu_renders += 1;
    }
    let warnings = crate::render::warning_texts(&image.warnings, &image.unimplemented);
    crate::render::emit_render_warnings(inner, variant_id, "合成", &warnings);
    report
        .warnings
        .extend(warnings.into_iter().map(|message| CompositeWarning {
            variant_id,
            message,
        }));
    let pixels = match image.pixels {
        ExportPixels::Rgb16(v) => v,
        ExportPixels::Rgb8(_) => {
            return Err(ApiError::Internal(
                "合成の現像が 16bit になりませんでした".to_owned(),
            ));
        }
    };
    Ok((image.width, image.height, pixels, loaded))
}

/// 書いたファイルをカタログに登録する（取り込みと同じ手順）。登録した写真と、メタデータを読めなかったときの理由
/// （写真は `status = error` で登録してある）を返す。
fn register_written(
    ctx: &JobCtx<'_>,
    path: &Path,
) -> Result<(VariantId, Option<String>), ApiError> {
    let inner = ctx.inner;
    let dir = path
        .parent()
        .ok_or_else(|| ApiError::Internal("保存先のフォルダがありません".to_owned()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| ApiError::InvalidArgument("UTF-8 で表せない名前です".to_owned()))?
        .to_owned();
    let vol = crate::volumes::locate(inner.volume_probe.as_ref(), dir)?;
    let mount = crate::volumes::mount_str(&vol.mount)?;
    // メタデータを先に読む（取り消したときに、カタログにフォルダの記録を残さない）。フォルダは登録の直前に決める。
    let facts = FileFacts::read(path)?;
    let pool = inner.pool()?;
    let target = ProbeTarget {
        path,
        kind: AssetKind::Photo,
        folder: genzo_model::FolderId::new(0),
        name: &name,
    };
    let mut request = probe_request(
        &pool,
        &target,
        facts,
        inner.default_offset(),
        Lane::Batch,
        ctx.token(),
    )?;
    // ここまでなら取り消せる（この後はカタログに登録する）。
    if ctx.is_cancelled() {
        return Err(ApiError::Cancelled);
    }
    request.folder_id = inner.with_catalog(|c| {
        let volume = c.ensure_volume(&vol.uuid, vol.label.as_deref(), Some(&mount))?;
        c.note_volume_seen(volume, &mount, vol.label.as_deref(), vol.fs_type.as_deref())?;
        c.ensure_folder(volume, &vol.rel)
    })?;
    let mut requests = vec![request];
    let mut import = ImportReport::default();
    // 元の写真と名前（拡張子の前）が違うので組にはならないが、念のため組にしない。
    let options = RegisterOptions {
        pair_raw_jpeg: false,
    };
    let outcomes = register_requests(
        inner,
        &mut requests,
        &options,
        &ImportTags::default(),
        &[None],
        &mut import,
    )?;
    let variant = outcomes
        .first()
        .map(|o| o.master_variant_id)
        .ok_or_else(|| ApiError::Internal("登録の結果がありません".to_owned()))?;
    let mut thumbs: Vec<ThumbTask> = Vec::new();
    let mut regenerate: Vec<VariantId> = Vec::new();
    let changed = tally_outcomes(
        inner,
        &requests,
        &outcomes,
        &[path],
        &mut import,
        &mut thumbs,
        &mut regenerate,
    )?;
    announce_changes(inner, changed);
    let mut no_embedded = Vec::new();
    make_thumbnails(ctx, 1, &thumbs, &mut import, &mut no_embedded);
    after_import(inner, &import, regenerate, no_embedded);
    let problem = requests[0]
        .error
        .as_ref()
        .map(|reason| format!("登録しましたが、メタデータを読めませんでした: {reason}"));
    Ok((variant, problem))
}

/// 取り消し・失敗のときの後片付け: このジョブが書いたファイル（`file`）と、このジョブが作った保存先のフォルダ（空の
/// ときだけ）を消す。消せなくても失敗にはしない（ログに残す）。
fn discard(file: Option<&Path>, dest: &Path, created_dest: bool) {
    if let Some(f) = file
        && let Err(e) = std::fs::remove_file(f)
    {
        tracing::warn!(path = %f.display(), error = %e, "合成: 取り消した結果のファイルを消せません");
    }
    if created_dest {
        // 空でなければ消えない（ほかのファイルを消さない）。
        let _ = std::fs::remove_dir(dest);
    }
}

/// 合成の結果を元の写真とスタックにまとめ、一番上にする。
fn stack_with_sources(
    inner: &Inner,
    result: VariantId,
    sources: &[VariantId],
) -> Result<(), ApiError> {
    let mut ids = sources.to_vec();
    ids.push(result);
    let (before, info) = inner.with_catalog(|c| {
        let before = crate::stacks::stack_members_of(c, &ids)?;
        let id = c.create_stack(&ids, Some(result))?;
        Ok((before, c.stack(id)?))
    })?;
    crate::stacks::after_change(
        inner,
        CatalogChange::Stacks,
        before.into_iter().chain(info.members),
    );
    Ok(())
}

fn run_composite(
    ctx: &JobCtx<'_>,
    variant_ids: &[VariantId],
    settings: &CompositeSettings,
    dest_dir: Option<&Path>,
) -> Result<CompositeReport, ApiError> {
    let inner = ctx.inner;
    // 保存待ちの現像設定を先に保存する（合成はカタログの設定を使う）。
    crate::develop::flush_for_export(inner)?;
    let checked = check(inner, variant_ids, settings, dest_dir)?;
    if !checked.problems.is_empty() {
        return Err(ApiError::InvalidArgument(checked.problems.join("。")));
    }
    let (Some(dest), Some(name)) = (checked.dest_dir.clone(), checked.file_name.clone()) else {
        return Err(ApiError::InvalidArgument(
            "1 枚目の写真のフォルダが分からないため、保存先を選んでください".to_owned(),
        ));
    };
    let dest = absolute_lexical(&dest)?;
    if excluded_dirs(inner).contains(&dest, std::fs::canonicalize(&dest).ok().as_deref()) {
        return Err(ApiError::InvalidArgument(format!(
            "アプリのデータのフォルダ（プレビューのキャッシュ・バックアップ）には保存できません: {}",
            dest.display()
        )));
    }
    // 原本の保護（6.4 節）: 合成の入力（asset のすべてのファイル）と、保存先のフォルダにあるカタログのファイル。
    let mut protected = ProtectedFiles::new();
    let inputs = inner.with_catalog_api(|c| {
        let mut paths = Vec::new();
        for a in c.assets_of_variants(&checked.sources)? {
            for f in c.files_of_asset(a)? {
                if let Some(p) = c.file_location(f.id)?.absolute_path() {
                    paths.push(p);
                }
            }
        }
        Ok(paths)
    })?;
    protected.extend(inputs);
    protected.extend(crate::export::catalog_files_in(inner, &dest)?);

    let mut report = CompositeReport {
        mode: settings.mode,
        sources: checked.sources.clone(),
        skipped_videos: checked.skipped_videos,
        ..CompositeReport::default()
    };
    // 現像は 16bit・ディザなし（形式によらない。モジュールの doc の 3）。
    let render_settings = ExportSettings {
        format: ExportFormat::Tiff16,
        ..write_settings(settings)
    };
    let total = checked.sources.len() as u64;
    ctx.progress(0, total);
    let mut compositor: Option<Compositor> = None;
    let mut first_meta = None;
    for (i, &v) in checked.sources.iter().enumerate() {
        if ctx.is_cancelled() {
            report.cancelled = true;
            return Ok(report);
        }
        let (w, h, pixels, loaded) = match render_one(ctx, v, &render_settings, &mut report) {
            Ok(r) => r,
            Err(e) if e.is_cancelled() => {
                report.cancelled = true;
                return Ok(report);
            }
            Err(e) => {
                fail(&mut report, Some(v), &e);
                return Ok(report);
            }
        };
        match &mut compositor {
            None => {
                let (capture, descriptive) = inner.with_catalog_api(|c| {
                    let f = variant_file(c, v)?;
                    Ok((c.capture_time(f.asset_id)?, c.asset_metadata(f.asset_id)?))
                })?;
                first_meta = Some((loaded.metadata, capture, descriptive));
                report.size = Some((w, h));
                compositor = Some(Compositor::new(
                    settings.mode,
                    settings.color_space,
                    w,
                    h,
                    pixels,
                )?);
            }
            Some(c) => {
                if c.size() != (w, h) {
                    let (fw, fh) = c.size();
                    let e = ApiError::InvalidArgument(format!(
                        "現像した後の寸法（{w} × {h}）が 1 枚目（{fw} × {fh}）と違うため、合成できません。向き・切り抜き・角度・レンズ補正をそろえてください"
                    ));
                    fail(&mut report, Some(v), &e);
                    return Ok(report);
                }
                c.add(&pixels)?;
            }
        }
        report.composited = i as u64 + 1;
        ctx.progress(i as u64 + 1, total);
    }
    if ctx.is_cancelled() {
        report.cancelled = true;
        return Ok(report);
    }
    let compositor = compositor.expect("2 枚以上を合成した");
    let (w, h) = compositor.size();
    let data = compositor.finish();
    let pixels = if settings.format.bits_per_channel() == 8 {
        DynRgbImage::Rgb8(RgbImage8::from_raw(
            w,
            h,
            requantize_u16_to_u8(&data, w, (0, 0), Dither::DEFAULT_8BIT),
        )?)
    } else {
        DynRgbImage::Rgb16(RgbImage16::from_raw(w, h, data)?)
    };
    let (metadata, capture, descriptive) = first_meta.expect("1 枚目のメタデータ");
    // 保存先のフォルダは、書く直前に作る（失敗・取り消しで空のフォルダを残さない）。
    let created_dest = !dest.exists();
    if let Err(e) = std::fs::create_dir_all(&dest) {
        fail(&mut report, None, &ApiError::io(&dest, e));
        return Ok(report);
    }
    let exif = ExifData::from_photo_metadata(&metadata)
        .with_capture_time(&capture)
        .with_descriptive(descriptive);
    let path = match genzo_media::export_image_cancellable(
        &pixels,
        &write_settings(settings),
        Some(&exif),
        &dest.join(&name),
        &protected,
        &|| ctx.is_cancelled(),
    ) {
        Ok(WriteOutcome::Written { path, .. }) => path,
        Ok(WriteOutcome::Skipped { existing }) => {
            // 連番の設定なので起きない（念のため）。
            let e = ApiError::Internal(format!(
                "同じ名前のファイルがあるため、保存しませんでした: {}",
                existing.display()
            ));
            fail(&mut report, None, &e);
            return Ok(report);
        }
        Err(e) => {
            let e = ApiError::from(e);
            discard(None, &dest, created_dest);
            if e.is_cancelled() {
                report.cancelled = true;
            } else {
                fail(&mut report, None, &e);
            }
            return Ok(report);
        }
    };
    // 公開した後でも、カタログに登録する前なら取り消せる（書いたファイルを消す）。
    let registered = if ctx.is_cancelled() {
        Err(ApiError::Cancelled)
    } else {
        register_written(ctx, &path)
    };
    match registered {
        Ok((v, problem)) => {
            report.path = Some(path);
            report.registered = Some(v);
            report.registration_error = problem;
        }
        Err(e) if e.is_cancelled() => {
            discard(Some(&path), &dest, created_dest);
            report.cancelled = true;
            return Ok(report);
        }
        Err(e) => {
            report.path = Some(path);
            report.registration_error = Some(e.to_string());
        }
    }
    if settings.stack_with_sources
        && let Some(result) = report.registered
    {
        match stack_with_sources(inner, result, &report.sources) {
            Ok(()) => report.stacked = true,
            Err(e) => report.stack_error = Some(e.to_string()),
        }
    }
    Ok(report)
}

impl Core {
    /// 合成する前の確認（DEV-34。写真の並び・除いた動画・見つからない写真・寸法・設定の違い・スタックの数・メモリの
    /// 見込み・保存先とファイル名）。カタログを読むだけ（ジョブにしない）。`dest_dir` が `None` なら 1 枚目の写真の
    /// フォルダ。
    pub fn composite_check(
        &self,
        variant_ids: &[VariantId],
        settings: &CompositeSettings,
        dest_dir: Option<&Path>,
    ) -> Result<CompositeCheck, ApiError> {
        self.inner.check_open()?;
        settings
            .validate()
            .map_err(|e| ApiError::InvalidArgument(e.to_string()))?;
        check(&self.inner, variant_ids, settings, dest_dir)
    }

    /// 比較明・比較暗合成（DEV-34。モジュールの doc）。P3 のジョブとして実行し、ジョブの ID を返す。結果
    /// （[`CompositeReport`]）は [`Core::wait_job`]・[`Core::job`] で取れる。`dest_dir` が `None` なら 1 枚目
    /// （撮影日時が最も早い写真）と同じフォルダに保存する。写真の数・見つからない写真は、ジョブを始める前に確かめる
    /// （[`ApiError::InvalidArgument`]）。
    pub fn composite(
        &self,
        variant_ids: &[VariantId],
        settings: &CompositeSettings,
        dest_dir: Option<&Path>,
    ) -> Result<u64, ApiError> {
        let inner = &self.inner;
        let checked = self.composite_check(variant_ids, settings, dest_dir)?;
        if !checked.problems.is_empty() {
            return Err(ApiError::InvalidArgument(checked.problems.join("。")));
        }
        let ids = variant_ids.to_vec();
        let settings = settings.clone();
        let dest = dest_dir.map(Path::to_path_buf);
        Ok(spawn_job(
            inner,
            JobKind::Composite,
            settings.mode.display_name(),
            move |ctx| {
                run_composite(ctx, &ids, &settings, dest.as_deref()).map(JobResult::Composite)
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genzo_model::CompositeMode;

    #[test]
    fn non_geometry_ignores_crop_and_lens_but_not_exposure() {
        let a = DevelopSettings::default();
        let mut b = a.clone();
        b.geometry.angle_deg = 3.0;
        assert_eq!(non_geometry(&a), non_geometry(&b));
        assert_ne!(geometry_of(&a), geometry_of(&b));
        b.exposure_ev = 1.0;
        assert_ne!(non_geometry(&a), non_geometry(&b));
    }

    #[test]
    fn mode_labels_in_file_names() {
        assert_eq!(CompositeMode::Lighten.file_label(), "lighten");
        assert_eq!(CompositeMode::DarkerColor.file_label(), "darker-color");
    }
}
