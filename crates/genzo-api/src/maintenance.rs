//! バックアップ・整合性・復元（SYS-04・DATA-04・DATA-05）、ジョブの操作、機能の有無、ワーカーの
//! テスト用の口。
//!
//! # バックアップからの復元
//!
//! 復元は、カタログを開いていない状態で行う（開いているカタログのファイルは差し替えられないため）。
//!
//! 1. [`prepare_restore`]: バックアップを検証して、カタログの隣の **別のファイル** に復元する（既存の
//!    カタログは変えない）。差し替えの手順（[`RestorePlan::steps`]）を返す。
//! 2. アプリ（[`Core`]）を終了する。
//! 3. [`apply_restore`]: 今のカタログ（と `-wal`・`-shm`・`-journal`。本体がなくても付随するファイルが
//!    残っていれば）を退避用の名前に変え、復元したファイルをカタログの名前に変える。CLI からも呼べる。
//! 4. もう一度開く。

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use genzo_jobs::CancellationToken;
use genzo_worker::{Lane, TestHook, TestHookOutput};
use serde::{Deserialize, Serialize};

use crate::core::Core;
use crate::error::ApiError;
use crate::jobs::spawn_job;
use crate::types::{BackupEntry, Capabilities, IntegrityReport, JobInfo, JobKind, JobResult};

impl Core {
    /// 手動でバックアップを作る（DATA-04。世代数を超えた古いものは削除する）。
    pub fn backup_now(&self) -> Result<BackupEntry, ApiError> {
        self.inner.check_open()?;
        self.inner
            .auto_backup(true)?
            .ok_or_else(|| ApiError::InvalidArgument("バックアップの世代数が 0 です".to_owned()))
    }

    /// バックアップの一覧（新しい順）。このカタログのものと、カタログの印のない古い形式の名前のもの
    /// （どのカタログのものか分からない）を返す。
    pub fn list_backups(&self) -> Result<Vec<BackupEntry>, ApiError> {
        self.inner.check_open()?;
        let dir = self.inner.config.backup_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        // このカタログのものだけ（同じファイル名の別のカタログのものは出さない。F10）。
        let owner = self.inner.with_catalog(|c| c.backup_owner())?;
        let mut list: Vec<BackupEntry> = genzo_catalog::list_backups(&dir, &owner)?
            .into_iter()
            .map(BackupEntry::from)
            .collect();
        list.sort_by_key(|b| std::cmp::Reverse(b.created_at));
        Ok(list)
    }

    /// カタログの詳細チェック（DATA-05: `integrity_check`・`foreign_key_check`・テキスト検索の索引）を
    /// P3 のジョブとして実行する。ジョブの ID を返す。問題があれば、バックアップからの復元を案内する。
    pub fn check_integrity(&self) -> Result<u64, ApiError> {
        self.inner.check_open()?;
        Ok(spawn_job(
            &self.inner,
            JobKind::IntegrityCheck,
            "カタログの詳細チェック",
            |ctx| {
                // 時間のかかる integrity_check は読み取り専用の別の接続で行い、書き込みを止めない（WAL）。
                let path = ctx.inner.config.catalog_path.clone();
                let ro = genzo_catalog::Catalog::open_read_only(&path)?;
                let mut report: IntegrityReport = ro.check_integrity()?.into();
                drop(ro);
                if ctx.is_cancelled() {
                    return Err(ApiError::Cancelled);
                }
                // テキスト検索の索引の確認は書き込み用の接続で行う（読み取り専用では実行できない）。
                // カタログのロックの中では FTS の確認だけを行う（integrity_check をもう一度行うと、その間
                // カタログを使う操作がすべて止まるため）。
                report.fts_error = ctx.inner.with_catalog(|c| c.check_text_index())?;
                report.ok = report.integrity_errors.is_empty()
                    && report.foreign_key_violations.is_empty()
                    && report.fts_error.is_none();
                Ok(JobResult::IntegrityCheck(report))
            },
        ))
    }

    /// キャッシュを回収するジョブを投入する（4.1 節「回収」。アイドル時に呼ぶ）: カタログに存在しない
    /// variant のサムネイル（L0）を削除し、L1 の索引とフォルダの内容を突き合わせ、上限容量を超えた分を
    /// 古いものから削除する。ジョブの ID を返す。
    pub fn collect_cache_garbage(&self) -> Result<u64, ApiError> {
        self.inner.check_open()?;
        Ok(spawn_job(
            &self.inner,
            JobKind::CollectGarbage,
            "キャッシュの回収",
            |ctx| {
                let inner = ctx.inner;
                // 1. L0: 回収の間に variant が増えて、そのサムネイルを消さないよう、カタログのロックを持ったまま
                //    行う（DB の操作だけ）。
                let (thumbnails_removed, root) = inner.with_catalog_api(|c| {
                    let alive: std::collections::HashSet<_> =
                        c.all_variant_ids()?.into_iter().collect();
                    inner.with_cache(|cache| {
                        let removed = cache.thumbs.collect_garbage(&alive)? as u64;
                        Ok((removed, cache.previews.root().to_path_buf()))
                    })
                })?;
                if ctx.is_cancelled() {
                    return Err(ApiError::Cancelled);
                }
                // 2. L1: フォルダの走査（時間がかかる）は、カタログとキャッシュのロックの外で行う（その間も
                //    評価・現像の保存・サムネイルの表示を止めない。指摘 F24）。索引の更新と上限容量を超えた
                //    分の削除は、キャッシュのロックの中で短く行う。
                let scan =
                    genzo_catalog::scan_preview_dir(&root).map_err(|source| ApiError::Cache {
                        path: root.clone(),
                        source: Box::new(source),
                    })?;
                inner.with_cache(|cache| {
                    let reconcile = cache.previews.apply_scan(scan)?;
                    let capacity = cache.previews.capacity_bytes();
                    let evicted = cache.previews.evict_to(capacity, None)?;
                    Ok(JobResult::CollectGarbage {
                        thumbnails_removed,
                        previews_removed: (reconcile.dropped + evicted.removed) as u64,
                        temp_files_removed: reconcile.temp_files_removed as u64,
                    })
                })
            },
        ))
    }

    /// ジョブの情報（終わったジョブも、しばらく覚えている）。
    pub fn job(&self, job_id: u64) -> Result<JobInfo, ApiError> {
        self.inner.jobs.reap_unrun(&self.inner);
        self.inner
            .jobs
            .info(job_id)
            .ok_or_else(|| ApiError::NotFound(format!("ジョブ {job_id}")))
    }

    /// ジョブが終わるまで待つ（`timeout` が `None` なら終わるまで。過ぎたら、その時点の情報を返す）。
    pub fn wait_job(&self, job_id: u64, timeout: Option<Duration>) -> Result<JobInfo, ApiError> {
        self.inner
            .jobs
            .wait(&self.inner, job_id, timeout)
            .ok_or_else(|| ApiError::NotFound(format!("ジョブ {job_id}")))
    }

    /// ジョブを取り消す（処理の区切りでやめる。実行中のワーカーの処理は強制終了する）。
    pub fn cancel_job(&self, job_id: u64) -> Result<(), ApiError> {
        if self.inner.jobs.cancel(job_id) {
            // キューで待っていたジョブは実行されずに終わったので、ここで記録して JobFinished を送る。
            self.inner.jobs.reap_unrun(&self.inner);
            Ok(())
        } else {
            Err(ApiError::NotFound(format!("ジョブ {job_id}")))
        }
    }

    /// 実行中のジョブの数。
    pub fn running_jobs(&self) -> usize {
        self.inner.jobs.reap_unrun(&self.inner);
        self.inner.jobs.running_count()
    }

    /// 機能の有無（ワーカーの RAW のデコーダ、GPU）。GPU は初期化していなければ初期化する。
    pub fn capabilities(&self) -> Result<Capabilities, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let pong = inner.pool()?.ping(Lane::Batch)?;
        Ok(Capabilities {
            raw_decoder: pong.raw_decoder,
            worker_version: pong.worker_version,
            gpu: inner.gpu().map(|r| r.context().summary().to_string()),
        })
    }

    /// ワーカーのテスト用の口を実行する（[`crate::CoreConfig::worker_test_hooks`] が真のときだけ動く。
    /// テスト専用）。ワーカーの異常（異常終了・ハング・不正な応答）を起こして、コアが落ちずに
    /// ワーカーを再起動することを確かめるのに使う。
    pub fn run_worker_test_hook(
        &self,
        lane: Lane,
        hook: TestHook,
    ) -> Result<TestHookOutput, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if !inner.config.worker_test_hooks {
            return Err(ApiError::InvalidArgument(
                "ワーカーのテスト用の口は有効になっていません".to_owned(),
            ));
        }
        let pool = inner.pool()?;
        let cancel = CancellationToken::new();
        let mut lease = pool.acquire(lane, &cancel)?;
        Ok(lease.run_test_hook(hook, None, &cancel)?)
    }
}

/// 復元の計画（[`prepare_restore`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestorePlan {
    /// 元のバックアップ。
    pub backup: PathBuf,
    /// 差し替えるカタログ。
    pub catalog_path: PathBuf,
    /// 復元したファイル（カタログの隣の別のファイル）。
    pub restored_path: PathBuf,
    /// 退避する今のカタログの名前（差し替えのときに使う）。
    pub displaced_path: PathBuf,
    /// 差し替えの手順（利用者への案内）。
    pub steps: Vec<String>,
}

/// 復元の手順の 1: バックアップを検証して、カタログの隣の別のファイルに復元する（DATA-04。既存の
/// カタログは変えない）。
pub fn prepare_restore(backup: &Path, catalog_path: &Path) -> Result<RestorePlan, ApiError> {
    let dir = catalog_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stem = catalog_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("catalog");
    let ts = Utc::now().format("%Y%m%dT%H%M%S%3fZ");
    let restored_path = dir.join(format!("{stem}-restored-{ts}.db"));
    let displaced_path = dir.join(format!("{stem}-before-restore-{ts}.db"));
    genzo_catalog::restore_backup(backup, &restored_path)?;
    let steps = vec![
        format!(
            "バックアップ {} を検証し、{} に復元しました",
            backup.display(),
            restored_path.display()
        ),
        "アプリを終了してください（開いているカタログは差し替えられません）".to_owned(),
        format!(
            "今のカタログ {} を {} に退避し（-wal・-shm も同じ名前に付けて退避）、復元したファイルをカタログの名前に変えます（apply_restore。CLI からも実行できます）",
            catalog_path.display(),
            displaced_path.display()
        ),
        "アプリを起動し直してください".to_owned(),
    ];
    Ok(RestorePlan {
        backup: backup.to_path_buf(),
        catalog_path: catalog_path.to_path_buf(),
        restored_path,
        displaced_path,
        steps,
    })
}

/// 復元の手順の 3: 今のカタログを退避し、復元したファイルをカタログの名前に変える。退避したファイル
/// （退避先のパス）を返す。
///
/// カタログを開いているアプリを終了してから呼ぶこと（開いていれば [`ApiError::CatalogInUse`]）。
/// 退避したファイルは消さない（利用者が確かめてから消す）。途中で名前の変更に失敗したら、変えた名前を元に戻してからエラーを返す（今のカタログを
/// 失わないため）。
///
/// カタログの本体がなくても、`-wal`・`-shm`・`-journal` が残っていれば退避する（異常終了の後に本体だけを
/// 失った場合。残すと、次に開いたときに古い `-wal` が復元したカタログに適用されて壊れる）。差し替えた後に
/// 付随するファイルが残っていないことを確かめる。
pub fn apply_restore(plan: &RestorePlan) -> Result<Vec<PathBuf>, ApiError> {
    // カタログを開いているアプリがあれば差し替えない（差し替えの間はロックを持つ）。
    let _lock = match crate::lock::acquire(&plan.catalog_path)? {
        crate::lock::Acquired::Locked(l) | crate::lock::Acquired::Unsupported(l, _) => l,
    };
    if !plan.restored_path.is_file() {
        return Err(ApiError::NotFound(format!(
            "復元したファイル {}",
            plan.restored_path.display()
        )));
    }
    if plan.displaced_path.exists() {
        return Err(ApiError::InvalidArgument(format!(
            "退避先 {} が既にあります",
            plan.displaced_path.display()
        )));
    }
    // 付随するファイル（SQLite の `-wal`・`-shm` と、ロールバックジャーナル）。
    const SIDE_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];
    let side = |p: &Path, suffix: &str| {
        let mut s = p.as_os_str().to_owned();
        s.push(suffix);
        PathBuf::from(s)
    };
    // 名前を変えたもの（元, 先）。途中で失敗したら、逆の順に元の名前へ戻す（カタログの本体がなく
    // `-wal` だけが残ると、次に開いたときに新しい空のカタログへ古い `-wal` が適用されうるため）。
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut steps: Vec<(PathBuf, PathBuf)> = Vec::new();
    // symlink_metadata: リンクの先ではなく、その名前の項目があるか（壊れたリンクも退避する）。
    let present = |p: &Path| std::fs::symlink_metadata(p).is_ok();
    if present(&plan.catalog_path) {
        steps.push((plan.catalog_path.clone(), plan.displaced_path.clone()));
    }
    // 本体の有無によらず、付随するファイルは退避する。
    for suffix in SIDE_SUFFIXES {
        let from = side(&plan.catalog_path, suffix);
        if present(&from) {
            let to = side(&plan.displaced_path, suffix);
            if present(&to) {
                return Err(ApiError::InvalidArgument(format!(
                    "退避先 {} が既にあります",
                    to.display()
                )));
            }
            steps.push((from, to));
        }
    }
    steps.push((plan.restored_path.clone(), plan.catalog_path.clone()));
    let undo = |moved: &[(PathBuf, PathBuf)]| {
        for (back_from, back_to) in moved.iter().rev() {
            if let Err(re) = std::fs::rename(back_to, back_from) {
                tracing::error!(
                    error = %re,
                    from = %back_to.display(),
                    to = %back_from.display(),
                    "復元の取り消しで、ファイルの名前を元に戻せない"
                );
            }
        }
    };
    for (from, to) in steps {
        if let Err(e) = std::fs::rename(&from, &to) {
            undo(&moved);
            return Err(ApiError::io(&from, e));
        }
        moved.push((from, to));
    }
    // 差し替えた後に、付随するファイルが残っていないこと（別のプロセスが作り直したなど）を確かめる。
    for suffix in SIDE_SUFFIXES {
        let leftover = side(&plan.catalog_path, suffix);
        if present(&leftover) {
            undo(&moved);
            return Err(ApiError::InvalidArgument(format!(
                "復元先に {} が残っています（カタログを開いているアプリを終了してから、やり直してください）",
                leftover.display()
            )));
        }
    }
    // 退避したファイル（最後の手順は復元したファイルの名前の変更なので除く）。
    moved.pop();
    Ok(moved.into_iter().map(|(_, to)| to).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_in(dir: &Path) -> RestorePlan {
        RestorePlan {
            backup: dir.join("backup.db"),
            catalog_path: dir.join("catalog.db"),
            restored_path: dir.join("catalog-restored.db"),
            displaced_path: dir.join("catalog-before-restore.db"),
            steps: Vec::new(),
        }
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    #[test]
    fn restore_swaps_the_catalog_and_its_wal() {
        let d = tempfile::tempdir().unwrap();
        let plan = plan_in(d.path());
        std::fs::write(&plan.catalog_path, "current").unwrap();
        std::fs::write(d.path().join("catalog.db-wal"), "current-wal").unwrap();
        std::fs::write(&plan.restored_path, "restored").unwrap();
        apply_restore(&plan).unwrap();
        assert_eq!(read(&plan.catalog_path), "restored");
        assert!(!d.path().join("catalog.db-wal").exists());
        assert_eq!(read(&plan.displaced_path), "current");
        assert_eq!(
            read(&d.path().join("catalog-before-restore.db-wal")),
            "current-wal"
        );
        assert!(!plan.restored_path.exists());
    }

    /// 復元先にカタログの本体がなく `-wal`・`-shm` だけが残っている（異常終了の後に本体だけを失った）
    /// 場合も、`-wal`・`-shm` を退避する（残すと、次に開いたときに古い `-wal` が復元したカタログに
    /// 適用されて壊れる。指摘 F01）。
    #[test]
    fn restore_without_catalog_moves_leftover_wal_aside() {
        let d = tempfile::tempdir().unwrap();
        let plan = plan_in(d.path());
        std::fs::write(d.path().join("catalog.db-wal"), "stale-wal").unwrap();
        std::fs::write(d.path().join("catalog.db-shm"), "stale-shm").unwrap();
        std::fs::write(d.path().join("catalog.db-journal"), "stale-journal").unwrap();
        std::fs::write(&plan.restored_path, "restored").unwrap();
        let moved = apply_restore(&plan).unwrap();
        assert_eq!(read(&plan.catalog_path), "restored");
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(
                !d.path().join(format!("catalog.db{suffix}")).exists(),
                "{suffix} が残っている"
            );
        }
        assert!(!plan.displaced_path.exists(), "本体はなかった");
        assert_eq!(
            read(&d.path().join("catalog-before-restore.db-wal")),
            "stale-wal"
        );
        assert_eq!(
            read(&d.path().join("catalog-before-restore.db-shm")),
            "stale-shm"
        );
        assert_eq!(
            read(&d.path().join("catalog-before-restore.db-journal")),
            "stale-journal"
        );
        assert_eq!(moved.len(), 3, "退避したファイルを返す: {moved:?}");
    }

    /// 退避の途中で失敗したら（`-wal` を移せないなど）、今のカタログを元の名前に戻す（カタログの本体が
    /// なく `-wal` だけが残ると、次に開いたときに新しい空のカタログに古い `-wal` が適用されうる）。
    #[test]
    fn failed_restore_puts_the_current_catalog_back() {
        let d = tempfile::tempdir().unwrap();
        let plan = plan_in(d.path());
        std::fs::write(&plan.catalog_path, "current").unwrap();
        std::fs::write(d.path().join("catalog.db-wal"), "current-wal").unwrap();
        std::fs::write(&plan.restored_path, "restored").unwrap();
        // `-wal` の退避先にフォルダ（中身あり）を置いて、`-wal` の名前の変更を失敗させる。
        let blocker = d.path().join("catalog-before-restore.db-wal");
        std::fs::create_dir(&blocker).unwrap();
        std::fs::write(blocker.join("x"), "x").unwrap();
        assert!(apply_restore(&plan).is_err());
        assert_eq!(read(&plan.catalog_path), "current", "元に戻す");
        assert_eq!(read(&d.path().join("catalog.db-wal")), "current-wal");
        assert!(!plan.displaced_path.exists());
        assert_eq!(
            read(&plan.restored_path),
            "restored",
            "復元したファイルは残す"
        );
    }
}
