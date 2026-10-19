//! ステージ 16 の後半: 効果（切り抜き後の周辺光量補正・粒子。DEV-24。処理バージョン 2。docs/04_architecture.md の
//! 2.1 節）。トーンカーブ（[`super::tone_curve`]）の後の B3（ディスプレイ参照のリニア）にかける。
//! **仮実装**（式・定数は合成の画像だけで決めた。PoC-4 で実写と Lightroom Classic を見て決める）。
//!
//! # 切り抜き後の周辺光量補正（画素ごと。f32、この順序）
//!
//! 設定の切り抜き（切り抜く前の全体の表示では、設定の切り抜きの枠）の中心 (cx, cy) と半分の幅・高さ（a, b。
//! C の格子の画素）から、丸み r（−1〜1）で楕円の半軸と指数を決める（f64 で求めて f32 に丸める）。手動の中心
//! （`effects.vignette.center`。G の外接矩形の各軸の割合）があれば、(cx, cy) だけをその位置（C の格子の座標に
//! 直したもの）にする（a, b・d_corner は切り抜きから求めたまま。中心を動かすと、遠い側の隅では ρ > 1 になる）:
//!
//! ```text
//! r ≥ 0: (a', b') = (a^(1−r)·m^r, b^(1−r)·m^r)（m = √(a·b)。+1 で円）、 指数 q = 2
//! r < 0: (a', b') = (a, b)、 q = 2 + |r|·(EXPONENT_MAX − 2)              （角の丸い四角に近づく）
//! u = |x + 0.5 − cx| / a'、 v = |y + 0.5 − cy| / b'
//! ρ = exp2(log2(max(exp2(q·log2(max(u, 1e-30))) + exp2(q·log2(max(v, 1e-30))), 1e-30)) / q) / d_corner
//!                                                                    （d_corner は切り抜きの四隅での値。四隅で ρ = 1）
//! w = smoothstep(c − h, c + h, ρ)    （c = MID_MIN + (MID_MAX − MID_MIN)·中点 / 100、
//!                                     h = FEATHER_MIN + (FEATHER_MAX − FEATHER_MIN)·ぼかし / 100）
//! 量 < 0: RGB' = RGB × (1 − |量|·DARK_MAX·w·(1 − ハイライト·smoothstep(HL_LO, HL_HI, Y)))
//! 量 > 0: RGB' = RGB + 量·LIGHT_MAX·w·(1 − RGB)                       （白に寄せる）
//! ```
//!
//! # 粒子
//!
//! 写真ごとの種（[`crate::StageContext::photo_seed`]）と G の座標（切り抜く前の全体。G の画素の単位）だけで決まる
//! 連続の模様 `N(gx, gy)` を、**各画素が覆う G の範囲で平均した値** にする（縮小のプレビューの画素は、フル解像度の
//! 画素の値をその範囲で平均したものと同じ値になる。IQ-07b。切り抜きを変えても模様は動かない）。
//!
//! ```text
//! 格子の値 v(i, j, o) = output::dither_noise(i, j, SALT + o, 種)（三角分布。−1〜1）
//! N_o(x, y)  = Σ_ij v(i, j, o)·Λ(x / s_o − i)·Λ(y / s_o − j)         （双線形の補間。Λ は三角形の関数）
//! 画素の値   = Σ_j Σ_i v(i, j, o)·Wx_i·Wy_j                            （W は画素の範囲での Λ の平均。j の外、i の内の順に足す）
//! N          = (N_0 + ρ_r·N_1) / √(1 + ρ_r²)                           （s_0 = 大きさの s、s_1 = s × COARSE_RATIO、ρ_r = 粗さ × ROUGH_WEIGHT）
//! e  = srgb_encode(clamp(Y, 0, 1))、 e' = clamp(e + 量·STRENGTH·N·4e(1 − e), 0, 1)、 Y' = srgb_decode(e')
//! RGB' = RGB × Y' / Y（Y > 2^−16）、 それ以外は RGB + (Y' − Y)
//! ```
//!
//! - s = G の長辺 × `SIZE_MIN · (SIZE_MAX / SIZE_MIN)^(大きさ / 100)`（G の画素）。
//! - 画素の範囲が格子の [`GRAIN_MAX_SPAN`] 個より広い（強く縮小したサムネイルなど）ときは、中心の
//!   `GRAIN_MAX_SPAN` 個の幅で平均し、`√(GRAIN_MAX_SPAN / 幅)` 倍する（その軸ごと。平均の揺らぎの大きさが
//!   幅に反比例することを使った近似。模様は正確には同じでない）。
//! - 黒（e = 0）・白（e = 1）の画素には粒子を足さない。
//!
//! 処理バージョン 1 では何もしない（値があれば [`super::unimplemented_settings`] で知らせる）。

use genzo_color::transfer::{srgb_decode_f32, srgb_encode_f32};
use genzo_model::{DevelopSettings, Phase};

use super::frequency::smoothstep;
use super::geometry::GeometryPlan;
use super::output::dither_noise;
use super::{luminance, map_pixels};
use crate::contract::ColorContract;
use crate::error::Result;
use crate::image::{ImageTile, Roi};
use crate::sanitize::LOG_FLOOR;
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// 中点 0 のときの境目の中心（ρ）。仮置き。
pub const VIGNETTE_MID_MIN: f32 = 0.3;
/// 中点 100 のときの境目の中心（ρ。四隅）。仮置き。
pub const VIGNETTE_MID_MAX: f32 = 1.0;
/// ぼかし 0 のときの境目の半分の幅（ρ）。仮置き。
pub const VIGNETTE_FEATHER_MIN: f32 = 0.005;
/// ぼかし 100 のときの境目の半分の幅（ρ）。仮置き。
pub const VIGNETTE_FEATHER_MAX: f32 = 0.5;
/// 丸み −100 のときの指数（大きいほど四角に近い）。仮置き。
pub const VIGNETTE_EXPONENT_MAX: f64 = 6.0;
/// 量 −100 のときに暗くする割合の最大（四隅で約 −3.3 段）。仮置き。
pub const VIGNETTE_DARK_MAX: f32 = 0.9;
/// 量 +100 のときに白に寄せる割合の最大。仮置き。
pub const VIGNETTE_LIGHT_MAX: f32 = 0.9;
/// ハイライトの保護が効き始める輝度（B3 のリニア）。仮置き。
pub const VIGNETTE_HL_LO: f32 = 0.25;
/// ハイライトの保護が最大になる輝度。仮置き。
pub const VIGNETTE_HL_HI: f32 = 1.0;
/// 粒子の大きさ 0 のときの格子の間隔（G の長辺に対する割合）。仮置き: 長辺 6000px で 0.9px。
pub const GRAIN_SIZE_MIN: f64 = 1.5e-4;
/// 粒子の大きさ 100 のときの格子の間隔（G の長辺に対する割合）。仮置き: 長辺 6000px で 9px。
pub const GRAIN_SIZE_MAX: f64 = 1.5e-3;
/// 粗い模様の格子の間隔の比。仮置き（細かい模様と格子がそろわない値）。
pub const GRAIN_COARSE_RATIO: f32 = 2.37;
/// 粗さ 100 のときの粗い模様の重み。仮置き。
pub const GRAIN_ROUGH_WEIGHT: f32 = 0.8;
/// 量 100 のときの、符号化した輝度（sRGB の伝達関数）の変化の尺度。仮置き。
pub const GRAIN_STRENGTH: f32 = 0.35;
/// 画素の範囲を正確に平均する格子の幅の上限（それより広い画素は近似。モジュールの doc）。
pub const GRAIN_MAX_SPAN: f32 = 6.0;
/// 粒子の格子の値の種に足す値（ディザリングと同じ関数を別の値で使う）。
pub const GRAIN_SALT: u32 = 0x6772_6e00;

/// 切り抜き後の周辺光量補正のパラメータ（GPU 版にも同じ値を渡す）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VignetteParams {
    /// 中心（切り抜きの中心か手動の中心。C の格子の連続座標）。
    pub center: [f32; 2],
    /// 1 / 半軸（a'・b'。C の格子の画素）。
    pub inv_axes: [f32; 2],
    /// 指数 q。
    pub exponent: f32,
    /// 1 / d_corner。
    pub inv_corner: f32,
    /// 境目（smoothstep の下端と上端。ρ）。
    pub edge: [f32; 2],
    /// 量（−1〜1）。
    pub amount: f32,
    /// ハイライトの保護（0〜1）。
    pub highlights: f32,
}

impl VignetteParams {
    /// C の格子の画素 (x, y) の重み w（0〜1）。
    #[inline]
    pub fn weight(&self, x: i64, y: i64) -> f32 {
        let u = ((x as f32 + 0.5) - self.center[0]).abs() * self.inv_axes[0];
        let v = ((y as f32 + 0.5) - self.center[1]).abs() * self.inv_axes[1];
        let q = self.exponent;
        let s = (q * u.max(1e-30).log2()).exp2() + (q * v.max(1e-30).log2()).exp2();
        let d = (s.max(1e-30).log2() / q).exp2();
        smoothstep(self.edge[0], self.edge[1], d * self.inv_corner)
    }

    /// 1 画素に適用する。
    #[inline]
    pub fn apply(&self, c: [f32; 3], w: f32) -> [f32; 3] {
        if self.amount < 0.0 {
            let protect =
                self.highlights * smoothstep(VIGNETTE_HL_LO, VIGNETTE_HL_HI, luminance(c));
            let k = 1.0 - (-self.amount) * VIGNETTE_DARK_MAX * w * (1.0 - protect);
            [c[0] * k, c[1] * k, c[2] * k]
        } else {
            let k = self.amount * VIGNETTE_LIGHT_MAX * w;
            [
                c[0] + k * (1.0 - c[0]),
                c[1] + k * (1.0 - c[1]),
                c[2] + k * (1.0 - c[2]),
            ]
        }
    }
}

/// 粒子のパラメータ（GPU 版にも同じ値を渡す）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrainParams {
    /// 写真ごとの種。
    pub seed: u32,
    /// C の格子の原点（G の格子での切り抜きの左上。[`GeometryPlan::crop_lattice`]）。
    pub origin: [i32; 2],
    /// 1 / 密度（格子の 1 画素あたりの G の画素。横, 縦）。
    pub inv_density: [f32; 2],
    /// 1 / 格子の間隔（G の画素。細かい模様, 粗い模様）。
    pub inv_cell: [f32; 2],
    /// 模様の重み（正規化済み。細かい模様, 粗い模様）。
    pub weights: [f32; 2],
    /// 強さ（量 / 100 × [`GRAIN_STRENGTH`]）。
    pub strength: f32,
}

/// 三角形の関数 Λ の累積（`t ≤ −1` で 0、`t ≥ 1` で 1）。
#[inline]
fn tent_cdf(t: f32) -> f32 {
    if t <= -1.0 {
        0.0
    } else if t <= 0.0 {
        let a = t + 1.0;
        0.5 * a * a
    } else if t < 1.0 {
        let a = 1.0 - t;
        1.0 - 0.5 * a * a
    } else {
        1.0
    }
}

/// 1 つの軸の範囲 `[a, b)`（格子の単位）の、正確に平均する範囲と倍率（モジュールの doc）。
#[inline]
fn grain_span(a: f32, b: f32) -> (f32, f32, f32) {
    let span = b - a;
    if span > GRAIN_MAX_SPAN {
        let c = 0.5 * (a + b);
        let h = 0.5 * GRAIN_MAX_SPAN;
        (c - h, c + h, (GRAIN_MAX_SPAN / span).sqrt())
    } else {
        (a, b, 1.0)
    }
}

impl GrainParams {
    /// 1 つの模様の、範囲 `[x0, x1) × [y0, y1)`（G の画素）での平均。
    pub fn octave(&self, o: usize, x0: f32, x1: f32, y0: f32, y1: f32) -> f32 {
        let k = self.inv_cell[o];
        let (ax, bx, sx) = grain_span(x0 * k, x1 * k);
        let (ay, by, sy) = grain_span(y0 * k, y1 * k);
        let (wx, wy) = (bx - ax, by - ay);
        let (i0, i1) = (ax.floor() as i32, bx.ceil() as i32);
        let (j0, j1) = (ay.floor() as i32, by.ceil() as i32);
        let salt = GRAIN_SALT.wrapping_add(o as u32);
        let mut acc = 0.0f32;
        for j in j0..=j1 {
            let fj = j as f32;
            let wyj = (tent_cdf(by - fj) - tent_cdf(ay - fj)) / wy;
            for i in i0..=i1 {
                let fi = i as f32;
                let wxi = (tent_cdf(bx - fi) - tent_cdf(ax - fi)) / wx;
                acc += dither_noise(i64::from(i), i64::from(j), salt, self.seed) * (wxi * wyj);
            }
        }
        acc * (sx * sy)
    }

    /// C の格子の画素 (x, y) の模様の値 N。
    #[inline]
    pub fn value(&self, x: i64, y: i64) -> f32 {
        let gx = self.origin[0] as i64 + x;
        let gy = self.origin[1] as i64 + y;
        let x0 = gx as f32 * self.inv_density[0];
        let x1 = (gx + 1) as f32 * self.inv_density[0];
        let y0 = gy as f32 * self.inv_density[1];
        let y1 = (gy + 1) as f32 * self.inv_density[1];
        let mut n = self.weights[0] * self.octave(0, x0, x1, y0, y1);
        if self.weights[1] != 0.0 {
            n += self.weights[1] * self.octave(1, x0, x1, y0, y1);
        }
        n
    }

    /// 1 画素に適用する（`n` は [`value`](Self::value)）。
    #[inline]
    pub fn apply(&self, c: [f32; 3], n: f32) -> [f32; 3] {
        let y = luminance(c);
        let e = srgb_encode_f32(y.clamp(0.0, 1.0));
        let e2 = (e + self.strength * n * (4.0 * e * (1.0 - e))).clamp(0.0, 1.0);
        let y2 = srgb_decode_f32(e2);
        if y > LOG_FLOOR {
            let k = y2 / y;
            [c[0] * k, c[1] * k, c[2] * k]
        } else {
            let d = y2 - y;
            [c[0] + d, c[1] + d, c[2] + d]
        }
    }
}

/// ステージ 16 の効果のパラメータ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectsStageParams {
    /// 切り抜き後の周辺光量補正（量 0 なら `None`）。
    pub vignette: Option<VignetteParams>,
    /// 粒子（量 0 なら `None`）。
    pub grain: Option<GrainParams>,
}

impl EffectsStageParams {
    /// C の格子の画素 (x, y) に適用する（周辺光量補正、粒子の順）。
    #[inline]
    pub fn apply(&self, x: i64, y: i64, c: [f32; 3]) -> [f32; 3] {
        let mut out = c;
        if let Some(v) = &self.vignette {
            out = v.apply(out, v.weight(x, y));
        }
        if let Some(g) = &self.grain {
            out = g.apply(out, g.value(x, y));
        }
        out
    }
}

/// ステージ 16 の後半: 効果（切り抜き後の周辺光量補正・粒子。処理バージョン 2）。どちらも量 0 なら飛ばす。
#[derive(Debug, Clone, Copy, Default)]
pub struct EffectsStage;

impl EffectsStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.effects";
    /// 04 の 2.1 節のステージの番号（トーンカーブと同じ 16）。
    pub const NUMBER: u8 = 16;
}

/// 切り抜き後の周辺光量補正のパラメータを求める（モジュールの doc）。
fn vignette_params(
    v: &genzo_model::PostCropVignette,
    plan: &GeometryPlan,
) -> Option<VignetteParams> {
    if v.is_neutral() {
        return None;
    }
    let crop = if plan.is_uncropped() {
        plan.requested_crop()
    } else {
        plan.crop()
    };
    let (gw, gh) = plan.frames().g_size_px();
    let (dx, dy) = plan.density();
    let cl = plan.crop_lattice();
    let x0 = f64::from(crop.left) * gw * dx - f64::from(cl.x);
    let x1 = f64::from(crop.right) * gw * dx - f64::from(cl.x);
    let y0 = f64::from(crop.top) * gh * dy - f64::from(cl.y);
    let y1 = f64::from(crop.bottom) * gh * dy - f64::from(cl.y);
    let center = match v.center {
        Some(p) => [
            f64::from(p.x) * gw * dx - f64::from(cl.x),
            f64::from(p.y) * gh * dy - f64::from(cl.y),
        ],
        None => [(x0 + x1) * 0.5, (y0 + y1) * 0.5],
    };
    let a = ((x1 - x0) * 0.5).max(1e-6);
    let b = ((y1 - y0) * 0.5).max(1e-6);
    let r = f64::from(v.roundness) / 100.0;
    let (ar, br, q) = if r >= 0.0 {
        let m = (a * b).sqrt();
        (
            ((1.0 - r) * a.ln() + r * m.ln()).exp(),
            ((1.0 - r) * b.ln() + r * m.ln()).exp(),
            2.0,
        )
    } else {
        (a, b, 2.0 + (-r) * (VIGNETTE_EXPONENT_MAX - 2.0))
    };
    let corner = ((a / ar).powf(q) + (b / br).powf(q)).powf(1.0 / q);
    let mid = VIGNETTE_MID_MIN + (VIGNETTE_MID_MAX - VIGNETTE_MID_MIN) * v.midpoint / 100.0;
    let half =
        VIGNETTE_FEATHER_MIN + (VIGNETTE_FEATHER_MAX - VIGNETTE_FEATHER_MIN) * v.feather / 100.0;
    Some(VignetteParams {
        center: [center[0] as f32, center[1] as f32],
        inv_axes: [(1.0 / ar) as f32, (1.0 / br) as f32],
        exponent: q as f32,
        inv_corner: (1.0 / corner) as f32,
        edge: [mid - half, mid + half],
        amount: v.amount / 100.0,
        highlights: v.highlights / 100.0,
    })
}

/// 粒子のパラメータを求める（モジュールの doc）。
fn grain_params(
    g: &genzo_model::Grain,
    plan: &GeometryPlan,
    ctx: &StageContext<'_>,
) -> Option<GrainParams> {
    if g.is_neutral() {
        return None;
    }
    let long = ctx.frames.g_long_side_px();
    let fraction =
        GRAIN_SIZE_MIN * (GRAIN_SIZE_MAX / GRAIN_SIZE_MIN).powf(f64::from(g.size) / 100.0);
    let cell = (fraction * long).max(1e-3);
    let (dx, dy) = plan.density();
    let cl = plan.crop_lattice();
    let rough = g.roughness / 100.0 * GRAIN_ROUGH_WEIGHT;
    let norm = (1.0 + f64::from(rough) * f64::from(rough)).sqrt();
    Some(GrainParams {
        seed: ctx.photo_seed,
        origin: [cl.x, cl.y],
        inv_density: [(1.0 / dx) as f32, (1.0 / dy) as f32],
        inv_cell: [
            (1.0 / cell) as f32,
            (1.0 / (cell * f64::from(GRAIN_COARSE_RATIO))) as f32,
        ],
        weights: [(1.0 / norm) as f32, (f64::from(rough) / norm) as f32],
        strength: g.amount / 100.0 * GRAIN_STRENGTH,
    })
}

impl Stage for EffectsStage {
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
            ProcessVersion::V1 => return Ok(None),
            ProcessVersion::V2 => {}
        }
        let s = settings.normalized();
        if s.effects.is_neutral() {
            return Ok(None);
        }
        let plan = GeometryPlan::from_settings(&s, ctx)?;
        Ok(Some(StageParams::new(EffectsStageParams {
            vignette: vignette_params(&s.effects.vignette, &plan),
            grain: grain_params(&s.effects.grain, &plan, ctx),
        })))
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
        let p = *params.expect::<EffectsStageParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 | ProcessVersion::V2 => {}
        }
        map_pixels(input, output, |x, y, c| p.apply(x, y, c))
    }
}

#[cfg(test)]
mod tests {
    use genzo_model::{CropRect, Orientation};

    use super::*;
    use crate::coords::{CoordinateFrames, SensorGrid};

    fn ctx(w: u32, h: u32, scale: u32, seed: u32) -> StageContext<'static> {
        let frames = CoordinateFrames::new(w, h, Orientation::Normal).unwrap();
        let grid = SensorGrid::new(w / scale, h / scale, (f64::from(w), f64::from(h))).unwrap();
        StageContext::new(ProcessVersion::V2, frames)
            .with_sensor_grid(grid)
            .with_resolution_scale(grid.resolution_scale())
            .with_photo_seed(seed)
    }

    fn params(s: &DevelopSettings, c: &StageContext<'_>) -> EffectsStageParams {
        *EffectsStage
            .params(s, c)
            .unwrap()
            .unwrap()
            .get::<EffectsStageParams>()
            .unwrap()
    }

    #[test]
    fn vignette_darkens_the_corners_and_keeps_the_center() {
        let mut s = DevelopSettings::default();
        s.effects.vignette.amount = -100.0;
        let c = ctx(300, 200, 1, 0);
        let p = params(&s, &c);
        let v = p.vignette.unwrap();
        assert!(p.grain.is_none());
        let center = p.apply(150, 100, [0.5; 3]);
        assert_eq!(center, [0.5; 3]);
        let corner = p.apply(0, 0, [0.5; 3]);
        assert!(corner[0] < 0.5 * 0.2, "{corner:?}");
        // 四隅で ρ ≒ 1、楕円（丸み 0）なので辺の中点は四隅より弱い。
        assert!(v.weight(150, 0) < v.weight(0, 0));
        // 量が正なら白に寄せる。
        s.effects.vignette.amount = 100.0;
        let light = params(&s, &c).apply(0, 0, [0.2; 3]);
        assert!(light[0] > 0.8, "{light:?}");
        // ハイライトの保護: 明るい画素は暗くしにくい。
        s.effects.vignette.amount = -100.0;
        s.effects.vignette.highlights = 100.0;
        let pr = params(&s, &c);
        assert!(pr.apply(0, 0, [1.0; 3])[0] > 0.9);
        assert!(pr.apply(0, 0, [0.1; 3])[0] < 0.03);
    }

    #[test]
    fn vignette_follows_the_crop_and_roundness() {
        let mut s = DevelopSettings::default();
        s.effects.vignette.amount = -50.0;
        s.geometry.crop = CropRect {
            left: 0.5,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        };
        let c = ctx(400, 200, 1, 0);
        let v = params(&s, &c).vignette.unwrap();
        // 切り抜いた後の画像（200 × 200）の中心。
        assert!((v.center[0] - 100.0).abs() < 1e-3 && (v.center[1] - 100.0).abs() < 1e-3);
        // 丸み +100 は円（縦横の半軸が同じ）、−100 は四角に近い（指数が大きい）。
        s.geometry.crop = CropRect::FULL;
        s.effects.vignette.roundness = 100.0;
        let circle = params(&s, &c).vignette.unwrap();
        assert!((circle.inv_axes[0] - circle.inv_axes[1]).abs() < 1e-6);
        s.effects.vignette.roundness = -100.0;
        let square = params(&s, &c).vignette.unwrap();
        assert_eq!(square.exponent, VIGNETTE_EXPONENT_MAX as f32);
        // 中点を上げると、中心寄りの位置の効きが弱くなる。
        s.effects.vignette.roundness = 0.0;
        s.effects.vignette.midpoint = 0.0;
        let near = params(&s, &c).vignette.unwrap().weight(300, 150);
        s.effects.vignette.midpoint = 100.0;
        let far = params(&s, &c).vignette.unwrap().weight(300, 150);
        assert!(near > far, "{near} {far}");
    }

    #[test]
    fn manual_vignette_center_ignores_the_crop() {
        let mut s = DevelopSettings::default();
        s.effects.vignette.amount = -50.0;
        s.effects.vignette.center = Some(genzo_model::VignetteCenter { x: 0.25, y: 0.75 });
        let c = ctx(400, 200, 1, 0);
        // 切り抜きなし: G の (100, 150)。
        let full = params(&s, &c).vignette.unwrap();
        assert!((full.center[0] - 100.0).abs() < 1e-3 && (full.center[1] - 150.0).abs() < 1e-3);
        // 切り抜いても、写真の上の同じ位置（C の格子の原点からの位置）のまま。半軸は切り抜きから決める。
        s.geometry.crop = CropRect {
            left: 0.1,
            top: 0.25,
            right: 0.6,
            bottom: 1.0,
        };
        let cropped = params(&s, &c).vignette.unwrap();
        assert!(
            (cropped.center[0] - 60.0).abs() < 1e-3 && (cropped.center[1] - 100.0).abs() < 1e-3,
            "{:?}",
            cropped.center
        );
        assert!(
            (cropped.inv_axes[0] * 100.0 - 1.0).abs() < 1e-3,
            "{:?}",
            cropped.inv_axes
        );
        // 中心は効かず、中心から遠い隅（右上）は近い隅（左下）より強く効く。
        assert_eq!(cropped.weight(60, 100), 0.0);
        assert!(cropped.weight(199, 0) > cropped.weight(0, 149));
        // 中心がなければ切り抜きの中心。
        s.effects.vignette.center = None;
        let auto = params(&s, &c).vignette.unwrap();
        assert!((auto.center[0] - 100.0).abs() < 1e-3 && (auto.center[1] - 75.0).abs() < 1e-3);
    }

    #[test]
    fn grain_is_deterministic_and_matches_the_area_average() {
        let mut s = DevelopSettings::default();
        s.effects.grain.amount = 50.0;
        s.effects.grain.size = 60.0;
        let full = ctx(6000, 3999, 1, 7);
        let pf = params(&s, &full).grain.unwrap();
        // 同じ種・同じ位置は同じ値、種が違えば違う模様。
        assert_eq!(pf.value(10, 20), pf.value(10, 20));
        let other = params(&s, &ctx(6000, 3999, 1, 8)).grain.unwrap();
        let diff: f32 = (0..50)
            .map(|i| (pf.value(i, 3) - other.value(i, 3)).abs())
            .sum();
        assert!(diff > 1.0, "{diff}");
        // 縮小（1/3）の画素の値は、フル解像度の 3 × 3 の画素の値の平均と同じ（正確に平均する範囲）。
        let small = ctx(6000, 3999, 3, 7);
        let ps = params(&s, &small).grain.unwrap();
        let mut worst = 0.0f32;
        for (x, y) in [(0, 0), (5, 7), (33, 21), (100, 66)] {
            let mut avg = 0.0f32;
            for j in 0..3 {
                for i in 0..3 {
                    avg += pf.value(x * 3 + i, y * 3 + j);
                }
            }
            avg /= 9.0;
            worst = worst.max((ps.value(x, y) - avg).abs());
        }
        assert!(worst < 1e-4, "{worst}");
        // 黒と白には足さない。
        assert_eq!(pf.apply([0.0; 3], 1.0), [0.0; 3]);
        assert_eq!(pf.apply([1.0; 3], 1.0), [1.0; 3]);
        let g = pf.apply([0.2; 3], 0.5);
        assert!(g[0] > 0.2);
    }

    #[test]
    fn grain_amplitude_falls_off_for_strong_downscaling() {
        let mut s = DevelopSettings::default();
        s.effects.grain.amount = 100.0;
        s.effects.grain.size = 0.0;
        s.effects.grain.roughness = 0.0;
        let std = |c: &StageContext<'_>| {
            let p = params(&s, c).grain.unwrap();
            let v: Vec<f32> = (0..400).map(|i| p.value(i % 20, i / 20)).collect();
            let m = v.iter().sum::<f32>() / v.len() as f32;
            (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32).sqrt()
        };
        let full = std(&ctx(6000, 4000, 1, 1));
        let thumb = std(&ctx(6000, 4000, 40, 1));
        assert!(full > 0.1, "{full}");
        assert!(thumb < full * 0.2, "{thumb} {full}");
        assert!(tent_cdf(-2.0) == 0.0 && tent_cdf(0.0) == 0.5 && tent_cdf(3.0) == 1.0);
    }

    #[test]
    fn version_1_and_neutral_effects_skip_the_stage() {
        let mut s = DevelopSettings::default();
        let c = ctx(100, 50, 1, 0);
        assert!(EffectsStage.params(&s, &c).unwrap().is_none());
        s.effects.grain.amount = 10.0;
        assert!(EffectsStage.params(&s, &c).unwrap().is_some());
        let v1 = StageContext {
            process_version: ProcessVersion::V1,
            ..c
        };
        assert!(EffectsStage.params(&s, &v1).unwrap().is_none());
    }
}
