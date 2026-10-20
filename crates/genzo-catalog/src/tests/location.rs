//! 位置情報の後付け（LIB-21）: 付ける・消す・見込み・履歴と取り消し・asset の削除・ファイルの読み直し・
//! 公開ビュー photos_v1 の使う位置の列・版 13 への移行。

use genzo_model::{AssetId, GpsCoord, PhotoMetadata};

use super::{Fixture, capture, facts, photo_meta, rows, video_req};
use crate::{
    CATALOG_SCHEMA_VERSION, Catalog, CatalogError, LOCATIONS_SCHEMA_VERSION, LocationEditKind,
    LocationPlan, MAX_LOCATION_EDITS, RegisterFile, RegisterOutcome,
};

/// 東京駅のあたり。
fn tokyo() -> GpsCoord {
    GpsCoord::new(35.681236, 139.767125).unwrap()
}

/// 清水寺のあたり。
fn kyoto() -> GpsCoord {
    GpsCoord::new(34.994856, 135.785046).unwrap()
}

/// シドニーのあたり（南半球・東経）。
fn sydney() -> GpsCoord {
    GpsCoord::new(-33.8568, 151.2153).unwrap()
}

/// カメラの位置（`gps`）のある写真の登録の要求。`n` はファイルの内容の番号。
fn gps_photo_req(f: &Fixture, name: &str, n: u64, gps: Option<GpsCoord>) -> RegisterFile {
    RegisterFile::photo(
        f.folder,
        name,
        facts(n),
        PhotoMetadata {
            gps,
            ..photo_meta("ILCE-7M4", "FE 35mm F1.8")
        },
        capture(Some("2024:05:01 12:00:00")),
    )
}

fn gps_photo(f: &mut Fixture, name: &str, n: u64, gps: Option<GpsCoord>) -> RegisterOutcome {
    let req = gps_photo_req(f, name, n, gps);
    f.cat.register_file(&req).unwrap()
}

/// 付けた日時（`asset_location.set_at`。付けていなければ `None`）。
fn set_at(cat: &Catalog, asset: AssetId) -> Option<String> {
    cat.conn
        .query_row(
            "SELECT set_at FROM asset_location WHERE asset_id = ?1",
            [asset.get()],
            |r| r.get(0),
        )
        .ok()
}

/// photos_v1 の使う位置の列（緯度・経度・出どころ）と、カメラの位置の列。
type ViewLocation = (
    Option<f64>,
    Option<f64>,
    Option<String>,
    Option<f64>,
    Option<f64>,
);

fn view_location(cat: &Catalog, variant_id: i64) -> ViewLocation {
    cat.conn
        .query_row(
            "SELECT location_lat, location_lon, location_source, gps_lat, gps_lon
             FROM photos_v1 WHERE variant_id = ?1",
            [variant_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap()
}

#[test]
fn locations_are_assigned_cleared_and_undone_in_order() {
    let mut f = Fixture::new();
    let a = gps_photo(&mut f, "A.ARW", 1, None);
    let b = gps_photo(&mut f, "B.ARW", 2, Some(kyoto()));
    let ids = [a.asset_id, b.asset_id];

    // 見込みは何も書かない。
    let plan = f.cat.plan_location_edit(&ids, Some(tokyo())).unwrap();
    assert_eq!(
        plan,
        LocationPlan {
            assets: 2,
            changed: 2,
            unchanged: 0,
            with_camera_gps: 1,
            with_assigned: 0,
            missing: 0,
        }
    );
    assert_eq!(rows(&f.cat, "asset_location"), 0);
    assert_eq!(rows(&f.cat, "location_edit"), 0);

    // 付ける。カメラの位置（gps）は変えず、使う位置は付けた位置になる。
    let set = f.cat.edit_locations(&ids, Some(tokyo())).unwrap();
    assert_eq!(set.plan, plan);
    let first = set.edit_id.unwrap();
    for (id, camera) in [(a.asset_id, None), (b.asset_id, Some(kyoto()))] {
        let rec = f.cat.asset(id).unwrap();
        assert_eq!(rec.gps, camera);
        assert_eq!(rec.assigned_gps, Some(tokyo()));
        assert_eq!(rec.effective_gps(), Some(tokyo()));
        assert_eq!(f.cat.asset_location(id).unwrap(), Some(tokyo()));
    }
    assert_eq!(rows(&f.cat, "location_edit_item"), 2);

    // 1 枚だけ別の位置にする。
    let moved = f.cat.edit_locations(&[b.asset_id], Some(sydney())).unwrap();
    assert_eq!(
        (
            moved.plan.changed,
            moved.plan.with_assigned,
            moved.plan.with_camera_gps
        ),
        (1, 1, 1)
    );
    // 消す（付けた位置のないものは変わらない）。
    let plan = f.cat.plan_location_edit(&ids, None).unwrap();
    assert_eq!(
        (plan.changed, plan.unchanged, plan.with_assigned),
        (2, 0, 2)
    );
    let cleared = f.cat.edit_locations(&ids, None).unwrap();
    assert_eq!(cleared.plan.changed, 2);
    assert_eq!(rows(&f.cat, "asset_location"), 0);
    let rec = f.cat.asset(b.asset_id).unwrap();
    assert_eq!(
        (rec.assigned_gps, rec.effective_gps()),
        (None, Some(kyoto()))
    );
    assert_eq!(f.cat.asset_location(a.asset_id).unwrap(), None);

    // 履歴は新しい順。
    let edits = f.cat.location_edits().unwrap();
    let summary: Vec<_> = edits
        .iter()
        .map(|e| (e.kind, e.location, e.asset_count))
        .collect();
    assert_eq!(
        summary,
        vec![
            (LocationEditKind::Clear, None, 2),
            (LocationEditKind::Set, Some(sydney()), 1),
            (LocationEditKind::Set, Some(tokyo()), 2),
        ]
    );
    assert_eq!(edits[2].id, first);

    // 新しい順に取り消す。
    let undo = f.cat.undo_location_edit().unwrap().unwrap();
    assert_eq!(undo.edit.kind, LocationEditKind::Clear);
    assert_eq!((undo.restored, undo.removed), (2, 0));
    assert_eq!(f.cat.asset_location(a.asset_id).unwrap(), Some(tokyo()));
    assert_eq!(f.cat.asset_location(b.asset_id).unwrap(), Some(sydney()));
    let undo = f.cat.undo_location_edit().unwrap().unwrap();
    assert_eq!(undo.edit.location, Some(sydney()));
    assert_eq!(undo.restored, 1);
    assert_eq!(f.cat.asset_location(b.asset_id).unwrap(), Some(tokyo()));
    let undo = f.cat.undo_location_edit().unwrap().unwrap();
    assert_eq!(undo.edit.id, first);
    assert_eq!(undo.restored, 2);
    assert_eq!(
        f.cat.asset(b.asset_id).unwrap().effective_gps(),
        Some(kyoto())
    );
    assert_eq!(rows(&f.cat, "asset_location"), 0);
    assert_eq!(rows(&f.cat, "location_edit_item"), 0);
    assert!(f.cat.undo_location_edit().unwrap().is_none());
    assert!(f.cat.location_edits().unwrap().is_empty());
}

#[test]
fn undo_restores_the_previous_location_and_when_it_was_set() {
    let mut f = Fixture::new();
    let a = gps_photo(&mut f, "A.ARW", 1, None);
    f.cat.edit_locations(&[a.asset_id], Some(tokyo())).unwrap();
    // 前に付けた日時を、見分けられる値にしておく。
    let earlier = "2020-01-02T03:04:05.006Z";
    f.cat
        .conn
        .execute("UPDATE asset_location SET set_at = ?1", [earlier])
        .unwrap();
    f.cat.edit_locations(&[a.asset_id], Some(kyoto())).unwrap();
    assert_ne!(set_at(&f.cat, a.asset_id).as_deref(), Some(earlier));
    f.cat.undo_location_edit().unwrap().unwrap();
    assert_eq!(f.cat.asset_location(a.asset_id).unwrap(), Some(tokyo()));
    assert_eq!(set_at(&f.cat, a.asset_id).as_deref(), Some(earlier));
}

#[test]
fn unchanged_assets_leave_no_history() {
    let mut f = Fixture::new();
    let a = gps_photo(&mut f, "A.ARW", 1, Some(kyoto()));
    let b = gps_photo(&mut f, "B.ARW", 2, None);
    // 付けた位置がないものを消しても、何も変わらない（カメラの位置は消さない）。
    let none = f
        .cat
        .edit_locations(&[a.asset_id, b.asset_id], None)
        .unwrap();
    assert_eq!(none.edit_id, None);
    assert_eq!((none.plan.changed, none.plan.unchanged), (0, 2));
    assert_eq!(rows(&f.cat, "location_edit"), 0);
    assert_eq!(f.cat.asset(a.asset_id).unwrap().gps, Some(kyoto()));

    f.cat.edit_locations(&[a.asset_id], Some(tokyo())).unwrap();
    let before = set_at(&f.cat, a.asset_id);
    // 同じ位置を付け直しても変わらない（付けた日時も変えない）。もう 1 枚だけが変わる。
    let again = f
        .cat
        .edit_locations(&[a.asset_id, b.asset_id], Some(tokyo()))
        .unwrap();
    assert_eq!(
        (
            again.plan.changed,
            again.plan.unchanged,
            again.plan.with_assigned
        ),
        (1, 1, 1)
    );
    assert_eq!(set_at(&f.cat, a.asset_id), before);
    let same = f.cat.edit_locations(&[a.asset_id], Some(tokyo())).unwrap();
    assert_eq!(same.edit_id, None);
    assert_eq!(rows(&f.cat, "location_edit"), 2);
    // 取り消すと、変えた 1 枚だけが戻る。
    let undo = f.cat.undo_location_edit().unwrap().unwrap();
    assert_eq!(undo.restored, 1);
    assert_eq!(f.cat.asset_location(a.asset_id).unwrap(), Some(tokyo()));
    assert_eq!(f.cat.asset_location(b.asset_id).unwrap(), None);
}

#[test]
fn out_of_range_locations_change_nothing() {
    let mut f = Fixture::new();
    let a = gps_photo(&mut f, "A.ARW", 1, None);
    f.cat.edit_locations(&[a.asset_id], Some(tokyo())).unwrap();
    for (lat, lon) in [
        (90.000001, 0.0),
        (-91.0, 0.0),
        (0.0, 180.5),
        (0.0, -1000.0),
        (f64::NAN, 0.0),
        (0.0, f64::INFINITY),
    ] {
        let bad = GpsCoord { lat, lon };
        let plan = f.cat.plan_location_edit(&[a.asset_id], Some(bad));
        assert!(
            matches!(plan, Err(CatalogError::InvalidInput(_))),
            "{lat}, {lon}: {plan:?}"
        );
        let err = f.cat.edit_locations(&[a.asset_id], Some(bad)).unwrap_err();
        assert!(matches!(err, CatalogError::InvalidInput(_)), "{err}");
    }
    assert_eq!(f.cat.asset_location(a.asset_id).unwrap(), Some(tokyo()));
    assert_eq!(rows(&f.cat, "location_edit"), 1);
    assert_eq!(rows(&f.cat, "location_edit_item"), 1);
    // 範囲の両端は付けられる。
    let edge = GpsCoord::new(-90.0, 180.0).unwrap();
    f.cat.edit_locations(&[a.asset_id], Some(edge)).unwrap();
    assert_eq!(f.cat.asset_location(a.asset_id).unwrap(), Some(edge));
    let edge = GpsCoord::new(90.0, -180.0).unwrap();
    f.cat.edit_locations(&[a.asset_id], Some(edge)).unwrap();
    assert_eq!(f.cat.asset_location(a.asset_id).unwrap(), Some(edge));
}

#[test]
fn duplicate_and_missing_ids_are_counted_once() {
    let mut f = Fixture::new();
    let a = gps_photo(&mut f, "A.ARW", 1, None);
    let b = gps_photo(&mut f, "B.ARW", 2, None);
    let gone = AssetId::new(9_999);
    let ids = [a.asset_id, b.asset_id, a.asset_id, gone, gone];
    let plan = f.cat.plan_location_edit(&ids, Some(tokyo())).unwrap();
    assert_eq!((plan.assets, plan.changed, plan.missing), (2, 2, 1));
    let done = f.cat.edit_locations(&ids, Some(tokyo())).unwrap();
    assert_eq!(done.plan, plan);
    assert_eq!(rows(&f.cat, "location_edit_item"), 2);
    // 見つからないものだけなら、何も記録しない。
    let none = f.cat.edit_locations(&[gone], Some(kyoto())).unwrap();
    assert_eq!((none.edit_id, none.plan.missing), (None, 1));
    assert!(matches!(
        f.cat.asset_location(gone),
        Err(CatalogError::NotFound(_))
    ));
    assert_eq!(
        f.cat.plan_location_edit(&[], None).unwrap(),
        LocationPlan::default()
    );
}

#[test]
fn removed_assets_drop_their_locations_and_history_is_limited() {
    let mut f = Fixture::new();
    let a = gps_photo(&mut f, "A.ARW", 1, None);
    let b = gps_photo(&mut f, "B.ARW", 2, None);
    f.cat
        .edit_locations(&[a.asset_id, b.asset_id], Some(tokyo()))
        .unwrap();
    assert_eq!(rows(&f.cat, "asset_location"), 2);
    f.cat.remove_assets(&[b.asset_id]).unwrap();
    // asset を消すと、付けた位置と履歴の明細も消える（ON DELETE CASCADE）。履歴の行は残る。
    assert_eq!(rows(&f.cat, "asset_location"), 1);
    assert_eq!(rows(&f.cat, "location_edit_item"), 1);
    let undo = f.cat.undo_location_edit().unwrap().unwrap();
    assert_eq!(undo.edit.asset_count, 2);
    assert_eq!((undo.restored, undo.removed), (1, 1));
    assert_eq!(rows(&f.cat, "asset_location"), 0);

    // 履歴は新しい順に MAX_LOCATION_EDITS 回分だけ残る。
    let spots = [tokyo(), kyoto(), sydney()];
    for i in 0..(MAX_LOCATION_EDITS + 3) {
        f.cat
            .edit_locations(&[a.asset_id], Some(spots[i % spots.len()]))
            .unwrap();
        assert!(
            f.cat.location_edits().unwrap().len() <= MAX_LOCATION_EDITS,
            "{i}"
        );
    }
    assert_eq!(rows(&f.cat, "location_edit") as usize, MAX_LOCATION_EDITS);
    assert_eq!(
        rows(&f.cat, "location_edit_item") as usize,
        MAX_LOCATION_EDITS
    );
    for _ in 0..MAX_LOCATION_EDITS {
        f.cat.undo_location_edit().unwrap().unwrap();
    }
    assert!(f.cat.undo_location_edit().unwrap().is_none());
    // 消えた古い履歴の分は戻らない（残っていた最も古い変更の前の位置になる）。
    assert_eq!(
        f.cat.asset_location(a.asset_id).unwrap(),
        Some(spots[2 % spots.len()])
    );
}

#[test]
fn rereading_a_changed_file_keeps_the_assigned_location() {
    let mut f = Fixture::new();
    let a = gps_photo(&mut f, "A.ARW", 1, Some(kyoto()));
    f.cat.edit_locations(&[a.asset_id], Some(tokyo())).unwrap();
    // 内容が変わった（別のアプリで位置を書き込んだ）ファイルを読み直す。カメラの位置はファイルの値になり、
    // 付けた位置は残る。
    let req = gps_photo_req(&f, "A.ARW", 77, Some(sydney()));
    let again = f.cat.register_file(&req).unwrap();
    assert_eq!(again.asset_id, a.asset_id);
    let rec = f.cat.asset(a.asset_id).unwrap();
    assert_eq!(rec.gps, Some(sydney()));
    assert_eq!(rec.assigned_gps, Some(tokyo()));
    assert_eq!(rec.effective_gps(), Some(tokyo()));
    // 消すと、読み直したカメラの位置に戻る。
    f.cat.edit_locations(&[a.asset_id], None).unwrap();
    assert_eq!(
        f.cat.asset(a.asset_id).unwrap().effective_gps(),
        Some(sydney())
    );
}

#[test]
fn photos_view_shows_the_location_in_use_and_its_source() {
    let mut f = Fixture::new();
    let assigned = gps_photo(&mut f, "A.ARW", 1, Some(kyoto()));
    let camera = gps_photo(&mut f, "B.ARW", 2, Some(sydney()));
    let none = gps_photo(&mut f, "C.ARW", 3, None);
    let half = gps_photo(&mut f, "D.ARW", 4, None);
    let video = f
        .cat
        .register_file(&video_req(f.folder, "V.MP4", 5, None))
        .unwrap();
    f.cat
        .edit_locations(&[assigned.asset_id, video.asset_id], Some(tokyo()))
        .unwrap();
    // カメラの位置が片方だけ（外部のツールで書き換えたなど）。緯度と経度を別々の出どころから混ぜない。
    f.cat
        .conn
        .execute(
            "UPDATE asset SET gps_lat = 35.0, gps_lon = NULL WHERE id = ?1",
            [half.asset_id.get()],
        )
        .unwrap();
    let copy = f
        .cat
        .create_virtual_copy(assigned.master_variant_id, Some("白黒"))
        .unwrap();
    let s = |v: &str| Some(v.to_owned());
    let k = kyoto();
    let (t, y) = (tokyo(), sydney());
    let expected_assigned = (
        Some(t.lat),
        Some(t.lon),
        s("assigned"),
        Some(k.lat),
        Some(k.lon),
    );
    assert_eq!(
        view_location(&f.cat, assigned.master_variant_id.get()),
        expected_assigned
    );
    // 仮想コピーも同じ asset の位置。
    assert_eq!(view_location(&f.cat, copy.get()), expected_assigned);
    assert_eq!(
        view_location(&f.cat, camera.master_variant_id.get()),
        (
            Some(y.lat),
            Some(y.lon),
            s("camera"),
            Some(y.lat),
            Some(y.lon)
        )
    );
    assert_eq!(
        view_location(&f.cat, none.master_variant_id.get()),
        (None, None, None, None, None)
    );
    assert_eq!(
        view_location(&f.cat, half.master_variant_id.get()),
        (None, None, None, Some(35.0), None)
    );
    // カタログは動画にも付けられる（動画を除くのはコア API）。
    assert_eq!(
        view_location(&f.cat, video.master_variant_id.get()),
        (Some(t.lat), Some(t.lon), s("assigned"), None, None)
    );
    // 消すとカメラの位置に戻る。
    f.cat.edit_locations(&[assigned.asset_id], None).unwrap();
    assert_eq!(
        view_location(&f.cat, copy.get()),
        (
            Some(k.lat),
            Some(k.lon),
            s("camera"),
            Some(k.lat),
            Some(k.lon)
        )
    );
}

#[test]
fn schema_rejects_inconsistent_rows() {
    // 外部のツールが書いた不正な行は、表の CHECK で入らない。
    let mut f = Fixture::new();
    let a = gps_photo(&mut f, "A.ARW", 1, None);
    let id = a.asset_id.get();
    let conn = &f.cat.conn;
    let now = "2024-05-01T00:00:00.000Z";
    for (lat, lon) in [(90.5, 0.0), (0.0, -180.5)] {
        assert!(
            conn.execute(
                "INSERT INTO asset_location(asset_id, lat, lon, set_at) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![id, lat, lon, now],
            )
            .is_err(),
            "{lat}, {lon}"
        );
    }
    for (kind, lat, lon) in [
        ("set", None, None),
        ("set", Some(1.0), None),
        ("clear", Some(1.0), Some(2.0)),
        ("clear", None, Some(2.0)),
        ("gpx", Some(1.0), Some(2.0)),
    ] {
        assert!(
            conn.execute(
                "INSERT INTO location_edit(kind, lat, lon, asset_count, created_at)
                 VALUES (?1, ?2, ?3, 0, ?4)",
                rusqlite::params![kind, lat, lon, now],
            )
            .is_err(),
            "{kind} {lat:?} {lon:?}"
        );
    }
    conn.execute(
        "INSERT INTO location_edit(kind, lat, lon, asset_count, created_at) VALUES ('clear', NULL, NULL, 1, ?1)",
        [now],
    )
    .unwrap();
    let edit = conn.last_insert_rowid();
    for (lat, lon, at) in [
        (Some(1.0), None, Some(now)),
        (Some(1.0), Some(2.0), None),
        (None, None, Some(now)),
    ] {
        assert!(
            conn.execute(
                "INSERT INTO location_edit_item(edit_id, asset_id, prev_lat, prev_lon, prev_set_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![edit, id, lat, lon, at],
            )
            .is_err(),
            "{lat:?} {lon:?} {at:?}"
        );
    }
}

/// 版 12 のカタログを開くと版 13 に移行し、付けた位置の表（空）・履歴の表・photos_v1 の列ができる。カメラの
/// 位置は変わらない。
#[test]
fn version_12_catalog_migrates_to_asset_locations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let (with_gps, without_gps) = {
        let mut cat = crate::testing::open_at_version(&path, LOCATIONS_SCHEMA_VERSION - 1).unwrap();
        let vol = cat.ensure_volume("v", None, None).unwrap();
        let folder = cat.ensure_folder(vol, "2024").unwrap();
        let photo = |name: &str, n: u64, gps: Option<GpsCoord>| {
            RegisterFile::photo(
                folder,
                name,
                facts(n),
                PhotoMetadata {
                    gps,
                    ..photo_meta("ILCE-7M4", "FE 35mm F1.8")
                },
                capture(Some("2024:05:01 12:00:00")),
            )
        };
        let with_gps = cat
            .register_file(&photo("A.ARW", 1, Some(kyoto())))
            .unwrap();
        let without_gps = cat.register_file(&photo("B.ARW", 2, None)).unwrap();
        assert!(
            cat.conn
                .prepare("SELECT asset_id FROM asset_location")
                .is_err()
        );
        assert!(
            cat.conn
                .prepare("SELECT location_source FROM photos_v1")
                .is_err()
        );
        cat.close().unwrap();
        (with_gps, without_gps)
    };
    let cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().migrated_from,
        Some(LOCATIONS_SCHEMA_VERSION - 1)
    );
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
    assert_eq!(rows(&cat, "asset_location"), 0);
    assert_eq!(rows(&cat, "location_edit"), 0);
    let rec = cat.asset(with_gps.asset_id).unwrap();
    assert_eq!((rec.gps, rec.assigned_gps), (Some(kyoto()), None));
    let k = kyoto();
    assert_eq!(
        view_location(&cat, with_gps.master_variant_id.get()),
        (
            Some(k.lat),
            Some(k.lon),
            Some("camera".to_owned()),
            Some(k.lat),
            Some(k.lon)
        )
    );
    assert_eq!(
        view_location(&cat, without_gps.master_variant_id.get()),
        (None, None, None, None, None)
    );
    // 移行の後は、位置を付けられる。
    cat.close().unwrap();
    let mut cat = Catalog::open(&path).unwrap();
    let done = cat
        .edit_locations(&[without_gps.asset_id], Some(tokyo()))
        .unwrap();
    assert!(done.edit_id.is_some());
    assert_eq!(
        cat.asset_location(without_gps.asset_id).unwrap(),
        Some(tokyo())
    );
    cat.close().unwrap();
}
