//! genzo-geo の結合テスト（テスト用の口（`UreqClient::for_test` など）を使うため、crate の中に置く）。
//!
//! 本物の OSM のサーバーには問い合わせない。手元の `TcpListener` の小さなサーバー（[`server`]）か、
//! [`crate::HttpClient`] の差し替え（[`FakeClient`]）で確かめる。時計は [`FakeClock`] で差し替える。

mod geocode;
mod http;
mod os_verifier;
mod server;
mod tiles;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use crate::clock::Clock;
use crate::{GeoError, HttpClient, HttpResponse};

/// [`FakeClient`] が受け取った要求。
#[derive(Debug, Clone)]
pub(crate) struct FakeRequest {
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) max_body_bytes: u64,
}

impl FakeRequest {
    /// 名前（大文字と小文字は区別しない）が同じヘッダーの値。
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

type Handler = dyn Fn(&FakeRequest) -> Result<HttpResponse, GeoError> + Send + Sync;

/// 差し替えの HTTP のクライアント（要求を記録し、テストが決めた応答を返す）。
pub(crate) struct FakeClient {
    handler: Box<Handler>,
    requests: Mutex<Vec<FakeRequest>>,
}

impl FakeClient {
    /// 要求ごとに `handler` が応答を決める。
    pub(crate) fn new(
        handler: impl Fn(&FakeRequest) -> Result<HttpResponse, GeoError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(handler),
            requests: Mutex::new(Vec::new()),
        })
    }

    /// 決めた順に応答を返す（足りなくなったら panic）。
    pub(crate) fn scripted(responses: Vec<Result<HttpResponse, GeoError>>) -> Arc<Self> {
        let queue = Mutex::new(VecDeque::from(responses));
        Self::new(move |req| {
            queue
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("用意していない要求: {}", req.url))
        })
    }

    /// 受け取った要求（届いた順）。
    pub(crate) fn requests(&self) -> Vec<FakeRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// 受け取った要求の数。
    pub(crate) fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl HttpClient for FakeClient {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, String)],
        max_body_bytes: u64,
    ) -> Result<HttpResponse, GeoError> {
        let request = FakeRequest {
            url: url.to_string(),
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.clone()))
                .collect(),
            max_body_bytes,
        };
        self.requests.lock().unwrap().push(request.clone());
        (self.handler)(&request)
    }
}

/// 応答を作る（ヘッダーの名前は小文字にする）。
pub(crate) fn http(status: u16, headers: &[(&str, &str)], body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        headers: headers
            .iter()
            .map(|(n, v)| (n.to_ascii_lowercase(), v.to_string()))
            .collect(),
        body: body.to_vec(),
    }
}

/// PNG の署名で始まる、`tag` で見分けられる本文（デコードはしないので中身は PNG でなくてよい）。
pub(crate) fn png(tag: &str) -> Vec<u8> {
    [b"\x89PNG\r\n\x1a\n".as_slice(), tag.as_bytes()].concat()
}

/// 差し替えの時計。`sleep` は待たずに時刻を進め、待った長さを記録する。
pub(crate) struct FakeClock {
    base: Instant,
    base_utc: DateTime<Utc>,
    offset: Mutex<Duration>,
    sleeps: Mutex<Vec<Duration>>,
}

impl FakeClock {
    /// 2026-10-19T12:00:00Z から始まる時計。
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            base_utc: Utc.with_ymd_and_hms(2026, 10, 19, 12, 0, 0).unwrap(),
            offset: Mutex::new(Duration::ZERO),
            sleeps: Mutex::new(Vec::new()),
        })
    }

    /// 時刻を進める。
    pub(crate) fn advance(&self, d: Duration) {
        *self.offset.lock().unwrap() += d;
    }

    /// `sleep` で待った長さ（呼ばれた順）。
    pub(crate) fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().unwrap().clone()
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.base + *self.offset.lock().unwrap()
    }

    fn now_utc(&self) -> DateTime<Utc> {
        self.base_utc + TimeDelta::from_std(*self.offset.lock().unwrap()).unwrap()
    }

    fn sleep(&self, d: Duration) {
        self.sleeps.lock().unwrap().push(d);
        self.advance(d);
    }
}
