//! 撮影日時（docs/04_architecture.md の 3.1 節。レビュー R-18）。
//!
//! 元の日時の文字列、元のオフセットの有無、推定に使ったオフセットとその出どころ、
//! 時計のずれの補正値、補正後の UTC を別々に持つ。後から時計のずれやタイムゾーンを
//! 修正（LIB-16）したり、修正を取り消したりできるようにするため。
//!
//! - オフセットの情報がない写真・動画は、ユーザーが設定した既定のオフセットで UTC を
//!   推定し、`tz_source = user_default` と記録する。
//! - UTC の印（`Z`）だけが記録されている値（ffprobe の動画の `creation_time` など）は、UTC を
//!   その値のまま使い、`tz_source = utc_only`・元のオフセット（`offset`）を `"Z"`
//!   （[`UTC_ONLY_OFFSET`]）と記録する。撮影した地点のオフセットは分からないため、表示の
//!   オフセット（`tz_assumed`）には既定のオフセットを使う（implementation_status の No.32 の (a)）。
//!   `+00:00` と書かれた値は、オフセットが記録されたもの（`exif`）として扱う。
//! - 撮影日時がないファイルは、現在時刻などで置き換えず `utc = None` とする
//!   （撮影日時順の並べ替えでは最後に並べる）。
//!
//! - 利用者のタイムゾーンの指定（LIB-16。`tz_source = user_set`）には 2 つの読み方がある（[`ZoneMode`]）:
//!   記録の時刻（現地時刻）をその地域の時刻として読み直す（[`ZoneMode::KeepLocal`]。カメラの時計がその地域の
//!   時刻に合っていた。撮影した瞬間（UTC）が変わる）と、撮影した瞬間はそのままで、その地域の時刻で表示する
//!   （[`ZoneMode::KeepInstant`]。表示の時刻が変わる）。UTC の計算に使うオフセットが表示のオフセットと違う
//!   ときと、UTC の印（`Z`）だけの値を現地時刻として読むときは、計算に使うオフセットを `basis`（`tz_basis`）に
//!   持つ（カタログのスキーマの版 11。決定事項ログ No.102）。
//!
//! タイムゾーンは固定のオフセット（`+09:00` など）で扱う。夏時間のある地域の既定の
//! タイムゾーンを、日付ごとのオフセットに変換する処理（IANA のタイムゾーンの表が必要）は
//! まだない。レビュー R-18 の確認方法にある「夏時間の重複時刻」は、その処理を追加するときに
//! 扱う（それまでは、夏時間のある地域の写真はユーザーが [`CaptureTime::with_user_offset`] で
//! オフセットを指定する）。
//!
//! 補正後の UTC は 0〜9999 年の範囲に限る（DB に保存する固定長の文字列の並び順を保つため。
//! [`CaptureTime::utc_db_string`]）。範囲外になる日時・補正値は [`CaptureTimeError::OutOfRange`]。

use std::fmt;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDateTime, SecondsFormat, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::catalog::TzSource;

/// UTC の印（`Z`）だけが記録されていたことを表す、元のオフセット（[`CaptureTime::offset`]・
/// `captured_offset`）の値。
pub const UTC_ONLY_OFFSET: &str = "Z";

/// 利用者がタイムゾーンを指定するときの読み方（LIB-16。[`CaptureTime::with_zone`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneMode {
    /// 記録の時刻（現地時刻）はそのままで、指定したタイムゾーンの時刻として読み直す（カメラの時計がその
    /// 地域の時刻に合っていた場合。撮影した瞬間（UTC）が変わる）。UTC の印（`Z`）だけの値も現地時刻として読む。
    KeepLocal,
    /// 撮影した瞬間（UTC）はそのままで、指定したタイムゾーンの時刻で表示する（表示の時刻が変わる）。
    KeepInstant,
}

/// 撮影日時の解析・計算のエラー。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaptureTimeError {
    /// 日時の文字列を解析できない。
    #[error("撮影日時を解析できません: {0:?}")]
    InvalidDateTime(String),
    /// オフセットの文字列を解析できない。
    #[error("タイムゾーンのオフセットを解析できません: {0:?}")]
    InvalidOffset(String),
    /// 補正を加えた結果が表せる日時の範囲を超えた。
    #[error("撮影日時が表せる範囲を超えました")]
    OutOfRange,
}

/// 撮影日時（`asset` テーブルの `captured_at_raw` 〜 `captured_at_utc` に対応する）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureTime {
    /// 元の日時の文字列（`captured_at_raw`）。ファイルに日時がなければ `None`。
    pub raw: Option<String>,
    /// ファイルに記録されていたオフセット（`captured_offset`。`"+09:00"` 形式）。なければ `None`。
    /// UTC の印（`Z`）だけが記録されていた場合は [`UTC_ONLY_OFFSET`]（`"Z"`）。
    pub offset: Option<String>,
    /// UTC の推定に使ったオフセットの出どころ（`tz_source`）。
    /// 日時がない場合は、日時が分かったときに使う既定の扱い（`user_default`）にしておく。
    pub tz_source: TzSource,
    /// 表示のオフセット（`tz_assumed`。`"+09:00"` 形式）。ふつうは UTC の推定にも使う（[`basis`](Self::basis)
    /// がなければ）。日時がなければ `None`。
    pub tz_assumed: Option<String>,
    /// UTC の計算に使ったオフセット（`tz_basis`。`"+09:00"` 形式）。表示のオフセット（`tz_assumed`）と違う
    /// 場合（[`ZoneMode::KeepInstant`] で表示だけを変えた）と、UTC の印（`Z`）だけの値を現地時刻として読む
    /// 場合（[`ZoneMode::KeepLocal`]）だけ持つ。`None` なら `tz_assumed`（UTC の印だけの値は UTC）で計算する。
    #[serde(default)]
    pub basis: Option<String>,
    /// 時計のずれの補正（秒。`time_correction_s`）。UTC に加える。
    pub correction_s: i64,
    /// 補正後の UTC（`captured_at_utc`）。日時が不明なら `None`。
    pub utc: Option<DateTime<Utc>>,
}

impl CaptureTime {
    /// 日時が分からない撮影日時。
    pub fn unknown() -> Self {
        Self {
            raw: None,
            offset: None,
            tz_source: TzSource::UserDefault,
            tz_assumed: None,
            basis: None,
            correction_s: 0,
            utc: None,
        }
    }

    /// ファイルのメタデータの日時とオフセットから撮影日時を求める。
    ///
    /// - `raw`: EXIF 形式（`"2024:05:01 12:34:56"`）または ISO 8601
    ///   （`"2024-05-01T12:34:56"`、`"2024-05-01T12:34:56.123+09:00"`、`"...Z"` など）。
    ///   小数秒は任意。`None`・空文字列・EXIF の空の値（`"0000:00:00 00:00:00"` など）は
    ///   日時なしとして扱う（エラーにしない）。
    /// - `offset`: EXIF の OffsetTimeOriginal など（`"+09:00"`、`"+0900"`、`"Z"`）。
    ///   `raw` 自体にオフセットが含まれる場合は、そちらを優先する。
    /// - `default_offset`: オフセットがない場合に使う、ユーザー設定の既定のオフセット。
    ///   UTC の印（`Z`）だけの場合は、表示のオフセット（`tz_assumed`）にだけ使う（UTC は変えない。
    ///   `tz_source = utc_only`）。
    pub fn resolve(
        raw: Option<&str>,
        offset: Option<&str>,
        default_offset: FixedOffset,
    ) -> Result<Self, CaptureTimeError> {
        let Some(raw_str) = raw.filter(|s| !s.is_empty()) else {
            return Ok(Self::unknown());
        };
        let Some((local, embedded)) = parse_datetime_marked(raw_str)? else {
            return Ok(Self {
                raw: Some(raw_str.to_owned()),
                ..Self::unknown()
            });
        };
        // 文字列に含まれるオフセットを優先し、なければ別に渡されたオフセットを使う。
        let original = match (embedded, offset.map(str::trim).filter(|s| !s.is_empty())) {
            (Some(o), _) => Some(o),
            (None, Some(s)) => Some(parse_original_offset(s)?),
            (None, None) => None,
        };
        let (recorded, assumed, tz_source, base) = match original {
            Some(Original::Offset(o)) => (Some(format_offset(o)), o, TzSource::Exif, o),
            Some(Original::UtcOnly) => (
                Some(UTC_ONLY_OFFSET.to_owned()),
                default_offset,
                TzSource::UtcOnly,
                utc_offset(),
            ),
            None => (None, default_offset, TzSource::UserDefault, default_offset),
        };
        Ok(Self {
            raw: Some(raw_str.to_owned()),
            offset: recorded,
            tz_source,
            tz_assumed: Some(format_offset(assumed)),
            basis: None,
            correction_s: 0,
            utc: Some(to_utc(local, base, 0)?),
        })
    }

    /// ファイルに UTC の印（`Z`）だけが記録されていたか（元のオフセットが [`UTC_ONLY_OFFSET`]）。
    ///
    /// このとき UTC はファイルの値で決まり、`tz_assumed` は表示のオフセットにだけ使う
    /// （[`with_user_offset`](Self::with_user_offset) で指定しても UTC は変わらない）。
    pub fn records_utc_only(&self) -> bool {
        self.offset
            .as_deref()
            .is_some_and(|o| o.trim().eq_ignore_ascii_case(UTC_ONLY_OFFSET))
    }

    /// 既定のタイムゾーンを使っているか（`tz_source` が `user_default` か `utc_only`）。
    ///
    /// 既定のタイムゾーンを変えたときに [`with_default_offset`](Self::with_default_offset) で
    /// 推定し直す対象（`user_default` は UTC も、`utc_only` は表示のオフセットだけが変わる）。
    pub fn uses_default_offset(&self) -> bool {
        matches!(self.tz_source, TzSource::UserDefault | TzSource::UtcOnly)
    }

    /// [`resolve`](Self::resolve) と同じだが、失敗しない。取り込み用。
    ///
    /// 別に渡されたオフセットを解析できない場合だけ、既定のオフセットで推定し直す。
    /// 日時を解析できない場合と、範囲外になる場合は、元の文字列だけを残して `utc = None`
    /// にする（有効なオフセットを捨てて既定のオフセットで推定し直すことはしない）。
    pub fn resolve_lossy(
        raw: Option<&str>,
        offset: Option<&str>,
        default_offset: FixedOffset,
    ) -> Self {
        let unknown_with_raw = || Self {
            raw: raw.filter(|s| !s.is_empty()).map(str::to_owned),
            ..Self::unknown()
        };
        match Self::resolve(raw, offset, default_offset) {
            Ok(t) => t,
            Err(CaptureTimeError::InvalidOffset(_)) => {
                Self::resolve(raw, None, default_offset).unwrap_or_else(|_| unknown_with_raw())
            }
            Err(_) => unknown_with_raw(),
        }
    }

    /// 写真のメタデータの日時から撮影日時を求める（[`resolve_lossy`](Self::resolve_lossy)）。
    pub fn from_capture_info(info: &crate::CaptureInfo, default_offset: FixedOffset) -> Self {
        Self::resolve_lossy(
            info.datetime.as_deref(),
            info.offset.as_deref(),
            default_offset,
        )
    }

    /// ユーザーが指定したオフセットで推定し直す（LIB-16。`tz_source = user_set`）。
    ///
    /// UTC の印（`Z`）だけが記録されていた場合（[`records_utc_only`](Self::records_utc_only)）は
    /// [`ZoneMode::KeepInstant`]（UTC はそのままで表示のオフセットだけを変える。撮影した瞬間は分かっている
    /// ため）、それ以外は [`ZoneMode::KeepLocal`]（記録の現地時刻をそのオフセットで読み直す）。読み方を選ぶ
    /// ときは [`with_zone`](Self::with_zone)。日時がない場合は何もしない。
    pub fn with_user_offset(&self, offset: FixedOffset) -> Result<Self, CaptureTimeError> {
        let mode = if self.records_utc_only() {
            ZoneMode::KeepInstant
        } else {
            ZoneMode::KeepLocal
        };
        self.with_zone(offset, mode)
    }

    /// ユーザーがタイムゾーンを指定する（LIB-16。`tz_source = user_set`。読み方は [`ZoneMode`]）。
    ///
    /// - [`ZoneMode::KeepLocal`]: 記録の時刻（現地時刻）を `offset` の時刻として読み、UTC を計算し直す。UTC の
    ///   印（`Z`）だけの値も、その時刻を `offset` の現地時刻として読む（DJI などで現地時刻が UTC として
    ///   書かれていた場合）。
    /// - [`ZoneMode::KeepInstant`]: UTC（撮影した瞬間）はそのままで、表示のオフセットだけを `offset` にする
    ///   （UTC の計算に使うオフセットを `basis` に残す）。
    ///
    /// 時計のずれの補正値は保つ。日時がない場合は何もしない。
    pub fn with_zone(&self, offset: FixedOffset, mode: ZoneMode) -> Result<Self, CaptureTimeError> {
        if self.utc.is_none() {
            return Ok(self.clone());
        }
        let mut next = self.clone();
        next.tz_source = TzSource::UserSet;
        next.tz_assumed = Some(format_offset(offset));
        next.basis = match mode {
            ZoneMode::KeepLocal => self.records_utc_only().then(|| format_offset(offset)),
            ZoneMode::KeepInstant => {
                let base = self.base_offset()?;
                if self.records_utc_only() {
                    // None は UTC で計算する（UTC の印だけの値）ので、そのまま残す。
                    self.basis.clone()
                } else if base == offset {
                    None
                } else {
                    Some(format_offset(base))
                }
            }
        };
        next.recomputed()
    }

    /// ユーザーが指定したオフセット（[`with_user_offset`](Self::with_user_offset)）を取り消し、
    /// ファイルに記録されていたオフセット（なければ `default_offset`）で推定し直す。
    ///
    /// 時計のずれの補正値は保つ。日時がない場合は何もしない。
    pub fn without_user_offset(
        &self,
        default_offset: FixedOffset,
    ) -> Result<Self, CaptureTimeError> {
        if self.utc.is_none() {
            return Ok(self.clone());
        }
        let mut next = self.clone();
        next.basis = None;
        match self.offset.as_deref() {
            Some(_) if self.records_utc_only() => {
                next.tz_source = TzSource::UtcOnly;
                next.tz_assumed = Some(format_offset(default_offset));
            }
            Some(original) => {
                next.tz_source = TzSource::Exif;
                next.tz_assumed = Some(format_offset(parse_offset(original)?));
            }
            None => {
                next.tz_source = TzSource::UserDefault;
                next.tz_assumed = Some(format_offset(default_offset));
            }
        }
        next.recomputed()
    }

    /// ユーザー設定の既定のオフセットを変えたときに、既定のオフセットで推定していた
    /// もの（`tz_source = user_default`）だけ UTC を推定し直す。`tz_source = utc_only` は
    /// 表示のオフセットだけを変える（UTC は変えない）。
    pub fn with_default_offset(
        &self,
        default_offset: FixedOffset,
    ) -> Result<Self, CaptureTimeError> {
        if !self.uses_default_offset() || self.utc.is_none() {
            return Ok(self.clone());
        }
        let mut next = self.clone();
        next.tz_assumed = Some(format_offset(default_offset));
        next.recomputed()
    }

    /// 時計のずれの補正（秒）を設定して UTC を計算し直す。0 で補正の取り消し。
    pub fn with_correction(&self, correction_s: i64) -> Result<Self, CaptureTimeError> {
        let mut next = self.clone();
        next.correction_s = correction_s;
        if next.utc.is_none() {
            return Ok(next);
        }
        next.recomputed()
    }

    /// ユーザーの修正（タイムゾーンの指定・時計のずれの補正。LIB-16）をすべて取り消し、ファイルの値
    /// （オフセットがなければ `default_offset`）に戻す。日時がない場合は補正値だけを 0 にする。
    pub fn without_user_changes(
        &self,
        default_offset: FixedOffset,
    ) -> Result<Self, CaptureTimeError> {
        let base = if self.tz_source == TzSource::UserSet {
            self.without_user_offset(default_offset)?
        } else {
            self.clone()
        };
        base.with_correction(0)
    }

    /// ユーザーの修正（タイムゾーンの指定か、時計のずれの補正）があるか。
    pub fn has_user_changes(&self) -> bool {
        self.tz_source == TzSource::UserSet || self.correction_s != 0
    }

    /// ほかの撮影日時のユーザーの修正（タイムゾーンの指定とその読み方・時計のずれの補正）を、この撮影日時に
    /// 移す（内容の変わったファイルを読み直したとき）。指定のオフセットを読めない・範囲外になるときは、その
    /// 修正を移さない。
    pub fn with_user_changes_of(&self, previous: &CaptureTime) -> Self {
        let mut out = self.clone();
        if previous.tz_source == TzSource::UserSet
            && out.utc.is_some()
            && let Some(assumed) = previous
                .tz_assumed
                .as_deref()
                .and_then(|s| parse_offset(s).ok())
        {
            let mut next = out.clone();
            next.tz_source = TzSource::UserSet;
            next.tz_assumed = Some(format_offset(assumed));
            // UTC の印だけかどうかが前と変わったら、計算に使うオフセットは移さない（意味が変わるため）。
            next.basis = if previous.records_utc_only() == out.records_utc_only() {
                previous.basis.clone()
            } else {
                None
            };
            if let Ok(c) = next.recomputed() {
                out = c;
            }
        }
        if previous.correction_s != 0
            && let Ok(c) = out.with_correction(previous.correction_s)
        {
            out = c;
        }
        out
    }

    /// UTC の計算に使うオフセット（`basis`、なければ UTC の印だけの値は UTC、ほかは `tz_assumed`）。
    pub fn base_offset(&self) -> Result<FixedOffset, CaptureTimeError> {
        if let Some(b) = self.basis.as_deref() {
            return parse_offset(b);
        }
        if self.records_utc_only() {
            return Ok(utc_offset());
        }
        match self.tz_assumed.as_deref() {
            Some(a) => parse_offset(a),
            None => Err(CaptureTimeError::InvalidOffset(String::new())),
        }
    }

    /// 元の文字列・計算に使うオフセット（[`base_offset`](Self::base_offset)）・補正値から UTC を計算し直す。
    ///
    /// UTC の印（`Z`）だけが記録されていた場合は、`basis` がなければ `tz_assumed`（表示のオフセット）に
    /// よらず、元の文字列を UTC として読む。
    fn recomputed(mut self) -> Result<Self, CaptureTimeError> {
        let parsed = match self.raw.as_deref() {
            Some(raw) => parse_datetime(raw)?,
            None => None,
        };
        let (Some((local, _)), Some(_)) = (parsed, self.tz_assumed.as_deref()) else {
            self.utc = None;
            return Ok(self);
        };
        let base = self.base_offset()?;
        self.utc = Some(to_utc(local, base, self.correction_s)?);
        Ok(self)
    }

    /// 補正後の UTC を、推定に使ったオフセットの現地時刻で返す（表示用）。
    pub fn local(&self) -> Option<DateTime<FixedOffset>> {
        let utc = self.utc?;
        let offset = parse_offset(self.tz_assumed.as_deref()?).ok()?;
        Some(utc.with_timezone(&offset))
    }

    /// 補正後の UTC を、DB に保存する固定長の文字列（`"2024-05-01T03:04:05.000Z"`）で返す。
    ///
    /// 桁数が一定なので、文字列の比較で時刻順に並ぶ（UTC は 0〜9999 年に限っている）。
    pub fn utc_db_string(&self) -> Option<String> {
        self.utc
            .map(|t| t.to_rfc3339_opts(SecondsFormat::Millis, true))
    }
}

impl fmt::Display for CaptureTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.local() {
            Some(t) => write!(f, "{}", t.format("%Y-%m-%d %H:%M:%S %:z")),
            None => f.write_str("（撮影日時なし）"),
        }
    }
}

/// UTC（+00:00）のオフセット。
fn utc_offset() -> FixedOffset {
    FixedOffset::east_opt(0).expect("0 は有効なオフセット")
}

/// ファイルに記録されていた元のオフセット。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Original {
    /// `+09:00` などのオフセット（`+00:00` を含む）。
    Offset(FixedOffset),
    /// UTC の印（`Z`）だけ。撮影した地点のオフセットは分からない。
    UtcOnly,
}

impl Original {
    fn to_fixed(self) -> FixedOffset {
        match self {
            Self::Offset(o) => o,
            Self::UtcOnly => utc_offset(),
        }
    }
}

/// 別に渡された元のオフセット（EXIF の OffsetTimeOriginal など）を解析する。`Z` は UTC の印だけ。
fn parse_original_offset(s: &str) -> Result<Original, CaptureTimeError> {
    if s.trim().eq_ignore_ascii_case(UTC_ONLY_OFFSET) {
        Ok(Original::UtcOnly)
    } else {
        parse_offset(s).map(Original::Offset)
    }
}

/// 現地時刻とオフセットと補正値から UTC を求める。
fn to_utc(
    local: NaiveDateTime,
    offset: FixedOffset,
    correction_s: i64,
) -> Result<DateTime<Utc>, CaptureTimeError> {
    let utc_naive = local
        .checked_sub_signed(TimeDelta::seconds(i64::from(offset.local_minus_utc())))
        .ok_or(CaptureTimeError::OutOfRange)?;
    let delta = TimeDelta::try_seconds(correction_s).ok_or(CaptureTimeError::OutOfRange)?;
    let corrected = utc_naive
        .checked_add_signed(delta)
        .ok_or(CaptureTimeError::OutOfRange)?;
    // 年が 4 桁を超えると、DB の文字列の長さが変わり、並び順が崩れる。
    if !(0..=9999).contains(&corrected.year()) {
        return Err(CaptureTimeError::OutOfRange);
    }
    Ok(corrected.and_utc())
}

/// 日時なしとして扱う文字列か（空、または EXIF の空の値 `"0000:00:00 00:00:00"`・
/// 空白と区切り文字だけのもの）。
fn is_blank_datetime(s: &str) -> bool {
    s.chars()
        .all(|c| matches!(c, ' ' | ':' | '-' | '+' | '.' | '0' | 'T' | 'Z'))
}

/// 日時の文字列を、現地時刻と（あれば）文字列に含まれるオフセットに分けて解析する。
///
/// 日時なしとして扱う文字列なら `Ok(None)`。末尾の `Z`（UTC の印）は +00:00 のオフセットとして返す。
pub fn parse_datetime(
    s: &str,
) -> Result<Option<(NaiveDateTime, Option<FixedOffset>)>, CaptureTimeError> {
    Ok(parse_datetime_marked(s)?.map(|(local, o)| (local, o.map(Original::to_fixed))))
}

/// [`parse_datetime`] と同じだが、末尾の `Z`（UTC の印だけ）と `+00:00` を区別する。
fn parse_datetime_marked(
    s: &str,
) -> Result<Option<(NaiveDateTime, Option<Original>)>, CaptureTimeError> {
    let trimmed = s.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if is_blank_datetime(trimmed) {
        return Ok(None);
    }
    let invalid = || CaptureTimeError::InvalidDateTime(s.to_owned());
    // 末尾のオフセットを切り離す。日付の部分（先頭 10 文字）の '-' と区別するため、
    // 時刻の部分だけを探す。
    let (body, offset) = if let Some(body) = trimmed.strip_suffix(['Z', 'z']) {
        (body, Some(Original::UtcOnly))
    } else {
        let time_start = trimmed
            .char_indices()
            .nth(11)
            .map(|(i, _)| i)
            .ok_or_else(invalid)?;
        match trimmed[time_start..].find(['+', '-']) {
            Some(pos) => {
                let split = time_start + pos;
                let offset = parse_offset(&trimmed[split..]).map_err(|_| invalid())?;
                (&trimmed[..split], Some(Original::Offset(offset)))
            }
            None => (trimmed, None),
        }
    };
    const FORMATS: [&str; 5] = [
        "%Y:%m:%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ];
    let body = body.trim_end();
    FORMATS
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(body, f).ok())
        .map(|local| Some((local, offset)))
        .ok_or_else(invalid)
}

/// オフセットの文字列（`"+09:00"`、`"+0900"`、`"+09"`、`"Z"`）を解析する。
pub fn parse_offset(s: &str) -> Result<FixedOffset, CaptureTimeError> {
    let invalid = || CaptureTimeError::InvalidOffset(s.to_owned());
    let t = s.trim();
    if t.eq_ignore_ascii_case("z") {
        return Ok(FixedOffset::east_opt(0).expect("0 は有効なオフセット"));
    }
    let (sign, rest) = match t.as_bytes().first() {
        Some(b'+') => (1, &t[1..]),
        Some(b'-') => (-1, &t[1..]),
        _ => return Err(invalid()),
    };
    let digits: String = rest.chars().filter(|&c| c != ':').collect();
    let colons = rest.chars().filter(|&c| c == ':').count();
    let well_formed = match digits.len() {
        2 => colons == 0,
        4 => colons == 0 || (colons == 1 && rest.as_bytes().get(2) == Some(&b':')),
        _ => false,
    };
    if !well_formed || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let hours: i32 = digits[..2].parse().map_err(|_| invalid())?;
    let minutes: i32 = if digits.len() == 4 {
        digits[2..].parse().map_err(|_| invalid())?
    } else {
        0
    };
    // 実在するオフセットは -12:00〜+14:00 だが、記録の誤りも読めるよう ±18:00 まで許す。
    if minutes >= 60 || hours * 60 + minutes > 18 * 60 {
        return Err(invalid());
    }
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60)).ok_or_else(invalid)
}

/// オフセットを `"+09:00"` 形式にする（秒の端数は切り捨てる）。
pub fn format_offset(offset: FixedOffset) -> String {
    let total = offset.local_minus_utc();
    let sign = if total < 0 { '-' } else { '+' };
    let minutes = total.unsigned_abs() / 60;
    format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn jst() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).unwrap()
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn exif_datetime_with_offset_uses_exif_offset() {
        let t = CaptureTime::resolve(Some("2024:05:01 12:34:56"), Some("+02:00"), jst()).unwrap();
        assert_eq!(t.raw.as_deref(), Some("2024:05:01 12:34:56"));
        assert_eq!(t.offset.as_deref(), Some("+02:00"));
        assert_eq!(t.tz_source, TzSource::Exif);
        assert_eq!(t.tz_assumed.as_deref(), Some("+02:00"));
        assert_eq!(t.utc, Some(utc(2024, 5, 1, 10, 34, 56)));
    }

    #[test]
    fn exif_datetime_without_offset_uses_user_default() {
        let t = CaptureTime::resolve(Some("2024:05:01 12:34:56"), None, jst()).unwrap();
        assert_eq!(t.offset, None);
        assert_eq!(t.tz_source, TzSource::UserDefault);
        assert_eq!(t.tz_assumed.as_deref(), Some("+09:00"));
        assert_eq!(t.utc, Some(utc(2024, 5, 1, 3, 34, 56)));
        assert_eq!(
            t.utc_db_string().as_deref(),
            Some("2024-05-01T03:34:56.000Z")
        );
        assert_eq!(t.to_string(), "2024-05-01 12:34:56 +09:00");
    }

    #[test]
    fn iso8601_forms_are_accepted() {
        // オフセット付き（文字列のオフセットが、別に渡したオフセットより優先される）。
        let t = CaptureTime::resolve(
            Some("2024-05-01T12:34:56.250+09:00"),
            Some("+01:00"),
            FixedOffset::east_opt(0).unwrap(),
        )
        .unwrap();
        assert_eq!(t.tz_source, TzSource::Exif);
        assert_eq!(t.offset.as_deref(), Some("+09:00"));
        assert_eq!(
            t.utc_db_string().as_deref(),
            Some("2024-05-01T03:34:56.250Z")
        );

        // ffprobe の creation_time の形式（UTC の印だけ。No.32）。UTC はそのまま、表示は既定のオフセット。
        let t = CaptureTime::resolve(Some("2024-05-01T03:34:56.000000Z"), None, jst()).unwrap();
        assert_eq!(t.offset.as_deref(), Some(UTC_ONLY_OFFSET));
        assert_eq!(t.tz_source, TzSource::UtcOnly);
        assert_eq!(t.tz_assumed.as_deref(), Some("+09:00"));
        assert_eq!(t.utc, Some(utc(2024, 5, 1, 3, 34, 56)));
        assert_eq!(t.to_string(), "2024-05-01 12:34:56 +09:00");

        // +00:00 と書かれた値は、オフセットが記録されたもの。
        let t = CaptureTime::resolve(Some("2024-05-01T03:34:56+00:00"), None, jst()).unwrap();
        assert_eq!(t.offset.as_deref(), Some("+00:00"));
        assert_eq!(t.tz_source, TzSource::Exif);
        assert_eq!(t.utc, Some(utc(2024, 5, 1, 3, 34, 56)));

        // オフセットなし、区切りが空白、負のオフセット（コロンなし）。
        let t = CaptureTime::resolve(Some("2024-05-01 12:34:56"), None, jst()).unwrap();
        assert_eq!(t.utc, Some(utc(2024, 5, 1, 3, 34, 56)));
        let t = CaptureTime::resolve(Some("2024-05-01T12:34:56-0530"), None, jst()).unwrap();
        assert_eq!(t.offset.as_deref(), Some("-05:30"));
        assert_eq!(t.utc, Some(utc(2024, 5, 1, 18, 4, 56)));

        // 秒なし。
        let t = CaptureTime::resolve(Some("2024-05-01T12:34"), None, jst()).unwrap();
        assert_eq!(t.utc, Some(utc(2024, 5, 1, 3, 34, 0)));
    }

    #[test]
    fn missing_datetime_is_not_replaced_with_now() {
        for raw in [
            None,
            Some(""),
            Some("0000:00:00 00:00:00"),
            Some("    :  :     :  :  "),
            Some("0000-00-00T00:00:00.000000Z"),
        ] {
            let t = CaptureTime::resolve(raw, Some("+09:00"), jst()).unwrap();
            assert_eq!(t.utc, None, "{raw:?}");
            assert_eq!(t.tz_assumed, None);
            assert_eq!(t.offset, None);
            assert_eq!(t.utc_db_string(), None);
            assert_eq!(t.local(), None);
        }
        // EXIF の空の値は、元の文字列として残す。
        let t = CaptureTime::resolve(Some("0000:00:00 00:00:00"), None, jst()).unwrap();
        assert_eq!(t.raw.as_deref(), Some("0000:00:00 00:00:00"));
        assert_eq!(CaptureTime::unknown().to_string(), "（撮影日時なし）");
    }

    #[test]
    fn invalid_inputs_are_errors_in_strict_mode() {
        assert!(matches!(
            CaptureTime::resolve(Some("yesterday"), None, jst()),
            Err(CaptureTimeError::InvalidDateTime(_))
        ));
        assert!(matches!(
            CaptureTime::resolve(Some("2024:13:01 00:00:00"), None, jst()),
            Err(CaptureTimeError::InvalidDateTime(_))
        ));
        assert!(matches!(
            CaptureTime::resolve(Some("2024:05:01 12:34:56"), Some("JST"), jst()),
            Err(CaptureTimeError::InvalidOffset(_))
        ));
    }

    #[test]
    fn lossy_mode_never_fails() {
        // オフセットが壊れていれば既定のオフセットを使う。
        let t = CaptureTime::resolve_lossy(Some("2024:05:01 12:34:56"), Some("JST"), jst());
        assert_eq!(t.tz_source, TzSource::UserDefault);
        assert_eq!(t.utc, Some(utc(2024, 5, 1, 3, 34, 56)));
        // 日時が壊れていれば、元の文字列だけを残す。
        let t = CaptureTime::resolve_lossy(Some("garbage"), None, jst());
        assert_eq!(t.raw.as_deref(), Some("garbage"));
        assert_eq!(t.utc, None);
    }

    #[test]
    fn from_capture_info() {
        let info = crate::CaptureInfo {
            datetime: Some("2023:12:31 23:30:00".to_owned()),
            offset: Some("+09:00".to_owned()),
        };
        let t = CaptureTime::from_capture_info(&info, FixedOffset::east_opt(0).unwrap());
        assert_eq!(t.utc, Some(utc(2023, 12, 31, 14, 30, 0)));
        assert_eq!(t.tz_source, TzSource::Exif);
    }

    #[test]
    fn user_corrections_can_be_applied_and_undone() {
        let t = CaptureTime::resolve(Some("2024:05:01 12:00:00"), None, jst()).unwrap();
        // カメラの時計が 90 秒遅れていた。
        let fixed = t.with_correction(90).unwrap();
        assert_eq!(fixed.correction_s, 90);
        assert_eq!(fixed.utc, Some(utc(2024, 5, 1, 3, 1, 30)));
        assert_eq!(fixed.raw, t.raw);
        // 補正の取り消し。
        assert_eq!(fixed.with_correction(0).unwrap(), t);

        // タイムゾーンの指定（海外で撮影）。補正値は保つ。
        let paris = FixedOffset::east_opt(2 * 3600).unwrap();
        let set = fixed.with_user_offset(paris).unwrap();
        assert_eq!(set.tz_source, TzSource::UserSet);
        assert_eq!(set.tz_assumed.as_deref(), Some("+02:00"));
        assert_eq!(set.utc, Some(utc(2024, 5, 1, 10, 1, 30)));
        assert_eq!(
            set.local().unwrap().to_rfc3339(),
            "2024-05-01T12:01:30+02:00"
        );
    }

    #[test]
    fn changing_the_default_offset_affects_only_user_default() {
        let utc0 = FixedOffset::east_opt(0).unwrap();
        let default = CaptureTime::resolve(Some("2024:05:01 12:00:00"), None, jst()).unwrap();
        let moved = default.with_default_offset(utc0).unwrap();
        assert_eq!(moved.utc, Some(utc(2024, 5, 1, 12, 0, 0)));
        assert_eq!(moved.tz_source, TzSource::UserDefault);

        let exif =
            CaptureTime::resolve(Some("2024:05:01 12:00:00"), Some("+09:00"), jst()).unwrap();
        assert_eq!(exif.with_default_offset(utc0).unwrap(), exif);
    }

    #[test]
    fn corrections_on_unknown_dates_keep_utc_none() {
        let t = CaptureTime::unknown();
        assert_eq!(t.with_correction(60).unwrap().utc, None);
        assert_eq!(t.with_user_offset(jst()).unwrap(), t);
    }

    #[test]
    fn huge_correction_is_out_of_range() {
        let t = CaptureTime::resolve(Some("2024:05:01 12:00:00"), None, jst()).unwrap();
        assert_eq!(
            t.with_correction(i64::MAX),
            Err(CaptureTimeError::OutOfRange)
        );
        // 1 万年を超える補正（DB の文字列が固定長でなくなる）。
        assert_eq!(
            t.with_correction(8000 * 366 * 86_400),
            Err(CaptureTimeError::OutOfRange)
        );
    }

    #[test]
    fn utc_is_limited_to_four_digit_years() {
        // 9999-12-31 の現地時刻を負のオフセットで UTC にすると 10000 年になる。
        assert_eq!(
            CaptureTime::resolve(Some("9999:12:31 23:00:00"), Some("-05:00"), jst()),
            Err(CaptureTimeError::OutOfRange)
        );
        let t = CaptureTime::resolve(Some("9999:12:31 23:00:00"), Some("+09:00"), jst()).unwrap();
        assert_eq!(
            t.utc_db_string().as_deref(),
            Some("9999-12-31T14:00:00.000Z")
        );
        // 取り込み用は失敗しない（日時は不明として扱う）。
        let t = CaptureTime::resolve_lossy(Some("9999:12:31 23:00:00"), Some("-05:00"), jst());
        assert_eq!(t.utc, None);
        assert_eq!(t.raw.as_deref(), Some("9999:12:31 23:00:00"));
    }

    #[test]
    fn db_strings_sort_in_time_order() {
        let a = CaptureTime::resolve(Some("0999:01:01 00:00:00"), Some("+00:00"), jst()).unwrap();
        let b = CaptureTime::resolve(Some("2024:05:01 12:00:00.5"), None, jst()).unwrap();
        let c = CaptureTime::resolve(Some("2024:05:01 12:00:01"), None, jst()).unwrap();
        let (a, b, c) = (
            a.utc_db_string().unwrap(),
            b.utc_db_string().unwrap(),
            c.utc_db_string().unwrap(),
        );
        assert_eq!(a.len(), b.len());
        assert!(a < b && b < c, "{a} {b} {c}");
    }

    #[test]
    fn user_offset_can_be_undone() {
        let paris = FixedOffset::east_opt(2 * 3600).unwrap();
        // ファイルにオフセットがあった場合は、それに戻す。
        let exif = CaptureTime::resolve(Some("2024:05:01 12:00:00"), Some("+09:00"), jst())
            .unwrap()
            .with_correction(30)
            .unwrap();
        let set = exif.with_user_offset(paris).unwrap();
        assert_ne!(set.utc, exif.utc);
        assert_eq!(set.without_user_offset(jst()).unwrap(), exif);
        // なかった場合は、既定のオフセットに戻す。
        let default = CaptureTime::resolve(Some("2024:05:01 12:00:00"), None, jst()).unwrap();
        let set = default.with_user_offset(paris).unwrap();
        assert_eq!(set.without_user_offset(jst()).unwrap(), default);
        // 日時がなければ何もしない。
        let unknown = CaptureTime::unknown();
        assert_eq!(unknown.without_user_offset(jst()).unwrap(), unknown);
    }

    #[test]
    fn offsets_parse_and_format() {
        for (s, secs) in [
            ("+09:00", 9 * 3600),
            ("+0900", 9 * 3600),
            ("+09", 9 * 3600),
            ("-05:30", -(5 * 3600 + 30 * 60)),
            ("Z", 0),
            (" +00:00 ", 0),
        ] {
            assert_eq!(parse_offset(s).unwrap().local_minus_utc(), secs, "{s}");
        }
        for s in [
            "", "09:00", "+9:00", "+09:60", "+19:00", "+09:0", "+0:900", "JST", "+09::00",
        ] {
            assert!(parse_offset(s).is_err(), "{s:?}");
        }
        assert_eq!(format_offset(jst()), "+09:00");
        assert_eq!(
            format_offset(FixedOffset::west_opt(3600 + 1800).unwrap()),
            "-01:30"
        );
        assert_eq!(format_offset(FixedOffset::east_opt(0).unwrap()), "+00:00");
    }

    #[test]
    fn utc_only_keeps_utc_and_uses_the_default_offset_for_display() {
        let utc0 = FixedOffset::east_opt(0).unwrap();
        let paris = FixedOffset::east_opt(2 * 3600).unwrap();
        let t = CaptureTime::resolve(Some("2024-05-01T03:00:00Z"), None, jst()).unwrap();
        assert!(t.records_utc_only());
        assert!(t.uses_default_offset());
        let instant = utc(2024, 5, 1, 3, 0, 0);

        // 既定のタイムゾーンを変えると、表示のオフセットだけが変わる（UTC は変えない）。
        let moved = t.with_default_offset(paris).unwrap();
        assert_eq!(moved.tz_source, TzSource::UtcOnly);
        assert_eq!(moved.tz_assumed.as_deref(), Some("+02:00"));
        assert_eq!(moved.utc, Some(instant));
        assert_eq!(moved.to_string(), "2024-05-01 05:00:00 +02:00");

        // 利用者がタイムゾーンを指定しても、撮影した瞬間（UTC）は変えない。
        let set = t.with_user_offset(paris).unwrap();
        assert_eq!(set.tz_source, TzSource::UserSet);
        assert_eq!(set.utc, Some(instant));
        assert_eq!(set.to_string(), "2024-05-01 05:00:00 +02:00");
        // 既定のタイムゾーンを変えても、指定したものは変わらない。
        assert_eq!(set.with_default_offset(utc0).unwrap(), set);
        // 指定の取り消しで utc_only（既定のオフセットで表示）に戻る。
        assert_eq!(set.without_user_offset(jst()).unwrap(), t);

        // 時計のずれの補正は UTC に加える（カメラが現地時刻を UTC として書いていた場合など）。
        let fixed = t.with_correction(-9 * 3600).unwrap();
        assert_eq!(fixed.utc, Some(utc(2024, 4, 30, 18, 0, 0)));
        assert_eq!(fixed.with_correction(0).unwrap(), t);
        assert_eq!(
            fixed.with_user_offset(paris).unwrap().utc,
            fixed.utc,
            "補正後の UTC を保つ"
        );

        // 別に渡されたオフセットが Z だけの場合も同じ扱い。
        let t2 = CaptureTime::resolve(Some("2024:05:01 03:00:00"), Some("Z"), jst()).unwrap();
        assert_eq!(t2.tz_source, TzSource::UtcOnly);
        assert_eq!(t2.offset.as_deref(), Some(UTC_ONLY_OFFSET));
        assert_eq!(t2.utc, Some(instant));
        // 取り込み用（失敗しない）でも同じ。
        let t3 = CaptureTime::resolve_lossy(Some("2024-05-01T03:00:00.000000Z"), None, jst());
        assert_eq!(t3.tz_source, TzSource::UtcOnly);
        assert_eq!(t3.utc, Some(instant));

        // 公開の parse_datetime は、Z を +00:00 として返す（従来どおり）。
        let (_, o) = parse_datetime("2024-05-01T03:00:00Z").unwrap().unwrap();
        assert_eq!(o, Some(utc0));
    }

    #[test]
    fn exif_records_do_not_use_the_default_offset() {
        let t = CaptureTime::resolve(Some("2024:05:01 12:00:00"), Some("+09:00"), jst()).unwrap();
        assert!(!t.records_utc_only());
        assert!(!t.uses_default_offset());
        let d = CaptureTime::resolve(Some("2024:05:01 12:00:00"), None, jst()).unwrap();
        assert!(d.uses_default_offset());
        assert!(!d.records_utc_only());
    }

    #[test]
    fn zone_modes_keep_the_local_time_or_the_instant() {
        let paris = FixedOffset::east_opt(2 * 3600).unwrap();
        // カメラの時計を東京の時刻のまま、オフセット +09:00 を記録していた（パリで撮影）。
        let t = CaptureTime::resolve(Some("2024:05:01 12:00:00"), Some("+09:00"), jst()).unwrap();
        let instant = utc(2024, 5, 1, 3, 0, 0);
        assert_eq!(t.utc, Some(instant));

        // 撮影した瞬間はそのままで、パリの時刻で表示する。
        let shown = t.with_zone(paris, ZoneMode::KeepInstant).unwrap();
        assert_eq!(shown.tz_source, TzSource::UserSet);
        assert_eq!(shown.utc, Some(instant));
        assert_eq!(shown.basis.as_deref(), Some("+09:00"));
        assert_eq!(shown.to_string(), "2024-05-01 05:00:00 +02:00");
        // 続けて補正しても、計算に使うオフセットは変わらない。
        let fixed = shown.with_correction(60).unwrap();
        assert_eq!(fixed.utc, Some(utc(2024, 5, 1, 3, 1, 0)));
        // 同じ指定を繰り返しても変わらない。
        assert_eq!(
            shown.with_zone(paris, ZoneMode::KeepInstant).unwrap(),
            shown
        );
        // 取り消すと、記録のオフセットに戻る。
        assert_eq!(shown.without_user_offset(jst()).unwrap(), t);

        // 記録の時刻（12:00）をパリの時刻として読み直す（カメラの時計がパリの時刻だった）。
        let local = t.with_zone(paris, ZoneMode::KeepLocal).unwrap();
        assert_eq!(local.basis, None);
        assert_eq!(local.utc, Some(utc(2024, 5, 1, 10, 0, 0)));
        assert_eq!(local.to_string(), "2024-05-01 12:00:00 +02:00");
        // 表示だけを変えた後に読み直しても、計算に使うオフセットは指定したものになる。
        assert_eq!(shown.with_zone(paris, ZoneMode::KeepLocal).unwrap(), local);
        // 表示のオフセットが計算に使うオフセットと同じになれば、basis は持たない。
        let back = shown.with_zone(jst(), ZoneMode::KeepInstant).unwrap();
        assert_eq!(back.basis, None);
        assert_eq!(back.utc, Some(instant));
    }

    #[test]
    fn utc_only_values_can_be_read_as_local_time() {
        // DJI などで、現地時刻（12:00 +09:00）が UTC の印付きで書かれていた。
        let t = CaptureTime::resolve(Some("2024-05-01T12:00:00Z"), None, jst()).unwrap();
        assert_eq!(t.to_string(), "2024-05-01 21:00:00 +09:00");
        let fixed = t.with_zone(jst(), ZoneMode::KeepLocal).unwrap();
        assert_eq!(fixed.tz_source, TzSource::UserSet);
        assert_eq!(fixed.basis.as_deref(), Some("+09:00"));
        assert_eq!(fixed.utc, Some(utc(2024, 5, 1, 3, 0, 0)));
        assert_eq!(fixed.to_string(), "2024-05-01 12:00:00 +09:00");
        // 繰り返しても同じ（補正値に足し込まないので、二重にずれない）。
        assert_eq!(fixed.with_zone(jst(), ZoneMode::KeepLocal).unwrap(), fixed);
        // その後に表示だけを変えても、撮影した瞬間は保つ。
        let paris = FixedOffset::east_opt(2 * 3600).unwrap();
        let shown = fixed.with_zone(paris, ZoneMode::KeepInstant).unwrap();
        assert_eq!(shown.utc, fixed.utc);
        assert_eq!(shown.basis.as_deref(), Some("+09:00"));
        assert_eq!(shown.to_string(), "2024-05-01 05:00:00 +02:00");
        // 既定のタイムゾーンを変えても、指定したものは変わらない。
        assert_eq!(
            fixed
                .with_default_offset(FixedOffset::east_opt(0).unwrap())
                .unwrap(),
            fixed
        );
        // 取り消すと UTC の印だけの値に戻る。
        assert_eq!(fixed.without_user_offset(jst()).unwrap(), t);
        // 表示だけの指定（KeepInstant）は、UTC の印だけの値の UTC を変えない（従来の with_user_offset）。
        assert_eq!(
            t.with_zone(paris, ZoneMode::KeepInstant).unwrap(),
            t.with_user_offset(paris).unwrap()
        );
    }

    #[test]
    fn keep_instant_on_default_offset_values() {
        // オフセットの記録がなく、既定の +09:00 で推定した（カメラの時計は東京の時刻のまま）。
        let t = CaptureTime::resolve(Some("2024:05:01 12:00:00"), None, jst()).unwrap();
        let paris = FixedOffset::east_opt(2 * 3600).unwrap();
        let shown = t.with_zone(paris, ZoneMode::KeepInstant).unwrap();
        assert_eq!(shown.utc, t.utc);
        assert_eq!(shown.basis.as_deref(), Some("+09:00"));
        assert_eq!(shown.to_string(), "2024-05-01 05:00:00 +02:00");
        // 既定のタイムゾーンを変えても、指定したものは変わらない（UTC も表示も）。
        let utc0 = FixedOffset::east_opt(0).unwrap();
        assert_eq!(shown.with_default_offset(utc0).unwrap(), shown);
        // 取り消すと、その時点の既定のオフセットで推定し直す。
        let undone = shown.without_user_offset(utc0).unwrap();
        assert_eq!(undone.tz_source, TzSource::UserDefault);
        assert_eq!(undone.basis, None);
        assert_eq!(undone.utc, Some(utc(2024, 5, 1, 12, 0, 0)));
    }

    #[test]
    fn user_changes_can_be_removed_and_carried_over() {
        let paris = FixedOffset::east_opt(2 * 3600).unwrap();
        let t = CaptureTime::resolve(Some("2024:05:01 12:00:00"), Some("+09:00"), jst()).unwrap();
        assert!(!t.has_user_changes());
        let changed = t
            .with_zone(paris, ZoneMode::KeepInstant)
            .unwrap()
            .with_correction(-30)
            .unwrap();
        assert!(changed.has_user_changes());
        assert!(t.with_correction(5).unwrap().has_user_changes());
        // すべて取り消す。
        assert_eq!(changed.without_user_changes(jst()).unwrap(), t);
        // 日時がなくても補正値は 0 に戻る。
        let unknown = CaptureTime::unknown().with_correction(5).unwrap();
        assert_eq!(
            unknown.without_user_changes(jst()).unwrap(),
            CaptureTime::unknown()
        );

        // 内容の変わったファイル（日時が 1 秒違う）を読み直したときに、修正を移す。
        let reread =
            CaptureTime::resolve(Some("2024:05:01 12:00:01"), Some("+09:00"), jst()).unwrap();
        let carried = reread.with_user_changes_of(&changed);
        assert_eq!(carried.tz_source, TzSource::UserSet);
        assert_eq!(carried.tz_assumed.as_deref(), Some("+02:00"));
        assert_eq!(carried.basis.as_deref(), Some("+09:00"));
        assert_eq!(carried.correction_s, -30);
        assert_eq!(
            carried.utc,
            Some(utc(2024, 5, 1, 3, 0, 1) - TimeDelta::seconds(30))
        );
        // 修正のないものからは何も移さない。
        assert_eq!(reread.with_user_changes_of(&t), reread);
        // UTC の印だけかどうかが変わったら、計算に使うオフセットは移さない。
        let dji = CaptureTime::resolve(Some("2024-05-01T12:00:00Z"), None, jst())
            .unwrap()
            .with_zone(jst(), ZoneMode::KeepLocal)
            .unwrap();
        let carried = t.with_user_changes_of(&dji);
        assert_eq!(carried.basis, None);
        assert_eq!(carried.utc, t.utc);
    }

    #[test]
    fn serde_round_trip() {
        let t = CaptureTime::resolve(Some("2024:05:01 12:00:00"), None, jst()).unwrap();
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"tz_source\":\"user_default\""), "{json}");
        let back: CaptureTime = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
        // basis のない古い JSON も読める。
        let old = json.replace(",\"basis\":null", "");
        assert_ne!(old, json);
        assert_eq!(serde_json::from_str::<CaptureTime>(&old).unwrap(), t);
    }
}
