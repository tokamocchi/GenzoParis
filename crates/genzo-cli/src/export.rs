//! `export`（EXP-01・EXP-02・EXP-04、01 のストーリー 7。04 の 2.4 節・6.4 節）と、書き出しの設定の組み立て。
//!
//! - `--preset`（EXP-02）: 書き出しプリセット（ID か名前。`crate::preset::resolve`）の中身（形式・色空間・大きさ・
//!   メタデータ・同じ名前の扱い・書き出し先・ファイル名の規則）を使い、指定したオプションで上書きする
//!   （[`export_plan`]。`--remove-gps`・`--remove-keywords` はオンにするだけ。`--keep-names` でプリセットの
//!   ファイル名の規則を使わない）。
//! - `--rename`・`--text`・`--seq-start`・`--seq-digits`（EXP-02）: ファイル名の規則（取り込みのリネームと同じ
//!   テンプレート。仮想コピーは「-名前」を足す）。
//! - `--ppi`・`--output-sharpen`・`--sharpen-amount`・`--no-output-sharpen`（EXP-03）: 解像度と出力シャープ
//!   （[`apply_finish_args`]）。`--watermark-*`・`--no-watermark`（EXP-03）: 透かし（[`apply_watermark_args`]。
//!   プリセットの透かしの一部だけを変えることもできる）。`--skip-videos`（EXP-05）: 動画を書き出さない（既定は元の
//!   ファイルのまま再エンコードせずにコピーする）。

use std::path::PathBuf;

use genzo_api::{
    ExportOutcome, ExportPresetSettings, ExportReport, JobResult, PresetKind, RenameRule,
    RenderBackend,
};
use genzo_model::{
    ExportFormat, ExportSettings, ExportSize, OutputColorSpace, OutputSharpening, VideoExport,
    Watermark, WatermarkContent,
};
use serde_json::json;

use crate::Status;
use crate::args::{
    ConflictArg, ExportArgs, ExportOpts, FinishArgs, FormatArg, GlobalArgs, WatermarkArgs,
};
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
        on_conflict: opts.on_conflict.unwrap_or(ConflictArg::Sequence).into(),
        remove_keywords: opts.remove_keywords,
        ..ExportSettings::default()
    };
    settings
        .validate()
        .map_err(|e| CliError::Usage(e.to_string()))?;
    Ok(settings)
}

/// 解像度・出力シャープ・動画のオプション（EXP-03・EXP-05）を `settings`（プリセットの値か既定値）に当てる。
///
/// `--sharpen-amount` だけなら、今の出力シャープ（プリセットの値）の適用量を変える（出力シャープがなければ使い方の誤り）。
pub fn apply_finish_args(settings: &mut ExportSettings, f: &FinishArgs) -> CliResult<()> {
    if let Some(p) = f.ppi {
        settings.resolution_ppi = p;
    }
    if f.no_output_sharpen {
        if f.sharpen_amount.is_some() {
            return Err(CliError::Usage(
                "--no-output-sharpen と --sharpen-amount は一緒に使えません".to_owned(),
            ));
        }
        settings.output_sharpening = None;
    }
    match (f.output_sharpen, f.sharpen_amount) {
        (Some(media), amount) => {
            settings.output_sharpening = Some(OutputSharpening {
                media: media.into(),
                amount: amount
                    .map(Into::into)
                    .or(settings.output_sharpening.map(|s| s.amount))
                    .unwrap_or_default(),
            });
        }
        (None, Some(amount)) => match &mut settings.output_sharpening {
            Some(s) => s.amount = amount.into(),
            None => {
                return Err(CliError::Usage(
                    "--sharpen-amount は --output-sharpen と一緒に指定してください".to_owned(),
                ));
            }
        },
        (None, None) => {}
    }
    if f.skip_videos {
        settings.videos = VideoExport::Skip;
    }
    Ok(())
}

/// パスを絶対パスにする（透かしのファイルはプリセットに保存し、ワーカーが読むため）。
fn absolute(p: &std::path::Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// 透かしのオプション（EXP-03）を `settings`（プリセットの値か既定値）に当てる。
///
/// - `--watermark-text`（フォントは `--watermark-font`。なければ今の文字の透かしのフォント）か `--watermark-image` で、
///   新しい透かしにする（位置・不透明度・余白は今の透かしがあればその値、大きさは同じ種類のときだけその値）。
/// - 中身を指定せずにフォント・色・位置・大きさ・不透明度・余白だけを指定したら、今の透かしのその項目だけを変える
///   （透かしがなければ使い方の誤り）。
/// - `--no-watermark` で透かしを付けない。
pub fn apply_watermark_args(settings: &mut ExportSettings, a: &WatermarkArgs) -> CliResult<()> {
    if a.no_watermark {
        if a.has_adjustments() {
            return Err(CliError::Usage(
                "--no-watermark とほかの透かしのオプションは一緒に使えません".to_owned(),
            ));
        }
        settings.watermark = None;
        return Ok(());
    }
    settings.watermark = watermark_from_args(settings.watermark.take(), a)?;
    Ok(())
}

/// 透かしのオプションと今の透かし（あれば）から透かしを作る（[`apply_watermark_args`]）。
pub fn watermark_from_args(
    current: Option<Watermark>,
    a: &WatermarkArgs,
) -> CliResult<Option<Watermark>> {
    let current_text = match &current {
        Some(Watermark {
            content:
                WatermarkContent::Text {
                    font,
                    font_index,
                    color,
                    ..
                },
            ..
        }) => Some((font.clone(), *font_index, *color)),
        _ => None,
    };
    let content = if let Some(text) = &a.watermark_text {
        let font = match (&a.watermark_font, &current_text) {
            (Some(p), _) => absolute(p),
            (None, Some((f, _, _))) => f.clone(),
            (None, None) => {
                return Err(CliError::Usage(
                    "--watermark-text には --watermark-font でフォントのファイルを指定してください（OS のフォントのフォルダは genzo watermark font-dirs）"
                        .to_owned(),
                ));
            }
        };
        Some(WatermarkContent::Text {
            text: text.replace("\\n", "\n"),
            font,
            font_index: a
                .watermark_font_index
                .or(current_text.as_ref().map(|t| t.1))
                .unwrap_or(0),
            color: a
                .watermark_color
                .or(current_text.as_ref().map(|t| t.2))
                .unwrap_or_default(),
        })
    } else if let Some(img) = &a.watermark_image {
        if a.watermark_font.is_some()
            || a.watermark_font_index.is_some()
            || a.watermark_color.is_some()
        {
            return Err(CliError::Usage(
                "--watermark-font・--watermark-font-index・--watermark-color は文字の透かし（--watermark-text）にだけ使えます"
                    .to_owned(),
            ));
        }
        Some(WatermarkContent::Image {
            path: absolute(img),
        })
    } else {
        None
    };
    let mut w = match (content, current) {
        (Some(c), Some(cur)) => {
            let same_kind = std::mem::discriminant(&c) == std::mem::discriminant(&cur.content);
            Watermark {
                size: if same_kind {
                    cur.size
                } else {
                    Watermark::new(c.clone()).size
                },
                content: c,
                ..cur
            }
        }
        (Some(c), None) => Watermark::new(c),
        (None, Some(mut cur)) => {
            if let WatermarkContent::Text {
                font,
                font_index,
                color,
                ..
            } = &mut cur.content
            {
                if let Some(p) = &a.watermark_font {
                    *font = absolute(p);
                }
                if let Some(i) = a.watermark_font_index {
                    *font_index = i;
                }
                if let Some(c) = a.watermark_color {
                    *color = c;
                }
            } else if a.watermark_font.is_some()
                || a.watermark_font_index.is_some()
                || a.watermark_color.is_some()
            {
                return Err(CliError::Usage(
                    "--watermark-font・--watermark-font-index・--watermark-color は文字の透かしにだけ使えます（プリセットの透かしは画像）"
                        .to_owned(),
                ));
            }
            cur
        }
        (None, None) => {
            if a.has_adjustments() {
                return Err(CliError::Usage(
                    "透かしの中身を --watermark-text か --watermark-image で指定してください"
                        .to_owned(),
                ));
            }
            return Ok(None);
        }
    };
    if let Some(p) = a.watermark_position {
        w.anchor = p.into();
    }
    if let Some(v) = a.watermark_size {
        w.size = v;
    }
    if let Some(v) = a.watermark_opacity {
        w.opacity = v;
    }
    if let Some(v) = a.watermark_margin {
        w.margin = v;
    }
    if let Some(p) = w.problem() {
        return Err(CliError::Usage(p));
    }
    Ok(Some(w))
}

/// `export` の書き出しの設定・書き出し先・ファイル名の規則（プリセットとオプションから）。
#[derive(Debug, Clone, PartialEq)]
pub struct ExportPlan {
    /// 書き出しの設定（色空間は `color_space_given` が偽なら、カタログの既定に置き換える）。
    pub settings: ExportSettings,
    /// 色空間を指定したか（オプションかプリセットで）。
    pub color_space_given: bool,
    /// 書き出し先のフォルダ。
    pub dest: PathBuf,
    /// ファイル名の規則（`None` なら元のファイル名）。
    pub naming: Option<RenameRule>,
}

/// プリセット（あれば）とオプションから、書き出しの設定を決める（モジュールの doc）。
///
/// - 形式: `--format` があればそれ（JPEG の品質は `--quality`、なければプリセットが JPEG ならその品質、なければ
///   既定の 90）。なければプリセットの形式（`--quality` は JPEG のときだけ）。どちらもなければ JPEG。
/// - 色空間・長辺・同じ名前の扱い: オプションがあればそれ、なければプリセットの値。
/// - GPS・キーワードの削除: オプションかプリセットのどちらかがオンならオン。
/// - 書き出し先: `--out`、なければプリセットの書き出し先（どちらもなければ使い方の誤り）。
/// - ファイル名: `--keep-names` なら元のファイル名、`--rename` があればそれ、なければプリセットの規則
///   （`--text`・`--seq-start`・`--seq-digits` で一部を変えられる）。
pub fn export_plan(a: &ExportArgs, preset: Option<&ExportPresetSettings>) -> CliResult<ExportPlan> {
    let base = preset.map(|p| p.export.clone()).unwrap_or_default();
    let base_quality = match base.format {
        ExportFormat::Jpeg { quality } if preset.is_some() => Some(quality),
        _ => None,
    };
    let format = match a.opts.format {
        Some(f) => {
            if a.opts.quality.is_some() && f != FormatArg::Jpeg {
                return Err(CliError::Usage(
                    "--quality は JPEG の書き出しのときだけ指定できます".to_owned(),
                ));
            }
            f.to_format(a.opts.quality.or(base_quality))
        }
        None => match (base.format, a.opts.quality) {
            (ExportFormat::Jpeg { .. }, Some(q)) => ExportFormat::Jpeg { quality: q },
            (_, Some(_)) => {
                return Err(CliError::Usage(
                    "--quality は JPEG の書き出しのときだけ指定できます（プリセットの形式は JPEG ではありません）"
                        .to_owned(),
                ));
            }
            (f, None) => f,
        },
    };
    let mut settings = ExportSettings {
        format,
        color_space: a
            .opts
            .color_space
            .map_or(base.color_space, OutputColorSpace::from),
        size: a.opts.long_edge.map_or(base.size, ExportSize::LongEdge),
        remove_gps: a.opts.remove_gps || base.remove_gps,
        on_conflict: a.opts.on_conflict.map_or(base.on_conflict, Into::into),
        remove_keywords: a.opts.remove_keywords || base.remove_keywords,
        ..base
    };
    apply_finish_args(&mut settings, &a.finish)?;
    apply_watermark_args(&mut settings, &a.watermark)?;
    settings
        .validate()
        .map_err(|e| CliError::Usage(e.to_string()))?;
    let dest = a
        .out
        .clone()
        .or_else(|| preset.and_then(|p| p.destination.clone()))
        .ok_or_else(|| {
            CliError::Usage(
                "--out に書き出し先のフォルダを指定してください（書き出し先のある --preset でも省略できます）"
                    .to_owned(),
            )
        })?;
    let naming = if a.keep_names {
        None
    } else {
        crate::preset::naming_rule(&a.naming, preset.and_then(|p| p.file_naming.as_ref()))?
    };
    Ok(ExportPlan {
        settings,
        color_space_given: a.opts.color_space.is_some() || preset.is_some(),
        dest,
        naming,
    })
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

/// `export`。一部でも失敗したら終了コード 3（[`crate::EXIT_PARTIAL`]。結果の一覧は出す）。
pub fn run(g: &GlobalArgs, out: Output, a: ExportArgs) -> CliResult<Status> {
    let ids = unique_ids(&a.variants);
    // 書き出しプリセット（EXP-02。カタログは使わない）。
    let preset = match &a.preset {
        Some(r) => {
            let store = crate::preset::store(g);
            let info = crate::preset::resolve(&store, Some(PresetKind::Export), r, None)?;
            let (info, settings) = store.export_settings(&info.id)?;
            // この PC で使えない値（別の OS のパス）は使わない（書き出し先は --out、透かしのファイルは
            // --watermark-font・--watermark-image で指定する）。
            for w in &info.warnings {
                out.warn(format!("書き出しプリセット「{}」: {w}", info.name));
            }
            Some((info, settings))
        }
        None => None,
    };
    // 使い方の誤り（--quality と形式の組み合わせ・書き出し先がないなど）は、カタログを開く前に確かめる。色空間の
    // 指定がなければ、開いた後にカタログの設定（書き出しの既定の色空間）にする。
    let mut plan = export_plan(&a, preset.as_ref().map(|(_, s)| s))?;
    // 書き出し先はフォルダ（既にあるファイルを指定した場合は、作れないフォルダとして分かりにくい
    // 入出力のエラーになるため、先に使い方の誤りにする）。
    if plan.dest.exists() && !plan.dest.is_dir() {
        return Err(CliError::Usage(format!(
            "--out には書き出し先のフォルダを指定してください（{} はフォルダではありません）",
            plan.dest.display()
        )));
    }
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &ids)?;
    if !plan.color_space_given {
        plan.settings.color_space = s.core.settings().default_export_color_space;
    }
    let job = s
        .core
        .export_with(&ids, &plan.settings, &plan.dest, plan.naming.as_ref())?;
    let info = s.wait_job_ok(job, "書き出し")?;
    let Some(JobResult::Export(report)) = info.result else {
        return Err(CliError::other("書き出しの結果がありません"));
    };
    s.close()?;
    let failed = report.failed > 0 || report.cancelled;
    print_report(out, &plan, preset.as_ref().map(|(p, _)| p), &report);
    Ok(if failed {
        Status::Partial
    } else {
        Status::Success
    })
}

fn print_report(
    out: Output,
    plan: &ExportPlan,
    preset: Option<&genzo_api::PresetInfo>,
    report: &ExportReport,
) {
    let settings = &plan.settings;
    if out.json {
        out.print_json(&json!({
            "settings": settings,
            "file_naming": plan.naming,
            "preset": preset.map(|p| json!({"id": p.id, "name": p.name})),
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
            ExportOutcome::Copied { path, warnings, .. } => t.row([
                item.variant_id.to_string(),
                "動画のコピー".to_owned(),
                String::new(),
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
        "{}・{}: 書き出し {} 件、動画のコピー {} 件、スキップ {} 件、失敗 {} 件{}（{}）",
        format_text(settings.format),
        color_space_text(settings.color_space),
        report.written,
        report.copied,
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
    use genzo_model::ConflictPolicy;

    fn opts() -> ExportOpts {
        ExportOpts {
            format: None,
            quality: None,
            color_space: None,
            long_edge: None,
            remove_gps: false,
            remove_keywords: false,
            on_conflict: None,
        }
    }

    fn args(o: ExportOpts, out: Option<&str>) -> ExportArgs {
        ExportArgs {
            variants: vec![genzo_api::VariantId::new(1)],
            out: out.map(PathBuf::from),
            preset: None,
            opts: o,
            naming: crate::args::NamingArgs::default(),
            keep_names: false,
            finish: FinishArgs::default(),
            watermark: WatermarkArgs::default(),
        }
    }

    /// `--preset` の中身とオプションの合わせ方（EXP-02）。
    #[test]
    fn plans_merge_presets_and_options() {
        // プリセットなし: 今までどおり（書き出し先は必須）。
        let p = export_plan(&args(opts(), Some("/o")), None).unwrap();
        assert_eq!(p.settings, ExportSettings::default());
        assert!(!p.color_space_given);
        assert_eq!(p.naming, None);
        assert!(matches!(
            export_plan(&args(opts(), None), None),
            Err(CliError::Usage(_))
        ));
        let preset = ExportPresetSettings {
            export: ExportSettings {
                format: ExportFormat::Jpeg { quality: 70 },
                color_space: OutputColorSpace::DisplayP3,
                size: ExportSize::LongEdge(2048),
                remove_gps: true,
                on_conflict: ConflictPolicy::Skip,
                remove_keywords: false,
                ..ExportSettings::default()
            },
            destination: Some(PathBuf::from("/web")),
            file_naming: Some(RenameRule {
                template: "{date}-{seq}".to_owned(),
                ..RenameRule::default()
            }),
        };
        // プリセットの値をそのまま使う（書き出し先も）。
        let p = export_plan(&args(opts(), None), Some(&preset)).unwrap();
        assert_eq!(p.settings, preset.export);
        assert!(p.color_space_given);
        assert_eq!(p.dest, PathBuf::from("/web"));
        assert_eq!(p.naming, preset.file_naming);
        // 指定したオプションが優先する。JPEG の品質だけを変えられる。
        let o = ExportOpts {
            quality: Some(95),
            long_edge: Some(800),
            remove_keywords: true,
            on_conflict: Some(ConflictArg::Overwrite),
            ..opts()
        };
        let mut a = args(o, Some("/other"));
        a.keep_names = true;
        let p = export_plan(&a, Some(&preset)).unwrap();
        assert_eq!(p.settings.format, ExportFormat::Jpeg { quality: 95 });
        assert_eq!(p.settings.size, ExportSize::LongEdge(800));
        assert!(p.settings.remove_gps && p.settings.remove_keywords);
        assert_eq!(p.settings.on_conflict, ConflictPolicy::Overwrite);
        assert_eq!(p.dest, PathBuf::from("/other"));
        assert_eq!(p.naming, None);
        // --format jpeg だけならプリセットの品質のまま。PNG のプリセットに --quality は使い方の誤り。
        let p = export_plan(
            &args(
                ExportOpts {
                    format: Some(FormatArg::Jpeg),
                    ..opts()
                },
                None,
            ),
            Some(&preset),
        )
        .unwrap();
        assert_eq!(p.settings.format, ExportFormat::Jpeg { quality: 70 });
        let png = ExportPresetSettings {
            export: ExportSettings {
                format: ExportFormat::Png16,
                ..ExportSettings::default()
            },
            ..preset.clone()
        };
        assert!(matches!(
            export_plan(
                &args(
                    ExportOpts {
                        quality: Some(80),
                        ..opts()
                    },
                    None
                ),
                Some(&png)
            ),
            Err(CliError::Usage(_))
        ));
    }

    /// 出力シャープ・透かしのオプション（EXP-03）とプリセットの値の合わせ方。
    #[test]
    fn finishing_options_merge_with_the_preset() {
        use crate::args::{SharpenAmountArg, SharpenMediaArg, WatermarkPositionArg};
        use genzo_model::{RgbColor, SharpenAmount, SharpenMedia, WatermarkAnchor};
        let mut s = ExportSettings::default();
        apply_finish_args(
            &mut s,
            &FinishArgs {
                ppi: Some(240),
                output_sharpen: Some(SharpenMediaArg::Glossy),
                skip_videos: true,
                ..FinishArgs::default()
            },
        )
        .unwrap();
        assert_eq!(s.resolution_ppi, 240);
        assert_eq!(
            s.output_sharpening,
            Some(OutputSharpening {
                media: SharpenMedia::GlossyPaper,
                amount: SharpenAmount::Standard,
            })
        );
        assert_eq!(s.videos, VideoExport::Skip);
        // 適用量だけを変える（対象はプリセットのまま）。
        apply_finish_args(
            &mut s,
            &FinishArgs {
                sharpen_amount: Some(SharpenAmountArg::Low),
                ..FinishArgs::default()
            },
        )
        .unwrap();
        assert_eq!(s.output_sharpening.unwrap().amount, SharpenAmount::Low);
        assert_eq!(
            s.output_sharpening.unwrap().media,
            SharpenMedia::GlossyPaper
        );
        apply_finish_args(
            &mut s,
            &FinishArgs {
                no_output_sharpen: true,
                ..FinishArgs::default()
            },
        )
        .unwrap();
        assert_eq!(s.output_sharpening, None);
        assert!(
            apply_finish_args(
                &mut s,
                &FinishArgs {
                    sharpen_amount: Some(SharpenAmountArg::High),
                    ..FinishArgs::default()
                }
            )
            .is_err()
        );

        // 透かし: 文字（フォント必須）→ 一部だけ変える → 画像に替える（位置は保つ、大きさは種類の既定）→ 消す。
        let font = std::env::temp_dir().join("f.ttf");
        let text = WatermarkArgs {
            watermark_text: Some("1 行目\\n2 行目".to_owned()),
            watermark_font: Some(font.clone()),
            watermark_color: Some(RgbColor([1, 2, 3])),
            watermark_position: Some(WatermarkPositionArg::Top),
            ..WatermarkArgs::default()
        };
        apply_watermark_args(&mut s, &text).unwrap();
        let w = s.watermark.clone().unwrap();
        assert_eq!(w.anchor, WatermarkAnchor::Top);
        assert_eq!(w.size, Watermark::DEFAULT_TEXT_SIZE);
        match &w.content {
            WatermarkContent::Text {
                text,
                font: f,
                color,
                ..
            } => {
                assert_eq!(text, "1 行目\n2 行目", "\\n は改行");
                assert_eq!(f, &font);
                assert_eq!(*color, RgbColor([1, 2, 3]));
            }
            other => panic!("{other:?}"),
        }
        apply_watermark_args(
            &mut s,
            &WatermarkArgs {
                watermark_opacity: Some(25),
                watermark_color: Some(RgbColor::WHITE),
                ..WatermarkArgs::default()
            },
        )
        .unwrap();
        let w = s.watermark.clone().unwrap();
        assert_eq!(w.opacity, 25);
        assert!(
            matches!(w.content, WatermarkContent::Text { color, .. } if color == RgbColor::WHITE)
        );
        // 文字だけ替える（フォント・色はそのまま）。
        apply_watermark_args(
            &mut s,
            &WatermarkArgs {
                watermark_text: Some("新しい".to_owned()),
                ..WatermarkArgs::default()
            },
        )
        .unwrap();
        assert!(
            matches!(&s.watermark.as_ref().unwrap().content, WatermarkContent::Text { text, font: f, .. } if text == "新しい" && f == &font)
        );
        let logo = std::env::temp_dir().join("logo.png");
        apply_watermark_args(
            &mut s,
            &WatermarkArgs {
                watermark_image: Some(logo.clone()),
                ..WatermarkArgs::default()
            },
        )
        .unwrap();
        let w = s.watermark.clone().unwrap();
        assert_eq!(w.anchor, WatermarkAnchor::Top);
        assert_eq!(w.opacity, 25);
        assert_eq!(w.size, Watermark::DEFAULT_IMAGE_SIZE);
        assert_eq!(w.content, WatermarkContent::Image { path: logo });
        // 画像の透かしにフォントの項目は使えない。
        assert!(
            apply_watermark_args(
                &mut s.clone(),
                &WatermarkArgs {
                    watermark_color: Some(RgbColor::WHITE),
                    ..WatermarkArgs::default()
                }
            )
            .is_err()
        );
        apply_watermark_args(
            &mut s,
            &WatermarkArgs {
                no_watermark: true,
                ..WatermarkArgs::default()
            },
        )
        .unwrap();
        assert_eq!(s.watermark, None);
        // 透かしがないのに調整だけ・文字にフォントがない。
        for a in [
            WatermarkArgs {
                watermark_margin: Some(3),
                ..WatermarkArgs::default()
            },
            WatermarkArgs {
                watermark_text: Some("A".to_owned()),
                ..WatermarkArgs::default()
            },
        ] {
            assert!(apply_watermark_args(&mut s, &a).is_err(), "{a:?}");
        }
        // 何も指定しなければ変えない。
        apply_watermark_args(&mut s, &WatermarkArgs::default()).unwrap();
        assert_eq!(s.watermark, None);
    }

    #[test]
    fn settings_from_options() {
        let s = export_settings(&opts(), FormatArg::Jpeg, OutputColorSpace::Srgb).unwrap();
        assert_eq!(s, ExportSettings::default());
        let o = ExportOpts {
            quality: Some(70),
            long_edge: Some(800),
            remove_gps: true,
            remove_keywords: true,
            on_conflict: Some(ConflictArg::Skip),
            ..opts()
        };
        let s = export_settings(&o, FormatArg::Jpeg, OutputColorSpace::AdobeRgb).unwrap();
        assert_eq!(s.format, ExportFormat::Jpeg { quality: 70 });
        assert_eq!(s.size, ExportSize::LongEdge(800));
        assert!(s.remove_gps);
        assert!(s.remove_keywords);
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
