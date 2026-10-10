//! 取り込み（IMP-01・VID-01・PRV-01、01 のストーリー 1。04 の 3.1 節・3.3 節・3.5 節・6.3 節）。
//!
//! 1. フォルダを走査する。**シンボリックリンク・ジャンクションの先はたどらない**（3.5 節。循環と二重
//!    登録を防ぐ）。名前が `.` で始まるものは飛ばす（[`crate::config::SKIP_HIDDEN_FILES`]）。Windows の
//!    ごみ箱（`$RECYCLE.BIN` など）・`System Volume Information` と、Windows で隠し属性とシステム属性の
//!    両方を持つ項目も飛ばす（隠し属性だけの項目は飛ばさない。利用者が自分で隠したフォルダを除かないため）。
//!    **アプリのデータのフォルダ**（データのフォルダ・L1 プレビューのキャッシュ・バックアップの置き場。
//!    [`excluded_dirs`]）の中は飛ばす。キャッシュの JPEG を写真として登録すると、キャッシュの回収で
//!    「元ファイル」が消える・ゴミ箱へ移せてしまうため。取り込むフォルダそのものがその中ならエラー。
//!    **大文字・小文字か Unicode の正規化だけが違う名前**（カタログの比較キー `path_key` が同じで、バイト列の
//!    違う名前）のファイル同士・フォルダ同士が同じフォルダにあれば、どれも登録せずに報告する（カタログでは
//!    区別できず、1 つのレコードにまとめてしまうため。区別するボリューム（Linux、大文字・小文字を区別する
//!    APFS、Windows のフォルダごとの設定、NTFS の NFC と NFD）でだけ起きる。F08）。登録済みのフォルダと
//!    比較キーが同じで名前の違うフォルダを取り込む場合も、登録済みの名前のフォルダが別に実在すれば、その
//!    フォルダのファイルは登録しない。
//! 2. 拡張子で写真（RAW: [`RAW_EXTENSIONS`]、画像: [`IMAGE_EXTENSIONS`]）と動画（[`VIDEO_EXTENSIONS`]）を
//!    判定する。
//! 3. ファイルごとに、サイズ・更新日時・クイックハッシュを求める（3.3 節）。登録済みで変化のない
//!    ファイルは解析を省く（途中で中断しても、やり直すと続きから進む）。
//! 4. バッチ用ワーカーでメタデータを読む（写真: `ProbePhoto`、動画: `ProbeVideo`）。撮影日時は
//!    genzo-model の `CaptureTime`（オフセットがなければ既定のタイムゾーン。3.1 節）。
//!    **読めないファイルは `status = error` と理由を記録して続ける**（6.3 節）。
//! 5. [`IMPORT_BATCH_FILES`] 件ずつ、1 つのトランザクションで登録する（冪等。同じフォルダを登録し直しても
//!    件数は増えない。3.5 節）。
//! 6. 続けて、ワーカーでサムネイル（写真: `Thumbnail`、動画: `VideoThumbnail`。B5 の JPEG）を作り、L0 として
//!    thumbs.db に保存する（PRV-01）。
//! 7. 終わったら、現像結果からの L0 / L1 の作り直しを投入する（4 章「最初の表示」。設定で止められる）。
//!
//! P3 のジョブとして実行し、進捗をイベントで知らせ、取り消せる（バッチの区切りで確かめ、ワーカーの
//! 処理中なら、そのワーカーを止める）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use genzo_catalog::{FileFacts, RegisterFile, RegisterStatus};
use genzo_media::CacheSpec;
use genzo_model::{AssetKind, CaptureTime, FileRole, FileStatus, FolderId, TzSource, VariantId};
use genzo_worker::{Lane, WorkerClientError};
use parking_lot::Mutex;

use crate::config::{IMPORT_BATCH_FILES, SKIP_HIDDEN_FILES};
use crate::core::Core;
use crate::error::ApiError;
use crate::events::{CatalogChange, Event};
use crate::jobs::{JobCtx, spawn_job};
use crate::paths::{absolute_lexical, join_rel, split_volume};
use crate::previews::{source_thumb_rev, spawn_regenerate};
use crate::types::{FileIssue, ImportReport, JobKind, JobResult};

/// RAW として取り込む拡張子（小文字）。**仮置き**: 対象機種（α7 IV / α7C）の ARW と、DNG。
pub const RAW_EXTENSIONS: &[&str] = &["arw", "dng"];
/// RAW 以外の写真として取り込む拡張子（小文字）。
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "tif", "tiff", "png"];
/// 動画として取り込む拡張子（小文字。DJI Osmo Pocket の MP4 / MOV。01 の 7 章の仮定 3）。
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov"];

/// ファイル名の拡張子から、写真か動画か（対象外なら `None`）。
pub fn media_kind(name: &str) -> Option<AssetKind> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    if RAW_EXTENSIONS.contains(&ext.as_str()) || IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        Some(AssetKind::Photo)
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(AssetKind::Video)
    } else {
        None
    }
}

/// 取り込む候補のファイル。
#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    /// フォルダのボリューム内の相対パス。
    dir_rel: String,
    name: String,
    kind: AssetKind,
}

/// 走査の結果。
#[derive(Default)]
struct Scan {
    files: Vec<Candidate>,
    dirs: Vec<String>,
    skipped_links: u64,
    unreadable: Vec<FileIssue>,
}

fn is_hidden(name: &str) -> bool {
    SKIP_HIDDEN_FILES && name.starts_with('.')
}

/// Windows のごみ箱とシステムのフォルダの名前（大文字・小文字を区別しない）。ドライブのルートを取り込んだ
/// ときに、ごみ箱に入れた写真（`$R…` は元の拡張子のまま残る）を登録しないため。名前で判定するので、
/// macOS・Linux で Windows 用のドライブを取り込むときにも効く。
const WINDOWS_SYSTEM_DIRS: &[&str] = &[
    "$recycle.bin",
    "recycler",
    "recycled",
    "system volume information",
];

/// 走査しないシステムの項目か（Windows のごみ箱・システムのフォルダ。Windows では、隠し属性とシステム
/// 属性の両方を持つ項目（エクスプローラーが「隠しファイルを表示」でも見せない、OS の保護されたファイル）も）。
fn is_system_entry(name: &str, entry: &fs::DirEntry) -> bool {
    if WINDOWS_SYSTEM_DIRS
        .iter()
        .any(|n| name.eq_ignore_ascii_case(n))
    {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        /// FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM（Win32 のファイルの属性）。
        const HIDDEN_SYSTEM: u32 = 0x2 | 0x4;
        if entry
            .metadata()
            .is_ok_and(|m| m.file_attributes() & HIDDEN_SYSTEM == HIDDEN_SYSTEM)
        {
            return true;
        }
    }
    #[cfg(not(windows))]
    let _ = entry;
    false
}

/// 取り込みで飛ばすフォルダ（アプリのデータ）。パスの書き方の違い（相対パス・`..`）と、リンクを経由した
/// 指定（macOS の `/var` → `/private/var` など）の両方で照合できるよう、字面の絶対パスと、解決できれば
/// 実際のパスを持つ。
#[derive(Debug, Default, Clone)]
pub(crate) struct ExcludedDirs {
    lexical: Vec<PathBuf>,
    canonical: Vec<PathBuf>,
}

impl ExcludedDirs {
    /// 照合するフォルダを作る（字面の絶対パスにできないものは無視する）。
    pub(crate) fn new<'a>(dirs: impl IntoIterator<Item = &'a Path>) -> Self {
        let mut out = Self::default();
        for d in dirs {
            if let Ok(l) = absolute_lexical(d)
                && !out.lexical.contains(&l)
            {
                out.lexical.push(l);
            }
            if let Ok(c) = fs::canonicalize(d)
                && !out.canonical.contains(&c)
            {
                out.canonical.push(c);
            }
        }
        out
    }

    /// `lexical`（字面の絶対パス）か `canonical`（実際のパス）が、飛ばすフォルダか、その中か。
    fn contains(&self, lexical: &Path, canonical: Option<&Path>) -> bool {
        self.lexical.iter().any(|e| lexical.starts_with(e))
            || canonical.is_some_and(|c| self.canonical.iter().any(|e| c.starts_with(e)))
    }
}

/// 取り込みで飛ばすフォルダ: データのフォルダ、L1 プレビューのキャッシュ、バックアップの置き場
/// （データのフォルダの外に置く設定もあるため、3 つとも照合する）。
pub(crate) fn excluded_dirs(inner: &crate::core::Inner) -> ExcludedDirs {
    let preview_dir = inner.settings.lock().preview_cache_dir.clone();
    let backup_dir = inner.config.backup_dir();
    ExcludedDirs::new([
        inner.config.data_dir.as_path(),
        preview_dir.as_path(),
        backup_dir.as_path(),
    ])
}

/// フォルダを走査する（シンボリックリンク・ジャンクションはたどらない。`excluded` の中は飛ばす）。
fn scan(
    root: &Path,
    root_rel: &str,
    recursive: bool,
    excluded: &ExcludedDirs,
    token: &genzo_jobs::CancellationToken,
) -> Scan {
    let mut out = Scan::default();
    // 実際のパス（リンクを解決したもの）。リンクはたどらないので、配下は「親の実際のパス / 名前」になる。
    let root_canonical = fs::canonicalize(root).ok();
    let mut stack = vec![(root.to_path_buf(), root_rel.to_owned(), root_canonical)];
    while let Some((dir, rel, dir_canonical)) = stack.pop() {
        if token.is_cancelled() {
            break;
        }
        out.dirs.push(rel.clone());
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                out.unreadable.push(FileIssue {
                    path: dir.clone(),
                    reason: format!("フォルダを読めません: {e}"),
                });
                continue;
            }
        };
        let mut items: Vec<(String, PathBuf, fs::FileType)> = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    out.unreadable.push(FileIssue {
                        path: dir.clone(),
                        reason: format!("フォルダの項目を読めません: {e}"),
                    });
                    continue;
                }
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                out.unreadable.push(FileIssue {
                    path: entry.path(),
                    reason: "UTF-8 で表せない名前は登録できません".to_owned(),
                });
                continue;
            };
            if is_hidden(&name) || is_system_entry(&name, &entry) {
                continue;
            }
            // file_type はリンクをたどらない（Windows のジャンクションもリンクとして扱われる）。
            match entry.file_type() {
                Ok(t) => items.push((name, entry.path(), t)),
                Err(e) => out.unreadable.push(FileIssue {
                    path: entry.path(),
                    reason: format!("種類を確かめられません: {e}"),
                }),
            }
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));
        // 比較キーだけが同じ名前（F08）。ファイル同士（取り込む対象）・フォルダ同士で確かめる。
        let file_clashes = clashing_names(
            items
                .iter()
                .filter(|(n, _, t)| !t.is_symlink() && t.is_file() && media_kind(n).is_some())
                .map(|(n, _, _)| n.as_str()),
        );
        let dir_clashes = clashing_names(
            items
                .iter()
                .filter(|(_, _, t)| recursive && !t.is_symlink() && t.is_dir())
                .map(|(n, _, _)| n.as_str()),
        );
        let mut subdirs = Vec::new();
        for (name, path, t) in items {
            if t.is_symlink() {
                out.skipped_links += 1;
            } else if t.is_dir() && dir_clashes.contains_key(&name) {
                out.unreadable.push(FileIssue {
                    reason: clash_reason("フォルダ", &dir_clashes[&name]),
                    path,
                });
            } else if t.is_file() && file_clashes.contains_key(&name) {
                out.unreadable.push(FileIssue {
                    reason: clash_reason("ファイル", &file_clashes[&name]),
                    path,
                });
            } else if t.is_dir() {
                let canonical = dir_canonical.as_ref().map(|c| c.join(&name));
                if recursive && !excluded.contains(&path, canonical.as_deref()) {
                    let sub_rel = if rel.is_empty() {
                        name.clone()
                    } else {
                        format!("{rel}/{name}")
                    };
                    subdirs.push((path, sub_rel, canonical));
                }
            } else if t.is_file()
                && let Some(kind) = media_kind(&name)
            {
                out.files.push(Candidate {
                    path,
                    dir_rel: rel.clone(),
                    name,
                    kind,
                });
            }
        }
        // 名前の順に処理するよう、逆順に積む。
        stack.extend(subdirs.into_iter().rev());
    }
    out
}

/// 名前の一覧のうち、カタログの比較キー（`path_key`。NFC ＋ 小文字化）が同じでバイト列の違う名前が
/// ほかにあるもの。戻り値は、その名前から、キーの同じほかの名前への対応（F08）。
fn clashing_names<'a>(
    names: impl Iterator<Item = &'a str>,
) -> std::collections::HashMap<String, Vec<String>> {
    let mut by_key: std::collections::HashMap<String, Vec<&str>> = std::collections::HashMap::new();
    for n in names {
        let group = by_key.entry(genzo_catalog::text::path_key(n)).or_default();
        if !group.contains(&n) {
            group.push(n);
        }
    }
    let mut out = std::collections::HashMap::new();
    for group in by_key.values().filter(|g| g.len() > 1) {
        for &n in group {
            let others = group
                .iter()
                .filter(|&&o| o != n)
                .map(|&o| o.to_owned())
                .collect();
            out.insert(n.to_owned(), others);
        }
    }
    out
}

/// 比較キーだけが同じ名前を登録しない理由（`what` は「ファイル」か「フォルダ」）。
fn clash_reason(what: &str, others: &[String]) -> String {
    format!(
        "大文字・小文字か Unicode の正規化だけが違う名前の{what}（{}）が同じフォルダにあり、カタログでは区別できないため登録しません（どちらかの名前を変えてから取り込み直してください）",
        others.join("、")
    )
}

/// 1 ファイルの準備の結果。
enum Prepared {
    /// 登録する（メタデータを読めなかった場合は `request.error` に理由）。
    Register(Box<RegisterFile>),
    /// 登録済みで変化がない（解析を省いた）。サムネイルがなければ作る。
    Unchanged {
        master: VariantId,
        file_id: genzo_model::FileId,
        revision: u32,
        needs_thumbnail: bool,
    },
    /// 登録できない（読めない）。
    NotRegistered(String),
    /// 取り消された。
    Cancelled,
}

/// `items` を最大 `threads` 本のスレッドで処理する（結果は入力の順）。
fn parallel_map<T: Sync, R: Send>(
    items: &[T],
    threads: usize,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    if threads <= 1 || items.len() <= 1 {
        return items.iter().map(&f).collect();
    }
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..threads.min(items.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= items.len() {
                        break;
                    }
                    *results[i].lock() = Some(f(&items[i]));
                }
            });
        }
    });
    results
        .into_iter()
        .map(|m| m.into_inner().expect("すべての項目を処理した"))
        .collect()
}

/// 1 ファイルを準備する（変化の確認とメタデータの読み取り）。
fn prepare(ctx: &JobCtx<'_>, c: &Candidate, folder: FolderId) -> Prepared {
    let inner = ctx.inner;
    if ctx.is_cancelled() {
        return Prepared::Cancelled;
    }
    let facts = match FileFacts::read(&c.path) {
        Ok(f) => f,
        Err(e) => return Prepared::NotRegistered(format!("ファイルを読めません: {e}")),
    };
    // 登録済みで変化がなければ、解析を省く（エラーだったファイルは読み直す）。
    let existing = inner.with_catalog_api(|cat| {
        let Some(rec) = cat.find_file(folder, &c.name)? else {
            return Ok(None);
        };
        if rec.facts != facts || rec.status != FileStatus::Ok {
            return Ok(None);
        }
        let master = cat
            .variants_of_asset(rec.asset_id)?
            .into_iter()
            .find(|v| v.is_master)
            .map(|v| v.id);
        Ok(master.map(|m| (m, rec)))
    });
    match existing {
        Ok(Some((master, rec))) => {
            let needs_thumbnail = rec.role == FileRole::Primary
                && inner
                    .with_cache(|cache| cache.thumbs.cache_key(master))
                    .map(|k| k.is_none())
                    .unwrap_or(false);
            return Prepared::Unchanged {
                master,
                file_id: rec.id,
                revision: rec.revision,
                needs_thumbnail,
            };
        }
        Ok(None) => {}
        Err(e) => return Prepared::NotRegistered(e.to_string()),
    }
    let pool = match inner.pool() {
        Ok(p) => p,
        Err(e) => return Prepared::NotRegistered(e.to_string()),
    };
    let offset = inner.default_offset();
    let target = ProbeTarget {
        path: &c.path,
        kind: c.kind,
        folder,
        name: &c.name,
    };
    match probe_request(&pool, &target, facts, offset, Lane::Batch, ctx.token()) {
        Ok(r) => Prepared::Register(Box::new(r)),
        Err(WorkerClientError::Cancelled) => Prepared::Cancelled,
        Err(e) => Prepared::NotRegistered(format!(
            "ワーカーを起動できないため登録しませんでした（もう一度取り込むと処理します）: {e}"
        )),
    }
}

/// メタデータを読むファイル（[`probe_request`]）。
pub(crate) struct ProbeTarget<'a> {
    /// ファイルの絶対パス。
    pub path: &'a Path,
    /// 写真か動画か。
    pub kind: AssetKind,
    /// フォルダ。
    pub folder: FolderId,
    /// ファイル名。
    pub name: &'a str,
}

/// ワーカーでメタデータを読み、登録の要求を作る（取り込み・ファイルの確認で共通）。撮影日時は既定の
/// オフセット `offset` で推定する。
///
/// 読めないファイルは、`error` に理由を入れた要求にする（`status = error` で登録する。6.3 節）。
/// 取り消し（[`WorkerClientError::Cancelled`]）とワーカーを起動できない場合
/// （[`WorkerClientError::Spawn`]）は `Err` を返す（登録しない）。ワーカーの問題で、ファイルの問題では
/// ないため（`status = error` で登録すると、問題のないファイルがエラーの一覧に並び、内容が同じままでは
/// メタデータを読み直さない）。
pub(crate) fn probe_request(
    pool: &genzo_worker::WorkerPool,
    target: &ProbeTarget<'_>,
    facts: FileFacts,
    offset: chrono::FixedOffset,
    lane: Lane,
    token: &genzo_jobs::CancellationToken,
) -> Result<RegisterFile, WorkerClientError> {
    let (folder, name) = (target.folder, target.name);
    let result = match target.kind {
        AssetKind::Photo => pool.probe_photo(lane, target.path, token).map(|p| {
            let capture = CaptureTime::from_capture_info(&p.metadata.capture, offset);
            RegisterFile::photo(folder, name, facts.clone(), p.metadata, capture)
        }),
        AssetKind::Video => pool.probe_video(lane, target.path, token).map(|v| {
            let capture =
                CaptureTime::resolve_lossy(v.metadata.creation_time.as_deref(), None, offset);
            RegisterFile::video(folder, name, facts.clone(), v.metadata, capture)
        }),
    };
    match result {
        Ok(r) => Ok(r),
        Err(e @ (WorkerClientError::Cancelled | WorkerClientError::Spawn(_))) => Err(e),
        Err(e) => Ok(RegisterFile {
            folder_id: folder,
            name: name.to_owned(),
            facts,
            kind: target.kind,
            metadata: genzo_catalog::MediaMetadata::None,
            capture: CaptureTime::unknown(),
            error: Some(e.to_string()),
        }),
    }
}

/// サムネイルを作る対象。
struct ThumbTask {
    variant: VariantId,
    file_id: genzo_model::FileId,
    revision: u32,
    path: PathBuf,
    kind: AssetKind,
}

/// 取り込みの本体。
fn run_import(ctx: &JobCtx<'_>, root: &Path, recursive: bool) -> Result<ImportReport, ApiError> {
    let inner = ctx.inner;
    let root = absolute_lexical(root)?;
    let meta = fs::metadata(&root).map_err(|e| ApiError::FileAccess {
        path: root.clone(),
        message: e.to_string(),
    })?;
    if !meta.is_dir() {
        return Err(ApiError::InvalidArgument(format!(
            "フォルダではありません: {}",
            root.display()
        )));
    }
    let excluded = excluded_dirs(inner);
    if excluded.contains(&root, fs::canonicalize(&root).ok().as_deref()) {
        return Err(ApiError::InvalidArgument(format!(
            "アプリのデータのフォルダ（プレビューのキャッシュ・バックアップ）は取り込めません: {}",
            root.display()
        )));
    }
    let vol = split_volume(&root)?;
    let mut report = ImportReport {
        root: root.clone(),
        ..Default::default()
    };
    let scanned = scan(&root, &vol.rel, recursive, &excluded, ctx.token());
    report.files_found = scanned.files.len() as u64;
    report.skipped_links = scanned.skipped_links;
    report.not_registered.extend(scanned.unreadable);
    if ctx.is_cancelled() {
        report.cancelled = true;
        return Ok(report);
    }
    let mount = vol
        .mount
        .to_str()
        .ok_or_else(|| ApiError::InvalidArgument("UTF-8 で表せないパスです".to_owned()))?
        .to_owned();
    // フォルダを確保する（取り込んだフォルダは、ファイルがなくてもフォルダツリーに出す）。
    let mut stored_rel: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let folders: std::collections::HashMap<String, FolderId> = inner.with_catalog(|c| {
        let volume = c.ensure_volume(&vol.uuid, None, Some(&mount))?;
        let mut map = std::collections::HashMap::new();
        for rel in &scanned.dirs {
            if !map.contains_key(rel) {
                let id = c.ensure_folder(volume, rel)?;
                stored_rel.insert(rel.clone(), c.folder(id)?.rel_path);
                map.insert(rel.clone(), id);
            }
        }
        Ok(map)
    })?;
    // 登録済みのフォルダ（比較キーが同じで、記録した名前が違うもの）が別のフォルダとして実在すれば、走査した
    // フォルダのファイルは登録しない（登録済みのフォルダのレコードにまとめると、記録するパスが別のフォルダを
    // 指すため。F08）。同じフォルダ（大文字・小文字を区別しないボリュームで、名前の書き方が違うだけ）なら
    // 登録する。
    let clashing_dirs: std::collections::HashMap<&str, &str> = stored_rel
        .iter()
        .filter(|(rel, stored)| {
            if rel == stored {
                return false;
            }
            let stored_path = join_rel(&vol.mount, stored, "");
            let scanned_path = join_rel(&vol.mount, rel, "");
            stored_path.exists()
                && !same_file::is_same_file(&stored_path, &scanned_path).unwrap_or(false)
        })
        .map(|(rel, stored)| (rel.as_str(), stored.as_str()))
        .collect();
    let mut files = Vec::with_capacity(scanned.files.len());
    for c in scanned.files {
        match clashing_dirs.get(c.dir_rel.as_str()) {
            Some(stored) => report.not_registered.push(FileIssue {
                reason: format!(
                    "登録済みのフォルダ {} と、大文字・小文字か Unicode の正規化だけが違う名前のフォルダにあり、カタログでは区別できないため登録しません",
                    join_rel(&vol.mount, stored, "").display()
                ),
                path: c.path,
            }),
            None => files.push(c),
        }
    }
    let threads = inner.pool()?.worker_count(Lane::Batch);
    let total = files.len() as u64;
    let mut done = 0u64;
    let mut regenerate: Vec<VariantId> = Vec::new();
    ctx.progress(0, total);
    for chunk in files.chunks(IMPORT_BATCH_FILES) {
        if ctx.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let prepared = parallel_map(chunk, threads, |c| prepare(ctx, c, folders[&c.dir_rel]));
        if prepared.iter().any(|p| matches!(p, Prepared::Cancelled)) {
            report.cancelled = true;
            break;
        }
        // 登録（1 つのトランザクション）。
        let mut requests: Vec<RegisterFile> = Vec::new();
        let mut request_index: Vec<usize> = Vec::new();
        let mut thumbs: Vec<ThumbTask> = Vec::new();
        for (i, p) in prepared.into_iter().enumerate() {
            match p {
                Prepared::Register(r) => {
                    if let Some(reason) = &r.error {
                        report.errors.push(FileIssue {
                            path: chunk[i].path.clone(),
                            reason: reason.clone(),
                        });
                    }
                    requests.push(*r);
                    request_index.push(i);
                }
                Prepared::Unchanged {
                    master,
                    file_id,
                    revision,
                    needs_thumbnail,
                } => {
                    report.unchanged += 1;
                    if needs_thumbnail {
                        thumbs.push(ThumbTask {
                            variant: master,
                            file_id,
                            revision,
                            path: chunk[i].path.clone(),
                            kind: chunk[i].kind,
                        });
                    }
                }
                Prepared::NotRegistered(reason) => report.not_registered.push(FileIssue {
                    path: chunk[i].path.clone(),
                    reason,
                }),
                Prepared::Cancelled => unreachable!("上で確かめた"),
            }
        }
        let outcomes = inner.with_catalog_api(|c| {
            // 既定のタイムゾーンで推定する撮影日時は、登録の直前（カタログのロックの中）の設定で推定し直す
            // （準備の後に設定が変わった場合、推定し直しのジョブの一覧より後に登録されたものが古い
            // オフセットのまま残らないように）。
            let offset = inner.default_offset();
            for r in &mut requests {
                if r.capture.tz_source == TzSource::UserDefault {
                    r.capture = r
                        .capture
                        .with_default_offset(offset)
                        .map_err(|e| ApiError::Internal(e.to_string()))?;
                }
            }
            Ok(c.register_batch(&requests)?)
        })?;
        let mut changed: Vec<VariantId> = Vec::new();
        for ((req, out), &i) in requests.iter().zip(&outcomes).zip(&request_index) {
            match out.status {
                RegisterStatus::Added => report.added += 1,
                RegisterStatus::Updated => report.updated += 1,
                RegisterStatus::Unchanged => report.unchanged += 1,
            }
            if out.status != RegisterStatus::Unchanged {
                changed.push(out.master_variant_id);
            }
            let ok = req.error.is_none();
            if ok && out.role == FileRole::Primary {
                let has_thumb = inner
                    .with_cache(|cache| cache.thumbs.cache_key(out.master_variant_id))?
                    .is_some();
                if out.status != RegisterStatus::Unchanged || !has_thumb {
                    thumbs.push(ThumbTask {
                        variant: out.master_variant_id,
                        file_id: out.file_id,
                        revision: out.revision,
                        path: chunk[i].path.clone(),
                        kind: req.kind,
                    });
                }
                if req.kind == AssetKind::Photo && out.status != RegisterStatus::Unchanged {
                    regenerate.push(out.master_variant_id);
                }
            }
        }
        changed.sort();
        changed.dedup();
        if !changed.is_empty() {
            inner.events.emit(Event::CatalogChanged {
                change: CatalogChange::Imported,
                variant_ids: changed,
                all: false,
            });
            crate::search::refresh(inner, &[]);
        }
        // サムネイル（L0。PRV-01）。
        let results = parallel_map(&thumbs, threads, |t| make_source_thumbnail(ctx, t));
        for (t, r) in thumbs.iter().zip(results) {
            match r {
                Ok(()) => report.thumbnails += 1,
                Err(e) if e.is_cancelled() => report.cancelled = true,
                Err(e) => report.thumbnail_failures.push(FileIssue {
                    path: t.path.clone(),
                    reason: e.to_string(),
                }),
            }
        }
        done += chunk.len() as u64;
        ctx.progress(done, total);
        if report.cancelled {
            break;
        }
    }
    if inner.config.render_previews_after_import && !report.cancelled && !regenerate.is_empty() {
        regenerate.sort();
        regenerate.dedup();
        spawn_regenerate(inner, regenerate);
    }
    Ok(report)
}

/// ワーカーで B5 のサムネイルを作り、L0 として保存する（PRV-01）。
fn make_source_thumbnail(ctx: &JobCtx<'_>, t: &ThumbTask) -> Result<(), ApiError> {
    let inner = ctx.inner;
    let pool = inner.pool()?;
    let thumb = match t.kind {
        AssetKind::Photo => {
            pool.thumbnail(Lane::Batch, &t.path, CacheSpec::L0_THUMBNAIL, ctx.token())?
        }
        AssetKind::Video => {
            pool.video_thumbnail(Lane::Batch, &t.path, CacheSpec::L0_THUMBNAIL, ctx.token())?
        }
    };
    let rev = source_thumb_rev(&inner.cache_generation, t.file_id, t.revision);
    // 調整済みの写真のサムネイルが既にあれば（内容が変わったファイルの取り込み直しなど）、埋め込みの
    // サムネイルで置き換えず、現像結果から作り直す（調整が見た目から消えないように）。
    let mut regenerate = false;
    let stored = inner.with_catalog_api(|c| {
        let file = c.file(t.file_id)?;
        if file.revision != t.revision {
            return Ok(false);
        }
        let neutral = c.develop_state(t.variant)?.settings.is_neutral();
        inner.with_cache(|cache| {
            let current = cache.thumbs.cache_key(t.variant)?;
            if current.is_some() && !neutral {
                regenerate = true;
                return Ok(false);
            }
            cache.thumbs.put(t.variant, &rev, &thumb.jpeg)?;
            Ok(true)
        })
    })?;
    if stored {
        inner.events.emit(Event::ThumbnailUpdated {
            variant_id: t.variant,
            rev,
        });
    }
    if regenerate {
        crate::previews::schedule_regenerate(inner, t.variant);
    }
    Ok(())
}

impl Core {
    /// フォルダを取り込む（IMP-01・VID-01）。P3 のジョブとして実行し、ジョブの ID を返す。
    ///
    /// 結果（[`ImportReport`]）は [`Core::wait_job`]・[`Core::job`] で取れる。
    pub fn import_folder(&self, path: &Path, recursive: bool) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let root = path.to_path_buf();
        Ok(spawn_job(
            inner,
            JobKind::Import,
            "取り込み",
            move |ctx| run_import(ctx, &root, recursive).map(JobResult::Import),
        ))
    }
}

/// 起動中のコアの状態（取り込みのジョブの数など。テスト用）。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_kinds_by_extension() {
        assert_eq!(media_kind("DSC00001.ARW"), Some(AssetKind::Photo));
        assert_eq!(media_kind("a.dng"), Some(AssetKind::Photo));
        assert_eq!(media_kind("b.JPEG"), Some(AssetKind::Photo));
        assert_eq!(media_kind("c.tif"), Some(AssetKind::Photo));
        assert_eq!(media_kind("d.png"), Some(AssetKind::Photo));
        assert_eq!(media_kind("DJI_0001.MP4"), Some(AssetKind::Video));
        assert_eq!(media_kind("e.mov"), Some(AssetKind::Video));
        assert_eq!(media_kind("f.xmp"), None);
        assert_eq!(media_kind("noext"), None);
    }

    #[test]
    fn parallel_map_keeps_order() {
        let items: Vec<u32> = (0..50).collect();
        let out = parallel_map(&items, 3, |x| x * 2);
        assert_eq!(out, items.iter().map(|x| x * 2).collect::<Vec<_>>());
        assert_eq!(parallel_map(&items[..1], 4, |x| x + 1), vec![1]);
    }

    #[cfg(unix)]
    #[test]
    fn scan_does_not_follow_symlinks_and_skips_hidden() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        fs::create_dir_all(root.join("sub/deeper")).unwrap();
        fs::write(root.join("a.jpg"), b"x").unwrap();
        fs::write(root.join(".hidden.jpg"), b"x").unwrap();
        fs::write(root.join("notes.txt"), b"x").unwrap();
        fs::write(root.join("sub/b.MP4"), b"x").unwrap();
        fs::write(root.join("sub/deeper/c.dng"), b"x").unwrap();
        // ループになるリンクと、ファイルへのリンク。
        std::os::unix::fs::symlink(root, root.join("sub/loop")).unwrap();
        std::os::unix::fs::symlink(root.join("a.jpg"), root.join("link.jpg")).unwrap();
        let token = genzo_jobs::CancellationToken::new();
        let none = ExcludedDirs::default();
        let s = scan(root, "r", true, &none, &token);
        let names: Vec<(&str, &str)> = s
            .files
            .iter()
            .map(|c| (c.dir_rel.as_str(), c.name.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("r", "a.jpg"),
                ("r/sub", "b.MP4"),
                ("r/sub/deeper", "c.dng")
            ]
        );
        assert_eq!(s.skipped_links, 2);
        assert_eq!(s.dirs, vec!["r", "r/sub", "r/sub/deeper"]);
        let flat = scan(root, "r", false, &none, &token);
        assert_eq!(flat.files.len(), 1);
    }

    /// Windows のごみ箱（`$RECYCLE.BIN` など）とシステムのフォルダは走査しない（ドライブのルートを
    /// 取り込んだときに、ごみ箱に入れた写真がカタログに戻らないように。指摘 F39）。名前で判定するので、
    /// macOS・Linux で Windows 用のドライブ（exFAT など）を取り込む場合にも効く。
    #[test]
    fn scan_skips_the_windows_recycle_bin_and_system_folders() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let sid = "S-1-5-21-1111111111-2222222222-3333333333-1001";
        for rel in [
            format!("$RECYCLE.BIN/{sid}/$R4K7P1A.JPG"),
            format!("$RECYCLE.BIN/{sid}/$I4K7P1A.JPG"),
            format!("$Recycle.Bin/{sid}/$RQ8WX2Z/IMG_0100.JPG"),
            "RECYCLER/x.jpg".to_owned(),
            "Recycled/y.jpg".to_owned(),
            "System Volume Information/z.jpg".to_owned(),
            "Photos/IMG_0001.JPG".to_owned(),
        ] {
            let p = root.join(&rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
        }
        let token = genzo_jobs::CancellationToken::new();
        let s = scan(root, "", true, &ExcludedDirs::default(), &token);
        let names: Vec<(&str, &str)> = s
            .files
            .iter()
            .map(|c| (c.dir_rel.as_str(), c.name.as_str()))
            .collect();
        assert_eq!(names, vec![("Photos", "IMG_0001.JPG")]);
        assert!(s.unreadable.is_empty(), "{:?}", s.unreadable);
        assert_eq!(s.dirs, vec!["", "Photos"]);
    }

    /// アプリのデータのフォルダ（プレビューのキャッシュの JPEG など）は取り込まない。字面の違う指定
    /// （`..` を含む相対的な書き方）でも飛ばす。
    #[test]
    fn scan_skips_the_app_data_dirs() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        fs::create_dir_all(root.join("photos")).unwrap();
        fs::create_dir_all(root.join("genzo/catalog-data/previews/ab/cd")).unwrap();
        fs::create_dir_all(root.join("elsewhere/backups")).unwrap();
        fs::write(root.join("photos/a.jpg"), b"x").unwrap();
        fs::write(
            root.join("genzo/catalog-data/previews/ab/cd/0123.jpg"),
            b"x",
        )
        .unwrap();
        fs::write(root.join("elsewhere/backups/b.jpg"), b"x").unwrap();
        fs::write(root.join("elsewhere/c.jpg"), b"x").unwrap();
        let data = root
            .join("photos")
            .join("..")
            .join("genzo")
            .join("catalog-data");
        let excluded =
            ExcludedDirs::new([data.as_path(), root.join("elsewhere/backups").as_path()]);
        let token = genzo_jobs::CancellationToken::new();
        let s = scan(root, "r", true, &excluded, &token);
        let names: Vec<&str> = s.files.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["c.jpg", "a.jpg"]);
        assert!(
            !s.dirs
                .iter()
                .any(|d| d.contains("catalog-data") || d.contains("backups"))
        );
        assert!(excluded.contains(&root.join("genzo/catalog-data/previews"), None));
        assert!(
            !excluded.contains(&root.join("genzo"), None),
            "親のフォルダは対象"
        );
        assert!(
            !excluded.contains(&root.join("genzo/catalog-data-old"), None),
            "名前の前方一致ではない"
        );
    }
}
