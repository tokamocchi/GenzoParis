//! 浮動小数点の画像（回帰テストと比較の単位）。
//!
//! パイプラインの各ステージの出力（04 の 2.6 節の B0〜B4b）を、圧縮せずにそのまま比べるための
//! 入れ物。チャンネル数は 1（CFA・マスク）、3（RGB）、4（GPU の RGBA のバッファ）に対応する。
//! 値の意味（リニアか符号化済みか、どの色空間か）は持たず、比べる側（[`crate::diff`]）が指定する。

use std::fmt;

/// 画像の画素数の上限。
///
/// 壊れた基準画像のヘッダなどによる巨大な確保を防ぐ。RAW の上限（[`genzo_raw::MAX_PIXELS`]）と
/// 同じ値にする（RAW を展開した結果より大きい画像は扱わない）。
pub const MAX_IMAGE_PIXELS: u64 = genzo_raw::MAX_PIXELS;

/// 画像の扱いのエラー。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ImageError {
    /// 幅か高さが 0。
    #[error("画像の寸法が不正です（{width} × {height}）")]
    InvalidDimensions {
        /// 幅。
        width: u32,
        /// 高さ。
        height: u32,
    },
    /// 対応していないチャンネル数。
    #[error("対応していないチャンネル数です（{0}。1・3・4 のどれか）")]
    UnsupportedChannels(u32),
    /// 画素数が上限を超えた。
    #[error("画像の画素数 {pixels} が上限 {max} を超えています")]
    TooManyPixels {
        /// 画素数。
        pixels: u64,
        /// 上限。
        max: u64,
    },
    /// データの長さが寸法と合わない。
    #[error("画像のデータの長さ {actual} が寸法から求めた {expected} と一致しません")]
    DataLengthMismatch {
        /// 寸法から求めた長さ（値の個数）。
        expected: u64,
        /// 実際の長さ。
        actual: usize,
    },
}

/// 有限でない値の位置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NonFiniteSample {
    /// x。
    pub x: u32,
    /// y。
    pub y: u32,
    /// チャンネル。
    pub channel: u32,
    /// 値（NaN・無限大）。
    pub value: f32,
}

impl fmt::Display for NonFiniteSample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "({}, {}) のチャンネル {} が {}",
            self.x, self.y, self.channel, self.value
        )
    }
}

/// 浮動小数点（f32）の画像。値は行優先・画素ごとにチャンネルを並べる（RGBRGB…）。
#[derive(Debug, Clone, PartialEq)]
pub struct FloatImage {
    width: u32,
    height: u32,
    channels: u32,
    data: Vec<f32>,
}

/// 寸法とチャンネル数を検証し、値の個数を返す。
fn checked_len(width: u32, height: u32, channels: u32) -> Result<usize, ImageError> {
    if width == 0 || height == 0 {
        return Err(ImageError::InvalidDimensions { width, height });
    }
    if !matches!(channels, 1 | 3 | 4) {
        return Err(ImageError::UnsupportedChannels(channels));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_IMAGE_PIXELS {
        return Err(ImageError::TooManyPixels {
            pixels,
            max: MAX_IMAGE_PIXELS,
        });
    }
    // 上限の検査の後なので、u64 の積は桁あふれしない。usize に入らない環境（32bit）ではエラー。
    let len = pixels * u64::from(channels);
    usize::try_from(len).map_err(|_| ImageError::TooManyPixels {
        pixels,
        max: MAX_IMAGE_PIXELS,
    })
}

impl FloatImage {
    /// 寸法とチャンネル数を検証し、値の個数（幅 × 高さ × チャンネル数）を返す（確保はしない）。
    pub fn value_count(width: u32, height: u32, channels: u32) -> Result<usize, ImageError> {
        checked_len(width, height, channels)
    }

    /// 値の列から作る。
    pub fn new(width: u32, height: u32, channels: u32, data: Vec<f32>) -> Result<Self, ImageError> {
        let len = checked_len(width, height, channels)?;
        if data.len() != len {
            return Err(ImageError::DataLengthMismatch {
                expected: len as u64,
                actual: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            channels,
            data,
        })
    }

    /// すべての画素を `value` で埋めた画像を作る（`value` の長さがチャンネル数）。
    pub fn filled(width: u32, height: u32, value: &[f32]) -> Result<Self, ImageError> {
        let channels = u32::try_from(value.len()).unwrap_or(u32::MAX);
        let len = checked_len(width, height, channels)?;
        let mut data = Vec::with_capacity(len);
        while data.len() < len {
            data.extend_from_slice(value);
        }
        Self::new(width, height, channels, data)
    }

    /// 画素 (x, y) ごとに関数で RGB の値を決めて 3 チャンネルの画像を作る。
    pub fn from_rgb_fn(
        width: u32,
        height: u32,
        mut f: impl FnMut(u32, u32) -> [f32; 3],
    ) -> Result<Self, ImageError> {
        let len = checked_len(width, height, 3)?;
        let mut data = Vec::with_capacity(len);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&f(x, y));
            }
        }
        Self::new(width, height, 3, data)
    }

    /// 画素 (x, y) ごとに関数で値を決めて 1 チャンネルの画像を作る。
    pub fn from_gray_fn(
        width: u32,
        height: u32,
        mut f: impl FnMut(u32, u32) -> f32,
    ) -> Result<Self, ImageError> {
        let len = checked_len(width, height, 1)?;
        let mut data = Vec::with_capacity(len);
        for y in 0..height {
            for x in 0..width {
                data.push(f(x, y));
            }
        }
        Self::new(width, height, 1, data)
    }

    /// 幅。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 高さ。
    pub fn height(&self) -> u32 {
        self.height
    }

    /// チャンネル数（1・3・4）。
    pub fn channels(&self) -> u32 {
        self.channels
    }

    /// 画素数。
    pub fn pixel_count(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// 寸法とチャンネル数が同じか。
    pub fn same_shape(&self, other: &FloatImage) -> bool {
        self.width == other.width && self.height == other.height && self.channels == other.channels
    }

    /// 値の列（行優先、画素ごとにチャンネルを並べる）。
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// 値の列（書き換え用）。
    pub fn data_mut(&mut self) -> &mut [f32] {
        &mut self.data
    }

    /// 値の列を取り出す。
    pub fn into_data(self) -> Vec<f32> {
        self.data
    }

    /// 画素 (x, y) の値（チャンネル数の長さ）。範囲外なら `None`。
    pub fn pixel(&self, x: u32, y: u32) -> Option<&[f32]> {
        let i = self.index(x, y)?;
        self.data.get(i..i + self.channels as usize)
    }

    /// 画素 (x, y) の値（書き換え用）。範囲外なら `None`。
    pub fn pixel_mut(&mut self, x: u32, y: u32) -> Option<&mut [f32]> {
        let i = self.index(x, y)?;
        let c = self.channels as usize;
        self.data.get_mut(i..i + c)
    }

    fn index(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some((y as usize * self.width as usize + x as usize) * self.channels as usize)
    }

    /// すべての値に関数を適用した画像（例: 伝達関数で符号化する）。
    pub fn map(&self, mut f: impl FnMut(f32) -> f32) -> FloatImage {
        FloatImage {
            width: self.width,
            height: self.height,
            channels: self.channels,
            data: self.data.iter().map(|&v| f(v)).collect(),
        }
    }

    /// 3 チャンネルの画像の各画素に関数を適用した画像（例: 行列で色空間を変換する）。
    /// 3 チャンネルでなければ `None`。
    pub fn map_rgb(&self, mut f: impl FnMut([f32; 3]) -> [f32; 3]) -> Option<FloatImage> {
        if self.channels != 3 {
            return None;
        }
        let data = self
            .data
            .chunks_exact(3)
            .flat_map(|p| f([p[0], p[1], p[2]]))
            .collect();
        Some(FloatImage {
            width: self.width,
            height: self.height,
            channels: 3,
            data,
        })
    }

    /// 最初に見つかった有限でない値（NaN・無限大）。すべて有限なら `None`。
    pub fn find_non_finite(&self) -> Option<NonFiniteSample> {
        let c = self.channels as usize;
        let w = self.width as usize;
        self.data
            .iter()
            .position(|v| !v.is_finite())
            .map(|i| NonFiniteSample {
                x: ((i / c) % w) as u32,
                y: ((i / c) / w) as u32,
                channel: (i % c) as u32,
                value: self.data[i],
            })
    }

    /// 矩形の範囲の、チャンネルごとの平均（f64）。範囲が画像の外にはみ出す・空なら `None`。
    ///
    /// カラーチャートの各パッチの中央部分の平均（PoC-4 の測定方法 ⑤）などに使う。
    pub fn region_mean(&self, rect: Rect) -> Option<Vec<f64>> {
        if rect.width == 0
            || rect.height == 0
            || u64::from(rect.x) + u64::from(rect.width) > u64::from(self.width)
            || u64::from(rect.y) + u64::from(rect.height) > u64::from(self.height)
        {
            return None;
        }
        let c = self.channels as usize;
        let mut sum = vec![0.0_f64; c];
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                let p = self.pixel(x, y)?;
                for (s, &v) in sum.iter_mut().zip(p) {
                    *s += f64::from(v);
                }
            }
        }
        let n = f64::from(rect.width) * f64::from(rect.height);
        Some(sum.into_iter().map(|s| s / n).collect())
    }
}

/// 画像の中の矩形（画素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// 左上の x。
    pub x: u32,
    /// 左上の y。
    pub y: u32,
    /// 幅。
    pub width: u32,
    /// 高さ。
    pub height: u32,
}

impl Rect {
    /// 作る。
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// 四辺から `margin` 画素ずつ内側に縮めた矩形。縮めると空になる場合は `None`。
    pub fn inset(&self, margin: u32) -> Option<Rect> {
        let m2 = margin.checked_mul(2)?;
        if self.width <= m2 || self.height <= m2 {
            return None;
        }
        Some(Rect::new(
            self.x + margin,
            self.y + margin,
            self.width - m2,
            self.height - m2,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_validates_shape() {
        assert!(FloatImage::new(2, 2, 3, vec![0.0; 12]).is_ok());
        assert_eq!(
            FloatImage::new(0, 2, 3, vec![]),
            Err(ImageError::InvalidDimensions {
                width: 0,
                height: 2
            })
        );
        assert_eq!(
            FloatImage::new(2, 2, 2, vec![0.0; 8]),
            Err(ImageError::UnsupportedChannels(2))
        );
        assert_eq!(
            FloatImage::new(2, 2, 3, vec![0.0; 11]),
            Err(ImageError::DataLengthMismatch {
                expected: 12,
                actual: 11
            })
        );
        assert!(matches!(
            FloatImage::new(u32::MAX, u32::MAX, 1, vec![]),
            Err(ImageError::TooManyPixels { .. })
        ));
    }

    #[test]
    fn pixel_access_and_layout() {
        let img = FloatImage::from_rgb_fn(3, 2, |x, y| [x as f32, y as f32, 9.0]).unwrap();
        assert_eq!(img.pixel(2, 1), Some(&[2.0, 1.0, 9.0][..]));
        assert_eq!(img.pixel(3, 0), None);
        assert_eq!(img.pixel(0, 2), None);
        // 行優先・画素ごとにチャンネル。
        assert_eq!(&img.data()[3..6], &[1.0, 0.0, 9.0]);
        let mut img = img;
        img.pixel_mut(1, 1).unwrap()[2] = -1.0;
        assert_eq!(img.pixel(1, 1), Some(&[1.0, 1.0, -1.0][..]));
    }

    #[test]
    fn filled_and_gray() {
        let img = FloatImage::filled(2, 3, &[0.1, 0.2, 0.3, 1.0]).unwrap();
        assert_eq!(img.channels(), 4);
        assert_eq!(img.data().len(), 24);
        assert_eq!(img.pixel(1, 2), Some(&[0.1, 0.2, 0.3, 1.0][..]));
        let g = FloatImage::from_gray_fn(4, 1, |x, _| x as f32).unwrap();
        assert_eq!(g.data(), &[0.0, 1.0, 2.0, 3.0]);
        assert!(FloatImage::filled(2, 2, &[0.0, 0.0]).is_err());
    }

    #[test]
    fn find_non_finite_reports_location() {
        let mut img = FloatImage::filled(4, 3, &[0.5, 0.5, 0.5]).unwrap();
        assert_eq!(img.find_non_finite(), None);
        img.pixel_mut(2, 1).unwrap()[1] = f32::NAN;
        img.pixel_mut(3, 2).unwrap()[0] = f32::INFINITY;
        let s = img.find_non_finite().unwrap();
        assert_eq!((s.x, s.y, s.channel), (2, 1, 1));
        assert!(s.value.is_nan());
    }

    #[test]
    fn region_mean_and_inset() {
        let img = FloatImage::from_rgb_fn(4, 4, |x, y| [x as f32, y as f32, 1.0]).unwrap();
        let m = img.region_mean(Rect::new(1, 1, 2, 2)).unwrap();
        assert_eq!(m, vec![1.5, 1.5, 1.0]);
        assert_eq!(img.region_mean(Rect::new(3, 3, 2, 1)), None);
        assert_eq!(img.region_mean(Rect::new(0, 0, 0, 1)), None);
        assert_eq!(Rect::new(0, 0, 10, 8).inset(2), Some(Rect::new(2, 2, 6, 4)));
        assert_eq!(Rect::new(0, 0, 10, 4).inset(2), None);
    }

    #[test]
    fn map_and_map_rgb() {
        let img = FloatImage::filled(2, 1, &[1.0, 2.0, 3.0]).unwrap();
        assert_eq!(img.map(|v| v * 2.0).data(), &[2.0, 4.0, 6.0, 2.0, 4.0, 6.0]);
        let swapped = img.map_rgb(|[r, g, b]| [b, g, r]).unwrap();
        assert_eq!(swapped.pixel(0, 0), Some(&[3.0, 2.0, 1.0][..]));
        let gray = FloatImage::filled(1, 1, &[0.5]).unwrap();
        assert!(gray.map_rgb(|p| p).is_none());
    }
}
