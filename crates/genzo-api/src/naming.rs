//! コピー取り込みのコピー先の名前（IMP-02 のフォルダの規則・IMP-03 のリネーム規則。04 の 3.12 節）。
//!
//! - **フォルダの規則**（[`FolderRule`]）: 撮影日（撮影日時の現地の日付。分からなければファイルの更新日時を
//!   既定のタイムゾーンで読んだ日付）で分ける（`2026/2026-10-13` など 5 種類）、1 つのフォルダ、元のフォルダの
//!   構成のまま、から選ぶ。
//! - **リネームの規則**（[`RenameRule`]）: テンプレート（`{date}-{seq}` など）の項目（[`TemplateToken`]）を、
//!   撮影日時・連番（開始番号・桁数）・元のファイル名・カメラ・任意の文字で埋める。テンプレートに直接書いた
//!   文字（`{{`・`}}` は波かっこそのもの）もそのまま使う。拡張子は元のファイルのもの（大文字・小文字も
//!   そのまま）。RAW と JPEG のペアは同じ名前にする（呼び出し側が 1 回だけ作る）。
//! - **OS で使えない名前**（[`safe_file_stem`]）: どの OS でも使える名前にする（コピー先のドライブを別の OS で
//!   使うことがあるため、Linux・macOS でも Windows の規則に合わせる）。禁止の文字（`< > : " / \ | ? *`）と
//!   制御文字は `_` に、先頭の `.`（Unix の隠しファイル。取り込みの走査も飛ばす）と前後の空白・末尾の `.` は
//!   除き、Windows の予約名（`CON`・`PRN`・`AUX`・`NUL`・`COM1`〜`COM9`・`LPT1`〜`LPT9` など。最初の `.` の前の
//!   部分で、大文字・小文字を区別しない）には `_` を足す。Unicode は NFC にし、長さは UTF-8 で
//!   [`MAX_STEM_BYTES`] バイトまで（文字の途中では切らない）。テンプレートに直接書いた文字と任意の文字に
//!   禁止の文字があればエラーにする（黙って変えない）。値（元のファイル名・カメラ）の禁止の文字は置き換える。
//! - **名前の衝突**: 同じ名前のファイルがあれば `-1`・`-2` …（[`with_suffix`]。書き出しの連番と同じ形）を
//!   付ける。比較は大文字・小文字と Unicode の正規化の違いを区別しない（カタログの比較キーと同じ）。衝突の
//!   判定は呼び出し側（`copy_import`）が行う。

use chrono::{Datelike, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::types::{CopySettings, NameExample, NamePreview};

/// テンプレートの文字数の上限（Unicode のスカラー値の数。**仮置き**: 200）。
pub const MAX_TEMPLATE_CHARS: usize = 200;

/// 任意の文字（`{text}`）の文字数の上限（**仮置き**: 100）。
pub const MAX_CUSTOM_TEXT_CHARS: usize = 100;

/// 連番の桁数の範囲（**仮置き**: 1〜9。足りない桁は 0 で埋め、超えた番号は桁を増やしてそのまま書く）。
pub const SEQUENCE_DIGITS_RANGE: std::ops::RangeInclusive<u8> = 1..=9;

/// 連番の開始番号の上限（**仮置き**: 999,999,999）。
pub const MAX_SEQUENCE_START: u32 = 999_999_999;

/// 作る名前（拡張子を除く部分）の長さの上限（UTF-8 のバイト数。**仮置き**: 200。多くのファイルシステムの
/// 名前の上限 255 バイト（NTFS は 255 文字の UTF-16）から、衝突を避ける `-9999` と拡張子の分を残す）。
pub const MAX_STEM_BYTES: usize = 200;

/// 衝突を避けて付ける番号の上限（`name-9999.ARW` まで。書き出しの連番の上限と同じ）。
pub const MAX_COLLISION_SUFFIX: u32 = genzo_media::safe_write::MAX_SEQUENCE_NUMBER;

/// リネームの規則の既定のテンプレート（撮影日と連番。`20261013-0001.ARW`）。
pub const DEFAULT_RENAME_TEMPLATE: &str = "{date}-{seq}";

/// ファイル名に使えない文字（Windows の禁止の文字。制御文字も使えない）。
pub const FORBIDDEN_NAME_CHARS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Windows の予約名（デバイス名。大文字・小文字を区別しない。上付きの数字の `COM¹` なども Windows 11 では
/// 予約されている）。
const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "com¹", "com²", "com³", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6",
    "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²", "lpt³", "conin$", "conout$",
];

// ---------------------------------------------------------------------------
// フォルダの規則（IMP-02）
// ---------------------------------------------------------------------------

/// コピー先のフォルダの規則（IMP-02）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderRule {
    /// 撮影日で分ける: `2026/2026-10-13`（既定。Lightroom Classic の既定の書式と同じ形）。
    #[default]
    YearDate,
    /// 撮影日で分ける: `2026/10/13`。
    YearMonthDay,
    /// 撮影月で分ける: `2026/2026-10`。
    YearMonth,
    /// 撮影日で分ける: `2026-10-13`。
    Date,
    /// 撮影日で分ける: `20261013`。
    DateCompact,
    /// 1 つのフォルダ（コピー先のフォルダそのもの）にまとめる。
    Single,
    /// 取り込むフォルダの中のフォルダの構成のまま（`DCIM/100MSDCF` など）。
    Source,
}

impl FolderRule {
    /// すべての規則（画面の並び）。
    pub const ALL: [Self; 7] = [
        Self::YearDate,
        Self::YearMonthDay,
        Self::YearMonth,
        Self::Date,
        Self::DateCompact,
        Self::Single,
        Self::Source,
    ];

    /// 名前（`year_date` など。JSON・CLI の値）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::YearDate => "year_date",
            Self::YearMonthDay => "year_month_day",
            Self::YearMonth => "year_month",
            Self::Date => "date",
            Self::DateCompact => "date_compact",
            Self::Single => "single",
            Self::Source => "source",
        }
    }

    /// 名前から（`-` と `_` を区別しない）。
    pub fn from_name(name: &str) -> Option<Self> {
        let n = name.trim().replace('-', "_");
        Self::ALL.into_iter().find(|r| r.as_str() == n)
    }

    /// 撮影日で分ける規則か。
    pub const fn uses_date(self) -> bool {
        !matches!(self, Self::Single | Self::Source)
    }

    /// コピー先のフォルダの中の相対パス（'/' 区切り。1 つのフォルダは空文字列）。`source_rel` は取り込む
    /// フォルダからの元のフォルダの相対パス（[`FolderRule::Source`] だけが使う。各段は [`safe_file_stem`]
    /// と同じ規則で整える）。
    pub fn folder_for(self, when: NaiveDateTime, source_rel: &str) -> String {
        let (y, m, d) = (when.year(), when.month(), when.day());
        match self {
            Self::YearDate => format!("{y:04}/{y:04}-{m:02}-{d:02}"),
            Self::YearMonthDay => format!("{y:04}/{m:02}/{d:02}"),
            Self::YearMonth => format!("{y:04}/{y:04}-{m:02}"),
            Self::Date => format!("{y:04}-{m:02}-{d:02}"),
            Self::DateCompact => format!("{y:04}{m:02}{d:02}"),
            Self::Single => String::new(),
            Self::Source => source_rel
                .split('/')
                .filter(|s| !s.is_empty())
                .map(safe_file_stem)
                .collect::<Vec<_>>()
                .join("/"),
        }
    }
}

// ---------------------------------------------------------------------------
// リネームの規則（IMP-03）
// ---------------------------------------------------------------------------

/// リネームの規則（IMP-03）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenameRule {
    /// テンプレート（`{date}-{seq}` など。項目は [`TemplateToken`]。`{{`・`}}` は波かっこそのもの）。
    pub template: String,
    /// 任意の文字（`{text}` に入れる。例: 撮影の名前）。
    pub custom_text: String,
    /// 連番の開始番号（`{seq}`）。
    pub sequence_start: u32,
    /// 連番の桁数（足りない桁は 0 で埋める）。
    pub sequence_digits: u8,
}

impl Default for RenameRule {
    fn default() -> Self {
        Self {
            template: DEFAULT_RENAME_TEMPLATE.to_owned(),
            custom_text: String::new(),
            sequence_start: 1,
            sequence_digits: 4,
        }
    }
}

/// テンプレートの項目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TemplateToken {
    /// `{name}`: 元のファイル名（拡張子を除く）。
    Name,
    /// `{date}`: 撮影日（`20261013`）。
    Date,
    /// `{time}`: 撮影時刻（`143052`）。
    Time,
    /// `{year}`: 年（`2026`）。
    Year,
    /// `{month}`: 月（`10`）。
    Month,
    /// `{day}`: 日（`13`）。
    Day,
    /// `{hour}`: 時（`14`。24 時間制）。
    Hour,
    /// `{minute}`: 分（`30`）。
    Minute,
    /// `{second}`: 秒（`52`）。
    Second,
    /// `{seq}`: 連番（開始番号・桁数は [`RenameRule`]。RAW と JPEG のペアは 1 つに数える）。
    Seq,
    /// `{camera}`: カメラの機種（`ILCE-7M4`。分からなければ空）。
    Camera,
    /// `{text}`: 任意の文字（[`RenameRule::custom_text`]）。
    Text,
}

impl TemplateToken {
    /// すべての項目（画面の並び）。
    pub const ALL: [Self; 12] = [
        Self::Date,
        Self::Time,
        Self::Year,
        Self::Month,
        Self::Day,
        Self::Hour,
        Self::Minute,
        Self::Second,
        Self::Seq,
        Self::Name,
        Self::Camera,
        Self::Text,
    ];

    /// テンプレートに書く名前（`{date}` の `date`）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Date => "date",
            Self::Time => "time",
            Self::Year => "year",
            Self::Month => "month",
            Self::Day => "day",
            Self::Hour => "hour",
            Self::Minute => "minute",
            Self::Second => "second",
            Self::Seq => "seq",
            Self::Camera => "camera",
            Self::Text => "text",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == name)
    }
}

/// テンプレートの部品。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePart {
    /// そのまま書く文字。
    Text(String),
    /// 項目。
    Token(TemplateToken),
}

/// テンプレートを部品に分ける（項目の名前・波かっこの対応・使えない文字・長さを確かめる）。
pub fn parse_template(template: &str) -> Result<Vec<TemplatePart>, ApiError> {
    let bad = |m: String| ApiError::InvalidArgument(format!("ファイル名のテンプレート: {m}"));
    if template.trim().is_empty() {
        return Err(bad("空です".to_owned()));
    }
    if template.chars().count() > MAX_TEMPLATE_CHARS {
        return Err(bad(format!("{MAX_TEMPLATE_CHARS} 文字までです")));
    }
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                text.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                text.push('}');
            }
            '{' => {
                let mut name = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    name.push(c);
                }
                if !closed {
                    return Err(bad(format!(
                        "「{{{name}」が閉じていません（波かっこそのものは {{{{ と書きます）"
                    )));
                }
                let token = TemplateToken::from_name(name.trim()).ok_or_else(|| {
                    bad(format!(
                        "「{{{name}}}」という項目はありません（使える項目: {}）",
                        TemplateToken::ALL
                            .iter()
                            .map(|t| format!("{{{}}}", t.as_str()))
                            .collect::<Vec<_>>()
                            .join(" ")
                    ))
                })?;
                if !text.is_empty() {
                    parts.push(TemplatePart::Text(std::mem::take(&mut text)));
                }
                parts.push(TemplatePart::Token(token));
            }
            '}' => {
                return Err(bad(
                    "「}」に対応する「{」がありません（波かっこそのものは }} と書きます）"
                        .to_owned(),
                ));
            }
            c => {
                check_literal_char(c).map_err(bad)?;
                text.push(c);
            }
        }
    }
    if !text.is_empty() {
        parts.push(TemplatePart::Text(text));
    }
    Ok(parts)
}

/// テンプレートに直接書いた文字・任意の文字に使えない文字があれば、その説明。
fn check_literal_char(c: char) -> Result<(), String> {
    if FORBIDDEN_NAME_CHARS.contains(&c) {
        Err(format!(
            "「{c}」はファイル名に使えません（使えない文字: {}）",
            FORBIDDEN_NAME_CHARS.iter().collect::<String>()
        ))
    } else if c.is_control() {
        Err("改行・タブなどの制御文字はファイル名に使えません".to_owned())
    } else {
        Ok(())
    }
}

impl RenameRule {
    /// 規則を確かめて、テンプレートの部品を返す（テンプレート・任意の文字・連番の設定）。
    pub fn parse(&self) -> Result<Vec<TemplatePart>, ApiError> {
        let parts = parse_template(&self.template)?;
        if self.custom_text.chars().count() > MAX_CUSTOM_TEXT_CHARS {
            return Err(ApiError::InvalidArgument(format!(
                "ファイル名の任意の文字は {MAX_CUSTOM_TEXT_CHARS} 文字までです"
            )));
        }
        for c in self.custom_text.chars() {
            check_literal_char(c)
                .map_err(|m| ApiError::InvalidArgument(format!("ファイル名の任意の文字: {m}")))?;
        }
        if !SEQUENCE_DIGITS_RANGE.contains(&self.sequence_digits) {
            return Err(ApiError::InvalidArgument(format!(
                "連番の桁数は {}〜{} です",
                SEQUENCE_DIGITS_RANGE.start(),
                SEQUENCE_DIGITS_RANGE.end()
            )));
        }
        if self.sequence_start > MAX_SEQUENCE_START {
            return Err(ApiError::InvalidArgument(format!(
                "連番の開始番号は 0〜{MAX_SEQUENCE_START} です"
            )));
        }
        Ok(parts)
    }
}

/// 名前を作る写真の情報。
#[derive(Debug, Clone, Copy)]
pub struct NameInput<'a> {
    /// 元のファイル名（拡張子を除く）。
    pub original_stem: &'a str,
    /// 撮影日時（現地の時刻。分からなければファイルの更新日時）。
    pub when: NaiveDateTime,
    /// カメラの機種。
    pub camera: Option<&'a str>,
    /// 連番（開始番号を足した後の値）。
    pub sequence: u64,
}

/// テンプレートの部品から、名前（拡張子を除く。[`safe_file_stem`] で整えたもの）を作る。結果が空になる
/// （値のない項目だけのテンプレート）ときは、元のファイル名にする。
pub fn render_stem(parts: &[TemplatePart], rule: &RenameRule, input: &NameInput<'_>) -> String {
    let w = input.when;
    let mut out = String::new();
    for p in parts {
        match p {
            TemplatePart::Text(t) => out.push_str(t),
            TemplatePart::Token(t) => {
                let v = match t {
                    TemplateToken::Name => input.original_stem.to_owned(),
                    TemplateToken::Date => {
                        format!("{:04}{:02}{:02}", w.year(), w.month(), w.day())
                    }
                    TemplateToken::Time => {
                        format!("{:02}{:02}{:02}", w.hour(), w.minute(), w.second())
                    }
                    TemplateToken::Year => format!("{:04}", w.year()),
                    TemplateToken::Month => format!("{:02}", w.month()),
                    TemplateToken::Day => format!("{:02}", w.day()),
                    TemplateToken::Hour => format!("{:02}", w.hour()),
                    TemplateToken::Minute => format!("{:02}", w.minute()),
                    TemplateToken::Second => format!("{:02}", w.second()),
                    TemplateToken::Seq => format!(
                        "{:0width$}",
                        input.sequence,
                        width = usize::from(rule.sequence_digits)
                    ),
                    TemplateToken::Camera => input.camera.unwrap_or_default().trim().to_owned(),
                    TemplateToken::Text => rule.custom_text.trim().to_owned(),
                };
                // 値の中の使えない文字は置き換える（元のファイル名・カメラの名前に含まれうる）。
                out.extend(v.chars().map(|c| {
                    if FORBIDDEN_NAME_CHARS.contains(&c) || c.is_control() {
                        '_'
                    } else {
                        c
                    }
                }));
            }
        }
    }
    if out.trim().trim_matches('.').is_empty() {
        out = input.original_stem.to_owned();
    }
    safe_file_stem(&out)
}

/// どの OS でも使える名前（拡張子を除く部分・フォルダの名前）にする（モジュールの doc）。
pub fn safe_file_stem(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if FORBIDDEN_NAME_CHARS.contains(&c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let normalized = genzo_catalog::text::nfc(&replaced);
    let mut s = trim_name(&normalized).to_owned();
    // 長さ（UTF-8 のバイト数。文字の途中では切らない）。
    if s.len() > MAX_STEM_BYTES {
        let mut end = MAX_STEM_BYTES;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s = trim_name(&s).to_owned();
    }
    if s.is_empty() {
        return "_".to_owned();
    }
    // Windows の予約名（最初の `.` の前の部分。末尾の空白は Windows が無視するので除いて比べる）。
    let first = s.split('.').next().unwrap_or_default();
    if WINDOWS_RESERVED
        .iter()
        .any(|r| first.trim_end().to_lowercase() == *r)
    {
        let rest = &s[first.len()..];
        s = format!("{}_{rest}", first.trim_end());
    }
    s
}

/// 前後の空白、先頭の `.`、末尾の `.` を除く（隠しファイルにしない。Windows は末尾の空白・`.` を許さない）。
fn trim_name(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace())
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' '])
        .trim_matches(|c: char| c.is_whitespace())
}

/// 名前（拡張子を除く）に衝突を避ける番号を付ける（`n` が 0 なら付けない。`name` → `name-1`）。
pub fn with_suffix(stem: &str, n: u32) -> String {
    if n == 0 {
        stem.to_owned()
    } else {
        format!("{stem}-{n}")
    }
}

/// 拡張子を除いた名前と拡張子に分ける（拡張子がなければ空文字列。`.` で始まる名前は拡張子なし）。
pub fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    }
}

/// 拡張子を付けた名前。
pub fn join_extension(stem: &str, ext: &str) -> String {
    if ext.is_empty() {
        stem.to_owned()
    } else {
        format!("{stem}.{ext}")
    }
}

// ---------------------------------------------------------------------------
// 名前の例（画面・CLI の確認用）
// ---------------------------------------------------------------------------

/// 名前の例の見本（ファイル名・撮影日時・カメラ・元のフォルダ・同じ写真の組の番号）。
const SAMPLES: &[(&str, &str, Option<&str>, &str, usize)] = &[
    (
        "DSC01234.ARW",
        "2026-10-13T14:30:52",
        Some("ILCE-7M4"),
        "DCIM/100MSDCF",
        0,
    ),
    (
        "DSC01234.JPG",
        "2026-10-13T14:30:52",
        Some("ILCE-7M4"),
        "DCIM/100MSDCF",
        0,
    ),
    (
        "DSC01235.ARW",
        "2026-10-13T14:30:53",
        Some("ILCE-7M4"),
        "DCIM/100MSDCF",
        1,
    ),
    (
        "DJI_0001.MP4",
        "2026-10-14T09:05:00",
        None,
        "DCIM/DJI_001",
        2,
    ),
];

/// コピーの設定の、名前の例（決まった見本の写真の、コピー先のフォルダからの相対パス）。設定が正しくなければ
/// `error` に理由（例は空）。カタログは使わない。
pub fn preview_names(settings: &CopySettings) -> NamePreview {
    let parts = match settings.rename.as_ref().map(RenameRule::parse).transpose() {
        Ok(p) => p,
        Err(e) => {
            return NamePreview {
                examples: Vec::new(),
                error: Some(e.to_string()),
            };
        }
    };
    let mut examples = Vec::new();
    for &(name, when, camera, source_rel, group) in SAMPLES {
        let when =
            NaiveDateTime::parse_from_str(when, "%Y-%m-%dT%H:%M:%S").expect("見本の日時は正しい");
        let (stem, ext) = split_extension(name);
        let new_stem = match (&settings.rename, &parts) {
            (Some(rule), Some(parts)) => render_stem(
                parts,
                rule,
                &NameInput {
                    original_stem: stem,
                    when,
                    camera,
                    sequence: u64::from(rule.sequence_start) + group as u64,
                },
            ),
            _ => safe_file_stem(stem),
        };
        let folder = settings.folder_rule.folder_for(when, source_rel);
        let file = join_extension(&new_stem, ext);
        examples.push(NameExample {
            source: format!("{source_rel}/{name}"),
            destination: if folder.is_empty() {
                file
            } else {
                format!("{folder}/{file}")
            },
            captured: when.format("%Y-%m-%d %H:%M:%S").to_string(),
        });
    }
    NamePreview {
        examples,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn when(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn render(template: &str, input: NameInput<'_>) -> String {
        let rule = RenameRule {
            template: template.to_owned(),
            custom_text: "京都".to_owned(),
            ..RenameRule::default()
        };
        render_stem(&rule.parse().unwrap(), &rule, &input)
    }

    fn input(seq: u64) -> NameInput<'static> {
        NameInput {
            original_stem: "DSC01234",
            when: when("2026-10-13 14:30:52"),
            camera: Some("ILCE-7M4"),
            sequence: seq,
        }
    }

    #[test]
    fn folder_rules_by_date_single_and_source() {
        let w = when("2026-01-05 07:08:09");
        let got: Vec<String> = FolderRule::ALL
            .iter()
            .map(|r| r.folder_for(w, "DCIM/100MSDCF"))
            .collect();
        assert_eq!(
            got,
            vec![
                "2026/2026-01-05",
                "2026/01/05",
                "2026/2026-01",
                "2026-01-05",
                "20260105",
                "",
                "DCIM/100MSDCF",
            ]
        );
        // 元のフォルダの名前も、どの OS でも使える形にする。
        assert_eq!(
            FolderRule::Source.folder_for(w, "a:b/ .hidden /c."),
            "a_b/hidden/c"
        );
        for r in FolderRule::ALL {
            assert_eq!(FolderRule::from_name(r.as_str()), Some(r));
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(json, format!("\"{}\"", r.as_str()));
        }
        assert_eq!(
            FolderRule::from_name("year-month-day"),
            Some(FolderRule::YearMonthDay)
        );
        assert_eq!(FolderRule::from_name("weekly"), None);
        assert!(FolderRule::YearDate.uses_date() && !FolderRule::Single.uses_date());
    }

    #[test]
    fn template_tokens_are_filled() {
        assert_eq!(render("{date}-{seq}", input(1)), "20261013-0001");
        assert_eq!(
            render("{year}{month}{day}_{hour}{minute}{second}", input(1)),
            "20261013_143052"
        );
        assert_eq!(render("{time}", input(1)), "143052");
        assert_eq!(
            render("{text}_{camera}_{name}", input(1)),
            "京都_ILCE-7M4_DSC01234"
        );
        // 桁を超えた番号はそのまま。
        assert_eq!(render("{seq}", input(123_456)), "123456");
        // 波かっこそのもの。
        assert_eq!(render("{{{seq}}}", input(7)), "{0007}");
        // 値のない項目だけなら、元のファイル名。
        let no_camera = NameInput {
            camera: None,
            ..input(1)
        };
        assert_eq!(render("{camera}", no_camera), "DSC01234");
        // 値の中の使えない文字は置き換える。
        let odd = NameInput {
            camera: Some("A/B:C"),
            ..input(1)
        };
        assert_eq!(render("{camera}", odd), "A_B_C");
        // 桁数。
        let rule = RenameRule {
            template: "{seq}".to_owned(),
            sequence_digits: 2,
            ..RenameRule::default()
        };
        assert_eq!(render_stem(&rule.parse().unwrap(), &rule, &input(3)), "03");
    }

    #[test]
    fn templates_with_errors_are_rejected() {
        for (t, needle) in [
            ("", "空"),
            ("   ", "空"),
            ("{date", "閉じていません"),
            ("date}", "対応する"),
            ("{unknown}", "unknown"),
            ("a/b{seq}", "「/」"),
            ("a:b", "「:」"),
            ("tab\there", "制御文字"),
        ] {
            let e = parse_template(t).unwrap_err().to_string();
            assert!(e.contains(needle), "{t:?}: {e}");
        }
        assert!(parse_template(&"a".repeat(MAX_TEMPLATE_CHARS + 1)).is_err());
        assert!(parse_template(&"a".repeat(MAX_TEMPLATE_CHARS)).is_ok());
        // 任意の文字・連番の設定。
        let base = RenameRule::default();
        assert!(
            RenameRule {
                custom_text: "a?b".to_owned(),
                ..base.clone()
            }
            .parse()
            .is_err()
        );
        assert!(
            RenameRule {
                custom_text: "あ".repeat(MAX_CUSTOM_TEXT_CHARS + 1),
                ..base.clone()
            }
            .parse()
            .is_err()
        );
        for digits in [0, 10] {
            assert!(
                RenameRule {
                    sequence_digits: digits,
                    ..base.clone()
                }
                .parse()
                .is_err()
            );
        }
        assert!(
            RenameRule {
                sequence_start: MAX_SEQUENCE_START + 1,
                ..base.clone()
            }
            .parse()
            .is_err()
        );
        assert!(base.parse().is_ok());
    }

    /// OS で使えない名前（Windows の予約名・末尾の点や空白・禁止の文字・先頭の点）と長さ・正規化。
    #[test]
    fn unsafe_names_are_made_portable() {
        assert_eq!(safe_file_stem("CON"), "CON_");
        assert_eq!(safe_file_stem("con.0001"), "con_.0001");
        assert_eq!(safe_file_stem("Lpt1 "), "Lpt1_");
        assert_eq!(safe_file_stem("COM¹"), "COM¹_");
        assert_eq!(safe_file_stem("CONSOLE"), "CONSOLE");
        assert_eq!(safe_file_stem("name. . "), "name");
        assert_eq!(safe_file_stem("..hidden"), "hidden");
        assert_eq!(safe_file_stem(" a<b>c|d?e*f\"g\\h "), "a_b_c_d_e_f_g_h");
        assert_eq!(safe_file_stem("..."), "_");
        assert_eq!(safe_file_stem(""), "_");
        // NFD（か + 濁点）は NFC（が）に。
        assert_eq!(safe_file_stem("\u{304b}\u{3099}"), "\u{304c}");
        // 長さは UTF-8 で MAX_STEM_BYTES バイトまで（文字の途中で切らない）。
        let long = "あ".repeat(100); // 300 バイト
        let s = safe_file_stem(&long);
        assert!(s.len() <= MAX_STEM_BYTES && s.len() > MAX_STEM_BYTES - 3);
        assert!(s.chars().all(|c| c == 'あ'));
        // 切った後の末尾の点も除く。
        let dotted = format!("{}.x", "a".repeat(MAX_STEM_BYTES - 1));
        assert_eq!(safe_file_stem(&dotted), "a".repeat(MAX_STEM_BYTES - 1));
    }

    #[test]
    fn suffixes_and_extensions() {
        assert_eq!(with_suffix("a", 0), "a");
        assert_eq!(with_suffix("a", 12), "a-12");
        assert_eq!(split_extension("DSC1.ARW"), ("DSC1", "ARW"));
        assert_eq!(split_extension("a.b.jpg"), ("a.b", "jpg"));
        assert_eq!(split_extension("noext"), ("noext", ""));
        assert_eq!(split_extension(".hidden"), (".hidden", ""));
        assert_eq!(split_extension("dot."), ("dot.", ""));
        assert_eq!(join_extension("a", "ARW"), "a.ARW");
        assert_eq!(join_extension("a", ""), "a");
    }

    /// 名前の例: 見本の RAW と JPEG のペアは同じ名前で、連番は 1 つに数える。
    #[test]
    fn previews_keep_pairs_together() {
        let settings = CopySettings {
            destination: "/dest".into(),
            folder_rule: FolderRule::YearDate,
            rename: Some(RenameRule::default()),
        };
        let p = preview_names(&settings);
        assert_eq!(p.error, None);
        let dest: Vec<&str> = p.examples.iter().map(|e| e.destination.as_str()).collect();
        assert_eq!(
            dest,
            vec![
                "2026/2026-10-13/20261013-0001.ARW",
                "2026/2026-10-13/20261013-0001.JPG",
                "2026/2026-10-13/20261013-0002.ARW",
                "2026/2026-10-14/20261014-0003.MP4",
            ]
        );
        assert_eq!(p.examples[0].source, "DCIM/100MSDCF/DSC01234.ARW");
        assert_eq!(p.examples[0].captured, "2026-10-13 14:30:52");
        // 元の名前のまま・1 つのフォルダ。
        let plain = preview_names(&CopySettings {
            rename: None,
            folder_rule: FolderRule::Single,
            ..settings.clone()
        });
        assert_eq!(plain.examples[3].destination, "DJI_0001.MP4");
        // 誤りのあるテンプレートは理由を返す。
        let bad = preview_names(&CopySettings {
            rename: Some(RenameRule {
                template: "{oops}".to_owned(),
                ..RenameRule::default()
            }),
            ..settings
        });
        assert!(bad.examples.is_empty());
        assert!(bad.error.unwrap().contains("oops"));
    }
}
