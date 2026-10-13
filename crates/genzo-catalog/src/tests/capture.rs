//! 撮影日時の一括修正（LIB-16）・機材・修正の履歴と取り消し・タイムラインの撮影日時（ORG-01）・版 11 への移行。

use chrono::{FixedOffset, TimeZone, Utc};
use genzo_model::{
    AssetKind, CaptureTime, PhotoMetadata, TzSource, VariantId, VideoMetadata, ZoneMode,
};

use super::{Fixture, capture, facts, photo_meta, rows, video_req};
use crate::{
    CAPTURE_TIME_EDITS_SCHEMA_VERSION, CATALOG_SCHEMA_VERSION, CaptureAdjustment, CaptureEditKind,
    Catalog, DeviceKey, MAX_CAPTURE_SHIFT_SECONDS, MAX_CAPTURE_TIME_EDITS, RegisterFile,
    RegisterOutcome, Sort, SortDirection, SortKey,
};

fn jst() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).unwrap()
}

fn paris() -> FixedOffset {
    FixedOffset::east_opt(2 * 3600).unwrap()
}

/// 機種・シリアル番号・撮影日時（オフセットなし。既定の +09:00 で推定）の写真。
fn camera_photo(
    f: &mut Fixture,
    name: &str,
    n: u64,
    model: &str,
    serial: Option<&str>,
    when: Option<&str>,
) -> RegisterOutcome {
    let meta = PhotoMetadata {
        body_serial: serial.map(str::to_owned),
        ..photo_meta(model, "FE 35mm F1.8")
    };
    let capture = match when {
        Some(w) => CaptureTime::resolve(Some(w), None, jst()).unwrap(),
        None => CaptureTime::unknown(),
    };
    f.cat
        .register_file(&RegisterFile::photo(
            f.folder,
            name,
            facts(n),
            meta,
            capture,
        ))
        .unwrap()
}

fn key(model: Option<&str>, serial: Option<&str>, kind: AssetKind) -> DeviceKey {
    DeviceKey {
        camera: model.map(|m| format!("SONY {m}")),
        serial: serial.map(str::to_owned),
        kind,
    }
}

fn utc_of(f: &Fixture, o: &RegisterOutcome) -> String {
    f.cat
        .capture_time(o.asset_id)
        .unwrap()
        .utc_db_string()
        .unwrap()
}

#[test]
fn devices_group_by_camera_serial_and_kind() {
    let mut f = Fixture::new();
    camera_photo(
        &mut f,
        "A1.ARW",
        1,
        "ILCE-7M4",
        Some("111"),
        Some("2024:05:01 10:00:00"),
    );
    camera_photo(&mut f, "A2.ARW", 2, "ILCE-7M4", Some("111"), None);
    camera_photo(
        &mut f,
        "B1.ARW",
        3,
        "ILCE-7M4",
        Some("222"),
        Some("2024:05:01 11:00:00"),
    );
    camera_photo(
        &mut f,
        "C1.ARW",
        4,
        "ILCE-7C",
        None,
        Some("2024:05:02 09:00:00"),
    );
    // 機材のタグのない動画と、タグのある動画。
    f.cat
        .register_file(&video_req(
            f.folder,
            "DJI_0001.MP4",
            5,
            Some("2024-05-01T03:00:00Z"),
        ))
        .unwrap();
    let mut tagged = video_req(f.folder, "IMG_0001.MOV", 6, Some("2024-05-01T03:00:00Z"));
    if let crate::MediaMetadata::Video(v) = &mut tagged.metadata {
        v.make = Some("Apple".to_owned());
        v.model = Some("iPhone 15".to_owned());
    }
    f.cat.register_file(&tagged).unwrap();

    let devices = f.cat.capture_devices().unwrap();
    type Row<'a> = (Option<&'a str>, Option<&'a str>, AssetKind, u64, u64);
    let summary: Vec<Row<'_>> = devices
        .iter()
        .map(|d| {
            (
                d.key.camera.as_deref(),
                d.key.serial.as_deref(),
                d.key.kind,
                d.assets,
                d.with_time,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (Some("Apple iPhone 15"), None, AssetKind::Video, 1, 1),
            (Some("SONY ILCE-7C"), None, AssetKind::Photo, 1, 1),
            (Some("SONY ILCE-7M4"), Some("111"), AssetKind::Photo, 2, 1),
            (Some("SONY ILCE-7M4"), Some("222"), AssetKind::Photo, 1, 1),
            (None, None, AssetKind::Video, 1, 1),
        ]
    );
    let a = &devices[2];
    assert_eq!(
        a.first,
        Some(Utc.with_ymd_and_hms(2024, 5, 1, 1, 0, 0).unwrap())
    );
    assert_eq!(a.first, a.last);
    // 機材の asset（NULL のカメラ・シリアル番号も一致する）。
    let ids = f
        .cat
        .assets_of_devices(&[
            key(Some("ILCE-7M4"), Some("111"), AssetKind::Photo),
            DeviceKey {
                camera: None,
                serial: None,
                kind: AssetKind::Video,
            },
        ])
        .unwrap();
    assert_eq!(ids.len(), 3);
    // 写真の機材。
    let v = f.cat.all_variant_ids().unwrap()[3];
    assert_eq!(
        f.cat.device_of_variant(v).unwrap(),
        key(Some("ILCE-7C"), None, AssetKind::Photo)
    );
    assert!(f.cat.device_of_variant(VariantId::new(999)).is_err());
}

#[test]
fn shifting_a_device_is_recorded_and_can_be_undone() {
    let mut f = Fixture::new();
    let a = camera_photo(
        &mut f,
        "A.ARW",
        1,
        "ILCE-7M4",
        Some("111"),
        Some("2024:05:01 10:00:00"),
    );
    let b = camera_photo(&mut f, "B.ARW", 2, "ILCE-7M4", Some("111"), None);
    let c = camera_photo(
        &mut f,
        "C.ARW",
        3,
        "ILCE-7C",
        None,
        Some("2024:05:01 10:00:30"),
    );
    let assets = f
        .cat
        .assets_of_devices(&[key(Some("ILCE-7M4"), Some("111"), AssetKind::Photo)])
        .unwrap();
    assert_eq!(assets, vec![a.asset_id, b.asset_id]);
    let shift = CaptureAdjustment::Shift { seconds: 3_661 };

    // 見込み（書かない）。
    let plan = f
        .cat
        .plan_capture_adjustment(&assets, &shift, jst(), 5)
        .unwrap();
    assert_eq!(
        (plan.assets, plan.changed, plan.unchanged, plan.without_time),
        (2, 1, 0, 1)
    );
    assert_eq!(plan.sources.user_default, 1);
    assert_eq!(plan.examples.len(), 1);
    assert_eq!(plan.examples[0].after.correction_s, 3_661);
    assert_eq!(utc_of(&f, &a), "2024-05-01T01:00:00.000Z");
    assert_eq!(rows(&f.cat, "capture_time_edit"), 0);

    // 修正する。元の値（文字列・オフセット）は変えない。
    let out = f
        .cat
        .adjust_capture_times(&assets, &shift, CaptureEditKind::Shift, jst(), 5)
        .unwrap();
    assert!(out.edit_id.is_some());
    assert_eq!(out.plan.changed, 1);
    let t = f.cat.capture_time(a.asset_id).unwrap();
    assert_eq!(t.raw.as_deref(), Some("2024:05:01 10:00:00"));
    assert_eq!(t.offset, None);
    assert_eq!(t.correction_s, 3_661);
    assert_eq!(utc_of(&f, &a), "2024-05-01T02:01:01.000Z");
    // 撮影日時の分からない写真・別の機材は変わらない。
    assert_eq!(f.cat.capture_time(b.asset_id).unwrap().correction_s, 0);
    assert_eq!(utc_of(&f, &c), "2024-05-01T01:00:30.000Z");
    // 撮影日時の順の並びに効く（a が c の後になる）。
    let sorted = f
        .cat
        .search(
            &crate::Filter::default(),
            &Sort::new(SortKey::CaptureTime, SortDirection::Ascending),
        )
        .unwrap();
    assert_eq!(
        sorted,
        vec![
            c.master_variant_id,
            a.master_variant_id,
            b.master_variant_id
        ]
    );

    // 続けてもう 1 回（足し込む）。
    f.cat
        .adjust_capture_times(
            &[a.asset_id],
            &CaptureAdjustment::Shift { seconds: -61 },
            CaptureEditKind::Sync,
            jst(),
            0,
        )
        .unwrap();
    assert_eq!(f.cat.capture_time(a.asset_id).unwrap().correction_s, 3_600);
    let edits = f.cat.capture_time_edits().unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0].kind, CaptureEditKind::Sync);
    assert_eq!(edits[0].shift_s, Some(-61));
    assert_eq!(edits[1].kind, CaptureEditKind::Shift);
    assert_eq!(edits[1].asset_count, 1);

    // 新しいものから順に取り消す。
    let undo = f.cat.undo_capture_time_edit(jst()).unwrap().unwrap();
    assert_eq!(undo.edit.kind, CaptureEditKind::Sync);
    assert_eq!(undo.restored, 1);
    assert_eq!(f.cat.capture_time(a.asset_id).unwrap().correction_s, 3_661);
    f.cat.undo_capture_time_edit(jst()).unwrap().unwrap();
    assert_eq!(utc_of(&f, &a), "2024-05-01T01:00:00.000Z");
    assert!(!f.cat.capture_time(a.asset_id).unwrap().has_user_changes());
    assert!(f.cat.undo_capture_time_edit(jst()).unwrap().is_none());
    assert_eq!(rows(&f.cat, "capture_time_edit_item"), 0);
}

#[test]
fn nothing_changes_when_some_photo_would_go_out_of_range() {
    let mut f = Fixture::new();
    let a = camera_photo(
        &mut f,
        "A.ARW",
        1,
        "ILCE-7M4",
        None,
        Some("2024:05:01 10:00:00"),
    );
    let b = camera_photo(
        &mut f,
        "B.ARW",
        2,
        "ILCE-7M4",
        None,
        Some("9999:12:31 00:00:00"),
    );
    let err = f
        .cat
        .adjust_capture_times(
            &[a.asset_id, b.asset_id],
            &CaptureAdjustment::Shift {
                seconds: 2 * 86_400,
            },
            CaptureEditKind::Shift,
            jst(),
            0,
        )
        .unwrap_err();
    assert!(matches!(err, crate::CatalogError::InvalidInput(_)), "{err}");
    assert_eq!(f.cat.capture_time(a.asset_id).unwrap().correction_s, 0);
    assert_eq!(rows(&f.cat, "capture_time_edit"), 0);
    // ずらす量の上限。
    assert!(
        f.cat
            .plan_capture_adjustment(
                &[a.asset_id],
                &CaptureAdjustment::Shift {
                    seconds: MAX_CAPTURE_SHIFT_SECONDS + 1
                },
                jst(),
                0,
            )
            .is_err()
    );
    // 変わるものがなければ履歴を残さない。
    let out = f
        .cat
        .adjust_capture_times(
            &[a.asset_id],
            &CaptureAdjustment::ClearZone,
            CaptureEditKind::ClearZone,
            jst(),
            0,
        )
        .unwrap();
    assert_eq!(out.edit_id, None);
    assert_eq!(out.plan.unchanged, 1);
    assert_eq!(rows(&f.cat, "capture_time_edit"), 0);
    // 種類と内容が合わなければ断る。
    assert!(
        f.cat
            .adjust_capture_times(
                &[a.asset_id],
                &CaptureAdjustment::Reset,
                CaptureEditKind::Shift,
                jst(),
                0
            )
            .is_err()
    );
}

#[test]
fn zones_can_be_set_cleared_reset_and_undone_with_the_current_default() {
    let mut f = Fixture::new();
    let a = camera_photo(
        &mut f,
        "A.ARW",
        1,
        "ILCE-7M4",
        None,
        Some("2024:05:01 12:00:00"),
    );
    let ids = [a.asset_id];
    // 撮影した瞬間はそのままで、パリの時刻で表示する。
    f.cat
        .adjust_capture_times(
            &ids,
            &CaptureAdjustment::SetZone {
                offset: paris(),
                mode: ZoneMode::KeepInstant,
            },
            CaptureEditKind::SetZone,
            jst(),
            0,
        )
        .unwrap();
    let t = f.cat.capture_time(a.asset_id).unwrap();
    assert_eq!(t.tz_source, TzSource::UserSet);
    assert_eq!(t.basis.as_deref(), Some("+09:00"));
    assert_eq!(utc_of(&f, &a), "2024-05-01T03:00:00.000Z");
    assert_eq!(t.to_string(), "2024-05-01 05:00:00 +02:00");
    // 履歴の値。
    let e = &f.cat.capture_time_edits().unwrap()[0];
    assert_eq!(e.zone.as_deref(), Some("+02:00"));
    assert_eq!(e.zone_mode, Some(ZoneMode::KeepInstant));
    // 時計のずれも足す。
    f.cat
        .adjust_capture_times(
            &ids,
            &CaptureAdjustment::Shift { seconds: 60 },
            CaptureEditKind::Shift,
            jst(),
            0,
        )
        .unwrap();
    // ファイルの値に戻す。
    f.cat
        .adjust_capture_times(
            &ids,
            &CaptureAdjustment::Reset,
            CaptureEditKind::Reset,
            jst(),
            0,
        )
        .unwrap();
    let t = f.cat.capture_time(a.asset_id).unwrap();
    assert!(!t.has_user_changes());
    assert_eq!(t.basis, None);
    // 取り消すと、指定とずれが戻る。
    f.cat.undo_capture_time_edit(jst()).unwrap().unwrap();
    let t = f.cat.capture_time(a.asset_id).unwrap();
    assert_eq!(t.tz_source, TzSource::UserSet);
    assert_eq!(t.correction_s, 60);
    assert_eq!(utc_of(&f, &a), "2024-05-01T03:01:00.000Z");
    // 既定のタイムゾーンが変わった後に取り消すと、ずれの取り消しは今の推定に当てる（指定は残る）。
    f.cat
        .undo_capture_time_edit(FixedOffset::east_opt(0).unwrap())
        .unwrap();
    assert_eq!(utc_of(&f, &a), "2024-05-01T03:00:00.000Z");
    // 指定の取り消し（ClearZone）は、既定のタイムゾーンで推定し直す。
    f.cat
        .adjust_capture_times(
            &ids,
            &CaptureAdjustment::ClearZone,
            CaptureEditKind::ClearZone,
            FixedOffset::east_opt(0).unwrap(),
            0,
        )
        .unwrap();
    let t = f.cat.capture_time(a.asset_id).unwrap();
    assert_eq!(t.tz_source, TzSource::UserDefault);
    assert_eq!(utc_of(&f, &a), "2024-05-01T12:00:00.000Z");
}

#[test]
fn utc_only_videos_can_be_read_as_local_time() {
    let mut f = Fixture::new();
    // DJI などが現地時刻（12:00 +09:00）を UTC の印付きで書いた動画（既定のタイムゾーン +09:00 で表示）。
    let req = RegisterFile::video(
        f.folder,
        "DJI_0001.MP4",
        facts(1),
        VideoMetadata::default(),
        CaptureTime::resolve(Some("2024-05-01T12:00:00.000000Z"), None, jst()).unwrap(),
    );
    let v = f.cat.register_file(&req).unwrap();
    let adj = CaptureAdjustment::SetZone {
        offset: jst(),
        mode: ZoneMode::KeepLocal,
    };
    let plan = f
        .cat
        .plan_capture_adjustment(&[v.asset_id], &adj, jst(), 1)
        .unwrap();
    assert_eq!(plan.sources.utc_only, 1);
    f.cat
        .adjust_capture_times(&[v.asset_id], &adj, CaptureEditKind::SetZone, jst(), 0)
        .unwrap();
    assert_eq!(utc_of(&f, &v), "2024-05-01T03:00:00.000Z");
    // 繰り返しても変わらない（二重にずれない）。
    let again = f
        .cat
        .adjust_capture_times(&[v.asset_id], &adj, CaptureEditKind::SetZone, jst(), 0)
        .unwrap();
    assert_eq!(again.edit_id, None);
    assert_eq!(utc_of(&f, &v), "2024-05-01T03:00:00.000Z");
}

#[test]
fn rereading_a_changed_file_keeps_the_zone_and_its_basis() {
    let mut f = Fixture::new();
    let a = camera_photo(
        &mut f,
        "A.ARW",
        1,
        "ILCE-7M4",
        None,
        Some("2024:05:01 12:00:00"),
    );
    f.cat
        .adjust_capture_times(
            &[a.asset_id],
            &CaptureAdjustment::SetZone {
                offset: paris(),
                mode: ZoneMode::KeepInstant,
            },
            CaptureEditKind::SetZone,
            jst(),
            0,
        )
        .unwrap();
    // 内容が変わった（別のアプリで書き換えた）ファイルを読み直す。
    let meta = PhotoMetadata {
        body_serial: Some("999".to_owned()),
        ..photo_meta("ILCE-7M4", "FE 35mm F1.8")
    };
    let req = RegisterFile::photo(
        f.folder,
        "A.ARW",
        facts(77),
        meta,
        CaptureTime::resolve(Some("2024:05:01 12:00:01"), None, jst()).unwrap(),
    );
    f.cat.register_file(&req).unwrap();
    let t = f.cat.capture_time(a.asset_id).unwrap();
    assert_eq!(t.tz_source, TzSource::UserSet);
    assert_eq!(t.basis.as_deref(), Some("+09:00"));
    assert_eq!(utc_of(&f, &a), "2024-05-01T03:00:01.000Z");
    // 読み直したファイルのシリアル番号が入る。
    assert_eq!(
        f.cat.asset(a.asset_id).unwrap().camera_serial.as_deref(),
        Some("999")
    );
}

#[test]
fn undo_skips_removed_assets_and_history_is_limited() {
    let mut f = Fixture::new();
    let a = camera_photo(
        &mut f,
        "A.ARW",
        1,
        "ILCE-7M4",
        None,
        Some("2024:05:01 12:00:00"),
    );
    let b = camera_photo(
        &mut f,
        "B.ARW",
        2,
        "ILCE-7M4",
        None,
        Some("2024:05:01 12:00:10"),
    );
    f.cat
        .adjust_capture_times(
            &[a.asset_id, b.asset_id, a.asset_id],
            &CaptureAdjustment::Shift { seconds: 5 },
            CaptureEditKind::Shift,
            jst(),
            0,
        )
        .unwrap();
    assert_eq!(rows(&f.cat, "capture_time_edit_item"), 2);
    f.cat.remove_assets(&[b.asset_id]).unwrap();
    assert_eq!(rows(&f.cat, "capture_time_edit_item"), 1);
    let undo = f.cat.undo_capture_time_edit(jst()).unwrap().unwrap();
    assert_eq!((undo.restored, undo.removed), (1, 0));
    assert_eq!(undo.edit.asset_count, 2);

    for i in 0..(MAX_CAPTURE_TIME_EDITS + 3) {
        f.cat
            .adjust_capture_times(
                &[a.asset_id],
                &CaptureAdjustment::Shift { seconds: 1 },
                CaptureEditKind::Shift,
                jst(),
                0,
            )
            .unwrap();
        assert!(
            f.cat.capture_time_edits().unwrap().len() <= MAX_CAPTURE_TIME_EDITS,
            "{i}"
        );
    }
    assert_eq!(
        rows(&f.cat, "capture_time_edit") as usize,
        MAX_CAPTURE_TIME_EDITS
    );
    assert_eq!(
        rows(&f.cat, "capture_time_edit_item") as usize,
        MAX_CAPTURE_TIME_EDITS
    );
}

#[test]
fn timeline_keys_follow_the_given_order() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, Some("2024:05:01 12:00:00"));
    let b = f.photo("B.ARW", 2, None);
    let v = f
        .cat
        .register_file(&video_req(
            f.folder,
            "V.MP4",
            3,
            Some("2024:05:02 00:30:00"),
        ))
        .unwrap();
    let ids = [
        v.master_variant_id,
        VariantId::new(9_999),
        a.master_variant_id,
        b.master_variant_id,
    ];
    let keys = f.cat.timeline_keys(&ids).unwrap();
    assert_eq!(keys.len(), 4);
    assert!(keys[0].video);
    assert_eq!(
        keys[0].utc_ms,
        Some(
            Utc.with_ymd_and_hms(2024, 5, 1, 15, 30, 0)
                .unwrap()
                .timestamp_millis()
        )
    );
    assert_eq!(keys[0].offset_s, 9 * 3600);
    assert_eq!(keys[1].utc_ms, None);
    assert!(!keys[2].video);
    assert_eq!(
        keys[2].utc_ms,
        Some(
            Utc.with_ymd_and_hms(2024, 5, 1, 3, 0, 0)
                .unwrap()
                .timestamp_millis()
        )
    );
    assert_eq!(keys[3].utc_ms, None);
    assert!(f.cat.timeline_keys(&[]).unwrap().is_empty());
    // id ごとに引く方法と、表を順に読む方法は同じ結果（id は検索の結果の順・重なり・0 を含む）。
    let mut many = ids.to_vec();
    many.extend([a.master_variant_id, VariantId::new(0)]);
    assert_eq!(
        f.cat.timeline_keys_by_id(&many).unwrap(),
        f.cat.timeline_keys_by_scan(&many, 3, 3).unwrap()
    );
    // 結果が少なければ id ごとに引く（どちらでも同じ値）。
    for i in 0..20 {
        f.photo(&format!("X{i}.ARW"), 100 + i, Some("2024:05:03 12:00:00"));
    }
    let one = f.cat.timeline_keys(&[a.master_variant_id]).unwrap();
    assert_eq!(one, vec![keys[2]]);
}

/// 版 10 のカタログを開くと版 11 に移行し、2 つの列（NULL）・修正の履歴の表・photos_v1 の列ができる。撮影日時は
/// 変わらない。
#[test]
fn version_10_catalog_migrates_to_capture_time_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let (photo, before) = {
        let mut cat =
            crate::testing::open_at_version(&path, CAPTURE_TIME_EDITS_SCHEMA_VERSION - 1).unwrap();
        let vol = cat.ensure_volume("v", None, None).unwrap();
        let folder = cat.ensure_folder(vol, "2024").unwrap();
        // 版 10 で記録した修正（タイムゾーンの指定と時計のずれ）。
        let req = RegisterFile::photo(
            folder,
            "A.ARW",
            facts(1),
            PhotoMetadata {
                body_serial: Some("123".to_owned()),
                ..photo_meta("ILCE-7M4", "FE 35mm F1.8")
            },
            capture(Some("2024:05:01 12:00:00"))
                .with_user_offset(paris())
                .unwrap()
                .with_correction(30)
                .unwrap(),
        );
        let photo = cat.register_file(&req).unwrap();
        let before = cat.capture_time(photo.asset_id).unwrap();
        assert!(cat.conn.prepare("SELECT camera_serial FROM asset").is_err());
        cat.close().unwrap();
        (photo, before)
    };
    let cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().migrated_from,
        Some(CAPTURE_TIME_EDITS_SCHEMA_VERSION - 1)
    );
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
    let after = cat.capture_time(photo.asset_id).unwrap();
    assert_eq!(after, before);
    assert_eq!(after.basis, None);
    // シリアル番号は読み直すまで NULL。
    assert_eq!(cat.asset(photo.asset_id).unwrap().camera_serial, None);
    let (serial, local): (Option<String>, String) = cat
        .conn
        .query_row(
            "SELECT camera_serial, captured_at_local FROM photos_v1 WHERE asset_id = ?1",
            [photo.asset_id.get()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(serial, None);
    assert_eq!(local, "2024-05-01T12:00:30.000");
    assert_eq!(rows(&cat, "capture_time_edit"), 0);
    // 移行の後は、修正を記録できる。
    cat.close().unwrap();
    let mut cat = Catalog::open(&path).unwrap();
    cat.adjust_capture_times(
        &[photo.asset_id],
        &CaptureAdjustment::Shift { seconds: -30 },
        CaptureEditKind::Shift,
        jst(),
        0,
    )
    .unwrap();
    assert_eq!(rows(&cat, "capture_time_edit_item"), 1);
    cat.close().unwrap();
}

#[test]
fn serials_and_video_devices_are_stored_on_registration() {
    let mut f = Fixture::new();
    let a = camera_photo(&mut f, "A.ARW", 1, "ILCE-7M4", Some(" 4012345 "), None);
    let rec = f.cat.asset(a.asset_id).unwrap();
    assert_eq!(rec.camera.as_deref(), Some("SONY ILCE-7M4"));
    assert_eq!(rec.camera_serial.as_deref(), Some("4012345"));
    let mut req = video_req(f.folder, "IMG.MOV", 2, None);
    if let crate::MediaMetadata::Video(v) = &mut req.metadata {
        v.make = Some("Apple".to_owned());
        v.model = Some("iPhone 15 Pro".to_owned());
    }
    let v = f.cat.register_file(&req).unwrap();
    let rec = f.cat.asset(v.asset_id).unwrap();
    assert_eq!(rec.camera.as_deref(), Some("Apple iPhone 15 Pro"));
    assert_eq!(rec.camera_serial, None);
    // カメラの絞り込みの候補にも出る。
    let facets = f.cat.facets().unwrap();
    assert!(
        facets
            .cameras
            .iter()
            .any(|(c, n)| c == "Apple iPhone 15 Pro" && *n == 1)
    );
}

/// 50 万件での時間の計測用（通常のテストでは実行しない。ORG-01・LIB-16）。
///
/// 実行例: `GENZO_POC6_ASSETS=500000 cargo test -p genzo-catalog --release -- --ignored capture_scale --nocapture`
/// （件数を省略すると 10 万件）。結果は標準エラー出力に表示する。
#[test]
#[ignore = "大量のデータで計測するため、明示的に実行する"]
fn capture_scale_measurement() {
    use std::time::Instant;
    let assets: usize = std::env::var("GENZO_POC6_ASSETS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);
    let mut f = Fixture::new();
    let t = Instant::now();
    crate::populate_dummy(
        &mut f.cat,
        &crate::DummySpec {
            assets,
            ..Default::default()
        },
    )
    .unwrap();
    eprintln!("登録 {assets} 件: {:?}", t.elapsed());
    let ids = f
        .cat
        .search(&crate::Filter::default(), &Sort::default())
        .unwrap();
    let t = Instant::now();
    let keys = f.cat.timeline_keys(&ids).unwrap();
    eprintln!("timeline_keys {} 件: {:?}", keys.len(), t.elapsed());
    assert_eq!(keys.len(), ids.len());
    let t = Instant::now();
    let by_id = f.cat.timeline_keys_by_id(&ids).unwrap();
    eprintln!("timeline_keys_by_id {} 件: {:?}", by_id.len(), t.elapsed());
    assert_eq!(by_id, keys);
    let t = Instant::now();
    let small = f.cat.timeline_keys(&ids[..1_000]).unwrap();
    eprintln!("timeline_keys 1,000 件: {:?}", t.elapsed());
    assert_eq!(small, keys[..1_000]);
    let t = Instant::now();
    let devices = f.cat.capture_devices().unwrap();
    eprintln!(
        "capture_devices（{} 機材）: {:?}",
        devices.len(),
        t.elapsed()
    );
    let largest = devices.iter().max_by_key(|d| d.assets).unwrap().key.clone();
    let t = Instant::now();
    let targets = f.cat.assets_of_devices(&[largest]).unwrap();
    eprintln!(
        "assets_of_devices（{} 件）: {:?}",
        targets.len(),
        t.elapsed()
    );
    let shift = CaptureAdjustment::Shift { seconds: 3_600 };
    let t = Instant::now();
    let plan = f
        .cat
        .plan_capture_adjustment(&targets, &shift, jst(), 5)
        .unwrap();
    eprintln!(
        "plan_capture_adjustment（変わる {} 件）: {:?}",
        plan.changed,
        t.elapsed()
    );
    let t = Instant::now();
    f.cat
        .adjust_capture_times(&targets, &shift, CaptureEditKind::Shift, jst(), 5)
        .unwrap();
    eprintln!("adjust_capture_times: {:?}", t.elapsed());
    let t = Instant::now();
    f.cat.undo_capture_time_edit(jst()).unwrap().unwrap();
    eprintln!("undo_capture_time_edit: {:?}", t.elapsed());
}
