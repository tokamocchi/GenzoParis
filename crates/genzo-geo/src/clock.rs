//! 時計（テストで差し替える）。

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

/// 時計。地名の検索の間隔（単調に進む [`Instant`]）と、タイルのキャッシュの期限（UTC の日時）に使う。
pub trait Clock: Send + Sync {
    /// 単調に進む時刻（間隔を測る）。
    fn now(&self) -> Instant;

    /// 今の日時（UTC）。
    fn now_utc(&self) -> DateTime<Utc>;

    /// `duration` だけ待つ。
    fn sleep(&self, duration: Duration);
}

/// OS の時計。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn now_utc(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
