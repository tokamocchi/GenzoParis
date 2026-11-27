//! GPX のログ（GPS ロガー・スマートフォンのアプリが記録した移動の記録）の読み取りと、時刻から位置を求める計算
//! （位置情報の後付け LIB-21 の、GPX のログからの位置付け）。
//!
//! - 読むのは、時刻（`<time>`）のある点だけ: 軌跡の点（`<trkpt>`）・経路の点（`<rtept>`）・ウェイポイント
//!   （`<wpt>`）。緯度・経度は属性 `lat`・`lon`（10 進数の度。WGS 84）、時刻は ISO 8601（GPX 1.1 では UTC）。
//!   高度・速度などのほかの要素は読まない。
//! - **XML の完全な解析はしない**（外部の XML のライブラリを足さないため）。点の開始タグを探し、その要素の中の
//!   `<time>` を読むだけ。コメント（`<!-- -->`）と CDATA の中は読み飛ばす。名前空間の接頭辞の付いた要素名
//!   （`<gpx:trkpt>`）にも対応する。実体参照（`&amp;` など）は、読む値（数と時刻）には現れないので展開しない。
//!   外部の実体・DTD は読まない（展開しないので、XML の実体を使った攻撃の対象にならない）。
//! - 壊れた点（座標が範囲外・数でない、時刻を読めない）は飛ばして数える（[`GpxTrack::skipped`]）。点が 1 つも
//!   なければエラー（[`GpxError::NoPoints`]）。大きさ・点の数に上限がある（[`MAX_GPX_BYTES`]・[`MAX_GPX_POINTS`]）。
//! - 位置を求める（[`GpxTrack::locate`]）: 時刻をはさむ 2 点の間隔が `max_gap` 以内なら、その 2 点の間を時刻で
//!   線形に補間する。そうでなければ（記録が途切れている・ログの前後）、近いほうの点が `max_gap` 以内にあれば
//!   その点を使う。どちらでもなければ位置なし。経度 180 度の線をまたぐ 2 点は、短いほうの向きで補間する。

use chrono::{DateTime, NaiveDateTime, Utc};

use crate::metadata::GpsCoord;

/// 読む GPX のファイルの大きさの上限（バイト。**仮置き**: 64 MiB。1 秒ごとの記録で数日分に当たる）。
pub const MAX_GPX_BYTES: u64 = 64 * 1024 * 1024;

/// 読む点の数の上限（**仮置き**: 2,000,000）。
pub const MAX_GPX_POINTS: usize = 2_000_000;

/// 記録の途切れとみなす間隔の既定の値（秒。**仮置き**: 300。これより離れた 2 点の間は補間しない）。
pub const DEFAULT_GPX_MAX_GAP_SECONDS: u32 = 300;

/// `max_gap` に指定できる上限（秒。24 時間）。
pub const MAX_GPX_MAX_GAP_SECONDS: u32 = 86_400;

/// GPX を読めない理由。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GpxError {
    /// 大きすぎる。
    #[error("GPX のファイルが大きすぎます（{actual} バイト。上限は {limit} バイト）")]
    TooLarge {
        /// 大きさ。
        actual: u64,
        /// 上限。
        limit: u64,
    },
    /// GPX ではない（`<gpx` の要素がない）。
    #[error("GPX のファイルではありません（gpx の要素が見つかりません）")]
    NotGpx,
    /// 時刻のある点がない。
    #[error("GPX のファイルに、時刻のある点がありません（読めなかった点: {skipped}）")]
    NoPoints {
        /// 読めなかった点の数。
        skipped: u64,
    },
    /// 点が多すぎる。
    #[error("GPX のファイルの点が多すぎます（上限は {limit} 点）")]
    TooManyPoints {
        /// 上限。
        limit: usize,
    },
}

/// ログの 1 点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackPoint {
    /// 時刻（UTC。Unix 時刻のミリ秒）。
    pub time_ms: i64,
    /// 位置。
    pub coord: GpsCoord,
}

/// 読んだログ（時刻の順に並べた点）。
#[derive(Debug, Clone, PartialEq)]
pub struct GpxTrack {
    points: Vec<TrackPoint>,
    /// 読めずに飛ばした点の数（時刻がない・座標が範囲外など）。
    pub skipped: u64,
}

/// 時刻から求めた位置（[`GpxTrack::locate`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Located {
    /// 位置。
    pub coord: GpsCoord,
    /// 2 点の間を補間したか（`false` なら、近いほうの 1 点をそのまま使った）。
    pub interpolated: bool,
    /// 使った点との時刻の差（秒。補間したときは、近いほうの点との差）。
    pub distance_seconds: f64,
}

/// `text` の `from` 以降で、コメント・CDATA の外にある次の `<` の位置。
fn next_tag(text: &str, mut from: usize) -> Option<usize> {
    loop {
        let at = from + text.get(from..)?.find('<')?;
        let rest = &text[at..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            from = at + 4 + comment.find("-->")? + 3;
        } else if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
            from = at + 9 + cdata.find("]]>")? + 3;
        } else {
            return Some(at);
        }
    }
}

/// 要素名から名前空間の接頭辞を除いた部分。
fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// 開始タグ（`<` の次から `>` の前まで）の属性 `key` の値。
fn attribute<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = tag;
    loop {
        let at = rest.find(key)?;
        let before_ok = rest[..at]
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace());
        let after = rest[at + key.len()..].trim_start();
        if before_ok && let Some(value) = after.strip_prefix('=') {
            let value = value.trim_start();
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            let inner = &value[1..];
            return inner.find(quote).map(|end| &inner[..end]);
        }
        rest = &rest[at + key.len()..];
    }
}

/// 時刻の文字列を UTC のミリ秒にする（ISO 8601。オフセットがなければ UTC とみなす）。
fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc).timestamp_millis());
    }
    ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
        .map(|t| t.and_utc().timestamp_millis())
}

impl GpxTrack {
    /// GPX の文字列を読む（モジュールの doc）。
    pub fn parse(text: &str) -> Result<Self, GpxError> {
        if text.len() as u64 > MAX_GPX_BYTES {
            return Err(GpxError::TooLarge {
                actual: text.len() as u64,
                limit: MAX_GPX_BYTES,
            });
        }
        let mut points: Vec<TrackPoint> = Vec::new();
        let mut skipped = 0u64;
        let mut is_gpx = false;
        let mut pos = 0;
        while let Some(at) = next_tag(text, pos) {
            let Some(len) = text[at..].find('>') else {
                break;
            };
            let tag = &text[at + 1..at + len];
            pos = at + len + 1;
            let name_end = tag
                .find(|c: char| c.is_whitespace() || c == '/')
                .unwrap_or(tag.len());
            let name = &tag[..name_end];
            let local = local_name(name);
            if local.eq_ignore_ascii_case("gpx") {
                is_gpx = true;
                continue;
            }
            if !matches!(local, "trkpt" | "rtept" | "wpt") {
                continue;
            }
            // 要素の中身（自己終了のタグなら空）。終了タグが見つからなければ、ファイルの終わりまで。
            let body = if tag.trim_end().ends_with('/') {
                ""
            } else {
                let close = format!("</{name}");
                let end = text[pos..].find(&close).map_or(text.len(), |e| pos + e);
                let body = &text[pos..end];
                pos = end;
                body
            };
            let coord = attribute(tag, "lat")
                .zip(attribute(tag, "lon"))
                .and_then(|(lat, lon)| {
                    GpsCoord::new(lat.trim().parse().ok()?, lon.trim().parse().ok()?)
                });
            let time = time_of(body);
            match (coord, time) {
                (Some(coord), Some(time_ms)) => {
                    if points.len() >= MAX_GPX_POINTS {
                        return Err(GpxError::TooManyPoints {
                            limit: MAX_GPX_POINTS,
                        });
                    }
                    points.push(TrackPoint { time_ms, coord });
                }
                _ => skipped += 1,
            }
        }
        if !is_gpx {
            return Err(GpxError::NotGpx);
        }
        if points.is_empty() {
            return Err(GpxError::NoPoints { skipped });
        }
        // 時刻の順に並べる（同じ時刻の点は、ファイルで先のものを残す）。
        points.sort_by_key(|p| p.time_ms);
        points.dedup_by_key(|p| p.time_ms);
        Ok(Self { points, skipped })
    }

    /// 点（時刻の順）。
    pub fn points(&self) -> &[TrackPoint] {
        &self.points
    }

    /// 最初の点の時刻（UTC のミリ秒）。
    pub fn start_ms(&self) -> i64 {
        self.points[0].time_ms
    }

    /// 最後の点の時刻（UTC のミリ秒）。
    pub fn end_ms(&self) -> i64 {
        self.points[self.points.len() - 1].time_ms
    }

    /// 時刻 `time_ms`（UTC のミリ秒）の位置（モジュールの doc）。`max_gap_seconds` は、記録の途切れとみなす間隔。
    pub fn locate(&self, time_ms: i64, max_gap_seconds: u32) -> Option<Located> {
        let max_gap = i64::from(max_gap_seconds) * 1000;
        let next = self.points.partition_point(|p| p.time_ms <= time_ms);
        let before = next.checked_sub(1).map(|i| self.points[i]);
        let after = self.points.get(next).copied();
        if let (Some(a), Some(b)) = (before, after)
            && b.time_ms - a.time_ms <= max_gap
        {
            let t = (time_ms - a.time_ms) as f64 / (b.time_ms - a.time_ms) as f64;
            // 経度 180 度の線をまたぐ 2 点は、短いほうの向きで補間する。
            let mut dlon = b.coord.lon - a.coord.lon;
            if dlon > 180.0 {
                dlon -= 360.0;
            } else if dlon < -180.0 {
                dlon += 360.0;
            }
            let mut lon = a.coord.lon + dlon * t;
            if lon > 180.0 {
                lon -= 360.0;
            } else if lon < -180.0 {
                lon += 360.0;
            }
            let lat = a.coord.lat + (b.coord.lat - a.coord.lat) * t;
            let nearest = (time_ms - a.time_ms).min(b.time_ms - time_ms);
            return GpsCoord::new(lat, lon).map(|coord| Located {
                coord,
                interpolated: true,
                distance_seconds: nearest as f64 / 1000.0,
            });
        }
        // 補間しない: 近いほうの点（同じ差なら前の点）が `max_gap` 以内なら使う。
        let candidates = [before, after];
        let nearest = candidates
            .iter()
            .flatten()
            .min_by_key(|p| (p.time_ms - time_ms).abs())?;
        let d = (nearest.time_ms - time_ms).abs();
        (d <= max_gap).then(|| Located {
            coord: nearest.coord,
            interpolated: false,
            distance_seconds: d as f64 / 1000.0,
        })
    }
}

/// 点の要素の中身から、`<time>` の値を読む。
fn time_of(body: &str) -> Option<i64> {
    let mut pos = 0;
    while let Some(at) = next_tag(body, pos) {
        let len = body[at..].find('>')?;
        let tag = &body[at + 1..at + len];
        pos = at + len + 1;
        let name_end = tag
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(tag.len());
        if local_name(&tag[..name_end]) == "time" && !tag.trim_end().ends_with('/') {
            let end = body[pos..].find('<').map_or(body.len(), |e| pos + e);
            return parse_time(&body[pos..end]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1">
  <metadata><time>2024-05-01T00:00:00Z</time></metadata>
  <!-- <trkpt lat="1" lon="1"><time>2024-05-01T00:00:00Z</time></trkpt> -->
  <wpt lat="35.0" lon="135.0"><name>時刻のない点</name></wpt>
  <trk><name>朝の散歩 &amp; 撮影</name><trkseg>
    <trkpt lat="35.0000" lon="135.0000"><ele>10</ele><time>2024-05-01T01:00:00Z</time></trkpt>
    <trkpt lon='135.0200' lat='35.0100'><time>2024-05-01T10:01:00+09:00</time><extensions><x:time>bad</x:time></extensions></trkpt>
    <trkpt lat="95.0" lon="135.0"><time>2024-05-01T01:01:30Z</time></trkpt>
    <trkpt lat="35.0200" lon="135.0400"><time>2024-05-01T01:02:00.500Z</time></trkpt>
  </trkseg><trkseg>
    <trkpt lat="36.0" lon="136.0"><time>2024-05-01T03:00:00Z</time></trkpt>
  </trkseg></trk>
</gpx>"#;

    fn ms(s: &str) -> i64 {
        DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis()
    }

    #[test]
    fn points_with_time_are_read_in_time_order() {
        let t = GpxTrack::parse(SAMPLE).unwrap();
        assert_eq!(t.points().len(), 4);
        // 時刻のないウェイポイントと、緯度が範囲外の点。コメントの中の点は数えない。
        assert_eq!(t.skipped, 2);
        assert_eq!(t.start_ms(), ms("2024-05-01T01:00:00Z"));
        assert_eq!(t.end_ms(), ms("2024-05-01T03:00:00Z"));
        // オフセット付きの時刻は UTC にする。属性の順・引用符の種類によらない。
        assert_eq!(t.points()[1].time_ms, ms("2024-05-01T01:01:00Z"));
        assert_eq!(t.points()[1].coord, GpsCoord::new(35.01, 135.02).unwrap());
        assert_eq!(t.points()[2].time_ms, ms("2024-05-01T01:02:00.500Z"));
    }

    #[test]
    fn positions_are_interpolated_between_close_points() {
        let t = GpxTrack::parse(SAMPLE).unwrap();
        // 1 点目と 2 点目（60 秒の間隔）の真ん中。
        let l = t.locate(ms("2024-05-01T01:00:30Z"), 300).unwrap();
        assert!(l.interpolated);
        assert!((l.coord.lat - 35.005).abs() < 1e-9 && (l.coord.lon - 135.01).abs() < 1e-9);
        assert_eq!(l.distance_seconds, 30.0);
        // 点の時刻ちょうど。
        let l = t.locate(ms("2024-05-01T01:01:00Z"), 300).unwrap();
        assert!((l.coord.lat - 35.01).abs() < 1e-9);
        assert_eq!(l.distance_seconds, 0.0);
    }

    #[test]
    fn gaps_are_not_interpolated_but_near_points_are_used() {
        let t = GpxTrack::parse(SAMPLE).unwrap();
        // 3 点目（01:02:00.5）と 4 点目（03:00:00）の間は途切れている。3 点目の 2 分後は、3 点目を使う。
        let l = t.locate(ms("2024-05-01T01:04:00Z"), 300).unwrap();
        assert!(!l.interpolated);
        assert_eq!(l.coord, GpsCoord::new(35.02, 135.04).unwrap());
        assert_eq!(l.distance_seconds, 119.5);
        // 途切れの真ん中は、どちらの点からも遠いので位置なし。
        assert_eq!(t.locate(ms("2024-05-01T02:00:00Z"), 300), None);
        // 途切れの間隔より大きな `max_gap` なら補間する。
        assert!(
            t.locate(ms("2024-05-01T02:00:00Z"), 86_400)
                .unwrap()
                .interpolated
        );
        // ログの前後: 近ければ端の点、遠ければ位置なし。
        let l = t.locate(ms("2024-05-01T00:59:00Z"), 300).unwrap();
        assert_eq!(l.coord, GpsCoord::new(35.0, 135.0).unwrap());
        assert_eq!(t.locate(ms("2024-05-01T00:50:00Z"), 300), None);
        let l = t.locate(ms("2024-05-01T03:04:59Z"), 300).unwrap();
        assert_eq!(l.coord, GpsCoord::new(36.0, 136.0).unwrap());
        assert_eq!(t.locate(ms("2024-05-01T03:05:01Z"), 300), None);
    }

    #[test]
    fn interpolation_takes_the_short_way_across_the_antimeridian() {
        let gpx = r#"<gpx><trk><trkseg>
            <trkpt lat="0" lon="179.5"><time>2024-01-01T00:00:00Z</time></trkpt>
            <trkpt lat="0" lon="-179.5"><time>2024-01-01T00:01:00Z</time></trkpt>
        </trkseg></trk></gpx>"#;
        let t = GpxTrack::parse(gpx).unwrap();
        let l = t.locate(ms("2024-01-01T00:00:45Z"), 300).unwrap();
        assert!((l.coord.lon - (-179.75)).abs() < 1e-9, "{l:?}");
        let l = t.locate(ms("2024-01-01T00:00:15Z"), 300).unwrap();
        assert!((l.coord.lon - 179.75).abs() < 1e-9, "{l:?}");
    }

    #[test]
    fn broken_input_is_reported() {
        assert_eq!(GpxTrack::parse("<html></html>"), Err(GpxError::NotGpx));
        assert_eq!(
            GpxTrack::parse(
                "<gpx><wpt lat=\"1\" lon=\"2\"/><trkpt lat=\"x\" lon=\"2\"><time>2024-01-01T00:00:00Z</time></trkpt></gpx>"
            ),
            Err(GpxError::NoPoints { skipped: 2 })
        );
        // 途中で切れたファイルでも、読めた点までは使う。
        let cut = "<gpx><trk><trkseg><trkpt lat=\"1\" lon=\"2\"><time>2024-01-01T00:00:00Z</time></trkpt><trkpt lat=\"3\" lon=";
        assert_eq!(GpxTrack::parse(cut).unwrap().points().len(), 1);
        // 名前空間の接頭辞・オフセットのない時刻（UTC とみなす）・同じ時刻の点（先のものを残す）。
        let ns = "<g:gpx><g:trkpt lat=\"1\" lon=\"2\"><g:time>2024-01-01T00:00:00</g:time></g:trkpt>\
                  <g:trkpt lat=\"5\" lon=\"6\"><g:time>2024-01-01T00:00:00Z</g:time></g:trkpt></g:gpx>";
        let t = GpxTrack::parse(ns).unwrap();
        assert_eq!(t.points().len(), 1);
        assert_eq!(t.points()[0].coord, GpsCoord::new(1.0, 2.0).unwrap());
        assert_eq!(t.start_ms(), ms("2024-01-01T00:00:00Z"));
    }

    #[test]
    fn attributes_are_matched_by_whole_name() {
        // `xlat`・`lon2` を `lat`・`lon` と取り違えない。
        assert_eq!(
            attribute("trkpt xlat=\"9\" lat=\"1\" lon=\"2\"", "lat"),
            Some("1")
        );
        assert_eq!(attribute("trkpt lon2=\"9\" lon = '2'", "lon"), Some("2"));
        assert_eq!(attribute("trkpt lat=\"1\"", "lon"), None);
    }
}
