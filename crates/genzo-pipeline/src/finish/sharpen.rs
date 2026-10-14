//! ステージ 14: シャープ（DEV-12。処理バージョン 2。docs/04_architecture.md の 2.1 節）。
//!
//! 輝度の対数の領域（[`super::frequency`]）のアンシャープマスク。色の成分には同じ倍率をかけるだけで、色の縁
//! （色ずれ）を強めない。**仮実装**（式・定数は合成の画像だけで決めた。PoC-4 で実写と Lightroom Classic を
//! 見て決める）。
//!
//! # 計算（画素ごと。f32、この順序）
//!
//! ```text
//! p  = log2(max(Y, 0) + 2^−8)、 b = p をガウスでぼかしたもの（σ = 半径（写真のフル解像度の画素）× 縮小率）
//! d  = p − b
//! f  = d · d² / (d² + t²)        （t = THRESHOLD_MAX × (1 − ディテール / 100)。t = 0 なら f = d）
//! h  = f / (1 + |f| / L)         （L = LIMIT_MIN + (LIMIT_MAX − LIMIT_MIN) × ディテール / 100。輪郭のにじみを抑える）
//! m  = smoothstep(0.5·M, 1.5·M, e)   （マスク。M = MASK_EDGE_MAX × マスク / 100。マスク 0 なら m = 1）
//! e  = √(gx² + gy²) × 縮小率       （gx = (b(x+1, y) − b(x−1, y)) / 2 など。フル解像度の 1 画素あたりの段の勾配）
//! Δ  = a · m · h                  （a = 適用量 / 100 × SHARPEN_GAIN）
//! RGB' = Δ を当てた画素（[`super::frequency::apply_luma_log_delta`]）
//! ```
//!
//! - ディテールが小さいほど、小さな明暗の差（細かい模様・雑音）を強めず（`t`）、大きな差（輪郭）の行き過ぎ
//!   （ハロー）を抑える（`L`）。マスクが大きいほど、勾配の小さい平らな部分（空・肌）にかけない。
//! - 半径は写真のフル解像度の画素（G の画素）で表し、G の長辺に対する割合にして縮小率で換算する。長辺 2560px の
//!   プレビューでは σ が 1 画素より小さくなり、効きは小さい（縮小すると細かい明暗は平均される。等倍・書き出しで
//!   効く。IQ-07b の比較の基準（フル解像度を画素の範囲で平均したもの）とも合う）。
//! - タイルの余白: ガウスの半径 + 1 画素（勾配）。
//! - 処理バージョン 1 では何もしない（値があれば [`super::unimplemented_settings`] で知らせる）。

use genzo_model::{DevelopSettings, Phase};

use super::frequency::{
    GaussKernel, apply_luma_log_delta, blur_plane, luma_log, luma_log_plane, smoothstep, soft_limit,
};
use super::map_pixels;
use crate::contract::ColorContract;
use crate::error::Result;
use crate::image::{ImageTile, Roi};
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// 適用量の強さ（適用量 100 で細部の差を 2 倍にする）。仮置き。
pub const SHARPEN_GAIN: f32 = 1.0;
/// ディテール 0 のときのしきい値（段）。仮置き: これより小さな明暗の差はほとんど強めない。
pub const SHARPEN_THRESHOLD_MAX: f32 = 0.08;
/// ディテール 0 のときの差の抑えの目安（段）。仮置き。
pub const SHARPEN_LIMIT_MIN: f32 = 0.25;
/// ディテール 100 のときの差の抑えの目安（段）。仮置き。
pub const SHARPEN_LIMIT_MAX: f32 = 1.0;
/// マスク 100 のときの勾配のしきい値（フル解像度の 1 画素あたりの段）。仮置き。
pub const SHARPEN_MASK_EDGE_MAX: f32 = 0.3;

/// ステージ 14 のパラメータ（GPU 版にも同じ値を渡す）。
#[derive(Debug, Clone, PartialEq)]
pub struct SharpenStageParams {
    /// 強さ（a）。
    pub amount: f32,
    /// しきい値 t（段）。
    pub threshold: f32,
    /// 差の抑えの目安 L（段）。
    pub limit: f32,
    /// マスクの勾配のしきい値 M（段 / フル解像度の画素）。0 ならマスクなし。
    pub mask_edge: f32,
    /// 格子の 1 画素の勾配をフル解像度の 1 画素あたりに直す倍率（[`StageContext::resolution_scale`]）。
    pub gradient_scale: f32,
    /// ぼかし（σ = 半径 × 縮小率）。
    pub kernel: GaussKernel,
}

impl SharpenStageParams {
    /// 細部の差 `d`（段）の、ディテールとにじみの抑えをかけた値（モジュールの doc の `h`）。
    #[inline]
    pub fn shaped(&self, d: f32) -> f32 {
        let f = if self.threshold > 0.0 {
            let d2 = d * d;
            d * d2 / (d2 + self.threshold * self.threshold)
        } else {
            d
        };
        soft_limit(f, self.limit)
    }

    /// マスクの値（`gx`・`gy` は格子の 1 画素あたりの勾配の半分の差。モジュールの doc）。
    #[inline]
    pub fn mask(&self, gx: f32, gy: f32) -> f32 {
        if self.mask_edge <= 0.0 {
            return 1.0;
        }
        let e = (gx * gx + gy * gy).sqrt() * self.gradient_scale;
        smoothstep(0.5 * self.mask_edge, 1.5 * self.mask_edge, e)
    }

    /// 1 画素に適用する（`b` はぼかした輝度の対数、`m` はマスクの値）。
    #[inline]
    pub fn apply(&self, c: [f32; 3], b: f32, m: f32) -> [f32; 3] {
        let delta = self.amount * m * self.shaped(luma_log(c) - b);
        if delta == 0.0 {
            return c;
        }
        apply_luma_log_delta(c, delta)
    }
}

/// ステージ 14: シャープ（処理バージョン 2）。適用量が 0 なら飛ばす。
#[derive(Debug, Clone, Copy, Default)]
pub struct SharpenStage;

impl SharpenStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.sharpen";
    /// 04 の 2.1 節のステージの番号。
    pub const NUMBER: u8 = 14;
}

impl Stage for SharpenStage {
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
        let s = settings.normalized().detail.sharpening;
        if s.is_disabled() {
            return Ok(None);
        }
        let detail = s.detail / 100.0;
        // 半径（G の画素）を G の長辺に対する割合にして、この格子の画素に直す。
        let fraction = f64::from(s.radius) / ctx.frames.g_long_side_px();
        Ok(Some(StageParams::new(SharpenStageParams {
            amount: s.amount / 100.0 * SHARPEN_GAIN,
            threshold: SHARPEN_THRESHOLD_MAX * (1.0 - detail),
            limit: SHARPEN_LIMIT_MIN + (SHARPEN_LIMIT_MAX - SHARPEN_LIMIT_MIN) * detail,
            mask_edge: SHARPEN_MASK_EDGE_MAX * s.masking / 100.0,
            gradient_scale: ctx.resolution_scale as f32,
            kernel: GaussKernel::new(ctx.length_px(fraction)),
        })))
    }
    fn input_roi(&self, output_roi: Roi, params: &StageParams, _: &StageContext<'_>) -> Roi {
        match params.get::<SharpenStageParams>() {
            Some(p) => output_roi.expand(p.kernel.radius + 1),
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
        let p = params.expect::<SharpenStageParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 | ProcessVersion::V2 => {}
        }
        let out = output.roi;
        let need = out.expand(p.kernel.radius + 1);
        let plane = luma_log_plane(input, need)?;
        // ぼかした面は、勾配のために出力より 1 画素広く求める。
        let b_roi = out.expand(1);
        let b = blur_plane(&plane, need, b_roi, &p.kernel)?;
        let bw = b_roi.width as usize;
        map_pixels(input, output, |x, y, c| {
            let i = (x - i64::from(b_roi.x)) as usize;
            let j = (y - i64::from(b_roi.y)) as usize;
            let at = |di: isize, dj: isize| {
                b[(j as isize + dj) as usize * bw + (i as isize + di) as usize]
            };
            let m = if p.mask_edge > 0.0 {
                p.mask((at(1, 0) - at(-1, 0)) * 0.5, (at(0, 1) - at(0, -1)) * 0.5)
            } else {
                1.0
            };
            p.apply(c, at(0, 0), m)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::CoordinateFrames;
    use crate::image::RgbImage;
    use crate::stage::run_stage_cpu;
    use genzo_model::Orientation;

    fn ctx_v2(w: u32, h: u32) -> StageContext<'static> {
        StageContext::new(
            ProcessVersion::V2,
            CoordinateFrames::new(w, h, Orientation::Normal).unwrap(),
        )
    }

    fn settings(amount: f32, detail: f32, masking: f32) -> DevelopSettings {
        let mut s = DevelopSettings::default();
        s.detail.sharpening.amount = amount;
        s.detail.sharpening.detail = detail;
        s.detail.sharpening.masking = masking;
        s
    }

    /// 左が暗く右が明るい縦の境目と、右の部分の細かい模様（±0.02 段）。
    fn edge_tile(w: u32, h: u32, roi: Roi) -> ImageTile {
        let img = RgbImage::from_fn(roi.width, roi.height, ColorContract::B2Working, |x, y| {
            let gx = roi.x + x as i32;
            let gy = roi.y + y as i32;
            let base = if gx < w as i32 / 2 { 0.05 } else { 0.4 };
            let tex = if gx >= w as i32 / 2 + 4 && (gx + gy) % 2 == 0 {
                1.014
            } else {
                1.0
            };
            let _ = h;
            [base * tex; 3]
        })
        .unwrap();
        ImageTile::from_image(roi, img).unwrap()
    }

    fn run(s: &DevelopSettings, w: u32, h: u32) -> ImageTile {
        let ctx = ctx_v2(w, h);
        let p = SharpenStage.params(s, &ctx).unwrap().unwrap();
        let out = Roi::full(w, h);
        let input = edge_tile(w, h, SharpenStage.input_roi(out, &p, &ctx));
        run_stage_cpu(&SharpenStage, &ctx, &input, out, &p)
            .unwrap()
            .0
    }

    #[test]
    fn sharpening_increases_edge_contrast_and_keeps_flat_regions() {
        let (w, h) = (40u32, 12u32);
        let out = run(&settings(100.0, 100.0, 0.0), w, h);
        // 境目のすぐ両側の差が大きくなる（暗い側はより暗く、明るい側はより明るく）。
        let dark = out.image.pixel(19, 6).unwrap()[0];
        let bright = out.image.pixel(20, 6).unwrap()[0];
        assert!(dark < 0.05 && bright > 0.4, "{dark} {bright}");
        // 境目から離れた平らな部分は変わらない。
        assert!((out.image.pixel(5, 6).unwrap()[0] - 0.05).abs() < 1e-6);
    }

    #[test]
    fn detail_and_masking_protect_fine_texture() {
        let (w, h) = (40u32, 12u32);
        let texture_gain = |s: &DevelopSettings| {
            let out = run(s, w, h);
            // 右の細かい模様の振幅（隣り合う画素の差）。
            let a = out.image.pixel(32, 6).unwrap()[0];
            let b = out.image.pixel(33, 6).unwrap()[0];
            (a - b).abs()
        };
        let original = 0.4 * 0.014;
        let high = texture_gain(&settings(150.0, 100.0, 0.0));
        let low = texture_gain(&settings(150.0, 0.0, 0.0));
        let masked = texture_gain(&settings(150.0, 100.0, 100.0));
        assert!(high > original * 1.5, "{high} {original}");
        assert!(low < high * 0.5, "{low} {high}");
        assert!(
            (masked - original).abs() < original * 0.2,
            "{masked} {original}"
        );
        // マスクは強い境目にはかける。
        let out = run(&settings(150.0, 100.0, 100.0), w, h);
        assert!(out.image.pixel(20, 6).unwrap()[0] > 0.4);
    }

    #[test]
    fn version_1_and_zero_amount_skip_and_radius_follows_the_scale() {
        let s = settings(50.0, 25.0, 0.0);
        let v1 = StageContext {
            process_version: ProcessVersion::V1,
            ..ctx_v2(100, 50)
        };
        assert!(SharpenStage.params(&s, &v1).unwrap().is_none());
        assert!(
            SharpenStage
                .params(&settings(0.0, 25.0, 0.0), &ctx_v2(100, 50))
                .unwrap()
                .is_none()
        );
        // 半径 1px（フル解像度）: σ = 1 → 半径 3、縮小率 0.25 では σ = 0.25 → 半径 1。
        let p = SharpenStage.params(&s, &ctx_v2(100, 50)).unwrap().unwrap();
        assert_eq!(p.get::<SharpenStageParams>().unwrap().kernel.radius, 3);
        let small = ctx_v2(100, 50).with_resolution_scale(0.25);
        let q = SharpenStage.params(&s, &small).unwrap().unwrap();
        let qp = q.get::<SharpenStageParams>().unwrap();
        assert_eq!(qp.kernel.radius, 1);
        assert_eq!(qp.gradient_scale, 0.25);
        assert_eq!(
            SharpenStage.input_roi(Roi::new(0, 0, 4, 4), &q, &small),
            Roi::new(-2, -2, 8, 8)
        );
        // 差の形: しきい値 0 ならそのまま、抑えは符号を保つ。
        let mut sp = qp.clone();
        sp.threshold = 0.0;
        assert_eq!(sp.shaped(0.0), 0.0);
        assert!(sp.shaped(-0.3) < 0.0);
        assert_eq!(sp.mask(0.0, 0.0), 1.0);
    }
}
