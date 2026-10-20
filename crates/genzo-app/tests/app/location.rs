//! (v1 の location) 位置情報の後付け（LIB-21）を、画面と同じ `core_execute` の JSON のコマンドで使う: 見込み・
//! 位置を付ける・詳細（使う位置・出どころ・カメラの位置）・変更の知らせ・範囲外の座標・履歴・消す・取り消し。
//! 座標の貼り付けの段階（第 1 段階）は、アプリにコマンドを足さずに、コアのコマンドの中継だけで動く。

use genzo_api::{CatalogChange, ErrorKind, Event, GpsCoord, GpsSource, Response};
use serde_json::json;

use crate::support::{Env, all_items, assert_kind, by_name, exec, import, ok, write_image};

/// 写真を選んで位置を付け、詳細で読み、消して取り消す。
pub fn locations_through_the_app() {
    let env = Env::new();
    let state = env.state();
    let photos = env.photos("位置 情報");
    write_image(&photos.join("A.jpg"), (48, 32), 1);
    write_image(&photos.join("B.png"), (48, 32), 2);
    state.open_catalog(&env.catalog("位置.db"), true).unwrap();
    import(&state, &photos);
    let items = all_items(&state);
    let a = by_name(&items, "A.jpg").variant_id;
    let b = by_name(&items, "B.png").variant_id;
    let tokyo = GpsCoord::new(35.681_236, 139.767_125).unwrap();
    let location = json!({"lat": 35.681_236, "lon": 139.767_125});

    let Response::LocationEditPlan(p) = ok(
        &state,
        json!({"type": "plan_location_edit", "variant_ids": [a, b], "location": location}),
    ) else {
        panic!("見込みの応答ではない");
    };
    assert_eq!(
        (p.photos, p.changed, p.with_camera_gps, p.edit_id),
        (2, 2, 0, None)
    );

    let mark = env.events.mark();
    let Response::LocationEditPlan(r) = ok(
        &state,
        json!({"type": "edit_locations", "variant_ids": [a, b], "location": location}),
    ) else {
        panic!("位置を付けた応答ではない");
    };
    assert_eq!(r.changed, 2);
    assert!(r.edit_id.is_some());
    let e = env.events.wait_for(mark, "位置の変更の知らせ", |e| {
        matches!(
            e,
            Event::CatalogChanged {
                change: CatalogChange::Location,
                ..
            }
        )
    });
    let Event::CatalogChanged {
        mut variant_ids,
        all,
        ..
    } = e
    else {
        unreachable!()
    };
    variant_ids.sort();
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!((variant_ids, all), (expected, false));

    let Response::Details(d) = ok(&state, json!({"type": "variant_details", "variant_id": a}))
    else {
        panic!("詳細の応答ではない");
    };
    assert_eq!(d.gps, Some(tokyo));
    assert_eq!(d.gps_source, Some(GpsSource::Assigned));
    assert_eq!(d.camera_gps, None);

    // 範囲外の座標は誤り（何も変えない）。
    assert_kind(
        exec(
            &state,
            json!({"type": "edit_locations", "variant_ids": [a], "location": {"lat": -91.0, "lon": 0.0}}),
        ),
        ErrorKind::InvalidInput,
    );
    let Response::LocationEdits { edits } = ok(&state, json!({"type": "location_edits"})) else {
        panic!("履歴の応答ではない");
    };
    assert_eq!(edits.len(), 1);
    assert_eq!(
        (edits[0].kind.as_str(), edits[0].location),
        ("set", Some(tokyo))
    );

    // 消す（null）と位置がなくなり、取り消すと戻る。
    let Response::LocationEditPlan(r) = ok(
        &state,
        json!({"type": "edit_locations", "variant_ids": [a], "location": null}),
    ) else {
        panic!("消した応答ではない");
    };
    assert_eq!(r.changed, 1);
    let Response::Details(d) = ok(&state, json!({"type": "variant_details", "variant_id": a}))
    else {
        panic!("詳細の応答ではない");
    };
    assert_eq!((d.gps, d.gps_source), (None, None));
    let Response::LocationUndone { undone } = ok(&state, json!({"type": "undo_location_edit"}))
    else {
        panic!("取り消しの応答ではない");
    };
    assert_eq!(undone.map(|u| u.restored), Some(1));
    let Response::Details(d) = ok(&state, json!({"type": "variant_details", "variant_id": a}))
    else {
        panic!("詳細の応答ではない");
    };
    assert_eq!(d.gps, Some(tokyo));
}
