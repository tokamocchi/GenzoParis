//! 面積平均による縮小（エイリアシングの少ない縮小。PRV-01・B5 のキャッシュと書き出しの寸法）。
//!
//! 出力の各画素は、入力画像の上でその画素が覆う矩形（幅 `src_w / dst_w`、高さ `src_h / dst_h`）
//! に重なる入力の画素を、**重なる面積で重み付けした平均** にする（box フィルタを面積で積分したもの。
//! 倍率が整数でなくても、すべての入力画素が重なりの割合どおりに寄与する）。
//!
//! - 値は **リニアな光の量** として平均する（符号化した値のまま平均すると、明暗の境界が暗くなるため）。
//!   符号化した画像を縮小するときは [`downscale_encoded`] を使う。
//! - 縮小だけを扱う（出力の幅・高さは入力以下）。拡大はしない（`ExportSize::fit` と同じ方針）。
//! - 入力の NaN・無限大は 0 とみなす（04 の 2.6 節）。
//! - 入力を 1 行ずつ受け取る（[`AreaDownscaler`]）ので、入力全体を浮動小数点で持たなくてよい。

use genzo_color::TransferFunction;

use crate::buffer::{DynRgbImage, RgbImage, Sample, checked_len};
use crate::error::{MediaError, Result};

/// 1 軸ぶんの重み。入力の画素 `i` は、最大 2 つの出力の画素に寄与する（倍率 ≥ 1 のため）。
#[derive(Debug, Clone)]
struct AxisTaps {
    /// 入力の画素ごとの（出力の位置, 重なりの長さ）× 2（2 つ目は重みが 0 のことがある）。
    taps: Vec<[(u32, f32); 2]>,
    /// 出力の画素ごとの重みの合計（正規化に使う）。
    norm: Vec<f32>,
}

impl AxisTaps {
    fn new(src: u32, dst: u32) -> Self {
        debug_assert!(dst >= 1 && dst <= src);
        let scale = f64::from(src) / f64::from(dst);
        let mut taps = Vec::with_capacity(src as usize);
        let mut norm = vec![0.0_f64; dst as usize];
        for i in 0..src {
            let start = f64::from(i);
            let end = start + 1.0;
            // この入力の画素が始まる出力の位置。
            let d0 = ((start / scale).floor() as u32).min(dst - 1);
            let boundary = f64::from(d0 + 1) * scale;
            let (w0, second) = if end <= boundary || d0 + 1 >= dst {
                (1.0, (d0, 0.0))
            } else {
                // 丸め誤差で負や 1 超えにならないよう、重なりの長さを 0〜1 に収める。
                let w0 = (boundary - start).clamp(0.0, 1.0);
                (w0, (d0 + 1, 1.0 - w0))
            };
            norm[d0 as usize] += w0;
            norm[second.0 as usize] += second.1;
            taps.push([(d0, w0 as f32), (second.0, second.1 as f32)]);
        }
        Self {
            taps,
            norm: norm.into_iter().map(|v| v as f32).collect(),
        }
    }
}

fn sanitize(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// 1 行ずつ入力を受け取って面積平均で縮小する。
///
/// [`push_row`](Self::push_row) で入力の行を上から順に渡し、最後に [`finish`](Self::finish) で
/// 出力（行優先、`dst_w × dst_h` 画素）を受け取る。
#[derive(Debug, Clone)]
pub struct AreaDownscaler {
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    xs: AxisTaps,
    ys: AxisTaps,
    next_row: u32,
    row: Vec<[f32; 3]>,
    acc: Vec<[f32; 3]>,
}

impl AreaDownscaler {
    /// `src_w × src_h` を `dst_w × dst_h` に縮小する。出力は入力以下の寸法（1 以上）であること。
    pub fn new(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Result<Self> {
        checked_len(src_w, src_h)?;
        let dst_len = checked_len(dst_w, dst_h)? / 3;
        if dst_w > src_w || dst_h > src_h {
            return Err(MediaError::invalid_argument(format!(
                "拡大はしない（{src_w}×{src_h} → {dst_w}×{dst_h}）"
            )));
        }
        Ok(Self {
            src_w,
            src_h,
            dst_w,
            dst_h,
            xs: AxisTaps::new(src_w, dst_w),
            ys: AxisTaps::new(src_h, dst_h),
            next_row: 0,
            row: vec![[0.0; 3]; dst_w as usize],
            acc: vec![[0.0; 3]; dst_len],
        })
    }

    /// 出力の寸法（幅, 高さ）。
    pub fn output_dimensions(&self) -> (u32, u32) {
        (self.dst_w, self.dst_h)
    }

    /// 入力の次の行（`src_w` 画素）を渡す。
    pub fn push_row(&mut self, row: &[[f32; 3]]) -> Result<()> {
        if row.len() != self.src_w as usize {
            return Err(MediaError::invalid_argument(format!(
                "行の長さ {} が入力の幅 {} と一致しない",
                row.len(),
                self.src_w
            )));
        }
        if self.next_row >= self.src_h {
            return Err(MediaError::invalid_argument(format!(
                "入力の高さ {} より多くの行が渡された",
                self.src_h
            )));
        }
        // 横方向。
        self.row.fill([0.0; 3]);
        for (px, taps) in row.iter().zip(&self.xs.taps) {
            let px = px.map(sanitize);
            for &(d, w) in taps {
                let o = &mut self.row[d as usize];
                for c in 0..3 {
                    o[c] += px[c] * w;
                }
            }
        }
        for (o, &n) in self.row.iter_mut().zip(&self.xs.norm) {
            for v in o.iter_mut() {
                *v /= n;
            }
        }
        // 縦方向。
        let dst_w = self.dst_w as usize;
        for &(d, w) in &self.ys.taps[self.next_row as usize] {
            if w == 0.0 {
                continue;
            }
            let out = &mut self.acc[d as usize * dst_w..(d as usize + 1) * dst_w];
            for (o, r) in out.iter_mut().zip(&self.row) {
                for c in 0..3 {
                    o[c] += r[c] * w;
                }
            }
        }
        self.next_row += 1;
        Ok(())
    }

    /// 縮小した結果（行優先、`dst_w × dst_h` 画素）を返す。すべての行を渡していなければエラー。
    pub fn finish(mut self) -> Result<Vec<[f32; 3]>> {
        if self.next_row != self.src_h {
            return Err(MediaError::invalid_argument(format!(
                "入力の行が足りない（{} / {}）",
                self.next_row, self.src_h
            )));
        }
        let dst_w = self.dst_w as usize;
        for (y, &n) in self.ys.norm.iter().enumerate() {
            for o in &mut self.acc[y * dst_w..(y + 1) * dst_w] {
                for v in o.iter_mut() {
                    *v /= n;
                }
            }
        }
        Ok(self.acc)
    }
}

/// リニアな値の画像（行優先、`src_w × src_h` 画素）を面積平均で縮小する。
pub fn downscale_area(
    src: &[[f32; 3]],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Result<Vec<[f32; 3]>> {
    let len = checked_len(src_w, src_h)? / 3;
    if src.len() != len {
        return Err(MediaError::invalid_argument(format!(
            "データの長さ {} が {src_w}×{src_h} と一致しない",
            src.len()
        )));
    }
    let mut ds = AreaDownscaler::new(src_w, src_h, dst_w, dst_h)?;
    for row in src.chunks_exact(src_w as usize) {
        ds.push_row(row)?;
    }
    ds.finish()
}

/// 符号化の値（0〜最大値）→ リニアな値の表。
fn decode_table<T: Sample>(tf: TransferFunction) -> Vec<f32> {
    (0..=u32::from(T::MAX))
        .map(|v| tf.decode_f32(v as f32 / f32::from(T::MAX)))
        .collect()
}

fn downscale_encoded_typed<T: Sample>(
    img: &RgbImage<T>,
    tf: TransferFunction,
    dst_w: u32,
    dst_h: u32,
) -> Result<RgbImage<T>> {
    let (w, h) = img.dimensions();
    if (w, h) == (dst_w, dst_h) {
        return Ok(img.clone());
    }
    let table = decode_table::<T>(tf);
    let mut ds = AreaDownscaler::new(w, h, dst_w, dst_h)?;
    let mut lin = vec![[0.0_f32; 3]; w as usize];
    for row in img.rows() {
        for (o, px) in lin.iter_mut().zip(row.chunks_exact(3)) {
            *o = [
                table[px[0].to_u32() as usize],
                table[px[1].to_u32() as usize],
                table[px[2].to_u32() as usize],
            ];
        }
        ds.push_row(&lin)?;
    }
    let out = ds.finish()?;
    let mut data = Vec::with_capacity(out.len() * 3);
    for px in out {
        for v in px {
            data.push(T::from_unit_f32(tf.encode_f32(v)));
        }
    }
    RgbImage::from_raw(dst_w, dst_h, data)
}

/// 伝達関数 `tf` で符号化された画像を、リニアに戻してから面積平均で縮小し、同じ伝達関数・
/// 同じビット数で符号化し直す（四捨五入。ディザリングはしない）。
///
/// 書き出しの寸法（`ExportSize::LongEdge`）を、出力の色空間で符号化した後の画像に適用するために
/// 使う。出力が入力と同じ寸法なら、そのまま返す。
pub fn downscale_encoded(
    img: &DynRgbImage,
    tf: TransferFunction,
    dst_w: u32,
    dst_h: u32,
) -> Result<DynRgbImage> {
    Ok(match img {
        DynRgbImage::Rgb8(i) => DynRgbImage::Rgb8(downscale_encoded_typed(i, tf, dst_w, dst_h)?),
        DynRgbImage::Rgb16(i) => DynRgbImage::Rgb16(downscale_encoded_typed(i, tf, dst_w, dst_h)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::RgbImage8;

    fn gray(v: f32) -> [f32; 3] {
        [v, v, v]
    }

    #[test]
    fn integer_ratio_is_block_average() {
        // 4×2 → 2×1: 2×2 のブロックの平均。
        let src = [
            gray(0.0),
            gray(1.0),
            gray(2.0),
            gray(3.0),
            gray(4.0),
            gray(5.0),
            gray(6.0),
            gray(7.0),
        ];
        let out = downscale_area(&src, 4, 2, 2, 1).unwrap();
        assert_eq!(out.len(), 2);
        assert!((out[0][0] - (0.0 + 1.0 + 4.0 + 5.0) / 4.0).abs() < 1e-6);
        assert!((out[1][0] - (2.0 + 3.0 + 6.0 + 7.0) / 4.0).abs() < 1e-6);
    }

    #[test]
    fn fractional_ratio_weights_by_overlap() {
        // 3 → 2: 出力 0 は [0, 1.5)、出力 1 は [1.5, 3)。
        let src = [gray(0.0), gray(3.0), gray(6.0)];
        let out = downscale_area(&src, 3, 1, 2, 1).unwrap();
        // (0×1 + 3×0.5) / 1.5 = 1、(3×0.5 + 6×1) / 1.5 = 5。
        assert!((out[0][0] - 1.0).abs() < 1e-6, "{out:?}");
        assert!((out[1][0] - 5.0).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn preserves_mean_and_constant() {
        // 一定の画像は一定のまま。平均（全体の積分）も保たれる（面積平均の性質）。
        let (w, h) = (37, 23);
        let src: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let v = ((i * 7919) % 101) as f32 / 100.0;
                [v, 0.5, 1.0 - v]
            })
            .collect();
        for (dw, dh) in [(37, 23), (10, 7), (1, 1), (36, 22), (5, 23), (37, 1)] {
            let out = downscale_area(&src, w, h, dw, dh).unwrap();
            assert_eq!(out.len(), (dw * dh) as usize);
            let mean_src: f64 = src.iter().map(|p| f64::from(p[0])).sum::<f64>() / src.len() as f64;
            let mean_out: f64 = out.iter().map(|p| f64::from(p[0])).sum::<f64>() / out.len() as f64;
            // 出力の画素は同じ面積を覆うので、出力の平均 = 入力の平均。
            assert!((mean_src - mean_out).abs() < 1e-4, "{dw}×{dh}");
            assert!(out.iter().all(|p| (p[1] - 0.5).abs() < 1e-5), "{dw}×{dh}");
        }
    }

    #[test]
    fn same_size_is_identity() {
        let src: Vec<[f32; 3]> = (0..12).map(|i| gray(i as f32)).collect();
        let out = downscale_area(&src, 4, 3, 4, 3).unwrap();
        for (a, b) in src.iter().zip(&out) {
            assert!((a[0] - b[0]).abs() < 1e-6);
        }
    }

    #[test]
    fn checkerboard_does_not_alias() {
        // 1 画素の市松模様を 1/2 や 1/3 に縮小すると、均一な灰色（0.5）になる。
        // 間引き（最近傍）ではモアレや 0 / 1 が残る。
        let (w, h) = (60, 60);
        let src: Vec<[f32; 3]> = (0..h)
            .flat_map(|y| (0..w).map(move |x| gray(((x + y) % 2) as f32)))
            .collect();
        // 1/2: 2×2 のブロックは白と黒が 2 つずつなので、ちょうど 0.5。
        let out = downscale_area(&src, w, h, 30, 30).unwrap();
        assert!(out.iter().all(|p| (p[0] - 0.5).abs() < 1e-6));
        // 1/3: 3×3 のブロックは 5 対 4 なので 4/9 または 5/9（面積平均として正しい値）。
        let out = downscale_area(&src, w, h, 20, 20).unwrap();
        for p in &out {
            let d = (p[0] - 0.5).abs();
            assert!((d - 1.0 / 18.0).abs() < 1e-5, "{p:?}");
        }
        // 倍率が整数でない場合（60 → 7）も 0.5 に近い。
        let out = downscale_area(&src, w, h, 7, 7).unwrap();
        for p in &out {
            assert!((p[0] - 0.5).abs() < 0.01, "{p:?}");
        }
    }

    #[test]
    fn non_finite_inputs_are_zero() {
        let src = [[f32::NAN, f32::INFINITY, 1.0], [1.0, 1.0, 1.0]];
        let out = downscale_area(&src, 2, 1, 1, 1).unwrap();
        assert_eq!(out[0], [0.5, 0.5, 1.0]);
    }

    #[test]
    fn invalid_arguments() {
        assert!(AreaDownscaler::new(2, 2, 3, 1).is_err());
        assert!(AreaDownscaler::new(2, 2, 0, 1).is_err());
        assert!(downscale_area(&[gray(0.0); 3], 2, 2, 1, 1).is_err());
        let mut ds = AreaDownscaler::new(2, 2, 1, 1).unwrap();
        assert_eq!(ds.output_dimensions(), (1, 1));
        assert!(ds.push_row(&[gray(0.0)]).is_err());
        ds.push_row(&[gray(0.0); 2]).unwrap();
        assert!(ds.clone().finish().is_err());
        ds.push_row(&[gray(0.0); 2]).unwrap();
        assert!(ds.push_row(&[gray(0.0); 2]).is_err());
        assert!(ds.finish().is_ok());
    }

    #[test]
    fn encoded_downscale_averages_in_linear_light() {
        // sRGB の 0 と 255 を 1:1 で平均すると、リニアでは 0.5 → 符号化して約 188（0.7354）。
        // 符号化した値のまま平均した 128 にはならない。
        let img = RgbImage8::from_fn(2, 1, |x, _| if x == 0 { [0; 3] } else { [255; 3] }).unwrap();
        let out = downscale_encoded(&DynRgbImage::Rgb8(img), TransferFunction::Srgb, 1, 1)
            .unwrap()
            .to_rgb8();
        let expected = (genzo_color::transfer::srgb_encode(0.5) * 255.0).round() as u8;
        assert_eq!(expected, 188);
        assert_eq!(out.pixel(0, 0), Some([expected; 3]));
        // 16bit も同じ。
        let img16 = RgbImage8::from_fn(4, 2, |x, _| [(x * 60) as u8; 3])
            .unwrap()
            .to_rgb16();
        let out16 = downscale_encoded(
            &DynRgbImage::Rgb16(img16.clone()),
            TransferFunction::Srgb,
            2,
            1,
        )
        .unwrap();
        assert_eq!(out16.bits_per_channel(), 16);
        assert_eq!(out16.dimensions(), (2, 1));
        // 同じ寸法ならそのまま。
        let same = downscale_encoded(
            &DynRgbImage::Rgb16(img16.clone()),
            TransferFunction::Srgb,
            4,
            2,
        )
        .unwrap();
        assert_eq!(same, DynRgbImage::Rgb16(img16));
    }
}
