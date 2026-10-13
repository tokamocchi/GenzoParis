//! アプリの中でのファイル操作（FILE-02）: 写真の移動・名前の変更、フォルダの作成・名前の変更・移動。04 の
//! 6.4 節・3.14 節。
//!
//! - **状態遷移**: どれも `file_op` に予定を記録してから（planned → executing）OS の操作を行い、成功したら
//!   カタログを更新して done にする（genzo-catalog の `plan_file_moves`・`plan_folder_op`・
//!   `complete_file_op_entries`）。OS の操作が途中で失敗したら、動かしたものを元に戻して failed にする（戻せな
//!   かったものは、移動先のままカタログに反映する。カタログと実際のファイルを食い違わせない）。アプリが途中で
//!   終わったら、次に開いたときに実際のファイルの場所を見て確定する（[`settle`]。DATA-07）。
//! - **上書きしない**: 名前の変更は、移動先にあれば失敗する（genzo-media の `rename_no_replace`）。大文字・
//!   小文字だけの名前の変更は、同じフォルダの一時的な名前を経由する（`rename_case_safe`。一時的な名前のまま
//!   アプリが終わったときは、次に開いたときの確定で名前の変更を終わらせる）。
//! - **写真の移動**（[`Core::move_files`]。ジョブ）: 写真（asset）ごとに、すべてのファイル（RAW と同時記録の
//!   JPEG。IMP-06）を一緒に動かす。同じボリュームの中は名前の変更、別のボリュームへはコピー取り込みと同じ照合
//!   （一時ファイルに書いて BLAKE3 を照合してから上書きせずに公開。`copy_import` の `copy_verified_with`）をして
//!   からカタログを更新し、**元のファイルはゴミ箱へ移す**（ゴミ箱へ移せなければ元の場所に残し、結果と警告で
//!   知らせる。直接は削除しない）。移動先に同じ名前・拡張子だけが違う別の写真の RAW・JPEG（1 枚の写真にまとめ
//!   られるため。決定事項ログ No.100）があれば、その写真は飛ばす（名前を変えて置かない）。
//! - **写真の名前の変更**（[`Core::rename_files`]）: リネームの規則（IMP-03 のテンプレート。重なる名前には
//!   `-1` などを付ける）か、1 枚の写真の名前。同じ写真のファイルは同じ名前（拡張子は元のまま）にする。
//! - **フォルダ**: 作成（[`Core::create_folder`]）・名前の変更（[`Core::rename_folder`]）・移動
//!   （[`Core::move_folder`]。同じボリュームの中だけ。別のボリュームへは写真を選んで移す）。ボリュームのルート
//!   と、カタログ・アプリのデータのフォルダを含むフォルダは動かせない。配下のフォルダが、名前の書き方だけが違う
//!   別のフォルダにある（大文字・小文字・正規化を区別するボリューム）ときも動かさない。写真が登録されていない
//!   フォルダは、カタログから除ける（[`Core::remove_folder`]。カタログの中だけ。FILE-04 の片付け）。
//! - **同時に行わない**: 取り込みの実行中は始めない（取り込みが同じファイルを登録し直すのを避ける）。ファイル
//!   操作どうしは `Inner::file_ops` のロックで 1 つずつにする。
//!
//! 名前の規則は、取り込みのリネーム（IMP-03）と同じく、どの OS でも使える名前だけを受け付ける（使えない文字・
//! Windows の予約名・先頭の `.`・末尾の空白と `.` はエラー。黙って変えない）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use genzo_catalog::{
    AppliedEntry, Catalog, FileFacts, FileOpRecord, FileRecord, MoveRequest, PairClass, pair_class,
    text::path_key,
};
use genzo_jobs::CancellationToken;
use genzo_media::{case_rename_temp_path, rename_case_safe, rename_no_replace};
use genzo_model::{AssetId, FileId, FileOpKind, FileOpState, FolderId, VariantId};

use crate::copy_import::{CopyFailure, copy_verified_with, remove_own_copy};
use crate::core::{Core, FileOpStage, Inner};
use crate::error::ApiError;
use crate::events::{CatalogChange, Event, WarningCode};
use crate::jobs::{JobCtx, spawn_job};
use crate::naming::{
    MAX_COLLISION_SUFFIX, NameInput, join_extension, render_stem, safe_file_stem, split_extension,
    with_suffix,
};
use crate::paths::join_rel;
use crate::types::{
    FileIssue, FileMovePlan, FileMoveReport, FileRenamePlan, FolderInfo, JobKind, JobResult,
    MoveMethod, MovedFile, PlannedFileMove, PlannedPhotoMove, RenameSpec,
};

/// 利用者が入力した名前（ファイル名の拡張子を除く部分・フォルダの名前）を確かめる。どの OS でも使える名前
/// （取り込みのリネームと同じ規則。`naming::safe_file_stem` で変わらない名前）だけを受け付け、Unicode を NFC に
/// した名前を返す。
pub(crate) fn validate_name(name: &str) -> Result<String, ApiError> {
    let nfc = genzo_catalog::text::nfc(name);
    if nfc.trim().is_empty() {
        return Err(ApiError::InvalidArgument("名前が空です".to_owned()));
    }
    let safe = safe_file_stem(&nfc);
    if safe != nfc {
        return Err(ApiError::InvalidArgument(format!(
            "「{name}」は名前に使えません（使えない文字 < > : \" / \\ | ? * と制御文字、Windows の予約名、先頭の「.」、末尾の空白と「.」、200 バイトを超える長さは使えません。例: 「{safe}」）"
        )));
    }
    Ok(nfc)
}

/// 取り込み・写真の移動の実行中は、ファイル操作を始めない（モジュールの doc）。
pub(crate) fn ensure_no_conflicting_jobs(
    inner: &Inner,
    except: Option<u64>,
) -> Result<(), ApiError> {
    if inner.jobs.is_running(JobKind::Import, None) {
        return Err(ApiError::InvalidArgument(
            "取り込みの実行中は、ファイルの移動・名前の変更・フォルダの操作はできません。取り込みが終わってからやり直してください".to_owned(),
        ));
    }
    if inner.jobs.is_running(JobKind::MoveFiles, except) {
        return Err(ApiError::InvalidArgument(
            "写真の移動の実行中です。終わってからやり直してください".to_owned(),
        ));
    }
    Ok(())
}

/// つながっている、実在するフォルダの場所（つながっていない・見つからなければエラー）。
pub(crate) fn folder_dir(cat: &Catalog, folder: FolderId) -> Result<PathBuf, ApiError> {
    let loc = cat.folder_location(folder)?;
    let Some(mount) = loc.mount_path.as_deref() else {
        return Err(ApiError::FileAccess {
            path: PathBuf::new(),
            message: format!(
                "フォルダ {folder} のドライブがつながっていません（ドライブをつないでから、やり直してください）"
            ),
        });
    };
    let dir = join_rel(Path::new(mount), &loc.rel_dir, "");
    if !dir.is_dir() {
        return Err(ApiError::FileAccess {
            path: dir.clone(),
            message: format!(
                "フォルダが見つかりません: {}（アプリの外で移動・名前の変更をした場合は「場所を指定し直す」を使ってください）",
                dir.display()
            ),
        });
    }
    Ok(dir)
}

/// フォルダ `folder`（ディスクの `dir`）の配下のフォルダが、どれもディスクでも `dir` の中にあるかを確かめる
/// （フォルダの名前の変更・移動の前）。
///
/// 配下のフォルダの行は、親の部分を登録したときの綴りのまま持つ（カタログは綴り（大文字・小文字・正規化）
/// だけが違うフォルダを 1 つにまとめる。04 の 3.5 節）。大文字・小文字・正規化を区別するボリュームでは、その
/// 綴りのフォルダが `dir` とは別にディスクにあり、`dir` を動かしても一緒に動かない。そのまま動かすと、カタログ
/// だけが配下を新しい場所に付け替えて実際のファイルと食い違うので、動かさずにエラーにする（配下のフォルダが
/// 見つからないときは、どこにあっても食い違いは増えないので確かめない）。
fn check_subfolders_inside(
    cat: &Catalog,
    folder: FolderId,
    rel: &str,
    mount: &Path,
    dir: &Path,
) -> Result<(), ApiError> {
    for id in cat.folder_subtree(folder)?.into_iter().skip(1) {
        let sub = cat.folder(id)?;
        let stored = join_rel(mount, &sub.rel_path, "");
        let expected = genzo_catalog::rel_path_below(rel, &sub.rel_path)
            .split('/')
            .fold(dir.to_path_buf(), |p, part| p.join(part));
        if stored == expected || !stored.is_dir() {
            continue;
        }
        if !same_file::is_same_file(&stored, &expected).unwrap_or(false) {
            return Err(ApiError::FileAccess {
                path: stored.clone(),
                message: format!(
                    "配下のフォルダ {} は、名前の書き方（大文字・小文字・正規化）だけが違う別のフォルダにあり、一緒に動かせません（ディスクのフォルダの名前をそろえてから、やり直してください）",
                    stored.display()
                ),
            });
        }
    }
    Ok(())
}

/// フォルダ `dir` の中にカタログ・アプリのデータのフォルダがあれば、動かせない理由を返す。
fn protected_reason(inner: &Inner, dir: &Path) -> Option<String> {
    let canonical_dir = std::fs::canonicalize(dir).ok();
    let inside = |p: &Path| {
        p.starts_with(dir)
            || canonical_dir.as_deref().is_some_and(|cd| {
                std::fs::canonicalize(p)
                    .ok()
                    .is_some_and(|cp| cp.starts_with(cd))
            })
    };
    let catalog = inner.config.catalog_path.as_path();
    let data = inner.config.data_dir.as_path();
    if inside(catalog) {
        return Some(format!(
            "このフォルダにはカタログ（{}）があるため、動かせません",
            catalog.display()
        ));
    }
    if inside(data) {
        return Some(format!(
            "このフォルダにはアプリのデータのフォルダ（{}）があるため、動かせません",
            data.display()
        ));
    }
    None
}

/// 名前（比較キー）が RAW か JPEG で、拡張子を除いた部分が同じか。
fn pairable_stem(name: &str) -> Option<String> {
    if pair_class(name) == PairClass::Other {
        return None;
    }
    let key = path_key(name);
    let (stem, _) = split_extension(&key);
    Some(stem.to_owned())
}

/// フォルダの中にある名前（ディスク）。比較キーの集まりと、RAW・JPEG の拡張子を除いた部分の集まり。
#[derive(Default)]
struct DiskNames {
    names: HashSet<String>,
    stems: HashSet<String>,
}

fn disk_names(dir: &Path) -> DiskNames {
    let mut out = DiskNames::default();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let Some(name) = e.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Some(stem) = pairable_stem(&name) {
                out.stems.insert(stem);
            }
            out.names.insert(path_key(&name));
        }
    }
    out
}

/// 1 枚の写真の移動・名前の変更の内容（予定と実行で共通）。
#[derive(Debug, Clone)]
struct PhotoOp {
    variant_id: VariantId,
    asset_id: AssetId,
    files: Vec<FileOp>,
    problem: Option<String>,
}

#[derive(Debug, Clone)]
struct FileOp {
    rec: FileRecord,
    from: Option<PathBuf>,
    dest_folder: FolderId,
    to: Option<PathBuf>,
    new_name: String,
    method: MoveMethod,
}

impl PhotoOp {
    fn planned(&self) -> PlannedPhotoMove {
        PlannedPhotoMove {
            variant_id: self.variant_id,
            asset_id: self.asset_id,
            files: self
                .files
                .iter()
                .map(|f| PlannedFileMove {
                    file_id: f.rec.id,
                    role: f.rec.role,
                    from: f.from.clone(),
                    to: f.to.clone(),
                    method: f.method,
                    size: f.rec.facts.size,
                })
                .collect(),
            problem: self.problem.clone(),
        }
    }

    /// 変える（名前か場所が変わる）ファイル。
    fn changing(&self) -> impl Iterator<Item = &FileOp> {
        self.files
            .iter()
            .filter(|f| f.dest_folder != f.rec.folder_id || f.new_name != f.rec.name)
    }

    fn label(&self) -> PathBuf {
        self.files
            .first()
            .and_then(|f| f.from.clone())
            .unwrap_or_else(|| PathBuf::from(format!("写真 {}", self.variant_id)))
    }
}

/// 選んだ写真の asset（重ねずに、与えた順）と、そのマスターの variant。
fn assets_in_order(
    cat: &Catalog,
    variant_ids: &[VariantId],
) -> Result<Vec<(AssetId, VariantId)>, ApiError> {
    let mut out: Vec<(AssetId, VariantId)> = Vec::new();
    for &v in variant_ids {
        let a = cat.asset_of_variant(v)?;
        if !out.iter().any(|(x, _)| *x == a) {
            out.push((a, cat.master_variant_of(a)?));
        }
    }
    Ok(out)
}

/// 写真の移動の予定を作る（カタログのロックの中で。ファイルの存在・名前の衝突を確かめる）。
fn build_move_plan(
    cat: &Catalog,
    variant_ids: &[VariantId],
    dest: FolderId,
) -> Result<(PathBuf, Vec<PhotoOp>), ApiError> {
    let dest_dir = folder_dir(cat, dest)?;
    let dest_volume = cat.folder_location(dest)?.volume_id;
    let disk = disk_names(&dest_dir);
    let mut out = Vec::new();
    // この移動で置く名前（同じ移動の写真どうしの衝突）。
    let mut taken: HashSet<String> = HashSet::new();
    let mut taken_stems: HashSet<String> = HashSet::new();
    for (asset_id, variant_id) in assets_in_order(cat, variant_ids)? {
        let mut photo = PhotoOp {
            variant_id,
            asset_id,
            files: Vec::new(),
            problem: None,
        };
        let records = cat.files_of_asset(asset_id)?;
        let own_stems: HashSet<String> = records
            .iter()
            .filter_map(|r| pairable_stem(&r.name))
            .collect();
        for rec in records {
            let loc = cat.file_location(rec.id)?;
            let from = loc.absolute_path();
            let method = if loc.volume_id == dest_volume {
                MoveMethod::Rename
            } else {
                MoveMethod::Copy
            };
            let to = dest_dir.join(&rec.name);
            let key = path_key(&rec.name);
            let problem = if loc.folder_id == dest {
                Some("すでにこのフォルダにあります".to_owned())
            } else if from.is_none() {
                Some("元のファイルのドライブがつながっていません".to_owned())
            } else if !from
                .as_deref()
                .is_some_and(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()))
            {
                Some("元のファイルが見つかりません".to_owned())
            } else if disk.names.contains(&key) || cat.find_file(dest, &rec.name)?.is_some() {
                Some(format!("移動先に同じ名前のもの（{}）があります", rec.name))
            } else if taken.contains(&key) {
                Some(format!(
                    "移動する別の写真と名前（{}）が重なります",
                    rec.name
                ))
            } else if let Some(stem) = pairable_stem(&rec.name)
                && (disk.stems.contains(&stem)
                    || taken_stems.contains(&stem)
                    || cat
                        .names_with_same_stem(dest, &rec.name)?
                        .iter()
                        .any(|n| pairable_stem(n).is_some()))
            {
                Some(format!(
                    "移動先に、拡張子だけが違う別の写真のファイル（{stem}.*）があります（1 枚の写真にまとめられるため、移しません）"
                ))
            } else {
                None
            };
            if photo.problem.is_none() {
                photo.problem = problem;
            }
            photo.files.push(FileOp {
                from,
                dest_folder: dest,
                to: Some(to),
                new_name: rec.name.clone(),
                method,
                rec,
            });
        }
        if photo.problem.is_none() {
            for f in &photo.files {
                taken.insert(path_key(&f.new_name));
            }
            taken_stems.extend(own_stems);
        }
        out.push(photo);
    }
    Ok((dest_dir, out))
}

/// 1 枚の写真の OS の操作の記録（戻すため）。
enum Done {
    Renamed { from: PathBuf, to: PathBuf },
    Copied { to: PathBuf, facts: FileFacts },
}

/// 動かしたものを元に戻す。戻せなかった名前の変更のファイルを返す（移動先のままカタログに反映する）。
fn roll_back(done: &[(FileId, Done)]) -> Vec<FileId> {
    let mut stuck = Vec::new();
    for (id, d) in done.iter().rev() {
        match d {
            Done::Renamed { from, to } => {
                if let Err(e) = rename_case_safe(to, from) {
                    tracing::warn!(from = %to.display(), to = %from.display(), error = %e, "移したファイルを元に戻せません");
                    stuck.push(*id);
                }
            }
            Done::Copied { to, facts } => {
                // コピーを消せなくても、元のファイルはあり、カタログは元を指す（食い違わない）。
                remove_own_copy(to, facts);
            }
        }
    }
    stuck
}

/// テスト専用のフックを呼ぶ。
fn hook(inner: &Inner, stage: FileOpStage) -> Result<(), String> {
    match &inner.on_file_op {
        Some(h) => h(&stage),
        None => Ok(()),
    }
}

/// 1 枚の写真を動かした結果。
struct Applied {
    /// 動かしたファイル。
    moved: Vec<MovedFile>,
    /// ゴミ箱へ移せずに残った元のファイル（別のボリュームへのコピー）。
    left: Vec<FileIssue>,
    /// コピーしたバイト数。
    bytes: u64,
}

/// 1 枚の写真のファイルを動かす（モジュールの doc の状態遷移）。失敗したら（場所, 理由）。
fn apply_photo(
    inner: &Inner,
    photo: &PhotoOp,
    kind: FileOpKind,
    token: &CancellationToken,
) -> Result<Applied, (PathBuf, String)> {
    let files: Vec<&FileOp> = photo.changing().collect();
    if files.is_empty() {
        return Ok(Applied {
            moved: Vec::new(),
            left: Vec::new(),
            bytes: 0,
        });
    }
    let label = photo.label();
    // 1. 直前に確かめる: 元のファイルが登録の内容のまま（大きさ・更新日時・クイックハッシュ）で、移動先にない。
    let mut mtimes = Vec::with_capacity(files.len());
    for f in &files {
        let from = f.from.clone().ok_or_else(|| {
            (
                label.clone(),
                "元のファイルのドライブがつながっていません".to_owned(),
            )
        })?;
        let now = FileFacts::read(&from)
            .map_err(|e| (from.clone(), format!("元のファイルを読めません: {e}")))?;
        if now != f.rec.facts {
            return Err((
                from,
                "登録の後にファイルが変わっています（ファイルの確認の後にやり直してください）"
                    .to_owned(),
            ));
        }
        mtimes.push(
            std::fs::metadata(&from)
                .and_then(|m| m.modified())
                .map_err(|e| (from.clone(), format!("更新日時を読めません: {e}")))?,
        );
    }
    // 2. 予定を記録する。
    let requests: Vec<MoveRequest> = files
        .iter()
        .map(|f| MoveRequest {
            file_id: f.rec.id,
            dest_folder: f.dest_folder,
            new_name: f.new_name.clone(),
            method: match f.method {
                MoveMethod::Rename => genzo_catalog::MoveMethod::Rename,
                MoveMethod::Copy => genzo_catalog::MoveMethod::Copy,
            },
        })
        .collect();
    let op = inner
        .with_catalog(|c| {
            let op = c.plan_file_moves(kind, &requests)?;
            c.start_file_op(op)?;
            Ok(op)
        })
        .map_err(|e| (label.clone(), e.to_string()))?;
    // 3. OS の操作。
    let mut done: Vec<(FileId, Done)> = Vec::new();
    let mut applied: Vec<AppliedEntry> = Vec::new();
    let mut bytes = 0u64;
    let mut failure: Option<(PathBuf, String)> = None;
    // 失敗した名前の変更の結果が分からない（[`rename_outcome_unknown`]）。
    let mut unsettled = false;
    for (f, mtime) in files.iter().zip(&mtimes) {
        let from = f.from.clone().expect("上で確かめた");
        let to = f.to.clone().expect("移動先がある");
        let r = match f.method {
            MoveMethod::Rename => rename_case_safe(&from, &to)
                .map(|()| {
                    done.push((
                        f.rec.id,
                        Done::Renamed {
                            from: from.clone(),
                            to: to.clone(),
                        },
                    ));
                    applied.push(AppliedEntry {
                        file_id: f.rec.id,
                        facts: None,
                        full_hash: None,
                    });
                })
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        "移動先に同じ名前のものがあります（上書きしません）".to_owned()
                    } else {
                        format!("ファイルを動かせません: {e}")
                    }
                }),
            MoveMethod::Copy => {
                if let Some(dir) = to.parent() {
                    crate::copy_import::remove_stale_temp_files(dir);
                }
                match copy_verified_with(None, &from, &f.rec.facts, *mtime, &to, token) {
                    Ok(info) => {
                        bytes += info.bytes;
                        done.push((
                            f.rec.id,
                            Done::Copied {
                                to: to.clone(),
                                facts: info.facts.clone(),
                            },
                        ));
                        applied.push(AppliedEntry {
                            file_id: f.rec.id,
                            facts: Some(info.facts),
                            full_hash: Some(info.full_hash),
                        });
                        Ok(())
                    }
                    Err(CopyFailure::Cancelled) => Err("取り消しました".to_owned()),
                    Err(CopyFailure::Exists) => {
                        Err("移動先に同じ名前のものがあります（上書きしません）".to_owned())
                    }
                    Err(CopyFailure::Failed(m) | CopyFailure::NoSpace(m)) => Err(m),
                }
            }
        };
        let r = r.and_then(|()| hook(inner, FileOpStage::Moved(to.clone())));
        if let Err(m) = r {
            unsettled = f.method == MoveMethod::Rename
                && !done.iter().any(|(id, _)| *id == f.rec.id)
                && rename_outcome_unknown(&from, &to);
            failure = Some((from, m));
            break;
        }
    }
    if failure.is_none()
        && let Err(m) = hook(inner, FileOpStage::Completing(op.get()))
    {
        failure = Some((label.clone(), m));
    }
    // 4. カタログを更新する（失敗したら OS の操作を戻す）。
    if failure.is_none()
        && let Err(e) = inner.with_catalog(|c| c.complete_file_op_entries(op, &applied, None))
    {
        failure = Some((label.clone(), format!("カタログを更新できません: {e}")));
    }
    if let Some((path, message)) = failure {
        let stuck = roll_back(&done);
        if unsettled {
            // 記録を実行中のまま残し、次に開いたときに実際の場所を見て確定する（settle。一時的な名前のままなら
            // 名前の変更を終わらせる）。ほかのファイルは上で元に戻した。
            tracing::warn!(op = op.get(), path = %path.display(), "名前の変更の結果が分からないため、ファイル操作を実行中のまま残します");
            return Err((path, format!("{message}{UNSETTLED_NOTE}")));
        }
        let r = inner.with_catalog(|c| {
            if stuck.is_empty() {
                c.fail_file_op(op, &message)
            } else {
                let keep: Vec<AppliedEntry> = applied
                    .iter()
                    .filter(|a| stuck.contains(&a.file_id))
                    .cloned()
                    .collect();
                c.complete_file_op_entries(
                    op,
                    &keep,
                    Some(&format!(
                        "{message}（元に戻せなかった {} 件のファイルは、移動先のままカタログに反映しました）",
                        keep.len()
                    )),
                )
            }
        });
        if let Err(e) = r {
            // 記録を確定できない: 実行中のまま残り、次に開いたときに実際の場所を見て確定する。
            tracing::warn!(error = %e, op = op.get(), "ファイル操作の失敗を記録できません");
        }
        return Err((path, message));
    }
    // 5. 別のボリュームへコピーした元のファイルを、ゴミ箱へ移す（カタログはもう移動先を指す）。
    let mut moved = Vec::new();
    let mut left = Vec::new();
    for f in &files {
        let from = f.from.clone().expect("上で確かめた");
        let to = f.to.clone().expect("移動先がある");
        if f.method == MoveMethod::Copy {
            let same = FileFacts::read(&from).is_ok_and(|now| now == f.rec.facts);
            let r = if same {
                inner.trash.move_to_trash(&from)
            } else {
                Err("コピーの後に元のファイルが変わったため、ゴミ箱へ移していません".to_owned())
            };
            if let Err(reason) = r {
                left.push(FileIssue {
                    path: from.clone(),
                    reason,
                });
            }
        }
        moved.push(MovedFile {
            file_id: f.rec.id,
            from,
            to,
        });
    }
    Ok(Applied { moved, left, bytes })
}

/// ファイルの場所が変わった写真について: 変更を知らせ、表示中の検索結果を作り直す。
fn after_moves(inner: &Inner, assets: &[AssetId]) {
    if assets.is_empty() {
        return;
    }
    let mut variants: Vec<VariantId> = Vec::new();
    for &a in assets {
        if let Ok(list) = inner.with_catalog(|c| c.variants_of_asset(a)) {
            variants.extend(list.into_iter().map(|v| v.id));
        }
    }
    inner.events.emit(Event::CatalogChanged {
        change: CatalogChange::Files,
        variant_ids: variants,
        all: false,
    });
    crate::search::refresh(inner, &[]);
}

/// フォルダが変わったことを知らせる（フォルダの一覧と、写真の場所の表示を読み直す）。
pub(crate) fn announce_folders(inner: &Inner) {
    inner.events.emit(Event::CatalogChanged {
        change: CatalogChange::Folders,
        variant_ids: Vec::new(),
        all: true,
    });
    crate::search::refresh(inner, &[]);
}

/// 写真の移動のジョブの本体。
fn run_move_job(
    ctx: &JobCtx<'_>,
    variant_ids: &[VariantId],
    dest: FolderId,
) -> Result<FileMoveReport, ApiError> {
    let inner = ctx.inner;
    let mut report = FileMoveReport::default();
    let photos = {
        let _ops = inner.file_ops.lock();
        crate::volumes::refresh(inner)?;
        inner
            .with_catalog_api(|c| build_move_plan(c, variant_ids, dest))?
            .1
    };
    let total = photos.len() as u64;
    ctx.progress(0, total);
    let mut moved_assets = Vec::new();
    for (i, photo) in photos.iter().enumerate() {
        if ctx.is_cancelled() {
            report.cancelled = true;
            break;
        }
        if let Some(p) = &photo.problem {
            report.skipped.push(FileIssue {
                path: photo.label(),
                reason: p.clone(),
            });
            ctx.progress(i as u64 + 1, total);
            continue;
        }
        let r = {
            let _ops = inner.file_ops.lock();
            apply_photo(inner, photo, FileOpKind::Move, ctx.token())
        };
        match r {
            Ok(Applied { moved, left, bytes }) => {
                report.photos += 1;
                report.bytes_copied += bytes;
                report.moved.extend(moved);
                for l in &left {
                    inner.events.emit(Event::Warning {
                        code: WarningCode::MoveSourceLeft,
                        message: format!(
                            "別のドライブへ移した写真の元のファイルを、ゴミ箱へ移せませんでした（元の場所に残っています）: {}",
                            l.reason
                        ),
                        variant_id: Some(photo.variant_id),
                        path: Some(l.path.clone()),
                    });
                }
                report.left_sources.extend(left);
                moved_assets.push(photo.asset_id);
            }
            Err((path, reason)) => {
                if ctx.is_cancelled() {
                    report.cancelled = true;
                }
                report.failed.push(FileIssue { path, reason });
            }
        }
        ctx.progress(i as u64 + 1, total);
    }
    after_moves(inner, &moved_assets);
    Ok(report)
}

/// 名前の変更の予定を作る（カタログのロックの中で）。
fn build_rename_plan(
    cat: &Catalog,
    variant_ids: &[VariantId],
    spec: &RenameSpec,
    default_offset: chrono::FixedOffset,
) -> Result<Vec<PhotoOp>, ApiError> {
    let assets = assets_in_order(cat, variant_ids)?;
    let (parts, rule) = match spec {
        RenameSpec::Template { rule } => (Some(rule.parse()?), Some(rule)),
        RenameSpec::Name { .. } => {
            if assets.len() != 1 {
                return Err(ApiError::InvalidArgument(
                    "名前を直接入力するときは、写真を 1 枚だけ選んでください（複数の写真はテンプレートで名前を付けます）"
                        .to_owned(),
                ));
            }
            (None, None)
        }
    };
    let explicit = match spec {
        RenameSpec::Name { name } => Some(validate_name(name)?),
        RenameSpec::Template { .. } => None,
    };
    // フォルダごとの、ディスクの名前（読んだもの）と、この変更で置く名前。
    let mut disk: HashMap<FolderId, DiskNames> = HashMap::new();
    let mut taken: HashSet<(FolderId, String)> = HashSet::new();
    let mut taken_stems: HashSet<(FolderId, String)> = HashSet::new();
    let mut out = Vec::new();
    let mut sequence = rule.map_or(1, |r| u64::from(r.sequence_start));
    for (asset_id, variant_id) in assets {
        let records = cat.files_of_asset(asset_id)?;
        let asset = cat.asset(asset_id)?;
        let mut photo = PhotoOp {
            variant_id,
            asset_id,
            files: Vec::new(),
            problem: None,
        };
        let Some(primary) = records.first() else {
            continue;
        };
        let base = match (&parts, rule, &explicit) {
            (Some(parts), Some(rule), _) => {
                // 撮影日時（表示の現地時刻）。分からなければファイルの更新日時（既定のタイムゾーン）。取り込みの
                // リネームと同じ。
                let when = asset
                    .capture
                    .local()
                    .map(|t| t.naive_local())
                    .unwrap_or_else(|| {
                        chrono::DateTime::from_timestamp_nanos(primary.facts.mtime_ns)
                            .with_timezone(&default_offset)
                            .naive_local()
                    });
                let (orig, _) = split_extension(&primary.name);
                let input = NameInput {
                    original_stem: orig,
                    when,
                    camera: asset.camera.as_deref(),
                    sequence,
                };
                sequence += 1;
                render_stem(parts, rule, &input)
            }
            (_, _, Some(name)) => name.clone(),
            _ => unreachable!("テンプレートか名前のどちらか"),
        };
        // 名前の衝突を避ける番号（テンプレートだけ。直接の名前は衝突すれば問題にする）。
        let own_ids: HashSet<FileId> = records.iter().map(|r| r.id).collect();
        let mut chosen: Option<String> = None;
        let max = if explicit.is_some() {
            0
        } else {
            MAX_COLLISION_SUFFIX
        };
        let mut why_taken = String::new();
        for n in 0..=max {
            let stem = with_suffix(&base, n);
            let mut free = true;
            for r in &records {
                let (_, ext) = split_extension(&r.name);
                let name = join_extension(&stem, ext);
                let key = path_key(&name);
                let dir_names = match disk.get(&r.folder_id) {
                    Some(d) => d,
                    None => {
                        let loc = cat.folder_location(r.folder_id)?;
                        let names = loc
                            .mount_path
                            .as_deref()
                            .map(|m| disk_names(&join_rel(Path::new(m), &loc.rel_dir, "")))
                            .unwrap_or_default();
                        disk.entry(r.folder_id).or_insert(names)
                    }
                };
                let own_name = key == path_key(&r.name);
                let registered_other = cat
                    .find_file(r.folder_id, &name)?
                    .is_some_and(|other| !own_ids.contains(&other.id));
                let on_disk_other = !own_name && dir_names.names.contains(&key);
                let in_batch = taken.contains(&(r.folder_id, key.clone()));
                let stem_clash = pairable_stem(&name).is_some_and(|s| {
                    let own_stem = pairable_stem(&r.name).is_some_and(|o| o == s);
                    taken_stems.contains(&(r.folder_id, s.clone()))
                        || (!own_stem
                            && (dir_names.stems.contains(&s)
                                || cat
                                    .names_with_same_stem(r.folder_id, &name)
                                    .is_ok_and(|v| v.iter().any(|n| pairable_stem(n).is_some()))))
                });
                if registered_other || on_disk_other || in_batch || stem_clash {
                    free = false;
                    why_taken = if stem_clash && !(registered_other || on_disk_other || in_batch) {
                        format!(
                            "同じフォルダに、拡張子だけが違う別の写真のファイル（{stem}.*）があります"
                        )
                    } else {
                        format!("同じフォルダに同じ名前のもの（{name}）があります")
                    };
                    break;
                }
            }
            if free {
                chosen = Some(stem);
                break;
            }
        }
        let Some(stem) = chosen else {
            photo.problem = Some(why_taken);
            for r in records {
                photo.files.push(FileOp {
                    from: cat.file_location(r.id)?.absolute_path(),
                    dest_folder: r.folder_id,
                    to: None,
                    new_name: r.name.clone(),
                    method: MoveMethod::Rename,
                    rec: r,
                });
            }
            out.push(photo);
            continue;
        };
        for r in records {
            let loc = cat.file_location(r.id)?;
            let from = loc.absolute_path();
            let (_, ext) = split_extension(&r.name);
            let new_name = join_extension(&stem, ext);
            if photo.problem.is_none() {
                if from.is_none() {
                    photo.problem = Some("ファイルのドライブがつながっていません".to_owned());
                } else if !from
                    .as_deref()
                    .is_some_and(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file()))
                {
                    photo.problem = Some("ファイルが見つかりません".to_owned());
                }
            }
            taken.insert((r.folder_id, path_key(&new_name)));
            if let Some(s) = pairable_stem(&new_name) {
                taken_stems.insert((r.folder_id, s));
            }
            let to = from
                .as_deref()
                .and_then(Path::parent)
                .map(|d| d.join(&new_name));
            photo.files.push(FileOp {
                from,
                dest_folder: r.folder_id,
                to,
                new_name,
                method: MoveMethod::Rename,
                rec: r,
            });
        }
        if photo.problem.is_none() && photo.changing().next().is_none() {
            photo.problem = Some("今と同じ名前です".to_owned());
        }
        out.push(photo);
    }
    Ok(out)
}

impl Core {
    /// 写真をフォルダ `dest_folder` へ移動する予定（ファイル・方法・移せない理由。カタログもファイルも変えない）。
    /// 写真（asset）ごとに、RAW と同時記録の JPEG を一緒に動かす。モジュールの doc。
    pub fn plan_move_files(
        &self,
        variant_ids: &[VariantId],
        dest_folder: FolderId,
    ) -> Result<FileMovePlan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if variant_ids.is_empty() {
            return Err(ApiError::InvalidArgument("写真を選んでください".to_owned()));
        }
        crate::volumes::refresh(inner)?;
        let (dest_dir, photos) =
            inner.with_catalog_api(|c| build_move_plan(c, variant_ids, dest_folder))?;
        let movable = photos.iter().filter(|p| p.problem.is_none()).count() as u64;
        let copy_bytes = photos
            .iter()
            .filter(|p| p.problem.is_none())
            .flat_map(|p| p.files.iter())
            .filter(|f| f.method == MoveMethod::Copy)
            .map(|f| f.rec.facts.size)
            .sum();
        Ok(FileMovePlan {
            dest_folder,
            dest_path: Some(dest_dir),
            photos: photos.iter().map(PhotoOp::planned).collect(),
            movable,
            copy_bytes,
        })
    }

    /// 写真をフォルダ `dest_folder` へ移動する（ジョブ。結果は [`JobResult::MoveFiles`]）。移せない写真は飛ばす。
    /// 取り込み・ほかの移動の実行中は始めない。モジュールの doc。
    pub fn move_files(
        &self,
        variant_ids: &[VariantId],
        dest_folder: FolderId,
    ) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if variant_ids.is_empty() {
            return Err(ApiError::InvalidArgument("写真を選んでください".to_owned()));
        }
        ensure_no_conflicting_jobs(inner, None)?;
        // 移動先を確かめる（つながっていない・見つからなければ、ジョブを始めずにエラー）。
        inner.with_catalog_api(|c| folder_dir(c, dest_folder))?;
        let ids = variant_ids.to_vec();
        Ok(spawn_job(
            inner,
            JobKind::MoveFiles,
            "写真の移動",
            move |ctx| run_move_job(ctx, &ids, dest_folder).map(JobResult::MoveFiles),
        ))
    }

    /// 写真の名前の変更の予定（新しい名前・変えられない理由。カタログもファイルも変えない）。
    pub fn plan_rename_files(
        &self,
        variant_ids: &[VariantId],
        spec: &RenameSpec,
    ) -> Result<FileRenamePlan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if variant_ids.is_empty() {
            return Err(ApiError::InvalidArgument("写真を選んでください".to_owned()));
        }
        crate::volumes::refresh(inner)?;
        let offset = inner.default_offset();
        let photos = inner.with_catalog_api(|c| build_rename_plan(c, variant_ids, spec, offset))?;
        Ok(FileRenamePlan {
            renamable: photos.iter().filter(|p| p.problem.is_none()).count() as u64,
            photos: photos.iter().map(PhotoOp::planned).collect(),
        })
    }

    /// 写真の名前を変える（写真ごとに、すべてのファイルを同じ名前にする。拡張子は元のまま）。変えられない写真は
    /// 飛ばす。モジュールの doc。
    pub fn rename_files(
        &self,
        variant_ids: &[VariantId],
        spec: &RenameSpec,
    ) -> Result<FileMoveReport, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if variant_ids.is_empty() {
            return Err(ApiError::InvalidArgument("写真を選んでください".to_owned()));
        }
        ensure_no_conflicting_jobs(inner, None)?;
        let _ops = inner.file_ops.lock();
        crate::volumes::refresh(inner)?;
        let offset = inner.default_offset();
        let photos = inner.with_catalog_api(|c| build_rename_plan(c, variant_ids, spec, offset))?;
        let token = CancellationToken::new();
        let mut report = FileMoveReport::default();
        let mut changed = Vec::new();
        for photo in &photos {
            if let Some(p) = &photo.problem {
                report.skipped.push(FileIssue {
                    path: photo.label(),
                    reason: p.clone(),
                });
                continue;
            }
            match apply_photo(inner, photo, FileOpKind::Rename, &token) {
                Ok(Applied { moved, .. }) => {
                    report.photos += 1;
                    report.moved.extend(moved);
                    changed.push(photo.asset_id);
                }
                Err((path, reason)) => report.failed.push(FileIssue { path, reason }),
            }
        }
        after_moves(inner, &changed);
        Ok(report)
    }

    /// フォルダ `parent` の中に、名前 `name` のフォルダを作る（ディスクとカタログ。同じ名前があればエラー）。
    pub fn create_folder(&self, parent: FolderId, name: &str) -> Result<FolderInfo, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let name = validate_name(name)?;
        let _ops = inner.file_ops.lock();
        crate::volumes::refresh(inner)?;
        let dir = inner.with_catalog_api(|c| folder_dir(c, parent))?;
        let path = dir.join(&name);
        std::fs::create_dir(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                ApiError::FileAccess {
                    path: path.clone(),
                    message: format!("同じ名前のフォルダ・ファイルがあります: {}", path.display()),
                }
            } else {
                ApiError::io(&path, e)
            }
        })?;
        let id = match inner.with_catalog(|c| c.create_folder(parent, &name)) {
            Ok(id) => id,
            Err(e) => {
                // カタログに登録できなければ、作った（空の）フォルダを消す。
                let _ = std::fs::remove_dir(&path);
                return Err(e);
            }
        };
        announce_folders(inner);
        self.folder_info(id)
    }

    /// フォルダの名前を変える（ディスクとカタログ。配下のフォルダ・ファイルも一緒に動く）。
    pub fn rename_folder(&self, folder: FolderId, name: &str) -> Result<FolderInfo, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let name = validate_name(name)?;
        let rel = inner.with_catalog(|c| c.folder(folder))?.rel_path;
        let parent = rel.rsplit_once('/').map_or("", |(p, _)| p);
        let new_rel = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}/{name}")
        };
        self.folder_op(FileOpKind::Rename, folder, &new_rel)
    }

    /// フォルダを別のフォルダ `new_parent` の中へ移す（同じボリュームの中だけ。ディスクとカタログ）。
    pub fn move_folder(
        &self,
        folder: FolderId,
        new_parent: FolderId,
    ) -> Result<FolderInfo, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let (f, p) = inner.with_catalog(|c| Ok((c.folder(folder)?, c.folder(new_parent)?)))?;
        if f.volume_id != p.volume_id {
            return Err(ApiError::InvalidArgument(
                "別のドライブへのフォルダの移動はできません（写真を選んで、移動先のフォルダへ移してください）"
                    .to_owned(),
            ));
        }
        if f.parent_id == Some(new_parent) {
            return self.folder_info(folder);
        }
        let new_rel = if p.rel_path.is_empty() {
            f.name().to_owned()
        } else {
            format!("{}/{}", p.rel_path, f.name())
        };
        self.folder_op(FileOpKind::Move, folder, &new_rel)
    }

    /// フォルダ（と配下のフォルダ）をカタログから除く（FILE-04。カタログの中だけで、ディスクのフォルダと
    /// ファイルには触れない）。写真のファイルが登録されていないフォルダ（見つからないフォルダ・写真を再リンク
    /// して空になったフォルダ・場所の指定し直しの先に登録済みのフォルダ）だけを除ける（写真があればエラー。
    /// 先に写真をカタログから除くか、再リンクする）。ボリュームのルートなら、ボリュームの記録も除く。除いた
    /// フォルダの数を返す。取り込み・写真の移動・見つからない写真の確認の実行中はできない（同じフォルダに
    /// 写真を登録しうる・確認がフォルダを読むため）。
    pub fn remove_folder(&self, folder: FolderId) -> Result<u64, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        ensure_no_conflicting_jobs(inner, None)?;
        if inner.jobs.is_running(JobKind::CheckMissing, None) {
            return Err(ApiError::InvalidArgument(
                "見つからない写真の確認の実行中です。終わってからやり直してください".to_owned(),
            ));
        }
        let _ops = inner.file_ops.lock();
        let r = inner.with_catalog(|c| c.remove_folder(folder))?;
        if let Some(v) = r.volume {
            inner.volume_states.lock().remove(&v);
        }
        announce_folders(inner);
        Ok(r.folders)
    }

    /// フォルダの名前の変更・移動（同じボリュームの中の OS の 1 回の名前の変更）。
    fn folder_op(
        &self,
        kind: FileOpKind,
        folder: FolderId,
        new_rel: &str,
    ) -> Result<FolderInfo, ApiError> {
        let inner = &self.inner;
        ensure_no_conflicting_jobs(inner, None)?;
        let _ops = inner.file_ops.lock();
        crate::volumes::refresh(inner)?;
        let (from_dir, to_dir) = inner.with_catalog_api(|c| {
            let f = c.folder(folder)?;
            if f.rel_path.is_empty() {
                return Err(ApiError::InvalidArgument(
                    "ボリュームのルートのフォルダは、移動・名前の変更ができません".to_owned(),
                ));
            }
            let from = folder_dir(c, folder)?;
            let loc = c.folder_location(folder)?;
            let mount = loc.mount_path.as_deref().expect("folder_dir で確かめた");
            check_subfolders_inside(c, folder, &f.rel_path, Path::new(mount), &from)?;
            Ok((from, join_rel(Path::new(mount), new_rel, "")))
        })?;
        if from_dir == to_dir {
            return self.folder_info(folder);
        }
        if let Some(reason) = protected_reason(inner, &from_dir) {
            return Err(ApiError::InvalidArgument(reason));
        }
        if let Some(parent) = to_dir.parent()
            && !parent.is_dir()
        {
            return Err(ApiError::FileAccess {
                path: parent.to_path_buf(),
                message: format!("移動先のフォルダが見つかりません: {}", parent.display()),
            });
        }
        let op = inner.with_catalog(|c| {
            let op = c.plan_folder_op(kind, folder, new_rel)?;
            c.start_file_op(op)?;
            Ok(op)
        })?;
        let r = rename_case_safe(&from_dir, &to_dir)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    format!("移動先に同じ名前のものがあります: {}", to_dir.display())
                } else {
                    format!("フォルダを動かせません: {e}")
                }
            })
            .and_then(|()| hook(inner, FileOpStage::Moved(to_dir.clone())));
        if let Err(message) = r {
            // 動いていれば戻す（フックの失敗の場合）。
            if !from_dir.exists() && to_dir.exists() {
                let _ = rename_case_safe(&to_dir, &from_dir);
            }
            if rename_outcome_unknown(&from_dir, &to_dir) {
                // 記録を実行中のまま残し、次に開いたときに実際の場所を見て確定する（settle）。
                tracing::warn!(op = op.get(), path = %from_dir.display(), "フォルダの名前の変更の結果が分からないため、ファイル操作を実行中のまま残します");
                return Err(ApiError::FileAccess {
                    path: from_dir,
                    message: format!("{message}{UNSETTLED_NOTE}"),
                });
            }
            if let Err(e) = inner.with_catalog(|c| c.fail_file_op(op, &message)) {
                tracing::warn!(error = %e, "フォルダの操作の失敗を記録できません");
            }
            return Err(ApiError::FileAccess {
                path: from_dir,
                message,
            });
        }
        if let Err(e) = inner.with_catalog(|c| c.complete_file_op(op)) {
            // カタログを更新できない: 戻して失敗にする（戻せなければ、実行中の記録を次に開いたときに確定する）。
            if rename_case_safe(&to_dir, &from_dir).is_ok() {
                let _ = inner.with_catalog(|c| c.fail_file_op(op, &e.to_string()));
            }
            return Err(e);
        }
        announce_folders(inner);
        self.folder_info(folder)
    }

    /// 1 つのフォルダの情報（[`Core::folders`] の 1 行）。
    pub(crate) fn folder_info(&self, folder: FolderId) -> Result<FolderInfo, ApiError> {
        self.folders()?
            .into_iter()
            .find(|f| f.folder_id == folder)
            .ok_or_else(|| ApiError::NotFound(format!("フォルダ {folder}")))
    }
}

// ---------------------------------------------------------------------------
// 起動時の確定（DATA-07）
// ---------------------------------------------------------------------------

/// 終わっていない移動・名前の変更の確定の仕方。
pub(crate) enum Settle {
    /// 確かめられない（ドライブがつながっていない）。次に開いたときに確かめ直す。
    Pending(String),
    /// 移動先にあるファイル（フォルダの操作は空）だけをカタログに反映して done にする。
    Done {
        applied: Vec<AppliedEntry>,
        note: Option<String>,
    },
    /// 何も動いていない（failed）。
    Failed(String),
}

/// 記録したファイルの今の場所（カタログのフォルダの今のマウント先から。ドライブがつながっていなければ `None`。
/// カタログにないフォルダは記録した絶対パス）。
fn current_path(cat: &Catalog, p: &genzo_catalog::FileOpPath) -> Option<PathBuf> {
    match cat.folder_location(p.folder_id) {
        Ok(loc) => loc
            .mount_path
            .as_deref()
            .map(|m| join_rel(Path::new(m), &loc.rel_dir, &p.name)),
        Err(_) => p.absolute_path.as_deref().map(PathBuf::from),
    }
}

/// フォルダがあり、その中の `name` の項目を、大文字・小文字まで同じ名前で見つけたか（大文字・小文字を区別しない
/// ボリュームで、名前の変更の前後を見分ける）。
fn exact_entry(path: &Path) -> bool {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().any(|e| e.file_name() == name))
        .unwrap_or(false)
}

/// 結果が分からない名前の変更の失敗に添える文（記録を実行中のまま残したとき）。
const UNSETTLED_NOTE: &str =
    "（ファイルの場所が分からないため、次にカタログを開いたときに、実際の場所を見て確定します）";

/// 名前の変更（`from` → `to`）が失敗した後に、結果が分からないか: 元の名前にも新しい名前にもなく、大文字・
/// 小文字だけの変更の一時的な名前（`case_rename_temp_path`）にある（2 回目の名前の変更も、元に戻すのも
/// 失敗した）か、新しい名前のフォルダが見えない（ドライブが外れたなど）。このときは操作を失敗として記録せず、
/// 実行中のまま残す（次に開いたときの [`settle`] が、実際の場所を見て確定する。一時的な名前のままなら名前の
/// 変更を終わらせる）。
fn rename_outcome_unknown(from: &Path, to: &Path) -> bool {
    let gone = |p: &Path| std::fs::symlink_metadata(p).is_err();
    gone(from)
        && gone(to)
        && (!gone(&case_rename_temp_path(to)) || !to.parent().is_some_and(Path::is_dir))
}

/// 大文字・小文字だけの名前の変更（genzo-media の `rename_case_safe`）が、2 回の名前の変更の間で終わり、一時的な
/// 名前（`case_rename_temp_path`。新しい名前 `to` から決まる）のまま残っていれば、名前の変更を終わらせる（`to`
/// へ。できなければ `from` へ戻す。どちらも上書きしない）。元の名前・新しい名前のどちらもないときに呼ぶ。
///
/// 一時的な名前のものがない、または `expected`（中身がこの操作のものか）に合わなければ `None`。`to` にしたら
/// `Some(Ok(true))`、`from` に戻したら `Some(Ok(false))`、どちらにもできなければ `Some(Err(説明))`。
fn finish_case_rename(
    from: &Path,
    to: &Path,
    expected: impl Fn(&Path) -> bool,
) -> Option<Result<bool, String>> {
    let temp = case_rename_temp_path(to);
    std::fs::symlink_metadata(&temp).ok()?;
    if !expected(&temp) {
        return None;
    }
    if rename_no_replace(&temp, to).is_ok() {
        return Some(Ok(true));
    }
    if rename_no_replace(&temp, from).is_ok() {
        return Some(Ok(false));
    }
    Some(Err(format!(
        "一時的な名前 {} のものを、新しい名前にも元の名前にも変えられません（手で名前を戻してから開き直してください）",
        temp.display()
    )))
}

/// 終わっていない移動・名前の変更を、実際のファイルの場所を見て確定させる（04 の 6.4 節。DATA-07）。
///
/// 大文字・小文字だけの名前の変更が一時的な名前のまま終わっていたものは、ここで名前の変更を終わらせてから
/// 確定する（[`finish_case_rename`]。ファイルに触れるのはこの場合だけ）。
pub(crate) fn settle(cat: &Catalog, op: &FileOpRecord) -> Settle {
    if let Some(f) = &op.payload.folder {
        let from = cat.folder_location(f.folder_id).ok().and_then(|l| {
            l.mount_path
                .map(|m| join_rel(Path::new(&m), &l.rel_dir, ""))
        });
        let to = cat
            .folder_location(f.folder_id)
            .ok()
            .and_then(|l| l.mount_path.map(|m| join_rel(Path::new(&m), &f.to_rel, "")));
        let (Some(from), Some(to)) = (from, to) else {
            return Settle::Pending(
                "フォルダのドライブがつながっていません。つながった後に開いたときに確定します"
                    .to_owned(),
            );
        };
        let gone = |p: &Path| std::fs::symlink_metadata(p).is_err();
        let moved = if same_file::is_same_file(&from, &to).unwrap_or(false) {
            // 大文字・小文字だけの名前の変更: 新しい名前の項目があるか。
            exact_entry(&to)
        } else if gone(&from) && gone(&to) {
            // 大文字・小文字だけの名前の変更が、一時的な名前のまま終わっていれば終わらせる。
            match finish_case_rename(&from, &to, Path::is_dir) {
                Some(Ok(moved)) => moved,
                Some(Err(note)) => return Settle::Pending(note),
                None => false,
            }
        } else {
            gone(&from) && to.is_dir()
        };
        return if moved {
            Settle::Done {
                applied: Vec::new(),
                note: Some("フォルダは移動先にありました".to_owned()),
            }
        } else {
            Settle::Failed("フォルダは元の場所にありました（動かしていません）".to_owned())
        };
    }
    let mut applied = Vec::new();
    let mut not_moved = 0usize;
    let mut sources_left = 0usize;
    for e in &op.payload.entries {
        let Some(to_rec) = e.to.as_ref() else {
            continue;
        };
        let (Some(from), Some(to)) = (current_path(cat, &e.from), current_path(cat, to_rec)) else {
            return Settle::Pending(
                "ファイルのドライブがつながっていません。つながった後に開いたときに確定します"
                    .to_owned(),
            );
        };
        let visible = |p: &Path| p.parent().is_some_and(Path::is_dir);
        if !visible(&from) || !visible(&to) {
            return Settle::Pending(
                "ファイルのフォルダが見えません（ドライブが外れている可能性があります）。つながった後に開いたときに確定します"
                    .to_owned(),
            );
        }
        let src = std::fs::symlink_metadata(&from).is_ok();
        let dst = std::fs::symlink_metadata(&to).is_ok();
        let registered = cat.file(e.file_id).ok();
        let moved = match e.method {
            genzo_catalog::MoveMethod::Rename => {
                if src && dst && same_file::is_same_file(&from, &to).unwrap_or(false) {
                    exact_entry(&to)
                } else if !src && !dst {
                    // 大文字・小文字だけの名前の変更が、一時的な名前のまま終わっていれば終わらせる（登録した
                    // 大きさ・クイックハッシュと同じものだけ）。
                    let same_content = |p: &Path| {
                        registered.as_ref().is_some_and(|r| {
                            FileFacts::read(p).is_ok_and(|f| {
                                f.size == r.facts.size && f.quick_hash == r.facts.quick_hash
                            })
                        })
                    };
                    match finish_case_rename(&from, &to, same_content) {
                        Some(Ok(moved)) => moved,
                        Some(Err(note)) => return Settle::Pending(note),
                        None => false,
                    }
                } else {
                    !src && dst
                }
            }
            genzo_catalog::MoveMethod::Copy => {
                // 公開したコピーは照合済み（名前は照合の後に付ける）。大きさとクイックハッシュも確かめる。
                let complete = dst
                    && registered.as_ref().is_some_and(|r| {
                        FileFacts::read(&to).is_ok_and(|f| {
                            f.size == r.facts.size && f.quick_hash == r.facts.quick_hash
                        })
                    });
                if complete && src {
                    sources_left += 1;
                }
                complete
            }
        };
        if moved {
            let facts = match e.method {
                genzo_catalog::MoveMethod::Copy => FileFacts::read(&to).ok(),
                genzo_catalog::MoveMethod::Rename => None,
            };
            applied.push(AppliedEntry {
                file_id: e.file_id,
                facts,
                full_hash: None,
            });
        } else {
            not_moved += 1;
        }
    }
    if applied.is_empty() {
        return Settle::Failed(
            "すべてのファイルが元の場所にありました（動かしていません）".to_owned(),
        );
    }
    let mut notes = Vec::new();
    if not_moved > 0 {
        notes.push(format!(
            "{} 件のうち {not_moved} 件は元の場所のまま（移動先にあるものだけをカタログに反映しました）",
            op.payload.entries.len()
        ));
    }
    if sources_left > 0 {
        notes.push(format!(
            "別のドライブへコピーした {sources_left} 件の元のファイルが、元の場所に残っています（確かめて、要らなければ消してください）"
        ));
    }
    Settle::Done {
        applied,
        note: (!notes.is_empty()).then(|| notes.join("。")),
    }
}

/// 確定の結果を記録する（[`settle`] の結果）。確定したら `Some((state, note))`。
pub(crate) fn record_settle(
    cat: &mut Catalog,
    op: &FileOpRecord,
    settle: Settle,
) -> Result<Option<(FileOpState, String)>, ApiError> {
    Ok(match settle {
        Settle::Pending(_) => None,
        Settle::Done { applied, note } => {
            // フォルダの操作は、ファイルの項目がなくてもフォルダを付け替える。
            cat.complete_file_op_entries(op.id, &applied, note.as_deref())?;
            Some((FileOpState::Done, note.unwrap_or_default()))
        }
        Settle::Failed(note) => {
            cat.fail_file_op(op.id, &note)?;
            Some((FileOpState::Failed, note))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 名前の変更が失敗した後の結果が分からない状態（一時的な名前のまま・フォルダが見えない）だけを、実行中の
    /// まま残す。
    #[test]
    fn unknown_rename_outcomes_are_left_for_settling() {
        let d = tempfile::tempdir().unwrap();
        let from = d.path().join("DSC1.ARW");
        let to = d.path().join("dsc1.arw");
        // どこにもない（アプリの外で消された）: 分かる（失敗として記録する）。
        assert!(!rename_outcome_unknown(&from, &to));
        // 元の名前にある（名前の変更をしなかった）: 分かる。
        std::fs::write(&from, b"A").unwrap();
        assert!(!rename_outcome_unknown(&from, &to));
        // 一時的な名前にだけある: 分からない。
        std::fs::rename(&from, case_rename_temp_path(&to)).unwrap();
        assert!(rename_outcome_unknown(&from, &to));
        // 新しい名前にある: 分かる。
        std::fs::rename(case_rename_temp_path(&to), &to).unwrap();
        assert!(!rename_outcome_unknown(&from, &to));
        // フォルダが見えない（ドライブが外れた）: 分からない。
        let gone = d.path().join("外れた");
        assert!(rename_outcome_unknown(&gone.join("a"), &gone.join("b")));
    }

    #[test]
    fn names_must_be_usable_on_every_os() {
        assert_eq!(validate_name("京都 2024").unwrap(), "京都 2024");
        // NFD で入力しても NFC にする。
        assert_eq!(validate_name("か\u{3099}").unwrap(), "が");
        for bad in [
            "", "  ", "a/b", "a\\b", "a:b", "a?", "CON", "con.txt", ".hidden", "末尾.", "末尾 ",
            "a\u{7}b",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        let long = "あ".repeat(70);
        assert!(validate_name(&long).is_err(), "200 バイトを超える");
    }

    #[test]
    fn pairable_stems_are_compared_without_case() {
        assert_eq!(pairable_stem("DSC1.ARW").as_deref(), Some("dsc1"));
        assert_eq!(pairable_stem("dsc1.jpg").as_deref(), Some("dsc1"));
        assert_eq!(pairable_stem("dsc1.mp4"), None);
    }
}
