//! 優先度付きのジョブスケジューラ。
//!
//! ジョブの優先度（docs/04_architecture.md の 6.1 節）、取り消し、古い要求の破棄
//! （6.2 節）、メモリの予算による並列度の制限を担当する。
//!
//! - 優先度 P0〜P3（6.1 節の表）: [`Priority`]。高い優先度を先に、同じ優先度は投入順（FIFO）。
//! - スケジューラ（1.3 節の「ジョブスケジューラ」、01 の SYS-02）: [`Scheduler`]。同期の
//!   スレッドプールで、`threads = 1` にすると GPU スレッド用の単一スレッドの優先度付きキューに
//!   なる（6.1 節「GPU への投入は本体の GPU スレッドだけが行う」）。
//! - 取り消しトークン（6.1 節「すべてのジョブに取り消しトークンを持たせる」）:
//!   [`CancellationToken`]。子トークンは親の取り消しに従う。
//! - 「最新の 1 件だけ」（6.1 節の P0、6.2 節のスライダー操作）: [`Scheduler::submit_latest`] と
//!   世代番号 [`Generation`]。
//! - メモリの予算（6.1 節、02 の SCL-05）: [`MemoryBudget`] と [`Reservation`]。
//!   並列度はスレッド数（SCL-07: [`recommended_worker_threads`]）と予算の両方で制限する。
//! - 進捗と状態の変化のイベント（P3 の進捗表示とキャンセル）: [`Scheduler::subscribe`] と
//!   [`JobEvent`]。
//!
//! # 例
//!
//! ```
//! use genzo_jobs::{JobError, JobOptions, Priority, Scheduler, SchedulerConfig};
//!
//! let scheduler = Scheduler::new(SchedulerConfig::new(2, 1_000_000)).unwrap();
//! let handle = scheduler.submit_with(
//!     JobOptions::new(Priority::P3).memory_bytes(4_096).label("プレビューの生成"),
//!     |ctx| {
//!         for tile in 0..4 {
//!             ctx.check()?; // 処理の区切りで取り消しを確認する（6.1 節）
//!             ctx.report_progress((tile + 1) as f32 / 4.0);
//!         }
//!         Ok::<_, JobError>("done")
//!     },
//! );
//! assert_eq!(handle.wait().unwrap(), "done");
//! ```
//!
//! # 設計からの補足
//!
//! - 04 の 1.3 節ではコマンド処理に tokio を使うが、この crate は tokio に依存しない（同期の
//!   スレッドプール）。コア API から使うときは、[`JobHandle::wait`] を非同期の処理の中で
//!   直接呼ばず、ブロックしてよいスレッドで待つか、[`JobHandle::try_result`] とイベントで
//!   完了を知る。
//! - 1.3 節の CPU ワーカープール（rayon）とは別のスレッドで動く。ジョブの中で rayon を使って
//!   よい（[`JobContext`] は複数のスレッドから使える）。

mod cancel;
mod events;
mod job;
mod memory;
mod priority;
mod scheduler;

pub use cancel::{CancelRegistration, CancellationToken, Cancelled};
pub use events::{JobEvent, JobEventKind, PROGRESS_EVENT_STEPS};
pub use job::{
    Generation, JobContext, JobError, JobHandle, JobId, JobOptions, JobState, SlotKey,
    SupersedePolicy,
};
pub use memory::{
    DEFAULT_BATCH_BUDGET_BYTES, DEFAULT_NORMAL_BUDGET_BYTES, MemoryBudget, MemoryError,
    NON_BUFFER_OVERHEAD_BYTES, Reservation, SCL05_BATCH_LIMIT_BYTES, SCL05_NORMAL_LIMIT_BYTES,
};
pub use priority::{
    Priority, UI_RESERVED_CORES, recommended_worker_threads, recommended_worker_threads_for,
};
pub use scheduler::{Scheduler, SchedulerConfig, SchedulerError};
