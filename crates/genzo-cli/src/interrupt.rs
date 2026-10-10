//! Ctrl+C の扱い（K3）。
//!
//! - **カタログを開いている間**（[`Graceful`] を持っている間。`session::Session` が持つ）の **1 回目の
//!   Ctrl+C**: 中断を求められた印（[`requested`]）を付け、標準エラーに知らせるだけで、すぐには終了しない。
//!   ジョブを待っている処理（`Session::wait_job`）は実行中のジョブを取り消して終わるのを待ち、閉じる処理
//!   （`Session::close`）はバックグラウンドのジョブ（サムネイルの作り直しなど）を待たずに取り消して、
//!   カタログを正常に閉じる。コマンドは途中までの結果を出し、終了コード [`crate::EXIT_INTERRUPTED`]
//!   （130）で終わる。
//! - **2 回目の Ctrl+C**、または **カタログを開いていないとき**（`render`・`bench`・引数や入力の確認の
//!   間など）: すぐに終了する（終了コード 130）。カタログを開いていれば正常に閉じないため、次に開いた
//!   ときに「正常に終了しなかった」と出る（DATA-05。カタログは SQLite のトランザクションで守られ、
//!   ロックは OS が外す）。
//! - ワーカーは本体の Ctrl+C を受け取らない（genzo-worker が別のプロセスグループで起動する）。端末の
//!   Ctrl+C は前面のプロセスグループのすべてのプロセスに届くため、同じグループのワーカーは本体が取り消す
//!   前に終わり、処理中のファイルが「読めない」（`status = error`）と記録されてしまうため。
//! - ワーカーとして動くとき（`genzo __worker`）は用意しない（本体が標準入力を閉じたら終わる）。
//!
//! 受け取りには ctrlc crate を使う（Unix は SIGINT、Windows は CTRL_C_EVENT・CTRL_BREAK_EVENT）。
//! 処理は ctrlc のスレッドで行う（シグナルのハンドラの中ではない）ので、標準エラーへの出力と
//! [`std::process::exit`] を使える。

use std::sync::Once;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Ctrl+C を押された回数。
static PRESSES: AtomicU32 = AtomicU32::new(0);
/// 持っている [`Graceful`] の数。
static GRACEFUL: AtomicUsize = AtomicUsize::new(0);

/// Ctrl+C を押されたときにすること。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// 中断を求められた印を付ける（実行中の処理を取り消し、カタログを閉じてから終了する）。
    Graceful,
    /// すぐに終了する。
    Exit,
}

/// `presses` 回目の Ctrl+C ですること（`graceful`: [`Graceful`] を持っているか）。
fn decide(presses: u32, graceful: bool) -> Action {
    if presses <= 1 && graceful {
        Action::Graceful
    } else {
        Action::Exit
    }
}

/// Ctrl+C の受け取りを用意する（`main` から 1 回だけ呼ぶ。2 回目以降は何もしない）。用意できなければ
/// 警告をログに出して続ける（その場合、Ctrl+C では OS の既定のとおりすぐに終了する）。
pub fn install() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        if let Err(e) = ctrlc::set_handler(on_ctrl_c) {
            tracing::warn!(error = %e, "Ctrl+C の扱いを用意できません（Ctrl+C ではすぐに終了します）");
        }
    });
}

/// Ctrl+C を受け取った（ctrlc のスレッドで呼ばれる）。
fn on_ctrl_c() {
    let presses = PRESSES.fetch_add(1, Ordering::SeqCst).saturating_add(1);
    let graceful = GRACEFUL.load(Ordering::SeqCst) > 0;
    match decide(presses, graceful) {
        Action::Graceful => eprintln!(
            "中断します: 実行中の処理を取り消し、カタログを閉じてから終了します（すぐに終了するには、もう一度 Ctrl+C を押してください）"
        ),
        Action::Exit => {
            if graceful {
                eprintln!(
                    "中断しました（カタログを閉じずに終了したため、次に開いたときに確認をおすすめします）"
                );
            } else {
                eprintln!("中断しました");
            }
            std::process::exit(i32::from(crate::EXIT_INTERRUPTED));
        }
    }
}

/// Ctrl+C で中断を求められたか。
pub fn requested() -> bool {
    PRESSES.load(Ordering::SeqCst) > 0
}

/// 正常に終える処理（カタログを開いている間、カタログのファイルの差し替えなど）の印。持っている間の
/// 1 回目の Ctrl+C ではすぐに終了せず、[`requested`] を立てるだけにする（drop で外す）。
#[derive(Debug)]
pub struct Graceful(());

impl Graceful {
    /// 印を付ける。
    pub fn begin() -> Self {
        GRACEFUL.fetch_add(1, Ordering::SeqCst);
        Self(())
    }
}

impl Drop for Graceful {
    fn drop(&mut self) {
        GRACEFUL.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_press_in_a_session_is_graceful_and_the_second_exits() {
        assert_eq!(decide(1, true), Action::Graceful);
        assert_eq!(decide(2, true), Action::Exit);
        assert_eq!(decide(3, true), Action::Exit);
    }

    #[test]
    fn a_press_outside_a_session_exits_at_once() {
        assert_eq!(decide(1, false), Action::Exit);
        assert_eq!(decide(2, false), Action::Exit);
    }
}
