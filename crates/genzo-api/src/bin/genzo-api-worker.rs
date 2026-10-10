//! 本体の実行ファイル自身を隠しサブコマンドでワーカーとして起動する構成（`genzo __worker`。
//! [`genzo_api::WorkerLaunch::SelfSubcommand`]）を確かめる最小の実行ファイル。
//!
//! 最初の引数が `__worker`（[`genzo_worker::WORKER_SUBCOMMAND`]）なら、ワーカーの本体
//! （[`genzo_worker::run_worker`]）を実行する。それ以外の引数では何もせずに終わる。genzo-cli の
//! `main` も同じ形でワーカーを呼ぶ。結合テスト（`tests/`）が使う。

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    match args.next() {
        Some(first) if first == genzo_worker::WORKER_SUBCOMMAND => {
            genzo_worker::run_worker(vec![first], args.collect())
        }
        _ => {
            eprintln!(
                "genzo-api-worker: 最初の引数に {} を指定すると、ワーカーとして動きます",
                genzo_worker::WORKER_SUBCOMMAND
            );
            ExitCode::from(2)
        }
    }
}
