//! テスト用の backend と推論（モデルのファイルを読まない）。genzo-api・CLI・画面のテストで、ONNX Runtime とモデルなしに
//! AI マスクの流れ（描画 → 推論 → 保存 → 設定 → 描画）を確かめるために使う。

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use genzo_model::AiMaskKind;

use crate::backend::{MlBackend, MlError, MlSession, ModelSpec, Tensor};
use crate::image::Rgb8Image;
use crate::segment::{Segmentation, Segmenter, SegmenterStatus};

/// テスト用の backend: 入力のチャンネルの平均（正規化した後の値）が `threshold` 以上なら 1、未満なら 0 の 1 チャンネルを
/// 返す（出力は `[1, 1, H, W]`）。モデルのファイルは存在すれば読まない（中身は見ない）。
#[derive(Debug, Default)]
pub struct ThresholdBackend {
    threshold: f32,
    loads: AtomicUsize,
}

impl ThresholdBackend {
    /// しきい値 `threshold`。
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold,
            loads: AtomicUsize::new(0),
        }
    }

    /// [`MlBackend::load`] を呼ばれた回数。
    pub fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }
}

impl MlBackend for ThresholdBackend {
    fn name(&self) -> String {
        format!("threshold {}", self.threshold)
    }

    fn load(&self, model: &ModelSpec) -> Result<Box<dyn MlSession>, MlError> {
        if !model.path.is_file() {
            return Err(MlError::ModelLoad {
                path: model.path.clone(),
                reason: "ファイルがありません".into(),
            });
        }
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(ThresholdSession {
            threshold: self.threshold,
        }))
    }
}

struct ThresholdSession {
    threshold: f32,
}

impl MlSession for ThresholdSession {
    fn input_names(&self) -> Vec<String> {
        vec!["input".into()]
    }
    fn output_names(&self) -> Vec<String> {
        vec!["output".into()]
    }
    fn run(&mut self, inputs: &[(&str, Tensor)]) -> Result<Vec<Tensor>, MlError> {
        let (_, t) = inputs
            .first()
            .ok_or_else(|| MlError::InvalidInput("入力がありません".into()))?;
        let (c, h, w) = match t.shape.as_slice() {
            [1, c, h, w] if *c == 3 => (*c, *h, *w),
            _ => {
                return Err(MlError::InvalidInput(format!(
                    "入力の形 {:?} は [1, 3, H, W] ではありません",
                    t.shape
                )));
            }
        };
        let n = h * w;
        let data = (0..n)
            .map(|i| {
                let mean = (0..c).map(|k| t.data[k * n + i]).sum::<f32>() / c as f32;
                if mean >= self.threshold { 1.0 } else { 0.0 }
            })
            .collect();
        Ok(vec![Tensor::new(vec![1, 1, h, w], data).expect("形は合う")])
    }
}

/// 画像と種類から重みのラスターを作る関数。
type RasterFn = dyn Fn(&Rgb8Image, AiMaskKind) -> Vec<u8> + Send + Sync;

/// テスト用の推論: 決まった関数で重みのラスターを作る（画像の大きさのまま）。
#[derive(Clone)]
pub struct FixedSegmenter {
    kinds: Vec<AiMaskKind>,
    model_id: String,
    f: Arc<RasterFn>,
}

impl fmt::Debug for FixedSegmenter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FixedSegmenter")
            .field("kinds", &self.kinds)
            .field("model_id", &self.model_id)
            .finish()
    }
}

impl FixedSegmenter {
    /// 種類 `kinds` を扱い、関数 `f`（画像と種類から `幅 × 高さ` の重み）で作る。
    pub fn new(
        kinds: Vec<AiMaskKind>,
        model_id: &str,
        f: impl Fn(&Rgb8Image, AiMaskKind) -> Vec<u8> + Send + Sync + 'static,
    ) -> Self {
        Self {
            kinds,
            model_id: model_id.to_owned(),
            f: Arc::new(f),
        }
    }

    /// すべての種類で、画像の右半分（x ≥ 幅 / 2）を 255 にする。
    pub fn right_half() -> Self {
        Self::new(AiMaskKind::ALL.to_vec(), "fixed-right-half", |img, _| {
            let w = img.width as usize;
            (0..img.height as usize)
                .flat_map(|_| (0..w).map(move |x| if x * 2 >= w { 255 } else { 0 }))
                .collect()
        })
    }

    /// 何も扱えない（モデルがない状態）。
    pub fn unavailable() -> Self {
        Self::new(Vec::new(), "none", |_, _| Vec::new())
    }
}

impl Segmenter for FixedSegmenter {
    fn status(&self) -> SegmenterStatus {
        SegmenterStatus {
            kinds: self.kinds.clone(),
            model_ids: self.kinds.iter().map(|_| self.model_id.clone()).collect(),
            backend: "fixed".into(),
            problems: if self.kinds.is_empty() {
                vec!["使えるモデルがありません".into()]
            } else {
                Vec::new()
            },
        }
    }

    fn segment(&self, image: &Rgb8Image, kind: AiMaskKind) -> Result<Segmentation, MlError> {
        if !self.kinds.contains(&kind) {
            return Err(MlError::NoModel(kind.display_name()));
        }
        let values = (self.f)(image, kind);
        if values.len() != image.width as usize * image.height as usize {
            return Err(MlError::InvalidOutput("長さが合いません".into()));
        }
        Ok(Segmentation {
            model_id: self.model_id.clone(),
            width: image.width,
            height: image.height,
            values,
            elapsed_ms: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_segmenter_marks_the_right_half() {
        let s = FixedSegmenter::right_half();
        let img = Rgb8Image::new(4, 1, vec![0; 12]).unwrap();
        let r = s.segment(&img, AiMaskKind::Sky).unwrap();
        assert_eq!(r.values, [0, 0, 255, 255]);
        assert_eq!(s.status().kinds.len(), 3);
        let none = FixedSegmenter::unavailable();
        assert!(matches!(
            none.segment(&img, AiMaskKind::Sky),
            Err(MlError::NoModel(_))
        ));
        assert_eq!(none.status().problems.len(), 1);
    }

    #[test]
    fn threshold_backend_needs_a_file_and_thresholds_the_mean() {
        let b = ThresholdBackend::new(0.5);
        assert!(b.load(&ModelSpec::new("/nonexistent/x.onnx")).is_err());
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.onnx");
        std::fs::write(&p, b"x").unwrap();
        let mut s = b.load(&ModelSpec::new(&p)).unwrap();
        assert_eq!(b.loads(), 1);
        let t = Tensor::new(vec![1, 3, 1, 2], vec![0.1, 0.9, 0.1, 0.9, 0.1, 0.9]).unwrap();
        let out = s.run(&[("input", t)]).unwrap();
        assert_eq!(out[0].shape, [1, 1, 1, 2]);
        assert_eq!(out[0].data, [0.0, 1.0]);
        let bad = Tensor::new(vec![1, 1, 1, 2], vec![0.0; 2]).unwrap();
        assert!(s.run(&[("input", bad)]).is_err());
    }
}
