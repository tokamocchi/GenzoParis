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
//! r  = exp2(L' − L)                       （輝度の倍率）
//! k  = Y < 0.18 なら min(r, CONTRAST_CHROMA_GAIN_MAX)、それ以外は r   （色の成分の倍率）
//! k = r なら RGB' = RGB × r               （RGB の比率を保つ。負の値は符号を保ったまま倍率がかかる）
//! それ以外は RGB' = Y·r + k·(RGB − Y)     （輝度は Y·r、色の成分（無彩色からの差）は k 倍）
//! ```
//!
//! 中間グレーは動かず、+100 で中間グレーから 1 段離れた値が `CONTRAST_STRENGTH` 段だけさらに離れる。
//!
//! **暗部の色の成分の倍率の上限（人の判断の表 No.18 の決定）**: 負のコントラストでは暗いほど輝度の
//! 倍率 `r` が大きい（−100 で 2^−16 以下は約 42 倍）。比率を保って RGB 全体に掛けると、暗部の雑音の
//! 色の成分（カメラ行列の後は、輝度が 0 に近くてもチャンネルの値が正負に散らばる）も同じ倍率で増え、
//! 持ち上げた暗部に鮮やかな点が出る。そこで、トーンカーブの黒の持ち上げ（ステージ 16。No.14）と同じ
//! 考え方で、輝度と色の成分の倍率を分け、中間グレーより暗い部分の色の成分の倍率を
//! [`CONTRAST_CHROMA_GAIN_MAX`]（仮置き）で止める。倍率が上限より小さい部分（正のコントラストの暗部、
//! 中間グレーの近く）と、中間グレーより明るい部分は、これまでどおり比率を保つ（`k = r`。上限は 1 以上
//! なので中間グレーで連続）。
//! `2^64` を超える結果は抑える（[`super::LOG2_CEILING`]。あふれによる無限大を防ぐ）。
//! ステージ 11 のガイド（局所の明るさ）にも同じ変換をかける（[`ContrastParams::apply_log2`]）。
//!
//! # ローカル補正（処理バージョン 2。DEV-18。[`crate::local`]）
//!
//! 画素の局所の値（色温度 `dt`・色かぶり `di`・露光量 `de`・コントラスト `dc`）を全体の値に足す:
//!
//! ```text
//! 露光量:     de = dt = di = 0 なら RGB × k（全体の倍率）。それ以外は
//!             k' = de = 0 なら k、それ以外は exp2(clamp(EV + de, ±LOCAL_EXPOSURE_LIMIT))（f32）
//!             g = dt = di = 0 なら (1, 1, 1)、それ以外は local::white_balance_gains(dt, di)（和は ±100 に収める）
//!             RGB' = (R·(k'·g_R), G·(k'·g_G), B·(k'·g_B))
//! コントラスト: dc = 0 なら全体の式（全体のコントラストが 0 でマスクだけで有効なときは何もしない）。
//!             それ以外は γ' = 1 + CONTRAST_STRENGTH × clamp(contrast / 100 + dc / 100, −2, 2) の式
//! ```
//!
//! マスクの外（局所の値が 0）の画素は、マスクのない描画とビット単位で同じになる。

use genzo_model::{DevelopSettings, Phase};

use super::{LOG2_CEILING, LOG2_MIDDLE_GRAY, MIDDLE_GRAY, luminance, map_pixels};
use crate::contract::ColorContract;
use crate::error::Result;
use crate::image::{ImageTile, Roi};
use crate::local::{
    LOCAL_EXPOSURE_LIMIT, LocalParam, LocalSlots, LocalValues, add_unit, checked_field,
    local_slots, white_balance_gains,
};
use crate::sanitize::safe_log2;
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// コントラストの強さ（仮置き）: スライダー ±100 で、対数の領域の傾きを `1 ± 0.4` にする。
///
/// 根拠: Lightroom のコントラスト +100 が中間調の傾きを 1.3〜1.5 倍程度にする見た目に近い値として
/// 置いた（実測ではない）。PoC-4 / PoC-5 で見直す。
pub const CONTRAST_STRENGTH: f32 = 0.4;

/// コントラストで、中間グレーより暗い部分の色の成分（無彩色からの差）に掛ける倍率の上限
/// （**仮置き**: 2 段。コントラスト −100 では中間グレーから 5 段暗い輝度（約 0.0056）で輝度の倍率が
/// 4 に達し、それより暗い部分では輝度だけが持ち上がる。人の判断の表 No.18。PoC-4 / PoC-5 で見直す）。
pub const CONTRAST_CHROMA_GAIN_MAX: f32 = 4.0;

/// 露光量のパラメータ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureParams {
    /// 露光量（EV。丸めた設定の値）。
    pub ev: f32,
    /// 掛ける倍率 `2^EV`（f64 で計算して f32 に丸めた値）。
    pub gain: f32,
    /// ローカル補正の局所の値の面の割り当て（色温度・色かぶり・露光量のどれかの面があるときだけ）。
    pub local: Option<LocalSlots>,
}

impl ExposureParams {
    /// EV から作る（ローカル補正なし）。
    pub fn new(ev: f32) -> Self {
        Self {
            ev,
            gain: f64::from(ev).exp2() as f32,
            local: None,
        }
    }

    /// 局所の値 `v` を当てた 1 画素（モジュールの doc の「ローカル補正」）。
    #[inline]
    pub fn apply_local(&self, c: [f32; 3], v: &LocalValues) -> [f32; 3] {
        let dt = v[LocalParam::Temperature.index()];
        let di = v[LocalParam::Tint.index()];
        let de = v[LocalParam::Exposure.index()];
        let k = if de == 0.0 {
            self.gain
        } else {
            (self.ev + de)
                .clamp(-LOCAL_EXPOSURE_LIMIT, LOCAL_EXPOSURE_LIMIT)
                .exp2()
        };
        if dt == 0.0 && di == 0.0 {
            return [c[0] * k, c[1] * k, c[2] * k];
        }
        let g = white_balance_gains(dt, di);
        [c[0] * (k * g[0]), c[1] * (k * g[1]), c[2] * (k * g[2])]
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
            ProcessVersion::V1 | ProcessVersion::V2 => {
                let s = settings.normalized();
                let local =
                    local_slots(&s, ctx).filter(|l| l.uses_any(&LocalParam::EXPOSURE_STAGE));
                let ev = s.exposure_ev;
                Ok((ev != 0.0 || local.is_some()).then(|| {
                    StageParams::new(ExposureParams {
                        local,
                        ..ExposureParams::new(ev)
                    })
                }))
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
            ProcessVersion::V1 | ProcessVersion::V2 => {}
        }
        let k = p.gain;
        match checked_field(ctx, Self::ID, p.local, output.roi)? {
            None => map_pixels(input, output, |_, _, c| [c[0] * k, c[1] * k, c[2] * k]),
            Some(f) => map_pixels(input, output, |x, y, c| p.apply_local(c, &f.values(x, y))),
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
        Self::from_unit(contrast / 100.0)
    }

    /// スライダーの値 / 100（ローカル補正の和は −2〜2）から作る: `γ = 1 + CONTRAST_STRENGTH × unit`。
    pub fn from_unit(unit: f32) -> Self {
        Self {
            gamma: 1.0 + CONTRAST_STRENGTH * unit,
        }
    }

    /// 全体のコントラスト（スライダーの値 / 100）`unit` に局所の値 `dc`（スライダーの値）を足したもの（`dc` が 0 なら
    /// `self`）。
    #[inline]
    pub fn with_local(&self, unit: f32, dc: f32) -> Self {
        if dc == 0.0 {
            *self
        } else {
            Self::from_unit(add_unit(unit, dc, -2.0, 2.0))
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

    /// 1 画素に適用する（モジュールの doc の式）。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let y = luminance(c);
        let l = safe_log2(y);
        let r = (self.apply_log2(l) - l).exp2();
        let k = if y < MIDDLE_GRAY {
            r.min(CONTRAST_CHROMA_GAIN_MAX)
        } else {
            r
        };
        if k == r {
            return [c[0] * r, c[1] * r, c[2] * r];
        }
        let y2 = y * r;
        [
            y2 + k * (c[0] - y),
            y2 + k * (c[1] - y),
            y2 + k * (c[2] - y),
        ]
    }
}

/// コントラストのステージのパラメータ（全体の値と、ローカル補正）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContrastStageParams {
    /// 全体のコントラスト。
    pub contrast: ContrastParams,
    /// 全体のコントラストのスライダーの値 / 100。
    pub unit: f32,
    /// ローカル補正の局所の値の面の割り当て（コントラストの面があるときだけ）。
    pub local: Option<LocalSlots>,
    /// 全体のコントラストが 0 で、ローカル補正だけで有効か（局所の値が 0 の画素は変えない）。
    pub local_only: bool,
}

impl ContrastStageParams {
    /// 局所の値 `v` を当てた 1 画素（モジュールの doc の「ローカル補正」）。
    #[inline]
    pub fn apply_local(&self, c: [f32; 3], v: &LocalValues) -> [f32; 3] {
        let dc = v[LocalParam::Contrast.index()];
        if dc == 0.0 {
            if self.local_only {
                return c;
            }
            return self.contrast.apply(c);
        }
        self.contrast.with_local(self.unit, dc).apply(c)
    }
}

/// コントラスト（ステージ 10 の後、11 の前。仮置き）。コントラストが 0 なら飛ばす（ローカル補正のコントラストが
/// あれば有効）。
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
            ProcessVersion::V1 | ProcessVersion::V2 => {
                let s = settings.normalized();
                let p = ContrastParams::from_slider(s.contrast);
                let local = local_slots(&s, ctx).filter(|l| l.uses(LocalParam::Contrast));
                Ok((!p.is_identity() || local.is_some()).then(|| {
                    StageParams::new(ContrastStageParams {
                        contrast: p,
                        unit: s.contrast / 100.0,
                        local,
                        local_only: p.is_identity(),
                    })
                }))
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
        let p = *params.expect::<ContrastStageParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 | ProcessVersion::V2 => {}
        }
        match checked_field(ctx, Self::ID, p.local, output.roi)? {
            None => map_pixels(input, output, |_, _, c| p.contrast.apply(c)),
            Some(f) => map_pixels(input, output, |x, y, c| p.apply_local(c, &f.values(x, y))),
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
    fn negative_contrast_does_not_amplify_dark_noise_colors_by_the_luminance_gain() {
        // 人の判断 No.18 の再現: コントラスト −100 では暗いほど輝度の倍率が大きく（2^−16 以下で約 42 倍）、
        // 比率を保って RGB に掛けると、暗部の雑音の色の成分（無彩色からの差）も同じ倍率で増えていた。
        let p = ContrastParams::from_slider(-100.0);
        let noise = [1.0e-3f32, -4.0e-4, 1.0e-3];
        let y = luminance(noise);
        let out = p.apply(noise);
        let gain = (p.apply_log2(safe_log2(y)) - safe_log2(y)).exp2();
        assert!(gain > 20.0, "{gain}");
        // 輝度は輝度の倍率どおり。
        assert!(
            (luminance(out) - y * gain).abs() <= 1e-6 * gain * y.abs().max(1e-6),
            "{out:?}"
        );
        // 色の成分の倍率は上限（CONTRAST_CHROMA_GAIN_MAX）まで。
        let chroma_in = (noise[0] - y).abs();
        let chroma_out = (out[0] - luminance(out)).abs();
        let k = chroma_out / chroma_in;
        assert!(
            (k - CONTRAST_CHROMA_GAIN_MAX).abs() < 1e-3,
            "色の成分の倍率 {k}"
        );
        // 中間グレーより明るい部分と、倍率が上限より小さい暗部では、これまでどおり比率を保つ。
        let bright = p.apply([0.6, 0.4, 0.3]);
        assert!((bright[0] / bright[1] - 1.5).abs() < 1e-5, "{bright:?}");
        let mid_dark = [0.06, 0.04, 0.03];
        let out = p.apply(mid_dark);
        assert!((out[0] / out[1] - 1.5).abs() < 1e-5, "{out:?}");
        // 正のコントラストの暗部（倍率 < 1）も比率を保つ。
        let q = ContrastParams::from_slider(100.0);
        let out = q.apply([0.006, 0.004, 0.003]);
        assert!((out[0] / out[1] - 1.5).abs() < 1e-5, "{out:?}");
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
