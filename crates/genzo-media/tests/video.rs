//! ffprobe / ffmpeg を使う動画のテスト（VID-02・VID-03、PoC-7）。
//!
//! ffmpeg・ffprobe（環境変数 GENZO_FFPROBE / GENZO_FFMPEG、なければ PATH）がない環境、または
//! テスト用の動画を作れない（libx264 がない）環境では、何もせずに成功する（スキップ）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use genzo_color::transfer::{srgb_decode, srgb_encode};
use genzo_media::{
    CacheSpec, FfmpegTools, MediaError, SourceProfile, VideoColorHandling, cache_icc_profile,
    decode_image_bytes,
};

fn tools() -> Option<FfmpegTools> {
    let t = FfmpegTools::from_env();
    if t.is_available() {
        Some(t)
    } else {
        eprintln!("ffmpeg / ffprobe がないため、動画のテストをスキップする");
        None
    }
}

/// ffmpeg でテスト用の動画を作る。作れなければ `None`（スキップ）。
fn make_video(t: &FfmpegTools, path: &Path, args: &[&str]) -> Option<PathBuf> {
    let status = Command::new(&t.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args(args)
        .arg(path)
        .stdin(Stdio::null())
        .status()
        .ok()?;
    if status.success() && path.exists() {
        Some(path.to_path_buf())
    } else {
        eprintln!("テスト用の動画を作れないため、スキップする: {args:?}");
        None
    }
}

#[test]
fn h264_metadata_and_thumbnail() {
    let Some(t) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let Some(video) = make_video(
        &t,
        &dir.path().join("testsrc.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=3:size=320x240:rate=25",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-metadata",
            "creation_time=2024-05-01T12:34:56Z",
        ],
    ) else {
        return;
    };

    // 元の動画の内容（DATA-01: 読み取りだけで、書き換えない）。
    let original = std::fs::read(&video).unwrap();

    let p = t.probe(&video).unwrap();
    let m = &p.metadata;
    assert!((m.duration_s.unwrap() - 3.0).abs() < 0.1, "{m:?}");
    assert!((m.fps.unwrap() - 25.0).abs() < 1e-9, "{m:?}");
    assert_eq!(m.codec.as_deref(), Some("h264"));
    assert_eq!(m.bit_depth, Some(8));
    assert_eq!((m.width, m.height), (Some(320), Some(240)));
    assert_eq!(p.rotation_deg, 0);
    assert!(
        m.creation_time
            .as_deref()
            .is_some_and(|s| s.starts_with("2024-05-01T12:34:56")),
        "{m:?}"
    );

    let spec = CacheSpec {
        long_edge: 160,
        quality: 80,
    };
    let thumb = t.thumbnail(&video, spec).unwrap();
    assert_eq!((thumb.jpeg.width, thumb.jpeg.height), (160, 120));
    // 3 秒の動画は長さの 10%（0.3 秒）の位置。
    assert_eq!(thumb.position_ms, 300);
    // 320×240 で色の記録がないので SD とみなす（BT.601 の行列。原色は変換しない。指摘 F29）。
    assert_eq!(thumb.color, VideoColorHandling::PrimariesNotConverted);
    let d = decode_image_bytes(&thumb.jpeg.bytes).unwrap();
    assert_eq!(
        d.profile,
        SourceProfile::Embedded(cache_icc_profile().unwrap())
    );
    // testsrc は色の帯などを含むので、一様な画像ではない。
    let rgb = d.pixels.to_rgb8();
    let first = rgb.pixel(0, 0).unwrap();
    assert!(rgb.as_raw().chunks_exact(3).any(|p| p != first));
    // ffprobe・ffmpeg を実行した後も、元の動画は変わらない。
    assert_eq!(std::fs::read(&video).unwrap(), original);
}

/// sRGB の 8bit の色を、Display P3 の 8bit にした値。
fn srgb8_to_p3(c: [f64; 3]) -> [f64; 3] {
    use genzo_color::space::{SRGB_TO_XYZ, XYZ_TO_DISPLAY_P3};
    let lin = c.map(|v| srgb_decode(v / 255.0));
    XYZ_TO_DISPLAY_P3
        .apply(SRGB_TO_XYZ.apply(lin))
        .map(|v| srgb_encode(v.clamp(0.0, 1.0)) * 255.0)
}

/// サムネイルの中央の色（8bit）。
fn thumbnail_center(t: &FfmpegTools, video: &Path) -> [u8; 3] {
    let thumb = t
        .thumbnail(
            video,
            CacheSpec {
                long_edge: 64,
                quality: 100,
            },
        )
        .unwrap();
    // 1 秒の動画は先頭のフレーム。
    assert_eq!(thumb.position_ms, 0);
    let d = decode_image_bytes(&thumb.jpeg.bytes).unwrap();
    let rgb = d.pixels.to_rgb8();
    rgb.pixel(rgb.width() / 2, rgb.height() / 2).unwrap()
}

#[test]
fn thumbnail_colors_use_bt709_matrix() {
    // BT.709 の行列で YUV にした単色の HD の動画。行列を記録したもの・しないものの両方で、元の色
    // （sRGB）を P3 にした値に近いこと（記録がないときに ffmpeg の既定の BT.601 で戻すと、
    // R が 10 段階以上ずれる）。記録がない SD の動画は BT.601 で戻すので（指摘 F29。
    // `untagged_sd_video_uses_bt601_matrix`）、ここでは HD の寸法にする。
    let Some(t) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let color = [192.0, 64.0, 32.0];
    let common = [
        "-f",
        "lavfi",
        "-i",
        "color=c=0xC04020:s=1280x720:d=1:r=10",
        "-vf",
        "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
    ];
    let tagged_args: Vec<&str> = common
        .iter()
        .copied()
        .chain([
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
        ])
        .collect();
    for (name, args) in [
        ("tagged.mp4", tagged_args),
        ("untagged.mp4", common.to_vec()),
    ] {
        let Some(video) = make_video(&t, &dir.path().join(name), &args) else {
            return;
        };
        let p = t.probe(&video).unwrap();
        assert_eq!(p.color_space.is_some(), name == "tagged.mp4", "{p:?}");
        assert_eq!(p.coded_size, Some((1280, 720)));
        let px = thumbnail_center(&t, &video);
        let expected = srgb8_to_p3(color);
        for k in 0..3 {
            // 4:2:0・制限範囲の 8bit・JPEG の丸めの分を見込む。
            assert!(
                (f64::from(px[k]) - expected[k]).abs() <= 6.0,
                "{name}: {px:?} vs {expected:?}"
            );
        }
    }
}

#[test]
fn untagged_sd_video_uses_bt601_matrix() {
    // 色の記録がない SD（640×480）の動画を BT.601・制限範囲で符号化したもの（古いカメラ・携帯電話の
    // H.264 など）。慣習（mpv などのプレイヤー）どおり BT.601 で戻し、元の色に近いこと。
    // 再現（指摘 F29）: 修正前は BT.709 で戻し、(200,60,60) が sRGB で約 (209,72,54)
    // （ΔE2000 で約 5.4）になった。縦に撮った動画（回転の記録付き）も、符号化された寸法で決める。
    let Some(t) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let color = [200.0, 60.0, 60.0];
    let args = [
        "-f",
        "lavfi",
        "-i",
        "color=c=0xC83C3C:s=640x480:d=1:r=10",
        "-vf",
        "scale=out_color_matrix=bt601:out_range=tv,format=yuv420p",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
    ];
    let Some(video) = make_video(&t, &dir.path().join("sd.mp4"), &args) else {
        return;
    };
    let src_str = video.to_str().unwrap().to_owned();
    let Some(rotated) = make_video(
        &t,
        &dir.path().join("sd_rotated.mp4"),
        &["-display_rotation:v:0", "90", "-i", &src_str, "-c", "copy"],
    ) else {
        return;
    };
    let expected = srgb8_to_p3(color);
    for v in [&video, &rotated] {
        let p = t.probe(v).unwrap();
        assert_eq!(p.color_space, None, "{p:?}");
        assert_eq!(p.coded_size, Some((640, 480)));
        let px = thumbnail_center(&t, v);
        for k in 0..3 {
            assert!(
                (f64::from(px[k]) - expected[k]).abs() <= 6.0,
                "{}: {px:?} vs {expected:?}",
                v.display()
            );
        }
        let thumb = t.thumbnail(v, CacheSpec::L0_THUMBNAIL).unwrap();
        // 原色（SMPTE-C / EBU）は変換していない。
        assert_eq!(thumb.color, VideoColorHandling::PrimariesNotConverted);
    }
}

#[test]
fn rotated_video_dimensions_and_thumbnail() {
    let Some(t) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let Some(src) = make_video(
        &t,
        &dir.path().join("src.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=320x240:rate=10",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ],
    ) else {
        return;
    };
    let src_str = src.to_str().unwrap().to_owned();
    // 回転の記録（Display Matrix）を付ける（-display_rotation は ffmpeg 6 以降）。
    let Some(rotated) = make_video(
        &t,
        &dir.path().join("rotated.mp4"),
        &["-display_rotation:v:0", "90", "-i", &src_str, "-c", "copy"],
    ) else {
        return;
    };
    let p = t.probe(&rotated).unwrap();
    assert_eq!(p.rotation_deg % 180, 90, "{p:?}");
    assert_eq!(p.coded_size, Some((320, 240)));
    assert_eq!(
        (p.metadata.width, p.metadata.height),
        (Some(240), Some(320))
    );
    let thumb = t.thumbnail(&rotated, CacheSpec::L0_THUMBNAIL).unwrap();
    assert_eq!((thumb.jpeg.width, thumb.jpeg.height), (240, 320));
}

#[test]
fn ten_bit_video_reports_bit_depth() {
    let Some(t) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let Some(video) = make_video(
        &t,
        &dir.path().join("ten.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=64x48:rate=10",
            "-pix_fmt",
            "yuv420p10le",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ],
    ) else {
        return;
    };
    let p = t.probe(&video).unwrap();
    assert_eq!(p.metadata.bit_depth, Some(10));
    let thumb = t.thumbnail(&video, CacheSpec::L0_THUMBNAIL).unwrap();
    assert_eq!((thumb.jpeg.width, thumb.jpeg.height), (64, 48));
}

#[test]
fn broken_and_missing_files_are_errors() {
    let Some(t) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("broken.mp4");
    std::fs::write(&bad, b"this is not a video at all").unwrap();
    assert!(matches!(
        t.probe(&bad),
        Err(MediaError::ToolFailed { .. } | MediaError::VideoProbe { .. })
    ));
    assert!(t.thumbnail(&bad, CacheSpec::L0_THUMBNAIL).is_err());
    assert!(t.probe(&dir.path().join("missing.mp4")).is_err());
    // ファイル名が '-' で始まっても、オプションとして解釈されない。
    let dash = dir.path().join("-version.mp4");
    std::fs::write(&dash, b"x").unwrap();
    assert!(t.probe(&dash).is_err());
}

#[test]
fn timeout_is_enforced() {
    let Some(mut t) = tools() else { return };
    let dir = tempfile::tempdir().unwrap();
    let Some(video) = make_video(
        &t,
        &dir.path().join("t.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=64x48:rate=10",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ],
    ) else {
        return;
    };
    t.probe_timeout = Duration::from_millis(1);
    assert!(matches!(
        t.probe(&video),
        Err(MediaError::ToolTimeout { .. })
    ));
}

/// 再生用のファイル（VID-04）を作る準備: エンコーダーを選ぶ（なければスキップ）。
fn proxy_encoder(t: &FfmpegTools) -> Option<genzo_media::ProxyEncoder> {
    let names = t.encoders().ok()?;
    let e = genzo_media::pick_proxy_encoder(&names);
    if e.is_none() {
        eprintln!("H.264 のエンコーダーがないため、再生用のファイルのテストをスキップする");
    }
    e
}

/// 再生用のファイル: 大きな動画（MPEG-4 Part 2・音声付き。WebView で再生できない形式の代わり）を、長辺 1280 の
/// H.264 ＋ AAC の MP4 にする。元の動画は変わらない。
#[test]
fn proxy_is_h264_aac_within_the_long_edge() {
    let Some(t) = tools() else { return };
    let Some(encoder) = proxy_encoder(&t) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let Some(video) = make_video(
        &t,
        &dir.path().join("big.mov"),
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=1920x1080:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "mpeg4",
            "-c:a",
            "pcm_s16le",
            "-shortest",
        ],
    ) else {
        return;
    };
    let original = std::fs::read(&video).unwrap();
    let probe = t.probe(&video).unwrap();
    let out = dir.path().join(".proxy.tmp");
    let mut seen = Vec::new();
    t.transcode_proxy(&video, &probe, &out, encoder, 2, &|| false, &mut |s| {
        seen.push(s)
    })
    .unwrap();
    let p = t.probe(&out).unwrap();
    assert_eq!(p.metadata.codec.as_deref(), Some("h264"));
    assert_eq!(
        (p.metadata.width, p.metadata.height),
        (Some(1280), Some(720))
    );
    assert_eq!(p.metadata.bit_depth, Some(8));
    assert_eq!(p.color_space.as_deref(), Some("bt709"), "{p:?}");
    genzo_media::verify_proxy(&p, probe.metadata.duration_s).unwrap();
    // 進捗を知らせた（変換した位置は 0 秒以上で、増えていく）。
    assert!(!seen.is_empty());
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
    // 音声も入っている（AAC）。
    let streams = Command::new(&t.ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "csv=p=0",
        ])
        .arg(&out)
        .output()
        .unwrap();
    let names = String::from_utf8_lossy(&streams.stdout);
    assert!(names.lines().any(|l| l.trim() == "aac"), "{names}");
    assert_eq!(std::fs::read(&video).unwrap(), original);
}

/// 再生用のファイル: 回転の記録のある縦の動画は、表示上の向きで作る（小さい動画は拡大しない）。
#[test]
fn proxy_of_a_rotated_small_video_keeps_its_size() {
    let Some(t) = tools() else { return };
    let Some(encoder) = proxy_encoder(&t) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let Some(src) = make_video(
        &t,
        &dir.path().join("src.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=320x240:rate=10",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "mpeg4",
        ],
    ) else {
        return;
    };
    let src_str = src.to_str().unwrap().to_owned();
    let Some(rotated) = make_video(
        &t,
        &dir.path().join("rotated.mp4"),
        &["-display_rotation:v:0", "90", "-i", &src_str, "-c", "copy"],
    ) else {
        return;
    };
    let probe = t.probe(&rotated).unwrap();
    let out = dir.path().join("proxy.bin");
    t.transcode_proxy(&rotated, &probe, &out, encoder, 1, &|| false, &mut |_| {})
        .unwrap();
    let p = t.probe(&out).unwrap();
    assert_eq!(
        (p.metadata.width, p.metadata.height),
        (Some(240), Some(320))
    );
    assert_eq!(p.rotation_deg, 0, "{p:?}");
    // 記録のない SD の動画は BT.601 とみなして記録する（サムネイルと同じ解釈）。
    assert_eq!(p.color_space.as_deref(), Some("smpte170m"), "{p:?}");
}

/// 再生用のファイル: 取り消すと ffmpeg を止め、書きかけのファイルを残さない。壊れた入力はエラー。
#[test]
fn proxy_cancel_and_broken_input() {
    let Some(t) = tools() else { return };
    let Some(encoder) = proxy_encoder(&t) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let Some(video) = make_video(
        &t,
        &dir.path().join("long.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=60:size=640x360:rate=30",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "mpeg4",
            "-q:v",
            "10",
        ],
    ) else {
        return;
    };
    let probe = t.probe(&video).unwrap();
    let out = dir.path().join(".cancel.tmp");
    let start = std::time::Instant::now();
    let r = t.transcode_proxy(
        &video,
        &probe,
        &out,
        encoder,
        1,
        &|| start.elapsed() > Duration::from_millis(200),
        &mut |_| {},
    );
    assert!(matches!(r, Err(MediaError::Cancelled)), "{r:?}");
    assert!(!out.exists());

    let bad = dir.path().join("broken.mp4");
    std::fs::write(&bad, b"this is not a video at all").unwrap();
    let out = dir.path().join(".bad.tmp");
    let r = t.transcode_proxy(&bad, &probe, &out, encoder, 1, &|| false, &mut |_| {});
    assert!(matches!(r, Err(MediaError::ToolFailed { .. })), "{r:?}");
    assert!(!out.exists());
}

/// 動画の最初のフレームを、記録された色の範囲のとおりに RGB へ戻したときの最小・最大の値（WebView が表示する値の
/// 代わり）。
fn decoded_rgb_extremes(t: &FfmpegTools, path: &Path) -> (u8, u8) {
    let out = Command::new(&t.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(!out.stdout.is_empty());
    let min = *out.stdout.iter().min().unwrap();
    let max = *out.stdout.iter().max().unwrap();
    (min, max)
}

/// 再現（検証 R1）: 全域（フルレンジ。`yuvj420p`・`color_range=pc`）の動画の再生用のファイルで、黒が浮き白が
/// 下がっていた（縮小のフィルターが値を制限の範囲へ変換するのに、出力には全域と記録していた）。黒は黒・白は白の
/// まま（RGB に戻して 0 と 255 の近く）にする。制限の範囲（`tv`）の動画も同じく黒・白が保たれる。
#[test]
fn proxy_keeps_black_and_white_of_full_and_limited_range_videos() {
    let Some(t) = tools() else { return };
    let Some(encoder) = proxy_encoder(&t) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let black_white =
        "color=c=black:s=64x64:d=1:r=10[b];color=c=white:s=64x64:d=1:r=10[w];[b][w]hstack";
    let Some(full) = make_video(
        &t,
        &dir.path().join("full.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            black_white,
            "-vf",
            "format=yuvj420p",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-color_range",
            "pc",
        ],
    ) else {
        return;
    };
    let Some(limited) = make_video(
        &t,
        &dir.path().join("limited.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            black_white,
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "mpeg4",
            "-q:v",
            "2",
        ],
    ) else {
        return;
    };
    for (name, video) in [("full", &full), ("limited", &limited)] {
        let probe = t.probe(video).unwrap();
        if name == "full" {
            // 前提: ffprobe が全域と読む動画であること。
            assert_eq!(probe.color_range.as_deref(), Some("pc"), "{probe:?}");
        }
        let (lo, hi) = decoded_rgb_extremes(&t, video);
        assert!(lo <= 3 && hi >= 252, "{name}: 元の動画 {lo}〜{hi}");
        let out = dir.path().join(format!(".{name}.tmp"));
        t.transcode_proxy(video, &probe, &out, encoder, 1, &|| false, &mut |_| {})
            .unwrap();
        let p = t.probe(&out).unwrap();
        let (lo, hi) = decoded_rgb_extremes(&t, &out);
        assert!(
            lo <= 3 && hi >= 252,
            "{name}: 再生用のファイル {lo}〜{hi}（{:?}・{:?}）",
            p.pix_fmt,
            p.color_range
        );
        // 出力は制限の範囲で記録する（WebView・OS のデコーダーで扱いが最も揃っている）。
        assert_eq!(p.color_range.as_deref(), Some("tv"), "{name}: {p:?}");
    }
}

/// 再現（検証 R1）: 画素が正方形でない（SAR が 1:1 でない）動画の再生用のファイルの縦横比が変わっていた
/// （保存上の寸法で縮小してから SAR を 1:1 にしていた）。表示上の縦横比のまま、正方形の画素で作る。
#[test]
fn proxy_of_non_square_pixels_keeps_the_display_aspect_ratio() {
    let Some(t) = tools() else { return };
    let Some(encoder) = proxy_encoder(&t) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    // 360×270 で SAR 4:3 → 表示上は 480×270（16:9）。
    let Some(video) = make_video(
        &t,
        &dir.path().join("anamorphic.mp4"),
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=360x270:rate=10,setsar=4/3",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "mpeg4",
        ],
    ) else {
        return;
    };
    let probe = t.probe(&video).unwrap();
    let out = dir.path().join(".anamorphic.tmp");
    t.transcode_proxy(&video, &probe, &out, encoder, 1, &|| false, &mut |_| {})
        .unwrap();
    let p = t.probe(&out).unwrap();
    assert_eq!(
        (p.metadata.width, p.metadata.height),
        (Some(480), Some(270)),
        "{p:?}"
    );
    let sar = Command::new(&t.ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=sample_aspect_ratio",
            "-of",
            "csv=p=0",
        ])
        .arg(&out)
        .output()
        .unwrap();
    let sar = String::from_utf8_lossy(&sar.stdout).trim().to_owned();
    assert!(sar == "1:1" || sar == "N/A" || sar == "0:1", "{sar}");
}
