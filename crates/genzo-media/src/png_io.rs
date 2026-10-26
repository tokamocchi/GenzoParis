//! PNG の書き出し（8bit / 16bit。EXP-01）。ICC プロファイルは iCCP チャンク、Exif は eXIf チャンク
//! （PNG 第 3 版（W3C）で規定されたチャンク。版の発行年などは原典で確認すること）に入れる。説明の
//! メタデータ（LIB-15）の XMP は、キーワード `XML:com.adobe.xmp` の圧縮しない iTXt チャンクに入れる
//! （XMP の仕様書の第 3 部。[`encode_png_with_xmp_to`]）。
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
    encode_png_with_xmp_to(img, icc, exif_tiff, None, w)
}

/// RGB 画像を PNG にして `w` に書く（`xmp` があれば iTXt チャンクにも入れる）。
pub fn encode_png_with_xmp_to<T: Sample, W: Write>(
    img: &RgbImage<T>,
    icc: Option<&[u8]>,
    exif_tiff: Option<&[u8]>,
    xmp: Option<&str>,
    w: W,
) -> Result<()> {
    encode_png_with_metadata_to(
        img,
        &PngMetadata {
            icc,
            exif_tiff,
            xmp,
            resolution_ppi: None,
        },
        w,
    )
}

/// PNG に入れるメタデータ（[`encode_png_with_metadata_to`]）。
#[derive(Debug, Clone, Copy, Default)]
pub struct PngMetadata<'a> {
    /// ICC プロファイル（iCCP）。
    pub icc: Option<&'a [u8]>,
    /// Exif の TIFF の構造（eXIf）。
    pub exif_tiff: Option<&'a [u8]>,
    /// XMP のパケット（iTXt）。
    pub xmp: Option<&'a str>,
    /// 解像度（ppi。pHYs に 1 メートルあたりの画素の数（四捨五入）で書く。EXP-03）。`None` なら pHYs を書かない。
    pub resolution_ppi: Option<u32>,
}

/// RGB 画像を、メタデータ（[`PngMetadata`]）と一緒に PNG にして `w` に書く。
pub fn encode_png_with_metadata_to<T: Sample, W: Write>(
    img: &RgbImage<T>,
    meta: &PngMetadata<'_>,
    w: W,
) -> Result<()> {
    let PngMetadata {
        icc,
        exif_tiff,
        xmp,
        resolution_ppi,
    } = *meta;
    let (width, height) = img.dimensions();
    let mut info = png::Info::with_size(width, height);
    info.color_type = png::ColorType::Rgb;
    info.bit_depth = match T::BITS {
        8 => png::BitDepth::Eight,
        _ => png::BitDepth::Sixteen,
    };
    info.icc_profile = icc.map(|b| b.to_vec().into());
    info.exif_metadata = exif_tiff.map(|b| b.to_vec().into());
    if let Some(ppi) = resolution_ppi {
        // 1 インチ = 0.0254 メートル。
        let ppm = (f64::from(ppi.max(1)) / 0.0254).round() as u32;
        info.pixel_dims = Some(png::PixelDimensions {
            xppu: ppm,
            yppu: ppm,
            unit: png::Unit::Meter,
        });
    }
    let mut encoder =
        png::Encoder::with_info(w, info).map_err(|e| MediaError::encode(format!("PNG: {e}")))?;
    if let Some(x) = xmp {
        // ITXtChunk::new は圧縮しない（png crate の text_metadata）。
        encoder
            .add_itxt_chunk(crate::xmp_write::XMP_PNG_KEYWORD.to_owned(), x.to_owned())
            .map_err(|e| MediaError::encode(format!("PNG: {e}")))?;
    }
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

/// 8 ビットのグレースケールの画像（`width × height`。行優先）を PNG のバイト列にする（AI マスクのラスターの保存。
/// DEV-20。色の情報は付けない）。長さが合わなければエラー。
pub fn encode_gray8_png(width: u32, height: u32, values: &[u8]) -> Result<Vec<u8>> {
    if width == 0 || height == 0 || values.len() != width as usize * height as usize {
        return Err(MediaError::encode(format!(
            "PNG（グレースケール）: 値の長さ {} が {width} × {height} に合わない",
            values.len()
        )));
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(PNG_COMPRESSION);
        let mut writer = encoder
            .write_header()
            .map_err(|e| MediaError::encode(format!("PNG: {e}")))?;
        writer
            .write_image_data(values)
            .map_err(|e| MediaError::encode(format!("PNG: {e}")))?;
        writer
            .finish()
            .map_err(|e| MediaError::encode(format!("PNG: {e}")))?;
    }
    Ok(out)
}

/// [`encode_gray8_png`] で作った PNG を読む（幅・高さ・値）。8 ビットのグレースケール以外はエラー。
pub fn decode_gray8_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|e| MediaError::decode(format!("PNG: {e}")))?;
    let info = reader.info();
    if info.color_type != png::ColorType::Grayscale || info.bit_depth != png::BitDepth::Eight {
        return Err(MediaError::decode(format!(
            "PNG: 8 ビットのグレースケールではない（{:?}, {:?}）",
            info.color_type, info.bit_depth
        )));
    }
    let (width, height) = (info.width, info.height);
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(0)];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| MediaError::decode(format!("PNG: {e}")))?;
    buf.truncate(frame.buffer_size());
    if buf.len() != width as usize * height as usize {
        return Err(MediaError::decode("PNG: 画素の数が合わない".to_owned()));
    }
    Ok((width, height, buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{RgbImage8, RgbImage16};

    #[test]
    fn gray8_png_round_trips() {
        let values: Vec<u8> = (0..24u8).map(|i| i.wrapping_mul(11)).collect();
        let png = encode_gray8_png(6, 4, &values).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(decode_gray8_png(&png).unwrap(), (6, 4, values.clone()));
        assert!(encode_gray8_png(5, 4, &values).is_err());
        assert!(decode_gray8_png(b"not a png").is_err());
        // RGB の PNG は読まない。
        let rgb = encode_png(&RgbImage8::new(2, 1).unwrap(), None, None).unwrap();
        assert!(decode_gray8_png(&rgb).is_err());
    }

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
    fn xmp_is_written_as_an_uncompressed_itxt_chunk() {
        let img8 = RgbImage8::from_fn(3, 2, |x, _| [x as u8, 0, 0]).unwrap();
        let xmp = "<x:xmpmeta>清水寺</x:xmpmeta>";
        let mut bytes = Vec::new();
        encode_png_with_xmp_to(&img8, None, None, Some(xmp), &mut bytes).unwrap();
        assert!(chunk_types(&bytes).iter().any(|t| t == b"iTXt"));
        let reader = png::Decoder::new(std::io::Cursor::new(&bytes))
            .read_info()
            .unwrap();
        let chunk = reader
            .info()
            .utf8_text
            .iter()
            .find(|c| c.keyword == crate::xmp_write::XMP_PNG_KEYWORD)
            .expect("XMP の iTXt");
        assert!(!chunk.compressed);
        assert_eq!(chunk.get_text().unwrap(), xmp);
        // xmp なしなら iTXt を書かない。
        let plain = encode_png(&img8, None, None).unwrap();
        assert!(!chunk_types(&plain).iter().any(|t| t == b"iTXt"));
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
