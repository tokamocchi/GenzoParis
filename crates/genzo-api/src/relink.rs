//! 見つからない写真の検出と再リンク（FILE-04）・フォルダの場所の指定し直し。04 の 3.3 節・3.14 節。
//!
//! - **見つからない写真の確認**（[`Core::check_missing`]。ジョブの種類 `check_missing`）: フォルダごとに、
//!   フォルダとファイルが今の場所にあるかを確かめ、ないものに見つからない印（`file.status = missing`・
//!   `folder.status = missing`）を付け、戻っていたものの印を外す（内容が変わっていれば取り込みと同じ経路で
//!   読み直す。`library::refresh_file`）。**つながっていないボリュームのファイルは確かめない**（ドライブを
//!   外しただけのファイルに印を付けない）。内容のハッシュの計算と同じ 1 本のスレッド（`Inner::maintenance`）で、
//!   対話の処理の合間に進める。アプリは、カタログを開いたときと、ボリュームがつながったときに自動で動かす
//!   （[`crate::CoreConfig::background_file_check`]。自動の確認はネットワークのボリュームを飛ばす）。
//! - **再リンクの候補**（[`Core::find_relink_candidates`]。ジョブ）: 利用者が選んだフォルダ（配下を含む）の
//!   写真・動画のファイルから、見つからないファイルと同じものを探す。一致の仕方（確からしい順。`RelinkMatch`）:
//!   全体のハッシュ（記録があれば。ORG-03）が同じ ＞ 大きさとクイックハッシュが同じ（全体のハッシュの記録が
//!   ないとき）＞ 名前（大文字・小文字と正規化を区別しない）と撮影日時（元の文字列）が同じで内容が違う
//!   （ほかのソフトで書き換えた）。カタログに登録済みのファイルは候補にしない。
//! - **再リンク**（[`Core::relink_files`]）: 利用者が確かめた候補に結び付け直す（カタログの中だけ。ファイルには
//!   触れない）。結び付ける直前に、内容が登録と同じか（大きさ・更新日時・クイックハッシュ、全体のハッシュの記録が
//!   あれば全体）を確かめ、違えば、利用者が「内容が変わっていても結び付ける」を選んだときだけ結び付けて
//!   読み直す。新しい場所のフォルダはカタログに登録する。
//! - **フォルダの場所の指定し直し**（[`Core::relocate_folder`]）: アプリの外で移動・名前の変更をしたフォルダを、
//!   新しい場所に結び付ける（配下のフォルダも。カタログの中だけ）。新しい場所が別のフォルダとして登録済み
//!   なら断る。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use genzo_catalog::{FileFacts, FileRecord, text::path_key};
use genzo_jobs::CancellationToken;
use genzo_model::{AssetId, FileId, FileStatus, FolderId, FolderStatus, VariantId, VolumeId};
use genzo_worker::Lane;

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::{CatalogChange, Event};
use crate::identity::{HashFailure, full_hash_cancellable, wait_for_interactive};
use crate::import::{ProbeTarget, media_kind, probe_request, scan};
use crate::jobs::{JobCtx, JobQueue, spawn_job_on};
use crate::library::{Refreshed, refresh_file};
use crate::paths::{absolute_lexical, join_rel};
use crate::types::{
    FileIssue, JobKind, JobResult, MissingCheckReport, MovedFile, RelinkCandidate, RelinkMatch,
    RelinkReport, RelinkRequest, RelinkSearch, RelinkTarget, RelocatePlan,
};

/// 再リンクの候補を探す見つからないファイルの上限（**仮置き**: 10,000。写真を選ばずに「すべての見つからない
/// 写真」で探すとき）。
pub const MAX_RELINK_TARGETS: usize = 10_000;

/// 1 つのファイルに挙げる候補の上限（**仮置き**: 10）。
pub const MAX_RELINK_CANDIDATES: usize = 10;

/// 確認の進捗を知らせる間隔（ファイルの数）。
const CHECK_PROGRESS_EVERY: u64 = 64;

/// 見つからない写真の確認のジョブの状態（`Inner::check`）。
#[derive(Debug, Default)]
pub(crate) struct CheckJobState {
    job: Option<u64>,
}

/// アプリが許していれば、すべてのファイルの確認を頼む（実行中なら何もしない）。
pub(crate) fn request_background_check(inner: &Arc<Inner>) -> Option<u64> {
    inner
        .config
        .background_file_check
        .then(|| request_check(inner, None, false))
}

/// 確認を頼む（すべてのファイルの確認が実行中なら、そのジョブを返す）。
fn request_check(inner: &Arc<Inner>, scope: Option<FolderId>, include_network: bool) -> u64 {
    let mut st = inner.check.lock();
    if scope.is_none()
        && let Some(id) = st.job
        && inner
            .jobs
            .info(id)
            .is_some_and(|info| !info.state.is_finished())
    {
        return id;
    }
    let id = spawn_job_on(
        inner,
        JobQueue::Maintenance,
        JobKind::CheckMissing,
        "見つからない写真の確認",
        move |ctx| {
            let r = run_check(ctx, scope, include_network);
            let mut st = ctx.inner.check.lock();
            if st.job == Some(ctx.id) {
                st.job = None;
            }
            drop(st);
            r.map(JobResult::CheckMissing)
        },
    );
    if scope.is_none() {
        st.job = Some(id);
    }
    id
}

/// 変わった写真のファイルの状態を知らせる（検索結果を作り直す）。`changed` は内容が変わって読み直した asset
/// （L0 / L1 を作り直す）。
fn announce_status(inner: &Arc<Inner>, assets: &[AssetId], changed: &[AssetId]) {
    if assets.is_empty() {
        return;
    }
    let mut variants: Vec<VariantId> = Vec::new();
    for &a in assets {
        if let Ok(list) = inner.with_catalog(|c| c.variants_of_asset(a)) {
            variants.extend(list.into_iter().map(|v| v.id));
        }
    }
    variants.sort();
    variants.dedup();
    inner.events.emit(Event::CatalogChanged {
        change: CatalogChange::FileStatus,
        variant_ids: variants,
        all: false,
    });
    crate::search::refresh(inner, &[]);
    for &a in changed {
        if let Ok(list) = inner.with_catalog(|c| c.variants_of_asset(a)) {
            for v in list {
                crate::previews::schedule_regenerate(inner, v.id);
            }
        }
    }
}

/// 確認のジョブの本体（モジュールの doc）。
fn run_check(
    ctx: &JobCtx<'_>,
    scope: Option<FolderId>,
    include_network: bool,
) -> Result<MissingCheckReport, ApiError> {
    let inner = ctx.inner;
    let mut report = MissingCheckReport::default();
    crate::volumes::refresh(inner)?;
    let states = inner.volume_states.lock().clone();
    let folders: Vec<(FolderId, VolumeId, String, FolderStatus)> = inner.with_catalog(|c| {
        let only: Option<HashSet<FolderId>> = match scope {
            Some(f) => Some(c.folder_subtree(f)?.into_iter().collect()),
            None => None,
        };
        Ok(c.folder_records()?
            .into_iter()
            .filter(|r| only.as_ref().is_none_or(|o| o.contains(&r.folder.id)))
            .map(|r| (r.folder.id, r.folder.volume_id, r.folder.rel_path, r.status))
            .collect())
    })?;
    let total: u64 = inner.with_catalog(|c| {
        let mut n = 0u64;
        for (id, ..) in &folders {
            n += c.files_in_folder(*id)?.len() as u64;
        }
        Ok(n)
    })?;
    let mut done = 0u64;
    ctx.progress(0, total);
    let mut folders_changed = false;
    let mut network_cache: HashMap<VolumeId, bool> = HashMap::new();
    for (folder_id, volume_id, rel, status) in folders {
        if ctx.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let (loc, files) = inner
            .with_catalog(|c| Ok((c.folder_location(folder_id)?, c.files_in_folder(folder_id)?)))?;
        let Some(mount) = loc.mount_path.as_deref() else {
            report.skipped_offline += files.len() as u64;
            done += files.len() as u64;
            continue;
        };
        let dir = join_rel(Path::new(mount), &rel, "");
        if !include_network {
            let network = *network_cache.entry(volume_id).or_insert_with(|| {
                states.get(&volume_id).is_some_and(|s| s.network)
                    || crate::paths::is_network_path(Path::new(mount))
            });
            if network {
                report.skipped_network += files.len() as u64;
                done += files.len() as u64;
                continue;
            }
        }
        let dir_state = std::fs::metadata(&dir);
        let dir_missing = match &dir_state {
            Ok(m) => !m.is_dir(),
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };
        if dir_state.is_err() && !dir_missing {
            // あるかないか分からない（権限など）: 確かめない。
            done += files.len() as u64;
            continue;
        }
        let want = if dir_missing {
            FolderStatus::Missing
        } else {
            FolderStatus::Ok
        };
        if want != status {
            inner.with_catalog(|c| c.set_folder_status(&[folder_id], want))?;
            folders_changed = true;
        }
        if dir_missing {
            report.missing_folders += 1;
        }
        let mut marked: Vec<AssetId> = Vec::new();
        let mut changed: Vec<AssetId> = Vec::new();
        for rec in files {
            if ctx.is_cancelled() {
                report.cancelled = true;
                break;
            }
            report.checked += 1;
            done += 1;
            if done.is_multiple_of(CHECK_PROGRESS_EVERY) {
                ctx.progress(done, total);
            }
            if dir_missing {
                if rec.status != FileStatus::Missing {
                    inner.with_catalog(|c| {
                        c.set_file_status(
                            rec.id,
                            FileStatus::Missing,
                            Some("フォルダが見つかりません"),
                        )
                    })?;
                    report.newly_missing += 1;
                    marked.push(rec.asset_id);
                }
                continue;
            }
            wait_for_interactive(inner, ctx.token());
            let path = dir.join(&rec.name);
            match std::fs::metadata(&path) {
                Ok(m) if m.is_file() => {
                    if rec.status == FileStatus::Missing {
                        match refresh_file(inner, &rec, &path, Lane::Batch, ctx.token()) {
                            Ok(Refreshed::Restored) => {
                                report.restored += 1;
                                marked.push(rec.asset_id);
                            }
                            Ok(Refreshed::Changed) => {
                                report.restored += 1;
                                marked.push(rec.asset_id);
                                changed.push(rec.asset_id);
                            }
                            Ok(_) => {}
                            Err(e) if e.is_cancelled() => {
                                report.cancelled = true;
                                break;
                            }
                            Err(e) => {
                                tracing::warn!(path = %path.display(), error = %e, "戻ったファイルを確かめられません");
                            }
                        }
                    }
                }
                Ok(_) => {
                    if rec.status != FileStatus::Missing {
                        inner.with_catalog(|c| {
                            c.set_file_status(
                                rec.id,
                                FileStatus::Missing,
                                Some("ファイルが見つかりません"),
                            )
                        })?;
                        report.newly_missing += 1;
                        marked.push(rec.asset_id);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if rec.status != FileStatus::Missing {
                        inner.with_catalog(|c| {
                            c.set_file_status(
                                rec.id,
                                FileStatus::Missing,
                                Some("ファイルが見つかりません"),
                            )
                        })?;
                        report.newly_missing += 1;
                        marked.push(rec.asset_id);
                    }
                }
                // 権限などで分からない: 状態を変えない。
                Err(_) => {}
            }
        }
        marked.sort();
        marked.dedup();
        announce_status(inner, &marked, &changed);
        if !marked.is_empty() {
            folders_changed = true;
        }
        if report.cancelled {
            break;
        }
    }
    report.missing = inner.with_catalog(|c| {
        let only: Option<Vec<FolderId>> = match scope {
            Some(f) => Some(c.folder_subtree(f)?),
            None => None,
        };
        Ok(match only {
            Some(ids) => c.missing_files(Some(&ids), None, usize::MAX)?.len() as u64,
            None => c.missing_files(None, None, usize::MAX)?.len() as u64,
        })
    })?;
    ctx.progress(total, total);
    if folders_changed {
        crate::fileops::announce_folders(inner);
    }
    Ok(report)
}

/// 見つからないファイルの候補を探すための情報。
struct Target {
    rec: FileRecord,
    variant_id: VariantId,
    last_path: Option<PathBuf>,
    capture_raw: Option<String>,
}

/// 走査したファイル 1 つ（読んだ情報を覚える）。
struct Scanned {
    path: PathBuf,
    name: String,
    size: u64,
    facts: Option<Option<FileFacts>>,
    full_hash: Option<Option<String>>,
    capture_raw: Option<Option<String>>,
    registered: Option<bool>,
}

/// 候補の探索のジョブの本体（モジュールの doc）。
fn run_find(
    ctx: &JobCtx<'_>,
    variant_ids: Option<Vec<VariantId>>,
    search_dir: PathBuf,
    recursive: bool,
) -> Result<RelinkSearch, ApiError> {
    let inner = ctx.inner;
    let token = ctx.token();
    let mut out = RelinkSearch {
        search_dir: search_dir.clone(),
        ..Default::default()
    };
    // 見つからないファイル。
    let targets: Vec<Target> = inner.with_catalog(|c| {
        let records: Vec<FileRecord> = match &variant_ids {
            Some(ids) => {
                let mut v = Vec::new();
                for a in c.assets_of_variants(ids)? {
                    v.extend(
                        c.files_of_asset(a)?
                            .into_iter()
                            .filter(|f| f.status == FileStatus::Missing),
                    );
                }
                v
            }
            None => c.missing_files(None, None, MAX_RELINK_TARGETS)?,
        };
        let mut out = Vec::new();
        for rec in records {
            let variant_id = c.master_variant_of(rec.asset_id)?;
            let last_path = c.file_location(rec.id)?.absolute_path();
            let capture_raw = c.capture_time(rec.asset_id)?.raw;
            out.push(Target {
                rec,
                variant_id,
                last_path,
                capture_raw,
            });
        }
        Ok(out)
    })?;
    // 走査。
    let excluded = crate::import::excluded_dirs(inner);
    let found = scan(&search_dir, "", recursive, &excluded, token);
    out.unreadable = found.unreadable;
    let mut files: Vec<Scanned> = Vec::new();
    for c in found.files {
        let Ok(m) = std::fs::metadata(&c.path) else {
            continue;
        };
        files.push(Scanned {
            path: c.path,
            name: c.name,
            size: m.len(),
            facts: None,
            full_hash: None,
            capture_raw: None,
            registered: None,
        });
    }
    out.scanned_files = files.len() as u64;
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, f) in files.iter().enumerate() {
        by_size.entry(f.size).or_default().push(i);
        by_name.entry(path_key(&f.name)).or_default().push(i);
    }
    let volumes: HashMap<String, VolumeId> = inner.with_catalog(|c| {
        Ok(c.volume_records()?
            .into_iter()
            .map(|v| (v.uuid, v.id))
            .collect())
    })?;
    let total = targets.len() as u64;
    ctx.progress(0, total);
    let pool = inner.pool()?;
    for (n, t) in targets.into_iter().enumerate() {
        if ctx.is_cancelled() {
            out.cancelled = true;
            break;
        }
        let key = path_key(&t.rec.name);
        let mut cands: Vec<(usize, RelinkMatch)> = Vec::new();
        for &i in by_size
            .get(&t.rec.facts.size)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let f = &mut files[i];
            let facts = f
                .facts
                .get_or_insert_with(|| FileFacts::read(&f.path).ok())
                .clone();
            let Some(facts) = facts else {
                continue;
            };
            if facts.quick_hash != t.rec.facts.quick_hash {
                continue;
            }
            match &t.rec.full_hash {
                Some(expected) => {
                    if f.full_hash.is_none() {
                        f.full_hash = Some(match full_hash_cancellable(&f.path, token) {
                            Ok(h) => Some(h),
                            Err(HashFailure::Cancelled) => {
                                out.cancelled = true;
                                break;
                            }
                            Err(HashFailure::Io(_)) => None,
                        });
                    }
                    if f.full_hash.as_ref().and_then(Option::as_deref) == Some(expected.as_str()) {
                        cands.push((i, RelinkMatch::Content));
                    }
                }
                None => cands.push((i, RelinkMatch::Quick)),
            }
        }
        if out.cancelled {
            break;
        }
        if let Some(raw) = &t.capture_raw {
            for &i in by_name.get(&key).map(Vec::as_slice).unwrap_or(&[]) {
                if cands.iter().any(|(j, _)| *j == i) {
                    continue;
                }
                let f = &mut files[i];
                if f.capture_raw.is_none() {
                    let facts = f
                        .facts
                        .get_or_insert_with(|| FileFacts::read(&f.path).ok())
                        .clone();
                    f.capture_raw = Some(facts.and_then(|facts| {
                        let target = ProbeTarget {
                            path: &f.path,
                            kind: media_kind(&f.name).unwrap_or(genzo_model::AssetKind::Photo),
                            folder: t.rec.folder_id,
                            name: &f.name,
                        };
                        probe_request(
                            &pool,
                            &target,
                            facts,
                            inner.default_offset(),
                            Lane::Batch,
                            token,
                        )
                        .ok()
                        .and_then(|r| r.capture.raw)
                    }));
                }
                if f.capture_raw.as_ref().and_then(Option::as_deref) == Some(raw.as_str()) {
                    cands.push((i, RelinkMatch::NameAndTime));
                }
            }
        }
        // カタログに登録済みのファイルは候補にしない（ほかの写真のファイル）。
        cands.retain(|(i, _)| {
            let f = &mut files[*i];
            let registered = *f
                .registered
                .get_or_insert_with(|| is_registered(inner, &volumes, &f.path).unwrap_or(true));
            !registered
        });
        cands.sort_by_key(|(i, m)| (*m, path_key(&files[*i].name) != key, *i));
        cands.truncate(MAX_RELINK_CANDIDATES);
        out.targets.push(RelinkTarget {
            file_id: t.rec.id,
            variant_id: t.variant_id,
            role: t.rec.role,
            name: t.rec.name.clone(),
            last_path: t.last_path,
            size: t.rec.facts.size,
            candidates: cands
                .into_iter()
                .map(|(i, m)| RelinkCandidate {
                    path: files[i].path.clone(),
                    matched: m,
                    size: files[i].size,
                    same_name: path_key(&files[i].name) == key,
                })
                .collect(),
        });
        ctx.progress(n as u64 + 1, total);
    }
    Ok(out)
}

/// `path` のファイルがカタログに登録済みか（ボリューム・フォルダ・名前で探す）。
fn is_registered(
    inner: &Inner,
    volumes: &HashMap<String, VolumeId>,
    path: &Path,
) -> Result<bool, ApiError> {
    let vp = crate::volumes::locate(inner.volume_probe.as_ref(), path)?;
    let Some(&volume) = volumes.get(&vp.uuid) else {
        return Ok(false);
    };
    let (dir, name) = match vp.rel.rsplit_once('/') {
        Some((d, n)) => (d.to_owned(), n.to_owned()),
        None => (String::new(), vp.rel.clone()),
    };
    inner.with_catalog(|c| match c.find_folder(volume, &dir)? {
        Some(folder) => Ok(c.find_file(folder, &name)?.is_some()),
        None => Ok(false),
    })
}

/// 見つからないファイルを、結び付ける前に確かめた内容。
enum Content {
    /// 登録と同じ（全体のハッシュを求めたら、その値）。
    Same(Option<String>),
    /// 違う。
    Changed,
}

fn compare_content(
    path: &Path,
    rec: &FileRecord,
    token: &CancellationToken,
) -> Result<Content, String> {
    let now = FileFacts::read(path).map_err(|e| format!("ファイルを読めません: {e}"))?;
    if now == rec.facts {
        return Ok(Content::Same(None));
    }
    if now.size != rec.facts.size || now.quick_hash != rec.facts.quick_hash {
        return Ok(Content::Changed);
    }
    match &rec.full_hash {
        Some(expected) => match full_hash_cancellable(path, token) {
            Ok(h) if h == *expected => Ok(Content::Same(Some(h))),
            Ok(_) => Ok(Content::Changed),
            Err(HashFailure::Cancelled) => Err("取り消しました".to_owned()),
            Err(HashFailure::Io(m)) => Err(m),
        },
        // 全体のハッシュの記録がない: 大きさとクイックハッシュが同じなら同じとみなす（更新日時だけが違う）。
        None => Ok(Content::Same(None)),
    }
}

impl Core {
    /// 見つからない写真を確かめる（ジョブ。`folder` を与えれば、そのフォルダと配下だけ。ネットワークの
    /// ボリュームも確かめる）。結果は [`JobResult::CheckMissing`]。モジュールの doc。
    pub fn check_missing(&self, folder: Option<FolderId>) -> Result<u64, ApiError> {
        self.inner.check_open()?;
        if let Some(f) = folder {
            self.inner.with_catalog(|c| c.folder(f))?;
        }
        Ok(request_check(&self.inner, folder, true))
    }

    /// 再リンクの候補を探す（ジョブ。`variant_ids` の写真の見つからないファイル。`None` ならすべての見つからない
    /// ファイル（[`MAX_RELINK_TARGETS`] まで）。`search_dir` の配下（`recursive`）から探す）。結果は
    /// [`JobResult::RelinkCandidates`]。モジュールの doc。
    pub fn find_relink_candidates(
        &self,
        variant_ids: Option<Vec<VariantId>>,
        search_dir: &Path,
        recursive: bool,
    ) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let dir = absolute_lexical(search_dir)?;
        if !dir.is_dir() {
            return Err(ApiError::InvalidArgument(format!(
                "フォルダではありません: {}",
                dir.display()
            )));
        }
        crate::volumes::refresh(inner)?;
        Ok(crate::jobs::spawn_job(
            inner,
            JobKind::FindRelinkCandidates,
            "再リンクの候補の探索",
            move |ctx| run_find(ctx, variant_ids, dir, recursive).map(JobResult::RelinkCandidates),
        ))
    }

    /// 見つからないファイルを、利用者が確かめた場所に結び付け直す（カタログの中だけ）。モジュールの doc。
    pub fn relink_files(&self, requests: &[RelinkRequest]) -> Result<RelinkReport, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        crate::fileops::ensure_no_conflicting_jobs(inner, None)?;
        let _ops = inner.file_ops.lock();
        // 排他を取った後にも確かめる（取り込みは、この排他を持ったまま取り込みのジョブを登録する）。
        crate::fileops::ensure_no_conflicting_jobs(inner, None)?;
        crate::volumes::refresh(inner)?;
        let token = CancellationToken::new();
        let mut report = RelinkReport::default();
        let mut assets: Vec<AssetId> = Vec::new();
        let mut changed: Vec<AssetId> = Vec::new();
        let mut seen: HashSet<FileId> = HashSet::new();
        for req in requests {
            if !seen.insert(req.file_id) {
                continue;
            }
            match self.relink_one(req, &token) {
                Ok((moved, reread)) => {
                    let rec = inner.with_catalog(|c| c.file(req.file_id))?;
                    assets.push(rec.asset_id);
                    if reread {
                        changed.push(rec.asset_id);
                        report.reread.push(moved.to.clone());
                    }
                    report.linked.push(moved);
                }
                Err(reason) => report.failed.push(FileIssue {
                    path: req.path.clone(),
                    reason,
                }),
            }
        }
        assets.sort();
        assets.dedup();
        announce_status(inner, &assets, &changed);
        if !report.linked.is_empty() {
            crate::fileops::announce_folders(inner);
        }
        Ok(report)
    }

    /// 1 つのファイルを結び付け直す。動かした場所と、読み直したかを返す。
    fn relink_one(
        &self,
        req: &RelinkRequest,
        token: &CancellationToken,
    ) -> Result<(MovedFile, bool), String> {
        let inner = &self.inner;
        let rec = inner
            .with_catalog(|c| c.file(req.file_id))
            .map_err(|e| e.to_string())?;
        if rec.status != FileStatus::Missing {
            return Err("見つかっているファイルは結び付け直しません".to_owned());
        }
        let last = inner
            .with_catalog(|c| c.file_location(rec.id))
            .ok()
            .and_then(|l| l.absolute_path())
            .unwrap_or_default();
        let path = absolute_lexical(&req.path).map_err(|e| e.to_string())?;
        if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
            return Err("ファイルが見つかりません".to_owned());
        }
        let content = compare_content(&path, &rec, token)?;
        let same_hash = match content {
            Content::Same(h) => h,
            Content::Changed if req.accept_changed => None,
            Content::Changed => {
                return Err(
                    "内容が登録のものと違います（内容が変わっていても結び付けるときは、確かめてから選んでください）"
                        .to_owned(),
                );
            }
        };
        let vp = crate::volumes::locate(inner.volume_probe.as_ref(), &path)
            .map_err(|e| e.to_string())?;
        let (dir_rel, name) = match vp.rel.rsplit_once('/') {
            Some((d, n)) => (d.to_owned(), n.to_owned()),
            None => (String::new(), vp.rel.clone()),
        };
        let mount = crate::volumes::mount_str(&vp.mount).map_err(|e| e.to_string())?;
        inner
            .with_catalog(|c| {
                let volume = c.ensure_volume(&vp.uuid, vp.label.as_deref(), Some(&mount))?;
                c.note_volume_seen(volume, &mount, vp.label.as_deref(), vp.fs_type.as_deref())?;
                let folder = c.ensure_folder(volume, &dir_rel)?;
                c.relink_file(rec.id, folder, &name)
            })
            .map_err(|e| match e {
                ApiError::Catalog(genzo_catalog::CatalogError::Conflict(_)) => {
                    "その場所のファイルは、別の写真としてカタログに登録されています".to_owned()
                }
                other => other.to_string(),
            })?;
        let updated = inner
            .with_catalog(|c| c.file(rec.id))
            .map_err(|e| e.to_string())?;
        let reread = match refresh_file(inner, &updated, &path, Lane::Batch, token) {
            Ok(Refreshed::Changed) => true,
            Ok(_) => false,
            Err(e) => {
                // 読み直せなかった: 場所を元に戻す（失敗と報告するのに、カタログの場所だけが変わったままに
                // しない）。元の場所は見つからないファイルの場所なので、ほかのファイルとはぶつからない。
                let _ = inner.with_catalog(|c| c.relink_file(rec.id, rec.folder_id, &rec.name));
                return Err(format!("結び付けた後にファイルを確かめられません: {e}"));
            }
        };
        if let Some(h) = same_hash {
            let now = inner
                .with_catalog(|c| c.file(rec.id))
                .map_err(|e| e.to_string())?;
            if now.full_hash.is_none() {
                let _ = inner.with_catalog(|c| {
                    c.set_full_hash_if_current(now.id, now.revision, &now.facts, &h)
                });
            }
        }
        Ok((
            MovedFile {
                file_id: rec.id,
                from: last,
                to: path,
            },
            reread,
        ))
    }

    /// フォルダの場所の指定し直しの予定（新しい場所に、登録したファイルがいくつあるか。カタログもファイルも
    /// 変えない）。
    pub fn plan_relocate_folder(
        &self,
        folder: FolderId,
        new_path: &Path,
    ) -> Result<RelocatePlan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        crate::volumes::refresh(inner)?;
        relocate_plan(inner, folder, new_path)
    }

    /// フォルダの場所を指定し直す（カタログの中だけ。配下のフォルダも）。新しい場所で見つかったファイルの
    /// 見つからない印を外す（内容が変わっていれば読み直す）。指定し直した後の予定（見つかった数）を返す。
    pub fn relocate_folder(
        &self,
        folder: FolderId,
        new_path: &Path,
    ) -> Result<RelocatePlan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        crate::fileops::ensure_no_conflicting_jobs(inner, None)?;
        let _ops = inner.file_ops.lock();
        // 排他を取った後にも確かめる（取り込みは、この排他を持ったまま取り込みのジョブを登録する）。
        crate::fileops::ensure_no_conflicting_jobs(inner, None)?;
        crate::volumes::refresh(inner)?;
        let plan = relocate_plan(inner, folder, new_path)?;
        if let Some(p) = &plan.problem {
            return Err(ApiError::InvalidArgument(p.clone()));
        }
        let vp = crate::volumes::locate(inner.volume_probe.as_ref(), &plan.to)?;
        let mount = crate::volumes::mount_str(&vp.mount)?;
        let ids = inner.with_catalog(|c| {
            let volume = c.ensure_volume(&vp.uuid, vp.label.as_deref(), Some(&mount))?;
            c.note_volume_seen(volume, &mount, vp.label.as_deref(), vp.fs_type.as_deref())?;
            c.relocate_folder(folder, volume, &vp.rel)
        })?;
        // 新しい場所のファイルを確かめる（見つからない印を外す）。
        let token = CancellationToken::new();
        let mut assets = Vec::new();
        let mut changed = Vec::new();
        for id in ids {
            let (files, dir) = inner.with_catalog(|c| {
                let loc = c.folder_location(id)?;
                Ok((
                    c.files_in_folder(id)?,
                    loc.mount_path
                        .map(|m| join_rel(Path::new(&m), &loc.rel_dir, "")),
                ))
            })?;
            let Some(dir) = dir else {
                continue;
            };
            for rec in files {
                let path = dir.join(&rec.name);
                let exists = std::fs::metadata(&path).is_ok_and(|m| m.is_file());
                if exists && rec.status == FileStatus::Missing {
                    match refresh_file(inner, &rec, &path, Lane::Batch, &token) {
                        Ok(Refreshed::Changed) => {
                            assets.push(rec.asset_id);
                            changed.push(rec.asset_id);
                        }
                        Ok(_) => assets.push(rec.asset_id),
                        Err(e) => {
                            tracing::warn!(path = %path.display(), error = %e, "指定し直した場所のファイルを確かめられません")
                        }
                    }
                } else if !exists && rec.status != FileStatus::Missing {
                    inner.with_catalog(|c| {
                        c.set_file_status(
                            rec.id,
                            FileStatus::Missing,
                            Some("ファイルが見つかりません"),
                        )
                    })?;
                    assets.push(rec.asset_id);
                }
            }
        }
        assets.sort();
        assets.dedup();
        announce_status(inner, &assets, &changed);
        crate::fileops::announce_folders(inner);
        relocate_plan(inner, folder, &plan.to).map(|mut p| {
            p.problem = None;
            p
        })
    }
}

/// フォルダの場所の指定し直しの予定を作る。
fn relocate_plan(
    inner: &Inner,
    folder: FolderId,
    new_path: &Path,
) -> Result<RelocatePlan, ApiError> {
    let to = absolute_lexical(new_path)?;
    let vp = crate::volumes::locate(inner.volume_probe.as_ref(), &to)?;
    inner.with_catalog_api(|c| {
        let f = c.folder(folder)?;
        let loc = c.folder_location(folder)?;
        let from = loc
            .mount_path
            .as_deref()
            .map(|m| join_rel(Path::new(m), &loc.rel_dir, ""));
        let subtree = c.folder_subtree(folder)?;
        let mut plan = RelocatePlan {
            folder_id: folder,
            from,
            to: to.clone(),
            files: 0,
            found: 0,
            subfolders: subtree.len().saturating_sub(1) as u64,
            problem: None,
        };
        if f.rel_path.is_empty() {
            plan.problem = Some(
                "ボリュームのルートのフォルダは、場所を指定し直せません（配下のフォルダを選んでください）".to_owned(),
            );
        } else if !to.is_dir() {
            plan.problem = Some(format!("フォルダではありません: {}", to.display()));
        } else if vp.rel.is_empty() {
            plan.problem = Some("ドライブのルートには指定し直せません".to_owned());
        } else if let Some(volume) = c
            .volume_records()?
            .into_iter()
            .find(|v| v.uuid == vp.uuid)
            .map(|v| v.id)
            && let Some(existing) = c.find_folder(volume, &vp.rel)?
            && !subtree.contains(&existing)
        {
            plan.problem = Some(format!(
                "{} は、別のフォルダとしてカタログに登録されています（先にそちらのフォルダを「カタログから除く」で除いてください。写真が登録されていれば、先に写真をカタログから除くか、再リンクしてください）",
                to.display()
            ));
        }
        for id in &subtree {
            let sub = c.folder(*id)?;
            let rest = genzo_catalog::rel_path_below(&f.rel_path, &sub.rel_path);
            let dir = if rest.is_empty() {
                to.clone()
            } else {
                rest.split('/').fold(to.clone(), |p, part| p.join(part))
            };
            for rec in c.files_in_folder(*id)? {
                plan.files += 1;
                if std::fs::metadata(dir.join(&rec.name))
                    .is_ok_and(|m| m.is_file() && m.len() == rec.facts.size)
                {
                    plan.found += 1;
                }
            }
        }
        Ok(plan)
    })
}
