//! ファイルの内容による同一性（ORG-03・IMP-04）のコマンド（`hash`）。
//!
//! - `hash compute`: 全体のハッシュ（ファイル全体の BLAKE3。`b3sum` と同じ値）のないファイルを読んで記録する
//!   （genzo-api の `Core::compute_content_hashes`。CLI はバックグラウンドの自動の計算をしないので、ここで
//!   明示的に計算する）。ネットワークのボリュームのファイルは `--include-network` のときだけ読む。
//! - `hash status`・`hash duplicates`: 計算の状況と、内容が同じファイルの組（記録した全体のハッシュの中で）。
//! - `hash identify <ファイル>`: ファイルの内容で、場所・名前によらず同じ内容の登録済みのファイルを探す。
//!
//! 取り込みの重複の扱い（`import --keep-duplicates` など）は `library` の `import`。

use genzo_api::{ContentFile, JobResult};

use crate::Status;
use crate::args::{GlobalArgs, HashCommand};
use crate::error::{CliError, CliResult};
use crate::output::{Output, Table, json_value, key_values};
use crate::session::{OpenOptions, Session};

/// `hash`。
pub fn run(g: &GlobalArgs, out: Output, cmd: HashCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = run_in(&s, out, cmd);
    s.close()?;
    r
}

fn run_in(s: &Session, out: Output, cmd: HashCommand) -> CliResult<Status> {
    match cmd {
        HashCommand::Compute { include_network } => {
            let job = s.core.compute_content_hashes(include_network)?;
            let info = s.wait_job_ok(job, "内容のハッシュの計算")?;
            let Some(JobResult::HashFiles(r)) = info.result else {
                return Err(CliError::other("内容のハッシュの計算の結果がありません"));
            };
            if out.json {
                out.print_json(&json_value(&r));
            } else {
                out.line(format!(
                    "内容のハッシュを {} 件記録しました（ネットワークのボリュームのため飛ばした {} 件、見つからない・ドライブが見えない {} 件、登録の後に内容が変わっていた {} 件、読めなかった {} 件）",
                    r.hashed, r.skipped_network, r.skipped_unavailable, r.changed, r.failed_count
                ));
                for f in &r.failed {
                    out.line(format!(
                        "  読めなかったファイル: {}: {}",
                        f.path.display(),
                        crate::output::one_line(&f.reason)
                    ));
                }
                if r.skipped_network > 0 && !include_network {
                    out.line("  ネットワークのボリュームのファイルも読むには --include-network を付けてください");
                }
                if r.changed > 0 {
                    out.line("  内容が変わっていたファイルは、ファイルの確認（catalog check --files）で登録し直すと計算の対象になります");
                }
            }
            Ok(if r.failed_count > 0 || r.cancelled {
                Status::Partial
            } else {
                Status::Success
            })
        }
        HashCommand::Status => {
            let st = s.core.content_hash_status()?;
            if out.json {
                out.print_json(&json_value(&st));
            } else {
                out.line(key_values(&[
                    ("ファイル", st.files.to_string()),
                    ("計算済み", st.hashed.to_string()),
                    ("計算の対象", st.pending.to_string()),
                    ("内容が同じファイルの組", st.duplicate_groups.to_string()),
                ]));
            }
            Ok(Status::Success)
        }
        HashCommand::Duplicates { limit } => {
            let list = s
                .core
                .duplicate_groups(usize::try_from(limit).unwrap_or(usize::MAX))?;
            if out.json {
                out.print_json(&json_value(&list));
            } else if list.groups.is_empty() {
                out.line("内容が同じファイルはありません（全体のハッシュを記録したファイルの中で。hash compute で計算できます）");
            } else {
                out.line(format!(
                    "内容が同じファイルの組: {} 組{}",
                    list.total,
                    if list.total > list.groups.len() as u64 {
                        format!("（最初の {} 組を表示）", list.groups.len())
                    } else {
                        String::new()
                    }
                ));
                for (i, g) in list.groups.iter().enumerate() {
                    out.line(format!("{}. {}…", i + 1, &g.full_hash[..16]));
                    out.table(&files_table(&g.files));
                }
            }
            Ok(Status::Success)
        }
        HashCommand::Identify { file } => {
            let id = s.core.identify_file(&file)?;
            if out.json {
                out.print_json(&json_value(&id));
            } else {
                out.line(format!(
                    "{}（{} バイト、BLAKE3 {}）",
                    id.path.display(),
                    id.size,
                    id.full_hash
                ));
                if id.matches.is_empty() {
                    out.line("同じ内容の登録済みのファイルはありません");
                } else {
                    out.table(&files_table(&id.matches));
                }
            }
            Ok(Status::Success)
        }
    }
}

/// 内容が同じファイルの表。
fn files_table(files: &[ContentFile]) -> Table {
    let mut t = Table::new(["写真", "役割", "状態", "大きさ", "ファイル"]);
    for f in files {
        t.row([
            f.variant_id.to_string(),
            match f.role {
                genzo_model::FileRole::Primary => "主".to_owned(),
                genzo_model::FileRole::SidecarJpeg => "RAW と組の JPEG".to_owned(),
            },
            f.status.as_str().to_owned(),
            f.size.to_string(),
            f.path
                .as_deref()
                .map_or_else(|| "場所不明".to_owned(), |p| p.display().to_string()),
        ]);
    }
    t
}
