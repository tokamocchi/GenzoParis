//! 位置情報の後付け（LIB-21。ジオタグ付け。`location`。docs/04_architecture.md の 3.16 節）。
//!
//! - 処理はコア API（genzo-api の `location`）と同じ規則。カタログの中だけを変え、元のファイルには書かない
//!   （カメラの位置も変えない）。使う位置（付けた位置があればそれ、なければカメラの位置）が `show` の GPS と
//!   書き出しの Exif に入る（`export --remove-gps` なら入らない）。
//! - 対象は variant の ID。同じ写真の仮想コピーと、RAW と同時記録の JPEG の組は位置を共有する。動画は除き、
//!   除いた数を警告に出す（`--json` では `plan.skipped_videos`）。
//! - `set --at` の座標の文字列は genzo-model の `parse_coordinates` で読む（画面の貼り付けと同じ形・同じ規則。
//!   読めない・範囲外は使い方の誤り）。`--dry-run` で、変わる数だけを出す。
//! - `history`・`undo` は撮影日時の修正（`time`）と同じ形（新しい順。最後の変更から取り消す。やり直しはない）。
//! - `--json`: `set`・`clear` は `{"dry_run", "variant_ids", "location", "plan"}`（`location` は付ける位置。消すときは
//!   `null`。`plan` はコア API の `LocationEditPlan` の形）、`history` は `{"edits"}`、`undo` は `{"undone"}`
//!   （取り消すものがなければ `null`）。

use genzo_api::{LocationEditInfo, LocationEditPlan, VariantId};
use genzo_model::{GpsCoord, format_coordinates};
use serde_json::json;

use crate::Status;
use crate::args::{GlobalArgs, LocationCommand};
use crate::error::CliResult;
use crate::library::{ensure_variants, unique_ids};
use crate::output::{Output, Table, json_value};
use crate::session::{OpenOptions, Session};

/// `location`。
pub fn run(g: &GlobalArgs, out: Output, cmd: LocationCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = run_in(&s, out, cmd);
    s.close()?;
    r
}

fn run_in(s: &Session, out: Output, cmd: LocationCommand) -> CliResult<Status> {
    match cmd {
        LocationCommand::Set {
            variants,
            at,
            dry_run,
        } => edit(s, out, &variants, Some(at), dry_run),
        LocationCommand::Clear { variants, dry_run } => edit(s, out, &variants, None, dry_run),
        LocationCommand::History => {
            let edits = s.core.location_edits()?;
            if out.json {
                out.print_json(&json!({ "edits": json_value(&edits) }));
            } else if edits.is_empty() {
                out.line("位置の変更の履歴はありません");
            } else {
                let mut t = Table::new(["ID", "日時（UTC）", "変更", "数"]);
                for e in &edits {
                    t.row([
                        e.id.to_string(),
                        e.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                        describe_edit(e),
                        e.asset_count.to_string(),
                    ]);
                }
                out.table(&t);
            }
            Ok(Status::Success)
        }
        LocationCommand::Undo => {
            let undone = s.core.undo_location_edit()?;
            if out.json {
                out.print_json(&json!({ "undone": json_value(&undone) }));
            } else {
                match undone {
                    Some(u) => {
                        out.line(format!(
                            "位置の変更（{}）を取り消しました（{} 枚を戻しました）",
                            describe_edit(&u.edit),
                            u.restored
                        ));
                        if u.removed > 0 {
                            out.line(format!(
                                "変更の後にカタログから除いた {} 枚は戻していません",
                                u.removed
                            ));
                        }
                    }
                    None => out.line("取り消す位置の変更はありません"),
                }
            }
            Ok(Status::Success)
        }
    }
}

/// 位置を付ける・付けた位置を消す（`location` が `None` なら消す。--dry-run なら見込みだけ）。
fn edit(
    s: &Session,
    out: Output,
    variants: &[VariantId],
    location: Option<GpsCoord>,
    dry_run: bool,
) -> CliResult<Status> {
    let ids = unique_ids(variants);
    ensure_variants(s, &ids)?;
    let plan = if dry_run {
        s.core.plan_location_edit(&ids, location)?
    } else {
        s.core.edit_locations(&ids, location)?
    };
    if plan.skipped_videos > 0 {
        out.warn(format!(
            "動画 {} 本は対象外のため除きました（動画には位置を付けません）",
            plan.skipped_videos
        ));
    }
    if out.json {
        out.print_json(&json!({
            "dry_run": dry_run,
            "variant_ids": json_value(&ids),
            "location": json_value(&location),
            "plan": json_value(&plan),
        }));
    } else {
        for line in describe_plan(&plan, location, dry_run) {
            out.line(line);
        }
    }
    Ok(Status::Success)
}

/// 履歴の 1 件の説明。
fn describe_edit(e: &LocationEditInfo) -> String {
    match (e.kind.as_str(), e.location) {
        ("set", Some(g)) => format!("位置を付ける {}", format_coordinates(g)),
        ("clear", _) => "付けた位置を消す".to_owned(),
        (other, _) => other.to_owned(),
    }
}

/// 結果の文の「元に戻せます」の案内（変わったものがなければ履歴に残らないので出さない）。
fn undo_hint(p: &LocationEditPlan) -> &'static str {
    if p.changed > 0 {
        "。location undo で元に戻せます"
    } else {
        ""
    }
}

/// 見込み・結果の文（`location` は付ける位置。`None` は消す）。
fn describe_plan(p: &LocationEditPlan, location: Option<GpsCoord>, dry_run: bool) -> Vec<String> {
    let first = match (location, dry_run) {
        (Some(g), true) => format!(
            "{} 枚に位置（{}）を付けます（対象 {} 枚。--dry-run のため変えていません）",
            p.changed,
            format_coordinates(g),
            p.photos
        ),
        (Some(g), false) => format!(
            "{} 枚に位置（{}）を付けました（対象 {} 枚{}）",
            p.changed,
            format_coordinates(g),
            p.photos,
            undo_hint(p)
        ),
        (None, true) => format!(
            "{} 枚の付けた位置を消します（対象 {} 枚。--dry-run のため変えていません）",
            p.changed, p.photos
        ),
        (None, false) => format!(
            "{} 枚の付けた位置を消しました（対象 {} 枚{}）",
            p.changed,
            p.photos,
            undo_hint(p)
        ),
    };
    let mut lines = vec![first];
    if p.unchanged > 0 {
        lines.push(if location.is_some() {
            format!("{} 枚は同じ位置が付いているため変わりません", p.unchanged)
        } else {
            format!("{} 枚は付けた位置がないため変わりません", p.unchanged)
        });
    }
    if location.is_some() {
        // 付けてあった位置を置き換える数（同じ位置が付いているものは変わらない）。
        let replaced = p.with_assigned.saturating_sub(p.unchanged);
        if replaced > 0 {
            let verb = if dry_run {
                "置き換えます"
            } else {
                "置き換えました"
            };
            lines.push(format!("{replaced} 枚は付けてあった位置を{verb}"));
        }
        if p.with_camera_gps > 0 {
            lines.push(format!(
                "カメラの位置がある {} 枚は、付けた位置を優先して使います（カメラの位置は変えません）",
                p.with_camera_gps
            ));
        }
    } else if p.with_camera_gps > 0 {
        lines.push(format!(
            "カメラの位置がある {} 枚は、カメラの位置を使います",
            p.with_camera_gps
        ));
    }
    if p.missing > 0 {
        lines.push(format!("見つからない写真 {} 枚は除きました", p.missing));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::parse_location;

    #[test]
    fn coordinates_of_the_at_option_are_parsed() {
        let g = parse_location("35.681236, 139.767125").unwrap();
        assert_eq!((g.lat, g.lon), (35.681236, 139.767125));
        let g = parse_location("-33.8568 151.2153").unwrap();
        assert_eq!((g.lat, g.lon), (-33.8568, 151.2153));
        let g = parse_location("北緯 35.681236°、東経 139.767125°").unwrap();
        assert_eq!((g.lat, g.lon), (35.681236, 139.767125));
        for bad in ["", "abc", "35.681236", "91, 0", "0, 181"] {
            assert!(parse_location(bad).is_err(), "{bad:?}");
        }
        // 説明は genzo-model の CoordParseError のまま（範囲外は読んだ値を含む）。
        assert_eq!(
            parse_location("1000, 0").unwrap_err(),
            "緯度が範囲外です（-90〜90）: 1000"
        );
    }

    #[test]
    fn plans_and_edits_are_described() {
        let at = GpsCoord::new(35.681236, 139.767125);
        let p = LocationEditPlan {
            photos: 4,
            changed: 3,
            unchanged: 1,
            with_camera_gps: 2,
            with_assigned: 2,
            skipped_videos: 1,
            ..Default::default()
        };
        let lines = describe_plan(&p, at, true);
        assert_eq!(
            lines[0],
            "3 枚に位置（35.681236, 139.767125）を付けます（対象 4 枚。--dry-run のため変えていません）"
        );
        assert_eq!(lines[1], "1 枚は同じ位置が付いているため変わりません");
        assert_eq!(lines[2], "1 枚は付けてあった位置を置き換えます");
        assert_eq!(
            lines[3],
            "カメラの位置がある 2 枚は、付けた位置を優先して使います（カメラの位置は変えません）"
        );
        // 動画を除いた数は警告に出す（文には入れない）。
        assert_eq!(lines.len(), 4);
        let cleared = describe_plan(&p, None, false);
        assert_eq!(
            cleared[0],
            "3 枚の付けた位置を消しました（対象 4 枚。location undo で元に戻せます）"
        );
        assert_eq!(cleared[1], "1 枚は付けた位置がないため変わりません");
        assert_eq!(
            cleared[2],
            "カメラの位置がある 2 枚は、カメラの位置を使います"
        );
        // 変わったものがなければ、履歴に残らないので元に戻す案内を出さない。
        let none = LocationEditPlan {
            photos: 1,
            unchanged: 1,
            with_assigned: 1,
            ..Default::default()
        };
        assert_eq!(
            describe_plan(&none, at, false),
            [
                "0 枚に位置（35.681236, 139.767125）を付けました（対象 1 枚）",
                "1 枚は同じ位置が付いているため変わりません",
            ]
        );

        let e = LocationEditInfo {
            id: 1,
            kind: "set".to_owned(),
            location: GpsCoord::new(-33.8568, 151.2153),
            asset_count: 2,
            created_at: chrono::Utc::now(),
        };
        assert_eq!(describe_edit(&e), "位置を付ける -33.856800, 151.215300");
        let c = LocationEditInfo {
            kind: "clear".to_owned(),
            location: None,
            ..e
        };
        assert_eq!(describe_edit(&c), "付けた位置を消す");
    }
}
