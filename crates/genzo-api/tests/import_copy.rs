//! v1 のパッケージ「import」の結合テスト: コピー取り込み（IMP-02。日付別のフォルダ・RAW と JPEG の組・
//! 内容の照合・上書きしない・失敗と取り消しの後始末・取り込み直し・名前を決めるだけの確認）、リネーム規則
//! （IMP-03。テンプレート・連番・名前の衝突の回避・名前の例）、取り込み時のキーワード・説明のメタデータの
//! 一括付与（IMP-05。その場で登録する取り込みでも）と、JSON のコマンドの形。

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use common::{Env, all_items, by_name, exif, hashes, make_dng, wait_done, wait_ok, write_jpeg};
use genzo_api::{
    Command, CopyImportStage, CopySettings, Core, ErrorKind, FolderRule, ImportMetadata,
    ImportOptions, ImportReport, JobResult, JobState, RenameRule, Response,
};
use genzo_model::{FileRole, MetadataField};

/// 設定 `options` で取り込んで、結果を返す。
fn import_with(core: &Core, path: &Path, options: &ImportOptions) -> ImportReport {
    let job = core.import_folder_with(path, true, options).unwrap();
    match wait_ok(core, job) {
        JobResult::Import(r) => r,
        other => panic!("{other:?}"),
    }
}

/// コピー取り込みの設定。
fn copy_to(dest: &Path, rule: FolderRule, rename: Option<RenameRule>) -> ImportOptions {
    ImportOptions {
        copy: Some(CopySettings {
            destination: dest.to_path_buf(),
            folder_rule: rule,
            rename,
        }),
        ..ImportOptions::default()
    }
}

/// `root` の下のファイルの相対パス（'/' 区切り。名前の順）。
fn files_under(root: &Path) -> Vec<String> {
    if !root.exists() {
        return Vec::new();
    }
    hashes(root)
        .keys()
        .map(|p| {
            p.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

/// `root` の下のフォルダの相対パス（空のものを含む）。
fn dirs_under(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                out.push(
                    e.path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
                stack.push(e.path());
            }
        }
    }
    out.sort();
    out
}

/// ファイルの更新日時を変える。
fn set_mtime(path: &Path, t: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

/// 登録されている写真のファイルの場所（主のファイルと付属のファイル。名前の順）。
fn registered_paths(core: &Core) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for item in all_items(core) {
        for f in core.variant_details(item.variant_id).unwrap().files {
            out.push(f.path.unwrap());
        }
    }
    out.sort();
    out
}

/// SD カードの見立て: 撮影日の違う JPEG 3 枚（うち 1 枚は「RAW」と組）。RAW は LibRaw なしでも読める必要が
/// ないので、中身は適当なバイト列（メタデータを読めず、`status = error` で登録される）。
fn make_card(card: &Path) {
    let dir = card.join("DCIM/100MSDCF");
    write_jpeg(
        &dir.join("DSC00001.JPG"),
        (64, 48),
        1,
        Some(&exif(Some("2026-10-13 14:30:52"), Some(540), None)),
    );
    std::fs::write(dir.join("DSC00001.ARW"), b"not really a raw file 00001").unwrap();
    write_jpeg(
        &dir.join("DSC00002.JPG"),
        (64, 48),
        2,
        Some(&exif(Some("2026-10-13 14:31:00"), Some(540), None)),
    );
    write_jpeg(
        &card.join("DCIM/101MSDCF/DSC00003.JPG"),
        (64, 48),
        3,
        Some(&exif(Some("2026-10-14 09:05:00"), Some(540), None)),
    );
}

/// 日付別のフォルダ（YYYY/YYYY-MM-DD）とリネームの規則でコピーし、コピー先のファイルを登録する。RAW と JPEG の
/// 組は同じ名前で 1 枚の写真になり、連番は組で 1 つ進む。内容は元と同じで、全体のハッシュを記録する。元の
/// ファイルは変えない（DATA-01）。キーワードと作成者を新しい写真に付ける（IMP-05）。一時ファイルは残らない。
#[test]
fn copy_import_by_date_with_rename_pairs_and_metadata() {
    let env = Env::new();
    let card = env.dir.path().join("card");
    make_card(&card);
    let dest = env.photos.join("library");
    std::fs::create_dir_all(&dest).unwrap();
    let before = hashes(&card);
    let core = env.open();
    let mut options = copy_to(
        &dest,
        FolderRule::YearDate,
        Some(RenameRule {
            template: "{date}_{text}-{seq}".to_owned(),
            custom_text: "京都".to_owned(),
            sequence_start: 7,
            sequence_digits: 3,
        }),
    );
    options.metadata = ImportMetadata {
        keywords: vec![
            vec!["場所".to_owned(), "京都".to_owned()],
            vec!["家族".to_owned()],
        ],
        fields: BTreeMap::from([
            (MetadataField::Creator, "岡本".to_owned()),
            (MetadataField::Copyright, "© 2026 岡本".to_owned()),
        ]),
    };
    let r = import_with(&core, &card, &options);
    assert_eq!(r.files_found, 4, "{r:?}");
    assert_eq!(r.added, 4);
    assert_eq!(r.paired, 1, "RAW と JPEG の組");
    assert_eq!(r.tagged, 3, "写真は 3 枚（組は 1 枚）");
    assert!(r.not_registered.is_empty(), "{:?}", r.not_registered);
    let copy = r.copy.as_deref().unwrap();
    assert!(!copy.dry_run);
    assert_eq!(copy.destination, dest);
    assert_eq!(copy.copied.len(), 4);
    assert!(copy.copied.iter().all(|c| !c.existing));
    let expected_bytes: u64 = before
        .keys()
        .map(|p| std::fs::metadata(p).unwrap().len())
        .sum();
    assert_eq!(copy.bytes_copied, expected_bytes);
    // 組の RAW は撮影日時を読めないが、組の JPEG の撮影日時で日付を決める。
    assert_eq!(copy.dated_by_file_time, 0);
    // 次の連番（7・8・9 を使った）。
    assert_eq!(copy.next_sequence, 10);
    assert_eq!(
        files_under(&dest),
        vec![
            "2026/2026-10-13/20261013_京都-007.ARW",
            "2026/2026-10-13/20261013_京都-007.JPG",
            "2026/2026-10-13/20261013_京都-008.JPG",
            "2026/2026-10-14/20261014_京都-009.JPG",
        ]
    );
    // 内容は元と同じ。
    for c in &copy.copied {
        assert_eq!(
            genzo_catalog::full_hash(&c.destination).unwrap(),
            before[&c.source],
            "{c:?}"
        );
    }
    // 元のファイルは変わらない（DATA-01）。
    assert_eq!(hashes(&card), before);
    // 登録されているのはコピー先のファイルだけ。
    let registered = registered_paths(&core);
    assert_eq!(registered.len(), 4);
    assert!(
        registered.iter().all(|p| p.starts_with(&dest)),
        "{registered:?}"
    );
    // 組（RAW が主、JPEG は付属）・キーワード・作成者・全体のハッシュ。
    let items = all_items(&core);
    assert_eq!(items.len(), 3);
    let pair = by_name(&items, "20261013_京都-007.ARW");
    let details = core.variant_details(pair.variant_id).unwrap();
    assert_eq!(details.files.len(), 2);
    assert_eq!(details.files[1].role, FileRole::SidecarJpeg);
    assert!(details.files.iter().all(|f| f.full_hash.is_some()));
    assert_eq!(details.metadata.creator.as_deref(), Some("岡本"));
    assert_eq!(details.metadata.copyright.as_deref(), Some("© 2026 岡本"));
    let kw = core
        .variant_keywords(&items.iter().map(|i| i.variant_id).collect::<Vec<_>>())
        .unwrap();
    let mut paths: Vec<(Vec<String>, u64)> = kw
        .keywords
        .iter()
        .map(|k| (k.path.clone(), k.count))
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            (vec!["場所".to_owned(), "京都".to_owned()], 3),
            (vec!["家族".to_owned()], 3),
        ]
    );
    // 更新日時は元と同じ。
    for c in &copy.copied {
        assert_eq!(
            std::fs::metadata(&c.destination)
                .unwrap()
                .modified()
                .unwrap(),
            std::fs::metadata(&c.source).unwrap().modified().unwrap()
        );
    }
    // 一時ファイルは残らない。
    assert!(
        files_under(&dest)
            .iter()
            .all(|n| !n.contains(".genzo-import-"))
    );

    // 同じカードをもう一度取り込むと、重複と思われるもの（既定で取り込まない）として何もコピーしない。
    let again = import_with(&core, &card, &copy_to(&dest, FolderRule::YearDate, None));
    assert_eq!(again.added, 0);
    assert_eq!(again.duplicates_skipped(), 4, "{again:?}");
    assert!(again.copy.as_deref().unwrap().copied.is_empty());
    assert_eq!(files_under(&dest).len(), 4);
    core.close().unwrap();
}

/// 名前の衝突: コピー先に同じ名前で内容の違うファイルがあれば `-1` を付け（上書きしない）、同じ名前で同じ内容の
/// 登録していないファイルがあれば、コピーせずにそれを登録する（前の取り込みが登録の前に終わった場合の続き）。
/// カタログに登録済みの名前も使わない。
#[test]
fn name_collisions_are_avoided_and_identical_files_are_adopted() {
    let env = Env::new();
    let card = env.dir.path().join("card");
    make_card(&card);
    let dest = env.photos.join("library");
    let day = dest.join("2026-10-13");
    std::fs::create_dir_all(&day).unwrap();
    // 内容の違う同じ名前のファイル（上書きしない）。
    std::fs::write(day.join("DSC00002.JPG"), b"someone else's file").unwrap();
    // 同じ内容のファイル（前の取り込みの途中で公開したもの）。
    std::fs::copy(
        card.join("DCIM/100MSDCF/DSC00001.JPG"),
        day.join("DSC00001.JPG"),
    )
    .unwrap();
    std::fs::copy(
        card.join("DCIM/100MSDCF/DSC00001.ARW"),
        day.join("DSC00001.ARW"),
    )
    .unwrap();
    let core = env.open();
    let r = import_with(&core, &card, &copy_to(&dest, FolderRule::Date, None));
    assert!(r.not_registered.is_empty(), "{:?}", r.not_registered);
    let copy = r.copy.as_deref().unwrap();
    let mut got: Vec<(String, bool)> = copy
        .copied
        .iter()
        .map(|c| {
            (
                c.destination
                    .strip_prefix(&dest)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                c.existing,
            )
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("2026-10-13/DSC00001.ARW".to_owned(), true),
            ("2026-10-13/DSC00001.JPG".to_owned(), true),
            ("2026-10-13/DSC00002-1.JPG".to_owned(), false),
            ("2026-10-14/DSC00003.JPG".to_owned(), false),
        ]
    );
    assert_eq!(
        std::fs::read(day.join("DSC00002.JPG")).unwrap(),
        b"someone else's file"
    );
    // 既にあった 2 つはコピーしていない。
    let copied_bytes: u64 = [
        card.join("DCIM/100MSDCF/DSC00002.JPG"),
        card.join("DCIM/101MSDCF/DSC00003.JPG"),
    ]
    .iter()
    .map(|p| std::fs::metadata(p).unwrap().len())
    .sum();
    assert_eq!(copy.bytes_copied, copied_bytes);
    assert_eq!(r.added, 4);

    // 登録済みの名前は使わない: 同じ内容のファイルを別のカードから、取り込む設定で取り込むと、-1 を付ける。
    let card2 = env.dir.path().join("card2");
    std::fs::create_dir_all(&card2).unwrap();
    std::fs::copy(
        card.join("DCIM/101MSDCF/DSC00003.JPG"),
        card2.join("DSC00003.JPG"),
    )
    .unwrap();
    let mut options = copy_to(&dest, FolderRule::Date, None);
    options.skip_duplicates = Some(false);
    let r2 = import_with(&core, &card2, &options);
    assert_eq!(r2.added, 1);
    assert_eq!(r2.duplicates.len(), 1);
    assert!(!r2.duplicates[0].skipped);
    assert_eq!(
        r2.copy.as_deref().unwrap().copied[0].destination,
        dest.join("2026-10-14/DSC00003-1.JPG")
    );
    core.close().unwrap();
}

/// 拡張子を除いた名前の衝突: 別の写真の RAW と JPEG は、コピー先の同じフォルダに同じ名前（拡張子だけが違う
/// 名前）で置かない（置くと、登録で 1 枚の写真にまとめられ、削除で両方が対象になる。IMP-06）。同じ取り込みの
/// 中（別のフォルダの RAW と JPEG を 1 つのフォルダへ）でも、別の取り込みの後（登録済みの RAW の隣へ JPEG）でも、
/// ほかの写真の組の RAW（拡張子の違う RAW）があるときでも、`-1` を付ける。
#[test]
fn files_of_different_photos_do_not_share_a_stem() {
    let env = Env::new();
    let dest = env.photos.join("single");
    std::fs::create_dir_all(&dest).unwrap();
    let core = env.open();
    let single = || copy_to(&dest, FolderRule::Single, None);

    // 同じ取り込み: 別のフォルダの DSC00001.ARW と DSC00001.JPG（別の写真）。RAW は撮影日時を読めないので、
    // 更新日時（JPEG より前）の順で先に置く。
    let card = env.dir.path().join("cardC");
    std::fs::create_dir_all(card.join("100")).unwrap();
    std::fs::write(card.join("100/DSC00001.ARW"), b"raw of photo A").unwrap();
    set_mtime(
        &card.join("100/DSC00001.ARW"),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000),
    );
    for (name, n, when) in [
        ("DSC00001.JPG", 1, "2026-10-13 10:00:00"),
        ("DSC00002.JPG", 2, "2026-10-13 10:00:01"),
    ] {
        write_jpeg(
            &card.join("101").join(name),
            (64, 48),
            n,
            Some(&exif(Some(when), Some(540), None)),
        );
    }
    let r = import_with(&core, &card, &single());
    assert!(r.not_registered.is_empty(), "{:?}", r.not_registered);
    assert_eq!(r.added, 3);
    assert_eq!(r.paired, 0, "別の写真の RAW と JPEG をまとめない");
    assert_eq!(all_items(&core).len(), 3);
    assert_eq!(
        files_under(&dest),
        vec!["DSC00001-1.JPG", "DSC00001.ARW", "DSC00002.JPG"]
    );

    // 別の取り込み: 登録済みの RAW（photo A のもの）の隣へ、別のカードの DSC00001.JPG（`-1` も使われている）。
    let card_b = env.dir.path().join("cardB");
    write_jpeg(
        &card_b.join("DSC00001.JPG"),
        (64, 48),
        9,
        Some(&exif(Some("2026-10-13 11:00:00"), Some(540), None)),
    );
    let r = import_with(&core, &card_b, &single());
    assert_eq!(r.added, 1);
    assert_eq!(r.paired, 0, "登録済みの別の写真の RAW と組にしない");
    assert_eq!(all_items(&core).len(), 4);
    assert_eq!(
        r.copy.as_deref().unwrap().copied[0].destination,
        dest.join("DSC00001-2.JPG")
    );
    // 逆の順（登録済みの JPEG の隣へ、別の写真の RAW）。
    let card_r = env.dir.path().join("cardR");
    std::fs::create_dir_all(&card_r).unwrap();
    std::fs::write(card_r.join("DSC00002.ARW"), b"raw of photo R").unwrap();
    let r = import_with(&core, &card_r, &single());
    assert_eq!(r.added, 1);
    assert_eq!(r.paired, 0);
    assert_eq!(
        r.copy.as_deref().unwrap().copied[0].destination,
        dest.join("DSC00002-1.ARW")
    );
    assert_eq!(all_items(&core).len(), 5);

    // ほかの写真の RAW（拡張子の違う RAW。DNG）があるフォルダへ、RAW と JPEG の組をコピーする（組の JPEG が、
    // 先に登録された別の RAW と組にならない）。
    let dest2 = env.photos.join("single2");
    std::fs::create_dir_all(&dest2).unwrap();
    let card_d = env.dir.path().join("cardD");
    std::fs::create_dir_all(&card_d).unwrap();
    std::fs::write(card_d.join("IMG_0001.DNG"), b"raw of photo N").unwrap();
    let r = import_with(&core, &card_d, &copy_to(&dest2, FolderRule::Single, None));
    assert_eq!(r.added, 1);
    let card_e = env.dir.path().join("cardE");
    std::fs::create_dir_all(&card_e).unwrap();
    std::fs::write(card_e.join("IMG_0001.ARW"), b"raw of photo E").unwrap();
    write_jpeg(
        &card_e.join("IMG_0001.JPG"),
        (64, 48),
        5,
        Some(&exif(Some("2026-10-13 12:00:00"), Some(540), None)),
    );
    let r = import_with(&core, &card_e, &copy_to(&dest2, FolderRule::Single, None));
    assert_eq!(r.added, 2);
    assert_eq!(r.paired, 1, "組の RAW と JPEG は 1 枚");
    assert_eq!(
        files_under(&dest2),
        vec!["IMG_0001-1.ARW", "IMG_0001-1.JPG", "IMG_0001.DNG"]
    );
    let n_items = all_items(&core);
    assert_eq!(n_items.len(), 7);
    for item in n_items {
        let files = core.variant_details(item.variant_id).unwrap().files;
        let stems: std::collections::BTreeSet<String> = files
            .iter()
            .map(|f| {
                let name = f.path.as_ref().unwrap().file_name().unwrap();
                let name = name.to_string_lossy();
                name.rsplit_once('.').unwrap().0.to_lowercase()
            })
            .collect();
        assert_eq!(stems.len(), 1, "1 枚の写真のファイルは同じ名前: {files:?}");
    }
    core.close().unwrap();
}

/// 名前を決めるだけの確認（dry_run）: 予定を返し、ファイルもカタログも変えない。
#[test]
fn dry_run_plans_names_without_copying() {
    let env = Env::new();
    let card = env.dir.path().join("card");
    make_card(&card);
    let dest = env.photos.join("library");
    std::fs::create_dir_all(&dest).unwrap();
    let core = env.open();
    let mut options = copy_to(
        &dest,
        FolderRule::YearMonth,
        Some(RenameRule {
            template: "{year}{month}{day}-{hour}{minute}{second}".to_owned(),
            ..RenameRule::default()
        }),
    );
    options.dry_run = true;
    let r = import_with(&core, &card, &options);
    let copy = r.copy.as_deref().unwrap();
    assert!(copy.dry_run);
    let mut planned: Vec<String> = copy
        .copied
        .iter()
        .map(|c| {
            c.destination
                .strip_prefix(&dest)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    planned.sort();
    assert_eq!(
        planned,
        vec![
            "2026/2026-10/20261013-143052.ARW",
            "2026/2026-10/20261013-143052.JPG",
            "2026/2026-10/20261013-143100.JPG",
            "2026/2026-10/20261014-090500.JPG",
        ]
    );
    assert_eq!(r.added, 0);
    assert!(files_under(&dest).is_empty());
    assert!(dirs_under(&dest).is_empty());
    assert!(all_items(&core).is_empty());
    // コピーしない取り込みでは使えない。
    let e = core
        .import_folder_with(
            &card,
            true,
            &ImportOptions {
                dry_run: true,
                ..ImportOptions::default()
            },
        )
        .unwrap_err();
    assert_eq!(e.info().kind, ErrorKind::InvalidInput);
    core.close().unwrap();
}

/// 一時ファイルの内容が元と一致しなければ（書き込みの誤り）、公開しない。組のほかのファイルも公開しない。
/// 一時ファイル・作ったフォルダは残らない。
#[test]
fn a_copy_that_does_not_match_is_not_published() {
    let env = Env::new();
    let card = env.dir.path().join("card");
    make_card(&card);
    let dest = env.photos.join("library");
    std::fs::create_dir_all(&dest).unwrap();
    // 最初に書く一時ファイル（撮影日時の順で最初の組の主のファイル DSC00001.ARW）を書き換える。
    let written = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let core = env.open_with_hooks(|h| {
        let written = Arc::clone(&written);
        h.on_copy_import = Some(Arc::new(move |stage: &CopyImportStage| {
            if let CopyImportStage::Written(tmp) = stage
                && written.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0
            {
                let mut bytes = std::fs::read(tmp).unwrap();
                bytes[10] ^= 0xFF;
                std::fs::write(tmp, bytes).unwrap();
            }
            Ok(())
        }));
    });
    let r = import_with(&core, &card, &copy_to(&dest, FolderRule::Single, None));
    // 書き換えたファイルは公開せず、理由を返す。
    let failed: Vec<&genzo_api::FileIssue> = r.not_registered.iter().collect();
    let mut failed_names: Vec<String> = failed
        .iter()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    failed_names.sort();
    assert_eq!(
        failed_names,
        vec!["DSC00001.ARW", "DSC00001.JPG"],
        "組ごと公開しない"
    );
    assert!(
        failed.iter().all(|f| f.reason.contains("一致しません")),
        "{failed:?}"
    );
    let names = files_under(&dest);
    for f in &failed {
        let n = f.path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!names.contains(&n), "{n} は公開しない: {names:?}");
    }
    assert!(
        names.iter().all(|n| !n.contains(".genzo-import-")),
        "{names:?}"
    );
    // 登録したものと、コピー先のファイルは食い違わない。
    let registered: Vec<PathBuf> = registered_paths(&core);
    let on_disk: Vec<PathBuf> = names.iter().map(|n| dest.join(n)).collect();
    assert_eq!(registered, on_disk);
    core.close().unwrap();
}

/// 組の途中で失敗したら、その組で公開したファイルを消す（RAW と JPEG の片方だけを残さない）。ほかの組は
/// コピーして登録する。失敗した組のためだけに作ったフォルダは消す。
#[test]
fn a_failure_inside_a_group_removes_the_published_files_of_the_group() {
    let env = Env::new();
    let card = env.dir.path().join("card");
    make_card(&card);
    let dest = env.photos.join("library");
    std::fs::create_dir_all(&dest).unwrap();
    let before = hashes(&card);
    let core = env.open_with_hooks(|h| {
        h.on_copy_import = Some(Arc::new(|stage: &CopyImportStage| match stage {
            // 組の 2 つ目（JPEG）を公開した直後に失敗させる（RAW は先に公開済み）。
            CopyImportStage::Published(p) if p.ends_with("DSC00001.JPG") => {
                Err("テストの失敗".to_owned())
            }
            // 2026-10-14 の写真の公開の後にも失敗させる（その日のフォルダは空になるので消える）。
            CopyImportStage::Published(p) if p.ends_with("DSC00003.JPG") => {
                Err("テストの失敗".to_owned())
            }
            _ => Ok(()),
        }));
    });
    let r = import_with(&core, &card, &copy_to(&dest, FolderRule::YearDate, None));
    let mut failed: Vec<String> = r
        .not_registered
        .iter()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    failed.sort();
    assert_eq!(failed, vec!["DSC00001.ARW", "DSC00001.JPG", "DSC00003.JPG"]);
    assert!(r.not_registered.iter().all(|f| f.reason == "テストの失敗"));
    assert_eq!(files_under(&dest), vec!["2026/2026-10-13/DSC00002.JPG"]);
    assert_eq!(dirs_under(&dest), vec!["2026", "2026/2026-10-13"]);
    assert_eq!(
        registered_paths(&core),
        vec![dest.join("2026/2026-10-13/DSC00002.JPG")]
    );
    assert_eq!(hashes(&card), before);
    core.close().unwrap();
}

/// カタログへの登録に失敗したら、そのバッチでコピーしたファイルを消してからジョブを失敗にする（作ったフォルダも
/// 消す）。元のファイルは変えない。
#[test]
fn a_registration_failure_removes_the_copies_of_the_batch() {
    let env = Env::new();
    let card = env.dir.path().join("card");
    make_card(&card);
    let dest = env.photos.join("library");
    std::fs::create_dir_all(&dest).unwrap();
    let before = hashes(&card);
    let core = env.open_with_hooks(|h| {
        h.on_copy_import = Some(Arc::new(|stage: &CopyImportStage| match stage {
            CopyImportStage::Registering(_) => Err("登録の失敗（テスト）".to_owned()),
            _ => Ok(()),
        }));
    });
    let rx = core.subscribe();
    let job = core
        .import_folder_with(&card, true, &copy_to(&dest, FolderRule::YearDate, None))
        .unwrap();
    let info = wait_done(&core, job);
    assert_eq!(info.state, JobState::Failed, "{info:?}");
    assert!(info.error.unwrap().message.contains("登録の失敗"));
    assert!(files_under(&dest).is_empty(), "{:?}", files_under(&dest));
    assert!(dirs_under(&dest).is_empty(), "{:?}", dirs_under(&dest));
    assert!(all_items(&core).is_empty());
    assert_eq!(hashes(&card), before);
    let warned = common::drain(&rx).into_iter().any(|e| {
        matches!(
            e,
            genzo_api::Event::Warning {
                code: genzo_api::WarningCode::ImportCopyRolledBack,
                ..
            }
        )
    });
    assert!(warned);
    core.close().unwrap();
}

/// カタログへの登録（コミット）の後の失敗では、登録済みのバッチのコピーを消さない（カタログに行が残り、ファイルが
/// ない状態にしない）。途中のバッチ（[`genzo_api::IMPORT_BATCH_FILES`] 件ごと）でも、最後のバッチでも。ジョブは
/// 失敗にし、残りはコピーしない。
#[test]
fn a_failure_after_registration_keeps_the_registered_copies() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let env = Env::new();
    let card = env.dir.path().join("card");
    let n_files = genzo_api::IMPORT_BATCH_FILES + 2;
    for i in 0..n_files {
        write_jpeg(
            &card.join(format!("IMG_{i:04}.JPG")),
            (16, 16),
            (i % 250) as u8,
            Some(&exif(
                Some(&format!("2026-10-13 10:{:02}:{:02}", i / 60, i % 60)),
                Some(540),
                None,
            )),
        );
    }
    let dest = env.photos.join("library");
    std::fs::create_dir_all(&dest).unwrap();
    let before = hashes(&card);
    let fail = Arc::new(AtomicBool::new(true));
    let fail_in_hook = Arc::clone(&fail);
    let core = env.open_with_hooks(move |h| {
        h.on_copy_import = Some(Arc::new(move |stage: &CopyImportStage| match stage {
            CopyImportStage::Registered(_) if fail_in_hook.swap(false, Ordering::SeqCst) => {
                Err("登録の後の失敗（テスト）".to_owned())
            }
            _ => Ok(()),
        }));
    });
    let rx = core.subscribe();
    let job = core
        .import_folder_with(&card, true, &copy_to(&dest, FolderRule::Single, None))
        .unwrap();
    let info = wait_done(&core, job);
    assert_eq!(info.state, JobState::Failed, "{info:?}");
    assert!(info.error.unwrap().message.contains("登録の後の失敗"));
    // 最初のバッチは登録済みで、コピーも残る（残りの 2 件はコピーしない）。
    let registered = registered_paths(&core);
    assert_eq!(registered.len(), genzo_api::IMPORT_BATCH_FILES);
    let mut on_disk: Vec<PathBuf> = files_under(&dest).iter().map(|n| dest.join(n)).collect();
    on_disk.sort();
    assert_eq!(on_disk, registered);
    assert!(!common::drain(&rx).into_iter().any(|e| matches!(
        e,
        genzo_api::Event::Warning {
            code: genzo_api::WarningCode::ImportCopyRolledBack,
            ..
        }
    )));

    // 最後のバッチ（残りの 2 件）で失敗しても、登録したコピーは残る。
    fail.store(true, Ordering::SeqCst);
    let job = core
        .import_folder_with(&card, true, &copy_to(&dest, FolderRule::Single, None))
        .unwrap();
    assert_eq!(wait_done(&core, job).state, JobState::Failed);
    let registered = registered_paths(&core);
    assert_eq!(registered.len(), n_files);
    let mut on_disk: Vec<PathBuf> = files_under(&dest).iter().map(|n| dest.join(n)).collect();
    on_disk.sort();
    assert_eq!(on_disk, registered);
    assert_eq!(hashes(&card), before, "元のファイルは変えない");
    core.close().unwrap();
}

/// 取り消し: 公開の途中で取り消すと、そこまでの完全な組を登録してから止め、登録したものとコピー先のファイルが
/// 食い違わない。もう一度取り込むと、残りをコピーする（コピー済みのものは重複として飛ばす）。
#[test]
fn cancelling_keeps_complete_groups_registered() {
    let env = Env::new();
    let card = env.dir.path().join("card");
    make_card(&card);
    let dest = env.photos.join("library");
    std::fs::create_dir_all(&dest).unwrap();
    let (reached_tx, reached_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let go_rx = parking_lot::Mutex::new(go_rx);
    let reached_tx = parking_lot::Mutex::new(reached_tx);
    let core = env.open_with_hooks(|h| {
        h.on_copy_import = Some(Arc::new(move |stage: &CopyImportStage| {
            if let CopyImportStage::Published(p) = stage
                && p.ends_with("DSC00002.JPG")
            {
                // テストが取り消すまで待つ。
                reached_tx.lock().send(()).unwrap();
                go_rx.lock().recv_timeout(Duration::from_secs(60)).unwrap();
            }
            Ok(())
        }));
    });
    let job = core
        .import_folder_with(&card, true, &copy_to(&dest, FolderRule::Single, None))
        .unwrap();
    reached_rx.recv_timeout(Duration::from_secs(120)).unwrap();
    core.cancel_job(job).unwrap();
    go_tx.send(()).unwrap();
    let info = wait_done(&core, job);
    let Some(JobResult::Import(r)) = info.result else {
        panic!("{info:?}");
    };
    assert!(r.cancelled);
    // DSC00001 の組と DSC00002 はコピーし終えて登録した。DSC00003 はコピーしていない。
    let names = files_under(&dest);
    assert_eq!(names, vec!["DSC00001.ARW", "DSC00001.JPG", "DSC00002.JPG"]);
    let on_disk: Vec<PathBuf> = names.iter().map(|n| dest.join(n)).collect();
    assert_eq!(registered_paths(&core), on_disk);
    // 続き。
    let r2 = import_with(&core, &card, &copy_to(&dest, FolderRule::Single, None));
    assert_eq!(r2.added, 1);
    assert_eq!(r2.duplicates_skipped(), 3);
    assert_eq!(files_under(&dest).len(), 4);
    core.close().unwrap();
}

/// 撮影日時の分からないファイルは、ファイルの更新日時（既定のタイムゾーン）で日付を決め、数を返す。同じ取り込みの
/// 中の同じ内容のファイルは、2 つ目をコピーしない。
#[test]
fn files_without_capture_time_use_the_file_time() {
    let env = Env::new();
    let card = env.dir.path().join("card");
    std::fs::create_dir_all(card.join("a")).unwrap();
    std::fs::create_dir_all(card.join("b")).unwrap();
    // 撮影日時のない PNG（2026-03-04 05:06:07 JST の更新日時）。
    common::write_plain_image(&card.join("a/scan.png"), (32, 32), 9);
    let t = chrono::DateTime::parse_from_rfc3339("2026-03-04T05:06:07+09:00")
        .unwrap()
        .with_timezone(&chrono::Utc);
    set_mtime(&card.join("a/scan.png"), SystemTime::from(t));
    std::fs::copy(card.join("a/scan.png"), card.join("b/scan-copy.png")).unwrap();
    let dest = env.photos.join("library");
    std::fs::create_dir_all(&dest).unwrap();
    let core = env.open();
    let r = import_with(
        &core,
        &card,
        &copy_to(&dest, FolderRule::YearMonthDay, None),
    );
    assert_eq!(r.added, 1, "{r:?}");
    assert_eq!(r.duplicates.len(), 1);
    assert!(r.duplicates[0].skipped);
    assert_eq!(
        r.duplicates[0].existing_path.as_deref(),
        Some(dest.join("2026/03/04/scan.png").as_path())
    );
    assert!(r.duplicates[0].existing_variant_id.is_some());
    assert_eq!(r.copy.as_deref().unwrap().dated_by_file_time, 1);
    assert_eq!(files_under(&dest), vec!["2026/03/04/scan.png"]);
    core.close().unwrap();
}

/// 取り込むフォルダがコピー先の中にあっても（`写真/取り込み待ち` から `写真` へ）、すべて走査する。コピー先が
/// 取り込むフォルダの中にあれば、コピー先は走査しない。
#[test]
fn nested_source_and_destination_folders() {
    let env = Env::new();
    let card = env.photos.join("incoming");
    make_card(&card);
    let core = env.open();
    let r = import_with(&core, &card, &copy_to(&env.photos, FolderRule::Date, None));
    assert_eq!(r.added, 4, "{r:?}");
    assert!(env.photos.join("2026-10-13/DSC00002.JPG").is_file());
    // コピー先が取り込むフォルダの中: 2 回目の取り込みは、コピー先にできたものを数えない。
    let card2 = env.dir.path().join("card2");
    make_card(&card2);
    let inner_dest = card2.join("imported");
    std::fs::create_dir_all(&inner_dest).unwrap();
    let mut options = copy_to(&inner_dest, FolderRule::Single, None);
    options.skip_duplicates = Some(false);
    let r = import_with(&core, &card2, &options);
    assert_eq!(r.files_found, 4);
    let r = import_with(&core, &card2, &options);
    assert_eq!(r.files_found, 4, "コピー先の中は走査しない");
    core.close().unwrap();
}

/// 元のフォルダの構成のまま（`source`）と、LibRaw が有効なら実際の RAW（合成 DNG）と JPEG の組。
#[test]
fn source_folder_rule_keeps_the_layout() {
    let env = Env::new();
    let card = env.dir.path().join("card");
    make_card(&card);
    std::fs::create_dir_all(card.join("DCIM/102MSDCF")).unwrap();
    let with_dng = make_dng(
        &card.join("DCIM/102MSDCF/IMG_0001.DNG"),
        "2026:10:15 10:00:00",
    );
    if with_dng {
        write_jpeg(
            &card.join("DCIM/102MSDCF/IMG_0001.JPG"),
            (64, 48),
            4,
            Some(&exif(Some("2026-10-15 10:00:00"), Some(540), None)),
        );
    }
    let dest = env.photos.join("library");
    std::fs::create_dir_all(&dest).unwrap();
    let core = env.open();
    let r = import_with(&core, &card, &copy_to(&dest, FolderRule::Source, None));
    assert!(r.not_registered.is_empty(), "{:?}", r.not_registered);
    let mut expected = vec![
        "DCIM/100MSDCF/DSC00001.ARW",
        "DCIM/100MSDCF/DSC00001.JPG",
        "DCIM/100MSDCF/DSC00002.JPG",
        "DCIM/101MSDCF/DSC00003.JPG",
    ];
    if with_dng {
        expected.extend(["DCIM/102MSDCF/IMG_0001.DNG", "DCIM/102MSDCF/IMG_0001.JPG"]);
        let items = all_items(&core);
        let dng = by_name(&items, "IMG_0001.DNG");
        assert!(dng.has_sidecar_jpeg);
        assert_eq!(r.paired, 2);
    }
    assert_eq!(files_under(&dest), expected);
    core.close().unwrap();
}

/// その場で登録する取り込みでも、新しく登録した写真にだけキーワード・メタデータを付ける（IMP-05）。
#[test]
fn in_place_import_applies_metadata_to_new_photos_only() {
    let env = Env::new();
    let dir = env.photos.join("2026");
    write_jpeg(&dir.join("A.jpg"), (64, 48), 1, None);
    let core = env.open();
    common::import(&core, &dir);
    write_jpeg(&dir.join("B.jpg"), (64, 48), 2, None);
    let options = ImportOptions {
        metadata: ImportMetadata {
            keywords: vec![vec!["イベント".to_owned(), "運動会".to_owned()]],
            fields: BTreeMap::from([(MetadataField::Creator, "岡本".to_owned())]),
        },
        ..ImportOptions::default()
    };
    let r = import_with(&core, &dir, &options);
    assert_eq!((r.added, r.unchanged, r.tagged), (1, 1, 1));
    assert!(r.copy.is_none());
    let items = all_items(&core);
    let a = core
        .variant_details(by_name(&items, "A.jpg").variant_id)
        .unwrap();
    let b = core
        .variant_details(by_name(&items, "B.jpg").variant_id)
        .unwrap();
    assert_eq!(a.metadata.creator, None);
    assert_eq!(b.metadata.creator.as_deref(), Some("岡本"));
    let kb = core.variant_keywords(&[b.summary.variant_id]).unwrap();
    assert_eq!(kb.keywords.len(), 1);
    assert_eq!(kb.keywords[0].path, vec!["イベント", "運動会"]);
    assert!(
        core.variant_keywords(&[a.summary.variant_id])
            .unwrap()
            .keywords
            .is_empty()
    );
    core.close().unwrap();
}

/// 設定の誤りは、ジョブを始める前に返す（何もコピー・登録しない）。
#[test]
fn invalid_settings_are_rejected_before_starting() {
    let env = Env::new();
    let card = env.dir.path().join("card");
    make_card(&card);
    let dest = env.photos.join("library");
    std::fs::create_dir_all(&dest).unwrap();
    let file = env.photos.join("file.txt");
    std::fs::write(&file, b"x").unwrap();
    let core = env.open();
    let bad_template = copy_to(
        &dest,
        FolderRule::YearDate,
        Some(RenameRule {
            template: "{date}/{seq}".to_owned(),
            ..RenameRule::default()
        }),
    );
    let mut bad_keyword = copy_to(&dest, FolderRule::YearDate, None);
    bad_keyword.metadata.keywords = vec![vec!["場所>京都".to_owned()]];
    let mut bad_value = ImportOptions::default();
    bad_value
        .metadata
        .fields
        .insert(MetadataField::CopyrightStatus, "たぶん".to_owned());
    for (options, needle) in [
        (bad_template, "「/」"),
        (
            copy_to(&env.photos.join("ない"), FolderRule::Single, None),
            "ありません",
        ),
        (
            copy_to(&file, FolderRule::Single, None),
            "フォルダではありません",
        ),
        (
            copy_to(Path::new(""), FolderRule::Single, None),
            "指定してください",
        ),
        (copy_to(&card, FolderRule::Single, None), "同じフォルダ"),
        (bad_keyword, "「>」"),
        (bad_value, "取り込みで付けるもの"),
    ] {
        let e = core.import_folder_with(&card, true, &options).unwrap_err();
        assert_eq!(e.info().kind, ErrorKind::InvalidInput, "{e}");
        assert!(e.to_string().contains(needle), "{needle}: {e}");
    }
    assert!(files_under(&dest).is_empty());
    assert!(all_items(&core).is_empty());
    core.close().unwrap();
}

/// JSON のコマンドの形: コピー取り込み（省いた項目は既定）と、名前の例。
#[test]
fn json_commands_for_copy_import_and_name_preview() {
    let env = Env::new();
    let card = env.dir.path().join("card");
    make_card(&card);
    let dest = env.photos.join("library");
    std::fs::create_dir_all(&dest).unwrap();
    let core = env.open();
    let cmd: Command = serde_json::from_value(serde_json::json!({
        "type": "import_folder",
        "path": card,
        "recursive": true,
        "options": {
            "copy": {"destination": dest},
            "metadata": {"keywords": [["旅行"]], "fields": {"creator": "岡本"}}
        }
    }))
    .unwrap();
    let Response::JobStarted { job_id } = core.execute(cmd).unwrap() else {
        panic!()
    };
    let JobResult::Import(r) = wait_ok(&core, job_id) else {
        panic!()
    };
    assert_eq!(r.added, 4);
    // 既定のフォルダの規則は YYYY/YYYY-MM-DD、名前は元のまま。
    assert!(dest.join("2026/2026-10-13/DSC00002.JPG").is_file());
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["tagged"], 3);
    assert_eq!(json["copy"]["copied"].as_array().unwrap().len(), 4);

    let cmd: Command = serde_json::from_value(serde_json::json!({
        "type": "import_name_preview",
        "copy": {
            "destination": "",
            "folder_rule": "date_compact",
            "rename": {"template": "{camera}_{seq}", "sequence_digits": 2}
        }
    }))
    .unwrap();
    let Response::NamePreview(p) = core.execute(cmd).unwrap() else {
        panic!()
    };
    assert_eq!(p.error, None);
    assert_eq!(p.examples[0].destination, "20261013/ILCE-7M4_01.ARW");
    // カメラの分からない動画は、項目が空になる（テンプレートに書いた「_」は残る）。
    assert_eq!(p.examples[3].destination, "20261014/_03.MP4");
    core.close().unwrap();
}
