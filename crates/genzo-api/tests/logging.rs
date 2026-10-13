//! ログとクラッシュレポート（SYS-07）の結合テスト。
//!
//! [`genzo_api::logging::init`] はプロセスに 1 回だけなので、このファイル（テストの実行ファイル）では
//! 初期化するテストを 1 つだけにする（ほかのテストを足すときは、初期化しないものにする）。

mod common;

use std::fs;

use common::Env;
use genzo_api::logging::{self, LogConfig, Program};
use genzo_api::{Lane, TestHook};

#[test]
fn init_writes_logs_and_a_crash_report_for_a_panic() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("logs");
    let status = logging::init(LogConfig {
        program: Program::App,
        version: "9.9.9-test".to_owned(),
        dir: Some(dir.clone()),
        file_filter: Some("info".to_owned()),
        stderr_filter: "off".to_owned(),
        stderr_target: true,
    });
    assert!(status.problems.is_empty(), "{:?}", status.problems);
    assert!(status.file_enabled);
    assert_eq!(status.file.as_deref(), Some(dir.join("app.log").as_path()));
    // 2 回目は何もせず、最初の結果を返す。
    assert_eq!(
        logging::init(LogConfig {
            program: Program::Cli,
            version: "0".to_owned(),
            dir: None,
            file_filter: None,
            stderr_filter: "warn".to_owned(),
            stderr_target: false,
        }),
        status
    );
    assert_eq!(logging::status(), Some(status.clone()));

    tracing::info!(path = "/photos/京都/DSC00001.ARW", "写真を開いた");
    tracing::debug!("info より細かいものはファイルに書かない");
    let log = fs::read_to_string(dir.join("app.log")).unwrap();
    assert!(log.contains("起動しました"), "{log}");
    assert!(log.contains("9.9.9-test"), "{log}");
    assert!(log.contains("写真を開いた"), "{log}");
    assert!(log.contains("DSC00001.ARW"), "{log}");
    assert!(!log.contains("細かいもの"), "{log}");

    // ワーカーの異常終了は本体のログに残す（終了状態・標準エラー出力の末尾）。ワーカーの標準エラー出力も
    // 行ごとに転送する。クラッシュレポートは作らない。
    let env = Env::new();
    let core = env.open();
    assert!(
        core.run_worker_test_hook(Lane::Batch, TestHook::Crash { with_shm: false })
            .is_err()
    );
    core.close().unwrap();
    let log = fs::read_to_string(dir.join("app.log")).unwrap();
    assert!(log.contains("ワーカーが異常終了しました"), "{log}");
    assert!(log.contains("genzo_worker::stderr"), "{log}");
    assert!(log.contains("テスト用の口で異常終了します"), "{log}");
    assert!(logging::list_crash_reports(&dir).is_empty());

    // スレッドの中のパニック（プロセスは続く）でも、レポートを保存する。
    let joined = std::thread::Builder::new()
        .name("テストのスレッド".to_owned())
        .spawn(|| panic!("テストのパニック: 索引が範囲外"))
        .unwrap()
        .join();
    assert!(joined.is_err());
    let reports = logging::list_crash_reports(&dir);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let report = fs::read_to_string(&reports[0].path).unwrap();
    for expected in [
        "外部へは送信しません",
        "プログラム: アプリ（app） 9.9.9-test",
        &format!("OS: {}", std::env::consts::OS),
        "スレッド: テストのスレッド",
        "内容: テストのパニック: 索引が範囲外",
        "バックトレース:",
        "写真を開いた",
    ] {
        assert!(report.contains(expected), "{expected:?} がない:\n{report}");
    }
    assert_eq!(
        reports[0].message.as_deref(),
        Some("テストのパニック: 索引が範囲外")
    );
    let log = fs::read_to_string(dir.join("app.log")).unwrap();
    assert!(log.contains("パニック（スレッド テストのスレッド"), "{log}");
    assert!(log.contains("クラッシュレポートを保存しました"), "{log}");
    // 今のプロセスのレポートは「前回」として知らせない。
    assert!(logging::take_unseen_crash_reports(&dir, Program::App).is_empty());

    // 1 つのプロセスで保存するのは上限まで。
    for _ in 0..logging::MAX_REPORTS_PER_PROCESS + 2 {
        let _ = std::thread::spawn(|| panic!("繰り返すパニック")).join();
    }
    assert_eq!(
        logging::list_crash_reports(&dir).len(),
        logging::MAX_REPORTS_PER_PROCESS
    );

    // 消す: 書いている途中のログは空にし、レポートは消す。
    let info = logging::folder_info(&dir);
    assert_eq!(
        info.current_file.as_deref(),
        Some(dir.join("app.log").as_path())
    );
    assert!(info.total_bytes > 0);
    let cleared = logging::clear_logs(&dir);
    assert_eq!(cleared.truncated, 1);
    assert_eq!(cleared.removed, logging::MAX_REPORTS_PER_PROCESS as u64);
    assert!(cleared.failed.is_empty(), "{:?}", cleared.failed);
    let log = fs::read_to_string(dir.join("app.log")).unwrap();
    assert!(!log.contains("DSC00001.ARW"), "{log}");
    assert!(
        log.contains("ログとクラッシュレポートを消しました"),
        "{log}"
    );
    // 消した後も書き続ける。
    tracing::warn!("消した後の警告");
    assert!(
        fs::read_to_string(dir.join("app.log"))
            .unwrap()
            .contains("消した後の警告")
    );
}
