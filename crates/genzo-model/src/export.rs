//! 書き出しの設定（EXP-01・EXP-03・EXP-04・EXP-05、docs/04_architecture.md の 2.4 節・2.6 節の B4b・6.4 節）。
//!
//! - 出力シャープ（EXP-03。[`OutputSharpening`]）と透かし（EXP-03。[`Watermark`]）は、縮小の後の出力の画素に
//!   かける書き出しの仕上げ（現像設定ではないので、処理バージョンに入れない）。印刷用の出力シャープの半径は、
//!   書き出しの解像度（[`ExportSettings::resolution_ppi`]）で出力の画素の大きさを決める。
//! - 動画（EXP-05。[`VideoExport`]）は、再エンコードせずに元のファイルをコピーする。
//!
//! 項目は `#[serde(default)]` で足す（項目のない古い JSON・書き出しプリセットは既定値で読む。決定事項ログ No.163）。

use std::path::{Path, PathBuf};

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

/// 出力シャープの対象（EXP-03。Lightroom Classic の「出力シャープ」の「画面・マット紙・光沢紙」に当たる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharpenMedia {
    /// 画面（Web・SNS など）。半径は出力の画素で決める。
    #[default]
    Screen,
    /// マット紙（インクがにじむので、光沢紙より強く・広くかける）。半径は解像度で出力の画素の大きさから決める。
    MattePaper,
    /// 光沢紙。
    GlossyPaper,
}

impl SharpenMedia {
    /// 印刷用か（半径を解像度から決める）。
    pub const fn is_print(self) -> bool {
        matches!(self, Self::MattePaper | Self::GlossyPaper)
    }
}

/// 出力シャープの適用量（EXP-03）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharpenAmount {
    /// 弱。
    Low,
    /// 標準。
    #[default]
    Standard,
    /// 強。
    High,
}

/// 出力シャープ（EXP-03）。縮小の後に、出力の画素の大きさに合わせてかける（genzo-pipeline の `output_sharpen`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSharpening {
    /// 対象（画面・マット紙・光沢紙）。
    pub media: SharpenMedia,
    /// 適用量（弱・標準・強）。
    pub amount: SharpenAmount,
}

/// 透かしの位置（EXP-03。写真の 9 か所）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatermarkAnchor {
    /// 左上。
    TopLeft,
    /// 上。
    Top,
    /// 右上。
    TopRight,
    /// 左。
    Left,
    /// 中央。
    Center,
    /// 右。
    Right,
    /// 左下。
    BottomLeft,
    /// 下。
    Bottom,
    /// 右下（既定）。
    #[default]
    BottomRight,
}

/// 横・縦の揃え（[`WatermarkAnchor::horizontal`]・[`WatermarkAnchor::vertical`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    /// 左・上。
    Start,
    /// 中央。
    Center,
    /// 右・下。
    End,
}

impl WatermarkAnchor {
    /// すべての位置（左上から右下へ、行ごと）。
    pub const ALL: [WatermarkAnchor; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Center,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
    ];

    /// 横の揃え（文字の透かしの行の揃えにも使う）。
    pub const fn horizontal(self) -> Align {
        match self {
            Self::TopLeft | Self::Left | Self::BottomLeft => Align::Start,
            Self::Top | Self::Center | Self::Bottom => Align::Center,
            Self::TopRight | Self::Right | Self::BottomRight => Align::End,
        }
    }

    /// 縦の揃え。
    pub const fn vertical(self) -> Align {
        match self {
            Self::TopLeft | Self::Top | Self::TopRight => Align::Start,
            Self::Left | Self::Center | Self::Right => Align::Center,
            Self::BottomLeft | Self::Bottom | Self::BottomRight => Align::End,
        }
    }
}

/// sRGB（8bit）の色。JSON では `"#RRGGBB"`（読み込みでは小文字も受け付ける）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RgbColor(pub [u8; 3]);

impl RgbColor {
    /// 白。
    pub const WHITE: RgbColor = RgbColor([255, 255, 255]);

    /// `"#RRGGBB"` を読む（`#` は省略できる）。
    pub fn parse(s: &str) -> Option<Self> {
        let hex = s.trim().strip_prefix('#').unwrap_or(s.trim());
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Self([byte(0)?, byte(2)?, byte(4)?]))
    }

    /// `"#RRGGBB"`（大文字）。
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0[0], self.0[1], self.0[2])
    }
}

impl Default for RgbColor {
    fn default() -> Self {
        Self::WHITE
    }
}

impl Serialize for RgbColor {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for RgbColor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!("色は #RRGGBB の形で指定してください（{s}）"))
        })
    }
}

/// 透かしの中身（EXP-03）。
///
/// JSON では `{"text": {"text": "© 2026 山田", "font": "C:\\Windows\\Fonts\\meiryo.ttc", "font_index": 0,
/// "color": "#FFFFFF"}}`・`{"image": {"path": "/Users/a/logo.png"}}` のように表す。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatermarkContent {
    /// 文字。フォントは利用者の OS のフォントのファイル（TrueType・OpenType・そのコレクション。アプリは同梱しない）。
    Text {
        /// 文字（改行で複数の行。前後の空白は除かない）。
        text: String,
        /// フォントのファイル（絶対パス）。
        font: PathBuf,
        /// コレクション（.ttc）の中の番号（0 から）。
        #[serde(default)]
        font_index: u32,
        /// 色（sRGB。既定は白）。
        #[serde(default)]
        color: RgbColor,
    },
    /// 画像（PNG・JPEG。PNG の透明度を使う。色は埋め込みの ICC プロファイル（なければ sRGB）で解釈する）。
    Image {
        /// 画像のファイル（絶対パス）。
        path: PathBuf,
    },
}

impl WatermarkContent {
    /// 読むファイル（フォントか画像）。
    pub fn file(&self) -> &Path {
        match self {
            Self::Text { font, .. } => font,
            Self::Image { path } => path,
        }
    }

    /// 読むファイル（変えるため）。
    pub fn file_mut(&mut self) -> &mut PathBuf {
        match self {
            Self::Text { font, .. } => font,
            Self::Image { path } => path,
        }
    }
}

/// 透かし（EXP-03）。縮小・出力シャープの後に、出力の画素に重ねる。
///
/// 大きさ・余白は、書き出す画像の **短辺** に対する割合（縦の写真と横の写真で同じ大きさになるように）。写真に
/// 収まらないときは、縦横比を保って縮める。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Watermark {
    /// 中身（文字か画像）。
    pub content: WatermarkContent,
    /// 位置。
    #[serde(default)]
    pub anchor: WatermarkAnchor,
    /// 大きさ（%。文字は文字の大きさ（フォントの em）、画像は幅を、短辺に対する割合で。1〜100）。
    pub size: u8,
    /// 不透明度（%。1〜100）。
    #[serde(default = "Watermark::default_opacity")]
    pub opacity: u8,
    /// 余白（%。写真の端から、短辺に対する割合で。0〜[`Watermark::MAX_MARGIN`]）。
    #[serde(default = "Watermark::default_margin")]
    pub margin: u8,
}

impl Watermark {
    /// 文字の大きさの既定（短辺の 4 %。仮置き）。
    pub const DEFAULT_TEXT_SIZE: u8 = 4;
    /// 画像の幅の既定（短辺の 20 %。仮置き）。
    pub const DEFAULT_IMAGE_SIZE: u8 = 20;
    /// 不透明度の既定（60 %。仮置き）。
    pub const DEFAULT_OPACITY: u8 = 60;
    /// 余白の既定（短辺の 2 %。仮置き）。
    pub const DEFAULT_MARGIN: u8 = 2;
    /// 余白の上限（短辺の 25 %）。
    pub const MAX_MARGIN: u8 = 25;
    /// 文字の数の上限（仮置き）。
    pub const MAX_TEXT_CHARS: usize = 200;
    /// 行の数の上限（仮置き）。
    pub const MAX_TEXT_LINES: usize = 5;
    /// コレクションの中の番号の上限（OpenType のコレクションの数の目安。仮置き）。
    pub const MAX_FONT_INDEX: u32 = 255;

    fn default_opacity() -> u8 {
        Self::DEFAULT_OPACITY
    }

    fn default_margin() -> u8 {
        Self::DEFAULT_MARGIN
    }

    /// 中身に合わせた既定の大きさ・位置・不透明度・余白の透かし。
    pub fn new(content: WatermarkContent) -> Self {
        let size = match content {
            WatermarkContent::Text { .. } => Self::DEFAULT_TEXT_SIZE,
            WatermarkContent::Image { .. } => Self::DEFAULT_IMAGE_SIZE,
        };
        Self {
            content,
            anchor: WatermarkAnchor::default(),
            size,
            opacity: Self::DEFAULT_OPACITY,
            margin: Self::DEFAULT_MARGIN,
        }
    }

    /// 値を確かめる（問題があれば理由。[`Watermark::value_problem`] と [`Watermark::file_problem`]）。
    pub fn problem(&self) -> Option<String> {
        self.value_problem().or_else(|| self.file_problem())
    }

    /// ファイル（フォント・画像）のほかの値を確かめる（問題があれば理由）。
    pub fn value_problem(&self) -> Option<String> {
        if !(1..=100).contains(&self.size) {
            return Some(format!(
                "透かしの大きさは 1〜100 % で指定してください（{}）",
                self.size
            ));
        }
        if !(1..=100).contains(&self.opacity) {
            return Some(format!(
                "透かしの不透明度は 1〜100 % で指定してください（{}）",
                self.opacity
            ));
        }
        if self.margin > Self::MAX_MARGIN {
            return Some(format!(
                "透かしの余白は 0〜{} % で指定してください（{}）",
                Self::MAX_MARGIN,
                self.margin
            ));
        }
        if let WatermarkContent::Text {
            text, font_index, ..
        } = &self.content
        {
            if text.trim().is_empty() {
                return Some("透かしの文字を入れてください".to_owned());
            }
            let chars = text.chars().count();
            if chars > Self::MAX_TEXT_CHARS {
                return Some(format!(
                    "透かしの文字は {} 文字までです（{chars} 文字）",
                    Self::MAX_TEXT_CHARS
                ));
            }
            if text.lines().count() > Self::MAX_TEXT_LINES {
                return Some(format!(
                    "透かしの文字は {} 行までです",
                    Self::MAX_TEXT_LINES
                ));
            }
            if let Some(c) = text.chars().find(|c| c.is_control() && *c != '\n') {
                return Some(format!(
                    "透かしの文字に制御文字（U+{:04X}）は使えません",
                    u32::from(c)
                ));
            }
            if *font_index > Self::MAX_FONT_INDEX {
                return Some(format!(
                    "フォントの番号は 0〜{} で指定してください（{font_index}）",
                    Self::MAX_FONT_INDEX
                ));
            }
        }
        None
    }

    /// ファイル（フォント・画像）を確かめる（選んでいない・絶対パスでなければ理由）。ファイルがあるかは確かめない
    /// （書き出すときにワーカーが読む）。
    pub fn file_problem(&self) -> Option<String> {
        let what = match &self.content {
            WatermarkContent::Text { .. } => "フォントのファイル",
            WatermarkContent::Image { .. } => "透かしの画像のファイル",
        };
        let file = self.content.file();
        if file.as_os_str().is_empty() {
            return Some(format!("{what}を選んでください"));
        }
        if !file.is_absolute() {
            return Some(format!(
                "{what}は絶対パスで指定してください（{}）",
                file.display()
            ));
        }
        None
    }
}

/// 動画の書き出し（EXP-05）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoExport {
    /// 再エンコードせずに元のファイルをそのままコピーする（既定）。
    #[default]
    Copy,
    /// 書き出さない（飛ばす）。
    Skip,
}

/// 書き出しの設定。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    /// ファイル形式。
    pub format: ExportFormat,
    /// 色空間。
    pub color_space: OutputColorSpace,
    /// 寸法。
    pub size: ExportSize,
    /// GPS 情報を削除する（EXP-04・SEC-03）。動画は元のファイルのままコピーするため位置情報を消せないので、
    /// この設定では動画を書き出さない（EXP-05。決定事項ログ No.168）。
    pub remove_gps: bool,
    /// 既存のファイルとの衝突の扱い。
    pub on_conflict: ConflictPolicy,
    /// キーワード（LIB-09。XMP の `dc:subject`）を書かない（既定は書く。キーワードごとの「書き出しに
    /// 含める」の印は、書くときに見る。決定事項ログ No.81）。
    pub remove_keywords: bool,
    /// 解像度（ppi。Exif の XResolution・YResolution（JPEG・TIFF）と、PNG の pHYs に書く。画素数は変えない）。
    /// 印刷用の出力シャープ（EXP-03）の半径を決めるのにも使う。
    pub resolution_ppi: u32,
    /// 出力シャープ（EXP-03。`None` ならかけない）。
    pub output_sharpening: Option<OutputSharpening>,
    /// 透かし（EXP-03。`None` なら付けない）。
    pub watermark: Option<Watermark>,
    /// 動画の扱い（EXP-05）。
    pub videos: VideoExport,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: ExportFormat::default(),
            color_space: OutputColorSpace::default(),
            size: ExportSize::default(),
            remove_gps: false,
            on_conflict: ConflictPolicy::default(),
            remove_keywords: false,
            resolution_ppi: Self::DEFAULT_RESOLUTION_PPI,
            output_sharpening: None,
            watermark: None,
            videos: VideoExport::default(),
        }
    }
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
    /// 解像度が範囲外。
    #[error("解像度は 1〜{max} ppi で指定してください（{value}）", max = ExportSettings::MAX_RESOLUTION_PPI)]
    InvalidResolution {
        /// 指定された値。
        value: u32,
    },
    /// 透かしの設定が正しくない。
    #[error("{0}")]
    InvalidWatermark(String),
}

impl ExportSettings {
    /// 解像度の既定値（ppi。Exif の既定値の 72 dpi。それまでの固定の値と同じ）。
    pub const DEFAULT_RESOLUTION_PPI: u32 = 72;
    /// 解像度の上限（ppi。仮置き）。
    pub const MAX_RESOLUTION_PPI: u32 = 1200;

    /// 値を検証する。
    pub fn validate(&self) -> Result<(), ExportSettingsError> {
        self.validate_values()?;
        if let Some(problem) = self.watermark.as_ref().and_then(Watermark::file_problem) {
            return Err(ExportSettingsError::InvalidWatermark(problem));
        }
        Ok(())
    }

    /// 透かしのファイル（フォント・画像。[`Watermark::file_problem`]）のほかの値を検証する。読み込んだ書き出し
    /// プリセットに使う（別の OS で作ったプリセットのファイルのパスは使わずに空にし、書き出す前に選び直す。genzo-api の
    /// `ExportPresetSettings`）。
    pub fn validate_values(&self) -> Result<(), ExportSettingsError> {
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
        if !(1..=Self::MAX_RESOLUTION_PPI).contains(&self.resolution_ppi) {
            return Err(ExportSettingsError::InvalidResolution {
                value: self.resolution_ppi,
            });
        }
        if let Some(problem) = self.watermark.as_ref().and_then(Watermark::value_problem) {
            return Err(ExportSettingsError::InvalidWatermark(problem));
        }
        Ok(())
    }

    /// 書き出しの仕上げ（出力シャープ・透かし。EXP-03）があるか。
    pub fn has_finishing(&self) -> bool {
        self.output_sharpening.is_some() || self.watermark.is_some()
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
        assert!(!s.remove_keywords, "キーワードは既定で書く");
        assert_eq!(s.resolution_ppi, 72);
        assert_eq!(s.output_sharpening, None);
        assert_eq!(s.watermark, None);
        assert_eq!(s.videos, VideoExport::Copy, "動画は既定でコピーする");
        assert!(!s.has_finishing());
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
            remove_keywords: true,
            resolution_ppi: 300,
            output_sharpening: Some(OutputSharpening {
                media: SharpenMedia::GlossyPaper,
                amount: SharpenAmount::High,
            }),
            watermark: None,
            videos: VideoExport::Skip,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            json,
            r#"{"format":"png16","color_space":"display_p3","size":{"long_edge":2048},"remove_gps":true,"on_conflict":"skip","remove_keywords":true,"resolution_ppi":300,"output_sharpening":{"media":"glossy_paper","amount":"high"},"watermark":null,"videos":"skip"}"#
        );
        assert_eq!(serde_json::from_str::<ExportSettings>(&json).unwrap(), s);
        let partial: ExportSettings =
            serde_json::from_str(r#"{"format":{"jpeg":{"quality":75}}}"#).unwrap();
        assert_eq!(partial.format, ExportFormat::Jpeg { quality: 75 });
        assert_eq!(partial.on_conflict, ConflictPolicy::Sequence);
        // remove_keywords のない古い形の JSON は、キーワードを書く設定として読む。
        assert!(!partial.remove_keywords);
        // 出力シャープ・透かし・解像度・動画（EXP-03・EXP-05）のない古い形の JSON（書き出しプリセット）は既定値で読む。
        assert_eq!(partial.resolution_ppi, 72);
        assert_eq!(partial.output_sharpening, None);
        assert_eq!(partial.watermark, None);
        assert_eq!(partial.videos, VideoExport::Copy);
    }

    #[test]
    fn watermark_json_and_defaults() {
        let text = Watermark::new(WatermarkContent::Text {
            text: "© 2026 山田".to_owned(),
            font: PathBuf::from("/fonts/a.ttc"),
            font_index: 1,
            color: RgbColor([0x12, 0xab, 0xff]),
        });
        assert_eq!(text.size, Watermark::DEFAULT_TEXT_SIZE);
        assert_eq!(text.anchor, WatermarkAnchor::BottomRight);
        let json = serde_json::to_string(&text).unwrap();
        assert_eq!(
            json,
            r##"{"content":{"text":{"text":"© 2026 山田","font":"/fonts/a.ttc","font_index":1,"color":"#12ABFF"}},"anchor":"bottom_right","size":4,"opacity":60,"margin":2}"##
        );
        assert_eq!(serde_json::from_str::<Watermark>(&json).unwrap(), text);
        // 省略した項目（位置・不透明度・余白・番号・色）は既定値。色は小文字・# なしも読む。
        let image: Watermark =
            serde_json::from_str(r#"{"content":{"image":{"path":"/logo.png"}},"size":20}"#)
                .unwrap();
        assert_eq!(
            image,
            Watermark::new(WatermarkContent::Image {
                path: PathBuf::from("/logo.png")
            })
        );
        let t: Watermark = serde_json::from_str(
            r#"{"content":{"text":{"text":"a","font":"/f.ttf","color":"ff0000"}},"size":3}"#,
        )
        .unwrap();
        assert!(matches!(
            t.content,
            WatermarkContent::Text {
                font_index: 0,
                color: RgbColor([255, 0, 0]),
                ..
            }
        ));
        assert!(
            serde_json::from_str::<Watermark>(
                r##"{"content":{"text":{"text":"a","font":"/f.ttf","color":"#12"}},"size":3}"##
            )
            .is_err()
        );
        assert_eq!(RgbColor::parse(" #00ff7F "), Some(RgbColor([0, 255, 127])));
        assert_eq!(RgbColor::parse("#GG0000"), None);
        assert_eq!(RgbColor::WHITE.to_hex(), "#FFFFFF");
    }

    #[test]
    fn anchors_split_into_alignments() {
        assert_eq!(WatermarkAnchor::TopLeft.horizontal(), Align::Start);
        assert_eq!(WatermarkAnchor::TopLeft.vertical(), Align::Start);
        assert_eq!(WatermarkAnchor::Bottom.horizontal(), Align::Center);
        assert_eq!(WatermarkAnchor::Bottom.vertical(), Align::End);
        assert_eq!(WatermarkAnchor::Right.horizontal(), Align::End);
        assert_eq!(WatermarkAnchor::Right.vertical(), Align::Center);
        let names: Vec<String> = WatermarkAnchor::ALL
            .iter()
            .map(|a| serde_json::to_string(a).unwrap())
            .collect();
        assert_eq!(names.len(), 9);
        assert!(SharpenMedia::MattePaper.is_print() && !SharpenMedia::Screen.is_print());
    }

    #[test]
    fn watermark_validation() {
        let abs = if cfg!(windows) { "C:\\f.ttf" } else { "/f.ttf" };
        let text = |t: &str| {
            Watermark::new(WatermarkContent::Text {
                text: t.to_owned(),
                font: PathBuf::from(abs),
                font_index: 0,
                color: RgbColor::WHITE,
            })
        };
        let settings = |w: Watermark| ExportSettings {
            watermark: Some(w),
            ..Default::default()
        };
        settings(text("© 2026")).validate().unwrap();
        assert!(settings(text("© 2026")).has_finishing());
        settings(text("1 行目\n2 行目")).validate().unwrap();
        for (w, needle) in [
            (text("  "), "文字を入れて"),
            (text(&"あ".repeat(201)), "200 文字まで"),
            (text("1\n2\n3\n4\n5\n6"), "5 行まで"),
            (text("a\tb"), "制御文字"),
            (
                Watermark {
                    size: 0,
                    ..text("a")
                },
                "大きさ",
            ),
            (
                Watermark {
                    opacity: 101,
                    ..text("a")
                },
                "不透明度",
            ),
            (
                Watermark {
                    margin: 26,
                    ..text("a")
                },
                "余白",
            ),
            (
                Watermark::new(WatermarkContent::Text {
                    text: "a".to_owned(),
                    font: PathBuf::new(),
                    font_index: 0,
                    color: RgbColor::WHITE,
                }),
                "フォントのファイルを選んで",
            ),
            (
                Watermark::new(WatermarkContent::Text {
                    text: "a".to_owned(),
                    font: PathBuf::from(abs),
                    font_index: 256,
                    color: RgbColor::WHITE,
                }),
                "フォントの番号",
            ),
            (
                Watermark::new(WatermarkContent::Image {
                    path: PathBuf::from("logo.png"),
                }),
                "絶対パス",
            ),
        ] {
            match settings(w).validate() {
                Err(ExportSettingsError::InvalidWatermark(m)) => {
                    assert!(m.contains(needle), "{m} / {needle}")
                }
                other => panic!("{needle}: {other:?}"),
            }
        }
        // ファイルを選んでいない透かしは、ファイルのほかの値だけを確かめる検証（読み込んだプリセット）では通す。
        let no_font = settings(Watermark::new(WatermarkContent::Text {
            text: "a".to_owned(),
            font: PathBuf::new(),
            font_index: 0,
            color: RgbColor::WHITE,
        }));
        no_font.validate_values().unwrap();
        assert!(no_font.validate().is_err());
        let mut foreign = no_font.clone();
        *foreign.watermark.as_mut().unwrap().content.file_mut() = PathBuf::from("fonts/a.ttf");
        foreign.validate_values().unwrap();
        assert!(foreign.validate().is_err());
        assert_eq!(
            settings(text(&"あ".repeat(201))).validate_values(),
            settings(text(&"あ".repeat(201))).validate()
        );
        for ppi in [0, 1201] {
            let s = ExportSettings {
                resolution_ppi: ppi,
                ..Default::default()
            };
            assert_eq!(
                s.validate(),
                Err(ExportSettingsError::InvalidResolution { value: ppi })
            );
        }
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
