//! CPU 版と GPU 版の差の許容誤差と比べ方（02 の IQ-07a、docs/04_architecture.md の 2.3 節「ステージ
//! ごとに許容誤差を定め」）。
//!
//! # 許容誤差（すべて**仮置き**）
//!
//! 値ごとに `|GPU − CPU| ≤ abs + rel × |CPU|` を満たせば許容する（[`StageTolerance`]・[`compare`]）。
//! RGB の画像は、`|CPU|` の代わりに **その画素の 3 つの値の絶対値の最大** を使う（[`compare_pixels`]）。
//! 色を混ぜるステージ（行列・OKLab・色域の圧縮）では、小さいチャンネルの誤差が、同じ画素の大きいチャンネルの
//! 丸めの誤差（桁落ち）で決まるため（例: ステージ 13 で `[0.0031, 16, 1.5e−5]` の R の差は約 1e−5 で、
//! 値ごとの相対誤差では 1e−4 を超えるが、画素の大きさ 16 に対しては 1e−6 程度）。下の表の値は、
//! このコンテナの Mesa llvmpipe（Vulkan）で測った差に余裕を持たせて置いたもので、実機（RTX 3080・M1）の
//! 差は **PoC-3 で測って決める**（WGSL は演算の結合・融合を許し、`exp2`・`log2`・`pow` の精度にも
//! 数 ULP の幅があるため、GPU ごとに差が違う）。
//!
//! | ステージ | abs | rel | 差が出る主な理由 |
//! |---|---|---|---|
//! | `finish.geometry` | 1e−4 | 1e−4 | 座標の積和（融合）・16 個の重み付きの和。座標の 1 ULP の差（座標 100 px で約 8e−6 px、7000 px で約 5e−4 px）が、くっきりした輪郭では値の差になる |
//! | `finish.exposure` | 0 | 1e−6 | 掛け算だけ（丸めは同じはず） |
//! | `finish.contrast` | 1e−6 | 2e−5 | `log2`・`exp2` の精度 |
//! | `finish.tone` | 1e−6 | 1e−4 | `exp2` を何度も使う（シグモイド）、ガイドの双線形 |
//! | `finish.color` | 1e−5 | 1e−4 | 立方根（WGSL にないので `pow` とニュートン法）、OKLab の行列の打ち消し |
//! | `finish.scene_to_display` | 1e−6 | 1e−5 | 割り算の精度（2.5 ULP） |
//! | `finish.tone_curve` | 1e−5 | 1e−4 | `pow`（IEC 61966-2-1）、LUT の位置の差 |
//! | `output.display` | 1e−4 | 0 | 色域の圧縮の `pow`、3D LUT の位置の差（0〜1 の値） |
//! | `output.export.*`・`output.cache.*` | 1e−4 | 0 | 同上（0〜1 の値） |
//! | `sensor.normalize`・`sensor.demosaic.half2x2` | 1e−7 | 1e−6 | 引き算と掛け算だけ |
//! | `sensor.white_balance` | 1e−7 | 1e−6 | 掛け算だけ |
//! | `sensor.color_matrix` | 1e−6 | 1e−5 | 行列の積和 |
//! | 上にない ID（新しく登録したステージ） | 1e−5 | 1e−4 | [`DEFAULT_STAGE_TOLERANCE`] |
//!
//! 最終出力（17b の符号化した値）は **8bit 換算で ±1 以内**（IQ-07a の目安。[`FINAL_OUTPUT_MAX_DIFF_8BIT`]）。
//! NaN・無限大は出力しない（どのステージも出力で 0 に置き換える）。
//!
//! 極端な値（1e30 など、途中で無限大になる値）は、CPU 版と GPU 版で途中の無限大の扱いが違いうる
//! （WGSL は NaN・無限大の結果を規定しない）ので、一致は比べず、有限であることだけを確かめる。
//! B3 の契約（2.6 節: 輝度は 0〜1）の外の入力（輝度が 1 を大きく超える値）も比べない（ステージ 16 の
//! 比率の計算 `y2 + r × (c − y)` で桁落ちが起き、`pow` の数 ULP の差が許容誤差を大きく超える）。

/// 1 つのステージの許容誤差（`|GPU − CPU| ≤ abs + rel × |CPU|`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageTolerance {
    /// 絶対誤差。
    pub abs: f32,
    /// 相対誤差。
    pub rel: f32,
}

impl StageTolerance {
    /// 作る。
    pub const fn new(abs: f32, rel: f32) -> Self {
        Self { abs, rel }
    }

    /// 許容する差の上限。
    pub fn bound(&self, expected: f32) -> f32 {
        self.abs + self.rel * expected.abs()
    }
}

/// 表にないステージの許容誤差（**仮置き**。モジュールの doc の表）。
pub const DEFAULT_STAGE_TOLERANCE: StageTolerance = StageTolerance::new(1e-5, 1e-4);

/// 最終出力の 8bit 換算の差の上限（IQ-07a の「8bit 換算で ±1 以内」）。
pub const FINAL_OUTPUT_MAX_DIFF_8BIT: f64 = 1.0;

/// ステージの ID の許容誤差（**仮置き**。モジュールの doc の表）。
pub fn stage_tolerance(stage_id: &str) -> StageTolerance {
    match stage_id {
        "finish.geometry" => StageTolerance::new(1e-4, 1e-4),
        "finish.exposure" => StageTolerance::new(0.0, 1e-6),
        "finish.contrast" => StageTolerance::new(1e-6, 2e-5),
        "finish.tone" => StageTolerance::new(1e-6, 1e-4),
        "finish.color" => StageTolerance::new(1e-5, 1e-4),
        "finish.scene_to_display" => StageTolerance::new(1e-6, 1e-5),
        "finish.tone_curve" => StageTolerance::new(1e-5, 1e-4),
        "output.display" => StageTolerance::new(1e-4, 0.0),
        id if id.starts_with("output.export.") || id.starts_with("output.cache.") => {
            StageTolerance::new(1e-4, 0.0)
        }
        "sensor.normalize" | "sensor.demosaic.half2x2" | "sensor.white_balance" => {
            StageTolerance::new(1e-7, 1e-6)
        }
        "sensor.color_matrix" => StageTolerance::new(1e-6, 1e-5),
        _ => DEFAULT_STAGE_TOLERANCE,
    }
}

/// CPU 版と GPU 版の値の差の統計。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DiffReport {
    /// 比べた値の数。
    pub values: usize,
    /// 差の絶対値の最大。
    pub max_abs: f32,
    /// 「差 ÷ 許容する差」の最大（1 以下なら許容誤差の内）。
    pub max_ratio: f32,
    /// 「差 ÷ 許容する差」が最大の値の番号。
    pub worst_index: usize,
    /// その値（CPU 版）。
    pub worst_expected: f32,
    /// その値（GPU 版）。
    pub worst_actual: f32,
    /// GPU 版の NaN・無限大の数。
    pub non_finite: usize,
    /// 長さが違ったか。
    pub length_mismatch: bool,
}

impl DiffReport {
    /// 許容誤差の内で、NaN・無限大がなく、長さが同じか。
    pub fn passed(&self) -> bool {
        self.max_ratio <= 1.0 && self.non_finite == 0 && !self.length_mismatch
    }
}

impl std::fmt::Display for DiffReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "値 {} 個: 差の最大 {:.3e}、許容に対する比の最大 {:.3}（番号 {}: CPU {} / GPU {}）、NaN・無限大 {} 個{}",
            self.values,
            self.max_abs,
            self.max_ratio,
            self.worst_index,
            self.worst_expected,
            self.worst_actual,
            self.non_finite,
            if self.length_mismatch {
                "、長さが違う"
            } else {
                ""
            }
        )
    }
}

/// `expected`（CPU 版）と `actual`（GPU 版）を許容誤差 `tol` で値ごとに比べる（CFA など 1 チャンネルの値）。
pub fn compare(expected: &[f32], actual: &[f32], tol: StageTolerance) -> DiffReport {
    compare_by(expected, actual, |_, e| tol.bound(e))
}

/// RGB の画像を許容誤差 `tol` で比べる。値ごとの上限は `abs + rel × m`（`m` は CPU 版のその画素の 3 つの値の
/// 絶対値の最大。モジュールの doc）。[`DiffReport::worst_index`] は値の番号（画素の番号 × 3 + チャンネル）。
pub fn compare_pixels(
    expected: &[[f32; 3]],
    actual: &[[f32; 3]],
    tol: StageTolerance,
) -> DiffReport {
    let scale: Vec<f32> = expected
        .iter()
        .map(|p| p.iter().fold(0.0f32, |m, v| m.max(v.abs())))
        .collect();
    compare_by(expected.as_flattened(), actual.as_flattened(), |i, _| {
        tol.bound(scale[i / 3])
    })
}

/// 値ごとに比べる（`bound(番号, CPU 版の値)` が許容する差の上限）。
fn compare_by(expected: &[f32], actual: &[f32], bound: impl Fn(usize, f32) -> f32) -> DiffReport {
    let mut r = DiffReport {
        values: expected.len().min(actual.len()),
        length_mismatch: expected.len() != actual.len(),
        ..Default::default()
    };
    let mut worst = f32::NEG_INFINITY;
    for (i, (&e, &a)) in expected.iter().zip(actual).enumerate() {
        if !a.is_finite() {
            r.non_finite += 1;
            continue;
        }
        let d = (a - e).abs();
        r.max_abs = r.max_abs.max(d);
        let bound = bound(i, e);
        let ratio = if d == 0.0 {
            0.0
        } else if bound > 0.0 {
            d / bound
        } else {
            f32::INFINITY
        };
        if ratio > worst {
            worst = ratio;
            r.worst_index = i;
            r.worst_expected = e;
            r.worst_actual = a;
        }
    }
    r.max_ratio = worst.max(0.0);
    r
}

/// 0〜1 の符号化した値の 8bit 換算の差の最大（`|a − e| × 255`。IQ-07a の最終出力の比較）。
pub fn max_diff_8bit(expected: &[f32], actual: &[f32]) -> f64 {
    expected
        .iter()
        .zip(actual)
        .map(|(&e, &a)| (f64::from(a) - f64::from(e)).abs() * 255.0)
        .fold(0.0, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerances_by_id() {
        assert_eq!(stage_tolerance("finish.exposure").abs, 0.0);
        assert_eq!(
            stage_tolerance("output.export.srgb"),
            stage_tolerance("output.cache.display_p3")
        );
        assert_eq!(stage_tolerance("new.stage"), DEFAULT_STAGE_TOLERANCE);
    }

    #[test]
    fn compare_reports_the_worst_value() {
        let tol = StageTolerance::new(0.01, 0.0);
        let r = compare(&[0.0, 1.0, 2.0], &[0.005, 1.02, 2.0], tol);
        assert!(!r.passed());
        assert_eq!(r.worst_index, 1);
        assert!((r.max_ratio - 2.0).abs() < 1e-3, "{r}");
        assert!((r.max_abs - 0.02).abs() < 1e-6);
        let ok = compare(&[0.0, 1.0], &[0.005, 1.0], tol);
        assert!(ok.passed(), "{ok}");
        let nan = compare(&[0.0], &[f32::NAN], tol);
        assert_eq!(nan.non_finite, 1);
        assert!(!nan.passed());
        assert!(compare(&[0.0], &[0.0, 1.0], tol).length_mismatch);
        // 許容 0 で同じ値は通る。
        assert!(compare(&[3.0], &[3.0], StageTolerance::new(0.0, 0.0)).passed());
        assert!(!compare(&[3.0], &[3.000001], StageTolerance::new(0.0, 0.0)).passed());
    }

    #[test]
    fn pixels_are_compared_relative_to_their_largest_value() {
        let tol = StageTolerance::new(1e-5, 1e-4);
        // 小さい R の差 1.1e−5 は、値ごとでは許容を超え、画素の大きさ（16）に対しては許容の内。
        let cpu = [[0.003_138_6f32, 15.999_994, 1.621_246_3e-5]];
        let gpu = [[0.003_127_157_7f32, 16.000_006, 1.525_878_9e-5]];
        assert!(!compare(cpu.as_flattened(), gpu.as_flattened(), tol).passed());
        let r = compare_pixels(&cpu, &gpu, tol);
        assert!(r.passed(), "{r}");
        assert_eq!(r.values, 3);
        // 画素の大きさに対しても大きい差は許容しない（番号は値の番号）。
        let bad = compare_pixels(
            &[[0.5, 0.0, 0.0], [1.0, 2.0, 3.0]],
            &[[0.5, 0.0, 0.0], [1.0, 2.01, 3.0]],
            tol,
        );
        assert!(!bad.passed());
        assert_eq!(bad.worst_index, 4);
        // NaN・無限大と長さの違いは値ごとの比較と同じに扱う。
        assert_eq!(
            compare_pixels(&[[0.0; 3]], &[[f32::NAN, 0.0, 0.0]], tol).non_finite,
            1
        );
        assert!(compare_pixels(&[[0.0; 3]], &[[0.0; 3], [0.0; 3]], tol).length_mismatch);
    }

    #[test]
    fn diff_8bit() {
        assert_eq!(max_diff_8bit(&[0.5, 0.2], &[0.5, 0.2]), 0.0);
        let d = max_diff_8bit(&[0.0, 0.5], &[1.0 / 255.0, 0.5]);
        assert!((d - 1.0).abs() < 1e-6, "{d}");
    }
}
