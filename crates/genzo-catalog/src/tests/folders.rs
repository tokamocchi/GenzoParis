//! ボリューム（FILE-03）・フォルダの移動と名前の変更・場所の指定し直し（FILE-02・FILE-04）・見つからない印・
//! 再リンクのテスト。

use genzo_model::{FileOpKind, FileOpState, FileStatus, FolderStatus, VolumeId};

use super::{Fixture, facts, hex, photo_req};
use crate::{
    AppliedEntry, Catalog, CatalogError, FileFacts, FileOpPayload, FolderRebind, MoveMethod,
    MoveRequest, VOLUMES_SCHEMA_VERSION,
};

/// 版 11 のカタログを開くと版 12 に移行し、ボリュームの 2 つの列（NULL）・フォルダの状態（ok）・索引ができる。
/// ボリュームの ID は変えない。
#[test]
fn version_11_catalog_migrates_to_volumes_and_folder_status() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let photo = {
        let mut cat = crate::testing::open_at_version(&path, VOLUMES_SCHEMA_VERSION - 1).unwrap();
        let vol = cat.ensure_volume("path:/", None, Some("/")).unwrap();
        let folder = cat.ensure_folder(vol, "home/me/写真").unwrap();
        let photo = cat
            .register_file(&photo_req(folder, "A.ARW", 1, None))
            .unwrap();
        assert!(cat.conn.prepare("SELECT fs_type FROM volume").is_err());
        assert!(cat.conn.prepare("SELECT status FROM folder").is_err());
        cat.close().unwrap();
        photo
    };
    let cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().migrated_from,
        Some(VOLUMES_SCHEMA_VERSION - 1)
    );
    let vols = cat.volume_records().unwrap();
    assert_eq!(vols.len(), 1);
    assert_eq!(vols[0].uuid, "path:/");
    assert_eq!(vols[0].last_mount_path.as_deref(), Some("/"));
    assert_eq!(vols[0].fs_type, None);
    assert_eq!(vols[0].last_seen_at, None);
    let folders = cat.folder_records().unwrap();
    assert!(folders.iter().all(|f| f.status == FolderStatus::Ok));
    assert_eq!(folders.len(), 4, "ルート・home・home/me・home/me/写真");
    assert_eq!(
        cat.file_location(photo.file_id)
            .unwrap()
            .absolute_path()
            .unwrap(),
        std::path::Path::new("/home/me/写真/A.ARW")
    );
    // 見つからないファイルの一覧は、部分索引で探す（ファイルの表を全部は読まない）。
    let plan: String = cat
        .conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT id FROM file WHERE status <> 'ok' AND status = 'missing'",
            [],
            |r| r.get(3),
        )
        .unwrap();
    assert!(plan.contains("file_not_ok"), "{plan}");
    cat.close().unwrap();
}

/// つながっていないボリュームのファイル・フォルダの場所は、マウント先が分からないものとして返す（記録した
/// マウント先に別のドライブがつながっていても、そのパスを使わない）。
#[test]
fn offline_volumes_have_no_mount_path() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    assert!(f.cat.file_location(a.file_id).unwrap().mount_path.is_some());
    f.cat.set_offline_volumes([f.volume]);
    assert!(f.cat.is_volume_offline(f.volume));
    let loc = f.cat.file_location(a.file_id).unwrap();
    assert_eq!(loc.mount_path, None);
    assert_eq!(loc.absolute_path(), None);
    assert_eq!(f.cat.folder_location(f.folder).unwrap().mount_path, None);
    let plan = f.cat.trash_plan(&[a.asset_id]).unwrap();
    assert_eq!(plan.files[0].location.absolute_path(), None);
    // ボリュームの記録（最後のマウント先）は変えない。
    assert_eq!(
        f.cat.volume_records().unwrap()[0]
            .last_mount_path
            .as_deref(),
        Some("/mnt/photos")
    );
    f.cat.set_offline_volumes([]);
    assert!(f.cat.file_location(a.file_id).unwrap().mount_path.is_some());
}

#[test]
fn volumes_record_when_they_were_seen() {
    let mut f = Fixture::new();
    f.cat
        .note_volume_seen(f.volume, "/media/me/SD", Some("SD"), Some("exfat"))
        .unwrap();
    let v = &f.cat.volume_records().unwrap()[0];
    assert_eq!(v.last_mount_path.as_deref(), Some("/media/me/SD"));
    assert_eq!(v.label.as_deref(), Some("SD"));
    assert_eq!(v.fs_type.as_deref(), Some("exfat"));
    assert!(v.last_seen_at.is_some());
    // ラベル・種類が分からなければ記録のまま。
    f.cat
        .note_volume_seen(f.volume, "/media/me/SD1", None, None)
        .unwrap();
    let v = &f.cat.volume_records().unwrap()[0];
    assert_eq!(v.last_mount_path.as_deref(), Some("/media/me/SD1"));
    assert_eq!(v.label.as_deref(), Some("SD"));
    assert!(
        f.cat
            .note_volume_seen(VolumeId::new(99), "/x", None, None)
            .is_err()
    );
}

/// フォルダの作成（カタログ）: 同じ名前（比較キー）があればエラー。名前の規則。
#[test]
fn folders_can_be_created_under_a_parent() {
    let mut f = Fixture::new();
    let id = f.cat.create_folder(f.folder, "選別済み").unwrap();
    let folder = f.cat.folder(id).unwrap();
    assert_eq!(folder.rel_path, "2024/京都/選別済み");
    assert_eq!(folder.parent_id, Some(f.folder));
    assert!(matches!(
        f.cat.create_folder(f.folder, "選別済み"),
        Err(CatalogError::Conflict(_))
    ));
    for bad in ["", ".", "..", "a/b"] {
        assert!(f.cat.create_folder(f.folder, bad).is_err(), "{bad:?}");
    }
    // ボリュームのルートの下にも作れる。
    let root = f.cat.find_folder(f.volume, "").unwrap().unwrap();
    let top = f.cat.create_folder(root, "新しい").unwrap();
    assert_eq!(f.cat.folder(top).unwrap().rel_path, "新しい");
}

/// フォルダの名前の変更・移動の予定と done: 配下のフォルダの相対パスも付け替わり、ファイルは同じフォルダの ID の
/// まま新しいパスになる。予定の前に衝突・自分の中への移動・ルートを断る。
#[test]
fn folder_rename_and_move_rebase_the_subtree_on_done() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let sub = f.cat.ensure_folder(f.volume, "2024/京都/嵐山").unwrap();
    let b = f
        .cat
        .register_file(&photo_req(sub, "B.ARW", 2, None))
        .unwrap();
    let other = f.cat.ensure_folder(f.volume, "2024/奈良").unwrap();

    // 名前の変更（2024/京都 → 2024/京都旅行）。
    let op = f
        .cat
        .plan_folder_op(FileOpKind::Rename, f.folder, "2024/京都旅行")
        .unwrap();
    let rec = f.cat.file_op(op).unwrap();
    let folder = rec.payload.folder.as_ref().unwrap();
    assert_eq!(folder.from_rel, "2024/京都");
    assert_eq!(folder.to_rel, "2024/京都旅行");
    assert!(rec.payload.entries.is_empty());
    // done の前は元のまま。
    assert_eq!(f.cat.folder(sub).unwrap().rel_path, "2024/京都/嵐山");
    f.cat.start_file_op(op).unwrap();
    f.cat.complete_file_op(op).unwrap();
    assert_eq!(f.cat.folder(f.folder).unwrap().rel_path, "2024/京都旅行");
    assert_eq!(f.cat.folder(sub).unwrap().rel_path, "2024/京都旅行/嵐山");
    assert_eq!(
        f.cat.file_location(b.file_id).unwrap().rel_path(),
        "2024/京都旅行/嵐山/B.ARW"
    );
    assert_eq!(
        f.cat.file_location(a.file_id).unwrap().rel_path(),
        "2024/京都旅行/A.ARW"
    );
    // 新しいパスで探せ、古いパスはもうない。
    assert_eq!(
        f.cat.find_folder(f.volume, "2024/京都旅行/嵐山").unwrap(),
        Some(sub)
    );
    assert_eq!(f.cat.find_folder(f.volume, "2024/京都").unwrap(), None);

    // 移動（2024/京都旅行/嵐山 → 2024/奈良/嵐山）。親は移動先のフォルダになる。
    let op = f
        .cat
        .plan_folder_op(FileOpKind::Move, sub, "2024/奈良/嵐山")
        .unwrap();
    f.cat.complete_file_op(op).unwrap();
    let moved = f.cat.folder(sub).unwrap();
    assert_eq!(moved.rel_path, "2024/奈良/嵐山");
    assert_eq!(moved.parent_id, Some(other));

    // 大文字・小文字だけの名前の変更（自分と同じキー）はできる。
    let op = f
        .cat
        .plan_folder_op(FileOpKind::Rename, other, "2024/NARA")
        .unwrap();
    f.cat.complete_file_op(op).unwrap();
    let op = f
        .cat
        .plan_folder_op(FileOpKind::Rename, other, "2024/nara")
        .unwrap();
    f.cat.complete_file_op(op).unwrap();
    assert_eq!(f.cat.folder(other).unwrap().rel_path, "2024/nara");
    assert_eq!(f.cat.folder(sub).unwrap().rel_path, "2024/nara/嵐山");

    // 衝突・自分の中・ルート・不正な名前は、記録せずに断る。
    let before = f.rows("file_op");
    assert!(matches!(
        f.cat
            .plan_folder_op(FileOpKind::Rename, f.folder, "2024/nara"),
        Err(CatalogError::Conflict(_))
    ));
    assert!(matches!(
        f.cat
            .plan_folder_op(FileOpKind::Move, other, "2024/nara/嵐山/中"),
        Err(CatalogError::InvalidInput(_))
    ));
    let root = f.cat.find_folder(f.volume, "").unwrap().unwrap();
    assert!(f.cat.plan_folder_op(FileOpKind::Rename, root, "x").is_err());
    assert!(
        f.cat
            .plan_folder_op(FileOpKind::Rename, other, "2024/a\0b")
            .is_err()
    );
    assert!(f.cat.plan_folder_op(FileOpKind::Trash, other, "x").is_err());
    assert_eq!(f.rows("file_op"), before);
}

/// 配下のフォルダの行が、親の部分を親の行と違う綴り（NFC と NFD）で持っていても（取り込みで綴りだけが違う
/// パスを登録した場合。ensure_folder は階層ごとに渡された綴りを保存する）、付け替えで文字の途中を切らず、
/// 配下の残りの部分をそのまま新しい親の下に付ける（検証のラウンド 1 の指摘）。
#[test]
fn rebasing_keeps_subfolders_whose_parent_part_is_spelled_differently() {
    let mut f = Fixture::new();
    // É（NFC。2 バイト）と E + U+0301（NFD。3 バイト）: バイト長で切ると文字の途中になる。
    let nfc = f.cat.ensure_folder(f.volume, "nf/\u{c9}").unwrap();
    let sub = f.cat.ensure_folder(f.volume, "nf/E\u{301}/sub").unwrap();
    assert_eq!(f.cat.folder(sub).unwrap().rel_path, "nf/E\u{301}/sub");
    assert_eq!(f.cat.folder_subtree(nfc).unwrap(), vec![nfc, sub]);
    let op = f
        .cat
        .plan_folder_op(FileOpKind::Rename, nfc, "nf/Zed")
        .unwrap();
    f.cat.complete_file_op(op).unwrap();
    assert_eq!(f.cat.folder(nfc).unwrap().rel_path, "nf/Zed");
    assert_eq!(f.cat.folder(sub).unwrap().rel_path, "nf/Zed/sub");
    assert_eq!(f.cat.folder(sub).unwrap().parent_id, Some(nfc));

    // が（NFC。3 バイト）と か + U+3099（NFD。6 バイト）: 切れ目が文字の境界に当たっても、結合文字を残さない。
    let ga = f.cat.ensure_folder(f.volume, "nf2/\u{304c}").unwrap();
    let sub2 = f
        .cat
        .ensure_folder(f.volume, "nf2/\u{304b}\u{3099}/sub")
        .unwrap();
    let usb = f
        .cat
        .ensure_volume("uuid:5678", Some("USB"), Some("/media/me/USB"))
        .unwrap();
    f.cat.relocate_folder(ga, usb, "写真/Zed").unwrap();
    assert_eq!(f.cat.folder(sub2).unwrap().rel_path, "写真/Zed/sub");
    assert_eq!(f.cat.folder(sub2).unwrap().volume_id, usb);
    assert_eq!(f.cat.find_folder(usb, "写真/Zed/sub").unwrap(), Some(sub2));

    // 残りの部分の切り出し（部分の数で切る）。
    assert_eq!(
        crate::rel_path_below("nf/\u{c9}", "nf/E\u{301}/sub/x"),
        "sub/x"
    );
    assert_eq!(crate::rel_path_below("nf/\u{c9}", "nf/E\u{301}"), "");
    assert_eq!(crate::rel_path_below("", "a/b"), "a/b");
}

/// フォルダをカタログから除く: 写真のファイルが登録されていないフォルダ（配下を含む）だけを除き、ボリュームの
/// ルートならボリュームの記録も除く。終わっていないファイル操作が使うフォルダは除かない。
#[test]
fn only_folders_without_registered_files_are_removed() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let empty = f.cat.ensure_folder(f.volume, "2024/京都/空/下").unwrap();
    let empty_top = f
        .cat
        .find_folder(f.volume, "2024/京都/空")
        .unwrap()
        .unwrap();
    let before = f.rows("folder");
    // 写真のあるフォルダ（配下に写真があるフォルダも）は除かない。
    for id in [
        f.folder,
        f.cat.find_folder(f.volume, "2024").unwrap().unwrap(),
    ] {
        let e = f.cat.remove_folder(id).unwrap_err();
        assert!(
            matches!(&e, CatalogError::InvalidInput(m) if m.contains("1 個")),
            "{e}"
        );
    }
    assert_eq!(f.rows("folder"), before);
    // 空のフォルダは、配下ごと除く（親は残す）。
    let r = f.cat.remove_folder(empty_top).unwrap();
    assert_eq!((r.folders, r.volume), (2, None));
    assert_eq!(f.rows("folder"), before - 2);
    assert!(matches!(
        f.cat.folder(empty),
        Err(CatalogError::NotFound(_))
    ));
    assert_eq!(f.cat.folder(f.folder).unwrap().rel_path, "2024/京都");

    // 終わっていないファイル操作が使うフォルダは除かない。
    let dest = f.cat.ensure_folder(f.volume, "2025").unwrap();
    let op = f
        .cat
        .plan_file_moves(
            FileOpKind::Move,
            &[MoveRequest {
                file_id: a.file_id,
                dest_folder: dest,
                new_name: "A.ARW".to_owned(),
                method: MoveMethod::Rename,
            }],
        )
        .unwrap();
    let e = f.cat.remove_folder(dest).unwrap_err();
    assert!(e.to_string().contains("終わっていないファイル操作"), "{e}");
    f.cat.fail_file_op(op, "テスト").unwrap();
    assert_eq!(f.cat.remove_folder(dest).unwrap().folders, 1);

    // 写真を除いた後は、ボリュームのルートごと除ける（ボリュームの記録も）。
    f.cat.remove_assets(&[a.asset_id]).unwrap();
    let root = f.cat.find_folder(f.volume, "").unwrap().unwrap();
    f.cat.set_offline_volumes([f.volume]);
    let r = f.cat.remove_folder(root).unwrap();
    assert_eq!(r.volume, Some(f.volume));
    assert_eq!(f.rows("folder"), 0);
    assert!(f.cat.volume_records().unwrap().is_empty());
    assert!(!f.cat.is_volume_offline(f.volume));
    assert!(matches!(
        f.cat.remove_folder(root),
        Err(CatalogError::NotFound(_))
    ));
}

/// 配下の新しい相対パスが、ほかのフォルダと重なるときも断る（自分だけでなく配下も確かめる）。
#[test]
fn folder_moves_check_the_whole_subtree_for_conflicts() {
    let mut f = Fixture::new();
    f.cat.ensure_folder(f.volume, "2024/京都/嵐山").unwrap();
    // 移動先に、同じ名前のフォルダはないが、その配下に「嵐山」に当たるものがある。
    f.cat.ensure_folder(f.volume, "2025/京都/嵐山").unwrap();
    let k = f.cat.find_folder(f.volume, "2025/京都").unwrap().unwrap();
    // 2025/京都 → 2026/京都（衝突なし）はできる。
    let op = f
        .cat
        .plan_folder_op(FileOpKind::Move, k, "2026/京都")
        .unwrap();
    f.cat.complete_file_op(op).unwrap();
    // 2024/京都 を 2026/京都 へ: 自分も配下（嵐山）も重なる。
    assert!(matches!(
        f.cat
            .plan_folder_op(FileOpKind::Move, f.folder, "2026/京都"),
        Err(CatalogError::Conflict(_))
    ));
}

/// 場所の指定し直し（カタログの中だけ）: 別のボリュームへも付け替えられ、見つからない印が外れる。
#[test]
fn relocating_a_folder_rebinds_it_and_clears_the_missing_mark() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let sub = f.cat.ensure_folder(f.volume, "2024/京都/嵐山").unwrap();
    f.cat
        .set_folder_status(&[f.folder, sub], FolderStatus::Missing)
        .unwrap();
    let missing: Vec<_> = f
        .cat
        .folder_records()
        .unwrap()
        .into_iter()
        .filter(|r| r.status == FolderStatus::Missing)
        .map(|r| r.folder.id)
        .collect();
    assert_eq!(missing.len(), 2);
    // 同じ状態にするだけなら変えた数は 0。
    assert_eq!(
        f.cat
            .set_folder_status(&[f.folder], FolderStatus::Missing)
            .unwrap(),
        0
    );

    let usb = f
        .cat
        .ensure_volume("uuid:1234", Some("USB"), Some("/media/me/USB"))
        .unwrap();
    let ids = f
        .cat
        .relocate_folder(f.folder, usb, "写真/2024 京都")
        .unwrap();
    assert_eq!(ids, vec![f.folder, sub]);
    assert_eq!(
        f.cat
            .file_location(a.file_id)
            .unwrap()
            .absolute_path()
            .unwrap(),
        std::path::Path::new("/media/me/USB/写真/2024 京都/A.ARW")
    );
    assert_eq!(f.cat.folder(sub).unwrap().volume_id, usb);
    assert!(
        f.cat
            .folder_records()
            .unwrap()
            .iter()
            .all(|r| r.status == FolderStatus::Ok)
    );
    // 移し先の親（写真）は作られる。
    assert!(f.cat.find_folder(usb, "写真").unwrap().is_some());
    // 新しい場所に別のフォルダが登録されていれば断る。
    let other = f.cat.ensure_folder(f.volume, "2023").unwrap();
    assert!(matches!(
        f.cat.relocate_folder(other, usb, "写真/2024 京都"),
        Err(CatalogError::Conflict(_))
    ));
    assert_eq!(f.cat.folder_subtree(f.folder).unwrap(), vec![f.folder, sub]);
}

/// 別のボリュームへのコピーの移動: 移したファイルだけを反映し（起動時の確定）、移動先の大きさ・更新日時（ファイル
/// システムが丸めた値）と全体のハッシュを、リビジョンを上げずに記録する。見つからない印も外れる。
#[test]
fn copy_moves_record_the_destination_facts_without_a_new_revision() {
    let mut f = Fixture::new();
    let raw = f.photo("A.ARW", 1, None);
    let jpg = f.photo("A.JPG", 2, None);
    let usb = f
        .cat
        .ensure_volume("uuid:9", None, Some("/media/me/USB"))
        .unwrap();
    let dest = f.cat.ensure_folder(usb, "DCIM").unwrap();
    f.cat
        .set_file_status(raw.file_id, FileStatus::Missing, Some("確認の途中"))
        .unwrap();
    let op = f
        .cat
        .plan_file_moves(
            FileOpKind::Move,
            &[
                MoveRequest {
                    file_id: raw.file_id,
                    dest_folder: dest,
                    new_name: "A.ARW".to_owned(),
                    method: MoveMethod::Copy,
                },
                MoveRequest {
                    file_id: jpg.file_id,
                    dest_folder: dest,
                    new_name: "A.JPG".to_owned(),
                    method: MoveMethod::Copy,
                },
            ],
        )
        .unwrap();
    let rec = f.cat.file_op(op).unwrap();
    assert!(
        rec.payload
            .entries
            .iter()
            .all(|e| e.method == MoveMethod::Copy)
    );
    let json: String = f
        .cat
        .conn
        .query_row(
            "SELECT payload_json FROM file_op WHERE id = ?1",
            [op.get()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(json.contains("\"method\":\"copy\""), "{json}");
    f.cat.start_file_op(op).unwrap();
    let rounded = FileFacts {
        mtime_ns: facts(1).mtime_ns - 1,
        ..facts(1)
    };
    f.cat
        .complete_file_op_entries(
            op,
            &[AppliedEntry {
                file_id: raw.file_id,
                facts: Some(rounded.clone()),
                full_hash: Some(hex(77)),
            }],
            Some("1 件は元の場所のまま"),
        )
        .unwrap();
    let rec = f.cat.file_op(op).unwrap();
    assert_eq!(rec.state, FileOpState::Done);
    assert_eq!(rec.error.as_deref(), Some("1 件は元の場所のまま"));
    let moved = f.cat.file(raw.file_id).unwrap();
    assert_eq!(moved.folder_id, dest);
    assert_eq!(moved.facts, rounded);
    assert_eq!(moved.full_hash.as_deref(), Some(hex(77).as_str()));
    assert_eq!(moved.revision, raw.revision);
    assert_eq!(moved.status, FileStatus::Ok);
    assert_eq!(moved.status_reason, None);
    // 移さなかったファイルは元のまま。
    assert_eq!(f.cat.file(jpg.file_id).unwrap().folder_id, f.folder);
}

/// 版 11 までのファイル操作の記録（`method`・`folder` のない JSON）も読める。
#[test]
fn old_file_op_payloads_are_still_readable() {
    let json = r#"{"entries":[{"file_id":1,"asset_id":1,"from":{"folder_id":2,"name":"A.ARW","absolute_path":"/p/A.ARW"},"to":{"folder_id":3,"name":"A.ARW","absolute_path":"/q/A.ARW"}}]}"#;
    let p: FileOpPayload = serde_json::from_str(json).unwrap();
    assert_eq!(p.entries[0].method, MoveMethod::Rename);
    assert_eq!(p.folder, None);
    // 書くときも、名前の変更の項目には method を書かない（前の版の形のまま）。
    assert!(!serde_json::to_string(&p).unwrap().contains("method"));
}

/// 再リンク: ファイルを別のフォルダ・名前に結び付け直す（状態・内容の記録は変えない）。別のファイルが登録された
/// 場所へは結び付けない。見つからないファイルの一覧とフォルダごとの数。
#[test]
fn relinking_moves_only_the_location_and_refuses_registered_places() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let b = f.photo("B.ARW", 2, None);
    let c = f.photo("C.ARW", 3, None);
    f.cat
        .set_file_status(a.file_id, FileStatus::Missing, Some("ない"))
        .unwrap();
    f.cat
        .set_file_status(c.file_id, FileStatus::Missing, Some("ない"))
        .unwrap();
    assert_eq!(
        f.cat
            .missing_files(None, None, 10)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![a.file_id, c.file_id]
    );
    assert_eq!(
        f.cat
            .missing_files(None, Some(a.file_id), 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.cat.missing_file_counts().unwrap().get(&f.folder).copied(),
        Some(2)
    );
    let elsewhere = f.cat.ensure_folder(f.volume, "移した先").unwrap();
    assert!(
        f.cat
            .missing_files(Some(&[elsewhere]), None, 10)
            .unwrap()
            .is_empty()
    );

    f.cat
        .relink_file(a.file_id, elsewhere, "A-移した.ARW")
        .unwrap();
    let rec = f.cat.file(a.file_id).unwrap();
    assert_eq!(rec.folder_id, elsewhere);
    assert_eq!(rec.name, "A-移した.ARW");
    assert_eq!(
        rec.status,
        FileStatus::Missing,
        "状態は呼び出し側が確かめ直す"
    );
    assert_eq!(rec.revision, a.revision);
    // ファイル名のテキスト検索の索引も新しい名前になる。
    let text: String = f
        .cat
        .conn
        .query_row(
            "SELECT text_norm FROM asset_text WHERE asset_id = ?1",
            [a.asset_id.get()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(text.contains("a-移した.arw"), "{text}");
    // 別のファイル（B）が登録された場所へは結び付けない。
    assert!(matches!(
        f.cat.relink_file(c.file_id, f.folder, "b.arw"),
        Err(CatalogError::Conflict(_))
    ));
    assert_eq!(f.cat.file(b.file_id).unwrap().name, "B.ARW");
    // 自分の今の場所（大文字・小文字だけ違う名前）にはできる。
    f.cat.relink_file(c.file_id, f.folder, "c.arw").unwrap();
    assert_eq!(f.cat.file(c.file_id).unwrap().name, "c.arw");
}

/// パスから作った ID のボリュームの移し替え（FILE-03 の既存のカタログ）: 見えるフォルダを OS のボリューム ID の
/// ボリュームへ移す。移し先に同じ場所のフォルダがあればファイルをまとめ、名前の重なるファイルは残す。移さなかった
/// フォルダは元のボリュームに親を作り直し、フォルダがなくなったボリュームは消す。
#[test]
fn legacy_path_volumes_are_rebound_to_os_volume_ids() {
    let mut f = Fixture::new();
    // 版 11 までの形: ルート `/` のボリュームに、内蔵の写真と外付けの写真（/media/me/SD）。
    let legacy = f.cat.ensure_volume("path:/", None, Some("/")).unwrap();
    let pics = f.cat.ensure_folder(legacy, "home/me/写真").unwrap();
    let sd = f.cat.ensure_folder(legacy, "media/me/SD/DCIM").unwrap();
    let gone = f.cat.ensure_folder(legacy, "home/me/写真/古い").unwrap();
    let a = f
        .cat
        .register_file(&photo_req(pics, "A.ARW", 11, None))
        .unwrap();
    let b = f
        .cat
        .register_file(&photo_req(sd, "B.ARW", 12, None))
        .unwrap();
    let c = f
        .cat
        .register_file(&photo_req(sd, "C.ARW", 13, None))
        .unwrap();
    let old = f
        .cat
        .register_file(&photo_req(gone, "O.ARW", 14, None))
        .unwrap();
    // 移し替えの前に、SD を OS のボリューム ID で取り込んだもの（同じ DCIM。C は二重の登録）。
    let sd_vol = f
        .cat
        .ensure_volume("uuid:SD", Some("SD"), Some("/media/me/SD"))
        .unwrap();
    let sd_dcim = f.cat.ensure_folder(sd_vol, "DCIM").unwrap();
    let c2 = f
        .cat
        .register_file(&photo_req(sd_dcim, "C.ARW", 13, None))
        .unwrap();
    let root_vol = f.cat.ensure_volume("uuid:ROOT", None, Some("/")).unwrap();

    // 見えるフォルダ（「古い」は見つからないので移さない）。
    let folder = |rel: &str| f.cat.find_folder(legacy, rel).unwrap().unwrap();
    let to = |id, volume, rel: &str| FolderRebind {
        folder_id: id,
        volume_id: volume,
        rel_path: rel.to_owned(),
    };
    let rebinds = vec![
        to(folder(""), root_vol, ""),
        to(folder("home"), root_vol, "home"),
        to(folder("home/me"), root_vol, "home/me"),
        to(pics, root_vol, "home/me/写真"),
        to(folder("media"), root_vol, "media"),
        to(folder("media/me"), root_vol, "media/me"),
        to(folder("media/me/SD"), sd_vol, ""),
        to(sd, sd_vol, "DCIM"),
    ];
    let report = f.cat.rebind_folders(&rebinds).unwrap();
    // SD のルートと DCIM は、既にあるフォルダへまとめる。
    assert_eq!(report.merged, 2);
    assert_eq!(report.moved, 6);
    assert_eq!(report.conflicting_files, vec![c.file_id]);
    // 内蔵の写真は同じフォルダの ID のまま新しいボリュームへ。
    assert_eq!(f.cat.folder(pics).unwrap().volume_id, root_vol);
    assert_eq!(
        f.cat
            .file_location(a.file_id)
            .unwrap()
            .absolute_path()
            .unwrap(),
        std::path::Path::new("/home/me/写真/A.ARW")
    );
    // SD の B は既にある DCIM へまとめた。C は二重の登録なので元のフォルダに残る。
    assert_eq!(f.cat.file(b.file_id).unwrap().folder_id, sd_dcim);
    assert_eq!(f.cat.file(c2.file_id).unwrap().folder_id, sd_dcim);
    assert_eq!(f.cat.file(c.file_id).unwrap().folder_id, sd);
    assert_eq!(f.cat.folder(sd).unwrap().volume_id, legacy);
    // 移さなかったフォルダ（古い）は元のボリュームにあり、親は元のボリュームに作り直したもの。
    let gone_folder = f.cat.folder(gone).unwrap();
    assert_eq!(gone_folder.volume_id, legacy);
    let parent = f.cat.folder(gone_folder.parent_id.unwrap()).unwrap();
    assert_eq!(parent.volume_id, legacy);
    assert_eq!(parent.rel_path, "home/me/写真");
    assert_ne!(parent.id, pics);
    assert_eq!(f.cat.file(old.file_id).unwrap().folder_id, gone);
    // どのフォルダも、親と同じボリュームにある。
    let crossed: i64 = f
        .cat
        .conn
        .query_row(
            "SELECT count(*) FROM folder c JOIN folder p ON p.id = c.parent_id
             WHERE p.volume_id <> c.volume_id",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(crossed, 0);
    // 元のボリュームは、残ったフォルダがあるので消さない。
    assert!(report.removed_volumes.is_empty());

    // 残りも移せたら（「古い」が戻り、C の二重の登録を除いた）、元のボリュームは消える。
    f.cat.remove_assets(&[c.asset_id]).unwrap();
    let left: Vec<FolderRebind> = f
        .cat
        .folder_records()
        .unwrap()
        .into_iter()
        .filter(|r| r.folder.volume_id == legacy)
        .map(|r| {
            let rel = r.folder.rel_path.clone();
            if rel.starts_with("media/me/SD") {
                let rest = rel
                    .trim_start_matches("media/me/SD")
                    .trim_start_matches('/');
                to(r.folder.id, sd_vol, rest)
            } else {
                to(r.folder.id, root_vol, &rel)
            }
        })
        .collect();
    let report = f.cat.rebind_folders(&left).unwrap();
    assert_eq!(report.removed_volumes, vec![legacy]);
    assert!(
        f.cat
            .volume_records()
            .unwrap()
            .iter()
            .all(|v| v.id != legacy)
    );
    assert_eq!(f.cat.folder(gone).unwrap().volume_id, root_vol);
    assert_eq!(
        f.cat.file_location(old.file_id).unwrap().rel_path(),
        "home/me/写真/古い/O.ARW"
    );
    // 元のボリュームのフォルダは残らない（作り直した親も、まとめて空になったので消えた）。
    assert!(
        f.cat
            .folder_records()
            .unwrap()
            .iter()
            .all(|r| r.folder.volume_id != legacy)
    );
    // 同じものをもう一度移しても何も変わらない。
    let again = f.cat.rebind_folders(&left).unwrap();
    assert_eq!(again.moved + again.merged, 0);
}
