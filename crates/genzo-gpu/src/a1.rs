//! 段階 A1 の簡易処理（Draft）の GPU 版（docs/04_architecture.md の 2.2 節「WB を変更したとき」:
//! ドラッグ中は 2 × 2 の画素をまとめる簡易処理で計算し、長辺 2560px に縮小する。PERF-01b）。
//!
//! ステージ 2（正規化）→ 3（WB・飽和の切りそろえ）→ 5（2 × 2 の簡易処理）→ 8（色変換）→ 面積平均の
//! 縮小（横・縦）を GPU で実行する。式と範囲のたどり方は genzo-pipeline の
//! `SensorPlan::render_preview` と同じ:
//!
//! - プレビューの出力の行の帯（[`A1_BAND_ROWS`] 行）ごとに、縮小に必要な半分の格子の行
//!   （`AreaWeights::source_range`）を求め、`SensorPlan::stage_rois` でステージ 8 → 5 → 3 → 2 の範囲を
//!   たどる（WB の飽和の近傍の余白、画像の外は鏡映）。帯ごとに投入して完了を待つ（GPU のメモリを帯
//!   1 つ分に抑えるため。結果は帯の分け方によらない）。
//! - 縮小しない場合（半分の格子がプレビューの寸法以下）は、半分の格子の行の帯ごとに同じ処理をする。
//! - RAW は GPU に 1 回だけ転送して保持する（同じ `Arc<RawImage>` の間。WB のドラッグの 1 コマごとに
//!   転送しない）。
//!
//! 最終品質（RCD）の GPU 版は未実装（CPU 版で処理する）。

use std::sync::{Arc, PoisonError};

use genzo_model::{DevelopSettings, RenderQuality};
use genzo_pipeline::resample::AreaWeights;
use genzo_pipeline::{
    A1Image, ColorContract, DemosaicMethod, RgbImage, Roi, SensorOptions, SensorPlan, preview_size,
};
use genzo_raw::RawImage;

use crate::error::{GpuError, Result};
use crate::kernels::{Batch, Dispatch, Kernel, Params, read_count};
use crate::renderer::{
    GpuRenderer, cfa_table, check_raw, pack_raw, record_normalize, record_white_balance,
};

/// 1 回に投入するプレビューの出力の行数（GPU のメモリと投入の回数の釣り合い）。**仮置き**: α7 IV で
/// 帯 1 つの作業領域が数十 MB になる値。結果には影響しない。PoC-3 で実機の時間とメモリを見て決める。
pub const A1_BAND_ROWS: u32 = 128;

/// 面積平均の重みの表（シェーダーの `taps` の並び: 出力ごとの (開始, 数) の後に (入力の番号, 重み)）と、
/// 表の中の (入力の番号, 重み) の開始位置。
pub(crate) fn taps_table(w: &AreaWeights) -> (Vec<u32>, u32) {
    let dst = w.dst_len();
    let mut spans = Vec::with_capacity(dst as usize * 2);
    let mut taps = Vec::new();
    for o in 0..dst {
        let t = w.taps(o);
        spans.push((taps.len() / 2) as u32);
        spans.push(t.len() as u32);
        for &(i, wt) in t {
            taps.push(i);
            taps.push(wt.to_bits());
        }
    }
    let off = spans.len() as u32;
    spans.extend(taps);
    (spans, off)
}

impl GpuRenderer {
    /// 段階 A1 の簡易処理（2 × 2）のプレビュー（長辺 `long_edge` px、B2）を GPU で作る
    /// （genzo-pipeline の `render_a1` の簡易処理と同じ結果を、許容誤差の範囲で返す）。
    ///
    /// `options` の品質・方式の指定は無視し、簡易処理（[`DemosaicMethod::Half2x2`]）で作る。
    pub fn render_a1_draft(
        &self,
        raw: &Arc<RawImage>,
        settings: &DevelopSettings,
        options: &SensorOptions,
        long_edge: u32,
    ) -> Result<A1Image> {
        if long_edge == 0 {
            return Err(GpuError::Pipeline(
                genzo_pipeline::PipelineError::InvalidArgument(
                    "プレビューの長辺は 1 以上が必要です".to_owned(),
                ),
            ));
        }
        let options = SensorOptions {
            quality: RenderQuality::Draft,
            demosaic: Some(DemosaicMethod::Half2x2),
            ..options.clone()
        };
        let plan = SensorPlan::new(raw, settings, &options)?;
        check_raw(&plan, raw)?;
        let (gw, gh) = plan.output_grid();
        let (fw, fh) = preview_size(raw.width, raw.height, long_edge);
        let (pw, ph) = (fw.min(gw), fh.min(gh));
        let words = self.raw_words(raw)?;
        let out = self
            .gpu
            .create_storage("genzo-gpu.a1（画像）", u64::from(pw) * u64::from(ph) * 12)?;
        let downscale = (pw, ph) != (gw, gh);
        let tables = if downscale {
            let (h_table, h_off) = taps_table(&AreaWeights::new(gw, pw)?);
            let (v_table, v_off) = taps_table(&AreaWeights::new(gh, ph)?);
            Some((
                self.gpu.create_storage_init(
                    "genzo-gpu.taps（縮小）",
                    bytemuck::cast_slice(&h_table),
                )?,
                h_off,
                self.gpu.create_storage_init(
                    "genzo-gpu.taps（縮小）",
                    bytemuck::cast_slice(&v_table),
                )?,
                v_off,
                AreaWeights::new(gh, ph)?,
            ))
        } else {
            None
        };
        // 置き換えた NaN・無限大は、帯をまたいで 1 つのカウンターで数え、最後に 1 回だけ読む。
        let stats = self.gpu.create_storage("genzo-gpu.stats", 16)?;
        let rows = if downscale { ph } else { gh };
        let mut y = 0;
        while y < rows {
            let y1 = (y + A1_BAND_ROWS).min(rows);
            // 半分の格子の行の範囲。
            let (iy0, iy1) = match &tables {
                Some((.., wy)) => wy.source_range(y, y1),
                None => (y, y1),
            };
            let roi = Roi::new(0, iy0 as i32, gw, iy1 - iy0);
            let rois = plan.stage_rois(roi)?;
            let mut batch = Batch::with_stats(&self.gpu, &self.kernels, stats.clone())?;
            // ステージ 2: RAW → CFA（rois[1]。ステージ 3 の入力の範囲）。
            let cfa_roi = rois[1];
            let n_cfa = cfa_roi.pixel_count() * 4;
            let values = self.gpu.create_storage("genzo-gpu.cfa（CFA）", n_cfa)?;
            let flags = self
                .gpu
                .create_storage("genzo-gpu.cfa_flags（CFA）", n_cfa)?;
            record_normalize(
                &mut batch, &plan, raw.cfa, raw, &words, &values, &flags, cfa_roi,
            )?;
            // ステージ 3: CFA → CFA（rois[2]）。
            let wb_roi = rois[2];
            let wb = self
                .gpu
                .create_storage("genzo-gpu.cfa（CFA）", wb_roi.pixel_count() * 4)?;
            let table = cfa_table(raw.cfa, Roi::new(0, 0, 1, 1));
            record_white_balance(
                &mut batch, &plan, table, cfa_roi, &values, &flags, &wb, None, wb_roi,
            )?;
            // ステージ 5: 2 × 2（rois[3]）。
            let half_roi = rois[3];
            let half = self
                .gpu
                .create_storage("genzo-gpu.half（画像）", half_roi.pixel_count() * 12)?;
            let mut p = Params::new(Kernel::Half2x2, wb_roi, half_roi);
            p.u32s("CFA", &table);
            batch.dispatch(Dispatch {
                kernel: Kernel::Half2x2,
                params: p.finish()?,
                src: &wb,
                dst: &half,
                aux: None,
                aux_out: None,
                threads: (half_roi.width, half_roi.height),
            })?;
            // ステージ 8: 色変換（rois[4] = roi）。
            let rgb = self
                .gpu
                .create_storage("genzo-gpu.a1_band（画像）", roi.pixel_count() * 12)?;
            let mut p = Params::new(Kernel::ColorMatrix, half_roi, roi);
            p.mat("MAT", plan.camera_to_working().rows());
            batch.dispatch(Dispatch {
                kernel: Kernel::ColorMatrix,
                params: p.finish()?,
                src: &half,
                dst: &rgb,
                aux: None,
                aux_out: None,
                threads: (roi.width, roi.height),
            })?;
            match &tables {
                Some((h_taps, h_off, v_taps, v_off, _)) => {
                    // 横方向: (gw × 行) → (pw × 行)。
                    let h_roi = Roi::new(0, iy0 as i32, pw, iy1 - iy0);
                    let hbuf = self
                        .gpu
                        .create_storage("genzo-gpu.downscale（縮小）", h_roi.pixel_count() * 12)?;
                    let mut p = Params::new(Kernel::DownscaleH, roi, h_roi);
                    p.u32("TAP_OFF", *h_off);
                    batch.dispatch(Dispatch {
                        kernel: Kernel::DownscaleH,
                        params: p.finish()?,
                        src: &rgb,
                        dst: &hbuf,
                        aux: Some(h_taps),
                        aux_out: None,
                        threads: (h_roi.width, h_roi.height),
                    })?;
                    // 縦方向: 出力の行 y..y1。
                    let v_roi = Roi::new(0, y as i32, pw, y1 - y);
                    let vbuf = self
                        .gpu
                        .create_storage("genzo-gpu.downscale（縮小）", v_roi.pixel_count() * 12)?;
                    let mut p = Params::new(Kernel::DownscaleV, h_roi, v_roi);
                    p.u32("TAP_OFF", *v_off);
                    batch.dispatch(Dispatch {
                        kernel: Kernel::DownscaleV,
                        params: p.finish()?,
                        src: &hbuf,
                        dst: &vbuf,
                        aux: Some(v_taps),
                        aux_out: None,
                        threads: (v_roi.width, v_roi.height),
                    })?;
                    batch.copy(
                        &vbuf,
                        0,
                        &out,
                        u64::from(y) * u64::from(pw) * 12,
                        v_roi.pixel_count() * 12,
                    )?;
                }
                None => {
                    batch.copy(
                        &rgb,
                        0,
                        &out,
                        u64::from(y) * u64::from(gw) * 12,
                        roi.pixel_count() * 12,
                    )?;
                }
            }
            // 帯ごとに完了を待つ（GPU のメモリを帯 1 つ分に抑えるため）。
            batch.submit_only()?;
            y = y1;
        }
        let replaced = read_count(&self.gpu, &stats)?;
        let bytes = self
            .gpu
            .read_buffer(&out, u64::from(pw) * u64::from(ph) * 12)?;
        let flat: Vec<f32> = bytemuck::pod_collect_to_vec(&bytes);
        let data = flat.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        let image = RgbImage::from_vec(pw, ph, ColorContract::B2Working, data)?;
        let px = f64::from(DemosaicMethod::Half2x2.sensor_pixels_per_output());
        Ok(A1Image {
            image,
            sensor_size: (raw.width, raw.height),
            sensor_extent: (f64::from(gw) * px, f64::from(gh) * px),
            orientation: plan.frames().orientation,
            quality: RenderQuality::Draft,
            warnings: plan.warnings().to_vec(),
            non_finite_replaced: replaced,
        })
    }

    /// RAW を GPU に置いたもの（同じ `Arc` なら前回のものを使う）。
    fn raw_words(&self, raw: &Arc<RawImage>) -> Result<wgpu::Buffer> {
        {
            let caches = self.caches.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((r, b)) = &caches.raw
                && Arc::ptr_eq(r, raw)
            {
                return Ok(b.clone());
            }
        }
        let b = self
            .gpu
            .create_storage_init("genzo-gpu.raw（RAW）", &pack_raw(raw))?;
        self.caches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .raw = Some((Arc::clone(raw), b.clone()));
        Ok(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taps_table_layout() {
        let w = AreaWeights::new(5, 2).unwrap();
        let (t, off) = taps_table(&w);
        assert_eq!(off, 4);
        // 出力 0 は入力 0, 1, 2（2.5 画素分）。
        let (s0, c0) = (t[0] as usize, t[1] as usize);
        assert_eq!(c0, w.taps(0).len());
        for k in 0..c0 {
            let e = off as usize + 2 * (s0 + k);
            assert_eq!(t[e], w.taps(0)[k].0);
            assert_eq!(f32::from_bits(t[e + 1]), w.taps(0)[k].1);
        }
        let (s1, c1) = (t[2] as usize, t[3] as usize);
        assert_eq!(s1, c0);
        assert_eq!(c1, w.taps(1).len());
    }
}
