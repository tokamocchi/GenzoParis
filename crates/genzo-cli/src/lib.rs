//! 検証用の CLI（バイナリ `genzo`。01 の ORG-05、02 の MAINT-01・MAINT-07、04 の 1.4 節、05 の 1.8 節）。
//!
//! UI なしで登録・検索・現像・書き出し・計測を行う。自動テストと、AI による動作確認に使う。処理は
//! genzo-api（コア API）を通して行い、UI と同じ経路を確かめる（MAINT-01）。
//!
//! # 使い方の例
//!
//! ```text
//! export GENZO_CATALOG=~/genzo/catalog.db
//! genzo catalog init
//! genzo import ~/Pictures/2024-05-01            # 進捗は標準エラー
//! genzo search --min-rating 3 --sort capture --desc --json
//! genzo rate 12 13 4
//! echo '{"exposure_ev": 0.5}' | genzo develop set 12 --json -
//! genzo export 12 13 --out ~/out --format jpeg --quality 90 --long-edge 2048 --remove-gps
//! genzo render DSC00001.ARW --out out.tif --settings settings.json   # カタログなし
//! genzo bench preview --synthetic 7008x4672 --out bench-results
//! ```
//!
//! # 出力と終了コード
//!
//! - 結果は標準出力に、人が読む表（既定）か JSON（`--json`）で出す。`--json` のとき標準出力は **1 つの
//!   JSON の文書** だけ（失敗は `{"error": {"kind", "message", "user_actionable", "retryable", "hint"}}`。
//!   引数の解釈の誤り（clap）も同じ形で出す）。`kind` は genzo-api の `ErrorKind`（snake_case）か、使い方の
//!   誤りの `usage` だけ（[`CliError`]）。進捗・警告は標準エラー（`--quiet` で進捗を止める）。
//! - 終了コード: [`EXIT_SUCCESS`]（0）成功、[`EXIT_ERROR`]（1）エラー、[`EXIT_USAGE`]（2）使い方の誤り、
//!   [`EXIT_INTERRUPTED`]（130）Ctrl+C で中断した（下の「Ctrl+C」）。
//!   次も 1 にする: 書き出し・削除・サムネイルの作り直しで一部が失敗した、取り込みで読めずに登録
//!   できなかったファイルがある（メタデータを読めずに `status = error` で登録したものは 0）、
//!   `catalog check` で問題が見つかった、`trash` を `--yes` なしで中止した（端末では確認を求める）、
//!   `bench` で前回より悪化した（今回の回数が 1.8 節の規則を満たす計測だけ）。
//! - 入力の JSON（`develop set`・`render --settings`）は UTF-8（BOM 付きも）と BOM 付きの UTF-16 を読む
//!   （Windows のメモ帳・PowerShell 5.1 の `>` で保存したファイル）。
//!
//! # Ctrl+C（K3。`interrupt` の doc）
//!
//! - カタログを開いている間の 1 回目の Ctrl+C: 実行中のジョブ（取り込み・書き出しなど）を取り消し、
//!   バックグラウンドのジョブ（取り込みの後のサムネイルの作り直しなど）は待たずに取り消して、カタログを
//!   正常に閉じてから終了する。取り込み・書き出しは途中までの結果（`cancelled: true`）を出す。取り込みは、
//!   もう一度実行すると続きから処理する。端末での確認（`trash`）は「いいえ」にする。
//! - 2 回目の Ctrl+C、またはカタログを開いていないとき（`render`・`bench`・`info` など）: すぐに終了する
//!   （カタログを開いていれば、次に開いたときに「正常に終了しなかった」と出る）。
//! - どちらも終了コードは [`EXIT_INTERRUPTED`]（130。Unix の慣習の 128 + SIGINT。Windows でも同じ値）。
//! - ワーカーは本体の Ctrl+C を受け取らない（genzo-worker が別のプロセスグループで起動する）。
//!
//! # 構成
//!
//! | モジュール | 内容 |
//! |---|---|
//! | [`args`] | clap の定義、値の解釈（評価・日付・寸法など）、`develop set --json <FILE>` の書き換え |
//! | `session` | カタログを使うコマンドの共通の処理（コアを開く・ジョブの進捗・閉じる・Ctrl+C での取り消し） |
//! | `interrupt` | Ctrl+C の受け取り（1 回目は取り消して閉じる、2 回目はすぐに終了） |
//! | `catalog` | `catalog init / info / check / backup / restore` |
//! | `library` | `import`・`search`・`show`・`rate / flag / label / caption`・`thumbs`・`remove`・`trash` |
//! | `develop` | `develop get / set / reset / undo / redo / history / copy / virtual-copy / delete-copy` |
//! | `export` | `export` と書き出しの設定 |
//! | `standalone` | カタログなしの `render`・`info`（ワーカーを直接使う） |
//! | `bench` | `bench preview / export`（genzo-testkit の計測と記録） |
//!
//! # ワーカー
//!
//! 最初の引数が隠しサブコマンド `__worker`（[`genzo_worker::WORKER_SUBCOMMAND`]）なら、ワーカーの本体
//! （[`genzo_worker::run_worker`]）を実行する。コアは本体の実行ファイル自身をこの形で起動する
//! （genzo-api の `WorkerLaunch::current_exe`。04 の 1.2 節。ワーカーの実行ファイルを別に配布しない）。
//!
//! # 設計からの逸脱
//!
//! - **依存の向き**（04 の 1.4 節では CLI は genzo-api だけに依存する）: `render`・`info`（カタログなし）と
//!   `bench`（段階 C などを直接測る）のため、genzo-worker・genzo-pipeline・genzo-gpu・genzo-media・
//!   genzo-testkit を直接使う。genzo-api の `Core` はカタログが前提で、計測には段階ごとの呼び出しが
//!   必要なため。展開・書き出しの手順は genzo-api と同じにしている（`standalone` の doc）。
//! - **計測の応答時間**: 1.8 節の定義（入力イベントから画面への表示まで）ではなく、呼び出しから結果が
//!   戻るまで（`bench` の doc）。UI での計測は PoC-1・PoC-3 で行う。
//!
//! # データの保全（6.4 節、DATA-01）
//!
//! - 元ファイルは書き換えない。書き出しは genzo-api・genzo-media の安全な書き出し（原本・カタログの
//!   ファイルへは書き出さない。`render` は入力のファイルを保護する）。`export --out` に既存のファイルを
//!   指定した場合は、使い方の誤りにする。
//! - `catalog restore --to` に既にあるファイルは、GenzoParis のカタログ（SQLite の application_id）の
//!   ときだけ退避して置き換える（写真などを取り違えて指定しても、名前を変えない）。
//! - 取り込みでは、アプリのデータのフォルダ（プレビューのキャッシュなど）を飛ばす（genzo-api）。

pub mod args;
mod bench;
mod catalog;
mod develop;
mod error;
mod export;
mod interrupt;
mod library;
pub mod output;
mod session;
mod standalone;

use std::ffi::OsString;
use std::process::ExitCode;

use genzo_api::DeleteKind;

pub use error::{CliError, CliResult};

use crate::args::{Cli, Command, GlobalArgs};
use crate::library::Mark;
use crate::output::Output;

/// 終了コード: 成功。
pub const EXIT_SUCCESS: u8 = 0;
/// 終了コード: エラー（一部の失敗・確認の中止・チェックで見つかった問題を含む）。
pub const EXIT_ERROR: u8 = 1;
/// 終了コード: 使い方の誤り（引数の誤り・カタログの指定がない）。
pub const EXIT_USAGE: u8 = 2;
/// 終了コード: Ctrl+C で中断した（Unix の慣習の 128 + SIGINT。`interrupt` の doc）。
pub const EXIT_INTERRUPTED: u8 = 130;

/// コマンドの結果（出力は済んでいる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 成功（終了コード 0）。
    Success,
    /// 結果は出したが、一部が失敗した・問題が見つかった（終了コード 1）。
    Failure,
}

/// ログの水準を指定する環境変数（`--log` がなければ使う）。
pub const ENV_LOG: &str = "GENZO_LOG";

/// ログの既定の水準（warn）。
///
/// Vulkan のローダーの知らせ（`wgpu_hal::vulkan::instance`）は既定では出さない。GPU のドライバーがない
/// 環境で `--gpu auto` のとき、ローダーが「ドライバーが見つからない」を error の水準で出すが、CLI は
/// CPU 版で処理を続けるため（2026-10-10 の統合の確認で、Vulkan の ICD を隠した Linux で確認）。
/// 調べるときは `--log debug` などで指定し直す（`--log` を指定すれば、この既定は使わない）。
const DEFAULT_LOG: &str = "warn,wgpu_hal::vulkan::instance=off";

/// `genzo` の入口（`main` から呼ぶ）。
pub fn main_entry() -> ExitCode {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    // ワーカーとして起動された（`genzo __worker ...`）。
    if args
        .get(1)
        .is_some_and(|a| a == genzo_worker::WORKER_SUBCOMMAND)
    {
        let rest = args.split_off(2);
        return genzo_worker::run_worker(vec![genzo_worker::WORKER_SUBCOMMAND.into()], rest);
    }
    let args = args::rewrite_develop_set(args);
    let json_requested = args::json_requested(&args);
    let cli = match <args::Cli as clap::Parser>::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            // --help・--version は標準出力で成功、それ以外は使い方の誤り（clap の終了コードは 2）。
            // --json のときは、使い方の誤りも標準出力に JSON で出す（標準出力を JSON として読む側が
            // 空の出力で止まらないように）。clap の説明は標準エラーにも出す。
            if json_requested && !args::is_help_or_version(&e) {
                Output {
                    json: true,
                    quiet: false,
                }
                .print_json(&error::usage_json(&args::clap_message(&e)));
            }
            let _ = e.print();
            return ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(EXIT_USAGE));
        }
    };
    init_logging(&cli.global);
    interrupt::install();
    let out = Output {
        json: cli.global.json,
        quiet: cli.global.quiet,
    };
    let code = match run(cli) {
        Ok(Status::Success) => ExitCode::from(EXIT_SUCCESS),
        Ok(Status::Failure) => ExitCode::from(EXIT_ERROR),
        Err(e) => {
            if out.json {
                out.print_json(&e.to_json());
            }
            eprintln!("{}", e.human());
            ExitCode::from(e.exit_code())
        }
    };
    // Ctrl+C で中断した（実行中のジョブを取り消し、カタログを閉じた後。結果・エラーは上で出した）。
    if interrupt::requested() {
        eprintln!("中断しました");
        return ExitCode::from(EXIT_INTERRUPTED);
    }
    code
}

/// tracing のログを標準エラーに出す（`--log`、環境変数 `GENZO_LOG`、既定は [`DEFAULT_LOG`]）。
fn init_logging(g: &GlobalArgs) {
    let spec = g
        .log
        .clone()
        .or_else(|| std::env::var(ENV_LOG).ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| DEFAULT_LOG.to_owned());
    let filter = tracing_subscriber::EnvFilter::try_new(&spec).unwrap_or_else(|e| {
        eprintln!("警告: ログの指定 {spec:?} を解釈できないため warn にします: {e}");
        tracing_subscriber::EnvFilter::new(DEFAULT_LOG)
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

/// 解釈した引数でコマンドを実行する。
pub fn run(cli: Cli) -> CliResult<Status> {
    let g = cli.global;
    let out = Output {
        json: g.json,
        quiet: g.quiet,
    };
    match cli.command {
        Command::Catalog(c) => catalog::run(&g, out, c),
        Command::Import(a) => library::import(&g, out, a),
        Command::Search(a) => library::search(&g, out, *a),
        Command::Show { variant } => library::show(&g, out, variant),
        Command::Rate { variants, rating } => {
            library::mark(&g, out, &variants, Mark::Rating(rating))
        }
        Command::Flag { variants, flag } => library::mark(&g, out, &variants, Mark::Flag(flag)),
        Command::Label { variants, label } => library::mark(&g, out, &variants, Mark::Label(label)),
        Command::Caption { variant, text } => library::caption(&g, out, variant, &text),
        Command::Develop(c) => develop::run(&g, out, c),
        Command::Export(a) => export::run(&g, out, a),
        Command::Render(a) => standalone::render(&g, out, a),
        Command::Info { file } => standalone::info(out, &file),
        Command::Thumbs(c) => library::thumbs(&g, out, c),
        Command::Remove { variants } => {
            library::delete(&g, out, DeleteKind::RemoveFromCatalog, &variants, true)
        }
        Command::Trash { variants, yes } => {
            library::delete(&g, out, DeleteKind::Trash, &variants, yes)
        }
        Command::Bench(a) => bench::run(&g, out, a),
    }
}
