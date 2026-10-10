//! 色域の圧縮（04 の 2.6 節「3 つの圧縮・変換」の 2。ステージ 17 の最初）。
//!
//! **方式（人の判断の表 No.3 の決定）**: 色域内の色は変えず、色域外の色だけを、輝度を保ったまま
//! 無彩色（同じ輝度のグレー）に向かって彩度を下げ、色域の境界に収める。
//!
//! ## 方式
//!
//! 出力先の色空間のリニア RGB で、色 `c` をその色空間の輝度 `Y`（RGB → XYZ の行列の 2 行目）と、
//! 無彩色からの差 `d = c − (Y, Y, Y)` に分ける。
//!
//! 1. `(Y, Y, Y) + s·d` が 0〜1 の立方体に収まる最大の `s` を `s_max` とし、色域の境界までの
//!    相対的な距離を `t = 1 / s_max`（色域内なら `t ≤ 1`、境界で 1、色域外で 1 超え）とする:
//!    `t = max_i( d_i / (1 − Y)（d_i > 0）, −d_i / Y（d_i < 0） )`。
//! 2. `t ≤ 1`（色域内。すべてのチャンネルが 0〜1）なら **何もしない**（入力をそのまま返す）。
//! 3. `t > 1`（色域外）なら `c' = (Y, Y, Y) + d / t` とする（境界の上の点。どれかのチャンネルが
//!    0 か 1 になる）。
//!
//! 性質:
//! - **色域内の色は変えない**（ビット単位で同じ）。以前の仮実装（膝 0.8・べき乗 1.2 のソフトな圧縮）では
//!   境界に近い鮮やかな色（原色を含む）も彩度が下がっていた。
//! - **輝度を保つ**（輝度の係数の和が 1 で、`d` の輝度は 0 のため）。
//! - **色相をなるべく保つ**: リニアな RGB で無彩色と混ぜるので、色度図の上では白色点に向かう直線上を
//!   動き、主波長が保たれる（知覚的な色相は Abney 効果で多少ずれる）。
//! - 色域外の色は境界に重なる（彩度の違う色域外の色が、境界で同じ色になる）。境界で値は連続だが、
//!   傾きは連続でない（ソフトな圧縮ではない）。
//! - 出力は必ず 0〜1。最後に 0〜1 に収めるのは丸め誤差（1e-7 程度）の吸収のためだけで、
//!   単純な切り捨て（clamp）で色域外の色を処理しているわけではない。
//! - 輝度が 0 以下の色は黒 (0, 0, 0)、1 以上の色は白 (1, 1, 1) にする（輝度を 0〜1 に収めるのは
//!   トーンの圧縮（ステージ 15）の役割で、ここに来る輝度は 0〜1 の前提）。
//! - NaN・無限大の成分は 0 とみなす（2.6 節: NaN・無限大は 0 に置き換える）。NaN は出力しない。
//!
//! 計算は f32 で行う（CPU 基準実装と GPU 版で同じ式・同じ順序にするため。2.3 節）。

use crate::error::{ColorError, Result};
use crate::space::RgbColorSpace;

/// 色域の圧縮。出力先の色空間のリニア RGB に対して使う（モジュールの doc の方式）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamutCompressor {
    luminance: [f32; 3],
}

impl GamutCompressor {
    /// 輝度の係数（出力先の色空間の RGB → XYZ の行列の 2 行目）から作る。
    ///
    /// 係数は和が 1 になるよう正規化する。係数が負・有限でない・和が 0 以下ならエラー。
    pub fn new(luminance_coefficients: [f64; 3]) -> Result<Self> {
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
        let l = luminance_coefficients.map(|w| (w / sum) as f32);
        Ok(Self { luminance: l })
    }

    /// 標準の色空間（出力先）用。
    pub fn for_space(space: RgbColorSpace) -> Self {
        Self {
            luminance: space.luminance_coefficients().map(|w| w as f32),
        }
    }

    /// 輝度の係数（和が 1）。
    pub fn luminance_coefficients(&self) -> [f32; 3] {
        self.luminance
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
        if t <= 1.0 {
            return c;
        }
        let scale = if t.is_finite() { 1.0 / t } else { 0.0 };
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

    /// どれかのチャンネルが 0 か 1（色域の境界の上）か。
    fn on_boundary(rgb: [f32; 3]) -> bool {
        rgb.iter().any(|&v| !(1e-5..=1.0 - 1e-5).contains(&v))
    }

    #[test]
    fn in_gamut_colors_are_unchanged() {
        // 境界の上の色（原色・二次色）も含め、色域内の色は変えない（人の判断 No.3）。
        let c = GamutCompressor::for_space(RgbColorSpace::Srgb);
        for rgb in [
            [0.5, 0.5, 0.5],
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.2, 0.25, 0.3],
            [0.6, 0.5, 0.45],
            [0.18, 0.18, 0.18],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
            [0.95, 0.02, 0.4],
        ] {
            assert_eq!(c.compress(rgb), rgb);
        }
    }

    #[test]
    fn out_of_gamut_examples_are_brought_to_the_boundary() {
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
            assert!(on_boundary(out), "{rgb:?} → {out:?} は境界の上");
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
        // 同じ輝度・同じ色相で彩度を上げていくと、出力の彩度は境界まで入力どおりに増え、境界で止まる
        // （追い越さない）。
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
            if in_unit_cube(rgb) {
                assert_eq!(out, rgb);
            }
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
    fn invalid_parameters() {
        assert!(GamutCompressor::new([0.0, 0.0, 0.0]).is_err());
        assert!(GamutCompressor::new([0.5, -0.1, 0.6]).is_err());
        assert!(GamutCompressor::new([0.5, f64::NAN, 0.6]).is_err());
        // 和が 1 でない係数は正規化される。
        let c = GamutCompressor::new([2.0, 6.0, 2.0]).unwrap();
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
        fn luminance_is_preserved_and_out_of_gamut_lands_on_the_boundary(
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
            if !in_unit_cube(rgb) {
                prop_assert!(on_boundary(out), "{:?} → {:?}", rgb, out);
            }
        }

        #[test]
        fn in_gamut_is_identity(
            r in 0.0_f32..=1.0, g in 0.0_f32..=1.0, b in 0.0_f32..=1.0,
            space in 0usize..4,
        ) {
            let c = GamutCompressor::for_space(RgbColorSpace::ALL[space]);
            let rgb = [r, g, b];
            let y = lum(&c, rgb);
            prop_assume!(y > 0.0 && y < 1.0);
            prop_assert_eq!(c.compress(rgb), rgb);
        }
    }
}
