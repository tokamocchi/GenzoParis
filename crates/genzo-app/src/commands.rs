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
//! [`region_frame_bytes`]・[`before_frame_bytes`]・[`before_region_frame_bytes`]・
//! [`view_region_frame_bytes`]・[`mask_overlay_frame_bytes`]）は別の関数にして、結合テスト（`tests/app`）から
//! [`AppState`] で直接呼ぶ。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use genzo_api::logging::{ClearReport, CrashReportSummary, LogFolderInfo, LogStatus};
use genzo_api::{
    BackupEntry, Command, DisplayInfo, ErrorInfo, ErrorKind, GpuMode, Response, VariantId,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::catalog_file::{self, CatalogFileState};
use crate::frames;
use crate::licenses::{self, LicenseDocuments};
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
    /// 既定のカタログの場所が OneDrive で同期されるフォルダの下か（Windows だけ。ほかの OS では常に
    /// `false`。[`under_onedrive`]）。画面は、同期しない場所に作るよう注意を出す（既定の場所（決定事項ログ
    /// No.63）を変えるかは人が決めるので、注意を出すだけにする）。
    pub default_catalog_in_onedrive: bool,
    /// 開いているカタログ。
    pub catalog: Option<CatalogStatus>,
    /// 今の設定で見つかる ffmpeg・ffprobe。
    pub tools: ExternalTools,
    /// ログ（ログのフォルダ・今のファイル・初期化の問題。SYS-07）。
    pub logs: LogStatus,
    /// 前回までに保存したアプリのクラッシュレポートのうち、まだ知らせていないもの（新しい順。起動して最初の
    /// 1 回だけ。画面は知らせのダイアログを出す。送信はしない）。
    pub crash_reports: Vec<CrashReportSummary>,
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

/// OneDrive の同期フォルダを示す環境変数（個人用・職場や学校用。OneDrive のクライアントが設定する）。
const ONEDRIVE_VARS: [&str; 3] = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"];

/// OneDrive の同期フォルダ（[`ONEDRIVE_VARS`] の値）。Windows 以外では空（ほかの OS で同じ名前の環境変数が
/// あっても使わない）。
fn onedrive_roots() -> Vec<std::ffi::OsString> {
    if !cfg!(windows) {
        return Vec::new();
    }
    ONEDRIVE_VARS.iter().filter_map(std::env::var_os).collect()
}

/// `path` が OneDrive の同期フォルダ（`roots`。環境変数 OneDrive・OneDriveConsumer・OneDriveCommercial
/// の値）のどれかの下か（そのフォルダ自身を含む）。
///
/// Windows のパスとして比べる: 大文字・小文字を区別せず、区切り（`\`・`/`）の違いと末尾の区切りを無視し、
/// フォルダの単位で比べる（`C:\Users\a\OneDrive` は `C:\Users\a\OneDriveBackup` の親ではない）。空の値は
/// 無視する。環境変数は読まず、値を引数で受ける（単体テストのため）。
///
/// 「PC フォルダーのバックアップ」が有効だと、ピクチャ・ドキュメント（既定のカタログの場所。
/// `default_catalog`）が OneDrive の下に移り、カタログ（SQLite の WAL）と標準プレビューのキャッシュ（既定で
/// 最大 20 GB。genzo-catalog の `DEFAULT_PREVIEW_CAPACITY_BYTES`）が同期の対象になる。
pub fn under_onedrive(path: &Path, roots: &[std::ffi::OsString]) -> bool {
    fn normalize(p: &str) -> String {
        p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
    }
    let path = normalize(&path.to_string_lossy());
    roots.iter().any(|root| {
        let root = normalize(&root.to_string_lossy());
        !root.is_empty()
            && path
                .strip_prefix(&root)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('\\'))
    })
}

/// アプリの状態。
#[tauri::command]
pub async fn app_status(app: AppHandle, state: Shared<'_>) -> Result<AppStatus, ErrorInfo> {
    let state = Arc::clone(&state);
    let default_catalog = default_catalog(&app);
    let default_catalog_in_onedrive = default_catalog
        .as_deref()
        .is_some_and(|p| under_onedrive(p, &onedrive_roots()));
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
            logs: state.logs().status().clone(),
            crash_reports: state.logs().take_crash_reports(),
            catalog: state.status()?,
            settings,
            default_catalog,
            default_catalog_in_onedrive,
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

/// ビフォー / アフターの表示（DEV-31）のビフォーの最新のプレビューを [`frames`] の形のバイト列にする
/// （[`before_frame`] の本体。ビフォーを設定していない・まだなければ空）。ヘッダーは `FrameInfo`。
pub fn before_frame_bytes(state: &AppState) -> Result<Vec<u8>, ErrorInfo> {
    let core = state.core()?;
    match core.before_frame() {
        Some(f) => frames::encode(&f.info, f.info.width, f.info.height, &f.display_rgb8)
            .ok_or_else(|| {
                error(
                    ErrorKind::Internal,
                    "ビフォーの画素の長さが合いません",
                    None,
                )
            }),
        None => Ok(Vec::new()),
    }
}

/// ビフォーの表示範囲の描画の最新の結果を [`frames`] の形のバイト列にする（[`before_region_frame`] の本体。
/// まだなければ空）。ヘッダーは `RegionInfo`（`before` が真）。
pub fn before_region_frame_bytes(state: &AppState) -> Result<Vec<u8>, ErrorInfo> {
    let core = state.core()?;
    match core.before_region_frame() {
        Some(f) => frames::encode(&f.info, f.info.width, f.info.height, &f.display_rgb8)
            .ok_or_else(|| {
                error(
                    ErrorKind::Internal,
                    "ビフォーの表示範囲の画素の長さが合いません",
                    None,
                )
            }),
        None => Ok(Vec::new()),
    }
}

/// 比較表示（LIB-03）の面 `pane` の表示範囲の描画の最新の結果を [`frames`] の形のバイト列にする
/// （[`view_region_frame`] の本体。まだない・面が範囲の外なら空）。ヘッダーは `RegionInfo`（`pane` が入る）。
pub fn view_region_frame_bytes(state: &AppState, pane: u32) -> Result<Vec<u8>, ErrorInfo> {
    let core = state.core()?;
    match core.view_region_frame(pane) {
        Some(f) => frames::encode(&f.info, f.info.width, f.info.height, &f.display_rgb8)
            .ok_or_else(|| {
                error(
                    ErrorKind::Internal,
                    "比較表示の表示範囲の画素の長さが合いません",
                    None,
                )
            }),
        None => Ok(Vec::new()),
    }
}

/// 現像中の写真のマスク `index` の重み（DEV-18・DEV-19 のマスクの重ねて表示）を [`frames`] の形のバイト列に
/// する（[`mask_overlay_frame`] の本体）。ヘッダーは `MaskOverlayInfo`、画素は重み（0〜255）を R・G・B に
/// 同じ値で入れる（プレビューと同じ格子。`uncropped` は `pick_white_balance` と同じ）。
pub fn mask_overlay_frame_bytes(
    state: &AppState,
    variant_id: VariantId,
    index: usize,
    uncropped: Option<bool>,
) -> Result<Vec<u8>, ErrorInfo> {
    let core = state.core()?;
    let o = core
        .mask_overlay(variant_id, index, uncropped)
        .map_err(|e| e.info())?;
    let rgb: Vec<u8> = o.weights.iter().flat_map(|&w| [w, w, w]).collect();
    frames::encode(&o.info, o.info.width, o.info.height, &rgb).ok_or_else(|| {
        error(
            ErrorKind::Internal,
            "マスクの重ねて表示の画素の長さが合いません",
            None,
        )
    })
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

/// ビフォーの最新のプレビュー（[`frames`] の形のバイト列。まだなければ空。DEV-31）。
#[tauri::command]
pub async fn before_frame(state: Shared<'_>) -> Result<tauri::ipc::Response, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || before_frame_bytes(&state).map(tauri::ipc::Response::new)).await
}

/// ビフォーの表示範囲の描画の最新の結果（[`frames`] の形のバイト列。まだなければ空。DEV-31）。
#[tauri::command]
pub async fn before_region_frame(state: Shared<'_>) -> Result<tauri::ipc::Response, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || before_region_frame_bytes(&state).map(tauri::ipc::Response::new)).await
}

/// 比較表示（LIB-03）の面 `pane` の表示範囲の描画の最新の結果（[`frames`] の形のバイト列。まだなければ空）。
#[tauri::command]
pub async fn view_region_frame(
    state: Shared<'_>,
    pane: u32,
) -> Result<tauri::ipc::Response, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || view_region_frame_bytes(&state, pane).map(tauri::ipc::Response::new)).await
}

/// 現像中の写真のマスク `index` の重み（[`frames`] の形のバイト列。DEV-18・DEV-19）。
#[tauri::command]
pub async fn mask_overlay_frame(
    state: Shared<'_>,
    variant_id: VariantId,
    index: usize,
    uncropped: Option<bool>,
) -> Result<tauri::ipc::Response, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || {
        mask_overlay_frame_bytes(&state, variant_id, index, uncropped)
            .map(tauri::ipc::Response::new)
    })
    .await
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

/// 取り込みのダイアログの設定を保存する（取り込みを始めたとき。次に開いたときの初めの値にする）。
#[tauri::command]
pub async fn save_import_preferences(
    state: Shared<'_>,
    preferences: crate::settings::ImportPreferences,
) -> Result<AppSettings, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || {
        state.update_settings(|s| {
            s.import = preferences;
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

/// OS の既定のアプリで開いてよいファイルか（取り込む対象の拡張子の写真・動画だけ）。
///
/// カタログに記録されたパスは、ほかの人から受け取ったカタログでは任意の値になりうる。OS の既定の動作は、
/// 実行ファイル（`.exe`・`.scr`・`.bat`・`.lnk`・`.app` など）を開かずに実行するので、取り込む対象の
/// 拡張子でなければ開かない。
fn openable_media(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(genzo_api::media_kind)
        .is_some()
        && !path.is_dir()
}

/// 元ファイルを OS の既定のアプリで開く（アプリの中で再生できない動画を OS のプレイヤーで見るなど。アプリの中の再生
/// （VID-04）は `genzo://video` と画面の `video` 要素。protocol.rs）。
/// 取り込む対象の拡張子の写真・動画だけを開く（`openable_media`）。
#[tauri::command]
pub async fn open_variant_file(
    app: AppHandle,
    state: Shared<'_>,
    variant_id: VariantId,
) -> Result<(), ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || {
        let path = primary_path(&state, variant_id)?;
        if !openable_media(&path) {
            return Err(error(
                ErrorKind::InvalidInput,
                format!(
                    "{} は写真・動画の拡張子ではないため、開きません",
                    path.display()
                ),
                None,
            ));
        }
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

/// ログのフォルダの中身（ログのファイル・クラッシュレポート・合計の大きさ。SYS-07）。
#[tauri::command]
pub async fn log_folder_info(state: Shared<'_>) -> Result<LogFolderInfo, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || state.logs().folder_info()).await
}

/// ログのフォルダを OS のファイル管理（エクスプローラー・Finder）で開く（なければ作ってから）。
#[tauri::command]
pub async fn open_log_folder(app: AppHandle, state: Shared<'_>) -> Result<(), ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || {
        let dir = state.logs().ensure_dir()?;
        app.opener()
            .open_path(dir.to_string_lossy(), None::<&str>)
            .map_err(|e| {
                error(
                    ErrorKind::FileAccess,
                    format!("ログのフォルダ {} を開けません: {e}", dir.display()),
                    None,
                )
            })
    })
    .await
}

/// 同梱したライセンスの文書のフォルダ（資源のフォルダの `licenses`。なければ `None`）。
fn licenses_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .resource_dir()
        .ok()
        .map(|d| d.join(licenses::LICENSES_DIR))
        .filter(|d| d.is_dir())
}

/// 同梱したライセンスの文書の一覧（DIST-05。設定の「このアプリについて」のタブ）。
#[tauri::command]
pub async fn license_documents(app: AppHandle) -> Result<LicenseDocuments, ErrorInfo> {
    blocking(move || {
        let Some(dir) = licenses_dir(&app) else {
            return Ok(LicenseDocuments {
                dir: None,
                documents: Vec::new(),
            });
        };
        let documents = licenses::list_documents(&dir).map_err(|e| {
            error(
                ErrorKind::FileAccess,
                format!(
                    "ライセンスの文書のフォルダ {} を読めません: {e}",
                    dir.display()
                ),
                None,
            )
        })?;
        Ok(LicenseDocuments {
            dir: Some(dir),
            documents,
        })
    })
    .await
}

/// 同梱したライセンスの文書を読む（`path` は [`license_documents`] の一覧にあるものだけ）。
#[tauri::command]
pub async fn read_license_document(app: AppHandle, path: String) -> Result<String, ErrorInfo> {
    blocking(move || {
        let dir = licenses_dir(&app).ok_or_else(|| {
            error(
                ErrorKind::NotFound,
                "ライセンスの文書のフォルダがありません",
                None,
            )
        })?;
        licenses::read_document(&dir, &path)
    })
    .await
}

/// 同梱したライセンスの文書のフォルダを OS のファイル管理（エクスプローラー・Finder）で開く。
#[tauri::command]
pub async fn open_license_folder(app: AppHandle) -> Result<(), ErrorInfo> {
    blocking(move || {
        let dir = licenses_dir(&app).ok_or_else(|| {
            error(
                ErrorKind::NotFound,
                "ライセンスの文書のフォルダがありません",
                None,
            )
        })?;
        app.opener()
            .open_path(dir.to_string_lossy(), None::<&str>)
            .map_err(|e| {
                error(
                    ErrorKind::FileAccess,
                    format!(
                        "ライセンスの文書のフォルダ {} を開けません: {e}",
                        dir.display()
                    ),
                    None,
                )
            })
    })
    .await
}

/// ログとクラッシュレポートを消す（書いている途中のログは空にする）。
#[tauri::command]
pub async fn clear_logs(state: Shared<'_>) -> Result<ClearReport, ErrorInfo> {
    let state = Arc::clone(&state);
    blocking(move || state.logs().clear()).await
}

/// 画面のエラー（JavaScript の例外・処理されなかった Promise の失敗）をアプリのログに残す（1 回の起動で
/// 件数の上限まで。[`crate::logs::AppLogs::record_ui_error`]）。
#[tauri::command]
pub fn report_ui_error(state: Shared<'_>, message: String, detail: Option<String>) {
    state.logs().record_ui_error(&message, detail.as_deref());
}

/// 画面が終了の要求（lib.rs の `QUIT_REQUESTED_EVENT`。値は要求の番号）を受け取ったと答える。答えがあれば、
/// アプリは画面を待たずに閉じるのをやめ、画面が送信待ちの設定を送ってから呼ぶ `quit_app` を待つ。
#[tauri::command]
pub fn ack_quit_request(id: u64) {
    crate::QUIT_REQUESTS.ack(id);
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
    fn default_catalog_under_onedrive_is_detected_by_folder() {
        let roots = |v: &[&str]| v.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let personal = roots(&[r"C:\Users\taro\OneDrive"]);
        let catalog = Path::new(r"C:\Users\taro\OneDrive\ピクチャ\GenzoParis\catalog.db");
        assert!(under_onedrive(catalog, &personal));
        // 大文字・小文字、区切り（/）、末尾の区切りの違いは無視する。
        assert!(under_onedrive(
            Path::new(r"c:\users\TARO\onedrive\Pictures\GenzoParis\catalog.db"),
            &roots(&[r"C:/Users/taro/OneDrive/"]),
        ));
        assert!(under_onedrive(
            Path::new("C:/Users/taro/OneDrive"),
            &personal
        ));
        // フォルダの単位で比べる（名前の先頭が同じだけの別のフォルダは下ではない）。
        assert!(!under_onedrive(
            Path::new(r"C:\Users\taro\OneDriveBackup\GenzoParis\catalog.db"),
            &personal
        ));
        assert!(!under_onedrive(
            Path::new(r"C:\Users\taro\Pictures\GenzoParis\catalog.db"),
            &personal
        ));
        // 環境変数が複数ある（個人用と職場や学校用）ときは、どれかの下なら下。空の値は無視する（空を
        // 比べると、何でも下になってしまう）。
        let both = roots(&["", r"C:\Users\taro\OneDrive", r"D:\OneDrive - 会社"]);
        assert!(under_onedrive(
            Path::new(r"D:\OneDrive - 会社\画像\GenzoParis\catalog.db"),
            &both
        ));
        assert!(!under_onedrive(
            Path::new(r"D:\写真\GenzoParis\catalog.db"),
            &both
        ));
        assert!(!under_onedrive(catalog, &roots(&[""])));
        assert!(!under_onedrive(catalog, &[]));
    }

    #[test]
    fn onedrive_folders_are_only_looked_up_on_windows() {
        // Windows 以外では、同じ名前の環境変数があっても使わない（既定の場所は OneDrive の下とみなさない）。
        if !cfg!(windows) {
            assert!(onedrive_roots().is_empty());
        }
    }

    #[test]
    fn only_media_files_are_opened_with_the_default_app() {
        assert!(openable_media(Path::new("/p/IMG_0001.ARW")));
        assert!(openable_media(Path::new("/p/動画 01.MP4")));
        for bad in [
            "/p/IMG_0001.scr",
            "/p/x.exe",
            "/p/x.bat",
            "/p/x.lnk",
            "/p/x.url",
            "/p/x.app",
            "/p/x",
        ] {
            assert!(!openable_media(Path::new(bad)), "{bad}");
        }
        // 拡張子が写真でもフォルダなら開かない（macOS の .app のようなバンドルを含む）。
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("偽物.jpg");
        std::fs::create_dir(&dir).unwrap();
        assert!(!openable_media(&dir));
    }

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
