//! v1 のパッケージ「organize-b」の結合テスト: スタック（LIB-12。作る・解除・写真の追加と除外・一番上の写真・
//! 折りたたみと検索結果のまとめ方・撮影時刻の間隔による自動スタック・削除との関係）、仮想コピー（LIB-13。
//! 元の写真のスタックに入る・名前・マスターの入れ替え・詳細の一覧・削除の規則）と、JSON のコマンドの形。

mod common;

use common::{Env, all_items, by_name, drain, exif, import, wait_until, write_jpeg};
use genzo_api::{
    CatalogChange, Command, Core, DeleteKind, ErrorKind, Event, Rating, Response, SearchFilter,
    SearchSort, VariantId,
};

/// 表示中の検索結果の id（並びの順）。
fn shown(core: &Core, filter: &SearchFilter) -> (Vec<VariantId>, u64) {
    let r = core.search(filter, SearchSort::default()).unwrap();
    (core.result_ids(r.generation).unwrap(), r.hidden)
}

/// 撮影日時が 1 秒ずつ違う写真 4 枚（A0〜A3）と、1 時間後の 1 枚（B）を取り込んだコア。
fn burst(env: &Env) -> (Core, Vec<VariantId>, VariantId) {
    for i in 0..4u8 {
        let e = exif(Some(&format!("2024-05-01 12:00:0{i}")), Some(540), None);
        write_jpeg(&env.photos.join(format!("A{i}.jpg")), (64, 48), i, Some(&e));
    }
    let e = exif(Some("2024-05-01 13:00:00"), Some(540), None);
    write_jpeg(&env.photos.join("B.jpg"), (64, 48), 9, Some(&e));
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = (0..4)
        .map(|i| by_name(&items, &format!("A{i}.jpg")).variant_id)
        .collect();
    let b = by_name(&items, "B.jpg").variant_id;
    (core, a, b)
}

/// スタックを作ると折りたたんで 1 枚だけを出し、折りたたみ・一番上・外す・解除で表示が変わる。
#[test]
fn stacks_collapse_in_search_results_and_follow_changes() {
    let env = Env::new();
    let (core, a, b) = burst(&env);
    let before = common::hashes(&env.photos);
    let all = SearchFilter::default();
    let (ids, hidden) = shown(&core, &all);
    assert_eq!((ids.len(), hidden), (5, 0));

    let rx = core.subscribe();
    let info = core.create_stack(&a[..3], Some(a[1])).unwrap();
    assert_eq!(info.members, vec![a[1], a[0], a[2]]);
    assert_eq!(info.top_variant_id, a[1]);
    assert!(info.collapsed);
    let mut events = drain(&rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::CatalogChanged {
                change: CatalogChange::Stacks,
                variant_ids,
                all: false,
            } if variant_ids.len() == 3
        )),
        "{events:?}"
    );
    // 作り直しの後、折りたたんだスタックは一番上の写真だけ（隠した写真は 2 枚）。
    let cur = core.wait_search_refreshed(None).unwrap().unwrap();
    assert_eq!((cur.count, cur.hidden), (3, 2));
    assert_eq!(
        core.result_ids(cur.generation).unwrap(),
        vec![a[1], a[3], b]
    );
    // 作り直しのスレッドは、知らせを送ってから作り直しを済んだとするので、知らせは上で受け取った中にあることも
    // ある（両方を合わせて探す）。
    events.extend(drain(&rx));
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::SearchUpdated {
                count: 3,
                hidden: 2,
                ..
            }
        )),
        "{events:?}"
    );
    // 選択を表示している写真だけにする問い合わせ: 隠れた写真は含まない（問い合わせた順。重複は 1 つ）。
    let shown_of = core
        .shown_variants(&[a[0], a[1], a[2], a[3], b, a[1]])
        .unwrap();
    assert_eq!(shown_of.generation, cur.generation);
    assert_eq!(shown_of.variant_ids, vec![a[1], a[3], b]);
    assert_eq!(
        core.execute(Command::ShownVariants {
            variant_ids: vec![a[2], a[1]],
        })
        .unwrap(),
        Response::ShownVariants(genzo_api::ShownVariants {
            generation: cur.generation,
            variant_ids: vec![a[1]],
        })
    );
    // セルの印（位置・数・折りたたみ）。隠れた写真の位置は分からない。
    let page = core.range(cur.generation, 0, 10).unwrap();
    let badge = page.items[0].stack.unwrap();
    assert_eq!(
        (badge.stack_id, badge.position, badge.count, badge.collapsed),
        (info.stack_id, 1, 3, true)
    );
    assert!(page.items[1].stack.is_none());
    assert_eq!(core.index_of(a[0]).unwrap().index, None);
    // 評価は表示している写真（代表）だけに付く（隠れた写真には付かない）。
    core.set_rating(&[a[1]], Rating::new(4).unwrap()).unwrap();
    let three_up = SearchFilter {
        rating_min: Rating::new(3),
        ..Default::default()
    };
    assert_eq!(shown(&core, &three_up), (vec![a[1]], 0));
    // すべて展開する条件では、スタックの中の順に 1 か所に並ぶ（保存した状態は変えない）。
    let expand = SearchFilter {
        expand_stacks: true,
        ..Default::default()
    };
    assert_eq!(shown(&core, &expand), (vec![a[1], a[0], a[2], a[3], b], 0));
    // 展開を保存する（S）。
    core.search(&all, SearchSort::default()).unwrap();
    assert_eq!(
        core.set_stacks_collapsed(Some(&[info.stack_id]), false)
            .unwrap(),
        1
    );
    let cur = core.wait_search_refreshed(None).unwrap().unwrap();
    assert_eq!(
        core.result_ids(cur.generation).unwrap(),
        vec![a[1], a[0], a[2], a[3], b]
    );
    // 一番上にする・写真を足す・外す。
    let info = core.set_stack_top(a[2]).unwrap();
    assert_eq!(info.members, vec![a[2], a[1], a[0]]);
    assert_eq!(core.add_to_stack(info.stack_id, &[a[3], a[2]]).unwrap(), 1);
    assert_eq!(core.remove_from_stack(&[a[2]]).unwrap(), 1);
    let s = core.variant_stacks(&[a[0], b]).unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].members, vec![a[1], a[0], a[3]]);
    assert_eq!(s[0].top_variant_id, a[1]);
    // 解除（写真は消えない）。
    assert_eq!(core.unstack(&[a[3]]).unwrap(), 1);
    let cur = core.wait_search_refreshed(None).unwrap().unwrap();
    assert_eq!((cur.count, cur.hidden), (5, 0));
    assert!(core.variant_stacks(&a).unwrap().is_empty());
    assert_eq!(core.catalog_stats().unwrap().stacks, 0);
    // 2 枚未満は invalid_input。
    let err = core.create_stack(&[a[0]], None).unwrap_err();
    assert_eq!(err.info().kind, ErrorKind::InvalidInput);
    // 元ファイルは変えない。
    assert_eq!(common::hashes(&env.photos), before);
    core.close().unwrap();
}

/// 自動スタック（見込みと実行）と、すべてのスタックの折りたたみの変更を JSON のコマンドで。
#[test]
fn auto_stack_and_collapse_all_through_json_commands() {
    let env = Env::new();
    let (core, a, b) = burst(&env);
    let exec = |c: serde_json::Value| core.execute(serde_json::from_value::<Command>(c).unwrap());
    let r =
        exec(serde_json::json!({"type": "auto_stack", "gap_seconds": 2, "dry_run": true})).unwrap();
    let Response::AutoStack(report) = r else {
        panic!("自動スタックの応答ではない: {r:?}");
    };
    assert_eq!((report.stacks, report.photos, report.considered), (1, 4, 5));
    assert!(report.dry_run);
    assert!(core.variant_stacks(&a).unwrap().is_empty());
    let Response::AutoStack(report) =
        exec(serde_json::json!({"type": "auto_stack", "gap_seconds": 2})).unwrap()
    else {
        panic!("自動スタックの応答ではない");
    };
    assert_eq!((report.stacks, report.photos), (1, 4));
    let s = core.variant_stacks(&[a[0]]).unwrap();
    assert_eq!(s[0].members, a);
    assert_eq!(shown(&core, &SearchFilter::default()), (vec![a[0], b], 3));
    // すべてのスタックを展開する（stack_ids を省略）。
    let r = exec(serde_json::json!({"type": "set_stacks_collapsed", "collapsed": false})).unwrap();
    assert!(matches!(r, Response::Updated { count: 1 }), "{r:?}");
    let cur = core.wait_search_refreshed(None).unwrap().unwrap();
    assert_eq!((cur.count, cur.hidden), (5, 0));
    // 間隔の範囲の外は invalid_input。
    let err = exec(serde_json::json!({"type": "auto_stack", "gap_seconds": 3601})).unwrap_err();
    assert_eq!(err.info().kind, ErrorKind::InvalidInput);
    let err = core
        .auto_stack(&SearchFilter::default(), f64::NAN, true)
        .unwrap_err();
    assert_eq!(err.info().kind, ErrorKind::InvalidInput);
    // 写真が入っているスタック（JSON）。
    let r =
        exec(serde_json::json!({"type": "variant_stacks", "variant_ids": [a[2].get()]})).unwrap();
    let Response::Stacks { stacks } = r else {
        panic!("スタックの一覧ではない");
    };
    assert_eq!(stacks.len(), 1);
    // 表示中の結果に出ている写真（JSON の応答の形）。すべて展開した後なので、すべて出ている。
    let r =
        exec(serde_json::json!({"type": "shown_variants", "variant_ids": [b.get(), a[3].get()]}))
            .unwrap();
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        serde_json::json!({
            "type": "shown_variants",
            "generation": cur.generation,
            "variant_ids": [b.get(), a[3].get()],
        })
    );
    core.close().unwrap();
}

/// 仮想コピー: 元の写真のスタックに入る（展開して見える）・詳細の一覧・名前・マスターの入れ替え・削除。
#[test]
fn virtual_copies_are_stacked_renamed_and_made_master() {
    let env = Env::new();
    let (core, a, _b) = burst(&env);
    let copy = core.create_virtual_copy(a[0], None).unwrap();
    // 元の写真を一番上にした、展開したスタック。
    let s = core.variant_stacks(&[copy]).unwrap();
    assert_eq!(s[0].members, vec![a[0], copy]);
    assert!(!s[0].collapsed);
    let cur = core.wait_search_refreshed(None).unwrap().unwrap();
    let ids = core.result_ids(cur.generation).unwrap();
    assert_eq!(&ids[..2], &[a[0], copy]);
    // 詳細: 同じ写真のマスターと仮想コピー、入っているスタック。
    let d = core.variant_details(copy).unwrap();
    assert!(!d.summary.is_master);
    assert_eq!(d.summary.variant_name.as_deref(), Some("コピー 1"));
    let v: Vec<(VariantId, bool)> = d
        .variants
        .iter()
        .map(|x| (x.variant_id, x.is_master))
        .collect();
    assert_eq!(v, vec![(a[0], true), (copy, false)]);
    assert_eq!(d.stack.as_ref().unwrap().members, vec![a[0], copy]);
    // 名前の変更（マスターは invalid_input）。
    let rx = core.subscribe();
    core.rename_virtual_copy(copy, "白黒").unwrap();
    assert!(drain(&rx).iter().any(|e| matches!(
        e,
        Event::CatalogChanged {
            change: CatalogChange::Variants,
            ..
        }
    )));
    let err = core.rename_virtual_copy(a[0], "x").unwrap_err();
    assert_eq!(err.info().kind, ErrorKind::InvalidInput);
    // マスターにする: 前のマスターが仮想コピーになり、名前を受け継ぐ。
    let r = core
        .execute(Command::SetAsMaster { variant_id: copy })
        .unwrap();
    assert_eq!(
        r,
        Response::MasterChanged {
            previous_master: a[0]
        }
    );
    let d = core.variant_details(a[0]).unwrap();
    assert!(!d.summary.is_master);
    assert_eq!(d.summary.variant_name.as_deref(), Some("白黒"));
    let masters = SearchFilter {
        masters_only: true,
        expand_stacks: true,
        ..Default::default()
    };
    let (ids, _) = shown(&core, &masters);
    assert!(ids.contains(&copy) && !ids.contains(&a[0]));
    // 仮想コピーだけの削除: 前のマスター（今は仮想コピー）を消すと、1 枚になったスタックも消える。
    let plan = core
        .plan_delete(DeleteKind::VirtualCopies, &[a[0]])
        .unwrap();
    core.execute_delete(plan.plan_id).unwrap();
    assert!(core.variant_stacks(&[copy]).unwrap().is_empty());
    let d = core.variant_details(copy).unwrap();
    assert_eq!(d.variants.len(), 1);
    assert!(d.stack.is_none());
    // マスターは仮想コピーの削除では消せない。
    let err = core
        .plan_delete(DeleteKind::VirtualCopies, &[copy])
        .unwrap_err();
    assert_eq!(err.info().kind, ErrorKind::InvalidInput);
    core.close().unwrap();
}

/// カタログからの除去で一番上の写真が消えると、次の写真が一番上になり、検索結果の代表も変わる。
#[test]
fn removing_the_top_photo_promotes_the_next_one() {
    let env = Env::new();
    let (core, a, b) = burst(&env);
    let info = core.create_stack(&a[..3], None).unwrap();
    assert_eq!(info.top_variant_id, a[0]);
    shown(&core, &SearchFilter::default());
    let plan = core
        .plan_delete(DeleteKind::RemoveFromCatalog, &[a[0]])
        .unwrap();
    core.execute_delete(plan.plan_id).unwrap();
    wait_until("検索結果の作り直し", || {
        core.wait_search_refreshed(None)
            .unwrap()
            .is_some_and(|r| r.count == 3)
    });
    let cur = core.current_search().unwrap();
    assert_eq!(cur.hidden, 1);
    assert_eq!(
        core.result_ids(cur.generation).unwrap(),
        vec![a[1], a[3], b]
    );
    let s = core.variant_stacks(&[a[1]]).unwrap();
    assert_eq!(s[0].top_variant_id, a[1]);
    assert_eq!(s[0].members, vec![a[1], a[2]]);
    core.close().unwrap();
}
