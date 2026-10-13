//! RAW の展開結果の型（LibRaw に依存しない）。
//!
//! パイプライン（段階 A0 の出力。docs/04_architecture.md の 2.1 節のステージ 1・2.2 節）と
//! ワーカー（共有メモリで本体に渡すバッファ。1.2 節）が共通に使う。

use std::fmt;
use std::str::FromStr;

use genzo_model::PhotoMetadata;
use serde::{Deserialize, Serialize};

use crate::lens::LensCorrectionData;

/// 受け取る RAW の画素数の上限（有効画素）。
///
/// 対象機種（α7 IV で約 33M 画素）より十分大きく、壊れたファイルによる巨大な確保を防ぐ値
/// （04 の 1.2 節の「本体は受け取ったバッファの上限を検証する」）。
///
/// genzo-pipeline は、同じ値をパイプラインの画像の型（`RgbImage` など）の画素数の上限と、ステージ 9
/// の出力（C の格子）の上限にも使っている（入力が上限以内でも、回転と切り抜きで出力が大きくなり
/// うるため）。この値を変えると、それらの上限も変わる。
pub const MAX_PIXELS: u64 = 200_000_000;

/// 白レベルの上限（データは u16）。
const MAX_WHITE_LEVEL: f32 = 65_535.0;

/// 飽和の閾値を決める割合（[`saturation_level_for`]。黒レベルから白レベルまでの範囲のうち、この割合を
/// 超えた値を飽和とみなす）。
///
/// **仮置き**（PoC-2 の 2-3・人の判断 No.19 で確定する）: 0.995。根拠は実機の α7 IV（ILCE-7M4、
/// Sony の圧縮 ARW、ISO 400）の 1 枚の実測で、緑が生の値 16372 に張り付き、LibRaw の白レベル 16380
/// より低かった（16372 ちょうどの 1,925 画素に、白レベルでの判定では飽和の印が付かなかった）。
/// 黒 512・白 16380 では閾値は 512 + 15868 × 0.995 = 16300.66 で、16372 は閾値以上になる。
/// α7C は飽和したサンプルがなく未確認。機種・記録方式・ISO ごとの値にするか、プラトーを自動で
/// 検出するかは PoC-2 の撮影で決める。
pub const SATURATION_FRACTION: f32 = 0.995;
/// 撮影時の WB の係数として受け付ける上限（G を 1 としたとき）。
pub(crate) const MAX_WB_COEFF: f32 = 1_000.0;
/// G の係数を 1 とみなす許容差。
const WB_GREEN_TOLERANCE: f32 = 1e-3;
/// カメラ行列の要素の絶対値の上限。
const MAX_MATRIX_ELEMENT: f32 = 100.0;
/// カメラ行列が正則とみなす行列式の絶対値の下限。
const MIN_MATRIX_DETERMINANT: f32 = 1e-9;

/// RAW の扱いのエラー。
#[derive(Debug, thiserror::Error)]
pub enum RawError {
    /// 幅か高さが 0。
    #[error("RAW の寸法が不正です（{width} × {height}）")]
    InvalidDimensions {
        /// 幅。
        width: u32,
        /// 高さ。
        height: u32,
    },
    /// 画素数が上限を超えた。
    #[error("RAW の画素数 {pixels} が上限 {max} を超えています")]
    TooManyPixels {
        /// 画素数。
        pixels: u64,
        /// 上限。
        max: u64,
    },
    /// データの長さが寸法と合わない。
    #[error("RAW のデータの長さ {actual} が寸法から求めた {expected} と一致しません")]
    DataLengthMismatch {
        /// 寸法から求めた長さ。
        expected: u64,
        /// 実際の長さ。
        actual: usize,
    },
    /// 白レベルが不正。
    #[error("白レベルが不正です（{0}。0 より大きく 65535 以下の有限の値が必要）")]
    InvalidWhiteLevel(f32),
    /// 飽和の閾値が不正。
    #[error(
        "飽和の閾値が不正です（{value}。黒レベルの最大 {max_black} より大きく、白レベル {white_level} 以下の有限の値が必要）"
    )]
    InvalidSaturationLevel {
        /// 値。
        value: f32,
        /// 黒レベル（4 つの位置）の最大。
        max_black: f32,
        /// 白レベル。
        white_level: f32,
    },
    /// 既定の切り抜きが不正。
    #[error(
        "既定の切り抜きが不正です（{crop:?}。有効画素 {width} × {height} の中の、幅・高さが 1 以上の範囲が必要）"
    )]
    InvalidDefaultCrop {
        /// 既定の切り抜き。
        crop: PixelRect,
        /// 有効画素の幅。
        width: u32,
        /// 有効画素の高さ。
        height: u32,
    },
    /// 黒レベルが不正。
    #[error(
        "黒レベル[{index}] が不正です（{value}。0 以上で白レベル {white_level} より小さい値が必要）"
    )]
    InvalidBlackLevel {
        /// CFA の位置（R, G1, B, G2 の順の番号）。
        index: usize,
        /// 値。
        value: f32,
        /// 白レベル。
        white_level: f32,
    },
    /// 撮影時の WB の係数が不正。
    #[error(
        "撮影時の WB の係数が不正です（{0:?}。正の有限の値で、G が 1 に正規化されている必要があります）"
    )]
    InvalidWhiteBalance([f32; 4]),
    /// カメラ行列が不正。
    #[error("カメラ行列が不正です: {0}")]
    InvalidColorMatrix(String),
    /// CFA の配列が不正。
    #[error("CFA の配列が不正です: {0}")]
    InvalidCfaPattern(String),
    /// 埋め込みのレンズ補正の情報が不正（[`crate::lens::LensCorrectionData::validate`]）。
    #[error("埋め込みのレンズ補正の情報が不正です: {0}")]
    InvalidLensCorrection(String),
    /// 対応していない形式。
    #[error("対応していない RAW です: {0}")]
    Unsupported(String),
    /// RAW のデータ・メタデータを読み取れない（壊れている・途中で切れている・展開に失敗したなど）。
    ///
    /// 展開（[`crate::decode_file`]）だけでなく、撮影情報の読み取り（[`crate::read_metadata`]）や
    /// 埋め込みサムネイルの取り出しの失敗にも使うため、表示は「展開」に限らない文言にする。
    #[error("RAW のデータを読み取れません: {0}")]
    Decode(String),
    /// 入出力のエラー（ファイルを開けない・読めないなど）。
    #[error("RAW のファイルの入出力に失敗しました: {0}")]
    Io(#[from] std::io::Error),
}

/// CFA の 1 つの位置の色。
///
/// 番号（[`index`](Self::index)）は R = 0、G1 = 1、B = 2、G2 = 3 で、
/// [`RawImage::black_level`] と [`RawImage::as_shot_wb`] の並びと同じ。
/// G1 は R と同じ行の緑、G2 は B と同じ行の緑とする。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CfaColor {
    /// 赤。
    Red,
    /// 緑（R と同じ行）。
    Green1,
    /// 青。
    Blue,
    /// 緑（B と同じ行）。
    Green2,
}

impl CfaColor {
    /// 番号（R = 0、G1 = 1、B = 2、G2 = 3）。
    pub const fn index(self) -> usize {
        match self {
            CfaColor::Red => 0,
            CfaColor::Green1 => 1,
            CfaColor::Blue => 2,
            CfaColor::Green2 => 3,
        }
    }

    /// 緑（G1 または G2）か。
    pub const fn is_green(self) -> bool {
        matches!(self, CfaColor::Green1 | CfaColor::Green2)
    }

    /// RGB のチャンネルの番号（R = 0、G1・G2 = 1、B = 2）。
    pub const fn rgb_channel(self) -> usize {
        match self {
            CfaColor::Red => 0,
            CfaColor::Green1 | CfaColor::Green2 => 1,
            CfaColor::Blue => 2,
        }
    }
}

/// 2 × 2 のベイヤー配列。
///
/// 有効な配列は [`RGGB`](Self::RGGB)・[`BGGR`](Self::BGGR)・[`GRBG`](Self::GRBG)・
/// [`GBRG`](Self::GBRG) の 4 種類（G1 は R の行、G2 は B の行の緑）。名前は左上から
/// 1 行目の 2 画素、2 行目の 2 画素の順。JSON では名前の文字列（`"RGGB"` など）で表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CfaPattern {
    /// `colors[y][x]`（x, y は 0 または 1）。
    colors: [[CfaColor; 2]; 2],
}

impl CfaPattern {
    /// R G / G B。
    pub const RGGB: CfaPattern = CfaPattern {
        colors: [
            [CfaColor::Red, CfaColor::Green1],
            [CfaColor::Green2, CfaColor::Blue],
        ],
    };
    /// B G / G R。
    pub const BGGR: CfaPattern = CfaPattern {
        colors: [
            [CfaColor::Blue, CfaColor::Green2],
            [CfaColor::Green1, CfaColor::Red],
        ],
    };
    /// G R / B G。
    pub const GRBG: CfaPattern = CfaPattern {
        colors: [
            [CfaColor::Green1, CfaColor::Red],
            [CfaColor::Blue, CfaColor::Green2],
        ],
    };
    /// G B / R G。
    pub const GBRG: CfaPattern = CfaPattern {
        colors: [
            [CfaColor::Green2, CfaColor::Blue],
            [CfaColor::Red, CfaColor::Green1],
        ],
    };

    /// 有効な 4 種類の配列。
    pub const ALL: [CfaPattern; 4] = [Self::RGGB, Self::BGGR, Self::GRBG, Self::GBRG];

    /// 各位置の色（`colors[y][x]`）から作る。有効な 4 種類のどれかでなければエラー。
    pub fn from_colors(colors: [[CfaColor; 2]; 2]) -> Result<Self, RawError> {
        Self::ALL
            .into_iter()
            .find(|p| p.colors == colors)
            .ok_or_else(|| RawError::InvalidCfaPattern(format!("{colors:?}")))
    }

    /// 各位置の色（`colors[y][x]`）。
    pub const fn colors(&self) -> [[CfaColor; 2]; 2] {
        self.colors
    }

    /// 画素 (x, y) の色。
    pub const fn color_at(&self, x: usize, y: usize) -> CfaColor {
        self.colors[y & 1][x & 1]
    }

    /// 原点を (dx, dy) だけずらした（画像の (dx, dy) を新しい原点とする）配列。
    ///
    /// 切り抜いた範囲やタイルの左上が奇数の座標のときに使う。
    pub const fn shifted(&self, dx: usize, dy: usize) -> CfaPattern {
        CfaPattern {
            colors: [
                [self.color_at(dx, dy), self.color_at(dx + 1, dy)],
                [self.color_at(dx, dy + 1), self.color_at(dx + 1, dy + 1)],
            ],
        }
    }

    /// 名前（`"RGGB"` など）。
    pub fn name(&self) -> &'static str {
        match *self {
            Self::RGGB => "RGGB",
            Self::BGGR => "BGGR",
            Self::GRBG => "GRBG",
            Self::GBRG => "GBRG",
            // 値は ALL のどれかでしか作れない。
            _ => unreachable!("CfaPattern は有効な 4 種類のどれか"),
        }
    }
}

impl fmt::Display for CfaPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for CfaPattern {
    type Err = RawError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.name().eq_ignore_ascii_case(s))
            .ok_or_else(|| RawError::InvalidCfaPattern(s.to_owned()))
    }
}

impl TryFrom<String> for CfaPattern {
    type Error = RawError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<CfaPattern> for String {
    fn from(p: CfaPattern) -> Self {
        p.name().to_owned()
    }
}

/// 画素の矩形（左上 (x, y) と幅・高さ。画素の単位）。
///
/// [`RawImage::default_crop`] では有効画素の左上を原点とする座標。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PixelRect {
    /// 左端。
    pub x: u32,
    /// 上端。
    pub y: u32,
    /// 幅。
    pub width: u32,
    /// 高さ。
    pub height: u32,
}

impl PixelRect {
    /// 作る。
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// `width × height` の全体（左上が (0, 0)）。
    pub const fn full(width: u32, height: u32) -> Self {
        Self::new(0, 0, width, height)
    }

    /// 右端（含まない。`x + width`）。
    pub fn right(&self) -> u64 {
        u64::from(self.x) + u64::from(self.width)
    }

    /// 下端（含まない。`y + height`）。
    pub fn bottom(&self) -> u64 {
        u64::from(self.y) + u64::from(self.height)
    }

    /// 幅・高さが 1 以上で、`width × height` の画像の中に収まるか。
    pub fn fits_within(&self, width: u32, height: u32) -> bool {
        self.width > 0
            && self.height > 0
            && self.right() <= u64::from(width)
            && self.bottom() <= u64::from(height)
    }
}

/// 黒レベルと白レベルから、飽和の閾値（生の値）を決める（[`SATURATION_FRACTION`]。仮置き）。
///
/// `黒 + (白 − 黒) × SATURATION_FRACTION`。黒は 4 つの位置の黒レベルの最大。結果は黒レベルの最大より
/// 大きく、白レベル以下（[`RawImage::validate`] の条件）になるように、範囲に収める（白と黒の差が
/// 非常に小さい場合は白レベルそのもの）。
pub fn saturation_level_for(black_level: [f32; 4], white_level: f32) -> f32 {
    let black = black_level
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let t = f64::from(black)
        + (f64::from(white_level) - f64::from(black)) * f64::from(SATURATION_FRACTION);
    let t = t as f32;
    if t > black && t <= white_level {
        t
    } else {
        white_level
    }
}

/// 展開した RAW（段階 A0 の出力。04 の 2.6 節の B0 の前）。
///
/// # 範囲（04 の 2.7 節）
///
/// - [`width`](Self::width) × [`height`](Self::height) は LibRaw の **有効画素**（デモザイクはこの全体で
///   行う。端の画素にも実際の隣の画素がある）。
/// - [`default_crop`](Self::default_crop) は、カメラが記録した **既定の切り抜き**（DNG の
///   DefaultCropOrigin / DefaultCropSize、Sony の ARW の同じタグ。カメラの JPEG と同じ範囲）。
///   ステージ 9 の基準の枠（G の「画像全体」）はこの範囲になる。α7 IV の APS-C の ARW では、
///   有効画素の右端 40 列の詰め物と下端の異常な 1 行がこの範囲の外にある。
#[derive(Debug, Clone, PartialEq)]
pub struct RawImage {
    /// 幅（有効画素）。
    pub width: u32,
    /// 高さ（有効画素）。
    pub height: u32,
    /// CFA の配列（有効画素の左上を原点とする）。
    pub cfa: CfaPattern,
    /// 画素の値（行優先、`width * height` 個）。
    pub data: Vec<u16>,
    /// 黒レベル（CFA の位置ごと。R, G1, B, G2 の順）。
    pub black_level: [f32; 4],
    /// 白レベル（正規化で 1 にする値。04 の 2.6 節の B0）。
    pub white_level: f32,
    /// 飽和の閾値（生の値。この値以上の画素に飽和の印を付ける）。黒レベルの最大より大きく、
    /// 白レベル以下。LibRaw で展開した RAW では [`saturation_level_for`]（仮置き）。白レベルと同じなら
    /// 従来の「白レベル以上を飽和とする」と同じ。
    pub saturation_level: f32,
    /// 既定の切り抜き（有効画素の座標）。`None` は有効画素の全体（型の doc の「範囲」）。
    pub default_crop: Option<PixelRect>,
    /// 撮影時の WB の係数（R, G, B, G2 の順。G を 1 に正規化したもの）。
    pub as_shot_wb: [f32; 4],
    /// XYZ（D65）→ カメラ RGB の行列（04 の 2.6 節）。分からなければ `None`。
    pub cam_xyz: Option<[[f32; 3]; 3]>,
    /// 撮影情報。
    pub metadata: PhotoMetadata,
    /// 埋め込みのレンズ補正の情報（DEV-14。Sony の ARW のタグ・DNG の opcode。[`crate::lens`]）。
    /// なければ `None`。
    pub lens_correction: Option<LensCorrectionData>,
}

impl RawImage {
    /// 画素数（`width * height`）。
    pub fn pixel_count(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// 画素 (x, y) の値。範囲外なら `None`。
    pub fn get(&self, x: u32, y: u32) -> Option<u16> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = y as usize * self.width as usize + x as usize;
        self.data.get(i).copied()
    }

    /// 画素 (x, y) の黒レベル。
    pub fn black_level_at(&self, x: u32, y: u32) -> f32 {
        self.black_level[self.cfa.color_at(x as usize, y as usize).index()]
    }

    /// 見える範囲（[`default_crop`](Self::default_crop)、なければ有効画素の全体。有効画素の座標）。
    pub fn visible_area(&self) -> PixelRect {
        self.default_crop
            .unwrap_or(PixelRect::full(self.width, self.height))
    }

    /// 寸法・長さ・上限・黒レベルと白レベル・飽和の閾値・既定の切り抜き・WB の係数・カメラ行列・
    /// 埋め込みのレンズ補正の情報を検証する。
    ///
    /// ワーカーから受け取ったバッファは、使う前に必ずこれで検証する（04 の 1.2 節）。
    pub fn validate(&self) -> Result<(), RawError> {
        if self.width == 0 || self.height == 0 {
            return Err(RawError::InvalidDimensions {
                width: self.width,
                height: self.height,
            });
        }
        let pixels = self.pixel_count();
        if pixels > MAX_PIXELS {
            return Err(RawError::TooManyPixels {
                pixels,
                max: MAX_PIXELS,
            });
        }
        if self.data.len() as u64 != pixels {
            return Err(RawError::DataLengthMismatch {
                expected: pixels,
                actual: self.data.len(),
            });
        }
        let white = self.white_level;
        if !(white.is_finite() && white > 0.0 && white <= MAX_WHITE_LEVEL) {
            return Err(RawError::InvalidWhiteLevel(white));
        }
        for (index, &value) in self.black_level.iter().enumerate() {
            if !(value.is_finite() && value >= 0.0 && value < white) {
                return Err(RawError::InvalidBlackLevel {
                    index,
                    value,
                    white_level: white,
                });
            }
        }
        let max_black = self
            .black_level
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let sat = self.saturation_level;
        if !(sat.is_finite() && sat > max_black && sat <= white) {
            return Err(RawError::InvalidSaturationLevel {
                value: sat,
                max_black,
                white_level: white,
            });
        }
        if let Some(crop) = self.default_crop
            && !crop.fits_within(self.width, self.height)
        {
            return Err(RawError::InvalidDefaultCrop {
                crop,
                width: self.width,
                height: self.height,
            });
        }
        let wb = self.as_shot_wb;
        let wb_ok = wb
            .iter()
            .all(|&c| c.is_finite() && c > 0.0 && c <= MAX_WB_COEFF)
            && (wb[1] - 1.0).abs() <= WB_GREEN_TOLERANCE;
        if !wb_ok {
            return Err(RawError::InvalidWhiteBalance(wb));
        }
        if let Some(m) = &self.cam_xyz {
            validate_matrix(m)?;
        }
        if let Some(lens) = &self.lens_correction {
            lens.validate()?;
        }
        Ok(())
    }
}

/// カメラ行列を検証する（有限・要素の大きさ・正則）。
pub(crate) fn validate_matrix(m: &[[f32; 3]; 3]) -> Result<(), RawError> {
    if !m.iter().flatten().all(|v| v.is_finite()) {
        return Err(RawError::InvalidColorMatrix(
            "有限でない要素があります".to_owned(),
        ));
    }
    if m.iter().flatten().any(|v| v.abs() > MAX_MATRIX_ELEMENT) {
        return Err(RawError::InvalidColorMatrix(format!(
            "要素の絶対値が {MAX_MATRIX_ELEMENT} を超えています"
        )));
    }
    let det = f64::from(m[0][0])
        * (f64::from(m[1][1]) * f64::from(m[2][2]) - f64::from(m[1][2]) * f64::from(m[2][1]))
        - f64::from(m[0][1])
            * (f64::from(m[1][0]) * f64::from(m[2][2]) - f64::from(m[1][2]) * f64::from(m[2][0]))
        + f64::from(m[0][2])
            * (f64::from(m[1][0]) * f64::from(m[2][1]) - f64::from(m[1][1]) * f64::from(m[2][0]));
    if det.abs() < f64::from(MIN_MATRIX_DETERMINANT) {
        return Err(RawError::InvalidColorMatrix(format!(
            "正則ではありません（行列式 {det}）"
        )));
    }
    Ok(())
}

/// カメラが記録した WB の係数（R, G, B, G2）を、G を 1 とする形に正規化する。
///
/// LibRaw の `cam_mul` のように G2 が 0（G と同じ意味）の場合は、G2 を G とみなす。
/// R・G・B のいずれかが正の有限の値でなければ `None`。
pub fn normalize_as_shot_wb(coeffs: [f32; 4]) -> Option<[f32; 4]> {
    let [r, g, b, g2] = coeffs;
    let positive = |v: f32| v.is_finite() && v > 0.0;
    if !(positive(r) && positive(g) && positive(b)) {
        return None;
    }
    let g2 = if positive(g2) { g2 } else { g };
    Some([r / g, 1.0, b / g, g2 / g])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> RawImage {
        RawImage {
            width,
            height,
            cfa: CfaPattern::RGGB,
            data: vec![512; (width * height) as usize],
            black_level: [512.0; 4],
            white_level: 16383.0,
            saturation_level: 16383.0,
            default_crop: None,
            as_shot_wb: [2.4, 1.0, 1.6, 1.0],
            cam_xyz: Some([[0.7, -0.2, -0.05], [-0.45, 1.25, 0.2], [-0.07, 0.15, 0.6]]),
            metadata: PhotoMetadata::default(),
            lens_correction: None,
        }
    }

    #[test]
    fn color_at_follows_the_named_patterns() {
        use CfaColor::*;
        let cases = [
            (CfaPattern::RGGB, [Red, Green1, Green2, Blue]),
            (CfaPattern::BGGR, [Blue, Green2, Green1, Red]),
            (CfaPattern::GRBG, [Green1, Red, Blue, Green2]),
            (CfaPattern::GBRG, [Green2, Blue, Red, Green1]),
        ];
        for (p, [c00, c10, c01, c11]) in cases {
            assert_eq!(p.color_at(0, 0), c00, "{p}");
            assert_eq!(p.color_at(1, 0), c10, "{p}");
            assert_eq!(p.color_at(0, 1), c01, "{p}");
            assert_eq!(p.color_at(1, 1), c11, "{p}");
            // 2 画素ごとに繰り返す。
            assert_eq!(p.color_at(4, 6), c00);
            assert_eq!(p.color_at(7, 9), c11);
        }
    }

    #[test]
    fn green1_is_on_the_red_row() {
        for p in CfaPattern::ALL {
            for y in 0..2 {
                let row = [p.color_at(0, y), p.color_at(1, y)];
                if row.contains(&CfaColor::Red) {
                    assert!(row.contains(&CfaColor::Green1), "{p}");
                } else {
                    assert!(row.contains(&CfaColor::Blue) && row.contains(&CfaColor::Green2));
                }
            }
        }
    }

    #[test]
    fn shifting_moves_between_patterns() {
        assert_eq!(CfaPattern::RGGB.shifted(1, 0), CfaPattern::GRBG);
        assert_eq!(CfaPattern::RGGB.shifted(0, 1), CfaPattern::GBRG);
        assert_eq!(CfaPattern::RGGB.shifted(1, 1), CfaPattern::BGGR);
        assert_eq!(CfaPattern::RGGB.shifted(2, 4), CfaPattern::RGGB);
        for p in CfaPattern::ALL {
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1), (3, 5)] {
                let s = p.shifted(dx, dy);
                for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1), (5, 2)] {
                    assert_eq!(s.color_at(x, y), p.color_at(x + dx, y + dy));
                }
            }
        }
    }

    #[test]
    fn names_parse_and_serialize() {
        for p in CfaPattern::ALL {
            assert_eq!(p.name().parse::<CfaPattern>().unwrap(), p);
            let json = serde_json::to_string(&p).unwrap();
            assert_eq!(json, format!("\"{}\"", p.name()));
            assert_eq!(serde_json::from_str::<CfaPattern>(&json).unwrap(), p);
            assert_eq!(CfaPattern::from_colors(p.colors()).unwrap(), p);
        }
        assert_eq!("rggb".parse::<CfaPattern>().unwrap(), CfaPattern::RGGB);
        assert!("RGBG".parse::<CfaPattern>().is_err());
        assert!(serde_json::from_str::<CfaPattern>("\"XTRANS\"").is_err());
        let all_red = [[CfaColor::Red; 2]; 2];
        assert!(matches!(
            CfaPattern::from_colors(all_red),
            Err(RawError::InvalidCfaPattern(_))
        ));
    }

    #[test]
    fn color_indices_match_level_order() {
        assert_eq!(CfaColor::Red.index(), 0);
        assert_eq!(CfaColor::Green1.index(), 1);
        assert_eq!(CfaColor::Blue.index(), 2);
        assert_eq!(CfaColor::Green2.index(), 3);
        assert_eq!(CfaColor::Green2.rgb_channel(), 1);
        assert!(CfaColor::Green1.is_green() && !CfaColor::Blue.is_green());
    }

    #[test]
    fn valid_image_passes() {
        let img = image(6, 4);
        img.validate().unwrap();
        assert_eq!(img.pixel_count(), 24);
        assert_eq!(img.get(5, 3), Some(512));
        assert_eq!(img.get(6, 0), None);
        assert_eq!(img.black_level_at(1, 1), 512.0);
    }

    #[test]
    fn dimensions_and_length_are_checked() {
        let mut img = image(6, 4);
        img.width = 0;
        assert!(matches!(
            img.validate(),
            Err(RawError::InvalidDimensions { width: 0, .. })
        ));
        let mut img = image(6, 4);
        img.data.pop();
        assert!(matches!(
            img.validate(),
            Err(RawError::DataLengthMismatch {
                expected: 24,
                actual: 23
            })
        ));
    }

    #[test]
    fn pixel_limit_is_checked_before_length() {
        // データを確保せずに、寸法だけで上限を超えることを確かめる。
        let mut img = image(2, 2);
        img.width = 20_000;
        img.height = 10_001;
        assert!(matches!(
            img.validate(),
            Err(RawError::TooManyPixels {
                pixels: 200_020_000,
                max: MAX_PIXELS
            })
        ));
        // u32 の積があふれる寸法でも正しく検出する。
        img.width = u32::MAX;
        img.height = u32::MAX;
        assert!(matches!(
            img.validate(),
            Err(RawError::TooManyPixels { .. })
        ));
    }

    #[test]
    fn levels_are_checked() {
        let mut img = image(2, 2);
        img.white_level = f32::NAN;
        assert!(matches!(
            img.validate(),
            Err(RawError::InvalidWhiteLevel(_))
        ));
        img.white_level = 70_000.0;
        assert!(matches!(
            img.validate(),
            Err(RawError::InvalidWhiteLevel(_))
        ));
        let mut img = image(2, 2);
        img.black_level[2] = 16383.0;
        assert!(matches!(
            img.validate(),
            Err(RawError::InvalidBlackLevel { index: 2, .. })
        ));
        img.black_level[2] = -1.0;
        assert!(img.validate().is_err());
    }

    #[test]
    fn white_balance_is_checked() {
        let mut img = image(2, 2);
        img.as_shot_wb = [2.4, 1.2, 1.6, 1.0];
        assert!(matches!(
            img.validate(),
            Err(RawError::InvalidWhiteBalance(_))
        ));
        img.as_shot_wb = [2.4, 1.0, 0.0, 1.0];
        assert!(img.validate().is_err());
        img.as_shot_wb = [f32::INFINITY, 1.0, 1.6, 1.0];
        assert!(img.validate().is_err());
    }

    #[test]
    fn color_matrix_is_checked() {
        let mut img = image(2, 2);
        img.cam_xyz = Some([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 1.0, 0.0]]);
        assert!(matches!(
            img.validate(),
            Err(RawError::InvalidColorMatrix(_))
        ));
        img.cam_xyz = Some([[f32::NAN, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!(img.validate().is_err());
        img.cam_xyz = Some([[1e6, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!(img.validate().is_err());
        img.cam_xyz = None;
        img.validate().unwrap();
    }

    #[test]
    fn as_shot_wb_normalization() {
        assert_eq!(
            normalize_as_shot_wb([2048.0, 1024.0, 1536.0, 0.0]),
            Some([2.0, 1.0, 1.5, 1.0])
        );
        assert_eq!(
            normalize_as_shot_wb([2.0, 1.0, 1.5, 1.01]),
            Some([2.0, 1.0, 1.5, 1.01])
        );
        assert_eq!(normalize_as_shot_wb([2.0, 0.0, 1.5, 1.0]), None);
        assert_eq!(normalize_as_shot_wb([f32::NAN, 1.0, 1.5, 1.0]), None);
    }

    #[test]
    fn saturation_level_is_checked() {
        let mut img = image(2, 2);
        // 白レベルと同じ値（従来の判定）と、黒と白の間の値は受け付ける。
        img.validate().unwrap();
        img.saturation_level = 16000.0;
        img.validate().unwrap();
        for bad in [f32::NAN, 512.0, 100.0, 16383.5, f32::INFINITY] {
            img.saturation_level = bad;
            assert!(
                matches!(img.validate(), Err(RawError::InvalidSaturationLevel { .. })),
                "{bad}"
            );
        }
        // 黒レベルの最大より大きいこと（位置ごとに黒レベルが違う場合）。
        img.black_level = [510.0, 512.0, 514.0, 516.0];
        img.saturation_level = 515.0;
        assert!(img.validate().is_err());
        img.saturation_level = 517.0;
        img.validate().unwrap();
    }

    /// 仮置きの閾値の式（黒 + (白 − 黒) × 0.995）。実機の α7 IV の値（黒 512・白 16380）で、緑が
    /// 張り付いた 16372 が閾値以上になることを確かめる（plan.md の A-3）。
    #[test]
    fn provisional_saturation_level() {
        let t = saturation_level_for([512.0; 4], 16380.0);
        assert!((t - 16300.66).abs() < 0.01, "{t}");
        assert!(16372.0 >= t);
        // α7C の白レベル 16383。
        let t = saturation_level_for([512.0; 4], 16383.0);
        assert!((t - 16303.645).abs() < 0.01, "{t}");
        // 黒は 4 つの位置の最大を使う。
        let t = saturation_level_for([500.0, 510.0, 520.0, 530.0], 1530.0);
        assert!((t - 1525.0).abs() < 1e-3, "{t}");
        // 範囲が非常に狭い場合も、黒より大きく白以下にする。
        let t = saturation_level_for([1.0; 4], 1.0 + f32::EPSILON);
        assert!(t > 1.0 && t <= 1.0 + f32::EPSILON);
        let mut img = image(2, 2);
        img.saturation_level = saturation_level_for(img.black_level, img.white_level);
        img.validate().unwrap();
    }

    #[test]
    fn default_crop_is_checked() {
        let mut img = image(6, 4);
        assert_eq!(img.visible_area(), PixelRect::full(6, 4));
        img.default_crop = Some(PixelRect::new(1, 1, 4, 2));
        img.validate().unwrap();
        assert_eq!(img.visible_area(), PixelRect::new(1, 1, 4, 2));
        // ちょうど右下の端に接する範囲は有効。
        img.default_crop = Some(PixelRect::new(2, 1, 4, 3));
        img.validate().unwrap();
        for bad in [
            PixelRect::new(3, 0, 4, 2),
            PixelRect::new(0, 2, 6, 3),
            PixelRect::new(0, 0, 0, 4),
            PixelRect::new(0, 0, 6, 0),
            PixelRect::new(u32::MAX, 0, 2, 2),
        ] {
            img.default_crop = Some(bad);
            assert!(
                matches!(img.validate(), Err(RawError::InvalidDefaultCrop { .. })),
                "{bad:?}"
            );
        }
        let json = serde_json::to_string(&PixelRect::new(44, 30, 4608, 3072)).unwrap();
        assert_eq!(json, r#"{"x":44,"y":30,"width":4608,"height":3072}"#);
    }

    #[test]
    fn errors_have_messages() {
        let msg = RawError::TooManyPixels { pixels: 1, max: 0 }.to_string();
        assert!(msg.contains("上限"), "{msg}");
        // Decode は展開以外（撮影情報の読み取りなど）の失敗にも使うので、「展開」に限らない文言。
        let msg = RawError::Decode("x".to_owned()).to_string();
        assert!(!msg.contains("展開"), "{msg}");
    }
}
