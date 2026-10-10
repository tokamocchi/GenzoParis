//! 2 枚の画像の差（05 の 1.8 節「指標の使い分け」。レビュー R-13）。
//!
//! | 指標 | 用途 | 関数 |
//! |---|---|---|
//! | **8bit 換算の差** | 同じ解像度での数値の比較。CPU 版と GPU 版（IQ-07a）、タイルの大きさの違い、回帰テスト（IQ-08・MAINT-03） | [`diff_8bit`] |
//! | **ΔE2000** | 色の正確さ（PoC-4）と、解像度の違う結果の比較（IQ-07b。比べる前に同じ大きさに縮小しておく） | [`delta_e2000_stats`] |
//!
//! この 2 つは同じものとして扱わない（R-13）。基準画像（[`crate::golden`]）でも、1 つの比較には
//! どちらか一方だけを指定する（[`Tolerance`]）。
//!
//! どちらも、入力に NaN・無限大があれば比較せずにエラーにする（[`DiffError::NonFinite`]）。
//! 02 の IQ-07a は「NaN・無限大は出力しない」ことを求めており、差の計算で黙って無視しない。

use std::fmt;

use genzo_color::lab::{Lab, delta_e2000, xyz_to_lab};
use genzo_color::{D65, Mat3, RgbColorSpace, TransferFunction};
use serde::{Deserialize, Serialize};

use crate::image::{FloatImage, NonFiniteSample};
use crate::stats::Summary;

/// IQ-07a の最終出力の許容差（8bit 換算で ±1 以内。02 の IQ-07a の「目安」）。
///
/// ステージごとの許容差は、各ステージのテストで別に決める（04 の 2.3 節）。
pub const IQ07A_MAX_DIFF_8BIT: f64 = 1.0;
/// IQ-07b の ΔE2000 の 95 パーセンタイルの上限（02 の IQ-07b。**仮置き**の目標値。07 の未決事項 2）。
pub const IQ07B_P95_DELTA_E: f64 = 2.0;
/// IQ-07b の ΔE2000 の最大の上限（02 の IQ-07b。**仮置き**の目標値。07 の未決事項 2）。
pub const IQ07B_MAX_DELTA_E: f64 = 5.0;

/// 8bit 換算の倍率（0〜1 の値の差 1/255 を 1 とする）。
pub const SCALE_8BIT: f64 = 255.0;

/// どちらの画像か。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// 基準（期待値）の画像。
    Expected,
    /// 比べる（実際の）画像。
    Actual,
}

impl fmt::Display for Which {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Which::Expected => "基準の画像",
            Which::Actual => "比べる画像",
        })
    }
}

/// 差の計算のエラー。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DiffError {
    /// 寸法かチャンネル数が違う。
    #[error(
        "画像の形が一致しません（基準 {expected_width} × {expected_height} × {expected_channels}、比べる画像 {actual_width} × {actual_height} × {actual_channels}）"
    )]
    ShapeMismatch {
        /// 基準の幅。
        expected_width: u32,
        /// 基準の高さ。
        expected_height: u32,
        /// 基準のチャンネル数。
        expected_channels: u32,
        /// 比べる画像の幅。
        actual_width: u32,
        /// 比べる画像の高さ。
        actual_height: u32,
        /// 比べる画像のチャンネル数。
        actual_channels: u32,
    },
    /// ΔE の計算に 3 チャンネル（RGB）以外の画像を渡した。
    #[error("ΔE2000 は 3 チャンネル（RGB）の画像でだけ計算できます（{0} チャンネル）")]
    NotRgb(u32),
    /// NaN・無限大を含む。
    #[error("{which}に有限でない値があります: {sample}")]
    NonFinite {
        /// どちらの画像か。
        which: Which,
        /// 最初に見つかった位置と値。
        sample: NonFiniteSample,
    },
    /// 閾値などの引数が不正。
    #[error("引数が不正です: {0}")]
    InvalidArgument(String),
}

/// 2 枚の画像の形と値（有限か）を確かめる。
fn check_pair(expected: &FloatImage, actual: &FloatImage) -> Result<(), DiffError> {
    if !expected.same_shape(actual) {
        return Err(DiffError::ShapeMismatch {
            expected_width: expected.width(),
            expected_height: expected.height(),
            expected_channels: expected.channels(),
            actual_width: actual.width(),
            actual_height: actual.height(),
            actual_channels: actual.channels(),
        });
    }
    if let Some(sample) = expected.find_non_finite() {
        return Err(DiffError::NonFinite {
            which: Which::Expected,
            sample,
        });
    }
    if let Some(sample) = actual.find_non_finite() {
        return Err(DiffError::NonFinite {
            which: Which::Actual,
            sample,
        });
    }
    Ok(())
}

/// 画素の位置（最大の差の位置など）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelLocation {
    /// x。
    pub x: u32,
    /// y。
    pub y: u32,
    /// チャンネル（ΔE のように画素単位の指標では `None`）。
    pub channel: Option<u32>,
}

/// 8bit 換算の差の統計（IQ-07a 用）。
///
/// 差は `|基準 − 実際| × 255` で、**量子化しない**（0〜1 の符号化済みの値の差をそのまま 255 倍する）。
/// 量子化すると、丸めの境目にある値の微小な差が 0 か 1 かに振れて、比較が不安定になるため。
/// 0〜1 の範囲外の値も切り詰めずに比べる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diff8Stats {
    /// 最大差（8bit 換算）。
    pub max: f64,
    /// 平均（全画素・全チャンネルの差の平均。8bit 換算）。
    pub mean: f64,
    /// チャンネルごとの最大差（8bit 換算）。
    pub max_per_channel: Vec<f64>,
    /// 閾値（8bit 換算）。
    pub threshold: f64,
    /// 閾値を超えた（どれかのチャンネルの差 > 閾値の）画素の数。
    pub pixels_over_threshold: u64,
    /// 画素数。
    pub pixel_count: u64,
    /// 最大差の位置（最初に見つかったもの）。
    pub max_at: PixelLocation,
}

/// 8bit 換算の差を計算する（IQ-07a 用）。
///
/// `threshold_8bit` を超えた（`>`。等しい場合は超えていない）画素の数も数える。
/// 例えば IQ-07a の最終出力では `threshold_8bit = 1.0`（[`IQ07A_MAX_DIFF_8BIT`]）とし、
/// 超えた画素が 0 であることを確かめる。
pub fn diff_8bit(
    expected: &FloatImage,
    actual: &FloatImage,
    threshold_8bit: f64,
) -> Result<Diff8Stats, DiffError> {
    if !(threshold_8bit.is_finite() && threshold_8bit >= 0.0) {
        return Err(DiffError::InvalidArgument(format!(
            "閾値は 0 以上の有限の値が必要です（{threshold_8bit}）"
        )));
    }
    check_pair(expected, actual)?;
    let c = expected.channels() as usize;
    let w = expected.width() as usize;
    let mut max = 0.0_f64;
    let mut max_index = 0usize;
    let mut max_per_channel = vec![0.0_f64; c];
    let mut sum = 0.0_f64;
    let mut over = 0u64;
    for (p, (pe, pa)) in expected
        .data()
        .chunks_exact(c)
        .zip(actual.data().chunks_exact(c))
        .enumerate()
    {
        let mut pixel_over = false;
        for ch in 0..c {
            let d = (f64::from(pe[ch]) - f64::from(pa[ch])).abs() * SCALE_8BIT;
            sum += d;
            if d > max {
                max = d;
                max_index = p * c + ch;
            }
            if d > max_per_channel[ch] {
                max_per_channel[ch] = d;
            }
            pixel_over |= d > threshold_8bit;
        }
        over += u64::from(pixel_over);
    }
    let samples = expected.data().len() as f64;
    let pixel = max_index / c;
    Ok(Diff8Stats {
        max,
        mean: sum / samples,
        max_per_channel,
        threshold: threshold_8bit,
        pixels_over_threshold: over,
        pixel_count: expected.pixel_count(),
        max_at: PixelLocation {
            x: (pixel % w) as u32,
            y: (pixel / w) as u32,
            channel: Some((max_index % c) as u32),
        },
    })
}

/// 画素ごとの 8bit 換算の差（チャンネルの最大）。差分画像（[`crate::golden`]）に使う。
pub fn diff_8bit_map(expected: &FloatImage, actual: &FloatImage) -> Result<Vec<f64>, DiffError> {
    check_pair(expected, actual)?;
    let c = expected.channels() as usize;
    Ok(expected
        .data()
        .chunks_exact(c)
        .zip(actual.data().chunks_exact(c))
        .map(|(pe, pa)| {
            pe.iter()
                .zip(pa)
                .map(|(&e, &a)| (f64::from(e) - f64::from(a)).abs() * SCALE_8BIT)
                .fold(0.0, f64::max)
        })
        .collect())
}

/// RGB の色空間（[`LabConversion`] の指定。JSON に保存できる形）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpaceId {
    /// ITU-R BT.2020（D65）。作業色空間（04 の 2.6 節の B2・B3）。
    Bt2020,
    /// sRGB（BT.709・D65）。
    Srgb,
    /// Display P3（D65）。
    DisplayP3,
    /// Adobe RGB (1998)（D65）。
    AdobeRgb1998,
}

impl ColorSpaceId {
    /// genzo-color の色空間。
    pub const fn to_color_space(self) -> RgbColorSpace {
        match self {
            ColorSpaceId::Bt2020 => RgbColorSpace::Bt2020,
            ColorSpaceId::Srgb => RgbColorSpace::Srgb,
            ColorSpaceId::DisplayP3 => RgbColorSpace::DisplayP3,
            ColorSpaceId::AdobeRgb1998 => RgbColorSpace::AdobeRgb1998,
        }
    }

    /// genzo-color の色空間から。
    pub const fn from_color_space(space: RgbColorSpace) -> Self {
        match space {
            RgbColorSpace::Bt2020 => ColorSpaceId::Bt2020,
            RgbColorSpace::Srgb => ColorSpaceId::Srgb,
            RgbColorSpace::DisplayP3 => ColorSpaceId::DisplayP3,
            RgbColorSpace::AdobeRgb1998 => ColorSpaceId::AdobeRgb1998,
        }
    }
}

/// 値の符号化（[`LabConversion`] の指定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    /// リニア（シーンリニア・ディスプレイ参照の B2・B3 など）。
    Linear,
    /// 色空間の伝達関数で符号化済み（書き出しの B4b など。sRGB と Display P3 は IEC 61966-2-1、
    /// Adobe RGB はガンマ 563/256。BT.2020 はリニアのまま）。
    Transfer,
}

/// RGB の値を Lab にする方法（ΔE2000 の計算用）。
///
/// RGB（`encoding` が [`Encoding::Transfer`] なら先に伝達関数を戻す）→ リニア RGB →
/// XYZ（D65、白の Y = 1。[`RgbColorSpace::to_xyz`]）→ CIELAB（基準白 D65）。
/// 色空間の白色点はすべて D65（04 の 2.6 節）なので、色順応はしない。
/// 1 を超える値（シーンリニア）は L\* > 100 になり、負の値（色域外）は Lab の線形の区間の式で
/// 計算する（genzo-color の [`xyz_to_lab`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LabConversion {
    /// 色空間。
    pub space: ColorSpaceId,
    /// 符号化。
    pub encoding: Encoding,
}

impl LabConversion {
    /// リニア RGB。
    pub const fn linear(space: RgbColorSpace) -> Self {
        Self {
            space: ColorSpaceId::from_color_space(space),
            encoding: Encoding::Linear,
        }
    }

    /// 色空間の伝達関数で符号化済みの RGB。
    pub const fn encoded(space: RgbColorSpace) -> Self {
        Self {
            space: ColorSpaceId::from_color_space(space),
            encoding: Encoding::Transfer,
        }
    }

    /// 1 画素を Lab にする準備（行列と白）。
    fn converter(&self) -> LabConverter {
        let space = self.space.to_color_space();
        LabConverter {
            transfer: match self.encoding {
                Encoding::Linear => TransferFunction::Linear,
                Encoding::Transfer => space.transfer_function(),
            },
            to_xyz: space.to_xyz(),
            white: d65_white(),
        }
    }

    /// RGB の 1 画素を Lab にする。
    pub fn to_lab(&self, rgb: [f32; 3]) -> Lab {
        self.converter().convert(rgb)
    }
}

/// D65 の白の XYZ（Y = 1）。
fn d65_white() -> [f64; 3] {
    D65.to_xyz(1.0)
        .expect("D65 の色度座標は定数で、y > 0 なので必ず XYZ にできる")
}

struct LabConverter {
    transfer: TransferFunction,
    to_xyz: Mat3,
    white: [f64; 3],
}

impl LabConverter {
    fn convert(&self, rgb: [f32; 3]) -> Lab {
        let linear = rgb.map(|v| self.transfer.decode(f64::from(v)));
        xyz_to_lab(self.to_xyz.apply(linear), self.white)
    }
}

/// ΔE2000 の統計（IQ-07b・PoC-4 用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeltaEStats {
    /// 平均。
    pub mean: f64,
    /// 95 パーセンタイル（最近接順位法。[`crate::stats`]）。
    pub p95: f64,
    /// 最大。
    pub max: f64,
    /// 画素数。
    pub pixel_count: u64,
    /// 最大の位置（最初に見つかったもの）。
    pub max_at: PixelLocation,
    /// Lab への変換の方法。
    pub conversion: LabConversion,
}

/// 画素ごとの ΔE2000（kL = kC = kH = 1）を計算する。差分画像にも使う。
pub fn delta_e2000_map(
    expected: &FloatImage,
    actual: &FloatImage,
    conversion: LabConversion,
) -> Result<Vec<f64>, DiffError> {
    check_pair(expected, actual)?;
    if expected.channels() != 3 {
        return Err(DiffError::NotRgb(expected.channels()));
    }
    let conv = conversion.converter();
    Ok(expected
        .data()
        .chunks_exact(3)
        .zip(actual.data().chunks_exact(3))
        .map(|(pe, pa)| {
            let le = conv.convert([pe[0], pe[1], pe[2]]);
            let la = conv.convert([pa[0], pa[1], pa[2]]);
            delta_e2000(le, la)
        })
        .collect())
}

/// ΔE2000 の平均・95 パーセンタイル・最大を計算する（IQ-07b・PoC-4 用）。
///
/// 2 枚は同じ寸法であること（解像度の違う結果は、決まった縮小フィルタで同じ大きさにしてから渡す。
/// 02 の IQ-07b）。
pub fn delta_e2000_stats(
    expected: &FloatImage,
    actual: &FloatImage,
    conversion: LabConversion,
) -> Result<DeltaEStats, DiffError> {
    let map = delta_e2000_map(expected, actual, conversion)?;
    delta_e_stats_from_map(&map, expected.width(), conversion)
}

/// 画素ごとの ΔE（[`delta_e2000_map`] の結果）から統計を求める。
pub(crate) fn delta_e_stats_from_map(
    map: &[f64],
    width: u32,
    conversion: LabConversion,
) -> Result<DeltaEStats, DiffError> {
    let summary = Summary::from_values(map).ok_or_else(|| {
        DiffError::InvalidArgument("ΔE の値が空か、有限でない値を含みます".to_owned())
    })?;
    let mut max_index = 0;
    for (i, &v) in map.iter().enumerate() {
        if v > map[max_index] {
            max_index = i;
        }
    }
    let w = width as usize;
    Ok(DeltaEStats {
        mean: summary.mean,
        p95: summary.p95,
        max: summary.max,
        pixel_count: map.len() as u64,
        max_at: PixelLocation {
            x: (max_index % w) as u32,
            y: (max_index / w) as u32,
            channel: None,
        },
        conversion,
    })
}

/// 8bit 換算の差の許容差。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Diff8Tolerance {
    /// 画素ごとの許容差（8bit 換算）。どれかのチャンネルの差がこれを超えた画素を数える。
    pub threshold: f64,
    /// 閾値を超えてよい画素の数（既定 0。0 なら「最大差 ≤ 閾値」と同じ）。
    #[serde(default)]
    pub max_pixels_over: u64,
    /// 平均の上限（任意。8bit 換算）。
    #[serde(default)]
    pub max_mean: Option<f64>,
}

/// ΔE2000 の許容差。上限は少なくとも 1 つ指定する。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeltaETolerance {
    /// Lab への変換の方法。
    pub conversion: LabConversion,
    /// 平均の上限（任意）。
    #[serde(default)]
    pub max_mean: Option<f64>,
    /// 95 パーセンタイルの上限（任意）。
    #[serde(default)]
    pub max_p95: Option<f64>,
    /// 最大の上限（任意）。
    #[serde(default)]
    pub max_max: Option<f64>,
}

/// 比較の許容差。1 つの比較には 1 つの指標だけを使う（05 の 1.8 節「指標の使い分け」）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "metric")]
pub enum Tolerance {
    /// 8bit 換算の差（同じ解像度の数値の比較。IQ-07a・IQ-08）。
    #[serde(rename = "diff_8bit")]
    Diff8Bit(Diff8Tolerance),
    /// ΔE2000（色の正確さ・解像度の違う結果の比較。IQ-07b・PoC-4）。
    #[serde(rename = "delta_e2000")]
    DeltaE2000(DeltaETolerance),
}

impl Tolerance {
    /// IQ-07a の最終出力の許容差（8bit 換算で 1 を超える画素がない。[`IQ07A_MAX_DIFF_8BIT`]）。
    pub fn iq07a() -> Self {
        Self::diff_8bit(IQ07A_MAX_DIFF_8BIT)
    }

    /// 8bit 換算で `threshold` を超える画素がないこと。
    pub fn diff_8bit(threshold: f64) -> Self {
        Tolerance::Diff8Bit(Diff8Tolerance {
            threshold,
            max_pixels_over: 0,
            max_mean: None,
        })
    }

    /// IQ-07b の許容差（ΔE2000 の 95 パーセンタイル ≤ 2、最大 ≤ 5。仮置き）。
    pub fn iq07b(conversion: LabConversion) -> Self {
        Tolerance::DeltaE2000(DeltaETolerance {
            conversion,
            max_mean: None,
            max_p95: Some(IQ07B_P95_DELTA_E),
            max_max: Some(IQ07B_MAX_DELTA_E),
        })
    }

    /// 値が正しいか（上限が 0 以上の有限の値、ΔE は上限が 1 つ以上）。
    pub fn validate(&self) -> Result<(), DiffError> {
        let check = |name: &str, v: f64| {
            if v.is_finite() && v >= 0.0 {
                Ok(())
            } else {
                Err(DiffError::InvalidArgument(format!(
                    "許容差 {name} は 0 以上の有限の値が必要です（{v}）"
                )))
            }
        };
        match self {
            Tolerance::Diff8Bit(t) => {
                check("threshold", t.threshold)?;
                if let Some(m) = t.max_mean {
                    check("max_mean", m)?;
                }
            }
            Tolerance::DeltaE2000(t) => {
                let bounds = [
                    ("max_mean", t.max_mean),
                    ("max_p95", t.max_p95),
                    ("max_max", t.max_max),
                ];
                if bounds.iter().all(|(_, b)| b.is_none()) {
                    return Err(DiffError::InvalidArgument(
                        "ΔE2000 の許容差には上限（max_mean・max_p95・max_max）が 1 つ以上必要です"
                            .to_owned(),
                    ));
                }
                for (name, b) in bounds {
                    if let Some(v) = b {
                        check(name, v)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// 差分画像で「許容差の範囲内」とみなす画素ごとの値（8bit 換算の閾値、または ΔE の上限）。
    pub(crate) fn per_pixel_limit(&self) -> f64 {
        match self {
            Tolerance::Diff8Bit(t) => t.threshold,
            Tolerance::DeltaE2000(t) => t.max_max.or(t.max_p95).or(t.max_mean).unwrap_or(1.0),
        }
    }

    /// 2 枚の画像を比べる。
    pub fn evaluate(
        &self,
        expected: &FloatImage,
        actual: &FloatImage,
    ) -> Result<MetricReport, DiffError> {
        self.validate()?;
        match self {
            Tolerance::Diff8Bit(t) => {
                let stats = diff_8bit(expected, actual, t.threshold)?;
                let mut violations = Vec::new();
                if stats.pixels_over_threshold > t.max_pixels_over {
                    violations.push(format!(
                        "8bit 換算の差が {} を超えた画素が {} 個（許容 {} 個。最大差 {:.4}、位置 ({}, {})）",
                        t.threshold,
                        stats.pixels_over_threshold,
                        t.max_pixels_over,
                        stats.max,
                        stats.max_at.x,
                        stats.max_at.y
                    ));
                }
                if let Some(m) = t.max_mean
                    && stats.mean > m
                {
                    violations.push(format!(
                        "8bit 換算の差の平均 {:.4} が上限 {m} を超えました",
                        stats.mean
                    ));
                }
                Ok(MetricReport {
                    metric: MetricStats::Diff8Bit(stats),
                    violations,
                })
            }
            Tolerance::DeltaE2000(t) => {
                let stats = delta_e2000_stats(expected, actual, t.conversion)?;
                let mut violations = Vec::new();
                for (name, value, bound) in [
                    ("平均", stats.mean, t.max_mean),
                    ("95 パーセンタイル", stats.p95, t.max_p95),
                    ("最大", stats.max, t.max_max),
                ] {
                    if let Some(b) = bound
                        && value > b
                    {
                        violations.push(format!(
                            "ΔE2000 の{name} {value:.4} が上限 {b} を超えました"
                        ));
                    }
                }
                Ok(MetricReport {
                    metric: MetricStats::DeltaE2000(stats),
                    violations,
                })
            }
        }
    }
}

/// 指標の値。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "metric")]
pub enum MetricStats {
    /// 8bit 換算の差。
    #[serde(rename = "diff_8bit")]
    Diff8Bit(Diff8Stats),
    /// ΔE2000。
    #[serde(rename = "delta_e2000")]
    DeltaE2000(DeltaEStats),
}

/// 比較の結果（指標の値と、許容差を超えた項目）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricReport {
    /// 指標の値。
    pub metric: MetricStats,
    /// 許容差を超えた項目の説明（空なら合格）。
    pub violations: Vec<String>,
}

impl MetricReport {
    /// 許容差の範囲内か。
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

impl fmt::Display for MetricReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.metric {
            MetricStats::Diff8Bit(s) => write!(
                f,
                "8bit 換算の差: 最大 {:.4}（({}, {})）、平均 {:.4}、閾値 {} を超えた画素 {} / {}",
                s.max,
                s.max_at.x,
                s.max_at.y,
                s.mean,
                s.threshold,
                s.pixels_over_threshold,
                s.pixel_count
            )?,
            MetricStats::DeltaE2000(s) => write!(
                f,
                "ΔE2000: 平均 {:.4}、95 パーセンタイル {:.4}、最大 {:.4}（({}, {})）、画素数 {}",
                s.mean, s.p95, s.max, s.max_at.x, s.max_at.y, s.pixel_count
            )?,
        }
        for v in &self.violations {
            write!(f, "\n  - {v}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use genzo_color::lab::delta_e2000;

    use super::*;

    fn rgb(width: u32, height: u32, v: [f32; 3]) -> FloatImage {
        FloatImage::filled(width, height, &v).unwrap()
    }

    #[test]
    fn diff_8bit_known_values() {
        // 4 画素。差（×255）は画素 1 の G に 2.0、画素 3 の B に 0.5、それ以外は 0。
        let expected = rgb(2, 2, [0.5, 0.5, 0.5]);
        let mut actual = expected.clone();
        actual.pixel_mut(1, 0).unwrap()[1] = 0.5 + 2.0 / 255.0;
        actual.pixel_mut(1, 1).unwrap()[2] = 0.5 - 0.5 / 255.0;
        let s = diff_8bit(&expected, &actual, 1.0).unwrap();
        assert!((s.max - 2.0).abs() < 1e-4, "{}", s.max);
        // 平均は全 12 サンプルの平均: (2.0 + 0.5) / 12。
        assert!((s.mean - 2.5 / 12.0).abs() < 1e-4, "{}", s.mean);
        assert_eq!(s.pixels_over_threshold, 1);
        assert_eq!(s.pixel_count, 4);
        assert_eq!(
            s.max_at,
            PixelLocation {
                x: 1,
                y: 0,
                channel: Some(1)
            }
        );
        assert!(s.max_per_channel[0] == 0.0);
        assert!((s.max_per_channel[1] - 2.0).abs() < 1e-4);
        assert!((s.max_per_channel[2] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn diff_8bit_threshold_is_strict() {
        // 差がちょうど閾値（0 と 1 の差は 8bit 換算でちょうど 255）の画素は超えていない。
        let expected = FloatImage::filled(1, 1, &[0.0]).unwrap();
        let actual = FloatImage::filled(1, 1, &[1.0]).unwrap();
        let s = diff_8bit(&expected, &actual, 255.0).unwrap();
        assert_eq!(s.max, 255.0);
        assert_eq!(s.pixels_over_threshold, 0);
        let s = diff_8bit(&expected, &actual, 254.999).unwrap();
        assert_eq!(s.pixels_over_threshold, 1);
    }

    #[test]
    fn diff_8bit_identical_and_out_of_range_values() {
        let a =
            FloatImage::from_rgb_fn(3, 3, |x, y| [x as f32 * 0.4 - 0.2, y as f32, 2.0]).unwrap();
        let s = diff_8bit(&a, &a, 0.0).unwrap();
        assert_eq!(s.max, 0.0);
        assert_eq!(s.mean, 0.0);
        assert_eq!(s.pixels_over_threshold, 0);
        // 0〜1 の範囲外も切り詰めない: -0.2 と 1.2 の差は 1.4 × 255。
        let b = a.map(|v| if v < 0.0 { 1.2 } else { v });
        let s = diff_8bit(&a, &b, 1.0).unwrap();
        assert!((s.max - 1.4 * 255.0).abs() < 1e-3, "{}", s.max);
    }

    #[test]
    fn diff_8bit_gray_and_rgba() {
        let a = FloatImage::from_gray_fn(4, 1, |x, _| x as f32 / 255.0).unwrap();
        let b = FloatImage::from_gray_fn(4, 1, |x, _| (x as f32 + 3.0) / 255.0).unwrap();
        let s = diff_8bit(&a, &b, 1.0).unwrap();
        assert!((s.max - 3.0).abs() < 1e-4 && (s.mean - 3.0).abs() < 1e-4);
        assert_eq!(s.pixels_over_threshold, 4);
        let a = FloatImage::filled(2, 1, &[0.0, 0.0, 0.0, 1.0]).unwrap();
        let mut b = a.clone();
        b.pixel_mut(1, 0).unwrap()[3] = 0.0;
        let s = diff_8bit(&a, &b, 1.0).unwrap();
        assert_eq!(s.max, 255.0);
        assert_eq!(s.max_at.channel, Some(3));
        assert_eq!(s.max_at.x, 1);
    }

    #[test]
    fn non_finite_is_an_explicit_error() {
        let a = rgb(3, 2, [0.5, 0.5, 0.5]);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut b = a.clone();
            b.pixel_mut(2, 1).unwrap()[0] = bad;
            match diff_8bit(&a, &b, 1.0) {
                Err(DiffError::NonFinite { which, sample }) => {
                    assert_eq!(which, Which::Actual);
                    assert_eq!((sample.x, sample.y, sample.channel), (2, 1, 0));
                }
                other => panic!("{other:?}"),
            }
            assert!(matches!(
                diff_8bit(&b, &a, 1.0),
                Err(DiffError::NonFinite {
                    which: Which::Expected,
                    ..
                })
            ));
            assert!(matches!(
                delta_e2000_stats(&a, &b, LabConversion::linear(RgbColorSpace::Srgb)),
                Err(DiffError::NonFinite { .. })
            ));
        }
    }

    #[test]
    fn shape_mismatch_and_bad_arguments() {
        let a = rgb(3, 2, [0.5; 3]);
        let b = rgb(2, 3, [0.5; 3]);
        assert!(matches!(
            diff_8bit(&a, &b, 1.0),
            Err(DiffError::ShapeMismatch { .. })
        ));
        let g = FloatImage::filled(3, 2, &[0.5]).unwrap();
        assert!(matches!(
            diff_8bit(&a, &g, 1.0),
            Err(DiffError::ShapeMismatch { .. })
        ));
        assert!(matches!(
            delta_e2000_stats(&g, &g, LabConversion::linear(RgbColorSpace::Srgb)),
            Err(DiffError::NotRgb(1))
        ));
        assert!(matches!(
            diff_8bit(&a, &a, -1.0),
            Err(DiffError::InvalidArgument(_))
        ));
        assert!(matches!(
            diff_8bit(&a, &a, f64::NAN),
            Err(DiffError::InvalidArgument(_))
        ));
    }

    #[test]
    fn lab_conversion_white_gray_and_primaries() {
        // RGB (1, 1, 1) は D65 の白なので L* = 100、a* = b* = 0（どの色空間でも）。
        for space in RgbColorSpace::ALL {
            for conv in [LabConversion::linear(space), LabConversion::encoded(space)] {
                let lab = conv.to_lab([1.0, 1.0, 1.0]);
                assert!((lab.l - 100.0).abs() < 1e-9, "{conv:?} {lab:?}");
                assert!(lab.a.abs() < 1e-9 && lab.b.abs() < 1e-9, "{conv:?} {lab:?}");
            }
        }
        // リニアの 0.18 は Y = 0.18 なので L* = 49.4961（CIE の式。genzo-color の試験値と同じ）。
        let lab = LabConversion::linear(RgbColorSpace::Srgb).to_lab([0.18; 3]);
        assert!((lab.l - 49.4961).abs() < 1e-4, "{lab:?}");
        // sRGB の赤 (1, 0, 0) の Lab（D65）は (53.2408, 80.0925, 67.2032)（Lindbloom 等の公表値、
        // 小数 4 桁。7 桁の行列による値）。genzo-color の行列は原色の色度座標から導出するため、
        // L* が約 0.004 小さくなる（原色から導出した行列で計算すると (53.2371, 80.0901, 67.2033)）。
        // その差を見込んで 0.01 で比べる。
        let lab = LabConversion::encoded(RgbColorSpace::Srgb).to_lab([1.0, 0.0, 0.0]);
        for (v, e) in [lab.l, lab.a, lab.b]
            .into_iter()
            .zip([53.2408, 80.0925, 67.2032])
        {
            assert!((v - e).abs() < 1e-2, "{lab:?}");
        }
        for (v, e) in [lab.l, lab.a, lab.b]
            .into_iter()
            .zip([53.2371, 80.0901, 67.2033])
        {
            assert!((v - e).abs() < 1e-4, "{lab:?}");
        }
    }

    #[test]
    fn encoded_conversion_decodes_transfer_function() {
        // 符号化済み 0.5 の sRGB は、リニア 0.214041… と同じ色。
        let enc = LabConversion::encoded(RgbColorSpace::Srgb).to_lab([0.5; 3]);
        let lin = LabConversion::linear(RgbColorSpace::Srgb).to_lab([0.214_041_14; 3]);
        assert!(delta_e2000(enc, lin) < 1e-5);
        // BT.2020 は伝達関数がリニアなので、encoded と linear は同じ。
        let a = LabConversion::encoded(RgbColorSpace::Bt2020).to_lab([0.3, 0.2, 0.1]);
        let b = LabConversion::linear(RgbColorSpace::Bt2020).to_lab([0.3, 0.2, 0.1]);
        assert_eq!(a, b);
    }

    #[test]
    fn delta_e_stats_known_distribution() {
        // 20 画素のうち 1 画素だけ色を変える。平均 = その ΔE / 20、p95（順位 19）= 0、最大 = その ΔE。
        let conv = LabConversion::linear(RgbColorSpace::Srgb);
        let expected = rgb(5, 4, [0.2, 0.2, 0.2]);
        let mut actual = expected.clone();
        actual
            .pixel_mut(3, 2)
            .unwrap()
            .copy_from_slice(&[0.25, 0.2, 0.2]);
        let one = delta_e2000(conv.to_lab([0.2; 3]), conv.to_lab([0.25, 0.2, 0.2]));
        assert!(one > 1.0);
        let s = delta_e2000_stats(&expected, &actual, conv).unwrap();
        assert!((s.max - one).abs() < 1e-12);
        assert!((s.mean - one / 20.0).abs() < 1e-12);
        assert_eq!(s.p95, 0.0);
        assert_eq!(s.pixel_count, 20);
        assert_eq!((s.max_at.x, s.max_at.y, s.max_at.channel), (3, 2, None));
        // 2 画素を変えると、p95（順位 19）は小さいほうの ΔE になる。
        actual
            .pixel_mut(0, 0)
            .unwrap()
            .copy_from_slice(&[0.2, 0.22, 0.2]);
        let two = delta_e2000(conv.to_lab([0.2; 3]), conv.to_lab([0.2, 0.22, 0.2]));
        let s = delta_e2000_stats(&expected, &actual, conv).unwrap();
        assert!((s.p95 - one.min(two)).abs() < 1e-12);
        assert!((s.max - one.max(two)).abs() < 1e-12);
    }

    #[test]
    fn delta_e_matches_sharma_pair_via_lab_roundtrip() {
        // Sharma ら (2005) の組 17: (50, 2.5, 0) と (73, 25, -18) の ΔE2000 = 27.1492。
        // Lab → XYZ → リニア BT.2020 にした画素の組で、同じ値になることを確かめる
        // （RGB → Lab の変換が正しく逆になっていること）。
        use genzo_color::lab::lab_to_xyz;
        let white = d65_white();
        let to_rgb = |lab: Lab| {
            let xyz = lab_to_xyz(lab, white);
            let m = RgbColorSpace::Bt2020.from_xyz();
            m.apply(xyz).map(|v| v as f32)
        };
        let a = FloatImage::filled(1, 1, &to_rgb(Lab::new(50.0, 2.5, 0.0))).unwrap();
        let b = FloatImage::filled(1, 1, &to_rgb(Lab::new(73.0, 25.0, -18.0))).unwrap();
        let s = delta_e2000_stats(&a, &b, LabConversion::linear(RgbColorSpace::Bt2020)).unwrap();
        // f32 に丸めた分の誤差を見込む。
        assert!((s.max - 27.1492).abs() < 2e-3, "{}", s.max);
    }

    #[test]
    fn delta_e_of_one_8bit_step_is_small_but_not_equivalent() {
        // 8bit 換算の差 1 と ΔE2000 は別の尺度（R-13）。sRGB の中間のグレーで 1 段の差は
        // ΔE2000 で 1 未満だが 0 ではない。
        let a = rgb(1, 1, [128.0 / 255.0; 3]);
        let b = rgb(1, 1, [129.0 / 255.0; 3]);
        let d8 = diff_8bit(&a, &b, 1.0).unwrap();
        let de = delta_e2000_stats(&a, &b, LabConversion::encoded(RgbColorSpace::Srgb)).unwrap();
        assert!((d8.max - 1.0).abs() < 1e-4);
        assert!(de.max > 0.1 && de.max < 1.0, "{}", de.max);
    }

    #[test]
    fn tolerance_evaluate_and_validate() {
        let a = rgb(4, 4, [0.5; 3]);
        let mut b = a.clone();
        b.pixel_mut(0, 0).unwrap()[0] = 0.5 + 1.5 / 255.0;
        let r = Tolerance::iq07a().evaluate(&a, &b).unwrap();
        assert!(!r.passed());
        assert_eq!(r.violations.len(), 1);
        let loose = Tolerance::Diff8Bit(Diff8Tolerance {
            threshold: 1.0,
            max_pixels_over: 1,
            max_mean: Some(0.5),
        });
        assert!(loose.evaluate(&a, &b).unwrap().passed());
        let strict_mean = Tolerance::Diff8Bit(Diff8Tolerance {
            threshold: 2.0,
            max_pixels_over: 0,
            max_mean: Some(0.01),
        });
        let r = strict_mean.evaluate(&a, &b).unwrap();
        assert_eq!(r.violations.len(), 1, "{r}");

        let conv = LabConversion::encoded(RgbColorSpace::Srgb);
        assert!(Tolerance::iq07b(conv).evaluate(&a, &b).unwrap().passed());
        let none = Tolerance::DeltaE2000(DeltaETolerance {
            conversion: conv,
            max_mean: None,
            max_p95: None,
            max_max: None,
        });
        assert!(none.validate().is_err());
        assert!(Tolerance::diff_8bit(-1.0).validate().is_err());
        assert!(Tolerance::diff_8bit(f64::INFINITY).validate().is_err());
    }

    #[test]
    fn tolerance_json_roundtrip() {
        for t in [
            Tolerance::iq07a(),
            Tolerance::iq07b(LabConversion::encoded(RgbColorSpace::DisplayP3)),
        ] {
            let json = serde_json::to_string(&t).unwrap();
            let back: Tolerance = serde_json::from_str(&json).unwrap();
            assert_eq!(back, t);
        }
        let json = serde_json::to_string(&Tolerance::iq07a()).unwrap();
        assert!(json.contains("\"metric\":\"diff_8bit\""), "{json}");
    }
}
