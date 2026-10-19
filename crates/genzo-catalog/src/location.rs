//! 位置情報の後付け（LIB-21。ジオタグ付け。docs/04_architecture.md の 3.1 節・3.16 節）。
//!
//! - **付けた位置**（`asset_location`。スキーマの版 13）: asset ごとに 1 つ。元のファイルには書かず、カタログの
//!   中だけに持つ（非破壊）。asset 単位なので、仮想コピーや RAW と JPEG の組も同じ位置になる。カメラ（ファイル）の
//!   位置（`asset.gps_lat`・`gps_lon`）は変えない（取り込み・ファイルの読み直しのたびにファイルの値で上書き
//!   するため）。使う位置は、付けた位置があればそれ、なければカメラの位置（[`crate::AssetRecord::effective_gps`]、
//!   公開ビュー `photos_v1` の `location_lat`・`location_lon`・`location_source`）。
//! - **変更**（[`Catalog::edit_locations`]）: 選んだ asset にまとめて同じ位置を付ける（`Some`）か、付けた位置を
//!   消してカメラの位置に戻す（`None`）。写真か動画かは見ない（動画を除くのはコア API）。
//! - **1 つのトランザクション**で、変えた asset ごとに変える前の付けた位置を履歴（`location_edit`・
//!   `location_edit_item`）に記録してから書く。途中で失敗すれば何も変えない。範囲外の座標なら、何も変えずに
//!   [`CatalogError::InvalidInput`]。同じ位置がすでに付いている（消すときは付いていない）asset は変えない。
//! - **元に戻す**（[`Catalog::undo_location_edit`]）: 最後の変更から順に取り消す（やり直しはない）。付けた位置は
//!   取り込み・ファイルの読み直しで変わらないので、変える前の値（付けた日時も）をそのまま書き戻す。履歴は
//!   新しい順に [`MAX_LOCATION_EDITS`] 回分だけ残す。
//! - 確かめるだけ（[`Catalog::plan_location_edit`]）: 何も書かずに、変わる数などを返す（画面の見込み）。

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use genzo_model::{AssetId, GpsCoord};
use rusqlite::{OptionalExtension, params};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::library::gps_columns;
use crate::util::{ids_to_json, now_utc_string, parse_db_utc};

/// 位置の変更の履歴を残す数（新しい順。**仮置き**: 20。撮影日時の修正の
/// [`crate::MAX_CAPTURE_TIME_EDITS`] と同じ）。
pub const MAX_LOCATION_EDITS: usize = 20;

/// 一度に読む asset の数（json_each に渡す id の数）。
const CHUNK: usize = 10_000;

/// 変更の種類（`location_edit.kind`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocationEditKind {
    /// 位置を付けた（付けてあった位置は置き換えた）。
    Set,
    /// 付けた位置を消した（カメラの位置に戻した）。
    Clear,
}

impl LocationEditKind {
    /// DB の値。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Clear => "clear",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "set" => Ok(Self::Set),
            "clear" => Ok(Self::Clear),
            _ => Err(CatalogError::Corrupt(format!(
                "位置の変更の種類が不正です: {s:?}"
            ))),
        }
    }

    /// 変更の内容（付ける位置。消すなら `None`）の種類。
    const fn of(location: Option<GpsCoord>) -> Self {
        match location {
            Some(_) => Self::Set,
            None => Self::Clear,
        }
    }
}

/// 変更の見込み（[`Catalog::plan_location_edit`]）。数はどれも asset の数（同じ id は 1 つと数える）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocationPlan {
    /// 対象の asset の数（見つかったもの）。
    pub assets: u64,
    /// 変わる数。
    pub changed: u64,
    /// 変わらない数（同じ位置がすでに付いている。消すときは付けた位置がない）。
    pub unchanged: u64,
    /// 対象のうち、カメラ（ファイル）の位置がある数（付けた位置があれば、そちらを使う）。
    pub with_camera_gps: u64,
    /// 対象のうち、変える前に付けた位置がある数。
    pub with_assigned: u64,
    /// 見つからなかった id の数（削除された asset など）。
    pub missing: u64,
}

/// 変更の結果（[`Catalog::edit_locations`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocationEditOutcome {
    /// 記録した変更（変わったものがなければ `None`）。
    pub edit_id: Option<i64>,
    /// 見込みと同じ数。
    pub plan: LocationPlan,
}

/// 変更の履歴の 1 件（`location_edit`）。
#[derive(Debug, Clone, PartialEq)]
pub struct LocationEditRecord {
    /// ID。
    pub id: i64,
    /// 種類。
    pub kind: LocationEditKind,
    /// 付けた位置（`Set`。`Clear` は `None`）。
    pub location: Option<GpsCoord>,
    /// 変えた asset の数。
    pub asset_count: u64,
    /// 変更した日時（UTC）。
    pub created_at: DateTime<Utc>,
}

/// 取り消しの結果（[`Catalog::undo_location_edit`]）。
#[derive(Debug, Clone, PartialEq)]
pub struct LocationUndoOutcome {
    /// 取り消した変更。
    pub edit: LocationEditRecord,
    /// 戻した asset の数（今の値と変わらなかったものを除く）。
    pub restored: u64,
    /// 変更の後に削除された asset の数（戻すものがない）。
    pub removed: u64,
}

/// 付けた位置（`asset_location` の行）。
#[derive(Debug, Clone, PartialEq)]
struct Assigned {
    gps: GpsCoord,
    /// 付けた日時（DB の文字列のまま）。
    set_at: String,
}

/// 1 つの asset の今の位置。
struct Current {
    id: i64,
    /// カメラ（ファイル）の位置があるか。
    camera: bool,
    assigned: Option<Assigned>,
}

/// 付ける位置が範囲内の有限の値か（消す `None` は常によい）。
fn check_location(location: Option<GpsCoord>) -> Result<()> {
    match location {
        Some(g) if !g.is_valid() => Err(CatalogError::InvalidInput(
            "座標が範囲外です（緯度は -90〜90、経度は -180〜180 の有限の値）".to_owned(),
        )),
        _ => Ok(()),
    }
}

/// DB の緯度・経度を座標にする（表の CHECK で範囲内のはず。外れていれば DB の値の不正）。
fn coord(lat: f64, lon: f64) -> Result<GpsCoord> {
    GpsCoord::new(lat, lon)
        .ok_or_else(|| CatalogError::Corrupt("記録された座標が範囲外です".to_owned()))
}

/// 重ならない id の列（最初に出た順）。
fn distinct_ids(assets: &[AssetId]) -> Vec<i64> {
    let mut seen = HashSet::with_capacity(assets.len());
    assets
        .iter()
        .map(|a| a.get())
        .filter(|id| seen.insert(*id))
        .collect()
}

/// asset の今の位置を、`ids`（重ならないもの）の順に読む（見つからない id は飛ばす）。
fn load_current(conn: &rusqlite::Connection, ids: &[i64]) -> Result<Vec<Current>> {
    let mut out = Vec::with_capacity(ids.len());
    let mut stmt = conn.prepare_cached(
        "SELECT a.id, a.gps_lat, a.gps_lon, al.lat, al.lon, al.set_at
         FROM json_each(?1) j
         JOIN asset a ON a.id = j.value
         LEFT JOIN asset_location al ON al.asset_id = a.id
         ORDER BY j.key",
    )?;
    for chunk in ids.chunks(CHUNK) {
        let mut rows = stmt.query([ids_to_json(chunk)])?;
        while let Some(row) = rows.next()? {
            // カメラの位置は、詳細の表示（Catalog::asset の gps）と同じく、範囲内の両方の値があるときだけ数える。
            let camera = gps_columns(row, 1, 2)?.is_some();
            let assigned = match (row.get::<_, Option<f64>>(3)?, row.get::<_, Option<f64>>(4)?) {
                (Some(lat), Some(lon)) => Some(Assigned {
                    gps: coord(lat, lon)?,
                    set_at: row.get(5)?,
                }),
                _ => None,
            };
            out.push(Current {
                id: row.get(0)?,
                camera,
                assigned,
            });
        }
    }
    Ok(out)
}

/// 変更を当てた見込み（書かない）。変わる asset ごとに、変える前の付けた位置を渡して `on_change` を呼ぶ。
fn plan_changes(
    conn: &rusqlite::Connection,
    assets: &[AssetId],
    location: Option<GpsCoord>,
    mut on_change: impl FnMut(i64, Option<&Assigned>) -> Result<()>,
) -> Result<LocationPlan> {
    check_location(location)?;
    let ids = distinct_ids(assets);
    let current = load_current(conn, &ids)?;
    let mut plan = LocationPlan {
        assets: current.len() as u64,
        missing: ids.len().saturating_sub(current.len()) as u64,
        ..LocationPlan::default()
    };
    for c in &current {
        if c.camera {
            plan.with_camera_gps += 1;
        }
        if c.assigned.is_some() {
            plan.with_assigned += 1;
        }
        if c.assigned.as_ref().map(|a| a.gps) == location {
            plan.unchanged += 1;
            continue;
        }
        plan.changed += 1;
        on_change(c.id, c.assigned.as_ref())?;
    }
    Ok(plan)
}

/// asset の付けた位置を書く（`None` なら消す）。`set_at` は付けた日時（DB の文字列）。
fn write_location(
    conn: &rusqlite::Connection,
    asset_id: i64,
    value: Option<(GpsCoord, &str)>,
) -> Result<()> {
    match value {
        Some((g, set_at)) => {
            conn.prepare_cached(
                "INSERT INTO asset_location(asset_id, lat, lon, set_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(asset_id) DO UPDATE
                     SET lat = excluded.lat, lon = excluded.lon, set_at = excluded.set_at",
            )?
            .execute(params![asset_id, g.lat, g.lon, set_at])?;
        }
        None => {
            conn.prepare_cached("DELETE FROM asset_location WHERE asset_id = ?1")?
                .execute([asset_id])?;
        }
    }
    Ok(())
}

/// `location_edit` の行を読む（列の順は id・kind・lat・lon・asset_count・created_at）。
fn edit_record(row: &rusqlite::Row<'_>) -> Result<LocationEditRecord> {
    let kind: String = row.get(1)?;
    let location = match (row.get::<_, Option<f64>>(2)?, row.get::<_, Option<f64>>(3)?) {
        (Some(lat), Some(lon)) => Some(coord(lat, lon)?),
        _ => None,
    };
    let created: String = row.get(5)?;
    Ok(LocationEditRecord {
        id: row.get(0)?,
        kind: LocationEditKind::parse(&kind)?,
        location,
        asset_count: row.get::<_, i64>(4)?.max(0) as u64,
        created_at: parse_db_utc(&created)?,
    })
}

/// `location_edit_item` の行（asset・変える前の緯度・経度・付けた日時）。
type EditItemRow = (i64, Option<f64>, Option<f64>, Option<String>);

impl Catalog {
    /// 変更の見込み（何も書かない）。`location` は付ける位置（`None` は付けた位置を消す）。範囲外の座標なら
    /// [`CatalogError::InvalidInput`]（変更しても同じエラーになる）。
    pub fn plan_location_edit(
        &self,
        assets: &[AssetId],
        location: Option<GpsCoord>,
    ) -> Result<LocationPlan> {
        plan_changes(&self.conn, assets, location, |_, _| Ok(()))
    }

    /// 位置を付ける・付けた位置を消す（モジュールの doc。1 つのトランザクション）。`location` は付ける位置
    /// （`None` は消して、カメラの位置に戻す）。変わったものがなければ履歴を残さない。
    pub fn edit_locations(
        &mut self,
        assets: &[AssetId],
        location: Option<GpsCoord>,
    ) -> Result<LocationEditOutcome> {
        check_location(location)?;
        let kind = LocationEditKind::of(location);
        let now = now_utc_string();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO location_edit(kind, lat, lon, asset_count, created_at)
             VALUES (?1, ?2, ?3, 0, ?4)",
            params![
                kind.as_str(),
                location.map(|g| g.lat),
                location.map(|g| g.lon),
                now
            ],
        )?;
        let edit_id = tx.last_insert_rowid();
        let plan = {
            let mut item = tx.prepare_cached(
                "INSERT INTO location_edit_item(edit_id, asset_id, prev_lat, prev_lon, prev_set_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            plan_changes(&tx, assets, location, |id, before| {
                item.execute(params![
                    edit_id,
                    id,
                    before.map(|b| b.gps.lat),
                    before.map(|b| b.gps.lon),
                    before.map(|b| b.set_at.as_str())
                ])?;
                write_location(&tx, id, location.map(|g| (g, now.as_str())))
            })?
        };
        let edit_id = if plan.changed == 0 {
            tx.execute("DELETE FROM location_edit WHERE id = ?1", [edit_id])?;
            None
        } else {
            tx.execute(
                "UPDATE location_edit SET asset_count = ?2 WHERE id = ?1",
                params![edit_id, plan.changed as i64],
            )?;
            // 古い履歴を消す（行は ON DELETE CASCADE で消える）。
            tx.execute(
                "DELETE FROM location_edit WHERE id NOT IN (
                     SELECT id FROM location_edit ORDER BY id DESC LIMIT ?1)",
                [MAX_LOCATION_EDITS as i64],
            )?;
            Some(edit_id)
        };
        tx.commit()?;
        Ok(LocationEditOutcome { edit_id, plan })
    }

    /// 変更の履歴（新しい順。最大 [`MAX_LOCATION_EDITS`] 件）。
    pub fn location_edits(&self) -> Result<Vec<LocationEditRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, kind, lat, lon, asset_count, created_at
             FROM location_edit ORDER BY id DESC",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(edit_record(row)?);
        }
        Ok(out)
    }

    /// 最後の変更を取り消す（モジュールの doc。1 つのトランザクション）。履歴がなければ `None`。
    ///
    /// 変更の後に削除された asset は、明細の行も消えている（`ON DELETE CASCADE`）ので、変えた数
    /// （`asset_count`）と残っている明細の数の差を [`LocationUndoOutcome::removed`] に数える。
    pub fn undo_location_edit(&mut self) -> Result<Option<LocationUndoOutcome>> {
        let tx = self.conn.transaction()?;
        let edit = {
            let mut stmt = tx.prepare_cached(
                "SELECT id, kind, lat, lon, asset_count, created_at
                 FROM location_edit ORDER BY id DESC LIMIT 1",
            )?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => edit_record(row)?,
                None => return Ok(None),
            }
        };
        let items: Vec<EditItemRow> = tx
            .prepare_cached(
                "SELECT asset_id, prev_lat, prev_lon, prev_set_at
                 FROM location_edit_item WHERE edit_id = ?1 ORDER BY asset_id",
            )?
            .query_map([edit.id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let ids: Vec<i64> = items.iter().map(|i| i.0).collect();
        let current: HashMap<i64, Option<GpsCoord>> = load_current(&tx, &ids)?
            .into_iter()
            .map(|c| (c.id, c.assigned.map(|a| a.gps)))
            .collect();
        let mut restored = 0u64;
        let mut removed = edit.asset_count.saturating_sub(items.len() as u64);
        for (id, lat, lon, set_at) in items {
            let Some(now) = current.get(&id) else {
                removed += 1;
                continue;
            };
            let previous = match (lat, lon, set_at) {
                (Some(lat), Some(lon), Some(set_at)) => Some((coord(lat, lon)?, set_at)),
                (None, None, None) => None,
                _ => {
                    return Err(CatalogError::Corrupt(format!(
                        "位置の変更の履歴（{}）の asset {id} の値が不正です",
                        edit.id
                    )));
                }
            };
            if previous.as_ref().map(|p| p.0) == *now {
                continue;
            }
            write_location(&tx, id, previous.as_ref().map(|(g, s)| (*g, s.as_str())))?;
            restored += 1;
        }
        tx.execute("DELETE FROM location_edit WHERE id = ?1", [edit.id])?;
        tx.commit()?;
        Ok(Some(LocationUndoOutcome {
            edit,
            restored,
            removed,
        }))
    }

    /// asset に後から付けた位置（付けていなければ `None`。カメラの位置は [`crate::AssetRecord::gps`]）。
    /// asset がなければ [`CatalogError::NotFound`]。
    pub fn asset_location(&self, asset_id: AssetId) -> Result<Option<GpsCoord>> {
        let row: Option<(Option<f64>, Option<f64>)> = self
            .conn
            .prepare_cached(
                "SELECT al.lat, al.lon FROM asset a
                 LEFT JOIN asset_location al ON al.asset_id = a.id
                 WHERE a.id = ?1",
            )?
            .query_row([asset_id.get()], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        match row.ok_or_else(|| CatalogError::NotFound(format!("asset {asset_id}")))? {
            (Some(lat), Some(lon)) => Ok(Some(coord(lat, lon)?)),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_and_follow_the_location() {
        for k in [LocationEditKind::Set, LocationEditKind::Clear] {
            assert_eq!(LocationEditKind::parse(k.as_str()).unwrap(), k);
        }
        assert!(matches!(
            LocationEditKind::parse("gpx"),
            Err(CatalogError::Corrupt(_))
        ));
        let g = GpsCoord::new(35.681236, 139.767125);
        assert_eq!(LocationEditKind::of(g), LocationEditKind::Set);
        assert_eq!(LocationEditKind::of(None), LocationEditKind::Clear);
    }

    #[test]
    fn locations_are_checked_and_ids_are_distinct() {
        assert!(check_location(None).is_ok());
        assert!(check_location(GpsCoord::new(-90.0, 180.0)).is_ok());
        for (lat, lon) in [
            (90.5, 0.0),
            (0.0, -180.5),
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
        ] {
            assert!(
                matches!(
                    check_location(Some(GpsCoord { lat, lon })),
                    Err(CatalogError::InvalidInput(_))
                ),
                "{lat}, {lon}"
            );
        }
        let ids = [3, 1, 3, 2, 1].map(AssetId::new);
        assert_eq!(distinct_ids(&ids), vec![3, 1, 2]);
    }
}
