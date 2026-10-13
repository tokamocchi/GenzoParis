//! アプリの中でのファイル操作（FILE-02）・外付けドライブの識別（FILE-03）・見つからない写真の検出と再リンク
//! （FILE-04）の結合テスト。04 の 6.4 節・3.14 節。
//!
//! 外付けドライブの付け外し・マウント先の変化は、OS のボリューム ID の問い合わせを偽物（[`FakeVolumes`]）に
//! 差し替えて確かめる（一時フォルダの中のフォルダを「ドライブ」とみなす）。

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{Env, all_items, by_name, exif, hashes, import, wait_ok, write_jpeg};
use genzo_api::{
    Event, FileOpStage, JobResult, MoveMethod, RelinkMatch, RelinkRequest, RenameRule, RenameSpec,
    VolumeIdentity, VolumeProbe, WarningCode,
};
use genzo_model::{FileStatus, VariantId};
use parking_lot::Mutex;

/// 偽のボリューム: 登録した「ドライブ」（ID とマウント先）の中のパスを、そのボリュームとする。
#[derive(Default)]
struct FakeVolumes {
    drives: Mutex<Vec<(String, PathBuf)>>,
}

impl FakeVolumes {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn mount(&self, uuid: &str, at: &Path) {
        let mut d = self.drives.lock();
        d.retain(|(u, _)| u != uuid);
        d.push((uuid.to_owned(), at.to_path_buf()));
    }

    fn unmount(&self, uuid: &str) {
        self.drives.lock().retain(|(u, _)| u != uuid);
    }
}

impl VolumeProbe for FakeVolumes {
    fn identify(&self, path: &Path) -> Option<VolumeIdentity> {
        self.drives
            .lock()
            .iter()
            .filter(|(_, m)| path.starts_with(m) && m.is_dir())
            .max_by_key(|(_, m)| m.components().count())
            .map(|(u, m)| VolumeIdentity {
                uuid: u.clone(),
                mount: m.clone(),
                label: Some(format!("ドライブ {u}")),
                fs_type: Some("exfat".to_owned()),
                network: false,
            })
    }

    fn mounted(&self) -> Vec<VolumeIdentity> {
        let drives = self.drives.lock().clone();
        drives
            .iter()
            .filter_map(|(_, m)| self.identify(m))
            .collect()
    }
}

fn jpeg_with_time(path: &Path, seed: u8, local: &str) {
    write_jpeg(
        path,
        (32, 24),
        seed,
        Some(&exif(Some(local), Some(9 * 60), None)),
    );
}

fn folder_of(core: &genzo_api::Core, path: &Path) -> genzo_model::FolderId {
    let folders = core.folders().unwrap();
    folders
        .iter()
        .find(|f| f.path.as_deref() == Some(path))
        .unwrap_or_else(|| panic!("{} のフォルダがない: {folders:?}", path.display()))
        .folder_id
}

fn path_of(core: &genzo_api::Core, v: VariantId) -> Vec<PathBuf> {
    core.variant_details(v)
        .unwrap()
        .files
        .into_iter()
        .filter_map(|f| f.path)
        .collect()
}

fn move_report(core: &genzo_api::Core, job: u64) -> genzo_api::FileMoveReport {
    match wait_ok(core, job) {
        JobResult::MoveFiles(r) => r,
        other => panic!("{other:?}"),
    }
}

/// 同じボリュームの中の移動: RAW と同時記録の JPEG を一緒に名前の変更で動かし、内容は変えない。移動先に同じ
/// 名前・拡張子だけが違う別の写真があれば、その写真は飛ばす。
#[test]
fn photos_move_with_their_pairs_inside_a_volume() {
    let env = Env::new();
    let src = env.photos.join("2024");
    let dest = env.photos.join("選別済み");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(src.join("A.dng"), b"not really a dng").unwrap();
    jpeg_with_time(&src.join("A.jpg"), 1, "2024-05-01 10:00:00");
    jpeg_with_time(&src.join("B.jpg"), 2, "2024-05-01 10:01:00");
    jpeg_with_time(&src.join("C.jpg"), 3, "2024-05-01 10:02:00");
    // 移動先に、C と拡張子だけが違う、カタログにない RAW がある。
    std::fs::write(dest.join("C.arw"), b"other camera").unwrap();
    let before = hashes(&env.photos);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "A.dng").variant_id;
    let b = by_name(&items, "B.jpg").variant_id;
    let c = by_name(&items, "C.jpg").variant_id;
    let dest_id = folder_of(&core, &dest);

    let plan = core.plan_move_files(&[a, b, c], dest_id).unwrap();
    assert_eq!(plan.movable, 2);
    assert_eq!(plan.copy_bytes, 0);
    let pa = &plan.photos[0];
    assert_eq!(pa.files.len(), 2, "RAW と JPEG を一緒に動かす");
    assert!(pa.files.iter().all(|f| f.method == MoveMethod::Rename));
    assert!(
        plan.photos[2]
            .problem
            .as_deref()
            .unwrap()
            .contains("拡張子だけが違う")
    );
    // 予定だけでは何も変わらない。
    assert_eq!(hashes(&env.photos), before);

    let r = move_report(&core, core.move_files(&[a, b, c], dest_id).unwrap());
    assert_eq!(r.photos, 2);
    assert_eq!(r.moved.len(), 3);
    assert_eq!(r.skipped.len(), 1);
    assert!(r.failed.is_empty() && r.left_sources.is_empty());
    assert_eq!(
        path_of(&core, a),
        vec![dest.join("A.dng"), dest.join("A.jpg")]
    );
    assert_eq!(path_of(&core, b), vec![dest.join("B.jpg")]);
    assert_eq!(path_of(&core, c), vec![src.join("C.jpg")]);
    assert!(!src.join("A.jpg").exists());
    // 内容は変わらない（同じハッシュのファイルが場所を変えただけ）。
    let mut after: Vec<String> = hashes(&env.photos).into_values().collect();
    let mut was: Vec<String> = before.into_values().collect();
    after.sort();
    was.sort();
    assert_eq!(after, was);
    // 予定と実行の記録はどれも完了している（次に開いても確定するものはない）。
    core.close().unwrap();
    let core = env.open();
    assert_eq!(core.startup_report().recovered_file_ops, 0);
    // 移動先で探せる（フォルダで絞り込む）。
    let r = core
        .search(
            &genzo_api::SearchFilter {
                folder_id: Some(dest_id),
                ..Default::default()
            },
            genzo_api::SearchSort::default(),
        )
        .unwrap();
    // A（RAW と JPEG で 1 枚）・B と、取り込みで登録した C.arw。
    assert_eq!(r.count, 3);
    core.close().unwrap();
}

/// 写真の移動の途中で失敗したら、動かしたファイルを元に戻し、カタログも元のまま（記録は failed）。
#[test]
fn a_failed_move_is_rolled_back() {
    let env = Env::new();
    let src = env.photos.join("src");
    let dest = env.photos.join("dest");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(src.join("A.dng"), b"raw").unwrap();
    jpeg_with_time(&src.join("A.jpg"), 1, "2024-05-01 10:00:00");
    let core = env.open_with_hooks(|h| {
        // 2 つ目のファイルを動かしたところで失敗させる。
        let count = Arc::new(Mutex::new(0));
        h.on_file_op = Some(Arc::new(move |stage: &FileOpStage| {
            if let FileOpStage::Moved(_) = stage {
                let mut n = count.lock();
                *n += 1;
                if *n == 2 {
                    return Err("テストの失敗".to_owned());
                }
            }
            Ok(())
        }));
    });
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    let dest_id = folder_of(&core, &dest);
    let r = move_report(&core, core.move_files(&[a], dest_id).unwrap());
    assert_eq!(r.photos, 0);
    assert_eq!(r.failed.len(), 1);
    assert!(r.failed[0].reason.contains("テストの失敗"));
    assert!(src.join("A.dng").exists() && src.join("A.jpg").exists());
    assert!(!dest.join("A.dng").exists() && !dest.join("A.jpg").exists());
    assert_eq!(
        path_of(&core, a),
        vec![src.join("A.dng"), src.join("A.jpg")]
    );
    core.close().unwrap();
}

/// 別のボリュームへの移動（FILE-03 の偽のボリューム）: コピーして照合し、カタログを移動先にしてから、元を
/// ゴミ箱へ移す。全体のハッシュも記録する。ゴミ箱へ移せなければ、元を残して知らせる。
#[test]
fn moving_to_another_volume_copies_verifies_and_trashes_the_source() {
    let env = Env::new();
    let drive_a = env.photos.join("driveA");
    let drive_b = env.photos.join("driveB");
    std::fs::create_dir_all(drive_a.join("DCIM")).unwrap();
    std::fs::create_dir_all(drive_b.join("写真")).unwrap();
    jpeg_with_time(
        &drive_a.join("DCIM").join("X.jpg"),
        1,
        "2024-05-01 10:00:00",
    );
    jpeg_with_time(
        &drive_a.join("DCIM").join("Y.jpg"),
        2,
        "2024-05-01 10:01:00",
    );
    let content = genzo_catalog::full_hash(&drive_a.join("DCIM").join("X.jpg")).unwrap();
    let probe = FakeVolumes::new();
    probe.mount("uuid:A", &drive_a);
    probe.mount("uuid:B", &drive_b);
    let p = probe.clone();
    let core = env.open_with_hooks(move |h| h.volumes = p);
    import(&core, &drive_a);
    import(&core, &drive_b);
    let vols = core.volumes().unwrap();
    assert!(
        vols.iter()
            .any(|v| v.uuid == "uuid:A" && v.identified && v.online)
    );
    assert_eq!(
        vols.iter()
            .find(|v| v.uuid == "uuid:A")
            .unwrap()
            .label
            .as_deref(),
        Some("ドライブ uuid:A")
    );
    let items = all_items(&core);
    let x = by_name(&items, "X.jpg").variant_id;
    let y = by_name(&items, "Y.jpg").variant_id;
    let dest_id = folder_of(&core, &drive_b.join("写真"));
    let plan = core.plan_move_files(&[x], dest_id).unwrap();
    assert_eq!(plan.photos[0].files[0].method, MoveMethod::Copy);
    assert!(plan.copy_bytes > 0);

    let r = move_report(&core, core.move_files(&[x], dest_id).unwrap());
    assert_eq!(r.photos, 1);
    assert!(r.bytes_copied > 0);
    let moved = drive_b.join("写真").join("X.jpg");
    assert_eq!(genzo_catalog::full_hash(&moved).unwrap(), content);
    assert!(!drive_a.join("DCIM").join("X.jpg").exists(), "元はゴミ箱へ");
    assert_eq!(
        env.trash.moved.lock().clone(),
        vec![drive_a.join("DCIM").join("X.jpg")]
    );
    let details = core.variant_details(x).unwrap();
    assert_eq!(details.files[0].path.as_deref(), Some(moved.as_path()));
    assert_eq!(
        details.files[0].full_hash.as_deref(),
        Some(content.as_str())
    );
    assert_eq!(details.files[0].status, FileStatus::Ok);

    // ゴミ箱へ移せない: 元を残して、結果と警告で知らせる（カタログは移動先を指す）。
    env.trash.fail_names.lock().insert("Y.jpg".to_owned());
    let rx = core.subscribe();
    let r = move_report(&core, core.move_files(&[y], dest_id).unwrap());
    assert_eq!(r.photos, 1);
    assert_eq!(r.left_sources.len(), 1);
    assert!(drive_a.join("DCIM").join("Y.jpg").exists());
    assert_eq!(path_of(&core, y), vec![drive_b.join("写真").join("Y.jpg")]);
    assert!(common::drain(&rx).iter().any(|e| matches!(
        e,
        Event::Warning {
            code: WarningCode::MoveSourceLeft,
            ..
        }
    )));
    core.close().unwrap();
}

/// ドライブ文字・マウント先が変わっても同じボリュームとして追い、つながっていないボリュームはオフラインにして、
/// 見つからない印を付けない（FILE-03）。
#[test]
fn volumes_are_followed_across_mount_points_and_offline_files_are_not_marked_missing() {
    let env = Env::new();
    let mount1 = env.photos.join("mnt1");
    std::fs::create_dir_all(mount1.join("DCIM")).unwrap();
    jpeg_with_time(&mount1.join("DCIM").join("S.jpg"), 1, "2024-05-01 10:00:00");
    let probe = FakeVolumes::new();
    probe.mount("uuid:SD", &mount1);
    let p = probe.clone();
    let core = env.open_with_hooks(move |h| h.volumes = p);
    import(&core, &mount1);
    let s = all_items(&core)[0].variant_id;

    // 外す（マウント先のフォルダもなくなる）。
    let mount2 = env.photos.join("mnt2");
    std::fs::rename(&mount1, &mount2).unwrap();
    probe.unmount("uuid:SD");
    let vols = core.refresh_volumes().unwrap();
    let sd = vols.iter().find(|v| v.uuid == "uuid:SD").unwrap();
    assert!(!sd.online);
    assert!(
        core.folders()
            .unwrap()
            .iter()
            .filter(|f| f.volume_id == sd.volume_id)
            .all(|f| !f.online)
    );
    assert_eq!(path_of(&core, s), Vec::<PathBuf>::new(), "場所は分からない");
    // 確かめても、見つからない印は付けない。
    let job = core.check_missing(None).unwrap();
    match wait_ok(&core, job) {
        JobResult::CheckMissing(r) => {
            assert_eq!(r.newly_missing, 0);
            assert_eq!(r.skipped_offline, 1);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        core.variant_details(s).unwrap().files[0].status,
        FileStatus::Ok
    );

    // 別のマウント先につなぐ: 同じ ID なので同じボリューム。新しいマウント先で同じファイルを指す。
    probe.mount("uuid:SD", &mount2);
    let vols = core.refresh_volumes().unwrap();
    let sd = vols.iter().find(|v| v.uuid == "uuid:SD").unwrap();
    assert!(sd.online);
    assert_eq!(sd.mount_path.as_deref(), Some(mount2.as_path()));
    assert_eq!(path_of(&core, s), vec![mount2.join("DCIM").join("S.jpg")]);
    assert!(core.check_files(&[s]).unwrap().missing.is_empty());
    // 取り込み直しても、写真は増えない（同じボリュームの同じフォルダ）。
    import(&core, &mount2);
    assert_eq!(all_items(&core).len(), 1);
    core.close().unwrap();
}

/// macOS の起動ディスクに似たボリューム: マウント先の中のパスはデータのボリュームの ID、マウント先そのものを
/// 問い合わせると別の（システムのボリュームの）ID を返し、つながっているボリュームの一覧にはデータのボリュームが
/// マウント先 `mount` で入っている。
struct BootDiskLikeVolumes {
    mount: PathBuf,
}

impl VolumeProbe for BootDiskLikeVolumes {
    fn identify(&self, path: &Path) -> Option<VolumeIdentity> {
        if !path.starts_with(&self.mount) {
            return None;
        }
        let uuid = if path == self.mount {
            "uuid:SYSTEM"
        } else {
            "uuid:DATA"
        };
        Some(VolumeIdentity {
            uuid: uuid.to_owned(),
            mount: self.mount.clone(),
            label: None,
            fs_type: Some("apfs".to_owned()),
            network: false,
        })
    }

    fn mounted(&self) -> Vec<VolumeIdentity> {
        vec![VolumeIdentity {
            uuid: "uuid:DATA".to_owned(),
            mount: self.mount.clone(),
            label: None,
            fs_type: Some("apfs".to_owned()),
            network: false,
        }]
    }
}

/// マウント先を問い合わせると別の ID が返るボリューム（macOS の起動ディスク）でも、ボリュームの一覧で同じ ID・
/// 同じマウント先が見つかれば、アプリの外で消したファイルに見つからない印を付け、書き換えたファイルを読み直す
/// （検証のラウンド 1 の指摘。ボリュームを確かめ直すときと同じ確かめ方）。
#[test]
fn files_on_a_boot_disk_like_volume_are_checked() {
    let env = Env::new();
    let disk = env.photos.join("disk");
    std::fs::create_dir_all(disk.join("Users")).unwrap();
    jpeg_with_time(&disk.join("Users").join("A.jpg"), 1, "2024-05-01 10:00:00");
    jpeg_with_time(&disk.join("Users").join("B.jpg"), 2, "2024-05-01 10:01:00");
    let probe = Arc::new(BootDiskLikeVolumes {
        mount: disk.clone(),
    });
    let core = env.open_with_hooks(move |h| h.volumes = probe);
    import(&core, &disk.join("Users"));
    let vols = core.refresh_volumes().unwrap();
    assert!(vols.iter().any(|v| v.uuid == "uuid:DATA" && v.online));
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg").variant_id;
    let b = by_name(&items, "B.jpg").variant_id;

    std::fs::remove_file(disk.join("Users").join("A.jpg")).unwrap();
    jpeg_with_time(&disk.join("Users").join("B.jpg"), 9, "2024-05-02 11:00:00");
    let report = core.check_files(&[a, b]).unwrap();
    assert_eq!(report.missing.len(), 1, "{report:?}");
    assert_eq!(report.changed, vec![disk.join("Users").join("B.jpg")]);
    assert_eq!(
        core.variant_details(a).unwrap().files[0].status,
        FileStatus::Missing
    );
    core.close().unwrap();
}

/// 見つからない写真の確認: アプリの外で削除・移動されたファイルとフォルダに印を付け、戻れば外す（FILE-04）。
/// 絞り込みの「見つからない」で探せる。
#[test]
fn missing_files_and_folders_are_marked_and_restored() {
    let env = Env::new();
    let f1 = env.photos.join("f1");
    let f2 = env.photos.join("f2");
    std::fs::create_dir_all(&f1).unwrap();
    std::fs::create_dir_all(&f2).unwrap();
    jpeg_with_time(&f1.join("A.jpg"), 1, "2024-05-01 10:00:00");
    jpeg_with_time(&f1.join("B.jpg"), 2, "2024-05-01 10:01:00");
    jpeg_with_time(&f2.join("C.jpg"), 3, "2024-05-01 10:02:00");
    let core = env.open();
    import(&core, &env.photos);
    let outside = env.dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::rename(f1.join("A.jpg"), outside.join("A.jpg")).unwrap();
    std::fs::rename(&f2, outside.join("f2")).unwrap();

    let job = core.check_missing(None).unwrap();
    let r = match wait_ok(&core, job) {
        JobResult::CheckMissing(r) => r,
        other => panic!("{other:?}"),
    };
    assert_eq!(r.newly_missing, 2);
    assert_eq!(r.missing, 2);
    assert_eq!(r.missing_folders, 1);
    let folders = core.folders().unwrap();
    let f2_info = folders.iter().find(|f| f.rel_path.ends_with("f2")).unwrap();
    assert!(f2_info.missing);
    assert_eq!(f2_info.missing_files, 1);
    let f1_info = folders.iter().find(|f| f.rel_path.ends_with("f1")).unwrap();
    assert!(!f1_info.missing);
    assert_eq!(f1_info.missing_files, 1);
    // 絞り込みの「見つからない」。
    let missing = core
        .search(
            &genzo_api::SearchFilter {
                file_statuses: Some(vec![FileStatus::Missing]),
                ..Default::default()
            },
            genzo_api::SearchSort::default(),
        )
        .unwrap();
    assert_eq!(missing.count, 2);

    // 戻すと、次の確認で印を外す。
    std::fs::rename(outside.join("A.jpg"), f1.join("A.jpg")).unwrap();
    std::fs::rename(outside.join("f2"), &f2).unwrap();
    let job = core.check_missing(None).unwrap();
    match wait_ok(&core, job) {
        JobResult::CheckMissing(r) => {
            assert_eq!(r.restored, 2);
            assert_eq!(r.missing, 0);
            assert_eq!(r.missing_folders, 0);
        }
        other => panic!("{other:?}"),
    }
    assert!(
        core.folders()
            .unwrap()
            .iter()
            .all(|f| !f.missing && f.missing_files == 0)
    );
    core.close().unwrap();
}

/// 再リンク: 選んだフォルダから、大きさとクイックハッシュ（全体のハッシュの記録があれば全体）が同じファイル、
/// 名前と撮影日時が同じで内容の違うファイルを候補にし、確かめてから結び付け直す（FILE-04）。
#[test]
fn missing_photos_are_relinked_after_confirmation() {
    let env = Env::new();
    let orig = env.photos.join("orig");
    std::fs::create_dir_all(&orig).unwrap();
    jpeg_with_time(&orig.join("A.jpg"), 1, "2024-05-01 10:00:00");
    jpeg_with_time(&orig.join("B.jpg"), 2, "2024-05-01 10:01:00");
    let core = env.open();
    import(&core, &env.photos);
    // A は全体のハッシュを記録しておく（内容で確かめる）。
    let job = core.compute_content_hashes(true).unwrap();
    wait_ok(&core, job);
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg").variant_id;
    let b = by_name(&items, "B.jpg").variant_id;
    // アプリの外で、A は名前を変えて移し、B は同じ名前・同じ撮影日時で内容を書き換えて別の場所へ。
    let moved = env.dir.path().join("moved");
    std::fs::create_dir_all(moved.join("sub")).unwrap();
    std::fs::rename(orig.join("A.jpg"), moved.join("sub").join("renamed.jpg")).unwrap();
    std::fs::remove_file(orig.join("B.jpg")).unwrap();
    jpeg_with_time(&moved.join("B.jpg"), 9, "2024-05-01 10:01:00");
    // 関係のない写真（同じ名前で撮影日時が違う）は候補にしない。
    jpeg_with_time(&moved.join("sub").join("B.jpg"), 8, "2023-01-01 00:00:00");
    let job = core.check_missing(None).unwrap();
    wait_ok(&core, job);

    let job = core
        .find_relink_candidates(Some(vec![a, b]), &moved, true)
        .unwrap();
    let search = match wait_ok(&core, job) {
        JobResult::RelinkCandidates(s) => s,
        other => panic!("{other:?}"),
    };
    assert_eq!(search.scanned_files, 3);
    let ta = search.targets.iter().find(|t| t.variant_id == a).unwrap();
    assert_eq!(ta.candidates.len(), 1);
    assert_eq!(ta.candidates[0].matched, RelinkMatch::Content);
    assert_eq!(ta.candidates[0].path, moved.join("sub").join("renamed.jpg"));
    assert!(!ta.candidates[0].same_name);
    let tb = search.targets.iter().find(|t| t.variant_id == b).unwrap();
    assert_eq!(tb.candidates.len(), 1);
    assert_eq!(tb.candidates[0].matched, RelinkMatch::NameAndTime);
    assert_eq!(tb.candidates[0].path, moved.join("B.jpg"));

    // 内容の違う候補は、利用者が選ばなければ結び付けない。
    let r = core
        .relink_files(&[
            RelinkRequest {
                file_id: ta.file_id,
                path: ta.candidates[0].path.clone(),
                accept_changed: false,
            },
            RelinkRequest {
                file_id: tb.file_id,
                path: tb.candidates[0].path.clone(),
                accept_changed: false,
            },
        ])
        .unwrap();
    assert_eq!(r.linked.len(), 1);
    assert_eq!(r.failed.len(), 1);
    let da = core.variant_details(a).unwrap();
    assert_eq!(da.files[0].status, FileStatus::Ok);
    assert_eq!(
        da.files[0].path.as_deref(),
        Some(moved.join("sub").join("renamed.jpg").as_path())
    );
    let r = core
        .relink_files(&[RelinkRequest {
            file_id: tb.file_id,
            path: tb.candidates[0].path.clone(),
            accept_changed: true,
        }])
        .unwrap();
    assert_eq!(r.linked.len(), 1);
    assert_eq!(r.reread, vec![moved.join("B.jpg")]);
    let db = core.variant_details(b).unwrap();
    assert_eq!(db.files[0].status, FileStatus::Ok);
    // 結び付けた場所のフォルダはカタログに登録される。
    assert!(
        core.folders()
            .unwrap()
            .iter()
            .any(|f| f.path.as_deref() == Some(moved.join("sub").as_path()))
    );
    // 見つかっているファイルは結び付け直さない。
    let r = core
        .relink_files(&[RelinkRequest {
            file_id: ta.file_id,
            path: moved.join("sub").join("B.jpg"),
            accept_changed: true,
        }])
        .unwrap();
    assert_eq!(r.failed.len(), 1);

    // 空になった元のフォルダ（ディスクからも消えて見つからない）は、カタログから除ける（ディスクには触れない）。
    // 写真の登録されたフォルダは除かない（検証のラウンド 1 の指摘）。
    let orig_id = folder_of(&core, &orig);
    std::fs::remove_dir(&orig).unwrap();
    let job = core.check_missing(None).unwrap();
    match wait_ok(&core, job) {
        JobResult::CheckMissing(r) => assert_eq!(r.missing_folders, 1, "{r:?}"),
        other => panic!("{other:?}"),
    }
    let sub_id = folder_of(&core, &moved.join("sub"));
    let err = core.remove_folder(sub_id).unwrap_err();
    assert!(err.to_string().contains("写真のファイルが 1 個"), "{err}");
    let before = (hashes(&env.photos), hashes(&moved));
    assert_eq!(core.remove_folder(orig_id).unwrap(), 1);
    assert!(
        core.folders()
            .unwrap()
            .iter()
            .all(|f| f.folder_id != orig_id)
    );
    assert_eq!(
        (hashes(&env.photos), hashes(&moved)),
        before,
        "ディスクには触れない"
    );
    let job = core.check_missing(None).unwrap();
    match wait_ok(&core, job) {
        JobResult::CheckMissing(r) => assert_eq!(r.missing_folders, 0, "{r:?}"),
        other => panic!("{other:?}"),
    }
    core.close().unwrap();
}

/// フォルダの場所の指定し直し（アプリの外で名前を変えたフォルダ）: 新しい場所で見つかる数を示し、配下の
/// フォルダごと結び付け直す（FILE-04）。
#[test]
fn a_renamed_folder_can_be_relocated() {
    let env = Env::new();
    let trip = env.photos.join("旅行");
    std::fs::create_dir_all(trip.join("1日目")).unwrap();
    jpeg_with_time(&trip.join("A.jpg"), 1, "2024-05-01 10:00:00");
    jpeg_with_time(&trip.join("1日目").join("B.jpg"), 2, "2024-05-01 10:01:00");
    let core = env.open();
    import(&core, &env.photos);
    let trip_id = folder_of(&core, &trip);
    let renamed = env.photos.join("2024 京都旅行");
    std::fs::rename(&trip, &renamed).unwrap();
    let job = core.check_missing(None).unwrap();
    wait_ok(&core, job);
    assert!(
        core.folders()
            .unwrap()
            .iter()
            .find(|f| f.folder_id == trip_id)
            .unwrap()
            .missing
    );

    let plan = core.plan_relocate_folder(trip_id, &renamed).unwrap();
    assert_eq!(plan.problem, None);
    assert_eq!((plan.files, plan.found, plan.subfolders), (2, 2, 1));
    // 登録済みの別のフォルダには指定し直せない。
    let other = env.photos.join("その他");
    std::fs::create_dir_all(&other).unwrap();
    import(&core, &other);
    assert!(
        core.plan_relocate_folder(trip_id, &other)
            .unwrap()
            .problem
            .is_some()
    );

    let after = core.relocate_folder(trip_id, &renamed).unwrap();
    assert_eq!(after.found, 2);
    let items = all_items(&core);
    for name in ["A.jpg", "B.jpg"] {
        let v = by_name(&items, name).variant_id;
        let d = core.variant_details(v).unwrap();
        assert_eq!(d.files[0].status, FileStatus::Ok, "{name}");
        assert!(d.files[0].path.as_deref().unwrap().starts_with(&renamed));
    }
    assert!(core.folders().unwrap().iter().all(|f| !f.missing));
    core.close().unwrap();
}

/// 写真の名前の変更（テンプレート・直接の名前）: RAW と JPEG は同じ名前にし、重なる名前には番号を付ける。
/// 大文字・小文字だけの変更もできる。
#[test]
fn photos_are_renamed_with_templates_and_names() {
    let env = Env::new();
    std::fs::write(env.photos.join("DSC1.dng"), b"raw").unwrap();
    jpeg_with_time(&env.photos.join("DSC1.jpg"), 1, "2024-05-01 10:00:00");
    jpeg_with_time(&env.photos.join("DSC2.jpg"), 2, "2024-05-01 10:00:00");
    jpeg_with_time(&env.photos.join("taken.jpg"), 3, "2024-05-01 10:00:00");
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let p1 = by_name(&items, "DSC1.dng").variant_id;
    let p2 = by_name(&items, "DSC2.jpg").variant_id;
    let taken = by_name(&items, "taken.jpg").variant_id;
    let spec = RenameSpec::Template {
        rule: RenameRule {
            template: "京都-{seq}".to_owned(),
            sequence_digits: 2,
            ..RenameRule::default()
        },
    };
    let plan = core.plan_rename_files(&[p1, p2], &spec).unwrap();
    assert_eq!(plan.renamable, 2);
    let names: Vec<String> = plan.photos[0]
        .files
        .iter()
        .map(|f| {
            f.to.as_ref()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    // DNG の撮影日時は読めない（作り物）が、JPEG と組の写真として JPEG と同じ名前になる。
    assert_eq!(names, vec!["京都-01.dng", "京都-01.jpg"]);
    let r = core.rename_files(&[p1, p2], &spec).unwrap();
    assert_eq!(r.photos, 2);
    assert!(env.photos.join("京都-01.dng").exists());
    assert!(env.photos.join("京都-01.jpg").exists());
    assert!(env.photos.join("京都-02.jpg").exists());
    // 同じ名前があれば番号を付ける（テンプレート）。直接の名前は断る。
    let r = core.rename_files(&[taken], &spec).unwrap();
    assert_eq!(r.photos, 1);
    assert_eq!(
        path_of(&core, taken),
        vec![env.photos.join("京都-01-1.jpg")]
    );
    let r = core
        .rename_files(
            &[taken],
            &RenameSpec::Name {
                name: "京都-02".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(r.photos, 0);
    assert_eq!(r.skipped.len(), 1);
    // 使えない名前・複数の写真への直接の名前はエラー。
    assert!(
        core.plan_rename_files(
            &[taken],
            &RenameSpec::Name {
                name: "a/b".to_owned()
            }
        )
        .is_err()
    );
    assert!(
        core.plan_rename_files(
            &[p1, p2],
            &RenameSpec::Name {
                name: "x".to_owned()
            }
        )
        .is_err()
    );
    // 大文字・小文字だけの変更。
    let r = core
        .rename_files(
            &[p2],
            &RenameSpec::Name {
                name: "京都-02".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(r.skipped[0].reason, "今と同じ名前です");
    jpeg_with_time(&env.photos.join("lower.jpg"), 4, "2024-05-01 10:00:00");
    import(&core, &env.photos);
    let lower = by_name(&all_items(&core), "lower.jpg").variant_id;
    let r = core
        .rename_files(
            &[lower],
            &RenameSpec::Name {
                name: "LOWER".to_owned(),
            },
        )
        .unwrap();
    assert_eq!(r.photos, 1);
    assert_eq!(path_of(&core, lower), vec![env.photos.join("LOWER.jpg")]);
    let on_disk: Vec<String> = std::fs::read_dir(&env.photos)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(on_disk.contains(&"LOWER.jpg".to_owned()));
    assert!(!on_disk.contains(&"lower.jpg".to_owned()));
    core.close().unwrap();
}

/// フォルダの作成・名前の変更・移動（ディスクとカタログ）。ボリュームのルートと、カタログのあるフォルダは
/// 動かせない。別のボリュームへのフォルダの移動はできない。
#[test]
fn folders_are_created_renamed_and_moved() {
    let env = Env::new();
    std::fs::create_dir_all(env.photos.join("a")).unwrap();
    std::fs::create_dir_all(env.photos.join("b")).unwrap();
    jpeg_with_time(
        &env.photos.join("a").join("A.jpg"),
        1,
        "2024-05-01 10:00:00",
    );
    let core = env.open();
    import(&core, &env.photos);
    let photos_id = folder_of(&core, &env.photos);
    let a_id = folder_of(&core, &env.photos.join("a"));
    let b_id = folder_of(&core, &env.photos.join("b"));

    let created = core.create_folder(photos_id, "選別").unwrap();
    assert!(env.photos.join("選別").is_dir());
    assert_eq!(
        created.path.as_deref(),
        Some(env.photos.join("選別").as_path())
    );
    assert!(core.create_folder(photos_id, "選別").is_err(), "同じ名前");
    assert!(core.create_folder(photos_id, "CON").is_err(), "予約名");

    let renamed = core.rename_folder(a_id, "a-2024").unwrap();
    assert_eq!(renamed.folder_id, a_id);
    assert!(env.photos.join("a-2024").join("A.jpg").exists());
    let a = all_items(&core)[0].variant_id;
    assert_eq!(
        path_of(&core, a),
        vec![env.photos.join("a-2024").join("A.jpg")]
    );
    let moved = core.move_folder(a_id, b_id).unwrap();
    assert_eq!(moved.parent_id, Some(b_id));
    assert_eq!(
        path_of(&core, a),
        vec![env.photos.join("b").join("a-2024").join("A.jpg")]
    );
    // 名前の重なり・自分の中へ。
    assert!(core.rename_folder(created.folder_id, "b").is_err());
    assert!(core.move_folder(b_id, a_id).is_err());
    // ボリュームのルート・カタログのあるフォルダ。
    let root = core
        .folders()
        .unwrap()
        .into_iter()
        .find(|f| f.parent_id.is_none())
        .unwrap();
    assert!(core.rename_folder(root.folder_id, "x").is_err());
    // カタログ（env.dir/catalog/catalog.db）を含むフォルダ（取り込みで登録した env.dir）は動かせない。
    import(&core, env.dir.path());
    let top = folder_of(&core, env.dir.path());
    let err = core.rename_folder(top, "x").unwrap_err();
    assert!(err.to_string().contains("カタログ"), "{err}");
    assert!(env.catalog_path().exists());
    // 終わったファイル操作の記録はどれも完了。
    core.close().unwrap();
    let core = env.open();
    assert_eq!(core.startup_report().recovered_file_ops, 0);
    core.close().unwrap();
}

/// 配下のフォルダが、親の部分を違う綴り（NFC と NFD）で登録されていても、フォルダの名前の変更・場所の指定し
/// 直しの予定で異常終了しない（検証のラウンド 1 の指摘）。綴りを区別するボリューム（Linux の ext4 など）では、
/// その綴りのフォルダは別にあって一緒に動かないので、名前の変更は動かさずに断る（区別しないボリュームでは同じ
/// フォルダなので、配下も一緒に動く）。
#[test]
fn folders_with_subfolders_registered_in_another_spelling_are_not_broken() {
    let env = Env::new();
    let nfc = env.photos.join("nf").join("\u{c9}");
    let nfd_sub = env.photos.join("nf").join("E\u{301}").join("sub");
    std::fs::create_dir_all(&nfc).unwrap();
    std::fs::create_dir_all(&nfd_sub).unwrap();
    jpeg_with_time(&nfc.join("A.jpg"), 1, "2024-05-01 10:00:00");
    jpeg_with_time(&nfd_sub.join("B.jpg"), 2, "2024-05-01 10:01:00");
    let distinct = !same_file::is_same_file(&nfc, nfd_sub.parent().unwrap()).unwrap();
    let core = env.open();
    import(&core, &nfc);
    import(&core, &nfd_sub);
    let id = folder_of(&core, &nfc);
    let items = all_items(&core);
    let b = by_name(&items, "B.jpg").variant_id;

    // 場所の指定し直しの予定（配下のファイルを数える）。
    let elsewhere = env.photos.join("別の場所");
    std::fs::create_dir_all(elsewhere.join("sub")).unwrap();
    let plan = core.plan_relocate_folder(id, &elsewhere).unwrap();
    assert_eq!((plan.files, plan.subfolders), (2, 1));

    let before = hashes(&env.photos);
    let r = core.rename_folder(id, "Zed");
    if distinct {
        let err = r.unwrap_err();
        assert!(err.to_string().contains("書き方"), "{err}");
        assert_eq!(hashes(&env.photos), before, "動かしていない");
        assert_eq!(path_of(&core, b), vec![nfd_sub.join("B.jpg")]);
    } else {
        r.unwrap();
        assert_eq!(
            path_of(&core, b),
            vec![env.photos.join("nf").join("Zed").join("sub").join("B.jpg")]
        );
    }
    core.close().unwrap();
    let core = env.open();
    assert_eq!(core.startup_report().recovered_file_ops, 0);
    core.close().unwrap();
}

/// 途中で終わった移動・名前の変更・フォルダの名前の変更は、次に開いたときに、移動先にあるファイルだけを
/// カタログに反映して確定する（DATA-07）。
#[test]
fn unfinished_moves_are_settled_by_where_the_files_are() {
    let env = Env::new();
    let dest = env.photos.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::create_dir_all(env.photos.join("folder")).unwrap();
    jpeg_with_time(&env.photos.join("A.jpg"), 1, "2024-05-01 10:00:00");
    jpeg_with_time(&env.photos.join("B.jpg"), 2, "2024-05-01 10:01:00");
    jpeg_with_time(
        &env.photos.join("folder").join("C.jpg"),
        3,
        "2024-05-01 10:02:00",
    );
    {
        let core = env.open();
        import(&core, &env.photos);
        core.close().unwrap();
    }
    {
        let mut cat = genzo_catalog::Catalog::open(env.catalog_path()).unwrap();
        let mut a = None;
        let mut b = None;
        let mut dest_id = None;
        let mut folder_id = None;
        for r in cat.folder_records().unwrap() {
            if r.folder.rel_path.ends_with("photos/dest") {
                dest_id = Some(r.folder.id);
            }
            if r.folder.rel_path.ends_with("photos/folder") {
                folder_id = Some(r.folder.id);
            }
            for f in cat.files_in_folder(r.folder.id).unwrap() {
                match f.name.as_str() {
                    "A.jpg" => a = Some(f.id),
                    "B.jpg" => b = Some(f.id),
                    _ => {}
                }
            }
        }
        let (a, b, dest_id, folder_id) =
            (a.unwrap(), b.unwrap(), dest_id.unwrap(), folder_id.unwrap());
        // A と B を 1 つの操作で移す予定で、A だけ動かしたところで終了した。
        let op = cat
            .plan_file_moves(
                genzo_model::FileOpKind::Move,
                &[
                    genzo_catalog::MoveRequest {
                        file_id: a,
                        dest_folder: dest_id,
                        new_name: "A.jpg".to_owned(),
                        method: genzo_catalog::MoveMethod::Rename,
                    },
                    genzo_catalog::MoveRequest {
                        file_id: b,
                        dest_folder: dest_id,
                        new_name: "B.jpg".to_owned(),
                        method: genzo_catalog::MoveMethod::Rename,
                    },
                ],
            )
            .unwrap();
        cat.start_file_op(op).unwrap();
        std::fs::rename(env.photos.join("A.jpg"), dest.join("A.jpg")).unwrap();
        // フォルダの名前の変更は、OS の名前の変更の後に終了した。
        let rel = cat.folder(folder_id).unwrap().rel_path;
        let new_rel = rel.replace("photos/folder", "photos/folder2");
        let fop = cat
            .plan_folder_op(genzo_model::FileOpKind::Rename, folder_id, &new_rel)
            .unwrap();
        cat.start_file_op(fop).unwrap();
        std::fs::rename(env.photos.join("folder"), env.photos.join("folder2")).unwrap();
        drop(cat);
    }
    let core = env.open();
    assert_eq!(core.startup_report().recovered_file_ops, 2);
    let items = all_items(&core);
    assert_eq!(
        path_of(&core, by_name(&items, "A.jpg").variant_id),
        vec![dest.join("A.jpg")]
    );
    assert_eq!(
        path_of(&core, by_name(&items, "B.jpg").variant_id),
        vec![env.photos.join("B.jpg")]
    );
    assert_eq!(
        path_of(&core, by_name(&items, "C.jpg").variant_id),
        vec![env.photos.join("folder2").join("C.jpg")]
    );
    let warnings = core.startup_warnings();
    assert!(warnings.iter().any(|e| matches!(e, Event::Warning { code: WarningCode::FileOperationRecovered, message, .. } if message.contains("元の場所のまま"))));
    core.close().unwrap();
}

/// フォルダの名前の変更の直後にドライブが外れ（フォルダが見えなくなり）、元に戻せないときは、失敗として記録せず
/// 実行中のまま残し、つないで開き直したときに実際の場所を見て確定する（検証のラウンド 1。結果が分からない
/// 名前の変更を失敗として記録すると、カタログが元の名前を指したままになる）。
#[test]
fn a_rename_with_an_unknown_outcome_is_settled_on_the_next_open() {
    let env = Env::new();
    let drive = env.photos.join("drive");
    std::fs::create_dir_all(drive.join("Trip")).unwrap();
    jpeg_with_time(&drive.join("Trip").join("A.jpg"), 1, "2024-05-01 10:00:00");
    let unplugged = env.photos.join("drive-unplugged");
    let (d, u) = (drive.clone(), unplugged.clone());
    let core = env.open_with_hooks(move |h| {
        // 名前を変えた直後にドライブが外れたことにする（フォルダごと見えなくして失敗させる）。
        h.on_file_op = Some(Arc::new(move |stage: &FileOpStage| {
            if let FileOpStage::Moved(_) = stage {
                std::fs::rename(&d, &u).unwrap();
                return Err("ドライブが外れた".to_owned());
            }
            Ok(())
        }));
    });
    import(&core, &drive);
    let trip = folder_of(&core, &drive.join("Trip"));
    let err = core.rename_folder(trip, "Trip 2024").unwrap_err();
    assert!(
        err.to_string().contains("次にカタログを開いたとき"),
        "{err}"
    );
    core.close().unwrap();
    // つなぎ直す: フォルダは新しい名前になっている。
    std::fs::rename(&unplugged, &drive).unwrap();
    assert!(drive.join("Trip 2024").join("A.jpg").is_file());
    let core = env.open();
    assert_eq!(core.startup_report().recovered_file_ops, 1);
    let a = all_items(&core)[0].variant_id;
    assert_eq!(
        path_of(&core, a),
        vec![drive.join("Trip 2024").join("A.jpg")]
    );
    core.close().unwrap();
}

/// 大文字・小文字だけの名前の変更（大文字・小文字を区別しないボリュームでは一時的な名前を経由する）が、一時的な
/// 名前のまま終わっていれば、次に開いたときに名前の変更を終わらせて確定する（検証のラウンド 1 の指摘）。この
/// テストは、区別するボリュームでも同じ状態（元の名前も新しい名前もなく、一時的な名前だけがある）を作って
/// 確かめる。中身が登録したものと違う一時的な名前のものには触れない。
#[test]
fn case_only_renames_left_in_their_temporary_name_are_finished() {
    let env = Env::new();
    let trip = env.photos.join("Trip");
    std::fs::create_dir_all(&trip).unwrap();
    jpeg_with_time(&env.photos.join("D.jpg"), 1, "2024-05-01 10:00:00");
    jpeg_with_time(&env.photos.join("E.jpg"), 2, "2024-05-01 10:01:00");
    jpeg_with_time(&trip.join("F.jpg"), 3, "2024-05-01 10:02:00");
    {
        let core = env.open();
        import(&core, &env.photos);
        core.close().unwrap();
    }
    let temp_of = genzo_media::case_rename_temp_path;
    {
        let mut cat = genzo_catalog::Catalog::open(env.catalog_path()).unwrap();
        let photos = cat
            .folder_records()
            .unwrap()
            .into_iter()
            .find(|r| r.folder.rel_path.ends_with("photos"))
            .unwrap()
            .folder
            .id;
        let trip_id = cat
            .folder_records()
            .unwrap()
            .into_iter()
            .find(|r| r.folder.rel_path.ends_with("photos/Trip"))
            .unwrap()
            .folder
            .id;
        let file = |name: &str| {
            cat.files_in_folder(photos)
                .unwrap()
                .into_iter()
                .find(|f| f.name == name)
                .unwrap()
                .id
        };
        let (d, e) = (file("D.jpg"), file("E.jpg"));
        let rename = |file_id, new_name: &str| genzo_catalog::MoveRequest {
            file_id,
            dest_folder: photos,
            new_name: new_name.to_owned(),
            method: genzo_catalog::MoveMethod::Rename,
        };
        // D.jpg → d.jpg: 一時的な名前にしたところで終了した。
        let op = cat
            .plan_file_moves(genzo_model::FileOpKind::Rename, &[rename(d, "d.jpg")])
            .unwrap();
        cat.start_file_op(op).unwrap();
        std::fs::rename(env.photos.join("D.jpg"), temp_of(&env.photos.join("d.jpg"))).unwrap();
        // E.jpg → e.jpg: 一時的な名前のものは、中身が別のもの（この操作のものではない）。
        let op = cat
            .plan_file_moves(genzo_model::FileOpKind::Rename, &[rename(e, "e.jpg")])
            .unwrap();
        cat.start_file_op(op).unwrap();
        std::fs::remove_file(env.photos.join("E.jpg")).unwrap();
        std::fs::write(temp_of(&env.photos.join("e.jpg")), b"other").unwrap();
        // フォルダ Trip → trip: 一時的な名前にしたところで終了した。
        let rel = cat.folder(trip_id).unwrap().rel_path;
        let new_rel = rel.replace("photos/Trip", "photos/trip");
        let fop = cat
            .plan_folder_op(genzo_model::FileOpKind::Rename, trip_id, &new_rel)
            .unwrap();
        cat.start_file_op(fop).unwrap();
        std::fs::rename(&trip, temp_of(&env.photos.join("trip"))).unwrap();
        drop(cat);
    }
    let core = env.open();
    assert_eq!(core.startup_report().recovered_file_ops, 3);
    let items = all_items(&core);
    assert_eq!(
        path_of(&core, by_name(&items, "d.jpg").variant_id),
        vec![env.photos.join("d.jpg")]
    );
    assert!(env.photos.join("d.jpg").is_file());
    assert!(!temp_of(&env.photos.join("d.jpg")).exists());
    assert_eq!(
        path_of(&core, by_name(&items, "F.jpg").variant_id),
        vec![env.photos.join("trip").join("F.jpg")]
    );
    assert!(env.photos.join("trip").join("F.jpg").is_file());
    // 中身の違う一時的な名前のものはそのまま、写真は元の名前のまま（見つからない）。
    assert_eq!(
        std::fs::read(temp_of(&env.photos.join("e.jpg"))).unwrap(),
        b"other"
    );
    assert_eq!(
        path_of(&core, by_name(&items, "E.jpg").variant_id),
        vec![env.photos.join("E.jpg")]
    );
    core.close().unwrap();
}

/// 以前の形式（パスで識別）のボリュームのフォルダは、開いたときに、見えるものから OS のボリューム ID の
/// ボリュームへ移す（FILE-03 の既存のカタログ）。
#[test]
fn legacy_path_volumes_are_rebound_when_opened() {
    let env = Env::new();
    let drive = env.photos.join("drive");
    std::fs::create_dir_all(drive.join("DCIM")).unwrap();
    jpeg_with_time(&drive.join("DCIM").join("L.jpg"), 1, "2024-05-01 10:00:00");
    // まず、ボリューム ID を取れない形（偽物にドライブを登録しない）で取り込む（版 11 までと同じ `path:`）。
    let probe = FakeVolumes::new();
    {
        let p = probe.clone();
        let core = env.open_with_hooks(move |h| h.volumes = p);
        import(&core, &drive);
        assert!(
            core.volumes()
                .unwrap()
                .iter()
                .all(|v| v.uuid.starts_with("path:"))
        );
        core.close().unwrap();
    }
    // ドライブの ID を取れるようになったら、開いたときに移す。
    probe.mount("uuid:DRIVE", &drive);
    let p = probe.clone();
    let core = env.open_with_hooks(move |h| h.volumes = p);
    assert!(core.startup_warnings().iter().any(|e| matches!(
        e,
        Event::Warning {
            code: WarningCode::VolumesRebound,
            ..
        }
    )));
    let vols = core.volumes().unwrap();
    let id = vols.iter().find(|v| v.uuid == "uuid:DRIVE").unwrap();
    let l = all_items(&core)[0].variant_id;
    assert_eq!(path_of(&core, l), vec![drive.join("DCIM").join("L.jpg")]);
    let details = core.variant_details(l).unwrap();
    let folder = core
        .folders()
        .unwrap()
        .into_iter()
        .find(|f| f.folder_id == details.summary.folder_id)
        .unwrap();
    assert_eq!(folder.volume_id, id.volume_id);
    assert_eq!(folder.rel_path, "DCIM");
    // 取り込み直しても増えない。
    import(&core, &drive);
    assert_eq!(all_items(&core).len(), 1);
    core.close().unwrap();
}
