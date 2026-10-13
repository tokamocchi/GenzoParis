//! 文書（docs/catalog_schema.md）と実際のスキーマが食い違わないことのテスト（ORG-07）。
//!
//! - 実際の DB（新しく作ったカタログと thumbs.db）の `sqlite_schema`・`PRAGMA table_list`・
//!   `PRAGMA table_info` から、すべての表・ビューとその列・宣言した型、索引・トリガーを集め
//!   （[`crate::Catalog::describe_schema`]）、文書に載っているかを確かめる。
//! - 逆に、文書の表・ビューの節と列の行が、実際の DB にあるかも確かめる（消した表・列が文書に残らない）。
//! - 列の表に「NULL」「既定値」の欄があれば、`PRAGMA table_info` の `NOT NULL`（と主キー）・既定値と
//!   合うかも確かめる（「不可」は `NOT NULL` か主キー、「可」はそれ以外。既定値の欄は SQL の式を `` ` `` で
//!   囲んだもので、空なら既定値なし）。
//! - 文書のスキーマの版（1 章・2.1 節の例）と版の履歴の行が、このアプリの版と合うかを確かめる。
//!
//! 文書の書き方の決まり（このテストが読む形）:
//! - カタログの表・ビューは `<!-- schema:catalog -->` と `<!-- /schema -->` の間、サムネイル DB の表は
//!   `<!-- schema:thumbs -->` と `<!-- /schema -->` の間に書く。
//! - 表・ビューごとに `` ### `名前` `` の見出しを置き、その下の最初の表（見出しの行が `| 列 |` で
//!   始まるもの）に、列ごとに `` | `列の名前` | 型 | ... `` の行を書く。型は宣言した型（`INTEGER` など）、
//!   宣言した型のない列（ビューの式・FTS5 の列）は `（式）` か `—`。
//! - 索引・トリガーは、同じ範囲のどこかに `` `名前` `` で書く。

use std::collections::BTreeMap;

use super::Fixture;
use crate::{CATALOG_SCHEMA_VERSION, SchemaDescription, THUMBS_SCHEMA_VERSION, ThumbStore};

/// 文書の本文。
const DOC: &str = include_str!("../../../../docs/catalog_schema.md");

/// 文書の 1 つの表・ビューの節（列の名前と、その行の欄）。
#[derive(Debug, Default)]
struct DocTable {
    columns: BTreeMap<String, DocColumn>,
}

/// 文書の列の行。
#[derive(Debug, Default, PartialEq, Eq)]
struct DocColumn {
    /// 型の欄。
    ty: String,
    /// 「NULL」の欄（表に欄がなければ `None`）。
    null: Option<String>,
    /// 「既定値」の欄（表に欄がなければ `None`。欄が空なら空の文字列）。
    default: Option<String>,
}

/// `<!-- schema:{name} -->` から次の `<!-- /schema -->` までを返す。
fn section<'a>(doc: &'a str, name: &str) -> &'a str {
    let start_marker = format!("<!-- schema:{name} -->");
    let start = doc
        .find(&start_marker)
        .unwrap_or_else(|| panic!("文書に {start_marker} がない"))
        + start_marker.len();
    let len = doc[start..]
        .find("<!-- /schema -->")
        .unwrap_or_else(|| panic!("文書の {start_marker} の後に <!-- /schema --> がない"));
    &doc[start..start + len]
}

/// 範囲の中の `` ### `名前` `` の節と、その最初の表の列（名前 → 型の欄）。
fn doc_tables(section: &str) -> BTreeMap<String, DocTable> {
    let mut tables = BTreeMap::new();
    let mut current: Option<String> = None;
    // 今の節で、列の表を読んでいる途中か・読み終えたか。
    let mut in_columns = false;
    let mut columns_done = false;
    // 列の表の「NULL」「既定値」の欄の位置（`|` で分けた位置）。
    let mut null_at: Option<usize> = None;
    let mut default_at: Option<usize> = None;
    for line in section.lines() {
        if let Some(rest) = line.strip_prefix("### ") {
            current = rest
                .strip_prefix('`')
                .and_then(|r| r.split_once('`'))
                .map(|(name, _)| name.to_owned());
            if let Some(name) = &current {
                assert!(
                    tables.insert(name.clone(), DocTable::default()).is_none(),
                    "文書に {name} の節が 2 つある"
                );
            }
            in_columns = false;
            columns_done = false;
            continue;
        }
        let Some(name) = &current else { continue };
        if columns_done {
            continue;
        }
        if !in_columns {
            in_columns = line.starts_with("| 列 |");
            if in_columns {
                let header: Vec<&str> = line.split('|').map(str::trim).collect();
                null_at = header.iter().position(|h| *h == "NULL");
                default_at = header.iter().position(|h| *h == "既定値");
            }
            continue;
        }
        if !line.starts_with('|') {
            columns_done = true;
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // cells[0] は行の先頭の `|` の前の空、cells[1] が列の名前、cells[2] が型。
        let Some(col) = cells
            .get(1)
            .and_then(|c| c.strip_prefix('`'))
            .and_then(|c| c.strip_suffix('`'))
        else {
            // 区切りの行（|---|）。
            continue;
        };
        let cell =
            |at: Option<usize>| at.map(|i| cells.get(i).copied().unwrap_or_default().to_owned());
        let column = DocColumn {
            ty: cells.get(2).copied().unwrap_or_default().to_owned(),
            null: cell(null_at),
            default: cell(default_at),
        };
        let table = tables.get_mut(name).expect("節を作った");
        assert!(
            table.columns.insert(col.to_owned(), column).is_none(),
            "文書の {name} に列 {col} が 2 回ある"
        );
    }
    tables
}

/// 実際の DB の構造と、文書の範囲 `section_name` を比べる。食い違いの一覧を返す。
fn compare(actual: &SchemaDescription, section_name: &str) -> Vec<String> {
    let text = section(DOC, section_name);
    let documented = doc_tables(text);
    let mut problems = Vec::new();
    for table in &actual.tables {
        let Some(doc) = documented.get(&table.name) else {
            problems.push(format!(
                "{} {}（{}）の節がない",
                section_name, table.name, table.kind
            ));
            continue;
        };
        for col in &table.columns {
            match doc.columns.get(&col.name) {
                None => problems.push(format!("{}.{} の列の行がない", table.name, col.name)),
                Some(documented) => {
                    let ty = &documented.ty;
                    let ok = if col.decl_type.is_empty() {
                        ty == "（式）" || ty == "—"
                    } else {
                        ty == &col.decl_type
                    };
                    if !ok {
                        problems.push(format!(
                            "{}.{} の型が違う（文書: {ty:?}、実際: {:?}）",
                            table.name, col.name, col.decl_type
                        ));
                    }
                    if let Some(null) = &documented.null {
                        let expected = if col.not_null || col.primary_key > 0 {
                            "不可"
                        } else {
                            "可"
                        };
                        if null != expected {
                            problems.push(format!(
                                "{}.{} の NULL の欄が違う（文書: {null:?}、実際: {expected:?}）",
                                table.name, col.name
                            ));
                        }
                    }
                    if let Some(default) = &documented.default {
                        let doc_default = default
                            .strip_prefix('`')
                            .and_then(|d| d.strip_suffix('`'))
                            .unwrap_or(default);
                        let actual = col.default_value.as_deref().unwrap_or("");
                        if doc_default != actual {
                            problems.push(format!(
                                "{}.{} の既定値の欄が違う（文書: {default:?}、実際: {actual:?}）",
                                table.name, col.name
                            ));
                        }
                    }
                }
            }
        }
        for name in doc.columns.keys() {
            if !table.columns.iter().any(|c| &c.name == name) {
                problems.push(format!(
                    "{}.{} は文書にあるが実際にはない",
                    table.name, name
                ));
            }
        }
    }
    for name in documented.keys() {
        if actual.table(name).is_none() {
            problems.push(format!(
                "{section_name} の {name} は文書にあるが実際にはない"
            ));
        }
    }
    for object in &actual.objects {
        if matches!(object.kind.as_str(), "index" | "trigger")
            && !text.contains(&format!("`{}`", object.name))
        {
            problems.push(format!(
                "{} {}（{}）が文書にない",
                object.kind, object.name, object.table
            ));
        }
    }
    problems
}

#[test]
fn catalog_schema_document_covers_every_table_column_index_and_trigger() {
    let f = Fixture::new();
    let actual = f.cat.describe_schema().unwrap();
    let problems = compare(&actual, "catalog");
    assert!(
        problems.is_empty(),
        "docs/catalog_schema.md を実際のスキーマに合わせてください:\n{}",
        problems.join("\n")
    );
}

#[test]
fn thumbs_schema_document_covers_every_table_column_and_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = ThumbStore::open(dir.path().join("thumbs.db")).unwrap();
    let actual = store.describe_schema().unwrap();
    assert_eq!(actual.schema_version, THUMBS_SCHEMA_VERSION);
    let problems = compare(&actual, "thumbs");
    assert!(
        problems.is_empty(),
        "docs/catalog_schema.md を実際のスキーマに合わせてください:\n{}",
        problems.join("\n")
    );
}

#[test]
fn schema_document_states_the_current_versions_and_history() {
    assert!(
        DOC.contains(&format!(
            "catalog.db のスキーマの版: **{CATALOG_SCHEMA_VERSION}**"
        )),
        "1 章のカタログのスキーマの版を {CATALOG_SCHEMA_VERSION} にしてください"
    );
    assert!(
        DOC.contains(&format!(
            "thumbs.db のスキーマの版: **{THUMBS_SCHEMA_VERSION}**"
        )),
        "1 章のサムネイル DB のスキーマの版を {THUMBS_SCHEMA_VERSION} にしてください"
    );
    // 読み方の例（2.1 節）のコメントの版も合っている（ほかの版の数字を書いていない）。
    assert!(
        DOC.contains(&format!(
            "PRAGMA user_version;            -- スキーマの版（{CATALOG_SCHEMA_VERSION}）"
        )),
        "2.1 節の例の PRAGMA user_version のコメントを {CATALOG_SCHEMA_VERSION} にしてください"
    );
    assert_eq!(
        DOC.matches("スキーマの版（").count(),
        1,
        "「スキーマの版（N）」の形で版を書くのは 2.1 節の例だけにしてください"
    );
    // 版の履歴の表に、1 からこのアプリの版までの行がある。
    for v in 1..=CATALOG_SCHEMA_VERSION {
        assert!(
            DOC.contains(&format!("\n| {v} | ")),
            "1 章の版の履歴に版 {v} の行がない"
        );
    }
    assert!(
        !DOC.contains(&format!("\n| {} | ", CATALOG_SCHEMA_VERSION + 1)),
        "1 章の版の履歴に、まだない版 {} の行がある",
        CATALOG_SCHEMA_VERSION + 1
    );
}

#[test]
fn document_parser_reads_only_the_column_table() {
    // テスト自身の確認: 節の 2 つ目の表（キーの一覧など）は列として読まない。
    let text = "### `t`\n\n説明\n\n| 列 | 型 | 意味 |\n|---|---|---|\n| `a` | TEXT | 列 |\n| `b` | （式） | 式 |\n\n| キー | 値 |\n|---|---|\n| `k` | 値 |\n\n### `u`\n\n| 列 | 型 | NULL | 既定値 | 意味 |\n|---|---|---|---|---|\n| `c` | INTEGER | 不可 | `1` | 列 |\n| `d` | TEXT | 可 | | 列 |\n\n### 索引\n\n| `t_a` | |\n";
    let tables = doc_tables(text);
    let types = |name: &str| {
        tables[name]
            .columns
            .iter()
            .map(|(c, d)| (c.clone(), d.ty.clone(), d.null.clone(), d.default.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        types("t"),
        vec![
            ("a".to_owned(), "TEXT".to_owned(), None, None),
            ("b".to_owned(), "（式）".to_owned(), None, None)
        ]
    );
    assert_eq!(
        types("u"),
        vec![
            (
                "c".to_owned(),
                "INTEGER".to_owned(),
                Some("不可".to_owned()),
                Some("`1`".to_owned())
            ),
            (
                "d".to_owned(),
                "TEXT".to_owned(),
                Some("可".to_owned()),
                Some(String::new())
            )
        ]
    );
    assert_eq!(tables.len(), 2);
}

#[test]
fn comparison_reports_missing_and_stale_entries() {
    // テスト自身の確認: 文書にない列・型の違い・文書にない表・索引を見つける。
    let f = Fixture::new();
    let mut actual = f.cat.describe_schema().unwrap();
    let volume = actual
        .tables
        .iter_mut()
        .find(|t| t.name == "volume")
        .unwrap();
    volume.columns[1].decl_type = "BLOB".to_owned();
    // label は NULL を入れられる列、last_mount_path は既定値のない列。
    volume.columns[2].not_null = true;
    volume.columns[3].default_value = Some("'/'".to_owned());
    volume.columns.push(crate::ColumnInfo {
        name: "bogus".to_owned(),
        decl_type: "TEXT".to_owned(),
        not_null: false,
        default_value: None,
        primary_key: 0,
    });
    actual.tables.push(crate::TableInfo {
        name: "new_table".to_owned(),
        kind: "table".to_owned(),
        columns: Vec::new(),
    });
    actual.objects.push(crate::SchemaObject {
        kind: "index".to_owned(),
        name: "new_index".to_owned(),
        table: "volume".to_owned(),
        sql: String::new(),
    });
    // 文書にあるが実際にはない表。
    actual.tables.retain(|t| t.name != "stack");
    let problems = compare(&actual, "catalog");
    for expected in [
        "volume.uuid の型が違う",
        "volume.label の NULL の欄が違う",
        "volume.last_mount_path の既定値の欄が違う",
        "volume.bogus の列の行がない",
        "new_table（table）の節がない",
        "new_index（volume）が文書にない",
        "stack は文書にあるが実際にはない",
    ] {
        assert!(
            problems.iter().any(|p| p.contains(expected)),
            "{expected} が見つからない: {problems:?}"
        );
    }
}
