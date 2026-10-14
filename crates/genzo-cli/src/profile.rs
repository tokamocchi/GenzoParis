//! カメラプロファイル（DCP）・ルック（3D LUT の .cube）のコマンド（`profile`。DEV-17）。
//!
//! - `profile import <ファイル>...`: 取り込む（genzo-api の `Core::import_profiles`。カタログのデータのフォルダの
//!   `profiles` に写し、元のファイルは変えない）。取り込めなかったファイルがあれば終了コード 3。
//! - `profile list`: 取り込んだものの一覧（`--camera`・`--for <写真>` で、その機種用かを表示する）。
//! - `profile show <camera|look> <ID>`・`profile remove <camera|look> <ID>`: 詳細・一覧から除く。
//! - `profile inspect <ファイル>`: 取り込まずにファイルを読んで中身を表示する（カタログは使わない）。
//!
//! 現像設定には `develop set --profile <ID>`・`--look <ID>`・`--look-amount <N>` で指定する（`develop`）。Adobe の
//! プロファイルなどは同梱しない（利用者が用意したファイルを使う）。

use genzo_api::{ProfileEntry, ProfileImportReport, ProfileKind};

use crate::Status;
use crate::args::{GlobalArgs, ProfileCommand};
use crate::error::{CliError, CliResult};
use crate::output::{Output, Table, json_value, key_values, one_line};
use crate::session::{OpenOptions, Session};

/// `profile`。
pub fn run(g: &GlobalArgs, out: Output, cmd: ProfileCommand) -> CliResult<Status> {
    if let ProfileCommand::Inspect { file, camera } = &cmd {
        return inspect(out, file, camera.as_deref());
    }
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = run_in(&s, out, cmd);
    s.close()?;
    r
}

/// `profile inspect`（カタログは使わない）。
fn inspect(out: Output, file: &std::path::Path, camera: Option<&str>) -> CliResult<Status> {
    let e = genzo_api::inspect_profile_file(file, camera)?;
    if out.json {
        out.print_json(&json_value(&e));
    } else {
        detail_lines(out, &e);
    }
    Ok(Status::Success)
}

/// 種類の表示。
pub fn kind_text(k: ProfileKind) -> &'static str {
    match k {
        ProfileKind::Camera => "カメラプロファイル",
        ProfileKind::Look => "ルック",
    }
}

fn dash(s: Option<&str>) -> String {
    s.map_or_else(|| "—".to_owned(), str::to_owned)
}

fn detail_lines(out: Output, e: &ProfileEntry) {
    let mut rows = vec![
        ("種類", kind_text(e.kind).to_owned()),
        ("ID", e.id.clone()),
        ("名前", e.name.clone()),
        ("機種", dash(e.camera_model.as_deref())),
    ];
    if let Some(m) = e.matches_camera {
        rows.push((
            "指定した機種用",
            if m { "はい" } else { "いいえ" }.to_owned(),
        ));
    }
    rows.push((
        "使える",
        if e.usable {
            "はい".to_owned()
        } else {
            "いいえ（取り込み直してください）".to_owned()
        },
    ));
    rows.push(("内容", e.description.clone()));
    for n in &e.notes {
        rows.push(("注意", one_line(n)));
    }
    rows.push((
        "著作権の表示",
        dash(e.copyright.as_deref().map(one_line).as_deref()),
    ));
    rows.push(("埋め込みの方針", dash(e.embed_policy.as_deref())));
    rows.push(("元のファイル", e.original_name.clone()));
    rows.push(("大きさ", format!("{} バイト", e.size)));
    rows.push(("SHA-256", e.sha256.clone()));
    if !e.imported_at.is_empty() {
        rows.push(("取り込んだ日時", e.imported_at.clone()));
    }
    out.line(key_values(&rows));
}

fn list_table(out: Output, list: &[ProfileEntry]) {
    if list.is_empty() {
        out.line("取り込んだカメラプロファイル・ルックはありません（genzo profile import で取り込めます）");
        return;
    }
    let mut t = Table::new(["種類", "ID", "名前", "機種", "この機種用", "内容"]);
    for e in list {
        t.row([
            kind_text(e.kind).to_owned(),
            e.id.clone(),
            e.name.clone(),
            dash(e.camera_model.as_deref()),
            match e.matches_camera {
                Some(true) => "はい".to_owned(),
                Some(false) => "いいえ".to_owned(),
                None => "—".to_owned(),
            },
            if e.usable {
                e.description.clone()
            } else {
                "読めません（取り込み直してください）".to_owned()
            },
        ]);
    }
    out.table(&t);
}

fn import_lines(out: Output, r: &ProfileImportReport) {
    let mut t = Table::new(["結果", "種類", "ID", "名前・理由"]);
    let mut add = |label: &str, e: &ProfileEntry| {
        t.row([
            label.to_owned(),
            kind_text(e.kind).to_owned(),
            e.id.clone(),
            e.name.clone(),
        ]);
    };
    for e in &r.imported {
        add("取り込んだ", e);
    }
    for e in &r.replaced {
        add("置き換えた", e);
    }
    for e in &r.repaired {
        add("写しを書き直した", e);
    }
    for e in &r.unchanged {
        add("同じ内容があった", e);
    }
    for f in &r.failed {
        t.row([
            "取り込めなかった".to_owned(),
            "—".to_owned(),
            f.path.display().to_string(),
            one_line(&f.message),
        ]);
    }
    out.table(&t);
    if !r.replaced.is_empty() {
        out.warn(
            "置き換えたものを使っている写真は、次に開いたときに「内容が違う」と知らせます（色が変わる可能性があります）",
        );
    }
}

fn run_in(s: &Session, out: Output, cmd: ProfileCommand) -> CliResult<Status> {
    match cmd {
        ProfileCommand::Import { files, replace } => {
            let r = s.core.import_profiles(&files, replace)?;
            if out.json {
                out.print_json(&json_value(&r));
            } else {
                import_lines(out, &r);
            }
            Ok(if r.failed.is_empty() {
                Status::Success
            } else {
                Status::Partial
            })
        }
        ProfileCommand::List {
            kind,
            camera,
            for_variant,
        } => {
            let camera = match (camera, for_variant) {
                (Some(c), _) => Some(c),
                (None, Some(v)) => {
                    let d = s.core.variant_details(v)?;
                    let c = d.summary.camera.ok_or_else(|| {
                        CliError::input(format!("variant {v} の機種が分かりません"))
                    })?;
                    Some(c)
                }
                (None, None) => None,
            };
            let mut list = s.core.list_profiles(camera.as_deref())?;
            if let Some(k) = kind {
                let k = ProfileKind::from(k);
                list.retain(|e| e.kind == k);
            }
            if out.json {
                out.print_json(&serde_json::json!({
                    "dir": s.core.profiles_dir(),
                    "camera": camera,
                    "profiles": json_value(&list),
                }));
            } else {
                if let Some(c) = &camera {
                    out.progress(format!("機種: {c}"));
                }
                list_table(out, &list);
            }
            Ok(Status::Success)
        }
        ProfileCommand::Show { kind, id } => {
            let kind = ProfileKind::from(kind);
            let e = s
                .core
                .list_profiles(None)?
                .into_iter()
                .find(|e| e.kind == kind && e.id == id)
                .ok_or_else(|| {
                    CliError::Api(genzo_api::ApiError::NotFound(format!(
                        "{}「{id}」",
                        kind_text(kind)
                    )))
                })?;
            if out.json {
                out.print_json(&json_value(&e));
            } else {
                detail_lines(out, &e);
            }
            Ok(Status::Success)
        }
        ProfileCommand::Remove { kind, id } => {
            let kind = ProfileKind::from(kind);
            s.core.remove_profile(kind, &id)?;
            if out.json {
                out.print_json(&serde_json::json!({"removed": {"kind": kind, "id": id}}));
            } else {
                out.line(format!(
                    "{}「{id}」を一覧から除きました（使っている写真は、標準の色・ルックなしで描きます）",
                    kind_text(kind)
                ));
            }
            Ok(Status::Success)
        }
        ProfileCommand::Inspect { file, camera } => inspect(out, &file, camera.as_deref()),
    }
}
