//! カスタム URI スキーム `genzo://`（サムネイルと標準プレビューの JPEG。04 の 1.5 節）。
//!
//! | URI（macOS・Linux / Windows） | 内容 |
//! |---|---|
//! | `genzo://localhost/thumb/{variant_id}?rev={rev}` / `http://genzo.localhost/thumb/...` | サムネイル（L0。長辺 320） |
//! | `genzo://localhost/preview/{variant_id}?rev={rev}` / `http://genzo.localhost/preview/...` | 標準プレビュー（L1）。なければ作る処理を予約して 404 を返す（作ったら `thumbnail_updated` のイベント） |
//!
//! - `rev`（L0 のキャッシュキーのハッシュ）が付いていれば、内容は変わらないのでブラウザに長くキャッシュ
//!   させる（`immutable`）。現像設定を変えると `rev` が変わり、新しい URI になる。
//! - JPEG は Display P3 の ICC を埋め込んである（genzo-api の `previews`）。モニターへの変換は WebView が
//!   行う。
//! - 処理はブロックしうる（カタログ・キャッシュの読み込み）ので、少数のスレッドで処理する（グリッドの
//!   スクロールで要求が多数来ても、スレッドを作りすぎない）。

use std::sync::{Arc, OnceLock};

use crossbeam_channel::{Sender, unbounded};
use genzo_api::VariantId;

use crate::state::AppState;

/// 要求を処理するスレッドの数。
///
/// **仮置き**: 4（サムネイルは JPEG のバイト列を返すだけで軽い。標準プレビューの読み込みも 1 MB 程度）。
pub const PROTOCOL_THREADS: usize = 4;

/// 応答（Tauri に依存しない形）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// HTTP の状態。
    pub status: u16,
    /// Content-Type。
    pub content_type: &'static str,
    /// Cache-Control。
    pub cache_control: &'static str,
    /// 本文。
    pub body: Vec<u8>,
}

impl Reply {
    fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: "text/plain",
            cache_control: "no-store",
            body: Vec::new(),
        }
    }

    fn jpeg(body: Vec<u8>, immutable: bool) -> Self {
        Self {
            status: 200,
            content_type: "image/jpeg",
            cache_control: if immutable {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            },
            body,
        }
    }
}

/// 要求の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// サムネイル（L0）。
    Thumb(VariantId),
    /// 標準プレビュー（L1）。
    Preview(VariantId),
}

/// パス（`/thumb/12`）を解釈する。
pub fn parse(path: &str) -> Option<Route> {
    let mut parts = path.trim_start_matches('/').split('/');
    let kind = parts.next()?;
    let id: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || id <= 0 {
        return None;
    }
    let id = VariantId::new(id);
    match kind {
        "thumb" => Some(Route::Thumb(id)),
        "preview" => Some(Route::Preview(id)),
        _ => None,
    }
}

/// 要求に応える。
pub fn reply(state: &AppState, path: &str, has_rev: bool) -> Reply {
    let Some(route) = parse(path) else {
        return Reply::empty(400);
    };
    let Ok(core) = state.core() else {
        return Reply::empty(503);
    };
    match route {
        Route::Thumb(id) => match core.thumbnail(id) {
            Ok(Some(t)) => Reply::jpeg(t.jpeg, has_rev),
            Ok(None) => Reply::empty(404),
            Err(e) => {
                tracing::debug!(%e, ?id, "サムネイルを返せない");
                Reply::empty(if e.kind() == genzo_api::ErrorKind::NotFound {
                    404
                } else {
                    500
                })
            }
        },
        Route::Preview(id) => match core.request_preview(id) {
            Ok(Some(path)) => match std::fs::read(&path) {
                Ok(bytes) => Reply::jpeg(bytes, has_rev),
                // キャッシュの回収で消えた直後など。作り直しを予約して 404 にする。
                Err(_) => {
                    let _ = core.request_preview(id);
                    Reply::empty(404)
                }
            },
            Ok(None) => Reply::empty(404),
            Err(e) => {
                tracing::debug!(%e, ?id, "標準プレビューを返せない");
                Reply::empty(if e.kind() == genzo_api::ErrorKind::NotFound {
                    404
                } else {
                    500
                })
            }
        },
    }
}

type Job = Box<dyn FnOnce() + Send>;

/// 要求を処理するスレッドに仕事を渡す。
pub fn submit(job: Job) {
    static POOL: OnceLock<Sender<Job>> = OnceLock::new();
    let tx = POOL.get_or_init(|| {
        let (tx, rx) = unbounded::<Job>();
        for i in 0..PROTOCOL_THREADS {
            let rx = rx.clone();
            let _ = std::thread::Builder::new()
                .name(format!("genzo-protocol-{i}"))
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        job();
                    }
                });
        }
        tx
    });
    let _ = tx.send(job);
}

/// Tauri の非同期の URI スキームの処理。
pub fn handle<R: tauri::Runtime>(
    ctx: tauri::UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    use tauri::Manager;
    let state: Arc<AppState> = Arc::clone(&ctx.app_handle().state::<Arc<AppState>>());
    let path = request.uri().path().to_owned();
    let has_rev = request.uri().query().is_some_and(|q| {
        q.split('&')
            .any(|kv| kv.starts_with("rev=") && kv.len() > 4)
    });
    submit(Box::new(move || {
        let r = reply(&state, &path, has_rev);
        let response = tauri::http::Response::builder()
            .status(r.status)
            .header("Content-Type", r.content_type)
            .header("Cache-Control", r.cache_control)
            .body(r.body);
        match response {
            Ok(resp) => responder.respond(resp),
            Err(e) => tracing::error!(error = %e, "URI スキームの応答を作れない"),
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_routes() {
        assert_eq!(parse("/thumb/12"), Some(Route::Thumb(VariantId::new(12))));
        assert_eq!(parse("preview/3"), Some(Route::Preview(VariantId::new(3))));
        assert_eq!(parse("/thumb/0"), None);
        assert_eq!(parse("/thumb/-1"), None);
        assert_eq!(parse("/thumb/x"), None);
        assert_eq!(parse("/thumb/1/2"), None);
        assert_eq!(parse("/other/1"), None);
        assert_eq!(parse("/"), None);
    }
}
