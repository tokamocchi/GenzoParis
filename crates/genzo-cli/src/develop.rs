//! `develop` のサブコマンド（DEV-00〜08・DEV-27・DEV-30、LIB-13。04 の 2.5 節）。
//!
//! - `get` / `set` / `reset` は写真を開かずに（展開せずに）カタログの設定を読み書きする。保存は
//!   genzo-api の [`genzo_api::Core::paste_settings_with_label`]（1 つのトランザクションで履歴に 1 件）。
//!   履歴の名前は `set` では変わった項目（[`genzo_api::describe_change`]。例: 「露光量 +0.50」）、`reset`
//!   では [`HISTORY_LABEL_RESET`]。
//! - `set` の JSON は **部分的な指定を今の設定にマージする**（[`merge_json`]）。オブジェクトは項目ごとに
//!   再帰的に合わせ、配列と値は置き換える。WB のような「種類を 1 つのキーで表す」値（`{"preset": ...}` と
//!   `{"custom": {...}}`）は、種類が違えば置き換える。知らない項目（打ち間違い）はエラーにする
//!   （genzo-model は知らない項目を無視するため、ここで確かめる）。
//! - 設定の値の範囲は genzo-model の検証（`DevelopSettings::validate`）で確かめる。
//! - JSON の出力: `get` は設定そのもの（そのまま `set --replace` に渡せる）。`set`・`reset`・`undo`・
//!   `redo` は `{"variant_id", "changed", "settings"}`（Undo / Redo できなければ `changed: false`、
//!   `settings: null`。終了コードは 0）。

use std::io::Read;

use genzo_api::{DeleteKind, DevelopSettings, SettingGroups, VariantId};
use serde_json::{Value, json};

use crate::Status;
use crate::args::{DevelopCommand, GlobalArgs, setting_groups};
use crate::error::{CliError, CliResult};
use crate::library::{ensure_variants, unique_ids};
use crate::output::{Output, Table, json_value};
use crate::session::{OpenOptions, Session};

/// `develop reset` の履歴の名前。
pub const HISTORY_LABEL_RESET: &str = "現像設定の初期化";

/// `develop` を実行する。
pub fn run(g: &GlobalArgs, out: Output, cmd: DevelopCommand) -> CliResult<Status> {
    match cmd {
        DevelopCommand::Get { variant } => get(g, out, variant),
        DevelopCommand::Set {
            variant,
            settings,
            replace,
        } => {
            let Some(source) = settings else {
                return Err(CliError::Usage(
                    "現像設定の JSON を --json <FILE>（または --settings <FILE>。- で標準入力）で指定してください"
                        .to_owned(),
                ));
            };
            let patch = read_json_input(&source)?;
            set(g, out, variant, &patch, replace)
        }
        DevelopCommand::Reset { variant } => reset(g, out, variant),
        DevelopCommand::Undo { variant } => step(g, out, variant, false),
        DevelopCommand::Redo { variant } => step(g, out, variant, true),
        DevelopCommand::History { variant } => history(g, out, variant),
        DevelopCommand::Copy { from, to, groups } => {
            copy(g, out, from, &to, setting_groups(&groups))
        }
        DevelopCommand::VirtualCopy { variant, name } => virtual_copy(g, out, variant, name),
        DevelopCommand::DeleteCopy { variant } => {
            crate::library::delete(g, out, DeleteKind::VirtualCopies, &[variant], true)
        }
    }
}

/// JSON の入力を読む（`-` なら標準入力）。文字コードは [`decode_text`]。
pub fn read_json_input(source: &str) -> CliResult<Value> {
    let name = if source == "-" {
        "（標準入力）"
    } else {
        source
    };
    let bytes = if source == "-" {
        let mut b = Vec::new();
        std::io::stdin()
            .read_to_end(&mut b)
            .map_err(|e| CliError::io(name, e))?;
        b
    } else {
        std::fs::read(source).map_err(|e| {
            CliError::classified(
                crate::error::io_kind(&e),
                format!("現像設定のファイル {source} を読めません: {e}"),
                None,
            )
        })?
    };
    let text = decode_text(&bytes).map_err(|why| {
        CliError::input(format!(
            "現像設定の JSON（{name}）を読めません: {why}（UTF-8 で保存してください）"
        ))
    })?;
    serde_json::from_str(&text)
        .map_err(|e| CliError::input(format!("現像設定の JSON（{name}）を解析できません: {e}")))
}

/// テキストの入力を文字列にする: UTF-8（BOM 付きも）と、BOM 付きの UTF-16（LE / BE）。
///
/// Windows では、メモ帳の古い版は UTF-8 に BOM を付け、Windows PowerShell 5.1 の `>`・`Out-File` は
/// BOM 付きの UTF-16LE で保存する。どちらもそのまま読めるようにする（それ以外の文字コード（Shift_JIS
/// など）は誤りにする）。
pub fn decode_text(bytes: &[u8]) -> Result<String, String> {
    let utf16 = |rest: &[u8], le: bool| -> Result<String, String> {
        if !rest.len().is_multiple_of(2) {
            return Err("UTF-16 のバイト数が奇数です".to_owned());
        }
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| "UTF-16 として正しくありません".to_owned())
    };
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(rest.to_vec())
            .map_err(|_| "UTF-8 として正しくありません".to_owned());
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "UTF-8 として正しくありません".to_owned())
}

/// `patch` を `base` に合わせる（オブジェクトは再帰的に、それ以外は置き換え）。
///
/// どちらもキーが 1 つのオブジェクトで、キーが違う場合は置き換える（serde の外部タグの列挙。
/// WB の `{"preset": "daylight"}` → `{"custom": {...}}` など）。
pub fn merge_json(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            let tag_changed = b.len() == 1 && p.len() == 1 && b.keys().next() != p.keys().next();
            if tag_changed {
                *b = p.clone();
                return;
            }
            for (k, v) in p {
                match b.get_mut(k) {
                    Some(bv) => merge_json(bv, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}

/// `patch` の項目がすべて `known`（設定を JSON にしたもの）にあるか確かめる。なければ、その項目の
/// パス（`tone.shadowz` など）を返す。
pub fn unknown_field(known: &Value, patch: &Value, prefix: &str) -> Option<String> {
    let (Value::Object(k), Value::Object(p)) = (known, patch) else {
        return None;
    };
    for (key, v) in p {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match k.get(key) {
            None => return Some(path),
            Some(kv) => {
                if let Some(u) = unknown_field(kv, v, &path) {
                    return Some(u);
                }
            }
        }
    }
    None
}

/// 今の設定に JSON をマージした設定を作って検証する（`replace` なら既定の設定に合わせる）。
pub fn merged_settings(
    current: &DevelopSettings,
    patch: &Value,
    replace: bool,
) -> CliResult<DevelopSettings> {
    if !patch.is_object() {
        return Err(CliError::input(
            "現像設定の JSON の最上位はオブジェクト（{...}）にしてください",
        ));
    }
    let base = if replace {
        DevelopSettings::default()
    } else {
        current.clone()
    };
    let mut value = serde_json::to_value(&base).map_err(CliError::other)?;
    merge_json(&mut value, patch);
    let settings = DevelopSettings::from_json(&value.to_string())?;
    let known = serde_json::to_value(&settings).map_err(CliError::other)?;
    if let Some(field) = unknown_field(&known, patch, "") {
        return Err(CliError::input(format!(
            "現像設定に {field} という項目はありません（genzo develop get で項目の名前を確かめてください）"
        )));
    }
    settings
        .validate()
        .map_err(genzo_model::DevelopError::from)?;
    Ok(settings)
}

fn print_settings(out: Output, variant: VariantId, settings: &DevelopSettings, changed: bool) {
    if out.json {
        out.print_json(&json!({
            "variant_id": variant,
            "changed": changed,
            "settings": json_value(settings),
        }));
    } else {
        out.line(serde_json::to_string_pretty(settings).unwrap_or_default());
    }
}

fn get(g: &GlobalArgs, out: Output, v: VariantId) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let settings = s.core.develop_settings(v)?;
    s.close()?;
    if out.json {
        out.print_json(&settings);
    } else {
        out.line(serde_json::to_string_pretty(&settings).unwrap_or_default());
    }
    Ok(Status::Success)
}

/// 設定を保存する（変わっていなければ何もしない）。保存したら `true`。
fn save(s: &Session, v: VariantId, new: &DevelopSettings, label: Option<&str>) -> CliResult<bool> {
    let current = s.core.develop_settings(v)?;
    if &current == new {
        return Ok(false);
    }
    let label = label
        .map(str::to_owned)
        .unwrap_or_else(|| genzo_api::describe_change(&current, new));
    s.core
        .paste_settings_with_label(new, &[v], SettingGroups::ALL, &label)?;
    Ok(true)
}

fn set(
    g: &GlobalArgs,
    out: Output,
    v: VariantId,
    patch: &Value,
    replace: bool,
) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let current = s.core.develop_settings(v)?;
    let new = merged_settings(&current, patch, replace)?;
    let changed = save(&s, v, &new, None)?;
    let saved = s.core.develop_settings(v)?;
    s.close()?;
    if !changed && !out.json {
        out.progress("設定は変わっていません（履歴に記録しませんでした）");
    }
    print_settings(out, v, &saved, changed);
    Ok(Status::Success)
}

fn reset(g: &GlobalArgs, out: Output, v: VariantId) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let current = s.core.develop_settings(v)?;
    // 処理バージョンは今のまま（初期化で見た目の基準を変えない。DevelopSettings の既定は最新の版）。
    let new = DevelopSettings {
        process_version: current.process_version,
        render_deps: current.render_deps.clone(),
        ..DevelopSettings::default()
    };
    let changed = save(&s, v, &new, Some(HISTORY_LABEL_RESET))?;
    let saved = s.core.develop_settings(v)?;
    s.close()?;
    print_settings(out, v, &saved, changed);
    Ok(Status::Success)
}

fn step(g: &GlobalArgs, out: Output, v: VariantId, forward: bool) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &[v])?;
    let result = if forward {
        s.core.redo(v)?
    } else {
        s.core.undo(v)?
    };
    s.close()?;
    let what = if forward {
        "やり直せる"
    } else {
        "取り消せる"
    };
    match &result {
        Some(settings) => print_settings(out, v, settings, true),
        None => {
            if out.json {
                out.print_json(&json!({ "variant_id": v, "changed": false, "settings": null }));
            } else {
                out.line(format!("{what}履歴がありません"));
            }
        }
    }
    Ok(Status::Success)
}

fn history(g: &GlobalArgs, out: Output, v: VariantId) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &[v])?;
    let entries = s.core.history(v)?;
    s.close()?;
    if out.json {
        out.print_json(&entries);
    } else {
        let mut t = Table::new(["ID", "日時", "操作", "現在"]);
        for h in &entries {
            t.row([
                h.id.to_string(),
                h.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                h.label.clone(),
                if h.is_current { "*" } else { "" }.to_owned(),
            ]);
        }
        out.table(&t);
    }
    Ok(Status::Success)
}

fn copy(
    g: &GlobalArgs,
    out: Output,
    from: VariantId,
    to: &[VariantId],
    groups: SettingGroups,
) -> CliResult<Status> {
    let to = unique_ids(to);
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &to)?;
    let source = s.core.develop_settings(from)?;
    let n = s.core.paste_settings(&source, &to, groups)?;
    s.close()?;
    if out.json {
        out.print_json(&json!({ "from": from, "updated": n }));
    } else {
        out.line(format!("variant {from} の設定を {n} 件に適用しました"));
    }
    Ok(Status::Success)
}

fn virtual_copy(
    g: &GlobalArgs,
    out: Output,
    v: VariantId,
    name: Option<String>,
) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let id = s.core.create_virtual_copy(v, name.as_deref())?;
    s.close()?;
    if out.json {
        out.print_json(&json!({ "variant_id": id, "from": v }));
    } else {
        out.line(format!("仮想コピーを作りました: variant {id}（元: {v}）"));
    }
    Ok(Status::Success)
}

#[cfg(test)]
mod tests {
    use super::*;
    use genzo_model::WhiteBalance;

    #[test]
    fn partial_json_is_merged_into_the_current_settings() {
        let mut current = DevelopSettings {
            exposure_ev: 0.3,
            contrast: 20.0,
            ..Default::default()
        };
        current.tone.highlights = -30.0;
        let s = merged_settings(&current, &json!({"tone": {"shadows": 40.0}}), false).unwrap();
        assert_eq!(s.exposure_ev, 0.3, "指定していない項目は残す");
        assert_eq!(s.contrast, 20.0);
        assert_eq!(s.tone.highlights, -30.0);
        assert_eq!(s.tone.shadows, 40.0);
        // replace では既定の設定に合わせる。
        let r = merged_settings(&current, &json!({"tone": {"shadows": 40.0}}), true).unwrap();
        assert_eq!(r.exposure_ev, 0.0);
        assert_eq!(r.tone.shadows, 40.0);
    }

    #[test]
    fn enum_values_are_replaced_when_the_kind_changes() {
        let current = DevelopSettings {
            white_balance: WhiteBalance::Custom {
                temperature_k: 5000.0,
                tint: 3.0,
            },
            ..Default::default()
        };
        // 同じ種類なら項目ごとに合わせる。
        let s = merged_settings(
            &current,
            &json!({"white_balance": {"custom": {"tint": 8.0}}}),
            false,
        )
        .unwrap();
        assert_eq!(
            s.white_balance,
            WhiteBalance::Custom {
                temperature_k: 5000.0,
                tint: 8.0
            }
        );
        // 種類が違えば置き換える。
        let s = merged_settings(
            &current,
            &json!({"white_balance": {"preset": "daylight"}}),
            false,
        )
        .unwrap();
        assert_eq!(
            s.white_balance,
            WhiteBalance::Preset(genzo_model::WbPreset::Daylight)
        );
        let s = merged_settings(&current, &json!({"white_balance": "as_shot"}), false).unwrap();
        assert_eq!(s.white_balance, WhiteBalance::AsShot);
    }

    #[test]
    fn unknown_fields_and_invalid_values_are_errors() {
        let current = DevelopSettings::default();
        let e = merged_settings(&current, &json!({"tone": {"shadowz": 1.0}}), false).unwrap_err();
        assert!(e.to_string().contains("tone.shadowz"), "{e}");
        let e = merged_settings(&current, &json!({"exposure": 1.0}), false).unwrap_err();
        assert!(e.to_string().contains("exposure"), "{e}");
        assert!(merged_settings(&current, &json!({"exposure_ev": 99.0}), false).is_err());
        assert!(merged_settings(&current, &json!({"exposure_ev": "a"}), false).is_err());
        assert!(merged_settings(&current, &json!([1, 2]), false).is_err());
    }

    #[test]
    fn input_errors_are_classified_as_invalid_input() {
        let current = DevelopSettings::default();
        for patch in [json!({"tone": {"shadowz": 1.0}}), json!([1])] {
            let e = merged_settings(&current, &patch, false).unwrap_err();
            assert_eq!(e.to_json()["error"]["kind"], "invalid_input", "{patch}");
            assert_eq!(e.exit_code(), crate::EXIT_ERROR);
        }
    }

    #[test]
    fn text_input_accepts_utf8_with_or_without_bom_and_utf16() {
        let text = r#"{"exposure_ev": 0.5, "caption": "夕焼け"}"#;
        assert_eq!(decode_text(text.as_bytes()).unwrap(), text);
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(text.as_bytes());
        assert_eq!(decode_text(&bom).unwrap(), text);
        let mut le = vec![0xFF, 0xFE];
        let mut be = vec![0xFE, 0xFF];
        for u in text.encode_utf16() {
            le.extend_from_slice(&u.to_le_bytes());
            be.extend_from_slice(&u.to_be_bytes());
        }
        assert_eq!(decode_text(&le).unwrap(), text);
        assert_eq!(decode_text(&be).unwrap(), text);
        // CRLF（Windows の改行）は JSON の空白として読める。
        let crlf = "{\r\n  \"exposure_ev\": 0.5\r\n}\r\n";
        let v: Value = serde_json::from_str(&decode_text(crlf.as_bytes()).unwrap()).unwrap();
        assert_eq!(v["exposure_ev"], 0.5);
        // Shift_JIS（「あ」= 0x82 0xA0）や、壊れた UTF-16 は誤り。
        assert!(decode_text(&[b'"', 0x82, 0xA0, b'"']).is_err());
        assert!(decode_text(&[0xFF, 0xFE, 0x41]).is_err());
        assert!(
            decode_text(&[0xFF, 0xFE, 0x00, 0xD8]).is_err(),
            "対のないサロゲート"
        );
    }

    #[test]
    fn arrays_are_replaced() {
        let mut base = json!({"a": [1, 2, 3], "b": {"c": 1, "d": 2}});
        merge_json(&mut base, &json!({"a": [9], "b": {"d": 5}}));
        assert_eq!(base, json!({"a": [9], "b": {"c": 1, "d": 5}}));
        assert_eq!(
            unknown_field(&base, &json!({"b": {"e": 1}}), ""),
            Some("b.e".into())
        );
        assert_eq!(unknown_field(&base, &json!({"b": {"c": 1}}), ""), None);
    }
}
