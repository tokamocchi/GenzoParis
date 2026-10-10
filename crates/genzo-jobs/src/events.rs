//! ジョブのイベント（進捗と状態の変化）。
//!
//! P3 のジョブの進捗表示とキャンセル（04 の 6.1 節、01 の SYS-02）のために、UI 側（コア API の
//! イベント。1.5 節「ジョブの進捗」）へ伝える。[`crate::Scheduler::subscribe`] で受け取る。

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;

use crate::job::{JobId, JobState};
use crate::priority::Priority;

/// ジョブのイベント。
#[derive(Debug, Clone, PartialEq)]
pub struct JobEvent {
    /// ジョブの ID。
    pub id: JobId,
    /// イベントの時点のジョブの優先度。
    pub priority: Priority,
    /// ジョブの表示名（[`crate::JobOptions::label`]）。
    pub label: Option<Arc<str>>,
    /// イベントの内容。
    pub kind: JobEventKind,
}

/// ジョブのイベントの内容。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JobEventKind {
    /// 状態が変わった（キューに入った・実行を始めた・終わった）。
    State(JobState),
    /// 進捗（0〜1）が変わった。
    ///
    /// 値は [`PROGRESS_EVENT_STEPS`] 段階に丸めて、丸めた値が変わったときだけ送る。
    Progress(f32),
}

/// 進捗のイベントを送る細かさ（0〜1 を何段階に分けるか）。
///
/// **仮置き**: 1000 段階（0.1%）。表示（進捗バー）には十分細かく、タイルや行ごとに報告しても
/// イベントの数が 1 ジョブあたり 1001 件以下に収まる。UI の更新頻度の制限はコア API 側で行う。
pub const PROGRESS_EVENT_STEPS: u32 = 1000;

/// イベントの購読者の一覧。
#[derive(Default)]
pub(crate) struct EventHub {
    subscribers: Mutex<Vec<Sender<JobEvent>>>,
}

impl EventHub {
    /// 新しい購読者を加え、受け取り側を返す。
    pub(crate) fn subscribe(&self) -> Receiver<JobEvent> {
        let (tx, rx) = crossbeam_channel::unbounded();
        self.subscribers.lock().push(tx);
        rx
    }

    /// すべての購読者に送る。受け取り側が drop された購読者は外す。
    ///
    /// 送り先のチャネルは上限がないので、送る処理で待つことはない。
    pub(crate) fn emit(&self, event: JobEvent) {
        let mut subscribers = self.subscribers.lock();
        if subscribers.is_empty() {
            return;
        }
        subscribers.retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// 購読者の数（テスト用）。
    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        self.subscribers.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: u64) -> JobEvent {
        JobEvent {
            id: JobId::from_raw(id),
            priority: Priority::P3,
            label: Some(Arc::from("書き出し")),
            kind: JobEventKind::Progress(0.5),
        }
    }

    #[test]
    fn events_reach_all_subscribers_and_dropped_ones_are_removed() {
        let hub = EventHub::default();
        hub.emit(event(1)); // 購読者がいなくても問題ない。
        let a = hub.subscribe();
        let b = hub.subscribe();
        hub.emit(event(2));
        assert_eq!(a.try_recv().unwrap(), event(2));
        assert_eq!(b.try_recv().unwrap(), event(2));
        drop(b);
        hub.emit(event(3));
        assert_eq!(hub.subscriber_count(), 1);
        assert_eq!(a.try_recv().unwrap(), event(3));
        assert!(a.try_recv().is_err());
    }
}
