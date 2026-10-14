//! マスクの重ねて表示（O キー）と色域の範囲のスポイト（DEV-18・DEV-19。[`Engine::mask_overlay`]・
//! [`Engine::mask_color_at`]）。
//!
//! どちらも、プレビューと同じ段階 A1（最終品質・長辺 `long_edge`。ふつうはエンジンのキャッシュにある）からステージ 9
//! だけを描いた **基準の画像**（[`MaskReference`]。局所の値の面を作るときと同じ、露光量の前の B2。C の格子）を使う。
//! 格子はプレビューと同じ（[`GeometryPlan::from_settings`]。切り抜く前の全体の表示ならその格子）なので、重みの画像は
//! 同じ `long_edge` のプレビューの画素にそのまま重なる。
//!
//! - 重ねて表示: マスク 1 つの重み（[`crate::local::mask_weights`]。差分・有効かによらない）。処理バージョン 1 の写真でも
//!   求められる（描画には使われない）。
//! - スポイト: 点（出力の画像の幅・高さに対する割合）のまわりの基準の画像の平均の色の OKLab と、点の G 座標。半径は
//!   WB のスポイトと同じ割合（[`super::WB_PICK_RADIUS`]。G の格子の長辺に対して）。切り抜く前の全体の表示で回転で
//!   画像の外になった四隅の画素は平均しない。

use genzo_color::oklab::linear_bt2020_to_oklab;
use genzo_model::{DevelopSettings, RenderQuality};

use super::{Engine, PhotoSource, WB_PICK_RADIUS, gather_mirror_from};
use crate::error::{PipelineError, Result};
use crate::finish::geometry::{GeometryPlan, GeometryStage, OUTSIDE_IMAGE_FILL};
use crate::image::{ImageTile, Roi};
use crate::stage::{InputKind, Stage, run_stage_cpu};
use crate::version::ProcessVersion;

/// マスクの基準の画像（ステージ 9 の出力。モジュールの doc）。
#[derive(Debug, Clone)]
pub struct MaskReference {
    /// 画像（B2。C の格子の全体）。
    pub tile: ImageTile,
    /// C の格子（プレビューと同じ）。
    pub geometry: GeometryPlan,
    /// 入力の種類（輝度の範囲の明るさのカーブ）。
    pub input_kind: InputKind,
}

/// マスク 1 つの重みの画像（[`Engine::mask_overlay`]）。
#[derive(Debug, Clone, PartialEq)]
pub struct MaskOverlay {
    /// 幅（プレビューと同じ C の格子）。
    pub width: u32,
    /// 高さ。
    pub height: u32,
    /// 重み（0〜1。行優先）。
    pub weights: Vec<f32>,
}

/// 色域の範囲のスポイトの結果（[`Engine::mask_color_at`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaskColorPick {
    /// 点の G 座標の x。
    pub x: f64,
    /// 点の G 座標の y。
    pub y: f64,
    /// 平均の色の OKLab（露光量の前の B2 から）。
    pub lab: [f32; 3],
    /// 平均した画素の数。
    pub samples: u64,
}

impl Engine {
    /// マスクの基準の画像（モジュールの doc）。`uncropped` は切り抜く前の全体の表示か。
    pub fn mask_reference(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        long_edge: u32,
        uncropped: bool,
    ) -> Result<MaskReference> {
        ProcessVersion::from_u32(settings.process_version)?;
        let a1 = self.a1(source, settings, RenderQuality::Final, long_edge)?;
        let profiles = self.resolve_profiles(settings);
        let ctx = a1
            .finish_context(settings)?
            .with_uncropped(uncropped)
            .with_profiles(&profiles);
        let geometry = GeometryPlan::from_settings(settings, &ctx)?;
        let (w, h) = geometry.output_size();
        let full = Roi::full(w, h);
        let mut tile = match GeometryStage.params(settings, &ctx)? {
            Some(p) => {
                let need = GeometryStage.input_roi(full, &p, &ctx);
                let input = gather_mirror_from(&a1.image, need)?;
                run_stage_cpu(&GeometryStage, &ctx, &input, full, &p)?.0
            }
            None => gather_mirror_from(&a1.image, full)?,
        };
        geometry.fill_outside_image(&mut tile, OUTSIDE_IMAGE_FILL);
        Ok(MaskReference {
            tile,
            geometry,
            input_kind: ctx.input_kind,
        })
    }

    /// マスク `index`（設定の `masks` の番号）の重みの画像（重ねて表示。モジュールの doc）。番号が範囲の外なら
    /// [`PipelineError::InvalidArgument`]。
    pub fn mask_overlay(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        index: usize,
        long_edge: u32,
        uncropped: bool,
    ) -> Result<MaskOverlay> {
        let s = settings.normalized();
        let Some(mask) = s.masks.get(index) else {
            return Err(PipelineError::InvalidArgument(format!(
                "マスク {index} はありません（{} 個）",
                s.masks.len()
            )));
        };
        let r = self.mask_reference(source, settings, long_edge, uncropped)?;
        let weights =
            crate::local::mask_weights(mask, &r.geometry, &r.tile, s.exposure_ev, r.input_kind);
        let (width, height) = r.geometry.output_size();
        Ok(MaskOverlay {
            width,
            height,
            weights,
        })
    }

    /// 色域の範囲のスポイト（モジュールの doc）。`point` は出力の画像（`uncropped` なら切り抜く前の全体）の幅・高さに
    /// 対する割合（各 0〜1）。点が範囲の外・まわりに画像の画素がない・暗すぎる（輝度が 0 以下）なら
    /// [`PipelineError::InvalidArgument`]。
    pub fn mask_color_at(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        point: (f64, f64),
        long_edge: u32,
        uncropped: bool,
    ) -> Result<MaskColorPick> {
        let (px, py) = point;
        let unit = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
        if !(unit(px) && unit(py)) {
            return Err(PipelineError::InvalidArgument(format!(
                "点の位置は画像の幅・高さに対する 0〜1 の割合で指定してください（{px}, {py}）"
            )));
        }
        let r = self.mask_reference(source, settings, long_edge, uncropped)?;
        let g = &r.geometry;
        let (w, h) = g.output_size();
        let (cx, cy) = (px * f64::from(w), py * f64::from(h));
        let (lw, lh) = g.lattice_size();
        let radius = (WB_PICK_RADIUS * f64::from(lw.max(lh))).max(1.0);
        let span = |c: f64, n: u32| {
            let lo = (c - radius - 0.5).ceil().max(0.0) as u32;
            let hi = ((c + radius - 0.5).floor() + 1.0).clamp(0.0, f64::from(n)) as u32;
            lo..hi.max(lo)
        };
        let mut sum = [0.0f64; 3];
        let mut samples = 0u64;
        for y in span(cy, h) {
            for x in span(cx, w) {
                if g.c_pixel_outside_image(i64::from(x), i64::from(y)) {
                    continue;
                }
                let p = r.tile.image.data()[(y * w + x) as usize];
                for (s, v) in sum.iter_mut().zip(p) {
                    *s += f64::from(v);
                }
                samples += 1;
            }
        }
        if samples == 0 {
            return Err(PipelineError::InvalidArgument(
                "指定した点のまわりに画像の画素がありません".to_owned(),
            ));
        }
        let mean = sum.map(|v| (v / samples as f64) as f32);
        if crate::finish::luminance(mean) <= 0.0 {
            return Err(PipelineError::InvalidArgument(
                "指定した点が暗すぎるため、色を選べません".to_owned(),
            ));
        }
        let (gx, gy) = g.c_to_g_px(cx, cy);
        let long = g.frames().g_long_side_px();
        Ok(MaskColorPick {
            x: gx / long,
            y: gy / long,
            lab: linear_bt2020_to_oklab(mean),
            samples,
        })
    }
}
