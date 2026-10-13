//! 公開ビュー（スキーマの版 5。ORG-07）と、マイグレーションでのビューの扱い、構造の一覧のテスト。

use genzo_model::{ColorLabel, Rating};
use rusqlite::Connection;

use super::{Fixture, rows, video_req};
use crate::schema::{CATALOG_MIGRATIONS, Migration};
use crate::{
    CATALOG_APPLICATION_ID, CATALOG_SCHEMA_VERSION, Catalog, CatalogError, OpenOptions,
    PUBLIC_VIEWS, PUBLIC_VIEWS_SCHEMA_VERSION,
};

/// ビューの数。
fn view_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type = 'view'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn public_views_list_photos_videos_copies_and_keywords() {
    let mut f = Fixture::new();
    let photo = f.photo("A.ARW", 1, Some("2024:05:01 12:00:00"));
    let video = f
        .cat
        .register_file(&video_req(
            f.folder,
            "V.MP4",
            2,
            Some("2024:05:02 08:30:00"),
        ))
        .unwrap();
    f.cat
        .set_rating(&[photo.master_variant_id], Rating::MAX)
        .unwrap();
    f.cat
        .set_color_label(&[photo.master_variant_id], Some(ColorLabel::Red))
        .unwrap();
    f.cat
        .set_caption(photo.asset_id, Some("清水寺の朝"))
        .unwrap();
    let copy = f
        .cat
        .create_virtual_copy(photo.master_variant_id, Some("白黒"))
        .unwrap();
    let kyoto = f.cat.ensure_keyword_path(&["場所", "京都"]).unwrap();
    f.cat
        .add_keyword(&[photo.master_variant_id, copy], kyoto)
        .unwrap();

    assert_eq!(
        rows(&f.cat, "photos_v1"),
        3,
        "写真・仮想コピー・動画の 3 行"
    );
    /// photos_v1 の 1 行（確かめる列だけ）。
    #[derive(Debug, PartialEq)]
    struct Row {
        kind: String,
        is_virtual_copy: i64,
        copy_name: Option<String>,
        rating: i64,
        color_label: Option<String>,
        times: [Option<String>; 3],
        caption: Option<String>,
        place: [Option<String>; 3],
        file_status: Option<String>,
    }
    let read = |id: i64| -> Row {
        f.cat
            .conn
            .query_row(
                "SELECT kind, is_virtual_copy, copy_name, rating, color_label, captured_at_utc,
                        captured_at_local, utc_offset, caption, file_name, folder_path,
                        volume_mount_path, file_status
                 FROM photos_v1 WHERE variant_id = ?1",
                [id],
                |r| {
                    Ok(Row {
                        kind: r.get(0)?,
                        is_virtual_copy: r.get(1)?,
                        copy_name: r.get(2)?,
                        rating: r.get(3)?,
                        color_label: r.get(4)?,
                        times: [r.get(5)?, r.get(6)?, r.get(7)?],
                        caption: r.get(8)?,
                        place: [r.get(9)?, r.get(10)?, r.get(11)?],
                        file_status: r.get(12)?,
                    })
                },
            )
            .unwrap()
    };
    let s = |v: &str| Some(v.to_owned());
    assert_eq!(
        read(photo.master_variant_id.get()),
        Row {
            kind: "photo".to_owned(),
            is_virtual_copy: 0,
            copy_name: None,
            rating: 5,
            color_label: s("red"),
            times: [
                s("2024-05-01T03:00:00.000Z"),
                s("2024-05-01T12:00:00.000"),
                s("+09:00")
            ],
            caption: s("清水寺の朝"),
            place: [s("A.ARW"), s("2024/京都"), s("/mnt/photos")],
            file_status: s("ok"),
        }
    );
    // 仮想コピーは評価・ラベルをコピーしない。場所・撮影日時は同じ asset のもの。
    let c = read(copy.get());
    assert_eq!(
        (
            c.is_virtual_copy,
            c.copy_name.as_deref(),
            c.rating,
            c.color_label
        ),
        (1, Some("白黒"), 0, None)
    );
    assert_eq!(c.place[0].as_deref(), Some("A.ARW"));
    // 動画の情報。
    let (kind, duration, codec, local): (String, f64, String, String) = f
        .cat
        .conn
        .query_row(
            "SELECT kind, duration_s, video_codec, captured_at_local FROM photos_v1 WHERE variant_id = ?1",
            [video.master_variant_id.get()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (kind.as_str(), duration, codec.as_str(), local.as_str()),
        ("video", 12.5, "hevc", "2024-05-02T08:30:00.000")
    );

    // キーワード（階層は「 > 」でつなぐ）。
    let mut keywords: Vec<(i64, String, String)> = f
        .cat
        .conn
        .prepare(
            "SELECT variant_id, keyword, keyword_path FROM photo_keywords_v1 ORDER BY variant_id",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    keywords.sort();
    assert_eq!(
        keywords,
        vec![
            (
                photo.master_variant_id.get(),
                "京都".to_owned(),
                "場所 > 京都".to_owned()
            ),
            (copy.get(), "京都".to_owned(), "場所 > 京都".to_owned()),
        ]
    );
}

#[test]
fn photos_without_capture_time_have_null_times() {
    let mut f = Fixture::new();
    let o = f.photo("NODATE.JPG", 1, None);
    let (utc, local, offset): (Option<String>, Option<String>, Option<String>) = f
        .cat
        .conn
        .query_row(
            "SELECT captured_at_utc, captured_at_local, utc_offset FROM photos_v1 WHERE variant_id = ?1",
            [o.master_variant_id.get()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((utc, local, offset), (None, None, None));
}

#[test]
fn keyword_cycles_do_not_hang_the_keyword_view() {
    let mut f = Fixture::new();
    let o = f.photo("A.ARW", 1, None);
    let a = f.cat.ensure_keyword(None, "A").unwrap();
    let b = f.cat.ensure_keyword(Some(a), "B").unwrap();
    f.cat.add_keyword(&[o.master_variant_id], b).unwrap();
    // 壊れたカタログ（親子の循環）でも、ビューは止まる（循環の中のキーワードは最上位からたどれない）。
    f.cat
        .conn
        .execute(
            "UPDATE keyword SET parent_id = ?1 WHERE id = ?2",
            [b.get(), a.get()],
        )
        .unwrap();
    assert_eq!(rows(&f.cat, "photo_keywords_v1"), 0);
}

/// 試験用の次の版: SQLite の「12 の手順」で asset を作り直す（列を 1 つ足す）。公開ビューのことは書かない。
const TEST_REBUILD_ASSET: Migration = Migration {
    version: CATALOG_SCHEMA_VERSION + 1,
    description: "試験用: asset の作り直し",
    sql: "CREATE TABLE asset_new (
              id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, captured_at_raw TEXT,
              captured_offset TEXT, tz_source TEXT NOT NULL DEFAULT 'user_default', tz_assumed TEXT,
              time_correction_s INTEGER NOT NULL DEFAULT 0, captured_at_utc TEXT, camera TEXT,
              lens TEXT, iso INTEGER, aperture REAL, shutter REAL, focal REAL, width INTEGER,
              height INTEGER, orientation INTEGER NOT NULL DEFAULT 1, gps_lat REAL, gps_lon REAL,
              caption TEXT, created_at TEXT NOT NULL, camera_serial TEXT, tz_basis TEXT,
              test_note TEXT) STRICT;
          INSERT INTO asset_new SELECT *, NULL FROM asset;
          DROP TABLE asset;
          ALTER TABLE asset_new RENAME TO asset;
          CREATE INDEX asset_captured_at ON asset(captured_at_utc);",
    post: None,
};

/// 試験用の次の版: ビューが参照する列（asset.lens）を消す（公開ビューの約束を破る）。
const TEST_DROP_VIEW_COLUMN: Migration = Migration {
    version: CATALOG_SCHEMA_VERSION + 1,
    description: "試験用: ビューが参照する列を消す",
    sql: "DROP INDEX asset_lens; ALTER TABLE asset DROP COLUMN lens;",
    post: None,
};

/// 試験用の次の版: 公開ビューを新しい定義（列を足したもの）で作り直す。
const TEST_REDEFINE_VIEW: Migration = Migration {
    version: CATALOG_SCHEMA_VERSION + 1,
    description: "試験用: ビューの作り直し",
    sql: "DROP VIEW IF EXISTS photo_keywords_v1;
          CREATE VIEW photo_keywords_v1 AS
          SELECT vk.variant_id AS variant_id, k.id AS keyword_id, k.name AS keyword,
                 k.name AS keyword_path, 'x' AS test_extra
          FROM variant_keyword vk JOIN keyword k ON k.id = vk.keyword_id;",
    post: None,
};

fn with_next(next: Migration) -> Vec<Migration> {
    CATALOG_MIGRATIONS.iter().copied().chain([next]).collect()
}

#[test]
fn views_survive_a_migration_that_rebuilds_a_table() {
    // 「12 の手順」の ALTER TABLE ... RENAME は、参照するビューがあると失敗する（SQLite 3.26 以降）。
    // マイグレーションの間はビューを外しているので、マイグレーションの SQL はビューを考えなくてよい。
    let mut f = Fixture::new();
    let o = f.photo("A.ARW", 1, Some("2024:05:01 12:00:00"));
    let path = f.catalog_path();
    f.cat.close().unwrap();
    let cat = Catalog::open_with_migrations(
        &path,
        &OpenOptions::default(),
        &with_next(TEST_REBUILD_ASSET),
    )
    .unwrap();
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION + 1);
    assert_eq!(view_count(&cat.conn), PUBLIC_VIEWS.len() as i64);
    let name: String = cat
        .conn
        .query_row(
            "SELECT file_name FROM photos_v1 WHERE asset_id = ?1",
            [o.asset_id.get()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(name, "A.ARW");
    cat.close().unwrap();
}

#[test]
fn migration_that_breaks_a_public_view_is_rolled_back() {
    let mut f = Fixture::new();
    f.photo("A.ARW", 1, None);
    let path = f.catalog_path();
    f.cat.close().unwrap();
    let err = Catalog::open_with_migrations(
        &path,
        &OpenOptions::default(),
        &with_next(TEST_DROP_VIEW_COLUMN),
    )
    .unwrap_err();
    assert!(
        matches!(&err, CatalogError::Corrupt(m) if m.contains("photos_v1")),
        "{err}"
    );
    // 移行は戻り、ビューも列も元のまま。
    let cat = Catalog::open(&path).unwrap();
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
    assert_eq!(view_count(&cat.conn), PUBLIC_VIEWS.len() as i64);
    assert_eq!(rows(&cat, "photos_v1"), 1);
    cat.close().unwrap();
}

#[test]
fn migration_can_redefine_a_view() {
    let f = Fixture::new();
    let path = f.catalog_path();
    f.cat.close().unwrap();
    let cat = Catalog::open_with_migrations(
        &path,
        &OpenOptions::default(),
        &with_next(TEST_REDEFINE_VIEW),
    )
    .unwrap();
    // マイグレーションが作ったものは、元の定義で作り直さない。
    assert!(
        cat.conn
            .prepare("SELECT test_extra FROM photo_keywords_v1")
            .is_ok()
    );
    assert_eq!(view_count(&cat.conn), PUBLIC_VIEWS.len() as i64);
    cat.close().unwrap();
}

#[test]
fn version_4_catalog_gets_public_views() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let variant = {
        let mut cat =
            crate::testing::open_at_version(&path, PUBLIC_VIEWS_SCHEMA_VERSION - 1).unwrap();
        assert_eq!(view_count(&cat.conn), 0);
        let vol = cat.ensure_volume("v", None, Some("/media")).unwrap();
        let folder = cat.ensure_folder(vol, "2024").unwrap();
        let o = cat
            .register_file(&super::photo_req(
                folder,
                "A.ARW",
                1,
                Some("2024:05:01 12:00:00"),
            ))
            .unwrap();
        cat.close().unwrap();
        o.master_variant_id
    };
    let cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().migrated_from,
        Some(PUBLIC_VIEWS_SCHEMA_VERSION - 1)
    );
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
    for view in PUBLIC_VIEWS {
        let n: i64 = cat
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'view' AND name = ?1",
                [view],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{view}");
    }
    let (folder, mount): (String, String) = cat
        .conn
        .query_row(
            "SELECT folder_path, volume_mount_path FROM photos_v1 WHERE variant_id = ?1",
            [variant.get()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((folder.as_str(), mount.as_str()), ("2024", "/media"));
    cat.close().unwrap();
}

#[test]
fn public_views_can_be_read_through_a_read_only_connection() {
    let mut f = Fixture::new();
    f.photo("A.ARW", 1, None);
    // 外部のツールと同じく、SQLite の読み取り専用で開いて読む（アプリが開いている間も WAL で読める）。
    let ro =
        Connection::open_with_flags(f.catalog_path(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let n: i64 = ro
        .query_row("SELECT count(*) FROM photos_v1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    assert!(
        ro.execute("DELETE FROM variant", []).is_err(),
        "書き込めない"
    );
    drop(ro);
    f.cat.close().unwrap();
}

#[test]
fn schema_description_lists_tables_views_and_objects() {
    let f = Fixture::new();
    let d = f.cat.describe_schema().unwrap();
    assert_eq!(d.schema_version, CATALOG_SCHEMA_VERSION);
    assert_eq!(d.application_id, CATALOG_APPLICATION_ID);
    // 表・仮想表・ビュー。SQLite の内部の表と FTS5 の中身の表は除く。
    let asset = d.table("asset").unwrap();
    assert_eq!(asset.kind, "table");
    let id = &asset.columns[0];
    assert_eq!(
        (id.name.as_str(), id.decl_type.as_str(), id.primary_key),
        ("id", "INTEGER", 1)
    );
    let kind = asset.columns.iter().find(|c| c.name == "kind").unwrap();
    assert!(kind.not_null);
    let tz = asset
        .columns
        .iter()
        .find(|c| c.name == "tz_source")
        .unwrap();
    assert_eq!(tz.default_value.as_deref(), Some("'user_default'"));
    assert_eq!(d.table("asset_fts").unwrap().kind, "virtual");
    assert_eq!(d.table("photos_v1").unwrap().kind, "view");
    assert!(
        d.table("photos_v1")
            .unwrap()
            .columns
            .iter()
            .any(|c| c.name == "captured_at_local")
    );
    for hidden in [
        "sqlite_sequence",
        "asset_fts_data",
        "asset_fts_idx",
        "asset_fts_docsize",
        "asset_fts_config",
    ] {
        assert!(d.table(hidden).is_none(), "{hidden}");
        assert!(d.objects.iter().all(|o| o.name != hidden), "{hidden}");
    }
    // 索引・トリガー・ビューの SQL（自動の索引は除く）。
    let kinds = |k: &str| d.objects.iter().filter(|o| o.kind == k).count();
    assert!(kinds("index") >= 10);
    assert_eq!(kinds("trigger"), 4);
    assert_eq!(kinds("view"), PUBLIC_VIEWS.len());
    assert!(d.objects.iter().all(|o| !o.name.starts_with("sqlite_")));
    let fts = d.objects.iter().find(|o| o.name == "asset_fts").unwrap();
    assert!(fts.sql.contains("trigram"));
}
