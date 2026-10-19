//! 位置情報の後付け（LIB-21。ジオタグ付け。docs/04_architecture.md の 3.16 節）。
//!
//! - 対象は選んだ写真（variant）。同じ写真の仮想コピーと RAW と JPEG の組は位置を共有する（asset に付く。
//!   genzo-catalog の `location`）。**動画は除く**（[`LocationEditPlan::skipped_videos`] に数える。すべて動画なら
//!   何も変えずに数だけ返す）。動画は書き出しで元のファイルをそのままコピーするので、付けた位置を書き出しに
//!   入れる手段がなく、取り込みでも動画の位置を読まないため（決定事項ログ No.171 の現像プリセットと同じ扱い）。
//! - 付けた位置はカタログの中だけに持ち、元のファイルには書かない。カメラ（ファイル）の位置も変えない。使う位置
//!   （付けた位置があればそれ、なければカメラの位置）を詳細（[`crate::VariantDetails::gps`]・
//!   [`crate::VariantDetails::gps_source`]・[`crate::VariantDetails::camera_gps`]）と書き出しの Exif
//!   （`export`）に使う。
//! - [`Core::plan_location_edit`] は何も変えずに見込みを返す（画面の確認）。[`Core::edit_locations`] は 1 つの
//!   トランザクションで変え、履歴に残す（[`Core::undo_location_edit`] で最後のものから取り消す。やり直しはない）。
//! - 座標は範囲内の有限の値だけを受け付ける（UI の JSON の `{"lat": 1000, ...}` などは、何も変えずに
//!   [`ApiError::InvalidArgument`]）。座標の文字列の解釈は genzo-model の `parse_coordinates`（CLI・画面）。
//! - 変えたら [`Event::CatalogChanged`]（[`CatalogChange::Location`]）で知らせる。付ける・消すでは対象の写真の
//!   すべての variant（仮想コピーを含む）、取り消しでは戻した写真が分からないので、すべてとして知らせる。
//!   位置は検索結果の並び・絞り込み・グリッドの要約（[`crate::VariantSummary`]）に使わないので、検索結果は
//!   作り直さない。
//! - ログには座標を書かない（数だけ。決定事項ログ No.75 の方針）。

use std::collections::HashSet;

use genzo_catalog::{Catalog, LocationPlan};
use genzo_model::{AssetId, AssetKind, GpsCoord, VariantId};

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::{CatalogChange, Event};
use crate::types::{LocationEditInfo, LocationEditPlan, LocationUndo};

/// 変更の対象（動画・見つからないものを除いた写真の asset と、除いた数）。
struct Targets {
    /// 写真の asset（重ならないもの。最初に出た順）。
    photos: Vec<AssetId>,
    /// 除いた動画の asset の数。
    skipped_videos: u64,
    /// 見つからなかった variant の数（重ならないもの）。
    missing: u64,
}

/// 付ける位置が範囲内の有限の値か（消す `None` は常によい）。
fn check_location(location: Option<GpsCoord>) -> Result<(), ApiError> {
    match location {
        Some(g) if !g.is_valid() => Err(ApiError::InvalidArgument(
            "座標が範囲外です（緯度は -90〜90、経度は -180〜180 の値を指定してください）"
                .to_owned(),
        )),
        _ => Ok(()),
    }
}

/// 選んだ写真を asset にまとめ、動画を除く。
fn targets(c: &Catalog, variant_ids: &[VariantId]) -> Result<Targets, ApiError> {
    let mut seen = HashSet::with_capacity(variant_ids.len());
    let ids: Vec<VariantId> = variant_ids
        .iter()
        .copied()
        .filter(|v| seen.insert(*v))
        .collect();
    let summaries = c.variant_summaries(&ids)?;
    let missing = ids.len().saturating_sub(summaries.len()) as u64;
    let mut photos = Vec::new();
    let mut photo_set = HashSet::new();
    let mut videos = HashSet::new();
    for s in summaries {
        if s.kind == AssetKind::Video {
            videos.insert(s.asset_id);
        } else if photo_set.insert(s.asset_id) {
            photos.push(s.asset_id);
        }
    }
    Ok(Targets {
        photos,
        skipped_videos: videos.len() as u64,
        missing,
    })
}

/// カタログの見込みを API の形にする。
fn to_plan(t: &Targets, plan: LocationPlan, edit_id: Option<i64>) -> LocationEditPlan {
    LocationEditPlan {
        photos: plan.assets,
        changed: plan.changed,
        unchanged: plan.unchanged,
        with_camera_gps: plan.with_camera_gps,
        with_assigned: plan.with_assigned,
        skipped_videos: t.skipped_videos,
        // カタログの missing は、対象を読んだ後に消えた asset（同じロックの中なので通常は 0）。
        missing: t.missing + plan.missing,
        edit_id,
    }
}

/// 写真のすべての variant（仮想コピーを含む。asset の順、asset の中はマスターが先）。
fn variants_of(c: &Catalog, assets: &[AssetId]) -> Result<Vec<VariantId>, ApiError> {
    let mut out = Vec::with_capacity(assets.len());
    for &a in assets {
        out.extend(c.variants_of_asset(a)?.into_iter().map(|v| v.id));
    }
    Ok(out)
}

/// 位置を変えた後: 知らせる（検索結果は作り直さない。モジュールの doc）。
fn after_change(inner: &Inner, variant_ids: Vec<VariantId>, all: bool) {
    inner.events.emit(Event::CatalogChanged {
        change: CatalogChange::Location,
        variant_ids,
        all,
    });
}

impl Core {
    /// 位置の変更の見込み（何も変えない。モジュールの doc）。`location` は付ける位置（`None` は付けた位置を
    /// 消して、カメラの位置に戻す）。範囲外の座標なら [`ApiError::InvalidArgument`]。
    pub fn plan_location_edit(
        &self,
        variant_ids: &[VariantId],
        location: Option<GpsCoord>,
    ) -> Result<LocationEditPlan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        check_location(location)?;
        inner.with_catalog_api(|c| {
            let t = targets(c, variant_ids)?;
            let plan = c.plan_location_edit(&t.photos, location)?;
            Ok(to_plan(&t, plan, None))
        })
    }

    /// 選んだ写真に位置を付ける・付けた位置を消す（1 つのトランザクション。履歴に残す。モジュールの doc）。
    /// `location` は付ける位置（`None` は消して、カメラの位置に戻す）。動画は除く。変わったものがあれば
    /// 知らせる。結果の `edit_id` は記録した変更（変わったものがなければ `null`）。範囲外の座標なら、何も
    /// 変えずに [`ApiError::InvalidArgument`]。
    pub fn edit_locations(
        &self,
        variant_ids: &[VariantId],
        location: Option<GpsCoord>,
    ) -> Result<LocationEditPlan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        check_location(location)?;
        let (plan, changed_variants) = inner.with_catalog_api(|c| {
            let t = targets(c, variant_ids)?;
            let out = c.edit_locations(&t.photos, location)?;
            let plan = to_plan(&t, out.plan, out.edit_id);
            let variants = if plan.changed > 0 {
                variants_of(c, &t.photos)?
            } else {
                Vec::new()
            };
            Ok((plan, variants))
        })?;
        if plan.changed > 0 {
            tracing::info!(
                changed = plan.changed,
                skipped_videos = plan.skipped_videos,
                clear = location.is_none(),
                edit_id = ?plan.edit_id,
                "写真の位置を変えました"
            );
            after_change(inner, changed_variants, false);
        }
        Ok(plan)
    }

    /// 位置の変更の履歴（新しい順。最大 genzo-catalog の `MAX_LOCATION_EDITS` 件）。
    pub fn location_edits(&self) -> Result<Vec<LocationEditInfo>, ApiError> {
        self.inner.check_open()?;
        Ok(self
            .inner
            .with_catalog(|c| c.location_edits())?
            .into_iter()
            .map(LocationEditInfo::from)
            .collect())
    }

    /// 最後の位置の変更を取り消す（履歴がなければ `None`）。変える前の付けた位置（付けた日時も）をそのまま
    /// 書き戻す（付けていなかった写真は、付けた位置を消す）。戻したものがあれば知らせる。
    pub fn undo_location_edit(&self) -> Result<Option<LocationUndo>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let Some(u) = inner.with_catalog(|c| c.undo_location_edit())? else {
            return Ok(None);
        };
        if u.restored > 0 {
            tracing::info!(
                restored = u.restored,
                removed = u.removed,
                edit_id = u.edit.id,
                "写真の位置の変更を取り消しました"
            );
            after_change(inner, Vec::new(), true);
        }
        Ok(Some(LocationUndo {
            edit: u.edit.into(),
            restored: u.restored,
            removed: u.removed,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_out_of_range_are_rejected() {
        assert!(check_location(None).is_ok());
        assert!(check_location(GpsCoord::new(90.0, -180.0)).is_ok());
        for (lat, lon) in [
            (90.000_001, 0.0),
            (-90.000_001, 0.0),
            (0.0, 180.000_001),
            (0.0, -180.000_001),
            (1000.0, 0.0),
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
        ] {
            let err = check_location(Some(GpsCoord { lat, lon })).unwrap_err();
            assert!(matches!(err, ApiError::InvalidArgument(_)), "{lat}, {lon}");
        }
    }
}
