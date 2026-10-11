//! ジョブの型: ID・世代番号・状態・エラー・投入の設定・ハンドル・実行中のコンテキスト。

use std::any::Any;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use parking_lot::Mutex;

use crate::cancel::{CancellationToken, Cancelled};
use crate::events::{EventHub, JobEvent, JobEventKind, PROGRESS_EVENT_STEPS};
use crate::memory::{MemoryBudget, MemoryError, Reservation};
use crate::priority::Priority;
use crate::scheduler::Shared;

/// ジョブの ID。スケジューラの中で一意で、投入順に大きくなる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(u64);

impl JobId {
    /// 数値から作る（スケジューラの内部とテスト用）。
    pub(crate) const fn from_raw(raw: u64) -> Self {
        JobId(raw)
    }

    /// 数値の値。
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "job#{}", self.0)
    }
}

/// 「最新の 1 件だけ」の要求の世代番号（04 の 6.2 節）。
///
/// [`crate::Scheduler::submit_latest`] で投入するたびに大きくなる。同じスロットの中では
/// 新しい要求ほど大きい（番号はスケジューラ全体で一意で、スロットごとの連番ではない）。
/// 番号はスケジューラのロックの中で、キューに入れる（古い要求を置き換える）順に割り当てる。
/// そのため、複数のスレッドから同じスロットへ同時に投入しても、スロットに残るのは常に
/// 最も大きい世代の要求になる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Generation(u64);

impl Generation {
    /// 数値から作る（スケジューラの内部用）。
    pub(crate) const fn from_raw(raw: u64) -> Self {
        Generation(raw)
    }

    /// 数値の値。
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "gen#{}", self.0)
    }
}

/// 「最新の 1 件だけ」を処理する単位（スロット）のキー。
///
/// 例: 現像中の写真のプレビュー更新（`"develop-preview"`）、ルーペの表示（`"loupe"`）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SlotKey(Arc<str>);

impl SlotKey {
    /// 文字列から作る。
    pub fn new(key: impl AsRef<str>) -> Self {
        SlotKey(Arc::from(key.as_ref()))
    }

    /// キーの文字列。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SlotKey {
    fn from(key: &str) -> Self {
        SlotKey::new(key)
    }
}

impl From<String> for SlotKey {
    fn from(key: String) -> Self {
        SlotKey(Arc::from(key))
    }
}

impl fmt::Display for SlotKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 新しい要求が来たときの、同じスロットで実行中のジョブの扱い（04 の 6.2 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SupersedePolicy {
    /// 実行中のジョブに取り消しを通知する（既定）。
    #[default]
    CancelRunning,
    /// 実行中のジョブは最後まで実行させる（まだ始まっていないジョブだけを置き換える）。
    ///
    /// 6.2 節の図のように、ドラッグ中も途中の結果を描画したい場合に使う。要求の間隔が
    /// 処理時間より短いと、`CancelRunning` では結果が 1 つも出ないことがあるため。
    KeepRunning,
}

/// ジョブの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum JobState {
    /// キューで実行を待っている。
    Queued = 0,
    /// 実行中。
    Running = 1,
    /// 成功した。
    Succeeded = 2,
    /// 失敗した（ジョブのエラー、メモリの予算のエラー）。
    Failed = 3,
    /// 取り消された（スケジューラの終了で実行されなかった場合を含む）。
    Cancelled = 4,
    /// 新しい要求に置き換えられた（[`crate::Scheduler::submit_latest`]）。
    Superseded = 5,
    /// パニックした。
    Panicked = 6,
}

impl JobState {
    /// 終わった状態か（`Queued`・`Running` 以外）。
    pub const fn is_terminal(self) -> bool {
        !matches!(self, JobState::Queued | JobState::Running)
    }

    /// 表示・ログ用の名前。
    pub const fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Succeeded => "succeeded",
            JobState::Failed => "failed",
            JobState::Cancelled => "cancelled",
            JobState::Superseded => "superseded",
            JobState::Panicked => "panicked",
        }
    }

    const fn from_u8(value: u8) -> JobState {
        match value {
            0 => JobState::Queued,
            1 => JobState::Running,
            2 => JobState::Succeeded,
            3 => JobState::Failed,
            4 => JobState::Cancelled,
            5 => JobState::Superseded,
            _ => JobState::Panicked,
        }
    }

    /// 結果から終わった状態を求める。
    pub(crate) fn from_result<T>(result: &Result<T, JobError>) -> JobState {
        match result {
            Ok(_) => JobState::Succeeded,
            Err(e) => e.state(),
        }
    }
}

impl fmt::Display for JobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// ジョブのエラー。
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    /// 取り消された。
    #[error("ジョブは取り消されました")]
    Cancelled,
    /// 新しい要求に置き換えられた（[`crate::Scheduler::submit_latest`]）。
    #[error("ジョブは新しい要求に置き換えられました")]
    Superseded,
    /// スケジューラが終了したため、実行されなかった。
    #[error("スケジューラが終了したため、ジョブは実行されませんでした")]
    Shutdown,
    /// ジョブがパニックした（メッセージ）。スレッドプールは動き続ける。
    #[error("ジョブがパニックしました: {0}")]
    Panicked(String),
    /// メモリの予算のエラー（総量を超える宣言など）。
    #[error(transparent)]
    Memory(MemoryError),
    /// ジョブ自身のエラー。
    #[error(transparent)]
    Failed(Box<dyn std::error::Error + Send + Sync + 'static>),
    /// 結果は既に取り出されている（[`JobHandle::try_result`] などで取り出した後）。
    #[error("ジョブの結果は既に取り出されています")]
    ResultTaken,
}

impl JobError {
    /// ジョブ自身のエラーを作る（文字列も渡せる）。
    pub fn failed(error: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>) -> Self {
        JobError::Failed(error.into())
    }

    /// 取り消し・置き換え・終了で実行されなかった（または途中でやめた）か。
    pub fn is_cancellation(&self) -> bool {
        matches!(
            self,
            JobError::Cancelled | JobError::Superseded | JobError::Shutdown
        )
    }

    /// このエラーで終わったジョブの状態。
    pub fn state(&self) -> JobState {
        match self {
            JobError::Cancelled | JobError::Shutdown => JobState::Cancelled,
            JobError::Superseded => JobState::Superseded,
            JobError::Panicked(_) => JobState::Panicked,
            JobError::Memory(_) | JobError::Failed(_) | JobError::ResultTaken => JobState::Failed,
        }
    }
}

impl From<Cancelled> for JobError {
    fn from(_: Cancelled) -> Self {
        JobError::Cancelled
    }
}

impl From<MemoryError> for JobError {
    fn from(error: MemoryError) -> Self {
        match error {
            MemoryError::Cancelled => JobError::Cancelled,
            other => JobError::Memory(other),
        }
    }
}

/// ジョブを投入するときの設定。
#[derive(Debug, Clone)]
pub struct JobOptions {
    pub(crate) priority: Priority,
    pub(crate) memory_bytes: u64,
    pub(crate) parent: Option<CancellationToken>,
    pub(crate) label: Option<Arc<str>>,
    pub(crate) supersede: SupersedePolicy,
}

impl JobOptions {
    /// 優先度 `priority` の設定（メモリの宣言なし・親トークンなし・表示名なし）。
    pub fn new(priority: Priority) -> Self {
        JobOptions {
            priority,
            memory_bytes: 0,
            parent: None,
            label: None,
            supersede: SupersedePolicy::default(),
        }
    }

    /// ジョブが使うメモリを宣言する（04 の 6.1 節「確保する前に予算から枠を取る」）。
    ///
    /// スケジューラは、この枠を予算から取れるまでジョブを始めない（並列度をスレッド数と
    /// 予算の両方で制限する）。枠はジョブの実行中だけ持ち、終わったら返却する
    /// （[`JobContext::take_reservation`] で結果と一緒に持ち出すこともできる）。
    /// 予算の総量を超える宣言は、投入した時点で [`MemoryError::ExceedsBudget`] で失敗する。
    pub fn memory_bytes(mut self, bytes: u64) -> Self {
        self.memory_bytes = bytes;
        self
    }

    /// 親トークンを指定する。ジョブのトークンはこの子になり、親を取り消すとジョブも
    /// 取り消される（例: グリッドの表示範囲のサムネイルをまとめて取り消す）。
    pub fn parent(mut self, parent: &CancellationToken) -> Self {
        self.parent = Some(parent.clone());
        self
    }

    /// 表示名（進捗の表示・ログ用）。
    pub fn label(mut self, label: impl AsRef<str>) -> Self {
        self.label = Some(Arc::from(label.as_ref()));
        self
    }

    /// 同じスロットで実行中のジョブの扱い（[`crate::Scheduler::submit_latest_with`] だけで使う）。
    pub fn supersede_policy(mut self, policy: SupersedePolicy) -> Self {
        self.supersede = policy;
        self
    }

    /// 優先度。
    pub fn priority(&self) -> Priority {
        self.priority
    }
}

/// ジョブとハンドル・コンテキストが共有する状態。
pub(crate) struct JobShared {
    pub(crate) id: JobId,
    priority: AtomicU8,
    /// 世代番号（0 はなし）。キューに入れるときに、スケジューラのロックの中で割り当てる
    /// （[`JobShared::set_generation`]）。
    generation: AtomicU64,
    pub(crate) label: Option<Arc<str>>,
    pub(crate) token: CancellationToken,
    pub(crate) memory_bytes: u64,
    state: AtomicU8,
    superseded: AtomicBool,
    /// 進捗（f32 のビット列）。
    progress_bits: AtomicU32,
    /// 最後にイベントで送った進捗の段階（未送信は `u32::MAX`）。
    last_progress_step: AtomicU32,
}

impl JobShared {
    pub(crate) fn new(id: JobId, options: &JobOptions, token: CancellationToken) -> Self {
        JobShared {
            id,
            priority: AtomicU8::new(options.priority.rank()),
            generation: AtomicU64::new(0),
            label: options.label.clone(),
            token,
            memory_bytes: options.memory_bytes,
            state: AtomicU8::new(JobState::Queued as u8),
            superseded: AtomicBool::new(false),
            progress_bits: AtomicU32::new(0.0_f32.to_bits()),
            last_progress_step: AtomicU32::new(u32::MAX),
        }
    }

    pub(crate) fn priority(&self) -> Priority {
        Priority::from_rank(self.priority.load(Ordering::Acquire)).unwrap_or(Priority::P3)
    }

    /// 世代番号（[`crate::Scheduler::submit_latest`] で投入した場合）。
    pub(crate) fn generation(&self) -> Option<Generation> {
        match self.generation.load(Ordering::Acquire) {
            0 => None,
            raw => Some(Generation::from_raw(raw)),
        }
    }

    /// 世代番号を記録する（スケジューラが投入の処理の中で 1 回だけ呼ぶ）。
    pub(crate) fn set_generation(&self, generation: Generation) {
        debug_assert!(generation.get() != 0, "世代番号 0 は「なし」を表す");
        self.generation.store(generation.get(), Ordering::Release);
    }

    pub(crate) fn set_priority(&self, priority: Priority) {
        self.priority.store(priority.rank(), Ordering::Release);
    }

    pub(crate) fn state(&self) -> JobState {
        JobState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub(crate) fn set_state(&self, state: JobState) {
        self.state.store(state as u8, Ordering::Release);
        if state == JobState::Succeeded {
            self.progress_bits
                .store(1.0_f32.to_bits(), Ordering::Release);
        }
    }

    pub(crate) fn is_superseded(&self) -> bool {
        self.superseded.load(Ordering::Acquire)
    }

    pub(crate) fn mark_superseded(&self) {
        self.superseded.store(true, Ordering::Release);
    }

    pub(crate) fn progress(&self) -> f32 {
        f32::from_bits(self.progress_bits.load(Ordering::Acquire))
    }

    /// イベントを作る。
    pub(crate) fn event(&self, kind: JobEventKind) -> JobEvent {
        JobEvent {
            id: self.id,
            priority: self.priority(),
            label: self.label.clone(),
            kind,
        }
    }

    /// 進捗を記録し、段階が変わったらイベントを送る。
    fn report_progress(&self, progress: f32, events: &EventHub) {
        if progress.is_nan() {
            return;
        }
        let progress = progress.clamp(0.0, 1.0);
        self.progress_bits
            .store(progress.to_bits(), Ordering::Release);
        // 0〜1 を 0〜PROGRESS_EVENT_STEPS に丸める（範囲内なので u32 に収まる）。
        let step = (progress * PROGRESS_EVENT_STEPS as f32).round() as u32;
        if self.last_progress_step.swap(step, Ordering::AcqRel) != step {
            events.emit(self.event(JobEventKind::Progress(progress)));
        }
    }
}

/// スケジューラのロックの外で drop する値（ジョブ本体・届かなかった結果など）。
///
/// ジョブのクロージャや結果は、メモリの枠（[`Reservation`]）を捕捉していることがある。
/// 枠の返却はスケジューラのロックを取るので、ロックの中で drop してはいけない。
pub(crate) type Deferred = Box<dyn Send>;

/// 型を消したジョブ（スケジューラのキューに入れる）。
pub(crate) trait ErasedJob: Send {
    /// 実行する。パニックは捕まえて [`JobError::Panicked`] にする。
    fn run(self: Box<Self>, ctx: &JobContext) -> Box<dyn Completion>;
    /// 実行せずに `error` で終える準備をする。届ける結果と、ジョブ本体（ロックの外で drop する）を
    /// 分けて返す。
    fn abort(self: Box<Self>, error: JobError) -> (Box<dyn Completion>, Deferred);
}

/// 終わったジョブの結果（届ける前）。
pub(crate) trait Completion: Send {
    /// 結果から求めた終わった状態。
    fn state(&self) -> JobState;
    /// 結果をハンドルに届ける。待つことはない（チャネルには 1 件分の空きがある）。
    ///
    /// 送り側（と、ハンドルが drop されていて届かなかった結果）を返す。呼び出し元は
    /// スケジューラのロックの外で drop すること。送った直後にハンドルが drop されると、
    /// 通路とその中の結果は送り側の drop で破棄されるため（結果が持つメモリの枠の返却は
    /// スケジューラのロックを取る）。
    fn deliver(self: Box<Self>) -> Deferred;
}

/// 型つきのジョブ。
pub(crate) struct TypedJob<T, F> {
    job: F,
    tx: Sender<Result<T, JobError>>,
}

impl<T, F> TypedJob<T, F> {
    pub(crate) fn new(job: F, tx: Sender<Result<T, JobError>>) -> Self {
        TypedJob { job, tx }
    }
}

struct TypedCompletion<T> {
    result: Result<T, JobError>,
    tx: Sender<Result<T, JobError>>,
}

impl<T: Send + 'static> Completion for TypedCompletion<T> {
    fn state(&self) -> JobState {
        JobState::from_result(&self.result)
    }

    fn deliver(self: Box<Self>) -> Deferred {
        let TypedCompletion { result, tx } = *self;
        // 送り側はここで drop しない（trait の説明を参照）。
        match tx.send(result) {
            Ok(()) => Box::new(tx),
            Err(undelivered) => Box::new((undelivered.into_inner(), tx)),
        }
    }
}

impl<T, F> ErasedJob for TypedJob<T, F>
where
    T: Send + 'static,
    F: FnOnce(&JobContext) -> Result<T, JobError> + Send + 'static,
{
    fn run(self: Box<Self>, ctx: &JobContext) -> Box<dyn Completion> {
        let TypedJob { job, tx } = *self;
        // パニックしたジョブはエラーとして返し、スレッドは動き続ける。ジョブの外の状態は
        // 不可分な値とロックだけで、パニックで壊れた状態を観測することはない。
        let mut result = match catch_unwind(AssertUnwindSafe(|| job(ctx))) {
            Ok(result) => result,
            Err(payload) => Err(JobError::Panicked(panic_message(payload.as_ref()))),
        };
        // 置き換えで取り消されたジョブは「置き換え」として報告する。
        if matches!(result, Err(JobError::Cancelled)) && ctx.is_superseded() {
            result = Err(JobError::Superseded);
        }
        Box::new(TypedCompletion { result, tx })
    }

    fn abort(self: Box<Self>, error: JobError) -> (Box<dyn Completion>, Deferred) {
        let TypedJob { job, tx } = *self;
        (
            Box::new(TypedCompletion::<T> {
                result: Err(error),
                tx,
            }),
            Box::new(job),
        )
    }
}

/// パニックの内容を文字列にする。
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "（文字列ではないパニック）".to_owned()
    }
}

/// 実行中のジョブに渡すコンテキスト。
///
/// 取り消しの確認、進捗の報告、メモリの予約に使う。複数のスレッドから同時に使える
/// （ジョブの中で rayon などで並列化する場合）。
pub struct JobContext {
    job: Arc<JobShared>,
    budget: MemoryBudget,
    events: Arc<EventHub>,
    best_queued_rank: Arc<AtomicU8>,
    reservation: Mutex<Option<Reservation>>,
}

impl JobContext {
    pub(crate) fn new(
        job: Arc<JobShared>,
        budget: MemoryBudget,
        events: Arc<EventHub>,
        best_queued_rank: Arc<AtomicU8>,
        reservation: Option<Reservation>,
    ) -> Self {
        JobContext {
            job,
            budget,
            events,
            best_queued_rank,
            reservation: Mutex::new(reservation),
        }
    }

    /// ジョブの ID。
    pub fn id(&self) -> JobId {
        self.job.id
    }

    /// ジョブの優先度。
    pub fn priority(&self) -> Priority {
        self.job.priority()
    }

    /// 世代番号（[`crate::Scheduler::submit_latest`] で投入した場合）。
    pub fn generation(&self) -> Option<Generation> {
        self.job.generation()
    }

    /// 表示名。
    pub fn label(&self) -> Option<&str> {
        self.job.label.as_deref()
    }

    /// ジョブの取り消しトークン。子トークンを作って下位の処理に渡せる。
    pub fn token(&self) -> &CancellationToken {
        &self.job.token
    }

    /// 取り消されたか（置き換えによる取り消しを含む）。
    pub fn is_cancelled(&self) -> bool {
        self.job.token.is_cancelled()
    }

    /// 取り消されていれば `Err(Cancelled)` を返す。処理の区切り（タイルごと・ステージごと）で
    /// `ctx.check()?` のように使う（6.1 節）。
    pub fn check(&self) -> Result<(), Cancelled> {
        self.job.token.check()
    }

    /// 同じスロットの新しい要求に置き換えられたか（[`crate::Scheduler::submit_latest`]）。
    pub fn is_superseded(&self) -> bool {
        self.job.is_superseded()
    }

    /// このジョブより優先度の高いジョブがキューで待っているか。
    ///
    /// 長い処理を区切りで中断して、残りを投入し直すかを決めるのに使う（6.1 節「P0 の要求が
    /// 来たらタイルの合間に割り込めるようにする」、PERF-13）。
    pub fn should_yield(&self) -> bool {
        self.best_queued_rank.load(Ordering::Acquire) < self.priority().rank()
    }

    /// 進捗（0〜1）を報告する。範囲外の値は 0〜1 に収め、NaN は無視する。
    pub fn report_progress(&self, progress: f32) {
        self.job.report_progress(progress, &self.events);
    }

    /// メモリの予算。
    pub fn budget(&self) -> &MemoryBudget {
        &self.budget
    }

    /// 予算から `bytes` の枠を追加で取る。空くまで待ち、ジョブが取り消されたら
    /// [`MemoryError::Cancelled`] を返す（ジョブの優先度で待つ）。
    ///
    /// 枠を持ったまま追加の枠を待つと、他のジョブと互いに待ち合うことがある。なるべく
    /// 投入時に [`JobOptions::memory_bytes`] でまとめて宣言する。
    ///
    /// 投入時に宣言した枠（[`JobContext::reserved_bytes`]）はジョブの実行中は返却されないため、
    /// `宣言した枠 + bytes` が予算の総量を超える要求は、待っても満たせない。この場合は待たずに
    /// [`MemoryError::ExceedsBudget`]（`requested` は `宣言した枠 + bytes`）を返す。
    /// [`JobContext::reserve`] で別に取って持っている枠は数えない。
    pub fn reserve(&self, bytes: u64) -> Result<Reservation, MemoryError> {
        let held = self.reserved_bytes();
        let total = self.budget.total();
        // 宣言した枠は総量以下なので、引き算は桁あふれしない。
        if bytes > total - held.min(total) {
            return Err(MemoryError::ExceedsBudget {
                requested: held.saturating_add(bytes),
                total,
            });
        }
        self.budget
            .acquire_with_priority(bytes, self.priority(), &self.job.token)
    }

    /// 待たずに予算から `bytes` の枠を取る。
    pub fn try_reserve(&self, bytes: u64) -> Result<Reservation, MemoryError> {
        self.budget
            .try_acquire_with_priority(bytes, self.priority())
    }

    /// 投入時に宣言したメモリの枠の大きさ（持ち出した後は 0）。
    pub fn reserved_bytes(&self) -> u64 {
        self.reservation
            .lock()
            .as_ref()
            .map_or(0, Reservation::bytes)
    }

    /// 投入時に宣言したメモリの枠を持ち出す。
    ///
    /// 結果のバッファと一緒に返して、バッファを使い終わるまで枠を持ち続けるときに使う。
    /// 持ち出さなければ、ジョブが終わったときに返却する。
    pub fn take_reservation(&self) -> Option<Reservation> {
        self.reservation.lock().take()
    }
}

impl fmt::Debug for JobContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobContext")
            .field("id", &self.job.id)
            .field("priority", &self.priority())
            .field("generation", &self.job.generation())
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// 投入したジョブのハンドル。結果の待機、取り消し、状態の取得に使う。
///
/// ハンドルを drop してもジョブは取り消されない（結果は捨てられる）。
pub struct JobHandle<T> {
    job: Arc<JobShared>,
    rx: Receiver<Result<T, JobError>>,
    scheduler: Weak<Shared>,
    taken: bool,
}

impl<T> JobHandle<T> {
    pub(crate) fn new(
        job: Arc<JobShared>,
        rx: Receiver<Result<T, JobError>>,
        scheduler: Weak<Shared>,
    ) -> Self {
        JobHandle {
            job,
            rx,
            scheduler,
            taken: false,
        }
    }

    /// ジョブの ID。
    pub fn id(&self) -> JobId {
        self.job.id
    }

    /// ジョブの優先度。
    pub fn priority(&self) -> Priority {
        self.job.priority()
    }

    /// 世代番号（[`crate::Scheduler::submit_latest`] で投入した場合）。
    pub fn generation(&self) -> Option<Generation> {
        self.job.generation()
    }

    /// 表示名。
    pub fn label(&self) -> Option<&str> {
        self.job.label.as_deref()
    }

    /// 今の状態。
    pub fn state(&self) -> JobState {
        self.job.state()
    }

    /// 終わったか（結果を取り出せる状態か）。
    pub fn is_finished(&self) -> bool {
        self.state().is_terminal()
    }

    /// 最後に報告された進捗（0〜1）。成功したら 1。
    pub fn progress(&self) -> f32 {
        self.job.progress()
    }

    /// ジョブの取り消しトークン。
    pub fn token(&self) -> &CancellationToken {
        &self.job.token
    }

    /// 取り消す。
    ///
    /// キューにいるジョブは実行されずに [`JobError::Cancelled`] で終わる。実行中のジョブには
    /// トークンで通知する（ジョブが確認してやめるまで実行は続く）。終わったジョブには影響しない。
    pub fn cancel(&self) {
        self.job.token.cancel();
    }

    /// キューにいるジョブの優先度を変える（例: 先読み P2 が表示範囲に入って P1 になった）。
    ///
    /// キューの中での位置は、新しい優先度の中で投入順（ID の順）になる。変えられた場合は
    /// `true`。実行中・終わったジョブは変えられない（`false`）。
    pub fn set_priority(&self, priority: Priority) -> bool {
        match self.scheduler.upgrade() {
            Some(shared) => shared.reprioritize(&self.job, priority),
            None => false,
        }
    }

    /// 終わるまで待って結果を返す。
    ///
    /// ジョブの中から、同じスケジューラの（まだ始まっていない）別のジョブを待つと、空いている
    /// スレッドがなければ戻らない。特に `threads = 1` のスケジューラ（GPU スレッド）では、
    /// ジョブの中で同じスケジューラのジョブを待たないこと。
    pub fn wait(mut self) -> Result<T, JobError> {
        if self.taken {
            return Err(JobError::ResultTaken);
        }
        self.taken = true;
        // 送り側が結果を送らずに消えるのは、スケジューラの内部の異常で実行スレッドが
        // 終わった場合だけ。実行されなかったものとして扱う。
        self.rx.recv().unwrap_or(Err(JobError::Shutdown))
    }

    /// 終わっていれば結果を返す（待たない）。結果は 1 度だけ取り出せ、2 回目以降は `None`。
    pub fn try_result(&mut self) -> Option<Result<T, JobError>> {
        if self.taken {
            return None;
        }
        let result = match self.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(JobError::Shutdown),
        };
        self.taken = true;
        Some(result)
    }

    /// 最大 `timeout` だけ待って、終わっていれば結果を返す。結果は 1 度だけ取り出せる。
    pub fn wait_timeout(&mut self, timeout: Duration) -> Option<Result<T, JobError>> {
        if self.taken {
            return None;
        }
        let result = match self.rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => Err(JobError::Shutdown),
        };
        self.taken = true;
        Some(result)
    }
}

impl<T> fmt::Debug for JobHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobHandle")
            .field("id", &self.job.id)
            .field("priority", &self.priority())
            .field("generation", &self.job.generation())
            .field("state", &self.state())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_generations_display() {
        assert_eq!(JobId::from_raw(7).to_string(), "job#7");
        assert_eq!(JobId::from_raw(7).get(), 7);
        assert_eq!(Generation::from_raw(3).to_string(), "gen#3");
        assert!(Generation::from_raw(3) < Generation::from_raw(4));
    }

    #[test]
    fn slot_keys_compare_by_content() {
        let a = SlotKey::from("develop-preview");
        let b = SlotKey::from(String::from("develop-preview"));
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "develop-preview");
        assert_eq!(a.to_string(), "develop-preview");
        assert_ne!(a, SlotKey::new("loupe"));
    }

    #[test]
    fn job_state_round_trips_and_terminal_flags() {
        let all = [
            JobState::Queued,
            JobState::Running,
            JobState::Succeeded,
            JobState::Failed,
            JobState::Cancelled,
            JobState::Superseded,
            JobState::Panicked,
        ];
        for s in all {
            assert_eq!(JobState::from_u8(s as u8), s);
        }
        assert!(!JobState::Queued.is_terminal());
        assert!(!JobState::Running.is_terminal());
        assert!(all[2..].iter().all(|s| s.is_terminal()));
        assert_eq!(JobState::Superseded.to_string(), "superseded");
    }

    #[test]
    fn errors_map_to_states() {
        assert_eq!(JobError::Cancelled.state(), JobState::Cancelled);
        assert_eq!(JobError::Shutdown.state(), JobState::Cancelled);
        assert_eq!(JobError::Superseded.state(), JobState::Superseded);
        assert_eq!(JobError::Panicked("x".into()).state(), JobState::Panicked);
        assert_eq!(JobError::failed("壊れたファイル").state(), JobState::Failed);
        assert_eq!(
            JobState::from_result::<()>(&Err(JobError::Memory(MemoryError::TimedOut {
                requested: 1
            }))),
            JobState::Failed
        );
        assert_eq!(JobState::from_result(&Ok(1)), JobState::Succeeded);
        assert!(JobError::Superseded.is_cancellation());
        assert!(!JobError::failed("x").is_cancellation());
    }

    #[test]
    fn conversions_into_job_error() {
        assert!(matches!(JobError::from(Cancelled), JobError::Cancelled));
        assert!(matches!(
            JobError::from(MemoryError::Cancelled),
            JobError::Cancelled
        ));
        assert!(matches!(
            JobError::from(MemoryError::ExceedsBudget {
                requested: 2,
                total: 1
            }),
            JobError::Memory(MemoryError::ExceedsBudget { .. })
        ));
        // ジョブ自身のエラーのメッセージはそのまま表示する。
        assert_eq!(
            JobError::failed("壊れたファイル").to_string(),
            "壊れたファイル"
        );
    }

    #[test]
    fn panic_messages_are_extracted() {
        assert_eq!(panic_message(&"static"), "static");
        assert_eq!(panic_message(&String::from("owned")), "owned");
        assert_eq!(panic_message(&42_i32), "（文字列ではないパニック）");
    }

    #[test]
    fn progress_is_clamped_and_quantized() {
        let shared = JobShared::new(
            JobId::from_raw(1),
            &JobOptions::new(Priority::P3).label("書き出し"),
            CancellationToken::new(),
        );
        let hub = EventHub::default();
        let rx = hub.subscribe();
        shared.report_progress(0.5, &hub);
        shared.report_progress(0.5001, &hub); // 同じ段階（0.1% 未満の変化）は送らない。
        shared.report_progress(f32::NAN, &hub); // NaN は無視する。
        shared.report_progress(2.0, &hub); // 1 に収める。
        shared.report_progress(-1.0, &hub); // 0 に収める。
        let kinds: Vec<_> = rx.try_iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                JobEventKind::Progress(0.5),
                JobEventKind::Progress(1.0),
                JobEventKind::Progress(0.0),
            ]
        );
        assert_eq!(shared.progress(), 0.0);
        shared.set_state(JobState::Succeeded);
        assert_eq!(shared.progress(), 1.0);
    }

    /// drop されたときに数を数える値。
    struct DropCounter(Arc<std::sync::atomic::AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn deliver_never_drops_the_result_itself() {
        // 回帰テスト（レビューで発見）: 送った直後にハンドル（受け取り側）が drop されると、
        // 通路は送り側の drop で破棄され、中の結果もそこで drop される。`deliver` はスケジューラの
        // ロックの中で呼ばれるので、結果が持つ枠（Reservation）の返却がロックを取り直して
        // デッドロックしていた。送り側は必ず呼び出し元へ返し、ロックの外で drop させる。
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (tx, rx) = crossbeam_channel::bounded(1);
        let completion: Box<dyn Completion> = Box::new(TypedCompletion {
            result: Ok(DropCounter(Arc::clone(&dropped))),
            tx,
        });
        assert_eq!(completion.state(), JobState::Succeeded);
        let deferred = completion.deliver();
        // 送った後で受け取り側が消えても、結果はまだ drop されない（送り側が生きているため）。
        drop(rx);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        // 返された値を（ロックの外で）drop した時点で、結果も drop される。
        drop(deferred);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);

        // 受け取り側が先に消えていた場合も、結果は返された値と一緒に drop される。
        let (tx, rx) = crossbeam_channel::bounded(1);
        drop(rx);
        let completion: Box<dyn Completion> = Box::new(TypedCompletion {
            result: Ok(DropCounter(Arc::clone(&dropped))),
            tx,
        });
        let deferred = completion.deliver();
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        drop(deferred);
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
    }
}
