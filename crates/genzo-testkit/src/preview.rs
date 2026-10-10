//! 目で確かめるための PNG（基準画像の比較に失敗したときの出力）。
//!
//! どれも **確認用** で、比較には使わない（比較は浮動小数点のまま [`crate::golden`] の形式で行う）。

use std::io::Cursor;

use image::{DynamicImage, ImageBuffer, ImageFormat, Luma, Rgb, Rgba};

use crate::image::FloatImage;

/// 16bit の PNG のバイト列にする。値は 0〜1 に切り詰めて 65535 倍し、四捨五入する
/// （伝達関数はかけない。NaN・無限大は 0）。
pub fn encode_png16(image: &FloatImage) -> Result<Vec<u8>, image::ImageError> {
    let to16 = |v: f32| -> u16 {
        if v.is_finite() {
            // 0〜1 に切り詰めた後なので、u16 の範囲に収まる。
            (v.clamp(0.0, 1.0) * 65535.0).round() as u16
        } else {
            0
        }
    };
    let data: Vec<u16> = image.data().iter().map(|&v| to16(v)).collect();
    let (w, h) = (image.width(), image.height());
    let dynamic = match image.channels() {
        1 => ImageBuffer::<Luma<u16>, _>::from_raw(w, h, data).map(DynamicImage::ImageLuma16),
        3 => ImageBuffer::<Rgb<u16>, _>::from_raw(w, h, data).map(DynamicImage::ImageRgb16),
        _ => ImageBuffer::<Rgba<u16>, _>::from_raw(w, h, data).map(DynamicImage::ImageRgba16),
    }
    .expect("FloatImage の長さは寸法とチャンネル数に一致する");
    encode(&dynamic)
}

/// 差分画像（8bit の RGB の PNG）のバイト列にする。
///
/// `map` は画素ごとの差（8bit 換算、または ΔE2000）。`limit`（画素ごとの許容差）以下の画素は
/// 差に比例した灰色（許容差でちょうど中間の灰色 127、差がなければ黒）、超えた画素は赤 (255, 0, 0)。
pub fn encode_diff_png(
    map: &[f64],
    width: u32,
    height: u32,
    limit: f64,
) -> Result<Vec<u8>, image::ImageError> {
    let mut data = Vec::with_capacity(map.len() * 3);
    for &d in map {
        data.extend_from_slice(&diff_color(d, limit));
    }
    let buf = ImageBuffer::<Rgb<u8>, _>::from_raw(width, height, data).ok_or_else(|| {
        image::ImageError::Parameter(image::error::ParameterError::from_kind(
            image::error::ParameterErrorKind::DimensionMismatch,
        ))
    })?;
    encode(&DynamicImage::ImageRgb8(buf))
}

/// 差分画像の 1 画素の色。
pub(crate) fn diff_color(d: f64, limit: f64) -> [u8; 3] {
    if d.is_nan() || d > limit {
        return [255, 0, 0];
    }
    if limit <= 0.0 {
        // limit = 0 で d ≤ 0（差なし）。
        return [0, 0, 0];
    }
    let g = (d / limit * 127.0).round().clamp(0.0, 127.0) as u8;
    [g, g, g]
}

fn encode(image: &DynamicImage) -> Result<Vec<u8>, image::ImageError> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png16_roundtrip_through_decoder() {
        let img =
            FloatImage::from_rgb_fn(3, 2, |x, y| [x as f32 / 2.0, y as f32, f32::NAN]).unwrap();
        let png = encode_png16(&img).unwrap();
        let back = image::load_from_memory_with_format(&png, ImageFormat::Png)
            .unwrap()
            .into_rgb16();
        assert_eq!(back.dimensions(), (3, 2));
        assert_eq!(back.get_pixel(0, 0).0, [0, 0, 0]);
        assert_eq!(back.get_pixel(1, 0).0, [32768, 0, 0]);
        assert_eq!(back.get_pixel(2, 1).0, [65535, 65535, 0]);
        for channels in [1u32, 4] {
            let v = vec![2.0_f32; channels as usize];
            let img = FloatImage::filled(2, 2, &v).unwrap();
            let png = encode_png16(&img).unwrap();
            let back = image::load_from_memory(&png).unwrap();
            assert_eq!(u32::from(back.color().channel_count()), channels);
        }
    }

    #[test]
    fn diff_colors() {
        assert_eq!(diff_color(0.0, 1.0), [0, 0, 0]);
        assert_eq!(diff_color(1.0, 1.0), [127, 127, 127]);
        assert_eq!(diff_color(0.5, 1.0), [64, 64, 64]);
        assert_eq!(diff_color(1.0001, 1.0), [255, 0, 0]);
        assert_eq!(diff_color(f64::NAN, 1.0), [255, 0, 0]);
        assert_eq!(diff_color(0.0, 0.0), [0, 0, 0]);
        assert_eq!(diff_color(0.1, 0.0), [255, 0, 0]);
        let png = encode_diff_png(&[0.0, 2.0], 2, 1, 1.0).unwrap();
        let back = image::load_from_memory(&png).unwrap().into_rgb8();
        assert_eq!(back.get_pixel(1, 0).0, [255, 0, 0]);
        assert!(encode_diff_png(&[0.0], 2, 1, 1.0).is_err());
    }
}
