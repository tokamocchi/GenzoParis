//! ステージ 11: トーン（ハイライト・シャドウ・白・黒。DEV-05、docs/04_architecture.md の 2.1 節・
//! 2.7 節、PoC-5）。
//!
//! 段階 B のガイド（[`crate::guide`]。切り抜く前の全体の、ぼかした log2 の輝度）を **局所の明るさ**
//! として使うローカルトーンマッピング。**仮実装**（式・定数は PoC-5 でハローと見た目を比べて決める）。
//!
//! # 計算（画素ごと。f32、この順序）
//!
//! ```text
//! Y  = 0.2627·R + 0.6780·G + 0.0593·B                    （BT.2020。[`super::luminance`]）
//! L  = log2(max(Y, 2^−16))                                 （画素の明るさ。露光量・コントラストの後）
//! (u, v) = ガイドの座標（[`GuideLookup`]）、 g = ガイドを双線形で引いた値（露光量の前）
//! B  = contrast(g + EV)                                    （局所の明るさ。ステージ 10 の EV を足し、
//!                                                           コントラストと同じ傾きの変更をかける）
//! D  = L − B                                               （詳細。保つ）
//! b  = B − L0                                              （中間グレーからの段数。L0 = log2(0.18)）
//! B' = B + h·H·σ((b − Ch)/Wh) + s·S·σ(−(b − Cs)/Ws)       （ハイライト h・シャドウ s。−1〜1）
//! L1 = B' + D
//! l1 = L1 − L0
//! L2 = min(L1 + w·Wt·σ((l1 − Cw)/Ww) + k·Bt·σ(−(l1 − Cb)/Wb), 64)   （白 w・黒 k。全体の端点）
//! RGB' = RGB × exp2(L2 − L)                                （RGB の比率を保つ）
//! σ(z) = 1 / (1 + exp2(−z))
//! ```
//!
//! - ハイライト・シャドウは **局所の明るさ B** の関数なので、明るい空の中の細部も暗い部屋の中の細部も
//!   `D` として残る。ガイドと画素が同じ（平らなところ）なら、`L` の全体のカーブになる。
//! - 白・黒は画素の明るさ（`L1`）の関数（全体の端点の調整。ガイドを使わない）。
//! - 正の値はどれも明るくする（Lightroom と同じ向き: ハイライト −100 で明るい部分を抑え、
//!   シャドウ +100 で暗い部分を持ち上げる）。
//! - 単調性: σ の傾きは最大 `ln2 / 4 ≒ 0.173` なので、局所の明るさ・画素の明るさについて
//!   `1 − (H + S)·0.173 / W > 0`、`1 − (Wt + Bt)·0.173 / W > 0` となるよう定数を選んでいる
//!   （平らな部分で明暗が逆転しない）。
//! - 負の値（色域外）は符号を保ったまま倍率がかかる。`Y ≤ 2^−16` の画素は下限の明るさで計算する。

use genzo_model::{DevelopSettings, Phase};

use super::exposure::ContrastParams;
use super::geometry::GeometryPlan;
use super::{LOG2_CEILING, LOG2_MIDDLE_GRAY, luminance, map_pixels};
use crate::contract::ColorContract;
use crate::error::Result;
use crate::guide::{GuideLookup, checked_guide};
use crate::image::{ImageTile, Roi};
use crate::interp::sample_bilinear_gray;
use crate::sanitize::safe_log2;
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// ハイライトの最大の変化（段。スライダー ±100 のとき）。仮置き（PoC-5）。
pub const HIGHLIGHTS_MAX_EV: f32 = 1.5;
/// ハイライトの重みの中心（中間グレーからの段数）。仮置き: 中間グレーより 1.5 段明るい（シーンの
/// 約 0.5）あたりから効き始める。
pub const HIGHLIGHTS_CENTER_EV: f32 = 1.5;
/// ハイライトの重みの幅（段）。仮置き。
pub const HIGHLIGHTS_WIDTH_EV: f32 = 1.0;
/// シャドウの最大の変化（段）。仮置き（PoC-5）。
pub const SHADOWS_MAX_EV: f32 = 1.5;
/// シャドウの重みの中心（中間グレーからの段数）。仮置き: 中間グレーより 2 段暗い（シーンの約 0.045）。
pub const SHADOWS_CENTER_EV: f32 = -2.0;
/// シャドウの重みの幅（段）。仮置き。
pub const SHADOWS_WIDTH_EV: f32 = 1.0;
/// 白の最大の変化（段）。仮置き（PoC-5）。
pub const WHITES_MAX_EV: f32 = 1.0;
/// 白の重みの中心（中間グレーからの段数）。仮置き: シーンの 1.0（中間グレーの約 2.5 段上）。
pub const WHITES_CENTER_EV: f32 = 2.5;
/// 白の重みの幅（段）。仮置き。
pub const WHITES_WIDTH_EV: f32 = 0.75;
/// 黒の最大の変化（段）。仮置き（PoC-5）。
pub const BLACKS_MAX_EV: f32 = 1.0;
/// 黒の重みの中心（中間グレーからの段数）。仮置き: シーンの約 0.008（中間グレーの 4.5 段下）。
pub const BLACKS_CENTER_EV: f32 = -4.5;
/// 黒の重みの幅（段）。仮置き。
pub const BLACKS_WIDTH_EV: f32 = 0.75;

/// σ(z) = 1 / (1 + exp2(−z))。
#[inline]
fn sigmoid2(z: f32) -> f32 {
    1.0 / (1.0 + (-z).exp2())
}

/// ステージ 11 のパラメータ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneStageParams {
    /// ハイライト（−1〜1。スライダー / 100）。
    pub highlights: f32,
    /// シャドウ（−1〜1）。
    pub shadows: f32,
    /// 白（−1〜1）。
    pub whites: f32,
    /// 黒（−1〜1）。
    pub blacks: f32,
    /// 露光量（EV。ガイドに足す）。
    pub ev: f32,
    /// コントラスト（ガイドにも同じ傾きの変更をかける）。
    pub contrast: ContrastParams,
    /// C の画素 → ガイドの座標。
    pub lookup: GuideLookup,
}

impl ToneStageParams {
    /// 局所の明るさ `base`（log2）と画素の明るさ `l`（log2）から、明るさの変化（段）を求める
    /// （モジュールの doc の `L2 − L`）。
    #[inline]
    pub fn log2_gain(&self, base: f32, l: f32) -> f32 {
        let detail = l - base;
        let b = base - LOG2_MIDDLE_GRAY;
        let base2 = base
            + self.highlights
                * HIGHLIGHTS_MAX_EV
                * sigmoid2((b - HIGHLIGHTS_CENTER_EV) / HIGHLIGHTS_WIDTH_EV)
            + self.shadows * SHADOWS_MAX_EV * sigmoid2(-(b - SHADOWS_CENTER_EV) / SHADOWS_WIDTH_EV);
        let l1 = base2 + detail;
        let d1 = l1 - LOG2_MIDDLE_GRAY;
        let l2 = (l1
            + self.whites * WHITES_MAX_EV * sigmoid2((d1 - WHITES_CENTER_EV) / WHITES_WIDTH_EV)
            + self.blacks * BLACKS_MAX_EV * sigmoid2(-(d1 - BLACKS_CENTER_EV) / BLACKS_WIDTH_EV))
        .min(LOG2_CEILING);
        l2 - l
    }

    /// 1 画素に適用する（`guide_value` はガイドを引いた値。露光量の前）。
    #[inline]
    pub fn apply(&self, c: [f32; 3], guide_value: f32) -> [f32; 3] {
        let l = safe_log2(luminance(c));
        let base = self.contrast.apply_log2(guide_value + self.ev);
        let r = self.log2_gain(base, l).exp2();
        [c[0] * r, c[1] * r, c[2] * r]
    }
}

/// ステージ 11: トーン（ハイライト・シャドウ・白・黒）。4 つとも 0 なら飛ばす。
///
/// 使うときは文脈にガイド（[`StageContext::guide`]）が必要（なければ
/// [`crate::PipelineError::GuideMismatch`]）。
#[derive(Debug, Clone, Copy, Default)]
pub struct ToneStage;

impl ToneStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.tone";
    /// 04 の 2.1 節のステージの番号。
    pub const NUMBER: u8 = 11;
}

impl Stage for ToneStage {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn phase(&self) -> Phase {
        Phase::C
    }
    fn input_contract(&self) -> ColorContract {
        ColorContract::B2Working
    }
    fn output_contract(&self) -> ColorContract {
        ColorContract::B2Working
    }
    fn params(
        &self,
        settings: &DevelopSettings,
        ctx: &StageContext<'_>,
    ) -> Result<Option<StageParams>> {
        match ctx.process_version {
            ProcessVersion::V1 => {}
        }
        let s = settings.normalized();
        let t = s.tone;
        if [t.highlights, t.shadows, t.whites, t.blacks]
            .iter()
            .all(|&v| v == 0.0)
        {
            return Ok(None);
        }
        let guide = checked_guide(ctx, Self::ID)?;
        let plan = GeometryPlan::new(&s.geometry, ctx.frames, ctx.sensor_grid)?;
        Ok(Some(StageParams::new(ToneStageParams {
            highlights: t.highlights / 100.0,
            shadows: t.shadows / 100.0,
            whites: t.whites / 100.0,
            blacks: t.blacks / 100.0,
            ev: s.exposure_ev,
            contrast: ContrastParams::from_slider(s.contrast),
            lookup: GuideLookup::new(&plan, guide.image.width(), guide.image.height()),
        })))
    }
    fn input_roi(&self, output_roi: Roi, _: &StageParams, _: &StageContext<'_>) -> Roi {
        output_roi
    }
    fn run_cpu(
        &self,
        ctx: &StageContext<'_>,
        input: &ImageTile,
        output: &mut ImageTile,
        params: &StageParams,
    ) -> Result<()> {
        let p = *params.expect::<ToneStageParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 => {}
        }
        let guide = &checked_guide(ctx, Self::ID)?.image;
        map_pixels(input, output, |x, y, c| {
            let (u, v) = p.lookup.coords(x, y);
            p.apply(c, sample_bilinear_gray(guide, u, v))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(h: f32, s: f32, w: f32, b: f32) -> ToneStageParams {
        ToneStageParams {
            highlights: h,
            shadows: s,
            whites: w,
            blacks: b,
            ev: 0.0,
            contrast: ContrastParams::from_slider(0.0),
            lookup: GuideLookup {
                origin: (0, 0),
                scale: (1.0, 1.0),
            },
        }
    }

    /// 平らな部分（ガイド = 画素）での明るさの対応。
    fn global(p: &ToneStageParams, y: f32) -> f32 {
        let out = p.apply([y; 3], y.log2());
        out[0]
    }

    #[test]
    fn neutral_parameters_do_nothing() {
        let p = params(0.0, 0.0, 0.0, 0.0);
        for y in [0.001f32, 0.18, 1.0, 7.0] {
            assert!((global(&p, y) - y).abs() <= y * 1e-6, "{y}");
        }
    }

    #[test]
    fn sliders_move_the_expected_regions() {
        // シャドウ +100 は暗部を持ち上げ、明部はほとんど変えない。
        let p = params(0.0, 1.0, 0.0, 0.0);
        assert!(global(&p, 0.01) > 0.01 * 2.0);
        assert!((global(&p, 2.0) / 2.0 - 1.0).abs() < 0.03);
        // ハイライト −100 は明部を抑え、暗部はほとんど変えない。
        let p = params(-1.0, 0.0, 0.0, 0.0);
        assert!(global(&p, 2.0) < 2.0 * 0.6);
        assert!((global(&p, 0.01) / 0.01 - 1.0).abs() < 0.03);
        // 白 +100 は最も明るい部分を明るく、黒 −100 は最も暗い部分を暗くする。
        assert!(global(&params(0.0, 0.0, 1.0, 0.0), 4.0) > 4.0 * 1.5);
        assert!(global(&params(0.0, 0.0, 0.0, -1.0), 0.002) < 0.002 * 0.6);
    }

    #[test]
    fn global_curve_is_monotonic_for_extreme_settings() {
        for (h, s, w, b) in [
            (-1.0, 1.0, 1.0, -1.0),
            (1.0, -1.0, -1.0, 1.0),
            (1.0, 1.0, 1.0, 1.0),
            (-1.0, -1.0, -1.0, -1.0),
        ] {
            let p = params(h, s, w, b);
            let mut prev = 0.0f32;
            for k in 0..=400 {
                let y = (k as f32 / 400.0 * 24.0 - 18.0).exp2();
                let out = global(&p, y);
                assert!(out >= prev, "{h} {s} {w} {b}: {y} → {out} < {prev}");
                prev = out;
            }
        }
    }

    #[test]
    fn detail_and_hue_are_preserved() {
        let p = params(-1.0, 1.0, 0.0, 0.0);
        // 同じ局所の明るさなら、画素の明るさの比（詳細）は保たれる。
        let base = 0.5f32.log2();
        let a = p.apply([0.4; 3], base)[0];
        let b = p.apply([0.6; 3], base)[0];
        assert!((b / a - 1.5).abs() < 1e-4, "{a} {b}");
        // RGB の比率（色相）は保たれる。
        let c = p.apply([0.3, 0.1, 0.05], (0.15f32).log2());
        assert!((c[0] / c[1] - 3.0).abs() < 1e-4 && (c[1] / c[2] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn exposure_is_added_to_the_guide() {
        // 露光量の前のガイドに EV を足した結果は、露光量の後の画像から作ったガイドを使った結果と同じ
        // （ガイドを作り直さなくてよい。2.7 節）。
        let mut p = params(-1.0, 1.0, 0.5, -0.5);
        let before = 0.05f32.log2();
        p.ev = 2.0;
        let with_ev = p.apply([0.2; 3], before)[0];
        p.ev = 0.0;
        let rebuilt = p.apply([0.2; 3], 0.2f32.log2())[0];
        assert!(
            (with_ev - rebuilt).abs() <= rebuilt * 1e-5,
            "{with_ev} {rebuilt}"
        );
    }

    #[test]
    fn flat_regions_have_no_detail_after_exposure_and_contrast() {
        // 平らな部分では、露光量とコントラスト（ステージ 10）をかけた画素の明るさと、露光量の前の
        // ガイドから求めた局所の明るさが一致する（詳細 D = 0。全体のカーブとして効く）。
        let ev = 1.0f32;
        let contrast = ContrastParams::from_slider(60.0);
        for y0 in [0.002f32, 0.05, 0.18, 0.9, 6.0] {
            let pixel = contrast.apply([y0 * ev.exp2(); 3]);
            let base = contrast.apply_log2(y0.log2() + ev);
            assert!(
                (base - pixel[0].log2()).abs() < 1e-5,
                "{y0}: {base} {pixel:?}"
            );
        }
    }

    #[test]
    fn non_finite_and_negative_inputs_stay_finite() {
        let p = params(1.0, 1.0, 1.0, 1.0);
        for c in [[-1.0, 0.5, 0.2], [0.0; 3], [1e30; 3], [-1e-20, 0.0, 0.0]] {
            for g in [-16.0, 0.0, 30.0] {
                let out = p.apply(c, g);
                assert!(out.iter().all(|v| v.is_finite()), "{c:?} {g} → {out:?}");
            }
        }
    }
}
