//! 比較のための縮小（IQ-07b: フル解像度の結果を、決まった縮小フィルタでプレビューと同じ大きさに
//! 縮小してから ΔE2000 で比べる。02 の IQ-07b、レビュー R-02）。
//!
//! [`downscale_area`] は **面積平均**（出力の 1 画素が覆う入力の範囲を、重なった面積で重み付けして
//! 平均する。ボックスフィルタの一般化）で、縮小率が整数でなくてもよい。値は線形に平均するので、
//! **リニアの値（B2・B3）に対して使う**（符号化済みの値を平均すると明るさが変わる）。
//!
//! 位置合わせは「入力と出力の画像の四隅を合わせる」（出力の画素 x は入力の範囲
//! `[x × W/w, (x + 1) × W/w)` に対応する）。境界の外は参照しない。ディザリングはしない。
//!
//! 回転・切り抜きのある画像の IQ-07b の比較では、プレビューとフル解像度の格子の端の丸め方が違い、
//! 四隅を合わせると位置がずれる。そのため比較の基準は、プレビューの各画素が覆うフル解像度の範囲
//! （任意の矩形）の面積平均 [`downscale_footprints`] にする（人の判断の表 No.8 の (b)）。回転・切り抜きが
//! なければ、四隅を合わせた [`downscale_area`] と同じ値。
//!
//! 注意: IQ-07b の「決まった縮小フィルタ」は、プレビューを作る処理（genzo-pipeline）と同じものを
//! 使う必要があり、その方式は PoC-3・PoC-5 で決める。この関数はそれまでの仮の基準
//! （**仮置き**: 位相のずれや折り返しの少ない、簡単で決定的な方式として面積平均を選んだ）である。

use crate::image::{FloatImage, ImageError};

/// 面積平均で `width` × `height` に縮小する。拡大（出力が入力より大きい）はしない。
///
/// **精度**: 重みと足し込みは f64 で計算し、最後に f32 に丸める（比較の基準なので、f32 の和の
/// 丸めの誤差を持ち込まないため）。genzo-pipeline のプレビューの縮小（`genzo_pipeline::resample`）は
/// 同じ面積平均を f32 で足し込むので、両者はビット単位では一致しない（genzo-pipeline の結合テスト
/// `sensor_pipeline.rs` では、差が「絶対 1e−6 ＋ 相対 1e−5」以内であることを確かめている）。
/// ビット単位の一致を確かめる用途には使わない。
pub fn downscale_area(
    image: &FloatImage,
    width: u32,
    height: u32,
) -> Result<FloatImage, ImageError> {
    FloatImage::value_count(width, height, image.channels())?;
    if width > image.width() || height > image.height() {
        return Err(ImageError::InvalidDimensions { width, height });
    }
    let c = image.channels() as usize;
    let (sw, sh) = (image.width() as usize, image.height() as usize);
    let (dw, dh) = (width as usize, height as usize);
    let wx = area_weights(sw, dw);
    let wy = area_weights(sh, dh);

    // 横方向（f64 で計算）: sh 行 × dw 列。
    let mut tmp = vec![0.0_f64; sh * dw * c];
    let src = image.data();
    for y in 0..sh {
        for (ox, taps) in wx.iter().enumerate() {
            let out = &mut tmp[(y * dw + ox) * c..(y * dw + ox + 1) * c];
            for &(i, w) in taps {
                let p = &src[(y * sw + i) * c..(y * sw + i + 1) * c];
                for k in 0..c {
                    out[k] += w * f64::from(p[k]);
                }
            }
        }
    }
    // 縦方向。
    let mut data = vec![0.0_f32; dh * dw * c];
    for (oy, taps) in wy.iter().enumerate() {
        for ox in 0..dw {
            for k in 0..c {
                let mut acc = 0.0_f64;
                for &(j, w) in taps {
                    acc += w * tmp[(j * dw + ox) * c + k];
                }
                data[(oy * dw + ox) * c + k] = acc as f32;
            }
        }
    }
    FloatImage::new(width, height, image.channels(), data)
}

/// 出力の各画素 (x, y) を、入力の画素の座標での矩形 `footprint(x, y) = [x0, y0, x1, y1]`
/// （`[x0, x1) × [y0, y1)`。画素 i は `[i, i + 1)`）の面積平均にする（IQ-07b の比較の基準。人の判断の表
/// No.8: プレビューの各画素が覆うフル解像度の範囲で平均したもの）。
///
/// - 重みは各入力の画素と矩形の重なった面積。矩形が画像の外にはみ出す部分は使わず、画像の内側の
///   重なりの面積で割る（画像の端の画素の平均）。
/// - 矩形が `[x·W/w, (x+1)·W/w)` なら [`downscale_area`] と同じ値（足す順序の違いの丸めを除く）。
/// - 足し込みは f64。矩形が有限でない・空・画像と重ならない場合はエラー。
pub fn downscale_footprints(
    image: &FloatImage,
    width: u32,
    height: u32,
    footprint: impl Fn(u32, u32) -> [f64; 4],
) -> Result<FloatImage, ImageError> {
    FloatImage::value_count(width, height, image.channels())?;
    let c = image.channels() as usize;
    let (sw, sh) = (f64::from(image.width()), f64::from(image.height()));
    let src = image.data();
    let mut data = vec![0.0_f32; width as usize * height as usize * c];
    for y in 0..height {
        for x in 0..width {
            let rect = footprint(x, y);
            if !rect.iter().all(|v| v.is_finite()) {
                return Err(ImageError::InvalidDimensions { width, height });
            }
            let [x0, y0, x1, y1] = rect;
            let (x0, x1) = (x0.max(0.0), x1.min(sw));
            let (y0, y1) = (y0.max(0.0), y1.min(sh));
            if x1 <= x0 || y1 <= y0 {
                return Err(ImageError::InvalidDimensions { width, height });
            }
            let mut acc = vec![0.0_f64; c];
            let mut area = 0.0_f64;
            for j in y0.floor() as u32..(y1.ceil() as u32).min(image.height()) {
                let wy = y1.min(f64::from(j) + 1.0) - y0.max(f64::from(j));
                if wy <= 0.0 {
                    continue;
                }
                for i in x0.floor() as u32..(x1.ceil() as u32).min(image.width()) {
                    let wx = x1.min(f64::from(i) + 1.0) - x0.max(f64::from(i));
                    if wx <= 0.0 {
                        continue;
                    }
                    let w = wx * wy;
                    let k = (j as usize * image.width() as usize + i as usize) * c;
                    for (a, v) in acc.iter_mut().zip(&src[k..k + c]) {
                        *a += w * f64::from(*v);
                    }
                    area += w;
                }
            }
            let o = (y as usize * width as usize + x as usize) * c;
            for (d, a) in data[o..o + c].iter_mut().zip(&acc) {
                *d = (a / area) as f32;
            }
        }
    }
    FloatImage::new(width, height, image.channels(), data)
}

/// 出力の各画素について、入力の画素の番号と重み（和は 1）の列。
fn area_weights(src: usize, dst: usize) -> Vec<Vec<(usize, f64)>> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|o| {
            // 端は整数の比で計算し、丸めの誤差で範囲の外に出ないようにする。
            let start = (o * src) as f64 / dst as f64;
            let end = ((o + 1) * src) as f64 / dst as f64;
            let first = start.floor() as usize;
            let last = (end.ceil() as usize).min(src);
            (first..last)
                .filter_map(|i| {
                    let overlap = end.min(i as f64 + 1.0) - start.max(i as f64);
                    (overlap > 0.0).then_some((i, overlap / scale))
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footprints_match_the_area_downscale_on_the_same_grid_and_clip_at_the_edges() {
        let img = FloatImage::from_rgb_fn(37, 23, |x, y| {
            [x as f32 * 0.1, y as f32 * 0.2, ((x * y) % 7) as f32]
        })
        .unwrap();
        let (w, h) = (10u32, 7u32);
        let a = downscale_area(&img, w, h).unwrap();
        let b = downscale_footprints(&img, w, h, |x, y| {
            let (sx, sy) = (37.0 / f64::from(w), 23.0 / f64::from(h));
            [
                f64::from(x) * sx,
                f64::from(y) * sy,
                f64::from(x + 1) * sx,
                f64::from(y + 1) * sy,
            ]
        })
        .unwrap();
        for (p, q) in a.data().iter().zip(b.data()) {
            assert!((p - q).abs() < 1e-5, "{p} {q}");
        }
        // はみ出した部分は使わない（画像の内側の重なりで平均する）。
        let one = FloatImage::from_gray_fn(4, 1, |x, _| [1.0, 2.0, 4.0, 8.0][x as usize]).unwrap();
        let out = downscale_footprints(&one, 1, 1, |_, _| [-0.5, 0.0, 1.5, 1.0]).unwrap();
        assert!((out.data()[0] - (1.0 + 0.5 * 2.0) / 1.5).abs() < 1e-6);
        // 画像と重ならない範囲・有限でない範囲はエラー。
        assert!(downscale_footprints(&one, 1, 1, |_, _| [5.0, 0.0, 6.0, 1.0]).is_err());
        assert!(downscale_footprints(&one, 1, 1, |_, _| [0.0, 0.0, f64::NAN, 1.0]).is_err());
    }

    #[test]
    fn integer_factor_is_block_mean() {
        let img = FloatImage::from_gray_fn(4, 4, |x, y| (y * 4 + x) as f32).unwrap();
        let out = downscale_area(&img, 2, 2).unwrap();
        // 左上の 2 × 2 は 0, 1, 4, 5 の平均 2.5。
        assert_eq!(out.data(), &[2.5, 4.5, 10.5, 12.5]);
    }

    #[test]
    fn fractional_factor_weights() {
        // 3 → 2: 出力 0 は [0, 1.5)、出力 1 は [1.5, 3)。
        let img = FloatImage::from_gray_fn(3, 1, |x, _| [1.0, 2.0, 4.0][x as usize]).unwrap();
        let out = downscale_area(&img, 2, 1).unwrap();
        let e0 = (1.0 + 0.5 * 2.0) / 1.5;
        let e1 = (0.5 * 2.0 + 4.0) / 1.5;
        assert!((f64::from(out.data()[0]) - e0).abs() < 1e-6);
        assert!((f64::from(out.data()[1]) - e1).abs() < 1e-6);
    }

    #[test]
    fn constant_and_mean_are_preserved() {
        let img = FloatImage::filled(37, 23, &[0.25, 1.5, 3.0]).unwrap();
        let out = downscale_area(&img, 10, 7).unwrap();
        for v in out.data().chunks(3) {
            assert!(
                (v[0] - 0.25).abs() < 1e-6
                    && (v[1] - 1.5).abs() < 1e-6
                    && (v[2] - 3.0).abs() < 1e-6
            );
        }
        // 縮小率が整数で割り切れなくても、全体の平均は保たれる（面積平均）。
        let img =
            FloatImage::from_rgb_fn(37, 23, |x, y| [x as f32, y as f32, (x * y) as f32]).unwrap();
        let out = downscale_area(&img, 10, 7).unwrap();
        let mean = |im: &FloatImage, k: usize| {
            im.data().chunks(3).map(|p| f64::from(p[k])).sum::<f64>() / im.pixel_count() as f64
        };
        for k in 0..3 {
            assert!((mean(&img, k) - mean(&out, k)).abs() < 1e-3, "{k}");
        }
    }

    #[test]
    fn preview_size_ratio() {
        // α7 IV のフル解像度（7008 × 4672）からプレビュー（2560 × 1707）への比 2.7375 の重み。
        let w = area_weights(7008, 2560);
        assert_eq!(w.len(), 2560);
        for taps in &w {
            let s: f64 = taps.iter().map(|t| t.1).sum();
            assert!((s - 1.0).abs() < 1e-12);
            assert!(taps.len() <= 4);
        }
        assert_eq!(w[2559].last().unwrap().0, 7007);
    }

    #[test]
    fn same_size_is_identity_and_upscale_is_rejected() {
        let img = FloatImage::from_rgb_fn(5, 3, |x, y| [x as f32, y as f32, 1.0]).unwrap();
        assert_eq!(downscale_area(&img, 5, 3).unwrap(), img);
        assert!(downscale_area(&img, 6, 3).is_err());
        assert!(downscale_area(&img, 0, 3).is_err());
    }
}
