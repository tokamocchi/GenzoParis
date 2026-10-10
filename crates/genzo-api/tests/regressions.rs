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

/// キャッシュの回収（L1 のフォルダの走査に時間がかかる）の間も、ドラッグの終了（保存）が待たされない
/// （回収はカタログのロックの外でフォルダを走査し、現像の保存は現像のロックの外でカタログに書く。
/// PERF-13、指摘 F24）。
#[test]
fn saving_develop_settings_does_not_wait_for_cache_garbage_collection() {
    use std::time::{Duration, Instant};
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (48, 32), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    // L1 のフォルダに、索引にないプレビューのファイルをたくさん置く（走査に時間がかかるように）。
    let root = env.dir.path().join("data").join("previews");
    for i in 0..30_000u32 {
        let key = format!("{:064x}", u128::from(i) * 0x9E37_79B9_7F4A_7C15);
        let dir = root.join(&key[0..2]).join(&key[2..4]);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{key}.jpg")), [0xFF, 0xD8, 0xFF]).unwrap();
    }
    // 1 回目で索引に加える。2 回目の、回収だけにかかる時間を測る（3 回目と同じ条件）。
    let job = core.collect_cache_garbage().unwrap();
    common::wait_ok(&core, job);
    let started = Instant::now();
    let job = core.collect_cache_garbage().unwrap();
    common::wait_ok(&core, job);
    let alone = started.elapsed();
    if alone < Duration::from_millis(100) {
        eprintln!("回収が速すぎる（{alone:?}）ため、待たされないことの確認を飛ばす");
        core.close().unwrap();
        return;
    }
    core.open_develop(v).unwrap();
    let s = DevelopSettings {
        exposure_ev: 0.5,
        ..Default::default()
    };
    core.set_settings(&s, true).unwrap();
    let started = Instant::now();
    let job = core.collect_cache_garbage().unwrap();
    // 回収のジョブが動き始めてから、ドラッグを終える（保存する）。
    std::thread::sleep(Duration::from_millis(20));
    core.end_drag().unwrap();
    let saved_after = started.elapsed();
    common::wait_ok(&core, job);
    assert!(
        saved_after < alone / 2,
        "保存は回収を待たない（保存まで {saved_after:?}、回収だけで {alone:?}）"
    );
    assert_eq!(core.history(v).unwrap().len(), 2);
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
