//! コレクション（LIB-10）・スマートコレクション（LIB-11）と、版 7 への移行のテスト。

use chrono::{TimeZone, Utc};
use genzo_model::{
    AssetKind, CollectionId, CollectionKind, ColorLabel, ContainsOp, DateOp, EqualityOp, Flag,
    KeywordOp, MatchMode, MemberOp, NumberOp, Rating, SmartCondition, SmartRule, TextOp, VariantId,
};

use super::{Fixture, rows};
use crate::{
    CATALOG_SCHEMA_VERSION, COLLECTIONS_SCHEMA_VERSION, Catalog, CatalogError, Filter,
    FolderFilter, STABLE_IDS_SCHEMA_VERSION, Sort, SortDirection, SortKey,
};

const ORDER: Sort = Sort::new(SortKey::ImportOrder, SortDirection::Ascending);

/// コレクションに含まれる variant（id の順）。
fn in_collection(cat: &Catalog, id: CollectionId) -> Vec<VariantId> {
    let mut v = cat
        .search(
            &Filter {
                collection: Some(id),
                ..Default::default()
            },
            &ORDER,
        )
        .unwrap();
    v.sort();
    v
}

/// 条件に一致する variant（id の順）。
fn matching(cat: &Catalog, rule: SmartRule) -> Vec<VariantId> {
    let mut v = cat
        .search(
            &Filter {
                smart_rule: Some(rule),
                ..Default::default()
            },
            &ORDER,
        )
        .unwrap();
    v.sort();
    v
}

fn all(rules: Vec<SmartCondition>) -> SmartRule {
    SmartRule::new(MatchMode::All, rules)
}

fn one(c: SmartCondition) -> SmartRule {
    all(vec![c])
}

#[test]
fn collections_nest_only_in_sets_with_unique_names() {
    let mut f = Fixture::new();
    let travel = f
        .cat
        .create_collection(None, "旅行", CollectionKind::Set, None)
        .unwrap();
    let kyoto = f
        .cat
        .create_collection(Some(travel), "京都 2024", CollectionKind::Manual, None)
        .unwrap();
    // 同じ場所の同じ名前（全角・半角、大文字・小文字の違いを含む。種類によらない）は Conflict。
    assert!(matches!(
        f.cat
            .create_collection(Some(travel), "京都　２０２４", CollectionKind::Set, None),
        Err(CatalogError::NameConflict(_))
    ));
    // 別の場所なら同じ名前を使える。
    f.cat
        .create_collection(None, "京都 2024", CollectionKind::Manual, None)
        .unwrap();
    // 手動のコレクションの中には入れられない。
    assert!(matches!(
        f.cat
            .create_collection(Some(kyoto), "x", CollectionKind::Manual, None),
        Err(CatalogError::InvalidInput(_))
    ));
    assert!(matches!(
        f.cat.create_collection(
            Some(CollectionId::new(999)),
            "x",
            CollectionKind::Manual,
            None
        ),
        Err(CatalogError::NotFound(_))
    ));
    assert!(
        f.cat
            .create_collection(None, " ", CollectionKind::Manual, None)
            .is_err()
    );
    // 条件はスマートコレクションにだけ。スマートコレクションには必須。
    let rule = one(SmartCondition::Kind {
        value: AssetKind::Video,
    });
    assert!(
        f.cat
            .create_collection(None, "x", CollectionKind::Manual, Some(&rule))
            .is_err()
    );
    assert!(
        f.cat
            .create_collection(None, "x", CollectionKind::Smart, None)
            .is_err()
    );
    let smart = f
        .cat
        .create_collection(Some(travel), "動画", CollectionKind::Smart, Some(&rule))
        .unwrap();
    let list = f.cat.collections().unwrap();
    assert_eq!(list.len(), 4);
    let c = list.iter().find(|c| c.id == smart).unwrap();
    assert_eq!(c.kind, CollectionKind::Smart);
    assert_eq!(c.parent_id, Some(travel));
    assert_eq!(c.rule.as_ref(), Some(&rule));
    assert_eq!(c.rule_error, None);
    assert_eq!(f.cat.collection(smart).unwrap().name, "動画");
    assert!(f.cat.collection(CollectionId::new(999)).is_err());
}

#[test]
fn rename_move_and_delete() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None).master_variant_id;
    let travel = f
        .cat
        .create_collection(None, "旅行", CollectionKind::Set, None)
        .unwrap();
    let y2024 = f
        .cat
        .create_collection(Some(travel), "2024", CollectionKind::Set, None)
        .unwrap();
    let kyoto = f
        .cat
        .create_collection(Some(y2024), "京都", CollectionKind::Manual, None)
        .unwrap();
    let other = f
        .cat
        .create_collection(None, "京都", CollectionKind::Manual, None)
        .unwrap();
    f.cat.add_to_collection(kyoto, &[a]).unwrap();
    // 名前の変更（同じ場所に同じ名前があれば Conflict）。
    f.cat.rename_collection(kyoto, "京都の秋").unwrap();
    assert_eq!(f.cat.collection(kyoto).unwrap().name, "京都の秋");
    f.cat.rename_collection(kyoto, "京都").unwrap();
    assert!(matches!(f.cat.rename_collection(y2024, "旅行"), Ok(())));
    f.cat.rename_collection(y2024, "2024").unwrap();
    // 移動: 同じ名前のある場所へは移せない。
    assert!(matches!(
        f.cat.move_collection(kyoto, None),
        Err(CatalogError::NameConflict(_))
    ));
    // 自分・中のセットの中へは移せない。
    assert!(matches!(
        f.cat.move_collection(travel, Some(y2024)),
        Err(CatalogError::InvalidInput(_))
    ));
    assert!(matches!(
        f.cat.move_collection(travel, Some(travel)),
        Err(CatalogError::InvalidInput(_))
    ));
    // 手動のコレクションの中へは移せない。
    assert!(matches!(
        f.cat.move_collection(kyoto, Some(other)),
        Err(CatalogError::InvalidInput(_))
    ));
    f.cat.move_collection(kyoto, Some(travel)).unwrap();
    assert_eq!(f.cat.collection(kyoto).unwrap().parent_id, Some(travel));
    // 削除: セットは中のコレクションも消す。写真は消えない。
    assert_eq!(f.cat.delete_collection(travel).unwrap(), 3);
    assert_eq!(rows(&f.cat, "collection"), 1);
    assert_eq!(rows(&f.cat, "collection_member"), 0);
    assert_eq!(rows(&f.cat, "variant"), 1);
    assert!(matches!(
        f.cat.delete_collection(travel),
        Err(CatalogError::NotFound(_))
    ));
}

#[test]
fn members_are_variants_in_insertion_order() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None).master_variant_id;
    let b = f.photo("B.ARW", 2, None).master_variant_id;
    let vc = f.cat.create_virtual_copy(a, None).unwrap();
    let fav = f
        .cat
        .create_collection(None, "お気に入り", CollectionKind::Manual, None)
        .unwrap();
    assert_eq!(
        f.cat
            .add_to_collection(fav, &[b, vc, VariantId::new(999)])
            .unwrap(),
        2
    );
    // 入っているものは何もしない。
    assert_eq!(f.cat.add_to_collection(fav, &[b, a]).unwrap(), 1);
    assert_eq!(f.cat.collection_members(fav).unwrap(), vec![b, vc, a]);
    assert_eq!(f.cat.collection(fav).unwrap().member_count, 3);
    // 仮想コピーは別のメンバー。
    assert_eq!(in_collection(&f.cat, fav), vec![a, b, vc]);
    assert_eq!(f.cat.collections_of_variant(vc).unwrap(), vec![fav]);
    assert_eq!(
        f.cat
            .remove_from_collection(fav, &[a, VariantId::new(999)])
            .unwrap(),
        1
    );
    assert_eq!(in_collection(&f.cat, fav), vec![b, vc]);
    assert!(f.cat.collections_of_variant(a).unwrap().is_empty());
    // 手動のコレクションでなければ入れられない。
    let set = f
        .cat
        .create_collection(None, "セット", CollectionKind::Set, None)
        .unwrap();
    assert!(matches!(
        f.cat.add_to_collection(set, &[a]),
        Err(CatalogError::InvalidInput(_))
    ));
    assert!(matches!(
        f.cat.remove_from_collection(set, &[a]),
        Err(CatalogError::InvalidInput(_))
    ));
    // 写真を削除するとメンバーからも消える。
    f.cat.delete_virtual_copies(&[vc]).unwrap();
    assert_eq!(in_collection(&f.cat, fav), vec![b]);
}

#[test]
fn sets_contain_the_union_of_their_collections() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None).master_variant_id;
    let b = f.photo("B.ARW", 2, None).master_variant_id;
    let c = f.photo("C.ARW", 3, None).master_variant_id;
    let d = f.photo("D.ARW", 4, None).master_variant_id;
    f.cat.set_rating(&[d], Rating::new(5).unwrap()).unwrap();
    let set = f
        .cat
        .create_collection(None, "旅行", CollectionKind::Set, None)
        .unwrap();
    let inner = f
        .cat
        .create_collection(Some(set), "国内", CollectionKind::Set, None)
        .unwrap();
    let m1 = f
        .cat
        .create_collection(Some(set), "京都", CollectionKind::Manual, None)
        .unwrap();
    let m2 = f
        .cat
        .create_collection(Some(inner), "大阪", CollectionKind::Manual, None)
        .unwrap();
    f.cat
        .create_collection(
            Some(inner),
            "★5",
            CollectionKind::Smart,
            Some(&one(SmartCondition::Rating {
                op: NumberOp::Eq,
                value: Rating::new(5).unwrap(),
            })),
        )
        .unwrap();
    f.cat.add_to_collection(m1, &[a, b]).unwrap();
    f.cat.add_to_collection(m2, &[b, c]).unwrap();
    assert_eq!(in_collection(&f.cat, set), vec![a, b, c, d]);
    assert_eq!(in_collection(&f.cat, inner), vec![b, c, d]);
    let empty = f
        .cat
        .create_collection(None, "空", CollectionKind::Set, None)
        .unwrap();
    assert!(in_collection(&f.cat, empty).is_empty());
    // 削除されたコレクションはどれにも一致しない。
    assert!(in_collection(&f.cat, CollectionId::new(999)).is_empty());
    // ほかの条件と AND。
    let r = f
        .cat
        .search(
            &Filter {
                collection: Some(set),
                rating_min: Rating::new(5),
                ..Default::default()
            },
            &ORDER,
        )
        .unwrap();
    assert_eq!(r, vec![d]);
    assert_eq!(
        f.cat
            .count(&Filter {
                collection: Some(set),
                ..Default::default()
            })
            .unwrap(),
        4
    );
}

/// スマートコレクションの条件の種類ごとの一致。
struct SmartFixture {
    f: Fixture,
    /// ★4・採用・赤・キーワード「場所 > 京都 > 祇園」・2024-05-01・テキスト「夕焼け」。
    a: VariantId,
    /// ★2・不採用・ラベルなし・キーワード「家族」・2023-01-01。
    b: VariantId,
    /// 動画・★0・撮影日時なし・別のフォルダ。
    v: VariantId,
    /// a の仮想コピー（★0・キーワードなし）。
    vc: VariantId,
    place: genzo_model::KeywordId,
    family: genzo_model::KeywordId,
    sub_folder: genzo_model::FolderId,
}

fn smart_fixture() -> SmartFixture {
    let mut f = Fixture::new();
    let a = f
        .photo("A.ARW", 1, Some("2024:05:01 12:00:00"))
        .master_variant_id;
    let b = f
        .photo("B.ARW", 2, Some("2023:01:01 12:00:00"))
        .master_variant_id;
    let sub_folder = f.cat.ensure_folder(f.volume, "2024/京都/動画").unwrap();
    let v = f
        .cat
        .register_file(&super::video_req(sub_folder, "V.MP4", 3, None))
        .unwrap()
        .master_variant_id;
    let vc = f.cat.create_virtual_copy(a, None).unwrap();
    f.cat.set_rating(&[a], Rating::new(4).unwrap()).unwrap();
    f.cat.set_rating(&[b], Rating::new(2).unwrap()).unwrap();
    f.cat.set_flag(&[a], Flag::Picked).unwrap();
    f.cat.set_flag(&[b], Flag::Rejected).unwrap();
    f.cat.set_color_label(&[a], Some(ColorLabel::Red)).unwrap();
    let gion = f
        .cat
        .ensure_keyword_path(&["場所", "京都", "祇園"])
        .unwrap();
    let place = f
        .cat
        .find_keywords("場所", crate::KeywordMatch::Exact)
        .unwrap()[0]
        .id;
    let family = f.cat.ensure_keyword(None, "家族").unwrap();
    f.cat.add_keywords(&[a], &[gion]).unwrap();
    f.cat.add_keywords(&[b], &[family]).unwrap();
    let asset = f.cat.assets_of_variants(&[a]).unwrap()[0];
    f.cat.set_caption(asset, Some("京都の夕焼け")).unwrap();
    SmartFixture {
        f,
        a,
        b,
        v,
        vc,
        place,
        family,
        sub_folder,
    }
}

#[test]
fn every_condition_kind_matches_as_documented() {
    let s = smart_fixture();
    let cat = &s.f.cat;
    let (a, b, v, vc) = (s.a, s.b, s.v, s.vc);
    let r = |n: u8| Rating::new(n).unwrap();
    // 評価。
    let rating = |op, n| matching(cat, one(SmartCondition::Rating { op, value: r(n) }));
    assert_eq!(rating(NumberOp::Gte, 2), vec![a, b]);
    assert_eq!(rating(NumberOp::Lte, 2), vec![b, v, vc]);
    assert_eq!(rating(NumberOp::Eq, 4), vec![a]);
    assert_eq!(rating(NumberOp::Ne, 0), vec![a, b]);
    // フラグ。
    let flag = |op, value| matching(cat, one(SmartCondition::Flag { op, value }));
    assert_eq!(flag(EqualityOp::Is, Flag::Picked), vec![a]);
    assert_eq!(flag(EqualityOp::IsNot, Flag::Rejected), vec![a, v, vc]);
    // カラーラベル（null はラベルなし）。
    let label = |op, value| matching(cat, one(SmartCondition::ColorLabel { op, value }));
    assert_eq!(label(EqualityOp::Is, Some(ColorLabel::Red)), vec![a]);
    assert_eq!(label(EqualityOp::Is, None), vec![b, v, vc]);
    assert_eq!(
        label(EqualityOp::IsNot, Some(ColorLabel::Red)),
        vec![b, v, vc]
    );
    assert_eq!(label(EqualityOp::IsNot, None), vec![a]);
    // キーワード（子のキーワードを含む）。
    let kw = |op, keyword_id| matching(cat, one(SmartCondition::Keyword { op, keyword_id }));
    assert_eq!(kw(KeywordOp::Has, Some(s.place)), vec![a]);
    assert_eq!(kw(KeywordOp::NotHas, Some(s.place)), vec![b, v, vc]);
    assert_eq!(kw(KeywordOp::Any, None), vec![a, b]);
    assert_eq!(kw(KeywordOp::None, None), vec![v, vc]);
    // 削除されたキーワード: has はどれにも一致せず、not_has はすべてに一致する。
    let gone = Some(genzo_model::KeywordId::new(999));
    assert!(kw(KeywordOp::Has, gone).is_empty());
    assert_eq!(kw(KeywordOp::NotHas, gone).len(), 4);
    // 撮影日時。
    let t = |y| Utc.with_ymd_and_hms(y, 1, 1, 0, 0, 0).unwrap();
    let date = |op, from, until, days| {
        matching(
            cat,
            one(SmartCondition::CaptureTime {
                op,
                from,
                until,
                days,
            }),
        )
    };
    assert_eq!(
        date(DateOp::Range, Some(t(2024)), Some(t(2025)), None),
        vec![a, vc]
    );
    assert_eq!(date(DateOp::Range, None, Some(t(2024)), None), vec![b]);
    assert_eq!(date(DateOp::Unknown, None, None, None), vec![v]);
    // 2023 年・2024 年の写真は、検索した時刻（2026 年以降）の直近 30 日に入らない。
    assert!(date(DateOp::InLastDays, None, None, Some(30)).is_empty());
    assert_eq!(
        date(
            DateOp::InLastDays,
            None,
            None,
            Some(genzo_model::MAX_SMART_DAYS)
        ),
        vec![a, b, vc]
    );
    // カメラ（動画にはカメラがない）。「X ではない」は値の分からないものも含む。
    let camera = |op, value: &str| {
        matching(
            cat,
            one(SmartCondition::Camera {
                op,
                value: value.to_owned(),
            }),
        )
    };
    let model = cat.facets().unwrap().cameras[0].0.clone();
    assert_eq!(camera(TextOp::Is, &model), vec![a, b, vc]);
    assert_eq!(camera(TextOp::IsNot, &model), vec![v]);
    assert_eq!(camera(TextOp::Contains, "ilce"), vec![a, b, vc]);
    assert_eq!(camera(TextOp::Contains, "%"), Vec::<VariantId>::new());
    assert_eq!(camera(TextOp::Unknown, ""), vec![v]);
    let lens = |op, value: &str| {
        matching(
            cat,
            one(SmartCondition::Lens {
                op,
                value: value.to_owned(),
            }),
        )
    };
    assert_eq!(lens(TextOp::Contains, "24-70"), vec![a, b, vc]);
    // 種類。
    assert_eq!(
        matching(
            cat,
            one(SmartCondition::Kind {
                value: AssetKind::Video
            })
        ),
        vec![v]
    );
    // テキスト（キャプション・ファイル名）。
    let text = |op, value: &str| {
        matching(
            cat,
            one(SmartCondition::Text {
                op,
                value: value.to_owned(),
            }),
        )
    };
    assert_eq!(text(ContainsOp::Contains, "夕焼け"), vec![a, vc]);
    assert_eq!(text(ContainsOp::Contains, "夕"), vec![a, vc]);
    assert_eq!(text(ContainsOp::NotContains, "夕焼け"), vec![b, v]);
    // フォルダ（配下を含めるか）。
    let folder = |op, folder_id, include_subfolders| {
        matching(
            cat,
            one(SmartCondition::Folder {
                op,
                folder_id,
                include_subfolders,
            }),
        )
    };
    assert_eq!(folder(MemberOp::In, s.f.folder, false), vec![a, b, vc]);
    assert_eq!(folder(MemberOp::In, s.f.folder, true), vec![a, b, v, vc]);
    assert_eq!(folder(MemberOp::NotIn, s.sub_folder, false), vec![a, b, vc]);
}

#[test]
fn all_any_and_nested_groups() {
    let s = smart_fixture();
    let cat = &s.f.cat;
    let r = |n: u8| Rating::new(n).unwrap();
    let picked = SmartCondition::Flag {
        op: EqualityOp::Is,
        value: Flag::Picked,
    };
    let family = SmartCondition::Keyword {
        op: KeywordOp::Has,
        keyword_id: Some(s.family),
    };
    let video = SmartCondition::Kind {
        value: AssetKind::Video,
    };
    assert_eq!(
        matching(
            cat,
            SmartRule::new(MatchMode::Any, vec![picked.clone(), family.clone()])
        ),
        vec![s.a, s.b]
    );
    assert!(matching(cat, all(vec![picked.clone(), family.clone()])).is_empty());
    // （採用 または 家族）かつ ★2 以下 → b。
    let nested = all(vec![
        SmartCondition::Group {
            match_mode: MatchMode::Any,
            rules: vec![picked, family],
        },
        SmartCondition::Rating {
            op: NumberOp::Lte,
            value: r(2),
        },
    ]);
    assert_eq!(matching(cat, nested), vec![s.b]);
    // 空の条件: すべて → 全件、いずれか → なし。空のグループも同じ。
    assert_eq!(matching(cat, all(vec![])).len(), 4);
    assert!(matching(cat, SmartRule::new(MatchMode::Any, vec![])).is_empty());
    assert_eq!(
        matching(
            cat,
            all(vec![
                video.clone(),
                SmartCondition::Group {
                    match_mode: MatchMode::All,
                    rules: vec![]
                }
            ])
        ),
        vec![s.v]
    );
    assert_eq!(
        matching(
            cat,
            SmartRule::new(
                MatchMode::Any,
                vec![
                    video,
                    SmartCondition::Group {
                        match_mode: MatchMode::Any,
                        rules: vec![]
                    }
                ]
            )
        ),
        vec![s.v]
    );
}

#[test]
fn smart_collections_update_automatically_and_reference_collections() {
    let mut s = smart_fixture();
    let (a, b, v, vc) = (s.a, s.b, s.v, s.vc);
    let good =
        s.f.cat
            .create_collection(
                None,
                "★3 以上",
                CollectionKind::Smart,
                Some(&one(SmartCondition::Rating {
                    op: NumberOp::Gte,
                    value: Rating::new(3).unwrap(),
                })),
            )
            .unwrap();
    assert_eq!(in_collection(&s.f.cat, good), vec![a]);
    // 評価を変えると、次の検索で結果が変わる（自動で更新される）。
    s.f.cat.set_rating(&[b], Rating::new(5).unwrap()).unwrap();
    assert_eq!(in_collection(&s.f.cat, good), vec![a, b]);
    // コレクションの参照（手動・スマート・含まれない）。
    let picks =
        s.f.cat
            .create_collection(None, "選んだ写真", CollectionKind::Manual, None)
            .unwrap();
    s.f.cat.add_to_collection(picks, &[vc, v]).unwrap();
    let rule = SmartRule::new(
        MatchMode::Any,
        vec![
            SmartCondition::Collection {
                op: MemberOp::In,
                collection_id: good,
            },
            SmartCondition::Collection {
                op: MemberOp::In,
                collection_id: picks,
            },
        ],
    );
    let union =
        s.f.cat
            .create_collection(None, "まとめ", CollectionKind::Smart, Some(&rule))
            .unwrap();
    assert_eq!(in_collection(&s.f.cat, union), vec![a, b, v, vc]);
    let not_picked = one(SmartCondition::Collection {
        op: MemberOp::NotIn,
        collection_id: picks,
    });
    assert_eq!(matching(&s.f.cat, not_picked), vec![a, b]);
    // 削除されたコレクション: in はどれにも一致せず、not_in はすべてに一致する。
    let gone = |op| {
        one(SmartCondition::Collection {
            op,
            collection_id: CollectionId::new(999),
        })
    };
    assert!(matching(&s.f.cat, gone(MemberOp::In)).is_empty());
    assert_eq!(matching(&s.f.cat, gone(MemberOp::NotIn)).len(), 4);
    // 条件の変更。
    s.f.cat
        .set_smart_rule(
            good,
            &one(SmartCondition::Kind {
                value: AssetKind::Video,
            }),
        )
        .unwrap();
    assert_eq!(in_collection(&s.f.cat, good), vec![v]);
    assert!(matches!(
        s.f.cat.set_smart_rule(picks, &all(vec![])),
        Err(CatalogError::InvalidInput(_))
    ));
}

#[test]
fn cyclic_references_are_rejected_and_ignored_at_search_time() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None).master_variant_id;
    let set = f
        .cat
        .create_collection(None, "セット", CollectionKind::Set, None)
        .unwrap();
    let refers_set = one(SmartCondition::Collection {
        op: MemberOp::In,
        collection_id: set,
    });
    let x = f
        .cat
        .create_collection(None, "X", CollectionKind::Smart, Some(&all(vec![])))
        .unwrap();
    // 自分を参照する条件。
    let self_ref = one(SmartCondition::Collection {
        op: MemberOp::NotIn,
        collection_id: x,
    });
    assert!(matches!(
        f.cat.set_smart_rule(x, &self_ref),
        Err(CatalogError::InvalidInput(_))
    ));
    // X がセットを参照し、そのセットの中に X を入れる（移動で循環する）。
    f.cat.set_smart_rule(x, &refers_set).unwrap();
    assert!(matches!(
        f.cat.move_collection(x, Some(set)),
        Err(CatalogError::InvalidInput(_))
    ));
    assert_eq!(f.cat.collection(x).unwrap().parent_id, None);
    // セットの中で作るのも同じ。
    assert!(matches!(
        f.cat
            .create_collection(Some(set), "Y", CollectionKind::Smart, Some(&refers_set)),
        Err(CatalogError::InvalidInput(_))
    ));
    // Y → X → セット → Y の循環（2 つのスマートコレクション）。
    let y = f
        .cat
        .create_collection(Some(set), "Y", CollectionKind::Smart, Some(&all(vec![])))
        .unwrap();
    assert!(matches!(
        f.cat.set_smart_rule(
            y,
            &one(SmartCondition::Collection {
                op: MemberOp::In,
                collection_id: x
            })
        ),
        Err(CatalogError::InvalidInput(_))
    ));
    // 外部のツールで循環を作られても、検索は失敗せず、循環した参照をどれにも一致しないとみなす。
    f.cat
        .conn
        .execute(
            "UPDATE collection SET smart_rule_json = ?2 WHERE id = ?1",
            rusqlite::params![x.get(), self_ref.to_json()],
        )
        .unwrap();
    // X = 「X に含まれない」: 内側の X は 0 とみなすので、NOT 0 = すべて。
    assert_eq!(in_collection(&f.cat, x), vec![a]);
    // 読めない条件（新しい版）はどれにも一致せず、一覧で理由を出す。
    f.cat
        .conn
        .execute(
            "UPDATE collection SET smart_rule_json = '{\"version\":99,\"rules\":[]}' WHERE id = ?1",
            [y.get()],
        )
        .unwrap();
    assert!(in_collection(&f.cat, y).is_empty());
    let c = f.cat.collection(y).unwrap();
    assert!(c.rule.is_none());
    assert!(c.rule_error.unwrap().contains("99"));
}

#[test]
fn collection_and_keyword_filters_do_not_scan_without_need() {
    // 手動のコレクション・キーワードの条件は索引（collection_member の主キー・variant_keyword）で引く
    // （実行計画に「SCAN cm」「SCAN vk」がない）。
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None).master_variant_id;
    let m = f
        .cat
        .create_collection(None, "m", CollectionKind::Manual, None)
        .unwrap();
    f.cat.add_to_collection(m, &[a]).unwrap();
    let kw = f.cat.ensure_keyword(None, "k").unwrap();
    let plan = f
        .cat
        .explain_search(
            &Filter {
                collection: Some(m),
                keywords: vec![kw],
                ..Default::default()
            },
            &Sort::new(SortKey::Rating, SortDirection::Descending),
        )
        .unwrap();
    let text = plan.join("\n");
    assert!(!text.contains("SCAN cm"), "{text}");
    assert!(!text.contains("SCAN vk"), "{text}");
}

#[test]
fn folder_filter_and_smart_folder_condition_agree() {
    let s = smart_fixture();
    let mut by_filter =
        s.f.cat
            .search(
                &Filter {
                    folder: Some(FolderFilter {
                        folder_id: s.f.folder,
                        include_subfolders: true,
                    }),
                    ..Default::default()
                },
                &ORDER,
            )
            .unwrap();
    by_filter.sort();
    let by_rule = matching(
        &s.f.cat,
        one(SmartCondition::Folder {
            op: MemberOp::In,
            folder_id: s.f.folder,
            include_subfolders: true,
        }),
    );
    assert_eq!(by_filter, by_rule);
}

#[test]
fn version_6_catalog_migrates_names_and_keyword_flags() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let (variant, kw) = {
        let mut cat =
            crate::testing::open_at_version(&path, COLLECTIONS_SCHEMA_VERSION - 1).unwrap();
        let vol = cat.ensure_volume("v", None, Some("/media")).unwrap();
        let folder = cat.ensure_folder(vol, "2024").unwrap();
        let o = cat
            .register_file(&super::photo_req(folder, "A.ARW", 1, None))
            .unwrap();
        let kw = cat.ensure_keyword_path(&["場所", "京都"]).unwrap();
        cat.add_keyword(&[o.master_variant_id], kw).unwrap();
        // 版 6 には name_key がなく、名前の重複も確かめていなかった（外部のツールで作った行を想定）。
        cat.conn
            .execute_batch(
                "INSERT INTO collection(id, parent_id, name, kind) VALUES (1, NULL, '旅行', 'set');
                 INSERT INTO collection(id, parent_id, name, kind) VALUES (2, 1, '京都', 'manual');
                 INSERT INTO collection(id, parent_id, name, kind) VALUES (3, 1, 'ｷｮｳﾄ', 'manual');
                 INSERT INTO collection(id, parent_id, name, kind) VALUES (4, 1, '京都', 'manual');
                 INSERT INTO collection(id, parent_id, name, kind) VALUES (5, NULL, '京都', 'manual');",
            )
            .unwrap();
        cat.conn
            .execute(
                "INSERT INTO collection_member(collection_id, variant_id) VALUES (2, ?1)",
                [o.master_variant_id.get()],
            )
            .unwrap();
        assert!(cat.conn.prepare("SELECT name_key FROM collection").is_err());
        cat.close().unwrap();
        (o.master_variant_id, kw)
    };
    let mut cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().migrated_from,
        Some(COLLECTIONS_SCHEMA_VERSION - 1)
    );
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
    // 重複した名前は後のものに「 (2)」を足す（別の親の同じ名前はそのまま）。
    let names: Vec<(i64, String)> = cat
        .collections()
        .unwrap()
        .into_iter()
        .map(|c| (c.id.get(), c.name))
        .collect();
    assert!(names.contains(&(2, "京都".to_owned())), "{names:?}");
    assert!(names.contains(&(3, "ｷｮｳﾄ".to_owned())), "{names:?}");
    assert!(names.contains(&(4, "京都 (2)".to_owned())), "{names:?}");
    assert!(names.contains(&(5, "京都".to_owned())), "{names:?}");
    // 移行の後は名前の重複を防ぐ。
    assert!(matches!(
        cat.create_collection(
            Some(CollectionId::new(1)),
            "京都 (2)",
            CollectionKind::Manual,
            None
        ),
        Err(CatalogError::NameConflict(_))
    ));
    assert_eq!(in_collection(&cat, CollectionId::new(1)), vec![variant]);
    // キーワードは書き出す（既定）。公開ビュー photo_collections_v1 が読める。
    assert!(
        cat.keyword_tree()
            .unwrap()
            .iter()
            .all(|k| k.include_on_export)
    );
    assert_eq!(cat.export_keywords(variant).unwrap(), vec!["京都", "場所"]);
    let (collection, path_text): (String, String) = cat
        .conn
        .query_row(
            "SELECT collection, collection_path FROM photo_collections_v1 WHERE variant_id = ?1",
            [variant.get()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (collection.as_str(), path_text.as_str()),
        ("京都", "旅行 > 京都")
    );
    let _ = kw;
    cat.close().unwrap();
}

#[test]
fn deleted_keyword_and_collection_ids_are_not_reused() {
    // 検証の指摘: 最大の id のキーワード・コレクションを削除して作り直すと、同じ id になり、削除した
    // キーワード・コレクションを参照するスマートコレクションの条件が、新しいものに付け替わっていた。
    let mut s = smart_fixture();
    let (a, b, v, vc) = (s.a, s.b, s.v, s.vc);
    let temp = s.f.cat.ensure_keyword(None, "Temp").unwrap();
    s.f.cat.add_keywords(&[a, b], &[temp]).unwrap();
    let not_temp =
        s.f.cat
            .create_collection(
                None,
                "NotTemp",
                CollectionKind::Smart,
                Some(&one(SmartCondition::Keyword {
                    op: KeywordOp::NotHas,
                    keyword_id: Some(temp),
                })),
            )
            .unwrap();
    assert_eq!(in_collection(&s.f.cat, not_temp), vec![v, vc]);
    // 削除した後は not_has がすべてに一致する（smart.rs の doc）。
    s.f.cat.delete_keyword(temp).unwrap();
    assert_eq!(in_collection(&s.f.cat, not_temp), vec![a, b, v, vc]);
    // 新しく作ったキーワードは削除した id を使わず、条件は付け替わらない。
    let other = s.f.cat.ensure_keyword(None, "Other").unwrap();
    assert!(other.get() > temp.get(), "{other} は {temp} より大きい");
    s.f.cat.add_keywords(&[a, v], &[other]).unwrap();
    assert_eq!(in_collection(&s.f.cat, not_temp), vec![a, b, v, vc]);

    // コレクションも同じ（手動のコレクションを参照する条件。参照するコレクションを最大の id にする）。
    let in_picks =
        s.f.cat
            .create_collection(
                None,
                "選んだ写真の中",
                CollectionKind::Smart,
                Some(&all(vec![])),
            )
            .unwrap();
    let picks =
        s.f.cat
            .create_collection(None, "選んだ写真", CollectionKind::Manual, None)
            .unwrap();
    s.f.cat
        .set_smart_rule(
            in_picks,
            &one(SmartCondition::Collection {
                op: MemberOp::In,
                collection_id: picks,
            }),
        )
        .unwrap();
    s.f.cat.add_to_collection(picks, &[b]).unwrap();
    assert_eq!(in_collection(&s.f.cat, in_picks), vec![b]);
    s.f.cat.delete_collection(picks).unwrap();
    assert!(in_collection(&s.f.cat, in_picks).is_empty());
    let again =
        s.f.cat
            .create_collection(None, "新しい", CollectionKind::Manual, None)
            .unwrap();
    assert!(again.get() > picks.get(), "{again} は {picks} より大きい");
    s.f.cat.add_to_collection(again, &[a]).unwrap();
    assert!(in_collection(&s.f.cat, in_picks).is_empty());
}

#[test]
fn version_7_catalog_migrates_to_ids_that_are_not_reused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let (variant, kyoto, deleted_keyword, deleted_collection, smart, folder) = {
        let mut cat =
            crate::testing::open_at_version(&path, STABLE_IDS_SCHEMA_VERSION - 1).unwrap();
        let vol = cat.ensure_volume("v", None, Some("/media")).unwrap();
        let folder = cat.ensure_folder(vol, "2024/京都").unwrap();
        let o = cat
            .register_file(&super::photo_req(folder, "A.ARW", 1, None))
            .unwrap();
        let kyoto = cat.ensure_keyword_path(&["場所", "京都"]).unwrap();
        cat.add_keyword(&[o.master_variant_id], kyoto).unwrap();
        // 版 7 で、最大の id のキーワード・コレクションを参照する条件を作ってから、それらを削除する。
        let temp = cat.ensure_keyword(None, "Temp").unwrap();
        let manual = cat
            .create_collection(None, "選んだ写真", CollectionKind::Manual, None)
            .unwrap();
        cat.add_to_collection(manual, &[o.master_variant_id])
            .unwrap();
        let smart = cat
            .create_collection(None, "条件", CollectionKind::Smart, Some(&all(vec![])))
            .unwrap();
        let removed = cat
            .create_collection(None, "消す", CollectionKind::Manual, None)
            .unwrap();
        cat.set_smart_rule(
            smart,
            &all(vec![
                SmartCondition::Keyword {
                    op: KeywordOp::NotHas,
                    keyword_id: Some(temp),
                },
                SmartCondition::Collection {
                    op: MemberOp::NotIn,
                    collection_id: removed,
                },
            ]),
        )
        .unwrap();
        cat.delete_keyword(temp).unwrap();
        cat.delete_collection(removed).unwrap();
        // 版 7 では AUTOINCREMENT ではない（連番の行がない）。
        let seq: i64 = cat
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_sequence WHERE name IN ('keyword', 'collection', 'folder')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(seq, 0);
        cat.close().unwrap();
        (o.master_variant_id, kyoto, temp, removed, smart, folder)
    };
    let mut cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().migrated_from,
        Some(STABLE_IDS_SCHEMA_VERSION - 1)
    );
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
    // 3 つの表が AUTOINCREMENT になり、行・付与・メンバー・条件・書き出しの印はそのまま。
    for table in ["folder", "keyword", "collection"] {
        let sql: String = cat
            .conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert!(sql.contains("AUTOINCREMENT"), "{table}: {sql}");
    }
    assert_eq!(
        cat.keyword_path(kyoto).unwrap(),
        vec!["場所".to_owned(), "京都".to_owned()]
    );
    assert_eq!(cat.export_keywords(variant).unwrap(), vec!["京都", "場所"]);
    assert_eq!(in_collection(&cat, smart), vec![variant]);
    let folders = cat
        .search(
            &Filter {
                folder: Some(FolderFilter {
                    folder_id: folder,
                    include_subfolders: false,
                }),
                ..Default::default()
            },
            &ORDER,
        )
        .unwrap();
    assert_eq!(folders, vec![variant]);
    // 削除した（条件が参照する）id は、移行の後に作るものに使わない。
    let new_keyword = cat.ensure_keyword(None, "Other").unwrap();
    assert!(new_keyword.get() > deleted_keyword.get());
    let new_collection = cat
        .create_collection(None, "新しい", CollectionKind::Manual, None)
        .unwrap();
    assert!(new_collection.get() > deleted_collection.get());
    cat.add_keyword(&[variant], new_keyword).unwrap();
    cat.add_to_collection(new_collection, &[variant]).unwrap();
    assert_eq!(in_collection(&cat, smart), vec![variant]);
    // 外部キー・索引は版 7 と同じ（削除の連鎖が効く）。
    cat.delete_keyword(kyoto).unwrap();
    assert_eq!(cat.export_keywords(variant).unwrap(), vec!["Other"]);
    let violations: i64 = cat
        .conn
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
    cat.close().unwrap();
}

#[test]
fn collection_names_have_a_length_limit_and_no_control_characters() {
    let mut f = Fixture::new();
    let max = "あ".repeat(crate::MAX_COLLECTION_NAME_CHARS);
    let c = f
        .cat
        .create_collection(None, &max, CollectionKind::Manual, None)
        .unwrap();
    assert_eq!(f.cat.collection(c).unwrap().name, max);
    let long = "あ".repeat(crate::MAX_COLLECTION_NAME_CHARS + 1);
    for bad in [long.as_str(), "a\nb", "a\tb"] {
        assert!(
            matches!(
                f.cat
                    .create_collection(None, bad, CollectionKind::Manual, None),
                Err(CatalogError::InvalidInput(_))
            ),
            "{bad:?}"
        );
        assert!(
            matches!(
                f.cat.rename_collection(c, bad),
                Err(CatalogError::InvalidInput(_))
            ),
            "{bad:?}"
        );
    }
    // 「>」などの記号はコレクションの名前には使える（区切りとして読む入力欄がない）。
    f.cat.rename_collection(c, "旅行 > 京都, 2024").unwrap();
    let err = f
        .cat
        .create_collection(None, "旅行 > 京都, 2024", CollectionKind::Set, None)
        .unwrap_err();
    assert!(matches!(err, CatalogError::NameConflict(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "同じ場所に「旅行 > 京都, 2024」というコレクションがあります"
    );
}
