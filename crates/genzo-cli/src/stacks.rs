//! スタック（LIB-12）のコマンド（`stack`）と、仮想コピー（LIB-13）の名前の変更・マスターの入れ替え
//! （`develop rename-copy`・`develop make-master`）。
//!
//! - スタックは ID で、写真は variant の ID で指定する（`stack show` で、写真が入っているスタックの ID と
//!   写真の順を見られる）。操作はコア API（genzo-api の `stacks`）と同じ規則（docs/04 の 3.10 節）。
//! - 折りたたみの状態はカタログに保存され、画面のグリッドと `search` の並びに効く（`search --expand-stacks` で
//!   すべて展開して並べる）。

use genzo_api::{AutoStackReport, StackInfo, VariantId};
use serde_json::json;

use crate::Status;
use crate::args::{AutoStackArgs, GlobalArgs, StackCommand};
use crate::error::{CliError, CliResult};
use crate::library::{ensure_variants, filter_of, unique_ids};
use crate::output::{Output, Table, json_value};
use crate::session::{OpenOptions, Session};

/// `stack`。
pub fn run(g: &GlobalArgs, out: Output, cmd: StackCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = run_in(&s, out, cmd);
    s.close()?;
    r
}

fn run_in(s: &Session, out: Output, cmd: StackCommand) -> CliResult<Status> {
    match cmd {
        StackCommand::Create { variants, top } => {
            let ids = unique_ids(&variants);
            ensure_variants(s, &ids)?;
            let info = s.core.create_stack(&ids, top)?;
            print_stack(out, &info, "スタックを作りました");
        }
        StackCommand::Add { stack, variants } => {
            let ids = unique_ids(&variants);
            ensure_variants(s, &ids)?;
            let n = s.core.add_to_stack(stack, &ids)?;
            print_count(out, n, &format!("スタック {stack} に {n} 枚を足しました"));
        }
        StackCommand::Remove { variants } => {
            let n = s.core.remove_from_stack(&unique_ids(&variants))?;
            print_count(
                out,
                n,
                &format!("{n} 枚をスタックから外しました（1 枚になったスタックは消えます）"),
            );
        }
        StackCommand::Unstack { variants } => {
            let n = s.core.unstack(&unique_ids(&variants))?;
            print_count(
                out,
                n,
                &format!("スタックを {n} 個解除しました（写真は削除していません）"),
            );
        }
        StackCommand::Top { variant } => {
            let info = s.core.set_stack_top(variant)?;
            print_stack(out, &info, "一番上の写真を変えました");
        }
        StackCommand::Collapse { stacks, all } => {
            let n = s
                .core
                .set_stacks_collapsed((!all).then_some(stacks.as_slice()), true)?;
            print_count(out, n, &format!("スタックを {n} 個折りたたみました"));
        }
        StackCommand::Expand { stacks, all } => {
            let n = s
                .core
                .set_stacks_collapsed((!all).then_some(stacks.as_slice()), false)?;
            print_count(out, n, &format!("スタックを {n} 個展開しました"));
        }
        StackCommand::Show { variants } => {
            let ids = unique_ids(&variants);
            ensure_variants(s, &ids)?;
            let stacks = s.core.variant_stacks(&ids)?;
            if out.json {
                out.print_json(&json!({ "stacks": json_value(&stacks) }));
            } else if stacks.is_empty() {
                out.line("指定した写真はスタックに入っていません");
            } else {
                let mut t = Table::new([
                    "スタック",
                    "写真の数",
                    "一番上",
                    "状態",
                    "写真（スタックの順）",
                ]);
                for st in &stacks {
                    t.row([
                        st.stack_id.to_string(),
                        st.members.len().to_string(),
                        st.top_variant_id.to_string(),
                        collapsed_text(st.collapsed).to_owned(),
                        ids_text(&st.members),
                    ]);
                }
                out.table(&t);
            }
        }
        StackCommand::Auto(a) => return auto(s, out, &a),
    }
    Ok(Status::Success)
}

/// `stack auto`。
fn auto(s: &Session, out: Output, a: &AutoStackArgs) -> CliResult<Status> {
    if a.gap > genzo_api::MAX_AUTO_STACK_GAP_SECONDS {
        return Err(CliError::Usage(format!(
            "--gap は {} 秒までです（{}）",
            genzo_api::MAX_AUTO_STACK_GAP_SECONDS,
            a.gap
        )));
    }
    let filter = filter_of(s, &a.filter)?;
    let r = s.core.auto_stack(&filter, a.gap, a.dry_run)?;
    if out.json {
        out.print_json(&json_value(&r));
    } else {
        for line in describe(&r) {
            out.line(line);
        }
    }
    Ok(Status::Success)
}

/// 自動スタックの結果の文（画面の library/stackLogic.ts の describeAutoStack と同じ内容）。
fn describe(r: &AutoStackReport) -> Vec<String> {
    let mut lines = Vec::new();
    if r.stacks == 0 {
        lines.push(format!(
            "この間隔でまとめられる写真はありません（対象 {} 枚）",
            r.considered
        ));
    } else if r.dry_run {
        lines.push(format!(
            "{} 個のスタックを作ります（{} 枚。対象 {} 枚。--dry-run のため作っていません）",
            r.stacks, r.photos, r.considered
        ));
    } else {
        lines.push(format!(
            "{} 個のスタックを作りました（{} 枚。対象 {} 枚）",
            r.stacks, r.photos, r.considered
        ));
    }
    if r.skipped_in_stacks > 0 {
        lines.push(format!(
            "すでにスタックに入っている {} 枚は対象にしませんでした",
            r.skipped_in_stacks
        ));
    }
    if r.skipped_without_time > 0 {
        lines.push(format!(
            "撮影日時の分からない {} 枚は対象にしませんでした",
            r.skipped_without_time
        ));
    }
    lines
}

fn collapsed_text(collapsed: bool) -> &'static str {
    if collapsed {
        "折りたたみ"
    } else {
        "展開"
    }
}

fn ids_text(ids: &[VariantId]) -> String {
    ids.iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_stack(out: Output, info: &StackInfo, what: &str) {
    if out.json {
        out.print_json(&json_value(info));
    } else {
        out.line(format!(
            "{what}: スタック {}（{} 枚・一番上 {}・{}。写真: {}）",
            info.stack_id,
            info.members.len(),
            info.top_variant_id,
            collapsed_text(info.collapsed),
            ids_text(&info.members)
        ));
    }
}

fn print_count(out: Output, n: usize, text: &str) {
    if out.json {
        out.print_json(&json!({ "count": n }));
    } else {
        out.line(text);
    }
}

/// `develop rename-copy`（LIB-13）。
pub fn rename_copy(g: &GlobalArgs, out: Output, v: VariantId, name: &str) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = s.core.rename_virtual_copy(v, name);
    s.close()?;
    r?;
    if out.json {
        out.print_json(&json!({ "ok": true }));
    } else {
        out.line(format!(
            "仮想コピー {v} の名前を「{}」にしました",
            name.trim()
        ));
    }
    Ok(Status::Success)
}

/// `develop make-master`（LIB-13）。
pub fn make_master(g: &GlobalArgs, out: Output, v: VariantId) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = s.core.set_as_master(v);
    s.close()?;
    let previous = r?;
    if out.json {
        out.print_json(&json!({ "master": v, "previous_master": previous }));
    } else if previous == v {
        out.line(format!("variant {v} はすでにマスターです"));
    } else {
        out.line(format!(
            "variant {v} をマスターにしました（前のマスター {previous} は仮想コピーになりました）"
        ));
    }
    Ok(Status::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_stack_reports_are_described() {
        let r = AutoStackReport {
            stacks: 3,
            photos: 8,
            considered: 20,
            skipped_in_stacks: 2,
            skipped_without_time: 1,
            dry_run: true,
        };
        let lines = describe(&r);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("3 個のスタックを作ります（8 枚。対象 20 枚。"));
        let none = describe(&AutoStackReport {
            considered: 5,
            ..Default::default()
        });
        assert_eq!(
            none,
            vec!["この間隔でまとめられる写真はありません（対象 5 枚）"]
        );
    }
}
