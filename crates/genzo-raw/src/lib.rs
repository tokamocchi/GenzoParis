//! RAW の展開。
//!
//! - [`types`] モジュール: パイプライン（`genzo-pipeline`）とワーカー（`genzo-worker`）が共通に使う、
//!   RAW の展開結果の型（CFA の配列・黒レベル・白レベル・撮影時の WB・カメラ行列）。
//!   LibRaw に依存しない。
//! - LibRaw の FFI（機能フラグ `libraw`）: ワーカープロセスの中だけで使う
//!   （docs/04_architecture.md の 1.2 節。PoC-2）。公開 API は [`decode_file`]・
//!   [`read_metadata`]・[`extract_thumbnail`]・[`libraw_version`]・[`decoder_id`]。
//!   機能フラグが無効の build でも同じ関数があり、[`RawError::Unsupported`] を返す。
//!   LibRaw の C++ API は C++ のシム（`src/shim/genzo_libraw_shim.cpp`）を通して呼ぶ。
//! - [`dng`] モジュール: テスト用の合成 DNG の書き出し（機能フラグに関係なく使える）。
//!
//! 本体は、ワーカーから受け取ったバッファを [`RawImage::validate`] で検証してから使う。

mod convert;
mod decode;
pub mod dng;
#[cfg(feature = "libraw")]
mod libraw;
pub mod thumbnail;
pub mod types;
// Windows の長いパスを LibRaw に渡す形にする（Windows で `libraw` が有効なときだけ使う。
// 文字列の変換だけなので、テストはどの OS でも行う）。
#[cfg(any(all(windows, feature = "libraw"), test))]
mod win_long_path;

pub use decode::{
    CamXyzSource, CaptureTimeSource, DecodedRaw, DngColorMatrixInfo, LIBRAW_ENABLED, RawDetails,
    WbSource, decode_bytes, decode_file, decode_file_with_details, decoder_id, extract_thumbnail,
    extract_thumbnail_bytes, libraw_version, read_metadata, read_metadata_bytes,
};
pub use thumbnail::{EmbeddedThumbnail, MAX_THUMBNAIL_BYTES, ThumbnailFormat};
pub use types::{CfaColor, CfaPattern, MAX_PIXELS, RawError, RawImage, normalize_as_shot_wb};
