//! ワーカーのジョブの結合テスト（実際に `genzo-worker` のバイナリを起動する）。
//!
//! - JPEG / PNG の `ProbePhoto`・`DecodeImage`（リニア BT.2020、向きは反映しない）・`Thumbnail`
//!   （B5、向きを反映する）
//! - 共有メモリが足りない場合の確保し直し
//! - バッチ用ワーカーでの並行処理
//! - 動画の `ProbeVideo`・`VideoThumbnail`（ffmpeg があるときだけ）
//! - 合成 DNG の `DecodeRaw`・`ProbePhoto`・`Thumbnail`（機能フラグ `libraw` のとき）。無効なときは
//!   `Unsupported`

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use common::{config, contents, exif_orientation, shm_files_under, write_jpeg};
use genzo_color::space::to_working;
use genzo_color::transfer::srgb_decode;
use genzo_color::{IccProfile, RgbColorSpace, StandardProfile};
use genzo_jobs::CancellationToken;
use genzo_media::{
    CacheSpec, DynRgbImage, FfmpegTools, RgbImage16, SourceProfile, cache_icc_profile,
};
use genzo_model::{ExportFormat, ExportSettings, Orientation};
use genzo_worker::shm::SHM_HEADER_LEN;
use genzo_worker::{
    InitialShmBytes, Lane, PhotoFormat, PoolConfig, ThumbnailSource, WorkerClient,
    WorkerClientError, WorkerErrorKind, WorkerPool,
};

fn token() -> CancellationToken {
    CancellationToken::new()
}

fn assert_close(a: f32, b: f64, tol: f64, what: &str) {
    assert!(
        (f64::from(a) - b).abs() <= tol,
        "{what}: {a} と {b} の差が大きい"
    );
}

/// sRGB の 8bit の色を、リニア BT.2020 にした値（行列と伝達関数による基準）。
fn srgb8_to_bt2020(c: [u8; 3]) -> [f64; 3] {
    to_working(RgbColorSpace::Srgb).apply(c.map(|v| srgb_decode(f64::from(v) / 255.0)))
}

#[test]
fn jpeg_probe_decode_and_thumbnail() {
    let root = tempfile::tempdir().unwrap();
    let color = [200u8, 100, 50];
    // 向き 6（時計回りに 90 度回して表示）を記録した 64 × 48 の JPEG。
    let path = write_jpeg(
        root.path(),
        "rotated.jpg",
        (64, 48),
        |_, _| color,
        Some(&exif_orientation(6)),
    );
    let original = contents(&path);
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();

    let probe = c.probe_photo(&path, &token()).unwrap();
    assert_eq!(probe.format, PhotoFormat::Jpeg);
    assert_eq!(probe.metadata.orientation, Orientation::Rotate90Cw);
    assert_eq!(
        (probe.metadata.width, probe.metadata.height),
        (Some(64), Some(48))
    );
    assert!(!probe.profile.unwrap().embedded);

    // DecodeImage: 向きは反映しない（記録された寸法のまま）。
    let img = c.decode_image(&path, &token()).unwrap();
    assert_eq!((img.width(), img.height()), (64, 48));
    assert_eq!(img.pixels.len(), 64 * 48);
    assert_eq!(img.info.metadata.orientation, Orientation::Rotate90Cw);
    assert_eq!(img.info.source_bits, 8);
    assert_eq!(img.non_finite_replaced, 0);
    let expected = srgb8_to_bt2020(color);
    let px = img.pixels[24 * 64 + 32];
    for k in 0..3 {
        // JPEG（品質 100）の YCbCr の丸めと、デコーダの実装（SIMD の有無）による違いを見込む
        // （8bit で ±2 段階程度）。sRGB のまま（BT.2020 に変換しない）なら R が 0.17 ほど違う。
        assert_close(px[k], expected[k], 0.03, "BT.2020");
    }

    // Thumbnail: B5（Display P3 の ICC 付き）、向きを反映する。
    let t = c
        .thumbnail(&path, CacheSpec::L0_THUMBNAIL, &token())
        .unwrap();
    assert_eq!(t.source, ThumbnailSource::Image);
    assert_eq!((t.width, t.height), (48, 64));
    assert!(t.video.is_none());
    let decoded = genzo_media::decode_image_bytes(&t.jpeg).unwrap();
    assert_eq!(decoded.pixels.dimensions(), (48, 64));
    assert_eq!(
        decoded.profile,
        SourceProfile::Embedded(cache_icc_profile().unwrap())
    );
    // 長辺の指定に合わせて縮小する。
    let small = c
        .thumbnail(
            &path,
            CacheSpec {
                long_edge: 16,
                quality: 80,
            },
            &token(),
        )
        .unwrap();
    assert_eq!((small.width, small.height), (12, 16));

    // 元のファイルは変わらない（DATA-01）。共有メモリは残らない。
    assert_eq!(contents(&path), original);
    assert_eq!(shm_files_under(root.path()), 0);
    assert_eq!(c.stats().restarts, 0);
}

#[test]
fn png_with_embedded_display_p3_profile() {
    let root = tempfile::tempdir().unwrap();
    // Display P3 の 16bit の緑。埋め込みのプロファイルで解釈されること。
    let p3 = IccProfile::standard(StandardProfile::DisplayP3).unwrap();
    let img = RgbImage16::from_fn(5, 3, |_, _| [0, 65535, 0]).unwrap();
    let bytes = genzo_media::png_io::encode_png(&img, Some(p3.as_bytes()), None).unwrap();
    let path = root.path().join("p3.png");
    std::fs::write(&path, bytes).unwrap();
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();
    let out = c.decode_image(&path, &token()).unwrap();
    assert_eq!(out.info.format, PhotoFormat::Png);
    assert_eq!(out.info.source_bits, 16);
    assert!(out.info.profile.embedded);
    let expected = to_working(RgbColorSpace::DisplayP3).apply([0.0, 1.0, 0.0]);
    for px in &out.pixels {
        for k in 0..3 {
            assert_close(px[k], expected[k], 2e-3, "P3 の緑");
        }
    }
}

#[test]
fn tiff_is_an_image_only_with_a_tiff_extension() {
    let root = tempfile::tempdir().unwrap();
    // sRGB の 16bit の中間の灰色（書き出しの関数で ICC を埋め込む）。
    let img = DynRgbImage::Rgb16(RgbImage16::from_fn(6, 4, |_, _| [32768; 3]).unwrap());
    let settings = ExportSettings {
        format: ExportFormat::Tiff16,
        ..ExportSettings::default()
    };
    let bytes = genzo_media::encode_export(&img, &settings, None).unwrap();
    let tif = root.path().join("a.TIF");
    std::fs::write(&tif, &bytes).unwrap();
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();
    let probe = c.probe_photo(&tif, &token()).unwrap();
    assert_eq!(probe.format, PhotoFormat::Tiff);
    let out = c.decode_image(&tif, &token()).unwrap();
    assert_eq!(out.info.format, PhotoFormat::Tiff);
    assert_eq!(out.info.source_bits, 16);
    assert!(out.info.profile.embedded);
    let expected = srgb_decode(32768.0 / 65535.0);
    for px in &out.pixels {
        for v in px {
            assert_close(*v, expected, 1e-3, "灰色");
        }
    }
    // 同じ内容でも、拡張子が TIFF でなければ RAW として扱う（ARW・DNG なども TIFF の印を持つため）。
    let raw_named = root.path().join("a.arw");
    std::fs::write(&raw_named, &bytes).unwrap();
    let err = c.decode_image(&raw_named, &token()).unwrap_err();
    assert_eq!(err.worker_error_kind(), Some(&WorkerErrorKind::Unsupported));
    assert_eq!(c.stats().restarts, 0);
}

#[test]
fn small_shared_memory_is_reallocated_once_and_remembered() {
    let root = tempfile::tempdir().unwrap();
    let path = write_jpeg(
        root.path(),
        "a.jpg",
        (40, 30),
        |x, y| [(x * 6) as u8, (y * 8) as u8, 128],
        None,
    );
    let mut cfg = config(root.path());
    // 最初はヘッダだけの大きさにして、必ず確保し直しが起きるようにする。
    cfg.initial_shm = InitialShmBytes {
        raw: SHM_HEADER_LEN as u64,
        image: SHM_HEADER_LEN as u64,
        thumbnail: SHM_HEADER_LEN as u64,
    };
    let mut c = WorkerClient::spawn(cfg).unwrap();
    // 1 回目は「容量不足」と「確保し直した後」の 2 回の応答、2 回目は覚えた大きさで 1 回。
    for expected_responses in [2, 1] {
        let before = c.stats().responses;
        let img = c.decode_image(&path, &token()).unwrap();
        assert_eq!((img.width(), img.height()), (40, 30));
        let first = c.stats().responses - before;
        let before = c.stats().responses;
        let t = c
            .thumbnail(&path, CacheSpec::L0_THUMBNAIL, &token())
            .unwrap();
        assert_eq!((t.width, t.height), (40, 30));
        let first_thumb = c.stats().responses - before;
        assert_eq!(
            (first, first_thumb),
            (expected_responses, expected_responses)
        );
    }
    assert_eq!(c.stats().restarts, 0);
    assert_eq!(shm_files_under(root.path()), 0);
}

#[test]
fn missing_broken_and_unsupported_inputs_are_job_errors() {
    let root = tempfile::tempdir().unwrap();
    let broken = root.path().join("broken.png");
    std::fs::write(&broken, b"\x89PNG\r\n\x1a\nbroken").unwrap();
    let text = root.path().join("note.txt");
    std::fs::write(&text, b"hello").unwrap();
    let jpeg = write_jpeg(root.path(), "a.jpg", (8, 8), |_, _| [9; 3], None);
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();
    let pid = c.pid();
    let kind = |r: Result<(), WorkerClientError>| r.unwrap_err().worker_error_kind().cloned();

    let missing = root.path().join("missing.jpg");
    assert_eq!(
        kind(c.probe_photo(&missing, &token()).map(drop)),
        Some(WorkerErrorKind::NotFound)
    );
    assert_eq!(
        kind(c.decode_image(&broken, &token()).map(drop)),
        Some(WorkerErrorKind::Decode)
    );
    assert_eq!(
        kind(
            c.thumbnail(&broken, CacheSpec::L0_THUMBNAIL, &token())
                .map(drop)
        ),
        Some(WorkerErrorKind::Decode)
    );
    // JPEG を RAW として展開しようとした。
    assert_eq!(
        kind(c.decode_raw(&jpeg, &token()).map(drop)),
        Some(WorkerErrorKind::Unsupported)
    );
    // 画像でも RAW でもないファイルは LibRaw に渡され、対応外かデコードの失敗になる。
    let k = kind(c.decode_image(&text, &token()).map(drop));
    assert_eq!(k, Some(WorkerErrorKind::Unsupported));
    // サムネイルの寸法の指定が不正（ワーカーには送らない）。
    assert!(matches!(
        c.thumbnail(
            &jpeg,
            CacheSpec {
                long_edge: 0,
                quality: 80
            },
            &token()
        ),
        Err(WorkerClientError::InvalidArgument(_))
    ));
    // どれもワーカーは無事（再起動しない）。
    assert_eq!(c.pid(), pid);
    assert_eq!(c.stats().restarts, 0);
    assert_eq!(shm_files_under(root.path()), 0);
}

#[test]
fn batch_workers_run_jobs_in_parallel() {
    let root = tempfile::tempdir().unwrap();
    let paths: Vec<PathBuf> = (0..6)
        .map(|i| {
            write_jpeg(
                root.path(),
                &format!("{i}.jpg"),
                (96 + i * 8, 64),
                move |x, _| [(x * 2) as u8, i as u8 * 30, 90],
                None,
            )
        })
        .collect();
    let pool = WorkerPool::new(PoolConfig {
        worker: config(root.path()),
        batch_workers: 2,
    })
    .unwrap();

    // バッチ用のワーカーを 1 つ借りたままでも、もう 1 つで処理が進む（並行に動く）。
    let held = pool.acquire(Lane::Batch, &token()).unwrap();
    let held_pid = held.pid().unwrap();
    let other_pid = thread::scope(|s| {
        s.spawn(|| {
            let lease = pool.acquire(Lane::Batch, &token()).unwrap();
            let pid = lease.pid().unwrap();
            drop(lease);
            pool.thumbnail(Lane::Batch, &paths[0], CacheSpec::L0_THUMBNAIL, &token())
                .unwrap();
            pid
        })
        .join()
        .unwrap()
    });
    assert_ne!(held_pid, other_pid);
    drop(held);
    let interactive_pid = pool.ping(Lane::Interactive).unwrap().pid;
    assert!(interactive_pid != held_pid && interactive_pid != other_pid);

    // 複数のスレッドから同時に依頼しても、すべて正しく終わる。
    thread::scope(|s| {
        for t in 0..4 {
            let pool = &pool;
            let paths = &paths;
            s.spawn(move || {
                for (i, p) in paths.iter().enumerate() {
                    let lane = if (i + t) % 3 == 0 {
                        Lane::Interactive
                    } else {
                        Lane::Batch
                    };
                    let probe = pool.probe_photo(lane, p, &token()).unwrap();
                    let w = 96 + i as u32 * 8;
                    assert_eq!(probe.metadata.width, Some(w));
                    let thumb = pool
                        .thumbnail(lane, p, CacheSpec::L0_THUMBNAIL, &token())
                        .unwrap();
                    assert_eq!((thumb.width, thumb.height), (w, 64));
                    let img = pool.decode_image(lane, p, &token()).unwrap();
                    assert_eq!(img.pixels.len() as u32, w * 64);
                }
            });
        }
    });
    assert_eq!(shm_files_under(root.path()), 0);
    pool.shutdown().unwrap();
}

/// ffmpeg でテスト用の動画を作る。作れなければ `None`（スキップ）。
fn make_video(tools: &FfmpegTools, path: &Path) -> Option<PathBuf> {
    for codec in [
        ["-c:v", "libx264", "-pix_fmt", "yuv420p"],
        ["-c:v", "mpeg4", "-pix_fmt", "yuv420p"],
    ] {
        let status = Command::new(&tools.ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=160x120:rate=10",
            ])
            .args(codec)
            .arg(path)
            .stdin(Stdio::null())
            .status()
            .ok()?;
        if status.success() && path.is_file() {
            return Some(path.to_path_buf());
        }
    }
    None
}

#[test]
fn video_probe_and_thumbnail() {
    let tools = FfmpegTools::from_env();
    if !tools.is_available() {
        eprintln!("ffmpeg / ffprobe がないため、動画のテストをスキップする");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let Some(video) = make_video(&tools, &root.path().join("clip.mp4")) else {
        eprintln!("テスト用の動画を作れないため、スキップする");
        return;
    };
    let original = contents(&video);
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();
    let probe = c.probe_video(&video, &token()).unwrap();
    assert_eq!(
        (probe.metadata.width, probe.metadata.height),
        (Some(160), Some(120))
    );
    assert!((probe.metadata.duration_s.unwrap() - 2.0).abs() < 0.2);
    let t = c
        .video_thumbnail(&video, CacheSpec::L0_THUMBNAIL, &token())
        .unwrap();
    assert_eq!(t.source, ThumbnailSource::VideoFrame);
    assert_eq!((t.width, t.height), (160, 120));
    assert!(t.video.is_some());
    assert!(t.jpeg.starts_with(&[0xFF, 0xD8, 0xFF]));
    // 動画でないファイルは ffprobe のエラー（ワーカーは無事）。
    let jpeg = write_jpeg(root.path(), "a.jpg", (8, 8), |_, _| [0; 3], None);
    let not_video = root.path().join("not-video.mp4");
    std::fs::write(&not_video, b"not a video at all").unwrap();
    let err = c.probe_video(&not_video, &token()).unwrap_err();
    assert!(err.worker_error_kind().is_some(), "{err:?}");
    assert!(!err.counts_toward_skip());
    let _ = jpeg;
    assert_eq!(contents(&video), original);
    assert_eq!(c.stats().restarts, 0);
    assert_eq!(shm_files_under(root.path()), 0);
}

/// 合成 DNG（genzo-raw の `dng`。機能フラグに関係なく書ける）。
mod dng {
    use super::*;
    use genzo_model::PhotoMetadata;
    use genzo_raw::dng::{DngOptions, DngPreview, write_file};
    use genzo_raw::{CfaPattern, RawImage};

    pub const W: u32 = 64;
    pub const H: u32 = 48;

    pub fn image(orientation: Orientation) -> RawImage {
        let data = (0..W * H)
            .map(|i| 600 + ((i * 7919) % 15_000) as u16)
            .collect();
        RawImage {
            width: W,
            height: H,
            cfa: CfaPattern::RGGB,
            data,
            black_level: [512.0; 4],
            white_level: 16383.0,
            as_shot_wb: [2.25, 1.0, 1.625, 1.0],
            cam_xyz: Some([
                [0.7424, -0.2329, -0.0466],
                [-0.4598, 1.2471, 0.2347],
                [-0.0715, 0.1505, 0.6066],
            ]),
            metadata: PhotoMetadata {
                make: Some("GenzoTest".to_owned()),
                model: Some("Synth-1".to_owned()),
                iso: Some(400),
                orientation,
                width: Some(W),
                height: Some(H),
                ..Default::default()
            },
        }
    }

    /// 埋め込みの JPEG（左半分が赤、右半分が青の 64 × 48）付きの DNG を書く。
    pub fn write(dir: &Path, name: &str, img: &RawImage) -> PathBuf {
        let preview =
            genzo_media::RgbImage8::from_fn(
                64,
                48,
                |x, _| {
                    if x < 32 { [255, 0, 0] } else { [0, 0, 255] }
                },
            )
            .unwrap();
        let jpeg = genzo_media::jpeg::encode_jpeg(&preview, 95, None, None).unwrap();
        let path = dir.join(name);
        write_file(
            &path,
            img,
            &DngOptions {
                preview: Some(DngPreview::Jpeg {
                    width: 64,
                    height: 48,
                    data: jpeg,
                }),
                ..DngOptions::default()
            },
        )
        .unwrap();
        path
    }
}

#[cfg(not(feature = "libraw"))]
#[test]
fn raw_jobs_are_unsupported_without_libraw() {
    let root = tempfile::tempdir().unwrap();
    let path = dng::write(root.path(), "a.dng", &dng::image(Orientation::Normal));
    let mut c = WorkerClient::spawn(config(root.path())).unwrap();
    assert_eq!(c.worker_info().unwrap().raw_decoder, None);
    let kind = |r: Result<(), WorkerClientError>| r.unwrap_err().worker_error_kind().cloned();
    assert_eq!(
        kind(c.decode_raw(&path, &token()).map(drop)),
        Some(WorkerErrorKind::Unsupported)
    );
    assert_eq!(
        kind(c.probe_photo(&path, &token()).map(drop)),
        Some(WorkerErrorKind::Unsupported)
    );
    assert_eq!(
        kind(
            c.thumbnail(&path, CacheSpec::L0_THUMBNAIL, &token())
                .map(drop)
        ),
        Some(WorkerErrorKind::Unsupported)
    );
    assert_eq!(c.stats().restarts, 0);
    assert!(!c.failures().is_skipped(&path));
}

#[cfg(feature = "libraw")]
mod libraw {
    use super::*;

    #[test]
    fn dng_decode_probe_and_thumbnail() {
        let root = tempfile::tempdir().unwrap();
        let src = dng::image(Orientation::Rotate90Cw);
        let path = dng::write(root.path(), "synth.dng", &src);
        let original = contents(&path);
        let pool = WorkerPool::new(PoolConfig {
            worker: config(root.path()),
            batch_workers: 1,
        })
        .unwrap();

        let frame = pool.decode_raw(Lane::Interactive, &path, &token()).unwrap();
        assert_eq!(frame.decoder_id, genzo_raw::decoder_id());
        let img = &frame.image;
        assert_eq!((img.width, img.height), (dng::W, dng::H));
        assert_eq!(img.cfa, src.cfa);
        assert_eq!(img.data, src.data, "CFA の値がそのまま届く");
        assert_eq!(img.black_level, src.black_level);
        assert_eq!(img.white_level, src.white_level);
        for (a, b) in img.as_shot_wb.iter().zip(src.as_shot_wb) {
            assert!((a - b).abs() < 1e-3, "{:?}", img.as_shot_wb);
        }
        assert!(img.cam_xyz.is_some());
        assert_eq!(img.metadata.orientation, Orientation::Rotate90Cw);
        img.validate().unwrap();

        let probe = pool.probe_photo(Lane::Batch, &path, &token()).unwrap();
        assert_eq!(probe.format, PhotoFormat::Raw);
        assert_eq!(probe.metadata.make.as_deref(), Some("GenzoTest"));
        assert_eq!(probe.metadata.iso, Some(400));
        assert!(probe.profile.is_none());

        // 埋め込みの JPEG から、RAW の向き（90 度）を反映した B5 を作る。
        let t = pool
            .thumbnail(Lane::Batch, &path, CacheSpec::L0_THUMBNAIL, &token())
            .unwrap();
        assert_eq!(t.source, ThumbnailSource::EmbeddedJpeg);
        assert_eq!((t.width, t.height), (48, 64));
        let d = genzo_media::decode_image_bytes(&t.jpeg)
            .unwrap()
            .pixels
            .to_rgb8();
        // 時計回りに 90 度回すと、左半分の赤は上半分になる。
        let top = d.pixel(24, 8).unwrap();
        let bottom = d.pixel(24, 56).unwrap();
        assert!(top[0] > 180 && top[2] < 80, "{top:?}");
        assert!(bottom[2] > 180 && bottom[0] < 80, "{bottom:?}");

        // RAW を DecodeImage に渡すのは誤り。
        let err = pool.decode_image(Lane::Batch, &path, &token()).unwrap_err();
        assert_eq!(err.worker_error_kind(), Some(&WorkerErrorKind::Unsupported));

        assert_eq!(contents(&path), original, "DATA-01");
        assert_eq!(shm_files_under(root.path()), 0);
        pool.shutdown().unwrap();
    }

    #[test]
    fn raw_shared_memory_is_reallocated_when_too_small() {
        let root = tempfile::tempdir().unwrap();
        let src = dng::image(Orientation::Normal);
        let path = dng::write(root.path(), "a.dng", &src);
        let mut cfg = config(root.path());
        cfg.initial_shm.raw = SHM_HEADER_LEN as u64 + 100;
        let mut c = WorkerClient::spawn(cfg).unwrap();
        let before = c.stats().responses;
        let frame = c.decode_raw(&path, &token()).unwrap();
        assert_eq!(frame.image.data, src.data);
        assert_eq!(c.stats().responses - before, 2);
        let before = c.stats().responses;
        c.decode_raw(&path, &token()).unwrap();
        assert_eq!(c.stats().responses - before, 1, "覚えた大きさで 1 回");
        assert_eq!(shm_files_under(root.path()), 0);
    }

    #[test]
    fn truncated_dng_fails_without_taking_down_the_host() {
        let root = tempfile::tempdir().unwrap();
        let full = dng::write(root.path(), "full.dng", &dng::image(Orientation::Normal));
        let bytes = contents(&full);
        let mut c = WorkerClient::spawn(config(root.path())).unwrap();
        for (i, cut) in [bytes.len() / 2, bytes.len() - 100, 600, 16]
            .into_iter()
            .enumerate()
        {
            let path = root.path().join(format!("cut-{i}.dng"));
            std::fs::write(&path, &bytes[..cut]).unwrap();
            let result = c.decode_raw(&path, &token());
            // 結果を記録する（PoC-2 の「壊れたファイル」の確認の参考）。
            eprintln!("{cut} バイトで切った DNG: {result:?}");
            match result {
                // LibRaw がエラーを返す（ワーカーは無事）か、異常終了・ハングして再起動される。
                // どれになるかは LibRaw の版（CI の macOS は Homebrew の版）で変わりうる。
                Err(WorkerClientError::Job(e)) => assert!(
                    matches!(
                        e.kind,
                        WorkerErrorKind::Decode
                            | WorkerErrorKind::Unsupported
                            | WorkerErrorKind::Io
                    ),
                    "{cut}: {e:?}"
                ),
                Err(WorkerClientError::Crashed { .. } | WorkerClientError::Timeout { .. }) => {}
                other => panic!("{cut}: {other:?}"),
            }
            // どちらでも、次のジョブは成功する。
            c.ping().unwrap();
            let frame = c.decode_raw(&full, &token()).unwrap();
            assert_eq!(frame.image.data.len(), (dng::W * dng::H) as usize);
        }
        assert_eq!(shm_files_under(root.path()), 0);
    }
}
