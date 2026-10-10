//! 検索（LIB-07・LIB-08）と、検索結果の世代（04 の 3.2 節・3.7 節）。
//!
//! - [`Core::search`] は、条件に合う variant の id を **すべて** 順番どおりに取得してコアのメモリに保持し、
//!   世代番号と件数を返す（グリッドの仮想スクロールの方式。3.2 節）。
//! - UI は [`Core::range`] で「世代・先頭・件数」を要求する。古い世代の要求は [`ApiError::Stale`]。
//! - 選択中の variant は [`Core::index_of`] で id から位置を求める（作り直した後も選択を保つ）。
//! - 評価の変更・取り込み・削除などでカタログが変わったら、表示中の検索結果を作り直し、変わっていれば
//!   新しい世代として [`crate::Event::SearchUpdated`] で知らせる（3.7 節）。
//! - 「★3 以上」で表示中に評価を 2 に下げた写真は、次に [`Core::search`] で条件を適用し直すまで
//!   結果に残す（3.7 節。Lightroom と同様）。選別の操作で変えた variant を（カタログに書く前に）覚えて
//!   おき、選別の条件（評価・フラグ・カラーラベル）を除いた条件で並べた中に残す。
//!
//! **設計からの逸脱**: 3.7 節は作り直しをバックグラウンドで行うとしているが、ここではカタログを変えた
//! 操作の直後に同期で行う（1 回の検索。50 万件での時間を PoC-6 で計測し、必要なら P2 のジョブにする）。

use std::collections::HashSet;
use std::sync::Arc;

use genzo_model::VariantId;

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::Event;
use crate::types::{IndexOf, RangeResult, SearchFilter, SearchResult, SearchSort, VariantSummary};

/// 1 回の [`Core::range`] で返す件数の上限（**仮置き**: 2,000。グリッドの表示範囲と先読みに十分で、
/// 応答が大きくなりすぎない数）。
pub const MAX_RANGE_LEN: u64 = 2_000;

/// 表示中の検索結果。
struct Current {
    generation: u64,
    filter: SearchFilter,
    sort: SearchSort,
    ids: Arc<Vec<VariantId>>,
    /// 選別の操作で変えた variant（次に条件を適用し直すまで、条件に合わなくても残す）。
    sticky: HashSet<VariantId>,
}

/// 検索の状態。
#[derive(Default)]
pub(crate) struct SearchState {
    last_generation: u64,
    current: Option<Current>,
}

impl SearchState {
    fn next_generation(&mut self) -> u64 {
        self.last_generation += 1;
        self.last_generation
    }

    fn current_generation(&self) -> Option<u64> {
        self.current.as_ref().map(|c| c.generation)
    }
}

/// 選別の操作で変える variant のうち、表示中の結果にあるものを「残す対象」にする（3.7 節）。
///
/// カタログに書く **前に** 呼ぶ。書いた後にすると、書いてから [`refresh`] するまでの間に別の操作
/// （取り込みのバッチなど）の作り直しが入ったとき、条件に合わなくなった写真がその作り直しで消えてしまう。
/// 書き込みが失敗しても、残す対象は表示中の結果にあったものだけなので、一覧は変わらない。
pub(crate) fn keep_marked(inner: &Inner, marked: &[VariantId]) {
    if marked.is_empty() {
        return;
    }
    let mut state = inner.search.lock();
    if let Some(cur) = state.current.as_mut() {
        let wanted: HashSet<VariantId> = marked.iter().copied().collect();
        let in_result: Vec<VariantId> = cur
            .ids
            .iter()
            .copied()
            .filter(|v| wanted.contains(v))
            .collect();
        cur.sticky.extend(in_result);
    }
}

/// 表示中の検索結果を作り直す（カタログを変えた操作の後に呼ぶ）。`marked` は選別の操作で変えた
/// variant（条件に合わなくなっても残す）。結果が変わったら新しい世代にしてイベントで知らせる。
///
/// 失敗はログに残すだけにする（呼び出し元の操作自体は成功している）。
pub(crate) fn refresh(inner: &Inner, marked: &[VariantId]) {
    if let Err(e) = try_refresh(inner, marked) {
        tracing::warn!(error = %e, "検索結果の作り直しに失敗しました");
    }
}

fn try_refresh(inner: &Inner, marked: &[VariantId]) -> Result<(), ApiError> {
    let mut state = inner.search.lock();
    let Some(cur) = state.current.as_mut() else {
        return Ok(());
    };
    if !marked.is_empty() {
        let in_result: HashSet<VariantId> = cur.ids.iter().copied().collect();
        cur.sticky
            .extend(marked.iter().copied().filter(|v| in_result.contains(v)));
    }
    let filter = cur.filter.clone();
    let sort = cur.sort;
    let sticky = cur.sticky.clone();
    let ids = inner.with_catalog(|c| {
        let strict = c.search(&filter.to_catalog(), &sort.to_catalog())?;
        if sticky.is_empty() {
            return Ok(strict);
        }
        let strict_set: HashSet<VariantId> = strict.into_iter().collect();
        let relaxed = c.search(&filter.without_marks().to_catalog(), &sort.to_catalog())?;
        Ok(relaxed
            .into_iter()
            .filter(|v| strict_set.contains(v) || sticky.contains(v))
            .collect())
    })?;
    let cur = state.current.as_ref().expect("上で確かめた");
    if *cur.ids == ids {
        return Ok(());
    }
    let generation = state.next_generation();
    let cur = state.current.as_mut().expect("上で確かめた");
    // 削除された variant は残す対象から外す。
    let alive: HashSet<VariantId> = ids.iter().copied().collect();
    cur.sticky.retain(|v| alive.contains(v));
    cur.generation = generation;
    let count = ids.len() as u64;
    cur.ids = Arc::new(ids);
    drop(state);
    inner
        .events
        .emit(Event::SearchUpdated { generation, count });
    Ok(())
}

impl Core {
    /// 条件で検索し、新しい世代の結果をコアのメモリに保持する（LIB-07・LIB-08。3.2 節）。
    pub fn search(
        &self,
        filter: &SearchFilter,
        sort: SearchSort,
    ) -> Result<SearchResult, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let mut state = inner.search.lock();
        let ids = inner.with_catalog(|c| c.search(&filter.to_catalog(), &sort.to_catalog()))?;
        let generation = state.next_generation();
        let count = ids.len() as u64;
        state.current = Some(Current {
            generation,
            filter: filter.clone(),
            sort,
            ids: Arc::new(ids),
            sticky: HashSet::new(),
        });
        Ok(SearchResult { generation, count })
    }

    /// 表示範囲の詳細（3.2 節）。古い世代なら [`ApiError::Stale`]。
    ///
    /// `len` は [`MAX_RANGE_LEN`] まで。範囲が結果の外にはみ出した分は返さない。
    pub fn range(&self, generation: u64, start: u64, len: u64) -> Result<RangeResult, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if len > MAX_RANGE_LEN {
            return Err(ApiError::InvalidArgument(format!(
                "一度に要求できるのは {MAX_RANGE_LEN} 件までです（{len}）"
            )));
        }
        let slice: Vec<VariantId> = {
            let state = inner.search.lock();
            let current = state.current_generation();
            let Some(cur) = state
                .current
                .as_ref()
                .filter(|c| c.generation == generation)
            else {
                return Err(ApiError::Stale {
                    requested: generation,
                    current,
                });
            };
            let n = cur.ids.len() as u64;
            let s = start.min(n) as usize;
            let e = start.saturating_add(len).min(n) as usize;
            cur.ids[s..e].to_vec()
        };
        let summaries = inner.with_catalog(|c| c.variant_summaries(&slice))?;
        let items = inner.with_cache(|cache| {
            summaries
                .into_iter()
                .map(|s| {
                    let rev = cache.thumbs.cache_key(s.variant_id)?;
                    Ok(VariantSummary::from_catalog(s, rev))
                })
                .collect::<genzo_catalog::Result<Vec<_>>>()
        })?;
        Ok(RangeResult {
            generation,
            start,
            items,
        })
    }

    /// 表示中の検索結果での、variant の位置（選択の追跡。3.7 節）。検索していなければ
    /// [`ApiError::NotFound`]。
    pub fn index_of(&self, variant_id: VariantId) -> Result<IndexOf, ApiError> {
        let state = self.inner.search.lock();
        let cur = state
            .current
            .as_ref()
            .ok_or_else(|| ApiError::NotFound("検索結果".to_owned()))?;
        Ok(IndexOf {
            generation: cur.generation,
            index: cur
                .ids
                .iter()
                .position(|&v| v == variant_id)
                .map(|i| i as u64),
        })
    }

    /// 表示中の検索結果の世代と件数（検索していなければ `None`）。
    pub fn current_search(&self) -> Option<SearchResult> {
        let state = self.inner.search.lock();
        state.current.as_ref().map(|c| SearchResult {
            generation: c.generation,
            count: c.ids.len() as u64,
        })
    }

    /// 表示中の検索結果の id（世代が違えば [`ApiError::Stale`]。CLI・テスト用）。
    pub fn result_ids(&self, generation: u64) -> Result<Vec<VariantId>, ApiError> {
        let state = self.inner.search.lock();
        let current = state.current_generation();
        match state.current.as_ref() {
            Some(c) if c.generation == generation => Ok(c.ids.to_vec()),
            _ => Err(ApiError::Stale {
                requested: generation,
                current,
            }),
        }
    }
}
