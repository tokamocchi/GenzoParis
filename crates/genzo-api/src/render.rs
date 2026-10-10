//! 現像の実行（GPU を既定とし、失敗したら CPU 版に切り替える。04 の 2.4 節・6.3 節）と、画面の
//! プロファイル（17a。IQ-05）。
//!
//! - GPU は最初に使うときに初期化する（起動を遅くしないため。PERF-12）。[`crate::GpuMode::Off`]・
//!   アダプターがない・環境変数 `GENZO_GPU=0` なら CPU 版だけで処理する。
//! - GPU の側の失敗（[`genzo_gpu::GpuError::should_fall_back_to_cpu`]）では、CPU 版で処理し直して
//!   警告のイベントを送る。次に使うときに GPU を初期化し直し、失敗が
//!   [`crate::config::MAX_GPU_FAILURES`] 回に達したら、そのセッションでは GPU を使わない（6.3 節）。

use std::sync::Arc;

use genzo_color::{DisplayProfile, DisplayProfileFallbackReason, DisplayProfileSource};
use genzo_gpu::{GpuContext, GpuContextOptions, GpuError, GpuPreviewOptions, GpuRenderer};
use genzo_model::{DevelopSettings, ExportSettings, VariantId};
use genzo_pipeline::finish::Histogram;
use genzo_pipeline::finish::output::DisplayTransform;
use genzo_pipeline::{
    ExportOptions, ExportedImage, ImageTile, OutputTarget, PhotoSource, PreviewRequest,
    RenderControl, RgbImage,
};

use crate::config::{GpuMode, MAX_GPU_FAILURES};
use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::WarningCode;
use crate::types::{DisplayInfo, RenderBackend};

/// GPU の状態。
#[derive(Default)]
pub(crate) struct GpuState {
    slot: GpuSlot,
    failures: u32,
}

#[derive(Default)]
enum GpuSlot {
    /// まだ初期化していない（または初期化し直す）。
    #[default]
    NotInitialized,
    /// 使える。
    Ready(Arc<GpuRenderer>),
    /// 使えない（GPU を使わない設定・アダプターがない・初期化の失敗・失敗が続いた・終了した）。
    Unavailable,
}

impl GpuState {
    /// 終了した状態。
    pub(crate) fn closed() -> Self {
        Self {
            slot: GpuSlot::Unavailable,
            failures: 0,
        }
    }
}

/// 画面のプロファイルと 17a の変換。
pub(crate) struct DisplayState {
    pub profile: DisplayProfile,
    pub transform: Arc<DisplayTransform>,
    pub info: DisplayInfo,
}

impl DisplayState {
    /// モニターのプロファイルが分からないとき（sRGB とみなす。IQ-05）。
    pub(crate) fn assumed_srgb() -> Result<Self, ApiError> {
        Self::from_profile(
            DisplayProfile::assumed_srgb(DisplayProfileFallbackReason::NotAvailable)
                .map_err(genzo_pipeline::PipelineError::from)?,
        )
    }

    fn from_profile(profile: DisplayProfile) -> Result<Self, ApiError> {
        let transform = Arc::new(DisplayTransform::new(
            &profile,
            genzo_pipeline::finish::output::DISPLAY_LUT_SIZE,
        )?);
        let reason = match profile.source() {
            DisplayProfileSource::Os => None,
            DisplayProfileSource::AssumedSrgb(r) => Some(match r {
                DisplayProfileFallbackReason::NotAvailable => {
                    "モニターのプロファイルを取得できない".to_owned()
                }
                DisplayProfileFallbackReason::Invalid(m) => {
                    format!("モニターのプロファイルを読み込めない: {m}")
                }
                DisplayProfileFallbackReason::Unsupported(m) => {
                    format!("モニターのプロファイルが使えない: {m}")
                }
            }),
        };
        let info = DisplayInfo {
            assumed_srgb: profile.is_assumed_srgb(),
            reason,
            description: profile.profile().description().map(str::to_owned),
        };
        Ok(Self {
            profile,
            transform,
            info,
        })
    }
}

/// プレビューの描画の結果。
pub(crate) struct Rendered {
    pub b3: ImageTile,
    pub output: Option<RgbImage>,
    pub histogram: Option<Histogram>,
    pub backend: RenderBackend,
    pub quality: genzo_model::RenderQuality,
    pub warnings: Vec<String>,
}

impl Inner {
    /// 使える GPU（なければ `None`）。最初に呼んだときに初期化する。
    pub(crate) fn gpu(&self) -> Option<Arc<GpuRenderer>> {
        if self.config.gpu == GpuMode::Off {
            return None;
        }
        let mut state = self.gpu.lock();
        match &state.slot {
            GpuSlot::Ready(r) => return Some(Arc::clone(r)),
            GpuSlot::Unavailable => return None,
            GpuSlot::NotInitialized => {}
        }
        let created = GpuContext::new(&GpuContextOptions::from_env())
            .and_then(|c| c.map(GpuRenderer::new).transpose());
        match created {
            Ok(Some(r)) => {
                let r = Arc::new(r);
                // 画面のプロファイルを GPU 版の 17a に登録する（登録できなければ 17a は CPU 版で処理する）。
                let profile = self.display.lock().profile.clone();
                if let Err(e) = r.register_display_profile(&profile) {
                    tracing::warn!(error = %e, "画面のプロファイルを GPU に登録できない");
                }
                tracing::info!(adapter = %r.context().summary(), "GPU を使う");
                state.slot = GpuSlot::Ready(Arc::clone(&r));
                Some(r)
            }
            Ok(None) => {
                state.slot = GpuSlot::Unavailable;
                None
            }
            Err(e) => {
                state.slot = GpuSlot::Unavailable;
                drop(state);
                self.events.warn(
                    WarningCode::GpuDisabled,
                    format!("GPU を初期化できないため、CPU 版で処理します: {e}"),
                    None,
                    None,
                );
                None
            }
        }
    }

    /// GPU の側の失敗を記録し、警告を送る（6.3 節）。
    pub(crate) fn gpu_failed(&self, error: &str, variant_id: Option<VariantId>) {
        let disabled = {
            let mut state = self.gpu.lock();
            state.failures += 1;
            if state.failures >= MAX_GPU_FAILURES {
                state.slot = GpuSlot::Unavailable;
                true
            } else {
                // 次に使うときに初期化し直す。
                state.slot = GpuSlot::NotInitialized;
                false
            }
        };
        self.events.warn(
            WarningCode::GpuFallback,
            format!("GPU の処理に失敗したため、CPU 版で処理しました: {error}"),
            variant_id,
            None,
        );
        if disabled {
            self.events.warn(
                WarningCode::GpuDisabled,
                "GPU の失敗が続いたため、このセッションでは CPU 版で処理します",
                None,
                None,
            );
        }
    }

    /// 現在の画面の変換。
    pub(crate) fn display_transform(&self) -> (Arc<DisplayTransform>, bool) {
        let d = self.display.lock();
        (Arc::clone(&d.transform), d.info.assumed_srgb)
    }

    /// プレビューを描く（GPU → 失敗したら CPU。6.3 節）。
    pub(crate) fn render_preview(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        request: &PreviewRequest,
        variant_id: Option<VariantId>,
    ) -> Result<Rendered, ApiError> {
        if let Some(gpu) = self.gpu() {
            let options = GpuPreviewOptions {
                download_b3: true,
                download_output: true,
                gpu_draft_a1: true,
            };
            match gpu.render_preview(&self.engine, source, settings, request, &options) {
                Ok(r) => {
                    if let Some(b3) = r.b3 {
                        return Ok(Rendered {
                            b3,
                            output: r.output,
                            histogram: r.histogram,
                            backend: RenderBackend::Gpu,
                            quality: r.quality,
                            warnings: warning_texts(&r.warnings, &r.unimplemented),
                        });
                    }
                    self.gpu_failed("GPU 版が B3 を返さなかった", variant_id);
                }
                Err(e) if e.should_fall_back_to_cpu() => {
                    self.gpu_failed(&e.to_string(), variant_id);
                }
                Err(GpuError::Pipeline(e)) => return Err(e.into()),
                Err(e) => return Err(e.into()),
            }
        }
        let r = self.engine.render_preview_with(source, settings, request)?;
        Ok(Rendered {
            b3: r.b3,
            output: r.output,
            histogram: r.histogram,
            backend: RenderBackend::Cpu,
            quality: r.quality,
            warnings: warning_texts(&r.warnings, &r.unimplemented),
        })
    }

    /// 書き出す画素を作る（GPU 版を既定とし、GPU の側の失敗なら CPU 版。2.4 節）。
    pub(crate) fn render_export(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        export: &ExportSettings,
        control: &dyn RenderControl,
        variant_id: VariantId,
    ) -> Result<(ExportedImage, RenderBackend), ApiError> {
        let gpu = self.gpu();
        let outcome = genzo_gpu::export_with_fallback(
            gpu.as_deref(),
            &self.engine,
            source,
            settings,
            export,
            &ExportOptions::default(),
            control,
        )?;
        let backend = match outcome.backend {
            genzo_gpu::ExportBackend::Gpu => RenderBackend::Gpu,
            genzo_gpu::ExportBackend::Cpu { gpu_error } => {
                if let Some(e) = gpu_error {
                    self.gpu_failed(&e, Some(variant_id));
                }
                RenderBackend::Cpu
            }
        };
        Ok((outcome.image, backend))
    }
}

/// センサー処理の警告と、適用しなかった項目を表示用の文字列（それぞれの Display。日本語の説明）にする。
pub(crate) fn warning_texts(
    warnings: &[genzo_pipeline::SensorWarning],
    unimplemented: &[genzo_pipeline::finish::UnimplementedSetting],
) -> Vec<String> {
    warnings
        .iter()
        .map(ToString::to_string)
        .chain(unimplemented.iter().map(ToString::to_string))
        .collect()
}

/// 現像の警告（カメラ行列がないなど）を、警告のイベントで知らせる（書き出し・キャッシュの作り直しの
/// 結果に載せるのとあわせて。6.3 節）。
pub(crate) fn emit_render_warnings(
    inner: &Inner,
    variant_id: VariantId,
    what: &str,
    warnings: &[String],
) {
    for w in warnings {
        inner.events.warn(
            WarningCode::Render,
            format!("{what}: {w}"),
            Some(variant_id),
            None,
        );
    }
}

/// 現像のプレビューの出力先（17a 画面）。
pub(crate) fn display_target(inner: &Inner) -> (OutputTarget, bool) {
    let (t, assumed) = inner.display_transform();
    (OutputTarget::Display(t), assumed)
}

impl Core {
    /// 画面（モニター）の ICC プロファイルを設定する（IQ-05。`None` なら取得できないとして sRGB と
    /// みなす）。読み込めない・使えないプロファイルも sRGB とみなし、警告のイベントを送る。
    ///
    /// 現像中の写真があれば、新しいプロファイルで描き直す。
    pub fn set_display_profile(&self, icc: Option<&[u8]>) -> Result<DisplayInfo, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let profile = DisplayProfile::resolve(icc).map_err(genzo_pipeline::PipelineError::from)?;
        let state = DisplayState::from_profile(profile)?;
        let info = state.info.clone();
        if let GpuSlot::Ready(r) = &inner.gpu.lock().slot
            && let Err(e) = r.register_display_profile(&state.profile)
        {
            tracing::warn!(error = %e, "画面のプロファイルを GPU に登録できない");
        }
        *inner.display.lock() = state;
        if icc.is_some() && info.assumed_srgb {
            inner.events.warn(
                WarningCode::DisplayProfileAssumedSrgb,
                format!(
                    "モニターのプロファイルが使えないため、sRGB とみなして表示します（{}）",
                    info.reason.clone().unwrap_or_default()
                ),
                None,
                None,
            );
        }
        crate::develop::rerender(inner);
        Ok(info)
    }

    /// 画面のプロファイルの情報。
    pub fn display_info(&self) -> DisplayInfo {
        self.inner.display.lock().info.clone()
    }

    /// GPU を使っているか（使っていれば、アダプターの説明）。初期化していなければ初期化する。
    pub fn gpu_adapter(&self) -> Option<String> {
        self.inner.gpu().map(|r| r.context().summary().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 警告は表示用の説明（Display。日本語）にする（Debug の名前を UI に渡さない。指摘 F27）。
    #[test]
    fn warnings_use_the_display_texts() {
        let texts = warning_texts(
            &[genzo_pipeline::SensorWarning::MissingCameraMatrix],
            &[genzo_pipeline::finish::UnimplementedSetting::Sharpening],
        );
        assert_eq!(
            texts,
            vec![
                genzo_pipeline::SensorWarning::MissingCameraMatrix.to_string(),
                genzo_pipeline::finish::UnimplementedSetting::Sharpening.to_string(),
            ]
        );
        assert!(texts[0].contains("カメラ行列"), "{texts:?}");
    }
}
