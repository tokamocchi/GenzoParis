//! 地名の検索（[`Geocoder`]）。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::server::{LoopbackServer, response};
use super::{FakeClient, FakeClock, http};
use crate::{
    GeoError, Geocoder, GeocoderProvider, MAX_QUERY_CHARS, MAX_SEARCH_BYTES, RESULT_CACHE_ENTRIES,
    UreqClient, user_agent,
};

const TOKYO: &[u8] = r#"[{"lat": "35.6812362", "lon": "139.7671248", "category": "railway",
  "type": "station", "name": "東京駅", "display_name": "東京駅, 千代田区, 東京都, 日本",
  "boundingbox": ["35.6762362", "35.6862362", "139.7621248", "139.7721248"]}]"#
    .as_bytes();

fn ok_json() -> Arc<FakeClient> {
    FakeClient::new(|_| Ok(http(200, &[("Content-Type", "application/json")], TOKYO)))
}

/// 設定した検索サービスの URL の見本（既定の提供元はない。本物のサーバーには問い合わせない）。
const SEARCH_URL: &str = "https://geocoder.example.org/search";

fn provider() -> GeocoderProvider {
    GeocoderProvider::new(SEARCH_URL).unwrap()
}

fn geocoder(client: &Arc<FakeClient>, clock: &Arc<FakeClock>) -> Geocoder {
    Geocoder::new(client.clone(), provider()).with_clock(clock.clone())
}

#[test]
fn search_sends_the_documented_parameters() {
    let client = ok_json();
    let g = geocoder(&client, &FakeClock::new());
    let places = g.search("  東京駅 ").unwrap();
    assert_eq!(places.len(), 1);
    assert_eq!(places[0].name, "東京駅");
    assert_eq!((places[0].lat, places[0].lon), (35.6812362, 139.7671248));
    let requests = client.requests();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    // 設定した検索サービスの URL に送る。
    assert_eq!(
        req.url,
        "https://geocoder.example.org/search?q=%E6%9D%B1%E4%BA%AC%E9%A7%85&format=jsonv2&limit=10&accept-language=ja"
    );
    assert!(!req.url.contains("email"));
    assert_eq!(req.header("accept"), Some("application/json"));
    assert_eq!(req.max_body_bytes, MAX_SEARCH_BYTES);
    for name in ["cache-control", "pragma"] {
        assert_eq!(req.header(name), None, "{name}");
    }
}

#[test]
fn results_are_cached_by_the_normalized_query() {
    let client = ok_json();
    let clock = FakeClock::new();
    let g = geocoder(&client, &clock);
    let first = g.search("東京駅").unwrap();
    assert_eq!(g.search(" 東京駅\u{3000}").unwrap(), first);
    assert_eq!(client.count(), 1);
    // 分解した形（か + 濁点）と合成した形（が）は同じ検索語。
    g.search("がっこう").unwrap();
    g.search("か\u{3099}っこう").unwrap();
    assert_eq!(client.count(), 2);
    // キャッシュから返すときは待たない。
    assert_eq!(clock.sleeps().len(), 1);
}

#[test]
fn the_cache_keeps_the_most_recent_queries() {
    let client = ok_json();
    let g = geocoder(&client, &FakeClock::new());
    for i in 0..RESULT_CACHE_ENTRIES {
        g.search(&format!("q{i}")).unwrap();
    }
    assert_eq!(client.count(), RESULT_CACHE_ENTRIES);
    // q0 を使い直すと新しくなり、次に追い出されるのは q1。
    g.search("q0").unwrap();
    g.search("new").unwrap();
    assert_eq!(client.count(), RESULT_CACHE_ENTRIES + 1);
    g.search("q0").unwrap();
    assert_eq!(client.count(), RESULT_CACHE_ENTRIES + 1);
    g.search("q1").unwrap();
    assert_eq!(client.count(), RESULT_CACHE_ENTRIES + 2);
}

#[test]
fn requests_are_at_least_one_second_apart() {
    let client = ok_json();
    let clock = FakeClock::new();
    let g = geocoder(&client, &clock);
    g.search("a").unwrap();
    assert!(clock.sleeps().is_empty());
    // 続けて送ると、前の要求の開始から 1 秒になるまで待つ。
    g.search("b").unwrap();
    assert_eq!(clock.sleeps(), [Duration::from_secs(1)]);
    clock.advance(Duration::from_millis(400));
    g.search("c").unwrap();
    assert_eq!(
        clock.sleeps(),
        [Duration::from_secs(1), Duration::from_millis(600)]
    );
    // 1 秒以上あいていれば待たない。
    clock.advance(Duration::from_secs(5));
    g.search("d").unwrap();
    assert_eq!(clock.sleeps().len(), 2);
    assert_eq!(client.count(), 4);
}

#[test]
fn only_one_request_is_sent_at_a_time() {
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let client = {
        let (active, max_active) = (active.clone(), max_active.clone());
        FakeClient::new(move |_| {
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            max_active.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(http(200, &[], TOKYO))
        })
    };
    let clock = FakeClock::new();
    let g = Arc::new(geocoder(&client, &clock));
    let threads: Vec<_> = (0..4)
        .map(|i| {
            let g = g.clone();
            std::thread::spawn(move || g.search(&format!("場所{i}")).map(|p| p.len()))
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap().unwrap(), 1);
    }
    assert_eq!(client.count(), 4);
    assert_eq!(max_active.load(Ordering::SeqCst), 1);
    // 差し替えた時計は待たないと進まないので、2 件目からはそれぞれ 1 秒待つ。
    assert_eq!(clock.sleeps(), [Duration::from_secs(1); 3]);
}

#[test]
fn the_same_query_from_two_threads_is_sent_only_once() {
    // A の要求の途中で B が同じ語で呼ぶ（B は関所で待つ）。A が結果を保存する前に関所を放すと、B は
    // キャッシュの確認を通り抜けて同じ問い合わせをもう一度送ってしまう（大きな応答で、A の解析の間を長くする）。
    let one = std::str::from_utf8(TOKYO).unwrap().trim();
    let one = &one[1..one.len() - 1];
    let body = format!("[{}]", vec![one; 2000].join(","));
    assert!(body.len() as u64 <= MAX_SEARCH_BYTES);
    let client = FakeClient::new(move |_| {
        std::thread::sleep(Duration::from_millis(60));
        Ok(http(
            200,
            &[("Content-Type", "application/json")],
            body.as_bytes(),
        ))
    });
    let clock = FakeClock::new();
    let g = Arc::new(geocoder(&client, &clock));
    let a = {
        let g = g.clone();
        std::thread::spawn(move || g.search("東京駅"))
    };
    while client.count() == 0 {
        std::thread::sleep(Duration::from_millis(1));
    }
    let b = {
        let g = g.clone();
        std::thread::spawn(move || g.search("東京駅"))
    };
    let a = a.join().unwrap().unwrap();
    let b = b.join().unwrap().unwrap();
    assert_eq!(a, b);
    assert_eq!(client.count(), 1);
    // B はキャッシュから返すので、関所で 1 秒待たない。
    assert!(clock.sleeps().is_empty(), "{:?}", clock.sleeps());
}

#[test]
fn geocoders_share_one_gate_in_the_process() {
    let client = ok_json();
    let a = Geocoder::new(client.clone(), provider());
    let b = Geocoder::new(
        client.clone(),
        GeocoderProvider::new("https://geocoder.example.net/v1/search?key=abc").unwrap(),
    );
    assert!(a.shares_gate_with(&b));
    // 時計を差し替えたものは、その検索だけの関所。
    let c = Geocoder::new(client, provider()).with_clock(FakeClock::new());
    assert!(!a.shares_gate_with(&c));
}

#[test]
fn errors_are_not_cached() {
    let client = FakeClient::scripted(vec![
        Err(GeoError::Network("つながりません".into())),
        Ok(http(429, &[], b"slow down")),
        Ok(http(500, &[], b"")),
        Ok(http(200, &[], b"<html>")),
        Ok(http(200, &[], TOKYO)),
    ]);
    let g = geocoder(&client, &FakeClock::new());
    assert!(matches!(g.search("東京駅"), Err(GeoError::Network(_))));
    assert!(matches!(g.search("東京駅"), Err(GeoError::Status(429))));
    assert!(matches!(g.search("東京駅"), Err(GeoError::Status(500))));
    assert!(matches!(g.search("東京駅"), Err(GeoError::BadResponse(_))));
    assert_eq!(g.search("東京駅").unwrap().len(), 1);
    assert_eq!(g.search("東京駅").unwrap().len(), 1);
    assert_eq!(client.count(), 5);
}

#[test]
fn invalid_queries_are_rejected_without_requests() {
    let client = ok_json();
    let clock = FakeClock::new();
    let g = geocoder(&client, &clock);
    for q in [String::new(), "   ".into(), "x".repeat(MAX_QUERY_CHARS + 1)] {
        assert!(
            matches!(g.search(&q), Err(GeoError::InvalidInput(_))),
            "{q}"
        );
    }
    assert_eq!(client.count(), 0);
    assert!(clock.sleeps().is_empty());
}

#[test]
fn the_real_client_sends_the_query_on_the_wire() {
    let server = LoopbackServer::start(|_| {
        response("200 OK", &[("Content-Type", "application/json")], TOKYO)
    });
    let g = Geocoder::new(
        Arc::new(UreqClient::for_test(true)),
        GeocoderProvider::for_loopback_test(&server.url("/search")),
    )
    .with_clock(FakeClock::new());
    assert_eq!(g.search("Tokyo Station").unwrap().len(), 1);
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert_eq!(
        req.target(),
        "/search?q=Tokyo%20Station&format=jsonv2&limit=10&accept-language=ja"
    );
    assert_eq!(req.header("user-agent"), Some(user_agent()));
    assert_eq!(req.header("accept"), Some("application/json"));
    for name in ["cache-control", "pragma", "referer", "cookie"] {
        assert_eq!(req.header(name), None, "{name}");
    }
}
