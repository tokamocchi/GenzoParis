//! テキスト検索（3.6 節。レビュー R-11）のテスト。
//!
//! PoC-6 の確認項目: 「京都」「海」「夕焼け」「京都旅行」、英数字の混在、全角・半角、検索語のエスケープ。

use genzo_model::{AssetId, VariantId};

use super::Fixture;
use crate::{Filter, Sort, SortDirection, SortKey};

struct TextFixture {
    f: Fixture,
    /// 名前 → (asset, マスターの variant)
    items: Vec<(&'static str, AssetId, VariantId)>,
}

impl TextFixture {
    fn id(&self, label: &str) -> VariantId {
        self.items.iter().find(|(l, _, _)| *l == label).unwrap().2
    }

    fn search(&self, text: &str) -> Vec<VariantId> {
        self.f
            .cat
            .search(
                &Filter {
                    text: Some(text.to_owned()),
                    ..Default::default()
                },
                &Sort::new(SortKey::ImportOrder, SortDirection::Ascending),
            )
            .unwrap()
    }

    fn labels(&self, text: &str) -> Vec<&'static str> {
        self.search(text)
            .into_iter()
            .map(|v| self.items.iter().find(|(_, _, id)| *id == v).unwrap().0)
            .collect()
    }
}

/// (ラベル, ファイル名, キャプション)
const ITEMS: &[(&str, &str, Option<&str>)] = &[
    ("kyoto_trip", "DSC0001.ARW", Some("京都旅行の初日")),
    ("kyoto", "DSC0002.ARW", Some("京都")),
    ("sea", "DSC0003.ARW", Some("海の夕焼け")),
    ("sunset", "DSC0004.ARW", Some("夕焼けと富士山")),
    ("mixed", "IMG_2024京都A1.JPG", None),
    ("fullwidth", "DSC0006.ARW", Some("ＡＢＣカメラ")),
    ("halfwidth", "DSC0007.ARW", Some("abcカメラ")),
    ("upper", "DSC0008.ARW", Some("Tokyo NIGHT")),
    ("ga", "DSC0009.ARW", Some("がっこう")),
    ("ka", "DSC0010.ARW", Some("かっこう")),
    (
        "special",
        "DSC0011.ARW",
        Some("50% off_sale \\path \"quoted\" a*b (x) -y NEAR OR AND ^z:w"),
    ),
    ("halfkana", "DSC0012.ARW", Some("ｶﾞｯｺｳ")),
];

fn text_fixture() -> TextFixture {
    let mut f = Fixture::new();
    let mut items = Vec::new();
    for (i, (label, name, caption)) in ITEMS.iter().enumerate() {
        let o = f.photo(name, i as u64, None);
        f.cat.set_caption(o.asset_id, *caption).unwrap();
        items.push((*label, o.asset_id, o.master_variant_id));
    }
    TextFixture { f, items }
}

#[test]
fn partial_match_in_japanese() {
    let t = text_fixture();
    // 「京都」（2 文字、LIKE）で「京都旅行」も見つかる。ファイル名の中の「京都」も見つかる。
    assert_eq!(t.labels("京都"), vec!["kyoto_trip", "kyoto", "mixed"]);
    // 「京都旅行」（4 文字、FTS5 の trigram）。
    assert_eq!(t.labels("京都旅行"), vec!["kyoto_trip"]);
    // 「海」（1 文字）。
    assert_eq!(t.labels("海"), vec!["sea"]);
    // 「夕焼け」（3 文字）。
    assert_eq!(t.labels("夕焼け"), vec!["sea", "sunset"]);
    // 見つからない語。
    assert!(t.labels("大阪").is_empty());
    assert!(t.labels("大阪城").is_empty());
}

#[test]
fn mixed_alphanumerics_and_multiple_terms_are_and() {
    let t = text_fixture();
    assert_eq!(t.labels("2024京都"), vec!["mixed"]);
    assert_eq!(t.labels("img_2024"), vec!["mixed"]);
    assert_eq!(t.labels("a1"), vec!["mixed"]);
    // 空白（全角を含む）で区切った語は AND。
    assert_eq!(t.labels("京都 初日"), vec!["kyoto_trip"]);
    assert_eq!(t.labels("京都\u{3000}旅行"), vec!["kyoto_trip"]);
    assert_eq!(t.labels("夕焼け 富士山"), vec!["sunset"]);
    assert!(t.labels("夕焼け 京都").is_empty());
    // ファイル名とキャプションをまたいだ AND。
    assert_eq!(t.labels("dsc0003 海"), vec!["sea"]);
}

#[test]
fn width_and_case_are_ignored_but_dakuten_is_not() {
    let t = text_fixture();
    // 全角・半角の英字を区別しない。
    assert_eq!(t.labels("abc"), vec!["fullwidth", "halfwidth"]);
    assert_eq!(t.labels("ＡＢＣ"), vec!["fullwidth", "halfwidth"]);
    assert_eq!(t.labels("ABC"), vec!["fullwidth", "halfwidth"]);
    // 大文字・小文字を区別しない。
    assert_eq!(t.labels("tokyo night"), vec!["upper"]);
    assert_eq!(t.labels("TOKYO"), vec!["upper"]);
    assert_eq!(t.labels("dsc0001.arw"), vec!["kyoto_trip"]);
    // 濁点の有無は区別する。
    assert_eq!(t.labels("がっこう"), vec!["ga"]);
    assert_eq!(t.labels("かっこう"), vec!["ka"]);
    assert_eq!(t.labels("が"), vec!["ga"]);
    // 半角カナは全角カナと同じ（濁点は結合される）。
    assert_eq!(t.labels("ガッコウ"), vec!["halfkana"]);
    assert_eq!(t.labels("ｶﾞｯ"), vec!["halfkana"]);
    // ひらがなとカタカナは区別する（NFKC は変換しない）。
    assert!(!t.labels("ガッコウ").contains(&"ga"));
}

#[test]
fn fts_special_characters_do_not_cause_errors() {
    let t = text_fixture();
    for input in [
        "\"",
        "\"\"",
        "*",
        ":",
        "^",
        "(",
        ")",
        "-",
        "OR",
        "AND",
        "NEAR",
        "NOT",
        "a OR b",
        "京都 OR 海",
        "NEAR(京都 海)",
        "\"京都",
        "京都\"",
        "col:京都",
        "^京都",
        "京都*",
        "-京都",
        "(京都)",
        "a AND",
        "{x}",
        "[y]",
        "'",
        "''; DROP TABLE asset; --",
        "\\",
        "%",
        "_",
        "\0",
    ] {
        let r = t.f.cat.search(
            &Filter {
                text: Some(input.to_owned()),
                ..Default::default()
            },
            &Sort::default(),
        );
        assert!(r.is_ok(), "{input:?}: {r:?}");
    }
    // 演算子の文字も、普通の文字として探す。
    assert_eq!(t.labels("NEAR"), vec!["special"]);
    assert_eq!(t.labels("OR"), vec!["special"]);
    assert_eq!(t.labels("a*b"), vec!["special"]);
    assert_eq!(t.labels("(x)"), vec!["special"]);
    assert_eq!(t.labels("-y"), vec!["special"]);
    assert_eq!(t.labels("^z:w"), vec!["special"]);
    assert_eq!(t.labels("\"quoted\""), vec!["special"]);
    // LIKE のワイルドカードは文字として扱う（「%」で全件にならない）。
    assert_eq!(t.labels("%"), vec!["special"]);
    assert_eq!(t.labels("0%"), vec!["special"]);
    assert_eq!(t.labels("f_"), vec!["special"]);
    assert_eq!(t.labels("_"), vec!["mixed", "special"]);
    assert_eq!(t.labels("\\"), vec!["special"]);
    // 空白だけの入力は条件にしない（全件）。
    assert_eq!(t.search("   ").len(), ITEMS.len());
}

#[test]
fn text_index_follows_caption_changes_moves_and_deletes() {
    let mut t = text_fixture();
    let (_, asset, variant) = t.items[0];
    t.f.cat.set_caption(asset, Some("大阪の夜景")).unwrap();
    assert_eq!(t.labels("大阪の夜景"), vec!["kyoto_trip"]);
    assert_eq!(t.labels("京都旅行"), Vec::<&str>::new());
    // キャプションの削除。
    t.f.cat.set_caption(asset, Some("   ")).unwrap();
    assert!(t.labels("大阪").is_empty());
    assert_eq!(t.f.cat.asset(asset).unwrap().caption, None);
    // リネーム（ファイル操作の完了）でファイル名の索引も変わる。
    let file = t.f.cat.files_of_asset(asset).unwrap()[0].id;
    let op =
        t.f.cat
            .plan_rename(&[(file, "嵐山の紅葉.ARW".to_owned())])
            .unwrap();
    t.f.cat.start_file_op(op).unwrap();
    t.f.cat.complete_file_op(op).unwrap();
    assert_eq!(t.labels("嵐山の紅葉"), vec!["kyoto_trip"]);
    assert!(t.labels("dsc0001").is_empty());
    // 削除で索引からも消える。
    t.f.cat.remove_assets(&[asset]).unwrap();
    assert!(t.labels("嵐山").is_empty());
    assert!(!t.search("dsc").contains(&variant));
    assert!(t.f.cat.check_integrity().unwrap().is_ok());
    // 存在しない asset のキャプションはエラー。
    assert!(t.f.cat.set_caption(asset, Some("x")).is_err());
    // 索引の作り直し。
    t.f.cat.rebuild_text_index().unwrap();
    assert_eq!(t.labels("夕焼け"), vec!["sea", "sunset"]);
}

#[test]
fn virtual_copies_match_through_their_asset() {
    let mut t = text_fixture();
    let master = t.id("sea");
    let vc =
        t.f.cat
            .create_virtual_copy(master, Some("モノクロ"))
            .unwrap();
    assert_eq!(t.search("海の夕焼け"), vec![master, vc]);
}
