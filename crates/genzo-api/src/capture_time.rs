//! 撮影日時の一括修正（LIB-16。docs/04_architecture.md の 3.13 節）。
//!
//! - 対象（[`CaptureTarget`]）は選んだ写真か、機材（カメラの表示名・本体のシリアル番号・写真か動画か。
//!   [`Core::capture_devices`]）のすべての写真・動画。同じ写真の仮想コピーは撮影日時を共有する（asset に付く）。
//! - 修正（[`CaptureTimeEdit`]）: ずらす（日・時・分・秒を秒にしたもの）・基準の写真の正しい時刻に合わせる
//!   （差を求めてずらす）・タイムゾーンの指定（2 つの読み方）・指定の取り消し・ファイルの値に戻す。元の値と
//!   元のファイルは変えない（genzo-catalog の `capture`）。書き出しのメタデータ（Exif・XMP）には修正後の値が
//!   入る（`export`）。
//! - [`Core::plan_capture_time_edit`] は何も変えずに見込みを返す（画面の確認）。[`Core::edit_capture_times`] は
//!   1 つのトランザクションで変え、履歴に残す（[`Core::undo_capture_time_edit`] で最後のものから取り消す）。
//! - 変えたら [`Event::CatalogChanged`]（[`CatalogChange::CaptureTime`]・すべて）で知らせ、表示中の検索結果を
//!   作り直す（撮影日時の順の並び・期間の絞り込み・タイムラインの日付のまとまりが変わるため）。

use chrono::{DateTime, FixedOffset, TimeDelta};
use genzo_catalog::{CaptureAdjustment, CaptureEditKind, CapturePlan, Catalog};
use genzo_model::{AssetId, FileRole, capture_time::parse_datetime};

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::{CatalogChange, Event};
use crate::types::{
    CaptureDevice, CaptureTarget, CaptureTimeEdit, CaptureTimeEditInfo, CaptureTimeExample,
    CaptureTimePlan, CaptureTimeUndo,
};

/// 見込み・結果に入れる例の数（**仮置き**: 5）。
pub const CAPTURE_TIME_EXAMPLES: usize = 5;

/// タイムゾーンとして指定できるオフセットの範囲（分。genzo-model の `parse_offset` と同じ ±18 時間）。
pub const CAPTURE_ZONE_RANGE_MINUTES: std::ops::RangeInclusive<i32> = -18 * 60..=18 * 60;

/// 修正の中身（対象の asset・当てる修正・履歴の種類・ずらす秒数）。
struct Resolved {
    assets: Vec<AssetId>,
    adjustment: CaptureAdjustment,
    kind: CaptureEditKind,
    shift_seconds: Option<i64>,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::InvalidArgument(message.into())
}

/// 分のオフセットを FixedOffset にする。
fn zone_offset(minutes: i32) -> Result<FixedOffset, ApiError> {
    if !CAPTURE_ZONE_RANGE_MINUTES.contains(&minutes) {
        return Err(invalid(format!(
            "タイムゾーンのオフセットは -18:00〜+18:00 で指定してください（{minutes} 分）"
        )));
    }
    FixedOffset::east_opt(minutes * 60).ok_or_else(|| invalid("タイムゾーンのオフセットが不正です"))
}

/// 対象と修正を、カタログの値で解く（基準の写真の差を求める）。
fn resolve(
    c: &Catalog,
    target: &CaptureTarget,
    edit: &CaptureTimeEdit,
) -> Result<Resolved, ApiError> {
    let assets = match target {
        CaptureTarget::Variants { variant_ids } => c.assets_of_variants(variant_ids)?,
        CaptureTarget::Devices { devices } => {
            let keys: Vec<_> = devices.iter().map(|d| d.to_catalog()).collect();
            c.assets_of_devices(&keys)?
        }
    };
    let (adjustment, kind, shift_seconds) = match edit {
        CaptureTimeEdit::Shift { seconds } => (
            CaptureAdjustment::Shift { seconds: *seconds },
            CaptureEditKind::Shift,
            Some(*seconds),
        ),
        CaptureTimeEdit::Sync {
            reference,
            correct,
            offset_minutes,
        } => {
            let reference_asset = c.asset_of_variant(*reference)?;
            let now = c.capture_time(reference_asset)?;
            let Some(now_utc) = now.utc else {
                return Err(invalid(
                    "基準の写真に撮影日時がありません（撮影日時の分かる写真を選んでください）",
                ));
            };
            let parsed = parse_datetime(correct.trim())
                .map_err(|_| invalid(format!("正しい撮影日時を読めません: {correct:?}")))?;
            let Some((local, embedded)) = parsed else {
                return Err(invalid("正しい撮影日時を入れてください"));
            };
            let offset = match (embedded, offset_minutes) {
                (Some(o), _) => o,
                (None, Some(m)) => zone_offset(*m)?,
                (None, None) => now
                    .tz_assumed
                    .as_deref()
                    .and_then(|s| genzo_model::capture_time::parse_offset(s).ok())
                    .ok_or_else(|| invalid("基準の写真のタイムゾーンが分かりません"))?,
            };
            let correct_utc = local
                .checked_sub_signed(TimeDelta::seconds(i64::from(offset.local_minus_utc())))
                .ok_or_else(|| invalid("正しい撮影日時が表せる範囲を超えています"))?
                .and_utc();
            // 秒の単位でそろえる（基準の写真の秒の端数は残す。表示は秒まで）。
            let delta = correct_utc.timestamp() - now_utc.timestamp();
            (
                CaptureAdjustment::Shift { seconds: delta },
                CaptureEditKind::Sync,
                Some(delta),
            )
        }
        CaptureTimeEdit::SetZone {
            offset_minutes,
            mode,
        } => (
            CaptureAdjustment::SetZone {
                offset: zone_offset(*offset_minutes)?,
                mode: *mode,
            },
            CaptureEditKind::SetZone,
            None,
        ),
        CaptureTimeEdit::ClearZone => (
            CaptureAdjustment::ClearZone,
            CaptureEditKind::ClearZone,
            None,
        ),
        CaptureTimeEdit::Reset => (CaptureAdjustment::Reset, CaptureEditKind::Reset, None),
    };
    Ok(Resolved {
        assets,
        adjustment,
        kind,
        shift_seconds,
    })
}

/// カタログの見込みを API の形にする（例のファイル名を引く）。
fn to_plan(
    c: &Catalog,
    plan: CapturePlan,
    shift_seconds: Option<i64>,
    edit_id: Option<i64>,
) -> Result<CaptureTimePlan, ApiError> {
    let mut examples = Vec::with_capacity(plan.examples.len());
    for e in plan.examples {
        let files = c.files_of_asset(e.asset_id)?;
        let file_name = files
            .iter()
            .find(|f| f.role == FileRole::Primary)
            .or(files.first())
            .map(|f| f.name.clone())
            .unwrap_or_default();
        examples.push(CaptureTimeExample {
            asset_id: e.asset_id,
            file_name,
            before: e.before,
            after: e.after,
        });
    }
    Ok(CaptureTimePlan {
        assets: plan.assets,
        changed: plan.changed,
        unchanged: plan.unchanged,
        without_time: plan.without_time,
        sources: plan.sources.into(),
        shift_seconds,
        examples,
        edit_id,
    })
}

/// 撮影日時を変えた後: 知らせて、表示中の検索結果を作り直す。
fn after_change(inner: &Inner) {
    inner.events.emit(Event::CatalogChanged {
        change: CatalogChange::CaptureTime,
        variant_ids: Vec::new(),
        all: true,
    });
    crate::search::refresh(inner, &[]);
}

impl Core {
    /// 機材の一覧（LIB-16。カメラの表示名・シリアル番号・種類の順。分からない機材は最後）。撮影日時の範囲は
    /// 既定のタイムゾーンの時刻で返す。
    pub fn capture_devices(&self) -> Result<Vec<CaptureDevice>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let offset = inner.default_offset();
        let local = |t: Option<DateTime<chrono::Utc>>| t.map(|t| t.with_timezone(&offset));
        Ok(inner
            .with_catalog(|c| c.capture_devices())?
            .into_iter()
            .map(|d| CaptureDevice {
                key: d.key.into(),
                assets: d.assets,
                with_time: d.with_time,
                adjusted: d.adjusted,
                first: local(d.first),
                last: local(d.last),
            })
            .collect())
    }

    /// 写真の機材（LIB-16。[`Core::capture_devices`] の機材の 1 つ）。写真が見つからなければ
    /// [`ApiError::NotFound`]。
    pub fn capture_device_of(
        &self,
        variant_id: genzo_model::VariantId,
    ) -> Result<crate::types::CaptureDeviceKey, ApiError> {
        self.inner.check_open()?;
        Ok(self
            .inner
            .with_catalog(|c| c.device_of_variant(variant_id))?
            .into())
    }

    /// 撮影日時の修正の見込み（何も変えない。モジュールの doc）。範囲外になる写真があれば種類 `invalid_input`。
    pub fn plan_capture_time_edit(
        &self,
        target: &CaptureTarget,
        edit: &CaptureTimeEdit,
    ) -> Result<CaptureTimePlan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let offset = inner.default_offset();
        inner.with_catalog_api(|c| {
            let r = resolve(c, target, edit)?;
            let plan =
                c.plan_capture_adjustment(&r.assets, &r.adjustment, offset, CAPTURE_TIME_EXAMPLES)?;
            to_plan(c, plan, r.shift_seconds, None)
        })
    }

    /// 撮影日時を修正する（1 つのトランザクション。履歴に残す。モジュールの doc）。変わったものがあれば
    /// 知らせて検索結果を作り直す。結果の `edit_id` は記録した修正（変わったものがなければ `null`）。
    pub fn edit_capture_times(
        &self,
        target: &CaptureTarget,
        edit: &CaptureTimeEdit,
    ) -> Result<CaptureTimePlan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let offset = inner.default_offset();
        let plan = inner.with_catalog_api(|c| {
            let r = resolve(c, target, edit)?;
            let out = c.adjust_capture_times(
                &r.assets,
                &r.adjustment,
                r.kind,
                offset,
                CAPTURE_TIME_EXAMPLES,
            )?;
            to_plan(c, out.plan, r.shift_seconds, out.edit_id)
        })?;
        if plan.changed > 0 {
            tracing::info!(
                changed = plan.changed,
                edit_id = ?plan.edit_id,
                "撮影日時を修正しました"
            );
            after_change(inner);
        }
        Ok(plan)
    }

    /// 撮影日時の修正の履歴（新しい順。最大 genzo-catalog の `MAX_CAPTURE_TIME_EDITS` 件）。
    pub fn capture_time_edits(&self) -> Result<Vec<CaptureTimeEditInfo>, ApiError> {
        self.inner.check_open()?;
        Ok(self
            .inner
            .with_catalog(|c| c.capture_time_edits())?
            .into_iter()
            .map(CaptureTimeEditInfo::from)
            .collect())
    }

    /// 最後の撮影日時の修正を取り消す（履歴がなければ `None`）。今の撮影日時に、修正の前の状態（補正値・
    /// タイムゾーンの指定）を当て直す（修正の後に既定のタイムゾーンを変えた場合も、その結果を保つ）。
    pub fn undo_capture_time_edit(&self) -> Result<Option<CaptureTimeUndo>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let offset = inner.default_offset();
        let undone = inner.with_catalog(|c| c.undo_capture_time_edit(offset))?;
        let Some(u) = undone else {
            return Ok(None);
        };
        if u.restored > 0 {
            after_change(inner);
        }
        Ok(Some(CaptureTimeUndo {
            edit: u.edit.into(),
            restored: u.restored,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_offsets_are_checked() {
        assert_eq!(zone_offset(540).unwrap().local_minus_utc(), 9 * 3600);
        assert_eq!(
            zone_offset(-330).unwrap().local_minus_utc(),
            -(5 * 3600 + 1800)
        );
        assert!(zone_offset(18 * 60 + 1).is_err());
        assert!(zone_offset(-18 * 60 - 1).is_err());
    }
}
