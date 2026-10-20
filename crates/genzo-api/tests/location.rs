//! v1 の位置情報の後付け（LIB-21。ジオタグ付け）の結合テスト: 選んだ写真にまとめて位置を付ける・付けた位置を
//! 消す（カメラの位置に戻す）・見込み・動画を除く・詳細の使う位置（`gps`・`gps_source`・`camera_gps`）・変更の
//! 通知（仮想コピーを含む）・履歴と取り消し・範囲外の座標・元のファイルを変えない・JSON のコマンドの形。
//! 書き出しの Exif に入ることは tests/export.rs で確かめる。

mod common;

use common::{Env, all_items, by_name, drain, exif, hashes, import, make_video, write_jpeg};
use genzo_api::{
    ApiError, CatalogChange, Command, Core, ErrorKind, Event, GpsCoord, GpsSource, Response,
    VariantId,
};

/// 付ける位置（東京駅付近）。
const TOKYO: GpsCoord = GpsCoord {
    lat: 35.681_236,
    lon: 139.767_125,
};

/// 付ける位置（南半球・東経。シドニー付近）。
const SYDNEY: GpsCoord = GpsCoord {
    lat: -33.856_784,
    lon: 151.215_297,
};

/// カメラ（ファイルの Exif）の位置（京都駅付近）。
const CAMERA: GpsCoord = GpsCoord {
    lat: 34.985_849,
    lon: 135.758_766,
};

/// Exif の度分秒（秒は 1/10000 単位）で書いて読み直した値なので、1e-6 度の違いを許す。
fn near(g: Option<GpsCoord>, expected: GpsCoord) -> bool {
    g.is_some_and(|g| (g.lat - expected.lat).abs() < 1e-6 && (g.lon - expected.lon).abs() < 1e-6)
}

/// 詳細の位置（使う位置・出どころ・カメラの位置）。
fn located(core: &Core, v: VariantId) -> (Option<GpsCoord>, Option<GpsSource>, Option<GpsCoord>) {
    let d = core.variant_details(v).unwrap();
    (d.gps, d.gps_source, d.camera_gps)
}

/// 位置の変更の通知（`variant_ids` と `all`）。
fn location_changes(events: &[Event]) -> Vec<(Vec<VariantId>, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::CatalogChanged {
                change: CatalogChange::Location,
                variant_ids,
                all,
            } => Some((variant_ids.clone(), *all)),
            _ => None,
        })
        .collect()
}

/// 選んだ写真に位置を付け（カメラの位置を優先して置き換える）、置き換え、消すとカメラの位置に戻り、新しい順に
/// 取り消せる。仮想コピーは位置を共有し、動画は除く。元のファイルは変わらない。
#[test]
fn assigning_replacing_clearing_and_undoing_locations() {
    let env = Env::new();
    write_jpeg(
        &env.photos.join("CAM.jpg"),
        (64, 48),
        1,
        Some(&exif(Some("2024-05-01 10:00:00"), None, Some(CAMERA))),
    );
    write_jpeg(
        &env.photos.join("NOGPS.jpg"),
        (64, 48),
        2,
        Some(&exif(Some("2024-05-01 10:01:00"), None, None)),
    );
    // 動画（ffmpeg がなければ動画の部分を飛ばす）。
    let has_video = make_video(&env.photos.join("V.mp4"), "2024-05-01T01:02:00Z");
    let before = hashes(&env.photos);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let cam = by_name(&items, "CAM.jpg").variant_id;
    let nogps = by_name(&items, "NOGPS.jpg").variant_id;
    let video = has_video.then(|| by_name(&items, "V.mp4").variant_id);
    let copy = core.create_virtual_copy(nogps, Some("白黒")).unwrap();

    // 付ける前: カメラの位置だけ。
    let (gps, source, camera) = located(&core, cam);
    assert!(near(gps, CAMERA), "{gps:?}");
    assert_eq!(source, Some(GpsSource::Camera));
    assert!(near(camera, CAMERA), "{camera:?}");
    assert_eq!(located(&core, nogps), (None, None, None));

    // 見込み（何も変えない）。仮想コピーは同じ写真なので 1 枚、動画は除く。
    let mut targets = vec![cam, nogps, copy];
    targets.extend(video);
    let plan = core.plan_location_edit(&targets, Some(TOKYO)).unwrap();
    assert_eq!(
        (plan.photos, plan.changed, plan.unchanged, plan.missing),
        (2, 2, 0, 0)
    );
    assert_eq!((plan.with_camera_gps, plan.with_assigned), (1, 0));
    assert_eq!(plan.skipped_videos, u64::from(has_video));
    assert_eq!(plan.edit_id, None);
    assert_eq!(located(&core, nogps), (None, None, None));
    assert!(core.location_edits().unwrap().is_empty());

    // 付ける: カメラの位置がある写真も、付けた位置を使う（カメラの位置は残る）。
    let rx = core.subscribe();
    let done = core.edit_locations(&targets, Some(TOKYO)).unwrap();
    assert_eq!((done.photos, done.changed), (2, 2));
    assert!(done.edit_id.is_some());
    let (gps, source, camera) = located(&core, cam);
    assert_eq!((gps, source), (Some(TOKYO), Some(GpsSource::Assigned)));
    assert!(near(camera, CAMERA), "{camera:?}");
    assert_eq!(
        located(&core, nogps),
        (Some(TOKYO), Some(GpsSource::Assigned), None)
    );
    assert_eq!(
        located(&core, copy),
        located(&core, nogps),
        "仮想コピーも同じ位置"
    );
    if let Some(v) = video {
        assert_eq!(located(&core, v), (None, None, None), "動画には付けない");
    }
    // 知らせる写真は、対象の写真のすべての variant（仮想コピーを含む。動画を除く）。
    let changes = location_changes(&drain(&rx));
    assert_eq!(changes.len(), 1, "{changes:?}");
    let (mut ids, all) = changes[0].clone();
    ids.sort();
    let mut expected = vec![cam, nogps, copy];
    expected.sort();
    assert_eq!((ids, all), (expected, false));

    // 同じ位置をもう一度付けても変わらない（履歴を残さず、知らせない）。
    let again = core.edit_locations(&[cam, nogps], Some(TOKYO)).unwrap();
    assert_eq!(
        (again.changed, again.unchanged, again.edit_id),
        (0, 2, None)
    );
    assert!(location_changes(&drain(&rx)).is_empty());
    assert_eq!(core.location_edits().unwrap().len(), 1);

    // 付けた位置を置き換える（1 枚だけ）。
    let replaced = core.edit_locations(&[cam], Some(SYDNEY)).unwrap();
    assert_eq!(
        (
            replaced.changed,
            replaced.with_assigned,
            replaced.with_camera_gps
        ),
        (1, 1, 1)
    );
    assert_eq!(located(&core, cam).0, Some(SYDNEY));
    assert_eq!(located(&core, nogps).0, Some(TOKYO));

    // 消すと、カメラの位置に戻る（カメラの位置がなければ位置なし）。
    let cleared = core.edit_locations(&[cam, copy], None).unwrap();
    assert_eq!((cleared.changed, cleared.with_assigned), (2, 2));
    let (gps, source, _) = located(&core, cam);
    assert!(near(gps, CAMERA), "{gps:?}");
    assert_eq!(source, Some(GpsSource::Camera));
    assert_eq!(located(&core, nogps), (None, None, None));
    // 付けた位置がなければ、消しても変わらない。
    let none = core.edit_locations(&[cam, nogps], None).unwrap();
    assert_eq!((none.changed, none.unchanged, none.edit_id), (0, 2, None));

    // 履歴（新しい順）。
    let edits = core.location_edits().unwrap();
    let summary: Vec<(&str, Option<GpsCoord>, u64)> = edits
        .iter()
        .map(|e| (e.kind.as_str(), e.location, e.asset_count))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("clear", None, 2),
            ("set", Some(SYDNEY), 1),
            ("set", Some(TOKYO), 2)
        ]
    );
    assert_eq!(Some(edits[2].id), done.edit_id);

    // 取り消し: 新しいものから 1 件ずつ。戻した写真が分からないので、すべてとして知らせる。
    drain(&rx);
    let u = core.undo_location_edit().unwrap().unwrap();
    assert_eq!(
        (u.edit.kind.as_str(), u.restored, u.removed),
        ("clear", 2, 0)
    );
    assert_eq!(located(&core, cam).0, Some(SYDNEY));
    assert_eq!(located(&core, nogps).0, Some(TOKYO));
    assert_eq!(location_changes(&drain(&rx)), vec![(Vec::new(), true)]);
    let u = core.undo_location_edit().unwrap().unwrap();
    assert_eq!((u.edit.location, u.restored), (Some(SYDNEY), 1));
    assert_eq!(located(&core, cam).0, Some(TOKYO));
    let u = core.undo_location_edit().unwrap().unwrap();
    assert_eq!((u.edit.location, u.restored), (Some(TOKYO), 2));
    let (gps, source, _) = located(&core, cam);
    assert!(near(gps, CAMERA), "{gps:?}");
    assert_eq!(source, Some(GpsSource::Camera));
    assert_eq!(located(&core, copy), (None, None, None));
    assert!(core.undo_location_edit().unwrap().is_none());
    assert!(core.location_edits().unwrap().is_empty());

    // 元のファイルは読むだけ。
    assert_eq!(hashes(&env.photos), before);
    core.close().unwrap();
}

/// 範囲外の座標は何も変えずに誤り。動画だけ・空・見つからない写真は、何も変えずに数だけ返す。
#[test]
fn invalid_locations_videos_and_missing_photos() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    let has_video = make_video(&env.photos.join("V.mp4"), "2024-05-01T01:02:00Z");
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg").variant_id;

    for bad in [
        GpsCoord {
            lat: 90.000_001,
            lon: 0.0,
        },
        GpsCoord {
            lat: 0.0,
            lon: -180.000_001,
        },
        GpsCoord {
            lat: 139.767_125,
            lon: 35.681_236,
        },
        GpsCoord {
            lat: f64::NAN,
            lon: 0.0,
        },
    ] {
        let err = core.edit_locations(&[a], Some(bad)).unwrap_err();
        assert!(matches!(err, ApiError::InvalidArgument(_)), "{err}");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        let err = core.plan_location_edit(&[a], Some(bad)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }
    assert!(core.location_edits().unwrap().is_empty());
    assert_eq!(core.variant_details(a).unwrap().gps, None);
    // 範囲の両端は付けられる。
    let edge = GpsCoord {
        lat: -90.0,
        lon: 180.0,
    };
    assert_eq!(core.edit_locations(&[a], Some(edge)).unwrap().changed, 1);
    assert_eq!(core.variant_details(a).unwrap().gps, Some(edge));

    // 空・見つからない写真・同じ写真の重なり。
    let empty = core.edit_locations(&[], Some(TOKYO)).unwrap();
    assert_eq!(empty, genzo_api::LocationEditPlan::default());
    let missing = VariantId::new(999_999);
    let p = core
        .plan_location_edit(&[a, a, missing, missing], Some(TOKYO))
        .unwrap();
    assert_eq!((p.photos, p.changed, p.missing), (1, 1, 1));

    // 動画だけなら何も変えずに数だけ返す（誤りにしない）。
    if has_video {
        let v = by_name(&items, "V.mp4").variant_id;
        let rx = core.subscribe();
        let r = core.edit_locations(&[v], Some(TOKYO)).unwrap();
        assert_eq!(
            (r.photos, r.changed, r.skipped_videos, r.edit_id),
            (0, 0, 1, None)
        );
        assert!(location_changes(&drain(&rx)).is_empty());
        assert_eq!(core.variant_details(v).unwrap().gps, None);
    }
    assert_eq!(core.location_edits().unwrap().len(), 1);
    core.close().unwrap();
}

/// JSON のコマンドの形（画面の ui/src/api/types.ts）。`location` の `null` は消す、省略は誤り。
#[test]
fn json_commands_for_locations() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let v = by_name(&all_items(&core), "A.jpg").variant_id;
    let send = |json: serde_json::Value| -> serde_json::Value {
        serde_json::from_str(&core.execute_json(&json.to_string())).unwrap()
    };
    let run = |json: serde_json::Value| -> serde_json::Value {
        let out = send(json);
        assert!(out.get("error").is_none(), "{out}");
        out["ok"].clone()
    };
    let location = serde_json::json!({"lat": 35.681_236, "lon": 139.767_125});
    let r = run(serde_json::json!({
        "type": "plan_location_edit", "variant_ids": [v.get()], "location": location
    }));
    assert_eq!(r["type"], "location_edit_plan", "{r}");
    assert_eq!(
        (r["photos"].as_u64(), r["changed"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(r["edit_id"], serde_json::Value::Null);
    let r = run(serde_json::json!({
        "type": "edit_locations", "variant_ids": [v.get()], "location": location
    }));
    assert_eq!(r["type"], "location_edit_plan");
    assert!(r["edit_id"].is_i64(), "{r}");
    // 詳細: 使う位置・出どころ・カメラの位置。
    let d = run(serde_json::json!({"type": "variant_details", "variant_id": v.get()}));
    assert_eq!(d["type"], "details");
    assert_eq!(d["gps"], location);
    assert_eq!(d["gps_source"], "assigned");
    assert_eq!(d["camera_gps"], serde_json::Value::Null);
    let r = run(serde_json::json!({"type": "location_edits"}));
    assert_eq!(r["type"], "location_edits");
    assert_eq!(r["edits"][0]["kind"], "set");
    assert_eq!(r["edits"][0]["location"], location);
    assert_eq!(r["edits"][0]["asset_count"], 1);

    // 範囲外は誤り（種類 invalid_input）。位置の省略も誤り（消すには null を送る）。
    for bad in [
        serde_json::json!({"type": "edit_locations", "variant_ids": [v.get()],
                           "location": {"lat": 1000.0, "lon": 0.0}}),
        serde_json::json!({"type": "edit_locations", "variant_ids": [v.get()]}),
    ] {
        let out = send(bad);
        assert_eq!(out["error"]["kind"], "invalid_input", "{out}");
    }
    assert_eq!(core.variant_details(v).unwrap().gps, Some(TOKYO));
    let r = run(serde_json::json!({
        "type": "edit_locations", "variant_ids": [v.get()], "location": null
    }));
    assert_eq!(r["changed"], 1);
    let d = run(serde_json::json!({"type": "variant_details", "variant_id": v.get()}));
    assert_eq!(
        (&d["gps"], &d["gps_source"]),
        (&serde_json::Value::Null, &serde_json::Value::Null)
    );
    let r = run(serde_json::json!({"type": "undo_location_edit"}));
    assert_eq!(r["type"], "location_undone");
    assert_eq!(r["undone"]["edit"]["kind"], "clear");
    assert_eq!(r["undone"]["restored"], 1);
    assert_eq!(r["undone"]["removed"], 0);
    match core.execute(Command::LocationEdits).unwrap() {
        Response::LocationEdits { edits } => assert_eq!(edits.len(), 1),
        other => panic!("{other:?}"),
    }
    core.close().unwrap();
}
