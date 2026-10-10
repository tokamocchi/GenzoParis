//! 接続の設定・スキーマ・制約・マイグレーション・起動時の確認のテスト。

use rusqlite::Connection;

use super::{Fixture, rows};
use crate::schema::{CATALOG_MIGRATIONS, Migration};
use crate::{
    CATALOG_APPLICATION_ID, CATALOG_SCHEMA_VERSION, Catalog, CatalogError, OpenOptions,
    PreviousShutdown,
};

#[test]
fn catalog_connection_uses_wal_full_sync_and_foreign_keys() {
    let f = Fixture::new();
    let s = f.cat.connection_settings().unwrap();
    assert_eq!(s.journal_mode, "wal");
    assert_eq!(s.synchronous, 2, "synchronous = FULL");
    assert!(s.foreign_keys);
    // macOS では fullfsync も有効にする（3.4 節）。
    assert_eq!(s.fullfsync, cfg!(target_os = "macos"));
    assert_eq!(f.cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
    let app_id: i32 = f
        .cat
        .conn
        .pragma_query_value(None, "application_id", |r| r.get(0))
        .unwrap();
    assert_eq!(app_id, CATALOG_APPLICATION_ID);
    let busy: i64 = f
        .cat
        .conn
        .pragma_query_value(None, "busy_timeout", |r| r.get(0))
        .unwrap();
    assert_eq!(busy, crate::BUSY_TIMEOUT.as_millis() as i64);
}

#[test]
fn all_tables_of_the_er_diagram_exist() {
    let f = Fixture::new();
    for table in [
        "volume",
        "folder",
        "file",
        "asset",
        "variant",
        "history_entry",
        "snapshot",
        "keyword",
        "variant_keyword",
        "collection",
        "collection_member",
        "stack",
        "stack_member",
        "video_meta",
        "file_op",
        "app_state",
        "setting",
        "asset_text",
        "asset_fts",
    ] {
        let n: i64 = f
            .cat
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name = ?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{table}");
    }
    // 3.2 節のインデックス。
    for index in [
        "asset_captured_at",
        "asset_camera",
        "asset_lens",
        "variant_asset",
        "variant_rating",
        "variant_keyword_keyword",
        "variant_one_master",
        "file_one_primary",
    ] {
        let n: i64 = f
            .cat
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'index' AND name = ?1",
                [index],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "{index}");
    }
}

#[test]
fn check_constraints_reject_invalid_rating_flag_and_label() {
    let mut f = Fixture::new();
    let o = f.photo("A.ARW", 1, None);
    let v = o.master_variant_id.get();
    let conn = &f.cat.conn;
    for sql in [
        "UPDATE variant SET rating = 6 WHERE id = ?1",
        "UPDATE variant SET rating = -1 WHERE id = ?1",
        "UPDATE variant SET flag = 2 WHERE id = ?1",
        "UPDATE variant SET color_label = 'pink' WHERE id = ?1",
        "UPDATE variant SET is_master = 2 WHERE id = ?1",
        // STRICT のテーブルは型の違う値を拒否する。
        "UPDATE variant SET rating = 'abc' WHERE id = ?1",
    ] {
        let err = conn.execute(sql, [v]).unwrap_err();
        assert!(
            CatalogError::from(err).is_constraint_violation() || sql.contains("'abc'"),
            "{sql}"
        );
    }
    // 境界の値は通る。
    for sql in [
        "UPDATE variant SET rating = 0 WHERE id = ?1",
        "UPDATE variant SET rating = 5 WHERE id = ?1",
        "UPDATE variant SET flag = -1 WHERE id = ?1",
        "UPDATE variant SET flag = 1 WHERE id = ?1",
        "UPDATE variant SET color_label = NULL WHERE id = ?1",
    ] {
        conn.execute(sql, [v]).unwrap();
    }
}

#[test]
fn only_one_master_and_one_primary_per_asset() {
    let mut f = Fixture::new();
    let o = f.photo("A.ARW", 1, None);
    let err = f
        .cat
        .conn
        .execute(
            "INSERT INTO variant(asset_id, is_master, develop_hash, process_version, created_at, updated_at)
             VALUES (?1, 1, 'x', 1, 'now', 'now')",
            [o.asset_id.get()],
        )
        .unwrap_err();
    assert!(CatalogError::from(err).is_constraint_violation());
    // 仮想コピー（is_master = 0）はいくつでも作れる。
    f.cat
        .create_virtual_copy(o.master_variant_id, None)
        .unwrap();
    f.cat
        .create_virtual_copy(o.master_variant_id, None)
        .unwrap();

    let err = f
        .cat
        .conn
        .execute(
            "INSERT INTO file(asset_id, folder_id, name, name_key, role, size, mtime, quick_hash)
             VALUES (?1, ?2, 'B.ARW', 'b.arw', 'primary', 1, 1, 'x')",
            [o.asset_id.get(), f.folder.get()],
        )
        .unwrap_err();
    assert!(CatalogError::from(err).is_constraint_violation());
}

#[test]
fn unique_folder_and_file_keys_reject_duplicates() {
    let mut f = Fixture::new();
    f.photo("A.ARW", 1, None);
    let err = f
        .cat
        .conn
        .execute(
            "INSERT INTO folder(volume_id, parent_id, rel_path, rel_path_key)
             SELECT volume_id, parent_id, '2024/京都', rel_path_key FROM folder WHERE id = ?1",
            [f.folder.get()],
        )
        .unwrap_err();
    assert!(CatalogError::from(err).is_constraint_violation());
    let asset: i64 = f
        .cat
        .conn
        .query_row("SELECT id FROM asset", [], |r| r.get(0))
        .unwrap();
    let err = f
        .cat
        .conn
        .execute(
            "INSERT INTO file(asset_id, folder_id, name, name_key, role, size, mtime, quick_hash)
             VALUES (?1, ?2, 'a.arw', 'a.arw', 'sidecar_jpeg', 1, 1, 'x')",
            [asset, f.folder.get()],
        )
        .unwrap_err();
    assert!(CatalogError::from(err).is_constraint_violation());
}

#[test]
fn foreign_keys_are_enforced() {
    let f = Fixture::new();
    let err = f
        .cat
        .conn
        .execute(
            "INSERT INTO variant(asset_id, is_master, develop_hash, process_version, created_at, updated_at)
             VALUES (9999, 0, 'x', 1, 'now', 'now')",
            [],
        )
        .unwrap_err();
    assert!(CatalogError::from(err).is_constraint_violation());
}

#[test]
fn deleting_an_asset_cascades_but_keeps_keywords_and_collections() {
    let mut f = Fixture::new();
    let o = f.photo("A.ARW", 1, Some("2024:05:01 12:00:00"));
    let jpg = f.photo("A.JPG", 2, Some("2024:05:01 12:00:00"));
    assert_eq!(jpg.asset_id, o.asset_id);
    let vc = f
        .cat
        .create_virtual_copy(o.master_variant_id, None)
        .unwrap();
    let kw = f.cat.ensure_keyword_path(&["場所", "京都"]).unwrap();
    f.cat.add_keyword(&[o.master_variant_id, vc], kw).unwrap();
    f.cat.create_snapshot(vc, "s").unwrap();
    f.cat
        .save_develop(
            vc,
            &genzo_model::DevelopSettings {
                exposure_ev: 1.0,
                ..Default::default()
            },
            "露光量",
        )
        .unwrap();
    f.cat
        .conn
        .execute(
            "INSERT INTO collection(id, name, kind) VALUES (1, 'お気に入り', 'manual')",
            [],
        )
        .unwrap();
    f.cat
        .conn
        .execute(
            "INSERT INTO collection_member(collection_id, variant_id) VALUES (1, ?1)",
            [vc.get()],
        )
        .unwrap();
    f.cat
        .conn
        .execute(
            "INSERT INTO stack(id, top_variant_id) VALUES (1, ?1)",
            [vc.get()],
        )
        .unwrap();
    f.cat
        .conn
        .execute(
            "INSERT INTO stack_member(stack_id, variant_id, position) VALUES (1, ?1, 0)",
            [vc.get()],
        )
        .unwrap();

    let report = f.cat.remove_assets(&[o.asset_id]).unwrap();
    assert_eq!(report.assets, vec![o.asset_id]);
    assert_eq!(report.variants, vec![o.master_variant_id, vc]);
    assert_eq!(report.files.len(), 2);
    for table in [
        "asset",
        "file",
        "variant",
        "history_entry",
        "snapshot",
        "variant_keyword",
        "collection_member",
        "stack_member",
        "asset_text",
    ] {
        assert_eq!(f.rows(table), 0, "{table}");
    }
    // キーワード・コレクション・スタック自体は残る。
    assert_eq!(f.rows("keyword"), 2);
    assert_eq!(f.rows("collection"), 1);
    assert_eq!(f.rows("stack"), 1);
    let top: Option<i64> = f
        .cat
        .conn
        .query_row("SELECT top_variant_id FROM stack", [], |r| r.get(0))
        .unwrap();
    assert_eq!(top, None);
    // テキスト検索の索引からも消える。
    let report = f.cat.check_integrity().unwrap();
    assert!(report.is_ok(), "{report:?}");
    let hits: i64 = f
        .cat
        .conn
        .query_row(
            "SELECT count(*) FROM asset_fts WHERE asset_fts MATCH '\"a.arw\"'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hits, 0);
}

#[test]
fn clean_and_unclean_shutdown_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().previous_shutdown,
        PreviousShutdown::FirstOpen
    );
    assert_eq!(cat.open_report().migrated_from, Some(0));
    assert!(cat.open_report().migration_backup.is_none());
    assert_eq!(
        cat.app_state("shutdown").unwrap().as_deref(),
        Some("in_use")
    );
    cat.close().unwrap();

    let cat = Catalog::open(&path).unwrap();
    assert_eq!(cat.open_report().previous_shutdown, PreviousShutdown::Clean);
    assert_eq!(cat.open_report().migrated_from, None);
    // close を呼ばずに破棄する（異常終了の代わり）。
    drop(cat);

    let cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().previous_shutdown,
        PreviousShutdown::Unclean
    );
    assert!(cat.app_state("last_opened_at").unwrap().is_some());
    cat.close().unwrap();
}

#[test]
fn settings_are_stored_as_key_values() {
    let mut f = Fixture::new();
    assert_eq!(f.cat.setting("default_tz").unwrap(), None);
    f.cat.set_setting("default_tz", "+09:00").unwrap();
    f.cat.set_setting("default_tz", "+01:00").unwrap();
    assert_eq!(
        f.cat.setting("default_tz").unwrap().as_deref(),
        Some("+01:00")
    );
    assert!(f.cat.remove_setting("default_tz").unwrap());
    assert!(!f.cat.remove_setting("default_tz").unwrap());
}

#[test]
fn future_schema_version_is_rejected_without_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    Catalog::open(&path).unwrap().close().unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    let err = Catalog::open(&path).unwrap_err();
    assert!(
        matches!(
            err,
            CatalogError::FutureSchema {
                found: 99,
                supported: CATALOG_SCHEMA_VERSION
            }
        ),
        "{err}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(matches!(
        Catalog::open_read_only(&path),
        Err(CatalogError::FutureSchema { .. })
    ));
    assert!(matches!(
        crate::restore_backup(&path, &dir.path().join("r.db")),
        Err(CatalogError::FutureSchema { .. })
    ));
}

#[test]
fn foreign_sqlite_files_and_non_databases_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("other.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);")
            .unwrap();
    }
    assert!(matches!(
        Catalog::open(&path),
        Err(CatalogError::NotOurDatabase(_))
    ));
    // 他の DB を WAL に切り替えるなどの変更をしていない。
    let conn = Connection::open(&path).unwrap();
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "delete");

    let text = dir.path().join("text.db");
    std::fs::write(&text, b"this is not a database file at all, just text....").unwrap();
    assert!(matches!(
        Catalog::open(&text),
        Err(CatalogError::NotOurDatabase(_))
    ));

    // サムネイル DB をカタログとして開かない。
    let thumbs = dir.path().join("thumbs.db");
    crate::ThumbStore::open(&thumbs).unwrap().close().unwrap();
    assert!(matches!(
        Catalog::open(&thumbs),
        Err(CatalogError::NotOurDatabase(_))
    ));
}

/// 試験用の版 2（列の追加と、Rust の処理による値の書き込み）。
const TEST_V2: Migration = Migration {
    version: 2,
    description: "試験用: asset に列を追加",
    sql: "ALTER TABLE asset ADD COLUMN test_note TEXT;",
    post: Some(|tx| {
        tx.execute("UPDATE asset SET test_note = 'migrated'", [])?;
        Ok(())
    }),
};

#[test]
fn old_catalog_is_backed_up_and_migrated() {
    let mut f = Fixture::new();
    let o = f.photo("A.ARW", 1, Some("2024:05:01 12:00:00"));
    let path = f.catalog_path();
    f.cat.close().unwrap();
    let backup_dir = f.dir.path().join("migration-backups");

    let migrations = [CATALOG_MIGRATIONS[0], TEST_V2];
    let cat = Catalog::open_with_migrations(
        &path,
        &OpenOptions {
            migration_backup_dir: Some(backup_dir.clone()),
        },
        &migrations,
    )
    .unwrap();
    let report = cat.open_report().clone();
    assert_eq!(report.migrated_from, Some(1));
    assert_eq!(report.schema_version, 2);
    assert_eq!(cat.schema_version().unwrap(), 2);
    assert_eq!(report.previous_shutdown, PreviousShutdown::Clean);
    // データは残り、移行の処理が動いた。
    let note: String = cat
        .conn
        .query_row(
            "SELECT test_note FROM asset WHERE id = ?1",
            [o.asset_id.get()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(note, "migrated");
    // 外部キーの確認が元に戻っている。
    assert!(cat.connection_settings().unwrap().foreign_keys);

    // 移行の前のバックアップがあり、版 1 のままのデータを持つ。
    let backup = report.migration_backup.expect("バックアップを作る");
    assert!(backup.starts_with(&backup_dir));
    assert!(
        backup
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("catalog.v1-before-migration-")
    );
    let b = Connection::open(&backup).unwrap();
    let v: i64 = b
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, 1);
    let n: i64 = b
        .query_row("SELECT count(*) FROM asset", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    assert!(b.prepare("SELECT test_note FROM asset").is_err());
    cat.close().unwrap();

    // 版 2 の DB は、版 1 までしか知らないアプリでは開けない。
    assert!(matches!(
        Catalog::open(&path),
        Err(CatalogError::FutureSchema {
            found: 2,
            supported: 1
        })
    ));
}

#[test]
fn failed_migration_is_rolled_back() {
    let f = Fixture::new();
    let path = f.catalog_path();
    f.cat.close().unwrap();
    const BROKEN_V2: Migration = Migration {
        version: 2,
        description: "試験用: 途中で失敗する",
        sql: "ALTER TABLE asset ADD COLUMN x TEXT; SELECT * FROM no_such_table;",
        post: None,
    };
    let migrations = [CATALOG_MIGRATIONS[0], BROKEN_V2];
    assert!(Catalog::open_with_migrations(&path, &OpenOptions::default(), &migrations).is_err());
    // 版 1 のまま、列も追加されていない。開き直せる。
    let cat = Catalog::open(&path).unwrap();
    assert_eq!(cat.schema_version().unwrap(), 1);
    assert!(cat.conn.prepare("SELECT x FROM asset").is_err());
    assert!(cat.connection_settings().unwrap().foreign_keys);
    // 移行の前のバックアップは作られている（失敗しても残す）。
    let backups: Vec<_> = std::fs::read_dir(f.dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("before-migration"))
        .collect();
    assert_eq!(backups.len(), 1);
}

#[test]
fn new_catalog_does_not_create_a_migration_backup() {
    let dir = tempfile::tempdir().unwrap();
    let cat = Catalog::open(dir.path().join("catalog.db")).unwrap();
    assert!(cat.open_report().migration_backup.is_none());
    cat.close().unwrap();
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|n| n.starts_with("catalog.db")),
        "{names:?}"
    );
}

#[test]
fn read_only_connection_reads_while_writer_is_open() {
    let mut f = Fixture::new();
    f.photo("A.ARW", 1, Some("2024:05:01 12:00:00"));
    let reader = Catalog::open_read_only(f.catalog_path()).unwrap();
    assert!(reader.is_read_only());
    assert_eq!(
        reader
            .search(&crate::Filter::default(), &crate::Sort::default())
            .unwrap()
            .len(),
        1
    );
    // 書き込み側の変更（コミット後）が見える。
    f.photo("B.ARW", 2, Some("2024:05:01 12:00:01"));
    assert_eq!(rows(&reader, "asset"), 2);
    // 読み取り専用の接続では書き込めない。
    let mut reader = reader;
    assert!(reader.set_setting("k", "v").is_err());
    // 詳細チェックは読み取り専用でも行える（FTS の確認は飛ばす）。
    assert!(reader.check_integrity().unwrap().is_ok());
    reader.close().unwrap();
    // 読み取り専用で閉じても、書き込み側の「使用中」の印は変えない。
    assert_eq!(
        f.cat.app_state("shutdown").unwrap().as_deref(),
        Some("in_use")
    );
}

#[test]
fn read_only_open_requires_current_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.db");
    Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA user_version = 0;")
        .unwrap();
    assert!(matches!(
        Catalog::open_read_only(&path),
        Err(CatalogError::OutdatedSchema { found: 0, .. })
    ));
}

#[test]
fn in_memory_catalog_works() {
    let mut cat = Catalog::open_in_memory().unwrap();
    assert!(cat.path().is_none());
    let v = cat.ensure_volume("m", None, None).unwrap();
    let folder = cat.ensure_folder(v, "").unwrap();
    let o = cat
        .register_file(&super::photo_req(folder, "A.ARW", 1, None))
        .unwrap();
    assert_eq!(
        cat.search(&crate::Filter::default(), &crate::Sort::default())
            .unwrap(),
        vec![o.master_variant_id]
    );
    assert_eq!(cat.backup_stem(), "catalog");
    cat.close().unwrap();
}
