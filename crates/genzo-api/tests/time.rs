//! v1 のパッケージ「time」の結合テスト: 撮影日時の一括修正（LIB-16。機材・基準の写真に合わせる・ずらす・
//! タイムゾーン・元に戻す・並べ替えと書き出しへの反映・元ファイルを変えない）と、写真と動画の統合タイムライン
//! （ORG-01）、JSON のコマンドの形。

mod common;

use std::path::Path;

use common::{
    Env, all_items, by_name, drain, exif, hashes, import, make_video, wait_ok, write_jpeg,
};
use genzo_api::{
    ApiError, AssetKind, CaptureDeviceKey, CaptureTarget, CaptureTimeEdit, CatalogChange, Command,
    Core, ErrorKind, Event, JobResult, Response, SearchFilter, SearchSort, VariantId, ZoneMode,
};
use genzo_model::{ConflictPolicy, ExportFormat, ExportSettings, ExportSize, OutputColorSpace};

/// 機種 `model` の Exif（オフセットなし。既定のタイムゾーン +09:00 で推定）の写真を書く。
fn shot(env: &Env, name: &str, model: &str, local: &str, seed: u8) {
    let mut e = exif(Some(local), None, None);
    e.model = Some(model.to_owned());
    write_jpeg(&env.photos.join(name), (64, 48), seed, Some(&e));
}

fn local_of(core: &Core, v: VariantId) -> String {
    core.variant_details(v).unwrap().summary.capture.to_string()
}

fn ids_in_capture_order(core: &Core) -> Vec<VariantId> {
    let r = core
        .search(&SearchFilter::default(), SearchSort::default())
        .unwrap();
    core.result_ids(r.generation).unwrap()
}

fn device(model: &str) -> CaptureDeviceKey {
    CaptureDeviceKey {
        camera: Some(format!("GenzoTest {model}")),
        serial: None,
        kind: AssetKind::Photo,
    }
}

/// 書き出したファイルの DateTimeOriginal と OffsetTimeOriginal。
fn exported_time(path: &Path) -> (Option<String>, Option<String>) {
    let bytes = std::fs::read(path).unwrap();
    let e = exif::Reader::new()
        .read_from_container(&mut std::io::Cursor::new(&bytes))
        .unwrap();
    let text = |tag| {
        e.get_field(tag, exif::In::PRIMARY)
            .map(|f| f.display_value().to_string())
    };
    (
        text(exif::Tag::DateTimeOriginal),
        text(exif::Tag::OffsetTimeOriginal),
    )
}

/// 基準の写真の正しい時刻に合わせると、同じ機材の写真がすべて同じだけずれ、並び・書き出しに効き、元に戻せる。
/// 元ファイルは変わらない。
#[test]
fn syncing_a_device_to_a_reference_photo() {
    let env = Env::new();
    // 機材 A（時計が 1 時間 2 分 3 秒遅れていた）と、機材 B。
    shot(&env, "A1.jpg", "A", "2024-05-01 10:00:00", 1);
    shot(&env, "A2.jpg", "A", "2024-05-01 10:30:00", 2);
    shot(&env, "B1.jpg", "B", "2024-05-01 10:45:00", 3);
    let before = hashes(&env.photos);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a1 = by_name(&items, "A1.jpg").variant_id;
    let a2 = by_name(&items, "A2.jpg").variant_id;
    let b1 = by_name(&items, "B1.jpg").variant_id;
    assert_eq!(ids_in_capture_order(&core), vec![a1, a2, b1]);

    // 機材の一覧と、写真の機材。
    let devices = core.capture_devices().unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].key, device("A"));
    assert_eq!(devices[0].assets, 2);
    assert_eq!(
        devices[0].first.unwrap().to_rfc3339(),
        "2024-05-01T10:00:00+09:00"
    );
    assert_eq!(core.capture_device_of(a2).unwrap(), device("A"));

    let target = CaptureTarget::Devices {
        devices: vec![device("A")],
    };
    let edit = CaptureTimeEdit::Sync {
        reference: a1,
        correct: "2024-05-01T11:02:03".to_owned(),
        offset_minutes: None,
    };
    // 見込み（何も変えない）。
    let plan = core.plan_capture_time_edit(&target, &edit).unwrap();
    assert_eq!(plan.shift_seconds, Some(3_723));
    assert_eq!((plan.assets, plan.changed), (2, 2));
    assert_eq!(plan.sources.user_default, 2);
    assert_eq!(plan.examples.len(), 2);
    assert_eq!(plan.examples[0].file_name, "A1.jpg");
    assert_eq!(plan.edit_id, None);
    assert_eq!(local_of(&core, a1), "2024-05-01 10:00:00 +09:00");

    let rx = core.subscribe();
    let done = core.edit_capture_times(&target, &edit).unwrap();
    assert_eq!(done.changed, 2);
    assert!(done.edit_id.is_some());
    assert_eq!(local_of(&core, a1), "2024-05-01 11:02:03 +09:00");
    assert_eq!(local_of(&core, a2), "2024-05-01 11:32:03 +09:00");
    assert_eq!(local_of(&core, b1), "2024-05-01 10:45:00 +09:00");
    let events = drain(&rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::CatalogChanged {
                change: CatalogChange::CaptureTime,
                all: true,
                ..
            }
        )),
        "{events:?}"
    );
    // 撮影日時の順の並びに効く（B が先になる）。
    core.wait_search_refreshed(None).unwrap();
    assert_eq!(ids_in_capture_order(&core), vec![b1, a1, a2]);
    // もう一度同じ時刻に合わせても変わらない（差は 0）。
    let again = core.plan_capture_time_edit(&target, &edit).unwrap();
    assert_eq!((again.shift_seconds, again.changed), (Some(0), 0));

    // 書き出したファイルの Exif は修正後の時刻。
    let out = env.dir.path().join("out");
    let settings = ExportSettings {
        format: ExportFormat::Jpeg { quality: 85 },
        color_space: OutputColorSpace::Srgb,
        size: ExportSize::LongEdge(32),
        remove_gps: true,
        on_conflict: ConflictPolicy::Sequence,
        remove_keywords: false,
        ..ExportSettings::default()
    };
    let job = core.export(&[a1], &settings, &out).unwrap();
    let JobResult::Export(r) = wait_ok(&core, job) else {
        panic!()
    };
    let path = match &r.items[0].outcome {
        genzo_api::ExportOutcome::Written { path, .. } => path.clone(),
        other => panic!("{other:?}"),
    };
    let (dt, _) = exported_time(&path);
    assert_eq!(dt.as_deref(), Some("2024-05-01 11:02:03"));

    // 履歴と取り消し。
    let edits = core.capture_time_edits().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].kind, "sync");
    assert_eq!(edits[0].shift_seconds, Some(3_723));
    let undo = core.undo_capture_time_edit().unwrap().unwrap();
    assert_eq!(undo.restored, 2);
    assert_eq!(local_of(&core, a1), "2024-05-01 10:00:00 +09:00");
    assert!(core.undo_capture_time_edit().unwrap().is_none());
    core.wait_search_refreshed(None).unwrap();
    assert_eq!(ids_in_capture_order(&core), vec![a1, a2, b1]);
    // 元ファイルは読むだけ。
    assert_eq!(hashes(&env.photos), before);
    core.close().unwrap();
}

/// 選んだ写真をずらす・タイムゾーンを指定する（2 つの読み方）・ファイルの値に戻す。
#[test]
fn shifting_and_zones_for_selected_photos() {
    let env = Env::new();
    shot(&env, "P1.jpg", "A", "2024-05-01 12:00:00", 1);
    let mut e = exif(Some("2024-05-01 12:00:00"), Some(540), None);
    e.model = Some("A".to_owned());
    write_jpeg(&env.photos.join("P2.jpg"), (64, 48), 2, Some(&e));
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let p1 = by_name(&items, "P1.jpg").variant_id;
    let p2 = by_name(&items, "P2.jpg").variant_id;
    let both = CaptureTarget::Variants {
        variant_ids: vec![p1, p2],
    };
    // 1 日 2 時間 3 分 4 秒 遅らせる。
    let r = core
        .edit_capture_times(
            &both,
            &CaptureTimeEdit::Shift {
                seconds: 86_400 + 2 * 3_600 + 3 * 60 + 4,
            },
        )
        .unwrap();
    assert_eq!(r.changed, 2);
    assert_eq!(local_of(&core, p1), "2024-05-02 14:03:04 +09:00");
    // パリの時刻で表示（撮影した瞬間はそのまま）。
    core.edit_capture_times(
        &both,
        &CaptureTimeEdit::SetZone {
            offset_minutes: 120,
            mode: ZoneMode::KeepInstant,
        },
    )
    .unwrap();
    assert_eq!(local_of(&core, p1), "2024-05-02 07:03:04 +02:00");
    assert_eq!(local_of(&core, p2), "2024-05-02 07:03:04 +02:00");
    // 記録の時刻をパリの時刻として読み直す（撮影した瞬間が変わる）。
    core.edit_capture_times(
        &CaptureTarget::Variants {
            variant_ids: vec![p2],
        },
        &CaptureTimeEdit::SetZone {
            offset_minutes: 120,
            mode: ZoneMode::KeepLocal,
        },
    )
    .unwrap();
    assert_eq!(local_of(&core, p2), "2024-05-02 14:03:04 +02:00");
    // ファイルの値に戻す。
    core.edit_capture_times(&both, &CaptureTimeEdit::Reset)
        .unwrap();
    assert_eq!(local_of(&core, p1), "2024-05-01 12:00:00 +09:00");
    assert_eq!(local_of(&core, p2), "2024-05-01 12:00:00 +09:00");
    assert_eq!(core.capture_time_edits().unwrap().len(), 4);

    // 入力の誤り。
    let err = core
        .edit_capture_times(
            &both,
            &CaptureTimeEdit::SetZone {
                offset_minutes: 19 * 60,
                mode: ZoneMode::KeepLocal,
            },
        )
        .unwrap_err();
    assert!(matches!(err, ApiError::InvalidArgument(_)), "{err}");
    let err = core
        .plan_capture_time_edit(
            &both,
            &CaptureTimeEdit::Sync {
                reference: p1,
                correct: "きのう".to_owned(),
                offset_minutes: None,
            },
        )
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput, "{err}");
    let err = core
        .plan_capture_time_edit(
            &both,
            &CaptureTimeEdit::Shift {
                seconds: genzo_api::MAX_CAPTURE_SHIFT_SECONDS + 1,
            },
        )
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput, "{err}");
    core.close().unwrap();
}

/// タイムライン: 撮影日・イベントのまとまり、写真と動画の混在、撮影日時の順でない結果・古い世代。
#[test]
fn timeline_groups_follow_days_events_and_corrections() {
    let env = Env::new();
    shot(&env, "D1a.jpg", "A", "2024-05-01 09:00:00", 1);
    shot(&env, "D1b.jpg", "A", "2024-05-01 09:20:00", 2);
    shot(&env, "D1c.jpg", "A", "2024-05-01 15:00:00", 3);
    shot(&env, "D2a.jpg", "A", "2024-05-02 08:00:00", 4);
    // 撮影日時のない写真。
    write_jpeg(&env.photos.join("NODATE.jpg"), (64, 48), 5, None);
    // 動画（UTC 2024-05-01T00:10:00Z = 現地 09:10）。ffmpeg がなければ飛ばす。
    let has_video = make_video(&env.photos.join("V.mp4"), "2024-05-01T00:10:00Z");
    let core = env.open();
    import(&core, &env.photos);
    let r = core
        .search(&SearchFilter::default(), SearchSort::default())
        .unwrap();
    let t = core.timeline(r.generation, 2 * 3_600).unwrap();
    let spans: Vec<(u64, Option<&str>, bool)> = t
        .groups
        .iter()
        .map(|g| (g.count, g.day.as_deref(), g.new_day))
        .collect();
    let first = if has_video { 3 } else { 2 };
    assert_eq!(
        spans,
        vec![
            (first, Some("2024-05-01"), true),
            (1, Some("2024-05-01"), false),
            (1, Some("2024-05-02"), true),
            (1, None, false),
        ]
    );
    assert_eq!((t.days, t.without_time), (2, 1));
    assert!(!t.events_merged);
    assert_eq!(t.groups[0].videos, u64::from(has_video));
    assert_eq!(t.groups[0].first.as_deref(), Some("2024-05-01T09:00:00"));
    assert_eq!(t.groups[0].last.as_deref(), Some("2024-05-01T09:20:00"));
    assert_eq!(
        t.groups.iter().map(|g| g.count).sum::<u64>(),
        r.count,
        "まとまりの数の合計は件数"
    );
    // イベントで分けない。
    let t = core.timeline(r.generation, 0).unwrap();
    assert_eq!(t.groups.len(), 3);

    // 時計を 16 時間進めると、2 日目の写真が 2024-05-03 になる（日付のまとまりに効く）。
    let d2a = by_name(&all_items(&core), "D2a.jpg").variant_id;
    core.edit_capture_times(
        &CaptureTarget::Variants {
            variant_ids: vec![d2a],
        },
        &CaptureTimeEdit::Shift {
            seconds: 16 * 3_600,
        },
    )
    .unwrap();
    let cur = core.wait_search_refreshed(None).unwrap().unwrap();
    // 並びは変わらない（世代も変わらない）が、タイムラインは今のカタログの値で求める。
    let t = core.timeline(cur.generation, 2 * 3_600).unwrap();
    assert_eq!(t.groups[2].day.as_deref(), Some("2024-05-03"));

    // 撮影日時の順でない・古い世代・間隔が長すぎる。
    let by_name_sort = SearchSort {
        key: genzo_api::SortKey::FileName,
        direction: genzo_api::SortDirection::Ascending,
    };
    let r2 = core.search(&SearchFilter::default(), by_name_sort).unwrap();
    assert_eq!(
        core.timeline(r2.generation, 0).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    assert!(matches!(
        core.timeline(r.generation, 0),
        Err(ApiError::Stale { .. })
    ));
    assert!(core.timeline(r2.generation, 86_401).is_err());
    core.close().unwrap();
}

/// UTC の印付きで現地時刻を書いた動画を、現地時刻として読み直す（DJI などの想定）。ffmpeg がなければ飛ばす。
#[test]
fn utc_only_videos_can_be_read_as_local_time() {
    let env = Env::new();
    if !make_video(&env.photos.join("DJI_0001.mp4"), "2024-05-01T12:00:00Z") {
        return;
    }
    let core = env.open();
    import(&core, &env.photos);
    let v = by_name(&all_items(&core), "DJI_0001.mp4").variant_id;
    // 既定の +09:00 で表示すると 21:00（現地時刻のつもりの 12:00 が UTC として読まれている）。
    assert_eq!(local_of(&core, v), "2024-05-01 21:00:00 +09:00");
    let video_device = core.capture_device_of(v).unwrap();
    assert_eq!(video_device.kind, AssetKind::Video);
    let target = CaptureTarget::Devices {
        devices: vec![video_device],
    };
    let edit = CaptureTimeEdit::SetZone {
        offset_minutes: 540,
        mode: ZoneMode::KeepLocal,
    };
    let plan = core.plan_capture_time_edit(&target, &edit).unwrap();
    assert_eq!(plan.sources.utc_only, 1);
    core.edit_capture_times(&target, &edit).unwrap();
    assert_eq!(local_of(&core, v), "2024-05-01 12:00:00 +09:00");
    core.close().unwrap();
}

/// JSON のコマンドの形（画面の ui/src/api/types.ts）。
#[test]
fn json_commands_for_capture_times_and_timeline() {
    let env = Env::new();
    shot(&env, "A1.jpg", "A", "2024-05-01 10:00:00", 1);
    let core = env.open();
    import(&core, &env.photos);
    let v = by_name(&all_items(&core), "A1.jpg").variant_id;
    let run = |json: serde_json::Value| -> serde_json::Value {
        let out = core.execute_json(&json.to_string());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("error").is_none(), "{v}");
        v["ok"].clone()
    };
    let r = run(serde_json::json!({"type": "capture_devices"}));
    assert_eq!(r["type"], "capture_devices");
    assert_eq!(r["devices"][0]["camera"], "GenzoTest A");
    assert_eq!(r["devices"][0]["kind"], "photo");
    let r = run(serde_json::json!({
        "type": "edit_capture_times",
        "target": {"kind": "variants", "variant_ids": [v.get()]},
        "edit": {"kind": "shift", "seconds": 60}
    }));
    assert_eq!(r["type"], "capture_time_plan", "{r}");
    assert_eq!(r["changed"], 1);
    assert_eq!(r["examples"][0]["after"]["correction_s"], 60);
    let r = run(serde_json::json!({"type": "capture_time_edits"}));
    assert_eq!(r["edits"][0]["kind"], "shift");
    let r = run(serde_json::json!({"type": "undo_capture_time_edit"}));
    assert_eq!(r["type"], "capture_time_undone");
    assert_eq!(r["undone"]["restored"], 1);
    let generation = core
        .search(&SearchFilter::default(), SearchSort::default())
        .unwrap()
        .generation;
    match core
        .execute(Command::Timeline {
            generation,
            gap_seconds: 3_600,
        })
        .unwrap()
    {
        Response::Timeline(t) => assert_eq!(t.groups.len(), 1),
        other => panic!("{other:?}"),
    }
    core.close().unwrap();
}
