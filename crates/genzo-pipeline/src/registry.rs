//! ステージの登録表（docs/04_architecture.md の 7.1 節「新しい補正処理は、トレイトを実装して登録する」
//! 「ステージの CPU 版と GPU 版の一致テストは、ステージを登録すると自動で対象になる」。02 の MAINT-08）。
//!
//! - RGB のステージ（[`Stage`]。仕上げの 9〜17。[`crate::finish`]）と、センサーのステージ（[`SensorStage`]。2〜8）を
//!   ID で登録する。ID の重複はエラー。
//! - genzo-gpu の一致テストは [`StageRegistry::builtin`] をたどり、ID ごとに GPU 版を探して CPU 版と比べる。
//!   GPU 版がないステージは [`GpuCoverage::cpu_only`] に入る（テストで一覧を表示する）。
//! - 動的に読み込むプラグインではなく、コンパイル時に組み込む（7.1 節）。

use std::sync::Arc;

use crate::error::{PipelineError, Result};
use crate::finish::{DisplayStage, ExportStage, scene_stages};
use crate::sensor::{SensorStage, builtin_sensor_stages};
use crate::stage::{GpuStageLookup, Stage};
use crate::version::ProcessVersion;

/// 登録されたステージの GPU 版の有無。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GpuCoverage {
    /// GPU 版があり、その処理バージョンに対応しているステージの ID（一致テストの対象）。
    pub with_gpu: Vec<&'static str>,
    /// GPU 版がない（または処理バージョンに対応していない）ステージの ID（CPU 版で処理する）。
    pub cpu_only: Vec<&'static str>,
}

/// ステージの登録表。
#[derive(Clone, Default)]
pub struct StageRegistry {
    sensor: Vec<Arc<dyn SensorStage>>,
    stages: Vec<Arc<dyn Stage>>,
}

impl std::fmt::Debug for StageRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageRegistry")
            .field("ids", &self.ids())
            .finish()
    }
}

/// `&'static dyn SensorStage` を `Arc` に包むための入れ物。
struct StaticSensor(&'static dyn SensorStage);

impl SensorStage for StaticSensor {
    fn id(&self) -> &'static str {
        self.0.id()
    }
    fn stage_number(&self) -> u8 {
        self.0.stage_number()
    }
    fn input_kind(&self) -> crate::sensor::SensorKind {
        self.0.input_kind()
    }
    fn output_kind(&self) -> crate::sensor::SensorKind {
        self.0.output_kind()
    }
    fn input_roi(
        &self,
        output_roi: crate::image::Roi,
        plan: &crate::sensor::SensorPlan,
    ) -> crate::image::Roi {
        self.0.input_roi(output_roi, plan)
    }
    fn run_cpu<'r>(
        &self,
        ctx: &crate::stage::StageContext<'_>,
        plan: &crate::sensor::SensorPlan,
        input: crate::sensor::SensorData<'r>,
        output_roi: crate::image::Roi,
    ) -> Result<crate::sensor::SensorData<'r>> {
        self.0.run_cpu(ctx, plan, input, output_roi)
    }
}

/// `&'static dyn Stage` を `Arc` に包むための入れ物。
struct StaticStage(&'static dyn Stage);

impl Stage for StaticStage {
    fn id(&self) -> &'static str {
        self.0.id()
    }
    fn phase(&self) -> genzo_model::Phase {
        self.0.phase()
    }
    fn input_contract(&self) -> crate::contract::ColorContract {
        self.0.input_contract()
    }
    fn output_contract(&self) -> crate::contract::ColorContract {
        self.0.output_contract()
    }
    fn params(
        &self,
        settings: &genzo_model::DevelopSettings,
        ctx: &crate::stage::StageContext<'_>,
    ) -> Result<Option<crate::stage::StageParams>> {
        self.0.params(settings, ctx)
    }
    fn input_roi(
        &self,
        output_roi: crate::image::Roi,
        params: &crate::stage::StageParams,
        ctx: &crate::stage::StageContext<'_>,
    ) -> crate::image::Roi {
        self.0.input_roi(output_roi, params, ctx)
    }
    fn run_cpu(
        &self,
        ctx: &crate::stage::StageContext<'_>,
        input: &crate::image::ImageTile,
        output: &mut crate::image::ImageTile,
        params: &crate::stage::StageParams,
    ) -> Result<()> {
        self.0.run_cpu(ctx, input, output, params)
    }
}

impl StageRegistry {
    /// 空の登録表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 組み込みのステージをすべて登録した表（処理の順）。
    ///
    /// - センサーのステージ（2・3・5（3 方式）・8）。6・7（v1）は ID だけ予約
    ///   （[`crate::finish::reserved`]）。
    /// - 仕上げのステージ 9〜16（[`crate::finish::scene_stages`]。12・14 は v1 の予約で、常に飛ばす）。
    /// - 出力: 17a 画面（[`DisplayStage::default`]。モニターのプロファイルがない場合の sRGB の変換で、
    ///   一致テスト用）、17b 書き出し（sRGB・Display P3・Adobe RGB）とキャッシュ（Display P3）。
    ///   17c のヒストグラムは画像を出力しないので登録しない。
    pub fn builtin() -> Self {
        let mut r = Self::new();
        for s in builtin_sensor_stages() {
            r.register_sensor(Arc::new(StaticSensor(s)))
                .expect("組み込みのステージの ID は重複しない");
        }
        for s in scene_stages() {
            r.register(Arc::new(StaticStage(s)))
                .expect("組み込みのステージの ID は重複しない");
        }
        r.register(Arc::new(DisplayStage::default()))
            .expect("組み込みのステージの ID は重複しない");
        for s in ExportStage::ALL {
            r.register(Arc::new(s))
                .expect("組み込みのステージの ID は重複しない");
        }
        r
    }

    fn check_unique(&self, id: &'static str) -> Result<()> {
        if self.contains(id) {
            return Err(PipelineError::DuplicateStageId(id));
        }
        Ok(())
    }

    /// RGB のステージを登録する。ID が重複していればエラー。
    pub fn register(&mut self, stage: Arc<dyn Stage>) -> Result<()> {
        self.check_unique(stage.id())?;
        self.stages.push(stage);
        Ok(())
    }

    /// センサーのステージを登録する。ID が重複していればエラー。
    pub fn register_sensor(&mut self, stage: Arc<dyn SensorStage>) -> Result<()> {
        self.check_unique(stage.id())?;
        self.sensor.push(stage);
        Ok(())
    }

    /// ID が登録されているか。
    pub fn contains(&self, id: &str) -> bool {
        self.ids().contains(&id)
    }

    /// RGB のステージ（登録順）。
    pub fn stages(&self) -> impl Iterator<Item = &dyn Stage> {
        self.stages.iter().map(|s| s.as_ref())
    }

    /// センサーのステージ（登録順）。
    pub fn sensor_stages(&self) -> impl Iterator<Item = &dyn SensorStage> {
        self.sensor.iter().map(|s| s.as_ref())
    }

    /// ID で RGB のステージを探す。
    pub fn find(&self, id: &str) -> Option<&dyn Stage> {
        self.stages().find(|s| s.id() == id)
    }

    /// ID でセンサーのステージを探す。
    pub fn find_sensor(&self, id: &str) -> Option<&dyn SensorStage> {
        self.sensor_stages().find(|s| s.id() == id)
    }

    /// すべての ID（センサーのステージ、RGB のステージの順。それぞれ登録順）。
    pub fn ids(&self) -> Vec<&'static str> {
        self.sensor
            .iter()
            .map(|s| s.id())
            .chain(self.stages.iter().map(|s| s.id()))
            .collect()
    }

    /// GPU 版の有無を ID ごとに調べる（一致テストの対象の一覧）。
    pub fn gpu_coverage(
        &self,
        lookup: &dyn GpuStageLookup,
        version: ProcessVersion,
    ) -> GpuCoverage {
        let mut c = GpuCoverage::default();
        for id in self.ids() {
            match lookup.find(id) {
                Some(g) if g.supports(version) => c.with_gpu.push(id),
                _ => c.cpu_only.push(id),
            }
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use super::*;
    use crate::sensor::demosaic::DemosaicMethod;
    use crate::sensor::{ColorMatrixStage, NormalizeStage, WhiteBalanceStage};
    use crate::stage::tests::{TestBoxX, TestExposure};
    use crate::stage::{GpuStage, NoGpu};

    #[test]
    fn builtin_lists_the_sensor_stages() {
        let r = StageRegistry::builtin();
        let ids = r.ids();
        assert_eq!(
            ids[..6],
            [
                NormalizeStage::ID,
                WhiteBalanceStage::ID,
                DemosaicMethod::Rcd.stage_id(),
                DemosaicMethod::Bilinear.stage_id(),
                DemosaicMethod::Half2x2.stage_id(),
                ColorMatrixStage::ID,
            ]
        );
        assert_eq!(
            ids[6..],
            [
                "finish.geometry",
                "finish.exposure",
                "finish.contrast",
                "finish.tone",
                "finish.texture",
                "finish.color",
                "finish.sharpen",
                "finish.scene_to_display",
                "finish.tone_curve",
                "output.display",
                "output.export.srgb",
                "output.export.display_p3",
                "output.export.adobe_rgb",
                "output.cache.display_p3",
            ]
        );
        // 仕上げのステージはすべて段階 C。
        assert!(r.stages().all(|s| s.phase() == genzo_model::Phase::C));
        assert!(r.find("finish.tone").is_some());
        let numbers: Vec<u8> = r.sensor_stages().map(|s| s.stage_number()).collect();
        assert_eq!(numbers, [2, 3, 5, 5, 5, 8]);
        assert!(r.find_sensor("sensor.demosaic.rcd").is_some());
        assert!(r.find("sensor.demosaic.rcd").is_none());
        assert!(format!("{r:?}").contains("sensor.normalize"));
    }

    #[test]
    fn registering_rgb_stages_and_duplicates() {
        let mut r = StageRegistry::builtin();
        r.register(Arc::new(TestExposure)).unwrap();
        r.register(Arc::new(TestBoxX)).unwrap();
        assert!(matches!(
            r.register(Arc::new(TestExposure)),
            Err(PipelineError::DuplicateStageId("test.exposure"))
        ));
        assert!(matches!(
            r.register_sensor(Arc::new(StaticSensor(&NormalizeStage))),
            Err(PipelineError::DuplicateStageId("sensor.normalize"))
        ));
        let builtin = StageRegistry::builtin();
        assert_eq!(r.find("test.box_x").map(|s| s.id()), Some("test.box_x"));
        assert_eq!(r.stages().count(), builtin.stages().count() + 2);
        assert!(r.contains("test.exposure"));
        assert_eq!(r.ids().len(), builtin.ids().len() + 2);
        assert!(matches!(
            r.register(Arc::new(ExportStage::SRGB)),
            Err(PipelineError::DuplicateStageId("output.export.srgb"))
        ));
    }

    struct Gpu(&'static str);
    impl GpuStage for Gpu {
        fn stage_id(&self) -> &'static str {
            self.0
        }
        fn supports(&self, _version: ProcessVersion) -> bool {
            true
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }
    struct Lookup(Vec<Gpu>);
    impl GpuStageLookup for Lookup {
        fn find(&self, stage_id: &str) -> Option<&dyn GpuStage> {
            self.0
                .iter()
                .find(|g| g.0 == stage_id)
                .map(|g| g as &dyn GpuStage)
        }
    }

    #[test]
    fn gpu_coverage_is_found_by_id() {
        let mut r = StageRegistry::builtin();
        r.register(Arc::new(TestExposure)).unwrap();
        let lookup = Lookup(vec![
            Gpu("sensor.demosaic.rcd"),
            Gpu("test.exposure"),
            Gpu("unknown"),
        ]);
        let c = r.gpu_coverage(&lookup, ProcessVersion::V1);
        assert_eq!(c.with_gpu, ["sensor.demosaic.rcd", "test.exposure"]);
        assert_eq!(c.cpu_only.len(), r.ids().len() - 2);
        assert!(c.cpu_only.contains(&"sensor.normalize"));
        assert!(c.cpu_only.contains(&"finish.tone_curve"));
        let none = r.gpu_coverage(&NoGpu, ProcessVersion::V1);
        assert!(none.with_gpu.is_empty());
        assert_eq!(none.cpu_only.len(), r.ids().len());
        // SensorStage::gpu も ID で探す。
        let s = r.find_sensor("sensor.demosaic.rcd").unwrap();
        assert!(s.gpu(&lookup).is_some());
        assert!(
            r.find_sensor("sensor.normalize")
                .unwrap()
                .gpu(&lookup)
                .is_none()
        );
    }
}
