//! 段階 B: かすみの推定（ステージ 12 のかすみの除去が使う。DEV-11。処理バージョン 2。docs/04_architecture.md の
//! 2.1 節・2.7 節）。
//!
//! 画像全体の情報（大気の光）と、場所ごとのかすみの濃さ（暗いチャンネル）を、**段階 B のガイドと同じ縮小した
//! 画像**（G の座標、長辺 [`crate::guide::GUIDE_LONG_EDGE`] px。切り抜く前の全体、露光量の前）から求め、
//! ガイドと一緒に持つ（[`crate::stage::Guide::haze`]）。フル解像度（等倍・書き出し）もプレビューと同じ推定を
//! 使うので、かすみの除去の効き方はプレビューと書き出しでそろう（IQ-07b）。切り抜きを変えても推定は変わらない。
//!
//! # 方法（He, Sun, Tang, "Single Image Haze Removal Using Dark Channel Prior", CVPR 2009 の暗いチャンネルの
//! 事前分布。精緻化は He, Sun, Tang, "Guided Image Filtering", ECCV 2010 のガイデッドフィルタ）
//!
//! ```text
//! m(x)    = max(min(R, G, B), 0)
//! dA(x)   = 半径 r の正方形の窓の m の最小                          （横、縦の順。端の外は鏡映）
//! A       = dA の大きい順（同じなら番号の小さい順）に上位 0.1 %（1 画素以上）の RGB の平均（f64 で番号の順に足す）。
//!           各チャンネル AIRLIGHT_MIN 以上
//! n(x)    = clamp(min_c(max(I_c, 0) / A_c), 0, 1)
//! dark0   = 半径 r の窓の n の最小
//! dark    = clamp(ガイデッドフィルタ(案内 n、入力 dark0、半径 2r、ε), 0, 1)
//! ```
//!
//! 窓の半径 r は G の長辺に対する割合（[`DEHAZE_PATCH_FRACTION`]）をガイドの画素に直したもの（1 以上）。
//! ステージ 12 は `dark` を双線形で引き、透過率 `t = 1 − ω·dark` で戻す（[`crate::finish::presence`]）。
//! **仮実装**: 値は合成の画像だけで決めた（PoC-4 で実写のかすみ・空で確かめる）。CPU 版だけ（ガイドと同じく
//! 段階 B は GPU 版を作らない）。

use rayon::prelude::*;

use crate::border::mirror_index;
use crate::error::{PipelineError, Result};
use crate::guide::box_mean;
use crate::image::GrayImage;

/// 暗いチャンネルの窓の半径（G の長辺に対する割合）。仮置き: 長辺 512px のガイドで 5px。
pub const DEHAZE_PATCH_FRACTION: f64 = 0.01;

/// 大気の光を求めるのに使う、暗いチャンネルの大きい画素の割合（He ら 2009 の 0.1 %）。
pub const DEHAZE_TOP_FRACTION: f64 = 0.001;

/// 精緻化のガイデッドフィルタの ε（暗いチャンネルの分散）。仮置き。
pub const DEHAZE_REFINE_EPS: f32 = 1e-3;

/// 大気の光の各チャンネルの下限（シーンリニア。0 で割らないため）。
pub const AIRLIGHT_MIN: f32 = 1e-3;

/// かすみの推定（ガイドと同じ寸法・座標）。
#[derive(Debug, Clone, PartialEq)]
pub struct HazeEstimate {
    /// 精緻化した暗いチャンネル（0〜1。大きいほどかすみが濃い）。画素 (i, j) の位置はガイドの画素と同じ。
    pub dark: GrayImage,
    /// 大気の光 A（B2。露光量の前の値）。
    pub airlight: [f32; 3],
}

/// ガイドの長辺 `long_side` px での窓の半径（px。1 以上）。
pub fn patch_radius(long_side: u32) -> u32 {
    ((DEHAZE_PATCH_FRACTION * f64::from(long_side)).round() as u32).max(1)
}

/// 縮小した RGB（`width` × `height`、行優先。B2）からかすみを推定する（モジュールの doc）。
pub fn estimate_haze(rgb: &[[f32; 3]], width: u32, height: u32) -> Result<HazeEstimate> {
    let n_px = width as usize * height as usize;
    if n_px == 0 || rgb.len() != n_px {
        return Err(PipelineError::InvalidArgument(format!(
            "かすみの推定の入力の寸法が違います（{} 画素、{width}×{height}）",
            rgb.len()
        )));
    }
    let r = patch_radius(width.max(height));
    // 1〜2. 暗いチャンネル（大気の光を求める用）。
    let m = GrayImage::from_vec(
        width,
        height,
        rgb.par_iter()
            .map(|c| c[0].min(c[1]).min(c[2]).max(0.0))
            .collect(),
    )?;
    let da = min_filter(&m, r)?;
    // 3. 大気の光。
    let mut order: Vec<usize> = (0..n_px).collect();
    let d = da.data();
    order.sort_by(|&a, &b| d[b].total_cmp(&d[a]).then(a.cmp(&b)));
    let top = ((n_px as f64 * DEHAZE_TOP_FRACTION).round() as usize).clamp(1, n_px);
    let mut picked: Vec<usize> = order[..top].to_vec();
    picked.sort_unstable();
    let mut sum = [0.0f64; 3];
    for &i in &picked {
        for (s, v) in sum.iter_mut().zip(rgb[i]) {
            *s += f64::from(v);
        }
    }
    let airlight = sum.map(|s| ((s / top as f64) as f32).max(AIRLIGHT_MIN));
    // 4〜5. A で割った最小のチャンネルと、その局所の最小。
    let inv = airlight.map(|a| 1.0 / a);
    let n = GrayImage::from_vec(
        width,
        height,
        rgb.par_iter()
            .map(|c| {
                let v = (c[0].max(0.0) * inv[0])
                    .min(c[1].max(0.0) * inv[1])
                    .min(c[2].max(0.0) * inv[2]);
                v.clamp(0.0, 1.0)
            })
            .collect(),
    )?;
    let dark0 = min_filter(&n, r)?;
    // 6. 精緻化。
    let mut dark = guided_filter_with(&n, &dark0, 2 * r, DEHAZE_REFINE_EPS)?;
    dark.sanitize();
    for v in dark.data_mut() {
        *v = v.clamp(0.0, 1.0);
    }
    Ok(HazeEstimate { dark, airlight })
}

/// 窓の最小（一辺 `2r + 1` の正方形。横、縦の順。端の外は鏡映）。
pub fn min_filter(image: &GrayImage, radius: u32) -> Result<GrayImage> {
    let (w, h) = (image.width(), image.height());
    let r = i64::from(radius);
    let src = image.data();
    let mut tmp = vec![0.0f32; src.len()];
    tmp.par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y, out)| {
            let row = &src[y * w as usize..(y + 1) * w as usize];
            for (x, dst) in out.iter_mut().enumerate() {
                let mut acc = f32::INFINITY;
                for k in -r..=r {
                    acc = acc.min(row[mirror_index(x as i64 + k, w) as usize]);
                }
                *dst = acc;
            }
        });
    let mut data = vec![0.0f32; src.len()];
    data.par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y, out)| {
            for (x, dst) in out.iter_mut().enumerate() {
                let mut acc = f32::INFINITY;
                for k in -r..=r {
                    let yy = mirror_index(y as i64 + k, h) as usize;
                    acc = acc.min(tmp[yy * w as usize + x]);
                }
                *dst = acc;
            }
        });
    GrayImage::from_vec(w, h, data)
}

/// 案内の画像 `guide` で入力 `src` をぼかすガイデッドフィルタ（He ら 2010 の式。[`crate::guide::guided_filter`]
/// は自身を案内にしたもの）。
///
/// ```text
/// mI = box(I)、 mp = box(p)、 cov = box(I·p) − mI·mp、 var = max(box(I·I) − mI·mI, 0)
/// a = cov / (var + ε)、 b = mp − a·mI、 q = box(a)·I + box(b)
/// ```
pub fn guided_filter_with(
    guide: &GrayImage,
    src: &GrayImage,
    radius: u32,
    epsilon: f32,
) -> Result<GrayImage> {
    if !(epsilon.is_finite() && epsilon > 0.0) {
        return Err(PipelineError::InvalidArgument(format!(
            "ガイデッドフィルタの ε は正の有限の値が必要です（{epsilon}）"
        )));
    }
    let (w, h) = (guide.width(), guide.height());
    if (src.width(), src.height()) != (w, h) {
        return Err(PipelineError::InvalidArgument(
            "ガイデッドフィルタの案内と入力の寸法が違います".to_owned(),
        ));
    }
    let i = guide.data();
    let p = src.data();
    let mean_i = box_mean(guide, radius)?;
    let mean_p = box_mean(src, radius)?;
    let ip = GrayImage::from_vec(w, h, i.par_iter().zip(p).map(|(&a, &b)| a * b).collect())?;
    let ii = GrayImage::from_vec(w, h, i.par_iter().map(|&a| a * a).collect())?;
    let mean_ip = box_mean(&ip, radius)?;
    let mean_ii = box_mean(&ii, radius)?;
    let (a, b): (Vec<f32>, Vec<f32>) = (0..i.len())
        .into_par_iter()
        .map(|k| {
            let mi = mean_i.data()[k];
            let mp = mean_p.data()[k];
            let cov = mean_ip.data()[k] - mi * mp;
            let var = (mean_ii.data()[k] - mi * mi).max(0.0);
            let a = cov / (var + epsilon);
            (a, mp - a * mi)
        })
        .unzip();
    let mean_a = box_mean(&GrayImage::from_vec(w, h, a)?, radius)?;
    let mean_b = box_mean(&GrayImage::from_vec(w, h, b)?, radius)?;
    let q = i
        .par_iter()
        .zip(mean_a.data().par_iter().zip(mean_b.data().par_iter()))
        .map(|(&v, (&ma, &mb))| ma * v + mb)
        .collect();
    GrayImage::from_vec(w, h, q)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 左半分が澄んだ濃い色（かすみなし）、右半分が白いかすみ（大気の光 0.8）に覆われた画像。
    fn hazy_scene(w: u32, h: u32) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|i| {
                let x = i % w;
                let j = [0.05f32, 0.2, 0.1];
                if x < w / 2 {
                    j
                } else {
                    // t = 0.3: I = J·t + A·(1 − t)。
                    j.map(|v| v * 0.3 + 0.8 * 0.7)
                }
            })
            .collect()
    }

    #[test]
    fn airlight_and_dark_channel_follow_the_haze() {
        let (w, h) = (80u32, 40u32);
        let img = hazy_scene(w, h);
        let e = estimate_haze(&img, w, h).unwrap();
        // 大気の光は、かすみの部分の色（かすみの濃い部分の最も明るい色）。
        for (a, v) in e.airlight.iter().zip([0.575f32, 0.62, 0.59]) {
            assert!((a - v).abs() < 1e-4, "{:?}", e.airlight);
        }
        // 暗いチャンネル: 澄んだ部分は小さく、かすみの部分は大きい（1 − t に近い）。
        let clear = e.dark.pixel(5, 20).unwrap();
        let hazy = e.dark.pixel(75, 20).unwrap();
        assert!(clear < 0.15, "{clear}");
        assert!(hazy > 0.9, "{hazy}");
        assert!(e.dark.data().iter().all(|v| (0.0..=1.0).contains(v)));
        assert_eq!((e.dark.width(), e.dark.height()), (w, h));
    }

    #[test]
    fn estimate_is_deterministic_and_handles_dark_and_negative_input() {
        let (w, h) = (31u32, 17u32);
        let img: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let v = ((i * 37) % 11) as f32 * 0.01 - 0.02;
                [v, -v, v * 0.5]
            })
            .collect();
        let a = estimate_haze(&img, w, h).unwrap();
        let b = estimate_haze(&img, w, h).unwrap();
        assert_eq!(a, b);
        assert!(a.airlight.iter().all(|&v| v >= AIRLIGHT_MIN));
        assert!(a.dark.data().iter().all(|v| v.is_finite()));
        // 真っ黒でも 0 で割らない。
        let black = vec![[0.0f32; 3]; 12];
        let e = estimate_haze(&black, 4, 3).unwrap();
        assert_eq!(e.airlight, [AIRLIGHT_MIN; 3]);
        assert!(estimate_haze(&black, 5, 3).is_err());
    }

    #[test]
    fn min_filter_and_guided_filter_basics() {
        let g = GrayImage::from_vec(5, 1, vec![3.0, 1.0, 4.0, 1.0, 5.0]).unwrap();
        let m = min_filter(&g, 1).unwrap();
        assert_eq!(m.data(), [1.0, 1.0, 1.0, 1.0, 1.0]);
        let c = GrayImage::filled(6, 4, 0.4).unwrap();
        let q = guided_filter_with(&c, &c, 2, 1e-3).unwrap();
        assert!(q.data().iter().all(|v| (v - 0.4).abs() < 1e-5));
        assert!(guided_filter_with(&c, &c, 2, 0.0).is_err());
        let other = GrayImage::filled(3, 4, 0.4).unwrap();
        assert!(guided_filter_with(&c, &other, 2, 1e-3).is_err());
        assert_eq!(patch_radius(512), 5);
        assert_eq!(patch_radius(20), 1);
    }
}
