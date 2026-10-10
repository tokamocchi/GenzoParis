//! 他の crate の結合テスト用の関数（機能フラグ `test-support`。配布するビルドでは使わない）。

use std::path::Path;

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
