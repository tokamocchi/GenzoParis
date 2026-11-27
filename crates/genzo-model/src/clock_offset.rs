//! 機材ごとの時計のずれの推定（ORG-02）: 同じ時間帯に 2 つの機材で撮った写真・動画の撮影日時から、片方の機材の
//! 時計が基準の機材からどれだけずれているかを推定する（[`estimate_clock_shift`]）。
//!
//! # 考え方
//!
//! 同じ人が 2 つの機材（カメラとアクションカメラなど）を持ち歩くと、どちらも同じ場面で使うので、時計が合って
//! いれば、片方の機材で撮った時刻の近くに、もう片方で撮った時刻がある。時計がずれていると（タイムゾーンの設定の
//! 誤り・時計を合わせていない）、そのずれの分だけ動かしたときに、近くに相手のある写真が最も多くなる。
//!
//! 1. 対象の機材の時刻 `b` ごとに、基準の機材の時刻 `a` との差 `a − b`（±[`ClockParams::max_shift_seconds`] の
//!    範囲）を、[`BIN_SECONDS`] 秒の幅の度数にする（数が多いときは、決まった間隔で間引く）。
//! 2. 度数の多い差を候補にし、候補ごとに「その差だけ動かしたとき、±[`ClockParams::window_seconds`] 以内に基準の
//!    機材の時刻がある `b` の数」を数える（1 つの `b` は 1 回だけ数える。連写の多い場面に引きずられないため）。
//! 3. 最も多い候補を選び、近くにあった相手との差の中央値で細かく合わせる。
//!
//! # 限界（提案にとどめる理由）
//!
//! - **2 つの機材で同じ瞬間を撮っているわけではない**ので、求まるのはおよその値（数十秒〜数分の誤差）。秒まで
//!   合わせるには、同じ時計を写した写真を基準に合わせる（撮影日時の一括修正 LIB-16 の「基準の写真に合わせる」）。
//! - 同じ時間帯に使っていない機材どうしでは求まらない（近くに相手のある写真が増えない）。
//! - 5 分より小さいずれは提案しない（[`MIN_SUGGESTED_SHIFT_SECONDS`]。合っているのと区別できないため）。
//! - ずれが 15 分の倍数に近いとき（タイムゾーン・夏時間の設定の誤りに多い）は、丸めた値も返す
//!   （[`ClockEstimate::rounded_seconds`]）。
//!
//! しきい値・幅はどれも **仮置き**（合成のデータで決めた。実際の撮影のデータで見直す）。

/// 度数の幅（秒）。
pub const BIN_SECONDS: i64 = 60;

/// 対象の機材の時刻を間引いた後の数の上限。
pub const MAX_SAMPLES: usize = 4_000;

/// 1 つの時刻について、差を数える基準の機材の時刻の数の上限（多いときは間引く）。
const MAX_NEIGHBORS: usize = 500;

/// 調べる候補の数（度数の多い順）。
const MAX_CANDIDATES: usize = 40;

/// 丸めた値を返すときの、15 分の倍数との差の上限（秒）。
pub const ROUND_TOLERANCE_SECONDS: i64 = 180;

/// 推定の設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockParams {
    /// 調べるずれの範囲（秒。± この値）。
    pub max_shift_seconds: i64,
    /// 「近くに相手がある」とみなす時刻の差（秒）。
    pub window_seconds: i64,
    /// 推定に要る、対象の機材の時刻の数の下限。
    pub min_items: usize,
}

impl Default for ClockParams {
    fn default() -> Self {
        Self {
            // ±14 時間（タイムゾーンの差の最大は 26 時間だが、設定の誤りはふつう ±14 時間に収まる）。
            max_shift_seconds: 14 * 3600,
            window_seconds: 120,
            min_items: 10,
        }
    }
}

/// 推定の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockEstimate {
    /// 対象の機材の時刻に足すと、基準の機材に合う秒数（正なら、対象の機材の時計が遅れている）。
    pub shift_seconds: i64,
    /// ずれが 15 分の倍数に近いときの、丸めた値（タイムゾーン・夏時間の設定の誤りの見込み）。
    pub rounded_seconds: Option<i64>,
    /// 調べた時刻の数（間引いた後）。
    pub samples: u64,
    /// 今のまま（ずらさない）で、近くに基準の機材の時刻がある数。
    pub matched_before: u64,
    /// `shift_seconds` だけずらしたときに、近くに基準の機材の時刻がある数。
    pub matched_after: u64,
    /// 提案できるだけの根拠があるか（[`is_confident`] の doc）。
    pub confident: bool,
}

/// `sorted` の中に、`[lo, hi]` の値があるか。
fn any_in(sorted: &[i64], lo: i64, hi: i64) -> bool {
    let i = sorted.partition_point(|v| *v < lo);
    sorted.get(i).is_some_and(|v| *v <= hi)
}

/// `shift` だけ動かしたときに、±`window` 以内に基準の時刻がある数。
fn matched(reference: &[i64], samples: &[i64], shift: i64, window: i64) -> u64 {
    samples
        .iter()
        .filter(|b| any_in(reference, **b + shift - window, **b + shift + window))
        .count() as u64
}

/// 提案するずれの下限（秒。**仮置き**: 5 分）。2 つの機材で同じ瞬間を撮っているわけではないので、これより小さい
/// ずれは、この方法では合っているのと区別できない（基準の写真に合わせる方法で直す）。
pub const MIN_SUGGESTED_SHIFT_SECONDS: i64 = 300;

/// 提案できるだけの根拠があるか: ずらした後に近くに相手のある数が、調べた数の 3 割以上かつ 5 以上で、ずらす前より
/// 「調べた数の 2 割（最低 5）」以上増える。ずれが [`MIN_SUGGESTED_SHIFT_SECONDS`]（と `window`）未満なら、今のままで
/// 合っているとみなして提案しない。
fn is_confident(shift: i64, before: u64, after: u64, samples: u64, window: i64) -> bool {
    let gain = (samples / 5).max(5);
    shift.abs() >= window.max(MIN_SUGGESTED_SHIFT_SECONDS)
        && after >= 5
        && after * 10 >= samples * 3
        && after >= before + gain
}

/// 15 分の倍数に近ければ、その値。
fn rounded(shift: i64) -> Option<i64> {
    const STEP: i64 = 15 * 60;
    let nearest = (shift as f64 / STEP as f64).round() as i64 * STEP;
    (nearest != 0 && nearest != shift && (shift - nearest).abs() <= ROUND_TOLERANCE_SECONDS)
        .then_some(nearest)
}

/// 対象の機材の時計のずれを推定する（モジュールの doc）。`reference`・`other` は撮影日時（UTC の Unix 時刻の秒。
/// **昇順**）。どちらかの数が [`ClockParams::min_items`] に足りない・近い時刻の組がまったくないなら `None`。
pub fn estimate_clock_shift(
    reference: &[i64],
    other: &[i64],
    params: &ClockParams,
) -> Option<ClockEstimate> {
    if reference.len() < params.min_items || other.len() < params.min_items {
        return None;
    }
    debug_assert!(reference.is_sorted() && other.is_sorted());
    // 対象の機材の時刻を、決まった間隔で間引く。
    let stride = other.len().div_ceil(MAX_SAMPLES).max(1);
    let samples: Vec<i64> = other.iter().copied().step_by(stride).collect();
    let max = params.max_shift_seconds;

    // 1. 差の度数。
    let mut bins: std::collections::HashMap<i64, u32> = std::collections::HashMap::new();
    for &b in &samples {
        let lo = reference.partition_point(|a| *a < b - max);
        let hi = reference.partition_point(|a| *a <= b + max);
        let near = &reference[lo..hi];
        let step = near.len().div_ceil(MAX_NEIGHBORS).max(1);
        for &a in near.iter().step_by(step) {
            *bins.entry((a - b).div_euclid(BIN_SECONDS)).or_default() += 1;
        }
    }
    if bins.is_empty() {
        return None;
    }
    // 2. 隣の階級を足して滑らかにした度数の多い順に、候補にする（同じ度数なら、ずれの小さいほう）。
    let mut scored: Vec<(u32, i64)> = bins
        .keys()
        .map(|&k| {
            let sum = (-1..=1)
                .map(|d| bins.get(&(k + d)).copied().unwrap_or(0))
                .sum();
            (sum, k)
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.abs().cmp(&b.1.abs())));
    let window = params.window_seconds;
    let before = matched(reference, &samples, 0, window);
    let mut best = (before, 0i64);
    for &(_, k) in scored.iter().take(MAX_CANDIDATES) {
        let shift = k * BIN_SECONDS + BIN_SECONDS / 2;
        let n = matched(reference, &samples, shift, window);
        if n > best.0 || (n == best.0 && shift.abs() < best.1.abs()) {
            best = (n, shift);
        }
    }
    // 3. 近くにあった相手との差の中央値で、細かく合わせる（よくなったときだけ使う）。
    let (mut after, mut shift) = best;
    let mut diffs: Vec<i64> = samples
        .iter()
        .filter_map(|&b| {
            let t = b + shift;
            let i = reference.partition_point(|a| *a < t);
            let nearest = [i.checked_sub(1), Some(i)]
                .into_iter()
                .flatten()
                .filter_map(|j| reference.get(j))
                .min_by_key(|a| (**a - t).abs())?;
            ((nearest - t).abs() <= window).then_some(nearest - b)
        })
        .collect();
    if !diffs.is_empty() {
        diffs.sort_unstable();
        let median = diffs[diffs.len() / 2];
        let n = matched(reference, &samples, median, window);
        if n >= after {
            (after, shift) = (n, median);
        }
    }
    let total = samples.len() as u64;
    Some(ClockEstimate {
        shift_seconds: shift,
        rounded_seconds: rounded(shift),
        samples: total,
        matched_before: before,
        matched_after: after,
        confident: is_confident(shift, before, after, total, window),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 決まった擬似乱数（xorshift）。
    struct Rng(u64);

    impl Rng {
        fn next(&mut self, n: u64) -> i64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n) as i64
        }
    }

    /// 2 つの機材で同じ場面（`sessions` 回）を撮った時刻。場面の中で、それぞれ数枚〜十数枚をばらばらに撮る。
    /// `offset` は、対象の機材の時計の遅れ（基準 − 対象）。
    fn two_devices(sessions: usize, offset: i64, seed: u64) -> (Vec<i64>, Vec<i64>) {
        let mut rng = Rng(seed | 1);
        let mut a = Vec::new();
        let mut b = Vec::new();
        let mut t = 1_700_000_000i64;
        for _ in 0..sessions {
            // 場面の間は 1〜30 時間。場面は 5〜40 分。
            t += 3600 + rng.next(29 * 3600);
            let len = 300 + rng.next(2100);
            for _ in 0..(4 + rng.next(12)) {
                a.push(t + rng.next(len as u64));
            }
            for _ in 0..(3 + rng.next(8)) {
                b.push(t + rng.next(len as u64) - offset);
            }
        }
        a.sort_unstable();
        b.sort_unstable();
        (a, b)
    }

    #[test]
    fn a_timezone_sized_offset_is_found_and_rounded() {
        // 対象の機材の時計が 9 時間遅れている（UTC のまま記録した）。
        let (a, b) = two_devices(40, 9 * 3600, 7);
        let e = estimate_clock_shift(&a, &b, &ClockParams::default()).unwrap();
        assert!(e.confident, "{e:?}");
        assert!((e.shift_seconds - 9 * 3600).abs() <= 180, "{e:?}");
        assert_eq!(
            e.rounded_seconds,
            Some(9 * 3600).filter(|r| *r != e.shift_seconds)
        );
        assert!(e.matched_after > e.matched_before * 3, "{e:?}");
        // 逆向き（進んでいる）も同じ。
        let (a, b) = two_devices(40, -(5 * 3600 + 1800), 11);
        let e = estimate_clock_shift(&a, &b, &ClockParams::default()).unwrap();
        assert!(e.confident, "{e:?}");
        assert!((e.shift_seconds + 5 * 3600 + 1800).abs() <= 180, "{e:?}");
    }

    #[test]
    fn a_drift_of_some_minutes_is_found_without_rounding() {
        // 23 分 20 秒の遅れ（15 分の倍数から 8 分 20 秒離れている）。
        let (a, b) = two_devices(60, 1400, 3);
        let e = estimate_clock_shift(&a, &b, &ClockParams::default()).unwrap();
        assert!(e.confident, "{e:?}");
        assert!((e.shift_seconds - 1400).abs() <= 240, "{e:?}");
        assert_eq!(e.rounded_seconds, None);
    }

    #[test]
    fn aligned_devices_get_no_suggestion() {
        let (a, b) = two_devices(40, 0, 5);
        let e = estimate_clock_shift(&a, &b, &ClockParams::default()).unwrap();
        assert!(!e.confident, "{e:?}");
        assert!(e.shift_seconds.abs() < 600, "{e:?}");
        assert!(e.matched_before * 10 >= e.samples * 5, "{e:?}");
    }

    #[test]
    fn unrelated_devices_get_no_suggestion() {
        // 別の時期に使った機材（1 年ずれている）: 近い時刻の組がない。
        let (a, _) = two_devices(30, 0, 13);
        let (_, b) = two_devices(30, 365 * 86_400, 17);
        let e = estimate_clock_shift(&a, &b, &ClockParams::default());
        assert!(e.is_none_or(|e| !e.confident), "{e:?}");
        // 数が足りない。
        assert_eq!(
            estimate_clock_shift(&a[..5], &b, &ClockParams::default()),
            None
        );
        assert_eq!(estimate_clock_shift(&a, &[], &ClockParams::default()), None);
    }

    #[test]
    fn large_inputs_are_sampled() {
        // 連写の多い機材でも、間引いて同じ答えになる。
        let (a, b) = two_devices(300, 3 * 3600, 23);
        let mut dense: Vec<i64> = b.iter().flat_map(|t| (0..20).map(move |i| t + i)).collect();
        dense.sort_unstable();
        assert!(dense.len() > MAX_SAMPLES);
        let e = estimate_clock_shift(&a, &dense, &ClockParams::default()).unwrap();
        assert!(e.samples as usize <= MAX_SAMPLES);
        assert!(
            e.confident && (e.shift_seconds - 3 * 3600).abs() <= 180,
            "{e:?}"
        );
    }

    #[test]
    fn rounding_only_applies_near_quarter_hours() {
        assert_eq!(rounded(9 * 3600 + 40), Some(9 * 3600));
        assert_eq!(rounded(-(3600 + 170)), Some(-3600));
        assert_eq!(rounded(9 * 3600), None, "ちょうどなら丸めた値は要らない");
        assert_eq!(rounded(1400), None);
        assert_eq!(rounded(100), None, "0 には丸めない");
    }
}
