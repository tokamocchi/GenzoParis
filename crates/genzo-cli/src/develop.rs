//! `develop` のサブコマンド（DEV-00〜08・DEV-14・DEV-27・DEV-30、LIB-13、DATA-09。04 の 2.5 節）。
//!
//! - `get` / `set` / `reset` は写真を開かずに（展開せずに）カタログの設定を読み書きする。保存は
//!   genzo-api の [`genzo_api::Core::paste_settings_with`]（1 つのトランザクションで履歴に 1 件。処理バージョンは
//!   作った設定の値をそのまま保存する: [`genzo_api::PasteVersion::Source`]）。履歴の名前は `set` では変わった
//!   項目（[`genzo_api::describe_change`]。例: 「露光量 +0.50」）、`reset` では [`HISTORY_LABEL_RESET`]。
//! - `set` の処理バージョンは、JSON に `process_version` を書いたときだけ変わる（DATA-09。`--replace` でも、
//!   書かなければ今の版のまま: [`merged_settings`]）。
//! - `set` のレンズ補正（DEV-14）はオプション（`--lens-profile`・`--remove-ca`・`--distortion-scale`・
//!   `--vignetting-scale`・`--distortion`・`--vignetting`・`--vignetting-midpoint`・`--ca-red-cyan`・
//!   `--ca-blue-yellow`）でも指定できる（JSON の後に当てる。[`settings_with_options`]）。処理バージョン 1 の
//!   写真では使われないので警告する（[`lens_ignored`]）。
//! - `set` のハイライト復元（DEV-06）・ノイズ軽減（DEV-13）もオプション（`--highlight-recovery`・`--highlight-amount`・
//!   `--nr-luminance`・`--nr-luminance-detail`・`--nr-luminance-contrast`・`--nr-color`・`--nr-color-detail`）で
//!   指定できる（JSON の後、レンズ補正と同じく当てる）。処理バージョン 1 の写真では使われないので警告する
//!   （[`sensor_ignored`]）。
//! - `set` の白黒（DEV-25）・カラーミキサー（DEV-09）・カラーグレーディング（DEV-10）もオプション（`--black-white`・
//!   `--hsl-hue`・`--hsl-saturation`・`--hsl-luminance`・`--bw-mix`（`帯=値`。繰り返せる）・`--grading-shadows`・
//!   `--grading-midtones`・`--grading-highlights`・`--grading-global`（`色相,強さ[,輝度]`）・`--grading-blending`・
//!   `--grading-balance`）で指定できる（JSON の後に当てる）。処理バージョン 1 の写真では使われないので警告する
//!   （[`color_ignored`]）。
//! - `upgrade` は処理バージョンを最新に更新する（DATA-09。履歴の名前はアプリの更新と同じ
//!   [`genzo_api::upgrade_process_label`]）。複数の ID をまとめて更新できる（genzo-api の
//!   [`genzo_api::Core::upgrade_process_versions`]。すでに最新の写真・動画は飛ばす）。`source` は写真を開いて、
//!   元の画像の情報（埋め込みのレンズ補正の情報の要約を含む。genzo-api の `SourceInfo`）と処理バージョンを
//!   表示する。
//! - `copy` はコピー先の処理バージョンを変えない（`--upgrade-process-version` で、コピー元より古い写真を
//!   コピー元の版に上げる）。古い版のために写した項目の一部が使われない写真があれば警告する。
//! - `set` の JSON は **部分的な指定を今の設定にマージする**（[`merge_json`]）。オブジェクトは項目ごとに
//!   再帰的に合わせ、配列と値は置き換える。WB のような「種類を 1 つのキーで表す」値（`{"preset": ...}` と
//!   `{"custom": {...}}`）は、種類が違えば置き換える。知らない項目（打ち間違い）はエラーにする
//!   （genzo-model は知らない項目を無視するため、ここで確かめる）。
//! - 設定の値の範囲は genzo-model の検証（`DevelopSettings::validate`）で確かめる。
//! - `region` は写真を開いて（展開して）、表示範囲を描き（genzo-api の `render_region`。ルーペの等倍表示・
//!   拡大とパンのコア API を試すため。implementation_status の No.52）、PNG に書く。`--before` でビフォーの
//!   設定で描く（`render_before_region`。DEV-31）。
//! - `before` は写真を開いて、ビフォー / アフターの表示のビフォー（DEV-31。genzo-api の `set_before`・
//!   `render_before`）のプレビューを PNG に書く（`--after` でアフターも同じ寸法で書く）。
//! - `compare` は、比較表示（LIB-03）の同期ズームと同じ範囲（中心は写真ごとの割合、大きさは出力の画素で
//!   共通）を、写真ごとに比較表示の面（genzo-api の `view_output_size`・`render_view_region`。現像の
//!   セッションは開かない）に同時に描いて、PNG に書く。
//! - JSON の出力: `get` は設定そのもの（そのまま `set --replace` に渡せる）。`set`・`upgrade`（ID が 1 つ）・
//!   `reset`・`undo`・`redo` は `{"variant_id", "changed", "settings"}`（Undo / Redo できなければ
//!   `changed: false`、`settings: null`。終了コードは 0）。`upgrade` に ID が複数なら `{"updated"}`（更新した
//!   数）。`copy` は `{"from", "updated", "upgraded", "needs_upgrade"}`。`source` は `{"variant_id", "source",
//!   "process_version", "latest_process_version"}`（埋め込みのレンズ補正の情報が壊れていて使えなかった理由は
//!   `source.lens_correction_note`）。
//! - `mask` はローカル補正（マスク。DEV-18・DEV-19）の一覧・追加・変更・削除・重ねて表示の PNG・色のスポイト
//!   （`crate::masks`）。
//! - `snapshot` はスナップショット（DEV-28）の一覧・作成（今の状態・`--from-history` で履歴の項目の状態）・名前の
//!   変更・削除・適用（genzo-api の `snapshots`。適用は履歴に 1 件記録し、処理バージョンも保存した版に戻す）。

use std::io::Read;

use genzo_api::{
    DeleteKind, DevelopSettings, MAX_VIEW_PANES, PasteVersion, RegionRequest, RegionSpace,
    SettingGroups, VariantId, ViewOutputSize,
};
use serde_json::{Value, json};

use crate::Status;
use crate::args::{
    BeforeArgs, ColorArgs, CompareArgs, DetailArgs, DevelopCommand, GlobalArgs, LensArgs,
    ProfileArgs, RegionArgs, RegionSpaceArg, SensorArgs, SnapshotCommand, setting_groups,
};
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
            lens,
            sensor,
            color,
            detail,
            profile,
        } => {
            let patch = match settings {
                Some(source) => read_json_input(&source)?,
                None if lens.any()
                    || sensor.any()
                    || color.any()
                    || detail.any()
                    || profile.any() =>
                {
                    json!({})
                }
                None => {
                    return Err(CliError::Usage(
                        "現像設定の JSON を --json <FILE>（または --settings <FILE>。- で標準入力）か、レンズ補正・\
                         ハイライト復元・ノイズ軽減・色・ディテール・効果のオプション（--lens-profile・\
                         --highlight-recovery・--nr-luminance・--black-white・--hsl-hue・--grading-shadows・\
                         --clarity・--sharpen-amount・--grain-amount・--profile・--look など）で指定してください"
                            .to_owned(),
                    ));
                }
            };
            set(
                g,
                out,
                variant,
                &patch,
                replace,
                &OptionArgs {
                    lens: &lens,
                    sensor: &sensor,
                    color: &color,
                    detail: &detail,
                    profile: &profile,
                },
            )
        }
        DevelopCommand::Upgrade { variants } => upgrade(g, out, &variants),
        DevelopCommand::Source { variant } => source(g, out, variant),
        DevelopCommand::Reset { variant } => reset(g, out, variant),
        DevelopCommand::Undo { variant } => step(g, out, variant, false),
        DevelopCommand::Redo { variant } => step(g, out, variant, true),
        DevelopCommand::History { variant } => history(g, out, variant),
        DevelopCommand::Copy {
            from,
            to,
            groups,
            upgrade_process_version,
        } => copy(
            g,
            out,
            from,
            &to,
            setting_groups(&groups),
            if upgrade_process_version {
                PasteVersion::Upgrade
            } else {
                PasteVersion::Keep
            },
        ),
        DevelopCommand::VirtualCopy { variant, name } => virtual_copy(g, out, variant, name),
        DevelopCommand::DeleteCopy { variant } => {
            crate::library::delete(g, out, DeleteKind::VirtualCopies, &[variant], true)
        }
        DevelopCommand::RenameCopy { variant, name } => {
            crate::stacks::rename_copy(g, out, variant, &name)
        }
        DevelopCommand::MakeMaster { variant } => crate::stacks::make_master(g, out, variant),
        DevelopCommand::Region(a) => region(g, out, &a),
        DevelopCommand::Before(a) => before(g, out, &a),
        DevelopCommand::Compare(a) => compare(g, out, &a),
        DevelopCommand::Mask(c) => crate::masks::run(g, out, c),
        DevelopCommand::Snapshot(c) => snapshot(g, out, c),
    }
}

/// `develop snapshot`（DEV-28）。JSON の出力: `list` はスナップショットの配列、`create`・`rename` は
/// スナップショット（`{"id", "variant_id", "name", "created_at", "process_version"}`）、`delete` は
/// `{"deleted"}`、`apply` は `{"snapshot", "settings"}`。
fn snapshot(g: &GlobalArgs, out: Output, cmd: SnapshotCommand) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let r = snapshot_in(&s, out, cmd);
    s.close()?;
    r
}

fn snapshot_in(s: &Session, out: Output, cmd: SnapshotCommand) -> CliResult<Status> {
    let line = |snap: &genzo_api::SnapshotInfo, done: &str| {
        if out.json {
            out.print_json(snap);
        } else {
            out.line(format!(
                "スナップショット {}「{}」を{done}（variant {}・処理バージョン {}）",
                snap.id, snap.name, snap.variant_id, snap.process_version
            ));
        }
    };
    match cmd {
        SnapshotCommand::List { variant } => {
            ensure_variants(s, &[variant])?;
            let list = s.core.snapshots(variant)?;
            if out.json {
                out.print_json(&list);
            } else if list.is_empty() {
                out.line("スナップショットはありません");
            } else {
                let mut t = Table::new(["ID", "日時", "名前", "処理バージョン"]);
                for x in &list {
                    t.row([
                        x.id.to_string(),
                        x.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                        x.name.clone(),
                        x.process_version.to_string(),
                    ]);
                }
                out.table(&t);
            }
        }
        SnapshotCommand::Create {
            variant,
            name,
            from_history,
        } => {
            ensure_variants(s, &[variant])?;
            let source = match from_history {
                Some(entry_id) => genzo_api::SnapshotSource::History { entry_id },
                None => genzo_api::SnapshotSource::Current,
            };
            let snap = s.core.create_snapshot(variant, &name, source)?;
            line(&snap, "作りました");
        }
        SnapshotCommand::Rename { snapshot, name } => {
            let snap = s.core.rename_snapshot(snapshot, &name)?;
            line(&snap, "名前を変えました");
        }
        SnapshotCommand::Delete { snapshot } => {
            let snap = s.core.snapshot(snapshot)?;
            s.core.delete_snapshot(snapshot)?;
            if out.json {
                out.print_json(&json!({ "deleted": snap }));
            } else {
                out.line(format!(
                    "スナップショット {}「{}」を削除しました",
                    snap.id, snap.name
                ));
            }
        }
        SnapshotCommand::Apply { snapshot } => {
            let snap = s.core.snapshot(snapshot)?;
            let settings = s.core.apply_snapshot(snapshot)?;
            if out.json {
                out.print_json(&json!({ "snapshot": snap, "settings": settings }));
            } else {
                out.line(format!(
                    "スナップショット {}「{}」を variant {} に適用しました（履歴に記録しました）",
                    snap.id, snap.name, snap.variant_id
                ));
            }
        }
    }
    Ok(Status::Success)
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
///
/// 処理バージョンは、JSON に `process_version` を書いたときだけ変わる（DATA-09: 明示的な更新なしに変えない）。
/// `replace` の土台も、処理バージョンと外部データ（`render_deps`）は今の値（`reset` と同じ。既定の設定の版は
/// 最新なので、そのまま使うと版 1 の写真が黙って版 2 になる。検証の指摘）。
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
        DevelopSettings {
            process_version: current.process_version,
            render_deps: current.render_deps.clone(),
            ..DevelopSettings::default()
        }
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
pub(crate) fn save(
    s: &Session,
    v: VariantId,
    new: &DevelopSettings,
    label: Option<&str>,
) -> CliResult<bool> {
    let current = s.core.develop_settings(v)?;
    if &current == new {
        return Ok(false);
    }
    let label = label
        .map(str::to_owned)
        .unwrap_or_else(|| genzo_api::describe_change(&current, new));
    // 作った設定をそのまま保存する（処理バージョンも。`set` の JSON に書いた版・`upgrade` の版）。
    s.core
        .paste_settings_with(new, &[v], SettingGroups::ALL, PasteVersion::Source, &label)?;
    Ok(true)
}

/// `develop set` のオプション（レンズ補正・ハイライト復元とノイズ軽減・色）。
#[derive(Debug, Clone, Copy)]
pub struct OptionArgs<'a> {
    /// レンズ補正（DEV-14）。
    pub lens: &'a LensArgs,
    /// ハイライト復元・ノイズ軽減（DEV-06・DEV-13）。
    pub sensor: &'a SensorArgs,
    /// 白黒・カラーミキサー・カラーグレーディング（DEV-25・DEV-09・DEV-10）。
    pub color: &'a ColorArgs,
    /// テクスチャ・明瞭度・かすみの除去・シャープ・効果（DEV-11・DEV-12・DEV-24）。
    pub detail: &'a DetailArgs,
    /// カメラプロファイル・ルック（DEV-17）。
    pub profile: &'a ProfileArgs,
}

impl OptionArgs<'_> {
    /// どれかを指定したか。
    pub fn any(&self) -> bool {
        self.lens.any()
            || self.sensor.any()
            || self.color.any()
            || self.detail.any()
            || self.profile.any()
    }
}

/// JSON をマージした設定に、レンズ補正・ハイライト復元・ノイズ軽減・色のオプションを当てて検証する。
pub fn settings_with_options(
    current: &DevelopSettings,
    patch: &Value,
    replace: bool,
    options: &OptionArgs<'_>,
) -> CliResult<DevelopSettings> {
    let mut new = merged_settings(current, patch, replace)?;
    if options.any() {
        options.lens.apply(&mut new.lens);
        options.sensor.apply(&mut new);
        options.color.apply(&mut new.color);
        options.detail.apply(&mut new);
        options.profile.apply(&mut new.profile);
        new.validate().map_err(genzo_model::DevelopError::from)?;
    }
    Ok(new)
}

/// 処理バージョンが古く、白黒・カラーミキサー・カラーグレーディングが使われない設定か（DEV-25・DEV-09・DEV-10。
/// 処理バージョン 2 から使われる）。
pub fn color_ignored(settings: &DevelopSettings) -> bool {
    let c = &settings.color;
    settings.process_version < genzo_pipeline::ProcessVersion::V2.get()
        && (c.black_white.enabled || !c.mixer.is_neutral() || !c.grading.is_neutral())
}

/// 処理バージョンが古く、テクスチャ・明瞭度・かすみの除去・シャープ・効果が使われない設定か（DEV-11・DEV-12・
/// DEV-24。処理バージョン 2 から使われる。量が 0 のものは何もしないので知らせない）。
pub fn detail_ignored(settings: &DevelopSettings) -> bool {
    settings.process_version < genzo_pipeline::ProcessVersion::V2.get()
        && (!settings.presence.is_neutral()
            || !settings.detail.sharpening.is_disabled()
            || !settings.effects.is_neutral())
}

/// 処理バージョンが古く、ハイライト復元・ノイズ軽減が使われない設定か（DEV-06・DEV-13。処理バージョン 2 から
/// 使われる）。
pub fn sensor_ignored(settings: &DevelopSettings) -> bool {
    settings.process_version < genzo_pipeline::ProcessVersion::V2.get()
        && (!settings.highlight_recovery.is_disabled()
            || !settings.detail.noise_reduction.is_disabled())
}

/// カメラプロファイル・ルック（DEV-17）の指定についての注意: 処理バージョン 1 では使われない・取り込んでいない ID・
/// 読めない写し（`profiles` は取り込んだものの一覧）。
pub fn profile_warnings(
    profiles: &[genzo_api::ProfileEntry],
    settings: &DevelopSettings,
) -> Vec<String> {
    let p = &settings.profile;
    let mut out = Vec::new();
    if settings.process_version < genzo_pipeline::ProcessVersion::V2.get()
        && (p.camera.is_some() || p.look_is_active())
    {
        out.push(format!(
            "この写真の処理バージョン（{}）では、カメラプロファイル・ルックは使われません（genzo develop upgrade で更新できます）",
            settings.process_version
        ));
    }
    let find = |kind: genzo_api::ProfileKind, id: &str| {
        profiles.iter().find(|e| e.kind == kind && e.id == id)
    };
    let check = |kind: genzo_api::ProfileKind, label: &str, id: Option<&String>| {
        let id = id?;
        match find(kind, id) {
            None => Some(format!(
                "{label}「{id}」は取り込まれていません（genzo profile list で一覧を確かめ、genzo profile import で取り込んでください。描画では使われません）"
            )),
            Some(e) if !e.usable => Some(format!(
                "{label}「{id}」の取り込んだファイルが読めません（取り込み直してください。描画では使われません）"
            )),
            Some(_) => None,
        }
    };
    out.extend(check(
        genzo_api::ProfileKind::Camera,
        "カメラプロファイル",
        p.camera.as_ref(),
    ));
    if p.look_is_active() {
        out.extend(check(
            genzo_api::ProfileKind::Look,
            "ルック",
            p.look.as_ref(),
        ));
    }
    out
}

/// 処理バージョンが古く、レンズ補正が使われない設定か（DEV-14。処理バージョン 2 から使われる）。
pub fn lens_ignored(settings: &DevelopSettings) -> bool {
    settings.process_version < genzo_pipeline::ProcessVersion::V2.get()
        && settings.lens != genzo_model::LensCorrection::default()
}

fn set(
    g: &GlobalArgs,
    out: Output,
    v: VariantId,
    patch: &Value,
    replace: bool,
    options: &OptionArgs<'_>,
) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    let current = s.core.develop_settings(v)?;
    let new = settings_with_options(&current, patch, replace, options)?;
    let changed = save(&s, v, &new, None)?;
    let saved = s.core.develop_settings(v)?;
    let profiles = s.core.list_profiles(None)?;
    s.close()?;
    if !changed && !out.json {
        out.progress("設定は変わっていません（履歴に記録しませんでした）");
    }
    if lens_ignored(&saved) && saved.lens != current.lens {
        out.warn(format!(
            "この写真の処理バージョン（{}）では、レンズ補正は使われません（genzo develop upgrade {v} で更新できます）",
            saved.process_version
        ));
    }
    if sensor_ignored(&saved)
        && (saved.highlight_recovery != current.highlight_recovery
            || saved.detail.noise_reduction != current.detail.noise_reduction)
    {
        out.warn(format!(
            "この写真の処理バージョン（{}）では、ハイライト復元・ノイズ軽減は使われません（genzo develop upgrade {v} で更新できます）",
            saved.process_version
        ));
    }
    if detail_ignored(&saved)
        && (saved.presence != current.presence
            || saved.detail.sharpening != current.detail.sharpening
            || saved.effects != current.effects)
    {
        out.warn(format!(
            "この写真の処理バージョン（{}）では、テクスチャ・明瞭度・かすみの除去・シャープ・効果は使われません（genzo develop upgrade {v} で更新できます）",
            saved.process_version
        ));
    }
    if color_ignored(&saved) && saved.color != current.color {
        out.warn(format!(
            "この写真の処理バージョン（{}）では、白黒・カラーミキサー・カラーグレーディングは使われません（genzo develop upgrade {v} で更新できます）",
            saved.process_version
        ));
    }
    if saved.profile != current.profile {
        for w in profile_warnings(&profiles, &saved) {
            out.warn(format!("{w}（variant {v}）"));
        }
    }
    print_settings(out, v, &saved, changed);
    Ok(Status::Success)
}

/// 処理バージョンを最新に更新する（DATA-09。履歴の名前はアプリの更新と同じ）。ID が複数ならまとめて
/// 更新する（genzo-api の [`genzo_api::Core::upgrade_process_versions`]。1 つのトランザクション）。
fn upgrade(g: &GlobalArgs, out: Output, variants: &[VariantId]) -> CliResult<Status> {
    let variants = unique_ids(variants);
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &variants)?;
    let latest = genzo_model::CURRENT_PROCESS_VERSION;
    let n = s.core.upgrade_process_versions(&variants)?;
    if let [v] = variants[..] {
        let saved = s.core.develop_settings(v)?;
        s.close()?;
        if n == 0 && !out.json {
            out.progress(format!(
                "処理バージョンはすでに最新（{}）です（履歴に記録しませんでした）",
                saved.process_version
            ));
        }
        print_settings(out, v, &saved, n > 0);
        return Ok(Status::Success);
    }
    s.close()?;
    if out.json {
        out.print_json(&json!({ "updated": n }));
    } else {
        out.line(format!(
            "{n} 件の処理バージョンを {latest} に更新しました（すでに最新の写真・動画の {} 件はそのまま）",
            variants.len() - n
        ));
    }
    Ok(Status::Success)
}

/// 元の画像の情報と処理バージョンを表示する（写真を開いて展開する）。
fn source(g: &GlobalArgs, out: Output, v: VariantId) -> CliResult<Status> {
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &[v])?;
    let state = s.core.open_develop(v);
    // 開けたら閉じる（設定は変えていないので保存するものはない）。
    let closed = if state.is_ok() {
        s.core.close_develop()
    } else {
        Ok(())
    };
    s.close()?;
    let state = state?;
    closed?;
    let src = &state.source;
    if out.json {
        out.print_json(&json!({
            "variant_id": v,
            "source": json_value(src),
            "process_version": state.settings.process_version,
            "latest_process_version": state.latest_process_version,
        }));
        return Ok(Status::Success);
    }
    let mut t = Table::new(["項目", "値"]);
    t.row(vec![
        "種類".to_owned(),
        if src.is_raw { "RAW" } else { "RAW 以外" }.to_owned(),
    ]);
    t.row(vec![
        "元の大きさ".to_owned(),
        format!("{} × {}（向きを反映する前）", src.width, src.height),
    ]);
    t.row(vec![
        "デコーダ".to_owned(),
        src.decoder.clone().unwrap_or_else(|| "-".to_owned()),
    ]);
    t.row(vec![
        "機種".to_owned(),
        src.camera.clone().unwrap_or_else(|| "-".to_owned()),
    ]);
    t.row(vec![
        "撮影時の WB".to_owned(),
        src.as_shot_white_balance
            .map(|w| format!("{:.0} K、tint {:+.0}", w.temperature_k, w.tint))
            .unwrap_or_else(|| "-".to_owned()),
    ]);
    t.row(vec![
        "レンズ補正の情報".to_owned(),
        match (&src.lens_correction, &src.lens_correction_note) {
            (Some(l), _) => l.description.clone(),
            (None, Some(note)) => {
                format!(
                    "使えません（埋め込みの情報が壊れています: {note}。手動の補正だけを使えます）"
                )
            }
            (None, None) if src.is_raw => "なし（手動の補正だけを使えます）".to_owned(),
            (None, None) => "-（RAW 以外。歪曲の手動の補正だけを使えます）".to_owned(),
        },
    ]);
    let pv = state.settings.process_version;
    t.row(vec![
        "処理バージョン".to_owned(),
        if pv < state.latest_process_version {
            format!(
                "{pv}（最新は {}。genzo develop upgrade {v} で更新できます）",
                state.latest_process_version
            )
        } else {
            pv.to_string()
        },
    ]);
    out.table(&t);
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
    version: PasteVersion,
) -> CliResult<Status> {
    let to = unique_ids(to);
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &to)?;
    let source = s.core.develop_settings(from)?;
    let r = s.core.paste_settings_with(
        &source,
        &to,
        groups,
        version,
        genzo_api::HISTORY_LABEL_PASTE,
    )?;
    s.close()?;
    if r.needs_upgrade > 0 {
        out.warn(format!(
            "{} 件は処理バージョンが古いため、写した項目の一部（レンズ補正・ハイライト復元・ノイズ軽減・カメラプロファイル・色の調整・明瞭度・シャープ・効果・マスクなど）は使われません\
             （--upgrade-process-version を付けてコピーするか、genzo develop upgrade で更新できます）",
            r.needs_upgrade
        ));
    }
    if out.json {
        out.print_json(&json!({
            "from": from,
            "updated": r.count,
            "upgraded": r.upgraded,
            "needs_upgrade": r.needs_upgrade,
        }));
    } else if r.upgraded > 0 {
        out.line(format!(
            "variant {from} の設定を {} 件に適用しました（{} 件の処理バージョンを {} に更新）",
            r.count, r.upgraded, source.process_version
        ));
    } else {
        out.line(format!(
            "variant {from} の設定を {} 件に適用しました",
            r.count
        ));
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
        out.line(format!(
            "仮想コピーを作りました: variant {id}（元: {v}。元の写真と同じスタックに入れました）"
        ));
    }
    Ok(Status::Success)
}

/// `develop region`: 写真を開いて、表示範囲を描き（genzo-api の [`genzo_api::Core::render_region`]。
/// implementation_status の No.52）、PNG に書く。画面のプロファイルは設定しないので sRGB とみなした値
/// （ICC なしの PNG。sRGB として表示される）になる。
fn region(g: &GlobalArgs, out: Output, a: &RegionArgs) -> CliResult<Status> {
    if a.out.exists() {
        return Err(CliError::Usage(format!(
            "--out {} は既にあります（上書きしません）",
            a.out.display()
        )));
    }
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &[a.variant])?;
    s.core.open_develop(a.variant)?;
    let request = RegionRequest {
        variant_id: a.variant,
        space: match a.space {
            RegionSpaceArg::Output => RegionSpace::Output,
            RegionSpaceArg::Geometry => RegionSpace::Geometry,
        },
        x: a.x,
        y: a.y,
        width: a.width,
        height: a.height,
        scale: a.scale,
    };
    let frame = match &a.before {
        Some(source) => s
            .core
            .set_before(Some(source.clone()))
            .and_then(|_| s.core.render_before_region(&request)),
        None => s.core.render_region(&request),
    };
    s.core.close_develop()?;
    s.close()?;
    let frame = frame?;
    let info = &frame.info;
    let image = genzo_media::RgbImage8::from_raw(
        info.width,
        info.height,
        frame.display_rgb8.as_ref().clone(),
    )
    .map_err(|e| CliError::other(e.to_string()))?;
    let png = genzo_media::png_io::encode_png(&image, None, None)
        .map_err(|e| CliError::other(e.to_string()))?;
    write_new_file(&a.out, &png)?;
    if out.json {
        out.print_json(&json!({ "path": a.out, "region": info }));
    } else {
        out.line(format!(
            "表示範囲を書き出しました: {}（範囲 x={} y={} {}×{}、倍率 {}、{}×{}、画像全体 {}×{}）",
            a.out.display(),
            info.x,
            info.y,
            info.region_width,
            info.region_height,
            info.scale,
            info.width,
            info.height,
            info.full_width,
            info.full_height
        ));
    }
    Ok(Status::Success)
}

/// 比較表示の同期ズームの範囲: 中心は出力の画像の大きさに対する割合（`cx`・`cy`）、大きさは出力の画素で、
/// 画像の内側に収める（画像より大きければ画像の大きさ）。
pub fn compare_region(
    size: &ViewOutputSize,
    (cx, cy): (f64, f64),
    (width, height): (f64, f64),
    scale: f64,
) -> RegionRequest {
    let (fw, fh) = (f64::from(size.width), f64::from(size.height));
    let w = width.min(fw);
    let h = height.min(fh);
    let x = (cx.clamp(0.0, 1.0) * fw - w / 2.0).clamp(0.0, (fw - w).max(0.0));
    let y = (cy.clamp(0.0, 1.0) * fh - h / 2.0).clamp(0.0, (fh - h).max(0.0));
    RegionRequest {
        variant_id: size.variant_id,
        space: RegionSpace::Output,
        x: x.floor(),
        y: y.floor(),
        width: w,
        height: h,
        scale,
    }
}

/// `develop compare`: 比較表示（LIB-03）の面に、写真ごとに同じ範囲を同時に描いて（genzo-api の
/// [`genzo_api::Core::render_view_region`]）、PNG に書く。
fn compare(g: &GlobalArgs, out: Output, a: &CompareArgs) -> CliResult<Status> {
    if a.variants.len() > MAX_VIEW_PANES as usize {
        return Err(CliError::Usage(format!(
            "比較できる写真は {MAX_VIEW_PANES} 枚までです（{} 枚を指定）",
            a.variants.len()
        )));
    }
    if !(a.width > 0.0 && a.height > 0.0) {
        return Err(CliError::Usage(
            "--width と --height は正の値にしてください".to_owned(),
        ));
    }
    let paths: Vec<std::path::PathBuf> = a
        .variants
        .iter()
        .enumerate()
        .map(|(pane, v)| a.out_dir.join(format!("compare-{pane}-{v}.png")))
        .collect();
    if let Some(p) = paths.iter().find(|p| p.exists()) {
        return Err(CliError::Usage(format!(
            "{} は既にあります（上書きしません）",
            p.display()
        )));
    }
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &a.variants)?;
    let core = &s.core;
    // 面ごとに出力の大きさを求め、同じ範囲（割合の中心と画素の大きさ）を同時に描く。
    let rendered = std::thread::scope(|scope| {
        let handles: Vec<_> = a
            .variants
            .iter()
            .enumerate()
            .map(|(pane, v)| {
                let pane = pane as u32;
                let v = *v;
                scope.spawn(move || {
                    let size = core.view_output_size(pane, v)?;
                    let req = compare_region(&size, (a.cx, a.cy), (a.width, a.height), a.scale);
                    core.render_view_region(pane, &req)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("描画のスレッドが異常終了しました"))
            .collect::<Vec<_>>()
    });
    s.core.release_view()?;
    s.close()?;
    let mut frames = Vec::with_capacity(rendered.len());
    for r in rendered {
        frames.push(r?);
    }
    std::fs::create_dir_all(&a.out_dir).map_err(|e| CliError::io(&a.out_dir, e))?;
    let mut written = Vec::new();
    for (frame, path) in frames.iter().zip(&paths) {
        let info = &frame.info;
        let image = genzo_media::RgbImage8::from_raw(
            info.width,
            info.height,
            frame.display_rgb8.as_ref().clone(),
        )
        .map_err(|e| CliError::other(e.to_string()))?;
        let png = genzo_media::png_io::encode_png(&image, None, None)
            .map_err(|e| CliError::other(e.to_string()))?;
        write_new_file(path, &png)?;
        written.push(json!({ "pane": info.pane, "path": path, "region": info }));
    }
    if out.json {
        out.print_json(&json!({ "panes": written }));
    } else {
        for (frame, path) in frames.iter().zip(&paths) {
            let info = &frame.info;
            out.line(format!(
                "面 {}: variant {} の範囲 x={} y={} {}×{}（画像全体 {}×{}、倍率 {}）を {} に書き出しました",
                info.pane.unwrap_or_default(),
                info.variant_id,
                info.x,
                info.y,
                info.region_width,
                info.region_height,
                info.full_width,
                info.full_height,
                info.scale,
                path.display()
            ));
        }
    }
    Ok(Status::Success)
}

/// `develop before`: 写真を開いて、ビフォーのプレビュー（と `--after` ならアフターのプレビュー）を PNG に
/// 書く（DEV-31）。
fn before(g: &GlobalArgs, out: Output, a: &BeforeArgs) -> CliResult<Status> {
    for p in std::iter::once(&a.out).chain(a.after.as_ref()) {
        if p.exists() {
            return Err(CliError::Usage(format!(
                "{} は既にあります（上書きしません）",
                p.display()
            )));
        }
    }
    if a.after.as_ref() == Some(&a.out) {
        return Err(CliError::Usage(
            "--out と --after に同じファイルを指定しています".to_owned(),
        ));
    }
    let s = Session::open(g, out, &OpenOptions::default())?;
    ensure_variants(&s, &[a.variant])?;
    s.core.open_develop(a.variant)?;
    let rendered = (|| -> Result<_, genzo_api::ApiError> {
        let info = s
            .core
            .set_before(Some(a.source.clone()))?
            .ok_or_else(|| genzo_api::ApiError::Internal("ビフォーがない".to_owned()))?;
        s.core.render_before()?;
        let before = s
            .core
            .before_frame()
            .ok_or_else(|| genzo_api::ApiError::Internal("ビフォーが描けていない".to_owned()))?;
        let after = match &a.after {
            Some(_) => {
                s.core.render_develop()?;
                s.core.develop_frame().map(|f| (f.info, f.display_rgb8))
            }
            None => None,
        };
        Ok((info, before, after))
    })();
    s.core.close_develop()?;
    s.close()?;
    let (info, before, after) = rendered?;
    let png = |w: u32, h: u32, rgb: &[u8]| -> CliResult<Vec<u8>> {
        let image = genzo_media::RgbImage8::from_raw(w, h, rgb.to_vec())
            .map_err(|e| CliError::other(e.to_string()))?;
        genzo_media::png_io::encode_png(&image, None, None)
            .map_err(|e| CliError::other(e.to_string()))
    };
    write_new_file(
        &a.out,
        &png(before.info.width, before.info.height, &before.display_rgb8)?,
    )?;
    if let (Some(path), Some((ainfo, rgb))) = (&a.after, &after) {
        write_new_file(path, &png(ainfo.width, ainfo.height, rgb)?)?;
    }
    if out.json {
        out.print_json(&json!({
            "path": a.out,
            "before": info,
            "frame": before.info,
            "after": after.as_ref().map(|(i, _)| json!({ "path": a.after, "frame": i })),
        }));
    } else {
        out.line(format!(
            "ビフォー（{}）を書き出しました: {}（{}×{}）",
            info.label,
            a.out.display(),
            before.info.width,
            before.info.height
        ));
        if let (Some(path), Some((ainfo, _))) = (&a.after, &after) {
            out.line(format!(
                "アフターを書き出しました: {}（{}×{}）",
                path.display(),
                ainfo.width,
                ainfo.height
            ));
        }
    }
    Ok(Status::Success)
}

/// 新しいファイルに書く（既にあれば書かない。途中で失敗したら書きかけのファイルを消す）。
pub(crate) fn write_new_file(path: &std::path::Path, bytes: &[u8]) -> CliResult<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| CliError::io(path, e))?;
    if let Err(e) = f.write_all(bytes).and_then(|()| f.sync_all()) {
        drop(f);
        let _ = std::fs::remove_file(path);
        return Err(CliError::io(path, e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// レンズ補正・ハイライト復元とノイズ軽減のオプションだけを当てる（色のオプションなし）。
    fn options_applied(
        current: &DevelopSettings,
        patch: &Value,
        replace: bool,
        lens: &LensArgs,
        sensor: &SensorArgs,
    ) -> CliResult<DevelopSettings> {
        settings_with_options(
            current,
            patch,
            replace,
            &OptionArgs {
                lens,
                sensor,
                color: &ColorArgs::default(),
                detail: &DetailArgs::default(),
                profile: &ProfileArgs::default(),
            },
        )
    }
    use genzo_model::WhiteBalance;

    #[test]
    fn compare_regions_share_the_relative_center_and_pixel_size() {
        let size = |w, h| ViewOutputSize {
            variant_id: VariantId::new(1),
            width: w,
            height: h,
        };
        // 中央の 100 × 50。
        let r = compare_region(&size(400, 300), (0.5, 0.5), (100.0, 50.0), 1.0);
        assert_eq!((r.x, r.y, r.width, r.height), (150.0, 125.0, 100.0, 50.0));
        // 別の大きさの写真でも中心は同じ割合、大きさは同じ画素。
        let r = compare_region(&size(800, 600), (0.5, 0.5), (100.0, 50.0), 1.0);
        assert_eq!((r.x, r.y, r.width, r.height), (350.0, 275.0, 100.0, 50.0));
        // 端は画像の内側に収め、画像より大きい範囲は画像の大きさにする。
        let r = compare_region(&size(400, 300), (1.0, 0.0), (100.0, 50.0), 2.0);
        assert_eq!((r.x, r.y, r.scale), (300.0, 0.0, 2.0));
        let r = compare_region(&size(80, 60), (0.3, 0.9), (100.0, 100.0), 1.0);
        assert_eq!((r.x, r.y, r.width, r.height), (0.0, 0.0, 80.0, 60.0));
    }

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

    /// 処理バージョンは、JSON に書いたときだけ変わる（DATA-09。検証の指摘: `--replace` の土台が既定の設定
    /// （最新の版）だったため、版 1 の写真が黙って版 2 になっていた）。
    #[test]
    fn replace_keeps_the_current_process_version_unless_written() {
        let mut current = DevelopSettings {
            process_version: 1,
            exposure_ev: 0.3,
            ..Default::default()
        };
        current.render_deps.raw_decoder = "libraw-0.21.2".to_owned();
        for patch in [json!({}), json!({"exposure_ev": 0.5})] {
            for replace in [false, true] {
                let s = merged_settings(&current, &patch, replace).unwrap();
                assert_eq!(s.process_version, 1, "{patch} replace={replace}");
                assert_eq!(s.render_deps, current.render_deps);
            }
        }
        // オプションだけ（JSON なし）でも同じ。
        let lens = LensArgs {
            lens_profile: Some(crate::args::SwitchArg::On),
            ..LensArgs::default()
        };
        let s = options_applied(&current, &json!({}), true, &lens, &SensorArgs::default()).unwrap();
        assert_eq!(s.process_version, 1);
        assert!(s.lens.profile_enabled);
        // JSON に書けば、その版にする（明示的な指定）。
        let s = merged_settings(&current, &json!({"process_version": 2}), true).unwrap();
        assert_eq!(s.process_version, 2);
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
    fn lens_options_are_applied_after_the_json_and_validated() {
        let current = DevelopSettings::default();
        let lens = LensArgs {
            lens_profile: Some(crate::args::SwitchArg::On),
            distortion: Some(-20.0),
            ..Default::default()
        };
        let patch = json!({"lens": {"distortion": 10, "ca_red_cyan": 4}, "exposure_ev": 0.5});
        let s = options_applied(&current, &patch, false, &lens, &SensorArgs::default()).unwrap();
        assert!(s.lens.profile_enabled);
        assert_eq!(s.lens.distortion, -20.0, "オプションが JSON より後");
        assert_eq!(s.lens.ca_red_cyan, 4.0);
        assert_eq!(s.exposure_ev, 0.5);
        // 範囲の外は検証で拒む。
        let bad = LensArgs {
            vignetting_scale: Some(250.0),
            ..Default::default()
        };
        assert!(
            options_applied(&current, &json!({}), false, &bad, &SensorArgs::default()).is_err()
        );
        // 処理バージョン 1 では、レンズ補正を設定していれば「使われない」。
        assert!(!lens_ignored(&s));
        let v1 = DevelopSettings {
            process_version: 1,
            ..s.clone()
        };
        assert!(lens_ignored(&v1));
        assert!(!lens_ignored(&DevelopSettings {
            process_version: 1,
            ..DevelopSettings::default()
        }));
    }

    #[test]
    fn sensor_options_are_applied_after_the_json_and_validated() {
        let current = DevelopSettings::default();
        let sensor = SensorArgs {
            highlight_recovery: Some(crate::args::HighlightArg::Reconstruct),
            highlight_amount: Some(60.0),
            nr_luminance: Some(30.0),
            nr_color_detail: Some(80.0),
            ..Default::default()
        };
        let patch = json!({"detail": {"noise_reduction": {"luminance": 10, "color": 25}}});
        let s = options_applied(&current, &patch, false, &LensArgs::default(), &sensor).unwrap();
        assert_eq!(
            s.highlight_recovery.method,
            genzo_model::HighlightMethod::Reconstruct
        );
        assert_eq!(s.highlight_recovery.amount, 60.0);
        assert_eq!(
            s.detail.noise_reduction.luminance, 30.0,
            "オプションが JSON より後"
        );
        assert_eq!(s.detail.noise_reduction.color, 25.0);
        assert_eq!(s.detail.noise_reduction.color_detail, 80.0);
        assert_eq!(s.detail.noise_reduction.luminance_detail, 50.0);
        // 範囲の外は検証で拒む。
        let bad = SensorArgs {
            nr_color: Some(120.0),
            ..Default::default()
        };
        assert!(options_applied(&current, &json!({}), false, &LensArgs::default(), &bad).is_err());
        // 処理バージョン 1 では使われない。
        assert!(!sensor_ignored(&s));
        assert!(sensor_ignored(&DevelopSettings {
            process_version: 1,
            ..s.clone()
        }));
        assert!(!sensor_ignored(&DevelopSettings {
            process_version: 1,
            ..DevelopSettings::default()
        }));
    }

    #[test]
    fn color_options_are_applied_after_the_json_and_validated() {
        use genzo_model::{GradingWheel, HueBand};
        let current = DevelopSettings::default();
        let color = ColorArgs {
            black_white: Some(crate::args::SwitchArg::On),
            hsl_luminance: vec![(HueBand::Blue, -40.0), (HueBand::Blue, -30.0)],
            hsl_hue: vec![(HueBand::Red, 15.0)],
            bw_mix: vec![(HueBand::Orange, 25.0)],
            grading_shadows: Some(GradingWheel {
                hue: 220.0,
                saturation: 30.0,
                luminance: -10.0,
            }),
            grading_balance: Some(-20.0),
            ..Default::default()
        };
        let patch = json!({"color": {"mixer": {"blue": {"luminance": 10, "hue": 5}}, "grading": {"blending": 70}}});
        let s = settings_with_options(
            &current,
            &patch,
            false,
            &OptionArgs {
                lens: &LensArgs::default(),
                sensor: &SensorArgs::default(),
                color: &color,
                detail: &DetailArgs::default(),
                profile: &ProfileArgs::default(),
            },
        )
        .unwrap();
        assert!(s.color.black_white.enabled);
        assert_eq!(
            s.color.mixer.blue.luminance, -30.0,
            "オプションが JSON より後。同じ帯は後の値"
        );
        assert_eq!(s.color.mixer.blue.hue, 5.0);
        assert_eq!(s.color.mixer.red.hue, 15.0);
        assert_eq!(s.color.black_white.mix.orange, 25.0);
        assert_eq!(s.color.grading.shadows.hue, 220.0);
        assert_eq!(s.color.grading.blending, 70.0);
        assert_eq!(s.color.grading.balance, -20.0);
        // 範囲の外は検証で拒む。
        let bad = ColorArgs {
            grading_global: Some(GradingWheel {
                hue: 400.0,
                saturation: 10.0,
                luminance: 0.0,
            }),
            ..Default::default()
        };
        assert!(
            settings_with_options(
                &current,
                &json!({}),
                false,
                &OptionArgs {
                    lens: &LensArgs::default(),
                    sensor: &SensorArgs::default(),
                    color: &bad,
                    detail: &DetailArgs::default(),
                    profile: &ProfileArgs::default(),
                },
            )
            .is_err()
        );
        // 処理バージョン 1 では使われない（白黒ミキサーだけ・強さ 0 の色相は何もしないので知らせない）。
        assert!(!color_ignored(&s));
        assert!(color_ignored(&DevelopSettings {
            process_version: 1,
            ..s.clone()
        }));
        let mut quiet = DevelopSettings {
            process_version: 1,
            ..DevelopSettings::default()
        };
        quiet.color.black_white.mix.red = 30.0;
        quiet.color.grading.midtones.hue = 120.0;
        assert!(!color_ignored(&quiet));
    }

    #[test]
    fn detail_options_are_applied_after_the_json_and_validated() {
        let current = DevelopSettings::default();
        let detail = DetailArgs {
            clarity: Some(35.0),
            dehaze: Some(-20.0),
            sharpen_amount: Some(60.0),
            sharpen_masking: Some(40.0),
            vignette_amount: Some(-30.0),
            grain_amount: Some(25.0),
            grain_size: Some(70.0),
            ..Default::default()
        };
        assert!(detail.any() && !DetailArgs::default().any());
        let patch = json!({"presence": {"texture": 15, "clarity": 10}, "effects": {"vignette": {"feather": 80}}});
        let s = settings_with_options(
            &current,
            &patch,
            false,
            &OptionArgs {
                lens: &LensArgs::default(),
                sensor: &SensorArgs::default(),
                color: &ColorArgs::default(),
                detail: &detail,
                profile: &ProfileArgs::default(),
            },
        )
        .unwrap();
        assert_eq!(s.presence.texture, 15.0);
        assert_eq!(s.presence.clarity, 35.0, "オプションが JSON より後");
        assert_eq!(s.presence.dehaze, -20.0);
        assert_eq!(s.detail.sharpening.amount, 60.0);
        assert_eq!(s.detail.sharpening.masking, 40.0);
        assert_eq!(s.detail.sharpening.radius, 1.0);
        assert_eq!(s.effects.vignette.amount, -30.0);
        assert_eq!(s.effects.vignette.feather, 80.0);
        assert_eq!(s.effects.grain.amount, 25.0);
        assert_eq!(s.effects.grain.size, 70.0);
        // 範囲の外は検証で拒む。
        let bad = DetailArgs {
            sharpen_radius: Some(5.0),
            ..Default::default()
        };
        assert!(
            settings_with_options(
                &current,
                &json!({}),
                false,
                &OptionArgs {
                    lens: &LensArgs::default(),
                    sensor: &SensorArgs::default(),
                    color: &ColorArgs::default(),
                    detail: &bad,
                    profile: &ProfileArgs::default(),
                },
            )
            .is_err()
        );
        // 処理バージョン 1 では使われない（量が 0 なら知らせない）。
        assert!(!detail_ignored(&s));
        assert!(detail_ignored(&DevelopSettings {
            process_version: 1,
            ..s.clone()
        }));
        let mut quiet = DevelopSettings {
            process_version: 1,
            ..DevelopSettings::default()
        };
        quiet.effects.grain.size = 80.0;
        quiet.detail.sharpening.radius = 2.0;
        assert!(!detail_ignored(&quiet));
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
