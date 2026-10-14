//! v1 のステージの ID と、処理バージョンで適用しない設定の一覧（docs/04_architecture.md の 2.1 節のステージ 4・6・7・
//! 12・13・14・16 とローカル補正）。v1 の機能のステージは処理バージョン 2 で実装した（下の表）。
//!
//! | ステージ | ID | 段階 | 現像設定の項目 | 処理バージョン 1 | 処理バージョン 2 |
//! |---|---|---|---|---|---|
//! | 4. ハイライト復元 | [`HIGHLIGHT_RECOVERY_ID`] | A1 | `highlight_recovery` | 適用しない（ステージ 3 の飽和の切りそろえ） | 適用する（復元する方式のとき。`crate::sensor::highlight`） |
//! | 6. ノイズ軽減 | [`NOISE_REDUCTION_ID`] | A1 | `detail.noise_reduction` | 適用しない | 適用する（`crate::sensor::denoise`） |
//! | 7. レンズ補正（周辺減光・倍率色収差） | [`LENS_CORRECTION_ID`] | A1 | `lens.profile_enabled`・`lens.vignetting_scale`・`lens.remove_chromatic_aberration`・手動の周辺減光・色収差 | 適用しない | 適用する（[`crate::lens`]・`crate::sensor::LensCorrectionStage`） |
//! | 9. の歪曲補正 | （ステージ 9 の一部） | C（B にも影響） | `lens.profile_enabled`・`lens.distortion_scale`・`lens.distortion` | 適用しない | 適用する（[`crate::lens::RadialDistortion`]） |
//! | 12. テクスチャ・明瞭度・かすみの除去 | [`super::TextureStage::ID`] | C（明瞭度・かすみの除去は B のガイドを使う） | `presence` | 適用しない | 適用する（[`super::presence`]） |
//! | 13. のカラーミキサー・カラーグレーディング・白黒 | [`super::ColorStage::ID`] | C | `color.mixer`・`color.grading`・`color.black_white` | 適用しない（彩度・自然な彩度だけ） | 適用する（[`super::color`]） |
//! | 14. シャープ | [`super::SharpenStage::ID`] | C | `detail.sharpening` | 適用しない | 適用する（[`super::sharpen`]） |
//! | 16. の効果（切り抜き後の周辺光量補正・粒子） | [`super::EffectsStage::ID`] | C | `effects` | 適用しない | 適用する（[`super::effects`]） |
//! | 8. のカメラプロファイル（DCP）と 10 の後のルックテーブル | `sensor.color_matrix`・[`super::ProfileLookTableStage::ID`] | A1・C | `profile.camera` | 適用しない（カメラ行列） | 適用する（[`crate::profile`]・[`super::look`]） |
//! | 15 の後の独自のルック（3D LUT） | [`super::LookStage::ID`] | C | `profile.look`・`profile.look_amount` | 適用しない | 適用する（[`super::look`]） |
//! | ローカル補正（マスク） | （ステージ 10〜14 の画素ごとの値） | C | `masks` | 適用しない | 適用する（[`crate::local`]） |
//!
//! **決めごと**: 処理バージョン 1 は、これらの設定に値があっても **無視して** 処理を続ける
//! （ステージの `params` は常に `None`）。エラーにしないのは、センサー処理（1 段目）が
//! ノイズ軽減・レンズ補正を同じように無視しているのと合わせ、写真を開けなくしないため。値が
//! 入っていることは [`unimplemented_settings`] で調べられるので、エンジン・UI は警告を出せる。
//! これらを実装するときは、既存の写真の結果を変えないよう **処理バージョンを上げる**（2.5 節）。
//!
//! 4・6・7 はセンサーのステージ（入出力が CFA・カメラ RGB）なので、ここでは ID だけを置く
//! （実装は [`crate::sensor`]・[`crate::lens`]）。

use std::fmt;

use genzo_model::{DevelopSettings, LocalAdjustment};

use super::{EffectsStage, SharpenStage, TextureStage};
use crate::version::ProcessVersion;

/// ステージ 4（ハイライト復元。v1。処理バージョン 2 で適用する）の ID。
pub const HIGHLIGHT_RECOVERY_ID: &str = "sensor.highlight_recovery";
/// ステージ 6（ノイズ軽減。v1。処理バージョン 2 で適用する）の ID。
pub const NOISE_REDUCTION_ID: &str = "sensor.noise_reduction";
/// ステージ 7（レンズ補正の周辺減光・倍率色収差。v1。処理バージョン 2 で適用する）の ID。
pub const LENS_CORRECTION_ID: &str = "sensor.lens_correction";

/// 設定の処理バージョンでは適用しない（値があっても無視する）現像設定の項目（処理バージョン 1 だけ）。
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
    /// HSL / カラーミキサー（ステージ 13）。
    ColorMixer,
    /// カラーグレーディング（ステージ 13）。
    ColorGrading,
    /// 白黒（ステージ 13）。
    BlackWhite,
    /// テクスチャ・明瞭度・かすみの除去（ステージ 12）。
    Presence,
    /// シャープ（ステージ 14）。
    Sharpening,
    /// 切り抜き後の周辺光量補正（ステージ 16 の効果）。
    PostCropVignette,
    /// 粒子（ステージ 16 の効果）。
    Grain,
    /// カメラプロファイル（DCP。ステージ 8 とルックテーブル）。
    CameraProfile,
    /// 独自のルック（3D LUT）。
    Look,
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
            Self::ColorMixer | Self::ColorGrading | Self::BlackWhite => super::ColorStage::ID,
            Self::Presence => TextureStage::ID,
            Self::Sharpening => SharpenStage::ID,
            Self::PostCropVignette | Self::Grain => EffectsStage::ID,
            Self::CameraProfile => "sensor.color_matrix",
            Self::Look => super::LookStage::ID,
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
            Self::ColorMixer => {
                "カラーミキサー（HSL）はこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::ColorGrading => {
                "カラーグレーディングはこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::BlackWhite => {
                "白黒はこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::Presence => {
                "テクスチャ・明瞭度・かすみの除去はこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::Sharpening => {
                "シャープはこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::PostCropVignette => {
                "切り抜き後の周辺光量補正はこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::Grain => {
                "粒子はこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::CameraProfile => {
                "カメラプロファイルはこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::Look => {
                "ルックはこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
            Self::Masks => {
                "ローカル補正（マスク）はこの処理バージョンでは適用されません（処理バージョンを更新すると使えます）"
            }
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
        if !s.color.mixer.is_neutral() {
            out.push(UnimplementedSetting::ColorMixer);
        }
        if !s.color.grading.is_neutral() {
            out.push(UnimplementedSetting::ColorGrading);
        }
        if s.color.black_white.enabled {
            out.push(UnimplementedSetting::BlackWhite);
        }
        if !s.presence.is_neutral() {
            out.push(UnimplementedSetting::Presence);
        }
        if !s.detail.sharpening.is_disabled() {
            out.push(UnimplementedSetting::Sharpening);
        }
        if !s.effects.vignette.is_neutral() {
            out.push(UnimplementedSetting::PostCropVignette);
        }
        if !s.effects.grain.is_neutral() {
            out.push(UnimplementedSetting::Grain);
        }
        if s.profile.camera.is_some() {
            out.push(UnimplementedSetting::CameraProfile);
        }
        if s.profile.look_is_active() {
            out.push(UnimplementedSetting::Look);
        }
        if !s.masks.iter().all(LocalAdjustment::is_neutral) {
            out.push(UnimplementedSetting::Masks);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use genzo_model::{LocalDelta, MaskComponent, MaskShape};

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
            delta: LocalDelta {
                exposure_ev: 0.5,
                ..Default::default()
            },
            ..LocalAdjustment::new(MaskComponent::add(MaskShape::Linear {
                x0: 0.0,
                y0: 0.0,
                x1: 0.0,
                y1: 1.0,
            }))
        });
        s.lens.vignetting_amount = 20.0;
        s.highlight_recovery.method = genzo_model::HighlightMethod::Reconstruct;
        s.color.mixer.blue.luminance = -20.0;
        s.color.grading.shadows.saturation = 10.0;
        s.color.black_white.enabled = true;
        s.presence.clarity = 10.0;
        s.effects.vignette.amount = -20.0;
        s.effects.grain.amount = 30.0;
        let u = unimplemented_settings(&s);
        assert_eq!(
            u,
            [
                UnimplementedSetting::HighlightRecovery,
                UnimplementedSetting::NoiseReduction,
                UnimplementedSetting::LensProfile,
                UnimplementedSetting::ChromaticAberration,
                UnimplementedSetting::LensManual,
                UnimplementedSetting::ColorMixer,
                UnimplementedSetting::ColorGrading,
                UnimplementedSetting::BlackWhite,
                UnimplementedSetting::Presence,
                UnimplementedSetting::Sharpening,
                UnimplementedSetting::PostCropVignette,
                UnimplementedSetting::Grain,
                UnimplementedSetting::Masks,
            ]
        );
        assert_eq!(u[8].stage_id(), TextureStage::ID);
        assert_eq!(u[10].stage_id(), EffectsStage::ID);
        assert_eq!(u[11].stage_id(), EffectsStage::ID);
        assert!(u[8].to_string().contains("明瞭度"));
        assert!(u[9].to_string().contains("処理バージョンを更新"));
        assert!(u[10].to_string().contains("周辺光量"));
        assert!(u[11].to_string().contains("粒子"));
        assert_eq!(u[5].stage_id(), super::super::ColorStage::ID);
        assert!(u[5].to_string().contains("カラーミキサー"));
        assert!(u[6].to_string().contains("カラーグレーディング"));
        assert!(u[7].to_string().contains("白黒"));
        assert!(u[7].to_string().contains("処理バージョンを更新"));
        assert_eq!(u[0].stage_id(), HIGHLIGHT_RECOVERY_ID);
        assert_eq!(u[1].stage_id(), NOISE_REDUCTION_ID);
        assert_eq!(u[4].stage_id(), LENS_CORRECTION_ID);
        assert_eq!(u[9].stage_id(), SharpenStage::ID);
        assert!(u[0].to_string().contains("ハイライト復元"));
        assert!(u[1].to_string().contains("ノイズ軽減"));
        assert!(u[1].to_string().contains("処理バージョンを更新"));
        assert!(u[4].to_string().contains("処理バージョンを更新"));
        assert!(u[12].to_string().contains("ローカル補正"));
        assert!(u[12].to_string().contains("処理バージョンを更新"));
        // 処理バージョン 2 ではすべて適用する（マスクも。DEV-18）。
        s.process_version = 2;
        assert!(unimplemented_settings(&s).is_empty());
        // 版 1 でも、復元しない設定（量 0）・量 0 のノイズ軽減は知らせない。
        let mut quiet = DevelopSettings {
            process_version: 1,
            ..Default::default()
        };
        quiet.highlight_recovery.method = genzo_model::HighlightMethod::Neutral;
        quiet.highlight_recovery.amount = 0.0;
        quiet.detail.noise_reduction.luminance_detail = 0.0;
        // 強さ 0 の色相・カラーのときの白黒ミキサー・ブレンドとバランスも知らせない（効かない）。
        quiet.color.grading.midtones.hue = 90.0;
        quiet.color.grading.blending = 10.0;
        quiet.color.black_white.mix.red = 30.0;
        // 量が 0 のシャープ・周辺光量補正・粒子のほかの値も知らせない。
        quiet.detail.sharpening.radius = 2.0;
        quiet.effects.vignette.midpoint = 10.0;
        quiet.effects.grain.size = 80.0;
        assert!(unimplemented_settings(&quiet).is_empty());
    }

    #[test]
    fn v1_features_are_skipped_in_v1() {
        use crate::stage::Stage;
        let mut s = DevelopSettings::default();
        s.detail.sharpening.amount = 100.0;
        s.presence.texture = 50.0;
        s.effects.grain.amount = 50.0;
        let ctx = crate::stage::tests::ctx();
        assert_eq!(ctx.process_version, ProcessVersion::V1);
        assert!(SharpenStage.params(&s, &ctx).unwrap().is_none());
        assert!(TextureStage.params(&s, &ctx).unwrap().is_none());
        assert!(EffectsStage.params(&s, &ctx).unwrap().is_none());
    }
}
