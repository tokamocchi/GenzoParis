//! コア API のジョブ（P3 のバックグラウンドの処理。04 の 6.1 節、SYS-02）の登録と進捗。
//!
//! genzo-jobs のスケジューラで実行し、状態・進捗・結果をここに記録する。UI へはイベント
//! （[`crate::Event::JobProgress`]・[`crate::Event::JobFinished`]）で知らせ、結果は
//! [`crate::Core::job`]・[`crate::Core::wait_job`] で取れる。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use genzo_jobs::{CancellationToken, JobHandle};
use parking_lot::{Condvar, Mutex};

use crate::core::Inner;
use crate::error::ApiError;
use crate::events::Event;
use crate::types::{JobInfo, JobKind, JobResult, JobState};

/// 終わったジョブを、この件数まで覚えておく（**仮置き**: 256。UI が結果を取りに来るまで残す）。
const FINISHED_JOBS_KEPT: usize = 256;

/// 終わったかを確かめる間隔（結果の通知を待つ間に、実行されずに終わったジョブも見つけるため）。
const WAIT_POLL: Duration = Duration::from_millis(50);

struct Entry {
    kind: JobKind,
    state: JobState,
    done: u64,
    total: u64,
    result: Option<JobResult>,
    error: Option<crate::error::ErrorInfo>,
    token: CancellationToken,
    handle: Option<JobHandle<()>>,
}

impl Entry {
    fn info(&self, job_id: u64) -> JobInfo {
        JobInfo {
            job_id,
            kind: self.kind,
            state: self.state,
            done: self.done,
            total: self.total,
            result: self.result.clone(),
            error: self.error.clone(),
        }
    }
}

/// ジョブの記録。
#[derive(Default)]
pub(crate) struct JobRegistry {
    next: AtomicU64,
    entries: Mutex<HashMap<u64, Entry>>,
    finished_order: Mutex<Vec<u64>>,
    changed: Condvar,
}

impl JobRegistry {
    /// 新しいジョブを記録する（実行中）。
    fn register(&self, kind: JobKind, token: CancellationToken) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.entries.lock().insert(
            id,
            Entry {
                kind,
                state: JobState::Running,
                done: 0,
                total: 0,
                result: None,
                error: None,
                token,
                handle: None,
            },
        );
        id
    }

    fn set_handle(&self, id: u64, handle: JobHandle<()>) {
        if let Some(e) = self.entries.lock().get_mut(&id) {
            e.handle = Some(handle);
        }
    }

    /// 進捗を記録する。記録したら `true`（終わったジョブには記録しない）。
    fn progress(&self, id: u64, done: u64, total: u64) -> Option<JobKind> {
        let mut entries = self.entries.lock();
        let e = entries.get_mut(&id)?;
        if e.state.is_finished() {
            return None;
        }
        e.done = done;
        e.total = total;
        Some(e.kind)
    }

    /// 終わった状態を記録する。
    fn finish(
        &self,
        id: u64,
        state: JobState,
        result: Option<JobResult>,
        error: Option<crate::error::ErrorInfo>,
    ) -> Option<JobKind> {
        let kind = {
            let mut entries = self.entries.lock();
            let e = entries.get_mut(&id)?;
            if e.state.is_finished() {
                return None;
            }
            e.state = state;
            e.result = result;
            e.error = error;
            e.handle = None;
            e.kind
        };
        let mut order = self.finished_order.lock();
        order.push(id);
        if order.len() > FINISHED_JOBS_KEPT {
            let drop_n = order.len() - FINISHED_JOBS_KEPT;
            let mut entries = self.entries.lock();
            for old in order.drain(..drop_n) {
                entries.remove(&old);
            }
        }
        drop(order);
        self.changed.notify_all();
        Some(kind)
    }

    /// ジョブの情報。
    pub(crate) fn info(&self, id: u64) -> Option<JobInfo> {
        self.entries.lock().get(&id).map(|e| e.info(id))
    }

    /// ジョブを取り消す。見つからなければ `false`。
    ///
    /// キューで待っているジョブは、取り消しのコールバックの中でスケジューラが実行せずに終える（クロージャが
    /// 呼ばれないので、記録と `JobFinished` はここでは送られない）。呼び出し側は続けて
    /// [`JobRegistry::reap_unrun`] を呼ぶこと（[`crate::Core::cancel_job`]）。
    pub(crate) fn cancel(&self, id: u64) -> bool {
        // 取り消しのコールバックは同期的に実行されるので、記録のロックの外で取り消す。
        let token = match self.entries.lock().get(&id) {
            Some(e) => e.token.clone(),
            None => return false,
        };
        token.cancel();
        true
    }

    /// 実行中のすべてのジョブを取り消す。
    pub(crate) fn cancel_all(&self) {
        for e in self.entries.lock().values() {
            if !e.state.is_finished() {
                e.token.cancel();
            }
        }
    }

    /// 実行中のジョブの数。
    pub(crate) fn running_count(&self) -> usize {
        self.entries
            .lock()
            .values()
            .filter(|e| !e.state.is_finished())
            .count()
    }

    /// 実行されずに終わったジョブ（キューで待っている間の取り消し、スケジューラの終了など）を見つけて、
    /// 取り消しとして記録し、`JobFinished` を送る。
    pub(crate) fn reap_unrun(&self, inner: &Inner) {
        let mut lost = Vec::new();
        {
            let entries = self.entries.lock();
            for (id, e) in entries.iter() {
                if !e.state.is_finished() && e.handle.as_ref().is_some_and(JobHandle::is_finished) {
                    lost.push(*id);
                }
            }
        }
        for id in lost {
            self.finish_and_notify(inner, id, JobState::Cancelled, None, None);
        }
    }

    fn finish_and_notify(
        &self,
        inner: &Inner,
        id: u64,
        state: JobState,
        result: Option<JobResult>,
        error: Option<crate::error::ErrorInfo>,
    ) {
        let message = error.as_ref().map(|e| e.message.clone());
        if let Some(kind) = self.finish(id, state, result, error) {
            inner.events.emit(Event::JobFinished {
                job_id: id,
                kind,
                state,
                message,
            });
        }
    }

    /// ジョブが終わるまで待つ（`timeout` を過ぎたら、その時点の情報を返す）。
    pub(crate) fn wait(
        &self,
        inner: &Inner,
        id: u64,
        timeout: Option<Duration>,
    ) -> Option<JobInfo> {
        let deadline = timeout.and_then(|t| Instant::now().checked_add(t));
        loop {
            self.reap_unrun(inner);
            let mut entries = self.entries.lock();
            let info = entries.get(&id)?.info(id);
            if info.state.is_finished() {
                return Some(info);
            }
            let wait = match deadline {
                Some(d) => match d.checked_duration_since(Instant::now()) {
                    Some(rest) if !rest.is_zero() => rest.min(WAIT_POLL),
                    _ => return Some(info),
                },
                None => WAIT_POLL,
            };
            // finish は entries のロックを離してから知らせるので、ここで entries のロックを持ったまま
            // 待ってよい（知らせを失っても WAIT_POLL で確かめ直す）。
            self.changed.wait_for(&mut entries, wait);
        }
    }
}

/// 実行中のジョブの文脈（取り消しの確認と進捗の通知）。
pub(crate) struct JobCtx<'a> {
    pub id: u64,
    pub inner: &'a Arc<Inner>,
    token: &'a CancellationToken,
}

impl JobCtx<'_> {
    /// 取り消しトークン（ワーカー・パイプラインに渡す）。
    pub fn token(&self) -> &CancellationToken {
        self.token
    }

    /// 取り消されたか。
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// 進捗を知らせる。
    pub fn progress(&self, done: u64, total: u64) {
        if let Some(kind) = self.inner.jobs.progress(self.id, done, total) {
            self.inner.events.emit(Event::JobProgress {
                job_id: self.id,
                kind,
                done,
                total,
            });
        }
    }
}

impl JobResult {
    /// 取り消されて途中で終わった結果か。
    pub(crate) fn was_cancelled(&self) -> bool {
        match self {
            JobResult::Import(r) => r.cancelled,
            JobResult::Export(r) => r.cancelled,
            _ => false,
        }
    }
}

/// P3 のジョブを投入する（[`Inner`] のバックグラウンドのスケジューラ）。ジョブの ID を返す。
pub(crate) fn spawn_job<F>(inner: &Arc<Inner>, kind: JobKind, label: &str, run: F) -> u64
where
    F: FnOnce(&JobCtx<'_>) -> Result<JobResult, ApiError> + Send + 'static,
{
    let token = CancellationToken::new();
    let id = inner.jobs.register(kind, token.clone());
    let weak = Arc::downgrade(inner);
    let handle = inner.background.submit_with(
        genzo_jobs::JobOptions::new(genzo_jobs::Priority::P3)
            .parent(&token)
            .label(label),
        move |sctx| {
            let Some(inner) = weak.upgrade() else {
                return Ok(());
            };
            // スケジューラのトークンは、記録したトークン（取り消しの操作）の子なので、どちらの取り消し
            // （利用者の取り消し・スケジューラの終了）も届く。
            let job_token = sctx.token();
            let ctx = JobCtx {
                id,
                inner: &inner,
                token: job_token,
            };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&ctx)));
            let (state, result, error) = match outcome {
                Ok(Ok(result)) => {
                    let state = if result.was_cancelled() || job_token.is_cancelled() {
                        JobState::Cancelled
                    } else {
                        JobState::Succeeded
                    };
                    (state, Some(result), None)
                }
                Ok(Err(e)) if e.is_cancelled() => (JobState::Cancelled, None, Some(e.info())),
                Ok(Err(e)) => (JobState::Failed, None, Some(e.info())),
                Err(panic) => {
                    let message = panic
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_owned())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "不明".to_owned());
                    let e = ApiError::Internal(format!("ジョブがパニックしました: {message}"));
                    (JobState::Failed, None, Some(e.info()))
                }
            };
            inner
                .jobs
                .finish_and_notify(&inner, id, state, result, error);
            Ok(())
        },
    );
    inner.jobs.set_handle(id, handle);
    // 記録してからハンドルを入れるまでの間に取り消された（キューから実行されずに外れた）場合。
    if token.is_cancelled() {
        inner.jobs.reap_unrun(inner);
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_records_progress_and_results() {
        let r = JobRegistry::default();
        let id = r.register(JobKind::Import, CancellationToken::new());
        assert_eq!(r.info(id).unwrap().state, JobState::Running);
        assert_eq!(r.progress(id, 1, 4), Some(JobKind::Import));
        let info = r.info(id).unwrap();
        assert_eq!((info.done, info.total), (1, 4));
        assert_eq!(r.running_count(), 1);
        assert!(r.cancel(id));
        assert!(!r.cancel(id + 100));
        r.finish(id, JobState::Cancelled, None, None);
        assert_eq!(r.info(id).unwrap().state, JobState::Cancelled);
        // 終わった後の進捗・2 回目の終了は記録しない。
        assert!(r.progress(id, 2, 4).is_none());
        assert!(r.finish(id, JobState::Succeeded, None, None).is_none());
        assert_eq!(r.running_count(), 0);
    }

    #[test]
    fn old_finished_jobs_are_forgotten() {
        let r = JobRegistry::default();
        let ids: Vec<u64> = (0..FINISHED_JOBS_KEPT + 3)
            .map(|_| {
                let id = r.register(JobKind::Export, CancellationToken::new());
                r.finish(id, JobState::Succeeded, None, None);
                id
            })
            .collect();
        assert!(r.info(ids[0]).is_none());
        assert!(r.info(*ids.last().unwrap()).is_some());
    }
}
