//! 優先度付きのジョブスケジューラ（04 の 1.3 節・6.1 節・6.2 節、01 の SYS-02）。
//!
//! # 動作
//!
//! - 決まった数のスレッド（[`SchedulerConfig::threads`]）で、キューのジョブを優先度の高い順・
//!   同じ優先度は投入順（FIFO）に実行する。
//! - ジョブが宣言したメモリ（[`JobOptions::memory_bytes`]）の枠を予算から取れるまで、ジョブを
//!   始めない。キューの先頭のジョブの枠が取れないときは、後ろのジョブも始めない（優先度の
//!   逆転を防ぐ）。並列度はスレッド数と予算の両方で制限される（6.1 節、SCL-05・SCL-07）。
//! - `threads = 1` にすると、単一スレッドの優先度付きキューになる。GPU スレッド
//!   （6.1 節「GPU への投入は本体の GPU スレッドだけが行う」）に使う。
//!
//! # ロックの順序（デッドロックの防止）
//!
//! スケジューラの状態のロック → 予算のロック → トークンのロック、の順にだけ取る
//! （イベントの購読者の一覧のロックはどのロックの中でも取ってよく、その中では他のロックを取らない）。
//! スケジューラの状態のロックを持ったまま、次のことはしない:
//! トークンの取り消し（コールバックがスケジューラのロックを取る）、メモリの枠の返却
//! （通知がスケジューラのロックを取る）、ジョブのクロージャや結果の drop（捕捉した枠を返却する
//! ことがある。結果を送った通路の送り側の drop も含む。ハンドルが先に drop されていると、
//! 送り側の drop で通路の中の結果が drop されるため）。これらはロックを外してから行う。

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::thread::{JoinHandle, ThreadId};

use parking_lot::{Condvar, Mutex};

use crate::cancel::{CancelRegistration, CancellationToken};
use crate::events::{EventHub, JobEvent, JobEventKind};
use crate::job::{
    Completion, Deferred, ErasedJob, Generation, JobContext, JobError, JobHandle, JobId,
    JobOptions, JobShared, JobState, SlotKey, SupersedePolicy, TypedJob,
};
use crate::memory::{DEFAULT_NORMAL_BUDGET_BYTES, MemoryBudget, MemoryError, Reservation};
use crate::priority::{Priority, recommended_worker_threads};

/// スレッド名の既定の接頭辞。
const DEFAULT_THREAD_NAME: &str = "genzo-jobs";

/// キューが空のときの「最も高い優先度の順位」の値（どの順位より大きい）。
const NO_QUEUED_RANK: u8 = u8::MAX;

/// 実行スレッドで、ロックの外に回した値（届かなかった結果・実行しなかったジョブの本体など）を
/// drop する。
///
/// これらの drop は利用者の型の `Drop` を呼ぶのでパニックしうる。パニックで実行スレッドが
/// 終わると、スレッドが 1 本だけのスケジューラ（GPU スレッド。6.1 節）では以後のジョブが
/// 永久に実行されなくなるため、捕まえて捨てる（内容は既定のパニックフックで出力される）。
fn drop_on_worker(deferred: Vec<Deferred>) {
    if deferred.is_empty() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(move || drop(deferred)));
}

/// スケジューラの設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerConfig {
    /// ジョブを実行するスレッドの数（1 以上）。
    pub threads: usize,
    /// メモリの予算の総量（バイト）。
    pub memory_budget_bytes: u64,
    /// スレッド名の接頭辞（`<接頭辞>-<番号>`）。
    pub thread_name: String,
}

impl SchedulerConfig {
    /// スレッド数と予算を指定した設定。
    pub fn new(threads: usize, memory_budget_bytes: u64) -> Self {
        SchedulerConfig {
            threads,
            memory_budget_bytes,
            thread_name: DEFAULT_THREAD_NAME.to_owned(),
        }
    }

    /// バックグラウンド処理用の既定の設定。
    ///
    /// スレッド数は [`recommended_worker_threads`]（SCL-07）、予算は
    /// [`DEFAULT_NORMAL_BUDGET_BYTES`]（SCL-05 の通常時。仮置き）。
    pub fn background() -> Self {
        SchedulerConfig::new(recommended_worker_threads(), DEFAULT_NORMAL_BUDGET_BYTES)
    }

    /// 単一スレッドの設定（GPU スレッドなど。6.1 節）。
    pub fn single_thread(memory_budget_bytes: u64) -> Self {
        SchedulerConfig::new(1, memory_budget_bytes)
    }

    /// スレッド名の接頭辞を変える。
    pub fn thread_name(mut self, name: impl Into<String>) -> Self {
        self.thread_name = name.into();
        self
    }
}

/// スケジューラの作成のエラー。
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// スレッド数が 0。
    #[error("スレッド数は 1 以上を指定してください")]
    ZeroThreads,
    /// スレッドを起動できなかった。
    #[error("ジョブのスレッドを起動できませんでした")]
    Spawn(#[source] std::io::Error),
}

/// 優先度付きのジョブスケジューラ。
///
/// `Scheduler` はスレッド間で共有できる（`Arc<Scheduler>` で持つ）。drop すると
/// [`Scheduler::shutdown_now`] と同じく、キューのジョブを取り消し、実行中のジョブに取り消しを
/// 通知して、スレッドの終了を待つ。
pub struct Scheduler {
    shared: Arc<Shared>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    worker_ids: Vec<ThreadId>,
    threads: usize,
    listener_id: u64,
}

impl Scheduler {
    /// スケジューラを作り、スレッドを起動する。予算は `config.memory_budget_bytes` で新しく作る。
    pub fn new(config: SchedulerConfig) -> Result<Scheduler, SchedulerError> {
        let budget = MemoryBudget::new(config.memory_budget_bytes);
        Scheduler::with_budget(config.threads, config.thread_name, budget)
    }

    /// 既存の予算を共有するスケジューラを作る。
    ///
    /// 本体の CPU のプールと GPU スレッドのように、1 つの予算（本体とワーカーを合わせた
    /// メモリの予算。6.1 節）を複数のスケジューラで分け合うときに使う。
    pub fn with_budget(
        threads: usize,
        thread_name: impl Into<String>,
        budget: MemoryBudget,
    ) -> Result<Scheduler, SchedulerError> {
        if threads == 0 {
            return Err(SchedulerError::ZeroThreads);
        }
        let thread_name = thread_name.into();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            work_cv: Condvar::new(),
            idle_cv: Condvar::new(),
            budget: budget.clone(),
            events: Arc::new(EventHub::default()),
            best_queued_rank: Arc::new(AtomicU8::new(NO_QUEUED_RANK)),
            next_id: AtomicU64::new(1),
        });

        // 枠が返却されたら、メモリを待っているキューの先頭を再確認する。
        // ロックを取ってから知らせる（確認してから眠るまでの間の知らせを失わない）。
        let weak = Arc::downgrade(&shared);
        let listener_id = budget.add_listener(Arc::new(move || {
            if let Some(shared) = weak.upgrade() {
                let _state = shared.state.lock();
                shared.work_cv.notify_all();
            }
        }));

        let mut workers = Vec::with_capacity(threads);
        for index in 0..threads {
            let worker_shared = Arc::clone(&shared);
            let spawned = std::thread::Builder::new()
                .name(format!("{thread_name}-{index}"))
                .spawn(move || worker_shared.worker_loop());
            match spawned {
                Ok(handle) => workers.push(handle),
                Err(error) => {
                    shared.state.lock().shutting_down = true;
                    shared.work_cv.notify_all();
                    for handle in workers {
                        let _ = handle.join();
                    }
                    budget.remove_listener(listener_id);
                    return Err(SchedulerError::Spawn(error));
                }
            }
        }
        let worker_ids = workers.iter().map(|h| h.thread().id()).collect();
        Ok(Scheduler {
            shared,
            workers: Mutex::new(workers),
            worker_ids,
            threads,
            listener_id,
        })
    }

    /// スレッドの数。
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// メモリの予算。
    pub fn budget(&self) -> &MemoryBudget {
        &self.shared.budget
    }

    /// ジョブのイベント（状態の変化と進捗）を受け取るチャネルを作る。
    ///
    /// 作った後に起きたイベントだけを受け取る。受け取り側を drop すると購読をやめる。
    /// チャネルには上限がないので、受け取らずに放置しないこと。
    pub fn subscribe(&self) -> crossbeam_channel::Receiver<JobEvent> {
        self.shared.events.subscribe()
    }

    /// ジョブを投入する。
    ///
    /// ジョブは [`JobContext`] を受け取り、結果か [`JobError`] を返す。パニックした場合は
    /// [`JobError::Panicked`] になり、スレッドは動き続ける。終了処理の後に投入したジョブは
    /// 実行されずに [`JobError::Shutdown`] で終わる。
    pub fn submit<T, F>(&self, priority: Priority, job: F) -> JobHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&JobContext) -> Result<T, JobError> + Send + 'static,
    {
        self.submit_with(JobOptions::new(priority), job)
    }

    /// 設定（メモリの宣言・親トークン・表示名）を指定してジョブを投入する。
    pub fn submit_with<T, F>(&self, options: JobOptions, job: F) -> JobHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&JobContext) -> Result<T, JobError> + Send + 'static,
    {
        self.submit_typed(options, None, job)
    }

    /// 「最新の 1 件だけ」を処理するジョブを投入する（04 の 6.1 節の P0、6.2 節）。
    ///
    /// - 同じスロットのまだ始まっていないジョブは実行せずに置き換え、
    ///   [`JobError::Superseded`] で終える。
    /// - 同じスロットで実行中のジョブには、トークンで取り消しを通知する
    ///   （[`SupersedePolicy::CancelRunning`]）。取り消されて終わったジョブの結果も
    ///   [`JobError::Superseded`] になる。
    /// - 返すハンドルの [`JobHandle::generation`] が世代番号（新しい要求ほど大きい）。
    ///
    /// 取り消しは協調的なので、取り消しを確認せずに最後まで実行された古いジョブは
    /// `Ok` を返す。また、スレッドが 2 本以上あると、古いジョブ（実行中）と新しいジョブが
    /// 同時に実行され、結果が届く順序は世代の順にならないことがある。結果を使う側は、
    /// 世代番号を比べて古い結果を捨てること（6.2 節。GPU スレッドのように `threads = 1` なら
    /// 同時には実行されない）。
    pub fn submit_latest<T, F>(
        &self,
        slot: impl Into<SlotKey>,
        priority: Priority,
        job: F,
    ) -> JobHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&JobContext) -> Result<T, JobError> + Send + 'static,
    {
        self.submit_latest_with(slot, JobOptions::new(priority), job)
    }

    /// 設定を指定して「最新の 1 件だけ」のジョブを投入する（[`Scheduler::submit_latest`]）。
    ///
    /// 実行中のジョブの扱いは [`JobOptions::supersede_policy`] で変えられる。
    pub fn submit_latest_with<T, F>(
        &self,
        slot: impl Into<SlotKey>,
        options: JobOptions,
        job: F,
    ) -> JobHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&JobContext) -> Result<T, JobError> + Send + 'static,
    {
        self.submit_typed(options, Some(slot.into()), job)
    }

    fn submit_typed<T, F>(&self, options: JobOptions, slot: Option<SlotKey>, job: F) -> JobHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&JobContext) -> Result<T, JobError> + Send + 'static,
    {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let job_shared = self.shared.new_job(&options);
        let handle = JobHandle::new(Arc::clone(&job_shared), rx, Arc::downgrade(&self.shared));
        self.shared
            .enqueue(&options, slot, job_shared, Box::new(TypedJob::new(job, tx)));
        handle
    }

    /// キューで待っているジョブの数。
    pub fn queued_count(&self) -> usize {
        self.shared.state.lock().queue.len()
    }

    /// 実行中のジョブの数。
    pub fn running_count(&self) -> usize {
        self.shared.state.lock().running.len()
    }

    /// 終了処理を始めたか。
    pub fn is_shut_down(&self) -> bool {
        self.shared.state.lock().shutting_down
    }

    /// キューが空になり、実行中のジョブがなくなるまで待つ。
    ///
    /// ジョブの中から呼ぶと、自分自身の終了を待つことになり戻らない。メモリの枠が取れずに
    /// 待ち続けるジョブがある場合も戻らない。
    pub fn wait_idle(&self) {
        let mut state = self.shared.state.lock();
        while !state.is_idle() {
            self.shared.idle_cv.wait(&mut state);
        }
    }

    /// 終了処理: キューのジョブを取り消し（[`JobError::Shutdown`]）、実行中のジョブが
    /// 終わるのを待ってスレッドを回収する。実行中のジョブには取り消しを通知しない。
    ///
    /// 以後に投入したジョブは実行されない。2 回目以降の呼び出しも、実行中のジョブが
    /// 終わるまで待つ。ジョブの中から呼んだ場合は、そのジョブ自身の終了は待たない。
    pub fn shutdown(&self) {
        self.shutdown_inner(false);
    }

    /// 終了処理: [`Scheduler::shutdown`] に加えて、実行中のジョブに取り消しを通知する。
    ///
    /// アプリの終了時など、実行中の処理を早くやめさせたいときに使う。
    pub fn shutdown_now(&self) {
        self.shutdown_inner(true);
    }

    fn shutdown_inner(&self, cancel_running: bool) {
        let shared = &self.shared;
        let mut deferred = Vec::new();
        let running_tokens = {
            let mut state = shared.state.lock();
            state.shutting_down = true;
            let drained = std::mem::take(&mut state.queue);
            state.queued_rank.clear();
            for queued in drained.into_values() {
                shared.finish_removed(queued, JobError::Shutdown, &mut deferred);
            }
            for slot in state.slots.values_mut() {
                slot.queued = None;
            }
            state.slots.retain(|_, slot| !slot.running.is_empty());
            let tokens: Vec<CancellationToken> = if cancel_running {
                state.running.values().map(|j| j.token.clone()).collect()
            } else {
                Vec::new()
            };
            shared.update_best_rank(&state);
            shared.notify_idle_if_idle(&state);
            shared.work_cv.notify_all();
            tokens
        };
        drop(deferred);
        for token in running_tokens {
            // 利用者が登録した取り消しのコールバックがパニックしても、残りのジョブへの通知と
            // スレッドの回収は最後まで行う（Drop からも呼ばれるため。内容は既定のパニック
            // フックで出力される）。
            let _ = catch_unwind(AssertUnwindSafe(|| token.cancel()));
        }

        let me = std::thread::current().id();
        let handles = std::mem::take(&mut *self.workers.lock());
        for handle in handles {
            if handle.thread().id() != me {
                // 実行スレッドはジョブのパニックを捕まえるので、ここでの失敗は内部の異常だけ。
                let _ = handle.join();
            }
        }
        // 別のスレッドが先にスレッドを回収した場合も、実行中のジョブが終わるまで待つ。
        if !self.worker_ids.contains(&me) {
            let mut state = shared.state.lock();
            while !state.running.is_empty() {
                shared.idle_cv.wait(&mut state);
            }
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.shutdown_now();
        self.shared.budget.remove_listener(self.listener_id);
    }
}

impl fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.shared.state.lock();
        f.debug_struct("Scheduler")
            .field("threads", &self.threads)
            .field("queued", &state.queue.len())
            .field("running", &state.running.len())
            .field("shutting_down", &state.shutting_down)
            .field("budget", &self.shared.budget)
            .finish()
    }
}

/// スケジューラの共有部分（実行スレッド・ハンドル・トークンのコールバックから参照する）。
pub(crate) struct Shared {
    state: Mutex<State>,
    /// 実行スレッドを起こす（投入・取り消し・枠の返却・終了）。
    work_cv: Condvar,
    /// 何もしていない状態になったこと・実行中のジョブが終わったことを知らせる。
    idle_cv: Condvar,
    budget: MemoryBudget,
    events: Arc<EventHub>,
    /// キューで待っているジョブの最も高い優先度の順位（空なら [`NO_QUEUED_RANK`]）。
    /// [`JobContext::should_yield`] がロックなしで読む。
    best_queued_rank: Arc<AtomicU8>,
    next_id: AtomicU64,
}

/// スケジューラの状態（`Shared::state` のロックで守る）。
#[derive(Default)]
struct State {
    /// キュー。キーは（優先度の順位, ID）で、小さいほど先に実行する（ID は投入順）。
    queue: BTreeMap<(u8, JobId), QueuedJob>,
    /// キューにいるジョブの順位（ID からキーを引くため）。
    queued_rank: HashMap<JobId, u8>,
    /// 実行中のジョブ。
    running: HashMap<JobId, Arc<JobShared>>,
    /// 「最新の 1 件だけ」のスロット。
    slots: HashMap<SlotKey, SlotState>,
    /// 終了処理を始めたか。
    shutting_down: bool,
    /// 最後に割り当てた世代番号（0 は未割り当て）。ロックの中で割り当てることで、
    /// 世代番号の順とスロットの置き換えの順を一致させる。
    last_generation: u64,
}

/// キューにいるジョブ。
struct QueuedJob {
    shared: Arc<JobShared>,
    job: Box<dyn ErasedJob>,
    slot: Option<SlotKey>,
    /// キューにいる間に取り消されたら、キューから外すためのコールバックの登録。
    registration: CancelRegistration,
}

/// 「最新の 1 件だけ」のスロットの状態。
#[derive(Default)]
struct SlotState {
    /// まだ始まっていないジョブ（最大 1 件）。
    queued: Option<JobId>,
    /// 実行中のジョブ（取り消しを通知した古いものが終わるまでは複数ありうる）。
    running: Vec<Arc<JobShared>>,
}

/// 実行スレッドが次にすること。
enum Next {
    /// 実行する（宣言したメモリの枠つき）。
    Run(QueuedJob, Option<Reservation>),
    /// 取り消されていたジョブを実行せずに終えたので、残った値をロックの外で drop する。
    Discard(Vec<Deferred>),
}

impl State {
    fn is_idle(&self) -> bool {
        self.queue.is_empty() && self.running.is_empty()
    }

    /// 次の世代番号を割り当てる（1 から始まる）。
    fn allocate_generation(&mut self) -> Generation {
        self.last_generation += 1;
        Generation::from_raw(self.last_generation)
    }

    /// キューからジョブを外す（スロットの記録も更新する）。
    fn remove_queued(&mut self, id: JobId) -> Option<QueuedJob> {
        let rank = self.queued_rank.remove(&id)?;
        let queued = self.queue.remove(&(rank, id))?;
        if let Some(key) = &queued.slot {
            if let Some(slot) = self.slots.get_mut(key)
                && slot.queued == Some(id)
            {
                slot.queued = None;
            }
            self.cleanup_slot(key);
        }
        Some(queued)
    }

    /// 空になったスロットを消す。
    fn cleanup_slot(&mut self, key: &SlotKey) {
        if self
            .slots
            .get(key)
            .is_some_and(|s| s.queued.is_none() && s.running.is_empty())
        {
            self.slots.remove(key);
        }
    }
}

impl Shared {
    /// 新しいジョブの共有状態を作る（ID・トークンを割り当てる）。
    ///
    /// 世代番号はここでは割り当てない（[`Shared::enqueue`] がロックの中で割り当てる）。
    fn new_job(&self, options: &JobOptions) -> Arc<JobShared> {
        let id = JobId::from_raw(self.next_id.fetch_add(1, Ordering::Relaxed));
        let token = match &options.parent {
            Some(parent) => parent.child(),
            None => CancellationToken::new(),
        };
        Arc::new(JobShared::new(id, options, token))
    }

    /// ジョブをキューに入れる。
    fn enqueue(
        self: &Arc<Self>,
        options: &JobOptions,
        slot: Option<SlotKey>,
        job_shared: Arc<JobShared>,
        job: Box<dyn ErasedJob>,
    ) {
        let id = job_shared.id;
        let total = self.budget.total();
        if job_shared.memory_bytes > total {
            // 待っても満たせないので、キューに入れずに失敗させる（スロットの古い要求は
            // 置き換えない）。
            if slot.is_some() {
                let generation = self.state.lock().allocate_generation();
                job_shared.set_generation(generation);
            }
            let error = JobError::Memory(MemoryError::ExceedsBudget {
                requested: job_shared.memory_bytes,
                total,
            });
            self.finish_unqueued(&job_shared, job, error);
            return;
        }

        // キューにいる間に取り消されたら、すぐにキューから外して結果を返す（親トークンの
        // 取り消しを含む）。コールバックはスケジューラのロックを取るので、ロックの外で登録する
        // （既に取り消されていれば、この場で呼ばれて何もしない）。
        let weak = Arc::downgrade(self);
        let registration = job_shared.token.on_cancel(move || {
            if let Some(shared) = weak.upgrade() {
                shared.cancel_queued(id);
            }
        });

        let mut deferred: Vec<Deferred> = Vec::new();
        let mut to_cancel: Vec<CancellationToken> = Vec::new();
        let rejected = {
            let mut state = self.state.lock();
            if slot.is_some() {
                // 世代番号は置き換えと同じロックの中で割り当てる（新しい世代ほど後に置き換える）。
                let generation = state.allocate_generation();
                job_shared.set_generation(generation);
            }
            if state.shutting_down {
                Some((job, registration, JobError::Shutdown))
            } else if job_shared.token.is_cancelled() {
                // 取り消しのフラグはコールバックより先に立つので、ここで見落とした取り消しは
                // コールバックがロックの後にキューから外す。
                Some((job, registration, JobError::Cancelled))
            } else {
                if let Some(key) = &slot {
                    // まだ始まっていない古い要求は、実行せずに「置き換え」で終える。
                    let old_id = state.slots.get_mut(key).and_then(|s| s.queued.take());
                    if let Some(old) = old_id.and_then(|old_id| state.remove_queued(old_id)) {
                        old.shared.mark_superseded();
                        to_cancel.push(old.shared.token.clone());
                        self.finish_removed(old, JobError::Superseded, &mut deferred);
                    }
                    // 実行中の古い要求には、取り消しを通知する（方針による）。
                    let slot_state = state.slots.entry(key.clone()).or_default();
                    for running in &slot_state.running {
                        running.mark_superseded();
                        if options.supersede == SupersedePolicy::CancelRunning {
                            to_cancel.push(running.token.clone());
                        }
                    }
                    slot_state.queued = Some(id);
                }
                // キューに入れる前に送る（実行開始のイベントより必ず先になる）。
                self.events
                    .emit(job_shared.event(JobEventKind::State(JobState::Queued)));
                let rank = job_shared.priority().rank();
                state.queued_rank.insert(id, rank);
                state.queue.insert(
                    (rank, id),
                    QueuedJob {
                        shared: Arc::clone(&job_shared),
                        job,
                        slot,
                        registration,
                    },
                );
                self.update_best_rank(&state);
                self.work_cv.notify_one();
                None
            }
        };
        drop(deferred);

        if let Some((job, registration, error)) = rejected {
            drop(registration);
            self.finish_unqueued(&job_shared, job, error);
        }
        for token in to_cancel {
            token.cancel();
        }
    }

    /// キューにいるジョブを取り消す（トークンのコールバックから呼ぶ）。
    fn cancel_queued(&self, id: JobId) {
        let mut deferred = Vec::new();
        {
            let mut state = self.state.lock();
            if let Some(queued) = state.remove_queued(id) {
                self.finish_removed(queued, JobError::Cancelled, &mut deferred);
                self.update_best_rank(&state);
                self.notify_idle_if_idle(&state);
                // 先頭が変わると、メモリを待っていた実行スレッドが次のジョブを始められることがある。
                self.work_cv.notify_all();
            }
        }
        drop(deferred);
    }

    /// キューにいるジョブの優先度を変える（[`JobHandle::set_priority`]）。
    pub(crate) fn reprioritize(&self, job: &JobShared, priority: Priority) -> bool {
        let mut state = self.state.lock();
        let Some(&old_rank) = state.queued_rank.get(&job.id) else {
            return false;
        };
        let new_rank = priority.rank();
        if old_rank != new_rank {
            let Some(queued) = state.queue.remove(&(old_rank, job.id)) else {
                return false;
            };
            state.queue.insert((new_rank, job.id), queued);
            state.queued_rank.insert(job.id, new_rank);
            job.set_priority(priority);
            self.update_best_rank(&state);
            self.work_cv.notify_all();
        }
        true
    }

    /// 実行スレッドの本体。
    fn worker_loop(self: Arc<Self>) {
        loop {
            let next = {
                let mut state = self.state.lock();
                loop {
                    if state.shutting_down {
                        break None;
                    }
                    if let Some(next) = self.take_next(&mut state) {
                        break Some(next);
                    }
                    self.work_cv.wait(&mut state);
                }
            };
            match next {
                None => return,
                Some(Next::Discard(deferred)) => drop_on_worker(deferred),
                Some(Next::Run(queued, reservation)) => self.run(queued, reservation),
            }
        }
    }

    /// キューの先頭のジョブを取り出す。メモリの枠が取れなければ `None`（先頭を飛ばさない）。
    fn take_next(&self, state: &mut State) -> Option<Next> {
        let (&(rank, id), head) = state.queue.first_key_value()?;
        if head.shared.token.is_cancelled() {
            // コールバックより先に気づいた場合。実行せずに終える。
            let queued = state.remove_queued(id)?;
            let mut deferred = Vec::new();
            self.finish_removed(queued, JobError::Cancelled, &mut deferred);
            self.update_best_rank(state);
            self.notify_idle_if_idle(state);
            return Some(Next::Discard(deferred));
        }
        let memory_bytes = head.shared.memory_bytes;
        let reservation = if memory_bytes == 0 {
            None
        } else {
            let priority = Priority::from_rank(rank).unwrap_or(Priority::P3);
            Some(
                self.budget
                    .try_acquire_with_priority(memory_bytes, priority)
                    .ok()?,
            )
        };
        let Some(queued) = state.remove_queued(id) else {
            // 到達しない（先頭はキューにある）。取った枠はロックの外で返却する。
            let deferred = reservation
                .into_iter()
                .map(|r| Box::new(r) as Deferred)
                .collect();
            return Some(Next::Discard(deferred));
        };
        state.running.insert(id, Arc::clone(&queued.shared));
        if let Some(key) = &queued.slot {
            state
                .slots
                .entry(key.clone())
                .or_default()
                .running
                .push(Arc::clone(&queued.shared));
        }
        queued.shared.set_state(JobState::Running);
        self.update_best_rank(state);
        Some(Next::Run(queued, reservation))
    }

    /// ジョブを実行し、後始末をして結果を届ける。
    fn run(&self, queued: QueuedJob, reservation: Option<Reservation>) {
        let QueuedJob {
            shared: job_shared,
            job,
            slot,
            registration,
        } = queued;
        // 実行を始めたら、キューから外すためのコールバックは不要。
        drop(registration);
        self.events
            .emit(job_shared.event(JobEventKind::State(JobState::Running)));

        let ctx = JobContext::new(
            Arc::clone(&job_shared),
            self.budget.clone(),
            Arc::clone(&self.events),
            Arc::clone(&self.best_queued_rank),
            reservation,
        );
        let completion = job.run(&ctx);
        // 宣言したメモリの枠をロックの外で返却する。
        drop(ctx);

        let id = job_shared.id;
        let mut deferred = Vec::new();
        {
            let mut state = self.state.lock();
            self.complete(&job_shared, completion, &mut deferred);
            state.running.remove(&id);
            if let Some(key) = &slot {
                if let Some(slot_state) = state.slots.get_mut(key) {
                    slot_state.running.retain(|j| j.id != id);
                }
                state.cleanup_slot(key);
            }
            if state.running.is_empty() {
                // 何もしていない状態を待つ呼び出し元（wait_idle）と、実行中のジョブの終了を
                // 待つ終了処理へ知らせる。
                self.idle_cv.notify_all();
            }
        }
        drop_on_worker(deferred);
    }

    /// 結果を届けて、終わった状態にする。
    ///
    /// 順序は「イベント → 結果 → 状態」。状態が終わったと見えた時点で、イベントは送信済みで、
    /// 結果は取り出せる。スケジューラのロックの中でも外でも呼べる（値の drop は `deferred` に
    /// 回す）。
    fn complete(
        &self,
        job_shared: &JobShared,
        completion: Box<dyn Completion>,
        deferred: &mut Vec<Deferred>,
    ) {
        let final_state = completion.state();
        self.events
            .emit(job_shared.event(JobEventKind::State(final_state)));
        // 送り側（と届かなかった結果）はロックの外で drop する（Completion::deliver を参照）。
        deferred.push(completion.deliver());
        job_shared.set_state(final_state);
    }

    /// キューから外したジョブを、実行せずに `error` で終える（ロックの中で呼ぶ）。
    fn finish_removed(&self, queued: QueuedJob, error: JobError, deferred: &mut Vec<Deferred>) {
        let QueuedJob {
            shared: job_shared,
            job,
            registration,
            ..
        } = queued;
        let (completion, body) = job.abort(error);
        deferred.push(body);
        deferred.push(Box::new(registration));
        self.complete(&job_shared, completion, deferred);
        // ハンドルが既に drop されていれば、これが最後の参照になる。トークンに登録された
        // 利用者のコールバック（捕捉した値）の drop もロックの外で行う。
        deferred.push(Box::new(job_shared));
    }

    /// キューに入れなかったジョブを、実行せずに `error` で終える（ロックの外で呼ぶ）。
    fn finish_unqueued(&self, job_shared: &JobShared, job: Box<dyn ErasedJob>, error: JobError) {
        let (completion, body) = job.abort(error);
        let mut deferred = vec![body];
        self.complete(job_shared, completion, &mut deferred);
        drop(deferred);
    }

    /// キューの最も高い優先度を記録する（ロックの中で呼ぶ）。
    fn update_best_rank(&self, state: &State) {
        let rank = state
            .queue
            .first_key_value()
            .map_or(NO_QUEUED_RANK, |(&(rank, _), _)| rank);
        self.best_queued_rank.store(rank, Ordering::Release);
    }

    /// 何もしていない状態なら、待っている呼び出し元を起こす（ロックの中で呼ぶ）。
    fn notify_idle_if_idle(&self, state: &State) {
        if state.is_idle() {
            self.idle_cv.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_threads_is_rejected() {
        assert!(matches!(
            Scheduler::new(SchedulerConfig::new(0, 1024)),
            Err(SchedulerError::ZeroThreads)
        ));
    }

    #[test]
    fn config_helpers() {
        let c = SchedulerConfig::single_thread(10).thread_name("gpu");
        assert_eq!(c.threads, 1);
        assert_eq!(c.memory_budget_bytes, 10);
        assert_eq!(c.thread_name, "gpu");
        let b = SchedulerConfig::background();
        assert!(b.threads >= 1);
        assert_eq!(b.memory_budget_bytes, DEFAULT_NORMAL_BUDGET_BYTES);
        assert_eq!(b.thread_name, DEFAULT_THREAD_NAME);
    }

    #[test]
    fn threads_are_named() {
        let scheduler = Scheduler::new(SchedulerConfig::new(2, 0).thread_name("gpu")).unwrap();
        let name = scheduler
            .submit(Priority::P0, |_| {
                Ok(std::thread::current().name().map(str::to_owned))
            })
            .wait()
            .unwrap()
            .unwrap();
        assert!(name == "gpu-0" || name == "gpu-1", "{name}");
        assert_eq!(scheduler.threads(), 2);
    }

    #[test]
    fn state_is_clean_after_jobs() {
        let scheduler = Scheduler::new(SchedulerConfig::new(2, 100)).unwrap();
        let handles: Vec<_> = (0..20)
            .map(|i| {
                scheduler.submit_latest_with(
                    format!("slot-{}", i % 3),
                    JobOptions::new(Priority::P1).memory_bytes(10),
                    |_| Ok(()),
                )
            })
            .collect();
        for h in handles {
            let _ = h.wait();
        }
        scheduler.wait_idle();
        let state = scheduler.shared.state.lock();
        assert!(state.queue.is_empty());
        assert!(state.queued_rank.is_empty());
        assert!(state.running.is_empty());
        assert!(state.slots.is_empty(), "空のスロットが残っている");
        assert_eq!(
            scheduler.shared.best_queued_rank.load(Ordering::Acquire),
            NO_QUEUED_RANK
        );
        drop(state);
        assert_eq!(scheduler.budget().used(), 0);
    }
}
