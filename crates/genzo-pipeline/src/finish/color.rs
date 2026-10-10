//! ステージ 13: 色（彩度・自然な彩度。DEV-08、docs/04_architecture.md の 2.1 節）。
//!
//! リニアな値（B2）を **OKLab**（genzo-color の [`linear_bt2020_to_oklab`]。知覚的な色空間の候補。
//! どれを使うかは PoC-4 で決める）に変換し、明るさ L を保ったまま a・b（彩度の成分）に倍率を
//! 掛けて、リニアに戻す。HSL・カラーグレーディング（v1）は未実装。
//!
//! # 計算（画素ごと。f32、この順序。仮置きの式）
//!
//! ```text
//! (L, a, b) = OKLab(RGB)                      （立方根は符号付き: 負の値（色域外）でも NaN にならない）
//! (an, bn) = (L·na, L·nb)                     （同じ L の無彩色の a・b。下の「無彩色の軸」）
//! (da, db) = (a − an, b − bn)                 （無彩色からの差）
//! C   = sqrt(da·da + db·db)                   （彩度。hypot は使わない: 結果が環境によらないように）
//! ks  = 1 + saturation                        （saturation は −1〜1。−1 で無彩色、+1 で 2 倍）
//! wc  = (1 − min(C / VIBRANCE_CHROMA_REF, 1))²          （彩度の低い色ほど 1 に近い）
//! skin = C > 0 かつ vibrance > 0 のとき:
//!          cosΔ = (da·cos h0 + db·sin h0) / C          （肌色の色相 h0 との角度の余弦）
//!          t = max(0, (cosΔ − cos W) / (1 − cos W))、 skin = t·t
//!        それ以外は 0
//! kv  = 1 + vibrance · VIBRANCE_MAX · wc · (1 − SKIN_PROTECTION · skin)
//! (a', b') = (an + da · ks · kv, bn + db · ks · kv)
//! RGB' = OKLab⁻¹(L, a', b')
//! ```
//!
//! **無彩色の軸**: OKLab の行列（Ottosson の公表値）と BT.2020 の白（D65）の定義の丸めの違いで、
//! 作業色空間の無彩色 (g, g, g) の OKLab は a・b がちょうど 0 にならない（約 1e−4）。無彩色は
//! `(L, L·na, L·nb)`（`(na, nb) = (a, b) / L` を白 (1, 1, 1) で求めたもの。[`ColorStageParams::neutral`]）
//! の直線上にあるので、彩度はこの軸からの差で測り、倍率もこの軸に向かって掛ける。こうすると、
//! 彩度 −100 の結果は R = G = B（丸めの誤差の範囲）になる。
//!
//! - 自然な彩度は、彩度の低い色ほど強く効き（`wc`）、彩度の高い色（`C ≥ VIBRANCE_CHROMA_REF`）は
//!   変えない。上げるとき（vibrance > 0）は、肌色の色相（OKLab で 40〜75° 付近）の効きを弱める
//!   （`SKIN_PROTECTION`）。下げるときは肌色も同じように下げる。
//! - 倍率はどれも 0 以上なので、色相を反転させない。明るさ L を保つので、彩度を 0 にすると同じ
//!   L の無彩色になる（OKLab の L は知覚的な明るさで、輝度 Y とは少し違う）。

use genzo_color::oklab::{linear_bt2020_to_oklab, oklab_to_linear_bt2020};
use genzo_model::{DevelopSettings, Phase};

use super::map_pixels;
use crate::contract::ColorContract;
use crate::error::Result;
use crate::image::{ImageTile, Roi};
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// 自然な彩度 +100 で、無彩色に近い色の彩度を何倍増やすか（`1 + VIBRANCE_MAX` 倍）。仮置き（PoC-4）。
pub const VIBRANCE_MAX: f32 = 1.0;
/// 自然な彩度が効かなくなる彩度（OKLab の C）。仮置き: sRGB の原色の C が 0.2〜0.32 程度なので、
/// それより少し低い 0.25 にした（鮮やかな色は変えない）。PoC-4 で決める。
pub const VIBRANCE_CHROMA_REF: f32 = 0.25;
/// 肌色の色相の中心（OKLab の色相。度）。仮置き: ColorChecker の dark skin・light skin（約 44〜46°）と、
/// 明るい黄みの肌（約 72°）の中間。
pub const SKIN_HUE_DEG: f64 = 55.0;
/// 肌色とみなす色相の半幅（度）。この角度で保護が 0 になる。仮置き。
pub const SKIN_HUE_HALF_WIDTH_DEG: f64 = 35.0;
/// 肌色の保護の強さ（0〜1。1 で肌色の中心では自然な彩度を上げない）。仮置き。
pub const SKIN_PROTECTION: f32 = 0.7;

/// ステージ 13 のパラメータ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorStageParams {
    /// 彩度（−1〜1。スライダー / 100）。
    pub saturation: f32,
    /// 自然な彩度（−1〜1）。
    pub vibrance: f32,
    /// `cos h0`（f64 で計算して f32 に丸めた値）。
    pub skin_cos: f32,
    /// `sin h0`。
    pub skin_sin: f32,
    /// `cos W`。
    pub skin_cos_width: f32,
    /// 無彩色の軸 `(na, nb)`（白 (1, 1, 1) の OKLab の `(a / L, b / L)`。モジュールの doc）。
    pub neutral: (f32, f32),
}

impl ColorStageParams {
    /// スライダーの値（−100〜100）から作る。
    pub fn from_sliders(saturation: f32, vibrance: f32) -> Self {
        let h0 = SKIN_HUE_DEG.to_radians();
        let [lw, aw, bw] = linear_bt2020_to_oklab([1.0; 3]);
        Self {
            saturation: saturation / 100.0,
            vibrance: vibrance / 100.0,
            skin_cos: h0.cos() as f32,
            skin_sin: h0.sin() as f32,
            skin_cos_width: SKIN_HUE_HALF_WIDTH_DEG.to_radians().cos() as f32,
            neutral: (aw / lw, bw / lw),
        }
    }

    /// 何もしないか。
    pub fn is_identity(&self) -> bool {
        self.saturation == 0.0 && self.vibrance == 0.0
    }

    /// 無彩色からの差 `(da, db)` に掛ける倍率（モジュールの doc の `ks · kv`）。
    #[inline]
    pub fn chroma_gain(&self, da: f32, db: f32) -> f32 {
        let ks = 1.0 + self.saturation;
        if self.vibrance == 0.0 {
            return ks;
        }
        let c = (da * da + db * db).sqrt();
        let x = 1.0 - (c / VIBRANCE_CHROMA_REF).min(1.0);
        let wc = x * x;
        let skin = if self.vibrance > 0.0 && c > 0.0 {
            let cos_d = (da * self.skin_cos + db * self.skin_sin) / c;
            let t = ((cos_d - self.skin_cos_width) / (1.0 - self.skin_cos_width)).max(0.0);
            t * t
        } else {
            0.0
        };
        let kv = 1.0 + self.vibrance * VIBRANCE_MAX * wc * (1.0 - SKIN_PROTECTION * skin);
        ks * kv
    }

    /// 1 画素に適用する。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let [l, a, b] = linear_bt2020_to_oklab(c);
        let an = l * self.neutral.0;
        let bn = l * self.neutral.1;
        let (da, db) = (a - an, b - bn);
        let k = self.chroma_gain(da, db);
        oklab_to_linear_bt2020([l, an + da * k, bn + db * k])
    }
}

/// ステージ 13: 彩度・自然な彩度（OKLab）。どちらも 0 なら飛ばす。
#[derive(Debug, Clone, Copy, Default)]
pub struct ColorStage;

impl ColorStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.color";
    /// 04 の 2.1 節のステージの番号。
    pub const NUMBER: u8 = 13;
}

impl Stage for ColorStage {
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
        ColorContract::B2Working
    }
    fn params(
        &self,
        settings: &DevelopSettings,
        ctx: &StageContext<'_>,
    ) -> Result<Option<StageParams>> {
        match ctx.process_version {
            ProcessVersion::V1 => {
                let c = settings.normalized().color;
                let p = ColorStageParams::from_sliders(c.saturation, c.vibrance);
                Ok((!p.is_identity()).then(|| StageParams::new(p)))
            }
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
        let p = *params.expect::<ColorStageParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 => map_pixels(input, output, |_, _, c| p.apply(c)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 無彩色の軸からの差。
    fn delta(c: [f32; 3]) -> (f32, f32) {
        let n = ColorStageParams::from_sliders(0.0, 0.0).neutral;
        let [l, a, b] = linear_bt2020_to_oklab(c);
        (a - l * n.0, b - l * n.1)
    }

    fn chroma(c: [f32; 3]) -> f32 {
        let (a, b) = delta(c);
        (a * a + b * b).sqrt()
    }

    #[test]
    fn neutral_axis_is_close_to_zero_and_grays_are_unchanged() {
        let p = ColorStageParams::from_sliders(100.0, 100.0);
        assert!(p.neutral.0.abs() < 1e-3 && p.neutral.1.abs() < 1e-3);
        for g in [0.0f32, 1e-4, 0.18, 1.0, 5.0] {
            let out = p.apply([g; 3]);
            for v in out {
                assert!((v - g).abs() <= 1e-5 * g.max(1e-3), "{g} → {out:?}");
            }
        }
    }

    #[test]
    fn zero_saturation_gives_neutral_colors() {
        let p = ColorStageParams::from_sliders(-100.0, 0.0);
        for c in [
            [0.5, 0.2, 0.1],
            [0.05, 0.3, 0.6],
            [2.0, 1.0, 0.1],
            [0.2, -0.05, 0.1],
        ] {
            let out = p.apply(c);
            let m = out[1].abs().max(1e-3);
            assert!(
                (out[0] - out[1]).abs() < 1e-5 * m && (out[2] - out[1]).abs() < 1e-5 * m,
                "{c:?} → {out:?}"
            );
            // 明るさ L は保たれる。
            let l0 = linear_bt2020_to_oklab(c)[0];
            let l1 = linear_bt2020_to_oklab(out)[0];
            assert!((l0 - l1).abs() < 1e-4, "{l0} {l1}");
        }
    }

    #[test]
    fn saturation_scales_chroma_and_keeps_hue() {
        let p = ColorStageParams::from_sliders(50.0, 0.0);
        let c = [0.4, 0.2, 0.1];
        let out = p.apply(c);
        assert!((chroma(out) / chroma(c) - 1.5).abs() < 1e-3);
        let (a0, b0) = delta(c);
        let (a1, b1) = delta(out);
        assert!((a0 * b1 - a1 * b0).abs() < 1e-5, "色相が変わった");
        assert!(ColorStageParams::from_sliders(0.0, 0.0).is_identity());
    }

    #[test]
    fn vibrance_prefers_low_chroma_and_protects_skin() {
        let p = ColorStageParams::from_sliders(0.0, 100.0);
        // 彩度の低い青みのグレー（肌色の色相から遠い）。
        let dull = [0.2, 0.22, 0.26];
        // 鮮やかな色。
        let vivid = [0.8, 0.05, 0.05];
        let g_dull = chroma(p.apply(dull)) / chroma(dull);
        let g_vivid = chroma(p.apply(vivid)) / chroma(vivid);
        assert!(g_dull > 1.5, "{g_dull}");
        assert!((g_vivid - 1.0).abs() < 1e-3, "{g_vivid}");
        // 肌色（ColorChecker の light skin を BT.2020 にした程度の値）は、同じくらいの彩度の
        // 肌色でない色より効きが弱い。
        let skin = [0.40, 0.29, 0.23];
        let (a, b) = delta(skin);
        let c = (a * a + b * b).sqrt();
        let g_skin = p.chroma_gain(a, b);
        let g_other = p.chroma_gain(-c, 0.0);
        assert!(g_skin < g_other, "{g_skin} {g_other}");
        assert!(g_skin > 1.0);
        // 下げるときは保護しない（肌色も同じ割合で下がる）。
        let q = ColorStageParams::from_sliders(0.0, -100.0);
        assert!((q.chroma_gain(a, b) - q.chroma_gain(-c, 0.0)).abs() < 1e-6);
    }

    #[test]
    fn negative_values_never_produce_nan() {
        let vals = [-2.0f32, -0.5, -1e-6, 0.0, 1e-6, 0.3, 1.0, 5.0];
        for p in [
            ColorStageParams::from_sliders(100.0, 100.0),
            ColorStageParams::from_sliders(-100.0, -100.0),
            ColorStageParams::from_sliders(30.0, -60.0),
        ] {
            for &r in &vals {
                for &g in &vals {
                    for &b in &vals {
                        let out = p.apply([r, g, b]);
                        assert!(out.iter().all(|v| v.is_finite()), "{r} {g} {b} → {out:?}");
                    }
                }
            }
        }
    }
}
