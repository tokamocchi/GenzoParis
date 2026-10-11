//! ステージ 16: トーンカーブ（DEV-07。docs/04_architecture.md の 2.1 節、2.6 節「トーンカーブ
//! （ステージ 16）の計算方法」）。B3 → B3。効果（周辺光量・粒子。v1）は未実装。
//!
//! # 計算（2.6 節のとおり。画素ごと。f32、この順序）
//!
//! ```text
//! Y   = 0.2627·R + 0.6780·G + 0.0593·B           （BT.2020 の係数。B3 のリニアな値）
//! e   = srgb_encode(Y)                            （IEC 61966-2-1。genzo-color の f32 の式。符号を保つ）
//! e'  = curve(e)                                  （輝度のカーブ = 点のカーブ ∘ パラメトリック。1D LUT）
//! Y'  = srgb_decode(e')
//! Y0' = srgb_decode(curve(0))                     （黒の持ち上げ。定数。カーブが黒を持ち上げなければ 0）
//! r   = |Y' − Y0'| / Y（Y ≥ 2^−16 のとき）、 r_min = |Y'(2^−16) − Y0'| / 2^−16（それより暗いとき。定数）
//! RGB' = Y' + r·(RGB − Y)                         （Y0' = 0 なら = (Y' / Y)·RGB。RGB の比率（色相）を保つ）
//! チャンネル別: c'' = srgb_decode(curve_c(srgb_encode(c')))   （R・G・B それぞれ。輝度のカーブの後）
//! ```
//!
//! - **カーブの評価**: 定義域 0〜1 の 1D LUT（[`TONE_CURVE_LUT_SIZE`] 点）と線形補間
//!   （[`crate::interp::Lut1d::eval`]）。GPU 版にも同じ表を渡す。0〜1 の外は傾き 1 で延長する
//!   （`curve(e) = curve(clamp(e, 0, 1)) + (e − clamp(e, 0, 1))`）。恒等のカーブなら外側も恒等。
//! - **点のカーブ**: 点を通る単調な 3 次スプライン（Fritsch–Carlson。[`MonotoneCubic`]）。点が単調なら
//!   カーブも単調で、点の y の範囲を出ない。最初の点より左・最後の点より右は端の点の y で一定。
//! - **パラメトリック**: シャドウ・ダーク・ライト・ハイライトの 4 つの領域（0〜0.25〜0.5〜0.75〜1 に
//!   分けた区間の中央 0.125・0.375・0.625・0.875）の点を、スライダー ±100 で
//!   ±[`PARAMETRIC_AMPLITUDE`] だけ上下させ、(0, 0)・(1, 1) と合わせた 6 点を同じスプラインで結ぶ。
//!   振幅 0.1 では 6 点が常に単調増加なので、カーブも単調増加（端点は動かない）。仮置き（DEV-07）。
//! - **黒の持ち上げ（2.6 節の式からの変更。レビューで修正）**: 2.6 節の「RGB に Y' / Y を掛ける」を
//!   そのまま使うと、カーブが黒を持ち上げる（`curve(0) > 0`）とき `Y' / Y` が `Y → 0` で発散する。
//!   暗部の雑音（カメラ行列の後は、輝度が 0 に近くてもチャンネルの値は正負に散らばる）の色の成分が
//!   数百倍になり、持ち上げた黒の上に鮮やかな点として現れる（例: 黒を符号化した値で 0.1 に持ち上げると、
//!   B3 の (1e−3, −4e−4, 1e−3) が (0.20, −0.08, 0.20) のマゼンタになっていた）。そこで、持ち上げの分
//!   `Y0'` は無彩色として足し、色の成分にはカーブの残りの部分の倍率 `|Y' − Y0'| / Y` だけを掛ける
//!   （フレアのように、持ち上げた黒は灰色になる）。`Y0' = 0`（黒を持ち上げないカーブ。既定の
//!   パラメトリック・S 字など）では 2.6 節の式と同じ。輝度は常に `Y'`、色相（無彩色からの向き）は
//!   保つ。絶対値は、単調でないカーブ（`Y' < Y0'` になる部分）でも色相を反転させないため。
//!   設計書 2.6 節への反映は要確認（最終報告に記載）。
//! - **暗部の比率**: `|Y| < 2^−16` 付近では `r` の分母が 0 に近いので、`2^−16` より暗い（負を含む）画素は
//!   定数 `r_min` で色の成分を倍にし、輝度は `Y'` そのものにする（`Y'` は延長したカーブで計算するので、
//!   恒等のカーブなら負の輝度もそのまま）。`r` は `Y = 2^−16` で `r_min` と連続につながる。
//! - 出力は B3 のまま（リニア）。「約 2.2」のような近似の符号化は使わない（2.6 節）。

use genzo_color::transfer::{srgb_decode_f32, srgb_encode_f32};
use genzo_model::{CurvePoint, DevelopSettings, ParametricCurve, Phase, ToneCurve};

use super::{luminance, map_pixels};
use crate::contract::ColorContract;
use crate::error::{PipelineError, Result};
use crate::image::{ImageTile, Roi};
use crate::interp::{DEFAULT_LUT1D_SIZE, Lut1d};
use crate::sanitize::LOG_FLOOR;
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// トーンカーブの 1D LUT の点数（2.3 節の例の 4096 点。[`DEFAULT_LUT1D_SIZE`]）。
pub const TONE_CURVE_LUT_SIZE: usize = DEFAULT_LUT1D_SIZE;

/// パラメトリックのカーブのスライダー ±100 での、領域の中央の点の上下の量（符号化した値）。
/// 仮置き: 8bit で約 25 段階。0.125 未満なら 6 点が常に単調増加になる（モジュールの doc）。
pub const PARAMETRIC_AMPLITUDE: f64 = 0.1;

/// パラメトリックのカーブの 4 つの領域の中央（シャドウ・ダーク・ライト・ハイライトの順）。
pub const PARAMETRIC_CENTERS: [f64; 4] = [0.125, 0.375, 0.625, 0.875];

/// 単調な 3 次スプライン（Fritsch–Carlson。f64）。
///
/// 1. 区間の傾き `δk = (y[k+1] − y[k]) / (x[k+1] − x[k])`。
/// 2. 接線: 両端は隣の区間の傾き。内側は `δ(k−1)·δk > 0` なら平均、それ以外は 0。
/// 3. 区間ごとに `δk = 0` なら両端の接線を 0。そうでなければ `α = m[k]/δk`、`β = m[k+1]/δk` について
///    `α² + β² > 9` なら `τ = 3 / sqrt(α² + β²)` を両方に掛ける。
/// 4. 区間の中は 3 次エルミート補間。
#[derive(Debug, Clone, PartialEq)]
pub struct MonotoneCubic {
    xs: Vec<f64>,
    ys: Vec<f64>,
    ms: Vec<f64>,
}

impl MonotoneCubic {
    /// 点から作る。2 点以上で、x が狭義の昇順、すべて有限であること。
    pub fn new(points: &[(f64, f64)]) -> Result<Self> {
        if points.len() < 2 {
            return Err(PipelineError::InvalidArgument(format!(
                "スプラインには 2 点以上が必要です（{} 点）",
                points.len()
            )));
        }
        if points
            .iter()
            .any(|(x, y)| !(x.is_finite() && y.is_finite()))
            || points.windows(2).any(|w| w[1].0 <= w[0].0)
        {
            return Err(PipelineError::InvalidArgument(
                "スプラインの点は有限で、x が狭義の昇順である必要があります".to_owned(),
            ));
        }
        let xs: Vec<f64> = points.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = points.iter().map(|p| p.1).collect();
        let n = xs.len();
        let delta: Vec<f64> = (0..n - 1)
            .map(|k| (ys[k + 1] - ys[k]) / (xs[k + 1] - xs[k]))
            .collect();
        let mut ms = vec![0.0; n];
        ms[0] = delta[0];
        ms[n - 1] = delta[n - 2];
        for k in 1..n - 1 {
            ms[k] = if delta[k - 1] * delta[k] > 0.0 {
                (delta[k - 1] + delta[k]) * 0.5
            } else {
                0.0
            };
        }
        for k in 0..n - 1 {
            if delta[k] == 0.0 {
                ms[k] = 0.0;
                ms[k + 1] = 0.0;
                continue;
            }
            let a = ms[k] / delta[k];
            let b = ms[k + 1] / delta[k];
            // 端の接線の符号が区間の傾きと逆なら 0 にする（単調性のため）。
            if a < 0.0 {
                ms[k] = 0.0;
            }
            if b < 0.0 {
                ms[k + 1] = 0.0;
            }
            let (a, b) = (a.max(0.0), b.max(0.0));
            let s = a * a + b * b;
            if s > 9.0 {
                let tau = 3.0 / s.sqrt();
                ms[k] = tau * a * delta[k];
                ms[k + 1] = tau * b * delta[k];
            }
        }
        Ok(Self { xs, ys, ms })
    }

    /// `x` での値。範囲の外は端の点の y。
    pub fn eval(&self, x: f64) -> f64 {
        let n = self.xs.len();
        if x.is_nan() || x <= self.xs[0] {
            return self.ys[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        // x[k] ≤ x < x[k+1] の k（点は高々 64 個なので線形探索で十分）。
        let k = self.xs.windows(2).position(|w| x < w[1]).unwrap_or(n - 2);
        let h = self.xs[k + 1] - self.xs[k];
        let t = (x - self.xs[k]) / h;
        let t2 = t * t;
        let t3 = t2 * t;
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        h00 * self.ys[k] + h10 * h * self.ms[k] + h01 * self.ys[k + 1] + h11 * h * self.ms[k + 1]
    }
}

/// 点のカーブのスプライン（恒等なら `None`）。点は丸めた設定（昇順・重複なし・0〜1）であること。
fn point_spline(points: &[CurvePoint]) -> Result<Option<MonotoneCubic>> {
    if ToneCurve::points_are_identity(points) {
        return Ok(None);
    }
    let pts: Vec<(f64, f64)> = points
        .iter()
        .map(|p| (f64::from(p.x), f64::from(p.y)))
        .collect();
    MonotoneCubic::new(&pts).map(Some)
}

/// パラメトリックのカーブのスプライン（すべて 0 なら `None`）。
pub fn parametric_spline(p: &ParametricCurve) -> Result<Option<MonotoneCubic>> {
    if *p == ParametricCurve::default() {
        return Ok(None);
    }
    let amounts = [p.shadows, p.darks, p.lights, p.highlights];
    let mut pts = vec![(0.0, 0.0)];
    for (c, a) in PARAMETRIC_CENTERS.iter().zip(amounts) {
        pts.push((*c, c + PARAMETRIC_AMPLITUDE * f64::from(a) / 100.0));
    }
    pts.push((1.0, 1.0));
    MonotoneCubic::new(&pts).map(Some)
}

/// 0〜1 の外を傾き 1 で延長した LUT の評価（モジュールの doc）。
#[inline]
pub fn eval_extended(lut: &Lut1d, e: f32) -> f32 {
    let inside = e.clamp(0.0, 1.0);
    lut.eval(inside) + (e - inside)
}

/// ステージ 16 のパラメータ（GPU 版にも同じ表を渡す）。
#[derive(Debug, Clone, PartialEq)]
pub struct ToneCurveParams {
    /// 輝度のカーブ（パラメトリック → 点のカーブ）。恒等なら `None`。
    pub luma: Option<Lut1d>,
    /// R・G・B のチャンネル別のカーブ。恒等なら `None`。
    pub channels: [Option<Lut1d>; 3],
    /// 黒の持ち上げ `Y0' = srgb_decode(curve(0))`（リニア。モジュールの doc）。輝度のカーブがない、
    /// または黒を持ち上げないカーブなら 0。
    pub black_lift: f32,
    /// 暗部の比率 `r_min = |Y'(2^−16) − Y0'| / 2^−16`（モジュールの doc）。
    pub ratio_min: f32,
}

impl ToneCurveParams {
    /// トーンカーブの設定（丸めたもの）から作る。すべて恒等なら `None`。
    pub fn from_curve(curve: &ToneCurve) -> Result<Option<Self>> {
        let param = parametric_spline(&curve.parametric)?;
        let luma_points = point_spline(&curve.luma)?;
        let luma = match (param, luma_points) {
            (None, None) => None,
            (p, q) => Some(Lut1d::from_fn(TONE_CURVE_LUT_SIZE, 0.0, 1.0, |x| {
                let x = p.as_ref().map_or(x, |s| s.eval(x));
                q.as_ref().map_or(x, |s| s.eval(x))
            })?),
        };
        let mut channels = [None, None, None];
        for (slot, points) in channels
            .iter_mut()
            .zip([&curve.red, &curve.green, &curve.blue])
        {
            if let Some(s) = point_spline(points)? {
                *slot = Some(Lut1d::from_fn(TONE_CURVE_LUT_SIZE, 0.0, 1.0, |x| {
                    s.eval(x)
                })?);
            }
        }
        if luma.is_none() && channels.iter().all(Option::is_none) {
            return Ok(None);
        }
        let (black_lift, ratio_min) = match &luma {
            Some(lut) => {
                // 画素ごとの式（apply）と同じ f32 の計算で求める（Y = 0 と Y = 2^−16 の値）。
                let lift = srgb_decode_f32(eval_extended(lut, srgb_encode_f32(0.0)));
                let at_floor = srgb_decode_f32(eval_extended(lut, srgb_encode_f32(LOG_FLOOR)));
                (lift, (at_floor - lift).abs() / LOG_FLOOR)
            }
            None => (0.0, 1.0),
        };
        Ok(Some(Self {
            luma,
            channels,
            black_lift,
            ratio_min,
        }))
    }

    /// 1 画素に適用する（モジュールの doc の式）。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let mut c = c;
        if let Some(lut) = &self.luma {
            let y = luminance(c);
            let y2 = srgb_decode_f32(eval_extended(lut, srgb_encode_f32(y)));
            let r = if y >= LOG_FLOOR {
                (y2 - self.black_lift).abs() / y
            } else {
                self.ratio_min
            };
            c = [
                y2 + r * (c[0] - y),
                y2 + r * (c[1] - y),
                y2 + r * (c[2] - y),
            ];
        }
        for (v, lut) in c.iter_mut().zip(&self.channels) {
            if let Some(lut) = lut {
                *v = srgb_decode_f32(eval_extended(lut, srgb_encode_f32(*v)));
            }
        }
        c
    }
}

/// ステージ 16: トーンカーブ（B3）。カーブがすべて恒等なら飛ばす。
#[derive(Debug, Clone, Copy, Default)]
pub struct ToneCurveStage;

impl ToneCurveStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.tone_curve";
    /// 04 の 2.1 節のステージの番号。
    pub const NUMBER: u8 = 16;
}

impl Stage for ToneCurveStage {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn phase(&self) -> Phase {
        Phase::C
    }
    fn input_contract(&self) -> ColorContract {
        ColorContract::B3DisplayReferred
    }
    fn output_contract(&self) -> ColorContract {
        ColorContract::B3DisplayReferred
    }
    fn params(
        &self,
        settings: &DevelopSettings,
        ctx: &StageContext<'_>,
    ) -> Result<Option<StageParams>> {
        match ctx.process_version {
            ProcessVersion::V1 => Ok(ToneCurveParams::from_curve(
                &settings.normalized().tone_curve,
            )?
            .map(StageParams::new)),
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
        let p = params.expect::<ToneCurveParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 => map_pixels(input, output, |_, _, c| p.apply(c)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finish::BT2020_LUMINANCE;

    const BT2020_LUMINANCE_R: f32 = BT2020_LUMINANCE[0];

    fn curve_with_luma(points: &[(f32, f32)]) -> ToneCurve {
        ToneCurve {
            luma: points.iter().map(|&(x, y)| CurvePoint::new(x, y)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn spline_passes_through_points_and_is_monotonic() {
        let s = MonotoneCubic::new(&[(0.0, 0.0), (0.25, 0.4), (0.5, 0.45), (1.0, 1.0)]).unwrap();
        for (x, y) in [(0.0, 0.0), (0.25, 0.4), (0.5, 0.45), (1.0, 1.0)] {
            assert!((s.eval(x) - y).abs() < 1e-12);
        }
        let mut prev = -1.0;
        for k in 0..=1000 {
            let v = s.eval(k as f64 / 1000.0);
            assert!(v >= prev - 1e-12, "{k}: {v} < {prev}");
            prev = v;
        }
        // 範囲の外は端の値。
        assert_eq!(s.eval(-1.0), 0.0);
        assert_eq!(s.eval(2.0), 1.0);
        // 平らな区間は平ら（行き過ぎない）。
        let f = MonotoneCubic::new(&[(0.0, 0.0), (0.3, 0.5), (0.7, 0.5), (1.0, 1.0)]).unwrap();
        for k in 0..=100 {
            let x = 0.3 + 0.4 * k as f64 / 100.0;
            assert!((f.eval(x) - 0.5).abs() < 1e-12);
        }
        // 直線上の点は直線。
        let l = MonotoneCubic::new(&[(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)]).unwrap();
        assert!((l.eval(0.3) - 0.3).abs() < 1e-12);
        assert!(MonotoneCubic::new(&[(0.0, 0.0)]).is_err());
        assert!(MonotoneCubic::new(&[(0.5, 0.0), (0.5, 1.0)]).is_err());
        assert!(MonotoneCubic::new(&[(0.0, f64::NAN), (0.5, 1.0)]).is_err());
    }

    #[test]
    fn identity_curves_are_skipped() {
        assert!(
            ToneCurveParams::from_curve(&ToneCurve::default())
                .unwrap()
                .is_none()
        );
        let c = curve_with_luma(&[(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)]);
        assert!(ToneCurveParams::from_curve(&c).unwrap().is_none());
    }

    #[test]
    fn luma_curve_preserves_hue_and_is_monotonic() {
        // S 字のカーブ。
        let c = curve_with_luma(&[(0.0, 0.0), (0.25, 0.18), (0.75, 0.85), (1.0, 1.0)]);
        let p = ToneCurveParams::from_curve(&c).unwrap().unwrap();
        // 無彩色のランプで単調。
        let mut prev = -1.0f32;
        for k in 0..=1000 {
            let y = k as f32 / 1000.0;
            let out = p.apply([y; 3]);
            assert!(out[0] >= prev, "{y}");
            assert!((out[0] - out[1]).abs() < 1e-6 && (out[1] - out[2]).abs() < 1e-6);
            prev = out[0];
        }
        // 符号化した値でカーブの点を通る: 輝度の符号化値 0.25 → 0.18。
        let y = srgb_decode_f32(0.25);
        let out = p.apply([y; 3]);
        assert!((srgb_encode_f32(out[0]) - 0.18).abs() < 1e-3, "{out:?}");
        // RGB の比率（色相）は保たれる。
        let col = [0.3, 0.15, 0.05];
        let out = p.apply(col);
        assert!((out[0] / out[1] - 2.0).abs() < 1e-5 && (out[1] / out[2] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn parametric_curve_moves_regions_and_keeps_endpoints() {
        let mut c = ToneCurve::default();
        c.parametric.shadows = 100.0;
        c.parametric.highlights = -100.0;
        let p = ToneCurveParams::from_curve(&c).unwrap().unwrap();
        let lut = p.luma.as_ref().unwrap();
        assert!(lut.eval(0.0).abs() < 1e-7);
        assert!((lut.eval(1.0) - 1.0).abs() < 1e-7);
        assert!((lut.eval(0.125) - 0.225).abs() < 1e-4);
        assert!((lut.eval(0.875) - 0.775).abs() < 1e-4);
        // 極端な組み合わせでも単調。
        let c = ToneCurve {
            parametric: ParametricCurve {
                highlights: -100.0,
                lights: 100.0,
                darks: -100.0,
                shadows: 100.0,
            },
            ..Default::default()
        };
        let p = ToneCurveParams::from_curve(&c).unwrap().unwrap();
        let v = p.luma.as_ref().unwrap().values();
        assert!(v.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn channel_curves_apply_per_channel() {
        let c = ToneCurve {
            red: vec![CurvePoint::new(0.0, 0.1), CurvePoint::new(1.0, 1.0)],
            ..Default::default()
        };
        let p = ToneCurveParams::from_curve(&c).unwrap().unwrap();
        assert!(p.luma.is_none());
        let out = p.apply([0.0, 0.0, 0.0]);
        assert!((srgb_encode_f32(out[0]) - 0.1).abs() < 1e-4);
        assert_eq!(out[1], 0.0);
        assert_eq!(out[2], 0.0);
    }

    #[test]
    fn dark_and_negative_luminance_stay_finite() {
        // 黒を持ち上げるカーブ。
        let c = curve_with_luma(&[(0.0, 0.1), (1.0, 1.0)]);
        let p = ToneCurveParams::from_curve(&c).unwrap().unwrap();
        for col in [
            [0.0; 3],
            [1e-9, 0.0, 2e-9],
            [-0.01, 0.002, 0.001],
            [-1.0, -1.0, -1.0],
        ] {
            let out = p.apply(col);
            assert!(out.iter().all(|v| v.is_finite()), "{col:?} → {out:?}");
        }
        // 輝度 0 の黒は持ち上がった灰色になる。
        let out = p.apply([0.0; 3]);
        assert!((srgb_encode_f32(out[0]) - 0.1).abs() < 1e-4);
        assert!((out[0] - out[1]).abs() < 1e-7);
    }

    /// 無彩色からの差の大きさ（RGB − Y の最大の絶対値）。
    fn chroma_extent(c: [f32; 3]) -> f32 {
        let y = luminance(c);
        c.iter().map(|v| (v - y).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn lifted_black_does_not_amplify_shadow_noise_into_vivid_dots() {
        // 黒を符号化した値で 0.1 に持ち上げるカーブ（よく使う「フェード」の形）。
        let c = curve_with_luma(&[(0.0, 0.1), (1.0, 1.0)]);
        let p = ToneCurveParams::from_curve(&c).unwrap().unwrap();
        assert!(
            p.black_lift > 0.009 && p.black_lift < 0.011,
            "{}",
            p.black_lift
        );
        // 暗部の雑音の画素（輝度はほぼ 0 だが、カメラ行列の後でチャンネルが正負に散らばった色）。
        // 2.6 節の Y' / Y をそのまま掛けると (0.20, −0.08, 0.20) の鮮やかなマゼンタ、青の点は
        // (0, 0, 0.17) になっていた。持ち上げた黒（リニアで約 0.01）より色の成分が大きくならない。
        for noise in [
            [1e-3f32, -4e-4, 1e-3],
            [0.0, 0.0, 3e-4],
            [-2e-4, 3e-4, -1e-4],
            [2e-5, 0.0, -1e-5],
        ] {
            let out = p.apply(noise);
            assert!(out.iter().all(|v| v.is_finite()), "{noise:?} → {out:?}");
            assert!(
                chroma_extent(out) < 0.01,
                "{noise:?} → {out:?}（色の成分 {}）",
                chroma_extent(out)
            );
            // 輝度は Y'（カーブの値）のまま。
            let y = luminance(noise);
            let y2 = srgb_decode_f32(eval_extended(p.luma.as_ref().unwrap(), srgb_encode_f32(y)));
            assert!((luminance(out) - y2).abs() < 1e-6, "{noise:?} → {out:?}");
        }
        // 色相（無彩色からの向き）は保つ: 色の成分は元の色の成分の正の倍数。
        let col = [0.05f32, 0.02, 0.01];
        let out = p.apply(col);
        let (y0, y1) = (luminance(col), luminance(out));
        let k0 = (out[0] - y1) / (col[0] - y0);
        for i in 0..3 {
            let k = (out[i] - y1) / (col[i] - y0);
            assert!(k > 0.0 && (k - k0).abs() < 1e-3 * k0, "{col:?} → {out:?}");
        }
        // 暗部の比率は Y = 2^−16 の前後でつながる（輝度が下限の少し下・少し上で、同じ色の成分）。
        let d = 2e-6f32;
        let at = |target: f32| {
            let base = target - BT2020_LUMINANCE_R * d;
            [base + d, base, base]
        };
        let (lo, hi) = (at(LOG_FLOOR * 0.99), at(LOG_FLOOR * 1.01));
        assert!(luminance(lo) < LOG_FLOOR && luminance(hi) >= LOG_FLOOR);
        let (below, above) = (p.apply(lo), p.apply(hi));
        for i in 0..3 {
            assert!((below[i] - above[i]).abs() < 1e-5, "{below:?} {above:?}");
        }
    }

    #[test]
    fn curves_without_lift_keep_the_ratio_exactly() {
        // 黒を持ち上げないカーブ（S 字・パラメトリック）では 2.6 節の式（Y' / Y を掛ける）そのもの。
        let c = curve_with_luma(&[(0.0, 0.0), (0.25, 0.18), (0.75, 0.85), (1.0, 1.0)]);
        let p = ToneCurveParams::from_curve(&c).unwrap().unwrap();
        assert_eq!(p.black_lift, 0.0);
        let lut = p.luma.as_ref().unwrap();
        for col in [[0.3f32, 0.15, 0.05], [0.01, 0.002, 0.004], [0.6, 0.7, 0.2]] {
            let y = luminance(col);
            let y2 = srgb_decode_f32(eval_extended(lut, srgb_encode_f32(y)));
            let r = y2 / y;
            let expect = [
                y2 + r * (col[0] - y),
                y2 + r * (col[1] - y),
                y2 + r * (col[2] - y),
            ];
            assert_eq!(p.apply(col), expect);
        }
        let mut pc = ToneCurve::default();
        pc.parametric.shadows = 60.0;
        let p = ToneCurveParams::from_curve(&pc).unwrap().unwrap();
        assert_eq!(p.black_lift, 0.0);
    }

    #[test]
    fn non_monotone_curves_never_flip_the_hue() {
        // 黒を持ち上げて途中で下がるカーブ（Y' < Y0' の部分がある）。色の成分の倍率は 0 以上。
        let c = curve_with_luma(&[(0.0, 0.3), (0.4, 0.1), (1.0, 1.0)]);
        let p = ToneCurveParams::from_curve(&c).unwrap().unwrap();
        for k in 1..100 {
            let s = k as f32 / 100.0;
            let col = [0.4 * s, 0.2 * s, 0.1 * s];
            let out = p.apply(col);
            let (y0, y1) = (luminance(col), luminance(out));
            let ratio = (out[0] - y1) / (col[0] - y0);
            assert!(ratio >= 0.0 && ratio.is_finite(), "{col:?} → {out:?}");
        }
    }
}
