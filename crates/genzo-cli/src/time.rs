//! 撮影日時の一括修正（LIB-16。`time`）と、写真と動画のタイムライン（ORG-01。`timeline`）。
//!
//! - 修正はコア API（genzo-api の `capture_time`）と同じ規則（docs/04 の 3.13 節）。カタログの中だけを変え、
//!   元のファイルには書かない。`--dry-run` で、変わる数と例だけを出す。
//! - 対象は variant の ID か、`--device-of` で指定した写真と同じ機材（カメラの機種・シリアル番号・写真か動画か）
//!   のすべて。`time sync` で対象を省くと、基準の写真と同じ機材のすべて。
//! - `timeline` は `search` と同じ条件で撮影日時の順に並べ、撮影日とイベントのまとまりを表で出す（`--json` では
//!   コア API の形のまま）。

use std::collections::BTreeSet;

use genzo_api::{
    CaptureDevice, CaptureDeviceKey, CaptureTarget, CaptureTime, CaptureTimeEdit,
    CaptureTimeEditInfo, CaptureTimePlan, SearchFilter, SearchSort, SortKey, VariantId, ZoneMode,
};
use serde_json::json;

use crate::Status;
use crate::args::{GlobalArgs, TimeCommand, TimeTargetArgs, TimelineArgs, direction};
use crate::error::{CliError, CliResult};
use crate::library::{ensure_variants, filter_of, unique_ids};
use crate::output::{Output, Table, json_value, utc_offset_text};
use crate::session::{OpenOptions, Session};

/// `time`。
pub fn run(g: &GlobalArgs, out: Output, cmd: TimeCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = run_in(&s, out, cmd);
    s.close()?;
    r
}

fn run_in(s: &Session, out: Output, cmd: TimeCommand) -> CliResult<Status> {
    match cmd {
        TimeCommand::Devices => {
            let devices = s.core.capture_devices()?;
            if out.json {
                out.print_json(&json!({ "devices": json_value(&devices) }));
            } else if devices.is_empty() {
                out.line("写真・動画がありません");
            } else {
                let mut t = Table::new([
                    "カメラ",
                    "シリアル番号",
                    "種別",
                    "数",
                    "撮影日時あり",
                    "修正あり",
                    "最初",
                    "最後",
                ]);
                for d in &devices {
                    t.row(device_row(d));
                }
                out.table(&t);
            }
            Ok(Status::Success)
        }
        TimeCommand::Shift { amount, target } => edit(
            s,
            out,
            &target,
            None,
            &CaptureTimeEdit::Shift { seconds: amount },
        ),
        TimeCommand::Sync {
            reference,
            correct,
            zone,
            target,
        } => {
            ensure_variants(s, &[reference])?;
            edit(
                s,
                out,
                &target,
                Some(reference),
                &CaptureTimeEdit::Sync {
                    reference,
                    correct,
                    offset_minutes: zone,
                },
            )
        }
        TimeCommand::Zone {
            offset,
            keep_instant,
            target,
        } => edit(
            s,
            out,
            &target,
            None,
            &CaptureTimeEdit::SetZone {
                offset_minutes: offset,
                mode: if keep_instant {
                    ZoneMode::KeepInstant
                } else {
                    ZoneMode::KeepLocal
                },
            },
        ),
        TimeCommand::ClearZone { target } => {
            edit(s, out, &target, None, &CaptureTimeEdit::ClearZone)
        }
        TimeCommand::Reset { target } => edit(s, out, &target, None, &CaptureTimeEdit::Reset),
        TimeCommand::History => {
            let edits = s.core.capture_time_edits()?;
            if out.json {
                out.print_json(&json!({ "edits": json_value(&edits) }));
            } else if edits.is_empty() {
                out.line("撮影日時の修正の履歴はありません");
            } else {
                let mut t = Table::new(["ID", "日時（UTC）", "修正", "数"]);
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
        TimeCommand::Undo => {
            let undone = s.core.undo_capture_time_edit()?;
            if out.json {
                out.print_json(&json!({ "undone": json_value(&undone) }));
            } else {
                match undone {
                    Some(u) => out.line(format!(
                        "撮影日時の修正（{}）を取り消しました（{} 枚を戻しました）",
                        describe_edit(&u.edit),
                        u.restored
                    )),
                    None => out.line("取り消す撮影日時の修正はありません"),
                }
            }
            Ok(Status::Success)
        }
    }
}

fn device_row(d: &CaptureDevice) -> Vec<String> {
    vec![
        d.key
            .camera
            .clone()
            .unwrap_or_else(|| "（機材の情報なし）".to_owned()),
        d.key.serial.clone().unwrap_or_default(),
        kind_text(d.key.kind).to_owned(),
        d.assets.to_string(),
        d.with_time.to_string(),
        d.adjusted.to_string(),
        d.first
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        d.last
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
    ]
}

fn kind_text(k: genzo_api::AssetKind) -> &'static str {
    match k {
        genzo_api::AssetKind::Photo => "写真",
        genzo_api::AssetKind::Video => "動画",
    }
}

/// 対象を決める（variant の ID と、--device-of の写真の機材。sync で両方を省けば基準の写真の機材）。
fn target_of(
    s: &Session,
    a: &TimeTargetArgs,
    reference: Option<VariantId>,
) -> CliResult<CaptureTarget> {
    let variants = unique_ids(&a.variants);
    let mut device_of = unique_ids(&a.device_of);
    if variants.is_empty()
        && device_of.is_empty()
        && let Some(r) = reference
    {
        device_of.push(r);
    }
    match (variants.is_empty(), device_of.is_empty()) {
        (true, true) => Err(CliError::Usage(
            "対象の写真（variant の ID）か --device-of を指定してください".to_owned(),
        )),
        (false, false) => Err(CliError::Usage(
            "variant の ID と --device-of は一緒に使えません".to_owned(),
        )),
        (false, true) => {
            ensure_variants(s, &variants)?;
            Ok(CaptureTarget::Variants {
                variant_ids: variants,
            })
        }
        (true, false) => {
            let mut seen = BTreeSet::new();
            let mut devices: Vec<CaptureDeviceKey> = Vec::new();
            for v in device_of {
                let d = s.core.capture_device_of(v)?;
                let key = (d.camera.clone(), d.serial.clone(), d.kind.as_str());
                if seen.insert(key) {
                    devices.push(d);
                }
            }
            Ok(CaptureTarget::Devices { devices })
        }
    }
}

/// 修正（--dry-run なら見込みだけ）。
fn edit(
    s: &Session,
    out: Output,
    a: &TimeTargetArgs,
    reference: Option<VariantId>,
    e: &CaptureTimeEdit,
) -> CliResult<Status> {
    let target = target_of(s, a, reference)?;
    let plan = if a.dry_run {
        s.core.plan_capture_time_edit(&target, e)?
    } else {
        s.core.edit_capture_times(&target, e)?
    };
    if out.json {
        out.print_json(&json!({
            "dry_run": a.dry_run,
            "target": json_value(&target),
            "plan": json_value(&plan),
        }));
    } else {
        for line in describe_plan(&plan, a.dry_run) {
            out.line(line);
        }
    }
    Ok(Status::Success)
}

/// 秒の量を「+1 日 2 時間 3 分 4 秒」の形にする（画面の ui/src/library/captureTimeLogic.ts の formatShift と
/// 同じ）。
pub fn format_shift(seconds: i64) -> String {
    if seconds == 0 {
        return "±0 秒".to_owned();
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let mut rest = seconds.unsigned_abs();
    let mut parts = Vec::new();
    for (unit, name) in [(86_400u64, "日"), (3_600, "時間"), (60, "分"), (1, "秒")] {
        let n = rest / unit;
        rest %= unit;
        if n > 0 {
            parts.push(format!("{n} {name}"));
        }
    }
    format!("{sign}{}", parts.join(" "))
}

/// 修正の履歴の 1 件の説明。
fn describe_edit(e: &CaptureTimeEditInfo) -> String {
    match e.kind.as_str() {
        "shift" => format!("ずらす {}", format_shift(e.shift_seconds.unwrap_or(0))),
        "sync" => format!(
            "基準の写真に合わせる {}",
            format_shift(e.shift_seconds.unwrap_or(0))
        ),
        "set_zone" => format!(
            "タイムゾーン {}（{}）",
            e.zone.as_deref().unwrap_or("?"),
            match e.zone_mode {
                Some(ZoneMode::KeepInstant) => "撮影した瞬間はそのまま",
                _ => "記録の時刻をその地域の時刻として読む",
            }
        ),
        "clear_zone" => "タイムゾーンの指定を取り消す".to_owned(),
        "reset" => "ファイルの値に戻す".to_owned(),
        other => other.to_owned(),
    }
}

fn capture_text(c: &CaptureTime) -> String {
    c.to_string()
}

/// 見込み・結果の文。
fn describe_plan(p: &CaptureTimePlan, dry_run: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(s) = p.shift_seconds {
        lines.push(format!("ずらす量: {}", format_shift(s)));
    }
    if dry_run {
        lines.push(format!(
            "{} 枚の撮影日時が変わります（対象 {} 枚。--dry-run のため変えていません）",
            p.changed, p.assets
        ));
    } else {
        lines.push(format!(
            "{} 枚の撮影日時を変えました（対象 {} 枚。time undo で元に戻せます）",
            p.changed, p.assets
        ));
    }
    if p.unchanged > 0 {
        lines.push(format!("{} 枚は変わりません", p.unchanged));
    }
    if p.without_time > 0 {
        lines.push(format!(
            "撮影日時の分からない {} 枚は変えません",
            p.without_time
        ));
    }
    let src = &p.sources;
    let mut parts = Vec::new();
    if src.exif > 0 {
        parts.push(format!("オフセットの記録あり {}", src.exif));
    }
    if src.user_default > 0 {
        parts.push(format!(
            "記録なし（既定のタイムゾーンで推定）{}",
            src.user_default
        ));
    }
    if src.utc_only > 0 {
        parts.push(format!("UTC の印だけ（動画など）{}", src.utc_only));
    }
    if src.user_set > 0 {
        parts.push(format!("タイムゾーンを指定済み {}", src.user_set));
    }
    if !parts.is_empty() {
        lines.push(format!("撮影日時の記録: {}", parts.join("・")));
    }
    for e in &p.examples {
        lines.push(format!(
            "  {}: {} → {}",
            e.file_name,
            capture_text(&e.before),
            capture_text(&e.after)
        ));
    }
    lines
}

/// `timeline`。
pub fn timeline(g: &GlobalArgs, out: Output, a: &TimelineArgs) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = timeline_in(&s, out, a);
    s.close()?;
    r
}

fn timeline_in(s: &Session, out: Output, a: &TimelineArgs) -> CliResult<Status> {
    let filter = SearchFilter {
        expand_stacks: a.expand_stacks,
        ..filter_of(s, &a.filter)?
    };
    let sort = SearchSort {
        key: SortKey::CaptureTime,
        direction: direction(a.desc),
    };
    let result = s.core.search(&filter, sort)?;
    let t = s.core.timeline(result.generation, a.gap)?;
    if out.json {
        out.print_json(&json_value(&t));
        return Ok(Status::Success);
    }
    let offset = utc_offset_text(s.core.settings().default_utc_offset_minutes);
    let mut table = Table::new(["撮影日", "時刻", "写真", "動画", "位置"]);
    for g in &t.groups {
        let day = match (&g.day, g.new_day) {
            (Some(d), true) => d.clone(),
            (Some(_), false) => String::new(),
            (None, _) => "（撮影日時なし）".to_owned(),
        };
        let time = match (&g.first, &g.last) {
            (Some(f), Some(l)) if f.get(11..16) != l.get(11..16) => format!(
                "{}〜{}",
                f.get(11..16).unwrap_or(""),
                l.get(11..16).unwrap_or("")
            ),
            (Some(f), _) => f.get(11..16).unwrap_or("").to_owned(),
            _ => String::new(),
        };
        table.row([
            day,
            time,
            g.photos.to_string(),
            g.videos.to_string(),
            format!("{}〜{}", g.start + 1, g.start + g.count),
        ]);
    }
    out.table(&table);
    out.line(format!(
        "{} 件・{} 日・{} のまとまり（イベントの区切り: {}。時刻は写真ごとの表示のタイムゾーン、既定は {offset}）",
        result.count,
        t.days,
        t.groups.len(),
        if t.gap_seconds == 0 {
            "なし".to_owned()
        } else {
            format_shift(i64::from(t.gap_seconds))
                .trim_start_matches('+')
                .to_owned()
        }
    ));
    if t.events_merged {
        out.warn("まとまりが多すぎるため、イベントで分けずに日付だけでまとめました");
    }
    Ok(Status::Success)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{parse_gap, parse_shift, parse_zone_minutes};

    #[test]
    fn shifts_are_parsed_and_formatted() {
        assert_eq!(parse_shift("+1h"), Ok(3_600));
        assert_eq!(parse_shift("-90s"), Ok(-90));
        assert_eq!(parse_shift("1d2h3m4s"), Ok(93_784));
        assert_eq!(parse_shift("3600"), Ok(3_600));
        assert_eq!(parse_shift("-3600"), Ok(-3_600));
        assert_eq!(parse_shift(" +2H30M "), Ok(9_000));
        for bad in ["", "+", "1x", "h", "1h1d", "1h1h", "1h2", "１h"] {
            assert!(parse_shift(bad).is_err(), "{bad:?}");
        }
        assert_eq!(format_shift(93_784), "+1 日 2 時間 3 分 4 秒");
        assert_eq!(format_shift(-60), "-1 分");
        assert_eq!(format_shift(0), "±0 秒");
        assert_eq!(parse_gap("2h"), Ok(7_200));
        assert_eq!(parse_gap("0"), Ok(0));
        assert!(parse_gap("25h").is_err());
        assert!(parse_gap("-1h").is_err());
        assert_eq!(parse_zone_minutes("+09:00"), Ok(540));
        assert_eq!(parse_zone_minutes("-0530"), Ok(-330));
        assert!(parse_zone_minutes("JST").is_err());
    }

    #[test]
    fn plans_are_described() {
        let p = CaptureTimePlan {
            assets: 5,
            changed: 3,
            unchanged: 1,
            without_time: 1,
            shift_seconds: Some(3_723),
            sources: genzo_api::CaptureSourceCounts {
                user_default: 3,
                utc_only: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = describe_plan(&p, true);
        assert_eq!(lines[0], "ずらす量: +1 時間 2 分 3 秒");
        assert!(lines[1].starts_with("3 枚の撮影日時が変わります（対象 5 枚。"));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("UTC の印だけ（動画など）1"))
        );
        let e = CaptureTimeEditInfo {
            id: 1,
            kind: "set_zone".to_owned(),
            shift_seconds: None,
            zone: Some("+02:00".to_owned()),
            zone_mode: Some(ZoneMode::KeepInstant),
            asset_count: 2,
            created_at: chrono::Utc::now(),
        };
        assert_eq!(
            describe_edit(&e),
            "タイムゾーン +02:00（撮影した瞬間はそのまま）"
        );
    }
}
