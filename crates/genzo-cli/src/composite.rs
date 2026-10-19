//! `composite`（比較明・比較暗合成。DEV-34。docs/composite_design.md の 6.3 節）。
//!
//! 選んだ写真を撮影日時の順に現像し、画素ごとに比べて 1 枚にして保存し、カタログに登録する（`genzo-api` の
//! `Core::composite`）。`--check` なら合成せずに、確認の結果（`Core::composite_check`）だけを表示する。合成に失敗した・
//! 取り消した・登録できなかった（メタデータを読めずにエラーの状態で登録したときを含む）・スタックにまとめられなかった
//! ときは終了コード 3（[`crate::EXIT_PARTIAL`]）。

use genzo_api::{CompositeCheck, CompositeReport, JobResult};
use genzo_model::{CompositeSettings, OutputColorSpace};
use serde_json::json;

use crate::Status;
use crate::args::{CompositeArgs, GlobalArgs};
use crate::error::{CliError, CliResult};
use crate::export::format_text;
use crate::library::{ensure_variants, unique_ids};
use crate::output::{Output, color_space_text};
use crate::session::{OpenOptions, Session};

/// 引数から合成の設定を作る（`--quality` は JPEG のときだけ）。
fn settings_of(a: &CompositeArgs) -> CliResult<CompositeSettings> {
    let defaults = CompositeSettings::default();
    let format = match a.format {
        Some(f) => f.to_format(a.quality),
        None => defaults.format,
    };
    if a.quality.is_some() && !matches!(format, genzo_model::ExportFormat::Jpeg { .. }) {
        return Err(CliError::Usage(
            "--quality は --format jpeg のときだけ指定できます".to_owned(),
        ));
    }
    Ok(CompositeSettings {
        mode: a.mode.into(),
        format,
        color_space: a
            .color_space
            .map_or(defaults.color_space, OutputColorSpace::from),
        remove_gps: a.remove_gps,
        stack_with_sources: !a.no_stack,
    })
}

fn ids_text(ids: &[genzo_model::VariantId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_check(out: Output, settings: &CompositeSettings, c: &CompositeCheck) {
    if out.json {
        out.print_json(&json!({"settings": settings, "check": c}));
        return;
    }
    out.line(format!(
        "{}（{}・{}）: {} 枚（撮影日時の順: {}）",
        settings.mode.display_name(),
        format_text(settings.format),
        color_space_text(settings.color_space),
        c.sources.len(),
        ids_text(&c.sources)
    ));
    if c.skipped_videos > 0 {
        out.line(format!("動画 {} 本は除きます", c.skipped_videos));
    }
    if let (Some(dir), Some(name)) = (&c.dest_dir, &c.file_name) {
        out.line(format!("保存先: {}", dir.join(name).display()));
    }
    if c.estimated_buffer_bytes > 0 {
        out.line(format!(
            "累積のバッファ: 約 {} MiB（元の写真の寸法から）",
            c.estimated_buffer_bytes.div_ceil(1 << 20)
        ));
    }
    if !c.size_differs.is_empty() {
        out.warn(format!(
            "元の写真の寸法・向きが 1 枚目と違う写真（現像した後の寸法が違えば合成できません）: {}",
            ids_text(&c.size_differs)
        ));
    }
    if !c.geometry_differs.is_empty() {
        out.warn(format!(
            "切り抜き・角度・歪曲補正が 1 枚目と違う写真（位置がずれます）: {}",
            ids_text(&c.geometry_differs)
        ));
    }
    if !c.settings_differ.is_empty() {
        out.warn(format!(
            "現像の設定が 1 枚目と違う写真: {}",
            ids_text(&c.settings_differ)
        ));
    }
    if settings.stack_with_sources && c.stacks > 0 {
        out.line(format!(
            "元の写真が入っている {} 個のスタックは、合成の結果と 1 つのスタックにまとまります",
            c.stacks
        ));
    }
    for p in &c.problems {
        out.warn(p);
    }
}

fn print_report(out: Output, settings: &CompositeSettings, r: &CompositeReport) {
    if out.json {
        out.print_json(&json!({"settings": settings, "report": r}));
        return;
    }
    for w in &r.warnings {
        out.warn(format!("{}: {}", w.variant_id, w.message));
    }
    if let Some(f) = &r.failure {
        match f.variant_id {
            Some(v) => out.line(format!(
                "合成できませんでした（写真 {v}。{} / {} 枚目まで重ねました）: {}",
                r.composited,
                r.sources.len(),
                f.error.message
            )),
            None => out.line(format!("合成できませんでした: {}", f.error.message)),
        }
        return;
    }
    if r.cancelled {
        out.line(format!(
            "取り消しました（{} / {} 枚。何も保存していません）",
            r.composited,
            r.sources.len()
        ));
        return;
    }
    if let Some(path) = &r.path {
        let size = r
            .size
            .map(|(w, h)| format!("{w} × {h}"))
            .unwrap_or_default();
        out.line(format!(
            "{}: {} 枚 → {}（{size}）",
            settings.mode.display_name(),
            r.composited,
            path.display()
        ));
    }
    match (r.registered, &r.registration_error) {
        (Some(v), None) => out.line(format!("カタログに登録しました: {v}")),
        (Some(v), Some(e)) => out.warn(format!("カタログに登録しました（{v}）が、{e}")),
        (None, Some(e)) => out.warn(format!(
            "カタログに登録できませんでした（ファイルは残しました。取り込み直せます）: {e}"
        )),
        (None, None) => {}
    }
    if r.stacked {
        out.line("元の写真とスタックにまとめ、一番上にしました");
    }
    if let Some(e) = &r.stack_error {
        out.warn(format!("スタックにまとめられませんでした: {e}"));
    }
    if r.cpu_renders > 0 {
        out.line(format!("CPU 版で現像した写真: {} 枚", r.cpu_renders));
    }
}

/// `composite`。
pub fn run(g: &GlobalArgs, out: Output, a: CompositeArgs) -> CliResult<Status> {
    let ids = unique_ids(&a.variants);
    let settings = settings_of(&a)?;
    if let Some(dir) = &a.out
        && dir.exists()
        && !dir.is_dir()
    {
        return Err(CliError::Usage(format!(
            "--out には保存先のフォルダを指定してください（{} はフォルダではありません）",
            dir.display()
        )));
    }
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &ids)?;
    let check = s.core.composite_check(&ids, &settings, a.out.as_deref())?;
    if a.check {
        s.close()?;
        print_check(out, &settings, &check);
        return Ok(if check.problems.is_empty() {
            Status::Success
        } else {
            Status::Partial
        });
    }
    if !check.problems.is_empty() {
        return Err(CliError::Usage(check.problems.join("。")));
    }
    let job = s.core.composite(&ids, &settings, a.out.as_deref())?;
    let info = s.wait_job_ok(job, "合成")?;
    let Some(JobResult::Composite(report)) = info.result else {
        return Err(CliError::other("合成の結果がありません"));
    };
    s.close()?;
    print_report(out, &settings, &report);
    let incomplete = report.failure.is_some()
        || report.cancelled
        || report.registered.is_none()
        || report.registration_error.is_some()
        || report.stack_error.is_some();
    Ok(if incomplete {
        Status::Partial
    } else {
        Status::Success
    })
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::args::{Cli, Command};

    fn parse(args: &[&str]) -> CompositeArgs {
        let mut v = vec!["genzo", "composite"];
        v.extend_from_slice(args);
        match Cli::try_parse_from(v).unwrap().command {
            Command::Composite(a) => a,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn defaults_follow_the_design() {
        let s = settings_of(&parse(&["1", "2"])).unwrap();
        assert_eq!(s, CompositeSettings::default());
    }

    #[test]
    fn options_map_to_settings() {
        let s = settings_of(&parse(&[
            "--mode",
            "lighter-color",
            "--format",
            "jpeg",
            "--quality",
            "80",
            "--color-space",
            "srgb",
            "--no-stack",
            "--remove-gps",
            "1",
            "2",
        ]))
        .unwrap();
        assert_eq!(s.mode, genzo_model::CompositeMode::LighterColor);
        assert_eq!(s.format, genzo_model::ExportFormat::Jpeg { quality: 80 });
        assert_eq!(s.color_space, OutputColorSpace::Srgb);
        assert!(!s.stack_with_sources);
        assert!(s.remove_gps);
    }

    #[test]
    fn quality_needs_jpeg() {
        assert!(matches!(
            settings_of(&parse(&["--quality", "80", "1", "2"])),
            Err(CliError::Usage(_))
        ));
    }
}
