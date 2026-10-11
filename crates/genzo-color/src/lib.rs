//! 色の処理の基盤。
//!
//! 色空間（リニア Rec.2020・Display P3・sRGB・Adobe RGB）の定義、行列、伝達関数
//! （IEC 61966-2-1 など）、Lab・ΔE2000・OKLab の計算と、lcms2 による ICC プロファイルの
//! 扱い・ディスプレイ変換用の 3D LUT の作成を担当する。
//!
//! 各境界の色の定義は docs/04_architecture.md の 2.6 節（色の契約）を正とする。
//! ディスプレイの色管理は 5 章を参照。
//!
//! | モジュール | 内容 | 設計書 |
//! |---|---|---|
//! | [`matrix`] | 3×3 の行列（f64 で計算し、f32 で取り出す） | 2.3 節 |
//! | [`space`] | 原色・白色点・RGB ⇔ XYZ・作業色空間と出力の色空間の変換・Bradford の色順応 | 2.6 節 |
//! | [`transfer`] | 伝達関数（IEC 61966-2-1、Adobe RGB のガンマ） | 2.6 節 |
//! | [`lab`] | XYZ ⇔ CIELAB、ΔE76・ΔE2000 | IQ-04・IQ-07b、PoC-4 |
//! | [`oklab`] | OKLab・OKLCh | 2.1 節のステージ 13、PoC-4 |
//! | [`camera`] | カメラ RGB → 作業色空間の行列 | 2.6 節（ステージ 8） |
//! | [`white_balance`] | 色温度・tint ⇔ WB の係数 | DEV-03、ステージ 3 |
//! | [`gamut`] | 色域の圧縮（仮実装） | 2.6 節「3 つの圧縮・変換」、PoC-4 |
//! | [`icc`] | ICC プロファイルの作成・読み込み、lcms2 の浮動小数点の変換 | 2.6 節、IQ-06 |
//! | [`lut`] | 3D LUT（lcms2 で作成、四面体補間で適用） | 2.6 節 17a・2.3 節・5 章 |
//! | [`display`] | モニターのプロファイル（取得できなければ sRGB とみなす）と画面への出力の準備 | 5 章、IQ-05 |
//!
//! # 例
//!
//! ```
//! use genzo_color::space::{XYZ_TO_SRGB, working_to};
//! use genzo_color::transfer::srgb_encode_f32;
//! use genzo_color::{
//!     DEFAULT_LUT_SIZE, DisplayLutSource, DisplayProfile, GamutCompressor, IccProfile,
//!     RgbColorSpace, StandardProfile, camera_to_working_matrix,
//! };
//!
//! // ステージ 8: カメラ RGB（WB 適用済み）→ 作業色空間（ここでは仮に sRGB のカメラ）。
//! let cam_to_working = camera_to_working_matrix(&XYZ_TO_SRGB)?.to_f32();
//! let working = cam_to_working.apply([0.5, 0.4, 0.3]);
//!
//! // ステージ 17b: 書き出し（sRGB）。色域の圧縮 → 符号化。ICC を埋め込む（IQ-06）。
//! let linear = working_to(RgbColorSpace::Srgb).to_f32().apply(working);
//! let encoded = GamutCompressor::for_space(RgbColorSpace::Srgb)
//!     .compress(linear)
//!     .map(srgb_encode_f32);
//! assert!(encoded.iter().all(|v| (0.0..=1.0).contains(v)));
//! let icc = IccProfile::standard(StandardProfile::Srgb)?;
//! assert!(!icc.as_bytes().is_empty());
//!
//! // ステージ 17a: 画面。モニターのプロファイルが取得できなければ sRGB とみなす（IQ-05）。
//! let display = DisplayProfile::resolve(None)?;
//! assert!(display.is_assumed_srgb());
//! let lut = display.build_lut(DisplayLutSource::Working, DEFAULT_LUT_SIZE)?;
//! let gamut = display.gamut().expect("sRGB は行列で表せる");
//! let shown = lut.apply(gamut.compress_working(working).map(srgb_encode_f32));
//! assert!(shown.iter().all(|v| v.is_finite()));
//! # Ok::<(), genzo_color::ColorError>(())
//! ```

pub mod camera;
pub mod display;
pub mod error;
pub mod gamut;
pub mod icc;
pub mod lab;
pub mod lut;
pub mod matrix;
pub mod oklab;
pub mod space;
pub mod transfer;
pub mod white_balance;

#[cfg(test)]
mod test_util;

pub use camera::camera_to_working_matrix;
pub use display::{
    DisplayGamut, DisplayLutSource, DisplayProfile, DisplayProfileFallbackReason,
    DisplayProfileSource,
};
pub use error::{ColorError, Result};
pub use gamut::GamutCompressor;
pub use icc::{
    EXPORT_ICC_VERSION, IccProfile, IccTransform, IccVersion, RenderingIntent, StandardProfile,
};
pub use lab::{Lab, delta_e76, delta_e2000};
pub use lut::{DEFAULT_LUT_SIZE, Lut3d};
pub use matrix::{Mat3, Mat3F32};
pub use space::{D50, D65, RgbColorSpace, RgbPrimaries, Uv, Xy};
pub use transfer::TransferFunction;
pub use white_balance::TemperatureTint;
