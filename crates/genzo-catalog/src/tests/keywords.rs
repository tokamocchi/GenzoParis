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
