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
//!   （genzo-api の `Core::close` の doc。決定事項ログ No.61 の閉じ方）。ただし、アプリが終わる間際
//!   （画面に選択肢を出せない。[`AppState::close_at_exit`]）は、保存できなければ捨てて閉じる。
//! - 開く・閉じる・復元は 1 つずつ行う（`lifecycle` のロック）。開いていないカタログのファイルを調べる処理
//!   （[`AppState::inspect_catalog`]・[`AppState::list_backups`]）も同じロックを持つ（復元の差し替えと
//!   重ならないように）。閉じている途中に届いた要求は、閉じ始めた時点でカタログが開いていない扱いになり、
//!   [`ErrorKind::Closed`] を返す。
//! - 別のカタログを開く・復元するときに、前のカタログを閉じる処理が「閉じた後の段階のエラー」を返したら、
//!   記録して続ける（`continue_after_close`）。開いたまま（保存できない）なら続けない。
//! - パスは絶対パスにしてから扱う（相対パスは今のフォルダから）。開いているカタログと同じファイルかは、
//!   書き方（`..`・シンボリックリンク・Windows の大文字と小文字）によらずに比べる（`same_file`）。
//!
//! # イベント
//!
//! コアのイベントは、カタログごとのスレッドで送り先（[`EventSink`]）に送る。
//!
//! - 起動時の警告（前回の異常終了など）は [`CatalogStatus::warnings`] で返し、イベントとしては送らない
//!   （コアは購読したときに起動時の警告を先に送るが、画面が両方を受けて 2 回知らせないよう、その分は
//!   送らない）。
//! - カタログを閉じたら、そのカタログのイベントはそれ以上送らない（送り先が遅く、イベントが溜まって
//!   いても）。次に開いたカタログの画面に、前のカタログの検索結果の世代・ジョブ・サムネイルの更新が届かない
//!   ようにするため。閉じる処理は、送っている途中の 1 件を送り終えるのを待つ。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use genzo_api::{
    ApiError, BackupEntry, Capabilities, CatalogStats, Core, CoreConfig, CoreSettings, DisplayInfo,
    ErrorInfo, ErrorKind, Event, StartupReport, WorkerLaunch,
};
use genzo_color::icc::{IccProfile, StandardProfile};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::catalog_file::{self, Assessment};
use crate::logs::AppLogs;
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
    /// イベントを送るスレッド（閉じたら止める）。
    events: Forwarder,
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
    /// 起動時の警告（前回の異常終了など）。イベント（`core-event`）としては送らない（モジュールの doc）。
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
    /// ログとクラッシュレポート（SYS-07）。
    logs: AppLogs,
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
            logs: AppLogs::new(genzo_api::logging::status().unwrap_or_default()),
        }
    }

    /// ログの状態を指定する（テスト用。既定は genzo-api の `logging::status`）。
    pub fn with_logs(mut self, logs: AppLogs) -> Self {
        self.logs = logs;
        self
    }

    /// ログとクラッシュレポート。
    pub fn logs(&self) -> &AppLogs {
        &self.logs
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

    /// 画面の色空間を設定する（開いているカタログにもすぐ反映する。開いていなければ `None` で、次に開いた
    /// カタログに使う）。
    pub fn set_display_target(
        &self,
        target: DisplayTarget,
    ) -> Result<Option<DisplayInfo>, ErrorInfo> {
        // 開いているカタログに反映し終えるまでロックを持つ（カタログを開く処理も、色空間を指定してから
        // セッションを公開するまで同じロックを持つので、同時に呼ばれても、どちらかの順に必ずそろう）。
        let mut display = self.display.lock();
        *display = target;
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

    /// パスがカタログか（画面の開く・作る前の確認。`inspect_catalog_path`）。開く処理・復元と同じ判定
    /// （[`catalog_file::assess`]）で、先頭が壊れたカタログもカタログとして返す（[`Assessment::file_state`]）。
    /// 相対パスは今のフォルダから（開く処理と同じ）。
    pub fn inspect_catalog(
        &self,
        path: &Path,
    ) -> Result<catalog_file::CatalogFileState, ErrorInfo> {
        // 開いていないカタログを SQLite で開いて調べることがあるので、復元の差し替えと重ならないようにする
        // （list_backups の doc）。
        let _guard = self.lifecycle.lock();
        Ok(assess(&absolute(path)?)?.file_state())
    }

    /// カタログを開く。`create` なら、なければ作る（あれば、GenzoParis のカタログならそのまま開く）。
    pub fn open_catalog(&self, path: &Path, create: bool) -> Result<CatalogStatus, ErrorInfo> {
        let _guard = self.lifecycle.lock();
        self.open_locked(path, create)
    }

    fn open_locked(&self, path: &Path, create: bool) -> Result<CatalogStatus, ErrorInfo> {
        let path = absolute(path)?;
        // 同じカタログが開いていれば、そのまま返す（書き方が違っても。閉じると、取り込みなどのジョブを
        // 取り消してしまう）。
        if let Some(s) = self.session()
            && same_file(&s.catalog_path, &path)
        {
            return self.describe(&s);
        }
        match assess(&path)? {
            Assessment::Damaged => {
                // 隣のデータのフォルダに GenzoParis のデータがある: 先頭が壊れたカタログとみなす（作り直さず、
                // バックアップからの復元を案内する）。
                return Err(error(
                    ErrorKind::CatalogCorrupt,
                    format!(
                        "カタログ {} を読めません（ファイルが壊れている可能性があります）",
                        path.display()
                    ),
                    Some(
                        "バックアップから復元してください（壊れたファイルは消さずに、別の名前で残します）",
                    ),
                ));
            }
            Assessment::NotCatalog => {
                return Err(error(
                    ErrorKind::InvalidInput,
                    format!("{} は GenzoParis のカタログではありません", path.display()),
                    Some("カタログ（.db）のファイルを選ぶか、新しいカタログを作ってください"),
                ));
            }
            Assessment::Missing if !create => {
                return Err(error(
                    ErrorKind::NotFound,
                    format!("カタログ {} が見つかりません", path.display()),
                    Some("ドライブの接続を確かめるか、別のカタログを開いてください"),
                ));
            }
            Assessment::Missing => {
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
            Assessment::Catalog => {}
        }
        // 前のカタログを閉じる（開いたままなら開かない。閉じた後の段階のエラーは記録して続ける）。
        continue_after_close(self.close_locked(false), "別のカタログを開く")
            .map_err(|e| e.info())?;

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
        // 色空間を指定してからセッションを公開するまで、色空間のロックを持つ（set_display_target の doc）。
        let display = self.display.lock();
        if let Err(e) = display
            .icc()
            .and_then(|icc| core.set_display_profile(Some(&icc)).map_err(api))
        {
            tracing::warn!(?e, "画面の色空間を設定できないので sRGB とみなす");
        }
        let events = spawn_forwarder(&core, Arc::clone(&self.events));
        let session = Arc::new(Session {
            core,
            catalog_path: path.clone(),
            data_dir,
            tools,
            events,
        });
        *self.session.write() = Some(Arc::clone(&session));
        drop(display);
        tracing::info!(catalog = %path.display(), create, "カタログを開きました");
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

    /// アプリが終わる間際（画面に選択肢を出せない）に、開いているカタログを閉じる（lib.rs の
    /// `RunEvent::Exit`。macOS の Dock の「終了」・ログアウト、Windows のシャットダウン・サインアウトなど、
    /// 終了の要求を経ずに終わる場合）。開いていなければ何もしない。
    ///
    /// まず通常どおり保存待ちの現像設定を保存して閉じ、保存できなければ、捨てて閉じる（`close_locked` の
    /// `discard_unsaved`。genzo-api の `Core::close_discarding` が最後にもう一度だけ保存を試す）。開いたまま
    /// にせずに捨てる理由:
    ///
    /// - この呼び出しから戻ると、すぐにプロセスが終わる（macOS は `applicationWillTerminate:` の後、Windows は
    ///   `WM_ENDSESSION` の後に OS が終わらせる）。メモリの中の未保存の設定は、どちらにしても失われる。
    /// - 開いたまま終わると、ジョブの取り消し・ワーカーの終了・終了時のバックアップ・ロックファイルの解放が
    ///   行われない。捨てて閉じれば、これらを済ませる。
    /// - 保存できなかった設定を捨てたときは、カタログに「正常に終了した」印を付けない（genzo-api の
    ///   `Core::close_discarding`）。次に開いたとき、前回の異常終了として警告するので、設定が失われたことを
    ///   利用者に知らせられる。
    ///
    /// 開く・閉じる・復元の途中なら、終わるのを待ってから閉じる（`lifecycle` のロック）。
    pub fn close_at_exit(&self) -> Result<(), CloseError> {
        let _guard = self.lifecycle.lock();
        match self.close_locked(false) {
            Err(CloseError::SaveFailed(e)) => {
                tracing::error!(
                    error = ?e,
                    "アプリが終わる間際に保存待ちの現像設定を保存できないので、捨てて閉じる"
                );
                self.close_locked(true)
            }
            r => r,
        }
    }

    fn close_locked(&self, discard_unsaved: bool) -> Result<(), CloseError> {
        let Some(session) = self.session.write().take() else {
            return Ok(());
        };
        match session.core.close() {
            Ok(()) => {
                session.events.stop();
                tracing::info!(catalog = %session.catalog_path.display(), "カタログを閉じました");
                Ok(())
            }
            Err(e) if !session.core.is_closed() => {
                if discard_unsaved {
                    // 最後にもう一度だけ保存を試し、できなければ捨てて閉じる（genzo-api の
                    // `close_discarding`）。ほかのスレッドが一時的にコアを持っていても、ここで閉じる
                    // （ロックファイルの解放・正常な終了の印などを、終了の前に終える）。
                    let result = session.core.close_discarding();
                    session.events.stop();
                    match result {
                        Ok(()) => Ok(()),
                        Err(e) => Err(CloseError::ClosedWithError(e.info())),
                    }
                } else {
                    *self.session.write() = Some(session);
                    Err(CloseError::SaveFailed(e.info()))
                }
            }
            Err(e) => {
                session.events.stop();
                Err(CloseError::ClosedWithError(e.info()))
            }
        }
    }

    /// カタログのバックアップの一覧（新しい順）。開いているカタログならコアから、開いていない（壊れて
    /// 開けない）カタログなら、データのフォルダの `backups` から探す。
    ///
    /// 開く・閉じる・復元と同じロック（`lifecycle`）を持つ: 開いていないカタログは SQLite で読み取り専用に
    /// 開いて調べるので、復元の差し替え（ファイルの名前の変更）と重ならないようにする（Windows では、開いて
    /// いるファイルの名前を変えられず、復元が失敗するため。この環境（Linux）では確かめていない。要確認）。
    pub fn list_backups(&self, catalog_path: &Path) -> Result<Vec<BackupEntry>, ErrorInfo> {
        let _guard = self.lifecycle.lock();
        let catalog_path = absolute(catalog_path)?;
        if let Some(s) = self.session()
            && same_file(&s.catalog_path, &catalog_path)
        {
            return s.core.list_backups().map_err(api);
        }
        // コアと同じ置き場（データのフォルダの backups。CoreConfig::backup_dir）。
        let dir =
            CoreConfig::new(&catalog_path, catalog_file::data_dir_for(&catalog_path)).backup_dir();
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

    /// バックアップから復元する: バックアップを検証してカタログの隣の別のファイルに復元し、カタログが
    /// 開いていれば閉じ、今のカタログを退避してから差し替え、開き直す（DATA-04。退避したファイルは消さない）。
    ///
    /// 差し替えるのは、GenzoParis のカタログ、ないファイル、先頭が壊れたカタログ（隣のデータのフォルダに
    /// GenzoParis のデータがある。[`catalog_file::has_catalog_data`]）だけ。バックアップが使えなければ、
    /// 開いているカタログは閉じない。
    ///
    /// - 開いているカタログを閉じられない（保存待ちの現像設定を保存できない）、または別のアプリ（アプリ・
    ///   CLI）がカタログを開いている（conflict）ときは、復元した写し（`*-restored-*.db`）を片付けて、
    ///   カタログのファイルは変えずにエラーを返す（写しはバックアップから作り直せる。残すと、やり直すたびに
    ///   写しが増える）。
    /// - 閉じる処理が閉じた後の段階でエラーを返したときは、カタログは閉じているので、記録して差し替える
    ///   （`continue_after_close`）。
    /// - 差し替えの途中の失敗（名前の変更など）では、写しを残し、その場所を案内に書く。
    pub fn restore(&self, catalog_path: &Path, backup: &Path) -> Result<RestoreOutcome, ErrorInfo> {
        let _guard = self.lifecycle.lock();
        let catalog_path = absolute(catalog_path)?;
        let backup = absolute(backup)?;
        if !backup.is_file() {
            return Err(error(
                ErrorKind::NotFound,
                format!("バックアップ {} が見つかりません", backup.display()),
                None,
            ));
        }
        if assess(&catalog_path)? == Assessment::NotCatalog {
            return Err(error(
                ErrorKind::InvalidInput,
                format!(
                    "{} は GenzoParis のカタログではありません（変更していません）",
                    catalog_path.display()
                ),
                None,
            ));
        }
        // 先にバックアップを検証して復元しておく（カタログの隣の別のファイル。今のカタログは変えないので、
        // 開いたままでよい）。使えないバックアップなら、開いているカタログを閉じずに返す。
        let plan = genzo_api::prepare_restore(&backup, &catalog_path).map_err(|e| {
            let mut info = e.info();
            if matches!(info.kind, ErrorKind::CatalogCorrupt) {
                info.hint = Some(
                    "別のバックアップを選んでください（カタログ・既存のファイルは変更していません）"
                        .to_owned(),
                );
            }
            info
        })?;
        if self
            .session()
            .is_some_and(|s| same_file(&s.catalog_path, &catalog_path))
            && let Err(e) = continue_after_close(self.close_locked(false), "復元")
        {
            // 閉じられない（保存待ちの現像設定を保存できず、開いたまま）: 差し替えずに、復元したファイルを
            // 片付ける（バックアップから作った写しなので、消しても失うものはない）。
            remove_restored_copy(&plan.restored_path);
            return Err(e.info());
        }
        let displaced = genzo_api::apply_restore(&plan).map_err(|e| {
            let mut info = e.info();
            if info.kind == ErrorKind::Conflict {
                // 別のアプリ（アプリ・CLI）がカタログを開いている: apply_restore はロックを取れずに、何も
                // 変えずに戻る。復元した写しは片付ける（残すと、やり直すたびに写しが増える。バックアップから
                // 作り直せるので、消しても失うものはない）。
                remove_restored_copy(&plan.restored_path);
                info.hint = Some(
                    "カタログを開いている GenzoParis（アプリ・CLI）を終了してから、やり直してください（カタログは変更していません）"
                        .to_owned(),
                );
            } else {
                // 名前の変更の途中の失敗など: 復元した写しは、手で差し替えられるように残す。
                info.hint = Some(format!(
                    "復元したファイル {} は残っています。カタログのフォルダの権限を確かめ、カタログを開いている GenzoParis（アプリ・CLI）を終了してから、やり直してください",
                    plan.restored_path.display()
                ));
            }
            info
        })?;
        let (status, open_error) = match self.open_locked(&catalog_path, false) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e)),
        };
        Ok(RestoreOutcome {
            backup,
            displaced,
            status,
            open_error,
        })
    }
}

/// 前のカタログを閉じた結果から、続けて（別のカタログを開く・差し替える）よいかを決める。
///
/// - 開いたまま（[`CloseError::SaveFailed`]。保存待ちの現像設定を保存できない）なら続けない（そのエラーを
///   返す。画面は再試行か、捨てて閉じるかを選ばせる）。
/// - 閉じた後の段階のエラー（[`CloseError::ClosedWithError`]。ワーカーの終了・サムネイルの DB・カタログを
///   閉じる処理など）は、記録して続ける。セッションはもう外れ、コアは閉じてロックも外れている
///   （genzo-api の `Core::close` の doc。閉じ直しはできない）ので、止めると、閉じたまま何もせずに返すことに
///   なる。
fn continue_after_close(r: Result<(), CloseError>, what: &str) -> Result<(), CloseError> {
    match r {
        Err(CloseError::ClosedWithError(e)) => {
            tracing::warn!(
                error = ?e,
                "{what}: 前のカタログを閉じる処理の途中でエラーがあったが、閉じたので続ける"
            );
            Ok(())
        }
        r => r,
    }
}

/// 復元した写し（`*-restored-*.db`）を片付ける（失敗は記録だけ）。
fn remove_restored_copy(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        tracing::warn!(error = %e, path = %path.display(), "復元したファイルを片付けられない");
    }
}

/// カタログのファイルを判定する（[`catalog_file::assess`]。読めなければ file_access のエラー）。
fn assess(path: &Path) -> Result<Assessment, ErrorInfo> {
    catalog_file::assess(path).map_err(|e| {
        error(
            ErrorKind::FileAccess,
            format!("{} を確かめられません: {e}", path.display()),
            Some("ファイルの場所・ドライブの接続・権限を確かめてください"),
        )
    })
}

/// 同じファイルか。書き方が同じか、どちらもあって、リンクと `..` を解決したパスが同じなら同じ
/// （Windows では大文字と小文字の違いも解決する）。
fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
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

/// コアのイベントを送り先に転送するスレッドの止め方（モジュールの doc の「イベント」）。
struct Forwarder {
    /// 送ってよいか。スレッドは、このロックを持ったまま確かめてから送る（止めた後に送らないため）。
    active: Arc<Mutex<bool>>,
}

impl Forwarder {
    /// 止める: 送っている途中のイベントがあれば送り終えるのを待ち、その後は送らない（スレッドは次の
    /// イベントを受けたとき、またはチャネルが切れたときに終わる）。
    fn stop(&self) {
        *self.active.lock() = false;
    }
}

/// コアのイベントを送り先に転送するスレッドを作る（止めるか、コアが閉じてチャネルが切れたら終わる）。
fn spawn_forwarder(core: &Core, sink: EventSink) -> Forwarder {
    // 起動時の警告は CatalogStatus::warnings で返すので、購読したときに先に届く分（同じもの）は送らない。
    // 起動時の警告はコアを開く処理の中でだけ記録されるので、数はここで決まる。
    let replayed = core.startup_warnings().len();
    let rx = core.subscribe();
    let active = Arc::new(Mutex::new(true));
    let flag = Arc::clone(&active);
    let spawned = std::thread::Builder::new()
        .name("genzo-events".to_owned())
        .spawn(move || {
            for event in rx.iter().skip(replayed) {
                let active = flag.lock();
                if !*active {
                    break;
                }
                sink(&event);
            }
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "イベントを転送するスレッドを作れない");
    }
    Forwarder { active }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 別のカタログを開く・復元する前に閉じた結果: 開いたまま（保存できない）なら止め、閉じた後の段階の
    /// エラーなら続ける（結合テストの catalog.rs・backup.rs でも、ワーカーを終わらせて起こした場合を
    /// 確かめる）。
    #[test]
    fn only_a_catalog_left_open_stops_the_next_step() {
        assert!(continue_after_close(Ok(()), "テスト").is_ok());
        let info = error(ErrorKind::Storage, "保存できない", None);
        let stopped = continue_after_close(Err(CloseError::SaveFailed(info.clone())), "テスト");
        assert!(matches!(stopped, Err(CloseError::SaveFailed(ref e)) if *e == info));
        let worker = error(ErrorKind::Internal, "ワーカーを終了できない", None);
        assert!(
            continue_after_close(Err(CloseError::ClosedWithError(worker)), "テスト").is_ok(),
            "閉じた後のエラーでは止めない"
        );
    }
}
