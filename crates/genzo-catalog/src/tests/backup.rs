//! バックアップ・世代の管理・復元・整合性チェックのテスト（SYS-04、DATA-04、DATA-05）。

use rusqlite::Connection;

use super::Fixture;
use crate::{
    BackupOwner, Catalog, CatalogError, DEFAULT_BACKUP_GENERATIONS, Filter, PreviousShutdown, Sort,
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
    let owner = f.cat.backup_owner().unwrap();
    let list = list_backups(&dir, &owner).unwrap();
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
    let removed = prune_backups(&dir, &owner, 2).unwrap();
    assert_eq!(removed.len(), 3);
    assert_eq!(list_backups(&dir, &owner).unwrap().len(), 2);
    assert!(f.cat.create_backup(&dir, 0).is_err());
    // ファイルから（別の接続で）作る。
    let info = create_backup(&f.catalog_path(), &dir, 2).unwrap();
    assert!(info.path.exists());
    assert_eq!(list_backups(&dir, &owner).unwrap()[0].path, info.path);
    assert!(
        list_backups(&f.dir.path().join("none"), &owner)
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
    // FTS の索引だけの確認（書き込み用の接続で、DB 全体の走査をせずに行う。コア API の詳細チェックが
    // カタログのロックを長く持たないため。指摘 F25）。
    assert_eq!(g.cat.check_text_index().unwrap(), report.fts_error);
    g.cat.rebuild_text_index().unwrap();
    assert!(g.cat.check_integrity().unwrap().is_ok());
    assert_eq!(g.cat.check_text_index().unwrap(), None);
}

/// キャッシュの世代は、同じカタログでは変わらず、バックアップから復元したカタログでは新しくなる
/// （復元の後に再利用される ID で、前の写真の外部キャッシュを使わないため）。
#[test]
fn cache_generation_changes_on_restore() {
    let mut f = Fixture::new();
    let g = f.cat.cache_generation().unwrap();
    assert_eq!(g.len(), 32);
    assert_eq!(f.cat.cache_generation().unwrap(), g, "同じカタログでは同じ");
    let backup = f.dir.path().join("bk.db");
    f.cat.backup_to(&backup).unwrap();
    let restored = f.dir.path().join("restored.db");
    restore_backup(&backup, &restored).unwrap();
    let mut cat = Catalog::open(&restored).unwrap();
    let r = cat.cache_generation().unwrap();
    assert_ne!(r, g, "復元したカタログは新しい世代");
    assert_eq!(cat.cache_generation().unwrap(), r);
    // 新しいカタログも別の世代。
    let mut other = Catalog::open(f.dir.path().join("other.db")).unwrap();
    assert_ne!(other.cache_generation().unwrap(), g);
}

/// 時計が既存のバックアップより前に戻っていても（電池切れの PC・時計の進んだ別の機器で作ったバックアップ）、
/// 今作ったバックアップは世代の管理で消さない。返したファイルは必ずある（F09）。
#[test]
fn a_new_backup_is_kept_even_if_existing_ones_are_dated_in_the_future() {
    let mut f = Fixture::new();
    f.photo("A.ARW", 1, None);
    let dir = f.dir.path().join("backups");
    std::fs::create_dir_all(&dir).unwrap();
    let generations = DEFAULT_BACKUP_GENERATIONS;
    let mut future = Vec::new();
    for day in 1..=generations {
        let p = dir.join(future_backup_name(&f.cat, day));
        f.cat.backup_to(&p).unwrap();
        future.push(p);
    }
    let owner = f.cat.backup_owner().unwrap();
    let first = f.cat.create_backup(&dir, generations).unwrap();
    assert!(first.path.is_file(), "今作ったバックアップが残っている");
    let list = list_backups(&dir, &owner).unwrap();
    assert_eq!(list.len(), generations);
    assert!(list.iter().any(|b| b.path == first.path));
    // もう一度作ると、前回の「今」のものと入れ替わる（今の状態のバックアップが常に 1 つ残る）。
    f.photo("B.ARW", 2, None);
    let second = create_backup(&f.catalog_path(), &dir, generations).unwrap();
    assert!(second.path.is_file());
    assert!(!first.path.exists());
    let list = list_backups(&dir, &owner).unwrap();
    assert_eq!(list.len(), generations);
    // 未来の日時のもの（世代数 - 1 個）と、最新の 1 個。
    assert_eq!(
        list.iter().filter(|b| future.contains(&b.path)).count(),
        generations - 1
    );
    let restored = f.dir.path().join("r.db");
    restore_backup(&second.path, &restored).unwrap();
    assert_eq!(super::rows(&Catalog::open(&restored).unwrap(), "asset"), 2);
}

/// 未来の日時（2099 年 11 月 `day` 日）の、このカタログのバックアップのファイル名。
fn future_backup_name(cat: &Catalog, day: usize) -> String {
    let owner = cat.backup_owner().unwrap();
    let day = u32::try_from(day).unwrap();
    let t = chrono::NaiveDate::from_ymd_opt(2099, 11, day)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    crate::backup::backup_file_name(&owner, t, 0)
}

/// 同じファイル名のカタログ（別々のフォルダの catalog.db）が同じ保存先を使っても、互いのバックアップを
/// 消さず、一覧にも出さない（F10）。古い形式の名前（カタログの印なし）のものは一覧に出すが、世代の管理では
/// 消さない（どのカタログのものか分からないため）。
#[test]
fn catalogs_with_the_same_name_do_not_touch_each_others_backups() {
    let tmp = tempfile::tempdir().unwrap();
    let shared = tmp.path().join("shared");
    for d in ["A", "B"] {
        std::fs::create_dir_all(tmp.path().join(d)).unwrap();
    }
    let a = Catalog::open(tmp.path().join("A").join("catalog.db")).unwrap();
    let b = Catalog::open(tmp.path().join("B").join("catalog.db")).unwrap();
    let (oa, ob) = (a.backup_owner().unwrap(), b.backup_owner().unwrap());
    assert_eq!(oa.stem, ob.stem);
    assert_ne!(oa, ob);
    let a1 = a
        .create_backup(&shared, DEFAULT_BACKUP_GENERATIONS)
        .unwrap();
    for _ in 0..DEFAULT_BACKUP_GENERATIONS {
        b.create_backup(&shared, DEFAULT_BACKUP_GENERATIONS)
            .unwrap();
    }
    assert!(
        a1.path.is_file(),
        "B の世代の管理で A のバックアップを消さない"
    );
    let la = list_backups(&shared, &oa).unwrap();
    assert_eq!(la.len(), 1);
    assert_eq!(la[0].path, a1.path);
    assert_eq!(la[0].catalog_tag, oa.tag);
    let lb = list_backups(&shared, &ob).unwrap();
    assert_eq!(lb.len(), DEFAULT_BACKUP_GENERATIONS);
    assert!(lb.iter().all(|x| x.catalog_tag == ob.tag));

    let legacy = shared.join("catalog-backup-20000101T000000000Z.db");
    a.backup_to(&legacy).unwrap();
    b.create_backup(&shared, DEFAULT_BACKUP_GENERATIONS)
        .unwrap();
    assert!(legacy.is_file());
    let lb = list_backups(&shared, &ob).unwrap();
    assert_eq!(lb.len(), DEFAULT_BACKUP_GENERATIONS + 1);
    assert!(
        lb.iter()
            .any(|x| x.path == legacy && x.catalog_tag.is_none())
    );
    assert_eq!(prune_backups(&shared, &ob, 1).unwrap().len(), 4);
    assert!(legacy.is_file());
    assert!(a1.path.is_file());
    assert_eq!(list_backups(&shared, &ob).unwrap().len(), 2);
    assert_eq!(list_backups(&shared, &oa).unwrap().len(), 2);
}

/// カタログの ID は、開き直しても・バックアップから復元しても変わらない（復元したカタログからも、復元の
/// 前のバックアップを一覧できる）。印のないカタログ（読み取り専用で開いた古いカタログ）は古い形式の名前を
/// 使う（F10）。
#[test]
fn the_catalog_id_is_kept_by_backup_and_restore() {
    let f = Fixture::new();
    let owner = f.cat.backup_owner().unwrap();
    assert_eq!(owner.stem, "catalog");
    let tag = owner
        .tag
        .clone()
        .expect("書き込み用に開いたカタログには ID がある");
    assert_eq!(tag.len(), 16);
    let dir = f.dir.path().join("backups");
    let b = f.cat.create_backup(&dir, 5).unwrap();
    let name = b.path.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(
        name.starts_with(&format!("catalog-backup-{tag}-")),
        "{name}"
    );
    let sub = f.dir.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let restored = sub.join("catalog.db");
    restore_backup(&b.path, &restored).unwrap();
    let r = Catalog::open(&restored).unwrap();
    assert_eq!(r.backup_owner().unwrap(), owner);
    assert_eq!(
        list_backups(&dir, &r.backup_owner().unwrap()).unwrap()[0].path,
        b.path
    );
    r.close().unwrap();
    let path = f.catalog_path();
    f.cat.close().unwrap();
    assert_eq!(Catalog::open(&path).unwrap().backup_owner().unwrap(), owner);
    assert_eq!(
        Catalog::open_read_only(&path)
            .unwrap()
            .backup_owner()
            .unwrap(),
        owner
    );

    // ID のないカタログ（この修正の前に作ったもの）は、古い形式の名前で作り、古い形式のものを管理する。
    let old = f.dir.path().join("old.db");
    Catalog::open(&old).unwrap().close().unwrap();
    Connection::open(&old)
        .unwrap()
        .execute("DELETE FROM app_state WHERE key = 'catalog_id'", [])
        .unwrap();
    let legacy_owner = BackupOwner {
        stem: "old".to_owned(),
        tag: None,
    };
    let info = create_backup(&old, &dir, 1).unwrap();
    let name = info.path.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(name.starts_with("old-backup-2"), "{name}");
    assert_eq!(list_backups(&dir, &legacy_owner).unwrap().len(), 1);
}
