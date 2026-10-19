//! HTTP のキャッシュの指示から、タイルをサーバーに問い合わせずに使ってよい期限を決める（RFC 9111 の一部）。
//!
//! タイルの利用ポリシーは「HTTP のキャッシュの指示（Cache-Control・Expires・ETag）に従う。読めなければ
//! 少なくとも 7 日キャッシュする」「期限の切れたタイルは If-None-Match・If-Modified-Since の条件付きの
//! 要求にする」を求める。この crate のキャッシュは利用者ごとの手元のキャッシュ（private cache）として扱う。
//!
//! - `Cache-Control: max-age=N` があれば N 秒（`Expires` より優先）。`Age` があれば、その分を引く。
//! - なければ `Expires`（`Date` との差。`Date` がなければ受け取った時刻との差）。読めない `Expires`
//!   （`0` など）は「すでに切れている」とみなす（RFC 9111 の 5.3 節）。
//! - どちらもなければ 7 日（[`DEFAULT_LIFETIME_SECS`]。ポリシーの「少なくとも 7 日」）。
//! - `no-store` は保存しない。`no-cache` は保存するが、使うたびに条件付きの要求で確かめる（期限 0）。
//! - `s-maxage`（共有のキャッシュ向け）・`private`・`public` などは期限に使わない。
//! - 期限が切れた後に通信に失敗したら、古いタイルを使ってよいか（stale-on-error）: `no-cache` か
//!   `must-revalidate` があれば使わない（RFC 9111 の 4.2.4 節）。この扱い自体は人の確認待ち。

use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};

use crate::HttpResponse;

/// キャッシュの指示がないときの期限（7 日。ポリシーの「少なくとも 7 日」）。
pub(crate) const DEFAULT_LIFETIME_SECS: i64 = 7 * 24 * 60 * 60;

/// 秒数の上限（RFC 9111 の 1.2.2 節。これより大きい値はこの値とみなす）。
const MAX_DELTA_SECONDS: i64 = 2_147_483_648;

/// 応答から決めたキャッシュの扱い。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Policy {
    /// 保存してよいか（`no-store` なら false）。
    pub(crate) store: bool,
    /// 受け取った時刻から、問い合わせずに使ってよい秒数（`Age` を引いた後。0 以上）。
    pub(crate) fresh_secs: i64,
    /// 期限の長さ（`Age` を引く前）。304 の応答にキャッシュの指示がないとき、前の値を使い直すため。
    pub(crate) lifetime_secs: i64,
    /// 期限が切れた後、通信に失敗したら古いものを使ってよいか。
    pub(crate) stale_ok: bool,
}

impl Policy {
    /// 受け取った時刻 `received` から見た期限。
    pub(crate) fn expires_at(&self, received: DateTime<Utc>) -> DateTime<Utc> {
        received + TimeDelta::seconds(self.fresh_secs)
    }
}

/// 前に保存した扱い（304 の応答にキャッシュの指示がないときに使い直す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Previous {
    /// 前の期限の長さ。
    pub(crate) lifetime_secs: i64,
    /// 前の stale-on-error の可否。
    pub(crate) stale_ok: bool,
}

/// `response`（200 か 304）のキャッシュの扱いを決める。`now` は受け取った時刻。`previous` は 304 のときの
/// 前の扱い（RFC 9111 の 4.3.4 節: 304 の応答のヘッダーで、保存したヘッダーを更新する）。304 に
/// `Cache-Control` がなければ、前の扱い（期限の長さと stale-on-error の可否）を使い直す。前の
/// `no-cache`・`must-revalidate` を 304 の `Expires` だけで緩めないため（`Expires` だけの提供元では、
/// 前の期限の長さも `Expires` と `Date` の差なので、ふつうは同じ長さになる）。
pub(crate) fn policy(
    response: &HttpResponse,
    now: DateTime<Utc>,
    previous: Option<Previous>,
) -> Policy {
    let cache_control = response.header_list("cache-control");
    let expires = response.header("expires");
    let age = response
        .header("age")
        .and_then(parse_delta_seconds)
        .unwrap_or(0);

    if cache_control.is_none()
        && let Some(prev) = previous
    {
        return Policy {
            store: true,
            fresh_secs: (prev.lifetime_secs - age).max(0),
            lifetime_secs: prev.lifetime_secs,
            stale_ok: prev.stale_ok,
        };
    }

    let cc = CacheControl::parse(cache_control.as_deref().unwrap_or(""));
    let lifetime_secs = if cc.no_cache {
        0
    } else if let Some(max_age) = cc.max_age {
        max_age
    } else if let Some(expires) = expires {
        match parse_http_date(expires) {
            Some(expires) => {
                let date = response
                    .header("date")
                    .and_then(parse_http_date)
                    .unwrap_or(now);
                (expires - date).num_seconds().clamp(0, MAX_DELTA_SECONDS)
            }
            None => 0,
        }
    } else {
        DEFAULT_LIFETIME_SECS
    };
    Policy {
        store: !cc.no_store,
        fresh_secs: (lifetime_secs - age).max(0),
        lifetime_secs,
        stale_ok: !(cc.no_cache || cc.must_revalidate),
    }
}

/// `Cache-Control` の、期限にかかわる指示。
#[derive(Debug, Default, PartialEq, Eq)]
struct CacheControl {
    no_store: bool,
    no_cache: bool,
    must_revalidate: bool,
    /// 複数あれば、いちばん短いもの（いちばん厳しい指示に従う。RFC 9111 の 4.2.1 節）。読めない値は 0。
    max_age: Option<i64>,
}

impl CacheControl {
    fn parse(value: &str) -> Self {
        let mut cc = Self::default();
        for directive in value.split(',') {
            let directive = directive.trim();
            if directive.is_empty() {
                continue;
            }
            let (name, arg) = match directive.split_once('=') {
                Some((name, arg)) => (name.trim(), Some(arg.trim().trim_matches('"'))),
                None => (directive, None),
            };
            match name.to_ascii_lowercase().as_str() {
                "no-store" => cc.no_store = true,
                // 「no-cache="Set-Cookie"」のように欄を限った形も、厳しい側（全体に効く）に扱う。
                "no-cache" => cc.no_cache = true,
                "must-revalidate" => cc.must_revalidate = true,
                "max-age" => {
                    let secs = arg.and_then(parse_delta_seconds).unwrap_or(0);
                    cc.max_age = Some(cc.max_age.map_or(secs, |m| m.min(secs)));
                }
                _ => {}
            }
        }
        cc
    }
}

/// 秒数（delta-seconds。数字だけ）。大きすぎる値は上限にする。
fn parse_delta_seconds(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(
        value
            .parse::<i64>()
            .map_or(MAX_DELTA_SECONDS, |v| v.min(MAX_DELTA_SECONDS)),
    )
}

/// HTTP の日付（RFC 9110 の 5.6.7 節。IMF-fixdate のほか、古い RFC 850 の形と asctime の形も読む）。
pub(crate) fn parse_http_date(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    // IMF-fixdate: Sun, 06 Nov 1994 08:49:37 GMT
    if let Ok(dt) = DateTime::parse_from_rfc2822(value) {
        return Some(dt.with_timezone(&Utc));
    }
    // RFC 850: Sunday, 06-Nov-94 08:49:37 GMT
    if let Ok(dt) = NaiveDateTime::parse_from_str(value, "%A, %d-%b-%y %H:%M:%S GMT") {
        return Some(dt.and_utc());
    }
    // asctime: Sun Nov  6 08:49:37 1994
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Ok(dt) = NaiveDateTime::parse_from_str(&collapsed, "%a %b %e %H:%M:%S %Y") {
        return Some(dt.and_utc());
    }
    None
}

/// HTTP の日付の文字列（IMF-fixdate。If-Modified-Since に使う。ふだんはサーバーの Last-Modified をそのまま送る）。
#[cfg(test)]
pub(crate) fn format_http_date(dt: DateTime<Utc>) -> String {
    dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn resp(headers: &[(&str, &str)]) -> HttpResponse {
        HttpResponse {
            status: 200,
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_ascii_lowercase(), v.to_string()))
                .collect(),
            body: Vec::new(),
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 19, 12, 0, 0).unwrap()
    }

    #[test]
    fn http_dates_in_all_three_forms_are_read() {
        let expected = Utc.with_ymd_and_hms(1994, 11, 6, 8, 49, 37).unwrap();
        for s in [
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
            "  Sun, 06 Nov 1994 08:49:37 GMT ",
        ] {
            assert_eq!(parse_http_date(s), Some(expected), "{s}");
        }
        for s in ["", "0", "-1", "tomorrow", "Sun, 32 Nov 1994 08:49:37 GMT"] {
            assert_eq!(parse_http_date(s), None, "{s}");
        }
        assert_eq!(format_http_date(expected), "Sun, 06 Nov 1994 08:49:37 GMT");
    }

    #[test]
    fn max_age_wins_over_expires_and_age_is_subtracted() {
        let p = policy(
            &resp(&[
                ("Cache-Control", "max-age=604800"),
                ("Expires", "Sun, 06 Nov 1994 08:49:37 GMT"),
                ("Age", "100"),
            ]),
            now(),
            None,
        );
        assert_eq!(p.lifetime_secs, 604_800);
        assert_eq!(p.fresh_secs, 604_700);
        assert!(p.store && p.stale_ok);
        assert_eq!(p.expires_at(now()), now() + TimeDelta::seconds(604_700));
        // Age が期限を超えていれば 0。
        let p = policy(
            &resp(&[("Cache-Control", "max-age=10"), ("Age", "50")]),
            now(),
            None,
        );
        assert_eq!(p.fresh_secs, 0);
    }

    #[test]
    fn several_cache_control_lines_and_odd_values_are_read() {
        let mut r = resp(&[
            ("Cache-Control", "public"),
            ("Cache-Control", "MAX-AGE=\"600\""),
        ]);
        assert_eq!(policy(&r, now(), None).lifetime_secs, 600);
        // いちばん短い max-age。
        r.headers
            .push(("cache-control".into(), "max-age=60".into()));
        assert_eq!(policy(&r, now(), None).lifetime_secs, 60);
        // 読めない max-age は 0（すぐ切れる）。
        let p = policy(&resp(&[("Cache-Control", "max-age=abc")]), now(), None);
        assert_eq!(p.lifetime_secs, 0);
        // 大きすぎる値は上限。
        let p = policy(
            &resp(&[("Cache-Control", "max-age=99999999999999999999")]),
            now(),
            None,
        );
        assert_eq!(p.lifetime_secs, MAX_DELTA_SECONDS);
        // 共有のキャッシュ向けの s-maxage は使わない（指示がないのと同じで 7 日）。
        let p = policy(
            &resp(&[("Cache-Control", "s-maxage=5, public")]),
            now(),
            None,
        );
        assert_eq!(p.lifetime_secs, DEFAULT_LIFETIME_SECS);
    }

    #[test]
    fn expires_is_measured_from_the_server_date() {
        // サーバーの時計が 1 時間ずれていても、Date との差（1 日）を使う。
        let p = policy(
            &resp(&[
                ("Date", "Mon, 19 Oct 2026 13:00:00 GMT"),
                ("Expires", "Tue, 20 Oct 2026 13:00:00 GMT"),
            ]),
            now(),
            None,
        );
        assert_eq!(p.lifetime_secs, 86_400);
        // Date がなければ受け取った時刻との差。
        let p = policy(
            &resp(&[("Expires", "Mon, 19 Oct 2026 12:30:00 GMT")]),
            now(),
            None,
        );
        assert_eq!(p.lifetime_secs, 1_800);
        // 過去の Expires・読めない Expires は 0。
        for e in ["Sun, 06 Nov 1994 08:49:37 GMT", "0", "-1"] {
            let p = policy(&resp(&[("Expires", e)]), now(), None);
            assert_eq!(p.lifetime_secs, 0, "{e}");
            assert!(p.store && p.stale_ok);
        }
    }

    #[test]
    fn without_cache_headers_tiles_are_kept_for_seven_days() {
        let p = policy(&resp(&[("ETag", "\"v1\"")]), now(), None);
        assert_eq!(DEFAULT_LIFETIME_SECS, 604_800);
        assert_eq!(p.lifetime_secs, DEFAULT_LIFETIME_SECS);
        assert_eq!(p.expires_at(now()), now() + TimeDelta::days(7));
        assert!(p.store && p.stale_ok);
    }

    #[test]
    fn no_store_no_cache_and_must_revalidate() {
        let p = policy(
            &resp(&[("Cache-Control", "no-store, max-age=600")]),
            now(),
            None,
        );
        assert!(!p.store);
        let p = policy(
            &resp(&[("Cache-Control", "no-cache, max-age=600")]),
            now(),
            None,
        );
        assert!(p.store);
        assert_eq!(p.fresh_secs, 0);
        assert!(!p.stale_ok);
        let p = policy(
            &resp(&[("Cache-Control", "No-Cache=\"Set-Cookie\"")]),
            now(),
            None,
        );
        assert_eq!(p.fresh_secs, 0);
        let p = policy(
            &resp(&[("Cache-Control", "max-age=600, must-revalidate")]),
            now(),
            None,
        );
        assert_eq!(p.fresh_secs, 600);
        assert!(!p.stale_ok);
    }

    #[test]
    fn not_modified_without_cache_headers_keeps_the_previous_policy() {
        let prev = Previous {
            lifetime_secs: 3_600,
            stale_ok: false,
        };
        let mut r = resp(&[("ETag", "\"v2\""), ("Age", "60")]);
        r.status = 304;
        let p = policy(&r, now(), Some(prev));
        assert_eq!(p.lifetime_secs, 3_600);
        assert_eq!(p.fresh_secs, 3_540);
        assert!(p.store);
        assert!(!p.stale_ok);
        // Expires だけの 304 でも、前の扱いを使い直す（前の must-revalidate を緩めない）。
        let mut r = resp(&[
            ("Date", "Mon, 19 Oct 2026 12:00:00 GMT"),
            ("Expires", "Tue, 20 Oct 2026 12:00:00 GMT"),
        ]);
        r.status = 304;
        let p = policy(&r, now(), Some(prev));
        assert_eq!((p.lifetime_secs, p.stale_ok), (3_600, false));
        // 304 に Cache-Control があれば、それで決め直す。
        let mut r = resp(&[("Cache-Control", "max-age=120")]);
        r.status = 304;
        let p = policy(&r, now(), Some(prev));
        assert_eq!(p.lifetime_secs, 120);
        assert!(p.stale_ok);
    }
}
