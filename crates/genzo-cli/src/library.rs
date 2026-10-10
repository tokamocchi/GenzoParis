//! 取り込み・検索・表示・選別・サムネイル・削除のコマンド（IMP-01、LIB-04・07・08・14、PRV-02、
//! FILE-01。01 のストーリー 1〜3）。

use std::collections::BTreeSet;
use std::path::Path;

use chrono::FixedOffset;
use genzo_api::{
    ApiError, AssetKind, DeleteKind, DeletePlan, DeleteReport, FileStatus, Flag, JobResult, Rating,
    SearchFilter, SearchSort, VariantId, VariantSummary,
};
use serde_json::json;

use crate::Status;
use crate::args::{
    FlagArg, GlobalArgs, ImportArgs, KindArg, LabelArg, SearchArgs, ThumbsCommand, direction,
};
use crate::error::{CliError, CliResult};
use crate::output::{Output, Table, json_value, key_values, one_line};
use crate::session::{OpenOptions, Session};

/// 指定した variant がすべてあるか確かめる（変更の前に。一部だけ変えて失敗しないため）。
pub fn ensure_variants(s: &Session, ids: &[VariantId]) -> CliResult<()> {
    let unique: BTreeSet<VariantId> = ids.iter().copied().collect();
    for v in unique {
        s.core.variant_details(v)?;
    }
    Ok(())
}

/// 重複を除いた ID（指定の順を保つ）。
pub fn unique_ids(ids: &[VariantId]) -> Vec<VariantId> {
    let mut seen = BTreeSet::new();
    ids.iter().copied().filter(|v| seen.insert(*v)).collect()
}

// ---------------------------------------------------------------------------
// 取り込み
// ---------------------------------------------------------------------------

/// `import`。
pub fn import(g: &GlobalArgs, out: Output, a: ImportArgs) -> CliResult<Status> {
    if !a.dir.is_dir() {
        return Err(CliError::Api(ApiError::NotFound(format!(
            "フォルダ {}",
            a.dir.display()
        ))));
    }
    let s = Session::open(
        g,
        out,
        &OpenOptions {
            render_previews_after_import: a.no_previews.then_some(false),
            ..Default::default()
        },
    )?;
    let job = s.core.import_folder(&a.dir, !a.no_recursive)?;
    let info = s.wait_job_ok(job, "取り込み")?;
    let Some(JobResult::Import(report)) = info.result else {
        return Err(CliError::other("取り込みの結果がありません"));
    };
    s.close()?;
    // 読めずに登録できなかったファイルがある・途中で取り消された場合は終了コード 1（書き出しの一部の
    // 失敗と同じ扱い）。メタデータを読めずに status = error で登録したファイルは、登録はできているので 0。
    let failed = !report.not_registered.is_empty() || report.cancelled;
    if out.json {
        out.print_json(&report);
    } else {
        out.line(format!(
            "取り込み: {}（対象 {} 件、新規 {} 件、更新 {} 件、変化なし {} 件、サムネイル {} 件）",
            report.root.display(),
            report.files_found,
            report.added,
            report.updated,
            report.unchanged,
            report.thumbnails
        ));
        for e in &report.errors {
            out.line(format!(
                "  読めないファイル（status = error で登録）: {}: {}",
                e.path.display(),
                one_line(&e.reason)
            ));
        }
        for e in &report.not_registered {
            out.line(format!(
                "  登録できなかったファイル: {}: {}",
                e.path.display(),
                one_line(&e.reason)
            ));
        }
        for e in &report.thumbnail_failures {
            out.line(format!(
                "  サムネイルを作れなかったファイル: {}: {}",
                e.path.display(),
                one_line(&e.reason)
            ));
        }
        if report.skipped_links > 0 {
            out.line(format!(
                "  たどらなかったリンク: {} 件",
                report.skipped_links
            ));
        }
        if report.cancelled {
            out.line("  途中で取り消されました（もう一度取り込むと続きから処理します）");
        }
    }
    Ok(if failed {
        Status::Failure
    } else {
        Status::Success
    })
}

// ---------------------------------------------------------------------------
// 検索
// ---------------------------------------------------------------------------

/// 検索の条件を作る（日付はカタログの既定のタイムゾーンで解釈する）。
fn filter_of(s: &Session, a: &SearchArgs) -> CliResult<SearchFilter> {
    let minutes = s.core.settings().default_utc_offset_minutes;
    let offset = FixedOffset::east_opt(minutes * 60)
        .ok_or_else(|| CliError::other(format!("既定のタイムゾーンが不正です（{minutes} 分）")))?;
    let bad_date = || CliError::Usage("日時を UTC に変換できません".to_owned());
    let folder_id = match &a.folder {
        Some(p) => Some(find_folder(s, p)?),
        None => None,
    };
    Ok(SearchFilter {
        rating_min: a.min_rating.and_then(Rating::new),
        rating_max: a.max_rating.and_then(Rating::new),
        flags: (!a.flag.is_empty()).then(|| a.flag.iter().map(|&f| Flag::from(f)).collect()),
        color_labels: (!a.label.is_empty()).then(|| a.label.iter().map(|l| l.to_label()).collect()),
        captured_from: match a.from {
            Some(d) => Some(d.to_utc(offset, false).ok_or_else(bad_date)?),
            None => None,
        },
        captured_until: match a.to {
            Some(d) => Some(d.to_utc(offset, true).ok_or_else(bad_date)?),
            None => None,
        },
        cameras: (!a.camera.is_empty()).then(|| a.camera.clone()),
        lenses: (!a.lens.is_empty()).then(|| a.lens.clone()),
        kind: a.kind.map(|k| match k {
            KindArg::Photo => AssetKind::Photo,
            KindArg::Video => AssetKind::Video,
        }),
        text: a.text.clone(),
        folder_id,
        include_subfolders: !a.no_subfolders,
        masters_only: a.masters_only,
        ..Default::default()
    })
}

/// フォルダのパスからカタログのフォルダを探す（パスのままと、リンクを解決したものの両方で比べる）。
fn find_folder(s: &Session, path: &Path) -> CliResult<genzo_model::FolderId> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::path::absolute(path) {
        candidates.push(p);
    }
    if let Ok(p) = std::fs::canonicalize(path) {
        candidates.push(p);
    }
    let folders = s.core.folders()?;
    // まずパスのままで比べ、見つからなければカタログの側のリンクも解決して比べる（フォルダが多いと
    // 解決に時間がかかるため）。
    for f in &folders {
        if let Some(fp) = &f.path
            && candidates.iter().any(|c| same_path(c, fp))
        {
            return Ok(f.folder_id);
        }
    }
    for f in &folders {
        let Some(fc) = f.path.as_ref().and_then(|p| std::fs::canonicalize(p).ok()) else {
            continue;
        };
        if candidates.iter().any(|c| same_path(c, &fc)) {
            return Ok(f.folder_id);
        }
    }
    Err(CliError::Api(ApiError::NotFound(format!(
        "カタログのフォルダ {}",
        path.display()
    ))))
}

/// 末尾の区切りの違いを除いて同じパスか。
fn same_path(a: &Path, b: &Path) -> bool {
    a.components().eq(b.components())
}

/// 撮影日時の表示（撮影地のオフセットでの日時）。
pub fn capture_text(v: &VariantSummary) -> String {
    match v.capture.local() {
        Some(t) => t.format("%Y-%m-%d %H:%M:%S%:z").to_string(),
        None => "—".to_owned(),
    }
}

fn flag_text(f: Flag) -> &'static str {
    match f {
        Flag::Picked => "採用",
        Flag::Rejected => "不採用",
        Flag::None => "",
    }
}

fn kind_text(k: AssetKind) -> &'static str {
    match k {
        AssetKind::Photo => "写真",
        AssetKind::Video => "動画",
    }
}

fn name_text(v: &VariantSummary) -> String {
    match (&v.variant_name, v.is_master) {
        (Some(n), false) => format!("{}（{n}）", v.file_name),
        (None, false) => format!("{}（仮想コピー）", v.file_name),
        _ => v.file_name.clone(),
    }
}

fn status_text(s: FileStatus) -> &'static str {
    match s {
        FileStatus::Ok => "",
        FileStatus::Missing => "見つからない",
        FileStatus::Error => "読めない",
    }
}

/// `search`。
pub fn search(g: &GlobalArgs, out: Output, a: SearchArgs) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let filter = filter_of(&s, &a)?;
    let sort = SearchSort {
        key: a.sort.key(),
        direction: direction(a.desc),
    };
    let result = s.core.search(&filter, sort)?;
    let end = if a.limit == 0 {
        result.count
    } else {
        a.offset.saturating_add(a.limit).min(result.count)
    };
    let mut items: Vec<VariantSummary> = Vec::new();
    let mut start = a.offset.min(result.count);
    while start < end {
        let len = (end - start).min(genzo_api::MAX_RANGE_LEN);
        let page = s.core.range(result.generation, start, len)?;
        items.extend(page.items);
        start += len;
    }
    s.close()?;
    if out.json {
        out.print_json(&json!({
            "generation": result.generation,
            "count": result.count,
            "offset": a.offset,
            "limit": a.limit,
            "items": json_value(&items),
        }));
    } else if a.ids {
        for v in &items {
            out.line(v.variant_id.to_string());
        }
    } else {
        let mut t = Table::new([
            "ID",
            "名前",
            "種別",
            "評価",
            "フラグ",
            "ラベル",
            "撮影日時",
            "カメラ",
            "状態",
        ]);
        for v in &items {
            t.row([
                v.variant_id.to_string(),
                name_text(v),
                kind_text(v.kind).to_owned(),
                v.rating.get().to_string(),
                flag_text(v.flag).to_owned(),
                v.color_label
                    .map(|l| l.as_str().to_owned())
                    .unwrap_or_default(),
                capture_text(v),
                v.camera.clone().unwrap_or_default(),
                status_text(v.file_status).to_owned(),
            ]);
        }
        out.table(&t);
        out.line(format!(
            "{} 件中 {} 件を表示（先頭から {} 件を飛ばした）",
            result.count,
            items.len(),
            a.offset.min(result.count)
        ));
    }
    Ok(Status::Success)
}

// ---------------------------------------------------------------------------
// 表示
// ---------------------------------------------------------------------------

/// `show`。
pub fn show(g: &GlobalArgs, out: Output, variant: VariantId) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let details = s.core.variant_details(variant)?;
    let settings = s.core.develop_settings(variant)?;
    let history = s.core.history(variant)?;
    s.close()?;
    if out.json {
        out.print_json(&json!({
            "details": json_value(&details),
            "develop": json_value(&settings),
            "history_entries": history.len(),
        }));
        return Ok(Status::Success);
    }
    let v = &details.summary;
    let opt = |o: Option<String>| o.unwrap_or_else(|| "—".to_owned());
    let mut items = vec![
        ("ID", v.variant_id.to_string()),
        ("名前", name_text(v)),
        ("種別", kind_text(v.kind).to_owned()),
        ("評価", v.rating.get().to_string()),
        (
            "フラグ",
            opt(Some(flag_text(v.flag).to_owned()).filter(|s| !s.is_empty())),
        ),
        ("ラベル", opt(v.color_label.map(|l| l.as_str().to_owned()))),
        ("撮影日時", capture_text(v)),
        ("カメラ", opt(v.camera.clone())),
        ("レンズ", opt(v.lens.clone())),
        (
            "寸法",
            match (v.width, v.height) {
                (Some(w), Some(h)) => format!("{w} × {h}（向き {}）", v.orientation as u8),
                _ => "—".to_owned(),
            },
        ),
        ("ISO", opt(details.iso.map(|x| x.to_string()))),
        ("絞り", opt(details.aperture.map(|x| format!("F{x:.1}")))),
        ("シャッター速度", opt(details.shutter_s.map(shutter_text))),
        (
            "焦点距離",
            opt(details.focal_mm.map(|x| format!("{x:.0} mm"))),
        ),
        (
            "GPS",
            opt(details.gps.map(|g| format!("{:.6}, {:.6}", g.lat, g.lon))),
        ),
        ("キャプション", opt(details.caption.clone())),
        ("現像の履歴", format!("{} 件", history.len())),
    ];
    if let Some(video) = &details.video {
        items.extend(video_items(video));
    }
    out.line(key_values(&items).trim_end());
    for f in &details.files {
        out.line(format!(
            "ファイル（{}）: {}  {} バイト  {}",
            f.role.as_str(),
            f.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "（場所が分かりません）".to_owned()),
            f.size,
            f.status.as_str()
        ));
    }
    // 動画は現像しない（EXP-05 は v1）ので、現像設定は表示しない（JSON には含める）。
    if v.kind == AssetKind::Photo {
        out.line("現像設定:");
        out.line(serde_json::to_string_pretty(&settings).unwrap_or_default());
    }
    Ok(Status::Success)
}

/// 動画のメタデータの表示（`show`・`info`）。
pub fn video_items(m: &genzo_model::VideoMetadata) -> Vec<(&'static str, String)> {
    let opt = |o: Option<String>| o.unwrap_or_else(|| "—".to_owned());
    vec![
        ("長さ", opt(m.duration_s.map(|d| format!("{d:.2} 秒")))),
        ("フレームレート", opt(m.fps.map(|f| format!("{f:.3} fps")))),
        ("コーデック", opt(m.codec.clone())),
        ("ビット深度", opt(m.bit_depth.map(|b| format!("{b} bit")))),
        (
            "伝達関数・原色",
            format!(
                "{} / {}",
                m.color_transfer.as_deref().unwrap_or("—"),
                m.color_primaries.as_deref().unwrap_or("—")
            ),
        ),
        ("作成日時（記録の値）", opt(m.creation_time.clone())),
    ]
}

/// シャッター速度の表示（1 秒未満は 1/n 秒、1 秒以上は小数 1 桁まで。`show`・`info`）。
pub fn shutter_text(x: f64) -> String {
    if x > 0.0 && x < 1.0 {
        format!("1/{:.0} 秒", 1.0 / x)
    } else {
        let t = format!("{x:.1}");
        format!("{} 秒", t.strip_suffix(".0").unwrap_or(&t))
    }
}

// ---------------------------------------------------------------------------
// 選別
// ---------------------------------------------------------------------------

/// 選別の操作の種類。
pub enum Mark {
    /// 評価。
    Rating(u8),
    /// フラグ。
    Flag(FlagArg),
    /// カラーラベル。
    Label(LabelArg),
}

/// `rate` / `flag` / `label`。
pub fn mark(g: &GlobalArgs, out: Output, ids: &[VariantId], m: Mark) -> CliResult<Status> {
    let ids = unique_ids(ids);
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &ids)?;
    let n = match m {
        Mark::Rating(r) => s.core.set_rating(
            &ids,
            Rating::new(r).ok_or_else(|| CliError::Usage(format!("評価は 0〜5 です（{r}）")))?,
        )?,
        Mark::Flag(f) => s.core.set_flag(&ids, f.into())?,
        Mark::Label(l) => s.core.set_color_label(&ids, l.to_label())?,
    };
    s.close()?;
    print_updated(out, n);
    Ok(Status::Success)
}

fn print_updated(out: Output, n: usize) {
    if out.json {
        out.print_json(&json!({ "updated": n }));
    } else {
        out.line(format!("{n} 件を変更しました"));
    }
}

/// `caption`（空の文字列で削除）。
pub fn caption(g: &GlobalArgs, out: Output, v: VariantId, text: &str) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &[v])?;
    let caption = (!text.trim().is_empty()).then_some(text);
    let n = s.core.set_caption(&[v], caption)?;
    s.close()?;
    print_updated(out, n);
    Ok(Status::Success)
}

// ---------------------------------------------------------------------------
// サムネイル
// ---------------------------------------------------------------------------

/// `thumbs`。
pub fn thumbs(g: &GlobalArgs, out: Output, cmd: ThumbsCommand) -> CliResult<Status> {
    let ThumbsCommand::Regenerate { variants } = cmd;
    let s = Session::open(g, out, &OpenOptions::default())?;
    let ids = if variants.is_empty() {
        let r = s
            .core
            .search(&SearchFilter::default(), SearchSort::default())?;
        s.core.result_ids(r.generation)?
    } else {
        let ids = unique_ids(&variants);
        ensure_variants(&s, &ids)?;
        ids
    };
    let job = s.core.regenerate_previews(&ids)?;
    let info = s.wait_job_ok(job, "サムネイルの作り直し")?;
    let Some(JobResult::RegeneratePreviews(report)) = info.result else {
        return Err(CliError::other("サムネイルの作り直しの結果がありません"));
    };
    s.close()?;
    let failed = !report.failed.is_empty();
    if out.json {
        out.print_json(&report);
    } else {
        out.line(format!(
            "サムネイル・プレビューを作り直しました: {} 件（対象外 {} 件、失敗 {} 件）",
            report.rendered,
            report.skipped,
            report.failed.len()
        ));
        for f in &report.failed {
            out.line(format!(
                "  variant {}: {}",
                f.variant_id,
                one_line(&f.reason)
            ));
        }
    }
    Ok(if failed {
        Status::Failure
    } else {
        Status::Success
    })
}

// ---------------------------------------------------------------------------
// 削除
// ---------------------------------------------------------------------------

/// 削除の計画を表示する（6.4 節: 影響するファイルと variant の一覧）。`main` なら結果として標準出力に、
/// そうでなければ進捗として標準エラーに出す。
fn print_plan(out: Output, plan: &DeletePlan, main: bool) {
    let emit = |text: String| {
        if main {
            out.line(text);
        } else {
            out.progress(text);
        }
    };
    let what = match plan.kind {
        DeleteKind::VirtualCopies => "仮想コピーの削除",
        DeleteKind::RemoveFromCatalog => "カタログからの除去（元ファイルは変更しない）",
        DeleteKind::Trash => "ゴミ箱へ移動（元ファイルを OS のゴミ箱へ移し、カタログから除く）",
    };
    emit(format!(
        "{what}: asset {} 件・variant {} 件・ファイル {} 件",
        plan.assets.len(),
        plan.variants.len(),
        plan.files.len()
    ));
    for v in &plan.variants {
        let kind = if v.is_master {
            "マスター"
        } else {
            "仮想コピー"
        };
        let name = v.name.as_deref().unwrap_or("");
        emit(format!("  variant {}（{kind}）{name}", v.variant_id));
    }
    for f in &plan.files {
        emit(format!(
            "  ファイル（{}）: {}",
            f.role.as_str(),
            f.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "（場所が分かりません）".to_owned())
        ));
    }
}

fn print_delete_report(out: Output, plan: &DeletePlan, report: &DeleteReport) {
    if out.json {
        // 中止したとき（`executed: false`）と同じ形にする。
        out.print_json(&json!({ "plan": plan, "report": report, "executed": true }));
        return;
    }
    out.line(format!(
        "variant {} 件・asset {} 件をカタログから除きました",
        report.removed_variants.len(),
        report.removed_assets.len()
    ));
    for p in &report.trashed_files {
        out.line(format!("  ゴミ箱へ移しました: {}", p.display()));
    }
    for f in &report.failed {
        out.line(format!(
            "  失敗: {}: {}",
            f.path.display(),
            one_line(&f.reason)
        ));
    }
}

/// 削除の計画を作って実行する。
pub fn delete(
    g: &GlobalArgs,
    out: Output,
    kind: DeleteKind,
    ids: &[VariantId],
    confirmed: bool,
) -> CliResult<Status> {
    let ids = unique_ids(ids);
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &ids)?;
    let plan = s.core.plan_delete(kind, &ids)?;
    if kind == DeleteKind::Trash && !confirmed {
        if !out.json {
            print_plan(out, &plan, true);
        }
        let proceed = !out.json && ask_yes_no("ゴミ箱へ移しますか？ [y/N] ");
        if !proceed {
            s.close()?;
            if out.json {
                out.print_json(&json!({ "plan": plan, "executed": false }));
            }
            out.warn("確認していないため中止しました（実行するには --yes を付けてください）");
            return Ok(Status::Failure);
        }
    } else if !out.json {
        print_plan(out, &plan, false);
    }
    let report = s.core.execute_delete(plan.plan_id)?;
    s.close()?;
    let failed = !report.failed.is_empty();
    print_delete_report(out, &plan, &report);
    Ok(if failed {
        Status::Failure
    } else {
        Status::Success
    })
}

/// 端末で確認を求める（標準入力と標準エラーが端末のときだけ。それ以外は「いいえ」）。
fn ask_yes_no(prompt: &str) -> bool {
    use std::io::{BufRead, IsTerminal, Write};
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return false;
    }
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_ids_keep_the_order() {
        let ids: Vec<VariantId> = [3, 1, 3, 2, 1].into_iter().map(VariantId::new).collect();
        assert_eq!(
            unique_ids(&ids),
            vec![VariantId::new(3), VariantId::new(1), VariantId::new(2)]
        );
    }

    #[test]
    fn shutter_speeds() {
        assert_eq!(shutter_text(1.0 / 250.0), "1/250 秒");
        // Exif の f32 の値（1/250 は 2 進数で割り切れない）でも丸めて表示する。
        assert_eq!(shutter_text(f64::from(0.004_f32)), "1/250 秒");
        assert_eq!(shutter_text(f64::from(1.3_f32)), "1.3 秒");
        assert_eq!(shutter_text(30.0), "30 秒");
    }

    #[test]
    fn paths_compare_by_components() {
        assert!(same_path(Path::new("/a/b/"), Path::new("/a/b")));
        assert!(!same_path(Path::new("/a/b"), Path::new("/a/c")));
    }
}
