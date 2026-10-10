//! 登録（IMP-01・VID-01）のテスト: 冪等な再登録、変化の検知、RAW と JPEG のペア、
//! バッチの中断と再開、メタデータの保存。

use genzo_model::{AssetKind, FileRole, FileStatus, TzSource};

use super::{Fixture, capture, facts, photo_req, video_req};
use crate::{
    Catalog, CatalogError, FileFacts, MediaMetadata, RegisterFile, RegisterStatus,
    rel_path_from_path,
};

#[test]
fn registering_creates_asset_master_variant_file_and_history() {
    let mut f = Fixture::new();
    let o = f.photo("DSC00001.ARW", 1, Some("2024:05:01 12:34:56"));
    assert_eq!(o.status, RegisterStatus::Added);
    assert_eq!(o.role, FileRole::Primary);
    assert_eq!(o.revision, 1);
    assert!(!o.paired);
    assert_eq!(f.rows("asset"), 1);
    assert_eq!(f.rows("variant"), 1);
    assert_eq!(f.rows("file"), 1);
    // 最初の履歴（読み込み）を 1 件持つ。
    let history = f.cat.history(o.master_variant_id).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].label, crate::HISTORY_LABEL_IMPORT);
    assert!(history[0].is_current);

    let a = f.cat.asset(o.asset_id).unwrap();
    assert_eq!(a.kind, AssetKind::Photo);
    assert_eq!(a.camera.as_deref(), Some("SONY ILCE-7M4"));
    assert_eq!(a.lens.as_deref(), Some("FE 24-70mm F2.8 GM II"));
    assert_eq!(a.iso, Some(100));
    assert_eq!(a.aperture, Some(f64::from(2.8f32)));
    assert_eq!(a.width, Some(7008));
    assert_eq!(a.height, Some(4672));
    assert_eq!(a.capture.raw.as_deref(), Some("2024:05:01 12:34:56"));
    assert_eq!(a.capture.offset.as_deref(), Some("+09:00"));
    assert_eq!(a.capture.tz_source, TzSource::Exif);
    assert_eq!(
        a.capture.utc_db_string().as_deref(),
        Some("2024-05-01T03:34:56.000Z")
    );
    assert!(a.video.is_none());

    let file = f.cat.file(o.file_id).unwrap();
    assert_eq!(file.name, "DSC00001.ARW");
    assert_eq!(file.facts, facts(1));
    assert_eq!(file.status, FileStatus::Ok);
    assert_eq!(file.full_hash, None);
    let loc = f.cat.file_location(o.file_id).unwrap();
    assert_eq!(loc.rel_path(), "2024/京都/DSC00001.ARW");
    assert_eq!(
        loc.absolute_path().unwrap(),
        std::path::Path::new("/mnt/photos/2024/京都/DSC00001.ARW")
    );
}

#[test]
fn reregistering_the_same_file_is_idempotent() {
    let mut f = Fixture::new();
    let first = f.photo("DSC00001.ARW", 1, Some("2024:05:01 12:34:56"));
    for _ in 0..3 {
        let again = f.photo("DSC00001.ARW", 1, Some("2024:05:01 12:34:56"));
        assert_eq!(again.status, RegisterStatus::Unchanged);
        assert_eq!(again.asset_id, first.asset_id);
        assert_eq!(again.file_id, first.file_id);
        assert_eq!(again.master_variant_id, first.master_variant_id);
        assert_eq!(again.revision, 1);
    }
    // 大文字・小文字・正規化の違う名前でも同じファイル（Windows・macOS の既定）。
    let lower = f.photo("dsc00001.arw", 1, Some("2024:05:01 12:34:56"));
    assert_eq!(lower.status, RegisterStatus::Unchanged);
    assert_eq!(lower.file_id, first.file_id);
    // 表示用の名前は新しいものになる。
    assert_eq!(f.cat.file(first.file_id).unwrap().name, "dsc00001.arw");
    assert_eq!(f.rows("asset"), 1);
    assert_eq!(f.rows("variant"), 1);
    assert_eq!(f.rows("file"), 1);
    assert_eq!(f.rows("history_entry"), 1);
}

#[test]
fn nfd_and_nfc_names_are_the_same_file() {
    let mut f = Fixture::new();
    let nfc = f.photo("\u{304c}.JPG", 1, None);
    let nfd = f.photo("\u{304b}\u{3099}.JPG", 1, None);
    assert_eq!(nfd.status, RegisterStatus::Unchanged);
    assert_eq!(nfd.file_id, nfc.file_id);
    // フォルダも同じ。
    let a = f
        .cat
        .ensure_folder(f.volume, "2024/\u{304b}\u{3099}")
        .unwrap();
    let b = f.cat.ensure_folder(f.volume, "2024/\u{304c}").unwrap();
    assert_eq!(a, b);
}

#[test]
fn content_changes_increment_revision() {
    let mut f = Fixture::new();
    let first = f.photo("A.ARW", 1, Some("2024:05:01 12:00:00"));
    f.cat.set_full_hash(first.file_id, &super::hex(99)).unwrap();

    // サイズ・更新日時・クイックハッシュのそれぞれの変化を検知する。
    let mut req = photo_req(f.folder, "A.ARW", 1, Some("2024:05:01 12:00:00"));
    req.facts.size += 1;
    let o = f.cat.register_file(&req).unwrap();
    assert_eq!(o.status, RegisterStatus::Updated);
    assert_eq!(o.revision, 2);
    assert_eq!(f.cat.file(first.file_id).unwrap().full_hash, None);

    req.facts.mtime_ns += 1;
    assert_eq!(f.cat.register_file(&req).unwrap().revision, 3);
    req.facts.quick_hash = super::hex(1234);
    let o = f.cat.register_file(&req).unwrap();
    assert_eq!((o.status, o.revision), (RegisterStatus::Updated, 4));
    // 同じ内容でもう一度登録しても変わらない。
    let o = f.cat.register_file(&req).unwrap();
    assert_eq!((o.status, o.revision), (RegisterStatus::Unchanged, 4));
    assert_eq!(f.rows("asset"), 1);
    assert_eq!(f.rows("file"), 1);
}

#[test]
fn content_change_updates_metadata_but_keeps_user_corrections() {
    let mut f = Fixture::new();
    let o = f.photo("A.ARW", 1, Some("2024:05:01 12:00:00"));
    // ユーザーが時計のずれ（+60 秒）を補正した。
    let corrected = f
        .cat
        .capture_time(o.asset_id)
        .unwrap()
        .with_correction(60)
        .unwrap();
    f.cat.set_capture_time(o.asset_id, &corrected).unwrap();
    f.cat
        .set_rating(&[o.master_variant_id], genzo_model::Rating::MAX)
        .unwrap();

    let mut req = photo_req(f.folder, "A.ARW", 2, Some("2024:05:01 12:00:10"));
    req.metadata = MediaMetadata::Photo(super::photo_meta("ILCE-7RM5", "FE 50mm F1.2 GM"));
    let u = f.cat.register_file(&req).unwrap();
    assert_eq!(u.status, RegisterStatus::Updated);
    let a = f.cat.asset(o.asset_id).unwrap();
    assert_eq!(a.camera.as_deref(), Some("SONY ILCE-7RM5"));
    assert_eq!(a.capture.correction_s, 60);
    assert_eq!(
        a.capture.utc_db_string().as_deref(),
        Some("2024-05-01T03:01:10.000Z")
    );
    // 評価は variant のものなので残る。
    assert_eq!(
        f.cat.variant_marks(o.master_variant_id).unwrap().0,
        genzo_model::Rating::MAX
    );
}

#[test]
fn raw_then_jpeg_are_grouped_into_one_asset() {
    let mut f = Fixture::new();
    let raw = f.photo("DSC00001.ARW", 1, Some("2024:05:01 12:00:00"));
    let jpg = f.photo("DSC00001.JPG", 2, Some("2024:05:01 12:00:00"));
    assert_eq!(jpg.status, RegisterStatus::Added);
    assert_eq!(jpg.asset_id, raw.asset_id);
    assert_eq!(jpg.master_variant_id, raw.master_variant_id);
    assert_eq!(jpg.role, FileRole::SidecarJpeg);
    assert!(jpg.paired);
    assert_eq!(f.rows("asset"), 1);
    assert_eq!(f.rows("variant"), 1);
    assert_eq!(f.rows("file"), 2);
    let files = f.cat.files_of_asset(raw.asset_id).unwrap();
    assert_eq!(files[0].role, FileRole::Primary);
    assert_eq!(files[1].role, FileRole::SidecarJpeg);
    // 再登録は冪等。
    let again = f.photo("DSC00001.JPG", 2, Some("2024:05:01 12:00:00"));
    assert_eq!(again.status, RegisterStatus::Unchanged);
    assert_eq!(again.role, FileRole::SidecarJpeg);

    // 別のフォルダの同じ名前の JPEG はまとめない。
    let other = f.cat.ensure_folder(f.volume, "2024/別").unwrap();
    let o = f
        .cat
        .register_file(&photo_req(other, "DSC00001.JPG", 3, None))
        .unwrap();
    assert_ne!(o.asset_id, raw.asset_id);
    assert_eq!(o.role, FileRole::Primary);
    // 名前の一部だけが同じもの・拡張子の違う動画はまとめない。
    let o = f.photo("DSC00001.1.JPG", 4, None);
    assert_ne!(o.asset_id, raw.asset_id);
    let mov = f
        .cat
        .register_file(&video_req(f.folder, "DSC00001.MP4", 5, None))
        .unwrap();
    assert_ne!(mov.asset_id, raw.asset_id);
    // 2 つ目の JPEG（JPEG 拡張子違い）は、既に JPEG を持つ RAW にはまとめない。
    let o = f.photo("DSC00001.JPEG", 6, None);
    assert_ne!(o.asset_id, raw.asset_id);
}

#[test]
fn jpeg_then_raw_are_grouped_and_raw_becomes_primary() {
    let mut f = Fixture::new();
    let jpg = f.photo("IMG_0001.JPG", 1, Some("2024:05:01 12:00:00"));
    f.cat
        .set_rating(
            &[jpg.master_variant_id],
            genzo_model::Rating::new(4).unwrap(),
        )
        .unwrap();
    let mut raw_req = photo_req(f.folder, "IMG_0001.CR3", 2, Some("2024:05:01 12:00:00"));
    raw_req.metadata = MediaMetadata::Photo(genzo_model::PhotoMetadata {
        make: Some("Canon".to_owned()),
        model: Some("Canon EOS R5".to_owned()),
        ..Default::default()
    });
    let raw = f.cat.register_file(&raw_req).unwrap();
    assert_eq!(raw.status, RegisterStatus::Added);
    assert!(raw.paired);
    assert_eq!(raw.role, FileRole::Primary);
    assert_eq!(raw.asset_id, jpg.asset_id);
    assert_eq!(raw.master_variant_id, jpg.master_variant_id);
    let files = f.cat.files_of_asset(jpg.asset_id).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].name, "IMG_0001.CR3");
    assert_eq!(files[0].role, FileRole::Primary);
    assert_eq!(files[1].role, FileRole::SidecarJpeg);
    // メタデータは RAW のもの。評価は残る。
    assert_eq!(
        f.cat.asset(jpg.asset_id).unwrap().camera.as_deref(),
        Some("Canon EOS R5")
    );
    assert_eq!(
        f.cat.variant_marks(jpg.master_variant_id).unwrap().0.get(),
        4
    );
    // JPEG を登録し直しても sidecar のまま。
    let again = f.photo("IMG_0001.JPG", 1, Some("2024:05:01 12:00:00"));
    assert_eq!(again.status, RegisterStatus::Unchanged);
    assert_eq!(again.role, FileRole::SidecarJpeg);
    assert_eq!(f.rows("asset"), 1);
}

#[test]
fn batch_groups_pairs_regardless_of_order_and_keeps_result_order() {
    let mut f = Fixture::new();
    let reqs = vec![
        photo_req(f.folder, "A.JPG", 1, None),
        photo_req(f.folder, "B.ARW", 2, None),
        photo_req(f.folder, "A.ARW", 3, None),
        photo_req(f.folder, "B.JPG", 4, None),
        photo_req(f.folder, "C.JPG", 5, None),
    ];
    let out = f.cat.register_batch(&reqs).unwrap();
    assert_eq!(out.len(), 5);
    assert_eq!(out[0].role, FileRole::SidecarJpeg);
    assert_eq!(out[1].role, FileRole::Primary);
    assert_eq!(out[2].role, FileRole::Primary);
    assert_eq!(out[3].role, FileRole::SidecarJpeg);
    assert_eq!(out[4].role, FileRole::Primary);
    assert_eq!(out[0].asset_id, out[2].asset_id);
    assert_eq!(out[1].asset_id, out[3].asset_id);
    assert_eq!(f.rows("asset"), 3);
    assert_eq!(f.rows("file"), 5);
}

#[test]
fn failed_batch_is_rolled_back_and_can_be_resumed() {
    let mut f = Fixture::new();
    let all: Vec<RegisterFile> = (0..20)
        .map(|i| {
            photo_req(
                f.folder,
                &format!("DSC{i:05}.ARW"),
                i,
                Some("2024:05:01 12:00:00"),
            )
        })
        .collect();
    // 前半を登録する。
    f.cat.register_batch(&all[..10]).unwrap();
    assert_eq!(f.rows("asset"), 10);

    // 後半の途中で失敗する（存在しないフォルダ。外部キーの違反）→ バッチ全体が取り消される。
    let mut broken = all[10..].to_vec();
    broken[5].folder_id = genzo_model::FolderId::new(9999);
    let err = f.cat.register_batch(&broken).unwrap_err();
    assert!(err.is_constraint_violation(), "{err}");
    assert_eq!(f.rows("asset"), 10);
    assert_eq!(f.rows("file"), 10);
    assert_eq!(f.rows("variant"), 10);
    assert_eq!(f.rows("asset_text"), 10);

    // 不正な要求（クイックハッシュの形が違う）は、書き込む前に拒否する。
    let mut invalid = all[10..].to_vec();
    invalid[0].facts.quick_hash = "xyz".to_owned();
    assert!(matches!(
        f.cat.register_batch(&invalid),
        Err(CatalogError::InvalidInput(_))
    ));

    // 全体を登録し直すと、前半は Unchanged、後半は Added になり、件数は 20。
    let out = f.cat.register_batch(&all).unwrap();
    assert!(
        out[..10]
            .iter()
            .all(|o| o.status == RegisterStatus::Unchanged)
    );
    assert!(out[10..].iter().all(|o| o.status == RegisterStatus::Added));
    assert_eq!(f.rows("asset"), 20);
    assert_eq!(f.rows("file"), 20);
    assert!(f.cat.check_integrity().unwrap().is_ok());
}

#[test]
fn interrupted_process_keeps_committed_batches() {
    let mut f = Fixture::new();
    let path = f.catalog_path();
    let folder = f.folder;
    let all: Vec<RegisterFile> = (0..30)
        .map(|i| photo_req(folder, &format!("IMG{i:04}.ARW"), i, None))
        .collect();
    f.cat.register_batch(&all[..15]).unwrap();
    // トランザクションの途中で接続を失う（異常終了の代わり）。
    {
        let tx = f.cat.conn.transaction().unwrap();
        tx.execute(
            "INSERT INTO asset(kind, created_at) VALUES ('photo', 'x')",
            [],
        )
        .unwrap();
        std::mem::forget(tx);
    }
    drop(f.cat);
    let mut cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().previous_shutdown,
        crate::PreviousShutdown::Unclean
    );
    assert_eq!(super::rows(&cat, "asset"), 15);
    let out = cat.register_batch(&all).unwrap();
    assert_eq!(
        out.iter()
            .filter(|o| o.status == RegisterStatus::Unchanged)
            .count(),
        15
    );
    assert_eq!(super::rows(&cat, "asset"), 30);
    cat.close().unwrap();
}

#[test]
fn video_metadata_is_stored() {
    let mut f = Fixture::new();
    let o = f
        .cat
        .register_file(&video_req(
            f.folder,
            "DJI_0001.MP4",
            1,
            Some("2024-05-01T03:00:00Z"),
        ))
        .unwrap();
    let a = f.cat.asset(o.asset_id).unwrap();
    assert_eq!(a.kind, AssetKind::Video);
    assert_eq!((a.width, a.height), (Some(3840), Some(2160)));
    let v = a.video.unwrap();
    assert_eq!(v.duration_s, Some(12.5));
    assert_eq!(v.fps, Some(59.94));
    assert_eq!(v.codec.as_deref(), Some("hevc"));
    assert_eq!(v.bit_depth, Some(10));
    assert_eq!(v.color_transfer.as_deref(), Some("arib-std-b67"));
    assert_eq!(v.color_primaries.as_deref(), Some("bt2020"));
    // 文字列に含まれるオフセット（Z）を使う。
    assert_eq!(
        a.capture.utc_db_string().as_deref(),
        Some("2024-05-01T03:00:00.000Z")
    );
    assert_eq!(a.capture.tz_source, TzSource::Exif);
}

#[test]
fn files_with_unreadable_metadata_are_registered_with_error_status() {
    let mut f = Fixture::new();
    let req = RegisterFile {
        folder_id: f.folder,
        name: "broken.ARW".to_owned(),
        facts: facts(1),
        kind: AssetKind::Photo,
        metadata: MediaMetadata::None,
        capture: capture(None),
        error: Some("メタデータを読めません".to_owned()),
    };
    let o = f.cat.register_file(&req).unwrap();
    let file = f.cat.file(o.file_id).unwrap();
    assert_eq!(file.status, FileStatus::Error);
    assert_eq!(
        file.status_reason.as_deref(),
        Some("メタデータを読めません")
    );
    assert_eq!(
        f.cat.files_with_status(FileStatus::Error).unwrap(),
        vec![o.file_id]
    );
    // 直ったファイルを登録し直すと ok に戻る（内容は同じなので Unchanged）。
    let fixed = RegisterFile { error: None, ..req };
    assert_eq!(
        f.cat.register_file(&fixed).unwrap().status,
        RegisterStatus::Unchanged
    );
    let file = f.cat.file(o.file_id).unwrap();
    assert_eq!(file.status, FileStatus::Ok);
    assert_eq!(file.status_reason, None);
}

#[test]
fn invalid_requests_are_rejected() {
    let mut f = Fixture::new();
    for name in ["", ".", "..", "a/b.jpg", "a\0.jpg"] {
        let r = photo_req(f.folder, name, 1, None);
        assert!(
            matches!(f.cat.register_file(&r), Err(CatalogError::InvalidInput(_))),
            "{name:?}"
        );
    }
    let mut r = photo_req(f.folder, "a.jpg", 1, None);
    r.kind = AssetKind::Video;
    assert!(matches!(
        f.cat.register_file(&r),
        Err(CatalogError::InvalidInput(_))
    ));
    let mut r = photo_req(f.folder, "a.jpg", 1, None);
    r.facts = FileFacts {
        size: u64::MAX,
        ..facts(1)
    };
    assert!(matches!(
        f.cat.register_file(&r),
        Err(CatalogError::InvalidInput(_))
    ));
    assert_eq!(f.rows("asset"), 0);
}

#[test]
fn folders_are_created_with_parents_and_are_idempotent() {
    let mut f = Fixture::new();
    let deep = f.cat.ensure_folder(f.volume, "/2024/京都/day1/").unwrap();
    let folder = f.cat.folder(deep).unwrap();
    assert_eq!(folder.rel_path, "2024/京都/day1");
    assert_eq!(folder.name(), "day1");
    let parent = f.cat.folder(folder.parent_id.unwrap()).unwrap();
    assert_eq!(parent.id, f.folder);
    let root = f.cat.ensure_folder(f.volume, "").unwrap();
    assert_eq!(f.cat.folder(root).unwrap().parent_id, None);
    assert_eq!(f.cat.folder(root).unwrap().name(), "");
    // ルート、2024、2024/京都、2024/京都/day1 の 4 つ。
    let all = f.cat.folders(f.volume).unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(
        f.cat.ensure_folder(f.volume, "2024/京都/DAY1").unwrap(),
        deep
    );
    assert_eq!(
        f.cat.find_folder(f.volume, "2024/京都/Day1").unwrap(),
        Some(deep)
    );
    assert_eq!(f.cat.find_folder(f.volume, "nothing").unwrap(), None);
    assert!(f.cat.ensure_folder(f.volume, "a/../b").is_err());
    assert!(matches!(
        f.cat.folder(genzo_model::FolderId::new(999)),
        Err(CatalogError::NotFound(_))
    ));
    let rel = rel_path_from_path(std::path::Path::new("2024/京都/day1")).unwrap();
    assert_eq!(f.cat.find_folder(f.volume, &rel).unwrap(), Some(deep));
}

#[test]
fn volumes_are_upserted() {
    let mut f = Fixture::new();
    let again = f
        .cat
        .ensure_volume("vol-1", None, Some("/media/new"))
        .unwrap();
    assert_eq!(again, f.volume);
    let vols = f.cat.volumes().unwrap();
    assert_eq!(vols.len(), 1);
    assert_eq!(vols[0].label.as_deref(), Some("内蔵"));
    assert_eq!(vols[0].last_mount_path.as_deref(), Some("/media/new"));
    assert!(f.cat.ensure_volume("  ", None, None).is_err());
}

#[test]
fn file_facts_updates_and_status_changes() {
    let mut f = Fixture::new();
    let o = f.photo("A.ARW", 1, None);
    use crate::FactsChange;
    assert_eq!(
        f.cat.update_file_facts(o.file_id, &facts(1)).unwrap(),
        FactsChange::Unchanged
    );
    f.cat
        .set_file_status(o.file_id, FileStatus::Missing, Some("見つかりません"))
        .unwrap();
    let file = f.cat.file(o.file_id).unwrap();
    assert_eq!(file.status, FileStatus::Missing);
    assert_eq!(file.status_reason.as_deref(), Some("見つかりません"));
    assert_eq!(
        f.cat.files_with_status(FileStatus::Missing).unwrap(),
        vec![o.file_id]
    );
    // 見つかった（内容が変わっていた）。
    assert_eq!(
        f.cat.update_file_facts(o.file_id, &facts(2)).unwrap(),
        FactsChange::Updated { revision: 2 }
    );
    let file = f.cat.file(o.file_id).unwrap();
    assert_eq!(file.status, FileStatus::Ok);
    assert_eq!(file.status_reason, None);
    assert_eq!(file.revision, 2);
    // ok にすると理由は消える。
    f.cat
        .set_file_status(o.file_id, FileStatus::Error, Some("壊れています"))
        .unwrap();
    f.cat
        .set_file_status(o.file_id, FileStatus::Ok, Some("無視される"))
        .unwrap();
    assert_eq!(f.cat.file(o.file_id).unwrap().status_reason, None);

    let missing = genzo_model::FileId::new(999);
    assert!(matches!(
        f.cat.set_file_status(missing, FileStatus::Ok, None),
        Err(CatalogError::NotFound(_))
    ));
    assert!(matches!(
        f.cat.update_file_facts(missing, &facts(1)),
        Err(CatalogError::NotFound(_))
    ));
    assert!(f.cat.set_full_hash(o.file_id, "ABC").is_err());
    assert!(f.cat.set_full_hash(missing, &super::hex(1)).is_err());
    let bad = FileFacts {
        quick_hash: "nothex".to_owned(),
        ..facts(1)
    };
    assert!(f.cat.update_file_facts(o.file_id, &bad).is_err());
}

#[test]
fn registering_real_files_reads_them_read_only() {
    let mut f = Fixture::new();
    let path = f.dir.path().join("photo.jpg");
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&path, &data).unwrap();
    let before = crate::full_hash(&path).unwrap();
    let facts = FileFacts::read(&path).unwrap();
    let o = f
        .cat
        .register_file(&RegisterFile::photo(
            f.folder,
            "photo.jpg",
            facts.clone(),
            Default::default(),
            capture(None),
        ))
        .unwrap();
    f.cat.set_full_hash(o.file_id, &before).unwrap();
    assert_eq!(
        f.cat.file(o.file_id).unwrap().full_hash.as_deref(),
        Some(before.as_str())
    );
    // DATA-01: 元ファイルは変わらない。
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert_eq!(crate::full_hash(&path).unwrap(), before);
}
