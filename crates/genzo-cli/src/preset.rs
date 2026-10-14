//! `preset`（現像プリセット DEV-29・書き出しプリセット EXP-02。genzo-api の `presets`）と、`export --preset` の
//! プリセットの解決。
//!
//! - プリセットのフォルダは `--presets-dir`、環境変数 `GENZO_PRESETS_DIR`、なければアプリと同じ場所
//!   （[`crate::presets_dir`]）。一覧・中身の表示・名前の変更・移動・削除・ファイルの読み込みと書き出し・書き出し
//!   プリセットの作成と更新はカタログを使わない。写真の設定を使うもの（`create-develop`・`update-develop`・`apply`）
//!   はカタログを開く。
//! - プリセットは ID（16 桁の 16 進数）か名前で指定する（[`resolve`]）。名前は全角・半角と大文字・小文字を区別せずに
//!   比べ、同じ名前が複数あれば（別のフォルダ・別の種類）、`--folder`・`--kind` で絞るよう求める（使い方の誤り）。
//! - JSON の出力: `list` はプリセットの配列（genzo-api の `PresetInfo`）。`show` は `{"preset", "settings"}`
//!   （`settings` は現像プリセットの設定。含めない項目は既定値。書き出しプリセットは `preset.export`）。
//!   作成・更新・名前の変更・移動は `{"preset"}`。`apply` は `{"preset", "updated", "upgraded", "needs_upgrade"}`。
//!   `import` は `{"imported", "failed"}`（読み込めなかったファイルがあれば終了コード 3）。`export-files` は
//!   `{"files"}`。`dir` は `{"dir"}`。

use std::path::PathBuf;

use genzo_api::{
    ExportPresetSettings, PasteVersion, PresetInfo, PresetKind, PresetStore, RenameRule,
    SettingGroups, is_valid_preset_id,
};
use genzo_model::OutputColorSpace;
use serde_json::json;

use crate::Status;
use crate::args::{
    ExportPresetArgs, FormatArg, GlobalArgs, NamingArgs, PresetCommand, setting_groups,
};
use crate::error::{CliError, CliResult};
use crate::library::{ensure_variants, unique_ids};
use crate::output::{Output, Table, color_space_text};
use crate::session::{OpenOptions, Session};

/// プリセットの保存場所（[`crate::presets_dir`]）。
pub fn store(g: &GlobalArgs) -> PresetStore {
    PresetStore::new(crate::presets_dir(g))
}

/// 名前の比較キー（genzo-api の `presets` と同じ: NFKC ＋ 小文字化）。
fn key(s: &str) -> String {
    genzo_catalog::text::search_key(s.trim())
}

/// プリセットを ID か名前で探す（`kind` が `None` なら両方の種類、`folder` があればそのフォルダだけ）。見つからない・
/// 複数あれば使い方の誤り（候補を挙げる）。
pub fn resolve(
    store: &PresetStore,
    kind: Option<PresetKind>,
    reference: &str,
    folder: Option<&str>,
) -> CliResult<PresetInfo> {
    let kinds = match kind {
        Some(k) => vec![k],
        None => vec![PresetKind::Develop, PresetKind::Export],
    };
    let mut all = Vec::new();
    for k in kinds {
        all.extend(store.list(k)?);
    }
    let r = reference.trim();
    if is_valid_preset_id(r)
        && let Some(p) = all.iter().find(|p| p.id == r)
    {
        return Ok(p.clone());
    }
    let matches: Vec<&PresetInfo> = all
        .iter()
        .filter(|p| key(&p.name) == key(r))
        .filter(|p| folder.is_none_or(|f| key(&p.folder) == key(f)))
        .collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(CliError::Api(genzo_api::ApiError::NotFound(format!(
            "プリセット「{r}」（genzo preset list で一覧を見られます）"
        )))),
        many => Err(CliError::Usage(format!(
            "「{r}」という名前のプリセットが複数あります。ID か --folder・--kind で指定してください: {}",
            many.iter()
                .map(|p| format!("{}（{}・{}）", p.id, p.kind.label(), folder_text(&p.folder)))
                .collect::<Vec<_>>()
                .join("、")
        ))),
    }
}

/// フォルダの表示（空なら「フォルダなし」）。
fn folder_text(f: &str) -> String {
    if f.is_empty() {
        "フォルダなし".to_owned()
    } else {
        f.to_owned()
    }
}

/// 含める項目の表示（「基本補正・外観」など）。
pub fn groups_text(g: &SettingGroups) -> String {
    let items = [
        (g.white_balance, "ホワイトバランス"),
        (g.basic_tone, "基本補正"),
        (g.presence, "外観"),
        (g.tone_curve, "トーンカーブ"),
        (g.color, "色"),
        (g.detail, "ディテール"),
        (g.lens, "レンズ補正"),
        (g.effects, "効果"),
        (g.profile, "プロファイル"),
        (g.masks, "マスク"),
        (g.geometry, "切り抜きと角度"),
    ];
    if g.is_all() {
        return "すべて".to_owned();
    }
    items
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, n)| *n)
        .collect::<Vec<_>>()
        .join("・")
}

/// 書き出しプリセットの中身の表示（「PNG 8bit・sRGB・長辺 2048・名前 {date}-{seq}・書き出し先 /out」など）。
pub fn export_text(e: &ExportPresetSettings) -> String {
    let mut parts = vec![
        crate::export::format_text(e.export.format),
        color_space_text(e.export.color_space).to_owned(),
    ];
    if let genzo_model::ExportSize::LongEdge(n) = e.export.size {
        parts.push(format!("長辺 {n}"));
    }
    if e.export.remove_gps {
        parts.push("GPS を削除".to_owned());
    }
    if e.export.remove_keywords {
        parts.push("キーワードなし".to_owned());
    }
    if let Some(r) = &e.file_naming {
        parts.push(format!("名前 {}", r.template));
    }
    if let Some(d) = &e.destination {
        parts.push(format!("書き出し先 {}", d.display()));
    }
    parts.join("・")
}

/// プリセットの中身の要約（一覧の列）。
fn summary(p: &PresetInfo) -> String {
    if let Some(problem) = &p.problem {
        return format!("使えません: {problem}");
    }
    match (p.kind, &p.groups, &p.export) {
        (PresetKind::Develop, Some(g), _) => format!(
            "{}（処理バージョン {}）",
            groups_text(g),
            p.process_version.unwrap_or_default()
        ),
        (PresetKind::Export, _, Some(e)) if p.warnings.is_empty() => export_text(e),
        (PresetKind::Export, _, Some(e)) => {
            format!("{}（注意: {}）", export_text(e), p.warnings.join(" / "))
        }
        _ => String::new(),
    }
}

/// ファイル名の規則（`--rename` があればそれ、なければ `base`（プリセットの規則）に `--text`・`--seq-start`・
/// `--seq-digits` を当てたもの）。どちらもなければ `None`（`--text` などだけを指定したら使い方の誤り）。
pub fn naming_rule(a: &NamingArgs, base: Option<&RenameRule>) -> CliResult<Option<RenameRule>> {
    let rule = match (&a.rename, base) {
        (Some(t), _) => RenameRule {
            template: t.clone(),
            custom_text: a.text.clone().unwrap_or_default(),
            sequence_start: a.seq_start.unwrap_or(1),
            sequence_digits: a.seq_digits.unwrap_or(4),
        },
        (None, Some(b)) => RenameRule {
            template: b.template.clone(),
            custom_text: a.text.clone().unwrap_or_else(|| b.custom_text.clone()),
            sequence_start: a.seq_start.unwrap_or(b.sequence_start),
            sequence_digits: a.seq_digits.unwrap_or(b.sequence_digits),
        },
        (None, None) => {
            if a.text.is_some() || a.seq_start.is_some() || a.seq_digits.is_some() {
                return Err(CliError::Usage(
                    "--text・--seq-start・--seq-digits は --rename（またはファイル名の規則のある書き出しプリセット）と一緒に指定してください"
                        .to_owned(),
                ));
            }
            return Ok(None);
        }
    };
    rule.parse().map_err(|e| CliError::Usage(e.to_string()))?;
    Ok(Some(rule))
}

/// `preset create-export`・`update-export` の中身（指定しなかった項目は既定値。色空間の既定は sRGB）。
fn export_preset_settings(a: &ExportPresetArgs) -> CliResult<ExportPresetSettings> {
    let mut export = crate::export::export_settings(
        &a.opts,
        a.opts.format.unwrap_or(FormatArg::Jpeg),
        a.opts
            .color_space
            .map_or(OutputColorSpace::Srgb, OutputColorSpace::from),
    )?;
    // 解像度・出力シャープ・透かし・動画（EXP-03・EXP-05）。
    crate::export::apply_finish_args(&mut export, &a.finish)?;
    crate::export::apply_watermark_args(&mut export, &a.watermark)?;
    let destination = a
        .dest
        .as_ref()
        .map(|d| std::path::absolute(d).unwrap_or_else(|_| d.clone()));
    let settings = ExportPresetSettings {
        export,
        destination,
        file_naming: naming_rule(&a.naming, None)?,
    };
    settings
        .validate()
        .map_err(|e| CliError::Usage(e.to_string()))?;
    Ok(settings)
}

/// `preset` を実行する。
pub fn run(g: &GlobalArgs, out: Output, cmd: PresetCommand) -> CliResult<Status> {
    let st = store(g);
    match cmd {
        PresetCommand::Dir => {
            let Some(dir) = st.root() else {
                return Err(CliError::Usage(
                    "プリセットのフォルダが分かりません（--presets-dir か環境変数 GENZO_PRESETS_DIR で指定してください）"
                        .to_owned(),
                ));
            };
            if out.json {
                out.print_json(&json!({ "dir": dir }));
            } else {
                out.line(dir.display().to_string());
            }
            Ok(Status::Success)
        }
        PresetCommand::List { kind } => {
            let kinds = match kind {
                Some(k) => vec![k.into()],
                None => vec![PresetKind::Develop, PresetKind::Export],
            };
            let mut all = Vec::new();
            for k in kinds {
                all.extend(st.list(k)?);
            }
            if out.json {
                out.print_json(&all);
            } else if all.is_empty() {
                out.line("プリセットはありません");
            } else {
                let mut t = Table::new(["種類", "ID", "フォルダ", "名前", "内容"]);
                for p in &all {
                    t.row([
                        p.kind.label().to_owned(),
                        p.id.clone(),
                        p.folder.clone(),
                        p.name.clone(),
                        summary(p),
                    ]);
                }
                out.table(&t);
            }
            Ok(Status::Success)
        }
        PresetCommand::Show {
            preset,
            kind,
            folder,
        } => {
            let p = resolve(&st, kind.map(Into::into), &preset, folder.as_deref())?;
            let settings = match (p.kind, p.problem.is_none()) {
                (PresetKind::Develop, true) => Some(st.develop_settings(&p.id)?.2),
                _ => None,
            };
            if out.json {
                out.print_json(&json!({ "preset": p, "settings": settings }));
            } else {
                out.line(crate::output::key_values(&[
                    ("種類", p.kind.label().to_owned()),
                    ("ID", p.id.clone()),
                    ("名前", p.name.clone()),
                    ("フォルダ", folder_text(&p.folder)),
                    ("内容", summary(&p)),
                    (
                        "変更",
                        p.updated_at
                            .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                            .unwrap_or_default(),
                    ),
                ]));
                if let Some(s) = settings {
                    out.line(serde_json::to_string_pretty(&s).unwrap_or_else(|e| e.to_string()));
                }
            }
            Ok(Status::Success)
        }
        PresetCommand::CreateDevelop {
            name,
            from,
            folder,
            groups,
        } => {
            let groups = if groups.is_empty() {
                SettingGroups::PRESET_DEFAULT
            } else {
                setting_groups(&groups)
            };
            let s = Session::open(g, out, &OpenOptions::default())?;
            ensure_variants(&s, &[from])?;
            let r = s.core.create_develop_preset(&name, &folder, groups, from);
            s.close()?;
            print_saved(out, &r?, "作りました");
            Ok(Status::Success)
        }
        PresetCommand::UpdateDevelop {
            preset,
            from,
            groups,
            folder,
        } => {
            let p = resolve(&st, Some(PresetKind::Develop), &preset, folder.as_deref())?;
            let groups = if groups.is_empty() {
                p.groups.unwrap_or(SettingGroups::PRESET_DEFAULT)
            } else {
                setting_groups(&groups)
            };
            let s = Session::open(g, out, &OpenOptions::default())?;
            ensure_variants(&s, &[from])?;
            let r = s.core.update_develop_preset(&p.id, groups, from);
            s.close()?;
            print_saved(out, &r?, "更新しました");
            Ok(Status::Success)
        }
        PresetCommand::Apply {
            preset,
            to,
            upgrade_process_version,
            folder,
        } => {
            let p = resolve(&st, Some(PresetKind::Develop), &preset, folder.as_deref())?;
            let to = unique_ids(&to);
            let version = if upgrade_process_version {
                PasteVersion::Upgrade
            } else {
                PasteVersion::Keep
            };
            let s = Session::open(g, out, &OpenOptions::default())?;
            ensure_variants(&s, &to)?;
            let r = s.core.apply_develop_preset(&p.id, &to, version);
            s.close()?;
            let r = r?;
            if r.needs_upgrade > 0 {
                out.warn(format!(
                    "{} 件は処理バージョンが古いため、プリセットの項目の一部（レンズ補正・ハイライト復元・ノイズ軽減・カメラプロファイル・色の調整・明瞭度・シャープ・効果・マスクなど）は使われません\
                     （--upgrade-process-version を付けて適用するか、genzo develop upgrade で更新できます）",
                    r.needs_upgrade
                ));
            }
            if r.skipped_videos > 0 {
                out.warn(format!(
                    "動画 {} 件には適用しませんでした（動画には現像設定を保存しません）",
                    r.skipped_videos
                ));
            }
            if out.json {
                out.print_json(&json!({
                    "preset": p,
                    "updated": r.count,
                    "upgraded": r.upgraded,
                    "needs_upgrade": r.needs_upgrade,
                    "skipped_videos": r.skipped_videos,
                }));
            } else {
                out.line(format!(
                    "プリセット「{}」を {} 件に適用しました{}",
                    p.name,
                    r.count,
                    if r.upgraded > 0 {
                        format!("（{} 件の処理バージョンを更新）", r.upgraded)
                    } else {
                        String::new()
                    }
                ));
            }
            Ok(Status::Success)
        }
        PresetCommand::CreateExport(a) => {
            let settings = export_preset_settings(&a)?;
            let p = st.create_export(&a.name, a.folder.as_deref().unwrap_or(""), &settings)?;
            print_saved(out, &p, "作りました");
            Ok(Status::Success)
        }
        PresetCommand::UpdateExport(a) => {
            let p = resolve(&st, Some(PresetKind::Export), &a.name, a.folder.as_deref())?;
            let settings = export_preset_settings(&a)?;
            let p = st.update_export(&p.id, &settings)?;
            print_saved(out, &p, "更新しました");
            Ok(Status::Success)
        }
        PresetCommand::Rename {
            preset,
            name,
            kind,
            folder,
        } => {
            let p = resolve(&st, kind.map(Into::into), &preset, folder.as_deref())?;
            let p = st.rename(p.kind, &p.id, &name)?;
            print_saved(out, &p, "名前を変えました");
            Ok(Status::Success)
        }
        PresetCommand::Move { preset, to, kind } => {
            let p = resolve(&st, kind.map(Into::into), &preset, None)?;
            let p = st.move_to(p.kind, &p.id, &to)?;
            print_saved(out, &p, "移しました");
            Ok(Status::Success)
        }
        PresetCommand::RenameFolder { kind, folder, name } => {
            let n = st.rename_folder(kind.into(), &folder, &name)?;
            if out.json {
                out.print_json(&json!({ "updated": n }));
            } else {
                out.line(format!(
                    "フォルダ「{}」の {n} 件を「{}」へ移しました",
                    folder.trim(),
                    folder_text(name.trim())
                ));
            }
            Ok(Status::Success)
        }
        PresetCommand::Delete {
            preset,
            kind,
            folder,
        } => {
            let p = resolve(&st, kind.map(Into::into), &preset, folder.as_deref())?;
            st.delete(p.kind, &p.id)?;
            if out.json {
                out.print_json(&json!({ "deleted": p }));
            } else {
                out.line(format!("{}「{}」を削除しました", p.kind.label(), p.name));
            }
            Ok(Status::Success)
        }
        PresetCommand::Import { files, kind } => {
            let r = st.import_files(&files, kind.map(Into::into))?;
            if out.json {
                out.print_json(&r);
            } else {
                for p in &r.imported {
                    out.line(format!(
                        "{}「{}」を読み込みました（{}・ID {}）",
                        p.kind.label(),
                        p.name,
                        folder_text(&p.folder),
                        p.id
                    ));
                }
                for f in &r.failed {
                    out.warn(format!(
                        "{} を読み込めません: {}",
                        f.path.display(),
                        f.message
                    ));
                }
            }
            Ok(if r.failed.is_empty() {
                Status::Success
            } else {
                Status::Partial
            })
        }
        PresetCommand::ExportFiles {
            presets,
            out: dir,
            kind,
        } => {
            let mut by_kind: Vec<(PresetKind, String)> = Vec::new();
            for r in &presets {
                let p = resolve(&st, kind.map(Into::into), r, None)?;
                by_kind.push((p.kind, p.id));
            }
            let mut files: Vec<PathBuf> = Vec::new();
            for (k, id) in by_kind {
                files.extend(st.export_files(k, std::slice::from_ref(&id), &dir)?);
            }
            if out.json {
                out.print_json(&json!({ "files": files }));
            } else {
                for f in &files {
                    out.line(f.display().to_string());
                }
            }
            Ok(Status::Success)
        }
    }
}

/// 保存したプリセットの表示。
fn print_saved(out: Output, p: &PresetInfo, done: &str) {
    if out.json {
        out.print_json(&json!({ "preset": p }));
    } else {
        out.line(format!(
            "{}「{}」を{done}（{}・ID {}）: {}",
            p.kind.label(),
            p.name,
            folder_text(&p.folder),
            p.id,
            summary(p)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genzo_model::DevelopSettings;

    fn naming(rename: Option<&str>, text: Option<&str>, start: Option<u32>) -> NamingArgs {
        NamingArgs {
            rename: rename.map(str::to_owned),
            text: text.map(str::to_owned),
            seq_start: start,
            seq_digits: None,
        }
    }

    #[test]
    fn naming_rules_from_options_and_presets() {
        assert_eq!(naming_rule(&naming(None, None, None), None).unwrap(), None);
        assert!(naming_rule(&naming(None, Some("x"), None), None).is_err());
        let r = naming_rule(&naming(Some("{date}-{seq}"), None, None), None)
            .unwrap()
            .unwrap();
        assert_eq!(r.template, "{date}-{seq}");
        assert_eq!((r.sequence_start, r.sequence_digits), (1, 4));
        // プリセットの規則に、指定した項目だけを当てる。
        let base = RenameRule {
            template: "{name}-{text}".to_owned(),
            custom_text: "旅".to_owned(),
            sequence_start: 5,
            sequence_digits: 2,
        };
        let r = naming_rule(&naming(None, None, Some(9)), Some(&base))
            .unwrap()
            .unwrap();
        assert_eq!(r.custom_text, "旅");
        assert_eq!((r.sequence_start, r.sequence_digits), (9, 2));
        assert!(naming_rule(&naming(Some("{x}"), None, None), None).is_err());
    }

    #[test]
    fn names_are_resolved_by_id_name_and_folder() {
        let d = tempfile::tempdir().unwrap();
        let st = PresetStore::new(Some(d.path().to_path_buf()));
        let s = DevelopSettings::default();
        let a = st
            .create_develop("Film", "人物", SettingGroups::ALL, &s)
            .unwrap();
        let b = st
            .create_develop("film", "風景", SettingGroups::ALL, &s)
            .unwrap();
        let e = st
            .create_export("Web", "", &ExportPresetSettings::default())
            .unwrap();
        assert_eq!(resolve(&st, None, &a.id, None).unwrap().id, a.id);
        assert_eq!(resolve(&st, None, "ｗｅｂ", None).unwrap().id, e.id);
        // 同じ名前が複数あれば、フォルダで絞る。
        assert!(matches!(
            resolve(&st, None, "FILM", None),
            Err(CliError::Usage(_))
        ));
        assert_eq!(resolve(&st, None, "FILM", Some("風景")).unwrap().id, b.id);
        assert!(matches!(
            resolve(&st, Some(PresetKind::Export), "Film", None),
            Err(CliError::Api(genzo_api::ApiError::NotFound(_)))
        ));
        assert_eq!(groups_text(&SettingGroups::ALL), "すべて");
        assert_eq!(
            groups_text(&SettingGroups::PRESET_DEFAULT),
            "ホワイトバランス・基本補正・外観・トーンカーブ・色・ディテール・レンズ補正・効果・プロファイル"
        );
    }
}
