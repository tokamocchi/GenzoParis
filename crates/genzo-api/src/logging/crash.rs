//! パニックのフックとクラッシュレポート（SYS-07。外部へは送信しない）。
//!
//! - パニック（Rust の `panic!`・`unwrap` の失敗など）が起きたら、ログに 1 行書き、ログのフォルダの `crash/` に
//!   クラッシュレポート（`crash-<UTC の時刻>-<プログラム>-<プロセス ID>.txt`）を保存する。中身は、版・OS・
//!   スレッド・場所・内容・バックトレース・直前のログ。そのあとで、それまでのフック（標準エラーに出す）を呼ぶ。
//! - パニックが 1 つのスレッドの中で止まり、プロセスが続く場合（Tauri のコマンドのスレッドなど）も保存する
//!   （不具合なので）。1 つのプロセスで保存するのは [`MAX_REPORTS_PER_PROCESS`] 件まで（同じ不具合が
//!   繰り返すときにディスクを使い続けないため。超えた分はログの 1 行だけ）。
//! - 保存しておくレポートは新しいものから [`MAX_KEPT_REPORTS`] 件（起動時に古いものを消す）。
//! - 次の起動で、アプリは前回までのアプリのレポートのうち、まだ知らせていないものを知らせる
//!   （[`take_unseen_crash_reports`]。知らせたものは `crash/seen-<プログラム>` に記録する）。
//! - ネイティブのコードの異常終了（セグメンテーション違反など）・強制終了・電源断は、パニックのフックでは
//!   捕まえられない（カタログの「前回は正常に終了しなかった」の知らせ（DATA-05）で分かる）。ワーカーの
//!   異常終了は、本体のログに記録する（genzo-worker の `client`。レポートは作らない）。

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::writer::{create_log_dir, private_file_options};
use super::{LogState, Program, STATE};

/// クラッシュレポートを置くフォルダ（ログのフォルダの下）。
pub const CRASH_DIR: &str = "crash";
/// 1 つのプロセスで保存するクラッシュレポートの数の上限（**仮置き**）。
pub const MAX_REPORTS_PER_PROCESS: usize = 3;
/// 保存しておくクラッシュレポートの数（新しいものから。**仮置き**）。
pub const MAX_KEPT_REPORTS: usize = 20;
/// レポートに付けるバックトレースの大きさの上限（バイト）。
const MAX_BACKTRACE_BYTES: usize = 32 * 1024;
/// ファイル名の時刻の形式（辞書順が時刻順）。
const NAME_TIME_FORMAT: &str = "%Y%m%dT%H%M%S%3fZ";

/// クラッシュレポートの概要（一覧・起動時の知らせ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashReportSummary {
    /// ファイル。
    pub path: PathBuf,
    /// ファイル名。
    pub file_name: String,
    /// 起きた日時（ファイル名から。読めなければ `None`）。
    pub created_at: Option<DateTime<Utc>>,
    /// プログラム（`app`・`cli`）。
    pub program: String,
    /// パニックの内容（1 行目。長ければ切る）。
    pub message: Option<String>,
}

/// パニックのフックを入れる（それまでのフックは後で呼ぶ）。
pub(super) fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(state) = STATE.get() {
            handle_panic(state, info);
        }
        previous(info);
    }));
}

/// パニックの内容の文字列。
fn payload_text(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "（文字列でない内容）".to_owned()
    }
}

fn handle_panic(state: &LogState, info: &PanicHookInfo<'_>) {
    let now = Utc::now();
    let message = payload_text(info);
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "不明".to_owned());
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("名前なし").to_owned();
    let stamp = now.to_rfc3339_opts(SecondsFormat::Micros, true);
    state.sink.write_event_from_hook(
        format!(
            "{stamp} ERROR genzo_api::logging: パニック（スレッド {thread}、{location}）: {}\n",
            one_line(&message, 2000)
        )
        .as_bytes(),
    );
    let Some(dir) = &state.dir else { return };
    let n = state.reports_written.fetch_add(1, Ordering::SeqCst);
    if n >= MAX_REPORTS_PER_PROCESS {
        return;
    }
    let mut backtrace = std::backtrace::Backtrace::force_capture().to_string();
    truncate_at_char(&mut backtrace, MAX_BACKTRACE_BYTES);
    let report = CrashReport {
        time: now,
        program: state.program,
        version: &state.version,
        os: &state.os,
        pid: std::process::id(),
        thread: &thread,
        location: &location,
        message: &message,
        backtrace: &backtrace,
        recent_log: state.sink.recent_from_hook().as_deref(),
    }
    .render();
    let line = match write_report(dir, now, state.program, &report) {
        Ok(path) => format!(
            "{stamp} ERROR genzo_api::logging: クラッシュレポートを保存しました: {}\n",
            path.display()
        ),
        Err(e) => {
            format!("{stamp} ERROR genzo_api::logging: クラッシュレポートを保存できません: {e}\n")
        }
    };
    state.sink.write_event_from_hook(line.as_bytes());
}

/// 改行を空白にし、`max` バイトまでにした文字列。
fn one_line(s: &str, max: usize) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    truncate_at_char(&mut out, max);
    out
}

/// `max` バイト以下の、文字の境目で切る（切ったら「…」を付ける）。
fn truncate_at_char(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push('…');
}

/// クラッシュレポートの中身。
pub(crate) struct CrashReport<'a> {
    pub time: DateTime<Utc>,
    pub program: Program,
    pub version: &'a str,
    pub os: &'a str,
    pub pid: u32,
    pub thread: &'a str,
    pub location: &'a str,
    pub message: &'a str,
    pub backtrace: &'a str,
    pub recent_log: Option<&'a str>,
}

/// レポートの「内容」の行の見出し（一覧で 1 行目を読む）。
const MESSAGE_LABEL: &str = "内容: ";

impl CrashReport<'_> {
    pub(crate) fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "GenzoParis のクラッシュレポート");
        let _ = writeln!(
            s,
            "このファイルは、この PC の中だけに保存しています（外部へは送信しません）。不具合の報告に使うときは、\
             ファイル名・フォルダの場所などの個人的な情報が含まれていないかを確かめてから渡してください。"
        );
        let _ = writeln!(s);
        let _ = writeln!(
            s,
            "日時（UTC）: {}",
            self.time.to_rfc3339_opts(SecondsFormat::Millis, true)
        );
        let _ = writeln!(
            s,
            "プログラム: {}（{}） {}",
            self.program.display_name(),
            self.program.stem(),
            self.version
        );
        let _ = writeln!(s, "OS: {}", self.os);
        let _ = writeln!(s, "プロセス ID: {}", self.pid);
        let _ = writeln!(s, "スレッド: {}", self.thread);
        let _ = writeln!(s, "場所: {}", self.location);
        let _ = writeln!(s, "{MESSAGE_LABEL}{}", self.message);
        let _ = writeln!(s);
        let _ = writeln!(s, "バックトレース:");
        let _ = writeln!(s, "{}", self.backtrace.trim_end());
        let _ = writeln!(s);
        let _ = writeln!(s, "直前のログ（新しいものが最後）:");
        match self.recent_log {
            Some(log) if !log.is_empty() => {
                let _ = write!(s, "{log}");
                if !log.ends_with('\n') {
                    let _ = writeln!(s);
                }
            }
            Some(_) => {
                let _ = writeln!(s, "（なし）");
            }
            None => {
                let _ = writeln!(s, "（読めませんでした）");
            }
        }
        s
    }
}

/// レポートのファイル名（`crash-<UTC の時刻>-<プログラム>-<プロセス ID>.txt`）。
fn report_name(time: DateTime<Utc>, program: Program, pid: u32, n: u32) -> String {
    let stamp = time.format(NAME_TIME_FORMAT);
    if n == 0 {
        format!("crash-{stamp}-{}-{pid}.txt", program.stem())
    } else {
        format!("crash-{stamp}-{}-{pid}-{n}.txt", program.stem())
    }
}

/// レポートを書く（既存のファイルは上書きしない。名前が重なれば番号を付ける）。
fn write_report(
    dir: &Path,
    time: DateTime<Utc>,
    program: Program,
    report: &str,
) -> std::io::Result<PathBuf> {
    let crash_dir = dir.join(CRASH_DIR);
    create_log_dir(&crash_dir)?;
    let pid = std::process::id();
    for n in 0..100 {
        let path = crash_dir.join(report_name(time, program, pid, n));
        match private_file_options()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(report.as_bytes())?;
                f.sync_all()?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("同じ名前のレポートが多すぎます"))
}

/// ファイル名が GenzoParis のクラッシュレポートの形なら、(時刻, プログラム, プロセス ID)。
pub(crate) fn parse_report_name(name: &str) -> Option<(Option<DateTime<Utc>>, String, u32)> {
    let rest = name.strip_prefix("crash-")?.strip_suffix(".txt")?;
    let mut parts = rest.split('-');
    let stamp = parts.next()?;
    let program = parts.next()?;
    let pid: u32 = parts.next()?.parse().ok()?;
    if let Some(n) = parts.next() {
        n.parse::<u32>().ok()?;
    }
    if parts.next().is_some() || Program::from_stem(program).is_none() {
        return None;
    }
    let time = NaiveDateTime::parse_from_str(stamp, NAME_TIME_FORMAT)
        .ok()
        .map(|t| t.and_utc());
    Some((time, program.to_owned(), pid))
}

/// レポートの「内容」の行（長ければ切る）。
fn read_message(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix(MESSAGE_LABEL))?;
    Some(one_line(line, 300))
}

/// ログのフォルダのクラッシュレポート（新しい順）。
pub fn list_crash_reports(dir: &Path) -> Vec<CrashReportSummary> {
    let Ok(entries) = fs::read_dir(dir.join(CRASH_DIR)) else {
        return Vec::new();
    };
    let mut out: Vec<CrashReportSummary> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let file_name = e.file_name().to_str()?.to_owned();
            let (created_at, program, _) = parse_report_name(&file_name)?;
            let path = e.path();
            Some(CrashReportSummary {
                message: read_message(&path),
                path,
                file_name,
                created_at,
                program,
            })
        })
        .collect();
    out.sort_by(|a, b| b.file_name.cmp(&a.file_name));
    out
}

/// 知らせた最後のレポートの名前を記録するファイル。
fn seen_path(dir: &Path, program: Program) -> PathBuf {
    dir.join(CRASH_DIR).join(format!("seen-{}", program.stem()))
}

/// まだ知らせていない、プログラム `program` の前回までのレポート（新しい順）を返し、知らせたことを記録する。
///
/// 今のプロセスが書いたレポートは「前回」ではないので含めない（記録もしない）。記録できなくても、
/// 一覧は返す（次の起動でもう一度知らせることがある）。
pub fn take_unseen_crash_reports(dir: &Path, program: Program) -> Vec<CrashReportSummary> {
    let seen = fs::read_to_string(seen_path(dir, program))
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();
    let me = std::process::id();
    let unseen: Vec<CrashReportSummary> = list_crash_reports(dir)
        .into_iter()
        .filter(|r| r.program == program.stem())
        .filter(|r| parse_report_name(&r.file_name).is_some_and(|(_, _, pid)| pid != me))
        .filter(|r| r.file_name > seen)
        .collect();
    if let Some(newest) = unseen.first()
        && let Err(e) = fs::write(seen_path(dir, program), &newest.file_name)
    {
        tracing::warn!(error = %e, "クラッシュレポートを知らせたことを記録できない");
    }
    unseen
}

/// 古いクラッシュレポートを消す（新しいものから `keep` 件を残す）。
pub(crate) fn prune_crash_reports(dir: &Path, keep: usize) {
    for old in list_crash_reports(dir).into_iter().skip(keep) {
        if let Err(e) = fs::remove_file(&old.path) {
            tracing::warn!(path = %old.path.display(), error = %e, "古いクラッシュレポートを消せない");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(recent: Option<&str>) -> String {
        CrashReport {
            time: DateTime::parse_from_rfc3339("2026-10-13T01:02:03.456Z")
                .unwrap()
                .with_timezone(&Utc),
            program: Program::App,
            version: "0.1.0",
            os: "linux x86_64（Ubuntu 24.04 LTS）",
            pid: 42,
            thread: "main",
            location: "crates/genzo-app/src/lib.rs:10:5",
            message: "index out of bounds",
            backtrace: "   0: genzo_app::run\n",
            recent_log: recent,
        }
        .render()
    }

    #[test]
    fn report_contains_version_os_backtrace_and_recent_log() {
        let r = sample(Some(
            "2026-10-13T01:02:03Z  INFO genzo_app: カタログを開いた\n",
        ));
        for expected in [
            "外部へは送信しません",
            "日時（UTC）: 2026-10-13T01:02:03.456Z",
            "プログラム: アプリ（app） 0.1.0",
            "OS: linux x86_64（Ubuntu 24.04 LTS）",
            "プロセス ID: 42",
            "スレッド: main",
            "場所: crates/genzo-app/src/lib.rs:10:5",
            "内容: index out of bounds",
            "バックトレース:\n   0: genzo_app::run\n",
            "直前のログ（新しいものが最後）:\n2026-10-13T01:02:03Z  INFO genzo_app: カタログを開いた\n",
        ] {
            assert!(r.contains(expected), "{expected:?} がない:\n{r}");
        }
        assert!(sample(None).contains("（読めませんでした）"));
        assert!(sample(Some("")).contains("（なし）"));
    }

    #[test]
    fn report_names_round_trip_and_sort_by_time() {
        let t = DateTime::parse_from_rfc3339("2026-10-13T01:02:03.456Z")
            .unwrap()
            .with_timezone(&Utc);
        let name = report_name(t, Program::Cli, 7, 0);
        assert_eq!(name, "crash-20261013T010203456Z-cli-7.txt");
        assert_eq!(
            parse_report_name(&name),
            Some((Some(t), "cli".to_owned(), 7))
        );
        assert_eq!(
            parse_report_name(&report_name(t, Program::App, 7, 2)),
            Some((Some(t), "app".to_owned(), 7))
        );
        for bad in [
            "crash-20261013T010203456Z-other-7.txt",
            "crash-20261013T010203456Z-app-x.txt",
            "crash-20261013T010203456Z-app-7.log",
            "notes.txt",
            "crash-20261013T010203456Z-app-7-1-2.txt",
        ] {
            assert_eq!(parse_report_name(bad), None, "{bad}");
        }
        let later = report_name(t + chrono::TimeDelta::seconds(1), Program::App, 1, 0);
        assert!(later > report_name(t, Program::App, 9, 0));
    }

    #[test]
    fn reports_are_written_without_overwriting_and_listed_newest_first() {
        let d = tempfile::tempdir().unwrap();
        let t = Utc::now();
        let a = write_report(d.path(), t, Program::App, "内容: 1 つ目\n").unwrap();
        let b = write_report(d.path(), t, Program::App, "内容: 2 つ目\n").unwrap();
        assert_ne!(a, b);
        assert_eq!(fs::read_to_string(&a).unwrap(), "内容: 1 つ目\n");
        let later = write_report(
            d.path(),
            t + chrono::TimeDelta::seconds(5),
            Program::Cli,
            "内容: 新しい\n",
        )
        .unwrap();
        // GenzoParis のレポートの形でないファイルは数えない。
        fs::write(d.path().join(CRASH_DIR).join("memo.txt"), "x").unwrap();
        // Unix では、作るフォルダ・レポートを利用者だけが読めるようにする（個人的な情報が入るため）。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&d.path().join(CRASH_DIR)), 0o700);
            assert_eq!(mode(&a), 0o600);
        }
        let list = list_crash_reports(d.path());
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].path, later);
        assert_eq!(list[0].program, "cli");
        assert_eq!(list[0].message.as_deref(), Some("新しい"));
        prune_crash_reports(d.path(), 1);
        assert_eq!(list_crash_reports(d.path()).len(), 1);
        assert!(d.path().join(CRASH_DIR).join("memo.txt").exists());
    }

    #[test]
    fn unseen_reports_are_reported_once_and_exclude_this_process() {
        let d = tempfile::tempdir().unwrap();
        let crash = d.path().join(CRASH_DIR);
        fs::create_dir_all(&crash).unwrap();
        let other_pid = std::process::id().wrapping_add(1);
        let old = format!("crash-20261001T000000000Z-app-{other_pid}.txt");
        let newer = format!("crash-20261002T000000000Z-app-{other_pid}.txt");
        let cli = format!("crash-20261003T000000000Z-cli-{other_pid}.txt");
        let mine = format!("crash-20261004T000000000Z-app-{}.txt", std::process::id());
        for name in [&old, &newer, &cli, &mine] {
            fs::write(crash.join(name), "内容: x\n").unwrap();
        }
        let first = take_unseen_crash_reports(d.path(), Program::App);
        assert_eq!(
            first
                .iter()
                .map(|r| r.file_name.as_str())
                .collect::<Vec<_>>(),
            vec![newer.as_str(), old.as_str()]
        );
        // 2 回目は知らせない。
        assert!(take_unseen_crash_reports(d.path(), Program::App).is_empty());
        // その後の新しいレポートだけを知らせる。
        let next = format!("crash-20261005T000000000Z-app-{other_pid}.txt");
        fs::write(crash.join(&next), "内容: y\n").unwrap();
        let second = take_unseen_crash_reports(d.path(), Program::App);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].file_name, next);
        // フォルダがなければ空。
        let empty = tempfile::tempdir().unwrap();
        assert!(take_unseen_crash_reports(empty.path(), Program::App).is_empty());
    }

    #[test]
    fn long_texts_are_cut_at_character_boundaries() {
        let mut s = "あいうえお".to_owned();
        truncate_at_char(&mut s, 7);
        assert_eq!(s, "あい…");
        assert_eq!(one_line("a\nb\r\nc", 100), "a b  c");
    }
}
