//! 撮影日時の一括修正（LIB-16。docs/04_architecture.md の 3.1 節・3.13 節）と、タイムライン（ORG-01）のための
//! 撮影日時の読み取り。
//!
//! - **機材**（[`DeviceKey`]）: カメラの表示名（`asset.camera`）・本体のシリアル番号（`asset.camera_serial`）・
//!   写真か動画か の組。時計は機材ごとにずれるので、ずれを機材ごとにまとめて直す（[`Catalog::capture_devices`]・
//!   [`Catalog::assets_of_devices`]）。シリアル番号を記録しないカメラは、同じ機種をまとめて 1 つの機材とみなす。
//! - **修正**（[`CaptureAdjustment`]）: 時計のずれを足す（`time_correction_s` に加える）・タイムゾーンを指定する
//!   （読み方は genzo-model の `ZoneMode`）・指定を取り消す・ファイルの値に戻す。元の値（`captured_at_raw`・
//!   `captured_offset`）は変えず、元のファイルにも書かない（非破壊）。撮影日時の分からない写真は変えない。
//! - **1 つのトランザクション**で、変えた asset ごとに変える前の修正の状態を履歴（`capture_time_edit`・
//!   `capture_time_edit_item`。スキーマの版 11）に記録してから書く。途中で失敗すれば何も変えない。範囲外に
//!   なる写真が 1 枚でもあれば、すべて変えずに [`CatalogError::InvalidInput`]。
//! - **元に戻す**（[`Catalog::undo_capture_time_edit`]）: 最後の修正から順に取り消す。変える前の値をそのまま
//!   書くのではなく、今の撮影日時に変える前の修正の状態（補正値・タイムゾーンの指定）を当て直す（修正の後に
//!   既定のタイムゾーンを変えた・ファイルを読み直した場合も、その結果を保つため）。履歴は新しい順に
//!   [`MAX_CAPTURE_TIME_EDITS`] 回分だけ残す。
//! - 確かめるだけ（[`Catalog::plan_capture_adjustment`]）: 何も書かずに、変わる数と例を返す（画面の見本）。

use chrono::{DateTime, FixedOffset, Utc};
use genzo_model::capture_time::{format_offset, parse_offset};
use genzo_model::{AssetId, AssetKind, CaptureTime, TzSource, VariantId, ZoneMode};
use rusqlite::{OptionalExtension, params};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::library::write_capture;
use crate::register::{CaptureColumns, capture_from_columns};
use crate::util::{ids_to_json, now_utc_string, parse_db_utc, parse_enum};

/// 撮影日時の修正の履歴を残す数（新しい順。**仮置き**: 20）。
pub const MAX_CAPTURE_TIME_EDITS: usize = 20;

/// 時計のずれとして足せる量の上限（秒。**仮置き**: 100 年 = 36,525 日）。電池が切れて 2000 年などに戻った
/// 時計も直せる大きさ。
pub const MAX_CAPTURE_SHIFT_SECONDS: i64 = 36_525 * 86_400;

/// 一度に読む asset の数（json_each に渡す id の数）。
const CHUNK: usize = 10_000;

/// タイムラインの撮影日時を、表を順に読んで求めることを考える id の数の下限（これより少なければ id ごとに引く）。
const SCAN_MIN_IDS: usize = 20_000;

/// 機材（カメラの表示名・本体のシリアル番号・写真か動画か）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceKey {
    /// カメラの表示名（`asset.camera`。分からなければ `None`）。
    pub camera: Option<String>,
    /// 本体のシリアル番号（`asset.camera_serial`。記録しないカメラは `None`）。
    pub serial: Option<String>,
    /// 写真か動画か（同じ名前の機材でも、写真と動画は別の機材として挙げる。まとめて直すときは両方を選ぶ）。
    pub kind: AssetKind,
}

/// 機材ごとの数（[`Catalog::capture_devices`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// 機材。
    pub key: DeviceKey,
    /// asset の数。
    pub assets: u64,
    /// 撮影日時の分かる asset の数。
    pub with_time: u64,
    /// 利用者の修正（時計のずれの補正・タイムゾーンの指定）のある asset の数。
    pub adjusted: u64,
    /// 最も早い撮影日時（補正後の UTC）。
    pub first: Option<DateTime<Utc>>,
    /// 最も遅い撮影日時（補正後の UTC）。
    pub last: Option<DateTime<Utc>>,
}

/// 撮影日時の修正（[`Catalog::adjust_capture_times`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureAdjustment {
    /// 時計のずれを足す（秒。今の補正値に加える。正で遅らせる＝後の時刻にする）。
    Shift {
        /// 秒。
        seconds: i64,
    },
    /// タイムゾーンを指定する（genzo-model の `CaptureTime::with_zone`）。
    SetZone {
        /// オフセット。
        offset: FixedOffset,
        /// 読み方。
        mode: ZoneMode,
    },
    /// タイムゾーンの指定を取り消す（ファイルの記録、なければ既定のタイムゾーンに戻す。補正値は保つ）。
    ClearZone,
    /// 利用者の修正をすべて取り消し、ファイルの値に戻す。
    Reset,
}

impl CaptureAdjustment {
    /// 1 枚の撮影日時に当てた結果。
    pub fn apply(
        &self,
        current: &CaptureTime,
        default_offset: FixedOffset,
    ) -> std::result::Result<CaptureTime, genzo_model::CaptureTimeError> {
        match *self {
            Self::Shift { seconds } => {
                if current.utc.is_none() {
                    return Ok(current.clone());
                }
                let next = current
                    .correction_s
                    .checked_add(seconds)
                    .ok_or(genzo_model::CaptureTimeError::OutOfRange)?;
                current.with_correction(next)
            }
            Self::SetZone { offset, mode } => current.with_zone(offset, mode),
            Self::ClearZone => {
                if current.tz_source == TzSource::UserSet {
                    current.without_user_offset(default_offset)
                } else {
                    Ok(current.clone())
                }
            }
            Self::Reset => {
                if current.utc.is_none() {
                    // 日時がなければ変えない（補正値だけが残っていても、表示・並びに効かない）。
                    return Ok(current.clone());
                }
                current.without_user_changes(default_offset)
            }
        }
    }
}

/// 修正の種類（`capture_time_edit.kind`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureEditKind {
    /// ずらす。
    Shift,
    /// 基準の写真の正しい時刻に合わせる（ずらす量は基準の写真から求めた）。
    Sync,
    /// タイムゾーンの指定。
    SetZone,
    /// 指定の取り消し。
    ClearZone,
    /// ファイルの値に戻す。
    Reset,
}

impl CaptureEditKind {
    /// DB の値。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shift => "shift",
            Self::Sync => "sync",
            Self::SetZone => "set_zone",
            Self::ClearZone => "clear_zone",
            Self::Reset => "reset",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "shift" => Self::Shift,
            "sync" => Self::Sync,
            "set_zone" => Self::SetZone,
            "clear_zone" => Self::ClearZone,
            "reset" => Self::Reset,
            _ => {
                return Err(CatalogError::Corrupt(format!(
                    "撮影日時の修正の種類が不正です: {s:?}"
                )));
            }
        })
    }

    /// 修正に合う種類か（`Sync` は `Shift` の修正）。
    fn matches(self, adj: &CaptureAdjustment) -> bool {
        matches!(
            (self, adj),
            (Self::Shift | Self::Sync, CaptureAdjustment::Shift { .. })
                | (Self::SetZone, CaptureAdjustment::SetZone { .. })
                | (Self::ClearZone, CaptureAdjustment::ClearZone)
                | (Self::Reset, CaptureAdjustment::Reset)
        )
    }
}

/// 1 枚の変化（見本）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureChange {
    /// asset。
    pub asset_id: AssetId,
    /// 変える前。
    pub before: CaptureTime,
    /// 変えた後。
    pub after: CaptureTime,
}

/// 対象の撮影日時の出どころごとの数（撮影日時の分かるもの。修正の前の値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TzSourceCounts {
    /// ファイルにオフセットが記録されていた。
    pub exif: u64,
    /// UTC の印（`Z`）だけが記録されていた。
    pub utc_only: u64,
    /// 記録がなく既定のタイムゾーンで推定した。
    pub user_default: u64,
    /// 利用者が指定した。
    pub user_set: u64,
}

impl TzSourceCounts {
    fn add(&mut self, source: TzSource) {
        match source {
            TzSource::Exif => self.exif += 1,
            TzSource::UtcOnly => self.utc_only += 1,
            TzSource::UserDefault => self.user_default += 1,
            TzSource::UserSet => self.user_set += 1,
        }
    }
}

/// 修正の見込み（[`Catalog::plan_capture_adjustment`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CapturePlan {
    /// 対象の asset の数（見つかったもの）。
    pub assets: u64,
    /// 変わる数。
    pub changed: u64,
    /// 変わらない数（撮影日時の分かるもの）。
    pub unchanged: u64,
    /// 撮影日時の分からない数（変えない）。
    pub without_time: u64,
    /// 見つからなかった id の数（削除された asset など）。
    pub missing: u64,
    /// 撮影日時の出どころごとの数（修正の前）。
    pub sources: TzSourceCounts,
    /// 変わるものの例（対象の順に最大 `example_limit` 件）。
    pub examples: Vec<CaptureChange>,
}

/// 修正の結果（[`Catalog::adjust_capture_times`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CaptureAdjustOutcome {
    /// 記録した修正（変わったものがなければ `None`）。
    pub edit_id: Option<i64>,
    /// 見込みと同じ数。
    pub plan: CapturePlan,
}

/// 修正の履歴の 1 件（`capture_time_edit`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureEditRecord {
    /// ID。
    pub id: i64,
    /// 種類。
    pub kind: CaptureEditKind,
    /// ずらした秒数（`Shift`・`Sync`）。
    pub shift_s: Option<i64>,
    /// 指定したオフセット（`SetZone`。`"+09:00"` の形）。
    pub zone: Option<String>,
    /// 指定の読み方（`SetZone`）。
    pub zone_mode: Option<ZoneMode>,
    /// 変えた asset の数。
    pub asset_count: u64,
    /// 修正した日時（UTC）。
    pub created_at: DateTime<Utc>,
}

/// 取り消しの結果（[`Catalog::undo_capture_time_edit`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureUndoOutcome {
    /// 取り消した修正。
    pub edit: CaptureEditRecord,
    /// 戻した asset の数（今の値と変わらなかったものを除く）。
    pub restored: u64,
    /// 修正の後に削除された asset の数（戻すものがない）。
    pub removed: u64,
}

/// タイムラインのための 1 枚の撮影日時（[`Catalog::timeline_keys`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimelineKey {
    /// 補正後の UTC（UNIX 時刻のミリ秒。撮影日時が分からなければ `None`）。
    pub utc_ms: Option<i64>,
    /// 表示のオフセット（秒。`tz_assumed`。読めなければ 0）。
    pub offset_s: i32,
    /// 動画か。
    pub video: bool,
}

fn zone_mode_str(mode: ZoneMode) -> &'static str {
    match mode {
        ZoneMode::KeepLocal => "keep_local",
        ZoneMode::KeepInstant => "keep_instant",
    }
}

fn parse_zone_mode(s: &str) -> Result<ZoneMode> {
    match s {
        "keep_local" => Ok(ZoneMode::KeepLocal),
        "keep_instant" => Ok(ZoneMode::KeepInstant),
        _ => Err(CatalogError::Corrupt(format!(
            "タイムゾーンの指定の読み方が不正です: {s:?}"
        ))),
    }
}

/// asset の撮影日時を、id の順にまとめて読む（見つからない id は飛ばす。同じ id は 1 回）。
fn load_captures(conn: &rusqlite::Connection, ids: &[i64]) -> Result<Vec<(i64, CaptureTime)>> {
    let mut out = Vec::with_capacity(ids.len());
    let mut stmt = conn.prepare_cached(
        "SELECT a.id, a.captured_at_raw, a.captured_offset, a.tz_source, a.tz_assumed,
                a.time_correction_s, a.captured_at_utc, a.tz_basis
         FROM (SELECT min(key) AS k, value AS id FROM json_each(?1) GROUP BY value) j
         JOIN asset a ON a.id = j.id
         ORDER BY j.k",
    )?;
    for chunk in ids.chunks(CHUNK) {
        let mut rows = stmt.query([ids_to_json(chunk)])?;
        while let Some(row) = rows.next()? {
            let source: String = row.get(3)?;
            let utc: Option<String> = row.get(6)?;
            let capture = capture_from_columns(CaptureColumns {
                raw: row.get(1)?,
                offset: row.get(2)?,
                tz_source: &source,
                tz_assumed: row.get(4)?,
                basis: row.get(7)?,
                correction_s: row.get(5)?,
                utc: utc.as_deref(),
            })?;
            out.push((row.get(0)?, capture));
        }
    }
    Ok(out)
}

/// 重ならない id の数。
fn distinct_count(ids: &[AssetId]) -> u64 {
    let set: std::collections::HashSet<AssetId> = ids.iter().copied().collect();
    set.len() as u64
}

/// 修正を当てた見込み（書かない）と、変わるもの。
fn plan_changes(
    conn: &rusqlite::Connection,
    assets: &[AssetId],
    adj: &CaptureAdjustment,
    default_offset: FixedOffset,
    example_limit: usize,
    mut on_change: impl FnMut(i64, &CaptureTime, &CaptureTime) -> Result<()>,
) -> Result<CapturePlan> {
    if let CaptureAdjustment::Shift { seconds } = adj
        && seconds.unsigned_abs() > MAX_CAPTURE_SHIFT_SECONDS.unsigned_abs()
    {
        return Err(CatalogError::InvalidInput(format!(
            "ずらす量は ±{} 日までです（{seconds} 秒）",
            MAX_CAPTURE_SHIFT_SECONDS / 86_400
        )));
    }
    let ids: Vec<i64> = assets.iter().map(|a| a.get()).collect();
    let captures = load_captures(conn, &ids)?;
    let mut plan = CapturePlan {
        assets: captures.len() as u64,
        missing: distinct_count(assets).saturating_sub(captures.len() as u64),
        ..CapturePlan::default()
    };
    let mut out_of_range = 0u64;
    for (id, current) in &captures {
        if current.utc.is_none() {
            plan.without_time += 1;
            continue;
        }
        plan.sources.add(current.tz_source);
        let next = match adj.apply(current, default_offset) {
            Ok(n) => n,
            Err(_) => {
                out_of_range += 1;
                continue;
            }
        };
        if next == *current {
            plan.unchanged += 1;
            continue;
        }
        plan.changed += 1;
        if plan.examples.len() < example_limit {
            plan.examples.push(CaptureChange {
                asset_id: AssetId::new(*id),
                before: current.clone(),
                after: next.clone(),
            });
        }
        on_change(*id, current, &next)?;
    }
    if out_of_range > 0 {
        return Err(CatalogError::InvalidInput(format!(
            "{out_of_range} 枚の撮影日時が表せる範囲（0〜9999 年）を超えるため、修正できません"
        )));
    }
    Ok(plan)
}

/// 変える前の修正の状態（`capture_time_edit_item` の行）を、今の撮影日時に当て直す。
fn restore_state(
    current: &CaptureTime,
    prev_source: TzSource,
    prev_assumed: Option<String>,
    prev_basis: Option<String>,
    prev_correction_s: i64,
    default_offset: FixedOffset,
) -> std::result::Result<CaptureTime, genzo_model::CaptureTimeError> {
    let base = current.without_user_changes(default_offset)?;
    if prev_source == TzSource::UserSet {
        let previous = CaptureTime {
            tz_source: TzSource::UserSet,
            tz_assumed: prev_assumed,
            basis: prev_basis,
            correction_s: prev_correction_s,
            ..base.clone()
        };
        Ok(base.with_user_changes_of(&previous))
    } else {
        base.with_correction(prev_correction_s)
    }
}

impl Catalog {
    /// 機材の一覧（カメラの表示名・シリアル番号・種類の順。分からない機材は最後）。表を 1 回読む（50 万件で
    /// asset の表の全体）。
    pub fn capture_devices(&self) -> Result<Vec<DeviceInfo>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT camera, camera_serial, kind, count(*), count(captured_at_utc),
                    sum(CASE WHEN tz_source = 'user_set' OR time_correction_s <> 0 THEN 1 ELSE 0 END),
                    min(captured_at_utc), max(captured_at_utc)
             FROM asset
             GROUP BY camera, camera_serial, kind
             ORDER BY camera IS NULL, camera, camera_serial IS NULL, camera_serial, kind",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let kind: String = row.get(2)?;
            let first: Option<String> = row.get(6)?;
            let last: Option<String> = row.get(7)?;
            out.push(DeviceInfo {
                key: DeviceKey {
                    camera: row.get(0)?,
                    serial: row.get(1)?,
                    kind: parse_enum(&kind)?,
                },
                assets: row.get::<_, i64>(3)?.max(0) as u64,
                with_time: row.get::<_, i64>(4)?.max(0) as u64,
                adjusted: row.get::<_, i64>(5)?.max(0) as u64,
                first: first.as_deref().map(parse_db_utc).transpose()?,
                last: last.as_deref().map(parse_db_utc).transpose()?,
            });
        }
        Ok(out)
    }

    /// 機材の asset（機材の順、その中は asset の ID の順。重ならない）。
    pub fn assets_of_devices(&self, devices: &[DeviceKey]) -> Result<Vec<AssetId>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id FROM asset
             WHERE camera IS ?1 AND camera_serial IS ?2 AND kind = ?3 ORDER BY id",
        )?;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for d in devices {
            let rows = stmt.query_map(params![d.camera, d.serial, d.kind.as_str()], |r| {
                r.get::<_, i64>(0)
            })?;
            for r in rows {
                let id = AssetId::new(r?);
                if seen.insert(id) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// variant の asset の機材（写真が見つからなければ [`CatalogError::NotFound`]）。
    pub fn device_of_variant(&self, variant_id: VariantId) -> Result<DeviceKey> {
        let row: Option<(Option<String>, Option<String>, String)> = self
            .conn
            .prepare_cached(
                "SELECT a.camera, a.camera_serial, a.kind FROM variant v JOIN asset a ON a.id = v.asset_id
                 WHERE v.id = ?1",
            )?
            .query_row([variant_id.get()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        let (camera, serial, kind) =
            row.ok_or_else(|| CatalogError::NotFound(format!("variant {variant_id}")))?;
        Ok(DeviceKey {
            camera,
            serial,
            kind: parse_enum(&kind)?,
        })
    }

    /// 修正の見込み（何も書かない）。`example_limit` は返す例の数の上限。範囲外になる写真があれば
    /// [`CatalogError::InvalidInput`]（修正しても同じエラーになる）。
    pub fn plan_capture_adjustment(
        &self,
        assets: &[AssetId],
        adj: &CaptureAdjustment,
        default_offset: FixedOffset,
        example_limit: usize,
    ) -> Result<CapturePlan> {
        plan_changes(
            &self.conn,
            assets,
            adj,
            default_offset,
            example_limit,
            |_, _, _| Ok(()),
        )
    }

    /// 撮影日時を修正する（モジュールの doc。1 つのトランザクション）。`kind` は履歴に残す種類（`adj` に合う
    /// もの。基準の写真に合わせたずらしは [`CaptureEditKind::Sync`]）。`default_offset` は既定のタイムゾーン
    /// （指定の取り消し・ファイルの値に戻すときに使う）。変わったものがなければ履歴を残さない。
    pub fn adjust_capture_times(
        &mut self,
        assets: &[AssetId],
        adj: &CaptureAdjustment,
        kind: CaptureEditKind,
        default_offset: FixedOffset,
        example_limit: usize,
    ) -> Result<CaptureAdjustOutcome> {
        if !kind.matches(adj) {
            return Err(CatalogError::InvalidInput(format!(
                "修正の種類 {} と内容が合いません",
                kind.as_str()
            )));
        }
        let tx = self.conn.transaction()?;
        let (shift_s, zone, zone_mode) = match *adj {
            CaptureAdjustment::Shift { seconds } => (Some(seconds), None, None),
            CaptureAdjustment::SetZone { offset, mode } => {
                (None, Some(format_offset(offset)), Some(zone_mode_str(mode)))
            }
            CaptureAdjustment::ClearZone | CaptureAdjustment::Reset => (None, None, None),
        };
        tx.execute(
            "INSERT INTO capture_time_edit(kind, shift_s, zone, zone_mode, asset_count, created_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
            params![kind.as_str(), shift_s, zone, zone_mode, now_utc_string()],
        )?;
        let edit_id = tx.last_insert_rowid();
        let plan = {
            let mut item = tx.prepare_cached(
                "INSERT INTO capture_time_edit_item(edit_id, asset_id, prev_tz_source,
                     prev_tz_assumed, prev_tz_basis, prev_correction_s)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            plan_changes(
                &tx,
                assets,
                adj,
                default_offset,
                example_limit,
                |id, before, after| {
                    item.execute(params![
                        edit_id,
                        id,
                        before.tz_source.as_str(),
                        before.tz_assumed,
                        before.basis,
                        before.correction_s
                    ])?;
                    write_capture(&tx, id, after)?;
                    Ok(())
                },
            )?
        };
        let edit_id = if plan.changed == 0 {
            tx.execute("DELETE FROM capture_time_edit WHERE id = ?1", [edit_id])?;
            None
        } else {
            tx.execute(
                "UPDATE capture_time_edit SET asset_count = ?2 WHERE id = ?1",
                params![edit_id, plan.changed as i64],
            )?;
            // 古い履歴を消す（行は ON DELETE CASCADE で消える）。
            tx.execute(
                "DELETE FROM capture_time_edit WHERE id NOT IN (
                     SELECT id FROM capture_time_edit ORDER BY id DESC LIMIT ?1)",
                [MAX_CAPTURE_TIME_EDITS as i64],
            )?;
            Some(edit_id)
        };
        tx.commit()?;
        Ok(CaptureAdjustOutcome { edit_id, plan })
    }

    /// 修正の履歴（新しい順。最大 [`MAX_CAPTURE_TIME_EDITS`] 件）。
    pub fn capture_time_edits(&self) -> Result<Vec<CaptureEditRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, kind, shift_s, zone, zone_mode, asset_count, created_at
             FROM capture_time_edit ORDER BY id DESC",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(edit_record(row)?);
        }
        Ok(out)
    }

    /// 最後の修正を取り消す（モジュールの doc。1 つのトランザクション）。履歴がなければ `None`。
    pub fn undo_capture_time_edit(
        &mut self,
        default_offset: FixedOffset,
    ) -> Result<Option<CaptureUndoOutcome>> {
        let tx = self.conn.transaction()?;
        let edit = {
            let mut stmt = tx.prepare_cached(
                "SELECT id, kind, shift_s, zone, zone_mode, asset_count, created_at
                 FROM capture_time_edit ORDER BY id DESC LIMIT 1",
            )?;
            let mut rows = stmt.query([])?;
            match rows.next()? {
                Some(row) => edit_record(row)?,
                None => return Ok(None),
            }
        };
        let items: Vec<EditItemRow> = tx
            .prepare_cached(
                "SELECT asset_id, prev_tz_source, prev_tz_assumed, prev_tz_basis, prev_correction_s
                 FROM capture_time_edit_item WHERE edit_id = ?1 ORDER BY asset_id",
            )?
            .query_map([edit.id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let ids: Vec<i64> = items.iter().map(|i| i.0).collect();
        let current: std::collections::HashMap<i64, CaptureTime> =
            load_captures(&tx, &ids)?.into_iter().collect();
        let mut restored = 0u64;
        let mut removed = 0u64;
        for (id, source, assumed, basis, correction) in items {
            let Some(now) = current.get(&id) else {
                removed += 1;
                continue;
            };
            let source: TzSource = parse_enum(&source)?;
            let next = restore_state(now, source, assumed, basis, correction, default_offset)
                .map_err(|e| {
                    CatalogError::InvalidInput(format!(
                        "asset {id} の撮影日時を元に戻せません: {e}"
                    ))
                })?;
            if next != *now {
                write_capture(&tx, id, &next)?;
                restored += 1;
            }
        }
        tx.execute("DELETE FROM capture_time_edit WHERE id = ?1", [edit.id])?;
        tx.commit()?;
        Ok(Some(CaptureUndoOutcome {
            edit,
            restored,
            removed,
        }))
    }

    /// 写真の撮影日時（補正後の UTC・表示のオフセット・動画か）を `ids` と同じ順で返す（タイムライン。ORG-01）。
    /// 見つからない id（検索の後に削除されたもの）は、撮影日時の分からないものとして返す。
    ///
    /// `ids` が 2 万件以上で、カタログの写真の 1/4 以上なら、asset と variant の表を 1 回ずつ順に読む（id ごとに
    /// 引くより速い。50 万件のダミーのカタログの全件（55 万）で、順に読む方法が約 0.41 秒、id ごとに引く方法が
    /// 約 0.90 秒だった（テストの `capture_scale_measurement`。release ビルド・このコンテナでの参考値。docs/04 の
    /// 3.13 節））。少なければ id ごとに引く。
    pub fn timeline_keys(&self, ids: &[VariantId]) -> Result<Vec<TimelineKey>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // 少なければ、表の数を数えずに id ごとに引く（数えるだけで 50 万件では数十ミリ秒かかる）。
        if ids.len() < SCAN_MIN_IDS {
            return self.timeline_keys_by_id(ids);
        }
        let (max_variant, variants): (i64, i64) = self.conn.query_row(
            "SELECT ifnull(max(id), 0), count(*) FROM variant",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let (max_asset, assets): (i64, i64) =
            self.conn
                .query_row("SELECT ifnull(max(id), 0), count(*) FROM asset", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        // id が疎ら（削除が多い）なら、id で引く表が大きくなりすぎるので使わない。
        let dense = |max: i64, n: i64| max <= n.saturating_mul(4).saturating_add(1_000_000);
        if (ids.len() as i64).saturating_mul(4) >= variants
            && dense(max_variant, variants)
            && dense(max_asset, assets)
        {
            self.timeline_keys_by_scan(ids, max_variant, max_asset)
        } else {
            self.timeline_keys_by_id(ids)
        }
    }

    /// [`Catalog::timeline_keys`] の、id ごとに引く方法。
    pub(crate) fn timeline_keys_by_id(&self, ids: &[VariantId]) -> Result<Vec<TimelineKey>> {
        let mut out = vec![TimelineKey::default(); ids.len()];
        let mut stmt = self.conn.prepare_cached(
            "SELECT j.key, a.kind, a.captured_at_utc, a.tz_assumed
             FROM json_each(?1) j
             JOIN variant v ON v.id = j.value
             JOIN asset a ON a.id = v.asset_id",
        )?;
        let mut offsets = OffsetCache::default();
        for (c, chunk) in ids.chunks(CHUNK).enumerate() {
            let base = c * CHUNK;
            let mut rows = stmt.query([ids_to_json(chunk)])?;
            while let Some(row) = rows.next()? {
                let key: i64 = row.get(0)?;
                let Some(slot) = usize::try_from(key)
                    .ok()
                    .and_then(|k| out.get_mut(base + k))
                else {
                    continue;
                };
                *slot = timeline_key_of(row, 1, &mut offsets)?;
            }
        }
        Ok(out)
    }

    /// [`Catalog::timeline_keys`] の、表を順に読む方法（id は `max_*` 以下の正の数）。
    pub(crate) fn timeline_keys_by_scan(
        &self,
        ids: &[VariantId],
        max_variant: i64,
        max_asset: i64,
    ) -> Result<Vec<TimelineKey>> {
        let len = |max: i64| usize::try_from(max).unwrap_or(0).saturating_add(1);
        let mut by_asset = vec![TimelineKey::default(); len(max_asset)];
        let mut offsets = OffsetCache::default();
        {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT id, kind, captured_at_utc, tz_assumed FROM asset")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                if let Some(slot) = usize::try_from(id).ok().and_then(|i| by_asset.get_mut(i)) {
                    *slot = timeline_key_of(row, 1, &mut offsets)?;
                }
            }
        }
        // variant → asset（0 は見つからない）。索引 variant_asset だけを読む。
        let mut asset_of = vec![0u32; len(max_variant)];
        {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT id, asset_id FROM variant")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let asset: i64 = row.get(1)?;
                if let (Some(slot), Ok(a)) = (
                    usize::try_from(id).ok().and_then(|i| asset_of.get_mut(i)),
                    u32::try_from(asset),
                ) {
                    *slot = a;
                }
            }
        }
        Ok(ids
            .iter()
            .map(|v| {
                usize::try_from(v.get())
                    .ok()
                    .and_then(|i| asset_of.get(i))
                    .filter(|&&a| a != 0)
                    .and_then(|&a| by_asset.get(a as usize))
                    .copied()
                    .unwrap_or_default()
            })
            .collect())
    }
}

/// 表示のオフセットの文字列（少ない種類）を解析した値。
#[derive(Default)]
struct OffsetCache(Vec<(String, i32)>);

impl OffsetCache {
    fn seconds(&mut self, s: &str) -> i32 {
        if let Some((_, v)) = self.0.iter().find(|(k, _)| k == s) {
            return *v;
        }
        let v = parse_offset(s).map(|o| o.local_minus_utc()).unwrap_or(0);
        self.0.push((s.to_owned(), v));
        v
    }
}

/// 行の `first` 列から順に kind・captured_at_utc・tz_assumed を読む。
fn timeline_key_of(
    row: &rusqlite::Row<'_>,
    first: usize,
    offsets: &mut OffsetCache,
) -> Result<TimelineKey> {
    let kind = row
        .get_ref(first)?
        .as_str()
        .map_err(rusqlite::Error::from)?;
    let utc = row
        .get_ref(first + 1)?
        .as_str_or_null()
        .map_err(rusqlite::Error::from)?;
    let assumed = row
        .get_ref(first + 2)?
        .as_str_or_null()
        .map_err(rusqlite::Error::from)?;
    Ok(TimelineKey {
        utc_ms: match utc {
            Some(s) => Some(match fast_db_utc_ms(s) {
                Some(ms) => ms,
                None => parse_db_utc(s)?.timestamp_millis(),
            }),
            None => None,
        },
        offset_s: assumed.map(|a| offsets.seconds(a)).unwrap_or(0),
        video: kind == AssetKind::Video.as_str(),
    })
}

/// `capture_time_edit_item` の行（asset・変える前の tz_source・tz_assumed・tz_basis・time_correction_s）。
type EditItemRow = (i64, String, Option<String>, Option<String>, i64);

/// `capture_time_edit` の行を読む（列の順は id・kind・shift_s・zone・zone_mode・asset_count・created_at）。
fn edit_record(row: &rusqlite::Row<'_>) -> Result<CaptureEditRecord> {
    let kind: String = row.get(1)?;
    let mode: Option<String> = row.get(4)?;
    let created: String = row.get(6)?;
    Ok(CaptureEditRecord {
        id: row.get(0)?,
        kind: CaptureEditKind::parse(&kind)?,
        shift_s: row.get(2)?,
        zone: row.get(3)?,
        zone_mode: mode.as_deref().map(parse_zone_mode).transpose()?,
        asset_count: row.get::<_, i64>(5)?.max(0) as u64,
        created_at: parse_db_utc(&created)?,
    })
}

/// DB の固定長の UTC の文字列（`"2024-05-01T03:04:05.000Z"`）を UNIX 時刻のミリ秒にする（速い道）。形が違えば
/// `None`（呼び出し側が chrono で読む）。
fn fast_db_utc_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 24
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'.'
        || b[23] != b'Z'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let mut v = 0i64;
        for &c in &b[r] {
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + i64::from(c - b'0');
        }
        Some(v)
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec, ms) = (num(11..13)?, num(14..16)?, num(17..19)?, num(20..23)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    // 1970-01-01 からの日数（Howard Hinnant の days_from_civil）。
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1000 + ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_utc_parser_matches_chrono() {
        for s in [
            "1970-01-01T00:00:00.000Z",
            "2024-02-29T23:59:59.999Z",
            "2024-05-01T03:04:05.250Z",
            "0000-01-01T00:00:00.000Z",
            "1899-12-31T12:00:00.001Z",
            "9999-12-31T23:59:59.999Z",
            "2000-03-01T00:00:00.000Z",
        ] {
            assert_eq!(
                fast_db_utc_ms(s),
                Some(parse_db_utc(s).unwrap().timestamp_millis()),
                "{s}"
            );
        }
        for s in [
            "2024-05-01T03:04:05Z",
            "2024-13-01T00:00:00.000Z",
            "2024-05-01 03:04:05.000Z",
            "２０２４-05-01T03:04:05.000Z",
        ] {
            assert_eq!(fast_db_utc_ms(s), None, "{s}");
        }
    }

    #[test]
    fn kinds_round_trip_and_match_adjustments() {
        for k in [
            CaptureEditKind::Shift,
            CaptureEditKind::Sync,
            CaptureEditKind::SetZone,
            CaptureEditKind::ClearZone,
            CaptureEditKind::Reset,
        ] {
            assert_eq!(CaptureEditKind::parse(k.as_str()).unwrap(), k);
        }
        assert!(CaptureEditKind::parse("move").is_err());
        let shift = CaptureAdjustment::Shift { seconds: 5 };
        assert!(CaptureEditKind::Sync.matches(&shift));
        assert!(CaptureEditKind::Shift.matches(&shift));
        assert!(!CaptureEditKind::Reset.matches(&shift));
        assert!(CaptureEditKind::ClearZone.matches(&CaptureAdjustment::ClearZone));
        for m in [ZoneMode::KeepLocal, ZoneMode::KeepInstant] {
            assert_eq!(parse_zone_mode(zone_mode_str(m)).unwrap(), m);
        }
    }
}
