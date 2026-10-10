//! 登録（IMP-01・VID-01）: ボリューム・フォルダの確保と、ファイルの登録。
//!
//! docs/04_architecture.md の 3.1 節・3.3 節・3.5 節。
//!
//! - 1 ファイルの登録で、asset ＋ マスターの variant ＋ file を 1 つのトランザクションで作る（DATA-02）。
//! - **同じファイルを登録し直しても件数は増えない**（`UNIQUE(folder_id, name_key)` で既存の行を探す）。
//!   サイズ・更新日時・クイックハッシュのいずれかが変わっていたら `revision` を 1 増やし、
//!   [`RegisterStatus::Updated`] を返す（キャッシュの無効化。4.1 節）。
//! - 同じフォルダで拡張子だけが違う RAW と JPEG は、同じ asset にまとめる（JPEG は
//!   `role = sidecar_jpeg`。IMP-06 の基本の扱い）。JPEG を先に登録していた場合は、RAW を
//!   登録したときに JPEG を `sidecar_jpeg` に変えて同じ asset に入れる（評価やキーワードは残る）。
//! - 多数のファイルは [`Catalog::register_batch`] で 1 つのトランザクションにまとめる。
//!   途中で失敗した場合はバッチ全体が取り消されるので、同じバッチをもう一度登録すればよい
//!   （登録済みのものは [`RegisterStatus::Unchanged`] になる）。

use std::path::{Component, Path};

use genzo_model::{
    AssetId, AssetKind, CaptureTime, FileId, FileRole, FileStatus, FolderId, PhotoMetadata,
    TzSource, VariantId, VideoMetadata, VolumeId,
};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::catalog::Catalog;
use crate::develop::{HISTORY_LABEL_IMPORT, insert_variant, new_variant_develop};
use crate::error::{CatalogError, Result};
use crate::hash::{FileFacts, is_hex64};
use crate::text::{path_key, searchable_text};
use crate::util::{i64_to_u32, now_utc_string, parse_enum, u64_to_i64};

/// RAW として扱う拡張子（小文字。RAW と JPEG のペアの判定に使う）。
///
/// 仮置き: LibRaw が対応している主な形式の拡張子。RAW の展開（genzo-raw）の対応状況とは
/// 独立に、ペアの判定だけに使う。必要に応じて追加する。
pub const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "ari", "arw", "bay", "cr2", "cr3", "crw", "dcr", "dcs", "dng", "erf", "fff", "iiq",
    "k25", "kdc", "mef", "mos", "mrw", "nef", "nrw", "orf", "ori", "pef", "raf", "raw", "rw2",
    "rwl", "sr2", "srf", "srw", "x3f",
];

/// RAW と同時に記録された JPEG として扱う拡張子（小文字）。
pub const JPEG_EXTENSIONS: &[&str] = &["jpg", "jpeg"];

/// RAW と JPEG のペアの判定での、ファイルの分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairClass {
    /// RAW（[`RAW_EXTENSIONS`]）。
    Raw,
    /// JPEG（[`JPEG_EXTENSIONS`]）。
    Jpeg,
    /// その他（ペアにしない）。
    Other,
}

/// ファイル名の拡張子からペアの分類を決める。
pub fn pair_class(name: &str) -> PairClass {
    match split_stem(&path_key(name)) {
        Some((_, ext)) if RAW_EXTENSIONS.contains(&ext) => PairClass::Raw,
        Some((_, ext)) if JPEG_EXTENSIONS.contains(&ext) => PairClass::Jpeg,
        _ => PairClass::Other,
    }
}

/// 名前を「拡張子を除いた部分」と拡張子に分ける（どちらも空でない場合だけ）。
fn split_stem(name: &str) -> Option<(&str, &str)> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then_some((stem, ext))
}

/// ボリューム。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// ID。
    pub id: VolumeId,
    /// OS のボリューム ID。
    pub uuid: String,
    /// ラベル。
    pub label: Option<String>,
    /// 最後に確認したマウント先。
    pub last_mount_path: Option<String>,
}

/// フォルダ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// ID。
    pub id: FolderId,
    /// ボリューム。
    pub volume_id: VolumeId,
    /// 親のフォルダ（ボリュームのルートは `None`）。
    pub parent_id: Option<FolderId>,
    /// ボリューム内の相対パス（'/' 区切り。ルートは空文字列）。表示用。
    pub rel_path: String,
}

impl Folder {
    /// フォルダの名前（相対パスの最後の部分。ルートは空文字列）。
    pub fn name(&self) -> &str {
        self.rel_path.rsplit('/').next().unwrap_or("")
    }
}

/// ボリューム内の相対パスを検証して正規の形（'/' 区切り、前後の '/' なし）にする。
///
/// 空文字列はボリュームのルート。空の部分（`a//b`）、`.`、`..`、NUL 文字はエラー。
/// Windows の `\` は区切りとして扱わない（呼び出し側で [`rel_path_from_path`] を使う）。
pub fn normalize_rel_path(rel_path: &str) -> Result<String> {
    let trimmed = rel_path.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains('\0') {
            return Err(CatalogError::InvalidInput(format!(
                "フォルダの相対パスが不正です: {rel_path:?}"
            )));
        }
    }
    Ok(trimmed.to_owned())
}

/// OS のパス（ボリュームのルートからの相対パス）を、カタログの相対パス（'/' 区切り）にする。
pub fn rel_path_from_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str().ok_or_else(|| {
                CatalogError::InvalidInput(format!(
                    "UTF-8 で表せないパスは登録できません: {}",
                    path.display()
                ))
            })?),
            Component::CurDir => {}
            _ => {
                return Err(CatalogError::InvalidInput(format!(
                    "ボリュームのルートからの相対パスではありません: {}",
                    path.display()
                )));
            }
        }
    }
    normalize_rel_path(&parts.join("/"))
}

/// ファイル名を検証する（空・`/`・NUL・`.`・`..` はエラー）。
fn validate_file_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(CatalogError::InvalidInput(format!(
            "ファイル名が不正です: {name:?}"
        )));
    }
    Ok(())
}

/// 写真・動画のメタデータ（ワーカーが読み取ったもの）。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MediaMetadata {
    /// 読み取れなかった、またはない。
    #[default]
    None,
    /// 写真。
    Photo(PhotoMetadata),
    /// 動画。
    Video(VideoMetadata),
}

/// 1 ファイルの登録の要求。
#[derive(Debug, Clone, PartialEq)]
pub struct RegisterFile {
    /// ファイルのあるフォルダ（[`Catalog::ensure_folder`] で確保する）。
    pub folder_id: FolderId,
    /// ファイル名（表示用の元の文字列）。
    pub name: String,
    /// サイズ・更新日時・クイックハッシュ（[`FileFacts::read`]）。
    pub facts: FileFacts,
    /// 写真か動画か。
    pub kind: AssetKind,
    /// メタデータ。
    pub metadata: MediaMetadata,
    /// 撮影日時（[`CaptureTime::from_capture_info`] などで求めたもの）。
    pub capture: CaptureTime,
    /// メタデータの読み取りに失敗した理由。`Some` なら `status = error` で登録する（6.3 節）。
    pub error: Option<String>,
}

impl RegisterFile {
    /// 写真の登録の要求を作る。
    pub fn photo(
        folder_id: FolderId,
        name: impl Into<String>,
        facts: FileFacts,
        metadata: PhotoMetadata,
        capture: CaptureTime,
    ) -> Self {
        Self {
            folder_id,
            name: name.into(),
            facts,
            kind: AssetKind::Photo,
            metadata: MediaMetadata::Photo(metadata),
            capture,
            error: None,
        }
    }

    /// 動画の登録の要求を作る。
    pub fn video(
        folder_id: FolderId,
        name: impl Into<String>,
        facts: FileFacts,
        metadata: VideoMetadata,
        capture: CaptureTime,
    ) -> Self {
        Self {
            folder_id,
            name: name.into(),
            facts,
            kind: AssetKind::Video,
            metadata: MediaMetadata::Video(metadata),
            capture,
            error: None,
        }
    }

    fn validate(&self) -> Result<()> {
        validate_file_name(&self.name)?;
        if !is_hex64(&self.facts.quick_hash) {
            return Err(CatalogError::InvalidInput(format!(
                "クイックハッシュが 16 進数の小文字 64 文字ではありません: {:?}",
                self.facts.quick_hash
            )));
        }
        u64_to_i64(self.facts.size, "ファイルサイズ")?;
        let consistent = matches!(
            (&self.metadata, self.kind),
            (MediaMetadata::None, _)
                | (MediaMetadata::Photo(_), AssetKind::Photo)
                | (MediaMetadata::Video(_), AssetKind::Video)
        );
        if !consistent {
            return Err(CatalogError::InvalidInput(format!(
                "種別 {} とメタデータの種類が一致しません: {}",
                self.kind, self.name
            )));
        }
        Ok(())
    }
}

/// 登録の結果の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterStatus {
    /// 新しく登録した。
    Added,
    /// 登録済みで、内容の変化を検知した（`revision` を 1 増やした）。
    Updated,
    /// 登録済みで、変化はなかった。
    Unchanged,
}

/// 1 ファイルの登録の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisterOutcome {
    /// 結果の種類。
    pub status: RegisterStatus,
    /// ファイルが属する asset。
    pub asset_id: AssetId,
    /// その asset のマスターの variant。
    pub master_variant_id: VariantId,
    /// ファイル。
    pub file_id: FileId,
    /// ファイルの役割。
    pub role: FileRole,
    /// ファイルのリビジョン。
    pub revision: u32,
    /// 今回の登録で、RAW と JPEG のペアとして既存の asset にまとめたか。
    pub paired: bool,
}

impl Catalog {
    /// ボリュームを確保する（なければ作る）。ラベルとマウント先は与えたもので更新する。
    pub fn ensure_volume(
        &mut self,
        uuid: &str,
        label: Option<&str>,
        mount_path: Option<&str>,
    ) -> Result<VolumeId> {
        if uuid.trim().is_empty() {
            return Err(CatalogError::InvalidInput(
                "ボリューム ID が空です".to_owned(),
            ));
        }
        let id: i64 = self.conn.query_row(
            "INSERT INTO volume(uuid, label, last_mount_path) VALUES (?1, ?2, ?3)
             ON CONFLICT(uuid) DO UPDATE SET
                 label = coalesce(excluded.label, label),
                 last_mount_path = coalesce(excluded.last_mount_path, last_mount_path)
             RETURNING id",
            params![uuid, label, mount_path],
            |row| row.get(0),
        )?;
        Ok(VolumeId::new(id))
    }

    /// 登録されているボリュームの一覧。
    pub fn volumes(&self) -> Result<Vec<Volume>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, uuid, label, last_mount_path FROM volume ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok(Volume {
                id: VolumeId::new(row.get(0)?),
                uuid: row.get(1)?,
                label: row.get(2)?,
                last_mount_path: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// フォルダを、親（ボリュームのルートまで）を含めて確保する（1 つのトランザクション）。
    ///
    /// 比較は [`path_key`]（NFC ＋ 小文字化）で行う。既にあるフォルダの表示用のパスは、
    /// 最初に登録したときのものを保つ。
    pub fn ensure_folder(&mut self, volume_id: VolumeId, rel_path: &str) -> Result<FolderId> {
        let rel_path = normalize_rel_path(rel_path)?;
        let tx = self.conn.transaction()?;
        let id = ensure_folder_tx(&tx, volume_id, &rel_path)?;
        tx.commit()?;
        Ok(id)
    }

    /// フォルダを相対パスで探す（なければ `None`）。
    pub fn find_folder(&self, volume_id: VolumeId, rel_path: &str) -> Result<Option<FolderId>> {
        let rel_path = normalize_rel_path(rel_path)?;
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM folder WHERE volume_id = ?1 AND rel_path_key = ?2",
                params![volume_id.get(), path_key(&rel_path)],
                |row| row.get(0),
            )
            .optional()?
            .map(FolderId::new))
    }

    /// フォルダを読む。
    pub fn folder(&self, id: FolderId) -> Result<Folder> {
        self.conn
            .query_row(
                "SELECT id, volume_id, parent_id, rel_path FROM folder WHERE id = ?1",
                [id.get()],
                folder_from_row,
            )
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("フォルダ {id}")))
    }

    /// ボリュームのフォルダの一覧（相対パスの比較キーの順。フォルダツリーの表示用。LIB-06）。
    pub fn folders(&self, volume_id: VolumeId) -> Result<Vec<Folder>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, volume_id, parent_id, rel_path FROM folder
             WHERE volume_id = ?1 ORDER BY rel_path_key",
        )?;
        let rows = stmt.query_map([volume_id.get()], folder_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// 1 ファイルを登録する（1 つのトランザクション）。
    pub fn register_file(&mut self, request: &RegisterFile) -> Result<RegisterOutcome> {
        let mut out = self.register_batch(std::slice::from_ref(request))?;
        Ok(out.pop().expect("1 件の要求には 1 件の結果がある"))
    }

    /// 複数のファイルを 1 つのトランザクションで登録する。結果は要求と同じ順に返す。
    ///
    /// どれか 1 件でも失敗したらバッチ全体を取り消す（DATA-02）。RAW と JPEG のペアを
    /// まとめやすくするため、内部では RAW を先、JPEG を後に処理する。
    pub fn register_batch(&mut self, requests: &[RegisterFile]) -> Result<Vec<RegisterOutcome>> {
        for r in requests {
            r.validate()?;
        }
        let mut order: Vec<usize> = (0..requests.len()).collect();
        order.sort_by_key(|&i| match pair_class(&requests[i].name) {
            PairClass::Raw => 0,
            PairClass::Other => 1,
            PairClass::Jpeg => 2,
        });
        let now = now_utc_string();
        let tx = self.conn.transaction()?;
        let mut results: Vec<Option<RegisterOutcome>> = vec![None; requests.len()];
        for i in order {
            results[i] = Some(register_one(&tx, &requests[i], &now)?);
        }
        tx.commit()?;
        Ok(results
            .into_iter()
            .map(|r| r.expect("すべての要求を処理した"))
            .collect())
    }
}

fn folder_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Folder> {
    Ok(Folder {
        id: FolderId::new(row.get(0)?),
        volume_id: VolumeId::new(row.get(1)?),
        parent_id: row.get::<_, Option<i64>>(2)?.map(FolderId::new),
        rel_path: row.get(3)?,
    })
}

/// トランザクションの中でフォルダを確保する（`rel_path` は正規の形）。
pub(crate) fn ensure_folder_tx(
    tx: &Transaction<'_>,
    volume_id: VolumeId,
    rel_path: &str,
) -> Result<FolderId> {
    let mut select =
        tx.prepare_cached("SELECT id FROM folder WHERE volume_id = ?1 AND rel_path_key = ?2")?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO folder(volume_id, parent_id, rel_path, rel_path_key) VALUES (?1, ?2, ?3, ?4)",
    )?;
    // ルート（''）から順に、各階層を確保する。
    let mut prefixes = vec![""];
    if !rel_path.is_empty() {
        prefixes.extend(
            rel_path
                .match_indices('/')
                .map(|(i, _)| &rel_path[..i])
                .chain(std::iter::once(rel_path)),
        );
    }
    let mut parent: Option<i64> = None;
    for prefix in prefixes {
        let key = path_key(prefix);
        let id = match select
            .query_row(params![volume_id.get(), key], |row| row.get::<_, i64>(0))
            .optional()?
        {
            Some(id) => id,
            None => {
                insert.execute(params![volume_id.get(), parent, prefix, key])?;
                tx.last_insert_rowid()
            }
        };
        parent = Some(id);
    }
    Ok(FolderId::new(parent.expect("ルートは必ずある")))
}

/// 登録済みのファイルの行。
struct ExistingFile {
    id: i64,
    asset_id: i64,
    name: String,
    role: FileRole,
    size: i64,
    mtime: i64,
    quick_hash: String,
    revision: i64,
    status: FileStatus,
    status_reason: Option<String>,
}

fn register_one(tx: &Transaction<'_>, req: &RegisterFile, now: &str) -> Result<RegisterOutcome> {
    let name_key = path_key(&req.name);
    let existing = tx
        .prepare_cached(
            "SELECT id, asset_id, name, role, size, mtime, quick_hash, revision, status, status_reason
             FROM file WHERE folder_id = ?1 AND name_key = ?2",
        )?
        .query_row(params![req.folder_id.get(), name_key], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Option<String>>(9)?,
            ))
        })
        .optional()?;
    if let Some((id, asset_id, name, role, size, mtime, quick_hash, revision, status, reason)) =
        existing
    {
        let existing = ExistingFile {
            id,
            asset_id,
            name,
            role: parse_enum(&role)?,
            size,
            mtime,
            quick_hash,
            revision,
            status: parse_enum(&status)?,
            status_reason: reason,
        };
        return reregister(tx, req, existing, now);
    }

    let (status, reason) = requested_status(req);
    let size = u64_to_i64(req.facts.size, "ファイルサイズ")?;
    let class = pair_class(&req.name);

    // JPEG: 同じフォルダに同じ名前の RAW があれば、その asset の sidecar_jpeg にする。
    if req.kind == AssetKind::Photo
        && class == PairClass::Jpeg
        && let Some((raw_asset, _)) =
            find_pair_partner(tx, req.folder_id, &name_key, PairClass::Raw)?
    {
        let file_id = insert_file(
            tx,
            raw_asset,
            req,
            &name_key,
            FileRole::SidecarJpeg,
            size,
            status,
            reason,
            now,
        )?;
        refresh_asset_text(tx, raw_asset)?;
        return Ok(RegisterOutcome {
            status: RegisterStatus::Added,
            asset_id: AssetId::new(raw_asset),
            master_variant_id: master_variant(tx, raw_asset)?,
            file_id: FileId::new(file_id),
            role: FileRole::SidecarJpeg,
            revision: 1,
            paired: true,
        });
    }

    // RAW: 同じフォルダに同じ名前の JPEG だけの asset があれば、それに RAW を加えて主とする。
    if req.kind == AssetKind::Photo
        && class == PairClass::Raw
        && let Some((jpeg_asset, jpeg_file)) =
            find_pair_partner(tx, req.folder_id, &name_key, PairClass::Jpeg)?
    {
        tx.prepare_cached("UPDATE file SET role = 'sidecar_jpeg' WHERE id = ?1")?
            .execute([jpeg_file])?;
        let file_id = insert_file(
            tx,
            jpeg_asset,
            req,
            &name_key,
            FileRole::Primary,
            size,
            status,
            reason,
            now,
        )?;
        // メタデータは RAW のものにする（JPEG の撮影日時へのユーザーの修正は引き継ぐ）。
        let previous = load_capture(tx, jpeg_asset)?;
        update_asset_metadata(tx, jpeg_asset, req, Some(&previous))?;
        refresh_asset_text(tx, jpeg_asset)?;
        return Ok(RegisterOutcome {
            status: RegisterStatus::Added,
            asset_id: AssetId::new(jpeg_asset),
            master_variant_id: master_variant(tx, jpeg_asset)?,
            file_id: FileId::new(file_id),
            role: FileRole::Primary,
            revision: 1,
            paired: true,
        });
    }

    // 新しい asset ＋ マスターの variant ＋ file。
    tx.prepare_cached("INSERT INTO asset(kind, created_at) VALUES (?1, ?2)")?
        .execute(params![req.kind.as_str(), now])?;
    let asset_id = tx.last_insert_rowid();
    update_asset_metadata(tx, asset_id, req, None)?;
    let variant_id = insert_variant(
        tx,
        asset_id,
        true,
        None,
        &new_variant_develop(),
        HISTORY_LABEL_IMPORT,
        now,
    )?;
    let file_id = insert_file(
        tx,
        asset_id,
        req,
        &name_key,
        FileRole::Primary,
        size,
        status,
        reason,
        now,
    )?;
    refresh_asset_text(tx, asset_id)?;
    Ok(RegisterOutcome {
        status: RegisterStatus::Added,
        asset_id: AssetId::new(asset_id),
        master_variant_id: VariantId::new(variant_id),
        file_id: FileId::new(file_id),
        role: FileRole::Primary,
        revision: 1,
        paired: false,
    })
}

/// 登録の要求から、ファイルの状態と理由を決める。
fn requested_status(req: &RegisterFile) -> (FileStatus, Option<&str>) {
    match req.error.as_deref() {
        Some(reason) => (FileStatus::Error, Some(reason)),
        None => (FileStatus::Ok, None),
    }
}

/// 登録済みのファイルを登録し直す（冪等。変化があれば revision を 1 増やす）。
fn reregister(
    tx: &Transaction<'_>,
    req: &RegisterFile,
    existing: ExistingFile,
    now: &str,
) -> Result<RegisterOutcome> {
    let size = u64_to_i64(req.facts.size, "ファイルサイズ")?;
    let changed = existing.size != size
        || existing.mtime != req.facts.mtime_ns
        || existing.quick_hash != req.facts.quick_hash;
    let (status, reason) = requested_status(req);
    let mut revision = existing.revision;
    if changed {
        revision += 1;
        tx.prepare_cached(
            "UPDATE file SET size = ?2, mtime = ?3, quick_hash = ?4, full_hash = NULL,
                 revision = ?5, status = ?6, status_reason = ?7, status_at = ?8
             WHERE id = ?1",
        )?
        .execute(params![
            existing.id,
            size,
            req.facts.mtime_ns,
            req.facts.quick_hash,
            revision,
            status.as_str(),
            reason,
            now
        ])?;
        if existing.role == FileRole::Primary {
            let previous = load_capture(tx, existing.asset_id)?;
            update_asset_metadata(tx, existing.asset_id, req, Some(&previous))?;
        }
    } else if existing.status != status || existing.status_reason.as_deref() != reason {
        // 見つからなかった・読めなかったファイルが、元に戻った（またはその逆）。
        tx.prepare_cached(
            "UPDATE file SET status = ?2, status_reason = ?3, status_at = ?4 WHERE id = ?1",
        )?
        .execute(params![existing.id, status.as_str(), reason, now])?;
    }
    if existing.name != req.name {
        // 大文字・小文字や正規化の違いだけの名前の変更。表示用の名前を新しくする。
        tx.prepare_cached("UPDATE file SET name = ?2 WHERE id = ?1")?
            .execute(params![existing.id, req.name])?;
        refresh_asset_text(tx, existing.asset_id)?;
    }
    Ok(RegisterOutcome {
        status: if changed {
            RegisterStatus::Updated
        } else {
            RegisterStatus::Unchanged
        },
        asset_id: AssetId::new(existing.asset_id),
        master_variant_id: master_variant(tx, existing.asset_id)?,
        file_id: FileId::new(existing.id),
        role: existing.role,
        revision: i64_to_u32(revision, "revision")?,
        paired: false,
    })
}

/// ペアの相手を探す。戻り値は (asset の ID, 相手のファイルの ID)。
///
/// - `want = Raw`: 同じ名前の RAW で、主のファイルで、まだ JPEG を持たない asset。
/// - `want = Jpeg`: 同じ名前の JPEG で、それだけからなる写真の asset。
fn find_pair_partner(
    tx: &Transaction<'_>,
    folder_id: FolderId,
    name_key: &str,
    want: PairClass,
) -> Result<Option<(i64, i64)>> {
    let Some((stem, _)) = split_stem(name_key) else {
        return Ok(None);
    };
    // 名前が「stem.」で始まるもの（'/' は '.' の次の文字で、名前には含まれない）。
    let lower = format!("{stem}.");
    let upper = format!("{stem}/");
    let mut stmt = tx.prepare_cached(
        "SELECT f.id, f.asset_id, f.name_key,
                (SELECT count(*) FROM file g WHERE g.asset_id = f.asset_id),
                (SELECT a.kind FROM asset a WHERE a.id = f.asset_id)
         FROM file f
         WHERE f.folder_id = ?1 AND f.name_key > ?2 AND f.name_key < ?3 AND f.role = 'primary'
         ORDER BY f.id",
    )?;
    let rows = stmt.query_map(params![folder_id.get(), lower, upper], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    for r in rows {
        let (file_id, asset_id, key, file_count, kind) = r?;
        if split_stem(&key).map(|(s, _)| s) != Some(stem) || kind != AssetKind::Photo.as_str() {
            continue;
        }
        if pair_class(&key) != want {
            continue;
        }
        // RAW を探す場合は、その asset に JPEG がまだないこと（ファイルが RAW の 1 つだけ）。
        // JPEG を探す場合は、JPEG だけの asset であること。どちらもファイルが 1 つだけの asset。
        if file_count == 1 {
            return Ok(Some((asset_id, file_id)));
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn insert_file(
    tx: &Transaction<'_>,
    asset_id: i64,
    req: &RegisterFile,
    name_key: &str,
    role: FileRole,
    size: i64,
    status: FileStatus,
    reason: Option<&str>,
    now: &str,
) -> Result<i64> {
    tx.prepare_cached(
        "INSERT INTO file(asset_id, folder_id, name, name_key, role, size, mtime, quick_hash,
                          revision, status, status_reason, status_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?11)",
    )?
    .execute(params![
        asset_id,
        req.folder_id.get(),
        req.name,
        name_key,
        role.as_str(),
        size,
        req.facts.mtime_ns,
        req.facts.quick_hash,
        status.as_str(),
        reason,
        now
    ])?;
    Ok(tx.last_insert_rowid())
}

/// asset のマスターの variant。
pub(crate) fn master_variant(tx: &rusqlite::Connection, asset_id: i64) -> Result<VariantId> {
    tx.prepare_cached("SELECT id FROM variant WHERE asset_id = ?1 AND is_master = 1")?
        .query_row([asset_id], |row| row.get(0))
        .optional()?
        .map(VariantId::new)
        .ok_or_else(|| {
            CatalogError::Corrupt(format!(
                "asset {asset_id} にマスターの variant がありません"
            ))
        })
}

/// asset の撮影日時の列を読む。
pub(crate) fn load_capture(conn: &rusqlite::Connection, asset_id: i64) -> Result<CaptureTime> {
    let row = conn
        .prepare_cached(
            "SELECT captured_at_raw, captured_offset, tz_source, tz_assumed, time_correction_s,
                    captured_at_utc
             FROM asset WHERE id = ?1",
        )?
        .query_row([asset_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .optional()?
        .ok_or_else(|| CatalogError::NotFound(format!("asset {asset_id}")))?;
    capture_from_columns(row.0, row.1, &row.2, row.3, row.4, row.5.as_deref())
}

/// asset の撮影日時の列から [`CaptureTime`] を作る。
pub(crate) fn capture_from_columns(
    raw: Option<String>,
    offset: Option<String>,
    tz_source: &str,
    tz_assumed: Option<String>,
    correction_s: i64,
    utc: Option<&str>,
) -> Result<CaptureTime> {
    Ok(CaptureTime {
        raw,
        offset,
        tz_source: parse_enum(tz_source)?,
        tz_assumed,
        correction_s,
        utc: utc.map(crate::util::parse_db_utc).transpose()?,
    })
}

/// 内容が変わったファイルの新しい撮影日時に、ユーザーの修正（タイムゾーンの指定・時計のずれの補正。
/// LIB-16）を引き継ぐ。
fn carry_over_user_corrections(previous: &CaptureTime, new: &CaptureTime) -> CaptureTime {
    let mut out = new.clone();
    if previous.tz_source == TzSource::UserSet
        && let Some(offset) = previous
            .tz_assumed
            .as_deref()
            .and_then(|s| genzo_model::capture_time::parse_offset(s).ok())
        && let Ok(c) = out.with_user_offset(offset)
    {
        out = c;
    }
    if previous.correction_s != 0
        && let Ok(c) = out.with_correction(previous.correction_s)
    {
        out = c;
    }
    out
}

/// asset のメタデータ（撮影情報・寸法・撮影日時・動画の情報）を、登録の要求の内容で書く。
///
/// `previous` を渡した場合は、撮影日時のユーザーの修正を引き継ぐ。
fn update_asset_metadata(
    tx: &Transaction<'_>,
    asset_id: i64,
    req: &RegisterFile,
    previous: Option<&CaptureTime>,
) -> Result<()> {
    let capture = match previous {
        Some(p) => carry_over_user_corrections(p, &req.capture),
        None => req.capture.clone(),
    };
    let cols = AssetColumns::from_metadata(&req.metadata);
    tx.prepare_cached(
        "UPDATE asset SET
             captured_at_raw = ?2, captured_offset = ?3, tz_source = ?4, tz_assumed = ?5,
             time_correction_s = ?6, captured_at_utc = ?7,
             camera = ?8, lens = ?9, iso = ?10, aperture = ?11, shutter = ?12, focal = ?13,
             width = ?14, height = ?15, orientation = ?16, gps_lat = ?17, gps_lon = ?18
         WHERE id = ?1",
    )?
    .execute(params![
        asset_id,
        capture.raw,
        capture.offset,
        capture.tz_source.as_str(),
        capture.tz_assumed,
        capture.correction_s,
        capture.utc_db_string(),
        cols.camera,
        cols.lens,
        cols.iso,
        cols.aperture,
        cols.shutter,
        cols.focal,
        cols.width,
        cols.height,
        cols.orientation,
        cols.gps_lat,
        cols.gps_lon,
    ])?;
    if let MediaMetadata::Video(v) = &req.metadata {
        tx.prepare_cached(
            "INSERT INTO video_meta(asset_id, duration_s, fps, codec, bit_depth, color_transfer,
                                    color_primaries)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(asset_id) DO UPDATE SET
                 duration_s = excluded.duration_s, fps = excluded.fps, codec = excluded.codec,
                 bit_depth = excluded.bit_depth, color_transfer = excluded.color_transfer,
                 color_primaries = excluded.color_primaries",
        )?
        .execute(params![
            asset_id,
            v.duration_s.filter(|x| x.is_finite()),
            v.fps.filter(|x| x.is_finite()),
            v.codec,
            v.bit_depth.map(i64::from),
            v.color_transfer,
            v.color_primaries,
        ])?;
    }
    Ok(())
}

/// asset のテーブルに入れる撮影情報の列。
struct AssetColumns {
    camera: Option<String>,
    lens: Option<String>,
    iso: Option<i64>,
    aperture: Option<f64>,
    shutter: Option<f64>,
    focal: Option<f64>,
    width: Option<i64>,
    height: Option<i64>,
    orientation: i64,
    gps_lat: Option<f64>,
    gps_lon: Option<f64>,
}

impl AssetColumns {
    fn from_metadata(meta: &MediaMetadata) -> Self {
        let finite = |v: Option<f32>| v.map(f64::from).filter(|x| x.is_finite());
        match meta {
            MediaMetadata::Photo(p) => {
                let gps = p.gps.filter(|g| g.is_valid());
                Self {
                    camera: p.camera_name(),
                    lens: p
                        .lens
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                    iso: p.iso.map(i64::from),
                    aperture: finite(p.aperture),
                    shutter: finite(p.shutter_s),
                    focal: finite(p.focal_mm),
                    width: p.width.map(i64::from),
                    height: p.height.map(i64::from),
                    orientation: i64::from(p.orientation.to_exif()),
                    gps_lat: gps.map(|g| g.lat),
                    gps_lon: gps.map(|g| g.lon),
                }
            }
            MediaMetadata::Video(v) => Self {
                width: v.width.map(i64::from),
                height: v.height.map(i64::from),
                ..Self::empty()
            },
            MediaMetadata::None => Self::empty(),
        }
    }

    fn empty() -> Self {
        Self {
            camera: None,
            lens: None,
            iso: None,
            aperture: None,
            shutter: None,
            focal: None,
            width: None,
            height: None,
            orientation: 1,
            gps_lat: None,
            gps_lon: None,
        }
    }
}

/// asset のテキスト検索の対象（ファイル名とキャプション）を作り直す（3.6 節）。
///
/// 内容が変わらなければ書き込まない（FTS の索引を無駄に更新しないため）。
pub(crate) fn refresh_asset_text(conn: &rusqlite::Connection, asset_id: i64) -> Result<()> {
    let mut names_stmt = conn.prepare_cached(
        "SELECT name FROM file WHERE asset_id = ?1 ORDER BY role = 'primary' DESC, id",
    )?;
    let names = names_stmt
        .query_map([asset_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let caption: Option<String> = conn
        .prepare_cached("SELECT caption FROM asset WHERE id = ?1")?
        .query_row([asset_id], |row| row.get(0))?;
    let text = searchable_text(names.iter().map(String::as_str).chain(caption.as_deref()));
    conn.prepare_cached(
        "INSERT INTO asset_text(asset_id, text_norm) VALUES (?1, ?2)
         ON CONFLICT(asset_id) DO UPDATE SET text_norm = excluded.text_norm
         WHERE text_norm IS NOT excluded.text_norm",
    )?
    .execute(params![asset_id, text])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_classes_by_extension() {
        assert_eq!(pair_class("DSC00001.ARW"), PairClass::Raw);
        assert_eq!(pair_class("dsc00001.arw"), PairClass::Raw);
        assert_eq!(pair_class("IMG_0001.CR3"), PairClass::Raw);
        assert_eq!(pair_class("DSC00001.JPG"), PairClass::Jpeg);
        assert_eq!(pair_class("a.jpeg"), PairClass::Jpeg);
        assert_eq!(pair_class("clip.MP4"), PairClass::Other);
        assert_eq!(pair_class("noext"), PairClass::Other);
        assert_eq!(pair_class(".jpg"), PairClass::Other);
        assert_eq!(pair_class("a."), PairClass::Other);
    }

    #[test]
    fn rel_paths_are_validated() {
        assert_eq!(normalize_rel_path("").unwrap(), "");
        assert_eq!(normalize_rel_path("/").unwrap(), "");
        assert_eq!(normalize_rel_path("/a/b/").unwrap(), "a/b");
        assert!(normalize_rel_path("a//b").is_err());
        assert!(normalize_rel_path("a/../b").is_err());
        assert!(normalize_rel_path("./a").is_err());
        assert!(normalize_rel_path("a\0").is_err());
        assert_eq!(
            rel_path_from_path(Path::new("Photos/2024/京都")).unwrap(),
            "Photos/2024/京都"
        );
        assert_eq!(rel_path_from_path(Path::new("./a")).unwrap(), "a");
        assert!(rel_path_from_path(Path::new("../a")).is_err());
        #[cfg(unix)]
        assert!(rel_path_from_path(Path::new("/abs")).is_err());
    }

    #[test]
    fn file_names_are_validated() {
        assert!(validate_file_name("a.jpg").is_ok());
        for bad in ["", ".", "..", "a/b", "a\0"] {
            assert!(validate_file_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn user_corrections_are_carried_over() {
        use chrono::FixedOffset;
        let jst = FixedOffset::east_opt(9 * 3600).unwrap();
        let utc0 = FixedOffset::east_opt(0).unwrap();
        let original = CaptureTime::resolve(Some("2024:05:01 12:00:00"), None, jst).unwrap();
        let previous = original
            .with_user_offset(utc0)
            .unwrap()
            .with_correction(60)
            .unwrap();
        let new = CaptureTime::resolve(Some("2024:05:01 12:00:30"), None, jst).unwrap();
        let merged = carry_over_user_corrections(&previous, &new);
        assert_eq!(merged.tz_source, TzSource::UserSet);
        assert_eq!(merged.correction_s, 60);
        assert_eq!(
            merged.utc_db_string().as_deref(),
            Some("2024-05-01T12:01:30.000Z")
        );
        // 修正がなければそのまま。
        assert_eq!(carry_over_user_corrections(&original, &new), new);
    }
}
