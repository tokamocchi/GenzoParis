//! 比較明・比較暗合成の計算（DEV-34。docs/composite_design.md の 3 章、決定事項ログ No.177・178）。
//!
//! 入力は、書き出しの経路で現像した **16bit・ディザなし** の RGB（R, G, B, … の並び。出力の色空間に符号化した値。
//! 境界 B4b）。1 枚目を累積のバッファにし、2 枚目からは画素ごとに比べて置き換える（[`Compositor`]）。
//!
//! | モード | 計算 |
//! |---|---|
//! | 比較明 | チャンネルごとの最大 |
//! | 比較暗 | チャンネルごとの最小 |
//! | カラー比較明 | 輝度が **より大きい** ときだけ、その画素の RGB をそのまま採る（同じ輝度なら先の写真のまま） |
//! | カラー比較暗 | 輝度が **より小さい** ときだけ、その画素の RGB をそのまま採る（同じ輝度なら先の写真のまま） |
//! | 平均 | チャンネルごとに、**リニアな値** の平均（符号化した値をリニアに戻して平均し、最後に符号化し直す） |
//!
//! - 平均（docs/composite_design.md の 3.5 節）は、光の量の平均にするためリニアな値で行う（符号化した値のまま
//!   平均すると、明るい部分と暗い部分が混ざる画素が暗く偏る）。累積は f32 の「これまでの平均」で持ち
//!   （`mean += (x - mean) / n`）、同じ画像を何枚重ねても元の画像に戻る（符号化し直しは f64 で行い、16bit の
//!   すべての値が元に戻ることをテストで確かめる）。結果は写真の順によらない（丸めの差を除く）。
//! - 伝達関数は単調に増えるので、チャンネルごとの最大・最小は、符号化した値で比べてもリニアの値で比べても同じ
//!   （符号化した値のまま比べる）。
//! - 輝度は、符号化した値をリニアに戻し、出力の色空間の輝度の係数（RGB → XYZ の行列の 2 行目）で求める。累積の
//!   バッファの画素の輝度は別のバッファ（f32）に持ち、毎回は計算しない。Photoshop の「カラー比較（明）」と同じ式かは
//!   確かめていない（docs/composite_design.md の 3.1 節）。
//! - 比較明・比較暗の結果は、写真の順によらない。カラー比較明・暗は、同じ輝度の画素で先の写真を採るので順による
//!   （呼び出し側が撮影日時の順に並べる）。
//! - 量子化（8bit の形式）は、合成の後で 1 回だけ呼び出し側がかける（ディザをかけた画像を比較明で重ねると、
//!   プラス側の雑音ばかりが選ばれて明るく偏るため。3.2 節）。

use genzo_model::{CompositeMode, OutputColorSpace};
use rayon::prelude::*;

use crate::error::{PipelineError, Result};
use crate::output_sharpen::rgb_space;

/// 並列に処理する単位（画素の数）。
const CHUNK_PIXELS: usize = 1 << 16;

/// 16bit の符号化した値から輝度を求める表と係数。
struct Luminance {
    /// 16bit の符号化した値 → リニアな値。
    decode: Vec<f32>,
    coefficients: [f32; 3],
}

impl Luminance {
    fn new(space: OutputColorSpace) -> Self {
        let rgb = rgb_space(space);
        let transfer = rgb.transfer_function();
        let decode = (0..=u16::MAX)
            .map(|v| transfer.decode_f32(f32::from(v) / 65535.0))
            .collect();
        let c = rgb.luminance_coefficients();
        Self {
            decode,
            coefficients: [c[0] as f32, c[1] as f32, c[2] as f32],
        }
    }

    #[inline]
    fn of(&self, p: &[u16]) -> f32 {
        self.coefficients[0] * self.decode[usize::from(p[0])]
            + self.coefficients[1] * self.decode[usize::from(p[1])]
            + self.coefficients[2] * self.decode[usize::from(p[2])]
    }
}

/// 合成の途中の状態（累積のバッファ）。
pub struct Compositor {
    mode: CompositeMode,
    width: u32,
    height: u32,
    /// 累積のバッファ（16bit の RGB）。
    acc: Vec<u16>,
    /// 累積のバッファの画素の輝度（カラー比較明・暗のときだけ）。
    luma: Option<(Luminance, Vec<f32>)>,
    /// 平均のときだけ: 16bit の符号化した値 → リニアな値の表と、これまでの平均（リニア。R, G, B, …）。
    /// 平均のときは `acc` を使わない（空）。
    mean: Option<(Vec<f32>, genzo_color::TransferFunction, Vec<f32>)>,
    /// 重ねた枚数（1 枚目を含む）。
    count: usize,
}

impl std::fmt::Debug for Compositor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compositor")
            .field("mode", &self.mode)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

fn expected_len(width: u32, height: u32) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(PipelineError::InvalidDimensions { width, height });
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(3))
        .ok_or(PipelineError::InvalidDimensions { width, height })
}

fn check_len(expected: usize, actual: usize) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(PipelineError::DataLengthMismatch { expected, actual })
    }
}

impl Compositor {
    /// 1 枚目（`first`。`width` × `height` の 16bit の RGB。出力の色空間 `space` に符号化した値）から始める。
    pub fn new(
        mode: CompositeMode,
        space: OutputColorSpace,
        width: u32,
        height: u32,
        first: Vec<u16>,
    ) -> Result<Self> {
        check_len(expected_len(width, height)?, first.len())?;
        if mode.averages() {
            let transfer = rgb_space(space).transfer_function();
            let decode: Vec<f32> = (0..=u16::MAX)
                .map(|v| transfer.decode(f64::from(v) / 65535.0) as f32)
                .collect();
            let values = first.par_iter().map(|&v| decode[usize::from(v)]).collect();
            return Ok(Self {
                mode,
                width,
                height,
                acc: Vec::new(),
                luma: None,
                mean: Some((decode, transfer, values)),
                count: 1,
            });
        }
        let luma = mode.compares_luminance().then(|| {
            let table = Luminance::new(space);
            let values = first.par_chunks(3).map(|p| table.of(p)).collect();
            (table, values)
        });
        Ok(Self {
            mode,
            width,
            height,
            acc: first,
            luma,
            mean: None,
            count: 1,
        })
    }

    /// 寸法（幅・高さ）。
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 重ねた枚数（1 枚目を含む）。
    pub fn count(&self) -> usize {
        self.count
    }

    /// 次の 1 枚（同じ寸法・同じ色空間の 16bit の RGB）を重ねる。
    pub fn add(&mut self, src: &[u16]) -> Result<()> {
        const CHUNK: usize = CHUNK_PIXELS * 3;
        if let Some((decode, _, mean)) = &mut self.mean {
            check_len(mean.len(), src.len())?;
            // これまでの平均に、n 枚目を足す（n = 重ねた後の枚数）。
            let n = (self.count + 1) as f32;
            mean.par_chunks_mut(CHUNK)
                .zip(src.par_chunks(CHUNK))
                .for_each(|(m, s)| {
                    m.iter_mut()
                        .zip(s)
                        .for_each(|(m, &s)| *m += (decode[usize::from(s)] - *m) / n)
                });
            self.count += 1;
            return Ok(());
        }
        check_len(self.acc.len(), src.len())?;
        match (self.mode, &mut self.luma) {
            (CompositeMode::Lighten, _) => self
                .acc
                .par_chunks_mut(CHUNK)
                .zip(src.par_chunks(CHUNK))
                .for_each(|(a, s)| a.iter_mut().zip(s).for_each(|(a, &s)| *a = (*a).max(s))),
            (CompositeMode::Darken, _) => self
                .acc
                .par_chunks_mut(CHUNK)
                .zip(src.par_chunks(CHUNK))
                .for_each(|(a, s)| a.iter_mut().zip(s).for_each(|(a, &s)| *a = (*a).min(s))),
            (
                mode @ (CompositeMode::LighterColor | CompositeMode::DarkerColor),
                Some((table, y)),
            ) => {
                let lighter = mode == CompositeMode::LighterColor;
                self.acc
                    .par_chunks_mut(CHUNK)
                    .zip(src.par_chunks(CHUNK))
                    .zip(y.par_chunks_mut(CHUNK_PIXELS))
                    .for_each(|((a, s), y)| {
                        for ((a, s), y) in a.chunks_exact_mut(3).zip(s.chunks_exact(3)).zip(y) {
                            let ys = table.of(s);
                            let take = if lighter { ys > *y } else { ys < *y };
                            if take {
                                a.copy_from_slice(s);
                                *y = ys;
                            }
                        }
                    });
            }
            (CompositeMode::LighterColor | CompositeMode::DarkerColor, None) => {
                unreachable!("輝度で比べるモードでは、輝度のバッファを作ってある")
            }
            (CompositeMode::Average, _) => unreachable!("平均は上で処理してある"),
        }
        self.count += 1;
        Ok(())
    }

    /// 合成の結果（16bit の RGB）。
    pub fn finish(self) -> Vec<u16> {
        match self.mean {
            // 平均: リニアな値を符号化し直す（f64 で計算して、16bit の値に丸める）。
            Some((_, transfer, mean)) => mean
                .par_iter()
                .map(|&m| {
                    let e = transfer.encode(f64::from(m).clamp(0.0, 1.0));
                    (e * 65535.0).round().clamp(0.0, 65535.0) as u16
                })
                .collect(),
            None => self.acc,
        }
    }
}

/// 累積のバッファが使うメモリの見込み（バイト。`width` × `height` の画像。カラー比較明・暗は輝度のバッファを含む。
/// 平均は、リニアな値の平均のバッファ（f32）と、最後に作る 16bit の結果）。
pub fn compositor_bytes(mode: CompositeMode, width: u32, height: u32) -> u64 {
    let pixels = u64::from(width) * u64::from(height);
    let rgb = pixels * 3 * 2;
    if mode.averages() {
        rgb + pixels * 3 * 4
    } else if mode.compares_luminance() {
        rgb + pixels * 4
    } else {
        rgb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODES: [CompositeMode; 4] = [
        CompositeMode::Lighten,
        CompositeMode::Darken,
        CompositeMode::LighterColor,
        CompositeMode::DarkerColor,
    ];

    fn run(mode: CompositeMode, images: &[Vec<u16>], width: u32, height: u32) -> Vec<u16> {
        let mut c = Compositor::new(
            mode,
            OutputColorSpace::AdobeRgb,
            width,
            height,
            images[0].clone(),
        )
        .unwrap();
        for img in &images[1..] {
            c.add(img).unwrap();
        }
        assert_eq!(c.count(), images.len());
        c.finish()
    }

    /// 決まった擬似乱数（xorshift）で作った画像。
    fn noise(seed: u64, len: usize) -> Vec<u16> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 48) as u16
            })
            .collect()
    }

    /// 平均: 同じ画像を重ねても元の画像に戻る（16bit のすべての値。どの色空間でも）。
    #[test]
    fn average_of_identical_images_is_the_image() {
        let all: Vec<u16> = (0..=u16::MAX).collect();
        // 3 の倍数の長さにする（65536 個 + 2 個）。
        let mut image = all.clone();
        image.extend([0, 65535]);
        let width = (image.len() / 3) as u32;
        for space in [
            OutputColorSpace::Srgb,
            OutputColorSpace::DisplayP3,
            OutputColorSpace::AdobeRgb,
        ] {
            let mut c =
                Compositor::new(CompositeMode::Average, space, width, 1, image.clone()).unwrap();
            assert_eq!(c.size(), (width, 1));
            for _ in 0..4 {
                c.add(&image).unwrap();
            }
            assert_eq!(c.count(), 5);
            assert_eq!(c.finish(), image, "{space:?}");
        }
    }

    /// 平均: リニアな値で平均する（符号化した値の平均より明るい）。写真の順によらず、ノイズが減る。
    #[test]
    fn average_is_taken_in_linear_light() {
        // 黒と白の平均は、リニアで 0.5（sRGB の符号化で約 0.7354 → 16bit で 48193 前後）。
        let black = vec![0u16; 3];
        let white = vec![65535u16; 3];
        let run_srgb = |images: &[Vec<u16>]| {
            let mut c = Compositor::new(
                CompositeMode::Average,
                OutputColorSpace::Srgb,
                1,
                1,
                images[0].clone(),
            )
            .unwrap();
            for img in &images[1..] {
                c.add(img).unwrap();
            }
            c.finish()
        };
        let mid = run_srgb(&[black.clone(), white.clone()]);
        let expected = (genzo_color::TransferFunction::Srgb.encode(0.5) * 65535.0).round() as u16;
        assert_eq!(mid, vec![expected; 3]);
        assert!((48100..48300).contains(&expected), "{expected}");
        assert_eq!(run_srgb(&[white.clone(), black.clone()]), mid);
        // 3 枚: 白が 1 枚なら、リニアで 1/3。
        let third = run_srgb(&[black.clone(), white, black]);
        let expected3 =
            (genzo_color::TransferFunction::Srgb.encode(1.0 / 3.0) * 65535.0).round() as u16;
        assert!(
            third.iter().all(|v| v.abs_diff(expected3) <= 1),
            "{third:?}"
        );

        // ノイズのある画像を重ねると、元の値（中間の灰色）からのずれが小さくなる。
        let len = 3 * 64 * 64;
        let base = 30000i32;
        let frames: Vec<Vec<u16>> = (1..=16)
            .map(|seed| {
                noise(seed, len)
                    .into_iter()
                    .map(|n| (base + i32::from(n % 4001) - 2000) as u16)
                    .collect()
            })
            .collect();
        let spread = |img: &[u16]| {
            img.iter()
                .map(|&v| (f64::from(v) - f64::from(base)).powi(2))
                .sum::<f64>()
                / img.len() as f64
        };
        let averaged = run(CompositeMode::Average, &frames, 64, 64);
        assert!(
            spread(&averaged) < spread(&frames[0]) / 8.0,
            "{} / {}",
            spread(&averaged),
            spread(&frames[0])
        );
    }

    #[test]
    fn lighten_and_darken_are_per_channel_max_and_min() {
        let a = vec![100, 5000, 30000, 0, 65535, 7];
        let b = vec![200, 4000, 30000, 1, 0, 9];
        assert_eq!(
            run(CompositeMode::Lighten, &[a.clone(), b.clone()], 2, 1),
            vec![200, 5000, 30000, 1, 65535, 9]
        );
        assert_eq!(
            run(CompositeMode::Darken, &[a, b], 2, 1),
            vec![100, 4000, 30000, 0, 0, 7]
        );
    }

    /// 比較明・比較暗は順によらない（大きな画像で、並列の区切りをまたぐ）。
    #[test]
    fn lighten_and_darken_do_not_depend_on_the_order() {
        let (w, h) = (300, 250);
        let len = (w * h * 3) as usize;
        let imgs: Vec<Vec<u16>> = (1..=4).map(|s| noise(s, len)).collect();
        let mut rev = imgs.clone();
        rev.reverse();
        for mode in [CompositeMode::Lighten, CompositeMode::Darken] {
            let r = run(mode, &imgs, w, h);
            assert_eq!(r, run(mode, &rev, w, h), "{mode:?}");
            // 素朴な計算と同じ。
            let expect: Vec<u16> = (0..len)
                .map(|i| {
                    let it = imgs.iter().map(|img| img[i]);
                    if mode == CompositeMode::Lighten {
                        it.max().unwrap()
                    } else {
                        it.min().unwrap()
                    }
                })
                .collect();
            assert_eq!(r, expect, "{mode:?}");
        }
    }

    /// 同じ画像を重ねても変わらない（どのモードでも）。
    #[test]
    fn compositing_the_same_image_changes_nothing() {
        let img = noise(7, 40 * 30 * 3);
        for mode in MODES {
            assert_eq!(
                run(mode, &[img.clone(), img.clone(), img.clone()], 40, 30),
                img
            );
        }
    }

    /// カラー比較明・暗は、輝度の大きい・小さい画素の RGB をそのまま採る（チャンネルを混ぜない）。
    #[test]
    fn color_modes_take_whole_pixels_by_luminance() {
        // 1 画素目: 赤（a）と緑（b）。Adobe RGB の輝度の係数は緑が最も大きいので、明るさの近い値でも緑の方が明るい。
        // 2 画素目: 暗い灰色（a）と明るい灰色（b）。
        let a = vec![60000, 0, 0, 1000, 1000, 1000];
        let b = vec![0, 50000, 0, 40000, 40000, 40000];
        let lighter = run(CompositeMode::LighterColor, &[a.clone(), b.clone()], 2, 1);
        assert_eq!(lighter, vec![0, 50000, 0, 40000, 40000, 40000]);
        let darker = run(CompositeMode::DarkerColor, &[a.clone(), b.clone()], 2, 1);
        assert_eq!(darker, vec![60000, 0, 0, 1000, 1000, 1000]);
        // 比較明はチャンネルを混ぜる（赤と緑で黄色）。
        let lighten = run(CompositeMode::Lighten, &[a, b], 2, 1);
        assert_eq!(&lighten[..3], &[60000, 50000, 0]);
    }

    /// 同じ輝度なら先の写真を採る。
    #[test]
    fn color_modes_keep_the_earlier_pixel_on_ties() {
        let a = vec![30000, 30000, 30000];
        let b = vec![30000, 30000, 30000];
        let c = vec![30000, 30000, 30001];
        // a と b は同じ値なので、結果は a（= b）。輝度が少しだけ大きい c は比較明で採り、比較暗では採らない。
        assert_eq!(
            run(
                CompositeMode::LighterColor,
                &[a.clone(), b.clone(), c.clone()],
                1,
                1
            ),
            c
        );
        assert_eq!(run(CompositeMode::DarkerColor, &[a.clone(), b, c], 1, 1), a);
    }

    /// 輝度のバッファは、置き換えた画素で更新する（3 枚目の比較は 2 枚目で置き換えた値と比べる）。
    #[test]
    fn color_modes_compare_with_the_current_result() {
        let dark = vec![1000, 1000, 1000];
        let mid = vec![20000, 20000, 20000];
        let bright_red = vec![65535, 0, 0];
        // 赤の輝度（Adobe RGB の係数で約 0.30）は、灰色の 20000（リニアで約 0.06）より明るい。
        let r = run(
            CompositeMode::LighterColor,
            &[dark, mid, bright_red.clone()],
            1,
            1,
        );
        assert_eq!(r, bright_red);
    }

    #[test]
    fn sizes_and_lengths_are_checked() {
        assert!(matches!(
            Compositor::new(CompositeMode::Lighten, OutputColorSpace::Srgb, 0, 1, vec![]),
            Err(PipelineError::InvalidDimensions { .. })
        ));
        assert!(matches!(
            Compositor::new(
                CompositeMode::Lighten,
                OutputColorSpace::Srgb,
                2,
                1,
                vec![0; 5]
            ),
            Err(PipelineError::DataLengthMismatch { .. })
        ));
        let mut c = Compositor::new(
            CompositeMode::Lighten,
            OutputColorSpace::Srgb,
            2,
            1,
            vec![0; 6],
        )
        .unwrap();
        assert!(matches!(
            c.add(&[0; 3]),
            Err(PipelineError::DataLengthMismatch { .. })
        ));
        assert_eq!(c.count(), 1);
        assert_eq!(c.size(), (2, 1));
    }

    #[test]
    fn memory_estimate() {
        // α7 IV（7008 × 4672）: RGB 16bit は 196,448,256 バイト、輝度のバッファ（f32）は 130,965,504 バイト。
        assert_eq!(
            compositor_bytes(CompositeMode::Lighten, 7008, 4672),
            196_448_256
        );
        assert_eq!(
            compositor_bytes(CompositeMode::LighterColor, 7008, 4672),
            196_448_256 + 130_965_504
        );
        // 平均: リニアな値の平均のバッファ（f32 × 3）は 392,896,512 バイト。
        assert_eq!(
            compositor_bytes(CompositeMode::Average, 7008, 4672),
            196_448_256 + 392_896_512
        );
    }
}
