//! モデルの台帳（[`ModelManifest`]。モデルのファイルと同じフォルダに置く JSON。docs/ai_masks.md）。
//!
//! モデルごとに、入力の前処理（大きさ・画素の範囲・平均と標準偏差・並び）、出力の読み方（活性化・チャンネル）、
//! AI マスクの種類（被写体・空・人物）との対応、出どころとライセンスを書く。アプリのコードはモデルの中身を知らず、
//! この台帳だけで前処理と後処理を決める（モデルを差し替えても、コードを変えない）。
//!
//! ```json
//! {
//!   "id": "u2netp-2020",
//!   "name": "U²-Net (small)",
//!   "file": "u2netp.onnx",
//!   "sha256": "<ファイルの SHA-256。省略なら確かめない>",
//!   "license": "Apache-2.0",
//!   "source": "https://example.invalid/u2netp",
//!   "input": {"name": "input", "width": 320, "height": 320, "layout": "nchw", "range": "unit",
//!             "mean": [0.485, 0.456, 0.406], "std": [0.229, 0.224, 0.225]},
//!   "output": {"name": "output", "activation": "sigmoid"},
//!   "kinds": {"subject": {"channel": 0}}
//! }
//! ```
//!
//! 多クラスのセマンティックセグメンテーションのモデルでは、`output.activation` を `softmax`（または logits でなければ
//! `none`）にし、種類ごとにクラスの番号の列を書く（`"sky": {"classes": [2]}`。確率はクラスの和）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use genzo_model::AiMaskKind;
use serde::{Deserialize, Serialize};

/// 台帳のファイルの拡張子（`<名前>.genzo-model.json`）。
pub const MANIFEST_EXTENSION: &str = ".genzo-model.json";

/// モデルの ID の長さの上限（genzo-model の `MAX_AI_MODEL_ID_CHARS` と同じ）。
const MAX_ID_CHARS: usize = genzo_model::MAX_AI_MODEL_ID_CHARS;

/// 入力のテンソルの並び。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputLayout {
    /// `[1, 3, 高さ, 幅]`（PyTorch 由来のモデルの既定）。
    #[default]
    Nchw,
    /// `[1, 高さ, 幅, 3]`（TensorFlow 由来のモデル）。
    Nhwc,
}

/// 入力の画素の値の範囲（平均・標準偏差を引く前）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelRange {
    /// 0〜1。
    #[default]
    Unit,
    /// 0〜255。
    Byte,
    /// −1〜1。
    Signed,
}

/// 出力の活性化（モデルの出力を確率にする処理）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activation {
    /// そのまま（すでに 0〜1 の確率）。
    #[default]
    None,
    /// シグモイド（チャンネルごと）。
    Sigmoid,
    /// ソフトマックス（チャンネルの軸）。
    Softmax,
}

/// 入力の前処理。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputSpec {
    /// 入力のテンソルの名前（空ならモデルの最初の入力）。
    pub name: String,
    /// 幅（画素。モデルの固定の入力の大きさ）。
    pub width: u32,
    /// 高さ（画素）。
    pub height: u32,
    /// 並び。
    pub layout: InputLayout,
    /// 画素の値の範囲。
    pub range: PixelRange,
    /// チャンネルごとの平均（範囲に直した後に引く。空なら 0）。
    pub mean: Vec<f32>,
    /// チャンネルごとの標準偏差（平均を引いた後に割る。空なら 1）。
    pub std: Vec<f32>,
    /// チャンネルの順を BGR にするか。
    pub bgr: bool,
}

impl Default for InputSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            width: 320,
            height: 320,
            layout: InputLayout::Nchw,
            range: PixelRange::Unit,
            mean: Vec::new(),
            std: Vec::new(),
            bgr: false,
        }
    }
}

/// 出力の読み方。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSpec {
    /// 出力のテンソルの名前（空ならモデルの最初の出力）。
    pub name: String,
    /// 活性化。
    pub activation: Activation,
    /// 出力の並び（`[1, C, H, W]` か `[1, H, W, C]`）。
    pub layout: InputLayout,
}

/// 種類（被写体・空・人物）ごとの、出力からの読み方。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KindSpec {
    /// 1 つのチャンネルの値（顕著性・2 値のモデル）。
    Channel {
        /// チャンネルの番号。
        channel: usize,
    },
    /// クラスの番号の列（確率の和。多クラスのモデル）。
    Classes {
        /// クラスの番号。
        classes: Vec<usize>,
    },
}

/// モデルの台帳（モジュールの doc）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelManifest {
    /// モデルの ID（現像設定に記録する。1〜100 文字）。
    pub id: String,
    /// 表示の名前。
    pub name: String,
    /// モデルのファイル（台帳と同じフォルダからの相対パス）。
    pub file: String,
    /// モデルのファイルの SHA-256（小文字の 16 進数。空なら確かめない）。
    pub sha256: String,
    /// ライセンス（SPDX の式など。表示用）。
    pub license: String,
    /// 出どころ（URL など。表示用）。
    pub source: String,
    /// 入力の前処理。
    pub input: InputSpec,
    /// 出力の読み方。
    pub output: OutputSpec,
    /// 種類ごとの読み方（この台帳が扱える種類だけ）。
    pub kinds: BTreeMap<AiMaskKind, KindSpec>,
    /// 1 回の推論に使うスレッドの数（省略なら backend の既定）。
    pub threads: Option<usize>,
}

impl ModelManifest {
    /// JSON から読む。
    pub fn from_json(json: &str) -> Result<Self, String> {
        let m: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        m.check()?;
        Ok(m)
    }

    /// ファイルから読む。
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::from_json(&text)
    }

    /// 内容を確かめる（ID・ファイル・大きさ・種類）。
    pub fn check(&self) -> Result<(), String> {
        let id_chars = self.id.chars().count();
        if id_chars == 0 || id_chars > MAX_ID_CHARS {
            return Err(format!("id は 1〜{MAX_ID_CHARS} 文字にしてください"));
        }
        if self
            .id
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '/' || c == '\\')
        {
            return Err("id に空白・制御文字・区切り（/ \\）は使えません".into());
        }
        if self.file.is_empty() {
            return Err("file（モデルのファイル）がありません".into());
        }
        let file = Path::new(&self.file);
        if file.is_absolute()
            || file
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err("file は台帳と同じフォルダからの相対パス（.. なし）にしてください".into());
        }
        if !self.sha256.is_empty()
            && !(self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err("sha256 は小文字の 16 進数 64 文字にしてください".into());
        }
        if self.input.width == 0 || self.input.height == 0 {
            return Err("input の width・height は 1 以上にしてください".into());
        }
        if self.input.width > 4096 || self.input.height > 4096 {
            return Err("input の width・height は 4096 以下にしてください".into());
        }
        for (what, v) in [("mean", &self.input.mean), ("std", &self.input.std)] {
            if !(v.is_empty() || v.len() == 1 || v.len() == 3) {
                return Err(format!("input の {what} は 1 つか 3 つにしてください"));
            }
            if v.iter().any(|x| !x.is_finite()) {
                return Err(format!("input の {what} は有限の数にしてください"));
            }
        }
        if self.input.std.contains(&0.0) {
            return Err("input の std に 0 は使えません".into());
        }
        if self.kinds.is_empty() {
            return Err("kinds（扱える種類）がありません".into());
        }
        for (k, spec) in &self.kinds {
            if let KindSpec::Classes { classes } = spec
                && classes.is_empty()
            {
                return Err(format!("kinds.{} の classes が空です", k.as_str()));
            }
        }
        if matches!(self.threads, Some(0)) {
            return Err("threads は 1 以上にしてください".into());
        }
        Ok(())
    }

    /// 種類 `kind` を扱えるか。
    pub fn supports(&self, kind: AiMaskKind) -> bool {
        self.kinds.contains_key(&kind)
    }

    /// 台帳のファイル `manifest_path` から見た、モデルのファイルの場所。
    pub fn model_path(&self, manifest_path: &Path) -> PathBuf {
        manifest_path
            .parent()
            .map(|d| d.join(&self.file))
            .unwrap_or_else(|| PathBuf::from(&self.file))
    }

    /// 入力の 1 画素の値（`c` は RGB のチャンネルの番号 0〜2、`v` は 0〜255）。
    pub fn normalize(&self, c: usize, v: u8) -> f32 {
        let x = f32::from(v);
        let x = match self.input.range {
            PixelRange::Unit => x / 255.0,
            PixelRange::Byte => x,
            PixelRange::Signed => x / 127.5 - 1.0,
        };
        let pick = |v: &[f32], default: f32| match v.len() {
            0 => default,
            1 => v[0],
            _ => v[c],
        };
        (x - pick(&self.input.mean, 0.0)) / pick(&self.input.std, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
        "id": "test-model", "name": "Test", "file": "model.onnx",
        "input": {"name": "input", "width": 32, "height": 24, "mean": [0.5], "std": [0.25]},
        "output": {"name": "output", "activation": "sigmoid"},
        "kinds": {"subject": {"channel": 0}, "sky": {"classes": [2, 3]}}
    }"#;

    #[test]
    fn reads_the_manifest_and_defaults() {
        let m = ModelManifest::from_json(GOOD).unwrap();
        assert_eq!(m.id, "test-model");
        assert_eq!(m.input.layout, InputLayout::Nchw);
        assert_eq!(m.input.range, PixelRange::Unit);
        assert_eq!(m.output.activation, Activation::Sigmoid);
        assert!(m.supports(AiMaskKind::Subject) && m.supports(AiMaskKind::Sky));
        assert!(!m.supports(AiMaskKind::Person));
        assert_eq!(
            m.kinds[&AiMaskKind::Subject],
            KindSpec::Channel { channel: 0 }
        );
        assert_eq!(
            m.kinds[&AiMaskKind::Sky],
            KindSpec::Classes {
                classes: vec![2, 3]
            }
        );
        assert_eq!(
            m.model_path(Path::new("/models/a.genzo-model.json")),
            PathBuf::from("/models/model.onnx")
        );
        // (255/255 − 0.5) / 0.25 = 2
        assert!((m.normalize(0, 255) - 2.0).abs() < 1e-6);
        assert!((m.normalize(2, 0) + 2.0).abs() < 1e-6);
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(ModelManifest::from_json(&json).unwrap(), m);
    }

    #[test]
    fn rejects_bad_manifests() {
        let bad = |patch: &dyn Fn(&mut ModelManifest)| {
            let mut m = ModelManifest::from_json(GOOD).unwrap();
            patch(&mut m);
            m.check().is_err()
        };
        assert!(bad(&|m| m.id = String::new()));
        assert!(bad(&|m| m.id = "a b".into()));
        assert!(bad(&|m| m.file = "../x.onnx".into()));
        assert!(bad(&|m| m.file = String::new()));
        assert!(bad(&|m| m.sha256 = "XYZ".into()));
        assert!(bad(&|m| m.input.width = 0));
        assert!(bad(&|m| m.input.mean = vec![1.0, 2.0]));
        assert!(bad(&|m| m.input.std = vec![0.0]));
        assert!(bad(&|m| m.kinds.clear()));
        assert!(bad(&|m| {
            m.kinds
                .insert(AiMaskKind::Person, KindSpec::Classes { classes: vec![] });
        }));
        assert!(bad(&|m| m.threads = Some(0)));
        assert!(ModelManifest::from_json("{").is_err());
    }

    #[test]
    fn pixel_ranges_are_applied() {
        let mut m = ModelManifest::from_json(GOOD).unwrap();
        m.input.mean.clear();
        m.input.std.clear();
        m.input.range = PixelRange::Byte;
        assert_eq!(m.normalize(0, 200), 200.0);
        m.input.range = PixelRange::Signed;
        assert!((m.normalize(0, 255) - 1.0).abs() < 1e-6);
        assert!((m.normalize(0, 0) + 1.0).abs() < 1e-6);
    }
}
