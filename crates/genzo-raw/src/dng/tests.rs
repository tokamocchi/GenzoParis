//! 合成 DNG のテスト。この crate と独立した読み取り（kamadak-exif の TIFF の解析）で確かめる。

use exif::{Context, In, Reader, Tag, Value as ExifValue};
use genzo_model::{CaptureInfo, Orientation};

use super::*;
use crate::CfaPattern;

fn sample_image(cfa: CfaPattern) -> RawImage {
    let (width, height) = (6u32, 4u32);
    let data = (0..width * height)
        .map(|i| 600 + (i * 37 % 1000) as u16)
        .collect();
    RawImage {
        width,
        height,
        cfa,
        data,
        black_level: [512.0, 513.0, 514.0, 515.0],
        white_level: 16383.0,
        as_shot_wb: [2.0, 1.0, 1.6, 1.0],
        cam_xyz: Some([[0.7, -0.2, -0.05], [-0.45, 1.25, 0.2], [-0.07, 0.15, 0.6]]),
        metadata: PhotoMetadata {
            make: Some("GenzoTest".to_owned()),
            model: Some("Synth-1".to_owned()),
            lens: Some("Test 50mm F1.8".to_owned()),
            iso: Some(400),
            aperture: Some(2.8),
            shutter_s: Some(1.0 / 250.0),
            focal_mm: Some(50.0),
            orientation: Orientation::Rotate90Cw,
            gps: GpsCoord::new(35.681236, -139.767125),
            capture: CaptureInfo {
                datetime: Some("2024:05:01 12:34:56.123".to_owned()),
                offset: Some("+09:00".to_owned()),
            },
            ..Default::default()
        },
    }
}

fn read(bytes: &[u8]) -> exif::Exif {
    Reader::new()
        .read_raw(bytes.to_vec())
        .expect("TIFF として読める")
}

fn field(e: &exif::Exif, ctx: Context, num: u16) -> ExifValue {
    e.get_field(Tag(ctx, num), In::PRIMARY)
        .unwrap_or_else(|| panic!("タグ {num} がありません"))
        .value
        .clone()
}

fn uint(e: &exif::Exif, num: u16) -> u32 {
    e.get_field(Tag(Context::Tiff, num), In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .unwrap_or_else(|| panic!("タグ {num} がありません"))
}

fn uints(e: &exif::Exif, ctx: Context, num: u16) -> Vec<u32> {
    let v = field(e, ctx, num);
    v.iter_uint().expect("整数のタグ").collect()
}

fn rationals(e: &exif::Exif, ctx: Context, num: u16) -> Vec<f64> {
    match field(e, ctx, num) {
        ExifValue::Rational(v) => v.iter().map(|r| r.to_f64()).collect(),
        ExifValue::SRational(v) => v.iter().map(|r| r.to_f64()).collect(),
        other => panic!("RATIONAL ではありません: {other:?}"),
    }
}

fn ascii_field(e: &exif::Exif, ctx: Context, num: u16) -> String {
    match field(e, ctx, num) {
        ExifValue::Ascii(v) => String::from_utf8(v[0].clone()).unwrap(),
        other => panic!("ASCII ではありません: {other:?}"),
    }
}

/// RAW の画素を StripOffsets から読み戻す。
fn pixels(bytes: &[u8], e: &exif::Exif) -> Vec<u16> {
    let off = uint(e, tag::STRIP_OFFSETS) as usize;
    let len = uint(e, tag::STRIP_BYTE_COUNTS) as usize;
    bytes[off..off + len]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

#[test]
fn structure_of_main_ifd() {
    let img = sample_image(CfaPattern::RGGB);
    let bytes = encode(&img, &DngOptions::default()).unwrap();
    assert_eq!(&bytes[..4], b"II*\0");
    let e = read(&bytes);
    assert_eq!(uint(&e, tag::NEW_SUBFILE_TYPE), 0);
    assert_eq!(uint(&e, tag::IMAGE_WIDTH), 6);
    assert_eq!(uint(&e, tag::IMAGE_LENGTH), 4);
    assert_eq!(uint(&e, tag::BITS_PER_SAMPLE), 16);
    assert_eq!(uint(&e, tag::COMPRESSION), 1);
    assert_eq!(uint(&e, tag::PHOTOMETRIC), 32803);
    assert_eq!(uint(&e, tag::SAMPLES_PER_PIXEL), 1);
    assert_eq!(uint(&e, tag::ORIENTATION), 6);
    assert_eq!(
        uints(&e, Context::Tiff, tag::CFA_REPEAT_PATTERN_DIM),
        vec![2, 2]
    );
    assert_eq!(uints(&e, Context::Tiff, tag::CFA_PATTERN), vec![0, 1, 1, 2]);
    assert_eq!(uints(&e, Context::Tiff, tag::DNG_VERSION), vec![1, 4, 0, 0]);
    assert_eq!(
        uints(&e, Context::Tiff, tag::BLACK_LEVEL_REPEAT_DIM),
        vec![2, 2]
    );
    // RGGB: (0,0) = R、(1,0) = G1、(0,1) = G2、(1,1) = B。
    assert_eq!(
        rationals(&e, Context::Tiff, tag::BLACK_LEVEL),
        vec![512.0, 513.0, 515.0, 514.0]
    );
    assert_eq!(uint(&e, tag::WHITE_LEVEL), 16383);
    let cm = rationals(&e, Context::Tiff, tag::COLOR_MATRIX_1);
    let expected = [0.7, -0.2, -0.05, -0.45, 1.25, 0.2, -0.07, 0.15, 0.6];
    for (a, b) in cm.iter().zip(expected) {
        assert!((a - b).abs() < 1e-6, "{a} / {b}");
    }
    assert_eq!(uint(&e, tag::CALIBRATION_ILLUMINANT_1), 21);
    let neutral = rationals(&e, Context::Tiff, tag::AS_SHOT_NEUTRAL);
    assert_eq!(neutral, vec![0.5, 1.0, 0.625]);
    assert_eq!(ascii_field(&e, Context::Tiff, tag::MAKE), "GenzoTest");
    assert_eq!(ascii_field(&e, Context::Tiff, tag::MODEL), "Synth-1");
    assert_eq!(
        ascii_field(&e, Context::Tiff, tag::UNIQUE_CAMERA_MODEL),
        "GenzoTest Synth-1"
    );
    assert!(
        e.get_field(Tag(Context::Tiff, tag::ACTIVE_AREA), In::PRIMARY)
            .is_none()
    );
    assert_eq!(pixels(&bytes, &e), img.data);
}

#[test]
fn cfa_pattern_bytes_for_all_patterns() {
    let cases = [
        (CfaPattern::RGGB, [0, 1, 1, 2]),
        (CfaPattern::BGGR, [2, 1, 1, 0]),
        (CfaPattern::GRBG, [1, 0, 2, 1]),
        (CfaPattern::GBRG, [1, 2, 0, 1]),
    ];
    for (cfa, expected) in cases {
        let img = sample_image(cfa);
        let e = read(&encode(&img, &DngOptions::default()).unwrap());
        assert_eq!(
            uints(&e, Context::Tiff, tag::CFA_PATTERN),
            expected.to_vec(),
            "{cfa}"
        );
        // 黒レベルは CFA の位置（行優先）ごと。
        let black = rationals(&e, Context::Tiff, tag::BLACK_LEVEL);
        for (i, (x, y)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
            assert_eq!(black[i], f64::from(img.black_level_at(x, y)), "{cfa}");
        }
    }
}

#[test]
fn exif_and_gps_ifds() {
    let img = sample_image(CfaPattern::BGGR);
    let e = read(&encode(&img, &DngOptions::default()).unwrap());
    assert_eq!(
        rationals(&e, Context::Exif, tag::EXPOSURE_TIME),
        vec![1.0 / 250.0]
    );
    match field(&e, Context::Exif, tag::EXPOSURE_TIME) {
        ExifValue::Rational(v) => assert_eq!((v[0].num, v[0].denom), (1, 250)),
        other => panic!("{other:?}"),
    }
    assert!((rationals(&e, Context::Exif, tag::F_NUMBER)[0] - 2.8).abs() < 1e-9);
    assert_eq!(uints(&e, Context::Exif, tag::ISO_SPEED_RATINGS), vec![400]);
    assert_eq!(rationals(&e, Context::Exif, tag::FOCAL_LENGTH), vec![50.0]);
    assert_eq!(
        ascii_field(&e, Context::Exif, tag::DATE_TIME_ORIGINAL),
        "2024:05:01 12:34:56"
    );
    assert_eq!(
        ascii_field(&e, Context::Exif, tag::SUB_SEC_TIME_ORIGINAL),
        "123"
    );
    assert_eq!(
        ascii_field(&e, Context::Exif, tag::OFFSET_TIME_ORIGINAL),
        "+09:00"
    );
    assert_eq!(
        ascii_field(&e, Context::Exif, tag::LENS_MODEL),
        "Test 50mm F1.8"
    );
    // GPS: 度・分・秒から戻した値が元の座標に近い。
    let to_deg = |v: Vec<f64>| v[0] + v[1] / 60.0 + v[2] / 3600.0;
    let lat = to_deg(rationals(&e, Context::Gps, tag::GPS_LATITUDE));
    let lon = to_deg(rationals(&e, Context::Gps, tag::GPS_LONGITUDE));
    assert!((lat - 35.681236).abs() < 1e-8, "{lat}");
    assert!((lon - 139.767125).abs() < 1e-8, "{lon}");
    assert_eq!(ascii_field(&e, Context::Gps, tag::GPS_LATITUDE_REF), "N");
    assert_eq!(ascii_field(&e, Context::Gps, tag::GPS_LONGITUDE_REF), "W");
}

#[test]
fn optional_fields_are_omitted() {
    let mut img = sample_image(CfaPattern::GRBG);
    img.metadata = PhotoMetadata::default();
    img.cam_xyz = None;
    let e = read(&encode(&img, &DngOptions::default()).unwrap());
    for t in [
        tag::MAKE,
        tag::MODEL,
        tag::COLOR_MATRIX_1,
        tag::CALIBRATION_ILLUMINANT_1,
        tag::EXIF_IFD,
        tag::GPS_IFD,
        tag::SUB_IFDS,
    ] {
        assert!(
            e.get_field(Tag(Context::Tiff, t), In::PRIMARY).is_none(),
            "{t}"
        );
    }
    assert!(e.fields().all(|f| f.tag.context() == Context::Tiff));
    assert_eq!(
        ascii_field(&e, Context::Tiff, tag::UNIQUE_CAMERA_MODEL),
        "GenzoParis Synthetic"
    );
}

#[test]
fn margins_and_active_area() {
    let img = sample_image(CfaPattern::GBRG);
    let opts = DngOptions {
        margins: DngMargins {
            left: 3,
            top: 1,
            right: 2,
            bottom: 4,
        },
        margin_value: 7,
        ..Default::default()
    };
    let bytes = encode(&img, &opts).unwrap();
    let e = read(&bytes);
    assert_eq!(uint(&e, tag::IMAGE_WIDTH), 11);
    assert_eq!(uint(&e, tag::IMAGE_LENGTH), 9);
    assert_eq!(uints(&e, Context::Tiff, tag::ACTIVE_AREA), vec![1, 3, 5, 9]);
    let px = pixels(&bytes, &e);
    assert_eq!(px.len(), 11 * 9);
    for y in 0..9 {
        for x in 0..11 {
            let v = px[y * 11 + x];
            let inside = (1..5).contains(&y) && (3..9).contains(&x);
            if inside {
                assert_eq!(v, img.data[(y - 1) * 6 + (x - 3)]);
            } else {
                assert_eq!(v, 7, "({x}, {y})");
            }
        }
    }
}

#[test]
fn second_matrix_and_illuminants() {
    let img = sample_image(CfaPattern::RGGB);
    let cm2 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let opts = DngOptions {
        illuminant_1: ILLUMINANT_STANDARD_A,
        color_matrix_2: Some((cm2, ILLUMINANT_D65)),
        unique_camera_model: Some("Genzo Unique".to_owned()),
        ..Default::default()
    };
    let e = read(&encode(&img, &opts).unwrap());
    assert_eq!(uint(&e, tag::CALIBRATION_ILLUMINANT_1), 17);
    assert_eq!(uint(&e, tag::CALIBRATION_ILLUMINANT_2), 21);
    assert_eq!(
        rationals(&e, Context::Tiff, tag::COLOR_MATRIX_2),
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
    );
    assert_eq!(
        ascii_field(&e, Context::Tiff, tag::UNIQUE_CAMERA_MODEL),
        "Genzo Unique"
    );
}

/// プレビューの SubIFD を、TIFF の先頭の IFD の位置を書き換えて読む。
fn read_sub_ifd(bytes: &[u8]) -> exif::Exif {
    let e = read(bytes);
    let off = uint(&e, tag::SUB_IFDS);
    let mut patched = bytes.to_vec();
    patched[4..8].copy_from_slice(&off.to_le_bytes());
    read(&patched)
}

#[test]
fn rgb_preview_sub_ifd() {
    let img = sample_image(CfaPattern::RGGB);
    let data: Vec<u8> = (0..4 * 3 * 3).map(|i| i as u8).collect();
    let opts = DngOptions {
        preview: Some(DngPreview::Rgb8 {
            width: 4,
            height: 3,
            data: data.clone(),
        }),
        ..Default::default()
    };
    let bytes = encode(&img, &opts).unwrap();
    // 本体の画素は変わらない。
    assert_eq!(pixels(&bytes, &read(&bytes)), img.data);
    let p = read_sub_ifd(&bytes);
    assert_eq!(uint(&p, tag::NEW_SUBFILE_TYPE), 1);
    assert_eq!(uint(&p, tag::IMAGE_WIDTH), 4);
    assert_eq!(uint(&p, tag::COMPRESSION), 1);
    assert_eq!(uint(&p, tag::PHOTOMETRIC), 2);
    assert_eq!(uint(&p, tag::SAMPLES_PER_PIXEL), 3);
    let off = uint(&p, tag::STRIP_OFFSETS) as usize;
    let len = uint(&p, tag::STRIP_BYTE_COUNTS) as usize;
    assert_eq!(&bytes[off..off + len], data.as_slice());
}

#[test]
fn jpeg_preview_sub_ifd() {
    let img = sample_image(CfaPattern::RGGB);
    let jpeg = vec![0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9];
    let opts = DngOptions {
        preview: Some(DngPreview::Jpeg {
            width: 8,
            height: 8,
            data: jpeg.clone(),
        }),
        ..Default::default()
    };
    let bytes = encode(&img, &opts).unwrap();
    let p = read_sub_ifd(&bytes);
    assert_eq!(uint(&p, tag::COMPRESSION), 7);
    let off = uint(&p, tag::STRIP_OFFSETS) as usize;
    let len = uint(&p, tag::STRIP_BYTE_COUNTS) as usize;
    assert_eq!(&bytes[off..off + len], jpeg.as_slice());
    // 画素のデータは 2 バイト境界に置く。
    assert_eq!(uint(&read(&bytes), tag::STRIP_OFFSETS) % 2, 0);
}

#[test]
fn large_iso_uses_recommended_exposure_index() {
    let mut img = sample_image(CfaPattern::RGGB);
    img.metadata.iso = Some(102_400);
    let e = read(&encode(&img, &DngOptions::default()).unwrap());
    assert_eq!(
        uints(&e, Context::Exif, tag::ISO_SPEED_RATINGS),
        vec![65535]
    );
    assert_eq!(
        uints(&e, Context::Exif, tag::RECOMMENDED_EXPOSURE_INDEX),
        vec![102_400]
    );
    assert_eq!(uints(&e, Context::Exif, tag::SENSITIVITY_TYPE), vec![2]);
}

#[test]
fn invalid_inputs_are_rejected() {
    let mut img = sample_image(CfaPattern::RGGB);
    img.data.pop();
    assert!(matches!(
        encode(&img, &DngOptions::default()),
        Err(DngError::InvalidImage(RawError::DataLengthMismatch { .. }))
    ));

    let mut img = sample_image(CfaPattern::RGGB);
    img.metadata.make = Some("ソニー".to_owned());
    assert!(matches!(
        encode(&img, &DngOptions::default()),
        Err(DngError::InvalidValue(_))
    ));

    for bad in [
        "2024-05-01T12:34:56",
        "2024:05:01 12:34",
        "2024:05:01 12:34:56.",
        "2024:05:01 12:34:56.1a",
    ] {
        let mut img = sample_image(CfaPattern::RGGB);
        img.metadata.capture.datetime = Some(bad.to_owned());
        assert!(
            matches!(
                encode(&img, &DngOptions::default()),
                Err(DngError::InvalidValue(_))
            ),
            "{bad}"
        );
    }

    let img = sample_image(CfaPattern::RGGB);
    let opts = DngOptions {
        preview: Some(DngPreview::Rgb8 {
            width: 2,
            height: 2,
            data: vec![0; 11],
        }),
        ..Default::default()
    };
    assert!(matches!(
        encode(&img, &opts),
        Err(DngError::InvalidValue(_))
    ));
    let opts = DngOptions {
        preview: Some(DngPreview::Jpeg {
            width: 2,
            height: 2,
            data: vec![0; 11],
        }),
        ..Default::default()
    };
    assert!(matches!(
        encode(&img, &opts),
        Err(DngError::InvalidValue(_))
    ));
}

#[test]
fn write_file_does_not_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.dng");
    let img = sample_image(CfaPattern::RGGB);
    write_file(&path, &img, &DngOptions::default()).unwrap();
    let first = std::fs::read(&path).unwrap();
    assert_eq!(first, encode(&img, &DngOptions::default()).unwrap());
    let err = write_file(&path, &img, &DngOptions::default()).unwrap_err();
    assert!(
        matches!(&err, DngError::Io(e) if e.kind() == std::io::ErrorKind::AlreadyExists),
        "{err}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), first);
}

#[test]
fn rational_conversion() {
    assert_eq!(rational(0.0).unwrap(), (0, 1));
    assert_eq!(rational(512.0).unwrap(), (512, 1));
    assert_eq!(rational(0.5).unwrap(), (1, 2));
    assert_eq!(rational(0.625).unwrap(), (5, 8));
    let (n, d) = rational(1.0 / 3.0).unwrap();
    assert!((f64::from(n) / f64::from(d) - 1.0 / 3.0).abs() < 1e-6);
    // 大きな値は分母を小さくする。
    let (n, d) = rational(123_456.789).unwrap();
    assert!((f64::from(n) / f64::from(d) - 123_456.789).abs() < 0.01);
    assert!(rational(-1.0).is_err());
    assert!(rational(f64::NAN).is_err());
    assert!(rational(1e12).is_err());
    assert_eq!(srational(-0.25).unwrap(), (-1, 4));
    // SRATIONAL の分子は i32 に収める。
    let (n, d) = srational(-3_000.123_456).unwrap();
    assert!((f64::from(n) / f64::from(d) + 3_000.123_456).abs() < 1e-3);
    assert!(srational(3e9).is_err());
    assert_eq!(srational(1.25).unwrap(), (5, 4));
    assert!(srational(f64::INFINITY).is_err());
    assert_eq!(exposure_time(1.0 / 8000.0).unwrap(), (1, 8000));
    // 電子シャッターの 1/32000 秒も 1/n の形にする（f32 の 1/32000 の逆数は 32000 から
    // 約 2e-3 ずれるため、差の絶対値で判定すると 31/1000000 になっていた）。
    assert_eq!(exposure_time(1.0 / 32000.0).unwrap(), (1, 32000));
    assert_eq!(exposure_time(1.0 / 16000.0).unwrap(), (1, 16000));
    // 1/n でない値は 1/n に丸めない。
    let (n, d) = exposure_time(1.0 / 250.4).unwrap();
    assert!(
        (f64::from(n) / f64::from(d) - 1.0 / 250.4).abs() < 1e-6,
        "{n}/{d}"
    );
    let (n, d) = exposure_time(0.3333).unwrap();
    assert_eq!((n, d), (3333, 10000));
    assert_eq!(exposure_time(0.3).unwrap(), (3, 10));
    assert_eq!(exposure_time(30.0).unwrap(), (30, 1));
    assert_eq!(exposure_time(2.5).unwrap(), (5, 2));
}

#[test]
fn dms_conversion() {
    let v = dms(35.681236).unwrap();
    let back: f64 = v[0].0 as f64 / v[0].1 as f64
        + (v[1].0 as f64 / v[1].1 as f64) / 60.0
        + (v[2].0 as f64 / v[2].1 as f64) / 3600.0;
    assert!((back - 35.681236).abs() < 1e-9);
    assert_eq!(v[0], (35, 1));
    assert_eq!(v[1], (40, 1));
    assert_eq!(dms(0.0).unwrap(), vec![(0, 1), (0, 1), (0, 1)]);
}

#[test]
fn exif_datetime_split() {
    assert_eq!(
        split_exif_datetime("2024:05:01 12:34:56").unwrap(),
        ("2024:05:01 12:34:56".to_owned(), None)
    );
    assert_eq!(
        split_exif_datetime("2024:05:01 12:34:56.07").unwrap(),
        ("2024:05:01 12:34:56".to_owned(), Some("07".to_owned()))
    );
    assert!(split_exif_datetime("").is_err());
    assert!(split_exif_datetime("2024/05/01 12:34:56").is_err());
}
