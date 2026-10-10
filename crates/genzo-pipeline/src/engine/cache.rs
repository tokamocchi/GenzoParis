//! 段階 A0 / A1 / B の中間キャッシュ（docs/04_architecture.md の 2.2 節・4.1 節）。
//!
//! 「最新の数件だけ」を持つ小さな LRU（内部の `Lru`）。キーが一致すれば再計算を省略する（2.2 節「各段階の
//! キャッシュのキーは 4.1 節の共通の規則に従う。キーが一致すれば再計算を省略します」）。ヒット・ミスの
//! 回数を数え（[`CacheCounters`]）、テストで「露光量だけを変えたときにガイドを作り直さない」などを
//! 確かめる。
//!
//! | 段階 | キー（[`crate::engine::Engine`] が作る） | 値 |
//! |---|---|---|
//! | A0 | [`A0Key`]: 元ファイル（ID とリビジョン）＋ `hash_for_phase(A0)`（処理バージョン・RAW デコーダ） | [`crate::SourceImage`]（RAW は `Arc` で保持） |
//! | A1 | genzo-model の [`CacheKey`]（`kind = A1Intermediate`、`develop_hash = hash_for_phase(A1)`、`size = (長辺, 0)`、`quality`） | [`crate::A1Image`] |
//! | B | [`GuideKey`]: [`CacheKey`]（`kind = BGuide`、`develop_hash = hash_for_phase(B)`、`size` = ガイドの寸法、`quality` = 入力の A1 の品質）＋ 入力の A1 の長辺 ＋ ガイドのパラメータ | [`crate::stage::Guide`] |
//!
//! 段階 C は毎回計算する（キャッシュしない。2.2 節の表）。

use std::collections::VecDeque;
use std::sync::Arc;

use genzo_model::CacheKey;

use crate::engine::SourceId;
use crate::guide::GuideParams;

/// キャッシュのヒット・ミスの回数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheCounters {
    /// キーが一致して再計算を省略した回数。
    pub hits: u64,
    /// キーが一致せず、計算（A0 は読み込み）した回数。
    pub misses: u64,
}

/// 段階 A0 のキー。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct A0Key {
    /// 元ファイル。
    pub source: SourceId,
    /// `DevelopSettings::hash_for_phase(Phase::A0)`（処理バージョンと RAW デコーダ）。
    pub develop_hash: [u8; 32],
}

/// 段階 B（ガイド）のキー（2.7 節の表の「含める」項目）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GuideKey {
    /// genzo-model のキー（元ファイル・処理バージョン・外部データ・`hash_for_phase(B)`・ガイドの寸法・
    /// 入力の A1 の品質・保存形式）。
    pub cache: CacheKey,
    /// 入力（段階 A1 のプレビュー）の長辺。
    pub a1_long_edge: u32,
    /// ガイドのアルゴリズムのパラメータ（[`GuideParams`] の各値のビット列）。処理バージョンで決まるが、
    /// 明示的にキーに含める（2.7 節「ガイドのアルゴリズムのパラメータ」）。
    pub params: GuideParamsKey,
}

/// [`GuideParams`] をキーにするための値（浮動小数点はビット列にする）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GuideParamsKey {
    /// 長辺（px）。
    pub long_edge: u32,
    /// 半径（G の長辺に対する割合）の f64 のビット列。
    pub radius_fraction_bits: u64,
    /// ε の f32 のビット列。
    pub epsilon_bits: u32,
}

impl From<GuideParams> for GuideParamsKey {
    fn from(p: GuideParams) -> Self {
        Self {
            long_edge: p.long_edge,
            radius_fraction_bits: p.radius_fraction.to_bits(),
            epsilon_bits: p.epsilon.to_bits(),
        }
    }
}

/// 最新の `capacity` 件だけを持つキャッシュ（最後に使ったものが先頭）。
pub(crate) struct Lru<K, V> {
    capacity: usize,
    entries: VecDeque<(K, Arc<V>)>,
    counters: CacheCounters,
}

impl<K, V> std::fmt::Debug for Lru<K, V> {
    /// 値（画像）は表示しない（大きいため）。件数と回数だけ。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lru")
            .field("capacity", &self.capacity)
            .field("len", &self.entries.len())
            .field("counters", &self.counters)
            .finish()
    }
}

impl<K: PartialEq, V> Lru<K, V> {
    /// `capacity` 件（1 以上）のキャッシュ。
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
            counters: CacheCounters::default(),
        }
    }

    /// キーを探す。見つかれば先頭に移してヒットを、なければミスを数える。
    pub(crate) fn lookup(&mut self, key: &K) -> Option<Arc<V>> {
        match self.entries.iter().position(|(k, _)| k == key) {
            Some(i) => {
                self.counters.hits += 1;
                let entry = self.entries.remove(i).expect("位置は範囲内");
                let value = Arc::clone(&entry.1);
                self.entries.push_front(entry);
                Some(value)
            }
            None => {
                self.counters.misses += 1;
                None
            }
        }
    }

    /// 値を先頭に入れる（同じキーがあれば置き換える）。件数を超えたら最も古いものを捨てる。
    pub(crate) fn insert(&mut self, key: K, value: Arc<V>) {
        if let Some(i) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries.remove(i);
        }
        self.entries.push_front((key, value));
        self.entries.truncate(self.capacity);
    }

    /// 件数。
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// 回数。
    pub(crate) fn counters(&self) -> CacheCounters {
        self.counters
    }

    /// 回数を 0 にする。
    pub(crate) fn reset_counters(&mut self) {
        self.counters = CacheCounters::default();
    }

    /// 中身を捨てる（回数は残す）。
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_keeps_the_latest_entries_and_counts() {
        let mut c: Lru<u32, &str> = Lru::new(2);
        assert!(c.lookup(&1).is_none());
        c.insert(1, Arc::new("a"));
        c.insert(2, Arc::new("b"));
        assert_eq!(*c.lookup(&1).unwrap(), "a");
        // 1 を使ったので、3 を入れると 2 が捨てられる。
        c.insert(3, Arc::new("c"));
        assert_eq!(c.len(), 2);
        assert!(c.lookup(&2).is_none());
        assert_eq!(*c.lookup(&3).unwrap(), "c");
        assert_eq!(*c.lookup(&1).unwrap(), "a");
        assert_eq!(c.counters(), CacheCounters { hits: 3, misses: 2 });
        // 同じキーは置き換える。
        c.insert(1, Arc::new("z"));
        assert_eq!(c.len(), 2);
        assert_eq!(*c.lookup(&1).unwrap(), "z");
        c.reset_counters();
        assert_eq!(c.counters(), CacheCounters::default());
        c.clear();
        assert_eq!(c.len(), 0);
        // Debug は値を表示しない。
        let dbg = format!("{c:?}");
        assert!(dbg.contains("len: 0") && !dbg.contains("\"z\""), "{dbg}");
        // 0 件の指定は 1 件として扱う。
        let mut one: Lru<u8, u8> = Lru::new(0);
        one.insert(1, Arc::new(1));
        one.insert(2, Arc::new(2));
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn guide_params_key_uses_bit_patterns() {
        let p = GuideParams {
            long_edge: 512,
            radius_fraction: 0.03,
            epsilon: 0.25,
        };
        let k = GuideParamsKey::from(p);
        assert_eq!(k.long_edge, 512);
        assert_eq!(f64::from_bits(k.radius_fraction_bits), 0.03);
        assert_eq!(f32::from_bits(k.epsilon_bits), 0.25);
        let q = GuideParams { epsilon: 0.5, ..p };
        assert_ne!(GuideParamsKey::from(q), k);
    }
}
