//! カタログの中で編集する説明のメタデータ（LIB-15。IPTC Core の主要な項目）。
//!
//! タイトル・キャプション・作成者と連絡先・著作権・撮影場所などを、**元のファイルには書かず**カタログに
//! 保存する（01 の LIB-15。決定事項ログ No.78）。書き出したファイルには XMP（UTF-8）と Exif（ASCII で
//! 表せる値だけ）で入れる（genzo-media の `xmp_write`・`exif_write`）。
//!
//! - 項目は [`MetadataField`] の順（画面の並び・CLI の名前・JSON のキー・カタログの列の名前は
//!   [`MetadataField::as_str`] で共通）。キャプションは MVP からの `asset.caption`、そのほかは
//!   `asset_iptc` の列（genzo-catalog）。どちらも asset（元ファイル）に付き、仮想コピーも同じ値。
//! - 値は [`normalize_metadata_value`] で整える（前後の空白を除き、制御文字を除き、1 行の項目では改行・
//!   タブを空白にする。空なら削除）。長さの上限は [`MAX_METADATA_CHARS`] 文字。
//! - 著作権の状態は、文字列の `copyrighted`・`public_domain`（[`CopyrightStatus`]）か、なし（不明）。

use serde::{Deserialize, Serialize};

/// 1 つの項目の値の最大の文字数（Unicode のスカラー値の数。**仮置き**: 2,000 文字）。
///
/// 書き出しの XMP は JPEG の APP1 の 1 つのセグメント（約 64 KiB）に入れるので、すべての項目が上限まで
/// 使われると入らないことがある（その場合の扱いは genzo-media の `xmp_write` の doc）。
pub const MAX_METADATA_CHARS: usize = 2000;

/// 説明のメタデータの項目（IPTC Core の主要な項目。決定事項ログ No.78）。
///
/// 並びは画面のパネルの並び（内容 → 作成者 → 著作権 → 撮影場所 → 状態）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataField {
    /// タイトル（XMP の `dc:title`。IPTC の Title）。
    Title,
    /// 見出し（`photoshop:Headline`）。
    Headline,
    /// キャプション（`dc:description`・Exif の ImageDescription。IPTC の Description）。
    Caption,
    /// 作成者（`dc:creator`・Exif の Artist）。
    Creator,
    /// 作成者の肩書き（`photoshop:AuthorsPosition`）。
    CreatorJobTitle,
    /// 作成者の住所（`Iptc4xmpCore:CiAdrExtadr`）。
    CreatorAddress,
    /// 作成者の市区町村（`CiAdrCity`）。
    CreatorCity,
    /// 作成者の都道府県・州（`CiAdrRegion`）。
    CreatorRegion,
    /// 作成者の郵便番号（`CiAdrPcode`）。
    CreatorPostalCode,
    /// 作成者の国（`CiAdrCtry`）。
    CreatorCountry,
    /// 作成者の電話（`CiTelWork`）。
    CreatorPhone,
    /// 作成者のメール（`CiEmailWork`）。
    CreatorEmail,
    /// 作成者の Web サイト（`CiUrlWork`）。
    CreatorUrl,
    /// 著作権表示（`dc:rights`・Exif の Copyright）。
    Copyright,
    /// 著作権の状態（`xmpRights:Marked`。[`CopyrightStatus`] の文字列。なしは不明）。
    CopyrightStatus,
    /// 使用条件（`xmpRights:UsageTerms`）。
    UsageTerms,
    /// 著作権情報の URL（`xmpRights:WebStatement`）。
    CopyrightUrl,
    /// 撮影場所の詳細（`Iptc4xmpCore:Location`。IPTC の Sublocation）。
    Sublocation,
    /// 撮影場所の市区町村（`photoshop:City`）。
    City,
    /// 撮影場所の都道府県・州（`photoshop:State`）。
    ProvinceState,
    /// 撮影場所の国（`photoshop:Country`）。
    Country,
    /// 撮影場所の国コード（`Iptc4xmpCore:CountryCode`。ISO 3166 の 2 文字か 3 文字を想定。確かめない）。
    CountryCode,
    /// クレジット（`photoshop:Credit`）。
    Credit,
    /// 提供元（`photoshop:Source`）。
    Source,
    /// 指示（`photoshop:Instructions`）。
    Instructions,
}

impl MetadataField {
    /// すべての項目（画面の並び）。
    pub const ALL: [Self; 25] = [
        Self::Title,
        Self::Headline,
        Self::Caption,
        Self::Creator,
        Self::CreatorJobTitle,
        Self::CreatorAddress,
        Self::CreatorCity,
        Self::CreatorRegion,
        Self::CreatorPostalCode,
        Self::CreatorCountry,
        Self::CreatorPhone,
        Self::CreatorEmail,
        Self::CreatorUrl,
        Self::Copyright,
        Self::CopyrightStatus,
        Self::UsageTerms,
        Self::CopyrightUrl,
        Self::Sublocation,
        Self::City,
        Self::ProvinceState,
        Self::Country,
        Self::CountryCode,
        Self::Credit,
        Self::Source,
        Self::Instructions,
    ];

    /// 名前（JSON のキー・CLI の名前・カタログの列の名前。snake_case）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Headline => "headline",
            Self::Caption => "caption",
            Self::Creator => "creator",
            Self::CreatorJobTitle => "creator_job_title",
            Self::CreatorAddress => "creator_address",
            Self::CreatorCity => "creator_city",
            Self::CreatorRegion => "creator_region",
            Self::CreatorPostalCode => "creator_postal_code",
            Self::CreatorCountry => "creator_country",
            Self::CreatorPhone => "creator_phone",
            Self::CreatorEmail => "creator_email",
            Self::CreatorUrl => "creator_url",
            Self::Copyright => "copyright",
            Self::CopyrightStatus => "copyright_status",
            Self::UsageTerms => "usage_terms",
            Self::CopyrightUrl => "copyright_url",
            Self::Sublocation => "sublocation",
            Self::City => "city",
            Self::ProvinceState => "province_state",
            Self::Country => "country",
            Self::CountryCode => "country_code",
            Self::Credit => "credit",
            Self::Source => "source",
            Self::Instructions => "instructions",
        }
    }

    /// 名前（[`as_str`](Self::as_str)）から項目を探す。
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.as_str() == name)
    }

    /// 表示名（CLI・エラーの説明用。項目だけで区別できる名前。画面はパネルの見出しの下に短い名前を出す）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Title => "タイトル",
            Self::Headline => "見出し",
            Self::Caption => "キャプション",
            Self::Creator => "作成者",
            Self::CreatorJobTitle => "作成者の肩書き",
            Self::CreatorAddress => "作成者の住所",
            Self::CreatorCity => "作成者の市区町村",
            Self::CreatorRegion => "作成者の都道府県・州",
            Self::CreatorPostalCode => "作成者の郵便番号",
            Self::CreatorCountry => "作成者の国",
            Self::CreatorPhone => "作成者の電話",
            Self::CreatorEmail => "作成者のメール",
            Self::CreatorUrl => "作成者の Web サイト",
            Self::Copyright => "著作権表示",
            Self::CopyrightStatus => "著作権の状態",
            Self::UsageTerms => "使用条件",
            Self::CopyrightUrl => "著作権情報の URL",
            Self::Sublocation => "撮影場所",
            Self::City => "撮影場所の市区町村",
            Self::ProvinceState => "撮影場所の都道府県・州",
            Self::Country => "撮影場所の国",
            Self::CountryCode => "撮影場所の国コード",
            Self::Credit => "クレジット",
            Self::Source => "提供元",
            Self::Instructions => "指示",
        }
    }

    /// 複数行を入れられる項目か（キャプション・住所・使用条件・指示）。それ以外は 1 行（改行は空白にする）。
    pub const fn multiline(self) -> bool {
        matches!(
            self,
            Self::Caption | Self::CreatorAddress | Self::UsageTerms | Self::Instructions
        )
    }

    /// テキスト検索（04 の 3.6 節）の対象か（タイトル・見出し・キャプション・作成者・著作権表示・撮影場所・
    /// クレジット・提供元。連絡先・URL・状態・使用条件・指示は対象にしない）。
    pub const fn searchable(self) -> bool {
        matches!(
            self,
            Self::Title
                | Self::Headline
                | Self::Caption
                | Self::Creator
                | Self::Copyright
                | Self::Sublocation
                | Self::City
                | Self::ProvinceState
                | Self::Country
                | Self::Credit
                | Self::Source
        )
    }
}

impl std::fmt::Display for MetadataField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 著作権の状態（`xmpRights:Marked`）。値がなければ不明（Marked を書かない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyrightStatus {
    /// 著作権あり（`xmpRights:Marked = True`）。
    Copyrighted,
    /// パブリックドメイン（`xmpRights:Marked = False`）。
    PublicDomain,
}

impl CopyrightStatus {
    /// 保存する文字列。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Copyrighted => "copyrighted",
            Self::PublicDomain => "public_domain",
        }
    }

    /// 文字列から（[`as_str`](Self::as_str) の値だけ）。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "copyrighted" => Some(Self::Copyrighted),
            "public_domain" => Some(Self::PublicDomain),
            _ => None,
        }
    }
}

/// 説明のメタデータ（[`MetadataField`] の各項目の値。なければ `None`）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IptcMetadata {
    /// タイトル。
    pub title: Option<String>,
    /// 見出し。
    pub headline: Option<String>,
    /// キャプション。
    pub caption: Option<String>,
    /// 作成者。
    pub creator: Option<String>,
    /// 作成者の肩書き。
    pub creator_job_title: Option<String>,
    /// 作成者の住所。
    pub creator_address: Option<String>,
    /// 作成者の市区町村。
    pub creator_city: Option<String>,
    /// 作成者の都道府県・州。
    pub creator_region: Option<String>,
    /// 作成者の郵便番号。
    pub creator_postal_code: Option<String>,
    /// 作成者の国。
    pub creator_country: Option<String>,
    /// 作成者の電話。
    pub creator_phone: Option<String>,
    /// 作成者のメール。
    pub creator_email: Option<String>,
    /// 作成者の Web サイト。
    pub creator_url: Option<String>,
    /// 著作権表示。
    pub copyright: Option<String>,
    /// 著作権の状態（`copyrighted`・`public_domain`。なしは不明）。
    pub copyright_status: Option<String>,
    /// 使用条件。
    pub usage_terms: Option<String>,
    /// 著作権情報の URL。
    pub copyright_url: Option<String>,
    /// 撮影場所の詳細。
    pub sublocation: Option<String>,
    /// 撮影場所の市区町村。
    pub city: Option<String>,
    /// 撮影場所の都道府県・州。
    pub province_state: Option<String>,
    /// 撮影場所の国。
    pub country: Option<String>,
    /// 撮影場所の国コード。
    pub country_code: Option<String>,
    /// クレジット。
    pub credit: Option<String>,
    /// 提供元。
    pub source: Option<String>,
    /// 指示。
    pub instructions: Option<String>,
}

impl IptcMetadata {
    fn slot(&self, field: MetadataField) -> &Option<String> {
        match field {
            MetadataField::Title => &self.title,
            MetadataField::Headline => &self.headline,
            MetadataField::Caption => &self.caption,
            MetadataField::Creator => &self.creator,
            MetadataField::CreatorJobTitle => &self.creator_job_title,
            MetadataField::CreatorAddress => &self.creator_address,
            MetadataField::CreatorCity => &self.creator_city,
            MetadataField::CreatorRegion => &self.creator_region,
            MetadataField::CreatorPostalCode => &self.creator_postal_code,
            MetadataField::CreatorCountry => &self.creator_country,
            MetadataField::CreatorPhone => &self.creator_phone,
            MetadataField::CreatorEmail => &self.creator_email,
            MetadataField::CreatorUrl => &self.creator_url,
            MetadataField::Copyright => &self.copyright,
            MetadataField::CopyrightStatus => &self.copyright_status,
            MetadataField::UsageTerms => &self.usage_terms,
            MetadataField::CopyrightUrl => &self.copyright_url,
            MetadataField::Sublocation => &self.sublocation,
            MetadataField::City => &self.city,
            MetadataField::ProvinceState => &self.province_state,
            MetadataField::Country => &self.country,
            MetadataField::CountryCode => &self.country_code,
            MetadataField::Credit => &self.credit,
            MetadataField::Source => &self.source,
            MetadataField::Instructions => &self.instructions,
        }
    }

    fn slot_mut(&mut self, field: MetadataField) -> &mut Option<String> {
        match field {
            MetadataField::Title => &mut self.title,
            MetadataField::Headline => &mut self.headline,
            MetadataField::Caption => &mut self.caption,
            MetadataField::Creator => &mut self.creator,
            MetadataField::CreatorJobTitle => &mut self.creator_job_title,
            MetadataField::CreatorAddress => &mut self.creator_address,
            MetadataField::CreatorCity => &mut self.creator_city,
            MetadataField::CreatorRegion => &mut self.creator_region,
            MetadataField::CreatorPostalCode => &mut self.creator_postal_code,
            MetadataField::CreatorCountry => &mut self.creator_country,
            MetadataField::CreatorPhone => &mut self.creator_phone,
            MetadataField::CreatorEmail => &mut self.creator_email,
            MetadataField::CreatorUrl => &mut self.creator_url,
            MetadataField::Copyright => &mut self.copyright,
            MetadataField::CopyrightStatus => &mut self.copyright_status,
            MetadataField::UsageTerms => &mut self.usage_terms,
            MetadataField::CopyrightUrl => &mut self.copyright_url,
            MetadataField::Sublocation => &mut self.sublocation,
            MetadataField::City => &mut self.city,
            MetadataField::ProvinceState => &mut self.province_state,
            MetadataField::Country => &mut self.country,
            MetadataField::CountryCode => &mut self.country_code,
            MetadataField::Credit => &mut self.credit,
            MetadataField::Source => &mut self.source,
            MetadataField::Instructions => &mut self.instructions,
        }
    }

    /// 項目の値。
    pub fn get(&self, field: MetadataField) -> Option<&str> {
        self.slot(field).as_deref()
    }

    /// 項目の値を置き換える（整えない。整えるのは [`normalize_metadata_value`]）。
    pub fn set(&mut self, field: MetadataField, value: Option<String>) {
        *self.slot_mut(field) = value;
    }

    /// どの項目にも値がないか。
    pub fn is_empty(&self) -> bool {
        MetadataField::ALL.iter().all(|&f| self.get(f).is_none())
    }

    /// 値のある項目と値（[`MetadataField::ALL`] の順）。
    pub fn entries(&self) -> impl Iterator<Item = (MetadataField, &str)> {
        MetadataField::ALL
            .into_iter()
            .filter_map(|f| self.get(f).map(|v| (f, v)))
    }

    /// 著作権の状態（値がない・解釈できなければ `None` = 不明）。
    pub fn copyright_status(&self) -> Option<CopyrightStatus> {
        self.copyright_status
            .as_deref()
            .and_then(CopyrightStatus::parse)
    }
}

/// 項目の値を整えたときのエラー。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MetadataValueError {
    /// 長すぎる。
    #[error("{label}が長すぎます（{chars} 文字。上限 {max} 文字）", label = field.label(), max = MAX_METADATA_CHARS)]
    TooLong {
        /// 項目。
        field: MetadataField,
        /// 整えた後の文字数。
        chars: usize,
    },
    /// 著作権の状態の値が不正。
    #[error(
        "著作権の状態は copyrighted（著作権あり）・public_domain（パブリックドメイン）・unknown（不明）のどれかにしてください（{0}）"
    )]
    InvalidCopyrightStatus(String),
}

/// 項目の値を保存する形に整える（`None`・空白だけなら `None` = 削除）。
///
/// - 前後の空白を除く。
/// - 改行は `\n` にそろえる（`\r\n`・`\r`）。1 行の項目（[`MetadataField::multiline`] が偽）では改行と
///   タブを空白にする。そのほかの制御文字（XML 1.0 に書けない文字を含む）は除く。
/// - 著作権の状態は `copyrighted`・`public_domain` だけ（`unknown` は `None`）。
/// - 上限は [`MAX_METADATA_CHARS`] 文字。
pub fn normalize_metadata_value(
    field: MetadataField,
    value: Option<&str>,
) -> Result<Option<String>, MetadataValueError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    let multiline = field.multiline();
    let cleaned: String = unified
        .chars()
        .filter_map(|c| match c {
            '\n' | '\t' if !multiline => Some(' '),
            '\n' | '\t' => Some(c),
            c if c.is_control() => None,
            // XML 1.0 に書けない非文字（U+FFFE・U+FFFF）も除く。
            '\u{FFFE}' | '\u{FFFF}' => None,
            c => Some(c),
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if field == MetadataField::CopyrightStatus {
        return match trimmed {
            "unknown" => Ok(None),
            s => CopyrightStatus::parse(s)
                .map(|st| Some(st.as_str().to_owned()))
                .ok_or_else(|| MetadataValueError::InvalidCopyrightStatus(s.to_owned())),
        };
    }
    let chars = trimmed.chars().count();
    if chars > MAX_METADATA_CHARS {
        return Err(MetadataValueError::TooLong { field, chars });
    }
    Ok(Some(trimmed.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_names_round_trip_and_match_serde() {
        let mut seen = std::collections::HashSet::new();
        let mut labels = std::collections::HashSet::new();
        for f in MetadataField::ALL {
            assert!(seen.insert(f.as_str()), "重複: {f}");
            assert!(labels.insert(f.label()), "表示名の重複: {}", f.label());
            assert_eq!(MetadataField::from_name(f.as_str()), Some(f));
            assert_eq!(
                serde_json::to_value(f).unwrap(),
                serde_json::Value::String(f.as_str().to_owned())
            );
            assert!(!f.label().is_empty());
        }
        assert_eq!(MetadataField::from_name("nope"), None);
        // 構造体の JSON のキーも項目の名前と同じ。
        let mut m = IptcMetadata::default();
        for f in MetadataField::ALL {
            m.set(f, Some(format!("v-{f}")));
        }
        let json = serde_json::to_value(&m).unwrap();
        for f in MetadataField::ALL {
            assert_eq!(json[f.as_str()], format!("v-{f}"), "{f}");
            assert_eq!(m.get(f), Some(format!("v-{f}").as_str()));
        }
        assert_eq!(m.entries().count(), MetadataField::ALL.len());
        assert!(!m.is_empty());
        assert!(IptcMetadata::default().is_empty());
    }

    #[test]
    fn values_are_trimmed_and_cleaned() {
        let n = |f, v: &str| normalize_metadata_value(f, Some(v)).unwrap();
        assert_eq!(
            n(MetadataField::Title, "  夕焼け  "),
            Some("夕焼け".to_owned())
        );
        assert_eq!(n(MetadataField::Title, " \u{3000}\t "), None);
        assert_eq!(
            normalize_metadata_value(MetadataField::Title, None),
            Ok(None)
        );
        // 1 行の項目は改行・タブを空白に、複数行の項目は改行を \n にそろえる。
        assert_eq!(
            n(MetadataField::Title, "a\r\nb\tc"),
            Some("a b c".to_owned())
        );
        assert_eq!(
            n(MetadataField::Caption, "一行目\r\n二行目\r三行目"),
            Some("一行目\n二行目\n三行目".to_owned())
        );
        // 制御文字（NUL・ESC など）と非文字は除く。
        assert_eq!(
            n(MetadataField::Creator, "山田\u{0}太郎\u{1b}\u{FFFF}"),
            Some("山田太郎".to_owned())
        );
    }

    #[test]
    fn copyright_status_accepts_known_values() {
        let n = |v: &str| normalize_metadata_value(MetadataField::CopyrightStatus, Some(v));
        assert_eq!(n("copyrighted"), Ok(Some("copyrighted".to_owned())));
        assert_eq!(n(" public_domain "), Ok(Some("public_domain".to_owned())));
        assert_eq!(n("unknown"), Ok(None));
        assert!(matches!(
            n("yes"),
            Err(MetadataValueError::InvalidCopyrightStatus(_))
        ));
        let mut m = IptcMetadata::default();
        assert_eq!(m.copyright_status(), None);
        m.copyright_status = Some("public_domain".to_owned());
        assert_eq!(m.copyright_status(), Some(CopyrightStatus::PublicDomain));
    }

    #[test]
    fn long_values_are_rejected() {
        let ok = "あ".repeat(MAX_METADATA_CHARS);
        assert_eq!(
            normalize_metadata_value(MetadataField::Caption, Some(&ok)),
            Ok(Some(ok.clone()))
        );
        let long = format!("{ok}い");
        let err = normalize_metadata_value(MetadataField::Caption, Some(&long)).unwrap_err();
        assert_eq!(
            err,
            MetadataValueError::TooLong {
                field: MetadataField::Caption,
                chars: MAX_METADATA_CHARS + 1
            }
        );
        assert!(err.to_string().contains("キャプション"), "{err}");
    }
}
