//! プレビューの段階 C の GPU 版（docs/04_architecture.md の 2.2 節・6.2 節。PERF-01）。
//!
//! [`GpuRenderer::render_preview`] は genzo-pipeline の `Engine::render_preview_with` と同じ手順で、
//! 段階 C（ステージ 9〜16 と、要求があれば 17a / 17b・17c）を GPU で実行する:
//!
//! 1. 段階 A1: 最終品質はエンジンのキャッシュ（`Engine::a1`。CPU 版の RCD）。WB のドラッグ中の簡易処理
//!    （[`RenderQuality::Draft`]）で RAW なら GPU 版（[`GpuRenderer::render_a1_draft`]）で作り、この型の
//!    キャッシュ（最新 [`DRAFT_CACHE_CAPACITY`] 件。キーはエンジンと同じ `CacheKey`）に入れる。
//! 2. 段階 B: トーンのスライダーが 0 でなければガイドを使う。最終品質はエンジンのキャッシュ
//!    （`Engine::guide`）、GPU 版の簡易処理の A1 からはこの型のキャッシュ（キーはエンジンと同じ
//!    `GuideKey`）。ガイドの計算は CPU 版（`compute_guide`）。
//! 3. 段階 C: ステージ 9〜16 を [`GpuRenderer`] のステージの列として実行する（GPU 版のないステージは
//!    CPU 版。7.1 節）。17a / 17b と 17c も GPU で実行する。
//!
//! 結果の B3・出力は、要求したとき（[`GpuPreviewOptions`]）だけ CPU に読み出す（ビューポートは GPU 上の
//! 結果（[`GpuPreviewRender::b3_gpu`]・[`GpuPreviewRender::output_gpu`]）を使う想定のため）。

use std::sync::{Arc, PoisonError};

use genzo_model::{CacheKey, DevelopSettings, RenderQuality};
use genzo_pipeline::border::mirror_index;
use genzo_pipeline::engine::{GuideKey, StageBackend};
use genzo_pipeline::finish::geometry::GeometryPlan;
use genzo_pipeline::finish::output::Histogram;
use genzo_pipeline::finish::{
    DisplayStage, ExportStage, FinishPlan, GeometryStage, UnimplementedSetting,
    unimplemented_settings,
};
use genzo_pipeline::guide::{GuideSpec, compute_guide};
use genzo_pipeline::{
    A1Image, Engine, Guide, ImageTile, OutputTarget, PhotoSource, PipelineError, PreviewRequest,
    ProcessVersion, RgbImage, Roi, SensorOptions, SensorWarning, SourceImage, Stage, StageContext,
};
use genzo_raw::RawImage;

use crate::error::Result;
use crate::renderer::{Chain, GpuRenderer, GpuTile};

/// GPU 版の簡易処理の A1 と、そこから作ったガイドを保持する件数。**仮置き**: WB のドラッグ中は 1 コマ
/// ごとにキーが変わるので、直前の 2 件だけ持つ（長辺 2560px で 1 件 約 52MB）。
pub const DRAFT_CACHE_CAPACITY: usize = 2;

/// プレビューの GPU 版の設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuPreviewOptions {
    /// B3（ステージ 16 の出力）を CPU に読み出すか。
    pub download_b3: bool,
    /// ステージ 17a / 17b の出力を CPU に読み出すか（`false` ならビューポートが GPU 上の画像
    /// （[`GpuPreviewRender::output_gpu`]）を使う）。
    pub download_output: bool,
    /// 簡易処理（Draft）の A1 を GPU 版で作るか（`false` ならエンジンの CPU 版）。
    pub gpu_draft_a1: bool,
}

impl Default for GpuPreviewOptions {
    fn default() -> Self {
        Self {
            download_b3: true,
            download_output: true,
            gpu_draft_a1: true,
        }
    }
}

/// プレビューの GPU 版の結果（genzo-pipeline の `PreviewRender` に、処理した側の記録を加えたもの）。
#[derive(Debug, Clone)]
pub struct GpuPreviewRender {
    /// ステージ 16 の出力（B3。[`GpuPreviewOptions::download_b3`] のときだけ）。
    pub b3: Option<ImageTile>,
    /// ステージ 16 の出力の GPU 上の画像（読み出さずに使う場合。ビューポートの描画など）。
    pub b3_gpu: GpuTile,
    /// ステージ 17a / 17b の出力（要求し、[`GpuPreviewOptions::download_output`] のとき）。
    pub output: Option<RgbImage>,
    /// ステージ 17a / 17b の出力の GPU 上の画像（要求したとき）。
    pub output_gpu: Option<GpuTile>,
    /// 17c のヒストグラム（要求したとき）。
    pub histogram: Option<Histogram>,
    /// 実際の品質。
    pub quality: RenderQuality,
    /// ジオメトリ。
    pub geometry: GeometryPlan,
    /// 有効だったステージの ID（処理の順。ステージ 9〜16）。
    pub active_stages: Vec<&'static str>,
    /// 実行したステージ（ステージ 9〜16 と 17a / 17b）と処理した側。
    pub backends: Vec<(&'static str, StageBackend)>,
    /// A1 を GPU 版で作ったか（簡易処理のときだけ）。
    pub a1_on_gpu: bool,
    /// ガイドを使ったか。
    pub used_guide: bool,
    /// センサー処理の警告。
    pub warnings: Vec<SensorWarning>,
    /// 値が入っているが適用しなかった項目（v1）。
    pub unimplemented: Vec<UnimplementedSetting>,
    /// 置き換えた NaN・無限大の数（A1 を含む）。
    pub non_finite_replaced: u64,
}

/// GPU 版のキャッシュ（RAW のバッファ、簡易処理の A1 とガイド）。
#[derive(Default)]
pub(crate) struct PreviewCaches {
    pub(crate) raw: Option<(Arc<RawImage>, wgpu::Buffer)>,
    a1_gpu: Vec<(Arc<A1Image>, GpuTile)>,
    a1: Vec<(CacheKey, Arc<A1Image>)>,
    guides: Vec<(GuideKey, Arc<Guide>)>,
    pub(crate) counters: GpuCacheCounters,
}

/// GPU 版のキャッシュのヒット・ミスの回数（テスト・ログ用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GpuCacheCounters {
    /// 簡易処理の A1 のヒット。
    pub draft_a1_hits: u64,
    /// 簡易処理の A1 のミス（GPU で作った回数）。
    pub draft_a1_misses: u64,
    /// 簡易処理の A1 から作ったガイドのヒット。
    pub draft_guide_hits: u64,
    /// 簡易処理の A1 から作ったガイドのミス。
    pub draft_guide_misses: u64,
}

fn lru_get<K: PartialEq, V: Clone>(v: &mut Vec<(K, V)>, key: &K) -> Option<V> {
    let i = v.iter().position(|(k, _)| k == key)?;
    let e = v.remove(i);
    let value = e.1.clone();
    v.push(e);
    Some(value)
}

fn lru_put<K, V>(v: &mut Vec<(K, V)>, key: K, value: V) {
    if v.len() >= DRAFT_CACHE_CAPACITY {
        v.remove(0);
    }
    v.push((key, value));
}

/// 画像 `image` から範囲 `roi`（画像の外にはみ出してよい）を鏡映で取り出す（genzo-pipeline の
/// 非公開の `gather_mirror_from` と同じ値）。
pub(crate) fn gather_mirror(image: &RgbImage, roi: Roi) -> Result<ImageTile> {
    let (w, h) = image.dimensions();
    if roi.is_inside(w, h) {
        return Ok(ImageTile::from_image(roi, image.crop(roi)?)?);
    }
    let xs: Vec<usize> = (0..roi.width)
        .map(|i| mirror_index(i64::from(roi.x) + i64::from(i), w) as usize)
        .collect();
    let mut data = Vec::with_capacity(roi.pixel_count() as usize);
    for j in 0..roi.height {
        let row = image.row(mirror_index(i64::from(roi.y) + i64::from(j), h));
        data.extend(xs.iter().map(|&x| row[x]));
    }
    Ok(ImageTile::from_image(
        roi,
        RgbImage::from_vec(roi.width, roi.height, image.contract(), data)?,
    )?)
}

/// 出力先のステージ（17a / 17b）。
pub(crate) fn output_stage(target: &OutputTarget) -> Box<dyn Stage> {
    match target {
        OutputTarget::Display(t) => Box::new(DisplayStage::new(Arc::clone(t))),
        OutputTarget::Export(s) => Box::new(*ExportStage::for_space(*s)),
        OutputTarget::CacheDisplayP3 => Box::new(ExportStage::CACHE_DISPLAY_P3),
    }
}

/// プレビューを GPU で描く（[`GpuRenderer::render_preview`] を既定の設定で呼ぶ。B3 も読み出す）。
pub fn render_preview_gpu(
    gpu: &GpuRenderer,
    engine: &Engine,
    source: &PhotoSource,
    settings: &DevelopSettings,
    request: &PreviewRequest,
) -> Result<GpuPreviewRender> {
    gpu.render_preview(
        engine,
        source,
        settings,
        request,
        &GpuPreviewOptions::default(),
    )
}

impl GpuRenderer {
    /// GPU 版のキャッシュの回数。
    pub fn cache_counters(&self) -> GpuCacheCounters {
        self.caches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .counters
    }

    /// キャッシュ（RAW のバッファ・簡易処理の A1・ガイド）を捨てる。
    pub fn clear_caches(&self) {
        let mut c = self.caches.lock().unwrap_or_else(PoisonError::into_inner);
        c.raw = None;
        c.a1_gpu.clear();
        c.a1.clear();
        c.guides.clear();
    }

    /// A1 の画像全体を GPU に置いたもの（同じ `Arc` なら前回のものを使う。露光量のドラッグの 1 コマ
    /// ごとに転送しない）。
    fn a1_on_gpu(&self, a1: &Arc<A1Image>) -> Result<GpuTile> {
        {
            let c = self.caches.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((_, t)) = c.a1_gpu.iter().find(|(a, _)| Arc::ptr_eq(a, a1)) {
                return Ok(t.clone());
            }
        }
        let (w, h) = a1.image.dimensions();
        let tile = self.upload(&ImageTile::from_image(Roi::full(w, h), a1.image.clone())?)?;
        let mut c = self.caches.lock().unwrap_or_else(PoisonError::into_inner);
        if c.a1_gpu.len() >= DRAFT_CACHE_CAPACITY {
            c.a1_gpu.remove(0);
        }
        c.a1_gpu.push((Arc::clone(a1), tile.clone()));
        Ok(tile)
    }

    fn draft_a1(
        &self,
        engine: &Engine,
        source: &PhotoSource,
        raw: &Arc<RawImage>,
        settings: &DevelopSettings,
        long_edge: u32,
    ) -> Result<Arc<A1Image>> {
        let key = engine.a1_cache_key(source, settings, RenderQuality::Draft, long_edge);
        {
            let mut c = self.caches.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(a1) = lru_get(&mut c.a1, &key) {
                c.counters.draft_a1_hits += 1;
                return Ok(a1);
            }
        }
        let options = SensorOptions {
            quality: RenderQuality::Draft,
            demosaic: None,
            ..engine.config().sensor.clone()
        };
        let a1 = Arc::new(self.render_a1_draft(raw, settings, &options, long_edge)?);
        let mut c = self.caches.lock().unwrap_or_else(PoisonError::into_inner);
        c.counters.draft_a1_misses += 1;
        lru_put(&mut c.a1, key, Arc::clone(&a1));
        Ok(a1)
    }

    fn draft_guide(
        &self,
        engine: &Engine,
        source: &PhotoSource,
        settings: &DevelopSettings,
        a1: &A1Image,
        long_edge: u32,
    ) -> Result<Arc<Guide>> {
        let key = engine.guide_cache_key(source, settings, RenderQuality::Draft, long_edge)?;
        {
            let mut c = self.caches.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(g) = lru_get(&mut c.guides, &key) {
                c.counters.draft_guide_hits += 1;
                return Ok(g);
            }
        }
        let g = Arc::new(compute_guide(a1, &GuideSpec::from_settings(settings)?)?);
        let mut c = self.caches.lock().unwrap_or_else(PoisonError::into_inner);
        c.counters.draft_guide_misses += 1;
        lru_put(&mut c.guides, key, Arc::clone(&g));
        Ok(g)
    }

    /// プレビューを GPU で描く（モジュールの doc の手順。`Engine::render_preview_with` の GPU 版）。
    ///
    /// 失敗は [`GpuError`](crate::GpuError) で返す。CPU 版で描き直すか（6.3 節）は呼び出し側が決める
    /// （[`GpuError::should_fall_back_to_cpu`](crate::GpuError::should_fall_back_to_cpu)）。
    pub fn render_preview(
        &self,
        engine: &Engine,
        source: &PhotoSource,
        settings: &DevelopSettings,
        request: &PreviewRequest,
        options: &GpuPreviewOptions,
    ) -> Result<GpuPreviewRender> {
        ProcessVersion::from_u32(settings.process_version)?;
        if request.histogram && request.output.is_none() {
            return Err(PipelineError::InvalidArgument(
                "ヒストグラムには出力先（17a / 17b）の指定が必要です".to_owned(),
            )
            .into());
        }
        if request.long_edge == 0 {
            return Err(PipelineError::InvalidArgument(
                "プレビューの長辺は 1 以上が必要です".to_owned(),
            )
            .into());
        }
        let quality = source.effective_quality(request.quality);
        let draft_raw = match &source.image {
            SourceImage::Raw(raw) if options.gpu_draft_a1 && quality == RenderQuality::Draft => {
                Some(raw)
            }
            _ => None,
        };
        let a1 = match draft_raw {
            Some(raw) => self.draft_a1(engine, source, raw, settings, request.long_edge)?,
            None => engine.a1(source, settings, quality, request.long_edge)?,
        };
        let base: StageContext<'static> = a1.finish_context(settings)?;
        let (plan, guide) = match FinishPlan::new(settings, &base) {
            Ok(plan) => (plan, None),
            Err(PipelineError::GuideMismatch { .. }) => {
                let g = match draft_raw {
                    Some(_) => {
                        self.draft_guide(engine, source, settings, &a1, request.long_edge)?
                    }
                    None => engine.guide(source, settings, quality, request.long_edge)?,
                };
                let plan = FinishPlan::new(settings, &base.with_guide(&g))?;
                (plan, Some(g))
            }
            Err(e) => return Err(e.into()),
        };
        let ctx = match &guide {
            Some(g) => base.with_guide(g),
            None => base,
        };
        let (w, h) = plan.output_size();
        let full = Roi::full(w, h);
        let rois = plan.stage_rois(&ctx, full);
        // 最初のステージを GPU で処理するなら、GPU に置いた A1 の全体から始める（ステージ 9 は端の外を
        // 自分で鏡映する。画素ごとのステージなら rois[0] は A1 の全体）。
        let (gw, gh) = a1.image.dimensions();
        let first_on_gpu = plan
            .stages()
            .first()
            .is_some_and(|(s, _)| self.backend_of(*s, ctx.process_version) == StageBackend::Gpu);
        let mut chain = if first_on_gpu
            && (rois[0] == Roi::full(gw, gh) || plan.stages()[0].0.id() == GeometryStage::ID)
        {
            Chain::from_gpu(self, self.a1_on_gpu(&a1)?)
        } else {
            Chain::from_cpu(self, gather_mirror(&a1.image, rois[0])?)
        };
        for ((stage, params), roi) in plan.stages().iter().zip(&rois[1..]) {
            chain.step(*stage, &ctx, params, *roi)?;
        }
        // B3 の置き場所（出力のステージは別のバッファに書くので、後から読み出せる）。
        // B3 の GPU 上の画像（出力のステージは別のバッファに書くので、後から読み出せる）。
        let b3_gpu = chain.gpu_tile()?;
        let mut histogram = None;
        let mut output = None;
        let mut output_gpu = None;
        if let Some(target) = &request.output {
            let stage = output_stage(target);
            let params = stage.params(settings, &ctx)?.ok_or_else(|| {
                PipelineError::InvalidArgument(format!(
                    "出力のステージ {} が無効になっています",
                    stage.id()
                ))
            })?;
            chain.step(stage.as_ref(), &ctx, &params, full)?;
            let tile = chain.gpu_tile()?;
            chain.flush()?;
            if request.histogram {
                histogram = Some(self.histogram_tile(&tile, target.luma_coefficients())?);
            }
            if options.download_output {
                output = Some(self.download(&tile)?.image);
            }
            output_gpu = Some(tile);
        }
        let (replaced, backends) = chain.finish()?;
        let b3 = if options.download_b3 {
            Some(self.download(&b3_gpu)?)
        } else {
            None
        };
        Ok(GpuPreviewRender {
            b3,
            b3_gpu,
            output,
            output_gpu,
            histogram,
            quality,
            geometry: plan.geometry().clone(),
            active_stages: plan.active_stage_ids(),
            backends,
            a1_on_gpu: draft_raw.is_some(),
            used_guide: guide.is_some(),
            warnings: a1.warnings.clone(),
            unimplemented: unimplemented_settings(settings),
            non_finite_replaced: replaced + a1.non_finite_replaced,
        })
    }
}

#[cfg(test)]
mod tests {
    use genzo_pipeline::ColorContract;

    use super::*;

    #[test]
    fn gather_mirror_reflects_outside() {
        let img = RgbImage::from_fn(4, 3, ColorContract::B2Working, |x, y| {
            [x as f32, y as f32, 0.0]
        })
        .unwrap();
        let t = gather_mirror(&img, Roi::new(-2, -1, 8, 5)).unwrap();
        assert_eq!(t.at(-2, -1), Some([2.0, 1.0, 0.0]));
        assert_eq!(t.at(5, 3), Some([1.0, 1.0, 0.0]));
        assert_eq!(t.at(1, 1), Some([1.0, 1.0, 0.0]));
        let inner = gather_mirror(&img, Roi::new(1, 1, 2, 2)).unwrap();
        assert_eq!(inner.image.pixel(0, 0), Some([1.0, 1.0, 0.0]));
    }

    #[test]
    fn lru_keeps_the_latest() {
        let mut v: Vec<(u32, u32)> = Vec::new();
        lru_put(&mut v, 1, 10);
        lru_put(&mut v, 2, 20);
        assert_eq!(lru_get(&mut v, &1), Some(10));
        lru_put(&mut v, 3, 30);
        assert_eq!(lru_get(&mut v, &2), None, "最も古いものを捨てる");
        assert_eq!(lru_get(&mut v, &1), Some(10));
        assert_eq!(lru_get(&mut v, &3), Some(30));
    }
}
