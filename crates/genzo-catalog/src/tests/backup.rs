//! バックアップ・世代の管理・復元・整合性チェックのテスト（SYS-04、DATA-04、DATA-05）。

use rusqlite::Connection;

use super::Fixture;
use crate::{
    Catalog, CatalogError, DEFAULT_BACKUP_GENERATIONS, Filter, PreviousShutdown, Sort,
    backup_catalog_file, create_backup, list_backups, prune_backups, restore_backup,
};

#[test]
fn online_backup_and_restore_to_a_new_file() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, Some("2024:05:01 12:00:00"));
    f.cat
        .save_develop(
            a.master_variant_id,
            &genzo_model::DevelopSettings {
                exposure_ev: 0.5,
                ..Default::default()
            },
            "露光量",
        )
        .unwrap();
    let backup = f.dir.path().join("manual-backup.db");
    // 開いたまま（書き込み用の接続を保ったまま）バックアップできる。
    f.cat.backup_to(&backup).unwrap();
    // 既にあるファイルは上書きしない。
    assert!(matches!(
        f.cat.backup_to(&backup),
        Err(CatalogError::AlreadyExists(_))
    ));
    // バックアップの後の変更は、バックアップには含まれない。
    f.photo("B.ARW", 2, None);

    let restored = f.dir.path().join("restored.db");
    let backup_bytes = std::fs::read(&backup).unwrap();
    let report = restore_backup(&backup, &restored).unwrap();
    assert_eq!(report.schema_version, crate::CATALOG_SCHEMA_VERSION);
    // バックアップのファイルは変更しない（開いている間に作ったので「使用中」の印のまま）。
    assert_eq!(std::fs::read(&backup).unwrap(), backup_bytes);
    let in_backup: String = Connection::open(&backup)
        .unwrap()
        .query_row(
            "SELECT value FROM app_state WHERE key = 'shutdown'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(in_backup, "in_use");
    let cat = Catalog::open(&restored).unwrap();
    // 検証を通った復元先は「正常に終了した」状態にするので、最初に開いたときに
    // 「前回は正常に終了しなかった」と報告しない（DATA-05 の印を誤らせない）。
    assert_eq!(cat.open_report().previous_shutdown, PreviousShutdown::Clean);
    let ids = cat.search(&Filter::default(), &Sort::default()).unwrap();
    assert_eq!(ids, vec![a.master_variant_id]);
    assert_eq!(
        cat.develop_settings(a.master_variant_id)
            .unwrap()
            .exposure_ev,
        0.5
    );
    assert!(cat.check_integrity().unwrap().is_ok());
    // テキスト検索の索引も復元されている。
    let hits = cat
        .search(
            &Filter {
                text: Some("a.arw".to_owned()),
                ..Default::default()
            },
            &Sort::default(),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    cat.close().unwrap();
    // 復元先が既にあれば上書きしない。
    assert!(matches!(
        restore_backup(&backup, &restored),
        Err(CatalogError::AlreadyExists(_))
    ));
    // 一時ファイルが残っていない。
    let leftovers: Vec<_> = std::fs::read_dir(f.dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn backup_from_a_separate_read_only_connection() {
    let mut f = Fixture::new();
    f.photo("A.ARW", 1, None);
    let dest = f.dir.path().join("b.db");
    backup_catalog_file(&f.catalog_path(), &dest).unwrap();
    let conn = Connection::open(&dest).unwrap();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM asset", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    assert!(matches!(
        backup_catalog_file(&f.dir.path().join("none.db"), &f.dir.path().join("c.db")),
        Err(CatalogError::NotFound(_))
    ));
}

#[test]
fn generations_are_pruned_keeping_the_newest() {
    let mut f = Fixture::new();
    f.photo("A.ARW", 1, None);
    let dir = f.dir.path().join("backups");
    std::fs::create_dir_all(&dir).unwrap();
    // 他のファイル・他のカタログのバックアップは消さない。
    std::fs::write(dir.join("notes.txt"), b"keep").unwrap();
    std::fs::write(dir.join("other-backup-20000101T000000000Z.db"), b"keep").unwrap();
    let mut made = Vec::new();
    for i in 0..7 {
        if i == 3 {
            f.photo("B.ARW", 2, None);
        }
        made.push(
            f.cat
                .create_backup(&dir, DEFAULT_BACKUP_GENERATIONS)
                .unwrap(),
        );
    }
    let list = list_backups(&dir, "catalog").unwrap();
    assert_eq!(list.len(), DEFAULT_BACKUP_GENERATIONS);
    // 新しい順に、最後に作った 5 つが残る。
    let expected: Vec<_> = made.iter().rev().take(5).map(|b| b.path.clone()).collect();
    assert_eq!(
        list.iter().map(|b| b.path.clone()).collect::<Vec<_>>(),
        expected
    );
    assert!(!made[0].path.exists());
    assert!(!made[1].path.exists());
    assert!(dir.join("notes.txt").exists());
    assert!(dir.join("other-backup-20000101T000000000Z.db").exists());
    for w in list.windows(2) {
        assert!(w[0].created_at >= w[1].created_at);
    }
    // 最新のバックアップには B が含まれる。
    let restored = f.dir.path().join("r.db");
    restore_backup(&list[0].path, &restored).unwrap();
    assert_eq!(super::rows(&Catalog::open(&restored).unwrap(), "asset"), 2);

    // 世代数を減らす。
    let removed = prune_backups(&dir, "catalog", 2).unwrap();
    assert_eq!(removed.len(), 3);
    assert_eq!(list_backups(&dir, "catalog").unwrap().len(), 2);
    assert!(f.cat.create_backup(&dir, 0).is_err());
    // ファイルから（別の接続で）作る。
    let info = create_backup(&f.catalog_path(), &dir, 2).unwrap();
    assert!(info.path.exists());
    assert_eq!(list_backups(&dir, "catalog").unwrap()[0].path, info.path);
    assert!(
        list_backups(&f.dir.path().join("none"), "catalog")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn corrupted_backup_is_not_restored() {
    let mut f = Fixture::new();
    for i in 0..200 {
        f.photo(&format!("IMG{i:04}.ARW"), i, Some("2024:05:01 12:00:00"));
    }
    let backup = f.dir.path().join("b.db");
    f.cat.backup_to(&backup).unwrap();
    // ページの中身を壊す（先頭のヘッダーは残す）。
    let mut bytes = std::fs::read(&backup).unwrap();
    let page = 4096;
    assert!(bytes.len() > page * 4);
    for b in &mut bytes[page * 2..page * 4] {
        *b = 0x5A;
    }
    std::fs::write(&backup, &bytes).unwrap();
    let dest = f.dir.path().join("restored.db");
    let err = restore_backup(&backup, &dest).unwrap_err();
    assert!(
        matches!(
            err,
            CatalogError::IntegrityCheckFailed(_) | CatalogError::Sqlite(_)
        ),
        "{err}"
    );
    assert!(!dest.exists());
    // カタログ以外のファイルは復元しない。
    let other = f.dir.path().join("other.db");
    Connection::open(&other)
        .unwrap()
        .execute_batch("CREATE TABLE t(x);")
        .unwrap();
    assert!(matches!(
        restore_backup(&other, &f.dir.path().join("x.db")),
        Err(CatalogError::NotOurDatabase(_))
    ));
    let empty = f.dir.path().join("empty.db");
    Connection::open(&empty)
        .unwrap()
        .execute_batch("PRAGMA user_version = 0;")
        .unwrap();
    assert!(matches!(
        restore_backup(&empty, &f.dir.path().join("y.db")),
        Err(CatalogError::NotOurDatabase(_))
    ));
}

#[test]
fn integrity_check_detects_problems() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    assert!(f.cat.check_integrity().unwrap().is_ok());
    // 外部キーの確認を止めて、参照先のない行を作る。
    f.cat
        .conn
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    f.cat
        .conn
        .execute("DELETE FROM asset WHERE id = ?1", [a.asset_id.get()])
        .unwrap();
    f.cat
        .conn
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    let report = f.cat.check_integrity().unwrap();
    assert!(!report.is_ok());
    let tables: Vec<_> = report
        .foreign_key_violations
        .iter()
        .map(|v| v.table.as_str())
        .collect();
    assert!(tables.contains(&"file"), "{tables:?}");
    assert!(tables.contains(&"variant"), "{tables:?}");
    assert!(
        report
            .foreign_key_violations
            .iter()
            .all(|v| v.parent == "asset")
    );

    // FTS の索引と内容の不一致（索引を通さずに内容を変える）。
    let mut g = Fixture::new();
    g.photo("A.ARW", 1, None);
    g.cat
        .conn
        .execute_batch(
            "DROP TRIGGER asset_text_after_update;
             UPDATE asset_text SET text_norm = 'changed';",
        )
        .unwrap();
    let report = g.cat.check_integrity().unwrap();
    assert!(report.fts_error.is_some(), "{report:?}");
    g.cat.rebuild_text_index().unwrap();
    assert!(g.cat.check_integrity().unwrap().is_ok());
}
