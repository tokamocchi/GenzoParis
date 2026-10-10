//! ジョブの優先度（docs/04_architecture.md の 6.1 節）と、スレッド数の目安（SCL-07）。

use std::fmt;
use std::num::NonZeroUsize;

/// ジョブの優先度（04 の 6.1 節の表）。
///
/// 並び順は緊急度の高い順（`P0 < P1 < P2 < P3`）。**小さいほど先に実行する**。
/// 同じ優先度の中では投入順（FIFO）に実行する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Priority {
    /// P0：対話。現像中の写真のプレビュー更新、ルーペに表示中の写真。
    /// 最優先。古い要求は捨てる（[`crate::Scheduler::submit_latest`]。6.1 節・6.2 節）。
    P0,
    /// P1：表示中。グリッドの表示範囲のサムネイル。
    /// スクロールで表示範囲から外れたら取り消す。
    P1,
    /// P2：先読み。ルーペで前後の写真、グリッドの少し先。P0・P1 がないときに実行する。
    P2,
    /// P3：バックグラウンド。プレビューの一括生成、書き出し、整合性チェック。
    /// 空いているときに実行する。進捗を表示し、キャンセルできる（SYS-02）。
    P3,
}

impl Priority {
    /// すべての優先度（緊急度の高い順）。
    pub const ALL: [Priority; 4] = [Priority::P0, Priority::P1, Priority::P2, Priority::P3];

    /// 並べ替え用の順位（P0 = 0 〜 P3 = 3）。小さいほど先に実行する。
    pub const fn rank(self) -> u8 {
        match self {
            Priority::P0 => 0,
            Priority::P1 => 1,
            Priority::P2 => 2,
            Priority::P3 => 3,
        }
    }

    /// 順位から優先度を得る（[`Priority::rank`] の逆）。範囲外なら `None`。
    pub const fn from_rank(rank: u8) -> Option<Priority> {
        match rank {
            0 => Some(Priority::P0),
            1 => Some(Priority::P1),
            2 => Some(Priority::P2),
            3 => Some(Priority::P3),
            _ => None,
        }
    }

    /// 表示・ログ用の名前（`"P0"` など）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Priority::P0 => "P0",
            Priority::P1 => "P1",
            Priority::P2 => "P2",
            Priority::P3 => "P3",
        }
    }

    /// 対話的な処理（P0）か。
    pub const fn is_interactive(self) -> bool {
        matches!(self, Priority::P0)
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// UI 用に残しておく CPU のコア数（SCL-07「UI 用に一定の余力を残す」）。
///
/// **仮置き**: 1.3 節の UI スレッド・コマンド処理・GPU スレッド・DB 書き込みスレッドは
/// いずれも常時 CPU を使い切る処理ではないため、まとめて 1 コア分とした。
/// コア数の異なる PC での PERF-09・PERF-13 の計測（02 の SCL-07 の検証方法）で見直す。
pub const UI_RESERVED_CORES: usize = 1;

/// バックグラウンド処理用のスレッド数の目安（SCL-07）。
///
/// 論理コア数から [`UI_RESERVED_CORES`] を引いた数（1 本以上）。
/// 論理コア数を取得できない環境では 1 本にする。
pub fn recommended_worker_threads() -> usize {
    let cores = std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1);
    recommended_worker_threads_for(cores)
}

/// 論理コア数 `cores` のときのバックグラウンド処理用のスレッド数（SCL-07）。
///
/// `cores - UI_RESERVED_CORES` を 1 本以上に丸めた値。
pub const fn recommended_worker_threads_for(cores: usize) -> usize {
    let threads = cores.saturating_sub(UI_RESERVED_CORES);
    if threads == 0 { 1 } else { threads }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities_are_ordered_by_urgency() {
        assert!(Priority::P0 < Priority::P1);
        assert!(Priority::P1 < Priority::P2);
        assert!(Priority::P2 < Priority::P3);
        let mut shuffled = [Priority::P2, Priority::P0, Priority::P3, Priority::P1];
        shuffled.sort();
        assert_eq!(shuffled, Priority::ALL);
    }

    #[test]
    fn rank_round_trips() {
        for p in Priority::ALL {
            assert_eq!(Priority::from_rank(p.rank()), Some(p));
        }
        assert_eq!(Priority::from_rank(4), None);
        assert_eq!(Priority::from_rank(u8::MAX), None);
        assert_eq!(Priority::ALL.map(Priority::rank), [0, 1, 2, 3]);
    }

    #[test]
    fn display_and_flags() {
        assert_eq!(
            Priority::ALL.map(|p| p.to_string()),
            ["P0", "P1", "P2", "P3"]
        );
        assert!(Priority::P0.is_interactive());
        assert!(!Priority::P1.is_interactive());
        assert!(!Priority::P3.is_interactive());
    }

    #[test]
    fn recommended_threads_leave_room_for_ui() {
        // 境界: 0 コア（取得できない場合の想定外の値）と 1 コアでも 1 本は動かす。
        assert_eq!(recommended_worker_threads_for(0), 1);
        assert_eq!(recommended_worker_threads_for(1), 1);
        assert_eq!(recommended_worker_threads_for(2), 1);
        assert_eq!(recommended_worker_threads_for(4), 3);
        assert_eq!(recommended_worker_threads_for(16), 15);
        assert!(recommended_worker_threads() >= 1);
    }
}
