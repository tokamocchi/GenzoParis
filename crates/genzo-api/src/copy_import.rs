//! コピー取り込み（IMP-02・IMP-03・IMP-05。04 の 3.12 節・6.4 節）。
//!
//! SD カードなどのフォルダの写真・動画を、コピー先のフォルダへ規則（[`crate::naming`]）に従ってコピーして
//! から、コピーしたファイルを登録する。**元のファイルは読み取り専用で開くだけで、移動・削除・変更しない**
//! （DATA-01。元を消す機能は作らない）。
//!
//! 1. **準備**（並列）: フォルダを走査し（取り込みと同じ規則。コピー先が中にあれば、コピー先は走査しない）、
//!    ファイルごとに、登録済みの写真との重複の確認（IMP-04）とメタデータの読み取り（ワーカー）をする。
//!    重複と思われるものを取り込まない設定なら、ここで外す。
//! 2. **組と順序**: 同じフォルダで拡張子を除いた名前が同じ RAW と JPEG（IMP-06）は 1 つの組にし、同じ名前で
//!    一緒にコピーする（別々に取り込む設定でも、名前はそろえる）。組を撮影日時の順に並べ、連番（`{seq}`）を
//!    組ごとに 1 つ進める。撮影日時が分からなければ、ファイルの更新日時を既定のタイムゾーンで読んだ時刻を
//!    使う（[`crate::types::CopyReport::dated_by_file_time`] に数える）。
//! 3. **名前**: フォルダの規則・リネームの規則で名前を作り、使えない名前なら `-1`・`-2` …を付ける
//!    （組のすべてのファイルで同じ番号）。使えない名前は、この取り込みで決めた名前、カタログに登録済みの名前
//!    （そのフォルダ・名前の比較キーが同じもの）、コピー先にある名前（大文字・小文字と Unicode の正規化だけが
//!    違う名前を含む）。組に RAW・JPEG があれば、**拡張子を除いた名前**が、ほかの写真の RAW・JPEG（同じ 3 つの
//!    うち、組の名前そのものでないもの）と同じ名前も使わない（同じフォルダで拡張子だけが違う RAW と JPEG は、
//!    登録で 1 枚の写真にまとめられ、削除で両方が対象になるため。IMP-06。別々に取り込む設定でも同じ）。ただし、コピー先に**同じ名前で同じ内容**（大きさと全体の BLAKE3 が同じ）の、登録して
//!    いないファイルがあれば、コピーせずにそれを登録する（前の取り込みが公開の後・登録の前に終わった場合の
//!    続き。[`crate::types::CopiedFile::existing`]）。
//! 4. **コピー**（1 つずつ）: コピー先と同じフォルダの一時ファイル（[`TEMP_PREFIX`] で始まる隠しファイル）に、
//!    元を読みながら BLAKE3 を求めて書き、更新日時を元と同じにして、ディスクに確定（fsync。macOS は
//!    `F_FULLFSYNC`）する。元のファイルがコピーの間に変わっていないこと（大きさ・更新日時・クイックハッシュ）と、
//!    一時ファイルを先頭から読み直した BLAKE3 が元と同じことを確かめてから、**上書きしない名前の変更**
//!    （`persist_noclobber`）で公開する。確定の前に同じ名前ができていたら、組で公開したファイルを消して次の
//!    番号で試す。読み直しは OS のキャッシュから読まれうる（ディスクの媒体そのものの読み直しは保証しない）。
//! 5. **登録**: [`IMPORT_BATCH_FILES`] 件ごとに（組は分けないので 1 件多くなることがある）、1 つのトランザクションで登録し、新しく登録した
//!    写真にキーワード・説明のメタデータを付け（IMP-05）、コピーで求めた全体のハッシュを記録する。続けて
//!    サムネイルを作る（取り込みと同じ）。
//!
//! **失敗・取り消し**: このジョブが作ったものだけを片付ける。一時ファイルは失敗・取り消し・パニックで消える
//! （`NamedTempFile`）。組の途中で失敗したら、その組で公開したファイルを消す（消す前に、公開した直後の大きさ・
//! 更新日時・クイックハッシュと同じことを確かめる）。カタログへの登録が失敗したら、そのバッチで公開した
//! ファイルを同じように消してからジョブを失敗にする（前のバッチは登録済みで、カタログとファイルは食い違わない）。
//! 登録をコミットした後の失敗（結果の集計など）では、そのバッチのコピーは消さずにジョブを失敗にする（全体の
//! ハッシュを記録できないときは警告だけにする）。
//! 取り消されたら、登録の済んでいない完全な組を登録してから止める。最後に、このジョブが作ったフォルダのうち
//! 空のものを消す。コピー先の空きが足りない（`StorageFull`）ときは、残りのファイルをコピーしない。
//!
//! **終了したときの残り**: プロセスが強制終了・電源断で止まると、一時ファイル（隠しファイル）が残ることがある。
//! 次のコピー取り込みが同じフォルダに書くときに、[`STALE_TEMP_AGE`] より古いものを消す。公開した後・登録の前に
//! 止まったファイルは残る（登録していない完全なコピー）。同じフォルダを取り込み直すと、3. の規則で
//! もう一度コピーせずに登録する。

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, FixedOffset, NaiveDateTime, Utc};
use genzo_catalog::text::path_key;
use genzo_catalog::{
    FileFacts, ImportTags, JPEG_EXTENSIONS, MediaMetadata, PairClass, RAW_EXTENSIONS, RegisterFile,
    RegisterOptions, pair_class,
};
use genzo_jobs::CancellationToken;
use genzo_model::{FolderId, VariantId, VolumeId};
use genzo_worker::{Lane, WorkerClientError};

use crate::config::IMPORT_BATCH_FILES;
use crate::core::{CopyImportHook, CopyImportStage, Inner};
use crate::error::ApiError;
use crate::identity::{ExistingCopy, HashFailure, ImportSeen, check_catalog_duplicate};
use crate::import::{
    Candidate, ProbeTarget, ResolvedOptions, ThumbTask, after_import, announce_changes,
    excluded_dirs, make_thumbnails, parallel_map, probe_request, register_requests, scan,
    tally_outcomes,
};
use crate::jobs::JobCtx;
use crate::naming::{
    MAX_COLLISION_SUFFIX, NameInput, TemplatePart, join_extension, render_stem, safe_file_stem,
    split_extension, with_suffix,
};
use crate::paths::{VolumePath, absolute_lexical, join_rel};
use crate::types::{CopiedFile, CopyReport, CopySettings, DuplicateFile, FileIssue, ImportReport};

/// コピーの一時ファイルの名前の先頭（Unix・macOS では隠しファイル。取り込みの走査も飛ばす）。
pub const TEMP_PREFIX: &str = ".genzo-import-";

/// コピーの一時ファイルの名前の末尾。
pub const TEMP_SUFFIX: &str = ".tmp";

/// 前のコピー取り込みが残した一時ファイルを消すまでの時間（**仮置き**: 1 時間。書いている途中の一時ファイルは
/// 更新日時が新しいので、別のコピーが書いているものは消さない）。
pub const STALE_TEMP_AGE: Duration = Duration::from_secs(60 * 60);

/// コピーで一度に読む長さ（この大きさごとに取り消しを確かめる）。
const COPY_BUFFER: usize = 1024 * 1024;

/// コピー取り込みの、始める前に確かめた設定。
#[derive(Debug, Clone)]
pub(crate) struct CopyPlan {
    /// コピー先（絶対パス）。
    destination: PathBuf,
    settings: CopySettings,
    /// リネームのテンプレートの部品（元の名前のままなら `None`）。
    parts: Option<Vec<TemplatePart>>,
    dry_run: bool,
}

impl CopyPlan {
    /// 設定を確かめる（テンプレート・コピー先のフォルダ）。
    pub(crate) fn new(
        inner: &Inner,
        root: &Path,
        settings: CopySettings,
        dry_run: bool,
    ) -> Result<Self, ApiError> {
        let parts = settings
            .rename
            .as_ref()
            .map(crate::naming::RenameRule::parse)
            .transpose()?;
        if settings.destination.as_os_str().is_empty() {
            return Err(ApiError::InvalidArgument(
                "コピー先のフォルダを指定してください".to_owned(),
            ));
        }
        let destination = absolute_lexical(&settings.destination)?;
        match fs::metadata(&destination) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(ApiError::InvalidArgument(format!(
                    "コピー先がフォルダではありません: {}",
                    destination.display()
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(ApiError::InvalidArgument(format!(
                    "コピー先のフォルダがありません（先に作ってください）: {}",
                    destination.display()
                )));
            }
            Err(e) => return Err(ApiError::io(&destination, e)),
        }
        // パスがボリュームと相対パスに分けられること（UTF-8 で表せるなど）。
        crate::volumes::locate(inner.volume_probe.as_ref(), &destination)?;
        if excluded_dirs(inner)
            .contains(&destination, fs::canonicalize(&destination).ok().as_deref())
        {
            return Err(ApiError::InvalidArgument(format!(
                "アプリのデータのフォルダ（プレビューのキャッシュ・バックアップ）にはコピーできません: {}",
                destination.display()
            )));
        }
        let root = absolute_lexical(root)?;
        let same = root == destination
            || matches!(
                (fs::canonicalize(&root), fs::canonicalize(&destination)),
                (Ok(a), Ok(b)) if a == b
            );
        if same {
            return Err(ApiError::InvalidArgument(
                "取り込むフォルダと同じフォルダにはコピーできません（コピー先を変えるか、その場で登録してください）"
                    .to_owned(),
            ));
        }
        Ok(Self {
            destination,
            settings,
            parts,
            dry_run,
        })
    }
}

/// `dir` が `parent` の中（`parent` そのものは含まない）か（字面の絶対パスか、解決した実際のパスで）。
fn is_inside(dir: &Path, parent: &Path) -> bool {
    let lexical = dir != parent && dir.starts_with(parent);
    let canonical = match (fs::canonicalize(dir), fs::canonicalize(parent)) {
        (Ok(d), Ok(p)) => d != p && d.starts_with(&p),
        _ => false,
    };
    lexical || canonical
}

/// 準備したファイル。
#[derive(Debug)]
struct Source {
    cand: Candidate,
    facts: FileFacts,
    mtime: SystemTime,
    /// 元のファイルの全体のハッシュ（求めていれば）。
    full_hash: Option<String>,
    /// 同じ内容の登録済みのファイル（取り込む設定のときに、結果に挙げる）。
    duplicate_of: Option<ExistingCopy>,
    /// 登録の要求（フォルダ・名前・内容はコピー先のものに入れ替える）。
    request: RegisterFile,
    /// フォルダ・名前に使う日時（現地の時刻）。
    when: NaiveDateTime,
    /// 撮影日時が分からず、ファイルの更新日時を使ったか。
    by_file_time: bool,
    /// カメラの機種。
    camera: Option<String>,
}

/// 1 ファイルの準備の結果。
enum Prepared {
    Ready(Box<Source>),
    Duplicate(ExistingCopy),
    NotRegistered(String),
    Cancelled,
}

/// 撮影日時（現地の時刻）か、なければファイルの更新日時を既定のタイムゾーンで読んだ時刻。
fn date_for(
    request: &RegisterFile,
    mtime: SystemTime,
    offset: FixedOffset,
) -> (NaiveDateTime, bool) {
    match request.capture.local() {
        Some(t) => (t.naive_local(), false),
        None => (
            DateTime::<Utc>::from(mtime)
                .with_timezone(&offset)
                .naive_local(),
            true,
        ),
    }
}

/// カメラの機種（なければメーカー）。
fn camera_of(request: &RegisterFile) -> Option<String> {
    match &request.metadata {
        MediaMetadata::Photo(p) => p
            .model
            .as_deref()
            .or(p.make.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        _ => None,
    }
}

fn prepare_source(ctx: &JobCtx<'_>, c: &Candidate, options: ResolvedOptions) -> Prepared {
    let inner = ctx.inner;
    if ctx.is_cancelled() {
        return Prepared::Cancelled;
    }
    let facts = match FileFacts::read(&c.path) {
        Ok(f) => f,
        Err(e) => return Prepared::NotRegistered(format!("ファイルを読めません: {e}")),
    };
    let mtime = match fs::metadata(&c.path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(e) => return Prepared::NotRegistered(format!("更新日時を読めません: {e}")),
    };
    let check = match check_catalog_duplicate(inner, &c.path, &facts, ctx.token()) {
        Ok(check) => check,
        Err(e) if e.is_cancelled() => return Prepared::Cancelled,
        Err(e) => return Prepared::NotRegistered(e.to_string()),
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
        // 登録の前にコピー先のフォルダに入れ替える。
        folder: FolderId::new(0),
        name: &c.name,
    };
    let request = match probe_request(
        &pool,
        &target,
        facts.clone(),
        offset,
        Lane::Batch,
        ctx.token(),
    ) {
        Ok(r) => r,
        Err(WorkerClientError::Cancelled) => return Prepared::Cancelled,
        Err(e) => {
            return Prepared::NotRegistered(format!(
                "ワーカーを起動できないためコピーしませんでした（もう一度取り込むと処理します）: {e}"
            ));
        }
    };
    let (when, by_file_time) = date_for(&request, mtime, offset);
    let camera = camera_of(&request);
    Prepared::Ready(Box::new(Source {
        cand: c.clone(),
        facts,
        mtime,
        full_hash: check.full_hash,
        duplicate_of: check.duplicate_of,
        request,
        when,
        by_file_time,
        camera,
    }))
}

/// 一緒にコピーするファイル（RAW と同時記録の JPEG。最初が主）。
#[derive(Debug)]
struct Group {
    members: Vec<Source>,
}

impl Group {
    /// フォルダ・名前に使う日時とカメラ（撮影日時の分かる最初のファイル。なければ主のファイル）。
    fn basis(&self) -> &Source {
        self.members
            .iter()
            .find(|m| !m.by_file_time)
            .unwrap_or(&self.members[0])
    }
}

/// RAW と JPEG の組を作り（同じフォルダで、拡張子を除いた名前の比較キーが同じもの。1 つの RAW に 1 つの
/// JPEG）、撮影日時の順に並べる。
fn make_groups(sources: Vec<Source>) -> Vec<Group> {
    let key = |s: &Source| {
        let (stem, _) = split_extension(&s.cand.name);
        (s.cand.dir_rel.clone(), path_key(stem))
    };
    let mut raw_of: HashMap<(String, String), usize> = HashMap::new();
    let mut jpeg_of: HashMap<(String, String), usize> = HashMap::new();
    for (i, s) in sources.iter().enumerate() {
        match pair_class(&s.cand.name) {
            PairClass::Raw => {
                raw_of.entry(key(s)).or_insert(i);
            }
            PairClass::Jpeg => {
                jpeg_of.entry(key(s)).or_insert(i);
            }
            PairClass::Other => {}
        }
    }
    let mut partner: HashMap<usize, usize> = HashMap::new();
    for (k, &r) in &raw_of {
        if let Some(&j) = jpeg_of.get(k) {
            partner.insert(r, j);
        }
    }
    let paired_jpegs: HashSet<usize> = partner.values().copied().collect();
    let mut slots: Vec<Option<Source>> = sources.into_iter().map(Some).collect();
    let mut groups = Vec::new();
    for i in 0..slots.len() {
        if paired_jpegs.contains(&i) {
            continue;
        }
        let Some(first) = slots[i].take() else {
            continue;
        };
        let mut members = vec![first];
        if let Some(&j) = partner.get(&i)
            && let Some(jpeg) = slots[j].take()
        {
            members.push(jpeg);
        }
        groups.push(Group { members });
    }
    groups.sort_by(|a, b| {
        (a.basis().when, &a.members[0].cand.path).cmp(&(b.basis().when, &b.members[0].cand.path))
    });
    groups
}

/// コピー先に置いた（置く）ファイル。
#[derive(Debug)]
struct Placed {
    source: Source,
    /// コピー先のフォルダのボリューム内の相対パス。
    dest_rel_dir: String,
    dest_name: String,
    dest_path: PathBuf,
    /// 公開した後のコピー先の内容（名前を決めるだけのときは `None`）。
    dest_facts: Option<FileFacts>,
    /// 全体のハッシュ（コピーで求めた・既にあったファイルと比べた値）。
    full_hash: Option<String>,
    existing: bool,
    bytes: u64,
    /// 組の日付を、撮影日時ではなくファイルの更新日時で決めたか。
    dated_by_file_time: bool,
}

/// 組をコピーできなかった理由。
enum GroupFailure {
    /// 取り消された。
    Cancelled,
    /// この組だけ失敗した（理由）。
    Failed(String),
    /// コピー先の空きが足りない（残りもコピーしない）。
    Fatal(String),
}

/// 1 ファイルのコピーの失敗（写真の移動（FILE-02。`fileops`）でも使う）。
#[derive(Debug)]
pub(crate) enum CopyFailure {
    Cancelled,
    /// 確定の前に同じ名前ができていた。
    Exists,
    Failed(String),
    NoSpace(String),
}

/// 1 ファイルのコピーの結果。
#[derive(Debug)]
pub(crate) struct CopiedInfo {
    pub(crate) facts: FileFacts,
    pub(crate) full_hash: String,
    pub(crate) bytes: u64,
}

/// コピー先の名前の使える・使えない。
enum Availability {
    Free,
    /// 同じ名前で同じ内容の、登録していないファイルがある（全体のハッシュ）。
    Same(String),
    Taken,
}

/// 書き込みの失敗を分類する（空きが足りないなら [`CopyFailure::NoSpace`]）。
fn write_failure(what: &str, path: &Path, e: &io::Error) -> CopyFailure {
    let message = format!("{what}（{}）: {e}", path.display());
    if e.kind() == io::ErrorKind::StorageFull {
        CopyFailure::NoSpace(message)
    } else {
        CopyFailure::Failed(message)
    }
}

fn exists_no_follow(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// 名前の変更の後、フォルダの変更を永続化する（Unix。失敗しても公開は済んでいる）。
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = genzo_model::fs_sync::sync_file_full(&d);
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn temp_file_in(dir: &Path) -> io::Result<tempfile::NamedTempFile> {
    let mut b = tempfile::Builder::new();
    b.prefix(TEMP_PREFIX).suffix(TEMP_SUFFIX);
    // 一時ファイルは既定で所有者だけが読める（0600）ため、公開後に通常のファイルと同じ権限（umask を適用した
    // 0666）になるようにする（genzo-media の書き出しと同じ）。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        b.permissions(fs::Permissions::from_mode(0o666));
    }
    b.tempfile_in(dir)
}

/// ファイルを読みながら BLAKE3 を求める（取り消しを確かめる）。
fn hash_reader(mut r: impl Read, token: &CancellationToken) -> Result<String, HashFailure> {
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; COPY_BUFFER];
    loop {
        if token.is_cancelled() {
            return Err(HashFailure::Cancelled);
        }
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(HashFailure::Io(e.to_string())),
        };
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// 1 つのファイルを、一時ファイルに書いて確かめてから、上書きせずに `dest` として公開する（モジュールの
/// doc の 4.。`hook` はテスト専用のフック）。
pub(crate) fn copy_verified_with(
    hook: Option<&CopyImportHook>,
    source: &Path,
    facts: &FileFacts,
    mtime: SystemTime,
    dest: &Path,
    token: &CancellationToken,
) -> Result<CopiedInfo, CopyFailure> {
    let dir = dest.parent().unwrap_or(Path::new("."));
    let mut src = File::open(source).map_err(|e| {
        CopyFailure::Failed(format!(
            "元のファイルを開けません（{}）: {e}",
            source.display()
        ))
    })?;
    let mut tmp =
        temp_file_in(dir).map_err(|e| write_failure("一時ファイルを作れません", dir, &e))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; COPY_BUFFER];
    let mut bytes = 0u64;
    loop {
        if token.is_cancelled() {
            return Err(CopyFailure::Cancelled);
        }
        let n = match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(CopyFailure::Failed(format!(
                    "元のファイルを読めません（{}）: {e}",
                    source.display()
                )));
            }
        };
        hasher.update(&buf[..n]);
        tmp.as_file_mut()
            .write_all(&buf[..n])
            .map_err(|e| write_failure("コピー先に書けません", dest, &e))?;
        bytes += n as u64;
    }
    drop(src);
    let source_hash = hasher.finalize().to_hex().to_string();
    // 更新日時を元と同じにする（日付で探す道具のため。できなくてもコピーは使える）。
    if let Err(e) = tmp.as_file().set_modified(mtime) {
        tracing::warn!(path = %dest.display(), error = %e, "コピーの更新日時を元にそろえられません");
    }
    tmp.as_file_mut()
        .flush()
        .and_then(|()| genzo_model::fs_sync::sync_file_full(tmp.as_file()).map(|_| ()))
        .map_err(|e| write_failure("コピー先をディスクに確定できません", dest, &e))?;
    if let Some(hook) = hook {
        hook(&CopyImportStage::Written(tmp.path().to_path_buf())).map_err(CopyFailure::Failed)?;
    }
    // 元のファイルがコピーの間に変わっていないこと。
    match FileFacts::read(source) {
        Ok(now) if now == *facts && now.size == bytes => {}
        Ok(_) => {
            return Err(CopyFailure::Failed(format!(
                "コピーの間に元のファイルが変わりました（{}）",
                source.display()
            )));
        }
        Err(e) => {
            return Err(CopyFailure::Failed(format!(
                "元のファイルを確かめられません（{}）: {e}",
                source.display()
            )));
        }
    }
    // 書いた内容を先頭から読み直して照合する。
    let file = tmp.as_file_mut();
    file.seek(io::SeekFrom::Start(0))
        .map_err(|e| CopyFailure::Failed(format!("コピーを読み直せません: {e}")))?;
    match hash_reader(&mut *file, token) {
        Ok(h) if h == source_hash => {}
        Ok(_) => {
            return Err(CopyFailure::Failed(format!(
                "コピーした内容が元と一致しません（書き込みの誤りの可能性があります。{}）",
                source.display()
            )));
        }
        Err(HashFailure::Cancelled) => return Err(CopyFailure::Cancelled),
        Err(HashFailure::Io(e)) => {
            return Err(CopyFailure::Failed(format!("コピーを読み直せません: {e}")));
        }
    }
    // 上書きしない名前の変更で公開する。
    match tmp.persist_noclobber(dest) {
        Ok(_) => {}
        Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(CopyFailure::Exists);
        }
        Err(e) => {
            // ハードリンクも RENAME_NOREPLACE も使えないファイルシステム。名前が空いていることを確かめてから
            // 通常の名前の変更をする（確かめてから変えるまでの間の競合は防げない。書き出しと同じ制限）。
            let tmp = e.file;
            if exists_no_follow(dest) {
                return Err(CopyFailure::Exists);
            }
            tmp.persist(dest)
                .map_err(|e| write_failure("コピー先の名前を確定できません", dest, &e.error))?;
        }
    }
    sync_dir(dir);
    match FileFacts::read(dest) {
        Ok(facts) => Ok(CopiedInfo {
            facts,
            full_hash: source_hash,
            bytes,
        }),
        Err(e) => {
            // 公開した直後のファイル（このジョブが作ったもの）を読めない。残さない。
            let _ = fs::remove_file(dest);
            Err(CopyFailure::Failed(format!(
                "コピーしたファイルを確かめられません（{}）: {e}",
                dest.display()
            )))
        }
    }
}

/// このジョブが公開したファイルを消す（公開した直後の内容と同じときだけ。ほかのものに置き換わっていれば
/// 消さない）。消したら `true`。
pub(crate) fn remove_own_copy(path: &Path, facts: &FileFacts) -> bool {
    match FileFacts::read(path) {
        Ok(now) if now == *facts => match fs::remove_file(path) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "コピーしたファイルを消せません");
                false
            }
        },
        _ => {
            tracing::warn!(path = %path.display(), "コピーしたファイルが変わっているため消しません");
            false
        }
    }
}

/// 前のコピー取り込みが残した、古い一時ファイルを消す。
pub(crate) fn remove_stale_temp_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !(name.starts_with(TEMP_PREFIX) && name.ends_with(TEMP_SUFFIX)) {
            continue;
        }
        let stale = e.metadata().is_ok_and(|m| {
            m.is_file()
                && m.modified()
                    .ok()
                    .and_then(|t| now.duration_since(t).ok())
                    .is_some_and(|age| age >= STALE_TEMP_AGE)
        });
        if stale {
            match fs::remove_file(e.path()) {
                Ok(()) => {
                    tracing::info!(path = %e.path().display(), "前のコピー取り込みの一時ファイルを消しました")
                }
                Err(err) => {
                    tracing::warn!(path = %e.path().display(), error = %err, "前のコピー取り込みの一時ファイルを消せません")
                }
            }
        }
    }
}

/// コピーの状態（名前の予約・コピー先のフォルダの中身・作ったフォルダ）。
struct CopyState<'a> {
    ctx: &'a JobCtx<'a>,
    plan: &'a CopyPlan,
    dest_vol: VolumePath,
    /// コピー先のボリューム（カタログに登録されていなければ `None`）。
    dest_volume: Option<VolumeId>,
    /// この取り込みで決めた名前（コピー先のフォルダからの相対パスの比較キー）。
    planned: HashSet<String>,
    /// コピー先のフォルダの中身（名前の比較キー → 名前）。
    listings: HashMap<PathBuf, HashMap<String, String>>,
    /// このジョブが作ったフォルダ（作った順）。
    created_dirs: Vec<PathBuf>,
    /// 古い一時ファイルを片付けたフォルダ。
    cleaned_dirs: HashSet<PathBuf>,
}

impl<'a> CopyState<'a> {
    fn dir_of(&self, folder_rel: &str) -> PathBuf {
        join_rel(&self.plan.destination, folder_rel, "")
    }

    /// コピー先のフォルダのボリューム内の相対パス。
    fn volume_rel(&self, folder_rel: &str) -> String {
        match (self.dest_vol.rel.is_empty(), folder_rel.is_empty()) {
            (true, _) => folder_rel.to_owned(),
            (false, true) => self.dest_vol.rel.clone(),
            (false, false) => format!("{}/{folder_rel}", self.dest_vol.rel),
        }
    }

    fn listing(&mut self, dir: &Path) -> Result<&mut HashMap<String, String>, String> {
        if !self.listings.contains_key(dir) {
            let mut map = HashMap::new();
            match fs::read_dir(dir) {
                Ok(entries) => {
                    for e in entries {
                        let e = e.map_err(|e| {
                            format!("コピー先のフォルダを読めません（{}）: {e}", dir.display())
                        })?;
                        let name = e.file_name().to_string_lossy().into_owned();
                        map.insert(path_key(&name), name);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(format!(
                        "コピー先のフォルダを読めません（{}）: {e}",
                        dir.display()
                    ));
                }
            }
            self.listings.insert(dir.to_path_buf(), map);
        }
        Ok(self.listings.get_mut(dir).expect("上で入れた"))
    }

    /// カタログに、そのフォルダ・名前のファイルが登録済みか。
    fn registered(&self, folder_rel: &str, name: &str) -> Result<bool, ApiError> {
        let Some(volume) = self.dest_volume else {
            return Ok(false);
        };
        let rel = self.volume_rel(folder_rel);
        self.ctx.inner.with_catalog(|c| {
            Ok(match c.find_folder(volume, &rel)? {
                Some(folder) => c.find_file(folder, name)?.is_some(),
                None => false,
            })
        })
    }

    /// 組の名前 `names`（拡張子だけが違う）の、拡張子を除いた部分を、ほかの写真の RAW・JPEG が使っているか
    /// （モジュールの doc の 3.）。この取り込みで決めた名前・カタログに登録済みの名前・コピー先にある名前の
    /// うち、組の名前そのものでない RAW・JPEG を調べる。組に RAW・JPEG がなければ（動画など）調べない。
    fn stem_taken(&mut self, folder_rel: &str, names: &[String]) -> Result<bool, GroupFailure> {
        let Some(first) = names.iter().find(|n| pair_class(n) != PairClass::Other) else {
            return Ok(false);
        };
        let (stem, _) = split_extension(first);
        let own: HashSet<String> = names.iter().map(|n| path_key(n)).collect();
        let dir = self.dir_of(folder_rel);
        let listing = self.listing(&dir).map_err(GroupFailure::Failed)?;
        for ext in RAW_EXTENSIONS.iter().chain(JPEG_EXTENSIONS) {
            // 拡張子だけを小文字にした名前の比較キー（名前全体で求める。比較キーの小文字化は前後の文字で
            // 変わることがある）。
            let name_key = path_key(&format!("{stem}.{ext}"));
            if own.contains(&name_key) {
                continue;
            }
            if listing.contains_key(&name_key) {
                return Ok(true);
            }
        }
        for ext in RAW_EXTENSIONS.iter().chain(JPEG_EXTENSIONS) {
            let name = format!("{stem}.{ext}");
            if !own.contains(&path_key(&name))
                && self
                    .planned
                    .contains(&path_key(&format!("{folder_rel}/{name}")))
            {
                return Ok(true);
            }
        }
        let Some(volume) = self.dest_volume else {
            return Ok(false);
        };
        let rel = self.volume_rel(folder_rel);
        let registered = self
            .ctx
            .inner
            .with_catalog(|c| {
                Ok(match c.find_folder(volume, &rel)? {
                    Some(folder) => c.names_with_same_stem(folder, first)?,
                    None => Vec::new(),
                })
            })
            .map_err(|e| GroupFailure::Failed(e.to_string()))?;
        Ok(registered
            .iter()
            .any(|n| pair_class(n) != PairClass::Other && !own.contains(&path_key(n))))
    }

    /// コピー先の名前を使えるか（モジュールの doc の 3.）。
    fn availability(
        &mut self,
        folder_rel: &str,
        name: &str,
        member: &mut Source,
    ) -> Result<Availability, GroupFailure> {
        let key = path_key(&format!("{folder_rel}/{name}"));
        if self.planned.contains(&key) {
            return Ok(Availability::Taken);
        }
        if self
            .registered(folder_rel, name)
            .map_err(|e| GroupFailure::Failed(e.to_string()))?
        {
            return Ok(Availability::Taken);
        }
        let dir = self.dir_of(folder_rel);
        let existing = self
            .listing(&dir)
            .map_err(GroupFailure::Failed)?
            .get(&path_key(name))
            .cloned();
        let Some(actual) = existing else {
            return Ok(Availability::Free);
        };
        if actual != name {
            return Ok(Availability::Taken);
        }
        // 同じ名前のファイルが同じ内容なら、それを登録する。
        let path = dir.join(name);
        let regular = fs::symlink_metadata(&path)
            .is_ok_and(|m| m.file_type().is_file() && m.len() == member.facts.size);
        if !regular {
            return Ok(Availability::Taken);
        }
        let token = self.ctx.token();
        let own = match &member.full_hash {
            Some(h) => h.clone(),
            None => {
                match crate::identity::verified_full_hash(&member.cand.path, &member.facts, token) {
                    Ok(Some(h)) => {
                        member.full_hash = Some(h.clone());
                        h
                    }
                    Ok(None) => {
                        return Err(GroupFailure::Failed(format!(
                            "コピーの前に元のファイルが変わりました（{}）",
                            member.cand.path.display()
                        )));
                    }
                    Err(HashFailure::Cancelled) => return Err(GroupFailure::Cancelled),
                    Err(HashFailure::Io(e)) => return Err(GroupFailure::Failed(e)),
                }
            }
        };
        let theirs = match File::open(&path) {
            Ok(f) => match hash_reader(f, token) {
                Ok(h) => h,
                Err(HashFailure::Cancelled) => return Err(GroupFailure::Cancelled),
                Err(HashFailure::Io(_)) => return Ok(Availability::Taken),
            },
            Err(_) => return Ok(Availability::Taken),
        };
        Ok(if theirs == own {
            Availability::Same(own)
        } else {
            Availability::Taken
        })
    }

    /// フォルダを作る（作ったものを覚える）。古い一時ファイルを片付ける。
    fn ensure_dir(&mut self, dir: &Path) -> Result<(), GroupFailure> {
        let mut missing = Vec::new();
        let mut cur = Some(dir);
        while let Some(d) = cur {
            if d.is_dir() {
                break;
            }
            missing.push(d.to_path_buf());
            cur = d.parent();
        }
        if !missing.is_empty() {
            fs::create_dir_all(dir).map_err(|e| {
                match write_failure("フォルダを作れません", dir, &e) {
                    CopyFailure::NoSpace(m) => GroupFailure::Fatal(m),
                    CopyFailure::Failed(m) => GroupFailure::Failed(m),
                    CopyFailure::Cancelled | CopyFailure::Exists => unreachable!(),
                }
            })?;
            self.created_dirs.extend(missing.into_iter().rev());
        }
        if self.cleaned_dirs.insert(dir.to_path_buf()) {
            remove_stale_temp_files(dir);
        }
        Ok(())
    }

    /// 組の名前を決めて、コピーする（名前を決めるだけのときはコピーしない）。
    fn place_group(
        &mut self,
        mut members: Vec<Source>,
        folder_rel: &str,
        stem: &str,
    ) -> Result<Vec<Placed>, (GroupFailure, Vec<Source>)> {
        match self.place_members(&mut members, folder_rel, stem) {
            Ok(infos) => Ok(members
                .into_iter()
                .zip(infos)
                .map(|(source, (name, facts, hash, existing, bytes))| Placed {
                    dest_rel_dir: self.volume_rel(folder_rel),
                    dest_path: self.dir_of(folder_rel).join(&name),
                    dest_name: name,
                    dest_facts: facts,
                    full_hash: hash,
                    existing,
                    bytes,
                    dated_by_file_time: false,
                    source,
                })
                .collect()),
            Err(f) => Err((f, members)),
        }
    }

    #[allow(clippy::type_complexity)]
    fn place_members(
        &mut self,
        members: &mut [Source],
        folder_rel: &str,
        stem: &str,
    ) -> Result<Vec<(String, Option<FileFacts>, Option<String>, bool, u64)>, GroupFailure> {
        let dir = self.dir_of(folder_rel);
        let token = self.ctx.token().clone();
        'attempt: for n in 0..=MAX_COLLISION_SUFFIX {
            if token.is_cancelled() {
                return Err(GroupFailure::Cancelled);
            }
            let names: Vec<String> = members
                .iter()
                .map(|m| join_extension(&with_suffix(stem, n), split_extension(&m.cand.name).1))
                .collect();
            // 別の写真の RAW と JPEG を同じ名前で並べない（登録で 1 枚にまとめられる。IMP-06）。
            if self.stem_taken(folder_rel, &names)? {
                continue 'attempt;
            }
            let mut same: Vec<Option<String>> = Vec::with_capacity(members.len());
            for (m, name) in members.iter_mut().zip(&names) {
                match self.availability(folder_rel, name, m)? {
                    Availability::Free => same.push(None),
                    Availability::Same(h) => same.push(Some(h)),
                    Availability::Taken => continue 'attempt,
                }
            }
            let mut out = Vec::with_capacity(members.len());
            if self.plan.dry_run {
                for ((m, name), same) in members.iter().zip(&names).zip(same) {
                    let existing = same.is_some();
                    out.push((
                        name.clone(),
                        None,
                        same.or_else(|| m.full_hash.clone()),
                        existing,
                        0,
                    ));
                }
            } else {
                self.ensure_dir(&dir)?;
                // この組で公開したファイル（失敗したら消す）。
                let mut published: Vec<(PathBuf, FileFacts)> = Vec::new();
                let rollback = |published: &[(PathBuf, FileFacts)], state: &mut Self| {
                    for (p, f) in published {
                        if remove_own_copy(p, f)
                            && let (Some(parent), Some(name)) = (p.parent(), p.file_name())
                            && let Some(list) = state.listings.get_mut(parent)
                        {
                            list.remove(&path_key(&name.to_string_lossy()));
                        }
                    }
                };
                for ((m, name), same) in members.iter().zip(&names).zip(same) {
                    let dest = dir.join(name);
                    if let Some(hash) = same {
                        match FileFacts::read(&dest) {
                            Ok(facts) => out.push((name.clone(), Some(facts), Some(hash), true, 0)),
                            Err(e) => {
                                rollback(&published, self);
                                return Err(GroupFailure::Failed(format!(
                                    "コピー先のファイルを読めません（{}）: {e}",
                                    dest.display()
                                )));
                            }
                        }
                        continue;
                    }
                    match copy_verified_with(
                        self.ctx.inner.on_copy_import.as_ref(),
                        &m.cand.path,
                        &m.facts,
                        m.mtime,
                        &dest,
                        &token,
                    ) {
                        Ok(c) => {
                            published.push((dest.clone(), c.facts.clone()));
                            if let Some(list) = self.listings.get_mut(&dir) {
                                list.insert(path_key(name), name.clone());
                            }
                            out.push((
                                name.clone(),
                                Some(c.facts),
                                Some(c.full_hash),
                                false,
                                c.bytes,
                            ));
                            if let Some(hook) = &self.ctx.inner.on_copy_import
                                && let Err(msg) = hook(&CopyImportStage::Published(dest.clone()))
                            {
                                rollback(&published, self);
                                return Err(GroupFailure::Failed(msg));
                            }
                        }
                        Err(CopyFailure::Exists) => {
                            // 確定の前に同じ名前ができていた。組で公開したものを消して、次の番号で試す。
                            rollback(&published, self);
                            self.listings.remove(&dir);
                            continue 'attempt;
                        }
                        Err(CopyFailure::Cancelled) => {
                            rollback(&published, self);
                            return Err(GroupFailure::Cancelled);
                        }
                        Err(CopyFailure::Failed(m)) => {
                            rollback(&published, self);
                            return Err(GroupFailure::Failed(m));
                        }
                        Err(CopyFailure::NoSpace(m)) => {
                            rollback(&published, self);
                            return Err(GroupFailure::Fatal(m));
                        }
                    }
                }
            }
            for name in &names {
                self.planned
                    .insert(path_key(&format!("{folder_rel}/{name}")));
            }
            return Ok(out);
        }
        Err(GroupFailure::Failed(format!(
            "空いている名前がありません（{}-{MAX_COLLISION_SUFFIX} まで試しました）",
            stem
        )))
    }

    /// このジョブが作ったフォルダのうち、空のものを消す（作った順の逆に。空でなければ消えない）。
    fn remove_empty_created_dirs(&mut self) {
        for d in self.created_dirs.drain(..).rev() {
            let _ = fs::remove_dir(&d);
        }
    }
}

/// 登録を待っているファイルと、結果に使う値。
#[derive(Default)]
struct Pending {
    placed: Vec<Placed>,
}

/// コピー取り込みの本体（モジュールの doc）。
pub(crate) fn run_copy_import(
    ctx: &JobCtx<'_>,
    root: &Path,
    recursive: bool,
    options: ResolvedOptions,
    tags: &ImportTags,
    plan: &CopyPlan,
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
    let mut excluded = excluded_dirs(inner);
    if excluded.contains(&root, fs::canonicalize(&root).ok().as_deref()) {
        return Err(ApiError::InvalidArgument(format!(
            "アプリのデータのフォルダ（プレビューのキャッシュ・バックアップ）は取り込めません: {}",
            root.display()
        )));
    }
    // コピー先が取り込むフォルダの中にあれば、コピー先は走査しない（コピーしたものをもう一度コピーしない）。
    // 取り込むフォルダがコピー先の中にある場合（`写真/取り込み待ち` から `写真` へ）は、何も除かない。
    if is_inside(&plan.destination, &root) {
        excluded.add(&plan.destination);
    }
    let src_vol = crate::volumes::locate(inner.volume_probe.as_ref(), &root)?;
    let dest_vol = crate::volumes::locate(inner.volume_probe.as_ref(), &plan.destination)?;
    let mut report = ImportReport {
        root: root.clone(),
        skip_duplicates: options.skip_duplicates,
        separate_raw_jpeg: options.separate_raw_jpeg,
        copy: Some(Box::new(CopyReport {
            destination: plan.destination.clone(),
            dry_run: plan.dry_run,
            ..Default::default()
        })),
        ..Default::default()
    };
    let scanned = scan(&root, &src_vol.rel, recursive, &excluded, ctx.token());
    report.files_found = scanned.files.len() as u64;
    report.skipped_links = scanned.skipped_links;
    report.not_registered.extend(scanned.unreadable);
    if ctx.is_cancelled() {
        report.cancelled = true;
        return Ok(report);
    }
    let threads = inner.pool()?.worker_count(Lane::Batch);
    let total = report.files_found * 2;
    let mut done = 0u64;
    ctx.progress(0, total);

    // 1. 準備（重複の確認・メタデータ）。
    let mut sources = Vec::with_capacity(scanned.files.len());
    for chunk in scanned.files.chunks(IMPORT_BATCH_FILES) {
        if ctx.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let prepared = parallel_map(chunk, threads, |c| prepare_source(ctx, c, options));
        for (c, p) in chunk.iter().zip(prepared) {
            match p {
                Prepared::Ready(s) => sources.push(*s),
                Prepared::Duplicate(existing) => report.duplicates.push(DuplicateFile {
                    path: c.path.clone(),
                    existing_path: existing.path,
                    existing_variant_id: Some(existing.variant_id),
                    skipped: true,
                }),
                Prepared::NotRegistered(reason) => report.not_registered.push(FileIssue {
                    path: c.path.clone(),
                    reason,
                }),
                Prepared::Cancelled => report.cancelled = true,
            }
        }
        done += chunk.len() as u64;
        ctx.progress(done, total);
        if report.cancelled {
            break;
        }
    }
    if report.cancelled {
        return Ok(report);
    }
    done = report.files_found;
    ctx.progress(done, total);

    // 2. 組と順序。
    let groups = make_groups(sources);

    // 3〜5. 名前・コピー・登録。
    let dest_volume = inner.with_catalog(|c| {
        Ok(c.volumes()?
            .into_iter()
            .find(|v| v.uuid == dest_vol.uuid)
            .map(|v| v.id))
    })?;
    let mut state = CopyState {
        ctx,
        plan,
        dest_vol,
        dest_volume,
        planned: HashSet::new(),
        listings: HashMap::new(),
        created_dirs: Vec::new(),
        cleaned_dirs: HashSet::new(),
    };
    let register_options = RegisterOptions {
        pair_raw_jpeg: !options.separate_raw_jpeg,
    };
    let mut seen = ImportSeen::default();
    // 同じ取り込みの中の重複で、先のファイルの写真が登録の後に分かるもの（結果の位置と、先の元のファイル）。
    let mut dup_fix: Vec<(usize, PathBuf)> = Vec::new();
    let mut pending = Pending::default();
    let mut regenerate: Vec<VariantId> = Vec::new();
    let mut no_embedded: Vec<VariantId> = Vec::new();
    let mut stop: Option<String> = None;
    let mut next_seq: u64 = plan
        .settings
        .rename
        .as_ref()
        .map_or(1, |r| u64::from(r.sequence_start));
    let mut copied_dest: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut result: Result<(), ApiError> = Ok(());
    for group in groups {
        let count = group.members.len() as u64;
        if report.cancelled {
            break;
        }
        if ctx.is_cancelled() {
            report.cancelled = true;
            break;
        }
        if let Some(reason) = &stop {
            for m in group.members {
                report.not_registered.push(FileIssue {
                    path: m.cand.path,
                    reason: format!(
                        "コピー先の空きが足りないため、コピーしませんでした（{reason}）"
                    ),
                });
            }
            done += count;
            ctx.progress(done, total);
            continue;
        }
        // 同じ取り込みの中の重複（IMP-04）。
        let mut members = Vec::with_capacity(group.members.len());
        for mut m in group.members {
            let mut own = m.full_hash.clone();
            match seen.find(&m.cand.path, &m.facts, &mut own, ctx.token()) {
                Ok(Some(first)) => {
                    let existing_path = copied_dest.get(&first).cloned().or(Some(first.clone()));
                    dup_fix.push((report.duplicates.len(), first));
                    report.duplicates.push(DuplicateFile {
                        path: m.cand.path.clone(),
                        existing_path,
                        existing_variant_id: None,
                        skipped: options.skip_duplicates,
                    });
                    if options.skip_duplicates {
                        continue;
                    }
                }
                Ok(None) => {}
                Err(e) if e.is_cancelled() => {
                    report.cancelled = true;
                    break;
                }
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
            m.full_hash = own;
            seen.add(m.cand.path.clone(), m.facts.clone(), m.full_hash.clone());
            if let Some(existing) = &m.duplicate_of {
                // 登録済みのファイルと同じ内容だが、取り込む設定（skip_duplicates が偽）。
                report.duplicates.push(DuplicateFile {
                    path: m.cand.path.clone(),
                    existing_path: existing.path.clone(),
                    existing_variant_id: Some(existing.variant_id),
                    skipped: false,
                });
            }
            members.push(m);
        }
        if report.cancelled || result.is_err() {
            break;
        }
        if members.is_empty() {
            done += count;
            ctx.progress(done, total);
            continue;
        }
        let group = Group { members };
        let basis = group.basis();
        let source_rel = basis
            .cand
            .dir_rel
            .strip_prefix(src_vol.rel.as_str())
            .unwrap_or(&basis.cand.dir_rel)
            .trim_start_matches('/')
            .to_owned();
        let folder_rel = plan
            .settings
            .folder_rule
            .folder_for(basis.when, &source_rel);
        let (original_stem, _) = split_extension(&group.members[0].cand.name);
        let stem = match (&plan.settings.rename, &plan.parts) {
            (Some(rule), Some(parts)) => render_stem(
                parts,
                rule,
                &NameInput {
                    original_stem,
                    when: basis.when,
                    camera: basis.camera.as_deref(),
                    sequence: next_seq,
                },
            ),
            _ => safe_file_stem(original_stem),
        };
        next_seq += 1;
        let dated_by_file_time = basis.by_file_time;
        match state.place_group(group.members, &folder_rel, &stem) {
            Ok(placed) => {
                for mut p in placed {
                    copied_dest.insert(p.source.cand.path.clone(), p.dest_path.clone());
                    p.dated_by_file_time = dated_by_file_time;
                    pending.placed.push(p);
                }
            }
            Err((GroupFailure::Cancelled, _)) => {
                report.cancelled = true;
            }
            Err((GroupFailure::Failed(reason), members)) => {
                for m in members {
                    report.not_registered.push(FileIssue {
                        path: m.cand.path,
                        reason: reason.clone(),
                    });
                }
            }
            Err((GroupFailure::Fatal(reason), members)) => {
                for m in members {
                    report.not_registered.push(FileIssue {
                        path: m.cand.path,
                        reason: reason.clone(),
                    });
                }
                stop = Some(reason);
            }
        }
        done += count;
        ctx.progress(done, total);
        if pending.placed.len() >= IMPORT_BATCH_FILES
            && let Err(e) = flush(
                ctx,
                &mut state,
                &mut pending,
                &register_options,
                tags,
                &mut report,
                &mut seen,
                &mut regenerate,
                &mut no_embedded,
                threads,
            )
        {
            result = Err(e);
            break;
        }
    }
    // 取り消されたときも、コピーし終えた組は登録する（カタログとファイルを食い違わせない）。
    if result.is_ok() {
        result = flush(
            ctx,
            &mut state,
            &mut pending,
            &register_options,
            tags,
            &mut report,
            &mut seen,
            &mut regenerate,
            &mut no_embedded,
            threads,
        );
    } else {
        rollback_pending(&mut pending);
    }
    state.remove_empty_created_dirs();
    result?;
    if let Some(copy) = report.copy.as_mut() {
        copy.next_sequence = next_seq;
    }
    for (i, first) in dup_fix {
        if let Some(d) = report.duplicates.get_mut(i)
            && d.existing_variant_id.is_none()
        {
            d.existing_variant_id = seen.variant_of(&first);
        }
    }
    if !plan.dry_run {
        after_import(inner, &report, regenerate, no_embedded);
    }
    Ok(report)
}

/// 登録の前に失敗したバッチの、このジョブが公開したファイルを消す（既にあったファイルは消さない）。
fn rollback_pending(pending: &mut Pending) {
    for p in pending.placed.drain(..) {
        if !p.existing
            && let Some(f) = &p.dest_facts
        {
            remove_own_copy(&p.dest_path, f);
        }
    }
}

/// 登録を待っているファイルを登録し、結果に入れ、サムネイルを作る（名前を決めるだけのときは結果に入れるだけ）。
/// 登録に失敗したら（コミットの前。何も登録されていない）、そのファイルのうちこのジョブが公開したものを消してから
/// エラーを返す。登録（コミット）の後の失敗では消さずに（`pending` を空にして）エラーを返す。
#[allow(clippy::too_many_arguments)]
fn flush(
    ctx: &JobCtx<'_>,
    state: &mut CopyState<'_>,
    pending: &mut Pending,
    register_options: &RegisterOptions,
    tags: &ImportTags,
    report: &mut ImportReport,
    seen: &mut ImportSeen,
    regenerate: &mut Vec<VariantId>,
    no_embedded: &mut Vec<VariantId>,
    threads: usize,
) -> Result<(), ApiError> {
    if pending.placed.is_empty() {
        return Ok(());
    }
    let inner = ctx.inner;
    if state.plan.dry_run {
        let copy = report.copy.as_mut().expect("コピー取り込みの結果");
        for p in pending.placed.drain(..) {
            if p.dated_by_file_time {
                copy.dated_by_file_time += 1;
            }
            copy.copied.push(CopiedFile {
                source: p.source.cand.path,
                destination: p.dest_path,
                existing: p.existing,
            });
        }
        return Ok(());
    }
    let registered = (|| -> Result<_, ApiError> {
        if let Some(hook) = &inner.on_copy_import {
            hook(&CopyImportStage::Registering(pending.placed.len()))
                .map_err(ApiError::Internal)?;
        }
        // コピー先のフォルダを確保する。
        let mut rels: Vec<&str> = pending
            .placed
            .iter()
            .map(|p| p.dest_rel_dir.as_str())
            .collect();
        rels.sort_unstable();
        rels.dedup();
        let mount = state
            .dest_vol
            .mount
            .to_str()
            .ok_or_else(|| ApiError::InvalidArgument("UTF-8 で表せないパスです".to_owned()))?
            .to_owned();
        let uuid = state.dest_vol.uuid.clone();
        let label = state.dest_vol.label.clone();
        let fs_type = state.dest_vol.fs_type.clone();
        let (volume, folders): (VolumeId, HashMap<String, FolderId>) = inner.with_catalog(|c| {
            let volume = c.ensure_volume(&uuid, label.as_deref(), Some(&mount))?;
            c.note_volume_seen(volume, &mount, label.as_deref(), fs_type.as_deref())?;
            let mut map = HashMap::new();
            for rel in rels {
                map.insert(rel.to_owned(), c.ensure_folder(volume, rel)?);
            }
            Ok((volume, map))
        })?;
        state.dest_volume = Some(volume);
        let mut requests: Vec<RegisterFile> = pending
            .placed
            .iter()
            .map(|p| {
                let mut r = p.source.request.clone();
                r.folder_id = folders[&p.dest_rel_dir];
                r.name.clone_from(&p.dest_name);
                r.facts = p.dest_facts.clone().expect("公開したファイルの内容");
                r
            })
            .collect();
        let hashes: Vec<Option<String>> =
            pending.placed.iter().map(|p| p.full_hash.clone()).collect();
        let outcomes = register_requests(
            inner,
            &mut requests,
            register_options,
            tags,
            &hashes,
            report,
        )?;
        Ok((requests, outcomes))
    })();
    let (requests, outcomes) = match registered {
        Ok(v) => v,
        Err(e) => {
            let n = pending.placed.iter().filter(|p| !p.existing).count();
            rollback_pending(pending);
            inner.events.emit(crate::events::Event::Warning {
                code: crate::events::WarningCode::ImportCopyRolledBack,
                message: format!(
                    "カタログに登録できなかったため、このバッチでコピーした {n} 件のファイルを消しました（元のファイルはそのままです）: {e}"
                ),
                variant_id: None,
                path: None,
            });
            return Err(e);
        }
    };
    // ここから先の失敗では、このバッチのコピーを消さない（カタログに登録済み。消すとカタログに行が残り、
    // ファイルがない状態になる）。呼び出し側の後始末（`rollback_pending`）の対象からも外す。
    let placed = std::mem::take(&mut pending.placed);
    let copy = report.copy.as_mut().expect("コピー取り込みの結果");
    for (p, out) in placed.iter().zip(&outcomes) {
        seen.set_registered(
            p.source.cand.path.clone(),
            out.master_variant_id,
            out.file_id,
            out.revision,
        );
        if p.dated_by_file_time {
            copy.dated_by_file_time += 1;
        }
        copy.bytes_copied += p.bytes;
        copy.copied.push(CopiedFile {
            source: p.source.cand.path.clone(),
            destination: p.dest_path.clone(),
            existing: p.existing,
        });
    }
    for (req, p) in requests.iter().zip(&placed) {
        if let Some(reason) = &req.error {
            report.errors.push(FileIssue {
                path: p.dest_path.clone(),
                reason: reason.clone(),
            });
        }
    }
    let paths: Vec<&Path> = placed.iter().map(|p| p.dest_path.as_path()).collect();
    let mut thumbs: Vec<ThumbTask> = Vec::new();
    let tallied = (|| -> Result<_, ApiError> {
        if let Some(hook) = &inner.on_copy_import {
            hook(&CopyImportStage::Registered(placed.len())).map_err(ApiError::Internal)?;
        }
        tally_outcomes(
            inner,
            &requests,
            &outcomes,
            &paths,
            report,
            &mut thumbs,
            regenerate,
        )
    })();
    let changed = match tallied {
        Ok(changed) => changed,
        Err(e) => {
            // 登録した写真は一覧に出す（サムネイルは、表示のときに作る）。
            tracing::warn!(
                files = placed.len(),
                error = %e,
                "コピー取り込み: 登録の後の処理に失敗しました（登録したコピーは残します）"
            );
            announce_changes(
                inner,
                outcomes
                    .iter()
                    .filter(|o| o.status != genzo_catalog::RegisterStatus::Unchanged)
                    .map(|o| o.master_variant_id)
                    .collect(),
            );
            return Err(e);
        }
    };
    announce_changes(inner, changed);
    make_thumbnails(ctx, threads, &thumbs, report, no_embedded);
    Ok(())
}

impl crate::core::Core {
    /// コピー取り込みの設定の、名前の例（IMP-03。決まった見本の写真の、コピー先のフォルダからの相対パス。
    /// 設定の誤りは [`crate::types::NamePreview::error`]）。カタログ・ファイルは使わない。
    pub fn import_name_preview(&self, settings: &CopySettings) -> crate::types::NamePreview {
        crate::naming::preview_names(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// 古い一時ファイルだけを消す（新しいもの・名前の形の違うものは消さない）。
    #[test]
    fn stale_temp_files_are_removed_by_age() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join(format!("{TEMP_PREFIX}old{TEMP_SUFFIX}"));
        let fresh = d.path().join(format!("{TEMP_PREFIX}new{TEMP_SUFFIX}"));
        let other = d.path().join("DSC0001.ARW");
        for p in [&old, &fresh, &other] {
            write(p, b"x");
        }
        let past = SystemTime::now() - STALE_TEMP_AGE - Duration::from_secs(60);
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(past)
            .unwrap();
        File::options()
            .write(true)
            .open(&other)
            .unwrap()
            .set_modified(past)
            .unwrap();
        remove_stale_temp_files(d.path());
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(other.exists());
    }

    /// コピー先が取り込むフォルダの中にあるか（同じフォルダ・外は含まない）。
    #[test]
    fn destination_inside_the_source() {
        let d = tempfile::tempdir().unwrap();
        let card = d.path().join("card");
        fs::create_dir_all(card.join("imported")).unwrap();
        assert!(is_inside(&card.join("imported"), &card));
        assert!(!is_inside(&card, &card));
        assert!(
            !is_inside(d.path(), &card),
            "取り込むフォルダがコピー先の中"
        );
        assert!(
            !is_inside(&d.path().join("cardboard"), &card),
            "名前の前方一致ではない"
        );
    }

    /// 公開した直後の内容と違うファイルは消さない。
    #[test]
    fn own_copies_are_removed_only_when_unchanged() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.jpg");
        write(&p, b"first");
        let facts = FileFacts::read(&p).unwrap();
        write(&p, b"second, longer");
        assert!(!remove_own_copy(&p, &facts));
        assert!(p.exists());
        let facts = FileFacts::read(&p).unwrap();
        assert!(remove_own_copy(&p, &facts));
        assert!(!p.exists());
    }

    /// コピーは一時ファイルを経て公開し、内容・更新日時が元と同じ。同じ名前があれば上書きしない。
    #[test]
    fn verified_copy_publishes_without_overwriting() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src/a.bin");
        let bytes: Vec<u8> = (0..(COPY_BUFFER * 2 + 5))
            .map(|i| (i % 253) as u8)
            .collect();
        write(&src, &bytes);
        let past = SystemTime::now() - Duration::from_secs(86_400);
        File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(past)
            .unwrap();
        let facts = FileFacts::read(&src).unwrap();
        let dest_dir = d.path().join("dest");
        fs::create_dir_all(&dest_dir).unwrap();
        let dest = dest_dir.join("b.bin");
        let inner_less = |dest: &Path| copy_plain(&src, &facts, past, dest);
        let info = inner_less(&dest).unwrap();
        assert_eq!(info.bytes, bytes.len() as u64);
        assert_eq!(fs::read(&dest).unwrap(), bytes);
        assert_eq!(info.full_hash, genzo_catalog::full_hash(&src).unwrap());
        assert_eq!(
            info.facts.mtime_ns, facts.mtime_ns,
            "更新日時を元にそろえる"
        );
        // 既にあるファイルは上書きしない。
        write(&dest_dir.join("c.bin"), b"keep");
        assert!(matches!(
            inner_less(&dest_dir.join("c.bin")),
            Err(CopyFailure::Exists)
        ));
        assert_eq!(fs::read(dest_dir.join("c.bin")).unwrap(), b"keep");
        // 一時ファイルは残らない。
        let names: Vec<String> = fs::read_dir(&dest_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| !n.starts_with(TEMP_PREFIX)),
            "{names:?}"
        );
        // 元のファイルは変わらない（DATA-01）。
        assert_eq!(fs::read(&src).unwrap(), bytes);
    }

    /// フックなしで [`copy_verified_with`] を呼ぶ。
    fn copy_plain(
        source: &Path,
        facts: &FileFacts,
        mtime: SystemTime,
        dest: &Path,
    ) -> Result<CopiedInfo, CopyFailure> {
        copy_verified_with(None, source, facts, mtime, dest, &CancellationToken::new())
    }

    /// RAW と JPEG の組（同じフォルダ・名前の比較キー）と、撮影日時の順。
    #[test]
    fn groups_pair_raw_and_jpeg_and_sort_by_time() {
        let at = |s: &str| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap();
        let src = |dir: &str, name: &str, when: &str| Source {
            cand: Candidate {
                path: PathBuf::from(format!("/sd/{dir}/{name}")),
                dir_rel: dir.to_owned(),
                name: name.to_owned(),
                kind: genzo_model::AssetKind::Photo,
            },
            facts: FileFacts {
                size: 1,
                mtime_ns: 0,
                quick_hash: "0".repeat(64),
            },
            mtime: SystemTime::UNIX_EPOCH,
            full_hash: None,
            duplicate_of: None,
            request: RegisterFile {
                folder_id: FolderId::new(0),
                name: name.to_owned(),
                facts: FileFacts {
                    size: 1,
                    mtime_ns: 0,
                    quick_hash: "0".repeat(64),
                },
                kind: genzo_model::AssetKind::Photo,
                metadata: MediaMetadata::None,
                capture: genzo_model::CaptureTime::unknown(),
                error: None,
            },
            when: at(when),
            by_file_time: false,
            camera: None,
        };
        let groups = make_groups(vec![
            src("a", "DSC2.JPG", "2026-01-01 10:00:02"),
            src("a", "DSC1.ARW", "2026-01-01 10:00:01"),
            src("a", "dsc1.jpg", "2026-01-01 10:00:01"),
            src("a", "DSC2.ARW", "2026-01-01 10:00:02"),
            src("b", "DSC1.JPG", "2026-01-01 09:00:00"),
            src("a", "DSC3.JPG", "2026-01-01 10:00:03"),
        ]);
        let names: Vec<Vec<&str>> = groups
            .iter()
            .map(|g| g.members.iter().map(|m| m.cand.name.as_str()).collect())
            .collect();
        assert_eq!(
            names,
            vec![
                vec!["DSC1.JPG"],
                vec!["DSC1.ARW", "dsc1.jpg"],
                vec!["DSC2.ARW", "DSC2.JPG"],
                vec!["DSC3.JPG"],
            ]
        );
    }
}
