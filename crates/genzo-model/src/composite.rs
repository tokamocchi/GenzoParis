//! 比較明・比較暗合成の設定（DEV-34。docs/composite_design.md、決定事項ログ No.175〜178）。

use serde::{Deserialize, Serialize};

use crate::export::{ExportFormat, ExportSettingsError, OutputColorSpace};

/// 合成の比べ方（docs/composite_design.md の 3.1 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositeMode {
    /// 比較明（チャンネルごとの最大。Photoshop の「比較（明）」）。
    #[default]
    Lighten,
    /// 比較暗（チャンネルごとの最小。Photoshop の「比較（暗）」）。
    Darken,
    /// カラー比較明（輝度が最大の写真の画素をそのまま採る）。
    LighterColor,
    /// カラー比較暗（輝度が最小の写真の画素をそのまま採る）。
    DarkerColor,
}

impl CompositeMode {
    /// ファイル名に使う名前（ASCII）。
    pub const fn file_label(self) -> &'static str {
        match self {
            Self::Lighten => "lighten",
            Self::Darken => "darken",
            Self::LighterColor => "lighter-color",
            Self::DarkerColor => "darker-color",
        }
    }

    /// 表示用の名前。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Lighten => "比較明合成",
            Self::Darken => "比較暗合成",
            Self::LighterColor => "カラー比較明合成",
            Self::DarkerColor => "カラー比較暗合成",
        }
    }

    /// 輝度で比べるモード（カラー比較明・暗）か。
    pub const fn compares_luminance(self) -> bool {
        matches!(self, Self::LighterColor | Self::DarkerColor)
    }
}

/// 合成の設定（docs/composite_design.md の 5 章・6.1 節）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct CompositeSettings {
    /// 比べ方。
    pub mode: CompositeMode,
    /// 形式（既定は TIFF 16bit）。
    pub format: ExportFormat,
    /// 色空間（既定は Adobe RGB (1998)。比べるのもこの色空間の値）。
    pub color_space: OutputColorSpace,
    /// GPS 情報を書かない（EXP-04 と同じ）。
    pub remove_gps: bool,
    /// 合成の元の写真とスタックにまとめ、結果を一番上にする（既定は true）。
    pub stack_with_sources: bool,
}

impl Default for CompositeSettings {
    fn default() -> Self {
        Self {
            mode: CompositeMode::default(),
            format: ExportFormat::Tiff16,
            color_space: OutputColorSpace::AdobeRgb,
            remove_gps: false,
            stack_with_sources: true,
        }
    }
}

impl CompositeSettings {
    /// 合成できる写真の数の下限。
    pub const MIN_SOURCES: usize = 2;
    /// 合成できる写真の数の上限（仮置き。決定事項ログ No.176）。
    pub const MAX_SOURCES: usize = 999;

    /// 値を検証する（JPEG の品質）。
    pub fn validate(&self) -> Result<(), ExportSettingsError> {
        if let ExportFormat::Jpeg { quality } = self.format
            && !(1..=100).contains(&quality)
        {
            return Err(ExportSettingsError::InvalidJpegQuality(quality));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_design() {
        let s = CompositeSettings::default();
        assert_eq!(s.mode, CompositeMode::Lighten);
        assert_eq!(s.format, ExportFormat::Tiff16);
        assert_eq!(s.color_space, OutputColorSpace::AdobeRgb);
        assert!(!s.remove_gps);
        assert!(s.stack_with_sources);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn json_fills_missing_fields_with_defaults() {
        let s: CompositeSettings = serde_json::from_str(r#"{"mode": "darker_color"}"#).unwrap();
        assert_eq!(s.mode, CompositeMode::DarkerColor);
        assert_eq!(s.format, ExportFormat::Tiff16);
        assert!(s.stack_with_sources);
    }

    #[test]
    fn jpeg_quality_is_checked() {
        let s = CompositeSettings {
            format: ExportFormat::Jpeg { quality: 0 },
            ..CompositeSettings::default()
        };
        assert_eq!(
            s.validate(),
            Err(ExportSettingsError::InvalidJpegQuality(0))
        );
    }

    #[test]
    fn file_labels_are_ascii() {
        for m in [
            CompositeMode::Lighten,
            CompositeMode::Darken,
            CompositeMode::LighterColor,
            CompositeMode::DarkerColor,
        ] {
            assert!(m.file_label().is_ascii());
        }
    }
}
