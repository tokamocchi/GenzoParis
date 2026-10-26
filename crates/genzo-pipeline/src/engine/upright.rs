//! 自動の遠近補正（Upright 相当。DEV-16）の解析（[`Engine::analyze_upright`]）。
//!
//! # 方法
//!
//! 1. 段階 A1 のプレビュー（最終品質、長辺 [`crate::EngineConfig::preview_long_edge`]。プレビューの描画と同じもので、
//!    ふつうはエンジンのキャッシュにある）を今の設定で求める。
//! 2. 歪曲補正の後の枠 D（[`crate::coords`]。回転・遠近補正の前。歪曲補正は今の設定のレンズ補正）の輝度の画像を、
//!    長辺 [`UPRIGHT_ANALYSIS_LONG_EDGE`] px 以下（A1 の密度以下）で作る。ガイドと同じく、A1 の輝度を解析の密度まで
//!    面積平均で縮小してから、D の画素ごとに D → O → S とたどって双線形補間で引く。
//! 3. 直線を検出して値を求める（[`crate::upright`]）。線分の座標は D の画素に直す。
//!
//! 結果は設定の値（角度補正・垂直・水平）で、適用はしない（genzo-api の `Core::analyze_upright` が返し、画面・CLI が
//! `set_settings` で書き込む）。

use genzo_model::{DevelopSettings, RenderQuality, Transform};
use rayon::prelude::*;

use super::{Engine, PhotoSource};
use crate::error::{PipelineError, Result};
use crate::finish::{luminance, stage_c_frames};
use crate::guide::downscale_gray;
use crate::image::GrayImage;
use crate::interp::sample_bilinear_gray;
use crate::upright::{self, ANALYSIS_LONG_EDGE, UprightAnalysis, UprightMode};
use crate::version::ProcessVersion;

/// 解析に使う D の画像の長辺の上限（px。[`crate::upright::ANALYSIS_LONG_EDGE`]）。
pub const UPRIGHT_ANALYSIS_LONG_EDGE: u32 = ANALYSIS_LONG_EDGE;

impl Engine {
    /// 自動の遠近補正の値を求める（モジュールの doc の「方法」）。設定は変えない。
    ///
    /// 垂直・完全のモードは処理バージョン 2 から（版 1 では遠近補正が適用されないので
    /// [`PipelineError::InvalidArgument`]）。水平（角度補正だけ）はどの版でも使える。直線が見つからなければ
    /// [`PipelineError::InvalidArgument`]（[`crate::upright::analyze`]）。
    pub fn analyze_upright(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        mode: UprightMode,
    ) -> Result<UprightAnalysis> {
        let version = ProcessVersion::from_u32(settings.process_version)?;
        if mode != UprightMode::Level && !version.has_v1_features() {
            return Err(PipelineError::InvalidArgument(format!(
                "この写真の処理バージョン（{}）では遠近補正が使われません。処理バージョンを更新してください",
                settings.process_version
            )));
        }
        let s = settings.normalized();
        let a1 = self.a1(
            source,
            settings,
            RenderQuality::Final,
            self.config.preview_long_edge,
        )?;
        // D の座標系（回転・遠近補正なし。歪曲補正は今の設定）。
        let frames = stage_c_frames(a1.frames(), version, 0.0, &s.lens, &Transform::default())?;
        let (ow, oh) = frames.oriented_size();
        let grid = a1.sensor_grid();
        let (gdx, gdy) = grid.density();
        // 解析の密度（D の 1 画素あたりの解析の画素）。A1 の密度より高くしない。
        let k = (f64::from(ANALYSIS_LONG_EDGE) / f64::from(ow.max(oh))).min(gdx.min(gdy));
        let w = ((f64::from(ow) * k).round() as u32).max(1);
        let h = ((f64::from(oh) * k).round() as u32).max(1);
        if w < upright::MIN_IMAGE_SIZE || h < upright::MIN_IMAGE_SIZE {
            return Err(PipelineError::InvalidArgument(format!(
                "画像が小さすぎて直線を検出できません（{ow} × {oh}）"
            )));
        }
        // A1 の輝度を解析の密度まで縮小する（ガイドと同じ手順）。
        let y_img = GrayImage::from_vec(
            grid.width,
            grid.height,
            a1.image.data().par_iter().map(|&c| luminance(c)).collect(),
        )?;
        let iw = ((grid.extent.0 * k).round() as u32).clamp(1, grid.width);
        let ih = ((grid.extent.1 * k).round() as u32).clamp(1, grid.height);
        let small = downscale_gray(&y_img, iw, ih)?;
        let (sx, sy) = (f64::from(iw) / grid.extent.0, f64::from(ih) / grid.extent.1);
        let mut values = vec![0.0f32; w as usize * h as usize];
        values
            .par_chunks_mut(w as usize)
            .enumerate()
            .for_each(|(q, row)| {
                for (p, out) in row.iter_mut().enumerate() {
                    // D の画素の中心 → O（歪曲補正の逆）→ S → 縮小した格子。
                    let (dx, dy) = ((p as f64 + 0.5) / k, (q as f64 + 0.5) / k);
                    let (u, v) = frames.g_to_oriented(dx, dy);
                    let (xs, ys) = frames.oriented_to_sensor(u, v);
                    *out = sample_bilinear_gray(&small, (xs * sx) as f32, (ys * sy) as f32);
                }
            });
        let gray = GrayImage::from_vec(w, h, values)?;
        let lines = upright::detect_lines(&gray)?;
        let in_d: Vec<_> = lines.iter().map(|l| l.scaled(1.0 / k, 1.0 / k)).collect();
        upright::analyze(&in_d, (ow, oh), mode)
    }
}
