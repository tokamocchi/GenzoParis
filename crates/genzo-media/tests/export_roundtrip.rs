//! 書き出した JPEG / TIFF / PNG を読み戻して、ICC プロファイル（IQ-06）・Exif・GPS の有無
//! （EXP-04・SEC-03）・画素を確かめる。原本の保護（6.4 節）と読み取り専用での読み込み（DATA-01）も
//! 確かめる。

use std::fs;
use std::io::Cursor;
use std::path::Path;

use genzo_media::{
    DynRgbImage, EXIF_SOFTWARE, ExifData, MediaError, ProtectedFiles, RgbImage16, SourceProfile,
    WriteOutcome, decode_image_file, export_icc_profile, export_image,
};
use genzo_model::{
    CaptureInfo, ConflictPolicy, ExportFormat, ExportSettings, ExportSize, GpsCoord, Orientation,
    OutputColorSpace, PhotoMetadata,
};

fn sample_image() -> DynRgbImage {
    DynRgbImage::Rgb16(
        RgbImage16::from_fn(48, 32, |x, y| {
            [(x * 1365) as u16, (y * 2047) as u16, ((x + y) * 800) as u16]
        })
        .unwrap(),
    )
}

fn sample_metadata() -> PhotoMetadata {
    PhotoMetadata {
        make: Some("SONY".to_owned()),
        model: Some("ILCE-7M4".to_owned()),
        lens: Some("FE 24-70mm F2.8 GM II".to_owned()),
        iso: Some(400),
        aperture: Some(4.0),
        shutter_s: Some(1.0 / 125.0),
        focal_mm: Some(50.0),
        // 元の写真は縦位置（向き 6）。書き出しでは向きを画素に反映済みなので 1 を書く。
        orientation: Orientation::Rotate90Cw,
        gps: GpsCoord::new(35.658_581, 139.745_433),
        capture: CaptureInfo {
            datetime: Some("2024:10:01 09:08:07".to_owned()),
            offset: Some("+09:00".to_owned()),
        },
        ..Default::default()
    }
}

fn read_exif(path: &Path) -> exif::Exif {
    let bytes = fs::read(path).unwrap();
    exif::Reader::new()
        .read_from_container(&mut Cursor::new(bytes))
        .unwrap()
}

fn ascii(e: &exif::Exif, tag: exif::Tag) -> Option<String> {
    match &e.get_field(tag, exif::In::PRIMARY)?.value {
        exif::Value::Ascii(v) => Some(String::from_utf8_lossy(&v[0]).into_owned()),
        _ => None,
    }
}

#[test]
fn every_format_and_color_space_round_trips_icc_and_exif() {
    let dir = tempfile::tempdir().unwrap();
    let img = sample_image();
    let exif_data = ExifData::from_photo_metadata(&sample_metadata());
    let formats = [
        ExportFormat::Jpeg { quality: 95 },
        ExportFormat::Tiff16,
        ExportFormat::Png8,
        ExportFormat::Png16,
    ];
    let spaces = [
        OutputColorSpace::Srgb,
        OutputColorSpace::DisplayP3,
        OutputColorSpace::AdobeRgb,
    ];
    for (fi, format) in formats.into_iter().enumerate() {
        for color_space in spaces {
            for remove_gps in [false, true] {
                let settings = ExportSettings {
                    format,
                    color_space,
                    remove_gps,
                    ..Default::default()
                };
                let name = format!(
                    "out{fi}-{color_space:?}-{remove_gps}.{}",
                    format.extension()
                );
                let desired = dir.path().join(name);
                let outcome = export_image(
                    &img,
                    &settings,
                    Some(&exif_data),
                    &desired,
                    &ProtectedFiles::new(),
                )
                .unwrap();
                assert_eq!(
                    outcome,
                    WriteOutcome::Written {
                        path: desired.clone(),
                        replaced: false
                    }
                );
                let ctx = format!("{format:?} {color_space:?} remove_gps={remove_gps}");

                // ICC: 出力の色空間のプロファイルがそのまま埋め込まれている（IQ-06）。
                let decoded = decode_image_file(&desired).unwrap();
                assert_eq!(
                    decoded.profile,
                    SourceProfile::Embedded(export_icc_profile(color_space).unwrap()),
                    "{ctx}"
                );
                assert_eq!(decoded.pixels.dimensions(), (48, 32), "{ctx}");
                assert_eq!(
                    decoded.pixels.bits_per_channel(),
                    format.bits_per_channel(),
                    "{ctx}"
                );

                // Exif（デコーダー経由の PhotoMetadata）。
                let m = &decoded.metadata;
                assert_eq!(m.make.as_deref(), Some("SONY"), "{ctx}");
                assert_eq!(m.model.as_deref(), Some("ILCE-7M4"), "{ctx}");
                assert_eq!(m.lens.as_deref(), Some("FE 24-70mm F2.8 GM II"), "{ctx}");
                assert_eq!(m.iso, Some(400), "{ctx}");
                assert_eq!(m.orientation, Orientation::Normal, "{ctx}");
                assert_eq!(
                    m.capture.datetime.as_deref(),
                    Some("2024:10:01 09:08:07"),
                    "{ctx}"
                );
                assert_eq!(m.capture.offset.as_deref(), Some("+09:00"), "{ctx}");
                // GPS は remove_gps = false のときだけ（EXP-04・SEC-03）。
                assert_eq!(m.gps.is_some(), !remove_gps, "{ctx}");
                if let Some(g) = m.gps {
                    assert!((g.lat - 35.658_581).abs() < 1e-6, "{ctx}: {g:?}");
                    assert!((g.lon - 139.745_433).abs() < 1e-6, "{ctx}: {g:?}");
                }

                // Exif（kamadak-exif でファイルを直接読む）。
                let e = read_exif(&desired);
                assert_eq!(
                    ascii(&e, exif::Tag::Software).as_deref(),
                    Some(EXIF_SOFTWARE),
                    "{ctx}"
                );
                let gps_fields = e
                    .fields()
                    .filter(|f| f.tag.context() == exif::Context::Gps)
                    .count();
                if remove_gps {
                    assert_eq!(gps_fields, 0, "{ctx}");
                } else {
                    assert!(gps_fields >= 4, "{ctx}");
                }
                let cs = e
                    .get_field(exif::Tag::ColorSpace, exif::In::PRIMARY)
                    .and_then(|f| f.value.get_uint(0));
                let expected_cs = if color_space == OutputColorSpace::Srgb {
                    1
                } else {
                    0xFFFF
                };
                assert_eq!(cs, Some(expected_cs), "{ctx}");
            }
        }
    }
}

#[test]
fn pixels_survive_lossless_formats() {
    let dir = tempfile::tempdir().unwrap();
    let img = sample_image();
    for format in [
        ExportFormat::Tiff16,
        ExportFormat::Png16,
        ExportFormat::Png8,
    ] {
        let settings = ExportSettings {
            format,
            ..Default::default()
        };
        let path = dir.path().join(format!("px.{}", format.extension()));
        export_image(&img, &settings, None, &path, &ProtectedFiles::new()).unwrap();
        let d = decode_image_file(&path).unwrap();
        match format {
            ExportFormat::Png8 => assert_eq!(d.pixels.to_rgb8(), img.to_rgb8()),
            _ => assert_eq!(d.pixels.to_rgb16(), img.to_rgb16()),
        }
        // Exif を渡さなければ Exif は書かない。
        assert_eq!(d.metadata.make, None);
    }
    // JPEG は非可逆なので、近いことだけを確かめる。
    let path = dir.path().join("px.jpg");
    export_image(
        &img,
        &ExportSettings {
            format: ExportFormat::Jpeg { quality: 100 },
            ..Default::default()
        },
        None,
        &path,
        &ProtectedFiles::new(),
    )
    .unwrap();
    let d = decode_image_file(&path).unwrap().pixels.to_rgb8();
    let src = img.to_rgb8();
    let max_diff = d
        .as_raw()
        .iter()
        .zip(src.as_raw())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(max_diff <= 12, "{max_diff}");
}

#[test]
fn long_edge_is_applied() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("small.png");
    let settings = ExportSettings {
        format: ExportFormat::Png8,
        size: ExportSize::LongEdge(24),
        ..Default::default()
    };
    export_image(
        &sample_image(),
        &settings,
        None,
        &path,
        &ProtectedFiles::new(),
    )
    .unwrap();
    assert_eq!(
        decode_image_file(&path).unwrap().pixels.dimensions(),
        (24, 16)
    );
}

#[test]
fn export_never_touches_the_original() {
    let dir = tempfile::tempdir().unwrap();
    // 原本（カタログに登録された JPEG）。
    let original = dir.path().join("DSC01234.JPG");
    let first = export_image(
        &sample_image(),
        &ExportSettings::default(),
        None,
        &original,
        &ProtectedFiles::new(),
    )
    .unwrap();
    assert!(matches!(first, WriteOutcome::Written { .. }));
    let before = fs::read(&original).unwrap();
    let protected: ProtectedFiles = [&original].into_iter().collect();

    // 上書きの設定でも、原本と同じパス・ハードリンクへは書き出さない。
    let link = dir.path().join("hardlink.jpg");
    fs::hard_link(&original, &link).unwrap();
    for dest in [&original, &link] {
        let settings = ExportSettings {
            on_conflict: ConflictPolicy::Overwrite,
            ..Default::default()
        };
        let r = export_image(&sample_image(), &settings, None, dest, &protected);
        assert!(
            matches!(r, Err(MediaError::ProtectedDestination { .. })),
            "{dest:?}: {r:?}"
        );
    }
    // 連番の設定（既定）なら、別の名前で書き出す。
    let r = export_image(
        &sample_image(),
        &ExportSettings::default(),
        None,
        &original,
        &protected,
    )
    .unwrap();
    assert_eq!(
        r,
        WriteOutcome::Written {
            path: dir.path().join("DSC01234-1.JPG"),
            replaced: false
        }
    );
    // スキップの設定なら何もしない。
    let r = export_image(
        &sample_image(),
        &ExportSettings {
            on_conflict: ConflictPolicy::Skip,
            ..Default::default()
        },
        None,
        &original,
        &protected,
    )
    .unwrap();
    assert_eq!(
        r,
        WriteOutcome::Skipped {
            existing: original.clone()
        }
    );
    assert_eq!(fs::read(&original).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn originals_are_opened_read_only() {
    use std::os::unix::fs::PermissionsExt;
    // 読み取り専用のファイルも読める（書き込みの権限を要求しない。DATA-01）。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ro.png");
    export_image(
        &sample_image(),
        &ExportSettings {
            format: ExportFormat::Png8,
            ..Default::default()
        },
        None,
        &path,
        &ProtectedFiles::new(),
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    decode_image_file(&path).unwrap();
    genzo_media::probe_image_file(&path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
}
