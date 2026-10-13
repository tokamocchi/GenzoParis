//! ファイルの内容による同一性（ORG-03）と、取り込みでの重複の検出（IMP-04）。04 の 3.3 節・3.11 節。
//!
//! - **内容のハッシュ**: ファイル全体の BLAKE3（`file.full_hash`）。クイックハッシュ（大きさ ＋ 先頭と末尾の
//!   64KB）は候補を絞るだけに使い、同じ内容かは全体のハッシュで確かめる。記録は genzo-catalog の
//!   `Catalog::set_full_hash_if_current`（記録の内容が計算した時と同じで、まだ記録していないときだけ）。
//! - **バックグラウンドの計算**（[`JobKind::HashFiles`]）: 全体のハッシュのないファイルを id の順に読み、
//!   記録する。専用の 1 本のスレッド（`Inner::maintenance`。書き出しなどの P3 のスレッドを占めない）で、
//!   現像のプレビューなどの対話の処理（P0）が動いている間は、ファイルごとに最大
//!   [`INTERACTIVE_MAX_WAIT`] 待ってから読む（優先度を低くする）。取り消せる（読んでいる途中のファイルも
//!   1 MiB ごとに確かめてやめる）。**ネットワークのボリューム**（SMB・NFS など。`paths::is_network_path`）
//!   のファイルは、設定（`CoreSettings::hash_network_volumes`）で許したときだけ読む（既定は読まない。
//!   通信量と時間がかかるため）。フォルダが見えない（ドライブが外れている）ファイルは飛ばす。
//!   - 自動で動かすのは、アプリが起動の設定（[`crate::CoreConfig::background_hashing`]）で許し、カタログの
//!     設定（`CoreSettings::hash_in_background`）が有効なときだけ: カタログを開いたとき（版 10 より前から
//!     移行したカタログの既存のファイルを含む）と、取り込みの後。実行中に頼まれたら、終わる前にもう一度
//!     対象を探す（[`HashJobState`]）。1 つのジョブで、同じファイルは 1 回だけ試す。
//!   - 明示的な計算（[`Core::compute_content_hashes`]。CLI の `genzo hash compute`、画面の「今すぐ計算」）は、
//!     起動の設定・カタログの設定によらず動く。
//! - **取り込みの重複**（[`check_catalog_duplicate`]・[`ImportSeen`]）: 新しく登録するファイル（同じ場所に
//!   登録がないもの）について、クイックハッシュが同じで見つからない状態でない登録済みのファイルを候補にし、
//!   候補があれば新しいファイルと候補の全体のハッシュを求めて（候補の値は記録する）比べる。同じ取り込みの
//!   中で先に登録した（する）ファイルとも比べる。見つからない（`missing`）ファイルは候補にしない（アプリの
//!   外で移されたファイルの再リンク（FILE-04）の対象のため）。

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use genzo_catalog::{Catalog, FileFacts, FileLocation, FileRecord};
use genzo_jobs::CancellationToken;
use genzo_model::{FileId, FileStatus, FolderId, VariantId};

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::jobs::{JobCtx, JobQueue, spawn_job_on};
use crate::paths::{is_network_path, join_rel};
use crate::types::{
    ContentFile, ContentHashStatus, DuplicateGroup, DuplicateGroupList, FileIdentity, FileIssue,
    HashReport, JobKind, JobResult,
};

/// 内容のハッシュの計算の結果に入れる、読めなかったファイルの上限（数は `failed_count` にすべて数える）。
pub const HASH_FAILURES_KEPT: usize = 200;

/// 重複の組の一覧（[`Core::duplicate_groups`]）で一度に返す組の上限。
pub const DUPLICATE_GROUPS_MAX: usize = 1000;

/// バックグラウンドの計算で、カタログから一度に読むファイルの数。
const HASH_PAGE: usize = 256;

/// 全体のハッシュを求めるときに一度に読む長さ（この大きさごとに取り消しを確かめる）。
const HASH_READ_BUFFER: usize = 1024 * 1024;

/// 対話の処理（P0）が動いている間、バックグラウンドの計算がファイルごとに待つ時間の上限（**仮置き**: 2 秒。
/// 対話の処理が続いても、計算が止まったままにならないように）。
pub const INTERACTIVE_MAX_WAIT: Duration = Duration::from_secs(2);

/// 対話の処理が終わったかを確かめる間隔。
const INTERACTIVE_POLL: Duration = Duration::from_millis(50);

/// 内容のハッシュの計算のジョブの状態（`Inner::hash`）。
#[derive(Debug, Default)]
pub(crate) struct HashJobState {
    /// 実行中（または待っている）ジョブ。
    job: Option<u64>,
    /// 実行中に、もう一度対象を探すよう頼まれたか（取り込みの後など）。
    requested: bool,
    /// ネットワークのボリュームのファイルも読むか（実行中に頼まれたら広げる）。
    include_network: bool,
}

/// 全体のハッシュを求められなかった理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HashFailure {
    /// 取り消された。
    Cancelled,
    /// 読めない（理由）。
    Io(String),
}

/// ファイル全体の BLAKE3（genzo-catalog の `full_hash` と同じ値）を、[`HASH_READ_BUFFER`] ごとに取り消しを
/// 確かめながら求める。ファイルは読み取り専用で開く（DATA-01）。
pub(crate) fn full_hash_cancellable(
    path: &Path,
    token: &CancellationToken,
) -> Result<String, HashFailure> {
    let io = |e: std::io::Error| HashFailure::Io(format!("ファイルを読めません: {e}"));
    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; HASH_READ_BUFFER];
    loop {
        if token.is_cancelled() {
            return Err(HashFailure::Cancelled);
        }
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io(e)),
        };
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// ファイルの内容が記録 `expected`（大きさ・更新日時・クイックハッシュ）と同じことを、読む前と後で確かめて
/// から全体のハッシュを返す（読む前か後で違えば `Ok(None)`。計算の間に書き換わった内容のハッシュを、記録の
/// 内容のものとして扱わないため）。
pub(crate) fn verified_full_hash(
    path: &Path,
    expected: &FileFacts,
    token: &CancellationToken,
) -> Result<Option<String>, HashFailure> {
    let facts = |path: &Path| {
        FileFacts::read(path).map_err(|e| HashFailure::Io(format!("ファイルを読めません: {e}")))
    };
    if facts(path)? != *expected {
        return Ok(None);
    }
    let hash = full_hash_cancellable(path, token)?;
    if facts(path)? != *expected {
        return Ok(None);
    }
    Ok(Some(hash))
}

/// 登録済みのファイルの情報（[`ContentFile`]）を作る。
fn content_file(c: &Catalog, rec: &FileRecord) -> genzo_catalog::Result<ContentFile> {
    Ok(ContentFile {
        file_id: rec.id,
        asset_id: rec.asset_id,
        variant_id: c.master_variant_of(rec.asset_id)?,
        role: rec.role,
        path: c.file_location(rec.id)?.absolute_path(),
        size: rec.facts.size,
        status: rec.status,
    })
}

// ---------------------------------------------------------------------------
// 取り込みの重複（IMP-04）
// ---------------------------------------------------------------------------

/// 取り込むファイルと同じ内容の、登録済みのファイル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExistingCopy {
    /// ファイルの場所（分からなければ `None`）。
    pub path: Option<PathBuf>,
    /// その写真（マスターの variant）。
    pub variant_id: VariantId,
}

/// 登録済みのファイルとの比べた結果（[`check_catalog_duplicate`]）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ContentCheck {
    /// 新しいファイルの全体のハッシュ（候補があって求めたもの。読めない・途中で変わったときは `None`）。
    pub full_hash: Option<String>,
    /// 同じ内容の登録済みのファイル。
    pub duplicate_of: Option<ExistingCopy>,
}

/// 取り込む新しいファイル（`path`、内容は `facts`）と同じ内容のファイルが、カタログに登録済みかを確かめる。
///
/// 候補は、クイックハッシュ（大きさを含む）が同じで、見つからない（`missing`）状態でないファイル。候補が
/// なければファイルを読まない。候補があれば、新しいファイルの全体のハッシュを求め、候補の全体のハッシュ
/// （記録になければ候補のファイルを読んで求め、記録する）と比べる。新しいファイルを読めない・途中で
/// 変わった場合と、候補を読めない場合は、同じとはみなさない（取り込む）。取り消されたら
/// [`ApiError::Cancelled`]。
pub(crate) fn check_catalog_duplicate(
    inner: &Inner,
    path: &Path,
    facts: &FileFacts,
    token: &CancellationToken,
) -> Result<ContentCheck, ApiError> {
    let candidates: Vec<(FileRecord, Option<PathBuf>, VariantId)> = inner.with_catalog(|c| {
        let mut out = Vec::new();
        for rec in c.files_with_quick_hash(&facts.quick_hash)? {
            if rec.status == FileStatus::Missing || rec.facts.size != facts.size {
                continue;
            }
            let p = c.file_location(rec.id)?.absolute_path();
            let v = c.master_variant_of(rec.asset_id)?;
            out.push((rec, p, v));
        }
        Ok(out)
    })?;
    if candidates.is_empty() {
        return Ok(ContentCheck::default());
    }
    let own = match verified_full_hash(path, facts, token) {
        Ok(Some(h)) => h,
        Ok(None) | Err(HashFailure::Io(_)) => return Ok(ContentCheck::default()),
        Err(HashFailure::Cancelled) => return Err(ApiError::Cancelled),
    };
    for (rec, cand_path, variant_id) in candidates {
        let theirs = match &rec.full_hash {
            Some(h) => h.clone(),
            None => {
                let Some(p) = cand_path.as_deref() else {
                    continue;
                };
                match verified_full_hash(p, &rec.facts, token) {
                    Ok(Some(h)) => {
                        // 候補の内容のハッシュも記録しておく（次からは読まない）。記録の内容が変わっていれば
                        // 記録しない（set_full_hash_if_current）。
                        let _ = inner.with_catalog(|c| {
                            c.set_full_hash_if_current(rec.id, rec.revision, &rec.facts, &h)
                        });
                        h
                    }
                    Ok(None) | Err(HashFailure::Io(_)) => continue,
                    Err(HashFailure::Cancelled) => return Err(ApiError::Cancelled),
                }
            }
        };
        if theirs == own {
            return Ok(ContentCheck {
                full_hash: Some(own),
                duplicate_of: Some(ExistingCopy {
                    path: cand_path,
                    variant_id,
                }),
            });
        }
    }
    Ok(ContentCheck {
        full_hash: Some(own),
        duplicate_of: None,
    })
}

/// 同じ取り込みの中で、先に登録した（する）ファイル（同じ内容のファイルが同じ取り込みに 2 つある場合の重複を
/// 見つけるため。登録済みのファイルとの比較は [`check_catalog_duplicate`]）。
#[derive(Debug, Default)]
pub(crate) struct ImportSeen {
    by_quick: HashMap<String, Vec<SeenFile>>,
    /// 登録したファイルの写真（マスターの variant）・ファイル・リビジョン。
    registered: HashMap<PathBuf, (VariantId, FileId, u32)>,
    /// 比べるために求めた、先に覚えたファイルの全体のハッシュ（登録の後に記録する。[`ImportSeen::take_computed`]）。
    computed: Vec<(PathBuf, FileFacts, String)>,
}

#[derive(Debug)]
struct SeenFile {
    path: PathBuf,
    facts: FileFacts,
    /// 全体のハッシュ（`None` はまだ求めていない。`Some(None)` は読めなかった・途中で変わった）。
    full_hash: Option<Option<String>>,
}

impl ImportSeen {
    /// 新しいファイル（`path`、内容は `facts`。全体のハッシュを求めていれば `own_hash`）と同じ内容のファイルが、
    /// 先に覚えたファイルにあれば、その場所を返す。比べるために求めた全体のハッシュは `own_hash` に入れる。
    pub(crate) fn find(
        &mut self,
        path: &Path,
        facts: &FileFacts,
        own_hash: &mut Option<String>,
        token: &CancellationToken,
    ) -> Result<Option<PathBuf>, ApiError> {
        let Some(list) = self.by_quick.get_mut(&facts.quick_hash) else {
            return Ok(None);
        };
        let list: Vec<&mut SeenFile> = list
            .iter_mut()
            .filter(|s| s.facts.size == facts.size && s.path != path)
            .collect();
        if list.is_empty() {
            return Ok(None);
        }
        if own_hash.is_none() {
            match verified_full_hash(path, facts, token) {
                Ok(Some(h)) => *own_hash = Some(h),
                Ok(None) | Err(HashFailure::Io(_)) => return Ok(None),
                Err(HashFailure::Cancelled) => return Err(ApiError::Cancelled),
            }
        }
        let own = own_hash.as_deref().expect("上で求めた");
        for seen in list {
            if seen.full_hash.is_none() {
                let h = match verified_full_hash(&seen.path, &seen.facts, token) {
                    Ok(h) => h,
                    Err(HashFailure::Io(_)) => None,
                    Err(HashFailure::Cancelled) => return Err(ApiError::Cancelled),
                };
                if let Some(h) = &h {
                    self.computed
                        .push((seen.path.clone(), seen.facts.clone(), h.clone()));
                }
                seen.full_hash = Some(h);
            }
            if seen.full_hash.as_ref().and_then(Option::as_deref) == Some(own) {
                return Ok(Some(seen.path.clone()));
            }
        }
        Ok(None)
    }

    /// 登録する（した）ファイルを覚える。
    pub(crate) fn add(&mut self, path: PathBuf, facts: FileFacts, full_hash: Option<String>) {
        self.by_quick
            .entry(facts.quick_hash.clone())
            .or_default()
            .push(SeenFile {
                path,
                facts,
                full_hash: full_hash.map(Some),
            });
    }

    /// 登録したファイルの写真（マスターの variant）・ファイル・リビジョンを覚える。
    pub(crate) fn set_registered(
        &mut self,
        path: PathBuf,
        variant: VariantId,
        file_id: FileId,
        revision: u32,
    ) {
        self.registered.insert(path, (variant, file_id, revision));
    }

    /// 覚えたファイルの写真。
    pub(crate) fn variant_of(&self, path: &Path) -> Option<VariantId> {
        self.registered.get(path).map(|r| r.0)
    }

    /// 比べるために求めた、登録したファイルの全体のハッシュ（ファイル・リビジョン・求めた時の内容・値）を
    /// 取り出す（記録は呼び出し側が `set_full_hash_if_current` で行う）。まだ登録していないファイルのものは
    /// 残す。
    pub(crate) fn take_computed(&mut self) -> Vec<(FileId, u32, FileFacts, String)> {
        let mut out = Vec::new();
        let mut rest = Vec::new();
        for (path, facts, hash) in self.computed.drain(..) {
            match self.registered.get(&path) {
                Some(&(_, file_id, revision)) => out.push((file_id, revision, facts, hash)),
                None => rest.push((path, facts, hash)),
            }
        }
        self.computed = rest;
        out
    }
}

// ---------------------------------------------------------------------------
// バックグラウンドの計算（ORG-03）
// ---------------------------------------------------------------------------

/// 起動の設定とカタログの設定が許していれば、内容のハッシュのバックグラウンドの計算を頼む（実行中なら、
/// 終わる前にもう一度対象を探す）。頼んだジョブを返す。
pub(crate) fn request_background_hashing(inner: &Arc<Inner>) -> Option<u64> {
    if !inner.config.background_hashing {
        return None;
    }
    let (enabled, include_network) = {
        let s = inner.settings.lock();
        (s.hash_in_background, s.hash_network_volumes)
    };
    enabled.then(|| request_hashing(inner, include_network))
}

/// 内容のハッシュの計算を頼む（実行中なら、終わる前にもう一度対象を探し、`include_network` なら
/// ネットワークのボリュームも読むよう広げる）。ジョブを返す。
pub(crate) fn request_hashing(inner: &Arc<Inner>, include_network: bool) -> u64 {
    let mut st = inner.hash.lock();
    if let Some(id) = st.job
        && inner
            .jobs
            .info(id)
            .is_some_and(|info| !info.state.is_finished())
    {
        st.requested = true;
        st.include_network |= include_network;
        return id;
    }
    st.requested = false;
    st.include_network = include_network;
    let id = spawn_job_on(
        inner,
        JobQueue::Maintenance,
        JobKind::HashFiles,
        "内容のハッシュの計算",
        |ctx| {
            let result = run_hash_job(ctx);
            // 失敗・取り消しで終わったときも、このジョブの記録を外す（次の依頼で新しく始める）。
            let mut st = ctx.inner.hash.lock();
            if st.job == Some(ctx.id) {
                st.job = None;
                st.requested = false;
            }
            drop(st);
            result.map(JobResult::HashFiles)
        },
    );
    st.job = Some(id);
    id
}

/// 実行中の内容のハッシュの計算を取り消す（バックグラウンドの計算を止める設定にしたとき）。
pub(crate) fn cancel_hashing(inner: &Arc<Inner>) {
    let job = inner.hash.lock().job;
    if let Some(id) = job {
        inner.jobs.cancel(id);
        inner.jobs.reap_unrun(inner);
    }
}

/// フォルダの読める・読めないの区別（バックグラウンドの計算でフォルダごとに覚える）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FolderAccess {
    /// ローカルのボリューム。
    Local,
    /// ネットワークのボリューム。
    Network,
    /// 見えない（ボリュームのマウント先が分からない・フォルダがない）。
    Unavailable,
}

fn folder_access(loc: &FileLocation) -> FolderAccess {
    let Some(mount) = loc.mount_path.as_deref() else {
        return FolderAccess::Unavailable;
    };
    let dir = join_rel(Path::new(mount), &loc.rel_dir, "");
    if !dir.is_dir() {
        FolderAccess::Unavailable
    } else if is_network_path(&dir) {
        FolderAccess::Network
    } else {
        FolderAccess::Local
    }
}

/// 対話の処理（現像のプレビューなど。P0）が動いている間、最大 [`INTERACTIVE_MAX_WAIT`] 待つ。
fn wait_for_interactive(inner: &Inner, token: &CancellationToken) {
    let start = Instant::now();
    while !token.is_cancelled()
        && start.elapsed() < INTERACTIVE_MAX_WAIT
        && inner.interactive.running_count() + inner.interactive.queued_count() > 0
    {
        std::thread::sleep(INTERACTIVE_POLL);
    }
}

/// バックグラウンドの計算で、このジョブが扱ったファイル（もう一度対象を探すときに、同じファイルを試し直さない・
/// 数え直さない）。
#[derive(Debug, Default)]
struct HashVisits {
    /// 試した（計算した・見えないものとして数えた）ファイル。
    tried: HashSet<FileId>,
    /// ネットワークのボリュームのため飛ばしたファイル。試したことにしない（実行中にネットワークのボリュームも
    /// 含めるよう広げられたら、探し直しで計算する）。
    skipped_network: HashSet<FileId>,
}

/// [`HashVisits::visit`] の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    /// 前に扱った（何もしない）。
    Seen,
    /// ネットワークのボリュームのため初めて飛ばした（進捗に数える）。
    SkippedNetwork,
    /// 計算する（見えないフォルダなら、見えないものとして数える）。`counted` は、前にネットワークのため
    /// 飛ばして進捗に数えてあるか。
    Process { counted: bool },
}

impl HashVisits {
    fn is_tried(&self, id: FileId) -> bool {
        self.tried.contains(&id)
    }

    /// ファイル `id`（フォルダの区別 `access`）をどう扱うかを決め、飛ばした数を `report` に数える。
    fn visit(
        &mut self,
        id: FileId,
        access: FolderAccess,
        include_network: bool,
        report: &mut HashReport,
    ) -> Visit {
        if self.tried.contains(&id) {
            return Visit::Seen;
        }
        if access == FolderAccess::Network && !include_network {
            if self.skipped_network.insert(id) {
                report.skipped_network += 1;
                return Visit::SkippedNetwork;
            }
            return Visit::Seen;
        }
        self.tried.insert(id);
        let counted = self.skipped_network.remove(&id);
        if counted {
            // 飛ばした数から外す（ここで計算する）。
            report.skipped_network -= 1;
        }
        Visit::Process { counted }
    }
}

/// バックグラウンドの計算の本体。
fn run_hash_job(ctx: &JobCtx<'_>) -> Result<HashReport, ApiError> {
    let inner = ctx.inner;
    let mut report = HashReport::default();
    let mut folders: HashMap<FolderId, FolderAccess> = HashMap::new();
    let mut visits = HashVisits::default();
    let mut done = 0u64;
    let mut total = inner.with_catalog(|c| c.content_hash_counts())?.pending;
    ctx.progress(0, total);
    let mut after: Option<FileId> = None;
    loop {
        if ctx.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let include_network = inner.hash.lock().include_network;
        let page: Vec<(FileRecord, FileLocation)> = inner.with_catalog(|c| {
            let mut out = Vec::new();
            for rec in c.files_without_full_hash(after, HASH_PAGE)? {
                let loc = c.file_location(rec.id)?;
                out.push((rec, loc));
            }
            Ok(out)
        })?;
        let Some((last, _)) = page.last() else {
            // 終わり。実行中に頼まれていれば（取り込みの後など）、最初からもう一度対象を探す（試したものは
            // 飛ばす。ネットワークのため飛ばしたものは、ネットワークも含めるよう広げられていれば計算する）。頼まれていなければ、同じロックの中でジョブの記録を外す（外した後の依頼は、新しい
            // ジョブを始める）。
            let mut st = inner.hash.lock();
            if st.requested {
                st.requested = false;
                drop(st);
                after = None;
                folders.clear();
                // 試したファイルは `done` に数えてあり、そのうち記録できなかったものはまだ対象に数えられる
                // （飛ばすので、進捗の全体からは除く）。
                let pending = inner.with_catalog(|c| c.content_hash_counts())?.pending;
                total = pending + report.hashed;
                continue;
            }
            if st.job == Some(ctx.id) {
                st.job = None;
            }
            break;
        };
        after = Some(last.id);
        for (rec, loc) in page {
            if visits.is_tried(rec.id) {
                continue;
            }
            if ctx.is_cancelled() {
                report.cancelled = true;
                break;
            }
            let access = *folders
                .entry(loc.folder_id)
                .or_insert_with(|| folder_access(&loc));
            match visits.visit(rec.id, access, include_network, &mut report) {
                Visit::Seen => continue,
                Visit::SkippedNetwork => done += 1,
                Visit::Process { counted } => {
                    if access == FolderAccess::Unavailable {
                        report.skipped_unavailable += 1;
                    } else {
                        hash_one(ctx, &rec, &loc, &mut report)?;
                    }
                    if !counted {
                        done += 1;
                    }
                }
            }
            ctx.progress(done, total.max(done));
        }
        if report.cancelled {
            break;
        }
    }
    Ok(report)
}

/// 1 つのファイルの全体のハッシュを求めて記録する。
fn hash_one(
    ctx: &JobCtx<'_>,
    rec: &FileRecord,
    loc: &FileLocation,
    report: &mut HashReport,
) -> Result<(), ApiError> {
    let Some(path) = loc.absolute_path() else {
        report.skipped_unavailable += 1;
        return Ok(());
    };
    wait_for_interactive(ctx.inner, ctx.token());
    match verified_full_hash(&path, &rec.facts, ctx.token()) {
        Ok(Some(h)) => {
            let stored = ctx
                .inner
                .with_catalog(|c| c.set_full_hash_if_current(rec.id, rec.revision, &rec.facts, &h));
            match stored {
                Ok(true) => report.hashed += 1,
                // 計算の間に登録し直された・カタログから除かれた。
                Ok(false) | Err(ApiError::Catalog(genzo_catalog::CatalogError::NotFound(_))) => {
                    report.changed += 1;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(None) => report.changed += 1,
        Err(HashFailure::Cancelled) => report.cancelled = true,
        Err(HashFailure::Io(reason)) => {
            if std::fs::symlink_metadata(&path)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                // 見つからない（アプリの外で移された・消された）。ファイルの確認で `missing` になる。
                report.skipped_unavailable += 1;
            } else {
                report.failed_count += 1;
                if report.failed.len() < HASH_FAILURES_KEPT {
                    report.failed.push(FileIssue { path, reason });
                }
            }
        }
    }
    Ok(())
}

impl Core {
    /// 全体のハッシュのないファイルの内容のハッシュを計算する（ORG-03。専用のスレッドのジョブ。起動の設定・
    /// カタログの設定によらず動く）。`include_network` なら、ネットワークのボリュームのファイルも読む。
    /// 既に計算中なら、そのジョブ（終わる前にもう一度対象を探す）を返す。
    ///
    /// 結果（[`HashReport`]）は [`Core::wait_job`]・[`Core::job`] で取れる。
    pub fn compute_content_hashes(&self, include_network: bool) -> Result<u64, ApiError> {
        self.inner.check_open()?;
        Ok(request_hashing(&self.inner, include_network))
    }

    /// 内容のハッシュの状況（計算済みの数・重複の組の数・実行中のジョブ）。
    pub fn content_hash_status(&self) -> Result<ContentHashStatus, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let (counts, groups) =
            inner.with_catalog(|c| Ok((c.content_hash_counts()?, c.duplicate_groups(0)?.total)))?;
        let job = inner.hash.lock().job.filter(|&id| {
            inner
                .jobs
                .info(id)
                .is_some_and(|info| !info.state.is_finished())
        });
        Ok(ContentHashStatus {
            files: counts.files,
            hashed: counts.hashed,
            pending: counts.pending,
            duplicate_groups: groups,
            job_id: job,
        })
    }

    /// 内容（全体のハッシュ）が同じファイルの組を、最初に登録したファイルの順に `limit` 組まで返す
    /// （上限は [`DUPLICATE_GROUPS_MAX`]。全体のハッシュを記録したファイルだけが対象）。
    pub fn duplicate_groups(&self, limit: usize) -> Result<DuplicateGroupList, ApiError> {
        self.inner.check_open()?;
        let limit = limit.min(DUPLICATE_GROUPS_MAX);
        self.inner.with_catalog(|c| {
            let list = c.duplicate_groups(limit)?;
            let mut groups = Vec::with_capacity(list.groups.len());
            for g in list.groups {
                let files = g
                    .files
                    .iter()
                    .map(|f| content_file(c, f))
                    .collect::<genzo_catalog::Result<Vec<_>>>()?;
                groups.push(DuplicateGroup {
                    full_hash: g.full_hash,
                    files,
                });
            }
            Ok(DuplicateGroupList {
                total: list.total,
                groups,
            })
        })
    }

    /// ファイル（カタログの外のものでもよい）の内容のハッシュを求め、同じ内容の登録済みのファイルを探す
    /// （ORG-03。場所・名前によらない。アプリの外で移された・名前を変えられたファイルの写真を探せる）。
    ///
    /// 全体のハッシュを記録したファイルと、クイックハッシュが同じで全体のハッシュのないファイル（読んで求め、
    /// 記録する）を比べる。ファイルは読み取り専用で開く。
    pub fn identify_file(&self, path: &Path) -> Result<FileIdentity, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let path = crate::paths::absolute_lexical(path)?;
        let facts = FileFacts::read(&path)?;
        let token = CancellationToken::new();
        let full_hash = match verified_full_hash(&path, &facts, &token) {
            Ok(Some(h)) => h,
            Ok(None) => {
                return Err(ApiError::FileAccess {
                    path,
                    message: "読んでいる間にファイルが変わりました".to_owned(),
                });
            }
            Err(HashFailure::Io(message)) => return Err(ApiError::FileAccess { path, message }),
            Err(HashFailure::Cancelled) => return Err(ApiError::Cancelled),
        };
        // 全体のハッシュのない候補（クイックハッシュが同じもの）を読んで記録してから、全体のハッシュで探す。
        let unhashed: Vec<(FileRecord, Option<PathBuf>)> = inner.with_catalog(|c| {
            let mut out = Vec::new();
            for rec in c.files_with_quick_hash(&facts.quick_hash)? {
                if rec.full_hash.is_none() && rec.status != FileStatus::Missing {
                    let p = c.file_location(rec.id)?.absolute_path();
                    out.push((rec, p));
                }
            }
            Ok(out)
        })?;
        for (rec, p) in unhashed {
            if let Some(p) = p
                && let Ok(Some(h)) = verified_full_hash(&p, &rec.facts, &token)
            {
                inner.with_catalog(|c| {
                    c.set_full_hash_if_current(rec.id, rec.revision, &rec.facts, &h)
                })?;
            }
        }
        let matches = inner.with_catalog(|c| {
            c.files_with_full_hash(&full_hash)?
                .iter()
                .map(|f| content_file(c, f))
                .collect::<genzo_catalog::Result<Vec<_>>>()
        })?;
        Ok(FileIdentity {
            path,
            size: facts.size,
            full_hash,
            matches,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::File::create(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }

    /// ネットワークのボリュームのため飛ばしたファイルは、試したことにしない: 探し直しで、ネットワークも含める
    /// よう広げられていれば計算し（飛ばした数から外す）、広げられていなければ数え直さない。計算した・見えない
    /// ファイルは、探し直しでもう一度扱わない。
    #[test]
    fn network_skips_are_retried_when_the_request_widens() {
        let mut v = HashVisits::default();
        let mut report = HashReport::default();
        let (net, local, gone) = (FileId::new(1), FileId::new(2), FileId::new(3));
        // 最初の巡回（ネットワークを含めない）。
        assert_eq!(
            v.visit(net, FolderAccess::Network, false, &mut report),
            Visit::SkippedNetwork
        );
        assert_eq!(
            v.visit(local, FolderAccess::Local, false, &mut report),
            Visit::Process { counted: false }
        );
        assert_eq!(
            v.visit(gone, FolderAccess::Unavailable, false, &mut report),
            Visit::Process { counted: false }
        );
        assert_eq!(report.skipped_network, 1);
        // 探し直し（まだ含めない）: 数え直さない。
        assert_eq!(
            v.visit(net, FolderAccess::Network, false, &mut report),
            Visit::Seen
        );
        assert_eq!(report.skipped_network, 1);
        assert!(v.is_tried(local) && v.is_tried(gone) && !v.is_tried(net));
        // 探し直し（含めるよう広げられた）: 計算する。進捗には数えてある。
        assert_eq!(
            v.visit(net, FolderAccess::Network, true, &mut report),
            Visit::Process { counted: true }
        );
        assert_eq!(report.skipped_network, 0);
        for id in [net, local, gone] {
            assert_eq!(
                v.visit(id, FolderAccess::Local, true, &mut report),
                Visit::Seen
            );
        }
        // 初めて見るネットワークのファイル（含める設定）は、そのまま計算する。
        assert_eq!(
            v.visit(FileId::new(4), FolderAccess::Network, true, &mut report),
            Visit::Process { counted: false }
        );
    }

    /// 取り消しを確かめながら求めた値は、genzo-catalog の全体のハッシュ（`b3sum` と同じ値）と同じ。
    /// 取り消されていればやめる。
    #[test]
    fn cancellable_full_hash_matches_the_catalog_hash() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.bin");
        let bytes: Vec<u8> = (0..(HASH_READ_BUFFER * 2 + 17))
            .map(|i| (i % 251) as u8)
            .collect();
        write(&p, &bytes);
        let token = CancellationToken::new();
        assert_eq!(
            full_hash_cancellable(&p, &token).unwrap(),
            genzo_catalog::full_hash(&p).unwrap()
        );
        token.cancel();
        assert_eq!(
            full_hash_cancellable(&p, &token),
            Err(HashFailure::Cancelled)
        );
        assert!(matches!(
            full_hash_cancellable(&d.path().join("ない"), &CancellationToken::new()),
            Err(HashFailure::Io(_))
        ));
    }

    /// 記録の内容（大きさ・更新日時・クイックハッシュ）と違うファイルのハッシュは返さない。
    #[test]
    fn verified_hash_requires_the_recorded_content() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.bin");
        write(&p, b"first");
        let facts = FileFacts::read(&p).unwrap();
        let token = CancellationToken::new();
        assert_eq!(
            verified_full_hash(&p, &facts, &token).unwrap().as_deref(),
            Some(genzo_catalog::full_hash(&p).unwrap().as_str())
        );
        write(&p, b"second, longer");
        assert_eq!(verified_full_hash(&p, &facts, &token).unwrap(), None);
    }

    /// 同じ取り込みの中で先に覚えたファイルと、内容が同じものを見つける（クイックハッシュが同じでも、
    /// 内容が違うものは見つけない）。
    #[test]
    fn files_of_the_same_import_are_compared_by_content() {
        let d = tempfile::tempdir().unwrap();
        let (a, b, c) = (d.path().join("a"), d.path().join("b"), d.path().join("c"));
        write(&a, b"same content");
        write(&b, b"same content");
        write(&c, b"other");
        let token = CancellationToken::new();
        let mut seen = ImportSeen::default();
        let fa = FileFacts::read(&a).unwrap();
        let mut h = None;
        assert_eq!(seen.find(&a, &fa, &mut h, &token).unwrap(), None);
        assert_eq!(h, None, "候補がなければ読まない");
        seen.add(a.clone(), fa.clone(), None);
        let fb = FileFacts::read(&b).unwrap();
        let mut hb = None;
        assert_eq!(
            seen.find(&b, &fb, &mut hb, &token).unwrap().as_deref(),
            Some(a.as_path())
        );
        assert_eq!(hb, Some(genzo_catalog::full_hash(&b).unwrap()));
        // 比べるために求めた a の値は、登録の後に取り出せる。
        assert!(seen.take_computed().is_empty(), "まだ登録していない");
        seen.set_registered(a.clone(), VariantId::new(7), FileId::new(3), 1);
        assert_eq!(seen.variant_of(&a), Some(VariantId::new(7)));
        let computed = seen.take_computed();
        assert_eq!(computed.len(), 1);
        assert_eq!(
            (computed[0].0, computed[0].1, &computed[0].3),
            (FileId::new(3), 1, &genzo_catalog::full_hash(&a).unwrap())
        );
        assert!(seen.take_computed().is_empty());
        let fc = FileFacts::read(&c).unwrap();
        let mut hc = None;
        assert_eq!(seen.find(&c, &fc, &mut hc, &token).unwrap(), None);
    }
}
