//! 取り消しと進捗（docs/04_architecture.md の 6.1 節「すべてのジョブに取り消しトークンを持たせる。
//! 処理の区切り（タイルごと、ステージごと）で確認し、取り消されたら途中でやめる」）。
//!
//! この crate は genzo-jobs に依存しない（1.4 節の依存の向き）。取り消しは小さなトレイト
//! [`RenderControl`] で受け取る。`Fn() -> bool` はそのまま使える（genzo-jobs の
//! `CancellationToken` なら `&|| token.is_cancelled()` を渡す）。

use crate::error::{PipelineError, Result};

/// 長い処理（等倍のタイル処理・書き出し）の取り消しの確認と進捗の通知。
///
/// エンジンは **タイルを処理する前ごとに** [`is_cancelled`](Self::is_cancelled) を呼び、`true` なら
/// [`PipelineError::Cancelled`] を返す（途中の結果は返さない）。タイルを 1 つ処理し終えるごとに
/// [`on_progress`](Self::on_progress) を呼ぶ。
pub trait RenderControl: Sync {
    /// 取り消されたか。
    fn is_cancelled(&self) -> bool;

    /// 進捗（`done` / `total` のタイル）。既定は何もしない。
    fn on_progress(&self, _done: u64, _total: u64) {}
}

impl<F: Fn() -> bool + Sync> RenderControl for F {
    fn is_cancelled(&self) -> bool {
        self()
    }
}

/// 取り消さない（テスト・同期の呼び出し用）。
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverCancel;

impl RenderControl for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// 取り消されていれば [`PipelineError::Cancelled`]。
pub(crate) fn check_cancelled(control: &dyn RenderControl) -> Result<()> {
    if control.is_cancelled() {
        Err(PipelineError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn closures_and_never_cancel() {
        assert!(check_cancelled(&NeverCancel).is_ok());
        let flag = AtomicBool::new(false);
        let c = || flag.load(Ordering::SeqCst);
        assert!(check_cancelled(&c).is_ok());
        flag.store(true, Ordering::SeqCst);
        assert!(matches!(check_cancelled(&c), Err(PipelineError::Cancelled)));
        // 既定の進捗の通知は何もしない。
        c.on_progress(1, 2);
    }
}
