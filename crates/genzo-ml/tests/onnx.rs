//! ONNX Runtime の backend の結合テスト（機能 `onnx`）。
//!
//! ONNX Runtime の共有ライブラリは配布物・リポジトリに含めないので、環境変数 `GENZO_ORT_DYLIB`（または `ORT_DYLIB_PATH`）
//! でライブラリのファイルを指定したときだけ実行する（なければスキップして知らせる）。モデルは
//! `tests/fixtures/mean_threshold.onnx`（`make_mean_threshold.py` で作った、チャンネルの平均のしきい値のモデル。学習した
//! ものではない）。
//!
//! ```text
//! GENZO_ORT_DYLIB=/path/to/libonnxruntime.so cargo test -p genzo-ml --test onnx
//! ```

#![cfg(feature = "onnx")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use genzo_ml::{
    AiMaskKind, MlBackend, ModelLibrary, ModelSpec, OnnxBackend, Rgb8Image, Segmenter, Tensor,
};

fn dylib() -> Option<PathBuf> {
    for key in ["GENZO_ORT_DYLIB", "ORT_DYLIB_PATH"] {
        if let Some(v) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(v));
        }
    }
    eprintln!(
        "GENZO_ORT_DYLIB（ONNX Runtime の共有ライブラリ）がないので、ONNX Runtime のテストをスキップする"
    );
    None
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mean_threshold.onnx")
}

#[test]
fn onnx_backend_loads_the_library_and_runs_the_fixture_model() {
    let Some(lib) = dylib() else { return };
    let backend = OnnxBackend::load_library(&lib).unwrap();
    assert!(
        backend.name().starts_with("onnxruntime"),
        "{}",
        backend.name()
    );
    eprintln!("{}", backend.name());
    let mut session = backend.load(&ModelSpec::new(fixture())).unwrap();
    assert_eq!(session.input_names(), ["input"]);
    assert_eq!(session.output_names(), ["output"]);
    // 2 × 1: 左は平均 0.1（→ 0 に近い）、右は平均 0.9（→ 1 に近い）。
    let input = Tensor::new(vec![1, 3, 1, 2], vec![0.1, 0.9, 0.1, 0.9, 0.1, 0.9]).unwrap();
    let out = session.run(&[("input", input)]).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].shape, [1, 1, 1, 2]);
    assert!(out[0].data[0] < 0.01, "{:?}", out[0].data);
    assert!(out[0].data[1] > 0.99, "{:?}", out[0].data);
    // 形の違う入力はエラー（パニックしない）。
    let bad = Tensor::new(vec![1, 2, 1, 2], vec![0.0; 4]).unwrap();
    assert!(session.run(&[("input", bad)]).is_err());
    let missing = backend.load(&ModelSpec::new("/nonexistent/model.onnx"));
    assert!(missing.is_err());
}

#[test]
fn model_library_segments_with_onnx_runtime() {
    let Some(lib) = dylib() else { return };
    let backend = Arc::new(OnnxBackend::load_library(&lib).unwrap());
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(fixture(), dir.path().join("mean_threshold.onnx")).unwrap();
    let sha = genzo_model::sha256_hex(&std::fs::read(fixture()).unwrap());
    std::fs::write(
        dir.path().join("mean-threshold.genzo-model.json"),
        format!(
            r#"{{"id": "mean-threshold", "name": "平均のしきい値", "file": "mean_threshold.onnx", "sha256": "{sha}",
                "license": "CC0-1.0", "input": {{"name": "input", "width": 16, "height": 8}},
                "output": {{"name": "output", "activation": "none"}},
                "kinds": {{"sky": {{"channel": 0}}, "subject": {{"channel": 0}}}}}}"#
        ),
    )
    .unwrap();
    let library = ModelLibrary::scan(dir.path(), backend).unwrap();
    let status = library.status();
    assert_eq!(status.kinds, [AiMaskKind::Subject, AiMaskKind::Sky]);
    assert!(status.problems.is_empty(), "{:?}", status.problems);
    // 左半分が暗く、右半分が明るい 32 × 16 の画像。
    let mut data = Vec::new();
    for _ in 0..16 {
        for x in 0..32u8 {
            let v = if x < 16 { 30 } else { 230 };
            data.extend([v, v, v]);
        }
    }
    let img = Rgb8Image::new(32, 16, data).unwrap();
    let r = library.segment(&img, AiMaskKind::Sky).unwrap();
    assert_eq!((r.width, r.height), (32, 16));
    assert_eq!(r.model_id, "mean-threshold");
    assert!(r.values[0] < 3, "{}", r.values[0]);
    assert!(r.values[31] > 252, "{}", r.values[31]);
    assert!(matches!(
        library.segment(&img, AiMaskKind::Person),
        Err(genzo_ml::MlError::NoModel(_))
    ));
}
