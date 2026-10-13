//! implementation_status の「人の判断・確認が必要な事項」の決定（No.11・24・27・31・32・45・52）を
//! 実装したことの結合テスト。
//!
//! 各テストは、決定の振る舞いを結果で確かめる（時間では決めない。待つときは上限だけを置く）。

mod common;

use std::path::Path;
use std::time::Duration;

use chrono::{FixedOffset, TimeZone, Utc};
use common::{Env, all_items, by_name, drain, wait_until};
use genzo_api::{
    ApiError, CoreConfig, DevelopSettings, Event, KEY_DEFAULT_UTC_OFFSET_MINUTES, Rating,
    RegionRequest, RegionSpace, SearchFilter, SearchSort, SettingGroups, SettingsUpdate, VariantId,
};
use genzo_catalog::{FileFacts, RegisterFile};
use genzo_model::{CaptureTime, PhotoMetadata, TzSource, UTC_ONLY_OFFSET, VideoMetadata};

/// 検索結果の作り直しを待つ上限。
const REFRESH_LIMIT: Duration = Duration::from_secs(120);

/// 写真を `n` 件、カタログに直接登録する（ファイルは作らない。撮影日時は 1 分ずつずらす）。
fn register_photos(catalog: &Path, n: usize) {
    std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    let mut cat = genzo_catalog::Catalog::open(catalog).unwrap();
    let volume = cat
        .ensure_volume("path:/genzo-test-none", None, Some("/genzo-test-none"))
        .unwrap();
    let folder = cat.ensure_folder(volume, "photos").unwrap();
    let jst = FixedOffset::east_opt(9 * 3600).unwrap();
    let requests: Vec<RegisterFile> = (0..n)
        .map(|i| {
            let when = format!("2024:05:01 {:02}:{:02}:00", 8 + i / 60, i % 60);
            RegisterFile::photo(
                folder,
                format!("P{i:05}.jpg"),
                facts(i as u64),
                PhotoMetadata::default(),
                CaptureTime::resolve_lossy(Some(&when), Some("+09:00"), jst),
            )
        })
        .collect();
    for chunk in requests.chunks(500) {
        cat.register_batch(chunk).unwrap();
    }
    cat.close().unwrap();
}

/// テスト用のファイルの情報（ファイルは作らない）。
fn facts(n: u64) -> FileFacts {
    FileFacts {
        size: n + 1,
        mtime_ns: 1,
        quick_hash: format!("{n:064x}"),
    }
}

/// 版 3 のカタログを作る: UTC の印 `Z` だけの動画（`CLIP.MP4`。版 3 までの取り込みの記録の形）と、
/// オフセットが +02:00 の写真（`PHOTO.JPG`）。既定のタイムゾーン（+09:00）は保存済みにする（移行だけで
/// 推定し直しが動くことを確かめるため）。動画の撮影した瞬間（UTC）を返す。
fn write_version_3_catalog_with_utc_only_clip(path: &Path) -> chrono::DateTime<Utc> {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let instant = Utc.with_ymd_and_hms(2024, 5, 1, 3, 0, 0).unwrap();
    let mut cat = genzo_catalog::testing::open_at_version(path, 3).unwrap();
    cat.set_setting(KEY_DEFAULT_UTC_OFFSET_MINUTES, "540")
        .unwrap();
    let volume = cat
        .ensure_volume("path:/genzo-test-none", None, Some("/genzo-test-none"))
        .unwrap();
    let folder = cat.ensure_folder(volume, "clips").unwrap();
    let legacy = CaptureTime {
        raw: Some("2024-05-01T03:00:00.000000Z".to_owned()),
        offset: Some("+00:00".to_owned()),
        tz_source: TzSource::Exif,
        tz_assumed: Some("+00:00".to_owned()),
        correction_s: 0,
        utc: Some(instant),
    };
    cat.register_file(&RegisterFile::video(
        folder,
        "CLIP.MP4",
        facts(1),
        VideoMetadata {
            creation_time: legacy.raw.clone(),
            ..Default::default()
        },
        legacy,
    ))
    .unwrap();
    let photo = CaptureTime::resolve(
        Some("2024:05:01 12:00:00"),
        Some("+02:00"),
        FixedOffset::east_opt(0).unwrap(),
    )
    .unwrap();
    cat.register_file(&RegisterFile::photo(
        folder,
        "PHOTO.JPG",
        facts(2),
        PhotoMetadata::default(),
        photo,
    ))
    .unwrap();
    cat.close().unwrap();
    instant
}

/// No.32: 版 3 のカタログ（UTC の印 `Z` だけの動画の撮影日時を、+00:00 のオフセットが記録されたもの
/// （`exif`）として記録した）をコアで開くと、`utc_only` に移行し、表示のオフセットを既定のタイムゾーンに
/// する（UTC は変えない）。既定のタイムゾーンを変えると、表示のオフセットだけが変わる。
#[test]
fn utc_only_capture_times_are_migrated_and_follow_the_default_time_zone() {
    let env = Env::new();
    let path = env.catalog_path();
    let instant = write_version_3_catalog_with_utc_only_clip(&path);

    let core = env.open();
    assert_eq!(core.startup_report().migrated_from, Some(3));
    let clip_shown_in = |offset: &str| {
        let items = all_items(&core);
        by_name(&items, "CLIP.MP4").capture.tz_assumed.as_deref() == Some(offset)
    };
    wait_until(
        "utc_only の表示のオフセットが既定のタイムゾーンになる",
        || clip_shown_in("+09:00"),
    );
    let items = all_items(&core);
    let clip = &by_name(&items, "CLIP.MP4").capture;
    assert_eq!(clip.tz_source, TzSource::UtcOnly);
    assert_eq!(clip.offset.as_deref(), Some(UTC_ONLY_OFFSET));
    assert_eq!(clip.utc, Some(instant), "UTC は変えない");
    assert_eq!(clip.to_string(), "2024-05-01 12:00:00 +09:00");
    let photo = &by_name(&items, "PHOTO.JPG").capture;
    assert_eq!(photo.tz_source, TzSource::Exif);
    assert_eq!(photo.tz_assumed.as_deref(), Some("+02:00"));

    // 既定のタイムゾーンを変えると、表示のオフセットだけが変わる。
    core.update_settings(&SettingsUpdate {
        default_utc_offset_minutes: Some(120),
        ..Default::default()
    })
    .unwrap();
    wait_until(
        "utc_only の表示のオフセットが新しい既定になる",
        || clip_shown_in("+02:00"),
    );
    let items = all_items(&core);
    assert_eq!(by_name(&items, "CLIP.MP4").capture.utc, Some(instant));
    assert_eq!(
        by_name(&items, "PHOTO.JPG").capture.tz_assumed.as_deref(),
        Some("+02:00")
    );
    core.close().unwrap();
    // もう一度開いても移行しない（版は最新）。
    let core = env.open();
    assert_eq!(core.startup_report().migrated_from, None);
    core.close().unwrap();
}

/// No.32: 移行した直後の推定し直し（`utc_only` の表示のオフセットを既定のタイムゾーンにする）が終わる前に
/// アプリが終わっても（強制終了・推定し直しの途中で閉じた）、次に開いたときに推定し直す（表示のオフセットが
/// +00:00 のまま残らない）。ここでは、コアを通さずにカタログだけを開いて移行した状態（移行は済み、
/// 推定し直しはまだ）を作る。
#[test]
fn utc_only_offsets_are_fixed_even_if_the_first_open_after_migration_was_interrupted() {
    /// 推定し直しを待つ上限（ジョブは数件の asset なのですぐ終わる）。
    const LIMIT: Duration = Duration::from_secs(60);
    let env = Env::new();
    let path = env.catalog_path();
    let instant = write_version_3_catalog_with_utc_only_clip(&path);
    // 移行だけが済んだ（推定し直しのジョブが書く前に終わった）。
    let cat = genzo_catalog::Catalog::open(&path).unwrap();
    assert_eq!(cat.open_report().migrated_from, Some(3));
    cat.close().unwrap();

    let core = env.open();
    assert_eq!(core.startup_report().migrated_from, None);
    let clip_offset = || {
        let items = all_items(&core);
        by_name(&items, "CLIP.MP4").capture.tz_assumed.clone()
    };
    let deadline = std::time::Instant::now() + LIMIT;
    while clip_offset().as_deref() != Some("+09:00") {
        assert!(
            std::time::Instant::now() < deadline,
            "utc_only の表示のオフセットが {LIMIT:?} 以内に既定のタイムゾーンにならない（{:?}）",
            clip_offset()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let items = all_items(&core);
    assert_eq!(by_name(&items, "CLIP.MP4").capture.utc, Some(instant));
    // 推定し直しを終えたら、済んでいない印を外す（次に開いたときは推定し直さない）。
    while core.running_jobs() > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "ジョブが {LIMIT:?} 以内に終わらない"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    core.close().unwrap();
    let cat = genzo_catalog::Catalog::open(&path).unwrap();
    assert!(!cat.capture_time_reresolve_pending().unwrap());
    cat.close().unwrap();

    // 既定のタイムゾーンを変えた後、推定し直しを終える前に終わった状態（設定と印は保存済み、撮影日時は
    // 古いオフセットのまま）も、次に開いたときに推定し直す。
    let mut cat = genzo_catalog::Catalog::open(&path).unwrap();
    cat.set_capture_time_reresolve_pending(true).unwrap();
    cat.set_setting(KEY_DEFAULT_UTC_OFFSET_MINUTES, "120")
        .unwrap();
    cat.close().unwrap();
    let core = env.open();
    let deadline = std::time::Instant::now() + LIMIT;
    loop {
        let items = all_items(&core);
        if by_name(&items, "CLIP.MP4").capture.tz_assumed.as_deref() == Some("+02:00") {
            assert_eq!(by_name(&items, "CLIP.MP4").capture.utc, Some(instant));
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "既定のタイムゾーンの変更が {LIMIT:?} 以内に反映されない"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    while core.running_jobs() > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "ジョブが {LIMIT:?} 以内に終わらない"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    core.close().unwrap();
    let cat = genzo_catalog::Catalog::open(&path).unwrap();
    assert!(!cat.capture_time_reresolve_pending().unwrap());
    cat.close().unwrap();
}

/// No.11・27: 検索結果の作り直しはバックグラウンドで行い（カタログを変える操作は待たない）、結果が
/// 変われば新しい世代としてイベントで知らせる。古い世代の range は Stale、選択は id で追跡できる。
/// 作り直している間に条件を適用し直しても、古い条件の結果で上書きしない（3.7 節）。
#[test]
fn search_results_are_refreshed_in_the_background() {
    const N: usize = 300;
    let env = Env::new();
    register_photos(&env.catalog_path(), N);
    let core = env.open();
    let all = core
        .search(&SearchFilter::default(), SearchSort::default())
        .unwrap();
    assert_eq!(all.count, N as u64);
    let ids = core.result_ids(all.generation).unwrap();
    let three_up = SearchFilter {
        rating_min: Rating::new(3),
        ..Default::default()
    };
    let r = core.search(&three_up, SearchSort::default()).unwrap();
    assert_eq!(r.count, 0);
    let events = core.subscribe();

    // 続けて評価を付ける（1 件ずつ 10 回）。作り直しはまとめて行い、最後の状態に合う結果になる。
    let rated: Vec<VariantId> = ids.iter().step_by(30).copied().collect();
    assert_eq!(rated.len(), 10);
    for &v in &rated {
        core.set_rating(&[v], Rating::new(5).unwrap()).unwrap();
    }
    let cur = core
        .wait_search_refreshed(Some(REFRESH_LIMIT))
        .unwrap()
        .unwrap();
    assert_eq!(cur.count, 10);
    assert!(cur.generation > r.generation);
    assert_eq!(
        core.result_ids(cur.generation).unwrap(),
        rated,
        "撮影日時順"
    );
    let ev = drain(&events);
    let updates: Vec<(u64, u64)> = ev
        .iter()
        .filter_map(|e| match e {
            Event::SearchUpdated {
                generation, count, ..
            } => Some((*generation, *count)),
            _ => None,
        })
        .collect();
    assert_eq!(updates.last(), Some(&(cur.generation, 10)), "{updates:?}");
    assert!(updates.len() <= rated.len(), "{updates:?}");
    // 世代は増える一方（古い結果で新しい結果を上書きしない）。
    assert!(updates.windows(2).all(|w| w[0].0 < w[1].0), "{updates:?}");
    // 古い世代の range は Stale。選択中の variant は id で位置を求め直せる。
    assert!(matches!(
        core.range(r.generation, 0, 10),
        Err(ApiError::Stale { .. })
    ));
    let idx = core.index_of(rated[3]).unwrap();
    assert_eq!((idx.generation, idx.index), (cur.generation, Some(3)));

    // 評価を下げた写真は、条件を適用し直すまで残す（3.7 節）。
    core.set_rating(&[rated[0]], Rating::new(1).unwrap())
        .unwrap();
    let after = core
        .wait_search_refreshed(Some(REFRESH_LIMIT))
        .unwrap()
        .unwrap();
    assert_eq!(after.count, 10);
    assert_eq!(core.index_of(rated[0]).unwrap().index, Some(0));

    // 作り直している間に条件を適用し直しても、古い条件の結果で上書きしない（何度か繰り返して、
    // 作り直しの途中に重なる場合を含める）。
    for (i, &v) in ids.iter().enumerate().skip(1).step_by(37).take(8) {
        core.search(&three_up, SearchSort::default()).unwrap();
        core.set_rating(&[v], Rating::new(4).unwrap()).unwrap();
        let b = core
            .search(&SearchFilter::default(), SearchSort::default())
            .unwrap();
        let cur = core
            .wait_search_refreshed(Some(REFRESH_LIMIT))
            .unwrap()
            .unwrap();
        assert_eq!(cur, b, "{i}: 条件を適用し直した結果のまま");
        assert_eq!(core.result_ids(b.generation).unwrap().len(), N);
    }
    core.close().unwrap();
    // 閉じた後は待てない。
    assert!(matches!(
        core.wait_search_refreshed(None),
        Err(ApiError::Closed)
    ));
}

/// No.45: 読めずに `error` で登録したファイルが、見つからない（`missing`）を経て同じ内容で戻ったとき、
/// メタデータを読み直す（以前は `ok` に戻すだけで、メタデータが空のままだった）。ffmpeg がなければ飛ばす。
#[test]
fn files_restored_after_missing_are_read_again_when_metadata_is_empty() {
    let env = Env::new();
    let clip = env.photos.join("clip.mp4");
    if !common::make_video(&clip, "2024-05-01T03:34:56Z") {
        return;
    }
    // ffprobe がない状態で取り込む（読めずに error で登録する）。
    let missing_ffprobe = env.dir.path().join("no-such-ffprobe");
    let core = env.open_with(|c| {
        c.worker_env.push((
            genzo_media::video::ENV_FFPROBE.into(),
            missing_ffprobe.clone().into_os_string(),
        ));
    });
    let first = common::import(&core, &env.photos);
    assert_eq!((first.added, first.errors.len()), (1, 1), "{first:?}");
    let v = all_items(&core)[0].variant_id;
    // ファイルを外す（見つからない）。
    let aside = env.dir.path().join("clip-aside.mp4");
    std::fs::rename(&clip, &aside).unwrap();
    let report = core.check_files(&[v]).unwrap();
    assert_eq!(report.missing.len(), 1, "{report:?}");
    assert_eq!(
        all_items(&core)[0].file_status,
        genzo_api::FileStatus::Missing
    );
    core.close().unwrap();

    // 同じ内容で戻す（名前の変更なので更新日時も同じ）。ffprobe がある状態で確かめる。
    std::fs::rename(&aside, &clip).unwrap();
    let core = env.open();
    let report = core.check_files(&[v]).unwrap();
    assert!(report.missing.is_empty(), "{report:?}");
    let items = all_items(&core);
    let s = &items[0];
    assert_eq!(s.file_status, genzo_api::FileStatus::Ok);
    assert_eq!(
        (s.width, s.height),
        (Some(96), Some(64)),
        "メタデータを読み直した"
    );
    assert_eq!(
        s.capture.utc.map(|t| t.to_rfc3339()).as_deref(),
        Some("2024-05-01T03:34:56+00:00")
    );
    let details = core.variant_details(v).unwrap();
    assert!(details.video.is_some(), "動画の情報が入る");
    // もう一度確かめても変化なし。取り込み直しても解析を省く。
    let again = core.check_files(&[v]).unwrap();
    assert!(
        again.changed.is_empty() && again.missing.is_empty(),
        "{again:?}"
    );
    let reimport = common::import(&core, &env.photos);
    assert_eq!(
        (reimport.unchanged, reimport.updated),
        (1, 0),
        "{reimport:?}"
    );
    core.close().unwrap();
}

/// No.24: 取り込みの後に、現像結果から L0 / L1 を作り直さない（既定）。埋め込みのプレビュー（B5）を L0 に
/// 使い、表示に L1 が要るとき（`request_preview`）に作る。
#[test]
fn previews_are_not_rendered_after_import_by_default_and_made_on_request() {
    assert!(
        !CoreConfig::new("/c.db", "/d").render_previews_after_import,
        "既定は作り直さない"
    );
    let env = Env::new();
    common::write_jpeg(&env.photos.join("A.jpg"), (120, 80), 1, None);
    // テストの共通の設定（作り直さない）ではなく、既定の値で開く。
    let core = env.open_with(|c| {
        c.render_previews_after_import =
            CoreConfig::new("/c.db", "/d").render_previews_after_import;
    });
    common::import(&core, &env.photos);
    core.wait_idle();
    let v = all_items(&core)[0].variant_id;
    let t0 = core
        .thumbnail(v)
        .unwrap()
        .expect("埋め込みのプレビューの L0");
    assert!(core.preview_path(v).unwrap().is_none(), "L1 は作らない");
    assert_eq!(core.thumbnail(v).unwrap().unwrap().rev, t0.rev);

    // 表示に要るときに作る（なければ予約して None）。JSON のコマンドでも同じ。
    let events = core.subscribe();
    assert!(core.request_preview(v).unwrap().is_none());
    wait_until("L1 ができる", || {
        core.preview_path(v).unwrap().is_some()
    });
    let path = core.request_preview(v).unwrap().expect("できた L1 を返す");
    assert_eq!(Some(path.clone()), core.preview_path(v).unwrap());
    let decoded = genzo_media::decode_image_file(&path).unwrap();
    assert_eq!((decoded.pixels.width(), decoded.pixels.height()), (120, 80));
    assert_ne!(
        core.thumbnail(v).unwrap().unwrap().rev,
        t0.rev,
        "L0 も現像結果に置き換わる"
    );
    core.wait_idle();
    assert!(
        drain(&events)
            .iter()
            .any(|e| matches!(e, Event::ThumbnailUpdated { variant_id, .. } if *variant_id == v))
    );
    let out = core.execute_json(&format!(
        r#"{{"type":"request_preview","variant_id":{}}}"#,
        v.get()
    ));
    assert!(out.contains(r#""path""#), "{out}");
    core.close().unwrap();
}

/// No.24: 表示のための L1 の要求（`request_preview`）が続いても、実行中の作り直しを取り消して作り直し
/// 続けない（ルーペ・先読みが同じ写真を何度も要求するため）。
#[test]
fn repeated_preview_requests_do_not_restart_regeneration() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let env = Env::new();
    common::write_jpeg(&env.photos.join("A.jpg"), (120, 80), 1, None);
    let starts = Arc::new(AtomicUsize::new(0));
    let (release_tx, release_rx) = crossbeam_channel::bounded::<()>(0);
    let core = {
        let starts = Arc::clone(&starts);
        env.open_with_hooks(move |h| {
            h.on_regenerate_start = Some(Arc::new(move |_| {
                // 1 回目は、テストが要求を続けて送り終えるまで止める（実行中の作り直しにする）。
                if starts.fetch_add(1, Ordering::SeqCst) == 0 {
                    let _ = release_rx.recv_timeout(Duration::from_secs(30));
                }
            }));
        })
    };
    common::import(&core, &env.photos);
    core.wait_idle();
    let v = all_items(&core)[0].variant_id;
    assert!(core.preview_path(v).unwrap().is_none());

    assert!(core.request_preview(v).unwrap().is_none());
    wait_until("作り直しが始まる", || {
        starts.load(Ordering::SeqCst) == 1
    });
    for _ in 0..10 {
        assert!(core.request_preview(v).unwrap().is_none());
    }
    release_tx.send(()).unwrap();
    wait_until("L1 ができる", || {
        core.preview_path(v).unwrap().is_some()
    });
    core.wait_idle();
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "実行中の作り直しを置き換えない"
    );
    // できた後の要求は、作り直さずにファイルを返す。
    assert!(core.request_preview(v).unwrap().is_some());
    core.wait_idle();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    core.close().unwrap();
}

/// No.31: 動画の variant には現像設定を保存しない（一括適用・Undo / Redo はエラーで、何も保存しない）。
#[test]
fn develop_settings_cannot_be_saved_for_videos() {
    let env = Env::new();
    let path = env.catalog_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let mut cat = genzo_catalog::Catalog::open(&path).unwrap();
        let volume = cat
            .ensure_volume("path:/genzo-test-none", None, Some("/genzo-test-none"))
            .unwrap();
        let folder = cat.ensure_folder(volume, "clips").unwrap();
        let jst = FixedOffset::east_opt(9 * 3600).unwrap();
        let when = CaptureTime::resolve_lossy(Some("2024:05:01 10:00:00"), None, jst);
        cat.register_file(&RegisterFile::video(
            folder,
            "CLIP.MP4",
            facts(1),
            VideoMetadata {
                width: Some(96),
                height: Some(64),
                ..Default::default()
            },
            when.clone(),
        ))
        .unwrap();
        cat.register_file(&RegisterFile::photo(
            folder,
            "PHOTO.JPG",
            facts(2),
            PhotoMetadata::default(),
            when,
        ))
        .unwrap();
        cat.close().unwrap();
    }
    let core = env.open();
    let items = all_items(&core);
    let video = by_name(&items, "CLIP.MP4").variant_id;
    let photo = by_name(&items, "PHOTO.JPG").variant_id;
    let edited = DevelopSettings {
        exposure_ev: 1.0,
        ..Default::default()
    };
    // 写真と動画をまとめて適用しようとすると、エラーで、写真にも保存しない。
    let err = core
        .paste_settings(&edited, &[photo, video], SettingGroups::ALL)
        .unwrap_err();
    assert!(matches!(err, ApiError::InvalidArgument(_)), "{err:?}");
    assert!(err.to_string().contains("動画"), "{err}");
    assert_eq!(core.develop_settings(photo).unwrap().exposure_ev, 0.0);
    assert_eq!(core.develop_settings(video).unwrap().exposure_ev, 0.0);
    assert!(core.history(video).unwrap().len() <= 1);
    // 動画の Undo / Redo もエラー。
    assert!(matches!(
        core.undo(video),
        Err(ApiError::InvalidArgument(_))
    ));
    assert!(matches!(
        core.redo(video),
        Err(ApiError::InvalidArgument(_))
    ));
    // 動画は現像のために開けない（今までどおり）。写真だけなら保存できる。
    assert!(matches!(
        core.open_develop(video),
        Err(ApiError::InvalidArgument(_))
    ));
    assert_eq!(
        core.paste_settings(&edited, &[photo], SettingGroups::ALL)
            .unwrap(),
        1
    );
    assert_eq!(core.develop_settings(photo).unwrap().exposure_ev, 1.0);
    core.close().unwrap();
}

/// 範囲の要求（C の座標）。
fn region(v: VariantId, x: f64, y: f64, w: f64, h: f64, scale: f64) -> RegionRequest {
    RegionRequest {
        variant_id: v,
        space: RegionSpace::Output,
        x,
        y,
        width: w,
        height: h,
        scale,
    }
}

/// 8bit の RGB の画像から範囲を切り出す。
fn crop_rgb8(src: &[u8], width: u32, (x, y, w, h): (u32, u32, u32, u32)) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for row in y..y + h {
        let start = ((row * width + x) * 3) as usize;
        out.extend_from_slice(&src[start..start + (w * 3) as usize]);
    }
    out
}

/// No.52: コア API で表示範囲（C または G の座標と倍率）を描く。部分の範囲は、全体を描いた結果の同じ
/// 範囲と一致する。縮小・拡大・範囲の切り詰め・不正な要求・写真を開いていない場合。
#[test]
fn regions_are_rendered_through_the_core_api() {
    let env = Env::new();
    common::write_jpeg(&env.photos.join("A.jpg"), (300, 200), 3, None);
    let core = env.open();
    common::import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    // 写真を開いていなければ描けない。
    assert!(matches!(
        core.render_region(&region(v, 0.0, 0.0, 10.0, 10.0, 1.0)),
        Err(ApiError::NoDevelopSession)
    ));
    core.open_develop(v).unwrap();
    let mut s = core.develop_settings(v).unwrap();
    s.exposure_ev = 0.3;
    core.set_settings(&s, false).unwrap();

    // 全体（等倍）。
    let full = core
        .render_region(&region(v, 0.0, 0.0, 300.0, 200.0, 1.0))
        .unwrap();
    assert_eq!((full.info.full_width, full.info.full_height), (300, 200));
    assert_eq!((full.info.width, full.info.height), (300, 200));
    assert_eq!(full.display_rgb8.len(), 300 * 200 * 3);
    // 部分の範囲は、全体の同じ範囲と一致する（タイルの境界・余白に左右されない）。
    let part = core
        .render_region(&region(v, 37.0, 21.0, 64.0, 48.0, 1.0))
        .unwrap();
    assert_eq!(
        (part.info.x, part.info.y, part.info.width, part.info.height),
        (37, 21, 64, 48)
    );
    assert!(part.info.request > full.info.request);
    assert_eq!(
        *part.display_rgb8,
        crop_rgb8(&full.display_rgb8, 300, (37, 21, 64, 48))
    );
    // 拡大（2 倍）は最近傍: 2 × 2 の画素が等倍の 1 画素と同じ。
    let zoom = core
        .render_region(&region(v, 37.0, 21.0, 64.0, 48.0, 2.0))
        .unwrap();
    assert_eq!((zoom.info.width, zoom.info.height), (128, 96));
    for (x, y) in [(0u32, 0u32), (5, 7), (63, 47)] {
        let p = |img: &[u8], w: u32, x: u32, y: u32| {
            let i = ((y * w + x) * 3) as usize;
            img[i..i + 3].to_vec()
        };
        let one = p(&part.display_rgb8, 64, x, y);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert_eq!(p(&zoom.display_rgb8, 128, 2 * x + dx, 2 * y + dy), one);
        }
    }
    // 縮小（半分）。
    let half = core
        .render_region(&region(v, 0.0, 0.0, 300.0, 200.0, 0.5))
        .unwrap();
    assert_eq!((half.info.width, half.info.height), (150, 100));
    assert_eq!(half.display_rgb8.len(), 150 * 100 * 3);
    // G の座標（長辺を 1 とした値）: 切り抜き・回転がなければ、全体は C の全体。
    let g = core
        .render_region(&RegionRequest {
            space: RegionSpace::Geometry,
            ..region(v, 0.0, 0.0, 1.0, 200.0 / 300.0, 1.0)
        })
        .unwrap();
    assert_eq!(
        (
            g.info.x,
            g.info.y,
            g.info.region_width,
            g.info.region_height
        ),
        (0, 0, 300, 200)
    );
    assert_eq!(*g.display_rgb8, *full.display_rgb8);
    // 画像の外にはみ出した範囲は内側に収める。重ならなければエラー。
    let clipped = core
        .render_region(&region(v, -10.0, 190.0, 50.0, 50.0, 1.0))
        .unwrap();
    assert_eq!(
        (
            clipped.info.x,
            clipped.info.y,
            clipped.info.width,
            clipped.info.height
        ),
        (0, 190, 40, 10)
    );
    for bad in [
        region(v, 400.0, 0.0, 10.0, 10.0, 1.0),
        region(v, 0.0, 0.0, 0.0, 10.0, 1.0),
        region(v, 0.0, 0.0, 10.0, 10.0, 0.0),
        region(v, 0.0, 0.0, 10.0, 10.0, f64::NAN),
    ] {
        assert!(
            matches!(core.render_region(&bad), Err(ApiError::InvalidArgument(_))),
            "{bad:?}"
        );
    }
    // 最後に描けた結果を取れる。JSON のコマンドでも描ける（応答は情報だけ）。
    assert_eq!(
        core.region_frame().unwrap().info.request,
        clipped.info.request
    );
    let out = core.execute_json(&format!(
        r#"{{"type":"render_region","request":{{"variant_id":{},"x":0,"y":0,"width":8,"height":4}}}}"#,
        v.get()
    ));
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["ok"]["type"], "region", "{out}");
    assert_eq!(json["ok"]["width"], 8, "{out}");
    assert_eq!(json["ok"]["scale"], 1.0, "{out}");
    // 取り消すものがなければ false。
    assert!(!core.cancel_region().unwrap());
    core.close().unwrap();
}

/// No.52: P0 の「最新の 1 件だけ」: 続けて要求すると、古い要求は取り消され（または先に終わり）、最後に
/// 描けた結果は最新の要求のもの。取り消された呼び出しは Cancelled で戻る。
#[test]
fn only_the_latest_region_request_is_rendered() {
    let env = Env::new();
    common::write_jpeg(&env.photos.join("B.jpg"), (1600, 1200), 5, None);
    let core = std::sync::Arc::new(env.open());
    common::import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    let mut handles = Vec::new();
    for i in 0..4 {
        let core = std::sync::Arc::clone(&core);
        handles.push(std::thread::spawn(move || {
            core.render_region(&region(v, 0.0, 0.0, 1600.0, 1200.0 - f64::from(i), 1.0))
        }));
        std::thread::sleep(Duration::from_millis(5));
    }
    let last = core
        .render_region(&region(v, 10.0, 10.0, 16.0, 16.0, 1.0))
        .expect("最新の要求は描ける");
    let mut cancelled = 0;
    for h in handles {
        match h.join().unwrap() {
            Ok(f) => assert!(f.info.request < last.info.request),
            Err(e) => {
                assert!(e.is_cancelled(), "{e:?}");
                cancelled += 1;
            }
        }
    }
    eprintln!("取り消された要求: {cancelled} / 4");
    assert_eq!(core.region_frame().unwrap().info.request, last.info.request);
    assert_eq!((last.info.width, last.info.height), (16, 16));
    // cancel_region の後も、次の要求は描ける。
    core.cancel_region().unwrap();
    assert!(
        core.render_region(&region(v, 0.0, 0.0, 4.0, 4.0, 1.0))
            .is_ok()
    );
    core.close().unwrap();
}
