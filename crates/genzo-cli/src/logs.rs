//! `log` のサブコマンド: ログのフォルダ・ファイル・クラッシュレポートの表示（`info`）と消去（`clear`）
//! （SYS-07。カタログは使わない。ログ・クラッシュレポートは外部へは送信しない）。
//!
//! 対象のフォルダは `--log-dir`、環境変数 `GENZO_LOG_DIR`、なければアプリと同じログのフォルダ
//! （genzo-api の `logging::default_log_dir`）。アプリのログ（`app.log`）とクラッシュレポートも含む。

use genzo_api::ErrorKind;
use genzo_api::logging::{self, LogFolderInfo};
use serde_json::json;

use crate::Status;
use crate::args::{GlobalArgs, LogCommand};
use crate::error::{CliError, CliResult};
use crate::output::{Output, Table, key_values};

/// `log` を実行する。
pub fn run(g: &GlobalArgs, out: Output, cmd: LogCommand) -> CliResult<Status> {
    let Some(dir) = crate::log_dir(g) else {
        return Err(CliError::classified(
            ErrorKind::NotFound,
            "ログのフォルダが分かりません",
            Some("--log-dir か環境変数 GENZO_LOG_DIR でフォルダを指定してください"),
        ));
    };
    match cmd {
        LogCommand::Info => {
            let info = logging::folder_info(&dir);
            print_info(out, &info);
            Ok(Status::Success)
        }
        LogCommand::Clear => {
            let r = logging::clear_logs(&dir);
            if out.json {
                out.print_json(&json!({ "dir": dir, "cleared": r }));
            } else {
                out.line(format!(
                    "消しました: {} 個（書いている途中のログを空にしたもの {} 個を含む）。フォルダ: {}",
                    r.removed + r.truncated,
                    r.truncated,
                    dir.display()
                ));
                for f in &r.failed {
                    out.warn(format!("消せませんでした: {f}"));
                }
            }
            Ok(if r.failed.is_empty() {
                Status::Success
            } else {
                Status::Partial
            })
        }
    }
}

fn print_info(out: Output, info: &LogFolderInfo) {
    let status = logging::status();
    if out.json {
        out.print_json(&json!({
            "dir": info.dir,
            "exists": info.exists,
            "current_file": info.current_file,
            "file_enabled": status.as_ref().map(|s| s.file_enabled),
            "files": info.files,
            "crash_reports": info.crash_reports,
            "total_bytes": info.total_bytes,
        }));
        return;
    }
    let mut items = vec![
        ("ログのフォルダ", info.dir.display().to_string()),
        (
            "このコマンドのログ",
            match (&info.current_file, status.as_ref().map(|s| s.file_enabled)) {
                (Some(f), _) => f.display().to_string(),
                (None, Some(false)) => {
                    "ファイルに書いていません（--no-log-file / GENZO_LOG_FILE=off）".to_owned()
                }
                (None, _) => "ファイルに書いていません".to_owned(),
            },
        ),
        ("合計", format!("{} バイト", info.total_bytes)),
    ];
    if !info.exists {
        items.push(("状態", "まだログはありません".to_owned()));
    }
    out.line(key_values(&items).trim_end());
    if !info.files.is_empty() {
        let mut t = Table::new(["ファイル", "バイト", "最後に書いた日時（UTC）"]);
        for f in &info.files {
            t.row([
                f.name.clone(),
                f.bytes.to_string(),
                f.modified
                    .map(|m| m.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_default(),
            ]);
        }
        out.table(&t);
    }
    if !info.crash_reports.is_empty() {
        out.line(format!(
            "クラッシュレポート（{} 件）:",
            info.crash_reports.len()
        ));
        for r in &info.crash_reports {
            out.line(format!(
                "  {}  {}",
                r.file_name,
                r.message.as_deref().unwrap_or("")
            ));
        }
    }
    out.line(
        "ログとクラッシュレポートは、この PC の中だけに保存しています（外部へは送信しません）。",
    );
}
