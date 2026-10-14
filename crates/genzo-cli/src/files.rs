//! ボリューム（FILE-03）・アプリの中でのファイル操作（FILE-02）・見つからない写真（FILE-04）のコマンド
//! （`volume`・`folder`・`file`・`missing`）。
//!
//! - どれも genzo-api の操作（UI と同じ経路）を呼ぶ。ディスクのファイル・フォルダを変えるもの（`file move`・
//!   `file rename`・`folder create / rename / move`）は、コアが意図を先に記録し（file_op）、上書きせず、途中で
//!   失敗した写真は元に戻す。別のドライブへの移動はコピーして照合してから登録し、元のファイルはゴミ箱へ移す。
//!   `folder remove`・`folder relocate` はカタログの中だけを変える（除けるのは写真が登録されていないフォルダ）。
//! - フォルダは ID（`folder list`）か、登録したフォルダのパスで指定する（[`crate::args::FolderArg`]）。
//! - `missing relink`: 確からしい候補（内容が同じ・大きさと先頭と末尾が同じ）が 1 つだけのファイルを結び付ける
//!   （UI の既定の選択と同じ。ui/src/library/fileOpsLogic.ts の defaultRelinkChoice）。名前と撮影日時だけが同じ
//!   （内容が変わっている）候補は `--accept-changed` のとき、それしか候補がなければ結び付ける。
//! - 終了コード: 一部の写真を移せなかった・名前を変えられなかった・結び付けられなかった、ゴミ箱へ移せずに元の
//!   ファイルが残った、`missing check` で見つからない写真があったときは 3（[`crate::EXIT_PARTIAL`]）。

use genzo_api::{
    FileMovePlan, FileMoveReport, FolderInfo, JobResult, MissingCheckReport, MoveMethod,
    PlannedPhotoMove, RelinkCandidate, RelinkMatch, RelinkRequest, RelinkSearch, RelinkTarget,
    RelocatePlan, RenameRule, RenameSpec,
};
use genzo_model::FolderId;

use crate::Status;
use crate::args::{
    FileCommand, FolderArg, FolderCommand, GlobalArgs, MissingCommand, VolumeCommand,
};
use crate::error::{CliError, CliResult};
use crate::output::{Output, Table, json_value, one_line};
use crate::session::{OpenOptions, Session};

/// 予定・結果の一覧に出す写真の数の上限（人が読む表。JSON はすべて）。
const LIST_LIMIT: usize = 200;

/// `volume`。
pub fn volume(g: &GlobalArgs, out: Output, cmd: VolumeCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = (|| {
        let VolumeCommand::List { refresh } = cmd;
        let list = if refresh {
            s.core.refresh_volumes()?
        } else {
            s.core.volumes()?
        };
        if out.json {
            out.print_json(&json_value(&list));
            return Ok(Status::Success);
        }
        let mut t = Table::new([
            "ID",
            "状態",
            "識別",
            "ラベル",
            "マウント先",
            "形式",
            "ボリューム ID",
        ]);
        for v in &list {
            t.row([
                v.volume_id.to_string(),
                if v.online {
                    "つながっている"
                } else {
                    "つながっていない"
                }
                .to_owned(),
                if v.network {
                    "ネットワーク".to_owned()
                } else if v.identified {
                    "OS のボリューム ID".to_owned()
                } else {
                    "パス".to_owned()
                },
                v.label.clone().unwrap_or_default(),
                v.mount_path
                    .as_deref()
                    .map_or_else(String::new, |p| p.display().to_string()),
                v.fs_type.clone().unwrap_or_default(),
                v.uuid.clone(),
            ]);
        }
        out.table(&t);
        Ok(Status::Success)
    })();
    s.close()?;
    r
}

/// `folder`。
pub fn folder(g: &GlobalArgs, out: Output, cmd: FolderCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = folder_in(&s, out, cmd);
    s.close()?;
    r
}

fn folder_in(s: &Session, out: Output, cmd: FolderCommand) -> CliResult<Status> {
    match cmd {
        FolderCommand::List => {
            let list = s.core.folders()?;
            if out.json {
                out.print_json(&json_value(&list));
                return Ok(Status::Success);
            }
            let mut t = Table::new(["ID", "状態", "見つからない写真", "パス"]);
            for f in &list {
                t.row([
                    f.folder_id.to_string(),
                    folder_state_text(f).to_owned(),
                    if f.missing_files > 0 {
                        f.missing_files.to_string()
                    } else {
                        String::new()
                    },
                    folder_path_text(f),
                ]);
            }
            out.table(&t);
            Ok(Status::Success)
        }
        FolderCommand::Create { parent, name } => {
            let parent = resolve_folder(s, &parent)?;
            let f = s.core.create_folder(parent, &name)?;
            print_folder(out, &f, "フォルダを作りました");
            Ok(Status::Success)
        }
        FolderCommand::Rename { folder, name } => {
            let folder = resolve_folder(s, &folder)?;
            let f = s.core.rename_folder(folder, &name)?;
            print_folder(out, &f, "フォルダの名前を変えました");
            Ok(Status::Success)
        }
        FolderCommand::Move { folder, parent } => {
            let folder = resolve_folder(s, &folder)?;
            let parent = resolve_folder(s, &parent)?;
            let f = s.core.move_folder(folder, parent)?;
            print_folder(out, &f, "フォルダを移しました");
            Ok(Status::Success)
        }
        FolderCommand::Remove { folder } => {
            let id = resolve_folder(s, &folder)?;
            let n = s.core.remove_folder(id)?;
            if out.json {
                out.print_json(&serde_json::json!({ "folder_id": id, "removed_folders": n }));
            } else {
                out.line(format!(
                    "フォルダ {id} をカタログから除きました（配下を含めて {n} 個。ディスクのフォルダはそのまま）"
                ));
            }
            Ok(Status::Success)
        }
        FolderCommand::Relocate {
            folder,
            path,
            dry_run,
        } => {
            let folder = resolve_folder(s, &folder)?;
            let path = std::path::absolute(&path).unwrap_or(path);
            let plan = if dry_run {
                s.core.plan_relocate_folder(folder, &path)?
            } else {
                s.core.relocate_folder(folder, &path)?
            };
            if out.json {
                out.print_json(
                    &serde_json::json!({ "dry_run": dry_run, "plan": json_value(&plan) }),
                );
            } else {
                print_relocate(out, &plan, dry_run);
            }
            Ok(Status::Success)
        }
    }
}

/// `file`。
pub fn file(g: &GlobalArgs, out: Output, cmd: FileCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = file_in(&s, out, cmd);
    s.close()?;
    r
}

fn file_in(s: &Session, out: Output, cmd: FileCommand) -> CliResult<Status> {
    match cmd {
        FileCommand::Move {
            variants,
            to,
            dry_run,
        } => {
            let dest = resolve_folder(s, &to)?;
            if dry_run {
                let plan = s.core.plan_move_files(&variants, dest)?;
                if out.json {
                    out.print_json(&json_value(&plan));
                } else {
                    print_move_plan(out, &plan);
                }
                return Ok(Status::Success);
            }
            let job = s.core.move_files(&variants, dest)?;
            let info = s.wait_job_ok(job, "写真の移動")?;
            let Some(JobResult::MoveFiles(r)) = info.result else {
                return Err(CliError::other("写真の移動の結果がありません"));
            };
            print_move_report(out, &r, "を移しました");
            Ok(move_status(&r))
        }
        FileCommand::Rename {
            variants,
            name,
            template,
            text,
            seq_start,
            seq_digits,
            dry_run,
        } => {
            let spec = match (name, template) {
                (Some(name), _) => RenameSpec::Name { name },
                (None, Some(template)) => RenameSpec::Template {
                    rule: RenameRule {
                        template,
                        custom_text: text,
                        sequence_start: seq_start,
                        sequence_digits: seq_digits,
                    },
                },
                (None, None) => {
                    return Err(CliError::Usage(
                        "--name か --template を指定してください".to_owned(),
                    ));
                }
            };
            if dry_run {
                let plan = s.core.plan_rename_files(&variants, &spec)?;
                if out.json {
                    out.print_json(&json_value(&plan));
                } else {
                    out.line(format!(
                        "{} 枚の写真の名前を変えます（{} 枚は変えられません）",
                        plan.renamable,
                        plan.photos.len() as u64 - plan.renamable
                    ));
                    out.table(&photos_table(&plan.photos, false));
                }
                return Ok(Status::Success);
            }
            let r = s.core.rename_files(&variants, &spec)?;
            print_move_report(out, &r, "の名前を変えました");
            Ok(move_status(&r))
        }
    }
}

/// `missing`。
pub fn missing(g: &GlobalArgs, out: Output, cmd: MissingCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = missing_in(&s, out, cmd);
    s.close()?;
    r
}

fn missing_in(s: &Session, out: Output, cmd: MissingCommand) -> CliResult<Status> {
    match cmd {
        MissingCommand::Check { folder } => {
            let folder = folder.map(|f| resolve_folder(s, &f)).transpose()?;
            let job = s.core.check_missing(folder)?;
            let info = s.wait_job_ok(job, "見つからない写真の確認")?;
            let Some(JobResult::CheckMissing(r)) = info.result else {
                return Err(CliError::other("見つからない写真の確認の結果がありません"));
            };
            if out.json {
                out.print_json(&json_value(&r));
            } else {
                print_check(out, &r);
            }
            Ok(if r.missing > 0 || r.cancelled {
                Status::Partial
            } else {
                Status::Success
            })
        }
        MissingCommand::Relink {
            dir,
            variants,
            no_recursive,
            accept_changed,
            dry_run,
        } => {
            let dir = std::path::absolute(&dir).unwrap_or(dir);
            let ids = (!variants.is_empty()).then_some(variants);
            let job = s.core.find_relink_candidates(ids, &dir, !no_recursive)?;
            let info = s.wait_job_ok(job, "再リンクの候補の探索")?;
            let Some(JobResult::RelinkCandidates(search)) = info.result else {
                return Err(CliError::other("再リンクの候補の探索の結果がありません"));
            };
            let chosen: Vec<RelinkRequest> = search
                .targets
                .iter()
                .filter_map(|t| {
                    default_choice(t, accept_changed).map(|c| RelinkRequest {
                        file_id: t.file_id,
                        path: c.path.clone(),
                        accept_changed: c.matched == RelinkMatch::NameAndTime,
                    })
                })
                .collect();
            let report = if dry_run || chosen.is_empty() {
                None
            } else {
                Some(s.core.relink_files(&chosen)?)
            };
            if out.json {
                out.print_json(&serde_json::json!({
                    "dry_run": dry_run,
                    "search": json_value(&search),
                    "chosen": json_value(&chosen),
                    "report": json_value(&report),
                }));
            } else {
                print_relink(out, &search, &chosen, dry_run);
                if let Some(r) = &report {
                    out.line(format!(
                        "{} 個のファイルを結び付け直しました",
                        r.linked.len()
                    ));
                    if !r.reread.is_empty() {
                        out.line(format!(
                            "  内容が変わっていたので読み直したファイル: {} 個",
                            r.reread.len()
                        ));
                    }
                    for f in &r.failed {
                        out.line(format!(
                            "  結び付けられなかったファイル: {}: {}",
                            f.path.display(),
                            one_line(&f.reason)
                        ));
                    }
                }
            }
            let failed = report.as_ref().is_some_and(|r| !r.failed.is_empty());
            Ok(if failed || search.cancelled {
                Status::Partial
            } else {
                Status::Success
            })
        }
        MissingCommand::Link {
            file,
            path,
            accept_changed,
        } => {
            let path = std::path::absolute(&path).unwrap_or(path);
            let r = s.core.relink_files(&[RelinkRequest {
                file_id: file,
                path,
                accept_changed,
            }])?;
            if out.json {
                out.print_json(&json_value(&r));
            } else {
                for l in &r.linked {
                    out.line(format!(
                        "結び付け直しました: {} → {}",
                        l.from.display(),
                        l.to.display()
                    ));
                }
                if !r.reread.is_empty() {
                    out.line("  内容が変わっていたので読み直しました");
                }
                for f in &r.failed {
                    out.line(format!(
                        "結び付けられません: {}: {}",
                        f.path.display(),
                        one_line(&f.reason)
                    ));
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

/// フォルダの指定（ID かパス）をフォルダの ID にする（ID はカタログにあるかを確かめる）。
fn resolve_folder(s: &Session, f: &FolderArg) -> CliResult<FolderId> {
    match f {
        FolderArg::Id(id) => {
            if s.core.folders()?.iter().any(|x| x.folder_id == *id) {
                Ok(*id)
            } else {
                Err(CliError::Api(genzo_api::ApiError::NotFound(format!(
                    "フォルダ {id}"
                ))))
            }
        }
        FolderArg::Path(p) => crate::library::find_folder(s, p),
    }
}

/// 既定で結び付ける候補（UI の defaultRelinkChoice と同じ。`accept_changed` なら、名前と撮影日時だけが同じ
/// 候補も、それしか候補がなければ選ぶ）。
pub(crate) fn default_choice(t: &RelinkTarget, accept_changed: bool) -> Option<&RelinkCandidate> {
    let safe: Vec<&RelinkCandidate> = t
        .candidates
        .iter()
        .filter(|c| c.matched != RelinkMatch::NameAndTime)
        .collect();
    if let [one] = safe.as_slice() {
        return Some(one);
    }
    let content: Vec<&RelinkCandidate> = t
        .candidates
        .iter()
        .filter(|c| c.matched == RelinkMatch::Content)
        .collect();
    if let [one] = content.as_slice() {
        return Some(one);
    }
    if accept_changed && safe.is_empty() && t.candidates.len() == 1 {
        return t.candidates.first();
    }
    None
}

fn folder_state_text(f: &FolderInfo) -> &'static str {
    if !f.online {
        "つながっていない"
    } else if f.missing {
        "見つからない"
    } else {
        "ok"
    }
}

fn folder_path_text(f: &FolderInfo) -> String {
    f.path
        .as_deref()
        .map_or_else(|| f.rel_path.clone(), |p| p.display().to_string())
}

fn print_folder(out: Output, f: &FolderInfo, what: &str) {
    if out.json {
        out.print_json(&json_value(f));
    } else {
        out.line(format!(
            "{what}: {}（フォルダ {}）",
            folder_path_text(f),
            f.folder_id
        ));
    }
}

fn print_relocate(out: Output, p: &RelocatePlan, dry_run: bool) {
    out.line(format!(
        "{}: {} → {}",
        if dry_run {
            "予定"
        } else {
            "指定し直しました"
        },
        p.from.as_deref().map_or_else(
            || "（前の場所が分かりません）".to_owned(),
            |f| f.display().to_string()
        ),
        p.to.display()
    ));
    out.line(format!(
        "  登録した写真のファイル {} 個のうち、{} 個が新しい場所に同じ名前・大きさで見つかりました（配下のフォルダ {} 個を含む）",
        p.files, p.found, p.subfolders
    ));
    if let Some(problem) = &p.problem {
        out.line(format!("  指定し直せません: {problem}"));
    }
}

fn print_move_plan(out: Output, plan: &FileMovePlan) {
    out.line(format!(
        "{} 枚の写真を {} へ移します（{} 枚は移せません）",
        plan.movable,
        plan.dest_path.as_deref().map_or_else(
            || format!("フォルダ {}", plan.dest_folder),
            |p| p.display().to_string()
        ),
        plan.photos.len() as u64 - plan.movable
    ));
    if plan.copy_bytes > 0 {
        out.line(format!(
            "  別のドライブへ {} バイトをコピーし、元と照合してから登録します（元のファイルはゴミ箱へ移します）",
            plan.copy_bytes
        ));
    }
    out.table(&photos_table(&plan.photos, true));
}

/// 予定の写真の表（ファイルごと）。
fn photos_table(photos: &[PlannedPhotoMove], show_method: bool) -> Table {
    let mut header = vec!["写真", "ファイル", "元", "先"];
    if show_method {
        header.push("方法");
    }
    header.push("問題");
    let mut t = Table::new(header);
    let path = |p: &Option<std::path::PathBuf>| {
        p.as_deref()
            .map_or_else(|| "—".to_owned(), |p| p.display().to_string())
    };
    for p in photos.iter().take(LIST_LIMIT) {
        for f in &p.files {
            let mut row = vec![
                p.variant_id.to_string(),
                f.file_id.to_string(),
                path(&f.from),
                path(&f.to),
            ];
            if show_method {
                row.push(
                    match f.method {
                        MoveMethod::Rename => "名前の変更",
                        MoveMethod::Copy => "コピーと照合",
                    }
                    .to_owned(),
                );
            }
            row.push(p.problem.clone().unwrap_or_default());
            t.row(row);
        }
    }
    if photos.len() > LIST_LIMIT {
        let mut row = vec![format!("ほか {} 枚", photos.len() - LIST_LIMIT)];
        row.resize(if show_method { 6 } else { 5 }, String::new());
        t.row(row);
    }
    t
}

fn print_move_report(out: Output, r: &FileMoveReport, what: &str) {
    if out.json {
        out.print_json(&json_value(r));
        return;
    }
    out.line(format!(
        "{} 枚の写真{what}（ファイル {} 個{}）{}",
        r.photos,
        r.moved.len(),
        if r.bytes_copied > 0 {
            format!("、コピー {} バイト", r.bytes_copied)
        } else {
            String::new()
        },
        if r.cancelled {
            "（途中で取り消しました）"
        } else {
            ""
        }
    ));
    for m in r.moved.iter().take(LIST_LIMIT) {
        out.line(format!("  {} → {}", m.from.display(), m.to.display()));
    }
    let issues = |title: &str, list: &[genzo_api::FileIssue]| {
        for f in list {
            out.line(format!(
                "  {title}: {}: {}",
                f.path.display(),
                one_line(&f.reason)
            ));
        }
    };
    issues("飛ばした", &r.skipped);
    issues("失敗した（元に戻しました）", &r.failed);
    issues("ゴミ箱へ移せず元の場所に残った", &r.left_sources);
}

fn move_status(r: &FileMoveReport) -> Status {
    if r.cancelled || !r.skipped.is_empty() || !r.failed.is_empty() || !r.left_sources.is_empty() {
        Status::Partial
    } else {
        Status::Success
    }
}

fn print_check(out: Output, r: &MissingCheckReport) {
    out.line(format!(
        "{} 件のファイルを確かめました（新しく見つからなくなった {} 件、元の場所に戻っていた {} 件）{}",
        r.checked,
        r.newly_missing,
        r.restored,
        if r.cancelled {
            "（途中で取り消しました）"
        } else {
            ""
        }
    ));
    out.line(format!(
        "  見つからない写真のファイル: {} 件、見つからないフォルダ: {} 個",
        r.missing, r.missing_folders
    ));
    if r.skipped_offline > 0 {
        out.line(format!(
            "  ドライブがつながっていないため確かめなかったファイル: {} 件",
            r.skipped_offline
        ));
    }
    if r.skipped_network > 0 {
        out.line(format!(
            "  ネットワークのドライブのため確かめなかったファイル: {} 件",
            r.skipped_network
        ));
    }
    if r.missing > 0 {
        out.line("  一覧は search --missing、結び付け直しは missing relink --in <DIR>");
    }
}

fn match_text(m: RelinkMatch) -> &'static str {
    match m {
        RelinkMatch::Content => "内容が同じ",
        RelinkMatch::Quick => "大きさと先頭・末尾が同じ",
        RelinkMatch::NameAndTime => "名前と撮影日時が同じ（内容は変わっている）",
    }
}

fn print_relink(out: Output, search: &RelinkSearch, chosen: &[RelinkRequest], dry_run: bool) {
    out.line(format!(
        "{} 個のファイルを調べました。見つからない写真のファイル {} 個のうち、候補のあるもの {} 個{}",
        search.scanned_files,
        search.targets.len(),
        search
            .targets
            .iter()
            .filter(|t| !t.candidates.is_empty())
            .count(),
        if search.cancelled {
            "（途中で取り消しました）"
        } else {
            ""
        }
    ));
    if !search.unreadable.is_empty() {
        out.line(format!(
            "  読めなかったフォルダ・ファイル: {} 個",
            search.unreadable.len()
        ));
    }
    let mut t = Table::new(["ファイル", "写真", "名前", "候補", "一致", "選択"]);
    for target in search.targets.iter().take(LIST_LIMIT) {
        let pick = chosen.iter().find(|c| c.file_id == target.file_id);
        if target.candidates.is_empty() {
            t.row([
                target.file_id.to_string(),
                target.variant_id.to_string(),
                target.name.clone(),
                "（候補なし）".to_owned(),
                String::new(),
                String::new(),
            ]);
        }
        for c in &target.candidates {
            let selected = pick.is_some_and(|p| p.path == c.path);
            t.row([
                target.file_id.to_string(),
                target.variant_id.to_string(),
                target.name.clone(),
                c.path.display().to_string(),
                match_text(c.matched).to_owned(),
                if selected {
                    if dry_run {
                        "結び付ける予定"
                    } else {
                        "結び付ける"
                    }
                    .to_owned()
                } else {
                    String::new()
                },
            ]);
        }
    }
    out.table(&t);
    if dry_run && !chosen.is_empty() {
        out.line(format!(
            "--dry-run を外すと {} 個を結び付けます（ほかは missing link <ファイル> <パス> で指定できます）",
            chosen.len()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genzo_model::{FileId, FileRole, VariantId};

    fn target(cands: &[RelinkMatch]) -> RelinkTarget {
        RelinkTarget {
            file_id: FileId::new(1),
            variant_id: VariantId::new(1),
            role: FileRole::Primary,
            name: "A.ARW".to_owned(),
            last_path: None,
            size: 10,
            candidates: cands
                .iter()
                .enumerate()
                .map(|(i, &m)| RelinkCandidate {
                    path: format!("/x/{i}/A.ARW").into(),
                    matched: m,
                    size: 10,
                    same_name: true,
                })
                .collect(),
        }
    }

    /// 既定の選択（UI の defaultRelinkChoice と同じ規則）。
    #[test]
    fn default_choice_picks_only_a_single_safe_candidate() {
        use RelinkMatch::*;
        let path = |t: &RelinkTarget, a| default_choice(t, a).map(|c| c.path.clone());
        assert_eq!(path(&target(&[Quick]), false), Some("/x/0/A.ARW".into()));
        assert_eq!(
            path(&target(&[Quick, NameAndTime]), false),
            Some("/x/0/A.ARW".into())
        );
        // 確からしい候補が 2 つ: 内容が同じものが 1 つならそれ、なければ選ばない。
        assert_eq!(
            path(&target(&[Quick, Content]), false),
            Some("/x/1/A.ARW".into())
        );
        assert_eq!(path(&target(&[Quick, Quick]), false), None);
        // 名前と撮影日時だけ: --accept-changed で、それしかなければ。
        assert_eq!(path(&target(&[NameAndTime]), false), None);
        assert_eq!(
            path(&target(&[NameAndTime]), true),
            Some("/x/0/A.ARW".into())
        );
        assert_eq!(path(&target(&[NameAndTime, NameAndTime]), true), None);
        assert_eq!(path(&target(&[]), true), None);
    }
}
