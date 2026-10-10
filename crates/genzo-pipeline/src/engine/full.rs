//! 等倍表示・書き出しのタイル処理（docs/04_architecture.md の 2.2 節「フル解像度をタイルに分けて
//! 段階 A1〜C を実行する」、2.7 節「タイル処理」。01 の PRV-03、02 の SCL-05）。
//!
//! # タイルの処理（[`FullResolutionJob::render_tile`]）
//!
//! 出力のタイル `T`（C の格子、フル解像度）ごとに、次の範囲を **出力側から順に** たどる
//! （[`FullResolutionJob::tile_rois`]）:
//!
//! 1. ステージ 16 → 9 の `input_roi`（[`FinishPlan::stage_rois`]）。ステージ 10〜16 は画素ごとの処理
//!    なので `T` のまま、ステージ 9 は C → G → O → S の逆写像の外接矩形にバイキュービックのタップと
//!    丸めの余白を足したもの（S の格子。画像の外にはみ出してよい）。
//! 2. RAW では、続けてステージ 8 → 5 → 3 → 2 の `input_roi`（デモザイクの余白・飽和の切りそろえの
//!    余白）。[`crate::SensorPlan::render_roi`] が、画像の内側の範囲だけを処理し、はみ出した部分を
//!    鏡映で埋める（2.7 節）。
//!
//! RAW 以外の入力は、1 の範囲を B2 の画像から鏡映で取り出す（NaN・無限大は 0 にする）。
//!
//! # タイルの並べ方
//!
//! タイルは出力の画像全体の格子（一辺 `tile_size`、左上が (0, 0)）に揃え、求める範囲と重なる部分を
//! 左上から行の順に処理する（[`FullResolutionJob::tiles`]）。ルーペの表示範囲を描くときも同じ格子を
//! 使うので、UI はタイルを再利用できる（L2。4 章）。画素の値はタイルの分け方によらない。
//!
//! # 縮小（[`FullResolutionJob::for_each_downscaled_band`]）
//!
//! 書き出しの寸法の指定（`ExportSize::LongEdge`）と IQ-07b の比較のため、フル解像度の B3 を面積平均で
//! 縮小する（2.4 節「フル解像度で処理してから縮小」。フィルタは [`crate::resample`] と同じ定義）。
//! 出力の行の帯ごとに、必要な入力の行（[`AreaWeights::source_range`]）だけをタイルで描き、
//! 横方向はタイルを左から順に入力の番号の小さい順に足し込み（[`crate::resample::downscale_row`] と同じ
//! 順序）、縦方向は [`crate::resample::downscale_column_row`] で足す。帯の境目の入力の行（最大 1 行）は
//! 両方の帯で描く。帯の入力の行がタイルの一辺より多い場合（縮小の倍率が大きい場合）は、縦にもタイルに
//! 分ける（行ごとの足し込みの順序は変わらない）。結果はフル解像度の B3 全体を [`crate::downscale_area`]
//! で縮小したものとビット単位で同じになる（テストで確認する）。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use genzo_model::{DevelopSettings, RenderQuality};
use rayon::prelude::*;

use super::control::{RenderControl, check_cancelled};
use super::{
    Engine, OutputTarget, PhotoSource, blit, gather_mirror_from, plan_with_guide, run_scene_stages,
};
use crate::contract::ColorContract;
use crate::coords::{CoordinateFrames, SensorGrid};
use crate::error::{PipelineError, Result};
use crate::finish::geometry::GeometryPlan;
use crate::finish::{FinishPlan, UnimplementedSetting, finish_context, unimplemented_settings};
use crate::image::{ImageTile, RgbImage, Roi};
use crate::resample::{AreaWeights, downscale_column_row};
use crate::sensor::{SensorPlan, SensorWarning, SourceImage};
use crate::stage::{GpuStageLookup, Guide, StageContext};
use crate::version::ProcessVersion;

/// タイル 1 つの作業領域の概算に使う、CFA の 1 画素あたりのバイト数。**仮置き**: f32 の値と飽和の印
/// （1 バイト）を、ステージ 2 と 3 の出力の 2 つ分。
const CFA_BYTES_PER_PIXEL: u64 = 2 * (4 + 1);

/// タイル 1 つの作業領域の概算に使う、センサーの出力（RGB）の 1 画素あたりのバイト数。**仮置き**:
/// f32 の RGB（12 バイト）を、デモザイクの出力と内部の作業領域の 2 つ分とみなした（RCD の内部の
/// 配列の大きさは実測していない。PoC-3 で確認する）。
const SENSOR_RGB_BYTES_PER_PIXEL: u64 = 2 * 12;

/// タイル 1 つの作業領域の概算に使う、段階 C の 1 画素あたりのバイト数。**仮置き**: 各ステージは
/// 入力と出力の 2 枚を同時に持つ（f32 の RGB）。
const SCENE_BYTES_PER_PIXEL: u64 = 2 * 12;

/// ステージを処理する側（[`FullResolutionJob::stage_backends`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StageBackend {
    /// GPU 版（genzo-gpu）。
    Gpu,
    /// CPU 版（この crate の基準実装）。
    Cpu,
}

/// 1 つのタイルの各ステージの範囲（出力側からたどったもの）。
#[derive(Debug, Clone, PartialEq)]
pub struct TileRois {
    /// 出力のタイル（C の格子）。
    pub output: Roi,
    /// [`FinishPlan::stage_rois`]: `[0]` がステージ 9（または最初の有効なステージ）の入力（S の格子）、
    /// 最後が `output`。
    pub scene: Vec<Roi>,
    /// RAW のときのセンサーのステージが実際に処理する範囲（[`SensorPlan::stage_rois`]）: `[0]` が
    /// ステージ 2 の入力（RAW の上。画像の外にはみ出す部分は鏡映で埋める）、`[1]`〜`[3]` がステージ
    /// 3・5・8 の入力、`[4]` がステージ 8 の出力（`scene[0]` を鏡映で覆う画像の内側の範囲。画像の
    /// 内側のタイルでは `scene[0]` と同じ。はみ出した部分はこの結果を鏡映して作る）。RAW 以外では空。
    pub sensor: Vec<Roi>,
}

impl TileRois {
    /// 最も外側の入力の範囲（RAW はステージ 2 の入力、RAW 以外はステージ 9 の入力）。
    pub fn outermost(&self) -> Roi {
        self.sensor.first().copied().unwrap_or(self.scene[0])
    }

    /// ステージ 9 の入力（`scene[0]`、S の格子）に対する、最も外側の入力の余白（左・上・右・下。
    /// 画素。センサー処理の余白）。
    ///
    /// 出力のタイル（C の格子）とは比べない: ステージ 9 は格子を変える（切り抜きの位置・回転・向き）
    /// ので、C と S の座標の差は余白の意味を持たない。ステージ 9 の余白（補間のタップ）は `scene[0]`
    /// に含まれる。画像の端では、センサー処理は画像の内側だけを計算する（はみ出した部分は鏡映）ので、
    /// 余白が負になりうる。RAW 以外は 0。
    pub fn margins(&self) -> [i64; 4] {
        let o = self.outermost();
        let t = self.scene[0];
        [
            i64::from(t.x) - i64::from(o.x),
            i64::from(t.y) - i64::from(o.y),
            o.x_end() - t.x_end(),
            o.y_end() - t.y_end(),
        ]
    }
}

/// 等倍表示（ルーペ）・書き出しのためのフル解像度の処理の準備（センサー処理の設定・ガイド・段階 C の
/// パラメータ）。タイルの処理（[`render_tile`](Self::render_tile)）は何度でも、どの順でも呼べる。
pub struct FullResolutionJob {
    source: SourceImage,
    sensor: Option<SensorPlan>,
    base: StageContext<'static>,
    guide: Option<Arc<Guide>>,
    plan: FinishPlan,
    settings: DevelopSettings,
    process_version: ProcessVersion,
    warnings: Vec<SensorWarning>,
    unimplemented: Vec<UnimplementedSetting>,
    tile_size: u32,
    tiles_rendered: Arc<AtomicU64>,
}

impl std::fmt::Debug for FullResolutionJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FullResolutionJob")
            .field("output_size", &self.output_size())
            .field("plan", &self.plan)
            .field("guide", &self.guide.is_some())
            .field("tile_size", &self.tile_size)
            .finish()
    }
}

/// 範囲を描いた結果（ルーペ）。
#[derive(Debug, Clone)]
pub struct RegionRender {
    /// ステージ 16 の出力（B3）。範囲は描いた範囲（C の格子、フル解像度。出力の画像の内側に収めたもの）。
    pub b3: ImageTile,
    /// ステージ 17a / 17b の出力（要求したとき。範囲は `b3` と同じ）。
    pub output: Option<ImageTile>,
    /// 出力の画像全体の寸法（C の格子、フル解像度）。
    pub full_size: (u32, u32),
    /// 処理したタイルの数。
    pub tiles: u64,
    /// 置き換えた NaN・無限大の数。
    pub non_finite_replaced: u64,
}

impl FullResolutionJob {
    /// 段階 C の文脈（フル解像度・最終品質。ガイドがあれば付けたもの）。
    pub fn context(&self) -> StageContext<'_> {
        match &self.guide {
            Some(g) => self.base.with_guide(g),
            None => self.base,
        }
    }

    /// 段階 C の計画（有効なステージとパラメータ）。
    pub fn plan(&self) -> &FinishPlan {
        &self.plan
    }

    /// ジオメトリ（C の格子と S の対応、切り抜きの位置）。
    pub fn geometry(&self) -> &GeometryPlan {
        self.plan.geometry()
    }

    /// 出力（C の格子、フル解像度）の寸法。
    pub fn output_size(&self) -> (u32, u32) {
        self.plan.output_size()
    }

    /// 使うガイド（ガイドが必要なステージがなければ `None`）。
    pub fn guide(&self) -> Option<&Arc<Guide>> {
        self.guide.as_ref()
    }

    /// センサー処理の設定（RAW のときだけ）。
    pub fn sensor_plan(&self) -> Option<&SensorPlan> {
        self.sensor.as_ref()
    }

    /// 処理バージョン。
    pub fn process_version(&self) -> ProcessVersion {
        self.process_version
    }

    /// センサー処理の警告。
    pub fn warnings(&self) -> &[SensorWarning] {
        &self.warnings
    }

    /// 値が入っているが適用しない項目（v1）。
    pub fn unimplemented(&self) -> &[UnimplementedSetting] {
        &self.unimplemented
    }

    /// 実行するステージ（センサーのステージ 2〜8 と、段階 C の有効なステージ。処理の順）ごとに、
    /// GPU 版で処理できるか（[`StageBackend`]）。GPU 版がない、またはこの処理バージョンに対応して
    /// いないステージは CPU 版で処理する（7.1 節）。GPU 版の実行は genzo-gpu の担当。
    pub fn stage_backends(&self, lookup: &dyn GpuStageLookup) -> Vec<(&'static str, StageBackend)> {
        let version = self.process_version;
        let pick = |gpu: Option<&dyn crate::stage::GpuStage>| match gpu {
            Some(g) if g.supports(version) => StageBackend::Gpu,
            _ => StageBackend::Cpu,
        };
        let mut out = Vec::new();
        if let Some(plan) = &self.sensor {
            for stage in plan.stages() {
                out.push((stage.id(), pick(stage.gpu(lookup))));
            }
        }
        for (stage, _) in self.plan.stages() {
            out.push((stage.id(), pick(stage.gpu(lookup))));
        }
        out
    }

    /// タイルの一辺の既定値（エンジンの設定）。
    pub fn tile_size(&self) -> u32 {
        self.tile_size
    }

    /// 範囲 `region`（C の格子）を、出力の画像全体の格子（一辺 `tile_size`）に揃えたタイルに分ける
    /// （左上から行の順。範囲は出力の画像の内側に収める）。範囲が出力と重ならなければエラー。
    pub fn tiles(&self, region: Roi, tile_size: u32) -> Result<Vec<Roi>> {
        if tile_size == 0 {
            return Err(PipelineError::InvalidArgument(
                "タイルの一辺は 1 以上が必要です".to_owned(),
            ));
        }
        let region = self.clip_region(region)?;
        let ts = i64::from(tile_size);
        let (x0, y0) = (i64::from(region.x) / ts, i64::from(region.y) / ts);
        let (x1, y1) = (
            (region.x_end() + ts - 1) / ts,
            (region.y_end() + ts - 1) / ts,
        );
        let mut out = Vec::new();
        for ty in y0..y1 {
            for tx in x0..x1 {
                let cell = Roi::new((tx * ts) as i32, (ty * ts) as i32, tile_size, tile_size);
                if let Some(t) = cell.intersect(&region) {
                    out.push(t);
                }
            }
        }
        Ok(out)
    }

    /// 範囲を出力の画像の内側に収める。重ならなければエラー。
    fn clip_region(&self, region: Roi) -> Result<Roi> {
        let (w, h) = self.output_size();
        region
            .intersect(&Roi::full(w, h))
            .ok_or(PipelineError::InvalidRoi {
                roi: region,
                reason: "範囲が出力の画像と重ならない",
            })
    }

    /// タイル `tile` の各ステージの範囲（出力側からたどる。モジュールの doc）。
    pub fn tile_rois(&self, tile: Roi) -> TileRois {
        let ctx = self.context();
        let scene = self.plan.stage_rois(&ctx, tile);
        // render_tile → SensorPlan::render_roi と同じ範囲（空のタイルでは空）。
        let sensor = match &self.sensor {
            Some(plan) => plan
                .stage_rois(scene[0])
                .map(|r| r.to_vec())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        TileRois {
            output: tile,
            scene,
            sensor,
        }
    }

    /// タイル 1 つの作業領域の概算（バイト。**仮置き**の係数による。メモリの予算（genzo-jobs の
    /// `MemoryBudget`）から枠を取るときの目安。PoC-3 で実測して見直す）。
    pub fn estimate_tile_bytes(&self, tile_size: u32) -> u64 {
        let t = tile_size.max(1);
        let (w, h) = self.output_size();
        let tile = Roi::new(0, 0, t.min(w), t.min(h));
        let r = self.tile_rois(tile);
        let scene_in = r.scene[0].pixel_count();
        let sensor = match r.sensor.first() {
            Some(cfa) => {
                cfa.pixel_count() * CFA_BYTES_PER_PIXEL + scene_in * SENSOR_RGB_BYTES_PER_PIXEL
            }
            None => scene_in * 12,
        };
        sensor + scene_in.max(tile.pixel_count()) * SCENE_BYTES_PER_PIXEL
    }

    /// タイル `tile`（C の格子。出力の画像の内側）の B3 を描く（段階 A1 のセンサー処理から段階 C の
    /// ステージ 16 まで）。置き換えた NaN・無限大の数も返す。
    pub fn render_tile(&self, tile: Roi) -> Result<(ImageTile, u64)> {
        let (w, h) = self.output_size();
        if tile.is_empty() || !tile.is_inside(w, h) {
            return Err(PipelineError::InvalidRoi {
                roi: tile,
                reason: "タイルが出力の画像の内側にない",
            });
        }
        let ctx = self.context();
        let rois = self.plan.stage_rois(&ctx, tile);
        let (input, mut replaced) = match (&self.source, &self.sensor) {
            (SourceImage::Raw(raw), Some(plan)) => plan.render_roi(raw, rois[0])?,
            (SourceImage::Working { image, .. }, None) => {
                let mut t = gather_mirror_from(image, rois[0])?;
                let n = t.image.sanitize();
                (t, n)
            }
            _ => unreachable!("RAW のときだけセンサー処理の設定を持つ（作るときに決まる）"),
        };
        let (b3, n) = run_scene_stages(&self.plan, &ctx, &rois, input)?;
        replaced += n;
        self.tiles_rendered.fetch_add(1, Ordering::Relaxed);
        Ok((b3, replaced))
    }

    /// B3 のタイルにステージ 17a / 17b を適用する。
    pub fn apply_output(&self, target: &OutputTarget, b3: &ImageTile) -> Result<(ImageTile, u64)> {
        target.apply(&self.settings, &self.context(), b3)
    }

    /// 範囲 `region` をタイル（一辺 `tile_size`）に分けて順に描き、タイルごとに `f(B3 のタイル)` を
    /// 呼ぶ。タイルの前ごとに取り消しを確かめ、後ごとに進捗を知らせる。置き換えた NaN・無限大の数を返す。
    pub fn for_each_tile(
        &self,
        region: Roi,
        tile_size: u32,
        control: &dyn RenderControl,
        mut f: impl FnMut(ImageTile) -> Result<()>,
    ) -> Result<u64> {
        let tiles = self.tiles(region, tile_size)?;
        let total = tiles.len() as u64;
        let mut replaced = 0;
        for (i, t) in tiles.into_iter().enumerate() {
            check_cancelled(control)?;
            let (b3, n) = self.render_tile(t)?;
            replaced += n;
            f(b3)?;
            control.on_progress(i as u64 + 1, total);
        }
        Ok(replaced)
    }

    /// 範囲 `region`（C の格子、フル解像度。出力の画像の内側に収める）を描く（ルーペの等倍表示。
    /// PRV-03）。`output` を指定すれば 17a / 17b も適用する。
    pub fn render_region(
        &self,
        region: Roi,
        tile_size: u32,
        output: Option<&OutputTarget>,
        control: &dyn RenderControl,
    ) -> Result<RegionRender> {
        let region = self.clip_region(region)?;
        let len = region.pixel_count() as usize;
        let mut b3 = vec![[0.0f32; 3]; len];
        let mut out = output.map(|_| vec![[0.0f32; 3]; len]);
        let mut tiles = 0;
        let mut extra = 0;
        let replaced = self.for_each_tile(region, tile_size, control, |tile| {
            blit(&mut b3, region, tile.roi, tile.image.data(), 1);
            if let (Some(target), Some(buf)) = (output, out.as_mut()) {
                let (o, n) = self.apply_output(target, &tile)?;
                extra += n;
                blit(buf, region, o.roi, o.image.data(), 1);
            }
            tiles += 1;
            Ok(())
        })?;
        let b3 = ImageTile::from_image(
            region,
            RgbImage::from_vec(
                region.width,
                region.height,
                ColorContract::B3DisplayReferred,
                b3,
            )?,
        )?;
        let output = match (output, out) {
            (Some(target), Some(data)) => Some(ImageTile::from_image(
                region,
                RgbImage::from_vec(region.width, region.height, target.contract(), data)?,
            )?),
            _ => None,
        };
        Ok(RegionRender {
            b3,
            output,
            full_size: self.output_size(),
            tiles,
            non_finite_replaced: replaced + extra,
        })
    }

    /// 出力の全体を面積平均で `size` に縮小しながら描き（モジュールの doc の「縮小」）、出力の行の帯ごとに
    /// `f(縮小した B3 の帯)` を呼ぶ（帯の範囲は縮小後の画像の座標で、幅は全体）。`size` は出力の寸法
    /// 以下（拡大はしない）。帯の入力の行数はおよそ `tile_size`。置き換えた NaN・無限大の数を返す。
    pub fn for_each_downscaled_band(
        &self,
        size: (u32, u32),
        tile_size: u32,
        control: &dyn RenderControl,
        mut f: impl FnMut(ImageTile) -> Result<()>,
    ) -> Result<u64> {
        if tile_size == 0 {
            return Err(PipelineError::InvalidArgument(
                "タイルの一辺は 1 以上が必要です".to_owned(),
            ));
        }
        let (w, h) = self.output_size();
        let (dw, dh) = size;
        let wx = AreaWeights::new(w, dw)?;
        let wy = AreaWeights::new(h, dh)?;
        let rows_per_band = rows_per_band(tile_size, h, dh);
        let bands: Vec<(u32, u32, u32, u32)> = (0..dh)
            .step_by(rows_per_band as usize)
            .map(|y| {
                let y1 = (y + rows_per_band).min(dh);
                let (iy0, iy1) = wy.source_range(y, y1);
                (y, y1, iy0, iy1)
            })
            .collect();
        // 帯の入力の行が tile_size を超える場合（縮小の倍率が大きい場合）は、縦にもタイルに分ける。
        let tiles_per_band = u64::from(w.div_ceil(tile_size));
        let total: u64 = bands
            .iter()
            .map(|&(_, _, iy0, iy1)| u64::from((iy1 - iy0).div_ceil(tile_size)) * tiles_per_band)
            .sum();
        let dw_us = dw as usize;
        let mut done = 0;
        let mut replaced = 0;
        for (oy0, oy1, iy0, iy1) in bands {
            // 横方向に縮小した入力の行（入力の行 iy0..iy1）。各行は、タイルを左から順に足し込む。
            let mut hacc = vec![[0.0f32; 3]; (iy1 - iy0) as usize * dw_us];
            let mut y = iy0;
            while y < iy1 {
                let th = tile_size.min(iy1 - y);
                let rows = &mut hacc[(y - iy0) as usize * dw_us..(y - iy0 + th) as usize * dw_us];
                let mut x = 0u32;
                while x < w {
                    check_cancelled(control)?;
                    let tw = tile_size.min(w - x);
                    let (b3, n) = self.render_tile(Roi::new(x as i32, y as i32, tw, th))?;
                    replaced += n;
                    rows.par_chunks_mut(dw_us)
                        .zip(b3.image.data().par_chunks(tw as usize))
                        .for_each(|(acc, src)| accumulate_columns(&wx, x, src, acc));
                    x += tw;
                    done += 1;
                    control.on_progress(done, total);
                }
                y += th;
            }
            // 縦方向（出力の行ごと）。
            let mut data = vec![[0.0f32; 3]; (oy1 - oy0) as usize * dw_us];
            data.par_chunks_mut(dw_us).enumerate().for_each(|(k, out)| {
                downscale_column_row(
                    &wy,
                    oy0 + k as u32,
                    |j| {
                        let r = (j - iy0) as usize;
                        &hacc[r * dw_us..(r + 1) * dw_us]
                    },
                    out,
                );
            });
            let band = Roi::new(0, oy0 as i32, dw, oy1 - oy0);
            f(ImageTile::from_image(
                band,
                RgbImage::from_vec(dw, oy1 - oy0, ColorContract::B3DisplayReferred, data)?,
            )?)?;
        }
        Ok(replaced)
    }

    /// 出力の全体を面積平均で `size` に縮小した B3（[`for_each_downscaled_band`](Self::for_each_downscaled_band)
    /// を集めたもの。IQ-07b の比較、書き出しの寸法の指定に使う）。
    pub fn render_downscaled(
        &self,
        size: (u32, u32),
        tile_size: u32,
        control: &dyn RenderControl,
    ) -> Result<(ImageTile, u64)> {
        let (w, h) = self.output_size();
        if size.0 == 0 || size.1 == 0 || size.0 > w || size.1 > h {
            return Err(PipelineError::InvalidArgument(format!(
                "縮小の寸法 {size:?} は 1 以上・出力の寸法 {w}×{h} 以下が必要です"
            )));
        }
        let full = Roi::full(size.0, size.1);
        let mut data = vec![[0.0f32; 3]; full.pixel_count() as usize];
        let replaced = self.for_each_downscaled_band(size, tile_size, control, |band| {
            blit(&mut data, full, band.roi, band.image.data(), 1);
            Ok(())
        })?;
        let image = RgbImage::from_vec(
            full.width,
            full.height,
            ColorContract::B3DisplayReferred,
            data,
        )?;
        Ok((ImageTile::from_image(full, image)?, replaced))
    }
}

/// 縮小の帯 1 つの出力の行数。帯の入力の行（[`AreaWeights::source_range`]）が `tile_size` 行に
/// 収まる最大の行数（1 行の出力の入力が `tile_size` を超える大きな縮小では 1 行）。
///
/// `r` 行の出力が参照する入力の行は `ceil(r × h / dh) + 1` 行以下（両端の画素の一部の重なり）なので、
/// `r = ⌊(tile_size − 1) × dh / h⌋` なら `tile_size` 行以下になる。`tile_size × dh / h` にすると帯の
/// 入力が `tile_size + 1` 行になる場合があり、1 行だけのタイル（余白は通常のタイルと同じだけ要る）を
/// 余分に描くことになる。結果（画素の値）は帯の分け方によらない。
fn rows_per_band(tile_size: u32, h: u32, dh: u32) -> u32 {
    let r = (u64::from(tile_size.saturating_sub(1)) * u64::from(dh)) / u64::from(h.max(1));
    r.clamp(1, u64::from(dh.max(1))) as u32
}

/// 入力の列 `[x0, x0 + src.len())` の画素を、横方向の面積平均の和 `acc`（長さは縮小後の幅）に足し込む。
///
/// 出力の画素ごとに、重みの表（[`AreaWeights::taps`]）のうちこの列の範囲に入る入力を、番号の小さい順に
/// `acc = acc + w × v` で足す。タイルを左から順に渡せば、[`crate::resample::downscale_row`] と同じ順序・
/// 同じ式の和になる（`acc` は 0 で始めること）。
fn accumulate_columns(wx: &AreaWeights, x0: u32, src: &[[f32; 3]], acc: &mut [[f32; 3]]) {
    let x1 = x0 + src.len() as u32;
    let (w, dw) = (u64::from(wx.src_len()), u64::from(wx.dst_len()));
    // この列の範囲に重なりうる出力の画素（前後に 1 画素の余裕を持たせ、重みの表で絞る）。
    let o0 = ((u64::from(x0) * dw) / w).saturating_sub(1) as u32;
    let o1 = ((u64::from(x1) * dw).div_ceil(w) + 1).min(dw) as u32;
    for o in o0..o1 {
        let a = &mut acc[o as usize];
        for &(i, wt) in wx.taps(o) {
            if i >= x0 && i < x1 {
                let p = src[(i - x0) as usize];
                a[0] += wt * p[0];
                a[1] += wt * p[1];
                a[2] += wt * p[2];
            }
        }
    }
}

impl Engine {
    /// フル解像度の処理（等倍表示・書き出し）を準備する。
    ///
    /// - センサー処理は最終品質（RCD。[`crate::EngineConfig::sensor`] の方式の指定があればそれ）。
    /// - ガイドが必要なら、プレビューと同じもの（長辺 [`crate::EngineConfig::preview_long_edge`] の最終
    ///   品質の A1 から作ったもの。キャッシュ）を使う（2.7 節）。
    /// - 未知の処理バージョンはエラー。
    pub fn full_resolution(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
    ) -> Result<FullResolutionJob> {
        let version = ProcessVersion::from_u32(settings.process_version)?;
        let (sensor, frames, grid, warnings) = match &source.image {
            SourceImage::Raw(raw) => {
                let plan =
                    SensorPlan::new(raw, settings, &self.sensor_options(RenderQuality::Final))?;
                let (frames, grid) = (plan.frames(), plan.sensor_grid());
                let warnings = plan.warnings().to_vec();
                (Some(plan), frames, grid, warnings)
            }
            SourceImage::Working { image, orientation } => {
                if image.contract() != ColorContract::B2Working {
                    return Err(PipelineError::ContractMismatch {
                        stage: "engine.source",
                        expected: ColorContract::B2Working,
                        found: image.contract(),
                    });
                }
                let frames = CoordinateFrames::new(image.width(), image.height(), *orientation)?;
                (
                    None,
                    frames,
                    SensorGrid::full_resolution(&frames),
                    Vec::new(),
                )
            }
        };
        let base: StageContext<'static> =
            finish_context(settings, frames, grid, RenderQuality::Final)?;
        let (plan, guide) = plan_with_guide(settings, &base, || {
            self.guide(
                source,
                settings,
                RenderQuality::Final,
                self.config.preview_long_edge,
            )
        })?;
        Ok(FullResolutionJob {
            source: source.image.clone(),
            sensor,
            base,
            guide,
            plan,
            settings: settings.clone(),
            process_version: version,
            warnings,
            unimplemented: unimplemented_settings(settings),
            tile_size: self.config.tile_size,
            tiles_rendered: Arc::clone(&self.tiles_rendered),
        })
    }

    /// 範囲 `region`（C の格子、フル解像度）を描く（ルーペの等倍表示。PRV-03）。タイルの一辺は設定の値。
    pub fn render_region(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        region: Roi,
        output: Option<&OutputTarget>,
        control: &dyn RenderControl,
    ) -> Result<RegionRender> {
        let job = self.full_resolution(source, settings)?;
        job.render_region(region, self.config.tile_size, output, control)
    }

    /// 出力の全体をフル解像度で描く（タイルに分けて処理し、B3 の全体を返す）。フル解像度の B3 を 1 枚
    /// 持つので（α7 IV で約 390MB）、テストと比較のための関数。書き出しは [`Engine::export`] を使う。
    pub fn render_full_resolution(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        output: Option<&OutputTarget>,
        control: &dyn RenderControl,
    ) -> Result<RegionRender> {
        let job = self.full_resolution(source, settings)?;
        let (w, h) = job.output_size();
        job.render_region(Roi::full(w, h), self.config.tile_size, output, control)
    }

    /// フル解像度で処理してから面積平均で `size` に縮小した B3（IQ-07b の比較用。2.4 節）。
    pub fn render_downscaled(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        size: (u32, u32),
        control: &dyn RenderControl,
    ) -> Result<(ImageTile, u64)> {
        let job = self.full_resolution(source, settings)?;
        job.render_downscaled(size, self.config.tile_size, control)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resample::downscale_row;

    #[test]
    fn downscale_bands_fit_in_one_row_of_tiles() {
        // 帯の入力の行がタイルの一辺を超えない（1 行だけのタイルを余分に描かない）。以前の式
        // （tile × dh / h）では、例えば 4672 → 1707 行・タイル 1024 で 1025 行の帯ができた。
        let cases = [
            (4672u32, 1707u32, 1024u32),
            (4672, 1707, 512),
            (7008, 2560, 1024),
            (4672, 3000, 1024),
            (140, 56, 64),
            (37, 36, 5),
            (1000, 1, 1024),
            (5000, 7, 64),
        ];
        let mut old_exceeds = 0;
        for (h, dh, t) in cases {
            let wy = AreaWeights::new(h, dh).unwrap();
            let rows = rows_per_band(t, h, dh);
            assert!(rows >= 1 && rows <= dh);
            let one_row = {
                let (a, b) = wy.source_range(0, 1);
                b - a
            };
            let mut y = 0;
            while y < dh {
                let y1 = (y + rows).min(dh);
                let (a, b) = wy.source_range(y, y1);
                if one_row <= t {
                    assert!(
                        b - a <= t,
                        "{h}→{dh} タイル {t}: 帯 {y}..{y1} の入力 {}",
                        b - a
                    );
                } else {
                    // 1 行の出力でもタイルの一辺を超える大きな縮小では、帯は 1 行。
                    assert_eq!(rows, 1);
                }
                y = y1;
            }
            // 以前の式で、帯の入力がタイルの一辺を超えた場合を数える（この修正の再現）。
            let old = ((u64::from(t) * u64::from(dh)) / u64::from(h)).max(1) as u32;
            let mut y = 0;
            while y < dh {
                let y1 = (y + old).min(dh);
                let (a, b) = wy.source_range(y, y1);
                if b - a > t && one_row <= t {
                    old_exceeds += 1;
                }
                y = y1;
            }
        }
        assert!(
            old_exceeds > 0,
            "以前の式で超える場合がテストに含まれていない"
        );
        assert_eq!(rows_per_band(1, 10, 3), 1);
        assert_eq!(rows_per_band(0, 10, 3), 1);
    }

    #[test]
    fn column_accumulation_matches_downscale_row() {
        let src: Vec<[f32; 3]> = (0..37)
            .map(|i| [i as f32 * 0.37, (i * i % 11) as f32, 1.0 / (i as f32 + 1.0)])
            .collect();
        for dst in [1u32, 5, 12, 36, 37] {
            let wx = AreaWeights::new(37, dst).unwrap();
            let mut expect = vec![[0.0f32; 3]; dst as usize];
            downscale_row(&wx, &src, &mut expect);
            for step in [1usize, 4, 10, 37] {
                let mut acc = vec![[0.0f32; 3]; dst as usize];
                let mut x = 0;
                while x < src.len() {
                    let end = (x + step).min(src.len());
                    accumulate_columns(&wx, x as u32, &src[x..end], &mut acc);
                    x = end;
                }
                for (a, b) in acc.iter().zip(&expect) {
                    assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits), "{dst} {step}");
                }
            }
        }
    }
}
