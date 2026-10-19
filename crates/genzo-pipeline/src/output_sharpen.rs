//! 出力シャープ（EXP-03。docs/04_architecture.md の 2.4 節「書き出しの仕上げ」）。
//!
//! 書き出しの **縮小の後**（ステージ 17b で出力の色空間に符号化し、16bit に量子化した画素）に、出力の画素の大きさに
//! 合わせてかけるアンシャープマスク。現像設定ではなく書き出しの設定（genzo-model の [`OutputSharpening`]）なので、
//! 処理バージョンに入れない。GPU 版・CPU 版のどちらで現像しても、この CPU の処理を同じようにかける（GPU 版は
//! 作らない。決定事項ログ No.166）。
//!
//! # 計算（DEV-12 のシャープの部品を使う。[`crate::finish::sharpen`]・[`crate::finish::frequency`]）
//!
//! ```text
//! c  = 出力の色空間のリニアな値（16bit の符号化した値を伝達関数で戻す）
//! Y  = 出力の色空間の輝度（原色の行列の 2 行目。sRGB なら BT.709 の係数）
//! p  = log2(max(Y, 0) + LUMA_LOG_OFFSET)、 b = p をガウスでぼかしたもの（σ = 出力の画素。下の表）
//! h  = SharpenStageParams::shaped(p − b)    （しきい値 t = OUTPUT_SHARPEN_THRESHOLD・にじみの抑え L = OUTPUT_SHARPEN_LIMIT）
//! Δ  = a · h                               （a = 下の表の適用量）
//! k  = 2^Δ、 Ŷ = max(Y, 0) + LUMA_LOG_OFFSET、 Y' = Y + Ŷ·(k − 1)、 c' = Y' + k·(c − Y)
//! 出力 = clamp(c', 0, 1) を符号化して 16bit に丸める
//! ```
//!
//! 輝度の対数の差に同じ倍率をかけるだけなので、色の比を保ち、色の縁を強めない（DEV-12 と同じ当て方）。`Δ = 0` の画素
//! （平らな部分）は値が変わらない（符号化し直しもしない）。
//!
//! | 対象 | σ（出力の画素） | 適用量 a（弱・標準・強） |
//! |---|---|---|
//! | 画面 | [`OUTPUT_SHARPEN_SCREEN_SIGMA`]（0.6） | 0.25・0.5・0.8 |
//! | マット紙 | [`OUTPUT_SHARPEN_MATTE_SIGMA_MM`]（0.12 mm）× 解像度 ÷ 25.4 | 0.45・0.8・1.2 |
//! | 光沢紙 | [`OUTPUT_SHARPEN_GLOSSY_SIGMA_MM`]（0.08 mm）× 解像度 ÷ 25.4 | 0.35・0.6・0.9 |
//!
//! σ は [`OUTPUT_SHARPEN_MIN_SIGMA`]〜[`OUTPUT_SHARPEN_MAX_SIGMA`] に収める。どの値も **仮置き**（合成の画像だけで
//! 決めた。Lightroom Classic の出力シャープの値は公開されていない。実写の印刷・画面で見て決める。人の判断の表 No.175）。
//!
//! # メモリ（SCL-05）
//!
//! 画像全体の浮動小数点のバッファは持たない。[`BAND_ROWS`] 行の帯ごとに、帯を半径だけ広げた輝度の対数の面を作って
//! ぼかし、その帯の画素を置き換える（上の余白の行は、前の帯で作った元の値の面を残して使う）。帯の分け方で結果は
//! 変わらない（テストで確認）。画像の端の外は端の画素を繰り返す。

use genzo_color::{RgbColorSpace, TransferFunction};
use genzo_model::{OutputColorSpace, OutputSharpening, SharpenAmount, SharpenMedia};
use rayon::prelude::*;

use crate::error::{PipelineError, Result};
use crate::finish::frequency::{GaussKernel, LUMA_LOG_OFFSET, blur_plane};
use crate::finish::sharpen::SharpenStageParams;
use crate::image::Roi;

/// 画面の σ（出力の画素）。仮置き。
pub const OUTPUT_SHARPEN_SCREEN_SIGMA: f64 = 0.6;
/// マット紙の σ（ミリメートル。解像度で画素に直す）。仮置き。
pub const OUTPUT_SHARPEN_MATTE_SIGMA_MM: f64 = 0.12;
/// 光沢紙の σ（ミリメートル）。仮置き。
pub const OUTPUT_SHARPEN_GLOSSY_SIGMA_MM: f64 = 0.08;
/// σ の下限（出力の画素）。
pub const OUTPUT_SHARPEN_MIN_SIGMA: f64 = 0.5;
/// σ の上限（出力の画素）。
pub const OUTPUT_SHARPEN_MAX_SIGMA: f64 = 4.0;
/// しきい値 t（段）。これより小さな明暗の差（雑音）はほとんど強めない。仮置き。
pub const OUTPUT_SHARPEN_THRESHOLD: f32 = 0.02;
/// にじみの抑え L（段）。仮置き。
pub const OUTPUT_SHARPEN_LIMIT: f32 = 0.5;
/// 帯の行の数（σ の上限の半径（12 画素）より大きいこと）。
pub const BAND_ROWS: u32 = 64;

/// 出力シャープのパラメータ（[`OutputSharpening`] と解像度から決めたもの）。
#[derive(Debug, Clone, PartialEq)]
pub struct OutputSharpenParams {
    /// ぼかしの σ（出力の画素）。
    pub sigma: f64,
    /// 適用量 a（段の差の倍率）。
    pub amount: f32,
}

/// 対象と強さの適用量（モジュールの doc の表）。
pub const fn output_sharpen_amount(media: SharpenMedia, amount: SharpenAmount) -> f32 {
    match (media, amount) {
        (SharpenMedia::Screen, SharpenAmount::Low) => 0.25,
        (SharpenMedia::Screen, SharpenAmount::Standard) => 0.5,
        (SharpenMedia::Screen, SharpenAmount::High) => 0.8,
        (SharpenMedia::MattePaper, SharpenAmount::Low) => 0.45,
        (SharpenMedia::MattePaper, SharpenAmount::Standard) => 0.8,
        (SharpenMedia::MattePaper, SharpenAmount::High) => 1.2,
        (SharpenMedia::GlossyPaper, SharpenAmount::Low) => 0.35,
        (SharpenMedia::GlossyPaper, SharpenAmount::Standard) => 0.6,
        (SharpenMedia::GlossyPaper, SharpenAmount::High) => 0.9,
    }
}

impl OutputSharpenParams {
    /// 設定と解像度（ppi）から決める。印刷用は、紙の上の大きさ（mm）を解像度で出力の画素に直す。
    pub fn new(settings: &OutputSharpening, resolution_ppi: u32) -> Self {
        let mm = match settings.media {
            SharpenMedia::Screen => None,
            SharpenMedia::MattePaper => Some(OUTPUT_SHARPEN_MATTE_SIGMA_MM),
            SharpenMedia::GlossyPaper => Some(OUTPUT_SHARPEN_GLOSSY_SIGMA_MM),
        };
        let sigma = match mm {
            None => OUTPUT_SHARPEN_SCREEN_SIGMA,
            Some(mm) => mm * f64::from(resolution_ppi.max(1)) / 25.4,
        };
        Self {
            sigma: sigma.clamp(OUTPUT_SHARPEN_MIN_SIGMA, OUTPUT_SHARPEN_MAX_SIGMA),
            amount: output_sharpen_amount(settings.media, settings.amount),
        }
    }
}

/// 書き出しの色空間の原色・伝達関数（合成（[`crate::composite`]）でも使う）。
pub(crate) fn rgb_space(space: OutputColorSpace) -> RgbColorSpace {
    match space {
        OutputColorSpace::Srgb => RgbColorSpace::Srgb,
        OutputColorSpace::DisplayP3 => RgbColorSpace::DisplayP3,
        OutputColorSpace::AdobeRgb => RgbColorSpace::AdobeRgb1998,
    }
}

/// 画素ごとの計算に使う表と係数。
struct Pixels {
    /// 16bit の符号化した値 → リニアな値。
    decode: Vec<f32>,
    transfer: TransferFunction,
    luma: [f32; 3],
}

impl Pixels {
    fn new(space: OutputColorSpace) -> Self {
        let rgb = rgb_space(space);
        let transfer = rgb.transfer_function();
        let decode = (0..=u16::MAX)
            .map(|v| transfer.decode_f32(f32::from(v) / 65535.0))
            .collect();
        let c = rgb.luminance_coefficients();
        Self {
            decode,
            transfer,
            luma: [c[0] as f32, c[1] as f32, c[2] as f32],
        }
    }

    #[inline]
    fn linear(&self, p: &[u16]) -> [f32; 3] {
        [
            self.decode[usize::from(p[0])],
            self.decode[usize::from(p[1])],
            self.decode[usize::from(p[2])],
        ]
    }

    #[inline]
    fn luminance(&self, c: [f32; 3]) -> f32 {
        self.luma[0] * c[0] + self.luma[1] * c[1] + self.luma[2] * c[2]
    }

    #[inline]
    fn luma_log(&self, p: &[u16]) -> f32 {
        (self.luminance(self.linear(p)).max(0.0) + LUMA_LOG_OFFSET).log2()
    }

    #[inline]
    fn encode(&self, v: f32) -> u16 {
        let e = self.transfer.encode_f32(v.clamp(0.0, 1.0));
        (e * 65535.0 + 0.5).floor().clamp(0.0, 65535.0) as u16
    }
}

/// 16bit の RGB（R, G, B, … の並び。出力の色空間 `space` に符号化した値）に出力シャープをかける（その場で置き換える）。
pub fn sharpen_rgb16(
    data: &mut [u16],
    width: u32,
    height: u32,
    space: OutputColorSpace,
    params: &OutputSharpenParams,
) -> Result<()> {
    sharpen_rgb16_banded(data, width, height, space, params, BAND_ROWS)
}

fn sharpen_rgb16_banded(
    data: &mut [u16],
    width: u32,
    height: u32,
    space: OutputColorSpace,
    params: &OutputSharpenParams,
    band_rows: u32,
) -> Result<()> {
    let expected = u64::from(width) * u64::from(height) * 3;
    if data.len() as u64 != expected {
        return Err(PipelineError::InvalidArgument(format!(
            "出力シャープの画素の数が寸法と合いません（{} 個、{width}×{height}）",
            data.len()
        )));
    }
    if width == 0 || height == 0 || params.amount == 0.0 {
        return Ok(());
    }
    if !params.sigma.is_finite() || params.sigma <= 0.0 {
        return Err(PipelineError::InvalidArgument(format!(
            "出力シャープの σ が正しくありません（{}）",
            params.sigma
        )));
    }
    let kernel = GaussKernel::new(params.sigma);
    let r = kernel.radius;
    if band_rows < r || band_rows == 0 {
        return Err(PipelineError::InvalidArgument(format!(
            "帯の行の数（{band_rows}）がぼかしの半径（{r}）より小さい"
        )));
    }
    // DEV-12 のしきい値・にじみの抑えの式（マスクはかけない）。
    let shape = SharpenStageParams {
        amount: params.amount,
        threshold: OUTPUT_SHARPEN_THRESHOLD,
        limit: OUTPUT_SHARPEN_LIMIT,
        mask_edge: 0.0,
        gradient_scale: 1.0,
        kernel: kernel.clone(),
        amount_unit: params.amount,
        local: None,
    };
    let px = Pixels::new(space);
    let w = width as usize;
    let pw = w + 2 * r as usize;
    // 前の帯の最後の r 行の、元の値の輝度の対数（横の余白を含む）。
    let mut tail: Vec<f32> = Vec::new();
    let mut y0 = 0u32;
    while y0 < height {
        let bh = band_rows.min(height - y0);
        let out = Roi::new(0, y0 as i32, width, bh);
        let plane_roi = out.expand(r);
        let ph = plane_roi.height as usize;
        let mut plane = vec![0.0f32; pw * ph];
        {
            let src: &[u16] = data;
            plane.par_chunks_mut(pw).enumerate().for_each(|(j, row)| {
                let y = i64::from(plane_roi.y) + j as i64;
                let sy = y.clamp(0, i64::from(height) - 1) as u32;
                if sy < y0 {
                    // 前の帯で置き換えた行: 残しておいた元の値を使う（帯は半径より高いので、必ず前の帯の最後の r 行）。
                    let k = (y - (i64::from(y0) - i64::from(r))) as usize;
                    row.copy_from_slice(&tail[k * pw..(k + 1) * pw]);
                    return;
                }
                let line = &src[sy as usize * w * 3..(sy as usize + 1) * w * 3];
                for (i, dst) in row.iter_mut().enumerate() {
                    let sx = (i as i64 - i64::from(r)).clamp(0, w as i64 - 1) as usize;
                    *dst = px.luma_log(&line[sx * 3..sx * 3 + 3]);
                }
            });
        }
        let blurred = blur_plane(&plane, plane_roi, out, &kernel)?;
        // 次の帯のために、この帯の最後の r 行（元の値）を残す。
        tail.clear();
        tail.extend_from_slice(&plane[bh as usize * pw..(bh as usize + r as usize) * pw]);
        let rows = &mut data[y0 as usize * w * 3..(y0 + bh) as usize * w * 3];
        rows.par_chunks_mut(w * 3)
            .zip(blurred.par_chunks(w))
            .enumerate()
            .for_each(|(j, (line, b))| {
                let prow = &plane[(j + r as usize) * pw..(j + r as usize + 1) * pw];
                for (i, p) in line.chunks_exact_mut(3).enumerate() {
                    let delta = shape.amount * shape.shaped(prow[i + r as usize] - b[i]);
                    if delta == 0.0 {
                        continue;
                    }
                    let c = px.linear(p);
                    let y = px.luminance(c);
                    let k = delta.exp2();
                    let yh = y.max(0.0) + LUMA_LOG_OFFSET;
                    let y2 = y + yh * (k - 1.0);
                    for ch in 0..3 {
                        p[ch] = px.encode(y2 + k * (c[ch] - y));
                    }
                }
            });
        y0 += bh;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(media: SharpenMedia, amount: SharpenAmount, ppi: u32) -> OutputSharpenParams {
        OutputSharpenParams::new(&OutputSharpening { media, amount }, ppi)
    }

    /// 縦の段差（左が暗く右が明るい）の 16bit の画像。
    fn step(width: u32, height: u32, dark: u16, bright: u16) -> Vec<u16> {
        let mut v = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..height {
            for x in 0..width {
                let g = if x < width / 2 { dark } else { bright };
                v.extend_from_slice(&[g, g, g]);
            }
        }
        v
    }

    #[test]
    fn sigma_follows_media_and_resolution() {
        let screen = params(SharpenMedia::Screen, SharpenAmount::Standard, 300);
        assert_eq!(screen.sigma, OUTPUT_SHARPEN_SCREEN_SIGMA);
        assert_eq!(screen.amount, 0.5);
        // マット紙 300 ppi: 0.12 mm × 300 / 25.4 ≈ 1.417 画素。
        let matte = params(SharpenMedia::MattePaper, SharpenAmount::High, 300);
        assert!(
            (matte.sigma - 0.12 * 300.0 / 25.4).abs() < 1e-12,
            "{}",
            matte.sigma
        );
        assert_eq!(matte.amount, 1.2);
        // 光沢紙 240 ppi ≈ 0.756 画素。72 ppi では下限の 0.5。1200 ppi では上限の手前（3.78）。
        let glossy = params(SharpenMedia::GlossyPaper, SharpenAmount::Low, 240);
        assert!((glossy.sigma - 0.08 * 240.0 / 25.4).abs() < 1e-12);
        assert_eq!(
            params(SharpenMedia::GlossyPaper, SharpenAmount::Low, 72).sigma,
            OUTPUT_SHARPEN_MIN_SIGMA
        );
        assert!(
            params(SharpenMedia::GlossyPaper, SharpenAmount::Low, 1200).sigma
                < OUTPUT_SHARPEN_MAX_SIGMA
        );
        assert_eq!(
            params(SharpenMedia::MattePaper, SharpenAmount::Low, 1200).sigma,
            OUTPUT_SHARPEN_MAX_SIGMA
        );
        // 強いほど適用量が大きい。
        for m in [
            SharpenMedia::Screen,
            SharpenMedia::MattePaper,
            SharpenMedia::GlossyPaper,
        ] {
            let a = |x| output_sharpen_amount(m, x);
            assert!(a(SharpenAmount::Low) < a(SharpenAmount::Standard));
            assert!(a(SharpenAmount::Standard) < a(SharpenAmount::High));
        }
    }

    #[test]
    fn flat_image_is_unchanged() {
        let mut v = vec![30_000u16; 40 * 30 * 3];
        let p = params(SharpenMedia::Screen, SharpenAmount::High, 72);
        sharpen_rgb16(&mut v, 40, 30, OutputColorSpace::Srgb, &p).unwrap();
        assert!(v.iter().all(|&x| x == 30_000));
    }

    #[test]
    fn edges_get_more_contrast_and_stay_neutral() {
        let (w, h) = (32u32, 8u32);
        let orig = step(w, h, 16_000, 40_000);
        for space in [
            OutputColorSpace::Srgb,
            OutputColorSpace::DisplayP3,
            OutputColorSpace::AdobeRgb,
        ] {
            let mut v = orig.clone();
            let p = params(SharpenMedia::Screen, SharpenAmount::Standard, 72);
            sharpen_rgb16(&mut v, w, h, space, &p).unwrap();
            let at =
                |x: u32, y: u32| &v[((y * w + x) * 3) as usize..((y * w + x) * 3 + 3) as usize];
            for y in 0..h {
                // 段差のすぐ隣は、暗い側がより暗く、明るい側がより明るい。
                assert!(
                    at(w / 2 - 1, y)[0] < 16_000,
                    "{space:?} {:?}",
                    at(w / 2 - 1, y)
                );
                assert!(at(w / 2, y)[0] > 40_000, "{space:?}");
                // 遠い画素は変わらない。
                assert_eq!(at(2, y), &[16_000; 3]);
                assert_eq!(at(w - 3, y), &[40_000; 3]);
                // 無彩色のまま（色の比を保つ）。
                for x in 0..w {
                    let c = at(x, y);
                    assert!(
                        c[0].abs_diff(c[1]) <= 1 && c[1].abs_diff(c[2]) <= 1,
                        "{c:?}"
                    );
                }
            }
        }
        // 強いほど差が大きい。
        let diff = |a: SharpenAmount| {
            let mut v = orig.clone();
            sharpen_rgb16(
                &mut v,
                w,
                h,
                OutputColorSpace::Srgb,
                &params(SharpenMedia::Screen, a, 72),
            )
            .unwrap();
            v[((w / 2) * 3) as usize] - v[((w / 2 - 1) * 3) as usize]
        };
        assert!(diff(SharpenAmount::Low) < diff(SharpenAmount::Standard));
        assert!(diff(SharpenAmount::Standard) < diff(SharpenAmount::High));
    }

    /// 帯の分け方によらず同じ結果（帯の境目の行で、前の帯が置き換えた値を使わない）。
    #[test]
    fn bands_do_not_change_the_result() {
        let (w, h) = (23u32, 61u32);
        let mut orig = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 7919 + y * 104_729) % 60_000 + 2_000) as u16;
                orig.extend_from_slice(&[v, v / 2 + 1000, 65_535 - v]);
            }
        }
        let p = params(SharpenMedia::MattePaper, SharpenAmount::High, 600);
        let mut whole = orig.clone();
        sharpen_rgb16_banded(&mut whole, w, h, OutputColorSpace::AdobeRgb, &p, h).unwrap();
        assert_ne!(whole, orig);
        let r = GaussKernel::new(p.sigma).radius;
        for band in [r, r + 1, 16, 30] {
            let mut v = orig.clone();
            sharpen_rgb16_banded(&mut v, w, h, OutputColorSpace::AdobeRgb, &p, band).unwrap();
            assert_eq!(v, whole, "帯 {band} 行");
        }
        // 半径より低い帯は使えない。
        let mut v = orig.clone();
        assert!(sharpen_rgb16_banded(&mut v, w, h, OutputColorSpace::Srgb, &p, r - 1).is_err());
    }

    #[test]
    fn invalid_sizes_are_rejected() {
        let p = params(SharpenMedia::Screen, SharpenAmount::Low, 72);
        let mut v = vec![0u16; 10];
        assert!(sharpen_rgb16(&mut v, 2, 2, OutputColorSpace::Srgb, &p).is_err());
        let mut empty: Vec<u16> = Vec::new();
        sharpen_rgb16(&mut empty, 0, 5, OutputColorSpace::Srgb, &p).unwrap();
        // 1 画素の画像（帯の余白はすべて端の繰り返し）。
        let mut one = vec![1000u16, 2000, 3000];
        sharpen_rgb16(&mut one, 1, 1, OutputColorSpace::Srgb, &p).unwrap();
        assert_eq!(one, vec![1000, 2000, 3000]);
    }
}
