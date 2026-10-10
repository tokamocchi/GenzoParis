//! 他の crate の結合テスト用の関数（機能フラグ `test-support`。配布するビルドでは使わない）。

use std::path::Path;

use rusqlite::Connection;

use crate::catalog::{Catalog, OpenOptions};
use crate::error::{CatalogError, Result};
use crate::schema::CATALOG_MIGRATIONS;

/// スキーマの版 `version` までのマイグレーションだけを適用して開く（古い版のカタログを作って、
/// 移行を確かめるため）。`version` は 1 からこのアプリの版まで。
///
/// 返したカタログは古いスキーマのまま書き込める。閉じた後に [`Catalog::open`] で開くと、
/// 最新の版へ移行する。
pub fn open_at_version(path: impl AsRef<Path>, version: u32) -> Result<Catalog> {
    let n = usize::try_from(version).unwrap_or(usize::MAX);
    if n == 0 || n > CATALOG_MIGRATIONS.len() {
        return Err(CatalogError::InvalidInput(format!(
            "スキーマの版 {version} はありません"
        )));
    }
    Catalog::open_with_migrations(
        path.as_ref(),
        &OpenOptions::default(),
        &CATALOG_MIGRATIONS[..n],
    )
}

/// 設定 `key` の書き込み（新規・更新のどちらも）を、SQLite のエラーで失敗させる（ディスクの空きがない・
/// 入出力のエラーなどで、複数の設定を保存する途中の書き込みが失敗する場合の再現用）。ほかの設定の書き込みは
/// 成功する。別の接続でカタログのファイルに仕掛けるので、すでに開いているカタログにも効く（同じ `key` には、
/// 前に仕掛けたものを置き換える）。外すには [`allow_setting_writes`]。
pub fn fail_setting_writes(path: impl AsRef<Path>, key: &str) -> Result<()> {
    let conn = Connection::open(path.as_ref())?;
    conn.busy_timeout(crate::schema::BUSY_TIMEOUT)?;
    // 文字列リテラルの中の `'` は二重にする（キーは固定の文字列だけを渡す想定だが、壊れた SQL にしない）。
    let key = key.replace('\'', "''");
    conn.execute_batch(&format!(
        "DROP TRIGGER IF EXISTS genzo_test_fail_setting_insert;
         DROP TRIGGER IF EXISTS genzo_test_fail_setting_update;
         CREATE TRIGGER genzo_test_fail_setting_insert BEFORE INSERT ON setting
         WHEN NEW.key = '{key}'
         BEGIN SELECT RAISE(ABORT, 'genzo test: setting write failed'); END;
         CREATE TRIGGER genzo_test_fail_setting_update BEFORE UPDATE ON setting
         WHEN NEW.key = '{key}'
         BEGIN SELECT RAISE(ABORT, 'genzo test: setting write failed'); END;"
    ))?;
    Ok(())
}

/// [`fail_setting_writes`] で仕掛けた失敗を外す。
pub fn allow_setting_writes(path: impl AsRef<Path>) -> Result<()> {
    let conn = Connection::open(path.as_ref())?;
    conn.busy_timeout(crate::schema::BUSY_TIMEOUT)?;
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS genzo_test_fail_setting_insert;
         DROP TRIGGER IF EXISTS genzo_test_fail_setting_update;",
    )?;
    Ok(())
}
