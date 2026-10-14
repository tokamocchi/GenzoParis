//! GenzoParis のデスクトップアプリ（案 A: Tauri 2 ＋ React / TypeScript。決定事項ログ No.14・No.60）。
//!
//! 画面（`ui/`）はコア API（genzo-api のコマンド・イベント）だけを使う（04 の 1.1 節・1.5 節）。この
//! crate は、Tauri と コアの間をつなぐ薄い層で、次を受け持つ。
//!
//! | モジュール | 内容 |
//! |---|---|
//! | [`state`] | 開いているカタログ（コア）、カタログを開く・閉じる・復元する、イベントの転送 |
//! | [`commands`] | Tauri のコマンド（画面 → アプリ。コアのコマンドの中継と、アプリ固有の操作） |
//! | [`protocol`] | `genzo://` の URI スキーム（サムネイル・標準プレビューの JPEG、動画の元のファイルと再生用のファイルを Range の要求に応えて配信する。VID-04） |
//! | [`frames`] | 現像のプレビュー・表示範囲の画素のバイナリの形 |
//! | [`settings`] | アプリの設定（カタログの場所・FFmpeg・GPU） |
//! | [`tools`] | ffmpeg・ffprobe を探す |
//! | [`catalog_file`] | 既定のカタログの場所、データのフォルダ、カタログのファイルの判定 |
//! | [`logs`] | ログ・クラッシュレポート（SYS-07。起動時の初期化、前回のクラッシュの知らせ、ログのフォルダを開く・消す、画面のエラーの記録。外部へは送信しない） |
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
//! # 終了
//!
//! 終わる前に、開いているカタログを閉じる（保存待ちの現像設定の保存・終了時のバックアップ・正常に終了した
//! 印。genzo-api の `Core::close`）。終わり方ごとに次のように通す。
//!
//! | 終わり方 | 通る処理 |
//! |---|---|
//! | ウィンドウを閉じる（閉じるボタン・Alt+F4・macOS の ⌘W） | `CloseRequested` を止めて `request_quit` |
//! | macOS のメニューの「GenzoParis を終了」（⌘Q） | メニューの項目（`app_menu`。[`QUIT_MENU_ID`]）から `request_quit` |
//! | `AppHandle::exit` など（`ExitRequested`） | 止めて `request_quit` |
//! | macOS の Dock の「終了」・ログアウト、Windows のシャットダウン・サインアウト | `RunEvent::Exit` で `close_at_exit` |
//!
//! `request_quit` は、まず画面に終了の要求を知らせ（`app-quit-requested`）、画面が現像の送信待ちの設定をコアに
//! 送り切ってから `quit_app` を呼ぶ（画面が答えなければ、数秒後に画面を待たずに閉じる）。保存できなければ
//! 終わらずに画面に知らせる（`app-close-failed`。画面が再試行か、捨てて終わるかを選ばせる）。最後の行は、終了の要求を経ずに、すぐにプロセスが終わる（macOS の NSApp の
//! `terminate:` は、tao 0.35.3 が `applicationShouldTerminate:` を実装していないので止められず、
//! `applicationWillTerminate:` の後に終わる。Windows の `WM_ENDSESSION` も、戻ると OS が終わらせる）ので、
//! 画面に選択肢を出せない。その場で同期して閉じ、保存できなければ捨てて閉じる
//! （[`state::AppState::close_at_exit`] の doc）。
//!
//! # テスト
//!
//! `tests/app` の結合テストは、本物のコアとワーカーで [`state::AppState`] を動かす。テストの実行ファイル
//! 自身をワーカーとして起動するので、この crate にテスト用の実行ファイル（`[[bin]]`）は足さない（Tauri の
//! CLI は `[[bin]]` と `src/bin/` の実行ファイルをすべて配布物に入れるため。`tests/app/main.rs` の doc）。

pub mod catalog_file;
pub mod commands;
pub mod frames;
pub mod logs;
pub mod protocol;
pub mod settings;
pub mod state;
pub mod tools;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu, WINDOW_SUBMENU_ID};
use tauri::{AppHandle, Emitter, Manager, RunEvent, Runtime, WindowEvent};

use crate::state::AppState;

/// 終了の処理を済ませたか（済ませた後の終了の要求は止めない）。
pub(crate) static QUITTING: AtomicBool = AtomicBool::new(false);

/// コアのイベントの名前（画面の `listen` で受け取る）。
pub const CORE_EVENT: &str = "core-event";
/// 終了のときに保存できなかったことを知らせるイベントの名前。
pub const CLOSE_FAILED_EVENT: &str = "app-close-failed";
/// 終了の要求を画面に知らせるイベントの名前（値は要求の通し番号）。画面は `ack_quit_request` で受け取ったと
/// 答え、現像の送信待ちの設定をコアに送り切ってから `quit_app` を呼ぶ（`request_quit`）。
pub const QUIT_REQUESTED_EVENT: &str = "app-quit-requested";
/// 画面が終了の要求を受け取ったと答えるまで待つ時間（過ぎたら、画面を待たずに閉じる。画面を読み込めて
/// いない・止まっているときに、終われなくならないように）。
const QUIT_ACK_TIMEOUT: Duration = Duration::from_secs(3);

/// 終了の要求の通し番号と、画面が受け取ったと答えた番号（`request_quit`・`commands::ack_quit_request`）。
pub(crate) struct QuitRequests {
    seq: AtomicU64,
    acked: AtomicU64,
}

impl QuitRequests {
    const fn new() -> Self {
        Self {
            seq: AtomicU64::new(0),
            acked: AtomicU64::new(0),
        }
    }

    /// 新しい要求の番号（1 から）。
    fn next(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 画面が要求 `id` を受け取った（それより前の要求も受け取ったとみなす）。
    pub(crate) fn ack(&self, id: u64) {
        self.acked.fetch_max(id, Ordering::SeqCst);
    }

    /// 要求 `id` を画面が受け取ったか。
    fn acked(&self, id: u64) -> bool {
        self.acked.load(Ordering::SeqCst) >= id
    }
}

pub(crate) static QUIT_REQUESTS: QuitRequests = QuitRequests::new();
/// macOS のアプリのメニューの「GenzoParis を終了」（⌘Q）の ID（`app_menu`）。
pub const QUIT_MENU_ID: &str = "genzo-quit";

/// 終了の要求を受けたとき（ウィンドウを閉じる・macOS の ⌘Q・`ExitRequested`）。
///
/// まず画面に知らせる（[`QUIT_REQUESTED_EVENT`]）。画面は、現像の送信待ちの設定（スライダーを動かした直後の
/// 最新の値など。`ui/src/state/develop.ts` の送信の列）をコアに送り切ってから `quit_app` を呼ぶ。ここで
/// 直接閉じると、送信待ちの値が届く前にコアを閉じて、最後の変更を失うため（DATA-03）。
///
/// 画面が [`QUIT_ACK_TIMEOUT`] のうちに受け取ったと答えなければ（読み込めていない・止まっている）、画面を
/// 待たずに閉じる: カタログを閉じてから終わる。保存できなければ画面に知らせて終わらない。
fn request_quit(app: &AppHandle) {
    let id = QUIT_REQUESTS.next();
    let delivered = match app.emit(QUIT_REQUESTED_EVENT, id) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "終了の要求を画面に知らせられない");
            false
        }
    };
    let app = app.clone();
    std::thread::spawn(move || {
        if delivered {
            let deadline = Instant::now() + QUIT_ACK_TIMEOUT;
            while Instant::now() < deadline {
                if QUIT_REQUESTS.acked(id) || QUITTING.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if QUIT_REQUESTS.acked(id) || QUITTING.load(Ordering::SeqCst) {
                return;
            }
            tracing::warn!("画面が終了の要求に答えないので、画面を待たずにカタログを閉じる");
        }
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

/// 終了の要求を経ずにプロセスが終わるとき（`RunEvent::Exit`。モジュールの doc の「終了」）: 終了の処理を
/// 済ませていなければ、その場で同期してカタログを閉じる（[`AppState::close_at_exit`]。保存できなければ
/// 捨てて閉じる）。失敗は記録だけ（もう終わるので、画面には知らせられない）。
///
/// `request_quit` が閉じている途中なら、閉じ終えるのを待つ（`AppState` の開く・閉じるのロック）。
fn close_at_exit(app: &AppHandle) {
    if QUITTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(state) = app.try_state::<Arc<AppState>>() else {
        return;
    };
    if let Err(e) = state.close_at_exit() {
        tracing::error!(error = ?e.info(), "アプリが終わる間際にカタログを閉じる処理でエラー");
    }
}

/// macOS のメニュー（Tauri 2.11.6 の `Menu::default` の macOS の分を元に、文言を日本語にし、「終了」を通常の
/// 項目にしたもの）。
///
/// - `Menu::default` の「終了」（`PredefinedMenuItem::quit`）は、NSApp に `terminate:` を直接送る。tao 0.35.3 は
///   `applicationShouldTerminate:` を実装していないので、終了の要求（`ExitRequested`）もウィンドウを閉じる
///   要求（`CloseRequested`）も来ずに終わり、保存できなかったときに画面で選ばせられない。そこで「GenzoParis を
///   終了」（⌘Q）は通常の項目（[`QUIT_MENU_ID`]）にし、`run` の `on_menu_event` で `request_quit` を通す。
///   ショートカットは `CmdOrCtrl+Q` と書く（muda 0.19.3 の macOS では `CmdOrCtrl` だけが ⌘ になり、`Cmd` は
///   ⌘ にならない）。
/// - 編集のメニューの取り消す・やり直す・カット・コピー・貼り付け・すべてを選択は、入力欄で ⌘Z・⌘C などを
///   使えるように、OS の項目（`PredefinedMenuItem`）のまま残す。
/// - ウィンドウのメニューは、ID を [`WINDOW_SUBMENU_ID`] にして、Tauri に macOS のウィンドウのメニューとして
///   登録させる（`Menu::default` と同じ）。`Menu::default` の「表示」のフルスクリーンは、表示のメニューを
///   作らずにウィンドウのメニューに置く。ファイルのメニュー（ウィンドウを閉じるだけ）と、空のヘルプの
///   メニューは作らない。
///
/// macOS でだけ付ける（Windows・Linux はメニューを付けない。Tauri も `Menu::default` を macOS でだけ付ける）。
/// macOS 以外でもコンパイルするのは、この関数の型を Linux の clippy・テストのビルドでも確かめるため。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let pkg = app.package_info();
    let config = app.config();
    let name = pkg.name.clone();
    let about = AboutMetadata {
        name: Some(name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };
    let app_submenu = Submenu::with_items(
        app,
        &name,
        true,
        &[
            &PredefinedMenuItem::about(app, Some(&format!("{name} について")), Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, Some("サービス"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some(&format!("{name} を隠す")))?,
            &PredefinedMenuItem::hide_others(app, Some("ほかを隠す"))?,
            &PredefinedMenuItem::show_all(app, Some("すべてを表示"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                QUIT_MENU_ID,
                format!("{name} を終了"),
                true,
                Some("CmdOrCtrl+Q"),
            )?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        "編集",
        true,
        &[
            &PredefinedMenuItem::undo(app, Some("取り消す"))?,
            &PredefinedMenuItem::redo(app, Some("やり直す"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some("カット"))?,
            &PredefinedMenuItem::copy(app, Some("コピー"))?,
            &PredefinedMenuItem::paste(app, Some("貼り付け"))?,
            &PredefinedMenuItem::select_all(app, Some("すべてを選択"))?,
        ],
    )?;
    let window = Submenu::with_id_and_items(
        app,
        WINDOW_SUBMENU_ID,
        "ウィンドウ",
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some("最小化"))?,
            &PredefinedMenuItem::maximize(app, Some("拡大／縮小"))?,
            &PredefinedMenuItem::fullscreen(app, Some("フルスクリーンにする"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some("ウィンドウを閉じる"))?,
        ],
    )?;
    Menu::with_items(app, &[&app_submenu, &edit, &window])
}

/// アプリを起動する。
pub fn run() {
    // ログ（ファイルと標準エラー）とパニックのフック（SYS-07）。Windows の配布版は標準エラーがないので、
    // ログはファイルにだけ残る。
    let log_status = logs::init();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            let config_dir = app.path().app_config_dir()?;
            let sink: state::EventSink = Arc::new(move |event| {
                if let Err(e) = handle.emit(CORE_EVENT, event) {
                    tracing::debug!(error = %e, "イベントを画面に送れない");
                }
            });
            app.manage(Arc::new(
                AppState::new(config_dir, sink).with_logs(logs::AppLogs::new(log_status)),
            ));
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
            commands::before_frame,
            commands::before_region_frame,
            commands::view_region_frame,
            commands::mask_overlay_frame,
            commands::set_display_target,
            commands::list_catalog_backups,
            commands::restore_catalog,
            commands::update_app_settings,
            commands::save_import_preferences,
            commands::locate_tools,
            commands::path_kinds,
            commands::reveal_variant_file,
            commands::open_variant_file,
            commands::quit_app,
            commands::ack_quit_request,
            commands::log_folder_info,
            commands::open_log_folder,
            commands::clear_logs,
            commands::report_ui_error,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && !QUITTING.load(Ordering::SeqCst)
            {
                api.prevent_close();
                request_quit(window.app_handle());
            }
        });
    // macOS だけ、Tauri の既定のメニューの代わりに app_menu を付ける（「終了」を request_quit に通すため）。
    // Windows・Linux は今までどおりメニューを付けない。
    #[cfg(target_os = "macos")]
    let builder = builder.menu(app_menu).on_menu_event(|app, event| {
        if event.id() == QUIT_MENU_ID && !QUITTING.load(Ordering::SeqCst) {
            request_quit(app);
        }
    });
    let app = builder
        .build(tauri::generate_context!())
        .expect("GenzoParis を起動できません");
    app.run(|handle, event| match event {
        RunEvent::ExitRequested { api, .. } if !QUITTING.load(Ordering::SeqCst) => {
            api.prevent_exit();
            request_quit(handle);
        }
        RunEvent::Exit => close_at_exit(handle),
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_requests_are_numbered_and_acknowledged_in_order() {
        let q = QuitRequests::new();
        let first = q.next();
        let second = q.next();
        assert_eq!((first, second), (1, 2));
        assert!(!q.acked(first));
        // 新しい要求に答えたら、前の要求も受け取ったとみなす。
        q.ack(second);
        assert!(q.acked(first) && q.acked(second));
        // 古い番号の答えが後から来ても戻らない。
        q.ack(first);
        assert!(q.acked(second));
        assert!(!q.acked(q.next()));
    }
}
