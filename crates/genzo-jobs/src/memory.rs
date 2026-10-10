//! メモリの予算（docs/04_architecture.md の 6.1 節、02 の SCL-05）。
//!
//! 「大きなバッファ（CFA、タイル、プレビュー）を確保する前に予算から枠を取り、枠が空くまで
//! 待ちます。並列度は、CPU のコア数とこの予算の両方で制限します」（6.1 節）を実現する。
//!
//! # 枠の割り当ての規則
//!
//! - [`MemoryBudget::acquire`]（待つ取得）は、空きが足りればすぐに取る。足りなければ待ち、
//!   枠が返却されたときに **優先度の高い順・同じ優先度は先着順に、入るものから** 割り当てる
//!   （first-fit）。大きな要求が入らないときに、後ろの小さな要求を止めない。実行中のジョブが
//!   枠を持ったまま追加の枠を待つ場合に、厳密な先着順にすると互いに待ち合って止まるため。
//! - [`MemoryBudget::try_acquire`]（待たない取得）は、空きが足りても、**同じか高い優先度の
//!   待ちがあれば失敗する**。スケジューラが新しいジョブを始めるときに使い、実行中の処理の
//!   続き（待っている取得）を新しい処理が追い越さないようにする。
//! - 予算の総量を超える要求は、待っても満たせないので即座にエラーにする。
//!
//! 枠を持ったまま別の枠を待つと、他のジョブと互いに待ち合う可能性がある。ジョブが使う
//! メモリは、なるべくジョブの投入時にまとめて宣言する（[`crate::JobOptions::memory_bytes`]）。

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::{Condvar, Mutex};

use crate::cancel::CancellationToken;
use crate::priority::Priority;

/// SCL-05 の通常操作時のメモリ使用量の上限（4GB）。
///
/// 02 の付録 A-1 の試算（α7 IV の RAW 16bit × 1ch が約 65MB = 32,741,376 画素 × 2 バイト）と
/// 同じく、1GB を 10^9 バイトとして扱う（2^30 として扱うより小さく、安全側）。
pub const SCL05_NORMAL_LIMIT_BYTES: u64 = 4_000_000_000;

/// SCL-05 の一括処理時のメモリ使用量の上限（8GB）。
pub const SCL05_BATCH_LIMIT_BYTES: u64 = 8_000_000_000;

/// 予算の対象外のメモリ使用量の見込み（大きなバッファ以外の、本体とワーカーの使用量）。
///
/// **仮置き**: UI（WebView を含む）、SQLite のキャッシュ、プログラム本体、ワーカーの常駐分を
/// まとめて 1GB と見込んだ。根拠となる実測値はまだない。PoC-1・PoC-2 でメモリ使用量を
/// 計測して見直す（SCL-05 の検証方法「長時間の連続操作でメモリ使用量を監視する」）。
pub const NON_BUFFER_OVERHEAD_BYTES: u64 = 1_000_000_000;

/// 通常操作時の予算の既定値（[`SCL05_NORMAL_LIMIT_BYTES`] − [`NON_BUFFER_OVERHEAD_BYTES`]）。
///
/// **仮置き**（[`NON_BUFFER_OVERHEAD_BYTES`] が仮置きのため）。
pub const DEFAULT_NORMAL_BUDGET_BYTES: u64 = SCL05_NORMAL_LIMIT_BYTES - NON_BUFFER_OVERHEAD_BYTES;

/// 一括処理時の予算の既定値（[`SCL05_BATCH_LIMIT_BYTES`] − [`NON_BUFFER_OVERHEAD_BYTES`]）。
///
/// **仮置き**（[`NON_BUFFER_OVERHEAD_BYTES`] が仮置きのため）。
pub const DEFAULT_BATCH_BUDGET_BYTES: u64 = SCL05_BATCH_LIMIT_BYTES - NON_BUFFER_OVERHEAD_BYTES;

/// メモリの予算のエラー。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MemoryError {
    /// 要求が予算の総量を超えている（待っても満たせない）。
    #[error("メモリの要求 {requested} バイトが予算の総量 {total} バイトを超えています")]
    ExceedsBudget {
        /// 要求したバイト数。
        requested: u64,
        /// 予算の総量。
        total: u64,
    },
    /// 待たない取得で、今は枠を取れなかった。
    #[error("メモリの枠を今は取れません（要求 {requested} バイト、空き {available} バイト）")]
    Unavailable {
        /// 要求したバイト数。
        requested: u64,
        /// 判定したときの空き。
        available: u64,
    },
    /// 待っている間に取り消された。
    #[error("メモリの枠の待ちが取り消されました")]
    Cancelled,
    /// 期限までに枠を取れなかった。
    #[error("メモリの枠を期限までに取れませんでした（要求 {requested} バイト）")]
    TimedOut {
        /// 要求したバイト数。
        requested: u64,
    },
}

impl From<crate::cancel::Cancelled> for MemoryError {
    fn from(_: crate::cancel::Cancelled) -> Self {
        MemoryError::Cancelled
    }
}

/// 枠が返却されたときなどに呼ぶ通知（スケジューラが待ちを再確認するため）。
type Listener = Arc<dyn Fn() + Send + Sync>;

/// メモリの予算（総量と使用中の量）。
///
/// 複製（`clone`）した値は同じ予算を共有する。本体の CPU のプールと GPU スレッドのように、
/// 複数のスケジューラで 1 つの予算を共有できる（[`crate::Scheduler::with_budget`]）。
#[derive(Clone)]
pub struct MemoryBudget {
    inner: Arc<BudgetInner>,
}

struct BudgetInner {
    /// 予算の総量（バイト）。
    total: u64,
    /// 使用中の量と待ち。
    state: Mutex<BudgetState>,
    /// 待っている取得を起こす。
    cv: Condvar,
    /// 枠の返却・待ちの解消を知らせる先（番号つき）。
    listeners: Mutex<Vec<(u64, Listener)>>,
    /// 次に割り当てる通知の番号。
    next_listener_id: AtomicU64,
}

struct BudgetState {
    /// 使用中の量（割り当て済みの待ちを含む）。常に `total` 以下。
    used: u64,
    /// 待っている取得。キーは（優先度の順位, 受付番号）で、小さいほど先に割り当てる。
    waiters: BTreeMap<(u8, u64), Waiter>,
    /// 次の受付番号。
    next_ticket: u64,
}

struct Waiter {
    /// 要求したバイト数。
    bytes: u64,
    /// 返却の時点で割り当て済みか（割り当て済みの分は `used` に含む）。
    granted: bool,
}

impl BudgetState {
    fn available(&self, total: u64) -> u64 {
        total - self.used
    }

    /// 優先度の高い順・先着順に、入る待ちへ割り当てる（first-fit）。割り当てがあれば `true`。
    fn grant_waiters(&mut self, total: u64) -> bool {
        let mut granted_any = false;
        let BudgetState { used, waiters, .. } = self;
        for waiter in waiters.values_mut().filter(|w| !w.granted) {
            if waiter.bytes <= total - *used {
                waiter.granted = true;
                *used += waiter.bytes;
                granted_any = true;
            }
        }
        granted_any
    }

    /// 順位 `rank` の新しい処理を、同じか高い優先度の未割り当ての待ちが止めているか。
    fn blocked_by_waiters(&self, rank: u8) -> bool {
        self.waiters
            .range(..(rank.saturating_add(1), 0))
            .any(|(_, w)| !w.granted)
    }
}

impl MemoryBudget {
    /// 総量 `total_bytes` の予算を作る。
    ///
    /// 総量 0 も作れる（0 バイトの要求だけが通る）。
    pub fn new(total_bytes: u64) -> Self {
        MemoryBudget {
            inner: Arc::new(BudgetInner {
                total: total_bytes,
                state: Mutex::new(BudgetState {
                    used: 0,
                    waiters: BTreeMap::new(),
                    next_ticket: 0,
                }),
                cv: Condvar::new(),
                listeners: Mutex::new(Vec::new()),
                next_listener_id: AtomicU64::new(0),
            }),
        }
    }

    /// 予算の総量（バイト）。
    pub fn total(&self) -> u64 {
        self.inner.total
    }

    /// 使用中の量（バイト）。待っている取得に割り当て済みの分を含む。
    pub fn used(&self) -> u64 {
        self.inner.state.lock().used
    }

    /// 空き（バイト）。
    pub fn available(&self) -> u64 {
        self.inner.state.lock().available(self.inner.total)
    }

    /// 枠が空くのを待っている取得の数。
    pub fn waiting(&self) -> usize {
        self.inner
            .state
            .lock()
            .waiters
            .values()
            .filter(|w| !w.granted)
            .count()
    }

    /// 2 つの値が同じ予算を共有しているか。
    pub fn same_as(&self, other: &MemoryBudget) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// `bytes` の枠を取る。空くまで待つ（優先度は P3 として扱う）。
    ///
    /// `token` が取り消されたら待つのをやめて [`MemoryError::Cancelled`] を返す。
    /// 総量を超える要求は待たずに [`MemoryError::ExceedsBudget`] を返す。
    pub fn acquire(
        &self,
        bytes: u64,
        token: &CancellationToken,
    ) -> Result<Reservation, MemoryError> {
        self.acquire_with_priority(bytes, Priority::P3, token)
    }

    /// 優先度を指定して `bytes` の枠を取る。空くまで待つ。
    ///
    /// 枠が返却されたとき、優先度の高い待ちから割り当てる（モジュールの説明を参照）。
    pub fn acquire_with_priority(
        &self,
        bytes: u64,
        priority: Priority,
        token: &CancellationToken,
    ) -> Result<Reservation, MemoryError> {
        self.acquire_inner(bytes, priority, token, None)
    }

    /// 期限つきで `bytes` の枠を取る。`deadline` までに取れなければ
    /// [`MemoryError::TimedOut`] を返す。
    pub fn acquire_with_deadline(
        &self,
        bytes: u64,
        priority: Priority,
        token: &CancellationToken,
        deadline: Instant,
    ) -> Result<Reservation, MemoryError> {
        self.acquire_inner(bytes, priority, token, Some(deadline))
    }

    /// 待たずに `bytes` の枠を取る（優先度は P3 として扱う）。
    ///
    /// 空きが足りない場合と、待っている取得がある場合は [`MemoryError::Unavailable`] を返す。
    pub fn try_acquire(&self, bytes: u64) -> Result<Reservation, MemoryError> {
        self.try_acquire_with_priority(bytes, Priority::P3)
    }

    /// 優先度を指定して、待たずに `bytes` の枠を取る。
    ///
    /// 空きが足りない場合と、同じか高い優先度の待ちがある場合は
    /// [`MemoryError::Unavailable`] を返す（待っている処理を追い越さない）。
    pub fn try_acquire_with_priority(
        &self,
        bytes: u64,
        priority: Priority,
    ) -> Result<Reservation, MemoryError> {
        let total = self.inner.total;
        if bytes > total {
            return Err(MemoryError::ExceedsBudget {
                requested: bytes,
                total,
            });
        }
        if bytes == 0 {
            return Ok(self.reservation(0));
        }
        let mut state = self.inner.state.lock();
        let available = state.available(total);
        if bytes <= available && !state.blocked_by_waiters(priority.rank()) {
            state.used += bytes;
            drop(state);
            Ok(self.reservation(bytes))
        } else {
            Err(MemoryError::Unavailable {
                requested: bytes,
                available,
            })
        }
    }

    fn acquire_inner(
        &self,
        bytes: u64,
        priority: Priority,
        token: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<Reservation, MemoryError> {
        let total = self.inner.total;
        if bytes > total {
            return Err(MemoryError::ExceedsBudget {
                requested: bytes,
                total,
            });
        }
        token.check()?;
        if bytes == 0 {
            return Ok(self.reservation(0));
        }
        // 速い経路: 空きが足りればすぐに取る（first-fit。待っている大きな要求に止められない）。
        {
            let mut state = self.inner.state.lock();
            if bytes <= state.available(total) {
                state.used += bytes;
                drop(state);
                return Ok(self.reservation(bytes));
            }
        }

        // 取り消しで待ちを起こす。コールバックは予算のロックを取ってから知らせる
        // （待つ側が取り消しを確認してから眠るまでの間に取り消されても、知らせを失わない）。
        // 既に取り消されていればコールバックはこの場で実行されるので、予算のロックを
        // 持たないうちに登録する。
        let weak = Arc::downgrade(&self.inner);
        let _registration = token.on_cancel(move || {
            if let Some(inner) = weak.upgrade() {
                let _state = inner.state.lock();
                inner.cv.notify_all();
            }
        });

        let mut state = self.inner.state.lock();
        if token.is_cancelled() {
            return Err(MemoryError::Cancelled);
        }
        if bytes <= state.available(total) {
            state.used += bytes;
            drop(state);
            return Ok(self.reservation(bytes));
        }
        let key = (priority.rank(), state.next_ticket);
        state.next_ticket += 1;
        state.waiters.insert(
            key,
            Waiter {
                bytes,
                granted: false,
            },
        );

        enum Outcome {
            Granted,
            Cancelled,
            TimedOut,
        }
        let outcome = loop {
            let granted = state.waiters.get(&key).is_some_and(|w| w.granted);
            if token.is_cancelled() {
                break Outcome::Cancelled;
            }
            if granted {
                break Outcome::Granted;
            }
            match deadline {
                Some(deadline) => {
                    if Instant::now() >= deadline {
                        break Outcome::TimedOut;
                    }
                    // 期限を過ぎて起きた場合も、ループの先頭で割り当て済みかを先に確認する。
                    let _ = self.inner.cv.wait_until(&mut state, deadline);
                }
                None => self.inner.cv.wait(&mut state),
            }
        };

        let waiter = state.waiters.remove(&key);
        match outcome {
            Outcome::Granted => {
                drop(state);
                Ok(self.reservation(bytes))
            }
            Outcome::Cancelled | Outcome::TimedOut => {
                // 割り当て済みだった場合は返却し、他の待ちへ回す。
                if waiter.is_some_and(|w| w.granted) {
                    state.used -= bytes;
                    state.grant_waiters(total);
                }
                drop(state);
                // 待ちが減ると、待たない取得（スケジューラ）が通るようになることがある。
                self.inner.cv.notify_all();
                self.notify_listeners();
                match outcome {
                    Outcome::Cancelled => Err(MemoryError::Cancelled),
                    _ => Err(MemoryError::TimedOut { requested: bytes }),
                }
            }
        }
    }

    fn reservation(&self, bytes: u64) -> Reservation {
        Reservation {
            budget: self.clone(),
            bytes,
        }
    }

    /// `bytes` を返却し、待ちに割り当てて知らせる。
    fn release(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        {
            let mut state = self.inner.state.lock();
            debug_assert!(state.used >= bytes, "返却が使用中の量を超えている");
            state.used = state.used.saturating_sub(bytes);
            state.grant_waiters(self.inner.total);
        }
        self.inner.cv.notify_all();
        self.notify_listeners();
    }

    /// 枠の返却と待ちの解消を知らせる先を登録する。番号を返す。
    ///
    /// 通知は予算のロックを外してから、返却したスレッドで呼ばれる。
    pub(crate) fn add_listener(&self, listener: Listener) -> u64 {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.inner.listeners.lock().push((id, listener));
        id
    }

    /// [`MemoryBudget::add_listener`] で登録した通知を解除する。
    pub(crate) fn remove_listener(&self, id: u64) {
        self.inner.listeners.lock().retain(|(lid, _)| *lid != id);
    }

    fn notify_listeners(&self) {
        let listeners: Vec<Listener> = self
            .inner
            .listeners
            .lock()
            .iter()
            .map(|(_, l)| Arc::clone(l))
            .collect();
        for listener in listeners {
            listener();
        }
    }
}

impl fmt::Debug for MemoryBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.inner.state.lock();
        f.debug_struct("MemoryBudget")
            .field("total", &self.inner.total)
            .field("used", &state.used)
            .field(
                "waiting",
                &state.waiters.values().filter(|w| !w.granted).count(),
            )
            .finish()
    }
}

/// 予算から取った枠。drop すると予算に返却する。
#[must_use = "drop するとすぐに枠が返却される"]
pub struct Reservation {
    budget: MemoryBudget,
    bytes: u64,
}

impl Reservation {
    /// 枠の大きさ（バイト）。
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// 枠を取った予算。
    pub fn budget(&self) -> &MemoryBudget {
        &self.budget
    }

    /// 枠を `bytes` まで減らし、差分を返却する。
    ///
    /// 今の大きさ以上を指定した場合は何もしない（枠を増やすことはできない。増やす場合は
    /// 別に取る）。
    pub fn shrink_to(&mut self, bytes: u64) {
        if bytes >= self.bytes {
            return;
        }
        let released = self.bytes - bytes;
        self.bytes = bytes;
        self.budget.release(released);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.release(self.bytes);
    }
}

impl fmt::Debug for Reservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reservation")
            .field("bytes", &self.bytes)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    /// 条件が成り立つまで待つ（時間で結果を決めない。失敗時に止まらないための上限だけ持つ）。
    fn wait_until(mut cond: impl FnMut() -> bool) {
        let limit = Instant::now() + Duration::from_secs(30);
        while !cond() {
            assert!(Instant::now() < limit, "条件が成り立たないまま上限を超えた");
            std::thread::yield_now();
        }
    }

    #[test]
    fn scl05_constants_match_requirement() {
        // SCL-05: 通常 4GB 以下、一括処理 8GB 以下（10^9 バイト換算）。
        assert_eq!(SCL05_NORMAL_LIMIT_BYTES, 4 * 1_000_000_000);
        assert_eq!(SCL05_BATCH_LIMIT_BYTES, 8 * 1_000_000_000);
        const { assert!(DEFAULT_NORMAL_BUDGET_BYTES < SCL05_NORMAL_LIMIT_BYTES) };
        const { assert!(DEFAULT_BATCH_BUDGET_BYTES < SCL05_BATCH_LIMIT_BYTES) };
        // 02 の付録 A-1: α7 IV（7008 × 4672）の RGBA float32 のバッファは約 524MB。
        let a7iv_f32 = 7008_u64 * 4672 * 16;
        assert_eq!(a7iv_f32, 523_862_016);
        // 通常時の既定の予算でも、フル解像度の作業バッファを少なくとも 1 枚は持てる。
        assert!(DEFAULT_NORMAL_BUDGET_BYTES / a7iv_f32 >= 1);
    }

    #[test]
    fn appendix_a1_uses_decimal_megabytes() {
        // 02 の付録 A-1 の表の値（画素数・MB）と照合し、MB が 10^6 バイトであること（したがって
        // SCL-05 の GB を 10^9 バイトとする解釈が付録と同じ単位であること）を確かめる。
        // 行: （幅, 高さ, 画素数, RAW 16bit × 1ch の MB, RGBA f16 の MB, RGBA f32 の MB）。
        // 画面表示用の行は RAW の列が「—」なので 0 とする。
        const ROWS: [(u64, u64, u64, u64, u64, u64); 3] = [
            (7008, 4672, 32_741_376, 65, 262, 524), // α7 IV
            (6000, 4000, 24_000_000, 48, 192, 384), // α7C
            (2560, 1440, 3_686_400, 0, 29, 59),     // 画面表示用
        ];
        const MB: u64 = 1_000_000;
        const MIB: u64 = 1 << 20;
        let round = |bytes: u64, unit: u64| (bytes + unit / 2) / unit;
        for (w, h, pixels, raw_mb, f16_mb, f32_mb) in ROWS {
            assert_eq!(w * h, pixels);
            if raw_mb != 0 {
                assert_eq!(round(pixels * 2, MB), raw_mb, "{w}x{h} RAW");
                // 2^20 バイトを 1MB とすると表の値と合わない（α7 IV で 62、α7C で 46）。
                assert_ne!(round(pixels * 2, MIB), raw_mb, "{w}x{h} RAW（2^20）");
            }
            assert_eq!(round(pixels * 8, MB), f16_mb, "{w}x{h} RGBA f16");
            assert_eq!(round(pixels * 16, MB), f32_mb, "{w}x{h} RGBA f32");
        }
        // 既定の予算（仮置き）の値そのもの。
        assert_eq!(DEFAULT_NORMAL_BUDGET_BYTES, 3_000_000_000);
        assert_eq!(DEFAULT_BATCH_BUDGET_BYTES, 7_000_000_000);
    }

    #[test]
    fn acquire_and_release() {
        let budget = MemoryBudget::new(100);
        let token = CancellationToken::new();
        let a = budget.acquire(30, &token).unwrap();
        let b = budget.try_acquire(70).unwrap();
        assert_eq!((a.bytes(), b.bytes()), (30, 70));
        assert_eq!(budget.used(), 100);
        assert_eq!(budget.available(), 0);
        assert!(a.budget().same_as(&budget));
        drop(a);
        assert_eq!(budget.used(), 70);
        drop(b);
        assert_eq!(budget.used(), 0);
        assert_eq!(budget.available(), 100);
    }

    #[test]
    fn exact_total_is_allowed_and_more_is_rejected() {
        let budget = MemoryBudget::new(100);
        let token = CancellationToken::new();
        let all = budget.acquire(100, &token).unwrap();
        drop(all);
        assert_eq!(
            budget.acquire(101, &token).unwrap_err(),
            MemoryError::ExceedsBudget {
                requested: 101,
                total: 100
            }
        );
        assert_eq!(
            budget.try_acquire(u64::MAX).unwrap_err(),
            MemoryError::ExceedsBudget {
                requested: u64::MAX,
                total: 100
            }
        );
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn zero_bytes_always_succeed() {
        let budget = MemoryBudget::new(0);
        let token = CancellationToken::new();
        let r = budget.acquire(0, &token).unwrap();
        assert_eq!(r.bytes(), 0);
        let r2 = budget.try_acquire(0).unwrap();
        drop((r, r2));
        assert_eq!(budget.used(), 0);
        assert!(matches!(
            budget.try_acquire(1),
            Err(MemoryError::ExceedsBudget { .. })
        ));
    }

    #[test]
    fn huge_total_does_not_overflow() {
        let budget = MemoryBudget::new(u64::MAX);
        let a = budget.try_acquire(u64::MAX - 1).unwrap();
        assert_eq!(
            budget.try_acquire(2).unwrap_err(),
            MemoryError::Unavailable {
                requested: 2,
                available: 1
            }
        );
        let b = budget.try_acquire(1).unwrap();
        assert_eq!(budget.available(), 0);
        drop((a, b));
    }

    #[test]
    fn try_acquire_reports_unavailable() {
        let budget = MemoryBudget::new(100);
        let _held = budget.try_acquire(80).unwrap();
        assert_eq!(
            budget.try_acquire(30).unwrap_err(),
            MemoryError::Unavailable {
                requested: 30,
                available: 20
            }
        );
        assert_eq!(budget.used(), 80);
    }

    #[test]
    fn cancelled_token_fails_before_waiting() {
        let budget = MemoryBudget::new(100);
        let token = CancellationToken::new();
        token.cancel();
        assert_eq!(
            budget.acquire(10, &token).unwrap_err(),
            MemoryError::Cancelled
        );
        // 総量を超える要求は、取り消しより先に判定する（待っても満たせないことを知らせる）。
        assert!(matches!(
            budget.acquire(1000, &token),
            Err(MemoryError::ExceedsBudget { .. })
        ));
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn acquire_waits_until_released() {
        let budget = MemoryBudget::new(100);
        let held = budget.try_acquire(60).unwrap();
        std::thread::scope(|s| {
            let waiter = s.spawn(|| budget.acquire(50, &CancellationToken::new()));
            wait_until(|| budget.waiting() == 1);
            // 待っている間は枠を取っていない。
            assert_eq!(budget.used(), 60);
            drop(held);
            let r = waiter.join().unwrap().unwrap();
            assert_eq!(r.bytes(), 50);
            assert_eq!(budget.used(), 50);
        });
        assert_eq!(budget.used(), 0);
        assert_eq!(budget.waiting(), 0);
    }

    #[test]
    fn cancel_while_waiting_returns_error() {
        let budget = MemoryBudget::new(100);
        let _held = budget.try_acquire(100).unwrap();
        let token = CancellationToken::new();
        std::thread::scope(|s| {
            let waiter = s.spawn(|| budget.acquire(10, &token));
            wait_until(|| budget.waiting() == 1);
            token.cancel();
            assert_eq!(waiter.join().unwrap().unwrap_err(), MemoryError::Cancelled);
        });
        assert_eq!(budget.waiting(), 0);
        assert_eq!(budget.used(), 100);
    }

    #[test]
    fn parent_cancel_stops_waiting_child() {
        let budget = MemoryBudget::new(10);
        let _held = budget.try_acquire(10).unwrap();
        let parent = CancellationToken::new();
        let child = parent.child();
        std::thread::scope(|s| {
            let waiter = s.spawn(|| budget.acquire(5, &child));
            wait_until(|| budget.waiting() == 1);
            parent.cancel();
            assert_eq!(waiter.join().unwrap().unwrap_err(), MemoryError::Cancelled);
        });
    }

    #[test]
    fn deadline_times_out() {
        let budget = MemoryBudget::new(10);
        let _held = budget.try_acquire(10).unwrap();
        let token = CancellationToken::new();
        // 期限が過去なら待たずに期限切れ。
        assert_eq!(
            budget
                .acquire_with_deadline(5, Priority::P3, &token, Instant::now())
                .unwrap_err(),
            MemoryError::TimedOut { requested: 5 }
        );
        // 誰も返却しないので、短い期限は必ず期限切れになる。
        let deadline = Instant::now() + Duration::from_millis(20);
        assert_eq!(
            budget
                .acquire_with_deadline(5, Priority::P0, &token, deadline)
                .unwrap_err(),
            MemoryError::TimedOut { requested: 5 }
        );
        assert_eq!(budget.waiting(), 0);
        assert_eq!(budget.used(), 10);
        // 空きがあれば期限に関係なく取れる。
        let budget = MemoryBudget::new(10);
        let r = budget
            .acquire_with_deadline(5, Priority::P3, &token, Instant::now())
            .unwrap();
        assert_eq!(r.bytes(), 5);
    }

    #[test]
    fn released_memory_goes_to_higher_priority_waiter_first() {
        let budget = MemoryBudget::new(100);
        let held = budget.try_acquire(100).unwrap();
        let token = CancellationToken::new();
        std::thread::scope(|s| {
            // 先に P3、後から P0 が待つ。どちらも 60 で、返却後の空き 100 には片方しか入らない。
            let low = s.spawn(|| budget.acquire_with_priority(60, Priority::P3, &token));
            wait_until(|| budget.waiting() == 1);
            let high = s.spawn(|| budget.acquire_with_priority(60, Priority::P0, &token));
            wait_until(|| budget.waiting() == 2);
            drop(held);
            let high = high.join().unwrap().unwrap();
            assert_eq!(budget.used(), 60);
            // P3 はまだ待っている（空き 40 < 60）。
            assert_eq!(budget.waiting(), 1);
            drop(high);
            let low = low.join().unwrap().unwrap();
            assert_eq!(low.bytes(), 60);
        });
    }

    #[test]
    fn same_priority_waiters_are_served_in_arrival_order() {
        let budget = MemoryBudget::new(100);
        let held = budget.try_acquire(100).unwrap();
        let token = CancellationToken::new();
        let order = Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for i in 0..3 {
                let (budget, token, order) = (&budget, &token, &order);
                s.spawn(move || {
                    // 1 件で枠全体を使うので、返却されるたびに次の 1 件だけに割り当てられる。
                    let r = budget
                        .acquire_with_priority(100, Priority::P2, token)
                        .unwrap();
                    order.lock().push(i);
                    drop(r);
                });
                wait_until(|| budget.waiting() == i + 1);
            }
            drop(held);
        });
        assert_eq!(*order.lock(), vec![0, 1, 2]);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn small_request_is_not_blocked_by_large_waiter() {
        // first-fit: 入らない大きな待ちがあっても、入る小さな「待つ取得」は通す。
        let budget = MemoryBudget::new(100);
        let _held = budget.try_acquire(50).unwrap();
        let token = CancellationToken::new();
        std::thread::scope(|s| {
            let big = s.spawn(|| budget.acquire_with_priority(80, Priority::P0, &token));
            wait_until(|| budget.waiting() == 1);
            let small = budget
                .acquire_with_priority(30, Priority::P3, &token)
                .unwrap();
            assert_eq!(small.bytes(), 30);
            token.cancel();
            assert_eq!(big.join().unwrap().unwrap_err(), MemoryError::Cancelled);
        });
    }

    #[test]
    fn try_acquire_does_not_overtake_waiters_of_same_or_higher_priority() {
        let budget = MemoryBudget::new(100);
        let _held = budget.try_acquire(50).unwrap();
        let token = CancellationToken::new();
        std::thread::scope(|s| {
            let waiter = s.spawn(|| budget.acquire_with_priority(80, Priority::P1, &token));
            wait_until(|| budget.waiting() == 1);
            // 空き 50 に 10 は入るが、P1 の待ちがあるので P1・P2・P3 の新しい処理は待たせる。
            for p in [Priority::P1, Priority::P2, Priority::P3] {
                assert!(matches!(
                    budget.try_acquire_with_priority(10, p),
                    Err(MemoryError::Unavailable { .. })
                ));
            }
            // P0 は P1 の待ちを追い越せる。
            let r = budget.try_acquire_with_priority(10, Priority::P0).unwrap();
            assert_eq!(r.bytes(), 10);
            token.cancel();
            assert_eq!(waiter.join().unwrap().unwrap_err(), MemoryError::Cancelled);
            // 待ちがなくなれば通る。
            let r = budget.try_acquire(10).unwrap();
            assert_eq!(r.bytes(), 10);
        });
    }

    #[test]
    fn shrink_returns_difference() {
        let budget = MemoryBudget::new(100);
        let mut r = budget.try_acquire(80).unwrap();
        r.shrink_to(100); // 増やせない（何もしない）。
        assert_eq!(r.bytes(), 80);
        r.shrink_to(30);
        assert_eq!(r.bytes(), 30);
        assert_eq!(budget.used(), 30);
        r.shrink_to(0);
        assert_eq!(budget.used(), 0);
        drop(r);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn shrink_wakes_waiter() {
        let budget = MemoryBudget::new(100);
        let mut held = budget.try_acquire(100).unwrap();
        std::thread::scope(|s| {
            let waiter = s.spawn(|| budget.acquire(40, &CancellationToken::new()));
            wait_until(|| budget.waiting() == 1);
            held.shrink_to(60);
            assert_eq!(waiter.join().unwrap().unwrap().bytes(), 40);
        });
        assert_eq!(budget.used(), 60);
    }

    #[test]
    fn listeners_are_notified_on_release_and_can_be_removed() {
        let budget = MemoryBudget::new(100);
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let id = budget.add_listener(Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
        let r = budget.try_acquire(10).unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 0);
        drop(r);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // 0 バイトの返却では知らせない。
        drop(budget.try_acquire(0).unwrap());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        budget.remove_listener(id);
        drop(budget.try_acquire(10).unwrap());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn concurrent_acquire_never_exceeds_total() {
        const TOTAL: u64 = 100;
        let budget = MemoryBudget::new(TOTAL);
        let in_use = AtomicU64::new(0);
        let max_seen = AtomicU64::new(0);
        std::thread::scope(|s| {
            for t in 0..8_u64 {
                let (budget, in_use, max_seen) = (&budget, &in_use, &max_seen);
                s.spawn(move || {
                    let token = CancellationToken::new();
                    for i in 0..200_u64 {
                        let bytes = 1 + (t * 7 + i * 13) % 60;
                        let r = budget.acquire(bytes, &token).unwrap();
                        let now = in_use.fetch_add(bytes, Ordering::SeqCst) + bytes;
                        max_seen.fetch_max(now, Ordering::SeqCst);
                        std::thread::yield_now();
                        in_use.fetch_sub(bytes, Ordering::SeqCst);
                        drop(r);
                    }
                });
            }
        });
        assert!(max_seen.load(Ordering::SeqCst) <= TOTAL);
        assert_eq!(budget.used(), 0);
        assert_eq!(budget.waiting(), 0);
    }

    #[test]
    fn error_messages_are_descriptive() {
        let e = MemoryError::ExceedsBudget {
            requested: 5,
            total: 3,
        };
        assert!(e.to_string().contains('5'));
        assert!(e.to_string().contains('3'));
        assert_eq!(
            MemoryError::from(crate::cancel::Cancelled),
            MemoryError::Cancelled
        );
    }
}
