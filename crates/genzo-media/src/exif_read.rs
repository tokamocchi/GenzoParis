//! Exif の読み取り（kamadak-exif）。LIB-14 のメタデータ（カメラ・レンズ・露出・GPS・撮影日時・向き）を
//! [`PhotoMetadata`] にする。ICC プロファイルのない画像の色空間の手がかり（DCF のオプション色空間）も
//! 読む（[`adobe_rgb_hint`]）。
//!
//! Exif は信頼できない入力として扱う。読めない・範囲外の値は `None` にし、エラーにはしない
//! （一部が壊れていても、読めた項目は使う）。

use exif::{Exif, Field, In, Reader, Tag, Value};
use genzo_model::{CaptureInfo, GpsCoord, Orientation, PhotoMetadata};

use crate::exif_write::EXIF_MAX_ASCII_CHARS;

/// Exif の TIFF の構造（JPEG の APP1 の中身、PNG の eXIf チャンク、TIFF ファイル全体）を解析する。
///
/// 先頭に `"Exif\0\0"` があれば取り除く。一部が壊れていても、読めた部分を返す。
/// まったく読めなければ `None`。
pub fn parse_exif(mut blob: Vec<u8>) -> Option<Exif> {
    if blob.starts_with(b"Exif\0\0") {
        blob.drain(..6);
    }
    match Reader::new().continue_on_error(true).read_raw(blob) {
        Ok(e) => Some(e),
        Err(e) => e.distill_partial_result(|_| {}).ok(),
    }
}

fn field(e: &Exif, tag: Tag) -> Option<&Field> {
    e.get_field(tag, In::PRIMARY)
}

fn ascii(e: &Exif, tag: Tag) -> Option<String> {
    match &field(e, tag)?.value {
        Value::Ascii(v) => {
            let raw = v.first()?;
            let s = String::from_utf8_lossy(raw);
            let t: String = s
                .trim_matches(|c: char| c.is_whitespace() || c == '\0')
                .chars()
                .take(EXIF_MAX_ASCII_CHARS)
                .collect();
            (!t.is_empty()).then_some(t)
        }
        _ => None,
    }
}

fn uint(e: &Exif, tag: Tag) -> Option<u32> {
    field(e, tag)?.value.get_uint(0)
}

/// 最初の値を f64 にする（RATIONAL・SRATIONAL・整数）。分母が 0・非有限は `None`。
fn number(e: &Exif, tag: Tag) -> Option<f64> {
    let v = match &field(e, tag)?.value {
        Value::Rational(v) => {
            let r = v.first()?;
            (r.denom != 0).then(|| r.to_f64())?
        }
        Value::SRational(v) => {
            let r = v.first()?;
            (r.denom != 0).then(|| r.to_f64())?
        }
        other => f64::from(other.get_uint(0)?),
    };
    v.is_finite().then_some(v)
}

fn positive_f32(v: Option<f64>) -> Option<f32> {
    v.filter(|x| *x > 0.0 && x.is_finite())
        .map(|x| x as f32)
        .filter(|x| x.is_finite())
}

/// 度・分・秒の 3 つの有理数を度にする。
fn dms(e: &Exif, tag: Tag) -> Option<f64> {
    match &field(e, tag)?.value {
        Value::Rational(v) if v.len() >= 3 => {
            let mut deg = 0.0;
            for (r, div) in v.iter().take(3).zip([1.0, 60.0, 3600.0]) {
                if r.denom == 0 {
                    return None;
                }
                deg += r.to_f64() / div;
            }
            deg.is_finite().then_some(deg)
        }
        _ => None,
    }
}

fn gps(e: &Exif) -> Option<GpsCoord> {
    let lat = dms(e, Tag::GPSLatitude)?;
    let lon = dms(e, Tag::GPSLongitude)?;
    let sign = |tag, negative: &str| match ascii(e, tag) {
        Some(r) if r.eq_ignore_ascii_case(negative) => -1.0,
        _ => 1.0,
    };
    GpsCoord::new(
        lat * sign(Tag::GPSLatitudeRef, "S"),
        lon * sign(Tag::GPSLongitudeRef, "W"),
    )
}

/// ISO 感度。PhotographicSensitivity が 65535（上限）で ISOSpeed があれば、そちらを使う。
fn iso(e: &Exif) -> Option<u32> {
    let base = uint(e, Tag::PhotographicSensitivity).filter(|&v| v > 0);
    match base {
        Some(65535) | None => uint(e, Tag::ISOSpeed).filter(|&v| v > 0).or(base),
        some => some,
    }
}

/// 日時と小数秒を `"YYYY:MM:DD HH:MM:SS.fff"` の形にまとめる（小数秒は数字だけのときに付ける）。
fn datetime_with_subsec(e: &Exif, dt: Tag, subsec: Tag) -> Option<String> {
    let base = ascii(e, dt)?;
    match ascii(e, subsec) {
        Some(s) if !s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit()) => {
            Some(format!("{base}.{s}"))
        }
        _ => Some(base),
    }
}

/// 撮影日時。DateTimeOriginal、なければ DateTimeDigitized（それぞれの小数秒・オフセットと組で）。
///
/// DateTime（0x0132、ファイルの変更日時）は撮影日時ではないので使わない。
fn capture(e: &Exif) -> CaptureInfo {
    for (dt, subsec, offset) in [
        (
            Tag::DateTimeOriginal,
            Tag::SubSecTimeOriginal,
            Tag::OffsetTimeOriginal,
        ),
        (
            Tag::DateTimeDigitized,
            Tag::SubSecTimeDigitized,
            Tag::OffsetTimeDigitized,
        ),
    ] {
        if let Some(datetime) = datetime_with_subsec(e, dt, subsec) {
            return CaptureInfo {
                datetime: Some(datetime),
                offset: ascii(e, offset),
            };
        }
    }
    CaptureInfo::default()
}

/// 向き（Orientation）。1〜8 以外・なしは [`Orientation::Normal`]。
pub fn exif_orientation(e: &Exif) -> Orientation {
    uint(e, Tag::Orientation)
        .and_then(|v| u16::try_from(v).ok())
        .and_then(Orientation::from_exif)
        .unwrap_or_default()
}

/// ICC プロファイルのない画像を Adobe RGB (1998) とみなす、Exif の手がかり（指摘 F28）。
///
/// DCF（CIPA DC-009）のオプション色空間の画像（カメラで色空間を AdobeRGB にして撮った JPEG）は、
/// ICC プロファイルを埋め込まず、Exif だけで色空間を示すことがある。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdobeRgbBasis {
    /// ColorSpace（0xA001）が 0xFFFF（Uncalibrated）で、Interoperability IFD の
    /// InteroperabilityIndex が `"R03"`（DCF のオプション色空間）。
    DcfOptionR03,
    /// ColorSpace が 2（Exif の規格にない値。Adobe RGB を示すのに使うカメラ・ソフトがある）。
    ColorSpace2,
}

/// ColorSpace の値: Uncalibrated（sRGB 以外）。
const COLOR_SPACE_UNCALIBRATED: u32 = 0xFFFF;
/// ColorSpace の値: Adobe RGB を示す非標準の値。
const COLOR_SPACE_NONSTANDARD_ADOBE_RGB: u32 = 2;

/// Exif が Adobe RGB (1998) を示すか（[`AdobeRgbBasis`]）。示さなければ `None`。
///
/// ColorSpace=1（sRGB）・InteropIndex が `"R98"`（基本の DCF）・Uncalibrated だけ（Display P3 など。
/// このアプリの Display P3 / Adobe RGB の書き出しも ICC を埋め込んだうえで 0xFFFF を書く）は
/// 手がかりにしない。ICC プロファイルがあれば ICC を優先する（呼び出し側。[`crate::decode`]）。
pub fn adobe_rgb_hint(e: &Exif) -> Option<AdobeRgbBasis> {
    match uint(e, Tag::ColorSpace)? {
        COLOR_SPACE_UNCALIBRATED
            if ascii(e, Tag::InteroperabilityIndex)
                .is_some_and(|i| i.eq_ignore_ascii_case("R03")) =>
        {
            Some(AdobeRgbBasis::DcfOptionR03)
        }
        COLOR_SPACE_NONSTANDARD_ADOBE_RGB => Some(AdobeRgbBasis::ColorSpace2),
        _ => None,
    }
}

/// Exif から写真のメタデータを作る。幅・高さは入れない（画像のデコード側で入れる）。
pub fn photo_metadata_from_exif(e: &Exif) -> PhotoMetadata {
    PhotoMetadata {
        make: ascii(e, Tag::Make),
        model: ascii(e, Tag::Model),
        lens: ascii(e, Tag::LensModel),
        iso: iso(e),
        aperture: positive_f32(number(e, Tag::FNumber)),
        shutter_s: positive_f32(number(e, Tag::ExposureTime)),
        focal_mm: positive_f32(number(e, Tag::FocalLength)),
        width: None,
        height: None,
        orientation: exif_orientation(e),
        gps: gps(e),
        capture: capture(e),
    }
}

/// テスト用の Exif（TIFF の構造）。
#[cfg(test)]
pub(crate) mod test_support {
    /// ColorSpace（Exif IFD）と InteroperabilityIndex（Interoperability IFD）だけを持つ Exif を
    /// 手で組み立てる（リトルエンディアン）。exif_write は Interoperability IFD を書かないため。
    pub(crate) fn color_space_exif(
        color_space: Option<u16>,
        interop_index: Option<&str>,
    ) -> Vec<u8> {
        fn entry(out: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: [u8; 4]) {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&value);
        }
        const SHORT: u16 = 3;
        const LONG: u16 = 4;
        const ASCII: u16 = 2;
        let mut out = b"II*\0".to_vec();
        out.extend_from_slice(&8u32.to_le_bytes());
        // 0th IFD（8 バイト目から。エントリは Exif IFD へのポインタだけ）。
        let exif_ifd = 8 + 2 + 12 + 4;
        out.extend_from_slice(&1u16.to_le_bytes());
        entry(&mut out, 0x8769, LONG, 1, (exif_ifd as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        // Exif IFD。
        let n = u16::from(color_space.is_some()) + u16::from(interop_index.is_some());
        let interop_ifd = exif_ifd + 2 + 12 * usize::from(n) + 4;
        out.extend_from_slice(&n.to_le_bytes());
        if let Some(cs) = color_space {
            let v = cs.to_le_bytes();
            entry(&mut out, 0xA001, SHORT, 1, [v[0], v[1], 0, 0]);
        }
        if interop_index.is_some() {
            entry(
                &mut out,
                0xA005,
                LONG,
                1,
                (interop_ifd as u32).to_le_bytes(),
            );
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        // Interoperability IFD（InteroperabilityIndex は 3 文字 + NUL なので値の欄に収まる）。
        if let Some(idx) = interop_index {
            assert_eq!(idx.len(), 3);
            let b = idx.as_bytes();
            out.extend_from_slice(&1u16.to_le_bytes());
            entry(&mut out, 0x0001, ASCII, 4, [b[0], b[1], b[2], 0]);
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif_write::{ByteOrder, ExifData, ExifImageInfo, build_ifds, encode_exif_tiff};
    use genzo_model::OutputColorSpace;

    fn sample() -> PhotoMetadata {
        PhotoMetadata {
            make: Some("SONY".to_owned()),
            model: Some("ILCE-7C".to_owned()),
            lens: Some("FE 35mm F1.8".to_owned()),
            iso: Some(3200),
            aperture: Some(1.8),
            shutter_s: Some(1.0 / 60.0),
            focal_mm: Some(35.0),
            gps: GpsCoord::new(-12.345_678, 98.765_432),
            capture: CaptureInfo {
                datetime: Some("2023:12:31 23:59:58.250".to_owned()),
                offset: Some("-03:30".to_owned()),
            },
            ..Default::default()
        }
    }

    fn write(meta: &PhotoMetadata) -> Vec<u8> {
        let ifds = build_ifds(
            &ExifData::from_photo_metadata(meta),
            &ExifImageInfo {
                width: 10,
                height: 10,
                color_space: OutputColorSpace::Srgb,
            },
        );
        encode_exif_tiff(&ifds, ByteOrder::Little).unwrap()
    }

    #[test]
    fn round_trip_through_writer() {
        let meta = sample();
        let e = parse_exif(write(&meta)).unwrap();
        let back = photo_metadata_from_exif(&e);
        assert_eq!(back.make, meta.make);
        assert_eq!(back.model, meta.model);
        assert_eq!(back.lens, meta.lens);
        assert_eq!(back.iso, meta.iso);
        assert!((back.aperture.unwrap() - 1.8).abs() < 1e-6);
        assert!((back.shutter_s.unwrap() - 1.0 / 60.0).abs() < 1e-9);
        assert!((back.focal_mm.unwrap() - 35.0).abs() < 1e-6);
        // 書き出しは向きを 1 にする。
        assert_eq!(back.orientation, Orientation::Normal);
        // GPS は 0.0001 秒（約 3mm）単位で丸める。
        let g = back.gps.unwrap();
        assert!((g.lat - -12.345_678).abs() < 1e-7, "{g:?}");
        assert!((g.lon - 98.765_432).abs() < 1e-7, "{g:?}");
        assert_eq!(
            back.capture.datetime.as_deref(),
            Some("2023:12:31 23:59:58.250")
        );
        assert_eq!(back.capture.offset.as_deref(), Some("-03:30"));
    }

    #[test]
    fn exif_header_prefix_is_accepted() {
        let mut blob = b"Exif\0\0".to_vec();
        blob.extend(write(&sample()));
        let e = parse_exif(blob).unwrap();
        assert_eq!(ascii(&e, Tag::Make).as_deref(), Some("SONY"));
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(parse_exif(Vec::new()).is_none());
        assert!(parse_exif(b"not exif at all".to_vec()).is_none());
        let good = write(&sample());
        for cut in [8, 16, 30, good.len() / 2, good.len() - 1] {
            if let Some(e) = parse_exif(good[..cut].to_vec()) {
                let _ = photo_metadata_from_exif(&e);
            }
        }
        // オフセットを壊したもの。
        let mut bad = good.clone();
        for b in bad.iter_mut().skip(20).step_by(7) {
            *b = 0xFF;
        }
        if let Some(e) = parse_exif(bad) {
            let _ = photo_metadata_from_exif(&e);
        }
    }

    #[test]
    fn orientation_and_iso_speed_fallback() {
        use crate::exif_write::{Entry, ExifIfds, Value as W, tag};
        let ifds = ExifIfds {
            ifd0: vec![Entry {
                tag: tag::ORIENTATION,
                value: W::Short(vec![6]),
            }],
            exif: vec![
                Entry {
                    tag: tag::PHOTOGRAPHIC_SENSITIVITY,
                    value: W::Short(vec![65535]),
                },
                Entry {
                    tag: tag::ISO_SPEED,
                    value: W::Long(vec![204_800]),
                },
                // 分母が 0 の値は読まない。
                Entry {
                    tag: tag::F_NUMBER,
                    value: W::Rational(vec![(28, 0)]),
                },
            ],
            gps: None,
        };
        let e = parse_exif(encode_exif_tiff(&ifds, ByteOrder::Big).unwrap()).unwrap();
        let m = photo_metadata_from_exif(&e);
        assert_eq!(m.orientation, Orientation::Rotate90Cw);
        assert_eq!(m.iso, Some(204_800));
        assert_eq!(m.aperture, None);
        assert_eq!(m.gps, None);
        assert_eq!(m.capture, CaptureInfo::default());

        // 範囲外の向きは 1 とみなす。
        let ifds = ExifIfds {
            ifd0: vec![Entry {
                tag: tag::ORIENTATION,
                value: W::Short(vec![9]),
            }],
            exif: vec![],
            gps: None,
        };
        let e = parse_exif(encode_exif_tiff(&ifds, ByteOrder::Big).unwrap()).unwrap();
        assert_eq!(exif_orientation(&e), Orientation::Normal);
    }

    /// ICC のない画像の色空間の手がかり（DCF のオプション色空間。指摘 F28）。
    #[test]
    fn adobe_rgb_hint_from_dcf_color_space() {
        use test_support::color_space_exif;
        let hint = |cs, idx| adobe_rgb_hint(&parse_exif(color_space_exif(cs, idx)).unwrap());
        // DCF のオプション色空間: ColorSpace=0xFFFF（Uncalibrated）かつ InteropIndex="R03"。
        assert_eq!(
            hint(Some(0xFFFF), Some("R03")),
            Some(AdobeRgbBasis::DcfOptionR03)
        );
        // 非標準の ColorSpace=2（Adobe RGB を示すのに使うカメラ・ソフトがある）。
        assert_eq!(hint(Some(2), None), Some(AdobeRgbBasis::ColorSpace2));
        assert_eq!(hint(Some(2), Some("R98")), Some(AdobeRgbBasis::ColorSpace2));
        // sRGB（ColorSpace=1）・基本の DCF（R98）・Uncalibrated だけ（このアプリの Display P3 の
        // 書き出しなど。ICC を埋め込む）は手がかりにしない。
        assert_eq!(hint(Some(1), Some("R98")), None);
        assert_eq!(hint(Some(1), Some("R03")), None);
        assert_eq!(hint(Some(0xFFFF), Some("R98")), None);
        assert_eq!(hint(Some(0xFFFF), None), None);
        assert_eq!(hint(None, Some("R03")), None);
        assert_eq!(hint(None, None), None);
        // このアプリの書き出しの Exif（sRGB）。
        assert_eq!(adobe_rgb_hint(&parse_exif(write(&sample())).unwrap()), None);
    }
}
