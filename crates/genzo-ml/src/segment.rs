//! AI マスクの推論の入口（[`Segmenter`]）と、台帳に従った前処理・後処理（[`segment_with`]）。

use std::fmt;
use std::time::Instant;

use genzo_model::AiMaskKind;

use crate::backend::{MlError, MlSession, Tensor};
use crate::image::{Rgb8Image, resize_plane};
use crate::manifest::{Activation, InputLayout, KindSpec, ModelManifest};

/// 推論に渡す画像の長辺（画素。**仮置き**: モデルの入力はふつう 320〜1024 で、入力の画像はモデルの大きさに縮める。
/// 出力のラスターはこの大きさに戻す）。
pub const DEFAULT_INPUT_LONG_EDGE: u32 = 1024;
/// 出力のラスターの長辺の上限（画素。genzo-model の `MAX_AI_MASK_SIDE` より小さい）。
pub const MAX_OUTPUT_LONG_EDGE: u32 = 2048;

/// 推論の結果（種類の確率を 0〜255 にした重みのラスター。入力の画像と同じ大きさ、行優先）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segmentation {
    /// 使ったモデルの ID。
    pub model_id: String,
    /// 幅（画素）。
    pub width: u32,
    /// 高さ（画素）。
    pub height: u32,
    /// 重み（0〜255）。
    pub values: Vec<u8>,
    /// 推論にかかった時間（ミリ秒。前処理・後処理を含む）。
    pub elapsed_ms: u64,
}

/// 推論の仕組みの状態（画面・CLI の表示用）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SegmenterStatus {
    /// 使える種類（モデルのある種類）。
    pub kinds: Vec<AiMaskKind>,
    /// 種類ごとに使うモデルの ID（`kinds` の順）。
    pub model_ids: Vec<String>,
    /// 推論の backend の名前（例: `onnxruntime 1.22.0`）。使えなければ空。
    pub backend: String,
    /// 使えない理由（使えるなら空）。
    pub problems: Vec<String>,
}

/// AI マスクの推論（画像 → 種類ごとの重みのラスター）。
pub trait Segmenter: Send + Sync + fmt::Debug {
    /// 状態。
    fn status(&self) -> SegmenterStatus;
    /// 画像 `image` の種類 `kind` の重みのラスター。
    fn segment(&self, image: &Rgb8Image, kind: AiMaskKind) -> Result<Segmentation, MlError>;
}

/// 台帳 `manifest` とセッション `session` で、画像 `image` の種類 `kind` を推論する（モジュールの doc）。
///
/// 1. 画像を台帳の入力の大きさに縮め（双線形。縦横比は保たない）、範囲・平均・標準偏差で正規化し、並びに詰める。
/// 2. 推論し、台帳の出力（名前か最初の出力。`[1, C, H, W]` か `[1, H, W, C]`）を活性化して確率にする。
/// 3. 種類のチャンネル（またはクラスの和）の面を、入力の画像の大きさに戻し（双線形）、0〜255 にする。
pub fn segment_with(
    manifest: &ModelManifest,
    session: &mut dyn MlSession,
    image: &Rgb8Image,
    kind: AiMaskKind,
) -> Result<Segmentation, MlError> {
    let started = Instant::now();
    let Some(kind_spec) = manifest.kinds.get(&kind) else {
        return Err(MlError::NoModel(kind.display_name()));
    };
    if image.width.max(image.height) > MAX_OUTPUT_LONG_EDGE {
        return Err(MlError::InvalidInput(format!(
            "画像が大きすぎます（{} × {}。長辺 {MAX_OUTPUT_LONG_EDGE} まで）",
            image.width, image.height
        )));
    }
    let (iw, ih) = (manifest.input.width, manifest.input.height);
    let small = image.resized(iw, ih);
    let n = iw as usize * ih as usize;
    let mut data = vec![0.0f32; n * 3];
    for y in 0..ih {
        for x in 0..iw {
            let p = small.pixel(x, y);
            let i = y as usize * iw as usize + x as usize;
            for c in 0..3 {
                let src = if manifest.input.bgr { 2 - c } else { c };
                let v = manifest.normalize(c, p[src]);
                let at = match manifest.input.layout {
                    InputLayout::Nchw => c * n + i,
                    InputLayout::Nhwc => i * 3 + c,
                };
                data[at] = v;
            }
        }
    }
    let shape = match manifest.input.layout {
        InputLayout::Nchw => vec![1, 3, ih as usize, iw as usize],
        InputLayout::Nhwc => vec![1, ih as usize, iw as usize, 3],
    };
    let input = Tensor::new(shape, data).expect("形と長さは合う");
    let input_name = if manifest.input.name.is_empty() {
        session
            .input_names()
            .into_iter()
            .next()
            .ok_or_else(|| MlError::InvalidInput("モデルに入力がありません".into()))?
    } else {
        manifest.input.name.clone()
    };
    let outputs = session.run(&[(&input_name, input)])?;
    let output_index = if manifest.output.name.is_empty() {
        0
    } else {
        session
            .output_names()
            .iter()
            .position(|n| *n == manifest.output.name)
            .ok_or_else(|| {
                MlError::InvalidOutput(format!("出力 {} がありません", manifest.output.name))
            })?
    };
    let out = outputs.get(output_index).ok_or_else(|| {
        MlError::InvalidOutput(format!("出力が {} 個しかありません", outputs.len()))
    })?;
    let (channels, oh, ow) = output_dims(&out.shape, manifest.output.layout)?;
    let plane_len = oh * ow;
    // 画素 i のチャンネル c の値。
    let value = |c: usize, i: usize| -> f32 {
        match manifest.output.layout {
            InputLayout::Nchw => out.data[c * plane_len + i],
            InputLayout::Nhwc => out.data[i * channels + c],
        }
    };
    let check_channel = |c: usize| -> Result<(), MlError> {
        if c >= channels {
            Err(MlError::InvalidOutput(format!(
                "チャンネル {c} は出力の範囲の外です（{channels} チャンネル）"
            )))
        } else {
            Ok(())
        }
    };
    let mut plane = vec![0.0f32; plane_len];
    match kind_spec {
        KindSpec::Channel { channel } => {
            check_channel(*channel)?;
            for (i, p) in plane.iter_mut().enumerate() {
                *p = match manifest.output.activation {
                    Activation::None => value(*channel, i),
                    Activation::Sigmoid => sigmoid(value(*channel, i)),
                    Activation::Softmax => softmax_at(&value, channels, i, *channel),
                };
            }
        }
        KindSpec::Classes { classes } => {
            for c in classes {
                check_channel(*c)?;
            }
            for (i, p) in plane.iter_mut().enumerate() {
                *p = match manifest.output.activation {
                    Activation::None => classes.iter().map(|&c| value(c, i)).sum(),
                    // 台帳の doc のとおりクラスの確率の和（1 を超えた分は最後に 0〜1 に丸める）。
                    Activation::Sigmoid => classes.iter().map(|&c| sigmoid(value(c, i))).sum(),
                    Activation::Softmax => {
                        let m = (0..channels)
                            .map(|c| value(c, i))
                            .fold(f32::NEG_INFINITY, f32::max);
                        let denom: f32 = (0..channels).map(|c| (value(c, i) - m).exp()).sum();
                        classes
                            .iter()
                            .map(|&c| (value(c, i) - m).exp())
                            .sum::<f32>()
                            / denom
                    }
                };
            }
        }
    }
    let full = resize_plane(&plane, ow as u32, oh as u32, image.width, image.height);
    let values = full
        .iter()
        .map(|v| {
            if v.is_finite() {
                (v.clamp(0.0, 1.0) * 255.0).round() as u8
            } else {
                0
            }
        })
        .collect();
    Ok(Segmentation {
        model_id: manifest.id.clone(),
        width: image.width,
        height: image.height,
        values,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

/// 出力の形から（チャンネル数、高さ、幅）。`[1, C, H, W]`・`[C, H, W]`・`[1, H, W, C]`・`[H, W, C]`、または `[1, H, W]`・
/// `[H, W]`（チャンネル 1）。
fn output_dims(shape: &[usize], layout: InputLayout) -> Result<(usize, usize, usize), MlError> {
    let dims: Vec<usize> = match shape {
        [1, rest @ ..] if rest.len() >= 2 => rest.to_vec(),
        _ => shape.to_vec(),
    };
    let r = match (dims.as_slice(), layout) {
        ([h, w], _) => (1, *h, *w),
        ([c, h, w], InputLayout::Nchw) => (*c, *h, *w),
        ([h, w, c], InputLayout::Nhwc) => (*c, *h, *w),
        _ => {
            return Err(MlError::InvalidOutput(format!(
                "出力の形 {shape:?} を読めません"
            )));
        }
    };
    if r.0 == 0 || r.1 == 0 || r.2 == 0 {
        return Err(MlError::InvalidOutput(format!(
            "出力の形 {shape:?} が空です"
        )));
    }
    Ok(r)
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// 画素 `i` のソフトマックスのチャンネル `channel` の値。
fn softmax_at(
    value: &dyn Fn(usize, usize) -> f32,
    channels: usize,
    i: usize,
    channel: usize,
) -> f32 {
    let m = (0..channels)
        .map(|c| value(c, i))
        .fold(f32::NEG_INFINITY, f32::max);
    let denom: f32 = (0..channels).map(|c| (value(c, i) - m).exp()).sum();
    (value(channel, i) - m).exp() / denom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MlSession;

    /// 入力をそのまま返す（または決まった出力を返す）セッション。
    struct Echo {
        output: Tensor,
        seen: Vec<(String, Tensor)>,
    }

    impl MlSession for Echo {
        fn input_names(&self) -> Vec<String> {
            vec!["in".into()]
        }
        fn output_names(&self) -> Vec<String> {
            vec!["out".into()]
        }
        fn run(&mut self, inputs: &[(&str, Tensor)]) -> Result<Vec<Tensor>, MlError> {
            self.seen = inputs
                .iter()
                .map(|(n, t)| ((*n).to_owned(), t.clone()))
                .collect();
            Ok(vec![self.output.clone()])
        }
    }

    fn manifest(json: &str) -> ModelManifest {
        ModelManifest::from_json(json).unwrap()
    }

    #[test]
    fn preprocessing_follows_the_manifest() {
        let m = manifest(
            r#"{"id": "m", "file": "m.onnx",
                "input": {"width": 2, "height": 1, "range": "byte", "bgr": true},
                "output": {"activation": "none"},
                "kinds": {"sky": {"channel": 0}}}"#,
        );
        // 2 × 1 の画像: (10, 20, 30), (40, 50, 60)。
        let img = Rgb8Image::new(2, 1, vec![10, 20, 30, 40, 50, 60]).unwrap();
        let mut s = Echo {
            output: Tensor::new(vec![1, 1, 1, 2], vec![0.25, 1.5]).unwrap(),
            seen: Vec::new(),
        };
        let r = segment_with(&m, &mut s, &img, AiMaskKind::Sky).unwrap();
        assert_eq!(s.seen[0].0, "in");
        assert_eq!(s.seen[0].1.shape, [1, 3, 1, 2]);
        // NCHW・BGR: チャンネル 0 は B（30, 60）、1 は G、2 は R。
        assert_eq!(s.seen[0].1.data, [30.0, 60.0, 20.0, 50.0, 10.0, 40.0]);
        assert_eq!((r.width, r.height), (2, 1));
        assert_eq!(r.values, [64, 255]);
        assert_eq!(r.model_id, "m");
    }

    #[test]
    fn classes_and_activations_are_applied_and_resized() {
        // 出力 1 × 1 で 3 クラスの logits: softmax の和。
        let m = manifest(
            r#"{"id": "m", "file": "m.onnx",
                "input": {"width": 1, "height": 1},
                "output": {"activation": "softmax"},
                "kinds": {"sky": {"classes": [1, 2]}, "person": {"channel": 0}}}"#,
        );
        let img = Rgb8Image::new(4, 2, vec![128; 24]).unwrap();
        let mut s = Echo {
            output: Tensor::new(vec![1, 3, 1, 1], vec![0.0, 0.0, 0.0]).unwrap(),
            seen: Vec::new(),
        };
        let r = segment_with(&m, &mut s, &img, AiMaskKind::Sky).unwrap();
        assert_eq!((r.width, r.height), (4, 2));
        assert!(r.values.iter().all(|&v| v == 170), "{:?}", r.values);
        let r = segment_with(&m, &mut s, &img, AiMaskKind::Person).unwrap();
        assert!(r.values.iter().all(|&v| v == 85));
        assert!(matches!(
            segment_with(&m, &mut s, &img, AiMaskKind::Subject),
            Err(MlError::NoModel(_))
        ));
        // シグモイドのクラスは確率の和（0 → 0.5 ずつで 1 に丸める。−100 → 0 と 0 → 0.5 で 0.5）。
        let m = manifest(
            r#"{"id": "m", "file": "m.onnx",
                "input": {"width": 1, "height": 1},
                "output": {"activation": "sigmoid"},
                "kinds": {"sky": {"classes": [0, 1]}, "person": {"classes": [1, 2]}}}"#,
        );
        let mut s = Echo {
            output: Tensor::new(vec![1, 3, 1, 1], vec![0.0, 0.0, -100.0]).unwrap(),
            seen: Vec::new(),
        };
        let r = segment_with(&m, &mut s, &img, AiMaskKind::Sky).unwrap();
        assert!(r.values.iter().all(|&v| v == 255), "{:?}", r.values);
        let r = segment_with(&m, &mut s, &img, AiMaskKind::Person).unwrap();
        assert!(r.values.iter().all(|&v| v == 128), "{:?}", r.values);
        // NHWC の出力、シグモイド、範囲の外のチャンネル。
        let m = manifest(
            r#"{"id": "m", "file": "m.onnx",
                "input": {"width": 1, "height": 1, "layout": "nhwc"},
                "output": {"activation": "sigmoid", "layout": "nhwc"},
                "kinds": {"sky": {"channel": 1}, "person": {"channel": 5}}}"#,
        );
        let mut s = Echo {
            output: Tensor::new(vec![1, 1, 1, 2], vec![-100.0, 100.0]).unwrap(),
            seen: Vec::new(),
        };
        let r = segment_with(&m, &mut s, &img, AiMaskKind::Sky).unwrap();
        assert_eq!(s.seen[0].1.shape, [1, 1, 1, 3]);
        assert!(r.values.iter().all(|&v| v == 255));
        assert!(matches!(
            segment_with(&m, &mut s, &img, AiMaskKind::Person),
            Err(MlError::InvalidOutput(_))
        ));
    }

    #[test]
    fn output_shapes_are_read() {
        assert_eq!(
            output_dims(&[1, 2, 3, 4], InputLayout::Nchw).unwrap(),
            (2, 3, 4)
        );
        assert_eq!(
            output_dims(&[1, 3, 4, 2], InputLayout::Nhwc).unwrap(),
            (2, 3, 4)
        );
        assert_eq!(output_dims(&[3, 4], InputLayout::Nchw).unwrap(), (1, 3, 4));
        assert_eq!(
            output_dims(&[1, 3, 4], InputLayout::Nchw).unwrap(),
            (1, 3, 4)
        );
        assert_eq!(
            output_dims(&[2, 3, 4], InputLayout::Nchw).unwrap(),
            (2, 3, 4)
        );
        assert!(output_dims(&[1, 0, 4], InputLayout::Nchw).is_err());
        assert!(output_dims(&[5], InputLayout::Nchw).is_err());
    }
}
