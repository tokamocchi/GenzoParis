//! 取り消しトークン（docs/04_architecture.md の 6.1 節）。
//!
//! 「すべてのジョブに取り消しトークンを持たせます。処理の区切り（タイルごと、ステージごと）で
//! 確認し、取り消されたら途中でやめます」（6.1 節）を実現する。
//!
//! - 取り消しは協調的に行う。トークンを取り消しても処理は強制終了されず、処理の側が
//!   [`CancellationToken::is_cancelled`] か [`CancellationToken::check`] で確認してやめる。
//! - 子トークン（[`CancellationToken::child`]）は、親が取り消されると一緒に取り消される。
//!   子を取り消しても親には影響しない。グリッドの表示範囲のサムネイル（P1）をまとめて
//!   取り消すときなどに使う。
//! - 取り消しの確認は 1 回の不可分な読み取りだけで済む（親子の伝播は取り消しの時点で行う）。

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::Mutex;

/// 取り消されたことを表すエラー（[`CancellationToken::check`] が返す）。
///
/// [`crate::JobError`] と [`crate::MemoryError`] に変換できるため、ジョブの中では `?` で使える。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, thiserror::Error)]
#[error("処理は取り消されました")]
pub struct Cancelled;

/// 取り消しのときに呼ぶコールバック。
type Callback = Box<dyn FnOnce() + Send>;

/// 生きていない子の参照を掃除し始める件数の下限。
///
/// 掃除は件数がこの値かつ前回の掃除後の件数の 2 倍を超えたときに行う（償却で O(1)）。
/// 性能だけに関わる内部の値で、結果には影響しない。
const CHILD_PRUNE_MIN: usize = 16;

/// 取り消しトークン。
///
/// 複製（`clone`）したトークンは同じ状態を共有する。スレッド間で共有できる。
#[derive(Clone, Default)]
pub struct CancellationToken {
    inner: Arc<TokenInner>,
}

#[derive(Default)]
struct TokenInner {
    /// 取り消されたか。一度 `true` になったら戻らない。
    cancelled: AtomicBool,
    /// 取り消しのときに伝える先。
    hooks: Mutex<Hooks>,
}

#[derive(Default)]
struct Hooks {
    /// 子トークン（子を保持し続けないように弱参照にする）。
    children: Vec<Weak<TokenInner>>,
    /// 取り消しのときに呼ぶコールバック（登録の番号つき）。
    callbacks: Vec<(u64, Callback)>,
    /// 次に割り当てるコールバックの番号。
    next_callback_id: u64,
    /// 次に子の参照を掃除する件数。
    prune_at: usize,
}

impl CancellationToken {
    /// 取り消されていない新しいトークンを作る。
    pub fn new() -> Self {
        Self::default()
    }

    /// 子トークンを作る。
    ///
    /// 子は、このトークンが取り消されると一緒に取り消される。子を取り消しても、
    /// このトークンには影響しない。このトークンが既に取り消されていれば、
    /// 取り消された状態の子を返す。
    pub fn child(&self) -> CancellationToken {
        let child = CancellationToken::new();
        let mut hooks = self.inner.hooks.lock();
        if self.inner.cancelled.load(Ordering::Acquire) {
            child.inner.cancelled.store(true, Ordering::Release);
        } else {
            if hooks.children.len() >= hooks.prune_at.max(CHILD_PRUNE_MIN) {
                hooks.children.retain(|c| c.strong_count() > 0);
                hooks.prune_at = hooks.children.len() * 2;
            }
            hooks.children.push(Arc::downgrade(&child.inner));
        }
        child
    }

    /// 取り消す。子トークンも取り消し、登録されたコールバックを呼ぶ。
    ///
    /// 2 回目以降の呼び出しは何もしない。コールバックはこの関数を呼んだスレッドで
    /// 同期的に実行される。
    ///
    /// コールバックがパニックした場合も、残りのコールバックの実行と子孫の取り消しを最後まで
    /// 行ってから、最初のパニックを呼び出し元へ伝える（親子の取り消しの伝播を途中で失わない）。
    pub fn cancel(&self) {
        let mut first_panic = None;
        // 深い親子関係でもスタックを使い切らないように、再帰ではなく明示的なスタックで辿る。
        let mut pending = vec![Arc::clone(&self.inner)];
        while let Some(node) = pending.pop() {
            if node.cancelled.swap(true, Ordering::AcqRel) {
                continue;
            }
            let (children, callbacks) = {
                let mut hooks = node.hooks.lock();
                (
                    std::mem::take(&mut hooks.children),
                    std::mem::take(&mut hooks.callbacks),
                )
            };
            pending.extend(children.iter().filter_map(Weak::upgrade));
            // コールバックはロックを外してから呼ぶ（コールバックの中で登録・解除しても
            // デッドロックしないように）。
            for (_, callback) in callbacks {
                if let Err(payload) = catch_unwind(AssertUnwindSafe(callback)) {
                    first_panic.get_or_insert(payload);
                }
            }
        }
        if let Some(payload) = first_panic {
            resume_unwind(payload);
        }
    }

    /// 取り消されたか。
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// 取り消されていれば `Err(Cancelled)` を返す。処理の区切りで `?` とともに使う。
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    /// 取り消されたときに呼ぶコールバックを登録する。
    ///
    /// - 既に取り消されていれば、`callback` をこの場で（呼び出したスレッドで）呼ぶ。
    /// - コールバックは [`CancellationToken::cancel`] を呼んだスレッドで同期的に、1 回だけ
    ///   実行される。重い処理をせず、パニックしないこと（パニックは `cancel` の呼び出し元へ
    ///   伝わる）。`cancel` を呼ぶ側が持っている
    ///   ロックを取ろうとするとデッドロックするので注意する。
    /// - 返り値を drop すると登録を解除する（解除と取り消しが同時に起きた場合は、
    ///   コールバックが呼ばれることがある）。
    pub fn on_cancel<F>(&self, callback: F) -> CancelRegistration
    where
        F: FnOnce() + Send + 'static,
    {
        let mut hooks = self.inner.hooks.lock();
        if self.inner.cancelled.load(Ordering::Acquire) {
            drop(hooks);
            callback();
            return CancelRegistration::empty();
        }
        let id = hooks.next_callback_id;
        hooks.next_callback_id += 1;
        hooks.callbacks.push((id, Box::new(callback)));
        CancelRegistration {
            token: Arc::downgrade(&self.inner),
            id: Some(id),
        }
    }

    /// 2 つのトークンが同じ状態を共有している（一方が他方の複製である）か。
    pub fn same_as(&self, other: &CancellationToken) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// 登録されているコールバックの数（テスト用）。
    #[cfg(test)]
    fn callback_count(&self) -> usize {
        self.inner.hooks.lock().callbacks.len()
    }

    /// 保持している子の参照の数（テスト用）。
    #[cfg(test)]
    fn child_slot_count(&self) -> usize {
        self.inner.hooks.lock().children.len()
    }
}

impl fmt::Debug for CancellationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancellationToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// [`CancellationToken::on_cancel`] の登録。drop すると登録を解除する。
#[must_use = "drop するとコールバックの登録が解除される"]
pub struct CancelRegistration {
    token: Weak<TokenInner>,
    id: Option<u64>,
}

impl CancelRegistration {
    /// 何も登録していない値（既に取り消されていた場合など）。
    fn empty() -> Self {
        CancelRegistration {
            token: Weak::new(),
            id: None,
        }
    }
}

impl Drop for CancelRegistration {
    fn drop(&mut self) {
        let (Some(id), Some(token)) = (self.id, self.token.upgrade()) else {
            return;
        };
        // 取り出したコールバックは、ロックを外してから drop する（コールバックが捕捉した値の
        // drop で別のロックを取ることがあるため）。
        let removed = {
            let mut hooks = token.hooks.lock();
            hooks
                .callbacks
                .iter()
                .position(|(cid, _)| *cid == id)
                .map(|pos| hooks.callbacks.swap_remove(pos))
        };
        drop(removed);
    }
}

impl fmt::Debug for CancelRegistration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelRegistration")
            .field("id", &self.id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn new_token_is_not_cancelled() {
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
        assert_eq!(token.check(), Ok(()));
    }

    #[test]
    fn cancel_is_shared_by_clones_and_idempotent() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(token.same_as(&clone));
        assert!(!token.same_as(&CancellationToken::new()));
        clone.cancel();
        assert!(token.is_cancelled());
        assert_eq!(token.check(), Err(Cancelled));
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn parent_cancels_descendants_but_not_vice_versa() {
        let root = CancellationToken::new();
        let child = root.child();
        let grandchild = child.child();
        let sibling = root.child();

        grandchild.cancel();
        assert!(grandchild.is_cancelled());
        assert!(!child.is_cancelled());
        assert!(!root.is_cancelled());

        root.cancel();
        assert!(child.is_cancelled());
        assert!(sibling.is_cancelled());
    }

    #[test]
    fn child_of_cancelled_parent_starts_cancelled() {
        let root = CancellationToken::new();
        root.cancel();
        let child = root.child();
        assert!(child.is_cancelled());
        assert_eq!(root.child_slot_count(), 0);
    }

    #[test]
    fn deep_chain_cancels_without_recursion() {
        // 再帰で辿るとスタックを使い切る深さでも動くこと。
        let root = CancellationToken::new();
        let mut chain = Vec::with_capacity(100_000);
        let mut last = root.clone();
        for _ in 0..100_000 {
            let next = last.child();
            chain.push(next.clone());
            last = next;
        }
        root.cancel();
        assert!(chain.iter().all(CancellationToken::is_cancelled));
        // 長い鎖を drop してもスタックを使い切らないこと（子は親を保持しない）。
        drop(chain);
    }

    #[test]
    fn dropped_children_are_pruned() {
        let root = CancellationToken::new();
        for _ in 0..10_000 {
            let _child = root.child();
        }
        // 掃除は償却で行うので、保持する数は生きている数（0）の定数倍＋下限に収まる。
        assert!(root.child_slot_count() <= CHILD_PRUNE_MIN + 1);
        let kept: Vec<_> = (0..100).map(|_| root.child()).collect();
        root.cancel();
        assert!(kept.iter().all(CancellationToken::is_cancelled));
    }

    #[test]
    fn callback_runs_once_on_cancel() {
        let token = CancellationToken::new();
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let _reg = token.on_cancel(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(count.load(Ordering::SeqCst), 0);
        token.cancel();
        token.cancel();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn callback_on_parent_cancel_reaches_child_callbacks() {
        let root = CancellationToken::new();
        let child = root.child();
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let _reg = child.on_cancel(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        root.cancel();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn callback_on_cancelled_token_runs_immediately() {
        let token = CancellationToken::new();
        token.cancel();
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let _reg = token.on_cancel(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dropping_registration_unregisters() {
        let token = CancellationToken::new();
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        let reg = token.on_cancel(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(token.callback_count(), 1);
        drop(reg);
        assert_eq!(token.callback_count(), 0);
        token.cancel();
        assert_eq!(count.load(Ordering::SeqCst), 0);
        // 空の登録・トークンが先に消えた登録の drop も安全。
        drop(CancelRegistration::empty());
        let gone = CancellationToken::new();
        let reg = gone.on_cancel(|| {});
        drop(gone);
        drop(reg);
    }

    #[test]
    fn callback_may_register_on_same_token() {
        // コールバックの中から同じトークンに登録してもデッドロックしない（その場で実行される）。
        let token = CancellationToken::new();
        let count = Arc::new(AtomicUsize::new(0));
        let t = token.clone();
        let c = Arc::clone(&count);
        let _reg = token.on_cancel(move || {
            let c2 = Arc::clone(&c);
            let _inner = t.on_cancel(move || {
                c2.fetch_add(1, Ordering::SeqCst);
            });
            c.fetch_add(1, Ordering::SeqCst);
        });
        token.cancel();
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn panicking_callback_does_not_stop_propagation() {
        // 回帰テスト（レビューで発見）: コールバックがパニックすると、同じトークンの残りの
        // コールバックと子トークンの取り消しが行われず、「親が取り消されたら子も取り消し」が
        // 崩れていた（キューにいる子のジョブが取り消されずに実行される）。
        let root = CancellationToken::new();
        let child = root.child();
        let grandchild = child.child();
        let count = Arc::new(AtomicUsize::new(0));
        let _bad = root.on_cancel(|| panic!("コールバックのパニック（テスト用）"));
        let regs: Vec<_> = [&root, &child, &grandchild]
            .into_iter()
            .map(|token| {
                let c = Arc::clone(&count);
                token.on_cancel(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| root.cancel()));
        // パニックは呼び出し元へ伝える（握りつぶさない）が、伝播は最後まで行う。
        assert!(result.is_err());
        assert!(child.is_cancelled());
        assert!(grandchild.is_cancelled());
        assert_eq!(count.load(Ordering::SeqCst), 3);
        drop(regs);
    }

    #[test]
    fn concurrent_cancel_runs_callbacks_once() {
        const THREADS: usize = 8;
        for _ in 0..50 {
            let root = CancellationToken::new();
            let children: Vec<_> = (0..16).map(|_| root.child()).collect();
            let count = Arc::new(AtomicUsize::new(0));
            let regs: Vec<_> = children
                .iter()
                .map(|child| {
                    let c = Arc::clone(&count);
                    child.on_cancel(move || {
                        c.fetch_add(1, Ordering::SeqCst);
                    })
                })
                .collect();
            let barrier = Barrier::new(THREADS);
            std::thread::scope(|s| {
                for i in 0..THREADS {
                    let root = &root;
                    let children = &children;
                    let barrier = &barrier;
                    s.spawn(move || {
                        barrier.wait();
                        if i % 2 == 0 {
                            root.cancel();
                        } else {
                            children[i].cancel();
                        }
                    });
                }
            });
            assert!(children.iter().all(CancellationToken::is_cancelled));
            assert_eq!(count.load(Ordering::SeqCst), children.len());
            drop(regs);
        }
    }

    #[test]
    fn concurrent_child_creation_and_cancel_never_misses_a_child() {
        for _ in 0..50 {
            let root = CancellationToken::new();
            let barrier = Barrier::new(2);
            let children = std::thread::scope(|s| {
                let creator = s.spawn(|| {
                    barrier.wait();
                    (0..200).map(|_| root.child()).collect::<Vec<_>>()
                });
                barrier.wait();
                root.cancel();
                creator.join().unwrap()
            });
            // 取り消しの前に作られた子も後に作られた子も、すべて取り消されている。
            assert!(children.iter().all(CancellationToken::is_cancelled));
        }
    }

    #[test]
    fn debug_shows_state() {
        let token = CancellationToken::new();
        assert_eq!(
            format!("{token:?}"),
            "CancellationToken { cancelled: false }"
        );
        token.cancel();
        assert_eq!(
            format!("{token:?}"),
            "CancellationToken { cancelled: true }"
        );
    }
}
