//! TIFF の書き出し（16bit の RGB。EXP-01）。
//!
//! - ICC プロファイルは 0th IFD のタグ 34675（InterColorProfile、型 UNDEFINED）に入れる
//!   （ICC.1 の付録 B「Embedding ICC profiles」の TIFF の節、TIFF/EP。節の番号は原典で確認すること）。
//! - Exif は、カメラ・Software・Orientation を 0th IFD に、撮影情報を Exif IFD（ポインタ 0x8769）に、
//!   GPS を GPS IFD（ポインタ 0x8825）に書く（JPEG・PNG と同じ内容。[`crate::exif_write`]）。
//! - 圧縮は [`TIFF_COMPRESSION`]（Deflate ＋ 水平差分の予測。仮置き）。
//! - バイト順は tiff crate に従い、この環境のバイト順（x86-64・Apple Silicon ではリトルエンディアン）。

use std::io::{Seek, Write};

use tiff::Directory;
use tiff::encoder::compression::DeflateLevel;
use tiff::encoder::{Compression, DirectoryEncoder, Predictor, TiffEncoder, TiffKind, colortype};
use tiff::tags::{Tag, Type};

use crate::buffer::RgbImage16;
use crate::error::{MediaError, Result};
use crate::exif_write::{ByteOrder, Entry, ExifIfds, tag};

/// TIFF の圧縮（仮置き）。
///
/// Deflate（Adobe Deflate、Compression = 8）に水平差分の予測（Predictor = 2）を組み合わせる。
/// 16bit の写真で無圧縮より小さくなり、Photoshop・Lightroom・macOS のプレビュー・Windows の
/// フォトなど主なソフトが読める形式。互換性と書き出しの時間（PERF-10）を PoC で確認して見直す。
pub const TIFF_COMPRESSION: (Compression, Predictor) = (
    Compression::Deflate(DeflateLevel::Balanced),
    Predictor::Horizontal,
);

/// ICC プロファイルのタグの番号（InterColorProfile）。
pub const TIFF_TAG_ICC_PROFILE: u16 = 34675;

fn tiff_err(e: tiff::TiffError) -> MediaError {
    MediaError::encode(format!("TIFF: {e}"))
}

/// エントリを IFD に書く（値はこの環境のバイト順で書く）。
fn write_entries<W: Write + Seek, K: TiffKind>(
    dir: &mut DirectoryEncoder<'_, W, K>,
    entries: &[Entry],
) -> Result<()> {
    let order = ByteOrder::native();
    let mut d = Directory::empty();
    for e in entries {
        let ty = Type::from_u16(e.value.type_code())
            .ok_or_else(|| MediaError::encode("TIFF: 未知の型"))?;
        let entry = dir
            .write_entry_bytes(ty, &e.value.bytes(order))
            .map_err(tiff_err)?;
        d.extend([(Tag::from_u16_exhaustive(e.tag), entry)]);
    }
    dir.extend_from(&d);
    Ok(())
}

/// 16bit の RGB 画像を TIFF にして `w` に書く（Exif は IFD のエントリで受け取る）。
///
/// 公開の入り口は [`crate::export::encode_export_to`]（ExportFormat::Tiff16）。
pub(crate) fn encode_tiff16_to<W: Write + Seek>(
    img: &RgbImage16,
    icc: Option<&[u8]>,
    exif: Option<&ExifIfds>,
    w: &mut W,
) -> Result<()> {
    let (compression, predictor) = TIFF_COMPRESSION;
    let mut enc = TiffEncoder::new(w)
        .map_err(tiff_err)?
        .with_compression(compression)
        .with_predictor(predictor);

    // Exif IFD と GPS IFD を先に書き、そのオフセットを 0th IFD に書く。
    let mut pointers = Vec::new();
    if let Some(ifds) = exif {
        if !ifds.exif.is_empty() {
            let mut dir = enc.extra_directory().map_err(tiff_err)?;
            write_entries(&mut dir, &ifds.exif)?;
            let off = dir.finish_with_offsets().map_err(tiff_err)?;
            pointers.push((tag::EXIF_IFD, off.offset));
        }
        if let Some(gps) = ifds.gps.as_ref().filter(|g| !g.is_empty()) {
            let mut dir = enc.extra_directory().map_err(tiff_err)?;
            write_entries(&mut dir, gps)?;
            let off = dir.finish_with_offsets().map_err(tiff_err)?;
            pointers.push((tag::GPS_IFD, off.offset));
        }
    }

    let (width, height) = img.dimensions();
    let mut image = enc
        .new_image::<colortype::RGB16>(width, height)
        .map_err(tiff_err)?;
    {
        let dir = image.encoder();
        if let Some(ifds) = exif {
            // 解像度などのタグも Exif と同じ値で上書きする（tiff crate の既定は単位なし 1/1）。
            write_entries(dir, &ifds.ifd0)?;
        }
        for (t, off) in pointers {
            dir.write_tag(Tag::from_u16_exhaustive(t), off)
                .map_err(tiff_err)?;
        }
        if let Some(icc) = icc {
            let entry = dir
                .write_entry_bytes(Type::UNDEFINED, icc)
                .map_err(tiff_err)?;
            let mut d = Directory::empty();
            d.extend([(Tag::from_u16_exhaustive(TIFF_TAG_ICC_PROFILE), entry)]);
            dir.extend_from(&d);
        }
    }
    image.write_data(img.as_raw()).map_err(tiff_err)
}

/// 16bit の RGB 画像を TIFF のバイト列にする。
#[cfg(test)]
pub(crate) fn encode_tiff16(
    img: &RgbImage16,
    icc: Option<&[u8]>,
    exif: Option<&ExifIfds>,
) -> Result<Vec<u8>> {
    let mut cur = std::io::Cursor::new(Vec::new());
    encode_tiff16_to(img, icc, exif, &mut cur)?;
    Ok(cur.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif_write::{ExifData, ExifImageInfo, build_ifds};
    use genzo_model::{GpsCoord, OutputColorSpace};

    #[test]
    fn round_trip_pixels_icc_and_exif() {
        let img = RgbImage16::from_fn(33, 17, |x, y| {
            [x as u16 * 1999, y as u16 * 3851, (x * y) as u16 * 113]
        })
        .unwrap();
        let icc: Vec<u8> = (0..1001).map(|i| (i % 253) as u8).collect();
        let data = ExifData {
            make: Some("SONY".to_owned()),
            model: Some("ILCE-7C".to_owned()),
            gps: GpsCoord::new(1.5, 2.5),
            iso: Some(100),
            ..Default::default()
        };
        let ifds = build_ifds(
            &data,
            &ExifImageInfo {
                width: 33,
                height: 17,
                color_space: OutputColorSpace::AdobeRgb,
            },
        );
        let bytes = encode_tiff16(&img, Some(&icc), Some(&ifds)).unwrap();

        // 画素と ICC（tiff crate のデコーダー）。
        let mut dec = tiff::decoder::Decoder::new(std::io::Cursor::new(&bytes)).unwrap();
        assert_eq!(dec.dimensions().unwrap(), (33, 17));
        assert_eq!(
            dec.get_tag_u8_vec(Tag::from_u16_exhaustive(TIFF_TAG_ICC_PROFILE))
                .unwrap(),
            icc
        );
        match dec.read_image().unwrap() {
            tiff::decoder::DecodingResult::U16(v) => assert_eq!(v, img.as_raw()),
            _ => panic!("16bit ではない"),
        }
        assert_eq!(
            dec.get_tag_u32(Tag::Compression).unwrap(),
            8,
            "Deflate（Adobe）"
        );
        assert_eq!(dec.get_tag_u32(Tag::Predictor).unwrap(), 2);

        // Exif（kamadak-exif は TIFF をそのまま読める）。
        let e = exif::Reader::new().read_raw(bytes).unwrap();
        let make = e.get_field(exif::Tag::Make, exif::In::PRIMARY).unwrap();
        assert_eq!(make.display_value().to_string(), "\"SONY\"");
        assert_eq!(
            e.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
                .unwrap()
                .value
                .get_uint(0),
            Some(1)
        );
        assert_eq!(
            e.get_field(exif::Tag::PhotographicSensitivity, exif::In::PRIMARY)
                .unwrap()
                .value
                .get_uint(0),
            Some(100)
        );
        assert!(
            e.get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY)
                .is_some()
        );
    }

    #[test]
    fn without_metadata() {
        let img = RgbImage16::new(3, 2).unwrap();
        let bytes = encode_tiff16(&img, None, None).unwrap();
        let mut dec = tiff::decoder::Decoder::new(std::io::Cursor::new(&bytes)).unwrap();
        assert!(
            dec.find_tag(Tag::from_u16_exhaustive(TIFF_TAG_ICC_PROFILE))
                .unwrap()
                .is_none()
        );
        assert!(dec.find_tag(Tag::ExifDirectory).unwrap().is_none());
    }
}
