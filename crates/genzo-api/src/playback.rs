//! 動画の再生（VID-04。04 の 4.2 節、決定事項ログ No.117〜119）。
//!
//! - **再生は WebView に任せる**: アプリ（genzo-app）の URI スキーム `genzo://` が、カタログに登録された動画の
//!   元のファイルを、Range の要求に応えて配信し（[`Core::video_stream`]）、画面の `video` 要素で再生する
//!   （デコードは WebView、つまり OS の動画の機能。Windows は WebView2（Chromium）と Media Foundation、macOS は
//!   WKWebView と AVFoundation）。任意のパスは配信しない（variant の ID だけを受け取り、種類が動画で、拡張子が
//!   取り込む対象の動画のものだけ）。
//! - **再生できない動画**（H.265 を OS が扱えない Windows・10bit・MPEG-4 Part 2 など。画面が `canPlayType` と
//!   再生の失敗で判定する）は、利用者の ffmpeg（決定事項ログ No.62・67。同梱しない）があれば、**再生用のファイル**
//!   （H.264 ＋ AAC の MP4。長辺 1280。genzo-media の `video_proxy`）を作って再生する（[`Core::create_video_proxy`]。
//!   [`JobKind::VideoProxy`] のジョブ）。ffmpeg がない・H.264 のエンコーダーがなければ、作れない理由を返す
//!   （[`VideoPlayback::proxy_unavailable`]）。
//! - **再生用のファイルの作成**は、長くかかる（数十秒〜数分）ので、専用の 1 本のスレッド（`genzo-video`）で、
//!   1 度に 1 本だけ行う（ほかの動画の作成を頼まれたら、今のものは取り消す。最後の要求だけ）。ffmpeg は
//!   ワーカーを通さず、本体の子プロセスとして動かす（ワーカーの 1 つを数分ふさがないため。デコードは ffmpeg の
//!   プロセスの中で行われる。決定事項ログ No.117）。取り消し・カタログを閉じると ffmpeg を止める。
//! - **キャッシュ**: データのフォルダの `video-proxies/` に `<ファイルの ID>-<キー>.mp4` で置く。キーは
//!   キャッシュの世代・ファイル・リビジョン・形式の版から決める（[`proxy_key`]。元のファイルの内容が変わると
//!   使われなくなる）。一時ファイル（`.<キー>.<プロセス>.<連番>.tmp`）に書いて、ffprobe で確かめ、`fsync` して
//!   から名前を変えて公開する（4.1 節の公開の順序と同じ。途中で終わっても壊れたファイルを使わない）。
//!   **上限**（設定の [`CoreSettings::video_proxy_cache_bytes`]。既定 [`DEFAULT_VIDEO_PROXY_CAPACITY_BYTES`]）を
//!   超えたら、最後に使った日時（ファイルの更新日時。再生の情報を読んだときに今の時刻にする）の古いものから
//!   消す。キャッシュの回収（[`Core::collect_cache_garbage`]）で、今のキーでないもの（カタログにない・内容が
//!   変わったファイルのもの）と、古い一時ファイルも消す。元の動画は読むだけで、変えない（DATA-01）。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use genzo_jobs::CancellationToken;
use genzo_media::{
    FfmpegTools, MediaError, PROXY_FORMAT_VERSION, PROXY_LONG_EDGE, ProxyEncoder,
    default_proxy_threads, pick_proxy_encoder, verify_proxy,
};
use genzo_model::{AssetKind, FileId, VariantId};
use genzo_worker::Lane;

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::jobs::{JobQueue, spawn_job_on};
use crate::previews::{VariantFile, refresh_variant_file, variant_file};
use crate::types::{
    CoreSettings, JobKind, JobResult, VideoPlayback, VideoProxyCacheStatus, VideoProxyInfo,
    VideoProxyReport, VideoStream,
};

/// 再生用のファイルのキャッシュの上限の既定値（バイト。**仮置き**: 5GB（10 進）。長辺 1280 の H.264
/// （映像 約 4 Mbps ＋ 音声 128 kbps。1 分で約 31 MB）で、合わせて約 2 時間 40 分ぶん。容量の予算（SCL-08）に
/// 足す）。
pub const DEFAULT_VIDEO_PROXY_CAPACITY_BYTES: u64 = 5_000_000_000;

/// 再生用のファイルのキャッシュの上限として受け付ける最小値（**仮置き**: 500MB（10 進）。4K の 10 分の動画の
/// 再生用のファイル（約 310 MB の見込み）が入る大きさ）。
pub const MIN_VIDEO_PROXY_CACHE_BYTES: u64 = 500_000_000;

/// 再生用のファイルのフォルダの名前（データのフォルダの中）。
pub const VIDEO_PROXY_DIR: &str = "video-proxies";

/// 一時ファイルの名前の連番（同じプロセスの中での衝突を防ぐ）。
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 再生用のファイルのキャッシュキー（16 進数 64 文字）。キャッシュの世代・ファイル・リビジョン・形式の版と
/// 長辺の上限から決まる（エンコーダーには依存しない。どのエンコーダーで作ったものも同じように再生できるため）。
pub(crate) fn proxy_key(generation: &str, file_id: FileId, revision: u32) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"genzo.video.proxy.v1\0");
    h.update(generation.as_bytes());
    h.update(b"\0");
    h.update(&file_id.get().to_le_bytes());
    h.update(&revision.to_le_bytes());
    h.update(&PROXY_FORMAT_VERSION.to_le_bytes());
    h.update(&PROXY_LONG_EDGE.to_le_bytes());
    h.finalize().to_hex().to_string()
}

/// 動画の拡張子の MIME の型（`mp4` は `video/mp4`、`mov` は `video/quicktime`）。動画でなければ `None`。
pub fn video_content_type(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        _ => None,
    }
}

/// キャッシュの中の再生用のファイル。
#[derive(Debug, Clone)]
pub(crate) struct ProxyFile {
    pub path: PathBuf,
    pub file_id: FileId,
    pub key: String,
    pub size: u64,
    pub modified: SystemTime,
}

/// 消した結果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProxyRemoval {
    /// 消した数。
    pub removed: u64,
    /// 空けた大きさ（バイト）。
    pub freed_bytes: u64,
    /// 消せなかった数（Windows で再生中のものなど。次の回収でもう一度試す）。
    pub failed: u64,
}

/// 再生用のファイルのキャッシュ（フォルダの中のファイルだけで管理する。索引の DB は持たない。数は多くても
/// 数百のため、走査で足りる）。
#[derive(Debug)]
pub(crate) struct ProxyCache {
    dir: PathBuf,
    capacity: u64,
}

impl ProxyCache {
    /// フォルダを開く（なければ作る）。
    pub(crate) fn open(dir: PathBuf, capacity: u64) -> std::io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self { dir, capacity })
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn capacity(&self) -> u64 {
        self.capacity
    }

    pub(crate) fn set_capacity(&mut self, capacity: u64) {
        self.capacity = capacity;
    }

    /// 公開する名前（`<ファイルの ID>-<キー>.mp4`）。
    fn file_name(file_id: FileId, key: &str) -> String {
        format!("{}-{key}.mp4", file_id.get())
    }

    /// 名前からファイルの ID とキーを読む（形が違えば `None`）。
    fn parse_name(name: &str) -> Option<(FileId, String)> {
        let stem = name.strip_suffix(".mp4")?;
        let (id, key) = stem.split_once('-')?;
        let id: i64 = id.parse().ok().filter(|&n| n > 0)?;
        let key_ok = key.len() == 64 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        key_ok.then(|| (FileId::new(id), key.to_owned()))
    }

    pub(crate) fn path_for(&self, file_id: FileId, key: &str) -> PathBuf {
        self.dir.join(Self::file_name(file_id, key))
    }

    /// 今のキーの再生用のファイル（あれば、そのパスと大きさ）。
    pub(crate) fn get(&self, file_id: FileId, key: &str) -> Option<(PathBuf, u64)> {
        let path = self.path_for(file_id, key);
        match fs::metadata(&path) {
            Ok(m) if m.is_file() => Some((path, m.len())),
            _ => None,
        }
    }

    /// 使った記録（ファイルの更新日時を今にする。失敗しても使える）。
    pub(crate) fn touch(path: &Path) {
        if let Ok(f) = fs::OpenOptions::new().write(true).open(path) {
            let _ = f.set_modified(SystemTime::now());
        }
    }

    /// 書き込みに使う一時ファイルの名前（同じフォルダの中。公開の名前の変更を同じボリュームの中で行うため）。
    pub(crate) fn temp_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!(
            ".{key}.{}.{}.tmp",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// フォルダを走査する。生成の途中で終わった古い一時ファイル（最後の更新から
    /// [`genzo_catalog::PREVIEW_TEMP_FILE_MIN_AGE`] 以上たったもの）は消し、その数も返す。
    pub(crate) fn scan(&self) -> std::io::Result<(Vec<ProxyFile>, u64)> {
        let mut files = Vec::new();
        let mut temp_removed = 0;
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((files, 0)),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if name.starts_with('.') && name.ends_with(".tmp") {
                let stale = meta
                    .modified()
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .is_some_and(|age| age >= genzo_catalog::PREVIEW_TEMP_FILE_MIN_AGE);
                if stale && fs::remove_file(&path).is_ok() {
                    temp_removed += 1;
                }
                continue;
            }
            let Some((file_id, key)) = Self::parse_name(&name) else {
                continue;
            };
            files.push(ProxyFile {
                path,
                file_id,
                key,
                size: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
        Ok((files, temp_removed))
    }

    /// 合計が `capacity` 以下になるまで、最後に使った日時の古いものから消す（`protect` は消さない）。
    pub(crate) fn evict_to(
        &self,
        capacity: u64,
        protect: Option<&Path>,
    ) -> std::io::Result<ProxyRemoval> {
        let (mut files, _) = self.scan()?;
        let mut total: u64 = files.iter().map(|f| f.size).sum();
        files.sort_by(|a, b| a.modified.cmp(&b.modified).then(a.path.cmp(&b.path)));
        let mut r = ProxyRemoval::default();
        for f in files {
            if total <= capacity {
                break;
            }
            if protect == Some(f.path.as_path()) {
                continue;
            }
            match fs::remove_file(&f.path) {
                Ok(()) => {
                    total = total.saturating_sub(f.size);
                    r.removed += 1;
                    r.freed_bytes += f.size;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    total = total.saturating_sub(f.size);
                }
                Err(_) => r.failed += 1,
            }
        }
        Ok(r)
    }

    /// `remove` が真を返すものを消す。
    pub(crate) fn remove_where(
        &self,
        remove: impl Fn(&ProxyFile) -> bool,
    ) -> std::io::Result<ProxyRemoval> {
        let (files, _) = self.scan()?;
        let mut r = ProxyRemoval::default();
        for f in files.iter().filter(|f| remove(f)) {
            match fs::remove_file(&f.path) {
                Ok(()) => {
                    r.removed += 1;
                    r.freed_bytes += f.size;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => r.failed += 1,
            }
        }
        Ok(r)
    }

    /// 状況（数・合計・上限）。
    pub(crate) fn status(&self, removed: u64) -> std::io::Result<VideoProxyCacheStatus> {
        let (files, _) = self.scan()?;
        Ok(VideoProxyCacheStatus {
            dir: self.dir.clone(),
            count: files.len() as u64,
            total_bytes: files.iter().map(|f| f.size).sum(),
            capacity_bytes: self.capacity,
            removed,
        })
    }
}

/// 作っている途中の再生用のファイル。
#[derive(Debug, Clone)]
pub(crate) struct RunningProxy {
    pub file_id: FileId,
    pub key: String,
    pub job_id: u64,
}

/// 再生用のファイルの状態（[`Inner`] の `proxies`。ロックは短く持ち、カタログのロックの中では取らない）。
#[derive(Debug, Default)]
pub(crate) struct ProxyState {
    /// キャッシュ（フォルダを作れなければ `None` で、理由は `open_error`）。
    pub cache: Option<ProxyCache>,
    pub open_error: Option<String>,
    /// 作っている途中のもの（1 度に 1 本だけ）。
    pub running: Option<RunningProxy>,
}

impl ProxyState {
    /// データのフォルダの `video-proxies` を開く（作れなければ、理由を覚えて再生用のファイルを使わない）。
    pub(crate) fn open(data_dir: &Path, settings: &CoreSettings) -> Self {
        let dir = data_dir.join(VIDEO_PROXY_DIR);
        match ProxyCache::open(dir.clone(), settings.video_proxy_cache_bytes) {
            Ok(cache) => {
                // 前回の生成の途中で終わった古い一時ファイルを消し、上限を超えていれば減らす。
                if let Err(e) = cache.evict_to(cache.capacity(), None) {
                    tracing::warn!(error = %e, dir = %dir.display(), "再生用のファイルのキャッシュを整理できない");
                }
                Self {
                    cache: Some(cache),
                    ..Self::default()
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, dir = %dir.display(), "再生用のファイルのフォルダを作れない");
                Self {
                    open_error: Some(format!(
                        "再生用のファイルのフォルダ {} を作れません: {e}",
                        dir.display()
                    )),
                    ..Self::default()
                }
            }
        }
    }

    /// 開けなかった理由のエラー。
    fn missing(&self) -> ApiError {
        ApiError::Internal(
            self.open_error
                .clone()
                .unwrap_or_else(|| "再生用のファイルのキャッシュがありません".to_owned()),
        )
    }
}

/// 再生用のファイルを作れるか（ffmpeg とエンコーダーを 1 度だけ確かめる。[`Inner`] の `proxy_support`）。
#[derive(Debug, Clone)]
pub(crate) enum ProxySupport {
    /// 作れる。
    Ready {
        tools: FfmpegTools,
        encoder: ProxyEncoder,
    },
    /// 作れない（理由）。
    Unavailable(String),
}

/// コアが使う ffmpeg・ffprobe（ワーカーに渡す環境変数 [`crate::CoreConfig::worker_env`] の `GENZO_FFMPEG`・
/// `GENZO_FFPROBE` を優先し、なければこのプロセスの環境変数、それもなければ PATH。ワーカーと同じものを使う）。
/// [`crate::CoreConfig::ffmpeg_path_fallback`] が `false` で、`worker_env` に両方がなければ `None`（アプリが
/// 見つけられなかった）。
pub(crate) fn ffmpeg_tools(config: &crate::CoreConfig) -> Option<FfmpegTools> {
    let from_config = |key: &str| {
        config
            .worker_env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    if !config.ffmpeg_path_fallback
        && (from_config(genzo_media::video::ENV_FFMPEG).is_none()
            || from_config(genzo_media::video::ENV_FFPROBE).is_none())
    {
        return None;
    }
    Some(FfmpegTools::from_lookup(|key| {
        from_config(key).or_else(|| std::env::var_os(key))
    }))
}

fn detect_support(config: &crate::CoreConfig) -> ProxySupport {
    let missing = || {
        ProxySupport::Unavailable(
            "ffmpeg・ffprobe が見つからないため、再生用のファイルを作れません（FFmpeg を入れて、見つからなければ設定で場所を指定してください）"
                .to_owned(),
        )
    };
    let Some(tools) = ffmpeg_tools(config) else {
        return missing();
    };
    if !tools.is_available() {
        return missing();
    }
    match tools.encoders() {
        Ok(names) => match pick_proxy_encoder(&names) {
            Some(encoder) => ProxySupport::Ready { tools, encoder },
            None => ProxySupport::Unavailable(
                "ffmpeg に H.264 のエンコーダー（libx264・VideoToolbox・Media Foundation・OpenH264 のどれか）がないため、再生用のファイルを作れません"
                    .to_owned(),
            ),
        },
        Err(e) => {
            ProxySupport::Unavailable(format!("ffmpeg のエンコーダーを確かめられません: {e}"))
        }
    }
}

impl Inner {
    /// 再生用のファイルを作れるか（初めて呼んだときに ffmpeg を確かめる）。
    pub(crate) fn proxy_support(&self) -> &ProxySupport {
        self.proxy_support
            .get_or_init(|| detect_support(&self.config))
    }

    /// 再生用のファイルのキャッシュを使う（入出力のエラーはフォルダの [`ApiError::Io`]。フォルダを作れなかった
    /// ときは [`ApiError::Internal`]）。
    pub(crate) fn with_proxies<T>(
        &self,
        f: impl FnOnce(&mut ProxyCache) -> std::io::Result<T>,
    ) -> Result<T, ApiError> {
        let mut state = self.proxies.lock();
        let missing = state.missing();
        let cache = state.cache.as_mut().ok_or(missing)?;
        let dir = cache.dir().to_path_buf();
        f(cache).map_err(|e| ApiError::io(dir, e))
    }
}

/// 動画の variant の主となるファイル（動画でなければ [`ApiError::InvalidArgument`]）。
fn video_file(inner: &Inner, variant_id: VariantId) -> Result<VariantFile, ApiError> {
    let f = inner.with_catalog_api(|c| variant_file(c, variant_id))?;
    if f.kind != AssetKind::Video || video_content_type(&f.name).is_none() {
        return Err(ApiError::InvalidArgument(format!(
            "variant {variant_id} は動画ではありません"
        )));
    }
    Ok(f)
}

/// ffmpeg が書いた一時ファイルの内容をストレージへ書き出し（`fsync`）、大きさを返す。
///
/// **書き込みのハンドルで開く**（検証 R1）: Windows の `File::sync_all`（`FlushFileBuffers`）は書き込みの権限
/// （`GENERIC_WRITE`）のないハンドルではアクセス拒否（os error 5）になり、読み取りだけで開くと、Windows では
/// 再生用のファイルの作成が毎回失敗していた（カタログのバックアップの `sync_file` と同じ開き方にする）。
fn sync_written_file(path: &Path) -> std::io::Result<u64> {
    let f = fs::OpenOptions::new().read(true).write(true).open(path)?;
    genzo_model::fs_sync::sync_file_fast(&f)?;
    Ok(f.metadata()?.len())
}

/// 名前の変更の後、フォルダの変更を永続化する（Unix。失敗しても公開は済んでいる）。
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = fs::File::open(dir) {
        let _ = genzo_model::fs_sync::sync_file_fast(&d);
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// 作っている途中の印を、ジョブが終わったら（成功・失敗・取り消し・パニックのどれでも）外す。
struct ClearRunning<'a> {
    inner: &'a Inner,
    job_id: u64,
}

impl Drop for ClearRunning<'_> {
    fn drop(&mut self) {
        let mut state = self.inner.proxies.lock();
        if state
            .running
            .as_ref()
            .is_some_and(|r| r.job_id == self.job_id)
        {
            state.running = None;
        }
    }
}

fn media_err(e: MediaError) -> ApiError {
    match e {
        MediaError::Cancelled => ApiError::Cancelled,
        e => ApiError::Media(e),
    }
}

/// 再生用のファイルを作るジョブの中身。
fn run_proxy_job(
    ctx: &crate::jobs::JobCtx<'_>,
    variant_id: VariantId,
) -> Result<JobResult, ApiError> {
    let inner = ctx.inner;
    let _clear = ClearRunning {
        inner,
        job_id: ctx.id,
    };
    let file = video_file(inner, variant_id)?;
    // 元のファイルをディスクと照合する（内容が変わっていればリビジョンが上がり、キーが変わる）。
    let file = refresh_variant_file(inner, &file, Lane::Batch, ctx.token())?.ok_or_else(|| {
        ApiError::FileAccess {
            path: file
                .path
                .clone()
                .unwrap_or_else(|| PathBuf::from(&file.name)),
            message: "元の動画が見つかりません".to_owned(),
        }
    })?;
    let path = file.path.clone().ok_or_else(|| ApiError::FileAccess {
        path: PathBuf::from(&file.name),
        message: "ボリュームのマウント先が分かりません".to_owned(),
    })?;
    let key = proxy_key(&inner.cache_generation, file.file_id, file.revision);
    if let Some((p, size)) = inner.with_proxies(|c| Ok(c.get(file.file_id, &key)))? {
        ProxyCache::touch(&p);
        return Ok(JobResult::VideoProxy(VideoProxyReport {
            variant_id,
            size,
            reused: true,
            encoder: None,
        }));
    }
    let (tools, encoder) = match inner.proxy_support() {
        ProxySupport::Ready { tools, encoder } => (tools.clone(), *encoder),
        ProxySupport::Unavailable(reason) => return Err(ApiError::InvalidArgument(reason.clone())),
    };
    if ctx.is_cancelled() {
        return Err(ApiError::Cancelled);
    }
    let probe = tools.probe(&path).map_err(media_err)?;
    let duration = probe
        .metadata
        .duration_s
        .filter(|d| d.is_finite() && *d > 0.0);
    let total_ms = duration.map_or(0, |d| (d * 1000.0).round() as u64);
    let tmp = inner.with_proxies(|c| Ok(c.temp_path(&key)))?;
    ctx.progress(0, total_ms);
    let cancelled = || ctx.is_cancelled();
    tools
        .transcode_proxy(
            &path,
            &probe,
            &tmp,
            encoder,
            default_proxy_threads(),
            &cancelled,
            &mut |t| {
                if total_ms > 0 {
                    ctx.progress(((t * 1000.0) as u64).min(total_ms), total_ms);
                }
            },
        )
        .map_err(media_err)?;
    // 書いたものを確かめてから公開する（確かめられなければ消す）。
    let publish = || -> Result<(PathBuf, u64), ApiError> {
        let out = tools.probe(&tmp).map_err(media_err)?;
        verify_proxy(&out, duration).map_err(media_err)?;
        let size = sync_written_file(&tmp).map_err(|e| ApiError::io(&tmp, e))?;
        if ctx.is_cancelled() {
            return Err(ApiError::Cancelled);
        }
        inner.with_proxies(|cache| {
            let dest = cache.path_for(file.file_id, &key);
            fs::rename(&tmp, &dest)?;
            sync_dir(cache.dir());
            let evicted = cache.evict_to(cache.capacity(), Some(&dest))?;
            if evicted.removed > 0 {
                tracing::info!(
                    removed = evicted.removed,
                    freed = evicted.freed_bytes,
                    "再生用のファイルを上限まで減らした"
                );
            }
            Ok((dest, size))
        })
    };
    let (dest, size) = match publish() {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    };
    tracing::info!(variant = %variant_id, size, encoder = encoder.ffmpeg_name(), path = %dest.display(), "再生用のファイルを作った");
    ctx.progress(total_ms, total_ms);
    Ok(JobResult::VideoProxy(VideoProxyReport {
        variant_id,
        size,
        reused: false,
        encoder: Some(encoder.ffmpeg_name().to_owned()),
    }))
}

/// キャッシュの回収（[`Core::collect_cache_garbage`]）の再生用のファイルの分: 今のキーでないもの（カタログに
/// ないファイル・内容が変わったファイル・世代の違うもの）を消し、上限を超えた分を古いものから消す。
/// 作っている途中のものの一時ファイルは消さない（古い一時ファイルだけを消す）。消した数と、消した一時ファイルの
/// 数を返す。
pub(crate) fn collect_garbage(inner: &Inner) -> Result<(u64, u64), ApiError> {
    if inner.proxies.lock().cache.is_none() {
        return Ok((0, 0));
    }
    let (files, temp_removed) = inner.with_proxies(|c| c.scan())?;
    // ファイルごとの今のキー（カタログのロックの中で、短く）。
    let current: HashSet<(FileId, String)> = inner.with_catalog_api(|c| {
        let ids: HashSet<FileId> = files.iter().map(|f| f.file_id).collect();
        let mut keys = HashSet::new();
        for id in ids {
            match c.file(id) {
                Ok(rec) => {
                    keys.insert((id, proxy_key(&inner.cache_generation, id, rec.revision)));
                }
                Err(genzo_catalog::CatalogError::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(keys)
    })?;
    let removed = inner.with_proxies(|cache| {
        let stale = cache.remove_where(|f| !current.contains(&(f.file_id, f.key.clone())))?;
        let evicted = cache.evict_to(cache.capacity(), None)?;
        Ok(stale.removed + evicted.removed)
    })?;
    Ok((removed, temp_removed))
}

impl Core {
    /// 動画の再生の情報（VID-04）: 元のファイルを読めるか（ディスクと照合する。内容が変わっていれば読み直す）・
    /// MIME の型・動画の情報・今の内容の再生用のファイル（あれば、最後に使った日時を今にする）・作っている途中の
    /// ジョブ・再生用のファイルを作れない理由。
    ///
    /// 動画でない variant は [`ApiError::InvalidArgument`]。
    pub fn video_playback(&self, variant_id: VariantId) -> Result<VideoPlayback, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let file = video_file(inner, variant_id)?;
        let content_type = video_content_type(&file.name)
            .unwrap_or("application/octet-stream")
            .to_owned();
        // 元のファイルを照合する（見つからなければ見つからない印を付ける。写真を開くときと同じ）。
        let token = CancellationToken::new();
        let (file, unavailable_reason) =
            match refresh_variant_file(inner, &file, Lane::Interactive, &token) {
                Ok(Some(f)) => (f, None),
                Ok(None) => (file, Some("元の動画が見つかりません".to_owned())),
                Err(ApiError::Closed) => return Err(ApiError::Closed),
                Err(e) => {
                    // 内容が変わった動画の読み直しの失敗など。ファイルがあれば再生は試せる。
                    let exists = file.path.as_deref().is_some_and(Path::is_file);
                    (file, (!exists).then(|| e.to_string()))
                }
            };
        let size = file
            .path
            .as_deref()
            .and_then(|p| fs::metadata(p).ok())
            .filter(|m| m.is_file())
            .map(|m| m.len());
        let available = unavailable_reason.is_none() && size.is_some();
        let unavailable_reason = if available {
            None
        } else {
            unavailable_reason.or_else(|| Some("元の動画を読めません".to_owned()))
        };
        let video = inner.with_catalog(|c| Ok(c.asset(file.asset_id)?.video))?;
        let key = proxy_key(&inner.cache_generation, file.file_id, file.revision);
        let (proxy, running) = {
            let state = inner.proxies.lock();
            let proxy = state
                .cache
                .as_ref()
                .and_then(|c| c.get(file.file_id, &key))
                .map(|(p, size)| {
                    ProxyCache::touch(&p);
                    VideoProxyInfo {
                        size,
                        long_edge: PROXY_LONG_EDGE,
                    }
                });
            let running = state
                .running
                .as_ref()
                .filter(|r| r.file_id == file.file_id && r.key == key)
                .map(|r| r.job_id);
            (proxy, running)
        };
        let proxy_job = running.filter(|&id| {
            inner
                .jobs
                .info(id)
                .is_some_and(|info| !info.state.is_finished())
        });
        // ffmpeg の確認（初めてなら数百ミリ秒）は、キャッシュのロックの外で行う。
        let open_error = inner.proxies.lock().open_error.clone();
        let (proxy_unavailable, proxy_encoder) = match (open_error, inner.proxy_support()) {
            (Some(e), _) => (Some(e), None),
            (None, ProxySupport::Ready { encoder, .. }) => {
                (None, Some(encoder.ffmpeg_name().to_owned()))
            }
            (None, ProxySupport::Unavailable(reason)) => (Some(reason.clone()), None),
        };
        Ok(VideoPlayback {
            variant_id,
            available,
            unavailable_reason,
            content_type,
            size,
            video,
            proxy,
            proxy_job,
            proxy_unavailable,
            proxy_encoder,
        })
    }

    /// 配信する動画のファイル（アプリの `genzo://` の URI スキーム用。ディスクと照合しないので軽い）。
    ///
    /// `proxy` なら今の内容の再生用のファイル（なければ [`ApiError::NotFound`]）、そうでなければカタログに
    /// 登録された元のファイル（種類が動画で、拡張子が `mp4`・`mov` で、通常のファイルのものだけ。任意のパスは
    /// 返さない）。
    pub fn video_stream(
        &self,
        variant_id: VariantId,
        proxy: bool,
    ) -> Result<VideoStream, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let file = video_file(inner, variant_id).map_err(|e| match e {
            ApiError::InvalidArgument(m) => ApiError::NotFound(m),
            e => e,
        })?;
        if proxy {
            let key = proxy_key(&inner.cache_generation, file.file_id, file.revision);
            let found = inner
                .proxies
                .lock()
                .cache
                .as_ref()
                .and_then(|c| c.get(file.file_id, &key));
            let (path, size) = found.ok_or_else(|| {
                ApiError::NotFound(format!("動画 {variant_id} の再生用のファイル"))
            })?;
            return Ok(VideoStream {
                variant_id,
                path,
                size,
                content_type: "video/mp4".to_owned(),
                proxy: true,
            });
        }
        let path = file
            .path
            .clone()
            .ok_or_else(|| ApiError::NotFound(format!("動画 {variant_id} の元のファイルの場所")))?;
        let meta = match fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ApiError::NotFound(path.display().to_string()));
            }
            Err(e) => {
                return Err(ApiError::FileAccess {
                    path,
                    message: e.to_string(),
                });
            }
        };
        if !meta.is_file() {
            return Err(ApiError::NotFound(path.display().to_string()));
        }
        let content_type = video_content_type(&file.name)
            .expect("video_file で確かめた")
            .to_owned();
        Ok(VideoStream {
            variant_id,
            path,
            size: meta.len(),
            content_type,
            proxy: false,
        })
    }

    /// 動画の再生用のファイルを作るジョブを投入する（[`JobKind::VideoProxy`]。ジョブの ID を返す）。
    ///
    /// 同じ動画（同じ内容）を作っている途中なら、そのジョブの ID を返す。別の動画を作っている途中なら、それを
    /// 取り消す（最後の要求だけ）。今の内容のものがあれば、ジョブはすぐに終わる（`reused`）。ffmpeg がない・
    /// H.264 のエンコーダーがなければ [`ApiError::InvalidArgument`]（理由は [`VideoPlayback::proxy_unavailable`]
    /// と同じ）。
    pub fn create_video_proxy(&self, variant_id: VariantId) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let file = video_file(inner, variant_id)?;
        let open_error = inner.proxies.lock().open_error.clone();
        if let Some(e) = open_error {
            return Err(ApiError::Internal(e));
        }
        if let ProxySupport::Unavailable(reason) = inner.proxy_support() {
            return Err(ApiError::InvalidArgument(reason.clone()));
        }
        let key = proxy_key(&inner.cache_generation, file.file_id, file.revision);
        let mut state = inner.proxies.lock();
        if let Some(r) = &state.running {
            let alive = inner
                .jobs
                .info(r.job_id)
                .is_some_and(|i| !i.state.is_finished());
            if alive && r.file_id == file.file_id && r.key == key {
                return Ok(r.job_id);
            }
            if alive {
                inner.jobs.cancel(r.job_id);
            }
        }
        let id = spawn_job_on(
            inner,
            JobQueue::Video,
            JobKind::VideoProxy,
            "再生用のファイルの作成",
            move |ctx| run_proxy_job(ctx, variant_id),
        );
        state.running = Some(RunningProxy {
            file_id: file.file_id,
            key,
            job_id: id,
        });
        drop(state);
        inner.jobs.reap_unrun(inner);
        Ok(id)
    }

    /// 再生用のファイルのキャッシュの状況（フォルダ・数・合計・上限）。
    pub fn video_proxy_cache(&self) -> Result<VideoProxyCacheStatus, ApiError> {
        self.inner.check_open()?;
        self.inner.with_proxies(|c| c.status(0))
    }

    /// 再生用のファイルをすべて消す（作り直せる。作っている途中のものは取り消す）。消した後の状況を返す。
    pub fn clear_video_proxies(&self) -> Result<VideoProxyCacheStatus, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let running = inner.proxies.lock().running.as_ref().map(|r| r.job_id);
        if let Some(id) = running {
            inner.jobs.cancel(id);
            inner.jobs.reap_unrun(inner);
        }
        inner.with_proxies(|cache| {
            let r = cache.remove_where(|_| true)?;
            cache.status(r.removed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn key(n: u8) -> String {
        format!("{:02x}", n).repeat(32)
    }

    fn write(cache: &ProxyCache, id: i64, k: &str, len: usize, age_s: u64) -> PathBuf {
        let p = cache.path_for(FileId::new(id), k);
        fs::write(&p, vec![0u8; len]).unwrap();
        let f = fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_s))
            .unwrap();
        p
    }

    #[test]
    fn keys_follow_generation_file_and_revision() {
        let a = proxy_key("g1", FileId::new(1), 1);
        assert_eq!(a.len(), 64);
        assert_eq!(a, proxy_key("g1", FileId::new(1), 1));
        assert_ne!(a, proxy_key("g2", FileId::new(1), 1));
        assert_ne!(a, proxy_key("g1", FileId::new(2), 1));
        assert_ne!(a, proxy_key("g1", FileId::new(1), 2));
    }

    #[test]
    fn content_types_by_extension() {
        assert_eq!(video_content_type("A.MP4"), Some("video/mp4"));
        assert_eq!(video_content_type("dji_0001.MOV"), Some("video/quicktime"));
        assert_eq!(video_content_type("a.jpg"), None);
        assert_eq!(video_content_type("mp4"), None);
    }

    #[test]
    fn names_are_parsed_strictly() {
        let k = key(0xab);
        assert_eq!(
            ProxyCache::parse_name(&format!("12-{k}.mp4")),
            Some((FileId::new(12), k.clone()))
        );
        for bad in [
            format!("0-{k}.mp4"),
            format!("x-{k}.mp4"),
            format!("12-{k}.mov"),
            format!("12-{}.mp4", &k[1..]),
            format!("12-{}.mp4", k.to_uppercase()),
            "12.mp4".to_owned(),
        ] {
            assert_eq!(ProxyCache::parse_name(&bad), None, "{bad}");
        }
    }

    /// 上限を超えたら、最後に使った日時の古いものから消す。守るものは消さない。使った記録で順番が変わる。
    #[test]
    fn eviction_is_least_recently_used() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ProxyCache::open(dir.path().join(VIDEO_PROXY_DIR), 250).unwrap();
        let old = write(&cache, 1, &key(1), 100, 300);
        let mid = write(&cache, 2, &key(2), 100, 200);
        let new = write(&cache, 3, &key(3), 100, 100);
        // 古いものを使うと、新しくなる。
        ProxyCache::touch(&old);
        let r = cache.evict_to(cache.capacity(), None).unwrap();
        assert_eq!((r.removed, r.freed_bytes, r.failed), (1, 100, 0));
        assert!(old.exists() && !mid.exists() && new.exists());
        // 守るものは、最も古くても消さない。
        let r = cache.evict_to(50, Some(&new)).unwrap();
        assert_eq!(r.removed, 1);
        assert!(!old.exists() && new.exists());
        let st = cache.status(0).unwrap();
        assert_eq!((st.count, st.total_bytes, st.capacity_bytes), (1, 100, 250));
    }

    /// 走査は、形の違う名前のファイルを数えず、古い一時ファイルだけを消す（書き込み中の新しいものは残す）。
    #[test]
    fn scan_ignores_foreign_files_and_removes_only_stale_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ProxyCache::open(dir.path().join(VIDEO_PROXY_DIR), 1000).unwrap();
        write(&cache, 1, &key(1), 10, 0);
        fs::write(cache.dir().join("readme.txt"), b"x").unwrap();
        fs::create_dir(cache.dir().join("sub")).unwrap();
        let fresh = cache.temp_path(&key(2));
        fs::write(&fresh, b"writing").unwrap();
        let stale = cache.temp_path(&key(3));
        fs::write(&stale, b"left").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(
                SystemTime::now()
                    - genzo_catalog::PREVIEW_TEMP_FILE_MIN_AGE
                    - Duration::from_secs(1),
            )
            .unwrap();
        let (files, temp_removed) = cache.scan().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_id, FileId::new(1));
        assert_eq!(temp_removed, 1);
        assert!(fresh.exists() && !stale.exists());
        assert!(cache.dir().join("readme.txt").exists());
        let r = cache.remove_where(|f| f.file_id == FileId::new(1)).unwrap();
        assert_eq!(r.removed, 1);
        assert_eq!(cache.status(r.removed).unwrap().count, 0);
    }

    /// 公開の前の同期（検証 R1）: 書き込みのハンドルで開いて同期し、大きさを返す（Windows の CI では、読み取り
    /// だけのハンドルの `FlushFileBuffers` がアクセス拒否になっていた。中身は変えない）。ないファイルはエラー。
    #[test]
    fn written_files_are_synced_through_a_writable_handle() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(".k.1.0.tmp");
        fs::write(&p, b"genzo-proxy").unwrap();
        assert_eq!(sync_written_file(&p).unwrap(), 11);
        assert_eq!(fs::read(&p).unwrap(), b"genzo-proxy");
        assert_eq!(
            sync_written_file(&dir.path().join("none.tmp"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn worker_env_wins_for_the_tools() {
        let mut config = crate::CoreConfig::new("/c/catalog.db", "/c/data");
        config.worker_env = vec![
            ("GENZO_FFMPEG".into(), "/opt/ff/ffmpeg".into()),
            ("GENZO_FFPROBE".into(), "/opt/ff/ffprobe".into()),
        ];
        let t = ffmpeg_tools(&config).unwrap();
        assert_eq!(t.ffmpeg, std::ffi::OsString::from("/opt/ff/ffmpeg"));
        assert_eq!(t.ffprobe, std::ffi::OsString::from("/opt/ff/ffprobe"));
        // アプリ（PATH に頼らない設定）で、片方しか見つかっていなければ使わない。
        config.ffmpeg_path_fallback = false;
        assert!(ffmpeg_tools(&config).is_some());
        config.worker_env.pop();
        assert!(ffmpeg_tools(&config).is_none());
        assert!(matches!(
            detect_support(&config),
            ProxySupport::Unavailable(_)
        ));
        config.ffmpeg_path_fallback = true;
        config.worker_env.clear();
        config
            .worker_env
            .push(("GENZO_FFMPEG".into(), "/nonexistent/genzo-ffmpeg".into()));
        config
            .worker_env
            .push(("GENZO_FFPROBE".into(), "/nonexistent/genzo-ffprobe".into()));
        assert!(matches!(
            detect_support(&config),
            ProxySupport::Unavailable(_)
        ));
    }
}
