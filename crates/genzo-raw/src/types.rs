//! RAW の展開結果の型（LibRaw に依存しない）。
//!
//! パイプライン（段階 A0 の出力。docs/04_architecture.md の 2.1 節のステージ 1・2.2 節）と
//! ワーカー（共有メモリで本体に渡すバッファ。1.2 節）が共通に使う。

use std::fmt;
use std::str::FromStr;

use genzo_model::PhotoMetadata;
use serde::{Deserialize, Serialize};

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
    /// 対応していない形式。
    #[error("対応していない RAW です: {0}")]
    Unsupported(String),
    /// 展開に失敗した。
    #[error("RAW を展開できません: {0}")]
    Decode(String),
    /// 入出力のエラー。
    #[error("RAW の読み込みに失敗しました: {0}")]
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

/// 展開した RAW（段階 A0 の出力。04 の 2.6 節の B0 の前）。
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
    /// 白レベル（飽和する値）。
    pub white_level: f32,
    /// 撮影時の WB の係数（R, G, B, G2 の順。G を 1 に正規化したもの）。
    pub as_shot_wb: [f32; 4],
    /// XYZ（D65）→ カメラ RGB の行列（04 の 2.6 節）。分からなければ `None`。
    pub cam_xyz: Option<[[f32; 3]; 3]>,
    /// 撮影情報。
    pub metadata: PhotoMetadata,
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

    /// 寸法・長さ・上限・黒レベルと白レベル・WB の係数・カメラ行列を検証する。
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
            as_shot_wb: [2.4, 1.0, 1.6, 1.0],
            cam_xyz: Some([[0.7, -0.2, -0.05], [-0.45, 1.25, 0.2], [-0.07, 0.15, 0.6]]),
            metadata: PhotoMetadata::default(),
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
    fn errors_have_messages() {
        let msg = RawError::TooManyPixels { pixels: 1, max: 0 }.to_string();
        assert!(msg.contains("上限"), "{msg}");
    }
}
