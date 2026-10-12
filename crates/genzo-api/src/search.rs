//! 検索（LIB-07・LIB-08）と、検索結果の世代（04 の 3.2 節・3.7 節）。
//!
//! - [`Core::search`] は、条件に合う variant の id を **すべて** 順番どおりに取得してコアのメモリに保持し、
//!   世代番号と件数を返す（グリッドの仮想スクロールの方式。3.2 節）。
//! - UI は [`Core::range`] で「世代・先頭・件数」を要求する。古い世代の要求は [`ApiError::Stale`]。
//! - 選択中の variant は [`Core::index_of`] で id から位置を求める（作り直した後も選択を保つ）。
//! - 評価の変更・取り込み・削除などでカタログが変わったら、表示中の検索結果を **バックグラウンドで**
//!   作り直し、変わっていれば新しい世代として [`crate::Event::SearchUpdated`] で知らせる（3.7 節。
//!   implementation_status の No.11・27 の (a)）。作り直しは専用のスレッド（`genzo-search-refresh`）が
//!   読み取り専用の接続（WAL なので書き込みと並行して読める）で行い、カタログを変える操作を待たせない。
//!   - 要求は通し番号で数え、続けて来た要求はまとめて 1 回で作り直す（実行中に来た要求は、終わった後に
//!     もう 1 回）。
//!   - 作り直している間に [`Core::search`] で条件を適用し直したら、その結果は捨てる（古い条件の結果で
//!     新しい結果を上書きしない）。
//!   - 作り直すまでは、今の世代のまま [`Core::range`] に答える（世代が変わるのは作り直した結果を入れた
//!     ときだけ）。
//!   - 作り直しが終わるのを待つ [`Core::wait_search_refreshed`]（CLI・テスト用。UI はイベントで知る）。
//! - 絞り込みの候補（カメラ・レンズ・動画のコーデックと件数）は [`Core::facets`]（カタログ全体。件数は
//!   asset の数。[`Facets`]）。
//! - 「★3 以上」で表示中に評価を 2 に下げた写真は、次に [`Core::search`] で条件を適用し直すまで
//!   結果に残す（3.7 節。Lightroom と同様）。選別の操作で変えた variant を（カタログに書く前に）覚えて
//!   おき、選別の条件（評価・フラグ・カラーラベル）を除いた条件で並べた中に残す。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use genzo_catalog::Catalog;
use genzo_model::VariantId;
use parking_lot::{Condvar, Mutex, MutexGuard};

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::Event;
use crate::types::{
    Facets, IndexOf, RangeResult, SearchFilter, SearchResult, SearchSort, VariantSummary,
};

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
    /// 条件を適用し直した回数（[`Core::search`] ごとに 1 増やす）。作り直しの結果が古い条件のもの
    /// （作り直している間に条件を適用し直した）なら捨てる。
    epoch: u64,
    /// 作り直しの要求の通し番号（最後の要求）。
    requested: u64,
    /// 作り直しを終えた要求の通し番号（`requested` 以下）。
    completed: u64,
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

/// 検索の状態と、作り直しのスレッドとの受け渡し。
#[derive(Default)]
pub(crate) struct SearchShared {
    state: Mutex<SearchState>,
    /// 要求の追加・作り直しの完了・停止を知らせる。
    changed: Condvar,
    stop: AtomicBool,
}

impl SearchShared {
    pub(crate) fn lock(&self) -> MutexGuard<'_, SearchState> {
        self.state.lock()
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

/// 表示中の検索結果の作り直しを要求する（カタログを変えた操作の後に呼ぶ）。`marked` は選別の操作で
/// 変えた variant（条件に合わなくなっても残す）。作り直しは専用のスレッドで行い、結果が変わったら新しい
/// 世代にしてイベントで知らせる。この関数は待たない。
pub(crate) fn refresh(inner: &Inner, marked: &[VariantId]) {
    let shared = &inner.search;
    let mut state = shared.lock();
    let Some(cur) = state.current.as_mut() else {
        return;
    };
    if !marked.is_empty() {
        let wanted: HashSet<VariantId> = marked.iter().copied().collect();
        let in_result: Vec<VariantId> = cur
            .ids
            .iter()
            .copied()
            .filter(|v| wanted.contains(v))
            .collect();
        cur.sticky.extend(in_result);
    }
    state.requested += 1;
    drop(state);
    shared.changed.notify_all();
}

/// 作り直しの 1 回分の材料（要求を受け取った時点の条件）。
struct Snapshot {
    target: u64,
    epoch: u64,
    filter: SearchFilter,
    sort: SearchSort,
    /// 選別の条件を除いた条件でも検索するか（残す対象があり、条件に選別の条件があるとき）。
    relaxed: bool,
}

/// 作り直しのスレッドを起動する（[`Core::open_with`] の最後）。
pub(crate) fn spawn_refresher(inner: &Arc<Inner>) -> Result<JoinHandle<()>, ApiError> {
    let shared = Arc::clone(&inner.search);
    let weak = Arc::downgrade(inner);
    let path = inner.config.catalog_path.clone();
    std::thread::Builder::new()
        .name("genzo-search-refresh".to_owned())
        .spawn(move || refresher_loop(&shared, &weak, &path))
        .map_err(|e| ApiError::Internal(format!("検索結果の作り直しのスレッドを作れません: {e}")))
}

/// 作り直しのスレッドを止めて、終わるのを待つ（[`Core::close`]。カタログを閉じる前に呼ぶ）。
pub(crate) fn stop_refresher(inner: &Inner, handle: Option<JoinHandle<()>>) {
    inner.search.stop.store(true, Ordering::Release);
    {
        // 待っているスレッドが停止の印を見落とさないよう、ロックを取ってから知らせる。
        let _state = inner.search.lock();
        inner.search.changed.notify_all();
    }
    if let Some(h) = handle {
        let _ = h.join();
    }
}

fn refresher_loop(shared: &SearchShared, weak: &Weak<Inner>, catalog_path: &PathBuf) {
    // 読み取り専用の接続（初めて使うときに開く。開けなければ本体の接続を使う）。
    let mut reader: Option<Catalog> = None;
    let mut reader_failed = false;
    loop {
        let snapshot = {
            let mut state = shared.lock();
            loop {
                if shared.stop.load(Ordering::Acquire) {
                    return;
                }
                if state.completed < state.requested {
                    break;
                }
                shared.changed.wait(&mut state);
            }
            let target = state.requested;
            match state.current.as_ref() {
                None => {
                    state.completed = target;
                    shared.changed.notify_all();
                    continue;
                }
                Some(cur) => Snapshot {
                    target,
                    epoch: state.epoch,
                    filter: cur.filter.clone(),
                    sort: cur.sort,
                    relaxed: !cur.sticky.is_empty() && has_mark_conditions(&cur.filter),
                },
            }
        };
        let Some(inner) = weak.upgrade() else {
            return;
        };
        if reader.is_none() && !reader_failed {
            match Catalog::open_read_only(catalog_path) {
                Ok(c) => reader = Some(c),
                Err(e) => {
                    tracing::warn!(error = %e, "検索結果の作り直しに読み取り専用の接続を使えないため、本体の接続を使います");
                    reader_failed = true;
                }
            }
        }
        let result = query(&inner, reader.as_ref(), &snapshot);
        let event = {
            let mut state = shared.lock();
            match result {
                Ok((strict, relaxed)) => commit(&mut state, &snapshot, strict, relaxed),
                Err(e) => {
                    tracing::warn!(error = %e, "検索結果の作り直しに失敗しました");
                    None
                }
            }
        };
        // イベントを送ってから、要求を済んだことにする（[`Core::wait_search_refreshed`] が戻ったときには、
        // その作り直しのイベントが届いている）。
        if let Some(e) = event {
            inner.events.emit(e);
        }
        let mut state = shared.lock();
        state.completed = state.completed.max(snapshot.target);
        shared.changed.notify_all();
    }
}

/// 条件に選別の条件（評価・フラグ・カラーラベル）があるか（あれば、選別の操作で結果から外れうる）。
fn has_mark_conditions(filter: &SearchFilter) -> bool {
    *filter != filter.without_marks()
}

/// 作り直しの検索（条件どおりの結果と、必要なら選別の条件を除いた結果）。
fn query(
    inner: &Inner,
    reader: Option<&Catalog>,
    snap: &Snapshot,
) -> Result<(Vec<VariantId>, Option<Vec<VariantId>>), ApiError> {
    let run = |c: &Catalog| -> genzo_catalog::Result<(Vec<VariantId>, Option<Vec<VariantId>>)> {
        let sort = snap.sort.to_catalog();
        let strict = c.search(&snap.filter.to_catalog(), &sort)?;
        let relaxed = if snap.relaxed {
            Some(c.search(&snap.filter.without_marks().to_catalog(), &sort)?)
        } else {
            None
        };
        Ok((strict, relaxed))
    };
    match reader {
        Some(c) => Ok(run(c)?),
        None => inner.with_catalog(|c| run(c)),
    }
}

/// 作り直した結果を入れる。結果が変わったら新しい世代にして、知らせるイベントを返す。
fn commit(
    state: &mut SearchState,
    snap: &Snapshot,
    strict: Vec<VariantId>,
    relaxed: Option<Vec<VariantId>>,
) -> Option<Event> {
    // 作り直している間に条件を適用し直した（古い条件の結果）。
    if state.epoch != snap.epoch {
        return None;
    }
    let cur = state.current.as_mut()?;
    let ids = if cur.sticky.is_empty() || !has_mark_conditions(&cur.filter) {
        strict
    } else if let Some(relaxed) = relaxed {
        let strict_set: HashSet<VariantId> = strict.into_iter().collect();
        relaxed
            .into_iter()
            .filter(|v| strict_set.contains(v) || cur.sticky.contains(v))
            .collect()
    } else {
        // 検索を始めた後に残す対象が増えた（選別の操作）。この結果では、その写真の位置が分からないので
        // 入れずに、選別の条件を除いた検索も含めてもう一度作り直す。
        state.requested += 1;
        return None;
    };
    if *cur.ids == ids {
        return None;
    }
    let generation = state.next_generation();
    let cur = state.current.as_mut().expect("上で確かめた");
    // 削除された variant は残す対象から外す。
    let alive: HashSet<VariantId> = ids.iter().copied().collect();
    cur.sticky.retain(|v| alive.contains(v));
    cur.generation = generation;
    let count = ids.len() as u64;
    cur.ids = Arc::new(ids);
    Some(Event::SearchUpdated { generation, count })
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
        // 実行中の作り直しの結果は捨て、これまでの要求は済んだことにする（この検索は、要求の元になった
        // カタログの変更をすべて反映している。変更を書いてから要求するため）。
        state.epoch += 1;
        state.completed = state.requested;
        drop(state);
        inner.search.changed.notify_all();
        Ok(SearchResult { generation, count })
    }

    /// 絞り込みの候補（カタログ全体のカメラ・レンズ・動画のコーデックと、それぞれの asset の数。LIB-07）。
    ///
    /// 値は [`SearchFilter`] の `cameras`・`lenses`・`codecs` にそのまま渡すと一致する（[`Facets`] の
    /// doc）。カメラ・レンズは索引だけを読む問い合わせ（genzo-catalog の `Catalog::facets`）。50 万件の
    /// ダミーのカタログ（PoC-6 の `populate_dummy`）で 3 つ合わせて約 58ms（2026-10-12 にこのコンテナの
    /// release ビルドで計測した参考値）。
    pub fn facets(&self) -> Result<Facets, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        Ok(inner.with_catalog(|c| c.facets())?.into())
    }

    /// これまでに要求した検索結果の作り直しが終わるまで待ち、表示中の検索結果の世代と件数を返す
    /// （検索していなければ `None`）。戻ったときには、その作り直しの [`crate::Event::SearchUpdated`] は
    /// 送ってある。`timeout` を過ぎたら [`ApiError::Job`]。
    ///
    /// 作り直しはバックグラウンドで行う（3.7 節）ので、カタログを変えた直後の結果が要る CLI・テストで
    /// 使う。UI は [`crate::Event::SearchUpdated`] で知る。
    pub fn wait_search_refreshed(
        &self,
        timeout: Option<Duration>,
    ) -> Result<Option<SearchResult>, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let deadline = timeout.and_then(|t| Instant::now().checked_add(t));
        let shared = &inner.search;
        let mut state = shared.lock();
        let target = state.requested;
        while state.completed < target {
            if shared.stop.load(Ordering::Acquire) {
                return Err(ApiError::Closed);
            }
            match deadline {
                Some(d) => {
                    if shared.changed.wait_until(&mut state, d).timed_out()
                        && state.completed < target
                    {
                        return Err(ApiError::Job(format!(
                            "検索結果の作り直しが {:?} 以内に終わりませんでした",
                            timeout.unwrap_or_default()
                        )));
                    }
                }
                None => shared.changed.wait(&mut state),
            }
        }
        Ok(state.current.as_ref().map(|c| SearchResult {
            generation: c.generation,
            count: c.ids.len() as u64,
        }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use genzo_model::Rating;

    fn v(n: i64) -> VariantId {
        VariantId::new(n)
    }

    fn three_up() -> SearchFilter {
        SearchFilter {
            rating_min: Rating::new(3),
            ..Default::default()
        }
    }

    /// 表示中の結果（世代 1、条件 `filter`、id `ids`）を持つ状態。
    fn state_with(filter: SearchFilter, ids: &[i64]) -> SearchState {
        let mut st = SearchState::default();
        let generation = st.next_generation();
        st.current = Some(Current {
            generation,
            filter,
            sort: SearchSort::default(),
            ids: Arc::new(ids.iter().map(|&n| v(n)).collect()),
            sticky: HashSet::new(),
        });
        st.requested = 1;
        st
    }

    fn snapshot(st: &SearchState, relaxed: bool) -> Snapshot {
        let cur = st.current.as_ref().unwrap();
        Snapshot {
            target: st.requested,
            epoch: st.epoch,
            filter: cur.filter.clone(),
            sort: cur.sort,
            relaxed,
        }
    }

    fn ids(st: &SearchState) -> Vec<i64> {
        st.current
            .as_ref()
            .unwrap()
            .ids
            .iter()
            .map(|x| x.get())
            .collect()
    }

    #[test]
    fn changed_results_become_a_new_generation() {
        let mut st = state_with(SearchFilter::default(), &[1, 2]);
        let snap = snapshot(&st, false);
        let ev = commit(&mut st, &snap, vec![v(1), v(2), v(3)], None);
        assert!(matches!(
            ev,
            Some(Event::SearchUpdated {
                generation: 2,
                count: 3
            })
        ));
        assert_eq!(ids(&st), vec![1, 2, 3]);
        // 同じ結果なら世代を変えない。
        let snap = snapshot(&st, false);
        assert!(commit(&mut st, &snap, vec![v(1), v(2), v(3)], None).is_none());
        assert_eq!(st.current_generation(), Some(2));
    }

    #[test]
    fn results_for_an_old_filter_are_discarded() {
        let mut st = state_with(SearchFilter::default(), &[1, 2]);
        let snap = snapshot(&st, false);
        // 作り直している間に条件を適用し直した（Core::search）。
        st.epoch += 1;
        assert!(commit(&mut st, &snap, vec![v(9)], None).is_none());
        assert_eq!(ids(&st), vec![1, 2]);
        assert_eq!(st.current_generation(), Some(1));
    }

    #[test]
    fn marked_variants_stay_until_the_filter_is_applied_again() {
        // ★3 以上を表示中に、2 の評価を下げた（残す対象）。
        let mut st = state_with(three_up(), &[1, 2, 3]);
        st.current.as_mut().unwrap().sticky.insert(v(2));
        let snap = snapshot(&st, true);
        // 条件どおりの結果からは 2 が外れ、新しい 4 が入った。選別の条件を除いた結果の順に並べる。
        let ev = commit(
            &mut st,
            &snap,
            vec![v(1), v(3), v(4)],
            Some(vec![v(1), v(2), v(3), v(4), v(5)]),
        );
        assert!(ev.is_some());
        assert_eq!(ids(&st), vec![1, 2, 3, 4]);
        // 削除された variant は残す対象から外す。
        let snap = snapshot(&st, true);
        commit(
            &mut st,
            &snap,
            vec![v(1), v(3)],
            Some(vec![v(1), v(3), v(5)]),
        );
        assert_eq!(ids(&st), vec![1, 3]);
        assert!(st.current.as_ref().unwrap().sticky.is_empty());
    }

    #[test]
    fn marks_made_during_the_query_trigger_another_refresh() {
        // 検索を始めた時点では残す対象がなかった（選別の条件を除いた検索をしていない）。
        let mut st = state_with(three_up(), &[1, 2, 3]);
        let snap = snapshot(&st, false);
        // 検索の間に 2 の評価を下げた（keep_marked）。条件どおりの結果からは 2 が外れている。
        st.current.as_mut().unwrap().sticky.insert(v(2));
        let before = st.requested;
        assert!(commit(&mut st, &snap, vec![v(1), v(3)], None).is_none());
        // 2 を一覧から消さずに、もう一度作り直す。
        assert_eq!(ids(&st), vec![1, 2, 3]);
        assert_eq!(st.requested, before + 1);
    }

    #[test]
    fn sticky_is_ignored_without_mark_conditions() {
        // 条件に選別の条件がなければ、選別の操作で結果から外れることはない（残す対象は使わない）。
        let mut st = state_with(SearchFilter::default(), &[1, 2]);
        st.current.as_mut().unwrap().sticky.insert(v(2));
        let snap = snapshot(&st, false);
        // 2 は削除された。
        commit(&mut st, &snap, vec![v(1)], None);
        assert_eq!(ids(&st), vec![1]);
    }
}
