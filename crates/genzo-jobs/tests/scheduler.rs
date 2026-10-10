//! スケジューラの結合テスト（公開 API だけを使う）。
//!
//! 時間に依存しないように、ジョブとテストの同期はチャネルとバリアで行う。「〜になるまで待つ」は
//! 条件を繰り返し確認する（上限の時間は、失敗したときに止まらないためだけのもの）。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use parking_lot::Mutex;

use genzo_jobs::{
    CancellationToken, JobContext, JobError, JobEvent, JobEventKind, JobHandle, JobOptions,
    JobState, MemoryBudget, MemoryError, Priority, Scheduler, SchedulerConfig, SupersedePolicy,
};

/// 条件が成り立つまで待つ（結果は時間で決めない）。
fn wait_until(mut cond: impl FnMut() -> bool) {
    let limit = Instant::now() + Duration::from_secs(30);
    while !cond() {
        assert!(Instant::now() < limit, "条件が成り立たないまま上限を超えた");
        std::thread::yield_now();
    }
}

fn scheduler(threads: usize, budget: u64) -> Scheduler {
    Scheduler::new(SchedulerConfig::new(threads, budget)).unwrap()
}

/// スレッドを 1 本ふさぐジョブ。実行を始めるまで待って返す。`release` を drop すると終わる。
struct Blocker {
    handle: JobHandle<()>,
    release: Sender<()>,
}

impl Blocker {
    fn start(scheduler: &Scheduler, options: JobOptions) -> Blocker {
        let (started_tx, started_rx) = bounded(1);
        let (release, release_rx) = bounded::<()>(0);
        let handle = scheduler.submit_with(options, move |_| {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv();
            Ok(())
        });
        started_rx.recv().unwrap();
        Blocker { handle, release }
    }

    fn finish(self) {
        drop(self.release);
        self.handle.wait().unwrap();
    }
}

/// 実行の順序を記録するジョブを投入する。
fn record(
    scheduler: &Scheduler,
    priority: Priority,
    name: &'static str,
    log: &Arc<Mutex<Vec<&'static str>>>,
) -> JobHandle<()> {
    let log = Arc::clone(log);
    scheduler.submit(priority, move |_| {
        log.lock().push(name);
        Ok(())
    })
}

/// 取り消されるまで回り続けるジョブ（開始を `started` で知らせる）。
fn spin_until_cancelled(started: Sender<()>) -> impl FnOnce(&JobContext) -> Result<(), JobError> {
    move |ctx| {
        started.send(()).unwrap();
        loop {
            ctx.check()?;
            std::thread::yield_now();
        }
    }
}

// ---- 優先度と FIFO ----

#[test]
fn later_p0_runs_before_earlier_p3_on_single_thread() {
    let s = scheduler(1, 0);
    let log = Arc::new(Mutex::new(Vec::new()));
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P3));
    let handles = vec![
        record(&s, Priority::P3, "p3-a", &log),
        record(&s, Priority::P3, "p3-b", &log),
        record(&s, Priority::P2, "p2", &log),
        record(&s, Priority::P1, "p1", &log),
        record(&s, Priority::P0, "p0", &log),
    ];
    assert_eq!(s.queued_count(), 5);
    blocker.finish();
    for h in handles {
        h.wait().unwrap();
    }
    assert_eq!(*log.lock(), ["p0", "p1", "p2", "p3-a", "p3-b"]);
}

#[test]
fn same_priority_runs_in_submission_order() {
    let s = scheduler(1, 0);
    let log = Arc::new(Mutex::new(Vec::new()));
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    const NAMES: [&str; 8] = ["0", "1", "2", "3", "4", "5", "6", "7"];
    let handles: Vec<_> = NAMES
        .iter()
        .map(|n| record(&s, Priority::P2, n, &log))
        .collect();
    blocker.finish();
    for h in handles {
        h.wait().unwrap();
    }
    assert_eq!(*log.lock(), NAMES);
}

#[test]
fn interleaved_priorities_are_stable() {
    let s = scheduler(1, 0);
    let log = Arc::new(Mutex::new(Vec::new()));
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    let jobs = [
        (Priority::P3, "p3-1"),
        (Priority::P1, "p1-1"),
        (Priority::P3, "p3-2"),
        (Priority::P0, "p0-1"),
        (Priority::P1, "p1-2"),
        (Priority::P0, "p0-2"),
    ];
    let handles: Vec<_> = jobs.iter().map(|(p, n)| record(&s, *p, n, &log)).collect();
    blocker.finish();
    for h in handles {
        h.wait().unwrap();
    }
    assert_eq!(
        *log.lock(),
        ["p0-1", "p0-2", "p1-1", "p1-2", "p3-1", "p3-2"]
    );
}

#[test]
fn set_priority_moves_queued_job() {
    let s = scheduler(1, 0);
    let log = Arc::new(Mutex::new(Vec::new()));
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    let a = record(&s, Priority::P2, "prefetch-a", &log);
    let b = record(&s, Priority::P2, "prefetch-b", &log);
    let c = record(&s, Priority::P1, "visible", &log);
    // 先読み b が表示範囲に入った（P2 → P1）。P1 の中では投入順なので c より前。
    assert!(b.set_priority(Priority::P1));
    assert_eq!(b.priority(), Priority::P1);
    assert!(a.set_priority(Priority::P2)); // 同じ優先度への変更も成功（位置は変わらない）。
    blocker.finish();
    for h in [a, b, c] {
        h.wait().unwrap();
    }
    assert_eq!(*log.lock(), ["prefetch-b", "visible", "prefetch-a"]);
}

#[test]
fn set_priority_fails_for_running_or_finished_jobs() {
    let s = scheduler(1, 0);
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P3));
    assert!(!blocker.handle.set_priority(Priority::P0));
    blocker.finish();
    let h = s.submit(Priority::P3, |_| Ok(()));
    s.wait_idle();
    assert!(!h.set_priority(Priority::P0));
    assert_eq!(h.priority(), Priority::P3);
}

#[test]
fn single_thread_scheduler_runs_everything_on_one_thread() {
    // GPU スレッド用（6.1 節）: すべてのジョブが同じ 1 本のスレッドで順に実行される。
    let s = Scheduler::new(SchedulerConfig::single_thread(0).thread_name("gpu")).unwrap();
    let running = Arc::new(AtomicUsize::new(0));
    let handles: Vec<JobHandle<ThreadId>> = (0..50)
        .map(|i| {
            let running = Arc::clone(&running);
            s.submit(Priority::ALL[i % 4], move |_| {
                assert_eq!(
                    running.fetch_add(1, Ordering::SeqCst),
                    0,
                    "同時に 2 件動いた"
                );
                std::thread::yield_now();
                running.fetch_sub(1, Ordering::SeqCst);
                Ok(std::thread::current().id())
            })
        })
        .collect();
    let ids: Vec<_> = handles.into_iter().map(|h| h.wait().unwrap()).collect();
    assert!(ids.iter().all(|id| *id == ids[0]));
}

// ---- 取り消し ----

#[test]
fn cancelled_queued_job_is_never_run() {
    let s = scheduler(1, 0);
    let events = s.subscribe();
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    let ran = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&ran);
    let h = s.submit(Priority::P1, move |_| {
        r.store(true, Ordering::SeqCst);
        Ok(())
    });
    assert_eq!(h.state(), JobState::Queued);
    h.cancel();
    // キューから外されるのは取り消しの時点（スレッドが空くのを待たない）。
    assert_eq!(h.state(), JobState::Cancelled);
    assert!(h.is_finished());
    assert_eq!(s.queued_count(), 0);
    let id = h.id();
    assert!(matches!(h.wait(), Err(JobError::Cancelled)));
    blocker.finish();
    s.wait_idle();
    assert!(!ran.load(Ordering::SeqCst));
    let states: Vec<_> = events
        .try_iter()
        .filter(|e| e.id == id)
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        states,
        [
            JobEventKind::State(JobState::Queued),
            JobEventKind::State(JobState::Cancelled)
        ]
    );
}

#[test]
fn cancelling_parent_cancels_group_of_queued_jobs() {
    // グリッドの表示範囲から外れたサムネイル（P1）をまとめて取り消す（6.1 節）。
    let s = scheduler(1, 0);
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    let visible = CancellationToken::new();
    let ran = Arc::new(AtomicUsize::new(0));
    let group: Vec<_> = (0..10)
        .map(|_| {
            let ran = Arc::clone(&ran);
            s.submit_with(JobOptions::new(Priority::P1).parent(&visible), move |_| {
                ran.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        })
        .collect();
    let other = s.submit(Priority::P2, |_| Ok(7));
    visible.cancel();
    assert_eq!(s.queued_count(), 1);
    blocker.finish();
    for h in group {
        assert_eq!(h.state(), JobState::Cancelled);
        assert!(matches!(h.wait(), Err(JobError::Cancelled)));
    }
    assert_eq!(other.wait().unwrap(), 7);
    assert_eq!(ran.load(Ordering::SeqCst), 0);
}

#[test]
fn job_with_cancelled_parent_is_not_queued() {
    let s = scheduler(1, 0);
    let parent = CancellationToken::new();
    parent.cancel();
    let ran = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&ran);
    let h = s.submit_with(JobOptions::new(Priority::P0).parent(&parent), move |_| {
        r.store(true, Ordering::SeqCst);
        Ok(())
    });
    assert_eq!(h.state(), JobState::Cancelled);
    assert!(matches!(h.wait(), Err(JobError::Cancelled)));
    s.wait_idle();
    assert!(!ran.load(Ordering::SeqCst));
}

#[test]
fn running_job_stops_when_cancelled() {
    let s = scheduler(2, 0);
    let (started_tx, started_rx) = bounded(1);
    let h = s.submit(Priority::P3, spin_until_cancelled(started_tx));
    started_rx.recv().unwrap();
    assert_eq!(h.state(), JobState::Running);
    h.cancel();
    assert!(matches!(h.wait(), Err(JobError::Cancelled)));
    // スケジューラは動き続ける。
    assert_eq!(s.submit(Priority::P0, |_| Ok(1)).wait().unwrap(), 1);
}

#[test]
fn cancel_after_finish_has_no_effect() {
    let s = scheduler(1, 0);
    let mut h = s.submit(Priority::P1, |_| Ok("ok"));
    s.wait_idle();
    h.cancel();
    assert_eq!(h.state(), JobState::Succeeded);
    assert_eq!(h.try_result().unwrap().unwrap(), "ok");
}

#[test]
fn dropping_handle_does_not_cancel() {
    let s = scheduler(1, 0);
    let (tx, rx) = bounded(1);
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    drop(s.submit(Priority::P3, move |_| {
        tx.send(42).unwrap();
        Ok(())
    }));
    blocker.finish();
    assert_eq!(rx.recv().unwrap(), 42);
}

// ---- 最新の 1 件だけ（6.1 節・6.2 節） ----

#[test]
fn submit_latest_replaces_job_that_has_not_started() {
    let s = scheduler(1, 0);
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    let log = Arc::new(Mutex::new(Vec::new()));
    let submit = |value: i32| {
        let log = Arc::clone(&log);
        s.submit_latest("develop-preview", Priority::P0, move |ctx| {
            log.lock().push(value);
            Ok((value, ctx.generation().unwrap()))
        })
    };
    let first = submit(1);
    let second = submit(2);
    let g1 = first.generation().unwrap();
    let g2 = second.generation().unwrap();
    assert!(g2 > g1);
    // 古い要求は実行されずに「置き換え」で終わる。トークンも取り消される。
    assert_eq!(first.state(), JobState::Superseded);
    assert!(first.token().is_cancelled());
    assert!(matches!(first.wait(), Err(JobError::Superseded)));
    assert_eq!(s.queued_count(), 1);
    blocker.finish();
    assert_eq!(second.wait().unwrap(), (2, g2));
    assert_eq!(*log.lock(), [2]);
}

#[test]
fn rapid_latest_requests_keep_only_the_last() {
    let s = scheduler(1, 0);
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    let ran = Arc::new(Mutex::new(Vec::new()));
    let handles: Vec<_> = (0..100)
        .map(|i| {
            let ran = Arc::clone(&ran);
            s.submit_latest("slider", Priority::P0, move |_| {
                ran.lock().push(i);
                Ok(i)
            })
        })
        .collect();
    let generations: Vec<_> = handles.iter().map(|h| h.generation().unwrap()).collect();
    assert!(generations.windows(2).all(|w| w[0] < w[1]));
    blocker.finish();
    let mut superseded = 0;
    for (i, h) in handles.into_iter().enumerate() {
        match h.wait() {
            Ok(v) => assert_eq!((i, v), (99, 99)),
            Err(JobError::Superseded) => superseded += 1,
            Err(e) => panic!("想定外のエラー: {e}"),
        }
    }
    assert_eq!(superseded, 99);
    assert_eq!(*ran.lock(), [99]);
}

#[test]
fn concurrent_latest_requests_keep_the_newest_generation() {
    // 回帰テスト（レビューで発見）: 世代番号の割り当てとキューへの投入が別々に行われていたため、
    // 複数のスレッドから同じスロットへ同時に投入すると、新しい世代の要求が古い世代の要求で
    // 置き換えられ、古い設定のプレビューが最後に残ることがあった（6.2 節「最新の 1 件だけ」）。
    const THREADS: usize = 4;
    const PER_THREAD: usize = 300;
    // 競合は確率的にしか起きないので、何回か繰り返す（修正後は常に成り立つ）。
    for _ in 0..40 {
        let s = scheduler(1, 0);
        let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
        let barrier = Barrier::new(THREADS);
        // 共通の親トークン（例: 現像セッション）を持たせる。子トークンの作成で親のロックを
        // 取り合うので、競合が起きやすくなる。
        let session = CancellationToken::new();
        let handles: Vec<JobHandle<u64>> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..THREADS)
                .map(|_| {
                    let (s, barrier, session) = (&s, &barrier, &session);
                    scope.spawn(move || {
                        barrier.wait();
                        (0..PER_THREAD)
                            .map(|_| {
                                s.submit_latest_with(
                                    "develop-preview",
                                    JobOptions::new(Priority::P0).parent(session),
                                    |ctx| Ok(ctx.generation().unwrap().get()),
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|w| w.join().unwrap())
                .collect()
        });
        // スロットに残るのは 1 件だけで、それは最も新しい世代の要求。
        assert_eq!(s.queued_count(), 1);
        let newest = handles
            .iter()
            .map(|h| h.generation().unwrap().get())
            .max()
            .unwrap();
        blocker.finish();
        let mut ran = Vec::new();
        for h in handles {
            match h.wait() {
                Ok(generation) => ran.push(generation),
                Err(JobError::Superseded) => {}
                Err(e) => panic!("想定外のエラー: {e}"),
            }
        }
        assert_eq!(ran, [newest]);
    }
}

#[test]
fn submit_latest_cancels_running_job() {
    let s = scheduler(2, 0);
    let (started_tx, started_rx) = bounded(1);
    let old = s.submit_latest(
        "develop-preview",
        Priority::P0,
        spin_until_cancelled(started_tx),
    );
    started_rx.recv().unwrap();
    let new = s.submit_latest("develop-preview", Priority::P0, |ctx| {
        Ok(ctx.is_superseded())
    });
    // 実行中の古いジョブは取り消しを受けてやめ、「置き換え」として報告される。
    assert!(matches!(old.wait(), Err(JobError::Superseded)));
    assert!(!new.wait().unwrap());
}

#[test]
fn keep_running_policy_lets_running_job_finish() {
    let s = scheduler(2, 0);
    let (started_tx, started_rx) = bounded(1);
    let (continue_tx, continue_rx) = bounded::<()>(1);
    let old = s.submit_latest_with(
        "develop-preview",
        JobOptions::new(Priority::P0).supersede_policy(SupersedePolicy::KeepRunning),
        move |ctx| {
            started_tx.send(()).unwrap();
            continue_rx.recv().unwrap();
            // 置き換えられたことは分かるが、取り消されてはいない。
            Ok((ctx.is_superseded(), ctx.is_cancelled()))
        },
    );
    started_rx.recv().unwrap();
    let new = s.submit_latest_with(
        "develop-preview",
        JobOptions::new(Priority::P0).supersede_policy(SupersedePolicy::KeepRunning),
        |_| Ok((false, false)),
    );
    continue_tx.send(()).unwrap();
    assert_eq!(old.wait().unwrap(), (true, false));
    assert_eq!(new.wait().unwrap(), (false, false));
}

#[test]
fn latest_slots_are_independent() {
    let s = scheduler(1, 0);
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    let a = s.submit_latest("loupe", Priority::P0, |_| Ok("loupe"));
    let b = s.submit_latest("develop-preview", Priority::P0, |_| Ok("develop"));
    let c = s.submit(Priority::P0, |_| Ok("plain")); // スロットなしのジョブは置き換えない。
    assert!(a.generation() < b.generation());
    assert_eq!(c.generation(), None);
    blocker.finish();
    assert_eq!(a.wait().unwrap(), "loupe");
    assert_eq!(b.wait().unwrap(), "develop");
    assert_eq!(c.wait().unwrap(), "plain");
}

#[test]
fn rejected_latest_requests_still_get_a_generation() {
    // 実行されずに終わる要求（総量を超える宣言・終了後の投入）にも世代番号を返す。
    let s = scheduler(1, 100);
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    let queued = s.submit_latest("loupe", Priority::P0, |_| Ok(()));
    let too_big = s.submit_latest_with(
        "loupe",
        JobOptions::new(Priority::P0).memory_bytes(101),
        |_| Ok(()),
    );
    assert!(too_big.generation() > queued.generation());
    assert!(matches!(
        too_big.wait(),
        Err(JobError::Memory(MemoryError::ExceedsBudget { .. }))
    ));
    // 満たせない要求は、スロットの古い要求を置き換えない。
    assert_eq!(queued.state(), JobState::Queued);
    blocker.finish();
    queued.wait().unwrap();
    s.shutdown();
    let late = s.submit_latest("loupe", Priority::P0, |_| Ok(()));
    assert!(late.generation().is_some());
    assert!(matches!(late.wait(), Err(JobError::Shutdown)));
}

#[test]
fn latest_job_finishing_normally_is_not_superseded() {
    let s = scheduler(1, 0);
    let h = s.submit_latest("loupe", Priority::P0, |ctx| Ok(ctx.is_superseded()));
    assert!(!h.wait().unwrap());
    // スロットが空になった後の要求は、新しい世代として普通に実行される。
    let h2 = s.submit_latest("loupe", Priority::P0, |_| Ok(()));
    h2.wait().unwrap();
}

// ---- メモリの予算（6.1 節、SCL-05） ----

#[test]
fn declared_memory_limits_parallelism() {
    const TOTAL: u64 = 100;
    const PER_JOB: u64 = 40; // 予算に同時に 2 件まで入る。
    let s = scheduler(4, TOTAL);
    let concurrent = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..24)
        .map(|_| {
            let (concurrent, max_seen) = (Arc::clone(&concurrent), Arc::clone(&max_seen));
            s.submit_with(
                JobOptions::new(Priority::P3).memory_bytes(PER_JOB),
                move |ctx| {
                    assert_eq!(ctx.reserved_bytes(), PER_JOB);
                    let now = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(now, Ordering::SeqCst);
                    for _ in 0..100 {
                        std::thread::yield_now();
                    }
                    concurrent.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                },
            )
        })
        .collect();
    for h in handles {
        h.wait().unwrap();
    }
    let max = max_seen.load(Ordering::SeqCst);
    assert!((1..=2).contains(&max), "同時実行数 {max}");
    assert_eq!(s.budget().used(), 0);
}

#[test]
fn job_waits_for_memory_and_head_is_not_overtaken() {
    let s = scheduler(2, 100);
    let log = Arc::new(Mutex::new(Vec::new()));
    // A が 60 を持ったまま 1 本目のスレッドをふさぐ。
    let a = Blocker::start(&s, JobOptions::new(Priority::P3).memory_bytes(60));
    assert_eq!(s.budget().used(), 60);
    let push = |name: &'static str| {
        let log = Arc::clone(&log);
        move |_: &JobContext| {
            log.lock().push(name);
            Ok(())
        }
    };
    // B（P1, 60）は入らない。C（P3, 10）は入るが、先頭の B を追い越さない。
    let b = s.submit_with(JobOptions::new(Priority::P1).memory_bytes(60), push("b"));
    let c = s.submit_with(JobOptions::new(Priority::P3).memory_bytes(10), push("c"));
    // P0 のメモリなしのジョブ D は先頭に来るので実行される。D が終わった後、空いているスレッドは
    // 先頭の B を確認して待つ。
    let d = s.submit(Priority::P0, push("d"));
    d.wait().unwrap();
    assert_eq!(b.state(), JobState::Queued);
    assert_eq!(c.state(), JobState::Queued);
    assert_eq!(*log.lock(), ["d"]);
    a.finish();
    b.wait().unwrap();
    c.wait().unwrap();
    // B と C は予算に一緒に入る（60 + 10）ので、この後の順序は決めない。
    let mut done = log.lock().clone();
    done.sort_unstable();
    assert_eq!(done, ["b", "c", "d"]);
    assert_eq!(s.budget().used(), 0);
}

#[test]
fn cancelling_memory_blocked_head_unblocks_the_rest() {
    let s = scheduler(2, 100);
    let a = Blocker::start(&s, JobOptions::new(Priority::P3).memory_bytes(60));
    let b = s.submit_with(JobOptions::new(Priority::P1).memory_bytes(60), |_| Ok("b"));
    let c = s.submit_with(JobOptions::new(Priority::P3).memory_bytes(10), |_| Ok("c"));
    b.cancel();
    // 先頭の B がなくなったので、C は A の実行中に始められる。
    assert_eq!(c.wait().unwrap(), "c");
    assert!(matches!(b.wait(), Err(JobError::Cancelled)));
    a.finish();
}

#[test]
fn declared_memory_over_total_fails_immediately() {
    let s = scheduler(1, 100);
    let ran = Arc::new(AtomicBool::new(false));
    let r = Arc::clone(&ran);
    let h = s.submit_with(JobOptions::new(Priority::P0).memory_bytes(101), move |_| {
        r.store(true, Ordering::SeqCst);
        Ok(())
    });
    assert_eq!(h.state(), JobState::Failed);
    match h.wait() {
        Err(JobError::Memory(MemoryError::ExceedsBudget { requested, total })) => {
            assert_eq!((requested, total), (101, 100));
        }
        other => panic!("想定外の結果: {other:?}"),
    }
    // ちょうど総量なら実行できる。
    s.submit_with(JobOptions::new(Priority::P0).memory_bytes(100), |_| Ok(()))
        .wait()
        .unwrap();
    assert!(!ran.load(Ordering::SeqCst));
}

#[test]
fn taken_reservation_lives_with_the_result() {
    let s = scheduler(1, 100);
    let h = s.submit_with(JobOptions::new(Priority::P2).memory_bytes(70), |ctx| {
        let r = ctx.take_reservation().unwrap();
        assert_eq!(ctx.reserved_bytes(), 0);
        assert!(ctx.take_reservation().is_none());
        Ok(r)
    });
    let reservation = h.wait().unwrap();
    assert_eq!(reservation.bytes(), 70);
    assert_eq!(s.budget().used(), 70);
    drop(reservation);
    assert_eq!(s.budget().used(), 0);
}

#[test]
fn dropping_handle_while_result_holds_reservation_does_not_deadlock() {
    // 回帰テスト（レビューで発見）: 枠を持った結果を返すジョブの終了と同時にハンドルを drop
    // すると、結果（枠）がスケジューラのロックの中で drop され、枠の返却の通知がロックを
    // 取り直して実行スレッドが止まっていた（修正前は数回〜数十回の繰り返しで再現した）。
    // 止まった場合にテストが戻らなくならないよう、別のスレッドで実行して上限を設ける
    // （上限は失敗の検出だけに使い、結果は時間で決めない）。
    const ROUNDS: usize = 2_000;
    let (done_tx, done_rx) = bounded::<usize>(1);
    std::thread::spawn(move || {
        let s = scheduler(2, 1_000);
        for _ in 0..ROUNDS {
            let returning = Arc::new(AtomicBool::new(false));
            let r = Arc::clone(&returning);
            let h = s.submit_with(JobOptions::new(Priority::P1).memory_bytes(10), move |ctx| {
                let reservation = ctx.take_reservation();
                r.store(true, Ordering::Release);
                Ok(reservation)
            });
            // ジョブが結果を返す直前まで待ってから、ハンドルを drop する。
            while !returning.load(Ordering::Acquire) {
                std::hint::spin_loop();
            }
            drop(h);
        }
        s.wait_idle();
        let _ = done_tx.send(s.budget().used() as usize);
    });
    let used = done_rx
        .recv_timeout(Duration::from_secs(120))
        .expect("実行スレッドが止まった（結果の drop でデッドロック）");
    assert_eq!(used, 0);
}

#[test]
fn reserve_inside_job_waits_and_can_be_cancelled() {
    let s = scheduler(2, 100);
    let a = Blocker::start(&s, JobOptions::new(Priority::P3).memory_bytes(100));
    let (started_tx, started_rx) = bounded(1);
    let h = s.submit(Priority::P1, move |ctx| {
        started_tx.send(()).unwrap();
        let r = ctx.reserve(50)?; // 予算が空くまで待つ。取り消されたら Cancelled。
        Ok(r.bytes())
    });
    started_rx.recv().unwrap();
    wait_until(|| s.budget().waiting() == 1);
    h.cancel();
    assert!(matches!(h.wait(), Err(JobError::Cancelled)));
    assert_eq!(s.budget().waiting(), 0);

    // 取り消さなければ、枠が空いたときに取れる。
    let (started_tx, started_rx) = bounded(1);
    let h = s.submit(Priority::P1, move |ctx| {
        started_tx.send(()).unwrap();
        let r = ctx.reserve(50)?;
        Ok(r.bytes())
    });
    started_rx.recv().unwrap();
    wait_until(|| s.budget().waiting() == 1);
    a.finish();
    assert_eq!(h.wait().unwrap(), 50);
    assert_eq!(s.budget().used(), 0);
}

#[test]
fn reserve_that_can_never_fit_beside_declared_memory_fails_immediately() {
    // 回帰テスト（レビューで発見）: 投入時に宣言した枠（60）を持ったまま、総量（100）から
    // それを引いた残りより大きい枠（50）を待つと、自分の枠が返らない限り満たせないので
    // 永久に待っていた（同じ優先度以下の新しいジョブも、この待ちに止められていた）。
    let s = scheduler(1, 100);
    let mut h = s.submit_with(JobOptions::new(Priority::P3).memory_bytes(60), |ctx| {
        let err = ctx.reserve(50).unwrap_err();
        // ちょうど残り（40）なら取れる。
        let fits = ctx.reserve(40)?;
        Ok((err, fits.bytes()))
    });
    let (err, fits) = h
        .wait_timeout(Duration::from_secs(30))
        .expect("満たせない枠を待ち続けている")
        .unwrap();
    assert_eq!(
        err,
        MemoryError::ExceedsBudget {
            requested: 110,
            total: 100
        }
    );
    assert_eq!(fits, 40);
    assert_eq!(s.budget().used(), 0);
    assert_eq!(s.budget().waiting(), 0);
}

#[test]
fn try_reserve_and_reserve_over_total() {
    let s = scheduler(1, 100);
    let h = s.submit(Priority::P0, |ctx| {
        let held = ctx.try_reserve(80)?;
        assert!(matches!(
            ctx.try_reserve(30),
            Err(MemoryError::Unavailable { .. })
        ));
        assert!(matches!(
            ctx.reserve(101),
            Err(MemoryError::ExceedsBudget { .. })
        ));
        drop(held);
        Ok(ctx.budget().used())
    });
    assert_eq!(h.wait().unwrap(), 0);
}

#[test]
fn schedulers_can_share_one_budget() {
    // 本体の CPU のプールと GPU スレッドで 1 つの予算を分け合う（6.1 節）。
    let budget = MemoryBudget::new(100);
    let cpu = Scheduler::with_budget(2, "cpu", budget.clone()).unwrap();
    let gpu = Scheduler::with_budget(1, "gpu", budget.clone()).unwrap();
    assert!(cpu.budget().same_as(gpu.budget()));
    let blocker = Blocker::start(&cpu, JobOptions::new(Priority::P3).memory_bytes(100));
    let g = gpu.submit_with(JobOptions::new(Priority::P1).memory_bytes(50), |_| {
        Ok("tile")
    });
    // P0 の別のジョブ（メモリなし）は g より前に並ぶので、GPU スレッドで先に動ける。
    // CPU 側が予算を持っている間、g は始まらない。
    gpu.submit(Priority::P0, |_| Ok(())).wait().unwrap();
    assert_eq!(g.state(), JobState::Queued);
    blocker.finish();
    assert_eq!(g.wait().unwrap(), "tile");
    assert_eq!(budget.used(), 0);
}

// ---- パニック・エラー ----

#[test]
fn panicking_job_is_reported_and_pool_survives() {
    let s = scheduler(1, 100);
    let events = s.subscribe();
    let h: JobHandle<()> = s.submit_with(JobOptions::new(Priority::P1).memory_bytes(50), |_| {
        panic!("壊れたファイル")
    });
    let id = h.id();
    match h.wait() {
        Err(JobError::Panicked(message)) => assert_eq!(message, "壊れたファイル"),
        other => panic!("想定外の結果: {other:?}"),
    }
    // パニックしても宣言したメモリは返却される。
    assert_eq!(s.budget().used(), 0);
    let h2: JobHandle<()> = s.submit(Priority::P1, |_| panic!("{}番目", 2));
    match h2.wait() {
        Err(JobError::Panicked(message)) => assert_eq!(message, "2番目"),
        other => panic!("想定外の結果: {other:?}"),
    }
    // 同じ 1 本のスレッドで続けて実行できる。
    assert_eq!(s.submit(Priority::P1, |_| Ok(5)).wait().unwrap(), 5);
    let last = events.try_iter().filter(|e| e.id == id).last().unwrap();
    assert_eq!(last.kind, JobEventKind::State(JobState::Panicked));
}

#[test]
fn panicking_drop_of_undelivered_result_does_not_kill_the_thread() {
    // 回帰テスト（レビューで発見）: ハンドルが drop されていて届かなかった結果は実行スレッドで
    // drop される。その drop がパニックすると、1 本だけのスレッド（GPU スレッド。6.1 節）が
    // 終わり、以後のジョブが永久に実行されなかった。
    struct PanicOnDrop;
    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("結果の drop でのパニック（テスト用）");
        }
    }

    let s = scheduler(1, 0);
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    drop(s.submit(Priority::P1, |_| Ok(PanicOnDrop)));
    blocker.finish();
    let mut next = s.submit(Priority::P1, |_| Ok("next"));
    let result = next
        .wait_timeout(Duration::from_secs(30))
        .expect("実行スレッドが失われた");
    assert_eq!(result.unwrap(), "next");
}

#[test]
fn job_errors_are_returned() {
    #[derive(Debug, thiserror::Error)]
    #[error("対応していない形式です")]
    struct Unsupported;

    let s = scheduler(1, 0);
    let h: JobHandle<()> = s.submit(Priority::P2, |_| Err(JobError::failed(Unsupported)));
    let err = h.wait().unwrap_err();
    assert_eq!(err.state(), JobState::Failed);
    assert_eq!(err.to_string(), "対応していない形式です");
    let h: JobHandle<()> = s.submit(Priority::P2, |_| Err(JobError::failed("理由")));
    s.wait_idle();
    assert_eq!(h.state(), JobState::Failed);
}

// ---- 結果の取り出し ----

#[test]
fn result_can_be_taken_once() {
    let s = scheduler(1, 0);
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    let mut h = s.submit(Priority::P1, |_| Ok(3));
    assert!(h.try_result().is_none());
    assert!(h.wait_timeout(Duration::from_millis(1)).is_none());
    blocker.finish();
    assert_eq!(h.wait_timeout(Duration::from_secs(30)).unwrap().unwrap(), 3);
    assert!(h.try_result().is_none());
    assert!(h.wait_timeout(Duration::from_millis(1)).is_none());
    assert!(matches!(h.wait(), Err(JobError::ResultTaken)));
}

#[test]
fn finished_state_implies_result_is_available() {
    // 状態が「終わった」と見えた時点で、結果は取り出せる（ポーリングで使えること）。
    let s = scheduler(2, 0);
    for i in 0..200_usize {
        let mut h = s.submit(Priority::ALL[i % 4], move |_| Ok(i));
        wait_until(|| h.is_finished());
        assert_eq!(h.try_result().unwrap().unwrap(), i);
    }
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    let blocker2 = Blocker::start(&s, JobOptions::new(Priority::P0));
    let mut queued = s.submit(Priority::P3, |_| Ok(0));
    queued.cancel();
    assert!(queued.is_finished());
    assert!(matches!(
        queued.try_result(),
        Some(Err(JobError::Cancelled))
    ));
    blocker.finish();
    blocker2.finish();
}

// ---- 進捗とイベント ----

#[test]
fn events_report_states_and_progress_in_order() {
    let s = scheduler(1, 0);
    let events = s.subscribe();
    let h = s.submit_with(JobOptions::new(Priority::P3).label("書き出し"), |ctx| {
        assert_eq!(ctx.label(), Some("書き出し"));
        for i in 1..=4 {
            ctx.report_progress(i as f32 / 4.0);
        }
        Ok(())
    });
    let id = h.id();
    h.wait().unwrap();
    let mine: Vec<JobEvent> = events.try_iter().filter(|e| e.id == id).collect();
    assert!(mine.iter().all(|e| e.label.as_deref() == Some("書き出し")));
    assert!(mine.iter().all(|e| e.priority == Priority::P3));
    let kinds: Vec<_> = mine.into_iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            JobEventKind::State(JobState::Queued),
            JobEventKind::State(JobState::Running),
            JobEventKind::Progress(0.25),
            JobEventKind::Progress(0.5),
            JobEventKind::Progress(0.75),
            JobEventKind::Progress(1.0),
            JobEventKind::State(JobState::Succeeded),
        ]
    );
}

#[test]
fn progress_is_visible_through_handle() {
    let s = scheduler(1, 0);
    let (reported_tx, reported_rx) = bounded(1);
    let (continue_tx, continue_rx) = bounded::<()>(1);
    let h = s.submit(Priority::P3, move |ctx| {
        ctx.report_progress(0.3);
        reported_tx.send(()).unwrap();
        continue_rx.recv().unwrap();
        Ok(())
    });
    reported_rx.recv().unwrap();
    assert_eq!(h.progress(), 0.3);
    continue_tx.send(()).unwrap();
    s.wait_idle();
    assert_eq!(h.progress(), 1.0);
}

#[test]
fn superseded_job_events() {
    let s = scheduler(1, 0);
    let events = s.subscribe();
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P0));
    let old = s.submit_latest("slot", Priority::P0, |_| Ok(()));
    let new = s.submit_latest("slot", Priority::P0, |_| Ok(()));
    blocker.finish();
    new.wait().unwrap();
    let old_kinds: Vec<_> = events
        .try_iter()
        .filter(|e| e.id == old.id())
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        old_kinds,
        [
            JobEventKind::State(JobState::Queued),
            JobEventKind::State(JobState::Superseded)
        ]
    );
}

// ---- 割り込み（PERF-13） ----

#[test]
fn should_yield_when_higher_priority_is_waiting() {
    let s = scheduler(1, 0);
    let (started_tx, started_rx) = bounded(1);
    let (check_tx, check_rx) = bounded::<()>(1);
    let export = s.submit(Priority::P3, move |ctx| {
        let before = ctx.should_yield();
        started_tx.send(()).unwrap();
        check_rx.recv().unwrap();
        Ok((before, ctx.should_yield()))
    });
    started_rx.recv().unwrap();
    // 同じ優先度のジョブでは譲らない。
    let same = s.submit(Priority::P3, |ctx| Ok(ctx.should_yield()));
    let p0 = s.submit(Priority::P0, |ctx| Ok(ctx.should_yield()));
    check_tx.send(()).unwrap();
    assert_eq!(export.wait().unwrap(), (false, true));
    // P0 の実行中、キューには P3 しかないので譲らない。
    assert!(!p0.wait().unwrap());
    assert!(!same.wait().unwrap());
}

// ---- 終了処理 ----

#[test]
fn shutdown_cancels_queue_and_waits_for_running() {
    let s = scheduler(1, 0);
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P3));
    let queued: Vec<_> = (0..5).map(|_| s.submit(Priority::P1, |_| Ok(()))).collect();
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let shutdown = scope.spawn(|| {
            s.shutdown();
            finished.store(true, Ordering::SeqCst);
        });
        // キューのジョブは、実行中のジョブの終了を待たずに取り消される。
        for h in queued {
            assert!(matches!(h.wait(), Err(JobError::Shutdown)));
        }
        assert!(s.is_shut_down());
        assert!(!finished.load(Ordering::SeqCst));
        // 実行中のジョブは取り消されない（shutdown は終了を待つ）。
        assert!(!blocker.handle.token().is_cancelled());
        blocker.finish();
        shutdown.join().unwrap();
    });
    assert!(finished.load(Ordering::SeqCst));
    assert_eq!(s.running_count(), 0);
    // 終了後の投入は実行されない。
    let late = s.submit(Priority::P0, |_| Ok(()));
    assert_eq!(late.state(), JobState::Cancelled);
    assert!(matches!(late.wait(), Err(JobError::Shutdown)));
    // 2 回目の呼び出しもすぐに戻る。
    s.shutdown();
    s.shutdown_now();
}

#[test]
fn shutdown_now_cancels_running_jobs() {
    let s = scheduler(2, 0);
    let (tx, rx) = unbounded();
    let a = s.submit(Priority::P3, spin_until_cancelled(tx.clone()));
    let b = s.submit(Priority::P3, spin_until_cancelled(tx));
    rx.recv().unwrap();
    rx.recv().unwrap();
    s.shutdown_now();
    assert!(matches!(a.wait(), Err(JobError::Cancelled)));
    assert!(matches!(b.wait(), Err(JobError::Cancelled)));
}

#[test]
fn shutdown_now_survives_panicking_cancel_callback() {
    // 取り消しのコールバックがパニックしても、他の実行中のジョブへの通知とスレッドの回収は
    // 最後まで行う（Drop からも呼ばれるため）。
    let s = scheduler(2, 0);
    let (tx, rx) = unbounded();
    let (reg_tx, reg_rx) = bounded(1);
    let a = s.submit(Priority::P3, move |ctx| {
        let registration = ctx
            .token()
            .on_cancel(|| panic!("取り消しのコールバックのパニック（テスト用）"));
        reg_tx.send(()).unwrap();
        let result = spin_until_cancelled(tx)(ctx);
        drop(registration);
        result
    });
    reg_rx.recv().unwrap();
    rx.recv().unwrap();
    let (tx2, rx2) = bounded(1);
    let b = s.submit(Priority::P3, spin_until_cancelled(tx2));
    rx2.recv().unwrap();
    s.shutdown_now();
    assert!(matches!(a.wait(), Err(JobError::Cancelled)));
    assert!(matches!(b.wait(), Err(JobError::Cancelled)));
    assert_eq!(s.running_count(), 0);
}

#[test]
fn drop_cancels_running_jobs_and_joins_threads() {
    let s = scheduler(2, 0);
    let (tx, rx) = bounded(1);
    let done = Arc::new(AtomicBool::new(false));
    let d = Arc::clone(&done);
    let spin = spin_until_cancelled(tx);
    let h = s.submit(Priority::P3, move |ctx| {
        let result = spin(ctx);
        d.store(true, Ordering::SeqCst);
        result
    });
    rx.recv().unwrap();
    drop(s);
    // drop が戻った時点で、実行中だったジョブは終わっている（スレッドを回収済み）。
    assert!(done.load(Ordering::SeqCst));
    assert!(matches!(h.wait(), Err(JobError::Cancelled)));
}

#[test]
fn concurrent_shutdown_calls_both_wait() {
    let s = scheduler(1, 0);
    let blocker = Blocker::start(&s, JobOptions::new(Priority::P3));
    let barrier = Barrier::new(3);
    let returned = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                barrier.wait();
                s.shutdown();
                returned.fetch_add(1, Ordering::SeqCst);
            });
        }
        barrier.wait();
        wait_until(|| s.is_shut_down());
        // どちらの呼び出しも、実行中のジョブが終わるまで戻らない。
        assert_eq!(returned.load(Ordering::SeqCst), 0);
        blocker.finish();
    });
    assert_eq!(returned.load(Ordering::SeqCst), 2);
}

#[test]
fn dropping_last_scheduler_reference_inside_job_does_not_deadlock() {
    let s = Arc::new(scheduler(2, 0));
    let (tx, rx) = bounded(1);
    let inner = Arc::clone(&s);
    let h = s.submit(Priority::P0, move |_| {
        rx.recv().unwrap();
        // ジョブの中でスケジューラの最後の参照を drop する（自分自身のスレッドは待たない）。
        drop(inner);
        Ok(())
    });
    drop(s);
    tx.send(()).unwrap();
    h.wait().unwrap();
}

// ---- 全体 ----

#[test]
fn nested_submission_works() {
    let s = Arc::new(scheduler(2, 0));
    let inner = Arc::clone(&s);
    let h = s.submit(Priority::P3, move |_| {
        let child = inner.submit(Priority::P0, |_| Ok(20));
        Ok(child.wait()? + 1)
    });
    assert_eq!(h.wait().unwrap(), 21);
}

#[test]
fn wait_idle_returns_after_all_jobs() {
    let s = scheduler(3, 0);
    let count = Arc::new(AtomicUsize::new(0));
    for _ in 0..100 {
        let count = Arc::clone(&count);
        drop(s.submit(Priority::P2, move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
    }
    s.wait_idle();
    assert_eq!(count.load(Ordering::SeqCst), 100);
    assert_eq!(s.queued_count(), 0);
    assert_eq!(s.running_count(), 0);
    // 何もないときはすぐに戻る。
    s.wait_idle();
}

#[test]
fn stress_mixed_operations_resolve_every_handle() {
    let s = scheduler(4, 1_000);
    let events = s.subscribe();
    let parent = CancellationToken::new();
    let mut handles: Vec<JobHandle<u64>> = Vec::new();
    for i in 0..600_u64 {
        let priority = Priority::ALL[(i * 7 % 4) as usize];
        // 宣言は 199 以下。4 本が同時に持っても 796 で、追加の 50 を 3 件持っていても 946 なので、
        // 追加の枠（ctx.reserve(50)）の待ちは必ず解消する（枠を持ったまま待ち合わない）。
        let mut options = JobOptions::new(priority).memory_bytes((i * 37) % 200);
        if i % 5 == 0 {
            options = options.parent(&parent);
        }
        let job = move |ctx: &JobContext| {
            for step in 0..4 {
                ctx.check()?;
                ctx.report_progress(step as f32 / 4.0);
                if i % 3 == 0 {
                    let extra = ctx.reserve(50)?;
                    drop(extra);
                }
            }
            if i % 97 == 0 {
                panic!("テスト用のパニック");
            }
            Ok(i)
        };
        let h = if i % 4 == 0 {
            s.submit_latest_with(format!("slot-{}", i % 8), options, job)
        } else {
            s.submit_with(options, job)
        };
        if i % 11 == 0 {
            h.cancel();
        }
        if i == 300 {
            parent.cancel();
        }
        handles.push(h);
    }
    let mut ok = 0;
    for (i, h) in handles.into_iter().enumerate() {
        match h.wait() {
            Ok(v) => {
                assert_eq!(v, i as u64);
                ok += 1;
            }
            Err(e) => assert!(
                e.is_cancellation() || matches!(e, JobError::Panicked(_)),
                "想定外のエラー: {e}"
            ),
        }
    }
    assert!(ok > 0);
    s.wait_idle();
    assert_eq!(s.budget().used(), 0);
    assert_eq!(s.budget().waiting(), 0);
    // 各ジョブの最後のイベントは終わった状態で、実行開始は 1 回まで。
    let mut last_state = std::collections::HashMap::new();
    let mut starts = std::collections::HashMap::new();
    for e in events.try_iter() {
        if let JobEventKind::State(state) = e.kind {
            last_state.insert(e.id, state);
            if state == JobState::Running {
                *starts.entry(e.id).or_insert(0) += 1;
            }
        }
    }
    assert_eq!(last_state.len(), 600);
    assert!(last_state.values().all(|s| s.is_terminal()));
    assert!(starts.values().all(|&n| n == 1));
}

/// 受け取り側を drop した購読は自動で外れ、他の購読には影響しない。
#[test]
fn dropped_subscriber_does_not_block_events() {
    let s = scheduler(1, 0);
    let kept: Receiver<JobEvent> = s.subscribe();
    drop(s.subscribe());
    s.submit(Priority::P1, |_| Ok(())).wait().unwrap();
    assert_eq!(kept.try_iter().count(), 3);
}
