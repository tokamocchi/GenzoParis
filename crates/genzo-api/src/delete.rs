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

use genzo_catalog::{Catalog, FileFacts, FileOpPath, FileOpRecord, FileRecord, QUICK_HASH_CHUNK};
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

/// OS のゴミ箱（trash crate。Windows のごみ箱、macOS のゴミ箱、Linux の freedesktop.org のゴミ箱）。
///
/// - **Windows**: 移す前に、そのドライブにごみ箱があるかを `SHQueryRecycleBinW` で確かめ、なければ失敗に
///   する（決定 No.23）。trash crate は `FOF_NO_UI | FOF_ALLOWUNDO | FOF_WANTNUKEWARNING` で削除を呼ぶため、
///   ごみ箱のないドライブ（ネットワークの共有フォルダなど）では、OS の「完全に削除しますか」の確認を利用者が
///   承認すると直接削除されうる（6.4 節の「直接の削除に切り替えない」に反する）ため。ごみ箱があっても、
///   ごみ箱の最大の大きさを超えるファイルや、ごみ箱の設定で「すぐに削除する」にしたドライブは、この確認では
///   分からない（OS の確認が出うる）。**実機で未確認。**
/// - **macOS**: trash crate の NSFileManager の方法（`trashItemAtURL`）を使う（決定 No.33）。Finder を
///   osascript で操作する既定の方法と違い、「Finder を制御する」許可が要らず、効果音も鳴らない。ただし
///   Finder の「戻す」が使えないことがある（macOS の不具合。ゴミ箱から手で戻すことはできる）。
///   **実機で未確認。**
/// - **Linux**: trash crate の freedesktop.org のゴミ箱。
#[derive(Debug, Clone, Copy, Default)]
pub struct OsTrash;

impl TrashBackend for OsTrash {
    fn move_to_trash(&self, path: &Path) -> Result<(), String> {
        os_trash::ensure_trash_available(path)?;
        os_trash::context().delete(path).map_err(|e| e.to_string())
    }
}

/// OS ごとのゴミ箱の扱い（[`OsTrash`]）。
mod os_trash {
    /// trash crate の設定（macOS は NSFileManager の方法。決定 No.33）。
    pub(super) fn context() -> trash::TrashContext {
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut ctx = trash::TrashContext::new();
        #[cfg(target_os = "macos")]
        {
            use trash::macos::{DeleteMethod, TrashContextExtMacos};
            ctx.set_delete_method(DeleteMethod::NsFileManager);
        }
        ctx
    }

    /// ゴミ箱へ移せるかを、移す前に確かめる（Windows: ドライブにごみ箱があるか。決定 No.23）。ほかの OS
    /// では何もしない（trash crate はゴミ箱を使えなければ失敗を返し、直接は削除しない）。
    #[cfg(not(windows))]
    pub(super) fn ensure_trash_available(_path: &std::path::Path) -> Result<(), String> {
        Ok(())
    }

    #[cfg(windows)]
    pub(super) fn ensure_trash_available(path: &std::path::Path) -> Result<(), String> {
        use std::os::windows::ffi::OsStrExt;

        use windows_sys::Win32::UI::Shell::{SHQUERYRBINFO, SHQueryRecycleBinW};

        // ドライブの判定は、リンク・ドライブの割り当てを解決した実際の場所で行う（割り当てたネットワーク
        // ドライブは \\?\UNC\server\share になる）。
        let real = std::fs::canonicalize(path)
            .map_err(|e| format!("ファイルの場所を確かめられません（{e}）。移していません"))?;
        let root = recycle_bin_root(&real).ok_or_else(|| {
            format!(
                "{} のドライブを判定できないため、ごみ箱の有無を確かめられません。移していません",
                real.display()
            )
        })?;
        let wide: Vec<u16> = root.encode_wide().chain(std::iter::once(0)).collect();
        let mut info = SHQUERYRBINFO {
            cbSize: u32::try_from(std::mem::size_of::<SHQUERYRBINFO>())
                .expect("構造体の大きさは u32 に収まる"),
            ..Default::default()
        };
        // SAFETY: wide は NUL で終わる UTF-16 の文字列、info は cbSize を設定した有効な構造体。関数は
        // info に書くだけ。
        let hr = unsafe { SHQueryRecycleBinW(wide.as_ptr(), &mut info) };
        if hr < 0 {
            return Err(format!(
                "{} にはごみ箱がありません（SHQueryRecycleBinW が 0x{:08X} を返しました）。直接の削除は行いません",
                root.to_string_lossy(),
                hr.cast_unsigned()
            ));
        }
        Ok(())
    }

    /// ごみ箱を確かめるドライブの根（`C:\`・`\\server\share\`）。判定できなければ `None`。
    #[cfg(windows)]
    pub(super) fn recycle_bin_root(path: &std::path::Path) -> Option<std::ffi::OsString> {
        use std::path::{Component, Prefix};

        let Some(Component::Prefix(prefix)) = path.components().next() else {
            return None;
        };
        let mut root = std::ffi::OsString::new();
        match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                root.push(format!("{}:\\", char::from(letter)));
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                root.push("\\\\");
                root.push(server);
                root.push("\\");
                root.push(share);
                root.push("\\");
            }
            Prefix::Verbatim(_) | Prefix::DeviceNS(_) => return None,
        }
        Some(root)
    }

    #[cfg(all(test, windows))]
    mod tests {
        use std::ffi::OsString;
        use std::path::Path;

        use super::recycle_bin_root;

        #[test]
        fn roots_of_drives_and_shares() {
            let root = |p: &str| recycle_bin_root(Path::new(p));
            assert_eq!(root(r"C:\photos\a.jpg"), Some(OsString::from(r"C:\")));
            assert_eq!(root(r"\\?\D:\photos\a.jpg"), Some(OsString::from(r"D:\")));
            assert_eq!(
                root(r"\\server\share\photos\a.jpg"),
                Some(OsString::from(r"\\server\share\"))
            );
            assert_eq!(
                root(r"\\?\UNC\server\share\a.jpg"),
                Some(OsString::from(r"\\server\share\"))
            );
            assert_eq!(root(r"\\.\COM1"), None);
            assert_eq!(root(r"photos\a.jpg"), None);
        }
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

/// ゴミ箱へ移す直前に、今のファイルが、カタログに登録したファイルと同じかを確かめる（6.4 節）。同じと
/// 確かめられなければ、利用者に見せる理由を返す。読めない・確かめられない場合も「同じ」とせず、移さない
/// 側に倒す。ファイルは読み取り専用で開く。
///
/// - サイズとクイックハッシュ（サイズ ＋ 先頭と末尾の 64KiB）が違えば、別のファイル。
/// - サイズが `2 * QUICK_HASH_CHUNK` 以下なら、クイックハッシュはファイルの全体を覆うので、これで確かめ
///   終わる。
/// - それより大きいファイルは、クイックハッシュが同じでも中間だけが違いうる（クイックハッシュは同一性の
///   確定に使えない。3.3 節）。
///   - カタログに全体のハッシュ（`file.full_hash`）があれば、ファイル全体の BLAKE3 を少しずつ読んで求め、
///     比べる。これが決め手で、更新日時は見ない（内容が同じで更新日時だけが違うファイルは移す）。
///   - なければ、登録の内容と比べるものがないので、更新日時が登録と同じことを求める。違えば、中身が同じか
///     確かめられないため移さない（exFAT やネットワークのボリュームでは、内容が同じでも更新日時が違う
///     ことがある。その場合は `check_files` で登録し直せば移せる）。更新日時まで保ったまま中間だけ書き換え
///     られた場合は、全体のハッシュを登録していなければ見分けられない。
fn verify_same_as_registered(path: &Path, registered: &FileRecord) -> Result<(), String> {
    const RETRY: &str = "ファイルの確認（check_files）の後にやり直してください";
    let now = FileFacts::read(path).map_err(|e| {
        format!("ファイルを読めないため、登録のものと同じか確かめられません（{e}）。移していません")
    })?;
    if now.size != registered.facts.size || now.quick_hash != registered.facts.quick_hash {
        return Err(format!(
            "ファイルが登録の後に変わっています（同じ名前の別のファイルの可能性があります）。移していません。{RETRY}"
        ));
    }
    if now.size <= 2 * QUICK_HASH_CHUNK {
        return Ok(());
    }
    if let Some(expected) = &registered.full_hash {
        return match genzo_catalog::full_hash(path) {
            Ok(h) if h == *expected => Ok(()),
            Ok(_) => Err("ファイルの全体のハッシュが登録のものと違います（同じ名前の別のファイルか、中間だけが書き換えられています）。移していません。ファイルを確かめてください（ファイルに触れない「カタログからの除去」は使えます）".to_owned()),
            Err(e) => Err(format!(
                "ファイルの全体のハッシュを求められないため、登録のものと同じか確かめられません（{e}）。移していません"
            )),
        };
    }
    if now.mtime_ns != registered.facts.mtime_ns {
        return Err(format!(
            "ファイルの更新日時が登録の時と違い、全体のハッシュも登録されていないため、同じファイルか確かめられません。移していません。{RETRY}"
        ));
    }
    Ok(())
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
        // 写真の移動・名前の変更（FILE-02）と重ならないよう、ファイル操作の排他を持ったまま確かめて実行する
        // （移動の途中のファイルを、カタログの古い場所で「見つからない」として消さない。PR #4 の Codex の指摘）。
        let _ops = inner.file_ops.lock();
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
                        Ok((trashed, missing)) => {
                            report.trashed_files.extend(trashed);
                            report.skipped_missing.extend(missing);
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

    /// 1 つの asset のファイルをゴミ箱へ移し、カタログから除く（file_op の状態遷移）。移したファイルと、
    /// 元の場所になかった（フォルダはあり、アプリの外で削除された）ファイルを返す。
    /// 失敗したら、失敗したファイルとエラーを返す（asset はカタログに残る）。
    ///
    /// 移し始める前に、すべてのファイルを確かめる（6.4 節「失敗したら元の状態のまま知らせる」）:
    /// - 元の場所が見えない（フォルダごと見えない・権限・入出力のエラー。ドライブを外した場合など）
    ///   ファイルがあれば、移さずに失敗にする（「移した」扱いにして写真を除かない）。
    /// - 登録したファイルと同じと確かめられなければ、移さずに失敗にする（同じ名前の別のファイルや、
    ///   中間だけ書き換えられたファイルを移さない。確かめ方は [`verify_same_as_registered`]）。
    ///
    /// 全体のハッシュの計算はファイル全体を読むので、全体のハッシュを登録したファイルが多いと、この確認に
    /// 時間がかかる。`execute_delete` は同期の API で、進捗の通知も取り消しもない。全体のハッシュを記録する
    /// 経路は今はない（再リンクの確定・コピー取り込みの検証で記録する設計で、どちらも未実装）ので、今は
    /// 影響しない。取り込みなどで多くのファイルに記録するようになったら、ジョブにして進捗と取り消しを付ける。
    fn trash_asset(
        &self,
        asset: AssetId,
        files: &[&PlannedFile],
    ) -> Result<(Vec<PathBuf>, Vec<PathBuf>), (PathBuf, ApiError)> {
        let inner = &self.inner;
        let first = files
            .first()
            .and_then(|f| f.path.clone())
            .unwrap_or_default();
        // 1. 確かめる（まだ何も変えない）。
        let mut to_move: Vec<(genzo_model::FileId, PathBuf)> = Vec::new();
        let mut missing: Vec<PathBuf> = Vec::new();
        for f in files {
            let Some(path) = f.path.clone() else {
                return Err((
                    PathBuf::new(),
                    ApiError::FileAccess {
                        path: PathBuf::new(),
                        message: "ボリュームのマウント先が分かりません".to_owned(),
                    },
                ));
            };
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {}
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound
                        && path.parent().is_some_and(Path::is_dir) =>
                {
                    // もう元の場所にない（アプリの外で削除・移動された）。移すものはない。
                    missing.push(path);
                    continue;
                }
                Err(e) => {
                    let message = if e.kind() == std::io::ErrorKind::NotFound {
                        "ファイルのフォルダが見つかりません（ドライブが外れている可能性があります）。移していません".to_owned()
                    } else {
                        format!("ファイルの場所を確かめられません（移していません）: {e}")
                    };
                    return Err((path.clone(), ApiError::FileAccess { path, message }));
                }
            }
            let registered = inner
                .with_catalog(|c| c.file(f.file_id))
                .map_err(|e| (path.clone(), e))?;
            // カタログのロックの外で確かめる（全体のハッシュはファイル全体を読むため時間がかかりうる）。
            if let Err(message) = verify_same_as_registered(&path, &registered) {
                return Err((path.clone(), ApiError::FileAccess { path, message }));
            }
            to_move.push((f.file_id, path));
        }
        // 2. 移す。
        let (op, _) = inner
            .with_catalog(|c| c.plan_trash(&[asset]))
            .map_err(|e| (first.clone(), e))?;
        inner
            .with_catalog(|c| c.start_file_op(op))
            .map_err(|e| (first.clone(), e))?;
        let mut moved: Vec<(genzo_model::FileId, PathBuf)> = Vec::new();
        for (file_id, path) in to_move {
            if let Err(message) = inner.trash.move_to_trash(&path) {
                let e = ApiError::Trash {
                    path: path.clone(),
                    message,
                };
                self.fail_trash(op, &moved, &e.to_string());
                return Err((path, e));
            }
            moved.push((file_id, path));
        }
        inner
            .with_catalog(|c| c.complete_file_op(op))
            .map_err(|e| (first, e))?;
        Ok((moved.into_iter().map(|(_, p)| p).collect(), missing))
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
/// 確定させる（6.4 節。DATA-07）。確定させた数を返す。ゴミ箱への移動で、元のフォルダごと見えない
/// （ドライブを外した）ファイルがあれば、確定を保留して次の起動で判断し直す（写真を「移した」として
/// カタログから除かない）。
pub(crate) fn recover_file_ops(catalog: &mut Catalog, events: &EventHub) -> Result<u64, ApiError> {
    let ops = catalog.unfinished_file_ops()?;
    let mut n = 0;
    for op in ops {
        if matches!(op.kind, FileOpKind::Move | FileOpKind::Rename) {
            // 移動・名前の変更（FILE-02）: 移動先にあるファイルだけをカタログに反映する（fileops の settle）。
            let settle = crate::fileops::settle(catalog, &op);
            let pending = match &settle {
                crate::fileops::Settle::Pending(note) => Some(note.clone()),
                _ => None,
            };
            match crate::fileops::record_settle(catalog, &op, settle)? {
                Some((state, note)) => {
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
                None => events.emit_sticky(Event::Warning {
                    code: WarningCode::FileOperationRecovered,
                    message: format!(
                        "終わっていなかったファイル操作 {}（{}）の確定を保留しました: {}",
                        op.id,
                        op.kind,
                        pending.unwrap_or_default()
                    ),
                    variant_id: None,
                    path: None,
                }),
            }
            continue;
        }
        let (state, note) = decide(&op);
        if !matches!(state, FileOpState::Done | FileOpState::Failed) {
            // 元の場所が見えない（ドライブを外したなど）: 確定せずに残し、次の起動で判断し直す。
            events.emit_sticky(Event::Warning {
                code: WarningCode::FileOperationRecovered,
                message: format!(
                    "終わっていなかったファイル操作 {}（{}）の確定を保留しました: {note}",
                    op.id, op.kind
                ),
                variant_id: None,
                path: None,
            });
            continue;
        }
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

/// ファイルが見えない理由が「元の場所にない」と言えるか（フォルダは見えて、ファイルだけがない）。
/// フォルダごと見えない（ドライブを外した・ドライブ文字が変わった）・マウント先が分からない場合は
/// `false`（移したかどうか判断できない）。
fn known_absent(p: &FileOpPath) -> bool {
    p.absolute_path.as_deref().is_some_and(|s| {
        let path = Path::new(s);
        matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
            && path.parent().is_some_and(Path::is_dir)
    })
}

/// 終わっていないファイル操作をどう確定させるか（done / failed。確定を保留するなら executing）。
fn decide(op: &FileOpRecord) -> (FileOpState, String) {
    let entries = &op.payload.entries;
    match op.kind {
        FileOpKind::Trash => {
            let remaining = entries.iter().filter(|e| exists(&e.from)).count();
            // 見えないファイルのうち、元の場所にないと言えないもの（フォルダごと見えない）があれば保留する。
            let unknown = entries
                .iter()
                .filter(|e| !exists(&e.from) && !known_absent(&e.from))
                .count();
            if unknown > 0 {
                (
                    FileOpState::Executing,
                    format!(
                        "{} 件のうち {unknown} 件の元の場所（フォルダ）が見えません（ドライブが外れている可能性があります）。ボリュームがつながった後の起動で確定します",
                        entries.len()
                    ),
                )
            } else if remaining == 0 {
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
        // 移動・名前の変更は recover_file_ops が fileops::settle で確定する。
        FileOpKind::Move | FileOpKind::Rename => (
            FileOpState::Executing,
            "移動・名前の変更は別の経路で確定します".to_owned(),
        ),
    }
}
