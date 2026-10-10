//! 接続の設定と、番号付きのマイグレーション（docs/04_architecture.md の 3.4 節・DATA-08）。
//!
//! - スキーマの版は `PRAGMA user_version` に、GenzoParis の DB であることの印は
//!   `PRAGMA application_id` に保存する（どちらもマイグレーションと同じトランザクションで書く）。
//! - 既存の DB を新しいスキーマへ移行する前に、`VACUUM INTO` で自動的にバックアップを作る。
//! - DB の版がアプリより新しい場合はエラーにする（古いアプリで新しいカタログを壊さないため）。
//! - マイグレーションは 1 版ずつ 1 つのトランザクションで適用する。テーブルの作り直しに備えて、
//!   適用中は外部キーの確認を止め、各版のコミット前に `PRAGMA foreign_key_check` で確認する
//!   （SQLite の説明書の ALTER TABLE の「12 の手順」と同じ考え方）。

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, Transaction};

use crate::backup::vacuum_into_file;
use crate::error::{CatalogError, Result};
use crate::util::timestamp_for_filename;

/// カタログ（catalog.db）の `PRAGMA application_id`（ASCII の "GZCT"）。
pub const CATALOG_APPLICATION_ID: i32 = 0x475A_4354;

/// サムネイル DB（thumbs.db）の `PRAGMA application_id`（ASCII の "GZTH"）。
pub const THUMBS_APPLICATION_ID: i32 = 0x475A_5448;

/// 書き込みの競合で待つ時間（`busy_timeout`）。
///
/// 仮置き: 書き込みは DB 書き込みスレッドに集め（04 の 1.3 節）、読み取りは WAL で待たないため、
/// 待つのはチェックポイントやバックアップとの競合だけの見込み。PoC-6 で見直す。
pub const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 接続ごとのページキャッシュの大きさ（KiB。`PRAGMA cache_size` に負の値で渡す）。
///
/// 仮置き: 64MiB。メモリの予算（通常時 4GB。SCL-05）に対して小さく、50 万件の索引の
/// 主要な部分が載る程度として置いた。PoC-6 で計測して見直す。
pub const CACHE_SIZE_KIB: i64 = 64 * 1024;

/// 1 つのマイグレーション（版 `version` へ上げる手順）。
#[derive(Clone, Copy)]
pub(crate) struct Migration {
    /// 適用後の版（1 から順に 1 ずつ増やす）。
    pub version: u32,
    /// 内容の説明（ログ・テスト用）。
    #[allow(dead_code)]
    pub description: &'static str,
    /// 実行する SQL（複数の文を含んでよい）。
    pub sql: &'static str,
    /// SQL の後に実行する Rust の処理（正規化した列の作り直しなど。SQL だけで書けない手順用）。
    pub post: Option<fn(&Transaction<'_>) -> Result<()>>,
}

/// カタログのマイグレーションの列。新しい版は末尾に追加し、既存の版の内容は変えない。
pub(crate) const CATALOG_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "初期スキーマ（04 の 3.1 節・3.5 節・3.6 節・6.4 節）",
        sql: include_str!("sql/catalog_0001_initial.sql"),
        post: None,
    },
    Migration {
        version: 2,
        description: "外部キーの子の列の索引（variant.history_pos・stack.top_variant_id）",
        sql: include_str!("sql/catalog_0002_fk_child_indexes.sql"),
        post: None,
    },
];

/// サムネイル DB のマイグレーションの列。
pub(crate) const THUMBS_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "サムネイル（L0）とプレビュー（L1）の索引（04 の 4 章・4.1 節）",
        sql: include_str!("sql/thumbs_0001_initial.sql"),
        post: None,
    },
    Migration {
        version: 2,
        description: "キャッシュの持ち主（カタログの世代）と、作り直し待ちの印",
        sql: include_str!("sql/thumbs_0002_owner_and_pending.sql"),
        post: None,
    },
];

/// このアプリのカタログのスキーマの版。
pub const CATALOG_SCHEMA_VERSION: u32 = 2;

/// このアプリのサムネイル DB のスキーマの版。
pub const THUMBS_SCHEMA_VERSION: u32 = 2;

/// DB の種類ごとのスキーマの定義。
#[derive(Clone, Copy)]
pub(crate) struct SchemaSpec<'a> {
    pub application_id: i32,
    pub migrations: &'a [Migration],
    /// 既存の DB を移行する前にバックアップを作るか（サムネイル DB は作り直せるので作らない）。
    pub backup_before_migration: bool,
}

impl SchemaSpec<'_> {
    /// 最新の版。
    pub fn target_version(&self) -> u32 {
        self.migrations.last().map_or(0, |m| m.version)
    }
}

pub(crate) const CATALOG_SPEC: SchemaSpec<'static> = SchemaSpec {
    application_id: CATALOG_APPLICATION_ID,
    migrations: CATALOG_MIGRATIONS,
    backup_before_migration: true,
};

pub(crate) const THUMBS_SPEC: SchemaSpec<'static> = SchemaSpec {
    application_id: THUMBS_APPLICATION_ID,
    migrations: THUMBS_MIGRATIONS,
    backup_before_migration: false,
};

/// 同期の強さ（3.4 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Durability {
    /// `synchronous = FULL`（macOS では `fullfsync` も）。カタログ用（DATA-03b）。
    Full,
    /// `synchronous = NORMAL`。作り直せるサムネイル DB 用。
    Normal,
}

/// 接続の設定（3.4 節）。
///
/// WAL、`synchronous`、macOS の `fullfsync`、`foreign_keys = ON`、`busy_timeout`。
/// メモリ上の DB（テスト用）は WAL にできないため、ジャーナルの設定を飛ばす。
pub(crate) fn configure_connection(
    conn: &Connection,
    durability: Durability,
    in_memory: bool,
) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // SQLite の既定では外部キーの制約が無効なため、すべての接続で有効にする（3.4 節）。
    conn.pragma_update(None, "foreign_keys", true)?;
    if !in_memory {
        let mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "wal", |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(CatalogError::InvalidInput(format!(
                "WAL モードにできません（journal_mode = {mode}）。ネットワーク上のドライブなどでは使えません"
            )));
        }
    }
    let synchronous = match durability {
        Durability::Full => "FULL",
        Durability::Normal => "NORMAL",
    };
    conn.pragma_update(None, "synchronous", synchronous)?;
    // macOS の通常の fsync は、ストレージ装置のキャッシュまで書き込まない場合があるため（3.4 節）。
    #[cfg(target_os = "macos")]
    if durability == Durability::Full {
        conn.pragma_update(None, "fullfsync", true)?;
        conn.pragma_update(None, "checkpoint_fullfsync", true)?;
    }
    conn.pragma_update(None, "cache_size", -CACHE_SIZE_KIB)?;
    conn.set_prepared_statement_cache_capacity(64);
    Ok(())
}

/// 書き込み用の接続のトランザクションを `BEGIN IMMEDIATE` で始めるようにする。
///
/// 既定の `BEGIN DEFERRED` では、読み取りから始めたトランザクションが途中で書き込みに移るとき、
/// その間に別の接続がコミットしていると、`busy_timeout` で待たずに直ちに `SQLITE_BUSY`
/// （`SQLITE_BUSY_SNAPSHOT`）で失敗する（SQLite の説明書「Write-Ahead Logging」の
/// 「Sometimes Queries Return SQLITE_BUSY In WAL Mode」）。登録・削除・履歴の移動などは
/// 読んでから書くので、書き込みのロックを最初に取り、競合したら `busy_timeout` まで待つ。
/// 書き込みは DB 書き込みスレッドに集める設計（04 の 1.3 節）だが、CLI とアプリが同じカタログを
/// 同時に開いた場合などにも、途中で失敗せずに待つようにするため。
pub(crate) fn use_immediate_transactions(conn: &mut Connection) {
    conn.set_transaction_behavior(rusqlite::TransactionBehavior::Immediate);
}

/// 読み取り専用の接続の設定（WAL・同期の設定は書き込み側の接続が行う）。
pub(crate) fn configure_read_only_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "cache_size", -CACHE_SIZE_KIB)?;
    conn.set_prepared_statement_cache_capacity(64);
    Ok(())
}

/// `PRAGMA user_version` を読む。
pub(crate) fn user_version(conn: &Connection) -> Result<u32> {
    let v: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    u32::try_from(v).map_err(|_| CatalogError::Corrupt(format!("user_version が不正です: {v}")))
}

/// DB の素性を確かめる（GenzoParis の DB か、版が新しすぎないか）。書き込みはしない。
///
/// 戻り値は現在の版（新しい空の DB なら 0）。
pub(crate) fn check_identity(conn: &Connection, spec: &SchemaSpec<'_>, path: &Path) -> Result<u32> {
    let not_ours = || CatalogError::NotOurDatabase(path.to_path_buf());
    let found = match user_version(conn) {
        Ok(v) => v,
        Err(CatalogError::Sqlite(rusqlite::Error::SqliteFailure(e, _)))
            if e.code == rusqlite::ErrorCode::NotADatabase =>
        {
            return Err(not_ours());
        }
        Err(e) => return Err(e),
    };
    let app_id: i32 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let object_count: i64 =
        conn.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;
    if found == 0 {
        // 版 0 でテーブルがあるのは、GenzoParis 以外の DB。
        if object_count > 0 {
            return Err(not_ours());
        }
    } else if app_id != spec.application_id {
        return Err(not_ours());
    }
    let target = spec.target_version();
    if found > target {
        return Err(CatalogError::FutureSchema {
            found,
            supported: target,
        });
    }
    Ok(found)
}

/// マイグレーションの結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MigrationOutcome {
    /// 適用前の版（新しい DB なら 0）。
    pub from: u32,
    /// 適用後の版。
    pub to: u32,
    /// 移行前に作ったバックアップ。
    pub backup: Option<PathBuf>,
}

/// 最新の版までマイグレーションを適用する。
///
/// `db_path` と `backup_dir` は、既存の DB を移行する前のバックアップに使う
/// （`backup_dir` が `None` なら DB と同じフォルダ）。メモリ上の DB（`db_path = None`）は
/// バックアップを作らない。
pub(crate) fn migrate(
    conn: &mut Connection,
    spec: &SchemaSpec<'_>,
    db_path: Option<&Path>,
    backup_dir: Option<&Path>,
) -> Result<MigrationOutcome> {
    validate_migrations(spec.migrations)?;
    let from = user_version(conn)?;
    let to = spec.target_version();
    if from > to {
        return Err(CatalogError::FutureSchema {
            found: from,
            supported: to,
        });
    }
    if from == to {
        return Ok(MigrationOutcome {
            from,
            to,
            backup: None,
        });
    }

    // DATA-08: 既存の DB を移行する前に、自動でバックアップする。
    let backup = match (from > 0 && spec.backup_before_migration, db_path) {
        (true, Some(path)) => {
            let dir = backup_dir
                .map(Path::to_path_buf)
                .or_else(|| path.parent().map(Path::to_path_buf))
                .unwrap_or_default();
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "catalog".to_owned());
            std::fs::create_dir_all(&dir).map_err(|e| CatalogError::io(&dir, e))?;
            let dest = dir.join(format!(
                "{stem}.v{from}-before-migration-{}.db",
                timestamp_for_filename(chrono::Utc::now())
            ));
            vacuum_into_file(conn, &dest)?;
            Some(dest)
        }
        _ => None,
    };

    conn.pragma_update(None, "foreign_keys", false)?;
    let result = apply_migrations(conn, spec, from);
    // 失敗した場合も外部キーの確認を元に戻す。
    let restore = conn.pragma_update(None, "foreign_keys", true);
    result?;
    restore?;
    Ok(MigrationOutcome { from, to, backup })
}

fn apply_migrations(conn: &mut Connection, spec: &SchemaSpec<'_>, from: u32) -> Result<()> {
    for m in spec.migrations.iter().filter(|m| m.version > from) {
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        if let Some(post) = m.post {
            post(&tx)?;
        }
        let violations: i64 =
            tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if violations > 0 {
            return Err(CatalogError::Corrupt(format!(
                "版 {} へのマイグレーションの後に外部キーの違反が {violations} 件あります",
                m.version
            )));
        }
        tx.pragma_update(None, "user_version", m.version)?;
        tx.pragma_update(None, "application_id", spec.application_id)?;
        tx.commit()?;
    }
    Ok(())
}

/// マイグレーションの版が 1 から 1 ずつ増えているかを確かめる（定義の誤りを防ぐ）。
pub(crate) fn validate_migrations(migrations: &[Migration]) -> Result<()> {
    for (i, m) in migrations.iter().enumerate() {
        let expected = u32::try_from(i + 1).unwrap_or(u32::MAX);
        if m.version != expected {
            return Err(CatalogError::InvalidInput(format!(
                "マイグレーションの版が連番ではありません（{i} 番目が版 {}）",
                m.version
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_lists_are_consecutive_and_match_constants() {
        validate_migrations(CATALOG_MIGRATIONS).unwrap();
        validate_migrations(THUMBS_MIGRATIONS).unwrap();
        assert_eq!(CATALOG_SPEC.target_version(), CATALOG_SCHEMA_VERSION);
        assert_eq!(THUMBS_SPEC.target_version(), THUMBS_SCHEMA_VERSION);
        let bad = [Migration {
            version: 2,
            description: "",
            sql: "",
            post: None,
        }];
        assert!(validate_migrations(&bad).is_err());
    }

    #[test]
    fn application_ids_are_ascii_tags() {
        assert_eq!(&CATALOG_APPLICATION_ID.to_be_bytes(), b"GZCT");
        assert_eq!(&THUMBS_APPLICATION_ID.to_be_bytes(), b"GZTH");
    }
}
