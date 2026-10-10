//! 書き出しの GPU 版（docs/04_architecture.md の 2.4 節「書き出しは GPU 版を既定とする。GPU が
//! 使えない環境やエラーの場合は、CPU 版に切り替える」、6.1 節「書き出しは小さなタイルに分けて実行し、
//! 一度に投入するのは 1〜2 タイル分まで」）。
//!
//! # 手順（genzo-pipeline の `FullResolutionJob::export` と同じ順序）
//!
//! | 寸法 | 順序 |
//! |---|---|
//! | 元の寸法 | タイルごとに: ステージ 2〜8（CPU 版。RCD の GPU 版は未実装）→ GPU: ステージ 9〜16 → 17b → 量子化（決定的なディザリング）→ 出力のバッファに書く |
//! | 縮小（`ExportSize::LongEdge`） | 出力の行の帯ごとに: タイルの B3 を GPU で描いて読み出し → 面積平均で縮小（CPU。genzo-pipeline と同じ足し方）→ GPU: 17b → 量子化 |
//!
//! - **投入の量**: タイル 1 つ分のコマンドを投入して完了を待ってから次のタイルを投入する（GPU に投入済みの
//!   処理は常に 1 タイル分以下）。その間に、次のタイルのセンサー処理（CPU）を別のスレッドで進める
//!   （準備を済ませて待つタイルは最大 1 つ。準備中のものを含めて CPU 側の作業領域は 2 タイル分）。
//! - **取り消し**: タイルを処理する前ごとに [`RenderControl`] を確かめる。進捗はタイルごとに知らせる。
//! - **CPU 版への切り替え**: [`export_with_fallback`] は、GPU 版が GPU の側の理由で失敗したら
//!   （[`GpuError::should_fall_back_to_cpu`]）、CPU 版（`Engine::export`）で最初から書き出し直す。
//!   取り消し・設定の誤りは切り替えずに返す。
//! - 優先度（P3）とメモリの予算（genzo-jobs）は呼び出し側（genzo-api）が扱う。

use std::sync::mpsc::sync_channel;

use genzo_model::{DevelopSettings, ExportSettings};
use genzo_pipeline::engine::{
    ExportPixels, ExportedImage, FullResolutionJob, StageBackend, export_icc_profile,
};
use genzo_pipeline::finish::ExportStage;
use genzo_pipeline::finish::output::Dither;
use genzo_pipeline::resample::{AreaWeights, downscale_column_row};
use genzo_pipeline::{
    ColorContract, Engine, ExportOptions, ImageTile, PhotoSource, PipelineError, RenderControl,
    RgbImage, Roi, SourceImage, Stage, StageContext, StageParams,
};
use rayon::prelude::*;

use crate::error::{GpuError, Result};
use crate::preview::gather_mirror;
use crate::renderer::{Chain, GpuRenderer};

/// 書き出しを処理した側。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportBackend {
    /// GPU 版。
    Gpu,
    /// CPU 版。`gpu_error` は GPU 版から切り替えた理由（GPU がない場合は `None`）。
    Cpu {
        /// GPU 版のエラーの表示。
        gpu_error: Option<String>,
    },
}

/// [`export_with_fallback`] の結果。
#[derive(Debug, Clone)]
pub struct GpuExportOutcome {
    /// 書き出した画像（ファイルに書く前のもの）。
    pub image: ExportedImage,
    /// 処理した側。
    pub backend: ExportBackend,
}

/// 書き出す（2.4 節: GPU 版を既定とし、GPU がない・GPU の側のエラーなら CPU 版）。
///
/// 設定の誤り・取り消しなど、CPU 版でも同じように失敗するエラーは切り替えずに返す。
pub fn export_with_fallback(
    gpu: Option<&GpuRenderer>,
    engine: &Engine,
    source: &PhotoSource,
    settings: &DevelopSettings,
    export: &ExportSettings,
    options: &ExportOptions,
    control: &dyn RenderControl,
) -> genzo_pipeline::Result<GpuExportOutcome> {
    export
        .validate()
        .map_err(|e| PipelineError::InvalidArgument(e.to_string()))?;
    let gpu_error = match gpu {
        Some(r) => match r.export(engine, source, settings, export, options, control) {
            Ok(image) => {
                return Ok(GpuExportOutcome {
                    image,
                    backend: ExportBackend::Gpu,
                });
            }
            Err(GpuError::Pipeline(e)) => return Err(e),
            Err(e) => {
                tracing::warn!(error = %e, "GPU 版の書き出しに失敗したので CPU 版で書き出す");
                Some(e.to_string())
            }
        },
        None => None,
    };
    let image = engine.export(source, settings, export, options, control)?;
    Ok(GpuExportOutcome {
        image,
        backend: ExportBackend::Cpu { gpu_error },
    })
}

/// 次のタイルを準備する（CPU のセンサー処理）スレッドのスタックの大きさ。**仮置き**: 呼び出し側の
/// スレッドの代わりに CPU 版の処理を行うので、一般的なメインスレッドと同じ 8MiB にした。
const PREPARE_THREAD_STACK: usize = 8 << 20;

/// 取り消されていれば [`PipelineError::Cancelled`]。
fn check_cancelled(control: &dyn RenderControl) -> Result<()> {
    if control.is_cancelled() {
        Err(PipelineError::Cancelled.into())
    } else {
        Ok(())
    }
}

/// `src`（範囲 `src_roi`）を、範囲 `dst_roi` を覆う画素の列 `dst` に書く（genzo-pipeline の非公開の
/// `blit` と同じ）。
fn blit<T: Copy>(dst: &mut [T], dst_roi: Roi, src_roi: Roi, src: &[T], channels: usize) {
    let (ox, oy) = src_roi
        .offset_in(&dst_roi)
        .expect("書き込む範囲は出力の内側（呼び出し側で確認済み）");
    let dw = dst_roi.width as usize * channels;
    let sw = src_roi.width as usize * channels;
    for (j, row) in src.chunks_exact(sw).enumerate() {
        let start = (oy + j) * dw + ox * channels;
        dst[start..start + sw].copy_from_slice(row);
    }
}

/// 縮小の帯 1 つの出力の行数（genzo-pipeline の非公開の `rows_per_band` と同じ式）。
fn rows_per_band(tile_size: u32, h: u32, dh: u32) -> u32 {
    let r = (u64::from(tile_size.saturating_sub(1)) * u64::from(dh)) / u64::from(h.max(1));
    r.clamp(1, u64::from(dh.max(1))) as u32
}

/// 入力の列 `[x0, x0 + src.len())` の画素を、横方向の面積平均の和 `acc` に足し込む（genzo-pipeline の
/// 非公開の `accumulate_columns` と同じ順序・同じ式）。
fn accumulate_columns(wx: &AreaWeights, x0: u32, src: &[[f32; 3]], acc: &mut [[f32; 3]]) {
    let x1 = x0 + src.len() as u32;
    let (w, dw) = (u64::from(wx.src_len()), u64::from(wx.dst_len()));
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

/// 量子化の出力のバッファ。
enum Buffer {
    U8(Vec<u8>, Dither),
    U16(Vec<u16>, Dither),
}

/// タイル 1 つのステージ 9 の入力（CPU で準備したもの）。
struct Prepared {
    tile: Roi,
    rois: Vec<Roi>,
    input: ImageTile,
    replaced: u64,
}

/// 書き出しの準備（ジョブ・文脈・出力のステージ）。
struct ExportRun<'a> {
    r: &'a GpuRenderer,
    job: FullResolutionJob,
    source: &'a PhotoSource,
    output_stage: &'static ExportStage,
    output_params: StageParams,
}

impl ExportRun<'_> {
    fn ctx(&self) -> StageContext<'_> {
        self.job.context()
    }

    /// タイルのステージ 9 の入力を CPU で作る（RAW はステージ 2〜8 の CPU 版）。
    fn prepare(&self, tile: Roi) -> Result<Prepared> {
        let ctx = self.ctx();
        let rois = self.job.plan().stage_rois(&ctx, tile);
        let (input, replaced) = match (&self.source.image, self.job.sensor_plan()) {
            (SourceImage::Raw(raw), Some(plan)) => plan.render_roi(raw, rois[0])?,
            (SourceImage::Working { image, .. }, None) => {
                let mut t = gather_mirror(image, rois[0])?;
                let n = t.image.sanitize();
                (t, n)
            }
            _ => {
                return Err(PipelineError::InvalidArgument(
                    "書き出しのジョブと入力が対応していない".to_owned(),
                )
                .into());
            }
        };
        Ok(Prepared {
            tile,
            rois,
            input,
            replaced,
        })
    }

    /// ステージ 9〜16（と、`with_output` なら 17b）を GPU で実行した鎖。
    fn render<'r>(&'r self, p: Prepared, with_output: bool) -> Result<Chain<'r>> {
        let ctx = self.ctx();
        let mut chain = Chain::from_cpu(self.r, p.input);
        for ((stage, params), roi) in self.job.plan().stages().iter().zip(&p.rois[1..]) {
            chain.step(*stage, &ctx, params, *roi)?;
        }
        if with_output {
            chain.step(self.output_stage, &ctx, &self.output_params, p.tile)?;
        }
        Ok(chain)
    }

    /// 17b の出力（GPU 上）を量子化してバッファに書く。
    fn quantize_into(
        &self,
        chain: &mut Chain<'_>,
        buffer: &mut Buffer,
        full: Roi,
        roi: Roi,
    ) -> Result<()> {
        let origin = (i64::from(roi.x), i64::from(roi.y));
        let tile = chain.gpu_tile()?;
        chain.flush()?;
        match buffer {
            Buffer::U8(buf, d) => {
                let q = self.r.quantize_tile_u8(&tile, origin, *d)?;
                blit(buf, full, roi, &q, 3);
            }
            Buffer::U16(buf, d) => {
                let q = self.r.quantize_tile_u16(&tile, origin, *d)?;
                blit(buf, full, roi, &q, 3);
            }
        }
        Ok(())
    }
}

/// タイルを順に処理する。`prepare`（CPU）は別のスレッドで 1 タイル先まで進め、`consume`（GPU）は
/// このスレッドで順に呼ぶ。取り消しはタイルの前ごとに確かめる。
fn pipelined<T: Send>(
    tiles: &[Roi],
    control: &dyn RenderControl,
    prepare: impl Fn(Roi) -> Result<T> + Sync,
    mut consume: impl FnMut(usize, T) -> Result<()>,
) -> Result<()> {
    std::thread::scope(|s| {
        let (tx, rx) = sync_channel::<Result<T>>(1);
        let prepare = &prepare;
        let producer = std::thread::Builder::new()
            .name("genzo-gpu.export-prepare".to_owned())
            .stack_size(PREPARE_THREAD_STACK);
        let spawned = producer.spawn_scoped(s, move || {
            for &t in tiles {
                if control.is_cancelled() {
                    break;
                }
                let r = prepare(t);
                let failed = r.is_err();
                if tx.send(r).is_err() || failed {
                    break;
                }
            }
        });
        if let Err(e) = spawned {
            return Err(GpuError::Internal(format!("準備のスレッドを作れない: {e}")));
        }
        for i in 0..tiles.len() {
            check_cancelled(control)?;
            let item = match rx.recv() {
                Ok(item) => item?,
                // 準備のスレッドは、取り消されたときだけ何も送らずに止まる。
                Err(_) => return Err(PipelineError::Cancelled.into()),
            };
            consume(i, item)?;
        }
        Ok(())
    })
}

impl GpuRenderer {
    /// 書き出し（[`GpuRenderer::export`]）で各ステージを処理する側（処理の順。センサーのステージ 2〜8 は
    /// CPU 版、段階 C と 17b は登録表のとおり）。
    ///
    /// genzo-pipeline の `FullResolutionJob::stage_backends` に [`GpuRenderer::registry`] を渡すと、
    /// GPU 版が登録されたセンサーのステージ（簡易処理のプレビュー用）も GPU と表示されるが、書き出しでは
    /// RCD の GPU 版がないのでセンサーのステージはまとめて CPU 版で処理する。
    pub fn export_backends(
        &self,
        job: &FullResolutionJob,
        export: &ExportSettings,
    ) -> Vec<(&'static str, StageBackend)> {
        let version = job.process_version();
        let mut out = Vec::new();
        if let Some(plan) = job.sensor_plan() {
            for s in plan.stages() {
                out.push((s.id(), StageBackend::Cpu));
            }
        }
        for (s, _) in job.plan().stages() {
            out.push((s.id(), self.backend_of(*s, version)));
        }
        let output = ExportStage::for_space(export.color_space);
        out.push((output.id(), self.backend_of(output, version)));
        out
    }

    /// 書き出す（モジュールの doc の手順。`FullResolutionJob::export` の GPU 版）。
    ///
    /// GPU の側の失敗は [`GpuError`] で返す（CPU 版への切り替えは [`export_with_fallback`]）。
    pub fn export(
        &self,
        engine: &Engine,
        source: &PhotoSource,
        settings: &DevelopSettings,
        export: &ExportSettings,
        options: &ExportOptions,
        control: &dyn RenderControl,
    ) -> Result<ExportedImage> {
        export
            .validate()
            .map_err(|e| PipelineError::InvalidArgument(e.to_string()))?;
        self.gpu.check_alive()?;
        let job = engine.full_resolution(source, settings)?;
        let tile_size = options.tile_size.unwrap_or(job.tile_size());
        if tile_size == 0 {
            return Err(PipelineError::InvalidArgument(
                "タイルの一辺は 1 以上が必要です".to_owned(),
            )
            .into());
        }
        let output_stage = ExportStage::for_space(export.color_space);
        let output_params = output_stage
            .params(settings, &job.context())?
            .ok_or_else(|| PipelineError::InvalidArgument("書き出しのステージが無効".to_owned()))?;
        let run = ExportRun {
            r: self,
            job,
            source,
            output_stage,
            output_params,
        };
        let (w, h) = run.job.output_size();
        let (dw, dh) = export.size.fit(w, h);
        let full = Roi::full(dw, dh);
        let len = full.pixel_count() as usize * 3;
        let mut buffer = match export.format.bits_per_channel() {
            8 => Buffer::U8(vec![0; len], options.dither_8bit),
            _ => Buffer::U16(vec![0; len], options.dither_16bit),
        };
        let mut replaced = 0u64;
        if (dw, dh) == (w, h) {
            let tiles = run.job.tiles(full, tile_size)?;
            let total = tiles.len() as u64;
            pipelined(
                &tiles,
                control,
                |t| run.prepare(t),
                |i, p| {
                    let tile = p.tile;
                    replaced += p.replaced;
                    let mut chain = run.render(p, true)?;
                    run.quantize_into(&mut chain, &mut buffer, full, tile)?;
                    let (n, _) = chain.finish()?;
                    replaced += n;
                    control.on_progress(i as u64 + 1, total);
                    Ok(())
                },
            )?;
        } else {
            replaced += self.export_downscaled(&run, &mut buffer, (dw, dh), tile_size, control)?;
        }
        let pixels = match buffer {
            Buffer::U8(v, _) => ExportPixels::Rgb8(v),
            Buffer::U16(v, _) => ExportPixels::Rgb16(v),
        };
        Ok(ExportedImage {
            width: dw,
            height: dh,
            pixels,
            color_space: export.color_space,
            icc: export_icc_profile(export.color_space)?,
            full_resolution_size: (w, h),
            warnings: run.job.warnings().to_vec(),
            unimplemented: run.job.unimplemented().to_vec(),
            non_finite_replaced: replaced,
        })
    }

    /// 縮小する書き出し（フル解像度で処理してから縮小。genzo-pipeline の
    /// `FullResolutionJob::for_each_downscaled_band` と同じ帯・同じ足し方）。
    fn export_downscaled(
        &self,
        run: &ExportRun<'_>,
        buffer: &mut Buffer,
        (dw, dh): (u32, u32),
        tile_size: u32,
        control: &dyn RenderControl,
    ) -> Result<u64> {
        let (w, h) = run.job.output_size();
        let wx = AreaWeights::new(w, dw)?;
        let wy = AreaWeights::new(h, dh)?;
        let full = Roi::full(dw, dh);
        let rows = rows_per_band(tile_size, h, dh);
        // 帯（出力の行 oy0..oy1、入力の行 iy0..iy1）と、帯ごとのタイル（左から、上から）。
        let mut bands = Vec::new();
        let mut tiles = Vec::new();
        let mut oy = 0;
        while oy < dh {
            let oy1 = (oy + rows).min(dh);
            let (iy0, iy1) = wy.source_range(oy, oy1);
            let first = tiles.len();
            let mut y = iy0;
            while y < iy1 {
                let th = tile_size.min(iy1 - y);
                let mut x = 0;
                while x < w {
                    let tw = tile_size.min(w - x);
                    tiles.push(Roi::new(x as i32, y as i32, tw, th));
                    x += tw;
                }
                y += th;
            }
            bands.push((oy, oy1, iy0, iy1, first, tiles.len()));
            oy = oy1;
        }
        let total = tiles.len() as u64;
        let dw_us = dw as usize;
        let mut replaced = 0u64;
        let mut band_index = 0usize;
        let mut hacc: Vec<[f32; 3]> = Vec::new();
        let ctx = run.ctx();
        pipelined(
            &tiles,
            control,
            |t| run.prepare(t),
            |i, p| {
                let (oy0, oy1, iy0, iy1, first, last) = bands[band_index];
                if i == first {
                    hacc = vec![[0.0f32; 3]; (iy1 - iy0) as usize * dw_us];
                }
                let t = p.tile;
                replaced += p.replaced;
                let mut chain = run.render(p, false)?;
                let b3 = chain.cpu_tile()?;
                let (n, _) = chain.finish()?;
                replaced += n;
                let start = (t.y as u32 - iy0) as usize * dw_us;
                let rows = &mut hacc[start..start + t.height as usize * dw_us];
                let x0 = t.x as u32;
                rows.par_chunks_mut(dw_us)
                    .zip(b3.image.data().par_chunks(t.width as usize))
                    .for_each(|(acc, src)| accumulate_columns(&wx, x0, src, acc));
                control.on_progress(i as u64 + 1, total);
                if i + 1 == last {
                    // 帯の縦方向の縮小（CPU）→ 17b と量子化（GPU）。
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
                    let image =
                        RgbImage::from_vec(dw, oy1 - oy0, ColorContract::B3DisplayReferred, data)?;
                    let mut chain = Chain::from_cpu(self, ImageTile::from_image(band, image)?);
                    chain.step(run.output_stage, &ctx, &run.output_params, band)?;
                    run.quantize_into(&mut chain, buffer, full, band)?;
                    let (n, _) = chain.finish()?;
                    replaced += n;
                    band_index += 1;
                }
                Ok(())
            },
        )?;
        Ok(replaced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_per_band_matches_the_pipeline_rule() {
        assert_eq!(rows_per_band(1024, 4672, 1707), 373);
        assert_eq!(rows_per_band(1, 10, 3), 1);
        assert_eq!(rows_per_band(0, 10, 3), 1);
        assert_eq!(rows_per_band(64, 140, 56), 25);
    }

    #[test]
    fn accumulation_in_pieces_equals_one_pass() {
        let src: Vec<[f32; 3]> = (0..37)
            .map(|i| [i as f32 * 0.37, (i * i % 11) as f32, 1.0 / (i as f32 + 1.0)])
            .collect();
        for dst in [1u32, 5, 12, 37] {
            let wx = AreaWeights::new(37, dst).unwrap();
            let mut expect = vec![[0.0f32; 3]; dst as usize];
            genzo_pipeline::resample::downscale_row(&wx, &src, &mut expect);
            let mut acc = vec![[0.0f32; 3]; dst as usize];
            let mut x = 0;
            while x < src.len() {
                let end = (x + 10).min(src.len());
                accumulate_columns(&wx, x as u32, &src[x..end], &mut acc);
                x = end;
            }
            for (a, b) in acc.iter().zip(&expect) {
                assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
            }
        }
    }

    #[test]
    fn blit_writes_rows() {
        let mut dst = vec![0u8; 4 * 3];
        blit(
            &mut dst,
            Roi::new(10, 20, 4, 3),
            Roi::new(11, 21, 2, 2),
            &[1, 2, 3, 4],
            1,
        );
        assert_eq!(dst, [0, 0, 0, 0, 0, 1, 2, 0, 0, 3, 4, 0]);
    }
}
