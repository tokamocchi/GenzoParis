//! カスタム URI スキーム `genzo://`（サムネイルと標準プレビューの JPEG。04 の 1.5 節）。
//!
//! | URI（macOS・Linux / Windows） | 内容 |
//! |---|---|
//! | `genzo://localhost/thumb/{variant_id}?rev={rev}` / `http://genzo.localhost/thumb/...` | サムネイル（L0。長辺 320） |
//! | `genzo://localhost/preview/{variant_id}?rev={rev}` / `http://genzo.localhost/preview/...` | 標準プレビュー（L1）。なければ作る処理を予約して 404 を返す（作ったら `thumbnail_updated` のイベント） |
//!
//! - `rev`（L0 のキャッシュキーのハッシュ）が今の内容のものなら、その URI の内容は変わらないので、ブラウザに
//!   長くキャッシュさせる（`immutable`）。現像設定を変えると `rev` が変わり、新しい URI になる。
//! - `rev` が今の内容のものでなければ（検索結果の `thumb_rev` が古いうちに要求したなど）、今の内容を返すが、
//!   長くはキャッシュさせない（`no-cache`）。古い `rev` の URI に新しい内容をキャッシュさせると、設定を元に
//!   戻して `rev` が戻ったときに、ブラウザが違う内容を表示し続けるため。標準プレビュー（L1）は L0 と同時に
//!   作り直すので、L0 の `rev` と比べる。
//! - カタログが開いていない・閉じている途中なら 503、ないものは 404、URI の形が違えば 400。
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

/// エラーの HTTP の状態（ないものは 404、閉じている途中は 503、それ以外は 500）。
fn error_status(e: &genzo_api::ApiError) -> u16 {
    match e.kind() {
        genzo_api::ErrorKind::NotFound => 404,
        genzo_api::ErrorKind::Closed => 503,
        _ => 500,
    }
}

/// URI の問い合わせ（`rev=...&...`）の `rev` の値（空なら `None`）。
pub fn query_rev(query: Option<&str>) -> Option<&str> {
    query?
        .split('&')
        .find_map(|kv| kv.strip_prefix("rev="))
        .filter(|v| !v.is_empty())
}

/// 要求に応える。`rev` は URI の `rev`（なければ `None`）。
pub fn reply(state: &AppState, path: &str, rev: Option<&str>) -> Reply {
    let Some(route) = parse(path) else {
        return Reply::empty(400);
    };
    let Ok(core) = state.core() else {
        return Reply::empty(503);
    };
    match route {
        Route::Thumb(id) => match core.thumbnail(id) {
            Ok(Some(t)) => {
                let immutable = rev == Some(t.rev.as_str());
                Reply::jpeg(t.jpeg, immutable)
            }
            Ok(None) => Reply::empty(404),
            Err(e) => {
                tracing::debug!(%e, ?id, "サムネイルを返せない");
                Reply::empty(error_status(&e))
            }
        },
        Route::Preview(id) => match core.request_preview(id) {
            Ok(Some(path)) => match std::fs::read(&path) {
                Ok(bytes) => {
                    // 読み終えてから、今の L0 の rev と比べる（読む前に作り直されていれば、L0 の rev も
                    // 変わっているので、長くキャッシュさせない）。
                    let immutable = rev.is_some_and(|r| {
                        core.thumbnail(id)
                            .ok()
                            .flatten()
                            .is_some_and(|t| t.rev == r)
                    });
                    Reply::jpeg(bytes, immutable)
                }
                // キャッシュの回収で消えた直後など。作り直しを予約して 404 にする。
                Err(_) => {
                    let _ = core.request_preview(id);
                    Reply::empty(404)
                }
            },
            Ok(None) => Reply::empty(404),
            Err(e) => {
                tracing::debug!(%e, ?id, "標準プレビューを返せない");
                Reply::empty(error_status(&e))
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
    let rev = query_rev(request.uri().query()).map(str::to_owned);
    submit(Box::new(move || {
        let r = reply(&state, &path, rev.as_deref());
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

    #[test]
    fn reads_the_rev_of_the_query() {
        assert_eq!(query_rev(Some("rev=ab12")), Some("ab12"));
        assert_eq!(query_rev(Some("x=1&rev=ab12&y=2")), Some("ab12"));
        assert_eq!(query_rev(Some("rev=")), None);
        assert_eq!(query_rev(Some("prev=ab12")), None);
        assert_eq!(query_rev(Some("")), None);
        assert_eq!(query_rev(None), None);
    }
}
