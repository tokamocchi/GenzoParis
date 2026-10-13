//! 比較表示・選別表示（LIB-03）の表示範囲の描画。
//!
//! ルーペの等倍表示（`region` の doc）は、現像のセッションで開いた 1 枚の写真だけを描く。比較表示は 2 枚の
//! 写真を並べて同じ倍率で拡大・パンする（同期ズーム）ので、現像のセッションとは別に、**面**（`pane`。0 から
//! [`MAX_VIEW_PANES`] 未満）ごとに任意の写真の表示範囲を描く入口を置く（決定事項ログ No.114）。
//!
//! - 描画（[`Core::render_view_region`]）: 面 `pane` に、写真 `request.variant_id` の表示範囲を描く。範囲・倍率・
//!   上限・返す画素は [`Core::render_region`] と同じ（[`RegionInfo::pane`](crate::RegionInfo::pane) に面が入る）。出力は常に切り抜いた後の
//!   画像（C の格子。切り抜きの編集中の表示は使わない）。
//!   - 現像中の写真（[`Core::open_develop`]）なら、そのセッションの入力と今の設定（保存待ちの変更を含む）で
//!     描き、[`RegionInfo::generation`](crate::RegionInfo::generation) はその世代。
//!   - それ以外は、カタログに保存した設定で描く（[`RegionInfo::generation`](crate::RegionInfo::generation) は 0）。展開した入力は
//!     [`MAX_VIEW_SOURCES`] 個までコアに持つ（ファイルとそのリビジョンごと。仮想コピーは同じ入力を使う。
//!     現像中の写真と同じファイルなら、セッションの入力を使う。新しく使ったものを残す）。
//!   - 展開は呼び出し側のスレッドで、対話用のワーカーで行う（[`Core::open_develop`] と同じ。展開の前にファイルを
//!     ディスクと照合する）。展開は 1 つずつ行う。同じ面の新しい要求が別のファイルなら、展開中・展開を待って
//!     いる古い要求をやめる（[`ApiError::Cancelled`]）。同じファイルなら、古い要求の展開をそのまま使う。
//!   - 動画は描けない（[`ApiError::InvalidArgument`]）。
//!   - **面ごとに別の P0 の「最新の 1 件だけ」の枠**（同じ面の古い要求は取り消されて [`ApiError::Cancelled`] で
//!     戻る。別の面の要求は取り消さない。対話用のスケジューラは 1 本のスレッドなので、面の描画は順に行う）。
//!     結果は面ごとに [`Core::view_region_frame`] で取る。要求の番号は面ごと。
//! - 出力の大きさ（[`Core::view_output_size`]）: 面で使う写真の、出力の画像（C）のフル解像度の大きさ（等倍表示の
//!   準備。ジオメトリだけを求めるので、描くより軽い）。入力の扱いは描画と同じ。
//! - 取り消し（[`Core::cancel_view_regions`]）: すべての面の描画と展開を取り消す。解放（[`Core::release_view`]）:
//!   比較表示をやめたとき、描画と展開を取り消し、持っている入力と結果を捨てる（メモリを返す。カタログを閉じた
//!   ときも捨てる）。入力を用意する間に解放したら、その要求は描かない（[`ApiError::Cancelled`]）。
//!
//! メモリ: 入力は RAW なら展開した CFA（ベイヤーなら 1 画素 2 バイト）、RAW 以外は B2 の画像（1 画素 12 バイト。
//! 24 M 画素で約 288 MB）。入力の数の上限（[`MAX_VIEW_SOURCES`]）と、比較表示をやめたときの解放で抑える。描画の
//! 一時的なメモリと、面ごとに持つ最後の結果は `region` と同じ（画素数の上限 [`crate::MAX_REGION_OUTPUT_PIXELS`]）。
//! L0 / L1 のキャッシュには書かない（作り直しの経路は `previews` の 1 つだけにする）。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use genzo_jobs::CancellationToken;
use genzo_model::{AssetKind, RenderDeps, VariantId};
use genzo_pipeline::{PhotoSource, SourceId};
use genzo_worker::Lane;
use parking_lot::Mutex;

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::previews::{VariantFile, load_source, variant_file};
use crate::region::{RegionInput, RegionMark, RegionSlot, RegionState, render_latest, validate};
use crate::types::{RegionFrame, RegionRequest, ViewOutputSize};

/// 比較表示の面の数（**仮置き**: 2。比較表示は 2 枚を並べる。選別表示は拡大しないので面を使わない）。
pub const MAX_VIEW_PANES: u32 = 2;

/// 比較表示のために展開しておく入力の数の上限（**仮置き**: 2。面の数と同じ。現像中の写真の入力は数えない）。
pub const MAX_VIEW_SOURCES: usize = 2;

/// 展開した入力（ファイルとそのリビジョンごと）。
struct ViewSource {
    source: PhotoSource,
    deps: RenderDeps,
}

/// 面の状態。
#[derive(Default)]
struct ViewPane {
    /// 表示範囲の描画（要求の番号・取り消し・最後の結果。`region` と同じ）。
    region: Mutex<RegionState>,
    /// 展開中・展開を待っている要求（展開するファイルと取り消しトークン）。
    loading: Mutex<Vec<(SourceId, CancellationToken)>>,
}

/// 比較表示の状態（[`Inner`] が持つ）。
pub(crate) struct ViewShared {
    /// 展開した入力（末尾が最も新しく使ったもの）。
    sources: Mutex<Vec<ViewSource>>,
    /// 展開を 1 つずつにする（持っている間に `sources`・面の `loading` のロックを短く取る）。
    load_lock: Mutex<()>,
    /// 捨てた回数（[`Core::release_view`]。捨てる前に始めた展開の結果を持たない）。
    epoch: AtomicU64,
    panes: Vec<ViewPane>,
}

impl Default for ViewShared {
    fn default() -> Self {
        Self {
            sources: Mutex::new(Vec::new()),
            load_lock: Mutex::new(()),
            epoch: AtomicU64::new(0),
            panes: (0..MAX_VIEW_PANES).map(|_| ViewPane::default()).collect(),
        }
    }
}

impl ViewShared {
    fn pane(&self, pane: u32) -> Result<&ViewPane, ApiError> {
        self.panes.get(pane as usize).ok_or_else(|| {
            ApiError::InvalidArgument(format!(
                "比較表示の面は 0〜{} です（{pane}）",
                MAX_VIEW_PANES - 1
            ))
        })
    }

    /// 持っている入力（あれば、新しく使ったことにする）。
    fn lookup(&self, id: SourceId) -> Option<(PhotoSource, RenderDeps)> {
        let mut sources = self.sources.lock();
        let at = sources.iter().position(|s| s.source.id == id)?;
        let hit = sources.remove(at);
        let out = (hit.source.clone(), hit.deps.clone());
        sources.push(hit);
        Some(out)
    }

    /// 入力を入れる（同じファイルの古いリビジョンは捨て、上限を超えたら古いものから捨てる）。
    fn insert(&self, source: PhotoSource, deps: RenderDeps) {
        let mut sources = self.sources.lock();
        sources.retain(|s| s.source.id.file_id != source.id.file_id);
        sources.push(ViewSource { source, deps });
        let over = sources.len().saturating_sub(MAX_VIEW_SOURCES);
        sources.drain(..over);
    }

    /// 持っている入力の数。
    fn len(&self) -> usize {
        self.sources.lock().len()
    }

    /// すべての面の描画と展開を取り消す。取り消したものがあれば `true`。
    fn cancel_all(&self) -> bool {
        let mut cancelled = false;
        for p in &self.panes {
            if let Some(token) = p.region.lock().take_pending() {
                token.cancel();
                cancelled = true;
            }
            for (_, token) in p.loading.lock().drain(..) {
                token.cancel();
                cancelled = true;
            }
        }
        cancelled
    }

    /// 描画と展開を取り消し、入力と結果を捨てる。捨てた入力の数を返す。
    pub(crate) fn release(&self) -> usize {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.cancel_all();
        for p in &self.panes {
            p.region.lock().clear_last();
        }
        let mut sources = self.sources.lock();
        let n = sources.len();
        sources.clear();
        n
    }
}

impl Core {
    /// 比較表示（LIB-03）の面 `pane` に、写真の表示範囲を描く（`view` の doc）。
    ///
    /// 描き終わるまで待つ（Tauri からは `spawn_blocking` で呼ぶ）。同じ面の新しい要求か
    /// [`Core::cancel_view_regions`]・[`Core::release_view`] で取り消されたら [`ApiError::Cancelled`]。面・範囲・
    /// 倍率が不正、動画なら [`ApiError::InvalidArgument`]、元ファイルが見つからなければ [`ApiError::FileAccess`]。
    pub fn render_view_region(
        &self,
        pane: u32,
        request: &RegionRequest,
    ) -> Result<RegionFrame, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        validate(request)?;
        let p = inner.view.pane(pane)?;
        let epoch = inner.view.epoch.load(Ordering::Acquire);
        let input = view_input(inner, p, request.variant_id)?;
        // 入力を用意する間に比較表示をやめた（release_view）: 描かない（捨てた後に結果を持たない）。
        if inner.view.epoch.load(Ordering::Acquire) != epoch {
            return Err(ApiError::Cancelled);
        }
        let slot = RegionSlot {
            state: &p.region,
            key: format!("view-region-{pane}").into(),
            label: "比較表示の表示範囲の描画",
            mark: RegionMark {
                before: false,
                pane: Some(pane),
            },
            hooks: false,
        };
        render_latest(inner, &slot, request, input)
    }

    /// 比較表示の面 `pane` で使う写真の、出力の画像（C）のフル解像度の大きさ（等倍表示の準備。`view` の doc）。
    pub fn view_output_size(
        &self,
        pane: u32,
        variant_id: VariantId,
    ) -> Result<ViewOutputSize, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let p = inner.view.pane(pane)?;
        let input = view_input(inner, p, variant_id)?;
        let plan = inner
            .engine
            .full_resolution_geometry(&input.source, &input.settings, false)?;
        let (width, height) = plan.output_size();
        Ok(ViewOutputSize {
            variant_id,
            width,
            height,
        })
    }

    /// 比較表示の面 `pane` の、最後に描けた表示範囲（なければ `None`。面が範囲の外でも `None`）。
    pub fn view_region_frame(&self, pane: u32) -> Option<RegionFrame> {
        let p = self.inner.view.pane(pane).ok()?;
        p.region.lock().last().cloned()
    }

    /// 比較表示のすべての面の描画と展開を取り消す（その呼び出しは [`ApiError::Cancelled`] で戻る）。取り消した
    /// ものがあれば `true`。
    pub fn cancel_view_regions(&self) -> Result<bool, ApiError> {
        self.inner.check_open()?;
        Ok(self.inner.view.cancel_all())
    }

    /// 比較表示をやめる: 描画と展開を取り消し、持っている入力と最後の結果を捨てる（メモリを返す）。捨てた入力の
    /// 数を返す。
    pub fn release_view(&self) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        Ok(self.inner.view.release())
    }

    /// 比較表示のために持っている入力の数（テスト・CLI 用。`view` の doc）。
    pub fn view_sources(&self) -> usize {
        self.inner.view.len()
    }
}

/// 面 `pane` で写真 `variant_id` を描く入力（`view` の doc）。
fn view_input(
    inner: &Arc<Inner>,
    pane: &ViewPane,
    variant_id: VariantId,
) -> Result<RegionInput, ApiError> {
    // 現像中の写真なら、セッションの入力と今の設定（保存待ちの変更を含む）。
    if let Ok(s) = crate::develop::session_snapshot(inner, variant_id) {
        return Ok(RegionInput {
            source: s.source,
            settings: s.settings,
            generation: s.generation,
            uncropped: false,
        });
    }
    let file = inner.with_catalog_api(|c| variant_file(c, variant_id))?;
    if file.kind == AssetKind::Video {
        return Err(ApiError::InvalidArgument(
            "動画は拡大して表示できません（カタログの管理だけ）".to_owned(),
        ));
    }
    let (source, deps) = source_of(inner, pane, &file)?;
    // 設定は入力を用意した後に読む（展開の間に設定が貼り付けられても、古い設定で描かない）。
    let state = inner.with_catalog(|c| c.develop_state(variant_id))?;
    Ok(RegionInput {
        source,
        settings: crate::deps::with_render_deps(&state.settings, &deps),
        generation: 0,
        uncropped: false,
    })
}

/// ファイルの入力（持っていれば、それ。現像中の写真と同じファイルなら、セッションの入力。なければ展開する）。
fn source_of(
    inner: &Arc<Inner>,
    pane: &ViewPane,
    file: &VariantFile,
) -> Result<(PhotoSource, RenderDeps), ApiError> {
    let id = SourceId::new(file.file_id, file.revision);
    let view = &inner.view;
    if let Some(hit) = view.lookup(id) {
        return Ok(hit);
    }
    if let Some(hit) = crate::develop::session_source_of_file(inner, id) {
        return Ok(hit);
    }
    // 同じ面の、別のファイルを展開している・待っている古い要求をやめる（同じファイルなら、その展開を使う）。
    let token = CancellationToken::new();
    {
        let mut loading = pane.loading.lock();
        loading.retain(|(other, t)| {
            if *other == id {
                true
            } else {
                t.cancel();
                false
            }
        });
        loading.push((id, token.clone()));
    }
    let epoch = view.epoch.load(Ordering::Acquire);
    let loaded = {
        let _one = view.load_lock.lock();
        if token.is_cancelled() {
            Err(ApiError::Cancelled)
        } else if let Some(hit) = view.lookup(id) {
            Ok(Ok(hit))
        } else {
            load_source(inner, file, Lane::Interactive, &token).map(Err)
        }
    };
    pane.loading.lock().retain(|(_, t)| !t.same_as(&token));
    match loaded? {
        Ok(hit) => Ok(hit),
        Err(loaded) => {
            if token.is_cancelled() {
                return Err(ApiError::Cancelled);
            }
            // 捨てた（比較表示をやめた）後に終わった展開は持たない。
            if view.epoch.load(Ordering::Acquire) == epoch {
                view.insert(loaded.source.clone(), loaded.render_deps.clone());
            }
            Ok((loaded.source, loaded.render_deps))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use genzo_model::{FileId, Orientation};
    use genzo_pipeline::{ColorContract, RgbImage, SourceImage};

    fn source(file: i64, revision: u32) -> PhotoSource {
        let image = RgbImage::filled(4, 4, ColorContract::B2Working, [0.18; 3]).unwrap();
        PhotoSource::new(
            SourceId::new(FileId::new(file), revision),
            SourceImage::Working {
                image: Arc::new(image),
                orientation: Orientation::Normal,
            },
        )
        .unwrap()
    }

    fn ids(v: &ViewShared) -> Vec<(i64, u32)> {
        v.sources
            .lock()
            .iter()
            .map(|s| (s.source.id.file_id.get(), s.source.id.file_revision))
            .collect()
    }

    #[test]
    fn sources_keep_the_most_recently_used_up_to_the_limit() {
        let v = ViewShared::default();
        v.insert(source(1, 1), RenderDeps::default());
        v.insert(source(2, 1), RenderDeps::default());
        assert_eq!(ids(&v), vec![(1, 1), (2, 1)]);
        // 使ったものは新しくなり、上限を超えると最も古いものから捨てる。
        assert!(v.lookup(SourceId::new(FileId::new(1), 1)).is_some());
        v.insert(source(3, 1), RenderDeps::default());
        assert_eq!(ids(&v), vec![(1, 1), (3, 1)]);
        // 別のリビジョンは別の入力（見つからない）。同じファイルの新しいリビジョンは古いものと入れ替わる。
        assert!(v.lookup(SourceId::new(FileId::new(3), 2)).is_none());
        v.insert(source(3, 2), RenderDeps::default());
        assert_eq!(ids(&v), vec![(1, 1), (3, 2)]);
        assert_eq!(v.release(), 2);
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn panes_outside_the_range_are_rejected() {
        let v = ViewShared::default();
        assert!(v.pane(0).is_ok());
        assert!(v.pane(MAX_VIEW_PANES - 1).is_ok());
        assert!(matches!(
            v.pane(MAX_VIEW_PANES),
            Err(ApiError::InvalidArgument(_))
        ));
    }

    #[test]
    fn cancelling_takes_pending_loads_of_every_pane() {
        let v = ViewShared::default();
        let t = CancellationToken::new();
        v.panes[1]
            .loading
            .lock()
            .push((SourceId::new(FileId::new(1), 1), t.clone()));
        assert!(v.cancel_all());
        assert!(t.is_cancelled());
        assert!(!v.cancel_all(), "2 回目は取り消すものがない");
    }
}
