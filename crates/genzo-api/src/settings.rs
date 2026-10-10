//! 設定（SYS-05）: キャッシュの場所と上限、書き出しの既定の色空間、既定のタイムゾーン。
//!
//! カタログの設定テーブル（`setting`）に保存する。カタログに値がなければ [`CoreConfig`] の初期値、
//! それもなければ組み込みの既定値を使う。既定のタイムゾーンは、カタログに値がなければ、開いたときに
//! 決めた値（初期値、なければその時点の OS のオフセット）を保存し、以後はその値を使う
//! （[`persist_default_offset`]）。

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Local, Offset};
use genzo_catalog::Catalog;
use genzo_model::OutputColorSpace;

use crate::config::CoreConfig;
use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::{CatalogChange, Event, EventHub, WarningCode};
use crate::jobs::spawn_job;
use crate::types::{CoreSettings, JobKind, JobResult, SettingsUpdate};

/// 設定テーブルのキー: L1 プレビューのキャッシュの場所。
pub const KEY_PREVIEW_CACHE_DIR: &str = "api.preview_cache_dir";
/// 設定テーブルのキー: L1 プレビューのキャッシュの上限（バイト）。
pub const KEY_PREVIEW_CACHE_BYTES: &str = "api.preview_cache_bytes";
/// 設定テーブルのキー: 書き出しの既定の色空間（JSON の文字列。`"srgb"` など）。
pub const KEY_DEFAULT_EXPORT_COLOR_SPACE: &str = "api.default_export_color_space";
/// 設定テーブルのキー: 既定のタイムゾーンのオフセット（分）。
pub const KEY_DEFAULT_UTC_OFFSET_MINUTES: &str = "api.default_utc_offset_minutes";

/// 既定のタイムゾーンのオフセットとして受け付ける範囲（分。UTC−14:00〜UTC+14:00。現存する
/// タイムゾーンの範囲）。
pub const UTC_OFFSET_RANGE_MINUTES: std::ops::RangeInclusive<i32> = -14 * 60..=14 * 60;

/// L1 プレビューのキャッシュの上限として受け付ける最小値（**仮置き**: 100MB。これより小さいと、
/// 長辺 2560px のプレビューが数十枚しか入らない）。
pub const MIN_PREVIEW_CACHE_BYTES: u64 = 100_000_000;

fn os_offset_minutes() -> i32 {
    Local::now().offset().fix().local_minus_utc() / 60
}

/// カタログから設定を読む（壊れた値は既定値にして警告する）。
pub(crate) fn load(
    catalog: &Catalog,
    config: &CoreConfig,
    events: &EventHub,
) -> Result<CoreSettings, ApiError> {
    let bad = |key: &str, value: &str| {
        events.emit_sticky(Event::Warning {
            code: WarningCode::InvalidSetting,
            message: format!("設定 {key} の値 {value:?} を読めないため、既定値を使います"),
            variant_id: None,
            path: None,
        });
    };
    let preview_cache_dir = catalog
        .setting(KEY_PREVIEW_CACHE_DIR)?
        .map(PathBuf::from)
        .unwrap_or_else(|| config.default_preview_dir());
    let preview_cache_bytes = match catalog.setting(KEY_PREVIEW_CACHE_BYTES)? {
        Some(v) => v.parse::<u64>().unwrap_or_else(|_| {
            bad(KEY_PREVIEW_CACHE_BYTES, &v);
            default_preview_bytes(config)
        }),
        None => default_preview_bytes(config),
    };
    let default_export_color_space = match catalog.setting(KEY_DEFAULT_EXPORT_COLOR_SPACE)? {
        Some(v) => serde_json::from_str::<OutputColorSpace>(&v).unwrap_or_else(|_| {
            bad(KEY_DEFAULT_EXPORT_COLOR_SPACE, &v);
            OutputColorSpace::default()
        }),
        None => OutputColorSpace::default(),
    };
    let fallback_offset = config
        .default_utc_offset_minutes
        .filter(|m| UTC_OFFSET_RANGE_MINUTES.contains(m))
        .unwrap_or_else(os_offset_minutes);
    let default_utc_offset_minutes = match catalog.setting(KEY_DEFAULT_UTC_OFFSET_MINUTES)? {
        Some(v) => match v.parse::<i32>() {
            Ok(m) if UTC_OFFSET_RANGE_MINUTES.contains(&m) => m,
            _ => {
                bad(KEY_DEFAULT_UTC_OFFSET_MINUTES, &v);
                fallback_offset
            }
        },
        None => fallback_offset,
    };
    Ok(CoreSettings {
        preview_cache_dir,
        preview_cache_bytes,
        default_export_color_space,
        default_utc_offset_minutes,
    })
}

/// 既定のタイムゾーンがカタログに保存されていなければ、決めた値（[`CoreConfig`] の初期値、なければ OS の
/// オフセット）を保存する。以後はその値だけを使う（開くたびに OS のオフセットが変わっても、オフセットの
/// ない写真の推定が取り込んだ時期ごとに混ざらないように）。新しく保存したら `true`。
///
/// 保存した値が今の OS のオフセットと違えば、警告を出す（自動では変えない）。初期値を [`CoreConfig`] で
/// 指定しているとき（OS のオフセットに頼っていないとき）は警告しない。
pub(crate) fn persist_default_offset(
    catalog: &mut Catalog,
    settings: &CoreSettings,
    config: &CoreConfig,
    events: &EventHub,
) -> Result<bool, ApiError> {
    let stored = catalog.setting(KEY_DEFAULT_UTC_OFFSET_MINUTES)?.is_some();
    if !stored {
        catalog.set_setting(
            KEY_DEFAULT_UTC_OFFSET_MINUTES,
            &settings.default_utc_offset_minutes.to_string(),
        )?;
    }
    if config.default_utc_offset_minutes.is_none()
        && let Some(message) =
            offset_mismatch_message(settings.default_utc_offset_minutes, os_offset_minutes())
    {
        events.emit_sticky(Event::Warning {
            code: WarningCode::DefaultTimeZoneDiffers,
            message,
            variant_id: None,
            path: None,
        });
    }
    Ok(!stored)
}

/// 既定のタイムゾーンと OS のオフセットが違うときの警告の文（同じなら `None`）。
fn offset_mismatch_message(saved_minutes: i32, os_minutes: i32) -> Option<String> {
    (saved_minutes != os_minutes).then(|| {
        format!(
            "既定のタイムゾーン（{}）が、今の OS のオフセット（{}）と違います。オフセットのない写真の撮影日時は既定のタイムゾーンで推定します（設定で変えられます）",
            offset_label(saved_minutes),
            offset_label(os_minutes)
        )
    })
}

/// オフセットの表示（`UTC+09:00` など）。
fn offset_label(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let m = minutes.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
}

fn default_preview_bytes(config: &CoreConfig) -> u64 {
    config
        .preview_cache_bytes
        .unwrap_or(genzo_catalog::DEFAULT_PREVIEW_CAPACITY_BYTES)
}

impl Core {
    /// 現在の設定（SYS-05）。
    pub fn settings(&self) -> CoreSettings {
        self.inner.settings.lock().clone()
    }

    /// 設定を変えてカタログに保存する。
    ///
    /// - キャッシュの上限: すぐに有効（超えていれば古いものから削除する。SCL-04）。
    /// - キャッシュの場所: 次に開いたときに有効（今のキャッシュは移さない）。
    /// - 既定のタイムゾーン: 既定のオフセットで推定していた撮影日時を、バックグラウンドのジョブで推定し
    ///   直す（[`JobKind::ReresolveCaptureTimes`]）。
    pub fn update_settings(&self, update: &SettingsUpdate) -> Result<CoreSettings, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if let Some(bytes) = update.preview_cache_bytes
            && bytes < MIN_PREVIEW_CACHE_BYTES
        {
            return Err(ApiError::InvalidArgument(format!(
                "プレビューのキャッシュの上限は {MIN_PREVIEW_CACHE_BYTES} バイト以上にしてください（{bytes}）"
            )));
        }
        if let Some(m) = update.default_utc_offset_minutes
            && !UTC_OFFSET_RANGE_MINUTES.contains(&m)
        {
            return Err(ApiError::InvalidArgument(format!(
                "タイムゾーンのオフセットは −14:00〜+14:00 の範囲で指定してください（{m} 分）"
            )));
        }
        if let Some(dir) = &update.preview_cache_dir
            && !dir.is_absolute()
        {
            return Err(ApiError::InvalidArgument(format!(
                "キャッシュの場所は絶対パスで指定してください（{}）",
                dir.display()
            )));
        }
        let previous = inner.settings.lock().clone();
        inner.with_catalog_api(|c| {
            if let Some(dir) = &update.preview_cache_dir {
                let s = dir.to_str().ok_or_else(|| {
                    ApiError::InvalidArgument("UTF-8 で表せないパスは使えません".to_owned())
                })?;
                c.set_setting(KEY_PREVIEW_CACHE_DIR, s)?;
            }
            if let Some(bytes) = update.preview_cache_bytes {
                c.set_setting(KEY_PREVIEW_CACHE_BYTES, &bytes.to_string())?;
            }
            if let Some(space) = update.default_export_color_space {
                let json =
                    serde_json::to_string(&space).map_err(|e| ApiError::Internal(e.to_string()))?;
                c.set_setting(KEY_DEFAULT_EXPORT_COLOR_SPACE, &json)?;
            }
            if let Some(m) = update.default_utc_offset_minutes {
                c.set_setting(KEY_DEFAULT_UTC_OFFSET_MINUTES, &m.to_string())?;
            }
            Ok(())
        })?;
        let current = {
            let mut s = inner.settings.lock();
            if let Some(dir) = &update.preview_cache_dir {
                s.preview_cache_dir = dir.clone();
            }
            if let Some(bytes) = update.preview_cache_bytes {
                s.preview_cache_bytes = bytes;
            }
            if let Some(space) = update.default_export_color_space {
                s.default_export_color_space = space;
            }
            if let Some(m) = update.default_utc_offset_minutes {
                s.default_utc_offset_minutes = m;
            }
            s.clone()
        };
        if let Some(bytes) = update.preview_cache_bytes {
            inner.with_cache(|c| c.previews.set_capacity_bytes(bytes))?;
        }
        if current.default_utc_offset_minutes != previous.default_utc_offset_minutes {
            spawn_reresolve(inner);
        }
        Ok(current)
    }
}

/// 既定のタイムゾーンを変えたときに、既定のオフセットで推定していた撮影日時を推定し直すジョブ。
///
/// 続けて変えたときに、古いオフセットのジョブが後から書いて上書きしないよう、(1) オフセットはジョブの
/// 開始時ではなく、チャンクごとにカタログのロックの中で今の設定から読む（後から同じチャンクを書くジョブは
/// 必ず最新の設定を読むので、どの順で走っても最後に残る値は今の設定になる）、(2) 前のジョブは取り消す
/// （無駄な処理を省くため）。
pub(crate) fn spawn_reresolve(inner: &Arc<Inner>) -> u64 {
    let mut last = inner.reresolve_job.lock();
    if let Some(previous) = last.take() {
        inner.jobs.cancel(previous);
        inner.jobs.reap_unrun(inner);
    }
    let id = spawn_job(
        inner,
        JobKind::ReresolveCaptureTimes,
        "撮影日時の推定し直し",
        |ctx| {
            let inner = ctx.inner;
            let assets = inner.with_catalog(|c| {
                let variants = c.all_variant_ids()?;
                c.assets_of_variants(&variants)
            })?;
            let total = assets.len() as u64;
            let mut updated = 0u64;
            for (i, chunk) in assets.chunks(256).enumerate() {
                if ctx.is_cancelled() {
                    return Err(ApiError::Cancelled);
                }
                updated += inner.with_catalog_api(|c| {
                    // 今の設定（このチャンクを書く直前）のオフセット。
                    let offset = inner.default_offset();
                    let mut n = 0;
                    for &a in chunk {
                        let current = c.capture_time(a)?;
                        let next = current
                            .with_default_offset(offset)
                            .map_err(|e| ApiError::Internal(e.to_string()))?;
                        if next != current {
                            c.set_capture_time(a, &next)?;
                            n += 1;
                        }
                    }
                    Ok(n)
                })?;
                ctx.progress(((i + 1) * 256).min(assets.len()) as u64, total);
            }
            if updated > 0 {
                inner.events.emit(Event::CatalogChanged {
                    change: CatalogChange::CaptureTime,
                    variant_ids: Vec::new(),
                    all: true,
                });
                crate::search::refresh(inner, &[]);
            }
            Ok(JobResult::ReresolveCaptureTimes { updated })
        },
    );
    *last = Some(id);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_mismatch_is_reported_with_both_offsets() {
        assert_eq!(offset_mismatch_message(540, 540), None);
        let m = offset_mismatch_message(540, 60).unwrap();
        assert!(m.contains("UTC+09:00") && m.contains("UTC+01:00"), "{m}");
        assert_eq!(offset_label(-210), "UTC-03:30");
        assert_eq!(offset_label(0), "UTC+00:00");
    }
}
