//! カタログの接続（[`Catalog`]）、起動時の確認、設定（docs/04_architecture.md の 3.4 節・DATA-05）。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::error::{CatalogError, Result};
#[cfg(test)]
use crate::schema::Migration;
use crate::schema::{self, CATALOG_SCHEMA_VERSION, CATALOG_SPEC, Durability, SchemaSpec};
use crate::util::now_utc_string;

/// `app_state` に保存する、前回の終了の状態のキー。
pub(crate) const STATE_SHUTDOWN: &str = "shutdown";
/// 使用中（開いている）の印。
const SHUTDOWN_IN_USE: &str = "in_use";
/// 正常に終了した印。
pub(crate) const SHUTDOWN_CLEAN: &str = "clean";
/// 最後に開いた日時のキー。
const STATE_LAST_OPENED: &str = "last_opened_at";
/// 最後に正常に閉じた日時のキー。
const STATE_LAST_CLOSED: &str = "last_closed_at";
/// キャッシュの世代のキー（カタログを作った・復元したときに新しくする。外部のキャッシュ（thumbs.db・
/// L1 プレビュー）がこのカタログのものかを確かめるのに使う）。
pub(crate) const STATE_CACHE_GENERATION: &str = "cache_generation";
/// カタログの ID のキー（カタログを初めて書き込み用に開いたときに作る。バックアップ・復元では変わらない。
/// 同じファイル名の別のカタログのバックアップを見分けるのに使う。F10）。
pub(crate) const STATE_CATALOG_ID: &str = "catalog_id";

/// 新しいキャッシュの世代（重ならない値。16 進数 32 文字）。
pub(crate) fn new_cache_generation() -> String {
    unique_hex(b"genzo.cache_generation\0")
}

/// 新しいカタログの ID（重ならない値。16 進数 32 文字）。
fn new_catalog_id() -> String {
    unique_hex(b"genzo.catalog_id\0")
}

/// 重ならない値（16 進数 32 文字）。時刻・プロセス・通し番号と、標準ライブラリの `RandomState`（OS の
/// 乱数で初期化される）から作る（乱数の crate を使わないため）。`domain` は用途の区別。
fn unique_hex(domain: &[u8]) -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = blake3::Hasher::new();
    h.update(domain);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    h.update(&now.to_le_bytes());
    h.update(&std::process::id().to_le_bytes());
    h.update(&COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    h.update(&random.to_le_bytes());
    h.finalize().to_hex()[..32].to_owned()
}

/// 前回の終了の状態（DATA-05 の起動時の短いチェック）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviousShutdown {
    /// 初めて開いた（新しいカタログ）。
    FirstOpen,
    /// 前回は正常に終了した。
    Clean,
    /// 前回は正常に終了しなかった（異常終了・強制終了・電源断）。
    /// 詳細チェック（[`Catalog::check_integrity`]）を案内する。
    Unclean,
}

/// カタログを開いたときの報告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenReport {
    /// 前回の終了の状態。読み取り専用で開いた場合は確認しないため `Clean` とする。
    pub previous_shutdown: PreviousShutdown,
    /// マイグレーションを適用した場合の、適用前の版（新しいカタログなら 0）。
    pub migrated_from: Option<u32>,
    /// 現在のスキーマの版。
    pub schema_version: u32,
    /// 移行の前に作ったバックアップ（DATA-08）。
    pub migration_backup: Option<PathBuf>,
}

/// カタログを開くときの設定。
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// マイグレーションの前のバックアップの保存先（`None` ならカタログと同じフォルダ）。
    pub migration_backup_dir: Option<PathBuf>,
}

/// 接続の設定の確認用の値（テスト・診断用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionSettings {
    /// `PRAGMA journal_mode`（ファイルなら `"wal"`）。
    pub journal_mode: String,
    /// `PRAGMA synchronous`（0 = OFF、1 = NORMAL、2 = FULL、3 = EXTRA）。
    pub synchronous: i64,
    /// `PRAGMA foreign_keys`。
    pub foreign_keys: bool,
    /// `PRAGMA fullfsync`（macOS でカタログを開いた場合だけ有効）。
    pub fullfsync: bool,
}

/// カタログ（catalog.db）への接続。
///
/// 書き込みは本体の DB 書き込みスレッドだけが行う（04 の 1.3 節）。検索・一覧の読み取りには、
/// [`Catalog::open_read_only`] で開いた別の接続を使える（WAL なので書き込みと並行して読める）。
///
/// 終了するときは [`Catalog::close`] を呼ぶ。呼ばずに破棄すると、次に開いたときに
/// 「前回は正常に終了しなかった」と報告する。
pub struct Catalog {
    pub(crate) conn: Connection,
    path: Option<PathBuf>,
    read_only: bool,
    report: OpenReport,
}

impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Catalog")
            .field("path", &self.path)
            .field("read_only", &self.read_only)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl Catalog {
    /// カタログを開く（なければ作る）。古いスキーマなら、バックアップを作ってから移行する。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_options(path, &OpenOptions::default())
    }

    /// 設定を指定してカタログを開く。
    pub fn open_with_options(path: impl AsRef<Path>, options: &OpenOptions) -> Result<Self> {
        Self::open_with_spec(path.as_ref(), options, &CATALOG_SPEC)
    }

    /// マイグレーションの列を指定して開く（マイグレーションのテスト用）。
    #[cfg(test)]
    pub(crate) fn open_with_migrations(
        path: &Path,
        options: &OpenOptions,
        migrations: &[Migration],
    ) -> Result<Self> {
        let spec = SchemaSpec {
            migrations,
            ..CATALOG_SPEC
        };
        Self::open_with_spec(path, options, &spec)
    }

    fn open_with_spec(path: &Path, options: &OpenOptions, spec: &SchemaSpec<'_>) -> Result<Self> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(schema::BUSY_TIMEOUT)?;
        // WAL への切り替えなどの書き込みの前に、GenzoParis のカタログかを確かめる。
        schema::check_identity(&conn, spec, path)?;
        schema::configure_connection(&conn, Durability::Full, false)?;
        schema::use_immediate_transactions(&mut conn);
        let outcome = schema::migrate(
            &mut conn,
            spec,
            Some(path),
            options.migration_backup_dir.as_deref(),
        )?;
        let previous_shutdown = mark_in_use(&conn)?;
        Ok(Self {
            conn,
            path: Some(path.to_path_buf()),
            read_only: false,
            report: OpenReport {
                previous_shutdown,
                migrated_from: (outcome.from != outcome.to).then_some(outcome.from),
                schema_version: outcome.to,
                migration_backup: outcome.backup,
            },
        })
    }

    /// メモリ上にカタログを作る（テスト・計測用。WAL とバックアップは使えない）。
    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        schema::configure_connection(&conn, Durability::Full, true)?;
        schema::use_immediate_transactions(&mut conn);
        let outcome = schema::migrate(&mut conn, &CATALOG_SPEC, None, None)?;
        let previous_shutdown = mark_in_use(&conn)?;
        Ok(Self {
            conn,
            path: None,
            read_only: false,
            report: OpenReport {
                previous_shutdown,
                migrated_from: Some(outcome.from),
                schema_version: outcome.to,
                migration_backup: None,
            },
        })
    }

    /// 読み取り専用で開く（検索・一覧の読み取りプール用。04 の 1.3 節）。
    ///
    /// スキーマは最新の版である必要がある（移行は書き込み用の接続で行う）。
    /// 前回の終了の状態の印は変えない。
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        schema::configure_read_only_connection(&conn)?;
        let found = schema::check_identity(&conn, &CATALOG_SPEC, path)?;
        if found != CATALOG_SCHEMA_VERSION {
            return Err(CatalogError::OutdatedSchema {
                found,
                expected: CATALOG_SCHEMA_VERSION,
            });
        }
        Ok(Self {
            conn,
            path: Some(path.to_path_buf()),
            read_only: true,
            report: OpenReport {
                previous_shutdown: PreviousShutdown::Clean,
                migrated_from: None,
                schema_version: found,
                migration_backup: None,
            },
        })
    }

    /// 正常に終了した印を付けて閉じる（DATA-05）。
    pub fn close(self) -> Result<()> {
        if !self.read_only {
            let tx = self.conn.unchecked_transaction()?;
            set_app_state(&tx, STATE_SHUTDOWN, SHUTDOWN_CLEAN)?;
            set_app_state(&tx, STATE_LAST_CLOSED, &now_utc_string())?;
            tx.commit()?;
            // 統計の更新（SQLite の推奨: 接続を閉じる前に PRAGMA optimize）。
            self.conn.execute_batch("PRAGMA optimize;")?;
        }
        self.conn.close().map_err(|(_, e)| CatalogError::Sqlite(e))
    }

    /// 開いたときの報告（前回の終了の状態、マイグレーションの有無）。
    pub fn open_report(&self) -> &OpenReport {
        &self.report
    }

    /// カタログのファイルのパス（メモリ上なら `None`）。
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// 読み取り専用で開いたか。
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// スキーマの版（`PRAGMA user_version`）。
    pub fn schema_version(&self) -> Result<u32> {
        schema::user_version(&self.conn)
    }

    /// 接続の設定を読む（テスト・診断用）。
    pub fn connection_settings(&self) -> Result<ConnectionSettings> {
        connection_settings(&self.conn)
    }

    /// 設定の値を読む。
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM setting WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    /// 設定の値を保存する（あれば置き換える）。
    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO setting(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// 設定を削除する。削除したら `true`。
    pub fn remove_setting(&mut self, key: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM setting WHERE key = ?1", [key])?
            > 0)
    }

    /// アプリの状態（`app_state`）の値を読む（最後に開いた日時など）。
    pub fn app_state(&self, key: &str) -> Result<Option<String>> {
        get_app_state(&self.conn, key)
    }

    /// キャッシュの世代（なければ作って保存する）。カタログを作ったとき・バックアップから復元したときに
    /// 新しい値になる。外部のキャッシュ（thumbs.db・L1 プレビュー）に記録しておき、違えば捨てる
    /// （復元の後は ID が再利用されるため、前の写真のキャッシュを別の写真のものとして使わないように）。
    pub fn cache_generation(&mut self) -> Result<String> {
        if let Some(g) = get_app_state(&self.conn, STATE_CACHE_GENERATION)? {
            return Ok(g);
        }
        let g = new_cache_generation();
        set_app_state(&self.conn, STATE_CACHE_GENERATION, &g)?;
        Ok(g)
    }

    /// カタログの ID（書き込み用に開いたときに、なければ作る。読み取り専用で開いた古いカタログでは
    /// `None` のことがある）。バックアップ・復元では変わらない（F10）。
    pub fn catalog_id(&self) -> Result<Option<String>> {
        get_app_state(&self.conn, STATE_CATALOG_ID)
    }
}

/// 接続の設定を読む。
pub(crate) fn connection_settings(conn: &Connection) -> Result<ConnectionSettings> {
    let journal_mode: String = conn.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    let synchronous: i64 = conn.pragma_query_value(None, "synchronous", |r| r.get(0))?;
    let foreign_keys: i64 = conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
    let fullfsync: i64 = conn.pragma_query_value(None, "fullfsync", |r| r.get(0))?;
    Ok(ConnectionSettings {
        journal_mode,
        synchronous,
        foreign_keys: foreign_keys != 0,
        fullfsync: fullfsync != 0,
    })
}

pub(crate) fn get_app_state(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM app_state WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

fn set_app_state(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO app_state(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// 前回の終了の状態を読み、「使用中」の印を付ける（DATA-05 の起動時の短いチェック）。
fn mark_in_use(conn: &Connection) -> Result<PreviousShutdown> {
    let tx = conn.unchecked_transaction()?;
    let previous = match get_app_state(&tx, STATE_SHUTDOWN)?.as_deref() {
        None => PreviousShutdown::FirstOpen,
        Some(SHUTDOWN_CLEAN) => PreviousShutdown::Clean,
        Some(_) => PreviousShutdown::Unclean,
    };
    set_app_state(&tx, STATE_SHUTDOWN, SHUTDOWN_IN_USE)?;
    set_app_state(&tx, STATE_LAST_OPENED, &now_utc_string())?;
    // カタログの ID（この修正の前に作ったカタログには、最初に開いたときに付ける。F10）。
    if get_app_state(&tx, STATE_CATALOG_ID)?.is_none() {
        set_app_state(&tx, STATE_CATALOG_ID, &new_catalog_id())?;
    }
    tx.commit()?;
    Ok(previous)
}
