//! アプリ（genzo-app）の結合テスト: 本物のコア（`genzo_api::Core`）とワーカーのプロセスで
//! [`genzo_app::state::AppState`] を動かし、画面が使う操作（カタログを開く・閉じる・復元する、コアの
//! コマンドの JSON、`genzo://` の応答、現像の画素のバイト列、イベントの転送）を確かめる。
//!
//! # ワーカーの起動（テスト用の実行ファイルを足さない理由）
//!
//! このテストの実行ファイル自身を、隠しサブコマンド（`__worker`。[`genzo_worker::WORKER_SUBCOMMAND`]）付きで
//! ワーカーとして起動する（[`genzo_api::WorkerLaunch::SelfSubcommand`] に `std::env::current_exe` を渡す。
//! genzo-api の結合テストが `genzo-api-worker` で行うのと同じ形）。最初の引数が `__worker` なら
//! [`genzo_worker::run_worker`] を呼び、それ以外ならテストを実行する。libtest の main はワーカーの引数を
//! テストの絞り込みとして扱ってしまうので使わない（Cargo.toml の `harness = false`。実行器は [`runner`]）。
//!
//! genzo-api のようにテスト用の実行ファイル（`[[bin]]`）を足さないのは、Tauri の CLI（`tauri build`・
//! `tauri bundle`）が、アプリの crate の `[[bin]]` と `src/bin/` の実行ファイルを、`mainBinaryName`
//! （`genzo-paris`）以外もすべて配布物に入れるため（Tauri の CLI 2.11.5 の .deb で確かめた。nsis・.app・
//! .dmg は要確認。確かめた方法は runner.rs の doc）。テストの実行ファイルは `cargo test` だけが作り、
//! `tauri build` は作らないので、配布物には入らない。この crate の実行ファイルは `genzo-paris` だけに
//! しておくこと。
//!
//! # 方針
//!
//! - テストは 1 つずつ順に実行する（各テストがカタログとワーカーのプロセスを作るため。並べると CI の
//!   ランナーで遅くなり、時間の上限で誤って失敗しうる）。
//! - 結果は応答の内容で決め、時間では決めない（待つときは十分に長い上限だけを置く）。
//! - GPU は使わない（アプリの設定の `gpu` を `off` にする。GPU の有無で結果が変わらないように）。

mod backup;
mod catalog;
mod develop;
mod events;
mod library;
mod runner;
mod support;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

/// テストの一覧（名前と関数）。
const TESTS: &[(&str, fn())] = &[
    (
        "runner::parses_libtest_arguments",
        runner::parses_libtest_arguments,
    ),
    (
        "catalog::open_creates_catalog_data_dir_and_remembers_it",
        catalog::open_creates_catalog_data_dir_and_remembers_it,
    ),
    (
        "catalog::other_spellings_of_the_open_catalog_reuse_it",
        catalog::other_spellings_of_the_open_catalog_reuse_it,
    ),
    (
        "catalog::relative_paths_are_made_absolute",
        catalog::relative_paths_are_made_absolute,
    ),
    (
        "catalog::catalog_open_in_another_app_is_a_conflict",
        catalog::catalog_open_in_another_app_is_a_conflict,
    ),
    (
        "catalog::open_and_close_from_many_threads",
        catalog::open_and_close_from_many_threads,
    ),
    (
        "catalog::requests_while_closing_get_closed_errors",
        catalog::requests_while_closing_get_closed_errors,
    ),
    (
        "catalog::failed_save_keeps_the_catalog_open_until_retry_or_discard",
        catalog::failed_save_keeps_the_catalog_open_until_retry_or_discard,
    ),
    (
        "catalog::errors_after_the_catalog_closed_do_not_stop_opening_another",
        catalog::errors_after_the_catalog_closed_do_not_stop_opening_another,
    ),
    (
        "catalog::exit_closes_the_catalog_left_open",
        catalog::exit_closes_the_catalog_left_open,
    ),
    (
        "library::import_thumbnails_and_previews_over_the_uri_scheme",
        library::import_thumbnails_and_previews_over_the_uri_scheme,
    ),
    (
        "library::previews_are_not_cached_long_when_settings_go_back",
        library::previews_are_not_cached_long_when_settings_go_back,
    ),
    (
        "library::keywords_and_collections_through_the_app",
        library::keywords_and_collections_through_the_app,
    ),
    (
        "library::stacks_and_virtual_copies_through_the_app",
        library::stacks_and_virtual_copies_through_the_app,
    ),
    (
        "library::content_identity_through_the_app",
        library::content_identity_through_the_app,
    ),
    (
        "library::copy_import_through_the_app",
        library::copy_import_through_the_app,
    ),
    (
        "develop::develop_and_region_frames_match_their_headers",
        develop::develop_and_region_frames_match_their_headers,
    ),
    (
        "develop::before_frames_match_their_headers",
        develop::before_frames_match_their_headers,
    ),
    (
        "backup::corrupt_catalog_is_restored_from_a_backup",
        backup::corrupt_catalog_is_restored_from_a_backup,
    ),
    (
        "backup::restoring_the_open_catalog_closes_and_reopens_it",
        backup::restoring_the_open_catalog_closes_and_reopens_it,
    ),
    (
        "backup::restore_refuses_files_that_are_not_catalogs",
        backup::restore_refuses_files_that_are_not_catalogs,
    ),
    (
        "backup::restore_waits_for_another_app_to_close_the_catalog",
        backup::restore_waits_for_another_app_to_close_the_catalog,
    ),
    (
        "backup::requests_during_a_restore_wait_or_get_closed_errors",
        backup::requests_during_a_restore_wait_or_get_closed_errors,
    ),
    (
        "backup::restore_goes_on_when_closing_fails_after_the_catalog_closed",
        backup::restore_goes_on_when_closing_fails_after_the_catalog_closed,
    ),
    (
        "events::core_events_reach_the_sink",
        events::core_events_reach_the_sink,
    ),
    (
        "events::startup_warnings_are_not_forwarded_twice",
        events::startup_warnings_are_not_forwarded_twice,
    ),
    (
        "events::events_of_a_closed_catalog_are_not_forwarded_later",
        events::events_of_a_closed_catalog_are_not_forwarded_later,
    ),
];

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    // ワーカーとして起動された（コアが `<この実行ファイル> __worker ...` で起動する）。標準出力はワーカーの
    // 通信に使うので、ここより前に何も出力しないこと。
    if args
        .first()
        .is_some_and(|a| a.as_os_str() == genzo_worker::WORKER_SUBCOMMAND)
    {
        let mut it = args.into_iter();
        let prefix: Vec<OsString> = it.next().into_iter().collect();
        return genzo_worker::run_worker(prefix, it.collect());
    }
    // 合図で終わるワーカー（support::ExitSwitch。閉じる処理の途中のエラーを起こすテスト用）。
    if let Some(marker) = args
        .first()
        .and_then(|a| a.to_str())
        .and_then(|a| a.strip_prefix(support::EXITING_WORKER_PREFIX))
    {
        support::exit_when_signalled(PathBuf::from(marker));
        let mut it = args.into_iter();
        let prefix: Vec<OsString> = it.next().into_iter().collect();
        return genzo_worker::run_worker(prefix, it.collect());
    }
    runner::run(TESTS, &args)
}
