//! ワーカープロセスの入口（`genzo-worker`）。
//!
//! 本体（[`genzo_worker::WorkerClient`]）から起動され、標準入出力で 1 行 1 メッセージの JSON の
//! ジョブを受け取る（docs/04_architecture.md の 1.2 節）。処理の本体はライブラリ側の
//! [`genzo_worker::worker`] に置く。
//!
//! 引数: `--memory-limit-bytes <N>`（メモリの上限。Linux だけ。`genzo_worker::limits`）。

fn main() -> std::process::ExitCode {
    genzo_worker::worker::main_entry()
}
