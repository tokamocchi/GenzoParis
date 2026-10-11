//! genzo-pipeline のエラー型。

use crate::contract::ColorContract;
use crate::image::Roi;

/// パイプラインの処理で起きるエラー。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PipelineError {
    /// このアプリが知らない処理バージョン（04 の 2.5 節・7.1 節。DATA-09）。
    #[error("処理バージョン {found} には対応していません（対応しているのは 1〜{supported}）")]
    UnsupportedProcessVersion {
        /// 指定された処理バージョン。
        found: u32,
        /// 対応している最新の処理バージョン。
        supported: u32,
    },

    /// 画像の寸法が不正（0、または上限を超える）。
    #[error("画像の寸法が不正です（{width} × {height}）")]
    InvalidDimensions {
        /// 幅。
        width: u32,
        /// 高さ。
        height: u32,
    },

    /// データの長さが寸法と合わない。
    #[error("画像のデータの長さ {actual} が寸法から求めた {expected} と一致しません")]
    DataLengthMismatch {
        /// 寸法から求めた長さ。
        expected: usize,
        /// 実際の長さ。
        actual: usize,
    },

    /// 範囲（Roi）が不正、または必要な範囲を入力が覆っていない。
    #[error("範囲が不正です: {reason}（範囲 {roi:?}）")]
    InvalidRoi {
        /// 問題の範囲。
        roi: Roi,
        /// 理由。
        reason: &'static str,
    },

    /// ステージの入力の色の契約（2.6 節）が期待と違う。
    #[error("ステージ {stage} の入力は {expected} が必要です（実際は {found}）")]
    ContractMismatch {
        /// ステージの ID。
        stage: &'static str,
        /// 期待した契約。
        expected: ColorContract,
        /// 実際の契約。
        found: ColorContract,
    },

    /// センサーのステージの入力の種類（RAW・CFA・RGB）が期待と違う。
    #[error("ステージ {stage} の入力は {expected} が必要です（実際は {found}）")]
    SensorInputMismatch {
        /// ステージの ID。
        stage: &'static str,
        /// 期待した種類。
        expected: &'static str,
        /// 実際の種類。
        found: &'static str,
    },

    /// ステージのパラメータが、そのステージの型と合わない（[`crate::stage::StageParams`]）。
    #[error("ステージ {stage} のパラメータの型が違います（{found}）")]
    ParamsTypeMismatch {
        /// ステージの ID。
        stage: &'static str,
        /// 渡されたパラメータの型の名前。
        found: &'static str,
    },

    /// センサー処理の設定（[`crate::sensor::SensorPlan`]）と RAW が対応しない。
    #[error("センサー処理の設定と RAW が対応しません: {0}")]
    PlanMismatch(&'static str),

    /// ガイド（段階 B。2.7 節）が必要なのに文脈にない、または文脈と合わない（回転・寸法・処理
    /// バージョンが違う、作り直していない古いガイドなど）。
    #[error("ステージ {stage} のガイドが使えません: {reason}")]
    GuideMismatch {
        /// ステージの ID。
        stage: &'static str,
        /// 理由。
        reason: &'static str,
    },

    /// 同じ ID のステージを 2 回登録しようとした（7.1 節）。
    #[error("ステージの ID {0} は登録済みです")]
    DuplicateStageId(&'static str),

    /// 引数が不正。
    #[error("引数が不正です: {0}")]
    InvalidArgument(String),

    /// 取り消された（04 の 6.1 節「処理の区切り（タイルごと、ステージごと）で確認し、取り消されたら
    /// 途中でやめる」。[`crate::engine::RenderControl`]）。途中の結果は返さない。
    #[error("処理が取り消されました")]
    Cancelled,

    /// RAW の検証・扱いのエラー（genzo-raw）。
    #[error(transparent)]
    Raw(#[from] genzo_raw::RawError),

    /// 色の計算のエラー（genzo-color。カメラ行列・WB の係数など）。
    #[error(transparent)]
    Color(#[from] genzo_color::ColorError),
}

/// genzo-pipeline の `Result`。
pub type Result<T, E = PipelineError> = std::result::Result<T, E>;
