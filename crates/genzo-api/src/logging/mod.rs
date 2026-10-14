//! ログとクラッシュレポート（01 の SYS-07。不具合の解析のため。**外部へは送信しない**: 02 の SEC-02）。
//!
//! アプリ（genzo-app）と CLI（genzo-cli）が起動の最初に [`init`] を呼ぶ。tracing のログを、標準エラー
//! （今までどおり。水準は呼び出し側が決める）と、ログのフォルダのファイルの両方に書き、パニックのフック
//! （[`crash`]）を入れる。
//!
//! # ログのフォルダ（[`default_log_dir`]）
//!
//! 環境変数 [`ENV_LOG_DIR`]（`GENZO_LOG_DIR`）があればそのフォルダ、なければ OS ごとのアプリのデータの
//! フォルダ（Tauri の `app_log_dir` と同じ場所。識別子は [`APP_IDENTIFIER`]）:
//!
//! | OS | 場所 |
//! |---|---|
//! | Windows | `%LOCALAPPDATA%\io.github.tokamocchi.genzoparis\logs` |
//! | macOS | `~/Library/Logs/io.github.tokamocchi.genzoparis` |
//! | Linux | `$XDG_DATA_HOME/io.github.tokamocchi.genzoparis/logs`（なければ `~/.local/share/...`） |
//!
//! | ファイル | 内容 |
//! |---|---|
//! | `app.log`・`app.1.log` 〜 `app.3.log` | アプリ（本体）のログ。ワーカーの標準エラー出力と異常終了も本体のログに入る（genzo-worker の `client`） |
//! | `cli.log`・`cli.1.log` 〜 `cli.3.log` | CLI のログ |
//! | `crash/crash-<UTC の時刻>-<プログラム>-<プロセス ID>.txt` | クラッシュレポート（[`crash`]） |
//! | `crash/seen-<プログラム>` | 起動時に知らせた最後のクラッシュレポートの名前 |
//!
//! - 1 つのファイルの上限は [`MAX_LOG_FILE_BYTES`]（5 MiB）で、超えたら古いものへずらし、
//!   [`KEPT_ROTATED_FILES`]（3）個まで残す（プログラムごとに最大でおよそ 20 MiB。**仮置き**）。
//! - ファイルに書く水準は、環境変数 [`ENV_LOG_FILE`]（`GENZO_LOG_FILE`。tracing の指定。`off` でファイルに
//!   書かない）、なければ [`DEFAULT_FILE_FILTER`]（info。wgpu・naga は warn）。
//! - ログには、写真のファイル名・フォルダの場所が入る（ローカルにだけ保存する前提）。画面の設定から
//!   フォルダを開く・消す（[`folder_info`]・[`clear_logs`]）。消すのは、上の名前の形のファイルだけ
//!   （`GENZO_LOG_DIR` に別のフォルダを指定しても、ほかのファイルは消さない）。
//! - Unix（Linux・macOS）では、作るフォルダは利用者だけ（0700）、ファイルは利用者だけ（0600）が読める権限に
//!   する（[`create_log_dir`]。既にあるフォルダ・ファイルの権限は変えない）。Windows は親のフォルダ
//!   （`%LOCALAPPDATA%`。利用者ごと）の権限を受け継ぐ。
//! - 時刻は UTC（tracing の既定の形式。例: `2026-10-13T01:02:03.456789Z`）。

pub mod crash;
mod system;
mod writer;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

pub use crash::{
    CRASH_DIR, CrashReportSummary, MAX_KEPT_REPORTS, MAX_REPORTS_PER_PROCESS, list_crash_reports,
    take_unseen_crash_reports,
};

pub use writer::create_log_dir;
use writer::{RotatingFile, Sink, SinkMakeWriter};

/// アプリの識別子（genzo-app の `tauri.conf.json` の `identifier` と同じ。ログのフォルダの名前に使う）。
pub const APP_IDENTIFIER: &str = "io.github.tokamocchi.genzoparis";
/// ログのフォルダを指定する環境変数。
pub const ENV_LOG_DIR: &str = "GENZO_LOG_DIR";
/// ログのファイルに書く水準を指定する環境変数（tracing の指定。`off` でファイルに書かない）。
pub const ENV_LOG_FILE: &str = "GENZO_LOG_FILE";
/// ログのファイルに書く既定の水準。
pub const DEFAULT_FILE_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn";
/// 1 つのログのファイルの大きさの上限（バイト。**仮置き**）。
pub const MAX_LOG_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// 残す古いログのファイルの数（**仮置き**）。
pub const KEPT_ROTATED_FILES: usize = 3;
/// クラッシュレポートに付ける直前のログの大きさ（バイト）。
pub const RECENT_LOG_BYTES: usize = 64 * 1024;

/// ログを書くプログラム。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Program {
    /// デスクトップアプリ（genzo-app）。
    App,
    /// 検証用の CLI（genzo-cli）。
    Cli,
}

impl Program {
    /// ファイル名に使う名前（`app`・`cli`）。
    pub fn stem(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Cli => "cli",
        }
    }

    /// 表示名。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::App => "アプリ",
            Self::Cli => "CLI",
        }
    }

    fn from_stem(s: &str) -> Option<Self> {
        match s {
            "app" => Some(Self::App),
            "cli" => Some(Self::Cli),
            _ => None,
        }
    }
}

/// [`init`] の設定。
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// プログラム。
    pub program: Program,
    /// プログラムの版（クラッシュレポート・最初の行に書く）。
    pub version: String,
    /// ログのフォルダ（`None` ならファイルにもクラッシュレポートにも書かない）。
    pub dir: Option<PathBuf>,
    /// ファイルに書く水準（tracing の指定。`None` なら [`ENV_LOG_FILE`]、なければ [`DEFAULT_FILE_FILTER`]。
    /// `off` でファイルに書かない（クラッシュレポートは書く））。
    pub file_filter: Option<String>,
    /// 標準エラーに出す水準（tracing の指定）。
    pub stderr_filter: String,
    /// 標準エラーの行に、ログを出したモジュールの名前を付けるか。
    pub stderr_target: bool,
}

/// [`init`] の結果（画面の設定・CLI の `log info` に出す）。既定値は「初期化していない」（フォルダなし）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LogStatus {
    /// ログのフォルダ。
    pub dir: Option<PathBuf>,
    /// 今書いているログのファイル（ファイルに書いていなければ `None`）。
    pub file: Option<PathBuf>,
    /// ファイルに書く設定か（`GENZO_LOG_FILE=off` などで書かないときは `false`）。
    pub file_enabled: bool,
    /// 初期化での問題（ファイルを開けない・水準の指定を解釈できないなど。ログは標準エラーと直前のログに残る）。
    pub problems: Vec<String>,
}

/// 初期化した後の状態（パニックのフックが使う）。
pub(crate) struct LogState {
    pub(crate) program: Program,
    pub(crate) version: String,
    pub(crate) os: String,
    pub(crate) dir: Option<PathBuf>,
    pub(crate) sink: Arc<Sink>,
    pub(crate) reports_written: AtomicUsize,
    status: LogStatus,
}

pub(crate) static STATE: OnceLock<LogState> = OnceLock::new();

/// 指定が `off` か（ファイルに書かない）。
fn is_off(spec: &str) -> bool {
    spec.trim().eq_ignore_ascii_case("off")
}

/// ログを初期化する（標準エラーとファイルの tracing の層・パニックのフック）。プロセスで 1 回だけ呼ぶ
/// （2 回目以降は何もせず、最初の結果を返す）。失敗してもアプリは続ける（結果の `problems` に書く）。
pub fn init(config: LogConfig) -> LogStatus {
    if let Some(state) = STATE.get() {
        return state.status.clone();
    }
    let mut problems = Vec::new();
    let spec = config
        .file_filter
        .clone()
        .or_else(|| std::env::var(ENV_LOG_FILE).ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_FILE_FILTER.to_owned());
    let file_enabled = !is_off(&spec);
    // ファイルに書かないときも、直前のログ（クラッシュレポートに付ける）は既定の水準で集める。
    let file_spec = if file_enabled {
        spec.as_str()
    } else {
        DEFAULT_FILE_FILTER
    };
    let file_filter = EnvFilter::try_new(file_spec).unwrap_or_else(|e| {
        problems.push(format!(
            "ログのファイルの水準の指定 {file_spec:?} を解釈できないため、既定の水準にします: {e}"
        ));
        EnvFilter::new(DEFAULT_FILE_FILTER)
    });
    let stderr_filter = EnvFilter::try_new(&config.stderr_filter).unwrap_or_else(|e| {
        problems.push(format!(
            "ログの水準の指定 {:?} を解釈できないため warn にします: {e}",
            config.stderr_filter
        ));
        EnvFilter::new("warn")
    });
    let file = match (&config.dir, file_enabled) {
        (Some(dir), true) => match RotatingFile::open(
            dir,
            config.program.stem(),
            MAX_LOG_FILE_BYTES,
            KEPT_ROTATED_FILES,
        ) {
            Ok(f) => Some(f),
            Err(e) => {
                problems.push(format!(
                    "ログのファイルを {} に作れません: {e}",
                    dir.display()
                ));
                None
            }
        },
        _ => None,
    };
    if let Some(dir) = &config.dir {
        crash::prune_crash_reports(dir, MAX_KEPT_REPORTS);
    }
    let sink = Arc::new(Sink::new(file, RECENT_LOG_BYTES));
    let status = LogStatus {
        dir: config.dir.clone(),
        file: sink.file_path(),
        file_enabled,
        problems,
    };
    let os = system::describe();
    let state = LogState {
        program: config.program,
        version: config.version.clone(),
        os: os.clone(),
        dir: config.dir.clone(),
        sink: Arc::clone(&sink),
        reports_written: AtomicUsize::new(0),
        status,
    };
    if STATE.set(state).is_err() {
        // ほかのスレッドが同時に初期化した。
        return STATE.get().map(|s| s.status.clone()).unwrap_or_default();
    }
    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(config.stderr_target)
        .with_filter(stderr_filter);
    let file_layer = fmt::layer()
        .with_writer(SinkMakeWriter(Arc::clone(&sink)))
        .with_ansi(false)
        .with_thread_names(true)
        .with_filter(file_filter);
    let installed = tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .try_init();
    crash::install_panic_hook();
    let state = STATE.get().expect("上で設定した");
    let mut status = state.status.clone();
    if let Err(e) = installed {
        status.problems.push(format!(
            "ログの仕組みを初期化できません（既にほかのものが使っている）: {e}"
        ));
    }
    tracing::info!(
        program = config.program.stem(),
        version = %config.version,
        os = %os,
        pid = std::process::id(),
        log_file = ?status.file,
        "起動しました"
    );
    for p in &status.problems {
        tracing::warn!("{p}");
    }
    status
}

/// [`init`] の結果（初期化していなければ `None`）。
pub fn status() -> Option<LogStatus> {
    STATE.get().map(|s| s.status.clone())
}

/// 既定のログのフォルダ（モジュールの doc。環境変数 [`ENV_LOG_DIR`] を優先する）。分からなければ `None`。
pub fn default_log_dir() -> Option<PathBuf> {
    log_dir_from(|k| std::env::var_os(k))
}

/// [`default_log_dir`] の本体（環境変数を引数で受ける。単体テストのため）。
fn log_dir_from(get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(dir) = get(ENV_LOG_DIR).filter(|v| !v.is_empty()) {
        let dir = PathBuf::from(dir);
        return Some(std::path::absolute(&dir).unwrap_or(dir));
    }
    platform_log_dir(&get)
}

#[cfg(windows)]
fn platform_log_dir(get: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let base = PathBuf::from(get("LOCALAPPDATA").filter(|v| !v.is_empty())?);
    base.is_absolute()
        .then(|| base.join(APP_IDENTIFIER).join("logs"))
}

#[cfg(target_os = "macos")]
fn platform_log_dir(get: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let home = PathBuf::from(get("HOME").filter(|v| !v.is_empty())?);
    home.is_absolute()
        .then(|| home.join("Library").join("Logs").join(APP_IDENTIFIER))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_log_dir(get: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    // XDG Base Directory の決まりどおり、相対パスの XDG_DATA_HOME は使わない。
    let data = get("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            let home = PathBuf::from(get("HOME").filter(|v| !v.is_empty())?);
            home.is_absolute()
                .then(|| home.join(".local").join("share"))
        })?;
    Some(data.join(APP_IDENTIFIER).join("logs"))
}

/// GenzoParis のログのファイルの名前か（`app.log`・`app.1.log`・`cli.12.log` など）。
pub fn is_log_file_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".log") else {
        return false;
    };
    let (program, n) = match stem.split_once('.') {
        Some((p, n)) => (p, Some(n)),
        None => (stem, None),
    };
    Program::from_stem(program).is_some()
        && n.is_none_or(|n| {
            !n.is_empty()
                && n.len() <= 2
                && n.bytes().all(|b| b.is_ascii_digit())
                && !n.starts_with('0')
        })
}

/// ログのファイル（[`LogFolderInfo::files`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFileInfo {
    /// ファイル名。
    pub name: String,
    /// 大きさ（バイト）。
    pub bytes: u64,
    /// 最後に書いた日時。
    pub modified: Option<DateTime<Utc>>,
}

/// ログのフォルダの中身（画面の設定・CLI の `log info`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFolderInfo {
    /// ログのフォルダ。
    pub dir: PathBuf,
    /// フォルダがあるか。
    pub exists: bool,
    /// ログのファイル（名前の順）。
    pub files: Vec<LogFileInfo>,
    /// クラッシュレポート（新しい順）。
    pub crash_reports: Vec<CrashReportSummary>,
    /// ログのファイルとクラッシュレポートの合計の大きさ（バイト）。
    pub total_bytes: u64,
    /// このプロセスが今書いているログのファイル。
    pub current_file: Option<PathBuf>,
}

/// ログのフォルダの中身を調べる。
pub fn folder_info(dir: &Path) -> LogFolderInfo {
    let mut files: Vec<LogFileInfo> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let meta = e.metadata().ok().filter(fs::Metadata::is_file)?;
            is_log_file_name(&name).then(|| LogFileInfo {
                name,
                bytes: meta.len(),
                modified: meta.modified().ok().map(DateTime::<Utc>::from),
            })
        })
        .collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    let crash_reports = list_crash_reports(dir);
    let total_bytes = files.iter().map(|f| f.bytes).sum::<u64>()
        + crash_reports
            .iter()
            .filter_map(|r| fs::metadata(&r.path).ok())
            .map(|m| m.len())
            .sum::<u64>();
    LogFolderInfo {
        dir: dir.to_path_buf(),
        exists: dir.is_dir(),
        files,
        crash_reports,
        total_bytes,
        current_file: STATE.get().and_then(|s| s.sink.file_path()),
    }
}

/// [`clear_logs`] の結果。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ClearReport {
    /// 消したファイルの数（ログ・クラッシュレポート）。
    pub removed: u64,
    /// 空にしたファイルの数（このプロセスが書いている途中のログ）。
    pub truncated: u64,
    /// 消せなかったファイルと理由（ほかのプロセスが使っているなど）。
    pub failed: Vec<String>,
}

/// ログのフォルダのログとクラッシュレポートを消す（名前の形が GenzoParis のものだけ。フォルダは残す）。
///
/// このプロセスが書いている途中のログのファイルは、消さずに空にする。クラッシュレポートに付ける直前の
/// ログ（メモリ）も消す。
pub fn clear_logs(dir: &Path) -> ClearReport {
    let mut report = ClearReport::default();
    let state = STATE.get();
    let current = state.and_then(|s| s.sink.file_path());
    for f in folder_info(dir).files {
        let path = dir.join(&f.name);
        let is_current = current
            .as_deref()
            .is_some_and(|c| same_file::is_same_file(c, &path).unwrap_or(false));
        if is_current && let Some(state) = state {
            let truncated = state
                .sink
                .file
                .lock()
                .as_mut()
                .map(writer::RotatingFile::truncate);
            match truncated {
                Some(Ok(())) => report.truncated += 1,
                Some(Err(e)) => report.failed.push(format!("{}: {e}", path.display())),
                None => {}
            }
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => report.removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => report.failed.push(format!("{}: {e}", path.display())),
        }
    }
    for r in list_crash_reports(dir) {
        match fs::remove_file(&r.path) {
            Ok(()) => report.removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => report.failed.push(format!("{}: {e}", r.path.display())),
        }
    }
    if let Some(state) = state {
        state.sink.recent.lock().clear();
    }
    tracing::info!(
        removed = report.removed,
        truncated = report.truncated,
        failed = report.failed.len(),
        "ログとクラッシュレポートを消しました"
    );
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| {
            vars.iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn log_dir_prefers_the_environment_variable() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().to_str().unwrap();
        assert_eq!(
            log_dir_from(env(&[(ENV_LOG_DIR, dir), ("HOME", "/home/u")])),
            Some(d.path().to_path_buf())
        );
        // 空の値は指定なし。
        let fallback = log_dir_from(env(&[(ENV_LOG_DIR, ""), ("HOME", "/home/u")]));
        assert_ne!(fallback, Some(PathBuf::new()));
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn linux_log_dir_follows_xdg() {
        assert_eq!(
            log_dir_from(env(&[("HOME", "/home/u")])),
            Some(PathBuf::from(
                "/home/u/.local/share/io.github.tokamocchi.genzoparis/logs"
            ))
        );
        assert_eq!(
            log_dir_from(env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/data")])),
            Some(PathBuf::from("/data/io.github.tokamocchi.genzoparis/logs"))
        );
        // 相対パスの XDG_DATA_HOME は使わない。HOME もなければ分からない。
        assert_eq!(
            log_dir_from(env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "rel")])),
            Some(PathBuf::from(
                "/home/u/.local/share/io.github.tokamocchi.genzoparis/logs"
            ))
        );
        assert_eq!(log_dir_from(env(&[])), None);
    }

    #[test]
    fn log_file_names_are_recognized_strictly() {
        for ok in ["app.log", "cli.log", "app.1.log", "cli.3.log", "app.12.log"] {
            assert!(is_log_file_name(ok), "{ok}");
        }
        for bad in [
            "app.txt",
            "photo.log",
            "app.0.log",
            "app.01.log",
            "app.123.log",
            "app..log",
            "app.x.log",
            "APP.log",
            "crash.log",
        ] {
            assert!(!is_log_file_name(bad), "{bad}");
        }
    }

    #[test]
    fn folder_info_and_clear_touch_only_genzo_files() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("app.log"), "aaa").unwrap();
        fs::write(d.path().join("cli.1.log"), "bb").unwrap();
        // 利用者のファイル（GENZO_LOG_DIR に別のフォルダを指定した場合など）は数えない・消さない。
        fs::write(d.path().join("notes.log"), "user").unwrap();
        fs::write(d.path().join("photo.jpg"), "user").unwrap();
        let crash = d.path().join(CRASH_DIR);
        fs::create_dir_all(&crash).unwrap();
        fs::write(
            crash.join("crash-20261001T000000000Z-app-1.txt"),
            "内容: x\n",
        )
        .unwrap();
        fs::write(crash.join("readme.txt"), "user").unwrap();

        let info = folder_info(d.path());
        assert!(info.exists);
        assert_eq!(
            info.files
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["app.log", "cli.1.log"]
        );
        assert_eq!(info.crash_reports.len(), 1);
        assert_eq!(info.total_bytes, 3 + 2 + "内容: x\n".len() as u64);

        let report = clear_logs(d.path());
        assert_eq!(report.removed, 3);
        assert!(report.failed.is_empty());
        assert!(!d.path().join("app.log").exists());
        assert!(!d.path().join("cli.1.log").exists());
        assert!(d.path().join("notes.log").exists());
        assert!(d.path().join("photo.jpg").exists());
        assert!(crash.join("readme.txt").exists());
        assert!(folder_info(d.path()).crash_reports.is_empty());

        // フォルダがなくても失敗しない。
        let missing = d.path().join("none");
        assert!(!folder_info(&missing).exists);
        assert_eq!(clear_logs(&missing), ClearReport::default());
    }

    #[test]
    fn off_disables_the_file() {
        assert!(is_off("off"));
        assert!(is_off(" OFF "));
        assert!(!is_off("info"));
    }
}
