//! NaN・無限大の置き換え（docs/04_architecture.md の 2.6 節「NaN・無限大は各ステージの出力で
//! 検出し、0 に置き換える」、2.3 節「NaN・無限大・負の値・ゼロ付近の値の扱いを CPU 版と GPU 版で
//! そろえる」）。
//!
//! 置き換えた値の数を返す。2.6 節の「デバッグ用のビルドでは件数を記録する」は、呼び出し側
//! （[`crate::stage::run_stage_cpu`] と [`crate::sensor`] の結果の `non_finite_replaced`）が
//! 件数を集計して行う（数えるのはリリースビルドでも行う。分岐 1 つで安いため）。
//!
//! 対数を取る前の下限は [`LOG_FLOOR`] と [`safe_log2`] を使う（2.3 節・2.7 節のガイド）。

/// 対数を取る前に設ける下限（2^−16）。2.7 節のガイドの定義 `log2(max(Y, 2^−16))` と同じ値。
pub const LOG_FLOOR: f32 = 1.0 / 65536.0;

/// 有限ならそのまま、NaN・無限大なら 0。
#[inline]
pub fn sanitize(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// 値の列の NaN・無限大を 0 に置き換え、置き換えた数を返す。
pub fn sanitize_slice(values: &mut [f32]) -> u64 {
    let mut count = 0;
    for v in values {
        if !v.is_finite() {
            *v = 0.0;
            count += 1;
        }
    }
    count
}

/// RGB の画素の列の NaN・無限大を 0 に置き換え、置き換えた値（チャンネル単位）の数を返す。
pub fn sanitize_rgb(pixels: &mut [[f32; 3]]) -> u64 {
    sanitize_slice(pixels.as_flattened_mut())
}

/// `log2(max(v, LOG_FLOOR))`。NaN は下限として扱う（`f32::max` は NaN でない方を返す）。
///
/// 注意: `log2` の結果は数学ライブラリ（OS・GPU）によって最下位のビットが変わりうるので、
/// CPU 版と GPU 版の比較には許容誤差を持たせる。
#[inline]
pub fn safe_log2(v: f32) -> f32 {
    v.max(LOG_FLOOR).log2()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_values_become_zero() {
        assert_eq!(sanitize(1.5), 1.5);
        assert_eq!(sanitize(-2.0), -2.0);
        assert_eq!(sanitize(f32::NAN), 0.0);
        assert_eq!(sanitize(f32::INFINITY), 0.0);
        assert_eq!(sanitize(f32::NEG_INFINITY), 0.0);
        // 非正規化数・負のゼロはそのまま（有限）。
        assert_eq!(sanitize(f32::MIN_POSITIVE / 4.0), f32::MIN_POSITIVE / 4.0);
        assert!(sanitize(-0.0).is_sign_negative());
    }

    #[test]
    fn slices_report_the_count() {
        let mut v = [1.0, f32::NAN, 3.0, f32::INFINITY, f32::NEG_INFINITY];
        assert_eq!(sanitize_slice(&mut v), 3);
        assert_eq!(v, [1.0, 0.0, 3.0, 0.0, 0.0]);
        let mut p = [[f32::NAN, 0.5, 1.0], [2.0, 3.0, f32::INFINITY]];
        assert_eq!(sanitize_rgb(&mut p), 2);
        assert_eq!(p, [[0.0, 0.5, 1.0], [2.0, 3.0, 0.0]]);
        assert_eq!(sanitize_rgb(&mut p), 0);
    }

    #[test]
    fn log_has_a_floor() {
        // log2 は OS の数学ライブラリによって最下位のビットが変わりうるので、許容誤差で比べる。
        let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(close(safe_log2(0.0), -16.0));
        assert!(close(safe_log2(-1.0), -16.0));
        assert!(close(safe_log2(f32::NAN), -16.0));
        assert!(close(safe_log2(LOG_FLOOR / 2.0), -16.0));
        // 下限より大きい値はそのまま log2。
        assert!(close(safe_log2(0.18), 0.18f32.log2()));
        assert!(close(safe_log2(1.0), 0.0));
        assert!(safe_log2(LOG_FLOOR * 4.0) > -15.0);
    }
}
