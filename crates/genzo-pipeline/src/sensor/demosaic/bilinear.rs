//! 双線形補間のデモザイク（比較用。RCD の品質の確認に使う）。
//!
//! 各画素で、足りない色を 3 × 3 の近傍の同じ色の画素の平均で補う:
//!
//! | 中心 | R | G | B |
//! |---|---|---|---|
//! | R | 中心 | `((N + S) + (W + E)) × 0.25` | `((NW + NE) + (SW + SE)) × 0.25` |
//! | B | `((NW + NE) + (SW + SE)) × 0.25` | `((N + S) + (W + E)) × 0.25` | 中心 |
//! | G1（R の行） | `(W + E) × 0.5` | 中心 | `(N + S) × 0.5` |
//! | G2（B の行） | `(N + S) × 0.5` | 中心 | `(W + E) × 0.5` |
//!
//! 足し算は 2 つずつ組にしてから足す（平らな画像で丸めの誤差が出ないように。`v + v = 2v` は正確）。

use genzo_raw::CfaColor;
use rayon::prelude::*;

use crate::image::{CfaImage, Roi};

/// 双線形補間に必要な余白（画素）。
pub const BILINEAR_MARGIN: u32 = 1;

/// `cfa`（`out` を 1 画素広げた範囲を含む）から、範囲 `out` の RGB を作る。
pub(crate) fn bilinear(cfa: &CfaImage, out: Roi) -> Vec<[f32; 3]> {
    let (ox, oy) = out
        .offset_in(&cfa.roi())
        .expect("呼び出し側で範囲を確認済み");
    let iw = cfa.width() as usize;
    let c = cfa.data();
    let pattern = cfa.pattern();
    let ow = out.width as usize;
    let mut result = vec![[0.0f32; 3]; out.pixel_count() as usize];
    result.par_chunks_mut(ow).enumerate().for_each(|(ly, row)| {
        let y = oy + ly;
        for (lx, dst) in row.iter_mut().enumerate() {
            let x = ox + lx;
            let i = y * iw + x;
            let v = c[i];
            let cross = || ((c[i - iw] + c[i + iw]) + (c[i - 1] + c[i + 1])) * 0.25;
            let diag =
                || ((c[i - iw - 1] + c[i - iw + 1]) + (c[i + iw - 1] + c[i + iw + 1])) * 0.25;
            let horiz = || (c[i - 1] + c[i + 1]) * 0.5;
            let vert = || (c[i - iw] + c[i + iw]) * 0.5;
            *dst = match pattern.color_at(x, y) {
                CfaColor::Red => [v, cross(), diag()],
                CfaColor::Blue => [diag(), cross(), v],
                CfaColor::Green1 => [horiz(), v, vert()],
                CfaColor::Green2 => [vert(), v, horiz()],
            };
        }
    });
    result
}

#[cfg(test)]
mod tests {
    use genzo_raw::CfaPattern;

    use super::*;
    use crate::sensor::demosaic::{DemosaicMethod, demosaic, demosaic_full};

    #[test]
    fn flat_colors_are_exact_for_every_pattern() {
        let rgb = [0.7f32, 0.3, 0.123_456_7];
        for p in CfaPattern::ALL {
            let cfa = CfaImage::from_fn(Roi::full(9, 7), p, |x, y| {
                rgb[p.color_at(x as usize, y as usize).rgb_channel()]
            })
            .unwrap();
            let out = demosaic_full(&cfa, DemosaicMethod::Bilinear).unwrap();
            assert!(out.image.data().iter().all(|&px| px == rgb), "{p}");
        }
    }

    #[test]
    fn hand_computed_values() {
        // RGGB、値は 10y + x。中心 (2, 2) は R、(3, 2) は G1、(2, 3) は G2、(3, 3) は B。
        let cfa = CfaImage::from_fn(Roi::full(6, 6), CfaPattern::RGGB, |x, y| {
            (10 * y + x) as f32
        })
        .unwrap();
        let out = demosaic(&cfa, DemosaicMethod::Bilinear, Roi::new(2, 2, 2, 2)).unwrap();
        // R: G = (12 + 32 + 21 + 23) / 4 = 22、B = (11 + 13 + 31 + 33) / 4 = 22。
        assert_eq!(out.at(2, 2), Some([22.0, 22.0, 22.0]));
        // G1 (3, 2): R = (22 + 24) / 2 = 23、B = (13 + 33) / 2 = 23。
        assert_eq!(out.at(3, 2), Some([23.0, 23.0, 23.0]));
        // G2 (2, 3): R = (22 + 42) / 2 = 32、B = (31 + 33) / 2 = 32。
        assert_eq!(out.at(2, 3), Some([32.0, 32.0, 32.0]));
        // B (3, 3): R = (22 + 24 + 42 + 44) / 4 = 33、G = (23 + 43 + 32 + 34) / 4 = 33。
        assert_eq!(out.at(3, 3), Some([33.0, 33.0, 33.0]));
    }
}
