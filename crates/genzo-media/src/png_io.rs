//! PNG の書き出し（8bit / 16bit。EXP-01）。ICC プロファイルは iCCP チャンク、Exif は eXIf チャンク
//! （PNG 第 3 版（W3C）で規定されたチャンク。版の発行年などは原典で確認すること）に入れる。
//!
//! iCCP があるときは sRGB・gAMA・cHRM のチャンクを書かない（PNG の規定で、iCCP と sRGB は同時に
//! 置かない）。

use std::io::Write;

use crate::buffer::{RgbImage, Sample};
use crate::error::{MediaError, Result};

/// PNG の書き出しの圧縮の設定（仮置き）。
///
/// png crate の既定（zlib の標準のレベルに相当）。書き出しの時間（PERF-10）と大きさの釣り合いを
/// PoC で見て見直す。
const PNG_COMPRESSION: png::Compression = png::Compression::Balanced;

/// RGB 画像を PNG にして `w` に書く。ビット数は画像の型（8bit / 16bit）に合わせる。
pub fn encode_png_to<T: Sample, W: Write>(
    img: &RgbImage<T>,
    icc: Option<&[u8]>,
    exif_tiff: Option<&[u8]>,
    w: W,
) -> Result<()> {
    let (width, height) = img.dimensions();
    let mut info = png::Info::with_size(width, height);
    info.color_type = png::ColorType::Rgb;
    info.bit_depth = match T::BITS {
        8 => png::BitDepth::Eight,
        _ => png::BitDepth::Sixteen,
    };
    info.icc_profile = icc.map(|b| b.to_vec().into());
    info.exif_metadata = exif_tiff.map(|b| b.to_vec().into());
    let mut encoder =
        png::Encoder::with_info(w, info).map_err(|e| MediaError::encode(format!("PNG: {e}")))?;
    encoder.set_compression(PNG_COMPRESSION);
    let mut writer = encoder
        .write_header()
        .map_err(|e| MediaError::encode(format!("PNG: {e}")))?;
    // PNG の 16bit はビッグエンディアン。
    let bytes: Vec<u8> = match T::BITS {
        8 => img.as_raw().iter().map(|v| v.to_u32() as u8).collect(),
        _ => img
            .as_raw()
            .iter()
            .flat_map(|v| (v.to_u32() as u16).to_be_bytes())
            .collect(),
    };
    writer
        .write_image_data(&bytes)
        .map_err(|e| MediaError::encode(format!("PNG: {e}")))?;
    writer
        .finish()
        .map_err(|e| MediaError::encode(format!("PNG: {e}")))
}

/// RGB 画像を PNG のバイト列にする。
pub fn encode_png<T: Sample>(
    img: &RgbImage<T>,
    icc: Option<&[u8]>,
    exif_tiff: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    encode_png_to(img, icc, exif_tiff, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{RgbImage8, RgbImage16};

    fn chunk_types(png_bytes: &[u8]) -> Vec<[u8; 4]> {
        let mut pos = 8;
        let mut out = Vec::new();
        while pos + 8 <= png_bytes.len() {
            let len = u32::from_be_bytes(png_bytes[pos..pos + 4].try_into().unwrap()) as usize;
            out.push(png_bytes[pos + 4..pos + 8].try_into().unwrap());
            pos += 12 + len;
        }
        out
    }

    #[test]
    fn round_trip_8_and_16_bit_with_metadata() {
        let icc = vec![7u8; 300];
        let exif = b"MM\0\x2a\0\0\0\x08\0\0\0\0\0\0".to_vec();
        let img8 = RgbImage8::from_fn(5, 3, |x, y| [x as u8 * 50, y as u8 * 100, 7]).unwrap();
        let bytes = encode_png(&img8, Some(&icc), Some(&exif)).unwrap();
        let types = chunk_types(&bytes);
        let idat = types.iter().position(|t| t == b"IDAT").unwrap();
        let iccp = types.iter().position(|t| t == b"iCCP").unwrap();
        let exif_pos = types.iter().position(|t| t == b"eXIf").unwrap();
        assert!(iccp < idat && exif_pos < idat, "{types:?}");
        assert!(!types.iter().any(|t| t == b"sRGB"));

        let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
        let mut reader = decoder.read_info().unwrap();
        assert_eq!(reader.info().icc_profile.as_deref(), Some(icc.as_slice()));
        assert_eq!(
            reader.info().exif_metadata.as_deref(),
            Some(exif.as_slice())
        );
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut buf).unwrap();
        assert_eq!(frame.bit_depth, png::BitDepth::Eight);
        assert_eq!(&buf[..frame.buffer_size()], img8.as_raw());

        let img16 =
            RgbImage16::from_fn(4, 2, |x, y| [x as u16 * 16000, y as u16 * 65535, 1]).unwrap();
        let bytes = encode_png(&img16, None, None).unwrap();
        assert!(
            !chunk_types(&bytes)
                .iter()
                .any(|t| t == b"iCCP" || t == b"eXIf")
        );
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgb16();
        assert_eq!(decoded.as_raw(), img16.as_raw());
    }
}
