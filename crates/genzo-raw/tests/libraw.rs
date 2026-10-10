//! LibRaw の FFI のテスト（機能フラグ `libraw`）。
//!
//! 合成 DNG（[`genzo_raw::dng`]）を書いて LibRaw で展開し、寸法・データ・CFA・黒レベル・
//! 白レベル・行列・WB・撮影情報が一致することを確かめる。壊れた入力でエラーを返し、
//! パニックしないことも確かめる。実機の ARW での確認は PoC-2 で行う。
#![cfg(feature = "libraw")]

use std::path::{Path, PathBuf};

use genzo_model::{CaptureInfo, GpsCoord, Orientation, PhotoMetadata};
use genzo_raw::dng::{
    self, DngMargins, DngOptions, DngPreview, ILLUMINANT_D65, ILLUMINANT_STANDARD_A,
};
use genzo_raw::{
    CamXyzSource, CaptureTimeSource, CfaPattern, RawError, RawImage, ThumbnailFormat, WbSource,
    decode_bytes, decode_file, decode_file_with_details, decoder_id, extract_thumbnail,
    extract_thumbnail_bytes, libraw_version, read_metadata, read_metadata_bytes,
};

/// 決まった種から作る疑似乱数（xorshift32）。
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

const CAM_XYZ: [[f32; 3]; 3] = [
    [0.7424, -0.2329, -0.0466],
    [-0.4598, 1.2471, 0.2347],
    [-0.0715, 0.1505, 0.6066],
];

fn metadata() -> PhotoMetadata {
    PhotoMetadata {
        make: Some("GenzoTest".to_owned()),
        model: Some("Synth-1".to_owned()),
        lens: Some("Test 50mm F1.8".to_owned()),
        iso: Some(400),
        aperture: Some(2.8),
        shutter_s: Some(1.0 / 250.0),
        focal_mm: Some(50.0),
        orientation: Orientation::Normal,
        gps: GpsCoord::new(35.681236, 139.767125),
        capture: CaptureInfo {
            datetime: Some("2024:05:01 12:34:56.123".to_owned()),
            offset: Some("+09:00".to_owned()),
        },
        ..Default::default()
    }
}

/// 合成の RAW（黒レベルは CFA の位置ごとに違う値）。
fn sample(cfa: CfaPattern, width: u32, height: u32, seed: u32) -> RawImage {
    let mut rng = Rng(seed);
    let data = (0..width * height)
        .map(|_| 500 + (rng.next() % 15_000) as u16)
        .collect();
    let mut metadata = metadata();
    metadata.width = Some(width);
    metadata.height = Some(height);
    RawImage {
        width,
        height,
        cfa,
        data,
        black_level: [510.0, 512.0, 514.0, 516.0],
        white_level: 16383.0,
        as_shot_wb: [2.25, 1.0, 1.625, 1.0],
        cam_xyz: Some(CAM_XYZ),
        metadata,
    }
}

fn write(dir: &Path, name: &str, img: &RawImage, opts: &DngOptions) -> PathBuf {
    let path = dir.join(name);
    dng::write_file(&path, img, opts).unwrap();
    path
}

fn assert_close(a: f32, b: f32, tol: f32, what: &str) {
    assert!(
        (a - b).abs() <= tol * b.abs().max(1.0),
        "{what}: {a} と {b} の差が大きい"
    );
}

#[test]
fn version_and_decoder_id() {
    let v = libraw_version().unwrap();
    assert!(v.starts_with("0.21."), "{v}");
    let id = decoder_id().unwrap();
    assert!(id.starts_with("libraw-0.21."), "{id}");
    const { assert!(genzo_raw::LIBRAW_ENABLED) };
}

#[test]
fn round_trip_all_cfa_patterns() {
    let dir = tempfile::tempdir().unwrap();
    for (i, cfa) in CfaPattern::ALL.into_iter().enumerate() {
        let img = sample(cfa, 64, 48, 11 + i as u32);
        let path = write(
            dir.path(),
            &format!("{cfa}.dng"),
            &img,
            &DngOptions::default(),
        );
        let d = decode_file_with_details(&path).unwrap();
        let out = &d.image;
        assert_eq!((out.width, out.height), (64, 48), "{cfa}");
        assert_eq!(out.cfa, cfa);
        assert!(out.data == img.data, "{cfa}: 画素の値が一致しない");
        assert_eq!(out.black_level, img.black_level, "{cfa}");
        assert_eq!(out.white_level, img.white_level, "{cfa}");
        for c in 0..4 {
            assert_close(out.as_shot_wb[c], img.as_shot_wb[c], 1e-6, "WB");
        }
        let m = out.cam_xyz.expect("D65 の ColorMatrix1");
        for r in 0..3 {
            for c in 0..3 {
                assert_close(m[r][c], CAM_XYZ[r][c], 1e-5, "cam_xyz");
            }
        }
        assert_eq!(
            d.details.cam_xyz_source,
            CamXyzSource::DngColorMatrix {
                index: 1,
                illuminant: 21
            }
        );
        assert_eq!(d.details.wb_source, WbSource::AsShot);
        assert!(d.details.is_dng);
        assert_eq!((d.details.raw_width, d.details.raw_height), (64, 48));
        assert_eq!((d.details.left_margin, d.details.top_margin), (0, 0));
        assert_eq!(d.details.raw_pitch_bytes, 128);
        assert_eq!(d.details.data_error_count, 0);
        assert_eq!(d.details.decoder_id, decoder_id().unwrap());
        assert!(!d.details.unpack_function.is_empty());
        assert_eq!(d.details.dng_color_matrices.len(), 1);
        // validate 済み。
        out.validate().unwrap();
    }
}

#[test]
fn metadata_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let img = sample(CfaPattern::RGGB, 32, 24, 3);
    let path = write(dir.path(), "meta.dng", &img, &DngOptions::default());
    let d = decode_file_with_details(&path).unwrap();
    let m = &d.image.metadata;
    assert_eq!(m.make.as_deref(), Some("GenzoTest"));
    assert_eq!(m.model.as_deref(), Some("Synth-1"));
    assert_eq!(m.lens.as_deref(), Some("Test 50mm F1.8"));
    assert_eq!(m.iso, Some(400));
    assert_close(m.aperture.unwrap(), 2.8, 1e-6, "F 値");
    assert_close(m.shutter_s.unwrap(), 1.0 / 250.0, 1e-6, "シャッター速度");
    assert_close(m.focal_mm.unwrap(), 50.0, 1e-6, "焦点距離");
    assert_eq!((m.width, m.height), (Some(32), Some(24)));
    assert_eq!(m.orientation, Orientation::Normal);
    let gps = m.gps.unwrap();
    assert!((gps.lat - 35.681236).abs() < 1e-5, "{gps:?}");
    assert!((gps.lon - 139.767125).abs() < 1e-5, "{gps:?}");
    assert_eq!(
        m.capture.datetime.as_deref(),
        Some("2024:05:01 12:34:56.123")
    );
    assert_eq!(m.capture.offset.as_deref(), Some("+09:00"));
    assert_eq!(
        d.details.capture_time_source,
        CaptureTimeSource::ExifDateTimeOriginal
    );
    assert!(d.details.timestamp.is_some());
    // 展開しない読み取りでも同じ値。
    assert_eq!(&read_metadata(&path).unwrap(), m);
}

#[test]
fn all_orientations_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    for exif in 1..=8u16 {
        let mut img = sample(CfaPattern::GRBG, 32, 24, 5);
        img.metadata.orientation = Orientation::from_exif(exif).unwrap();
        let path = write(
            dir.path(),
            &format!("o{exif}.dng"),
            &img,
            &DngOptions::default(),
        );
        let m = read_metadata(&path).unwrap();
        assert_eq!(m.orientation.to_exif(), exif as u8);
    }
}

#[test]
fn even_margins_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let img = sample(CfaPattern::BGGR, 40, 30, 7);
    let opts = DngOptions {
        margins: DngMargins {
            left: 4,
            top: 2,
            right: 6,
            bottom: 8,
        },
        margin_value: 1,
        ..Default::default()
    };
    let path = write(dir.path(), "margins.dng", &img, &opts);
    let d = decode_file_with_details(&path).unwrap();
    assert_eq!((d.image.width, d.image.height), (40, 30));
    assert_eq!(d.image.cfa, CfaPattern::BGGR);
    assert!(d.image.data == img.data);
    assert_eq!(d.image.black_level, img.black_level);
    assert_eq!((d.details.raw_width, d.details.raw_height), (50, 40));
    assert_eq!((d.details.left_margin, d.details.top_margin), (4, 2));
}

/// 奇数の余白（ActiveArea）の DNG では、LibRaw は余白を偶数にそろえ、有効画素を 1 画素ずつ
/// 狭め、CFA の配列をずらす（LibRaw 0.21 の `open_datastream`）。CFA の値と配列は、ずれた
/// 原点に合わせて一致する。
#[test]
fn odd_margins_shift_the_origin() {
    let dir = tempfile::tempdir().unwrap();
    let mut img = sample(CfaPattern::RGGB, 40, 30, 9);
    // 色ごとに違う黒レベル（位置のパターンとしても、色としても同じ）。
    img.black_level = [510.0, 512.0, 514.0, 512.0];
    let opts = DngOptions {
        margins: DngMargins {
            left: 3,
            top: 1,
            right: 1,
            bottom: 1,
        },
        ..Default::default()
    };
    let path = write(dir.path(), "odd.dng", &img, &opts);
    let d = decode_file_with_details(&path).unwrap();
    let out = &d.image;
    assert_eq!((out.width, out.height), (39, 29));
    assert_eq!((d.details.left_margin, d.details.top_margin), (4, 2));
    assert_eq!(out.cfa, CfaPattern::RGGB.shifted(1, 1));
    for y in 0..29 {
        for x in 0..39 {
            assert_eq!(out.get(x, y), img.get(x + 1, y + 1), "({x}, {y})");
        }
    }
    // LibRaw は余白をずらしても黒レベルの繰り返しのパターン（cblack[6..]）をずらさない。
    // この crate は LibRaw と同じ添字（有効画素の座標）でパターンを読むため、奇数の余白の DNG では
    // パターンの位置が 1 画素ずれる（LibRaw 自身の処理と同じ結果）。ここでは、その現在の動作を
    // 記録する（LibRaw が直した場合はこのテストが失敗するので、そのときに見直す）。
    for (x, y) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
        assert_eq!(
            out.black_level_at(x, y),
            img.black_level_at(x, y),
            "({x}, {y})"
        );
    }
}

#[test]
fn second_matrix_with_d65_is_selected() {
    let dir = tempfile::tempdir().unwrap();
    let mut img = sample(CfaPattern::RGGB, 32, 32, 13);
    let std_a = [[0.9, -0.3, -0.1], [-0.5, 1.4, 0.1], [-0.1, 0.2, 0.7]];
    img.cam_xyz = Some(std_a);
    let opts = DngOptions {
        illuminant_1: ILLUMINANT_STANDARD_A,
        color_matrix_2: Some((CAM_XYZ, ILLUMINANT_D65)),
        ..Default::default()
    };
    let path = write(dir.path(), "two.dng", &img, &opts);
    let d = decode_file_with_details(&path).unwrap();
    let m = d.image.cam_xyz.unwrap();
    for r in 0..3 {
        for c in 0..3 {
            assert_close(m[r][c], CAM_XYZ[r][c], 1e-5, "cam_xyz");
        }
    }
    assert_eq!(
        d.details.cam_xyz_source,
        CamXyzSource::DngColorMatrix {
            index: 2,
            illuminant: 21
        }
    );
    assert_eq!(d.details.dng_color_matrices.len(), 2);
    assert_eq!(d.details.dng_color_matrices[0].illuminant, 17);
}

#[test]
fn non_d65_or_missing_matrix_gives_none() {
    let dir = tempfile::tempdir().unwrap();
    let img = sample(CfaPattern::RGGB, 32, 32, 17);
    let opts = DngOptions {
        illuminant_1: ILLUMINANT_STANDARD_A,
        ..Default::default()
    };
    let path = write(dir.path(), "stda.dng", &img, &opts);
    let d = decode_file_with_details(&path).unwrap();
    assert_eq!(d.image.cam_xyz, None);
    assert_eq!(d.details.cam_xyz_source, CamXyzSource::None);

    let mut img = sample(CfaPattern::RGGB, 32, 32, 19);
    img.cam_xyz = None;
    let path = write(dir.path(), "nomatrix.dng", &img, &DngOptions::default());
    let d = decode_file_with_details(&path).unwrap();
    assert_eq!(d.image.cam_xyz, None);
}

#[test]
fn thumbnails() {
    let dir = tempfile::tempdir().unwrap();
    let img = sample(CfaPattern::RGGB, 64, 48, 23);

    // 8bit の RGB のプレビュー。
    let rgb: Vec<u8> = (0..16 * 12 * 3).map(|i| (i * 7 % 251) as u8).collect();
    let opts = DngOptions {
        preview: Some(DngPreview::Rgb8 {
            width: 16,
            height: 12,
            data: rgb.clone(),
        }),
        ..Default::default()
    };
    let path = write(dir.path(), "rgb.dng", &img, &opts);
    let t = extract_thumbnail(&path).unwrap();
    assert_eq!(t.format, ThumbnailFormat::Rgb8);
    assert_eq!((t.width, t.height), (16, 12));
    assert_eq!(t.data, rgb);
    // プレビューがあっても RAW の展開は変わらない。
    assert!(decode_file(&path).unwrap().data == img.data);

    // JPEG のプレビュー。
    let jpeg = make_jpeg(24, 16);
    let opts = DngOptions {
        preview: Some(DngPreview::Jpeg {
            width: 24,
            height: 16,
            data: jpeg.clone(),
        }),
        ..Default::default()
    };
    let path = write(dir.path(), "jpeg.dng", &img, &opts);
    let t = extract_thumbnail(&path).unwrap();
    assert_eq!(t.format, ThumbnailFormat::Jpeg);
    assert_eq!((t.width, t.height), (24, 16));
    assert_eq!(t.data, jpeg);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(extract_thumbnail_bytes(&bytes).unwrap(), t);

    // プレビューがない。
    let path = write(dir.path(), "none.dng", &img, &DngOptions::default());
    assert!(matches!(
        extract_thumbnail(&path),
        Err(RawError::Unsupported(_))
    ));
}

fn make_jpeg(width: u32, height: u32) -> Vec<u8> {
    let rgb: Vec<u8> = (0..width * height * 3).map(|i| (i % 256) as u8).collect();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .unwrap();
    out
}

#[test]
fn bytes_api_matches_file_api() {
    let dir = tempfile::tempdir().unwrap();
    let img = sample(CfaPattern::GBRG, 32, 32, 29);
    let path = write(dir.path(), "bytes.dng", &img, &DngOptions::default());
    let bytes = std::fs::read(&path).unwrap();
    let a = decode_file_with_details(&path).unwrap();
    let b = decode_bytes(&bytes).unwrap();
    assert_eq!(a, b);
    assert_eq!(
        read_metadata(&path).unwrap(),
        read_metadata_bytes(&bytes).unwrap()
    );
}

#[test]
fn non_ascii_path_and_read_only_file() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("写真 テスト");
    std::fs::create_dir(&sub).unwrap();
    let img = sample(CfaPattern::RGGB, 32, 32, 31);
    let path = write(&sub, "画像_01 ①.dng", &img, &DngOptions::default());
    // 読み取り専用のファイルでも開ける（DATA-01: 書き込みの権限を要求しない）。
    let mut perm = std::fs::metadata(&path).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&path, perm).unwrap();
    let before = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert!(decode_file(&path).unwrap().data == img.data);
    read_metadata(&path).unwrap();
    let _ = extract_thumbnail(&path);
    // 元ファイルの内容と更新日時が変わらない（DATA-01）。
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn parallel_decoding_gives_identical_results() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<PathBuf> = CfaPattern::ALL
        .into_iter()
        .enumerate()
        .map(|(i, cfa)| {
            let img = sample(cfa, 128, 96, 100 + i as u32);
            write(
                dir.path(),
                &format!("p{i}.dng"),
                &img,
                &DngOptions::default(),
            )
        })
        .collect();
    let expected: Vec<_> = paths
        .iter()
        .map(|p| decode_file_with_details(p).unwrap())
        .collect();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let paths = &paths;
                let expected = &expected;
                s.spawn(move || {
                    for round in 0..5 {
                        let i = (t + round) % paths.len();
                        let d = decode_file_with_details(&paths[i]).unwrap();
                        assert_eq!(&d, &expected[i]);
                        let _ = read_metadata(&paths[i]).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    });
}

// ---- 壊れた入力 ----

#[test]
fn missing_path_and_directory() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.dng");
    for r in [
        decode_file(&missing).map(|_| ()),
        read_metadata(&missing).map(|_| ()),
        extract_thumbnail(&missing).map(|_| ()),
    ] {
        assert!(
            matches!(&r, Err(RawError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound),
            "{r:?}"
        );
    }
    assert!(matches!(decode_file(dir.path()), Err(RawError::Io(_))));
}

#[test]
fn empty_and_non_raw_files() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.dng");
    std::fs::write(&empty, b"").unwrap();
    assert!(decode_file(&empty).is_err());
    assert!(read_metadata(&empty).is_err());
    assert!(extract_thumbnail(&empty).is_err());
    assert!(decode_bytes(&[]).is_err());

    // RAW でないファイル（LibRaw は形式を判別できないか、読み込みのエラーを返す）。
    let text = dir.path().join("text.dng");
    std::fs::write(&text, b"this is not a raw file at all, just text....").unwrap();
    assert!(matches!(
        decode_file(&text),
        Err(RawError::Unsupported(_) | RawError::Decode(_))
    ));

    let jpeg = dir.path().join("photo.jpg");
    std::fs::write(&jpeg, make_jpeg(32, 32)).unwrap();
    assert!(decode_file(&jpeg).is_err());
}

#[test]
fn truncated_files_return_errors() {
    let dir = tempfile::tempdir().unwrap();
    let img = sample(CfaPattern::RGGB, 64, 48, 37);
    let full = dng::encode(&img, &DngOptions::default()).unwrap();
    let raw_start = full.len() - 64 * 48 * 2;
    let cuts = [
        1,
        4,
        8,
        16,
        100,
        raw_start / 2,
        raw_start,
        raw_start + 1,
        raw_start + 2 * 64 * 10,
        full.len() - 2,
        full.len() - 1,
    ];
    for cut in cuts {
        let path = dir.path().join(format!("cut{cut}.dng"));
        std::fs::write(&path, &full[..cut]).unwrap();
        let r = decode_file(&path);
        assert!(
            r.is_err(),
            "{cut} バイトで切れたファイルが展開できてしまった"
        );
        // メモリ上のデータでも同じ。ただし LibRaw 0.21 のメモリ上の読み込み
        // （LibRaw_buffer_datastream::read）は、最後の値が途中で切れていても読めた数に数えるため、
        // 最後の 1 バイトだけが欠けた場合は検出できない（decode_bytes の制限。lib の説明を参照）。
        match decode_bytes(&full[..cut]) {
            Err(_) => {}
            Ok(d) if cut == full.len() - 1 => d.image.validate().unwrap(),
            Ok(_) => panic!("{cut} バイトで切れたデータが展開できてしまった"),
        }
        // 撮影情報の読み取りはパニックしない（RAW のデータの前で切れていれば読めてよい）。
        let _ = read_metadata(&path);
        let _ = extract_thumbnail(&path);
    }
}

/// IFD0 のタグの値（4 バイト以内のもの）を書き換える。
fn patch_tag(bytes: &mut [u8], tag: u16, value: u32) {
    let ifd = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let n = u16::from_le_bytes([bytes[ifd], bytes[ifd + 1]]) as usize;
    for i in 0..n {
        let e = ifd + 2 + 12 * i;
        if u16::from_le_bytes([bytes[e], bytes[e + 1]]) == tag {
            let typ = u16::from_le_bytes([bytes[e + 2], bytes[e + 3]]);
            match typ {
                3 => bytes[e + 8..e + 10].copy_from_slice(&(value as u16).to_le_bytes()),
                4 => bytes[e + 8..e + 12].copy_from_slice(&value.to_le_bytes()),
                _ => panic!("型 {typ}"),
            }
            return;
        }
    }
    panic!("タグ {tag} がありません");
}

#[test]
fn abnormal_dimensions_return_errors() {
    let img = sample(CfaPattern::RGGB, 64, 48, 41);
    let full = dng::encode(&img, &DngOptions::default()).unwrap();
    // (幅, 高さ): 0、データより大きい、上限を超える、TIFF の上限に近い値。
    let cases = [
        (0, 48),
        (64, 0),
        (128, 48),
        (64, 4800),
        (60_000, 60_000),
        (65_535, 65_535),
        (u32::MAX, 2),
    ];
    for (w, h) in cases {
        let mut bytes = full.clone();
        patch_tag(&mut bytes, 256, w);
        patch_tag(&mut bytes, 257, h);
        let r = decode_bytes(&bytes);
        assert!(
            r.is_err(),
            "{w} × {h}: {:?}",
            r.map(|d| (d.image.width, d.image.height))
        );
        let _ = read_metadata_bytes(&bytes);
    }
    // 画素数の上限を超える寸法は、展開する前に TooManyPixels で止める。
    let mut bytes = full.clone();
    patch_tag(&mut bytes, 256, 60_000);
    patch_tag(&mut bytes, 257, 60_000);
    patch_tag(&mut bytes, 278, 60_000);
    patch_tag(&mut bytes, 279, u32::MAX);
    assert!(
        matches!(decode_bytes(&bytes), Err(RawError::TooManyPixels { .. })),
        "{:?}",
        decode_bytes(&bytes).map(|_| ())
    );
}

#[test]
fn garbage_inputs_do_not_panic() {
    let img = sample(CfaPattern::RGGB, 32, 32, 43);
    let full = dng::encode(&img, &DngOptions::default()).unwrap();
    let mut rng = Rng(0x1234_5678);
    // ヘッダと IFD の領域（RAW のデータより前）のバイトをランダムに壊す。
    let header_len = full.len() - 32 * 32 * 2;
    for _ in 0..300 {
        let mut bytes = full.clone();
        for _ in 0..4 {
            let pos = rng.next() as usize % header_len;
            bytes[pos] = rng.next() as u8;
        }
        if let Ok(d) = decode_bytes(&bytes) {
            d.image.validate().unwrap();
        }
        let _ = read_metadata_bytes(&bytes);
        let _ = extract_thumbnail_bytes(&bytes);
    }
    // 完全にランダムなデータ。
    for len in [2usize, 7, 64, 512, 4096] {
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        assert!(decode_bytes(&bytes).is_err());
    }
}

/// メーカーが SONY の DNG: 撮影情報は EXIF の元の名前、行列は DNG の ColorMatrix を優先する。
/// ColorMatrix がなければ LibRaw の内蔵の表（ILCE-7M4 の値）を使う。
#[test]
fn sony_dng_names_and_libraw_table() {
    let dir = tempfile::tempdir().unwrap();
    let mut img = sample(CfaPattern::RGGB, 64, 48, 47);
    img.metadata.make = Some("SONY".to_owned());
    img.metadata.model = Some("ILCE-7M4".to_owned());
    img.metadata.iso = Some(102_400);
    let path = write(dir.path(), "sony.dng", &img, &DngOptions::default());
    let d = decode_file_with_details(&path).unwrap();
    assert_eq!(d.image.metadata.make.as_deref(), Some("SONY"));
    assert_eq!(d.image.metadata.model.as_deref(), Some("ILCE-7M4"));
    assert_eq!(d.details.libraw_make.as_deref(), Some("Sony"));
    // ISO 65535 を超える値は RecommendedExposureIndex から読む（LibRaw は SONY と Canon だけ）。
    assert_eq!(d.image.metadata.iso, Some(102_400));
    assert_eq!(
        d.details.cam_xyz_source,
        CamXyzSource::DngColorMatrix {
            index: 1,
            illuminant: 21
        }
    );
    assert!(d.image.data == img.data);
    assert_eq!(d.image.black_level, img.black_level);

    // ColorMatrix のない DNG では LibRaw の内蔵の表を使う。値は LibRaw 0.21 の
    // src/tables/colordata.cpp の ILCE-7M4 の行（10000 倍の整数）。
    img.cam_xyz = None;
    let path = write(
        dir.path(),
        "sony_nomatrix.dng",
        &img,
        &DngOptions::default(),
    );
    let d = decode_file_with_details(&path).unwrap();
    assert_eq!(d.details.cam_xyz_source, CamXyzSource::LibRawTable);
    let table = [7460, -2365, -588, -5687, 13442, 2474, -624, 1156, 6584];
    let m = d.image.cam_xyz.unwrap();
    for (i, &t) in table.iter().enumerate() {
        assert_close(m[i / 3][i % 3], t as f32 / 10_000.0, 1e-6, "ILCE-7M4 の表");
    }
}

/// LibRaw が DNG の ColorMatrix から計算する `pre_mul`（昼光の WB）と `rgb_cam` が、
/// dcraw の `cam_xyz_coeff` の式（docs: decode モジュールの「カメラ行列」）どおりであることを、
/// sRGB（ITU-R BT.709 の原色・D65）→ XYZ の行列（LibRaw の `xyz_rgb` と同じ値）で確かめる。
#[test]
fn libraw_derived_matrices_follow_cam_xyz_coeff() {
    const XYZ_RGB: [[f64; 3]; 3] = [
        [0.4124564, 0.3575761, 0.1804375],
        [0.2126729, 0.7151522, 0.0721750],
        [0.0193339, 0.1191920, 0.9503041],
    ];
    let dir = tempfile::tempdir().unwrap();
    let img = sample(CfaPattern::RGGB, 32, 32, 53);
    let path = write(dir.path(), "coeff.dng", &img, &DngOptions::default());
    let d = decode_file_with_details(&path).unwrap();
    // cam_rgb = cam_xyz × xyz_rgb を行ごとに和 1 に正規化する。
    let mut cam_rgb = [[0.0f64; 3]; 3];
    let mut row_sum = [0.0f64; 3];
    for i in 0..3 {
        for j in 0..3 {
            cam_rgb[i][j] = (0..3)
                .map(|k| f64::from(CAM_XYZ[i][k]) * XYZ_RGB[k][j])
                .sum();
        }
        row_sum[i] = cam_rgb[i].iter().sum();
        for v in &mut cam_rgb[i] {
            *v /= row_sum[i];
        }
    }
    // pre_mul = 1 / 行の和（G を基準にした比で比べる）。
    let pre = d.details.pre_mul;
    for i in [0usize, 2] {
        let expected = row_sum[1] / row_sum[i];
        let actual = f64::from(pre[i]) / f64::from(pre[1]);
        assert!(
            (actual - expected).abs() < 1e-5,
            "pre_mul[{i}]: {actual} / {expected}"
        );
    }
    // rgb_cam は正規化した cam_rgb の逆行列。
    let rgb_cam = d.details.rgb_cam;
    let product = |i: usize, j: usize| -> f64 {
        (0..3)
            .map(|k| f64::from(rgb_cam[i][k]) * cam_rgb[k][j])
            .sum()
    };
    for i in 0..3 {
        for j in 0..3 {
            let v = product(i, j);
            let expected = if i == j { 1.0 } else { 0.0 };
            assert!((v - expected).abs() < 1e-4, "({i}, {j}): {v}");
        }
    }
}
