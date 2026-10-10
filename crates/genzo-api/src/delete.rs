//! 削除（FILE-01・DATA-06。04 の 6.4 節）と、ファイル操作の状態遷移の起動時の確定（DATA-07）。
//!
//! 削除は 3 つの別々の操作（[`DeleteKind`]）: 仮想コピーの削除、カタログからの除去、ゴミ箱へ移動。
//! どれも、実行の前に影響を受けるファイルと variant の一覧（[`DeletePlan`]）を返し（[`Core::plan_delete`]）、
//! 利用者が確認した後に [`Core::execute_delete`] で実行する。計画の後にカタログが変わっていたら
//! （仮想コピーが増えたなど）、実行せずに [`ApiError::PlanOutdated`] を返す。
//!
//! ゴミ箱への移動は、asset ごとに `file_op` の状態遷移（planned → executing → done / failed）で行う。
//! OS のゴミ箱（[`TrashBackend`]。既定は trash crate）が使えない場合は、**直接の削除に切り替えず失敗にする**。
//! 途中で失敗した asset はカタログに残し、すでにゴミ箱へ移したファイルは `status = missing` にする。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use genzo_catalog::{Catalog, FileOpPath, FileOpRecord};
use genzo_model::{AssetId, FileOpKind, FileOpState, FileStatus, VariantId};

use crate::core::Core;
use crate::error::ApiError;
use crate::events::{CatalogChange, Event, EventHub, WarningCode};
use crate::types::{DeleteKind, DeletePlan, DeleteReport, FileIssue, PlannedFile, PlannedVariant};

/// OS のゴミ箱への移動（テストでは偽物に差し替える。[`crate::CoreHooks`]）。
pub trait TrashBackend: Send + Sync + 'static {
    /// ファイルをゴミ箱へ移す。ゴミ箱が使えない場合はエラーにする（直接は削除しない）。
    fn move_to_trash(&self, path: &Path) -> Result<(), String>;
}

/// OS のゴミ箱（trash crate。Windows のごみ箱、macOS の Finder のゴミ箱、Linux の freedesktop.org の
/// ゴミ箱）。
#[derive(Debug, Clone, Copy, Default)]
pub struct OsTrash;

impl TrashBackend for OsTrash {
    fn move_to_trash(&self, path: &Path) -> Result<(), String> {
        trash::delete(path).map_err(|e| e.to_string())
    }
}

/// 確認待ちの削除の計画。
#[derive(Default)]
pub(crate) struct PlanStore {
    next: u64,
    plans: HashMap<u64, DeletePlan>,
}

/// 覚えておく確認待ちの計画の数（**仮置き**: 32。古いものから捨てる）。
const MAX_PENDING_PLANS: usize = 32;

impl PlanStore {
    fn insert(&mut self, mut plan: DeletePlan) -> DeletePlan {
        self.next += 1;
        plan.plan_id = self.next;
        if self.plans.len() >= MAX_PENDING_PLANS
            && let Some(&oldest) = self.plans.keys().min()
        {
            self.plans.remove(&oldest);
        }
        self.plans.insert(plan.plan_id, plan.clone());
        plan
    }
}

/// asset の一覧から、ゴミ箱への移動・カタログからの除去の影響の一覧を作る。
fn asset_plan(cat: &Catalog, kind: DeleteKind, assets: &[AssetId]) -> Result<DeletePlan, ApiError> {
    let p = cat.trash_plan(assets)?;
    Ok(DeletePlan {
        plan_id: 0,
        kind,
        assets: p.assets,
        variants: p
            .variants
            .into_iter()
            .map(|v| PlannedVariant {
                variant_id: v.id,
                asset_id: v.asset_id,
                is_master: v.is_master,
                name: v.name,
            })
            .collect(),
        files: p
            .files
            .into_iter()
            .map(|f| PlannedFile {
                file_id: f.file_id,
                asset_id: f.asset_id,
                role: f.role,
                path: f.location.absolute_path(),
            })
            .collect(),
    })
}

/// 仮想コピーの削除の影響の一覧（マスターが含まれていればエラー）。
fn virtual_copy_plan(cat: &Catalog, variant_ids: &[VariantId]) -> Result<DeletePlan, ApiError> {
    let mut variants = Vec::new();
    for s in cat.variant_summaries(variant_ids)? {
        if s.is_master {
            return Err(ApiError::InvalidArgument(format!(
                "variant {} はマスターのため、仮想コピーの削除では削除できません（カタログからの除去を使います）",
                s.variant_id
            )));
        }
        if !variants
            .iter()
            .any(|v: &PlannedVariant| v.variant_id == s.variant_id)
        {
            variants.push(PlannedVariant {
                variant_id: s.variant_id,
                asset_id: s.asset_id,
                is_master: false,
                name: s.variant_name,
            });
        }
    }
    Ok(DeletePlan {
        plan_id: 0,
        kind: DeleteKind::VirtualCopies,
        assets: Vec::new(),
        variants,
        files: Vec::new(),
    })
}

/// 計画の内容が同じか（計画の ID を除く）。
fn same_content(a: &DeletePlan, b: &DeletePlan) -> bool {
    a.kind == b.kind && a.assets == b.assets && a.variants == b.variants && a.files == b.files
}

impl Core {
    /// 削除の計画を作る（影響を受けるファイルと variant の一覧。カタログもファイルも変えない）。
    ///
    /// - 仮想コピーの削除: `variant_ids` の仮想コピー（マスターが含まれていれば [`ApiError::InvalidArgument`]）。
    /// - カタログからの除去・ゴミ箱へ移動: `variant_ids` の asset と、そのすべての variant・ファイル。
    pub fn plan_delete(
        &self,
        kind: DeleteKind,
        variant_ids: &[VariantId],
    ) -> Result<DeletePlan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if variant_ids.is_empty() {
            return Err(ApiError::InvalidArgument("対象がありません".to_owned()));
        }
        let plan = inner.with_catalog_api(|c| match kind {
            DeleteKind::VirtualCopies => virtual_copy_plan(c, variant_ids),
            DeleteKind::RemoveFromCatalog | DeleteKind::Trash => {
                let assets = c.assets_of_variants(variant_ids)?;
                asset_plan(c, kind, &assets)
            }
        })?;
        if plan.variants.is_empty() {
            return Err(ApiError::NotFound("削除する variant".to_owned()));
        }
        Ok(inner.plans.lock().insert(plan))
    }

    /// 確認した計画を実行する。
    pub fn execute_delete(&self, plan_id: u64) -> Result<DeleteReport, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let plan = inner
            .plans
            .lock()
            .plans
            .remove(&plan_id)
            .ok_or_else(|| ApiError::NotFound(format!("削除の計画 {plan_id}")))?;
        // 計画の後にカタログが変わっていないか確かめる。
        let current = inner.with_catalog_api(|c| match plan.kind {
            DeleteKind::VirtualCopies => {
                let ids: Vec<VariantId> = plan.variants.iter().map(|v| v.variant_id).collect();
                virtual_copy_plan(c, &ids)
            }
            _ => asset_plan(c, plan.kind, &plan.assets),
        })?;
        if !same_content(&plan, &current) {
            return Err(ApiError::PlanOutdated(
                "影響を受けるファイルか variant が変わりました".to_owned(),
            ));
        }
        let mut report = DeleteReport::default();
        match plan.kind {
            DeleteKind::VirtualCopies => {
                // 1 つのトランザクションで削除する（途中で失敗したら何も削除しない。DATA-02）。
                let ids: Vec<VariantId> = plan.variants.iter().map(|v| v.variant_id).collect();
                inner.with_catalog(|c| c.delete_virtual_copies(&ids))?;
                report.removed_variants = ids;
            }
            DeleteKind::RemoveFromCatalog => {
                let r = inner.with_catalog(|c| c.remove_assets(&plan.assets))?;
                report.removed_assets = r.assets;
                report.removed_variants = r.variants;
            }
            DeleteKind::Trash => {
                for &asset in &plan.assets {
                    let files: Vec<&PlannedFile> =
                        plan.files.iter().filter(|f| f.asset_id == asset).collect();
                    match self.trash_asset(asset, &files) {
                        Ok(trashed) => {
                            report.trashed_files.extend(trashed);
                            report.removed_assets.push(asset);
                            report.removed_variants.extend(
                                plan.variants
                                    .iter()
                                    .filter(|v| v.asset_id == asset)
                                    .map(|v| v.variant_id),
                            );
                        }
                        Err((path, e)) => report.failed.push(FileIssue {
                            path,
                            reason: e.to_string(),
                        }),
                    }
                }
            }
        }
        if !report.removed_variants.is_empty() {
            crate::develop::close_if_removed(inner, &report.removed_variants);
            let removed = report.removed_variants.clone();
            if let Err(e) = inner.with_cache(|c| c.thumbs.remove(&removed)) {
                tracing::warn!(error = %e, "削除した variant のサムネイルを消せない");
            }
            inner.events.emit(Event::CatalogChanged {
                change: CatalogChange::Removed,
                variant_ids: removed,
                all: false,
            });
            crate::search::refresh(inner, &[]);
        }
        Ok(report)
    }

    /// 1 つの asset のファイルをゴミ箱へ移し、カタログから除く（file_op の状態遷移）。
    /// 失敗したら、失敗したファイルとエラーを返す（asset はカタログに残る）。
    fn trash_asset(
        &self,
        asset: AssetId,
        files: &[&PlannedFile],
    ) -> Result<Vec<PathBuf>, (PathBuf, ApiError)> {
        let inner = &self.inner;
        let first = files
            .first()
            .and_then(|f| f.path.clone())
            .unwrap_or_default();
        let (op, _) = inner
            .with_catalog(|c| c.plan_trash(&[asset]))
            .map_err(|e| (first.clone(), e))?;
        inner
            .with_catalog(|c| c.start_file_op(op))
            .map_err(|e| (first.clone(), e))?;
        let mut moved: Vec<(genzo_model::FileId, PathBuf)> = Vec::new();
        for f in files {
            let Some(path) = f.path.clone() else {
                let e = ApiError::FileAccess {
                    path: PathBuf::new(),
                    message: "ボリュームのマウント先が分かりません".to_owned(),
                };
                self.fail_trash(op, &moved, &e.to_string());
                return Err((PathBuf::new(), e));
            };
            if std::fs::symlink_metadata(&path).is_err() {
                // もう元の場所にない（アプリの外で削除・移動された）。移すものはないので続ける。
                continue;
            }
            if let Err(message) = inner.trash.move_to_trash(&path) {
                let e = ApiError::Trash {
                    path: path.clone(),
                    message,
                };
                self.fail_trash(op, &moved, &e.to_string());
                return Err((path, e));
            }
            moved.push((f.file_id, path));
        }
        inner
            .with_catalog(|c| c.complete_file_op(op))
            .map_err(|e| (first, e))?;
        Ok(moved.into_iter().map(|(_, p)| p).collect())
    }

    /// ゴミ箱への移動が途中で失敗した: 記録を failed にし、すでに移したファイルを missing にする。
    fn fail_trash(
        &self,
        op: genzo_model::FileOpId,
        moved: &[(genzo_model::FileId, PathBuf)],
        reason: &str,
    ) {
        let r = self.inner.with_catalog(|c| {
            c.fail_file_op(op, reason)?;
            for (file_id, _) in moved {
                c.set_file_status(
                    *file_id,
                    FileStatus::Missing,
                    Some("ゴミ箱へ移動した（同じ写真の他のファイルの移動に失敗した）"),
                )?;
            }
            Ok(())
        });
        if let Err(e) = r {
            tracing::warn!(error = %e, "ゴミ箱への移動の失敗を記録できない");
        }
    }
}

/// 起動時に、planned / executing のままのファイル操作を、実際のファイルの場所を見て done / failed に
/// 確定させる（6.4 節。DATA-07）。確定させた数を返す。
pub(crate) fn recover_file_ops(catalog: &mut Catalog, events: &EventHub) -> Result<u64, ApiError> {
    let ops = catalog.unfinished_file_ops()?;
    let mut n = 0;
    for op in ops {
        let (state, note) = decide(&op);
        match state {
            FileOpState::Done => catalog.complete_file_op(op.id)?,
            _ => {
                catalog.fail_file_op(op.id, &note)?;
                // ゴミ箱へ移したファイルは、カタログでは見つからないものにする。
                if op.kind == FileOpKind::Trash {
                    for e in &op.payload.entries {
                        if !exists(&e.from) {
                            catalog.set_file_status(
                                e.file_id,
                                FileStatus::Missing,
                                Some("ファイル操作の途中で終了した"),
                            )?;
                        }
                    }
                }
            }
        }
        n += 1;
        events.emit_sticky(Event::Warning {
            code: WarningCode::FileOperationRecovered,
            message: format!(
                "終わっていなかったファイル操作 {}（{}）を {} として確定しました: {note}",
                op.id,
                op.kind,
                state.as_str()
            ),
            variant_id: None,
            path: None,
        });
    }
    Ok(n)
}

fn exists(p: &FileOpPath) -> bool {
    p.absolute_path
        .as_deref()
        .is_some_and(|s| std::fs::symlink_metadata(s).is_ok())
}

/// 終わっていないファイル操作をどう確定させるか。
fn decide(op: &FileOpRecord) -> (FileOpState, String) {
    let entries = &op.payload.entries;
    match op.kind {
        FileOpKind::Trash => {
            let remaining = entries.iter().filter(|e| exists(&e.from)).count();
            if remaining == 0 {
                (
                    FileOpState::Done,
                    "すべてのファイルが元の場所にない（ゴミ箱へ移した）".to_owned(),
                )
            } else if remaining == entries.len() {
                (
                    FileOpState::Failed,
                    "すべてのファイルが元の場所にある（移していない）".to_owned(),
                )
            } else {
                (
                    FileOpState::Failed,
                    format!(
                        "{} 件のうち {remaining} 件が元の場所に残っている（移したファイルは見つからないものにした）",
                        entries.len()
                    ),
                )
            }
        }
        FileOpKind::Move | FileOpKind::Rename => {
            let moved = entries
                .iter()
                .filter(|e| !exists(&e.from) && e.to.as_ref().is_some_and(exists))
                .count();
            let untouched = entries
                .iter()
                .filter(|e| exists(&e.from) && !e.to.as_ref().is_some_and(exists))
                .count();
            if moved == entries.len() {
                (
                    FileOpState::Done,
                    "すべてのファイルが移動先にある".to_owned(),
                )
            } else if untouched == entries.len() {
                (
                    FileOpState::Failed,
                    "すべてのファイルが元の場所にある".to_owned(),
                )
            } else {
                (
                    FileOpState::Failed,
                    format!(
                        "{} 件のうち {moved} 件だけが移動先にある（手で確かめてください）",
                        entries.len()
                    ),
                )
            }
        }
    }
}
