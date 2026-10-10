//! `export`（EXP-01・EXP-04、01 のストーリー 7。04 の 2.4 節・6.4 節）と、書き出しの設定の組み立て。

use genzo_api::{ExportOutcome, ExportReport, JobResult, RenderBackend};
use genzo_model::{ExportFormat, ExportSettings, ExportSize, OutputColorSpace};
use serde_json::json;

use crate::Status;
use crate::args::{ExportArgs, ExportOpts, FormatArg, GlobalArgs};
use crate::error::{CliError, CliResult};
use crate::library::{ensure_variants, unique_ids};
use crate::output::{Output, Table, color_space_text};
use crate::session::{OpenOptions, Session};

/// 書き出しの設定を作る（`format` と `color_space` は省略時の値を呼び出し側が決める）。
///
/// `--quality` は JPEG のときだけ指定できる（それ以外は使い方の誤り）。
pub fn export_settings(
    opts: &ExportOpts,
    format: FormatArg,
    color_space: OutputColorSpace,
) -> CliResult<ExportSettings> {
    if opts.quality.is_some() && format != FormatArg::Jpeg {
        return Err(CliError::Usage(
            "--quality は JPEG の書き出しのときだけ指定できます".to_owned(),
        ));
    }
    let settings = ExportSettings {
        format: format.to_format(opts.quality),
        color_space,
        size: opts
            .long_edge
            .map_or(ExportSize::Original, ExportSize::LongEdge),
        remove_gps: opts.remove_gps,
        on_conflict: opts.on_conflict.into(),
    };
    settings
        .validate()
        .map_err(|e| CliError::Usage(e.to_string()))?;
    Ok(settings)
}

/// 処理した側の表示名（JSON の `backend` と同じ名前）。
pub fn backend_text(b: RenderBackend) -> &'static str {
    match b {
        RenderBackend::Gpu => "gpu",
        RenderBackend::Cpu => "cpu",
    }
}

/// 形式の表示名。
pub fn format_text(f: ExportFormat) -> String {
    match f {
        ExportFormat::Jpeg { quality } => format!("JPEG（品質 {quality}）"),
        ExportFormat::Tiff16 => "TIFF 16bit".to_owned(),
        ExportFormat::Png8 => "PNG 8bit".to_owned(),
        ExportFormat::Png16 => "PNG 16bit".to_owned(),
    }
}

/// `export`。一部でも失敗したら終了コード 1（結果の一覧は出す）。
pub fn run(g: &GlobalArgs, out: Output, a: ExportArgs) -> CliResult<Status> {
    let ids = unique_ids(&a.variants);
    // 使い方の誤り（--quality と形式の組み合わせなど）は、カタログを開く前に確かめる。色空間の指定が
    // なければ、開いた後にカタログの設定（書き出しの既定の色空間）にする。
    let mut settings = export_settings(
        &a.opts,
        a.opts.format.unwrap_or(FormatArg::Jpeg),
        a.opts
            .color_space
            .map_or(OutputColorSpace::Srgb, OutputColorSpace::from),
    )?;
    // 書き出し先はフォルダ（既にあるファイルを指定した場合は、作れないフォルダとして分かりにくい
    // 入出力のエラーになるため、先に使い方の誤りにする）。
    if a.out.exists() && !a.out.is_dir() {
        return Err(CliError::Usage(format!(
            "--out には書き出し先のフォルダを指定してください（{} はフォルダではありません）",
            a.out.display()
        )));
    }
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &ids)?;
    if a.opts.color_space.is_none() {
        settings.color_space = s.core.settings().default_export_color_space;
    }
    let job = s.core.export(&ids, &settings, &a.out)?;
    let info = s.wait_job_ok(job, "書き出し")?;
    let Some(JobResult::Export(report)) = info.result else {
        return Err(CliError::other("書き出しの結果がありません"));
    };
    s.close()?;
    let failed = report.failed > 0 || report.cancelled;
    print_report(out, &settings, &report);
    Ok(if failed {
        Status::Failure
    } else {
        Status::Success
    })
}

fn print_report(out: Output, settings: &ExportSettings, report: &ExportReport) {
    if out.json {
        out.print_json(&json!({
            "settings": settings,
            "report": report,
        }));
        return;
    }
    // 長いパス・理由は最後の列に置く（途中の列だと、ほかの行が長い空白で埋まるため）。
    let mut t = Table::new(["ID", "結果", "処理", "ファイル・理由"]);
    for item in &report.items {
        match &item.outcome {
            ExportOutcome::Written {
                path,
                replaced,
                backend,
                warnings,
                ..
            } => t.row([
                item.variant_id.to_string(),
                if *replaced {
                    "上書き"
                } else {
                    "書き出し"
                }
                .to_owned(),
                backend_text(*backend).to_owned(),
                if warnings.is_empty() {
                    path.display().to_string()
                } else {
                    format!("{}（警告: {}）", path.display(), warnings.join(" / "))
                },
            ]),
            ExportOutcome::Skipped { reason, existing } => t.row([
                item.variant_id.to_string(),
                "スキップ".to_owned(),
                String::new(),
                match existing {
                    Some(p) => format!("{reason}（{}）", p.display()),
                    None => reason.clone(),
                },
            ]),
            ExportOutcome::Failed { error } => t.row([
                item.variant_id.to_string(),
                "失敗".to_owned(),
                String::new(),
                error.message.clone(),
            ]),
        }
    }
    out.table(&t);
    out.line(format!(
        "{}・{}: 書き出し {} 件、スキップ {} 件、失敗 {} 件{}（{}）",
        format_text(settings.format),
        color_space_text(settings.color_space),
        report.written,
        report.skipped,
        report.failed,
        if report.cancelled {
            "（取り消し）"
        } else {
            ""
        },
        report.dest_dir.display()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::ConflictArg;
    use genzo_model::ConflictPolicy;

    fn opts() -> ExportOpts {
        ExportOpts {
            format: None,
            quality: None,
            color_space: None,
            long_edge: None,
            remove_gps: false,
            on_conflict: ConflictArg::Sequence,
        }
    }

    #[test]
    fn settings_from_options() {
        let s = export_settings(&opts(), FormatArg::Jpeg, OutputColorSpace::Srgb).unwrap();
        assert_eq!(s, ExportSettings::default());
        let o = ExportOpts {
            quality: Some(70),
            long_edge: Some(800),
            remove_gps: true,
            on_conflict: ConflictArg::Skip,
            ..opts()
        };
        let s = export_settings(&o, FormatArg::Jpeg, OutputColorSpace::AdobeRgb).unwrap();
        assert_eq!(s.format, ExportFormat::Jpeg { quality: 70 });
        assert_eq!(s.size, ExportSize::LongEdge(800));
        assert!(s.remove_gps);
        assert_eq!(s.on_conflict, ConflictPolicy::Skip);
        assert_eq!(s.color_space, OutputColorSpace::AdobeRgb);
        let e = export_settings(&o, FormatArg::Png16, OutputColorSpace::Srgb).unwrap_err();
        assert_eq!(e.exit_code(), crate::EXIT_USAGE);
    }

    #[test]
    fn backend_names_match_the_json() {
        for b in [RenderBackend::Gpu, RenderBackend::Cpu] {
            assert_eq!(
                serde_json::to_value(b).unwrap(),
                serde_json::Value::String(backend_text(b).to_owned())
            );
        }
    }
}
