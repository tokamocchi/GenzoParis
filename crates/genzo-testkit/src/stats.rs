//! 統計の計算（平均・パーセンタイル・最大）。画像の差（[`crate::diff`]）と計測（[`crate::bench`]）で
//! 同じ定義を使う。
//!
//! # パーセンタイルの定義
//!
//! **最近接順位法（nearest-rank method）** を使う。n 個の値を小さい順に並べたとき、p パーセンタイルは
//! 順位 `⌈p/100 × n⌉`（1 始まり。0 になる場合は 1）の値とする。補間をしないので、結果は必ず
//! 実際に観測した値のどれかになり、「95 パーセンタイル以下の値が全体の 95% 以上を占める」ことが
//! 保証される（例: 30 回の計測なら順位 29、つまり 2 番目に大きい値）。
//!
//! 線形補間を使う定義（表計算ソフトの `PERCENTILE.INC` など）とは値が異なりうるため、
//! 前回の結果との比較（05 の 1.8 節）では必ずこの関数で計算した値どうしを比べる。

/// 最近接順位法の p パーセンタイル（0 ≤ p ≤ 100）の順位（0 始まりの位置）。
///
/// `n` が 0、または `p` が範囲外・NaN なら `None`。
pub fn nearest_rank_index(n: usize, p: f64) -> Option<usize> {
    if n == 0 || !(0.0..=100.0).contains(&p) {
        return None;
    }
    // p/100 × n の切り上げ。浮動小数点の誤差で整数をわずかに超えて 1 つ上の順位にならないよう、
    // 十分小さな値を引いてから切り上げる（n は高々数億なので、相対誤差 1e-12 で足りる）。
    let exact = p / 100.0 * n as f64;
    let rank = (exact - exact * 1e-12).ceil() as usize;
    Some(rank.clamp(1, n) - 1)
}

/// 最近接順位法の p パーセンタイル。`values` は並べ替える（順序が変わる）。
///
/// NaN を含む場合の結果は [`f64::total_cmp`] の順序による（呼び出し側で除外しておく）。
/// 空、または `p` が範囲外なら `None`。
pub fn percentile_nearest_rank(values: &mut [f64], p: f64) -> Option<f64> {
    let k = nearest_rank_index(values.len(), p)?;
    let (_, v, _) = values.select_nth_unstable_by(k, f64::total_cmp);
    Some(*v)
}

/// 値の列の要約（平均・最小・中央値・95 パーセンタイル・最大）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// 個数。
    pub count: usize,
    /// 平均。
    pub mean: f64,
    /// 最小。
    pub min: f64,
    /// 中央値（最近接順位法の 50 パーセンタイル）。
    pub median: f64,
    /// 95 パーセンタイル（最近接順位法）。
    pub p95: f64,
    /// 最大。
    pub max: f64,
}

impl Summary {
    /// 値の列から計算する。空、または有限でない値を含むなら `None`。
    ///
    /// 平均は先頭から順に f64 で足して計算する（順序が決まっているので結果は再現する）。
    pub fn from_values(values: &[f64]) -> Option<Summary> {
        if values.is_empty() || !values.iter().all(|v| v.is_finite()) {
            return None;
        }
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let at = |p: f64| nearest_rank_index(sorted.len(), p).map(|k| sorted[k]);
        Some(Summary {
            count: values.len(),
            mean,
            min: sorted[0],
            median: at(50.0)?,
            p95: at(95.0)?,
            max: sorted[sorted.len() - 1],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_matches_textbook_examples() {
        // 最近接順位法の教科書的な例（値 15, 20, 35, 40, 50）:
        // 5 → 15、30 → 20、40 → 20、50 → 35、100 → 50。
        let data = [15.0, 20.0, 35.0, 40.0, 50.0];
        for (p, expected) in [
            (5.0, 15.0),
            (30.0, 20.0),
            (40.0, 20.0),
            (50.0, 35.0),
            (100.0, 50.0),
        ] {
            let mut v = data.to_vec();
            assert_eq!(
                percentile_nearest_rank(&mut v, p),
                Some(expected),
                "p = {p}"
            );
        }
        // 0 パーセンタイルは最小値。
        let mut v = data.to_vec();
        assert_eq!(percentile_nearest_rank(&mut v, 0.0), Some(15.0));
    }

    #[test]
    fn p95_of_30_samples_is_second_largest() {
        // 1.8 節の応答時間の最小回数（30 回）では、順位 ⌈28.5⌉ = 29（2 番目に大きい値）。
        assert_eq!(nearest_rank_index(30, 95.0), Some(28));
        // 20 回なら ⌈19⌉ = 19（ちょうど整数のときに 1 つ上にずれない）。
        assert_eq!(nearest_rank_index(20, 95.0), Some(18));
        // 100 回なら 95 番目。
        assert_eq!(nearest_rank_index(100, 95.0), Some(94));
        assert_eq!(nearest_rank_index(1, 95.0), Some(0));
    }

    #[test]
    fn invalid_inputs() {
        assert_eq!(nearest_rank_index(0, 50.0), None);
        assert_eq!(nearest_rank_index(10, -1.0), None);
        assert_eq!(nearest_rank_index(10, 100.5), None);
        assert_eq!(nearest_rank_index(10, f64::NAN), None);
        assert_eq!(Summary::from_values(&[]), None);
        assert_eq!(Summary::from_values(&[1.0, f64::NAN]), None);
        assert_eq!(Summary::from_values(&[1.0, f64::INFINITY]), None);
    }

    #[test]
    fn summary_of_1_to_100() {
        let values: Vec<f64> = (1..=100).rev().map(f64::from).collect();
        let s = Summary::from_values(&values).unwrap();
        assert_eq!(s.count, 100);
        assert_eq!(s.mean, 50.5);
        assert_eq!(s.min, 1.0);
        assert_eq!(s.median, 50.0);
        assert_eq!(s.p95, 95.0);
        assert_eq!(s.max, 100.0);
    }

    #[test]
    fn summary_single_value() {
        let s = Summary::from_values(&[7.0]).unwrap();
        assert_eq!(
            (s.min, s.median, s.p95, s.max, s.mean),
            (7.0, 7.0, 7.0, 7.0, 7.0)
        );
    }
}
