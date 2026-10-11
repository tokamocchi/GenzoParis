//! 合成のテスト画像（シーンリニア）。
//!
//! 公開リポジトリに入れてよい（人物・個人情報を含まない）テストデータを、コードから決定的に作る
//! （05 の 3 章「注意：サンプルデータの扱い」）。値はシーンリニア（04 の 2.6 節の B1・B2。
//! 0 以上、1 を超えてよい）で、無彩色の画像は R = G = B なので色空間によらない。
//!
//! | 関数 | 用途 |
//! |---|---|
//! | [`gray_steps`]・[`ev_levels`] | グレーの階段（トーン・露光量） |
//! | [`crate::colorchecker::color_checker_image`] | 色のパッチ（色の正確さ。PoC-4） |
//! | [`zone_plate`]・[`line_pattern`] | 細い線・ゾーンプレート（デモザイクの評価。PoC-3） |
//! | [`step_edge`] | 明暗差の大きいエッジ（ハローの評価。PoC-5） |
//! | [`horizontal_gradient`]・[`ev_ramp`] | なめらかなグラデーション（トーンの段差・バンディング） |
//!
//! 画素 (x, y) の値は画素の中心 (x + 0.5, y + 0.5) の位置で決める（アンチエイリアスはしない）。

use crate::image::{FloatImage, ImageError};

/// 中間のグレー（反射率 18%）のシーンリニアの値。露光量 0 EV の基準。
pub const MID_GRAY: f32 = 0.18;

/// プレビューの解像度（幅、高さ）。05 の 1.8 節「解像度: プレビューは 2560 × 1707（3:2）で固定」・
/// 02 の PERF-01。
pub const PREVIEW_SIZE: (u32, u32) = (2560, 1707);

/// ハローの評価用のエッジの暗い側の既定値（シーンリニア）。
///
/// **仮置き**: 逆光の人物・窓のある室内（05 の撮影リスト No.5）を想定し、[`HALO_BRIGHT`] との比を
/// 1600 : 1（約 10.6 EV）とした。実際の撮影データで見直す。
pub const HALO_DARK: f32 = 0.005;
/// ハローの評価用のエッジの明るい側の既定値（シーンリニア。白 = 1 を 3 EV 超える）。
///
/// **仮置き**（[`HALO_DARK`] を参照）。
pub const HALO_BRIGHT: f32 = 8.0;

/// 合成の画像の作成のエラー。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SynthError {
    /// 寸法が不正。
    #[error(transparent)]
    Image(#[from] ImageError),
    /// 引数が不正。
    #[error("引数が不正です: {0}")]
    InvalidArgument(String),
}

fn finite(name: &str, v: f64) -> Result<(), SynthError> {
    if v.is_finite() {
        Ok(())
    } else {
        Err(SynthError::InvalidArgument(format!(
            "{name} は有限の値が必要です（{v}）"
        )))
    }
}

fn finite_rgb(name: &str, v: [f32; 3]) -> Result<(), SynthError> {
    for c in v {
        finite(name, f64::from(c))?;
    }
    Ok(())
}

/// 中間のグレーから `ev` 段ずらした値（`0.18 × 2^ev`）。
pub fn gray_at_ev(ev: f32) -> f32 {
    (f64::from(MID_GRAY) * f64::from(ev).exp2()) as f32
}

/// `min_ev`〜`max_ev` を等間隔（EV で）に `count` 段に分けたグレーの値（[`gray_at_ev`]）。
///
/// `count` が 1 なら `min_ev` の 1 段。0 なら空。
pub fn ev_levels(count: usize, min_ev: f32, max_ev: f32) -> Vec<f32> {
    match count {
        0 => Vec::new(),
        1 => vec![gray_at_ev(min_ev)],
        n => (0..n)
            .map(|i| {
                let t = i as f64 / (n - 1) as f64;
                let ev = f64::from(min_ev) + (f64::from(max_ev) - f64::from(min_ev)) * t;
                gray_at_ev(ev as f32)
            })
            .collect(),
    }
}

/// グレーの階段（左から右へ `levels` の値の縦の帯。帯の幅はほぼ等しい）。
///
/// 画素 x の帯は `⌊x × 段数 / 幅⌋` 番目。幅は段数以上であること。
pub fn gray_steps(width: u32, height: u32, levels: &[f32]) -> Result<FloatImage, SynthError> {
    if levels.is_empty() {
        return Err(SynthError::InvalidArgument("段数が 0 です".to_owned()));
    }
    for &v in levels {
        finite("段の値", f64::from(v))?;
    }
    let n = levels.len() as u64;
    if u64::from(width) < n {
        return Err(SynthError::InvalidArgument(format!(
            "幅 {width} が段数 {n} より小さい"
        )));
    }
    Ok(FloatImage::from_rgb_fn(width, height, |x, _| {
        let i = (u64::from(x) * n / u64::from(width)) as usize;
        [levels[i]; 3]
    })?)
}

/// 円形のゾーンプレート（グレー）。デモザイクの偽色・モアレの評価用。
///
/// 中心からの距離 r（画素）での値は `low + (high − low) × (1 + cos(π f r² / R)) / 2`。
/// R は幅と高さの小さいほうの半分で、局所的な空間周波数は `f × r / R`（サイクル／画素）。
/// つまり中心で 0、距離 R（上下または左右の端）で `max_frequency` になる。
/// `max_frequency = 0.5` なら端でナイキスト周波数になる。
pub fn zone_plate(
    width: u32,
    height: u32,
    max_frequency: f64,
    low: f32,
    high: f32,
) -> Result<FloatImage, SynthError> {
    finite("max_frequency", max_frequency)?;
    finite("low", f64::from(low))?;
    finite("high", f64::from(high))?;
    if max_frequency <= 0.0 {
        return Err(SynthError::InvalidArgument(format!(
            "max_frequency は正の値が必要です（{max_frequency}）"
        )));
    }
    let r_edge = f64::from(width.min(height)) / 2.0;
    let (cx, cy) = (f64::from(width) / 2.0, f64::from(height) / 2.0);
    let k = std::f64::consts::PI * max_frequency / r_edge;
    let (lo, hi) = (f64::from(low), f64::from(high));
    Ok(FloatImage::from_rgb_fn(width, height, |x, y| {
        let dx = f64::from(x) + 0.5 - cx;
        let dy = f64::from(y) + 0.5 - cy;
        let v = lo + (hi - lo) * 0.5 * (1.0 + (k * (dx * dx + dy * dy)).cos());
        [v as f32; 3]
    })?)
}

/// 線の向き。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOrientation {
    /// 縦の線（x 方向に繰り返す）。
    Vertical,
    /// 横の線（y 方向に繰り返す）。
    Horizontal,
    /// 斜めの線（右上から左下。`x + y` 方向に繰り返す）。
    Diagonal,
}

/// 細い線の繰り返し。デモザイクの解像・偽色の評価用。
///
/// 位置 t（縦なら x、横なら y、斜めなら x + y）で `t mod period < line_width` の画素が `line`、
/// それ以外が `background`。`0 < line_width < period` であること（例: 周期 2・幅 1 は
/// 1 画素おきの線で、ナイキスト周波数）。色の付いた線（例: 赤と青）も指定できる。
pub fn line_pattern(
    width: u32,
    height: u32,
    orientation: LineOrientation,
    period: u32,
    line_width: u32,
    background: [f32; 3],
    line: [f32; 3],
) -> Result<FloatImage, SynthError> {
    if line_width == 0 || line_width >= period {
        return Err(SynthError::InvalidArgument(format!(
            "線の幅 {line_width} は 1 以上で周期 {period} より小さい必要があります"
        )));
    }
    finite_rgb("background", background)?;
    finite_rgb("line", line)?;
    Ok(FloatImage::from_rgb_fn(width, height, |x, y| {
        let t = match orientation {
            LineOrientation::Vertical => u64::from(x),
            LineOrientation::Horizontal => u64::from(y),
            LineOrientation::Diagonal => u64::from(x) + u64::from(y),
        };
        if t % u64::from(period) < u64::from(line_width) {
            line
        } else {
            background
        }
    })?)
}

/// エッジの形。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EdgeShape {
    /// 縦の境目。x が `x` 以上の画素が明るい側（左が暗い）。
    Vertical {
        /// 境目の位置（画素）。
        x: u32,
    },
    /// 横の境目。y が `y` 以上の画素が明るい側（上が暗い）。
    Horizontal {
        /// 境目の位置（画素）。
        y: u32,
    },
    /// 円（窓や光源に相当）。画素の中心が円の内側（境界を含む）なら明るい側。
    Disc {
        /// 中心の x（画素の座標。画像の左端が 0）。
        center_x: f32,
        /// 中心の y。
        center_y: f32,
        /// 半径（画素）。
        radius: f32,
    },
}

/// 明暗差の大きいエッジ（ハローの評価用。04 の 2.7 節・PoC-5）。
///
/// 境目はぼかさない（1 画素で切り替わる）。既定の明るさは [`HALO_DARK`]・[`HALO_BRIGHT`]。
pub fn step_edge(
    width: u32,
    height: u32,
    shape: EdgeShape,
    dark: [f32; 3],
    bright: [f32; 3],
) -> Result<FloatImage, SynthError> {
    finite_rgb("dark", dark)?;
    finite_rgb("bright", bright)?;
    if let EdgeShape::Disc {
        center_x,
        center_y,
        radius,
    } = shape
    {
        for (name, v) in [
            ("center_x", center_x),
            ("center_y", center_y),
            ("radius", radius),
        ] {
            finite(name, f64::from(v))?;
        }
        if radius < 0.0 {
            return Err(SynthError::InvalidArgument(format!(
                "半径は 0 以上が必要です（{radius}）"
            )));
        }
    }
    Ok(FloatImage::from_rgb_fn(width, height, |x, y| {
        let is_bright = match shape {
            EdgeShape::Vertical { x: e } => x >= e,
            EdgeShape::Horizontal { y: e } => y >= e,
            EdgeShape::Disc {
                center_x,
                center_y,
                radius,
            } => {
                let dx = f64::from(x) + 0.5 - f64::from(center_x);
                let dy = f64::from(y) + 0.5 - f64::from(center_y);
                dx * dx + dy * dy <= f64::from(radius) * f64::from(radius)
            }
        };
        if is_bright { bright } else { dark }
    })?)
}

/// 横方向のなめらかなグラデーション（左端が `start`、右端が `end` の線形補間）。
///
/// 画素 x の値は `start + (end − start) × x / (幅 − 1)`（幅 1 なら `start`）。
pub fn horizontal_gradient(
    width: u32,
    height: u32,
    start: [f32; 3],
    end: [f32; 3],
) -> Result<FloatImage, SynthError> {
    finite_rgb("start", start)?;
    finite_rgb("end", end)?;
    let denom = f64::from(width.saturating_sub(1).max(1));
    Ok(FloatImage::from_rgb_fn(width, height, |x, _| {
        let t = f64::from(x) / denom;
        std::array::from_fn(|c| {
            let (s, e) = (f64::from(start[c]), f64::from(end[c]));
            (s + (e - s) * t) as f32
        })
    })?)
}

/// 露光量で等間隔のグレーのグラデーション（左端が `min_ev`、右端が `max_ev`。値は [`gray_at_ev`]）。
///
/// シーンリニアの値は指数的に変わるので、トーンカーブ・シーン → ディスプレイ変換の段差の
/// 確認に使う。
pub fn ev_ramp(
    width: u32,
    height: u32,
    min_ev: f32,
    max_ev: f32,
) -> Result<FloatImage, SynthError> {
    finite("min_ev", f64::from(min_ev))?;
    finite("max_ev", f64::from(max_ev))?;
    // 値は EV について単調に増えるので、両端が有限なら途中もすべて有限（f32 で無限大にならない）。
    for (name, ev) in [("min_ev", min_ev), ("max_ev", max_ev)] {
        if !gray_at_ev(ev).is_finite() {
            return Err(SynthError::InvalidArgument(format!(
                "{name} = {ev} では値が f32 の範囲を超えます"
            )));
        }
    }
    let denom = f64::from(width.saturating_sub(1).max(1));
    Ok(FloatImage::from_rgb_fn(width, height, |x, _| {
        let t = f64::from(x) / denom;
        let ev = f64::from(min_ev) + (f64::from(max_ev) - f64::from(min_ev)) * t;
        [gray_at_ev(ev as f32); 3]
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(img: &FloatImage, x: u32, y: u32) -> f32 {
        img.pixel(x, y).unwrap()[0]
    }

    #[test]
    fn ev_levels_and_gray() {
        assert_eq!(gray_at_ev(0.0), 0.18);
        assert!((gray_at_ev(1.0) - 0.36).abs() < 1e-7);
        assert!((gray_at_ev(-2.0) - 0.045).abs() < 1e-7);
        let l = ev_levels(5, -2.0, 2.0);
        assert_eq!(l.len(), 5);
        for (v, ev) in l.iter().zip([-2.0_f32, -1.0, 0.0, 1.0, 2.0]) {
            assert!((v - gray_at_ev(ev)).abs() < 1e-7);
        }
        assert_eq!(ev_levels(1, -3.0, 3.0), vec![gray_at_ev(-3.0)]);
        assert!(ev_levels(0, 0.0, 1.0).is_empty());
    }

    #[test]
    fn gray_steps_bands() {
        let img = gray_steps(10, 2, &[0.1, 0.2, 0.3]).unwrap();
        // x × 3 / 10: 0〜3 → 0、4〜6 → 1、7〜9 → 2。
        let row: Vec<f32> = (0..10).map(|x| r(&img, x, 1)).collect();
        assert_eq!(row, vec![0.1, 0.1, 0.1, 0.1, 0.2, 0.2, 0.2, 0.3, 0.3, 0.3]);
        let p = img.pixel(5, 0).unwrap();
        assert_eq!(p, &[0.2, 0.2, 0.2]);
        assert!(gray_steps(2, 2, &[0.1, 0.2, 0.3]).is_err());
        assert!(gray_steps(4, 2, &[]).is_err());
        assert!(gray_steps(4, 2, &[f32::NAN]).is_err());
        assert!(matches!(
            gray_steps(4, 0, &[0.1]),
            Err(SynthError::Image(ImageError::InvalidDimensions { .. }))
        ));
    }

    #[test]
    fn zone_plate_phase_and_frequency() {
        // 64 × 64、R = 32、端で 0.5 サイクル／画素。位相 φ = π × 0.5 × r² / 32。
        let img = zone_plate(64, 64, 0.5, 0.0, 1.0).unwrap();
        // 中心付近（r² = 0.5）は 1 に近い。
        assert!(r(&img, 32, 32) > 0.999);
        // 位相が π になる r = 8（r² = 64）で最小値 0。画素 (40, 32) の中心は (40.5, 32.5)、
        // 中心からの差は (8.5, 0.5) なので r² = 72.5。式どおりの値になることを確かめる。
        let expected = 0.5 * (1.0 + (std::f64::consts::PI * 0.5 * 72.5 / 32.0).cos());
        assert!((f64::from(r(&img, 40, 32)) - expected).abs() < 1e-6);
        // 値は low〜high の範囲。
        assert!(img.data().iter().all(|&v| (0.0..=1.0).contains(&v)));
        // R G B は同じ（グレー）。
        let p = img.pixel(10, 20).unwrap();
        assert!(p[0] == p[1] && p[1] == p[2]);
        // 端の近くの局所周波数: 位相の差分から求めた周波数が f × r / R に近い。
        let phase = |x: f64| std::f64::consts::PI * 0.5 * x * x / 32.0;
        let freq = (phase(31.0) - phase(30.0)) / (2.0 * std::f64::consts::PI);
        assert!((freq - 0.5 * 30.5 / 32.0).abs() < 1e-12);
        assert!(zone_plate(8, 8, 0.0, 0.0, 1.0).is_err());
        assert!(zone_plate(8, 8, f64::NAN, 0.0, 1.0).is_err());
    }

    #[test]
    fn line_patterns() {
        let bg = [0.0; 3];
        let fg = [1.0, 0.5, 0.25];
        let v = line_pattern(6, 2, LineOrientation::Vertical, 3, 1, bg, fg).unwrap();
        let row: Vec<f32> = (0..6).map(|x| r(&v, x, 0)).collect();
        assert_eq!(row, vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(v.pixel(3, 1), Some(&fg[..]));
        let h = line_pattern(2, 4, LineOrientation::Horizontal, 2, 1, bg, fg).unwrap();
        let col: Vec<f32> = (0..4).map(|y| r(&h, 1, y)).collect();
        assert_eq!(col, vec![1.0, 0.0, 1.0, 0.0]);
        let d = line_pattern(4, 4, LineOrientation::Diagonal, 4, 2, bg, fg).unwrap();
        // (x + y) mod 4 < 2。
        assert_eq!(r(&d, 0, 0), 1.0);
        assert_eq!(r(&d, 1, 0), 1.0);
        assert_eq!(r(&d, 2, 0), 0.0);
        assert_eq!(r(&d, 1, 2), 0.0);
        assert_eq!(r(&d, 2, 2), 1.0);
        assert!(line_pattern(4, 4, LineOrientation::Vertical, 2, 2, bg, fg).is_err());
        assert!(line_pattern(4, 4, LineOrientation::Vertical, 2, 0, bg, fg).is_err());
        assert!(line_pattern(4, 4, LineOrientation::Vertical, 0, 0, bg, fg).is_err());
        assert!(line_pattern(4, 4, LineOrientation::Vertical, 3, 1, [f32::NAN; 3], fg).is_err());
    }

    #[test]
    fn step_edges() {
        let dark = [HALO_DARK; 3];
        let bright = [HALO_BRIGHT; 3];
        let v = step_edge(8, 2, EdgeShape::Vertical { x: 5 }, dark, bright).unwrap();
        assert_eq!(r(&v, 4, 0), HALO_DARK);
        assert_eq!(r(&v, 5, 0), HALO_BRIGHT);
        let h = step_edge(2, 8, EdgeShape::Horizontal { y: 3 }, dark, bright).unwrap();
        assert_eq!(r(&h, 0, 2), HALO_DARK);
        assert_eq!(r(&h, 0, 3), HALO_BRIGHT);
        let disc = EdgeShape::Disc {
            center_x: 5.0,
            center_y: 5.0,
            radius: 2.0,
        };
        let d = step_edge(10, 10, disc, dark, bright).unwrap();
        // 画素 (4, 4) の中心 (4.5, 4.5) は距離 0.707 で内側。(6, 4) の中心 (6.5, 4.5) は
        // 距離 1.58 で内側。(7, 5) の中心 (7.5, 5.5) は距離 2.55 で外側。
        assert_eq!(r(&d, 4, 4), HALO_BRIGHT);
        assert_eq!(r(&d, 6, 4), HALO_BRIGHT);
        assert_eq!(r(&d, 7, 5), HALO_DARK);
        assert_eq!(r(&d, 0, 0), HALO_DARK);
        // 内側の画素の数: 中心が (5, 5) から 2 以内の画素の中心 (x + 0.5, y + 0.5) は 12 個。
        let count = d.data().chunks(3).filter(|p| p[0] == HALO_BRIGHT).count();
        assert_eq!(count, 12);
        let bad = EdgeShape::Disc {
            center_x: 0.0,
            center_y: 0.0,
            radius: -1.0,
        };
        assert!(step_edge(4, 4, bad, dark, bright).is_err());
        // 比は約 10.6 EV（仮置きの値の確認）。
        let ev = (f64::from(HALO_BRIGHT) / f64::from(HALO_DARK)).log2();
        assert!((ev - 10.64).abs() < 0.01, "{ev}");
    }

    #[test]
    fn gradients() {
        let g = horizontal_gradient(5, 1, [0.0, 1.0, 0.5], [1.0, 0.0, 0.5]).unwrap();
        assert_eq!(g.pixel(0, 0), Some(&[0.0, 1.0, 0.5][..]));
        assert_eq!(g.pixel(2, 0), Some(&[0.5, 0.5, 0.5][..]));
        assert_eq!(g.pixel(4, 0), Some(&[1.0, 0.0, 0.5][..]));
        let one = horizontal_gradient(1, 1, [0.2; 3], [0.8; 3]).unwrap();
        assert_eq!(one.pixel(0, 0), Some(&[0.2, 0.2, 0.2][..]));
        // 単調増加。
        let g = horizontal_gradient(100, 1, [0.0; 3], [1.0; 3]).unwrap();
        assert!(
            g.data()
                .chunks(3)
                .zip(g.data().chunks(3).skip(1))
                .all(|(a, b)| b[0] > a[0])
        );

        let e = ev_ramp(9, 1, -4.0, 4.0).unwrap();
        assert_eq!(r(&e, 4, 0), MID_GRAY);
        assert!((r(&e, 0, 0) - gray_at_ev(-4.0)).abs() < 1e-7);
        assert!((r(&e, 8, 0) - gray_at_ev(4.0)).abs() < 1e-6);
        assert!(ev_ramp(9, 1, f32::INFINITY, 0.0).is_err());
    }

    #[test]
    fn ev_ramp_rejects_overflowing_values() {
        // 0.18 × 2^200 は f32 で無限大になる。引数は有限でも、無限大の画素を黙って作らない
        // （差の計算で NonFinite のエラーになり、原因が分かりにくい）。
        assert!(matches!(
            ev_ramp(4, 1, 0.0, 200.0),
            Err(SynthError::InvalidArgument(_))
        ));
        assert!(matches!(
            ev_ramp(4, 1, 200.0, 0.0),
            Err(SynthError::InvalidArgument(_))
        ));
        // 非常に暗い側（0 に丸められる）は有限なので作れる。
        let img = ev_ramp(4, 1, -200.0, 0.0).unwrap();
        assert!(img.find_non_finite().is_none());
    }
}
