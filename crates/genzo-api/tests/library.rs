//! 取り込み・検索・選別・ファイルの状態・ワーカーの異常の結合テスト（01 のストーリー 1〜3・9、
//! IMP-01・VID-01・PRV-01・LIB-04・LIB-07・LIB-08、04 の 3.2 節・3.3 節・3.5 節・3.7 節・6.3 節）。

mod common;

use std::time::Duration;

use common::{
    Env, JST_MINUTES, all_items, by_name, drain, exif, hashes, import, items_of, make_dng,
    make_video, names, wait_done, write_jpeg, write_plain_image,
};
use genzo_api::{
    ApiError, AssetKind, ErrorKind, Event, FileStatus, JobState, Lane, Rating, SearchFilter,
    SearchSort, SortDirection, SortKey, TestHook, WarningCode,
};
use genzo_model::{ColorLabel, Flag, GpsCoord, TzSource};
use genzo_worker::{GarbageMode, JobTimeouts, WorkerClientError};

/// 混在したフォルダの中身（撮影日時のあるもの・ないもの・壊れたもの）。
struct Fixture {
    /// 撮影日時順に並ぶはずのもの（撮影日時のあるもの）。
    dated: Vec<&'static str>,
    /// 撮影日時がない（最後に並ぶ）もの。
    undated: Vec<&'static str>,
    /// 読めない（status = error）もの。
    broken: &'static str,
    has_video: bool,
}

fn populate(env: &Env) -> Fixture {
    let p = &env.photos;
    let gps = GpsCoord::new(35.0116, 135.7681);
    // 日本時間（Exif の OffsetTimeOriginal あり）。
    write_jpeg(
        &p.join("IMG_0002.jpg"),
        (64, 48),
        2,
        Some(&exif(Some("2024-05-01 10:00:00"), Some(JST_MINUTES), gps)),
    );
    // オフセットなし（既定のタイムゾーンで推定する）。
    write_jpeg(
        &p.join("IMG_0001.jpg"),
        (48, 64),
        1,
        Some(&exif(Some("2024-05-01 09:00:00"), None, None)),
    );
    write_plain_image(&p.join("sub").join("IMG_0003.png"), (40, 30), 3);
    write_plain_image(&p.join("scan.tif"), (30, 40), 4);
    std::fs::write(p.join("broken.jpg"), b"this is not a jpeg").unwrap();
    std::fs::write(p.join("notes.txt"), b"ignored").unwrap();
    let mut dated = Vec::new();
    // DNG（LibRaw があれば）: 08:00 JST が最初。
    if make_dng(&p.join("RAW_0001.dng"), "2024:05:01 08:00:00") {
        dated.push("RAW_0001.dng");
    }
    dated.push("IMG_0001.jpg");
    // 動画（ffmpeg があれば）: 00:30 UTC = 09:30 JST。
    let has_video = make_video(&p.join("clip.mp4"), "2024-05-01T00:30:00Z");
    if has_video {
        dated.push("clip.mp4");
    }
    dated.push("IMG_0002.jpg");
    // 配下のフォルダへのシンボリックリンク（たどらない。3.5 節）。
    #[cfg(unix)]
    std::os::unix::fs::symlink(p.join("sub"), p.join("link-to-sub")).unwrap();
    Fixture {
        dated,
        undated: vec!["IMG_0003.png", "scan.tif", "broken.jpg"],
        broken: "broken.jpg",
        has_video,
    }
}

/// テスト 1: 混在したフォルダを取り込み、撮影日時順に写真と動画が並ぶ（撮影日時のないものは最後）。
/// もう一度取り込んでも件数が増えない。元ファイルは変わらない（DATA-01）。
#[test]
fn import_mixed_folder_orders_by_capture_time_and_is_idempotent() {
    let env = Env::new();
    let fx = populate(&env);
    let before = hashes(&env.photos);
    let core = env.open();
    let events = core.subscribe();

    let report = import(&core, &env.photos);
    let total = fx.dated.len() + fx.undated.len();
    assert_eq!(report.files_found as usize, total, "{report:?}");
    assert_eq!(report.added as usize, total, "{report:?}");
    assert!(report.not_registered.is_empty(), "{report:?}");
    // 読めないファイルは status = error と理由を記録して続ける（6.3 節）。
    assert_eq!(report.errors.len(), 1, "{report:?}");
    assert!(report.errors[0].path.ends_with(fx.broken));
    assert!(!report.errors[0].reason.is_empty());
    #[cfg(unix)]
    assert_eq!(report.skipped_links, 1);

    let items = all_items(&core);
    let order = names(&items);
    assert_eq!(order.len(), total);
    assert_eq!(&order[..fx.dated.len()], fx.dated.as_slice(), "{order:?}");
    let mut tail: Vec<String> = order[fx.dated.len()..].to_vec();
    tail.sort();
    let mut expected: Vec<String> = fx.undated.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(tail, expected, "撮影日時のないものは最後");

    // 撮影日時（3.1 節）。
    let img2 = by_name(&items, "IMG_0002.jpg");
    assert_eq!(img2.capture.tz_source, TzSource::Exif);
    assert_eq!(
        img2.capture.utc.unwrap().to_rfc3339(),
        "2024-05-01T01:00:00+00:00"
    );
    let img1 = by_name(&items, "IMG_0001.jpg");
    assert_eq!(img1.capture.tz_source, TzSource::UserDefault);
    assert_eq!(img1.capture.tz_assumed.as_deref(), Some("+09:00"));
    let broken = by_name(&items, fx.broken);
    assert_eq!(broken.file_status, FileStatus::Error);
    assert!(broken.capture.utc.is_none());
    if fx.has_video {
        let clip = by_name(&items, "clip.mp4");
        assert_eq!(clip.kind, AssetKind::Video);
        let details = core.variant_details(clip.variant_id).unwrap();
        let video = details.video.unwrap();
        assert!(video.duration_s.unwrap() > 0.5, "{video:?}");
    }

    // サムネイル（L0。PRV-01）: 読めたファイルにはある。壊れたファイルにはない。
    for s in &items {
        let t = core.thumbnail(s.variant_id).unwrap();
        if s.file_name == fx.broken {
            assert!(t.is_none());
            continue;
        }
        let t = t.unwrap_or_else(|| panic!("{} のサムネイルがない", s.file_name));
        assert_eq!(&t.jpeg[..3], &[0xFF, 0xD8, 0xFF]);
        assert_eq!(t.rev.len(), 64);
        assert_eq!(s.thumb_rev.as_deref(), Some(t.rev.as_str()));
        let decoded = genzo_media::decode_image_bytes(&t.jpeg).unwrap();
        assert!(decoded.profile.is_embedded(), "B5 は ICC 付き");
    }

    // 進捗とカタログの変更のイベント。
    let ev = drain(&events);
    assert!(ev.iter().any(|e| matches!(e, Event::JobProgress { .. })));
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::JobFinished {
            state: JobState::Succeeded,
            ..
        }
    )));
    assert!(ev.iter().any(|e| matches!(e, Event::CatalogChanged { .. })));

    // もう一度取り込んでも件数が増えない（冪等。3.5 節）。
    let again = import(&core, &env.photos);
    assert_eq!(again.added, 0, "{again:?}");
    assert_eq!(again.unchanged as usize, total, "{again:?}");
    assert_eq!(all_items(&core).len(), total);
    // フォルダ（LIB-06）。
    let folders = core.folders().unwrap();
    assert!(folders.iter().any(|f| f.rel_path.ends_with("photos/sub")));
    core.close().unwrap();
    assert_eq!(
        hashes(&env.photos),
        before,
        "元ファイルは変わらない（DATA-01）"
    );
}

/// 取り込みを取り消しても、やり直せば続きから処理して、件数が正しくなる。
#[test]
fn cancelled_import_can_be_resumed() {
    let env = Env::new();
    for i in 0..12 {
        write_jpeg(
            &env.photos.join(format!("P{i:03}.jpg")),
            (32, 24),
            i as u8,
            Some(&exif(Some("2024-06-01 12:00:00"), Some(JST_MINUTES), None)),
        );
    }
    let core = env.open();
    let job = core.import_folder(&env.photos, true).unwrap();
    core.cancel_job(job).unwrap();
    let info = wait_done(&core, job);
    // 取り消しが間に合ったかどうかは、どちらでもよい（結果は取り消し・成功のどちらか）。
    assert!(
        matches!(info.state, JobState::Cancelled | JobState::Succeeded),
        "{info:?}"
    );
    let report = import(&core, &env.photos);
    assert_eq!(report.added + report.unchanged, 12, "{report:?}");
    assert_eq!(all_items(&core).len(), 12);
    // 取り消しの対象がない・知らないジョブ。
    assert!(matches!(core.cancel_job(9999), Err(ApiError::NotFound(_))));
}

/// テスト 2: 評価を付けて「★3 以上」「動画のみ」「テキスト」で絞り込む。古い世代の range は Stale。
/// 評価の変更で結果の作り直しが必要なら、新しい世代としてイベントで知らせる（3.7 節）。
#[test]
fn filters_generations_and_marks() {
    let env = Env::new();
    let fx = populate(&env);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let v1 = by_name(&items, "IMG_0001.jpg").variant_id;
    let v2 = by_name(&items, "IMG_0002.jpg").variant_id;
    let v3 = by_name(&items, "IMG_0003.png").variant_id;
    assert_eq!(
        core.set_rating(&[v1, v2], Rating::new(4).unwrap()).unwrap(),
        2
    );
    core.set_rating(&[v3], Rating::new(3).unwrap()).unwrap();
    core.set_flag(&[v1], Flag::Picked).unwrap();
    core.set_color_label(&[v2], Some(ColorLabel::Red)).unwrap();
    core.set_caption(&[v3], Some("京都旅行の夕焼け")).unwrap();

    let three_up = SearchFilter {
        rating_min: Rating::new(3),
        ..Default::default()
    };
    let r = core.search(&three_up, SearchSort::default()).unwrap();
    assert_eq!(r.count, 3);
    let got = items_of(&core, r.generation, r.count);
    assert_eq!(
        names(&got),
        vec!["IMG_0001.jpg", "IMG_0002.jpg", "IMG_0003.png"]
    );
    assert_eq!(got[0].flag, Flag::Picked);
    assert_eq!(got[1].color_label, Some(ColorLabel::Red));

    // 動画だけ。
    let videos = SearchFilter {
        kind: Some(AssetKind::Video),
        ..Default::default()
    };
    let r = core.search(&videos, SearchSort::default()).unwrap();
    assert_eq!(r.count, u64::from(fx.has_video));

    // テキスト（ファイル名とキャプション。3.6 節）。
    let text = |t: &str| SearchFilter {
        text: Some(t.to_owned()),
        ..Default::default()
    };
    let r = core
        .search(&text("IMG_0002"), SearchSort::default())
        .unwrap();
    assert_eq!(
        names(&items_of(&core, r.generation, r.count)),
        vec!["IMG_0002.jpg"]
    );
    let r = core.search(&text("夕焼け"), SearchSort::default()).unwrap();
    assert_eq!(
        names(&items_of(&core, r.generation, r.count)),
        vec!["IMG_0003.png"]
    );
    let r = core.search(&text("京都"), SearchSort::default()).unwrap();
    assert_eq!(r.count, 1, "2 文字の語も部分一致で探せる");

    // 期間と並べ替え（評価の高い順）。
    let period = |from: chrono::DateTime<chrono::Utc>| SearchFilter {
        captured_from: Some(from),
        captured_until: Some(common::utc(2024, 5, 2)),
        ..Default::default()
    };
    // 撮影日時は 2024-04-30T23:00Z（DNG）〜 2024-05-01T01:00Z。撮影日時のないものは期間で除く。
    let r = core
        .search(&period(common::utc(2024, 4, 30)), SearchSort::default())
        .unwrap();
    assert_eq!(r.count as usize, fx.dated.len());
    let r = core
        .search(
            &period(common::utc(2024, 5, 1) + chrono::TimeDelta::minutes(45)),
            SearchSort::default(),
        )
        .unwrap();
    assert_eq!(
        names(&items_of(&core, r.generation, r.count)),
        vec!["IMG_0002.jpg"]
    );
    let by_rating = SearchSort {
        key: SortKey::Rating,
        direction: SortDirection::Descending,
    };
    let r = core.search(&SearchFilter::default(), by_rating).unwrap();
    let got = items_of(&core, r.generation, r.count);
    assert_eq!(got[0].rating, Rating::new(4).unwrap());

    // 古い世代の range は Stale（3.7 節）。
    let g1 = core.search(&three_up, SearchSort::default()).unwrap();
    let g2 = core.search(&three_up, SearchSort::default()).unwrap();
    assert!(g2.generation > g1.generation);
    let err = core.range(g1.generation, 0, 10).unwrap_err();
    assert!(matches!(err, ApiError::Stale { .. }), "{err:?}");
    assert_eq!(err.info().kind, ErrorKind::Stale);
    assert!(err.info().retryable);
    assert_eq!(core.range(g2.generation, 0, 10).unwrap().items.len(), 3);
    assert!(
        core.range(g2.generation, 0, genzo_api::MAX_RANGE_LEN + 1)
            .is_err()
    );

    // 「★3 以上」の表示中に評価を 2 に下げても、条件を適用し直すまで残す（3.7 節）。
    let events = core.subscribe();
    core.set_rating(&[v3], Rating::new(2).unwrap()).unwrap();
    let cur = core.current_search().unwrap();
    assert_eq!(
        cur.generation, g2.generation,
        "結果は変わらないので世代も変わらない"
    );
    assert_eq!(core.index_of(v3).unwrap().index, Some(2));
    // 表示していない写真を ★5 にすると、結果が変わるので新しい世代になる。
    let scan = by_name(&items, "scan.tif").variant_id;
    core.set_rating(&[scan], Rating::new(5).unwrap()).unwrap();
    let cur = core.current_search().unwrap();
    assert!(cur.generation > g2.generation);
    assert_eq!(cur.count, 4);
    let ev = drain(&events);
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::SearchUpdated { generation, count: 4 } if *generation == cur.generation
    )));
    // 古い世代は Stale。選択中の variant は id で位置を求め直せる。
    assert!(matches!(
        core.range(g2.generation, 0, 10),
        Err(ApiError::Stale { .. })
    ));
    let idx = core.index_of(v3).unwrap();
    assert_eq!(idx.generation, cur.generation);
    assert!(idx.index.is_some());
    // 条件を適用し直すと、★2 にした写真は消える。
    let r = core.search(&three_up, SearchSort::default()).unwrap();
    assert_eq!(r.count, 3);
    assert_eq!(core.index_of(v3).unwrap().index, None);

    // JSON のコマンドでも同じことができる（Tauri のコマンドの入口）。
    let out = core.execute_json(r#"{"type":"search","filter":{"rating_min":4}}"#);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["ok"]["type"], "search");
    assert_eq!(v["ok"]["count"], 3);
    let out = core.execute_json(r#"{"type":"range","generation":1,"start":0,"len":1}"#);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["error"]["kind"], "stale");
    core.close().unwrap();
}

/// ファイルの状態の確認（3.3 節）: なくなったファイルは missing、戻れば ok。
#[test]
fn missing_files_are_detected_and_restored() {
    let env = Env::new();
    let a = env.photos.join("A.jpg");
    write_jpeg(&a, (32, 24), 1, None);
    write_jpeg(&env.photos.join("B.jpg"), (32, 24), 2, None);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let va = by_name(&items, "A.jpg").variant_id;
    let moved = env.dir.path().join("A.jpg.moved");
    std::fs::rename(&a, &moved).unwrap();
    let report = core.check_files(&[va]).unwrap();
    assert_eq!(report.missing.len(), 1);
    assert_eq!(
        by_name(&all_items(&core), "A.jpg").file_status,
        FileStatus::Missing
    );
    // 見つからないファイルだけで絞り込める。
    let r = core
        .search(
            &SearchFilter {
                file_statuses: Some(vec![FileStatus::Missing]),
                ..Default::default()
            },
            SearchSort::default(),
        )
        .unwrap();
    assert_eq!(r.count, 1);
    // 戻すと ok に戻る（内容は同じなのでリビジョンは変わらない）。
    std::fs::rename(&moved, &a).unwrap();
    let report = core.check_files(&[va]).unwrap();
    assert_eq!((report.ok, report.missing.len()), (1, 0));
    assert!(report.changed.is_empty());
    let s = by_name(&all_items(&core), "A.jpg").clone();
    assert_eq!(s.file_status, FileStatus::Ok);
    assert_eq!(s.file_revision, 1);
    // 内容が変わったらリビジョンを上げる（キャッシュを無効にする。4.1 節）。
    write_jpeg(&a, (40, 30), 9, None);
    let report = core.check_files(&[va]).unwrap();
    assert_eq!(report.changed, vec![a.clone()]);
    assert_eq!(by_name(&all_items(&core), "A.jpg").file_revision, 2);
    core.close().unwrap();
}

/// テスト 7: ワーカーの異常（テスト用の口）で Core が落ちず、続けて使える（6.3 節。SEC-05）。
#[test]
fn worker_failures_do_not_bring_down_the_core() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (32, 24), 1, None);
    // ハングのテスト用に、テスト用の口のタイムアウトだけ短くする。
    let core = env.open_with(|c| {
        c.worker_timeouts = Some(JobTimeouts {
            test_hook: Duration::from_secs(2),
            ..JobTimeouts::uniform(common::TEST_JOB_TIMEOUT)
        });
    });
    // 異常終了。
    let err = core
        .run_worker_test_hook(Lane::Batch, TestHook::Crash { with_shm: true })
        .unwrap_err();
    assert!(
        matches!(err, ApiError::Worker(WorkerClientError::Crashed { .. })),
        "{err:?}"
    );
    assert_eq!(err.info().kind, ErrorKind::Worker);
    // 不正な応答。
    let err = core
        .run_worker_test_hook(Lane::Interactive, TestHook::Garbage(GarbageMode::NotJson))
        .unwrap_err();
    assert!(
        matches!(err, ApiError::Worker(WorkerClientError::Protocol(_))),
        "{err:?}"
    );
    // ハング（タイムアウトで強制終了して再起動）。
    let err = core
        .run_worker_test_hook(Lane::Batch, TestHook::Hang { with_shm: false })
        .unwrap_err();
    assert!(
        matches!(err, ApiError::Worker(WorkerClientError::Timeout { .. })),
        "{err:?}"
    );
    // 再起動したワーカーで、取り込み・検索・現像ができる。
    let report = import(&core, &env.photos);
    assert_eq!(report.added, 1);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    core.render_develop().unwrap();
    core.close().unwrap();
    // テスト用の口が無効なら使えない。
    let core = env.open_with(|c| c.worker_test_hooks = false);
    assert!(matches!(
        core.run_worker_test_hook(Lane::Batch, TestHook::Crash { with_shm: false }),
        Err(ApiError::InvalidArgument(_))
    ));
}

/// 起動時の確認: 前回の異常終了を警告として知らせる（DATA-05。後から購読しても届く）。
#[test]
fn unclean_shutdown_is_reported_at_startup() {
    let env = Env::new();
    std::fs::create_dir_all(env.catalog_path().parent().unwrap()).unwrap();
    // カタログを開いて、閉じずに終わる（異常終了の代わり）。
    let cat = genzo_catalog::Catalog::open(env.catalog_path()).unwrap();
    drop(cat);
    let core = env.open();
    assert_eq!(
        core.startup_report().previous_shutdown,
        genzo_api::PreviousShutdown::Unclean
    );
    let rx = core.subscribe();
    let ev = drain(&rx);
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::Warning {
            code: WarningCode::UncleanShutdown,
            ..
        }
    )));
    core.close().unwrap();
    // 正常に閉じた後は警告しない。
    let core = env.open();
    assert_eq!(
        core.startup_report().previous_shutdown,
        genzo_api::PreviousShutdown::Clean
    );
    assert!(core.startup_warnings().is_empty());
    core.close().unwrap();
    // 閉じた後の操作は Closed。
    assert!(matches!(
        core.search(&SearchFilter::default(), SearchSort::default()),
        Err(ApiError::Closed)
    ));
}
