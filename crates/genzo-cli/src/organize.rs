//! 階層キーワード（LIB-09）・コレクション（LIB-10）・スマートコレクション（LIB-11）のコマンド
//! （`keyword`・`collection`。`search --keyword / --collection / --rule` の解釈も）。
//!
//! - キーワードは「場所>京都」のような階層のパス（「>」の前後の空白は無視。名前は NFKC ＋ 小文字化で比べる。
//!   コアの検索のキーと同じ）か、`id:12` の形の ID で指定する（名前が数字だけのキーワード「2024」と ID を
//!   取り違えないため）。
//! - コレクションは ID で指定する（同じ名前のコレクションが別のセットにありうるため。`collection list`）。
//! - スマートコレクションの条件は JSON のファイル（`-` で標準入力。UTF-8（BOM 付きも）・BOM 付きの UTF-16）。

use genzo_api::{
    CollectionId, CollectionInfo, CollectionKind, KeywordId, KeywordInfo, SmartRule, VariantId,
};
use serde_json::json;

use crate::Status;
use crate::args::{CollectionCommand, GlobalArgs, KeywordCommand};
use crate::error::{CliError, CliResult};
use crate::library::{ensure_variants, unique_ids};
use crate::output::{Output, Table, json_value};
use crate::session::{OpenOptions, Session};

/// 名前の比較用（NFKC ＋ 小文字化。カタログの `keyword.name_key` と同じ関数）。
fn name_key(s: &str) -> String {
    genzo_catalog::text::search_key(s.trim())
}

/// 階層のパスの文字列（「場所 > 京都」・「場所>京都」・全角の「＞」）を名前の並びにする（空の段は捨てる）。
pub fn parse_keyword_path(spec: &str) -> Vec<String> {
    spec.split(['>', '＞'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// キーワードのパス（最上位から）。
fn path_of(list: &[KeywordInfo], id: KeywordId) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = list.iter().find(|k| k.keyword_id == id);
    while let Some(k) = cur {
        out.push(k.name.clone());
        if out.len() > 1000 {
            break;
        }
        cur = k
            .parent_id
            .and_then(|p| list.iter().find(|x| x.keyword_id == p));
    }
    out.reverse();
    out
}

/// パスの表示（「場所 > 京都」）。
fn path_text(list: &[KeywordInfo], id: KeywordId) -> String {
    path_of(list, id).join(" > ")
}

/// キーワードの指定（パスか `id:N`）を、一覧から探す。なければ [`CliError::Usage`]。
pub fn resolve_keyword(list: &[KeywordInfo], spec: &str) -> CliResult<KeywordId> {
    if let Some(n) = spec.trim().strip_prefix("id:") {
        let id = n
            .trim()
            .parse::<i64>()
            .map_err(|_| CliError::Usage(format!("キーワードの ID は整数です: {spec}")))?;
        return list
            .iter()
            .find(|k| k.keyword_id.get() == id)
            .map(|k| k.keyword_id)
            .ok_or_else(|| CliError::Usage(format!("キーワード {spec} はありません")));
    }
    let path = parse_keyword_path(spec);
    if path.is_empty() {
        return Err(CliError::Usage(format!(
            "キーワードのパスが空です: {spec:?}"
        )));
    }
    let mut parent: Option<KeywordId> = None;
    for name in &path {
        let key = name_key(name);
        let found = list
            .iter()
            .find(|k| k.parent_id == parent && name_key(&k.name) == key)
            .ok_or_else(|| CliError::Usage(format!("キーワード「{spec}」はありません")))?;
        parent = Some(found.keyword_id);
    }
    Ok(parent.expect("パスは空でない"))
}

/// スマートコレクションの条件の JSON を読む（`-` なら標準入力）。
pub fn read_rule(source: &str) -> CliResult<SmartRule> {
    let name = if source == "-" {
        "（標準入力）"
    } else {
        source
    };
    let bytes = if source == "-" {
        let mut b = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut b)
            .map_err(|e| CliError::io(name, e))?;
        b
    } else {
        std::fs::read(source).map_err(|e| {
            CliError::classified(
                crate::error::io_kind(&e),
                format!("条件のファイル {source} を読めません: {e}"),
                None,
            )
        })?
    };
    let text = crate::develop::decode_text(&bytes).map_err(|why| {
        CliError::input(format!(
            "条件の JSON（{name}）を読めません: {why}（UTF-8 で保存してください）"
        ))
    })?;
    SmartRule::from_json(&text).map_err(|e| CliError::input(format!("条件の JSON（{name}）: {e}")))
}

fn print_ok(out: Output, text: &str) {
    if out.json {
        out.print_json(&json!({ "ok": true }));
    } else {
        out.line(text);
    }
}

/// `keyword`。
pub fn keyword(g: &GlobalArgs, out: Output, cmd: KeywordCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = keyword_in(&s, out, cmd);
    s.close()?;
    r
}

fn keyword_in(s: &Session, out: Output, cmd: KeywordCommand) -> CliResult<Status> {
    match cmd {
        KeywordCommand::List => {
            let list = s.core.keyword_tree()?;
            if out.json {
                let items: Vec<_> = list
                    .iter()
                    .map(|k| {
                        let mut v = json_value(k);
                        v["path"] = json!(path_of(&list, k.keyword_id));
                        v
                    })
                    .collect();
                out.print_json(&json!({ "keywords": items }));
            } else {
                let mut rows: Vec<(String, &KeywordInfo)> = list
                    .iter()
                    .map(|k| (path_text(&list, k.keyword_id), k))
                    .collect();
                rows.sort_by(|a, b| a.0.cmp(&b.0));
                let mut t = Table::new(["ID", "キーワード", "写真", "書き出し"]);
                for (p, k) in rows {
                    t.row([
                        k.keyword_id.to_string(),
                        p,
                        k.photo_count.to_string(),
                        if k.include_on_export {
                            "含める"
                        } else {
                            "含めない"
                        }
                        .to_owned(),
                    ]);
                }
                out.table(&t);
                out.line(format!("{} 個", list.len()));
            }
        }
        KeywordCommand::Add { variants, keywords } => {
            let ids = unique_ids(&variants);
            ensure_variants(s, &ids)?;
            let paths: Vec<Vec<String>> = keywords.iter().map(|k| parse_keyword_path(k)).collect();
            if let Some(i) = paths.iter().position(Vec::is_empty) {
                return Err(CliError::Usage(format!(
                    "キーワードのパスが空です: {:?}",
                    keywords[i]
                )));
            }
            let n = s.core.add_keywords(&ids, &[], &paths)?;
            if out.json {
                out.print_json(&json!({ "updated": n }));
            } else {
                out.line(format!("{n} 組（写真とキーワードの組）を付けました"));
            }
        }
        KeywordCommand::Remove { variants, keywords } => {
            let ids = unique_ids(&variants);
            ensure_variants(s, &ids)?;
            let list = s.core.keyword_tree()?;
            let kws = keywords
                .iter()
                .map(|k| resolve_keyword(&list, k))
                .collect::<CliResult<Vec<_>>>()?;
            let n = s.core.remove_keywords(&ids, &kws)?;
            if out.json {
                out.print_json(&json!({ "updated": n }));
            } else {
                out.line(format!("{n} 組（写真とキーワードの組）を外しました"));
            }
        }
        KeywordCommand::Show { variants } => {
            let ids = unique_ids(&variants);
            ensure_variants(s, &ids)?;
            let vk = s.core.variant_keywords(&ids)?;
            if out.json {
                out.print_json(&json!(vk));
            } else if vk.keywords.is_empty() {
                out.line("キーワードはありません");
            } else {
                let mut t = Table::new(["ID", "キーワード", "写真"]);
                for k in &vk.keywords {
                    t.row([
                        k.keyword_id.to_string(),
                        k.path.join(" > "),
                        format!("{} / {}", k.count, vk.variants),
                    ]);
                }
                out.table(&t);
            }
        }
        KeywordCommand::Create { path } => {
            let p = parse_keyword_path(&path);
            if p.is_empty() {
                return Err(CliError::Usage(format!(
                    "キーワードのパスが空です: {path:?}"
                )));
            }
            let id = s.core.create_keyword(None, &p)?;
            if out.json {
                out.print_json(&json!({ "keyword_id": id }));
            } else {
                out.line(format!("キーワード「{}」（ID {id}）", p.join(" > ")));
            }
        }
        KeywordCommand::Rename { keyword, name } => {
            let list = s.core.keyword_tree()?;
            let id = resolve_keyword(&list, &keyword)?;
            s.core.rename_keyword(id, &name)?;
            print_ok(out, "名前を変えました");
        }
        KeywordCommand::Move { keyword, to } => {
            let list = s.core.keyword_tree()?;
            let id = resolve_keyword(&list, &keyword)?;
            let parent = to.map(|t| resolve_keyword(&list, &t)).transpose()?;
            s.core.move_keyword(id, parent)?;
            print_ok(out, "移しました");
        }
        KeywordCommand::Delete { keyword } => {
            let list = s.core.keyword_tree()?;
            let id = resolve_keyword(&list, &keyword)?;
            let usage = s.core.keyword_usage(id)?;
            let n = s.core.delete_keyword(id)?;
            if out.json {
                out.print_json(&json!({ "deleted": n, "usage": usage }));
            } else {
                out.line(format!(
                    "キーワードを {n} 個削除しました（{} 枚の写真から外れました。写真は削除していません）",
                    usage.photos_with_descendants
                ));
                for c in &usage.smart_collections {
                    out.warn(format!(
                        "スマートコレクション「{}」（ID {}）の条件がこのキーワードを使っていました（条件の「含む」はどの写真にも一致しなくなり、「含まない」はすべての写真に一致するようになります）",
                        c.name, c.collection_id
                    ));
                }
            }
        }
        KeywordCommand::Export { keyword, on, off } => {
            let list = s.core.keyword_tree()?;
            let id = resolve_keyword(&list, &keyword)?;
            let include = on && !off;
            s.core.set_keyword_export(id, include)?;
            print_ok(
                out,
                if include {
                    "書き出すファイルに含めます"
                } else {
                    "書き出すファイルに含めません"
                },
            );
        }
    }
    Ok(Status::Success)
}

/// コレクション `id`（セットなら中のものも）を削除すると条件の参照先が消える、残るスマートコレクション
/// （一覧の順）。
fn smart_references(list: &[CollectionInfo], id: CollectionId) -> Vec<&CollectionInfo> {
    let mut removed = vec![id];
    let mut i = 0;
    while i < removed.len() {
        let parent = removed[i];
        for c in list {
            if c.parent_id == Some(parent) && !removed.contains(&c.collection_id) {
                removed.push(c.collection_id);
            }
        }
        i += 1;
    }
    list.iter()
        .filter(|c| c.kind == CollectionKind::Smart && !removed.contains(&c.collection_id))
        .filter(|c| {
            c.rule.as_ref().is_some_and(|r| {
                r.referenced_collections()
                    .iter()
                    .any(|x| removed.contains(x))
            })
        })
        .collect()
}

fn kind_text(k: CollectionKind) -> &'static str {
    match k {
        CollectionKind::Manual => "コレクション",
        CollectionKind::Smart => "スマート",
        CollectionKind::Set => "セット",
    }
}

/// コレクションのパス（最上位のセットから）。
fn collection_path(list: &[CollectionInfo], id: CollectionId) -> String {
    let mut out = Vec::new();
    let mut cur = list.iter().find(|c| c.collection_id == id);
    while let Some(c) = cur {
        out.push(c.name.clone());
        if out.len() > 1000 {
            break;
        }
        cur = c
            .parent_id
            .and_then(|p| list.iter().find(|x| x.collection_id == p));
    }
    out.reverse();
    out.join(" > ")
}

/// `collection`。
pub fn collection(g: &GlobalArgs, out: Output, cmd: CollectionCommand) -> CliResult<Status> {
    // 条件のファイルはカタログを開く前に読む（読めなければ何も変えない）。
    let rule = match &cmd {
        CollectionCommand::Create { rule: Some(r), .. }
        | CollectionCommand::Rule { set: Some(r), .. } => Some(read_rule(r)?),
        _ => None,
    };
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = collection_in(&s, out, cmd, rule);
    s.close()?;
    r
}

fn collection_in(
    s: &Session,
    out: Output,
    cmd: CollectionCommand,
    rule: Option<SmartRule>,
) -> CliResult<Status> {
    match cmd {
        CollectionCommand::List => {
            let list = s.core.collections(true)?;
            if out.json {
                out.print_json(&json!({ "collections": json_value(&list) }));
            } else {
                let mut rows: Vec<(String, &CollectionInfo)> = list
                    .iter()
                    .map(|c| (collection_path(&list, c.collection_id), c))
                    .collect();
                rows.sort_by(|a, b| a.0.cmp(&b.0));
                let mut t = Table::new(["ID", "種類", "コレクション", "写真"]);
                for (p, c) in rows {
                    t.row([
                        c.collection_id.to_string(),
                        kind_text(c.kind).to_owned(),
                        p,
                        match (&c.rule_error, c.count) {
                            (Some(_), _) => "（条件を読めません）".to_owned(),
                            (None, Some(n)) => n.to_string(),
                            (None, None) => "—".to_owned(),
                        },
                    ]);
                }
                out.table(&t);
                out.line(format!("{} 個", list.len()));
            }
        }
        CollectionCommand::Create {
            name,
            parent,
            kind,
            add,
            ..
        } => {
            let kind: CollectionKind = match (kind, &rule) {
                (Some(k), _) => k.into(),
                (None, Some(_)) => CollectionKind::Smart,
                (None, None) => CollectionKind::Manual,
            };
            let add = unique_ids(&add);
            if !add.is_empty() {
                if kind != CollectionKind::Manual {
                    return Err(CliError::Usage(
                        "--add は手動のコレクションにだけ使えます".to_owned(),
                    ));
                }
                ensure_variants(s, &add)?;
            }
            if kind == CollectionKind::Smart && rule.is_none() {
                return Err(CliError::Usage(
                    "スマートコレクションには --rule で条件を指定してください".to_owned(),
                ));
            }
            if kind != CollectionKind::Smart && rule.is_some() {
                return Err(CliError::Usage(
                    "--rule はスマートコレクションにだけ使えます".to_owned(),
                ));
            }
            let id = s
                .core
                .create_collection(parent, &name, kind, rule.as_ref(), &add)?;
            if out.json {
                out.print_json(&json!({ "collection_id": id }));
            } else {
                out.line(format!(
                    "{}「{name}」を作りました（ID {id}）",
                    kind_text(kind)
                ));
            }
        }
        CollectionCommand::Add {
            collection,
            variants,
        } => {
            let ids = unique_ids(&variants);
            ensure_variants(s, &ids)?;
            let n = s.core.add_to_collection(collection, &ids)?;
            if out.json {
                out.print_json(&json!({ "updated": n }));
            } else {
                out.line(format!("{n} 枚を入れました"));
            }
        }
        CollectionCommand::Remove {
            collection,
            variants,
        } => {
            let ids: Vec<VariantId> = unique_ids(&variants);
            let n = s.core.remove_from_collection(collection, &ids)?;
            if out.json {
                out.print_json(&json!({ "updated": n }));
            } else {
                out.line(format!("{n} 枚を外しました（写真は削除していません）"));
            }
        }
        CollectionCommand::Rename { collection, name } => {
            s.core.rename_collection(collection, &name)?;
            print_ok(out, "名前を変えました");
        }
        CollectionCommand::Move { collection, to } => {
            s.core.move_collection(collection, to)?;
            print_ok(out, "移しました");
        }
        CollectionCommand::Delete { collection } => {
            let list = s.core.collections(false)?;
            let refs = smart_references(&list, collection);
            let n = s.core.delete_collection(collection)?;
            if out.json {
                let refs: Vec<_> = refs
                    .iter()
                    .map(|c| json!({ "collection_id": c.collection_id, "name": c.name }))
                    .collect();
                out.print_json(&json!({ "deleted": n, "smart_collections": refs }));
            } else {
                out.line(format!(
                    "コレクションを {n} 個削除しました（写真は削除していません）"
                ));
                for c in refs {
                    out.warn(format!(
                        "スマートコレクション「{}」（ID {}）の条件がこのコレクションを使っていました（条件の「含まれる」はどの写真にも一致しなくなり、「含まれない」はすべての写真に一致するようになります）",
                        c.name, c.collection_id
                    ));
                }
            }
        }
        CollectionCommand::Rule { collection, .. } => {
            if let Some(rule) = rule {
                s.core.set_smart_rule(collection, &rule)?;
                print_ok(out, "条件を変えました");
            } else {
                let list = s.core.collections(false)?;
                let c = list
                    .iter()
                    .find(|c| c.collection_id == collection)
                    .ok_or_else(|| {
                        CliError::Usage(format!("コレクション {collection} はありません"))
                    })?;
                match (&c.rule, &c.rule_error) {
                    (Some(r), _) => {
                        if out.json {
                            out.print_json(r);
                        } else {
                            out.line(
                                serde_json::to_string_pretty(r)
                                    .map_err(|e| CliError::other(e.to_string()))?,
                            );
                        }
                    }
                    (None, Some(e)) => {
                        return Err(CliError::other(format!("条件を読めません: {e}")));
                    }
                    (None, None) => {
                        return Err(CliError::Usage(format!(
                            "コレクション {collection} はスマートコレクションではありません"
                        )));
                    }
                }
            }
        }
    }
    Ok(Status::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(id: i64, parent: Option<i64>, kind: CollectionKind, refers: &[i64]) -> CollectionInfo {
        use genzo_model::{MatchMode, MemberOp, SmartCondition};
        let rule = (kind == CollectionKind::Smart).then(|| {
            SmartRule::new(
                MatchMode::Any,
                vec![SmartCondition::Group {
                    match_mode: MatchMode::All,
                    rules: refers
                        .iter()
                        .map(|r| SmartCondition::Collection {
                            op: MemberOp::NotIn,
                            collection_id: CollectionId::new(*r),
                        })
                        .collect(),
                }],
            )
        });
        CollectionInfo {
            collection_id: CollectionId::new(id),
            parent_id: parent.map(CollectionId::new),
            name: format!("C{id}"),
            kind,
            rule,
            rule_error: None,
            count: None,
        }
    }

    #[test]
    fn smart_references_cover_nested_collections_and_exact_ids() {
        use CollectionKind::{Manual, Set, Smart};
        let list = vec![
            col(1, None, Manual, &[]),
            col(12, None, Manual, &[]),
            col(5, None, Set, &[]),
            col(6, Some(5), Manual, &[]),
            col(20, None, Smart, &[12]),
            col(21, None, Smart, &[1]),
            col(22, None, Smart, &[6]),
            // 削除されるセットの中のスマートコレクションは挙げない。
            col(7, Some(5), Smart, &[6]),
        ];
        let names = |id: i64| {
            smart_references(&list, CollectionId::new(id))
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(1), vec!["C21"]);
        assert_eq!(names(12), vec!["C20"]);
        assert_eq!(names(5), vec!["C22"]);
        assert!(names(20).is_empty());
    }

    fn kw(id: i64, parent: Option<i64>, name: &str) -> KeywordInfo {
        KeywordInfo {
            keyword_id: KeywordId::new(id),
            parent_id: parent.map(KeywordId::new),
            name: name.to_owned(),
            include_on_export: true,
            photo_count: 0,
        }
    }

    #[test]
    fn keyword_paths_are_split_on_greater_than_signs() {
        assert_eq!(parse_keyword_path("場所 > 京都"), vec!["場所", "京都"]);
        assert_eq!(
            parse_keyword_path("場所＞京都>>祇園 "),
            vec!["場所", "京都", "祇園"]
        );
        assert!(parse_keyword_path(" > ").is_empty());
    }

    #[test]
    fn keywords_resolve_by_path_or_id() {
        let list = vec![
            kw(1, None, "場所"),
            kw(2, Some(1), "京都"),
            kw(3, None, "2024"),
            kw(4, Some(3), "Tokyo"),
        ];
        assert_eq!(
            resolve_keyword(&list, "場所>京都").unwrap(),
            KeywordId::new(2)
        );
        assert_eq!(
            resolve_keyword(&list, " 場所 ＞ 京都 ").unwrap(),
            KeywordId::new(2)
        );
        // 名前が数字だけのキーワードは、パスとして探す（ID は id:N）。
        assert_eq!(resolve_keyword(&list, "2024").unwrap(), KeywordId::new(3));
        assert_eq!(resolve_keyword(&list, "id:2").unwrap(), KeywordId::new(2));
        // 大文字・小文字、全角・半角を区別しない。
        assert_eq!(
            resolve_keyword(&list, "２０２４>ＴＯＫＹＯ").unwrap(),
            KeywordId::new(4)
        );
        for bad in ["京都", "場所>大阪", "id:9", "id:x", ""] {
            assert!(
                matches!(resolve_keyword(&list, bad), Err(CliError::Usage(_))),
                "{bad}"
            );
        }
        assert_eq!(path_text(&list, KeywordId::new(4)), "2024 > Tokyo");
    }

    #[test]
    fn rules_are_read_from_files_with_a_bom() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rule.json");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(
            br#"{"version":1,"match":"any","rules":[{"type":"kind","value":"video"}]}"#,
        );
        std::fs::write(&p, &bytes).unwrap();
        let r = read_rule(p.to_str().unwrap()).unwrap();
        assert_eq!(r.rules.len(), 1);
        std::fs::write(&p, br#"{"version":9,"rules":[]}"#).unwrap();
        assert!(read_rule(p.to_str().unwrap()).is_err());
        assert!(read_rule(dir.path().join("none.json").to_str().unwrap()).is_err());
    }
}
