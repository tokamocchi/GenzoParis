//! GPU 版のエラー（docs/04_architecture.md の 6.3 節「GPU のエラー（デバイスの消失など）: GPU を
//! 初期化し直す。失敗が続く場合は CPU 版に切り替え、その旨を表示する」）。
//!
//! この crate は GPU の失敗をすべて [`GpuError`] で返し、パニックしない（wgpu の既定の「捕まえて
//! いないエラーでパニックする」動作は [`crate::GpuContext`] が置き換える）。初期化し直すか、CPU 版に
//! 切り替えるかは呼び出し側が決める（[`GpuError::is_device_lost`]・[`GpuError::should_fall_back_to_cpu`]）。
//! 書き出しの [`crate::export_with_fallback`] は、GPU のエラーなら CPU 版で処理し直す（2.4 節）。

use std::time::Duration;

use genzo_pipeline::PipelineError;

/// GPU 版の結果。
pub type Result<T> = std::result::Result<T, GpuError>;

/// GPU 版のエラー。
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    /// デバイスを作れない（アダプターはあるが、要求した機能・上限を満たさない、ドライバの失敗など）。
    #[error("GPU のデバイスを作れません: {0}")]
    RequestDevice(String),
    /// デバイスが失われた（ドライバのリセット、`destroy` など）。初期化し直すまで使えない。
    #[error("GPU のデバイスが失われました: {0}")]
    DeviceLost(String),
    /// wgpu の検証エラー（シェーダーのコンパイル、資源の作成、コマンドの記録）。この crate の不具合か、
    /// 上限を超える入力。
    #[error("GPU の検証エラー: {0}")]
    Validation(String),
    /// GPU のメモリが足りない。
    #[error("GPU のメモリが足りません: {0}")]
    OutOfMemory(String),
    /// wgpu の内部エラー。
    #[error("GPU の内部エラー: {0}")]
    Internal(String),
    /// 投入した処理が決まった時間内に終わらなかった（ハングの検出）。
    #[error("GPU の処理が {0:?} 以内に終わりませんでした")]
    Timeout(Duration),
    /// 結果のバッファを CPU から読めなかった。
    #[error("GPU のバッファの読み出しに失敗しました: {0}")]
    Map(String),
    /// バッファの大きさがデバイスの上限を超える（CPU 版で処理するか、タイルを小さくする）。
    #[error("{what} が GPU の上限を超えます（{bytes} バイト、上限 {limit} バイト）")]
    TooLarge {
        /// 何のバッファか。
        what: String,
        /// 必要な大きさ。
        bytes: u64,
        /// デバイスの上限。
        limit: u64,
    },
    /// このステージ（または、このパラメータ）は GPU 版で処理できない（CPU 版で処理する）。
    #[error("ステージ {stage} は GPU 版で処理できません: {reason}")]
    Unsupported {
        /// ステージの ID。
        stage: &'static str,
        /// 理由。
        reason: String,
    },
    /// パイプライン（CPU 版と共通の部分）のエラー（入力の検証、取り消しなど）。
    #[error(transparent)]
    Pipeline(#[from] PipelineError),
}

impl GpuError {
    /// デバイスが失われたか（6.3 節: 初期化し直す）。
    pub fn is_device_lost(&self) -> bool {
        matches!(self, GpuError::DeviceLost(_))
    }

    /// CPU 版で処理し直せば結果を得られる見込みのある失敗か（GPU の側の失敗）。
    ///
    /// パイプラインのエラー（入力・設定の誤り、取り消し）は CPU 版でも同じように失敗するので `false`。
    pub fn should_fall_back_to_cpu(&self) -> bool {
        !matches!(self, GpuError::Pipeline(_))
    }

    /// 取り消されたか。
    pub fn is_cancelled(&self) -> bool {
        matches!(self, GpuError::Pipeline(PipelineError::Cancelled))
    }
}

impl From<wgpu::Error> for GpuError {
    fn from(e: wgpu::Error) -> Self {
        match e {
            wgpu::Error::OutOfMemory { .. } => GpuError::OutOfMemory(e.to_string()),
            wgpu::Error::Validation { description, .. } => GpuError::Validation(description),
            wgpu::Error::Internal { description, .. } => GpuError::Internal(description),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert!(GpuError::DeviceLost("x".into()).is_device_lost());
        assert!(GpuError::DeviceLost("x".into()).should_fall_back_to_cpu());
        assert!(GpuError::Timeout(Duration::from_secs(1)).should_fall_back_to_cpu());
        assert!(
            GpuError::TooLarge {
                what: "画像".into(),
                bytes: 10,
                limit: 5
            }
            .should_fall_back_to_cpu()
        );
        let cancelled = GpuError::from(PipelineError::Cancelled);
        assert!(cancelled.is_cancelled());
        assert!(!cancelled.should_fall_back_to_cpu());
        assert!(!GpuError::Validation("v".into()).is_device_lost());
        // 表示は日本語。
        let msg = GpuError::Unsupported {
            stage: "output.display",
            reason: "行列".into(),
        }
        .to_string();
        assert!(msg.contains("output.display"), "{msg}");
    }
}
