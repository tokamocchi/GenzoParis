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
//! genzo import ~/Pictures/2024-05-01            # 進捗は標準エラー（同じ内容のファイルは既定で取り込まない）
//! genzo hash compute                             # 内容のハッシュ（全体の BLAKE3）を計算して記録する
//! genzo hash duplicates                          # 内容が同じファイルの組
//! genzo search --min-rating 3 --sort capture --desc --json
//! genzo rate 12 13 4
//! genzo keyword add 12 13 --keyword "場所>京都"
//! genzo collection create "京都 2024" --add 12 13
//! genzo search --keyword "場所" --collection 1
//! genzo stack auto --gap 2 --folder ~/Pictures/2024-05-01 --dry-run
//! genzo stack create 12 13 14 --top 13
//! genzo time sync 12 "2024-05-01 12:34:56" --dry-run   # 12 と同じ機材の写真を、12 の正しい時刻に合わせる
//! genzo time zone +02:00 --keep-instant 12 13       # 撮影した瞬間はそのままで、パリの時刻で表示する
//! genzo timeline --gap 2h --folder ~/Pictures/2024-05-01
//! genzo location set 12 13 --at "35.681236, 139.767125" --dry-run   # 位置を付ける見込み（LIB-21。元のファイルは変えない）
//! genzo location clear 12 && genzo location undo   # 付けた位置を消す（カメラの位置に戻す）・取り消す
//! genzo file move 12 13 --to ~/Pictures/選別済み --dry-run   # 移す予定（移せない写真・コピーの量）
//! genzo file rename 12 13 --template "{date}-{seq}"
//! genzo missing check && genzo missing relink --in /media/me/外付け --dry-run
//! genzo video info 21 && genzo video proxy 21      # 動画の再生の情報と、再生用のファイル（H.264）を作る
//! genzo profile import "$HOME/DCP/ILCE-7M4 標準.dcp" film.cube   # カメラプロファイル・ルックを取り込む（DEV-17）
//! genzo develop set 12 --profile "ILCE-7M4 標準" --look film --look-amount 60
//! genzo develop mask add 12 --radial 0.5,0.4,0.2,0.15 --exposure 0.5 --name 顔   # ローカル補正（DEV-18・DEV-19）
//! genzo develop mask add-part 12 1 --luminance 20,80          # マスク 1 に輝度範囲を交差させる
//! genzo develop mask list 12 && genzo develop mask overlay 12 1 --out mask1.png
//! echo '{"exposure_ev": 0.5}' | genzo develop set 12 --json -
//! genzo develop snapshot create 12 "明るめ"                # スナップショット（DEV-28）
//! genzo develop snapshot apply 3                          # 履歴に 1 件記録して戻す
//! genzo preset create-develop "明るく" --from 12 --groups basic-tone,presence --folder 基本   # 現像プリセット（DEV-29）
//! genzo preset apply "明るく" --to 13 14
//! genzo preset create-export "Web 用" --format jpeg --long-edge 2048 --remove-gps --rename "{date}-{seq}"   # EXP-02
//! genzo export 12 13 --preset "Web 用" --out ~/out
//! genzo export 12 13 --out ~/out --format jpeg --quality 90 --long-edge 2048 --remove-gps
//! genzo composite 21 22 23 24 --mode lighten   # 比較明合成（DEV-34。1 枚目と同じフォルダに 16bit の TIFF）
//! genzo render DSC00001.ARW --out out.tif --settings settings.json   # カタログなし
//! genzo render DSC00001.ARW --out out.jpg --dcp camera.dcp --cube film.cube   # ファイルのプロファイル・ルック
//! genzo bench preview --synthetic 7008x4672 --out bench-results
//! genzo bench preview --synthetic 7008x4672 --sample-masks --cube film.cube   # マスク・ルックがあるときの計測
//! ```
//!
//! # 出力と終了コード
//!
//! - 結果は標準出力に、人が読む表（既定）か JSON（`--json`）で出す。`--json` のとき標準出力は **1 つの
//!   JSON の文書** だけ（失敗は `{"error": {"kind", "message", "user_actionable", "retryable", "hint"}}`。
//!   引数の解釈の誤り（clap）も同じ形で出す）。`kind` は genzo-api の `ErrorKind`（snake_case）か、使い方の
//!   誤りの `usage` だけ（[`CliError`]）。進捗・警告は標準エラー（`--quiet` で進捗を止める）。
//! - 終了コード（implementation_status の No.30。`--help` の末尾にも出す。[`args::EXIT_CODES_HELP`]）:
//!   [`EXIT_SUCCESS`]（0）成功、[`EXIT_ERROR`]（1）エラー、[`EXIT_USAGE`]（2）使い方の誤り、
//!   [`EXIT_PARTIAL`]（3）結果は出したが一部が失敗した・問題が見つかった、[`EXIT_INTERRUPTED`]（130）
//!   Ctrl+C で中断した（下の「Ctrl+C」）。
//!   - 3 にするもの: 書き出し・削除・サムネイルの作り直しで一部が失敗した、取り込みで読めずに登録
//!     できなかったファイルがある（メタデータを読めずに `status = error` で登録したものは 0）、写真の移動・
//!     名前の変更・再リンクで一部の写真を飛ばした・失敗した、`missing check` で見つからない写真があった、
//!     `catalog check` で問題が見つかった、`profile import`・`preset import` で取り込めなかったファイルがある、
//!     `bench` で前回より悪化した（今回の回数が 1.8 節の規則を満たす計測だけ）。問題の検出も「結果は出したが、問題がある」として 3 にまとめる（スクリプトは 3 を見て
//!     結果の JSON を確かめる）。
//!   - 1 にするもの: 処理できなかったエラーのほか、`trash` を `--yes` なしで中止した場合（端末では確認を
//!     求める。何も実行していないので、一部の失敗ではない）。
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
//! | `catalog` | `catalog init / info / check / backup / schema / restore` |
//! | `library` | `import`・`search`・`show`・`rate / flag / label / caption / metadata`・`thumbs`・`remove`・`trash` |
//! | `organize` | `keyword list / add / remove / show / create / rename / move / delete / export`・`collection list / create / add / remove / rename / move / delete / rule`（LIB-09〜11）と、`search --keyword / --collection / --rule` の解釈 |
//! | `stacks` | `stack create / add / remove / unstack / top / collapse / expand / show / auto`（LIB-12）と `develop rename-copy / make-master`（LIB-13） |
//! | `develop` | `develop get / set / upgrade / source / reset / undo / redo / history / copy / virtual-copy / delete-copy / rename-copy / make-master / region / before / compare`（`set` のレンズ補正のオプション・`upgrade`・`source` は DEV-14・DATA-09）と `develop snapshot list / create / rename / delete / apply`（DEV-28） |
//! | `masks` | `develop mask list / add / add-part / remove-part / set / remove / overlay / pick-color`（ローカル補正のマスク・部品・調整、重ねて表示の PNG、色域のスポイト。DEV-18・DEV-19） |
//! | `profile` | `profile import / list / show / remove / inspect`（カメラプロファイル（DCP）・ルック（3D LUT の .cube）。DEV-17） |
//! | `preset` | `preset dir / list / show / create-develop / update-develop / apply / create-export / update-export / rename / move / rename-folder / delete / import / export-files`（現像プリセット・書き出しプリセット。DEV-29・EXP-02。アプリと同じフォルダ（`--presets-dir`・`GENZO_PRESETS_DIR`）。写真の設定を使うものだけカタログを開く） |
//! | `export` | `export` と書き出しの設定（`--preset` の書き出しプリセット・`--rename` などのファイル名の規則（EXP-02）・`--ppi`・`--output-sharpen`・`--watermark-*`（EXP-03）・`--skip-videos`（動画は既定で元のファイルのままコピーする。EXP-05）） |
//! | `composite` | `composite`（比較明・比較暗合成。DEV-34。`--check` で確認だけ。docs/composite_design.md） |
//! | `watermark` | `watermark preview / font-dirs`（透かしの見本の PNG・OS のフォントのフォルダ。カタログは使わない。EXP-03） |
//! | `standalone` | カタログなしの `render`・`info`（ワーカーを直接使う） |
//! | `bench` | `bench preview / export`（genzo-testkit の計測と記録。計測の設定に `develop set` と同じオプション・`--dcp`・`--cube`・`--masks`・`--sample-masks` を当てられる） |
//! | `logs` | `log info / clear`（ログ・クラッシュレポート。カタログは使わない。SYS-07） |
//! | `identity` | `hash compute / status / duplicates / identify`（ファイルの内容による同一性。ORG-03・IMP-04） |
//! | `time` | `time devices / shift / sync / zone / clear-zone / reset / history / undo`（撮影日時の一括修正。LIB-16）と `timeline`（写真と動画のタイムライン。ORG-01） |
//! | `location` | `location set / clear / history / undo`（位置情報の後付け。LIB-21。座標の文字列は genzo-model の `parse_coordinates`） |
//! | `files` | `volume list`（FILE-03）・`folder list / create / rename / move / relocate`・`file move / rename`（FILE-02）・`missing check / relink / link`（FILE-04） |
//! | `video` | `video info / proxy / cache / clear`（動画の再生の情報と再生用のファイル。VID-04） |
//!
//! # ログ（SYS-07）
//!
//! 標準エラーへのログ（`--log`・`GENZO_LOG`）に加えて、ログのフォルダ（`--log-dir`・`GENZO_LOG_DIR`、なければ
//! アプリと同じ場所）の `cli.log` に info の水準で書き、パニックのときはクラッシュレポートを `crash/` に
//! 保存する（genzo-api の `logging`。外部へは送信しない）。ファイルに書かないときは `--no-log-file`
//! （`GENZO_LOG_FILE=off`）。ワーカー（`genzo __worker`）はファイルに書かず、本体が標準エラー出力をログに残す。
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

mod ai;
pub mod args;
mod bench;
mod catalog;
mod composite;
mod develop;
mod error;
mod export;
mod files;
mod identity;
mod interrupt;
mod library;
mod location;
mod logs;
mod masks;
mod organize;
pub mod output;
mod preset;
mod profile;
mod session;
mod stacks;
mod standalone;
mod time;
mod video;
mod watermark;

use std::ffi::OsString;
use std::process::ExitCode;

use genzo_api::DeleteKind;

pub use error::{CliError, CliResult};

use crate::args::{Cli, Command, GlobalArgs};
use crate::library::Mark;
use crate::output::Output;

/// 終了コード: 成功。
pub const EXIT_SUCCESS: u8 = 0;
/// 終了コード: エラー（処理できなかった。確認の中止を含む）。
pub const EXIT_ERROR: u8 = 1;
/// 終了コード: 使い方の誤り（引数の誤り・カタログの指定がない）。
pub const EXIT_USAGE: u8 = 2;
/// 終了コード: 結果は出したが、一部が失敗した・問題が見つかった（書き出し・削除・サムネイルの作り直しの
/// 一部の失敗、取り込みで登録できなかったファイル、`catalog check` で見つかった問題、`bench` での悪化、
/// `profile import`・`preset import` で取り込めなかったファイル。
/// implementation_status の No.30）。
pub const EXIT_PARTIAL: u8 = 3;
/// 終了コード: Ctrl+C で中断した（Unix の慣習の 128 + SIGINT。`interrupt` の doc）。
pub const EXIT_INTERRUPTED: u8 = 130;

/// コマンドの結果（出力は済んでいる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 成功（終了コード 0）。
    Success,
    /// 結果は出したが、一部が失敗した・問題が見つかった（終了コード 3。[`EXIT_PARTIAL`]）。
    Partial,
    /// 実行しなかった（確認の中止。終了コード 1）。
    Failure,
}

impl Status {
    /// 終了コード。
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Success => EXIT_SUCCESS,
            Self::Partial => EXIT_PARTIAL,
            Self::Failure => EXIT_ERROR,
        }
    }
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
    let arg_count = args.len().saturating_sub(1);
    let parsed = parse_cli(args.iter().cloned());
    let (cli, command_path) = match parsed {
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
    // 引数の値（パス・キャプション・連絡先・ID の並びなど）は記録しない。サブコマンドの名前と引数の数だけ
    // （SYS-07。決定事項ログ No.75）。
    tracing::info!(
        command = %command_path,
        args = arg_count,
        "コマンドを実行します"
    );
    interrupt::install();
    let out = Output {
        json: cli.global.json,
        quiet: cli.global.quiet,
    };
    let code = match run(cli) {
        Ok(status) => ExitCode::from(status.exit_code()),
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
///
/// あわせて、ログのフォルダ（`--log-dir`、環境変数 `GENZO_LOG_DIR`、なければアプリと同じ場所）の `cli.log` に
/// info の水準で書き（`--no-log-file` か `GENZO_LOG_FILE=off` で書かない）、パニックのときはクラッシュ
/// レポートを保存する（genzo-api の `logging`。SYS-07。外部へは送信しない）。
fn init_logging(g: &GlobalArgs) {
    let mut spec = g
        .log
        .clone()
        .or_else(|| std::env::var(ENV_LOG).ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| DEFAULT_LOG.to_owned());
    if let Err(e) = tracing_subscriber::EnvFilter::try_new(&spec) {
        eprintln!("警告: ログの指定 {spec:?} を解釈できないため warn にします: {e}");
        spec = DEFAULT_LOG.to_owned();
    }
    genzo_api::logging::init(genzo_api::logging::LogConfig {
        program: genzo_api::logging::Program::Cli,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        dir: log_dir(g),
        file_filter: g.no_log_file.then(|| "off".to_owned()),
        stderr_filter: spec,
        stderr_target: false,
    });
}

/// 引数を解釈し、サブコマンドの名前の並び（「keyword add」など。ログに書く）とともに返す。
fn parse_cli(argv: impl IntoIterator<Item = OsString>) -> Result<(args::Cli, String), clap::Error> {
    use clap::{CommandFactory, FromArgMatches};
    let matches = args::Cli::command().try_get_matches_from(argv)?;
    let cli = args::Cli::from_arg_matches(&matches)?;
    Ok((cli, subcommand_path(&matches)))
}

/// 解釈した引数のサブコマンドの名前の並び（入れ子は空白でつなぐ。引数の値は含めない）。
fn subcommand_path(matches: &clap::ArgMatches) -> String {
    let mut names = Vec::new();
    let mut cur = matches;
    while let Some((name, sub)) = cur.subcommand() {
        names.push(name.to_owned());
        cur = sub;
    }
    names.join(" ")
}

/// ログのフォルダ（`--log-dir`（相対パスは今のフォルダから）、なければ genzo-api の
/// `logging::default_log_dir`）。
pub(crate) fn log_dir(g: &GlobalArgs) -> Option<std::path::PathBuf> {
    g.log_dir
        .as_ref()
        .map(|d| std::path::absolute(d).unwrap_or_else(|_| d.clone()))
        .or_else(genzo_api::logging::default_log_dir)
}

/// プリセット（DEV-29・EXP-02）のフォルダ（`--presets-dir`（相対パスは今のフォルダから）、なければ genzo-api の
/// `default_presets_dir`: 環境変数 `GENZO_PRESETS_DIR`、なければアプリと同じ場所）。分からなければ `None`。
pub(crate) fn presets_dir(g: &GlobalArgs) -> Option<std::path::PathBuf> {
    g.presets_dir
        .as_ref()
        .map(|d| std::path::absolute(d).unwrap_or_else(|_| d.clone()))
        .or_else(genzo_api::default_presets_dir)
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
        Command::Metadata {
            variants,
            set,
            clear,
        } => library::metadata(&g, out, &variants, set, clear),
        Command::Keyword(c) => organize::keyword(&g, out, c),
        Command::Collection(c) => organize::collection(&g, out, c),
        Command::Stack(c) => stacks::run(&g, out, c),
        Command::Develop(c) => develop::run(&g, out, c),
        Command::Profile(c) => profile::run(&g, out, c),
        Command::Preset(c) => preset::run(&g, out, c),
        Command::Ai(c) => ai::run(&g, out, c),
        Command::Export(a) => export::run(&g, out, a),
        Command::Composite(a) => composite::run(&g, out, a),
        Command::Render(a) => standalone::render(&g, out, a),
        Command::Watermark(c) => watermark::run(out, c),
        Command::Info { file } => standalone::info(out, &file),
        Command::Thumbs(c) => library::thumbs(&g, out, c),
        Command::Remove { variants } => {
            library::delete(&g, out, DeleteKind::RemoveFromCatalog, &variants, true)
        }
        Command::Trash { variants, yes } => {
            library::delete(&g, out, DeleteKind::Trash, &variants, yes)
        }
        Command::Bench(a) => bench::run(&g, out, a),
        Command::Log(c) => logs::run(&g, out, c),
        Command::Hash(c) => identity::run(&g, out, c),
        Command::Time(c) => time::run(&g, out, c),
        Command::Timeline(a) => time::timeline(&g, out, &a),
        Command::Location(c) => location::run(&g, out, c),
        Command::Volume(c) => files::volume(&g, out, c),
        Command::Folder(c) => files::folder(&g, out, c),
        Command::File(c) => files::file(&g, out, c),
        Command::Missing(c) => files::missing(&g, out, c),
        Command::Video(c) => video::run(&g, out, c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 終了コードの対応（implementation_status の No.30）。
    #[test]
    fn statuses_map_to_exit_codes() {
        assert_eq!(Status::Success.exit_code(), 0);
        assert_eq!(Status::Partial.exit_code(), 3);
        assert_eq!(Status::Failure.exit_code(), 1);
        assert_eq!(
            (
                EXIT_SUCCESS,
                EXIT_ERROR,
                EXIT_USAGE,
                EXIT_PARTIAL,
                EXIT_INTERRUPTED
            ),
            (0, 1, 2, 3, 130)
        );
        assert!(args::EXIT_CODES_HELP.contains("130"));
    }

    /// ログに書くのはサブコマンドの名前だけで、引数の値（キャプション・連絡先・パスなど）は含めない
    /// （検証の指摘: 以前は全引数を cli.log に書いていた）。
    #[test]
    fn logged_command_path_has_no_argument_values() {
        let parse = |argv: &[&str]| parse_cli(argv.iter().map(OsString::from)).unwrap().1;
        assert_eq!(
            parse(&[
                "genzo",
                "--catalog",
                "/home/u/秘密/catalog.db",
                "metadata",
                "1",
                "--set",
                "creator_email=taro@example.com",
            ]),
            "metadata"
        );
        assert_eq!(
            parse(&[
                "genzo",
                "keyword",
                "add",
                "1",
                "2",
                "--keyword",
                "場所>京都"
            ]),
            "keyword add"
        );
        assert_eq!(parse(&["genzo", "log", "info"]), "log info");
        // 解釈の誤りは clap のエラー（使い方の誤り）のまま。
        assert!(parse_cli(["genzo", "--no-such-option"].map(OsString::from)).is_err());
    }

    /// `--log-dir` の相対パスは、今のフォルダからの絶対パスにする（環境変数 GENZO_LOG_DIR と同じ。表示する
    /// 場所と書く場所を同じにする）。
    #[test]
    fn log_dir_option_is_made_absolute() {
        let cli = <args::Cli as clap::Parser>::try_parse_from([
            "genzo",
            "log",
            "info",
            "--log-dir",
            "rel/logs",
        ])
        .unwrap();
        let dir = log_dir(&cli.global).unwrap();
        assert!(dir.is_absolute(), "{}", dir.display());
        assert_eq!(
            dir,
            std::env::current_dir().unwrap().join("rel").join("logs")
        );
    }
}
