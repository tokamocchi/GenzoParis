//! 明示的な補間（docs/04_architecture.md の 2.3 節「GPU のハードウェア補間に頼らず、自前で補間する」）。
//!
//! - [`Lut1d`]: 1D LUT と線形補間。トーンカーブ（ステージ 16）などのカーブは、この表（既定 4096 点）で
//!   評価する。GPU 版にも同じ表（[`Lut1d::values`]）を渡し、同じ式で補間する。
//! - [`sample_bilinear_gray`]・[`sample_bilinear_rgb`]: 双線形の拡大（ガイドの参照・ジオメトリ）。
//!   画像の端の外は鏡映（[`crate::border`]）。
//! - 3D LUT（ステージ 17a）は genzo-color の [`genzo_color::Lut3d`]（四面体補間）を使う。
//!
//! どの式も f32 で、演算の順序を doc のとおりに固定する（融合積和演算は使わない）。

use crate::border::mirror_index;
use crate::error::{PipelineError, Result};
use crate::image::{GrayImage, RgbImage};

/// 1D LUT の既定の点数（仮置き: 2.3 節の例の値。カーブの精度と GPU のメモリの釣り合いは PoC-3 で確認する）。
pub const DEFAULT_LUT1D_SIZE: usize = 4096;

/// 1D LUT（等間隔の標本と線形補間）。
///
/// 定義域 `[lo, hi]` を `n − 1` 等分した点 `x_i = lo + i × (hi − lo) / (n − 1)` の値を持つ。
/// 評価（[`eval`](Self::eval)）の式:
///
/// 1. `t = (x − lo) × inv_step`（`inv_step = (n − 1) / (hi − lo)` を f64 で計算して f32 に丸めた値）
/// 2. `t` を `[0, n − 1]` に切り詰める（NaN は 0）。定義域の外は端の値になる
/// 3. `i = min(⌊t⌋, n − 2)`、`f = t − i`
/// 4. `v[i] + (v[i + 1] − v[i]) × f`
#[derive(Debug, Clone, PartialEq)]
pub struct Lut1d {
    lo: f32,
    hi: f32,
    inv_step: f32,
    values: Vec<f32>,
}

impl Lut1d {
    /// 値の列から作る。点数は 2 以上、`lo < hi`、値はすべて有限であること。
    pub fn from_values(lo: f32, hi: f32, values: Vec<f32>) -> Result<Self> {
        if values.len() < 2 {
            return Err(PipelineError::InvalidArgument(format!(
                "1D LUT の点数は 2 以上が必要です（{}）",
                values.len()
            )));
        }
        if !(lo.is_finite() && hi.is_finite() && lo < hi) {
            return Err(PipelineError::InvalidArgument(format!(
                "1D LUT の定義域が不正です（{lo}〜{hi}）"
            )));
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Err(PipelineError::InvalidArgument(
                "1D LUT に有限でない値があります".to_owned(),
            ));
        }
        let inv_step = ((values.len() - 1) as f64 / (f64::from(hi) - f64::from(lo))) as f32;
        Ok(Self {
            lo,
            hi,
            inv_step,
            values,
        })
    }

    /// 関数 `f`（f64 で計算する）を `n` 点で標本化して作る。
    pub fn from_fn(n: usize, lo: f32, hi: f32, f: impl Fn(f64) -> f64) -> Result<Self> {
        if n < 2 {
            return Err(PipelineError::InvalidArgument(format!(
                "1D LUT の点数は 2 以上が必要です（{n}）"
            )));
        }
        let (l, h) = (f64::from(lo), f64::from(hi));
        let values = (0..n)
            .map(|i| f(l + (h - l) * i as f64 / (n - 1) as f64) as f32)
            .collect();
        Self::from_values(lo, hi, values)
    }

    /// 定義域の下端。
    pub fn lo(&self) -> f32 {
        self.lo
    }

    /// 定義域の上端。
    pub fn hi(&self) -> f32 {
        self.hi
    }

    /// `(n − 1) / (hi − lo)`（f32。GPU 版にも同じ値を渡す）。
    pub fn inv_step(&self) -> f32 {
        self.inv_step
    }

    /// 標本の値（GPU 版にも同じ表を渡す）。
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// `x` での値（型の doc の式）。
    #[inline]
    pub fn eval(&self, x: f32) -> f32 {
        let last = (self.values.len() - 1) as f32;
        let t = (x - self.lo) * self.inv_step;
        // NaN は比較が偽になるので 0 にする。
        let t = if t >= 0.0 { t.min(last) } else { 0.0 };
        let i = (t as usize).min(self.values.len() - 2);
        let f = t - i as f32;
        let a = self.values[i];
        let b = self.values[i + 1];
        a + (b - a) * f
    }
}

/// 双線形補間の 4 点と重み（端の外は鏡映）。座標は連続座標（画素の中心が +0.5）。
#[inline]
fn bilinear_taps(width: u32, height: u32, x: f32, y: f32) -> ([usize; 4], f32, f32) {
    // NaN・無限大の座標は 0 とみなす（パニックや NaN の伝播を避ける）。
    let fx = if x.is_finite() { x - 0.5 } else { -0.5 };
    let fy = if y.is_finite() { y - 0.5 } else { -0.5 };
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    // 極端な座標で x0 + 1 があふれないよう、鏡映の周期より十分大きい範囲に切り詰める
    // （f32 で 2^40 を超える値は整数で、tx は 0 になる）。
    const LIMIT: f32 = (1u64 << 40) as f32;
    let x0 = x0.clamp(-LIMIT, LIMIT) as i64;
    let y0 = y0.clamp(-LIMIT, LIMIT) as i64;
    let xa = mirror_index(x0, width) as usize;
    let xb = mirror_index(x0 + 1, width) as usize;
    let ya = mirror_index(y0, height) as usize;
    let yb = mirror_index(y0 + 1, height) as usize;
    let w = width as usize;
    ([ya * w + xa, ya * w + xb, yb * w + xa, yb * w + xb], tx, ty)
}

/// 1 チャンネルの画像を連続座標 (x, y)（画素の中心が +0.5）で双線形補間する。端の外は鏡映。
///
/// 式: `fx = x − 0.5`、`x0 = ⌊fx⌋`、`tx = fx − x0`（y も同様）。4 点 a=(x0,y0)・b=(x0+1,y0)・
/// c=(x0,y0+1)・d=(x0+1,y0+1) の値から `top = a + (b − a) × tx`、`bottom = c + (d − c) × tx`、
/// 結果は `top + (bottom − top) × ty`。
pub fn sample_bilinear_gray(image: &GrayImage, x: f32, y: f32) -> f32 {
    let (idx, tx, ty) = bilinear_taps(image.width(), image.height(), x, y);
    let d = image.data();
    let top = d[idx[0]] + (d[idx[1]] - d[idx[0]]) * tx;
    let bottom = d[idx[2]] + (d[idx[3]] - d[idx[2]]) * tx;
    top + (bottom - top) * ty
}

/// RGB の画像を連続座標 (x, y) で双線形補間する（チャンネルごとに [`sample_bilinear_gray`] と
/// 同じ式）。端の外は鏡映。
pub fn sample_bilinear_rgb(image: &RgbImage, x: f32, y: f32) -> [f32; 3] {
    let (idx, tx, ty) = bilinear_taps(image.width(), image.height(), x, y);
    let d = image.data();
    std::array::from_fn(|k| {
        let top = d[idx[0]][k] + (d[idx[1]][k] - d[idx[0]][k]) * tx;
        let bottom = d[idx[2]][k] + (d[idx[3]][k] - d[idx[2]][k]) * tx;
        top + (bottom - top) * ty
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ColorContract;

    #[test]
    fn lut_reproduces_samples_and_interpolates_linearly() {
        let lut = Lut1d::from_fn(5, 0.0, 1.0, |x| x * x).unwrap();
        assert_eq!(lut.values(), &[0.0, 0.0625, 0.25, 0.5625, 1.0]);
        assert_eq!(lut.inv_step(), 4.0);
        assert_eq!(lut.eval(0.0), 0.0);
        assert_eq!(lut.eval(0.5), 0.25);
        assert_eq!(lut.eval(1.0), 1.0);
        // 0.375 は 0.25 と 0.5 の中点: (0.0625 + 0.25) / 2。
        assert!((lut.eval(0.375) - 0.15625).abs() < 1e-7);
        // 定義域の外は端の値。NaN は下端。
        assert_eq!(lut.eval(-3.0), 0.0);
        assert_eq!(lut.eval(7.0), 1.0);
        assert_eq!(lut.eval(f32::NAN), 0.0);
        assert_eq!(lut.eval(f32::INFINITY), 1.0);
    }

    #[test]
    fn identity_lut_is_close_to_identity() {
        let lut = Lut1d::from_fn(DEFAULT_LUT1D_SIZE, 0.0, 1.0, |x| x).unwrap();
        for i in 0..=1000 {
            let x = i as f32 / 1000.0;
            assert!((lut.eval(x) - x).abs() < 1e-6, "{x}");
        }
        assert_eq!((lut.lo(), lut.hi()), (0.0, 1.0));
    }

    #[test]
    fn lut_rejects_bad_input() {
        assert!(Lut1d::from_values(0.0, 1.0, vec![1.0]).is_err());
        assert!(Lut1d::from_values(1.0, 1.0, vec![0.0, 1.0]).is_err());
        assert!(Lut1d::from_values(0.0, f32::NAN, vec![0.0, 1.0]).is_err());
        assert!(Lut1d::from_values(0.0, 1.0, vec![0.0, f32::NAN]).is_err());
        assert!(Lut1d::from_fn(1, 0.0, 1.0, |x| x).is_err());
    }

    #[test]
    fn bilinear_hits_pixel_centers_and_mirrors_outside() {
        let g = GrayImage::from_fn(3, 2, |x, y| (10 * y + x) as f32).unwrap();
        // 画素の中心ではその画素の値。
        assert_eq!(sample_bilinear_gray(&g, 0.5, 0.5), 0.0);
        assert_eq!(sample_bilinear_gray(&g, 2.5, 1.5), 12.0);
        // 中間。
        assert_eq!(sample_bilinear_gray(&g, 1.0, 0.5), 0.5);
        assert_eq!(sample_bilinear_gray(&g, 1.5, 1.0), 6.0);
        // 端の外は鏡映: x = −0.5 は画素 −1（= 画素 1）の中心。
        assert_eq!(sample_bilinear_gray(&g, -0.5, 0.5), 1.0);
        assert_eq!(sample_bilinear_gray(&g, 3.5, 0.5), 1.0);
        // RGB も同じ式。
        let rgb = RgbImage::from_fn(3, 2, ColorContract::B2Working, |x, y| {
            [(10 * y + x) as f32, 1.0, -(x as f32)]
        })
        .unwrap();
        assert_eq!(sample_bilinear_rgb(&rgb, 1.5, 1.0), [6.0, 1.0, -1.0]);
        // 極端な座標でもパニックしない。
        assert!(sample_bilinear_gray(&g, f32::NAN, 1e30).is_finite());
    }
}
