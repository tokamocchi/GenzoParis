//! 8bit / 16bit の RGB 画像（画素ごとに R, G, B を並べたもの。アルファなし）。
//!
//! アルファチャンネルは使わない（04 の 2.6 節「アルファチャンネルは使いません」）。
//! 値の意味（色空間・伝達関数）は画像そのものには持たせず、使う側が ICC プロファイルなどで扱う。

use std::fmt;

use crate::error::{MediaError, Result};

/// 扱う画像の画素数の上限（仮置き）。
///
/// genzo-raw の `MAX_PIXELS`（2 億画素）と同じ値。対象機種（α7 IV で約 3,300 万画素）や
/// 一般的な画像より十分大きく、壊れたファイルの寸法による巨大な確保を防ぐ（SEC-05）。
pub const MAX_IMAGE_PIXELS: u64 = 200_000_000;

mod sealed {
    pub trait Sealed {}
    impl Sealed for u8 {}
    impl Sealed for u16 {}
}

/// 画素の 1 チャンネルの値の型（`u8` または `u16`）。
pub trait Sample:
    Copy + Default + PartialEq + Eq + fmt::Debug + Send + Sync + sealed::Sealed + 'static
{
    /// ビット数。
    const BITS: u8;
    /// 最大値（白）。
    const MAX: u16;

    /// `u32` にする。
    fn to_u32(self) -> u32;

    /// 0〜1 に正規化した値。
    fn to_unit_f32(self) -> f32 {
        self.to_u32() as f32 / f32::from(Self::MAX)
    }

    /// 0〜1 の値を四捨五入して量子化する。範囲外は 0〜1 に収め、NaN は 0 にする。
    fn from_unit_f32(v: f32) -> Self;
}

impl Sample for u8 {
    const BITS: u8 = 8;
    const MAX: u16 = 255;

    fn to_u32(self) -> u32 {
        u32::from(self)
    }

    fn from_unit_f32(v: f32) -> Self {
        // NaN は clamp を通っても NaN だが、`as` による整数への変換で 0 になる。
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    }
}

impl Sample for u16 {
    const BITS: u8 = 16;
    const MAX: u16 = 65535;

    fn to_u32(self) -> u32 {
        u32::from(self)
    }

    fn from_unit_f32(v: f32) -> Self {
        (v.clamp(0.0, 1.0) * 65535.0).round() as u16
    }
}

/// 16bit の値を 8bit にする（四捨五入。`round(v × 255 / 65535)`）。
pub const fn u16_to_u8(v: u16) -> u8 {
    ((v as u32 * 255 + 32767) / 65535) as u8
}

/// 8bit の値を 16bit にする（`v × 257`。0 → 0、255 → 65535）。
pub const fn u8_to_u16(v: u8) -> u16 {
    v as u16 * 257
}

/// 幅と高さを検証し、データの要素数（幅 × 高さ × 3）を返す。
pub(crate) fn checked_len(width: u32, height: u32) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(MediaError::invalid_image(format!(
            "幅と高さは 1 以上が必要（{width}×{height}）"
        )));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_IMAGE_PIXELS {
        return Err(MediaError::TooLarge {
            what: "画素数",
            actual: pixels,
            max: MAX_IMAGE_PIXELS,
        });
    }
    usize::try_from(pixels * 3).map_err(|_| MediaError::TooLarge {
        what: "画素数",
        actual: pixels,
        max: MAX_IMAGE_PIXELS,
    })
}

/// RGB 画像（行優先、画素ごとに R, G, B）。
#[derive(Clone, PartialEq, Eq)]
pub struct RgbImage<T: Sample> {
    width: u32,
    height: u32,
    data: Vec<T>,
}

/// 8bit の RGB 画像。
pub type RgbImage8 = RgbImage<u8>;
/// 16bit の RGB 画像。
pub type RgbImage16 = RgbImage<u16>;

impl<T: Sample> fmt::Debug for RgbImage<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RgbImage")
            .field("bits", &T::BITS)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl<T: Sample> RgbImage<T> {
    /// 黒（すべて 0）の画像を作る。
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let len = checked_len(width, height)?;
        Ok(Self {
            width,
            height,
            data: vec![T::default(); len],
        })
    }

    /// データから作る。データの長さは幅 × 高さ × 3。
    pub fn from_raw(width: u32, height: u32, data: Vec<T>) -> Result<Self> {
        let len = checked_len(width, height)?;
        if data.len() != len {
            return Err(MediaError::invalid_image(format!(
                "データの長さ {} が {width}×{height}×3 = {len} と一致しない",
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// 関数で各画素の値を決めて作る。
    pub fn from_fn(width: u32, height: u32, mut f: impl FnMut(u32, u32) -> [T; 3]) -> Result<Self> {
        let len = checked_len(width, height)?;
        let mut data = Vec::with_capacity(len);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&f(x, y));
            }
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// 幅（画素）。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 高さ（画素）。
    pub fn height(&self) -> u32 {
        self.height
    }

    /// （幅, 高さ）。
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 画素数。
    pub fn pixel_count(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// データ（行優先、画素ごとに R, G, B）。
    pub fn as_raw(&self) -> &[T] {
        &self.data
    }

    /// データ（書き換え用）。
    pub fn as_raw_mut(&mut self) -> &mut [T] {
        &mut self.data
    }

    /// データを取り出す。
    pub fn into_raw(self) -> Vec<T> {
        self.data
    }

    /// 1 行分の要素数（幅 × 3）。
    pub fn row_len(&self) -> usize {
        self.width as usize * 3
    }

    /// 各行（幅 × 3 要素）。
    pub fn rows(&self) -> impl ExactSizeIterator<Item = &[T]> {
        self.data.chunks_exact(self.row_len())
    }

    /// 画素の値。範囲外なら `None`。
    pub fn pixel(&self, x: u32, y: u32) -> Option<[T; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 3;
        Some([self.data[i], self.data[i + 1], self.data[i + 2]])
    }

    /// 画素の値を書き換える。範囲外なら何もせず `false`。
    pub fn set_pixel(&mut self, x: u32, y: u32, value: [T; 3]) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let i = (y as usize * self.width as usize + x as usize) * 3;
        self.data[i..i + 3].copy_from_slice(&value);
        true
    }
}

impl RgbImage8 {
    /// 16bit にする（`v × 257`）。
    pub fn to_rgb16(&self) -> RgbImage16 {
        RgbImage {
            width: self.width,
            height: self.height,
            data: self.data.iter().map(|&v| u8_to_u16(v)).collect(),
        }
    }
}

impl RgbImage16 {
    /// 8bit にする（四捨五入。ディザリングはしない。ディザリングはパイプラインのステージ 17b の役割）。
    pub fn to_rgb8(&self) -> RgbImage8 {
        RgbImage {
            width: self.width,
            height: self.height,
            data: self.data.iter().map(|&v| u16_to_u8(v)).collect(),
        }
    }
}

/// 8bit または 16bit の RGB 画像。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DynRgbImage {
    /// 8bit。
    Rgb8(RgbImage8),
    /// 16bit。
    Rgb16(RgbImage16),
}

impl DynRgbImage {
    /// 幅（画素）。
    pub fn width(&self) -> u32 {
        match self {
            Self::Rgb8(i) => i.width(),
            Self::Rgb16(i) => i.width(),
        }
    }

    /// 高さ（画素）。
    pub fn height(&self) -> u32 {
        match self {
            Self::Rgb8(i) => i.height(),
            Self::Rgb16(i) => i.height(),
        }
    }

    /// （幅, 高さ）。
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width(), self.height())
    }

    /// 1 チャンネルあたりのビット数（8 または 16）。
    pub fn bits_per_channel(&self) -> u8 {
        match self {
            Self::Rgb8(_) => 8,
            Self::Rgb16(_) => 16,
        }
    }

    /// 8bit の画像にする（16bit なら四捨五入で変換する）。
    pub fn to_rgb8(&self) -> RgbImage8 {
        match self {
            Self::Rgb8(i) => i.clone(),
            Self::Rgb16(i) => i.to_rgb8(),
        }
    }

    /// 16bit の画像にする（8bit なら `v × 257`）。
    pub fn to_rgb16(&self) -> RgbImage16 {
        match self {
            Self::Rgb8(i) => i.to_rgb16(),
            Self::Rgb16(i) => i.clone(),
        }
    }
}

impl From<RgbImage8> for DynRgbImage {
    fn from(i: RgbImage8) -> Self {
        Self::Rgb8(i)
    }
}

impl From<RgbImage16> for DynRgbImage {
    fn from(i: RgbImage16) -> Self {
        Self::Rgb16(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_conversion_round_trips_and_rounds() {
        for v in 0..=255u8 {
            assert_eq!(u16_to_u8(u8_to_u16(v)), v);
        }
        assert_eq!(u8_to_u16(255), 65535);
        // 境界: 128.5 / 255 付近。round(v * 255 / 65535) と一致する。
        for v in [
            0u16, 1, 128, 129, 32767, 32768, 32896, 65407, 65408, 65534, 65535,
        ] {
            let expected = (f64::from(v) * 255.0 / 65535.0).round() as u8;
            assert_eq!(u16_to_u8(v), expected, "{v}");
        }
        // すべての 16bit の値で浮動小数点の計算と一致する。
        for v in 0..=u16::MAX {
            let expected = (f64::from(v) * 255.0 / 65535.0).round() as u8;
            assert_eq!(u16_to_u8(v), expected, "{v}");
        }
    }

    #[test]
    fn sample_quantization_clamps_and_handles_nan() {
        assert_eq!(u8::from_unit_f32(-0.5), 0);
        assert_eq!(u8::from_unit_f32(1.5), 255);
        assert_eq!(u8::from_unit_f32(f32::NAN), 0);
        assert_eq!(u8::from_unit_f32(0.5), 128);
        assert_eq!(u16::from_unit_f32(1.0), 65535);
        assert_eq!(u16::from_unit_f32(f32::INFINITY), 65535);
        assert_eq!(u16::from_unit_f32(f32::NEG_INFINITY), 0);
        assert!((255u8.to_unit_f32() - 1.0).abs() < 1e-7);
        assert!((65535u16.to_unit_f32() - 1.0).abs() < 1e-7);
    }

    #[test]
    fn construction_validates_dimensions() {
        assert!(RgbImage8::new(0, 1).is_err());
        assert!(RgbImage8::new(1, 0).is_err());
        assert!(matches!(
            RgbImage8::new(20_000, 20_000),
            Err(MediaError::TooLarge { .. })
        ));
        assert!(RgbImage8::from_raw(2, 2, vec![0; 11]).is_err());
        let img = RgbImage8::from_raw(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(img.pixel(1, 0), Some([4, 5, 6]));
        assert_eq!(img.pixel(2, 0), None);
        assert_eq!(img.rows().len(), 1);
        assert_eq!(img.pixel_count(), 2);
    }

    #[test]
    fn set_pixel_and_from_fn() {
        let mut img = RgbImage16::from_fn(3, 2, |x, y| [x as u16, y as u16, 7]).unwrap();
        assert_eq!(img.pixel(2, 1), Some([2, 1, 7]));
        assert!(img.set_pixel(0, 0, [9, 9, 9]));
        assert!(!img.set_pixel(3, 0, [9, 9, 9]));
        assert_eq!(img.pixel(0, 0), Some([9, 9, 9]));
        let d = DynRgbImage::from(img.clone());
        assert_eq!(d.bits_per_channel(), 16);
        assert_eq!(d.dimensions(), (3, 2));
        assert_eq!(d.to_rgb16(), img);
        assert_eq!(d.to_rgb8().pixel(0, 0), Some([0, 0, 0]));
        let d8 = DynRgbImage::from(img.to_rgb8());
        assert_eq!(d8.to_rgb16().pixel(0, 0), Some([0, 0, 0]));
        assert!(format!("{img:?}").contains("bits: 16"));
    }
}
