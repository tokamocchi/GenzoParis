//! 処理ステージのインターフェース（docs/04_architecture.md の 7.1 節。02 の MAINT-08）。
//!
//! RGB の画像を扱うステージ（2.1 節のステージ 9〜17。仕上げ）は [`Stage`] を実装し、
//! [`crate::registry::StageRegistry`] に登録する。CFA を扱うセンサーのステージ（2〜8）は
//! 入出力の型が違うため、同じ考え方の別の小さなトレイト [`crate::sensor::SensorStage`] で表す。
//!
//! # 7.1 節との対応
//!
//! | 7.1 節 | このモジュール | 補足 |
//! |---|---|---|
//! | `id()` | [`Stage::id`] | 一意な ID。GPU 版の検索と一致テストのキーにもなる |
//! | `phase()` | [`Stage::phase`] | 段階 A0 / A1 / B / C（genzo-model の [`Phase`]） |
//! | `params(settings)` | [`Stage::params`] | `Ok(None)` ならステージを飛ばす。`ctx` も受け取る（画像の寸法に依存するパラメータのため） |
//! | `input_roi(output_roi, params)` | [`Stage::input_roi`] | |
//! | `run_cpu(ctx, input, output, params)` | [`Stage::run_cpu`] | 失敗を `Result` で返す。呼び出しは [`run_stage_cpu`] を通す |
//! | `gpu()` | [`Stage::gpu`] | **ステージの ID で GPU 版を探す**（下記） |
//! | — | [`Stage::input_contract`]・[`Stage::output_contract`] | 色の契約（2.6 節）を明示し、つなぎ方の誤りを検出する |
//!
//! # GPU 版の対応付け
//!
//! この crate は wgpu と genzo-gpu に依存しない（genzo-gpu がこの crate に依存する）。そのため、
//! GPU 版は genzo-gpu が [`GpuStageLookup`]（ステージの ID → [`GpuStage`]）として実装し、実行時に
//! 渡す。[`Stage::gpu`] の既定の実装は `lookup.find(self.id())`。GPU 版の実行のしかた（バッファ・
//! キュー）は genzo-gpu とエンジン（3 段目）が決め、[`GpuStage::as_any`] で genzo-gpu の具体的な型に
//! 戻して使う。GPU 版がないステージは CPU 版で処理する（7.1 節）。
//!
//! # 実装の約束（GPU 版と一致させるため。2.3 節）
//!
//! - 画素の処理は f32。行列の導出などの前処理は f64 でよい。
//! - 演算の順序を doc に書き、融合積和演算（`mul_add`）は使わない。合計・平均は分割と順序を固定する
//!   （並列化してもスレッド数で結果が変わらないこと）。
//! - 補間（拡大・LUT・カーブ）は [`crate::interp`] の明示的な式を使う（ハードウェア補間に頼らない）。
//! - 空間的なパラメータは G の長辺に対する割合で定義し、[`StageContext::length_px`] で画素に直す。
//! - 画像の端の外は鏡映で拡張する（[`crate::border`]）。
//! - 出力の NaN・無限大は [`run_stage_cpu`] が 0 に置き換える（2.6 節）。

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use genzo_model::{DevelopSettings, Phase, RenderQuality};

use crate::contract::ColorContract;
use crate::coords::{CoordinateFrames, SensorGrid};
use crate::error::{PipelineError, Result};
use crate::image::{GrayImage, ImageTile, Roi};
use crate::version::ProcessVersion;

/// ステージのパラメータ（7.1 節の `StageParams`）。
///
/// ステージごとに型が違うので、型を消して持つ。ステージは自分の型に戻して使う
/// （[`get`](Self::get)・[`expect`](Self::expect)）。GPU 版（genzo-gpu）も同じ型に戻して
/// uniform バッファを作る。複製は参照を数えるだけで安い。
#[derive(Clone)]
pub struct StageParams {
    value: Arc<dyn Any + Send + Sync>,
    type_name: &'static str,
}

impl StageParams {
    /// 値を包む。
    pub fn new<T: Any + Send + Sync>(value: T) -> Self {
        Self {
            value: Arc::new(value),
            type_name: std::any::type_name::<T>(),
        }
    }

    /// 型 `T` なら参照を返す。
    pub fn get<T: Any>(&self) -> Option<&T> {
        self.value.downcast_ref::<T>()
    }

    /// 型 `T` の参照。違う型なら [`PipelineError::ParamsTypeMismatch`]。
    pub fn expect<T: Any>(&self, stage: &'static str) -> Result<&T> {
        self.get::<T>().ok_or(PipelineError::ParamsTypeMismatch {
            stage,
            found: self.type_name,
        })
    }

    /// 包んだ値の型の名前（ログ用）。
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }
}

impl fmt::Debug for StageParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StageParams")
            .field("type", &self.type_name)
            .finish()
    }
}

/// ガイド（2.7 節。段階 B で作り、ステージ 11・12 がタイル処理で参照する）。
///
/// 値は、ステージ 8 の出力（B2、露光量を適用する前）の輝度 Y を G 座標で長辺 512px 程度に縮小し、
/// `log2(max(Y, 2^−16))`（[`crate::sanitize::safe_log2`]）をぼかしたもの。露光量は使うときに EV を
/// 足す。作り方（縮小・ぼかし）は [`crate::guide`]（[`crate::guide::compute_guide`]）を参照。
/// キャッシュは 3 段目（キーは [`crate::guide::GuideSpec`] の doc）。
#[derive(Debug, Clone, PartialEq)]
pub struct Guide {
    /// G 座標の画像（切り抜く前の全体）。画素 (i, j) の中心は G の
    /// `((i + 0.5) / 幅 × gw, (j + 0.5) / 高さ × gh)`（gw・gh は G の外接矩形の幅・高さ）。
    pub image: GrayImage,
    /// ガイドを作ったときの座標系（S の寸法・向き・回転）。使う側の文脈の
    /// [`StageContext::frames`] と一致すること（回転を変えたらガイドを作り直す。2.7 節）。
    pub frames: CoordinateFrames,
    /// ガイドを作った処理バージョン（アルゴリズムのパラメータは版で決まる）。
    pub process_version: ProcessVersion,
}

impl Guide {
    /// 画像と、作ったときの座標系・処理バージョンから作る。
    pub fn new(
        image: GrayImage,
        frames: CoordinateFrames,
        process_version: ProcessVersion,
    ) -> Self {
        Self {
            image,
            frames,
            process_version,
        }
    }
}

/// ステージの実行の文脈（7.1 節の `StageContext`）。
#[derive(Debug, Clone, Copy)]
pub struct StageContext<'a> {
    /// 処理バージョン（2.5 節）。ステージはこれでアルゴリズムを切り替える。
    pub process_version: ProcessVersion,
    /// 座標系（S の寸法と向き。2.7 節）。
    pub frames: CoordinateFrames,
    /// 出力の解像度の倍率（このステージの画素の格子の 1 画素が、フル解像度の何画素に当たるかの
    /// 逆数）。フル解像度（等倍・書き出し）なら 1、長辺 2560px のプレビューなら
    /// `2560 / フル解像度の長辺`、2 × 2 の簡易処理の出力なら 0.5。
    pub resolution_scale: f64,
    /// 品質（ドラッグ中の簡易処理か、最終品質か。2.2 節）。
    pub quality: RenderQuality,
    /// ガイド（段階 B の結果）。ガイドを使わないステージ・段階では `None`。
    pub guide: Option<&'a Guide>,
    /// ステージ 9 の入力の格子（S 座標の標本化。段階 A1 のプレビュー、フル解像度のタイルなど）。
    /// ステージ 9 は、この格子から C の格子（[`crate::finish::geometry::GeometryPlan`]）へ
    /// リサンプリングする。既定はフル解像度（[`SensorGrid::full_resolution`]）。
    pub sensor_grid: SensorGrid,
}

impl<'a> StageContext<'a> {
    /// フル解像度・最終品質・ガイドなしの文脈。
    pub fn new(process_version: ProcessVersion, frames: CoordinateFrames) -> Self {
        Self {
            process_version,
            frames,
            resolution_scale: 1.0,
            quality: RenderQuality::Final,
            guide: None,
            sensor_grid: SensorGrid::full_resolution(&frames),
        }
    }

    /// ステージ 9 の入力の格子を変えたもの。
    pub fn with_sensor_grid(mut self, grid: SensorGrid) -> Self {
        self.sensor_grid = grid;
        self
    }

    /// 出力の解像度の倍率を変えたもの。
    pub fn with_resolution_scale(mut self, scale: f64) -> Self {
        self.resolution_scale = scale;
        self
    }

    /// 品質を変えたもの。
    pub fn with_quality(mut self, quality: RenderQuality) -> Self {
        self.quality = quality;
        self
    }

    /// ガイドを付けたもの。
    pub fn with_guide(mut self, guide: &'a Guide) -> Self {
        self.guide = Some(guide);
        self
    }

    /// G の長辺に対する割合 `fraction` を、このステージの画素の格子の長さ（画素）に直す
    /// （04 の 1.1 節の 5「パラメータは解像度に依存させない」、2.7 節）。
    pub fn length_px(&self, fraction: f64) -> f64 {
        fraction * self.frames.g_long_side_px() * self.resolution_scale
    }
}

/// GPU 版のステージ（genzo-gpu が実装する。7.1 節の `GpuStage`）。
///
/// この crate は wgpu を知らないので、GPU の資源と実行のしかたは実装側に隠す。エンジンや一致テストは
/// [`GpuStageLookup`] でステージの ID から探し、[`as_any`](Self::as_any) で genzo-gpu の型に戻す。
pub trait GpuStage: Any + Send + Sync {
    /// 対応する CPU 版のステージの ID（[`Stage::id`]・[`crate::sensor::SensorStage::id`]）。
    fn stage_id(&self) -> &'static str;

    /// この処理バージョンのアルゴリズムを実装しているか。`false` なら CPU 版で処理する。
    fn supports(&self, version: ProcessVersion) -> bool;

    /// genzo-gpu の具体的な型に戻すため。
    fn as_any(&self) -> &dyn Any;
}

/// ステージの ID から GPU 版を探す（genzo-gpu が登録表として実装する）。
pub trait GpuStageLookup: Send + Sync {
    /// ID `stage_id` の GPU 版。なければ `None`。
    fn find(&self, stage_id: &str) -> Option<&dyn GpuStage>;
}

/// GPU 版が 1 つもない（GPU のない環境・CI。すべて CPU 版で処理する）。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoGpu;

impl GpuStageLookup for NoGpu {
    fn find(&self, _stage_id: &str) -> Option<&dyn GpuStage> {
        None
    }
}

/// 現像パイプラインの 1 ステージ（RGB の画像を扱うもの。7.1 節の `Stage`）。
///
/// 新しい補正処理はこのトレイトを実装して [`crate::registry::StageRegistry`] に登録する
/// （MAINT-08）。登録すると、CPU 版と GPU 版の一致テストの対象になる（genzo-gpu が登録表を
/// たどる）。
pub trait Stage: Send + Sync {
    /// 一意な ID（例: `"exposure"`）。
    fn id(&self) -> &'static str;

    /// 段階 A0 / A1 / B / C のどこに属するか（2.2 節）。
    fn phase(&self) -> Phase;

    /// 入力の色の契約（2.6 節）。
    fn input_contract(&self) -> ColorContract;

    /// 出力の色の契約（2.6 節）。多くのステージは入力と同じ。
    fn output_contract(&self) -> ColorContract;

    /// 現像設定からこのステージのパラメータを取り出す。何もしない設定（露光量 0 など）なら
    /// `Ok(None)` を返し、ステージを飛ばす。
    fn params(
        &self,
        settings: &DevelopSettings,
        ctx: &StageContext<'_>,
    ) -> Result<Option<StageParams>>;

    /// 出力の範囲 `output_roi` を計算するのに必要な入力の範囲（周辺の画素を使う処理や、
    /// ジオメトリの変換のため）。画素ごとの処理なら `output_roi` をそのまま返す。
    fn input_roi(&self, output_roi: Roi, params: &StageParams, ctx: &StageContext<'_>) -> Roi;

    /// CPU 版の基準実装。`ctx.process_version` によってアルゴリズムを切り替える（2.5 節）。
    ///
    /// `input.roi` は `input_roi(output.roi)` を含み、`output` は `output.roi` の寸法で、
    /// 契約が [`output_contract`](Self::output_contract) の 0 で埋めた画像（[`run_stage_cpu`] が
    /// 用意する）。直接呼ばずに [`run_stage_cpu`] を通すこと。
    fn run_cpu(
        &self,
        ctx: &StageContext<'_>,
        input: &ImageTile,
        output: &mut ImageTile,
        params: &StageParams,
    ) -> Result<()>;

    /// GPU 版（ステージの ID で探す）。未実装なら `None` を返し、CPU 版で処理する。
    fn gpu<'g>(&self, lookup: &'g dyn GpuStageLookup) -> Option<&'g dyn GpuStage> {
        lookup.find(self.id())
    }
}

/// ステージを CPU 版で 1 回実行する（エンジンと一致テストの共通の入口）。
///
/// 1. `input` の色の契約が [`Stage::input_contract`] と一致するか確かめる。
/// 2. `input.roi` が `input_roi(output_roi)` を含むか確かめる。
/// 3. 出力のタイル（0 で埋めたもの）を用意して [`Stage::run_cpu`] を呼ぶ。
/// 4. 出力の NaN・無限大を 0 に置き換える（2.6 節）。置き換えた数を返す。
pub fn run_stage_cpu(
    stage: &dyn Stage,
    ctx: &StageContext<'_>,
    input: &ImageTile,
    output_roi: Roi,
    params: &StageParams,
) -> Result<(ImageTile, u64)> {
    if input.contract() != stage.input_contract() {
        return Err(PipelineError::ContractMismatch {
            stage: stage.id(),
            expected: stage.input_contract(),
            found: input.contract(),
        });
    }
    let need = stage.input_roi(output_roi, params, ctx);
    if !input.roi.contains_roi(&need) {
        return Err(PipelineError::InvalidRoi {
            roi: input.roi,
            reason: "入力のタイルがステージの必要な範囲を覆っていない",
        });
    }
    let mut output = ImageTile::new(output_roi, stage.output_contract())?;
    stage.run_cpu(ctx, input, &mut output, params)?;
    if output.image.contract() != stage.output_contract() {
        return Err(PipelineError::ContractMismatch {
            stage: stage.id(),
            expected: stage.output_contract(),
            found: output.image.contract(),
        });
    }
    let replaced = output.image.sanitize();
    Ok((output, replaced))
}

#[cfg(test)]
pub(crate) mod tests {
    use genzo_model::Orientation;

    use super::*;

    /// テスト用のステージ: 露光量（2^EV 倍）。画素ごとの処理。
    pub(crate) struct TestExposure;

    /// テスト用のステージ: 左右の画素の平均（周辺の画素を使う処理の例）。
    pub(crate) struct TestBoxX;

    impl Stage for TestExposure {
        fn id(&self) -> &'static str {
            "test.exposure"
        }
        fn phase(&self) -> Phase {
            Phase::C
        }
        fn input_contract(&self) -> ColorContract {
            ColorContract::B2Working
        }
        fn output_contract(&self) -> ColorContract {
            ColorContract::B2Working
        }
        fn params(
            &self,
            settings: &DevelopSettings,
            _ctx: &StageContext<'_>,
        ) -> Result<Option<StageParams>> {
            let ev = settings.exposure_ev;
            Ok((ev != 0.0).then(|| StageParams::new(ev.exp2())))
        }
        fn input_roi(&self, output_roi: Roi, _: &StageParams, _: &StageContext<'_>) -> Roi {
            output_roi
        }
        fn run_cpu(
            &self,
            ctx: &StageContext<'_>,
            input: &ImageTile,
            output: &mut ImageTile,
            params: &StageParams,
        ) -> Result<()> {
            match ctx.process_version {
                ProcessVersion::V1 => {}
            }
            let k = *params.expect::<f32>(self.id())?;
            let src = input.crop(output.roi)?;
            for (o, i) in output.image.data_mut().iter_mut().zip(src.image.data()) {
                *o = i.map(|v| v * k);
            }
            Ok(())
        }
    }

    impl Stage for TestBoxX {
        fn id(&self) -> &'static str {
            "test.box_x"
        }
        fn phase(&self) -> Phase {
            Phase::C
        }
        fn input_contract(&self) -> ColorContract {
            ColorContract::B2Working
        }
        fn output_contract(&self) -> ColorContract {
            ColorContract::B2Working
        }
        fn params(&self, _: &DevelopSettings, _: &StageContext<'_>) -> Result<Option<StageParams>> {
            Ok(Some(StageParams::new(())))
        }
        fn input_roi(&self, output_roi: Roi, _: &StageParams, _: &StageContext<'_>) -> Roi {
            output_roi.expand_xy(1, 0)
        }
        fn run_cpu(
            &self,
            _ctx: &StageContext<'_>,
            input: &ImageTile,
            output: &mut ImageTile,
            _params: &StageParams,
        ) -> Result<()> {
            let roi = output.roi;
            for y in 0..roi.height {
                for x in 0..roi.width {
                    let gx = i64::from(roi.x) + i64::from(x);
                    let gy = i64::from(roi.y) + i64::from(y);
                    let l = input.at(gx - 1, gy).expect("余白");
                    let r = input.at(gx + 1, gy).expect("余白");
                    *output.image.pixel_mut(x, y).expect("範囲内") =
                        std::array::from_fn(|k| (l[k] + r[k]) * 0.5);
                }
            }
            Ok(())
        }
    }

    pub(crate) fn ctx() -> StageContext<'static> {
        StageContext::new(
            ProcessVersion::V1,
            CoordinateFrames::new(100, 50, Orientation::Normal).unwrap(),
        )
    }

    fn tile(roi: Roi, contract: ColorContract, f: impl Fn(i32, i32) -> f32) -> ImageTile {
        let image = crate::image::RgbImage::from_fn(roi.width, roi.height, contract, |x, y| {
            [f(roi.x + x as i32, roi.y + y as i32); 3]
        })
        .unwrap();
        ImageTile::from_image(roi, image).unwrap()
    }

    #[test]
    fn params_round_trip_and_type_errors() {
        let p = StageParams::new(2.5f32);
        assert_eq!(p.get::<f32>(), Some(&2.5));
        assert!(p.get::<f64>().is_none());
        assert!(matches!(
            p.expect::<u32>("x"),
            Err(PipelineError::ParamsTypeMismatch { stage: "x", .. })
        ));
        assert!(format!("{p:?}").contains("f32"));
        assert_eq!(p.clone().type_name(), "f32");
    }

    #[test]
    fn neutral_settings_skip_the_stage() {
        let s = DevelopSettings::default();
        assert!(TestExposure.params(&s, &ctx()).unwrap().is_none());
        let s = DevelopSettings {
            exposure_ev: 1.0,
            ..Default::default()
        };
        let p = TestExposure.params(&s, &ctx()).unwrap().unwrap();
        assert_eq!(p.get::<f32>(), Some(&2.0));
    }

    #[test]
    fn run_stage_checks_contract_roi_and_sanitizes() {
        let roi = Roi::new(0, 0, 4, 2);
        let p = StageParams::new(2.0f32);
        let input = tile(roi, ColorContract::B2Working, |x, _| x as f32);
        let (out, replaced) = run_stage_cpu(&TestExposure, &ctx(), &input, roi, &p).unwrap();
        assert_eq!(replaced, 0);
        assert_eq!(out.image.pixel(3, 0), Some([6.0; 3]));

        // 契約が違う。
        let wrong = tile(roi, ColorContract::B1CameraRgb, |_, _| 0.0);
        assert!(matches!(
            run_stage_cpu(&TestExposure, &ctx(), &wrong, roi, &p),
            Err(PipelineError::ContractMismatch { .. })
        ));
        // 入力が必要な範囲（左右 1 画素の余白）を覆っていない。
        let unit = StageParams::new(());
        assert!(matches!(
            run_stage_cpu(&TestBoxX, &ctx(), &input, roi, &unit),
            Err(PipelineError::InvalidRoi { .. })
        ));
        let wide = tile(roi.expand_xy(1, 0), ColorContract::B2Working, |x, _| {
            x as f32
        });
        let (out, _) = run_stage_cpu(&TestBoxX, &ctx(), &wide, roi, &unit).unwrap();
        assert_eq!(out.image.pixel(0, 0), Some([0.0; 3]));
        assert_eq!(out.image.pixel(2, 1), Some([2.0; 3]));

        // 出力の NaN・無限大は 0 になる。
        let big = tile(roi, ColorContract::B2Working, |_, _| f32::MAX);
        let (out, replaced) = run_stage_cpu(&TestExposure, &ctx(), &big, roi, &p).unwrap();
        assert_eq!(replaced, 8 * 3);
        assert!(out.image.as_flat().iter().all(|&v| v == 0.0));
    }

    #[test]
    fn length_is_relative_to_g_long_side() {
        let c = ctx();
        assert_eq!(c.length_px(0.01), 1.0);
        assert_eq!(c.with_resolution_scale(0.5).length_px(0.01), 0.5);
        let guide = Guide::new(
            GrayImage::filled(4, 2, -3.0).unwrap(),
            c.frames,
            ProcessVersion::V1,
        );
        let g = c.with_guide(&guide).with_quality(RenderQuality::Draft);
        assert_eq!(g.guide.unwrap().image.pixel(0, 0), Some(-3.0));
        assert_eq!(g.quality, RenderQuality::Draft);
    }

    struct FakeGpu;
    impl GpuStage for FakeGpu {
        fn stage_id(&self) -> &'static str {
            "test.exposure"
        }
        fn supports(&self, version: ProcessVersion) -> bool {
            version == ProcessVersion::V1
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    struct FakeLookup(FakeGpu);
    impl GpuStageLookup for FakeLookup {
        fn find(&self, stage_id: &str) -> Option<&dyn GpuStage> {
            (stage_id == self.0.stage_id()).then_some(&self.0 as &dyn GpuStage)
        }
    }

    #[test]
    fn gpu_versions_are_found_by_id() {
        let lookup = FakeLookup(FakeGpu);
        let g = TestExposure.gpu(&lookup).expect("ID で見つかる");
        assert_eq!(g.stage_id(), "test.exposure");
        assert!(g.supports(ProcessVersion::V1));
        assert!(g.as_any().downcast_ref::<FakeGpu>().is_some());
        assert!(TestBoxX.gpu(&lookup).is_none());
        assert!(TestExposure.gpu(&NoGpu).is_none());
    }
}
