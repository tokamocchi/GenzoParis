//! 階層キーワード（LIB-09。docs/04_architecture.md の 3.2 節・3.6 節）。
//!
//! - 同じ親の下で同じ名前（NFKC ＋ 小文字化で比較）のキーワードは 1 つだけ。
//! - 名前の検索は、全文検索ではなくキーワードのテーブルで、完全一致と前方一致を行う（3.6 節）。
//! - キーワードは variant に付ける。asset や variant を削除しても、キーワード自体は残る（3.5 節）。
//! - キーワードを削除すると、子のキーワードと付与も削除される。

use genzo_model::{KeywordId, VariantId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::text::{LIKE_ESCAPE, like_prefix_pattern, search_key};
use crate::util::{ids_to_json, non_empty_name};

/// キーワード。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyword {
    /// ID。
    pub id: KeywordId,
    /// 親（最上位は `None`）。
    pub parent_id: Option<KeywordId>,
    /// 名前（表示用）。
    pub name: String,
}

/// キーワードの名前の検索の方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordMatch {
    /// 完全一致（NFKC ＋ 小文字化して比較）。
    Exact,
    /// 前方一致（NFKC ＋ 小文字化して比較）。
    Prefix,
}

fn keyword_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Keyword> {
    Ok(Keyword {
        id: KeywordId::new(row.get(0)?),
        parent_id: row.get::<_, Option<i64>>(1)?.map(KeywordId::new),
        name: row.get(2)?,
    })
}

pub(crate) fn ensure_keyword_tx(
    conn: &Connection,
    parent: Option<KeywordId>,
    name: &str,
) -> Result<KeywordId> {
    let name = non_empty_name(name, "キーワードの名前")?;
    let key = search_key(name);
    let parent_raw = parent.map(KeywordId::get);
    if let Some(id) = conn
        .prepare_cached(
            "SELECT id FROM keyword WHERE ifnull(parent_id, 0) = ifnull(?1, 0) AND name_key = ?2",
        )?
        .query_row(params![parent_raw, key], |row| row.get::<_, i64>(0))
        .optional()?
    {
        return Ok(KeywordId::new(id));
    }
    conn.prepare_cached("INSERT INTO keyword(parent_id, name, name_key) VALUES (?1, ?2, ?3)")?
        .execute(params![parent_raw, name, key])?;
    Ok(KeywordId::new(conn.last_insert_rowid()))
}

impl Catalog {
    /// キーワードを確保する（同じ親の下に同じ名前があればそれを返し、なければ作る）。
    pub fn ensure_keyword(&mut self, parent: Option<KeywordId>, name: &str) -> Result<KeywordId> {
        let tx = self.conn.transaction()?;
        let id = ensure_keyword_tx(&tx, parent, name)?;
        tx.commit()?;
        Ok(id)
    }

    /// 階層のパス（例: `["場所", "京都"]`）でキーワードを確保する。最後のキーワードを返す。
    pub fn ensure_keyword_path<S: AsRef<str>>(&mut self, path: &[S]) -> Result<KeywordId> {
        if path.is_empty() {
            return Err(CatalogError::InvalidInput(
                "キーワードのパスが空です".to_owned(),
            ));
        }
        let tx = self.conn.transaction()?;
        let mut parent = None;
        for name in path {
            parent = Some(ensure_keyword_tx(&tx, parent, name.as_ref())?);
        }
        tx.commit()?;
        Ok(parent.expect("パスは空でない"))
    }

    /// キーワードを読む。
    pub fn keyword(&self, id: KeywordId) -> Result<Keyword> {
        self.conn
            .prepare_cached("SELECT id, parent_id, name FROM keyword WHERE id = ?1")?
            .query_row([id.get()], keyword_from_row)
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("キーワード {id}")))
    }

    /// キーワードの階層のパス（最上位から自分まで）の名前。
    pub fn keyword_path(&self, id: KeywordId) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "WITH RECURSIVE up(id, parent_id, name, depth) AS (
                 SELECT id, parent_id, name, 0 FROM keyword WHERE id = ?1
                 UNION ALL
                 SELECT k.id, k.parent_id, k.name, up.depth + 1
                 FROM keyword k JOIN up ON k.id = up.parent_id
                 WHERE up.depth < 1000
             )
             SELECT name FROM up ORDER BY depth DESC",
        )?;
        let names = stmt
            .query_map([id.get()], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if names.is_empty() {
            return Err(CatalogError::NotFound(format!("キーワード {id}")));
        }
        Ok(names)
    }

    /// 子のキーワードの一覧（`parent = None` なら最上位。名前の順）。
    pub fn keyword_children(&self, parent: Option<KeywordId>) -> Result<Vec<Keyword>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, parent_id, name FROM keyword
             WHERE ifnull(parent_id, 0) = ifnull(?1, 0) ORDER BY name_key, id",
        )?;
        let rows = stmt.query_map([parent.map(KeywordId::get)], keyword_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 名前でキーワードを探す（完全一致・前方一致。NFKC ＋ 小文字化して比較。3.6 節）。
    pub fn find_keywords(&self, query: &str, mode: KeywordMatch) -> Result<Vec<Keyword>> {
        let key = search_key(query.trim());
        // キーワードの名前は NUL を含まない（non_empty_name で拒否している）。NUL を含む検索語を
        // LIKE に渡すと NUL の位置でパターンが切れて意図しない一致になるため、一致なしとする。
        if key.is_empty() || key.contains('\0') {
            return Ok(Vec::new());
        }
        let rows = match mode {
            KeywordMatch::Exact => {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT id, parent_id, name FROM keyword WHERE name_key = ?1 ORDER BY name_key, id",
                )?;
                stmt.query_map([key], keyword_from_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
            KeywordMatch::Prefix => {
                let mut stmt = self.conn.prepare_cached(&format!(
                    "SELECT id, parent_id, name FROM keyword
                     WHERE name_key LIKE ?1 ESCAPE '{LIKE_ESCAPE}' ORDER BY name_key, id"
                ))?;
                stmt.query_map([like_prefix_pattern(&key)], keyword_from_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            }
        };
        Ok(rows)
    }

    /// キーワードを variant に付ける（付いていれば何もしない）。新しく付けた数を返す。
    pub fn add_keyword(&mut self, variant_ids: &[VariantId], keyword: KeywordId) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM keyword WHERE id = ?1)",
            [keyword.get()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(CatalogError::NotFound(format!("キーワード {keyword}")));
        }
        let n = tx.execute(
            "INSERT OR IGNORE INTO variant_keyword(variant_id, keyword_id)
             SELECT v.id, ?2 FROM variant v WHERE v.id IN (SELECT value FROM json_each(?1))",
            params![ids_to_json(variant_ids), keyword.get()],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// キーワードを variant から外す。外した数を返す。
    pub fn remove_keyword(
        &mut self,
        variant_ids: &[VariantId],
        keyword: KeywordId,
    ) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = tx.execute(
            "DELETE FROM variant_keyword
             WHERE keyword_id = ?2 AND variant_id IN (SELECT value FROM json_each(?1))",
            params![ids_to_json(variant_ids), keyword.get()],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// variant に付いているキーワード（名前の順）。
    pub fn keywords_of(&self, variant_id: VariantId) -> Result<Vec<Keyword>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT k.id, k.parent_id, k.name FROM keyword k
             JOIN variant_keyword vk ON vk.keyword_id = k.id
             WHERE vk.variant_id = ?1 ORDER BY k.name_key, k.id",
        )?;
        let rows = stmt.query_map([variant_id.get()], keyword_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// キーワードを削除する（子のキーワードと、variant への付与も削除される）。
    pub fn delete_keyword(&mut self, id: KeywordId) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM keyword WHERE id = ?1", [id.get()])?;
        if n == 0 {
            return Err(CatalogError::NotFound(format!("キーワード {id}")));
        }
        Ok(())
    }
}
