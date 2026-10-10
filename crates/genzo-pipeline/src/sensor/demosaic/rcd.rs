//! RCD（Ratio Corrected Demosaicing）によるデモザイク（最終品質。02 の IQ-03）。
//!
//! # 出典
//!
//! RCD の考案者は Luis Sanz Rodríguez（2017 年ごろに公開）。この実装は、公開されている
//! アルゴリズムの説明（手順の構成と考え方）をもとに書いたもので、他のプロジェクト（RawTherapee・
//! darktable・考案者の参照実装など）のソースは参照・複製していない（docs/third_party.md の 3.5 節の
//! 方針）。説明の正確な出典（URL・版）はこの環境では確認できていないため、人が確認して台帳に記入する。
//!
//! # 手順
//!
//! 記号: `c` は WB 適用済みの CFA（B1 の値）、`w` は行の長さ、`ε = 1e-5`、`ε² = 1e-10`。
//! 「lerp(a, b, t)」は `a + (b − a) × t`。和は書いた順（括弧の内側から、左から）に計算する。
//!
//! 1. **縦横の方向の判定**（全画素）: 色の並びによらない高域通過フィルタ
//!    `HP(i) = ((c[i−3d] − c[i−d]) + (c[i+3d] − c[i+d])) + 3 × ((c[i] − c[i−2d]) + (c[i] − c[i+2d]))`
//!    （d = w なら縦、1 なら横。係数は [1, −3, −1, 6, −1, −3, 1]）の 2 乗を、その方向に 3 画素足して
//!    `V = max(ε², (hv[i−w] + hv[i]) + hv[i+w])`、`H` も同様。`vh_dir = V / (V + H)`。
//!    フィルタは、色ごとに一定の値と、色ごとの 1 次のなだらかな変化に反応しない（偶数の位置の係数
//!    −3, 6, −3 と奇数の位置の係数 1, −1, −1, 1 の和と 1 次モーメントがどちらも 0）。
//!    近傍との比較: 斜めの 4 画素の平均 `nb = 0.25 × ((d[i−w−1] + d[i−w+1]) + (d[i+w−1] + d[i+w+1]))` と
//!    比べて、0.5 から遠い（判定がはっきりした）方を `vh_disc` とする。
//! 2. **低域通過（輝度の近似）**（全画素）: `lpf = c + 0.5 × ((N + S) + (W + E)) + 0.25 × ((NW + NE) + (SW + SE))`。
//!    ベイヤー配列では、どの位置でも R : G : B = 1 : 2 : 1 の重みになる。
//! 3. **R・B の位置の G**: 4 方向の G の推定を、輝度の比で補正する（Ratio Corrected）:
//!    `N_est = c[i−w] × ((lpf[i] + lpf[i]) + ε) / ((lpf[i] + lpf[i−2w]) + ε)`（S・W・E も同様）。
//!    方向の勾配 `N_grad = ε + |c[i−w] − c[i+w]| + |c[i] − c[i−2w]| + |c[i−w] − c[i−3w]| + |c[i−2w] − c[i−4w]|`
//!    （左から順に足す）で重み付けし、`V_est = lerp(N_est, S_est, N_grad / (N_grad + S_grad))`、
//!    `H_est = lerp(W_est, E_est, W_grad / (W_grad + E_grad))`、`G = max(lerp(V_est, H_est, vh_disc), 0)`。
//! 4. **斜めの方向の判定**（R・B の位置）: 1. と同じフィルタを斜め（d = w + 1 と w − 1）に使い、
//!    `pq_dir = P / (P + Q)`、近傍（斜めの 4 画素）と比べて `pq_disc`。
//! 5. **R の位置の B と、B の位置の R**: 斜めの 4 画素の色差（X − G）を、勾配
//!    `NW_grad = ε + |X[i−w−1] − X[i+w+1]| + |X[i−w−1] − X[i−3w−3]| + |G[i] − G[i−2w−2]|` などで重み付けし、
//!    `P_est = lerp(NW_est, SE_est, NW_grad / (NW_grad + SE_grad))`、`Q_est` も同様、
//!    `X = max(G + lerp(P_est, Q_est, pq_disc), 0)`。
//! 6. **G の位置の R・B**: 上下左右の色差（X − G。X は 5. までで R・B の位置のすべてにある）を、勾配
//!    `N_grad = ε + |G[i] − G[i−2w]| + |X[i−w] − X[i+w]| + |X[i−w] − X[i−3w]|` などで重み付けし、
//!    `X = max(G + lerp(V_est, H_est, vh_disc), 0)`。
//!
//! # 参照の説明からの変更点（独自の工夫）
//!
//! - 2 方向の推定の重み付き平均を `lerp` の形で書いた（数学的には同じ）。2 つの推定が等しいとき
//!   丸めの誤差が出ないため、平らな画像を正確に再現できる。
//! - 比の補正の ε を分子にも入れた（`(2 lpf + ε) / (lpf + lpf' + ε)`）。輝度が等しいとき比がちょうど
//!   1 になり、平らな画像で G が正確に再現される。輝度が 0 の付近では比が 1 に近づく。
//! - 値の上限で切り詰めない（下限 0 だけ）。WB を掛けた後の値は 1 を超えるため（B1）。
//! - 近傍との比較には斜めの 4 画素（同じ種類の位置：R・B どうし、G どうし）を使う。
//!
//! # 余白と並列化
//!
//! 出力の 1 画素は、周り [`RCD_MARGIN`] 画素以内の CFA の値だけで決まる（1. が 4、近傍の比較で 5、
//! 3. の G が 5、5. が 7、6. が 10）。出力を行の帯（[`BAND_ROWS`] 行）に分け、帯ごとに余白を付けた
//! 窓で計算する（帯は並列に計算する）。画素ごとの計算は帯の分け方によらないので、帯の大きさや
//! スレッドの数で結果は変わらない。

use rayon::prelude::*;

use crate::image::{CfaImage, Roi};

use super::channel_table;

/// RCD の出力の 1 画素が依存する CFA の範囲の半径（画素）。タイルの余白に使う。
pub const RCD_MARGIN: u32 = 10;

/// 勾配・比の分母の下限（ε）。値の尺度（白レベル = 1）に対して十分小さい値（**仮置き**: 参照の
/// 説明と同じ桁。値を変えると結果がわずかに変わるので、変えるときは処理バージョンを上げる）。
const EPS: f32 = 1e-5;
/// 方向の統計の下限（ε²）。
const EPS_SQ: f32 = 1e-10;
/// 1 つの窓で計算する出力の行数（並列化の単位。結果には影響しない）。
pub const BAND_ROWS: u32 = 64;

/// `cfa`（`out` を [`RCD_MARGIN`] 広げた範囲を含む）から、範囲 `out` の RGB を作る。
pub(crate) fn rcd(cfa: &CfaImage, out: Roi) -> Vec<[f32; 3]> {
    let need = out.expand(RCD_MARGIN);
    let (ox, oy) = need
        .offset_in(&cfa.roi())
        .expect("呼び出し側で範囲を確認済み");
    let ow = out.width as usize;
    let m = RCD_MARGIN as usize;
    let mut result = vec![[0.0f32; 3]; out.pixel_count() as usize];
    result
        .par_chunks_mut(ow * BAND_ROWS as usize)
        .enumerate()
        .for_each(|(band, chunk)| {
            let rows = chunk.len() / ow;
            let y0 = band * BAND_ROWS as usize;
            rcd_window(cfa, ox, oy + y0, ow + 2 * m, rows + 2 * m, chunk);
        });
    result
}

#[inline]
fn sq(v: f32) -> f32 {
    v * v
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// 近傍（斜めの 4 画素）と比べて、0.5 から遠い方を返す。
#[inline]
fn discriminate(dir: &[f32], i: usize, w: usize) -> f32 {
    let d = dir[i];
    let nb = 0.25 * ((dir[i - w - 1] + dir[i - w + 1]) + (dir[i + w - 1] + dir[i + w + 1]));
    if (0.5 - d).abs() < (0.5 - nb).abs() {
        nb
    } else {
        d
    }
}

/// 色の並びによらない高域通過フィルタ（間隔 `d`）。
#[inline]
fn high_pass(c: &[f32], i: usize, d: usize) -> f32 {
    ((c[i - 3 * d] - c[i - d]) + (c[i + 3 * d] - c[i + d]))
        + 3.0 * ((c[i] - c[i - 2 * d]) + (c[i] - c[i + 2 * d]))
}

/// 窓（CFA の画像の中の左上 (bx, by)、幅 ww、高さ wh）で計算し、余白を除いた内側
/// （(ww − 2M) × (wh − 2M)）を `out` に書く。
fn rcd_window(cfa: &CfaImage, bx: usize, by: usize, ww: usize, wh: usize, out: &mut [[f32; 3]]) {
    let w = ww;
    let n = ww * wh;
    let iw = cfa.width() as usize;
    let mut c = Vec::with_capacity(n);
    for y in by..by + wh {
        c.extend_from_slice(&cfa.data()[y * iw + bx..y * iw + bx + ww]);
    }
    // 窓の (x, y) のチャンネル（R = 0・G = 1・B = 2）。
    let table = channel_table(cfa.pattern().shifted(bx, by));
    let ch = |x: usize, y: usize| table[y & 1][x & 1];

    // 1. 縦横の方向の判定。
    let mut hv = vec![0.0f32; n];
    let mut hh = vec![0.0f32; n];
    for y in 3..wh - 3 {
        for x in 3..ww - 3 {
            let i = y * w + x;
            hv[i] = sq(high_pass(&c, i, w));
            hh[i] = sq(high_pass(&c, i, 1));
        }
    }
    let mut vh_dir = vec![0.0f32; n];
    for y in 4..wh - 4 {
        for x in 4..ww - 4 {
            let i = y * w + x;
            let v = EPS_SQ.max((hv[i - w] + hv[i]) + hv[i + w]);
            let h = EPS_SQ.max((hh[i - 1] + hh[i]) + hh[i + 1]);
            vh_dir[i] = v / (v + h);
        }
    }
    let mut vh_disc = vec![0.0f32; n];
    for y in 5..wh - 5 {
        for x in 5..ww - 5 {
            let i = y * w + x;
            vh_disc[i] = discriminate(&vh_dir, i, w);
        }
    }
    drop(vh_dir);

    // 2. 低域通過。
    let mut lpf = vec![0.0f32; n];
    for y in 1..wh - 1 {
        for x in 1..ww - 1 {
            let i = y * w + x;
            lpf[i] = c[i]
                + 0.5 * ((c[i - w] + c[i + w]) + (c[i - 1] + c[i + 1]))
                + 0.25 * ((c[i - w - 1] + c[i - w + 1]) + (c[i + w - 1] + c[i + w + 1]));
        }
    }

    // 3. R・B の位置の G（G の位置は CFA の値のまま）。
    let mut g = c.clone();
    for y in 5..wh - 5 {
        for x in 5..ww - 5 {
            if ch(x, y) == 1 {
                continue;
            }
            let i = y * w + x;
            let grad = |d: usize, forward: bool| -> f32 {
                // forward = true なら i + d の向き（S・E）、false なら i − d の向き（N・W）。
                let at = |k: usize| if forward { c[i + k * d] } else { c[i - k * d] };
                let back = if forward { c[i - d] } else { c[i + d] };
                EPS + (at(1) - back).abs()
                    + (c[i] - at(2)).abs()
                    + (at(1) - at(3)).abs()
                    + (at(2) - at(4)).abs()
            };
            let lc = lpf[i];
            let est = |near: f32, far_lpf: f32| near * (((lc + lc) + EPS) / ((lc + far_lpf) + EPS));
            let n_est = est(c[i - w], lpf[i - 2 * w]);
            let s_est = est(c[i + w], lpf[i + 2 * w]);
            let w_est = est(c[i - 1], lpf[i - 2]);
            let e_est = est(c[i + 1], lpf[i + 2]);
            let (n_grad, s_grad) = (grad(w, false), grad(w, true));
            let (w_grad, e_grad) = (grad(1, false), grad(1, true));
            let v_est = lerp(n_est, s_est, n_grad / (n_grad + s_grad));
            let h_est = lerp(w_est, e_est, w_grad / (w_grad + e_grad));
            g[i] = lerp(v_est, h_est, vh_disc[i]).max(0.0);
        }
    }
    drop(lpf);

    // 4. 斜めの方向の判定（R・B の位置）。hv・hh を P・Q の高域通過に使い回す。
    let (hp, hq) = (&mut hv, &mut hh);
    for y in 3..wh - 3 {
        for x in 3..ww - 3 {
            if ch(x, y) == 1 {
                continue;
            }
            let i = y * w + x;
            hp[i] = sq(high_pass(&c, i, w + 1));
            hq[i] = sq(high_pass(&c, i, w - 1));
        }
    }
    let mut pq_dir = vec![0.0f32; n];
    for y in 4..wh - 4 {
        for x in 4..ww - 4 {
            if ch(x, y) == 1 {
                continue;
            }
            let i = y * w + x;
            let p = EPS_SQ.max((hp[i - w - 1] + hp[i]) + hp[i + w + 1]);
            let q = EPS_SQ.max((hq[i - w + 1] + hq[i]) + hq[i + w - 1]);
            pq_dir[i] = p / (p + q);
        }
    }
    let mut pq_disc = vec![0.0f32; n];
    for y in 5..wh - 5 {
        for x in 5..ww - 5 {
            if ch(x, y) == 1 {
                continue;
            }
            let i = y * w + x;
            pq_disc[i] = discriminate(&pq_dir, i, w);
        }
    }
    drop(pq_dir);
    drop(hv);
    drop(hh);

    // R・B の配列（自分の色の位置は CFA の値）。
    let mut r = vec![0.0f32; n];
    let mut b = vec![0.0f32; n];
    for y in 0..wh {
        for x in 0..ww {
            let i = y * w + x;
            match ch(x, y) {
                0 => r[i] = c[i],
                2 => b[i] = c[i],
                _ => {}
            }
        }
    }

    // 5. R の位置の B と、B の位置の R（斜めの色差）。斜めの隣はもう一方の色なので、X は CFA の値。
    for y in 7..wh - 7 {
        for x in 7..ww - 7 {
            let own = ch(x, y);
            if own == 1 {
                continue;
            }
            let i = y * w + x;
            let diag = |d: usize, forward: bool| -> (f32, f32) {
                // (推定, 勾配)。forward = true なら i + d の向き。
                let at = |k: usize| if forward { i + k * d } else { i - k * d };
                let opposite = if forward { i - d } else { i + d };
                let grad = EPS
                    + (c[at(1)] - c[opposite]).abs()
                    + (c[at(1)] - c[at(3)]).abs()
                    + (g[i] - g[at(2)]).abs();
                (c[at(1)] - g[at(1)], grad)
            };
            let (nw_est, nw_grad) = diag(w + 1, false);
            let (se_est, se_grad) = diag(w + 1, true);
            let (ne_est, ne_grad) = diag(w - 1, false);
            let (sw_est, sw_grad) = diag(w - 1, true);
            let p_est = lerp(nw_est, se_est, nw_grad / (nw_grad + se_grad));
            let q_est = lerp(ne_est, sw_est, ne_grad / (ne_grad + sw_grad));
            let v = (g[i] + lerp(p_est, q_est, pq_disc[i])).max(0.0);
            if own == 0 {
                b[i] = v;
            } else {
                r[i] = v;
            }
        }
    }

    // 6. G の位置の R・B（上下左右の色差）。上下左右の隣は R・B の位置で、R・B ともにある。
    let cardinal = |x_arr: &[f32], i: usize| -> f32 {
        let dir = |d: usize, forward: bool| -> (f32, f32) {
            let at = |k: usize| if forward { i + k * d } else { i - k * d };
            let opposite = if forward { i - d } else { i + d };
            let grad = EPS
                + (g[i] - g[at(2)]).abs()
                + (x_arr[at(1)] - x_arr[opposite]).abs()
                + (x_arr[at(1)] - x_arr[at(3)]).abs();
            (x_arr[at(1)] - g[at(1)], grad)
        };
        let (n_est, n_grad) = dir(w, false);
        let (s_est, s_grad) = dir(w, true);
        let (w_est, w_grad) = dir(1, false);
        let (e_est, e_grad) = dir(1, true);
        let v_est = lerp(n_est, s_est, n_grad / (n_grad + s_grad));
        let h_est = lerp(w_est, e_est, w_grad / (w_grad + e_grad));
        (g[i] + lerp(v_est, h_est, vh_disc[i])).max(0.0)
    };
    let m = RCD_MARGIN as usize;
    let inner_w = ww - 2 * m;
    for y in m..wh - m {
        for x in m..ww - m {
            let i = y * w + x;
            let px = match ch(x, y) {
                0 => [c[i], g[i], b[i]],
                2 => [r[i], g[i], c[i]],
                _ => [cardinal(&r, i), c[i], cardinal(&b, i)],
            };
            out[(y - m) * inner_w + (x - m)] = px;
        }
    }
}

#[cfg(test)]
mod tests {
    use genzo_raw::CfaPattern;

    use super::*;
    use crate::sensor::demosaic::{DemosaicMethod, demosaic, demosaic_full, pad_cfa_mirror};

    fn mosaic(p: CfaPattern, w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3]) -> CfaImage {
        CfaImage::from_fn(Roi::full(w, h), p, |x, y| {
            f(x, y)[p.color_at(x as usize, y as usize).rgb_channel()]
        })
        .unwrap()
    }

    #[test]
    fn flat_colors_are_reconstructed() {
        // 灰色はビット単位で正確に、色の付いた平らな画像は丸めの誤差（数 ULP）以内で再現する。
        for p in CfaPattern::ALL {
            let gray = mosaic(p, 30, 26, |_, _| [0.18; 3]);
            let out = demosaic_full(&gray, DemosaicMethod::Rcd).unwrap();
            assert!(out.image.data().iter().all(|&px| px == [0.18; 3]), "{p}");
            for rgb in [
                [0.7f32, 0.3, 0.123_456_7],
                [2.4, 1.0, 0.01],
                [0.0, 0.5, 0.0],
            ] {
                let flat = mosaic(p, 30, 26, |_, _| rgb);
                let out = demosaic_full(&flat, DemosaicMethod::Rcd).unwrap();
                for px in out.image.data() {
                    for k in 0..3 {
                        let tol = 4.0 * f32::EPSILON * rgb[k].max(rgb[1]).max(1e-3);
                        assert!((px[k] - rgb[k]).abs() <= tol, "{p} {rgb:?} {px:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn output_depends_only_on_the_margin() {
        // タイルの大きさ・位置によらず、同じ画素は同じ値（ビット単位）。
        let scene = |x: u32, y: u32| {
            let fx = x as f32 * 0.37;
            let fy = y as f32 * 0.23;
            [
                0.4 + 0.3 * (fx).sin(),
                0.5 + 0.2 * (fy).cos(),
                0.3 + 0.25 * (fx + fy).sin(),
            ]
        };
        let cfa = mosaic(CfaPattern::RGGB, 90, 150, scene);
        let padded = pad_cfa_mirror(&cfa, RCD_MARGIN).unwrap();
        let full = demosaic(&padded, DemosaicMethod::Rcd, Roi::full(90, 150)).unwrap();
        for tile in [
            Roi::new(0, 0, 7, 5),
            Roi::new(33, 71, 40, 70),
            Roi::new(80, 140, 10, 10),
        ] {
            let t = demosaic(&padded, DemosaicMethod::Rcd, tile).unwrap();
            assert_eq!(t, full.crop(tile).unwrap(), "{tile:?}");
        }
    }

    #[test]
    fn cfa_offsets_give_the_same_result_in_the_interior() {
        // RGGB の画像を (dx, dy) ずらして切り出すと、4 種の CFA の並びになる。内側の結果は同じ。
        let scene = |x: u32, y: u32| {
            let fx = x as f32 * 0.21;
            let fy = y as f32 * 0.17;
            [
                0.5 + 0.4 * (fx).sin(),
                0.4 + 0.3 * (fx * fy * 0.1).cos(),
                0.2 + 0.1 * (fy).sin(),
            ]
        };
        let cfa = mosaic(CfaPattern::RGGB, 64, 64, scene);
        let base = demosaic(&cfa, DemosaicMethod::Rcd, Roi::new(16, 16, 32, 32)).unwrap();
        let mut seen = std::collections::HashSet::new();
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let sub = cfa.window(Roi::new(dx, dy, 60, 60)).unwrap();
            seen.insert(sub.pattern());
            let out = demosaic(&sub, DemosaicMethod::Rcd, Roi::new(16, 16, 32, 32)).unwrap();
            assert_eq!(out, base, "({dx}, {dy}) {}", sub.pattern());
        }
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn values_stay_finite_and_non_negative() {
        // 0 と大きな値が隣り合う極端な CFA（チェッカー模様）。
        let cfa = CfaImage::from_fn(Roi::full(40, 40), CfaPattern::GBRG, |x, y| {
            if (x / 3 + y / 2) % 2 == 0 { 0.0 } else { 900.0 }
        })
        .unwrap();
        let out = demosaic_full(&cfa, DemosaicMethod::Rcd).unwrap();
        assert!(
            out.image
                .as_flat()
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0)
        );
        let zero = CfaImage::from_fn(Roi::full(24, 24), CfaPattern::RGGB, |_, _| 0.0).unwrap();
        let out = demosaic_full(&zero, DemosaicMethod::Rcd).unwrap();
        assert!(out.image.as_flat().iter().all(|&v| v == 0.0));
    }
}
