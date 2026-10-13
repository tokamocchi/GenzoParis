//! 階層キーワード（LIB-09。docs/04_architecture.md の 3.2 節・3.6 節・3.9 節）。
//!
//! - 同じ親の下で同じ名前（NFKC ＋ 小文字化で比較）のキーワードは 1 つだけ。名前の変更・移動で重なる
//!   ときは [`CatalogError::NameConflict`]（まとめない）。
//! - 名前は [`MAX_KEYWORD_NAME_CHARS`] 文字まで。画面の入力欄・CLI が区切りとして読む文字
//!   （[`KEYWORD_NAME_SEPARATORS`]）と制御文字は使えない（階層の表示「場所 > 京都」と区別できない名前を
//!   作らないため。決定事項ログ No.81）。
//! - 名前の検索は、全文検索ではなくキーワードのテーブルで、完全一致・前方一致・部分一致を行う（3.6 節。
//!   部分一致は入力欄の補完用。キーワードの数は写真より桁違いに少ないので、索引のない LIKE で足りる）。
//! - キーワードは variant に付ける（仮想コピーは別に付けられる）。asset や variant を削除しても、キーワード
//!   自体は残る（3.5 節）。
//! - キーワードを削除すると、子のキーワードと付与も削除される。
//! - 親のキーワードでの絞り込みは子のキーワードを含める（[`crate::Filter::keywords`]。再帰 CTE）。
//! - 書き出すファイルのメタデータ（XMP の `dc:subject`）には、付いているキーワードと、その親（含まれる
//!   キーワード）のうち `include_on_export` が真のものの名前を書く（[`Catalog::export_keywords`]。
//!   決定事項ログ No.81）。
//! - 親子の循環は作らない（移動で自分の下へは移せない）。外部のツールで循環を作られても、階層をたどる
//!   問い合わせは `UNION`（重複を除く）か深さの上限で止まる。

use std::collections::{HashMap, HashSet};

use genzo_model::{CollectionId, KeywordId, SmartRule, VariantId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::text::{LIKE_ESCAPE, like_contains_pattern, like_prefix_pattern, search_key};
use crate::util::{checked_name, ids_to_json};

/// キーワードの階層の深さの上限（たどる問い合わせを止める。**仮置き**: 64。公開ビュー
/// `photo_keywords_v1` と同じ）。
pub const MAX_KEYWORD_DEPTH: usize = 64;

/// キーワードの名前の文字数の上限（Unicode のスカラー値の数。**仮置き**: 200。コレクションの名前と同じ）。
pub const MAX_KEYWORD_NAME_CHARS: usize = 200;

/// キーワードの名前に使えない文字: 画面の入力欄が階層の区切り（「>」「＞」「|」）・複数の区切り
/// （「,」「，」「、」）として読む文字（CLI のパスの「>」「＞」も）。
pub const KEYWORD_NAME_SEPARATORS: &[char] = &['>', '＞', '|', ',', '，', '、'];

/// キーワードの階層のパス（`["場所", "京都"]`）の名前を、作らずに確かめる（取り込みの前に、付けるキーワードの
/// 誤りを知らせるため。IMP-05）。各段の規則は作るとき（[`Catalog::ensure_keyword`]）と同じ。空のパスもエラー。
pub fn check_keyword_path<S: AsRef<str>>(path: &[S]) -> Result<()> {
    if path.is_empty() {
        return Err(CatalogError::InvalidInput(
            "キーワードのパスが空です".to_owned(),
        ));
    }
    for name in path {
        keyword_name(name.as_ref())?;
    }
    Ok(())
}

/// キーワードの名前を確かめる（空・長すぎ・制御文字・区切りの文字）。前後の空白を除いた名前を返す。
fn keyword_name(name: &str) -> Result<&str> {
    let name = checked_name(name, "キーワードの名前", MAX_KEYWORD_NAME_CHARS)?;
    if let Some(c) = name.chars().find(|c| KEYWORD_NAME_SEPARATORS.contains(c)) {
        return Err(CatalogError::InvalidInput(format!(
            "キーワードの名前に「{c}」は使えません（階層や複数のキーワードの区切りに使う文字です）"
        )));
    }
    Ok(name)
}

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

/// キーワードの一覧の 1 件（[`Catalog::keyword_tree`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeywordNode {
    /// ID。
    pub id: KeywordId,
    /// 親（最上位は `None`）。
    pub parent_id: Option<KeywordId>,
    /// 名前（表示用）。
    pub name: String,
    /// 書き出すファイルのメタデータに書くか。
    pub include_on_export: bool,
    /// このキーワードが直接付いている variant の数（子のキーワードの分は数えない。Lightroom の
    /// キーワードの一覧と同じ）。
    pub photo_count: u64,
}

/// キーワードの使われ方（削除の確認用。[`Catalog::keyword_usage`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeywordUsage {
    /// このキーワードが直接付いている variant の数。
    pub photos: u64,
    /// このキーワードか子孫のキーワードが付いている variant の数（削除で付与が消える写真の数）。
    pub photos_with_descendants: u64,
    /// 子孫のキーワードの数（削除で一緒に消える数）。
    pub descendants: u64,
    /// このキーワードか子孫を条件に使っているスマートコレクション（ID と名前。名前の順）。
    pub smart_collections: Vec<(CollectionId, String)>,
}

/// キーワードの名前の検索の方法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordMatch {
    /// 完全一致（NFKC ＋ 小文字化して比較）。
    Exact,
    /// 前方一致（NFKC ＋ 小文字化して比較）。
    Prefix,
    /// 部分一致（NFKC ＋ 小文字化して比較。入力欄の補完用）。
    Contains,
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
    let name = keyword_name(name)?;
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
        self.ensure_keyword_path_in(None, path)
    }

    /// `parent` の下（`None` で最上位）に、階層のパスでキーワードを確保する（1 つのトランザクション）。
    /// 最後のキーワードを返す。
    pub fn ensure_keyword_path_in<S: AsRef<str>>(
        &mut self,
        parent: Option<KeywordId>,
        path: &[S],
    ) -> Result<KeywordId> {
        if path.is_empty() {
            return Err(CatalogError::InvalidInput(
                "キーワードのパスが空です".to_owned(),
            ));
        }
        let tx = self.conn.transaction()?;
        if let Some(p) = parent {
            keyword_parent_tx(&tx, p)?;
        }
        let mut parent = parent;
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
            KeywordMatch::Prefix | KeywordMatch::Contains => {
                let pattern = if mode == KeywordMatch::Prefix {
                    like_prefix_pattern(&key)
                } else {
                    like_contains_pattern(&key)
                };
                // 前方一致のものを先に並べる（補完の候補の順）。
                let mut stmt = self.conn.prepare_cached(&format!(
                    "SELECT id, parent_id, name FROM keyword
                     WHERE name_key LIKE ?1 ESCAPE '{LIKE_ESCAPE}'
                     ORDER BY (name_key LIKE ?2 ESCAPE '{LIKE_ESCAPE}') DESC, name_key, id"
                ))?;
                stmt.query_map(
                    params![pattern, like_prefix_pattern(&key)],
                    keyword_from_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
            }
        };
        Ok(rows)
    }

    /// キーワードを variant に付ける（付いていれば何もしない）。新しく付けた数を返す。
    pub fn add_keyword(&mut self, variant_ids: &[VariantId], keyword: KeywordId) -> Result<usize> {
        self.add_keywords(variant_ids, &[keyword])
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

    /// キーワードを削除する（子のキーワードと、variant への付与も削除される）。削除したキーワードの数
    /// （子孫を含む）を返す。
    pub fn delete_keyword(&mut self, id: KeywordId) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let ids = descendants_tx(&tx, id)?;
        if ids.is_empty() {
            return Err(CatalogError::NotFound(format!("キーワード {id}")));
        }
        tx.execute("DELETE FROM keyword WHERE id = ?1", [id.get()])?;
        tx.commit()?;
        Ok(ids.len())
    }

    /// すべてのキーワード（名前の順。画面のキーワードの一覧の木は `parent_id` で組む）と、直接付いている
    /// variant の数。数は `variant_keyword(keyword_id, variant_id)` の索引だけを読んで数える。
    pub fn keyword_tree(&self) -> Result<Vec<KeywordNode>> {
        let mut counts: HashMap<i64, u64> = HashMap::new();
        {
            let mut stmt = self.conn.prepare_cached(
                "SELECT keyword_id, count(*) FROM variant_keyword GROUP BY keyword_id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
            for r in rows {
                let (k, n) = r?;
                counts.insert(k, u64::try_from(n).unwrap_or(0));
            }
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, parent_id, name, include_on_export FROM keyword ORDER BY name_key, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(KeywordNode {
                id: KeywordId::new(r.get(0)?),
                parent_id: r.get::<_, Option<i64>>(1)?.map(KeywordId::new),
                name: r.get(2)?,
                include_on_export: r.get::<_, i64>(3)? != 0,
                photo_count: 0,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            let mut node = r?;
            node.photo_count = counts.get(&node.id.get()).copied().unwrap_or(0);
            out.push(node);
        }
        Ok(out)
    }

    /// キーワードの名前を変える。同じ親の下に同じ名前（NFKC ＋ 小文字化で比較）の別のキーワードがあれば
    /// [`CatalogError::NameConflict`]（まとめない）。名前の規則はモジュールの doc。
    pub fn rename_keyword(&mut self, id: KeywordId, name: &str) -> Result<()> {
        let name = keyword_name(name)?;
        let tx = self.conn.transaction()?;
        let parent = keyword_parent_tx(&tx, id)?;
        check_keyword_name_free(&tx, parent, name, Some(id))?;
        tx.execute(
            "UPDATE keyword SET name = ?2, name_key = ?3 WHERE id = ?1",
            params![id.get(), name, search_key(name)],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// キーワードを別の親の下へ移す（`None` で最上位）。自分・子孫の下へは移せない（[`CatalogError::InvalidInput`]）。
    /// 移す先に同じ名前があれば [`CatalogError::NameConflict`]。
    pub fn move_keyword(&mut self, id: KeywordId, parent: Option<KeywordId>) -> Result<()> {
        let tx = self.conn.transaction()?;
        let name: String = tx
            .query_row("SELECT name FROM keyword WHERE id = ?1", [id.get()], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("キーワード {id}")))?;
        if let Some(p) = parent {
            keyword_parent_tx(&tx, p)?;
            if descendants_tx(&tx, id)?.contains(&p) {
                return Err(CatalogError::InvalidInput(
                    "キーワードを自分や子のキーワードの下には移せません".to_owned(),
                ));
            }
        }
        check_keyword_name_free(&tx, parent, &name, Some(id))?;
        tx.execute(
            "UPDATE keyword SET parent_id = ?2 WHERE id = ?1",
            params![id.get(), parent.map(KeywordId::get)],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 書き出すファイルのメタデータにキーワードを書くかを変える。
    pub fn set_keyword_include_on_export(&mut self, id: KeywordId, include: bool) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE keyword SET include_on_export = ?2 WHERE id = ?1",
            params![id.get(), i64::from(include)],
        )?;
        if n == 0 {
            return Err(CatalogError::NotFound(format!("キーワード {id}")));
        }
        Ok(())
    }

    /// キーワードの使われ方（削除の確認用）。
    pub fn keyword_usage(&self, id: KeywordId) -> Result<KeywordUsage> {
        let ids = descendants_tx(&self.conn, id)?;
        if ids.is_empty() {
            return Err(CatalogError::NotFound(format!("キーワード {id}")));
        }
        let photos: i64 = self.conn.query_row(
            "SELECT count(*) FROM variant_keyword WHERE keyword_id = ?1",
            [id.get()],
            |r| r.get(0),
        )?;
        let photos_with_descendants: i64 = self.conn.query_row(
            "SELECT count(DISTINCT variant_id) FROM variant_keyword
             WHERE keyword_id IN (SELECT value FROM json_each(?1))",
            [ids_to_json(&ids)],
            |r| r.get(0),
        )?;
        let set: HashSet<KeywordId> = ids.iter().copied().collect();
        let mut smart = Vec::new();
        for (cid, name, rule) in smart_rules(&self.conn)? {
            if let Some(rule) = rule
                && rule.referenced_keywords().iter().any(|k| set.contains(k))
            {
                smart.push((cid, name));
            }
        }
        Ok(KeywordUsage {
            photos: u64::try_from(photos).unwrap_or(0),
            photos_with_descendants: u64::try_from(photos_with_descendants).unwrap_or(0),
            descendants: (ids.len() - 1) as u64,
            smart_collections: smart,
        })
    }

    /// 複数のキーワードを複数の variant に付ける（1 つのトランザクション。付いているものは何もしない）。
    /// 新しく付けた（variant, キーワード）の組の数を返す。キーワードがなければ [`CatalogError::NotFound`]
    /// で、何も付けない。存在しない variant は飛ばす。
    pub fn add_keywords(
        &mut self,
        variant_ids: &[VariantId],
        keyword_ids: &[KeywordId],
    ) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = add_keywords_tx(&tx, variant_ids, keyword_ids)?;
        tx.commit()?;
        Ok(n)
    }

    /// 階層のパス（`[["場所", "京都"], ["家族"]]`）でキーワードを確保して（なければ作って）、variant に付ける
    /// （1 つのトランザクション）。確保したキーワードと、新しく付けた組の数を返す。
    pub fn add_keyword_paths<S: AsRef<str>>(
        &mut self,
        variant_ids: &[VariantId],
        paths: &[Vec<S>],
    ) -> Result<(Vec<KeywordId>, usize)> {
        self.add_keywords_with_paths(variant_ids, &[], paths)
    }

    /// 既存のキーワード（`keyword_ids`）と、階層のパスで確保した（なければ作った）キーワードを、variant に
    /// 付ける（すべて 1 つのトランザクション。どれかが失敗したら、作ったキーワードも残さない）。付けた
    /// キーワード（重複なし。パスのものが先）と、新しく付けた組の数を返す。
    pub fn add_keywords_with_paths<S: AsRef<str>>(
        &mut self,
        variant_ids: &[VariantId],
        keyword_ids: &[KeywordId],
        paths: &[Vec<S>],
    ) -> Result<(Vec<KeywordId>, usize)> {
        let tx = self.conn.transaction()?;
        let mut ids = Vec::new();
        for path in paths {
            if path.is_empty() {
                return Err(CatalogError::InvalidInput(
                    "キーワードのパスが空です".to_owned(),
                ));
            }
            let mut parent = None;
            for name in path {
                parent = Some(ensure_keyword_tx(&tx, parent, name.as_ref())?);
            }
            let id = parent.expect("パスは空でない");
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        for k in keyword_ids {
            if !ids.contains(k) {
                ids.push(*k);
            }
        }
        let n = add_keywords_tx(&tx, variant_ids, &ids)?;
        tx.commit()?;
        Ok((ids, n))
    }

    /// 複数のキーワードを複数の variant から外す（1 つのトランザクション）。外した組の数を返す。
    /// 子のキーワードは外さない（付いているキーワードそのものだけ）。
    pub fn remove_keywords(
        &mut self,
        variant_ids: &[VariantId],
        keyword_ids: &[KeywordId],
    ) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = tx.execute(
            "DELETE FROM variant_keyword
             WHERE variant_id IN (SELECT value FROM json_each(?1))
               AND keyword_id IN (SELECT value FROM json_each(?2))",
            params![ids_to_json(variant_ids), ids_to_json(keyword_ids)],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// 複数の variant に付いているキーワードと、そのキーワードが付いている variant の数（名前の順。
    /// 画面の「選択中の写真のキーワード」。数が variant の数より少なければ一部の写真だけに付いている）。
    pub fn keywords_of_variants(&self, variant_ids: &[VariantId]) -> Result<Vec<(Keyword, u64)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT k.id, k.parent_id, k.name, count(*) FROM variant_keyword vk
             JOIN keyword k ON k.id = vk.keyword_id
             WHERE vk.variant_id IN (SELECT DISTINCT value FROM json_each(?1))
             GROUP BY k.id ORDER BY k.name_key, k.id",
        )?;
        let rows = stmt.query_map([ids_to_json(variant_ids)], |r| {
            Ok((
                keyword_from_row(r)?,
                u64::try_from(r.get::<_, i64>(3)?).unwrap_or(0),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 複数のキーワードの階層のパス（最上位から自分までの名前）。存在しないキーワードは含めない。
    /// キーワードの表を 1 回読んで、メモリの中でたどる（キーワードの数は少ない）。
    pub fn keyword_paths(&self, ids: &[KeywordId]) -> Result<HashMap<KeywordId, Vec<String>>> {
        let mut all: HashMap<i64, (Option<i64>, String)> = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT id, parent_id, name FROM keyword")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for r in rows {
                let (id, parent, name) = r?;
                all.insert(id, (parent, name));
            }
        }
        let mut out = HashMap::new();
        for &id in ids {
            let mut path = Vec::new();
            let mut cur = Some(id.get());
            while let Some(c) = cur {
                let Some((parent, name)) = all.get(&c) else {
                    break;
                };
                path.push(name.clone());
                if path.len() >= MAX_KEYWORD_DEPTH {
                    break;
                }
                cur = *parent;
            }
            if !path.is_empty() {
                path.reverse();
                out.insert(id, path);
            }
        }
        Ok(out)
    }

    /// 書き出すファイルのメタデータ（XMP の `dc:subject`）に書くキーワードの名前（決定事項ログ No.81）:
    /// variant に付いているキーワードと、その親（含まれるキーワード）のうち `include_on_export` が真の
    /// もの。同じ名前は 1 つにまとめ、名前の順に並べる。
    pub fn export_keywords(&self, variant_id: VariantId) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "WITH RECURSIVE up(id, depth) AS (
                 SELECT keyword_id, 0 FROM variant_keyword WHERE variant_id = ?1
                 UNION
                 SELECT k.parent_id, up.depth + 1 FROM keyword k JOIN up ON k.id = up.id
                 WHERE k.parent_id IS NOT NULL AND up.depth < {MAX_KEYWORD_DEPTH}
             )
             SELECT k.name, min(k.name_key) AS nk FROM keyword k
             WHERE k.id IN (SELECT id FROM up) AND k.include_on_export = 1
             GROUP BY k.name ORDER BY nk, k.name"
        ))?;
        let rows = stmt.query_map([variant_id.get()], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// キーワードとその子孫の ID（自分を先頭に。なければ空）。親子の循環があっても止まる（`UNION`）。
pub(crate) fn descendants_tx(conn: &Connection, id: KeywordId) -> Result<Vec<KeywordId>> {
    let mut stmt = conn.prepare_cached(
        "WITH RECURSIVE down(id) AS (
             SELECT id FROM keyword WHERE id = ?1
             UNION
             SELECT k.id FROM keyword k JOIN down ON k.parent_id = down.id
         )
         SELECT id FROM down",
    )?;
    let rows = stmt.query_map([id.get()], |r| r.get::<_, i64>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(KeywordId::new(r?));
    }
    Ok(out)
}

/// キーワードの親（なければ [`CatalogError::NotFound`]）。
fn keyword_parent_tx(conn: &Connection, id: KeywordId) -> Result<Option<KeywordId>> {
    conn.prepare_cached("SELECT parent_id FROM keyword WHERE id = ?1")?
        .query_row([id.get()], |r| r.get::<_, Option<i64>>(0))
        .optional()?
        .map(|p| p.map(KeywordId::new))
        .ok_or_else(|| CatalogError::NotFound(format!("キーワード {id}")))
}

/// 親の下に同じ名前（NFKC ＋ 小文字化で比較）のキーワードがないか（`except` は自分）。
fn check_keyword_name_free(
    conn: &Connection,
    parent: Option<KeywordId>,
    name: &str,
    except: Option<KeywordId>,
) -> Result<()> {
    let other: Option<i64> = conn
        .prepare_cached(
            "SELECT id FROM keyword
             WHERE ifnull(parent_id, 0) = ifnull(?1, 0) AND name_key = ?2 AND id IS NOT ?3",
        )?
        .query_row(
            params![
                parent.map(KeywordId::get),
                search_key(name),
                except.map(KeywordId::get)
            ],
            |r| r.get(0),
        )
        .optional()?;
    if other.is_some() {
        return Err(CatalogError::NameConflict(format!(
            "同じ階層に「{name}」というキーワードがあります"
        )));
    }
    Ok(())
}

pub(crate) fn add_keywords_tx(
    conn: &Connection,
    variant_ids: &[VariantId],
    keyword_ids: &[KeywordId],
) -> Result<usize> {
    let mut n = 0;
    for &keyword in keyword_ids {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM keyword WHERE id = ?1)",
            [keyword.get()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(CatalogError::NotFound(format!("キーワード {keyword}")));
        }
        n += conn.execute(
            "INSERT OR IGNORE INTO variant_keyword(variant_id, keyword_id)
             SELECT v.id, ?2 FROM variant v WHERE v.id IN (SELECT value FROM json_each(?1))",
            params![ids_to_json(variant_ids), keyword.get()],
        )?;
    }
    Ok(n)
}

/// すべてのスマートコレクションの ID・名前・条件（読めない条件は `None`。名前の順）。
pub(crate) fn smart_rules(
    conn: &Connection,
) -> Result<Vec<(CollectionId, String, Option<SmartRule>)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, name, smart_rule_json FROM collection WHERE kind = 'smart' ORDER BY name_key, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            CollectionId::new(r.get(0)?),
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        let (id, name, json) = r?;
        let rule = json.and_then(|j| SmartRule::from_json(&j).ok());
        out.push((id, name, rule));
    }
    Ok(out)
}
