//! コア（[`Core`]）: 起動と終了、状態の持ち方、ロックの順序。
//!
//! # 実行モデル（04 の 1.3 節からの逸脱）
//!
//! 1.3 節はコマンド処理に tokio の非同期ランタイムを想定しているが、このコアは **同期の API ＋ 内部の
//! スレッド ＋ イベントのチャネル** で作る。
//!
//! - コマンド（[`Core::execute`] と型付きのメソッド）は呼び出したスレッドで実行し、結果を返す。
//!   ブロックしうる（現像の写真を開く・同期の描画など）ので、Tauri からは `spawn_blocking` で呼ぶ。
//! - 長い処理（取り込み・書き出し・プレビューの作り直し・詳細チェック）は genzo-jobs のスケジューラ
//!   （P3、[`crate::config::DEFAULT_BACKGROUND_THREADS`] 本）で実行し、ジョブの ID をすぐに返す。
//! - 現像のプレビュー（P0）は 1 本のスレッドのスケジューラで「最新の 1 件だけ」を処理する
//!   （`submit_latest`。6.2 節）。
//! - DB 書き込みスレッド（1.3 節）の代わりに、カタログの接続を 1 つの `Mutex` で守り、書き込みを
//!   1 つずつにする（SQLite の書き込みは 1 本しか進まないので、同じ効果）。読み取りプールはまだない
//!   （検索も同じ接続。PoC-6 で必要なら分ける）。
//! - GPU スレッド（1.3 節）は置かず、GPU を使う処理は呼び出したスレッドで行う（genzo-gpu の
//!   `GpuRenderer` はスレッド間で共有できる）。P0 と P3 の GPU の投入の順序の制御（PERF-13）は PoC-3 で
//!   GPU スレッドを置くかとあわせて決める。
//! - イベントは [`Core::subscribe`] のチャネルで送る。
//!
//! # ロックの順序（デッドロックを避ける）
//!
//! 現像のセッション → 検索の状態 → カタログ → キャッシュ（thumbs.db・previews）。それ以外のロック
//! （ジョブの記録・GPU・画面のプロファイル・削除の計画・設定）は、他のロックを持たずに短く使う。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use chrono::{FixedOffset, Utc};
use genzo_catalog::{Catalog, OpenOptions, PreviewCache, ThumbStore};
use genzo_jobs::{MemoryBudget, Scheduler};
use genzo_pipeline::{Engine, EngineConfig};
use genzo_worker::{PoolConfig, WorkerConfig, WorkerPool};
use parking_lot::Mutex;

use crate::config::{AUTO_BACKUP_INTERVAL, CoreConfig, DEFAULT_BACKGROUND_THREADS, WorkerLaunch};
use crate::delete::{PlanStore, TrashBackend, recover_file_ops};
use crate::develop::DevelopShared;
use crate::error::ApiError;
use crate::events::{Event, EventHub, WarningCode};
use crate::jobs::JobRegistry;
use crate::render::{DisplayState, GpuState};
use crate::search::SearchState;
use crate::types::{BackupEntry, CoreSettings, PreviousShutdown, StartupReport};

/// サムネイル DB（L0）と L1 プレビューのキャッシュ。
pub(crate) struct CacheDb {
    pub thumbs: ThumbStore,
    pub previews: PreviewCache,
}

/// [`Core::open_with`] で差し替えられる部品（テスト用）。
#[derive(Clone)]
pub struct CoreHooks {
    /// OS のゴミ箱への移動（既定は trash crate。テストでは偽物を渡して OS のゴミ箱を汚さない）。
    pub trash: Arc<dyn TrashBackend>,
}

impl Default for CoreHooks {
    fn default() -> Self {
        Self {
            trash: Arc::new(crate::delete::OsTrash),
        }
    }
}

impl std::fmt::Debug for CoreHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreHooks").finish_non_exhaustive()
    }
}

/// コアの状態（[`Core`] と、ジョブ・自動保存のスレッドが共有する）。
pub(crate) struct Inner {
    pub config: CoreConfig,
    pub catalog: Mutex<Option<Catalog>>,
    pub cache: Mutex<Option<CacheDb>>,
    pub events: EventHub,
    pub workers: Mutex<Option<Arc<WorkerPool>>>,
    pub background: Scheduler,
    pub interactive: Scheduler,
    pub engine: Engine,
    pub gpu: Mutex<GpuState>,
    pub display: Mutex<DisplayState>,
    pub search: Mutex<SearchState>,
    pub develop: Arc<DevelopShared>,
    pub jobs: JobRegistry,
    pub plans: Mutex<PlanStore>,
    pub trash: Arc<dyn TrashBackend>,
    pub settings: Mutex<CoreSettings>,
    /// 最後に投入した撮影日時の推定し直しのジョブ（続けて設定を変えたときに、前のジョブを取り消す）。
    pub reresolve_job: Mutex<Option<u64>>,
    /// キャッシュの世代（カタログの `cache_generation`。キャッシュキーの材料。復元の後に変わる）。
    pub cache_generation: String,
    /// ワーカーの RAW デコーダの識別子（起動時に確かめる。L0 / L1 のキャッシュキーの材料。`deps` の doc）。
    pub raw_decoder: Option<String>,
    pub startup: StartupReport,
    pub closed: AtomicBool,
    pub saver: Mutex<Option<JoinHandle<()>>>,
    /// カタログのロック（閉じたら解放する）。
    pub lock: Mutex<Option<crate::lock::CatalogLock>>,
}

impl Inner {
    /// 終了していないか。
    pub(crate) fn check_open(&self) -> Result<(), ApiError> {
        if self.closed.load(Ordering::Acquire) {
            Err(ApiError::Closed)
        } else {
            Ok(())
        }
    }

    /// カタログを使う（genzo-catalog のエラーを変換する）。
    pub(crate) fn with_catalog<T>(
        &self,
        f: impl FnOnce(&mut Catalog) -> genzo_catalog::Result<T>,
    ) -> Result<T, ApiError> {
        let mut guard = self.catalog.lock();
        let cat = guard.as_mut().ok_or(ApiError::Closed)?;
        Ok(f(cat)?)
    }

    /// カタログを使う（コア API のエラーを返す処理）。
    pub(crate) fn with_catalog_api<T>(
        &self,
        f: impl FnOnce(&mut Catalog) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let mut guard = self.catalog.lock();
        let cat = guard.as_mut().ok_or(ApiError::Closed)?;
        f(cat)
    }

    /// キャッシュ（thumbs.db・previews）を使う（エラーは [`ApiError::Cache`]。作り直せるので、カタログの
    /// 復元は案内しない）。
    pub(crate) fn with_cache<T>(
        &self,
        f: impl FnOnce(&mut CacheDb) -> genzo_catalog::Result<T>,
    ) -> Result<T, ApiError> {
        let mut guard = self.cache.lock();
        let cache = guard.as_mut().ok_or(ApiError::Closed)?;
        f(cache).map_err(|source| ApiError::Cache {
            path: self.config.thumbs_path(),
            source: Box::new(source),
        })
    }

    /// ファイル名 `name` の写真の L0 / L1 のキャッシュキーに入れる RAW デコーダの識別子（RAW 以外は `None`）。
    pub(crate) fn raw_decoder_for(&self, name: &str) -> Option<&str> {
        (genzo_catalog::pair_class(name) == genzo_catalog::PairClass::Raw)
            .then_some(self.raw_decoder.as_deref())
            .flatten()
    }

    /// ワーカーの組。
    pub(crate) fn pool(&self) -> Result<Arc<WorkerPool>, ApiError> {
        self.workers.lock().clone().ok_or(ApiError::Closed)
    }

    /// 既定のタイムゾーンのオフセット。
    pub(crate) fn default_offset(&self) -> FixedOffset {
        let minutes = self.settings.lock().default_utc_offset_minutes;
        FixedOffset::east_opt(minutes * 60).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap())
    }

    /// 自動バックアップ（DATA-04）。前回から [`AUTO_BACKUP_INTERVAL`] 以上たっていれば（`force` なら
    /// 経過時間によらず）作る。世代数が 0 なら作らない。
    pub(crate) fn auto_backup(&self, force: bool) -> Result<Option<BackupEntry>, ApiError> {
        let generations = self.config.backup_generations;
        if generations == 0 {
            return Ok(None);
        }
        let dir = self.config.backup_dir();
        let owner = self.with_catalog(|c| c.backup_owner())?;
        if !force {
            let list = if dir.is_dir() {
                genzo_catalog::list_backups(&dir, &owner)?
            } else {
                Vec::new()
            };
            // このカタログのバックアップ（同じファイル名の別のカタログのもの・印のない古い形式のものは
            // 数えない。F10）のうち、今より前の日時で最も新しいもの。未来の日時のもの（時計が戻った・時計の
            // 進んだ機器で作った）は数えないので、今の状態のバックアップがなければ作る。作ったものは世代の
            // 管理で消さないので、次に開いたときは作り直さない（F09）。
            let now = Utc::now();
            let newest = list
                .iter()
                .filter(|b| owner.owns(b) && b.created_at <= now)
                .map(|b| b.created_at)
                .max();
            if let Some(newest) = newest
                && now
                    .signed_duration_since(newest)
                    .to_std()
                    .is_ok_and(|e| e < AUTO_BACKUP_INTERVAL)
            {
                return Ok(None);
            }
        }
        std::fs::create_dir_all(&dir).map_err(|e| ApiError::io(&dir, e))?;
        let info = genzo_catalog::create_backup(&self.config.catalog_path, &dir, generations)?;
        Ok(Some(info.into()))
    }
}

/// コア API（UI と CLI が共通に使う。MAINT-01）。
///
/// [`Core::open`] で開き、[`Core::close`] で閉じる（drop しても閉じる）。スレッド間で共有できる
/// （`Arc<Core>` で持つ）。
pub struct Core {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Core {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core")
            .field("catalog", &self.inner.config.catalog_path)
            .field("closed", &self.inner.closed.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Core {
    /// カタログを開き、起動時の確認（DATA-05・DATA-07・DATA-04）をして、ワーカーを起動する。
    ///
    /// 同じカタログを別の [`Core`]（別のアプリ・CLI・同じプロセスを含む）が開いていれば
    /// [`ApiError::CatalogInUse`]（`lock` の doc）。作り直せるキャッシュ（thumbs.db・プレビューのフォルダ）
    /// を開けなくても、退避して作り直すか既定の場所に切り替えて開く（警告 [`WarningCode::CacheRebuilt`]）。
    pub fn open(config: CoreConfig) -> Result<Self, ApiError> {
        Self::open_with(config, CoreHooks::default())
    }

    /// 部品を差し替えて開く（テスト用。[`CoreHooks`]）。
    pub fn open_with(config: CoreConfig, hooks: CoreHooks) -> Result<Self, ApiError> {
        std::fs::create_dir_all(&config.data_dir).map_err(|e| ApiError::io(&config.data_dir, e))?;
        if let Some(parent) = config.catalog_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| ApiError::io(parent, e))?;
        }
        let events = EventHub::default();

        // カタログの排他（1 つのカタログは 1 つのアプリだけが開く。`lock` の doc）。
        let lock = match crate::lock::acquire(&config.catalog_path)? {
            crate::lock::Acquired::Locked(l) => l,
            crate::lock::Acquired::Unsupported(l, reason) => {
                events.emit_sticky(Event::Warning {
                    code: WarningCode::CatalogLockUnavailable,
                    message: format!(
                        "このボリュームではファイルロックが使えないため、カタログを同時に開くことを防げません（{reason}）"
                    ),
                    variant_id: None,
                    path: Some(config.catalog_path.clone()),
                });
                l
            }
        };

        // カタログ（マイグレーションの前のバックアップは、自動バックアップと同じ場所に置く）。
        let backup_dir = config.backup_dir();
        std::fs::create_dir_all(&backup_dir).map_err(|e| ApiError::io(&backup_dir, e))?;
        let mut catalog = Catalog::open_with_options(
            &config.catalog_path,
            &OpenOptions {
                migration_backup_dir: Some(backup_dir),
            },
        )?;
        let report = catalog.open_report().clone();
        let previous_shutdown = match report.previous_shutdown {
            genzo_catalog::PreviousShutdown::FirstOpen => PreviousShutdown::FirstOpen,
            genzo_catalog::PreviousShutdown::Clean => PreviousShutdown::Clean,
            genzo_catalog::PreviousShutdown::Unclean => PreviousShutdown::Unclean,
        };
        if previous_shutdown == PreviousShutdown::Unclean {
            events.emit_sticky(Event::Warning {
                code: WarningCode::UncleanShutdown,
                message:
                    "前回は正常に終了しませんでした。詳細チェック（整合性の確認）をおすすめします"
                        .to_owned(),
                variant_id: None,
                path: Some(config.catalog_path.clone()),
            });
        }
        let migrated_from = report.migrated_from.filter(|&v| v > 0);
        if let Some(from) = migrated_from {
            events.emit_sticky(Event::Warning {
                code: WarningCode::CatalogMigrated,
                message: format!(
                    "カタログのスキーマを版 {from} から {} へ移行しました（移行の前のバックアップ: {:?}）",
                    report.schema_version, report.migration_backup
                ),
                variant_id: None,
                path: report.migration_backup.clone(),
            });
        }

        // ここから先で失敗したら、カタログを閉じてから返す（開いたままにすると、次に開いたときに
        // 「前回は正常に終了しなかった」と報告してしまうため）。
        let parts = match build_parts(&mut catalog, &config, &events) {
            Ok(p) => p,
            Err(e) => {
                if let Err(close_err) = catalog.close() {
                    tracing::warn!(error = %close_err, "起動の失敗の後にカタログを閉じられない");
                }
                return Err(e);
            }
        };
        let Parts {
            recovered,
            reresolve_capture_times,
            cache_generation,
            raw_decoder,
            settings,
            thumbs,
            previews,
            pool,
            background,
            interactive,
            engine,
            display,
        } = parts;

        let inner = Arc::new(Inner {
            config,
            catalog: Mutex::new(Some(catalog)),
            cache: Mutex::new(Some(CacheDb { thumbs, previews })),
            events,
            workers: Mutex::new(Some(Arc::new(pool))),
            background,
            interactive,
            engine,
            gpu: Mutex::new(GpuState::default()),
            display: Mutex::new(display),
            search: Mutex::new(SearchState::default()),
            develop: Arc::new(DevelopShared::default()),
            jobs: JobRegistry::default(),
            plans: Mutex::new(PlanStore::default()),
            trash: hooks.trash,
            settings: Mutex::new(settings),
            reresolve_job: Mutex::new(None),
            cache_generation,
            raw_decoder,
            startup: StartupReport {
                previous_shutdown,
                migrated_from,
                recovered_file_ops: recovered,
                backup: None,
            },
            closed: AtomicBool::new(false),
            saver: Mutex::new(None),
            lock: Mutex::new(Some(lock)),
        });

        // 自動バックアップ（DATA-04。前回から 1 日以上たっていれば）。失敗しても起動は続ける。
        let backup = match inner.auto_backup(false) {
            Ok(b) => b,
            Err(e) => {
                inner.events.emit_sticky(Event::Warning {
                    code: WarningCode::BackupFailed,
                    message: format!("自動バックアップに失敗しました: {e}"),
                    variant_id: None,
                    path: Some(inner.config.backup_dir()),
                });
                None
            }
        };
        // 起動の報告にバックアップを入れる（Inner は作った直後で、他に共有していない）。
        let mut inner = inner;
        if let Some(i) = Arc::get_mut(&mut inner) {
            i.startup.backup = backup;
        }

        // 現像設定の自動保存のスレッド（DATA-03）。
        match crate::develop::spawn_saver(&inner) {
            Ok(saver) => *inner.saver.lock() = Some(saver),
            Err(e) => {
                let _ = close_inner(&inner);
                return Err(e);
            }
        }
        if reresolve_capture_times {
            crate::settings::spawn_reresolve(&inner);
        }
        // 前回作り直せなかった L0 / L1（編集の直後に終了したなど）を作り直す。
        crate::previews::regenerate_pending(&inner);
        Ok(Self { inner })
    }

    /// 終了する: 保存待ちの現像設定を保存し、ジョブを取り消して終わるのを待ち、ワーカーを終了させ、
    /// 必要なら自動バックアップを作り、カタログに「正常に終了した」印を付けて閉じる（DATA-05）。
    ///
    /// 2 回目以降は何もしない。
    pub fn close(&self) -> Result<(), ApiError> {
        close_inner(&self.inner)
    }

    /// イベントの受け取り側を作る（04 の 1.5 節）。起動時の警告は、後から作った受け取り側にも届く。
    pub fn subscribe(&self) -> crossbeam_channel::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// 起動時の確認の結果。
    pub fn startup_report(&self) -> StartupReport {
        self.inner.startup.clone()
    }

    /// 起動時の警告（後から購読しても受け取れるもの）。
    pub fn startup_warnings(&self) -> Vec<Event> {
        self.inner.events.sticky()
    }

    /// 設定。
    pub fn config(&self) -> &CoreConfig {
        &self.inner.config
    }

    /// カタログのパス。
    pub fn catalog_path(&self) -> &Path {
        &self.inner.config.catalog_path
    }

    /// バックグラウンドのジョブ（P3）と現像のプレビュー（P0）がすべて終わるまで待つ（CLI・テスト用）。
    ///
    /// ジョブの中で投入されたジョブ（取り込みの後のプレビューの作り直しなど）も待つ。
    pub fn wait_idle(&self) {
        loop {
            self.inner.background.wait_idle();
            self.inner.interactive.wait_idle();
            if self.inner.background.queued_count() == 0
                && self.inner.background.running_count() == 0
                && self.inner.interactive.queued_count() == 0
                && self.inner.interactive.running_count() == 0
            {
                break;
            }
        }
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if !self.inner.closed.load(Ordering::Acquire)
            && let Err(e) = close_inner(&self.inner)
        {
            tracing::warn!(error = %e, "コアを閉じるときにエラーが起きました");
        }
    }
}

/// 終了の処理（[`Core::close`]）。
fn close_inner(inner: &Arc<Inner>) -> Result<(), ApiError> {
    if inner.closed.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let mut first_err: Option<ApiError> = None;
    let mut keep = |r: Result<(), ApiError>| {
        if let Err(e) = r {
            tracing::warn!(error = %e, "終了の処理でエラー");
            first_err.get_or_insert(e);
        }
    };
    // 1. 保存待ちの現像設定を保存し、自動保存のスレッドを止める。
    keep(crate::develop::shutdown(inner));
    if let Some(h) = inner.saver.lock().take() {
        let _ = h.join();
    }
    // 2. ジョブを取り消して、終わるのを待つ。
    inner.jobs.cancel_all();
    inner.background.shutdown_now();
    inner.interactive.shutdown_now();
    // キューから実行されずに外れたジョブも、取り消しとして記録して知らせる。
    inner.jobs.reap_unrun(inner);
    // 3. ワーカーを終了させる（他に参照が残っていれば drop で強制終了する）。
    if let Some(pool) = inner.workers.lock().take()
        && let Ok(pool) = Arc::try_unwrap(pool)
    {
        keep(pool.shutdown().map_err(ApiError::from));
    }
    // 4. 自動バックアップ（DATA-04。1 日以上たっていれば、または終了時に必ず作る設定なら）。
    match inner.auto_backup(inner.config.backup_on_close) {
        Ok(_) => {}
        Err(e) => {
            inner.events.warn(
                WarningCode::BackupFailed,
                format!("自動バックアップに失敗しました: {e}"),
                None,
                Some(inner.config.backup_dir()),
            );
        }
    }
    // 5. キャッシュとカタログを閉じる（カタログに「正常に終了した」印を付ける）。
    if let Some(cache) = inner.cache.lock().take() {
        drop(cache.previews);
        keep(cache.thumbs.close().map_err(ApiError::from));
    }
    if let Some(cat) = inner.catalog.lock().take() {
        keep(cat.close().map_err(ApiError::from));
    }
    // カタログを閉じてから、ロックを解放する。
    inner.lock.lock().take();
    *inner.gpu.lock() = GpuState::closed();
    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// カタログを開いた後に用意する部品。
struct Parts {
    recovered: u64,
    reresolve_capture_times: bool,
    cache_generation: String,
    raw_decoder: Option<String>,
    settings: CoreSettings,
    thumbs: ThumbStore,
    previews: PreviewCache,
    pool: WorkerPool,
    background: Scheduler,
    interactive: Scheduler,
    engine: Engine,
    display: DisplayState,
}

/// 起動時の確認と部品の用意（[`Core::open_with`]）。
fn build_parts(
    catalog: &mut Catalog,
    config: &CoreConfig,
    events: &EventHub,
) -> Result<Parts, ApiError> {
    // 終わっていないファイル操作の確定（6.4 節。DATA-07）。
    let recovered = recover_file_ops(catalog, events)?;
    // 設定（SYS-05）。既定のタイムゾーンは、まだ保存していなければ保存する。
    let settings = crate::settings::load(catalog, config, events)?;
    let offset_newly_saved =
        crate::settings::persist_default_offset(catalog, &settings, config, events)?;
    // 保存していなかった既存のカタログでは、取り込んだ時期ごとに推定のオフセットが混ざっていうるので、
    // 保存した値で推定し直す（写真がなければ不要）。
    let reresolve_capture_times = offset_newly_saved && catalog.counts()?.assets > 0;
    // キャッシュ（作り直せるので、開けなくてもカタログは開く。DATA-03b）。
    let (mut thumbs, mut previews) = open_caches(config, &settings, events)?;
    // キャッシュがこのカタログ（の今の世代）のものでなければ捨てる（復元の後・別のカタログ・データの
    // フォルダを残したままの catalog init。ID が再利用されるため）。
    let cache_generation = catalog.cache_generation()?;
    adopt_caches(
        &mut thumbs,
        &mut previews,
        &cache_generation,
        &config.thumbs_path(),
        events,
    )?;
    // ワーカー。RAW デコーダの識別子を確かめる（L0 / L1 のキャッシュキーの材料。確かめられなければ、
    // 本体の識別子（同じ build のワーカーなら同じ）を使う）。
    let pool = WorkerPool::new(pool_config(config)?)?;
    let raw_decoder = pool
        .ping(genzo_worker::Lane::Batch)
        .map(|p| p.raw_decoder)
        .unwrap_or_else(|_| genzo_raw::decoder_id());
    // スケジューラ（P3 と P0 で 1 つのメモリの予算を分け合う。6.1 節）。
    let budget = MemoryBudget::new(genzo_jobs::DEFAULT_NORMAL_BUDGET_BYTES);
    let background = Scheduler::with_budget(
        config
            .background_threads
            .unwrap_or(DEFAULT_BACKGROUND_THREADS)
            .max(1),
        "genzo-p3",
        budget.clone(),
    )
    .map_err(|e| ApiError::Job(e.to_string()))?;
    let interactive =
        Scheduler::with_budget(1, "genzo-p0", budget).map_err(|e| ApiError::Job(e.to_string()))?;
    let engine = Engine::new(EngineConfig::default())?;
    let display = DisplayState::assumed_srgb()?;
    Ok(Parts {
        recovered,
        reresolve_capture_times,
        cache_generation,
        raw_decoder,
        settings,
        thumbs,
        previews,
        pool,
        background,
        interactive,
        engine,
        display,
    })
}

/// サムネイル DB（thumbs.db）とプレビューのキャッシュを開く。
///
/// - thumbs.db が壊れている・別のファイル・版が合わない場合は、`-wal`・`-shm` とともに
///   `thumbs.db.broken-<UTC の時刻>` の名前に変えて退避し（削除しない）、作り直す。ディスクの容量不足・
///   権限などのエラーでは退避しない（[`ApiError::Cache`] を返す）。
/// - 設定したプレビューのフォルダを使えない（外付けのドライブを外したなど）場合は、既定の場所
///   （`data_dir/previews`）に切り替える（起動した後に設定を変えられるように）。
/// - どちらも警告（[`WarningCode::CacheRebuilt`]）を出す。
fn open_caches(
    config: &CoreConfig,
    settings: &CoreSettings,
    events: &EventHub,
) -> Result<(ThumbStore, PreviewCache), ApiError> {
    let thumbs_path = config.thumbs_path();
    let cache_err = |path: &Path, source| ApiError::Cache {
        path: path.to_path_buf(),
        source: Box::new(source),
    };
    let thumbs = match ThumbStore::open(&thumbs_path) {
        Ok(t) => t,
        Err(e) if e.indicates_broken_database() => {
            let moved_to = move_cache_db_aside(&thumbs_path)?;
            events.emit_sticky(Event::Warning {
                code: WarningCode::CacheRebuilt,
                message: format!(
                    "サムネイルのキャッシュ（{}）を開けないため、{} に退避して作り直します（{e}）",
                    thumbs_path.display(),
                    moved_to.display()
                ),
                variant_id: None,
                path: Some(moved_to),
            });
            ThumbStore::open(&thumbs_path).map_err(|e| cache_err(&thumbs_path, e))?
        }
        Err(e) => return Err(cache_err(&thumbs_path, e)),
    };
    let previews = match PreviewCache::open(
        &settings.preview_cache_dir,
        &thumbs_path,
        settings.preview_cache_bytes,
    ) {
        Ok(p) => p,
        Err(e @ genzo_catalog::CatalogError::Io { .. })
            if settings.preview_cache_dir != config.default_preview_dir() =>
        {
            let fallback = config.default_preview_dir();
            events.emit_sticky(Event::Warning {
                code: WarningCode::CacheRebuilt,
                message: format!(
                    "プレビューのフォルダ {} を使えないため、既定の場所 {} を使います（設定で変えられます。{e}）",
                    settings.preview_cache_dir.display(),
                    fallback.display()
                ),
                variant_id: None,
                path: Some(settings.preview_cache_dir.clone()),
            });
            PreviewCache::open(&fallback, &thumbs_path, settings.preview_cache_bytes)
                .map_err(|e| cache_err(&fallback, e))?
        }
        Err(e) => return Err(cache_err(&settings.preview_cache_dir, e)),
    };
    Ok((thumbs, previews))
}

/// thumbs.db に記録する、キャッシュを作ったカタログの世代のキー。
const CACHE_META_CATALOG_GENERATION: &str = "catalog_generation";

/// キャッシュ（L0・L1・作り直し待ちの印）が、カタログの世代 `generation` のものか確かめ、違えば捨てて
/// 世代を記録する。
fn adopt_caches(
    thumbs: &mut ThumbStore,
    previews: &mut PreviewCache,
    generation: &str,
    thumbs_path: &Path,
    events: &EventHub,
) -> Result<(), ApiError> {
    let cache_err = |source| ApiError::Cache {
        path: thumbs_path.to_path_buf(),
        source: Box::new(source),
    };
    let owner = thumbs
        .meta(CACHE_META_CATALOG_GENERATION)
        .map_err(cache_err)?;
    if owner.as_deref() == Some(generation) {
        return Ok(());
    }
    let removed = thumbs.clear().map_err(cache_err)?;
    previews.reconcile().map_err(cache_err)?;
    let evicted = previews.evict_to(0, None).map_err(cache_err)?;
    thumbs
        .set_meta(CACHE_META_CATALOG_GENERATION, generation)
        .map_err(cache_err)?;
    if owner.is_some() && (removed > 0 || evicted.removed > 0) {
        events.emit_sticky(Event::Warning {
            code: WarningCode::CacheRebuilt,
            message: format!(
                "サムネイル・プレビューのキャッシュは別のカタログ（または復元の前のカタログ）のものだったため、捨てて作り直します（サムネイル {removed} 件・プレビュー {} 件）",
                evicted.removed
            ),
            variant_id: None,
            path: Some(thumbs_path.to_path_buf()),
        });
    }
    Ok(())
}

/// 壊れたキャッシュの DB（と `-wal`・`-shm`）を、`<名前>.broken-<UTC の時刻>` に変えて退避する（削除・
/// 上書きはしない）。本体の退避先を返す。
fn move_cache_db_aside(path: &Path) -> Result<std::path::PathBuf, ApiError> {
    let ts = Utc::now().format("%Y%m%dT%H%M%S%3fZ");
    let with_suffix = |p: &Path, suffix: &str| {
        let mut s = p.as_os_str().to_owned();
        s.push(suffix);
        std::path::PathBuf::from(s)
    };
    let target = with_suffix(path, &format!(".broken-{ts}"));
    if target.exists() {
        return Err(ApiError::InvalidArgument(format!(
            "退避先 {} が既にあります",
            target.display()
        )));
    }
    // 付随するファイルを先に移す（本体だけを移して `-wal` が残ると、新しい DB に適用されうるため）。
    for suffix in ["-wal", "-shm", "-journal"] {
        let from = with_suffix(path, suffix);
        if std::fs::symlink_metadata(&from).is_ok() {
            let to = with_suffix(&target, suffix);
            std::fs::rename(&from, &to).map_err(|e| ApiError::io(&from, e))?;
        }
    }
    std::fs::rename(path, &target).map_err(|e| ApiError::io(path, e))?;
    Ok(target)
}

/// ワーカーの組の設定（[`WorkerLaunch`] から）。
fn pool_config(config: &CoreConfig) -> Result<PoolConfig, ApiError> {
    let (executable, args) = match &config.worker {
        WorkerLaunch::Executable { path } => (path.clone(), Vec::new()),
        WorkerLaunch::SelfSubcommand {
            executable,
            subcommand,
        } => {
            let exe = match executable {
                Some(p) => p.clone(),
                None => std::env::current_exe().map_err(|e| {
                    ApiError::Internal(format!("本体の実行ファイルの場所が分かりません: {e}"))
                })?,
            };
            (Some(exe), vec![subcommand.into()])
        }
    };
    let mut worker = WorkerConfig {
        executable,
        args,
        test_hooks: config.worker_test_hooks,
        shm_root: config.worker_shm_dir.clone(),
        env: config.worker_env.clone(),
        ..WorkerConfig::default()
    };
    if let Some(t) = config.worker_timeouts {
        worker.timeouts = t;
    }
    if let Some(t) = config.worker_startup_timeout {
        worker.startup_timeout = t;
    }
    let mut pool = PoolConfig {
        worker,
        ..PoolConfig::default()
    };
    if let Some(n) = config.batch_workers {
        pool.batch_workers = n;
    }
    Ok(pool)
}
