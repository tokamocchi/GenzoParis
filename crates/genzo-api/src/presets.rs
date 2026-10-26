//! プリセット: 現像プリセット（DEV-29）と書き出しプリセット（EXP-02）。04 の 2.5 節・6.4 節。
//!
//! # 保存の場所（[`default_presets_dir`]）
//!
//! カタログをまたいで使うので、カタログ・カタログのデータのフォルダではなく、**アプリのデータのフォルダ**の
//! `presets` に置く（アプリと CLI で同じ場所。環境変数 [`ENV_PRESETS_DIR`]（`GENZO_PRESETS_DIR`）があれば
//! そのフォルダ）:
//!
//! | OS | 場所 |
//! |---|---|
//! | Windows | `%APPDATA%\io.github.tokamocchi.genzoparis\presets`（Tauri の `app_data_dir` と同じ。ローミング） |
//! | macOS | `~/Library/Application Support/io.github.tokamocchi.genzoparis/presets` |
//! | Linux | `$XDG_DATA_HOME/io.github.tokamocchi.genzoparis/presets`（なければ `~/.local/share/...`） |
//!
//! 中は種類ごとのフォルダ（`develop`・`export`）に、**1 件 1 ファイル**（`<ID>.json`）で置く。1 つの一覧の
//! ファイルにしないのは、アプリと CLI が同時に別のプリセットを変えても互いの変更を失わず、1 つのファイルが壊れても
//! ほかのプリセットに響かないようにするため。フォルダ・ファイルは初めて保存するときに作る（読むだけなら作らない）。
//!
//! # ファイルの形（版付きの JSON。[`PRESET_FORMAT`]・[`PRESET_FORMAT_VERSION`]）
//!
//! ```json
//! {"format": "genzoparis-preset", "format_version": 1, "kind": "develop", "id": "0123456789abcdef",
//!  "name": "フィルム風", "folder": "ルック", "created_at": "2026-10-14T01:02:03.456Z",
//!  "updated_at": "2026-10-14T01:02:03.456Z", "app_version": "0.1.0",
//!  "groups": {"white_balance": false, "basic_tone": true, ...}, "settings": {現像設定の JSON}}
//! ```
//!
//! - 現像プリセット（`kind` が `develop`）: `groups`（含める項目。設定の貼り付けと同じ [`SettingGroups`]）と
//!   `settings`（現像設定。カタログと同じ JSON。含めない項目は既定値、外部データ `render_deps` は空にする）。
//! - 書き出しプリセット（`kind` が `export`）: `settings`（[`ExportPresetSettings`]。形式・色空間・大きさ・品質・
//!   メタデータ・同じ名前の扱い・解像度・出力シャープ・透かし・動画（genzo-model の `ExportSettings`）、書き出し先、
//!   ファイル名の規則）。項目を足すときは `#[serde(default)]` で足す（古いファイルは既定値で読める）。
//! - **別の OS のパス**（書き出しプリセットの書き出し先・透かしのフォントと画像）: この OS の絶対パスでないもの（Linux・
//!   macOS で読んだ `C:\…`、Windows で読んだ `/Users/…` など）は、読むときに使わない（書き出し先はなし、透かしの
//!   ファイルは空にして書き出す前に選び直す）。プリセット全体は使えるままにし、外したものを
//!   [`PresetInfo::warnings`] で知らせる（[`ExportPresetSettings::without_foreign_paths`]。ファイルの中のパスは
//!   名前の変更・移動で消さない。決定事項ログ No.170）。
//! - **知らない項目は消さない**: 名前の変更・フォルダの移動では、ファイルの中の知らない項目（新しい版のアプリが
//!   足した項目など）をそのまま残す（`StoredPreset::body`）。
//! - **形式の版が新しいファイル**（`format_version` が [`PRESET_FORMAT_VERSION`] より大きい）・壊れたファイルは、
//!   一覧に「使えない理由」（[`PresetInfo::problem`]）を付けて出し、適用・更新・名前の変更はしない（削除はできる）。
//!
//! # 現像プリセットと処理バージョン（DATA-09。決定事項ログ No.161）
//!
//! - 作ったときの写真の処理バージョンを、設定の `process_version` に残す（[`PresetInfo::process_version`]）。
//! - **適用は設定の貼り付けと同じ**（`develop` の `paste_settings_with`）: 写真の処理バージョンは変えない
//!   （[`PasteVersion::Keep`]。利用者が選べば [`PasteVersion::Upgrade`] で上げる）。版 1 と 2 の写真の間で使える
//!   （版 1 の写真に版 2 にしかない項目を当てると、値は保存するが処理バージョンを更新するまで使われない。結果の
//!   `needs_upgrade` で知らせる）。
//! - 古いスキーマの版で作ったプリセットは、読むときにカタログの設定と同じマイグレーションで今の形にする（ファイルは
//!   書き換えない）。このアプリが知らない処理バージョン・スキーマの版のプリセット（新しい版のアプリで作った）は
//!   使えない（[`PresetInfo::problem`]）。
//!
//! # 名前・フォルダ
//!
//! 名前は前後の空白を除いて NFC にし、1〜[`MAX_PRESET_NAME_CHARS`] 文字で、制御文字は使えない。フォルダは 1 階層の
//! 名前（空なら「フォルダなし」）で、同じ規則（空でもよい）。**同じ種類・同じフォルダに同じ名前は 1 つ**（NFKC ＋
//! 小文字化で比べる。[`ApiError::NameConflict`]）。フォルダはプリセットの属性で、空のフォルダはない。
//!
//! # 書き込みの安全性（6.4 節）
//!
//! 書き込みは、同じフォルダの一時ファイルに書いて同期してから名前を変えて公開する（genzo-media の
//! `write_atomically`）。新しいプリセットは既存のファイルを上書きしない（ID が重なれば作り直す）。ファイルへの
//! 書き出し（[`PresetStore::export_files`]）も上書きしない（同じ名前があれば `-1` などを付ける）。読み込み
//! （[`PresetStore::import_files`]）は元のファイルを変えず、新しい ID で保存する（同じフォルダに同じ名前があれば
//! 「 (2)」などを付ける）。ネットワークには何も送らない。

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, SecondsFormat, Utc};
use genzo_media::{MediaError, ProtectedFiles, WriteOutcome, write_atomically};
use genzo_model::{
    ConflictPolicy, DevelopSettings, ExportFormat, ExportSettings, RenderDeps, VariantId,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::core::Core;
use crate::error::ApiError;
use crate::logging::APP_IDENTIFIER;
use crate::naming::RenameRule;
use crate::types::{NamePreview, PasteReport, PasteVersion, SettingGroups};

/// アプリのデータのフォルダの下の、プリセットのフォルダの名前。
pub const PRESETS_DIR: &str = "presets";

/// プリセットのフォルダを指定する環境変数（アプリ・CLI。テスト・持ち運び用）。
pub const ENV_PRESETS_DIR: &str = "GENZO_PRESETS_DIR";

/// プリセットのファイルの `format` の値。
pub const PRESET_FORMAT: &str = "genzoparis-preset";

/// プリセットのファイルの形の版（`format_version`）。項目の意味を変えたときに上げる（項目を足すだけなら上げない）。
pub const PRESET_FORMAT_VERSION: u32 = 1;

/// 名前・フォルダの名前の文字数の上限（Unicode のスカラー値の数。**仮置き**: 100）。
pub const MAX_PRESET_NAME_CHARS: usize = 100;

/// 1 つのプリセットのファイルの大きさの上限（バイト。**仮置き**: 32 MiB。ブラシのマスクを含む現像設定を考えて
/// 大きめにする。壊れた・関係のない大きなファイルを読み込まないため）。
pub const MAX_PRESET_FILE_BYTES: u64 = 32 << 20;

/// 1 つの種類のプリセットの数の上限（**仮置き**: 10,000。一覧は毎回すべてのファイルを読む）。
pub const MAX_PRESETS: usize = 10_000;

/// 1 回の読み込み（[`PresetStore::import_files`]）・書き出し（[`PresetStore::export_files`]）のファイルの数の上限
/// （**仮置き**: 1,000）。
pub const MAX_PRESET_FILES_PER_CALL: usize = 1_000;

/// プリセットの ID の 16 進数の桁数。
const ID_HEX_DIGITS: usize = 16;

/// 名前が重なったときに付ける番号の上限（「 (2)」〜「 (999)」）。
const MAX_NAME_SUFFIX: u32 = 999;

/// プリセットの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetKind {
    /// 現像プリセット（DEV-29）。
    Develop,
    /// 書き出しプリセット（EXP-02）。
    Export,
}

impl PresetKind {
    /// 名前（`develop`・`export`。JSON・CLI の値・種類ごとのフォルダの名前）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Develop => "develop",
            Self::Export => "export",
        }
    }

    /// 表示の名前。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Develop => "現像プリセット",
            Self::Export => "書き出しプリセット",
        }
    }
}

/// 書き出しプリセットの中身（EXP-02。出力シャープ・透かし・解像度・動画（EXP-03・EXP-05）は `export` の中）。項目は
/// `#[serde(default)]` で足す（古いファイルは既定値で読める）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportPresetSettings {
    /// 形式・色空間・大きさ・品質・メタデータ（GPS・キーワード）・同じ名前があるときの扱い。
    pub export: ExportSettings,
    /// 書き出し先のフォルダ（絶対パス。`None` なら書き出すときに選ぶ）。
    pub destination: Option<PathBuf>,
    /// ファイル名の規則（`None` なら元のファイル名。仮想コピーは名前を付ける）。
    pub file_naming: Option<RenameRule>,
}

impl ExportPresetSettings {
    /// 値を確かめる（形式・大きさ・ファイル名の規則・書き出し先・透かしのファイル。保存するときに使う。読んだ
    /// プリセットは、この OS で使えないパスを外してから（[`ExportPresetSettings::without_foreign_paths`]）、透かしの
    /// ファイルのほかの値を確かめる）。
    pub fn validate(&self) -> Result<(), ApiError> {
        self.export
            .validate()
            .map_err(|e| ApiError::InvalidArgument(e.to_string()))?;
        if let Some(rule) = &self.file_naming {
            rule.parse()?;
        }
        if let Some(d) = &self.destination {
            if !d.is_absolute() {
                return Err(ApiError::InvalidArgument(format!(
                    "書き出し先のフォルダは絶対パスで指定してください（{}）",
                    d.display()
                )));
            }
            if d.to_str().is_none() {
                return Err(ApiError::InvalidArgument(format!(
                    "書き出し先のフォルダの名前を保存できません（文字として読めない名前。{}）",
                    d.display()
                )));
            }
        }
        Ok(())
    }

    /// 読んだプリセットから、この OS で使えないパス（この OS の絶対パスでないもの。別の OS で作ったプリセットなど）を
    /// 外す（書き出し先はなし、透かしのファイルは空にする）。外したものの説明を返す（モジュールの doc の「別の OS の
    /// パス」）。
    pub fn without_foreign_paths(&mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(d) = &self.destination
            && !d.is_absolute()
        {
            warnings.push(format!(
                "書き出し先のフォルダ（{}）は、この PC のパスではないため使いません（書き出すときに選んでください）",
                d.display()
            ));
            self.destination = None;
        }
        if let Some(w) = &mut self.export.watermark {
            let what = match &w.content {
                genzo_model::WatermarkContent::Text { .. } => "透かしのフォントのファイル",
                genzo_model::WatermarkContent::Image { .. } => "透かしの画像のファイル",
            };
            let file = w.content.file_mut();
            if !file.as_os_str().is_empty() && !file.is_absolute() {
                warnings.push(format!(
                    "{what}（{}）は、この PC のパスではないため使いません（書き出す前に選び直してください）",
                    file.display()
                ));
                *file = PathBuf::new();
            }
        }
        warnings
    }
}

/// プリセットの一覧の 1 件（[`PresetStore::list`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetInfo {
    /// 種類。
    pub kind: PresetKind,
    /// ID（16 桁の 16 進数。ファイルの名前）。
    pub id: String,
    /// 名前（読めないファイルは「（読めないプリセット）」）。
    pub name: String,
    /// フォルダ（空ならフォルダなし）。
    pub folder: String,
    /// 作った日時（分からなければ `None`）。
    pub created_at: Option<DateTime<Utc>>,
    /// 最後に変えた日時（分からなければ `None`）。
    pub updated_at: Option<DateTime<Utc>>,
    /// ファイルの形の版（読めないファイルは 0）。
    pub format_version: u32,
    /// 使えない理由（新しい版のアプリで作った・壊れているなど。使えるなら `None`）。使えないものは適用・更新・名前の
    /// 変更をしない（削除はできる）。
    pub problem: Option<String>,
    /// 現像プリセット: 含める項目。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<SettingGroups>,
    /// 現像プリセット: 作ったときの写真の処理バージョン。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_version: Option<u32>,
    /// 書き出しプリセット: 中身（この OS で使えないパスは外したもの。[`ExportPresetSettings::without_foreign_paths`]）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<ExportPresetSettings>,
    /// 使えるが、一部の値を使わないことの知らせ（別の OS のパスなど）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl PresetInfo {
    /// 使えるか。
    pub fn usable(&self) -> bool {
        self.problem.is_none()
    }
}

/// プリセットのファイルの読み込みの結果（[`PresetStore::import_files`]）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetImportReport {
    /// 読み込んだもの（保存した後の情報。名前が重なったものは「 (2)」などを付けた名前）。
    pub imported: Vec<PresetInfo>,
    /// 読み込めなかったファイル。
    pub failed: Vec<PresetFileFailure>,
}

/// 読み込めなかったファイル。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetFileFailure {
    /// ファイル。
    #[serde(serialize_with = "crate::types::lossy_path")]
    pub path: PathBuf,
    /// 理由。
    pub message: String,
}

/// プリセットのファイル（`<ID>.json`）の中身。知らない項目は `body` に入れて、書き直すときにそのまま残す。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredPreset {
    format: String,
    format_version: u32,
    kind: PresetKind,
    #[serde(default)]
    id: String,
    name: String,
    #[serde(default)]
    folder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
    /// 種類ごとの中身（`groups`・`settings`）と、知らない項目。
    #[serde(flatten)]
    body: Map<String, Value>,
}

/// 名前（または空でないことを求めないフォルダ）を整える: 前後の空白を除いて NFC にし、長さと制御文字を確かめる。
fn checked_name(name: &str, what: &str, allow_empty: bool) -> Result<String, ApiError> {
    let n = genzo_catalog::text::nfc(name.trim());
    if n.is_empty() && !allow_empty {
        return Err(ApiError::InvalidArgument(format!("{what}が空です")));
    }
    if n.chars().count() > MAX_PRESET_NAME_CHARS {
        return Err(ApiError::InvalidArgument(format!(
            "{what}は {MAX_PRESET_NAME_CHARS} 文字までです"
        )));
    }
    if n.chars().any(char::is_control) {
        return Err(ApiError::InvalidArgument(format!(
            "{what}に改行・タブなどの制御文字は使えません"
        )));
    }
    Ok(n)
}

/// 名前の比較キー（NFKC ＋ 小文字化。全角・半角と大文字・小文字の違いを区別しない）。
fn name_key(s: &str) -> String {
    genzo_catalog::text::search_key(s)
}

/// プリセットの ID として正しいか（16 桁の小文字の 16 進数。ファイルの名前にするので、パスの区切りなどを含む
/// 値を受け付けない）。
pub fn is_valid_preset_id(id: &str) -> bool {
    id.len() == ID_HEX_DIGITS
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn check_id(id: &str) -> Result<(), ApiError> {
    if is_valid_preset_id(id) {
        Ok(())
    } else {
        Err(ApiError::InvalidArgument(format!(
            "プリセットの ID は 16 桁の 16 進数です（{id}）"
        )))
    }
}

/// 新しい ID（時刻・プロセス・通しの番号から作る。重なりは保存するときに確かめる）。
fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = blake3::Hasher::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    h.update(&now.as_nanos().to_le_bytes());
    h.update(&std::process::id().to_le_bytes());
    h.update(&COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    let local = 0u8;
    h.update(&(std::ptr::addr_of!(local) as usize).to_le_bytes());
    let hex = h.finalize().to_hex();
    hex[..ID_HEX_DIGITS].to_owned()
}

fn now_string() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_time(s: Option<&str>) -> Option<DateTime<Utc>> {
    s.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
}

/// 既定のプリセットのフォルダ（モジュールの doc。環境変数 [`ENV_PRESETS_DIR`] を優先する）。分からなければ `None`。
pub fn default_presets_dir() -> Option<PathBuf> {
    presets_dir_from(|k| std::env::var_os(k))
}

/// [`default_presets_dir`] の本体（環境変数を引数で受ける。単体テストのため）。
fn presets_dir_from(get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(dir) = get(ENV_PRESETS_DIR).filter(|v| !v.is_empty()) {
        let dir = PathBuf::from(dir);
        return Some(std::path::absolute(&dir).unwrap_or(dir));
    }
    platform_data_dir(&get).map(|d| d.join(APP_IDENTIFIER).join(PRESETS_DIR))
}

/// OS のアプリのデータのフォルダ（Tauri の `data_dir` と同じ場所。識別子の前まで）。
#[cfg(windows)]
fn platform_data_dir(get: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let base = PathBuf::from(get("APPDATA").filter(|v| !v.is_empty())?);
    base.is_absolute().then_some(base)
}

#[cfg(target_os = "macos")]
fn platform_data_dir(get: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let home = PathBuf::from(get("HOME").filter(|v| !v.is_empty())?);
    home.is_absolute()
        .then(|| home.join("Library").join("Application Support"))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_data_dir(get: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    // XDG Base Directory の決まりどおり、相対パスの XDG_DATA_HOME は使わない。
    get("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            let home = PathBuf::from(get("HOME").filter(|v| !v.is_empty())?);
            home.is_absolute()
                .then(|| home.join(".local").join("share"))
        })
}

/// 現像プリセットに保存する設定: `current`（写真の今の設定）の、`groups` で選んだ項目だけを持ち、ほかの項目は
/// 既定値にしたもの（処理バージョンは `current` のまま。外部データ `render_deps` は空にする: 写真ごとのため）。
pub fn develop_preset_settings(
    groups: SettingGroups,
    current: &DevelopSettings,
) -> DevelopSettings {
    let base = DevelopSettings {
        process_version: current.process_version,
        ..DevelopSettings::default()
    };
    let mut s = groups.merge(current, &base);
    s.render_deps = RenderDeps::default();
    s
}

/// 1 つも項目を選んでいなければエラー。
fn check_groups(groups: &SettingGroups) -> Result<(), ApiError> {
    let none = SettingGroups {
        white_balance: false,
        basic_tone: false,
        presence: false,
        tone_curve: false,
        color: false,
        geometry: false,
        lens: false,
        transform: false,
        detail: false,
        effects: false,
        profile: false,
        masks: false,
        retouch: false,
    };
    if *groups == none {
        return Err(ApiError::InvalidArgument(
            "プリセットに含める項目を 1 つ以上選んでください".to_owned(),
        ));
    }
    Ok(())
}

/// 現像プリセットの中身（`groups`・`settings`）。
fn develop_body(
    groups: SettingGroups,
    settings: &DevelopSettings,
) -> Result<Map<String, Value>, ApiError> {
    check_groups(&groups)?;
    let json = settings.to_json()?;
    let settings: Value = serde_json::from_str(&json)
        .map_err(|e| ApiError::Internal(format!("現像設定を JSON にできません: {e}")))?;
    let mut body = Map::new();
    body.insert(
        "groups".to_owned(),
        serde_json::to_value(groups)
            .map_err(|e| ApiError::Internal(format!("項目を JSON にできません: {e}")))?,
    );
    body.insert("settings".to_owned(), settings);
    Ok(body)
}

/// 書き出しプリセットの中身（`settings`）。
fn export_body(settings: &ExportPresetSettings) -> Result<Map<String, Value>, ApiError> {
    settings.validate()?;
    let mut body = Map::new();
    body.insert(
        "settings".to_owned(),
        serde_json::to_value(settings)
            .map_err(|e| ApiError::Internal(format!("書き出しの設定を JSON にできません: {e}")))?,
    );
    Ok(body)
}

/// 現像プリセットの中身を読む（含める項目と設定。読めなければ理由）。
fn read_develop(body: &Map<String, Value>) -> Result<(SettingGroups, DevelopSettings), String> {
    let groups: SettingGroups = match body.get("groups") {
        Some(v) => {
            serde_json::from_value(v.clone()).map_err(|e| format!("含める項目を読めません: {e}"))?
        }
        None => return Err("含める項目がありません".to_owned()),
    };
    let settings = match body.get("settings") {
        Some(v) => DevelopSettings::from_json(&v.to_string()).map_err(|e| e.to_string())?,
        None => return Err("現像設定がありません".to_owned()),
    };
    Ok((groups, settings))
}

/// 書き出しプリセットの中身を読む（読めない・値が正しくなければ理由）。この OS で使えないパスは外し、その説明を返す
/// （[`ExportPresetSettings::without_foreign_paths`]。外した透かしのファイルは書き出す前に選ぶので、ここでは
/// ファイルのほかの値だけを確かめる）。
fn read_export(body: &Map<String, Value>) -> Result<(ExportPresetSettings, Vec<String>), String> {
    let mut settings: ExportPresetSettings = match body.get("settings") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| format!("書き出しの設定を読めません: {e}"))?,
        None => return Err("書き出しの設定がありません".to_owned()),
    };
    let warnings = settings.without_foreign_paths();
    settings
        .export
        .validate_values()
        .map_err(|e| ApiError::InvalidArgument(e.to_string()).to_string())?;
    if let Some(rule) = &settings.file_naming {
        rule.parse().map_err(|e| e.to_string())?;
    }
    Ok((settings, warnings))
}

/// 読んだファイルから一覧の 1 件を作る（使えない理由を調べる）。
fn info_of(kind: PresetKind, id: &str, s: &StoredPreset) -> PresetInfo {
    let mut info = PresetInfo {
        kind,
        id: id.to_owned(),
        name: s.name.clone(),
        folder: s.folder.clone(),
        created_at: parse_time(s.created_at.as_deref()),
        updated_at: parse_time(s.updated_at.as_deref()),
        format_version: s.format_version,
        problem: None,
        groups: None,
        process_version: None,
        export: None,
        warnings: Vec::new(),
    };
    if s.format != PRESET_FORMAT {
        info.problem = Some("GenzoParis のプリセットのファイルではありません".to_owned());
    } else if s.format_version > PRESET_FORMAT_VERSION {
        info.problem = Some(format!(
            "新しい版のアプリで作ったプリセットです（形式の版 {}。このアプリは {PRESET_FORMAT_VERSION} まで）。アプリを更新すると使えます",
            s.format_version
        ));
    } else if s.format_version == 0 {
        info.problem = Some("形式の版が正しくありません".to_owned());
    } else if s.kind != kind {
        info.problem = Some(format!("{}のファイルです", s.kind.label()));
    } else {
        match kind {
            PresetKind::Develop => match read_develop(&s.body) {
                Ok((groups, settings)) => {
                    info.groups = Some(groups);
                    info.process_version = Some(settings.process_version);
                }
                Err(m) => info.problem = Some(m),
            },
            PresetKind::Export => match read_export(&s.body) {
                Ok((e, warnings)) => {
                    info.export = Some(e);
                    info.warnings = warnings;
                }
                Err(m) => info.problem = Some(m),
            },
        }
    }
    info
}

/// 読めないファイルの一覧の 1 件。
fn broken_info(kind: PresetKind, id: &str, message: String) -> PresetInfo {
    PresetInfo {
        kind,
        id: id.to_owned(),
        name: "（読めないプリセット）".to_owned(),
        folder: String::new(),
        created_at: None,
        updated_at: None,
        format_version: 0,
        problem: Some(message),
        groups: None,
        process_version: None,
        export: None,
        warnings: Vec::new(),
    }
}

/// ファイルを読む（大きさの上限を確かめる）。
fn read_limited(path: &Path) -> Result<Vec<u8>, ApiError> {
    let meta = std::fs::metadata(path).map_err(|e| ApiError::io(path, e))?;
    if !meta.is_file() {
        return Err(ApiError::InvalidArgument(format!(
            "ファイルではありません（{}）",
            path.display()
        )));
    }
    if meta.len() > MAX_PRESET_FILE_BYTES {
        return Err(ApiError::InvalidArgument(format!(
            "プリセットのファイルが大きすぎます（{} バイト。上限 {MAX_PRESET_FILE_BYTES} バイト）",
            meta.len()
        )));
    }
    std::fs::read(path).map_err(|e| ApiError::io(path, e))
}

fn parse_stored(bytes: &[u8]) -> Result<StoredPreset, String> {
    serde_json::from_slice::<StoredPreset>(bytes)
        .map_err(|e| format!("プリセットのファイルとして読めません: {e}"))
}

/// プリセットの保存場所（アプリのデータのフォルダの `presets`。モジュールの doc）。
#[derive(Debug)]
pub struct PresetStore {
    root: Option<PathBuf>,
    /// このプロセスの中で、読んでから書くまでの操作を 1 つずつにする（名前の重なりの確認と保存の間に、別の操作が
    /// 入らないように）。
    ops: Mutex<()>,
}

impl PresetStore {
    /// `root`（`None` なら場所が分からない: 操作はエラー）のプリセット。フォルダは保存するときに作る。
    pub fn new(root: Option<PathBuf>) -> Self {
        Self {
            root,
            ops: Mutex::new(()),
        }
    }

    /// 既定の場所（[`default_presets_dir`]）。
    pub fn at_default() -> Self {
        Self::new(default_presets_dir())
    }

    /// プリセットのフォルダ（分からなければ `None`）。
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    fn kind_dir(&self, kind: PresetKind) -> Result<PathBuf, ApiError> {
        let root = self.root.as_ref().ok_or_else(|| {
            ApiError::InvalidArgument(format!(
                "プリセットのフォルダが分かりません（環境変数 {ENV_PRESETS_DIR} で指定してください）"
            ))
        })?;
        Ok(root.join(kind.as_str()))
    }

    fn path_of(&self, kind: PresetKind, id: &str) -> Result<PathBuf, ApiError> {
        check_id(id)?;
        Ok(self.kind_dir(kind)?.join(format!("{id}.json")))
    }

    /// 1 件のファイルを読む（ない・読めない・形が違えばエラー）。
    fn read(&self, kind: PresetKind, id: &str) -> Result<StoredPreset, ApiError> {
        let path = self.path_of(kind, id)?;
        let bytes = match read_limited(&path) {
            Ok(b) => b,
            Err(ApiError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(ApiError::NotFound(format!("{} {id}", kind.label())));
            }
            Err(e) => return Err(e),
        };
        parse_stored(&bytes).map_err(ApiError::InvalidArgument)
    }

    /// 1 件を読み、使えることを確かめる（使えなければ [`ApiError::InvalidArgument`]）。
    fn read_usable(
        &self,
        kind: PresetKind,
        id: &str,
    ) -> Result<(StoredPreset, PresetInfo), ApiError> {
        let stored = self.read(kind, id)?;
        let info = info_of(kind, id, &stored);
        if let Some(p) = &info.problem {
            return Err(ApiError::InvalidArgument(format!(
                "{}「{}」は使えません: {p}",
                kind.label(),
                info.name
            )));
        }
        Ok((stored, info))
    }

    /// 1 件の情報（ファイルがなければ [`ApiError::NotFound`]。使えないものは理由を付けて返す）。
    pub fn get(&self, kind: PresetKind, id: &str) -> Result<PresetInfo, ApiError> {
        match self.read(kind, id) {
            Ok(s) => Ok(info_of(kind, id, &s)),
            Err(ApiError::InvalidArgument(m)) if is_valid_preset_id(id) => {
                Ok(broken_info(kind, id, m))
            }
            Err(e) => Err(e),
        }
    }

    /// 種類 `kind` のプリセットの一覧（フォルダ → 名前の順。読めないファイルも理由を付けて出す）。フォルダが
    /// なければ空。
    pub fn list(&self, kind: PresetKind) -> Result<Vec<PresetInfo>, ApiError> {
        let dir = self.kind_dir(kind)?;
        let entries = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ApiError::io(&dir, e)),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| ApiError::io(&dir, e))?;
            let file_name = entry.file_name();
            let Some(id) = file_name
                .to_str()
                .and_then(|n| n.strip_suffix(".json"))
                .filter(|id| is_valid_preset_id(id))
            else {
                continue;
            };
            if out.len() >= MAX_PRESETS {
                tracing::warn!(dir = %dir.display(), "プリセットが多すぎるので、{MAX_PRESETS} 件までを読む");
                break;
            }
            let info = match read_limited(&entry.path()) {
                Ok(bytes) => match parse_stored(&bytes) {
                    Ok(s) => info_of(kind, id, &s),
                    Err(m) => broken_info(kind, id, m),
                },
                Err(e) => broken_info(kind, id, e.to_string()),
            };
            out.push(info);
        }
        out.sort_by(|a, b| {
            (name_key(&a.folder), name_key(&a.name), &a.id).cmp(&(
                name_key(&b.folder),
                name_key(&b.name),
                &b.id,
            ))
        });
        Ok(out)
    }

    /// 同じフォルダに同じ名前のもの（`except` を除く）があればエラー。
    fn check_unique(
        &self,
        kind: PresetKind,
        folder: &str,
        name: &str,
        except: Option<&str>,
    ) -> Result<(), ApiError> {
        let (fk, nk) = (name_key(folder), name_key(name));
        if let Some(other) = self.list(kind)?.into_iter().find(|p| {
            Some(p.id.as_str()) != except
                && p.problem.is_none()
                && name_key(&p.folder) == fk
                && name_key(&p.name) == nk
        }) {
            let place = if other.folder.is_empty() {
                String::new()
            } else {
                format!("フォルダ「{}」に", other.folder)
            };
            return Err(ApiError::NameConflict(format!(
                "{place}同じ名前の{}「{}」があります",
                kind.label(),
                other.name
            )));
        }
        Ok(())
    }

    /// 書く（`create` なら既存のファイルを上書きしない。重なれば `Ok(false)`）。
    fn write(
        &self,
        kind: PresetKind,
        stored: &StoredPreset,
        create: bool,
    ) -> Result<bool, ApiError> {
        let dir = self.kind_dir(kind)?;
        std::fs::create_dir_all(&dir).map_err(|e| ApiError::io(&dir, e))?;
        let path = self.path_of(kind, &stored.id)?;
        let json = serde_json::to_vec_pretty(stored)
            .map_err(|e| ApiError::Internal(format!("プリセットを JSON にできません: {e}")))?;
        if json.len() as u64 > MAX_PRESET_FILE_BYTES {
            return Err(ApiError::InvalidArgument(format!(
                "プリセットが大きすぎます（{} バイト。上限 {MAX_PRESET_FILE_BYTES} バイト）",
                json.len()
            )));
        }
        let policy = if create {
            ConflictPolicy::Skip
        } else {
            ConflictPolicy::Overwrite
        };
        let outcome = write_atomically(&path, policy, &ProtectedFiles::new(), |f| {
            f.write_all(&json).map_err(|e| MediaError::Io {
                path: Some(path.clone()),
                source: e,
            })
        })?;
        Ok(matches!(outcome, WriteOutcome::Written { .. }))
    }

    /// 新しい ID で作る（ID が重なれば作り直す）。
    fn create_stored(
        &self,
        kind: PresetKind,
        mut stored: StoredPreset,
    ) -> Result<PresetInfo, ApiError> {
        for _ in 0..8 {
            stored.id = new_id();
            if self.write(kind, &stored, true)? {
                return Ok(info_of(kind, &stored.id, &stored));
            }
        }
        Err(ApiError::Internal(
            "プリセットの ID を決められません（重なりが続きました）".to_owned(),
        ))
    }

    fn new_stored(
        kind: PresetKind,
        name: String,
        folder: String,
        mut body: Map<String, Value>,
    ) -> StoredPreset {
        let now = now_string();
        body.insert(
            "app_version".to_owned(),
            Value::String(env!("CARGO_PKG_VERSION").to_owned()),
        );
        StoredPreset {
            format: PRESET_FORMAT.to_owned(),
            format_version: PRESET_FORMAT_VERSION,
            kind,
            id: String::new(),
            name,
            folder,
            created_at: Some(now.clone()),
            updated_at: Some(now),
            body,
        }
    }

    /// 現像プリセットを作る（DEV-29）。`settings` は写真の今の設定（[`develop_preset_settings`] で、選んだ項目
    /// だけにして保存する）。
    pub fn create_develop(
        &self,
        name: &str,
        folder: &str,
        groups: SettingGroups,
        settings: &DevelopSettings,
    ) -> Result<PresetInfo, ApiError> {
        let name = checked_name(name, "プリセットの名前", false)?;
        let folder = checked_name(folder, "フォルダの名前", true)?;
        let body = develop_body(groups, &develop_preset_settings(groups, settings))?;
        let _ops = self.ops.lock();
        self.check_unique(PresetKind::Develop, &folder, &name, None)?;
        self.create_stored(
            PresetKind::Develop,
            Self::new_stored(PresetKind::Develop, name, folder, body),
        )
    }

    /// 現像プリセットの中身を、写真の今の設定 `settings` と含める項目 `groups` で置き換える（「今の設定で更新」）。
    pub fn update_develop(
        &self,
        id: &str,
        groups: SettingGroups,
        settings: &DevelopSettings,
    ) -> Result<PresetInfo, ApiError> {
        let body = develop_body(groups, &develop_preset_settings(groups, settings))?;
        self.modify(PresetKind::Develop, id, |s| {
            for (k, v) in body {
                s.body.insert(k, v);
            }
            Ok(())
        })
    }

    /// 現像プリセットの含める項目と設定（使えなければエラー）。
    pub fn develop_settings(
        &self,
        id: &str,
    ) -> Result<(PresetInfo, SettingGroups, DevelopSettings), ApiError> {
        let (stored, info) = self.read_usable(PresetKind::Develop, id)?;
        let (groups, settings) = read_develop(&stored.body).map_err(ApiError::InvalidArgument)?;
        Ok((info, groups, settings))
    }

    /// 書き出しプリセットを作る（EXP-02）。
    pub fn create_export(
        &self,
        name: &str,
        folder: &str,
        settings: &ExportPresetSettings,
    ) -> Result<PresetInfo, ApiError> {
        let name = checked_name(name, "プリセットの名前", false)?;
        let folder = checked_name(folder, "フォルダの名前", true)?;
        let body = export_body(settings)?;
        let _ops = self.ops.lock();
        self.check_unique(PresetKind::Export, &folder, &name, None)?;
        self.create_stored(
            PresetKind::Export,
            Self::new_stored(PresetKind::Export, name, folder, body),
        )
    }

    /// 書き出しプリセットの中身を置き換える（「今の設定で更新」）。
    pub fn update_export(
        &self,
        id: &str,
        settings: &ExportPresetSettings,
    ) -> Result<PresetInfo, ApiError> {
        let body = export_body(settings)?;
        self.modify(PresetKind::Export, id, |s| {
            for (k, v) in body {
                s.body.insert(k, v);
            }
            Ok(())
        })
    }

    /// 書き出しプリセットの中身（使えなければエラー）。
    pub fn export_settings(
        &self,
        id: &str,
    ) -> Result<(PresetInfo, ExportPresetSettings), ApiError> {
        let (_, info) = self.read_usable(PresetKind::Export, id)?;
        let settings = info
            .export
            .clone()
            .ok_or_else(|| ApiError::Internal("書き出しプリセットの中身がありません".to_owned()))?;
        Ok((info, settings))
    }

    /// 使えるプリセットを読んで `change` で変えて書き直す（知らない項目は残す。更新の日時を変える）。
    fn modify(
        &self,
        kind: PresetKind,
        id: &str,
        change: impl FnOnce(&mut StoredPreset) -> Result<(), ApiError>,
    ) -> Result<PresetInfo, ApiError> {
        let _ops = self.ops.lock();
        let (mut stored, _) = self.read_usable(kind, id)?;
        change(&mut stored)?;
        stored.id = id.to_owned();
        stored.updated_at = Some(now_string());
        self.write(kind, &stored, false)?;
        Ok(info_of(kind, id, &stored))
    }

    /// 名前を変える。
    pub fn rename(&self, kind: PresetKind, id: &str, name: &str) -> Result<PresetInfo, ApiError> {
        let name = checked_name(name, "プリセットの名前", false)?;
        let current = self.get(kind, id)?;
        // 重なりの確認は、読んで書くまでのロックの中で行う（modify の中ではロックを取り直さない）。
        let _ops = self.ops.lock();
        self.check_unique(kind, &current.folder, &name, Some(id))?;
        let (mut stored, _) = self.read_usable(kind, id)?;
        stored.name = name;
        stored.id = id.to_owned();
        stored.updated_at = Some(now_string());
        self.write(kind, &stored, false)?;
        Ok(info_of(kind, id, &stored))
    }

    /// フォルダを移す（空の名前でフォルダなし）。
    pub fn move_to(
        &self,
        kind: PresetKind,
        id: &str,
        folder: &str,
    ) -> Result<PresetInfo, ApiError> {
        let folder = checked_name(folder, "フォルダの名前", true)?;
        let _ops = self.ops.lock();
        let (mut stored, _) = self.read_usable(kind, id)?;
        self.check_unique(kind, &folder, &stored.name, Some(id))?;
        stored.folder = folder;
        stored.id = id.to_owned();
        stored.updated_at = Some(now_string());
        self.write(kind, &stored, false)?;
        Ok(info_of(kind, id, &stored))
    }

    /// フォルダの名前を変える（そのフォルダのプリセットをすべて `name` のフォルダへ移す。空の名前でフォルダなし）。
    /// 移した数を返す。移す先に同じ名前のものがあれば、何も変えずに [`ApiError::NameConflict`]。使えない
    /// プリセット（新しい版のアプリで作ったものなど）は移さない。途中で書けなければエラーで、それまでに移したものは
    /// 移したまま。
    pub fn rename_folder(
        &self,
        kind: PresetKind,
        folder: &str,
        name: &str,
    ) -> Result<usize, ApiError> {
        let new = checked_name(name, "フォルダの名前", true)?;
        let _ops = self.ops.lock();
        let all = self.list(kind)?;
        let fk = name_key(folder.trim());
        let moving: Vec<&PresetInfo> = all
            .iter()
            .filter(|p| p.problem.is_none() && name_key(&p.folder) == fk)
            .collect();
        if moving.is_empty() {
            return Err(ApiError::NotFound(format!("フォルダ「{}」", folder.trim())));
        }
        let moving_ids: HashSet<&str> = moving.iter().map(|p| p.id.as_str()).collect();
        let nk = name_key(&new);
        for p in &moving {
            if let Some(other) = all.iter().find(|o| {
                !moving_ids.contains(o.id.as_str())
                    && o.problem.is_none()
                    && name_key(&o.folder) == nk
                    && name_key(&o.name) == name_key(&p.name)
            }) {
                return Err(ApiError::NameConflict(format!(
                    "移す先のフォルダに同じ名前の{}「{}」があります",
                    kind.label(),
                    other.name
                )));
            }
        }
        let mut n = 0;
        for p in moving {
            let (mut stored, _) = self.read_usable(kind, &p.id)?;
            stored.folder = new.clone();
            stored.id = p.id.clone();
            stored.updated_at = Some(now_string());
            self.write(kind, &stored, false)?;
            n += 1;
        }
        Ok(n)
    }

    /// 削除する（ファイルを消す。使えないものも消せる）。
    pub fn delete(&self, kind: PresetKind, id: &str) -> Result<(), ApiError> {
        let path = self.path_of(kind, id)?;
        let _ops = self.ops.lock();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(ApiError::NotFound(format!("{} {id}", kind.label())))
            }
            Err(e) => Err(ApiError::io(&path, e)),
        }
    }

    /// プリセットのファイルを読み込む（元のファイルは変えない。モジュールの doc）。`kind` を指定すると、その種類の
    /// ファイルだけを読み込む。ファイルごとに結果を返し、読み込めなかったファイルがあってもほかは続ける。
    pub fn import_files(
        &self,
        paths: &[PathBuf],
        kind: Option<PresetKind>,
    ) -> Result<PresetImportReport, ApiError> {
        if paths.len() > MAX_PRESET_FILES_PER_CALL {
            return Err(ApiError::InvalidArgument(format!(
                "一度に読み込めるファイルは {MAX_PRESET_FILES_PER_CALL} 個までです"
            )));
        }
        let mut report = PresetImportReport::default();
        for path in paths {
            match self.import_one(path, kind) {
                Ok(info) => report.imported.push(info),
                Err(e) => report.failed.push(PresetFileFailure {
                    path: path.clone(),
                    message: e.to_string(),
                }),
            }
        }
        Ok(report)
    }

    fn import_one(&self, path: &Path, kind: Option<PresetKind>) -> Result<PresetInfo, ApiError> {
        let bytes = read_limited(path)?;
        let mut stored = parse_stored(&bytes).map_err(ApiError::InvalidArgument)?;
        if let Some(k) = kind
            && stored.kind != k
        {
            return Err(ApiError::InvalidArgument(format!(
                "{}のファイルです（{}を選んでください）",
                stored.kind.label(),
                k.label()
            )));
        }
        let k = stored.kind;
        let info = info_of(k, "0000000000000000", &stored);
        if let Some(p) = info.problem {
            return Err(ApiError::InvalidArgument(p));
        }
        let folder = checked_name(&stored.folder, "フォルダの名前", true)?;
        let base = checked_name(&stored.name, "プリセットの名前", false)?;
        let _ops = self.ops.lock();
        let name = self.free_name(k, &folder, &base)?;
        let now = now_string();
        stored.name = name;
        stored.folder = folder;
        if parse_time(stored.created_at.as_deref()).is_none() {
            stored.created_at = Some(now.clone());
        }
        stored.updated_at = Some(now);
        self.create_stored(k, stored)
    }

    /// フォルダ `folder` で使える名前（`base` が使われていれば「 (2)」から順に番号を付ける）。
    fn free_name(&self, kind: PresetKind, folder: &str, base: &str) -> Result<String, ApiError> {
        let fk = name_key(folder);
        let taken: HashSet<String> = self
            .list(kind)?
            .into_iter()
            .filter(|p| p.problem.is_none() && name_key(&p.folder) == fk)
            .map(|p| name_key(&p.name))
            .collect();
        if !taken.contains(&name_key(base)) {
            return Ok(base.to_owned());
        }
        for n in 2..=MAX_NAME_SUFFIX {
            let suffix = format!(" ({n})");
            let keep = MAX_PRESET_NAME_CHARS.saturating_sub(suffix.chars().count());
            let stem: String = base.chars().take(keep).collect();
            let candidate = format!("{}{suffix}", stem.trim_end());
            if !taken.contains(&name_key(&candidate)) {
                return Ok(candidate);
            }
        }
        Err(ApiError::NameConflict(format!(
            "同じ名前のプリセットが多すぎます（{base}）"
        )))
    }

    /// プリセットをファイルに書き出す（`dest_dir` に `<名前>.json`。同じ名前のファイルがあれば `-1` などを付け、
    /// 上書きしない）。書いたファイルを `ids` の順に返す。途中で失敗したらエラー（それまでに書いたファイルは残る）。
    pub fn export_files(
        &self,
        kind: PresetKind,
        ids: &[String],
        dest_dir: &Path,
    ) -> Result<Vec<PathBuf>, ApiError> {
        if ids.len() > MAX_PRESET_FILES_PER_CALL {
            return Err(ApiError::InvalidArgument(format!(
                "一度に書き出せるプリセットは {MAX_PRESET_FILES_PER_CALL} 個までです"
            )));
        }
        let dest = crate::paths::absolute_lexical(dest_dir)?;
        if !dest.is_dir() {
            return Err(ApiError::NotFound(format!(
                "書き出し先のフォルダ {}",
                dest.display()
            )));
        }
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let path = self.path_of(kind, id)?;
            let bytes = match read_limited(&path) {
                Ok(b) => b,
                Err(ApiError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Err(ApiError::NotFound(format!("{} {id}", kind.label())));
                }
                Err(e) => return Err(e),
            };
            let stored = parse_stored(&bytes).map_err(ApiError::InvalidArgument)?;
            let file = format!("{}.json", crate::naming::safe_file_stem(&stored.name));
            let desired = dest.join(file);
            // 中身はそのまま写す（知らない項目も残す）。
            let outcome = write_atomically(
                &desired,
                ConflictPolicy::Sequence,
                &ProtectedFiles::new(),
                |f| {
                    f.write_all(&bytes).map_err(|e| MediaError::Io {
                        path: Some(desired.clone()),
                        source: e,
                    })
                },
            )?;
            match outcome {
                WriteOutcome::Written { path, .. } => out.push(path),
                WriteOutcome::Skipped { existing } => {
                    return Err(ApiError::Internal(format!(
                        "書き出し先に同じ名前のファイルがあります（{}）",
                        existing.display()
                    )));
                }
            }
        }
        Ok(out)
    }
}

/// 現像プリセットを適用したときの履歴の名前（「プリセット「フィルム風」を適用」）。
pub fn apply_preset_label(name: &str) -> String {
    format!("プリセット「{name}」を適用")
}

impl Core {
    /// プリセットのフォルダ（分からなければ `None`）。
    pub fn presets_dir(&self) -> Option<PathBuf> {
        self.inner.presets.root().map(Path::to_path_buf)
    }

    /// 種類 `kind` のプリセットの一覧（[`PresetStore::list`]）。
    pub fn presets(&self, kind: PresetKind) -> Result<Vec<PresetInfo>, ApiError> {
        self.inner.check_open()?;
        self.inner.presets.list(kind)
    }

    /// 現像プリセットの設定（含めない項目は既定値。使えなければエラー）。
    pub fn develop_preset_settings(&self, id: &str) -> Result<DevelopSettings, ApiError> {
        self.inner.check_open()?;
        Ok(self.inner.presets.develop_settings(id)?.2)
    }

    /// 写真 `variant_id` の今の設定（現像中なら保存待ちの変更を含む）から、`groups` の項目を含む現像プリセットを
    /// 作る（DEV-29）。動画は [`ApiError::InvalidArgument`]。
    pub fn create_develop_preset(
        &self,
        name: &str,
        folder: &str,
        groups: SettingGroups,
        variant_id: VariantId,
    ) -> Result<PresetInfo, ApiError> {
        let current = self.preset_source(variant_id)?;
        self.inner
            .presets
            .create_develop(name, folder, groups, &current)
    }

    /// 現像プリセットを、写真 `variant_id` の今の設定と含める項目 `groups` で置き換える（「今の設定で更新」）。
    pub fn update_develop_preset(
        &self,
        id: &str,
        groups: SettingGroups,
        variant_id: VariantId,
    ) -> Result<PresetInfo, ApiError> {
        let current = self.preset_source(variant_id)?;
        self.inner.presets.update_develop(id, groups, &current)
    }

    /// プリセットの元にする写真の今の設定（動画は拒む）。
    fn preset_source(&self, variant_id: VariantId) -> Result<DevelopSettings, ApiError> {
        self.inner.check_open()?;
        self.inner
            .with_catalog_api(|c| crate::develop::reject_videos(c, &[variant_id]))?;
        self.develop_settings(variant_id)
    }

    /// 現像プリセットを選んだ写真に適用する（DEV-29。モジュールの doc の「処理バージョン」）。プリセットに含める
    /// 項目だけを写し、それぞれの写真の履歴に「プリセット「名前」を適用」を 1 件記録する（1 つのトランザクション）。
    /// **動画は除いて**（現像設定を保存しない）写真にだけ適用し、除いた数を [`PasteReport::skipped_videos`] で返す
    /// （統合タイムラインで写真と動画をまとめて選んでも使えるように。決定事項ログ No.171）。
    pub fn apply_develop_preset(
        &self,
        id: &str,
        targets: &[VariantId],
        version: PasteVersion,
    ) -> Result<PasteReport, ApiError> {
        self.inner.check_open()?;
        let (info, groups, settings) = self.inner.presets.develop_settings(id)?;
        let videos: HashSet<VariantId> = self.inner.with_catalog_api(|c| {
            Ok(c.variant_summaries(targets)?
                .into_iter()
                .filter(|s| s.kind == genzo_model::AssetKind::Video)
                .map(|s| s.variant_id)
                .collect())
        })?;
        let photos: Vec<VariantId> = targets
            .iter()
            .copied()
            .filter(|v| !videos.contains(v))
            .collect();
        let mut report = self.paste_settings_with(
            &settings,
            &photos,
            groups,
            version,
            &apply_preset_label(&info.name),
        )?;
        report.skipped_videos = videos.len() as u64;
        Ok(report)
    }

    /// 書き出しプリセットを作る（EXP-02）。
    pub fn create_export_preset(
        &self,
        name: &str,
        folder: &str,
        settings: &ExportPresetSettings,
    ) -> Result<PresetInfo, ApiError> {
        self.inner.check_open()?;
        self.inner.presets.create_export(name, folder, settings)
    }

    /// 書き出しプリセットの中身を置き換える（EXP-02。「今の設定で更新」）。
    pub fn update_export_preset(
        &self,
        id: &str,
        settings: &ExportPresetSettings,
    ) -> Result<PresetInfo, ApiError> {
        self.inner.check_open()?;
        self.inner.presets.update_export(id, settings)
    }

    /// プリセットの名前を変える。
    pub fn rename_preset(
        &self,
        kind: PresetKind,
        id: &str,
        name: &str,
    ) -> Result<PresetInfo, ApiError> {
        self.inner.check_open()?;
        self.inner.presets.rename(kind, id, name)
    }

    /// プリセットをフォルダへ移す（空の名前でフォルダなし）。
    pub fn move_preset(
        &self,
        kind: PresetKind,
        id: &str,
        folder: &str,
    ) -> Result<PresetInfo, ApiError> {
        self.inner.check_open()?;
        self.inner.presets.move_to(kind, id, folder)
    }

    /// フォルダの名前を変える（移した数を返す。[`PresetStore::rename_folder`]）。
    pub fn rename_preset_folder(
        &self,
        kind: PresetKind,
        folder: &str,
        name: &str,
    ) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        self.inner.presets.rename_folder(kind, folder, name)
    }

    /// プリセットを削除する。
    pub fn delete_preset(&self, kind: PresetKind, id: &str) -> Result<(), ApiError> {
        self.inner.check_open()?;
        self.inner.presets.delete(kind, id)
    }

    /// プリセットのファイルを読み込む（[`PresetStore::import_files`]）。
    pub fn import_presets(
        &self,
        paths: &[PathBuf],
        kind: Option<PresetKind>,
    ) -> Result<PresetImportReport, ApiError> {
        self.inner.check_open()?;
        self.inner.presets.import_files(paths, kind)
    }

    /// プリセットをファイルに書き出す（[`PresetStore::export_files`]）。
    pub fn export_preset_files(
        &self,
        kind: PresetKind,
        ids: &[String],
        dest_dir: &Path,
    ) -> Result<Vec<PathBuf>, ApiError> {
        self.inner.check_open()?;
        self.inner.presets.export_files(kind, ids, dest_dir)
    }

    /// 書き出しのファイル名の例（見本の写真。[`crate::export::preview_export_names`]）。
    pub fn export_name_preview(
        &self,
        file_naming: Option<&RenameRule>,
        format: ExportFormat,
    ) -> NamePreview {
        crate::export::preview_export_names(file_naming, format.extension())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, PresetStore) {
        let d = tempfile::tempdir().unwrap();
        let s = PresetStore::new(Some(d.path().join("presets")));
        (d, s)
    }

    fn edited() -> DevelopSettings {
        let mut s = DevelopSettings {
            exposure_ev: 0.7,
            contrast: 20.0,
            ..DevelopSettings::default()
        };
        s.geometry.angle_deg = 3.0;
        s.render_deps.raw_decoder = "libraw-0.21.2".to_owned();
        s
    }

    fn only_tone() -> SettingGroups {
        SettingGroups {
            white_balance: false,
            basic_tone: true,
            presence: false,
            tone_curve: false,
            color: false,
            geometry: false,
            lens: false,
            transform: false,
            detail: false,
            effects: false,
            profile: false,
            masks: false,
            retouch: false,
        }
    }

    #[test]
    fn default_dir_follows_the_os_and_the_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| OsString::from(*v))
            }
        };
        if cfg!(windows) {
            assert_eq!(
                presets_dir_from(env(&[("APPDATA", r"C:\Users\u\AppData\Roaming")])),
                Some(PathBuf::from(
                    r"C:\Users\u\AppData\Roaming\io.github.tokamocchi.genzoparis\presets"
                ))
            );
        } else if cfg!(target_os = "macos") {
            assert_eq!(
                presets_dir_from(env(&[("HOME", "/Users/u")])),
                Some(PathBuf::from(
                    "/Users/u/Library/Application Support/io.github.tokamocchi.genzoparis/presets"
                ))
            );
        } else {
            assert_eq!(
                presets_dir_from(env(&[("HOME", "/home/u")])),
                Some(PathBuf::from(
                    "/home/u/.local/share/io.github.tokamocchi.genzoparis/presets"
                ))
            );
            assert_eq!(
                presets_dir_from(env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/data")])),
                Some(PathBuf::from(
                    "/data/io.github.tokamocchi.genzoparis/presets"
                ))
            );
            // 相対パスの XDG_DATA_HOME は使わない。HOME もなければ分からない。
            assert_eq!(presets_dir_from(env(&[("XDG_DATA_HOME", "rel")])), None);
        }
        // 環境変数があれば、それを使う。
        let dir = std::env::temp_dir().join("genzo-presets");
        let s = dir.to_str().unwrap().to_owned();
        let got = presets_dir_from(move |k| (k == ENV_PRESETS_DIR).then(|| OsString::from(&s)));
        assert_eq!(got, Some(dir));
        let (_d, empty) = (0, PresetStore::new(None));
        assert!(matches!(
            empty.list(PresetKind::Develop),
            Err(ApiError::InvalidArgument(_))
        ));
    }

    #[test]
    fn ids_are_hex_and_reject_paths() {
        let id = new_id();
        assert!(is_valid_preset_id(&id), "{id}");
        assert_ne!(new_id(), id);
        for bad in [
            "",
            "../../etc/passwd",
            "0123456789ABCDEF",
            "0123456789abcde",
            "0123456789abcdefg",
            "0123456789abcde/",
        ] {
            assert!(!is_valid_preset_id(bad), "{bad}");
        }
        let (_d, s) = store();
        assert!(matches!(
            s.get(PresetKind::Develop, "../x"),
            Err(ApiError::InvalidArgument(_))
        ));
        assert!(matches!(
            s.delete(PresetKind::Develop, "..\\..\\x"),
            Err(ApiError::InvalidArgument(_))
        ));
    }

    /// 現像プリセット: 選んだ項目だけを保存し、ほかの項目は既定値・外部データは空にする。一覧・名前の規則・重なり。
    #[test]
    fn develop_presets_keep_only_the_chosen_groups() {
        let (_d, s) = store();
        assert!(s.list(PresetKind::Develop).unwrap().is_empty());
        let p = s
            .create_develop(" 明るめ ", "  ", only_tone(), &edited())
            .unwrap();
        assert_eq!(p.name, "明るめ");
        assert_eq!(p.folder, "");
        assert_eq!(p.groups, Some(only_tone()));
        assert_eq!(
            p.process_version,
            Some(genzo_model::CURRENT_PROCESS_VERSION)
        );
        assert!(p.usable());
        let (_, groups, settings) = s.develop_settings(&p.id).unwrap();
        assert_eq!(groups, only_tone());
        assert_eq!(settings.exposure_ev, 0.7);
        assert_eq!(settings.contrast, 20.0);
        // 含めない項目（切り抜きと角度）は既定値、外部データは空。
        assert_eq!(settings.geometry, DevelopSettings::default().geometry);
        assert_eq!(settings.render_deps, RenderDeps::default());
        // ファイルは種類のフォルダの <ID>.json。
        let file = s
            .root()
            .unwrap()
            .join("develop")
            .join(format!("{}.json", p.id));
        let v: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(v["format"], PRESET_FORMAT);
        assert_eq!(v["format_version"], PRESET_FORMAT_VERSION);
        assert_eq!(v["kind"], "develop");
        assert_eq!(v["id"], p.id.as_str());

        // 同じフォルダの同じ名前（全角・半角・大文字・小文字の違いは同じとみなす）は作れない。
        let mut tone = only_tone();
        tone.white_balance = true;
        assert!(matches!(
            s.create_develop("明るめ", "", tone, &edited()),
            Err(ApiError::NameConflict(_))
        ));
        let film = s.create_develop("Film", "ルック", tone, &edited()).unwrap();
        assert!(matches!(
            s.create_develop("ｆｉｌｍ", "ルック", tone, &edited()),
            Err(ApiError::NameConflict(_))
        ));
        // 別のフォルダなら同じ名前でよい。
        s.create_develop("Film", "", tone, &edited()).unwrap();
        // 一覧はフォルダ → 名前の順。
        let names: Vec<(String, String)> = s
            .list(PresetKind::Develop)
            .unwrap()
            .into_iter()
            .map(|p| (p.folder, p.name))
            .collect();
        assert_eq!(
            names,
            vec![
                (String::new(), "Film".to_owned()),
                (String::new(), "明るめ".to_owned()),
                ("ルック".to_owned(), "Film".to_owned()),
            ]
        );
        // 名前の規則。
        assert!(matches!(
            s.create_develop("  ", "", tone, &edited()),
            Err(ApiError::InvalidArgument(_))
        ));
        assert!(matches!(
            s.create_develop("a\tb", "", tone, &edited()),
            Err(ApiError::InvalidArgument(_))
        ));
        assert!(matches!(
            s.create_develop(&"あ".repeat(MAX_PRESET_NAME_CHARS + 1), "", tone, &edited()),
            Err(ApiError::InvalidArgument(_))
        ));
        // 項目を 1 つも選ばないものは作れない。
        let mut none = only_tone();
        none.basic_tone = false;
        assert!(matches!(
            s.create_develop("空", "", none, &edited()),
            Err(ApiError::InvalidArgument(_))
        ));
        // 名前の変更・移動・削除。
        let renamed = s.rename(PresetKind::Develop, &film.id, "フィルム").unwrap();
        assert_eq!(renamed.name, "フィルム");
        assert!(matches!(
            s.move_to(PresetKind::Develop, &film.id, ""),
            Ok(ref p) if p.folder.is_empty()
        ));
        assert!(matches!(
            s.rename(PresetKind::Develop, &film.id, "明るめ"),
            Err(ApiError::NameConflict(_))
        ));
        s.delete(PresetKind::Develop, &film.id).unwrap();
        assert!(matches!(
            s.delete(PresetKind::Develop, &film.id),
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            s.get(PresetKind::Develop, &film.id),
            Err(ApiError::NotFound(_))
        ));
    }

    /// 更新・名前の変更では、知らない項目を残す（新しい版のアプリが足した項目を消さない）。
    #[test]
    fn unknown_fields_survive_renames_and_updates() {
        let (_d, s) = store();
        let p = s
            .create_develop("a", "", SettingGroups::ALL, &edited())
            .unwrap();
        let file = s
            .root()
            .unwrap()
            .join("develop")
            .join(format!("{}.json", p.id));
        let mut v: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        v["future_field"] = serde_json::json!({"x": 1});
        std::fs::write(&file, serde_json::to_vec(&v).unwrap()).unwrap();
        s.rename(PresetKind::Develop, &p.id, "b").unwrap();
        let mut groups = only_tone();
        groups.color = true;
        let updated = s.update_develop(&p.id, groups, &edited()).unwrap();
        assert_eq!(updated.groups, Some(groups));
        let v: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(v["future_field"], serde_json::json!({"x": 1}));
        assert_eq!(v["name"], "b");
        assert!(v["created_at"].as_str().is_some());
    }

    /// 新しい版の形式・知らない処理バージョン・壊れたファイルは、理由を付けて一覧に出し、使わない（消せる）。
    #[test]
    fn newer_and_broken_files_are_listed_but_not_used() {
        let (_d, s) = store();
        let p = s
            .create_develop("ok", "", SettingGroups::ALL, &edited())
            .unwrap();
        let dir = s.root().unwrap().join("develop");
        let mut newer: Value =
            serde_json::from_slice(&std::fs::read(dir.join(format!("{}.json", p.id))).unwrap())
                .unwrap();
        newer["format_version"] = serde_json::json!(PRESET_FORMAT_VERSION + 1);
        std::fs::write(dir.join("aaaaaaaaaaaaaaaa.json"), newer.to_string()).unwrap();
        let mut pv: Value = newer.clone();
        pv["format_version"] = serde_json::json!(1);
        pv["settings"]["process_version"] =
            serde_json::json!(genzo_model::CURRENT_PROCESS_VERSION + 1);
        std::fs::write(dir.join("bbbbbbbbbbbbbbbb.json"), pv.to_string()).unwrap();
        std::fs::write(dir.join("cccccccccccccccc.json"), b"{broken").unwrap();
        // 名前の形が違うファイルは見ない。
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.join("ABCDEF0123456789.json"), b"{}").unwrap();
        let list = s.list(PresetKind::Develop).unwrap();
        assert_eq!(list.len(), 4);
        let by_id = |id: &str| list.iter().find(|p| p.id == id).unwrap().clone();
        assert!(by_id(&p.id).usable());
        assert!(
            by_id("aaaaaaaaaaaaaaaa")
                .problem
                .unwrap()
                .contains("新しい版")
        );
        assert!(
            by_id("bbbbbbbbbbbbbbbb")
                .problem
                .unwrap()
                .contains("process_version")
        );
        assert_eq!(by_id("cccccccccccccccc").name, "（読めないプリセット）");
        // 使えないものは適用・名前の変更をしない（ファイルは変えない）。
        let before = std::fs::read(dir.join("aaaaaaaaaaaaaaaa.json")).unwrap();
        assert!(matches!(
            s.rename(PresetKind::Develop, "aaaaaaaaaaaaaaaa", "x"),
            Err(ApiError::InvalidArgument(_))
        ));
        assert!(s.develop_settings("bbbbbbbbbbbbbbbb").is_err());
        assert_eq!(
            std::fs::read(dir.join("aaaaaaaaaaaaaaaa.json")).unwrap(),
            before
        );
        // 使えないものと同じ名前でも作れる（使えるものとだけ比べる）。
        s.delete(PresetKind::Develop, "cccccccccccccccc").unwrap();
        s.delete(PresetKind::Develop, "aaaaaaaaaaaaaaaa").unwrap();
        assert_eq!(s.list(PresetKind::Develop).unwrap().len(), 2);
    }

    /// 古いスキーマの版で保存した設定は、読むときに今の形にする（ファイルは書き換えない）。
    #[test]
    fn old_schema_presets_are_migrated_on_read() {
        let (_d, s) = store();
        let dir = s.root().unwrap().join("develop");
        std::fs::create_dir_all(&dir).unwrap();
        let old = serde_json::json!({
            "format": PRESET_FORMAT, "format_version": 1, "kind": "develop", "id": "1111111111111111",
            "name": "古い", "groups": {"basic_tone": true},
            "settings": {"schema_version": 1, "exposure_ev": 1.5}
        });
        std::fs::write(dir.join("1111111111111111.json"), old.to_string()).unwrap();
        let (info, groups, settings) = s.develop_settings("1111111111111111").unwrap();
        assert_eq!(info.process_version, Some(1));
        assert!(groups.basic_tone);
        assert_eq!(settings.exposure_ev, 1.5);
        assert_eq!(settings.process_version, 1);
        assert_eq!(settings.schema_version, genzo_model::CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn folders_can_be_renamed_without_conflicts() {
        let (_d, s) = store();
        let g = only_tone();
        let a = s.create_develop("A", "人物", g, &edited()).unwrap();
        s.create_develop("B", "人物", g, &edited()).unwrap();
        s.create_develop("A", "風景", g, &edited()).unwrap();
        // 移す先に同じ名前があれば、何も変えない。
        assert!(matches!(
            s.rename_folder(PresetKind::Develop, "人物", "風景"),
            Err(ApiError::NameConflict(_))
        ));
        assert_eq!(s.get(PresetKind::Develop, &a.id).unwrap().folder, "人物");
        assert_eq!(
            s.rename_folder(PresetKind::Develop, "人物", "ポートレート")
                .unwrap(),
            2
        );
        assert_eq!(
            s.get(PresetKind::Develop, &a.id).unwrap().folder,
            "ポートレート"
        );
        assert!(matches!(
            s.rename_folder(PresetKind::Develop, "人物", "x"),
            Err(ApiError::NotFound(_))
        ));
    }

    #[test]
    fn files_can_be_exported_and_imported_without_overwriting() {
        let (d, s) = store();
        let p = s
            .create_develop("フィルム/風", "ルック", only_tone(), &edited())
            .unwrap();
        let out = d.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let first = s
            .export_files(PresetKind::Develop, std::slice::from_ref(&p.id), &out)
            .unwrap();
        assert_eq!(first, vec![out.join("フィルム_風.json")]);
        // 同じ名前があれば上書きしない。
        let second = s
            .export_files(PresetKind::Develop, std::slice::from_ref(&p.id), &out)
            .unwrap();
        assert_eq!(second, vec![out.join("フィルム_風-1.json")]);
        // 読み込み: 新しい ID で保存し、同じフォルダの同じ名前には番号を付ける。元のファイルは変えない。
        let before = std::fs::read(&first[0]).unwrap();
        let report = s
            .import_files(
                &[first[0].clone(), second[0].clone(), out.join("none.json")],
                Some(PresetKind::Develop),
            )
            .unwrap();
        assert_eq!(report.imported.len(), 2);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.imported[0].name, "フィルム/風 (2)");
        assert_eq!(report.imported[1].name, "フィルム/風 (3)");
        assert_ne!(report.imported[0].id, p.id);
        assert_eq!(report.imported[0].folder, "ルック");
        assert_eq!(std::fs::read(&first[0]).unwrap(), before);
        // 種類が違うファイルは読み込まない。
        let report = s
            .import_files(std::slice::from_ref(&first[0]), Some(PresetKind::Export))
            .unwrap();
        assert!(report.imported.is_empty());
        assert!(report.failed[0].message.contains("現像プリセット"));
        // JSON でないファイル。
        std::fs::write(out.join("x.json"), b"not json").unwrap();
        let report = s.import_files(&[out.join("x.json")], None).unwrap();
        assert_eq!(report.failed.len(), 1);
    }

    #[test]
    fn export_presets_validate_and_round_trip() {
        let (d, s) = store();
        let settings = ExportPresetSettings {
            export: ExportSettings {
                format: genzo_model::ExportFormat::Png16,
                size: genzo_model::ExportSize::LongEdge(2048),
                remove_gps: true,
                ..ExportSettings::default()
            },
            destination: Some(d.path().join("web")),
            file_naming: Some(RenameRule {
                template: "{date}-{seq}".to_owned(),
                ..RenameRule::default()
            }),
        };
        let p = s.create_export("Web 用", "", &settings).unwrap();
        assert_eq!(p.export.as_ref(), Some(&settings));
        assert_eq!(s.export_settings(&p.id).unwrap().1, settings);
        // 正しくない値は保存しない。
        let bad_quality = ExportPresetSettings {
            export: ExportSettings {
                format: genzo_model::ExportFormat::Jpeg { quality: 0 },
                ..ExportSettings::default()
            },
            ..ExportPresetSettings::default()
        };
        assert!(s.create_export("x", "", &bad_quality).is_err());
        let relative = ExportPresetSettings {
            destination: Some(PathBuf::from("rel/dir")),
            ..ExportPresetSettings::default()
        };
        assert!(s.create_export("x", "", &relative).is_err());
        let bad_template = ExportPresetSettings {
            file_naming: Some(RenameRule {
                template: "{nope}".to_owned(),
                ..RenameRule::default()
            }),
            ..ExportPresetSettings::default()
        };
        assert!(s.create_export("x", "", &bad_template).is_err());
        // 出力シャープ・透かし・解像度・動画（EXP-03・EXP-05）も保存する。透かしのファイルは絶対パス。
        let finishing = ExportPresetSettings {
            export: ExportSettings {
                resolution_ppi: 300,
                output_sharpening: Some(genzo_model::OutputSharpening {
                    media: genzo_model::SharpenMedia::GlossyPaper,
                    amount: genzo_model::SharpenAmount::High,
                }),
                watermark: Some(genzo_model::Watermark::new(
                    genzo_model::WatermarkContent::Image {
                        path: d.path().join("logo.png"),
                    },
                )),
                videos: genzo_model::VideoExport::Skip,
                ..ExportSettings::default()
            },
            ..ExportPresetSettings::default()
        };
        let f = s.create_export("印刷", "", &finishing).unwrap();
        assert_eq!(s.export_settings(&f.id).unwrap().1, finishing);
        let relative_mark = ExportPresetSettings {
            export: ExportSettings {
                watermark: Some(genzo_model::Watermark::new(
                    genzo_model::WatermarkContent::Image {
                        path: PathBuf::from("logo.png"),
                    },
                )),
                ..ExportSettings::default()
            },
            ..ExportPresetSettings::default()
        };
        assert!(s.create_export("x", "", &relative_mark).is_err());
        // 更新。
        let updated = s
            .update_export(&p.id, &ExportPresetSettings::default())
            .unwrap();
        assert_eq!(updated.export, Some(ExportPresetSettings::default()));
        // 項目の足りない古いファイル（出力シャープ・透かしなどを足す前に作ったもの）は既定値で読む。
        let dir = s.root().unwrap().join("export");
        let old = serde_json::json!({
            "format": PRESET_FORMAT, "format_version": 1, "kind": "export",
            "name": "古い", "settings": {"export": {"format": "tiff16"}}
        });
        std::fs::write(dir.join("2222222222222222.json"), old.to_string()).unwrap();
        let (_, e) = s.export_settings("2222222222222222").unwrap();
        assert_eq!(e.export.format, genzo_model::ExportFormat::Tiff16);
        assert_eq!(e.destination, None);
        // 種類のフォルダが違えば使わない。
        let develop_dir = s.root().unwrap().join("develop");
        std::fs::create_dir_all(&develop_dir).unwrap();
        std::fs::copy(
            dir.join("2222222222222222.json"),
            develop_dir.join("2222222222222222.json"),
        )
        .unwrap();
        assert!(
            s.get(PresetKind::Develop, "2222222222222222")
                .unwrap()
                .problem
                .unwrap()
                .contains("書き出しプリセット")
        );
    }

    /// 別の OS で作った書き出しプリセット（書き出し先・透かしのフォントのパスがこの OS の絶対パスでない）も使える。
    /// そのパスだけを使わずに警告を付け、プリセット全体は使えないことにしない。ファイルの中のパスは消さない
    /// （決定事項ログ No.162・170。検証のラウンド 1 の指摘）。
    #[test]
    fn export_presets_from_another_os_are_usable_without_its_paths() {
        let (d, s) = store();
        let (dest, font, image) = if cfg!(windows) {
            ("/Users/a/out", "/Library/Fonts/A.ttf", "/Users/a/logo.png")
        } else {
            (
                "C:\\Users\\a\\out",
                "C:\\Windows\\Fonts\\meiryo.ttc",
                "C:\\Users\\a\\logo.png",
            )
        };
        let file = d.path().join("winpreset.json");
        let json = serde_json::json!({
            "format": PRESET_FORMAT, "format_version": 1, "kind": "export",
            "id": "0123456789abcdef", "name": "Web", "folder": "",
            "settings": {
                "export": {"format": "png8", "watermark": {
                    "content": {"text": {"text": "© 2026", "font": font}}, "size": 4}},
                "destination": dest, "file_naming": null
            }
        });
        std::fs::write(&file, json.to_string()).unwrap();
        // 読み込める（指摘の手順では、全体が「絶対パスで指定してください」で失敗していた）。
        let report = s.import_files(std::slice::from_ref(&file), None).unwrap();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let p = &report.imported[0];
        assert!(p.usable(), "{:?}", p.problem);
        let e = p.export.as_ref().unwrap();
        // パスは使わない（書き出し先は書き出すときに選び、フォントは選び直す）。ほかの値はそのまま。
        assert_eq!(e.destination, None);
        assert_eq!(e.export.format, genzo_model::ExportFormat::Png8);
        let w = e.export.watermark.as_ref().unwrap();
        assert_eq!(w.content.file(), Path::new(""));
        assert!(matches!(&w.content,
            genzo_model::WatermarkContent::Text { text, .. } if text == "© 2026"));
        assert_eq!(p.warnings.len(), 2, "{:?}", p.warnings);
        assert!(p.warnings[0].contains("書き出し先") && p.warnings[0].contains(dest));
        assert!(p.warnings[1].contains("フォント") && p.warnings[1].contains(font));
        // 一覧・中身の取り出しでも同じ。
        let listed = s.list(PresetKind::Export).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].usable());
        let (info, settings) = s.export_settings(&p.id).unwrap();
        assert_eq!(info.warnings, p.warnings);
        assert_eq!(settings.destination, None);
        // 書き出すときの検証は、フォントを選んでいないことを知らせる（選び直せば書き出せる）。
        let err = settings.export.validate().unwrap_err().to_string();
        assert!(err.contains("フォントのファイルを選んで"), "{err}");
        // 名前の変更ではファイルの中のパスを消さない（元の OS へ戻せば使える）。
        s.rename(PresetKind::Export, &p.id, "Web 2").unwrap();
        let saved: Value = serde_json::from_slice(
            &std::fs::read(
                s.root()
                    .unwrap()
                    .join("export")
                    .join(format!("{}.json", p.id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(saved["settings"]["destination"], serde_json::json!(dest));
        assert_eq!(
            saved["settings"]["export"]["watermark"]["content"]["text"]["font"],
            serde_json::json!(font)
        );
        // 画像の透かしも同じ。
        let mut image_json = json.clone();
        image_json["settings"]["export"]["watermark"] =
            serde_json::json!({"content": {"image": {"path": image}}, "size": 20});
        image_json["settings"]["destination"] = Value::Null;
        std::fs::write(&file, image_json.to_string()).unwrap();
        let report = s.import_files(std::slice::from_ref(&file), None).unwrap();
        let p = &report.imported[0];
        assert!(p.usable(), "{:?}", p.problem);
        assert_eq!(p.warnings.len(), 1);
        assert!(p.warnings[0].contains("画像") && p.warnings[0].contains(image));
        // 新しく保存するときは、これまでどおり絶対パスを求める。
        let relative = ExportPresetSettings {
            destination: Some(PathBuf::from(dest)),
            ..ExportPresetSettings::default()
        };
        assert!(s.create_export("x", "", &relative).is_err());
    }

    #[test]
    fn preset_settings_reset_unselected_groups() {
        let s = develop_preset_settings(only_tone(), &edited());
        assert_eq!(s.exposure_ev, 0.7);
        assert_eq!(s.geometry, DevelopSettings::default().geometry);
        assert_eq!(s.render_deps, RenderDeps::default());
        let all = develop_preset_settings(SettingGroups::ALL, &edited());
        assert_eq!(all.geometry.angle_deg, 3.0);
        assert_eq!(all.render_deps, RenderDeps::default());
        let v1 = DevelopSettings {
            process_version: 1,
            ..edited()
        };
        assert_eq!(develop_preset_settings(only_tone(), &v1).process_version, 1);
        assert_eq!(apply_preset_label("A"), "プリセット「A」を適用");
    }
}
