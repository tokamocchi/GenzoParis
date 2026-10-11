//! 現像設定の保存・履歴・スナップショット・仮想コピー（DEV-00、DEV-27、DEV-28、DEV-30、LIB-13）。
//!
//! docs/04_architecture.md の 2.5 節・3.1 節・6.2 節。
//!
//! - variant の `develop_json`・`develop_hash`・`process_version`・`updated_at` に現在の設定を保存し、
//!   保存のたびに履歴（`history_entry`）を 1 件追加する。
//! - **未調整の設定は `develop_json = NULL`**（`process_version` の既定の設定）として保存する。
//!   50 万件の未調整の写真ごとに既定の設定の JSON を持たせないため（SCL-02）。
//! - 履歴の最初の 1 件は、variant を作ったときの状態（マスターは「読み込み」、仮想コピーは
//!   「仮想コピーの作成」。Lightroom の履歴の「読み込み」に相当）。これにより、最初の編集も
//!   取り消せる。
//! - **Undo / Redo は再起動後もできる**: variant に現在の履歴の位置（`history_pos`）を持たせる。
//!   Undo の後に新しい編集をしたら、それより先の履歴を削除する。
//! - 設定の一括適用（DEV-30）は、複数の variant に同じ設定を 1 つのトランザクションで保存し、
//!   それぞれに履歴を 1 件ずつ追加する。

use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use genzo_model::{
    AssetId, CURRENT_PROCESS_VERSION, DevelopError, DevelopSettings, HistoryEntryId, SnapshotId,
    VariantId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::util::{i64_to_u32, non_empty_name, now_utc_string, parse_db_utc};

/// マスターの variant を作ったとき（登録したとき）の履歴の名前。
pub const HISTORY_LABEL_IMPORT: &str = "読み込み";
/// 仮想コピーを作ったときの履歴の名前。
pub const HISTORY_LABEL_VIRTUAL_COPY: &str = "仮想コピーの作成";

/// 処理バージョン `pv` の既定の（何も調整していない）設定。
pub(crate) fn default_settings(process_version: u32) -> DevelopSettings {
    DevelopSettings {
        process_version,
        ..DevelopSettings::default()
    }
}

/// 処理バージョン `pv` の既定の設定のハッシュ（16 進数）。現在の版の値は 1 回だけ計算する。
fn default_develop_hash(process_version: u32) -> String {
    static CURRENT: OnceLock<String> = OnceLock::new();
    if process_version == CURRENT_PROCESS_VERSION {
        CURRENT
            .get_or_init(|| default_settings(CURRENT_PROCESS_VERSION).develop_hash_hex())
            .clone()
    } else {
        default_settings(process_version).develop_hash_hex()
    }
}

/// DB に保存する形にした現像設定。
#[derive(Debug, Clone)]
pub(crate) struct EncodedDevelop {
    /// `develop_json`（既定の設定なら `None`）。
    pub json: Option<String>,
    /// `develop_hash`。
    pub hash: String,
    /// `process_version`。
    pub process_version: u32,
}

/// 現像設定を DB に保存する形にする（検証してから JSON にする）。
pub(crate) fn encode(settings: &DevelopSettings) -> Result<EncodedDevelop> {
    let pv = settings.process_version;
    if *settings == default_settings(pv) {
        // 既定の設定も検証は行う（未知の処理バージョンを保存しないため）。
        settings.validate().map_err(DevelopError::from)?;
        return Ok(EncodedDevelop {
            json: None,
            hash: default_develop_hash(pv),
            process_version: pv,
        });
    }
    Ok(EncodedDevelop {
        json: Some(settings.to_json()?),
        hash: settings.develop_hash_hex(),
        process_version: pv,
    })
}

/// DB の `develop_json` と `process_version` から現像設定を読む。
pub(crate) fn decode(json: Option<&str>, process_version: i64) -> Result<DevelopSettings> {
    let pv = i64_to_u32(process_version, "process_version")?;
    match json {
        Some(j) => Ok(DevelopSettings::from_json(j)?),
        None => {
            if pv == 0 || pv > CURRENT_PROCESS_VERSION {
                return Err(DevelopError::UnsupportedProcessVersion {
                    found: pv,
                    supported: CURRENT_PROCESS_VERSION,
                }
                .into());
            }
            Ok(default_settings(pv))
        }
    }
}

/// 新しいマスターの variant の現像設定の列（既定の設定、現在の処理バージョン）。
pub(crate) fn new_variant_develop() -> EncodedDevelop {
    EncodedDevelop {
        json: None,
        hash: default_develop_hash(CURRENT_PROCESS_VERSION),
        process_version: CURRENT_PROCESS_VERSION,
    }
}

/// variant を作り、最初の履歴を 1 件記録する。戻り値は variant の ID。
pub(crate) fn insert_variant(
    tx: &Connection,
    asset_id: i64,
    is_master: bool,
    name: Option<&str>,
    develop: &EncodedDevelop,
    history_label: &str,
    now: &str,
) -> Result<i64> {
    tx.prepare_cached(
        "INSERT INTO variant(asset_id, is_master, name, develop_json, develop_hash, process_version,
                             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
    )?
    .execute(params![
        asset_id,
        is_master,
        name,
        develop.json,
        develop.hash,
        develop.process_version,
        now
    ])?;
    let variant_id = tx.last_insert_rowid();
    let entry = insert_history(tx, variant_id, history_label, develop, now)?;
    tx.prepare_cached("UPDATE variant SET history_pos = ?2 WHERE id = ?1")?
        .execute(params![variant_id, entry])?;
    Ok(variant_id)
}

fn insert_history(
    tx: &Connection,
    variant_id: i64,
    label: &str,
    develop: &EncodedDevelop,
    now: &str,
) -> Result<i64> {
    tx.prepare_cached(
        "INSERT INTO history_entry(variant_id, created_at, label, develop_json, process_version)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![
        variant_id,
        now,
        label,
        develop.json,
        develop.process_version
    ])?;
    Ok(tx.last_insert_rowid())
}

/// variant の現在の履歴の位置（variant がなければ NotFound）。
fn history_pos(conn: &Connection, variant_id: VariantId) -> Result<Option<i64>> {
    conn.prepare_cached("SELECT history_pos FROM variant WHERE id = ?1")?
        .query_row([variant_id.get()], |row| row.get::<_, Option<i64>>(0))
        .optional()?
        .ok_or_else(|| CatalogError::NotFound(format!("variant {variant_id}")))
}

/// 設定を保存し、履歴を 1 件追加する（Undo の後なら、先の履歴を削除してから）。
pub(crate) fn save_develop_tx(
    tx: &Transaction<'_>,
    variant_id: VariantId,
    develop: &EncodedDevelop,
    label: &str,
    now: &str,
) -> Result<HistoryEntryId> {
    if let Some(pos) = history_pos(tx, variant_id)? {
        tx.prepare_cached("DELETE FROM history_entry WHERE variant_id = ?1 AND id > ?2")?
            .execute(params![variant_id.get(), pos])?;
    }
    let entry = insert_history(tx, variant_id.get(), label, develop, now)?;
    set_current(tx, variant_id, develop, entry, now)?;
    Ok(HistoryEntryId::new(entry))
}

/// variant の現在の設定と履歴の位置を書く。
fn set_current(
    tx: &Connection,
    variant_id: VariantId,
    develop: &EncodedDevelop,
    history_pos: i64,
    now: &str,
) -> Result<()> {
    tx.prepare_cached(
        "UPDATE variant SET develop_json = ?2, develop_hash = ?3, process_version = ?4,
             history_pos = ?5, updated_at = ?6
         WHERE id = ?1",
    )?
    .execute(params![
        variant_id.get(),
        develop.json,
        develop.hash,
        develop.process_version,
        history_pos,
        now
    ])?;
    Ok(())
}

/// variant の現在の現像設定の状態。
#[derive(Debug, Clone, PartialEq)]
pub struct DevelopState {
    /// variant。
    pub variant_id: VariantId,
    /// 現在の設定。
    pub settings: DevelopSettings,
    /// 設定のハッシュ（`develop_hash`）。
    pub develop_hash: String,
    /// 処理バージョン。
    pub process_version: u32,
    /// 最後に設定を変更した日時。
    pub updated_at: DateTime<Utc>,
    /// 取り消せる履歴があるか。
    pub can_undo: bool,
    /// やり直せる履歴があるか。
    pub can_redo: bool,
}

/// 履歴の 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// ID。
    pub id: HistoryEntryId,
    /// 記録した日時。
    pub created_at: DateTime<Utc>,
    /// 操作の名前（「露光量 +0.30」など）。
    pub label: String,
    /// 現在の位置か。
    pub is_current: bool,
}

/// スナップショット（DEV-28）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// ID。
    pub id: SnapshotId,
    /// variant。
    pub variant_id: VariantId,
    /// 名前。
    pub name: String,
    /// 作った日時。
    pub created_at: DateTime<Utc>,
}

/// variant の一覧の 1 件（マスターと仮想コピー）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantInfo {
    /// ID。
    pub id: VariantId,
    /// asset。
    pub asset_id: AssetId,
    /// マスターか。
    pub is_master: bool,
    /// 仮想コピーの名前（マスターは `None`）。
    pub name: Option<String>,
}

impl Catalog {
    /// variant の現在の現像設定の状態を読む。
    pub fn develop_state(&self, variant_id: VariantId) -> Result<DevelopState> {
        let row = self
            .conn
            .prepare_cached(
                "SELECT develop_json, develop_hash, process_version, updated_at, history_pos,
                    EXISTS(SELECT 1 FROM history_entry h WHERE h.variant_id = v.id AND h.id < v.history_pos),
                    EXISTS(SELECT 1 FROM history_entry h WHERE h.variant_id = v.id AND h.id > v.history_pos)
                 FROM variant v WHERE id = ?1",
            )?
            .query_row([variant_id.get()], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, bool>(5)?,
                    row.get::<_, bool>(6)?,
                ))
            })
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("variant {variant_id}")))?;
        let (json, hash, pv, updated_at, pos, can_undo, can_redo) = row;
        Ok(DevelopState {
            variant_id,
            settings: decode(json.as_deref(), pv)?,
            develop_hash: hash,
            process_version: i64_to_u32(pv, "process_version")?,
            updated_at: parse_db_utc(&updated_at)?,
            can_undo: pos.is_some() && can_undo,
            can_redo: pos.is_some() && can_redo,
        })
    }

    /// variant の現在の現像設定を読む。
    pub fn develop_settings(&self, variant_id: VariantId) -> Result<DevelopSettings> {
        Ok(self.develop_state(variant_id)?.settings)
    }

    /// 現像設定を保存し、履歴を 1 件追加する（DEV-00・DEV-27）。
    ///
    /// スライダーのドラッグ中の値は保存せず、ドラッグの終了時に 1 回だけ呼ぶ（6.2 節）。
    /// 設定は検証してから保存する（範囲外の値・NaN はエラー。先に `normalized` で丸める）。
    pub fn save_develop(
        &mut self,
        variant_id: VariantId,
        settings: &DevelopSettings,
        label: &str,
    ) -> Result<HistoryEntryId> {
        let develop = encode(settings)?;
        let now = now_utc_string();
        let tx = self.conn.transaction()?;
        let id = save_develop_tx(&tx, variant_id, &develop, label, &now)?;
        tx.commit()?;
        Ok(id)
    }

    /// 同じ設定を複数の variant に保存する（DEV-30。1 つのトランザクション）。
    ///
    /// それぞれの variant に履歴を 1 件ずつ追加する（同じ variant が重複していても 1 件）。
    /// 1 件でも失敗したら（存在しない variant など）すべて取り消す。結果は `variant_ids` と同じ順。
    pub fn apply_develop_to_many(
        &mut self,
        variant_ids: &[VariantId],
        settings: &DevelopSettings,
        label: &str,
    ) -> Result<Vec<HistoryEntryId>> {
        let develop = encode(settings)?;
        let now = now_utc_string();
        let tx = self.conn.transaction()?;
        let mut out = Vec::with_capacity(variant_ids.len());
        // 同じ variant が重複して渡された場合も、履歴は 1 件だけにする。
        let mut done = std::collections::HashMap::new();
        for &v in variant_ids {
            let entry = match done.get(&v) {
                Some(&e) => e,
                None => {
                    let e = save_develop_tx(&tx, v, &develop, label, &now)?;
                    done.insert(v, e);
                    e
                }
            };
            out.push(entry);
        }
        tx.commit()?;
        Ok(out)
    }

    /// variant ごとに別の設定を、1 つのトランザクションで保存する（DEV-30 の一部の項目だけの貼り付けなど）。
    ///
    /// それぞれの variant に履歴を 1 件ずつ追加する。同じ variant が重複していたら、最後の設定を
    /// 保存して履歴は 1 件にする。1 件でも失敗したら（存在しない variant・不正な設定など）すべて
    /// 取り消す。結果は `items` と同じ順。
    pub fn save_develop_batch(
        &mut self,
        items: &[(VariantId, DevelopSettings)],
        label: &str,
    ) -> Result<Vec<HistoryEntryId>> {
        let mut last = std::collections::HashMap::new();
        for (i, (v, _)) in items.iter().enumerate() {
            last.insert(*v, i);
        }
        let encoded = items
            .iter()
            .map(|(_, s)| encode(s))
            .collect::<Result<Vec<_>>>()?;
        let now = now_utc_string();
        let tx = self.conn.transaction()?;
        let mut saved = std::collections::HashMap::new();
        for (i, (v, _)) in items.iter().enumerate() {
            if last[v] == i {
                saved.insert(*v, save_develop_tx(&tx, *v, &encoded[i], label, &now)?);
            }
        }
        tx.commit()?;
        Ok(items.iter().map(|(v, _)| saved[v]).collect())
    }

    /// 1 つ前の履歴に戻す（Undo）。戻せない場合は `None`。
    pub fn undo_develop(&mut self, variant_id: VariantId) -> Result<Option<DevelopSettings>> {
        self.step_history(variant_id, false)
    }

    /// 1 つ先の履歴に進める（Redo）。進められない場合は `None`。
    pub fn redo_develop(&mut self, variant_id: VariantId) -> Result<Option<DevelopSettings>> {
        self.step_history(variant_id, true)
    }

    fn step_history(
        &mut self,
        variant_id: VariantId,
        forward: bool,
    ) -> Result<Option<DevelopSettings>> {
        let now = now_utc_string();
        let tx = self.conn.transaction()?;
        let Some(pos) = history_pos(&tx, variant_id)? else {
            return Ok(None);
        };
        let sql = if forward {
            "SELECT id FROM history_entry WHERE variant_id = ?1 AND id > ?2 ORDER BY id ASC LIMIT 1"
        } else {
            "SELECT id FROM history_entry WHERE variant_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT 1"
        };
        let target: Option<i64> = tx
            .prepare_cached(sql)?
            .query_row(params![variant_id.get(), pos], |row| row.get(0))
            .optional()?;
        let Some(target) = target else {
            return Ok(None);
        };
        let settings = move_to_entry(&tx, variant_id, target, &now)?;
        tx.commit()?;
        Ok(Some(settings))
    }

    /// 履歴の任意の位置へ移動する（履歴のパネルで項目を選んだとき）。
    pub fn go_to_history(
        &mut self,
        variant_id: VariantId,
        entry: HistoryEntryId,
    ) -> Result<DevelopSettings> {
        let now = now_utc_string();
        let tx = self.conn.transaction()?;
        let settings = move_to_entry(&tx, variant_id, entry.get(), &now)?;
        tx.commit()?;
        Ok(settings)
    }

    /// variant の履歴（古い順）。
    pub fn history(&self, variant_id: VariantId) -> Result<Vec<HistoryEntry>> {
        let pos = history_pos(&self.conn, variant_id)?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, created_at, label FROM history_entry WHERE variant_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([variant_id.get()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, created_at, label) = r?;
            out.push(HistoryEntry {
                id: HistoryEntryId::new(id),
                created_at: parse_db_utc(&created_at)?,
                label,
                is_current: Some(id) == pos,
            });
        }
        Ok(out)
    }

    /// 履歴の 1 件の設定を読む（履歴のパネルでのプレビュー用）。
    pub fn history_settings(&self, entry: HistoryEntryId) -> Result<DevelopSettings> {
        let (json, pv) = self
            .conn
            .prepare_cached(
                "SELECT develop_json, process_version FROM history_entry WHERE id = ?1",
            )?
            .query_row([entry.get()], |row| {
                Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?))
            })
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("履歴 {entry}")))?;
        decode(json.as_deref(), pv)
    }

    /// 現在の設定でスナップショットを作る（DEV-28）。
    pub fn create_snapshot(&mut self, variant_id: VariantId, name: &str) -> Result<SnapshotId> {
        let name = non_empty_name(name, "スナップショットの名前")?;
        let now = now_utc_string();
        let changed = self.conn.execute(
            "INSERT INTO snapshot(variant_id, name, develop_json, process_version, created_at)
             SELECT id, ?2, develop_json, process_version, ?3 FROM variant WHERE id = ?1",
            params![variant_id.get(), name, now],
        )?;
        if changed == 0 {
            return Err(CatalogError::NotFound(format!("variant {variant_id}")));
        }
        Ok(SnapshotId::new(self.conn.last_insert_rowid()))
    }

    /// variant のスナップショットの一覧（作った順）。
    pub fn snapshots(&self, variant_id: VariantId) -> Result<Vec<Snapshot>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, created_at FROM snapshot WHERE variant_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([variant_id.get()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, created_at) = r?;
            out.push(Snapshot {
                id: SnapshotId::new(id),
                variant_id,
                name,
                created_at: parse_db_utc(&created_at)?,
            });
        }
        Ok(out)
    }

    /// スナップショットの設定を読む。
    pub fn snapshot_settings(&self, snapshot: SnapshotId) -> Result<DevelopSettings> {
        let (json, pv) = self.snapshot_row(snapshot)?;
        decode(json.as_deref(), pv)
    }

    fn snapshot_row(&self, snapshot: SnapshotId) -> Result<(Option<String>, i64)> {
        self.conn
            .prepare_cached("SELECT develop_json, process_version FROM snapshot WHERE id = ?1")?
            .query_row([snapshot.get()], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("スナップショット {snapshot}")))
    }

    /// スナップショットの設定を variant に適用する（履歴を 1 件追加する）。
    ///
    /// 他の variant のスナップショットも適用できる。
    pub fn apply_snapshot(
        &mut self,
        variant_id: VariantId,
        snapshot: SnapshotId,
    ) -> Result<HistoryEntryId> {
        let name: String = self
            .conn
            .query_row(
                "SELECT name FROM snapshot WHERE id = ?1",
                [snapshot.get()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("スナップショット {snapshot}")))?;
        let settings = self.snapshot_settings(snapshot)?;
        self.save_develop(
            variant_id,
            &settings,
            &format!("スナップショット「{name}」を適用"),
        )
    }

    /// スナップショットを削除する。
    pub fn delete_snapshot(&mut self, snapshot: SnapshotId) -> Result<()> {
        let n = self
            .conn
            .execute("DELETE FROM snapshot WHERE id = ?1", [snapshot.get()])?;
        if n == 0 {
            return Err(CatalogError::NotFound(format!(
                "スナップショット {snapshot}"
            )));
        }
        Ok(())
    }

    /// 仮想コピーを作る（LIB-13）。元の variant の現在の現像設定をコピーする。
    ///
    /// 評価・フラグ・ラベル・キーワードはコピーしない。`name` が `None` なら「コピー n」とする。
    pub fn create_virtual_copy(
        &mut self,
        source: VariantId,
        name: Option<&str>,
    ) -> Result<VariantId> {
        let name = name
            .map(|n| non_empty_name(n, "仮想コピーの名前"))
            .transpose()?;
        let now = now_utc_string();
        let tx = self.conn.transaction()?;
        let (asset_id, json, hash, pv): (i64, Option<String>, String, i64) = tx
            .query_row(
                "SELECT asset_id, develop_json, develop_hash, process_version FROM variant WHERE id = ?1",
                [source.get()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?
            .ok_or_else(|| CatalogError::NotFound(format!("variant {source}")))?;
        let name = match name {
            Some(n) => n.to_owned(),
            None => {
                let copies: i64 = tx.query_row(
                    "SELECT count(*) FROM variant WHERE asset_id = ?1 AND is_master = 0",
                    [asset_id],
                    |row| row.get(0),
                )?;
                format!("コピー {}", copies + 1)
            }
        };
        let develop = EncodedDevelop {
            json,
            hash,
            process_version: i64_to_u32(pv, "process_version")?,
        };
        let id = insert_variant(
            &tx,
            asset_id,
            false,
            Some(&name),
            &develop,
            HISTORY_LABEL_VIRTUAL_COPY,
            &now,
        )?;
        tx.commit()?;
        Ok(VariantId::new(id))
    }

    /// 仮想コピーを削除する（6.4 節）。マスターは削除できない。元ファイルは変更しない。
    ///
    /// 履歴・スナップショット・キーワードの付与・コレクションの所属も削除される（ON DELETE CASCADE）。
    pub fn delete_virtual_copy(&mut self, variant_id: VariantId) -> Result<()> {
        self.delete_virtual_copies(&[variant_id])
    }

    /// 複数の仮想コピーを 1 つのトランザクションで削除する（6.4 節。DATA-02）。1 件でも削除できない
    /// （マスター・存在しない variant）なら、何も削除しない。重複は 1 回として扱う。
    pub fn delete_virtual_copies(&mut self, variant_ids: &[VariantId]) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        let tx = self.conn.transaction()?;
        for &variant_id in variant_ids {
            if !seen.insert(variant_id) {
                continue;
            }
            let is_master: bool = tx
                .query_row(
                    "SELECT is_master FROM variant WHERE id = ?1",
                    [variant_id.get()],
                    |row| row.get(0),
                )
                .optional()?
                .ok_or_else(|| CatalogError::NotFound(format!("variant {variant_id}")))?;
            if is_master {
                return Err(CatalogError::CannotDeleteMaster(variant_id));
            }
            tx.execute("DELETE FROM variant WHERE id = ?1", [variant_id.get()])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// asset の variant の一覧（マスターが先、仮想コピーは作った順）。
    pub fn variants_of_asset(&self, asset_id: AssetId) -> Result<Vec<VariantInfo>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, is_master, name FROM variant WHERE asset_id = ?1 ORDER BY is_master DESC, id",
        )?;
        let rows = stmt.query_map([asset_id.get()], |row| {
            Ok(VariantInfo {
                id: VariantId::new(row.get(0)?),
                asset_id,
                is_master: row.get(1)?,
                name: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// 履歴の `entry` の設定を variant の現在の設定にする（履歴は追加しない）。
fn move_to_entry(
    tx: &Transaction<'_>,
    variant_id: VariantId,
    entry: i64,
    now: &str,
) -> Result<DevelopSettings> {
    let (json, pv): (Option<String>, i64) = tx
        .prepare_cached(
            "SELECT develop_json, process_version FROM history_entry WHERE id = ?1 AND variant_id = ?2",
        )?
        .query_row(params![entry, variant_id.get()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?
        .ok_or_else(|| CatalogError::NotFound(format!("variant {variant_id} の履歴 {entry}")))?;
    let settings = decode(json.as_deref(), pv)?;
    let develop = EncodedDevelop {
        hash: match json {
            Some(_) => settings.develop_hash_hex(),
            None => default_develop_hash(i64_to_u32(pv, "process_version")?),
        },
        json,
        process_version: i64_to_u32(pv, "process_version")?,
    };
    set_current(tx, variant_id, &develop, entry, now)?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_stored_as_null() {
        let enc = encode(&DevelopSettings::default()).unwrap();
        assert!(enc.json.is_none());
        assert_eq!(enc.hash, DevelopSettings::default().develop_hash_hex());
        assert_eq!(enc.process_version, CURRENT_PROCESS_VERSION);
        let back = decode(None, i64::from(CURRENT_PROCESS_VERSION)).unwrap();
        assert_eq!(back, DevelopSettings::default());
    }

    #[test]
    fn edited_settings_round_trip_through_json() {
        let s = DevelopSettings {
            exposure_ev: 0.5,
            ..Default::default()
        };
        let enc = encode(&s).unwrap();
        assert!(enc.json.is_some());
        assert_eq!(enc.hash, s.develop_hash_hex());
        let back = decode(enc.json.as_deref(), i64::from(enc.process_version)).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn invalid_settings_are_rejected() {
        let nan = DevelopSettings {
            exposure_ev: f32::NAN,
            ..Default::default()
        };
        assert!(matches!(encode(&nan), Err(CatalogError::Develop(_))));
        let future = default_settings(CURRENT_PROCESS_VERSION + 1);
        assert!(matches!(encode(&future), Err(CatalogError::Develop(_))));
        assert!(matches!(
            decode(None, i64::from(CURRENT_PROCESS_VERSION + 1)),
            Err(CatalogError::Develop(_))
        ));
        assert!(decode(None, 0).is_err());
        assert!(decode(Some("not json"), 1).is_err());
    }
}
