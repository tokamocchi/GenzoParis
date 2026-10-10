//! 色域の圧縮（04 の 2.6 節「3 つの圧縮・変換」の 2。ステージ 17 の最初）。
//!
//! **仮実装**: 方式は PoC-4 で候補を比較して選ぶ（05 の PoC-4 (2)）。それまでの仮の方式として、
//! 次の「輝度に向かって彩度を下げる、ソフトな膝つきの圧縮」を実装する。
//!
//! ## 方式
//!
//! 出力先の色空間のリニア RGB で、色 `c` をその色空間の輝度 `Y`（RGB → XYZ の行列の 2 行目）と、
//! 無彩色からの差 `d = c − (Y, Y, Y)` に分ける。
//!
//! 1. `(Y, Y, Y) + s·d` が 0〜1 の立方体に収まる最大の `s` を `s_max` とし、色域の境界までの
//!    相対的な距離を `t = 1 / s_max`（色域内なら `t ≤ 1`、境界で 1、色域外で 1 超え）とする。
//! 2. `t` が膝 `k`（[`GamutCompressor::threshold`]）以下なら **何もしない**。
//! 3. `t > k` なら、`t' = k + (1 − k)·f((t − k)/(1 − k))`、
//!    `f(x) = x / (1 + x^p)^(1/p)`（p は [`GamutCompressor::power`]）で圧縮し、
//!    `c' = (Y, Y, Y) + (t'/t)·d` とする。`f` は単調増加で、`f(0) = 0`、`f'(0) = 1`（p ≥ 1 のとき
//!    膝でなめらか）、`x → ∞` で 1 に近づくので、どれだけ遠い色域外の色も色域内に収まる。
//!
//! 性質:
//! - **輝度を保つ**（輝度の係数の和が 1 で、`d` の輝度は 0 のため）。
//! - **色相をなるべく保つ**: リニアな RGB で無彩色（同じ輝度のグレー）と混ぜるので、色度図の上では
//!   白色点に向かう直線上を動き、主波長が保たれる（知覚的な色相は Abney 効果で多少ずれる）。
//! - 出力は必ず 0〜1。最後に 0〜1 に収めるのは丸め誤差（1e-7 程度）の吸収のためだけで、
//!   単純な切り捨て（clamp）で色域外の色を処理しているわけではない。
//! - 膝より外側では、色域内の色（境界に近い鮮やかな色）も少し彩度が下がる（膝つきの圧縮の性質）。
//! - 輝度が 0 以下の色は黒 (0, 0, 0)、1 以上の色は白 (1, 1, 1) にする（輝度を 0〜1 に収めるのは
//!   トーンの圧縮（ステージ 15）の役割で、ここに来る輝度は 0〜1 の前提）。
//! - NaN・無限大の成分は 0 とみなす（2.6 節: NaN・無限大は 0 に置き換える）。NaN は出力しない。
//!
//! 計算は f32 で行う（CPU 基準実装と GPU 版で同じ精度にするため。2.3 節）。

use crate::error::{ColorError, Result};
use crate::space::RgbColorSpace;

/// 膝（圧縮を始める、色域の境界までの相対的な距離）の既定値（仮置き）。
///
/// 0.8 は「境界の 8 割より内側の色は変えない」という意味。PoC-4 で画質を比較して決める。
pub const DEFAULT_GAMUT_THRESHOLD: f32 = 0.8;
/// 圧縮の曲線の形（べき乗 p）の既定値（仮置き）。
///
/// p が大きいほど膝が鋭く（色域内の色の変化が小さく）なり、p = 1 が最もなだらか。
/// 1.2 は、膝の位置での変化をなだらかにしつつ、境界（t = 1）の色を t' ≒ 0.91 に保つ値として
/// 選んだ。根拠のある値ではなく、PoC-4 で決める。
pub const DEFAULT_GAMUT_POWER: f32 = 1.2;

/// 色域の圧縮（仮実装）。出力先の色空間のリニア RGB に対して使う。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamutCompressor {
    luminance: [f32; 3],
    threshold: f32,
    power: f32,
}

impl GamutCompressor {
    /// 輝度の係数（出力先の色空間の RGB → XYZ の行列の 2 行目）と、膝・べき乗から作る。
    ///
    /// 係数は和が 1 になるよう正規化する。係数が負・有限でない・和が 0 以下、膝が 0 以上 1 未満で
    /// ない、べき乗が 1 以上の有限の値でないならエラー。
    pub fn new(luminance_coefficients: [f64; 3], threshold: f32, power: f32) -> Result<Self> {
        let sum: f64 = luminance_coefficients.iter().sum();
        if !luminance_coefficients
            .iter()
            .all(|w| w.is_finite() && *w >= 0.0)
            || !(sum > 0.0 && sum.is_finite())
        {
            return Err(ColorError::InvalidArgument {
                reason: "輝度の係数は 0 以上の有限の値で、和が正である必要がある",
            });
        }
        if !(threshold.is_finite() && (0.0..1.0).contains(&threshold)) {
            return Err(ColorError::InvalidArgument {
                reason: "色域の圧縮の膝は 0 以上 1 未満である必要がある",
            });
        }
        if !(power.is_finite() && power >= 1.0) {
            return Err(ColorError::InvalidArgument {
                reason: "色域の圧縮のべき乗は 1 以上の有限の値である必要がある",
            });
        }
        let l = luminance_coefficients.map(|w| (w / sum) as f32);
        Ok(Self {
            luminance: l,
            threshold,
            power,
        })
    }

    /// 標準の色空間（出力先）用。膝とべき乗は既定値（仮置き）。
    pub fn for_space(space: RgbColorSpace) -> Self {
        Self {
            luminance: space.luminance_coefficients().map(|w| w as f32),
            threshold: DEFAULT_GAMUT_THRESHOLD,
            power: DEFAULT_GAMUT_POWER,
        }
    }

    /// 膝とべき乗を変えたものを返す（検証は [`GamutCompressor::new`] と同じ）。
    pub fn with_params(self, threshold: f32, power: f32) -> Result<Self> {
        Self::new(self.luminance.map(f64::from), threshold, power)
    }

    /// 輝度の係数（和が 1）。
    pub fn luminance_coefficients(&self) -> [f32; 3] {
        self.luminance
    }

    /// 膝。
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// べき乗。
    pub fn power(&self) -> f32 {
        self.power
    }

    /// 1 画素を圧縮する（リニア RGB → 0〜1 のリニア RGB）。
    #[inline]
    pub fn compress(&self, rgb: [f32; 3]) -> [f32; 3] {
        let c = rgb.map(|v| if v.is_finite() { v } else { 0.0 });
        let w = self.luminance;
        let y = w[0] * c[0] + w[1] * c[1] + w[2] * c[2];
        if y.is_nan() || y <= 0.0 {
            return [0.0; 3];
        }
        if y >= 1.0 {
            return [1.0; 3];
        }
        let d = [c[0] - y, c[1] - y, c[2] - y];
        // 境界までの相対的な距離 t = 1 / s_max = max(d_i / (1 − Y)（d_i > 0）, −d_i / Y（d_i < 0）)。
        let mut t = 0.0_f32;
        for di in d {
            let ti = if di > 0.0 {
                di / (1.0 - y)
            } else if di < 0.0 {
                -di / y
            } else {
                0.0
            };
            t = t.max(ti);
        }
        if t <= self.threshold {
            return c;
        }
        let k = self.threshold;
        let x = (t - k) / (1.0 - k);
        // f(x) = x / (1 + x^p)^(1/p)。x が大きいときは 1 / (x^(−p) + 1)^(1/p) で計算する
        // （x^p のあふれを避ける）。
        let p = self.power;
        let f = if x <= 1.0 {
            x / (1.0 + x.powf(p)).powf(1.0 / p)
        } else {
            1.0 / (x.powf(-p) + 1.0).powf(1.0 / p)
        };
        let t_new = k + (1.0 - k) * f;
        let scale = if t.is_finite() { t_new / t } else { 0.0 };
        [
            (y + scale * d[0]).clamp(0.0, 1.0),
            (y + scale * d[1]).clamp(0.0, 1.0),
            (y + scale * d[2]).clamp(0.0, 1.0),
        ]
    }

    /// 画素の列をその場で圧縮する。
    pub fn compress_in_place(&self, pixels: &mut [[f32; 3]]) {
        for p in pixels {
            *p = self.compress(*p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::working_to;
    use proptest::prelude::*;

    fn lum(c: &GamutCompressor, rgb: [f32; 3]) -> f32 {
        let w = c.luminance_coefficients();
        w[0] * rgb[0] + w[1] * rgb[1] + w[2] * rgb[2]
    }

    fn in_unit_cube(rgb: [f32; 3]) -> bool {
        rgb.iter().all(|v| (0.0..=1.0).contains(v))
    }

    #[test]
    fn colors_inside_knee_are_unchanged() {
        let c = GamutCompressor::for_space(RgbColorSpace::Srgb);
        for rgb in [
            [0.5, 0.5, 0.5],
            [0.0, 0.0, 0.0],
            [0.2, 0.25, 0.3],
            [0.6, 0.5, 0.45],
            [0.18, 0.18, 0.18],
        ] {
            assert_eq!(c.compress(rgb), rgb);
        }
    }

    #[test]
    fn out_of_gamut_examples_are_brought_inside() {
        let c = GamutCompressor::for_space(RgbColorSpace::Srgb);
        // BT.2020 の原色を sRGB にすると色域外（負の値・1 超え）になる。
        let m = working_to(RgbColorSpace::Srgb);
        for prim in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.5, 0.5],
        ] {
            let v = m.apply(prim);
            let rgb = [v[0] as f32, v[1] as f32, v[2] as f32];
            // 輝度が 1 を超えるものはスケールしてから（ステージ 15 の後を想定）。
            let y = lum(&c, rgb);
            let rgb = if y > 0.9 {
                rgb.map(|v| v * 0.5 / y)
            } else {
                rgb
            };
            assert!(!in_unit_cube(rgb), "{rgb:?} は色域外のはず");
            let out = c.compress(rgb);
            assert!(in_unit_cube(out), "{rgb:?} → {out:?}");
            assert!(
                (lum(&c, out) - lum(&c, rgb)).abs() < 1e-5,
                "{rgb:?} → {out:?}"
            );
        }
    }

    #[test]
    fn hue_direction_is_preserved() {
        // 出力の無彩色からの差は、入力の差と同じ向き（正の倍数）。
        let c = GamutCompressor::for_space(RgbColorSpace::DisplayP3);
        let rgb = [1.4_f32, 0.2, -0.3];
        let out = c.compress(rgb);
        let y = lum(&c, rgb);
        let d_in = rgb.map(|v| v - y);
        let d_out = out.map(|v| v - y);
        let ratio = d_out[0] / d_in[0];
        assert!(ratio > 0.0 && ratio < 1.0);
        for k in 0..3 {
            assert!((d_out[k] - ratio * d_in[k]).abs() < 1e-5, "{out:?}");
        }
    }

    #[test]
    fn monotonic_in_saturation() {
        // 同じ輝度・同じ色相で彩度を上げていくと、出力の彩度も単調に増える（追い越さない）。
        let c = GamutCompressor::for_space(RgbColorSpace::Srgb);
        let y = 0.4_f32;
        let dir = [0.6_f32, -0.2, -0.4];
        let w = c.luminance_coefficients();
        let dl = w[0] * dir[0] + w[1] * dir[1] + w[2] * dir[2];
        let dir = dir.map(|v| v - dl); // 輝度 0 の向きにする
        let mut prev = -1.0_f32;
        for i in 0..400 {
            let s = i as f32 * 0.02;
            let rgb = [y + s * dir[0], y + s * dir[1], y + s * dir[2]];
            let out = c.compress(rgb);
            let sat = out[0] - y;
            assert!(sat >= prev - 1e-6, "s = {s}: {sat} < {prev}");
            prev = sat;
        }
    }

    #[test]
    fn extreme_luminance_and_non_finite() {
        let c = GamutCompressor::for_space(RgbColorSpace::Srgb);
        assert_eq!(c.compress([2.0, 2.0, 2.0]), [1.0; 3]);
        assert_eq!(c.compress([-0.1, -0.1, -0.1]), [0.0; 3]);
        // 輝度 0.2126·5 − 0.7152·3 + 0.0722·9 < 0 なので黒。
        assert_eq!(c.compress([5.0, -3.0, 9.0]), [0.0; 3]);
        // 輝度 > 1 なので白。
        assert_eq!(c.compress([5.0, 3.0, -9.0]), [1.0; 3]);
        assert_eq!(c.compress([f32::NAN; 3]), [0.0; 3]);
        let out = c.compress([f32::NAN, 0.5, f32::INFINITY]);
        assert!(in_unit_cube(out) && out.iter().all(|v| v.is_finite()));
        let out = c.compress([f32::MAX, f32::MIN, 0.0]);
        assert!(in_unit_cube(out));
        // 輝度がごく小さく、彩度が非常に大きい（t が巨大）。
        let out = c.compress([1e30, -1e30 * 0.2126 / 0.7152, 1e-6]);
        assert!(in_unit_cube(out), "{out:?}");
    }

    #[test]
    fn knee_and_power_parameters() {
        let base = GamutCompressor::for_space(RgbColorSpace::Srgb);
        // 境界上の色（sRGB の赤）: t = 1 は t' = k + (1 − k)·2^(−1/p) になる。
        let red = [1.0_f32, 0.0, 0.0];
        let y = lum(&base, red);
        let out = base.compress(red);
        let t_new = (out[0] - y) / (1.0 - y);
        let k = DEFAULT_GAMUT_THRESHOLD;
        let p = DEFAULT_GAMUT_POWER;
        let expected = k + (1.0 - k) * 2.0_f32.powf(-1.0 / p);
        assert!((t_new - expected).abs() < 1e-5, "{t_new} vs {expected}");
        // べき乗を大きくすると、境界の色の変化は小さくなる。
        let sharp = base.with_params(0.8, 8.0).unwrap();
        let out_sharp = sharp.compress(red);
        assert!(out_sharp[0] > out[0]);
        // 膝 0: すべての有彩色が圧縮される。
        let all = base.with_params(0.0, 1.0).unwrap();
        assert_ne!(all.compress([0.5, 0.4, 0.4]), [0.5, 0.4, 0.4]);
    }

    #[test]
    fn invalid_parameters() {
        let l = RgbColorSpace::Srgb.luminance_coefficients();
        assert!(GamutCompressor::new(l, 1.0, 1.2).is_err());
        assert!(GamutCompressor::new(l, -0.1, 1.2).is_err());
        assert!(GamutCompressor::new(l, f32::NAN, 1.2).is_err());
        assert!(GamutCompressor::new(l, 0.8, 0.5).is_err());
        assert!(GamutCompressor::new(l, 0.8, f32::INFINITY).is_err());
        assert!(GamutCompressor::new([0.0, 0.0, 0.0], 0.8, 1.2).is_err());
        assert!(GamutCompressor::new([0.5, -0.1, 0.6], 0.8, 1.2).is_err());
        assert!(GamutCompressor::new([0.5, f64::NAN, 0.6], 0.8, 1.2).is_err());
        // 和が 1 でない係数は正規化される。
        let c = GamutCompressor::new([2.0, 6.0, 2.0], 0.8, 1.2).unwrap();
        assert_eq!(c.luminance_coefficients(), [0.2, 0.6, 0.2]);
    }

    #[test]
    fn in_place_matches_single() {
        let c = GamutCompressor::for_space(RgbColorSpace::AdobeRgb1998);
        let src = [[1.2_f32, 0.1, -0.2], [0.3, 0.3, 0.3], [0.0, 0.9, 0.2]];
        let mut buf = src;
        c.compress_in_place(&mut buf);
        for (a, b) in src.iter().zip(buf.iter()) {
            assert_eq!(c.compress(*a), *b);
        }
    }

    proptest! {
        #[test]
        fn output_always_in_unit_cube_and_finite(
            r in -10.0_f32..10.0, g in -10.0_f32..10.0, b in -10.0_f32..10.0,
            space in 0usize..4,
        ) {
            let c = GamutCompressor::for_space(RgbColorSpace::ALL[space]);
            let out = c.compress([r, g, b]);
            prop_assert!(out.iter().all(|v| v.is_finite()));
            prop_assert!(in_unit_cube(out), "{:?} → {:?}", [r, g, b], out);
        }

        #[test]
        fn luminance_is_preserved_for_y_in_range(
            y in 0.001_f32..0.999,
            dr in -3.0_f32..3.0, dg in -3.0_f32..3.0, db in -3.0_f32..3.0,
            space in 0usize..4,
        ) {
            let c = GamutCompressor::for_space(RgbColorSpace::ALL[space]);
            let w = c.luminance_coefficients();
            let dl = w[0] * dr + w[1] * dg + w[2] * db;
            let rgb = [y + dr - dl, y + dg - dl, y + db - dl];
            let y_in = lum(&c, rgb);
            prop_assume!(y_in > 0.0 && y_in < 1.0);
            let out = c.compress(rgb);
            prop_assert!((lum(&c, out) - y_in).abs() < 2e-5, "{:?} → {:?}", rgb, out);
        }

        #[test]
        fn in_gamut_inside_knee_is_identity(
            r in 0.0_f32..=1.0, g in 0.0_f32..=1.0, b in 0.0_f32..=1.0,
        ) {
            let c = GamutCompressor::for_space(RgbColorSpace::Srgb);
            let rgb = [r, g, b];
            let y = lum(&c, rgb);
            prop_assume!(y > 0.0 && y < 1.0);
            let t = rgb
                .iter()
                .map(|&v| if v > y { (v - y) / (1.0 - y) } else { (y - v) / y })
                .fold(0.0_f32, f32::max);
            prop_assume!(t <= DEFAULT_GAMUT_THRESHOLD * 0.999);
            prop_assert_eq!(c.compress(rgb), rgb);
        }
    }
}
