//! ステージ 15: シーン → ディスプレイ変換（docs/04_architecture.md の 2.1 節、2.6 節「3 つの
//! 圧縮・変換」の 1「トーンの圧縮」）。B2 → B3。
//!
//! シーンリニアの値（1 を超えうる）を、ディスプレイの表示範囲（輝度 0〜1）に滑らかに収める
//! 基本カーブ。**仮置き**: カーブの形は PoC-4 で候補を比べて選ぶ。HDR 表示（CLR-03）ではこの
//! ステージを差し替える。
//!
//! # カーブ（輝度に適用）
//!
//! ```text
//! f(Y) = Y                                   （Y ≤ K）
//! f(Y) = K + (1 − K) · t / (1 + t)           （Y > K、t = min((Y − K) / (1 − K), 10^30)）
//! ```
//!
//! - 単調増加、`f(0) = 0`、`K` で傾き 1 のままつながる（1 階微分まで連続）。`Y → ∞` で 1 に近づき、
//!   1 を超えない（`t` の上限は無限大どうしの割り算で NaN を出さないため）。
//! - **中間グレーの対応**: `K` より暗い部分は変えないので、シーンの 0.18 はディスプレイの 0.18
//!   （sRGB の 8bit で約 118）になる。白（シーンの 1.0）は `K + (1 − K) / 2`。
//!
//! # 色の扱い（画素ごと。f32、この順序）
//!
//! ```text
//! Y  = 0.2627·R + 0.6780·G + 0.0593·B          （BT.2020）
//! Y' = f(Y)、 r = Y' / Y（Y > K のとき。それ以外は 1）
//! c  = RGB × r                                  （RGB の比率を保つ → 色相を保つ）
//! m  = max(c_R, c_G, c_B)
//! m > 1 なら: s = (1 − Y') / (m − Y')、 c = Y' + s·(c − Y')   （輝度に向かって彩度を下げ、最大を 1 に）
//! ```
//!
//! - 比率を保つと 1 を超えるチャンネルは、輝度（`Y'`）を保ったまま無彩色の方向へ寄せる
//!   （明るい鮮やかな色が白へ向かう）。
//! - 色域外（負の値）は保持する（切り捨てない。色域の圧縮はステージ 17 の役割）。輝度が 0 以下の
//!   画素も `r = 1` で、そのまま通る（最大が 1 を超えれば彩度だけ下げる）。

use genzo_model::{DevelopSettings, Phase};

use super::{luminance, map_pixels};
use crate::contract::ColorContract;
use crate::error::Result;
use crate::image::{ImageTile, Roi};
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// カーブの膝（これより暗い輝度は変えない）。仮置き: 中間グレー（0.18）を変えずに、ハイライトの
/// 圧縮の余地を残す値として 0.5 にした（シーンの 1.0 → 0.75、4.0 → 0.94）。PoC-4 で決める。
pub const SCENE_TO_DISPLAY_KNEE: f32 = 0.5;

/// `t` の上限（`t / (1 + t)` で NaN を出さないため）。GPU 版（`genzo-gpu`）も同じ値を使う。
pub const SCENE_TO_DISPLAY_T_MAX: f32 = 1e30;

/// ステージ 15 のパラメータ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneToDisplayParams {
    /// 膝 `K`（0 < K < 1）。
    pub knee: f32,
}

impl Default for SceneToDisplayParams {
    fn default() -> Self {
        Self {
            knee: SCENE_TO_DISPLAY_KNEE,
        }
    }
}

impl SceneToDisplayParams {
    /// 輝度のカーブ `f`。
    #[inline]
    pub fn curve(&self, y: f32) -> f32 {
        let k = self.knee;
        if y <= k {
            return y;
        }
        let t = ((y - k) / (1.0 - k)).min(SCENE_TO_DISPLAY_T_MAX);
        k + (1.0 - k) * (t / (1.0 + t))
    }

    /// 1 画素に適用する（モジュールの doc の「色の扱い」）。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let y = luminance(c);
        let (y2, c) = if y > self.knee {
            let y2 = self.curve(y);
            let r = y2 / y;
            (y2, [c[0] * r, c[1] * r, c[2] * r])
        } else {
            (y, c)
        };
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

/// ステージ 15: シーン → ディスプレイ変換（B2 → B3）。常に適用する（パラメータは処理バージョンで
/// 決まり、現像設定には依存しない）。
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
            ProcessVersion::V1 => Ok(Some(StageParams::new(SceneToDisplayParams::default()))),
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
        let p = *params.expect::<SceneToDisplayParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 => map_pixels(input, output, |_, _, c| p.apply(c)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_is_monotonic_bounded_and_keeps_middle_gray() {
        let p = SceneToDisplayParams::default();
        assert_eq!(p.curve(0.0), 0.0);
        assert_eq!(p.curve(0.18), 0.18);
        assert!((p.curve(1.0) - 0.75).abs() < 1e-6);
        let mut prev = -1.0f32;
        for k in 0..=2000 {
            let y = (k as f32 / 2000.0 * 30.0 - 20.0).exp2();
            let v = p.curve(y);
            assert!(v >= prev && v <= 1.0, "{y} → {v}");
            prev = v;
        }
        for y in [1e10f32, 1e30, f32::MAX] {
            let v = p.curve(y);
            assert!(v.is_finite() && v <= 1.0, "{y} → {v}");
        }
        // 膝でなめらか（傾きが 1 のままつながる）。
        let k = SCENE_TO_DISPLAY_KNEE;
        let d = (p.curve(k + 1e-3) - p.curve(k)) / 1e-3;
        assert!((d - 1.0).abs() < 0.01, "{d}");
    }

    #[test]
    fn colors_keep_their_ratio_and_stay_within_one() {
        let p = SceneToDisplayParams::default();
        // 膝より暗い色は変えない。
        let c = [0.3, 0.2, 0.1];
        assert_eq!(p.apply(c), c);
        // 明るい無彩色は輝度のカーブどおり。
        let w = p.apply([4.0; 3]);
        assert!(w.iter().all(|v| (v - p.curve(4.0)).abs() < 1e-6));
        // 明るい色は比率を保つ（1 を超えない範囲）。
        let c = [0.9, 0.6, 0.3];
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
        let p = SceneToDisplayParams::default();
        // 膝より暗い色の負の値はそのまま。
        let c = [-0.05, 0.3, 0.1];
        assert_eq!(p.apply(c), c);
        // 圧縮される明るい色でも、1 を超えなければ負の値は比率どおりに残る。
        let c = [-0.2, 0.9, 0.5];
        let out = p.apply(c);
        assert!(out[0] < 0.0, "{out:?}");
        assert!((out[0] / out[1] - c[0] / c[1]).abs() < 1e-5);
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
}
