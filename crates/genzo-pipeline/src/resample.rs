//! 縮小（docs/04_architecture.md の 2.2 節「プレビュー解像度（長辺 2560px）に縮小した作業色空間の
//! 画像」、02 の IQ-07b「決まった縮小フィルタ」）。
//!
//! 縮小のフィルタは **面積平均**（出力の 1 画素が覆う入力の範囲を、重なった面積で重み付けして
//! 平均する）。定義は genzo-testkit の `resample::downscale_area` と同じ:
//!
//! - 位置合わせは「入力と出力の画像の四隅を合わせる」。出力の画素 `o` は入力の範囲
//!   `[o × W / w, (o + 1) × W / w)` に対応する（W は入力、w は出力の寸法）。境界の外は参照しない。
//! - 値は線形に平均するので、**リニアの値（B1〜B3）に対して使う**。
//!
//! # GPU 版と一致させるための決めごと（2.3 節）
//!
//! - 重み（[`AreaWeights`]）は f64 で計算して f32 に丸めた表とし、GPU 版にも同じ表を渡す。
//! - 先に横方向、次に縦方向に縮小する（分離可能）。どちらも、重み付きの和を **入力の番号の小さい
//!   順に** f32 で足す（`acc = acc + w × v`、最初は 0）。
//! - 出力の画素ごとの計算は帯（行のまとまり）の分け方によらないので、並列化してもスレッド数で
//!   結果は変わらない。
//!
//! testkit の実装は f64 で足すため、結果は f32 の丸めの範囲で一致する（テストで確認する）。

use rayon::prelude::*;

use crate::error::{PipelineError, Result};
use crate::image::RgbImage;

/// プレビュー（段階 A1 の出力・L1）の長辺の既定値（px）。04 の 2.2 節・4 章。
pub const DEFAULT_PREVIEW_LONG_EDGE: u32 = 2560;

/// 縦方向の縮小で、1 回にまとめて処理する出力の行数（並列化の単位）。結果には影響しない。
const ROWS_PER_TASK: usize = 16;

/// 長辺を `long_edge` px にしたときの寸法（拡大はしない）。
///
/// 長辺が `long_edge` 以下ならそのまま。そうでなければ長辺を `long_edge`、短辺を
/// `round(短辺 × long_edge / 長辺)`（0.5 は切り上げ、1 以上）にする。
/// 例: 7008 × 4672（α7 IV）→ 2560 × 1707。
pub fn preview_size(width: u32, height: u32, long_edge: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= long_edge || long_edge == 0 {
        return (width, height);
    }
    let scale = |short: u32| -> u32 {
        let v = (u64::from(short) * u64::from(long_edge) + u64::from(long) / 2) / u64::from(long);
        (v as u32).max(1)
    };
    if width >= height {
        (long_edge, scale(height))
    } else {
        (scale(width), long_edge)
    }
}

/// 面積平均の重みの表（1 次元）。出力の画素ごとに、入力の画素の番号と重み（和はほぼ 1）を持つ。
#[derive(Debug, Clone, PartialEq)]
pub struct AreaWeights {
    src: u32,
    dst: u32,
    /// 出力の画素ごとの、`taps` の中の開始位置と数。
    spans: Vec<(u32, u32)>,
    /// (入力の番号, 重み) の列。
    taps: Vec<(u32, f32)>,
}

impl AreaWeights {
    /// 長さ `src` を `dst` に縮小する重み。`0 < dst ≤ src` であること（拡大はしない）。
    pub fn new(src: u32, dst: u32) -> Result<Self> {
        if dst == 0 || src == 0 || dst > src {
            return Err(PipelineError::InvalidArgument(format!(
                "面積平均の縮小は 0 < 出力 ≤ 入力 が必要です（入力 {src}、出力 {dst}）"
            )));
        }
        let scale = f64::from(src) / f64::from(dst);
        let mut spans = Vec::with_capacity(dst as usize);
        let mut taps = Vec::new();
        for o in 0..u64::from(dst) {
            // 端は整数の比で計算し、丸めの誤差で範囲の外に出ないようにする（testkit と同じ）。
            let start = (o * u64::from(src)) as f64 / f64::from(dst);
            let end = ((o + 1) * u64::from(src)) as f64 / f64::from(dst);
            let first = start.floor() as u32;
            let last = (end.ceil() as u32).min(src);
            let begin = taps.len() as u32;
            for i in first..last {
                let overlap = end.min(f64::from(i) + 1.0) - start.max(f64::from(i));
                if overlap > 0.0 {
                    taps.push((i, (overlap / scale) as f32));
                }
            }
            spans.push((begin, taps.len() as u32 - begin));
        }
        Ok(Self {
            src,
            dst,
            spans,
            taps,
        })
    }

    /// 入力の長さ。
    pub fn src_len(&self) -> u32 {
        self.src
    }

    /// 出力の長さ。
    pub fn dst_len(&self) -> u32 {
        self.dst
    }

    /// 出力の画素 `o` の (入力の番号, 重み) の列（入力の番号の小さい順）。
    pub fn taps(&self, o: u32) -> &[(u32, f32)] {
        let (b, n) = self.spans[o as usize];
        &self.taps[b as usize..(b + n) as usize]
    }

    /// 出力の範囲 `[o0, o1)` が参照する入力の範囲 `[i0, i1)`。
    pub fn source_range(&self, o0: u32, o1: u32) -> (u32, u32) {
        debug_assert!(o0 < o1 && o1 <= self.dst);
        let first = self.taps(o0).first().map_or(0, |t| t.0);
        let last = self.taps(o1 - 1).last().map_or(0, |t| t.0 + 1);
        (first, last)
    }
}

/// 重み付きの和（入力の番号の小さい順に f32 で足す）。
#[inline]
fn weighted_sum(taps: &[(u32, f32)], mut get: impl FnMut(u32) -> [f32; 3]) -> [f32; 3] {
    let mut acc = [0.0f32; 3];
    for &(i, w) in taps {
        let p = get(i);
        acc[0] += w * p[0];
        acc[1] += w * p[1];
        acc[2] += w * p[2];
    }
    acc
}

/// 1 行を横方向に縮小する（`src.len() == wx.src_len()`、`out.len() == wx.dst_len()`）。
pub fn downscale_row(wx: &AreaWeights, src: &[[f32; 3]], out: &mut [[f32; 3]]) {
    debug_assert_eq!(src.len(), wx.src_len() as usize);
    debug_assert_eq!(out.len(), wx.dst_len() as usize);
    for (o, dst) in out.iter_mut().enumerate() {
        *dst = weighted_sum(wx.taps(o as u32), |i| src[i as usize]);
    }
}

/// 横方向に縮小済みの行から、出力の行 `oy` を縦方向に縮小して作る。`row(j)` は入力の行 `j` を
/// 横方向に縮小したもの（長さは出力の幅）。
pub fn downscale_column_row<'a>(
    wy: &AreaWeights,
    oy: u32,
    row: impl Fn(u32) -> &'a [[f32; 3]],
    out: &mut [[f32; 3]],
) {
    let taps = wy.taps(oy);
    for (x, dst) in out.iter_mut().enumerate() {
        *dst = weighted_sum(taps, |j| row(j)[x]);
    }
}

/// 面積平均で `width` × `height` に縮小する（拡大はしない）。同じ寸法ならそのまま複製する。
///
/// 値はリニアであること（[`crate::contract::ColorContract::is_linear`]）。符号化済みの値（B4・B5）は
/// エラー。
pub fn downscale_area(image: &RgbImage, width: u32, height: u32) -> Result<RgbImage> {
    if !image.contract().is_linear() {
        return Err(PipelineError::InvalidArgument(format!(
            "面積平均の縮小はリニアの値にだけ使えます（{}）",
            image.contract()
        )));
    }
    if (width, height) == image.dimensions() {
        return Ok(image.clone());
    }
    let wx = AreaWeights::new(image.width(), width)?;
    let wy = AreaWeights::new(image.height(), height)?;
    let (dw, dh) = (width as usize, height as usize);
    let sw = image.width() as usize;

    // 横方向: 入力の行ごと。
    let mut tmp = vec![[0.0f32; 3]; image.height() as usize * dw];
    tmp.par_chunks_mut(dw)
        .zip(image.data().par_chunks(sw))
        .for_each(|(out, src)| downscale_row(&wx, src, out));

    // 縦方向: 出力の行ごと。
    let mut data = vec![[0.0f32; 3]; dh * dw];
    data.par_chunks_mut(dw * ROWS_PER_TASK)
        .enumerate()
        .for_each(|(chunk, rows)| {
            for (k, out) in rows.chunks_mut(dw).enumerate() {
                let oy = (chunk * ROWS_PER_TASK + k) as u32;
                downscale_column_row(
                    &wy,
                    oy,
                    |j| &tmp[j as usize * dw..(j as usize + 1) * dw],
                    out,
                );
            }
        });
    RgbImage::from_vec(width, height, image.contract(), data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::ColorContract;

    #[test]
    fn preview_sizes() {
        assert_eq!(preview_size(7008, 4672, 2560), (2560, 1707));
        assert_eq!(preview_size(4672, 7008, 2560), (1707, 2560));
        assert_eq!(preview_size(2560, 1000, 2560), (2560, 1000));
        assert_eq!(preview_size(100, 50, 2560), (100, 50));
        assert_eq!(preview_size(10000, 1, 2560), (2560, 1));
        assert_eq!(preview_size(5000, 5000, 320), (320, 320));
        // 0.5 は切り上げ: 3 × 4 / 8 = 1.5 → 2。
        assert_eq!(preview_size(8, 3, 4), (4, 2));
        assert_eq!(preview_size(10, 5, 4), (4, 2));
        assert_eq!(preview_size(10, 5, 0), (10, 5));
    }

    #[test]
    fn weights_sum_to_one_and_stay_in_range() {
        for (src, dst) in [(7008, 2560), (3, 2), (10, 10), (37, 10), (5, 1)] {
            let w = AreaWeights::new(src, dst).unwrap();
            for o in 0..dst {
                let taps = w.taps(o);
                assert!(!taps.is_empty());
                let s: f64 = taps.iter().map(|t| f64::from(t.1)).sum();
                assert!((s - 1.0).abs() < 1e-6, "{src}→{dst} o={o} s={s}");
                assert!(taps.windows(2).all(|p| p[0].0 < p[1].0));
                assert!(taps.iter().all(|t| t.0 < src));
            }
            assert_eq!(w.source_range(0, dst), (0, src));
        }
        assert!(AreaWeights::new(3, 4).is_err());
        assert!(AreaWeights::new(3, 0).is_err());
    }

    #[test]
    fn integer_factor_is_block_mean() {
        let img = RgbImage::from_fn(4, 4, ColorContract::B2Working, |x, y| {
            [(y * 4 + x) as f32, 1.0, -1.0]
        })
        .unwrap();
        let out = downscale_area(&img, 2, 2).unwrap();
        let r: Vec<f32> = out.data().iter().map(|p| p[0]).collect();
        assert_eq!(r, [2.5, 4.5, 10.5, 12.5]);
        assert!(out.data().iter().all(|p| p[1] == 1.0 && p[2] == -1.0));
        assert_eq!(out.contract(), ColorContract::B2Working);
    }

    #[test]
    fn same_size_is_a_copy_and_errors_are_reported() {
        let img = RgbImage::filled(3, 2, ColorContract::B1CameraRgb, [0.5; 3]).unwrap();
        assert_eq!(downscale_area(&img, 3, 2).unwrap(), img);
        assert!(downscale_area(&img, 4, 2).is_err());
        let enc = RgbImage::filled(3, 2, ColorContract::B5Cache, [0.5; 3]).unwrap();
        assert!(downscale_area(&enc, 1, 1).is_err());
    }
}
