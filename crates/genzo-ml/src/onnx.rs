//! ONNX Runtime の backend（pyke の `ort`。機能 `onnx`）。
//!
//! ONNX Runtime の共有ライブラリ（`onnxruntime.dll`・`libonnxruntime.dylib`・`libonnxruntime.so`）は、ビルド時に取らず
//! （`ort` の既定の機能 `download-binaries` を外している）、[`OnnxBackend::load_library`] で指定したファイルから実行時に
//! 読み込む（`ort` の `load-dynamic`）。読み込みはプロセスで 1 度だけ（2 度目以降は最初のライブラリのまま。`ort` の
//! 制約）。実行プロバイダーは CPU だけ（CUDA・DirectML・Core ML は機能フラグを足していない。決定事項ログ No.212）。
//!
//! # テレメトリー
//!
//! Microsoft の公式の配布物の ONNX Runtime は、利用の情報（モデルの生産者・演算子・推論の回数と時間など）を
//! Microsoft に送るテレメトリーが既定で有効（ONNX Runtime の `Privacy.md`。1.31.0 で確認）。写真の情報を外に出さない
//! ため、次の 2 つで止める（決定事項ログ No.216）。
//!
//! - 環境変数 [`ORT_DISABLE_TELEMETRY_ENV`]`=1`: ONNX Runtime が初期化される前に設定されていれば、送信の仕組み
//!   （1DS）そのものを作らない。プロセスの環境変数を変えるのはほかのスレッドが動く前でなければ安全でないので、
//!   アプリ・CLI の `main` の最初で [`disable_telemetry_before_threads`] を呼ぶ。設定されていなければ、読み込みのときに
//!   警告を記録する。
//! - 環境（`Environment`）を作るときに `DisableTelemetryEvents` を呼ぶ（`ort` の `with_telemetry(false)`。`ort` の既定は
//!   有効にする側）。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use ort::session::Session;
use ort::value::Tensor as OrtTensor;

use crate::backend::{MlBackend, MlError, MlSession, ModelSpec, Tensor};

/// プロセスで 1 度だけ読み込んだライブラリ（パスと、読み込みの結果）。
static LOADED: OnceLock<(PathBuf, Result<ort::environment::Environment, String>)> = OnceLock::new();

/// ONNX Runtime の backend（モジュールの doc）。
#[derive(Debug, Clone)]
pub struct OnnxBackend {
    env: ort::environment::Environment,
    library: PathBuf,
}

impl OnnxBackend {
    /// ライブラリ `library` を読み込んで作る。読み込めなければ [`MlError::RuntimeUnavailable`]。同じプロセスで 2 度目以降は、
    /// 最初に読み込んだライブラリをそのまま使う（別のパスを指定しても読み込み直さない）。
    pub fn load_library(library: &Path) -> Result<Self, MlError> {
        let (loaded_path, result) = LOADED.get_or_init(|| {
            if !crate::telemetry_disabled_by_env() {
                tracing::warn!(
                    env = crate::ORT_DISABLE_TELEMETRY_ENV,
                    "ONNX Runtime のテレメトリーを止める環境変数が設定されていない（API では止める）"
                );
            }
            let r = ort::init_from(library)
                .map_err(|e| e.to_string())
                .and_then(|b| {
                    b.with_name("genzo")
                        .with_telemetry(false)
                        .build()
                        .map_err(|e| e.to_string())
                });
            (library.to_path_buf(), r)
        });
        match result {
            Ok(env) => {
                if loaded_path != library {
                    tracing::warn!(
                        requested = %library.display(),
                        loaded = %loaded_path.display(),
                        "ONNX Runtime はプロセスで 1 度しか読み込めないので、最初に読み込んだものを使う"
                    );
                }
                Ok(Self {
                    env: env.clone(),
                    library: loaded_path.clone(),
                })
            }
            Err(reason) => Err(MlError::RuntimeUnavailable {
                path: loaded_path.clone(),
                reason: reason.clone(),
            }),
        }
    }

    /// 読み込んだライブラリ。
    pub fn library(&self) -> &Path {
        &self.library
    }

    /// ONNX Runtime の版（ライブラリの報告）。
    pub fn version(&self) -> String {
        ort::info().to_owned()
    }
}

impl MlBackend for OnnxBackend {
    fn name(&self) -> String {
        format!("onnxruntime {}", self.version())
    }

    fn load(&self, model: &ModelSpec) -> Result<Box<dyn MlSession>, MlError> {
        let fail = |reason: String| MlError::ModelLoad {
            path: model.path.clone(),
            reason,
        };
        let mut builder = Session::builder(&self.env).map_err(|e| fail(e.to_string()))?;
        if let Some(t) = model.threads {
            builder = builder
                .with_intra_threads(t)
                .map_err(|e| fail(e.to_string()))?;
        }
        let session = builder
            .commit_from_file(&model.path)
            .map_err(|e| fail(e.to_string()))?;
        let inputs = session
            .inputs()
            .iter()
            .map(|o| o.name().to_owned())
            .collect();
        let outputs = session
            .outputs()
            .iter()
            .map(|o| o.name().to_owned())
            .collect();
        Ok(Box::new(OnnxSession {
            session: Arc::new(Mutex::new(session)),
            inputs,
            outputs,
        }))
    }
}

struct OnnxSession {
    session: Arc<Mutex<Session>>,
    inputs: Vec<String>,
    outputs: Vec<String>,
}

impl MlSession for OnnxSession {
    fn input_names(&self) -> Vec<String> {
        self.inputs.clone()
    }

    fn output_names(&self) -> Vec<String> {
        self.outputs.clone()
    }

    fn run(&mut self, inputs: &[(&str, Tensor)]) -> Result<Vec<Tensor>, MlError> {
        let mut values: Vec<(
            std::borrow::Cow<'_, str>,
            ort::session::SessionInputValue<'_>,
        )> = Vec::with_capacity(inputs.len());
        for (name, t) in inputs {
            let shape: Vec<i64> = t
                .shape
                .iter()
                .map(|&d| i64::try_from(d).unwrap_or(i64::MAX))
                .collect();
            let value = OrtTensor::<f32>::from_array((shape, t.data.clone()))
                .map_err(|e| MlError::InvalidInput(e.to_string()))?;
            values.push((std::borrow::Cow::<str>::from(*name), value.into()));
        }
        let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        let outputs = session
            .run(values)
            .map_err(|e| MlError::Inference(e.to_string()))?;
        let mut out = Vec::with_capacity(self.outputs.len());
        for name in &self.outputs {
            let v = outputs
                .get(name.as_str())
                .ok_or_else(|| MlError::InvalidOutput(format!("出力 {name} がありません")))?;
            let (shape, data) = v
                .try_extract_tensor::<f32>()
                .map_err(|e| MlError::InvalidOutput(format!("{name}: {e}")))?;
            let shape: Vec<usize> = shape
                .iter()
                .map(|&d| usize::try_from(d).unwrap_or(0))
                .collect();
            out.push(Tensor::new(shape, data.to_vec()).ok_or_else(|| {
                MlError::InvalidOutput(format!("出力 {name} の形と長さが合いません"))
            })?);
        }
        Ok(out)
    }
}
