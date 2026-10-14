//! ステージ 6: ノイズ軽減（docs/04_architecture.md の 2.1 節、2.6 節の B1。01 の DEV-13）。処理バージョン 2 から。
//!
//! デモザイクの後のカメラ RGB（B1。段階 A1）の輝度ノイズと色ノイズを、**ウェーブレット（à trous）の係数の縮小**で
//! 軽減する（AI の仮決定。決定事項ログ No.127）。設定は genzo-model の `NoiseReduction`（輝度の量・ディテール・
//! コントラスト、色の量・ディテール）。手法は公開の文献のもの（Anscombe 変換・B3 スプラインの à trous・
//! non-negative garrote。出典は docs/third_party.md の 3.10 節）で、第三者のコードは参照していない。
//!
//! # 計算（GPU 版も同じ式と順序。2.3 節）
//!
//! 1. **分散の安定化と輝度・色への分解**（画素ごと）: 光のショットノイズは明るさとともに大きくなるので、各チャンネルの
//!    平方根を取って明るさによらない大きさにする（Anscombe 変換の近似）:
//!    ```text
//!    s_c = sqrt(max(x_c, 0) + ε)                     （ε = NR_EPSILON）
//!    Y = (s_R + 2·s_G + s_B) × 0.25、 D1 = s_R − s_G、 D2 = s_B − s_G
//!    ```
//! 2. **ウェーブレットの分解と係数の縮小**（Y・D1・D2 の面ごと。量が 0 の面は計算しない）: B3 スプラインの 5 タップ
//!    `[1, 4, 6, 4, 1] / 16` を、段 `j`（0 から）で間隔 `2^j` を空けて、横 → 縦の順にかける（`c_{j+1}`）。各段の細部
//!    `w_j = c_j − c_{j+1}` を、段ごとのしきい値 `T_j` で縮小する（non-negative garrote: `|w| ≤ T` なら 0、それ以外は
//!    `w − T² / w`。軟判定のしきい値処理より、強い縁を弱めにくい）。結果は `c_L + (w'_0 + w'_1 + …)`（細かい段から足す）。
//!    5 タップの重みの和は `(((a0·1/16 + a1·4/16) + a2·6/16) + a3·4/16) + a4·1/16` の順。
//! 3. **戻す**: `s_G = Y − (D1 + D2) × 0.25`、`s_R = D1 + s_G`、`s_B = D2 + s_G`、`x_c = max(s_c, 0)² − ε`（0 未満は
//!    0。B1 は 0 以上）。
//!
//! # しきい値（[`NoisePlan::new`]）とスケール（IQ-07b）
//!
//! しきい値は **S（センサー）の画素の尺度の段**（段 `k` は S の `2^k` 画素の細部。`k` = 0〜[`NR_LEVELS`] − 1）で定義する:
//!
//! ```text
//! T_Y[k] = 輝度 / 100 · NR_THRESHOLD_MAX · NR_LUMA_LEVEL_NOISE[k] · (1 − ディテール / 100 · NR_DETAIL_SHAPE[k])
//!                                                         · (1 − コントラスト / 100 · NR_CONTRAST_SHAPE[k])
//! T_D[k] = 色 / 100 · NR_THRESHOLD_MAX · NR_CHROMA_BOOST · NR_CHROMA_LEVEL_NOISE[k] · (1 − 色のディテール / 100 · NR_COLOR_DETAIL_SHAPE[k])
//! ```
//!
//! - `NR_LUMA_LEVEL_NOISE[k]`・`NR_CHROMA_LEVEL_NOISE[k]` は、CFA の各画素に独立な雑音を足して RCD でデモザイクした
//!   画像の、平方根の領域の Y・D の段 `k` の細部の標準偏差（画素の雑音 1 あたり。テストの `demosaiced_noise_per_level` で
//!   計測した値（512 × 512 の CFA））。デモザイクで隣の画素と混ざるので、白色雑音より粗い段に多く残り（Y の段 3 で
//!   約 1.7 倍）、色（D）の最も細かい段は少ない。しきい値を、その段に残る雑音の大きさに比例させる。
//! - `NR_THRESHOLD_MAX` は量 100 のときの、平方根の領域の画素の雑音（の目安）に対する倍率で、ISO 25600 相当のショット
//!   ノイズ（平方根の領域で 約 0.036）の 約 3 倍。色は `NR_CHROMA_BOOST` 倍（色ノイズは目立つので強めに消す）。
//! - **段階 A1 は出力の格子（最終品質は S と同じ、ドラッグ中の 2 × 2 の簡易処理は半分）で計算する**。簡易処理の格子の
//!   段 `j` は S の段 `k = j + 1` に当たるので、ディテール・コントラストの割合は `k` の値を使い、S の最も細かい段
//!   （2 × 2 でまとめた時点で消えている）は計算しない。雑音の大きさは、補間しない簡易処理の格子の値
//!   （`NR_HALF_LUMA_LEVEL_NOISE`・`NR_HALF_CHROMA_LEVEL_NOISE`）にして、同じ量で最終品質と同じ程度に効くようにする。
//! - プレビュー（長辺 2560）は、このステージを出力の格子（フル解像度）でかけてから縮小する（[`super::SensorPlan::render_preview`]）
//!   ので、書き出し（フル解像度のタイル）と画素ごとに同じ値になる。IQ-07b（縮小したプレビューとフル解像度の一致）に
//!   このステージは差を加えない。差が出るのは、ドラッグ中の簡易処理（IQ-07b の対象外。2.2 節）だけ。
//!
//! # 範囲
//!
//! 出力の画素は、入力の `±2·(2^L − 1)` 画素（`L` は格子の段の数。フル解像度で 62、簡易処理で 30）だけで決まり、和は決まった
//! 順で足すので、タイルに分けても分けなくても同じ（[`NoisePlan::margin`]）。画像の端の外の入力は、ステージ 2 が鏡映で
//! 作った CFA からデモザイクした値（2.7 節）。

use genzo_model::NoiseReduction;
use rayon::prelude::*;

use super::demosaic::DemosaicMethod;
use crate::error::{PipelineError, Result};
use crate::image::{ImageTile, Roi};

/// S の尺度の段の数（細部の段。S の 1・2・4・8・16 画素）。
pub const NR_LEVELS: usize = 5;

/// 分散の安定化の平方根に足す値（白レベルを 1 とする B1 の値。**仮置き**: 2^−12。読み出しのノイズの分散とショット
/// ノイズの係数の比の目安。0 の付近で平方根の傾きが無限大にならないようにする）。
pub const NR_EPSILON: f32 = 1.0 / 4096.0;

/// 量 100 のときのしきい値の、平方根の領域の画素の雑音の標準偏差 1 に対する値（**仮置き**: ISO 25600 相当のショット
/// ノイズ（平方根の領域で 約 0.036）の 約 3 倍）。実写で決める（人の判断の表 No.138）。
pub const NR_THRESHOLD_MAX: f32 = 0.1;

/// 色（D1・D2）のしきい値の倍率（**仮置き**）。
pub const NR_CHROMA_BOOST: f32 = 2.0;

/// RCD でデモザイクした独立な雑音の、平方根の領域の輝度（Y）の段 `k` の細部の標準偏差（画素の雑音 1 あたり。
/// `demosaiced_noise_matches_the_level_tables` のテストで確かめる値）。
pub const NR_LUMA_LEVEL_NOISE: [f32; NR_LEVELS] = [0.61, 0.205, 0.086, 0.043, 0.020];

/// 同じく色（D1・D2）の段ごとの標準偏差。
pub const NR_CHROMA_LEVEL_NOISE: [f32; NR_LEVELS] = [0.41, 0.295, 0.185, 0.10, 0.052];

/// 2 × 2 の簡易処理の格子（補間せず、G は 2 画素の平均、R・B は 1 画素）の、平方根の領域の輝度（Y）の格子の段 `j`
/// （S の段 `j + 1`）の細部の標準偏差（画素の雑音 1 あたり）。白色雑音の à trous の段の標準偏差（0.8907・0.2007・
/// 0.0856・0.0413）に、Y の画素の雑音（0.5）を掛けた値（`half_grid_noise_matches_the_level_tables` のテストで確かめる）。
pub const NR_HALF_LUMA_LEVEL_NOISE: [f32; NR_LEVELS - 1] = [0.445, 0.100, 0.043, 0.021];

/// 同じく色（D1・D2。画素の雑音 √1.5 ≈ 1.225）の段ごとの標準偏差。
pub const NR_HALF_CHROMA_LEVEL_NOISE: [f32; NR_LEVELS - 1] = [1.09, 0.246, 0.105, 0.051];

/// 輝度のディテール 100 で、段ごとにしきい値を下げる割合（細かい段ほど下げる。**仮置き**）。
pub const NR_DETAIL_SHAPE: [f32; NR_LEVELS] = [0.8, 0.5, 0.25, 0.0, 0.0];

/// 輝度のコントラスト 100 で、段ごとにしきい値を下げる割合（粗い段ほど下げる。**仮置き**）。
pub const NR_CONTRAST_SHAPE: [f32; NR_LEVELS] = [0.0, 0.0, 0.3, 0.6, 0.9];

/// 色のディテール 100 で、段ごとにしきい値を下げる割合（細かい段ほど下げる。**仮置き**）。
pub const NR_COLOR_DETAIL_SHAPE: [f32; NR_LEVELS] = [0.9, 0.6, 0.3, 0.0, 0.0];

/// B3 スプラインの 5 タップの重み（端・その隣・中心）。
pub const B3_WEIGHTS: [f32; 3] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0];

/// ステージ 6 の計画（GPU 版にも同じ値を渡す）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoisePlan {
    /// 格子の段の数（フル解像度は 5、簡易処理は 4）。
    pub levels: u32,
    /// 輝度（Y）の格子の段ごとのしきい値（段 `j` は格子の間隔 `2^j`。`levels` より先は 0）。
    pub luma: [f32; NR_LEVELS],
    /// 色（D1・D2）の格子の段ごとのしきい値。
    pub chroma: [f32; NR_LEVELS],
}

impl NoisePlan {
    /// 丸めた設定と、デモザイクの方式（出力の格子）から作る。量がどちらも 0 なら `None`。
    ///
    /// 2 × 2 の簡易処理（格子の 1 画素が S の 2 画素）では、格子の段 `j` を S の段 `j + 1` として、ディテール・
    /// コントラストの段ごとの割合をそろえ、雑音の大きさは簡易処理の格子の値（[`NR_HALF_LUMA_LEVEL_NOISE`]・
    /// [`NR_HALF_CHROMA_LEVEL_NOISE`]）を使う（同じ量で、最終品質と同じ程度に効くように）。
    pub fn new(nr: &NoiseReduction, demosaic: DemosaicMethod) -> Option<Self> {
        if nr.is_disabled() {
            return None;
        }
        let half = demosaic == DemosaicMethod::Half2x2;
        let shift = demosaic.sensor_pixels_per_output().trailing_zeros() as usize;
        let levels = NR_LEVELS - shift;
        let pct = |v: f32| v / 100.0;
        let mut luma = [0.0f32; NR_LEVELS];
        let mut chroma = [0.0f32; NR_LEVELS];
        for (j, (l, c)) in luma
            .iter_mut()
            .zip(chroma.iter_mut())
            .take(levels)
            .enumerate()
        {
            let k = j + shift;
            let (ln, cn) = if half {
                (NR_HALF_LUMA_LEVEL_NOISE[j], NR_HALF_CHROMA_LEVEL_NOISE[j])
            } else {
                (NR_LUMA_LEVEL_NOISE[k], NR_CHROMA_LEVEL_NOISE[k])
            };
            *l = pct(nr.luminance)
                * NR_THRESHOLD_MAX
                * ln
                * (1.0 - pct(nr.luminance_detail) * NR_DETAIL_SHAPE[k])
                * (1.0 - pct(nr.luminance_contrast) * NR_CONTRAST_SHAPE[k]);
            *c = pct(nr.color)
                * NR_THRESHOLD_MAX
                * NR_CHROMA_BOOST
                * cn
                * (1.0 - pct(nr.color_detail) * NR_COLOR_DETAIL_SHAPE[k]);
        }
        Some(Self {
            levels: levels as u32,
            luma,
            chroma,
        })
    }

    /// 輝度の面を計算するか（量が 0 でない）。
    pub fn luma_active(&self) -> bool {
        self.luma.iter().any(|&t| t > 0.0)
    }

    /// 色の面を計算するか（量が 0 でない）。
    pub fn chroma_active(&self) -> bool {
        self.chroma.iter().any(|&t| t > 0.0)
    }

    /// 出力の範囲の外に必要な入力の余白（格子の画素）: `2·(2^L − 1)`。
    pub fn margin(&self) -> u32 {
        2 * ((1u32 << self.levels) - 1)
    }

    /// 格子の段 `j` の、段 `j + 1` の範囲の外側の幅（5 タップの間隔 `2^j` の 2 倍）。
    pub fn level_margin(j: u32) -> u32 {
        2 << j
    }
}

/// non-negative garrote（モジュールの doc）。`t` は 0 以上。
#[inline]
pub fn garrote(w: f32, t: f32) -> f32 {
    if t <= 0.0 {
        w
    } else if w.abs() <= t {
        0.0
    } else {
        w - t * t / w
    }
}

/// 5 タップの重み付きの和（モジュールの doc の順序）。
#[inline]
pub fn b3_sum(a: [f32; 5]) -> f32 {
    let [w0, w1, w2] = B3_WEIGHTS;
    (((a[0] * w0 + a[1] * w1) + a[2] * w2) + a[3] * w1) + a[4] * w0
}

/// 画素を平方根の領域の (Y, D1, D2) にする。
#[inline]
pub fn forward(c: [f32; 3]) -> [f32; 3] {
    let s = c.map(|x| (x.max(0.0) + NR_EPSILON).sqrt());
    [(s[0] + 2.0 * s[1] + s[2]) * 0.25, s[0] - s[1], s[2] - s[1]]
}

/// (Y, D1, D2) を B1 の値に戻す。
#[inline]
pub fn inverse(p: [f32; 3]) -> [f32; 3] {
    let g = p[0] - (p[1] + p[2]) * 0.25;
    let s = [p[1] + g, g, p[2] + g];
    s.map(|v| {
        let v = v.max(0.0);
        (v * v - NR_EPSILON).max(0.0)
    })
}

/// 5 本の同じ長さの列の、要素ごとの [`b3_sum`]（`out[x] = b3_sum([a0[x], …, a4[x]])`）。
#[inline]
fn b3_lanes(out: &mut [f32], a: [&[f32]; 5]) {
    let n = out.len();
    let [a0, a1, a2, a3, a4] = a.map(|s| &s[..n]);
    for x in 0..n {
        out[x] = b3_sum([a0[x], a1[x], a2[x], a3[x], a4[x]]);
    }
}

/// 段ごとの計算で、1 回にまとめて処理する出力の行の数の最小（横方向の途中の値を、キャッシュに収まる大きさの帯で
/// 作って使い切るため。結果には影響しない）。
const BLOCK_ROWS: usize = 32;

/// 1 つの面のウェーブレットの縮小（モジュールの doc の 2.）。`plane` は範囲 `in_roi` の値（行優先）、結果は範囲
/// `out_roi` の値。`in_roi` は `out_roi` を [`NoisePlan::margin`] だけ広げた範囲そのもの。
///
/// 段ごとに、次の段の行を帯（[`BLOCK_ROWS`] 行以上）に分け、帯ごとに横方向の途中の値を作って縦方向にかけ、同じ帯の
/// 細部を縮小して足す（途中の値を画像全体の大きさで持たず、メモリの読み書きを減らすため）。式と足す順序は帯の分け方に
/// よらない。
fn shrink_plane(
    plane: Vec<f32>,
    in_roi: Roi,
    out_roi: Roi,
    levels: u32,
    thresholds: &[f32; NR_LEVELS],
) -> Vec<f32> {
    let (ow, oh) = (out_roi.width as usize, out_roi.height as usize);
    let mut acc = vec![0.0f32; ow * oh];
    let mut cur = plane;
    let mut cur_roi = in_roi;
    let mut next: Vec<f32> = Vec::new();
    for j in 0..levels {
        let hole = 1usize << j;
        let m = NoisePlan::level_margin(j);
        let next_roi = cur_roi.shrink(m).expect("範囲は余白の分だけ大きい");
        let cw = cur_roi.width as usize;
        let (nw, nh) = (next_roi.width as usize, next_roi.height as usize);
        let mm = m as usize;
        let t = thresholds[j as usize];
        // out_roi の行・列の、next の中の位置（細部を足す範囲）。
        let (ox, oy) = out_roi
            .offset_in(&next_roi)
            .expect("out_roi は next_roi の内側");
        let block = BLOCK_ROWS.max(8 * hole);
        next.clear();
        next.resize(nw * nh, 0.0);
        let cur_ref = &cur;
        next.par_chunks_mut(nw.max(1) * block)
            .enumerate()
            .for_each_init(Vec::new, |tmp: &mut Vec<f32>, (b, out)| {
                let y0 = b * block;
                let rows = out.len() / nw.max(1);
                // 横方向: cur の行 y0 .. y0 + rows + 4·hole（next の行 y の縦の 5 タップは cur の行 y .. y + 4·hole）。
                // 途中の値の領域はスレッドごとに使い回す。
                let th = rows + 4 * hole;
                tmp.resize(nw * th, 0.0);
                for (r, t) in tmp[..nw * th].chunks_mut(nw.max(1)).enumerate() {
                    let src = &cur_ref[(y0 + r) * cw..(y0 + r + 1) * cw];
                    let at = |d: usize| &src[mm + d - 2 * hole..];
                    b3_lanes(
                        t,
                        [at(0), at(hole), at(2 * hole), at(3 * hole), at(4 * hole)],
                    );
                }
                // 縦方向。
                for (y, o) in out.chunks_mut(nw.max(1)).enumerate() {
                    let row = |r: usize| &tmp[r * nw..(r + 1) * nw];
                    b3_lanes(
                        o,
                        [
                            row(y),
                            row(y + hole),
                            row(y + 2 * hole),
                            row(y + 3 * hole),
                            row(y + 4 * hole),
                        ],
                    );
                }
            });
        // 細部を縮小して足す（out_roi の画素だけ。cur の中の位置は next の中の位置 + 余白）。
        let next_ref = &next;
        acc.par_chunks_mut(ow.max(1))
            .enumerate()
            .for_each(|(y, a)| {
                let ny = oy + y;
                let crow = &cur_ref[(ny + mm) * cw + ox + mm..][..ow];
                let nrow = &next_ref[ny * nw + ox..][..ow];
                for ((a, &c), &n) in a.iter_mut().zip(crow).zip(nrow) {
                    *a += garrote(c - n, t);
                }
            });
        std::mem::swap(&mut cur, &mut next);
        cur_roi = next_roi;
    }
    debug_assert_eq!(cur_roi, out_roi);
    cur.truncate(ow * oh);
    cur.iter_mut().zip(&acc).for_each(|(c, a)| *c += a);
    cur
}

/// ステージ 6 の CPU 版（モジュールの doc）。`input` は範囲 `out_roi` を [`NoisePlan::margin`] だけ広げた範囲を含むこと。
pub fn reduce_noise(input: &ImageTile, plan: &NoisePlan, out_roi: Roi) -> Result<ImageTile> {
    let in_roi = out_roi.expand(plan.margin());
    let Some((ix, iy)) = in_roi.offset_in(&input.roi) else {
        return Err(PipelineError::InvalidRoi {
            roi: input.roi,
            reason: "ノイズ軽減の入力が必要な範囲（ウェーブレットの余白）を覆っていない",
        });
    };
    let (w, h) = (in_roi.width as usize, in_roi.height as usize);
    let src_w = input.roi.width as usize;
    let src = input.image.data();
    // 平方根の領域の 3 つの面（入力の範囲 in_roi）。
    let mut planes = [
        vec![0.0f32; w * h],
        vec![0.0f32; w * h],
        vec![0.0f32; w * h],
    ];
    {
        let [py, p1, p2] = &mut planes;
        py.par_chunks_mut(w.max(1))
            .zip(p1.par_chunks_mut(w.max(1)))
            .zip(p2.par_chunks_mut(w.max(1)))
            .enumerate()
            .for_each(|(y, ((ry, r1), r2))| {
                let row = &src[(iy + y) * src_w + ix..][..w];
                for (x, c) in row.iter().enumerate() {
                    let [a, b, d] = forward(*c);
                    ry[x] = a;
                    r1[x] = b;
                    r2[x] = d;
                }
            });
    }
    let (ox, oy) = out_roi
        .offset_in(&in_roi)
        .expect("in_roi は out_roi を含む");
    let (ow, oh) = (out_roi.width as usize, out_roi.height as usize);
    // 面ごとの結果（量が 0 の面は元の値）。
    let [py, p1, p2] = planes;
    let crop = |p: &[f32]| -> Vec<f32> {
        let mut out = Vec::with_capacity(ow * oh);
        for y in 0..oh {
            out.extend_from_slice(&p[(oy + y) * w + ox..][..ow]);
        }
        out
    };
    let ry = if plan.luma_active() {
        shrink_plane(py, in_roi, out_roi, plan.levels, &plan.luma)
    } else {
        crop(&py)
    };
    let (r1, r2) = if plan.chroma_active() {
        (
            shrink_plane(p1, in_roi, out_roi, plan.levels, &plan.chroma),
            shrink_plane(p2, in_roi, out_roi, plan.levels, &plan.chroma),
        )
    } else {
        (crop(&p1), crop(&p2))
    };
    let mut out = ImageTile::new(out_roi, input.contract())?;
    out.image
        .data_mut()
        .par_chunks_mut(ow.max(1))
        .enumerate()
        .for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let i = y * ow + x;
                *px = inverse([ry[i], r1[i], r2[i]]);
            }
        });
    Ok(out)
}

#[cfg(test)]
mod tests;
