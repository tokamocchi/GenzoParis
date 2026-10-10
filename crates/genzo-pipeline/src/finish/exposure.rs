//! ステージ 10: 露光量と、コントラスト（DEV-04。docs/04_architecture.md の 2.1 節）。
//!
//! # 露光量（ステージ 10）
//!
//! 画素の値に `k = 2^EV` を掛けるだけ（`k` は f64 の `exp2` を f32 に丸めた値。GPU 版にも同じ値を
//! 渡す）。シーンリニアのまま行い、切り捨てない（IQ-02。+3EV の後に −3EV で元に戻る）。
//!
//! # コントラスト（ステージ 10 の後、11 の前。仮置き）
//!
//! シーンリニアのまま、**対数の領域で中間グレー（0.18）を軸にした傾きの変更**として行う
//! （PoC-4 / PoC-5 で見直す）。輝度 Y（BT.2020 の係数）について
//!
//! ```text
//! L  = log2(max(Y, 2^−16))
//! L' = min(L0 + γ·(L − L0), 64)          L0 = log2(0.18)、γ = 1 + CONTRAST_STRENGTH × contrast / 100
//! RGB' = RGB × exp2(L' − L)               （RGB の比率を保つ。負の値は符号を保ったまま倍率がかかる）
//! ```
//!
//! 中間グレーは動かず、+100 で中間グレーから 1 段離れた値が `CONTRAST_STRENGTH` 段だけさらに離れる。
//! `2^64` を超える結果は抑える（[`super::LOG2_CEILING`]。あふれによる無限大を防ぐ）。
//! ステージ 11 のガイド（局所の明るさ）にも同じ変換をかける（[`ContrastParams::apply_log2`]）。

use genzo_model::{DevelopSettings, Phase};

use super::{LOG2_CEILING, LOG2_MIDDLE_GRAY, luminance, map_pixels};
use crate::contract::ColorContract;
use crate::error::Result;
use crate::image::{ImageTile, Roi};
use crate::sanitize::safe_log2;
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// コントラストの強さ（仮置き）: スライダー ±100 で、対数の領域の傾きを `1 ± 0.4` にする。
///
/// 根拠: Lightroom のコントラスト +100 が中間調の傾きを 1.3〜1.5 倍程度にする見た目に近い値として
/// 置いた（実測ではない）。PoC-4 / PoC-5 で見直す。
pub const CONTRAST_STRENGTH: f32 = 0.4;

/// 露光量のパラメータ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureParams {
    /// 露光量（EV。丸めた設定の値）。
    pub ev: f32,
    /// 掛ける倍率 `2^EV`（f64 で計算して f32 に丸めた値）。
    pub gain: f32,
}

impl ExposureParams {
    /// EV から作る。
    pub fn new(ev: f32) -> Self {
        Self {
            ev,
            gain: f64::from(ev).exp2() as f32,
        }
    }
}

/// ステージ 10: 露光量（`2^EV` を掛ける）。EV が 0 なら飛ばす。
#[derive(Debug, Clone, Copy, Default)]
pub struct ExposureStage;

impl ExposureStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.exposure";
    /// 04 の 2.1 節のステージの番号。
    pub const NUMBER: u8 = 10;
}

impl Stage for ExposureStage {
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
            ProcessVersion::V1 => {
                let ev = settings.normalized().exposure_ev;
                Ok((ev != 0.0).then(|| StageParams::new(ExposureParams::new(ev))))
            }
        }
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
        let p = *params.expect::<ExposureParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 => {
                let k = p.gain;
                map_pixels(input, output, |_, _, c| [c[0] * k, c[1] * k, c[2] * k])
            }
        }
    }
}

/// コントラストのパラメータ（モジュールの doc の式）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContrastParams {
    /// 対数の領域の傾き γ。
    pub gamma: f32,
}

impl ContrastParams {
    /// スライダーの値（−100〜100）から作る。
    pub fn from_slider(contrast: f32) -> Self {
        Self {
            gamma: 1.0 + CONTRAST_STRENGTH * (contrast / 100.0),
        }
    }

    /// 何もしない（γ = 1）か。
    pub fn is_identity(&self) -> bool {
        self.gamma == 1.0
    }

    /// 対数の値 `l`（log2）に傾きの変更をかける: `min(L0 + γ·(l − L0), 64)`。
    #[inline]
    pub fn apply_log2(&self, l: f32) -> f32 {
        (LOG2_MIDDLE_GRAY + self.gamma * (l - LOG2_MIDDLE_GRAY)).min(LOG2_CEILING)
    }

    /// 1 画素に適用する。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let l = safe_log2(luminance(c));
        let r = (self.apply_log2(l) - l).exp2();
        [c[0] * r, c[1] * r, c[2] * r]
    }
}

/// コントラスト（ステージ 10 の後、11 の前。仮置き）。コントラストが 0 なら飛ばす。
#[derive(Debug, Clone, Copy, Default)]
pub struct ContrastStage;

impl ContrastStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.contrast";
    /// 04 の 2.1 節のステージの番号（ステージ 10 の一部として扱う）。
    pub const NUMBER: u8 = 10;
}

impl Stage for ContrastStage {
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
            ProcessVersion::V1 => {
                let p = ContrastParams::from_slider(settings.normalized().contrast);
                Ok((!p.is_identity()).then(|| StageParams::new(p)))
            }
        }
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
        let p = *params.expect::<ContrastParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 => map_pixels(input, output, |_, _, c| p.apply(c)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_gain_is_a_power_of_two() {
        assert_eq!(ExposureParams::new(3.0).gain, 8.0);
        assert_eq!(ExposureParams::new(-3.0).gain, 0.125);
        assert_eq!(ExposureParams::new(0.0).gain, 1.0);
        assert!((ExposureParams::new(0.5).gain - std::f32::consts::SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn contrast_keeps_middle_gray_and_changes_slope() {
        let p = ContrastParams::from_slider(100.0);
        assert!((p.gamma - 1.4).abs() < 1e-6);
        let g = p.apply([0.18; 3]);
        assert!(g.iter().all(|v| (v - 0.18).abs() < 1e-6), "{g:?}");
        // 中間グレーより 1 段明るい値は 1.4 段明るくなる。
        let b = p.apply([0.36; 3]);
        assert!((b[0] / 0.18 - 2f32.powf(1.4)).abs() < 1e-4, "{b:?}");
        // 負のコントラストは傾きを小さくする。
        let q = ContrastParams::from_slider(-100.0);
        let d = q.apply([0.09; 3]);
        assert!((d[0] / 0.18 - 2f32.powf(-0.6)).abs() < 1e-4, "{d:?}");
        assert!(ContrastParams::from_slider(0.0).is_identity());
    }

    #[test]
    fn contrast_handles_negative_and_huge_values() {
        let p = ContrastParams::from_slider(100.0);
        for c in [
            [-0.5, 0.2, 0.1],
            [0.0; 3],
            [-1e-9, -2.0, 0.0],
            [1e30, 1e30, 1e30],
        ] {
            let out = p.apply(c);
            assert!(out.iter().all(|v| v.is_finite()), "{c:?} → {out:?}");
        }
        // 比率は保たれる（色相を変えない）。
        let out = p.apply([0.4, 0.2, 0.1]);
        assert!((out[0] / out[1] - 2.0).abs() < 1e-5 && (out[1] / out[2] - 2.0).abs() < 1e-5);
    }
}
