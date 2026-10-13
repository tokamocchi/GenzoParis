//! 動画の再生（VID-04）のコマンド（`video`）。
//!
//! - `video info <動画>`: 再生の情報（genzo-api の `Core::video_playback`）と、アプリの `genzo://video` が配信する
//!   ファイル（`Core::video_stream`。カタログに登録された動画の元のファイル・キャッシュの再生用のファイル）。
//! - `video proxy <動画>...`: 再生用のファイル（利用者の ffmpeg で H.264 ＋ AAC の MP4。長辺 1280）を作る
//!   （`Core::create_video_proxy`。コアは 1 度に 1 本だけ作るので、1 本ずつ順に作って待つ）。
//! - `video cache`・`video clear`: 再生用のファイルのキャッシュの状況と消去。
//!
//! ffmpeg・ffprobe は、環境変数 `GENZO_FFMPEG`・`GENZO_FFPROBE`（なければ PATH）のものを使う（ワーカーと同じ）。

use genzo_api::{JobResult, VariantId, VideoPlayback, VideoProxyCacheStatus};

use crate::Status;
use crate::args::{GlobalArgs, VideoCommand};
use crate::error::{CliError, CliResult};
use crate::output::{Output, Table, json_value, key_values, one_line};
use crate::session::{OpenOptions, Session};

/// `video`。
pub fn run(g: &GlobalArgs, out: Output, cmd: VideoCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = run_in(&s, out, cmd);
    s.close()?;
    r
}

fn yes_no(b: bool) -> String {
    if b { "はい" } else { "いいえ" }.to_owned()
}

fn cache_lines(out: Output, st: &VideoProxyCacheStatus) {
    out.line(key_values(&[
        ("フォルダ", st.dir.display().to_string()),
        ("再生用のファイル", format!("{} 個", st.count)),
        ("合計", format!("{} バイト", st.total_bytes)),
        ("上限", format!("{} バイト", st.capacity_bytes)),
    ]));
}

fn info_lines(out: Output, p: &VideoPlayback, original: Option<String>, proxy: Option<String>) {
    let v = p.video.as_ref();
    let text = |o: Option<String>| o.unwrap_or_else(|| "—".to_owned());
    out.line(key_values(&[
        ("動画", p.variant_id.to_string()),
        ("元のファイルを読める", yes_no(p.available)),
        (
            "読めない理由",
            text(p.unavailable_reason.as_deref().map(one_line)),
        ),
        ("MIME の型", p.content_type.clone()),
        ("大きさ", text(p.size.map(|n| format!("{n} バイト")))),
        ("コーデック", text(v.and_then(|v| v.codec.clone()))),
        (
            "ビット深度",
            text(v.and_then(|v| v.bit_depth).map(|b| format!("{b} bit"))),
        ),
        ("配信する元のファイル", text(original)),
        (
            "再生用のファイル",
            match (&p.proxy, proxy) {
                (Some(x), Some(path)) => format!("{path}（{} バイト）", x.size),
                _ => "なし".to_owned(),
            },
        ),
        ("作っているジョブ", text(p.proxy_job.map(|j| j.to_string()))),
        ("作るときのエンコーダー", text(p.proxy_encoder.clone())),
        (
            "作れない理由",
            text(p.proxy_unavailable.as_deref().map(one_line)),
        ),
    ]));
}

fn run_in(s: &Session, out: Output, cmd: VideoCommand) -> CliResult<Status> {
    match cmd {
        VideoCommand::Info { variant } => {
            let p = s.core.video_playback(variant)?;
            let original = s
                .core
                .video_stream(variant, false)
                .ok()
                .map(|st| st.path.display().to_string());
            let proxy = s
                .core
                .video_stream(variant, true)
                .ok()
                .map(|st| st.path.display().to_string());
            if out.json {
                out.print_json(&serde_json::json!({
                    "playback": json_value(&p),
                    "stream": original,
                    "proxy_stream": proxy,
                }));
            } else {
                info_lines(out, &p, original, proxy);
            }
            Ok(Status::Success)
        }
        VideoCommand::Proxy { variants } => proxies(s, out, &variants),
        VideoCommand::Cache => {
            let st = s.core.video_proxy_cache()?;
            if out.json {
                out.print_json(&json_value(&st));
            } else {
                cache_lines(out, &st);
            }
            Ok(Status::Success)
        }
        VideoCommand::Clear => {
            let st = s.core.clear_video_proxies()?;
            if out.json {
                out.print_json(&json_value(&st));
            } else {
                out.line(format!("再生用のファイルを {} 個消しました", st.removed));
                cache_lines(out, &st);
            }
            Ok(Status::Success)
        }
    }
}

/// 再生用のファイルを 1 本ずつ作る（コアは別の動画を頼まれると作っている途中のものを取り消すので、待ってから次へ）。
fn proxies(s: &Session, out: Output, variants: &[VariantId]) -> CliResult<Status> {
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut table = Table::new(["動画", "結果", "大きさ", "エンコーダー・理由"]);
    let mut failed = 0usize;
    for &v in variants {
        let started = s.core.create_video_proxy(v);
        let outcome = match started {
            Ok(job) => match s.wait_job_ok(job, &format!("動画 {v} の再生用のファイルの作成"))
            {
                Ok(info) => match info.result {
                    Some(JobResult::VideoProxy(r)) => Ok(r),
                    other => Err(CliError::other(format!(
                        "再生用のファイルの作成の結果がありません（{other:?}）"
                    ))),
                },
                Err(e) => Err(e),
            },
            Err(e) => Err(CliError::Api(e)),
        };
        match outcome {
            Ok(r) => {
                table.row([
                    v.to_string(),
                    if r.reused { "あった" } else { "作った" }.to_owned(),
                    format!("{} バイト", r.size),
                    r.encoder.clone().unwrap_or_else(|| "—".to_owned()),
                ]);
                rows.push(
                    serde_json::json!({"variant_id": v, "ok": true, "report": json_value(&r)}),
                );
            }
            // Ctrl+C の取り消しは、残りを作らずに終える（結果は作ったところまで）。
            Err(CliError::Api(e)) if e.is_cancelled() => {
                failed += 1;
                table.row([
                    v.to_string(),
                    "取り消した".to_owned(),
                    "—".to_owned(),
                    "—".to_owned(),
                ]);
                rows.push(serde_json::json!({"variant_id": v, "ok": false, "error": e.info()}));
                break;
            }
            Err(e) => {
                failed += 1;
                let reason = one_line(&e.human());
                table.row([
                    v.to_string(),
                    "作れなかった".to_owned(),
                    "—".to_owned(),
                    reason.clone(),
                ]);
                rows.push(serde_json::json!({"variant_id": v, "ok": false, "error": reason}));
            }
        }
    }
    if out.json {
        out.print_json(&serde_json::json!({ "proxies": rows }));
    } else {
        out.table(&table);
    }
    Ok(if failed > 0 {
        Status::Partial
    } else {
        Status::Success
    })
}
