//! 2 × 2 の画素をまとめる簡易処理（docs/04_architecture.md の 2.2 節「WB を変更したとき」。WB の
//! ドラッグ中に、デモザイクの代わりに使う）。
//!
//! 出力の画素 (bx, by) は S の 2 × 2 の画素 `[2bx, 2bx + 2) × [2by, 2by + 2)`（S の偶数の座標から
//! 始まるブロック）から作る。ブロックには R・G1・B・G2 が 1 つずつ入る（どの CFA の並びでも同じ）:
//!
//! ```text
//! R = R、 G = (G1 + G2) × 0.5、 B = B
//! ```
//!
//! 出力の格子は S の半分の解像度（幅・高さは切り捨て。S の幅・高さが奇数なら最後の列・行は使わない）。
//! 出力の画素の中心は S の `(2bx + 1, 2by + 1)`。R・G・B の標本の位置は 1 画素ずれているが、
//! 簡易処理なので補正しない（指を離した後の最終品質の描画で置き換わる。2.2 節）。

use genzo_raw::CfaColor;
use rayon::prelude::*;

use crate::image::{CfaImage, Roi};

/// `cfa`（`out.doubled()` を含む）から、半分の解像度の範囲 `out` の RGB を作る。
pub(crate) fn half(cfa: &CfaImage, out: Roi) -> Vec<[f32; 3]> {
    let need = out.doubled();
    let (ox, oy) = need
        .offset_in(&cfa.roi())
        .expect("呼び出し側で範囲を確認済み");
    let iw = cfa.width() as usize;
    let c = cfa.data();
    let pattern = cfa.pattern();
    let ow = out.width as usize;
    let mut result = vec![[0.0f32; 3]; out.pixel_count() as usize];
    result.par_chunks_mut(ow).enumerate().for_each(|(by, row)| {
        let y = oy + 2 * by;
        for (bx, dst) in row.iter_mut().enumerate() {
            let x = ox + 2 * bx;
            let mut v = [0.0f32; 4];
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                v[pattern.color_at(x + dx, y + dy).index()] = c[(y + dy) * iw + x + dx];
            }
            *dst = [
                v[CfaColor::Red.index()],
                (v[CfaColor::Green1.index()] + v[CfaColor::Green2.index()]) * 0.5,
                v[CfaColor::Blue.index()],
            ];
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
    fn dimensions_are_halved_and_rounded_down() {
        for (w, h) in [(8, 6), (9, 7), (2, 2)] {
            let cfa = CfaImage::from_fn(Roi::full(w, h), CfaPattern::RGGB, |_, _| 0.25).unwrap();
            let out = demosaic_full(&cfa, DemosaicMethod::Half2x2).unwrap();
            assert_eq!(out.image.dimensions(), (w / 2, h / 2));
            assert_eq!(out.roi, Roi::full(w / 2, h / 2));
        }
    }

    #[test]
    fn values_are_r_mean_green_and_b() {
        // 各位置の色ごとに違う値: R = 0.9、G1 = 0.2、B = 0.5、G2 = 0.4 → (0.9, 0.3, 0.5)。
        let by_index = [0.9f32, 0.2, 0.5, 0.4];
        for p in CfaPattern::ALL {
            let cfa = CfaImage::from_fn(Roi::full(6, 4), p, |x, y| {
                by_index[p.color_at(x as usize, y as usize).index()]
            })
            .unwrap();
            let out = demosaic_full(&cfa, DemosaicMethod::Half2x2).unwrap();
            for px in out.image.data() {
                assert_eq!(*px, [0.9, (0.2 + 0.4) * 0.5, 0.5], "{p}");
            }
        }
    }

    #[test]
    fn blocks_follow_the_sensor_grid() {
        // CFA の画像の左上が S の (2, 4) のとき、出力の格子の (1, 2) は S の (2..4, 4..6)。
        let cfa = CfaImage::from_fn(Roi::new(2, 4, 6, 4), CfaPattern::RGGB, |x, y| {
            (10 * (y + 4) + x + 2) as f32
        })
        .unwrap();
        let out = demosaic(&cfa, DemosaicMethod::Half2x2, Roi::new(1, 2, 3, 2)).unwrap();
        // ブロック S (2..4, 4..6): R = 42、G1 = 43、G2 = 52、B = 53。
        assert_eq!(out.at(1, 2), Some([42.0, 47.5, 53.0]));
        // ブロック S (6..8, 6..8): R = 66、G = (67 + 76) / 2、B = 77。
        assert_eq!(out.at(3, 3), Some([66.0, 71.5, 77.0]));
    }
}
