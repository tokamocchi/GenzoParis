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
//! | `finish.geometry` | ステージ 9（バイキュービック。歪曲補正・遠近補正（処理バージョン 2）を含む） | `geometry.wgsl` |
//! | `finish.exposure`・`finish.contrast` | ステージ 10 | `finish.wgsl` |
//! | `finish.profile_look_table` | ステージ 10 の後のカメラプロファイルのルックテーブル（処理バージョン 2。DEV-17） | `profile.wgsl`（`hsv_table_main`） |
//! | `finish.tone` | ステージ 11（ガイドの双線形） | `finish.wgsl` |
//! | `finish.texture` | ステージ 12（テクスチャ・明瞭度・かすみの除去。処理バージョン 2。輝度の対数・横と縦のぼかし・仕上げの 4 つのカーネル） | `detail.wgsl` |
//! | `finish.color` | ステージ 13（OKLab。彩度・自然な彩度と、処理バージョン 2 のカラーミキサー・カラーグレーディング・白黒） | `finish.wgsl` |
//! | `finish.sharpen` | ステージ 14（シャープ。処理バージョン 2。輝度の対数・横と縦のぼかし・仕上げの 4 つのカーネル） | `detail.wgsl` |
//! | `finish.scene_to_display` | ステージ 15（1D LUT） | `finish.wgsl` |
//! | `finish.look` | ステージ 15 の後の独自のルック（3D LUT の四面体補間。処理バージョン 2。DEV-17） | `profile.wgsl`（`look_main`）・`lut3d.wgsl` |
//! | `finish.tone_curve` | ステージ 16（1D LUT） | `finish.wgsl` |
//! | `finish.effects` | ステージ 16 の効果（切り抜き後の周辺光量補正・粒子。処理バージョン 2） | `detail.wgsl` |
//! | `output.display` | ステージ 17a（3D LUT の四面体補間） | `output.wgsl` |
//! | `output.export.*`・`output.cache.display_p3` | ステージ 17b | `output.wgsl` |
//! | `sensor.normalize`・`sensor.white_balance`・`sensor.demosaic.half2x2` | ステージ 2・3・5（簡易） | `sensor.wgsl` |
//! | `sensor.highlight_recovery` | ステージ 4（ハイライト復元。処理バージョン 2） | `sensor.wgsl`（`highlight_main`） |
//! | `sensor.noise_reduction` | ステージ 6（ノイズ軽減。処理バージョン 2。順・段ごとの横と縦・細部・逆の 5 種類のカーネル） | `denoise.wgsl` |
//! | `sensor.lens_correction` | ステージ 7（周辺減光・倍率色収差。処理バージョン 2） | `geometry.wgsl`（`lens_main`） |
//! | `sensor.color_matrix` | ステージ 8（カメラプロファイルの色相彩度のマップがあれば `profile.wgsl` の `hsv_table_main`。DEV-17） | `resample.wgsl` |
//!
//! ステージ 10〜14（露光量・コントラスト・トーン・テクスチャ・色・シャープ）は、処理バージョン 2 のローカル補正（マスク。
//! DEV-18）があれば、局所の値の面（`local.wgsl`。[`local_params`] のカーネルが作る）から画素の局所の値を引く。
//!
//! GPU 版がないステージ（`sensor.demosaic.rcd`・`sensor.demosaic.bilinear`）は CPU 版で処理する。17c のヒストグラム・量子化は画像を出力するステージで
//! はないので、[`crate::GpuRenderer::histogram`]・[`crate::GpuRenderer::quantize_u8`] で実行する。

use std::any::Any;
use std::sync::Arc;

use genzo_color::{DisplayGamut, Mat3F32, RgbColorSpace, TransferFunction};
use genzo_model::MaskMode;
use genzo_pipeline::finish::effects::EffectsStageParams;
use genzo_pipeline::finish::exposure::{ContrastStageParams, ExposureParams};
use genzo_pipeline::finish::geometry::GeometryPlan;
use genzo_pipeline::finish::look::{LookParams, LookTableParams};
use genzo_pipeline::finish::output::{DisplayTransform, ExportTransform};
use genzo_pipeline::finish::presence::PresenceStageParams;
use genzo_pipeline::finish::scene_to_display::SceneToDisplayParams;
use genzo_pipeline::finish::sharpen::SharpenStageParams;
use genzo_pipeline::finish::tone::ToneStageParams;
use genzo_pipeline::finish::tone_curve::ToneCurveParams;
use genzo_pipeline::finish::{
    ColorStage, ContrastStage, DisplayStage, EffectsStage, ExportStage, ExposureStage,
    GeometryStage, LookStage, ProfileLookTableStage, SceneToDisplayStage, SharpenStage,
    TextureStage, ToneCurveStage, ToneStage,
};
use genzo_pipeline::guide::checked_guide;
use genzo_pipeline::interp::Lut1d;
use genzo_pipeline::local::{BrushRaster, LocalPlan, LocalSlots, PreparedShape};
use genzo_pipeline::sensor::{
    ColorMatrixStage, HighlightRecoveryStage, LensCorrectionStage, NoiseReductionStage,
    NormalizeStage, WhiteBalanceStage,
};
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
    /// ステージ 12: テクスチャ・明瞭度・かすみの除去。
    Texture,
    /// ステージ 13: 彩度・自然な彩度・カラーミキサー・カラーグレーディング・白黒。
    Color,
    /// ステージ 14: シャープ。
    Sharpen,
    /// ステージ 15: シーン → ディスプレイ。
    SceneToDisplay,
    /// ステージ 16: トーンカーブ。
    ToneCurve,
    /// ステージ 16 の効果（切り抜き後の周辺光量補正・粒子）。
    Effects,
    /// ステージ 10 の後のカメラプロファイルのルックテーブル。
    ProfileLookTable,
    /// ステージ 15 の後の独自のルック（3D LUT）。
    Look,
    /// ステージ 17a: 画面。
    Display,
    /// ステージ 17b: 書き出し・キャッシュ。
    Export,
    /// ステージ 2: 正規化。
    Normalize,
    /// ステージ 3: ホワイトバランス。
    WhiteBalance,
    /// ステージ 4: ハイライト復元。
    HighlightRecovery,
    /// ステージ 5: 2 × 2 の簡易処理。
    Half2x2,
    /// ステージ 6: ノイズ軽減。
    NoiseReduction,
    /// ステージ 7: レンズ補正（周辺減光・倍率色収差）。
    LensCorrection,
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
                | GpuStageKind::HighlightRecovery
                | GpuStageKind::Half2x2
                | GpuStageKind::NoiseReduction
                | GpuStageKind::LensCorrection
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
            ProcessVersion::V1 | ProcessVersion::V2 => true,
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
            GpuStageEntry::new(TextureStage::ID, K::Texture),
            GpuStageEntry::new(ColorStage::ID, K::Color),
            GpuStageEntry::new(SharpenStage::ID, K::Sharpen),
            GpuStageEntry::new(SceneToDisplayStage::ID, K::SceneToDisplay),
            GpuStageEntry::new(ToneCurveStage::ID, K::ToneCurve),
            GpuStageEntry::new(EffectsStage::ID, K::Effects),
            GpuStageEntry::new(ProfileLookTableStage::ID, K::ProfileLookTable),
            GpuStageEntry::new(LookStage::ID, K::Look),
            GpuStageEntry::new(DisplayStage::ID, K::Display),
        ];
        for s in ExportStage::ALL {
            entries.push(GpuStageEntry::new(s.id(), K::Export));
        }
        entries.extend([
            GpuStageEntry::new(NormalizeStage::ID, K::Normalize),
            GpuStageEntry::new(WhiteBalanceStage::ID, K::WhiteBalance),
            GpuStageEntry::new(HighlightRecoveryStage::ID, K::HighlightRecovery),
            GpuStageEntry::new(DemosaicMethod::Half2x2.stage_id(), K::Half2x2),
            GpuStageEntry::new(NoiseReductionStage::ID, K::NoiseReduction),
            GpuStageEntry::new(LensCorrectionStage::ID, K::LensCorrection),
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

/// [`prepare_rgb`] が使う、ステージの外の情報（登録したモニターの色域の行列と、チェーンが作った局所の値の面の範囲）。
pub(crate) struct PrepareEnv<'a> {
    pub(crate) displays: &'a [DisplayMatrices],
    pub(crate) local: Option<Roi>,
}

/// 1 回の GPU 版の実行に必要なもの（カーネル・パラメータ・補助の入力）。
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    pub(crate) kernel: Kernel,
    pub(crate) params: Vec<u32>,
    /// 補助の入力（binding 4）のバイト列。
    pub(crate) aux: Option<Arc<Vec<u8>>>,
    /// 仕上げのカーネルの前に、輝度の対数の面をぼかす（ステージ 12・14。結果の面を binding 5 で渡す）。
    pub(crate) blur: Option<BlurPrep>,
    /// ローカル補正の局所の値の面（binding 6）を使うか。
    pub(crate) uses_local: bool,
}

/// ステージ 10〜14 のカーネルの、ローカル補正のパラメータ（`local.wgsl` の P_LOCAL_*）を入れる。`slots` がなければ
/// 使わない（`LOCAL_FLAGS` = 0）。`field` は局所の値の面の範囲（チェーンが作ったもの）。使うなら `true`。
fn set_local(
    p: &mut Params,
    id: &'static str,
    slots: Option<LocalSlots>,
    only: bool,
    field: Option<Roi>,
    output: Roi,
) -> Result<bool> {
    let Some(slots) = slots else {
        p.u32("LOCAL_FLAGS", 0);
        return Ok(false);
    };
    let roi = field.ok_or_else(|| {
        GpuError::Internal(format!("{id}: 局所の値の面がない（ステージ 9 の後に作る）"))
    })?;
    if !roi.contains_roi(&output) {
        return Err(GpuError::Internal(format!(
            "{id}: 局所の値の面が出力の範囲を覆っていない"
        )));
    }
    let n = u32::try_from(roi.pixel_count())
        .map_err(|_| unsupported(id, "局所の値の面の画素数が u32 の範囲の外"))?;
    p.u32("LOCAL_FLAGS", 1 | if only { 2 } else { 0 })
        .i32("LOCAL_X", roi.x)
        .i32("LOCAL_Y", roi.y)
        .u32("LOCAL_W", roi.width)
        .u32("LOCAL_N", n)
        .u32s("LOCAL_SLOT", &slots.to_u32());
    Ok(true)
}

/// 局所の値の面を作るカーネル（`local.wgsl` の `local_main`）のパラメータと補助の入力。
///
/// 補助の入力（f32 の並び。数・位置は f32 でちょうど表せる整数）:
///
/// | 位置 | 内容 |
/// |---|---|
/// | 0〜 | ステージ 15 の輝度のカーブの表（RAW のとき。`LUT_OFF` の表 0） |
/// | `LOCAL_DATA`〜 | マスクごとに [部品の数, 初めの値, 面ごとの差分 × 面の数] と、部品ごとに 16 個 [種類, 合成, 反転, 値 13 個] |
/// | その後 | ブラシのラスター（部品の値に、値の位置・幅・高さ・原点・1 / セル・あるか） |
///
/// 部品の種類は 0 線形（x0, y0, dx, dy, inv_len2）、1 円形（cx, cy, cos, sin, inv_rx, inv_ry, inner）、2 ブラシ、3 色域
/// （見本の数, lo, hi, 見本の (a, b) × 5）、4 輝度（lo, hi, 滑らかさ）。合成は 0 加える・1 除く・2 交差。
pub(crate) fn local_params(
    plan: &LocalPlan,
    input: Roi,
    output: Roi,
) -> Result<(Params, Arc<Vec<u8>>)> {
    const ID: &str = "local.field";
    let mut values: Vec<f32> = Vec::new();
    let mut p = Params::new(Kernel::Local, input, output);
    match &plan.tone.lut {
        Some(l) => {
            values.extend_from_slice(l.values());
            p.u32("S2D_MODE", 1)
                .u32s("LUT_OFF", &[0; 4])
                .u32s("LUT_LEN", &[l.values().len() as u32, 2, 2, 2])
                .f32s("LUT_LO", &[l.lo(), 0.0, 0.0, 0.0])
                .f32s("LUT_INV", &[l.inv_step(), 1.0, 1.0, 1.0]);
        }
        None => {
            p.u32("S2D_MODE", 0)
                .u32s("LUT_OFF", &[0; 4])
                .u32s("LUT_LEN", &[2; 4])
                .f32s("LUT_LO", &[0.0; 4])
                .f32s("LUT_INV", &[1.0; 4]);
        }
    }
    p.f32("S2D_FLOOR_RATIO", plan.tone.floor_ratio);
    let data = values.len();
    // ブラシのラスターは後ろに置く（位置は後で埋める）。
    let mut rasters: Vec<(usize, &Arc<BrushRaster>)> = Vec::new();
    for m in &plan.masks {
        values.push(m.components.len() as f32);
        values.push(m.initial);
        values.extend_from_slice(&m.delta);
        for c in &m.components {
            let mut rec = [0.0f32; 16];
            rec[1] = match c.mode {
                MaskMode::Add => 0.0,
                MaskMode::Subtract => 1.0,
                MaskMode::Intersect => 2.0,
            };
            rec[2] = if c.invert { 1.0 } else { 0.0 };
            let q = &mut rec[3..];
            match &c.shape {
                PreparedShape::Linear {
                    x0,
                    y0,
                    dx,
                    dy,
                    inv_len2,
                } => {
                    q[..5].copy_from_slice(&[*x0, *y0, *dx, *dy, *inv_len2]);
                }
                PreparedShape::Radial {
                    cx,
                    cy,
                    cos,
                    sin,
                    inv_rx,
                    inv_ry,
                    inner,
                } => {
                    rec[0] = 1.0;
                    rec[3..10].copy_from_slice(&[*cx, *cy, *cos, *sin, *inv_rx, *inv_ry, *inner]);
                }
                PreparedShape::Brush(r) => {
                    rec[0] = 2.0;
                    if let Some(r) = r {
                        rec[4] = r.width as f32;
                        rec[5] = r.height as f32;
                        rec[6] = r.origin[0];
                        rec[7] = r.origin[1];
                        rec[8] = r.inv_cell;
                        rec[9] = 1.0;
                        rasters.push((values.len() + 3, r));
                    }
                }
                PreparedShape::Color { targets, lo, hi } => {
                    rec[0] = 3.0;
                    let n = targets.len().min(5);
                    rec[3] = n as f32;
                    rec[4] = *lo;
                    rec[5] = *hi;
                    for (i, t) in targets.iter().take(5).enumerate() {
                        rec[6 + 2 * i] = t[0];
                        rec[7 + 2 * i] = t[1];
                    }
                }
                PreparedShape::Luminance { lo, hi, smooth } => {
                    rec[0] = 4.0;
                    rec[3..6].copy_from_slice(&[*lo, *hi, *smooth]);
                }
            }
            values.extend_from_slice(&rec);
        }
    }
    for (at, r) in rasters {
        // f32 でちょうど表せる位置まで（2^24 個）。
        if values.len() + r.values.len() > 1 << 24 {
            return Err(unsupported(ID, "ブラシのラスターが大きすぎる"));
        }
        values[at] = values.len() as f32;
        values.extend_from_slice(&r.values);
    }
    if values.len() > 1 << 24 {
        return Err(unsupported(ID, "マスクの記録が大きすぎる"));
    }
    if values.is_empty() {
        values.extend_from_slice(&[0.0, 0.0]);
    }
    p.i32("LOCAL_OX", to_i32(ID, plan.origin.0)?)
        .i32("LOCAL_OY", to_i32(ID, plan.origin.1)?)
        .f32("LOCAL_SX", plan.scale.0)
        .f32("LOCAL_SY", plan.scale.1)
        .f32("LOCAL_GAIN", plan.exposure_gain)
        .u32("LOCAL_RANGE", u32::from(plan.uses_range))
        .u32("LOCAL_PLANES", plan.slots.count() as u32)
        .u32("LOCAL_MASKS", plan.masks.len() as u32)
        .u32("LOCAL_DATA", data as u32);
    Ok((p, f32_bytes(&values)))
}

/// ステージ 12・14 の前処理のぼかし（genzo-pipeline の `finish::frequency`。輝度の対数の面 → 横 → 縦）。
#[derive(Debug, Clone)]
pub(crate) struct BlurPrep {
    /// ぼかしの半径（画素）。
    pub(crate) radius: u32,
    /// 重み（f32 のバイト列。`2 · radius + 1` 個）。
    pub(crate) weights: Arc<Vec<u8>>,
    /// ぼかした面の範囲（仕上げのカーネルが読む範囲）。
    pub(crate) plane: Roi,
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
    env: &PrepareEnv<'_>,
) -> Result<Prepared> {
    let (displays, local) = (env.displays, env.local);
    let id = stage.id();
    match ctx.process_version {
        ProcessVersion::V1 | ProcessVersion::V2 => {}
    }
    let mut blur = None;
    let mut uses_local = false;
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
            let flat = |m: [[f32; 3]; 2]| [m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2]];
            // 歪曲補正・遠近補正（処理バージョン 2）: 倍率の表を aux の表 0 に入れる。
            let aux = match (plan.projective(), plan.warp()) {
                // 遠近補正（歪曲補正があれば同じ写像に含む。GeometryPlan::projective の map_f32 の式）。
                (Some(pr), _) => {
                    let h = pr.pre;
                    p.u32("PERSP", 1)
                        .f32s(
                            "HOM",
                            &[
                                h[0][0], h[0][1], h[0][2], h[1][0], h[1][1], h[1][2], h[2][0],
                                h[2][1], h[2][2],
                            ],
                        )
                        .f32("DLIM", pr.limit)
                        .f32s("PRE", &[0.0; 6])
                        .f32s("POST", &flat(pr.post))
                        .f32("INV_R2", pr.inv_r2);
                    match &pr.lut {
                        Some(lut) => {
                            p.u32("WARP", 1)
                                .u32s("LUT_OFF", &[0; 3])
                                .u32s("LUT_LEN", &[lut.values().len() as u32, 2, 2])
                                .f32s("LUT_INV", &[lut.inv_step(), 1.0, 1.0]);
                            Some(f32_bytes(lut.values()))
                        }
                        None => {
                            p.u32("WARP", 0)
                                .u32s("LUT_OFF", &[0; 3])
                                .u32s("LUT_LEN", &[2; 3])
                                .f32s("LUT_INV", &[1.0; 3]);
                            None
                        }
                    }
                }
                (None, Some(w)) => {
                    p.u32("PERSP", 0)
                        .f32s("HOM", &[0.0; 9])
                        .f32("DLIM", 0.0)
                        .u32("WARP", 1)
                        .f32s("PRE", &flat(w.pre))
                        .f32s("POST", &flat(w.post))
                        .f32("INV_R2", w.inv_r2)
                        .u32s("LUT_OFF", &[0; 3])
                        .u32s("LUT_LEN", &[w.lut.values().len() as u32, 2, 2])
                        .f32s("LUT_INV", &[w.lut.inv_step(), 1.0, 1.0]);
                    Some(f32_bytes(w.lut.values()))
                }
                (None, None) => {
                    p.u32("PERSP", 0)
                        .f32s("HOM", &[0.0; 9])
                        .f32("DLIM", 0.0)
                        .u32("WARP", 0)
                        .f32s("PRE", &[0.0; 6])
                        .f32s("POST", &[0.0; 6])
                        .f32("INV_R2", 0.0)
                        .u32s("LUT_OFF", &[0; 3])
                        .u32s("LUT_LEN", &[2; 3])
                        .f32s("LUT_INV", &[1.0; 3]);
                    None
                }
            };
            (Kernel::Geometry, p, aux)
        }
        GpuStageKind::Exposure => {
            let e = params.expect::<ExposureParams>(id)?;
            let mut p = Params::new(Kernel::Exposure, input, output);
            p.f32("GAIN", e.gain).f32("LOCAL_EV", e.ev);
            uses_local = set_local(&mut p, id, e.local, false, local, output)?;
            (Kernel::Exposure, p, None)
        }
        GpuStageKind::Contrast => {
            let c = params.expect::<ContrastStageParams>(id)?;
            let mut p = Params::new(Kernel::Contrast, input, output);
            p.f32("GAMMA", c.contrast.gamma).f32("LOCAL_CU", c.unit);
            uses_local = set_local(&mut p, id, c.local, c.local_only, local, output)?;
            (Kernel::Contrast, p, None)
        }
        GpuStageKind::Tone => {
            let t = params.expect::<ToneStageParams>(id)?;
            let guide = checked_guide(ctx, id)?;
            let g = &guide.image;
            let mut p = Params::new(Kernel::Tone, input, output);
            p.f32("LOCAL_CU", t.contrast_unit);
            uses_local = set_local(&mut p, id, t.local, t.local_only, local, output)?;
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
            let mut p = color_params(c, input, output);
            uses_local = set_local(&mut p, id, c.local, c.local_only, local, output)?;
            (Kernel::Color, p, None)
        }
        GpuStageKind::Texture => {
            let t = params.expect::<PresenceStageParams>(id)?;
            let (mut p, aux) = presence_params(t, ctx, id, input, output)?;
            uses_local = set_local(&mut p, id, t.local, false, local, output)?;
            if let Some(k) = &t.kernel {
                blur = Some(BlurPrep {
                    radius: k.radius,
                    weights: f32_bytes(&k.weights),
                    plane: output,
                });
            }
            (Kernel::Presence, p, Some(aux))
        }
        GpuStageKind::Sharpen => {
            let sp = params.expect::<SharpenStageParams>(id)?;
            let plane = output.expand(1);
            let mut p = Params::new(Kernel::Sharpen, input, output);
            p.f32("SH_AMOUNT", sp.amount)
                .f32("SH_THRESHOLD", sp.threshold)
                .f32("SH_LIMIT", sp.limit)
                .f32("SH_MASK", sp.mask_edge)
                .f32("SH_GSCALE", sp.gradient_scale)
                .i32("PLANE_X", plane.x)
                .i32("PLANE_Y", plane.y)
                .u32("PLANE_W", plane.width)
                .f32s("LOCAL_U3", &[sp.amount_unit, 0.0, 0.0]);
            uses_local = set_local(&mut p, id, sp.local, false, local, output)?;
            blur = Some(BlurPrep {
                radius: sp.kernel.radius,
                weights: f32_bytes(&sp.kernel.weights),
                plane,
            });
            (Kernel::Sharpen, p, None)
        }
        GpuStageKind::Effects => {
            let e = params.expect::<EffectsStageParams>(id)?;
            (Kernel::Effects, effects_params(e, input, output), None)
        }
        GpuStageKind::ProfileLookTable => {
            let t = params.expect::<LookTableParams>(id)?;
            let (p, aux) = hsv_table_params(
                t.table(),
                &t.working_to_prophoto,
                &t.prophoto_to_working,
                input,
                output,
            );
            (Kernel::HsvTable, p, Some(aux))
        }
        GpuStageKind::Look => {
            let l = params.expect::<LookParams>(id)?;
            let mut p = Params::new(Kernel::Look, input, output);
            p.mat("MAT", l.working_to_srgb.rows())
                .mat("MAT2", l.srgb_to_working.rows())
                .u32("LUT_N", l.lut.size() as u32)
                .f32s("DOMAIN_MIN", &l.lut.domain_min)
                .f32s("DOMAIN_SCALE", &l.lut.domain_scale())
                .f32("AMOUNT", l.amount);
            (Kernel::Look, p, Some(f32_bytes(l.lut.lut().as_flat())))
        }
        GpuStageKind::SceneToDisplay => {
            let s = params.expect::<SceneToDisplayParams>(id)?;
            prepare_scene_to_display(s, input, output)
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
                .u32("TRANSFER", transfer);
            (Kernel::Export, p, None)
        }
        GpuStageKind::Normalize
        | GpuStageKind::WhiteBalance
        | GpuStageKind::HighlightRecovery
        | GpuStageKind::Half2x2
        | GpuStageKind::NoiseReduction
        | GpuStageKind::LensCorrection
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
        blur,
        uses_local,
    })
}

/// HSV の表を当てるカーネル（`profile.wgsl` の `hsv_table_main`。行列 → 表 → 行列）のパラメータと補助の入力（表の点）。
/// ステージ 8 の色相彩度のマップ（[`crate::a1`]）と、ステージ 10 の後のルックテーブルで使う。
pub(crate) fn hsv_table_params(
    table: &genzo_color::HsvTable,
    before: &Mat3F32,
    after: &Mat3F32,
    input: Roi,
    output: Roi,
) -> (Params, Arc<Vec<u8>>) {
    let mut p = Params::new(Kernel::HsvTable, input, output);
    p.mat("MAT", before.rows())
        .mat("MAT2", after.rows())
        .u32s("HSV_DIMS", &table.dims())
        .u32("HSV_SRGB", u32::from(table.srgb_value_encoding()))
        .f32("HSV_HSCALE", table.hue_scale());
    (p, f32_bytes(table.as_flat()))
}

/// ステージ 12 のパラメータと補助の入力（ガイドと、その後ろのかすみの暗いチャンネル。`detail.wgsl` の
/// `presence_main`）。ガイドを使わないときの補助の入力は小さな空の表。
fn presence_params(
    t: &PresenceStageParams,
    ctx: &StageContext<'_>,
    id: &'static str,
    input: Roi,
    output: Roi,
) -> Result<(Params, Arc<Vec<u8>>)> {
    let mut flags = 0u32;
    if t.kernel.is_some() {
        flags |= 1;
    }
    let mut values: Vec<f32> = Vec::new();
    let (mut gw, mut gh, mut haze_off) = (1u32, 1u32, 0u32);
    if t.uses_guide {
        flags |= 16;
        let guide = checked_guide(ctx, id)?;
        gw = guide.image.width();
        gh = guide.image.height();
        values.extend_from_slice(guide.image.data());
        if t.uses_haze {
            flags |= 8;
            let haze = guide.haze.as_ref().ok_or_else(|| {
                GpuError::from(genzo_pipeline::PipelineError::GuideMismatch {
                    stage: id,
                    reason: "ガイドにかすみの推定がない",
                })
            })?;
            haze_off = values.len() as u32;
            values.extend_from_slice(haze.dark.data());
        }
    } else if t.uses_haze {
        return Err(GpuError::Internal(
            "かすみの除去はガイドを使う（uses_guide が偽）".to_owned(),
        ));
    }
    if values.is_empty() {
        values.extend_from_slice(&[0.0, 0.0]);
    }
    let tone = &t.tone;
    let mut p = Params::new(Kernel::Presence, input, output);
    p.u32("PR_FLAGS", flags)
        .f32("PR_TEX", t.texture)
        .f32("PR_CLA", t.clarity)
        .f32("PR_DEHAZE", t.dehaze)
        .f32s("PR_AIR", &t.airlight)
        .f32s("PR_HAZE_AIR", &t.haze_airlight)
        .f32s("LOCAL_U3", &t.units)
        .f32("LOCAL_CU", tone.contrast_unit)
        .u32("HAZE_OFF", haze_off)
        .i32("PLANE_X", output.x)
        .i32("PLANE_Y", output.y)
        .u32("PLANE_W", output.width)
        .f32("TONE_H", tone.highlights)
        .f32("TONE_S", tone.shadows)
        .f32("TONE_W", tone.whites)
        .f32("TONE_B", tone.blacks)
        .f32("TONE_EV", tone.ev)
        .f32("GAMMA", tone.contrast.gamma)
        .i32("GUIDE_OX", to_i32(id, tone.lookup.origin.0)?)
        .i32("GUIDE_OY", to_i32(id, tone.lookup.origin.1)?)
        .f32("GUIDE_SX", tone.lookup.scale.0)
        .f32("GUIDE_SY", tone.lookup.scale.1)
        .u32("GUIDE_W", gw)
        .u32("GUIDE_H", gh);
    Ok((p, f32_bytes(&values)))
}

/// ステージ 16 の効果のパラメータ（`detail.wgsl` の `effects_main`。使わない部分は 0 を入れ、`FX_FLAGS` で飛ばす）。
fn effects_params(e: &EffectsStageParams, input: Roi, output: Roi) -> Params {
    let mut flags = 0u32;
    let mut p = Params::new(Kernel::Effects, input, output);
    match &e.vignette {
        Some(v) => {
            flags |= 1;
            p.f32s("VG_CENTER", &v.center)
                .f32s("VG_INV", &v.inv_axes)
                .f32("VG_EXP", v.exponent)
                .f32("VG_INV_CORNER", v.inv_corner)
                .f32s("VG_EDGE", &v.edge)
                .f32("VG_AMOUNT", v.amount)
                .f32("VG_HL", v.highlights);
        }
        None => {
            p.f32s("VG_CENTER", &[0.0; 2])
                .f32s("VG_INV", &[0.0; 2])
                .f32("VG_EXP", 2.0)
                .f32("VG_INV_CORNER", 0.0)
                .f32s("VG_EDGE", &[0.0, 1.0])
                .f32("VG_AMOUNT", 0.0)
                .f32("VG_HL", 0.0);
        }
    }
    match &e.grain {
        Some(g) => {
            flags |= 2;
            p.u32("GR_SEED", g.seed)
                .i32s("GR_ORIGIN", &g.origin)
                .f32s("GR_INV_DENSITY", &g.inv_density)
                .f32s("GR_INV_CELL", &g.inv_cell)
                .f32s("GR_W", &g.weights)
                .f32("GR_STRENGTH", g.strength);
        }
        None => {
            p.u32("GR_SEED", 0)
                .i32s("GR_ORIGIN", &[0; 2])
                .f32s("GR_INV_DENSITY", &[1.0; 2])
                .f32s("GR_INV_CELL", &[1.0; 2])
                .f32s("GR_W", &[0.0; 2])
                .f32("GR_STRENGTH", 0.0);
        }
    }
    p.u32("GR_SALT", genzo_pipeline::finish::effects::GRAIN_SALT)
        .u32("FX_FLAGS", flags);
    p
}

/// カーネル・パラメータ・補助の入力の組。
type KernelSetup = (Kernel, Params, Option<Arc<Vec<u8>>>);

/// ステージ 13 のパラメータ（`finish.wgsl` の `color_main`。使わない部分は 0 を入れ、`COLOR_FLAGS` で飛ばす）。
fn color_params(
    c: &genzo_pipeline::finish::color::ColorStageParams,
    input: Roi,
    output: Roi,
) -> Params {
    let mut flags = 0u32;
    let mut p = Params::new(Kernel::Color, input, output);
    p.f32("SAT", c.saturation)
        .f32("VIB", c.vibrance)
        .f32("SKIN_COS", c.skin_cos)
        .f32("SKIN_SIN", c.skin_sin)
        .f32("SKIN_COS_W", c.skin_cos_width)
        .f32("NEUTRAL_A", c.neutral.0)
        .f32("NEUTRAL_B", c.neutral.1)
        .f32s("HUE_C", &c.bands.centers)
        .f32s("HUE_INV", &c.bands.inv_widths);
    match &c.mixer {
        Some(m) => {
            flags |= 1;
            p.f32s("HSL_H", &m.hue)
                .f32s("HSL_S", &m.saturation)
                .f32s("HSL_L", &m.luminance);
        }
        None => {
            p.f32s("HSL_H", &[0.0; 8])
                .f32s("HSL_S", &[0.0; 8])
                .f32s("HSL_L", &[0.0; 8]);
        }
    }
    match &c.black_white {
        Some(bw) => {
            flags |= 2;
            if bw.active {
                flags |= 4;
            }
            p.f32s("BW_MIX", &bw.mix);
        }
        None => {
            p.f32s("BW_MIX", &[0.0; 8]);
        }
    }
    match &c.grading {
        Some(g) => {
            flags |= 8;
            p.f32s("CG_TINT", g.tint.as_flattened())
                .f32s("CG_EV", &g.ev)
                .f32s(
                    "CG_ZONE",
                    &[
                        g.pivot,
                        g.inv_lower,
                        g.inv_upper,
                        g.x0,
                        g.inv_x0,
                        g.inv_x1,
                        g.sharpness,
                    ],
                );
        }
        None => {
            p.f32s("CG_TINT", &[0.0; 8])
                .f32s("CG_EV", &[0.0; 4])
                .f32s("CG_ZONE", &[0.0; 7]);
        }
    }
    if c.saturation != 0.0 || c.vibrance != 0.0 {
        flags |= 16;
    }
    p.u32("COLOR_FLAGS", flags);
    p
}

/// ステージ 15 のパラメータ（RAW は輝度のカーブの表を aux の表 0 に入れる。RAW 以外は表なし）。
fn prepare_scene_to_display(s: &SceneToDisplayParams, input: Roi, output: Roi) -> KernelSetup {
    let mut p = Params::new(Kernel::SceneToDisplay, input, output);
    let (mode, values, len, lo, inv) = match &s.lut {
        Some(l) => (
            1u32,
            l.values().to_vec(),
            l.values().len() as u32,
            l.lo(),
            l.inv_step(),
        ),
        None => (0, vec![0.0, 0.0], 2, 0.0, 1.0),
    };
    p.u32("S2D_MODE", mode)
        .f32("S2D_FLOOR_RATIO", s.floor_ratio)
        .u32s("LUT_OFF", &[0; 4])
        .u32s("LUT_LEN", &[len, 2, 2, 2])
        .f32s("LUT_LO", &[lo, 0.0, 0.0, 0.0])
        .f32s("LUT_INV", &[inv, 1.0, 1.0, 1.0]);
    (Kernel::SceneToDisplay, p, Some(f32_bytes(&values)))
}

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

/// ステージ 7（レンズ補正）のパラメータと補助の入力（周辺減光・R・B の表を aux の表 0〜2 につなぐ）。
/// `sensor_per_grid` は入力の格子の 1 画素あたりの S の画素（1 か 2）。
pub(crate) fn prepare_lens(
    lens: &genzo_pipeline::lens::LensStagePlan,
    sensor_per_grid: u32,
    grid: (u32, u32),
    input: Roi,
    output: Roi,
) -> Result<(Vec<u32>, Arc<Vec<u8>>)> {
    let k = lens.kernel_params(sensor_per_grid);
    let mut values: Vec<f32> = Vec::new();
    let (mut off, mut len, mut inv) = ([0u32; 3], [2u32; 3], [1f32; 3]);
    let mut flags = 0u32;
    let mut put = |slot: usize, lut: &Lut1d, values: &mut Vec<f32>| {
        off[slot] = values.len() as u32;
        len[slot] = lut.values().len() as u32;
        inv[slot] = lut.inv_step();
        values.extend_from_slice(lut.values());
    };
    if let Some(v) = &lens.vignetting {
        flags |= 1;
        put(0, v, &mut values);
    }
    if let Some((r, b)) = &lens.chromatic_aberration {
        flags |= 2;
        put(1, r, &mut values);
        put(2, b, &mut values);
    }
    if values.is_empty() {
        values.extend_from_slice(&[0.0, 0.0]);
    }
    let mut p = Params::new(Kernel::LensCorrection, input, output);
    p.u32("GRID_W", grid.0)
        .u32("GRID_H", grid.1)
        .f32("INV_R2", k.inv_r2)
        .u32s("LUT_OFF", &off)
        .u32s("LUT_LEN", &len)
        .f32s("LUT_INV", &inv)
        .f32s("CENTER", &k.center)
        .f32("SPG", k.sensor_per_grid)
        .u32("LFLAGS", flags);
    Ok((p.finish()?, f32_bytes(&values)))
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
            "finish.texture",
            "finish.color",
            "finish.sharpen",
            "finish.scene_to_display",
            "finish.tone_curve",
            "finish.effects",
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
        for id in ["sensor.demosaic.rcd", "sensor.demosaic.bilinear"] {
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
