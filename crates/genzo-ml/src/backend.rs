//! 推論の抽象（docs/04_architecture.md の 7.2 節の `MlBackend`・`MlSession`）。

use std::fmt;
use std::path::{Path, PathBuf};

/// 推論のエラー。
#[derive(Debug, thiserror::Error)]
pub enum MlError {
    /// 推論の実行時のライブラリ（ONNX Runtime など）を読み込めない。
    #[error("推論のライブラリを読み込めません（{path}）: {reason}")]
    RuntimeUnavailable {
        /// 読み込もうとしたファイル。
        path: PathBuf,
        /// 理由。
        reason: String,
    },
    /// モデルのファイルを読めない・形が違う。
    #[error("モデル {path} を読み込めません: {reason}")]
    ModelLoad {
        /// モデルのファイル。
        path: PathBuf,
        /// 理由。
        reason: String,
    },
    /// 入力のテンソルがモデルに合わない。
    #[error("推論の入力が合いません: {0}")]
    InvalidInput(String),
    /// 出力のテンソルの形が台帳の記述と合わない。
    #[error("推論の出力が合いません: {0}")]
    InvalidOutput(String),
    /// 推論に失敗した。
    #[error("推論に失敗しました: {0}")]
    Inference(String),
    /// 求めた種類に使えるモデルがない。
    #[error("{0} の AI マスクに使えるモデルがありません")]
    NoModel(&'static str),
    /// 推論の仕組みがない（backend を付けていない）。
    #[error("AI マスクの推論の仕組みがありません（ONNX Runtime とモデルを入れてください）")]
    Unavailable,
}

/// f32 のテンソル（行優先。`shape` の積が `data` の長さ）。
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    /// 形（例: `[1, 3, 320, 320]`）。
    pub shape: Vec<usize>,
    /// 値（行優先）。
    pub data: Vec<f32>,
}

impl Tensor {
    /// 作る（形の積と値の長さが合わなければ `None`）。
    pub fn new(shape: Vec<usize>, data: Vec<f32>) -> Option<Self> {
        (shape.iter().product::<usize>() == data.len()).then_some(Self { shape, data })
    }

    /// 要素の数。
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// 空か。
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// モデルの指定（ファイルと、セッションの設定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    /// モデルのファイル（ONNX）。
    pub path: PathBuf,
    /// 1 回の推論に使うスレッドの数（`None` なら backend の既定）。
    pub threads: Option<usize>,
}

impl ModelSpec {
    /// ファイル `path` のモデル。
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            threads: None,
        }
    }

    /// ファイル名（表示用）。
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// 推論の backend（ONNX Runtime など。04 の 7.2 節）。
pub trait MlBackend: Send + Sync + fmt::Debug {
    /// 名前（表示・記録用。例: `"onnxruntime 1.22.0"`）。
    fn name(&self) -> String;
    /// モデルを読み込んでセッションを作る。
    fn load(&self, model: &ModelSpec) -> Result<Box<dyn MlSession>, MlError>;
}

/// 読み込んだモデルのセッション。
pub trait MlSession: Send {
    /// 入力の名前（モデルの定義の順）。
    fn input_names(&self) -> Vec<String>;
    /// 出力の名前（モデルの定義の順）。
    fn output_names(&self) -> Vec<String>;
    /// 推論する（入力は名前とテンソル。出力はモデルの定義の順）。
    fn run(&mut self, inputs: &[(&str, Tensor)]) -> Result<Vec<Tensor>, MlError>;
}

/// `path` のファイルの SHA-256（小文字の 16 進数）。
pub(crate) fn file_sha256(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tensor_checks_the_shape() {
        assert!(Tensor::new(vec![2, 3], vec![0.0; 6]).is_some());
        assert!(Tensor::new(vec![2, 3], vec![0.0; 5]).is_none());
        let t = Tensor::new(vec![1, 0], Vec::new()).unwrap();
        assert!(t.is_empty());
        assert_eq!(ModelSpec::new("/a/b.onnx").file_name(), "b.onnx");
    }

    #[test]
    fn file_sha256_matches_the_model_crate() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.bin");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(file_sha256(&p).unwrap(), genzo_model::sha256_hex(b"abc"));
    }
}
