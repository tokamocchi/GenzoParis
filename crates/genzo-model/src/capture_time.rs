//! 撮影日時（docs/04_architecture.md の 3.1 節。レビュー R-18）。
//!
//! 元の日時の文字列、元のオフセットの有無、推定に使ったオフセットとその出どころ、
//! 時計のずれの補正値、補正後の UTC を別々に持つ。後から時計のずれやタイムゾーンを
//! 修正（LIB-16）したり、修正を取り消したりできるようにするため。
//!
//! - オフセットの情報がない写真・動画は、ユーザーが設定した既定のオフセットで UTC を
//!   推定し、`tz_source = user_default` と記録する。
//! - 撮影日時がないファイルは、現在時刻などで置き換えず `utc = None` とする
//!   （撮影日時順の並べ替えでは最後に並べる）。
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
    pub offset: Option<String>,
    /// UTC の推定に使ったオフセットの出どころ（`tz_source`）。
    /// 日時がない場合は、日時が分かったときに使う既定の扱い（`user_default`）にしておく。
    pub tz_source: TzSource,
    /// UTC の推定に使ったオフセット（`tz_assumed`。`"+09:00"` 形式）。日時がなければ `None`。
    pub tz_assumed: Option<String>,
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
    pub fn resolve(
        raw: Option<&str>,
        offset: Option<&str>,
        default_offset: FixedOffset,
    ) -> Result<Self, CaptureTimeError> {
        let Some(raw_str) = raw.filter(|s| !s.is_empty()) else {
            return Ok(Self::unknown());
        };
        let Some((local, embedded)) = parse_datetime(raw_str)? else {
            return Ok(Self {
                raw: Some(raw_str.to_owned()),
                ..Self::unknown()
            });
        };
        // 文字列に含まれるオフセットを優先し、なければ別に渡されたオフセットを使う。
        let original = match (embedded, offset.map(str::trim).filter(|s| !s.is_empty())) {
            (Some(o), _) => Some(o),
            (None, Some(s)) => Some(parse_offset(s)?),
            (None, None) => None,
        };
        let (used, tz_source) = match original {
            Some(o) => (o, TzSource::Exif),
            None => (default_offset, TzSource::UserDefault),
        };
        Ok(Self {
            raw: Some(raw_str.to_owned()),
            offset: original.map(format_offset),
            tz_source,
            tz_assumed: Some(format_offset(used)),
            correction_s: 0,
            utc: Some(to_utc(local, used, 0)?),
        })
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

    /// ユーザーが指定したオフセットで UTC を推定し直す（LIB-16。`tz_source = user_set`）。
    ///
    /// 日時がない場合は何もしない。
    pub fn with_user_offset(&self, offset: FixedOffset) -> Result<Self, CaptureTimeError> {
        if self.utc.is_none() {
            return Ok(self.clone());
        }
        let mut next = self.clone();
        next.tz_source = TzSource::UserSet;
        next.tz_assumed = Some(format_offset(offset));
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
        match self.offset.as_deref() {
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
    /// もの（`tz_source = user_default`）だけ UTC を推定し直す。
    pub fn with_default_offset(
        &self,
        default_offset: FixedOffset,
    ) -> Result<Self, CaptureTimeError> {
        if self.tz_source != TzSource::UserDefault || self.utc.is_none() {
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

    /// 元の文字列・推定に使ったオフセット・補正値から UTC を計算し直す。
    fn recomputed(mut self) -> Result<Self, CaptureTimeError> {
        let parsed = match self.raw.as_deref() {
            Some(raw) => parse_datetime(raw)?,
            None => None,
        };
        let (Some((local, _)), Some(assumed)) = (parsed, self.tz_assumed.as_deref()) else {
            self.utc = None;
            return Ok(self);
        };
        let offset = parse_offset(assumed)?;
        self.utc = Some(to_utc(local, offset, self.correction_s)?);
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
/// 日時なしとして扱う文字列なら `Ok(None)`。
pub fn parse_datetime(
    s: &str,
) -> Result<Option<(NaiveDateTime, Option<FixedOffset>)>, CaptureTimeError> {
    let trimmed = s.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if is_blank_datetime(trimmed) {
        return Ok(None);
    }
    let invalid = || CaptureTimeError::InvalidDateTime(s.to_owned());
    // 末尾のオフセットを切り離す。日付の部分（先頭 10 文字）の '-' と区別するため、
    // 時刻の部分だけを探す。
    let (body, offset) = if let Some(body) = trimmed.strip_suffix(['Z', 'z']) {
        (
            body,
            Some(FixedOffset::east_opt(0).expect("0 は有効なオフセット")),
        )
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
                (&trimmed[..split], Some(offset))
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

        // ffprobe の creation_time の形式。
        let t = CaptureTime::resolve(Some("2024-05-01T03:34:56.000000Z"), None, jst()).unwrap();
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
    fn serde_round_trip() {
        let t = CaptureTime::resolve(Some("2024:05:01 12:00:00"), None, jst()).unwrap();
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"tz_source\":\"user_default\""), "{json}");
        let back: CaptureTime = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }
}
