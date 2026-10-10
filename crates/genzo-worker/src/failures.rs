//! 同じファイルでの失敗の記録（docs/04_architecture.md の 1.2 節・6.3 節「同じファイルで 2 回続けて
//! 失敗した場合は、そのファイルを以後スキップします」）。
//!
//! - 数える失敗は、ワーカーの異常終了・タイムアウト・不正な応答・不正なバッファ
//!   （[`crate::WorkerClientError::counts_toward_skip`]）。ワーカーが通常のエラーとして返したもの
//!   （壊れたファイル・対応していない形式・ファイルがない）は数えない（ワーカーは無事で、同じ
//!   ファイルを再び処理しても害がないため。ドライブが一時的に外れている場合などに、ファイルを
//!   スキップし続けないため）。取り消しも数えない。
//! - 数はファイルと **ジョブの種類**（[`JobKind`]）の組ごとに数える。同じ種類のジョブが成功したら、
//!   その種類の数を 0 に戻す。別の種類のジョブの成功では戻さない（例: メタデータの読み取りや
//!   埋め込みサムネイルは成功するが、展開でハングする RAW を、合間の成功でスキップし損ねないため）。
//!   どれかの種類が上限に達したら、そのファイルを以後スキップする（すべての種類のジョブ）。
//!   通常のエラーは数を変えない。
//! - 記録はメモリの中だけ（アプリを起動し直すと消える）。ファイルを置き換えた後などに、
//!   [`FailureTracker::reset`] で手動で戻せる。
//! - ファイルはパスの文字列で区別する（正規化しない）。呼び出し側はカタログのパスなど、同じ
//!   表記を使う。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::protocol::JobKind;

/// 何回続けて失敗したらスキップするか（04 の 1.2 節・6.3 節の「2 回」）。
pub const MAX_CONSECUTIVE_FAILURES: u32 = 2;

/// ファイルごと・ジョブの種類ごとの、続けて失敗した回数。
type Counts = HashMap<PathBuf, HashMap<JobKind, u32>>;

/// ファイルごとの、続けて失敗した回数の記録。複数のワーカーで共有できる。
#[derive(Debug)]
pub struct FailureTracker {
    threshold: u32,
    counts: Mutex<Counts>,
}

impl Default for FailureTracker {
    fn default() -> Self {
        Self::new(MAX_CONSECUTIVE_FAILURES)
    }
}

impl FailureTracker {
    /// `threshold` 回続けて失敗したらスキップする記録を作る（0 は 1 とみなす）。
    pub fn new(threshold: u32) -> Self {
        Self {
            threshold: threshold.max(1),
            counts: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Counts> {
        // 記録は単純な数なので、他のスレッドがパニックしても使い続けてよい。
        self.counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// スキップする回数。
    pub fn threshold(&self) -> u32 {
        self.threshold
    }

    /// 続けて失敗した回数（ジョブの種類ごとの数のうち最大のもの）。
    pub fn consecutive_failures(&self, path: &Path) -> u32 {
        self.lock()
            .get(path)
            .and_then(|kinds| kinds.values().copied().max())
            .unwrap_or(0)
    }

    /// スキップするか（どれかの種類で、続けて失敗した回数が上限に達しているか）。
    pub fn is_skipped(&self, path: &Path) -> bool {
        self.consecutive_failures(path) >= self.threshold
    }

    /// 種類 `kind` のジョブの失敗を記録し、その種類で続けて失敗した回数を返す。
    pub fn record_failure(&self, path: &Path, kind: JobKind) -> u32 {
        let mut counts = self.lock();
        let n = counts
            .entry(path.to_path_buf())
            .or_default()
            .entry(kind)
            .or_insert(0);
        *n = n.saturating_add(1);
        *n
    }

    /// 種類 `kind` のジョブの成功を記録する（その種類の数だけを 0 に戻す）。
    pub fn record_success(&self, path: &Path, kind: JobKind) {
        let mut counts = self.lock();
        if let Some(kinds) = counts.get_mut(path) {
            kinds.remove(&kind);
            if kinds.is_empty() {
                counts.remove(path);
            }
        }
    }

    /// 記録を消す（スキップをやめる）。
    pub fn reset(&self, path: &Path) {
        self.lock().remove(path);
    }

    /// すべての記録を消す。
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// スキップしているファイルの一覧（順不同）。
    pub fn skipped_paths(&self) -> Vec<PathBuf> {
        self.lock()
            .iter()
            .filter(|(_, kinds)| kinds.values().any(|n| *n >= self.threshold))
            .map(|(p, _)| p.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DECODE: JobKind = JobKind::DecodeRaw;
    const PROBE: JobKind = JobKind::ProbePhoto;

    #[test]
    fn two_consecutive_failures_skip_the_file() {
        let t = FailureTracker::default();
        let a = Path::new("a.arw");
        let b = Path::new("b.arw");
        assert_eq!(t.threshold(), 2);
        assert!(!t.is_skipped(a));
        assert_eq!(t.record_failure(a, DECODE), 1);
        assert!(!t.is_skipped(a));
        assert_eq!(t.record_failure(b, DECODE), 1);
        assert_eq!(t.record_failure(a, DECODE), 2);
        assert!(t.is_skipped(a));
        assert!(!t.is_skipped(b));
        assert_eq!(t.skipped_paths(), vec![a.to_path_buf()]);
        t.reset(a);
        assert!(!t.is_skipped(a));
        assert_eq!(t.consecutive_failures(a), 0);
    }

    #[test]
    fn success_resets_only_the_same_kind() {
        let t = FailureTracker::default();
        let a = Path::new("a.arw");
        t.record_failure(a, DECODE);
        t.record_success(a, DECODE);
        assert_eq!(t.record_failure(a, DECODE), 1, "続けての失敗ではない");
        assert!(!t.is_skipped(a));
        // 別の種類の成功では戻さない。
        t.record_success(a, PROBE);
        assert_eq!(t.consecutive_failures(a), 1);
        assert_eq!(t.record_failure(a, DECODE), 2);
        assert!(t.is_skipped(a));
        // 種類ごとに数える（別々の種類で 1 回ずつなら、続けての失敗ではない）。
        let b = Path::new("b.arw");
        t.record_failure(b, DECODE);
        t.record_failure(b, PROBE);
        assert_eq!(t.consecutive_failures(b), 1);
        assert!(!t.is_skipped(b));
        t.record_success(b, DECODE);
        t.record_success(b, PROBE);
        assert_eq!(t.consecutive_failures(b), 0);
        t.record_failure(Path::new("c"), PROBE);
        t.clear();
        assert_eq!(t.consecutive_failures(Path::new("c")), 0);
        assert!(t.skipped_paths().is_empty());
    }

    #[test]
    fn zero_threshold_means_one() {
        let t = FailureTracker::new(0);
        assert_eq!(t.threshold(), 1);
        t.record_failure(Path::new("x"), DECODE);
        assert!(t.is_skipped(Path::new("x")));
    }
}
