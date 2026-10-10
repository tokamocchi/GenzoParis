//! 選別（LIB-04）・キャプション・メタデータの表示（LIB-14）・フォルダ（LIB-06）・ファイルの状態の確認
//! （04 の 3.3 節）。

use std::path::Path;
use std::sync::Arc;

use genzo_catalog::{FileFacts, FileRecord};
use genzo_model::{ColorLabel, FileStatus, Flag, Rating, VariantId};
use genzo_worker::Lane;

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::{CatalogChange, Event};
use crate::import::{ProbeTarget, media_kind, probe_request};
use crate::paths::join_rel;
use crate::types::{
    CatalogStats, FileCheckReport, FileInfo, FileIssue, FolderInfo, VariantDetails, VariantSummary,
};

/// 選別の操作の後: カタログの変更を知らせ、表示中の検索結果を作り直す（変えた variant は残す。3.7 節）。
fn after_marks(inner: &Inner, change: CatalogChange, ids: &[VariantId]) {
    inner.events.emit(Event::CatalogChanged {
        change,
        variant_ids: ids.to_vec(),
        all: false,
    });
    crate::search::refresh(inner, ids);
}

impl Core {
    /// 評価を一括で変える（LIB-04）。変えた variant の数を返す。
    pub fn set_rating(&self, variant_ids: &[VariantId], rating: Rating) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        crate::search::keep_marked(&self.inner, variant_ids);
        let n = self
            .inner
            .with_catalog(|c| c.set_rating(variant_ids, rating))?;
        after_marks(&self.inner, CatalogChange::Marks, variant_ids);
        Ok(n)
    }

    /// フラグ（採用 / 不採用）を一括で変える（LIB-04）。
    pub fn set_flag(&self, variant_ids: &[VariantId], flag: Flag) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        crate::search::keep_marked(&self.inner, variant_ids);
        let n = self.inner.with_catalog(|c| c.set_flag(variant_ids, flag))?;
        after_marks(&self.inner, CatalogChange::Marks, variant_ids);
        Ok(n)
    }

    /// カラーラベルを一括で変える（`None` でラベルなし。LIB-04）。
    pub fn set_color_label(
        &self,
        variant_ids: &[VariantId],
        label: Option<ColorLabel>,
    ) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        crate::search::keep_marked(&self.inner, variant_ids);
        let n = self
            .inner
            .with_catalog(|c| c.set_color_label(variant_ids, label))?;
        after_marks(&self.inner, CatalogChange::Marks, variant_ids);
        Ok(n)
    }

    /// キャプションを一括で変える（asset に付ける。`None` または空白だけで削除）。変えた asset の数を
    /// 返す。テキスト検索の索引も更新する（3.6 節）。
    pub fn set_caption(
        &self,
        variant_ids: &[VariantId],
        caption: Option<&str>,
    ) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        let n = self.inner.with_catalog(|c| {
            let assets = c.assets_of_variants(variant_ids)?;
            for &a in &assets {
                c.set_caption(a, caption)?;
            }
            Ok(assets.len())
        })?;
        // キャプションは選別の条件ではないので、条件に合わなくなったものは残さない。
        self.inner.events.emit(Event::CatalogChanged {
            change: CatalogChange::Caption,
            variant_ids: variant_ids.to_vec(),
            all: false,
        });
        crate::search::refresh(&self.inner, &[]);
        Ok(n)
    }

    /// variant の詳細（メタデータの表示。LIB-14）。
    pub fn variant_details(&self, variant_id: VariantId) -> Result<VariantDetails, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let (summary, asset, files) = inner.with_catalog_api(|c| {
            let s = c
                .variant_summaries(&[variant_id])?
                .pop()
                .ok_or_else(|| ApiError::NotFound(format!("variant {variant_id}")))?;
            let a = c.asset(s.asset_id)?;
            let mut files = Vec::new();
            for f in c.files_of_asset(s.asset_id)? {
                files.push(FileInfo {
                    file_id: f.id,
                    role: f.role,
                    path: c.file_location(f.id)?.absolute_path(),
                    size: f.facts.size,
                    status: f.status,
                    status_reason: f.status_reason,
                });
            }
            Ok((s, a, files))
        })?;
        let rev = inner.with_cache(|c| c.thumbs.cache_key(variant_id))?;
        Ok(VariantDetails {
            summary: VariantSummary::from_catalog(summary, rev),
            iso: asset.iso,
            aperture: asset.aperture,
            shutter_s: asset.shutter_s,
            focal_mm: asset.focal_mm,
            gps: asset.gps,
            caption: asset.caption,
            video: asset.video,
            files,
        })
    }

    /// カタログの件数とスキーマの版（CLI の `catalog info` など。数え方は genzo-catalog の
    /// `Catalog::counts`）。
    pub fn catalog_stats(&self) -> Result<CatalogStats, ApiError> {
        self.inner.check_open()?;
        self.inner.with_catalog(|c| {
            let counts = c.counts()?;
            Ok(CatalogStats {
                schema_version: c.schema_version()?,
                assets: counts.assets,
                variants: counts.variants,
                files: counts.files,
                history_entries: counts.history_entries,
            })
        })
    }

    /// 登録されているフォルダ（LIB-06。ボリュームごと、相対パスの順）。
    pub fn folders(&self) -> Result<Vec<FolderInfo>, ApiError> {
        self.inner.check_open()?;
        self.inner.with_catalog(|c| {
            let mut out = Vec::new();
            for v in c.volumes()? {
                let mount = v.last_mount_path.clone();
                for f in c.folders(v.id)? {
                    out.push(FolderInfo {
                        folder_id: f.id,
                        volume_id: f.volume_id,
                        parent_id: f.parent_id,
                        path: mount
                            .as_deref()
                            .map(|m| join_rel(std::path::Path::new(m), &f.rel_path, "")),
                        rel_path: f.rel_path,
                    });
                }
            }
            Ok(out)
        })
    }

    /// 指定した variant のファイルの状態を確かめる（3.3 節「表示したときに確認」）。
    ///
    /// - 見つからなければ `status = missing` にする。
    /// - 見つかったら、サイズ・更新日時が変わっていればクイックハッシュを求め直し、内容の変化を検知したら
    ///   **メタデータ（寸法・撮影日時など）をワーカーで読み直して登録し直す**（取り込みと同じ経路。
    ///   リビジョンを上げてキャッシュを無効にする。4.1 節。読めなければ `status = error`）。見つからなかった
    ///   ファイルが同じ内容で戻っていれば `status = ok` に戻す（読めなかったファイルの `error` は、内容が
    ///   同じならそのまま）。
    pub fn check_files(&self, variant_ids: &[VariantId]) -> Result<FileCheckReport, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let files = inner.with_catalog_api(|c| {
            let mut out = Vec::new();
            for a in c.assets_of_variants(variant_ids)? {
                for f in c.files_of_asset(a)? {
                    let path = c.file_location(f.id)?.absolute_path();
                    out.push((f, path));
                }
            }
            Ok(out)
        })?;
        let mut report = FileCheckReport::default();
        let mut changed_assets: Vec<genzo_model::AssetId> = Vec::new();
        let token = genzo_jobs::CancellationToken::new();
        for (f, path) in files {
            report.checked += 1;
            let Some(path) = path else {
                continue;
            };
            match refresh_file(inner, &f, &path, Lane::Batch, &token) {
                Ok(Refreshed::Missing) => {
                    if f.status != FileStatus::Missing {
                        inner.with_catalog(|c| {
                            c.set_file_status(
                                f.id,
                                FileStatus::Missing,
                                Some("ファイルが見つかりません"),
                            )
                        })?;
                        changed_assets.push(f.asset_id);
                    }
                    report.missing.push(FileIssue {
                        path,
                        reason: "ファイルが見つかりません".to_owned(),
                    });
                }
                Ok(Refreshed::Unchanged) => report.ok += 1,
                Ok(Refreshed::Restored) => {
                    report.ok += 1;
                    changed_assets.push(f.asset_id);
                }
                Ok(Refreshed::Changed) => {
                    report.ok += 1;
                    report.changed.push(path);
                    changed_assets.push(f.asset_id);
                }
                // 読めない（ハッシュを求められない）ファイルは、今の状態のまま。
                Err(ApiError::FileAccess { .. }) => report.ok += 1,
                Err(e) => return Err(e),
            }
        }
        after_files_changed(inner, &changed_assets)?;
        Ok(report)
    }
}

/// ファイルをディスクと照合した結果（[`refresh_file`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refreshed {
    /// 変化はない（読めなかったファイルで、内容が同じものを含む）。
    Unchanged,
    /// 見つからない（状態は変えていない。呼び出し側が `status = missing` にする）。
    Missing,
    /// 見つからなかったファイルが、同じ内容で戻った（`status = ok` にした）。
    Restored,
    /// 内容が変わった（メタデータを読み直して登録し直した。リビジョンが上がった）。
    Changed,
}

/// 1 つのファイルをディスクと照合する（3.3 節。[`Core::check_files`]・現像を開く・書き出し・作り直しで
/// 共通）。
///
/// サイズ・更新日時が記録と同じで、見つからない印もなければ、何もしない。違えば、クイックハッシュを
/// 求め、内容が変わっていればワーカーでメタデータを読み直し、取り込みと同じ経路（`register_file`。
/// リビジョンを上げ、利用者の撮影日時の修正を引き継ぎ、読めなければ `status = error`）で登録し直す。
/// 読めることを確かめずに `error` を `ok` にしない。ファイルを読めない（ハッシュを求められない）場合は
/// [`ApiError::FileAccess`]。
pub(crate) fn refresh_file(
    inner: &Inner,
    rec: &FileRecord,
    path: &Path,
    lane: Lane,
    token: &genzo_jobs::CancellationToken,
) -> Result<Refreshed, ApiError> {
    let Some(meta) = std::fs::metadata(path).ok().filter(|m| m.is_file()) else {
        return Ok(Refreshed::Missing);
    };
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_nanos()).ok())
        .unwrap_or(0);
    let same = meta.len() == rec.facts.size && mtime_ns == rec.facts.mtime_ns;
    if same && rec.status != FileStatus::Missing {
        return Ok(Refreshed::Unchanged);
    }
    let facts = FileFacts::read(path).map_err(|e| ApiError::FileAccess {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    if !facts.differs_from(&rec.facts) {
        return Ok(match rec.status {
            FileStatus::Missing => {
                inner.with_catalog(|c| c.update_file_facts(rec.id, &facts))?;
                Refreshed::Restored
            }
            _ => Refreshed::Unchanged,
        });
    }
    // 内容が変わった: メタデータを読み直して登録し直す。
    let pool = inner.pool()?;
    let target = ProbeTarget {
        path,
        kind: media_kind(&rec.name).unwrap_or(genzo_model::AssetKind::Photo),
        folder: rec.folder_id,
        name: &rec.name,
    };
    let mut request = probe_request(&pool, &target, facts, inner.default_offset(), lane, token)?;
    inner.with_catalog_api(|c| {
        // 既定のタイムゾーンで推定する撮影日時は、登録の直前の設定で推定し直す（取り込みと同じ）。
        if request.capture.tz_source == genzo_model::TzSource::UserDefault {
            request.capture = request
                .capture
                .with_default_offset(inner.default_offset())
                .map_err(|e| ApiError::Internal(e.to_string()))?;
        }
        c.register_file(&request)?;
        Ok(())
    })?;
    Ok(Refreshed::Changed)
}

/// ファイルの状態・内容が変わった asset について: カタログの変更を知らせ、表示中の検索結果を作り直し、
/// L0 / L1 の作り直しを予約する。
pub(crate) fn after_files_changed(
    inner: &Arc<Inner>,
    assets: &[genzo_model::AssetId],
) -> Result<(), ApiError> {
    if assets.is_empty() {
        return Ok(());
    }
    let mut variants: Vec<VariantId> = Vec::new();
    for &a in assets {
        variants.extend(variant_ids_of(inner, a)?);
    }
    variants.sort();
    variants.dedup();
    inner.events.emit(Event::CatalogChanged {
        change: CatalogChange::FileStatus,
        variant_ids: variants.clone(),
        all: false,
    });
    crate::search::refresh(inner, &[]);
    for v in variants {
        crate::previews::schedule_regenerate(inner, v);
    }
    Ok(())
}

fn variant_ids_of(inner: &Inner, asset: genzo_model::AssetId) -> Result<Vec<VariantId>, ApiError> {
    Ok(inner
        .with_catalog(|c| c.variants_of_asset(asset))?
        .into_iter()
        .map(|v| v.id)
        .collect())
}
