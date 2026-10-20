//! 地名の検索（Nominatim と同じ API の `/search`。`format=jsonv2`）。
//!
//! 問い合わせる先は、利用者が設定した検索サービスか、利用者が設定で選んだときの OSM Foundation の公開の
//! Nominatim だけ（[`GeocoderProvider`] に既定の提供元はない。公開の Nominatim を既定で使わない理由は
//! provider.rs の doc）。規則は、OSM Foundation の Nominatim
//! の利用ポリシー（<https://operations.osmfoundation.org/policies/nominatim/> 。原文はそのページの元の GitHub の
//! ソースで確かめた。公開ページは未確認）に合わせたもので、設定した提供元にもそのまま当てる（提供元の利用条件は
//! 利用者が確かめる）:
//! - **1 秒に 1 回まで**（Nominatim のポリシーの上限は、利用者ごとではなくアプリの全利用者の合計）。プロセス
//!   全体で 1 つの関所（`RequestGate`）を持ち、前の要求の開始から [`MIN_REQUEST_INTERVAL`] たっていなければ、その分
//!   待ってから送る。関所は要求から結果の保存までずっと持つので、同時に送るのは 1 件だけ（同じ検索語を同時に
//!   呼んでも、送るのは 1 回）。この関所が数えるのは 1 つのプロセスの中だけで、ほかの PC や、同じ PC で複数
//!   起動したアプリの要求は数えられない（利用者が増えると、ポリシーの上限を超えうる。docs/04_architecture.md
//!   の 9 章の AR-13）。
//! - **結果はキャッシュする**（同じ検索語を繰り返し送らない）。ポリシーは、アプリには「できれば」要求の
//!   キャッシュを勧め（Websites and Apps の節）、一括の処理（Bulk Geocoding の節）には「結果は必ずキャッシュ
//!   する。同じ検索を繰り返し送るクライアントはブロックされうる」とする。この crate は安全側として常に
//!   キャッシュする。検索語は前後の空白を除いて NFC にしたものをキーにし、最近の [`RESULT_CACHE_ENTRIES`] 件
//!   （仮置き）をプロセスの中に持つ。誤りはキャッシュしない。
//! - **オートコンプリートは作らない**（ポリシーで禁止）。この crate は呼ばれたときに 1 回だけ問い合わせる。
//!   入力の途中で呼ばないことは画面（Enter かボタンのときだけ呼ぶ）が守る。逆ジオコーディングも作らない。
//! - パラメータは `q`・`format=jsonv2`（省くと Web の画面が返る）・`limit`（[`RESULT_LIMIT`]。仮置き。
//!   Nominatim の上限は 40）・`accept-language=ja`。email は送らない（利用者の個人の情報になるため）。
//! - 応答の `lat`・`lon` は文字列、`boundingbox` は「最小緯度, 最大緯度, 最小経度, 最大経度」の文字列 4 つ。
//!   範囲外・読めない結果は捨てる（ほかの提供元のために、数の形も受け付ける）。
//! - 本文の上限は [`MAX_SEARCH_BYTES`]（仮置き）。
//!
//! ログには検索語と結果の座標を書かない（決定事項ログ No.75 の方針）。

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use genzo_model::GpsCoord;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

use crate::clock::{Clock, SystemClock};
use crate::{GeoError, GeocoderProvider, HttpClient};

/// 要求の最小の間隔（ポリシーの「1 秒に 1 回まで」）。
pub const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(1);

/// 検索語の長さの上限（文字数。仮置き）。
pub const MAX_QUERY_CHARS: usize = 200;

/// 1 回の検索で受け取る結果の数（仮置き。Nominatim の上限は 40）。
pub const RESULT_LIMIT: u8 = 10;

/// 結果をキャッシュする検索語の数（仮置き）。
pub const RESULT_CACHE_ENTRIES: usize = 100;

/// 検索の応答の本文の上限（1 MiB。仮置き）。
pub const MAX_SEARCH_BYTES: u64 = 1024 * 1024;

/// 結果の言語（`accept-language`。送らないと、その土地の言語の名前が返る）。
pub const SEARCH_LANGUAGE: &str = "ja";

/// 検索の結果の場所。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// 名前（`name`。ないこともある）。
    pub name: String,
    /// 住所などを並べた表示用の名前（`display_name`。なければ `name`）。
    pub display_name: String,
    /// 緯度（WGS 84）。
    pub lat: f64,
    /// 経度（WGS 84）。
    pub lon: f64,
    /// 範囲（`[最小緯度, 最大緯度, 最小経度, 最大経度]`。Nominatim の `boundingbox` と同じ並び）。
    pub bbox: Option<[f64; 4]>,
    /// 分類（jsonv2 の `category`。json の形なら `class`）。
    pub category: String,
    /// 種類（`type`。例 `station`）。
    pub kind: String,
}

/// プロセス全体で 1 つの、要求の関所（最後に送った要求の開始の時刻）。
#[derive(Debug, Default)]
struct RequestGate {
    last_start: Mutex<Option<Instant>>,
}

static PROCESS_GATE: LazyLock<Arc<RequestGate>> =
    LazyLock::new(|| Arc::new(RequestGate::default()));

/// 地名の検索。複数のスレッドから同時に使える（要求は関所で 1 件ずつになる）。
pub struct Geocoder {
    client: Arc<dyn HttpClient>,
    provider: GeocoderProvider,
    clock: Arc<dyn Clock>,
    gate: Arc<RequestGate>,
    cache: Mutex<VecDeque<(String, Vec<Place>)>>,
}

impl std::fmt::Debug for Geocoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Geocoder")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

impl Geocoder {
    /// 作る（関所はプロセス全体で共有する。作り直しても間隔は保たれる）。
    pub fn new(client: Arc<dyn HttpClient>, provider: GeocoderProvider) -> Self {
        Self {
            client,
            provider,
            clock: Arc::new(SystemClock),
            gate: PROCESS_GATE.clone(),
            cache: Mutex::new(VecDeque::new()),
        }
    }

    /// 時計を差し替える（テスト用）。差し替えた時計の時刻はプロセス全体の関所と比べられないので、
    /// この検索だけの関所に替える。
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self.gate = Arc::new(RequestGate::default());
        self
    }

    /// 提供元。
    pub fn provider(&self) -> &GeocoderProvider {
        &self.provider
    }

    /// テスト用: `other` と同じ関所を使うか。
    #[cfg(test)]
    pub(crate) fn shares_gate_with(&self, other: &Geocoder) -> bool {
        Arc::ptr_eq(&self.gate, &other.gate)
    }

    /// `query` で検索する。前後の空白を除いて空なら、または [`MAX_QUERY_CHARS`] 文字を超えたら
    /// [`GeoError::InvalidInput`]（通信しない）。同じ検索語の結果はキャッシュから返す。
    pub fn search(&self, query: &str) -> Result<Vec<Place>, GeoError> {
        let key = normalize_query(query)?;
        if let Some(places) = self.cached(&key) {
            return Ok(places);
        }
        let mut last_start = lock(&self.gate.last_start);
        // 関所で待つ間に、同じ検索語の結果がキャッシュに入っているかもしれない。
        if let Some(places) = self.cached(&key) {
            return Ok(places);
        }
        if let Some(last) = *last_start {
            let elapsed = self.clock.now().saturating_duration_since(last);
            if elapsed < MIN_REQUEST_INTERVAL {
                self.clock.sleep(MIN_REQUEST_INTERVAL - elapsed);
            }
        }
        *last_start = Some(self.clock.now());
        let url = self.provider.url_with_query(&format!(
            "q={}&format=jsonv2&limit={RESULT_LIMIT}&accept-language={SEARCH_LANGUAGE}",
            encode_component(&key)
        ));
        let response = self.client.get(
            &url,
            &[("Accept", "application/json".to_string())],
            MAX_SEARCH_BYTES,
        )?;
        if response.status != 200 {
            return Err(GeoError::Status(response.status));
        }
        let places = parse_places(&response.body)?;
        tracing::debug!("地名を検索しました（{} 件）", places.len());
        self.store(key, places.clone());
        // 関所は結果を保存してから放す（関所で待っていた同じ検索語の呼び出しが、保存の前にキャッシュの確認を
        // 通り抜けて、同じ問い合わせをもう一度送らないように。間隔は開始の時刻で測るので、持つ時間が延びても
        // 間隔は変わらない。誤りのときは、ここまで来ずに放す）。
        drop(last_start);
        Ok(places)
    }

    fn cached(&self, key: &str) -> Option<Vec<Place>> {
        let mut cache = lock(&self.cache);
        let index = cache.iter().position(|(k, _)| k == key)?;
        // 最近使ったものを後ろへ（古いものから捨てる）。
        let entry = cache.remove(index)?;
        let places = entry.1.clone();
        cache.push_back(entry);
        Some(places)
    }

    fn store(&self, key: String, places: Vec<Place>) {
        let mut cache = lock(&self.cache);
        cache.retain(|(k, _)| *k != key);
        cache.push_back((key, places));
        while cache.len() > RESULT_CACHE_ENTRIES {
            cache.pop_front();
        }
    }
}

/// 検索語を正規化する（前後の空白を除き、NFC にする）。
fn normalize_query(query: &str) -> Result<String, GeoError> {
    let key: String = query.trim().nfc().collect();
    if key.is_empty() {
        return Err(GeoError::InvalidInput("検索語が空です".to_string()));
    }
    if key.chars().count() > MAX_QUERY_CHARS {
        return Err(GeoError::InvalidInput(format!(
            "検索語が長すぎます（{MAX_QUERY_CHARS} 文字まで）"
        )));
    }
    Ok(key)
}

/// URL の問い合わせの値のパーセントエンコード（RFC 3986 の unreserved 以外を UTF-8 の %XX にする。空白は %20）。
fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// 応答の JSON（結果の配列）を読む。読めない結果は捨てる。
fn parse_places(body: &[u8]) -> Result<Vec<Place>, GeoError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|e| GeoError::BadResponse(format!("検索の結果の JSON を読めません: {e}")))?;
    let Value::Array(items) = value else {
        return Err(GeoError::BadResponse(
            "検索の結果が配列ではありません".to_string(),
        ));
    };
    Ok(items.iter().filter_map(place_from_json).collect())
}

fn place_from_json(item: &Value) -> Option<Place> {
    let obj = item.as_object()?;
    let lat = number(obj.get("lat")?)?;
    let lon = number(obj.get("lon")?)?;
    GpsCoord::new(lat, lon)?;
    let text = |key: &str| {
        obj.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let name = text("name");
    let display_name = match text("display_name") {
        d if d.is_empty() => name.clone(),
        d => d,
    };
    let category = match text("category") {
        c if c.is_empty() => text("class"),
        c => c,
    };
    Some(Place {
        name,
        display_name,
        lat,
        lon,
        bbox: obj.get("boundingbox").and_then(bbox),
        category,
        kind: text("type"),
    })
}

/// `boundingbox`（最小緯度, 最大緯度, 最小経度, 最大経度）。範囲外・逆順なら捨てる。
fn bbox(value: &Value) -> Option<[f64; 4]> {
    let items = value.as_array()?;
    if items.len() != 4 {
        return None;
    }
    let mut out = [0.0; 4];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = number(item)?;
    }
    let [min_lat, max_lat, min_lon, max_lon] = out;
    (GpsCoord::new(min_lat, min_lon).is_some()
        && GpsCoord::new(max_lat, max_lon).is_some()
        && min_lat <= max_lat
        && min_lon <= max_lon)
        .then_some(out)
}

/// 文字列（jsonv2）か数の値。有限の値だけ。
fn number(value: &Value) -> Option<f64> {
    let v = match value {
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    v.is_finite().then_some(v)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_trimmed_normalized_and_limited() {
        assert_eq!(normalize_query("  東京駅 \n").unwrap(), "東京駅");
        // 分解した形（が + 濁点）は合成した形（が）になる。
        assert_eq!(normalize_query("か\u{3099}").unwrap(), "が");
        for q in ["", "   ", "\u{3000}\t"] {
            assert!(
                matches!(normalize_query(q), Err(GeoError::InvalidInput(_))),
                "{q:?}"
            );
        }
        let ok = "あ".repeat(MAX_QUERY_CHARS);
        assert_eq!(normalize_query(&ok).unwrap(), ok);
        assert!(matches!(
            normalize_query(&"あ".repeat(MAX_QUERY_CHARS + 1)),
            Err(GeoError::InvalidInput(_))
        ));
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode_component("Tokyo Station"), "Tokyo%20Station");
        assert_eq!(encode_component("東京駅"), "%E6%9D%B1%E4%BA%AC%E9%A7%85");
        assert_eq!(
            encode_component("a&b=c?d#e+f/g"),
            "a%26b%3Dc%3Fd%23e%2Bf%2Fg"
        );
        assert_eq!(encode_component("A-z_0.9~"), "A-z_0.9~");
    }

    #[test]
    fn jsonv2_results_are_read_and_bad_items_are_dropped() {
        let body = r#"[
          {"place_id": 1, "licence": "Data (c) OpenStreetMap contributors, ODbL 1.0. http://osm.org/copyright",
           "lat": "35.6812362", "lon": "139.7671248", "category": "railway", "type": "station",
           "name": "東京駅", "display_name": "東京駅, 丸の内一丁目, 千代田区, 東京都, 日本",
           "boundingbox": ["35.6762362", "35.6862362", "139.7621248", "139.7721248"]},
          {"lat": "-33.8567844", "lon": 151.2152967, "class": "tourism", "type": "attraction",
           "display_name": "Sydney Opera House", "boundingbox": ["1", "2", "3"]},
          {"lat": "abc", "lon": "139"},
          {"lat": "95", "lon": "139"},
          {"lat": "35"},
          {"lat": "NaN", "lon": "0"},
          "not an object",
          {"lat": "0", "lon": "0", "boundingbox": ["10", "-10", "0", "1"]}
        ]"#
        .as_bytes();
        let places = parse_places(body).unwrap();
        assert_eq!(places.len(), 3);
        assert_eq!(
            places[0],
            Place {
                name: "東京駅".to_string(),
                display_name: "東京駅, 丸の内一丁目, 千代田区, 東京都, 日本".to_string(),
                lat: 35.6812362,
                lon: 139.7671248,
                bbox: Some([35.6762362, 35.6862362, 139.7621248, 139.7721248]),
                category: "railway".to_string(),
                kind: "station".to_string(),
            }
        );
        // 数の形の lon、json の形の class、要素の足りない boundingbox。
        assert_eq!(places[1].lon, 151.2152967);
        assert_eq!(places[1].category, "tourism");
        assert_eq!(places[1].name, "");
        assert_eq!(places[1].bbox, None);
        // 逆順の boundingbox は捨てる（場所は残す）。
        assert_eq!(
            (places[2].lat, places[2].lon, places[2].bbox),
            (0.0, 0.0, None)
        );
        assert_eq!(parse_places(b"[]").unwrap(), Vec::new());
    }

    #[test]
    fn broken_or_unexpected_json_is_a_bad_response() {
        for body in [
            b"".as_slice(),
            b"[{".as_slice(),
            b"<html></html>".as_slice(),
            br#"{"error": "x"}"#.as_slice(),
        ] {
            assert!(
                matches!(parse_places(body), Err(GeoError::BadResponse(_))),
                "{}",
                String::from_utf8_lossy(body)
            );
        }
    }
}
