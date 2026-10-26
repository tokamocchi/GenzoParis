//! ML 推論（docs/04_architecture.md の 7.2 節「AI 機能を差し込む位置」）と、AI マスク（DEV-20）の推論。
//!
//! # 構成
//!
//! | 部品 | 役割 |
//! |---|---|
//! | [`MlBackend`]・[`MlSession`]・[`Tensor`] | 推論の抽象（04 の 7.2 節のトレイト）。モデルのファイルから セッションを作り、f32 のテンソルを入れて出す |
//! | [`OnnxBackend`]（機能 `onnx`。既定） | ONNX Runtime（pyke の `ort`）の backend。共有ライブラリは実行時に指定したファイルから読み込む（ビルド時に取らない） |
//! | [`ModelManifest`] | モデルの台帳（モデルのファイルと同じフォルダの JSON）。入力の前処理・出力の読み方・種類（被写体・空・人物）との対応・出どころとライセンスを書く |
//! | [`ModelLibrary`] | モデルのフォルダの台帳を読み、種類ごとに使うモデルを選び、セッションを使い回す |
//! | [`Segmenter`]・[`Segmentation`] | AI マスクの推論の入口（画像 → 種類ごとの重みのラスター）。[`ModelLibrary`] が実装する。テスト用に [`FixedSegmenter`] |
//! | [`ThresholdBackend`] | テスト用の backend（モデルのファイルを読まず、入力の平均で決まる出力を返す） |
//!
//! # 流れ（AI マスク）
//!
//! 1. genzo-api が、写真の切り抜く前の全体（G の外接矩形）を長辺 [`DEFAULT_INPUT_LONG_EDGE`] で描き、sRGB の 8 ビットの
//!    画像（[`Rgb8Image`]）にする。
//! 2. [`Segmenter::segment`] が、台帳の前処理（大きさ・正規化・並び）でテンソルにし、backend で推論し、台帳の出力の
//!    読み方（活性化・チャンネル・クラス）で種類の確率（0〜1）の画像にし、入力の画像の大きさに戻す
//!    （[`Segmentation`]。8 ビット）。
//! 3. genzo-api がラスターをカタログに保存し、現像設定（genzo-model の `MaskShape::Ai`）から参照する。描画のたびに推論
//!    し直さない。
//!
//! モデルのファイル自体はこのリポジトリに含めない（選定・配布は人の判断。docs/ai_masks.md）。

mod backend;
mod fake;
mod image;
mod library;
mod manifest;
#[cfg(feature = "onnx")]
mod onnx;
mod segment;

pub use backend::{MlBackend, MlError, MlSession, ModelSpec, Tensor};
pub use fake::{FixedSegmenter, ThresholdBackend};
pub use genzo_model::AiMaskKind;
pub use image::Rgb8Image;
pub use library::{ModelEntry, ModelLibrary, ModelLoadError};
pub use manifest::{
    Activation, InputLayout, InputSpec, KindSpec, MANIFEST_EXTENSION, ModelManifest, OutputSpec,
    PixelRange,
};
#[cfg(feature = "onnx")]
pub use onnx::OnnxBackend;

/// この OS での ONNX Runtime の共有ライブラリのファイル名（機能 `onnx` によらず、置き場の案内に使う）。
pub fn onnxruntime_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    }
}
pub use segment::{
    DEFAULT_INPUT_LONG_EDGE, MAX_OUTPUT_LONG_EDGE, Segmentation, Segmenter, SegmenterStatus,
    segment_with,
};
