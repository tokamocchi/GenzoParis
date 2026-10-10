//! ワーカープロセスの異常への対応の結合テスト（04 の 1.2 節・6.3 節、02 の SEC-05、05 の PoC-2）。
//!
//! 実際に `genzo-worker` のバイナリを起動し、テスト用の口（`GENZO_WORKER_TEST_HOOKS=1`）で
//! 異常終了・ハング・不正な行・偽ったヘッダを起こして、本体が落ちずに再起動・検証・スキップ・
//! 共有メモリの回収をすることを確かめる。

mod common;

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use common::{TEST_JOB_TIMEOUT, config, shm_files_under, wait_until, write_jpeg};
use genzo_jobs::CancellationToken;
use genzo_worker::shm::SHM_HEADER_LEN;
use genzo_worker::{
    BufferError, GarbageMode, JobKind, Lane, MemoryLimitStatus, OversizeMode, PROTOCOL_VERSION,
    PoolConfig, TestHook, TestHookOutput, WorkerClient, WorkerClientError, WorkerErrorKind,
    WorkerPool,
};

fn token() -> CancellationToken {
    CancellationToken::new()
}

#[test]
fn ping_reports_version_and_pid() {
    let root = tempfile::tempdir().unwrap();
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();
    let pong = c.ping().unwrap();
    assert_eq!(pong.protocol_version, PROTOCOL_VERSION);
    assert_eq!(Some(pong.pid), c.pid());
    assert_ne!(pong.pid, std::process::id(), "別のプロセス");
    assert!(pong.test_hooks);
    assert_eq!(pong.raw_decoder, genzo_raw::decoder_id());
    assert_eq!(pong.memory_limit, MemoryLimitStatus::NotRequested);
    assert_eq!(c.worker_info().unwrap().pid, pong.pid);
    let s = c.stats();
    assert_eq!((s.spawns, s.restarts, s.responses), (1, 0, 1));
    c.shutdown().unwrap();
}

#[test]
fn crash_is_recovered_and_the_next_job_succeeds() {
    let root = tempfile::tempdir().unwrap();
    let jpeg = write_jpeg(root.path(), "a.jpg", (32, 16), |_, _| [10, 20, 30], None);
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();
    let first_pid = c.pid().unwrap();
    let err = c
        .run_test_hook(TestHook::Crash { with_shm: true }, None, &token())
        .unwrap_err();
    match &err {
        WorkerClientError::Crashed {
            detail,
            stderr_tail,
        } => {
            assert!(detail.contains("終了状態"), "{detail}");
            assert!(
                stderr_tail.contains("テスト用の口で異常終了"),
                "{stderr_tail}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(err.counts_toward_skip());
    // 本体は落ちず、ワーカーは再起動されている。
    let second_pid = c.pid().unwrap();
    assert_ne!(first_pid, second_pid);
    let s = c.stats();
    assert_eq!((s.crashes, s.restarts, s.spawns), (1, 1, 2));
    c.ping().unwrap();
    let probe = c.probe_photo(&jpeg, &token()).unwrap();
    assert_eq!(probe.metadata.width, Some(32));
    assert_eq!(shm_files_under(root.path()), 0, "共有メモリは回収される");
}

/// ワーカーが落ちても、孫プロセス（ffmpeg など）が制御のパイプを持ったままだと出力が閉じない
/// （Windows では孫プロセスがワーカーの標準入出力のハンドルを引き継ぐ）。本体はプロセスの終了で
/// 異常終了を見つけ、タイムアウトを待たない。
#[test]
fn crash_is_detected_while_a_grandchild_holds_the_pipes() {
    let root = tempfile::tempdir().unwrap();
    let cfg = config(root.path());
    // 出力が閉じるのを待つ実装では、ここでタイムアウトになる（子プロセスは共有メモリのファイルが
    // 消えるまで、最長 TEST_CHILD_HOLD_MAX（120 秒）パイプを持つ）。
    const {
        assert!(TEST_JOB_TIMEOUT.as_secs() < genzo_worker::worker::TEST_CHILD_HOLD_MAX.as_secs());
    };
    let mut c = WorkerClient::spawn(cfg).unwrap();
    let pid = c.pid().unwrap();
    let err = c
        .run_test_hook(TestHook::CrashLeavingChild, None, &token())
        .unwrap_err();
    match &err {
        WorkerClientError::Crashed { stderr_tail, .. } => assert!(
            stderr_tail.contains("テスト用の口で異常終了"),
            "{stderr_tail}"
        ),
        other => panic!("{other:?}"),
    }
    assert_ne!(c.pid().unwrap(), pid);
    assert_eq!(c.stats().crashes, 1);
    c.ping().unwrap();
    // 共有メモリは回収される（子プロセスもこれで終わる）。
    assert_eq!(shm_files_under(root.path()), 0);
}

#[test]
fn hang_times_out_and_the_worker_is_restarted() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path());
    // ハングは応答しないので、タイムアウトの長さは結果に影響しない。
    cfg.timeouts.test_hook = Duration::from_millis(500);
    let mut c = WorkerClient::spawn(cfg).unwrap();
    let pid = c.pid().unwrap();
    let err = c
        .run_test_hook(TestHook::Hang { with_shm: true }, None, &token())
        .unwrap_err();
    assert!(
        matches!(
            err,
            WorkerClientError::Timeout {
                kind: JobKind::TestHook,
                timeout
            } if timeout == Duration::from_millis(500)
        ),
        "{err:?}"
    );
    assert_ne!(c.pid().unwrap(), pid);
    assert_eq!(c.stats().timeouts, 1);
    c.ping().unwrap();
    assert_eq!(shm_files_under(root.path()), 0);
}

#[test]
fn garbage_lines_are_protocol_errors() {
    let root = tempfile::tempdir().unwrap();
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();
    for (i, mode) in [
        GarbageMode::NotJson,
        GarbageMode::WrongId,
        GarbageMode::TooLong,
    ]
    .into_iter()
    .enumerate()
    {
        let pid = c.pid().unwrap();
        let err = c
            .run_test_hook(TestHook::Garbage(mode), None, &token())
            .unwrap_err();
        assert!(
            matches!(err, WorkerClientError::Protocol(_)),
            "{mode:?}: {err:?}"
        );
        assert_ne!(c.pid().unwrap(), pid, "{mode:?}: 再起動される");
        assert_eq!(c.stats().protocol_errors, i as u64 + 1);
        c.ping().unwrap();
    }
}

/// 検証のエラーが期待どおりかを判定する関数。
type ExpectBufferError = fn(&BufferError) -> bool;

#[test]
fn forged_buffers_are_rejected_by_validation() {
    let root = tempfile::tempdir().unwrap();
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();
    let cases: Vec<(OversizeMode, ExpectBufferError)> = vec![
        (OversizeMode::TooManyPixels, |e| {
            matches!(
                e,
                BufferError::TooManyPixels {
                    pixels: 200_020_000,
                    ..
                }
            )
        }),
        (OversizeMode::LengthMismatch, |e| {
            matches!(
                e,
                BufferError::LengthMismatch {
                    expected: 6144,
                    actual: 6142
                }
            )
        }),
        (OversizeMode::BeyondCapacity, |e| {
            matches!(e, BufferError::BeyondCapacity { .. })
        }),
        (OversizeMode::DimensionMismatch, |e| {
            matches!(
                e,
                BufferError::DimensionMismatch {
                    header: (64, 48),
                    info: (48, 64)
                }
            )
        }),
        (OversizeMode::BadMagic, |e| {
            matches!(e, BufferError::BadMagic)
        }),
        (
            OversizeMode::InvalidLevels,
            |e| matches!(e, BufferError::Invalid(m) if m.contains("白レベル")),
        ),
        (OversizeMode::BadChecksum, |e| {
            matches!(e, BufferError::ChecksumMismatch)
        }),
    ];
    for (mode, expected) in cases {
        let pid = c.pid().unwrap();
        let err = c
            .run_test_hook(TestHook::Oversize(mode), None, &token())
            .unwrap_err();
        match &err {
            WorkerClientError::InvalidBuffer(e) => assert!(expected(e), "{mode:?}: {e:?}"),
            other => panic!("{mode:?}: {other:?}"),
        }
        assert!(err.counts_toward_skip());
        assert_ne!(c.pid().unwrap(), pid, "{mode:?}: 再起動される");
        assert_eq!(shm_files_under(root.path()), 0, "{mode:?}");
        c.ping().unwrap();
    }
    assert_eq!(c.stats().invalid_buffers, 7);
}

#[test]
fn checksum_is_not_required_when_disabled() {
    // チェックサムを確かめない設定では、偽ったチェックサムは見逃される（検証の他の項目は通る）。
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path());
    cfg.verify_checksum = false;
    let mut c = WorkerClient::spawn(cfg).unwrap();
    match c
        .run_test_hook(
            TestHook::Oversize(OversizeMode::BadChecksum),
            None,
            &token(),
        )
        .unwrap()
    {
        TestHookOutput::Raw(frame) => {
            assert_eq!((frame.image.width, frame.image.height), (64, 48));
            assert_eq!(frame.image.data[1], 601);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn two_consecutive_failures_skip_the_file() {
    let root = tempfile::tempdir().unwrap();
    let bad = write_jpeg(root.path(), "bad.jpg", (8, 8), |_, _| [0; 3], None);
    let good = write_jpeg(root.path(), "good.jpg", (8, 8), |_, _| [0; 3], None);
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();
    for n in 1..=2 {
        let err = c
            .run_test_hook(TestHook::Crash { with_shm: false }, Some(&bad), &token())
            .unwrap_err();
        assert!(matches!(err, WorkerClientError::Crashed { .. }), "{err:?}");
        assert_eq!(c.failures().consecutive_failures(&bad), n);
    }
    assert!(c.failures().is_skipped(&bad));
    // 以後は依頼せずにスキップする（ワーカーへの応答の数が増えない）。
    let responses = c.stats().responses;
    let err = c.probe_photo(&bad, &token()).unwrap_err();
    assert!(
        matches!(&err, WorkerClientError::Skipped { path, failures: 2 } if path == &bad),
        "{err:?}"
    );
    assert!(matches!(
        c.decode_image(&bad, &token()),
        Err(WorkerClientError::Skipped { .. })
    ));
    assert_eq!(c.stats().responses, responses);
    // 他のファイルは処理できる。
    c.probe_photo(&good, &token()).unwrap();
    // 手動で戻せば、再び処理する。
    c.failures().reset(&bad);
    c.probe_photo(&bad, &token()).unwrap();
}

/// 別の種類のジョブが合間に成功しても、続けての失敗は数え続ける（例: メタデータの読み取りは成功するが
/// 展開でハングする RAW を、サムネイルの作り直しなどの成功でスキップし損ねない）。
#[test]
fn success_of_another_job_kind_does_not_reset_the_count() {
    let root = tempfile::tempdir().unwrap();
    let path = write_jpeg(root.path(), "a.jpg", (8, 8), |_, _| [0; 3], None);
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();
    c.run_test_hook(TestHook::Crash { with_shm: false }, Some(&path), &token())
        .unwrap_err();
    // 別の種類（メタデータの読み取り）は成功する。
    c.probe_photo(&path, &token()).unwrap();
    assert_eq!(c.failures().consecutive_failures(&path), 1);
    c.run_test_hook(TestHook::Crash { with_shm: false }, Some(&path), &token())
        .unwrap_err();
    assert!(c.failures().is_skipped(&path));
    assert!(matches!(
        c.probe_photo(&path, &token()),
        Err(WorkerClientError::Skipped { failures: 2, .. })
    ));
}

#[test]
fn success_resets_and_normal_errors_do_not_count() {
    let root = tempfile::tempdir().unwrap();
    let path = write_jpeg(root.path(), "a.jpg", (8, 8), |_, _| [0; 3], None);
    let broken = root.path().join("broken.jpg");
    std::fs::write(&broken, b"\xFF\xD8\xFF\xE0 this is not a real jpeg").unwrap();
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();

    c.run_test_hook(TestHook::Crash { with_shm: false }, Some(&path), &token())
        .unwrap_err();
    assert_eq!(c.failures().consecutive_failures(&path), 1);
    // 同じ種類（テスト用の口）のジョブの成功で戻る。
    c.run_test_hook(TestHook::Allocate(1), Some(&path), &token())
        .unwrap();
    assert_eq!(c.failures().consecutive_failures(&path), 0, "成功で戻る");
    c.run_test_hook(TestHook::Crash { with_shm: false }, Some(&path), &token())
        .unwrap_err();
    assert!(!c.failures().is_skipped(&path), "続けての失敗ではない");

    // 壊れたファイル（ワーカーが通常のエラーとして返す）は何度失敗してもスキップしない。
    let pid = c.pid();
    for _ in 0..3 {
        let err = c.decode_image(&broken, &token()).unwrap_err();
        assert_eq!(
            err.worker_error_kind(),
            Some(&WorkerErrorKind::Decode),
            "{err:?}"
        );
        assert!(!err.counts_toward_skip());
    }
    assert!(!c.failures().is_skipped(&broken));
    assert_eq!(c.pid(), pid, "ワーカーは再起動されない");
}

#[test]
fn cancellation_kills_the_running_job_and_restarts() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path());
    // 取り消しで終わることを確かめるため、タイムアウトは十分に長くする。
    cfg.timeouts.test_hook = Duration::from_secs(600);
    let mut c = WorkerClient::spawn(cfg).unwrap();
    let pid = c.pid().unwrap();
    let cancel = token();
    let arena_dir = c.arena().path().to_path_buf();
    let canceller = {
        let cancel = cancel.clone();
        thread::spawn(move || {
            // ワーカーが依頼を受け取って共有メモリに書き始めたら（ハングの直前）取り消す。
            wait_until(Duration::from_secs(120), || {
                std::fs::read_dir(&arena_dir).unwrap().any(|e| {
                    let bytes = std::fs::read(e.unwrap().path()).unwrap_or_default();
                    bytes.get(SHM_HEADER_LEN) == Some(&0xAB)
                })
            });
            cancel.cancel();
        })
    };
    let err = c
        .run_test_hook(TestHook::Hang { with_shm: true }, None, &cancel)
        .unwrap_err();
    canceller.join().unwrap();
    assert!(matches!(err, WorkerClientError::Cancelled), "{err:?}");
    assert!(!err.counts_toward_skip());
    assert_eq!(c.stats().cancellations, 1);
    assert_ne!(
        c.pid().unwrap(),
        pid,
        "実行中のワーカーは終了させて再起動する"
    );
    c.ping().unwrap();
    assert_eq!(shm_files_under(root.path()), 0);
    // 取り消し済みのトークンでは、何も依頼しない。
    let responses = c.stats().responses;
    assert!(matches!(
        c.run_test_hook(TestHook::Crash { with_shm: false }, None, &cancel),
        Err(WorkerClientError::Cancelled)
    ));
    assert_eq!(c.stats().responses, responses);
    assert_eq!(c.stats().crashes, 0);
}

#[test]
fn test_hooks_are_disabled_unless_configured() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path());
    cfg.test_hooks = false;
    // 追加の環境変数で有効にしようとしても、設定が優先される。
    cfg.env
        .push((genzo_worker::ENV_TEST_HOOKS.into(), "1".into()));
    let mut c = WorkerClient::spawn(cfg).unwrap();
    assert!(!c.worker_info().unwrap().test_hooks);
    let pid = c.pid();
    for hook in [
        TestHook::Crash { with_shm: false },
        TestHook::Hang { with_shm: false },
        TestHook::Garbage(GarbageMode::NotJson),
        TestHook::Oversize(OversizeMode::BadMagic),
        TestHook::Allocate(1),
    ] {
        let err = c.run_test_hook(hook, None, &token()).unwrap_err();
        assert_eq!(
            err.worker_error_kind(),
            Some(&WorkerErrorKind::InvalidRequest),
            "{hook:?}: {err:?}"
        );
    }
    assert_eq!(c.pid(), pid);
    assert_eq!(shm_files_under(root.path()), 0);
}

#[test]
fn shared_memory_directory_is_removed_with_the_client() {
    let root = tempfile::tempdir().unwrap();
    let jpeg = write_jpeg(root.path(), "a.jpg", (40, 30), |_, _| [1, 2, 3], None);
    let c = {
        let mut c = WorkerClient::spawn(config(root.path())).unwrap();
        c.decode_image(&jpeg, &token()).unwrap();
        c.run_test_hook(TestHook::Crash { with_shm: true }, None, &token())
            .unwrap_err();
        c
    };
    let dir = c.arena().path().to_path_buf();
    assert!(dir.is_dir());
    assert_eq!(shm_files_under(root.path()), 0);
    drop(c);
    assert!(!dir.exists(), "一時ディレクトリごと削除される");
}

#[test]
fn spawn_errors_are_reported() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path());
    cfg.executable = Some(root.path().join("no-such-worker"));
    assert!(matches!(
        WorkerClient::spawn(cfg),
        Err(WorkerClientError::Spawn(_))
    ));
    // 実行できないファイル。
    let not_exe = root.path().join("not-a-worker.txt");
    std::fs::write(&not_exe, b"hello").unwrap();
    let mut cfg = config(root.path());
    cfg.executable = Some(not_exe);
    assert!(matches!(
        WorkerClient::spawn(cfg),
        Err(WorkerClientError::Spawn(_))
    ));
}

#[test]
fn pool_acquire_can_be_cancelled_while_waiting() {
    let root = tempfile::tempdir().unwrap();
    let pool = WorkerPool::new(PoolConfig {
        worker: config(root.path()),
        batch_workers: 1,
    })
    .unwrap();
    let lease = pool.acquire(Lane::Interactive, &token()).unwrap();
    let cancel = token();
    let (tx, rx) = mpsc::channel();
    thread::scope(|s| {
        s.spawn(|| {
            let r = pool.acquire(Lane::Interactive, &cancel).map(|_| ());
            tx.send(r).unwrap();
        });
        cancel.cancel();
        let r = rx.recv().unwrap();
        assert!(matches!(r, Err(WorkerClientError::Cancelled)), "{r:?}");
    });
    drop(lease);
    // 返したワーカーは再び借りられる。
    pool.ping(Lane::Interactive).unwrap();
    pool.shutdown().unwrap();
}

#[test]
fn pool_shares_failures_between_workers() {
    let root = tempfile::tempdir().unwrap();
    let path = write_jpeg(root.path(), "a.jpg", (8, 8), |_, _| [0; 3], None);
    let pool = WorkerPool::new(PoolConfig {
        worker: config(root.path()),
        batch_workers: 2,
    })
    .unwrap();
    // 1 回目は対話用、2 回目はバッチ用のワーカーで失敗しても、続けての失敗として数える。
    pool.acquire(Lane::Interactive, &token())
        .unwrap()
        .run_test_hook(TestHook::Crash { with_shm: false }, Some(&path), &token())
        .unwrap_err();
    pool.acquire(Lane::Batch, &token())
        .unwrap()
        .run_test_hook(TestHook::Crash { with_shm: false }, Some(&path), &token())
        .unwrap_err();
    for lane in [Lane::Interactive, Lane::Batch] {
        assert!(matches!(
            pool.probe_photo(lane, &path, &token()),
            Err(WorkerClientError::Skipped { .. })
        ));
    }
    assert_eq!(pool.failures().skipped_paths(), vec![path]);
    pool.ping(Lane::Interactive).unwrap();
    pool.ping(Lane::Batch).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn memory_limit_is_applied_on_linux() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path());
    cfg.memory_limit_bytes = Some(256 * 1024 * 1024);
    let mut c = WorkerClient::spawn(cfg).unwrap();
    assert_eq!(
        c.worker_info().unwrap().memory_limit,
        MemoryLimitStatus::Applied {
            bytes: 256 * 1024 * 1024
        }
    );
    // 上限を超える確保は失敗する（ワーカーは無事）。
    let pid = c.pid();
    let err = c
        .run_test_hook(TestHook::Allocate(1024 * 1024 * 1024), None, &token())
        .unwrap_err();
    assert_eq!(
        err.worker_error_kind(),
        Some(&WorkerErrorKind::TooLarge),
        "{err:?}"
    );
    assert_eq!(c.pid(), pid);
    // 上限より小さい確保はできる。
    assert_eq!(
        c.run_test_hook(TestHook::Allocate(16 * 1024 * 1024), None, &token())
            .unwrap(),
        TestHookOutput::Allocated(16 * 1024 * 1024)
    );
}

#[cfg(not(target_os = "linux"))]
#[test]
fn memory_limit_is_reported_as_not_applied() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path());
    cfg.memory_limit_bytes = Some(256 * 1024 * 1024);
    let c = WorkerClient::spawn(cfg).unwrap();
    assert!(matches!(
        c.worker_info().unwrap().memory_limit,
        MemoryLimitStatus::NotApplied { .. }
    ));
}

#[test]
fn shutdown_is_graceful() {
    let root = tempfile::tempdir().unwrap();
    let c = WorkerClient::spawn(config(root.path())).unwrap();
    c.shutdown().unwrap();
    let pool = WorkerPool::new(PoolConfig {
        worker: config(root.path()),
        batch_workers: 2,
    })
    .unwrap();
    assert_eq!(pool.worker_count(Lane::Interactive), 1);
    assert_eq!(pool.worker_count(Lane::Batch), 2);
    pool.shutdown().unwrap();
}
