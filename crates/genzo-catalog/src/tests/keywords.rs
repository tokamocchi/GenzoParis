//! 階層キーワード（LIB-09）のテスト。

use genzo_model::{KeywordId, VariantId};

use super::Fixture;
use crate::{CatalogError, KeywordMatch};

#[test]
fn keywords_are_hierarchical_and_unique_per_parent() {
    let mut f = Fixture::new();
    let place = f.cat.ensure_keyword(None, "場所").unwrap();
    let kyoto = f.cat.ensure_keyword(Some(place), "京都").unwrap();
    // 同じ親の下の同じ名前（全角・半角、大文字・小文字の違いを含む）は同じキーワード。
    assert_eq!(f.cat.ensure_keyword(Some(place), " 京都 ").unwrap(), kyoto);
    let tokyo = f.cat.ensure_keyword(Some(place), "Tokyo").unwrap();
    assert_eq!(
        f.cat.ensure_keyword(Some(place), "ＴＯＫＹＯ").unwrap(),
        tokyo
    );
    // 別の親の下なら別のキーワード。
    let event = f.cat.ensure_keyword(None, "イベント").unwrap();
    let kyoto2 = f.cat.ensure_keyword(Some(event), "京都").unwrap();
    assert_ne!(kyoto, kyoto2);
    // パスでの確保。
    assert_eq!(f.cat.ensure_keyword_path(&["場所", "京都"]).unwrap(), kyoto);
    let gion = f
        .cat
        .ensure_keyword_path(&["場所", "京都", "祇園"])
        .unwrap();
    assert_eq!(
        f.cat.keyword_path(gion).unwrap(),
        vec!["場所", "京都", "祇園"]
    );
    let k = f.cat.keyword(gion).unwrap();
    assert_eq!(k.parent_id, Some(kyoto));
    assert_eq!(k.name, "祇園");
    let top: Vec<_> = f
        .cat
        .keyword_children(None)
        .unwrap()
        .into_iter()
        .map(|k| k.name)
        .collect();
    assert_eq!(top, vec!["イベント", "場所"]);
    assert_eq!(f.cat.keyword_children(Some(place)).unwrap().len(), 2);
    assert!(f.cat.ensure_keyword(None, "  ").is_err());
    assert!(f.cat.ensure_keyword_path::<&str>(&[]).is_err());
    assert!(
        f.cat
            .ensure_keyword(Some(KeywordId::new(999)), "x")
            .is_err()
    );
    assert!(matches!(
        f.cat.keyword(KeywordId::new(999)),
        Err(CatalogError::NotFound(_))
    ));
    assert!(f.cat.keyword_path(KeywordId::new(999)).is_err());
}

#[test]
fn find_keywords_by_exact_and_prefix_match() {
    let mut f = Fixture::new();
    let kyoto = f.cat.ensure_keyword_path(&["場所", "京都"]).unwrap();
    let kyoto_st = f.cat.ensure_keyword_path(&["場所", "京都駅"]).unwrap();
    let tokyo = f.cat.ensure_keyword_path(&["場所", "Tokyo"]).unwrap();
    let percent = f.cat.ensure_keyword(None, "100%_off").unwrap();
    let ids = |v: Vec<crate::Keyword>| v.into_iter().map(|k| k.id).collect::<Vec<_>>();
    assert_eq!(
        ids(f.cat.find_keywords("京都", KeywordMatch::Exact).unwrap()),
        vec![kyoto]
    );
    assert_eq!(
        ids(f.cat.find_keywords("京都", KeywordMatch::Prefix).unwrap()),
        vec![kyoto, kyoto_st]
    );
    assert_eq!(
        ids(f.cat.find_keywords("ｔｏｋ", KeywordMatch::Prefix).unwrap()),
        vec![tokyo]
    );
    assert_eq!(
        ids(f.cat.find_keywords("TOKYO", KeywordMatch::Exact).unwrap()),
        vec![tokyo]
    );
    // LIKE のワイルドカードは文字として扱う。
    assert!(
        f.cat
            .find_keywords("1_0", KeywordMatch::Prefix)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        ids(f.cat.find_keywords("100%", KeywordMatch::Prefix).unwrap()),
        vec![percent]
    );
    assert!(
        f.cat
            .find_keywords("%", KeywordMatch::Prefix)
            .unwrap()
            .is_empty()
    );
    assert!(
        f.cat
            .find_keywords("  ", KeywordMatch::Prefix)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn tagging_and_untagging() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None).master_variant_id;
    let b = f.photo("B.ARW", 2, None).master_variant_id;
    let kw = f.cat.ensure_keyword_path(&["人物", "家族"]).unwrap();
    assert_eq!(
        f.cat.add_keyword(&[a, b, VariantId::new(999)], kw).unwrap(),
        2
    );
    // 付いているものは何もしない。
    assert_eq!(f.cat.add_keyword(&[a], kw).unwrap(), 0);
    assert_eq!(f.cat.keywords_of(a).unwrap()[0].id, kw);
    assert_eq!(f.cat.remove_keyword(&[a], kw).unwrap(), 1);
    assert!(f.cat.keywords_of(a).unwrap().is_empty());
    assert_eq!(f.cat.keywords_of(b).unwrap().len(), 1);
    assert!(matches!(
        f.cat.add_keyword(&[a], KeywordId::new(999)),
        Err(CatalogError::NotFound(_))
    ));
    // キーワードを削除すると、子と付与も消える。
    let parent = f.cat.keyword(kw).unwrap().parent_id.unwrap();
    f.cat.delete_keyword(parent).unwrap();
    assert!(f.cat.keyword(kw).is_err());
    assert!(f.cat.keywords_of(b).unwrap().is_empty());
    assert!(f.cat.delete_keyword(parent).is_err());
}

#[test]
fn keyword_search_with_nul_matches_nothing() {
    let mut f = Fixture::new();
    f.cat.ensure_keyword_path(&["場所", "京都"]).unwrap();
    // NUL で LIKE のパターンが切れて「京」の前方一致や全件一致にならない。
    for q in ["京\0", "\0", "京都\0x"] {
        assert!(
            f.cat
                .find_keywords(q, KeywordMatch::Prefix)
                .unwrap()
                .is_empty(),
            "{q:?}"
        );
        assert!(
            f.cat
                .find_keywords(q, KeywordMatch::Exact)
                .unwrap()
                .is_empty(),
            "{q:?}"
        );
    }
    assert_eq!(
        f.cat
            .find_keywords("京", KeywordMatch::Prefix)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn keyword_tree_has_direct_counts_and_export_flags() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None).master_variant_id;
    let b = f.photo("B.ARW", 2, None).master_variant_id;
    let vc = f.cat.create_virtual_copy(a, None).unwrap();
    let kyoto = f.cat.ensure_keyword_path(&["場所", "京都"]).unwrap();
    let place = f.cat.keyword(kyoto).unwrap().parent_id.unwrap();
    let family = f.cat.ensure_keyword(None, "家族").unwrap();
    // 仮想コピーは別に付けられる。
    f.cat.add_keywords(&[a, vc], &[kyoto]).unwrap();
    f.cat.add_keywords(&[b], &[family, kyoto]).unwrap();
    f.cat.set_keyword_include_on_export(family, false).unwrap();
    let tree = f.cat.keyword_tree().unwrap();
    let node = |id: KeywordId| tree.iter().find(|k| k.id == id).unwrap().clone();
    assert_eq!(tree.len(), 3);
    // 数は直接付いている variant だけ（親の「場所」には子の分を数えない）。
    assert_eq!(node(kyoto).photo_count, 3);
    assert_eq!(node(place).photo_count, 0);
    assert_eq!(node(family).photo_count, 1);
    assert!(node(kyoto).include_on_export);
    assert!(!node(family).include_on_export);
    assert_eq!(node(kyoto).parent_id, Some(place));
    assert!(matches!(
        f.cat
            .set_keyword_include_on_export(KeywordId::new(999), true),
        Err(CatalogError::NotFound(_))
    ));
}

#[test]
fn rename_and_move_keep_names_unique_and_the_tree_acyclic() {
    let mut f = Fixture::new();
    let kyoto = f.cat.ensure_keyword_path(&["場所", "京都"]).unwrap();
    let place = f.cat.keyword(kyoto).unwrap().parent_id.unwrap();
    let gion = f.cat.ensure_keyword(Some(kyoto), "祇園").unwrap();
    let osaka = f.cat.ensure_keyword(Some(place), "大阪").unwrap();
    let top_kyoto = f.cat.ensure_keyword(None, "京都").unwrap();
    // 同じ親の下の同じ名前（全角・半角を含む）は Conflict。自分の名前の大文字・小文字の変更はできる。
    assert!(matches!(
        f.cat.rename_keyword(osaka, " 京都 "),
        Err(CatalogError::NameConflict(_))
    ));
    f.cat.rename_keyword(osaka, "Osaka").unwrap();
    f.cat.rename_keyword(osaka, "ＯＳＡＫＡ").unwrap();
    assert_eq!(f.cat.keyword(osaka).unwrap().name, "ＯＳＡＫＡ");
    assert_eq!(
        f.cat
            .find_keywords("osaka", KeywordMatch::Exact)
            .unwrap()
            .len(),
        1
    );
    assert!(f.cat.rename_keyword(osaka, "  ").is_err());
    assert!(matches!(
        f.cat.rename_keyword(KeywordId::new(999), "x"),
        Err(CatalogError::NotFound(_))
    ));
    // 自分・子孫の下へは移せない。
    assert!(matches!(
        f.cat.move_keyword(place, Some(gion)),
        Err(CatalogError::InvalidInput(_))
    ));
    assert!(matches!(
        f.cat.move_keyword(place, Some(place)),
        Err(CatalogError::InvalidInput(_))
    ));
    // 移す先に同じ名前があれば Conflict。
    assert!(matches!(
        f.cat.move_keyword(kyoto, None),
        Err(CatalogError::NameConflict(_))
    ));
    f.cat.move_keyword(gion, Some(top_kyoto)).unwrap();
    assert_eq!(f.cat.keyword_path(gion).unwrap(), vec!["京都", "祇園"]);
    f.cat.move_keyword(gion, None).unwrap();
    assert_eq!(f.cat.keyword(gion).unwrap().parent_id, None);
    assert!(matches!(
        f.cat.move_keyword(gion, Some(KeywordId::new(999))),
        Err(CatalogError::NotFound(_))
    ));
}

#[test]
fn usage_counts_descendants_and_smart_collections() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None).master_variant_id;
    let b = f.photo("B.ARW", 2, None).master_variant_id;
    let kyoto = f.cat.ensure_keyword_path(&["場所", "京都"]).unwrap();
    let place = f.cat.keyword(kyoto).unwrap().parent_id.unwrap();
    let gion = f.cat.ensure_keyword(Some(kyoto), "祇園").unwrap();
    f.cat.add_keywords(&[a], &[place]).unwrap();
    f.cat.add_keywords(&[a, b], &[gion]).unwrap();
    let rule = genzo_model::SmartRule::new(
        genzo_model::MatchMode::All,
        vec![genzo_model::SmartCondition::Keyword {
            op: genzo_model::KeywordOp::Has,
            keyword_id: Some(gion),
        }],
    );
    let smart = f
        .cat
        .create_collection(
            None,
            "祇園の写真",
            genzo_model::CollectionKind::Smart,
            Some(&rule),
        )
        .unwrap();
    let u = f.cat.keyword_usage(place).unwrap();
    assert_eq!(u.photos, 1);
    assert_eq!(u.photos_with_descendants, 2);
    assert_eq!(u.descendants, 2);
    assert_eq!(u.smart_collections, vec![(smart, "祇園の写真".to_owned())]);
    let u = f.cat.keyword_usage(gion).unwrap();
    assert_eq!(
        (u.photos, u.photos_with_descendants, u.descendants),
        (2, 2, 0)
    );
    assert!(f.cat.keyword_usage(KeywordId::new(999)).is_err());
    // 削除は子孫の数も返す。
    assert_eq!(f.cat.delete_keyword(place).unwrap(), 3);
    assert_eq!(f.rows("variant_keyword"), 0);
}

#[test]
fn many_keywords_are_added_and_removed_in_one_call() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None).master_variant_id;
    let b = f.photo("B.ARW", 2, None).master_variant_id;
    let (ids, n) = f
        .cat
        .add_keyword_paths(
            &[a, b],
            &[vec!["場所", "京都"], vec!["家族"], vec!["場所", "京都"]],
        )
        .unwrap();
    assert_eq!(ids.len(), 2, "同じパスは 1 つ");
    assert_eq!(n, 4);
    let kyoto = ids[0];
    assert_eq!(f.cat.keyword_path(kyoto).unwrap(), vec!["場所", "京都"]);
    // 存在しないキーワードがあれば何も付けない。
    let x = f.cat.ensure_keyword(None, "x").unwrap();
    assert!(matches!(
        f.cat.add_keywords(&[a, b], &[x, KeywordId::new(999)]),
        Err(CatalogError::NotFound(_))
    ));
    assert!(f.cat.keywords_of(a).unwrap().iter().all(|k| k.id != x));
    assert!(f.cat.add_keyword_paths::<&str>(&[a], &[vec![]]).is_err());
    // 選択中の写真のキーワードと数。
    f.cat.remove_keywords(&[b], &[ids[1]]).unwrap();
    let of = f.cat.keywords_of_variants(&[a, b, a]).unwrap();
    let counts: Vec<(String, u64)> = of.iter().map(|(k, n)| (k.name.clone(), *n)).collect();
    assert_eq!(counts, vec![("京都".to_owned(), 2), ("家族".to_owned(), 1)]);
    assert_eq!(f.cat.remove_keywords(&[a, b], &ids).unwrap(), 3);
    assert!(f.cat.keywords_of_variants(&[a, b]).unwrap().is_empty());
    // パスの一括の取得。
    let paths = f.cat.keyword_paths(&[kyoto, KeywordId::new(999)]).unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[&kyoto], vec!["場所", "京都"]);
}

#[test]
fn export_keywords_include_parents_and_respect_the_flag() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None).master_variant_id;
    let b = f.photo("B.ARW", 2, None).master_variant_id;
    let gion = f
        .cat
        .ensure_keyword_path(&["場所", "京都", "祇園"])
        .unwrap();
    let kyoto = f.cat.keyword(gion).unwrap().parent_id.unwrap();
    let place = f.cat.keyword(kyoto).unwrap().parent_id.unwrap();
    let event_kyoto = f.cat.ensure_keyword_path(&["イベント", "京都"]).unwrap();
    let private = f.cat.ensure_keyword_path(&["人物", "家族"]).unwrap();
    f.cat
        .add_keywords(&[a], &[gion, event_kyoto, private])
        .unwrap();
    // 親（含まれるキーワード）も書く。同じ名前は 1 つ。並びは名前の順（符号位置の順）。
    assert_eq!(
        f.cat.export_keywords(a).unwrap(),
        vec!["イベント", "京都", "人物", "場所", "家族", "祇園"]
    );
    // 書き出さないキーワード: そのものと、親としても書かない。子は書く。
    f.cat.set_keyword_include_on_export(place, false).unwrap();
    f.cat.set_keyword_include_on_export(private, false).unwrap();
    assert_eq!(
        f.cat.export_keywords(a).unwrap(),
        vec!["イベント", "京都", "人物", "祇園"]
    );
    assert!(f.cat.export_keywords(b).unwrap().is_empty());
}

#[test]
fn contains_match_lists_prefix_matches_first() {
    let mut f = Fixture::new();
    let station = f.cat.ensure_keyword_path(&["場所", "京都駅"]).unwrap();
    let east = f.cat.ensure_keyword_path(&["場所", "東京都"]).unwrap();
    let ids = |v: Vec<crate::Keyword>| v.into_iter().map(|k| k.id).collect::<Vec<_>>();
    assert_eq!(
        ids(f.cat.find_keywords("京都", KeywordMatch::Contains).unwrap()),
        vec![station, east]
    );
    assert_eq!(
        ids(f.cat.find_keywords("京都", KeywordMatch::Prefix).unwrap()),
        vec![station]
    );
    assert!(
        f.cat
            .find_keywords("%", KeywordMatch::Contains)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn keyword_names_reject_separators_control_characters_and_long_names() {
    // 検証の指摘: 区切りの文字（「>」「,」など）・長すぎる名前を受け付け、階層の表示と区別できない名前や、
    // 書き出しの XMP が大きすぎて失敗する名前ができていた。
    let mut f = Fixture::new();
    let place = f.cat.ensure_keyword(None, "場所").unwrap();
    for bad in [
        "a>b", "a＞b", "a|b", "a,b", "a，b", "a、b", "a\nb", "a\tb", "a\u{7f}b",
    ] {
        assert!(
            matches!(
                f.cat.ensure_keyword(None, bad),
                Err(CatalogError::InvalidInput(_))
            ),
            "{bad:?}"
        );
        assert!(
            matches!(
                f.cat.rename_keyword(place, bad),
                Err(CatalogError::InvalidInput(_))
            ),
            "{bad:?}"
        );
        assert!(
            f.cat.add_keyword_paths(&[], &[vec!["場所", bad]]).is_err(),
            "{bad:?}"
        );
    }
    let max = "あ".repeat(crate::MAX_KEYWORD_NAME_CHARS);
    let k = f.cat.ensure_keyword(Some(place), &max).unwrap();
    assert_eq!(f.cat.keyword(k).unwrap().name, max);
    let long = "あ".repeat(crate::MAX_KEYWORD_NAME_CHARS + 1);
    let err = f.cat.ensure_keyword(None, &long).unwrap_err();
    assert!(err.to_string().contains("文字まで"), "{err}");
    assert!(matches!(
        f.cat.rename_keyword(place, &long),
        Err(CatalogError::InvalidInput(_))
    ));
    // 失敗した操作は何も変えない。
    assert_eq!(f.cat.keyword(place).unwrap().name, "場所");
    assert_eq!(f.cat.keyword_tree().unwrap().len(), 2);
    // メッセージは使えない文字を示す。
    let err = f.cat.rename_keyword(place, "場所>京都").unwrap_err();
    assert!(err.to_string().contains("「>」"), "{err}");
}

#[test]
fn name_conflicts_are_reported_as_names_not_files() {
    let mut f = Fixture::new();
    let kyoto = f.cat.ensure_keyword(None, "京都").unwrap();
    let osaka = f.cat.ensure_keyword(None, "大阪").unwrap();
    let err = f.cat.rename_keyword(osaka, "京都").unwrap_err();
    assert!(matches!(err, CatalogError::NameConflict(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "同じ階層に「京都」というキーワードがあります"
    );
    let _ = kyoto;
}
