//! AI マスク（DEV-20。被写体・空・人物の自動の選択。docs/04_architecture.md の 2.1 節「ローカル補正」・7.2 節、
//! docs/ai_masks.md）。
//!
//! # 流れ（[`Core::compute_ai_mask`]）
//!
//! 1. 現像中の写真の切り抜く前の全体（G の外接矩形）を、今の設定でプレビューと同じ A1 から sRGB の 8 ビットの画像に描く
//!    （CPU 版。長辺は genzo-ml の [`DEFAULT_INPUT_LONG_EDGE`] まで縮める）。
//! 2. 推論の仕組み（[`Segmenter`]。既定は genzo-ml の [`ModelLibrary`] と ONNX Runtime。テストでは
//!    [`CoreHooks::segmenter`](crate::CoreHooks::segmenter) で差し替える）で、種類の重みのラスターを作る。
//! 3. ラスターを 8 ビットのグレースケールの PNG にしてカタログ（`ai_mask` の表。内容の SHA-256 がキー）に保存する。
//! 4. 設定に入れる形（genzo-model の `MaskShape::Ai`。参照・大きさ・G 座標の範囲）を返す。設定への適用は呼び出し側
//!    （画面・CLI）が `set_settings` で行う（色域のスポイトと同じ。履歴の名前は「マスクを追加」など）。
//!
//! 描画のときは、エンジンの [`ProfileResolver`]（[`Resolvers`]）がカタログからラスターを引く（[`AiMaskStore`]。メモリに
//! 使い回す）。見つからなければ重み 0 で描き、警告する（genzo-pipeline の `SensorWarning::AiMaskMissing`）。
//!
//! # モデルと ONNX Runtime の置き場
//!
//! - モデルのフォルダ: [`CoreConfig::models_dir`](crate::CoreConfig::models_dir) → 環境変数 [`ENV_MODELS_DIR`] → OS の
//!   アプリのデータのフォルダの `models`（プリセットと同じ場所の決め方。アプリと CLI で同じ）。
//! - ONNX Runtime の共有ライブラリ: [`CoreConfig::onnxruntime`](crate::CoreConfig::onnxruntime) → 環境変数
//!   [`ENV_ONNXRUNTIME`] → モデルのフォルダ → 実行ファイルと同じフォルダ（配布物に同梱したとき）。
//!
//! どちらもなければ、状態（[`Core::ai_mask_status`]）で理由を知らせ、計算はエラー（[`ApiError::AiMask`]）になる。

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use genzo_catalog::{AiMaskInsert, Catalog};
use genzo_color::{CubeLut, DcpProfile};
use genzo_media::png_io::{decode_gray8_png, encode_gray8_png};
use genzo_ml::{DEFAULT_INPUT_LONG_EDGE, MlError, Rgb8Image, Segmenter};
use genzo_model::RenderQuality;
use genzo_model::{AiMaskKind, DataRef, MaskShape, VariantId};
use genzo_pipeline::engine::{OutputTarget, PreviewRequest};
use genzo_pipeline::finish::output::{Dither, quantize_u8};
use genzo_pipeline::profile::Resolved;
use genzo_pipeline::{AiMaskRaster, ProfileResolver};

use crate::core::{Core, Inner};
use crate::develop::session_snapshot;
use crate::error::ApiError;
use crate::profiles::ProfileLibrary;
use crate::types::{AiMaskCollectReport, AiMaskResult, AiMaskStatusInfo};

/// モデルのフォルダを指定する環境変数。
pub const ENV_MODELS_DIR: &str = "GENZO_MODELS_DIR";
/// ONNX Runtime の共有ライブラリを指定する環境変数。
pub const ENV_ONNXRUNTIME: &str = "GENZO_ONNXRUNTIME";
/// OS のアプリのデータのフォルダの中の、モデルのフォルダの名前。
pub const MODELS_DIR: &str = "models";
/// メモリに使い回すラスターの数の上限（**仮置き**: 1 枚あたり長辺 1024 で 1 MB 弱）。
const CACHE_ENTRIES: usize = 32;

/// 既定のモデルのフォルダ（モジュールの doc。分からなければ `None`）。
pub fn default_models_dir() -> Option<PathBuf> {
    models_dir_from(|k| std::env::var_os(k))
}

fn models_dir_from(get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(dir) = get(ENV_MODELS_DIR).filter(|v| !v.is_empty()) {
        let dir = PathBuf::from(dir);
        return Some(std::path::absolute(&dir).unwrap_or(dir));
    }
    crate::presets::app_data_subdir(&get, MODELS_DIR)
}

/// ONNX Runtime の共有ライブラリの場所の候補（モジュールの doc の順。存在するものだけ）。
pub fn onnxruntime_candidates(
    configured: Option<&Path>,
    models_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let name = genzo_ml::onnxruntime_file_name();
    let mut out = Vec::new();
    if let Some(p) = configured {
        out.push(p.to_path_buf());
    }
    if let Some(p) = std::env::var_os(ENV_ONNXRUNTIME).filter(|v| !v.is_empty()) {
        out.push(PathBuf::from(p));
    }
    if let Some(d) = models_dir {
        out.push(d.join(name));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        out.push(dir.join(name));
        out.push(dir.join("lib").join(name));
    }
    out.into_iter().filter(|p| p.is_file()).collect()
}

/// カタログの AI マスクのラスターを描画のために引く（メモリに使い回す）。
#[derive(Debug)]
pub(crate) struct AiMaskStore {
    catalog: Arc<parking_lot::Mutex<Option<Catalog>>>,
    cache: Mutex<HashMap<String, Arc<AiMaskRaster>>>,
    order: Mutex<Vec<String>>,
}

impl AiMaskStore {
    pub(crate) fn new(catalog: Arc<parking_lot::Mutex<Option<Catalog>>>) -> Self {
        Self {
            catalog,
            cache: Mutex::new(HashMap::new()),
            order: Mutex::new(Vec::new()),
        }
    }

    /// SHA-256 `sha256` のラスター（カタログになければ `None`）。
    pub(crate) fn get(&self, sha256: &str) -> Option<Arc<AiMaskRaster>> {
        if let Some(r) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(sha256)
        {
            return Some(r.clone());
        }
        let record = {
            let guard = self.catalog.lock();
            let cat = guard.as_ref()?;
            match cat.ai_mask(sha256) {
                Ok(r) => r?,
                Err(e) => {
                    tracing::warn!(sha256, error = %e, "AI マスクのラスターを読めない");
                    return None;
                }
            }
        };
        let values = match record.encoding.as_str() {
            "raw" => record.data,
            "png" => match decode_gray8_png(&record.data) {
                Ok((w, h, v)) if (w, h) == (record.width, record.height) => v,
                Ok((w, h, _)) => {
                    tracing::warn!(sha256, w, h, "AI マスクのラスターの大きさが記録と違う");
                    return None;
                }
                Err(e) => {
                    tracing::warn!(sha256, error = %e, "AI マスクのラスターの PNG を読めない");
                    return None;
                }
            },
            other => {
                tracing::warn!(
                    sha256,
                    encoding = other,
                    "AI マスクのラスターの符号化を扱えない"
                );
                return None;
            }
        };
        let raster = Arc::new(AiMaskRaster::new(record.width, record.height, values)?);
        self.insert(sha256, raster.clone());
        Some(raster)
    }

    /// 使い回しているものをすべて捨てる（カタログからラスターを消した後）。
    pub(crate) fn clear(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.order.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub(crate) fn insert(&self, sha256: &str, raster: Arc<AiMaskRaster>) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let mut order = self.order.lock().unwrap_or_else(|e| e.into_inner());
        if cache.insert(sha256.to_owned(), raster).is_none() {
            order.push(sha256.to_owned());
        }
        while order.len() > CACHE_ENTRIES {
            let old = order.remove(0);
            cache.remove(&old);
        }
    }
}

/// エンジンに渡す、設定の参照からデータを引く仕組み（カメラプロファイル・ルックはライブラリ、AI マスクのラスターは
/// カタログ）。
#[derive(Debug)]
pub(crate) struct Resolvers {
    pub profiles: Arc<ProfileLibrary>,
    pub ai_masks: Arc<AiMaskStore>,
}

impl ProfileResolver for Resolvers {
    fn camera_profile(&self, id: &str) -> Option<Resolved<DcpProfile>> {
        self.profiles.camera_profile(id)
    }
    fn look(&self, id: &str) -> Option<Resolved<CubeLut>> {
        self.profiles.look(id)
    }
    fn ai_mask(&self, data_ref: &DataRef) -> Option<Arc<AiMaskRaster>> {
        self.ai_masks.get(&data_ref.sha256)
    }
}

/// 推論の仕組みを用意する（初めて要るときに 1 度だけ。[`Inner::segmenter`]）。
pub(crate) fn build_segmenter(inner: &Inner) -> Result<Arc<dyn Segmenter>, String> {
    if let Some(s) = &inner.segmenter_hook {
        return Ok(s.clone());
    }
    let models_dir = inner
        .config
        .models_dir
        .clone()
        .or_else(default_models_dir)
        .ok_or_else(|| {
            "モデルのフォルダが分かりません（GENZO_MODELS_DIR を設定してください）".to_owned()
        })?;
    let backend = onnx_backend(inner.config.onnxruntime.as_deref(), Some(&models_dir))?;
    let library = genzo_ml::ModelLibrary::scan(&models_dir, backend).map_err(|e| e.to_string())?;
    Ok(Arc::new(library))
}

#[cfg(feature = "onnx")]
fn onnx_backend(
    configured: Option<&Path>,
    models_dir: Option<&Path>,
) -> Result<Arc<dyn genzo_ml::MlBackend>, String> {
    let candidates = onnxruntime_candidates(configured, models_dir);
    let Some(path) = candidates.first() else {
        return Err(format!(
            "ONNX Runtime の共有ライブラリ（{}）が見つかりません（{ENV_ONNXRUNTIME} か、モデルのフォルダ・実行ファイルのフォルダに置いてください）",
            genzo_ml::onnxruntime_file_name()
        ));
    };
    let backend = genzo_ml::OnnxBackend::load_library(path).map_err(|e| e.to_string())?;
    tracing::info!(path = %path.display(), version = %backend.version(), "ONNX Runtime を読み込んだ");
    Ok(Arc::new(backend))
}

#[cfg(not(feature = "onnx"))]
fn onnx_backend(
    _configured: Option<&Path>,
    _models_dir: Option<&Path>,
) -> Result<Arc<dyn genzo_ml::MlBackend>, String> {
    Err("このビルドには ONNX Runtime の backend がありません（機能 onnx）".to_owned())
}

fn ml_error(e: MlError) -> ApiError {
    match e {
        MlError::NoModel(_) | MlError::Unavailable | MlError::RuntimeUnavailable { .. } => {
            ApiError::AiMask(e.to_string())
        }
        MlError::InvalidInput(m) => ApiError::InvalidArgument(m),
        other => ApiError::Internal(other.to_string()),
    }
}

impl Core {
    /// AI マスクの推論の仕組みの状態（使える種類・モデル・backend・問題）。カタログの中のラスターの数と大きさも。
    pub fn ai_mask_status(&self) -> Result<AiMaskStatusInfo, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let (rasters, raster_bytes) = inner.with_catalog(|c| c.ai_mask_totals())?;
        let models_dir = inner.config.models_dir.clone().or_else(default_models_dir);
        let mut info = AiMaskStatusInfo {
            available: false,
            kinds: Vec::new(),
            models: Vec::new(),
            backend: String::new(),
            problems: Vec::new(),
            models_dir,
            onnxruntime: None,
            rasters,
            raster_bytes,
        };
        match inner.segmenter() {
            Ok(s) => {
                let st = s.status();
                info.available = !st.kinds.is_empty();
                info.kinds = st.kinds;
                info.models = st.model_ids;
                info.backend = st.backend;
                info.problems = st.problems;
            }
            Err(reason) => info.problems.push(reason),
        }
        #[cfg(feature = "onnx")]
        {
            info.onnxruntime = onnxruntime_candidates(
                inner.config.onnxruntime.as_deref(),
                info.models_dir.as_deref(),
            )
            .into_iter()
            .next();
        }
        Ok(info)
    }

    /// 現像中の写真 `variant_id` の種類 `kind` の AI マスクを計算して保存し、設定に入れる形を返す（モジュールの doc）。
    ///
    /// エラー: 写真を開いていない・別の variant なら [`ApiError::NoDevelopSession`]。推論の仕組み・モデルがなければ
    /// [`ApiError::AiMask`]。
    pub fn compute_ai_mask(
        &self,
        variant_id: VariantId,
        kind: AiMaskKind,
    ) -> Result<AiMaskResult, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let segmenter = inner.segmenter().map_err(ApiError::AiMask)?;
        let snap = session_snapshot(inner, variant_id)?;
        let started = std::time::Instant::now();
        // 1. 切り抜く前の全体を sRGB の 8 ビットで描く（CPU 版。プレビューと同じ A1）。
        let request = PreviewRequest::new(
            RenderQuality::Final,
            inner.engine.config().preview_long_edge,
        )
        .with_output(OutputTarget::assumed_srgb_display()?)
        .with_uncropped(true);
        let r = inner.render_preview(&snap.source, &snap.settings, &request, Some(variant_id))?;
        let output = r
            .output
            .ok_or_else(|| ApiError::Internal("推論に渡す画像がない".to_owned()))?;
        let (w, h) = output.dimensions();
        let rgb8 = quantize_u8(&output, (0, 0), Dither::None)?;
        let image = Rgb8Image::new(w, h, rgb8)
            .ok_or_else(|| ApiError::Internal("推論に渡す画像の大きさが合わない".to_owned()))?;
        let long = w.max(h);
        let image = if long > DEFAULT_INPUT_LONG_EDGE {
            let scale = f64::from(DEFAULT_INPUT_LONG_EDGE) / f64::from(long);
            let tw = ((f64::from(w) * scale).round() as u32).max(1);
            let th = ((f64::from(h) * scale).round() as u32).max(1);
            image.resized(tw, th)
        } else {
            image
        };
        let render_ms = started.elapsed().as_millis() as u64;
        // 2. 推論。
        let seg = segmenter.segment(&image, kind).map_err(ml_error)?;
        // 3. カタログに保存（内容の SHA-256 がキー。同じなら 1 行）。
        // 内容のアドレス（幅・高さも含める。同じ値の並びで大きさの違うラスターを別のものにする）。
        let sha256 = genzo_model::ai_mask_sha256(seg.width, seg.height, &seg.values);
        let png = encode_gray8_png(seg.width, seg.height, &seg.values)?;
        let asset_id = inner.with_catalog(|c| c.asset_of_variant(variant_id))?;
        let insert = AiMaskInsert {
            sha256: sha256.clone(),
            asset_id,
            kind: kind.as_str().to_owned(),
            model: seg.model_id.clone(),
            width: seg.width,
            height: seg.height,
            encoding: "png".to_owned(),
            data: png,
        };
        inner.with_catalog(|c| c.put_ai_mask(&insert))?;
        inner.ai_masks.insert(
            &sha256,
            Arc::new(
                AiMaskRaster::new(seg.width, seg.height, seg.values.clone())
                    .ok_or_else(|| ApiError::Internal("ラスターの長さが合わない".to_owned()))?,
            ),
        );
        // 4. 設定に入れる形（G 座標の範囲は描いた画像の縁）。
        let b = r.g_bounds;
        let shape = MaskShape::Ai {
            kind,
            model: seg.model_id.clone(),
            mask: DataRef::new(kind.ref_id(), sha256.clone()),
            width: seg.width,
            height: seg.height,
            bounds: [b[0] as f32, b[1] as f32, b[2] as f32, b[3] as f32],
        };
        tracing::info!(
            variant = variant_id.get(),
            kind = kind.as_str(),
            model = %seg.model_id,
            width = seg.width,
            height = seg.height,
            render_ms,
            inference_ms = seg.elapsed_ms,
            "AI マスクを計算した"
        );
        Ok(AiMaskResult {
            variant_id,
            kind,
            shape,
            model: seg.model_id,
            sha256,
            width: seg.width,
            height: seg.height,
            render_ms,
            inference_ms: seg.elapsed_ms,
        })
    }

    /// どの設定・履歴・スナップショットにも参照されていない AI マスクのラスターをカタログから消す（カタログの保守）。
    pub fn collect_ai_masks(&self) -> Result<AiMaskCollectReport, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        self.flush_develop()?;
        let removed = inner.with_catalog(|c| c.delete_unreferenced_ai_masks())?;
        if removed > 0 {
            inner.ai_masks.clear();
        }
        let (remaining, remaining_bytes) = inner.with_catalog(|c| c.ai_mask_totals())?;
        Ok(AiMaskCollectReport {
            removed: removed as u64,
            remaining,
            remaining_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_dir_prefers_the_environment_variable() {
        // どの OS でも絶対パスになる場所（Windows では `/m` はドライブを補った `D:\m` になるため）。
        let base = std::env::temp_dir().join("genzo-models-test");
        let m = base.join("m");
        let dir = models_dir_from(|k| (k == ENV_MODELS_DIR).then(|| m.clone().into_os_string()));
        assert_eq!(dir, Some(m.clone()));
        // 環境変数が空なら、OS のアプリのデータのフォルダ（Windows は APPDATA、macOS は HOME、ほかは
        // XDG_DATA_HOME）の下の models。
        let dir = models_dir_from(|k| match k {
            ENV_MODELS_DIR => Some(OsString::from("")),
            "APPDATA" | "HOME" | "XDG_DATA_HOME" => Some(base.clone().into_os_string()),
            _ => None,
        });
        let d = dir.unwrap();
        assert!(d.ends_with(MODELS_DIR), "{}", d.display());
        assert!(d.starts_with(&base), "{}", d.display());
    }

    #[test]
    fn ml_errors_are_classified() {
        assert!(matches!(
            ml_error(MlError::NoModel("空")),
            ApiError::AiMask(_)
        ));
        assert!(matches!(
            ml_error(MlError::Inference("x".into())),
            ApiError::Internal(_)
        ));
        assert!(matches!(
            ml_error(MlError::InvalidInput("x".into())),
            ApiError::InvalidArgument(_)
        ));
    }
}
