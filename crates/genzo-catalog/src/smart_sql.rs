//! スマートコレクションの条件（genzo-model の [`SmartRule`]。LIB-11）とコレクション（LIB-10）を、検索の
//! SQL の条件（`WHERE` に AND でつなぐ 1 つの式）にする（docs/04_architecture.md の 3.9 節）。
//!
//! - 条件の式は、variant（`v`）と、必要なら asset（`a`。[`SearchSql::needs_asset`] で結合する）の列だけを
//!   使う。キーワード・フォルダの階層は共通表式（`WITH RECURSIVE`）、コレクションのメンバー・テキスト・
//!   キーワードの有無は副問い合わせにする（既存の索引: `variant(rating)`・`asset(camera)`・
//!   `asset(lens)`・`asset(captured_at_utc)`・`variant_keyword(keyword_id, variant_id)`・
//!   `collection_member` の主キー・FTS5）。
//! - 値はすべてバインドする（利用者の入力を SQL に埋め込まない）。
//! - `NULL` の扱い: 否定（「含まれない」）は `NOT coalesce((式), 0)` にして、`NULL`（値の分からない列）を
//!   「一致しない」とみなしてから否定する。「等しくない」は `IS NOT` で、値の分からないものも一致させる
//!   （カメラが「X ではない」に、カメラの分からない写真も入る）。
//! - コレクションの参照: 手動のコレクションはメンバー、スマートコレクションはその条件、セットは中の
//!   コレクションのいずれか。参照をたどって同じコレクションに戻る（循環する）・深すぎる参照と、削除された
//!   コレクションは「どれにも一致しない」（`0`）とする（保存のときに循環は防いでいる。`collection.rs` の
//!   `check_smart_cycles`）。

use chrono::{Duration, Utc};
use genzo_model::{
    CollectionId, CollectionKind, ContainsOp, DateOp, EqualityOp, KeywordOp, MatchMode, MemberOp,
    NumberOp, SmartCondition, SmartRule, TextOp,
};
use rusqlite::{Connection, OptionalExtension};

use crate::collection::MAX_COLLECTION_DEPTH;
use crate::error::Result;
use crate::search::{FolderFilter, SearchSql};
use crate::text::{LIKE_ESCAPE, like_contains_pattern};
use crate::util::{parse_enum, utc_to_db_string};

/// 否定（`NULL` は「一致しない」とみなしてから否定する）。
fn not(expr: &str) -> String {
    format!("NOT coalesce(({expr}), 0)")
}

/// 条件の並びを、すべて / いずれかでつなぐ（空なら、すべて = 1、いずれか = 0）。
fn join(mode: MatchMode, parts: Vec<String>) -> String {
    match (mode, parts.len()) {
        (MatchMode::All, 0) => "1".to_owned(),
        (MatchMode::Any, 0) => "0".to_owned(),
        (_, 1) => parts.into_iter().next().expect("1 つある"),
        (MatchMode::All, _) => format!("({})", parts.join(" AND ")),
        (MatchMode::Any, _) => format!("({})", parts.join(" OR ")),
    }
}

/// 条件（最上位）を SQL の式にする。`stack` はたどっているコレクション（循環の検出）。
pub(crate) fn rule_predicate(
    conn: &Connection,
    q: &mut SearchSql,
    rule: &SmartRule,
    stack: &mut Vec<CollectionId>,
) -> Result<String> {
    group_predicate(conn, q, rule.match_mode, &rule.rules, stack)
}

fn group_predicate(
    conn: &Connection,
    q: &mut SearchSql,
    mode: MatchMode,
    rules: &[SmartCondition],
    stack: &mut Vec<CollectionId>,
) -> Result<String> {
    let mut parts = Vec::with_capacity(rules.len());
    for c in rules {
        parts.push(condition_predicate(conn, q, c, stack)?);
    }
    Ok(join(mode, parts))
}

/// 文字列の列（カメラ・レンズ）の条件。
fn text_column(q: &mut SearchSql, column: &str, op: TextOp, value: &str) -> String {
    q.needs_asset = true;
    match op {
        TextOp::Is => {
            let p = q.bind(value.to_owned());
            format!("{column} = {p}")
        }
        TextOp::IsNot => {
            let p = q.bind(value.to_owned());
            format!("{column} IS NOT {p}")
        }
        TextOp::Contains => {
            // LIKE は ASCII の英字の大文字・小文字を区別しない（SQLite の既定）。
            let p = q.bind(like_contains_pattern(value.trim()));
            format!("{column} LIKE {p} ESCAPE '{LIKE_ESCAPE}'")
        }
        TextOp::Unknown => format!("({column} IS NULL OR {column} = '')"),
    }
}

fn condition_predicate(
    conn: &Connection,
    q: &mut SearchSql,
    c: &SmartCondition,
    stack: &mut Vec<CollectionId>,
) -> Result<String> {
    Ok(match c {
        SmartCondition::Group { match_mode, rules } => {
            group_predicate(conn, q, *match_mode, rules, stack)?
        }
        SmartCondition::Rating { op, value } => {
            let p = q.bind(i64::from(*value));
            let o = match op {
                NumberOp::Eq => "=",
                NumberOp::Ne => "<>",
                NumberOp::Gte => ">=",
                NumberOp::Lte => "<=",
            };
            format!("v.rating {o} {p}")
        }
        SmartCondition::Flag { op, value } => {
            let p = q.bind(i64::from(*value));
            match op {
                EqualityOp::Is => format!("v.flag = {p}"),
                EqualityOp::IsNot => format!("v.flag <> {p}"),
            }
        }
        SmartCondition::ColorLabel { op, value } => {
            // IS は NULL（ラベルなし）どうしも等しいとみなす。
            let p = q.bind(value.map(|l| l.as_str().to_owned()));
            match op {
                EqualityOp::Is => format!("v.color_label IS {p}"),
                EqualityOp::IsNot => format!("v.color_label IS NOT {p}"),
            }
        }
        SmartCondition::Keyword { op, keyword_id } => match (op, keyword_id) {
            (KeywordOp::Has, Some(k)) => q.keyword_cond(*k),
            (KeywordOp::NotHas, Some(k)) => {
                let c = q.keyword_cond(*k);
                not(&c)
            }
            // 検証で弾いているが、読めた条件に値がなければ「どれにも一致しない」とする。
            (KeywordOp::Has | KeywordOp::NotHas, None) => "0".to_owned(),
            (KeywordOp::Any, _) => {
                "EXISTS (SELECT 1 FROM variant_keyword vk WHERE vk.variant_id = v.id)".to_owned()
            }
            (KeywordOp::None, _) => {
                "NOT EXISTS (SELECT 1 FROM variant_keyword vk WHERE vk.variant_id = v.id)"
                    .to_owned()
            }
        },
        SmartCondition::CaptureTime {
            op,
            from,
            until,
            days,
        } => {
            q.needs_asset = true;
            match op {
                DateOp::Range => {
                    let mut parts = Vec::new();
                    if let Some(f) = from {
                        let p = q.bind(utc_to_db_string(*f));
                        parts.push(format!("a.captured_at_utc >= {p}"));
                    }
                    if let Some(u) = until {
                        let p = q.bind(utc_to_db_string(*u));
                        parts.push(format!("a.captured_at_utc < {p}"));
                    }
                    if parts.is_empty() {
                        "a.captured_at_utc IS NOT NULL".to_owned()
                    } else {
                        join(MatchMode::All, parts)
                    }
                }
                DateOp::InLastDays => {
                    let d = i64::from(days.unwrap_or(0));
                    let since = Utc::now()
                        .checked_sub_signed(Duration::days(d))
                        .unwrap_or(chrono::DateTime::<Utc>::MIN_UTC);
                    let p = q.bind(utc_to_db_string(since));
                    format!("a.captured_at_utc >= {p}")
                }
                DateOp::Unknown => "a.captured_at_utc IS NULL".to_owned(),
            }
        }
        SmartCondition::Camera { op, value } => text_column(q, "a.camera", *op, value),
        SmartCondition::Lens { op, value } => text_column(q, "a.lens", *op, value),
        SmartCondition::Kind { value } => {
            q.needs_asset = true;
            let p = q.bind(value.as_str().to_owned());
            format!("a.kind = {p}")
        }
        SmartCondition::Text { op, value } => {
            // 検索語がない（区切りの文字だけ）なら、すべての写真が「含む」とみなす（絞り込みのテキストと同じ）。
            let c = q.text_cond(value).unwrap_or_else(|| "1".to_owned());
            match op {
                ContainsOp::Contains => c,
                ContainsOp::NotContains => not(&c),
            }
        }
        SmartCondition::Folder {
            op,
            folder_id,
            include_subfolders,
        } => {
            let c = q.folder_cond(FolderFilter {
                folder_id: *folder_id,
                include_subfolders: *include_subfolders,
            });
            match op {
                MemberOp::In => c,
                MemberOp::NotIn => not(&c),
            }
        }
        SmartCondition::Collection { op, collection_id } => {
            let c = collection_predicate(conn, q, *collection_id, stack)?;
            match op {
                MemberOp::In => c,
                MemberOp::NotIn => not(&c),
            }
        }
    })
}

/// コレクションに含まれる variant の条件（手動: メンバー、スマート: 条件、セット: 中のいずれか）。
pub(crate) fn collection_predicate(
    conn: &Connection,
    q: &mut SearchSql,
    id: CollectionId,
    stack: &mut Vec<CollectionId>,
) -> Result<String> {
    if stack.contains(&id) || stack.len() >= MAX_COLLECTION_DEPTH {
        // 参照が循環している・深すぎる（保存のときに防いでいるので、外部のツールで作られた場合だけ）。
        return Ok("0".to_owned());
    }
    let row: Option<(String, Option<String>)> = conn
        .prepare_cached("SELECT kind, smart_rule_json FROM collection WHERE id = ?1")?
        .query_row([id.get()], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((kind, json)) = row else {
        // 削除されたコレクション。
        return Ok("0".to_owned());
    };
    let kind: CollectionKind = parse_enum(&kind)?;
    stack.push(id);
    let out = match kind {
        CollectionKind::Manual => {
            let p = q.bind(id.get());
            format!(
                "v.id IN (SELECT cm.variant_id FROM collection_member cm WHERE cm.collection_id = {p})"
            )
        }
        CollectionKind::Smart => match json.as_deref().map(SmartRule::from_json) {
            Some(Ok(rule)) => rule_predicate(conn, q, &rule, stack)?,
            // 読めない条件（アプリより新しい版など。一覧では `Collection::rule_error` で知らせる）。
            Some(Err(_)) | None => "0".to_owned(),
        },
        CollectionKind::Set => {
            let children: Vec<(i64, String)> = {
                let mut stmt = conn.prepare_cached(
                    "SELECT id, kind FROM collection WHERE parent_id = ?1 ORDER BY id",
                )?;
                stmt.query_map([id.get()], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            // 手動のコレクションはまとめて 1 つの副問い合わせにする。
            let mut manual = Vec::new();
            let mut parts = Vec::new();
            for (child, child_kind) in children {
                let child = CollectionId::new(child);
                if parse_enum::<CollectionKind>(&child_kind)? == CollectionKind::Manual {
                    manual.push(child.get());
                } else {
                    parts.push(collection_predicate(conn, q, child, stack)?);
                }
            }
            if !manual.is_empty() {
                let list = q.bind_list(manual);
                parts.insert(
                    0,
                    format!(
                        "v.id IN (SELECT cm.variant_id FROM collection_member cm WHERE cm.collection_id IN {list})"
                    ),
                );
            }
            join(MatchMode::Any, parts)
        }
    };
    stack.pop();
    Ok(out)
}
