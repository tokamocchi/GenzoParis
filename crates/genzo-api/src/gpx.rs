//! GPX のログからの位置付け（LIB-21 の拡張。implementation_status の「v1 の残り」No.26）。
//!
//! GPS ロガー・スマートフォンのアプリが記録した GPX のログと、写真の撮影日時（補正後の UTC）を照らし合わせて、
//! 選んだ写真にまとめて位置を付ける（[`Core::plan_gpx_locations`] は見込み、[`Core::apply_gpx_locations`] は実行）。
//!
//! - ログの読み取りと、時刻から位置を求める計算は genzo-model の `gpx`（時刻をはさむ 2 点の間隔が
//!   [`GpxOptions::max_gap_seconds`] 以内なら補間し、そうでなければ近いほうの点がその秒数以内にあるときだけ使う）。
//! - **写真の時計のずれ**: [`GpxOptions::offset_seconds`] を撮影日時に足してからログと照らし合わせる（カメラの時計が
//!   2 分進んでいたなら −120）。カタログの撮影日時は変えない（変えるなら、撮影日時の一括修正 LIB-16 を使う）。
//!   タイムゾーンは、カタログの撮影日時（UTC。取り込みのときに決めたオフセットで求めた値）に従う。
//! - **すでに位置のある写真は変えない**（カメラの位置・後から付けた位置のどちらか。[`GpxOptions::overwrite`] が
//!   真なら、位置のある写真にも付ける。カメラの位置は消えず、付けた位置を優先して使う）。
//! - 動画は除く（位置情報の後付けと同じ。`location` の doc）。撮影日時のない写真・ログに近い点のない写真は付けずに
//!   数える。
//! - 付けた位置は、位置情報の後付けと同じくカタログの中だけに持つ（元のファイルには書かない）。**1 回の変更として
//!   履歴に残し**、[`Core::undo_location_edit`] でまとめて取り消せる（履歴の [`crate::LocationEditInfo::from_track`]）。
//! - ファイルはこのプロセスで読む（ワーカーには渡さない）: 読むのは文字列の走査だけで、外部の実体・DTD は
//!   展開せず、大きさと点の数に上限がある（genzo-model の `MAX_GPX_BYTES`・`MAX_GPX_POINTS`）。
//! - ログには座標・ファイルの場所を書かない（数だけ。決定事項ログ No.75 の方針）。

use std::collections::HashSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use genzo_catalog::Catalog;
use genzo_model::{AssetId, AssetKind, GpsCoord, GpxTrack, MAX_GPX_BYTES, VariantId};

use crate::core::Core;
use crate::error::ApiError;
use crate::events::{CatalogChange, Event};
use crate::types::{GpxLocationPlan, GpxOptions};

/// 写真の時計のずれに指定できる範囲（秒。±14 日。**仮置き**）。
pub const MAX_GPX_OFFSET_SECONDS: i64 = 14 * 86_400;

/// 選んだファイルを読めない（見つからない・権限がない）。
fn file_error(path: &Path, e: &std::io::Error) -> ApiError {
    ApiError::FileAccess {
        path: path.to_path_buf(),
        message: e.to_string(),
    }
}

/// GPX のファイルを読む。
fn read_track(path: &Path) -> Result<GpxTrack, ApiError> {
    let meta = std::fs::metadata(path).map_err(|e| file_error(path, &e))?;
    if !meta.is_file() {
        return Err(ApiError::InvalidArgument(
            "GPX のファイルを指定してください（フォルダは指定できません）".to_owned(),
        ));
    }
    if meta.len() > MAX_GPX_BYTES {
        return Err(ApiError::InvalidArgument(
            genzo_model::GpxError::TooLarge {
                actual: meta.len(),
                limit: MAX_GPX_BYTES,
            }
            .to_string(),
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| file_error(path, &e))?;
    // UTF-8 でない部分（名前など）は置き換える。読む値（数と時刻）は ASCII。BOM は除く。
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    GpxTrack::parse(text).map_err(|e| ApiError::InvalidArgument(e.to_string()))
}

fn check_options(o: &GpxOptions) -> Result<(), ApiError> {
    if o.max_gap_seconds == 0 || o.max_gap_seconds > genzo_model::MAX_GPX_MAX_GAP_SECONDS {
        return Err(ApiError::InvalidArgument(format!(
            "記録の途切れとみなす間隔は 1〜{} 秒で指定してください",
            genzo_model::MAX_GPX_MAX_GAP_SECONDS
        )));
    }
    if o.offset_seconds.abs() > MAX_GPX_OFFSET_SECONDS {
        return Err(ApiError::InvalidArgument(format!(
            "写真の時計のずれは ±{MAX_GPX_OFFSET_SECONDS} 秒までで指定してください"
        )));
    }
    Ok(())
}

fn utc_of(ms: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

/// 選んだ写真のうち、位置を付けるもの（asset と位置）と、付けないものの数。
fn match_photos(
    c: &Catalog,
    track: &GpxTrack,
    variant_ids: &[VariantId],
    o: &GpxOptions,
) -> Result<(GpxLocationPlan, Vec<(AssetId, GpsCoord)>), ApiError> {
    let mut plan = GpxLocationPlan {
        track_points: track.points().len() as u64,
        track_skipped: track.skipped,
        track_start: utc_of(track.start_ms()),
        track_end: utc_of(track.end_ms()),
        ..GpxLocationPlan::default()
    };
    let mut seen = HashSet::with_capacity(variant_ids.len());
    let ids: Vec<VariantId> = variant_ids
        .iter()
        .copied()
        .filter(|v| seen.insert(*v))
        .collect();
    let summaries = c.variant_summaries(&ids)?;
    plan.missing = ids.len().saturating_sub(summaries.len()) as u64;
    let mut assets = HashSet::new();
    let mut videos = HashSet::new();
    let mut items = Vec::new();
    for s in summaries {
        if s.kind == AssetKind::Video {
            videos.insert(s.asset_id);
            continue;
        }
        if !assets.insert(s.asset_id) {
            continue;
        }
        plan.photos += 1;
        let asset = c.asset(s.asset_id)?;
        if !o.overwrite && asset.effective_gps().is_some() {
            plan.skipped_located += 1;
            continue;
        }
        let Some(utc) = asset.capture.utc else {
            plan.no_capture_time += 1;
            continue;
        };
        let at = utc.timestamp_millis() + o.offset_seconds * 1000;
        match track.locate(at, o.max_gap_seconds) {
            Some(l) => {
                plan.matched += 1;
                if l.interpolated {
                    plan.interpolated += 1;
                }
                items.push((s.asset_id, l.coord));
            }
            None => plan.out_of_track += 1,
        }
    }
    plan.skipped_videos = videos.len() as u64;
    Ok((plan, items))
}

impl Core {
    /// GPX のログから位置を付ける見込み（何も変えない。モジュールの doc）。
    ///
    /// エラー: ファイルを読めなければ [`ApiError::FileAccess`]、GPX として読めない・時刻のある点がない・
    /// 設定が範囲外なら [`ApiError::InvalidArgument`]。
    pub fn plan_gpx_locations(
        &self,
        variant_ids: &[VariantId],
        gpx_path: &Path,
        options: &GpxOptions,
    ) -> Result<GpxLocationPlan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        check_options(options)?;
        let track = read_track(gpx_path)?;
        inner.with_catalog_api(|c| {
            let (mut plan, items) = match_photos(c, &track, variant_ids, options)?;
            let p = c.plan_location_edit_each(&items)?;
            plan.changed = p.changed;
            plan.unchanged = p.unchanged;
            Ok(plan)
        })
    }

    /// GPX のログから、選んだ写真に位置を付ける（1 つのトランザクション。1 回の変更として履歴に残す。モジュールの
    /// doc）。変わったものがあれば知らせる。結果の `edit_id` は記録した変更（変わったものがなければ `null`）。
    pub fn apply_gpx_locations(
        &self,
        variant_ids: &[VariantId],
        gpx_path: &Path,
        options: &GpxOptions,
    ) -> Result<GpxLocationPlan, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        check_options(options)?;
        let track = read_track(gpx_path)?;
        let (plan, changed_variants) = inner.with_catalog_api(|c| {
            let (mut plan, items) = match_photos(c, &track, variant_ids, options)?;
            let out = c.edit_locations_each(&items)?;
            plan.changed = out.plan.changed;
            plan.unchanged = out.plan.unchanged;
            plan.edit_id = out.edit_id;
            let mut variants = Vec::new();
            if plan.changed > 0 {
                for (a, _) in &items {
                    variants.extend(c.variants_of_asset(*a)?.into_iter().map(|v| v.id));
                }
            }
            Ok((plan, variants))
        })?;
        if plan.changed > 0 {
            tracing::info!(
                changed = plan.changed,
                matched = plan.matched,
                out_of_track = plan.out_of_track,
                track_points = plan.track_points,
                edit_id = ?plan.edit_id,
                "GPX のログから写真に位置を付けました"
            );
            inner.events.emit(Event::CatalogChanged {
                change: CatalogChange::Location,
                variant_ids: changed_variants,
                all: false,
            });
            if crate::search::current_uses_location(inner) {
                crate::search::refresh(inner, &[]);
            }
        }
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_out_of_range_are_rejected() {
        assert!(check_options(&GpxOptions::default()).is_ok());
        for o in [
            GpxOptions {
                max_gap_seconds: 0,
                ..GpxOptions::default()
            },
            GpxOptions {
                max_gap_seconds: genzo_model::MAX_GPX_MAX_GAP_SECONDS + 1,
                ..GpxOptions::default()
            },
            GpxOptions {
                offset_seconds: MAX_GPX_OFFSET_SECONDS + 1,
                ..GpxOptions::default()
            },
            GpxOptions {
                offset_seconds: -MAX_GPX_OFFSET_SECONDS - 1,
                ..GpxOptions::default()
            },
        ] {
            assert!(
                matches!(check_options(&o), Err(ApiError::InvalidArgument(_))),
                "{o:?}"
            );
        }
    }
}
