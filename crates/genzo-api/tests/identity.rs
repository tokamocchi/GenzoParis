//! v1 のパッケージ「identity」の結合テスト: ファイルの内容による同一性（ORG-03。内容のハッシュの
//! バックグラウンドの計算・明示的な計算・状況・重複の組・ファイルの識別・移行したカタログ）、取り込みでの
//! 重複の検出（IMP-04。登録済みのファイル・同じ取り込みの中・クイックハッシュだけが同じもの・見つからない
//! ファイル・取り込む設定）、RAW と JPEG のペア（IMP-06。1 枚にまとめる・別々に取り込む・設定の保存）と、
//! JSON のコマンドの形。

mod common;

use std::path::Path;

use common::{Env, all_items, by_name, hashes, import, make_dng, wait_ok, write_jpeg};
use genzo_api::{
    Command, Core, ImportOptions, ImportReport, JobKind, JobResult, Response, SettingsUpdate,
};
use genzo_catalog::FileFacts;
use genzo_model::FileRole;

/// 設定 `options` で取り込んで、結果を返す。
fn import_with(core: &Core, path: &Path, options: ImportOptions) -> ImportReport {
    let job = core.import_folder_with(path, true, &options).unwrap();
    match wait_ok(core, job) {
        JobResult::Import(r) => r,
        other => panic!("{other:?}"),
    }
}

/// カタログを開き直さずに、ファイルの全体のハッシュの記録を読む（`variant_details` の一覧）。
fn recorded_hashes(core: &Core, name: &str) -> Vec<(String, Option<String>)> {
    let items = all_items(core);
    let v = by_name(&items, name).variant_id;
    core.variant_details(v)
        .unwrap()
        .files
        .into_iter()
        .map(|f| {
            (
                f.path
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                f.full_hash,
            )
        })
        .collect()
}

/// クイックハッシュが読む先頭と末尾の 64KiB（合わせて 128KiB）より大きい、決まった列のバイト列。
fn large_bytes(seed: u64) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ seed;
    (0..300_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 32) as u8
        })
        .collect()
}

/// 別のフォルダにある同じ内容のファイルは、既定（重複と思われるものを取り込まない）では登録せず、結果に
/// 登録済みのファイルと写真を挙げる。内容の違うファイルは登録する。候補の全体のハッシュは記録する。
/// 元のファイルは変えない（DATA-01）。
#[test]
fn duplicates_in_another_folder_are_skipped_by_default() {
    let env = Env::new();
    let first = env.photos.join("2024");
    write_jpeg(&first.join("A.jpg"), (64, 48), 1, None);
    let core = env.open();
    let r = import(&core, &first);
    assert_eq!(r.added, 1);
    assert!(r.duplicates.is_empty());
    assert!(r.skip_duplicates, "既定は取り込まない");
    let a = by_name(&all_items(&core), "A.jpg").variant_id;
    // 候補がなかったので、全体のハッシュは求めていない。
    assert_eq!(recorded_hashes(&core, "A.jpg")[0].1, None);

    // 別のフォルダに、同じ内容の B.jpg と、内容の違う C.jpg。
    let copy = env.photos.join("コピー");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::copy(first.join("A.jpg"), copy.join("B.jpg")).unwrap();
    write_jpeg(&copy.join("C.jpg"), (64, 48), 2, None);
    let before = hashes(&env.photos);
    let r = import(&core, &copy);
    assert_eq!((r.files_found, r.added), (2, 1), "{r:?}");
    assert_eq!(r.duplicates.len(), 1);
    let d = &r.duplicates[0];
    assert_eq!(d.path, copy.join("B.jpg"));
    assert_eq!(
        d.existing_path.as_deref(),
        Some(first.join("A.jpg").as_path())
    );
    assert_eq!(d.existing_variant_id, Some(a));
    assert!(d.skipped);
    assert_eq!(r.duplicates_skipped(), 1);
    let items = all_items(&core);
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|i| i.file_name != "B.jpg"));
    // 候補（A.jpg）の全体のハッシュは記録した。候補のなかった C.jpg は求めていない。
    let expected = genzo_catalog::full_hash(&first.join("A.jpg")).unwrap();
    assert_eq!(
        recorded_hashes(&core, "A.jpg")[0].1.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(recorded_hashes(&core, "C.jpg")[0].1, None);
    assert_eq!(hashes(&env.photos), before);
    // もう一度取り込んでも同じ（冪等）。
    let again = import(&core, &copy);
    assert_eq!((again.added, again.unchanged), (0, 1));
    assert_eq!(again.duplicates.len(), 1);
    core.close().unwrap();
}

/// 取り込む設定では、同じ内容のファイルも別の写真として登録し、結果に挙げる。両方の全体のハッシュを
/// 記録するので、重複の組として読める。
#[test]
fn duplicates_are_registered_when_asked_and_listed_as_groups() {
    let env = Env::new();
    let first = env.photos.join("a");
    let second = env.photos.join("b");
    write_jpeg(&first.join("A.jpg"), (64, 48), 1, None);
    std::fs::create_dir_all(&second).unwrap();
    std::fs::copy(first.join("A.jpg"), second.join("A.jpg")).unwrap();
    let core = env.open();
    import(&core, &first);
    let r = import_with(
        &core,
        &second,
        ImportOptions {
            skip_duplicates: Some(false),
            ..Default::default()
        },
    );
    assert_eq!(r.added, 1);
    assert!(!r.skip_duplicates);
    assert_eq!(r.duplicates.len(), 1);
    assert!(!r.duplicates[0].skipped);
    assert_eq!(r.duplicates_skipped(), 0);
    assert_eq!(all_items(&core).len(), 2);
    let groups = core.duplicate_groups(10).unwrap();
    assert_eq!(groups.total, 1);
    let paths: Vec<_> = groups.groups[0]
        .files
        .iter()
        .map(|f| f.path.clone().unwrap())
        .collect();
    assert_eq!(paths, vec![first.join("A.jpg"), second.join("A.jpg")]);
    assert_eq!(
        groups.groups[0].full_hash,
        genzo_catalog::full_hash(&first.join("A.jpg")).unwrap()
    );
    let status = core.content_hash_status().unwrap();
    assert_eq!(
        (status.files, status.hashed, status.duplicate_groups),
        (2, 2, 1)
    );
    core.close().unwrap();
}

/// 同じ取り込みの中に同じ内容のファイルが 2 つあれば、2 つ目（走査の順）を重複とし、先に登録したファイルの
/// 写真を挙げる（同じバッチでも、登録の後に分かる）。
#[test]
fn the_same_content_twice_in_one_import_is_detected() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    std::fs::create_dir_all(env.photos.join("sub")).unwrap();
    std::fs::copy(env.photos.join("A.jpg"), env.photos.join("sub/A-copy.jpg")).unwrap();
    write_jpeg(&env.photos.join("B.jpg"), (64, 48), 2, None);
    let core = env.open();
    let r = import(&core, &env.photos);
    assert_eq!(r.added, 2, "{r:?}");
    assert_eq!(r.duplicates.len(), 1);
    let d = &r.duplicates[0];
    assert_eq!(d.path, env.photos.join("sub/A-copy.jpg"));
    assert_eq!(
        d.existing_path.as_deref(),
        Some(env.photos.join("A.jpg").as_path())
    );
    let a = by_name(&all_items(&core), "A.jpg").variant_id;
    assert_eq!(d.existing_variant_id, Some(a));
    // 比べるために求めた全体のハッシュは記録した。
    assert!(recorded_hashes(&core, "A.jpg")[0].1.is_some());
    core.close().unwrap();
}

/// 大きさと先頭・末尾（クイックハッシュ）が同じで、中間だけが違うファイルは重複ではない（全体のハッシュで
/// 確かめる。3.3 節）。比べた両方のファイルの全体のハッシュを記録する。
#[test]
fn files_differing_only_in_the_middle_are_not_duplicates() {
    let env = Env::new();
    let first = env.photos.join("a");
    let second = env.photos.join("b");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let bytes = large_bytes(1);
    std::fs::write(first.join("X.jpg"), &bytes).unwrap();
    let mut changed = bytes.clone();
    changed[bytes.len() / 2] ^= 0xff;
    std::fs::write(second.join("X.jpg"), &changed).unwrap();
    let qa = FileFacts::read(&first.join("X.jpg")).unwrap();
    let qb = FileFacts::read(&second.join("X.jpg")).unwrap();
    assert_eq!(qa.quick_hash, qb.quick_hash, "テストの前提");
    let core = env.open();
    import(&core, &first);
    let r = import(&core, &second);
    // JPEG としては読めない（status = error）が、登録はする。
    assert_eq!(r.added, 1, "{r:?}");
    assert!(r.duplicates.is_empty());
    let status = core.content_hash_status().unwrap();
    assert_eq!(
        (status.files, status.hashed, status.duplicate_groups),
        (2, 2, 0)
    );
    core.close().unwrap();
}

/// 見つからない（`missing`）ファイルは重複の候補にしない（アプリの外で移されたファイルの再リンク（FILE-04）の
/// 対象のため）。同じ内容のファイルを別の場所から取り込むと、別の写真として登録する。
#[test]
fn missing_files_are_not_duplicate_candidates() {
    let env = Env::new();
    let first = env.photos.join("a");
    write_jpeg(&first.join("A.jpg"), (64, 48), 1, None);
    let moved = env.dir.path().join("moved");
    std::fs::create_dir_all(&moved).unwrap();
    let core = env.open();
    import(&core, &first);
    // 全体のハッシュを記録してから、アプリの外で移す。
    let job = core.compute_content_hashes(false).unwrap();
    let JobResult::HashFiles(h) = wait_ok(&core, job) else {
        panic!("内容のハッシュの結果ではない");
    };
    assert_eq!(h.hashed, 1);
    std::fs::rename(first.join("A.jpg"), moved.join("A.jpg")).unwrap();
    let a = by_name(&all_items(&core), "A.jpg").variant_id;
    let check = core.check_files(&[a]).unwrap();
    assert_eq!(check.missing.len(), 1);
    let r = import(&core, &moved);
    assert_eq!(r.added, 1, "{r:?}");
    assert!(r.duplicates.is_empty());
    assert_eq!(all_items(&core).len(), 2);
    core.close().unwrap();
}

/// 同じフォルダで拡張子だけが違う RAW と JPEG は、既定では 1 枚の写真（RAW が主、JPEG は付属）にまとめ、
/// 結果に組にした数を出す。別々に取り込む設定では 2 枚にする。設定はカタログに保存され、省いた取り込みの
/// 設定はカタログの設定に従う。
#[test]
fn raw_and_jpeg_pairs_follow_the_import_setting() {
    let env = Env::new();
    let pair = env.photos.join("pair");
    std::fs::create_dir_all(&pair).unwrap();
    // LibRaw がなくても登録できる（メタデータを読めなければ status = error）。
    std::fs::write(pair.join("A.dng"), b"not really a dng").unwrap();
    write_jpeg(&pair.join("A.jpg"), (32, 24), 1, None);
    let core = env.open();
    let r = import(&core, &pair);
    assert_eq!((r.added, r.paired), (2, 1), "{r:?}");
    assert!(!r.separate_raw_jpeg);
    let items = all_items(&core);
    assert_eq!(items.len(), 1);
    let a = by_name(&items, "A.dng");
    assert!(a.has_sidecar_jpeg);
    let files = core.variant_details(a.variant_id).unwrap().files;
    assert_eq!(
        files.iter().map(|f| f.role).collect::<Vec<_>>(),
        vec![FileRole::Primary, FileRole::SidecarJpeg]
    );

    // 別々に取り込む設定を保存すると、次の取り込み（設定を省く）は別々にする。
    let saved = core
        .update_settings(&SettingsUpdate {
            import_separate_raw_jpeg: Some(true),
            import_skip_duplicates: Some(false),
            ..Default::default()
        })
        .unwrap();
    assert!(saved.import_separate_raw_jpeg && !saved.import_skip_duplicates);
    let separate = env.photos.join("separate");
    std::fs::create_dir_all(&separate).unwrap();
    std::fs::write(separate.join("B.dng"), b"another fake dng").unwrap();
    write_jpeg(&separate.join("B.jpg"), (32, 24), 2, None);
    let r = import(&core, &separate);
    assert_eq!((r.added, r.paired), (2, 0), "{r:?}");
    assert!(r.separate_raw_jpeg && !r.skip_duplicates);
    let items = all_items(&core);
    assert_eq!(items.len(), 3);
    assert!(!by_name(&items, "B.dng").has_sidecar_jpeg);
    assert!(!by_name(&items, "B.jpg").has_sidecar_jpeg);
    core.close().unwrap();
    // 保存した設定は、開き直しても残る。
    let core = env.open();
    let s = core.settings();
    assert!(s.import_separate_raw_jpeg && !s.import_skip_duplicates);
    assert!(s.hash_in_background && !s.hash_network_volumes);
    core.close().unwrap();
}

/// RAW と JPEG のペアの書き出しは RAW（主なファイル）から現像する（JPEG の寸法ではない）。LibRaw が
/// 無効な build では飛ばす。
#[test]
fn export_of_a_pair_uses_the_raw() {
    let env = Env::new();
    if !make_dng(&env.photos.join("A.dng"), "2024:05:01 12:00:00") {
        return;
    }
    write_jpeg(&env.photos.join("A.jpg"), (32, 24), 1, None);
    let core = env.open();
    let r = import(&core, &env.photos);
    assert_eq!(r.paired, 1);
    let items = all_items(&core);
    assert_eq!(items.len(), 1);
    let v = items[0].variant_id;
    let out = env.dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let job = core
        .export(&[v], &genzo_api::ExportSettings::default(), &out)
        .unwrap();
    let JobResult::Export(e) = wait_ok(&core, job) else {
        panic!("書き出しの結果ではない");
    };
    assert_eq!(e.written, 1, "{e:?}");
    let genzo_api::ExportOutcome::Written { path, .. } = &e.items[0].outcome else {
        panic!("{e:?}");
    };
    let img = image::open(path).unwrap();
    // 合成 DNG は 96 × 64（既定の切り抜きの後で、JPEG の 32 × 24 より大きい）。
    assert!(
        img.width() > 32 && img.height() > 24,
        "{}x{}",
        img.width(),
        img.height()
    );
    core.close().unwrap();
}

/// 内容のハッシュのバックグラウンドの計算: アプリが許した設定（`background_hashing`）では、取り込みの後に
/// すべてのファイルの全体のハッシュを記録する。止める設定では計算しない。
#[test]
fn content_hashes_are_computed_in_the_background_when_allowed() {
    let env = Env::new();
    for i in 0..3u8 {
        write_jpeg(&env.photos.join(format!("P{i}.jpg")), (48, 32), i, None);
    }
    let core = env.open_with(|c| c.background_hashing = true);
    import(&core, &env.photos);
    core.wait_idle();
    let status = core.content_hash_status().unwrap();
    assert_eq!(
        (status.files, status.hashed, status.pending),
        (3, 3, 0),
        "{status:?}"
    );
    assert_eq!(status.job_id, None);
    for i in 0..3u8 {
        let name = format!("P{i}.jpg");
        assert_eq!(
            recorded_hashes(&core, &name)[0].1,
            Some(genzo_catalog::full_hash(&env.photos.join(&name)).unwrap())
        );
    }
    // 止める設定では、新しく取り込んだファイルを計算しない。
    core.update_settings(&SettingsUpdate {
        hash_in_background: Some(false),
        ..Default::default()
    })
    .unwrap();
    write_jpeg(&env.photos.join("P9.jpg"), (48, 32), 9, None);
    import(&core, &env.photos);
    core.wait_idle();
    let status = core.content_hash_status().unwrap();
    assert_eq!((status.hashed, status.pending), (3, 1));
    core.close().unwrap();

    // アプリが許さない（CLI・既定）では、設定が有効でも自動では計算しない。明示的な計算は動く。
    let core = env.open();
    core.update_settings(&SettingsUpdate {
        hash_in_background: Some(true),
        ..Default::default()
    })
    .unwrap();
    core.wait_idle();
    assert_eq!(core.content_hash_status().unwrap().pending, 1);
    let job = core.compute_content_hashes(false).unwrap();
    let JobResult::HashFiles(r) = wait_ok(&core, job) else {
        panic!("内容のハッシュの結果ではない");
    };
    assert_eq!((r.hashed, r.failed_count, r.skipped_network), (1, 0, 0));
    assert_eq!(core.job(job).unwrap().kind, JobKind::HashFiles);
    assert_eq!(core.content_hash_status().unwrap().pending, 0);
    core.close().unwrap();
}

/// 計算の対象のファイルが見つからない（アプリの外で消された）・フォルダが見えない場合は、失敗に数えずに
/// 飛ばす。読めたファイルは記録する。
#[test]
fn hashing_skips_files_that_are_gone() {
    let env = Env::new();
    write_jpeg(&env.photos.join("a/A.jpg"), (48, 32), 1, None);
    write_jpeg(&env.photos.join("b/B.jpg"), (48, 32), 2, None);
    write_jpeg(&env.photos.join("b/C.jpg"), (48, 32), 3, None);
    let core = env.open();
    import(&core, &env.photos);
    std::fs::remove_dir_all(env.photos.join("a")).unwrap();
    std::fs::remove_file(env.photos.join("b/C.jpg")).unwrap();
    let job = core.compute_content_hashes(false).unwrap();
    let JobResult::HashFiles(r) = wait_ok(&core, job) else {
        panic!("内容のハッシュの結果ではない");
    };
    assert_eq!(
        (r.hashed, r.skipped_unavailable, r.failed_count),
        (1, 2, 0),
        "{r:?}"
    );
    core.close().unwrap();
}

/// 版 10 より前のカタログ（全体のハッシュのないファイル）を、バックグラウンドの計算を許したアプリで開くと、
/// 移行の後に既存のファイルの全体のハッシュを計算する。
#[test]
fn migrated_catalog_files_are_hashed_in_the_background() {
    let env = Env::new();
    let path = env.photos.join("A.jpg");
    write_jpeg(&path, (48, 32), 1, None);
    {
        std::fs::create_dir_all(env.catalog_path().parent().unwrap()).unwrap();
        let mut cat = genzo_catalog::testing::open_at_version(
            env.catalog_path(),
            genzo_catalog::CONTENT_IDENTITY_SCHEMA_VERSION - 1,
        )
        .unwrap();
        let abs = std::path::absolute(&path).unwrap();
        let parent = abs.parent().unwrap();
        let mut root = std::path::PathBuf::new();
        let mut comps = parent.components();
        // ボリュームはパスの先頭（Unix の `/`。genzo-api の paths の doc）。
        for c in comps.by_ref() {
            root.push(c.as_os_str());
            if matches!(c, std::path::Component::RootDir) {
                break;
            }
        }
        let rel = genzo_catalog::rel_path_from_path(comps.as_path()).unwrap();
        let mount = root.to_str().unwrap().to_owned();
        let vol = cat
            .ensure_volume(&format!("path:{mount}"), None, Some(&mount))
            .unwrap();
        let folder = cat.ensure_folder(vol, &rel).unwrap();
        cat.register_file(&genzo_catalog::RegisterFile {
            folder_id: folder,
            name: "A.jpg".to_owned(),
            facts: FileFacts::read(&path).unwrap(),
            kind: genzo_model::AssetKind::Photo,
            metadata: genzo_catalog::MediaMetadata::None,
            capture: genzo_model::CaptureTime::unknown(),
            error: None,
        })
        .unwrap();
        cat.close().unwrap();
    }
    let core = env.open_with(|c| c.background_hashing = true);
    assert_eq!(
        core.startup_report().migrated_from,
        Some(genzo_catalog::CONTENT_IDENTITY_SCHEMA_VERSION - 1)
    );
    core.wait_idle();
    let status = core.content_hash_status().unwrap();
    assert_eq!((status.files, status.hashed), (1, 1), "{status:?}");
    core.close().unwrap();
}

/// ファイルの内容で、場所・名前によらず登録済みの写真を探せる（アプリの外で名前を変えて移したファイル）。
/// 全体のハッシュのない候補は読んで記録する。
#[test]
fn files_are_identified_by_content() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    write_jpeg(&env.photos.join("B.jpg"), (64, 48), 2, None);
    let core = env.open();
    import(&core, &env.photos);
    let a = by_name(&all_items(&core), "A.jpg").variant_id;
    let elsewhere = env.dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::copy(env.photos.join("A.jpg"), elsewhere.join("renamed.jpg")).unwrap();
    let id = core.identify_file(&elsewhere.join("renamed.jpg")).unwrap();
    assert_eq!(
        id.full_hash,
        genzo_catalog::full_hash(&env.photos.join("A.jpg")).unwrap()
    );
    assert_eq!(id.matches.len(), 1);
    assert_eq!(id.matches[0].variant_id, a);
    assert_eq!(
        id.matches[0].path.as_deref(),
        Some(env.photos.join("A.jpg").as_path())
    );
    assert!(recorded_hashes(&core, "A.jpg")[0].1.is_some());
    // 内容の違うファイルは見つからない。
    write_jpeg(&elsewhere.join("other.jpg"), (64, 48), 7, None);
    assert!(
        core.identify_file(&elsewhere.join("other.jpg"))
            .unwrap()
            .matches
            .is_empty()
    );
    assert!(core.identify_file(&elsewhere.join("ない.jpg")).is_err());
    core.close().unwrap();
}

/// JSON のコマンド: 取り込みの設定（省くとカタログの設定）・内容のハッシュの計算と状況・重複の組・識別。
#[test]
fn identity_commands_through_json() {
    let env = Env::new();
    write_jpeg(&env.photos.join("a/A.jpg"), (48, 32), 1, None);
    std::fs::create_dir_all(env.photos.join("b")).unwrap();
    std::fs::copy(env.photos.join("a/A.jpg"), env.photos.join("b/A.jpg")).unwrap();
    let core = env.open();
    // 設定を省いた形（以前の形）も受け付ける。
    let c: Command = serde_json::from_value(serde_json::json!({
        "type": "import_folder", "path": env.photos.join("a"), "recursive": true
    }))
    .unwrap();
    let Response::JobStarted { job_id } = core.execute(c).unwrap() else {
        panic!("ジョブの応答ではない");
    };
    wait_ok(&core, job_id);
    let c: Command = serde_json::from_value(serde_json::json!({
        "type": "import_folder", "path": env.photos.join("b"), "recursive": true,
        "options": {"skip_duplicates": false}
    }))
    .unwrap();
    let Response::JobStarted { job_id } = core.execute(c).unwrap() else {
        panic!("ジョブの応答ではない");
    };
    let JobResult::Import(r) = wait_ok(&core, job_id) else {
        panic!("取り込みの結果ではない");
    };
    assert_eq!((r.added, r.duplicates.len()), (1, 1));
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["duplicates"][0]["skipped"], false);
    assert_eq!(json["skip_duplicates"], false);
    let Response::JobStarted { job_id } = core
        .execute(serde_json::from_str(r#"{"type": "compute_content_hashes"}"#).unwrap())
        .unwrap()
    else {
        panic!("ジョブの応答ではない");
    };
    wait_ok(&core, job_id);
    let Response::ContentHashStatus(s) = core
        .execute(serde_json::from_str(r#"{"type": "content_hash_status"}"#).unwrap())
        .unwrap()
    else {
        panic!("状況の応答ではない");
    };
    assert_eq!(
        (s.files, s.hashed, s.pending, s.duplicate_groups),
        (2, 2, 0, 1)
    );
    let Response::DuplicateGroups(g) = core
        .execute(serde_json::from_str(r#"{"type": "duplicate_groups"}"#).unwrap())
        .unwrap()
    else {
        panic!("重複の組の応答ではない");
    };
    assert_eq!((g.total, g.groups[0].files.len()), (1, 2));
    let c: Command = serde_json::from_value(serde_json::json!({
        "type": "identify_file", "path": env.photos.join("a/A.jpg")
    }))
    .unwrap();
    let Response::FileIdentity(id) = core.execute(c).unwrap() else {
        panic!("識別の応答ではない");
    };
    assert_eq!(id.matches.len(), 2);
    let v = serde_json::to_value(core.settings()).unwrap();
    assert_eq!(v["import_skip_duplicates"], true);
    assert_eq!(v["hash_network_volumes"], false);
    core.close().unwrap();
}
