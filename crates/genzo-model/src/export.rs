//! 書き出しの設定（EXP-01・EXP-04、docs/04_architecture.md の 2.4 節・2.6 節の B4b・6.4 節）。

use serde::{Deserialize, Serialize};

/// 書き出しのファイル形式（EXP-01）。
///
/// JSON では `{"jpeg": {"quality": 90}}`、`"tiff16"`、`"png8"`、`"png16"` のように表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// JPEG（8bit）。
    Jpeg {
        /// 品質（1〜100）。
        quality: u8,
    },
    /// TIFF（16bit）。
    Tiff16,
    /// PNG（8bit）。
    Png8,
    /// PNG（16bit）。
    Png16,
}

impl ExportFormat {
    /// JPEG の品質の既定値。
    pub const DEFAULT_JPEG_QUALITY: u8 = 90;

    /// ファイルの拡張子（小文字、ドットなし）。
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Jpeg { .. } => "jpg",
            Self::Tiff16 => "tif",
            Self::Png8 | Self::Png16 => "png",
        }
    }

    /// 1 チャンネルあたりのビット数。
    pub const fn bits_per_channel(self) -> u8 {
        match self {
            Self::Jpeg { .. } | Self::Png8 => 8,
            Self::Tiff16 | Self::Png16 => 16,
        }
    }
}

impl Default for ExportFormat {
    fn default() -> Self {
        Self::Jpeg {
            quality: Self::DEFAULT_JPEG_QUALITY,
        }
    }
}

/// 書き出しの色空間（04 の 2.6 節の B4b）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputColorSpace {
    /// sRGB（IEC 61966-2-1）。
    #[default]
    Srgb,
    /// Display P3（D65、伝達関数は IEC 61966-2-1）。
    DisplayP3,
    /// Adobe RGB (1998)（ガンマ 563/256）。
    AdobeRgb,
}

/// 書き出しの寸法。
///
/// JSON では `"original"`、`{"long_edge": 2048}` のように表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportSize {
    /// 元の寸法（切り抜いた後）。
    #[default]
    Original,
    /// 長辺を指定した画素数にする（拡大はしない）。
    LongEdge(u32),
}

impl ExportSize {
    /// 長辺の画素数の上限（JPEG の寸法の上限）。
    pub const MAX_LONG_EDGE: u32 = 65_535;

    /// `width` × `height` の画像を書き出すときの寸法を求める。縦横比を保ち、拡大はしない。
    pub fn fit(self, width: u32, height: u32) -> (u32, u32) {
        let long = width.max(height);
        match self {
            Self::Original => (width, height),
            Self::LongEdge(target) if target >= long || long == 0 => (width, height),
            Self::LongEdge(target) => {
                let scale = |v: u32| -> u32 {
                    let scaled =
                        (u64::from(v) * u64::from(target) + u64::from(long) / 2) / u64::from(long);
                    // 縦横比が極端な場合も 0 にはしない。scaled ≤ target なので u32 に収まる。
                    (scaled as u32).max(1)
                };
                (scale(width), scale(height))
            }
        }
    }
}

/// 既存のファイルと名前が衝突したときの扱い（04 の 6.4 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// 連番を付けて別の名前にする（既定）。
    #[default]
    Sequence,
    /// 上書きする（原本の照合は必ず行う）。
    Overwrite,
    /// 書き出さずに飛ばす。
    Skip,
}

/// 書き出しの設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    /// ファイル形式。
    pub format: ExportFormat,
    /// 色空間。
    pub color_space: OutputColorSpace,
    /// 寸法。
    pub size: ExportSize,
    /// GPS 情報を削除する（EXP-04・SEC-03）。
    pub remove_gps: bool,
    /// 既存のファイルとの衝突の扱い。
    pub on_conflict: ConflictPolicy,
}

/// 書き出しの設定の検証エラー。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExportSettingsError {
    /// JPEG の品質が 1〜100 ではない。
    #[error("JPEG の品質は 1〜100 で指定してください（{0}）")]
    InvalidJpegQuality(u8),
    /// 長辺の画素数が範囲外。
    #[error("長辺の画素数は 1〜{max} で指定してください（{value}）", max = ExportSize::MAX_LONG_EDGE)]
    InvalidLongEdge {
        /// 指定された値。
        value: u32,
    },
}

impl ExportSettings {
    /// 値を検証する。
    pub fn validate(&self) -> Result<(), ExportSettingsError> {
        if let ExportFormat::Jpeg { quality } = self.format
            && !(1..=100).contains(&quality)
        {
            return Err(ExportSettingsError::InvalidJpegQuality(quality));
        }
        if let ExportSize::LongEdge(value) = self.size
            && !(1..=ExportSize::MAX_LONG_EDGE).contains(&value)
        {
            return Err(ExportSettingsError::InvalidLongEdge { value });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let s = ExportSettings::default();
        assert_eq!(s.format, ExportFormat::Jpeg { quality: 90 });
        assert_eq!(s.color_space, OutputColorSpace::Srgb);
        assert_eq!(s.size, ExportSize::Original);
        assert!(!s.remove_gps);
        assert_eq!(s.on_conflict, ConflictPolicy::Sequence);
        s.validate().unwrap();
    }

    #[test]
    fn json_forms() {
        let s = ExportSettings {
            format: ExportFormat::Png16,
            color_space: OutputColorSpace::DisplayP3,
            size: ExportSize::LongEdge(2048),
            remove_gps: true,
            on_conflict: ConflictPolicy::Skip,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            json,
            r#"{"format":"png16","color_space":"display_p3","size":{"long_edge":2048},"remove_gps":true,"on_conflict":"skip"}"#
        );
        assert_eq!(serde_json::from_str::<ExportSettings>(&json).unwrap(), s);
        let partial: ExportSettings =
            serde_json::from_str(r#"{"format":{"jpeg":{"quality":75}}}"#).unwrap();
        assert_eq!(partial.format, ExportFormat::Jpeg { quality: 75 });
        assert_eq!(partial.on_conflict, ConflictPolicy::Sequence);
    }

    #[test]
    fn validation() {
        let mut s = ExportSettings {
            format: ExportFormat::Jpeg { quality: 0 },
            ..Default::default()
        };
        assert_eq!(
            s.validate(),
            Err(ExportSettingsError::InvalidJpegQuality(0))
        );
        s.format = ExportFormat::Jpeg { quality: 101 };
        assert!(s.validate().is_err());
        s.format = ExportFormat::Tiff16;
        s.size = ExportSize::LongEdge(0);
        assert_eq!(
            s.validate(),
            Err(ExportSettingsError::InvalidLongEdge { value: 0 })
        );
        s.size = ExportSize::LongEdge(70_000);
        assert!(s.validate().is_err());
        s.size = ExportSize::LongEdge(4000);
        s.validate().unwrap();
    }

    #[test]
    fn size_fit_keeps_aspect_and_does_not_upscale() {
        assert_eq!(ExportSize::Original.fit(7008, 4672), (7008, 4672));
        assert_eq!(ExportSize::LongEdge(2048).fit(7008, 4672), (2048, 1365));
        assert_eq!(ExportSize::LongEdge(2048).fit(4672, 7008), (1365, 2048));
        assert_eq!(ExportSize::LongEdge(10_000).fit(7008, 4672), (7008, 4672));
        assert_eq!(ExportSize::LongEdge(100).fit(10_000, 10), (100, 1));
        assert_eq!(ExportSize::LongEdge(100).fit(0, 0), (0, 0));
    }

    #[test]
    fn format_properties() {
        assert_eq!(ExportFormat::default().extension(), "jpg");
        assert_eq!(ExportFormat::Tiff16.extension(), "tif");
        assert_eq!(ExportFormat::Png8.bits_per_channel(), 8);
        assert_eq!(ExportFormat::Png16.bits_per_channel(), 16);
    }
}
