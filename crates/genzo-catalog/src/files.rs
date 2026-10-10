//! ファイルの状態・削除・ファイル操作の記録（docs/04_architecture.md の 3.3 節・6.3 節・6.4 節）。
//!
//! **削除は 3 つの別々の操作**（6.4 節）:
//! - 仮想コピーの削除: [`Catalog::delete_virtual_copy`]（マスターは削除できない）。
//! - カタログからの除去: [`Catalog::remove_assets`]（asset ごと。元ファイルは触らない）。
//! - ゴミ箱へ移動: [`Catalog::trash_plan`] で影響を受けるファイルと variant の一覧を示して確認し、
//!   [`Catalog::plan_trash`] で `file_op` に予定を記録する。OS のゴミ箱への移動は呼び出し側が行い、
//!   成功したら [`Catalog::complete_file_op`] でカタログから除く（同じトランザクション）。
//!
//! **ファイル操作の状態遷移**（6.4 節。DATA-07）: planned → executing → done / failed。
//! 遷移の可否は [`FileOpState::can_transition_to`] で判定する。起動時は
//! [`Catalog::unfinished_file_ops`] で planned / executing のままの記録を列挙し、呼び出し側が
//! 実際のファイルの場所を確認して done か failed に確定させる。

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use genzo_model::{
    AssetId, FileId, FileOpId, FileOpKind, FileOpState, FileRole, FileStatus, FolderId, VariantId,
    VolumeId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::develop::VariantInfo;
use crate::error::{CatalogError, Result};
use crate::hash::{FileFacts, is_hex64};
use crate::register::refresh_asset_text;
use crate::text::path_key;
use crate::util::{i64_to_u32, ids_to_json, now_utc_string, parse_db_utc, parse_enum, u64_to_i64};

/// ファイルの行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    /// ID。
    pub id: FileId,
    /// asset。
    pub asset_id: AssetId,
    /// フォルダ。
    pub folder_id: FolderId,
    /// 名前（表示用）。
    pub name: String,
    /// 役割。
    pub role: FileRole,
    /// 変化の検知に使う値。
    pub facts: FileFacts,
    /// 全体のハッシュ（計算していなければ `None`）。
    pub full_hash: Option<String>,
    /// リビジョン。
    pub revision: u32,
    /// 状態。
    pub status: FileStatus,
    /// missing / error の理由。
    pub status_reason: Option<String>,
}

/// ファイルの場所（ボリューム ＋ フォルダの相対パス ＋ 名前。3.3 節）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileLocation {
    /// ボリューム。
    pub volume_id: VolumeId,
    /// OS のボリューム ID。
    pub volume_uuid: String,
    /// 最後に確認したマウント先。
    pub mount_path: Option<String>,
    /// フォルダ。
    pub folder_id: FolderId,
    /// フォルダのボリューム内の相対パス（'/' 区切り）。
    pub rel_dir: String,
    /// ファイル名。
    pub name: String,
}

impl FileLocation {
    /// ボリューム内の相対パス（'/' 区切り）。
    pub fn rel_path(&self) -> String {
        if self.rel_dir.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.rel_dir, self.name)
        }
    }

    /// マウント先から組み立てた絶対パス（マウント先が分からなければ `None`）。
    pub fn absolute_path(&self) -> Option<PathBuf> {
        let mut p = PathBuf::from(self.mount_path.as_deref()?);
        for part in self.rel_dir.split('/').filter(|s| !s.is_empty()) {
            p.push(part);
        }
        p.push(&self.name);
        Some(p)
    }
}

/// ファイルの内容の変化の確認の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactsChange {
    /// 変化はない。
    Unchanged,
    /// 変化を検知し、リビジョンを上げた（キャッシュを無効にする。4.1 節）。
    Updated {
        /// 新しいリビジョン。
        revision: u32,
    },
}

/// カタログからの除去の結果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemovalReport {
    /// 除いた asset。
    pub assets: Vec<AssetId>,
    /// 除いた variant（サムネイル DB・プレビューの回収用）。
    pub variants: Vec<VariantId>,
    /// 除いたファイル。
    pub files: Vec<FileId>,
}

/// ゴミ箱へ移すファイル 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// ファイル。
    pub file_id: FileId,
    /// asset。
    pub asset_id: AssetId,
    /// 役割。
    pub role: FileRole,
    /// 場所。
    pub location: FileLocation,
}

/// ゴミ箱への移動の影響の一覧（6.4 節。実行前にユーザーに示して確認する）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrashPlan {
    /// 対象の asset。
    pub assets: Vec<AssetId>,
    /// ゴミ箱へ移すファイル（RAW と JPEG のペアなど、asset に属するすべてのファイル）。
    pub files: Vec<PlannedFile>,
    /// カタログから消える variant（仮想コピーを含む）。
    pub variants: Vec<VariantInfo>,
}

/// ファイル操作の対象の場所（記録用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOpPath {
    /// フォルダ。
    pub folder_id: FolderId,
    /// ファイル名。
    pub name: String,
    /// 予定を記録したときの絶対パス（マウント先が分からなければ `None`）。
    pub absolute_path: Option<String>,
}

/// ファイル操作の対象の 1 ファイル。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOpEntry {
    /// ファイル。
    pub file_id: FileId,
    /// asset。
    pub asset_id: AssetId,
    /// 元の場所。
    pub from: FileOpPath,
    /// 移動・リネーム先（ゴミ箱への移動では `None`）。
    pub to: Option<FileOpPath>,
}

/// ファイル操作の内容（`file_op.payload_json`）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileOpPayload {
    /// 対象のファイル。
    pub entries: Vec<FileOpEntry>,
}

/// ファイル操作の記録。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOpRecord {
    /// ID。
    pub id: FileOpId,
    /// 種類。
    pub kind: FileOpKind,
    /// 状態。
    pub state: FileOpState,
    /// 内容。
    pub payload: FileOpPayload,
    /// 失敗の理由。
    pub error: Option<String>,
    /// 記録した日時。
    pub created_at: DateTime<Utc>,
    /// 最後に状態を変えた日時。
    pub updated_at: DateTime<Utc>,
}

const FILE_COLUMNS: &str = "id, asset_id, folder_id, name, role, size, mtime, quick_hash, full_hash, revision, status, status_reason";

fn file_from_row(row: &rusqlite::Row<'_>) -> Result<FileRecord> {
    let role: String = row.get(4)?;
    let size: i64 = row.get(5)?;
    let status: String = row.get(10)?;
    Ok(FileRecord {
        id: FileId::new(row.get(0)?),
        asset_id: AssetId::new(row.get(1)?),
        folder_id: FolderId::new(row.get(2)?),
        name: row.get(3)?,
        role: parse_enum(&role)?,
        facts: FileFacts {
            size: u64::try_from(size)
                .map_err(|_| CatalogError::Corrupt(format!("ファイルサイズが負です: {size}")))?,
            mtime_ns: row.get(6)?,
            quick_hash: row.get(7)?,
        },
        full_hash: row.get(8)?,
        revision: i64_to_u32(row.get(9)?, "revision")?,
        status: parse_enum(&status)?,
        status_reason: row.get(11)?,
    })
}

fn location(conn: &Connection, file_id: FileId) -> Result<FileLocation> {
    conn.prepare_cached(
        "SELECT v.id, v.uuid, v.last_mount_path, d.id, d.rel_path, f.name
         FROM file f JOIN folder d ON d.id = f.folder_id JOIN volume v ON v.id = d.volume_id
         WHERE f.id = ?1",
    )?
    .query_row([file_id.get()], |row| {
        Ok(FileLocation {
            volume_id: VolumeId::new(row.get(0)?),
            volume_uuid: row.get(1)?,
            mount_path: row.get(2)?,
            folder_id: FolderId::new(row.get(3)?),
            rel_dir: row.get(4)?,
            name: row.get(5)?,
        })
    })
    .optional()?
    .ok_or_else(|| CatalogError::NotFound(format!("ファイル {file_id}")))
}

/// フォルダの絶対パス上の、`name` のファイルの絶対パス（記録用）。
fn folder_file_path(conn: &Connection, folder_id: FolderId, name: &str) -> Result<Option<String>> {
    let (mount, rel_dir): (Option<String>, String) = conn
        .prepare_cached(
            "SELECT v.last_mount_path, d.rel_path FROM folder d JOIN volume v ON v.id = d.volume_id
             WHERE d.id = ?1",
        )?
        .query_row([folder_id.get()], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?
        .ok_or_else(|| CatalogError::NotFound(format!("フォルダ {folder_id}")))?;
    let loc = FileLocation {
        volume_id: VolumeId::new(0),
        volume_uuid: String::new(),
        mount_path: mount,
        folder_id,
        rel_dir,
        name: name.to_owned(),
    };
    Ok(loc
        .absolute_path()
        .map(|p| p.to_string_lossy().into_owned()))
}

impl Catalog {
    /// ファイルの行を読む。
    pub fn file(&self, file_id: FileId) -> Result<FileRecord> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT {FILE_COLUMNS} FROM file WHERE id = ?1"))?;
        let mut rows = stmt.query([file_id.get()])?;
        match rows.next()? {
            Some(row) => file_from_row(row),
            None => Err(CatalogError::NotFound(format!("ファイル {file_id}"))),
        }
    }

    /// asset のファイル（主となるファイルが先）。
    pub fn files_of_asset(&self, asset_id: AssetId) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {FILE_COLUMNS} FROM file WHERE asset_id = ?1 ORDER BY role = 'primary' DESC, id"
        ))?;
        let mut rows = stmt.query([asset_id.get()])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(file_from_row(row)?);
        }
        Ok(out)
    }

    /// ファイルの場所。
    pub fn file_location(&self, file_id: FileId) -> Result<FileLocation> {
        location(&self.conn, file_id)
    }

    /// 指定した状態のファイルの一覧（ID の順）。
    pub fn files_with_status(&self, status: FileStatus) -> Result<Vec<FileId>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM file WHERE status = ?1 ORDER BY id")?;
        let rows = stmt.query_map([status.as_str()], |row| row.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(FileId::new(r?));
        }
        Ok(out)
    }

    /// ファイルの状態と理由を記録する（missing / error。6.3 節）。`ok` では理由を消す。
    pub fn set_file_status(
        &mut self,
        file_id: FileId,
        status: FileStatus,
        reason: Option<&str>,
    ) -> Result<()> {
        let reason = if status == FileStatus::Ok {
            None
        } else {
            reason
        };
        let n = self.conn.execute(
            "UPDATE file SET status = ?2, status_reason = ?3, status_at = ?4 WHERE id = ?1",
            params![file_id.get(), status.as_str(), reason, now_utc_string()],
        )?;
        if n == 0 {
            return Err(CatalogError::NotFound(format!("ファイル {file_id}")));
        }
        Ok(())
    }

    /// ファイルの現在のサイズ・更新日時・クイックハッシュを記録する（3.3 節）。
    ///
    /// いずれかが変わっていたら `revision` を 1 増やし、全体のハッシュを消す。
    /// 見つかったファイルとして状態を `ok` にする。
    pub fn update_file_facts(&mut self, file_id: FileId, facts: &FileFacts) -> Result<FactsChange> {
        if !is_hex64(&facts.quick_hash) {
            return Err(CatalogError::InvalidInput(format!(
                "クイックハッシュが 16 進数の小文字 64 文字ではありません: {:?}",
                facts.quick_hash
            )));
        }
        let size = u64_to_i64(facts.size, "ファイルサイズ")?;
        let now = now_utc_string();
        let tx = self.conn.transaction()?;
        let current = {
            let mut stmt =
                tx.prepare_cached(&format!("SELECT {FILE_COLUMNS} FROM file WHERE id = ?1"))?;
            let mut rows = stmt.query([file_id.get()])?;
            match rows.next()? {
                Some(row) => file_from_row(row)?,
                None => return Err(CatalogError::NotFound(format!("ファイル {file_id}"))),
            }
        };
        let change = if current.facts.differs_from(facts) {
            let revision = current.revision + 1;
            tx.execute(
                "UPDATE file SET size = ?2, mtime = ?3, quick_hash = ?4, full_hash = NULL,
                     revision = ?5, status = 'ok', status_reason = NULL, status_at = ?6
                 WHERE id = ?1",
                params![
                    file_id.get(),
                    size,
                    facts.mtime_ns,
                    facts.quick_hash,
                    revision,
                    now
                ],
            )?;
            FactsChange::Updated { revision }
        } else {
            tx.execute(
                "UPDATE file SET status = 'ok', status_reason = NULL, status_at = ?2 WHERE id = ?1",
                params![file_id.get(), now],
            )?;
            FactsChange::Unchanged
        };
        tx.commit()?;
        Ok(change)
    }

    /// 全体のハッシュを記録する（3.3 節。再リンクの確定・コピーの検証で計算したもの）。
    pub fn set_full_hash(&mut self, file_id: FileId, full_hash: &str) -> Result<()> {
        if !is_hex64(full_hash) {
            return Err(CatalogError::InvalidInput(format!(
                "全体のハッシュが 16 進数の小文字 64 文字ではありません: {full_hash:?}"
            )));
        }
        let n = self.conn.execute(
            "UPDATE file SET full_hash = ?2 WHERE id = ?1",
            params![file_id.get(), full_hash],
        )?;
        if n == 0 {
            return Err(CatalogError::NotFound(format!("ファイル {file_id}")));
        }
        Ok(())
    }

    /// asset をカタログから除く（6.4 節の「カタログからの除去」。元ファイルは変更しない）。
    ///
    /// ファイル・variant・履歴・スナップショット・キーワードの付与・コレクションの所属・
    /// 動画の情報・テキスト検索の索引も削除される（ON DELETE CASCADE とトリガー）。
    /// キーワードとコレクション自体は残る。存在しない asset は無視する。
    pub fn remove_assets(&mut self, asset_ids: &[AssetId]) -> Result<RemovalReport> {
        let tx = self.conn.transaction()?;
        let report = remove_assets_tx(&tx, asset_ids)?;
        tx.commit()?;
        Ok(report)
    }

    /// ゴミ箱への移動で影響を受けるファイルと variant の一覧を返す（6.4 節。変更はしない）。
    pub fn trash_plan(&self, asset_ids: &[AssetId]) -> Result<TrashPlan> {
        trash_plan(&self.conn, asset_ids)
    }

    /// ゴミ箱への移動の予定を `file_op` に記録する（planned）。記録した ID と影響の一覧を返す。
    pub fn plan_trash(&mut self, asset_ids: &[AssetId]) -> Result<(FileOpId, TrashPlan)> {
        let tx = self.conn.transaction()?;
        let plan = trash_plan(&tx, asset_ids)?;
        if plan.files.is_empty() {
            return Err(CatalogError::InvalidInput(
                "ゴミ箱へ移すファイルがありません".to_owned(),
            ));
        }
        let payload = FileOpPayload {
            entries: plan
                .files
                .iter()
                .map(|f| FileOpEntry {
                    file_id: f.file_id,
                    asset_id: f.asset_id,
                    from: FileOpPath {
                        folder_id: f.location.folder_id,
                        name: f.location.name.clone(),
                        absolute_path: f
                            .location
                            .absolute_path()
                            .map(|p| p.to_string_lossy().into_owned()),
                    },
                    to: None,
                })
                .collect(),
        };
        let id = insert_file_op(&tx, FileOpKind::Trash, &payload)?;
        tx.commit()?;
        Ok((id, plan))
    }

    /// ファイルを別のフォルダへ移動する予定を記録する（FILE-02）。
    ///
    /// 移動先に、カタログ上で同じ名前のファイルがあればエラー（[`CatalogError::Conflict`]）。
    pub fn plan_move(&mut self, file_ids: &[FileId], dest_folder: FolderId) -> Result<FileOpId> {
        let tx = self.conn.transaction()?;
        let mut entries = Vec::with_capacity(file_ids.len());
        for &file_id in file_ids {
            let name: String = tx
                .query_row(
                    "SELECT name FROM file WHERE id = ?1",
                    [file_id.get()],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| CatalogError::NotFound(format!("ファイル {file_id}")))?;
            entries.push(planned_entry(&tx, file_id, dest_folder, &name)?);
        }
        check_destinations(&tx, &entries)?;
        let id = insert_file_op(&tx, FileOpKind::Move, &FileOpPayload { entries })?;
        tx.commit()?;
        Ok(id)
    }

    /// ファイルの名前を変える予定を記録する（FILE-02）。`renames` は (ファイル, 新しい名前)。
    pub fn plan_rename(&mut self, renames: &[(FileId, String)]) -> Result<FileOpId> {
        let tx = self.conn.transaction()?;
        let mut entries = Vec::with_capacity(renames.len());
        for (file_id, new_name) in renames {
            if new_name.is_empty()
                || new_name == "."
                || new_name == ".."
                || new_name.contains(['/', '\0'])
            {
                return Err(CatalogError::InvalidInput(format!(
                    "ファイル名が不正です: {new_name:?}"
                )));
            }
            let folder: i64 = tx
                .query_row(
                    "SELECT folder_id FROM file WHERE id = ?1",
                    [file_id.get()],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| CatalogError::NotFound(format!("ファイル {file_id}")))?;
            entries.push(planned_entry(
                &tx,
                *file_id,
                FolderId::new(folder),
                new_name,
            )?);
        }
        check_destinations(&tx, &entries)?;
        let id = insert_file_op(&tx, FileOpKind::Rename, &FileOpPayload { entries })?;
        tx.commit()?;
        Ok(id)
    }

    /// ファイル操作の記録を読む。
    pub fn file_op(&self, id: FileOpId) -> Result<FileOpRecord> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, kind, state, payload_json, error, created_at, updated_at FROM file_op WHERE id = ?1",
        )?;
        let mut rows = stmt.query([id.get()])?;
        match rows.next()? {
            Some(row) => file_op_from_row(row),
            None => Err(CatalogError::NotFound(format!("ファイル操作 {id}"))),
        }
    }

    /// 終わっていない（planned / executing のままの）ファイル操作の記録を、古い順に返す（DATA-07）。
    ///
    /// 起動時に呼び、呼び出し側が実際のファイルの場所を確認して、[`Catalog::complete_file_op`] か
    /// [`Catalog::fail_file_op`] で確定させる。
    pub fn unfinished_file_ops(&self) -> Result<Vec<FileOpRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, kind, state, payload_json, error, created_at, updated_at FROM file_op
             WHERE state IN ('planned', 'executing') ORDER BY id",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(file_op_from_row(row)?);
        }
        Ok(out)
    }

    /// OS の操作を始める前に、状態を executing にする。
    pub fn start_file_op(&mut self, id: FileOpId) -> Result<()> {
        self.transition_file_op(id, FileOpState::Executing, None)
    }

    /// OS の操作が成功したので、カタログを更新して done にする（同じトランザクション）。
    ///
    /// - ゴミ箱: 対象の asset をカタログから除く。
    /// - 移動・リネーム: ファイルのフォルダと名前を新しくし、テキスト検索の索引も更新する。
    ///
    /// どちらも、予定の後にカタログから除かれた asset・ファイルは飛ばす（更新するものがないため）。
    pub fn complete_file_op(&mut self, id: FileOpId) -> Result<()> {
        self.transition_file_op(id, FileOpState::Done, None)
    }

    /// OS の操作が失敗したので failed にする（カタログは元のまま）。
    pub fn fail_file_op(&mut self, id: FileOpId, error: &str) -> Result<()> {
        self.transition_file_op(id, FileOpState::Failed, Some(error))
    }

    /// ファイル操作の状態を変える（遷移の可否は [`FileOpState::can_transition_to`]）。
    ///
    /// `done` にするときは、カタログの更新を同じトランザクションで行う。
    pub fn transition_file_op(
        &mut self,
        id: FileOpId,
        next: FileOpState,
        error: Option<&str>,
    ) -> Result<()> {
        let now = now_utc_string();
        let tx = self.conn.transaction()?;
        let (kind, state, payload): (String, String, String) = tx
            .query_row(
                "SELECT kind, state, payload_json FROM file_op WHERE id = ?1",
                [id.get()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("ファイル操作 {id}")))?;
        let current: FileOpState = parse_enum(&state)?;
        if !current.can_transition_to(next) {
            return Err(CatalogError::InvalidFileOpTransition {
                id,
                from: current,
                to: next,
            });
        }
        if next == FileOpState::Done {
            let kind: FileOpKind = parse_enum(&kind)?;
            let payload: FileOpPayload = serde_json::from_str(&payload)?;
            apply_file_op(&tx, kind, &payload)?;
        }
        tx.execute(
            "UPDATE file_op SET state = ?2, error = ?3, updated_at = ?4 WHERE id = ?1",
            params![id.get(), next.as_str(), error, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 終わった（done / failed の）ファイル操作の記録のうち、新しい `keep` 件を残して削除する。
    pub fn purge_finished_file_ops(&mut self, keep: usize) -> Result<usize> {
        let keep = i64::try_from(keep).unwrap_or(i64::MAX);
        Ok(self.conn.execute(
            "DELETE FROM file_op WHERE state IN ('done', 'failed') AND id NOT IN (
                 SELECT id FROM file_op WHERE state IN ('done', 'failed') ORDER BY id DESC LIMIT ?1)",
            [keep],
        )?)
    }
}

/// トランザクションの中で asset を除く。
pub(crate) fn remove_assets_tx(
    tx: &Transaction<'_>,
    asset_ids: &[AssetId],
) -> Result<RemovalReport> {
    let json = ids_to_json(asset_ids);
    let collect = |sql: &str| -> Result<Vec<i64>> {
        let mut stmt = tx.prepare_cached(sql)?;
        let rows = stmt.query_map([&json], |row| row.get::<_, i64>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    let assets =
        collect("SELECT id FROM asset WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id")?;
    let variants = collect(
        "SELECT id FROM variant WHERE asset_id IN (SELECT value FROM json_each(?1)) ORDER BY id",
    )?;
    let files = collect(
        "SELECT id FROM file WHERE asset_id IN (SELECT value FROM json_each(?1)) ORDER BY id",
    )?;
    tx.execute(
        "DELETE FROM asset WHERE id IN (SELECT value FROM json_each(?1))",
        [&json],
    )?;
    Ok(RemovalReport {
        assets: assets.into_iter().map(AssetId::new).collect(),
        variants: variants.into_iter().map(VariantId::new).collect(),
        files: files.into_iter().map(FileId::new).collect(),
    })
}

fn trash_plan(conn: &Connection, asset_ids: &[AssetId]) -> Result<TrashPlan> {
    let json = ids_to_json(asset_ids);
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM asset WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id",
    )?;
    let assets = stmt
        .query_map([&json], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut plan = TrashPlan::default();
    for a in assets {
        let asset_id = AssetId::new(a);
        plan.assets.push(asset_id);
        let mut fstmt = conn.prepare_cached(
            "SELECT id, role FROM file WHERE asset_id = ?1 ORDER BY role = 'primary' DESC, id",
        )?;
        let files = fstmt
            .query_map([a], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (file_id, role) in files {
            let file_id = FileId::new(file_id);
            plan.files.push(PlannedFile {
                file_id,
                asset_id,
                role: parse_enum(&role)?,
                location: location(conn, file_id)?,
            });
        }
        let mut vstmt = conn.prepare_cached(
            "SELECT id, is_master, name FROM variant WHERE asset_id = ?1 ORDER BY is_master DESC, id",
        )?;
        let variants = vstmt
            .query_map([a], |row| {
                Ok(VariantInfo {
                    id: VariantId::new(row.get(0)?),
                    asset_id,
                    is_master: row.get(1)?,
                    name: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        plan.variants.extend(variants);
    }
    Ok(plan)
}

/// 移動・リネームの 1 ファイルの予定を作る。
fn planned_entry(
    tx: &Transaction<'_>,
    file_id: FileId,
    dest_folder: FolderId,
    new_name: &str,
) -> Result<FileOpEntry> {
    let loc = location(tx, file_id)?;
    let asset_id: i64 = tx.query_row(
        "SELECT asset_id FROM file WHERE id = ?1",
        [file_id.get()],
        |row| row.get(0),
    )?;
    Ok(FileOpEntry {
        file_id,
        asset_id: AssetId::new(asset_id),
        from: FileOpPath {
            folder_id: loc.folder_id,
            name: loc.name.clone(),
            absolute_path: loc
                .absolute_path()
                .map(|p| p.to_string_lossy().into_owned()),
        },
        to: Some(FileOpPath {
            folder_id: dest_folder,
            name: new_name.to_owned(),
            absolute_path: folder_file_path(tx, dest_folder, new_name)?,
        }),
    })
}

/// 移動・リネーム先に、この操作の対象以外の同じ名前のファイルがないかを確かめる。
fn check_destinations(tx: &Transaction<'_>, entries: &[FileOpEntry]) -> Result<()> {
    let own: Vec<i64> = entries.iter().map(|e| e.file_id.get()).collect();
    let mut seen = std::collections::HashSet::new();
    for e in entries {
        let to = e.to.as_ref().expect("移動・リネームには移動先がある");
        let key = path_key(&to.name);
        if !seen.insert((to.folder_id, key.clone())) {
            return Err(CatalogError::Conflict(to.name.clone()));
        }
        let other: Option<i64> = tx
            .prepare_cached("SELECT id FROM file WHERE folder_id = ?1 AND name_key = ?2")?
            .query_row(params![to.folder_id.get(), key], |row| row.get(0))
            .optional()?;
        if let Some(other) = other
            && !own.contains(&other)
        {
            return Err(CatalogError::Conflict(to.name.clone()));
        }
    }
    Ok(())
}

fn insert_file_op(
    tx: &Transaction<'_>,
    kind: FileOpKind,
    payload: &FileOpPayload,
) -> Result<FileOpId> {
    let now = now_utc_string();
    tx.execute(
        "INSERT INTO file_op(kind, state, payload_json, created_at, updated_at)
         VALUES (?1, 'planned', ?2, ?3, ?3)",
        params![kind.as_str(), serde_json::to_string(payload)?, now],
    )?;
    Ok(FileOpId::new(tx.last_insert_rowid()))
}

fn file_op_from_row(row: &rusqlite::Row<'_>) -> Result<FileOpRecord> {
    let kind: String = row.get(1)?;
    let state: String = row.get(2)?;
    let payload: String = row.get(3)?;
    let created_at: String = row.get(5)?;
    let updated_at: String = row.get(6)?;
    Ok(FileOpRecord {
        id: FileOpId::new(row.get(0)?),
        kind: parse_enum(&kind)?,
        state: parse_enum(&state)?,
        payload: serde_json::from_str(&payload)?,
        error: row.get(4)?,
        created_at: parse_db_utc(&created_at)?,
        updated_at: parse_db_utc(&updated_at)?,
    })
}

/// done にするときのカタログの更新。
fn apply_file_op(tx: &Transaction<'_>, kind: FileOpKind, payload: &FileOpPayload) -> Result<()> {
    match kind {
        FileOpKind::Trash => {
            let mut assets: Vec<AssetId> = payload.entries.iter().map(|e| e.asset_id).collect();
            assets.sort();
            assets.dedup();
            remove_assets_tx(tx, &assets)?;
        }
        FileOpKind::Move | FileOpKind::Rename => {
            // 同じ操作の中で名前を入れ替える場合（A → B、B → A）に、途中で UNIQUE(folder_id, name_key)
            // に違反しないよう、先に比較キーを一時的な値（NUL は名前に含まれないので衝突しない）にする。
            for e in &payload.entries {
                tx.execute(
                    "UPDATE file SET name_key = char(0) || id WHERE id = ?1",
                    [e.file_id.get()],
                )?;
            }
            let mut touched = Vec::new();
            for e in &payload.entries {
                let to = e.to.as_ref().ok_or_else(|| {
                    CatalogError::Corrupt(format!(
                        "ファイル操作に移動先がありません（ファイル {}）",
                        e.file_id
                    ))
                })?;
                // 予定の後にカタログから除かれたファイル（asset の除去など）は、更新するものがない
                // ので飛ばす（ゴミ箱の done と同じく、存在しないものは無視する）。OS の操作は
                // 成功しているので、NotFound にすると記録を done にできなくなる（レビューで再現）。
                // テキスト検索の索引は、記録の asset ではなく、ファイルの現在の asset で作り直す。
                let asset: Option<i64> = tx
                    .prepare_cached(
                        "UPDATE file SET folder_id = ?2, name = ?3, name_key = ?4 WHERE id = ?1
                         RETURNING asset_id",
                    )?
                    .query_row(
                        params![
                            e.file_id.get(),
                            to.folder_id.get(),
                            to.name,
                            path_key(&to.name)
                        ],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(asset) = asset {
                    touched.push(asset);
                }
            }
            touched.sort_unstable();
            touched.dedup();
            for a in touched {
                refresh_asset_text(tx, a)?;
            }
        }
    }
    Ok(())
}
