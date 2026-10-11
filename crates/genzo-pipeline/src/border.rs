//! 画像の端の外側の扱い（docs/04_architecture.md の 2.7 節「画像の端の外側は、鏡映（反射）で
//! 拡張する。CPU 版と GPU 版で同じ方法を使う」）。
//!
//! 鏡映は **端の画素を重複させない反射**（`… 2 1 | 0 1 2 … n−2 n−1 | n−2 n−3 …`。OpenCV の
//! `BORDER_REFLECT_101` と同じ形）とする。この形は周期 `2(n − 1)` が偶数なので、**座標の偶奇を
//! 保つ**。ベイヤー配列（CFA）を鏡映で拡張しても、各位置の色の並びが崩れない（端の画素を重複
//! させる反射では、端の外側で色の並びがずれる）。CFA と RGB で同じ定義を使う。
//!
//! GPU 版（WGSL）でも同じ式（[`mirror_index`]）で座標を折り返す。

use crate::image::Roi;

/// 座標 `i` を長さ `n` の範囲 `0..n` に鏡映で折り返す（端の画素を重複させない反射）。
///
/// - `n = 1` なら常に 0。
/// - `n ≥ 2` なら周期 `2(n − 1)` で折り返す。例えば `n = 5` で `−1 → 1`、`−4 → 4`、`5 → 3`、`8 → 0`。
///
/// `n = 0` は呼ばない（範囲が空）。デバッグビルドでは検査する。
#[inline]
pub fn mirror_index(i: i64, n: u32) -> u32 {
    debug_assert!(n > 0, "長さ 0 の範囲には折り返せない");
    if n <= 1 {
        return 0;
    }
    let n = i64::from(n);
    let period = 2 * (n - 1);
    let m = i.rem_euclid(period);
    // m は 0..period なので、結果は 0..n に収まる。
    (if m < n { m } else { period - m }) as u32
}

/// 長さ `n` の範囲で、区間 `[start, start + len)` の座標を鏡映で折り返したときに参照される
/// 範囲 `[lo, hi)`（範囲の内側）。`len = 0` なら `None`。
fn mirror_span(start: i64, len: u32, n: u32) -> Option<(u32, u32)> {
    if len == 0 || n == 0 {
        return None;
    }
    let end = start + i64::from(len);
    if start >= 0 && end <= i64::from(n) {
        return Some((start as u32, end as u32));
    }
    // 1 周期分以上あれば全体を参照する。
    let period = 2 * (i64::from(n) - 1).max(1);
    if i64::from(len) >= period {
        return Some((0, n));
    }
    let mut lo = u32::MAX;
    let mut hi = 0;
    for i in start..end {
        let m = mirror_index(i, n);
        lo = lo.min(m);
        hi = hi.max(m + 1);
    }
    Some((lo, hi))
}

/// 幅 `width`・高さ `height` の画像で、範囲 `roi`（画像の外にはみ出してよい）の画素を鏡映で
/// 折り返したときに参照される画素をすべて含む、画像の内側の最小の範囲。
///
/// `roi` が空か、画像の寸法が 0 なら `None`。
pub fn mirror_cover(roi: Roi, width: u32, height: u32) -> Option<Roi> {
    let (x0, x1) = mirror_span(i64::from(roi.x), roi.width, width)?;
    let (y0, y1) = mirror_span(i64::from(roi.y), roi.height, height)?;
    Some(Roi::new(x0 as i32, y0 as i32, x1 - x0, y1 - y0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflect_101_examples() {
        // n = 5: … 2 1 | 0 1 2 3 4 | 3 2 1 0 1 …
        let expect = [
            (-5, 3),
            (-4, 4),
            (-3, 3),
            (-2, 2),
            (-1, 1),
            (0, 0),
            (4, 4),
            (5, 3),
            (6, 2),
            (8, 0),
            (9, 1),
            (12, 4),
        ];
        for (i, e) in expect {
            assert_eq!(mirror_index(i, 5), e, "i = {i}");
        }
    }

    #[test]
    fn short_ranges() {
        for i in -7..7 {
            assert_eq!(mirror_index(i, 1), 0);
        }
        // n = 2: 周期 2。
        assert_eq!(mirror_index(-1, 2), 1);
        assert_eq!(mirror_index(-2, 2), 0);
        assert_eq!(mirror_index(2, 2), 0);
        assert_eq!(mirror_index(3, 2), 1);
    }

    #[test]
    fn parity_is_preserved_for_n_at_least_2() {
        // CFA の色の並びを保つ（偶奇が変わらない）。
        for n in 2..9u32 {
            for i in -40..40i64 {
                let m = mirror_index(i, n);
                assert!(m < n);
                assert_eq!(
                    i64::from(m).rem_euclid(2),
                    i.rem_euclid(2),
                    "n = {n}, i = {i}"
                );
            }
        }
    }

    #[test]
    fn inside_is_identity_and_extreme_values_do_not_overflow() {
        for i in 0..10 {
            assert_eq!(mirror_index(i, 10), i as u32);
        }
        assert!(mirror_index(i64::MIN / 2, 7) < 7);
        assert!(mirror_index(i64::MAX / 2, 7) < 7);
        assert!(mirror_index(-1, u32::MAX) < u32::MAX);
    }

    #[test]
    fn mirror_cover_examples() {
        // 内側ならそのまま。
        let r = Roi::new(2, 3, 4, 5);
        assert_eq!(mirror_cover(r, 10, 10), Some(r));
        // 左上にはみ出す: x ∈ [−3, 2) → {3, 2, 1, 0, 1} → [0, 4)。
        assert_eq!(
            mirror_cover(Roi::new(-3, -1, 5, 3), 10, 10),
            Some(Roi::new(0, 0, 4, 2))
        );
        // 右下にはみ出す: x ∈ [8, 12) → {8, 9, 8, 7} → [7, 10)。
        assert_eq!(
            mirror_cover(Roi::new(8, 9, 4, 1), 10, 10),
            Some(Roi::new(7, 9, 3, 1))
        );
        // 1 周期以上なら全体。
        assert_eq!(
            mirror_cover(Roi::new(-50, 0, 100, 1), 10, 10),
            Some(Roi::new(0, 0, 10, 1))
        );
        assert_eq!(mirror_cover(Roi::new(0, 0, 0, 3), 10, 10), None);
        assert_eq!(mirror_cover(Roi::new(0, 0, 3, 3), 0, 10), None);
    }

    #[test]
    fn mirror_cover_contains_every_mirrored_pixel() {
        for (x, w) in [(-7, 3), (-2, 9), (5, 8), (-1, 1), (11, 2)] {
            let roi = Roi::new(x, x, w, w);
            let c = mirror_cover(roi, 9, 9).unwrap();
            for i in i64::from(x)..i64::from(x) + i64::from(w) {
                let m = mirror_index(i, 9) as i32;
                assert!(c.x <= m && m < c.x + c.width as i32, "{roi:?} {c:?} {i}");
            }
        }
    }
}
