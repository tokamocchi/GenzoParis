//! v1 のパッケージ「video」の結合テスト: 動画の再生（VID-04）の情報・配信するファイル（カタログに登録された
//! 動画だけ）・再生用のファイルの作成（利用者の ffmpeg で H.264 ＋ AAC の MP4。再利用・取り消し・最後の要求
//! だけ）・キャッシュの上限と回収・元ファイルを変えないこと、JSON のコマンドの形。
//!
//! ffmpeg・ffprobe（と H.264 のエンコーダー）がない環境では、動画の部分を飛ばす（失敗にしない）。

mod common;

use std::path::Path;
use std::process::{Command as Process, Stdio};

use common::{Env, all_items, by_name, hashes, import, make_video, wait_done, wait_ok, write_jpeg};
use genzo_api::{
    ApiError, Command, Core, ErrorKind, JobResult, JobState, MIN_VIDEO_PROXY_CACHE_BYTES, Response,
    SettingsUpdate, VIDEO_PROXY_DIR,
};
use genzo_media::FfmpegTools;

/// 再生用のファイルを作れる環境か（ffmpeg・ffprobe と H.264 のエンコーダー）。
fn proxy_tools() -> Option<FfmpegTools> {
    let t = FfmpegTools::from_env();
    if !t.is_available() {
        eprintln!("ffmpeg / ffprobe がないため、動画の部分を飛ばす");
        return None;
    }
    let names = t.encoders().ok()?;
    if genzo_media::pick_proxy_encoder(&names).is_none() {
        eprintln!("H.264 のエンコーダーがないため、再生用のファイルの部分を飛ばす");
        return None;
    }
    Some(t)
}

/// ffmpeg で長めの動画（変換に時間のかかるもの）を作る。
fn make_long_video(t: &FfmpegTools, path: &Path, seconds: u32) -> bool {
    Process::new(&t.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=duration={seconds}:size=640x360:rate=30"),
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "mpeg4",
            "-q:v",
            "10",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn proxy_dir(env: &Env) -> std::path::PathBuf {
    env.dir.path().join("data").join(VIDEO_PROXY_DIR)
}

/// 再生用のファイルのフォルダの、公開した再生用のファイルと一時ファイルの数。
fn proxy_files(env: &Env) -> (usize, usize) {
    let mut done = 0;
    let mut temp = 0;
    for e in std::fs::read_dir(proxy_dir(env)).unwrap() {
        let name = e.unwrap().file_name().to_string_lossy().into_owned();
        if name.ends_with(".mp4") {
            done += 1;
        } else if name.ends_with(".tmp") {
            temp += 1;
        }
    }
    (done, temp)
}

/// 再生の情報・配信するファイル・再生用のファイルの作成と再利用・消去。元の動画は変わらない。
#[test]
fn playback_info_stream_and_proxy() {
    let Some(t) = proxy_tools() else { return };
    let env = Env::new();
    if !make_video(&env.photos.join("clip.mp4"), "2024-05-01T03:00:00Z") {
        return;
    }
    write_jpeg(&env.photos.join("photo.jpg"), (64, 48), 1, None);
    let before = hashes(&env.photos);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let video = by_name(&items, "clip.mp4").variant_id;
    let photo = by_name(&items, "photo.jpg").variant_id;

    // 再生の情報: 元のファイルを読める・MIME の型・再生用のファイルはまだない・作れる。
    let p = core.video_playback(video).unwrap();
    assert!(p.available, "{p:?}");
    assert_eq!(p.content_type, "video/mp4");
    assert_eq!(
        p.size,
        Some(
            std::fs::metadata(env.photos.join("clip.mp4"))
                .unwrap()
                .len()
        )
    );
    assert!(
        p.video
            .as_ref()
            .is_some_and(|v| v.codec.as_deref() == Some("mpeg4")),
        "{p:?}"
    );
    assert_eq!(p.proxy, None);
    assert_eq!(p.proxy_job, None);
    assert_eq!(p.proxy_unavailable, None);
    assert!(p.proxy_encoder.is_some());

    // 配信するファイル: カタログに登録された元のファイル。再生用のファイルはまだない。写真は配信しない。
    let s = core.video_stream(video, false).unwrap();
    assert_eq!(s.path, env.photos.join("clip.mp4"));
    assert_eq!(s.content_type, "video/mp4");
    assert!(!s.proxy);
    assert_eq!(
        core.video_stream(video, true).unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert_eq!(
        core.video_stream(photo, false).unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert!(matches!(
        core.video_playback(photo),
        Err(ApiError::InvalidArgument(_))
    ));
    assert!(matches!(
        core.create_video_proxy(photo),
        Err(ApiError::InvalidArgument(_))
    ));

    // 再生用のファイルを作る。
    let job = core.create_video_proxy(video).unwrap();
    let r = match wait_ok(&core, job) {
        JobResult::VideoProxy(r) => r,
        other => panic!("{other:?}"),
    };
    assert_eq!(r.variant_id, video);
    assert!(!r.reused);
    assert!(r.encoder.is_some());
    assert!(r.size > 0);
    let p = core.video_playback(video).unwrap();
    assert_eq!(p.proxy.as_ref().map(|x| x.size), Some(r.size));
    assert_eq!(p.proxy_job, None);
    let s = core.video_stream(video, true).unwrap();
    assert!(s.proxy);
    assert_eq!(s.content_type, "video/mp4");
    assert!(s.path.starts_with(proxy_dir(&env)), "{s:?}");
    let probe = t.probe(&s.path).unwrap();
    assert_eq!(probe.metadata.codec.as_deref(), Some("h264"));
    assert_eq!(proxy_files(&env), (1, 0));

    // もう一度頼むと、作らずに今のものを使う。
    let job = core.create_video_proxy(video).unwrap();
    match wait_ok(&core, job) {
        JobResult::VideoProxy(r) => assert!(r.reused, "{r:?}"),
        other => panic!("{other:?}"),
    }
    let st = core.video_proxy_cache().unwrap();
    assert_eq!((st.count, st.total_bytes), (1, r.size));

    // 上限の設定: 最小値より小さいものは受け付けない。最小値は受け付ける（入っているものは収まる）。
    let err = core
        .update_settings(&SettingsUpdate {
            video_proxy_cache_bytes: Some(MIN_VIDEO_PROXY_CACHE_BYTES - 1),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    let s = core
        .update_settings(&SettingsUpdate {
            video_proxy_cache_bytes: Some(MIN_VIDEO_PROXY_CACHE_BYTES),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(s.video_proxy_cache_bytes, MIN_VIDEO_PROXY_CACHE_BYTES);
    assert_eq!(
        core.video_proxy_cache().unwrap().capacity_bytes,
        MIN_VIDEO_PROXY_CACHE_BYTES
    );

    // 消去（作り直せる）。
    let st = core.clear_video_proxies().unwrap();
    assert_eq!((st.removed, st.count), (1, 0));
    assert_eq!(
        core.video_stream(video, true).unwrap_err().kind(),
        ErrorKind::NotFound
    );
    core.close().unwrap();

    // 上限の設定は次に開いたときも残る。元の動画・写真は変わらない（DATA-01）。
    let core = env.open();
    assert_eq!(
        core.settings().video_proxy_cache_bytes,
        MIN_VIDEO_PROXY_CACHE_BYTES
    );
    core.close().unwrap();
    assert_eq!(hashes(&env.photos), before);
}

/// キャッシュの回収: 今のキーでない再生用のファイル（カタログにないファイル・古い内容のもの）を消し、今のものは残す。
/// 見つからない動画は「読めない」と返す。
#[test]
fn garbage_collection_and_missing_video() {
    if proxy_tools().is_none() {
        return;
    }
    let env = Env::new();
    if !make_video(&env.photos.join("a.mov"), "2024-05-01T03:00:00Z") {
        return;
    }
    let core = env.open();
    import(&core, &env.photos);
    let video = by_name(&all_items(&core), "a.mov").variant_id;
    assert_eq!(
        core.video_playback(video).unwrap().content_type,
        "video/quicktime"
    );
    wait_ok(&core, core.create_video_proxy(video).unwrap());
    let keep = core.video_stream(video, true).unwrap().path;
    // カタログにないファイルのもの・同じファイルの古い内容のもの（キーが違う）を置く。
    let name = keep.file_name().unwrap().to_string_lossy().into_owned();
    let (id, _) = name.split_once('-').unwrap();
    let orphan = proxy_dir(&env).join(format!("999999-{}.mp4", "ab".repeat(32)));
    let stale = proxy_dir(&env).join(format!("{id}-{}.mp4", "cd".repeat(32)));
    std::fs::copy(&keep, &orphan).unwrap();
    std::fs::copy(&keep, &stale).unwrap();
    let job = core.collect_cache_garbage().unwrap();
    match wait_ok(&core, job) {
        JobResult::CollectGarbage {
            video_proxies_removed,
            ..
        } => assert_eq!(video_proxies_removed, 2),
        other => panic!("{other:?}"),
    }
    assert!(keep.exists() && !orphan.exists() && !stale.exists());

    // 元の動画がなくなると、読めないと返し、配信もしない。
    std::fs::remove_file(env.photos.join("a.mov")).unwrap();
    let p = core.video_playback(video).unwrap();
    assert!(!p.available);
    assert!(p.unavailable_reason.is_some());
    assert_eq!(
        core.video_stream(video, false).unwrap_err().kind(),
        ErrorKind::NotFound
    );
    core.close().unwrap();
}

/// 取り消すと ffmpeg を止めて一時ファイルを残さない。別の動画を頼むと、作っている途中のものを取り消す（最後の
/// 要求だけ）。同じ動画を作っている途中に頼むと、同じジョブを返す。
#[test]
fn proxy_cancel_and_latest_request_wins() {
    let Some(t) = proxy_tools() else { return };
    let env = Env::new();
    // 長さを変えて、内容の違う動画にする（同じ内容は重複として取り込まれない。IMP-04）。
    for (name, seconds) in [("long1.mp4", 120), ("long2.mp4", 121)] {
        if !make_long_video(&t, &env.photos.join(name), seconds) {
            eprintln!("テスト用の動画を作れないため、飛ばす");
            return;
        }
    }
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "long1.mp4").variant_id;
    let b = by_name(&items, "long2.mp4").variant_id;

    let job_a = core.create_video_proxy(a).unwrap();
    assert_eq!(core.create_video_proxy(a).unwrap(), job_a);
    assert_eq!(core.video_playback(a).unwrap().proxy_job, Some(job_a));
    let job_b = core.create_video_proxy(b).unwrap();
    assert_ne!(job_a, job_b);
    assert_eq!(wait_done(&core, job_a).state, JobState::Cancelled);
    assert_eq!(core.video_playback(a).unwrap().proxy_job, None);
    core.cancel_job(job_b).unwrap();
    assert_eq!(wait_done(&core, job_b).state, JobState::Cancelled);
    assert_eq!(proxy_files(&env), (0, 0));
    core.close().unwrap();
}

/// JSON のコマンドの形（UI の core_execute）。
#[test]
fn json_commands() {
    let c: Command = serde_json::from_value(serde_json::json!({
        "type": "video_playback",
        "variant_id": 3
    }))
    .unwrap();
    assert_eq!(
        c,
        Command::VideoPlayback {
            variant_id: genzo_api::VariantId::new(3)
        }
    );
    for t in ["video_proxy_cache", "clear_video_proxies"] {
        let c: Command = serde_json::from_value(serde_json::json!({ "type": t })).unwrap();
        assert!(matches!(
            c,
            Command::VideoProxyCache | Command::ClearVideoProxies
        ));
    }
    let env = Env::new();
    let core: Core = env.open();
    match core.execute(Command::VideoProxyCache).unwrap() {
        Response::VideoProxyCache(st) => {
            assert_eq!(st.count, 0);
            let v = serde_json::to_value(Response::VideoProxyCache(st)).unwrap();
            assert_eq!(v["type"], "video_proxy_cache");
            assert!(v["dir"].as_str().unwrap().ends_with(VIDEO_PROXY_DIR));
        }
        other => panic!("{other:?}"),
    }
    core.close().unwrap();
}
