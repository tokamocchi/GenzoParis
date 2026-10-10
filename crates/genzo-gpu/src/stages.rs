//! GPU 版のステージの登録表（docs/04_architecture.md の 7.1 節「GPU 版の実装（WGSL）。未実装なら
//! None を返し、CPU 版で処理する」「ステージの CPU 版と GPU 版の一致テストは、ステージを登録すると
//! 自動で対象になる」）。
//!
//! genzo-pipeline の [`Stage::gpu`]・[`SensorStage::gpu`](genzo_pipeline::sensor::SensorStage::gpu) は、ステージの ID で
//! [`GpuStageLookup`] を引く。この crate の [`GpuStageRegistry`] がその実装で、ID ごとに
//! [`GpuStageEntry`]（[`GpuStage`] の実装）を持つ。GPU 版を実行するときは
//! [`GpuStage::as_any`] でこの型に戻し、[`GpuStageKind`] で WGSL のカーネルを選ぶ。
//!
//! | ID | GPU 版 | カーネル（`shaders/`） |
//! |---|---|---|
//! | `finish.geometry` | ステージ 9（バイキュービック） | `geometry.wgsl` |
//! | `finish.exposure`・`finish.contrast` | ステージ 10 | `finish.wgsl` |
//! | `finish.tone` | ステージ 11（ガイドの双線形） | `finish.wgsl` |
//! | `finish.color` | ステージ 13（OKLab） | `finish.wgsl` |
//! | `finish.scene_to_display` | ステージ 15 | `finish.wgsl` |
//! | `finish.tone_curve` | ステージ 16（1D LUT） | `finish.wgsl` |
//! | `output.display` | ステージ 17a（3D LUT の四面体補間） | `output.wgsl` |
//! | `output.export.*`・`output.cache.display_p3` | ステージ 17b | `output.wgsl` |
//! | `sensor.normalize`・`sensor.white_balance`・`sensor.demosaic.half2x2` | ステージ 2・3・5（簡易） | `sensor.wgsl` |
//! | `sensor.color_matrix` | ステージ 8 | `resample.wgsl` |
//!
//! GPU 版がないステージ（`finish.texture`・`finish.sharpen`（v1 の予約）、`sensor.demosaic.rcd`・
//! `sensor.demosaic.bilinear`）は CPU 版で処理する。17c のヒストグラム・量子化は画像を出力するステージで
//! はないので、[`crate::GpuRenderer::histogram`]・[`crate::GpuRenderer::quantize_u8`] で実行する。

use std::any::Any;
use std::sync::Arc;

use genzo_color::{DisplayGamut, Mat3F32, RgbColorSpace, TransferFunction};
use genzo_pipeline::finish::exposure::{ContrastParams, ExposureParams};
use genzo_pipeline::finish::geometry::GeometryPlan;
use genzo_pipeline::finish::output::{DisplayTransform, ExportTransform};
use genzo_pipeline::finish::scene_to_display::SceneToDisplayParams;
use genzo_pipeline::finish::tone::ToneStageParams;
use genzo_pipeline::finish::tone_curve::ToneCurveParams;
use genzo_pipeline::finish::{
    ColorStage, ContrastStage, DisplayStage, ExportStage, ExposureStage, GeometryStage,
    SceneToDisplayStage, ToneCurveStage, ToneStage,
};
use genzo_pipeline::guide::checked_guide;
use genzo_pipeline::interp::Lut1d;
use genzo_pipeline::sensor::{ColorMatrixStage, NormalizeStage, WhiteBalanceStage};
use genzo_pipeline::{
    DemosaicMethod, GpuStage, GpuStageLookup, ProcessVersion, Roi, Stage, StageContext, StageParams,
};

use crate::error::{GpuError, Result};
use crate::kernels::{Kernel, Params};

/// GPU 版のステージの種類（使う WGSL のカーネル）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GpuStageKind {
    /// ステージ 9: ジオメトリ。
    Geometry,
    /// ステージ 10: 露光量。
    Exposure,
    /// ステージ 10 の後: コントラスト。
    Contrast,
    /// ステージ 11: トーン。
    Tone,
    /// ステージ 13: 彩度・自然な彩度。
    Color,
    /// ステージ 15: シーン → ディスプレイ。
    SceneToDisplay,
    /// ステージ 16: トーンカーブ。
    ToneCurve,
    /// ステージ 17a: 画面。
    Display,
    /// ステージ 17b: 書き出し・キャッシュ。
    Export,
    /// ステージ 2: 正規化。
    Normalize,
    /// ステージ 3: ホワイトバランス。
    WhiteBalance,
    /// ステージ 5: 2 × 2 の簡易処理。
    Half2x2,
    /// ステージ 8: 色変換。
    ColorMatrix,
}

impl GpuStageKind {
    /// センサーのステージ（[`genzo_pipeline::sensor::SensorStage`]）か。
    pub fn is_sensor(self) -> bool {
        matches!(
            self,
            GpuStageKind::Normalize
                | GpuStageKind::WhiteBalance
                | GpuStageKind::Half2x2
                | GpuStageKind::ColorMatrix
        )
    }
}

/// 登録表の 1 件（genzo-pipeline の [`GpuStage`] の実装）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuStageEntry {
    id: &'static str,
    kind: GpuStageKind,
}

impl GpuStageEntry {
    /// ID と種類から作る。
    pub const fn new(id: &'static str, kind: GpuStageKind) -> Self {
        Self { id, kind }
    }

    /// 種類。
    pub fn kind(&self) -> GpuStageKind {
        self.kind
    }
}

impl GpuStage for GpuStageEntry {
    fn stage_id(&self) -> &'static str {
        self.id
    }

    fn supports(&self, version: ProcessVersion) -> bool {
        match version {
            ProcessVersion::V1 => true,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// GPU 版のステージの登録表（[`GpuStageLookup`] の実装）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuStageRegistry {
    entries: Vec<GpuStageEntry>,
}

impl GpuStageRegistry {
    /// 空の登録表（すべて CPU 版で処理する）。
    pub fn new() -> Self {
        Self::default()
    }

    /// この crate が実装した GPU 版をすべて登録した表（モジュールの doc の表）。
    pub fn builtin() -> Self {
        use GpuStageKind as K;
        let mut entries = vec![
            GpuStageEntry::new(GeometryStage::ID, K::Geometry),
            GpuStageEntry::new(ExposureStage::ID, K::Exposure),
            GpuStageEntry::new(ContrastStage::ID, K::Contrast),
            GpuStageEntry::new(ToneStage::ID, K::Tone),
            GpuStageEntry::new(ColorStage::ID, K::Color),
            GpuStageEntry::new(SceneToDisplayStage::ID, K::SceneToDisplay),
            GpuStageEntry::new(ToneCurveStage::ID, K::ToneCurve),
            GpuStageEntry::new(DisplayStage::ID, K::Display),
        ];
        for s in ExportStage::ALL {
            entries.push(GpuStageEntry::new(s.id(), K::Export));
        }
        entries.extend([
            GpuStageEntry::new(NormalizeStage::ID, K::Normalize),
            GpuStageEntry::new(WhiteBalanceStage::ID, K::WhiteBalance),
            GpuStageEntry::new(DemosaicMethod::Half2x2.stage_id(), K::Half2x2),
            GpuStageEntry::new(ColorMatrixStage::ID, K::ColorMatrix),
        ]);
        Self { entries }
    }

    /// ID `id` の GPU 版を除いた表（そのステージだけ CPU 版で処理する。混在の確認・不具合の回避用）。
    pub fn without(mut self, id: &str) -> Self {
        self.entries.retain(|e| e.id != id);
        self
    }

    /// 1 件を加える（同じ ID があれば置き換える）。
    pub fn with(mut self, entry: GpuStageEntry) -> Self {
        self.entries.retain(|e| e.id != entry.id);
        self.entries.push(entry);
        self
    }

    /// 登録した ID（登録順）。
    pub fn ids(&self) -> Vec<&'static str> {
        self.entries.iter().map(|e| e.id).collect()
    }

    /// 登録した件。
    pub fn entries(&self) -> &[GpuStageEntry] {
        &self.entries
    }
}

impl GpuStageLookup for GpuStageRegistry {
    fn find(&self, stage_id: &str) -> Option<&dyn GpuStage> {
        self.entries
            .iter()
            .find(|e| e.id == stage_id)
            .map(|e| e as &dyn GpuStage)
    }
}

/// `GpuStage` をこの crate の種類に戻す（この処理バージョンに対応していなければ `None`）。
pub(crate) fn kind_of(gpu: Option<&dyn GpuStage>, version: ProcessVersion) -> Option<GpuStageKind> {
    let g = gpu?;
    if !g.supports(version) {
        return None;
    }
    g.as_any()
        .downcast_ref::<GpuStageEntry>()
        .map(GpuStageEntry::kind)
}

/// 画面の色域の圧縮の行列（作業色空間 → 表示先、表示先 → 作業色空間）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DisplayMatrices {
    pub(crate) gamut: DisplayGamut,
    pub(crate) to_display: Mat3F32,
    pub(crate) to_working: Mat3F32,
}

impl DisplayMatrices {
    /// 標準の色空間を表示先とするもの（genzo-color の `DisplayGamut::for_space`）。
    pub(crate) fn for_space(space: RgbColorSpace) -> Self {
        Self::from_gamut(DisplayGamut::for_space(space))
    }

    /// genzo-color の [`DisplayGamut`] から作る（行列は CPU 版が使う f32 の値そのもの）。
    pub(crate) fn from_gamut(gamut: DisplayGamut) -> Self {
        Self {
            gamut,
            to_display: gamut.working_to_display(),
            to_working: gamut.display_to_working(),
        }
    }
}

/// 1 回の GPU 版の実行に必要なもの（カーネル・パラメータ・補助の入力）。
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    pub(crate) kernel: Kernel,
    pub(crate) params: Vec<u32>,
    /// 補助の入力（binding 4）のバイト列。
    pub(crate) aux: Option<Arc<Vec<u8>>>,
}

fn unsupported(stage: &'static str, reason: impl Into<String>) -> GpuError {
    GpuError::Unsupported {
        stage,
        reason: reason.into(),
    }
}

fn f32_bytes(v: &[f32]) -> Arc<Vec<u8>> {
    Arc::new(bytemuck::cast_slice(v).to_vec())
}

/// i64 の座標を i32 にする（範囲の外なら GPU 版では扱えない）。
fn to_i32(stage: &'static str, v: i64) -> Result<i32> {
    i32::try_from(v).map_err(|_| unsupported(stage, format!("座標 {v} が i32 の範囲の外")))
}

/// RGB のステージのパラメータを GPU 版の形にする（モジュールの doc の表）。
///
/// GPU 版で扱えないパラメータ（行列のわからないモニターのプロファイルなど）は
/// [`GpuError::Unsupported`]（そのステージは CPU 版で処理する）。
pub(crate) fn prepare_rgb(
    kind: GpuStageKind,
    stage: &dyn Stage,
    ctx: &StageContext<'_>,
    params: &StageParams,
    input: Roi,
    output: Roi,
    displays: &[DisplayMatrices],
) -> Result<Prepared> {
    let id = stage.id();
    match ctx.process_version {
        ProcessVersion::V1 => {}
    }
    let (kernel, p, aux) = match kind {
        GpuStageKind::Geometry => {
            let plan = params.expect::<GeometryPlan>(id)?;
            let a = plan.lattice_to_input_f32();
            let crop = plan.crop_lattice();
            let grid = plan.sensor_grid();
            let mut p = Params::new(Kernel::Geometry, input, output);
            p.f32s(
                "AFF",
                &[a[0][0], a[0][1], a[0][2], a[1][0], a[1][1], a[1][2]],
            )
            .i32("CROP_X", crop.x)
            .i32("CROP_Y", crop.y)
            .u32("GRID_W", grid.width)
            .u32("GRID_H", grid.height);
            (Kernel::Geometry, p, None)
        }
        GpuStageKind::Exposure => {
            let e = params.expect::<ExposureParams>(id)?;
            let mut p = Params::new(Kernel::Exposure, input, output);
            p.f32("GAIN", e.gain);
            (Kernel::Exposure, p, None)
        }
        GpuStageKind::Contrast => {
            let c = params.expect::<ContrastParams>(id)?;
            let mut p = Params::new(Kernel::Contrast, input, output);
            p.f32("GAMMA", c.gamma);
            (Kernel::Contrast, p, None)
        }
        GpuStageKind::Tone => {
            let t = params.expect::<ToneStageParams>(id)?;
            let guide = checked_guide(ctx, id)?;
            let g = &guide.image;
            let mut p = Params::new(Kernel::Tone, input, output);
            p.f32("TONE_H", t.highlights)
                .f32("TONE_S", t.shadows)
                .f32("TONE_W", t.whites)
                .f32("TONE_B", t.blacks)
                .f32("TONE_EV", t.ev)
                .f32("GAMMA", t.contrast.gamma)
                .i32("GUIDE_OX", to_i32(id, t.lookup.origin.0)?)
                .i32("GUIDE_OY", to_i32(id, t.lookup.origin.1)?)
                .f32("GUIDE_SX", t.lookup.scale.0)
                .f32("GUIDE_SY", t.lookup.scale.1)
                .u32("GUIDE_W", g.width())
                .u32("GUIDE_H", g.height());
            (Kernel::Tone, p, Some(f32_bytes(g.data())))
        }
        GpuStageKind::Color => {
            let c = params.expect::<genzo_pipeline::finish::color::ColorStageParams>(id)?;
            let mut p = Params::new(Kernel::Color, input, output);
            p.f32("SAT", c.saturation)
                .f32("VIB", c.vibrance)
                .f32("SKIN_COS", c.skin_cos)
                .f32("SKIN_SIN", c.skin_sin)
                .f32("SKIN_COS_W", c.skin_cos_width)
                .f32("NEUTRAL_A", c.neutral.0)
                .f32("NEUTRAL_B", c.neutral.1);
            (Kernel::Color, p, None)
        }
        GpuStageKind::SceneToDisplay => {
            let s = params.expect::<SceneToDisplayParams>(id)?;
            let mut p = Params::new(Kernel::SceneToDisplay, input, output);
            p.f32("KNEE", s.knee);
            (Kernel::SceneToDisplay, p, None)
        }
        GpuStageKind::ToneCurve => {
            let t = params.expect::<ToneCurveParams>(id)?;
            prepare_tone_curve(t, input, output)?
        }
        GpuStageKind::Display => {
            let t = params.expect::<Arc<DisplayTransform>>(id)?;
            let m = displays
                .iter()
                .find(|d| d.gamut == t.gamut)
                .copied()
                .or_else(|| {
                    RgbColorSpace::ALL
                        .into_iter()
                        .map(DisplayMatrices::for_space)
                        .find(|d| d.gamut == t.gamut)
                })
                .ok_or_else(|| {
                    unsupported(
                        id,
                        "モニターの色域の行列がわからない（GpuRenderer::register_display_profile で登録する）",
                    )
                })?;
            let c = t.gamut.compressor();
            let n = t.lut.size();
            let mut p = Params::new(Kernel::Display, input, output);
            p.mat("W2D", m.to_display.rows())
                .mat("D2W", m.to_working.rows())
                .f32s("GC_LUMA", &c.luminance_coefficients())
                .f32("GC_THR", c.threshold())
                .f32("GC_POW", c.power())
                .f32("GC_INV_POW", 1.0 / c.power())
                .u32("LUT_N", n as u32);
            (Kernel::Display, p, Some(f32_bytes(t.lut.as_flat())))
        }
        GpuStageKind::Export => {
            let t = params.expect::<ExportTransform>(id)?;
            let c = &t.compressor;
            let transfer = match t.transfer {
                TransferFunction::Linear => 0u32,
                TransferFunction::Srgb => 1,
                TransferFunction::AdobeRgbGamma => 2,
            };
            let mut p = Params::new(Kernel::Export, input, output);
            p.mat("MAT", t.matrix.rows())
                .f32s("GC_LUMA", &c.luminance_coefficients())
                .f32("GC_THR", c.threshold())
                .f32("GC_POW", c.power())
                .f32("GC_INV_POW", 1.0 / c.power())
                .u32("TRANSFER", transfer);
            (Kernel::Export, p, None)
        }
        GpuStageKind::Normalize
        | GpuStageKind::WhiteBalance
        | GpuStageKind::Half2x2
        | GpuStageKind::ColorMatrix => {
            return Err(unsupported(
                id,
                "センサーのステージは RGB のステージとして実行できない",
            ));
        }
    };
    Ok(Prepared {
        kernel,
        params: p.finish()?,
        aux,
    })
}

/// カーネル・パラメータ・補助の入力の組。
type KernelSetup = (Kernel, Params, Option<Arc<Vec<u8>>>);

/// ステージ 16 のパラメータ（1D LUT を 1 つの表につなぐ。輝度、R、G、B の順）。
fn prepare_tone_curve(t: &ToneCurveParams, input: Roi, output: Roi) -> Result<KernelSetup> {
    let luts: [Option<&Lut1d>; 4] = [
        t.luma.as_ref(),
        t.channels[0].as_ref(),
        t.channels[1].as_ref(),
        t.channels[2].as_ref(),
    ];
    let mut values: Vec<f32> = Vec::new();
    let mut flags = 0u32;
    let (mut off, mut len, mut lo, mut inv) = ([0u32; 4], [2u32; 4], [0f32; 4], [1f32; 4]);
    for (k, lut) in luts.iter().enumerate() {
        if let Some(l) = lut {
            flags |= 1 << k;
            off[k] = values.len() as u32;
            len[k] = l.values().len() as u32;
            lo[k] = l.lo();
            inv[k] = l.inv_step();
            values.extend_from_slice(l.values());
        }
    }
    if values.is_empty() {
        values.extend_from_slice(&[0.0, 0.0]);
    }
    let mut p = Params::new(Kernel::ToneCurve, input, output);
    p.u32("CURVE_FLAGS", flags)
        .u32s("LUT_OFF", &off)
        .u32s("LUT_LEN", &len)
        .f32s("LUT_LO", &lo)
        .f32s("LUT_INV", &inv)
        .f32("BLACK_LIFT", t.black_lift)
        .f32("RATIO_MIN", t.ratio_min);
    Ok((Kernel::ToneCurve, p, Some(f32_bytes(&values))))
}

#[cfg(test)]
mod tests {
    use genzo_pipeline::{NoGpu, StageRegistry};

    use super::*;

    #[test]
    fn builtin_covers_the_planned_stages() {
        let r = GpuStageRegistry::builtin();
        let reg = StageRegistry::builtin();
        let c = reg.gpu_coverage(&r, ProcessVersion::V1);
        for id in [
            "finish.geometry",
            "finish.exposure",
            "finish.contrast",
            "finish.tone",
            "finish.color",
            "finish.scene_to_display",
            "finish.tone_curve",
            "output.display",
            "output.export.srgb",
            "output.export.display_p3",
            "output.export.adobe_rgb",
            "output.cache.display_p3",
            "sensor.normalize",
            "sensor.white_balance",
            "sensor.demosaic.half2x2",
            "sensor.color_matrix",
        ] {
            assert!(c.with_gpu.contains(&id), "{id}");
        }
        // GPU 版のないステージは CPU 版。
        for id in [
            "finish.texture",
            "finish.sharpen",
            "sensor.demosaic.rcd",
            "sensor.demosaic.bilinear",
        ] {
            assert!(c.cpu_only.contains(&id), "{id}");
        }
        // 登録したすべての ID がパイプラインに存在する（綴りの誤りがない）。
        for id in r.ids() {
            assert!(reg.contains(id), "{id}");
        }
        assert!(
            reg.gpu_coverage(&NoGpu, ProcessVersion::V1)
                .with_gpu
                .is_empty()
        );
    }

    #[test]
    fn stages_are_found_by_id_and_downcast() {
        let r = GpuStageRegistry::builtin();
        let tone = StageRegistry::builtin();
        let s = tone.find("finish.tone").unwrap();
        assert_eq!(
            kind_of(s.gpu(&r), ProcessVersion::V1),
            Some(GpuStageKind::Tone)
        );
        let without = r.clone().without("finish.tone");
        assert_eq!(kind_of(s.gpu(&without), ProcessVersion::V1), None);
        assert_eq!(without.ids().len(), r.ids().len() - 1);
        let back = without.with(GpuStageEntry::new("finish.tone", GpuStageKind::Tone));
        assert_eq!(back.ids().len(), r.ids().len());
        assert!(GpuStageKind::Normalize.is_sensor());
        assert!(!GpuStageKind::Export.is_sensor());
    }

    #[test]
    fn standard_display_matrices_match_genzo_color() {
        for space in RgbColorSpace::ALL {
            let m = DisplayMatrices::for_space(space);
            // DisplayGamut::compress_working と、行列と圧縮を順にかけたものが同じ。
            let c = [0.3f32, 0.2, 0.1];
            let d = m.to_display.apply(c);
            let back = m.to_working.apply(m.gamut.compressor().compress(d));
            assert_eq!(back, m.gamut.compress_working(c), "{space:?}");
        }
    }
}
