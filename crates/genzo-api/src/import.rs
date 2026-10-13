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
//!    **重複の確認**（IMP-04。3.11 節。`identity` の doc）: 新しく登録するファイル（同じ場所に登録がないもの）
//!    について、クイックハッシュが同じ登録済みのファイルがあれば、全体のハッシュで同じ内容かを確かめる。
//!    「重複と思われるものを取り込まない」設定（[`ImportOptions::skip_duplicates`]。既定はカタログの設定で、
//!    その既定は有効）なら、メタデータを読まずに飛ばす。同じ取り込みの中の同じ内容のファイルも、2 つ目からを
//!    重複とする。結果の [`ImportReport::duplicates`] に、取り込まなかったもの・設定により登録したものを
//!    並べる。
//! 5. [`IMPORT_BATCH_FILES`] 件ずつ、1 つのトランザクションで登録する（冪等。同じフォルダを登録し直しても
//!    件数は増えない。3.5 節）。同じフォルダで拡張子だけが違う RAW と JPEG は 1 枚にまとめる（IMP-06。
//!    [`ImportOptions::separate_raw_jpeg`] で別々にもできる）。重複の確認で求めた全体のハッシュは記録する。
//! 6. 続けて、ワーカーでサムネイル（写真: `Thumbnail`、動画: `VideoThumbnail`。B5 の JPEG）を作り、L0 として
//!    thumbs.db に保存する（PRV-01）。
//! 7. 終わったら、現像結果からの L0 / L1 の作り直しを投入する（4 章「最初の表示」。設定で止められる）。
//!    内容のハッシュのバックグラウンドの計算（ORG-03）も頼む（アプリが許し、設定が有効なときだけ）。
//!
//! P3 のジョブとして実行し、進捗をイベントで知らせ、取り消せる（バッチの区切りで確かめ、ワーカーの
//! 処理中なら、そのワーカーを止める）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use genzo_catalog::{FileFacts, RegisterFile, RegisterOptions, RegisterStatus};
use genzo_media::CacheSpec;
use genzo_model::{AssetKind, CaptureTime, FileRole, FileStatus, FolderId, VariantId};
use genzo_worker::{Lane, WorkerClientError, WorkerErrorKind};
use parking_lot::Mutex;

use crate::config::{IMPORT_BATCH_FILES, SKIP_HIDDEN_FILES};
use crate::core::Core;
use crate::error::ApiError;
use crate::events::{CatalogChange, Event};
use crate::identity::{ContentCheck, ExistingCopy, ImportSeen, check_catalog_duplicate};
use crate::jobs::{JobCtx, spawn_job};
use crate::paths::{VolumePath, absolute_lexical, join_rel};
use crate::previews::{source_thumb_rev, spawn_regenerate};
use crate::types::{DuplicateFile, FileIssue, ImportOptions, ImportReport, JobKind, JobResult};

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
pub(crate) struct Candidate {
    pub path: PathBuf,
    /// フォルダのボリューム内の相対パス。
    pub dir_rel: String,
    pub name: String,
    pub kind: AssetKind,
}

/// 走査の結果。
#[derive(Default)]
pub(crate) struct Scan {
    pub files: Vec<Candidate>,
    pub dirs: Vec<String>,
    pub skipped_links: u64,
    pub unreadable: Vec<FileIssue>,
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

    /// 照合するフォルダを足す（コピー取り込みのコピー先など）。
    pub(crate) fn add(&mut self, dir: &Path) {
        let more = Self::new([dir]);
        for l in more.lexical {
            if !self.lexical.contains(&l) {
                self.lexical.push(l);
            }
        }
        for c in more.canonical {
            if !self.canonical.contains(&c) {
                self.canonical.push(c);
            }
        }
    }

    /// `lexical`（字面の絶対パス）か `canonical`（実際のパス）が、飛ばすフォルダか、その中か。
    pub(crate) fn contains(&self, lexical: &Path, canonical: Option<&Path>) -> bool {
        self.lexical.iter().any(|e| lexical.starts_with(e))
            || canonical.is_some_and(|c| self.canonical.iter().any(|e| c.starts_with(e)))
    }
}

/// 取り込みで飛ばすフォルダ: データのフォルダ、L1 プレビューのキャッシュ、バックアップの置き場
/// （データのフォルダの外に置く設定もあるため、すべて照合する）。
///
/// L1 プレビューのキャッシュは、**今開いているキャッシュのフォルダ**（[`genzo_catalog::PreviewCache::root`]）と、
/// **設定に記録されたフォルダ**の両方を飛ばす。設定の場所は、変えても次に開くまで有効にならない（今のキャッシュは
/// 開き直さず、移さない。[`crate::Core::update_settings`]）ので、設定だけを見ると、設定を変えた後に、まだ使っている
/// 古い場所の生成済みの JPEG を写真として登録してしまう。起動時に設定の場所を開けず既定の場所で開いた場合も、
/// 開いているフォルダと設定のフォルダが違う。どちらも、この関数を呼ぶたびに今の状態から求める（開き直した後に
/// 古い場所を覚えておくことはしない）。データのフォルダとバックアップの置き場は、起動の設定（[`CoreConfig`]）で
/// 決まり、開いている間は変わらない。
///
/// [`CoreConfig`]: crate::config::CoreConfig
pub(crate) fn excluded_dirs(inner: &crate::core::Inner) -> ExcludedDirs {
    // 2 つのロックは続けて取らず、それぞれ値を取り出してすぐに放す（ほかの処理とのロックの順序を作らない）。
    let configured_preview_dir = inner.settings.lock().preview_cache_dir.clone();
    let open_preview_dir = inner
        .cache
        .lock()
        .as_ref()
        .map(|c| c.previews.root().to_path_buf());
    let backup_dir = inner.config.backup_dir();
    let mut dirs: Vec<&Path> = vec![
        inner.config.data_dir.as_path(),
        configured_preview_dir.as_path(),
        backup_dir.as_path(),
    ];
    dirs.extend(open_preview_dir.as_deref());
    ExcludedDirs::new(dirs)
}

/// 走査したフォルダ（取り込むフォルダ `root` のボリューム `vol` の中の相対パス `rel`）のボリューム。Unix では、
/// 装置番号が取り込むフォルダと違えば（走査が別のボリュームのマウント先に入った）、そのボリュームとする。
/// Windows では、ボリュームを割り当てたフォルダ（マウント先のフォルダ）を見分けず、取り込むフォルダのボリュームと
/// する（実機で未確認）。
fn dir_volume(
    inner: &crate::core::Inner,
    root: &Path,
    vol: &VolumePath,
    rel: &str,
) -> Result<VolumePath, ApiError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let abs = join_rel(&vol.mount, rel, "");
        let crossed = match (fs::metadata(root), fs::metadata(&abs)) {
            (Ok(a), Ok(b)) => a.dev() != b.dev(),
            _ => false,
        };
        if crossed {
            return crate::volumes::locate(inner.volume_probe.as_ref(), &abs);
        }
    }
    #[cfg(not(unix))]
    let _ = (inner, root);
    Ok(VolumePath {
        rel: rel.to_owned(),
        ..vol.clone()
    })
}

/// フォルダを走査する（シンボリックリンク・ジャンクションはたどらない。`excluded` の中は飛ばす）。
pub(crate) fn scan(
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

/// 取り込みの設定（[`ImportOptions`] の `None` をカタログの設定で埋めたもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedOptions {
    pub skip_duplicates: bool,
    pub separate_raw_jpeg: bool,
}

/// 1 ファイルの準備の結果。
enum Prepared {
    /// 登録する（メタデータを読めなかった場合は `request.error` に理由）。
    Register {
        request: Box<RegisterFile>,
        /// 同じ場所に登録がない（新しいファイル）か。
        new: bool,
        /// 登録済みのファイルとの内容の比較（新しいファイルだけ。IMP-04）。
        check: ContentCheck,
    },
    /// 内容が同じファイルが登録済みなので、取り込まない（IMP-04）。
    Duplicate(ExistingCopy),
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
pub(crate) fn parallel_map<T: Sync, R: Send>(
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

/// 1 ファイルを準備する（変化の確認・重複の確認とメタデータの読み取り）。
fn prepare(
    ctx: &JobCtx<'_>,
    c: &Candidate,
    folder: FolderId,
    options: ResolvedOptions,
) -> Prepared {
    let inner = ctx.inner;
    if ctx.is_cancelled() {
        return Prepared::Cancelled;
    }
    let facts = match FileFacts::read(&c.path) {
        Ok(f) => f,
        Err(e) => return Prepared::NotRegistered(format!("ファイルを読めません: {e}")),
    };
    // 登録済みで変化がなければ、解析を省く（エラーだったファイルと、メタデータが空の asset（読めずに
    // 登録した後、見つからない状態を経て戻ったもの。implementation_status の No.45）は読み直す）。
    // 同じ場所に登録があるか（なければ新しいファイルとして、重複を確かめる）。
    let mut registered_here = false;
    let existing = inner.with_catalog_api(|cat| {
        let Some(rec) = cat.find_file(folder, &c.name)? else {
            return Ok(None);
        };
        registered_here = true;
        if rec.facts != facts || rec.status != FileStatus::Ok {
            return Ok(None);
        }
        if rec.role == FileRole::Primary && cat.asset_metadata_is_empty(rec.asset_id)? {
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
    // 重複の確認（IMP-04）。取り込まない設定なら、メタデータを読まずに飛ばす。
    let check = if registered_here {
        ContentCheck::default()
    } else {
        match check_catalog_duplicate(inner, &c.path, &facts, ctx.token()) {
            Ok(check) => check,
            Err(e) if e.is_cancelled() => return Prepared::Cancelled,
            Err(e) => return Prepared::NotRegistered(e.to_string()),
        }
    };
    if options.skip_duplicates
        && let Some(existing) = &check.duplicate_of
    {
        return Prepared::Duplicate(existing.clone());
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
        Ok(r) => Prepared::Register {
            request: Box::new(r),
            new: !registered_here,
            check,
        },
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
pub(crate) struct ThumbTask {
    pub variant: VariantId,
    pub file_id: genzo_model::FileId,
    pub revision: u32,
    pub path: PathBuf,
    pub kind: AssetKind,
}

/// 取り込みの本体。
fn run_import(
    ctx: &JobCtx<'_>,
    root: &Path,
    recursive: bool,
    options: ResolvedOptions,
    tags: &genzo_catalog::ImportTags,
) -> Result<ImportReport, ApiError> {
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
    // ボリューム（FILE-03。OS のボリューム ID。取れなければパスの先頭）。
    let vol = crate::volumes::locate(inner.volume_probe.as_ref(), &root)?;
    let mut report = ImportReport {
        root: root.clone(),
        skip_duplicates: options.skip_duplicates,
        separate_raw_jpeg: options.separate_raw_jpeg,
        ..Default::default()
    };
    let register_options = RegisterOptions {
        pair_raw_jpeg: !options.separate_raw_jpeg,
    };
    // 同じ取り込みの中で登録した（する）ファイル（同じ内容のファイルの 2 つ目からを重複とするため）。
    let mut seen = ImportSeen::default();
    let scanned = scan(&root, &vol.rel, recursive, &excluded, ctx.token());
    report.files_found = scanned.files.len() as u64;
    report.skipped_links = scanned.skipped_links;
    report.not_registered.extend(scanned.unreadable);
    if ctx.is_cancelled() {
        report.cancelled = true;
        return Ok(report);
    }
    // 走査したフォルダのボリューム（ふつうは取り込むフォルダと同じ。走査が別のボリュームのマウント先に入った
    // とき（Unix で装置番号が違う）は、そのボリューム。FILE-03）。
    let dir_vols: std::collections::HashMap<String, VolumePath> = scanned
        .dirs
        .iter()
        .map(|rel| Ok((rel.clone(), dir_volume(inner, &root, &vol, rel)?)))
        .collect::<Result<_, ApiError>>()?;
    // フォルダを確保する（取り込んだフォルダは、ファイルがなくてもフォルダツリーに出す）。
    // 走査の相対パス → （ボリュームのマウント先, 登録済みのフォルダの相対パス, 走査したフォルダの相対パス）。
    let mut stored_rel: std::collections::HashMap<String, (PathBuf, String, String)> =
        std::collections::HashMap::new();
    let folders: std::collections::HashMap<String, FolderId> = inner.with_catalog_api(|c| {
        let mut volumes: std::collections::HashMap<String, genzo_model::VolumeId> =
            std::collections::HashMap::new();
        let mut map = std::collections::HashMap::new();
        for rel in &scanned.dirs {
            if map.contains_key(rel) {
                continue;
            }
            let v = &dir_vols[rel];
            let volume = match volumes.get(&v.uuid) {
                Some(&id) => id,
                None => {
                    let mount = crate::volumes::mount_str(&v.mount)?;
                    let id = c.ensure_volume(&v.uuid, v.label.as_deref(), Some(&mount))?;
                    c.note_volume_seen(id, &mount, v.label.as_deref(), v.fs_type.as_deref())?;
                    volumes.insert(v.uuid.clone(), id);
                    id
                }
            };
            let id = c.ensure_folder(volume, &v.rel)?;
            stored_rel.insert(
                rel.clone(),
                (v.mount.clone(), c.folder(id)?.rel_path, v.rel.clone()),
            );
            map.insert(rel.clone(), id);
        }
        Ok(map)
    })?;
    // 登録済みのフォルダ（比較キーが同じで、記録した名前が違うもの）が別のフォルダとして実在すれば、走査した
    // フォルダのファイルは登録しない（登録済みのフォルダのレコードにまとめると、記録するパスが別のフォルダを
    // 指すため。F08）。同じフォルダ（大文字・小文字を区別しないボリュームで、名前の書き方が違うだけ）なら
    // 登録する。
    let clashing_dirs: std::collections::HashMap<&str, PathBuf> = stored_rel
        .iter()
        .filter(|(_, (mount, stored, scanned_rel))| {
            if stored == scanned_rel {
                return false;
            }
            let stored_path = join_rel(mount, stored, "");
            let scanned_path = join_rel(mount, scanned_rel, "");
            stored_path.exists()
                && !same_file::is_same_file(&stored_path, &scanned_path).unwrap_or(false)
        })
        .map(|(rel, (mount, stored, _))| (rel.as_str(), join_rel(mount, stored, "")))
        .collect();
    let mut files = Vec::with_capacity(scanned.files.len());
    for c in scanned.files {
        match clashing_dirs.get(c.dir_rel.as_str()) {
            Some(stored) => report.not_registered.push(FileIssue {
                reason: format!(
                    "登録済みのフォルダ {} と、大文字・小文字か Unicode の正規化だけが違う名前のフォルダにあり、カタログでは区別できないため登録しません",
                    stored.display()
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
    // 埋め込みのサムネイルがない写真（プレビューを埋め込まない DNG など）。L0 がないとグリッドに何も出ない
    // ので、`render_previews_after_import` によらず、現像結果から作る。
    let mut no_embedded: Vec<VariantId> = Vec::new();
    ctx.progress(0, total);
    for chunk in files.chunks(IMPORT_BATCH_FILES) {
        if ctx.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let prepared = parallel_map(chunk, threads, |c| {
            prepare(ctx, c, folders[&c.dir_rel], options)
        });
        if prepared.iter().any(|p| matches!(p, Prepared::Cancelled)) {
            report.cancelled = true;
            break;
        }
        // 登録（1 つのトランザクション）。
        let mut requests: Vec<RegisterFile> = Vec::new();
        let mut request_index: Vec<usize> = Vec::new();
        // 登録する新しいファイルの全体のハッシュ（重複の確認で求めたもの。登録の後に記録する）。
        let mut full_hashes: Vec<Option<String>> = Vec::new();
        let mut thumbs: Vec<ThumbTask> = Vec::new();
        let batch_duplicates = report.duplicates.len();
        for (i, p) in prepared.into_iter().enumerate() {
            match p {
                Prepared::Register {
                    request: r,
                    new,
                    check,
                } => {
                    let path = &chunk[i].path;
                    let mut own_hash = check.full_hash;
                    if new {
                        // 登録済みのファイルと同じ内容（取り込む設定）か、同じ取り込みで先に登録したファイルと
                        // 同じ内容か。
                        let duplicate = match check.duplicate_of {
                            Some(existing) => Some((existing.path, Some(existing.variant_id))),
                            None => match seen.find(path, &r.facts, &mut own_hash, ctx.token()) {
                                Ok(found) => found.map(|p| {
                                    let v = seen.variant_of(&p);
                                    (Some(p), v)
                                }),
                                Err(e) if e.is_cancelled() => {
                                    report.cancelled = true;
                                    break;
                                }
                                Err(e) => return Err(e),
                            },
                        };
                        if let Some((existing_path, existing_variant_id)) = duplicate {
                            report.duplicates.push(DuplicateFile {
                                path: path.clone(),
                                existing_path,
                                existing_variant_id,
                                skipped: options.skip_duplicates,
                            });
                            if options.skip_duplicates {
                                continue;
                            }
                        }
                        seen.add(path.clone(), r.facts.clone(), own_hash.clone());
                    }
                    if let Some(reason) = &r.error {
                        report.errors.push(FileIssue {
                            path: path.clone(),
                            reason: reason.clone(),
                        });
                    }
                    requests.push(*r);
                    request_index.push(i);
                    full_hashes.push(own_hash);
                }
                Prepared::Duplicate(existing) => report.duplicates.push(DuplicateFile {
                    path: chunk[i].path.clone(),
                    existing_path: existing.path,
                    existing_variant_id: Some(existing.variant_id),
                    skipped: true,
                }),
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
        if report.cancelled {
            break;
        }
        let outcomes = register_requests(
            inner,
            &mut requests,
            &register_options,
            tags,
            &full_hashes,
            &mut report,
        )?;
        let paths: Vec<&Path> = request_index
            .iter()
            .map(|&i| chunk[i].path.as_path())
            .collect();
        for (out, &i) in outcomes.iter().zip(&request_index) {
            seen.set_registered(
                chunk[i].path.clone(),
                out.master_variant_id,
                out.file_id,
                out.revision,
            );
        }
        let changed = tally_outcomes(
            inner,
            &requests,
            &outcomes,
            &paths,
            &mut report,
            &mut thumbs,
            &mut regenerate,
        )?;
        // 同じバッチで先に登録したファイルの重複は、登録の後に写真が分かる。
        for d in &mut report.duplicates[batch_duplicates..] {
            if d.existing_variant_id.is_none() {
                d.existing_variant_id = d.existing_path.as_deref().and_then(|p| seen.variant_of(p));
            }
        }
        // 同じ取り込みの中の比較で求めた、先に登録したファイルの全体のハッシュを記録する。
        let computed = seen.take_computed();
        if !computed.is_empty() {
            inner.with_catalog(|c| {
                for (file_id, revision, facts, hash) in &computed {
                    c.set_full_hash_if_current(*file_id, *revision, facts, hash)?;
                }
                Ok(())
            })?;
        }
        announce_changes(inner, changed);
        // サムネイル（L0。PRV-01）。
        make_thumbnails(ctx, threads, &thumbs, &mut report, &mut no_embedded);
        done += chunk.len() as u64;
        ctx.progress(done, total);
        if report.cancelled {
            break;
        }
    }
    after_import(inner, &report, regenerate, no_embedded);
    Ok(report)
}

/// 取り込みの終わりの処理: 現像結果からの L0 / L1 の作り直し（設定で止められる。埋め込みのサムネイルが
/// ない写真は設定によらず作る）と、内容のハッシュのバックグラウンドの計算の依頼（取り込み・コピー取り込みで
/// 共通。取り消されたときはしない）。
pub(crate) fn after_import(
    inner: &std::sync::Arc<crate::core::Inner>,
    report: &ImportReport,
    mut regenerate: Vec<VariantId>,
    no_embedded: Vec<VariantId>,
) {
    if !inner.config.render_previews_after_import {
        regenerate.clear();
    }
    if !report.cancelled {
        regenerate.extend(no_embedded);
    }
    if !report.cancelled && !regenerate.is_empty() {
        regenerate.sort();
        regenerate.dedup();
        spawn_regenerate(inner, regenerate);
    }
    if !report.cancelled {
        // 新しく登録したファイルの内容のハッシュ（ORG-03。アプリが許し、設定が有効なときだけ）。
        crate::identity::request_background_hashing(inner);
    }
}

/// 1 つのバッチを、1 つのトランザクションで登録し、新しく登録した写真に `tags` を付け（IMP-05）、求めた全体の
/// ハッシュ（`full_hashes`。要求と同じ順）を記録する（取り込み・コピー取り込みで共通）。`Err` を返すのは、
/// 登録をコミットする前の失敗だけ（何も登録していない）。ハッシュの記録の失敗は警告にする。
///
/// 既定のタイムゾーンで推定する撮影日時（utc_only の表示のオフセットを含む）は、登録の直前（カタログのロックの
/// 中）の設定で推定し直す（準備の後に設定が変わった場合、推定し直しのジョブの一覧より後に登録されたものが古い
/// オフセットのまま残らないように）。
pub(crate) fn register_requests(
    inner: &crate::core::Inner,
    requests: &mut [RegisterFile],
    options: &RegisterOptions,
    tags: &genzo_catalog::ImportTags,
    full_hashes: &[Option<String>],
    report: &mut ImportReport,
) -> Result<Vec<genzo_catalog::RegisterOutcome>, ApiError> {
    let (outcomes, tagged) = inner.with_catalog_api(|c| {
        let offset = inner.default_offset();
        for r in requests.iter_mut() {
            if r.capture.uses_default_offset() {
                r.capture = r
                    .capture
                    .with_default_offset(offset)
                    .map_err(|e| ApiError::Internal(e.to_string()))?;
            }
        }
        let (outcomes, tagged) = c.register_batch_tagged(requests, options, tags)?;
        // 求めた全体のハッシュを記録する（記録の内容が求めた時と同じときだけ）。登録はコミット済みなので、
        // 記録できなくても失敗にしない（警告だけ。全体のハッシュはバックグラウンドのジョブが後で求める）。
        // 失敗にすると、コピー取り込みが登録済みのコピーを消してしまう。
        for ((req, out), hash) in requests.iter().zip(&outcomes).zip(full_hashes) {
            if let Some(h) = hash
                && let Err(e) = c.set_full_hash_if_current(out.file_id, out.revision, &req.facts, h)
            {
                tracing::warn!(file_id = %out.file_id, error = %e, "取り込み: 全体のハッシュを記録できません");
            }
        }
        Ok((outcomes, tagged))
    })?;
    report.tagged += tagged.assets;
    Ok(outcomes)
}

/// 登録の結果を数え、サムネイル（L0）と現像結果からの作り直しの対象を集める。`paths` はファイルの場所（要求と
/// 同じ順）。一覧を作り直す写真（新しく登録した・変化を検知した）を返す。
pub(crate) fn tally_outcomes(
    inner: &crate::core::Inner,
    requests: &[RegisterFile],
    outcomes: &[genzo_catalog::RegisterOutcome],
    paths: &[&Path],
    report: &mut ImportReport,
    thumbs: &mut Vec<ThumbTask>,
    regenerate: &mut Vec<VariantId>,
) -> Result<Vec<VariantId>, ApiError> {
    let mut changed: Vec<VariantId> = Vec::new();
    for ((req, out), path) in requests.iter().zip(outcomes).zip(paths) {
        match out.status {
            RegisterStatus::Added => report.added += 1,
            RegisterStatus::Updated => report.updated += 1,
            RegisterStatus::Unchanged => report.unchanged += 1,
        }
        if out.paired {
            report.paired += 1;
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
                    path: path.to_path_buf(),
                    kind: req.kind,
                });
            }
            if req.kind == AssetKind::Photo && out.status != RegisterStatus::Unchanged {
                regenerate.push(out.master_variant_id);
            }
        }
    }
    Ok(changed)
}

/// 一覧を作り直す写真を知らせ、表示中の検索結果を作り直す。
pub(crate) fn announce_changes(inner: &crate::core::Inner, mut changed: Vec<VariantId>) {
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
}

/// サムネイル（L0。PRV-01）を作る。埋め込みのサムネイルがない写真は `no_embedded` に入れる（現像結果から作る）。
pub(crate) fn make_thumbnails(
    ctx: &JobCtx<'_>,
    threads: usize,
    thumbs: &[ThumbTask],
    report: &mut ImportReport,
    no_embedded: &mut Vec<VariantId>,
) {
    let results = parallel_map(thumbs, threads, |t| make_source_thumbnail(ctx, t));
    for (t, r) in thumbs.iter().zip(results) {
        match r {
            Ok(()) => report.thumbnails += 1,
            Err(e) if e.is_cancelled() => report.cancelled = true,
            Err(ApiError::Worker(WorkerClientError::Job(job)))
                if t.kind == AssetKind::Photo && job.kind == WorkerErrorKind::Unsupported =>
            {
                no_embedded.push(t.variant);
            }
            Err(e) => report.thumbnail_failures.push(FileIssue {
                path: t.path.clone(),
                reason: e.to_string(),
            }),
        }
    }
}

/// ワーカーで B5 のサムネイルを作り、L0 として保存する（PRV-01）。
pub(crate) fn make_source_thumbnail(ctx: &JobCtx<'_>, t: &ThumbTask) -> Result<(), ApiError> {
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
    /// フォルダを取り込む（IMP-01・VID-01）。P3 のジョブとして実行し、ジョブの ID を返す。重複と思われる
    /// もの・RAW と JPEG の扱いは、カタログの設定に従う（[`Core::import_folder_with`]）。
    ///
    /// 結果（[`ImportReport`]）は [`Core::wait_job`]・[`Core::job`] で取れる。
    pub fn import_folder(&self, path: &Path, recursive: bool) -> Result<u64, ApiError> {
        self.import_folder_with(path, recursive, &ImportOptions::default())
    }

    /// 設定 `options` でフォルダを取り込む（IMP-01〜IMP-06）。`skip_duplicates`・`separate_raw_jpeg` の
    /// `None` は、呼んだ時点のカタログの設定（[`crate::CoreSettings`] の `import_skip_duplicates`・
    /// `import_separate_raw_jpeg`）に従う。
    ///
    /// `options.copy` があればコピー取り込み（IMP-02・IMP-03。`copy_import` の doc）。`options.metadata` の
    /// キーワード・説明のメタデータは、新しく登録した写真に登録と同じトランザクションで付ける（IMP-05）。
    /// 設定の誤り（テンプレート・キーワードの名前・メタデータの値・コピー先）は、ジョブを始める前に
    /// [`ApiError::InvalidArgument`] などで返す。
    pub fn import_folder_with(
        &self,
        path: &Path,
        recursive: bool,
        options: &ImportOptions,
    ) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        // 写真の移動（FILE-02）の実行中は取り込まない（移している途中のファイルを登録し直さないため）。
        if inner.jobs.is_running(JobKind::MoveFiles, None) {
            return Err(ApiError::InvalidArgument(
                "写真の移動の実行中は取り込めません。移動が終わってからやり直してください"
                    .to_owned(),
            ));
        }
        let resolved = {
            let s = inner.settings.lock();
            ResolvedOptions {
                skip_duplicates: options.skip_duplicates.unwrap_or(s.import_skip_duplicates),
                separate_raw_jpeg: options
                    .separate_raw_jpeg
                    .unwrap_or(s.import_separate_raw_jpeg),
            }
        };
        let tags = options.metadata.to_tags();
        tags.validate().map_err(|e| match e {
            genzo_catalog::CatalogError::InvalidInput(m) => {
                ApiError::InvalidArgument(format!("取り込みで付けるもの: {m}"))
            }
            other => ApiError::Catalog(other),
        })?;
        let root = path.to_path_buf();
        let Some(copy) = options.copy.clone() else {
            if options.dry_run {
                return Err(ApiError::InvalidArgument(
                    "名前を決めるだけの確認（dry_run）は、コピー取り込みでだけ使えます".to_owned(),
                ));
            }
            return Ok(spawn_job(
                inner,
                JobKind::Import,
                "取り込み",
                move |ctx| {
                    run_import(ctx, &root, recursive, resolved, &tags).map(JobResult::Import)
                },
            ));
        };
        let plan = crate::copy_import::CopyPlan::new(inner, &root, copy, options.dry_run)?;
        Ok(spawn_job(
            inner,
            JobKind::Import,
            "コピー取り込み",
            move |ctx| {
                crate::copy_import::run_copy_import(ctx, &root, recursive, resolved, &tags, &plan)
                    .map(JobResult::Import)
            },
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
