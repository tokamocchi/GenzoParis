//! GPU 版の実行（ステージ単位の実行、CPU 版との混在、17c のヒストグラム、17b の量子化、センサーの
//! ステージ）。docs/04_architecture.md の 2.3 節・7.1 節。
//!
//! # ステージの実行と混在（7.1 節）
//!
//! [`GpuRenderer::run_stages`] は、ステージの列を順に実行する。各ステージは [`Stage::gpu`]（ステージの
//! ID で登録表を引く）で GPU 版を探し、あれば GPU で、なければ CPU 版（[`run_stage_cpu`]）で処理する
//! （混在できる）。GPU 版が続く間はコマンドを 1 回の投入にまとめ、CPU 版のステージの前で投入して結果を
//! 読み出す。GPU 版がパラメータを扱えない場合（[`GpuError::Unsupported`]。行列のわからないモニターの
//! プロファイルなど）も、そのステージだけ CPU 版で処理する。
//!
//! # CPU 版と同じにしていること
//!
//! - 入力の色の契約・範囲の確認（[`run_stage_cpu`] と同じ条件。違えば同じ [`PipelineError`]）。
//! - 各ステージの出力の NaN・無限大を 0 に置き換え、数を返す（GPU では原子的なカウンターで数える）。
//! - 画素ごとの式と演算の順序（`shaders/` の各ファイル）。
//!
//! # 1 回に投入する量（6.1 節）
//!
//! この型のメソッドは、呼び出し 1 回の処理（1 タイル、またはプレビュー 1 枚）を投入して完了まで待つ。
//! 書き出し（[`crate::export`]）は 1 タイルずつ呼ぶので、GPU に投入済みの処理は常に 1 タイル分以下に
//! なる（P0 の要求はタイルの合間に割り込める）。

use std::sync::{Mutex, PoisonError};

use genzo_color::{DisplayProfile, DisplayProfileFallbackReason};
use genzo_pipeline::engine::StageBackend;
use genzo_pipeline::finish::output::{ClipCounts, Dither, Histogram};
use genzo_pipeline::sensor::wb::SaturationClip;
use genzo_pipeline::sensor::{SensorData, SensorPlan, SensorStage};
use genzo_pipeline::{
    ColorContract, ImageTile, PipelineError, ProcessVersion, RgbImage, Roi, Stage, StageContext,
    StageParams, run_stage_cpu,
};
use genzo_raw::{CfaPattern, RawImage};

use crate::context::GpuContext;
use crate::error::{GpuError, Result};
use crate::kernels::{Batch, Dispatch, Kernel, KernelCache, Params};
use crate::stages::{DisplayMatrices, GpuStageKind, GpuStageRegistry, kind_of, prepare_rgb};

/// GPU 上の RGB の画像（f32 の R, G, B の並び。行優先）と、その範囲・色の契約。
///
/// ビューポートの描画（genzo-view）は、ダウンロードせずにこのバッファを使える。
#[derive(Debug, Clone)]
pub struct GpuTile {
    buffer: wgpu::Buffer,
    /// 範囲（そのステージの格子）。
    pub roi: Roi,
    /// 色の契約。
    pub contract: ColorContract,
}

impl GpuTile {
    /// wgpu のバッファ（ストレージバッファ。`roi` の画素数 × 3 個の f32）。
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// 値の数（画素数 × 3）。
    pub fn value_count(&self) -> u64 {
        self.roi.pixel_count() * 3
    }
}

/// ステージの列の 1 段（[`GpuRenderer::run_stages`]）。
#[derive(Clone, Copy)]
pub struct StageStep<'a> {
    /// ステージ。
    pub stage: &'a dyn Stage,
    /// パラメータ（[`Stage::params`] の結果）。
    pub params: &'a StageParams,
    /// 出力の範囲。
    pub output_roi: Roi,
}

impl std::fmt::Debug for StageStep<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageStep")
            .field("stage", &self.stage.id())
            .field("output_roi", &self.output_roi)
            .finish()
    }
}

/// ステージの列の結果。
#[derive(Debug, Clone)]
pub struct ChainOutput {
    /// 最後のステージの出力（CPU に読み出したもの）。
    pub tile: ImageTile,
    /// 置き換えた NaN・無限大の数（CPU 版と GPU 版の合計）。
    pub non_finite_replaced: u64,
    /// 実行したステージと、処理した側（実行の順）。
    pub backends: Vec<(&'static str, StageBackend)>,
}

/// 現在の画像（CPU か GPU のどちらかにある）。
enum Current {
    Cpu(ImageTile),
    Gpu(GpuTile),
}

impl Current {
    fn roi(&self) -> Roi {
        match self {
            Current::Cpu(t) => t.roi,
            Current::Gpu(t) => t.roi,
        }
    }

    fn contract(&self) -> ColorContract {
        match self {
            Current::Cpu(t) => t.contract(),
            Current::Gpu(t) => t.contract,
        }
    }
}

/// ステージの列を実行する途中の状態（[`GpuRenderer::run_stages`] とプレビュー・書き出しで使う）。
pub(crate) struct Chain<'r> {
    r: &'r GpuRenderer,
    batch: Option<Batch<'r>>,
    current: Current,
    replaced: u64,
    backends: Vec<(&'static str, StageBackend)>,
}

impl<'r> Chain<'r> {
    pub(crate) fn from_cpu(r: &'r GpuRenderer, input: ImageTile) -> Self {
        Self {
            r,
            batch: None,
            current: Current::Cpu(input),
            replaced: 0,
            backends: Vec::new(),
        }
    }

    /// GPU 上の画像から始める。
    pub(crate) fn from_gpu(r: &'r GpuRenderer, input: GpuTile) -> Self {
        Self {
            r,
            batch: None,
            current: Current::Gpu(input),
            replaced: 0,
            backends: Vec::new(),
        }
    }

    fn batch(&mut self) -> Result<&mut Batch<'r>> {
        if self.batch.is_none() {
            self.batch = Some(Batch::new(&self.r.gpu, &self.r.kernels)?);
        }
        Ok(self.batch.as_mut().expect("作った直後"))
    }

    /// まとめたコマンドを投入して完了まで待つ。
    pub(crate) fn flush(&mut self) -> Result<()> {
        if let Some(b) = self.batch.take() {
            self.replaced += b.submit()?;
        }
        Ok(())
    }

    /// 現在の画像を GPU に置いたもの（CPU にあればアップロードする）。
    pub(crate) fn gpu_tile(&mut self) -> Result<GpuTile> {
        if let Current::Cpu(t) = &self.current {
            let g = self.r.upload(t)?;
            self.current = Current::Gpu(g);
        }
        match &self.current {
            Current::Gpu(g) => Ok(g.clone()),
            Current::Cpu(_) => unreachable!("直前にアップロードした"),
        }
    }

    /// 現在の画像を CPU に読み出したもの（投入していないコマンドがあれば投入する）。
    pub(crate) fn cpu_tile(&mut self) -> Result<ImageTile> {
        if let Current::Gpu(g) = &self.current {
            let g = g.clone();
            self.flush()?;
            let t = self.r.download(&g)?;
            self.current = Current::Cpu(t);
        }
        match &self.current {
            Current::Cpu(t) => Ok(t.clone()),
            Current::Gpu(_) => unreachable!("直前に読み出した"),
        }
    }

    /// 1 段を実行する（GPU 版があれば GPU、なければ CPU 版）。
    pub(crate) fn step(
        &mut self,
        stage: &dyn Stage,
        ctx: &StageContext<'_>,
        params: &StageParams,
        output_roi: Roi,
    ) -> Result<StageBackend> {
        // run_stage_cpu と同じ確認。
        if self.current.contract() != stage.input_contract() {
            return Err(PipelineError::ContractMismatch {
                stage: stage.id(),
                expected: stage.input_contract(),
                found: self.current.contract(),
            }
            .into());
        }
        let need = stage.input_roi(output_roi, params, ctx);
        let in_roi = self.current.roi();
        let kind = kind_of(stage.gpu(&self.r.registry), ctx.process_version);
        // ステージ 9 の GPU 版は、入力の格子の画像全体を渡せば、端の外を CPU 版の鏡映
        // （FinishPlan::render_roi の gather_mirror）と同じ値で自分で引く（geometry.wgsl の fetch）。
        let grid = ctx.sensor_grid;
        let whole_grid = in_roi == Roi::full(grid.width, grid.height);
        let mirrors_itself = kind == Some(GpuStageKind::Geometry) && whole_grid;
        if !in_roi.contains_roi(&need) && !mirrors_itself {
            return Err(PipelineError::InvalidRoi {
                roi: in_roi,
                reason: "入力のタイルがステージの必要な範囲を覆っていない",
            }
            .into());
        }
        if output_roi.is_empty() {
            return Err(PipelineError::InvalidRoi {
                roi: output_roi,
                reason: "出力の範囲が空",
            }
            .into());
        }
        let prepared = match kind {
            Some(kind) if !kind.is_sensor() => {
                let displays = self.r.displays();
                match prepare_rgb(kind, stage, ctx, params, in_roi, output_roi, &displays) {
                    Ok(p) => Some(p),
                    Err(GpuError::Unsupported { stage, reason }) => {
                        tracing::debug!(stage, reason, "GPU 版で扱えないので CPU 版で処理する");
                        None
                    }
                    Err(e) => return Err(e),
                }
            }
            _ => None,
        };
        let backend = match prepared {
            Some(p) => {
                let input = self.gpu_tile()?;
                let out = self.r.alloc(output_roi, stage.output_contract())?;
                let aux = match &p.aux {
                    Some(bytes) => Some(self.r.gpu.create_storage_init("genzo-gpu.aux", bytes)?),
                    None => None,
                };
                self.batch()?.dispatch(Dispatch {
                    kernel: p.kernel,
                    params: p.params,
                    src: &input.buffer,
                    dst: &out.buffer,
                    aux: aux.as_ref(),
                    aux_out: None,
                    threads: (output_roi.width, output_roi.height),
                })?;
                self.current = Current::Gpu(out);
                StageBackend::Gpu
            }
            None => {
                let mut input = self.cpu_tile()?;
                if !input.roi.contains_roi(&need) {
                    // 画像全体を渡されたステージ 9 を CPU 版で処理する場合は、鏡映で広げる。
                    input = input.gather_mirror(need, grid.width, grid.height)?;
                }
                let (out, n) = run_stage_cpu(stage, ctx, &input, output_roi, params)?;
                self.replaced += n;
                self.current = Current::Cpu(out);
                StageBackend::Cpu
            }
        };
        self.backends.push((stage.id(), backend));
        Ok(backend)
    }

    /// 終える（投入していないコマンドを投入する）。置き換えた NaN・無限大の数と、処理した側の記録を返す。
    pub(crate) fn finish(mut self) -> Result<(u64, Vec<(&'static str, StageBackend)>)> {
        self.flush()?;
        Ok((self.replaced, self.backends))
    }
}

/// GPU 版の実行（コンパイルしたカーネル・登録表・キャッシュを持つ）。スレッド間で共有できる。
pub struct GpuRenderer {
    pub(crate) gpu: GpuContext,
    pub(crate) kernels: KernelCache,
    pub(crate) registry: GpuStageRegistry,
    displays: Mutex<Vec<DisplayMatrices>>,
    pub(crate) caches: Mutex<crate::preview::PreviewCaches>,
}

impl std::fmt::Debug for GpuRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuRenderer")
            .field("gpu", &self.gpu)
            .field("registry", &self.registry.ids())
            .finish_non_exhaustive()
    }
}

impl GpuRenderer {
    /// コンテキストから作る（登録表は [`GpuStageRegistry::builtin`]。カーネルは最初に使うときに
    /// コンパイルする）。
    pub fn new(gpu: GpuContext) -> Result<Self> {
        let kernels = KernelCache::new(&gpu)?;
        let r = Self {
            gpu,
            kernels,
            registry: GpuStageRegistry::builtin(),
            displays: Mutex::new(Vec::new()),
            caches: Mutex::new(crate::preview::PreviewCaches::default()),
        };
        // モニターのプロファイルが取得できないとき（sRGB とみなす。genzo-pipeline の
        // DisplayTransform::assumed_srgb）の色域は、lcms2 の sRGB のプロファイルの行列から求めるので、
        // 標準の sRGB の行列とは丸めが違う。最初から登録しておく。
        match DisplayProfile::assumed_srgb(DisplayProfileFallbackReason::NotAvailable) {
            Ok(p) => {
                r.register_display_profile(&p)?;
            }
            Err(e) => {
                tracing::warn!(error = %e, "sRGB のプロファイルを作れない（17a は CPU 版で処理する）")
            }
        }
        Ok(r)
    }

    /// 環境変数の設定で作る（[`GpuContext::from_env`]）。アダプターがなければ `Ok(None)`。
    pub fn from_env() -> Result<Option<Self>> {
        match GpuContext::from_env()? {
            Some(gpu) => Ok(Some(Self::new(gpu)?)),
            None => Ok(None),
        }
    }

    /// 登録表を変えたもの（特定のステージを CPU 版で処理する場合など）。
    pub fn with_registry(mut self, registry: GpuStageRegistry) -> Self {
        self.registry = registry;
        self
    }

    /// コンテキスト。
    pub fn context(&self) -> &GpuContext {
        &self.gpu
    }

    /// 登録表（genzo-pipeline の [`genzo_pipeline::GpuStageLookup`]。
    /// `FullResolutionJob::stage_backends` などに渡せる）。
    pub fn registry(&self) -> &GpuStageRegistry {
        &self.registry
    }

    /// すべてのカーネルをコンパイルする（シェーダーの誤りを早く見つける・計測の前の準備）。
    pub fn compile_all(&self) -> Result<()> {
        self.kernels.compile_all(&self.gpu)
    }

    /// ステージを処理する側（GPU 版があり、この処理バージョンに対応していれば GPU）。
    pub fn backend_of(&self, stage: &dyn Stage, version: ProcessVersion) -> StageBackend {
        match kind_of(stage.gpu(&self.registry), version) {
            Some(k) if !k.is_sensor() => StageBackend::Gpu,
            _ => StageBackend::Cpu,
        }
    }

    /// モニターのプロファイルを登録する（ステージ 17a の色域の圧縮の行列を GPU 版に渡すため）。
    ///
    /// 標準の色空間（sRGB・Display P3・Adobe RGB・BT.2020）と同じ色域なら登録しなくても GPU で
    /// 処理できる。行列で表せないプロファイル（LUT 型など）は登録できず（`false`）、17a はそのプロファイル
    /// で sRGB とみなした色域（genzo-pipeline の `DisplayTransform::new` の暫定）になるので、標準の行列で
    /// 処理できる。
    pub fn register_display_profile(&self, profile: &DisplayProfile) -> Result<bool> {
        let Some(gamut) = profile.gamut() else {
            return Ok(false);
        };
        // 行列は genzo-color の DisplayGamut が CPU 版で使う f32 の値そのもの。
        let entry = DisplayMatrices::from_gamut(gamut);
        let mut d = self.displays.lock().unwrap_or_else(PoisonError::into_inner);
        if !d.iter().any(|x| x.gamut == gamut) {
            d.push(entry);
        }
        Ok(true)
    }

    fn displays(&self) -> Vec<DisplayMatrices> {
        self.displays
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    // -----------------------------------------------------------------------
    // 画像の転送
    // -----------------------------------------------------------------------

    /// 0 で埋めた GPU 上の画像を作る。
    pub(crate) fn alloc(&self, roi: Roi, contract: ColorContract) -> Result<GpuTile> {
        let buffer = self
            .gpu
            .create_storage("genzo-gpu.image（画像）", roi.pixel_count() * 12)?;
        Ok(GpuTile {
            buffer,
            roi,
            contract,
        })
    }

    /// CPU の画像を GPU に転送する。
    pub fn upload(&self, tile: &ImageTile) -> Result<GpuTile> {
        let buffer = self.gpu.create_storage_init(
            "genzo-gpu.image（画像）",
            bytemuck::cast_slice(tile.image.as_flat()),
        )?;
        Ok(GpuTile {
            buffer,
            roi: tile.roi,
            contract: tile.contract(),
        })
    }

    /// GPU の画像を CPU に読み出す。
    pub fn download(&self, tile: &GpuTile) -> Result<ImageTile> {
        let bytes = self.gpu.read_buffer(&tile.buffer, tile.value_count() * 4)?;
        let flat: Vec<f32> = bytemuck::pod_collect_to_vec(&bytes);
        let data = flat
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .collect::<Vec<_>>();
        let image = RgbImage::from_vec(tile.roi.width, tile.roi.height, tile.contract, data)?;
        Ok(ImageTile::from_image(tile.roi, image)?)
    }

    // -----------------------------------------------------------------------
    // RGB のステージ
    // -----------------------------------------------------------------------

    /// ステージ 1 つを GPU 版で実行する（[`run_stage_cpu`] の GPU 版。入力の確認・出力の NaN・無限大の
    /// 置き換えも同じ）。GPU 版がない・パラメータを扱えない場合は [`GpuError::Unsupported`]。
    pub fn run_stage(
        &self,
        stage: &dyn Stage,
        ctx: &StageContext<'_>,
        input: &ImageTile,
        output_roi: Roi,
        params: &StageParams,
    ) -> Result<(ImageTile, u64)> {
        if self.backend_of(stage, ctx.process_version) == StageBackend::Cpu {
            return Err(GpuError::Unsupported {
                stage: stage.id(),
                reason: "GPU 版が登録されていない".to_owned(),
            });
        }
        let mut chain = Chain::from_cpu(self, input.clone());
        if chain.step(stage, ctx, params, output_roi)? == StageBackend::Cpu {
            return Err(GpuError::Unsupported {
                stage: stage.id(),
                reason: "GPU 版がこのパラメータを扱えない".to_owned(),
            });
        }
        let tile = chain.cpu_tile()?;
        let (replaced, _) = chain.finish()?;
        Ok((tile, replaced))
    }

    /// ステージの列を順に実行する（GPU 版のないステージは CPU 版で処理する。モジュールの doc）。
    pub fn run_stages(
        &self,
        steps: &[StageStep<'_>],
        ctx: &StageContext<'_>,
        input: ImageTile,
    ) -> Result<ChainOutput> {
        let mut chain = Chain::from_cpu(self, input);
        for s in steps {
            chain.step(s.stage, ctx, s.params, s.output_roi)?;
        }
        let tile = chain.cpu_tile()?;
        let (non_finite_replaced, backends) = chain.finish()?;
        Ok(ChainOutput {
            tile,
            non_finite_replaced,
            backends,
        })
    }

    // -----------------------------------------------------------------------
    // 17c ヒストグラム・17b 量子化
    // -----------------------------------------------------------------------

    /// 符号化した画像（B4a・B4b・B5）のヒストグラム（genzo-pipeline の `Histogram::compute` の GPU 版。
    /// 整数の原子的な加算なので、入力が同じなら CPU 版と同じ度数になる）。
    pub fn histogram(&self, image: &RgbImage, luma: [f32; 3]) -> Result<Histogram> {
        check_encoded(image.contract())?;
        let tile = self.upload(&ImageTile::from_image(
            Roi::full(image.width(), image.height()),
            image.clone(),
        )?)?;
        self.histogram_tile(&tile, luma)
    }

    /// GPU 上の符号化した画像のヒストグラム。
    pub fn histogram_tile(&self, tile: &GpuTile, luma: [f32; 3]) -> Result<Histogram> {
        check_encoded(tile.contract)?;
        const LEN: usize = 1034;
        let hist = self
            .gpu
            .create_storage("genzo-gpu.histogram（ヒストグラム）", LEN as u64 * 4)?;
        let roi = Roi::full(tile.roi.width, tile.roi.height);
        let mut p = Params::new(Kernel::Histogram, roi, roi);
        p.f32s("HLUMA", &luma);
        let mut batch = Batch::new(&self.gpu, &self.kernels)?;
        batch.dispatch(Dispatch {
            kernel: Kernel::Histogram,
            params: p.finish()?,
            src: &tile.buffer,
            dst: &hist,
            aux: None,
            aux_out: None,
            threads: (roi.width, roi.height),
        })?;
        batch.submit()?;
        let bytes = self.gpu.read_buffer(&hist, LEN as u64 * 4)?;
        let w: Vec<u32> = bytemuck::pod_collect_to_vec(&bytes);
        let mut h = Histogram::default();
        for k in 0..256 {
            h.red[k] = u64::from(w[k]);
            h.green[k] = u64::from(w[256 + k]);
            h.blue[k] = u64::from(w[512 + k]);
            h.luma[k] = u64::from(w[768 + k]);
        }
        let clip = |o: usize| ClipCounts {
            red: u64::from(w[o]),
            green: u64::from(w[o + 1]),
            blue: u64::from(w[o + 2]),
            any: u64::from(w[o + 3]),
            all: u64::from(w[o + 4]),
        };
        h.clipped_high = clip(1024);
        h.clipped_low = clip(1029);
        h.pixel_count = roi.pixel_count();
        Ok(h)
    }

    /// 8bit に量子化する（genzo-pipeline の `quantize_u8` の GPU 版。並び・ディザリングのノイズは同じ）。
    pub fn quantize_u8(
        &self,
        image: &RgbImage,
        origin: (i64, i64),
        dither: Dither,
    ) -> Result<Vec<u8>> {
        let tile = self.upload(&ImageTile::from_image(
            Roi::full(image.width(), image.height()),
            image.clone(),
        )?)?;
        self.quantize_tile_u8(&tile, origin, dither)
    }

    /// 16bit に量子化する（genzo-pipeline の `quantize_u16` の GPU 版）。
    pub fn quantize_u16(
        &self,
        image: &RgbImage,
        origin: (i64, i64),
        dither: Dither,
    ) -> Result<Vec<u16>> {
        let tile = self.upload(&ImageTile::from_image(
            Roi::full(image.width(), image.height()),
            image.clone(),
        )?)?;
        self.quantize_tile_u16(&tile, origin, dither)
    }

    /// GPU 上の画像を 8bit に量子化する。
    pub fn quantize_tile_u8(
        &self,
        tile: &GpuTile,
        origin: (i64, i64),
        dither: Dither,
    ) -> Result<Vec<u8>> {
        let n = (tile.value_count()) as usize;
        let bytes = self.quantize_words(tile, origin, dither, 8)?;
        Ok(bytes[..n].to_vec())
    }

    /// GPU 上の画像を 16bit に量子化する。
    pub fn quantize_tile_u16(
        &self,
        tile: &GpuTile,
        origin: (i64, i64),
        dither: Dither,
    ) -> Result<Vec<u16>> {
        let n = (tile.value_count()) as usize;
        let bytes = self.quantize_words(tile, origin, dither, 16)?;
        Ok(bytes
            .chunks_exact(2)
            .take(n)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect())
    }

    fn quantize_words(
        &self,
        tile: &GpuTile,
        origin: (i64, i64),
        dither: Dither,
        bits: u32,
    ) -> Result<Vec<u8>> {
        check_encoded(tile.contract)?;
        let to_i32 = |v: i64| {
            i32::try_from(v).map_err(|_| GpuError::Unsupported {
                stage: "output.quantize",
                reason: format!("原点 {v} が i32 の範囲の外"),
            })
        };
        let values = tile.value_count();
        let per_thread = if bits == 16 { 6 } else { 12 };
        let threads = values.div_ceil(per_thread).max(1);
        let groups = threads.div_ceil(64);
        let max = u64::from(self.gpu.limits().max_compute_workgroups_per_dimension);
        let gx = groups.min(max);
        let stride = gx * 64;
        let rows = threads.div_ceil(stride);
        let words = threads * 3;
        let out = self
            .gpu
            .create_storage("genzo-gpu.quantize（量子化）", words * 4)?;
        let roi = Roi::full(tile.roi.width, tile.roi.height);
        let (seed, on) = match dither {
            Dither::None => (0, 0),
            Dither::Triangular { seed } => (seed, 1),
        };
        let mut p = Params::new(Kernel::Quantize, roi, roi);
        p.u32("QBITS", bits)
            .i32("QORIGIN_X", to_i32(origin.0)?)
            .i32("QORIGIN_Y", to_i32(origin.1)?)
            .u32("QDITHER", on)
            .u32("QSEED", seed)
            .u32("QSTRIDE", stride as u32);
        let mut batch = Batch::new(&self.gpu, &self.kernels)?;
        batch.dispatch(Dispatch {
            kernel: Kernel::Quantize,
            params: p.finish()?,
            src: &tile.buffer,
            dst: &out,
            aux: None,
            aux_out: None,
            threads: (stride as u32, rows as u32),
        })?;
        batch.submit()?;
        self.gpu.read_buffer(&out, words * 4)
    }

    // -----------------------------------------------------------------------
    // センサーのステージ
    // -----------------------------------------------------------------------

    /// センサーのステージ 1 つを GPU 版で実行する（genzo-pipeline の `run_sensor_stage` の GPU 版）。
    /// GPU 版がなければ [`GpuError::Unsupported`]。
    pub fn run_sensor_stage<'d>(
        &self,
        stage: &dyn SensorStage,
        plan: &SensorPlan,
        input: SensorData<'d>,
        output_roi: Roi,
    ) -> Result<(SensorData<'d>, u64)> {
        let Some(kind) =
            kind_of(stage.gpu(&self.registry), plan.process_version()).filter(|k| k.is_sensor())
        else {
            return Err(GpuError::Unsupported {
                stage: stage.id(),
                reason: "GPU 版が登録されていない".to_owned(),
            });
        };
        if input.kind() != stage.input_kind() {
            return Err(PipelineError::SensorInputMismatch {
                stage: stage.id(),
                expected: stage.input_kind().name(),
                found: input.kind().name(),
            }
            .into());
        }
        if !matches!(input, SensorData::Raw(_)) {
            let need = stage.input_roi(output_roi, plan);
            if !input.roi().contains_roi(&need) {
                return Err(PipelineError::InvalidRoi {
                    roi: input.roi(),
                    reason: "センサーのステージの入力が必要な範囲を覆っていない",
                }
                .into());
            }
        }
        if output_roi.is_empty() {
            return Err(PipelineError::InvalidRoi {
                roi: output_roi,
                reason: "出力の範囲が空",
            }
            .into());
        }
        match (kind, input) {
            (GpuStageKind::Normalize, SensorData::Raw(raw)) => {
                check_raw(plan, raw)?;
                let words = self
                    .gpu
                    .create_storage_init("genzo-gpu.raw（RAW）", &pack_raw(raw))?;
                let (cfa, n) = self.normalize_on_gpu(plan, raw.cfa, &words, raw, output_roi)?;
                Ok((SensorData::Cfa(cfa), n))
            }
            (GpuStageKind::WhiteBalance, SensorData::Cfa(cfa)) => {
                let (out, n) = self.white_balance_on_gpu(plan, &cfa, output_roi)?;
                Ok((SensorData::Cfa(out), n))
            }
            (GpuStageKind::Half2x2, SensorData::Cfa(cfa)) => {
                let src = self.gpu.create_storage_init(
                    "genzo-gpu.cfa（CFA）",
                    bytemuck::cast_slice(cfa.data()),
                )?;
                let out = self.alloc(output_roi, ColorContract::B1CameraRgb)?;
                let mut p = Params::new(Kernel::Half2x2, cfa.roi(), output_roi);
                p.u32s("CFA", &cfa_table(cfa.pattern(), cfa.roi()));
                let mut batch = Batch::new(&self.gpu, &self.kernels)?;
                batch.dispatch(Dispatch {
                    kernel: Kernel::Half2x2,
                    params: p.finish()?,
                    src: &src,
                    dst: &out.buffer,
                    aux: None,
                    aux_out: None,
                    threads: (output_roi.width, output_roi.height),
                })?;
                let n = batch.submit()?;
                Ok((SensorData::Rgb(self.download(&out)?), n))
            }
            (GpuStageKind::ColorMatrix, SensorData::Rgb(tile)) => {
                if tile.contract() != ColorContract::B1CameraRgb {
                    return Err(PipelineError::ContractMismatch {
                        stage: stage.id(),
                        expected: ColorContract::B1CameraRgb,
                        found: tile.contract(),
                    }
                    .into());
                }
                let src = self.upload(&tile)?;
                let out = self.alloc(output_roi, ColorContract::B2Working)?;
                let mut p = Params::new(Kernel::ColorMatrix, tile.roi, output_roi);
                p.mat("MAT", plan.camera_to_working().rows());
                let mut batch = Batch::new(&self.gpu, &self.kernels)?;
                batch.dispatch(Dispatch {
                    kernel: Kernel::ColorMatrix,
                    params: p.finish()?,
                    src: &src.buffer,
                    dst: &out.buffer,
                    aux: None,
                    aux_out: None,
                    threads: (output_roi.width, output_roi.height),
                })?;
                let n = batch.submit()?;
                Ok((SensorData::Rgb(self.download(&out)?), n))
            }
            (_, input) => Err(PipelineError::SensorInputMismatch {
                stage: stage.id(),
                expected: stage.input_kind().name(),
                found: input.kind().name(),
            }
            .into()),
        }
    }

    /// ステージ 2 を GPU で実行して CPU に読み出す（`words` は詰めた RAW）。
    pub(crate) fn normalize_on_gpu(
        &self,
        plan: &SensorPlan,
        pattern: CfaPattern,
        words: &wgpu::Buffer,
        raw: &RawImage,
        roi: Roi,
    ) -> Result<(genzo_pipeline::CfaImage, u64)> {
        let values = self
            .gpu
            .create_storage("genzo-gpu.cfa（CFA）", roi.pixel_count() * 4)?;
        let flags = self
            .gpu
            .create_storage("genzo-gpu.cfa_flags（CFA）", roi.pixel_count() * 4)?;
        let mut batch = Batch::new(&self.gpu, &self.kernels)?;
        record_normalize(&mut batch, plan, pattern, raw, words, &values, &flags, roi)?;
        let n = batch.submit()?;
        let data: Vec<f32> =
            bytemuck::pod_collect_to_vec(&self.gpu.read_buffer(&values, roi.pixel_count() * 4)?);
        let sat: Vec<u32> =
            bytemuck::pod_collect_to_vec(&self.gpu.read_buffer(&flags, roi.pixel_count() * 4)?);
        let shifted = pattern.shifted(
            i64::from(roi.x).rem_euclid(2) as usize,
            i64::from(roi.y).rem_euclid(2) as usize,
        );
        let cfa = genzo_pipeline::CfaImage::new(
            roi,
            shifted,
            data,
            sat.into_iter().map(|s| s != 0).collect(),
        )?;
        Ok((cfa, n))
    }

    fn white_balance_on_gpu(
        &self,
        plan: &SensorPlan,
        cfa: &genzo_pipeline::CfaImage,
        out_roi: Roi,
    ) -> Result<(genzo_pipeline::CfaImage, u64)> {
        let src = self
            .gpu
            .create_storage_init("genzo-gpu.cfa（CFA）", bytemuck::cast_slice(cfa.data()))?;
        let flags_in: Vec<u32> = cfa.saturated().iter().map(|&s| u32::from(s)).collect();
        let flags = self.gpu.create_storage_init(
            "genzo-gpu.cfa_flags（CFA）",
            bytemuck::cast_slice(&flags_in),
        )?;
        let out = self
            .gpu
            .create_storage("genzo-gpu.cfa（CFA）", out_roi.pixel_count() * 4)?;
        let flags_out = self
            .gpu
            .create_storage("genzo-gpu.cfa_flags（CFA）", out_roi.pixel_count() * 4)?;
        let mut batch = Batch::new(&self.gpu, &self.kernels)?;
        record_white_balance(
            &mut batch,
            plan,
            cfa_table(cfa.pattern(), cfa.roi()),
            cfa.roi(),
            &src,
            &flags,
            &out,
            Some(&flags_out),
            out_roi,
        )?;
        let n = batch.submit()?;
        let data: Vec<f32> =
            bytemuck::pod_collect_to_vec(&self.gpu.read_buffer(&out, out_roi.pixel_count() * 4)?);
        let mut window = cfa.window(out_roi)?;
        window.data_mut().copy_from_slice(&data);
        Ok((window, n))
    }
}

/// CFA の色の番号の表（全体での座標の偶奇で引く: 番号 = 表[(y の偶奇) × 2 + (x の偶奇)]）。
/// `pattern` は範囲 `roi` の左上から見た並び（CfaImage と同じ）。
pub(crate) fn cfa_table(pattern: CfaPattern, roi: Roi) -> [u32; 4] {
    let mut t = [0u32; 4];
    for (k, slot) in t.iter_mut().enumerate() {
        let (px, py) = ((k & 1) as i64, (k >> 1) as i64);
        let lx = (px - i64::from(roi.x)).rem_euclid(2) as usize;
        let ly = (py - i64::from(roi.y)).rem_euclid(2) as usize;
        *slot = pattern.color_at(lx, ly).index() as u32;
    }
    t
}

/// RAW の u16 の値を 2 つずつ u32 に詰めたバイト列（小さい番号が下位の 16 ビット。リトルエンディアン）。
pub(crate) fn pack_raw(raw: &RawImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.data.len() * 2 + 2);
    for v in &raw.data {
        out.extend_from_slice(&v.to_le_bytes());
    }
    if out.len() % 4 != 0 {
        out.extend_from_slice(&[0, 0]);
    }
    out
}

/// RAW がセンサー処理の設定を作った RAW と同じ形か（寸法・CFA の並び・データの長さ。CPU 版の
/// `SensorPlan::check_raw` そのもので、条件もエラーも同じ）。
///
/// 以前は寸法とデータの長さだけを確かめていたため、CFA の並びが違う RAW を CPU 版はエラーにし、GPU 版は
/// 違う並びのまま正規化していた（独立レビューで修正）。
pub(crate) fn check_raw(plan: &SensorPlan, raw: &RawImage) -> Result<()> {
    plan.check_raw(raw)?;
    Ok(())
}

/// ステージ 2 のディスパッチを記録する。
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_normalize(
    batch: &mut Batch<'_>,
    plan: &SensorPlan,
    pattern: CfaPattern,
    raw: &RawImage,
    words: &wgpu::Buffer,
    values: &wgpu::Buffer,
    flags: &wgpu::Buffer,
    roi: Roi,
) -> Result<()> {
    let levels = plan.levels();
    let mut p = Params::new(Kernel::Normalize, Roi::full(raw.width, raw.height), roi);
    p.u32s("CFA", &cfa_table(pattern, Roi::new(0, 0, 1, 1)))
        .u32("RAW_W", raw.width)
        .u32("RAW_H", raw.height)
        .f32s("BLACK", &levels.black_level)
        .f32s("INV", &levels.inv_range)
        .f32("WHITE", levels.white_level);
    batch.dispatch(Dispatch {
        kernel: Kernel::Normalize,
        params: p.finish()?,
        src: words,
        dst: values,
        aux: None,
        aux_out: Some(flags),
        threads: (roi.width, roi.height),
    })
}

/// ステージ 3 のディスパッチを記録する。
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_white_balance(
    batch: &mut Batch<'_>,
    plan: &SensorPlan,
    table: [u32; 4],
    in_roi: Roi,
    values: &wgpu::Buffer,
    flags: &wgpu::Buffer,
    out: &wgpu::Buffer,
    flags_out: Option<&wgpu::Buffer>,
    out_roi: Roi,
) -> Result<()> {
    let wb = plan.wb();
    let (mode, radius) = match plan.saturation_clip() {
        SaturationClip::Off => (0u32, 0u32),
        SaturationClip::Neighborhood { radius } => (1, radius),
        SaturationClip::Global => (2, 0),
    };
    let radius = i32::try_from(radius).map_err(|_| GpuError::Unsupported {
        stage: "sensor.white_balance",
        reason: format!("飽和の切りそろえの半径 {radius} が大きすぎる"),
    })?;
    let mut p = Params::new(Kernel::WhiteBalance, in_roi, out_roi);
    p.u32s("CFA", &table)
        .f32s("MULT", &wb.multipliers)
        .f32("CLIP_LEVEL", wb.clip_level)
        .u32("CLIP_MODE", mode)
        .i32("RADIUS", radius)
        .u32("FLAGS_OUT", u32::from(flags_out.is_some()));
    batch.dispatch(Dispatch {
        kernel: Kernel::WhiteBalance,
        params: p.finish()?,
        src: values,
        dst: out,
        aux: Some(flags),
        aux_out: flags_out,
        threads: (out_roi.width, out_roi.height),
    })
}

/// 符号化した値（B4a・B4b・B5）か（genzo-pipeline の量子化・ヒストグラムと同じ確認）。
fn check_encoded(contract: ColorContract) -> Result<()> {
    if contract.is_linear() {
        return Err(PipelineError::InvalidArgument(format!(
            "量子化・ヒストグラムは符号化した値（B4a・B4b・B5）にだけ使えます（{contract}）"
        ))
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfa_table_follows_global_parity() {
        // RGGB の RAW を (1, 0) から切り出した CFA の並びは GRBG。全体の偶奇で引けば RGGB のまま。
        let p = CfaPattern::RGGB.shifted(1, 0);
        let t = cfa_table(p, Roi::new(1, 0, 4, 4));
        let rggb = cfa_table(CfaPattern::RGGB, Roi::new(0, 0, 4, 4));
        assert_eq!(t, rggb);
        assert_eq!(rggb, [0, 1, 3, 2]);
        // 負の座標から始まる範囲でも同じ。
        let q = CfaPattern::RGGB.shifted(1, 1);
        assert_eq!(cfa_table(q, Roi::new(-3, -1, 8, 8)), rggb);
    }

    #[test]
    fn raw_is_packed_little_endian() {
        let raw = RawImage {
            width: 3,
            height: 1,
            cfa: CfaPattern::RGGB,
            data: vec![0x0102, 0x0304, 0x0506],
            black_level: [0.0; 4],
            white_level: 4095.0,
            as_shot_wb: [1.0; 4],
            cam_xyz: None,
            metadata: genzo_model::PhotoMetadata::default(),
        };
        assert_eq!(pack_raw(&raw), vec![2, 1, 4, 3, 6, 5, 0, 0]);
    }
}
