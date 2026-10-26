//! `genzo ai`（AI マスク。DEV-20）: 推論の仕組みの状態と、カタログの中のラスターの保守。マスクを足すのは
//! `develop mask add <ID> --ai <種類>`（`masks.rs`）。

use serde_json::json;

use crate::Status;
use crate::args::{AiCommand, GlobalArgs};
use crate::error::CliResult;
use crate::output::{Output, json_value};
use crate::session::{OpenOptions, Session};

/// `ai` のサブコマンドを実行する。
pub fn run(g: &GlobalArgs, out: Output, cmd: AiCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = run_in(&s, out, cmd);
    s.close()?;
    r
}

fn run_in(s: &Session, out: Output, cmd: AiCommand) -> CliResult<Status> {
    match cmd {
        AiCommand::Status => {
            let st = s.core.ai_mask_status()?;
            if out.json {
                out.print_json(&json_value(&st));
            } else {
                out.line(format!(
                    "AI マスク: {}",
                    if st.available {
                        "使えます"
                    } else {
                        "使えません"
                    }
                ));
                for (kind, model) in st.kinds.iter().zip(&st.models) {
                    out.line(format!("  {}: モデル {model}", kind.display_name()));
                }
                if !st.backend.is_empty() {
                    out.line(format!("  推論: {}", st.backend));
                }
                out.line(format!(
                    "  モデルのフォルダ: {}",
                    st.models_dir
                        .as_ref()
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| "（不明）".to_owned())
                ));
                if let Some(d) = &st.bundled_models_dir {
                    out.line(format!("  同梱のモデルのフォルダ: {}", d.display()));
                }
                out.line(format!(
                    "  ONNX Runtime: {}",
                    st.onnxruntime
                        .as_ref()
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| "（見つからない）".to_owned())
                ));
                out.line(format!(
                    "  カタログの中のラスター: {} 件（{} バイト）",
                    st.rasters, st.raster_bytes
                ));
                for p in &st.problems {
                    out.line(format!("  問題: {p}"));
                }
            }
            Ok(if st.available {
                Status::Success
            } else {
                Status::Partial
            })
        }
        AiCommand::Collect => {
            let r = s.core.collect_ai_masks()?;
            if out.json {
                out.print_json(&json!({
                    "removed": r.removed,
                    "remaining": r.remaining,
                    "remaining_bytes": r.remaining_bytes,
                }));
            } else {
                out.line(format!(
                    "参照のない AI マスクのラスターを {} 件消しました（残り {} 件、{} バイト）",
                    r.removed, r.remaining, r.remaining_bytes
                ));
            }
            Ok(Status::Success)
        }
    }
}
