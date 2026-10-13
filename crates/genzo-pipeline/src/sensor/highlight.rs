//! ステージ 4: ハイライト復元（docs/04_architecture.md の 2.1 節、2.6 節の B1。01 の DEV-06）。処理バージョン 2 から。
//!
//! WB を掛けた CFA（B1 の値）の、飽和の印の付いた画素（ステージ 2。白レベルで切れたチャンネル）の値を、
//! 飽和していないチャンネルと近傍の画素から推定し直す。設定は genzo-model の `HighlightRecovery`
//! （方式と量）。
//!
//! # ステージ 3 の飽和の切りそろえとの関係
//!
//! 処理バージョン 1（と、版 2 で方式が「白で切りそろえる」・量が 0 の設定）は、ステージ 4 を使わず、ステージ 3 が
//! 飽和した画素の近傍を「すべてのチャンネルが飽和する値」で切りそろえる（[`super::wb`] の doc。白飛びが
//! マゼンタになるのを避ける仮の処理）。版 2 で復元するときは、**ステージ 3 では切りそろえず**
//! （[`super::wb::SaturationClip::Off`]）、このステージが、切りそろえた値（[`HighlightPlan::baseline`]。
//! ステージ 3 と同じ規則）と復元した値を量で混ぜる: `出力 = 切りそろえ + 量 × (復元 − 切りそろえ)`
//! （量が 100 % なら復元した値そのもの）。
//!
//! # 復元の方式（AI の仮決定。決定事項ログ No.126）
//!
//! 自前の方式（近傍の色の比で再構成する一般的な考え方に、最小二乗の比と、手がかりの少なさによる明るさだけの
//! 復元への移り方を組み合わせた。第三者のコードは参照していない。docs/third_party.md の 3.10 節）。
//!
//! CFA の 2 × 2 の組（S の偶数の座標から始まる「クアッド」。R・G（G1 と G2 の平均）・B の 3 チャンネル）を単位に
//! 推定する。チャンネルは、組の中にそのチャンネルの飽和した画素があれば「飽和した」とする。飽和したチャンネルの
//! 値（WB の係数 × 1）は、本当の値の **下限** として扱う（`lb_c`）。
//!
//! - **明るさだけ**（`neutral`）: 飽和したチャンネル `c` を `e_n(c) = N` にする。`N` はこの組のチャンネルの値の
//!   最大（飽和したチャンネルは下限 `lb` の値）。飽和したチャンネルが飽和していないチャンネルより暗くならないので、
//!   白飛びの周りがマゼンタ（G だけが飽和して R・B が残る）にならず、すべてのチャンネルが飽和した部分は、すべての
//!   チャンネルが飽和する最小の明るさの無彩色になる（周りから連続する）。飽和するチャンネルが増えても `N` は
//!   下がらない（下限も `N` に入れるため。飽和していないチャンネルだけの最大にすると、2 つ目のチャンネルが飽和した
//!   ところで明るさが落ちる段差ができた）。
//! - **色を保って復元**（`reconstruct`）: 飽和したチャンネル `c` を、近傍の組（チェビシェフ距離
//!   [`HR_SEARCH_RADIUS`] 組以内）のうち `c` と参照のチャンネル `u`（この組で飽和していないチャンネル）が
//!   どちらも飽和していない組の、`c` と `u` の比（最小二乗の比）で、この組の `u` の値から求める:
//!
//!   ```text
//!   s(d) = (R + 1 − d) / (R + 1)                       （d は組のチェビシェフ距離。R = HR_SEARCH_RADIUS）
//!   num = Σ_n Σ_u s · (v_u(n) / L_u²) · v_c(n) · v_u(q)  （n: 近傍の組、u: 参照のチャンネル。L_u は u の飽和の値）
//!   den = Σ_n Σ_u s · (v_u(n) / L_u²) · v_u(n)
//!   e_r = num / den、conf = min(den / HR_CONFIDENCE_WEIGHT, 1)
//!   e(c) = max(lb_c, conf · e_r + (1 − conf) · e_n(c))     （den = 0 なら e_n(c)。e_n(c) = N）
//!   ```
//!
//!   重み `v_u(n)² / L_u²` は、明るい（飽和に近い）参照の組を重く見る（白飛びの縁の色を使う）。比は割り算を
//!   せず最小二乗の形で足すので、暗い組で比が発散しない。近傍の手がかりが少ない（白飛びの奥）ほど `conf` が
//!   下がり、明るさだけの復元に滑らかに近づく（白飛びの奥でマゼンタ・色の段差を作らない）。すべての
//!   チャンネルが飽和した組は、明るさだけの復元と同じ（色は推定しない）。
//!
//! 画素ごとの出力: 飽和した画素は `max(元の値, e(そのチャンネル))`、飽和していない画素は元の値（量が 100 % の
//! とき）。飽和の印はそのまま引き継ぐ。
//!
//! # 範囲と GPU 版
//!
//! 組の位置は S の座標の偶奇で決まり（タイルの位置によらない）、各画素の結果は近傍の決まった範囲
//! （[`HighlightPlan::margin`]）の入力だけで決まるので、タイルに分けても分けなくても同じ。和は決まった順
//! （近傍の組を上の行から、左から。参照のチャンネルは R・G・B の順）で足す。GPU 版（genzo-gpu の
//! `sensor.wgsl` の `highlight_main`）も同じ式と順序で、画素ごとに自分の組の推定を計算する。

use genzo_model::{HighlightMethod, HighlightRecovery};
use rayon::prelude::*;

use super::wb::{SaturationClip, WbCoefficients, dilate};
use crate::error::{PipelineError, Result};
use crate::image::{CfaImage, Roi};

/// 色を保って復元するときに、比を探す近傍の半径（2 × 2 の組の数。チェビシェフ距離）。**仮置き**: 4 組
/// （S の 8 画素）。広げると大きな白飛びの奥まで色が届くが、飽和した画素ごとの計算が `(2R + 1)²` に比例して
/// 増える。実写で決める（人の判断の表 No.137）。
pub const HR_SEARCH_RADIUS: u32 = 4;

/// 色の比の手がかりの重みの和がこの値以上なら、比による推定をそのまま使う（`conf = 1`）。**仮置き**: 飽和に
/// 近い明るさ（重み 約 0.6）の組が、近くに数組あれば十分とした値。
pub const HR_CONFIDENCE_WEIGHT: f32 = 4.0;

/// 復元の方式（[`HighlightMethod`] のうち、ステージ 4 を使うもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HighlightMode {
    /// 明るさだけ（無彩色）。
    Neutral,
    /// 色を保って復元。
    Reconstruct,
}

impl HighlightMode {
    /// GPU 版に渡す番号（0: 明るさだけ、1: 色を保って復元）。
    pub const fn code(self) -> u32 {
        match self {
            HighlightMode::Neutral => 0,
            HighlightMode::Reconstruct => 1,
        }
    }
}

/// ステージ 4 の計画（GPU 版にも同じ値を渡す）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighlightPlan {
    /// 方式。
    pub mode: HighlightMode,
    /// 量（0〜1。設定の 0〜100 を 100 で割った値）。
    pub amount: f32,
    /// 混ぜる相手の切りそろえ（ステージ 3 の規則。[`super::SensorOptions::saturation_clip`]）。
    pub baseline: SaturationClip,
    /// 切りそろえの値（[`WbCoefficients::clip_level`]）。
    pub clip_level: f32,
    /// 重みの `1 / L_u²`（R・G・B。`L_u` はチャンネルの飽和の値（WB の係数。G は G1 と G2 の平均））。
    pub inv_white_sq: [f32; 3],
}

impl HighlightPlan {
    /// 丸めた設定と WB の係数から作る。復元しない設定（[`HighlightRecovery::is_disabled`]）なら `None`。
    pub fn new(
        hr: &HighlightRecovery,
        wb: &WbCoefficients,
        baseline: SaturationClip,
    ) -> Option<Self> {
        if hr.is_disabled() {
            return None;
        }
        let mode = match hr.method {
            HighlightMethod::Clip => return None,
            HighlightMethod::Neutral => HighlightMode::Neutral,
            HighlightMethod::Reconstruct => HighlightMode::Reconstruct,
        };
        let m = wb.multipliers;
        let white = [m[0], (m[1] + m[3]) * 0.5, m[2]];
        Some(Self {
            mode,
            amount: (hr.amount / 100.0).clamp(0.0, 1.0),
            baseline,
            clip_level: wb.clip_level,
            inv_white_sq: white.map(|l| 1.0 / (l * l)),
        })
    }

    /// 出力の範囲の外に必要な入力の余白（S の画素。近傍の組と切りそろえの近傍）。
    pub fn margin(&self) -> u32 {
        (2 * HR_SEARCH_RADIUS + 2).max(self.baseline.margin())
    }
}

/// 2 × 2 の組の値（R・G・B）と、チャンネルが飽和しているか。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Quad {
    v: [f32; 3],
    clipped: [bool; 3],
}

/// 入力の範囲に完全に入る組の表（組の座標 `(qx, qy)` は S の座標の半分の切り捨て）。
struct QuadGrid {
    qx0: i64,
    qy0: i64,
    w: usize,
    h: usize,
    quads: Vec<Quad>,
}

impl QuadGrid {
    fn new(input: &CfaImage) -> Self {
        let roi = input.roi();
        let qx0 = i64::from(roi.x).div_euclid(2) + i64::from(roi.x).rem_euclid(2);
        let qy0 = i64::from(roi.y).div_euclid(2) + i64::from(roi.y).rem_euclid(2);
        let qx1 = roi.x_end().div_euclid(2);
        let qy1 = roi.y_end().div_euclid(2);
        let w = (qx1 - qx0).max(0) as usize;
        let h = (qy1 - qy0).max(0) as usize;
        let iw = roi.width as usize;
        let data = input.data();
        let sat = input.saturated();
        let mut quads = vec![Quad::default(); w * h];
        quads
            .par_chunks_mut(w.max(1))
            .enumerate()
            .for_each(|(j, row)| {
                let sy = 2 * (qy0 + j as i64);
                let ly = (sy - i64::from(roi.y)) as usize;
                for (i, q) in row.iter_mut().enumerate() {
                    let sx = 2 * (qx0 + i as i64);
                    let lx = (sx - i64::from(roi.x)) as usize;
                    let mut g = 0.0f32;
                    let mut out = Quad::default();
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let idx = (ly + dy) * iw + lx + dx;
                            let color = input.color_at((lx + dx) as u32, (ly + dy) as u32);
                            let c = color.rgb_channel();
                            if color.is_green() {
                                g += data[idx];
                            } else {
                                out.v[c] = data[idx];
                            }
                            out.clipped[c] |= sat[idx];
                        }
                    }
                    out.v[1] = g * 0.5;
                    *q = out;
                }
            });
        Self {
            qx0,
            qy0,
            w,
            h,
            quads,
        }
    }

    fn at(&self, qx: i64, qy: i64) -> &Quad {
        let i = (qx - self.qx0) as usize;
        let j = (qy - self.qy0) as usize;
        debug_assert!(i < self.w && j < self.h);
        &self.quads[j * self.w + i]
    }
}

/// 組 `(qx, qy)` の各チャンネルの推定（飽和していないチャンネルは元の値。モジュールの doc の式）。
fn estimate(grid: &QuadGrid, plan: &HighlightPlan, qx: i64, qy: i64) -> [f32; 3] {
    let q = grid.at(qx, qy);
    let mut out = q.v;
    if !q.clipped.iter().any(|&c| c) {
        return out;
    }
    let any_unclipped = q.clipped.iter().any(|&c| !c);
    // N: チャンネルの値の最大（飽和したチャンネルは下限の値）。明るさだけの復元の値。
    let neutral = q.v[0].max(q.v[1]).max(q.v[2]);
    for (c, o) in out.iter_mut().enumerate() {
        if !q.clipped[c] {
            continue;
        }
        *o = if plan.mode == HighlightMode::Neutral || !any_unclipped {
            neutral
        } else {
            reconstruct_channel(grid, plan, q, (qx, qy), c, neutral)
        };
    }
    out
}

/// 色を保って復元するときの、組 `q`（座標 `(qx, qy)`）の飽和したチャンネル `c` の推定（モジュールの doc の式）。
fn reconstruct_channel(
    grid: &QuadGrid,
    plan: &HighlightPlan,
    q: &Quad,
    (qx, qy): (i64, i64),
    c: usize,
    neutral: f32,
) -> f32 {
    let r = HR_SEARCH_RADIUS as i64;
    let r1 = HR_SEARCH_RADIUS as f32 + 1.0;
    let inv_r1 = 1.0 / r1;
    let mut num = 0.0f32;
    let mut den = 0.0f32;
    for dy in -r..=r {
        for dx in -r..=r {
            let nb = grid.at(qx + dx, qy + dy);
            if nb.clipped[c] {
                continue;
            }
            let d = dx.abs().max(dy.abs()) as f32;
            let s = (r1 - d) * inv_r1;
            for u in 0..3 {
                if u == c || q.clipped[u] || nb.clipped[u] {
                    continue;
                }
                let t = s * nb.v[u] * plan.inv_white_sq[u];
                num += t * nb.v[c] * q.v[u];
                den += t * nb.v[u];
            }
        }
    }
    if den > 0.0 {
        let conf = (den / HR_CONFIDENCE_WEIGHT).min(1.0);
        let e = num / den;
        q.v[c].max(conf * e + (1.0 - conf) * neutral)
    } else {
        neutral
    }
}

/// ステージ 4 の CPU 版（モジュールの doc）。`input` は ステージ 3 の出力（切りそろえていない B1 の値）で、範囲
/// `out_roi` を [`HighlightPlan::margin`] だけ広げた範囲を含むこと。出力は範囲 `out_roi` の CFA。
pub fn recover_highlights(
    input: &CfaImage,
    plan: &HighlightPlan,
    out_roi: Roi,
) -> Result<CfaImage> {
    let need = out_roi.expand(plan.margin());
    if !input.roi().contains_roi(&need) {
        return Err(PipelineError::InvalidRoi {
            roi: input.roi(),
            reason: "ハイライト復元の入力が必要な範囲（近傍の余白）を覆っていない",
        });
    }
    let roi = input.roi();
    let (iw, ih) = (roi.width as usize, roi.height as usize);
    let near = match plan.baseline {
        SaturationClip::Neighborhood { radius } if plan.amount < 1.0 => {
            Some(dilate(input.saturated(), iw, ih, radius as usize))
        }
        _ => None,
    };
    // 出力の範囲に掛かる組の推定（飽和した画素のある組だけ計算する）。
    let grid = QuadGrid::new(input);
    let qx_lo = i64::from(out_roi.x).div_euclid(2);
    let qy_lo = i64::from(out_roi.y).div_euclid(2);
    let qx_hi = (out_roi.x_end() - 1).div_euclid(2);
    let qy_hi = (out_roi.y_end() - 1).div_euclid(2);
    let qw = (qx_hi - qx_lo + 1).max(0) as usize;
    let qh = (qy_hi - qy_lo + 1).max(0) as usize;
    let mut est = vec![[0.0f32; 3]; qw * qh];
    est.par_chunks_mut(qw.max(1))
        .enumerate()
        .for_each(|(j, row)| {
            let qy = qy_lo + j as i64;
            for (i, e) in row.iter_mut().enumerate() {
                let qx = qx_lo + i as i64;
                *e = estimate(&grid, plan, qx, qy);
            }
        });
    let (ox, oy) = out_roi.offset_in(&roi).expect("need が out_roi を含む");
    let mut out = input.window(out_roi)?;
    let ow = out.width() as usize;
    let pattern = out.pattern();
    let src = input.data();
    let sat = input.saturated();
    let amount = plan.amount;
    let clip_level = plan.clip_level;
    let baseline = plan.baseline;
    let (data, _) = out.parts_mut();
    data.par_chunks_mut(ow.max(1))
        .enumerate()
        .for_each(|(ly, row)| {
            let sy = i64::from(out_roi.y) + ly as i64;
            let qj = (sy.div_euclid(2) - qy_lo) as usize;
            for (lx, v) in row.iter_mut().enumerate() {
                let sx = i64::from(out_roi.x) + lx as i64;
                let li = (oy + ly) * iw + ox + lx;
                let x = src[li];
                let recon = if sat[li] {
                    let c = pattern.color_at(lx, ly).rgb_channel();
                    let qi = (sx.div_euclid(2) - qx_lo) as usize;
                    x.max(est[qj * qw + qi][c])
                } else {
                    x
                };
                *v = if amount >= 1.0 {
                    recon
                } else {
                    let clipped = match (&near, baseline) {
                        (_, SaturationClip::Global) => true,
                        (Some(near), _) => near[li],
                        _ => false,
                    };
                    let base = if clipped { x.min(clip_level) } else { x };
                    base + amount * (recon - base)
                };
            }
        });
    Ok(out)
}

#[cfg(test)]
mod tests;
