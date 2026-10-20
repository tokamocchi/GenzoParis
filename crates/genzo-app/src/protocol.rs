//! カスタム URI スキーム `genzo://`（サムネイルと標準プレビューの JPEG、動画の再生。04 の 1.5 節・4.2 節）。
//!
//! | URI（macOS・Linux / Windows） | 内容 |
//! |---|---|
//! | `genzo://localhost/thumb/{variant_id}?rev={rev}` / `http://genzo.localhost/thumb/...` | サムネイル（L0。長辺 320） |
//! | `genzo://localhost/preview/{variant_id}?rev={rev}` / `http://genzo.localhost/preview/...` | 標準プレビュー（L1）。なければ作る処理を予約して 404 を返す（作ったら `thumbnail_updated` のイベント） |
//! | `genzo://localhost/video/{variant_id}` / `http://genzo.localhost/video/...` | 動画の元のファイル（VID-04。Range の要求に応える） |
//! | `genzo://localhost/video-proxy/{variant_id}` / `http://genzo.localhost/video-proxy/...` | 動画の再生用のファイル（H.264 ＋ AAC の MP4。なければ 404） |
//! | `genzo://localhost/tile/{z}/{x}/{y}` / `http://genzo.localhost/tile/...` | 地図のタイル（PNG。オンラインの地図が有効なときだけ。LIB-21。[`reply_tile`]） |
//!
//! 動画（[`reply_media`]）:
//!
//! - 配信するのは、カタログに登録された動画（種類が動画で、拡張子が `mp4`・`mov`）の元のファイルと、コアの
//!   キャッシュの再生用のファイルだけ（genzo-api の `Core::video_stream`。URI は variant の ID だけを受け取り、
//!   パスを受け取らない。任意のファイルは読めない）。元のファイルは読むだけ（DATA-01）。
//! - `Range: bytes=…` に 206 と `Content-Range` で応える。1 回の応答は [`MAX_MEDIA_CHUNK`] まで（ファイル全体を
//!   メモリに読まない。WebView は続きを要求し直す）。範囲がなければ（WebView の動画の読み込みは通常範囲を付ける）、
//!   [`MAX_MEDIA_CHUNK`] 以下のファイルは 200 で全体を、大きいファイルは先頭の 206 を返す。範囲がファイルの外なら
//!   416。複数の範囲は最初の範囲だけに応える。`HEAD` は大きさだけ。
//! - 大きいので、WebView にキャッシュさせない（`no-store`）。パスの問い合わせ（カタログのロック）を要求ごとに
//!   しないよう、セッションごとに数秒だけ覚える（[`MediaCache`]）。
//!
//! - サムネイル: `rev`（L0 のキャッシュキーのハッシュ）が今の内容のものなら、その URI の内容は変わらないので、
//!   ブラウザに長くキャッシュさせる（`immutable`）。現像設定を変えると `rev` が変わり、新しい URI になる。
//!   `rev` が今の内容のものでなければ（検索結果の `thumb_rev` が古いうちに要求したなど）、今の内容を返すが、
//!   長くはキャッシュさせない（`no-cache`）。古い `rev` の URI に新しい内容をキャッシュさせると、設定を元に
//!   戻して `rev` が戻ったときに、ブラウザが違う内容を表示し続けるため。
//! - 標準プレビュー: 常に `no-cache`（長くキャッシュさせない）。URI の `rev` は L0 のものだが、返す L1 は
//!   「今の現像設定」のキーでキャッシュから引くので、L0 の `rev` と L1 の内容が食い違う間がある。設定を
//!   A → B → A と戻すと、A の L1 はキャッシュ（LRU）に残っていてすぐ返る一方、L0 は作り直しのジョブが保存する
//!   まで B のまま。この間に `rev` = B の URI へ A の L1 を長くキャッシュさせると、後で設定を B にしたときに、
//!   ブラウザが A の画像を表示し続ける。L1 のキーを URI に入れて比べるには、コア API に L1 のキー（または今の
//!   設定から求めた L0 のキー）を返す口が要る（genzo-api。今はない）。
//! - カタログが開いていない・閉じている途中なら 503（地図のタイルを除く）、ないものは 404、URI の形が違えば 400。
//! - JPEG は Display P3 の ICC を埋め込んである（genzo-api の `previews`）。モニターへの変換は WebView が
//!   行う。
//! - 処理はブロックしうる（カタログ・キャッシュの読み込み）ので、少数のスレッドで処理する（グリッドの
//!   スクロールで要求が多数来ても、スレッドを作りすぎない）。
//!
//! 地図のタイル（[`reply_tile`]。位置情報の後付け LIB-21 の第 2 段階。[`crate::map`]）:
//!
//! - カタログによらない（カタログが開いていなくても返す。カタログの 503 より前で分ける）。
//! - オンラインの地図が無効なら 403 で、通信しない（docs/02 の SEC-02）。キャッシュにあっても返さない。
//! - URI の番号の形が違う・範囲外（`x`・`y` が `2^z` 以上、ズームが genzo-geo の `MAX_ZOOM_LIMIT` を超える）なら
//!   400。提供元の最大のズームを超えるズームも 400（genzo-geo）。上流の 404 は 404、通信の失敗・上流のエラー・
//!   不正な応答は 502（期限の切れたキャッシュがあれば、代わりにそれを返す。genzo-geo の stale-on-error）。
//! - 本文は上流の PNG のまま返す（デコードしない。SEC-05）。キャッシュの期限は Rust の側のキャッシュ（genzo-geo。
//!   ディスク。ディスクがない・書けなければメモリー）が HTTP のキャッシュの指示に従って管理するので、WebView には
//!   長くキャッシュさせない
//!   （`no-cache`。期限の切れたキャッシュを代わりに返したときは `no-store`）。提供元を設定で変えても URI は
//!   同じなので、WebView に前の提供元のタイルを残さないためでもある。
//! - 通信を待つので、サムネイルなどとは別の、タイル専用のスレッド（[`TILE_THREADS`]）で処理する
//!   （[`submit_tile`]）。待ちの列は新しい要求から処理し（今表示している範囲を先に）、[`MAX_QUEUED_TILES`] を
//!   超えたら古い要求を 503 で捨てる（地図を大きく動かした後に、もう見ていない範囲のタイルを取りに行かない
//!   ため。画面の地図（Leaflet）がタイルの読み込みをやめても、この処理には取り消しが伝わらない）。

use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, unbounded};
use genzo_api::{VariantId, VideoStream};
use genzo_geo::GeoError;
use parking_lot::{Condvar, Mutex};

use crate::state::AppState;

/// 要求を処理するスレッドの数。
///
/// **仮置き**: 4（サムネイルは JPEG のバイト列を返すだけで軽い。標準プレビューの読み込みも 1 MB 程度）。
pub const PROTOCOL_THREADS: usize = 4;

/// 動画の 1 回の応答の本文の上限（バイト。**仮置き**: 4 MiB。100 Mbps（4K の HEVC の上限の目安）の動画でも
/// 1 秒あたり 3〜4 回の要求で済み、1 回の読み込みのメモリを抑える大きさ）。
pub const MAX_MEDIA_CHUNK: u64 = 4 * 1024 * 1024;

/// 配信する動画のファイルを覚えておく時間（**仮置き**: 5 秒。再生中の続きの要求のたびに、カタログのロックを
/// 取って問い合わせないため。ファイルを動かした直後の要求は、古いパスを開けずに 404 になり、覚えたものを捨てる）。
pub const MEDIA_CACHE_TTL: Duration = Duration::from_secs(5);

/// 覚えておく動画のファイルの数。
const MEDIA_CACHE_ENTRIES: usize = 8;

/// 地図のタイルを処理するスレッドの数（**仮置き**: 2。上流のタイルのサーバーに同時に送る要求の数の上限にも
/// なる。OSM のタイルの利用ポリシーの原文に数値はない）。
pub const TILE_THREADS: usize = 2;

/// 待ちの列に置く地図のタイルの要求の上限（**仮置き**: 64。ダイアログの地図が 1 回に表示するタイル（十数枚）より
/// 十分に多い数。超えたら古い要求を 503 で捨てる）。
pub const MAX_QUEUED_TILES: usize = 64;

/// 応答（Tauri に依存しない形）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// HTTP の状態。
    pub status: u16,
    /// Content-Type。
    pub content_type: &'static str,
    /// Cache-Control。
    pub cache_control: &'static str,
    /// ほかの見出し（`Content-Range`・`Accept-Ranges`・`Content-Length` など）。
    pub headers: Vec<(&'static str, String)>,
    /// 本文。
    pub body: Vec<u8>,
}

impl Reply {
    fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: "text/plain",
            cache_control: "no-store",
            headers: Vec::new(),
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
            headers: Vec::new(),
            body,
        }
    }

    /// 見出しの値（なければ `None`。名前は大文字・小文字を区別しない）。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// 要求の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// サムネイル（L0）。
    Thumb(VariantId),
    /// 標準プレビュー（L1）。
    Preview(VariantId),
    /// 動画の元のファイル。
    Video(VariantId),
    /// 動画の再生用のファイル。
    VideoProxy(VariantId),
    /// 地図のタイル（ズーム `z` の `(x, y)`。オンラインの地図が有効なときだけ。カタログによらない）。
    Tile {
        /// ズーム。
        z: u8,
        /// 横の番号（`2^z` 未満）。
        x: u32,
        /// 縦の番号（`2^z` 未満）。
        y: u32,
    },
}

/// パス（`/thumb/12`・`/tile/5/28/12`）を解釈する。
pub fn parse(path: &str) -> Option<Route> {
    let mut parts = path.trim_start_matches('/').split('/');
    let kind = parts.next()?;
    if kind == "tile" {
        return parse_tile(parts);
    }
    let id: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || id <= 0 {
        return None;
    }
    let id = VariantId::new(id);
    match kind {
        "thumb" => Some(Route::Thumb(id)),
        "preview" => Some(Route::Preview(id)),
        "video" => Some(Route::Video(id)),
        "video-proxy" => Some(Route::VideoProxy(id)),
        _ => None,
    }
}

/// 地図のタイルのパスの `{z}/{x}/{y}`（10 進の数字だけ。拡張子・符号は付けない）。ズームは genzo-geo の
/// `MAX_ZOOM_LIMIT` まで、`x`・`y` は `2^z` 未満（提供元の最大のズームは [`reply_tile`] で genzo-geo が確かめる）。
fn parse_tile<'a>(mut parts: impl Iterator<Item = &'a str>) -> Option<Route> {
    fn number(s: Option<&str>) -> Option<u32> {
        let s = s?;
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    }
    let z = number(parts.next())?;
    let x = number(parts.next())?;
    let y = number(parts.next())?;
    if parts.next().is_some() {
        return None;
    }
    let z = u8::try_from(z)
        .ok()
        .filter(|z| *z <= genzo_geo::MAX_ZOOM_LIMIT)?;
    let n = 1u64 << z;
    (u64::from(x) < n && u64::from(y) < n).then_some(Route::Tile { z, x, y })
}

/// 要求の範囲の解釈（[`parse_range`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRange {
    /// 範囲の指定がない（または解釈できないので無視する。RFC 9110 の 14.2 節）。
    Whole,
    /// `start` から `end` まで（両端を含む。ファイルの中に収めたもの）。
    Part {
        /// 最初のバイト。
        start: u64,
        /// 最後のバイト。
        end: u64,
    },
    /// ファイルの外（416）。
    Unsatisfiable,
}

/// `Range` の見出し（`bytes=0-`・`bytes=100-199`・`bytes=-500`）を、大きさ `len` のファイルの範囲にする。
///
/// 複数の範囲（`bytes=0-1,5-9`）は最初の範囲だけを使う。`bytes` 以外の単位・形の違う値は、範囲がないものとして
/// 扱う（全体を返してよい）。終わりはファイルの最後に切り詰める。
pub fn parse_range(header: Option<&str>, len: u64) -> ByteRange {
    let Some(h) = header else {
        return ByteRange::Whole;
    };
    let Some(spec) = h.trim().strip_prefix("bytes=") else {
        return ByteRange::Whole;
    };
    let first = spec.split(',').next().unwrap_or_default().trim();
    let Some((a, b)) = first.split_once('-') else {
        return ByteRange::Whole;
    };
    let (a, b) = (a.trim(), b.trim());
    let num = |s: &str| -> Option<u64> {
        (!s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    match (a.is_empty(), num(a), num(b)) {
        // 末尾の n バイト。
        (true, _, Some(n)) => {
            if n == 0 || len == 0 {
                ByteRange::Unsatisfiable
            } else {
                ByteRange::Part {
                    start: len.saturating_sub(n),
                    end: len - 1,
                }
            }
        }
        (false, Some(start), end) if b.is_empty() || end.is_some() => {
            if start >= len {
                return ByteRange::Unsatisfiable;
            }
            let end = end.unwrap_or(u64::MAX).min(len - 1);
            if end < start {
                ByteRange::Whole
            } else {
                ByteRange::Part { start, end }
            }
        }
        _ => ByteRange::Whole,
    }
}

/// 配信する動画のファイルを、セッション（開いているカタログ）ごとに少しの間だけ覚える。
#[derive(Debug, Default)]
pub struct MediaCache {
    entries: Mutex<Vec<(VariantId, bool, VideoStream, Instant)>>,
}

impl MediaCache {
    /// 覚えているもの（[`MEDIA_CACHE_TTL`] 以内）を返し、なければ `fetch` で求めて覚える。
    pub fn get_or_fetch(
        &self,
        id: VariantId,
        proxy: bool,
        fetch: impl FnOnce() -> Result<VideoStream, genzo_api::ApiError>,
    ) -> Result<VideoStream, genzo_api::ApiError> {
        let now = Instant::now();
        {
            let mut e = self.entries.lock();
            e.retain(|(_, _, _, at)| now.duration_since(*at) < MEDIA_CACHE_TTL);
            if let Some((_, _, s, _)) = e.iter().find(|(i, p, _, _)| *i == id && *p == proxy) {
                return Ok(s.clone());
            }
        }
        // 問い合わせ（カタログのロック）は、このロックの外で行う。
        let s = fetch()?;
        let mut e = self.entries.lock();
        e.retain(|(i, p, _, _)| !(*i == id && *p == proxy));
        if e.len() >= MEDIA_CACHE_ENTRIES {
            e.remove(0);
        }
        e.push((id, proxy, s.clone(), now));
        Ok(s)
    }

    /// 覚えたものを捨てる（開けなかったとき）。
    pub fn forget(&self, id: VariantId, proxy: bool) {
        self.entries
            .lock()
            .retain(|(i, p, _, _)| !(*i == id && *p == proxy));
    }
}

/// 動画の MIME の型（[`Reply::content_type`] の `&'static str` にする。ほかは `application/octet-stream`）。
fn static_video_type(t: &str) -> &'static str {
    match t {
        "video/mp4" => "video/mp4",
        "video/quicktime" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// 動画のファイルを読む（`Range` の見出し `range` の範囲を [`MAX_MEDIA_CHUNK`] まで）。範囲は、開いた時点の
/// 大きさで解釈する（覚えていた [`VideoStream::size`] は古いことがある）。
fn read_media(stream: &VideoStream, range: Option<&str>, head: bool) -> std::io::Result<Reply> {
    let mut file = std::fs::File::open(&stream.path)?;
    let len = file.metadata()?.len();
    let range = parse_range(range, len);
    let content_type = static_video_type(&stream.content_type);
    let mut headers = vec![("Accept-Ranges", "bytes".to_owned())];
    let range = match range {
        // 範囲がなく、小さいファイルは全体を返す。
        ByteRange::Whole if len <= MAX_MEDIA_CHUNK => {
            headers.push(("Content-Length", len.to_string()));
            let mut body = Vec::new();
            if !head {
                file.read_to_end(&mut body)?;
            }
            return Ok(Reply {
                status: 200,
                content_type,
                cache_control: "no-store",
                headers,
                body,
            });
        }
        // 範囲がなく大きいファイルは、先頭を部分の応答で返す（全体をメモリに読まない）。
        ByteRange::Whole => ByteRange::Part {
            start: 0,
            end: len - 1,
        },
        r => r,
    };
    match range {
        ByteRange::Part { start, end } => {
            let end = end.min(start + MAX_MEDIA_CHUNK - 1);
            let n = end - start + 1;
            headers.push(("Content-Range", format!("bytes {start}-{end}/{len}")));
            headers.push(("Content-Length", n.to_string()));
            let mut body = Vec::new();
            if !head {
                file.seek(SeekFrom::Start(start))?;
                file.take(n).read_to_end(&mut body)?;
            }
            Ok(Reply {
                status: 206,
                content_type,
                cache_control: "no-store",
                headers,
                body,
            })
        }
        _ => Ok(Reply {
            status: 416,
            content_type: "text/plain",
            cache_control: "no-store",
            headers: vec![("Content-Range", format!("bytes */{len}"))],
            body: Vec::new(),
        }),
    }
}

/// 動画の要求に応える（`range` は `Range` の見出し、`head` は `HEAD` の要求か）。
pub fn reply_media(state: &AppState, route: Route, range: Option<&str>, head: bool) -> Reply {
    let (id, proxy) = match route {
        Route::Video(id) => (id, false),
        Route::VideoProxy(id) => (id, true),
        _ => return Reply::empty(400),
    };
    let Some(session) = state.session() else {
        return Reply::empty(503);
    };
    let stream = match session
        .media
        .get_or_fetch(id, proxy, || session.core.video_stream(id, proxy))
    {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(%e, ?id, proxy, "動画を返せない");
            return Reply::empty(error_status(&e));
        }
    };
    match read_media(&stream, range, head) {
        Ok(reply) => reply,
        Err(e) => {
            session.media.forget(id, proxy);
            tracing::debug!(error = %e, path = %stream.path.display(), "動画のファイルを読めない");
            Reply::empty(if e.kind() == std::io::ErrorKind::NotFound {
                404
            } else {
                500
            })
        }
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

/// 地図のタイルの要求に応える（モジュールの doc の「地図のタイル」。カタログによらない）。オンラインの地図が
/// 無効なら 403 で、通信しない。
pub fn reply_tile(state: &AppState, z: u8, x: u32, y: u32) -> Reply {
    let preferences = state.settings().map;
    if !preferences.online {
        return Reply::empty(403);
    }
    let tiles = match state.map().tiles(&preferences) {
        Ok(tiles) => tiles,
        Err(e) => {
            // settings.json を手で書き換えた場合など（保存のコマンドは使えない URL を保存しない）。
            tracing::warn!(error = %e, "設定の地図のタイルの URL が使えない");
            return Reply::empty(500);
        }
    };
    match tiles.tile(z, x, y) {
        Ok(tile) => Reply {
            status: 200,
            content_type: tile.content_type,
            // 期限は Rust の側のキャッシュが管理する（モジュールの doc）。
            cache_control: if tile.stale { "no-store" } else { "no-cache" },
            headers: Vec::new(),
            body: tile.bytes,
        },
        Err(e) => {
            // タイルの番号は見ている場所がわかるので、info 以上で書かない（決定事項ログ No.75 の方針）。
            tracing::debug!(error = %e, "地図のタイルを返せない");
            Reply::empty(tile_error_status(&e))
        }
    }
}

/// 地図のタイルのエラーの HTTP の状態。
fn tile_error_status(e: &GeoError) -> u16 {
    match e {
        GeoError::Disabled => 403,
        GeoError::InvalidInput(_) => 400,
        GeoError::Status(404) => 404,
        GeoError::Network(_) | GeoError::Status(_) | GeoError::BadResponse(_) => 502,
        GeoError::Io { .. } => 500,
    }
}

/// 要求に応える。`rev` は URI の `rev`（なければ `None`。サムネイルだけが使う）。
pub fn reply(state: &AppState, path: &str, rev: Option<&str>) -> Reply {
    let Some(route) = parse(path) else {
        return Reply::empty(400);
    };
    // 地図のタイルはカタログによらないので、カタログの 503 より前で分ける。
    if let Route::Tile { z, x, y } = route {
        return reply_tile(state, z, x, y);
    }
    if matches!(route, Route::Video(_) | Route::VideoProxy(_)) {
        return reply_media(state, route, None, false);
    }
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
                // L0 の rev と L1 の内容は食い違いうるので、rev によらず長くキャッシュさせない（モジュールの doc）。
                Ok(bytes) => Reply::jpeg(bytes, false),
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
        Route::Video(_) | Route::VideoProxy(_) | Route::Tile { .. } => {
            unreachable!("上で返した")
        }
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

/// 地図のタイルの要求の待ちの列（新しい要求から処理する。上限を超えたら古い要求を外して返す）。
struct TileQueue<T> {
    jobs: Mutex<VecDeque<T>>,
    ready: Condvar,
    limit: usize,
}

impl<T> TileQueue<T> {
    fn new(limit: usize) -> Self {
        Self {
            jobs: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            limit,
        }
    }

    /// 置く。上限を超えたら、いちばん古い要求を外して返す（呼び出し元が 503 で答える）。
    fn push(&self, job: T) -> Option<T> {
        let mut jobs = self.jobs.lock();
        jobs.push_back(job);
        let dropped = if jobs.len() > self.limit {
            jobs.pop_front()
        } else {
            None
        };
        self.ready.notify_one();
        dropped
    }

    /// いちばん新しい要求を取る（なければ来るまで待つ）。
    fn pop(&self) -> T {
        let mut jobs = self.jobs.lock();
        loop {
            if let Some(job) = jobs.pop_back() {
                return job;
            }
            self.ready.wait(&mut jobs);
        }
    }
}

/// 地図のタイルの要求（応え方は呼び出し元が決める。Tauri では `UriSchemeResponder`）。
struct TileJob {
    state: Arc<AppState>,
    tile: (u8, u32, u32),
    respond: Box<dyn FnOnce(Reply) + Send>,
}

/// 地図のタイルの要求を、タイル専用のスレッド（[`TILE_THREADS`]）に渡す（モジュールの doc の「地図の
/// タイル」）。待ちの列が [`MAX_QUEUED_TILES`] を超えたら、いちばん古い要求に 503 で答える。`respond` は
/// [`reply_tile`] の応答で 1 回だけ呼ばれる（処理が panic したら 500）。
pub fn submit_tile(
    state: Arc<AppState>,
    (z, x, y): (u8, u32, u32),
    respond: Box<dyn FnOnce(Reply) + Send>,
) {
    static QUEUE: OnceLock<Arc<TileQueue<TileJob>>> = OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let queue = Arc::new(TileQueue::<TileJob>::new(MAX_QUEUED_TILES));
        for i in 0..TILE_THREADS {
            let queue = Arc::clone(&queue);
            let _ = std::thread::Builder::new()
                .name(format!("genzo-tile-{i}"))
                .spawn(move || {
                    loop {
                        let job = queue.pop();
                        let (z, x, y) = job.tile;
                        // 1 枚の失敗でスレッドを失わない（ほかのタイルの要求が答えを待ち続けないように）。
                        let reply = std::panic::catch_unwind(AssertUnwindSafe(|| {
                            reply_tile(&job.state, z, x, y)
                        }))
                        .unwrap_or_else(|_| Reply::empty(500));
                        (job.respond)(reply);
                    }
                });
        }
        queue
    });
    let job = TileJob {
        state,
        tile: (z, x, y),
        respond,
    };
    if let Some(dropped) = queue.push(job) {
        (dropped.respond)(Reply::empty(503));
    }
}

/// 応答を WebView に返す。
fn respond(responder: tauri::UriSchemeResponder, r: Reply) {
    let mut builder = tauri::http::Response::builder()
        .status(r.status)
        .header("Content-Type", r.content_type)
        .header("Cache-Control", r.cache_control);
    for (name, value) in &r.headers {
        builder = builder.header(*name, value);
    }
    match builder.body(r.body) {
        Ok(resp) => responder.respond(resp),
        Err(e) => tracing::error!(error = %e, "URI スキームの応答を作れない"),
    }
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
    let route = parse(&path);
    // 地図のタイルは通信を待つので、サムネイルなどとは別のスレッドで処理する（モジュールの doc）。
    if let Some(Route::Tile { z, x, y }) = route {
        submit_tile(state, (z, x, y), Box::new(move |r| respond(responder, r)));
        return;
    }
    let rev = query_rev(request.uri().query()).map(str::to_owned);
    let range = request
        .headers()
        .get("range")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let head = request.method() == tauri::http::Method::HEAD;
    submit(Box::new(move || {
        let r = match route {
            Some(route @ (Route::Video(_) | Route::VideoProxy(_))) => {
                reply_media(&state, route, range.as_deref(), head)
            }
            _ => reply(&state, &path, rev.as_deref()),
        };
        respond(responder, r);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_routes() {
        assert_eq!(parse("/thumb/12"), Some(Route::Thumb(VariantId::new(12))));
        assert_eq!(parse("preview/3"), Some(Route::Preview(VariantId::new(3))));
        assert_eq!(parse("/video/7"), Some(Route::Video(VariantId::new(7))));
        assert_eq!(
            parse("/video-proxy/7"),
            Some(Route::VideoProxy(VariantId::new(7)))
        );
        assert_eq!(parse("/video/0"), None);
        assert_eq!(parse("/video/../../etc/passwd"), None);
        assert_eq!(parse("/video/%2Fetc%2Fpasswd"), None);
        assert_eq!(parse("/thumb/0"), None);
        assert_eq!(parse("/thumb/-1"), None);
        assert_eq!(parse("/thumb/x"), None);
        assert_eq!(parse("/thumb/1/2"), None);
        assert_eq!(parse("/other/1"), None);
        assert_eq!(parse("/"), None);
    }

    #[test]
    fn parses_tile_routes() {
        assert_eq!(parse("/tile/0/0/0"), Some(Route::Tile { z: 0, x: 0, y: 0 }));
        assert_eq!(
            parse("/tile/5/28/12"),
            Some(Route::Tile { z: 5, x: 28, y: 12 })
        );
        assert_eq!(
            parse("/tile/19/524287/0"),
            Some(Route::Tile {
                z: 19,
                x: 524_287,
                y: 0
            })
        );
        // 番号は 2^z 未満、ズームは genzo-geo の上限まで。
        let top = genzo_geo::MAX_ZOOM_LIMIT;
        assert!(parse(&format!("/tile/{top}/0/0")).is_some());
        for bad in [
            "/tile/1/2/0",
            "/tile/1/0/2",
            "/tile/0/1/0",
            &format!("/tile/{}/0/0", top + 1),
            "/tile/256/0/0",
            "/tile/19/524288/0",
            // 形の違うもの（拡張子・符号・空・足りない・多い・数字でない）。
            "/tile/1/0/0.png",
            "/tile/+1/0/0",
            "/tile/-1/0/0",
            "/tile/1/0/",
            "/tile//0/0",
            "/tile/1/0",
            "/tile/1/0/0/0",
            "/tile/a/0/0",
            "/tile/1/0/99999999999",
            "/tile",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn tile_errors_get_statuses() {
        assert_eq!(tile_error_status(&GeoError::Disabled), 403);
        assert_eq!(
            tile_error_status(&GeoError::InvalidInput("範囲外".into())),
            400
        );
        assert_eq!(tile_error_status(&GeoError::Status(404)), 404);
        for e in [
            GeoError::Status(503),
            GeoError::Status(429),
            GeoError::Network("接続できない".into()),
            GeoError::BadResponse("PNG ではない".into()),
        ] {
            assert_eq!(tile_error_status(&e), 502, "{e:?}");
        }
    }

    /// 待ちの列は新しい要求から処理し、上限を超えたら古い要求から外す。
    #[test]
    fn tile_queue_serves_the_newest_first_and_drops_the_oldest() {
        let q = TileQueue::new(3);
        assert_eq!(q.push(1), None);
        assert_eq!(q.push(2), None);
        assert_eq!(q.push(3), None);
        assert_eq!(q.push(4), Some(1));
        assert_eq!(q.pop(), 4);
        assert_eq!(q.push(5), None);
        assert_eq!((q.pop(), q.pop(), q.pop()), (5, 3, 2));
        // 空なら、来るまで待つ。
        let q = Arc::new(TileQueue::new(3));
        let waiter = {
            let q = Arc::clone(&q);
            std::thread::spawn(move || q.pop())
        };
        q.push(7);
        assert_eq!(waiter.join().unwrap(), 7);
    }

    #[test]
    fn parses_byte_ranges() {
        use ByteRange::*;
        assert_eq!(parse_range(None, 100), Whole);
        assert_eq!(
            parse_range(Some("bytes=0-"), 100),
            Part { start: 0, end: 99 }
        );
        assert_eq!(
            parse_range(Some("bytes=0-1"), 100),
            Part { start: 0, end: 1 }
        );
        assert_eq!(
            parse_range(Some("bytes=10-19"), 100),
            Part { start: 10, end: 19 }
        );
        assert_eq!(
            parse_range(Some("bytes=90-500"), 100),
            Part { start: 90, end: 99 }
        );
        assert_eq!(
            parse_range(Some("bytes=-30"), 100),
            Part { start: 70, end: 99 }
        );
        assert_eq!(
            parse_range(Some("bytes=-300"), 100),
            Part { start: 0, end: 99 }
        );
        assert_eq!(
            parse_range(Some(" bytes=5-9, 20-29"), 100),
            Part { start: 5, end: 9 }
        );
        assert_eq!(parse_range(Some("bytes=100-"), 100), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-0"), 100), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-"), 0), Unsatisfiable);
        // 形の違う値・ほかの単位・逆順は、範囲がないものとして扱う。
        for bad in [
            "items=0-5",
            "bytes=",
            "bytes=-",
            "bytes=a-b",
            "bytes=5",
            "bytes=9-3",
            "bytes=+1-2",
        ] {
            assert_eq!(parse_range(Some(bad), 100), Whole, "{bad}");
        }
    }

    fn stream_of(path: &std::path::Path) -> VideoStream {
        VideoStream {
            variant_id: VariantId::new(1),
            path: path.to_path_buf(),
            size: std::fs::metadata(path).unwrap().len(),
            content_type: "video/quicktime".to_owned(),
            proxy: false,
        }
    }

    #[test]
    fn media_is_read_in_bounded_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.mov");
        let len = MAX_MEDIA_CHUNK + 1000;
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        std::fs::write(&big, &data).unwrap();
        let s = stream_of(&big);
        // 範囲の要求: 206 と Content-Range。
        let r = read_media(&s, Some("bytes=10-19"), false).unwrap();
        assert_eq!(r.status, 206);
        assert_eq!(r.content_type, "video/quicktime");
        assert_eq!(
            r.header("content-range"),
            Some(format!("bytes 10-19/{len}").as_str())
        );
        assert_eq!(r.header("Content-Length"), Some("10"));
        assert_eq!(r.header("Accept-Ranges"), Some("bytes"));
        assert_eq!(r.body, &data[10..20]);
        assert_eq!(r.cache_control, "no-store");
        // 終わりのない範囲は、上限まで。
        let r = read_media(&s, Some("bytes=0-"), false).unwrap();
        assert_eq!(r.body.len() as u64, MAX_MEDIA_CHUNK);
        assert_eq!(
            r.header("Content-Range"),
            Some(format!("bytes 0-{}/{len}", MAX_MEDIA_CHUNK - 1).as_str())
        );
        // 範囲がなく大きいファイルは、先頭の部分の応答。
        let r = read_media(&s, None, false).unwrap();
        assert_eq!((r.status, r.body.len() as u64), (206, MAX_MEDIA_CHUNK));
        // 末尾。
        let r = read_media(&s, Some("bytes=-5"), false).unwrap();
        assert_eq!(r.body, &data[data.len() - 5..]);
        // ファイルの外は 416。HEAD は本文なし。
        let r = read_media(&s, Some(&format!("bytes={len}-")), false).unwrap();
        assert_eq!(r.status, 416);
        assert_eq!(
            r.header("Content-Range"),
            Some(format!("bytes */{len}").as_str())
        );
        let r = read_media(&s, Some("bytes=0-99"), true).unwrap();
        assert_eq!((r.status, r.body.len()), (206, 0));
        assert_eq!(r.header("Content-Length"), Some("100"));
        // 小さいファイルは、範囲がなければ全体を 200 で。
        let small = dir.path().join("small.mp4");
        std::fs::write(&small, b"0123456789").unwrap();
        let mut s = stream_of(&small);
        s.content_type = "video/mp4".to_owned();
        let r = read_media(&s, None, false).unwrap();
        assert_eq!((r.status, r.content_type), (200, "video/mp4"));
        assert_eq!(r.body, b"0123456789");
        // 覚えていた大きさによらず、開いた時点の大きさで答える。
        s.size = 1000;
        let r = read_media(&s, Some("bytes=5-99"), false).unwrap();
        assert_eq!(r.body, b"56789");
        assert_eq!(r.header("Content-Range"), Some("bytes 5-9/10"));
        let r = read_media(&s, Some("bytes=50-99"), false).unwrap();
        assert_eq!(r.status, 416);
        // ないファイルはエラー（呼び出し側で 404）。
        std::fs::remove_file(&small).unwrap();
        assert!(read_media(&s, None, false).is_err());
    }

    #[test]
    fn media_cache_remembers_for_a_while_and_can_forget() {
        let cache = MediaCache::default();
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.mp4");
        std::fs::write(&f, b"x").unwrap();
        let mut calls = 0;
        let fetch = |calls: &mut i32| {
            *calls += 1;
            Ok(stream_of(&f))
        };
        let id = VariantId::new(1);
        cache.get_or_fetch(id, false, || fetch(&mut calls)).unwrap();
        cache.get_or_fetch(id, false, || fetch(&mut calls)).unwrap();
        assert_eq!(calls, 1);
        // 再生用のファイルは別に覚える。
        cache.get_or_fetch(id, true, || fetch(&mut calls)).unwrap();
        assert_eq!(calls, 2);
        cache.forget(id, false);
        cache.get_or_fetch(id, false, || fetch(&mut calls)).unwrap();
        assert_eq!(calls, 3);
        // 求められなければ覚えない。
        let e = cache.get_or_fetch(VariantId::new(2), false, || {
            Err(genzo_api::ApiError::NotFound("x".into()))
        });
        assert!(e.is_err());
        for i in 3..20 {
            cache
                .get_or_fetch(VariantId::new(i), false, || fetch(&mut calls))
                .unwrap();
        }
        assert!(cache.entries.lock().len() <= MEDIA_CACHE_ENTRIES);
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
