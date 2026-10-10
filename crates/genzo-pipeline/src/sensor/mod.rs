//! センサー処理（docs/04_architecture.md の 2.1 節のステージ 2〜8、2.2 節の段階 A1、2.6 節の B0〜B2）。
//!
//! | ステージ | モジュール | 入力 → 出力 | ID |
//! |---|---|---|---|
//! | 2. 正規化 | [`normalize`] | RAW（u16）→ CFA（B0） | `sensor.normalize` |
//! | 3. ホワイトバランス | [`wb`] | CFA（B0）→ CFA（B1 の値） | `sensor.white_balance` |
//! | 4. ハイライト復元 | （v1。未実装。仮の処理はステージ 3 の飽和の切りそろえ） | — | — |
//! | 5. デモザイク | [`demosaic`] | CFA → RGB（B1） | `sensor.demosaic.rcd` など |
//! | 6. ノイズ軽減・7. レンズ補正 | （v1。未実装） | — | — |
//! | 8. 色変換 | [`color_matrix`] | RGB（B1）→ RGB（B2） | `sensor.color_matrix` |
//!
//! # センサーのステージのインターフェース（設計の意図）
//!
//! センサーのステージは、入出力の型がステージごとに違う（u16 の RAW → f32 の CFA → RGB）ため、
//! RGB のステージの [`crate::stage::Stage`] とは別の小さなトレイト [`SensorStage`] で表す。考え方は
//! 7.1 節と同じ（ID・入力の範囲・CPU 版・ID で探す GPU 版）で、入出力を [`SensorData`]（RAW / CFA /
//! RGB のどれか）にまとめて、ステージを同じ形で並べて実行・比較できるようにした。パラメータは、
//! 現像設定だけでなく RAW のメタデータ（黒レベル・撮影時の WB・カメラ行列）にも依存するため、
//! 7.1 節の `params` の代わりに [`SensorPlan`]（全ステージ分のパラメータ）をまとめて作る。
//! GPU 版（genzo-gpu）も [`SensorPlan`] の値（[`SensorPlan::levels`]・[`SensorPlan::wb`]・
//! [`SensorPlan::camera_to_working`] など）を uniform バッファにして使う。
//!
//! # 決めごと
//!
//! - **出力の向き**: センサー処理の出力は S（センサー）の向き。RAW の向きは G 座標の最初の変換として
//!   ステージ 9 で反映する（[`crate::coords`]）。[`A1Image::orientation`] で仕上げのステージ（[`crate::finish`]）に渡す。
//! - **カメラ行列がない場合**（`RawImage::cam_xyz` が `None`、かつ [`SensorOptions::camera_matrix`]
//!   も `None`）: エラーにはせず、カメラ RGB を **BT.2020 の原色**とみなす（`cam_xyz` の代わりに
//!   XYZ → BT.2020 の行列 [`FALLBACK_CAM_XYZ`] を使う。ステージ 8 は単位行列になり、ステージ 3 の
//!   色温度・tint の係数も同じ仮定で計算する）。色は正確ではないので、[`SensorWarning::MissingCameraMatrix`]
//!   を返し、呼び出し側（UI）が表示する。理由: 対応していない機種でも写真を開けなくしない
//!   （01 の方針「他のファイルの処理は続ける」）。厳密に扱いたい呼び出し側は、警告を見てエラーに
//!   してよい。
//! - **カメラ行列の出どころ**: 04 の 2.5 節（render_deps）の設計では、アプリのデータファイルの行列を
//!   [`SensorOptions::camera_matrix`] で渡す。**今はデータファイルがなく、呼び出し側（genzo-api・
//!   genzo-cli）は渡していない**ので、RAW の `cam_xyz`（ARW は LibRaw の内蔵の表、DNG は D65 の
//!   ColorMatrix。genzo-raw の `select_cam_xyz`）を使う。LibRaw を更新すると、同じ処理バージョン・同じ
//!   設定でも色が変わりうる（genzo-api は使った行列の出どころとハッシュ・デコーダを `render_deps` に記録し、
//!   違えば警告する。implementation_status No.25）。また `SensorOptions` はエンジン全体で 1 つで
//!   キャッシュキーに入らないため、データファイルを導入するときは、行列を写真ごと（`PhotoSource` か
//!   `render_deps` から解決）に渡す仕組みが要る。
//! - **端の外**: 各ステージの入力の端の外は鏡映で拡張する（2.7 節。[`crate::border`]）。CFA は
//!   ステージ 2 で鏡映して作り、出力の範囲が画像の外にはみ出す部分は、出力（B2）を鏡映した値に
//!   する（[`SensorPlan::render_roi`]）。タイルに分けても分けなくても、画素ごとの結果は同じ。
//! - **RAW 以外の入力**（JPEG / TIFF / PNG）: ワーカーがデコードし、ICC からリニア BT.2020（B2）に
//!   変換した画像を [`SourceImage::Working`] で受け取り、ステージ 1〜8 を飛ばす（2.6 節）。WB などの
//!   センサーの設定は使わない。
//! - **縮小**: 段階 A1 のプレビューは、出力の格子（フル解像度、簡易処理では半分）から長辺
//!   [`crate::resample::DEFAULT_PREVIEW_LONG_EDGE`] px へ面積平均で縮小する（[`crate::resample`]）。

pub mod color_matrix;
pub mod demosaic;
pub mod normalize;
pub mod wb;

use std::fmt;
use std::sync::Arc;

use genzo_color::Mat3;
use genzo_color::Mat3F32;
use genzo_color::space::XYZ_TO_BT2020;
use genzo_model::{DevelopSettings, Orientation, RenderQuality};
use genzo_raw::{CfaPattern, RawImage};
use rayon::prelude::*;

use crate::border::mirror_cover;
use crate::contract::ColorContract;
use crate::coords::{CoordinateFrames, SensorGrid};
use crate::error::{PipelineError, Result};
use crate::image::{CfaImage, ImageTile, RgbImage, Roi};
use crate::resample::{
    AreaWeights, downscale_area, downscale_column_row, downscale_row, preview_size,
};
use crate::stage::{GpuStage, GpuStageLookup, StageContext};
use crate::version::ProcessVersion;

use self::demosaic::DemosaicMethod;
use self::normalize::SensorLevels;
use self::wb::{SaturationClip, WbCoefficients};

/// カメラ行列がないときに代わりに使う「XYZ（D65）→ カメラ RGB」の行列（XYZ → BT.2020。カメラ RGB を
/// BT.2020 の原色とみなす。モジュールの doc を参照）。
pub const FALLBACK_CAM_XYZ: Mat3 = XYZ_TO_BT2020;

/// プレビューを作るときに 1 回にまとめて処理する出力の行数（並列化とメモリの単位。**仮置き**:
/// α7 IV（7008 × 4672 → 2560 × 1707）では 1 つの帯が入力の約 90 行＋余白（上下 12 行ずつ）になり、
/// 帯 1 つの作業領域は概算で数十 MB（実測していない。メモリの予算との関係は 3 段目と PoC-3 で確認する）。
/// 結果には影響しない。
pub const PREVIEW_BAND_ROWS: u32 = 32;

/// センサー処理の警告（処理は続けるが、結果が正確でないことを示す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SensorWarning {
    /// カメラ行列がないため、カメラ RGB を BT.2020 の原色とみなした（色は正確ではない）。
    MissingCameraMatrix,
}

impl fmt::Display for SensorWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SensorWarning::MissingCameraMatrix => f.write_str(
                "カメラ行列がないため、色は正確ではありません（カメラ RGB を BT.2020 とみなしました）",
            ),
        }
    }
}

/// センサー処理の設定のうち、現像設定以外のもの。
#[derive(Debug, Clone, PartialEq)]
pub struct SensorOptions {
    /// 品質。最終品質（RCD）か、WB のドラッグ中の簡易処理（2 × 2）か（2.2 節）。
    pub quality: RenderQuality,
    /// デモザイクの方式を指定する（比較・テスト用）。`None` なら処理バージョンと品質で決める
    /// （[`DemosaicMethod::for_quality`]）。
    pub demosaic: Option<DemosaicMethod>,
    /// 飽和した画素の切りそろえ（[`wb`] の doc）。
    pub saturation_clip: SaturationClip,
    /// 「XYZ（D65）→ カメラ RGB」の行列（設計の予定ではアプリのデータファイルの値。2.5 節の
    /// render_deps）。`None` なら RAW の `cam_xyz` を使う（今は呼び出し側が渡さないので、常にこちら。
    /// モジュールの doc の「カメラ行列の出どころ」）。
    pub camera_matrix: Option<Mat3>,
}

impl Default for SensorOptions {
    fn default() -> Self {
        Self {
            quality: RenderQuality::Final,
            demosaic: None,
            saturation_clip: SaturationClip::default(),
            camera_matrix: None,
        }
    }
}

/// センサーのステージの入出力の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SensorKind {
    /// RAW（段階 A0 の出力。u16）。
    Raw,
    /// CFA（f32。B0、またはステージ 3 の後の B1 の値）。
    Cfa,
    /// RGB（B1 か B2）。
    Rgb,
}

impl SensorKind {
    /// 名前（エラーの表示用）。
    pub const fn name(self) -> &'static str {
        match self {
            SensorKind::Raw => "RAW",
            SensorKind::Cfa => "CFA",
            SensorKind::Rgb => "RGB",
        }
    }
}

/// センサーのステージの入出力。
#[derive(Debug, Clone, PartialEq)]
pub enum SensorData<'r> {
    /// RAW 全体（ステージ 2 の入力。ステージが必要な範囲を取り出す）。
    Raw(&'r RawImage),
    /// CFA（範囲を持つ）。
    Cfa(CfaImage),
    /// RGB のタイル（範囲を持つ）。
    Rgb(ImageTile),
}

impl SensorData<'_> {
    /// 種類。
    pub fn kind(&self) -> SensorKind {
        match self {
            SensorData::Raw(_) => SensorKind::Raw,
            SensorData::Cfa(_) => SensorKind::Cfa,
            SensorData::Rgb(_) => SensorKind::Rgb,
        }
    }

    /// 範囲（RAW なら RAW 全体）。
    pub fn roi(&self) -> Roi {
        match self {
            SensorData::Raw(r) => Roi::full(r.width, r.height),
            SensorData::Cfa(c) => c.roi(),
            SensorData::Rgb(t) => t.roi,
        }
    }
}

/// センサーのステージ（2〜8）。RGB のステージの [`crate::stage::Stage`] に対応する、CFA を扱う
/// ステージ用の小さなトレイト（モジュールの doc の「設計の意図」）。
pub trait SensorStage: Send + Sync {
    /// 一意な ID（GPU 版の検索と一致テストのキー）。
    fn id(&self) -> &'static str;

    /// 04 の 2.1 節のステージの番号。
    fn stage_number(&self) -> u8;

    /// 入力の種類。
    fn input_kind(&self) -> SensorKind;

    /// 出力の種類。
    fn output_kind(&self) -> SensorKind;

    /// 出力の範囲を計算するのに必要な入力の範囲（ステージ 2 は RAW の上の範囲で、はみ出した部分は
    /// 鏡映で作るので、出力の範囲と同じ）。
    fn input_roi(&self, output_roi: Roi, plan: &SensorPlan) -> Roi;

    /// CPU 版の基準実装。`input` は [`input_roi`](Self::input_roi) を含み、出力は範囲 `output_roi`。
    /// 直接呼ばずに [`run_sensor_stage`] を通すこと。
    fn run_cpu<'r>(
        &self,
        ctx: &StageContext<'_>,
        plan: &SensorPlan,
        input: SensorData<'r>,
        output_roi: Roi,
    ) -> Result<SensorData<'r>>;

    /// GPU 版（ステージの ID で探す）。未実装なら `None`。
    fn gpu<'g>(&self, lookup: &'g dyn GpuStageLookup) -> Option<&'g dyn GpuStage> {
        lookup.find(self.id())
    }
}

/// センサーのステージを CPU 版で 1 回実行する（入力の種類と範囲の確認、出力の NaN・無限大の
/// 置き換え。置き換えた数を返す）。
pub fn run_sensor_stage<'r>(
    stage: &dyn SensorStage,
    ctx: &StageContext<'_>,
    plan: &SensorPlan,
    input: SensorData<'r>,
    output_roi: Roi,
) -> Result<(SensorData<'r>, u64)> {
    if input.kind() != stage.input_kind() {
        return Err(PipelineError::SensorInputMismatch {
            stage: stage.id(),
            expected: stage.input_kind().name(),
            found: input.kind().name(),
        });
    }
    if input.kind() != SensorKind::Raw {
        let need = stage.input_roi(output_roi, plan);
        if !input.roi().contains_roi(&need) {
            return Err(PipelineError::InvalidRoi {
                roi: input.roi(),
                reason: "センサーのステージの入力が必要な範囲を覆っていない",
            });
        }
    }
    let mut out = stage.run_cpu(ctx, plan, input, output_roi)?;
    if out.kind() != stage.output_kind() || out.roi() != output_roi {
        return Err(PipelineError::InvalidRoi {
            roi: out.roi(),
            reason: "センサーのステージの出力の種類か範囲が違う",
        });
    }
    let replaced = match &mut out {
        SensorData::Raw(_) => 0,
        SensorData::Cfa(c) => c.sanitize(),
        SensorData::Rgb(t) => t.image.sanitize(),
    };
    Ok((out, replaced))
}

/// 種類が違う入力のエラー。
fn mismatch(stage: &'static str, expected: SensorKind, input: &SensorData<'_>) -> PipelineError {
    PipelineError::SensorInputMismatch {
        stage,
        expected: expected.name(),
        found: input.kind().name(),
    }
}

/// ステージ 2: 正規化（[`normalize`]）。
#[derive(Debug, Clone, Copy, Default)]
pub struct NormalizeStage;

impl NormalizeStage {
    /// ステージの ID。
    pub const ID: &'static str = "sensor.normalize";
}

impl SensorStage for NormalizeStage {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn stage_number(&self) -> u8 {
        2
    }
    fn input_kind(&self) -> SensorKind {
        SensorKind::Raw
    }
    fn output_kind(&self) -> SensorKind {
        SensorKind::Cfa
    }
    fn input_roi(&self, output_roi: Roi, _plan: &SensorPlan) -> Roi {
        output_roi
    }
    fn run_cpu<'r>(
        &self,
        ctx: &StageContext<'_>,
        plan: &SensorPlan,
        input: SensorData<'r>,
        output_roi: Roi,
    ) -> Result<SensorData<'r>> {
        let SensorData::Raw(raw) = input else {
            return Err(mismatch(Self::ID, SensorKind::Raw, &input));
        };
        match ctx.process_version {
            ProcessVersion::V1 => {
                plan.check_raw(raw)?;
                Ok(SensorData::Cfa(normalize::normalize(
                    raw,
                    &plan.levels,
                    output_roi,
                )?))
            }
        }
    }
}

/// ステージ 3: ホワイトバランスと飽和の切りそろえ（[`wb`]）。
#[derive(Debug, Clone, Copy, Default)]
pub struct WhiteBalanceStage;

impl WhiteBalanceStage {
    /// ステージの ID。
    pub const ID: &'static str = "sensor.white_balance";
}

impl SensorStage for WhiteBalanceStage {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn stage_number(&self) -> u8 {
        3
    }
    fn input_kind(&self) -> SensorKind {
        SensorKind::Cfa
    }
    fn output_kind(&self) -> SensorKind {
        SensorKind::Cfa
    }
    fn input_roi(&self, output_roi: Roi, plan: &SensorPlan) -> Roi {
        output_roi.expand(plan.saturation_clip.margin())
    }
    fn run_cpu<'r>(
        &self,
        ctx: &StageContext<'_>,
        plan: &SensorPlan,
        input: SensorData<'r>,
        output_roi: Roi,
    ) -> Result<SensorData<'r>> {
        let SensorData::Cfa(cfa) = input else {
            return Err(mismatch(Self::ID, SensorKind::Cfa, &input));
        };
        match ctx.process_version {
            ProcessVersion::V1 => Ok(SensorData::Cfa(wb::apply_white_balance(
                &cfa,
                &plan.wb,
                plan.saturation_clip,
                output_roi,
            )?)),
        }
    }
}

/// ステージ 5: デモザイク（[`demosaic`]）。方式ごとに別のステージ（別の ID）として登録する。
#[derive(Debug, Clone, Copy)]
pub struct DemosaicStage {
    /// 方式。
    pub method: DemosaicMethod,
}

impl DemosaicStage {
    /// RCD（最終品質）。
    pub const RCD: DemosaicStage = DemosaicStage {
        method: DemosaicMethod::Rcd,
    };
    /// 双線形補間（比較用）。
    pub const BILINEAR: DemosaicStage = DemosaicStage {
        method: DemosaicMethod::Bilinear,
    };
    /// 2 × 2 の簡易処理（WB のドラッグ中）。
    pub const HALF_2X2: DemosaicStage = DemosaicStage {
        method: DemosaicMethod::Half2x2,
    };

    /// 方式のステージ。
    pub const fn for_method(method: DemosaicMethod) -> &'static DemosaicStage {
        match method {
            DemosaicMethod::Rcd => &Self::RCD,
            DemosaicMethod::Bilinear => &Self::BILINEAR,
            DemosaicMethod::Half2x2 => &Self::HALF_2X2,
        }
    }
}

impl SensorStage for DemosaicStage {
    fn id(&self) -> &'static str {
        self.method.stage_id()
    }
    fn stage_number(&self) -> u8 {
        5
    }
    fn input_kind(&self) -> SensorKind {
        SensorKind::Cfa
    }
    fn output_kind(&self) -> SensorKind {
        SensorKind::Rgb
    }
    fn input_roi(&self, output_roi: Roi, _plan: &SensorPlan) -> Roi {
        self.method.input_roi(output_roi)
    }
    fn run_cpu<'r>(
        &self,
        ctx: &StageContext<'_>,
        _plan: &SensorPlan,
        input: SensorData<'r>,
        output_roi: Roi,
    ) -> Result<SensorData<'r>> {
        let SensorData::Cfa(cfa) = input else {
            return Err(mismatch(self.id(), SensorKind::Cfa, &input));
        };
        match ctx.process_version {
            ProcessVersion::V1 => Ok(SensorData::Rgb(demosaic::demosaic(
                &cfa,
                self.method,
                output_roi,
            )?)),
        }
    }
}

/// ステージ 8: 色変換（[`color_matrix`]）。
#[derive(Debug, Clone, Copy, Default)]
pub struct ColorMatrixStage;

impl ColorMatrixStage {
    /// ステージの ID。
    pub const ID: &'static str = "sensor.color_matrix";
}

impl SensorStage for ColorMatrixStage {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn stage_number(&self) -> u8 {
        8
    }
    fn input_kind(&self) -> SensorKind {
        SensorKind::Rgb
    }
    fn output_kind(&self) -> SensorKind {
        SensorKind::Rgb
    }
    fn input_roi(&self, output_roi: Roi, _plan: &SensorPlan) -> Roi {
        output_roi
    }
    fn run_cpu<'r>(
        &self,
        ctx: &StageContext<'_>,
        plan: &SensorPlan,
        input: SensorData<'r>,
        output_roi: Roi,
    ) -> Result<SensorData<'r>> {
        let SensorData::Rgb(tile) = input else {
            return Err(mismatch(Self::ID, SensorKind::Rgb, &input));
        };
        match ctx.process_version {
            ProcessVersion::V1 => {
                let mut tile = if tile.roi == output_roi {
                    tile
                } else {
                    tile.crop(output_roi)?
                };
                color_matrix::apply_color_matrix(&mut tile.image, &plan.camera_to_working)?;
                Ok(SensorData::Rgb(tile))
            }
        }
    }
}

/// 組み込みのセンサーのステージ（すべて。登録表用）。
pub fn builtin_sensor_stages() -> [&'static dyn SensorStage; 6] {
    [
        &NormalizeStage,
        &WhiteBalanceStage,
        &DemosaicStage::RCD,
        &DemosaicStage::BILINEAR,
        &DemosaicStage::HALF_2X2,
        &ColorMatrixStage,
    ]
}

/// センサー処理（ステージ 2〜8）の全ステージ分のパラメータ（7.1 節の `params` に当たるもの）。
///
/// RAW のメタデータと現像設定から [`SensorPlan::new`] で作る。同じ RAW の画素データと組み合わせて
/// [`render_roi`](Self::render_roi)（タイル）や [`render_preview`](Self::render_preview)
/// （段階 A1 のプレビュー）を実行する。
#[derive(Debug, Clone, PartialEq)]
pub struct SensorPlan {
    process_version: ProcessVersion,
    quality: RenderQuality,
    sensor_width: u32,
    sensor_height: u32,
    cfa: CfaPattern,
    orientation: Orientation,
    levels: SensorLevels,
    wb: WbCoefficients,
    saturation_clip: SaturationClip,
    demosaic: DemosaicMethod,
    cam_xyz: Mat3,
    camera_to_working: Mat3F32,
    warnings: Vec<SensorWarning>,
}

impl SensorPlan {
    /// RAW のメタデータ・現像設定・設定から作る。
    ///
    /// エラー: 対応していない処理バージョン（`settings.process_version` を丸める前に確認する）、
    /// RAW の検証の失敗、2 × 2 画素より小さい RAW、カメラ行列が不正（非有限・特異など）、WB の係数を
    /// 計算できない（色温度・tint の白がカメラの色域の外など）。
    pub fn new(
        raw: &RawImage,
        settings: &DevelopSettings,
        options: &SensorOptions,
    ) -> Result<Self> {
        let process_version = ProcessVersion::from_u32(settings.process_version)?;
        raw.validate()?;
        if raw.width < 2 || raw.height < 2 {
            return Err(PipelineError::InvalidDimensions {
                width: raw.width,
                height: raw.height,
            });
        }
        let settings = settings.normalized();
        let mut warnings = Vec::new();
        let raw_matrix = raw
            .cam_xyz
            .map(|m| Mat3::from_rows(m.map(|r| r.map(f64::from))));
        let cam_xyz = match options.camera_matrix.or(raw_matrix) {
            Some(m) => m,
            None => {
                warnings.push(SensorWarning::MissingCameraMatrix);
                FALLBACK_CAM_XYZ
            }
        };
        let camera_to_working = color_matrix::camera_to_working(&cam_xyz)?;
        let multipliers =
            wb::resolve_multipliers(&settings.white_balance, raw.as_shot_wb, &cam_xyz)?;
        let wb = WbCoefficients::new(multipliers)?;
        let demosaic = options.demosaic.unwrap_or(DemosaicMethod::for_quality(
            process_version,
            options.quality,
        ));
        Ok(Self {
            process_version,
            quality: options.quality,
            sensor_width: raw.width,
            sensor_height: raw.height,
            cfa: raw.cfa,
            orientation: raw.metadata.orientation,
            levels: SensorLevels::from_raw(raw)?,
            wb,
            saturation_clip: options.saturation_clip,
            demosaic,
            cam_xyz,
            camera_to_working,
            warnings,
        })
    }

    /// 処理バージョン。
    pub fn process_version(&self) -> ProcessVersion {
        self.process_version
    }

    /// 品質。
    pub fn quality(&self) -> RenderQuality {
        self.quality
    }

    /// S の寸法（RAW の幅・高さ）。
    pub fn sensor_size(&self) -> (u32, u32) {
        (self.sensor_width, self.sensor_height)
    }

    /// 正規化の値（ステージ 2）。
    pub fn levels(&self) -> &SensorLevels {
        &self.levels
    }

    /// WB の係数（ステージ 3）。
    pub fn wb(&self) -> &WbCoefficients {
        &self.wb
    }

    /// 飽和した画素の切りそろえの方法（ステージ 3）。
    pub fn saturation_clip(&self) -> SaturationClip {
        self.saturation_clip
    }

    /// デモザイクの方式（ステージ 5）。
    pub fn demosaic(&self) -> DemosaicMethod {
        self.demosaic
    }

    /// 使ったカメラ行列（XYZ（D65）→ カメラ RGB。カメラ行列がないときは [`FALLBACK_CAM_XYZ`]）。
    pub fn cam_xyz(&self) -> &Mat3 {
        &self.cam_xyz
    }

    /// カメラ RGB → 作業色空間の行列（ステージ 8）。
    pub fn camera_to_working(&self) -> &Mat3F32 {
        &self.camera_to_working
    }

    /// 警告。
    pub fn warnings(&self) -> &[SensorWarning] {
        &self.warnings
    }

    /// 座標系（S の寸法と RAW の向き）。
    pub fn frames(&self) -> CoordinateFrames {
        CoordinateFrames {
            sensor_width: self.sensor_width,
            sensor_height: self.sensor_height,
            orientation: self.orientation,
            // 段階 A1 は回転を知らない（coords の doc）。
            rotation_deg: 0.0,
        }
    }

    /// 出力の格子（[`output_grid`](Self::output_grid)）を、ステージ 9 の入力の格子として表したもの。
    pub fn sensor_grid(&self) -> SensorGrid {
        let (gw, gh) = self.output_grid();
        let px = f64::from(self.demosaic.sensor_pixels_per_output());
        SensorGrid {
            width: gw,
            height: gh,
            extent: (f64::from(gw) * px, f64::from(gh) * px),
        }
    }

    /// 出力の格子の寸法（フル解像度、2 × 2 の簡易処理では半分）。
    pub fn output_grid(&self) -> (u32, u32) {
        self.demosaic
            .output_grid(self.sensor_width, self.sensor_height)
    }

    /// 実行するステージの並び（2 → 3 → 5 → 8）。
    pub fn stages(&self) -> [&'static dyn SensorStage; 4] {
        [
            &NormalizeStage,
            &WhiteBalanceStage,
            DemosaicStage::for_method(self.demosaic),
            &ColorMatrixStage,
        ]
    }

    /// ステージの文脈（出力の格子の解像度の倍率と品質を含む）。
    pub fn context(&self) -> StageContext<'static> {
        StageContext::new(self.process_version, self.frames())
            .with_resolution_scale(1.0 / f64::from(self.demosaic.sensor_pixels_per_output()))
            .with_quality(self.quality)
            .with_sensor_grid(self.sensor_grid())
    }

    /// RAW がこの設定を作った RAW と同じ形（寸法・CFA の並び・データの長さ）か確かめる。
    ///
    /// [`render_roi`](Self::render_roi) などが最初に呼ぶ。GPU 版（`genzo-gpu`）も同じ条件・同じエラーに
    /// するために使う。
    pub fn check_raw(&self, raw: &RawImage) -> Result<()> {
        if (raw.width, raw.height) != (self.sensor_width, self.sensor_height) {
            return Err(PipelineError::PlanMismatch("RAW の寸法が違う"));
        }
        if raw.cfa != self.cfa {
            return Err(PipelineError::PlanMismatch("RAW の CFA の配列が違う"));
        }
        if raw.data.len() as u64 != raw.pixel_count() {
            return Err(PipelineError::DataLengthMismatch {
                expected: raw.pixel_count() as usize,
                actual: raw.data.len(),
            });
        }
        Ok(())
    }

    /// 出力の格子の範囲 `roi`（画像の外にはみ出してよい）のステージ 2〜8 の結果（B2）を作る
    /// （等倍表示・書き出しのタイル。2.2 節）。置き換えた NaN・無限大の数も返す。
    ///
    /// はみ出した部分は、画像の内側の結果を鏡映した値（2.7 節）。
    pub fn render_roi(&self, raw: &RawImage, roi: Roi) -> Result<(ImageTile, u64)> {
        self.check_raw(raw)?;
        let (gw, gh) = self.output_grid();
        let rois = self.stage_rois(roi)?;
        let ctx = self.context();
        let mut data = SensorData::Raw(raw);
        let mut replaced = 0;
        // ステージ k の出力の範囲は rois[k + 1]。
        for (stage, out_roi) in self.stages().iter().zip(&rois[1..]) {
            let (next, n) = run_sensor_stage(*stage, &ctx, self, data, *out_roi)?;
            data = next;
            replaced += n;
        }
        let SensorData::Rgb(tile) = data else {
            unreachable!("ステージ 8 の出力は RGB（run_sensor_stage で確認済み）");
        };
        Ok((tile.gather_mirror(roi, gw, gh)?, replaced))
    }

    /// 出力の格子の範囲 `roi`（画像の外にはみ出してよい）を [`render_roi`](Self::render_roi) で作る
    /// ときに、各ステージが **実際に処理する** 範囲（出力側から `input_roi` をたどったもの。2.7 節）。
    ///
    /// `[0]` がステージ 2 の入力（RAW の上。画像の外の部分は鏡映で作る）、`[1]`〜`[3]` がステージ
    /// 3・5・8 の入力、`[4]` がステージ 8 の出力。`[4]` は `roi` そのものではなく、`roi` を鏡映で
    /// 折り返したときに参照する **画像の内側** の範囲（[`mirror_cover`]）で、`roi` のはみ出した部分は
    /// この結果を鏡映して作る。`roi` が空ならエラー。
    pub fn stage_rois(&self, roi: Roi) -> Result<[Roi; 5]> {
        let (gw, gh) = self.output_grid();
        let inner = mirror_cover(roi, gw, gh).ok_or(PipelineError::InvalidRoi {
            roi,
            reason: "範囲が空、または出力の格子が空",
        })?;
        let stages = self.stages();
        let mut rois = [inner; 5];
        for k in (0..stages.len()).rev() {
            rois[k] = stages[k].input_roi(rois[k + 1], self);
        }
        Ok(rois)
    }

    /// 段階 A1 のプレビュー（長辺 `long_edge` px。2.2 節）を作る。
    ///
    /// 出力の格子（フル解像度、2 × 2 の簡易処理では半分）の画像を、[`preview_size`] の寸法へ面積平均で
    /// 縮小する（拡大はしない。簡易処理で半分の格子がプレビューの寸法より小さい場合は、その寸法の
    /// まま返す）。出力の行の帯（[`PREVIEW_BAND_ROWS`]）ごとに、必要な入力の行だけを処理するので、
    /// フル解像度の RGB の画像全体を一度に持たない。
    pub fn render_preview(&self, raw: &RawImage, long_edge: u32) -> Result<A1Image> {
        self.check_raw(raw)?;
        let (gw, gh) = self.output_grid();
        let (fw, fh) = preview_size(self.sensor_width, self.sensor_height, long_edge);
        let (pw, ph) = (fw.min(gw), fh.min(gh));
        let px = self.demosaic.sensor_pixels_per_output();
        let extent = (f64::from(gw * px), f64::from(gh * px));
        let (image, replaced) = if (pw, ph) == (gw, gh) {
            let (tile, n) = self.render_roi(raw, Roi::full(gw, gh))?;
            (tile.image, n)
        } else {
            self.render_downscaled(raw, (gw, gh), (pw, ph))?
        };
        Ok(A1Image {
            image,
            sensor_size: (self.sensor_width, self.sensor_height),
            sensor_extent: extent,
            orientation: self.orientation,
            quality: self.quality,
            warnings: self.warnings.clone(),
            non_finite_replaced: replaced,
        })
    }

    /// 帯ごとにステージ 2〜8 を実行して縮小する。
    fn render_downscaled(
        &self,
        raw: &RawImage,
        (gw, gh): (u32, u32),
        (pw, ph): (u32, u32),
    ) -> Result<(RgbImage, u64)> {
        let wx = AreaWeights::new(gw, pw)?;
        let wy = AreaWeights::new(gh, ph)?;
        let row_len = pw as usize;
        let band = PREVIEW_BAND_ROWS as usize;
        let mut data = vec![[0.0f32; 3]; row_len * ph as usize];
        let counts = data
            .par_chunks_mut(row_len * band)
            .enumerate()
            .map(|(b, rows)| -> Result<u64> {
                let oy0 = (b * band) as u32;
                let oy1 = oy0 + (rows.len() / row_len) as u32;
                let (iy0, iy1) = wy.source_range(oy0, oy1);
                let roi = Roi::new(0, iy0 as i32, gw, iy1 - iy0);
                let (tile, n) = self.render_roi(raw, roi)?;
                // 横方向（入力の行ごと）。
                let mut tmp = vec![[0.0f32; 3]; (iy1 - iy0) as usize * row_len];
                for (j, out) in tmp.chunks_mut(row_len).enumerate() {
                    downscale_row(&wx, tile.image.row(j as u32), out);
                }
                // 縦方向（出力の行ごと）。
                for (k, out) in rows.chunks_mut(row_len).enumerate() {
                    downscale_column_row(
                        &wy,
                        oy0 + k as u32,
                        |j| {
                            let r = (j - iy0) as usize;
                            &tmp[r * row_len..(r + 1) * row_len]
                        },
                        out,
                    );
                }
                Ok(n)
            })
            .collect::<Result<Vec<u64>>>()?;
        let image = RgbImage::from_vec(pw, ph, ColorContract::B2Working, data)?;
        Ok((image, counts.into_iter().sum()))
    }
}

/// 段階 A1 の出力（プレビュー解像度の作業色空間の画像。2.2 節）。3 段目（エンジン）がキャッシュし、
/// 仕上げのステージ（[`crate::finish`]）とガイド（[`crate::guide`]）の入力になる。
#[derive(Debug, Clone, PartialEq)]
pub struct A1Image {
    /// 画像（B2。S の向き。向きは [`orientation`](Self::orientation) で、ステージ 9 が反映する）。
    pub image: RgbImage,
    /// S の寸法（フル解像度。RAW 以外の入力では元の画像の寸法）。
    pub sensor_size: (u32, u32),
    /// この画像が覆う S の範囲の幅・高さ（左上は S の (0, 0)）。画像の四隅をこの範囲の四隅に合わせて
    /// 対応させる。2 × 2 の簡易処理で S の幅・高さが奇数なら、最後の列・行の分だけ小さい。
    pub sensor_extent: (f64, f64),
    /// RAW の向き（G 座標の最初の変換。[`crate::coords`]）。
    pub orientation: Orientation,
    /// 品質（簡易か最終か）。
    pub quality: RenderQuality,
    /// 警告。
    pub warnings: Vec<SensorWarning>,
    /// 置き換えた NaN・無限大の数（2.6 節。ログ用）。
    pub non_finite_replaced: u64,
}

impl A1Image {
    /// この画像の 1 画素が覆う S の画素の数（横, 縦）。
    pub fn sensor_pixels_per_pixel(&self) -> (f64, f64) {
        (
            self.sensor_extent.0 / f64::from(self.image.width()),
            self.sensor_extent.1 / f64::from(self.image.height()),
        )
    }

    /// 座標系（S の寸法と向き）。
    pub fn frames(&self) -> CoordinateFrames {
        CoordinateFrames {
            sensor_width: self.sensor_size.0,
            sensor_height: self.sensor_size.1,
            orientation: self.orientation,
            // 段階 A1 は回転を知らない（coords の doc）。回転は段階 B・C の文脈で加える。
            rotation_deg: 0.0,
        }
    }

    /// この画像を、ステージ 9 の入力の格子として表したもの（寸法と、覆う S の範囲）。
    pub fn sensor_grid(&self) -> SensorGrid {
        SensorGrid {
            width: self.image.width(),
            height: self.image.height(),
            extent: self.sensor_extent,
        }
    }
}

/// 現像の入力（2.6 節「RAW 以外の入力」）。
#[derive(Debug, Clone, PartialEq)]
pub enum SourceImage {
    /// RAW（段階 A0 の出力。本体が検証したもの）。ステージ 2〜8 を通す。
    Raw(Arc<RawImage>),
    /// RAW 以外（JPEG / TIFF / PNG）。ワーカーがデコードし、埋め込みの ICC（なければ sRGB）から
    /// リニア BT.2020（B2）に変換した画像。ステージ 1〜8 を飛ばす。画素は向きを反映する前のもので、
    /// 向きは `orientation`（RAW と同じく G 座標の最初の変換で反映する）。
    Working {
        /// B2 の画像。
        image: Arc<RgbImage>,
        /// EXIF の向き。
        orientation: Orientation,
    },
}

impl SourceImage {
    /// S の寸法。
    pub fn sensor_size(&self) -> (u32, u32) {
        match self {
            SourceImage::Raw(r) => (r.width, r.height),
            SourceImage::Working { image, .. } => image.dimensions(),
        }
    }

    /// 向き。
    pub fn orientation(&self) -> Orientation {
        match self {
            SourceImage::Raw(r) => r.metadata.orientation,
            SourceImage::Working { orientation, .. } => *orientation,
        }
    }
}

/// 入力から段階 A1 のプレビュー（長辺 `long_edge` px、B2）を作る（2.2 節）。
///
/// - RAW: [`SensorPlan::new`] → [`SensorPlan::render_preview`]。
/// - RAW 以外: 処理バージョンを確かめ、NaN・無限大を 0 にして、面積平均で縮小する。
pub fn render_a1(
    source: &SourceImage,
    settings: &DevelopSettings,
    options: &SensorOptions,
    long_edge: u32,
) -> Result<A1Image> {
    match source {
        SourceImage::Raw(raw) => {
            SensorPlan::new(raw, settings, options)?.render_preview(raw, long_edge)
        }
        SourceImage::Working { image, orientation } => {
            ProcessVersion::from_u32(settings.process_version)?;
            if image.contract() != ColorContract::B2Working {
                return Err(PipelineError::ContractMismatch {
                    stage: "sensor.working_input",
                    expected: ColorContract::B2Working,
                    found: image.contract(),
                });
            }
            let (w, h) = image.dimensions();
            let (pw, ph) = preview_size(w, h, long_edge);
            // NaN・無限大は縮小の前に 0 にする（縮小の後では周りの画素の平均まで 0 になる）。入力は
            // ふつう変換の時点で置き換え済み（crate::input）なので、ある場合だけ複製する（フル解像度の
            // 画像をもう 1 枚持たないため。SCL-05）。
            let non_finite = image.as_flat().iter().filter(|v| !v.is_finite()).count() as u64;
            let (image, replaced) = if non_finite == 0 {
                (downscale_area(image, pw, ph)?, 0)
            } else {
                let mut clean = (**image).clone();
                let replaced = clean.sanitize();
                (downscale_area(&clean, pw, ph)?, replaced)
            };
            Ok(A1Image {
                image,
                sensor_size: (w, h),
                sensor_extent: (f64::from(w), f64::from(h)),
                orientation: *orientation,
                quality: RenderQuality::Final,
                warnings: Vec::new(),
                non_finite_replaced: replaced,
            })
        }
    }
}
