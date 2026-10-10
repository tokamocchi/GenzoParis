//! `catalog` のサブコマンド: 作成・情報・詳細チェック・バックアップ・復元（SYS-04、DATA-04・05）。

use std::path::Path;

use genzo_api::{ApiError, ErrorKind, JobResult, PreviousShutdown, SearchFilter, SearchSort};
use serde_json::json;

use crate::Status;
use crate::args::{CatalogCommand, GlobalArgs};
use crate::error::{CliError, CliResult};
use crate::output::{Output, color_space_text, key_values, utc_offset_text};
use crate::session::{OpenOptions, Session, catalog_path};

/// `catalog` を実行する。
pub fn run(g: &GlobalArgs, out: Output, cmd: CatalogCommand) -> CliResult<Status> {
    match cmd {
        CatalogCommand::Init => init(g, out),
        CatalogCommand::Info => info(g, out),
        CatalogCommand::Check { files } => check(g, out, files),
        CatalogCommand::Backup { to } => backup(g, out, to),
        CatalogCommand::Restore { backup, to } => restore(out, &backup, &to),
    }
}

fn shutdown_text(s: PreviousShutdown) -> &'static str {
    match s {
        PreviousShutdown::FirstOpen => "初めて開いた",
        PreviousShutdown::Clean => "正常に終了した",
        PreviousShutdown::Unclean => "正常に終了しなかった（catalog check をおすすめします）",
    }
}

fn init(g: &GlobalArgs, out: Output) -> CliResult<Status> {
    let path = catalog_path(g)?;
    if path.exists() {
        return Err(CliError::classified(
            ErrorKind::Conflict,
            format!("{} は既にあります", path.display()),
            Some(
                "新しいカタログは、まだないファイルのパスを指定して作ってください（既存のファイルは変更しません）",
            ),
        ));
    }
    let s = Session::open(
        g,
        out,
        &OpenOptions {
            create: true,
            ..Default::default()
        },
    )?;
    let stats = s.core.catalog_stats()?;
    let data_dir = s.core.config().data_dir.clone();
    s.close()?;
    if out.json {
        out.print_json(&json!({
            "catalog": path,
            "data_dir": data_dir,
            "schema_version": stats.schema_version,
        }));
    } else {
        out.line(format!(
            "カタログを作りました: {}（スキーマの版 {}、データのフォルダ {}）",
            path.display(),
            stats.schema_version,
            data_dir.display()
        ));
    }
    Ok(Status::Success)
}

fn info(g: &GlobalArgs, out: Output) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let stats = s.core.catalog_stats()?;
    let startup = s.core.startup_report();
    let backups = s.core.list_backups()?;
    let settings = s.core.settings();
    let config = s.core.config().clone();
    s.close()?;
    if out.json {
        out.print_json(&json!({
            "catalog": config.catalog_path,
            "data_dir": config.data_dir,
            "schema_version": stats.schema_version,
            "counts": {
                "assets": stats.assets,
                "variants": stats.variants,
                "files": stats.files,
                "history_entries": stats.history_entries,
            },
            "previous_shutdown": startup.previous_shutdown,
            "migrated_from": startup.migrated_from,
            "recovered_file_ops": startup.recovered_file_ops,
            "startup_backup": startup.backup,
            "backups": backups,
            "settings": settings,
        }));
        return Ok(Status::Success);
    }
    let mut items = vec![
        ("カタログ", config.catalog_path.display().to_string()),
        ("データのフォルダ", config.data_dir.display().to_string()),
        ("スキーマの版", stats.schema_version.to_string()),
        ("asset", stats.assets.to_string()),
        ("variant", stats.variants.to_string()),
        ("ファイル", stats.files.to_string()),
        ("現像の履歴", stats.history_entries.to_string()),
        (
            "前回の終了",
            shutdown_text(startup.previous_shutdown).to_owned(),
        ),
        (
            "既定のタイムゾーン",
            utc_offset_text(settings.default_utc_offset_minutes),
        ),
        (
            "書き出しの既定の色空間",
            color_space_text(settings.default_export_color_space).to_owned(),
        ),
    ];
    if let Some(v) = startup.migrated_from {
        items.push(("スキーマの移行", format!("版 {v} から移行しました")));
    }
    if startup.recovered_file_ops > 0 {
        items.push((
            "確定したファイル操作",
            startup.recovered_file_ops.to_string(),
        ));
    }
    items.push(("バックアップ", format!("{} 件", backups.len())));
    out.line(key_values(&items).trim_end());
    for b in &backups {
        out.line(format!(
            "  {}  {}",
            b.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
            b.path.display()
        ));
    }
    Ok(Status::Success)
}

fn check(g: &GlobalArgs, out: Output, files: bool) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let job = s.core.check_integrity()?;
    let info = s.wait_job_ok(job, "詳細チェック")?;
    let Some(JobResult::IntegrityCheck(report)) = info.result else {
        return Err(CliError::other("詳細チェックの結果がありません"));
    };
    let file_report = if files {
        let r = s
            .core
            .search(&SearchFilter::default(), SearchSort::default())?;
        let ids = s.core.result_ids(r.generation)?;
        Some(s.core.check_files(&ids)?)
    } else {
        None
    };
    s.close()?;
    let ok = report.ok && file_report.as_ref().is_none_or(|f| f.missing.is_empty());
    if out.json {
        out.print_json(&json!({
            "ok": ok,
            "integrity": report,
            "files": file_report,
        }));
    } else {
        if report.ok {
            out.line("カタログの詳細チェック: 問題は見つかりませんでした");
        } else {
            out.line("カタログの詳細チェック: 問題が見つかりました（バックアップからの復元を検討してください）");
            for e in &report.integrity_errors {
                out.line(format!("  整合性: {e}"));
            }
            for e in &report.foreign_key_violations {
                out.line(format!("  外部キー: {e}"));
            }
            if let Some(e) = &report.fts_error {
                out.line(format!("  テキスト検索の索引: {e}"));
            }
        }
        if let Some(f) = &file_report {
            out.line(format!(
                "元ファイル: {} 件を確認、{} 件が見つかり、{} 件が見つかりません、{} 件の内容の変化を検知",
                f.checked,
                f.ok,
                f.missing.len(),
                f.changed.len()
            ));
            for m in &f.missing {
                out.line(format!("  見つかりません: {}", m.path.display()));
            }
        }
    }
    Ok(if ok { Status::Success } else { Status::Failure })
}

fn backup(g: &GlobalArgs, out: Output, to: Option<std::path::PathBuf>) -> CliResult<Status> {
    let s = Session::open(
        g,
        out,
        &OpenOptions {
            backup_dir: to,
            ..Default::default()
        },
    )?;
    // 開いたときの自動バックアップ（DATA-04。前回から 1 日以上たっていれば作る）が今作ったものなら、
    // それを結果にする（同じ内容のバックアップを続けて 2 つ作らない）。
    let entry = match s.core.startup_report().backup {
        Some(b) => b,
        None => s.core.backup_now()?,
    };
    s.close()?;
    if out.json {
        out.print_json(&entry);
    } else {
        out.line(format!(
            "バックアップを作りました: {}（{}）",
            entry.path.display(),
            entry.created_at.format("%Y-%m-%d %H:%M:%S UTC")
        ));
    }
    Ok(Status::Success)
}

/// SQLite のファイルの先頭（16 バイト）。
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// ファイルが GenzoParis のカタログか（SQLite の先頭と、オフセット 68 の application_id で確かめる。
/// ファイルを開いてデータベースとしては扱わない（ほかのプロセスが使っていても変えないため））。
pub fn is_genzo_catalog(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut head = [0u8; 72];
    let mut f = std::fs::File::open(path)?;
    let mut read = 0;
    while read < head.len() {
        match f.read(&mut head[read..])? {
            0 => break,
            n => read += n,
        }
    }
    if read < head.len() || &head[..16] != SQLITE_HEADER {
        return Ok(false);
    }
    // SQLite のヘッダーのオフセット 68 は application_id（ビッグエンディアンの 32 bit 整数）。
    let id = i32::from_be_bytes([head[68], head[69], head[70], head[71]]);
    Ok(id == genzo_catalog::CATALOG_APPLICATION_ID)
}

/// 復元先（`--to`）を確かめる: 復元先のフォルダがあること、既にあるファイルは GenzoParis の
/// カタログであること（写真など、ほかのファイルを「退避」の名前に変えてしまわないため）。
fn check_restore_target(to: &Path) -> CliResult<()> {
    let parent = to
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(CliError::classified(
            ErrorKind::FileAccess,
            format!("復元先のフォルダ {} がありません", parent.display()),
            Some("先にフォルダを作るか、既にあるフォルダの中のパスを指定してください"),
        ));
    }
    if !to.exists() {
        return Ok(());
    }
    let ok = to.is_file() && is_genzo_catalog(to).map_err(|e| CliError::io(to, e))?;
    if ok {
        Ok(())
    } else {
        Err(CliError::classified(
            ErrorKind::InvalidInput,
            format!(
                "復元先 {} は GenzoParis のカタログではありません（変更していません）",
                to.display()
            ),
            Some("--to には、置き換えるカタログか、まだないファイルのパスを指定してください"),
        ))
    }
}

fn restore(out: Output, backup: &Path, to: &Path) -> CliResult<Status> {
    if !backup.is_file() {
        return Err(CliError::Api(ApiError::NotFound(format!(
            "バックアップ {}",
            backup.display()
        ))));
    }
    check_restore_target(to)?;
    let plan = genzo_api::prepare_restore(backup, to).map_err(|e| {
        // バックアップが壊れている・カタログでない場合の genzo-api の案内（「バックアップから復元して
        // ください」）は、復元の場面では合わないため置き換える。
        if e.kind() == ErrorKind::CatalogCorrupt {
            CliError::with_hint(
                e,
                "バックアップのファイルを確かめてください（カタログ・既存のファイルは変更していません）",
            )
        } else if e.kind() == ErrorKind::IncompatibleVersion {
            CliError::with_hint(
                e,
                "新しい版のアプリで作られたバックアップです。アプリを更新してから復元してください（カタログ・既存のファイルは変更していません）",
            )
        } else {
            CliError::from(e)
        }
    })?;
    // 退避したファイル（カタログの本体と、付随する -wal・-shm など。本体がなくても -wal だけを退避する
    // ことがある）。差し替え（名前の変更）の途中では止めない（1 回目の Ctrl+C は、差し替えを終えてから
    // 終了する。K3）。
    let displaced = {
        let _graceful = crate::interrupt::Graceful::begin();
        genzo_api::apply_restore(&plan)
    }
    .map_err(|e| {
        CliError::with_hint(
            e,
            format!(
                "復元したファイル {} は残っています。カタログを開いているアプリを終了してから、やり直してください",
                plan.restored_path.display()
            ),
        )
    })?;
    if out.json {
        out.print_json(&json!({
            "backup": plan.backup,
            "catalog": plan.catalog_path,
            "displaced": displaced
                .iter()
                .any(|p| p == &plan.displaced_path)
                .then_some(&plan.displaced_path),
            "displaced_files": &displaced,
        }));
    } else {
        out.line(format!(
            "バックアップ {} を検証し、{} に復元しました",
            backup.display(),
            to.display()
        ));
        if displaced.iter().any(|p| p == &plan.displaced_path) {
            out.line(format!(
                "元のカタログは {} に退避しました（確かめてから削除してください）",
                plan.displaced_path.display()
            ));
        }
        for p in displaced.iter().filter(|p| **p != plan.displaced_path) {
            out.line(format!(
                "復元先に残っていた付随するファイルを {} に退避しました（確かめてから削除してください）",
                p.display()
            ));
        }
    }
    Ok(Status::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_files_are_identified_by_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.db");
        std::fs::write(&p, b"").unwrap();
        assert!(!is_genzo_catalog(&p).unwrap(), "空のファイル");
        let mut head = Vec::from(&SQLITE_HEADER[..]);
        head.resize(100, 0);
        head[68..72].copy_from_slice(&genzo_catalog::CATALOG_APPLICATION_ID.to_be_bytes());
        std::fs::write(&p, &head).unwrap();
        assert!(is_genzo_catalog(&p).unwrap());
        head[71] ^= 1;
        std::fs::write(&p, &head).unwrap();
        assert!(!is_genzo_catalog(&p).unwrap(), "別のアプリの SQLite");
        std::fs::write(&p, b"\xff\xd8\xff\xe0 jpeg").unwrap();
        assert!(!is_genzo_catalog(&p).unwrap());
    }

    #[test]
    fn restore_target_must_be_a_catalog_or_absent() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("A.jpg");
        std::fs::write(&photo, b"\xff\xd8 photo").unwrap();
        let e = check_restore_target(&photo).unwrap_err();
        assert_eq!(e.to_json()["error"]["kind"], "invalid_input");
        assert_eq!(std::fs::read(&photo).unwrap(), b"\xff\xd8 photo");
        assert!(check_restore_target(dir.path()).is_err(), "フォルダ");
        assert!(check_restore_target(&dir.path().join("new.db")).is_ok());
        let e = check_restore_target(&dir.path().join("nodir").join("c.db")).unwrap_err();
        assert_eq!(e.to_json()["error"]["kind"], "file_access");
    }
}
