//! 現像設定の各項目の型（docs/04_architecture.md の 2.5 節）と値の範囲。
//!
//! どの構造体も `#[serde(default)]` を付け、JSON に欠けている項目は既定値（何もしない値）
//! で補う。値の範囲は [`ranges`] にまとめ、検証（`validate`）と丸め（`normalized`）、
//! UI のスライダーの範囲で共通に使う。

use serde::{Deserialize, Serialize};

/// 値の範囲（両端を含む）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValueRange {
    /// 最小値。
    pub min: f32,
    /// 最大値。
    pub max: f32,
}

impl ValueRange {
    /// 範囲を作る。
    pub const fn new(min: f32, max: f32) -> Self {
        Self { min, max }
    }

    /// `value` が範囲内（NaN は範囲外）か。
    pub fn contains(self, value: f32) -> bool {
        value >= self.min && value <= self.max
    }

    /// `value` を範囲内に丸める。NaN・無限大は `fallback` に置き換える。
    /// 結果が -0.0 のときは 0.0 にする（ハッシュを安定させるため）。
    pub fn clamp_or(self, value: f32, fallback: f32) -> f32 {
        let v = if value.is_nan() {
            fallback
        } else {
            value.clamp(self.min, self.max)
        };
        if v == 0.0 { 0.0 } else { v }
    }
}

/// 各項目の値の範囲。
pub mod ranges {
    use super::ValueRange;

    /// 露光量（EV）。
    pub const EXPOSURE_EV: ValueRange = ValueRange::new(-5.0, 5.0);
    /// -100〜+100 のスライダー（コントラスト、ハイライト・シャドウ・白・黒、彩度など）。
    pub const SLIDER: ValueRange = ValueRange::new(-100.0, 100.0);
    /// 0〜100 のスライダー（ノイズ軽減、シャープのディテール・マスクなど）。
    pub const AMOUNT_0_100: ValueRange = ValueRange::new(0.0, 100.0);
    /// WB の色温度（K）。
    pub const TEMPERATURE_K: ValueRange = ValueRange::new(2000.0, 50000.0);
    /// WB の色かぶり補正（tint）。
    pub const TINT: ValueRange = ValueRange::new(-150.0, 150.0);
    /// トーンカーブの点の座標（0〜1）。
    pub const CURVE_COORD: ValueRange = ValueRange::new(0.0, 1.0);
    /// 切り抜きの座標（G の外接矩形の幅・高さに対する割合。各軸 0〜1。[`CropRect`](super::CropRect)）。
    pub const CROP_COORD: ValueRange = ValueRange::new(0.0, 1.0);
    /// 角度補正（度）。
    pub const ANGLE_DEG: ValueRange = ValueRange::new(-45.0, 45.0);
    /// レンズ補正の適用量（%。100 でプロファイルどおり）。
    pub const LENS_SCALE: ValueRange = ValueRange::new(0.0, 200.0);
    /// シャープの適用量。
    pub const SHARPEN_AMOUNT: ValueRange = ValueRange::new(0.0, 150.0);
    /// シャープの半径。
    pub const SHARPEN_RADIUS: ValueRange = ValueRange::new(0.5, 3.0);
    /// マスクの位置（G 座標。外接矩形の長辺を 1 とする。画像の外側も少し許す）。
    pub const MASK_COORD: ValueRange = ValueRange::new(-1.0, 2.0);
    /// 円形マスクの半径（G 座標。外接矩形の長辺を 1 とする）。
    pub const MASK_RADIUS: ValueRange = ValueRange::new(0.0, 2.0);
    /// 円形マスクの回転（度）。
    pub const MASK_ANGLE_DEG: ValueRange = ValueRange::new(-180.0, 180.0);
}

/// トーンカーブ 1 本あたりの点の数の上限。
pub const MAX_CURVE_POINTS: usize = 64;
/// 切り抜きの幅・高さの最小値（[`CropRect`] と同じ、各軸の割合）。
pub const MIN_CROP_SIZE: f32 = 0.001;
/// 縦横比の各項の上限。
pub const MAX_ASPECT_TERM: u32 = 65_535;
/// ローカル補正（マスク）の数の上限。
pub const MAX_MASKS: usize = 100;

/// ホワイトバランス（DEV-03。04 の 2.1 節のステージ 3）。
///
/// JSON では `"as_shot"`、`{"preset": "daylight"}`、
/// `{"custom": {"temperature_k": 5500.0, "tint": 10.0}}` のように表す。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhiteBalance {
    /// 撮影時の値（カメラが記録した係数）。
    #[default]
    AsShot,
    /// プリセット。
    Preset(WbPreset),
    /// 色温度（K）と色かぶり補正（tint）。カメラ行列を使ってカメラ RGB の係数に変換する。
    Custom {
        /// 色温度（K）。範囲は [`ranges::TEMPERATURE_K`]。
        temperature_k: f32,
        /// 色かぶり補正。正で緑を弱め（マゼンタ寄り）、負で緑を強める。範囲は [`ranges::TINT`]。
        tint: f32,
    },
}

/// ホワイトバランスのプリセット。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WbPreset {
    /// 晴天。
    Daylight,
    /// 曇天。
    Cloudy,
    /// 日陰。
    Shade,
    /// 白熱灯（タングステン）。
    Tungsten,
    /// 蛍光灯。
    Fluorescent,
    /// フラッシュ。
    Flash,
}

impl WbPreset {
    /// すべてのプリセット。
    pub const ALL: [WbPreset; 6] = [
        WbPreset::Daylight,
        WbPreset::Cloudy,
        WbPreset::Shade,
        WbPreset::Tungsten,
        WbPreset::Fluorescent,
        WbPreset::Flash,
    ];

    /// 代表的な色温度（K）と tint を返す。
    ///
    /// 一般的な RAW 現像ソフトのプリセットに近い値を仮に置いている。実際の色は
    /// PoC-4 の評価を経て決める（値を変えると結果が変わるため、変更は process_version の
    /// 更新として扱う）。
    pub const fn temperature_tint(self) -> (f32, f32) {
        match self {
            WbPreset::Daylight => (5500.0, 10.0),
            WbPreset::Cloudy => (6500.0, 10.0),
            WbPreset::Shade => (7500.0, 10.0),
            WbPreset::Tungsten => (2850.0, 0.0),
            WbPreset::Fluorescent => (3800.0, 21.0),
            WbPreset::Flash => (5500.0, 0.0),
        }
    }
}

/// ハイライト・シャドウ・白・黒（DEV-05。04 の 2.1 節のステージ 11）。各 -100〜+100。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToneParams {
    /// ハイライト。
    pub highlights: f32,
    /// シャドウ。
    pub shadows: f32,
    /// 白レベル。
    pub whites: f32,
    /// 黒レベル。
    pub blacks: f32,
}

/// パラメトリックのトーンカーブ（DEV-07）。各 -100〜+100。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ParametricCurve {
    /// ハイライト。
    pub highlights: f32,
    /// ライト。
    pub lights: f32,
    /// ダーク。
    pub darks: f32,
    /// シャドウ。
    pub shadows: f32,
}

/// トーンカーブの点。座標は 0〜1。
///
/// 横軸・縦軸とも、輝度（または各チャンネル）を IEC 61966-2-1 の伝達関数で符号化した値
/// （04 の 2.6 節のトーンカーブの計算方法）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct CurvePoint {
    /// 入力（横軸）。
    pub x: f32,
    /// 出力（縦軸）。
    pub y: f32,
}

impl CurvePoint {
    /// 点を作る。
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// トーンカーブ（DEV-07。04 の 2.1 節のステージ 16）。
///
/// 点のカーブは x の昇順に並べ、同じ x の点を含まない。空のカーブは恒等（何もしない）。
/// 点が 1 つだけのカーブは不正とする（2 点以上が必要）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToneCurve {
    /// パラメトリックのカーブ。
    pub parametric: ParametricCurve,
    /// 輝度のカーブ（色相を保つため、輝度に対して適用し RGB の比率を保つ）。
    pub luma: Vec<CurvePoint>,
    /// R チャンネルのカーブ。
    pub red: Vec<CurvePoint>,
    /// G チャンネルのカーブ。
    pub green: Vec<CurvePoint>,
    /// B チャンネルのカーブ。
    pub blue: Vec<CurvePoint>,
}

impl ToneCurve {
    /// 何もしないカーブか。
    ///
    /// パラメトリックの値がすべて 0 で、点のカーブがすべて恒等（空、または 0 と 1 を
    /// 端点に持ち、すべての点が y = x の上にある）なら真。
    pub fn is_identity(&self) -> bool {
        self.parametric == ParametricCurve::default()
            && [&self.luma, &self.red, &self.green, &self.blue]
                .into_iter()
                .all(|points| Self::points_are_identity(points))
    }

    /// 点のカーブ 1 本が恒等（何もしない）か。
    ///
    /// 空、または 0 と 1 を端点に持ち、すべての点が y = x の上にあるなら真。点は丸めた設定
    /// （[`DevelopSettings::normalized`](crate::DevelopSettings::normalized) の後。x の昇順）を前提とする。
    /// パイプライン（ステージ 16）が、恒等のカーブを飛ばす判定にこの関数を使う。
    pub fn points_are_identity(points: &[CurvePoint]) -> bool {
        match (points.first(), points.last()) {
            (Some(first), Some(last)) => {
                first.x == 0.0 && last.x == 1.0 && points.iter().all(|p| p.x == p.y)
            }
            _ => true,
        }
    }

    /// 点のカーブ 4 本を、名前とともに返す（検証・丸め用）。
    pub(crate) fn point_curves(&self) -> [(&'static str, &Vec<CurvePoint>); 4] {
        [
            ("luma", &self.luma),
            ("red", &self.red),
            ("green", &self.green),
            ("blue", &self.blue),
        ]
    }

    /// 点のカーブ 4 本への可変参照を返す。
    pub(crate) fn point_curves_mut(&mut self) -> [&mut Vec<CurvePoint>; 4] {
        [
            &mut self.luma,
            &mut self.red,
            &mut self.green,
            &mut self.blue,
        ]
    }
}

/// 彩度・自然な彩度（DEV-08。04 の 2.1 節のステージ 13）。各 -100〜+100。
///
/// HSL・カラーグレーディング（v1）は、必要になったときに項目を追加する。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorParams {
    /// 彩度。
    pub saturation: f32,
    /// 自然な彩度。
    pub vibrance: f32,
}

/// 切り抜きの範囲（DEV-15）。
///
/// 値は G（ジオメトリ補正後の全体。04 の 2.7 節）の外接矩形に対する **各軸の割合** で、
/// 横方向は幅を 1、縦方向は高さを 1 とする（各軸 0〜1）。既定は全体。
/// `left < right`、`top < bottom` で、幅・高さは [`MIN_CROP_SIZE`] 以上。
///
/// **2.7 節の G 座標（外接矩形の長辺を 1 とする座標）とは尺度が異なる。** 保存する値を
/// 各軸の割合にしたのは、画像の寸法（と、角度補正で変わる外接矩形の縦横比）を知らなくても
/// 「全体」を [`CropRect::FULL`] で表せ、範囲の検証ができるようにするため。パイプラインで
/// G 座標が必要なときは [`to_g_coords`](Self::to_g_coords) で変換する（C → G の対応。2.7 節）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CropRect {
    /// 左端。
    pub left: f32,
    /// 上端。
    pub top: f32,
    /// 右端。
    pub right: f32,
    /// 下端。
    pub bottom: f32,
}

impl CropRect {
    /// 全体（切り抜きなし）。
    pub const FULL: CropRect = CropRect {
        left: 0.0,
        top: 0.0,
        right: 1.0,
        bottom: 1.0,
    };

    /// 幅（正規化値）。
    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    /// 高さ（正規化値）。
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    /// 全体（切り抜きなし）か。
    pub fn is_full(&self) -> bool {
        *self == Self::FULL
    }

    /// 2.7 節の G 座標（外接矩形の長辺を 1 とする座標）に変換し、
    /// `[left, top, right, bottom]` を返す。
    ///
    /// `g_width`・`g_height` は G の外接矩形の寸法（単位は問わない。比だけを使う）。
    /// どちらかが正の有限の値でなければ `None`。
    pub fn to_g_coords(&self, g_width: f32, g_height: f32) -> Option<[f32; 4]> {
        let ok = |v: f32| v.is_finite() && v > 0.0;
        if !(ok(g_width) && ok(g_height)) {
            return None;
        }
        let long = g_width.max(g_height);
        let (sx, sy) = (g_width / long, g_height / long);
        Some([
            self.left * sx,
            self.top * sy,
            self.right * sx,
            self.bottom * sy,
        ])
    }
}

impl Default for CropRect {
    fn default() -> Self {
        Self::FULL
    }
}

/// 切り抜きの縦横比の指定（UI で切り抜き枠を操作するときの制約）。
///
/// JSON では `"original"`、`{"ratio": {"w": 3, "h": 2}}` のように表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AspectRatio {
    /// 元の画像と同じ縦横比。
    Original,
    /// 指定した縦横比（w : h）。各項は 1〜[`MAX_ASPECT_TERM`]。
    Ratio {
        /// 横。
        w: u32,
        /// 縦。
        h: u32,
    },
}

/// ジオメトリ（切り抜き・角度補正。DEV-15。04 の 2.1 節のステージ 9）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Geometry {
    /// 切り抜きの範囲（G の外接矩形に対する各軸の割合。[`CropRect`]）。
    pub crop: CropRect,
    /// 角度補正（度。-45〜+45）。正の値で、画面上で反時計回り（数学の正の向き）に回転する。
    pub angle_deg: f32,
    /// 縦横比の指定。`None` は自由。
    pub aspect: Option<AspectRatio>,
}

/// レンズ補正（DEV-14。v1）。既定は無効。
///
/// 補正のデータは [`RenderDeps::lens_profile`] で参照する。
/// 周辺減光と倍率色収差はステージ 7（段階 A1）、歪曲補正はステージ 9（ジオメトリ。段階 C で、
/// ガイドの段階 B にも影響する）で適用する（04 の 2.1 節・2.7 節）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensCorrection {
    /// プロファイルによる補正（歪曲・周辺減光）を有効にする。
    pub profile_enabled: bool,
    /// 歪曲補正の適用量（%。0〜200、100 でプロファイルどおり）。
    pub distortion_scale: f32,
    /// 周辺減光補正の適用量（%。0〜200、100 でプロファイルどおり）。
    pub vignetting_scale: f32,
    /// 倍率色収差を除去する。
    pub remove_chromatic_aberration: bool,
}

impl LensCorrection {
    /// 補正を何もしない設定か。
    pub fn is_disabled(&self) -> bool {
        !self.profile_enabled && !self.remove_chromatic_aberration
    }
}

impl Default for LensCorrection {
    fn default() -> Self {
        Self {
            profile_enabled: false,
            distortion_scale: 100.0,
            vignetting_scale: 100.0,
            remove_chromatic_aberration: false,
        }
    }
}

/// シャープ（DEV-12。v1。04 の 2.1 節のステージ 14）。既定は適用量 0（何もしない）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sharpening {
    /// 適用量（0〜150）。
    pub amount: f32,
    /// 半径（0.5〜3.0）。解像度に依存させない定義（04 の 1.1 節の 5.）への換算は実装時に決める。
    pub radius: f32,
    /// ディテール（0〜100）。
    pub detail: f32,
    /// マスク（0〜100）。
    pub masking: f32,
}

impl Default for Sharpening {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius: 1.0,
            detail: 0.0,
            masking: 0.0,
        }
    }
}

/// ノイズ軽減（DEV-13。v1。04 の 2.1 節のステージ 6）。既定は 0（何もしない）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NoiseReduction {
    /// 輝度ノイズの軽減（0〜100）。
    pub luminance: f32,
    /// 色ノイズの軽減（0〜100）。
    pub color: f32,
}

/// ディテール（シャープ・ノイズ軽減。v1）。
///
/// ノイズ軽減は段階 A1、シャープは段階 C に属するため、段階ごとのハッシュでは別々に扱う。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DetailParams {
    /// シャープ。
    pub sharpening: Sharpening,
    /// ノイズ軽減。
    pub noise_reduction: NoiseReduction,
}

impl DetailParams {
    /// 何もしない設定か。
    pub fn is_neutral(&self) -> bool {
        self.sharpening.amount == 0.0
            && self.noise_reduction.luminance == 0.0
            && self.noise_reduction.color == 0.0
    }
}

/// マスクの形（DEV-18。v1。仮の定義で、v1 の実装時に見直す）。
///
/// 位置と半径は 04 の 2.7 節の G 座標（外接矩形の長辺を 1 とする座標）で表す。
/// [`CropRect`]（各軸の割合）とは異なり縦横の尺度が同じなので、`rx == ry` の円形マスクは
/// 画像の縦横比によらず円になる。切り抜きを変えてもマスクの位置は変わらない。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskShape {
    /// 線形グラデーション。(x0, y0) で効果 100%、(x1, y1) で 0%。
    Linear {
        /// 始点の x。
        x0: f32,
        /// 始点の y。
        y0: f32,
        /// 終点の x。
        x1: f32,
        /// 終点の y。
        y1: f32,
    },
    /// 円形グラデーション。
    Radial {
        /// 中心の x。
        cx: f32,
        /// 中心の y。
        cy: f32,
        /// 横方向の半径。
        rx: f32,
        /// 縦方向の半径。
        ry: f32,
        /// 回転（度）。
        angle_deg: f32,
        /// ぼかし（0〜100）。
        feather: f32,
        /// 内側と外側を反転する。
        invert: bool,
    },
}

/// ローカル補正の調整値の差分（ステージ 10〜14 にマスクの値で重み付けして適用する。04 の 2.1 節）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalDelta {
    /// 露光量（EV）。
    pub exposure_ev: f32,
    /// コントラスト。
    pub contrast: f32,
    /// ハイライト。
    pub highlights: f32,
    /// シャドウ。
    pub shadows: f32,
    /// 白レベル。
    pub whites: f32,
    /// 黒レベル。
    pub blacks: f32,
    /// 彩度。
    pub saturation: f32,
}

impl LocalDelta {
    /// すべての差分が 0 か。
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

/// ローカル補正（マスク）1 つ分（DEV-18。v1。仮の定義で、v1 の実装時に見直す）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LocalAdjustment {
    /// 有効か。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// マスクの形。
    pub mask: MaskShape,
    /// 調整値の差分。
    #[serde(default)]
    pub delta: LocalDelta,
}

impl LocalAdjustment {
    /// 結果に影響しない（無効、または差分がすべて 0）か。
    pub fn is_neutral(&self) -> bool {
        !self.enabled || self.delta.is_zero()
    }
}

fn default_true() -> bool {
    true
}

/// 結果に影響する外部データの参照（04 の 2.5 節。レビュー R-08）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DataRef {
    /// データの ID（例: カメラ行列のデータファイルの名前）。
    pub id: String,
    /// データの内容の SHA-256（小文字の 16 進数 64 文字）。未定のときは空文字列。
    pub sha256: String,
}

impl DataRef {
    /// 参照を作る。
    pub fn new(id: impl Into<String>, sha256: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            sha256: sha256.into(),
        }
    }
}

/// 現像結果に影響する外部データ。値が変わったら、同じ設定でも結果が変わりうる（04 の 2.5 節）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderDeps {
    /// カメラ行列・プロファイル。RAW 以外の入力では空のままでよい。
    pub camera_profile: DataRef,
    /// レンズ補正のデータ。
    pub lens_profile: Option<DataRef>,
    /// RAW デコーダとその設定（例: `"libraw-0.21.2"`）。
    pub raw_decoder: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_range_clamps_and_replaces_nan() {
        let r = ValueRange::new(-1.0, 1.0);
        assert!(r.contains(1.0) && r.contains(-1.0) && !r.contains(1.5));
        assert!(!r.contains(f32::NAN));
        assert_eq!(r.clamp_or(2.0, 0.0), 1.0);
        assert_eq!(r.clamp_or(f32::NEG_INFINITY, 0.0), -1.0);
        assert_eq!(r.clamp_or(f32::NAN, 0.25), 0.25);
        let z = r.clamp_or(-0.0, 0.0);
        assert!(z == 0.0 && z.is_sign_positive());
    }

    #[test]
    fn wb_presets_have_temperatures_in_range() {
        for p in WbPreset::ALL {
            let (t, tint) = p.temperature_tint();
            assert!(ranges::TEMPERATURE_K.contains(t), "{p:?}");
            assert!(ranges::TINT.contains(tint), "{p:?}");
        }
        assert!(WbPreset::Tungsten.temperature_tint().0 < WbPreset::Daylight.temperature_tint().0);
        assert!(WbPreset::Shade.temperature_tint().0 > WbPreset::Cloudy.temperature_tint().0);
    }

    #[test]
    fn white_balance_json_forms() {
        assert_eq!(
            serde_json::to_string(&WhiteBalance::AsShot).unwrap(),
            "\"as_shot\""
        );
        assert_eq!(
            serde_json::to_string(&WhiteBalance::Preset(WbPreset::Daylight)).unwrap(),
            r#"{"preset":"daylight"}"#
        );
        assert_eq!(
            serde_json::to_string(&WhiteBalance::Custom {
                temperature_k: 5000.0,
                tint: -3.5
            })
            .unwrap(),
            r#"{"custom":{"temperature_k":5000.0,"tint":-3.5}}"#
        );
        let wb: WhiteBalance = serde_json::from_str(r#"{"preset":"tungsten"}"#).unwrap();
        assert_eq!(wb, WhiteBalance::Preset(WbPreset::Tungsten));
    }

    #[test]
    fn tone_curve_identity() {
        let mut c = ToneCurve::default();
        assert!(c.is_identity());
        c.luma = vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)];
        assert!(c.is_identity());
        c.luma.insert(1, CurvePoint::new(0.5, 0.5));
        assert!(c.is_identity());
        // 端点が 0 と 1 でないと、範囲外の扱いによっては恒等にならないため、恒等とみなさない。
        c.luma = vec![CurvePoint::new(0.2, 0.2), CurvePoint::new(0.8, 0.8)];
        assert!(!c.is_identity());
        c.luma = vec![CurvePoint::new(0.0, 0.1), CurvePoint::new(1.0, 1.0)];
        assert!(!c.is_identity());
        c.luma.clear();
        c.parametric.darks = 5.0;
        assert!(!c.is_identity());
        // 1 本ずつの判定（公開の関数）。
        assert!(ToneCurve::points_are_identity(&[]));
        assert!(ToneCurve::points_are_identity(&[
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(0.25, 0.25),
            CurvePoint::new(1.0, 1.0),
        ]));
        assert!(!ToneCurve::points_are_identity(&[
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(0.5, 0.6),
            CurvePoint::new(1.0, 1.0),
        ]));
        assert!(!ToneCurve::points_are_identity(&[
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(0.9, 0.9),
        ]));
    }

    #[test]
    fn crop_rect_defaults_to_full() {
        let c = CropRect::default();
        assert!(c.is_full());
        assert_eq!(c.width(), 1.0);
        assert_eq!(c.height(), 1.0);
    }

    #[test]
    fn crop_rect_converts_to_long_side_coordinates() {
        // 横長（3:2）の全体は、G 座標では 1 × 2/3。
        let full = CropRect::FULL.to_g_coords(6000.0, 4000.0).unwrap();
        assert_eq!(full, [0.0, 0.0, 1.0, 4000.0 / 6000.0]);
        // 縦長では横が短い。
        let c = CropRect {
            left: 0.5,
            top: 0.25,
            right: 1.0,
            bottom: 0.75,
        };
        assert_eq!(c.to_g_coords(2.0, 4.0).unwrap(), [0.25, 0.25, 0.5, 0.75]);
        assert_eq!(c.to_g_coords(0.0, 4.0), None);
        assert_eq!(c.to_g_coords(f32::NAN, 4.0), None);
    }

    #[test]
    fn aspect_ratio_json_forms() {
        assert_eq!(
            serde_json::to_string(&AspectRatio::Original).unwrap(),
            "\"original\""
        );
        assert_eq!(
            serde_json::to_string(&AspectRatio::Ratio { w: 3, h: 2 }).unwrap(),
            r#"{"ratio":{"w":3,"h":2}}"#
        );
    }

    #[test]
    fn local_adjustment_defaults() {
        let adj: LocalAdjustment =
            serde_json::from_str(r#"{"mask":{"linear":{"x0":0,"y0":0,"x1":0,"y1":1}}}"#).unwrap();
        assert!(adj.enabled);
        assert!(adj.delta.is_zero());
        assert!(adj.is_neutral());
    }
}
