//! 内部で使う小さな関数（日時の文字列、整数の変換、列挙値の読み取り）。

use std::str::FromStr;

use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use genzo_model::ParseEnumError;

use crate::error::{CatalogError, Result};

/// 現在時刻を DB に保存する形式（`"2026-10-09T01:02:03.456Z"`）で返す。
pub(crate) fn now_utc_string() -> String {
    utc_to_db_string(Utc::now())
}

/// UTC の日時を DB に保存する固定長の文字列にする（`CaptureTime::utc_db_string` と同じ形式）。
///
/// 年が 0〜9999 の範囲外の場合は、範囲の端に丸める（検索の境界として使うため）。
pub(crate) fn utc_to_db_string(t: DateTime<Utc>) -> String {
    if t.year() < 0 {
        return "0000-01-01T00:00:00.000Z".to_owned();
    }
    if t.year() > 9999 {
        return "9999-12-31T23:59:59.999Z".to_owned();
    }
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// DB に保存した UTC の文字列を読む。
pub(crate) fn parse_db_utc(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| CatalogError::Corrupt(format!("日時の文字列 {s:?}: {e}")))
}

/// ファイル名に使う時刻の文字列（`"20261009T010203456Z"`。辞書順が時刻順になる）。
pub(crate) fn timestamp_for_filename(t: DateTime<Utc>) -> String {
    t.format("%Y%m%dT%H%M%S%3fZ").to_string()
}

/// u64 を SQLite の整数（i64）にする。
pub(crate) fn u64_to_i64(value: u64, what: &str) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| CatalogError::InvalidInput(format!("{what} が大きすぎます: {value}")))
}

/// SQLite の整数を u32 にする（範囲外は DB の値の不正として扱う）。
pub(crate) fn i64_to_u32(value: i64, what: &str) -> Result<u32> {
    u32::try_from(value)
        .map_err(|_| CatalogError::Corrupt(format!("{what} の値が範囲外です: {value}")))
}

/// DB の文字列を列挙値にする。
pub(crate) fn parse_enum<T: FromStr<Err = ParseEnumError>>(s: &str) -> Result<T> {
    s.parse::<T>()
        .map_err(|e| CatalogError::Corrupt(e.to_string()))
}

/// ID の列を `json_each` に渡す JSON の配列にする（`[1,2,3]`）。
pub(crate) fn ids_to_json<I: Into<i64> + Copy>(ids: &[I]) -> String {
    let mut s = String::with_capacity(ids.len() * 8 + 2);
    s.push('[');
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&(*id).into().to_string());
    }
    s.push(']');
    s
}

/// 名前（キーワード・スナップショット・仮想コピーなど）の前後の空白を取り、空でないことを確かめる。
pub(crate) fn non_empty_name<'a>(name: &'a str, what: &str) -> Result<&'a str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(CatalogError::InvalidInput(format!("{what} が空です")));
    }
    if trimmed.contains('\0') {
        return Err(CatalogError::InvalidInput(format!(
            "{what} に NUL 文字が含まれています"
        )));
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn db_utc_strings_are_fixed_length_and_round_trip() {
        let t = Utc.with_ymd_and_hms(2024, 5, 1, 3, 4, 5).unwrap();
        let s = utc_to_db_string(t);
        assert_eq!(s, "2024-05-01T03:04:05.000Z");
        assert_eq!(parse_db_utc(&s).unwrap(), t);
        assert!(parse_db_utc("yesterday").is_err());
        assert_eq!(timestamp_for_filename(t), "20240501T030405000Z");
    }

    #[test]
    fn out_of_range_years_are_clamped() {
        let far = Utc.with_ymd_and_hms(12000, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(utc_to_db_string(far), "9999-12-31T23:59:59.999Z");
        let past = Utc.with_ymd_and_hms(-5, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(utc_to_db_string(past), "0000-01-01T00:00:00.000Z");
    }

    #[test]
    fn integer_conversions() {
        assert_eq!(u64_to_i64(5, "x").unwrap(), 5);
        assert!(u64_to_i64(u64::MAX, "x").is_err());
        assert_eq!(i64_to_u32(7, "x").unwrap(), 7);
        assert!(i64_to_u32(-1, "x").is_err());
    }

    #[test]
    fn ids_json_and_names() {
        assert_eq!(ids_to_json::<i64>(&[]), "[]");
        assert_eq!(ids_to_json(&[1i64, 22, 333]), "[1,22,333]");
        assert_eq!(non_empty_name("  a ", "名前").unwrap(), "a");
        assert!(non_empty_name("   ", "名前").is_err());
        assert!(non_empty_name("a\0b", "名前").is_err());
    }
}
