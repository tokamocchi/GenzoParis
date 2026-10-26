//! `develop retouch` のサブコマンド（スポット修正。DEV-21。04 の 2.1 節「スポット修正」）。
//!
//! - スポットは **1 から数えた番号** で指定する（アプリの「スポット 1」などの表示と同じ。設定の JSON の
//!   `retouch` の位置は 0 から）。位置・大きさ・コピー元のずれは O 座標（RAW の向きを反映した画像の長辺を 1 と
//!   する。左上が 0,0。角度補正・切り抜きの前の画像）。
//! - `list` は写真を開かずに設定を読む。`add`・`set`・`remove` は `develop set` と同じく写真を開かずに保存する
//!   （履歴に 1 件。名前は genzo-api の `describe_change`。例:「スポット修正を追加」「スポット 2 の不透明度」）。
//!   値の範囲・数の上限は genzo-model の検証で確かめる。
//! - `add` で `--source` を省くと、写真を開いて（展開して）コピー元を自動で選ぶ（genzo-api の
//!   `retouch_auto_source`）。よい候補がなければ、右（はみ出すなら左）に大きさの 2.5 倍ずらす。
//! - 処理バージョン 1 の写真ではスポット修正は使われないので、変えたときに警告する。
//! - JSON の出力: `{"variant_id", "changed"（変更のとき）, "retouch": [{"number", ...スポットの JSON}]}`。

use genzo_api::{DevelopSettings, VariantId};
use genzo_model::{RetouchMode, RetouchSpot};
use serde_json::{Value, json};

use crate::Status;
use crate::args::{GlobalArgs, RetouchCommand, RetouchValueArgs, SwitchArg};
use crate::develop::save;
use crate::error::{CliError, CliResult};
use crate::library::ensure_variants;
use crate::output::{Output, Table, json_value};
use crate::session::{OpenOptions, Session};

/// `develop retouch` を実行する。
pub fn run(g: &GlobalArgs, out: Output, cmd: RetouchCommand) -> CliResult<Status> {
    match cmd {
        RetouchCommand::List { variant } => list(g, out, variant),
        RetouchCommand::Add {
            variant,
            at,
            values,
        } => change(g, out, variant, |s, settings| {
            let mut spot = RetouchSpot::new(at, [0.0, 0.0]);
            apply_values(&mut spot, &values);
            if values.source.is_none() {
                spot.offset = auto_source(s, variant, &spot)?;
            }
            settings.retouch.push(spot);
            Ok(())
        }),
        RetouchCommand::Set {
            variant,
            spot,
            enabled,
            values,
        } => change(g, out, variant, |_, settings| {
            let i = index_of(settings, spot)?;
            let r = &mut settings.retouch[i];
            if let Some(e) = enabled {
                r.enabled = e == SwitchArg::On;
            }
            apply_values(r, &values);
            Ok(())
        }),
        RetouchCommand::Remove { variant, spot, all } => change(g, out, variant, |_, settings| {
            if all {
                settings.retouch.clear();
            } else if let Some(n) = spot {
                let i = index_of(settings, n)?;
                settings.retouch.remove(i);
            }
            Ok(())
        }),
    }
}

/// 指定した値をスポットに当てる（指定しない項目はそのまま）。
fn apply_values(spot: &mut RetouchSpot, v: &RetouchValueArgs) {
    if let Some(m) = v.mode {
        spot.mode = m.mode();
    }
    if let Some(x) = v.size {
        spot.size = x;
    }
    if let Some(x) = v.feather {
        spot.feather = x;
    }
    if let Some(x) = v.opacity {
        spot.opacity = x;
    }
    if let Some(x) = v.source {
        spot.offset = x;
    }
}

/// 写真を開いてコピー元を自動で選ぶ（閉じてから返す）。候補がなければ大きさの 2.5 倍だけ右、はみ出すなら左。
fn auto_source(s: &Session, v: VariantId, spot: &RetouchSpot) -> CliResult<[f32; 2]> {
    ensure_variants(s, &[v])?;
    s.core.open_develop(v)?;
    let picked = s.core.retouch_auto_source(v, spot);
    s.core.close_develop()?;
    Ok(picked?.unwrap_or_else(|| fallback_offset(spot)))
}

/// 自動で選べなかったときのずれ（大きさの 2.5 倍だけ右。点が右半分にあれば左）。
fn fallback_offset(spot: &RetouchSpot) -> [f32; 2] {
    let d = spot.size * 2.5;
    let right_half = spot.points.first().is_some_and(|p| p[0] > 0.5);
    [if right_half { -d } else { d }, 0.0]
}

/// スポットの番号（1 から）を設定の位置にする（範囲の外は入力の誤り）。
fn index_of(settings: &DevelopSettings, number: usize) -> CliResult<usize> {
    let n = settings.retouch.len();
    if number == 0 || number > n {
        return Err(CliError::input(if n == 0 {
            format!(
                "スポット {number} はありません（スポットがありません。genzo develop retouch add で足せます）"
            )
        } else {
            format!("スポット {number} はありません（スポットは {n} 個）")
        }));
    }
    Ok(number - 1)
}

/// スポットの JSON（番号を足す）。
fn spots_json(settings: &DevelopSettings) -> Vec<Value> {
    settings
        .retouch
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut v = json_value(r);
            if let Value::Object(o) = &mut v {
                o.insert("number".to_owned(), json!(i + 1));
            }
            v
        })
        .collect()
}

/// 形の要約（「円 (0.4100, 0.2200)」「線 12 点」）。
fn shape_text(r: &RetouchSpot) -> String {
    match r.points.as_slice() {
        [] => "（点なし）".to_owned(),
        [p] => format!("円 ({:.4}, {:.4})", p[0], p[1]),
        ps => format!("線 {} 点", ps.len()),
    }
}

fn print_spots(out: Output, v: VariantId, settings: &DevelopSettings, changed: Option<bool>) {
    if out.json {
        let mut obj = json!({ "variant_id": v, "retouch": spots_json(settings) });
        if let Some(c) = changed {
            obj["changed"] = json!(c);
        }
        out.print_json(&obj);
        return;
    }
    if settings.retouch.is_empty() {
        out.line("スポット修正はありません");
        return;
    }
    let mut t = Table::new([
        "番号",
        "有効",
        "直し方",
        "形",
        "大きさ",
        "ぼかし",
        "不透明度",
        "コピー元のずれ",
    ]);
    for (i, r) in settings.retouch.iter().enumerate() {
        t.row([
            (i + 1).to_string(),
            if r.enabled { "はい" } else { "いいえ" }.to_owned(),
            match r.mode {
                RetouchMode::Heal => "修復",
                RetouchMode::Clone => "コピースタンプ",
            }
            .to_owned(),
            shape_text(r),
            format!("{:.4}", r.size),
            format!("{:.0}", r.feather),
            format!("{:.0}", r.opacity),
            format!("({:+.4}, {:+.4})", r.offset[0], r.offset[1]),
        ]);
    }
    out.table(&t);
}

fn list(g: &GlobalArgs, out: Output, v: VariantId) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let settings = s.core.develop_settings(v)?;
    s.close()?;
    print_spots(out, v, &settings, None);
    Ok(Status::Success)
}

/// 設定のスポットを変えて保存する（検証してから。変わらなければ履歴に記録しない）。
fn change(
    g: &GlobalArgs,
    out: Output,
    v: VariantId,
    edit: impl FnOnce(&Session, &mut DevelopSettings) -> CliResult<()>,
) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let current = s.core.develop_settings(v)?;
    let mut new = current.clone();
    edit(&s, &mut new)?;
    new.validate().map_err(genzo_model::DevelopError::from)?;
    let changed = save(&s, v, &new, None)?;
    let saved = s.core.develop_settings(v)?;
    s.close()?;
    if !changed && !out.json {
        out.progress("設定は変わっていません（履歴に記録しませんでした）");
    }
    if changed
        && saved.process_version < genzo_pipeline::ProcessVersion::V2.get()
        && !saved.retouch.is_empty()
    {
        out.warn(format!(
            "この写真の処理バージョン（{}）では、スポット修正は使われません（genzo develop upgrade {v} で更新できます）",
            saved.process_version
        ));
    }
    print_spots(out, v, &saved, Some(changed));
    Ok(Status::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_numbers_start_at_one() {
        let mut s = DevelopSettings::default();
        assert!(index_of(&s, 1).is_err());
        s.retouch
            .push(RetouchSpot::new(vec![[0.2, 0.3]], [0.05, 0.0]));
        assert_eq!(index_of(&s, 1).unwrap(), 0);
        assert!(index_of(&s, 0).is_err());
        assert!(index_of(&s, 2).is_err());
    }

    #[test]
    fn summaries_and_fallback_offsets() {
        let circle = RetouchSpot::new(vec![[0.41, 0.22]], [0.03, 0.0]);
        assert_eq!(shape_text(&circle), "円 (0.4100, 0.2200)");
        let line = RetouchSpot::new(vec![[0.1, 0.5], [0.6, 0.45], [0.8, 0.4]], [0.0, 0.01]);
        assert_eq!(shape_text(&line), "線 3 点");
        let d = RetouchSpot::DEFAULT_SIZE * 2.5;
        assert_eq!(fallback_offset(&circle), [d, 0.0]);
        let right = RetouchSpot::new(vec![[0.9, 0.5]], [0.0, 0.0]);
        assert_eq!(fallback_offset(&right), [-d, 0.0]);
    }
}
