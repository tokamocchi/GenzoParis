//! ビフォー / アフターの表示のビフォー（DEV-31）。
//!
//! - [`Core::set_before`]: 現像中の写真（[`Core::open_develop`] で開いたもの）のビフォーの設定を決め、
//!   ビフォーのプレビューを描く。出どころ（[`BeforeSource`]）は、variant を作ったときの設定（履歴の最初の
//!   1 件。既定）・履歴の 1 件・スナップショット・今の設定の写し。後のパッケージ（スナップショット・履歴。
//!   DEV-28）は、画面から出どころを選べるようにするだけでよい。
//! - **ビフォーで実際に描く設定**（[`effective_settings`]）は、出どころの設定の切り抜き・角度補正
//!   （`geometry`）を今の設定（アフター）のものに置き換え、外部データ（`render_deps`）を今使っている値に
//!   したもの。切り抜きをそろえるのは、ビフォーとアフターの出力の格子（C）を同じにして、位置を合わせて
//!   比べ、拡大・パンを同期するため（決定事項ログ No.79）。処理バージョンは出どころの設定のまま（版 1 の
//!   写真のビフォーは版 1 で描く）。
//! - 描画は現像のプレビューと同じ対話用のスケジューラ（P0）の別の枠（「最新の 1 件だけ」）に投入する。
//!   アフターの描画を先に投入するので、アフターが先に描ける。**実際に描く設定のハッシュが前の要求と同じなら
//!   描き直さない**（露光量などアフターだけの変更では描き直さず、切り抜き・角度を変えたとき・画面の
//!   プロファイルを変えたとき・出どころを変えたときに描き直す）。ヒストグラムは求めない。
//! - 結果はセッションに持ち（[`Core::before_frame`]）、[`Event::BeforeUpdated`] で知らせる。前の写真・前の
//!   要求の結果では置き換えない（世代で比べる。現像のプレビューと同じ）。
//! - **キャッシュ**: ビフォーの描画はサムネイル（L0）・標準プレビュー（L1）のキャッシュに書かない（それらの
//!   キーは variant の今の設定のハッシュなので、ビフォーの画素を入れると取り違える）。パイプラインの
//!   キャッシュ（A1・ガイド。genzo-pipeline の `Engine`、GPU 版も）はキーに設定のハッシュを含むので、
//!   ビフォーとアフターの結果は混ざらない（ビフォーの分だけ枠を使う）。
//! - 切り抜きの編集中（切り抜く前の全体の表示。[`Core::set_develop_view`]）はビフォーを描かない（画面は
//!   ビフォー / アフターの表示をやめる）。
//! - 等倍表示の範囲は [`Core::render_before_region`]（`region` の doc）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use genzo_jobs::{JobOptions, Priority, SupersedePolicy};
use genzo_model::{DevelopSettings, RenderDeps, RenderQuality};

use crate::core::{Core, Inner};
use crate::develop::{RenderJob, Session, render_frame};
use crate::error::ApiError;
use crate::events::{Event, WarningCode};
use crate::types::{BeforeFrame, BeforeInfo, BeforeSource, FrameInfo};

/// ビフォーの描画の「最新の 1 件だけ」の枠（genzo-jobs の `SlotKey`）。
const BEFORE_SLOT: &str = "develop-before";

/// 履歴がない variant（ふつうはない。variant を作ると履歴の最初の 1 件を記録する）のビフォーの表示名。
pub const BEFORE_LABEL_DEFAULT: &str = "既定の設定";
/// 今の設定の写しのビフォーの表示名。
pub const BEFORE_LABEL_CURRENT: &str = "アフターの設定のコピー";

/// セッションが持つビフォーの状態。
pub(crate) struct BeforeState {
    source: BeforeSource,
    label: String,
    /// 出どころの設定（切り抜きを置き換える前）。
    base: DevelopSettings,
    /// 最後に投入した描画の、実際に描く設定のハッシュ（同じなら描き直さない。`None` なら次に必ず描く）。
    requested: Option<[u8; 32]>,
    /// 最後に投入した描画の世代。
    generation: u64,
    /// 最新の結果。
    frame: Option<Arc<BeforeFrame>>,
}

impl BeforeState {
    /// 出どころの設定。
    pub(crate) fn base(&self) -> &DevelopSettings {
        &self.base
    }

    /// 次の描画の投入で、設定が同じでも描き直す（画面のプロファイルを変えたときなど）。
    pub(crate) fn force_redraw(&mut self) {
        self.requested = None;
    }
}

/// ビフォーで実際に描く設定: 出どころの設定の切り抜き・角度補正を今の設定（アフター）のものにし、外部データを
/// 今使っている値にする（`before` の doc）。
pub(crate) fn effective_settings(
    engine: &genzo_pipeline::Engine,
    base: &DevelopSettings,
    after: &DevelopSettings,
    deps: &RenderDeps,
) -> DevelopSettings {
    let mut s = base.clone();
    s.geometry = after.geometry;
    crate::deps::with_render_deps(engine, &s, deps)
}

/// セッションのビフォーの状態（設定していなければ `None`）。
pub(crate) fn info_of(s: &Session) -> Option<BeforeInfo> {
    s.before.as_ref().map(|b| BeforeInfo {
        variant_id: s.variant_id,
        source: b.source.clone(),
        label: b.label.clone(),
        process_version: b.base.process_version,
        generation: b.generation,
    })
}

/// ビフォーを設定していて、実際に描く設定が前の要求と違えば、ビフォーの描画を投入する（切り抜きの編集中は
/// 描かない）。state のロックの中で呼ぶ。
pub(crate) fn maybe_submit(inner: &Arc<Inner>, s: &mut Session) {
    if s.uncropped {
        return;
    }
    let settings = match s.before.as_ref() {
        Some(b) => effective_settings(&inner.engine, &b.base, &s.settings, &s.render_deps),
        None => return,
    };
    let hash = settings.develop_hash();
    let generation = {
        let b = s.before.as_mut().expect("上で確かめた");
        if b.requested == Some(hash) {
            return;
        }
        b.requested = Some(hash);
        b.generation = inner.develop.next_generation();
        b.generation
    };
    let job = RenderJob {
        variant_id: s.variant_id,
        source: s.source.clone(),
        settings,
        generation,
        quality: RenderQuality::Final,
        uncropped: false,
        histogram: false,
    };
    let weak = Arc::downgrade(inner);
    let _ = inner.interactive.submit_latest_with(
        BEFORE_SLOT,
        JobOptions::new(Priority::P0)
            .label("ビフォーのプレビュー")
            .supersede_policy(SupersedePolicy::KeepRunning),
        move |_ctx| {
            let Some(inner) = weak.upgrade() else {
                return Ok(());
            };
            match render_frame(&inner, &job) {
                Ok(f) => publish(
                    &inner,
                    BeforeFrame {
                        info: f.info,
                        display_rgb8: f.display_rgb8,
                    },
                ),
                Err(e) if e.is_cancelled() => {}
                Err(e) => inner.events.warn(
                    WarningCode::Render,
                    format!("ビフォーのプレビューを描けませんでした: {e}"),
                    Some(job.variant_id),
                    None,
                ),
            }
            Ok(())
        },
    );
}

/// ビフォーの描画の結果をセッションに入れて知らせる（別の写真・前のセッション・古い世代の結果は使わない）。
/// イベントは、結果を入れたのと同じロックの中で送る（現像のプレビューの `publish_frame` と同じ理由）。
fn publish(inner: &Inner, frame: BeforeFrame) {
    let info = frame.info.clone();
    let mut state = inner.develop.state.lock();
    let Some(s) = state.as_mut() else {
        return;
    };
    if s.variant_id != info.variant_id || info.generation < s.first_generation {
        return;
    }
    let Some(b) = s.before.as_mut() else {
        return;
    };
    // 最後に投入した要求の結果だけを使う（それより前の要求は、出どころ・切り抜きが今と違う。ビフォーは
    // 途中の結果を見せなくても、アフターの操作を妨げない）。
    if info.generation < b.generation
        || b.frame
            .as_ref()
            .is_some_and(|f| f.info.generation >= info.generation)
    {
        return;
    }
    b.frame = Some(Arc::new(frame));
    inner.events.emit(Event::BeforeUpdated {
        variant_id: info.variant_id,
        generation: info.generation,
        width: info.width,
        height: info.height,
    });
    inner.develop.cv.notify_all();
}

/// 出どころの設定と表示名をカタログから読む（`current` は今の設定）。
fn resolve(
    c: &genzo_catalog::Catalog,
    variant_id: genzo_model::VariantId,
    source: &BeforeSource,
    current: &DevelopSettings,
) -> Result<(DevelopSettings, String), ApiError> {
    Ok(match source {
        BeforeSource::Initial => match c.history(variant_id)?.first() {
            Some(first) => (c.history_settings(first.id)?, first.label.clone()),
            None => (
                DevelopSettings {
                    process_version: current.process_version,
                    ..DevelopSettings::default()
                },
                BEFORE_LABEL_DEFAULT.to_owned(),
            ),
        },
        BeforeSource::History { entry_id } => {
            let entry = c
                .history(variant_id)?
                .into_iter()
                .find(|h| h.id == *entry_id)
                .ok_or_else(|| {
                    ApiError::NotFound(format!(
                        "履歴の項目 {entry_id}（この写真の履歴にありません）"
                    ))
                })?;
            (
                c.history_settings(entry.id)?,
                format!("履歴: {}", entry.label),
            )
        }
        BeforeSource::Snapshot { snapshot_id } => {
            let snap = c
                .snapshots(variant_id)?
                .into_iter()
                .find(|x| x.id == *snapshot_id)
                .ok_or_else(|| {
                    ApiError::NotFound(format!(
                        "スナップショット {snapshot_id}（この写真のスナップショットにありません）"
                    ))
                })?;
            (
                c.snapshot_settings(snap.id)?,
                format!("スナップショット: {}", snap.name),
            )
        }
        BeforeSource::Current => (current.clone(), BEFORE_LABEL_CURRENT.to_owned()),
    })
}

impl Core {
    /// ビフォー / アフターの表示のビフォーを設定する（DEV-31。`before` の doc）。`None` でやめる（ビフォーの
    /// プレビューを捨てる）。
    ///
    /// 現像中の写真のビフォーを、出どころ（[`BeforeSource`]）の設定にして、ビフォーのプレビューの描画を投入
    /// する（描けたら [`Event::BeforeUpdated`]。ピクセルは [`Core::before_frame`]）。実際に描く設定が今の
    /// ビフォーと同じなら、描いたものをそのまま使う。写真を開いていなければ [`ApiError::NoDevelopSession`]、
    /// 履歴の項目・スナップショットがこの写真のものでなければ [`ApiError::NotFound`]。写真を開き直すと、
    /// ビフォーはやめた状態に戻る（画面は写真ごとに設定し直す）。
    pub fn set_before(&self, source: Option<BeforeSource>) -> Result<Option<BeforeInfo>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let mut state = inner.develop.state.lock();
        let s = state.as_mut().ok_or(ApiError::NoDevelopSession)?;
        let Some(source) = source else {
            s.before = None;
            return Ok(None);
        };
        // ロックの順序は state → カタログ（保存と同じ）。
        let (base, label) =
            inner.with_catalog_api(|c| resolve(c, s.variant_id, &source, &s.settings))?;
        let hash =
            effective_settings(&inner.engine, &base, &s.settings, &s.render_deps).develop_hash();
        // 実際に描く設定が同じなら、描いたもの（と投入した描画）をそのまま使う。
        let (requested, generation, frame) = match s.before.take() {
            Some(old) if old.requested == Some(hash) => (old.requested, old.generation, old.frame),
            _ => (None, 0, None),
        };
        s.before = Some(BeforeState {
            source,
            label,
            base,
            requested,
            generation,
            frame,
        });
        maybe_submit(inner, s);
        Ok(info_of(s))
    }

    /// 最新のビフォーのプレビュー（ピクセルを含む）。ビフォーを設定していない・まだ描けていなければ `None`。
    pub fn before_frame(&self) -> Option<BeforeFrame> {
        let state = self.inner.develop.state.lock();
        state
            .as_ref()
            .and_then(|s| s.before.as_ref())
            .and_then(|b| b.frame.as_deref().cloned())
    }

    /// 今のビフォーの設定で、呼び出したスレッドで描く（同期。CLI・テスト用）。結果は
    /// [`Core::before_frame`] にも入る。ビフォーを設定していなければ [`ApiError::InvalidArgument`]。
    pub fn render_before(&self) -> Result<FrameInfo, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let job = {
            let mut state = inner.develop.state.lock();
            let s = state.as_mut().ok_or(ApiError::NoDevelopSession)?;
            let settings = match s.before.as_ref() {
                Some(b) => effective_settings(&inner.engine, &b.base, &s.settings, &s.render_deps),
                None => return Err(no_before()),
            };
            let generation = inner.develop.next_generation();
            let b = s.before.as_mut().expect("上で確かめた");
            b.requested = Some(settings.develop_hash());
            b.generation = generation;
            RenderJob {
                variant_id: s.variant_id,
                source: s.source.clone(),
                settings,
                generation,
                quality: RenderQuality::Final,
                uncropped: false,
                histogram: false,
            }
        };
        let f = render_frame(inner, &job)?;
        let info = f.info.clone();
        publish(
            inner,
            BeforeFrame {
                info: f.info,
                display_rgb8: f.display_rgb8,
            },
        );
        Ok(info)
    }

    /// 世代 `generation` 以降のビフォーのプレビューが描けるまで待つ（`timeout` を過ぎたら、その時点の
    /// 最新の情報）。ビフォーを設定していなければ `None`。
    pub fn wait_for_before(
        &self,
        generation: u64,
        timeout: Duration,
    ) -> Result<Option<FrameInfo>, ApiError> {
        let shared = &self.inner.develop;
        let deadline = Instant::now() + timeout;
        let mut state = shared.state.lock();
        loop {
            let s = state.as_ref().ok_or(ApiError::NoDevelopSession)?;
            let Some(b) = s.before.as_ref() else {
                return Ok(None);
            };
            if let Some(f) = b.frame.as_ref()
                && f.info.generation >= generation
            {
                return Ok(Some(f.info.clone()));
            }
            if shared.cv.wait_until(&mut state, deadline).timed_out() {
                return Ok(state
                    .as_ref()
                    .and_then(|s| s.before.as_ref())
                    .and_then(|b| b.frame.as_ref().map(|f| f.info.clone())));
            }
        }
    }
}

/// ビフォーを設定していないときのエラー。
pub(crate) fn no_before() -> ApiError {
    ApiError::InvalidArgument(
        "ビフォーが設定されていません（先にビフォー / アフターの表示を始めてください）".to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use genzo_model::CropRect;

    #[test]
    fn effective_settings_take_the_after_geometry_and_deps() {
        let engine = genzo_pipeline::Engine::new(genzo_pipeline::EngineConfig::default()).unwrap();
        let mut base = DevelopSettings {
            exposure_ev: -1.0,
            ..DevelopSettings::default()
        };
        base.geometry.angle_deg = 3.0;
        let mut after = DevelopSettings {
            exposure_ev: 1.5,
            ..DevelopSettings::default()
        };
        after.geometry.angle_deg = -5.0;
        after.geometry.crop = CropRect {
            left: 0.1,
            top: 0.2,
            right: 0.9,
            bottom: 0.8,
        };
        let deps = RenderDeps {
            raw_decoder: "libraw-test".to_owned(),
            ..RenderDeps::default()
        };
        let e = effective_settings(&engine, &base, &after, &deps);
        assert_eq!(e.exposure_ev, -1.0, "現像の項目はビフォーのもの");
        assert_eq!(e.geometry, after.geometry, "切り抜き・角度はアフターのもの");
        assert_eq!(e.render_deps.raw_decoder, "libraw-test");
        assert_eq!(e.process_version, base.process_version);
        // アフターの切り抜き以外の変更では、実際に描く設定は変わらない（描き直さない）。
        let mut after2 = after.clone();
        after2.exposure_ev = 0.3;
        assert_eq!(
            effective_settings(&engine, &base, &after2, &deps).develop_hash(),
            e.develop_hash()
        );
        let mut after3 = after.clone();
        after3.geometry.angle_deg = 1.0;
        assert_ne!(
            effective_settings(&engine, &base, &after3, &deps).develop_hash(),
            e.develop_hash()
        );
    }
}
