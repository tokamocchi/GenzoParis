//! ステージ 12（テクスチャ・明瞭度）・14（シャープ）の共通の部品: 輝度の対数の領域と、ガウスのぼかし
//! （docs/04_architecture.md の 2.1 節・2.3 節・2.7 節）。
//!
//! # 輝度の対数の領域
//!
//! 細部の明暗は、輝度に小さな値を足した対数 `p = log2(max(Y, 0) + LUMA_LOG_OFFSET)` の差（段）で測る
//! （[`luma_log`]）。対数なので、明るい部分と暗い部分の同じ比の明暗が同じ大きさになり、足した値
//! （[`LUMA_LOG_OFFSET`]。シーンの約 0.0039。中間グレーの約 5.5 段下）より暗い部分では差が小さくなる
//! （暗部の雑音を強めない）。変化 `Δ`（段）は [`apply_luma_log_delta`] で画素に当てる:
//!
//! ```text
//! Ŷ  = max(Y, 0) + LUMA_LOG_OFFSET、 k = exp2(Δ)
//! Y' = Y + Ŷ·(k − 1)                    （対数の領域の輝度が Δ 段変わる）
//! RGB' = Y' + k·(RGB − Y)               （色の成分も k 倍。Y ≫ LUMA_LOG_OFFSET なら RGB × k と同じ）
//! ```
//!
//! `Δ = 0` なら値は変わらない（負の輝度（色域外）の画素も保つ）。
//!
//! # ガウスのぼかし（GPU 版と一致させるための決めごと。2.3 節）
//!
//! - 重み（[`GaussKernel`]）は f64 で `exp(−k² / (2σ²))` を求め、和が 1 になるよう割ってから f32 に丸める。
//!   半径は `ceil(3σ)`（1 以上 [`GaussKernel::MAX_RADIUS`] 以下）。σ が 0 に近ければ中心だけが 1。
//! - 横、縦の順に、窓の中を番号の小さい順（`k = −r..=r`）に `w[k + r] · v` を f32 で足す（[`blur_plane`]）。
//!   入力は出力を半径だけ広げた範囲を含むこと（鏡映は使わない。タイルの余白は各ステージの `input_roi`）。
//!   行ごとの計算は分割によらないので、並列化してもスレッド数で結果は変わらない。

use rayon::prelude::*;

use super::luminance;
use crate::error::{PipelineError, Result};
use crate::image::{ImageTile, Roi};

/// 輝度の対数の領域で足す値（シーンリニア。2^−8）。**仮置き**: これより暗い部分では細部の差を小さくして、
/// 暗部の雑音を強めない。PoC-4 で実写を見て決める。
pub const LUMA_LOG_OFFSET: f32 = 1.0 / 256.0;

/// 輝度の対数 `log2(max(Y, 0) + LUMA_LOG_OFFSET)`（Y は BT.2020 の輝度。[`super::luminance`]）。
#[inline]
pub fn luma_log(c: [f32; 3]) -> f32 {
    (luminance(c).max(0.0) + LUMA_LOG_OFFSET).log2()
}

/// 輝度の対数の変化 `delta`（段）を画素に当てる（モジュールの doc の式）。
#[inline]
pub fn apply_luma_log_delta(c: [f32; 3], delta: f32) -> [f32; 3] {
    let y = luminance(c);
    let k = delta.exp2();
    let yh = y.max(0.0) + LUMA_LOG_OFFSET;
    let y2 = y + yh * (k - 1.0);
    [
        y2 + k * (c[0] - y),
        y2 + k * (c[1] - y),
        y2 + k * (c[2] - y),
    ]
}

/// `x / (1 + |x| / limit)`（大きな差を `limit` に向かってなめらかに抑える。符号は保つ。`limit` は正）。
#[inline]
pub fn soft_limit(x: f32, limit: f32) -> f32 {
    x / (1.0 + x.abs() / limit)
}

/// 0〜1 のなめらかな段（`t = clamp((x − e0) / (e1 − e0), 0, 1)`、`t²(3 − 2t)`）。`e1 > e0` であること。
#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// ガウスのぼかしの重み（モジュールの doc）。
#[derive(Debug, Clone, PartialEq)]
pub struct GaussKernel {
    /// 半径（画素）。重みは `2 · radius + 1` 個。
    pub radius: u32,
    /// 重み（`k = −radius..=radius` の順。和は 1（f32 に丸める前））。
    pub weights: Vec<f32>,
}

impl GaussKernel {
    /// 半径の上限（画素）。極端に大きな σ でもタイルの余白が増えすぎないようにする。
    pub const MAX_RADIUS: u32 = 64;

    /// 標準偏差 `sigma`（画素）の重み。NaN・負の値は 0（中心だけ）とみなす。
    pub fn new(sigma: f64) -> Self {
        let sigma = if sigma.is_finite() {
            sigma.max(0.0)
        } else {
            0.0
        };
        let radius = ((3.0 * sigma).ceil() as u32).clamp(1, Self::MAX_RADIUS);
        let r = i64::from(radius);
        let raw: Vec<f64> = (-r..=r)
            .map(|k| {
                if sigma < 1e-6 {
                    if k == 0 { 1.0 } else { 0.0 }
                } else {
                    (-((k * k) as f64) / (2.0 * sigma * sigma)).exp()
                }
            })
            .collect();
        let sum: f64 = raw.iter().sum();
        Self {
            radius,
            weights: raw.iter().map(|w| (w / sum) as f32).collect(),
        }
    }
}

/// タイル `input` の範囲 `roi`（`input.roi` に含まれること）の輝度の対数の面（行優先。[`luma_log`]）。
pub fn luma_log_plane(input: &ImageTile, roi: Roi) -> Result<Vec<f32>> {
    let Some((ox, oy)) = roi.offset_in(&input.roi) else {
        return Err(PipelineError::InvalidRoi {
            roi,
            reason: "入力のタイルが輝度の面の範囲を覆っていない",
        });
    };
    let in_w = input.roi.width as usize;
    let w = roi.width as usize;
    let src = input.image.data();
    let mut out = vec![0.0f32; w * roi.height as usize];
    out.par_chunks_mut(w.max(1))
        .enumerate()
        .for_each(|(j, row)| {
            let start = (oy + j) * in_w + ox;
            for (dst, c) in row.iter_mut().zip(&src[start..start + w]) {
                *dst = luma_log(*c);
            }
        });
    Ok(out)
}

/// 範囲 `roi` の面 `plane`（行優先）を横、縦の順にぼかし、範囲 `out` の値を返す（モジュールの doc）。
/// `roi` は `out` を `kernel.radius` だけ広げた範囲を含むこと。
pub fn blur_plane(plane: &[f32], roi: Roi, out: Roi, kernel: &GaussKernel) -> Result<Vec<f32>> {
    let r = kernel.radius;
    let need = out.expand(r);
    if !roi.contains_roi(&need) || plane.len() != roi.pixel_count() as usize {
        return Err(PipelineError::InvalidRoi {
            roi,
            reason: "ぼかしの入力が必要な範囲（半径の余白）を覆っていない",
        });
    }
    let ri = r as i64;
    let w_in = roi.width as usize;
    // 横方向: 出力の列 × 上下に半径だけ広げた行。
    let h_roi = Roi::new(out.x, out.y - r as i32, out.width, out.height + 2 * r);
    let hw = h_roi.width as usize;
    let mut tmp = vec![0.0f32; hw * h_roi.height as usize];
    tmp.par_chunks_mut(hw.max(1))
        .enumerate()
        .for_each(|(j, row)| {
            let y = (i64::from(h_roi.y) + j as i64 - i64::from(roi.y)) as usize;
            let line = &plane[y * w_in..(y + 1) * w_in];
            for (i, dst) in row.iter_mut().enumerate() {
                let x0 = i64::from(h_roi.x) + i as i64 - i64::from(roi.x) - ri;
                let mut acc = 0.0f32;
                for (k, w) in kernel.weights.iter().enumerate() {
                    acc += w * line[(x0 + k as i64) as usize];
                }
                *dst = acc;
            }
        });
    // 縦方向。
    let ow = out.width as usize;
    let mut result = vec![0.0f32; ow * out.height as usize];
    result
        .par_chunks_mut(ow.max(1))
        .enumerate()
        .for_each(|(j, row)| {
            for (i, dst) in row.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                for (k, w) in kernel.weights.iter().enumerate() {
                    acc += w * tmp[(j + k) * hw + i];
                }
                *dst = acc;
            }
        });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ColorContract;
    use crate::image::RgbImage;

    #[test]
    fn kernel_weights_sum_to_one_and_are_symmetric() {
        for sigma in [0.0, 0.3, 1.0, 2.7, 9.6] {
            let k = GaussKernel::new(sigma);
            assert_eq!(k.weights.len() as u32, 2 * k.radius + 1);
            let s: f32 = k.weights.iter().sum();
            assert!((s - 1.0).abs() < 1e-5, "{sigma}: {s}");
            let n = k.weights.len();
            for i in 0..n / 2 {
                assert_eq!(k.weights[i], k.weights[n - 1 - i]);
            }
        }
        assert_eq!(GaussKernel::new(1.0).radius, 3);
        assert_eq!(GaussKernel::new(0.0).weights, [0.0, 1.0, 0.0]);
        assert_eq!(GaussKernel::new(f64::NAN).weights, [0.0, 1.0, 0.0]);
        assert_eq!(GaussKernel::new(1e9).radius, GaussKernel::MAX_RADIUS);
    }

    #[test]
    fn blur_keeps_constants_and_needs_the_margin() {
        let k = GaussKernel::new(1.5);
        let r = k.radius;
        let out = Roi::new(3, -2, 10, 6);
        let roi = out.expand(r);
        let plane = vec![-2.5f32; roi.pixel_count() as usize];
        let b = blur_plane(&plane, roi, out, &k).unwrap();
        assert_eq!(b.len(), 60);
        assert!(b.iter().all(|v| (v + 2.5).abs() < 1e-5));
        // 余白が足りない。
        let small = out.expand(r - 1);
        let plane = vec![0.0f32; small.pixel_count() as usize];
        assert!(blur_plane(&plane, small, out, &k).is_err());
    }

    #[test]
    fn blur_does_not_depend_on_the_tiling() {
        // 同じ全体の面から、2 つの範囲に分けてぼかしても、まとめてぼかしても同じ値。
        let k = GaussKernel::new(2.0);
        let full = Roi::new(0, 0, 40, 30);
        let plane: Vec<f32> = (0..full.pixel_count())
            .map(|i| ((i * 7919) % 101) as f32 * 0.01)
            .collect();
        let out = Roi::new(8, 8, 24, 14);
        let all = blur_plane(&plane, full, out, &k).unwrap();
        let left = Roi::new(8, 8, 10, 14);
        let part = blur_plane(&plane, full, left, &k).unwrap();
        for y in 0..14usize {
            for x in 0..10usize {
                assert_eq!(part[y * 10 + x], all[y * 24 + x]);
            }
        }
    }

    #[test]
    fn luma_log_delta_scales_ratios_and_keeps_zero_change() {
        let c = [0.3f32, 0.2, 0.1];
        assert_eq!(apply_luma_log_delta(c, 0.0), c);
        let neg = [-0.2f32, 0.05, 0.01];
        assert_eq!(apply_luma_log_delta(neg, 0.0), neg);
        // 明るい画素では RGB の比を保って 2^Δ 倍に近い。
        let b = apply_luma_log_delta([2.0, 1.0, 0.5], 1.0);
        assert!((b[0] / b[1] - 2.0).abs() < 5e-3 && (b[1] / b[2] - 2.0).abs() < 2e-2);
        assert!((luma_log(b) - luma_log([2.0, 1.0, 0.5]) - 1.0).abs() < 1e-5);
        assert!((luma_log([0.18; 3]) - (0.18f32 + LUMA_LOG_OFFSET).log2()).abs() < 1e-6);
        assert_eq!(luma_log([-1.0; 3]), LUMA_LOG_OFFSET.log2());
        assert!((soft_limit(10.0, 1.0) - 10.0 / 11.0).abs() < 1e-6);
        assert_eq!(soft_limit(-0.5, 1.0), -0.5 / 1.5);
        assert_eq!(smoothstep(0.0, 1.0, -1.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 2.0), 1.0);
        assert_eq!(smoothstep(0.0, 1.0, 0.5), 0.5);
        // タイルの面。
        let img = RgbImage::filled(4, 3, ColorContract::B2Working, [0.18; 3]).unwrap();
        let t = ImageTile::from_image(Roi::new(2, 1, 4, 3), img).unwrap();
        let p = luma_log_plane(&t, Roi::new(3, 2, 2, 2)).unwrap();
        assert_eq!(p.len(), 4);
        assert!(luma_log_plane(&t, Roi::new(0, 0, 2, 2)).is_err());
    }
}
