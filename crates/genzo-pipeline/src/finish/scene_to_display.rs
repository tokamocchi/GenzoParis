//! ステージ 15: シーン → ディスプレイ変換（docs/04_architecture.md の 2.1 節、2.6 節「3 つの
//! 圧縮・変換」の 1「トーンの圧縮」）。B2 → B3。
//!
//! シーンリニアの値（1 を超えうる）を、ディスプレイの表示範囲（輝度 0〜1）に滑らかに収める
//! 基本カーブ。HDR 表示（CLR-03）ではこのステージを差し替える。
//!
//! # 基本カーブ（RAW の入力。輝度に適用。人の判断の表 No.1 の決定）
//!
//! 実機の ARW（α7C・α7 IV）のカメラの JPEG に当てはめた Naka–Rushton 型のカーブ `Pc` を基にする:
//!
//! ```text
//! Pc(Y) = P · Y^c / (Y^c + K)                    （K = k^c）
//! Pc(Sm) = 0.18、Pc(Sw) = 1 から P と K を決める     （Sm = 0.18 / 2^1.3 ≒ 0.0731、Sw = 1、c = 1.6）
//!   → K = a·(1 − 0.18) / (0.18 − a / b)、P = (b + K) / b    （a = Sm^c、b = Sw^c。P ≒ 1.0757、k ≒ 0.1993）
//!
//! f(Y) = Pc(Y)                                    （0 ≤ Y ≤ Yj）
//! f(Y) = 1 − (1 − fj) / (1 + sj · (Y − Yj) / (1 − fj))   （Y > Yj。fj = Pc(Yj)、sj = Pc'(Yj)）
//! ```
//!
//! - **中間グレーの対応**: シーンの `Sm`（≒ 0.0731。シーンの 0.18 より 1.3 段暗い値）を表示の 0.18
//!   （sRGB の 8bit で約 118）にする。シーンの 0.18 は表示の約 0.494（L\* 約 75.7）。カメラの
//!   BaselineExposure は使わない（中間グレーの位置はこのカーブで持つ）。
//! - **白**: `Pc` はシーンの 1.0（RAW の飽和）で表示の 1.0 に届く。ただし「1 を超える値（露光量を
//!   上げたとき）は 1 に向かってなめらかに収める（単調・1 階微分まで連続・上限 1）」ことと、
//!   `f(1) = 1` ちょうどは両立しない（`f(1) = 1` で傾きが正なら 1 を少し超える。傾き 0 なら 1 より
//!   上は一定）。そこで `Yj`（[`SCENE_TO_DISPLAY_SHOULDER_START`]、仮置き 0.95）から先を、`Pc` と
//!   値・傾きが一致して 1 に漸近する有理式に置き換えた。シーンの 1.0 は表示の約 0.99710（sRGB の 8bit
//!   で 254.67 → 255）、2.0 は約 0.99974 になる（単体テストで確かめる）。
//! - 単調増加、`f(0) = 0`、`Y → ∞` で 1 に近づき、1 を超えない。`Yj` で 1 階微分まで連続。
//! - **評価**: CPU で f64 の式から作った **対数の軸の 1D LUT**（`L = log2(Y)` の
//!   [`SCENE_TO_DISPLAY_LUT_LOG2_MIN`]〜[`SCENE_TO_DISPLAY_LUT_LOG2_MAX`] を
//!   [`SCENE_TO_DISPLAY_LUT_SIZE`] 点。値は `f(2^L)`）を、CPU 版と GPU 版の両方で同じ線形補間
//!   （[`crate::interp::Lut1d::eval`]）で評価する（GPU の `pow` の精度に左右されないため。IQ-07a）。
//!   範囲より明るい輝度は表の最後の値（f32 で 1）、`2^−16`（[`LOG_FLOOR`]）より暗い輝度（0 以下を
//!   含む）は一定の比率 `r_floor = f(2^−16) / 2^−16` を掛ける（原点を通る直線。`f(0) = 0`）。
//! - パラメータは処理バージョンで決まり、現像設定には依存しない。値は 2 枚の実写だけに当てはめた
//!   もので、**仮置き**（ほかの場面は PoC-4 で確かめる）。
//!
//! # RAW 以外の入力（ディスプレイ参照の画像。人の判断の表 No.2 の決定）
//!
//! JPEG などはすでにディスプレイ向けに作られているので、既定では **恒等** にする（元の見た目を
//! 変えない）: 輝度 1 以下・チャンネルが 1 以下の画素は変えない。露光量などの調整で輝度が 1 を
//! 超えた画素だけ、輝度を 1 にする（`Y' = 1`、`r = 1 / Y`。下の「色の扱い」の後段で白に寄る）。
//! 入力の種類は [`StageContext::input_kind`]（[`InputKind`]）で受け取る。
//!
//! # 色の扱い（画素ごと。f32、この順序）
//!
//! ```text
//! Y  = 0.2627·R + 0.6780·G + 0.0593·B          （BT.2020）
//! RAW:     Y ≥ 2^−16 なら Y' = LUT(log2(Y))、r = Y' / Y。それ以外は r = r_floor、Y' = Y·r
//! RAW 以外: Y > 1 なら Y' = 1、r = 1 / Y。それ以外は Y' = Y、r = 1
//! c  = RGB × r                                  （RGB の比率を保つ → 色相を保つ）
//! m  = max(c_R, c_G, c_B)
//! m > 1 なら: s = (1 − Y') / (m − Y')、 c = Y' + s·(c − Y')   （輝度に向かって彩度を下げ、最大を 1 に）
//! ```
//!
//! - 比率を保つと 1 を超えるチャンネルは、輝度（`Y'`）を保ったまま無彩色の方向へ寄せる
//!   （明るい鮮やかな色が白へ向かう）。
//! - 色域外（負の値）は保持する（切り捨てない。色域の圧縮はステージ 17 の役割）。輝度が 0 以下の
//!   画素も一定の比率 `r_floor`（RAW）・1（RAW 以外）で、符号を保ったまま通る（最大が 1 を超えれば
//!   彩度だけ下げる）。

use std::sync::{Arc, OnceLock};

use genzo_model::{DevelopSettings, Phase};

use super::{luminance, map_pixels};
use crate::contract::ColorContract;
use crate::error::Result;
use crate::image::{ImageTile, Roi};
use crate::interp::Lut1d;
use crate::sanitize::LOG_FLOOR;
use crate::stage::{InputKind, Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// 表示の中間グレー（0.18）にするシーンの値が、シーンの 0.18 より何段暗いか（EV）。
///
/// **仮置き**: 実機の ARW 2 枚（α7C・α7 IV）の位置合わせした平らなブロックで、カメラの JPEG の L\* に
/// 当てはめた値（2 枚まとめての最適値。1 枚ずつでは 1.35・1.25）。シーンの `0.18 / 2^1.3 ≒ 0.0731` が
/// 表示の 0.18 になる。
pub const SCENE_TO_DISPLAY_MIDDLE_GRAY_EV: f64 = 1.3;

/// 表示の中間グレー（0.18。f64 の値。[`super::MIDDLE_GRAY`] は f32 に丸めた値）。
pub const SCENE_TO_DISPLAY_MIDDLE_GRAY: f64 = 0.18;

/// 表示の 1.0 にするシーンの値（RAW の飽和 = 1。**仮置き**: 露光量に連動させない）。
pub const SCENE_TO_DISPLAY_WHITE: f64 = 1.0;

/// Naka–Rushton 型のカーブの指数 `c`（**仮置き**: 実機の 2 枚の当てはめで、0.05 刻みの格子探索の
/// 最適値。中間グレーでの対数-対数の傾きは約 1.33）。
pub const SCENE_TO_DISPLAY_EXPONENT: f64 = 1.6;

/// 肩の始まり `Yj`（シーンの値）。これより明るい部分を、値・傾きをそろえて 1 に漸近する有理式に
/// 置き換える。**仮置き**: シーンの 1.0 が sRGB の 8bit で 255 に丸まる（約 0.99710 → 254.67）範囲で、
/// `Pc` からの変更が小さくなる値として選んだ（0.9 では 1.0 → 254.33 で 255 に届かない）。
pub const SCENE_TO_DISPLAY_SHOULDER_START: f64 = 0.95;

/// 対数の軸の 1D LUT の点数（**仮置き**: 32 段を 8191 等分（約 0.0039 段ごと）。式との相対誤差は
/// このコンテナの計測で最大 3.8e−6（4096 点では 1.5e−5）。単体テストで 5e−6 未満を確かめる）。
pub const SCENE_TO_DISPLAY_LUT_SIZE: usize = 8192;

/// 1D LUT の定義域の下端（`log2(Y)`。[`LOG_FLOOR`] = 2^−16 と同じ）。
pub const SCENE_TO_DISPLAY_LUT_LOG2_MIN: f32 = -16.0;

/// 1D LUT の定義域の上端（`log2(Y)`。2^16 では `1 − f(Y)` が f32 の 1 の刻みより小さく、表の値は 1）。
pub const SCENE_TO_DISPLAY_LUT_LOG2_MAX: f32 = 16.0;

/// ステージ 15 の基本カーブ（f64。モジュールの doc の式）。LUT を作るためと、テスト・分析用。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneToDisplayCurve {
    /// `P`（`Pc` の漸近値）。
    pub p: f64,
    /// `K = k^c`。
    pub k_pow_c: f64,
    /// 指数 `c`。
    pub exponent: f64,
    /// 肩の始まり `Yj`。
    pub shoulder_start: f64,
    /// `fj = Pc(Yj)`。
    pub shoulder_value: f64,
    /// `sj = Pc'(Yj)`。
    pub shoulder_slope: f64,
}

impl SceneToDisplayCurve {
    /// シーンの `middle_in` → 表示の `middle_out`、シーンの `white` → 表示の 1 を通る `Pc`（指数
    /// `exponent`）に、`shoulder_start` からの肩を付けたカーブ。
    ///
    /// 条件: `0 < middle_in < white`、`0 < middle_out < 1`、`exponent > 0`、`0 < shoulder_start < white`、
    /// かつ `Pc` が単調で `Pc(shoulder_start) < 1` になること（満たさなければ `None`）。
    pub fn new(
        middle_in: f64,
        middle_out: f64,
        white: f64,
        exponent: f64,
        shoulder_start: f64,
    ) -> Option<Self> {
        let ok = middle_in > 0.0
            && middle_in < white
            && middle_out > 0.0
            && middle_out < 1.0
            && exponent > 0.0
            && shoulder_start > 0.0
            && shoulder_start < white
            && [middle_in, middle_out, white, exponent, shoulder_start]
                .iter()
                .all(|v| v.is_finite());
        if !ok {
            return None;
        }
        // Pc(Sm) = 0.18、Pc(Sw) = 1:
        //   P = (Sw^c + K) / Sw^c、 0.18 (a + K) = P a  （a = Sm^c、b = Sw^c）
        //   → K = a (1 − 0.18) / (0.18 − a / b)
        let a = middle_in.powf(exponent);
        let b = white.powf(exponent);
        let denom = middle_out - a / b;
        if denom <= 0.0 {
            return None;
        }
        let k_pow_c = a * (1.0 - middle_out) / denom;
        let p = (b + k_pow_c) / b;
        let mut curve = Self {
            p,
            k_pow_c,
            exponent,
            shoulder_start,
            shoulder_value: 0.0,
            shoulder_slope: 0.0,
        };
        curve.shoulder_value = curve.base(shoulder_start);
        curve.shoulder_slope = curve.base_derivative(shoulder_start);
        (k_pow_c > 0.0
            && curve.shoulder_value < 1.0
            && curve.shoulder_slope > 0.0
            && curve.shoulder_value.is_finite())
        .then_some(curve)
    }

    /// 処理バージョン 1 のカーブ（モジュールの doc の定数）。
    pub fn v1() -> Self {
        Self::new(
            SCENE_TO_DISPLAY_MIDDLE_GRAY / SCENE_TO_DISPLAY_MIDDLE_GRAY_EV.exp2(),
            SCENE_TO_DISPLAY_MIDDLE_GRAY,
            SCENE_TO_DISPLAY_WHITE,
            SCENE_TO_DISPLAY_EXPONENT,
            SCENE_TO_DISPLAY_SHOULDER_START,
        )
        .expect("定数は条件を満たす")
    }

    /// `k`（`K^(1/c)`）。
    pub fn k(&self) -> f64 {
        self.k_pow_c.powf(1.0 / self.exponent)
    }

    /// 肩を付ける前の `Pc(Y)`（`Y ≤ 0` は 0）。
    pub fn base(&self, y: f64) -> f64 {
        if y <= 0.0 || y.is_nan() {
            return 0.0;
        }
        let t = y.powf(self.exponent);
        if !t.is_finite() {
            return self.p;
        }
        self.p * t / (t + self.k_pow_c)
    }

    /// `Pc'(Y)`。
    pub fn base_derivative(&self, y: f64) -> f64 {
        if y <= 0.0 || y.is_nan() {
            return 0.0;
        }
        let c = self.exponent;
        let t = y.powf(c);
        let d = t + self.k_pow_c;
        self.p * c * y.powf(c - 1.0) * self.k_pow_c / (d * d)
    }

    /// `f(Y)`（モジュールの doc の式。`Y ≤ 0` は 0、`+∞` は 1）。
    pub fn eval(&self, y: f64) -> f64 {
        if y <= self.shoulder_start || y.is_nan() {
            return self.base(y);
        }
        let room = 1.0 - self.shoulder_value;
        let x = self.shoulder_slope * (y - self.shoulder_start) / room;
        if !x.is_finite() {
            return 1.0;
        }
        1.0 - room / (1.0 + x)
    }

    /// 対数の軸の 1D LUT（モジュールの doc）。
    pub fn lut(&self) -> Lut1d {
        Lut1d::from_fn(
            SCENE_TO_DISPLAY_LUT_SIZE,
            SCENE_TO_DISPLAY_LUT_LOG2_MIN,
            SCENE_TO_DISPLAY_LUT_LOG2_MAX,
            |l| self.eval(l.exp2()),
        )
        .expect("定数の範囲と点数は正しく、値は有限")
    }
}

/// ステージ 15 のパラメータ（GPU 版にも同じ表・値を渡す）。
#[derive(Debug, Clone, PartialEq)]
pub struct SceneToDisplayParams {
    /// 輝度のカーブの表（対数の軸。RAW の入力）。`None` は RAW 以外の入力の恒等（輝度 1 以下は
    /// 変えない）。
    pub lut: Option<Arc<Lut1d>>,
    /// `2^−16` より暗い輝度に掛ける比率 `r_floor`（表の最初の値 / 2^−16。f32 の計算）。`lut` が
    /// `None` なら 1。
    pub floor_ratio: f32,
}

impl SceneToDisplayParams {
    /// RAW の入力（処理バージョン 1 の基本カーブ。表は 1 回だけ作って共有する）。
    pub fn scene_referred() -> Self {
        static PARAMS: OnceLock<SceneToDisplayParams> = OnceLock::new();
        PARAMS
            .get_or_init(|| {
                let lut = SceneToDisplayCurve::v1().lut();
                let floor_ratio = lut.values()[0] / LOG_FLOOR;
                Self {
                    lut: Some(Arc::new(lut)),
                    floor_ratio,
                }
            })
            .clone()
    }

    /// RAW 以外の入力（恒等。輝度 1 を超える画素だけ 1 にする）。
    pub fn display_referred() -> Self {
        Self {
            lut: None,
            floor_ratio: 1.0,
        }
    }

    /// 入力の種類に応じたパラメータ。
    pub fn for_input(kind: InputKind) -> Self {
        match kind {
            InputKind::SceneReferred => Self::scene_referred(),
            InputKind::DisplayReferred => Self::display_referred(),
        }
    }

    /// 輝度 `y` の変換: `(Y', r)`（モジュールの doc の「色の扱い」の 2 行目）。
    #[inline]
    pub fn map_luminance(&self, y: f32) -> (f32, f32) {
        match &self.lut {
            Some(lut) => {
                if y >= LOG_FLOOR {
                    let y2 = lut.eval(y.log2());
                    (y2, y2 / y)
                } else {
                    let r = self.floor_ratio;
                    (y * r, r)
                }
            }
            None => {
                if y > 1.0 {
                    (1.0, 1.0 / y)
                } else {
                    (y, 1.0)
                }
            }
        }
    }

    /// 輝度のカーブ `f`（f32。[`map_luminance`](Self::map_luminance) の `Y'`）。
    #[inline]
    pub fn curve(&self, y: f32) -> f32 {
        self.map_luminance(y).0
    }

    /// 1 画素に適用する（モジュールの doc の「色の扱い」）。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let (y2, r) = self.map_luminance(luminance(c));
        let c = [c[0] * r, c[1] * r, c[2] * r];
        let m = c[0].max(c[1]).max(c[2]);
        if m > 1.0 {
            let s = (1.0 - y2) / (m - y2);
            [
                y2 + s * (c[0] - y2),
                y2 + s * (c[1] - y2),
                y2 + s * (c[2] - y2),
            ]
        } else {
            c
        }
    }
}

/// ステージ 15: シーン → ディスプレイ変換（B2 → B3）。常に適用する（パラメータは処理バージョンと
/// 入力の種類で決まり、現像設定には依存しない）。
#[derive(Debug, Clone, Copy, Default)]
pub struct SceneToDisplayStage;

impl SceneToDisplayStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.scene_to_display";
    /// 04 の 2.1 節のステージの番号。
    pub const NUMBER: u8 = 15;
}

impl Stage for SceneToDisplayStage {
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
        ColorContract::B3DisplayReferred
    }
    fn params(&self, _: &DevelopSettings, ctx: &StageContext<'_>) -> Result<Option<StageParams>> {
        match ctx.process_version {
            ProcessVersion::V1 => Ok(Some(StageParams::new(SceneToDisplayParams::for_input(
                ctx.input_kind,
            )))),
        }
    }
    fn input_roi(&self, output_roi: Roi, _: &StageParams, _: &StageContext<'_>) -> Roi {
        output_roi
    }
    fn run_cpu(
        &self,
        ctx: &StageContext<'_>,
        input: &ImageTile,
        output: &mut ImageTile,
        params: &StageParams,
    ) -> Result<()> {
        let p = params.expect::<SceneToDisplayParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 => map_pixels(input, output, |_, _, c| p.apply(c)),
        }
    }
}

#[cfg(test)]
mod tests {
    use genzo_color::transfer::srgb_encode;

    use super::*;

    fn raw() -> SceneToDisplayParams {
        SceneToDisplayParams::scene_referred()
    }

    #[test]
    fn v1_curve_matches_the_decided_parameters() {
        let c = SceneToDisplayCurve::v1();
        // 実機の当てはめ（Pc）: P ≒ 1.0757、k ≒ 0.1993。
        assert!((c.p - 1.075_716_7).abs() < 1e-6, "{}", c.p);
        assert!((c.k() - 0.199_294_3).abs() < 1e-6, "{}", c.k());
        let sm = 0.18 / 1.3f64.exp2();
        assert!((c.eval(sm) - 0.18).abs() < 1e-12);
        assert!((c.base(1.0) - 1.0).abs() < 1e-12);
        // 分析の値: 0.01 → 0.0089、0.18 → 0.494、0.5 → 0.875。
        assert!((c.eval(0.01) - 0.008_890).abs() < 1e-6);
        assert!((c.eval(0.18) - 0.494_141).abs() < 1e-6);
        assert!((c.eval(0.5) - 0.874_901).abs() < 1e-6);
        // 肩: シーンの 1.0 は約 0.99710（sRGB の 8bit で 255 に丸まる）、2.0 は約 0.99974。
        let w = c.eval(1.0);
        assert!((w - 0.997_098).abs() < 1e-6, "{w}");
        assert!((srgb_encode(w) * 255.0).round() == 255.0);
        assert!((c.eval(2.0) - 0.999_743).abs() < 1e-6);
        assert_eq!(c.eval(f64::INFINITY), 1.0);
        assert_eq!(c.eval(0.0), 0.0);
    }

    #[test]
    fn curve_is_monotonic_c1_and_bounded() {
        let c = SceneToDisplayCurve::v1();
        let mut prev = -1.0f64;
        for k in 0..=40_000 {
            let y = (k as f64 / 40_000.0 * 48.0 - 24.0).exp2();
            let v = c.eval(y);
            assert!(v > prev && v < 1.0 + 1e-15, "{y} → {v}");
            prev = v;
        }
        // 肩の始まりで値・傾きが連続。
        let yj = SCENE_TO_DISPLAY_SHOULDER_START;
        let h = 1e-7;
        assert!((c.eval(yj + h) - c.eval(yj - h)).abs() < 3.0 * h);
        let left = (c.eval(yj) - c.eval(yj - h)) / h;
        let right = (c.eval(yj + h) - c.eval(yj)) / h;
        assert!((left - right).abs() < 1e-5, "{left} {right}");
        assert!((left - c.shoulder_slope).abs() < 1e-5);
        // 不正なパラメータ。
        assert!(SceneToDisplayCurve::new(0.0, 0.18, 1.0, 1.6, 0.95).is_none());
        assert!(SceneToDisplayCurve::new(0.07, 0.18, 1.0, 1.6, 1.0).is_none());
        assert!(SceneToDisplayCurve::new(0.5, 0.18, 1.0, 1.6, 0.9).is_none());
        assert!(SceneToDisplayCurve::new(0.07, 1.2, 1.0, 1.6, 0.9).is_none());
    }

    #[test]
    fn lut_matches_the_formula() {
        let c = SceneToDisplayCurve::v1();
        let p = raw();
        let mut worst = 0.0f64;
        for k in 0..=200_000 {
            let y = (k as f64 / 200_000.0 * 31.9 - 15.95).exp2();
            let v = f64::from(p.curve(y as f32));
            let e = c.eval(f64::from(y as f32));
            worst = worst.max((v - e).abs() / e.max(1e-6));
        }
        eprintln!("ステージ 15 の LUT の相対誤差の最大: {worst:.3e}");
        assert!(worst < 5e-6, "LUT の相対誤差 {worst}");
        // 表の端: 2^16 より明るい輝度は 1、2^−16 より暗い輝度は一定の比率。
        assert_eq!(p.curve(1e30), 1.0);
        assert_eq!(p.curve(f32::INFINITY), 1.0);
        let r = p.floor_ratio;
        assert!(r > 0.0 && r < 0.05, "{r}");
        assert_eq!(p.map_luminance(LOG_FLOOR * 0.5), (LOG_FLOOR * 0.5 * r, r));
        // 2^−16 でつながる。
        let (y2, r2) = p.map_luminance(LOG_FLOOR);
        assert!((r2 - r).abs() <= r * 1e-6, "{r2} {r}");
        assert!((y2 - LOG_FLOOR * r).abs() <= 1e-12);
        assert_eq!(p.curve(0.0), 0.0);
    }

    #[test]
    fn colors_keep_their_ratio_and_stay_within_one() {
        let p = raw();
        // 中間グレーの位置: シーンの 0.0731 → 表示の 0.18。
        let g = p.apply([0.073_102_72; 3]);
        assert!(g.iter().all(|v| (v - 0.18).abs() < 1e-5), "{g:?}");
        // 明るい無彩色は輝度のカーブどおり。
        let w = p.apply([4.0; 3]);
        assert!(w.iter().all(|v| (v - p.curve(4.0)).abs() < 1e-6));
        // 色は比率を保つ（1 を超えない範囲）。
        let c = [0.3, 0.2, 0.1];
        let out = p.apply(c);
        assert!((out[0] / out[1] - 1.5).abs() < 1e-5 && (out[1] / out[2] - 2.0).abs() < 1e-5);
        // 比率を保つと 1 を超える色は、輝度を保って彩度を下げる。
        let c = [6.0, 0.5, 0.2];
        let out = p.apply(c);
        let m = out[0].max(out[1]).max(out[2]);
        assert!((m - 1.0).abs() < 1e-6, "{out:?}");
        assert!((luminance(out) - p.curve(luminance(c))).abs() < 1e-5);
        assert!(
            out[0] > out[1] && out[1] > out[2],
            "色相の順は保つ: {out:?}"
        );
    }

    #[test]
    fn negative_values_are_kept() {
        let p = raw();
        // 負の値は比率どおりに残る（符号を保つ）。
        let c = [-0.05, 0.3, 0.1];
        let out = p.apply(c);
        assert!(out[0] < 0.0, "{out:?}");
        assert!((out[0] / out[1] - c[0] / c[1]).abs() < 1e-5);
        // 輝度が 0 以下の画素も一定の比率で通る（0 にしない）。
        let c = [0.01, -0.02, 0.01];
        assert!(luminance(c) < 0.0);
        let out = p.apply(c);
        assert_eq!(out, c.map(|v| v * p.floor_ratio));
        // 1 を超えるチャンネルがあると輝度に向かって彩度を下げる（負の値も輝度の方へ寄る）。
        for c in [[-0.5, 3.0, 0.2], [1.2, -2.0, 0.0]] {
            let out = p.apply(c);
            assert!(out.iter().all(|v| v.is_finite()), "{out:?}");
            assert!(out.iter().all(|&v| v <= 1.0 + 1e-6), "{out:?}");
            assert!(
                (luminance(out) - p.curve(luminance(c))).abs() < 1e-5,
                "{out:?}"
            );
        }
    }

    #[test]
    fn display_referred_input_is_identity_up_to_one() {
        let p = SceneToDisplayParams::display_referred();
        for c in [
            [0.0, 0.0, 0.0],
            [0.18, 0.18, 0.18],
            [1.0, 1.0, 1.0],
            [0.9, 0.2, 0.05],
            [-0.01, 0.4, 0.3],
            [1.0, 0.0, 0.0],
        ] {
            assert_eq!(p.apply(c), c, "{c:?}");
        }
        // 輝度が 1 を超える画素（露光量を上げたとき）は白に収める。
        let out = p.apply([2.0, 2.0, 2.0]);
        assert!(out.iter().all(|v| (v - 1.0).abs() < 1e-6), "{out:?}");
        let out = p.apply([3.0, 1.0, 0.5]);
        assert!((luminance(out) - 1.0).abs() < 1e-5, "{out:?}");
        assert!(out.iter().all(|&v| v <= 1.0 + 1e-6));
        // 輝度 1 以下で 1 を超えるチャンネルは、輝度を保って彩度を下げる。
        let c = [1.5, 0.6, 0.2];
        let out = p.apply(c);
        assert!((luminance(out) - luminance(c)).abs() < 1e-6, "{out:?}");
        assert!((out[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn params_follow_the_input_kind() {
        use genzo_model::Orientation;

        use crate::coords::CoordinateFrames;
        let f = CoordinateFrames::new(8, 8, Orientation::Normal).unwrap();
        let ctx = StageContext::new(ProcessVersion::V1, f);
        let s = DevelopSettings::default();
        let p = SceneToDisplayStage.params(&s, &ctx).unwrap().unwrap();
        assert!(p.get::<SceneToDisplayParams>().unwrap().lut.is_some());
        let ctx = ctx.with_input_kind(InputKind::DisplayReferred);
        let p = SceneToDisplayStage.params(&s, &ctx).unwrap().unwrap();
        assert_eq!(
            p.get::<SceneToDisplayParams>().unwrap(),
            &SceneToDisplayParams::display_referred()
        );
    }
}
