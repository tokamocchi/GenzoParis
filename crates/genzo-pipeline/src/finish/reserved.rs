//! v1 で実装するステージの予約（docs/04_architecture.md の 2.1 節のステージ 6・7・12・14、
//! 4: ハイライト復元は 1 段目の仮の処理）。
//!
//! | ステージ | ID | 段階 | 現像設定の項目 | 処理バージョン v1 での扱い |
//! |---|---|---|---|---|
//! | 6. ノイズ軽減 | [`NOISE_REDUCTION_ID`] | A1 | `detail.noise_reduction` | 適用しない |
//! | 7. レンズ補正（周辺減光・倍率色収差） | [`LENS_CORRECTION_ID`] | A1 | `lens.profile_enabled`・`lens.vignetting_scale`・`lens.remove_chromatic_aberration` | 適用しない |
//! | 9. の歪曲補正 | （ステージ 9 の一部） | C（B にも影響） | `lens.profile_enabled`・`lens.distortion_scale` | 適用しない |
//! | 12. テクスチャ・明瞭度・かすみの除去 | [`TextureStage::ID`] | C | （設定の項目がまだない） | 何もしない |
//! | 14. シャープ | [`SharpenStage::ID`] | C | `detail.sharpening` | 適用しない |
//! | ローカル補正（マスク） | — | C | `masks` | 適用しない |
//!
//! **決めごと**: 処理バージョン v1 は、これらの設定に値があっても **無視して** 処理を続ける
//! （ステージの `params` は常に `None`）。エラーにしないのは、センサー処理（1 段目）が
//! ノイズ軽減・レンズ補正を同じように無視しているのと合わせ、写真を開けなくしないため。値が
//! 入っていることは [`unimplemented_settings`] で調べられるので、エンジン・UI は警告を出せる。
//! これらを実装するときは、既存の写真の結果を変えないよう **処理バージョンを上げる**（2.5 節）。
//!
//! 6・7 はセンサーのステージ（入出力が CFA・カメラ RGB）なので、ここでは ID だけを予約する
//! （[`crate::sensor`] に実装する）。12・14 は [`Stage`] として登録表に載せ、並び順と ID を固定する。

use std::fmt;

use genzo_model::{DevelopSettings, LocalAdjustment, Phase};

use super::map_pixels;
use crate::contract::ColorContract;
use crate::error::Result;
use crate::image::{ImageTile, Roi};
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// ステージ 6（ノイズ軽減。v1）の予約の ID。
pub const NOISE_REDUCTION_ID: &str = "sensor.noise_reduction";
/// ステージ 7（レンズ補正の周辺減光・倍率色収差。v1）の予約の ID。
pub const LENS_CORRECTION_ID: &str = "sensor.lens_correction";

/// 処理バージョン v1 では適用しない（値があっても無視する）現像設定の項目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnimplementedSetting {
    /// ノイズ軽減（ステージ 6）。
    NoiseReduction,
    /// レンズのプロファイルによる補正（ステージ 7 の周辺減光・ステージ 9 の歪曲補正）。
    LensProfile,
    /// 倍率色収差の除去（ステージ 7）。
    ChromaticAberration,
    /// シャープ（ステージ 14）。
    Sharpening,
    /// ローカル補正（マスク）。
    Masks,
}

impl UnimplementedSetting {
    /// 関係するステージの ID。
    pub const fn stage_id(self) -> &'static str {
        match self {
            Self::NoiseReduction => NOISE_REDUCTION_ID,
            Self::LensProfile | Self::ChromaticAberration => LENS_CORRECTION_ID,
            Self::Sharpening => SharpenStage::ID,
            Self::Masks => "finish.masks",
        }
    }
}

impl fmt::Display for UnimplementedSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoiseReduction => "ノイズ軽減はこの処理バージョンでは適用されません",
            Self::LensProfile => {
                "レンズのプロファイルによる補正はこの処理バージョンでは適用されません"
            }
            Self::ChromaticAberration => "倍率色収差の除去はこの処理バージョンでは適用されません",
            Self::Sharpening => "シャープはこの処理バージョンでは適用されません",
            Self::Masks => "ローカル補正（マスク）はこの処理バージョンでは適用されません",
        })
    }
}

/// 値が入っているが、処理バージョン v1 では適用しない項目の一覧（丸めた設定で判定する）。
pub fn unimplemented_settings(settings: &DevelopSettings) -> Vec<UnimplementedSetting> {
    let s = settings.normalized();
    let mut out = Vec::new();
    let nr = s.detail.noise_reduction;
    if nr.luminance != 0.0 || nr.color != 0.0 {
        out.push(UnimplementedSetting::NoiseReduction);
    }
    if s.lens.profile_enabled {
        out.push(UnimplementedSetting::LensProfile);
    }
    if s.lens.remove_chromatic_aberration {
        out.push(UnimplementedSetting::ChromaticAberration);
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
            ProcessVersion::V1 => Ok(None),
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
            ProcessVersion::V1 => Ok(None),
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
        let mut s = DevelopSettings::default();
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
        let u = unimplemented_settings(&s);
        assert_eq!(
            u,
            [
                UnimplementedSetting::NoiseReduction,
                UnimplementedSetting::LensProfile,
                UnimplementedSetting::ChromaticAberration,
                UnimplementedSetting::Sharpening,
                UnimplementedSetting::Masks,
            ]
        );
        assert_eq!(u[0].stage_id(), NOISE_REDUCTION_ID);
        assert_eq!(u[3].stage_id(), SharpenStage::ID);
        assert!(u[0].to_string().contains("ノイズ軽減"));
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
