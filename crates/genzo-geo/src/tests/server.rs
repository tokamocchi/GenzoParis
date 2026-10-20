//! テスト用の小さな HTTP/1.1 のサーバー（`std::net::TcpListener`。新しい crate を使わない）。
//!
//! 127.0.0.1 の空いているポートで待ち、届いた要求（要求の行とヘッダー）を記録して、テストが決めた応答を返す。
//! 応答のたびに接続を閉じる（`Connection: close`）。TLS のハンドシェイク（最初のバイトが 0x16）が届いたら、
//! 読むのをやめて応答（TLS でない文字列）を返す。

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// 届いた要求。
#[derive(Debug, Clone)]
pub(crate) struct RecordedRequest {
    /// 要求の行（例 `GET /a HTTP/1.1`）。TLS のハンドシェイクなら空。
    pub(crate) line: String,
    /// ヘッダー（名前は小文字）。
    pub(crate) headers: Vec<(String, String)>,
    /// TLS のハンドシェイクだった。
    pub(crate) tls: bool,
}

impl RecordedRequest {
    /// 名前が同じ最初のヘッダーの値。
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// 要求の対象（パスと問い合わせ）。
    pub(crate) fn target(&self) -> &str {
        self.line.split(' ').nth(1).unwrap_or("")
    }
}

type Responder = dyn Fn(&RecordedRequest) -> Vec<u8> + Send + Sync;

/// テスト用のサーバー。落とすと止まる。
pub(crate) struct LoopbackServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LoopbackServer {
    /// `respond` が要求ごとの応答（そのまま書くバイト列。[`response`] で作る）を返す。
    pub(crate) fn start(
        respond: impl Fn(&RecordedRequest) -> Vec<u8> + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let respond: Arc<Responder> = Arc::new(respond);
        let thread = {
            let (requests, connections, stop) =
                (requests.clone(), connections.clone(), stop.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    connections.fetch_add(1, Ordering::SeqCst);
                    serve(stream, &requests, respond.as_ref());
                }
            })
        };
        Self {
            addr,
            requests,
            connections,
            stop,
            thread: Some(thread),
        }
    }

    /// `path`（`/` で始まる）の http の URL。
    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// 同じポートの https の URL（このサーバーは TLS を話さない）。
    pub(crate) fn https_url(&self, path: &str) -> String {
        format!("https://{}{path}", self.addr)
    }

    /// 届いた要求（届いた順）。
    pub(crate) fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// 受け付けた接続の数。
    pub(crate) fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // accept の待ちを起こす。
        let _ = TcpStream::connect(self.addr);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(mut stream: TcpStream, requests: &Mutex<Vec<RecordedRequest>>, respond: &Responder) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let tls = loop {
        if buf.first() == Some(&0x16) {
            break true;
        }
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 64 * 1024 {
            break false;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break false,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let request = if tls {
        RecordedRequest {
            line: String::new(),
            headers: Vec::new(),
            tls: true,
        }
    } else {
        let head = String::from_utf8_lossy(&buf).into_owned();
        let head = head.split("\r\n\r\n").next().unwrap_or("");
        let mut lines = head.split("\r\n");
        let line = lines.next().unwrap_or("").to_string();
        let headers = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        RecordedRequest {
            line,
            headers,
            tls: false,
        }
    };
    requests.lock().unwrap().push(request.clone());
    let _ = stream.write_all(&respond(&request));
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
}

/// 応答のバイト列（`Content-Length` と `Connection: close` を足す）。`status` は `200 OK` などの形。
pub(crate) fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}
