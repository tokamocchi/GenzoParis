//! 地図のタイルの取得と、ディスクのキャッシュ。
//!
//! 画面の地図（Leaflet）は、表示している範囲のタイルを genzo-app のカスタムプロトコル経由で 1 枚ずつ求め、
//! genzo-app がここの [`TileService::tile`] を呼ぶ。タイルの利用ポリシーに合わせて次のようにする:
//! - キャッシュの期限は HTTP のキャッシュの指示に従う（読めなければ 7 日。src/freshness.rs の doc）。
//!   期限内はキャッシュから返し、通信しない。期限切れは条件付きの要求（`If-None-Match`・
//!   `If-Modified-Since`）にし、304 なら期限を延ばしてキャッシュを返す。
//! - 期限切れのキャッシュがあり、通信に失敗した（通信の誤りか 5xx）ときは、古いタイルを返す
//!   （stale-on-error。[`Tile::stale`]。`no-cache`・`must-revalidate` のタイルには使わない）。この扱いは
//!   人の確認待ち（ポリシーが明示して認めているのは「キャッシュの指示に従った再訪」だけ）。
//! - 先読み・一括取得の機能は作らない（ポリシーで禁止）。求められたタイルだけを取る。
//! - 同じタイルの同時の要求は 1 つにまとめる（地図を動かすと同じタイルが重ねて求められることがある）。
//! - 応答の `Content-Type` が `image/png` でなく、本文が PNG の署名で始まらなければ、保存せずに誤りにする。
//!   画像は **デコードしない**（SEC-05。WebView がデコードする）。
//! - 本文の上限は [`MAX_TILE_BYTES`]（仮置き）。
//!
//! ディスクのキャッシュの形（`cache_dir` は genzo-app が渡す専用のフォルダ）:
//! - `cache_dir/<提供元>/<z>/<x>/<y>.png`（本文）と同じ名前の `.json`（`TileMeta`: ETag・Last-Modified・
//!   期限・取得した日時）。`<提供元>` は URL のひな形の SHA-256 の先頭 16 桁（提供元を変えても混ざらない）。
//! - 書き込みは同じフォルダの一時ファイルに書いてから名前を変える（読み手が書きかけを読まない）。
//! - 容量の上限（既定 [`DEFAULT_CACHE_LIMIT_BYTES`]。仮置き）を超えたら、更新日時の古いタイルから消す。
//!   上限の確かめは書き込みの [`EVICTION_CHECK_EVERY`] 回ごと（最初の書き込みを含む）。
//! - [`TileService::clear_cache`] と [`TileService::cache_info`] は、`cache_dir` の中の提供元のフォルダ
//!   （16 桁の 16 進の名前）だけを扱う（ほかのファイル・フォルダには触らない）。
//!
//! ログには、タイルの番号・パスを info 以上で書かない（見ている場所がわかるため。決定事項ログ No.75 の方針）。

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::clock::{Clock, SystemClock};
use crate::freshness::{self, Policy, Previous};
use crate::{GeoError, HttpClient, HttpResponse, TileProvider};

/// タイルの本文の上限（2 MiB。仮置き。地図のタイルの画像 1 枚には十分な大きさとして置いた値）。
pub const MAX_TILE_BYTES: u64 = 2 * 1024 * 1024;

/// ディスクのキャッシュの容量の既定の上限（200 MiB。仮置き）。
pub const DEFAULT_CACHE_LIMIT_BYTES: u64 = 200 * 1024 * 1024;

/// 容量の上限を確かめる間隔（書き込みの回数。仮置き。毎回フォルダを走査しないため）。
pub const EVICTION_CHECK_EVERY: u64 = 128;

/// タイルの種類（PNG だけを扱う）。
pub const TILE_CONTENT_TYPE: &str = "image/png";

/// PNG の署名（ファイルの先頭 8 バイト）。
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// 地図のタイル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    /// 本文（PNG。デコードしていない）。
    pub bytes: Vec<u8>,
    /// 種類（常に [`TILE_CONTENT_TYPE`]）。
    pub content_type: &'static str,
    /// 期限の切れたキャッシュ（通信に失敗したので代わりに返した）。genzo-app は WebView に長く
    /// キャッシュさせない。
    pub stale: bool,
}

/// ディスクのキャッシュの大きさ。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CacheInfo {
    /// ファイルの数（タイルの本文と、付随の `.json` の両方を数える）。
    pub files: u64,
    /// 合計のバイト数。
    pub bytes: u64,
}

/// キャッシュに保存するタイルの付随の情報（`<y>.json`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TileMeta {
    /// サーバーの ETag（条件付きの要求の If-None-Match に使う）。
    pub(crate) etag: Option<String>,
    /// サーバーの Last-Modified（If-Modified-Since に使う）。
    pub(crate) last_modified: Option<String>,
    /// この日時までは問い合わせずに使う（UTC）。
    pub(crate) expires_at: DateTime<Utc>,
    /// 取得した（または 304 で確かめた）日時（UTC）。
    pub(crate) fetched_at: DateTime<Utc>,
    /// 期限の長さ（秒。304 の応答にキャッシュの指示がないときに使い直す）。
    pub(crate) lifetime_secs: i64,
    /// 期限が切れた後、通信に失敗したら使ってよいか。
    pub(crate) stale_ok: bool,
}

type TileKey = (u8, u32, u32);

/// 同じタイルの取得の待ち合わせ。
#[derive(Default)]
struct Flight {
    result: Mutex<Option<Result<Tile, GeoError>>>,
    done: Condvar,
    waiters: AtomicUsize,
}

/// 地図のタイルを取る（キャッシュつき）。複数のスレッドから同時に使える。
pub struct TileService {
    client: Arc<dyn HttpClient>,
    provider: TileProvider,
    cache: Option<DiskCache>,
    clock: Arc<dyn Clock>,
    in_flight: Mutex<HashMap<TileKey, Arc<Flight>>>,
}

impl std::fmt::Debug for TileService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileService")
            .field("provider", &self.provider)
            .field("cache_dir", &self.cache.as_ref().map(|c| &c.root))
            .finish_non_exhaustive()
    }
}

impl TileService {
    /// `cache_dir` が `None` ならディスクにキャッシュしない（genzo-app がキャッシュの場所を得られないとき）。
    pub fn new(
        client: Arc<dyn HttpClient>,
        provider: TileProvider,
        cache_dir: Option<PathBuf>,
    ) -> Self {
        let cache = cache_dir.map(|root| {
            let provider_dir = root.join(provider_dir_name(&provider));
            DiskCache {
                root,
                provider_dir,
                limit_bytes: DEFAULT_CACHE_LIMIT_BYTES,
                writes: AtomicU64::new(0),
                evicting: Mutex::new(()),
            }
        });
        Self {
            client,
            provider,
            cache,
            clock: Arc::new(SystemClock),
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// ディスクのキャッシュの容量の上限を変える（既定は [`DEFAULT_CACHE_LIMIT_BYTES`]）。
    pub fn with_cache_limit(mut self, bytes: u64) -> Self {
        if let Some(cache) = &mut self.cache {
            cache.limit_bytes = bytes;
        }
        self
    }

    /// 時計を差し替える（テスト用）。
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// 提供元。
    pub fn provider(&self) -> &TileProvider {
        &self.provider
    }

    /// ディスクのキャッシュのフォルダ（`new` に渡したもの）。
    pub fn cache_dir(&self) -> Option<&Path> {
        self.cache.as_ref().map(|c| c.root.as_path())
    }

    /// ズーム `z` のタイル `(x, y)` を返す（キャッシュか通信）。`z` が提供元の最大のズームを超えるか、
    /// `x`・`y` が `2^z` 以上なら [`GeoError::InvalidInput`]（通信しない）。
    pub fn tile(&self, z: u8, x: u32, y: u32) -> Result<Tile, GeoError> {
        check_range(z, x, y, self.provider.max_zoom())?;
        let key = (z, x, y);
        let (flight, leader) = {
            let mut map = lock(&self.in_flight);
            match map.get(&key) {
                Some(flight) => (flight.clone(), false),
                None => {
                    let flight = Arc::new(Flight::default());
                    map.insert(key, flight.clone());
                    (flight, true)
                }
            }
        };
        if !leader {
            flight.waiters.fetch_add(1, Ordering::SeqCst);
            let mut result = lock(&flight.result);
            while result.is_none() {
                result = flight
                    .done
                    .wait(result)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            return result.clone().expect("待ち合わせの結果");
        }
        let guard = FlightGuard {
            service: self,
            key,
            flight: &flight,
            finished: false,
        };
        let result = self.fetch(z, x, y);
        guard.finish(result.clone());
        result
    }

    /// テスト用: 同じタイルの取得を待っている呼び出しの数。
    #[cfg(test)]
    pub(crate) fn waiting_followers(&self) -> usize {
        lock(&self.in_flight)
            .values()
            .map(|f| f.waiters.load(Ordering::SeqCst))
            .sum()
    }

    /// ディスクのキャッシュの大きさ（キャッシュがなければ 0。読めないファイルは数えない）。
    pub fn cache_info(&self) -> CacheInfo {
        let Some(cache) = &self.cache else {
            return CacheInfo::default();
        };
        let mut info = CacheInfo::default();
        for file in cache.files() {
            info.files += 1;
            info.bytes += file.len;
        }
        info
    }

    /// ディスクのキャッシュを消す（`cache_dir` の中の提供元のフォルダだけ。`cache_dir` 自体と、ほかの
    /// ファイル・フォルダは消さない）。
    pub fn clear_cache(&self) -> Result<(), GeoError> {
        let Some(cache) = &self.cache else {
            return Ok(());
        };
        for dir in cache.provider_dirs() {
            match fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(GeoError::io(dir, e)),
            }
        }
        tracing::info!("地図のタイルのキャッシュを消しました");
        Ok(())
    }

    /// テスト用: 容量の上限を今すぐ確かめる。
    #[cfg(test)]
    pub(crate) fn enforce_cache_limit(&self) {
        if let Some(cache) = &self.cache {
            cache.enforce_limit();
        }
    }

    /// キャッシュを見て、要れば通信する（待ち合わせの先頭の呼び出しだけが通る）。
    fn fetch(&self, z: u8, x: u32, y: u32) -> Result<Tile, GeoError> {
        let now = self.clock.now_utc();
        let cached = self.cache.as_ref().and_then(|c| c.read(z, x, y));
        if let Some(entry) = &cached
            && let Some(meta) = &entry.meta
            && now < meta.expires_at
        {
            return Ok(tile(entry.bytes.clone(), false));
        }

        let mut headers = vec![("Accept", TILE_CONTENT_TYPE.to_string())];
        // 付随の情報がなければ ETag などがわからないので、条件なしで取り直す。
        let cached_meta = cached.as_ref().and_then(|c| c.meta.clone());
        if let Some(meta) = &cached_meta {
            if let Some(etag) = &meta.etag {
                headers.push(("If-None-Match", etag.clone()));
            }
            if let Some(last_modified) = &meta.last_modified {
                headers.push(("If-Modified-Since", last_modified.clone()));
            }
        }
        let url = self.provider.tile_url(z, x, y);
        let response = match self.client.get(&url, &headers, MAX_TILE_BYTES) {
            Ok(response) => response,
            Err(e @ GeoError::Network(_)) => return stale_or(cached, e),
            Err(e) => return Err(e),
        };
        let now = self.clock.now_utc();
        match response.status {
            304 => {
                let (Some(entry), Some(meta)) = (cached, cached_meta) else {
                    return Err(GeoError::BadResponse(
                        "条件付きでない要求に 304 が返りました".to_string(),
                    ));
                };
                let previous = Previous {
                    lifetime_secs: meta.lifetime_secs,
                    stale_ok: meta.stale_ok,
                };
                let policy = freshness::policy(&response, now, Some(previous));
                if let Some(cache) = &self.cache {
                    if policy.store {
                        let meta = TileMeta {
                            etag: response.header("etag").map(str::to_string).or(meta.etag),
                            last_modified: response
                                .header("last-modified")
                                .map(str::to_string)
                                .or(meta.last_modified),
                            ..meta_for(&policy, now, None, None)
                        };
                        cache.write_meta(z, x, y, &meta);
                    } else {
                        cache.remove(z, x, y);
                    }
                }
                Ok(tile(entry.bytes, false))
            }
            200 => {
                check_png(&response)?;
                let policy = freshness::policy(&response, now, None);
                if let Some(cache) = &self.cache {
                    if policy.store {
                        let meta = meta_for(
                            &policy,
                            now,
                            response.header("etag"),
                            response.header("last-modified"),
                        );
                        cache.write(z, x, y, &response.body, &meta);
                    } else {
                        cache.remove(z, x, y);
                    }
                }
                Ok(tile(response.body, false))
            }
            status if status >= 500 => stale_or(cached, GeoError::Status(status)),
            status if (200..300).contains(&status) => Err(GeoError::BadResponse(format!(
                "タイルの応答の状態が 200 ではありません（HTTP {status}）"
            ))),
            status => Err(GeoError::Status(status)),
        }
    }
}

/// 期限の切れたキャッシュがあり、古いものを使ってよければそれを、なければ `error` を返す。
fn stale_or(cached: Option<CachedTile>, error: GeoError) -> Result<Tile, GeoError> {
    match cached {
        Some(CachedTile {
            bytes,
            meta: Some(meta),
        }) if meta.stale_ok => {
            tracing::debug!("通信に失敗したので、期限の切れた地図のタイルを使います: {error}");
            Ok(tile(bytes, true))
        }
        _ => Err(error),
    }
}

fn tile(bytes: Vec<u8>, stale: bool) -> Tile {
    Tile {
        bytes,
        content_type: TILE_CONTENT_TYPE,
        stale,
    }
}

fn meta_for(
    policy: &Policy,
    now: DateTime<Utc>,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> TileMeta {
    TileMeta {
        etag: etag.map(str::to_string),
        last_modified: last_modified.map(str::to_string),
        expires_at: policy.expires_at(now),
        fetched_at: now,
        lifetime_secs: policy.lifetime_secs,
        stale_ok: policy.stale_ok,
    }
}

/// `Content-Type` が image/png（引数は無視。大文字と小文字は区別しない）で、本文が PNG の署名で始まるか。
fn check_png(response: &HttpResponse) -> Result<(), GeoError> {
    let content_type = response.header("content-type").map(|v| {
        v.split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    });
    if content_type.as_deref() != Some(TILE_CONTENT_TYPE) {
        return Err(GeoError::BadResponse(format!(
            "タイルの種類が {TILE_CONTENT_TYPE} ではありません（{}）",
            content_type.as_deref().unwrap_or("種類なし")
        )));
    }
    if !response.body.starts_with(PNG_SIGNATURE) {
        return Err(GeoError::BadResponse(
            "タイルの本文が PNG ではありません".to_string(),
        ));
    }
    Ok(())
}

/// タイルの番号の範囲（`z` は `max_zoom` まで、`x`・`y` は `2^z` 未満）。
fn check_range(z: u8, x: u32, y: u32, max_zoom: u8) -> Result<(), GeoError> {
    if z > max_zoom {
        return Err(GeoError::InvalidInput(format!(
            "地図のズームは {max_zoom} までです: {z}"
        )));
    }
    let n = 1u64.checked_shl(u32::from(z)).unwrap_or(u64::MAX);
    if u64::from(x) >= n || u64::from(y) >= n {
        return Err(GeoError::InvalidInput(format!(
            "ズーム {z} のタイルの番号は 0〜{} です",
            n - 1
        )));
    }
    Ok(())
}

/// 提供元のフォルダの名前（URL のひな形の SHA-256 の先頭 8 バイトの 16 進。16 桁）。
fn provider_dir_name(provider: &TileProvider) -> String {
    let digest = Sha256::digest(provider.url_template().as_bytes());
    hex::encode(&digest[..8])
}

/// 提供元のフォルダの名前の形か（16 桁の小文字の 16 進）。
fn is_provider_dir_name(name: &str) -> bool {
    name.len() == 16 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 待ち合わせの先頭の呼び出しが、結果を配って待ち合わせを片付ける（panic しても待ち手を起こす）。
struct FlightGuard<'a> {
    service: &'a TileService,
    key: TileKey,
    flight: &'a Flight,
    finished: bool,
}

impl FlightGuard<'_> {
    fn finish(mut self, result: Result<Tile, GeoError>) {
        self.complete(result);
    }

    fn complete(&mut self, result: Result<Tile, GeoError>) {
        self.finished = true;
        lock(&self.service.in_flight).remove(&self.key);
        *lock(&self.flight.result) = Some(result);
        self.flight.done.notify_all();
    }
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.complete(Err(GeoError::Network(
                "地図のタイルの取得が途中で止まりました".to_string(),
            )));
        }
    }
}

/// キャッシュから読んだタイル。
struct CachedTile {
    bytes: Vec<u8>,
    /// 付随の情報（読めなければ `None`。期限がわからないので使う前に取り直す）。
    meta: Option<TileMeta>,
}

/// キャッシュの中のファイル。
struct CacheFile {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
}

/// ディスクのキャッシュ。
struct DiskCache {
    /// genzo-app が渡した専用のフォルダ。
    root: PathBuf,
    /// 今の提供元のフォルダ（`root/<提供元>`）。
    provider_dir: PathBuf,
    limit_bytes: u64,
    writes: AtomicU64,
    /// 容量の上限の確かめを同時に 2 つ走らせない。
    evicting: Mutex<()>,
}

impl DiskCache {
    fn tile_dir(&self, z: u8, x: u32) -> PathBuf {
        self.provider_dir.join(z.to_string()).join(x.to_string())
    }

    fn read(&self, z: u8, x: u32, y: u32) -> Option<CachedTile> {
        let dir = self.tile_dir(z, x);
        let bytes = match fs::read(dir.join(format!("{y}.png"))) {
            Ok(bytes) => bytes,
            Err(e) => {
                if e.kind() != io::ErrorKind::NotFound {
                    tracing::debug!("地図のタイルのキャッシュを読めません: {e}");
                }
                return None;
            }
        };
        let meta = fs::read(dir.join(format!("{y}.json")))
            .ok()
            .and_then(|json| serde_json::from_slice::<TileMeta>(&json).ok());
        Some(CachedTile { bytes, meta })
    }

    /// 本文と付随の情報を書く（失敗してもタイルは返すので、ログに残すだけ）。
    fn write(&self, z: u8, x: u32, y: u32, bytes: &[u8], meta: &TileMeta) {
        let dir = self.tile_dir(z, x);
        let result = write_atomic(&dir, &format!("{y}.png"), bytes)
            .and_then(|()| write_atomic(&dir, &format!("{y}.json"), &meta_json(meta)));
        if let Err(e) = result {
            tracing::warn!("地図のタイルをキャッシュに書けませんでした: {e}");
            return;
        }
        if self
            .writes
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(EVICTION_CHECK_EVERY)
        {
            self.enforce_limit();
        }
    }

    fn write_meta(&self, z: u8, x: u32, y: u32, meta: &TileMeta) {
        if let Err(e) = write_atomic(&self.tile_dir(z, x), &format!("{y}.json"), &meta_json(meta)) {
            tracing::warn!("地図のタイルのキャッシュの期限を書けませんでした: {e}");
        }
    }

    fn remove(&self, z: u8, x: u32, y: u32) {
        let dir = self.tile_dir(z, x);
        for name in [format!("{y}.png"), format!("{y}.json")] {
            let _ = fs::remove_file(dir.join(name));
        }
    }

    /// `root` の中の提供元のフォルダ。
    fn provider_dirs(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| e.file_name().to_str().is_some_and(is_provider_dir_name))
            .map(|e| e.path())
            .collect()
    }

    /// 提供元のフォルダの中のすべてのファイル（読めないものは除く）。
    fn files(&self) -> Vec<CacheFile> {
        let mut files = Vec::new();
        let mut dirs = self.provider_dirs();
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    dirs.push(entry.path());
                } else if file_type.is_file()
                    && let Ok(meta) = entry.metadata()
                {
                    files.push(CacheFile {
                        path: entry.path(),
                        len: meta.len(),
                        modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    });
                }
            }
        }
        files
    }

    /// 合計が上限を超えていたら、更新日時の古いタイル（本文と `.json` の組）から消す。
    fn enforce_limit(&self) {
        let Ok(_running) = self.evicting.try_lock() else {
            return;
        };
        // 本文と `.json` を組にする（拡張子を除いたパスで。書きかけの一時ファイルはそれぞれ 1 つの組）。
        let mut groups: HashMap<PathBuf, (u64, SystemTime, Vec<PathBuf>)> = HashMap::new();
        let mut total = 0u64;
        for file in self.files() {
            total += file.len;
            let key = match file.path.extension().and_then(|e| e.to_str()) {
                Some("png" | "json") => file.path.with_extension(""),
                _ => file.path.clone(),
            };
            let group = groups
                .entry(key)
                .or_insert((0, SystemTime::UNIX_EPOCH, Vec::new()));
            group.0 += file.len;
            group.1 = group.1.max(file.modified);
            group.2.push(file.path);
        }
        if total <= self.limit_bytes {
            return;
        }
        let mut groups: Vec<_> = groups.into_values().collect();
        groups.sort_by_key(|(_, modified, _)| *modified);
        let mut removed = 0u64;
        for (len, _, paths) in groups {
            if total <= self.limit_bytes {
                break;
            }
            for path in paths {
                let _ = fs::remove_file(path);
            }
            total = total.saturating_sub(len);
            removed += 1;
        }
        tracing::debug!(
            "地図のタイルのキャッシュが上限を超えたので、古いもの {removed} 件を消しました"
        );
    }
}

fn meta_json(meta: &TileMeta) -> Vec<u8> {
    serde_json::to_vec(meta).expect("TileMeta は JSON にできる")
}

/// `dir/name` に書く（同じフォルダの一時ファイルに書いてから名前を変える）。
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new().prefix(".tmp-").tempfile_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.persist(dir.join(name)).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_numbers_are_checked() {
        assert!(check_range(0, 0, 0, 19).is_ok());
        assert!(check_range(19, (1 << 19) - 1, (1 << 19) - 1, 19).is_ok());
        assert!(check_range(22, (1 << 22) - 1, 0, 22).is_ok());
        for (z, x, y) in [
            (0, 1, 0),
            (0, 0, 1),
            (1, 2, 0),
            (20, 0, 0),
            (19, 1 << 19, 0),
        ] {
            assert!(
                matches!(check_range(z, x, y, 19), Err(GeoError::InvalidInput(_))),
                "{z}/{x}/{y}"
            );
        }
        // 提供元の最大のズームが大きくても、桁あふれしない。
        assert!(check_range(200, 0, 0, 255).is_ok());
    }

    #[test]
    fn provider_dirs_are_named_by_the_template() {
        let osm = provider_dir_name(&TileProvider::openstreetmap());
        assert!(is_provider_dir_name(&osm), "{osm}");
        let other = provider_dir_name(
            &TileProvider::new("https://tiles.example.org/{z}/{x}/{y}.png", 19, "x").unwrap(),
        );
        assert_ne!(osm, other);
        // 同じひな形なら同じ名前（起動し直しても同じフォルダを使う）。
        assert_eq!(osm, provider_dir_name(&TileProvider::openstreetmap()));
        for name in [
            "",
            "0123456789abcde",
            "0123456789ABCDEF",
            "0123456789abcdefg",
            "map-tiles",
        ] {
            assert!(!is_provider_dir_name(name), "{name}");
        }
    }

    #[test]
    fn content_types_are_checked_without_decoding() {
        let png = [PNG_SIGNATURE.as_slice(), b"rest"].concat();
        let resp = |ct: Option<&str>, body: &[u8]| HttpResponse {
            status: 200,
            headers: ct
                .map(|ct| vec![("content-type".to_string(), ct.to_string())])
                .unwrap_or_default(),
            body: body.to_vec(),
        };
        assert!(check_png(&resp(Some("image/png"), &png)).is_ok());
        assert!(check_png(&resp(Some("Image/PNG; charset=binary"), &png)).is_ok());
        for (ct, body) in [
            (Some("text/html"), png.as_slice()),
            (Some("image/jpeg"), png.as_slice()),
            (None, png.as_slice()),
            (Some("image/png"), b"<html>".as_slice()),
            (Some("image/png"), b"".as_slice()),
        ] {
            assert!(
                matches!(check_png(&resp(ct, body)), Err(GeoError::BadResponse(_))),
                "{ct:?}"
            );
        }
    }
}
