//! 自動補正（DEV-32。露光量・コントラスト・ハイライト・シャドウ・白レベル・黒レベルを写真に合わせて決める。
//! [`Engine::auto_tone`]）。**仮実装**（目標の値・重みは合成の画像だけで決めた仮置き。実写で見直す。
//! 決定事項ログ No.200）。
//!
//! # 方法
//!
//! 1. プレビューと同じ段階 A1（最終品質、長辺 `long_edge`）からステージ 9 だけを描いた **基準の画像**
//!    （[`Engine::mask_reference`]。露光量の前の B2。切り抜いた後の C の格子）の、画像の上の画素
//!    （回転で画像の外になった四隅を除く）の輝度 `Y`（BT.2020）の `L = log2(max(Y, 2^−16))` の分布を取る
//!    （[`LuminanceHistogram`]。1/32 段刻み）。今の露光量・コントラスト・トーンの値によらず、同じ写真・同じ
//!    WB・同じ切り抜きなら同じ値になる（ローカル補正・トーンカーブ・プロファイルは見ない）。
//! 2. 黒の点 `lo`（[`BLACK_PERCENTILE`]）・中間 `mid`（中央値）・白の点 `hi`（[`WHITE_PERCENTILE`]）の 3 つの
//!    代表の明るさを取り、[`ToneTargets`]（入力の種類ごとの、表示でちょうどよい黒・中間グレー・白にあたる
//!    シーンの輝度）に合うように、ステージ 10・11 の平らな部分の式（ガイドと画素が同じとみなす）を順に解く
//!    （[`solve_auto_tone`]）:
//!
//!    ```text
//!    L0 = log2(0.18)、W・M・B = 目標の白・中間・黒の log2
//!    コントラスト: 範囲 hi − lo が、目標の範囲 W − B より（白レベル・黒レベルで広げられる分 AUTO_ENDPOINT_REACH_EV を
//!                  引いて）狭ければ、傾き γ = (W − B) / (hi − lo + reach) で広げる（上限 AUTO_CONTRAST_GAMMA_MAX。
//!                  狭めはしない。圧縮はハイライト・シャドウで行う）。スライダー = (γ − 1) / CONTRAST_STRENGTH × 100
//!    露光量:       x → L0 + γ·(x + EV − L0) で、中央値が M になる EV を基本とし、白の点が
//!                  W − AUTO_WHITE_UNDER_EV 〜 W + AUTO_WHITE_OVER_EV に入る範囲に収める（白の点を優先。
//!                  ハイトーンの写真を灰色にせず、白飛びはハイライトで戻せる分だけ許す）。±AUTO_EXPOSURE_LIMIT_EV
//!    ハイライト:   露光量・コントラストの後の白の点が W より上なら、その分をハイライトの重み（ステージ 11 の σ）で
//!                  割った値だけ下げる（−100 まで）
//!    シャドウ:     ハイライトの後の中央値が M より下なら、その分をシャドウの重みで割った値だけ上げる（+100 まで。
//!                  下げはしない）
//!    白レベル:     ハイライト・シャドウの後の白の点と W の差を、白の重みで割った値（±100）
//!    黒レベル:     同じく黒の点と B の差を、黒の重みで割った値（±100）
//!    ```
//!
//!    各段で、前の段の値を丸めた（スライダーは整数、露光量は 0.01 段）後の効果を次の段の代表の明るさに
//!    反映する。
//!
//! # 決めごと
//!
//! - **適用はしない**: 値を返すだけ（WB のスポイトと同じ）。UI・CLI が設定に入れて履歴に 1 件記録する。
//! - **目標**（[`ToneTargets`]）: RAW（シーン参照）はステージ 15 の基本カーブで、シーンの 0.0731（= 0.18 / 2^1.3）が
//!   表示の中間グレー 0.18、シーンの 1.0 が白になる（`scene_to_display` の doc）ので、中間は 0.0731、白は 0.85
//!   （表示の約 0.98）、黒は 0.003（表示の約 0.0015。sRGB の 8bit で約 5）。RAW 以外（ディスプレイ参照。
//!   ステージ 15 は恒等）は中間 0.18、白 0.9、黒 0.002。**仮置き**。
//! - **代表の明るさ**: 黒は下から 0.5%、白は上から 0.5%（鏡面の反射・光源が白の点を決めないように）、中間は
//!   中央値。**仮置き**。
//! - **求められない写真**: 画像の上の画素が 1 つもなければ [`PipelineError::InvalidArgument`]。真っ黒な写真
//!   （すべて 2^−16 以下）でも値は求まる（露光量は上限、黒レベルは ±100 の範囲）。
//! - 返す値は genzo-model の `ranges` に収める（露光量 ±5、ほかは ±100）。

use genzo_model::DevelopSettings;
use genzo_model::develop::ranges;

use super::{Engine, PhotoSource};
use crate::error::{PipelineError, Result};
use crate::finish::exposure::CONTRAST_STRENGTH;
use crate::finish::tone::{
    BLACKS_CENTER_EV, BLACKS_MAX_EV, BLACKS_WIDTH_EV, HIGHLIGHTS_CENTER_EV, HIGHLIGHTS_MAX_EV,
    HIGHLIGHTS_WIDTH_EV, SHADOWS_CENTER_EV, SHADOWS_MAX_EV, SHADOWS_WIDTH_EV, WHITES_CENTER_EV,
    WHITES_MAX_EV, WHITES_WIDTH_EV,
};
use crate::finish::{LOG2_MIDDLE_GRAY, luminance};
use crate::sanitize::safe_log2;
use crate::stage::InputKind;

/// 黒の点の百分位（下から）。仮置き。
pub const BLACK_PERCENTILE: f64 = 0.005;
/// 白の点の百分位（下から）。仮置き。
pub const WHITE_PERCENTILE: f64 = 0.995;
/// 露光量の上限（段）。仮置き。
pub const AUTO_EXPOSURE_LIMIT_EV: f32 = 3.0;
/// 露光量を決めるとき、白の点が目標より上に出てよい段数（ハイライトで戻せる分）。仮置き。
pub const AUTO_WHITE_OVER_EV: f32 = 1.0;
/// 露光量を決めるとき、白の点が目標より下に残ってよい段数（白レベルで上げる分）。仮置き。
pub const AUTO_WHITE_UNDER_EV: f32 = 0.5;
/// コントラストで範囲を広げるときの傾きの上限（1.1 → スライダー +25）。仮置き。
pub const AUTO_CONTRAST_GAMMA_MAX: f32 = 1.1;
/// 白レベル・黒レベルで広げられるとみなす段数（コントラストを決めるときに範囲から引く）。仮置き。
pub const AUTO_ENDPOINT_REACH_EV: f32 = 1.0;

/// 輝度の分布の log2 の下限（`2^−16`。[`crate::sanitize::LOG_FLOOR`]）。
const HIST_MIN: f32 = -16.0;
/// 輝度の分布の log2 の上限（これより明るい画素は最後の区間に入れる）。
const HIST_MAX: f32 = 8.0;
/// 1 段あたりの区間の数。
const HIST_BINS_PER_EV: usize = 32;
/// 区間の数。
const HIST_BINS: usize = ((HIST_MAX - HIST_MIN) as usize) * HIST_BINS_PER_EV;

/// 自動補正の結果の値（そのまま設定に入れられる。genzo-model の `ranges` に収めた値）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AutoToneValues {
    /// 露光量（EV。0.01 段に丸めた値）。
    pub exposure_ev: f32,
    /// コントラスト（整数。0 以上）。
    pub contrast: f32,
    /// ハイライト（整数。0 以下）。
    pub highlights: f32,
    /// シャドウ（整数。0 以上）。
    pub shadows: f32,
    /// 白レベル（整数）。
    pub whites: f32,
    /// 黒レベル（整数）。
    pub blacks: f32,
}

impl AutoToneValues {
    /// 設定に入れる（露光量・コントラスト・ハイライト・シャドウ・白レベル・黒レベルだけを置き換える）。
    pub fn apply(&self, settings: &mut DevelopSettings) {
        settings.exposure_ev = self.exposure_ev;
        settings.contrast = self.contrast;
        settings.tone.highlights = self.highlights;
        settings.tone.shadows = self.shadows;
        settings.tone.whites = self.whites;
        settings.tone.blacks = self.blacks;
    }
}

/// 自動補正に使った代表の明るさ（log2 の輝度。露光量の前の B2）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoToneStats {
    /// 数えた画素の数（画像の上の画素）。
    pub samples: u64,
    /// 黒の点（下から [`BLACK_PERCENTILE`]）。
    pub black_log2: f32,
    /// 中央値。
    pub median_log2: f32,
    /// 白の点（下から [`WHITE_PERCENTILE`]）。
    pub white_log2: f32,
}

/// 自動補正の結果（[`Engine::auto_tone`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoTonePick {
    /// 値。
    pub values: AutoToneValues,
    /// 代表の明るさ。
    pub stats: AutoToneStats,
}

/// 入力の種類ごとの目標（露光量の前の輝度。モジュールの doc の「決めごと」）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneTargets {
    /// 白の点の目標の輝度。
    pub white: f32,
    /// 中央値の目標の輝度。
    pub mid: f32,
    /// 黒の点の目標の輝度。
    pub black: f32,
}

impl ToneTargets {
    /// RAW（シーン参照。ステージ 15 の基本カーブを通る）の目標。仮置き。
    pub const SCENE_REFERRED: Self = Self {
        white: 0.85,
        mid: 0.18 / 2.462_288_8, // 0.18 / 2^1.3
        black: 0.003,
    };
    /// RAW 以外（ディスプレイ参照。ステージ 15 は恒等）の目標。仮置き。
    pub const DISPLAY_REFERRED: Self = Self {
        white: 0.9,
        mid: 0.18,
        black: 0.002,
    };

    /// 入力の種類の目標。
    pub const fn for_input(kind: InputKind) -> Self {
        match kind {
            InputKind::SceneReferred => Self::SCENE_REFERRED,
            InputKind::DisplayReferred => Self::DISPLAY_REFERRED,
        }
    }
}

/// 輝度の log2 の分布（1/32 段刻み。−16〜8 段）。
#[derive(Debug, Clone)]
pub struct LuminanceHistogram {
    counts: Vec<u64>,
    total: u64,
}

impl Default for LuminanceHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl LuminanceHistogram {
    /// 空の分布。
    pub fn new() -> Self {
        Self {
            counts: vec![0; HIST_BINS],
            total: 0,
        }
    }

    /// 画素（B2）を 1 つ数える。
    #[inline]
    pub fn add(&mut self, rgb: [f32; 3]) {
        self.add_log2(safe_log2(luminance(rgb)));
    }

    /// log2 の輝度を 1 つ数える。
    #[inline]
    pub fn add_log2(&mut self, l: f32) {
        let bin = ((l - HIST_MIN) * HIST_BINS_PER_EV as f32).floor();
        let bin = if bin.is_nan() {
            0
        } else {
            (bin as i64).clamp(0, HIST_BINS as i64 - 1) as usize
        };
        self.counts[bin] += 1;
        self.total += 1;
    }

    /// 数えた画素の数。
    pub fn total(&self) -> u64 {
        self.total
    }

    /// 百分位 `p`（0〜1）の log2 の輝度（区間の中で線形に補間する）。画素がなければ `None`。
    pub fn percentile(&self, p: f64) -> Option<f32> {
        if self.total == 0 {
            return None;
        }
        let want = (p.clamp(0.0, 1.0) * self.total as f64).max(f64::EPSILON);
        let mut before = 0u64;
        for (i, &c) in self.counts.iter().enumerate() {
            if c == 0 {
                continue;
            }
            let after = before + c;
            if (after as f64) >= want {
                let frac = ((want - before as f64) / c as f64).clamp(0.0, 1.0);
                let lo = HIST_MIN + i as f32 / HIST_BINS_PER_EV as f32;
                return Some(lo + frac as f32 / HIST_BINS_PER_EV as f32);
            }
            before = after;
        }
        Some(HIST_MAX)
    }
}

/// σ(z) = 1 / (1 + exp2(−z))（ステージ 11 と同じ）。
#[inline]
fn sigmoid2(z: f32) -> f32 {
    1.0 / (1.0 + (-z).exp2())
}

/// ハイライト +100 が、局所の明るさ `b`（中間グレーからの段数）の平らな部分を動かす段数。
fn highlights_reach(b: f32) -> f32 {
    HIGHLIGHTS_MAX_EV * sigmoid2((b - HIGHLIGHTS_CENTER_EV) / HIGHLIGHTS_WIDTH_EV)
}

/// シャドウ +100 の同上。
fn shadows_reach(b: f32) -> f32 {
    SHADOWS_MAX_EV * sigmoid2(-(b - SHADOWS_CENTER_EV) / SHADOWS_WIDTH_EV)
}

/// 白レベル +100 が、画素の明るさ `l1`（中間グレーからの段数）を動かす段数。
fn whites_reach(l1: f32) -> f32 {
    WHITES_MAX_EV * sigmoid2((l1 - WHITES_CENTER_EV) / WHITES_WIDTH_EV)
}

/// 黒レベル +100 の同上。
fn blacks_reach(l1: f32) -> f32 {
    BLACKS_MAX_EV * sigmoid2(-(l1 - BLACKS_CENTER_EV) / BLACKS_WIDTH_EV)
}

/// `need` 段を動かすのに要るスライダーの値（−1〜1。`reach` が 0 以下なら 0）。
fn ratio(need: f32, reach: f32) -> f32 {
    if reach > 0.0 && need.is_finite() {
        (need / reach).clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// スライダー（−1〜1）を整数のスライダーの値に丸める。
fn slider(unit: f32) -> f32 {
    ranges::SLIDER.clamp_or((unit * 100.0).round(), 0.0)
}

/// 代表の明るさ（log2 の輝度。黒・中間・白）から値を決める（モジュールの doc の「方法」の 2）。純粋な関数。
pub fn solve_auto_tone(targets: ToneTargets, lo: f32, mid: f32, hi: f32) -> AutoToneValues {
    let l0 = LOG2_MIDDLE_GRAY;
    let w = targets.white.log2();
    let m = targets.mid.log2();
    let b = targets.black.log2();
    // 1. コントラスト（範囲が狭ければ広げる）。
    let range = hi - lo;
    let gamma = if range > 0.0 {
        ((w - b) / (range + AUTO_ENDPOINT_REACH_EV)).clamp(1.0, AUTO_CONTRAST_GAMMA_MAX)
    } else {
        1.0
    };
    let contrast = slider((gamma - 1.0) / CONTRAST_STRENGTH).max(0.0);
    let gamma = 1.0 + CONTRAST_STRENGTH * contrast / 100.0;
    // 2. 露光量（中央値を M に。白の点の許容の範囲に収める）。
    let ev_mid = (m - l0) / gamma + l0 - mid;
    let ev_hi = (w - l0) / gamma + l0 - hi;
    // `f32::max` / `min` は NaN でない方を返す（代表の明るさが NaN でも落ちない。`clamp` は NaN の境界で panic する）。
    let ev = ev_mid
        .max(ev_hi - AUTO_WHITE_UNDER_EV / gamma)
        .min(ev_hi + AUTO_WHITE_OVER_EV / gamma)
        .clamp(-AUTO_EXPOSURE_LIMIT_EV, AUTO_EXPOSURE_LIMIT_EV);
    let exposure_ev = ranges::EXPOSURE_EV.clamp_or((ev * 100.0).round() / 100.0, 0.0);
    let map = |x: f32| l0 + gamma * (x + exposure_ev - l0);
    let (hi2, mid2, lo2) = (map(hi), map(mid), map(lo));
    // 3. ハイライト（白の点が目標より上なら下げる）。
    let over = hi2 - w;
    let h = if over > 0.0 {
        -ratio(over, highlights_reach(hi2 - l0))
    } else {
        0.0
    };
    let highlights = slider(h).min(0.0);
    let h = highlights / 100.0;
    let after_h = |x: f32| x + h * highlights_reach(x - l0);
    let (hi3, mid3, lo3) = (after_h(hi2), after_h(mid2), after_h(lo2));
    // 4. シャドウ（中央値が目標より下なら上げる）。
    let deficit = m - mid3;
    let s = if deficit > 0.0 {
        ratio(deficit, shadows_reach(mid3 - l0))
    } else {
        0.0
    };
    let shadows = slider(s).max(0.0);
    let s = shadows / 100.0;
    let after_s = |x: f32| x + s * shadows_reach(x - l0);
    let (hi4, lo4) = (after_s(hi3), after_s(lo3));
    // 5. 白レベル（白の点を目標に）。
    let whites = slider(ratio(w - hi4, whites_reach(hi4 - l0)));
    let wv = whites / 100.0;
    let lo5 = lo4 + wv * whites_reach(lo4 - l0);
    // 6. 黒レベル（黒の点を目標に）。
    let blacks = slider(ratio(b - lo5, blacks_reach(lo5 - l0)));
    AutoToneValues {
        exposure_ev,
        contrast,
        highlights,
        shadows,
        whites,
        blacks,
    }
}

impl Engine {
    /// 自動補正の値を求める（モジュールの doc の「方法」）。設定は変えない。`long_edge` はプレビューの長辺
    /// （ふつうは [`crate::EngineConfig::preview_long_edge`]。A1 がキャッシュにあればそれを使う）。
    ///
    /// エラー: 処理バージョンを扱えない、A1 を作れない、画像の上の画素がない（[`PipelineError::InvalidArgument`]）。
    pub fn auto_tone(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        long_edge: u32,
    ) -> Result<AutoTonePick> {
        let r = self.mask_reference(source, settings, long_edge, false)?;
        let g = &r.geometry;
        let (w, h) = g.output_size();
        let data = r.tile.image.data();
        let mut hist = LuminanceHistogram::new();
        for y in 0..h {
            let row = &data[(y * w) as usize..((y + 1) * w) as usize];
            for (x, p) in row.iter().enumerate() {
                if g.c_pixel_outside_image(x as i64, i64::from(y)) {
                    continue;
                }
                hist.add(*p);
            }
        }
        let (Some(lo), Some(mid), Some(hi)) = (
            hist.percentile(BLACK_PERCENTILE),
            hist.percentile(0.5),
            hist.percentile(WHITE_PERCENTILE),
        ) else {
            return Err(PipelineError::InvalidArgument(
                "画像の上の画素がないため、自動補正を求められません".to_owned(),
            ));
        };
        let targets = ToneTargets::for_input(r.input_kind);
        Ok(AutoTonePick {
            values: solve_auto_tone(targets, lo, mid, hi),
            stats: AutoToneStats {
                samples: hist.total(),
                black_log2: lo,
                median_log2: mid,
                white_log2: hi,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: ToneTargets = ToneTargets::SCENE_REFERRED;

    fn targets_log2(t: ToneTargets) -> (f32, f32, f32) {
        (t.black.log2(), t.mid.log2(), t.white.log2())
    }

    #[test]
    fn targets_match_the_scene_to_display_curve() {
        // RAW の中間の目標はシーンの 0.18 / 2^1.3（ステージ 15 の doc の Sm）。
        assert!((T.mid - 0.18 / 1.3f32.exp2()).abs() < 1e-6);
        let d = ToneTargets::DISPLAY_REFERRED;
        assert!((d.mid - 0.18).abs() < 1e-6);
        for t in [T, d] {
            let ordered = t.black < t.mid && t.mid < t.white && t.white < 1.0;
            assert!(ordered, "{t:?}");
        }
        assert_eq!(ToneTargets::for_input(InputKind::SceneReferred), T);
        assert_eq!(ToneTargets::for_input(InputKind::DisplayReferred), d);
    }

    #[test]
    fn a_photo_already_at_the_targets_gets_no_change() {
        for t in [ToneTargets::SCENE_REFERRED, ToneTargets::DISPLAY_REFERRED] {
            let (b, m, w) = targets_log2(t);
            assert_eq!(solve_auto_tone(t, b, m, w), AutoToneValues::default());
        }
    }

    #[test]
    fn a_uniformly_underexposed_photo_gets_the_exposure_back() {
        let (b, m, w) = targets_log2(T);
        let v = solve_auto_tone(T, b - 2.0, m - 2.0, w - 2.0);
        assert!((v.exposure_ev - 2.0).abs() < 1e-5, "{v:?}");
        assert_eq!(
            (v.contrast, v.highlights, v.shadows, v.whites, v.blacks),
            (0.0, 0.0, 0.0, 0.0, 0.0),
            "{v:?}"
        );
        let v = solve_auto_tone(T, b + 1.0, m + 1.0, w + 1.0);
        assert!((v.exposure_ev + 1.0).abs() < 1e-5, "{v:?}");
        // 露光量の上限（±3 段）を超える分は、シャドウ・黒レベルが補う（符号は上げる向き）。
        let v = solve_auto_tone(T, b - 5.0, m - 5.0, w - 5.0);
        assert_eq!(v.exposure_ev, AUTO_EXPOSURE_LIMIT_EV, "{v:?}");
        assert!(v.shadows > 0.0 && v.whites > 0.0 && v.blacks > 0.0, "{v:?}");
    }

    #[test]
    fn a_dark_subject_against_a_bright_sky_is_lifted_and_the_sky_held() {
        // 白の点は目標（空）、中央値は 2.5 段暗い（逆光の被写体）。露光量は白の許容の上限まで、
        // 超えた白はハイライトで戻し、残りの中間の不足はシャドウで補う。
        let (b, m, w) = targets_log2(T);
        let v = solve_auto_tone(T, b - 2.5, m - 2.5, w);
        assert!((v.exposure_ev - AUTO_WHITE_OVER_EV).abs() < 1e-5, "{v:?}");
        assert!(v.highlights < -50.0, "{v:?}");
        assert!(v.shadows > 50.0, "{v:?}");
        assert_eq!(v.contrast, 0.0, "{v:?}");
    }

    #[test]
    fn a_high_key_photo_is_not_pulled_down_to_middle_gray() {
        // 雪: 中央値が目標より 2 段明るく、白の点は目標の近く。露光量は白の許容の下限まで（−0.5 段）しか
        // 下げず、白レベルで白の点を戻す。シャドウは上げない。
        let (_, m, w) = targets_log2(T);
        let v = solve_auto_tone(T, m - 2.0, m + 2.0, w - 0.1);
        assert!(v.exposure_ev > -0.7 && v.exposure_ev < 0.0, "{v:?}");
        assert!(v.whites > 0.0, "{v:?}");
        assert_eq!(v.shadows, 0.0, "{v:?}");
        assert!(v.blacks < 0.0, "{v:?}");
    }

    #[test]
    fn a_low_contrast_photo_is_expanded_with_contrast_and_the_endpoints() {
        // かすみ: 黒の点は 1.5 段明るく、白の点は 1.5 段暗い。コントラストは上限（+25）、白レベルは上げ、
        // 黒レベルは下げる。ハイライトは使わない。
        let (b, m, w) = targets_log2(T);
        let v = solve_auto_tone(T, b + 1.5, m, w - 1.5);
        assert_eq!(v.contrast, 25.0, "{v:?}");
        assert!(v.whites > 0.0, "{v:?}");
        assert_eq!(v.blacks, -100.0, "{v:?}");
        assert_eq!(v.highlights, 0.0, "{v:?}");
        assert!(v.exposure_ev > 0.0, "{v:?}");
        // 広い範囲（夜景など）ではコントラストは上げない（狭めもしない）。目標より明るい白の点はハイライトで
        // 下げ、目標より暗い黒の点は黒レベルで上げる。
        let v = solve_auto_tone(T, b - 1.0, m, w + 0.5);
        assert_eq!(v.contrast, 0.0, "{v:?}");
        assert!(v.highlights < 0.0 && v.blacks > 0.0, "{v:?}");
    }

    #[test]
    fn values_stay_in_the_setting_ranges_and_are_rounded() {
        let (b, m, w) = targets_log2(T);
        for (lo, mid, hi) in [
            (-16.0, -16.0, -16.0),
            (8.0, 8.0, 8.0),
            (-16.0, m, 8.0),
            (b, m, w),
            (-3.0, -2.0, -1.0),
            (f32::NAN, f32::NAN, f32::NAN),
        ] {
            let v = solve_auto_tone(T, lo, mid, hi);
            assert!(ranges::EXPOSURE_EV.contains(v.exposure_ev), "{v:?}");
            for s in [v.contrast, v.highlights, v.shadows, v.whites, v.blacks] {
                assert!(ranges::SLIDER.contains(s) && s == s.round(), "{v:?}");
            }
            assert!(
                ((v.exposure_ev * 100.0).round() - v.exposure_ev * 100.0).abs() < 1e-3,
                "{v:?}"
            );
            assert!(
                v.contrast >= 0.0 && v.highlights <= 0.0 && v.shadows >= 0.0,
                "{v:?}"
            );
        }
    }

    #[test]
    fn histogram_percentiles_interpolate_and_clamp() {
        let mut h = LuminanceHistogram::new();
        assert_eq!(h.percentile(0.5), None);
        // 1 段ごとに 1 つ（−3〜0 段）。
        for l in [-3.0, -2.0, -1.0, 0.0] {
            h.add_log2(l);
        }
        assert_eq!(h.total(), 4);
        let p = h.percentile(0.5).unwrap();
        assert!(
            (p - -2.0).abs() <= 1.0 / HIST_BINS_PER_EV as f32 + 1e-6,
            "{p}"
        );
        let lo = h.percentile(0.0).unwrap();
        assert!(
            (lo - -3.0).abs() <= 1.0 / HIST_BINS_PER_EV as f32 + 1e-6,
            "{lo}"
        );
        let hi = h.percentile(1.0).unwrap();
        assert!(
            (hi - 0.0).abs() <= 1.0 / HIST_BINS_PER_EV as f32 + 1e-6,
            "{hi}"
        );
        // 範囲の外・NaN は端の区間に入る。
        let mut h = LuminanceHistogram::new();
        h.add_log2(-100.0);
        h.add_log2(100.0);
        h.add_log2(f32::NAN);
        h.add([0.0, 0.0, 0.0]);
        assert_eq!(h.total(), 4);
        assert!(h.percentile(0.0).unwrap() <= HIST_MIN + 1.0 / HIST_BINS_PER_EV as f32);
        assert!(h.percentile(1.0).unwrap() >= HIST_MAX - 1.0 / HIST_BINS_PER_EV as f32);
        // 画素の輝度は log2 で数える。
        let mut h = LuminanceHistogram::new();
        h.add([0.25, 0.25, 0.25]);
        let p = h.percentile(0.5).unwrap();
        assert!(
            (p - -2.0).abs() <= 1.0 / HIST_BINS_PER_EV as f32 + 1e-6,
            "{p}"
        );
    }

    #[test]
    fn apply_replaces_only_the_six_values() {
        let mut s = DevelopSettings::default();
        s.presence.clarity = 30.0;
        let v = AutoToneValues {
            exposure_ev: 0.5,
            contrast: 10.0,
            highlights: -40.0,
            shadows: 30.0,
            whites: 5.0,
            blacks: -15.0,
        };
        v.apply(&mut s);
        assert_eq!(s.exposure_ev, 0.5);
        assert_eq!(s.contrast, 10.0);
        assert_eq!(
            (
                s.tone.highlights,
                s.tone.shadows,
                s.tone.whites,
                s.tone.blacks
            ),
            (-40.0, 30.0, 5.0, -15.0)
        );
        assert_eq!(s.presence.clarity, 30.0);
    }
}
