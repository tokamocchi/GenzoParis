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
    // 仮想コピーは元の写真と同じスタックに入る（LIB-12・LIB-13。版 9 から）。
    assert_eq!(f.rows("stack_member"), 2);

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
    // キーワード・コレクション自体は残る。写真がなくなったスタックは消える（版 9 のトリガー
    // stack_member_after_delete。1 枚以下のスタックを残さない）。
    assert_eq!(f.rows("keyword"), 2);
    assert_eq!(f.rows("collection"), 1);
    assert_eq!(f.rows("stack"), 0);
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

/// 複数の設定と、撮影日時の推定し直しが済んでいない印を、1 つのトランザクションで保存する。
#[test]
fn several_settings_and_the_reresolve_mark_are_saved_together() {
    let mut f = Fixture::new();
    f.cat.set_setting("a", "old").unwrap();
    // 何もなければ、何も書かない（印も付けない）。
    f.cat.set_settings(&[], false).unwrap();
    assert!(!f.cat.capture_time_reresolve_pending().unwrap());
    // 印を付けない保存は、印に触れない。
    f.cat
        .set_settings(&[("a", "new"), ("b", "1")], false)
        .unwrap();
    assert_eq!(f.cat.setting("a").unwrap().as_deref(), Some("new"));
    assert_eq!(f.cat.setting("b").unwrap().as_deref(), Some("1"));
    assert!(!f.cat.capture_time_reresolve_pending().unwrap());
    // 同じキーを続けて指定したときは、後の値が残る。印は、設定がなくても付けられる。
    f.cat.set_settings(&[("b", "2"), ("b", "3")], true).unwrap();
    assert_eq!(f.cat.setting("b").unwrap().as_deref(), Some("3"));
    assert!(f.cat.capture_time_reresolve_pending().unwrap());
    f.cat.set_settings(&[], true).unwrap();
    assert!(f.cat.capture_time_reresolve_pending().unwrap());
}

/// 後の設定の書き込みが失敗したら、先の設定も印も保存しない（ディスクが一杯・入出力のエラーなどでの
/// 途中の失敗で、一部だけ保存されない）。失敗の後も、同じカタログに書ける。
#[test]
fn settings_are_not_partly_saved_when_a_later_write_fails() {
    let mut f = Fixture::new();
    f.cat.set_setting("a", "old").unwrap();
    let path = f.cat.path().unwrap().to_path_buf();
    // 3 つ目の項目（`c`）の書き込みだけを失敗させる。
    crate::testing::fail_setting_writes(&path, "c").unwrap();
    let result = f
        .cat
        .set_settings(&[("a", "new"), ("b", "1"), ("c", "2")], true);
    assert!(matches!(result, Err(CatalogError::Sqlite(_))), "{result:?}");
    assert_eq!(f.cat.setting("a").unwrap().as_deref(), Some("old"));
    assert_eq!(f.cat.setting("b").unwrap(), None);
    assert_eq!(f.cat.setting("c").unwrap(), None);
    assert!(!f.cat.capture_time_reresolve_pending().unwrap());
    // 失敗したトランザクションが残らず、続けて書ける（失敗する項目以外）。
    f.cat.set_setting("d", "1").unwrap();
    assert_eq!(f.cat.setting("d").unwrap().as_deref(), Some("1"));
    // 失敗の原因がなくなれば、同じ保存が通る。
    crate::testing::allow_setting_writes(&path).unwrap();
    f.cat
        .set_settings(&[("a", "new"), ("b", "1"), ("c", "2")], true)
        .unwrap();
    assert_eq!(f.cat.setting("a").unwrap().as_deref(), Some("new"));
    assert_eq!(f.cat.setting("c").unwrap().as_deref(), Some("2"));
    assert!(f.cat.capture_time_reresolve_pending().unwrap());
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

/// 試験用の次の版（列の追加と、Rust の処理による値の書き込み）。
const TEST_NEXT: Migration = Migration {
    version: CATALOG_SCHEMA_VERSION + 1,
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

    let migrations: Vec<Migration> = CATALOG_MIGRATIONS
        .iter()
        .copied()
        .chain([TEST_NEXT])
        .collect();
    let cat = Catalog::open_with_migrations(
        &path,
        &OpenOptions {
            migration_backup_dir: Some(backup_dir.clone()),
        },
        &migrations,
    )
    .unwrap();
    let report = cat.open_report().clone();
    assert_eq!(report.migrated_from, Some(CATALOG_SCHEMA_VERSION));
    assert_eq!(report.schema_version, CATALOG_SCHEMA_VERSION + 1);
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION + 1);
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

    // 移行の前のバックアップがあり、移行前の版のままのデータを持つ。
    let backup = report.migration_backup.expect("バックアップを作る");
    assert!(backup.starts_with(&backup_dir));
    assert!(
        backup
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with(&format!(
                "catalog.v{CATALOG_SCHEMA_VERSION}-before-migration-"
            ))
    );
    let b = Connection::open(&backup).unwrap();
    let v: i64 = b
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, i64::from(CATALOG_SCHEMA_VERSION));
    let n: i64 = b
        .query_row("SELECT count(*) FROM asset", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    assert!(b.prepare("SELECT test_note FROM asset").is_err());
    cat.close().unwrap();

    // 次の版の DB は、今の版までしか知らないアプリでは開けない。
    let err = Catalog::open(&path).unwrap_err();
    assert!(
        matches!(
            err,
            CatalogError::FutureSchema { found, supported }
                if found == CATALOG_SCHEMA_VERSION + 1 && supported == CATALOG_SCHEMA_VERSION
        ),
        "{err}"
    );
}

#[test]
fn failed_migration_is_rolled_back() {
    let f = Fixture::new();
    let path = f.catalog_path();
    f.cat.close().unwrap();
    const BROKEN_NEXT: Migration = Migration {
        version: CATALOG_SCHEMA_VERSION + 1,
        description: "試験用: 途中で失敗する",
        sql: "ALTER TABLE asset ADD COLUMN x TEXT; SELECT * FROM no_such_table;",
        post: None,
    };
    let migrations: Vec<Migration> = CATALOG_MIGRATIONS
        .iter()
        .copied()
        .chain([BROKEN_NEXT])
        .collect();
    assert!(Catalog::open_with_migrations(&path, &OpenOptions::default(), &migrations).is_err());
    // 元の版のまま、列も追加されていない。開き直せる。
    let cat = Catalog::open(&path).unwrap();
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
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

/// 外部キーの子の列の組（`pragma_foreign_key_list` の 1 つの制約）ごとに、その列を先頭に持つ
/// 索引（または INTEGER PRIMARY KEY）があることを確かめる。
///
/// 子の列に索引がないと、親の行を削除するたびに子のテーブル全体を読む（SQLite の説明書
/// 「SQLite Foreign Key Support」の 3 節）。版 1 では variant.history_pos に索引がなかった。
#[test]
fn every_foreign_key_child_column_is_indexed() {
    let f = Fixture::new();
    let conn = &f.cat.conn;
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             AND sql NOT LIKE 'CREATE VIRTUAL TABLE%'",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut checked = 0;
    for table in &tables {
        // 外部キー（id ごとに、子の列を seq の順に）。
        let mut fks: std::collections::BTreeMap<i64, Vec<String>> = Default::default();
        let mut stmt = conn
            .prepare("SELECT id, seq, \"from\" FROM pragma_foreign_key_list(?1) ORDER BY id, seq")
            .unwrap();
        for row in stmt
            .query_map([table], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(2)?))
            })
            .unwrap()
        {
            let (id, col) = row.unwrap();
            fks.entry(id).or_default().push(col);
        }
        if fks.is_empty() {
            continue;
        }
        // 索引の先頭の列の並び（式の索引の列は名前がないので None）。
        let mut index_columns: Vec<Vec<Option<String>>> = Vec::new();
        let names: Vec<String> = conn
            .prepare("SELECT name FROM pragma_index_list(?1)")
            .unwrap()
            .query_map([table], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for name in names {
            let cols: Vec<Option<String>> = conn
                .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
                .unwrap()
                .query_map([&name], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            index_columns.push(cols);
        }
        // INTEGER PRIMARY KEY（rowid の別名）は索引の一覧に出ないので別に調べる。
        let rowid_alias: Option<String> = conn
            .query_row(
                "SELECT name FROM pragma_table_info(?1) WHERE pk = 1 AND upper(type) = 'INTEGER'
                 AND (SELECT count(*) FROM pragma_table_info(?1) WHERE pk > 0) = 1",
                [table],
                |r| r.get(0),
            )
            .ok();
        for cols in fks.values() {
            let covered = index_columns.iter().any(|idx| {
                idx.len() >= cols.len()
                    && idx
                        .iter()
                        .zip(cols)
                        .all(|(i, c)| i.as_deref() == Some(c.as_str()))
            }) || (cols.len() == 1
                && rowid_alias.as_deref() == Some(cols[0].as_str()));
            assert!(covered, "{table}({}) に索引がない", cols.join(", "));
            checked += 1;
        }
    }
    // file・folder・variant・history_entry・snapshot・keyword などの外部キーを調べた。
    assert!(checked >= 15, "{checked}");
}

/// 履歴や asset の削除で、variant などのテーブル全体を読まない（索引で参照を探す）。
#[test]
fn deleting_history_and_assets_does_not_scan_whole_tables() {
    use rusqlite::StatementStatus;
    let mut f = Fixture::new();
    let mut outcomes = Vec::new();
    for i in 0..50u64 {
        outcomes.push(f.photo(&format!("IMG{i:04}.ARW"), i, None));
    }
    let v = outcomes[0].master_variant_id;
    let edit = |ev: f32| genzo_model::DevelopSettings {
        exposure_ev: ev,
        ..Default::default()
    };
    f.cat.save_develop(v, &edit(0.5), "1").unwrap();
    f.cat.save_develop(v, &edit(1.0), "2").unwrap();
    let last: i64 = f
        .cat
        .conn
        .query_row("SELECT max(id) FROM history_entry", [], |r| r.get(0))
        .unwrap();

    let conn = &f.cat.conn;
    let mut stmt = conn
        .prepare("DELETE FROM history_entry WHERE id = ?1")
        .unwrap();
    assert_eq!(stmt.execute([last]).unwrap(), 1);
    assert_eq!(stmt.get_status(StatementStatus::FullscanStep), 0);

    // asset の削除（file・variant・history_entry などへの ON DELETE CASCADE を含む）。
    let mut stmt = conn.prepare("DELETE FROM asset WHERE id = ?1").unwrap();
    assert_eq!(stmt.execute([outcomes[1].asset_id.get()]).unwrap(), 1);
    assert_eq!(stmt.get_status(StatementStatus::FullscanStep), 0);
}

/// 版 1 のカタログ（索引の追加の前）を開くと、バックアップを作ってから最新の版へ移行する。
#[test]
fn version_1_catalog_is_migrated_to_the_current_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let (asset, variant) = {
        let mut cat =
            Catalog::open_with_migrations(&path, &OpenOptions::default(), &CATALOG_MIGRATIONS[..1])
                .unwrap();
        assert_eq!(cat.schema_version().unwrap(), 1);
        let vol = cat.ensure_volume("v", None, None).unwrap();
        let folder = cat.ensure_folder(vol, "a").unwrap();
        let o = cat
            .register_file(&super::photo_req(
                folder,
                "A.ARW",
                1,
                Some("2024:05:01 12:00:00"),
            ))
            .unwrap();
        cat.save_develop(
            o.master_variant_id,
            &genzo_model::DevelopSettings {
                exposure_ev: 0.7,
                ..Default::default()
            },
            "露光量",
        )
        .unwrap();
        cat.set_caption(o.asset_id, Some("京都旅行")).unwrap();
        cat.close().unwrap();
        (o.asset_id, o.master_variant_id)
    };
    let index_count = |conn: &Connection| -> i64 {
        conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'index'
             AND name IN ('variant_history_pos', 'stack_top_variant')",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(index_count(&Connection::open(&path).unwrap()), 0);

    let mut cat = Catalog::open(&path).unwrap();
    let report = cat.open_report().clone();
    assert_eq!(report.migrated_from, Some(1));
    assert_eq!(report.schema_version, CATALOG_SCHEMA_VERSION);
    assert_eq!(report.previous_shutdown, PreviousShutdown::Clean);
    assert_eq!(index_count(&cat.conn), 2);
    // データ・履歴・テキスト検索はそのまま使える。
    assert_eq!(cat.develop_settings(variant).unwrap().exposure_ev, 0.7);
    assert!(cat.undo_develop(variant).unwrap().is_some());
    let hits = cat
        .search(
            &crate::Filter {
                text: Some("京都旅行".to_owned()),
                ..Default::default()
            },
            &crate::Sort::default(),
        )
        .unwrap();
    assert_eq!(hits, vec![variant]);
    assert!(cat.check_integrity().unwrap().is_ok());
    cat.remove_assets(&[asset]).unwrap();
    assert_eq!(rows(&cat, "history_entry"), 0);
    cat.close().unwrap();

    // 移行の前のバックアップは版 1 のまま。版 1 のバックアップも復元でき、開くと移行される。
    let backup = report.migration_backup.expect("バックアップを作る");
    let restored = dir.path().join("restored.db");
    let r = crate::restore_backup(&backup, &restored).unwrap();
    assert_eq!(r.schema_version, 1);
    let cat = Catalog::open(&restored).unwrap();
    assert_eq!(cat.open_report().migrated_from, Some(1));
    assert_eq!(rows(&cat, "asset"), 1);
    cat.close().unwrap();
}

/// 同じカタログを 2 つの接続（別のスレッド）から書いても、読んでから書くトランザクションが
/// `SQLITE_BUSY`（スナップショットの競合）で途中で失敗せず、ロックを待って成功する。
#[test]
fn two_writers_wait_for_each_other_instead_of_failing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let folder = {
        let mut cat = Catalog::open(&path).unwrap();
        let vol = cat.ensure_volume("v", None, None).unwrap();
        let folder = cat.ensure_folder(vol, "a").unwrap();
        cat.close().unwrap();
        folder
    };
    // 両方の接続を開き終えてから書き始める。開く処理は印の書き込みを含むため、相手が書き続けて
    // いると、遅いディスク（Windows の CI）では busy_timeout を超えて待たされることがある
    // （このテストが確かめたいのは、読んでから書くトランザクションが待たずに失敗しないこと）。
    // 回数も、busy_timeout（5 秒）の間に相手の書き込みが終わる程度に抑える。
    const ITERATIONS: u64 = 20;
    let opened = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2u64)
        .map(|w| {
            let path = path.clone();
            let opened = std::sync::Arc::clone(&opened);
            std::thread::spawn(move || {
                let mut cat = Catalog::open(&path).unwrap();
                opened.wait();
                for i in 0..ITERATIONS {
                    let n = w * 1000 + i;
                    // 登録は既存の行を読んでから書く（冪等性のため）。
                    let o = cat
                        .register_batch(&[
                            super::photo_req(folder, &format!("W{w}_{i:03}.ARW"), n, None),
                            super::photo_req(folder, &format!("W{w}_{i:03}.JPG"), n + 500, None),
                        ])
                        .unwrap();
                    cat.set_rating(
                        &[o[0].master_variant_id],
                        genzo_model::Rating::new(3).unwrap(),
                    )
                    .unwrap();
                    cat.undo_develop(o[0].master_variant_id).unwrap();
                }
                cat.close().unwrap();
            })
        })
        .collect();
    for w in workers {
        w.join().expect("書き込みが SQLITE_BUSY で失敗しない");
    }
    let cat = Catalog::open(&path).unwrap();
    assert_eq!(rows(&cat, "asset"), 2 * ITERATIONS as i64);
    assert_eq!(rows(&cat, "file"), 4 * ITERATIONS as i64);
    assert!(cat.check_integrity().unwrap().is_ok());
}

/// 書き出しの原本の照合で、名前だけで探す問い合わせ（`files_named`）は、版 3 の `file(name_key)` の
/// 索引を使い、`file` の表を走査しない（implementation_status の No.11・27）。
#[test]
fn files_named_uses_the_name_key_index() {
    use rusqlite::StatementStatus;
    let mut f = Fixture::new();
    for i in 0..200u64 {
        f.photo(&format!("IMG{i:04}.ARW"), i, None);
    }
    let plan: Vec<String> = f
        .cat
        .conn
        .prepare("EXPLAIN QUERY PLAN SELECT id FROM file WHERE name_key IN (?1, ?2)")
        .unwrap()
        .query_map(["img0001.arw", "img0002.arw"], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(plan.iter().any(|d| d.contains("file_name_key")), "{plan:?}");
    let mut stmt = f
        .cat
        .conn
        .prepare("SELECT id FROM file WHERE name_key IN (?1, ?2)")
        .unwrap();
    let n = stmt
        .query_map(["img0001.arw", "img0002.arw"], |r| r.get::<_, i64>(0))
        .unwrap()
        .count();
    assert_eq!(n, 2);
    assert_eq!(stmt.get_status(StatementStatus::FullscanStep), 0);
    assert_eq!(f.cat.files_named(&["Img0003.arw"]).unwrap().len(), 1);
}

/// 版 3 のカタログ（`tz_source` の CHECK 制約に `utc_only` がない）を開くと、asset の表を作り直して
/// `utc_only` を使えるようにし、UTC の印（Z）だけの撮影日時を `utc_only` にする（No.32）。
/// UTC・利用者の指定・ほかの列・外部キー・索引・AUTOINCREMENT の連番は保つ。
#[test]
fn version_3_catalog_is_migrated_to_utc_only() {
    use chrono::{FixedOffset, TimeZone, Utc};
    use genzo_model::{AssetId, CaptureTime, TzSource};

    let jst = FixedOffset::east_opt(9 * 3600).unwrap();
    // 版 3 までの取り込みが記録した形（Z を +00:00 のオフセットとして記録した）。
    let legacy = |tz_source: TzSource, assumed: &str, utc_hour: u32| CaptureTime {
        raw: Some("2024-05-01T03:00:00.000000Z".to_owned()),
        offset: Some("+00:00".to_owned()),
        tz_source,
        tz_assumed: Some(assumed.to_owned()),
        basis: None,
        correction_s: 0,
        utc: Some(Utc.with_ymd_and_hms(2024, 5, 1, utc_hour, 0, 0).unwrap()),
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let (old_video, set_video, photo, plus_zero, deleted) = {
        let mut cat =
            Catalog::open_with_migrations(&path, &OpenOptions::default(), &CATALOG_MIGRATIONS[..3])
                .unwrap();
        assert_eq!(cat.schema_version().unwrap(), 3);
        let vol = cat.ensure_volume("v", None, None).unwrap();
        let folder = cat.ensure_folder(vol, "a").unwrap();
        let mut video = super::video_req(folder, "OLD.MP4", 1, None);
        video.capture = legacy(TzSource::Exif, "+00:00", 3);
        let old_video = cat.register_file(&video).unwrap();
        cat.set_caption(old_video.asset_id, Some("京都旅行"))
            .unwrap();
        // 利用者がタイムゾーンを指定したもの（版 3 の計算: 文字列の時刻を +09:00 の現地時刻とみなした）。
        let mut video = super::video_req(folder, "SET.MP4", 2, None);
        video.capture = CaptureTime {
            utc: Some(Utc.with_ymd_and_hms(2024, 4, 30, 18, 0, 0).unwrap()),
            ..legacy(TzSource::UserSet, "+09:00", 0)
        };
        let set_video = cat.register_file(&video).unwrap();
        let photo = cat
            .register_file(&super::photo_req(
                folder,
                "A.ARW",
                3,
                Some("2024:05:01 12:00:00"),
            ))
            .unwrap();
        // 別に +00:00 が記録された写真（Z ではない）。
        let mut req = super::photo_req(folder, "B.ARW", 4, None);
        req.capture =
            CaptureTime::resolve(Some("2024:05:01 12:00:00"), Some("+00:00"), jst).unwrap();
        let plus_zero = cat.register_file(&req).unwrap();
        // 最後の id の asset を除く（id を再利用しないことの確認用）。
        let del = cat
            .register_file(&super::photo_req(folder, "DEL.ARW", 5, None))
            .unwrap();
        cat.remove_assets(&[del.asset_id]).unwrap();
        // 版 3 の CHECK 制約は utc_only を拒む。
        assert!(
            cat.conn
                .execute(
                    "UPDATE asset SET tz_source = 'utc_only' WHERE id = ?1",
                    [old_video.asset_id.get()],
                )
                .is_err()
        );
        cat.close().unwrap();
        (
            old_video.asset_id,
            set_video.asset_id,
            photo.asset_id,
            plus_zero.asset_id,
            del.asset_id,
        )
    };

    let mut cat = Catalog::open(&path).unwrap();
    let report = cat.open_report().clone();
    assert_eq!(report.migrated_from, Some(3));
    assert_eq!(report.schema_version, CATALOG_SCHEMA_VERSION);
    assert!(report.migration_backup.is_some());
    assert!(report.migrated_from.unwrap() < crate::UTC_ONLY_SCHEMA_VERSION);
    // utc_only にした asset があるので、推定し直しが済んでいない印が（移行と同じトランザクションで）
    // 付いている。外すと消える。
    assert!(cat.capture_time_reresolve_pending().unwrap());
    assert_eq!(
        cat.app_state(crate::STATE_CAPTURE_TIME_RERESOLVE)
            .unwrap()
            .as_deref(),
        Some("pending")
    );

    // Z だけの動画は utc_only（UTC は同じ。表示のオフセットは API が既定のタイムゾーンにする）。
    let t = cat.capture_time(old_video).unwrap();
    assert_eq!(t.tz_source, TzSource::UtcOnly);
    assert_eq!(t.offset.as_deref(), Some(genzo_model::UTC_ONLY_OFFSET));
    assert_eq!(t.tz_assumed.as_deref(), Some("+00:00"));
    assert_eq!(
        t.utc_db_string().as_deref(),
        Some("2024-05-01T03:00:00.000Z")
    );
    let shown = t.with_default_offset(jst).unwrap();
    assert_eq!(shown.utc, t.utc);
    assert_eq!(shown.to_string(), "2024-05-01 12:00:00 +09:00");
    cat.set_capture_time(old_video, &shown).unwrap();
    assert_eq!(cat.capture_time(old_video).unwrap(), shown);
    // 利用者の指定・オフセットが記録された写真は変えない。
    let s = cat.capture_time(set_video).unwrap();
    assert_eq!(s.tz_source, TzSource::UserSet);
    assert_eq!(s.offset.as_deref(), Some("+00:00"));
    assert_eq!(
        s.utc_db_string().as_deref(),
        Some("2024-04-30T18:00:00.000Z")
    );
    assert_eq!(cat.capture_time(photo).unwrap().tz_source, TzSource::Exif);
    let p = cat.capture_time(plus_zero).unwrap();
    assert_eq!(
        (p.tz_source, p.offset.as_deref()),
        (TzSource::Exif, Some("+00:00"))
    );
    // ほかの列（動画の情報・キャプション・寸法）。
    let a = cat.asset(old_video).unwrap();
    assert_eq!(a.caption.as_deref(), Some("京都旅行"));
    assert_eq!(a.video.as_ref().and_then(|v| v.fps), Some(59.94));

    // 新しい CHECK 制約: utc_only は受け付け、それ以外の値は拒む。
    assert!(
        cat.conn
            .execute(
                "UPDATE asset SET tz_source = 'bogus' WHERE id = ?1",
                [photo.get()]
            )
            .is_err()
    );
    // 索引（撮影日時順・カメラ・レンズ）と名前の索引がある。
    let n: i64 = cat
        .conn
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'index' AND name IN
             ('asset_captured_at', 'asset_camera', 'asset_lens', 'file_name_key')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 4);
    // 除いた asset の id を再利用しない。
    let vol = cat.ensure_volume("v", None, None).unwrap();
    let folder = cat.ensure_folder(vol, "a").unwrap();
    let next = cat
        .register_file(&super::photo_req(folder, "NEW.ARW", 6, None))
        .unwrap();
    assert!(
        next.asset_id > deleted,
        "{:?} > {:?}",
        next.asset_id,
        deleted
    );
    // 新しく取り込む Z だけの値は utc_only で保存できる。
    let mut video = super::video_req(folder, "NEW.MP4", 7, None);
    video.capture = CaptureTime::resolve(Some("2024-05-02T00:00:00Z"), None, jst).unwrap();
    let new_video = cat.register_file(&video).unwrap();
    assert_eq!(
        cat.capture_time(new_video.asset_id).unwrap().tz_source,
        TzSource::UtcOnly
    );
    // テキスト検索・撮影日時順の検索・外部キー（asset の削除で file・variant・video_meta も消える）。
    let hits = cat
        .search(
            &crate::Filter {
                text: Some("京都旅行".to_owned()),
                ..Default::default()
            },
            &crate::Sort::default(),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(cat.check_integrity().unwrap().is_ok());
    let fk: i64 = cat
        .conn
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(fk, 0);
    cat.remove_assets(&[old_video]).unwrap();
    for table in ["video_meta", "asset_text"] {
        let n: i64 = cat
            .conn
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE asset_id = ?1"),
                [old_video.get()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "{table}");
    }
    let _: AssetId = old_video;
    cat.set_capture_time_reresolve_pending(false).unwrap();
    assert!(!cat.capture_time_reresolve_pending().unwrap());
    cat.close().unwrap();
}

/// 新しいカタログ・utc_only にする asset がないカタログの移行では、撮影日時の推定し直しの印を付けない。
#[test]
fn migration_without_utc_only_assets_does_not_request_reresolve() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = Catalog::open(dir.path().join("fresh.db")).unwrap();
    assert!(!fresh.capture_time_reresolve_pending().unwrap());
    fresh.close().unwrap();

    let path = dir.path().join("v3.db");
    {
        let mut cat =
            Catalog::open_with_migrations(&path, &OpenOptions::default(), &CATALOG_MIGRATIONS[..3])
                .unwrap();
        let vol = cat.ensure_volume("v", None, None).unwrap();
        let folder = cat.ensure_folder(vol, "a").unwrap();
        cat.register_file(&super::photo_req(
            folder,
            "A.ARW",
            1,
            Some("2024:05:01 12:00:00"),
        ))
        .unwrap();
        cat.close().unwrap();
    }
    let mut cat = Catalog::open(&path).unwrap();
    assert_eq!(cat.open_report().migrated_from, Some(3));
    assert!(!cat.capture_time_reresolve_pending().unwrap());
    // 印を付ける・外す（genzo-api が既定のタイムゾーンを変えたとき・推定し直しを終えたとき）。
    cat.set_capture_time_reresolve_pending(true).unwrap();
    assert!(cat.capture_time_reresolve_pending().unwrap());
    cat.set_capture_time_reresolve_pending(true).unwrap();
    cat.set_capture_time_reresolve_pending(false).unwrap();
    assert!(!cat.capture_time_reresolve_pending().unwrap());
    cat.close().unwrap();
}
