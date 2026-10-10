//! ワーカーの組（docs/04_architecture.md の 1.2 節のプロセス構成）。
//!
//! | 種類（[`Lane`]） | 数 | 用途 |
//! |---|---|---|
//! | 対話用（[`Lane::Interactive`]） | 1 | ルーペ・現像パネルで開く写真の展開とデコード。優先度が最も高い処理専用で、常に待機させておく |
//! | バッチ用（[`Lane::Batch`]） | 1〜2（[`MAX_BATCH_WORKERS`]） | 取り込み時のメタデータ・埋め込みサムネイル、プレビュー生成と書き出しのための展開 |
//!
//! - 各ワーカーは同時に 1 件だけ処理する。空いているワーカーがなければ、空くまで待つ（待っている間も
//!   取り消せる）。
//! - 共有メモリの一時ディレクトリと、同じファイルでの失敗の記録（[`FailureTracker`]）は、すべての
//!   ワーカーで共有する（同じファイルが別のワーカーで処理されても、続けての失敗として数える）。
//! - 対話用の古い要求を捨てる（6.1 節の P0）には、呼び出し側（`genzo_jobs::Scheduler::submit_latest`
//!   など）が古いジョブの [`CancellationToken`] を取り消す。実行中だったワーカーは強制終了・再起動
//!   され、次の要求をすぐに処理できる。
//! - 優先度の制御（P0〜P3）とメモリの予算は `genzo-jobs` の役割で、この型は行わない。共有メモリの
//!   大きさ（RAW で約 66 MB）を予算に含める場合は、ジョブの投入時に宣言する（`genzo_jobs` の
//!   `JobOptions::memory_bytes`）。

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use genzo_jobs::CancellationToken;
use genzo_media::{CacheSpec, VideoProbe};

use crate::client::{
    LinearImage, RawFrame, Thumbnail, WorkerClient, WorkerClientError, WorkerConfig,
};
use crate::failures::FailureTracker;
use crate::protocol::{PhotoProbe, PongInfo};
use crate::shm::ShmArena;

/// バッチ用のワーカーの数の上限（04 の 1.2 節「バッチ用ワーカー（1〜2 個）」）。
pub const MAX_BATCH_WORKERS: usize = 2;

/// バッチ用のワーカーの数の既定値。
///
/// **仮置き**: 論理 CPU が 8 以上なら 2、それ未満なら 1。対話用・本体の処理（GPU スレッド・
/// エンコード）に CPU を残すため。PoC-2 で展開の並行性能を計測して決める（AR-7）。
pub fn default_batch_workers() -> usize {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    if cpus >= 8 { 2 } else { 1 }
}

/// ワーカーの組の設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolConfig {
    /// 各ワーカーの設定。
    pub worker: WorkerConfig,
    /// バッチ用のワーカーの数（1〜[`MAX_BATCH_WORKERS`]）。
    pub batch_workers: usize,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            worker: WorkerConfig::default(),
            batch_workers: default_batch_workers(),
        }
    }
}

/// ワーカーの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lane {
    /// 対話用（1 個）。
    Interactive,
    /// バッチ用（1〜2 個）。
    Batch,
}

/// 同じ種類のワーカーの置き場。
struct LaneSlots {
    slots: Mutex<Vec<Option<WorkerClient>>>,
    available: Condvar,
}

impl LaneSlots {
    fn lock(&self) -> MutexGuard<'_, Vec<Option<WorkerClient>>> {
        // 置き場は「取り出し中なら None」という単純な状態なので、パニックの後も使い続けてよい。
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// 借りたワーカー（drop すると返す）。
pub struct WorkerLease<'a> {
    lane: &'a LaneSlots,
    index: usize,
    client: Option<WorkerClient>,
}

impl WorkerLease<'_> {
    /// 種類の中での番号（0 から）。
    pub fn index(&self) -> usize {
        self.index
    }
}

impl Deref for WorkerLease<'_> {
    type Target = WorkerClient;

    fn deref(&self) -> &WorkerClient {
        self.client.as_ref().expect("drop の前は常に Some")
    }
}

impl DerefMut for WorkerLease<'_> {
    fn deref_mut(&mut self) -> &mut WorkerClient {
        self.client.as_mut().expect("drop の前は常に Some")
    }
}

impl Drop for WorkerLease<'_> {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            self.lane.lock()[self.index] = Some(client);
            self.lane.available.notify_one();
        }
    }
}

/// 対話用 1 個とバッチ用 1〜2 個のワーカー。複数のスレッドから使える。
pub struct WorkerPool {
    interactive: Arc<LaneSlots>,
    batch: Arc<LaneSlots>,
    failures: Arc<FailureTracker>,
    arena: Arc<ShmArena>,
}

impl std::fmt::Debug for WorkerPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerPool")
            .field("batch_workers", &self.worker_count(Lane::Batch))
            .field("shm_dir", &self.arena.path())
            .finish_non_exhaustive()
    }
}

impl WorkerPool {
    /// ワーカーをすべて起動する。
    pub fn new(config: PoolConfig) -> Result<Self, WorkerClientError> {
        if !(1..=MAX_BATCH_WORKERS).contains(&config.batch_workers) {
            return Err(WorkerClientError::InvalidArgument(format!(
                "バッチ用のワーカーの数は 1〜{MAX_BATCH_WORKERS}（{}）",
                config.batch_workers
            )));
        }
        config.worker.validate()?;
        let arena = Arc::new(config.worker.create_arena()?);
        let failures = Arc::new(FailureTracker::new(config.worker.max_consecutive_failures));
        let worker = Arc::new(config.worker);
        let spawn = |label: String| {
            WorkerClient::spawn_shared(
                Arc::clone(&worker),
                Arc::clone(&arena),
                Arc::clone(&failures),
                label,
            )
        };
        let interactive = vec![Some(spawn("interactive-0".to_owned())?)];
        let batch = (0..config.batch_workers)
            .map(|i| spawn(format!("batch-{i}")).map(Some))
            .collect::<Result<Vec<_>, _>>()?;
        let lane = |slots| {
            Arc::new(LaneSlots {
                slots: Mutex::new(slots),
                available: Condvar::new(),
            })
        };
        Ok(Self {
            interactive: lane(interactive),
            batch: lane(batch),
            failures,
            arena,
        })
    }

    fn slots(&self, lane: Lane) -> &Arc<LaneSlots> {
        match lane {
            Lane::Interactive => &self.interactive,
            Lane::Batch => &self.batch,
        }
    }

    /// 種類 `lane` のワーカーの数。
    pub fn worker_count(&self, lane: Lane) -> usize {
        self.slots(lane).lock().len()
    }

    /// 失敗の記録（全ワーカーで共有）。
    pub fn failures(&self) -> &Arc<FailureTracker> {
        &self.failures
    }

    /// 共有メモリの一時ディレクトリ。
    pub fn arena(&self) -> &Arc<ShmArena> {
        &self.arena
    }

    /// 種類 `lane` の空いているワーカーを借りる。空くまで待つ。`cancel` が取り消されたら
    /// [`WorkerClientError::Cancelled`]。
    pub fn acquire(
        &self,
        lane: Lane,
        cancel: &CancellationToken,
    ) -> Result<WorkerLease<'_>, WorkerClientError> {
        let slots = self.slots(lane);
        if cancel.is_cancelled() {
            return Err(WorkerClientError::Cancelled);
        }
        // 取り消されたら待っているスレッドを起こす（ロックを取ってから通知し、通知を失わない）。
        let waker = Arc::clone(slots);
        let _registration = cancel.on_cancel(move || {
            let _guard = waker.lock();
            waker.available.notify_all();
        });
        let mut guard = slots.lock();
        loop {
            if cancel.is_cancelled() {
                return Err(WorkerClientError::Cancelled);
            }
            if let Some(index) = guard.iter().position(Option::is_some) {
                let client = guard[index].take();
                return Ok(WorkerLease {
                    lane: slots,
                    index,
                    client,
                });
            }
            guard = slots
                .available
                .wait(guard)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn skip_check(&self, path: &Path) -> Result<(), WorkerClientError> {
        let failures = self.failures.consecutive_failures(path);
        if failures >= self.failures.threshold() {
            return Err(WorkerClientError::Skipped {
                path: path.to_path_buf(),
                failures,
            });
        }
        Ok(())
    }

    /// 生存の確認。
    pub fn ping(&self, lane: Lane) -> Result<PongInfo, WorkerClientError> {
        self.acquire(lane, &CancellationToken::new())?.ping()
    }

    /// 写真のメタデータを読む（[`WorkerClient::probe_photo`]）。
    pub fn probe_photo(
        &self,
        lane: Lane,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<PhotoProbe, WorkerClientError> {
        self.skip_check(path)?;
        self.acquire(lane, cancel)?.probe_photo(path, cancel)
    }

    /// 動画のメタデータを読む（[`WorkerClient::probe_video`]）。
    pub fn probe_video(
        &self,
        lane: Lane,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<VideoProbe, WorkerClientError> {
        self.skip_check(path)?;
        self.acquire(lane, cancel)?.probe_video(path, cancel)
    }

    /// RAW を展開する（[`WorkerClient::decode_raw`]）。
    pub fn decode_raw(
        &self,
        lane: Lane,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<RawFrame, WorkerClientError> {
        self.skip_check(path)?;
        self.acquire(lane, cancel)?.decode_raw(path, cancel)
    }

    /// RAW 以外の画像をデコードする（[`WorkerClient::decode_image`]）。
    pub fn decode_image(
        &self,
        lane: Lane,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<LinearImage, WorkerClientError> {
        self.skip_check(path)?;
        self.acquire(lane, cancel)?.decode_image(path, cancel)
    }

    /// 写真のサムネイルを作る（[`WorkerClient::thumbnail`]）。
    pub fn thumbnail(
        &self,
        lane: Lane,
        path: &Path,
        spec: CacheSpec,
        cancel: &CancellationToken,
    ) -> Result<Thumbnail, WorkerClientError> {
        self.skip_check(path)?;
        self.acquire(lane, cancel)?.thumbnail(path, spec, cancel)
    }

    /// 動画のサムネイルを作る（[`WorkerClient::video_thumbnail`]）。
    pub fn video_thumbnail(
        &self,
        lane: Lane,
        path: &Path,
        spec: CacheSpec,
        cancel: &CancellationToken,
    ) -> Result<Thumbnail, WorkerClientError> {
        self.skip_check(path)?;
        self.acquire(lane, cancel)?
            .video_thumbnail(path, spec, cancel)
    }

    /// すべてのワーカーに終了を依頼して、終了を待つ。最初のエラーを返す（残りも終了させる）。
    pub fn shutdown(self) -> Result<(), WorkerClientError> {
        let mut first_err = None;
        for lane in [&self.interactive, &self.batch] {
            let clients: Vec<WorkerClient> =
                lane.lock().iter_mut().filter_map(Option::take).collect();
            for client in clients {
                if let Err(e) = client.shutdown() {
                    first_err.get_or_insert(e);
                }
            }
        }
        first_err.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_can_be_shared_between_threads() {
        fn send_sync<T: Send + Sync>() {}
        fn send<T: Send>() {}
        send_sync::<WorkerPool>();
        send::<WorkerClient>();
        send::<WorkerLease<'static>>();
    }

    #[test]
    fn batch_worker_count_is_validated() {
        for n in [0, MAX_BATCH_WORKERS + 1] {
            let r = WorkerPool::new(PoolConfig {
                worker: WorkerConfig::default(),
                batch_workers: n,
            });
            assert!(
                matches!(r, Err(WorkerClientError::InvalidArgument(_))),
                "{n}"
            );
        }
        assert!((1..=MAX_BATCH_WORKERS).contains(&default_batch_workers()));
    }
}
