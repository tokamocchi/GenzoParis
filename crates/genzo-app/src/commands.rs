//! Tauri のコマンド（画面 → アプリ）。
//!
//! コアの操作は、汎用のコマンド [`core_execute`]（genzo-api の `Command` の JSON をそのまま渡す。04 の
//! 1.5 節）で行う。アプリ固有の操作（カタログを開く・閉じる・復元する、画素のバイナリの受け渡し、
//! アプリの設定、ファイルを OS で表示する）は個別のコマンドにする。
//!
//! コアの関数はブロックしうるので、すべて `spawn_blocking` で呼ぶ（UI のスレッドを止めない。
//! 決定事項ログ No.36）。失敗は genzo-api の `ErrorInfo`（JSON）で返す。
//!
//! Tauri の状態（`State`）を使わない処理の本体（[`execute_command`]・[`develop_frame_bytes`]・
//! [`region_frame_bytes`]）は別の関数にして、結合テスト（`tests/app`）から [`AppState`] で直接呼ぶ。

use std::path::PathBuf;
use std::sync::Arc;

use genzo_api::{
    BackupEntry, Command, DisplayInfo, ErrorInfo, ErrorKind, GpuMode, Response, VariantId,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::catalog_file::{self, CatalogFileState};
use crate::frames;
use crate::settings::AppSettings;
use crate::state::{AppState, CatalogStatus, DisplayTarget, RestoreOutcome, error};
use crate::tools::{self, ExternalTools};

type Shared<'a> = State<'a, Arc<AppState>>;

/// ブロックする処理を別のスレッドで実行する。
async fn blocking<T, F>(f: F) -> Result<T, ErrorInfo>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ErrorInfo> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| {
        error(
            ErrorKind::Internal,
            format!("処理が異常終了しました: {e}"),
            None,
        )
    })?
}

/// アプリの状態（起動時に画面が最初に読む）。
#[derive(Debug, Clone, Serialize)]
pub struct AppStatus {
    /// アプリの版。
    pub version: String,
    /// OS（`windows` / `macos` / `linux`）。
    pub platform: &'static str,
    /// アプリの設定。
    pub settings: AppSettings,
    /// 起動時の知らせ（設定のファイルが壊れていたなど）。
    pub notice: Option<String>,
    /// 既定のカタログの場所（初回の起動で案内する）。
    pub default_catalog: Option<PathBuf>,
    /// 開いているカタログ。
    pub catalog: Option<CatalogStatus>,
    /// 今の設定で見つかる ffmpeg・ffprobe。
    pub tools: ExternalTools,
}

fn default_catalog(app: &AppHandle) -> Option<PathBuf> {
    let p = app.path();
    p.picture_dir()
        .or_else(|_| p.document_dir())
        .or_else(|_| p.app_data_dir())
        .ok()
        .map(|d| {
            d.join(catalog_file::DEFAULT_CATALOG_DIR)
                .join(catalog_file::DEFAULT_CATALOG_FILE)
        })
}

/// アプリの状態。
#[tauri::command]
pub async fn app_status(app: AppHandle, state: Shared<'_>) -> Result<AppStatus, ErrorInfo> {
    let state = Arc::clone(&state);
    let default_catalog = default_catalog(&app);
    blocking(move || {
        let settings = state.settings();
        let tools = tools::locate(settings.ffmpeg_dir.as_deref());
        Ok(AppStatus {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: if cfg!(windows) {
                "windows"
            } else if cfg!(target_os = "macos") {
                "macos"
            } else {
                "linux"
            },
            notice: state.take_settings_notice(),
            catalog: state.status()?,
            settings,
            default_catalog,
            tools,
        })
    })
    .await
}

/// パスがカタログか（開く・作る前の確認。[`AppState::inspect_catalog`]。先頭が壊れたカタログも、開く処理と
/// 同じくカタログとして返し、開く処理の catalog_corrupt で復元を案内する）。
#[tauri::command]
pub async fn inspect_catalog_path(
    state: Shared<'_>,
    path: PathBuf,
) -> Result<CatalogFileState, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || state.inspect_catalog(&path)).await
}

/// カタログを開く（`create` なら、なければ作る）。
#[tauri::command]
pub async fn open_catalog(
    state: Shared<'_>,
    path: PathBuf,
    create: bool,
) -> Result<CatalogStatus, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || state.open_catalog(&path, create)).await
}

/// 閉じられなかったときの応答。
#[derive(Debug, Clone, Serialize)]
pub struct CloseFailure {
    /// エラー。
    pub error: ErrorInfo,
    /// カタログが開いたままか（保存待ちの現像設定を保存できなかった。再試行か、捨てて閉じるかを選ぶ）。
    pub still_open: bool,
}

/// カタログを閉じる。
#[tauri::command]
pub async fn close_catalog(state: Shared<'_>, discard_unsaved: bool) -> Result<(), CloseFailure> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        state
            .close_catalog(discard_unsaved)
            .map_err(|e| CloseFailure {
                error: e.info(),
                still_open: e.still_open(),
            })
    })
    .await
    .unwrap_or_else(|e| {
        Err(CloseFailure {
            error: error(
                ErrorKind::Internal,
                format!("処理が異常終了しました: {e}"),
                None,
            ),
            still_open: true,
        })
    })
}

/// コアのコマンド（genzo-api の `Command` の JSON）を、開いているカタログのコアで実行する
/// （[`core_execute`] の本体）。
pub fn execute_command(
    state: &AppState,
    command: serde_json::Value,
) -> Result<Response, ErrorInfo> {
    let command: Command = serde_json::from_value(command).map_err(|e| {
        error(
            ErrorKind::InvalidInput,
            format!("コマンドを解釈できません: {e}"),
            None,
        )
    })?;
    state.core()?.execute(command).map_err(|e| e.info())
}

/// コアのコマンドを実行する（genzo-api の `Command` の JSON）。
#[tauri::command]
pub async fn core_execute(
    state: Shared<'_>,
    command: serde_json::Value,
) -> Result<Response, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || execute_command(&state, command)).await
}

/// 現像中の写真の最新のプレビューを [`frames`] の形のバイト列にする（[`develop_frame`] の本体。まだ
/// なければ空）。
pub fn develop_frame_bytes(state: &AppState) -> Result<Vec<u8>, ErrorInfo> {
    let core = state.core()?;
    match core.develop_frame() {
        Some(f) => frames::encode(&f.info, f.info.width, f.info.height, &f.display_rgb8)
            .ok_or_else(|| {
                error(
                    ErrorKind::Internal,
                    "プレビューの画素の長さが合いません",
                    None,
                )
            }),
        None => Ok(Vec::new()),
    }
}

/// 表示範囲の描画の最新の結果を [`frames`] の形のバイト列にする（[`region_frame`] の本体。まだなければ
/// 空）。
pub fn region_frame_bytes(state: &AppState) -> Result<Vec<u8>, ErrorInfo> {
    let core = state.core()?;
    match core.region_frame() {
        Some(f) => frames::encode(&f.info, f.info.width, f.info.height, &f.display_rgb8)
            .ok_or_else(|| {
                error(
                    ErrorKind::Internal,
                    "表示範囲の画素の長さが合いません",
                    None,
                )
            }),
        None => Ok(Vec::new()),
    }
}

/// 現像中の写真の最新のプレビュー（[`frames`] の形のバイト列。まだなければ空）。
#[tauri::command]
pub async fn develop_frame(state: Shared<'_>) -> Result<tauri::ipc::Response, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || develop_frame_bytes(&state).map(tauri::ipc::Response::new)).await
}

/// 表示範囲の描画の最新の結果（[`frames`] の形のバイト列。まだなければ空）。
#[tauri::command]
pub async fn region_frame(state: Shared<'_>) -> Result<tauri::ipc::Response, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || region_frame_bytes(&state).map(tauri::ipc::Response::new)).await
}

/// 画面の色空間（canvas が Display P3 を使えるか）。
#[tauri::command]
pub async fn set_display_target(
    state: Shared<'_>,
    target: DisplayTarget,
) -> Result<Option<DisplayInfo>, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || state.set_display_target(target)).await
}

/// カタログのバックアップの一覧（カタログが開けなくても使える）。
#[tauri::command]
pub async fn list_catalog_backups(
    state: Shared<'_>,
    catalog_path: PathBuf,
) -> Result<Vec<BackupEntry>, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || state.list_backups(&catalog_path)).await
}

/// バックアップから復元して開き直す。
#[tauri::command]
pub async fn restore_catalog(
    state: Shared<'_>,
    catalog_path: PathBuf,
    backup: PathBuf,
) -> Result<RestoreOutcome, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || state.restore(&catalog_path, &backup)).await
}

/// アプリの設定のうち、画面から変えられるもの（次にカタログを開いたときから使う）。
#[derive(Debug, Clone, Deserialize)]
pub struct AppSettingsUpdate {
    /// ffmpeg・ffprobe のフォルダ（`null` なら自動で探す）。
    pub ffmpeg_dir: Option<PathBuf>,
    /// GPU の使用。
    pub gpu: GpuMode,
    /// 終了時に毎回バックアップするか。
    pub backup_on_close: bool,
}

/// アプリの設定を変える。
#[tauri::command]
pub async fn update_app_settings(
    state: Shared<'_>,
    update: AppSettingsUpdate,
) -> Result<AppSettings, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || {
        state.update_settings(|s| {
            s.ffmpeg_dir = update.ffmpeg_dir.filter(|p| !p.as_os_str().is_empty());
            s.gpu = update.gpu;
            s.backup_on_close = update.backup_on_close;
        })
    })
    .await
}

/// 指定したフォルダ（なければ自動）で ffmpeg・ffprobe を探す（設定の画面の確認用）。
#[tauri::command]
pub async fn locate_tools(dir: Option<PathBuf>) -> Result<ExternalTools, ErrorInfo> {
    blocking(move || Ok(tools::locate(dir.as_deref()))).await
}

/// パスの種類（ドロップしたものがフォルダかの判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    /// フォルダ（シンボリックリンクの先がフォルダのものを含む）。
    Dir,
    /// フォルダ以外（ファイルなど）。
    File,
    /// ない・調べられない。
    Missing,
}

/// パスの種類を調べる（シンボリックリンクはたどる）。
pub fn path_kind(path: &std::path::Path) -> PathKind {
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => PathKind::Dir,
        Ok(_) => PathKind::File,
        Err(_) => PathKind::Missing,
    }
}

/// パスの種類をまとめて調べる（ウィンドウへドロップしたものからフォルダだけを取り込むため）。
#[tauri::command]
pub async fn path_kinds(paths: Vec<PathBuf>) -> Result<Vec<PathKind>, ErrorInfo> {
    blocking(move || Ok(paths.iter().map(|p| path_kind(p)).collect())).await
}

/// variant の元ファイル（主なファイル）のパス。
fn primary_path(state: &AppState, variant_id: VariantId) -> Result<PathBuf, ErrorInfo> {
    let details = state
        .core()?
        .variant_details(variant_id)
        .map_err(|e| e.info())?;
    details
        .files
        .iter()
        .find(|f| f.role == genzo_model::FileRole::Primary)
        .or(details.files.first())
        .and_then(|f| f.path.clone())
        .ok_or_else(|| {
            error(
                ErrorKind::NotFound,
                "元ファイルの場所が分かりません（ドライブが接続されていない可能性があります）",
                None,
            )
        })
}

/// 元ファイルを OS のファイル管理（エクスプローラー・Finder）で表示する。
#[tauri::command]
pub async fn reveal_variant_file(
    app: AppHandle,
    state: Shared<'_>,
    variant_id: VariantId,
) -> Result<(), ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || {
        let path = primary_path(&state, variant_id)?;
        app.opener().reveal_item_in_dir(&path).map_err(|e| {
            error(
                ErrorKind::FileAccess,
                format!("{} を表示できません: {e}", path.display()),
                Some("ファイルの場所・ドライブの接続を確かめてください"),
            )
        })
    })
    .await
}

/// 元ファイルを OS の既定のアプリで開く（動画の再生など。VID-04 は v1 のため OS のアプリを使う）。
#[tauri::command]
pub async fn open_variant_file(
    app: AppHandle,
    state: Shared<'_>,
    variant_id: VariantId,
) -> Result<(), ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || {
        let path = primary_path(&state, variant_id)?;
        app.opener()
            .open_path(path.to_string_lossy(), None::<&str>)
            .map_err(|e| {
                error(
                    ErrorKind::FileAccess,
                    format!("{} を開けません: {e}", path.display()),
                    Some("ファイルの種類に対応するアプリが入っているか確かめてください"),
                )
            })
    })
    .await
}

/// アプリを終了する（カタログを閉じてから。`discard_unsaved` なら保存できなかった設定を捨てる）。
#[tauri::command]
pub async fn quit_app(
    app: AppHandle,
    state: Shared<'_>,
    discard_unsaved: bool,
) -> Result<(), CloseFailure> {
    match close_catalog(state, discard_unsaved).await {
        Ok(()) => {}
        // 開いたまま（保存できなかった）なら終わらない（画面が再試行か破棄かを選ばせる）。
        Err(f) if f.still_open => return Err(f),
        // 閉じた後の段階のエラー（ワーカーの終了など）は、記録して終わる（lib.rs の終了の要求と同じ）。
        Err(f) => tracing::warn!(error = ?f.error, "カタログを閉じる処理でエラー"),
    }
    crate::QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_kinds_tell_folders_from_files() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("a.jpg");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(path_kind(d.path()), PathKind::Dir);
        assert_eq!(path_kind(&file), PathKind::File);
        assert_eq!(path_kind(&d.path().join("none")), PathKind::Missing);
        // 拡張子のようなものが付いたフォルダもフォルダ（名前では判断しない）。
        let dotted = d.path().join("2024.05.01 京都");
        std::fs::create_dir(&dotted).unwrap();
        assert_eq!(path_kind(&dotted), PathKind::Dir);
    }
}
