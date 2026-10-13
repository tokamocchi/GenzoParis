//! コレクション（LIB-10）・コレクションセット・スマートコレクション（LIB-11。docs/04_architecture.md の
//! 3.9 節）。
//!
//! - 種類は 3 つ（`collection.kind`）: 手動のコレクション（`manual`。variant を入れる）、スマート
//!   コレクション（`smart`。条件 [`SmartRule`] を `smart_rule_json` に保存し、検索のたびに SQL にする）、
//!   コレクションセット（`set`。ほかのコレクション・セットを入れる入れ物）。
//! - 入れ子はコレクションセットの中だけ（手動・スマートのコレクションの下には入れない）。自分・子孫の
//!   セットの下へは移せない。
//! - 同じ親の下で同じ名前（NFKC ＋ 小文字化で比較。種類によらない）は 1 つだけ（索引
//!   `collection_unique_name`。重なるときは [`CatalogError::NameConflict`]）。名前は
//!   [`MAX_COLLECTION_NAME_CHARS`] 文字までで、制御文字は使えない。
//! - メンバーは variant 単位（仮想コピーを別に入れられる）。`sort_order` は入れた順（同じ操作で入れた
//!   ものは渡した順）。写真を削除するとメンバーからも消える（`ON DELETE CASCADE`）。
//! - コレクションを削除すると、セットの中のコレクションとメンバーの記録も消える（写真は消えない）。
//! - スマートコレクションの条件は、コレクション（セットを含む）を参照できる。参照をたどって自分に戻る
//!   （循環する）条件・移動は [`CatalogError::InvalidInput`] で保存しない（[`check_smart_cycles`]）。
//!   外部のツールで循環を作られた場合は、検索で循環した参照を「どれにも一致しない」とみなす
//!   （`search.rs` の `collection_predicate`）。

use std::collections::{HashMap, HashSet};

use genzo_model::{CollectionId, CollectionKind, SmartRule, VariantId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::text::search_key;
use crate::util::{checked_name, ids_to_json, parse_enum};

/// コレクションの入れ子・参照をたどる深さの上限（**仮置き**: 64）。
pub const MAX_COLLECTION_DEPTH: usize = 64;

/// コレクションの名前の文字数の上限（Unicode のスカラー値の数。**仮置き**: 200。画面の入力欄と同じ）。
pub const MAX_COLLECTION_NAME_CHARS: usize = 200;

/// コレクション（[`Catalog::collections`]）。
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    /// ID。
    pub id: CollectionId,
    /// 親のコレクションセット（最上位は `None`）。
    pub parent_id: Option<CollectionId>,
    /// 名前。
    pub name: String,
    /// 種類。
    pub kind: CollectionKind,
    /// スマートコレクションの条件（読めた場合）。
    pub rule: Option<SmartRule>,
    /// スマートコレクションの条件を読めなかった理由（アプリより新しい版など）。
    pub rule_error: Option<String>,
    /// 手動のコレクションのメンバーの数（ほかの種類は 0。スマートコレクション・セットの数は検索で数える）。
    pub member_count: u64,
}

fn read_rule(kind: CollectionKind, json: Option<String>) -> (Option<SmartRule>, Option<String>) {
    match (kind, json) {
        (CollectionKind::Smart, Some(j)) => match SmartRule::from_json(&j) {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e.to_string())),
        },
        (CollectionKind::Smart, None) => (None, Some("条件がありません".to_owned())),
        _ => (None, None),
    }
}

/// コレクションの種類と親（なければ [`CatalogError::NotFound`]）。
pub(crate) fn kind_and_parent(
    conn: &Connection,
    id: CollectionId,
) -> Result<(CollectionKind, Option<CollectionId>)> {
    let row: Option<(String, Option<i64>)> = conn
        .prepare_cached("SELECT kind, parent_id FROM collection WHERE id = ?1")?
        .query_row([id.get()], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let (kind, parent) = row.ok_or_else(|| CatalogError::NotFound(format!("コレクション {id}")))?;
    Ok((parse_enum(&kind)?, parent.map(CollectionId::new)))
}

/// 親がコレクションセットであることを確かめる（`None` は最上位で、確かめない）。
fn check_parent_is_set(conn: &Connection, parent: Option<CollectionId>) -> Result<()> {
    if let Some(p) = parent {
        let (kind, _) = kind_and_parent(conn, p)?;
        if kind != CollectionKind::Set {
            return Err(CatalogError::InvalidInput(
                "コレクションはコレクションセットの中にだけ入れられます".to_owned(),
            ));
        }
    }
    Ok(())
}

/// 親の下に同じ名前（NFKC ＋ 小文字化で比較）のコレクションがないか（`except` は自分）。
fn check_name_free(
    conn: &Connection,
    parent: Option<CollectionId>,
    name: &str,
    except: Option<CollectionId>,
) -> Result<()> {
    let other: Option<i64> = conn
        .prepare_cached(
            "SELECT id FROM collection
             WHERE ifnull(parent_id, 0) = ifnull(?1, 0) AND name_key = ?2 AND id IS NOT ?3",
        )?
        .query_row(
            params![
                parent.map(CollectionId::get),
                search_key(name),
                except.map(CollectionId::get)
            ],
            |r| r.get(0),
        )
        .optional()?;
    if other.is_some() {
        return Err(CatalogError::NameConflict(format!(
            "同じ場所に「{name}」というコレクションがあります"
        )));
    }
    Ok(())
}

/// コレクションとその子孫の ID（自分を先頭に）。親子の循環があっても止まる（`UNION`）。
pub(crate) fn collection_descendants(
    conn: &Connection,
    id: CollectionId,
) -> Result<Vec<CollectionId>> {
    let mut stmt = conn.prepare_cached(
        "WITH RECURSIVE down(id) AS (
             SELECT id FROM collection WHERE id = ?1
             UNION
             SELECT c.id FROM collection c JOIN down ON c.parent_id = down.id
         )
         SELECT id FROM down",
    )?;
    let rows = stmt.query_map([id.get()], |r| r.get::<_, i64>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(CollectionId::new(r?));
    }
    Ok(out)
}

/// スマートコレクションの条件の参照が循環していないかを確かめる（[`CatalogError::InvalidInput`]）。
///
/// コレクションを点、「スマートコレクション → 条件で参照するコレクション」と「セット → 中のコレクション」を
/// 辺とした有向グラフで、スマートコレクションから自分に戻る道があれば循環とする（手動のコレクションは
/// 行き止まり）。`overrides` は保存する前の条件（まだカタログに書いていないもの）。カタログ全体を
/// 1 回読んで、メモリの中でたどる（コレクションの数は写真より桁違いに少ない）。
pub(crate) fn check_smart_cycles(
    conn: &Connection,
    overrides: &[(CollectionId, &SmartRule)],
) -> Result<()> {
    let mut kinds: HashMap<CollectionId, (CollectionKind, String)> = HashMap::new();
    let mut edges: HashMap<CollectionId, Vec<CollectionId>> = HashMap::new();
    {
        let mut stmt = conn
            .prepare_cached("SELECT id, parent_id, kind, name, smart_rule_json FROM collection")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                CollectionId::new(r.get(0)?),
                r.get::<_, Option<i64>>(1)?.map(CollectionId::new),
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        for r in rows {
            let (id, parent, kind, name, json) = r?;
            let kind: CollectionKind = parse_enum(&kind)?;
            if let Some(p) = parent {
                edges.entry(p).or_default().push(id);
            }
            if kind == CollectionKind::Smart {
                let refs = match overrides.iter().find(|(o, _)| *o == id) {
                    Some((_, rule)) => rule.referenced_collections(),
                    None => json
                        .and_then(|j| SmartRule::from_json(&j).ok())
                        .map(|r| r.referenced_collections())
                        .unwrap_or_default(),
                };
                edges.entry(id).or_default().extend(refs);
            }
            kinds.insert(id, (kind, name));
        }
    }
    // スマートコレクションごとに、辺をたどって自分に戻るかを調べる。
    for (&start, (kind, name)) in &kinds {
        if *kind != CollectionKind::Smart {
            continue;
        }
        let mut seen: HashSet<CollectionId> = HashSet::new();
        let mut stack: Vec<CollectionId> = edges.get(&start).cloned().unwrap_or_default();
        while let Some(c) = stack.pop() {
            if c == start {
                return Err(CatalogError::InvalidInput(format!(
                    "スマートコレクション「{name}」の条件が、たどると自分自身を参照します（循環する条件は保存できません）"
                )));
            }
            if !seen.insert(c) {
                continue;
            }
            if let Some(next) = edges.get(&c) {
                stack.extend(next.iter().copied());
            }
        }
    }
    Ok(())
}

impl Catalog {
    /// すべてのコレクション（名前の順。画面の木は `parent_id` で組む）。
    pub fn collections(&self) -> Result<Vec<Collection>> {
        let mut counts: HashMap<i64, u64> = HashMap::new();
        {
            let mut stmt = self.conn.prepare_cached(
                "SELECT collection_id, count(*) FROM collection_member GROUP BY collection_id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
            for r in rows {
                let (c, n) = r?;
                counts.insert(c, u64::try_from(n).unwrap_or(0));
            }
        }
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, parent_id, name, kind, smart_rule_json FROM collection ORDER BY name_key, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, parent, name, kind, json) = r?;
            let kind: CollectionKind = parse_enum(&kind)?;
            let (rule, rule_error) = read_rule(kind, json);
            out.push(Collection {
                id: CollectionId::new(id),
                parent_id: parent.map(CollectionId::new),
                name,
                kind,
                rule,
                rule_error,
                member_count: if kind == CollectionKind::Manual {
                    counts.get(&id).copied().unwrap_or(0)
                } else {
                    0
                },
            });
        }
        Ok(out)
    }

    /// コレクションを 1 つ読む。
    pub fn collection(&self, id: CollectionId) -> Result<Collection> {
        let row: Option<(Option<i64>, String, String, Option<String>)> = self
            .conn
            .prepare_cached(
                "SELECT parent_id, name, kind, smart_rule_json FROM collection WHERE id = ?1",
            )?
            .query_row([id.get()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()?;
        let (parent, name, kind, json) =
            row.ok_or_else(|| CatalogError::NotFound(format!("コレクション {id}")))?;
        let kind: CollectionKind = parse_enum(&kind)?;
        let (rule, rule_error) = read_rule(kind, json);
        let member_count: i64 = self.conn.query_row(
            "SELECT count(*) FROM collection_member WHERE collection_id = ?1",
            [id.get()],
            |r| r.get(0),
        )?;
        Ok(Collection {
            id,
            parent_id: parent.map(CollectionId::new),
            name,
            kind,
            rule,
            rule_error,
            member_count: u64::try_from(member_count).unwrap_or(0),
        })
    }

    /// コレクションを作る。`parent` はコレクションセット（`None` で最上位）。スマートコレクションは条件が
    /// 要り（検証し、循環する参照なら [`CatalogError::InvalidInput`]）、ほかの種類は条件を渡さない。
    pub fn create_collection(
        &mut self,
        parent: Option<CollectionId>,
        name: &str,
        kind: CollectionKind,
        rule: Option<&SmartRule>,
    ) -> Result<CollectionId> {
        let name = checked_name(name, "コレクションの名前", MAX_COLLECTION_NAME_CHARS)?;
        let rule_json = match (kind, rule) {
            (CollectionKind::Smart, Some(r)) => {
                r.validate()
                    .map_err(|e| CatalogError::InvalidInput(e.to_string()))?;
                Some(r.to_json())
            }
            (CollectionKind::Smart, None) => {
                return Err(CatalogError::InvalidInput(
                    "スマートコレクションには条件が要ります".to_owned(),
                ));
            }
            (_, Some(_)) => {
                return Err(CatalogError::InvalidInput(
                    "条件はスマートコレクションにだけ付けられます".to_owned(),
                ));
            }
            (_, None) => None,
        };
        let tx = self.conn.transaction()?;
        check_parent_is_set(&tx, parent)?;
        check_name_free(&tx, parent, name, None)?;
        tx.execute(
            "INSERT INTO collection(parent_id, name, name_key, kind, smart_rule_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                parent.map(CollectionId::get),
                name,
                search_key(name),
                kind.as_str(),
                rule_json
            ],
        )?;
        let id = CollectionId::new(tx.last_insert_rowid());
        if kind == CollectionKind::Smart {
            check_smart_cycles(&tx, &[])?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// コレクションの名前を変える（同じ場所に同じ名前があれば [`CatalogError::NameConflict`]）。
    pub fn rename_collection(&mut self, id: CollectionId, name: &str) -> Result<()> {
        let name = checked_name(name, "コレクションの名前", MAX_COLLECTION_NAME_CHARS)?;
        let tx = self.conn.transaction()?;
        let (_, parent) = kind_and_parent(&tx, id)?;
        check_name_free(&tx, parent, name, Some(id))?;
        tx.execute(
            "UPDATE collection SET name = ?2, name_key = ?3 WHERE id = ?1",
            params![id.get(), name, search_key(name)],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// コレクションを別のコレクションセットへ移す（`None` で最上位）。自分・子孫の下へは移せない。移すと
    /// スマートコレクションの参照が循環する場合（セットを参照する条件の中へ、その条件のスマートコレクションを
    /// 入れるなど）も [`CatalogError::InvalidInput`] で移さない。
    pub fn move_collection(
        &mut self,
        id: CollectionId,
        parent: Option<CollectionId>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let name: String = tx
            .query_row(
                "SELECT name FROM collection WHERE id = ?1",
                [id.get()],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("コレクション {id}")))?;
        check_parent_is_set(&tx, parent)?;
        if let Some(p) = parent
            && collection_descendants(&tx, id)?.contains(&p)
        {
            return Err(CatalogError::InvalidInput(
                "コレクションセットを自分や中のセットの中には移せません".to_owned(),
            ));
        }
        check_name_free(&tx, parent, &name, Some(id))?;
        tx.execute(
            "UPDATE collection SET parent_id = ?2 WHERE id = ?1",
            params![id.get(), parent.map(CollectionId::get)],
        )?;
        check_smart_cycles(&tx, &[])?;
        tx.commit()?;
        Ok(())
    }

    /// コレクションを削除する（セットなら中のコレクションも。メンバーの記録は消えるが、写真は消えない）。
    /// 削除したコレクションの数（中のものを含む）を返す。
    pub fn delete_collection(&mut self, id: CollectionId) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let ids = collection_descendants(&tx, id)?;
        if ids.is_empty() {
            return Err(CatalogError::NotFound(format!("コレクション {id}")));
        }
        tx.execute("DELETE FROM collection WHERE id = ?1", [id.get()])?;
        tx.commit()?;
        Ok(ids.len())
    }

    /// スマートコレクションの条件を変える（検証し、循環する参照なら [`CatalogError::InvalidInput`]）。
    pub fn set_smart_rule(&mut self, id: CollectionId, rule: &SmartRule) -> Result<()> {
        rule.validate()
            .map_err(|e| CatalogError::InvalidInput(e.to_string()))?;
        let tx = self.conn.transaction()?;
        let (kind, _) = kind_and_parent(&tx, id)?;
        if kind != CollectionKind::Smart {
            return Err(CatalogError::InvalidInput(
                "条件はスマートコレクションにだけ付けられます".to_owned(),
            ));
        }
        check_smart_cycles(&tx, &[(id, rule)])?;
        tx.execute(
            "UPDATE collection SET smart_rule_json = ?2 WHERE id = ?1",
            params![id.get(), rule.to_json()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 手動のコレクションに variant を入れる（入っているものは何もしない。存在しない variant は飛ばす）。
    /// 新しく入れた数を返す。手動のコレクションでなければ [`CatalogError::InvalidInput`]。
    pub fn add_to_collection(
        &mut self,
        id: CollectionId,
        variant_ids: &[VariantId],
    ) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let (kind, _) = kind_and_parent(&tx, id)?;
        if kind != CollectionKind::Manual {
            return Err(CatalogError::InvalidInput(
                "写真を入れられるのは手動のコレクションだけです（スマートコレクションは条件で決まり、セットにはコレクションを入れます）".to_owned(),
            ));
        }
        let start: i64 = tx.query_row(
            "SELECT ifnull(max(sort_order), 0) FROM collection_member WHERE collection_id = ?1",
            [id.get()],
            |r| r.get(0),
        )?;
        // 渡した順に sort_order を振る（json_each の key は配列の位置）。
        let n = tx.execute(
            "INSERT OR IGNORE INTO collection_member(collection_id, variant_id, sort_order)
             SELECT ?1, v.id, ?3 + j.key + 1
             FROM json_each(?2) AS j JOIN variant v ON v.id = j.value",
            params![id.get(), ids_to_json(variant_ids), start],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// 手動のコレクションから variant を外す（写真は消えない）。外した数を返す。
    pub fn remove_from_collection(
        &mut self,
        id: CollectionId,
        variant_ids: &[VariantId],
    ) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let (kind, _) = kind_and_parent(&tx, id)?;
        if kind != CollectionKind::Manual {
            return Err(CatalogError::InvalidInput(
                "写真を外せるのは手動のコレクションだけです".to_owned(),
            ));
        }
        let n = tx.execute(
            "DELETE FROM collection_member
             WHERE collection_id = ?1 AND variant_id IN (SELECT value FROM json_each(?2))",
            params![id.get(), ids_to_json(variant_ids)],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// variant が入っている手動のコレクション（名前の順）。
    pub fn collections_of_variant(&self, variant_id: VariantId) -> Result<Vec<CollectionId>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.id FROM collection_member cm JOIN collection c ON c.id = cm.collection_id
             WHERE cm.variant_id = ?1 ORDER BY c.name_key, c.id",
        )?;
        let rows = stmt.query_map([variant_id.get()], |r| r.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(CollectionId::new(r?));
        }
        Ok(out)
    }

    /// 手動のコレクションのメンバー（入れた順）。
    pub fn collection_members(&self, id: CollectionId) -> Result<Vec<VariantId>> {
        kind_and_parent(&self.conn, id)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT variant_id FROM collection_member WHERE collection_id = ?1
             ORDER BY sort_order, variant_id",
        )?;
        let rows = stmt.query_map([id.get()], |r| r.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(VariantId::new(r?));
        }
        Ok(out)
    }
}
