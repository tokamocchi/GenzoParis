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

/// 回収で使う、ファイルのない索引の行のキー（16 進数の小文字 64 文字）。
const FAKE_PREVIEW_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// プレビューの索引（thumbs.db の `preview`）の行の削除を、SQLite のエラーで失敗させる（ディスクの空きがない・
/// 入出力のエラーなどで、キャッシュの容量を超えた分の回収だけが失敗する場合の再現用）。[`PreviewCache::set_capacity_bytes`]
/// や [`PreviewCache::evict_to`] は、回収の対象があるときだけ行を削除するので、必ず回収が起きるよう、ファイルの
/// ない大きな行（1 TB）を 1 つ足してから仕掛ける。ほかの書き込み・読み出しは成功する。別の接続で thumbs.db に
/// 仕掛けるので、すでに開いているキャッシュにも効く。外すには [`allow_preview_evictions`]（足した行は、外した後の
/// 最初の回収で、ファイルがないまま索引から除かれる）。
///
/// 容量の上限は、設定の最小値（100 MB）以上にしかできないので、実際のプレビューでは回収を起こせない。
/// そのため、索引の行だけを足す。
///
/// [`PreviewCache::set_capacity_bytes`]: crate::PreviewCache::set_capacity_bytes
/// [`PreviewCache::evict_to`]: crate::PreviewCache::evict_to
pub fn fail_preview_evictions(thumbs_db: impl AsRef<Path>) -> Result<()> {
    let conn = Connection::open(thumbs_db.as_ref())?;
    conn.busy_timeout(crate::schema::BUSY_TIMEOUT)?;
    // 先に前の仕掛けを外してから行を足す（`INSERT OR REPLACE` が、仕掛けのある削除に当たらないように）。
    conn.execute_batch(&format!(
        "DROP TRIGGER IF EXISTS genzo_test_fail_preview_delete;
         INSERT OR REPLACE INTO preview(cache_key, variant_id, size, last_used_at, use_seq)
         VALUES ('{FAKE_PREVIEW_KEY}', NULL, 1000000000000, '2000-01-01T00:00:00.000Z', 0);
         CREATE TRIGGER genzo_test_fail_preview_delete BEFORE DELETE ON preview
         BEGIN SELECT RAISE(ABORT, 'genzo test: preview index delete failed'); END;"
    ))?;
    Ok(())
}

/// [`fail_preview_evictions`] で仕掛けた失敗を外す（足した行は残る。次の回収で索引から除かれる）。
pub fn allow_preview_evictions(thumbs_db: impl AsRef<Path>) -> Result<()> {
    let conn = Connection::open(thumbs_db.as_ref())?;
    conn.busy_timeout(crate::schema::BUSY_TIMEOUT)?;
    conn.execute_batch("DROP TRIGGER IF EXISTS genzo_test_fail_preview_delete;")?;
    Ok(())
}

/// 現像設定の保存（[`Catalog::save_develop`] など）の書き込みを、SQLite のエラーで失敗させる（ディスクの空きが
/// ない・入出力のエラーで、現像設定の保存だけが失敗する場合の再現用）。履歴（`history_entry`）への追加と、
/// variant の現像設定（`variant.develop_json`）の更新を失敗させる。ほかの書き込み（設定・評価・
/// 終了の印など）は成功する。別の接続でカタログのファイルに仕掛けるので、すでに開いているカタログにも効く。
/// 外すには [`allow_develop_writes`]。
///
/// 履歴への追加はどれも失敗するので、仕掛けている間は、写真の取り込み（最初の履歴を作る）や仮想コピーの
/// 作成も失敗する。取り込みなどは、仕掛ける前に済ませる。
pub fn fail_develop_writes(path: impl AsRef<Path>) -> Result<()> {
    let conn = Connection::open(path.as_ref())?;
    conn.busy_timeout(crate::schema::BUSY_TIMEOUT)?;
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS genzo_test_fail_develop_history;
         DROP TRIGGER IF EXISTS genzo_test_fail_develop_variant;
         CREATE TRIGGER genzo_test_fail_develop_history BEFORE INSERT ON history_entry
         BEGIN SELECT RAISE(ABORT, 'genzo test: develop write failed'); END;
         CREATE TRIGGER genzo_test_fail_develop_variant BEFORE UPDATE OF develop_json ON variant
         BEGIN SELECT RAISE(ABORT, 'genzo test: develop write failed'); END;",
    )?;
    Ok(())
}

/// [`fail_develop_writes`] で仕掛けた失敗を外す。
pub fn allow_develop_writes(path: impl AsRef<Path>) -> Result<()> {
    let conn = Connection::open(path.as_ref())?;
    conn.busy_timeout(crate::schema::BUSY_TIMEOUT)?;
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS genzo_test_fail_develop_history;
         DROP TRIGGER IF EXISTS genzo_test_fail_develop_variant;",
    )?;
    Ok(())
}
