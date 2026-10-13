//! 写真と動画の統合タイムライン（ORG-01。docs/04_architecture.md の 3.13 節）。
//!
//! - 表示中の検索結果（撮影日時の順。写真と動画が混ざる。折りたたんだスタックは代表の 1 枚）を、**撮影日**
//!   （写真ごとの表示のオフセットの現地の日付）と、その日の中の**イベント**（前の写真との撮影の間隔が
//!   `gap_seconds` を超えたら区切る）のまとまりに分ける（[`Core::timeline`]）。まとまりは検索結果の中の連続した
//!   範囲（先頭の位置と数）なので、画面は見出しの行を含めた行の位置を、まとまりの数に比例する手間で求められる
//!   （50 万件でも写真ごとの情報を画面に送らない）。
//! - 撮影日時の分からない写真・動画は、最後の 1 つのまとまり（日付なし）にする（撮影日時の順では最後に並ぶ）。
//! - 日付は写真ごとの現地の日付なので、タイムゾーンをまたぐ旅行では、撮影の順（UTC）と日付の順が前後しうる
//!   （同じ日付の見出しが 2 回出ることがある）。イベントは日付をまたがない（深夜のまたぎは日付で分ける）。
//! - まとまりが [`MAX_TIMELINE_GROUPS`] を超えるときは、イベントで分けずに日付だけでまとめ直す
//!   （[`Timeline::events_merged`]）。
//! - 撮影日時の順でない検索結果には使えない（種類 `invalid_argument`）。

use genzo_catalog::TimelineKey;

use crate::core::Core;
use crate::error::ApiError;
use crate::types::{SortKey, Timeline, TimelineGroup};

/// まとまりの数の上限（**仮置き**: 50,000。画面へ送る量と、行の計算の手間の上限）。
pub const MAX_TIMELINE_GROUPS: usize = 50_000;

/// イベントの区切りの間隔の上限（秒。1 日。イベントは日付をまたがないので、これより長くしても変わらない）。
pub const MAX_TIMELINE_GAP_SECONDS: u32 = 86_400;

const DAY_MS: i64 = 86_400_000;

/// 1 枚の現地の時刻（ミリ秒。UTC に表示のオフセットを足したもの）。
fn local_ms(k: &TimelineKey) -> Option<i64> {
    k.utc_ms.map(|u| u + i64::from(k.offset_s) * 1000)
}

/// 現地の時刻（ミリ秒）の日付（`"2024-05-01"`）と時刻（`"2024-05-01T09:12:05"`）。
fn local_strings(ms: i64) -> Option<(String, String)> {
    let t = chrono::DateTime::from_timestamp_millis(ms)?.naive_utc();
    Some((
        t.format("%Y-%m-%d").to_string(),
        t.format("%Y-%m-%dT%H:%M:%S").to_string(),
    ))
}

/// まとまりを作る（`gap_ms` が `None` ならイベントで分けない）。撮影日の数（重ならない日付の数）と、撮影日時の
/// 分からない数も返す。日付の見出し（`new_day`）は、前のまとまりと日付が違うとき（撮影日時のないまとまりの後を
/// 含む）に付ける。
pub(crate) fn build_groups(
    keys: &[TimelineKey],
    gap_ms: Option<i64>,
) -> (Vec<TimelineGroup>, u64, u64) {
    let mut groups: Vec<TimelineGroup> = Vec::new();
    let mut days = std::collections::HashSet::new();
    let mut without_time = 0u64;
    // 今のまとまりの、日付の番号（1970-01-01 からの日数。撮影日時のないまとまりは None）と最後の UTC。
    let mut current: Option<(Option<i64>, i64)> = None;
    // 今のまとまりの最後の写真の現地の時刻（ミリ秒。文字列にするのは、まとまりを閉じるときだけ）。
    let mut last_local: Option<i64> = None;
    let close = |groups: &mut Vec<TimelineGroup>, last_local: Option<i64>| {
        if let (Some(g), Some(l)) = (groups.last_mut(), last_local) {
            g.last = local_strings(l).map(|(_, t)| t);
        }
    };
    for (i, k) in keys.iter().enumerate() {
        let local = local_ms(k);
        let day = local.map(|l| l.div_euclid(DAY_MS));
        let starts_new = match (current, k.utc_ms) {
            (None, _) => true,
            (Some((prev_day, _)), None) => prev_day.is_some(),
            (Some((prev_day, prev_utc)), Some(utc)) => {
                prev_day != day || gap_ms.is_some_and(|g| (utc - prev_utc).abs() > g)
            }
        };
        if starts_new {
            close(&mut groups, last_local);
            last_local = None;
            let new_day = match (current, day) {
                (_, None) => false,
                (None, Some(_)) => true,
                (Some((prev_day, _)), Some(d)) => prev_day != Some(d),
            };
            if let Some(d) = day {
                days.insert(d);
            }
            let (day_text, time_text) = match local.and_then(local_strings) {
                Some((d, t)) => (Some(d), Some(t)),
                None => (None, None),
            };
            groups.push(TimelineGroup {
                start: i as u64,
                count: 0,
                day: day_text,
                new_day,
                first: time_text.clone(),
                last: time_text,
                photos: 0,
                videos: 0,
            });
        }
        let g = groups.last_mut().expect("最初の写真でまとまりを作った");
        g.count += 1;
        if k.video {
            g.videos += 1;
        } else {
            g.photos += 1;
        }
        match k.utc_ms {
            Some(utc) => {
                last_local = local;
                current = Some((day, utc));
            }
            None => {
                without_time += 1;
                current = Some((None, current.map(|c| c.1).unwrap_or(0)));
            }
        }
    }
    close(&mut groups, last_local);
    (groups, days.len() as u64, without_time)
}

impl Core {
    /// 表示中の検索結果（世代 `generation`）のタイムライン（モジュールの doc）。`gap_seconds` はイベントの区切りの
    /// 間隔（0 ならイベントで分けない。[`MAX_TIMELINE_GAP_SECONDS`] まで）。古い世代なら [`ApiError::Stale`]、
    /// 撮影日時の順でなければ種類 `invalid_argument`。
    pub fn timeline(&self, generation: u64, gap_seconds: u32) -> Result<Timeline, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if gap_seconds > MAX_TIMELINE_GAP_SECONDS {
            return Err(ApiError::InvalidArgument(format!(
                "イベントの区切りの間隔は {MAX_TIMELINE_GAP_SECONDS} 秒までです（{gap_seconds}）"
            )));
        }
        let (ids, sort) = crate::search::current_ids(inner, generation)?;
        if sort.key != SortKey::CaptureTime {
            return Err(ApiError::InvalidArgument(
                "タイムラインは撮影日時の順の表示でだけ使えます".to_owned(),
            ));
        }
        // 50 万件では表を読むのに 0.4 秒ほどかかるので、読み取り専用の接続で読み、カタログを変える操作を
        // 待たせない（開けなければ本体の接続で読む）。
        let keys = match genzo_catalog::Catalog::open_read_only(&inner.config.catalog_path) {
            Ok(reader) => {
                let keys = reader.timeline_keys(&ids);
                drop(reader);
                keys?
            }
            Err(e) => {
                tracing::debug!(error = %e, "タイムラインの読み取りに本体の接続を使います");
                inner.with_catalog(|c| c.timeline_keys(&ids))?
            }
        };
        let gap = (gap_seconds > 0).then(|| i64::from(gap_seconds) * 1000);
        let (mut groups, mut days, mut without_time) = build_groups(&keys, gap);
        let mut events_merged = false;
        if gap.is_some() && groups.len() > MAX_TIMELINE_GROUPS {
            (groups, days, without_time) = build_groups(&keys, None);
            events_merged = true;
        }
        Ok(Timeline {
            generation,
            gap_seconds,
            groups,
            events_merged,
            days,
            without_time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000;

    /// 2024-05-01T00:00:00Z からの時間（時）の写真（+09:00）。
    fn at(hours: f64) -> TimelineKey {
        TimelineKey {
            utc_ms: Some(1_714_521_600_000 + (hours * H as f64) as i64),
            offset_s: 9 * 3600,
            video: false,
        }
    }

    fn video(mut k: TimelineKey) -> TimelineKey {
        k.video = true;
        k
    }

    fn spans(groups: &[TimelineGroup]) -> Vec<(u64, u64, Option<&str>, bool)> {
        groups
            .iter()
            .map(|g| (g.start, g.count, g.day.as_deref(), g.new_day))
            .collect()
    }

    #[test]
    fn groups_split_by_local_day_and_gap() {
        // 現地 09:00・09:30・13:00（4 時間後）・翌日 00:30（UTC では 15:30）、撮影日時なし 2 枚。
        let keys = [
            at(0.0),
            video(at(0.5)),
            at(4.0),
            at(15.5),
            TimelineKey::default(),
            TimelineKey::default(),
        ];
        let (g, days, without) = build_groups(&keys, Some(2 * H));
        assert_eq!(
            spans(&g),
            vec![
                (0, 2, Some("2024-05-01"), true),
                (2, 1, Some("2024-05-01"), false),
                (3, 1, Some("2024-05-02"), true),
                (4, 2, None, false),
            ]
        );
        assert_eq!((days, without), (2, 2));
        assert_eq!((g[0].photos, g[0].videos), (1, 1));
        assert_eq!(g[0].first.as_deref(), Some("2024-05-01T09:00:00"));
        assert_eq!(g[0].last.as_deref(), Some("2024-05-01T09:30:00"));
        assert_eq!(g[3].first, None);
        // 間隔なし（イベントで分けない）。
        let (g, ..) = build_groups(&keys, None);
        assert_eq!(
            spans(&g),
            vec![
                (0, 3, Some("2024-05-01"), true),
                (3, 1, Some("2024-05-02"), true),
                (4, 2, None, false),
            ]
        );
        // 数の合計は件数と同じ。
        assert_eq!(g.iter().map(|x| x.count).sum::<u64>(), keys.len() as u64);
    }

    #[test]
    fn descending_order_and_time_zones() {
        // 新しい順（間隔は絶対値で比べる）。
        let keys = [at(4.0), at(3.5), at(0.0)];
        let (g, ..) = build_groups(&keys, Some(H));
        assert_eq!(
            spans(&g),
            vec![
                (0, 2, Some("2024-05-01"), true),
                (2, 1, Some("2024-05-01"), false)
            ]
        );
        assert_eq!(g[0].first.as_deref(), Some("2024-05-01T13:00:00"));
        assert_eq!(g[0].last.as_deref(), Some("2024-05-01T12:30:00"));
        // 東京（+09:00）の 5/1 23:00 の後に、パリ（+02:00）の 5/1 18:00（UTC では後）。日付が同じなので 1 つのまとまり
        // （間隔は UTC で 2 時間以内）。
        let tokyo = at(14.0);
        let paris = TimelineKey {
            utc_ms: Some(1_714_521_600_000 + 16 * H),
            offset_s: 2 * 3600,
            video: false,
        };
        let (g, days, _) = build_groups(&[tokyo, paris], Some(3 * H));
        assert_eq!(spans(&g), vec![(0, 2, Some("2024-05-01"), true)]);
        assert_eq!(days, 1);
        // 日付が戻る（東京の 5/2 01:00 の後に、パリの 5/1 19:00（UTC では後）、また東京の 5/2）と、同じ日付の
        // 見出しが 2 回出る。
        let paris_later = TimelineKey {
            utc_ms: Some(1_714_521_600_000 + 17 * H),
            offset_s: 2 * 3600,
            video: false,
        };
        let back = [at(16.0), paris_later, at(18.0)];
        let (g, days, _) = build_groups(&back, None);
        assert_eq!(
            spans(&g),
            vec![
                (0, 1, Some("2024-05-02"), true),
                (1, 1, Some("2024-05-01"), true),
                (2, 1, Some("2024-05-02"), true),
            ]
        );
        assert_eq!(days, 2);
        assert!(build_groups(&[], Some(H)).0.is_empty());
    }

    /// 55 万件のまとまりを作る時間の計測用（通常のテストでは実行しない）。
    /// 実行例: `cargo test -p genzo-api --lib -- --ignored timeline_scale --nocapture`
    #[test]
    #[ignore = "計測用"]
    fn timeline_scale_measurement() {
        // 10 年分・1 日 150 枚（1 時間に 15 枚ほど、1 日 10 時間）。
        let keys: Vec<TimelineKey> = (0..550_000i64)
            .map(|i| TimelineKey {
                utc_ms: Some(1_500_000_000_000 + (i / 150) * 86_400_000 + (i % 150) * 240_000),
                offset_s: 9 * 3600,
                video: i % 20 == 0,
            })
            .collect();
        let t = std::time::Instant::now();
        let (g, days, _) = build_groups(&keys, Some(2 * H));
        eprintln!(
            "build_groups {} 件 → {} まとまり・{days} 日: {:?}",
            keys.len(),
            g.len(),
            t.elapsed()
        );
    }

    #[test]
    fn photos_without_time_in_the_middle_form_their_own_group() {
        let keys = [at(0.0), TimelineKey::default(), at(0.1)];
        let (g, days, without) = build_groups(&keys, Some(H));
        assert_eq!(
            spans(&g),
            vec![
                (0, 1, Some("2024-05-01"), true),
                (1, 1, None, false),
                (2, 1, Some("2024-05-01"), true),
            ]
        );
        assert_eq!((days, without), (1, 1));
    }
}
