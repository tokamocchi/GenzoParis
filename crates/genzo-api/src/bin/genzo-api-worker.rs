//! 本体の実行ファイル自身を隠しサブコマンドでワーカーとして起動する構成（`genzo __worker`。
//! [`genzo_api::WorkerLaunch::SelfSubcommand`]）を確かめる最小の実行ファイル。
//!
//! 最初の引数が `__worker`（[`genzo_worker::WORKER_SUBCOMMAND`]）なら、ワーカーの本体
//! （[`genzo_worker::run_worker`]）を実行する。それ以外の引数では何もせずに終わる。genzo-cli の
//! `main` も同じ形でワーカーを呼ぶ。結合テスト（`tests/`）が使う。
//!
//! テスト用:
//! - 環境変数 [`ENV_REFUSE_IF`] が指すファイルがあれば、ワーカーとして動かずに終わる（ワーカーを起動
//!   できない状態（ウイルス対策ソフトが実行ファイルを開かせないなど）を模す）。
//! - 環境変数 [`ENV_SLOW_START_IF`] が指すファイルがあれば、[`SLOW_START`] 待ってからワーカーとして動く
//!   （起動に時間がかかる状態を模し、その間に届いた要求を確かめる）。

use std::process::ExitCode;
use std::time::Duration;

/// この環境変数が指すファイルがあれば、ワーカーとして動かずに終わる（テスト用）。
const ENV_REFUSE_IF: &str = "GENZO_API_WORKER_REFUSE_IF";
/// この環境変数が指すファイルがあれば、起動を [`SLOW_START`] 遅らせる（テスト用）。
const ENV_SLOW_START_IF: &str = "GENZO_API_WORKER_SLOW_START_IF";
/// 起動を遅らせる時間。
const SLOW_START: Duration = Duration::from_secs(3);

/// 環境変数 `name` が指すファイルがあるか。
fn flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|p| std::path::Path::new(&p).exists())
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    match args.next() {
        Some(_) if flag(ENV_REFUSE_IF) => {
            eprintln!(
                "genzo-api-worker: {ENV_REFUSE_IF} のファイルがあるため、起動しません（テスト用）"
            );
            ExitCode::from(3)
        }
        Some(first) if first == genzo_worker::WORKER_SUBCOMMAND => {
            if flag(ENV_SLOW_START_IF) {
                std::thread::sleep(SLOW_START);
            }
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
