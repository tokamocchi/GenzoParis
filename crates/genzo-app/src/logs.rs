//! アプリのログとクラッシュレポート（SYS-07。外部へは送信しない）。
//!
//! - 起動の最初に [`init`] でログを初期化する（genzo-api の `logging`。ログのフォルダの `app.log` と標準
//!   エラー。パニックのフックでクラッシュレポートを保存する）。
//! - 画面には、ログのフォルダ・今のファイル・初期化の問題（[`AppLogs::status`]）と、前回までに保存した
//!   アプリのクラッシュレポートのうち、まだ知らせていないもの（[`AppLogs::take_crash_reports`]。起動して
//!   最初の 1 回だけ）を返す。画面は、起動時に知らせのダイアログを出す（送信はしない）。
//! - 設定の画面から、ログのフォルダを開く・中身を見る・消す（[`AppLogs::folder_info`]・
//!   [`AppLogs::clear`]）。
//! - 画面（WebView）の JavaScript の例外も、アプリのログに残す（[`AppLogs::record_ui_error`]。1 回の起動で
//!   [`MAX_UI_ERRORS`] 件まで）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use genzo_api::logging::{
    self, ClearReport, CrashReportSummary, LogConfig, LogFolderInfo, LogStatus, Program,
};
use genzo_api::{ErrorInfo, ErrorKind};

use crate::state::error;

/// 画面のエラーを記録する数の上限（1 回の起動で。**仮置き**。同じ例外が繰り返してもログを埋めないため）。
pub const MAX_UI_ERRORS: u32 = 100;
/// 画面のエラーの文字列の長さの上限（バイト）。
const MAX_UI_ERROR_BYTES: usize = 4096;

/// ログを初期化する（`run` の最初に 1 回）。標準エラーの水準は環境変数 `GENZO_LOG`（なければ warn）。
pub fn init() -> LogStatus {
    let stderr_filter = std::env::var("GENZO_LOG")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "warn".to_owned());
    let dir = logging::default_log_dir();
    let mut status = logging::init(LogConfig {
        program: Program::App,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        dir: dir.clone(),
        file_filter: None,
        stderr_filter,
        stderr_target: true,
    });
    if dir.is_none() {
        status
            .problems
            .push("ログのフォルダが分からないため、ログをファイルに残しません".to_owned());
    }
    status
}

/// アプリのログの状態。
#[derive(Debug, Default)]
pub struct AppLogs {
    status: LogStatus,
    crash_notice_taken: AtomicBool,
    ui_errors: AtomicU32,
}

impl AppLogs {
    /// 初期化の結果から作る。
    pub fn new(status: LogStatus) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }

    /// 初期化の結果（ログのフォルダ・今のファイル・問題）。
    pub fn status(&self) -> &LogStatus {
        &self.status
    }

    /// ログのフォルダ（分からなければエラー）。
    pub fn dir(&self) -> Result<&Path, ErrorInfo> {
        self.status.dir.as_deref().ok_or_else(|| {
            error(
                ErrorKind::NotFound,
                "ログのフォルダが分かりません",
                Some("環境変数 GENZO_LOG_DIR でフォルダを指定して、アプリを起動し直してください"),
            )
        })
    }

    /// 前回までのアプリのクラッシュレポートのうち、まだ知らせていないもの（新しい順）。起動して最初の
    /// 1 回だけ返し、2 回目からは空（画面を読み込み直しても、2 回知らせない）。
    pub fn take_crash_reports(&self) -> Vec<CrashReportSummary> {
        if self.crash_notice_taken.swap(true, Ordering::SeqCst) {
            return Vec::new();
        }
        match &self.status.dir {
            Some(dir) => logging::take_unseen_crash_reports(dir, Program::App),
            None => Vec::new(),
        }
    }

    /// ログのフォルダの中身。
    pub fn folder_info(&self) -> Result<LogFolderInfo, ErrorInfo> {
        Ok(logging::folder_info(self.dir()?))
    }

    /// ログのフォルダを作る（開く前に。まだ何も書いていないとフォルダがないことがある。Unix では利用者だけが
    /// 読める権限で作る。genzo-api の `logging::create_log_dir`）。
    pub fn ensure_dir(&self) -> Result<PathBuf, ErrorInfo> {
        let dir = self.dir()?;
        logging::create_log_dir(dir).map_err(|e| {
            error(
                ErrorKind::FileAccess,
                format!("ログのフォルダ {} を作れません: {e}", dir.display()),
                None,
            )
        })?;
        Ok(dir.to_path_buf())
    }

    /// ログとクラッシュレポートを消す（genzo-api の `logging::clear_logs`）。
    pub fn clear(&self) -> Result<ClearReport, ErrorInfo> {
        Ok(logging::clear_logs(self.dir()?))
    }

    /// 画面のエラー（JavaScript の例外など）をログに残す。上限を超えたら残さずに `false`（上限に達したときは
    /// 1 回だけ知らせる）。
    pub fn record_ui_error(&self, message: &str, detail: Option<&str>) -> bool {
        let n = self.ui_errors.fetch_add(1, Ordering::SeqCst) + 1;
        if n > MAX_UI_ERRORS {
            if n == MAX_UI_ERRORS + 1 {
                tracing::warn!(
                    target: "genzo_ui",
                    "画面のエラーが {MAX_UI_ERRORS} 件を超えたので、以降はログに残しません"
                );
            }
            return false;
        }
        let message = clip(message);
        let detail = detail.map(clip).unwrap_or_default();
        tracing::error!(target: "genzo_ui", detail = %detail, "画面のエラー: {message}");
        true
    }
}

/// 長い文字列を [`MAX_UI_ERROR_BYTES`] までにする（文字の境目で）。
fn clip(s: &str) -> String {
    if s.len() <= MAX_UI_ERROR_BYTES {
        return s.to_owned();
    }
    let mut cut = MAX_UI_ERROR_BYTES;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_folder_uses_the_app_identifier() {
        // ログのフォルダの名前（genzo-api の APP_IDENTIFIER）は、Tauri の識別子（Tauri の app_log_dir と
        // 同じ場所）と同じ。
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], logging::APP_IDENTIFIER);
    }

    #[test]
    fn crash_reports_are_taken_only_once() {
        let d = tempfile::tempdir().unwrap();
        let crash = d.path().join(logging::CRASH_DIR);
        std::fs::create_dir_all(&crash).unwrap();
        let other_pid = std::process::id().wrapping_add(1);
        let name = format!("crash-20261001T000000000Z-app-{other_pid}.txt");
        std::fs::write(crash.join(&name), "内容: テスト\n").unwrap();
        let logs = AppLogs::new(LogStatus {
            dir: Some(d.path().to_path_buf()),
            ..Default::default()
        });
        let first = logs.take_crash_reports();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].file_name, name);
        assert_eq!(first[0].message.as_deref(), Some("テスト"));
        assert!(logs.take_crash_reports().is_empty());
        // 次の起動（新しい AppLogs）でも、知らせたものは知らせない。
        let next = AppLogs::new(LogStatus {
            dir: Some(d.path().to_path_buf()),
            ..Default::default()
        });
        assert!(next.take_crash_reports().is_empty());
    }

    #[test]
    fn without_a_log_dir_the_folder_operations_fail_clearly() {
        let logs = AppLogs::default();
        assert!(logs.take_crash_reports().is_empty());
        assert_eq!(logs.folder_info().unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(logs.clear().unwrap_err().kind, ErrorKind::NotFound);
    }

    #[test]
    fn folder_info_and_clear_use_the_log_dir() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("logs");
        let logs = AppLogs::new(LogStatus {
            dir: Some(dir.clone()),
            ..Default::default()
        });
        assert!(!logs.folder_info().unwrap().exists);
        assert_eq!(logs.ensure_dir().unwrap(), dir);
        std::fs::write(dir.join("app.1.log"), "古いログ").unwrap();
        let info = logs.folder_info().unwrap();
        assert!(info.exists);
        assert_eq!(info.files.len(), 1);
        assert_eq!(logs.clear().unwrap().removed, 1);
        assert!(!dir.join("app.1.log").exists());
    }

    #[test]
    fn ui_errors_are_limited_and_clipped() {
        let logs = AppLogs::default();
        for _ in 0..MAX_UI_ERRORS {
            assert!(logs.record_ui_error("TypeError: x is undefined", Some("at f (app.js:1)")));
        }
        assert!(!logs.record_ui_error("もう 1 件", None));
        let long = "あ".repeat(3000);
        let clipped = clip(&long);
        assert!(clipped.len() <= MAX_UI_ERROR_BYTES + '…'.len_utf8());
        assert!(clipped.ends_with('…'));
        assert_eq!(clip("短い"), "短い");
    }
}
