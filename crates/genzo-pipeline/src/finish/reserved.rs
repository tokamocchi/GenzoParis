//! v1 のステージの ID と予約（docs/04_architecture.md の 2.1 節のステージ 4・6・7・12・14）。4・6・7 と歪曲補正は
//! 処理バージョン 2 で実装した（下の表）。12・14・マスクはまだ予約。
//!
//! | ステージ | ID | 段階 | 現像設定の項目 | 処理バージョン 1 | 処理バージョン 2 |
//! |---|---|---|---|---|---|
//! | 4. ハイライト復元 | [`HIGHLIGHT_RECOVERY_ID`] | A1 | `highlight_recovery` | 適用しない（ステージ 3 の飽和の切りそろえ） | 適用する（復元する方式のとき。`crate::sensor::highlight`） |
//! | 6. ノイズ軽減 | [`NOISE_REDUCTION_ID`] | A1 | `detail.noise_reduction` | 適用しない | 適用する（`crate::sensor::denoise`） |
//! | 7. レンズ補正（周辺減光・倍率色収差） | [`LENS_CORRECTION_ID`] | A1 | `lens.profile_enabled`・`lens.vignetting_scale`・`lens.remove_chromatic_aberration`・手動の周辺減光・色収差 | 適用しない | 適用する（[`crate::lens`]・`crate::sensor::LensCorrectionStage`） |
//! | 9. の歪曲補正 | （ステージ 9 の一部） | C（B にも影響） | `lens.profile_enabled`・`lens.distortion_scale`・`lens.distortion` | 適用しない | 適用する（[`crate::lens::RadialDistortion`]） |
//! | 12. テクスチャ・明瞭度・かすみの除去 | [`TextureStage::ID`] | C | （設定の項目がまだない） | 何もしない | 何もしない（予約） |
//! | 14. シャープ | [`SharpenStage::ID`] | C | `detail.sharpening` | 適用しない | 適用しない（予約） |
//! | ローカル補正（マスク） | — | C | `masks` | 適用しない | 適用しない（予約） |
//!
//! **決めごと**: 処理バージョン 1 は、これらの設定に値があっても **無視して** 処理を続ける
//! （ステージの `params` は常に `None`。処理バージョン 2 でも、12・14・マスクは同じ）。エラーにしないのは、センサー処理（1 段目）が
//! ノイズ軽減・レンズ補正を同じように無視しているのと合わせ、写真を開けなくしないため。値が
//! 入っていることは [`unimplemented_settings`] で調べられるので、エンジン・UI は警告を出せる。
//! これらを実装するときは、既存の写真の結果を変えないよう **処理バージョンを上げる**（2.5 節）。
//!
//! 4・6・7 はセンサーのステージ（入出力が CFA・カメラ RGB）なので、ここでは ID だけを置く
//! （実装は [`crate::sensor`]・[`crate::lens`]）。12・14 は [`Stage`] として登録表に載せ、並び順と ID を固定する。

use std::fmt;

use genzo_model::{DevelopSettings, LocalAdjustment, Phase};

use super::map_pixels;
use crate::contract::ColorContract;
use crate::error::Result;
use crate::image::{ImageTile, Roi};
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// ステージ 4（ハイライト復元。v1。処理バージョン 2 で適用する）の ID。
pub const HIGHLIGHT_RECOVERY_ID: &str = "sensor.highlight_recovery";
/// ステージ 6（ノイズ軽減。v1。処理バージョン 2 で適用する）の ID。
pub const NOISE_REDUCTION_ID: &str = "sensor.noise_reduction";
/// ステージ 7（レンズ補正の周辺減光・倍率色収差。v1。処理バージョン 2 で適用する）の ID。
pub const LENS_CORRECTION_ID: &str = "sensor.lens_correction";

/// 設定の処理バージョンでは適用しない（値があっても無視する）現像設定の項目（ハイライト復元・ノイズ軽減・
/// レンズ補正は処理バージョン 1 だけ、シャープ・マスクはどの版でも）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnimplementedSetting {
    /// ハイライト復元（ステージ 4）。
    HighlightRecovery,
    /// ノイズ軽減（ステージ 6）。
    NoiseReduction,
    /// レンズのプロファイルによる補正（ステージ 7 の周辺減光・ステージ 9 の歪曲補正）。
    LensProfile,
    /// 倍率色収差の除去（ステージ 7）。
    ChromaticAberration,
    /// レンズ補正の手動の補正（歪曲・周辺減光・色収差。ステージ 7・9）。
    LensManual,
    /// シャープ（ステージ 14）。
    Sharpening,
    /// ローカル補正（マスク）。
    Masks,
}

impl UnimplementedSetting {
    /// 関係するステージの ID。
    pub const fn stage_id(self) -> &'static str {
        match self {
            Self::HighlightRecovery => HIGHLIGHT_RECOVERY_ID,
            Self::NoiseReduction => NOISE_REDUCTION_ID,
            Self::LensProfile | Self::ChromaticAberration | Self::LensManual => LENS_CORRECTION_ID,
            Self::Sharpening => SharpenStage::ID,
            Self::Masks => "finish.masks",
        }
    }
}

impl fmt::Display for UnimplementedSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::HighlightRecovery => {
                "ハイライト復元はこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::NoiseReduction => {
                "ノイズ軽減はこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::LensProfile => {
                "レンズのプロファイルによる補正はこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::ChromaticAberration => {
                "倍率色収差の除去はこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::LensManual => {
                "レンズ補正の手動の補正はこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::Sharpening => "シャープはこの処理バージョンでは適用されません",
            Self::Masks => "ローカル補正（マスク）はこの処理バージョンでは適用されません",
        })
    }
}

/// 値が入っているが、設定の処理バージョンでは適用しない項目の一覧（丸めた設定で判定する。モジュールの
/// doc の表。未知の処理バージョンは丸めた版で判定する）。
pub fn unimplemented_settings(settings: &DevelopSettings) -> Vec<UnimplementedSetting> {
    let s = settings.normalized();
    let version = ProcessVersion::from_u32(s.process_version).unwrap_or(ProcessVersion::CURRENT);
    let mut out = Vec::new();
    if !version.has_v1_features() {
        if !s.highlight_recovery.is_disabled() {
            out.push(UnimplementedSetting::HighlightRecovery);
        }
        if !s.detail.noise_reduction.is_disabled() {
            out.push(UnimplementedSetting::NoiseReduction);
        }
        if s.lens.profile_enabled {
            out.push(UnimplementedSetting::LensProfile);
        }
        if s.lens.remove_chromatic_aberration {
            out.push(UnimplementedSetting::ChromaticAberration);
        }
        if s.lens.has_manual() {
            out.push(UnimplementedSetting::LensManual);
        }
    }
    if s.detail.sharpening.amount != 0.0 {
        out.push(UnimplementedSetting::Sharpening);
    }
    if !s.masks.iter().all(LocalAdjustment::is_neutral) {
        out.push(UnimplementedSetting::Masks);
    }
    out
}

/// 予約したステージの共通の実装（v1 では常に飛ばす。`run_cpu` は呼ばれないが、入力を複製する）。
fn passthrough(input: &ImageTile, output: &mut ImageTile) -> Result<()> {
    map_pixels(input, output, |_, _, c| c)
}

/// ステージ 12: テクスチャ・明瞭度・かすみの除去（v1。予約）。
#[derive(Debug, Clone, Copy, Default)]
pub struct TextureStage;

impl TextureStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.texture";
    /// 04 の 2.1 節のステージの番号。
    pub const NUMBER: u8 = 12;
}

impl Stage for TextureStage {
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
    fn params(&self, _: &DevelopSettings, ctx: &StageContext<'_>) -> Result<Option<StageParams>> {
        match ctx.process_version {
            ProcessVersion::V1 | ProcessVersion::V2 => Ok(None),
        }
    }
    fn input_roi(&self, output_roi: Roi, _: &StageParams, _: &StageContext<'_>) -> Roi {
        output_roi
    }
    fn run_cpu(
        &self,
        _ctx: &StageContext<'_>,
        input: &ImageTile,
        output: &mut ImageTile,
        _params: &StageParams,
    ) -> Result<()> {
        passthrough(input, output)
    }
}

/// ステージ 14: シャープ（v1。予約）。
#[derive(Debug, Clone, Copy, Default)]
pub struct SharpenStage;

impl SharpenStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.sharpen";
    /// 04 の 2.1 節のステージの番号。
    pub const NUMBER: u8 = 14;
}

impl Stage for SharpenStage {
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
    fn params(&self, _: &DevelopSettings, ctx: &StageContext<'_>) -> Result<Option<StageParams>> {
        match ctx.process_version {
            ProcessVersion::V1 | ProcessVersion::V2 => Ok(None),
        }
    }
    fn input_roi(&self, output_roi: Roi, _: &StageParams, _: &StageContext<'_>) -> Roi {
        output_roi
    }
    fn run_cpu(
        &self,
        _ctx: &StageContext<'_>,
        input: &ImageTile,
        output: &mut ImageTile,
        _params: &StageParams,
    ) -> Result<()> {
        passthrough(input, output)
    }
}

#[cfg(test)]
mod tests {
    use genzo_model::{LocalDelta, MaskShape};

    use super::*;

    #[test]
    fn defaults_have_nothing_unimplemented() {
        assert!(unimplemented_settings(&DevelopSettings::default()).is_empty());
    }

    #[test]
    fn values_in_v1_items_are_reported() {
        let mut s = DevelopSettings {
            process_version: 1,
            ..Default::default()
        };
        s.detail.noise_reduction.color = 20.0;
        s.detail.sharpening.amount = 40.0;
        s.lens.profile_enabled = true;
        s.lens.remove_chromatic_aberration = true;
        s.masks.push(LocalAdjustment {
            enabled: true,
            mask: MaskShape::Linear {
                x0: 0.0,
                y0: 0.0,
                x1: 0.0,
                y1: 1.0,
            },
            delta: LocalDelta {
                exposure_ev: 0.5,
                ..Default::default()
            },
        });
        s.lens.vignetting_amount = 20.0;
        s.highlight_recovery.method = genzo_model::HighlightMethod::Reconstruct;
        let u = unimplemented_settings(&s);
        assert_eq!(
            u,
            [
                UnimplementedSetting::HighlightRecovery,
                UnimplementedSetting::NoiseReduction,
                UnimplementedSetting::LensProfile,
                UnimplementedSetting::ChromaticAberration,
                UnimplementedSetting::LensManual,
                UnimplementedSetting::Sharpening,
                UnimplementedSetting::Masks,
            ]
        );
        assert_eq!(u[0].stage_id(), HIGHLIGHT_RECOVERY_ID);
        assert_eq!(u[1].stage_id(), NOISE_REDUCTION_ID);
        assert_eq!(u[4].stage_id(), LENS_CORRECTION_ID);
        assert_eq!(u[5].stage_id(), SharpenStage::ID);
        assert!(u[0].to_string().contains("ハイライト復元"));
        assert!(u[1].to_string().contains("ノイズ軽減"));
        assert!(u[1].to_string().contains("処理バージョンを更新"));
        assert!(u[4].to_string().contains("処理バージョンを更新"));
        // 処理バージョン 2 ではハイライト復元・ノイズ軽減・レンズ補正を適用する（ほかの項目はまだ予約のまま）。
        s.process_version = 2;
        assert_eq!(
            unimplemented_settings(&s),
            [
                UnimplementedSetting::Sharpening,
                UnimplementedSetting::Masks
            ]
        );
        // 版 1 でも、復元しない設定（量 0）・量 0 のノイズ軽減は知らせない。
        let mut quiet = DevelopSettings {
            process_version: 1,
            ..Default::default()
        };
        quiet.highlight_recovery.method = genzo_model::HighlightMethod::Neutral;
        quiet.highlight_recovery.amount = 0.0;
        quiet.detail.noise_reduction.luminance_detail = 0.0;
        assert!(unimplemented_settings(&quiet).is_empty());
    }

    #[test]
    fn reserved_stages_are_skipped_in_v1() {
        let mut s = DevelopSettings::default();
        s.detail.sharpening.amount = 100.0;
        let ctx = crate::stage::tests::ctx();
        assert!(SharpenStage.params(&s, &ctx).unwrap().is_none());
        assert!(TextureStage.params(&s, &ctx).unwrap().is_none());
    }
}
