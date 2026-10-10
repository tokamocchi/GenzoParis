//! エンジン: 段階 A0 / A1 / B / C の実行とキャッシュ、プレビュー、等倍・書き出しのタイル処理
//! （docs/04_architecture.md の 2.2 節・2.4 節・2.7 節・4.1 節、6.1 節・6.2 節。01 の SYS-03・PRV-02・
//! PRV-03・EXP-01、02 の SCL-05）。
//!
//! # 段階とキャッシュ（2.2 節）
//!
//! | 段階 | このモジュール | キャッシュ（[`cache`]） | 作り直す条件 |
//! |---|---|---|---|
//! | A0: RAW の展開 | [`Engine::load_source`]（展開は呼び出し側の関数。RAW は `Arc` で保持） | 最新 [`EngineConfig::a0_capacity`] 件 | 元ファイル・処理バージョン・RAW デコーダ |
//! | A1: センサー処理（2〜8） | [`Engine::a1`] → [`crate::render_a1`] | 最新 [`EngineConfig::a1_capacity`] 件 | `hash_for_phase(A1)`（WB など）・品質・長辺 |
//! | B: ガイド | [`Engine::guide`] → [`crate::compute_guide`] | 最新 [`EngineConfig::guide_capacity`] 件 | `hash_for_phase(B)`（A1 の項目＋回転）・ガイドのパラメータ・入力の A1 |
//! | C: 仕上げ（9〜17） | [`Engine::render_preview`]・[`FullResolutionJob`] | なし（毎回計算） | — |
//!
//! 露光量・トーン・カーブなど段階 C の項目だけを変えたときは、A1 とガイドはキャッシュから使う
//! （ガイドは露光量を含まない。2.7 節）。WB を変えたときは A0 を保持したまま A1（とガイド）を
//! 作り直す（レビュー R-04）。ヒット・ミスの回数は [`Engine::stats`] で取れる。
//!
//! # プレビュー（SYS-03・PRV-02。2.2 節・6.2 節）
//!
//! [`Engine::render_preview`]: A1（長辺 `long_edge`、既定 2560px）→ ガイド（必要なときだけ）→
//! ステージ 9〜16 → B3。[`PreviewRequest`] で 17a（画面）・17b（書き出し・キャッシュ B5）と 17c
//! （ヒストグラム）も求められる。
//!
//! - **品質**: WB のスライダーをドラッグしている間は [`RenderQuality::Draft`]（2 × 2 の簡易処理）、
//!   指を離したら [`RenderQuality::Final`]（RCD）で描き直す（6.2 節・PERF-01b）。RAW 以外の入力は
//!   センサー処理がないので、常に最終品質として扱う。
//! - **ガイド**: 有効なステージがガイドを必要とする（トーンのスライダーが 0 でない）ときだけ作る
//!   （ガイドなしで [`FinishPlan::new`] が [`PipelineError::GuideMismatch`] を返したら作って作り直す）。
//!   プレビューのガイドは、そのプレビューの A1（同じ品質・長辺）から作る。ガイドは A1 の項目（WB など）に
//!   依存するので、トーンを使っているときは WB のドラッグの 1 コマごとにガイドも作り直す（このコンテナの
//!   CPU で長辺 2560 の A1 から約 70ms。参考値）。ドラッグ中は前のガイドを使い回すか（近似）は、
//!   PERF-01b の計測とあわせて PoC-3 で決める。
//!
//! # 等倍・書き出し（PRV-03・EXP-01。2.2 節・2.4 節・2.7 節）
//!
//! [`Engine::full_resolution`] で [`FullResolutionJob`] を作り、出力（C の格子、フル解像度）を
//! タイル（既定 [`DEFAULT_TILE_SIZE`]² ＋ 余白）に分けて段階 A1〜C を実行する（[`full`]）。
//!
//! - **余白**: タイルごとに、ステージ 9〜16 の `input_roi` を出力側から順にたどり（[`FinishPlan::stage_rois`]）、
//!   さらにセンサーのステージ 8 → 5 → 3 → 2 の `input_roi` をたどる（[`crate::SensorPlan::render_roi`]）。
//!   画像の端の外は鏡映（2.7 節）。たどった範囲は [`FullResolutionJob::tile_rois`] で確かめられる。
//! - **ガイド**: プレビューと同じもの（長辺 [`EngineConfig::preview_long_edge`] の最終品質の A1 から
//!   作った、G の全体の 512px のガイド）を使う（2.7 節）。等倍の処理は常に最終品質。
//! - **メモリ**（SCL-05）: フル解像度のバッファは持たない。タイルを 1 つずつ処理し（各ステージの中は
//!   rayon で並列）、結果をすぐ出力（ルーペの範囲・量子化した書き出しのバッファ・縮小の帯）に書く。
//! - **取り消し**: タイルを処理する前ごとに [`RenderControl`] を確認する（6.1 節）。
//! - **書き出しの縮小**（2.4 節「フル解像度で処理してから縮小」）: B3（リニア）のタイルを、面積平均
//!   （[`crate::resample`] と同じ定義・同じ足し方）で横・縦の順に縮小し、帯ごとに 17b と量子化を行う。
//!   結果は「フル解像度の B3 全体を [`crate::downscale_area`] で縮小したもの」とビット単位で同じ。
//!
//! # 例
//!
//! ```
//! use std::sync::Arc;
//!
//! use genzo_model::{DevelopSettings, ExportSettings, ExportSize, FileId, Orientation, RenderQuality};
//! use genzo_pipeline::{
//!     ColorContract, Engine, EngineConfig, ExportOptions, ExportPixels, NeverCancel, PhotoSource,
//!     RgbImage, SourceId, SourceImage,
//! };
//!
//! // RAW 以外の入力（B2 の 120 × 80 の画像）。
//! let image = RgbImage::filled(120, 80, ColorContract::B2Working, [0.18; 3])?;
//! let source = PhotoSource::new(
//!     SourceId::new(FileId::new(1), 1),
//!     SourceImage::Working { image: Arc::new(image), orientation: Orientation::Normal },
//! )?;
//! let engine = Engine::new(EngineConfig { preview_long_edge: 60, tile_size: 64, ..Default::default() })?;
//! let mut settings = DevelopSettings::default();
//! settings.tone.shadows = 30.0;
//! // プレビュー（A1・ガイドはキャッシュされる）。
//! let preview = engine.render_preview(&source, &settings, RenderQuality::Final, 60)?;
//! assert_eq!(preview.b3.image.dimensions(), (60, 40));
//! // 露光量だけを変えても、A1 とガイドは作り直さない。
//! settings.exposure_ev = 0.5;
//! engine.render_preview(&source, &settings, RenderQuality::Final, 60)?;
//! assert_eq!((engine.stats().a1.misses, engine.stats().guide.misses), (1, 1));
//! // 書き出し（長辺 100px、sRGB の 8bit。フル解像度で処理してから縮小）。
//! let export = ExportSettings { size: ExportSize::LongEdge(100), ..Default::default() };
//! let out = engine.export(&source, &settings, &export, &ExportOptions::default(), &NeverCancel)?;
//! assert_eq!((out.width, out.height), (100, 67));
//! assert!(matches!(out.pixels, ExportPixels::Rgb8(ref v) if v.len() == 100 * 67 * 3));
//! assert!(!out.icc.as_bytes().is_empty());
//! # Ok::<(), genzo_pipeline::PipelineError>(())
//! ```
//!
//! # 並列化と決定性
//!
//! 画素ごとの計算は、C の格子の全体での座標だけに依存する（タイルの分け方・スレッド数によらない）。
//! 縮小の和はタイルを左から順に、入力の番号の小さい順に足すので、分けない縮小と同じ順序になる。
//! キャッシュは `Mutex` で守り、計算はロックの外で行う（同時に同じキーを計算した場合は、両方が
//! ミスとして数えられ、後から入れた方が残る。結果は同じ）。

pub mod cache;
pub mod control;
pub mod export;
pub mod full;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use genzo_color::RgbColorSpace;
use genzo_model::{
    CacheColorSpace, CacheFormat, CacheKey, CacheKind, DevelopSettings, FileId, OutputColorSpace,
    Phase, RenderQuality,
};

pub use self::cache::{A0Key, CacheCounters, GuideKey, GuideParamsKey};
pub use self::control::{NeverCancel, RenderControl};
pub use self::export::{
    EXPORT_ICC_VERSION, ExportOptions, ExportPixels, ExportedImage, export_icc_profile,
};
pub use self::full::{FullResolutionJob, RegionRender, StageBackend, TileRois};

use self::cache::Lru;
use crate::border::mirror_index;
use crate::contract::ColorContract;
use crate::coords::CoordinateFrames;
use crate::error::{PipelineError, Result};
use crate::finish::geometry::GeometryPlan;
use crate::finish::output::{DisplayTransform, output_space};
use crate::finish::{
    DisplayStage, ExportStage, FinishPlan, Histogram, UnimplementedSetting, unimplemented_settings,
};
use crate::guide::{GUIDE_FORMAT_VERSION, GuideSpec, compute_guide, guide_size};
use crate::image::{ImageTile, RgbImage, Roi, checked_len};
use crate::resample::DEFAULT_PREVIEW_LONG_EDGE;
use crate::sensor::{A1Image, SensorOptions, SensorWarning, SourceImage, render_a1};
use crate::stage::{Guide, Stage, StageContext, run_stage_cpu};
use crate::version::ProcessVersion;

/// 等倍・書き出しのタイルの一辺の既定値（px。2.2 節の「例：1024 × 1024 ＋ 周辺の余白」）。
/// **仮置き**: GPU へ一度に投入する量（6.1 節「1〜2 タイル分まで」）と P0 の割り込みの遅れ、
/// 余白の割合（RCD とバイキュービックで片側十数画素）の釣り合いで PoC-3 で決める。結果には影響しない。
pub const DEFAULT_TILE_SIZE: u32 = 1024;

/// A0（展開した RAW）を保持する件数の既定値。**仮置き**: 開いている写真と、先読み（6.1 節の P2）の
/// 1 枚。α7 IV で 1 件 約 65MB（2.2 節）。
pub const DEFAULT_A0_CAPACITY: usize = 2;

/// A1（プレビュー解像度の作業色空間の画像）を保持する件数の既定値。**仮置き**: 最終品質と簡易処理の
/// 2 件 × 2 枚。長辺 2560px で 1 件 約 52MB（f32 の RGB。2.2 節の表は GPU 上の 16 バイト/画素で
/// 約 70MB）。メモリの予算（SCL-05）との関係は PoC-3 で確認する。
pub const DEFAULT_A1_CAPACITY: usize = 4;

/// ガイドを保持する件数の既定値。**仮置き**: 1 件は長辺 512px の f32 で 1MB 未満。回転を戻したときに
/// 作り直さずに済むよう、A1 と同じ件数にした。
pub const DEFAULT_GUIDE_CAPACITY: usize = 4;

/// A1 のキャッシュの保存形式のバージョン（`CacheFormat::format_version`）。
pub const A1_FORMAT_VERSION: u32 = 1;

/// 元ファイルの識別（キャッシュのキーの `file_id`・`file_revision`。4.1 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceId {
    /// 元ファイル。
    pub file_id: FileId,
    /// 元ファイルの内容のリビジョン（内容が変わったら +1。3.3 節）。
    pub file_revision: u32,
}

impl SourceId {
    /// 作る。
    pub const fn new(file_id: FileId, file_revision: u32) -> Self {
        Self {
            file_id,
            file_revision,
        }
    }
}

/// 現像する写真の入力（段階 A0 の結果と、その識別）。複製は `Arc` を数えるだけで安い。
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoSource {
    /// 識別（キャッシュのキー）。
    pub id: SourceId,
    /// 入力（RAW、または RAW 以外の B2 の画像。[`crate::input`]）。
    pub image: SourceImage,
}

impl PhotoSource {
    /// 作る。RAW は検証し（2 × 2 画素以上）、RAW 以外は B2 の画像であることを確かめる。
    pub fn new(id: SourceId, image: SourceImage) -> Result<Self> {
        match &image {
            SourceImage::Raw(raw) => {
                raw.validate()?;
                if raw.width < 2 || raw.height < 2 {
                    return Err(PipelineError::InvalidDimensions {
                        width: raw.width,
                        height: raw.height,
                    });
                }
            }
            SourceImage::Working { image, .. } => {
                if image.contract() != ColorContract::B2Working {
                    return Err(PipelineError::ContractMismatch {
                        stage: "engine.source",
                        expected: ColorContract::B2Working,
                        found: image.contract(),
                    });
                }
            }
        }
        Ok(Self { id, image })
    }

    /// 実際に使う品質（RAW 以外はセンサー処理がないので常に最終品質）。
    pub fn effective_quality(&self, quality: RenderQuality) -> RenderQuality {
        match self.image {
            SourceImage::Raw(_) => quality,
            SourceImage::Working { .. } => RenderQuality::Final,
        }
    }

    /// 座標系（S の寸法・向き。回転なし）。
    pub fn frames(&self) -> Result<CoordinateFrames> {
        let (w, h) = self.image.sensor_size();
        CoordinateFrames::new(w, h, self.image.orientation())
    }
}

/// エンジンの設定。
#[derive(Debug, Clone, PartialEq)]
pub struct EngineConfig {
    /// 等倍・書き出しで使うガイドの入力（最終品質の A1）の長辺（px）。プレビューを同じ長辺で描けば、
    /// プレビューと等倍で同じガイドを使う（2.7 節）。既定は [`DEFAULT_PREVIEW_LONG_EDGE`]。
    pub preview_long_edge: u32,
    /// 等倍・書き出しのタイルの一辺（px）。
    pub tile_size: u32,
    /// A0 を保持する件数（1 以上）。
    pub a0_capacity: usize,
    /// A1 を保持する件数（1 以上）。
    pub a1_capacity: usize,
    /// ガイドを保持する件数（1 以上）。
    pub guide_capacity: usize,
    /// センサー処理の設定（飽和の切りそろえ・カメラ行列）。`quality` は呼び出しごとの品質で
    /// 上書きし、`demosaic`（方式の指定。比較用）は最終品質のときだけ使う。この設定はキャッシュの
    /// キーに含めないので、変えるときはエンジンを作り直す。
    pub sensor: SensorOptions,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            preview_long_edge: DEFAULT_PREVIEW_LONG_EDGE,
            tile_size: DEFAULT_TILE_SIZE,
            a0_capacity: DEFAULT_A0_CAPACITY,
            a1_capacity: DEFAULT_A1_CAPACITY,
            guide_capacity: DEFAULT_GUIDE_CAPACITY,
            sensor: SensorOptions::default(),
        }
    }
}

impl EngineConfig {
    /// 値を検証する（長辺・タイルの一辺・件数が 1 以上）。
    pub fn validate(&self) -> Result<()> {
        if self.preview_long_edge == 0 || self.tile_size == 0 {
            return Err(PipelineError::InvalidArgument(format!(
                "プレビューの長辺とタイルの一辺は 1 以上が必要です（{}、{}）",
                self.preview_long_edge, self.tile_size
            )));
        }
        if self.a0_capacity == 0 || self.a1_capacity == 0 || self.guide_capacity == 0 {
            return Err(PipelineError::InvalidArgument(
                "キャッシュの件数は 1 以上が必要です".to_owned(),
            ));
        }
        Ok(())
    }
}

/// エンジンの回数の記録（テスト・ログ用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EngineStats {
    /// 段階 A0（[`Engine::load_source`]）。ミスは展開の関数を呼んだ回数。
    pub a0: CacheCounters,
    /// 段階 A1。
    pub a1: CacheCounters,
    /// 段階 B（ガイド）。
    pub guide: CacheCounters,
    /// プレビューの段階 C を実行した回数。
    pub previews_rendered: u64,
    /// 等倍・書き出しのタイルを処理した回数。
    pub tiles_rendered: u64,
    /// いまキャッシュにある件数（A0, A1, ガイド）。
    pub cached: [usize; 3],
}

/// ステージ 17 の出力先（プレビュー・ルーペで使う）。
#[derive(Debug, Clone)]
pub enum OutputTarget {
    /// 17a 画面（B4a。モニターのプロファイルの変換）。
    Display(Arc<DisplayTransform>),
    /// 17b 書き出し（B4b。出力の色空間）。
    Export(OutputColorSpace),
    /// 17b キャッシュ L0 / L1（B5。Display P3）。
    CacheDisplayP3,
}

impl OutputTarget {
    /// モニターのプロファイルが取得できないとき（sRGB とみなす）の画面への出力。
    pub fn assumed_srgb_display() -> Result<Self> {
        Ok(Self::Display(DisplayTransform::assumed_srgb()?))
    }

    /// ヒストグラムの輝度の係数（画面はモニターの原色がわからないので sRGB の係数。
    /// [`Histogram::luma_coefficients`] の doc）。
    pub fn luma_coefficients(&self) -> [f32; 3] {
        let space = match self {
            OutputTarget::Display(_) => RgbColorSpace::Srgb,
            OutputTarget::Export(s) => output_space(*s),
            OutputTarget::CacheDisplayP3 => RgbColorSpace::DisplayP3,
        };
        Histogram::luma_coefficients(space)
    }

    /// 出力の色の契約。
    pub fn contract(&self) -> ColorContract {
        match self {
            OutputTarget::Display(_) => ColorContract::B4aDisplay,
            OutputTarget::Export(_) => ColorContract::B4bExport,
            OutputTarget::CacheDisplayP3 => ColorContract::B5Cache,
        }
    }

    /// B3 のタイルにステージ 17a / 17b を適用する（範囲はタイルと同じ。置き換えた NaN・無限大の数も返す）。
    pub fn apply(
        &self,
        settings: &DevelopSettings,
        ctx: &StageContext<'_>,
        b3: &ImageTile,
    ) -> Result<(ImageTile, u64)> {
        let run = |stage: &dyn Stage| -> Result<(ImageTile, u64)> {
            let params = stage
                .params(settings, ctx)?
                .ok_or(PipelineError::InvalidArgument(format!(
                    "出力のステージ {} が無効になっています",
                    stage.id()
                )))?;
            run_stage_cpu(stage, ctx, b3, b3.roi, &params)
        };
        match self {
            OutputTarget::Display(t) => run(&DisplayStage::new(Arc::clone(t))),
            OutputTarget::Export(s) => run(ExportStage::for_space(*s)),
            OutputTarget::CacheDisplayP3 => run(&ExportStage::CACHE_DISPLAY_P3),
        }
    }
}

/// プレビューの要求。
#[derive(Debug, Clone)]
pub struct PreviewRequest {
    /// 品質（WB のドラッグ中は簡易、離したら最終。6.2 節）。
    pub quality: RenderQuality,
    /// A1 の長辺（px。既定 2560）。
    pub long_edge: u32,
    /// ステージ 17 の出力先（`None` なら B3 だけ）。
    pub output: Option<OutputTarget>,
    /// 17c のヒストグラムを求めるか（`output` が必要。出力の符号化した値で数える）。
    pub histogram: bool,
}

impl PreviewRequest {
    /// B3 だけを求める要求。
    pub fn new(quality: RenderQuality, long_edge: u32) -> Self {
        Self {
            quality,
            long_edge,
            output: None,
            histogram: false,
        }
    }

    /// ステージ 17 の出力先を付ける。
    pub fn with_output(mut self, output: OutputTarget) -> Self {
        self.output = Some(output);
        self
    }

    /// ヒストグラムも求める。
    pub fn with_histogram(mut self) -> Self {
        self.histogram = true;
        self
    }
}

/// プレビューの結果。
#[derive(Debug, Clone)]
pub struct PreviewRender {
    /// ステージ 16 の出力（B3。C の格子のプレビューの密度。範囲は `(0, 0)` から出力の寸法）。
    pub b3: ImageTile,
    /// ステージ 17a / 17b の出力（要求したとき）。
    pub output: Option<RgbImage>,
    /// 17c のヒストグラム（要求したとき）。
    pub histogram: Option<Histogram>,
    /// 実際の品質。
    pub quality: RenderQuality,
    /// ジオメトリ（切り抜きの位置 [`GeometryPlan::crop_lattice`] など）。
    pub geometry: GeometryPlan,
    /// 有効だったステージの ID（処理の順）。
    pub active_stages: Vec<&'static str>,
    /// ガイドを使ったか。
    pub used_guide: bool,
    /// センサー処理の警告。
    pub warnings: Vec<SensorWarning>,
    /// 値が入っているが適用しなかった項目（v1。[`unimplemented_settings`]）。
    pub unimplemented: Vec<UnimplementedSetting>,
    /// 置き換えた NaN・無限大の数（A1 を含む。2.6 節）。
    pub non_finite_replaced: u64,
}

/// エンジン（段階 A0〜C の実行とキャッシュ）。スレッド間で共有できる（`&self` のメソッド）。
#[derive(Debug)]
pub struct Engine {
    config: EngineConfig,
    state: Mutex<EngineState>,
    tiles_rendered: Arc<AtomicU64>,
}

#[derive(Debug)]
struct EngineState {
    a0: Lru<A0Key, SourceImage>,
    a1: Lru<CacheKey, A1Image>,
    guide: Lru<GuideKey, Guide>,
    previews_rendered: u64,
}

impl Engine {
    /// 設定を検証して作る。
    pub fn new(config: EngineConfig) -> Result<Self> {
        config.validate()?;
        let state = EngineState {
            a0: Lru::new(config.a0_capacity),
            a1: Lru::new(config.a1_capacity),
            guide: Lru::new(config.guide_capacity),
            previews_rendered: 0,
        };
        Ok(Self {
            config,
            state: Mutex::new(state),
            tiles_rendered: Arc::new(AtomicU64::new(0)),
        })
    }

    /// 設定。
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    fn lock(&self) -> MutexGuard<'_, EngineState> {
        // 計算はロックの外で行うので、パニックで毒された状態でも中身（LRU と回数）は壊れていない。
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 回数。
    pub fn stats(&self) -> EngineStats {
        let s = self.lock();
        EngineStats {
            a0: s.a0.counters(),
            a1: s.a1.counters(),
            guide: s.guide.counters(),
            previews_rendered: s.previews_rendered,
            tiles_rendered: self.tiles_rendered.load(Ordering::Relaxed),
            cached: [s.a0.len(), s.a1.len(), s.guide.len()],
        }
    }

    /// 回数を 0 にする（キャッシュの中身は残す）。
    pub fn reset_stats(&self) {
        let mut s = self.lock();
        s.a0.reset_counters();
        s.a1.reset_counters();
        s.guide.reset_counters();
        s.previews_rendered = 0;
        self.tiles_rendered.store(0, Ordering::Relaxed);
    }

    /// キャッシュの中身を捨てる（回数は残す）。
    pub fn clear_caches(&self) {
        let mut s = self.lock();
        s.a0.clear();
        s.a1.clear();
        s.guide.clear();
    }

    /// 品質に合わせたセンサー処理の設定。
    fn sensor_options(&self, quality: RenderQuality) -> SensorOptions {
        SensorOptions {
            quality,
            demosaic: match quality {
                RenderQuality::Final => self.config.sensor.demosaic,
                RenderQuality::Draft => None,
            },
            ..self.config.sensor.clone()
        }
    }

    // -----------------------------------------------------------------------
    // 段階 A0
    // -----------------------------------------------------------------------

    /// 段階 A0: 元ファイル `id` の入力を返す。キャッシュになければ `load`（RAW の展開。対話用
    /// ワーカーで行い、共有メモリで受け取ったもの。1.2 節）を呼んで保持する。
    ///
    /// キーは元ファイルと `hash_for_phase(A0)`（処理バージョン・RAW デコーダ）なので、WB などを
    /// 変えても展開し直さない（レビュー R-04）。未知の処理バージョンはエラー。
    pub fn load_source(
        &self,
        id: SourceId,
        settings: &DevelopSettings,
        load: impl FnOnce() -> Result<SourceImage>,
    ) -> Result<PhotoSource> {
        ProcessVersion::from_u32(settings.process_version)?;
        let key = A0Key {
            source: id,
            develop_hash: settings.hash_for_phase(Phase::A0),
        };
        if let Some(image) = self.lock().a0.lookup(&key) {
            return Ok(PhotoSource {
                id,
                image: (*image).clone(),
            });
        }
        let source = PhotoSource::new(id, load()?)?;
        self.lock().a0.insert(key, Arc::new(source.image.clone()));
        Ok(source)
    }

    // -----------------------------------------------------------------------
    // 段階 A1
    // -----------------------------------------------------------------------

    /// 段階 A1 のキャッシュのキー（genzo-model の `CacheKey`。`size` は `(長辺, 0)`）。
    pub fn a1_cache_key(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        quality: RenderQuality,
        long_edge: u32,
    ) -> CacheKey {
        CacheKey::for_settings(
            source.id.file_id,
            source.id.file_revision,
            settings,
            (long_edge, 0),
            source.effective_quality(quality),
            CacheFormat {
                kind: CacheKind::A1Intermediate,
                color_space: CacheColorSpace::LinearRec2020,
                format_version: A1_FORMAT_VERSION,
            },
        )
    }

    /// 段階 A1: 長辺 `long_edge` px のプレビュー解像度の作業色空間の画像（B2。キャッシュする）。
    pub fn a1(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        quality: RenderQuality,
        long_edge: u32,
    ) -> Result<Arc<A1Image>> {
        ProcessVersion::from_u32(settings.process_version)?;
        if long_edge == 0 {
            return Err(PipelineError::InvalidArgument(
                "プレビューの長辺は 1 以上が必要です".to_owned(),
            ));
        }
        let quality = source.effective_quality(quality);
        let key = self.a1_cache_key(source, settings, quality, long_edge);
        if let Some(a1) = self.lock().a1.lookup(&key) {
            return Ok(a1);
        }
        let a1 = Arc::new(render_a1(
            &source.image,
            settings,
            &self.sensor_options(quality),
            long_edge,
        )?);
        self.lock().a1.insert(key, Arc::clone(&a1));
        Ok(a1)
    }

    // -----------------------------------------------------------------------
    // 段階 B
    // -----------------------------------------------------------------------

    /// 段階 B のキャッシュのキー（2.7 節の表。入力の A1 の品質・長辺を含む）。
    pub fn guide_cache_key(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        quality: RenderQuality,
        long_edge: u32,
    ) -> Result<GuideKey> {
        let spec = GuideSpec::from_settings(settings)?;
        let frames = source.frames()?.with_rotation(spec.rotation_deg)?;
        let size = guide_size(&frames, spec.params.long_edge);
        let cache = CacheKey::for_settings(
            source.id.file_id,
            source.id.file_revision,
            settings,
            size,
            source.effective_quality(quality),
            CacheFormat {
                kind: CacheKind::BGuide,
                color_space: CacheColorSpace::Log2Luminance,
                format_version: GUIDE_FORMAT_VERSION,
            },
        );
        debug_assert_eq!(cache.develop_hash, GuideSpec::develop_hash(settings));
        Ok(GuideKey {
            cache,
            a1_long_edge: long_edge,
            params: spec.params.into(),
        })
    }

    /// 段階 B: ガイド（長辺 `long_edge` px・品質 `quality` の A1 から作る。キャッシュする）。
    pub fn guide(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        quality: RenderQuality,
        long_edge: u32,
    ) -> Result<Arc<Guide>> {
        self.guide_with(source, settings, quality, long_edge, None)
    }

    /// [`guide`](Self::guide) の本体。`a1` が渡されれば（同じ品質・長辺の A1）、A1 のキャッシュを
    /// 引き直さずに使う（回数を二重に数えないため）。
    fn guide_with(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        quality: RenderQuality,
        long_edge: u32,
        a1: Option<&Arc<A1Image>>,
    ) -> Result<Arc<Guide>> {
        let key = self.guide_cache_key(source, settings, quality, long_edge)?;
        if let Some(g) = self.lock().guide.lookup(&key) {
            return Ok(g);
        }
        let a1 = match a1 {
            Some(a) => Arc::clone(a),
            None => self.a1(source, settings, quality, long_edge)?,
        };
        let guide = Arc::new(compute_guide(&a1, &GuideSpec::from_settings(settings)?)?);
        self.lock().guide.insert(key, Arc::clone(&guide));
        Ok(guide)
    }

    // -----------------------------------------------------------------------
    // 段階 C（プレビュー）
    // -----------------------------------------------------------------------

    /// プレビュー（B3）を描く（[`render_preview_with`](Self::render_preview_with) の簡易版）。
    pub fn render_preview(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        quality: RenderQuality,
        long_edge: u32,
    ) -> Result<PreviewRender> {
        self.render_preview_with(source, settings, &PreviewRequest::new(quality, long_edge))
    }

    /// プレビューを描く（SYS-03・PRV-02）: A1（キャッシュ）→ ガイド（必要なときだけ。キャッシュ）→
    /// ステージ 9〜16（B3）→ 要求があれば 17a / 17b と 17c。
    pub fn render_preview_with(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        request: &PreviewRequest,
    ) -> Result<PreviewRender> {
        ProcessVersion::from_u32(settings.process_version)?;
        if request.histogram && request.output.is_none() {
            return Err(PipelineError::InvalidArgument(
                "ヒストグラムには出力先（17a / 17b）の指定が必要です".to_owned(),
            ));
        }
        let quality = source.effective_quality(request.quality);
        let a1 = self.a1(source, settings, quality, request.long_edge)?;
        let base: StageContext<'static> = a1.finish_context(settings)?;
        let (plan, guide) = plan_with_guide(settings, &base, || {
            self.guide_with(source, settings, quality, request.long_edge, Some(&a1))
        })?;
        let ctx = match &guide {
            Some(g) => base.with_guide(g),
            None => base,
        };
        let (w, h) = plan.output_size();
        let rois = plan.stage_rois(&ctx, Roi::full(w, h));
        let input = gather_mirror_from(&a1.image, rois[0])?;
        let (b3, mut replaced) = run_scene_stages(&plan, &ctx, &rois, input)?;
        let mut output = None;
        let mut histogram = None;
        if let Some(target) = &request.output {
            let (out, n) = target.apply(settings, &ctx, &b3)?;
            replaced += n;
            if request.histogram {
                histogram = Some(Histogram::compute(&out.image, target.luma_coefficients())?);
            }
            output = Some(out.image);
        }
        self.lock().previews_rendered += 1;
        Ok(PreviewRender {
            b3,
            output,
            histogram,
            quality,
            geometry: plan.geometry().clone(),
            active_stages: plan.active_stage_ids(),
            used_guide: guide.is_some(),
            warnings: a1.warnings.clone(),
            unimplemented: unimplemented_settings(settings),
            non_finite_replaced: replaced + a1.non_finite_replaced,
        })
    }
}

/// 有効なステージのパラメータを求める。ガイドが必要なステージがあれば（ガイドなしで
/// [`PipelineError::GuideMismatch`] になれば）、`make_guide` でガイドを作って求め直す。
pub(crate) fn plan_with_guide(
    settings: &DevelopSettings,
    base: &StageContext<'_>,
    make_guide: impl FnOnce() -> Result<Arc<Guide>>,
) -> Result<(FinishPlan, Option<Arc<Guide>>)> {
    match FinishPlan::new(settings, base) {
        Ok(plan) => Ok((plan, None)),
        Err(PipelineError::GuideMismatch { .. }) if base.guide.is_none() => {
            let guide = make_guide()?;
            let plan = FinishPlan::new(settings, &base.with_guide(&guide))?;
            Ok((plan, Some(guide)))
        }
        Err(e) => Err(e),
    }
}

/// ステージ 9〜16 を順に実行する。`rois` は [`FinishPlan::stage_rois`] の結果、`input` はステージ 9 の
/// 入力の格子のタイルで `rois[0]` を含むこと。出力は B3（契約を確かめる）。
pub(crate) fn run_scene_stages(
    plan: &FinishPlan,
    ctx: &StageContext<'_>,
    rois: &[Roi],
    input: ImageTile,
) -> Result<(ImageTile, u64)> {
    let mut tile = input;
    let mut replaced = 0;
    for ((stage, params), roi) in plan.stages().iter().zip(&rois[1..]) {
        let (next, n) = run_stage_cpu(*stage, ctx, &tile, *roi, params)?;
        tile = next;
        replaced += n;
    }
    if tile.contract() != ColorContract::B3DisplayReferred {
        return Err(PipelineError::ContractMismatch {
            stage: "engine.scene",
            expected: ColorContract::B3DisplayReferred,
            found: tile.contract(),
        });
    }
    Ok((tile, replaced))
}

/// 画像 `image` から範囲 `roi`（画像の外にはみ出してよい）を取り出す。はみ出した部分は鏡映（2.7 節。
/// [`ImageTile::gather_mirror`] と同じ値）。
pub(crate) fn gather_mirror_from(image: &RgbImage, roi: Roi) -> Result<ImageTile> {
    let (w, h) = image.dimensions();
    if roi.is_inside(w, h) {
        return ImageTile::from_image(roi, image.crop(roi)?);
    }
    let mut data = Vec::with_capacity(checked_len(roi.width, roi.height)?);
    let xs: Vec<usize> = (0..roi.width)
        .map(|i| mirror_index(i64::from(roi.x) + i64::from(i), w) as usize)
        .collect();
    for j in 0..roi.height {
        let row = image.row(mirror_index(i64::from(roi.y) + i64::from(j), h));
        data.extend(xs.iter().map(|&x| row[x]));
    }
    ImageTile::from_image(
        roi,
        RgbImage::from_vec(roi.width, roi.height, image.contract(), data)?,
    )
}

/// `src` を、範囲 `dst_roi` を覆う画素の列 `dst`（行優先、幅 `dst_roi.width`）に書く（`src.roi` は
/// `dst_roi` の内側）。
pub(crate) fn blit<T: Copy>(dst: &mut [T], dst_roi: Roi, src_roi: Roi, src: &[T], channels: usize) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_validation() {
        assert!(EngineConfig::default().validate().is_ok());
        for bad in [
            EngineConfig {
                tile_size: 0,
                ..Default::default()
            },
            EngineConfig {
                preview_long_edge: 0,
                ..Default::default()
            },
            EngineConfig {
                a1_capacity: 0,
                ..Default::default()
            },
        ] {
            assert!(Engine::new(bad).is_err());
        }
    }

    #[test]
    fn gather_mirror_matches_image_tile() {
        let img = RgbImage::from_fn(5, 4, ColorContract::B2Working, |x, y| {
            [x as f32, y as f32, (x * 10 + y) as f32]
        })
        .unwrap();
        let tile = ImageTile::from_image(Roi::full(5, 4), img.clone()).unwrap();
        for roi in [
            Roi::new(0, 0, 5, 4),
            Roi::new(1, 1, 2, 2),
            Roi::new(-3, -2, 11, 9),
            Roi::new(3, 2, 4, 4),
        ] {
            let a = gather_mirror_from(&img, roi).unwrap();
            let b = tile.gather_mirror(roi, 5, 4).unwrap();
            assert_eq!(a, b, "{roi:?}");
        }
    }

    #[test]
    fn blit_writes_rows() {
        let mut dst = vec![0u8; 4 * 3];
        let dst_roi = Roi::new(10, 20, 4, 3);
        blit(&mut dst, dst_roi, Roi::new(11, 21, 2, 2), &[1, 2, 3, 4], 1);
        assert_eq!(dst, [0, 0, 0, 0, 0, 1, 2, 0, 0, 3, 4, 0]);
    }
}
