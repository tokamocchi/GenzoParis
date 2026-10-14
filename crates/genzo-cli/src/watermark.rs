//! `watermark preview / font-dirs`（EXP-03。カタログは使わない）。
//!
//! - `preview`: 透かしの見本を PNG に書く（genzo-api の `render_watermark_preview`。灰色の濃淡の見本の画像に、
//!   書き出しと同じ計算で重ねる。透かしは自分で起動したワーカーで作る（SEC-05））。書き出し先に同じ名前があれば
//!   連番を付ける（上書きしない）。フォントにない文字・縮めたことは警告に出す。
//! - `font-dirs`: OS の既定のフォントのフォルダ（あるものだけ。文字の透かしのフォントを探す場所。フォントは
//!   同梱しない）。

use genzo_jobs::CancellationToken;
use genzo_model::ConflictPolicy;
use serde_json::json;

use crate::Status;
use crate::args::{WatermarkCommand, WatermarkPreviewArgs};
use crate::error::{CliError, CliResult};
use crate::output::Output;
use crate::standalone::Worker;

/// `watermark` を実行する。
pub fn run(out: Output, cmd: WatermarkCommand) -> CliResult<Status> {
    match cmd {
        WatermarkCommand::Preview(a) => preview(out, &a),
        WatermarkCommand::FontDirs => {
            let dirs = genzo_api::system_font_dirs();
            if out.json {
                out.print_json(&json!({ "dirs": dirs }));
            } else if dirs.is_empty() {
                out.line("OS の既定のフォントのフォルダが見つかりません");
            } else {
                for d in &dirs {
                    out.line(d.display().to_string());
                }
            }
            Ok(Status::Success)
        }
    }
}

fn preview(out: Output, a: &WatermarkPreviewArgs) -> CliResult<Status> {
    let Some(watermark) = crate::export::watermark_from_args(None, &a.watermark)? else {
        return Err(CliError::Usage(
            "透かしの中身を --watermark-text か --watermark-image で指定してください".to_owned(),
        ));
    };
    if a.out.is_dir() {
        return Err(CliError::Usage(format!(
            "--out には書き出す PNG のパスを指定してください（{} はフォルダです）",
            a.out.display()
        )));
    }
    let size = match (a.width, a.height) {
        (None, None) => None,
        (w, h) => {
            let (dw, dh) = genzo_api::PREVIEW_DEFAULT_SIZE;
            Some((w.unwrap_or(dw), h.unwrap_or(dh)))
        }
    };
    let mut worker = Worker::spawn()?;
    let token = CancellationToken::new();
    let result = genzo_api::render_watermark_preview(&watermark, size, |request| {
        Ok(worker.client().render_watermark(request, &token)?)
    });
    worker.shutdown(out);
    let (png, info) = result?;
    let written = genzo_media::write_atomically(
        &a.out,
        ConflictPolicy::Sequence,
        &genzo_media::ProtectedFiles::new(),
        |f| {
            use std::io::Write;
            f.write_all(&png).map_err(|e| genzo_media::MediaError::Io {
                path: Some(a.out.clone()),
                source: e,
            })
        },
    )?;
    let path = match written {
        genzo_media::WriteOutcome::Written { path, .. } => path,
        genzo_media::WriteOutcome::Skipped { existing } => existing,
    };
    for w in &info.warnings {
        out.warn(w);
    }
    if out.json {
        out.print_json(&json!({
            "output": path,
            "width": info.width,
            "height": info.height,
            "mark_width": info.mark_width,
            "mark_height": info.mark_height,
            "warnings": info.warnings,
            "watermark": watermark,
        }));
    } else {
        out.line(format!(
            "透かしの見本を書き出しました: {}（{} × {}、透かし {} × {}）",
            path.display(),
            info.width,
            info.height,
            info.mark_width,
            info.mark_height
        ));
    }
    Ok(Status::Success)
}
