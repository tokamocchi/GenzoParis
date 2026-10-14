//! v1 のパッケージ「organize-a」の結合テスト: 階層キーワード（LIB-09。付与・外す・一覧・名前の変更・移動・
//! 削除・親での絞り込み・書き出しの XMP）、コレクション（LIB-10。作成・入れ子・メンバー・絞り込み）、
//! スマートコレクション（LIB-11。条件・自動の更新・検索結果の世代）と、JSON のコマンドの形。

mod common;

use common::{Env, all_items, by_name, drain, import, wait_ok, wait_until, write_jpeg};
use genzo_api::{
    ApiError, CatalogChange, CollectionKind, Command, Core, ErrorKind, Event, ExportOutcome,
    ExportSettings, JobResult, Rating, Response, SearchFilter, SearchSort, SmartCondition,
    SmartRule, VariantId,
};
use genzo_model::{KeywordOp, MatchMode, MemberOp, NumberOp};

fn ids_of(core: &Core, filter: SearchFilter) -> Vec<VariantId> {
    let r = core.search(&filter, SearchSort::default()).unwrap();
    let mut ids = core.result_ids(r.generation).unwrap();
    ids.sort();
    ids
}

fn path(p: &[&str]) -> Vec<String> {
    p.iter().map(|s| (*s).to_owned()).collect()
}

/// 3 枚の写真（A・B・C）を取り込んだコア。
fn three(env: &Env) -> (Core, VariantId, VariantId, VariantId) {
    for (i, n) in ["A.jpg", "B.jpg", "C.jpg"].iter().enumerate() {
        write_jpeg(&env.photos.join(n), (64, 48), i as u8, None);
    }
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let id = |n| by_name(&items, n).variant_id;
    let (a, b, c) = (id("A.jpg"), id("B.jpg"), id("C.jpg"));
    (core, a, b, c)
}

/// キーワードの付与（パスで作る・既存のもの）・一覧・選択中の写真のまとめ・親での絞り込み・イベント。
#[test]
fn keywords_are_tagged_listed_and_searched_with_children() {
    let env = Env::new();
    let (core, a, b, c) = three(&env);
    let before = common::hashes(&env.photos);
    let rx = core.subscribe();
    let n = core
        .add_keywords(
            &[a, b],
            &[],
            &[path(&["場所", "京都", "祇園"]), path(&["家族"])],
        )
        .unwrap();
    assert_eq!(n, 4);
    let events = drain(&rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::CatalogChanged {
                change: CatalogChange::Keywords,
                variant_ids,
                ..
            } if variant_ids == &vec![a, b]
        )),
        "{events:?}"
    );
    let tree = core.keyword_tree().unwrap();
    let find = |name: &str| tree.iter().find(|k| k.name == name).unwrap().clone();
    let place = find("場所");
    let kyoto = find("京都");
    let gion = find("祇園");
    assert_eq!(kyoto.parent_id, Some(place.keyword_id));
    assert_eq!((gion.photo_count, place.photo_count), (2, 0));
    // 既存のキーワードを付ける。
    core.add_keywords(&[c], &[kyoto.keyword_id], &[]).unwrap();
    // 親のキーワードで絞り込むと子を含める。
    let by_kw = |k| {
        ids_of(
            &core,
            SearchFilter {
                keywords: vec![k],
                ..Default::default()
            },
        )
    };
    assert_eq!(by_kw(place.keyword_id), vec![a, b, c]);
    assert_eq!(by_kw(gion.keyword_id), vec![a, b]);
    // 選択中の写真のまとめ（数が選択の数より少なければ一部だけ）。
    let vk = core.variant_keywords(&[a, c, a]).unwrap();
    assert_eq!(vk.variants, 2);
    let names: Vec<(Vec<String>, u64)> = vk
        .keywords
        .iter()
        .map(|k| (k.path.clone(), k.count))
        .collect();
    assert_eq!(
        names,
        vec![
            (path(&["場所", "京都"]), 1),
            (path(&["家族"]), 1),
            (path(&["場所", "京都", "祇園"]), 1),
        ]
    );
    // 補完（部分一致。パスつき）。
    let hits = core.find_keywords("園", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, path(&["場所", "京都", "祇園"]));
    // 外す（子のキーワードは外さない）。
    assert_eq!(
        core.remove_keywords(&[a, b, c], &[kyoto.keyword_id])
            .unwrap(),
        1
    );
    assert_eq!(by_kw(place.keyword_id), vec![a, b]);
    // 元ファイルは変えない。
    assert_eq!(common::hashes(&env.photos), before);
    core.close().unwrap();
}

/// 名前の変更・移動・削除と、表示中の検索結果の作り直し（キーワードで絞り込んでいる間に削除する）。
#[test]
fn keyword_edits_refresh_the_shown_results() {
    let env = Env::new();
    let (core, a, b, _c) = three(&env);
    let kyoto = core.create_keyword(None, &path(&["場所", "京都"])).unwrap();
    let place = core
        .keyword_tree()
        .unwrap()
        .into_iter()
        .find(|k| k.name == "場所")
        .unwrap()
        .keyword_id;
    let osaka = core.create_keyword(Some(place), &path(&["大阪"])).unwrap();
    core.add_keywords(&[a], &[kyoto], &[]).unwrap();
    core.add_keywords(&[b], &[osaka], &[]).unwrap();
    // 同じ階層の同じ名前は conflict。
    let err = core.rename_keyword(osaka, "京都").unwrap_err();
    assert_eq!(err.info().kind, ErrorKind::Conflict, "{err}");
    // ファイルの移動の重なりの文言（「移動先に同じ名前のファイル…」）にしない（検証の指摘）。
    assert_eq!(
        err.to_string(),
        "同じ階層に「京都」というキーワードがあります",
        "{err}"
    );
    // 自分の下へは移せない。
    let err = core.move_keyword(place, Some(kyoto)).unwrap_err();
    assert_eq!(err.info().kind, ErrorKind::InvalidInput, "{err}");
    core.move_keyword(osaka, None).unwrap();
    core.set_keyword_export(kyoto, false).unwrap();
    let tree = core.keyword_tree().unwrap();
    assert!(
        !tree
            .iter()
            .find(|k| k.keyword_id == kyoto)
            .unwrap()
            .include_on_export
    );
    assert_eq!(
        tree.iter()
            .find(|k| k.keyword_id == osaka)
            .unwrap()
            .parent_id,
        None
    );
    // 「場所」で絞り込んで表示中に「京都」を消すと、結果が作り直される。
    let r = core
        .search(
            &SearchFilter {
                keywords: vec![place],
                ..Default::default()
            },
            SearchSort::default(),
        )
        .unwrap();
    assert_eq!(r.count, 1);
    let usage = core.keyword_usage(place).unwrap();
    assert_eq!((usage.photos_with_descendants, usage.descendants), (1, 1));
    assert_eq!(core.delete_keyword(kyoto).unwrap(), 1);
    let now = core
        .wait_search_refreshed(Some(common::WAIT_LIMIT))
        .unwrap()
        .unwrap();
    assert_eq!(now.count, 0);
    assert!(now.generation > r.generation);
    core.close().unwrap();
}

/// コレクション: セットの入れ子・手動のメンバー（仮想コピーを別に）・絞り込み・外すとすぐに消える。
#[test]
fn collections_hold_variants_and_filter_the_grid() {
    let env = Env::new();
    let (core, a, b, c) = three(&env);
    let copy = core.create_virtual_copy(a, Some("白黒")).unwrap();
    let set = core
        .create_collection(None, "旅行", CollectionKind::Set, None, &[])
        .unwrap();
    let kyoto = core
        .create_collection(Some(set), "京都", CollectionKind::Manual, None, &[a, copy])
        .unwrap();
    // 手動のコレクションの中には作れない。写真を入れられるのは手動のコレクションだけ。
    assert!(
        core.create_collection(Some(kyoto), "x", CollectionKind::Manual, None, &[])
            .is_err()
    );
    assert!(core.add_to_collection(set, &[b]).is_err());
    let err = core
        .create_collection(None, "y", CollectionKind::Set, None, &[a])
        .unwrap_err();
    assert!(matches!(err, ApiError::InvalidArgument(_)), "{err}");
    assert_eq!(core.add_to_collection(kyoto, &[b, a]).unwrap(), 1);
    let in_col = |id| {
        ids_of(
            &core,
            SearchFilter {
                collection_id: Some(id),
                ..Default::default()
            },
        )
    };
    assert_eq!(in_col(kyoto), vec![a, b, copy]);
    assert_eq!(in_col(set), vec![a, b, copy]);
    assert_eq!(core.variant_collections(copy).unwrap(), vec![kyoto]);
    // コレクションを表示中に外すと、次の作り直しで消える（評価の変更のように残さない）。
    let r = core
        .search(
            &SearchFilter {
                collection_id: Some(kyoto),
                ..Default::default()
            },
            SearchSort::default(),
        )
        .unwrap();
    assert_eq!(r.count, 3);
    core.remove_from_collection(kyoto, &[b]).unwrap();
    let now = core
        .wait_search_refreshed(Some(common::WAIT_LIMIT))
        .unwrap()
        .unwrap();
    assert_eq!(now.count, 2);
    // 一覧（手動は常に数、セット・スマートは with_counts のときだけ）。
    let list = core.collections(false).unwrap();
    let get = |l: &[genzo_api::CollectionInfo], id| {
        l.iter().find(|x| x.collection_id == id).unwrap().clone()
    };
    assert_eq!(get(&list, kyoto).count, Some(2));
    assert_eq!(get(&list, set).count, None);
    let list = core.collections(true).unwrap();
    assert_eq!(get(&list, set).count, Some(2));
    // 名前の変更・移動・削除（写真は消えない）。
    core.rename_collection(kyoto, "京都 2024").unwrap();
    core.move_collection(kyoto, None).unwrap();
    assert_eq!(core.delete_collection(set).unwrap(), 1);
    assert_eq!(core.collections(false).unwrap().len(), 1);
    assert_eq!(all_items(&core).len(), 4);
    let _ = c;
    core.close().unwrap();
}

/// スマートコレクション: 条件の保存・自動の更新（評価を変えると表示中の結果が新しい世代になる）・
/// 循環する参照の拒否・保存していない条件の件数。
#[test]
fn smart_collections_update_automatically() {
    let env = Env::new();
    let (core, a, b, c) = three(&env);
    let rule = SmartRule::new(
        MatchMode::All,
        vec![SmartCondition::Rating {
            op: NumberOp::Gte,
            value: Rating::new(3).unwrap(),
        }],
    );
    let smart = core
        .create_collection(None, "★3 以上", CollectionKind::Smart, Some(&rule), &[])
        .unwrap();
    core.set_rating(&[a], Rating::new(4).unwrap()).unwrap();
    let r = core
        .search(
            &SearchFilter {
                collection_id: Some(smart),
                ..Default::default()
            },
            SearchSort::default(),
        )
        .unwrap();
    assert_eq!(r.count, 1);
    // 評価を変えると、表示中の結果が作り直される。
    core.set_rating(&[b], Rating::new(5).unwrap()).unwrap();
    wait_until(
        "スマートコレクションの結果の作り直し",
        || {
            core.wait_search_refreshed(Some(common::WAIT_LIMIT))
                .unwrap()
                .is_some_and(|s| s.count == 2)
        },
    );
    // 条件を変えると作り直される。
    let has_kw = SmartRule::new(
        MatchMode::Any,
        vec![SmartCondition::Keyword {
            op: KeywordOp::Any,
            keyword_id: None,
        }],
    );
    core.set_smart_rule(smart, &has_kw).unwrap();
    let now = core
        .wait_search_refreshed(Some(common::WAIT_LIMIT))
        .unwrap()
        .unwrap();
    assert_eq!(now.count, 0);
    core.add_keywords(&[c], &[], &[path(&["家族"])]).unwrap();
    let now = core
        .wait_search_refreshed(Some(common::WAIT_LIMIT))
        .unwrap()
        .unwrap();
    assert_eq!(now.count, 1);
    // 自分を参照する条件は保存しない（invalid_input）。
    let cyclic = SmartRule::new(
        MatchMode::All,
        vec![SmartCondition::Collection {
            op: MemberOp::NotIn,
            collection_id: smart,
        }],
    );
    let err = core.set_smart_rule(smart, &cyclic).unwrap_err();
    assert_eq!(err.info().kind, ErrorKind::InvalidInput, "{err}");
    // 保存していない条件の件数（表示中の結果は変えない）。
    let gen_before = core.current_search().unwrap().generation;
    let n = core
        .count(&SearchFilter {
            smart_rule: Some(rule),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!(core.current_search().unwrap().generation, gen_before);
    core.close().unwrap();
}

/// 書き出したファイルの XMP に、キーワードと親が dc:subject で入る（書き出さないキーワード・設定に従う）。
#[test]
fn export_writes_keywords_into_xmp_subject() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    core.add_keywords(
        &[a],
        &[],
        &[path(&["場所", "京都"]), path(&["人物", "家族"])],
    )
    .unwrap();
    let private = core
        .keyword_tree()
        .unwrap()
        .into_iter()
        .find(|k| k.name == "家族")
        .unwrap()
        .keyword_id;
    core.set_keyword_export(private, false).unwrap();
    let export = |settings: ExportSettings, dir: &str| -> String {
        let out = env.dir.path().join(dir);
        let job = core.export(&[a], &settings, &out).unwrap();
        let JobResult::Export(r) = wait_ok(&core, job) else {
            panic!("書き出しの結果ではない");
        };
        let ExportOutcome::Written { path, .. } = &r.items[0].outcome else {
            panic!("{:?}", r.items[0].outcome);
        };
        let bytes = std::fs::read(path).unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let text = export(ExportSettings::default(), "out1");
    assert!(text.contains("<dc:subject>"), "XMP の dc:subject");
    for name in ["京都", "場所", "人物"] {
        assert!(text.contains(&format!("<rdf:li>{name}</rdf:li>")), "{name}");
    }
    assert!(
        !text.contains("<rdf:li>家族</rdf:li>"),
        "書き出さないキーワード"
    );
    let text = export(
        ExportSettings {
            remove_keywords: true,
            ..Default::default()
        },
        "out2",
    );
    assert!(!text.contains("dc:subject"), "設定で書かない");
    core.close().unwrap();
}

/// 画面（ui/src/api/types.ts）が使う JSON の形。
#[test]
fn organize_commands_have_the_ui_json_forms() {
    let env = Env::new();
    let (core, a, b, _c) = three(&env);
    let run = |v: serde_json::Value| -> serde_json::Value {
        let c: Command = serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
        serde_json::to_value(core.execute(c).unwrap()).unwrap()
    };
    let r = run(serde_json::json!({
        "type": "add_keywords", "variant_ids": [a, b], "paths": [["場所", "京都"]]
    }));
    assert_eq!(r, serde_json::json!({"type": "updated", "count": 2}));
    let r = run(serde_json::json!({"type": "keyword_tree"}));
    assert_eq!(r["type"], "keywords");
    let kyoto = r["keywords"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == "京都")
        .unwrap()
        .clone();
    assert_eq!(kyoto["photo_count"], 2);
    assert_eq!(kyoto["include_on_export"], true);
    let r = run(serde_json::json!({"type": "find_keywords", "query": "きょう"}));
    assert_eq!(r, serde_json::json!({"type": "keyword_hits", "hits": []}));
    let r = run(serde_json::json!({"type": "variant_keywords", "variant_ids": [a]}));
    assert_eq!(r["type"], "variant_keywords");
    assert_eq!(r["variants"], 1);
    assert_eq!(
        r["keywords"][0]["path"],
        serde_json::json!(["場所", "京都"])
    );
    let r = run(serde_json::json!({
        "type": "create_collection", "name": "お気に入り", "kind": "manual", "variant_ids": [a]
    }));
    assert_eq!(r["type"], "collection_created");
    let col = r["collection_id"].clone();
    let rule = serde_json::json!({
        "version": 1, "match": "all",
        "rules": [{"type": "collection", "op": "in", "collection_id": col}]
    });
    let r = run(serde_json::json!({
        "type": "create_collection", "name": "参照", "kind": "smart", "rule": rule
    }));
    let smart = r["collection_id"].clone();
    let r = run(serde_json::json!({"type": "collections", "with_counts": true}));
    let list = r["collections"].as_array().unwrap();
    let smart_row = list.iter().find(|c| c["collection_id"] == smart).unwrap();
    assert_eq!(smart_row["kind"], "smart");
    assert_eq!(smart_row["count"], 1);
    assert_eq!(smart_row["rule"]["rules"][0]["type"], "collection");
    let r = run(serde_json::json!({
        "type": "count", "filter": {"smart_rule": rule}
    }));
    assert_eq!(r, serde_json::json!({"type": "count", "count": 1}));
    let r = run(serde_json::json!({
        "type": "search", "filter": {"collection_id": smart}
    }));
    assert_eq!(r["count"], 1);
    let r = run(serde_json::json!({"type": "variant_collections", "variant_id": a}));
    assert_eq!(
        r,
        serde_json::json!({"type": "variant_collections", "collection_ids": [col]})
    );
    // 型付きのコマンドの往復。
    for c in [
        Command::KeywordTree,
        Command::Collections { with_counts: false },
        Command::CreateKeyword {
            parent_id: None,
            path: path(&["a", "b"]),
        },
        Command::SetSmartRule {
            collection_id: genzo_api::CollectionId::new(1),
            rule: SmartRule::default(),
        },
    ] {
        let back: Command = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
    let r = core.execute(Command::DeleteCollection {
        collection_id: genzo_api::CollectionId::new(999),
    });
    assert!(
        matches!(r, Err(ref e) if e.info().kind == ErrorKind::NotFound),
        "{r:?}"
    );
    let _: Response = core.execute(Command::KeywordTree).unwrap();
    core.close().unwrap();
}
