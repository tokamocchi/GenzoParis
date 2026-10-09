//! RAW の展開。
//!
//! - [`types`] モジュール: パイプライン（`genzo-pipeline`）とワーカー（`genzo-worker`）が共通に使う、
//!   RAW の展開結果の型（CFA の配列・黒レベル・白レベル・撮影時の WB・カメラ行列）。
//!   LibRaw に依存しない。
//! - LibRaw の FFI（機能フラグ `libraw`）: ワーカープロセスの中だけで使う
//!   （docs/04_architecture.md の 1.2 節。PoC-2）。
//!
//! 本体は、ワーカーから受け取ったバッファを [`RawImage::validate`] で検証してから使う。

pub mod types;

pub use types::{CfaColor, CfaPattern, MAX_PIXELS, RawError, RawImage, normalize_as_shot_wb};
