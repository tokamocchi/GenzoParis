//! GenzoParis のデスクトップアプリ（案 A: Tauri 2 ＋ React / TypeScript。決定事項ログ No.14・No.60）。
//!
//! 画面（`ui/`）はコア API（genzo-api のコマンド・イベント）だけを使う（04 の 1.1 節・1.5 節）。この
//! crate は、Tauri と コアの間をつなぐ薄い層で、次を受け持つ。
//!
//! | モジュール | 内容 |
//! |---|---|
//! | [`state`] | 開いているカタログ（コア）、カタログを開く・閉じる・復元する、イベントの転送 |
//! | [`commands`] | Tauri のコマンド（画面 → アプリ。コアのコマンドの中継と、アプリ固有の操作） |
//! | [`protocol`] | `genzo://` の URI スキーム（サムネイル・標準プレビューの JPEG） |
//! | [`frames`] | 現像のプレビュー・表示範囲の画素のバイナリの形 |
//! | [`settings`] | アプリの設定（カタログの場所・FFmpeg・GPU） |
//! | [`tools`] | ffmpeg・ffprobe を探す |
//! | [`catalog_file`] | 既定のカタログの場所、データのフォルダ、カタログのファイルの判定 |
//!
//! # 画面への画像の表示（決定事項ログ No.60。04 の 1.5 節からの変更）
//!
//! 設計では、現像の画像を WebView を通さずにネイティブのビューポート（wgpu のサーフェス）に描き、その上に
//! 透明な WebView を重ねる方式だった。この方式は Windows・macOS の実機での確認（PoC-1）が要るため、MVP では
//! 次の方式にした。
//!
//! - コアは画面用の画素を **Display P3**（canvas が使えなければ sRGB）で作る（ステージ 17a の画面の
//!   プロファイルに標準の Display P3 を指定する）。
//! - 画面は、その画素を同じ色空間の canvas（`colorSpace: "display-p3"`）に描く。サムネイル・標準プレビューは
//!   Display P3 の ICC を埋め込んだ JPEG を `<img>` で表示する。
//! - モニターのプロファイルへの変換は WebView（OS の色管理）が行う。
//!
//! # イベント
//!
//! コアのイベント（genzo-api の `Event`）は `core-event` という名前で画面に送る。アプリのイベントは
//! `app-close-failed`（終了のときに保存できなかった）。
//!
//! - 起動時の警告（前回の異常終了など）は、カタログを開いた結果（[`state::CatalogStatus`] の `warnings`）で
//!   返し、`core-event` では送らない（画面が 2 回知らせないため）。
//! - カタログを閉じた後は、そのカタログのイベントを送らない（溜まっていても。次に開いたカタログの画面に
//!   前のカタログのイベントが届かないように。[`state`] の doc）。
//!
//! # テスト
//!
//! `tests/app` の結合テストは、本物のコアとワーカーで [`state::AppState`] を動かす。テストの実行ファイル
//! 自身をワーカーとして起動するので、この crate にテスト用の実行ファイル（`[[bin]]`）は足さない（Tauri の
//! CLI は `[[bin]]` と `src/bin/` の実行ファイルをすべて配布物に入れるため。`tests/app/main.rs` の doc）。

pub mod catalog_file;
pub mod commands;
pub mod frames;
pub mod protocol;
pub mod settings;
pub mod state;
pub mod tools;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use crate::state::AppState;

/// 終了の処理を済ませたか（済ませた後の終了の要求は止めない）。
pub(crate) static QUITTING: AtomicBool = AtomicBool::new(false);

/// コアのイベントの名前（画面の `listen` で受け取る）。
pub const CORE_EVENT: &str = "core-event";
/// 終了のときに保存できなかったことを知らせるイベントの名前。
pub const CLOSE_FAILED_EVENT: &str = "app-close-failed";

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("GENZO_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// 終了の要求を受けたとき: カタログを閉じてから終わる。保存できなければ画面に知らせて終わらない。
fn request_quit(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let state = Arc::clone(&app.state::<Arc<AppState>>());
        match state.close_catalog(false) {
            Ok(()) => {}
            Err(e) if e.still_open() => {
                let failure = commands::CloseFailure {
                    error: e.info(),
                    still_open: true,
                };
                if let Err(err) = app.emit(CLOSE_FAILED_EVENT, &failure) {
                    tracing::error!(error = %err, "終了の失敗を画面に知らせられない");
                }
                return;
            }
            Err(e) => tracing::warn!(error = ?e.info(), "カタログを閉じる処理でエラー"),
        }
        QUITTING.store(true, Ordering::SeqCst);
        app.exit(0);
    });
}

/// アプリを起動する。
pub fn run() {
    init_tracing();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let config_dir = app.path().app_config_dir()?;
            let sink: state::EventSink = Arc::new(move |event| {
                if let Err(e) = handle.emit(CORE_EVENT, event) {
                    tracing::debug!(error = %e, "イベントを画面に送れない");
                }
            });
            app.manage(Arc::new(AppState::new(config_dir, sink)));
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("genzo", protocol::handle)
        .invoke_handler(tauri::generate_handler![
            commands::app_status,
            commands::inspect_catalog_path,
            commands::open_catalog,
            commands::close_catalog,
            commands::core_execute,
            commands::develop_frame,
            commands::region_frame,
            commands::set_display_target,
            commands::list_catalog_backups,
            commands::restore_catalog,
            commands::update_app_settings,
            commands::locate_tools,
            commands::path_kinds,
            commands::reveal_variant_file,
            commands::open_variant_file,
            commands::quit_app,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && !QUITTING.load(Ordering::SeqCst)
            {
                api.prevent_close();
                request_quit(window.app_handle());
            }
        })
        .build(tauri::generate_context!())
        .expect("GenzoParis を起動できません");
    app.run(|handle, event| {
        if let RunEvent::ExitRequested { api, .. } = event
            && !QUITTING.load(Ordering::SeqCst)
        {
            api.prevent_exit();
            request_quit(handle);
        }
    });
}
