//! アプリの状態: 開いているカタログ（[`Session`]）と、アプリの設定。
//!
//! Tauri に依存しない形にして、単体テストでカタログを開く・閉じる・復元するまでを確かめられるようにする
//! （イベントの送り先は [`EventSink`] で差し替える）。
//!
//! # カタログを開く・閉じる
//!
//! - 1 つのアプリで開くカタログは 1 つ（genzo-api のロックファイルで、ほかのアプリ・CLI と同時に開くことも
//!   防ぐ）。別のカタログを開くときは、今のカタログを閉じてから開く。
//! - 閉じるときに保存待ちの現像設定を保存できなかった場合（ディスクの空き容量の不足など）は、閉じずに
//!   エラーを返す（[`CloseError::SaveFailed`]）。画面は「再試行」と「保存せずに閉じる」を選ばせる
//!   （genzo-api の `Core::close` の doc。決定事項ログ No.61 の閉じ方）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use genzo_api::{
    ApiError, BackupEntry, Capabilities, CatalogStats, Core, CoreConfig, CoreSettings, DisplayInfo,
    ErrorInfo, ErrorKind, Event, StartupReport, WorkerLaunch,
};
use genzo_color::icc::{IccProfile, StandardProfile};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::catalog_file::{self, CatalogFileState};
use crate::settings::{self, AppSettings};
use crate::tools::{self, ExternalTools};

/// コアのイベントの送り先（Tauri では WebView へのイベント）。
pub type EventSink = Arc<dyn Fn(&Event) + Send + Sync>;

/// 画面（WebView）に渡す画素の色空間。
///
/// コアはこの色空間で 8bit の画素を作り（ステージ 17a）、WebView の canvas に同じ色空間で描く。
/// モニターへの変換は WebView（OS の色管理）が行う（決定事項ログ No.60）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayTarget {
    /// Display P3（canvas の `colorSpace: "display-p3"` が使える WebView）。
    DisplayP3,
    /// sRGB（P3 の canvas が使えない WebView）。
    #[default]
    Srgb,
}

impl DisplayTarget {
    fn icc(self) -> Result<Vec<u8>, ErrorInfo> {
        let kind = match self {
            Self::DisplayP3 => StandardProfile::DisplayP3,
            Self::Srgb => StandardProfile::Srgb,
        };
        IccProfile::standard(kind)
            .map(|p| p.as_bytes().to_vec())
            .map_err(|e| internal(format!("画面用のプロファイルを作れません: {e}")))
    }
}

/// 開いているカタログ。
pub struct Session {
    /// コア。
    pub core: Arc<Core>,
    /// カタログのファイル。
    pub catalog_path: PathBuf,
    /// データのフォルダ。
    pub data_dir: PathBuf,
    /// 見つけた ffmpeg・ffprobe。
    pub tools: ExternalTools,
}

/// 開いたカタログの情報（画面に返す）。
#[derive(Debug, Clone, Serialize)]
pub struct CatalogStatus {
    /// カタログのファイル。
    pub catalog_path: PathBuf,
    /// データのフォルダ。
    pub data_dir: PathBuf,
    /// 起動時の確認の結果。
    pub startup: StartupReport,
    /// 起動時の警告（前回の異常終了など）。
    pub warnings: Vec<Event>,
    /// 件数とスキーマの版。
    pub stats: CatalogStats,
    /// RAW のデコーダ・GPU など（ワーカーを起動できなければ `None` で、`worker_error` に理由）。
    pub capabilities: Option<Capabilities>,
    /// ワーカーを起動できなかった理由。
    pub worker_error: Option<ErrorInfo>,
    /// 画面の色空間。
    pub display: DisplayInfo,
    /// 見つけた ffmpeg・ffprobe。
    pub tools: ExternalTools,
    /// カタログに保存した設定。
    pub core_settings: CoreSettings,
}

/// 閉じられなかった理由。
#[derive(Debug)]
pub enum CloseError {
    /// 保存待ちの現像設定を保存できなかった（カタログは開いたまま）。
    SaveFailed(ErrorInfo),
    /// 閉じたが、終了の処理の途中でエラーがあった。
    ClosedWithError(ErrorInfo),
}

impl CloseError {
    /// 画面に返すエラー。
    pub fn info(&self) -> ErrorInfo {
        match self {
            Self::SaveFailed(e) => ErrorInfo {
                hint: Some(
                    "空き容量・カタログのフォルダの権限を確かめて再試行するか、保存していない現像設定を捨てて閉じてください"
                        .to_owned(),
                ),
                ..e.clone()
            },
            Self::ClosedWithError(e) => e.clone(),
        }
    }

    /// カタログが開いたままか。
    pub fn still_open(&self) -> bool {
        matches!(self, Self::SaveFailed(_))
    }
}

/// 復元の結果。
#[derive(Debug, Clone, Serialize)]
pub struct RestoreOutcome {
    /// 復元に使ったバックアップ。
    pub backup: PathBuf,
    /// 退避した（以前の）カタログのファイル。
    pub displaced: Vec<PathBuf>,
    /// 開き直したカタログ（開けなければ `None` で、`open_error` に理由）。
    pub status: Option<CatalogStatus>,
    /// 開き直せなかった理由。
    pub open_error: Option<ErrorInfo>,
}

/// アプリの状態。
pub struct AppState {
    config_dir: PathBuf,
    settings: Mutex<AppSettings>,
    settings_notice: Mutex<Option<String>>,
    session: RwLock<Option<Arc<Session>>>,
    /// 開く・閉じる・復元を 1 つずつにする。
    lifecycle: Mutex<()>,
    display: Mutex<DisplayTarget>,
    events: EventSink,
    /// ワーカーの起動方法（テストで差し替える。`None` なら既定の別の実行ファイル）。
    worker: Option<WorkerLaunch>,
}

/// エラーの情報を作る。
pub fn error(kind: ErrorKind, message: impl Into<String>, hint: Option<&str>) -> ErrorInfo {
    ErrorInfo {
        kind,
        message: message.into(),
        user_actionable: kind.user_actionable(),
        retryable: kind.retryable(),
        hint: hint.map(str::to_owned),
    }
}

fn internal(message: impl Into<String>) -> ErrorInfo {
    error(ErrorKind::Internal, message, None)
}

fn api(e: ApiError) -> ErrorInfo {
    e.info()
}

/// カタログが開いていないときのエラー。
pub fn no_catalog() -> ErrorInfo {
    error(
        ErrorKind::Closed,
        "カタログが開いていません",
        Some("カタログを開いてから操作してください"),
    )
}

impl AppState {
    /// 設定のフォルダ（`settings.json` を置く）から作る。
    pub fn new(config_dir: PathBuf, events: EventSink) -> Self {
        let loaded = settings::load(&config_dir);
        let notice = loaded.broken_backup.map(|p| {
            format!(
                "アプリの設定のファイルが壊れていたため、既定の設定で起動しました（壊れたファイルは {} に残しています）",
                p.display()
            )
        });
        Self {
            config_dir,
            settings: Mutex::new(loaded.settings),
            settings_notice: Mutex::new(notice),
            session: RwLock::new(None),
            lifecycle: Mutex::new(()),
            display: Mutex::new(DisplayTarget::default()),
            events,
            worker: None,
        }
    }

    /// ワーカーの起動方法を指定する（テスト用）。
    pub fn with_worker(mut self, worker: WorkerLaunch) -> Self {
        self.worker = Some(worker);
        self
    }

    /// 設定。
    pub fn settings(&self) -> AppSettings {
        self.settings.lock().clone()
    }

    /// 設定を変えて保存する。
    pub fn update_settings(
        &self,
        f: impl FnOnce(&mut AppSettings),
    ) -> Result<AppSettings, ErrorInfo> {
        let mut s = self.settings.lock();
        let mut next = s.clone();
        f(&mut next);
        settings::save(&self.config_dir, &next).map_err(|e| {
            error(
                ErrorKind::Storage,
                format!("アプリの設定を保存できません: {e}"),
                Some("設定のフォルダの空き容量と権限を確かめてください"),
            )
        })?;
        *s = next.clone();
        Ok(next)
    }

    /// 起動時の設定の読み込みの知らせ（1 回だけ返す）。
    pub fn take_settings_notice(&self) -> Option<String> {
        self.settings_notice.lock().take()
    }

    /// 開いているカタログ。
    pub fn session(&self) -> Option<Arc<Session>> {
        self.session.read().clone()
    }

    /// 開いているカタログのコア（なければエラー）。
    pub fn core(&self) -> Result<Arc<Core>, ErrorInfo> {
        self.session()
            .map(|s| Arc::clone(&s.core))
            .ok_or_else(no_catalog)
    }

    /// 画面の色空間を設定する（開いているカタログにもすぐ反映する）。
    pub fn set_display_target(
        &self,
        target: DisplayTarget,
    ) -> Result<Option<DisplayInfo>, ErrorInfo> {
        *self.display.lock() = target;
        match self.session() {
            Some(s) => Ok(Some(
                s.core
                    .set_display_profile(Some(&target.icc()?))
                    .map_err(api)?,
            )),
            None => Ok(None),
        }
    }

    /// 開いているカタログの情報。
    pub fn status(&self) -> Result<Option<CatalogStatus>, ErrorInfo> {
        match self.session() {
            Some(s) => Ok(Some(self.describe(&s)?)),
            None => Ok(None),
        }
    }

    fn describe(&self, s: &Session) -> Result<CatalogStatus, ErrorInfo> {
        let core = &s.core;
        let (capabilities, worker_error) = match core.capabilities() {
            Ok(c) => (Some(c), None),
            Err(e) => (None, Some(e.info())),
        };
        Ok(CatalogStatus {
            catalog_path: s.catalog_path.clone(),
            data_dir: s.data_dir.clone(),
            startup: core.startup_report(),
            warnings: core.startup_warnings(),
            stats: core.catalog_stats().map_err(api)?,
            capabilities,
            worker_error,
            display: core.display_info(),
            tools: s.tools.clone(),
            core_settings: core.settings(),
        })
    }

    /// カタログを開く。`create` なら、なければ作る（あれば、GenzoParis のカタログならそのまま開く）。
    pub fn open_catalog(&self, path: &Path, create: bool) -> Result<CatalogStatus, ErrorInfo> {
        let _guard = self.lifecycle.lock();
        self.open_locked(path, create)
    }

    fn open_locked(&self, path: &Path, create: bool) -> Result<CatalogStatus, ErrorInfo> {
        let path = absolute(path)?;
        let state = catalog_file::inspect(&path).map_err(|e| {
            error(
                ErrorKind::FileAccess,
                format!("{} を確かめられません: {e}", path.display()),
                Some("ファイルの場所・ドライブの接続・権限を確かめてください"),
            )
        })?;
        match state {
            CatalogFileState::NotCatalog => {
                return Err(error(
                    ErrorKind::InvalidInput,
                    format!("{} は GenzoParis のカタログではありません", path.display()),
                    Some("カタログ（.db）のファイルを選ぶか、新しいカタログを作ってください"),
                ));
            }
            CatalogFileState::Missing if !create => {
                return Err(error(
                    ErrorKind::NotFound,
                    format!("カタログ {} が見つかりません", path.display()),
                    Some("ドライブの接続を確かめるか、別のカタログを開いてください"),
                ));
            }
            CatalogFileState::Missing => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        error(
                            ErrorKind::FileAccess,
                            format!("フォルダ {} を作れません: {e}", parent.display()),
                            Some("別の場所を選ぶか、フォルダの権限を確かめてください"),
                        )
                    })?;
                }
            }
            CatalogFileState::Catalog => {}
        }
        // 同じカタログが開いていれば、そのまま返す。
        if let Some(s) = self.session()
            && s.catalog_path == path
        {
            return self.describe(&s);
        }
        self.close_locked(false).map_err(|e| e.info())?;

        let settings = self.settings();
        let tools = tools::locate(settings.ffmpeg_dir.as_deref());
        let data_dir = catalog_file::data_dir_for(&path);
        let mut config = CoreConfig::new(&path, &data_dir);
        if let Some(w) = &self.worker {
            config.worker = w.clone();
        }
        config.gpu = settings.gpu;
        config.backup_on_close = settings.backup_on_close;
        config.worker_env = tools.worker_env();
        let core = Arc::new(Core::open(config).map_err(api)?);
        let target = *self.display.lock();
        if let Err(e) = target
            .icc()
            .and_then(|icc| core.set_display_profile(Some(&icc)).map_err(api))
        {
            tracing::warn!(?e, "画面の色空間を設定できないので sRGB とみなす");
        }
        spawn_forwarder(&core, Arc::clone(&self.events));
        let session = Arc::new(Session {
            core,
            catalog_path: path.clone(),
            data_dir,
            tools,
        });
        *self.session.write() = Some(Arc::clone(&session));
        if let Err(e) = self.update_settings(|s| s.remember_catalog(&path)) {
            tracing::warn!(?e, "最近開いたカタログを記録できない");
        }
        self.describe(&session)
    }

    /// カタログを閉じる。`discard_unsaved` なら、保存できなかった現像設定を捨てて閉じる。
    pub fn close_catalog(&self, discard_unsaved: bool) -> Result<(), CloseError> {
        let _guard = self.lifecycle.lock();
        self.close_locked(discard_unsaved)
    }

    fn close_locked(&self, discard_unsaved: bool) -> Result<(), CloseError> {
        let Some(session) = self.session.write().take() else {
            return Ok(());
        };
        match session.core.close() {
            Ok(()) => Ok(()),
            Err(e) if !session.core.is_closed() => {
                if discard_unsaved {
                    // 最後にもう一度だけ保存を試し、できなければ捨てて閉じる（drop。genzo-api の doc）。
                    // ほかのスレッドが一時的にコアを持っていても、最後の参照が外れた時点で閉じる。
                    drop(session);
                    Ok(())
                } else {
                    *self.session.write() = Some(session);
                    Err(CloseError::SaveFailed(e.info()))
                }
            }
            Err(e) => Err(CloseError::ClosedWithError(e.info())),
        }
    }

    /// カタログのバックアップの一覧（新しい順）。開いているカタログならコアから、開いていない（壊れて
    /// 開けない）カタログなら、データのフォルダの `backups` から探す。
    pub fn list_backups(&self, catalog_path: &Path) -> Result<Vec<BackupEntry>, ErrorInfo> {
        let catalog_path = absolute(catalog_path)?;
        if let Some(s) = self.session()
            && s.catalog_path == catalog_path
        {
            return s.core.list_backups().map_err(api);
        }
        let dir = catalog_file::data_dir_for(&catalog_path).join("backups");
        // カタログを読めれば、そのカタログの印のものだけ。読めなければ、同じファイル名のものすべて。
        let owned = genzo_catalog::Catalog::open_read_only(&catalog_path)
            .and_then(|c| c.backup_owner())
            .and_then(|owner| genzo_catalog::list_backups(&dir, &owner));
        let list = match owned {
            Ok(l) => l,
            Err(e) => {
                tracing::info!(error = %e, "カタログを読めないので、ファイル名でバックアップを探す");
                let stem = catalog_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                genzo_catalog::list_backups_by_stem(&dir, &stem).map_err(|e| {
                    error(
                        ErrorKind::FileAccess,
                        format!("バックアップのフォルダ {} を読めません: {e}", dir.display()),
                        None,
                    )
                })?
            }
        };
        Ok(list.into_iter().map(BackupEntry::from).collect())
    }

    /// バックアップから復元する: 開いていれば閉じ、バックアップを検証してカタログの隣に復元し、今の
    /// カタログを退避してから差し替え、開き直す（DATA-04。退避したファイルは消さない）。
    pub fn restore(&self, catalog_path: &Path, backup: &Path) -> Result<RestoreOutcome, ErrorInfo> {
        let _guard = self.lifecycle.lock();
        let catalog_path = absolute(catalog_path)?;
        if !backup.is_file() {
            return Err(error(
                ErrorKind::NotFound,
                format!("バックアップ {} が見つかりません", backup.display()),
                None,
            ));
        }
        if catalog_file::inspect(&catalog_path).map_err(|e| internal(e.to_string()))?
            == CatalogFileState::NotCatalog
        {
            return Err(error(
                ErrorKind::InvalidInput,
                format!(
                    "{} は GenzoParis のカタログではありません（変更していません）",
                    catalog_path.display()
                ),
                None,
            ));
        }
        if self
            .session()
            .is_some_and(|s| s.catalog_path == catalog_path)
        {
            self.close_locked(false).map_err(|e| e.info())?;
        }
        let plan = genzo_api::prepare_restore(backup, &catalog_path).map_err(|e| {
            let mut info = e.info();
            if matches!(info.kind, ErrorKind::CatalogCorrupt) {
                info.hint = Some(
                    "別のバックアップを選んでください（カタログ・既存のファイルは変更していません）"
                        .to_owned(),
                );
            }
            info
        })?;
        let displaced = genzo_api::apply_restore(&plan).map_err(|e| {
            let mut info = e.info();
            info.hint = Some(format!(
                "復元したファイル {} は残っています。カタログを開いている GenzoParis（アプリ・CLI）を終了してから、やり直してください",
                plan.restored_path.display()
            ));
            info
        })?;
        let (status, open_error) = match self.open_locked(&catalog_path, false) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e)),
        };
        Ok(RestoreOutcome {
            backup: backup.to_path_buf(),
            displaced,
            status,
            open_error,
        })
    }
}

/// 相対パスを絶対パスにする（同じカタログかの比較とロックのため）。
fn absolute(path: &Path) -> Result<PathBuf, ErrorInfo> {
    std::path::absolute(path).map_err(|e| {
        error(
            ErrorKind::InvalidInput,
            format!("パス {} を解釈できません: {e}", path.display()),
            None,
        )
    })
}

/// コアのイベントを送り先に転送するスレッド（コアが閉じてチャネルが切れたら終わる）。
fn spawn_forwarder(core: &Core, sink: EventSink) {
    let rx = core.subscribe();
    let spawned = std::thread::Builder::new()
        .name("genzo-events".to_owned())
        .spawn(move || {
            while let Ok(event) = rx.recv() {
                sink(&event);
            }
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "イベントを転送するスレッドを作れない");
    }
}
