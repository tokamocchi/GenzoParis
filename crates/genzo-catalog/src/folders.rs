//! ボリューム（外付けドライブの識別。FILE-03）とフォルダ（作成・移動・名前の変更・場所の指定し直し・見つからない
//! 印。FILE-02・FILE-04）。docs/04_architecture.md の 3.3 節・3.14 節。
//!
//! - **ボリューム**: ID（`volume.uuid`）の形とつながっているかの判定は genzo-api が OS に問い合わせて決める。
//!   この crate は、記録（ラベル・最後のマウント先・ファイルシステムの種類・最後につながった日時。スキーマの版 12）
//!   と、開いている間のつながっていないボリュームの集まり（[`Catalog::set_offline_volumes`]）を持つ。つながって
//!   いないボリュームのファイル・フォルダの場所は、マウント先が分からないもの（`None`）として返す（記録した
//!   マウント先に別のドライブがつながっていても、そのファイルを読まない）。
//! - **フォルダの付け替え**（[`rebase_folder_tx`]）: フォルダと配下のフォルダの相対パス（と移動先のボリューム）を
//!   付け替える。ファイルはフォルダの ID で結び付いているので変えない。OS の名前の変更の後（ファイル操作の
//!   done。files.rs）と、場所の指定し直し（[`Catalog::relocate_folder`]。カタログの中だけ）で使う。
//! - **パスから作った ID のボリュームの移し替え**（[`Catalog::rebind_folders`]）: 版 11 までの `path:` の
//!   ボリュームのフォルダを、OS のボリューム ID のボリュームへ移す（同じ場所のフォルダが既にあれば、ファイルを
//!   そちらへまとめる）。
//! - **フォルダをカタログから除く**（[`Catalog::remove_folder`]）: 写真のファイルが登録されていないフォルダ
//!   （配下を含む）だけを除く（見つからないフォルダ・再リンクで空になったフォルダの片付け）。

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use genzo_model::{FileId, FolderId, FolderStatus, VolumeId};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::files::check_file_name;
use crate::register::{Folder, ensure_folder_tx, normalize_rel_path};
use crate::text::path_key;
use crate::util::{ids_to_json, now_utc_string, parse_db_utc, parse_enum};

/// ボリュームの記録（スキーマの版 12 の列を含む）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeRecord {
    /// ID。
    pub id: VolumeId,
    /// OS のボリューム ID（`path:` で始まるものはパスの先頭から作ったもの）。
    pub uuid: String,
    /// ラベル（ボリュームの名前）。
    pub label: Option<String>,
    /// 最後に確認したマウント先。
    pub last_mount_path: Option<String>,
    /// ファイルシステムの種類（ext4・apfs・NTFS・exFAT など）。
    pub fs_type: Option<String>,
    /// つながっていることを最後に確かめた日時。
    pub last_seen_at: Option<DateTime<Utc>>,
}

/// フォルダの記録（状態を含む。スキーマの版 12）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRecord {
    /// フォルダ。
    pub folder: Folder,
    /// 状態（見つからない印）。
    pub status: FolderStatus,
}

/// フォルダの移し替えの 1 件（[`Catalog::rebind_folders`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRebind {
    /// フォルダ。
    pub folder_id: FolderId,
    /// 移し先のボリューム。
    pub volume_id: VolumeId,
    /// 移し先のボリューム内の相対パス（'/' 区切り。ルートは空）。
    pub rel_path: String,
}

/// フォルダの移し替えの結果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RebindReport {
    /// 移したフォルダの数。
    pub moved: u64,
    /// 同じ場所のフォルダが既にあったため、ファイルをそちらへまとめたフォルダの数。
    pub merged: u64,
    /// まとめる先に同じ名前のファイルが登録されていたため、移せなかったファイル（元のフォルダに残す）。
    pub conflicting_files: Vec<FileId>,
    /// 空になったので消したフォルダの数。
    pub removed_folders: u64,
    /// フォルダがなくなったので消したボリューム。
    pub removed_volumes: Vec<VolumeId>,
}

/// フォルダをカタログから除いた結果（[`Catalog::remove_folder`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderRemoval {
    /// 除いたフォルダの数（配下を含む）。
    pub folders: u64,
    /// 除いたボリューム（ボリュームのルートを除いたとき）。
    pub volume: Option<VolumeId>,
}

/// フォルダと配下のフォルダ（id と相対パス。最初が自分）。
fn subtree(conn: &Connection, folder_id: FolderId) -> Result<(Folder, Vec<(i64, String)>)> {
    let folder = conn
        .query_row(
            "SELECT id, volume_id, parent_id, rel_path FROM folder WHERE id = ?1",
            [folder_id.get()],
            |row| {
                Ok(Folder {
                    id: FolderId::new(row.get(0)?),
                    volume_id: VolumeId::new(row.get(1)?),
                    parent_id: row.get::<_, Option<i64>>(2)?.map(FolderId::new),
                    rel_path: row.get(3)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| CatalogError::NotFound(format!("フォルダ {folder_id}")))?;
    let mut members = vec![(folder.id.get(), folder.rel_path.clone())];
    let mut stmt = if folder.rel_path.is_empty() {
        // ボリュームのルート: ボリュームのすべてのフォルダ。
        conn.prepare_cached(
            "SELECT id, rel_path FROM folder WHERE volume_id = ?1 AND rel_path_key <> ?2
             ORDER BY rel_path_key",
        )?
    } else {
        // 比較キーが「key/」で始まるもの（'0' は '/' の次の文字）。
        conn.prepare_cached(
            "SELECT id, rel_path FROM folder
             WHERE volume_id = ?1 AND rel_path_key > ?2 || '/' AND rel_path_key < ?2 || '0'
             ORDER BY rel_path_key",
        )?
    };
    let rows = stmt.query_map(
        params![folder.volume_id.get(), path_key(&folder.rel_path)],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    )?;
    for r in rows {
        members.push(r?);
    }
    Ok((folder, members))
}

/// 相対パスの親（最後の部分を除いたもの。1 つの部分なら空）。
fn parent_rel(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(p, _)| p)
}

/// 配下のフォルダの相対パス `member` から、フォルダ `base` の部分を除いた残り（先頭の '/' は含まない。自分なら
/// 空。`base` がボリュームのルート（空）なら `member` のまま）。
///
/// 配下のフォルダの行は、親の部分を登録したときの綴りのまま持つ（`ensure_folder_tx` は階層ごとに渡された綴りを
/// 保存するので、NFC と NFD・大文字と小文字が親の行と違うことがある）。配下は比較キーの前置で選ぶ
/// （`subtree`）ので、`base` のバイト長では切れない。部分（'/' 区切り。綴りが違っても数は同じ）の数で切る。
pub fn rel_path_below<'a>(base: &str, member: &'a str) -> &'a str {
    if base.is_empty() {
        return member;
    }
    let depth = base.split('/').count();
    member
        .match_indices('/')
        .nth(depth - 1)
        .map_or("", |(i, _)| &member[i + 1..])
}

/// フォルダ `folder_id` と配下を、ボリューム `new_volume` の `new_rel` へ付け替えられるかを確かめる（ボリュームの
/// ルートは動かせない・自分の配下へは移せない・付け替えた先にほかのフォルダが登録されていれば
/// [`CatalogError::Conflict`]）。
pub(crate) fn check_rebase(
    conn: &Connection,
    folder_id: FolderId,
    new_volume: VolumeId,
    new_rel: &str,
) -> Result<()> {
    let (folder, members) = subtree(conn, folder_id)?;
    plan_rebase(conn, &folder, &members, new_volume, new_rel).map(|_| ())
}

/// 付け替えの後の相対パス（`members` と同じ順）。確かめ方は [`check_rebase`]。
fn plan_rebase(
    conn: &Connection,
    folder: &Folder,
    members: &[(i64, String)],
    new_volume: VolumeId,
    new_rel: &str,
) -> Result<Vec<String>> {
    if folder.rel_path.is_empty() {
        return Err(CatalogError::InvalidInput(
            "ボリュームのルートのフォルダは、移動・名前の変更・場所の指定し直しができません"
                .to_owned(),
        ));
    }
    let new_rel = normalize_rel_path(new_rel)?;
    if new_rel.is_empty() {
        return Err(CatalogError::InvalidInput(
            "フォルダをボリュームのルートにはできません".to_owned(),
        ));
    }
    for part in new_rel.split('/') {
        check_file_name(part)?;
    }
    let old_key = path_key(&folder.rel_path);
    let new_key = path_key(&new_rel);
    if new_volume == folder.volume_id && new_key.starts_with(&format!("{old_key}/")) {
        return Err(CatalogError::InvalidInput(
            "フォルダを自分の中へは移せません".to_owned(),
        ));
    }
    let own: HashSet<i64> = members.iter().map(|(id, _)| *id).collect();
    let mut out = Vec::with_capacity(members.len());
    let mut find =
        conn.prepare_cached("SELECT id FROM folder WHERE volume_id = ?1 AND rel_path_key = ?2")?;
    for (_, rel) in members {
        let rest = rel_path_below(&folder.rel_path, rel);
        let target = if rest.is_empty() {
            new_rel.clone()
        } else {
            format!("{new_rel}/{rest}")
        };
        let other: Option<i64> = find
            .query_row(params![new_volume.get(), path_key(&target)], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(other) = other
            && !own.contains(&other)
        {
            return Err(CatalogError::Conflict(target));
        }
        out.push(target);
    }
    Ok(out)
}

/// フォルダ `folder_id` と配下の相対パスを付け替える（トランザクションの中で。親はボリューム `new_volume` の
/// `new_rel` の親で、なければ作る）。付け替えたフォルダを返す（最初が自分）。
pub(crate) fn rebase_folder_tx(
    tx: &Transaction<'_>,
    folder_id: FolderId,
    new_volume: VolumeId,
    new_rel: &str,
) -> Result<Vec<FolderId>> {
    let new_rel = normalize_rel_path(new_rel)?;
    let (folder, members) = subtree(tx, folder_id)?;
    let targets = plan_rebase(tx, &folder, &members, new_volume, &new_rel)?;
    // 比較キーを一時的な値（NUL は名前に含まれないので衝突しない）にしてから付け替える（大文字・小文字だけの
    // 名前の変更や、配下の新しい相対パスが別の配下の古いものと同じになる場合に、途中で UNIQUE に違反しない
    // ため）。
    for (id, _) in &members {
        tx.execute(
            "UPDATE folder SET rel_path_key = char(0) || id WHERE id = ?1",
            [id],
        )?;
    }
    let parent = ensure_folder_tx(tx, new_volume, parent_rel(&new_rel))?;
    for ((id, _), target) in members.iter().zip(&targets) {
        tx.execute(
            "UPDATE folder SET volume_id = ?2, rel_path = ?3, rel_path_key = ?4 WHERE id = ?1",
            params![id, new_volume.get(), target, path_key(target)],
        )?;
    }
    tx.execute(
        "UPDATE folder SET parent_id = ?2 WHERE id = ?1",
        params![folder_id.get(), parent.get()],
    )?;
    Ok(members
        .into_iter()
        .map(|(id, _)| FolderId::new(id))
        .collect())
}

fn volume_from_row(row: &rusqlite::Row<'_>) -> Result<VolumeRecord> {
    let seen: Option<String> = row.get(5)?;
    Ok(VolumeRecord {
        id: VolumeId::new(row.get(0)?),
        uuid: row.get(1)?,
        label: row.get(2)?,
        last_mount_path: row.get(3)?,
        fs_type: row.get(4)?,
        last_seen_at: seen.as_deref().map(parse_db_utc).transpose()?,
    })
}

impl Catalog {
    /// ボリュームの記録の一覧（ID の順。ファイルシステムの種類・最後につながった日時を含む）。
    pub fn volume_records(&self) -> Result<Vec<VolumeRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, uuid, label, last_mount_path, fs_type, last_seen_at FROM volume ORDER BY id",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(volume_from_row(row)?);
        }
        Ok(out)
    }

    /// ボリュームがつながっていることを記録する（マウント先・ラベル・ファイルシステムの種類と、最後につながった
    /// 日時。ラベル・種類は `None` なら記録のまま）。
    pub fn note_volume_seen(
        &mut self,
        id: VolumeId,
        mount_path: &str,
        label: Option<&str>,
        fs_type: Option<&str>,
    ) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE volume SET last_mount_path = ?2, label = coalesce(?3, label),
                 fs_type = coalesce(?4, fs_type), last_seen_at = ?5
             WHERE id = ?1",
            params![id.get(), mount_path, label, fs_type, now_utc_string()],
        )?;
        if n == 0 {
            return Err(CatalogError::NotFound(format!("ボリューム {id}")));
        }
        Ok(())
    }

    /// つながっていないボリュームを設定する（開いている間だけ。カタログには保存しない）。このボリュームの
    /// ファイル・フォルダの場所（[`Catalog::file_location`]・[`Catalog::folder_location`]・削除の計画）は、
    /// マウント先が分からないもの（`None`）として返す。
    pub fn set_offline_volumes(&mut self, ids: impl IntoIterator<Item = VolumeId>) {
        self.offline_volumes = ids.into_iter().collect();
    }

    /// ボリュームがつながっていないとされているか（[`Catalog::set_offline_volumes`]）。
    pub fn is_volume_offline(&self, id: VolumeId) -> bool {
        self.offline_volumes.contains(&id)
    }

    /// すべてのフォルダと状態（ボリューム・相対パスの比較キーの順）。
    pub fn folder_records(&self) -> Result<Vec<FolderRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, volume_id, parent_id, rel_path, status FROM folder
             ORDER BY volume_id, rel_path_key",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let status: String = row.get(4)?;
            out.push(FolderRecord {
                folder: Folder {
                    id: FolderId::new(row.get(0)?),
                    volume_id: VolumeId::new(row.get(1)?),
                    parent_id: row.get::<_, Option<i64>>(2)?.map(FolderId::new),
                    rel_path: row.get(3)?,
                },
                status: parse_enum(&status)?,
            });
        }
        Ok(out)
    }

    /// フォルダごとの、見つからない（`missing`）ファイルの数（配下のフォルダは含まない。0 のフォルダは入れない）。
    /// 索引 `file_not_ok` で数える。
    pub fn missing_file_counts(&self) -> Result<HashMap<FolderId, u64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT folder_id, count(*) FROM file WHERE status <> 'ok' AND status = 'missing'
             GROUP BY folder_id",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
        let mut out = HashMap::new();
        for r in rows {
            let (id, n) = r?;
            out.insert(FolderId::new(id), u64::try_from(n).unwrap_or(0));
        }
        Ok(out)
    }

    /// フォルダの状態を記録する（変えた数を返す。同じ状態のものは変えない）。
    pub fn set_folder_status(&mut self, ids: &[FolderId], status: FolderStatus) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE folder SET status = ?2, status_at = ?3
             WHERE id IN (SELECT value FROM json_each(?1)) AND status <> ?2",
            params![ids_to_json(ids), status.as_str(), now_utc_string()],
        )?)
    }

    /// フォルダと配下のフォルダ（最初が自分。ボリュームのルートならボリュームのすべてのフォルダ）。
    pub fn folder_subtree(&self, folder_id: FolderId) -> Result<Vec<FolderId>> {
        let (_, members) = subtree(&self.conn, folder_id)?;
        Ok(members
            .into_iter()
            .map(|(id, _)| FolderId::new(id))
            .collect())
    }

    /// フォルダ `parent` の下に、名前 `name` のフォルダを登録する（FILE-02 のフォルダの作成。OS のフォルダは
    /// 呼び出し側が先に作る）。同じ名前（比較キー）のフォルダが既に登録されていれば [`CatalogError::Conflict`]。
    pub fn create_folder(&mut self, parent: FolderId, name: &str) -> Result<FolderId> {
        check_file_name(name)?;
        let tx = self.conn.transaction()?;
        let (volume, rel): (i64, String) = tx
            .query_row(
                "SELECT volume_id, rel_path FROM folder WHERE id = ?1",
                [parent.get()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("フォルダ {parent}")))?;
        let rel = if rel.is_empty() {
            name.to_owned()
        } else {
            format!("{rel}/{name}")
        };
        let rel = normalize_rel_path(&rel)?;
        let exists: Option<i64> = tx
            .query_row(
                "SELECT id FROM folder WHERE volume_id = ?1 AND rel_path_key = ?2",
                params![volume, path_key(&rel)],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_some() {
            return Err(CatalogError::Conflict(rel));
        }
        let id = ensure_folder_tx(&tx, VolumeId::new(volume), &rel)?;
        tx.commit()?;
        Ok(id)
    }

    /// フォルダの場所を指定し直す（FILE-04。アプリの外で移動・名前の変更をしたフォルダを、新しい場所に結び
    /// 付ける。カタログの中だけで、ファイルには触れない）。フォルダと配下をボリューム `volume` の `rel_path` へ
    /// 付け替え、見つからない印を外す。付け替えたフォルダを返す。
    ///
    /// 新しい場所（と配下）にほかのフォルダが登録されていれば [`CatalogError::Conflict`]（同じフォルダを二重に
    /// 持たない。先にそちらをカタログから除く）。
    pub fn relocate_folder(
        &mut self,
        folder_id: FolderId,
        volume: VolumeId,
        rel_path: &str,
    ) -> Result<Vec<FolderId>> {
        let tx = self.conn.transaction()?;
        let ids = rebase_folder_tx(&tx, folder_id, volume, rel_path)?;
        tx.execute(
            "UPDATE folder SET status = 'ok', status_at = ?2
             WHERE id IN (SELECT value FROM json_each(?1)) AND status <> 'ok'",
            params![ids_to_json(&ids), now_utc_string()],
        )?;
        tx.commit()?;
        Ok(ids)
    }

    /// フォルダと配下のフォルダをカタログから除く（FILE-04。見つからないフォルダ・写真を再リンクして空になった
    /// フォルダ・場所の指定し直しの先に登録済みのフォルダを片付ける。カタログの中だけで、ディスクのフォルダと
    /// ファイルには触れない）。ボリュームのルートなら、ボリュームの記録も除く。
    ///
    /// 写真のファイルが 1 つでも登録されていれば（配下を含む）、何も変えずに [`CatalogError::InvalidInput`]
    /// （先に写真をカタログから除くか、別の場所に結び付け直す。写真の除去は確認と後片付けがある別の操作）。
    /// 終わっていないファイル操作（`file_op` の planned・executing）がフォルダを使っていても断る（次に開いた
    /// ときの確定が、フォルダを必要とするため）。スマートコレクションの条件の「フォルダ」は、除いたフォルダを
    /// 指したまま（何にも当たらなくなる。フォルダの ID は使い回さない）。
    pub fn remove_folder(&mut self, folder_id: FolderId) -> Result<FolderRemoval> {
        let unfinished = self.unfinished_file_ops()?;
        let tx = self.conn.transaction()?;
        let (folder, members) = subtree(&tx, folder_id)?;
        let ids: Vec<FolderId> = members.iter().map(|(id, _)| FolderId::new(*id)).collect();
        let files: i64 = tx.query_row(
            "SELECT count(*) FROM file WHERE folder_id IN (SELECT value FROM json_each(?1))",
            [ids_to_json(&ids)],
            |row| row.get(0),
        )?;
        if files > 0 {
            return Err(CatalogError::InvalidInput(format!(
                "フォルダ（と配下）に写真のファイルが {files} 個登録されています（先に写真をカタログから除くか、別の場所に結び付け直してください）"
            )));
        }
        let own: HashSet<FolderId> = ids.iter().copied().collect();
        for op in &unfinished {
            let uses = op
                .payload
                .folder
                .as_ref()
                .is_some_and(|f| own.contains(&f.folder_id))
                || op.payload.entries.iter().any(|e| {
                    own.contains(&e.from.folder_id)
                        || e.to.as_ref().is_some_and(|t| own.contains(&t.folder_id))
                });
            if uses {
                return Err(CatalogError::InvalidInput(format!(
                    "終わっていないファイル操作 {} がこのフォルダを使っています（カタログを開き直して確定してから、やり直してください）",
                    op.id
                )));
            }
        }
        // 深いものから消す（親を指す行を先に消す）。
        let mut order: Vec<(usize, i64)> = members
            .iter()
            .map(|(id, rel)| {
                (
                    if rel.is_empty() {
                        0
                    } else {
                        rel.split('/').count()
                    },
                    *id,
                )
            })
            .collect();
        order.sort_by(|a, b| b.cmp(a));
        let mut removed = 0u64;
        for (_, id) in order {
            removed +=
                u64::try_from(tx.execute("DELETE FROM folder WHERE id = ?1", [id])?).unwrap_or(0);
        }
        let volume = if folder.rel_path.is_empty() {
            tx.execute("DELETE FROM volume WHERE id = ?1", [folder.volume_id.get()])?;
            Some(folder.volume_id)
        } else {
            None
        };
        tx.commit()?;
        if let Some(v) = volume {
            self.offline_volumes.remove(&v);
        }
        Ok(FolderRemoval {
            folders: removed,
            volume,
        })
    }

    /// パスから作った ID のボリュームのフォルダを、OS のボリューム ID のボリュームへ移す（FILE-03。1 つの
    /// トランザクション）。移し先のボリュームは呼び出し側が先に作る（[`Catalog::ensure_volume`]）。
    ///
    /// - 親から順に（元の相対パスの浅い順に）移す。移し先に同じ場所のフォルダが既にあれば、ファイルをそちらへ
    ///   まとめ（同じ名前のファイルが登録されていれば移さずに残す。結果の `conflicting_files`）、空になった
    ///   フォルダは消す。
    /// - 移さなかった（つながっていない・見つからない）フォルダの親が別のボリュームへ移ったときは、元の
    ///   ボリュームに親のフォルダを作り直して付ける（親子が別のボリュームにならないように）。
    /// - フォルダがなくなった元のボリュームは消す。
    pub fn rebind_folders(&mut self, rebinds: &[FolderRebind]) -> Result<RebindReport> {
        let tx = self.conn.transaction()?;
        let mut report = RebindReport::default();
        let mut sources: HashSet<i64> = HashSet::new();
        let mut merged_from: Vec<i64> = Vec::new();
        let mut list: Vec<(&FolderRebind, Folder)> = Vec::new();
        for r in rebinds {
            let f = tx
                .query_row(
                    "SELECT id, volume_id, parent_id, rel_path FROM folder WHERE id = ?1",
                    [r.folder_id.get()],
                    |row| {
                        Ok(Folder {
                            id: FolderId::new(row.get(0)?),
                            volume_id: VolumeId::new(row.get(1)?),
                            parent_id: row.get::<_, Option<i64>>(2)?.map(FolderId::new),
                            rel_path: row.get(3)?,
                        })
                    },
                )
                .optional()?;
            if let Some(f) = f {
                list.push((r, f));
            }
        }
        let depth = |rel: &str| {
            if rel.is_empty() {
                0
            } else {
                rel.split('/').count()
            }
        };
        list.sort_by_key(|(_, f)| (depth(&f.rel_path), f.id));
        for (r, f) in list {
            let rel = normalize_rel_path(&r.rel_path)?;
            if f.volume_id == r.volume_id && f.rel_path == rel {
                continue;
            }
            sources.insert(f.volume_id.get());
            let key = path_key(&rel);
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM folder WHERE volume_id = ?1 AND rel_path_key = ?2",
                    params![r.volume_id.get(), key],
                    |row| row.get(0),
                )
                .optional()?;
            match existing {
                Some(e) if e != f.id.get() => {
                    // 同じ場所のフォルダが既にある: 名前の重ならないファイルをそちらへまとめる。
                    tx.execute(
                        "UPDATE file SET folder_id = ?2 WHERE folder_id = ?1
                           AND name_key NOT IN (SELECT name_key FROM file WHERE folder_id = ?2)",
                        params![f.id.get(), e],
                    )?;
                    let mut stmt =
                        tx.prepare_cached("SELECT id FROM file WHERE folder_id = ?1 ORDER BY id")?;
                    let left = stmt.query_map([f.id.get()], |row| row.get::<_, i64>(0))?;
                    for id in left {
                        report.conflicting_files.push(FileId::new(id?));
                    }
                    merged_from.push(f.id.get());
                    report.merged += 1;
                }
                _ => {
                    if rel.is_empty() {
                        tx.execute(
                            "UPDATE folder SET volume_id = ?2, rel_path = '', rel_path_key = '',
                                 parent_id = NULL
                             WHERE id = ?1",
                            params![f.id.get(), r.volume_id.get()],
                        )?;
                    } else {
                        // 親を先に確保する（自分の行はまだ元のボリュームにある）。
                        let parent = ensure_folder_tx(&tx, r.volume_id, parent_rel(&rel))?;
                        tx.execute(
                            "UPDATE folder SET volume_id = ?2, rel_path = ?3, rel_path_key = ?4,
                                 parent_id = ?5
                             WHERE id = ?1",
                            params![f.id.get(), r.volume_id.get(), rel, key, parent.get()],
                        )?;
                    }
                    report.moved += 1;
                }
            }
        }
        // 親が別のボリュームへ移ったフォルダ（移さなかったもの・まとめた先の子）は、自分のボリュームの親に付け
        // 直す。
        let crossed: Vec<(i64, i64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT c.id, c.volume_id, c.rel_path FROM folder c JOIN folder p ON p.id = c.parent_id
                 WHERE p.volume_id <> c.volume_id ORDER BY c.id",
            )?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (id, volume, rel) in crossed {
            let parent = ensure_folder_tx(&tx, VolumeId::new(volume), parent_rel(&rel))?;
            tx.execute(
                "UPDATE folder SET parent_id = ?2 WHERE id = ?1",
                params![id, parent.get()],
            )?;
        }
        // まとめて空になったフォルダを消す（ファイルも子もないもの。移した子は、上の確保で移し先の親に付いて
        // いる）。深いものから消す（子を消した後に親が空になる）。
        for id in merged_from.into_iter().rev() {
            report.removed_folders += u64::try_from(tx.execute(
                "DELETE FROM folder WHERE id = ?1
                   AND NOT EXISTS (SELECT 1 FROM file WHERE folder_id = ?1)
                   AND NOT EXISTS (SELECT 1 FROM folder WHERE parent_id = ?1)",
                [id],
            )?)
            .unwrap_or(0);
        }
        // 元のボリュームで、ルートだけが残った（ファイルも子もない）ものは、ルートとボリュームを消す。
        for v in sources {
            let only_root: bool = tx.query_row(
                "SELECT count(*) = 1 AND sum(rel_path = '') = 1 FROM folder WHERE volume_id = ?1",
                [v],
                |row| row.get::<_, Option<bool>>(0).map(|b| b.unwrap_or(false)),
            )?;
            if only_root {
                report.removed_folders += u64::try_from(tx.execute(
                    "DELETE FROM folder WHERE volume_id = ?1 AND rel_path = ''
                       AND NOT EXISTS (SELECT 1 FROM file f JOIN folder d ON d.id = f.folder_id
                                       WHERE d.volume_id = ?1)",
                    [v],
                )?)
                .unwrap_or(0);
            }
            let empty: bool = tx.query_row(
                "SELECT NOT EXISTS (SELECT 1 FROM folder WHERE volume_id = ?1)",
                [v],
                |row| row.get(0),
            )?;
            if empty {
                tx.execute("DELETE FROM volume WHERE id = ?1", [v])?;
                report.removed_volumes.push(VolumeId::new(v));
            }
        }
        tx.commit()?;
        Ok(report)
    }
}
