//! 書き出し（EXP-01・EXP-04、01 のストーリー 7。04 の 2.4 節・6.1 節・6.4 節）。
//!
//! variant ごとに:
//! 1. バッチ用ワーカーで元ファイルを展開する（RAW は `DecodeRaw`、それ以外は `DecodeImage`）。
//! 2. 本体で現像する（GPU 版を既定とし、GPU の側の失敗なら CPU 版。2.4 節）。寸法の指定は
//!    「フル解像度で処理してから縮小」（genzo-pipeline の書き出し）。
//! 3. genzo-media でエンコードする（出力の色空間の ICC を埋め込み、Exif を書く。`remove_gps` なら GPS を
//!    書かない。SEC-03）。
//! 4. 安全に書き出す（6.4 節）: **原本の保護**（書き出し先が、カタログに登録されたファイル、または今回の
//!    入力と同じファイルなら中止する。genzo-media の `ProtectedFiles` で、パスとファイルの同一性で照合する）、
//!    一時ファイルに書いてから名前を変更する、衝突の扱い（連番・上書き・スキップ）。
//!
//! カタログに登録されたファイルとの照合は、カタログ全体の数十万件のファイルを開かないため、次のファイルを
//! 保護の対象に加えて行う:
//! - 書き出し先のフォルダ（パスのままのものと、リンクを解決したもの）にカタログが登録しているファイル。
//! - 上書きの設定では、書き出すファイルと **同じ名前** の、カタログのすべてのフォルダのファイル
//!   （`same_named_catalog_files`）。名前を変更して公開する書き出しが原本の内容を失わせるのは、書き出し先の
//!   ディレクトリの項目が原本の項目そのもの（同じ名前）の場合だけなので、書き出し先のフォルダがカタログに
//!   別のパス（シンボリックリンク・大文字と小文字の違いなど）で登録されていても、同一性で見分けられる。
//!   連番・スキップの設定は既存のファイルを置き換えない（genzo-media の `persist_noclobber`）。
//!
//! 6.4 節との差: 別のフォルダの原本へのハードリンク（別の名前）を書き出し先に置いた場合は、原本として
//! 拒否しない（置き換わるのはリンクの項目だけで、原本の内容は変わらない）。
//!
//! P3 のジョブとして実行し、進捗（1 件ごと）をイベントで知らせ、取り消せる（タイルの区切りで確かめる）。
//! 結果は 1 件ずつ（成功・スキップ・失敗と理由）の一覧（[`ExportReport`]）。動画の書き出し（EXP-05）は v1
//! なので、動画はスキップする。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use genzo_media::{DynRgbImage, ExifData, ProtectedFiles, RgbImage8, RgbImage16, WriteOutcome};
use genzo_model::{AssetKind, ConflictPolicy, ExportSettings, VariantId};
use genzo_pipeline::ExportPixels;
use genzo_worker::Lane;

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::jobs::{JobCtx, spawn_job};
use crate::paths::{absolute_lexical, join_rel, sanitize_file_component, split_volume};
use crate::previews::{load_source, variant_file};
use crate::types::{ExportItem, ExportOutcome, ExportReport, JobKind, JobResult};

/// 書き出し先のフォルダにある、カタログに登録されたファイルの絶対パス（パスのままと、リンクを解決した
/// もの）。
fn catalog_files_in(inner: &Inner, dir: &Path) -> Result<Vec<PathBuf>, ApiError> {
    let mut candidates = vec![dir.to_path_buf()];
    if let Ok(c) = std::fs::canonicalize(dir) {
        candidates.push(c);
    }
    let mut out = Vec::new();
    for d in candidates {
        let Ok(abs) = absolute_lexical(&d) else {
            continue;
        };
        let Ok(vol) = split_volume(&abs) else {
            continue;
        };
        inner.with_catalog(|c| {
            let Some(volume) = c.volumes()?.into_iter().find(|v| v.uuid == vol.uuid) else {
                return Ok(());
            };
            let Some(folder) = c.find_folder(volume.id, &vol.rel)? else {
                return Ok(());
            };
            for f in c.files_in_folder(folder)? {
                out.push(join_rel(&vol.mount, &vol.rel, &f.name));
            }
            Ok(())
        })?;
    }
    Ok(out)
}

/// 書き出すファイルと同じ名前の、カタログに登録されたすべてのファイルの絶対パス（上書きの設定で使う）。
///
/// 名前を変更して公開する書き出しが原本を置き換えるのは、書き出し先の **ディレクトリの項目** が原本の
/// 項目そのものの場合だけ（ハードリンク・シンボリックリンクの別の名前を置き換えても、原本の内容は
/// 変わらない）。その項目の名前は、書き出すファイルの名前と（大文字・小文字と正規化の違いを除いて）
/// 同じになる。そこで、同じ名前のカタログのファイルを [`ProtectedFiles`] に入れ、パスに加えて
/// ファイルの同一性（ボリュームとファイル ID）で照合する。書き出し先のフォルダが、カタログに別のパス
/// （シンボリックリンクを経由したフォルダ、大文字・小文字の違い、`..` を含むパスなど）で登録されて
/// いても見分けられる（6.4 節）。
fn same_named_catalog_files(
    inner: &Inner,
    variant_ids: &[VariantId],
    settings: &ExportSettings,
) -> Result<Vec<PathBuf>, ApiError> {
    inner.with_catalog_api(|c| {
        let mut names: Vec<String> = Vec::with_capacity(variant_ids.len());
        for &v in variant_ids {
            let f = variant_file(c, v)?;
            names.push(output_name(
                &f.name,
                (!f.is_master)
                    .then_some(f.variant_name.as_deref())
                    .flatten(),
                settings.format.extension(),
            ));
        }
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut out = Vec::new();
        for f in c.files_named(&refs)? {
            if let Some(p) = c.file_location(f.id)?.absolute_path() {
                out.push(p);
            }
        }
        Ok(out)
    })
}

/// 書き出しのファイル名（主となるファイルの名前から拡張子を除いたもの。仮想コピーは名前を付ける）。
fn output_name(file_name: &str, variant_name: Option<&str>, ext: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    let mut name = sanitize_file_component(stem);
    if let Some(v) = variant_name {
        name.push('-');
        name.push_str(&sanitize_file_component(v));
    }
    format!("{name}.{ext}")
}

/// 1 件を書き出す。
fn export_one(
    ctx: &JobCtx<'_>,
    variant_id: VariantId,
    settings: &ExportSettings,
    dest_dir: &Path,
    protected: &ProtectedFiles,
    written: &mut HashSet<PathBuf>,
) -> Result<ExportOutcome, ApiError> {
    let inner = ctx.inner;
    let (file, develop, capture) = inner.with_catalog_api(|c| {
        let f = variant_file(c, variant_id)?;
        let d = c.develop_settings(variant_id)?;
        let t = c.capture_time(f.asset_id)?;
        Ok((f, d, t))
    })?;
    if file.kind == AssetKind::Video {
        return Ok(ExportOutcome::Skipped {
            reason: "動画の書き出しは対象外です（EXP-05 は v1）".to_owned(),
            existing: None,
        });
    }
    let loaded = load_source(inner, &file, Lane::Batch, ctx.token())?;
    let token = ctx.token().clone();
    let control = move || token.is_cancelled();
    let (image, backend) =
        inner.render_export(&loaded.source, &develop, settings, &control, variant_id)?;
    let (w, h) = (image.width, image.height);
    let pixels = match image.pixels {
        ExportPixels::Rgb8(v) => DynRgbImage::Rgb8(RgbImage8::from_raw(w, h, v)?),
        ExportPixels::Rgb16(v) => DynRgbImage::Rgb16(RgbImage16::from_raw(w, h, v)?),
    };
    // Exif（撮影情報と、利用者が補正した撮影日時）。GPS は export_image が設定に従って除く。
    let exif = ExifData::from_photo_metadata(&loaded.metadata).with_capture_time(&capture);
    let desired = dest_dir.join(output_name(
        &file.name,
        (!file.is_master)
            .then_some(file.variant_name.as_deref())
            .flatten(),
        settings.format.extension(),
    ));
    // 同じ書き出しの中で書いたファイルは上書きしない（同じ名前の写真が別のフォルダにある場合など）。
    let mut effective = *settings;
    if effective.on_conflict == ConflictPolicy::Overwrite && written.contains(&desired) {
        effective.on_conflict = ConflictPolicy::Sequence;
    }
    match genzo_media::export_image(&pixels, &effective, Some(&exif), &desired, protected)? {
        WriteOutcome::Written { path, replaced } => {
            written.insert(path.clone());
            Ok(ExportOutcome::Written {
                path,
                replaced,
                backend,
            })
        }
        WriteOutcome::Skipped { existing } => Ok(ExportOutcome::Skipped {
            reason: "同じ名前のファイルがあるため、書き出しませんでした".to_owned(),
            existing: Some(existing),
        }),
    }
}

/// 書き出しの本体。
fn run_export(
    ctx: &JobCtx<'_>,
    variant_ids: &[VariantId],
    settings: &ExportSettings,
    dest_dir: &Path,
) -> Result<ExportReport, ApiError> {
    let inner = ctx.inner;
    let dest = absolute_lexical(dest_dir)?;
    std::fs::create_dir_all(&dest).map_err(|e| ApiError::io(&dest, e))?;
    // 保存待ちの現像設定を先に保存する（書き出しはカタログの設定を使う）。
    crate::develop::flush_for_export(inner)?;
    // 原本の保護（6.4 節）: 今回の入力（asset のすべてのファイル）と、書き出し先のフォルダにあるカタログの
    // ファイル。
    let mut protected = ProtectedFiles::new();
    let inputs = inner.with_catalog_api(|c| {
        let mut paths = Vec::new();
        for a in c.assets_of_variants(variant_ids)? {
            for f in c.files_of_asset(a)? {
                if let Some(p) = c.file_location(f.id)?.absolute_path() {
                    paths.push(p);
                }
            }
        }
        Ok(paths)
    })?;
    protected.extend(inputs);
    protected.extend(catalog_files_in(inner, &dest)?);
    // 上書きの設定では、書き出すファイルと同じ名前の、カタログのすべてのファイルも保護の対象にする
    // （書き出し先のフォルダが、カタログに別のパスで登録されたフォルダでも、同一性で照合するため）。
    if settings.on_conflict == ConflictPolicy::Overwrite {
        protected.extend(same_named_catalog_files(inner, variant_ids, settings)?);
    }

    let mut report = ExportReport {
        dest_dir: dest.clone(),
        ..Default::default()
    };
    let total = variant_ids.len() as u64;
    let mut written = HashSet::new();
    ctx.progress(0, total);
    for (i, &v) in variant_ids.iter().enumerate() {
        if ctx.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let outcome = match export_one(ctx, v, settings, &dest, &protected, &mut written) {
            Ok(o) => o,
            Err(e) if e.is_cancelled() => {
                report.cancelled = true;
                break;
            }
            Err(e) => ExportOutcome::Failed { error: e.info() },
        };
        match &outcome {
            ExportOutcome::Written { .. } => report.written += 1,
            ExportOutcome::Skipped { .. } => report.skipped += 1,
            ExportOutcome::Failed { .. } => report.failed += 1,
        }
        report.items.push(ExportItem {
            variant_id: v,
            outcome,
        });
        ctx.progress(i as u64 + 1, total);
    }
    Ok(report)
}

impl Core {
    /// 書き出す（EXP-01・EXP-04）。P3 のジョブとして実行し、ジョブの ID を返す。
    ///
    /// 結果（[`ExportReport`]）は [`Core::wait_job`]・[`Core::job`] で取れる。
    pub fn export(
        &self,
        variant_ids: &[VariantId],
        settings: &ExportSettings,
        dest_dir: &Path,
    ) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        settings
            .validate()
            .map_err(|e| ApiError::InvalidArgument(e.to_string()))?;
        let ids = variant_ids.to_vec();
        let settings = *settings;
        let dest = dest_dir.to_path_buf();
        Ok(spawn_job(
            inner,
            JobKind::Export,
            "書き出し",
            move |ctx| run_export(ctx, &ids, &settings, &dest).map(JobResult::Export),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_names() {
        assert_eq!(output_name("DSC00001.ARW", None, "jpg"), "DSC00001.jpg");
        assert_eq!(
            output_name("DSC00001.ARW", Some("白黒"), "tif"),
            "DSC00001-白黒.tif"
        );
        assert_eq!(
            output_name("a.b.c.png", Some("x/y"), "png"),
            "a.b.c-x_y.png"
        );
    }
}
