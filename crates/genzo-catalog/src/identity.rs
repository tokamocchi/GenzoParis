//! 内容による同一性（ORG-03）・重複の候補（IMP-04）・RAW と JPEG のペアの付属のファイル（IMP-06）。
//!
//! docs/04_architecture.md の 3.3 節・3.11 節。
//!
//! - ファイルの内容は、**クイックハッシュ**（[`crate::quick_hash`]。大きさ ＋ 先頭と末尾の 64KB）で候補を
//!   絞り、**全体のハッシュ**（[`crate::full_hash`]。ファイル全体の BLAKE3）で確定する。クイックハッシュが
//!   同じだけでは同じ内容とみなさない（中間だけが違うファイルを見分けられないため。3.3 節）。
//! - 全体のハッシュは、ファイルの内容が記録と同じ（大きさ・更新日時・クイックハッシュ・リビジョン）で、
//!   まだ記録していないときだけ記録する（[`Catalog::set_full_hash_if_current`]）。計算の間にファイルが
//!   変わった・登録し直された場合に、古い内容のハッシュを記録しないため。記録した全体のハッシュは、
//!   ゴミ箱へ移す前の「登録したものと同じファイルか」の確認（6.4 節）に使うので、記録した後に別の値で
//!   上書きしない（内容の変化を検知して登録し直すと消える。[`Catalog::update_file_facts`] と登録し直し）。
//! - 索引は `file(quick_hash)` と `file(full_hash) WHERE full_hash IS NOT NULL`（スキーマの版 10）。

use genzo_model::{AssetId, FileId, VariantId};
use rusqlite::{OptionalExtension, params};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::files::{FILE_COLUMNS, FileRecord, file_from_row};
use crate::hash::{FileFacts, is_hex64};
use crate::util::u64_to_i64;

/// 全体のハッシュの計算の状況（[`Catalog::content_hash_counts`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContentHashCounts {
    /// 登録されているファイルの数（RAW と組の JPEG も 1 つに数える）。
    pub files: u64,
    /// 全体のハッシュを記録したファイルの数。
    pub hashed: u64,
    /// 全体のハッシュがなく、見つからない（`missing`）状態でないファイルの数（計算の対象）。
    pub pending: u64,
}

/// 全体のハッシュ（内容）が同じファイルの組（[`Catalog::duplicate_groups`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// 全体のハッシュ。
    pub full_hash: String,
    /// 同じ内容のファイル（2 つ以上。id の順）。
    pub files: Vec<FileRecord>,
}

/// 重複の組の一覧（[`Catalog::duplicate_groups`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DuplicateGroups {
    /// 組の総数（`groups` は上限までの数）。
    pub total: u64,
    /// 組（最初のファイルの id の順）。
    pub groups: Vec<DuplicateGroup>,
}

/// ハッシュの文字列を確かめる（16 進数の小文字 64 文字）。
fn check_hex(value: &str, what: &str) -> Result<()> {
    if is_hex64(value) {
        Ok(())
    } else {
        Err(CatalogError::InvalidInput(format!(
            "{what}が 16 進数の小文字 64 文字ではありません: {value:?}"
        )))
    }
}

impl Catalog {
    /// クイックハッシュが `quick_hash` のファイル（id の順。状態によらない。索引 `file_quick_hash`）。
    ///
    /// 重複の **候補** を探すのに使う（同じ内容かは全体のハッシュで確かめる）。クイックハッシュはファイルの
    /// 大きさを含むので、大きさも同じファイルだけが返る。
    pub fn files_with_quick_hash(&self, quick_hash: &str) -> Result<Vec<FileRecord>> {
        check_hex(quick_hash, "クイックハッシュ")?;
        self.files_where("quick_hash = ?1", quick_hash)
    }

    /// 全体のハッシュが `full_hash` のファイル（id の順。状態によらない。索引 `file_full_hash`）。
    pub fn files_with_full_hash(&self, full_hash: &str) -> Result<Vec<FileRecord>> {
        check_hex(full_hash, "全体のハッシュ")?;
        self.files_where("full_hash = ?1 AND full_hash IS NOT NULL", full_hash)
    }

    fn files_where(&self, condition: &str, value: &str) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {FILE_COLUMNS} FROM file WHERE {condition} ORDER BY id"
        ))?;
        let mut rows = stmt.query([value])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(file_from_row(row)?);
        }
        Ok(out)
    }

    /// 全体のハッシュを、ファイルの記録が計算した時と同じ（リビジョン `revision`・大きさ・更新日時・
    /// クイックハッシュが `facts`）で、見つからない状態でなく、まだ全体のハッシュを記録していないときだけ
    /// 記録する。記録した（または既に同じ値だった）なら `true`。
    ///
    /// 記録しないのは、計算の間に登録し直された（リビジョン・大きさなどが違う）・見つからなくなった・
    /// 既に別の値を記録している（記録の後に中間だけが書き換わったファイル。上書きすると、ゴミ箱へ移す前の
    /// 確認で見分けられなくなる）場合。ファイルがなければ [`CatalogError::NotFound`]。
    pub fn set_full_hash_if_current(
        &mut self,
        file_id: FileId,
        revision: u32,
        facts: &FileFacts,
        full_hash: &str,
    ) -> Result<bool> {
        check_hex(full_hash, "全体のハッシュ")?;
        let size = u64_to_i64(facts.size, "ファイルサイズ")?;
        let tx = self.conn.transaction()?;
        let current: Option<Option<String>> = tx
            .prepare_cached("SELECT full_hash FROM file WHERE id = ?1")?
            .query_row([file_id.get()], |row| row.get(0))
            .optional()?;
        let Some(current) = current else {
            return Err(CatalogError::NotFound(format!("ファイル {file_id}")));
        };
        let stored = match current {
            Some(existing) => existing == full_hash,
            None => {
                tx.prepare_cached(
                    "UPDATE file SET full_hash = ?2
                     WHERE id = ?1 AND full_hash IS NULL AND revision = ?3 AND size = ?4
                       AND mtime = ?5 AND quick_hash = ?6 AND status != 'missing'",
                )?
                .execute(params![
                    file_id.get(),
                    full_hash,
                    i64::from(revision),
                    size,
                    facts.mtime_ns,
                    facts.quick_hash
                ])? == 1
            }
        };
        tx.commit()?;
        Ok(stored)
    }

    /// 全体のハッシュがなく、見つからない（`missing`）状態でないファイルを、id の順に `limit` 件まで返す
    /// （`after` より大きい id から。バックグラウンドの計算で少しずつ読む）。
    pub fn files_without_full_hash(
        &self,
        after: Option<FileId>,
        limit: usize,
    ) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {FILE_COLUMNS} FROM file
             WHERE id > ?1 AND full_hash IS NULL AND status != 'missing'
             ORDER BY id LIMIT ?2"
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut rows = stmt.query(params![after.map_or(0, |f| f.get()), limit])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(file_from_row(row)?);
        }
        Ok(out)
    }

    /// 全体のハッシュの計算の状況（すべてのファイルを数える。50 万件で表を 1 回読む）。
    pub fn content_hash_counts(&self) -> Result<ContentHashCounts> {
        let (files, hashed, pending): (i64, i64, Option<i64>) = self.conn.query_row(
            "SELECT count(*), count(full_hash), sum(full_hash IS NULL AND status != 'missing')
             FROM file",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let to_u64 = |v: i64| u64::try_from(v).unwrap_or(0);
        Ok(ContentHashCounts {
            files: to_u64(files),
            hashed: to_u64(hashed),
            pending: to_u64(pending.unwrap_or(0)),
        })
    }

    /// 全体のハッシュ（内容）が同じファイルの組（2 つ以上のもの）を、最初のファイルの id の順に `limit` 組まで
    /// 返す（全体のハッシュを記録したファイルだけが対象。索引 `file_full_hash`）。
    pub fn duplicate_groups(&self, limit: usize) -> Result<DuplicateGroups> {
        let total: i64 = self.conn.query_row(
            "SELECT count(*) FROM (
                 SELECT 1 FROM file WHERE full_hash IS NOT NULL
                 GROUP BY full_hash HAVING count(*) > 1)",
            [],
            |row| row.get(0),
        )?;
        let hashes: Vec<String> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT full_hash FROM file WHERE full_hash IS NOT NULL
                 GROUP BY full_hash HAVING count(*) > 1 ORDER BY min(id) LIMIT ?1",
            )?;
            let limit = i64::try_from(limit).unwrap_or(i64::MAX);
            stmt.query_map([limit], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut groups = Vec::with_capacity(hashes.len());
        for h in hashes {
            let files = self.files_with_full_hash(&h)?;
            groups.push(DuplicateGroup {
                full_hash: h,
                files,
            });
        }
        Ok(DuplicateGroups {
            total: u64::try_from(total).unwrap_or(0),
            groups,
        })
    }

    /// 同じ asset のほかのファイル（RAW と組の JPEG など。IMP-06）。主なファイルが先、あとは id の順。
    ///
    /// ファイルの移動・リネーム（FILE-02）で、付属のファイルを一緒に扱うために使う。ファイルがなければ
    /// [`CatalogError::NotFound`]。
    pub fn companion_files(&self, file_id: FileId) -> Result<Vec<FileRecord>> {
        let asset: Option<i64> = self
            .conn
            .prepare_cached("SELECT asset_id FROM file WHERE id = ?1")?
            .query_row([file_id.get()], |row| row.get(0))
            .optional()?;
        let Some(asset) = asset else {
            return Err(CatalogError::NotFound(format!("ファイル {file_id}")));
        };
        Ok(self
            .files_of_asset(AssetId::new(asset))?
            .into_iter()
            .filter(|f| f.id != file_id)
            .collect())
    }

    /// asset のマスターの variant。
    pub fn master_variant_of(&self, asset_id: AssetId) -> Result<VariantId> {
        crate::register::master_variant(&self.conn, asset_id.get())
    }
}
