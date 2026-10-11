//! 独立レビューの指摘（F01〜F39・K1・K2）の回帰テスト。
//!
//! 各テストは、指摘の起きる手順を再現して、修正の後の結果を確かめる（時間では決めず、結果で決める）。

mod common;

use std::path::{Path, PathBuf};

use chrono::FixedOffset;
use common::{Env, all_items, by_name, exif, import, items_of, write_jpeg};
use genzo_api::{
    DeleteKind, DevelopSettings, Event, FileStatus, JobState, Lane, SearchFilter, SearchSort,
    SettingsUpdate, TestHook, WarningCode,
};
use genzo_catalog::{FileFacts, RegisterFile};
use genzo_model::{CaptureTime, PhotoMetadata, TzSource};

/// ワーカーの実行ファイル（テスト用の `genzo-api-worker`）に起動を拒ませる環境変数（`src/bin` の doc）。
const ENV_REFUSE_IF: &str = "GENZO_API_WORKER_REFUSE_IF";
/// ワーカーの実行ファイルの起動を 3 秒遅らせる環境変数（`src/bin` の doc）。
const ENV_SLOW_START_IF: &str = "GENZO_API_WORKER_SLOW_START_IF";

/// ワーカーを起動できない間に取り込んだファイルを、`status = error` で登録しない（ファイル自体には問題が
/// ない。次の取り込みでもう一度処理する。指摘 F34）。
#[test]
fn files_are_not_marked_as_errors_while_the_worker_cannot_start() {
    let env = Env::new();
    for (i, name) in ["A.jpg", "B.jpg", "C.jpg"].iter().enumerate() {
        write_jpeg(
            &env.photos.join(name),
            (32, 24),
            i as u8,
            Some(&exif(Some("2024-05-01 10:00:00"), Some(540), None)),
        );
    }
    let flag: PathBuf = env.dir.path().join("refuse-worker");
    let core = env.open_with(|c| {
        c.worker_env
            .push((ENV_REFUSE_IF.into(), flag.clone().into_os_string()));
    });
    // ここから先、ワーカーは起動できない。バッチ用のワーカーを異常終了させると、起動し直しに失敗する。
    std::fs::write(&flag, b"x").unwrap();
    let _ = core.run_worker_test_hook(Lane::Batch, TestHook::Crash { with_shm: false });
    let report = import(&core, &env.photos);
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(report.added, 0, "{report:?}");
    assert_eq!(report.not_registered.len(), 3, "{report:?}");
    assert!(
        report
            .not_registered
            .iter()
            .all(|i| i.reason.contains("ワーカー")),
        "{report:?}"
    );
    // ワーカーが起動できるようになったら、取り込み直すとメタデータ付きで登録される。
    std::fs::remove_file(&flag).unwrap();
    let again = import(&core, &env.photos);
    assert_eq!(again.added, 3, "{again:?}");
    assert!(again.errors.is_empty(), "{again:?}");
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg");
    assert_eq!(a.file_status, FileStatus::Ok);
    assert_eq!(a.camera.as_deref(), Some("GenzoTest Synthetic"), "{a:?}");
    core.close().unwrap();
}

/// 既定のタイムゾーンは、最初に開いたときの値をカタログに保存し、以後はその値を使う（開くたびに OS の
/// オフセット（初期値）が変わっても、写真ごとに推定のオフセットが混ざらないように。指摘 F15）。
#[test]
fn the_default_time_zone_is_kept_in_the_catalog() {
    let env = Env::new();
    write_jpeg(
        &env.photos.join("A.jpg"),
        (32, 24),
        1,
        Some(&exif(Some("2024-01-15 12:00:00"), None, None)),
    );
    let core = env.open_with(|c| c.default_utc_offset_minutes = Some(60));
    assert_eq!(core.settings().default_utc_offset_minutes, 60);
    import(&core, &env.photos);
    core.close().unwrap();
    // 初期値（OS のオフセットの代わり）が変わっても、保存した値を使う。
    let core = env.open_with(|c| c.default_utc_offset_minutes = Some(120));
    assert_eq!(core.settings().default_utc_offset_minutes, 60);
    write_jpeg(
        &env.photos.join("B.jpg"),
        (32, 24),
        2,
        Some(&exif(Some("2024-07-15 12:00:00"), None, None)),
    );
    import(&core, &env.photos);
    let items = all_items(&core);
    for name in ["A.jpg", "B.jpg"] {
        let s = by_name(&items, name);
        assert_eq!(
            s.capture.tz_assumed.as_deref(),
            Some("+01:00"),
            "{name}: {:?}",
            s.capture
        );
    }
    core.close().unwrap();
}

/// オフセットのない（既定のタイムゾーンで推定する）写真を `n` 件、カタログに直接登録する（ファイルは
/// 作らない。撮影日時の推定し直しだけを確かめるため）。
fn register_user_default_assets(catalog: &Path, n: usize) {
    let mut cat = genzo_catalog::Catalog::open(catalog).unwrap();
    let volume = cat
        .ensure_volume("path:/genzo-test-none", None, Some("/genzo-test-none"))
        .unwrap();
    let folder = cat.ensure_folder(volume, "photos").unwrap();
    let jst = FixedOffset::east_opt(9 * 3600).unwrap();
    let requests: Vec<RegisterFile> = (0..n)
        .map(|i| {
            let capture = CaptureTime::resolve_lossy(Some("2024:05:01 10:00:00"), None, jst);
            assert_eq!(capture.tz_source, TzSource::UserDefault);
            RegisterFile::photo(
                folder,
                format!("P{i:05}.jpg"),
                FileFacts {
                    size: i as u64 + 1,
                    mtime_ns: 1,
                    quick_hash: format!("{i:064x}"),
                },
                PhotoMetadata::default(),
                capture,
            )
        })
        .collect();
    for chunk in requests.chunks(1000) {
        cat.register_batch(chunk).unwrap();
    }
    cat.close().unwrap();
}

/// 既定のタイムゾーンを続けて変えても、撮影日時の推定し直しが最後の設定で終わる（古いオフセットの
/// ジョブが後から書いて上書きしない。指摘 F14）。
///
/// ジョブの追い越しはタイミングに依存するので、このテストは「必ず再現する」ものではない（修正の前は
/// 負荷が高いときに失敗しやすい）。結果は最後の状態で決める。
#[test]
fn rapid_time_zone_changes_end_with_the_last_offset() {
    const N: usize = 3000;
    let env = Env::new();
    let open = || {
        env.open_with(|c| {
            c.background_threads = Some(2);
        })
    };
    // 既定のタイムゾーン（+09:00）を保存してから、写真を登録する。
    open().close().unwrap();
    register_user_default_assets(&env.catalog_path(), N);
    let core = open();
    let offsets = [8 * 60, 7 * 60, 6 * 60, 5 * 60];
    for &m in &offsets {
        core.update_settings(&SettingsUpdate {
            default_utc_offset_minutes: Some(m),
            ..Default::default()
        })
        .unwrap();
    }
    core.wait_idle();
    let r = core
        .search(&SearchFilter::default(), SearchSort::default())
        .unwrap();
    let items = items_of(&core, r.generation, r.count);
    assert_eq!(items.len(), N);
    let wrong: Vec<_> = items
        .iter()
        .filter(|s| s.capture.tz_assumed.as_deref() != Some("+05:00"))
        .map(|s| s.capture.tz_assumed.clone())
        .collect();
    assert!(
        wrong.is_empty(),
        "{} 件が最後の設定（+05:00）になっていない: {:?}",
        wrong.len(),
        &wrong[..wrong.len().min(5)]
    );
    core.close().unwrap();
}

/// ドラッグでない変更が 1 秒未満の間隔で続いても、最初の未保存の変更から一定の時間
/// （`DEVELOP_SAVE_MAX_DELAY`）がたてば保存する（保存がいつまでも延びて、異常終了で長い時間の操作を
/// 失わないように。DATA-03a、指摘 F35）。自動保存の待ち時間は既定（1 秒）のまま。
#[test]
fn continuous_changes_are_saved_without_waiting_for_a_pause() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (48, 32), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    let history_len = || core.history(v).unwrap().len();
    assert_eq!(history_len(), 1, "読み込みだけ");
    let mut s = DevelopSettings::default();
    // 0.3 秒ごとに 3.3 秒間変え続ける（自動保存の待ち時間 1 秒より短い間隔）。
    for i in 1..=11 {
        s.contrast = i as f32;
        core.set_settings(&s, false).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    assert!(
        history_len() >= 2,
        "変え続けている間にも保存する: {:?}",
        core.history(v).unwrap()
    );
    core.close().unwrap();
}

/// キューで待っている P3 のジョブを取り消すと、記録が「取り消し」になり、`JobFinished` が届く
/// （[`genzo_api::Core::wait_job`] を呼ばなくても。イベントで完了を知る UI の進捗の表示が終わるように。
/// 指摘 F23）。
#[test]
fn cancelling_a_queued_job_finishes_it_and_sends_job_finished() {
    let env = Env::new();
    for i in 0..20u8 {
        write_jpeg(&env.photos.join(format!("P{i:02}.jpg")), (64, 48), i, None);
    }
    let core = env.open_with(|c| c.background_threads = Some(1));
    let rx = core.subscribe();
    // 1 本だけの P3 のスレッドを取り込みで使い、次のジョブをキューで待たせる。
    let j1 = core.import_folder(&env.photos, true).unwrap();
    let j2 = core.check_integrity().unwrap();
    core.cancel_job(j2).unwrap();
    // wait_job を呼ばずに、イベントだけで待つ。
    let deadline = std::time::Instant::now() + common::WAIT_LIMIT;
    let mut finished = std::collections::HashMap::new();
    while !(finished.contains_key(&j1) && finished.contains_key(&j2)) {
        let rest = deadline
            .checked_duration_since(std::time::Instant::now())
            .expect("JobFinished が届かない");
        match rx.recv_timeout(rest) {
            Ok(Event::JobFinished { job_id, state, .. }) => {
                finished.insert(job_id, state);
            }
            Ok(_) => {}
            Err(e) => panic!("JobFinished が届かない（{finished:?}）: {e}"),
        }
    }
    assert_eq!(finished[&j2], JobState::Cancelled);
    assert_eq!(core.job(j2).unwrap().state, JobState::Cancelled);
    core.wait_idle();
    assert_eq!(core.running_jobs(), 0);
    core.close().unwrap();
}

/// 写真を次々に開くと、途中の要求は取り消して最新の要求だけを処理する（04 の 6.1 節の P0。指摘 F26）。
///
/// 対話用のワーカーの起動を遅らせて（起動の間は展開が終わらない）、1 枚目を開いている途中で 2 枚目を
/// 開く。1 枚目は取り消しで戻り、2 枚目が現像のセッションになる。
#[test]
fn opening_another_photo_cancels_the_pending_open() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    write_jpeg(&env.photos.join("B.jpg"), (64, 48), 2, None);
    let refuse: PathBuf = env.dir.path().join("refuse-worker");
    let slow: PathBuf = env.dir.path().join("slow-worker");
    let core = std::sync::Arc::new(env.open_with(|c| {
        c.worker_env
            .push((ENV_REFUSE_IF.into(), refuse.clone().into_os_string()));
        c.worker_env
            .push((ENV_SLOW_START_IF.into(), slow.clone().into_os_string()));
    }));
    import(&core, &env.photos);
    let items = all_items(&core);
    let (a, b) = (
        by_name(&items, "A.jpg").variant_id,
        by_name(&items, "B.jpg").variant_id,
    );
    // 対話用のワーカーを止め（起動し直しは拒ませる）、次の起動を遅らせる。
    std::fs::write(&refuse, b"x").unwrap();
    let _ = core.run_worker_test_hook(Lane::Interactive, TestHook::Crash { with_shm: false });
    std::fs::remove_file(&refuse).unwrap();
    std::fs::write(&slow, b"x").unwrap();
    let first = {
        let core = std::sync::Arc::clone(&core);
        std::thread::spawn(move || core.open_develop(a))
    };
    // 1 枚目はワーカーの起動（3 秒）を待っている。
    std::thread::sleep(std::time::Duration::from_millis(500));
    std::fs::remove_file(&slow).unwrap();
    let opened = core.open_develop(b).unwrap();
    assert_eq!(opened.variant_id, b);
    let first = first.join().unwrap();
    match &first {
        Err(e) => assert!(e.is_cancelled(), "{e:?}"),
        Ok(s) => panic!("途中の要求は取り消す: {:?}", s.variant_id),
    }
    assert_eq!(core.develop_state().unwrap().unwrap().variant_id, b);
    core.close().unwrap();
}

/// LibRaw が有効なら、カメラ行列のない合成 DNG（ColorMatrix なし・LibRaw の表にない機種）を書く。
fn make_dng_without_matrix(path: &Path) -> bool {
    if !genzo_raw::LIBRAW_ENABLED {
        eprintln!("LibRaw が無効な build のため、DNG の部分を飛ばす");
        return false;
    }
    use genzo_testkit::bayer::{BayerParams, mosaic};
    use genzo_testkit::image::FloatImage;
    let (w, h) = (64u32, 48u32);
    let camera = FloatImage::from_rgb_fn(w, h, |x, y| {
        [
            0.05 + 0.5 * x as f32 / w as f32,
            0.05 + 0.4 * y as f32 / h as f32,
            0.2,
        ]
    })
    .unwrap();
    let params = BayerParams {
        cam_xyz: None,
        black_level: [512.0; 4],
        ..BayerParams::default()
    };
    let raw = mosaic(&camera, &params).unwrap();
    genzo_raw::dng::write_file(path, &raw, &genzo_raw::dng::DngOptions::default()).unwrap();
    true
}

/// カメラ行列のない RAW を書き出す・プレビューを作り直すと、結果に現像の警告（表示用の説明）が載り、
/// 警告のイベントでも知らせる（「書き出した」とだけ報告しない。指摘 F27）。
#[test]
fn export_and_regeneration_report_render_warnings() {
    let env = Env::new();
    if !make_dng_without_matrix(&env.photos.join("NOMAT.dng")) {
        return;
    }
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    let rx = core.subscribe();
    let out = env.dir.path().join("out");
    let job = core
        .export(&[v], &genzo_api::ExportSettings::default(), &out)
        .unwrap();
    let report = match common::wait_ok(&core, job) {
        genzo_api::JobResult::Export(r) => r,
        other => panic!("{other:?}"),
    };
    match &report.items[0].outcome {
        genzo_api::ExportOutcome::Written { warnings, .. } => {
            assert!(
                warnings.iter().any(|w| w.contains("カメラ行列")),
                "{warnings:?}"
            );
        }
        other => panic!("{other:?}"),
    }
    let job = core.regenerate_previews(&[v]).unwrap();
    let regen = match common::wait_ok(&core, job) {
        genzo_api::JobResult::RegeneratePreviews(r) => r,
        other => panic!("{other:?}"),
    };
    assert_eq!(regen.rendered, 1, "{regen:?}");
    assert!(
        regen
            .warnings
            .iter()
            .any(|w| w.variant_id == v && w.reason.contains("カメラ行列")),
        "{regen:?}"
    );
    let events = common::drain(&rx);
    let render_warnings = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::Warning {
                    code: genzo_api::WarningCode::Render,
                    message,
                    ..
                } if message.contains("カメラ行列")
            )
        })
        .count();
    assert!(render_warnings >= 2, "{events:?}");
    core.close().unwrap();
}

/// ゴミ箱へ移す直前に、ファイルがカタログに登録したものと同じかを確かめる（登録の後に同じ名前の別の
/// ファイルに置き換わっていれば移さず、その写真をカタログに残す。指摘 F32）。
#[test]
fn trash_refuses_a_file_replaced_after_registration() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (32, 24), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    // アプリの外で消して、別のカードの同じ名前の写真をコピーした。
    std::fs::remove_file(env.photos.join("A.jpg")).unwrap();
    write_jpeg(&env.photos.join("A.jpg"), (40, 30), 9, None);
    let plan = core.plan_delete(DeleteKind::Trash, &[a]).unwrap();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(report.failed[0].reason.contains("登録"), "{report:?}");
    assert!(report.removed_assets.is_empty());
    assert!(env.trash.moved.lock().is_empty(), "移さない");
    assert!(env.photos.join("A.jpg").is_file());
    assert_eq!(all_items(&core).len(), 1, "写真はカタログに残る");
    core.close().unwrap();
}

/// 更新日時を設定する（ファイルシステムの時刻の細かさによらず、同じ値・違う値に決めるため）。
fn set_mtime(path: &Path, t: std::time::SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

/// クイックハッシュが読む先頭と末尾の 64KiB（合わせて 128KiB）より大きい、ノイズの JPEG を書き、更新日時を
/// `mtime` にする。
fn write_large_jpeg(path: &Path, mtime: std::time::SystemTime) {
    let (w, h) = (320u32, 240u32);
    // 決まった列のノイズ（xorshift）。圧縮が効かず、ファイルが大きくなる。
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let raw: Vec<u8> = (0..w * h * 3)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 32) as u8
        })
        .collect();
    let rgb = genzo_media::RgbImage8::from_raw(w, h, raw).unwrap();
    let bytes = genzo_media::jpeg::encode_jpeg(&rgb, 92, None, None).unwrap();
    assert!(
        bytes.len() > 128 * 1024 + 4096,
        "中間のあるファイルにする: {} バイト",
        bytes.len()
    );
    std::fs::write(path, bytes).unwrap();
    set_mtime(path, mtime);
}

/// ファイルの中間の 1 バイトだけを書き換える（サイズも先頭と末尾の 64KiB も同じなので、クイックハッシュ
/// は同じになる）。更新日時は `mtime` にする。
fn rewrite_middle(path: &Path, mtime: std::time::SystemTime) {
    let mut bytes = std::fs::read(path).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    std::fs::write(path, &bytes).unwrap();
    set_mtime(path, mtime);
}

/// 中間だけの書き換えは、クイックハッシュでは見分けられず、全体のハッシュなら見分けられる（テストの前提）。
fn assert_only_the_middle_differs(path: &Path, before: &FileFacts, full_before: &str) {
    let now = FileFacts::read(path).unwrap();
    assert_eq!(now.size, before.size);
    assert_eq!(now.quick_hash, before.quick_hash, "クイックハッシュは同じ");
    assert_ne!(genzo_catalog::full_hash(path).unwrap(), full_before);
}

/// カタログに、ファイルの全体のハッシュを記録する（取り込みは記録しない。再リンクの確定・コピーの検証で
/// 計算した値を記録する。04 の 3.3 節）。`Core` を閉じているときに呼ぶ。
fn store_full_hash(env: &Env, path: &Path) {
    let mut cat = genzo_catalog::Catalog::open(env.catalog_path()).unwrap();
    let ids = cat.all_variant_ids().unwrap();
    let assets = cat.assets_of_variants(&ids).unwrap();
    let plan = cat.trash_plan(&assets).unwrap();
    assert_eq!(plan.files.len(), 1);
    cat.set_full_hash(
        plan.files[0].file_id,
        &genzo_catalog::full_hash(path).unwrap(),
    )
    .unwrap();
    cat.close().unwrap();
}

/// ゴミ箱へ移す直前の確認は、サイズとクイックハッシュ（大きなファイルの中間を含まない）だけでは足りない:
/// 登録の後に、サイズも先頭と末尾の 64KiB も同じまま、中間だけが書き換わったファイルは移さない
/// （全体のハッシュを登録していない場合は、更新日時が登録と違えば同じファイルと確かめられない）。
#[test]
fn trash_refuses_a_file_whose_middle_was_rewritten_after_registration() {
    let env = Env::new();
    let path = env.photos.join("A.jpg");
    let t0 = std::time::SystemTime::now() - std::time::Duration::from_secs(1000);
    write_large_jpeg(&path, t0);
    let before = FileFacts::read(&path).unwrap();
    let full_before = genzo_catalog::full_hash(&path).unwrap();
    let core = env.open();
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    // アプリの外で、中間だけ書き換えた（更新日時は新しくなった）。
    rewrite_middle(&path, t0 + std::time::Duration::from_secs(500));
    assert_only_the_middle_differs(&path, &before, &full_before);
    let plan = core.plan_delete(DeleteKind::Trash, &[a]).unwrap();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(report.failed[0].reason.contains("登録"), "{report:?}");
    assert!(report.removed_assets.is_empty(), "{report:?}");
    assert!(env.trash.moved.lock().is_empty(), "移さない");
    assert!(path.is_file());
    assert_eq!(all_items(&core).len(), 1, "写真はカタログに残る");
    core.close().unwrap();
}

/// 全体のハッシュがカタログにあれば、更新日時まで元に戻して中間だけ書き換えたファイルも、全体のハッシュで
/// 見分けて移さない。内容が同じなら、更新日時が違っていても移す（全体のハッシュが決め手）。
#[test]
fn trash_compares_the_full_hash_when_the_catalog_has_one() {
    let env = Env::new();
    let path = env.photos.join("A.jpg");
    let t0 = std::time::SystemTime::now() - std::time::Duration::from_secs(1000);
    write_large_jpeg(&path, t0);
    let before = FileFacts::read(&path).unwrap();
    let full_before = genzo_catalog::full_hash(&path).unwrap();
    {
        let core = env.open();
        import(&core, &env.photos);
        core.close().unwrap();
    }
    store_full_hash(&env, &path);
    let core = env.open();
    let a = all_items(&core)[0].variant_id;
    // 中間だけ書き換え、更新日時も登録の時と同じにした（サイズ・更新日時・クイックハッシュが同じ）。
    rewrite_middle(&path, t0);
    assert_only_the_middle_differs(&path, &before, &full_before);
    assert_eq!(FileFacts::read(&path).unwrap().mtime_ns, before.mtime_ns);
    let plan = core.plan_delete(DeleteKind::Trash, &[a]).unwrap();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(
        report.failed[0].reason.contains("全体のハッシュ"),
        "{report:?}"
    );
    assert!(report.removed_assets.is_empty(), "{report:?}");
    assert!(env.trash.moved.lock().is_empty(), "移さない");
    assert_eq!(all_items(&core).len(), 1, "写真はカタログに残る");
    // 内容を戻した（更新日時は違う）: 全体のハッシュが同じなので移す。
    rewrite_middle(&path, t0 + std::time::Duration::from_secs(500));
    assert_eq!(genzo_catalog::full_hash(&path).unwrap(), full_before);
    let plan = core.plan_delete(DeleteKind::Trash, &[a]).unwrap();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(report.trashed_files.len(), 1, "{report:?}");
    assert_eq!(env.trash.moved.lock().len(), 1);
    assert!(all_items(&core).is_empty());
    core.close().unwrap();
}

/// 全体のハッシュを登録していない場合、更新日時だけが登録と違うファイル（内容は同じ）は、同じファイルと
/// 確かめられないので移さない。ファイルの確認（`check_files`）で登録し直した後なら移せる。
#[test]
fn trash_needs_check_files_when_only_the_modified_time_differs() {
    let env = Env::new();
    let path = env.photos.join("A.jpg");
    let t0 = std::time::SystemTime::now() - std::time::Duration::from_secs(1000);
    write_large_jpeg(&path, t0);
    let core = env.open();
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    set_mtime(&path, t0 + std::time::Duration::from_secs(500));
    let plan = core.plan_delete(DeleteKind::Trash, &[a]).unwrap();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(
        report.failed[0].reason.contains("check_files"),
        "{report:?}"
    );
    assert!(env.trash.moved.lock().is_empty(), "移さない");
    // 確認して登録し直した（更新日時が今の値になる）後は、移せる。
    let check = core.check_files(&[a]).unwrap();
    assert_eq!(check.changed.len(), 1, "{check:?}");
    let plan = core.plan_delete(DeleteKind::Trash, &[a]).unwrap();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(env.trash.moved.lock().len(), 1);
    core.close().unwrap();
}

/// クイックハッシュがファイルの全体を覆う小さなファイル（128KiB 以下）は、クイックハッシュが同じなら内容が
/// 同じなので、更新日時が違っていても（全体のハッシュを登録していなくても）移す。
#[test]
fn trash_accepts_a_small_file_with_a_different_modified_time() {
    let env = Env::new();
    let path = env.photos.join("A.jpg");
    write_jpeg(&path, (32, 24), 1, None);
    assert!(std::fs::metadata(&path).unwrap().len() <= 128 * 1024);
    let t0 = std::fs::metadata(&path).unwrap().modified().unwrap();
    let core = env.open();
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    set_mtime(&path, t0 + std::time::Duration::from_secs(500));
    let plan = core.plan_delete(DeleteKind::Trash, &[a]).unwrap();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(env.trash.moved.lock().len(), 1);
    core.close().unwrap();
}

/// ゴミ箱への移動で、元の場所が見えない（ドライブを外した・フォルダごと見えない）ファイルは「移した」
/// 扱いにせず、失敗にしてカタログに残す。フォルダはあってファイルだけがない（アプリの外で削除された）
/// 場合は、移すものがないとして報告する（指摘 F06）。
#[test]
fn trash_does_not_drop_photos_on_an_unreachable_volume() {
    let env = Env::new();
    write_jpeg(&env.photos.join("vol").join("A.jpg"), (32, 24), 1, None);
    write_jpeg(&env.photos.join("here").join("B.jpg"), (32, 24), 2, None);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg").variant_id;
    let b = by_name(&items, "B.jpg").variant_id;
    // A のフォルダごと見えなくする（ドライブを外した状態の代わり）。
    std::fs::rename(env.photos.join("vol"), env.photos.join("vol-offline")).unwrap();
    let plan = core.plan_delete(DeleteKind::Trash, &[a]).unwrap();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(report.removed_assets.is_empty(), "{report:?}");
    assert_eq!(all_items(&core).len(), 2, "A はカタログに残る");
    // B はアプリの外で削除された（フォルダはある）: 移すものはなく、カタログから除いて報告する。
    std::fs::remove_file(env.photos.join("here").join("B.jpg")).unwrap();
    let plan = core.plan_delete(DeleteKind::Trash, &[b]).unwrap();
    let report = core.execute_delete(plan.plan_id).unwrap();
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(report.removed_assets.len(), 1);
    assert_eq!(report.skipped_missing.len(), 1, "{report:?}");
    assert!(env.trash.moved.lock().is_empty());
    core.close().unwrap();
}

/// 起動時のファイル操作の確定で、元のフォルダごと見えないファイル（ドライブを外した）があれば、確定を
/// 保留する（「ゴミ箱へ移した」として写真をカタログから除かない。指摘 F06）。
#[test]
fn unfinished_trash_on_an_unreachable_volume_is_left_pending() {
    let env = Env::new();
    write_jpeg(&env.photos.join("vol").join("A.jpg"), (32, 24), 1, None);
    {
        let core = env.open();
        import(&core, &env.photos);
        core.close().unwrap();
    }
    // ゴミ箱への移動を始めたところで終了し、その後ドライブを外した。
    {
        let mut cat = genzo_catalog::Catalog::open(env.catalog_path()).unwrap();
        let ids = cat.all_variant_ids().unwrap();
        let assets = cat.assets_of_variants(&ids).unwrap();
        let (op, _) = cat.plan_trash(&assets).unwrap();
        cat.start_file_op(op).unwrap();
        cat.close().unwrap();
    }
    std::fs::rename(env.photos.join("vol"), env.photos.join("vol-offline")).unwrap();
    let core = env.open();
    assert_eq!(core.startup_report().recovered_file_ops, 0, "確定しない");
    assert!(
        core.startup_warnings().iter().any(|e| matches!(
            e,
            Event::Warning {
                code: WarningCode::FileOperationRecovered,
                message,
                ..
            } if message.contains("保留")
        )),
        "{:?}",
        core.startup_warnings()
    );
    assert_eq!(all_items(&core).len(), 1, "写真はカタログに残る");
    core.close().unwrap();
    // ドライブをつなぐと、次の起動で確定する（ファイルは元の場所にある: 移していない）。
    std::fs::rename(env.photos.join("vol-offline"), env.photos.join("vol")).unwrap();
    let core = env.open();
    assert_eq!(core.startup_report().recovered_file_ops, 1);
    assert_eq!(all_items(&core).len(), 1);
    core.close().unwrap();
}

/// データのフォルダの中で、名前が `prefix` で始まるファイルの名前。
fn files_starting_with(dir: &Path, prefix: &str) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix))
        .collect();
    out.sort();
    out
}

/// 作り直せるキャッシュ（thumbs.db）を開けないときは、退避して作り直し、カタログは開く（カタログの
/// 復元を案内しない。DATA-03b、指摘 F05）。先頭を壊す・1 ページ目を壊す・新しい版のスキーマの 3 通り。
#[test]
fn a_broken_thumbnail_db_is_moved_aside_and_rebuilt() {
    type Damage = fn(&mut Vec<u8>);
    let cases: [(&str, Damage); 3] = [
        ("先頭", |b| b[..16].fill(0x55)),
        ("1 ページ目", |b| b[100..2100].fill(0xA5)),
        // SQLite のヘッダーのオフセット 60 は user_version（ビッグエンディアン）。
        ("新しい版", |b| {
            b[60..64].copy_from_slice(&99u32.to_be_bytes())
        }),
    ];
    for (what, damage) in cases {
        let env = Env::new();
        write_jpeg(&env.photos.join("A.jpg"), (32, 24), 1, None);
        let core = env.open();
        import(&core, &env.photos);
        core.close().unwrap();
        let data = env.dir.path().join("data");
        let thumbs = data.join("thumbs.db");
        let mut bytes = std::fs::read(&thumbs).unwrap();
        damage(&mut bytes);
        std::fs::write(&thumbs, &bytes).unwrap();
        let core = env.open_with(|_| {});
        assert!(
            core.startup_warnings().iter().any(|e| matches!(
                e,
                Event::Warning {
                    code: WarningCode::CacheRebuilt,
                    ..
                }
            )),
            "{what}: {:?}",
            core.startup_warnings()
        );
        let moved = files_starting_with(&data, "thumbs.db.broken-");
        assert!(
            !moved.is_empty(),
            "{what}: 壊れたファイルは消さずに退避する"
        );
        // カタログはそのまま使える。サムネイルは作り直せる。
        let items = all_items(&core);
        assert_eq!(items.len(), 1, "{what}");
        let job = core.regenerate_previews(&[items[0].variant_id]).unwrap();
        common::wait_ok(&core, job);
        assert!(
            core.thumbnail(items[0].variant_id).unwrap().is_some(),
            "{what}"
        );
        core.close().unwrap();
    }
}

/// プレビューのフォルダ（設定した場所）を使えないときは、既定の場所に切り替えて開く（設定を変えられる
/// ように、起動ごと失敗しない。指摘 F05）。
#[test]
fn an_unusable_preview_folder_does_not_prevent_opening() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (32, 24), 1, None);
    let blocker = env.dir.path().join("not-a-dir");
    std::fs::write(&blocker, b"x").unwrap();
    let core = env.open();
    import(&core, &env.photos);
    core.update_settings(&SettingsUpdate {
        preview_cache_dir: Some(blocker.join("previews")),
        ..Default::default()
    })
    .unwrap();
    core.close().unwrap();
    let core = env.open();
    assert!(
        core.startup_warnings().iter().any(|e| matches!(
            e,
            Event::Warning {
                code: WarningCode::CacheRebuilt,
                ..
            }
        )),
        "{:?}",
        core.startup_warnings()
    );
    // 設定を直せる。
    let default_dir = env.dir.path().join("data").join("previews");
    core.update_settings(&SettingsUpdate {
        preview_cache_dir: Some(default_dir.clone()),
        ..Default::default()
    })
    .unwrap();
    let v = all_items(&core)[0].variant_id;
    let job = core.regenerate_previews(&[v]).unwrap();
    common::wait_ok(&core, job);
    let l1 = core
        .preview_path(v)
        .unwrap()
        .expect("L1 は既定の場所に作る");
    assert!(l1.starts_with(&default_dir), "{l1:?}");
    core.close().unwrap();
}

/// データのフォルダの外にあるプレビューのキャッシュ（L1）の場所を設定で変えても、変えた後の取り込みが、
/// 今使っている（次に開くまで開いたままの）古い場所の JPEG を写真として登録しない。キャッシュは開き直さない
/// ので、設定に記録された新しい場所だけを除外すると、古い場所の生成済みの JPEG が除外から漏れていた
/// （Codex のレビュー P2）。設定に記録された新しい場所（次に開いたときに使う）も除外する。
#[test]
fn changing_the_preview_folder_keeps_excluding_the_one_in_use_from_import() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (32, 24), 1, None);
    // データのフォルダの外の、1 つのフォルダの下に、古い場所と新しい場所を並べる（親を再帰で取り込む）。
    let external = env.dir.path().join("external");
    let old_dir = external.join("old-previews");
    let new_dir = external.join("new-previews");
    // 1. 古い場所を設定し、開き直して使う（キャッシュが古い場所で開く）。
    let core = env.open();
    core.update_settings(&SettingsUpdate {
        preview_cache_dir: Some(old_dir.clone()),
        ..Default::default()
    })
    .unwrap();
    core.close().unwrap();
    let core = env.open();
    assert_eq!(core.settings().preview_cache_dir, old_dir);
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    let job = core.regenerate_previews(&[v]).unwrap();
    common::wait_ok(&core, job);
    let l1 = core.preview_path(v).unwrap().expect("L1 を作る");
    assert!(l1.starts_with(&old_dir), "{l1:?}");
    assert!(l1.is_file());
    // 2. 開いたまま、設定を新しい場所に変える（キャッシュは古い場所のまま。新しい場所は次に開いたときに使う）。
    //    新しい場所には、キャッシュのファイルに見える JPEG を置いておく。
    core.update_settings(&SettingsUpdate {
        preview_cache_dir: Some(new_dir.clone()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(core.settings().preview_cache_dir, new_dir);
    write_jpeg(
        &new_dir.join("ab").join("cd").join("next.jpg"),
        (32, 24),
        2,
        None,
    );
    // 3. 親のフォルダを再帰で取り込んでも、古い場所・新しい場所のどちらの JPEG も登録しない。
    let report = import(&core, &external);
    assert_eq!(report.files_found, 0, "{report:?}");
    assert_eq!(report.added, 0, "{report:?}");
    assert!(report.not_registered.is_empty(), "{report:?}");
    let items = all_items(&core);
    assert_eq!(
        common::names(&items),
        vec!["A.jpg".to_owned()],
        "プレビューの JPEG を写真として登録しない"
    );
    // 4. 古い場所・新しい場所そのものを指定した取り込みは、エラーにする。
    for dir in [&old_dir, &new_dir] {
        let job = core.import_folder(dir, true).unwrap();
        let info = common::wait_done(&core, job);
        assert_eq!(info.state, JobState::Failed, "{dir:?}: {info:?}");
    }
    assert_eq!(all_items(&core).len(), 1);
    // 元の写真は変わらず、L1 のファイルも残っている。
    assert!(l1.is_file());
    core.close().unwrap();
}

/// 復元の後（ID が再利用される）や、データのフォルダを残したまま新しいカタログを作った後に、前の写真の
/// サムネイル・プレビューを別の写真のものとして使わない（指摘 F07）。
#[test]
fn caches_from_before_a_restore_or_another_catalog_are_not_reused() {
    let env = Env::new();
    // 1. 空のカタログのバックアップを取ってから、写真を取り込む（variant 1 のサムネイルができる）。
    let core = env.open();
    let backup = core.backup_now().unwrap();
    write_jpeg(&env.photos.join("G.jpg"), (32, 24), 1, None);
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    let old = core.thumbnail(v).unwrap().expect("サムネイルがある");
    core.close().unwrap();
    // 2. 空のカタログに復元する。
    let plan = genzo_api::prepare_restore(&backup.path, &env.catalog_path()).unwrap();
    genzo_api::apply_restore(&plan).unwrap();
    // 3. 読めない写真を取り込む（同じ ID が再利用される。サムネイルは作らない）。
    std::fs::remove_file(env.photos.join("G.jpg")).unwrap();
    std::fs::write(env.photos.join("broken.jpg"), b"not a jpeg").unwrap();
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].variant_id, v, "ID が再利用される前提");
    assert_eq!(items[0].file_status, FileStatus::Error);
    assert!(
        core.thumbnail(v)
            .unwrap()
            .is_none_or(|t| t.jpeg != old.jpeg),
        "前の写真のサムネイルを使わない"
    );
    assert!(core.preview_path(v).unwrap().is_none());
    core.close().unwrap();
    // 4. データのフォルダを残したまま、別のカタログを作る。
    let moved = env.dir.path().join("old-catalog.db");
    std::fs::rename(env.catalog_path(), &moved).unwrap();
    std::fs::write(env.photos.join("broken.jpg"), b"still not a jpeg").unwrap();
    write_jpeg(&env.photos.join("H.jpg"), (32, 24), 7, None);
    let core = env.open();
    // 新しいカタログでは、取り込む前にサムネイルはない。
    for id in 1..=3i64 {
        assert!(
            core.thumbnail(genzo_api::VariantId::new(id))
                .unwrap()
                .is_none(),
            "variant {id}"
        );
    }
    core.close().unwrap();
}

/// 現像設定を変えた直後に終了しても、L0 / L1 の作り直しは次の起動で行う（作り直しの依頼は終了で取り消され
/// るため、印を残す。指摘 F11）。設定の保存と一括適用の両方。
#[test]
fn regeneration_cancelled_by_close_is_done_after_reopening() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (2000, 1500), 1, None);
    write_jpeg(&env.photos.join("B.jpg"), (2000, 1500), 2, None);
    let core = env.open();
    import(&core, &env.photos);
    core.wait_idle();
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg").variant_id;
    let b = by_name(&items, "B.jpg").variant_id;
    let rev_a = core.thumbnail(a).unwrap().unwrap().rev;
    let rev_b = core.thumbnail(b).unwrap().unwrap().rev;
    core.open_develop(a).unwrap();
    let mut s = DevelopSettings {
        exposure_ev: 2.0,
        ..Default::default()
    };
    core.set_settings(&s, false).unwrap();
    s.exposure_ev = -1.5;
    core.paste_settings(&s, &[b], genzo_api::SettingGroups::default())
        .unwrap();
    // すぐに閉じる（保存待ちの設定は保存され、作り直しの依頼は取り消される）。
    core.close().unwrap();
    let core = env.open();
    core.wait_idle();
    assert_eq!(core.develop_settings(a).unwrap().exposure_ev, 2.0);
    assert_ne!(
        core.thumbnail(a).unwrap().unwrap().rev,
        rev_a,
        "編集の後の見た目で作り直す"
    );
    assert_ne!(core.thumbnail(b).unwrap().unwrap().rev, rev_b);
    assert!(core.preview_path(a).unwrap().is_some());
    core.close().unwrap();
}

/// 一様な色の PNG を書き、更新日時を `mtime_offset_s` 秒ずらす（ファイルシステムの時刻の細かさによらず、
/// 変化を検知させるため）。
fn write_uniform_png(path: &Path, (w, h): (u32, u32), value: u8, mtime_offset_s: i64) {
    image::RgbImage::from_pixel(w, h, image::Rgb([value, value, value]))
        .save(path)
        .unwrap();
    let base = std::time::SystemTime::now();
    let t = if mtime_offset_s >= 0 {
        base + std::time::Duration::from_secs(mtime_offset_s as u64)
    } else {
        base - std::time::Duration::from_secs((-mtime_offset_s) as u64)
    };
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

/// 現像のプレビューの平均（画面の表示用の 8bit）。
fn frame_mean(core: &genzo_api::Core) -> f64 {
    core.render_develop().unwrap();
    let f = core.develop_frame().unwrap();
    let px = &f.display_rgb8;
    px.iter().map(|&v| f64::from(v)).sum::<f64>() / px.len() as f64
}

/// 現像を開く・作り直すときに、カタログの revision・missing の状態を実際のファイルと照合する（他のアプリで
/// 上書きされた内容を古い A1 から描かない。missing から戻ったファイルを ok に戻す。指摘 F12）。
#[test]
fn opening_checks_the_file_against_the_catalog() {
    let env = Env::new();
    let a_path = env.photos.join("A.png");
    write_uniform_png(&a_path, (64, 48), 40, -100);
    let core = env.open();
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    core.open_develop(a).unwrap();
    let dark = frame_mean(&core);
    core.close_develop().unwrap();
    // 他のアプリで上書き保存した（同じ寸法の明るい画像）。
    write_uniform_png(&a_path, (64, 48), 210, -50);
    core.open_develop(a).unwrap();
    let bright = frame_mean(&core);
    assert!(
        bright > dark + 50.0,
        "新しい内容で描く（前 {dark:.1}、後 {bright:.1}）"
    );
    let item = &all_items(&core)[0];
    assert_eq!(item.file_revision, 2, "revision を上げる");
    core.close_develop().unwrap();
    // 見つからない → 戻った: 開くと ok に戻る。
    let moved = env.dir.path().join("A.moved.png");
    std::fs::rename(&a_path, &moved).unwrap();
    assert!(core.open_develop(a).is_err());
    assert_eq!(all_items(&core)[0].file_status, FileStatus::Missing);
    std::fs::rename(&moved, &a_path).unwrap();
    core.open_develop(a).unwrap();
    assert_eq!(all_items(&core)[0].file_status, FileStatus::Ok);
    core.close().unwrap();
}

/// ファイルの確認（check_files）で内容の変化を検知したら、メタデータ（寸法・撮影日時）も読み直す。読めない
/// ファイルを、読めることを確かめずに ok にしない（指摘 F13）。
#[test]
fn check_files_rereads_metadata_of_changed_files() {
    let env = Env::new();
    let a_path = env.photos.join("A.jpg");
    write_jpeg(
        &a_path,
        (32, 24),
        1,
        Some(&exif(Some("2024-05-01 10:00:00"), Some(540), None)),
    );
    std::fs::write(env.photos.join("B.jpg"), b"not a jpeg").unwrap();
    std::fs::write(env.photos.join("C.jpg"), b"not a jpeg either").unwrap();
    let core = env.open();
    import(&core, &env.photos);
    // A を別の寸法・撮影日時で上書き、B を正しい JPEG に置き換え、C を別の読めない内容にする。
    write_jpeg(
        &a_path,
        (40, 30),
        2,
        Some(&exif(Some("2025-01-02 12:34:56"), Some(540), None)),
    );
    write_jpeg(
        &env.photos.join("B.jpg"),
        (40, 30),
        3,
        Some(&exif(Some("2023-03-03 03:03:03"), Some(540), None)),
    );
    std::fs::write(env.photos.join("C.jpg"), b"still not a jpeg, longer").unwrap();
    let ids: Vec<_> = all_items(&core).iter().map(|s| s.variant_id).collect();
    let report = core.check_files(&ids).unwrap();
    assert_eq!(report.changed.len(), 3, "{report:?}");
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg");
    assert_eq!((a.width, a.height), (Some(40), Some(30)), "{a:?}");
    assert_eq!(
        a.capture.raw.as_deref(),
        Some("2025:01:02 12:34:56"),
        "{:?}",
        a.capture
    );
    let b = by_name(&items, "B.jpg");
    assert_eq!(b.file_status, FileStatus::Ok);
    assert_eq!((b.width, b.height), (Some(40), Some(30)));
    assert!(b.capture.utc.is_some());
    let c = by_name(&items, "C.jpg");
    assert_eq!(
        c.file_status,
        FileStatus::Error,
        "読めないファイルは error のまま"
    );
    // 取り込み直しても、同じ結果のまま。
    let again = import(&core, &env.photos);
    assert_eq!((again.added, again.updated), (0, 0), "{again:?}");
    let items = all_items(&core);
    assert_eq!(
        (
            by_name(&items, "A.jpg").width,
            by_name(&items, "A.jpg").height
        ),
        (Some(40), Some(30))
    );
    assert_eq!(by_name(&items, "C.jpg").file_status, FileStatus::Error);
    core.close().unwrap();
}

/// 同じカタログを 2 つ同時に開けない（1 つのカタログは 1 つのアプリだけが開く。CLI とアプリが同時に
/// 開くと、検索の世代や現像のセッションの前提が崩れる。K2）。閉じれば開ける。開いている間は復元の
/// 差し替えもしない。
#[test]
fn a_catalog_cannot_be_opened_twice_at_the_same_time() {
    let env = Env::new();
    let first = env.open();
    let second = genzo_api::Core::open(env.config());
    match second {
        Err(e) => {
            assert_eq!(e.kind(), genzo_api::ErrorKind::Conflict, "{e:?}");
            assert!(e.info().hint.is_some());
        }
        Ok(_) => panic!("2 つ目は開けない"),
    }
    // 開いている間は、復元の差し替えもしない。
    let backup = first.backup_now().unwrap();
    let plan = genzo_api::prepare_restore(&backup.path, &env.catalog_path()).unwrap();
    assert!(genzo_api::apply_restore(&plan).is_err());
    assert!(env.catalog_path().is_file(), "今のカタログはそのまま");
    first.close().unwrap();
    // 閉じた後は開ける（drop でも閉じる）。
    let again = env.open();
    drop(again);
    genzo_api::apply_restore(&plan).unwrap();
    env.open().close().unwrap();
}

/// 同じ既存のカタログを、シンボリックリンクの別名で開いても、同じカタログとして排他になる（ロックの
/// ファイルの名前を、渡されたパスの書き方ではなく、リンクを解決したパスから求める。以前は別名にも
/// `.lock` を付けたので別のロックになり、同じカタログを同時に開けた。正常終了の印も食い違った）。
/// 別名で開いている間は、本体の名前でも、復元の差し替えでも同じ。
#[cfg(unix)]
#[test]
fn a_catalog_cannot_be_opened_twice_through_a_symlink_alias() {
    use std::os::unix::fs::symlink;

    let env = Env::new();
    // 既存のカタログを作っておく（別名で開くのは、既にあるカタログ）。
    env.open().close().unwrap();
    let link_dir = env.dir.path().join("links");
    std::fs::create_dir_all(&link_dir).unwrap();
    let alias = link_dir.join("alias.db");
    symlink(env.catalog_path(), &alias).unwrap();
    let config_via_alias = || {
        let mut c = env.config();
        c.catalog_path = alias.clone();
        c
    };
    let assert_in_use = |r: Result<genzo_api::Core, genzo_api::ApiError>| match r {
        Err(e) => assert_eq!(e.kind(), genzo_api::ErrorKind::Conflict, "{e:?}"),
        Ok(_) => panic!("同じカタログの 2 つ目は開けない"),
    };

    // 本体の名前で開いている間は、別名で開けない。
    let first = env.open();
    assert_in_use(genzo_api::Core::open(config_via_alias()));
    // 開いている間は、別名を渡した復元の差し替えもしない（リンクの名前の変更もしない）。
    let backup = first.backup_now().unwrap();
    let plan = genzo_api::prepare_restore(&backup.path, &alias).unwrap();
    assert!(genzo_api::apply_restore(&plan).is_err());
    assert!(
        std::fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink(),
        "別名はそのまま"
    );
    first.close().unwrap();

    // 別名で開いている間は、本体の名前で開けない。
    let via_alias = genzo_api::Core::open(config_via_alias()).unwrap();
    assert_in_use(genzo_api::Core::open(env.config()));
    via_alias.close().unwrap();

    // 閉じた後は、どちらでも開ける。
    env.open().close().unwrap();
    genzo_api::Core::open(config_via_alias())
        .unwrap()
        .close()
        .unwrap();
}

/// キャッシュの回収（L1 のフォルダの走査に時間がかかる）の間も、ドラッグの終了（保存）が待たされない
/// （回収はカタログとキャッシュのロックの外でフォルダを走査し、現像の保存は現像のロックの外でカタログに
/// 書く。PERF-13、指摘 F24）。
///
/// 時間の長さは比べない（以前は「保存までの時間が、回収だけの時間の半分より短い」と比べていたが、
/// ディスクが忙しい環境や遅い CI で、保存の書き込みも遅れて不安定だった）。回収のジョブを、フォルダを
/// 走査する直前（`CoreHooks::before_preview_scan`。ロックの外）で止めておき、止めている間に保存が終わる
/// ことを確かめる。保存が回収の終わりを待つなら、保存は止まったまま終わらない（待つ上限を過ぎたら
/// 失敗にする。上限は失敗のときだけ使う）。
#[test]
fn saving_develop_settings_does_not_wait_for_cache_garbage_collection() {
    use crossbeam_channel::{bounded, unbounded};
    let env = Env::new();
    // 回収のジョブが走査の直前に着いたことの知らせと、ジョブを再開する合図（送り側が閉じても再開する）。
    let (reached_tx, reached_rx) = unbounded::<()>();
    let (resume_tx, resume_rx) = unbounded::<()>();
    let core = env.open_with_hooks(|hooks| {
        hooks.before_preview_scan = Some(std::sync::Arc::new(move || {
            let _ = reached_tx.send(());
            let _ = resume_rx.recv();
        }));
    });
    write_jpeg(&env.photos.join("A.jpg"), (48, 32), 1, None);
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    let s = DevelopSettings {
        exposure_ev: 0.5,
        ..Default::default()
    };
    // ドラッグ中（保存はドラッグの終わりまで待つ）。
    core.set_settings(&s, true).unwrap();
    let job = core.collect_cache_garbage().unwrap();
    reached_rx
        .recv_timeout(common::WAIT_LIMIT)
        .expect("回収のジョブが、走査の直前まで進まない");
    // 回収を止めたまま、ドラッグを終える（保存する）。保存の完了を、別のスレッドの結果で待つ。
    let (saved, job_state) = std::thread::scope(|scope| {
        let (done_tx, done_rx) = bounded(1);
        let core = &core;
        scope.spawn(move || {
            let _ = done_tx.send(core.end_drag().map(|_| ()));
        });
        let saved = done_rx.recv_timeout(common::WAIT_LIMIT);
        // 回収を再開する前の状態（保存が終わったときに、回収が終わっていないことの確認）。
        let job_state = core.job(job).map(|info| info.state);
        // 結果によらず再開する（止めたままだと、保存を待つスレッドも回収のジョブも終わらない）。
        let _ = resume_tx.send(());
        (saved, job_state)
    });
    saved
        .expect("保存が、止めておいた回収の終わりを待っている")
        .unwrap();
    assert_eq!(
        job_state.unwrap(),
        JobState::Running,
        "保存が終わったとき、回収は止まったまま（終わっていない）"
    );
    assert_eq!(core.history(v).unwrap().len(), 2);
    common::wait_ok(&core, job);
    core.close().unwrap();
}

/// 現像設定の render_deps（04 の 2.5 節、R-08）はアプリが決める: RAW 以外は空のまま、UI から送られた値では
/// 変わらない（K1）。
#[test]
fn render_deps_are_managed_by_the_app() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (48, 32), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    let mut s = core.develop_state().unwrap().unwrap().settings;
    s.exposure_ev = 0.5;
    s.render_deps.raw_decoder = "bogus-decoder".into();
    core.set_settings(&s, false).unwrap();
    core.flush_develop().unwrap();
    core.close_develop().unwrap();
    let stored = core.develop_settings(v).unwrap();
    assert_eq!(stored.exposure_ev, 0.5);
    assert_eq!(stored.render_deps, genzo_model::RenderDeps::default());
    core.close().unwrap();
}

/// RAW の現像設定を保存・書き出すと、実際に使ったカメラ行列（出どころと内容のハッシュ）と RAW デコーダを
/// 記録する。保存された値が空の写真は、開いたときに今の値で埋める（K1）。LibRaw が有効な build だけ。
#[test]
fn render_deps_of_raw_photos_are_recorded() {
    let env = Env::new();
    if !common::make_dng(&env.photos.join("R.dng"), "2024:05:01 08:00:00") {
        return;
    }
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    assert_eq!(
        core.develop_settings(v).unwrap().render_deps,
        genzo_model::RenderDeps::default(),
        "まだ記録していない"
    );
    let state = core.open_develop(v).unwrap();
    let deps = state.settings.render_deps.clone();
    assert!(deps.raw_decoder.starts_with("libraw"), "{deps:?}");
    assert_eq!(deps.camera_profile.sha256.len(), 64, "{deps:?}");
    assert!(!state.dirty, "開いただけでは保存待ちにしない");
    let mut s = state.settings;
    s.exposure_ev = 0.3;
    core.set_settings(&s, false).unwrap();
    core.close_develop().unwrap();
    assert_eq!(
        core.develop_settings(v).unwrap().render_deps,
        deps,
        "保存で記録する"
    );
    let out = env.dir.path().join("out");
    let job = core
        .export(&[v], &genzo_api::ExportSettings::default(), &out)
        .unwrap();
    match common::wait_ok(&core, job) {
        genzo_api::JobResult::Export(r) => match &r.items[0].outcome {
            genzo_api::ExportOutcome::Written { render_deps, .. } => {
                assert_eq!(render_deps.as_ref(), Some(&deps));
            }
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
    core.close().unwrap();
}

/// ffprobe がない状態で取り込んで `status = error` になった動画を、ffprobe が使えるようになってから
/// 取り込み直すと、内容が同じでもメタデータ（撮影日時・寸法・長さ）が入り、カタログの変更が知らされる
/// （指摘 F03）。
#[test]
fn reimporting_a_file_that_could_not_be_read_stores_its_metadata() {
    let env = Env::new();
    let clip = env.photos.join("clip.mp4");
    if !common::make_video(&clip, "2024-05-01T03:34:56Z") {
        return;
    }
    let missing_ffprobe = env.dir.path().join("no-such-ffprobe");
    let core = env.open_with(|c| {
        c.worker_env.push((
            genzo_media::video::ENV_FFPROBE.into(),
            missing_ffprobe.clone().into_os_string(),
        ));
    });
    let first = import(&core, &env.photos);
    assert_eq!((first.added, first.errors.len()), (1, 1), "{first:?}");
    let items = all_items(&core);
    assert_eq!(items[0].file_status, FileStatus::Error);
    assert!(items[0].capture.utc.is_none());
    core.close().unwrap();

    let core = env.open();
    let events = core.subscribe();
    let second = import(&core, &env.photos);
    assert!(second.errors.is_empty(), "{second:?}");
    assert_eq!(second.updated, 1, "{second:?}");
    assert!(
        common::drain(&events)
            .iter()
            .any(|e| matches!(e, Event::CatalogChanged { .. })),
        "error から ok に変わったことを知らせる"
    );
    let items = all_items(&core);
    let s = &items[0];
    assert_eq!(s.file_status, FileStatus::Ok);
    assert_eq!(
        s.capture.utc.map(|t| t.to_rfc3339()).as_deref(),
        Some("2024-05-01T03:34:56+00:00")
    );
    assert_eq!((s.width, s.height), (Some(96), Some(64)));
    let details = core.variant_details(s.variant_id).unwrap();
    let video = details.video.expect("動画の情報が入る");
    assert!(video.duration_s.unwrap() > 0.5, "{video:?}");
    // 3 回目は変化なし（解析を省く）。
    let third = import(&core, &env.photos);
    assert_eq!((third.unchanged, third.updated), (1, 0), "{third:?}");
    core.close().unwrap();
}

/// 時計が既存のバックアップより前に戻っていても、開いたときの自動バックアップが残る（作った直後に世代の
/// 管理で消さない）。次に開いたときは、今の状態のバックアップがあるので作り直さない（指摘 F09）。
#[test]
fn automatic_backups_survive_a_clock_set_back() {
    let env = Env::new();
    let core = env.open();
    let first = core
        .startup_report()
        .backup
        .expect("初めて開いたときに作る");
    let generations = core.config().backup_generations;
    core.close().unwrap();
    // 同じカタログの、未来の日時（2099 年 11 月）のバックアップを世代数だけ置く。
    let name = first.path.file_name().unwrap().to_str().unwrap().to_owned();
    let ts = first.created_at.format("%Y%m%dT%H%M%S%3fZ").to_string();
    assert!(name.contains(&ts), "{name}");
    let mut future = Vec::new();
    for day in 1..=generations {
        let p = first
            .path
            .with_file_name(name.replace(&ts, &format!("209911{day:02}T000000000Z")));
        std::fs::copy(&first.path, &p).unwrap();
        future.push(p);
    }
    std::fs::remove_file(&first.path).unwrap();

    let core = env.open();
    let made = core
        .startup_report()
        .backup
        .expect("今の状態のバックアップがないので作る");
    assert!(made.path.is_file(), "作ったバックアップが残っている");
    let list = core.list_backups().unwrap();
    assert_eq!(list.len(), generations);
    assert!(list.iter().any(|b| b.path == made.path));
    let manual = core.backup_now().unwrap();
    assert!(manual.path.is_file());
    core.close().unwrap();
    let core = env.open();
    assert_eq!(
        core.startup_report().backup,
        None,
        "今の状態のバックアップがあるので、開くたびには作らない"
    );
    core.close().unwrap();
}

/// 別々のフォルダの同じファイル名のカタログが、同じバックアップの保存先を使っても、互いのバックアップを
/// 消さず、自動バックアップも止まらず、一覧にも出さない（指摘 F10）。
#[test]
fn catalogs_with_the_same_name_can_share_a_backup_folder() {
    let shared_root = tempfile::tempdir().unwrap();
    let shared = shared_root.path().join("backups");
    let (env_a, env_b) = (Env::new(), Env::new());
    assert_eq!(
        env_a.catalog_path().file_name(),
        env_b.catalog_path().file_name()
    );
    let open = |env: &Env| {
        env.open_with(|c| {
            c.backup_dir = Some(shared.clone());
        })
    };
    let a = open(&env_a);
    let a_backups = vec![
        a.startup_report().backup.expect("A の自動バックアップ"),
        a.backup_now().unwrap(),
    ];
    a.close().unwrap();

    let b = open(&env_b);
    let b_first = b
        .startup_report()
        .backup
        .expect("A のバックアップがあっても、B の自動バックアップを作る");
    let generations = b.config().backup_generations;
    for _ in 0..generations {
        b.backup_now().unwrap();
    }
    let list = b.list_backups().unwrap();
    assert_eq!(list.len(), generations);
    assert!(
        list.iter()
            .all(|x| a_backups.iter().all(|y| y.path != x.path)),
        "B の一覧に A のバックアップを出さない"
    );
    assert!(!b_first.path.exists(), "B の世代の管理は B のものだけ");
    b.close().unwrap();
    for x in &a_backups {
        assert!(
            x.path.is_file(),
            "B の世代の管理で A のバックアップを消さない"
        );
    }
    let a = open(&env_a);
    let list = a.list_backups().unwrap();
    assert_eq!(
        list.iter().map(|x| x.path.clone()).collect::<Vec<_>>(),
        a_backups
            .iter()
            .rev()
            .map(|x| x.path.clone())
            .collect::<Vec<_>>()
    );
    a.close().unwrap();
}

/// フォルダ `dir` に、名前 `names` のファイルが別々のファイルとして並ぶか（大文字・小文字や Unicode の
/// 正規化を区別するファイルシステムか）。
fn distinct_names_coexist(dir: &Path, names: &[&str]) -> bool {
    let found: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.iter().all(|n| found.iter().any(|f| f == n))
}

/// 大文字・小文字や Unicode の正規化だけが違う名前のファイル・フォルダが同じフォルダに並ぶ場合（区別する
/// ボリューム。Linux・大文字・小文字を区別する APFS・Windows のフォルダごとの設定、NTFS の NFC と NFD）、
/// カタログでは区別できないので、どちらも登録せずに報告する（1 つのレコードにまとめて取り込みのたびに
/// 入れ替えたり、実在しないパスを記録したりしない。指摘 F08）。区別しないボリューム（Windows・macOS の
/// 既定）では、そもそも並ばないので確かめる部分を飛ばす。
#[test]
fn names_that_differ_only_in_case_or_normalization_are_not_merged() {
    let env = Env::new();
    let photos = &env.photos;
    write_jpeg(&photos.join("other.jpg"), (32, 24), 9, None);
    write_jpeg(&photos.join("IMG_0001.JPG"), (64, 48), 1, None);
    write_jpeg(&photos.join("img_0001.jpg"), (80, 60), 2, None);
    let nfc = "\u{304c}.jpg";
    let nfd = "\u{304b}\u{3099}.jpg";
    write_jpeg(&photos.join(nfc), (32, 24), 3, None);
    write_jpeg(&photos.join(nfd), (40, 30), 4, None);
    std::fs::create_dir_all(photos.join("Trip")).unwrap();
    std::fs::create_dir_all(photos.join("trip")).unwrap();
    write_jpeg(&photos.join("Trip").join("a.jpg"), (32, 24), 5, None);
    write_jpeg(&photos.join("trip").join("b.jpg"), (32, 24), 6, None);
    let case_sensitive = distinct_names_coexist(photos, &["IMG_0001.JPG", "img_0001.jpg"]);
    let normalization_sensitive = distinct_names_coexist(photos, &[nfc, nfd]);
    let dirs_case_sensitive = distinct_names_coexist(photos, &["Trip", "trip"]);

    let core = env.open();
    let report = import(&core, photos);
    let names = common::names(&all_items(&core));
    assert!(names.contains(&"other.jpg".to_owned()), "{names:?}");
    let not_registered: Vec<&Path> = report
        .not_registered
        .iter()
        .map(|i| i.path.as_path())
        .collect();
    let refused = |p: &Path| {
        report
            .not_registered
            .iter()
            .any(|i| i.path == p && i.reason.contains("大文字・小文字"))
    };
    if case_sensitive {
        assert!(refused(&photos.join("IMG_0001.JPG")), "{report:?}");
        assert!(refused(&photos.join("img_0001.jpg")), "{report:?}");
        assert!(
            !names.iter().any(|n| n.eq_ignore_ascii_case("img_0001.jpg")),
            "{names:?}"
        );
    } else {
        eprintln!("大文字・小文字を区別しないボリュームのため、ファイル名の部分を飛ばす");
    }
    if normalization_sensitive {
        assert!(refused(&photos.join(nfc)), "{report:?}");
        assert!(refused(&photos.join(nfd)), "{report:?}");
        assert!(
            !names
                .iter()
                .any(|n| n.ends_with("\u{3099}.jpg") || n == nfc)
        );
    }
    if dirs_case_sensitive {
        assert!(refused(&photos.join("Trip")), "{not_registered:?}");
        assert!(refused(&photos.join("trip")), "{not_registered:?}");
        assert!(!names.contains(&"a.jpg".to_owned()) && !names.contains(&"b.jpg".to_owned()));
    }
    // 取り込み直しても、入れ替わらない。
    let again = import(&core, photos);
    assert_eq!(again.updated, 0, "{again:?}");
    core.close().unwrap();

    // 別々に取り込んだ場合（先に Trip だけを取り込んだカタログに、trip を取り込む）も、Trip のレコードに
    // まとめない（記録したパスが実在しない・別のファイルを指すことを防ぐ）。
    if dirs_case_sensitive {
        let env2 = Env::new();
        let p2 = &env2.photos;
        std::fs::create_dir_all(p2.join("Trip")).unwrap();
        write_jpeg(&p2.join("Trip").join("a.jpg"), (32, 24), 5, None);
        let core = env2.open();
        assert_eq!(import(&core, &p2.join("Trip")).added, 1);
        std::fs::create_dir_all(p2.join("trip")).unwrap();
        write_jpeg(&p2.join("trip").join("b.jpg"), (32, 24), 6, None);
        let r = import(&core, &p2.join("trip"));
        assert_eq!(r.added, 0, "{r:?}");
        assert!(
            r.not_registered
                .iter()
                .any(|i| i.path == p2.join("trip").join("b.jpg")
                    && i.reason.contains("大文字・小文字")),
            "{r:?}"
        );
        // Trip の中の a.jpg の場所は変わらない。
        let items = all_items(&core);
        assert_eq!(common::names(&items), vec!["a.jpg".to_owned()]);
        let details = core.variant_details(items[0].variant_id).unwrap();
        assert_eq!(
            details.files[0].path.as_deref(),
            Some(p2.join("Trip").join("a.jpg").as_path())
        );
        core.close().unwrap();
    }
}

/// 同じ項目を同時に変える 2 つの設定の更新が、保存（カタログ）と反映（メモリ上の設定・キャッシュの上限）
/// の順序を入れ替えて終わらない（Codex のレビュー。`Core` は `Sync` で、UI の複数の経路から同時に呼べる）。
///
/// 以前は、保存（カタログのロック）と反映（設定のロック）を別々のロックで行ったので、A が保存して反映の
/// 前で止まっている間に、B が保存・反映まで終えると、後から A が古い値を反映した（カタログは B の値、
/// 実行中は A の値になり、次に開くまで食い違う）。
///
/// 順序は、時間ではなくフック（`CoreHooks::on_settings_update`）とチャネルで決める。A を保存の直後で止めて、
/// B を始める。B が A を追い越して保存まで進んだ（`Saved`）か、A の終わりを待つ（`Waiting`）かの
/// 知らせのどちらかが届くまで待つので、どちらの場合も順序が決まる（待つ上限は失敗のときだけ使う）。
#[test]
fn concurrent_settings_updates_do_not_leave_memory_different_from_the_catalog() {
    use crossbeam_channel::unbounded;
    use genzo_api::SettingsUpdateStage as Stage;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    enum Seen {
        ASaved,
        BSaved,
        BWaiting,
    }

    let env = Env::new();
    let (seen_tx, seen_rx) = unbounded::<Seen>();
    let (release_a_tx, release_a_rx) = unbounded::<()>();
    let (release_b_tx, release_b_rx) = unbounded::<()>();
    let saved_count = Arc::new(AtomicUsize::new(0));
    let core = env.open_with_hooks(|hooks| {
        hooks.on_settings_update = Some(Arc::new(move |stage| match stage {
            // 保存の直後で止める（1 回目が A、2 回目が B。A が止まってから B を始めるので、順序は決まっている）。
            Stage::Saved => {
                if saved_count.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = seen_tx.send(Seen::ASaved);
                    let _ = release_a_rx.recv();
                } else {
                    let _ = seen_tx.send(Seen::BSaved);
                    let _ = release_b_rx.recv();
                }
            }
            Stage::Waiting => {
                let _ = seen_tx.send(Seen::BWaiting);
            }
        }));
    });
    let update_a = SettingsUpdate {
        preview_cache_bytes: Some(150_000_000),
        default_utc_offset_minutes: Some(0),
        ..Default::default()
    };
    let update_b = SettingsUpdate {
        preview_cache_bytes: Some(250_000_000),
        default_utc_offset_minutes: Some(60),
        ..Default::default()
    };
    let (seen_b, in_memory) = std::thread::scope(|scope| {
        let core = &core;
        let a = scope.spawn(|| core.update_settings(&update_a));
        // A が、カタログに保存し終えて、反映の前で止まるまで待つ。
        let first = seen_rx.recv_timeout(common::WAIT_LIMIT);
        assert!(matches!(first, Ok(Seen::ASaved)), "{first:?}");
        let b = scope.spawn(|| core.update_settings(&update_b));
        // B が A を追い越して保存まで進んだか、A の終わりを待つか。
        let seen_b = seen_rx.recv_timeout(common::WAIT_LIMIT);
        // 結果によらず、止めたスレッドを再開する（止めたままだと、スレッドが終わらない）。追い越した場合は、
        // 先に B を終えてから A を再開する（以前の、食い違う順序）。待つ場合は、A を終えてから B が進む。
        if matches!(seen_b, Ok(Seen::BSaved)) {
            let _ = release_b_tx.send(());
            b.join().unwrap().unwrap();
            let _ = release_a_tx.send(());
            a.join().unwrap().unwrap();
        } else {
            let _ = release_a_tx.send(());
            a.join().unwrap().unwrap();
            let _ = release_b_tx.send(());
            b.join().unwrap().unwrap();
        }
        (seen_b, core.settings())
    });
    assert!(
        matches!(seen_b, Ok(Seen::BSaved | Seen::BWaiting)),
        "{seen_b:?}"
    );
    // 実行中の設定は、カタログに保存された設定（次に開いたときに復元される値）と同じ。
    core.close().unwrap();
    let core = env.open();
    let restored = core.settings();
    assert_eq!(
        in_memory, restored,
        "実行中の設定が、カタログに保存された設定と食い違う（追い越し: {seen_b:?}）"
    );
    // 保存は A、B の順に終わっているので、残るのは B の値。
    assert_eq!(restored.preview_cache_bytes, 250_000_000);
    assert_eq!(restored.default_utc_offset_minutes, 60);
    core.close().unwrap();
}

/// 1 つの設定の更新が複数の項目を変えるとき、途中の保存が失敗しても、カタログに要求の一部だけを残さない
/// （Codex のレビュー。ディスクが一杯・入出力のエラーなどで、後の項目の書き込みが失敗する場合）。
///
/// 以前は、項目ごとの書き込みと、撮影日時の推定し直しが済んでいない印の書き込みが別々にコミットされたので、
/// 後の項目が失敗すると、先の項目と印がカタログに残った。実行中の設定は古い値のままなのに、次に開くと
/// 一部だけ適用された要求を読み込んだ（キャッシュの上限などだけが変わり、印のせいで撮影日時を推定し直す）。
///
/// 失敗は、テスト用の関数（`genzo_catalog::testing::fail_setting_writes`）で、既定のオフセットの書き込みだけを
/// SQLite のエラーにして起こす（時間には依存しない）。更新は、ほかの項目を書いた後で、最後にオフセットを書く。
#[test]
fn a_failed_settings_update_does_not_leave_part_of_it_in_the_catalog() {
    use genzo_api::{
        KEY_DEFAULT_EXPORT_COLOR_SPACE, KEY_DEFAULT_UTC_OFFSET_MINUTES, KEY_PREVIEW_CACHE_BYTES,
        KEY_PREVIEW_CACHE_DIR,
    };
    use genzo_model::OutputColorSpace;

    let env = Env::new();
    let core = env.open();
    // 写真がないので、開いたときに印（推定し直しが済んでいない印）は外れている。
    let before = core.settings();
    let keys = [
        KEY_PREVIEW_CACHE_DIR,
        KEY_PREVIEW_CACHE_BYTES,
        KEY_DEFAULT_EXPORT_COLOR_SPACE,
        KEY_DEFAULT_UTC_OFFSET_MINUTES,
    ];
    // カタログに保存されている項目と印（別の接続で読む）。
    let stored = || {
        let cat = genzo_catalog::Catalog::open_read_only(env.catalog_path()).unwrap();
        let values: Vec<Option<String>> = keys.iter().map(|k| cat.setting(k).unwrap()).collect();
        (values, cat.capture_time_reresolve_pending().unwrap())
    };
    let stored_before = stored();
    assert!(!stored_before.1, "{stored_before:?}");
    // 4 項目のどれも、今の値と違う値にする（既定のオフセットを変えるので、印も付く）。
    let update = SettingsUpdate {
        preview_cache_dir: Some(env.dir.path().join("another-preview-cache")),
        preview_cache_bytes: Some(before.preview_cache_bytes + 50_000_000),
        default_export_color_space: Some(OutputColorSpace::AdobeRgb),
        default_utc_offset_minutes: Some(before.default_utc_offset_minutes + 60),
    };
    assert_ne!(
        update.default_export_color_space,
        Some(before.default_export_color_space)
    );

    // 既定のオフセットの書き込みだけを失敗させる。
    genzo_catalog::testing::fail_setting_writes(env.catalog_path(), KEY_DEFAULT_UTC_OFFSET_MINUTES)
        .unwrap();
    let err = core.update_settings(&update).unwrap_err();
    // 実行中の設定も、カタログ（項目と印）も、更新の前のまま（一部だけ保存されていない）。
    assert_eq!(core.settings(), before, "{err}");
    assert_eq!(stored(), stored_before, "一部だけ保存された: {err}");
    // 次に開いても、更新の前の設定を読み込む。
    core.close().unwrap();
    let core = env.open();
    assert_eq!(core.settings(), before);

    // 失敗の原因がなくなれば、同じ更新が通り、全項目が保存される。
    genzo_catalog::testing::allow_setting_writes(env.catalog_path()).unwrap();
    let current = core.update_settings(&update).unwrap();
    assert_eq!(
        current.preview_cache_bytes,
        update.preview_cache_bytes.unwrap()
    );
    core.wait_idle();
    core.close().unwrap();
    let core = env.open();
    assert_eq!(core.settings(), current);
    assert_eq!(
        current.default_utc_offset_minutes,
        before.default_utc_offset_minutes + 60
    );
    assert_eq!(
        current.default_export_color_space,
        OutputColorSpace::AdobeRgb
    );
    assert_eq!(
        current.preview_cache_dir,
        env.dir.path().join("another-preview-cache")
    );
    core.close().unwrap();
}

/// 1 回の更新でキャッシュの上限と既定のタイムゾーンの両方を変えるとき、新しい上限の反映（超えた分の回収。
/// SQLite のエラーなどで失敗しうる）が失敗しても、撮影日時の推定し直しのジョブを投入する（Codex のレビュー。
/// コア API は、エラーを返すが、更新は保存とメモリ上の設定への反映まで済んでいる）。
///
/// 以前は、上限の反映の失敗が、ジョブの投入より前に `?` で戻ったので、実行中の設定とカタログは新しい
/// タイムゾーンなのに、撮影日時は古いタイムゾーンで推定したままで、次に開く（印を見てジョブを動かす）まで
/// 直らなかった。
///
/// 失敗は、テスト用の関数（`genzo_catalog::testing::fail_preview_evictions`）で、プレビューの索引の行の削除
/// だけを SQLite のエラーにして起こす（カタログへの保存は成功する。時間には依存しない）。
#[test]
fn a_failed_cache_capacity_update_still_reresolves_capture_times_for_the_new_offset() {
    use genzo_api::ApiError;

    const N: usize = 5;
    let env = Env::new();
    let open = || env.open();
    // 既定のタイムゾーン（+09:00）を保存してから、オフセットのない写真を登録する。
    open().close().unwrap();
    register_user_default_assets(&env.catalog_path(), N);
    let core = open();
    let offsets = |core: &genzo_api::Core| -> Vec<Option<String>> {
        all_items(core)
            .iter()
            .map(|s| s.capture.tz_assumed.clone())
            .collect()
    };
    let jst = Some("+09:00".to_owned());
    assert_eq!(offsets(&core), vec![jst; N], "更新の前は +09:00");
    let before = core.settings();
    let update = SettingsUpdate {
        preview_cache_bytes: Some(before.preview_cache_bytes + 50_000_000),
        default_utc_offset_minutes: Some(60),
        ..Default::default()
    };

    // 上限の反映（超えた分の回収）だけを失敗させる。
    let thumbs_db = env.config().thumbs_path();
    genzo_catalog::testing::fail_preview_evictions(&thumbs_db).unwrap();
    let err = core.update_settings(&update).unwrap_err();
    assert!(
        matches!(&err, ApiError::Cache { .. })
            && format!("{err:?}").contains("preview index delete"),
        "上限の反映の失敗のはず: {err:?}"
    );
    // 保存とメモリ上の設定は、更新のとおり（失敗したのは、上限の反映のうちの回収だけ）。
    let current = core.settings();
    assert_eq!(current.default_utc_offset_minutes, 60);
    assert_eq!(
        current.preview_cache_bytes,
        update.preview_cache_bytes.unwrap()
    );
    // 推定し直しのジョブが投入されていて、撮影日時が新しいオフセットで求め直される。
    core.wait_idle();
    assert_eq!(
        offsets(&core),
        vec![Some("+01:00".to_owned()); N],
        "プレビューの上限の反映の失敗で、撮影日時の推定し直しが飛ばされた（実行中の設定は +01:00）"
    );
    // 終えたジョブが、推定し直しが済んでいない印を外している（次に開いても、もう一度動かさない）。
    let pending = genzo_catalog::Catalog::open_read_only(env.catalog_path())
        .unwrap()
        .capture_time_reresolve_pending()
        .unwrap();
    assert!(!pending, "ジョブが終わったので、印は外れているはず");

    // 失敗の原因がなくなれば、同じ上限の更新が通る。開き直しても、新しい設定と撮影日時のまま。
    genzo_catalog::testing::allow_preview_evictions(&thumbs_db).unwrap();
    core.update_settings(&SettingsUpdate {
        preview_cache_bytes: update.preview_cache_bytes,
        ..Default::default()
    })
    .unwrap();
    core.close().unwrap();
    let core = open();
    assert_eq!(core.settings(), current);
    assert_eq!(offsets(&core), vec![Some("+01:00".to_owned()); N]);
    core.close().unwrap();
}

/// 表示範囲の描画（ルーペ・パン）が重なっても、先に番号を取った古い要求が、後から投入して、先に投入した
/// 新しい要求を取り消さない（Codex のレビュー。`Core` は `Sync` で、Tauri からは要求ごとに `spawn_blocking`
/// で呼ぶので、パンの連続で 2 つの `render_region` は重なる）。
///
/// 以前は、要求の番号の割り当て（`region` のロック）とスケジューラへの投入が別々で、間を通しで守るものが
/// なかった。A が番号 1 を割り当てて止まっている間に、B が番号 2 を割り当てて投入すると、後から A が
/// 投入して B（実行中・待ち）を取り消した。残るのは古い範囲（番号 1）の描画だけで、最新の範囲は描かれない。
///
/// 順序は、時間ではなくフック（`CoreHooks::on_region_request`）とチャネルで決める。A を番号の割り当ての
/// 直後で止めて、B を始める。B が A を追い越して番号を取った（`Numbered(2)`）か、A の区間の終わりを待つ
/// （`Waiting`）かの知らせのどちらかが届くまで待つので、どちらの場合も順序が決まる。追い越した場合は、
/// B のジョブを実行中のまま止めてから A を再開する（B が先に終わると、A が後から投入しても取り消すものが
/// ない）。待つ上限は失敗のときだけ使う。
#[test]
fn an_older_region_request_does_not_cancel_a_newer_one_submitted_before_it() {
    use crossbeam_channel::unbounded;
    use genzo_api::{RegionRequest, RegionRequestStage as Stage, RegionSpace};
    use std::sync::Arc;

    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    let (seen_tx, seen_rx) = unbounded::<Stage>();
    let (release_a_tx, release_a_rx) = unbounded::<()>();
    let (release_job_tx, release_job_rx) = unbounded::<()>();
    let core = env.open_with_hooks(|hooks| {
        // 止めたスレッドは、再開の合図か、送り側が閉じる（テストが失敗して抜ける）と動く。
        hooks.on_region_request = Some(Arc::new(move |stage| {
            let _ = seen_tx.send(stage);
            match stage {
                // 1 番目の要求（A）を、番号を割り当てた直後（投入の前）で止める。
                Stage::Numbered(1) => {
                    let _ = release_a_rx.recv();
                }
                // 2 番目の要求（B）のジョブを、実行中のまま止める。
                Stage::Running(2) => {
                    let _ = release_job_rx.recv();
                }
                _ => {}
            }
        }));
    });
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    let request = |x: f64, y: f64, w: f64, h: f64| RegionRequest {
        variant_id: v,
        space: RegionSpace::Output,
        x,
        y,
        width: w,
        height: h,
        scale: 1.0,
    };
    let (req_a, req_b) = (
        request(0.0, 0.0, 32.0, 24.0),
        request(16.0, 8.0, 16.0, 16.0),
    );

    let core_ref = &core;
    let (a_result, b_result, b_first) = std::thread::scope(move |scope| {
        let wait_for = |want: Stage| {
            loop {
                match seen_rx.recv_timeout(common::WAIT_LIMIT) {
                    Ok(s) if s == want => return,
                    Ok(_) => {}
                    Err(e) => panic!("{want:?} の知らせが届かない: {e}"),
                }
            }
        };
        let a = scope.spawn(move || core_ref.render_region(&req_a));
        // A が番号 1 を割り当てて、止まるまで待つ。
        wait_for(Stage::Numbered(1));
        let b = scope.spawn(move || core_ref.render_region(&req_b));
        // B が A を追い越して番号 2 を取ったか、A の区間の終わりを待つか。
        let b_first = loop {
            match seen_rx.recv_timeout(common::WAIT_LIMIT) {
                Ok(s @ (Stage::Numbered(2) | Stage::Waiting)) => break s,
                Ok(_) => {}
                Err(e) => panic!("B の最初の知らせが届かない: {e}"),
            }
        };
        if b_first == Stage::Waiting {
            // 直した後: A が区間を出てから、B が番号を取って投入する。
            let _ = release_a_tx.send(());
            wait_for(Stage::Running(2));
        } else {
            // 以前: 追い越した B のジョブが実行中で止まってから、A を再開する（A の投入が B を取り消す）。
            wait_for(Stage::Running(2));
            let _ = release_a_tx.send(());
            wait_for(Stage::Submitted(1));
        }
        let _ = release_job_tx.send(());
        (a.join().unwrap(), b.join().unwrap(), b_first)
    });

    let b_frame = b_result.unwrap_or_else(|e| {
        panic!("最新の要求（番号 2）が、古い要求（番号 1）に取り消された（B の最初: {b_first:?}）: {e:?}")
    });
    assert_eq!(b_frame.info.request, 2);
    assert_eq!(
        (b_frame.info.x, b_frame.info.y, b_frame.info.width),
        (16, 8, 16)
    );
    // A は、描けたか、B に取り消された（どちらも誤りではない）。
    match a_result {
        Ok(f) => assert_eq!(f.info.request, 1),
        Err(e) => assert!(e.is_cancelled(), "{e:?}"),
    }
    // 最後に描けた結果は最新の要求のもの。
    assert_eq!(core.region_frame().unwrap().info.request, 2);
    core.close().unwrap();
}

/// 現像設定の保存（現像のロックを外して書く保存）を、ロックを外した直後で止めておく道具（テスト用）。
struct SaveStall {
    /// 真の間に、次の保存が止まる（止まったら偽に戻す。1 回だけ止める）。
    armed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// 保存が止まったことの知らせ。
    stalled: crossbeam_channel::Receiver<()>,
    /// 止めた保存を再開する合図（送り側が閉じても再開する。テストが失敗して抜けても、スレッドが残らない）。
    release: crossbeam_channel::Sender<()>,
}

fn open_with_save_stall(env: &Env) -> (genzo_api::Core, SaveStall) {
    use crossbeam_channel::unbounded;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let armed = Arc::new(AtomicBool::new(false));
    let (stalled_tx, stalled) = unbounded::<()>();
    let (release, release_rx) = unbounded::<()>();
    let hook_armed = Arc::clone(&armed);
    let core = env.open_with_config_and_hooks(
        // 自動保存のスレッドが割り込まないよう、待ち時間を十分に長くする（保存は、テストが呼ぶ操作だけ）。
        |c| c.develop_save_delay = Some(std::time::Duration::from_secs(3600)),
        |hooks| {
            hooks.on_develop_save =
                Some(Arc::new(move |genzo_api::DevelopSaveStage::Unlocked(_)| {
                    if hook_armed.swap(false, Ordering::SeqCst) {
                        let _ = stalled_tx.send(());
                        let _ = release_rx.recv();
                    }
                }));
        },
    );
    (
        core,
        SaveStall {
            armed,
            stalled,
            release,
        },
    )
}

fn develop_exposure(ev: f32) -> DevelopSettings {
    DevelopSettings {
        exposure_ev: ev,
        ..Default::default()
    }
}

fn history_labels(core: &genzo_api::Core, v: genzo_api::VariantId) -> Vec<String> {
    core.history(v)
        .unwrap()
        .into_iter()
        .map(|h| h.label)
        .collect()
}

/// 現像のロックを外して書く保存（`flush_develop`・自動保存・ドラッグの終了）が、ロックを取る前に止まって
/// いる間に、同じ写真の新しい設定と、別の写真の設定が保存されても、古い写しで新しい設定を上書きしない
/// （Codex のレビュー。`Core` は `Sync` で、自動保存のスレッドと UI の呼び出しは同時に動く）。
///
/// 以前は、保存の追い越しの記録が「最後に書いた 1 件（variant と番号）」だけだった。別の写真を書くと、
/// A の新しい書き込みの記録が消え、止まっていた古い写しが A の新しい設定と履歴を上書きした。
///
/// 順序は、時間ではなくフック（`CoreHooks::on_develop_save`）とチャネルで決める。A の保存を、写しを取って
/// ロックを外した直後で止め、止めている間に A への貼り付けと、B の編集・保存を終えてから再開する。
#[test]
fn a_stale_save_does_not_overwrite_newer_settings_after_another_photo_was_saved() {
    use genzo_api::SettingGroups;

    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (48, 32), 1, None);
    write_jpeg(&env.photos.join("B.jpg"), (48, 32), 2, None);
    let (core, stall) = open_with_save_stall(&env);
    import(&core, &env.photos);
    let items = all_items(&core);
    let (a, b) = (
        by_name(&items, "A.jpg").variant_id,
        by_name(&items, "B.jpg").variant_id,
    );
    core.open_develop(a).unwrap();
    // 保存待ちの変更（これが、止める保存の古い写しになる）。
    core.set_settings(&develop_exposure(0.5), false).unwrap();

    let core_ref = &core;
    let stale_saved = std::thread::scope(move |scope| {
        let core = core_ref;
        let SaveStall {
            armed,
            stalled,
            release,
        } = stall;
        armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let stale = scope.spawn(move || core.flush_develop());
        stalled
            .recv_timeout(common::WAIT_LIMIT)
            .expect("保存が、ロックを外した直後で止まらない");

        // 止めている間: A の新しい設定を貼り付ける（一括適用。保存待ちの古い写しも、先に保存される）。
        core.paste_settings(&develop_exposure(2.0), &[a], SettingGroups::ALL)
            .unwrap();
        // 続けて B を開いて編集し、保存する（保存の記録が B のものになる）。
        core.open_develop(b).unwrap();
        core.set_settings(&develop_exposure(1.0), false).unwrap();
        assert!(core.flush_develop().unwrap());
        assert_eq!(core.develop_settings(b).unwrap(), develop_exposure(1.0));

        // 止めていた古い写しの保存を再開する。
        release.send(()).unwrap();
        let stale_saved = stale.join().unwrap().unwrap();
        // B は開いたままなので、A はカタログの値を読む。
        assert_eq!(
            core.develop_settings(a).unwrap(),
            develop_exposure(2.0),
            "古い写し（露光量 +0.50）が、A の新しい設定を上書きした"
        );
        assert_eq!(
            history_labels(core, a),
            vec!["読み込み", "露光量 +0.50", "設定の貼り付け"],
            "古い写しが履歴に追加された"
        );
        assert_eq!(core.develop_settings(b).unwrap(), develop_exposure(1.0));
        stale_saved
    });
    assert!(!stale_saved, "古い写しは書かない");
}

/// 上の回帰テストの別の経路: 止めている間の A の新しい書き込みが Undo で、追い越す別の写真の書き込みが、
/// 現像していない B の Undo（現像のセッションを経由せずに、保存の記録を書き換える）。
#[test]
fn a_stale_save_does_not_overwrite_an_undo_after_another_photos_undo() {
    use genzo_api::SettingGroups;

    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (48, 32), 1, None);
    write_jpeg(&env.photos.join("B.jpg"), (48, 32), 2, None);
    let (core, stall) = open_with_save_stall(&env);
    import(&core, &env.photos);
    let items = all_items(&core);
    let (a, b) = (
        by_name(&items, "A.jpg").variant_id,
        by_name(&items, "B.jpg").variant_id,
    );
    // B に、Undo できる履歴を作っておく（B は開かない）。
    core.paste_settings(&develop_exposure(0.3), &[b], SettingGroups::ALL)
        .unwrap();
    core.open_develop(a).unwrap();
    core.set_settings(&develop_exposure(0.5), false).unwrap();

    let core_ref = &core;
    let stale_saved = std::thread::scope(move |scope| {
        let core = core_ref;
        let SaveStall {
            armed,
            stalled,
            release,
        } = stall;
        armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let stale = scope.spawn(move || core.flush_develop());
        stalled
            .recv_timeout(common::WAIT_LIMIT)
            .expect("保存が、ロックを外した直後で止まらない");

        // 止めている間: A を Undo する（保存待ちの古い写しを先に保存してから、1 つ前に戻す）。
        assert_eq!(core.undo(a).unwrap(), Some(DevelopSettings::default()));
        // 続けて B を Undo する（保存の記録が B のものになる）。
        assert_eq!(core.undo(b).unwrap(), Some(DevelopSettings::default()));

        release.send(()).unwrap();
        let stale_saved = stale.join().unwrap().unwrap();
        // カタログの値を読むため、A を閉じる。
        core.close_develop().unwrap();
        assert_eq!(
            core.develop_settings(a).unwrap(),
            DevelopSettings::default(),
            "古い写し（露光量 +0.50）が、Undo の結果を上書きした"
        );
        let history = core.history(a).unwrap();
        assert_eq!(
            history.iter().map(|h| h.label.as_str()).collect::<Vec<_>>(),
            vec!["読み込み", "露光量 +0.50"],
            "古い写しが履歴に追加された"
        );
        assert!(history[0].is_current, "Undo した位置のまま");
        stale_saved
    });
    assert!(!stale_saved, "古い写しは書かない");
}

/// 上の回帰テストの別の経路: 止めている間に、A の現像のセッションが A を書かずに閉じ（設定を保存済みの
/// 値に戻していたので、閉じるときに書くものがない）、現像していない A への貼り付けが、止めた保存を追い
/// 越す。貼り付けは、現像中の写真に限らず、書いた variant をすべて記録しなければならない（検証担当が追加。
/// 現像中の写真だけを記録する実装に戻すと失敗する）。
#[test]
fn a_stale_save_does_not_overwrite_a_paste_into_a_photo_that_is_no_longer_open() {
    use genzo_api::SettingGroups;

    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (48, 32), 1, None);
    let (core, stall) = open_with_save_stall(&env);
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    core.open_develop(a).unwrap();
    // 保存待ちの変更（これが、止める保存の古い写しになる）。
    core.set_settings(&develop_exposure(0.5), false).unwrap();

    let core_ref = &core;
    let stale_saved = std::thread::scope(move |scope| {
        let core = core_ref;
        let SaveStall {
            armed,
            stalled,
            release,
        } = stall;
        armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let stale = scope.spawn(move || core.flush_develop());
        stalled
            .recv_timeout(common::WAIT_LIMIT)
            .expect("保存が、ロックを外した直後で止まらない");

        // 止めている間: 設定を保存済みの値に戻し（保存待ちがなくなる）、A を閉じる（何も書かない）。
        core.set_settings(&DevelopSettings::default(), false)
            .unwrap();
        core.close_develop().unwrap();
        assert_eq!(history_labels(core, a), vec!["読み込み"]);
        // 現像していない A に貼り付ける。
        core.paste_settings(&develop_exposure(2.0), &[a], SettingGroups::ALL)
            .unwrap();

        release.send(()).unwrap();
        let stale_saved = stale.join().unwrap().unwrap();
        assert_eq!(
            core.develop_settings(a).unwrap(),
            develop_exposure(2.0),
            "古い写し（露光量 +0.50）が、貼り付けた設定を上書きした"
        );
        assert_eq!(
            history_labels(core, a),
            vec!["読み込み", "設定の貼り付け"],
            "古い写しが履歴に追加された"
        );
        stale_saved
    });
    assert!(!stale_saved, "古い写しは書かない");
}

/// ファイルの状態を調べる `metadata` が「ファイルがない」以外の理由で失敗したとき、ファイルを見つからない
/// （`missing`）と報告しない。状態は変えず、見つからないという誤解を招く警告も出さない（指摘 3）。
///
/// 権限を使わずに再現する: 写真のフォルダを通常のファイルに置き換える（フォルダの部分がフォルダでない
/// ので、`metadata` は NotFound ではなく NotADirectory で失敗する。root で実行しても同じ）。Windows では
/// この状況が NotFound になるため、Unix だけで確かめる。
#[cfg(unix)]
#[test]
fn unreadable_metadata_is_not_reported_as_a_missing_file() {
    use genzo_api::ApiError;

    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (32, 24), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    let rx = core.subscribe();

    // フォルダを退避し、同じ名前の通常のファイルを置く。
    let aside = env.dir.path().join("photos.aside");
    std::fs::rename(&env.photos, &aside).unwrap();
    std::fs::write(&env.photos, b"not a folder").unwrap();
    let file_missing_warnings = |rx: &crossbeam_channel::Receiver<Event>| {
        common::drain(rx)
            .into_iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::Warning {
                        code: WarningCode::FileMissing,
                        ..
                    }
                )
            })
            .count()
    };
    let status = |core: &genzo_api::Core| {
        let item = all_items(core).remove(0);
        (item.file_status, item.file_revision)
    };

    // ファイルの確認: 見つからないものに数えず、状態を変えない（読めないファイルは、今の状態のまま）。
    let report = core.check_files(&[a]).unwrap();
    assert!(report.missing.is_empty(), "{report:?}");
    assert_eq!(report.checked, 1, "{report:?}");
    assert_eq!(status(&core), (FileStatus::Ok, 1));

    // 現像を開く（元ファイルの照合を経由する経路）: FileAccess で失敗する。見つからないという警告は
    // 出さず、状態も変えない。
    let err = core.open_develop(a).unwrap_err();
    assert!(matches!(err, ApiError::FileAccess { .. }), "{err:?}");
    assert!(
        !err.to_string().contains("見つかりません"),
        "見つからないという誤った理由: {err}"
    );
    assert_eq!(status(&core), (FileStatus::Ok, 1));
    assert_eq!(file_missing_warnings(&rx), 0, "見つからないという警告");

    // フォルダが戻れば、何も変わっていない（missing にしていないので、戻す処理も要らない）。
    std::fs::remove_file(&env.photos).unwrap();
    std::fs::rename(&aside, &env.photos).unwrap();
    let report = core.check_files(&[a]).unwrap();
    assert_eq!((report.ok, report.missing.len()), (1, 0), "{report:?}");
    assert!(report.changed.is_empty(), "{report:?}");
    assert_eq!(status(&core), (FileStatus::Ok, 1));
    core.open_develop(a).unwrap();
    core.close().unwrap();
}

/// フォルダごと見えない（ドライブを外した状態の代わり）ファイルは、これまでどおり見つからない（`missing`）
/// として記録し、戻れば `ok` に戻す。`missing` は戻せる印なので記録してよい。一方、ゴミ箱への移動は、
/// フォルダごと見えなければ移さず失敗にしてカタログに残す（`trash_does_not_drop_photos_on_an_unreachable_volume`）。
/// 両者は矛盾しない（指摘 3。NotFound 以外のエラーだけを「見つからない」にしない変更の境界）。
#[test]
fn a_vanished_folder_is_marked_missing_and_restored_but_not_trashed() {
    let env = Env::new();
    write_jpeg(&env.photos.join("vol").join("A.jpg"), (32, 24), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    let offline = env.photos.join("vol-offline");
    std::fs::rename(env.photos.join("vol"), &offline).unwrap();

    let report = core.check_files(&[a]).unwrap();
    assert_eq!(report.missing.len(), 1, "{report:?}");
    assert_eq!(all_items(&core)[0].file_status, FileStatus::Missing);
    // 見つからない印が付いていても、ゴミ箱への移動は失敗にして写真を残す。
    let plan = core.plan_delete(DeleteKind::Trash, &[a]).unwrap();
    let delete = core.execute_delete(plan.plan_id).unwrap();
    assert_eq!(delete.failed.len(), 1, "{delete:?}");
    assert!(delete.removed_assets.is_empty(), "{delete:?}");
    assert_eq!(all_items(&core).len(), 1, "写真はカタログに残る");

    // フォルダが戻れば ok に戻る。
    std::fs::rename(&offline, env.photos.join("vol")).unwrap();
    let report = core.check_files(&[a]).unwrap();
    assert_eq!((report.ok, report.missing.len()), (1, 0), "{report:?}");
    assert_eq!(all_items(&core)[0].file_status, FileStatus::Ok);
    core.close().unwrap();
}

/// 現像設定の保存だけを失敗させた Core を開く（Codex のレビュー。ディスクが一杯・入出力のエラーなどで、
/// 終了のときの保存が失敗する場合の再現）。写真 `A.jpg` を取り込んで開き、露光量 +0.5 を保存待ちにして、
/// 保存の書き込みを失敗させた状態で返す（自動保存は動かない。保存は、テストが呼ぶ操作だけ）。
fn open_with_unsaved_develop_and_failing_saves(
    env: &Env,
) -> (genzo_api::Core, genzo_api::VariantId) {
    write_jpeg(&env.photos.join("A.jpg"), (48, 32), 1, None);
    // 自動保存のスレッドが割り込まないよう、待ち時間を十分に長くする。
    let core = env.open_with(|c| c.develop_save_delay = Some(std::time::Duration::from_secs(3600)));
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    core.open_develop(a).unwrap();
    core.set_settings(&develop_exposure(0.5), false).unwrap();
    // 取り込み（最初の履歴を作る）の後で、現像設定の書き込みだけを失敗させる。
    genzo_catalog::testing::fail_develop_writes(env.catalog_path()).unwrap();
    (core, a)
}

/// 終了のときに現像設定を保存できなくても、未保存の設定の唯一のメモリ上の写しを捨てず、カタログに「正常に
/// 終了した」印も付けない。原因がなくなってからもう一度閉じれば、保存して閉じる（Codex のレビュー）。
///
/// 以前は、終了の処理が保存に失敗しても、現像のセッション（未保存の設定の唯一の写し）を無条件に捨て、
/// 「終了済み」の印を先に付けてカタログを閉じた。呼び出し側はエラーを受け取るが、閉じ直せず（2 回目の
/// `close` は何もしない）、編集は失われ、次に開いたときも「前回は正常に終了した」と扱われた。
///
/// 失敗は、テスト用の関数（`genzo_catalog::testing::fail_develop_writes`）で、現像設定の書き込みだけを
/// SQLite のエラーにして起こす（時間には依存しない）。
#[test]
fn a_failed_save_at_close_keeps_the_unsaved_settings_and_allows_closing_again() {
    let env = Env::new();
    let (core, a) = open_with_unsaved_develop_and_failing_saves(&env);

    // 保存に失敗したので、閉じられない（エラー）。コアは開いたままで、何も捨てていない。
    let err = core.close().expect_err("保存できないのに、閉じられた");
    assert!(!core.is_closed(), "保存できないのに、終了済みにした: {err}");
    assert_eq!(
        core.develop_settings(a).unwrap(),
        develop_exposure(0.5),
        "未保存の設定を捨てた: {err}"
    );
    // 原因が残っている間は、閉じ直しても同じ（何もしないで成功にしない）。
    assert!(core.close().is_err(), "2 回目の close が何もせずに成功した");
    assert!(!core.is_closed());
    assert!(core.flush_develop().is_err());
    assert_eq!(
        core.develop_settings(a).unwrap(),
        develop_exposure(0.5),
        "閉じ直しの失敗で、未保存の設定を捨てた"
    );

    // 原因がなくなれば、閉じ直せて、保存した設定が残る。
    genzo_catalog::testing::allow_develop_writes(env.catalog_path()).unwrap();
    core.close().unwrap();
    assert!(core.is_closed());
    let core = env.open();
    assert_eq!(
        core.startup_report().previous_shutdown,
        genzo_api::PreviousShutdown::Clean
    );
    assert_eq!(core.develop_settings(a).unwrap(), develop_exposure(0.5));
    assert_eq!(history_labels(&core, a), vec!["読み込み", "露光量 +0.50"]);
    core.close().unwrap();
}

/// 上の回帰テストの別の経路: 保存できなかった `close` の後、原因がなくなってから、閉じ直さずに drop しても
/// （drop は最後の機会として、もう一度保存する）、未保存の設定を保存して、正常に終了した印を付ける。
#[test]
fn dropping_the_core_after_a_failed_close_saves_once_the_cause_is_gone() {
    let env = Env::new();
    let (core, a) = open_with_unsaved_develop_and_failing_saves(&env);
    assert!(core.close().is_err());
    genzo_catalog::testing::allow_develop_writes(env.catalog_path()).unwrap();
    drop(core);

    let core = env.open();
    assert_eq!(
        core.startup_report().previous_shutdown,
        genzo_api::PreviousShutdown::Clean
    );
    assert_eq!(core.develop_settings(a).unwrap(), develop_exposure(0.5));
    core.close().unwrap();
}

/// drop のように、もう閉じ直せない最後の機会に保存できなければ、閉じるしかない（ロックを放し、ワーカーを
/// 終わらせる）が、カタログには「正常に終了した」印を付けず、次に開いたときに前回の異常終了として知らせる
/// （保存できなかった編集があることを、利用者が気づけるようにする。Codex のレビュー）。保存できなかった
/// ことは、警告（`SaveFailed`）でも知らせる。
///
/// 以前は、保存に失敗しても「正常に終了した」印を付けたので、次に開いても何も知らされなかった。
#[test]
fn closing_for_the_last_time_without_saving_is_not_marked_as_a_clean_shutdown() {
    let env = Env::new();
    let (core, a) = open_with_unsaved_develop_and_failing_saves(&env);
    let rx = core.subscribe();
    drop(core);
    // 保存できなかったことの警告（閉じる処理の中で出す）。
    let saved_warning = rx.try_iter().any(|e| {
        matches!(
            e,
            Event::Warning {
                code: WarningCode::SaveFailed,
                variant_id: Some(v),
                ..
            } if v == a
        )
    });
    assert!(saved_warning, "保存できなかったことを警告していない");

    genzo_catalog::testing::allow_develop_writes(env.catalog_path()).unwrap();
    let core = env.open();
    assert_eq!(
        core.startup_report().previous_shutdown,
        genzo_api::PreviousShutdown::Unclean,
        "保存できなかったのに、正常に終了した印を付けた"
    );
    assert!(
        core.startup_warnings().iter().any(|e| matches!(
            e,
            Event::Warning {
                code: WarningCode::UncleanShutdown,
                ..
            }
        )),
        "前回の異常終了を知らせていない"
    );
    core.close().unwrap();
}

/// UI の「写真を閉じる」（`close_develop`）と写真の切り替え（`open_develop`）は、保存に失敗したら、現像の
/// セッションを残す（未保存の設定の唯一の写しを捨てない）。原因がなくなってからもう一度閉じれば保存される。
/// 終了（`close`）の経路と違い、以前から守られていた経路の回帰テスト（Codex のレビューの確認）。
#[test]
fn a_failed_save_keeps_the_develop_session_when_closing_or_switching_photos() {
    let env = Env::new();
    write_jpeg(&env.photos.join("B.jpg"), (48, 32), 2, None);
    let (core, a) = open_with_unsaved_develop_and_failing_saves(&env);
    // 写真 B（取り込みは、書き込みを失敗させる前に済ませる必要がある）。
    genzo_catalog::testing::allow_develop_writes(env.catalog_path()).unwrap();
    import(&core, &env.photos);
    let b = by_name(&all_items(&core), "B.jpg").variant_id;
    genzo_catalog::testing::fail_develop_writes(env.catalog_path()).unwrap();

    // 写真の切り替え: 保存に失敗したら、切り替えず、A のセッションを残す。
    assert!(core.open_develop(b).is_err());
    assert_eq!(core.develop_settings(a).unwrap(), develop_exposure(0.5));
    // 写真を閉じる: 同じ。
    assert!(core.close_develop().is_err());
    assert_eq!(core.develop_settings(a).unwrap(), develop_exposure(0.5));

    genzo_catalog::testing::allow_develop_writes(env.catalog_path()).unwrap();
    core.close_develop().unwrap();
    assert_eq!(core.develop_settings(a).unwrap(), develop_exposure(0.5));
    assert_eq!(history_labels(&core, a), vec!["読み込み", "露光量 +0.50"]);
    core.close().unwrap();
}
