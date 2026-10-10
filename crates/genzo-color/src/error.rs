//! genzo-color のエラー型。

use thiserror::Error;

/// genzo-color の処理で起きるエラー。
#[derive(Debug, Clone, PartialEq, Error)]
#[non_exhaustive]
pub enum ColorError {
    /// 行列が特異（または特異に近い）ため、逆行列を計算できない。
    #[error("行列が特異のため逆行列を計算できない（行列式 {determinant:e}）")]
    SingularMatrix {
        /// 計算した行列式。
        determinant: f64,
    },

    /// 入力に NaN・無限大が含まれる。
    #[error("{what} に有限でない値（NaN・無限大）が含まれる")]
    NonFinite {
        /// どの入力か。
        what: &'static str,
    },

    /// 色度座標 (x, y) が不正（y ≤ 0、または x・y が有限でない）。
    #[error("色度座標 ({x}, {y}) が不正（y > 0 の有限の値が必要）")]
    InvalidChromaticity {
        /// x。
        x: f64,
        /// y。
        y: f64,
    },

    /// カメラ行列が 2.6 節の手順に使えない。
    #[error("カメラ行列が不正: {reason}")]
    InvalidCameraMatrix {
        /// 理由。
        reason: &'static str,
    },

    /// 色温度が範囲外。
    #[error("色温度 {temperature_k} K は範囲外（{min}〜{max} K）")]
    TemperatureOutOfRange {
        /// 指定された色温度（K）。
        temperature_k: f64,
        /// 下限（K）。
        min: f64,
        /// 上限（K）。
        max: f64,
    },

    /// 色かぶり補正（tint）が範囲外。
    #[error("色かぶり補正 {tint} は範囲外（{min}〜{max}）")]
    TintOutOfRange {
        /// 指定された値。
        tint: f64,
        /// 下限。
        min: f64,
        /// 上限。
        max: f64,
    },

    /// ホワイトバランスの係数、または白の色が不正（0 以下・有限でない・色度にできない）。
    #[error("ホワイトバランスの計算ができない: {reason}")]
    InvalidWhiteBalance {
        /// 理由。
        reason: &'static str,
    },

    /// 係数から推定した色温度・tint が範囲外。値は推定値（色温度は探索範囲の端で打ち切ることがある）。
    #[error(
        "ホワイトバランスの係数から推定した値が範囲外（色温度 {temperature_k} K、色かぶり補正 {tint}）"
    )]
    WhiteBalanceEstimateOutOfRange {
        /// 推定した色温度（K）。
        temperature_k: f64,
        /// 推定した色かぶり補正。
        tint: f64,
    },

    /// ICC プロファイルとして読み込めない（形式の不正・大きすぎるなど）。
    #[error("ICC プロファイルを読み込めない: {reason}")]
    IccInvalid {
        /// 理由。
        reason: String,
    },

    /// ICC プロファイルは読めたが、この用途には使えない（RGB でないなど）。
    #[error("ICC プロファイルがこの用途に対応していない: {reason}")]
    IccUnsupported {
        /// 理由。
        reason: String,
    },

    /// lcms2 でのプロファイルの作成・変換の作成に失敗した。
    #[error("lcms2 の処理に失敗した: {reason}")]
    Lcms {
        /// 理由。
        reason: String,
    },

    /// 3D LUT の格子数が範囲外。
    #[error("3D LUT の格子数 {size} は範囲外（{min}〜{max}）")]
    InvalidLutSize {
        /// 指定された格子数。
        size: usize,
        /// 下限。
        min: usize,
        /// 上限。
        max: usize,
    },

    /// 入力と出力のバッファの長さが一致しない。
    #[error("バッファの長さが一致しない（入力 {input}、出力 {output}）")]
    LengthMismatch {
        /// 入力の要素数。
        input: usize,
        /// 出力の要素数。
        output: usize,
    },

    /// 引数が不正（範囲外のパラメーターなど）。
    #[error("引数が不正: {reason}")]
    InvalidArgument {
        /// 理由。
        reason: &'static str,
    },
}

/// genzo-color の `Result`。
pub type Result<T> = std::result::Result<T, ColorError>;
