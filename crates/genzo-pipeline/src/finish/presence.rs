//! ステージ 12: テクスチャ・明瞭度・かすみの除去（DEV-11。処理バージョン 2。docs/04_architecture.md の 2.1 節・
//! 2.7 節）。
//!
//! 輝度の対数の領域（[`super::frequency`]。`p = log2(max(Y, 0) + 2^−8)`）で、明暗を大きさごとに分けて強める・
//! 弱める（周波数分解）。かすみの除去は、段階 B のかすみの推定（[`crate::haze`]）で、リニアの RGB をかすみの
//! 式で戻す。**仮実装**（式・定数は合成の画像だけで決めた。PoC-4 で実写と Lightroom Classic を見て決める）。
//!
//! # 計算（画素ごと。f32、この順序）
//!
//! ```text
//! p   = log2(max(Y, 0) + 2^−8)                                   （Y は BT.2020 の輝度）
//! ps  = p をガウスでぼかしたもの（σ = TEXTURE_SIGMA_FRACTION × G の長辺。画素に直して縮小率で換算する）
//! --- テクスチャ（中くらいの細部） ---
//! Δt  = kt · soft(p − ps, TEXTURE_LIMIT)                          （kt = テクスチャ / 100 × TEXTURE_GAIN）
//! --- 明瞭度（大きめの範囲の明暗） ---
//! g   = ガイド（ステージ 11 と同じ。露光量の前のぼかした log2 の輝度。[`crate::guide`]）を双線形で引いた値
//! B   = contrast(g + EV)、 T = min(B + tone(B, B), 64)            （ステージ 10・11 の後の、平らな部分の明るさ）
//! bp  = log2(2^T + 2^−8)                                          （p と同じ領域の局所の明るさ）
//! w   = 1 / (1 + ((T − log2 0.18) / CLARITY_MID_WIDTH)²)          （中間調ほど強く効く）
//! Δc  = kc · w · soft(ps − bp, CLARITY_LIMIT)                     （kc = 明瞭度 / 100 × CLARITY_GAIN）
//! RGB1 = Δ = Δt + Δc を当てた画素（[`super::frequency::apply_luma_log_delta`]。Δ = 0 なら元のまま）
//! --- かすみの除去（リニアの RGB） ---
//! d   = かすみの推定の暗いチャンネルを、ガイドと同じ位置で双線形で引いた値（0〜1）
//! A'  = A × 2^(T(log2 Y_A) − log2 Y_A)                            （大気の光をステージ 10・11 の明るさに合わせる）
//! 正: t = max(1 − ω·k·d, T_MIN)、 J = (RGB1 − A'·(1 − t)) / t、 J の各チャンネルは min(RGB1, 0) 以上   （ω = 0.95）
//! 負: h = |k|·ADD_MAX·(ADD_FLOOR + (1 − ADD_FLOOR)·d)、 J = RGB1 + h·(A' − RGB1)                         （k = かすみの除去 / 100）
//! soft(x, L) = x / (1 + |x| / L)
//! ```
//!
//! - テクスチャ・明瞭度の負の値は、細部をぼかした値に寄せる（−100 で、差が小さい部分はほぼぼかした値になる。
//!   大きな差（輪郭）は `soft` で残る）。
//! - 明瞭度の局所の明るさは、ステージ 11 と同じガイド（G の長辺の 3 % の範囲の、輪郭を保つぼかし）なので、
//!   **縮小したプレビューとフル解像度で同じ局所の明るさ** を使う（IQ-07b。2.7 節）。テクスチャの σ は G の長辺に
//!   対する割合で、プレビューでは縮小率で画素に直す（細かい模様が見えない縮小では、効きも小さい）。
//! - かすみの除去は、段階 B の大気の光と暗いチャンネルを使う（切り抜きを変えても同じ。プレビューと書き出しで同じ）。
//!   新しい負の値（色域外）は作らない（もとから負のチャンネルは保つ）。
//! - タイルの余白: ガウスの半径（`ceil(3σ)` 画素。[`super::frequency::GaussKernel`]）。かすみの除去だけなら画素ごと。
//! - 処理バージョン 1 では何もしない（値があれば [`super::unimplemented_settings`] で知らせる）。

use genzo_model::{DevelopSettings, Phase};

use super::exposure::ContrastParams;
use super::frequency::{
    GaussKernel, LUMA_LOG_OFFSET, apply_luma_log_delta, blur_plane, luma_log, luma_log_plane,
    soft_limit,
};
use super::geometry::GeometryPlan;
use super::tone::ToneStageParams;
use super::{LOG2_CEILING, LOG2_MIDDLE_GRAY, luminance, map_pixels};
use crate::contract::ColorContract;
use crate::error::{PipelineError, Result};
use crate::guide::{GuideLookup, checked_guide};
use crate::image::{ImageTile, Roi};
use crate::interp::sample_bilinear_gray;
use crate::sanitize::safe_log2;
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// テクスチャの分解のぼかしの σ（G の長辺に対する割合）。仮置き: 長辺 6000px で約 4.8px、長辺 2560px の
/// プレビューで約 2px。
pub const TEXTURE_SIGMA_FRACTION: f64 = 0.0008;
/// テクスチャの強さ（スライダー ±100 のときの細部の倍率の変化）。仮置き。
pub const TEXTURE_GAIN: f32 = 1.0;
/// テクスチャの細部の差を抑える目安（段）。仮置き。
pub const TEXTURE_LIMIT: f32 = 1.0;
/// 明瞭度の強さ。仮置き。
pub const CLARITY_GAIN: f32 = 1.0;
/// 明瞭度の明暗の差を抑える目安（段）。仮置き。
pub const CLARITY_LIMIT: f32 = 1.5;
/// 明瞭度の中間調の重みの幅（段）。仮置き: 中間グレーから 3 段離れると半分。
pub const CLARITY_MID_WIDTH: f32 = 3.0;
/// かすみの除去の ω（He ら 2009 の 0.95。かすみを少し残す）。
pub const DEHAZE_OMEGA: f32 = 0.95;
/// かすみの除去の透過率の下限（He ら 2009 の 0.1）。
pub const DEHAZE_T_MIN: f32 = 0.1;
/// かすみを足す（負の値）ときの、大気の光に寄せる割合の最大。仮置き。
pub const DEHAZE_ADD_MAX: f32 = 0.5;
/// かすみを足すとき、かすみの薄い部分（暗いチャンネルが 0）にも足す割合。仮置き。
pub const DEHAZE_ADD_FLOOR: f32 = 0.3;

/// ステージ 12 のパラメータ（GPU 版にも同じ値を渡す）。
#[derive(Debug, Clone, PartialEq)]
pub struct PresenceStageParams {
    /// テクスチャの強さ（kt。スライダー / 100 × [`TEXTURE_GAIN`]）。
    pub texture: f32,
    /// 明瞭度の強さ（kc。スライダー / 100 × [`CLARITY_GAIN`]）。
    pub clarity: f32,
    /// かすみの除去（k。スライダー / 100）。
    pub dehaze: f32,
    /// テクスチャ・明瞭度の分解のぼかし（どちらも 0 なら `None`）。
    pub kernel: Option<GaussKernel>,
    /// 局所の明るさを求めるためのステージ 10・11 の項目と、C の画素 → ガイドの座標（明瞭度・かすみの除去が
    /// 0 なら使わない）。
    pub tone: ToneStageParams,
    /// 明瞭度かかすみの除去でガイドを使うか。
    pub uses_guide: bool,
    /// ステージ 12 の入力の明るさに合わせた大気の光 A'（かすみの除去が 0 なら 0）。
    pub airlight: [f32; 3],
}

impl PresenceStageParams {
    /// ガイドの値 `g`（露光量の前）から、ステージ 10・11 の後の平らな部分の明るさ（log2）を求める
    /// （モジュールの doc の `T`）。
    #[inline]
    pub fn local_base(&self, g: f32) -> f32 {
        let b = self.tone.contrast.apply_log2(g + self.tone.ev);
        (b + self.tone.log2_gain(b, b)).min(LOG2_CEILING)
    }

    /// テクスチャ・明瞭度の輝度の対数の変化（段）。`p` は画素の [`luma_log`]、`ps` はぼかした値、`g` はガイドの値。
    #[inline]
    pub fn delta(&self, p: f32, ps: f32, g: f32) -> f32 {
        let mut delta = 0.0f32;
        if self.texture != 0.0 {
            delta += self.texture * soft_limit(p - ps, TEXTURE_LIMIT);
        }
        if self.clarity != 0.0 {
            let t = self.local_base(g);
            let bp = (t.exp2() + LUMA_LOG_OFFSET).log2();
            let m = (t - LOG2_MIDDLE_GRAY) / CLARITY_MID_WIDTH;
            let w = 1.0 / (1.0 + m * m);
            delta += self.clarity * w * soft_limit(ps - bp, CLARITY_LIMIT);
        }
        delta
    }

    /// かすみの除去（`d` は暗いチャンネルの値。モジュールの doc）。
    #[inline]
    pub fn dehaze_pixel(&self, c: [f32; 3], d: f32) -> [f32; 3] {
        let a = self.airlight;
        if self.dehaze > 0.0 {
            let t = (1.0 - DEHAZE_OMEGA * self.dehaze * d).max(DEHAZE_T_MIN);
            let keep = 1.0 - t;
            std::array::from_fn(|k| {
                let j = (c[k] - a[k] * keep) / t;
                j.max(c[k].min(0.0))
            })
        } else {
            let h =
                -self.dehaze * DEHAZE_ADD_MAX * (DEHAZE_ADD_FLOOR + (1.0 - DEHAZE_ADD_FLOOR) * d);
            std::array::from_fn(|k| c[k] + h * (a[k] - c[k]))
        }
    }

    /// 1 画素に適用する（`ps` はぼかした輝度の対数、`g` はガイドの値、`d` は暗いチャンネル。使わない値は何でもよい）。
    #[inline]
    pub fn apply(&self, c: [f32; 3], ps: f32, g: f32, d: f32) -> [f32; 3] {
        let mut out = c;
        if self.kernel.is_some() {
            let delta = self.delta(luma_log(c), ps, g);
            if delta != 0.0 {
                out = apply_luma_log_delta(c, delta);
            }
        }
        if self.dehaze != 0.0 {
            out = self.dehaze_pixel(out, d);
        }
        out
    }
}

/// ステージ 12: テクスチャ・明瞭度・かすみの除去（処理バージョン 2）。3 つとも 0 なら飛ばす。
///
/// 明瞭度・かすみの除去が 0 でなければ、文脈にガイド（かすみの除去ではかすみの推定付き）が必要（なければ
/// [`PipelineError::GuideMismatch`]。エンジンが作って求め直す）。
#[derive(Debug, Clone, Copy, Default)]
pub struct TextureStage;

impl TextureStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.texture";
    /// 04 の 2.1 節のステージの番号。
    pub const NUMBER: u8 = 12;
}

impl Stage for TextureStage {
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
            ProcessVersion::V1 => return Ok(None),
            ProcessVersion::V2 => {}
        }
        let s = settings.normalized();
        let p = s.presence;
        if p.is_neutral() {
            return Ok(None);
        }
        let uses_guide = p.clarity != 0.0 || p.dehaze != 0.0;
        let mut tone = ToneStageParams {
            highlights: s.tone.highlights / 100.0,
            shadows: s.tone.shadows / 100.0,
            whites: s.tone.whites / 100.0,
            blacks: s.tone.blacks / 100.0,
            ev: s.exposure_ev,
            contrast: ContrastParams::from_slider(s.contrast),
            lookup: GuideLookup {
                origin: (0, 0),
                scale: (1.0, 1.0),
            },
        };
        let mut params = PresenceStageParams {
            texture: p.texture / 100.0 * TEXTURE_GAIN,
            clarity: p.clarity / 100.0 * CLARITY_GAIN,
            dehaze: p.dehaze / 100.0,
            kernel: (p.texture != 0.0 || p.clarity != 0.0)
                .then(|| GaussKernel::new(ctx.length_px(TEXTURE_SIGMA_FRACTION))),
            tone,
            uses_guide,
            airlight: [0.0; 3],
        };
        if uses_guide {
            let guide = checked_guide(ctx, Self::ID)?;
            // ステージ 9 と同じ C の格子（切り抜く前の全体の表示を含む）でガイドを引く。
            let plan = GeometryPlan::from_settings(&s, ctx)?;
            tone.lookup = GuideLookup::new(&plan, guide.image.width(), guide.image.height());
            params.tone = tone;
            if p.dehaze != 0.0 {
                let haze = guide.haze.as_ref().ok_or(PipelineError::GuideMismatch {
                    stage: Self::ID,
                    reason: "ガイドにかすみの推定がない（かすみの除去のある設定で段階 B を作り直す）",
                })?;
                let a = haze.airlight;
                let la = safe_log2(luminance(a));
                let gain = (params.local_base(la) - la).exp2();
                params.airlight = a.map(|v| v * gain);
            }
        }
        Ok(Some(StageParams::new(params)))
    }
    fn input_roi(&self, output_roi: Roi, params: &StageParams, _: &StageContext<'_>) -> Roi {
        match params
            .get::<PresenceStageParams>()
            .and_then(|p| p.kernel.as_ref())
        {
            Some(k) => output_roi.expand(k.radius),
            None => output_roi,
        }
    }
    fn run_cpu(
        &self,
        ctx: &StageContext<'_>,
        input: &ImageTile,
        output: &mut ImageTile,
        params: &StageParams,
    ) -> Result<()> {
        let p = params.expect::<PresenceStageParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 | ProcessVersion::V2 => {}
        }
        let out = output.roi;
        let blurred = match &p.kernel {
            Some(k) => {
                let need = out.expand(k.radius);
                let plane = luma_log_plane(input, need)?;
                Some(blur_plane(&plane, need, out, k)?)
            }
            None => None,
        };
        let guide = if p.uses_guide {
            Some(checked_guide(ctx, Self::ID)?)
        } else {
            None
        };
        let dark = match guide {
            Some(g) if p.dehaze != 0.0 => Some(
                &g.haze
                    .as_ref()
                    .ok_or(PipelineError::GuideMismatch {
                        stage: Self::ID,
                        reason: "ガイドにかすみの推定がない",
                    })?
                    .dark,
            ),
            _ => None,
        };
        let w = out.width as usize;
        map_pixels(input, output, |x, y, c| {
            let ps = match &blurred {
                Some(b) => b[(y - i64::from(out.y)) as usize * w + (x - i64::from(out.x)) as usize],
                None => 0.0,
            };
            let (g, d) = match guide {
                Some(gd) => {
                    let (u, v) = p.tone.lookup.coords(x, y);
                    (
                        sample_bilinear_gray(&gd.image, u, v),
                        dark.map_or(0.0, |dk| sample_bilinear_gray(dk, u, v)),
                    )
                }
                None => (0.0, 0.0),
            };
            p.apply(c, ps, g, d)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(texture: f32, clarity: f32, dehaze: f32) -> PresenceStageParams {
        PresenceStageParams {
            texture,
            clarity,
            dehaze,
            kernel: (texture != 0.0 || clarity != 0.0).then(|| GaussKernel::new(1.0)),
            tone: ToneStageParams {
                highlights: 0.0,
                shadows: 0.0,
                whites: 0.0,
                blacks: 0.0,
                ev: 0.0,
                contrast: ContrastParams::from_slider(0.0),
                lookup: GuideLookup {
                    origin: (0, 0),
                    scale: (1.0, 1.0),
                },
            },
            uses_guide: clarity != 0.0 || dehaze != 0.0,
            airlight: [0.8, 0.82, 0.85],
        }
    }

    #[test]
    fn texture_scales_fine_detail_and_keeps_flat_regions() {
        let p = params(1.0, 0.0, 0.0);
        let c = [0.3f32; 3];
        let pc = luma_log(c);
        // 平らな部分（ぼかした値と同じ）は変わらない。
        assert_eq!(p.apply(c, pc, 0.0, 0.0), c);
        // 周りより明るい細部はさらに明るく、負の値では周りに寄る。
        let ps = pc - 0.2;
        let up = p.apply(c, ps, 0.0, 0.0);
        assert!(luma_log(up) - pc > 0.15, "{up:?}");
        let down = params(-1.0, 0.0, 0.0).apply(c, ps, 0.0, 0.0);
        assert!((luma_log(down) - ps).abs() < 0.04, "{down:?}");
        // 色相（RGB の比）は保つ。
        let col = [0.4f32, 0.2, 0.1];
        let out = p.apply(col, luma_log(col) - 0.3, 0.0, 0.0);
        assert!((out[0] / out[1] - 2.0).abs() < 1e-2 && (out[1] / out[2] - 2.0).abs() < 2e-2);
    }

    #[test]
    fn clarity_uses_the_local_base_from_the_guide() {
        let p = params(0.0, 1.0, 0.0);
        // ガイド（局所の明るさ）と同じ明るさの平らな部分は変わらない。
        let c = [0.18f32; 3];
        let g = 0.18f32.log2();
        let pc = luma_log(c);
        let out = p.apply(c, pc, g, 0.0);
        assert!((luma_log(out) - pc).abs() < 0.01, "{out:?}");
        // 局所の明るさより明るい部分は明るく、暗い部分は暗くなる（中間調の局所のコントラスト）。
        let bright = p.apply([0.36; 3], luma_log([0.36; 3]), g, 0.0);
        assert!(bright[0] > 0.36 * 1.4, "{bright:?}");
        let dark = p.apply([0.09; 3], luma_log([0.09; 3]), g, 0.0);
        assert!(dark[0] < 0.09 * 0.7, "{dark:?}");
        // 負の値では局所の明るさに寄る。
        let soft = params(0.0, -1.0, 0.0).apply([0.36; 3], luma_log([0.36; 3]), g, 0.0);
        assert!(soft[0] < 0.36 && soft[0] > 0.18, "{soft:?}");
        // 中間調から遠い（とても明るい）局所の明るさでは効きが弱い。
        let w_mid = p.delta(luma_log([0.36; 3]), luma_log([0.36; 3]), g);
        let hi = 8.0f32;
        let w_hi = p.delta(luma_log([hi * 2.0; 3]), luma_log([hi * 2.0; 3]), hi.log2());
        assert!(w_hi < w_mid * 0.6, "{w_hi} {w_mid}");
        // 露光量を足したガイドは、露光量の後の画素と同じ局所の明るさ（ガイドを作り直さない）。
        let mut e = params(0.0, 1.0, 0.0);
        e.tone.ev = 1.0;
        assert!((e.local_base(g) - (g + 1.0)).abs() < 1e-5);
    }

    #[test]
    fn dehaze_removes_and_adds_haze() {
        let p = params(0.0, 0.0, 1.0);
        let a = p.airlight;
        // かすみの濃い部分（d = 0.9）の、かすみの式 I = J·t + A·(1 − t) で作った画素は J に近づく。
        let j = [0.05f32, 0.2, 0.1];
        let t = 1.0 - DEHAZE_OMEGA * 0.9;
        let hazy: [f32; 3] = std::array::from_fn(|k| j[k] * t + a[k] * (1.0 - t));
        let out = p.apply(hazy, 0.0, 0.0, 0.9);
        for k in 0..3 {
            assert!((out[k] - j[k]).abs() < 1e-5, "{out:?}");
        }
        // かすみのない部分（d = 0）は変わらない。
        assert_eq!(p.apply(j, 0.0, 0.0, 0.0), j);
        // 新しい負の値は作らない（もとからの負の値は保つ）。
        let dark = p.apply([0.01, 0.5, -0.1], 0.0, 0.0, 0.9);
        assert!(dark[0] >= 0.0 && dark[2] == -0.1, "{dark:?}");
        // 負の値はかすみ（大気の光）に寄せる。濃い部分ほど強い。
        let q = params(0.0, 0.0, -1.0);
        let near = q.apply(j, 0.0, 0.0, 0.0);
        let far = q.apply(j, 0.0, 0.0, 1.0);
        assert!(near[0] > j[0] && far[0] > near[0], "{near:?} {far:?}");
        assert!((far[1] - (j[1] + DEHAZE_ADD_MAX * (a[1] - j[1]))).abs() < 1e-6);
    }

    #[test]
    fn version_1_and_neutral_settings_skip_the_stage() {
        let mut s = DevelopSettings::default();
        s.presence.texture = 50.0;
        let ctx = crate::stage::tests::ctx();
        assert_eq!(ctx.process_version, ProcessVersion::V1);
        assert!(TextureStage.params(&s, &ctx).unwrap().is_none());
        let v2 = StageContext {
            process_version: ProcessVersion::V2,
            ..ctx
        };
        let p = TextureStage.params(&s, &v2).unwrap().unwrap();
        let pp = p.get::<PresenceStageParams>().unwrap();
        assert!(pp.kernel.is_some() && !pp.uses_guide);
        // テクスチャの σ は G の長辺に対する割合（100px の文脈では 0.08px → 半径 1）。
        assert_eq!(pp.kernel.as_ref().unwrap().radius, 1);
        assert_eq!(
            TextureStage.input_roi(Roi::new(0, 0, 4, 4), &p, &v2),
            Roi::new(-1, -1, 6, 6)
        );
        // 明瞭度はガイドが必要。
        s.presence.clarity = 20.0;
        assert!(matches!(
            TextureStage.params(&s, &v2),
            Err(PipelineError::GuideMismatch { .. })
        ));
        assert!(
            TextureStage
                .params(&DevelopSettings::default(), &v2)
                .unwrap()
                .is_none()
        );
    }
}
