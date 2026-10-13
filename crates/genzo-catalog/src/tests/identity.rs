//! 内容による同一性（ORG-03）・重複の候補（IMP-04）・RAW と JPEG のペアの扱い（IMP-06）。

use genzo_model::{FileRole, FileStatus};
use rusqlite::StatementStatus;

use super::{Fixture, facts, hex, photo_req, rows};
use crate::{
    CATALOG_SCHEMA_VERSION, CONTENT_IDENTITY_SCHEMA_VERSION, Catalog, CatalogError, RegisterOptions,
};

/// クイックハッシュ・全体のハッシュで探すと、同じ値のファイルだけが id の順に返る（状態によらない）。
/// 値は 16 進数の小文字 64 文字でなければエラー。
#[test]
fn files_are_found_by_quick_and_full_hash() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let other = f.cat.ensure_folder(f.volume, "2024/コピー").unwrap();
    // 別のフォルダの、同じ大きさ・クイックハッシュのファイル（重複の候補）。
    let b = f
        .cat
        .register_file(&photo_req(other, "A.ARW", 1, None))
        .unwrap();
    let c = f.photo("C.ARW", 3, None);
    let found = f.cat.files_with_quick_hash(&hex(1)).unwrap();
    assert_eq!(
        found.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![a.file_id, b.file_id]
    );
    assert!(f.cat.files_with_quick_hash(&hex(99)).unwrap().is_empty());
    assert!(matches!(
        f.cat.files_with_quick_hash("ABC"),
        Err(CatalogError::InvalidInput(_))
    ));
    // 全体のハッシュは記録したものだけ。
    assert!(f.cat.files_with_full_hash(&hex(50)).unwrap().is_empty());
    for o in [&a, &b] {
        assert!(
            f.cat
                .set_full_hash_if_current(o.file_id, 1, &facts(1), &hex(50))
                .unwrap()
        );
    }
    assert!(
        f.cat
            .set_full_hash_if_current(c.file_id, 1, &facts(3), &hex(51))
            .unwrap()
    );
    let same = f.cat.files_with_full_hash(&hex(50)).unwrap();
    assert_eq!(
        same.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![a.file_id, b.file_id]
    );
    // 見つからない状態のファイルも返す（呼び出し側が状態で選ぶ）。
    f.cat
        .set_file_status(b.file_id, FileStatus::Missing, Some("ない"))
        .unwrap();
    assert_eq!(f.cat.files_with_full_hash(&hex(50)).unwrap().len(), 2);
    assert_eq!(f.cat.files_with_quick_hash(&hex(1)).unwrap().len(), 2);
}

/// 全体のハッシュは、記録の内容（リビジョン・大きさ・更新日時・クイックハッシュ）が計算した時と同じで、
/// まだ記録していないときだけ記録する。記録した後は別の値で上書きしない（ゴミ箱へ移す前の確認のため）。
#[test]
fn full_hash_is_stored_only_for_the_current_content() {
    let mut f = Fixture::new();
    let o = f.photo("A.ARW", 1, None);
    // リビジョン・大きさ・更新日時・クイックハッシュのどれかが違えば記録しない。
    assert!(
        !f.cat
            .set_full_hash_if_current(o.file_id, 2, &facts(1), &hex(70))
            .unwrap()
    );
    let mut other = facts(1);
    other.mtime_ns += 1;
    assert!(
        !f.cat
            .set_full_hash_if_current(o.file_id, 1, &other, &hex(70))
            .unwrap()
    );
    assert!(
        !f.cat
            .set_full_hash_if_current(o.file_id, 1, &facts(2), &hex(70))
            .unwrap()
    );
    assert_eq!(f.cat.file(o.file_id).unwrap().full_hash, None);
    // 同じなら記録する。同じ値をもう一度は true、別の値は記録しない。
    assert!(
        f.cat
            .set_full_hash_if_current(o.file_id, 1, &facts(1), &hex(70))
            .unwrap()
    );
    assert!(
        f.cat
            .set_full_hash_if_current(o.file_id, 1, &facts(1), &hex(70))
            .unwrap()
    );
    assert!(
        !f.cat
            .set_full_hash_if_current(o.file_id, 1, &facts(1), &hex(71))
            .unwrap()
    );
    assert_eq!(
        f.cat.file(o.file_id).unwrap().full_hash.as_deref(),
        Some(hex(70).as_str())
    );
    // 内容が変わって登録し直すと消え、新しいリビジョンで記録できる。
    let u = f.photo("A.ARW", 2, None);
    assert_eq!(u.revision, 2);
    assert_eq!(f.cat.file(o.file_id).unwrap().full_hash, None);
    assert!(
        f.cat
            .set_full_hash_if_current(o.file_id, 2, &facts(2), &hex(72))
            .unwrap()
    );
    // 見つからない状態のファイルには記録しない。
    let m = f.photo("M.ARW", 5, None);
    f.cat
        .set_file_status(m.file_id, FileStatus::Missing, Some("ない"))
        .unwrap();
    assert!(
        !f.cat
            .set_full_hash_if_current(m.file_id, 1, &facts(5), &hex(73))
            .unwrap()
    );
    // 値の形・ないファイル。
    assert!(matches!(
        f.cat
            .set_full_hash_if_current(o.file_id, 2, &facts(2), "xyz"),
        Err(CatalogError::InvalidInput(_))
    ));
    let missing_id = genzo_model::FileId::new(9999);
    assert!(matches!(
        f.cat
            .set_full_hash_if_current(missing_id, 1, &facts(1), &hex(1)),
        Err(CatalogError::NotFound(_))
    ));
}

/// 全体のハッシュのない（見つからない状態でない）ファイルを、id の順に少しずつ読める。数も数えられる。
#[test]
fn files_without_full_hash_are_listed_in_pages_and_counted() {
    let mut f = Fixture::new();
    let outs: Vec<_> = (0..5u64)
        .map(|i| f.photo(&format!("P{i}.ARW"), i, None))
        .collect();
    f.cat
        .set_full_hash_if_current(outs[1].file_id, 1, &facts(1), &hex(100))
        .unwrap();
    f.cat
        .set_file_status(outs[3].file_id, FileStatus::Missing, Some("ない"))
        .unwrap();
    // 読めない（error）ファイルも対象（内容は読める）。
    f.cat
        .set_file_status(outs[4].file_id, FileStatus::Error, Some("壊れている"))
        .unwrap();
    let page1 = f.cat.files_without_full_hash(None, 2).unwrap();
    assert_eq!(
        page1.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![outs[0].file_id, outs[2].file_id]
    );
    let page2 = f.cat.files_without_full_hash(Some(page1[1].id), 2).unwrap();
    assert_eq!(
        page2.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![outs[4].file_id]
    );
    assert!(
        f.cat
            .files_without_full_hash(Some(outs[4].file_id), 10)
            .unwrap()
            .is_empty()
    );
    let counts = f.cat.content_hash_counts().unwrap();
    assert_eq!((counts.files, counts.hashed, counts.pending), (5, 1, 3));
    // 空のカタログ。
    let empty = Fixture::new();
    assert_eq!(
        empty.cat.content_hash_counts().unwrap(),
        crate::ContentHashCounts::default()
    );
}

/// 内容が同じファイルの組を、最初のファイルの id の順に上限まで返し、総数も返す。
#[test]
fn duplicate_groups_list_files_with_the_same_content() {
    let mut f = Fixture::new();
    let other = f.cat.ensure_folder(f.volume, "2024/コピー").unwrap();
    let a1 = f.photo("A.ARW", 1, None);
    let b1 = f.photo("B.ARW", 2, None);
    let a2 = f
        .cat
        .register_file(&photo_req(other, "A.ARW", 1, None))
        .unwrap();
    let b2 = f
        .cat
        .register_file(&photo_req(other, "B2.ARW", 2, None))
        .unwrap();
    let b3 = f
        .cat
        .register_file(&photo_req(other, "B3.ARW", 2, None))
        .unwrap();
    let solo = f.photo("S.ARW", 3, None);
    for (o, n, h) in [
        (&a1, 1, 10),
        (&a2, 1, 10),
        (&b1, 2, 20),
        (&b2, 2, 20),
        (&b3, 2, 20),
        (&solo, 3, 30),
    ] {
        assert!(
            f.cat
                .set_full_hash_if_current(o.file_id, 1, &facts(n), &hex(h))
                .unwrap()
        );
    }
    let all = f.cat.duplicate_groups(10).unwrap();
    assert_eq!(all.total, 2);
    assert_eq!(all.groups.len(), 2);
    assert_eq!(all.groups[0].full_hash, hex(10));
    assert_eq!(
        all.groups[0].files.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![a1.file_id, a2.file_id]
    );
    assert_eq!(
        all.groups[1].files.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![b1.file_id, b2.file_id, b3.file_id]
    );
    let first = f.cat.duplicate_groups(1).unwrap();
    assert_eq!((first.total, first.groups.len()), (2, 1));
    // 全体のハッシュで探す問い合わせは索引を使う（表を走査しない）。
    let plan: Vec<String> = f
        .cat
        .conn
        .prepare(
            "EXPLAIN QUERY PLAN SELECT id FROM file WHERE full_hash = ?1 AND full_hash IS NOT NULL",
        )
        .unwrap()
        .query_map([hex(10)], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(
        plan.iter().any(|d| d.contains("file_full_hash")),
        "{plan:?}"
    );
}

/// クイックハッシュで探す問い合わせは索引 `file_quick_hash` を使い、`file` の表を走査しない（50 万件でも
/// 取り込みのファイルごとに候補だけを読む）。
#[test]
fn quick_hash_lookup_uses_the_index() {
    let mut f = Fixture::new();
    for i in 0..200u64 {
        f.photo(&format!("IMG{i:04}.ARW"), i, None);
    }
    let mut stmt = f
        .cat
        .conn
        .prepare("SELECT id FROM file WHERE quick_hash = ?1 ORDER BY id")
        .unwrap();
    let n = stmt
        .query_map([hex(7)], |r| r.get::<_, i64>(0))
        .unwrap()
        .count();
    assert_eq!(n, 1);
    assert_eq!(stmt.get_status(StatementStatus::FullscanStep), 0);
    assert_eq!(f.cat.files_with_quick_hash(&hex(7)).unwrap().len(), 1);
}

/// RAW と組の JPEG は、同じ asset のほかのファイルとしてたどれる（FILE-02 で一緒に移すため）。
#[test]
fn companion_files_follow_the_asset() {
    let mut f = Fixture::new();
    let raw = f.photo("DSC00001.ARW", 1, None);
    let jpg = f.photo("DSC00001.JPG", 2, None);
    let solo = f.photo("DSC00002.ARW", 3, None);
    let of_raw = f.cat.companion_files(raw.file_id).unwrap();
    assert_eq!(of_raw.len(), 1);
    assert_eq!(of_raw[0].id, jpg.file_id);
    assert_eq!(of_raw[0].role, FileRole::SidecarJpeg);
    let of_jpg = f.cat.companion_files(jpg.file_id).unwrap();
    assert_eq!(of_jpg.len(), 1);
    assert_eq!(of_jpg[0].id, raw.file_id);
    assert!(f.cat.companion_files(solo.file_id).unwrap().is_empty());
    assert!(matches!(
        f.cat.companion_files(genzo_model::FileId::new(9999)),
        Err(CatalogError::NotFound(_))
    ));
    assert_eq!(
        f.cat.master_variant_of(jpg.asset_id).unwrap(),
        raw.master_variant_id
    );
}

/// 拡張子を除いた名前が同じファイル（コピー取り込みの名前の衝突の確認。IMP-02・IMP-06）: 比較キー（大文字・
/// 小文字の違いを区別しない）で同じ名前のものだけを、比較キーの順に返す（`DSC00001.x.ARW` や
/// `DSC000010.JPG`、別のフォルダのものは含まない）。
#[test]
fn names_with_the_same_stem_are_listed() {
    let mut f = Fixture::new();
    f.photo("DSC00001.ARW", 1, None);
    f.photo("dsc00001.jpg", 2, None);
    f.photo("DSC00001.x.ARW", 3, None);
    f.photo("DSC000010.JPG", 4, None);
    f.photo("DSC00001-1.JPG", 5, None);
    let other = f.cat.ensure_folder(f.volume, "別").unwrap();
    f.cat
        .register_file(&photo_req(other, "DSC00001.MP4", 6, None))
        .unwrap();
    assert_eq!(
        f.cat
            .names_with_same_stem(f.folder, "DSC00001.DNG")
            .unwrap(),
        vec!["DSC00001.ARW".to_owned(), "dsc00001.jpg".to_owned()]
    );
    assert!(
        f.cat
            .names_with_same_stem(f.folder, "DSC00002.DNG")
            .unwrap()
            .is_empty()
    );
    // 拡張子のない名前は空。
    assert!(
        f.cat
            .names_with_same_stem(f.folder, "DSC00001")
            .unwrap()
            .is_empty()
    );
}

/// ペアにしない設定では、同じフォルダの同じ名前の RAW と JPEG を別々の asset（写真）にする。登録済みの
/// ペアは分けない。後でペアにする設定で取り込んだ RAW は、別々に登録した JPEG だけの asset にまとまる。
#[test]
fn raw_and_jpeg_can_be_registered_separately() {
    let mut f = Fixture::new();
    let separate = RegisterOptions {
        pair_raw_jpeg: false,
    };
    let out = f
        .cat
        .register_batch_with(
            &[
                photo_req(f.folder, "A.JPG", 1, None),
                photo_req(f.folder, "A.ARW", 2, None),
            ],
            &separate,
        )
        .unwrap();
    assert_ne!(out[0].asset_id, out[1].asset_id);
    assert!(out.iter().all(|o| o.role == FileRole::Primary && !o.paired));
    assert_eq!(f.rows("asset"), 2);
    // 登録し直しても（ペアにする設定でも）まとめない。
    let again = f
        .cat
        .register_batch(&[photo_req(f.folder, "A.JPG", 1, None)])
        .unwrap();
    assert_eq!(again[0].asset_id, out[0].asset_id);
    assert_eq!(again[0].role, FileRole::Primary);
    // 登録済みのペアは、ペアにしない設定で登録し直しても分けない。
    let pair = f
        .cat
        .register_batch(&[
            photo_req(f.folder, "B.ARW", 3, None),
            photo_req(f.folder, "B.JPG", 4, None),
        ])
        .unwrap();
    assert_eq!(pair[0].asset_id, pair[1].asset_id);
    let kept = f
        .cat
        .register_batch_with(&[photo_req(f.folder, "B.JPG", 4, None)], &separate)
        .unwrap();
    assert_eq!(kept[0].role, FileRole::SidecarJpeg);
    // 別々に登録した JPEG だけの asset に、後でペアにする設定で RAW を取り込むとまとまる。
    let c_jpg = f
        .cat
        .register_batch_with(&[photo_req(f.folder, "C.JPG", 5, None)], &separate)
        .unwrap();
    let c_raw = f
        .cat
        .register_batch(&[photo_req(f.folder, "C.ARW", 6, None)])
        .unwrap();
    assert_eq!(c_raw[0].asset_id, c_jpg[0].asset_id);
    assert!(c_raw[0].paired);
    assert!(RegisterOptions::default().pair_raw_jpeg);
}

/// `photo_files_v1` の 1 行（file_id・asset_id・master_variant_id・role・file_name・folder_path・
/// volume_mount_path・full_hash）。
type FileViewRow = (
    i64,
    i64,
    i64,
    String,
    String,
    String,
    String,
    Option<String>,
);

/// 版 9 のカタログを開くと版 10 に移行し、クイックハッシュ・全体のハッシュの索引と公開ビュー
/// `photo_files_v1` ができる。記録済みの全体のハッシュ・ファイルの行は変わらない。
#[test]
fn version_9_catalog_migrates_to_content_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let (raw, jpg, solo) = {
        let mut cat =
            crate::testing::open_at_version(&path, CONTENT_IDENTITY_SCHEMA_VERSION - 1).unwrap();
        let vol = cat
            .ensure_volume("v", Some("内蔵"), Some("/media"))
            .unwrap();
        let folder = cat.ensure_folder(vol, "2024").unwrap();
        let raw = cat
            .register_file(&photo_req(folder, "A.ARW", 1, None))
            .unwrap();
        let jpg = cat
            .register_file(&photo_req(folder, "A.JPG", 2, None))
            .unwrap();
        let solo = cat
            .register_file(&photo_req(folder, "B.ARW", 3, None))
            .unwrap();
        cat.set_full_hash(raw.file_id, &hex(40)).unwrap();
        let indexes: i64 = cat
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'index'
                 AND name IN ('file_quick_hash', 'file_full_hash')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(indexes, 0);
        cat.close().unwrap();
        (raw, jpg, solo)
    };
    let cat = Catalog::open(&path).unwrap();
    assert_eq!(
        cat.open_report().migrated_from,
        Some(CONTENT_IDENTITY_SCHEMA_VERSION - 1)
    );
    assert_eq!(cat.schema_version().unwrap(), CATALOG_SCHEMA_VERSION);
    let indexes: Vec<String> = cat
        .conn
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'index'
             AND name IN ('file_quick_hash', 'file_full_hash') ORDER BY name",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(indexes, vec!["file_full_hash", "file_quick_hash"]);
    assert_eq!(rows(&cat, "file"), 3);
    assert_eq!(
        cat.file(raw.file_id).unwrap().full_hash.as_deref(),
        Some(hex(40).as_str())
    );
    // 公開ビュー: 1 行 = 1 つのファイル。JPEG も同じ asset・マスターの行として出る。
    let view: Vec<FileViewRow> = cat
        .conn
        .prepare(
            "SELECT file_id, asset_id, master_variant_id, role, file_name, folder_path,
                    volume_mount_path, full_hash
             FROM photo_files_v1 ORDER BY file_id",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(view.len(), 3);
    assert_eq!(view[0].0, raw.file_id.get());
    assert_eq!(view[0].2, raw.master_variant_id.get());
    assert_eq!(view[0].3, "primary");
    assert_eq!(view[0].7.as_deref(), Some(hex(40).as_str()));
    assert_eq!(view[1].0, jpg.file_id.get());
    assert_eq!(view[1].1, raw.asset_id.get());
    assert_eq!(view[1].2, raw.master_variant_id.get());
    assert_eq!(view[1].3, "sidecar_jpeg");
    assert_eq!(view[1].4, "A.JPG");
    assert_eq!((view[1].5.as_str(), view[1].6.as_str()), ("2024", "/media"));
    assert_eq!(view[2].0, solo.file_id.get());
    assert_eq!(view[2].7, None);
    cat.close().unwrap();
}
