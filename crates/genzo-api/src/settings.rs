//! 設定（SYS-05）: キャッシュの場所と上限、書き出しの既定の色空間、既定のタイムゾーン。
//!
//! カタログの設定テーブル（`setting`）に保存する。カタログに値がなければ [`CoreConfig`] の初期値、
//! それもなければ組み込みの既定値を使う。

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
fn spawn_reresolve(inner: &Arc<Inner>) -> u64 {
    spawn_job(
        inner,
        JobKind::ReresolveCaptureTimes,
        "撮影日時の推定し直し",
        |ctx| {
            let inner = ctx.inner;
            let offset = inner.default_offset();
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
    )
}
