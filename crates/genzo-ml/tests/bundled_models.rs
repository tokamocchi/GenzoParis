//! 配布物に同梱する AI マスクのモデル（crates/genzo-app/models。docs/ai_masks.md の 2 章）の確認。
//!
//! - 台帳が読め、モデルのファイルの SHA-256 が台帳と一致し、被写体・人物に使えること（ONNX Runtime は要らない。
//!   リポジトリのファイルが壊れていないか・台帳を変え忘れていないかを CI で確かめる）。
//! - 環境変数 `GENZO_ORT_DYLIB` で ONNX Runtime を指定したときだけ、実際に推論する（機能 `onnx`）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use genzo_ml::{AiMaskKind, ModelLibrary, Segmenter, ThresholdBackend};

fn bundled_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../genzo-app/models")
}

#[test]
fn bundled_models_are_valid() {
    let lib = ModelLibrary::scan(&bundled_dir(), Arc::new(ThresholdBackend::default())).unwrap();
    let status = lib.status();
    assert!(status.problems.is_empty(), "{:?}", status.problems);
    assert_eq!(status.kinds, [AiMaskKind::Subject, AiMaskKind::Person]);
    assert_eq!(
        status.model_ids,
        ["u2netp-subject", "mediapipe-deeplab-v3-person"]
    );
    for e in lib.entries() {
        let m = e.manifest.as_ref().unwrap();
        assert!(!m.license.is_empty() && !m.source.is_empty(), "{}", m.id);
        assert_eq!(m.sha256.len(), 64, "{}", m.id);
    }
}

/// 明るい灰色の背景の中央に、暗い円がある画像（被写体は円）。
#[cfg(feature = "onnx")]
fn disc_image(w: u32, h: u32) -> genzo_ml::Rgb8Image {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, h as f32 / 4.0);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let px = if d < r {
                [40, 30, 160]
            } else {
                [200, 200, 200]
            };
            data.extend(px);
        }
    }
    genzo_ml::Rgb8Image::new(w, h, data).unwrap()
}

#[cfg(feature = "onnx")]
#[test]
fn bundled_models_run_with_onnx_runtime() {
    let Some(lib) = std::env::var_os("GENZO_ORT_DYLIB").filter(|v| !v.is_empty()) else {
        eprintln!("GENZO_ORT_DYLIB がないので、同梱のモデルの推論のテストをスキップする");
        return;
    };
    let backend = Arc::new(genzo_ml::OnnxBackend::load_library(Path::new(&lib)).unwrap());
    let library = ModelLibrary::scan(&bundled_dir(), backend).unwrap();
    let img = disc_image(96, 64);
    let subject = library.segment(&img, AiMaskKind::Subject).unwrap();
    assert_eq!((subject.width, subject.height), (96, 64));
    assert_eq!(subject.model_id, "u2netp-subject");
    let at = |x: u32, y: u32| subject.values[(y * subject.width + x) as usize];
    // 円の中心は選ばれ、隅は選ばれない。
    assert!(at(48, 32) > 200, "中心 {}", at(48, 32));
    assert!(at(2, 2) < 50, "隅 {}", at(2, 2));
    // 人物: 人の写っていない画像では、ほぼ何も選ばない。
    let person = library.segment(&img, AiMaskKind::Person).unwrap();
    assert_eq!(person.model_id, "mediapipe-deeplab-v3-person");
    let mean =
        person.values.iter().map(|&v| f64::from(v)).sum::<f64>() / person.values.len() as f64;
    assert!(mean < 64.0, "人物の平均 {mean}");
}
