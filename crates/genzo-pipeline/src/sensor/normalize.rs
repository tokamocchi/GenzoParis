//! ステージ 2: 正規化（docs/04_architecture.md の 2.1 節、2.6 節の B0）。
//!
//! RAW の u16 の値を、CFA の位置ごとの黒レベルを 0、白レベルを 1 とする f32 にする:
//!
//! ```text
//! v = (DN − 黒レベル[位置]) × inv_range[位置]      （inv_range = 1 / (白レベル − 黒レベル)。f64 で計算して f32 に丸めた値）
//! v = min(max(v, 0), 1)
//! DN ≥ 白レベル なら v = 1 とし、飽和の印を付ける
//! ```
//!
//! - 割り算の代わりに逆数を掛けるのは、GPU（WGSL）の割り算が正しく丸められる保証がないため
//!   （掛け算と引き算は正しく丸められる。2.3 節）。
//! - **負の値は 0 に切り詰める**（仮置き）。B1 は「0 以上」（2.6 節）で、RCD の比の計算は負の値を
//!   扱えないため。黒の付近の雑音の平均がわずかに持ち上がる。負の値を残すかは PoC-3 で実写を見て
//!   決める。
//! - 飽和の判定は整数の値で行う（`DN ≥ 白レベル`）。センサーが白レベルより少し低い値で飽和する
//!   機種への対応（閾値を下げるか）は PoC-2 で確認する。
//! - 範囲が RAW の外にはみ出す部分は、RAW の画素を鏡映で折り返して作る（[`crate::border`]。偶奇を
//!   保つので CFA の色の並びは崩れない）。

use genzo_raw::RawImage;
use rayon::prelude::*;

use crate::border::mirror_index;
use crate::error::{PipelineError, Result};
use crate::image::{CfaImage, Roi, checked_len};

/// 正規化の値（RAW の黒レベル・白レベルから作る。GPU 版にも同じ値を渡す）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorLevels {
    /// 黒レベル（CFA の位置ごと。R, G1, B, G2 の順。genzo-raw の `CfaColor::index`）。
    pub black_level: [f32; 4],
    /// 白レベル（この値以上を飽和とする）。
    pub white_level: f32,
    /// `1 / (白レベル − 黒レベル)`（f64 で計算して f32 に丸めた値）。
    pub inv_range: [f32; 4],
}

impl SensorLevels {
    /// RAW の値から作る。黒レベル・白レベルが不正ならエラー（[`RawImage::validate`] と同じ条件）。
    pub fn from_raw(raw: &RawImage) -> Result<Self> {
        let white = raw.white_level;
        let mut inv_range = [0.0f32; 4];
        for (k, &black) in raw.black_level.iter().enumerate() {
            if !(black.is_finite() && white.is_finite() && black >= 0.0 && black < white) {
                return Err(PipelineError::Raw(genzo_raw::RawError::InvalidBlackLevel {
                    index: k,
                    value: black,
                    white_level: white,
                }));
            }
            inv_range[k] = (1.0 / (f64::from(white) - f64::from(black))) as f32;
        }
        Ok(Self {
            black_level: raw.black_level,
            white_level: white,
            inv_range,
        })
    }

    /// 1 画素の正規化（型の doc の式）。戻り値は (値, 飽和したか)。`k` は CFA の位置の番号。
    #[inline]
    pub fn normalize(&self, dn: u16, k: usize) -> (f32, bool) {
        let d = f32::from(dn);
        if d >= self.white_level {
            return (1.0, true);
        }
        let v = (d - self.black_level[k]) * self.inv_range[k];
        // 入力は有限（u16 と検証済みの値）なので、WGSL の clamp（min(max(v, 0), 1)）と同じ結果。
        (v.clamp(0.0, 1.0), false)
    }
}

/// RAW の S 座標の範囲 `roi`（RAW の外にはみ出してよい）を正規化した CFA（B0）を作る。
///
/// RAW は 2 × 2 画素以上であること（鏡映で CFA の色の並びを保つため）。
pub fn normalize(raw: &RawImage, levels: &SensorLevels, roi: Roi) -> Result<CfaImage> {
    let (w, h) = (raw.width, raw.height);
    if w < 2 || h < 2 {
        return Err(PipelineError::InvalidDimensions {
            width: w,
            height: h,
        });
    }
    let expected = u64::from(w) * u64::from(h);
    if raw.data.len() as u64 != expected {
        return Err(PipelineError::DataLengthMismatch {
            expected: expected as usize,
            actual: raw.data.len(),
        });
    }
    let len = checked_len(roi.width, roi.height)?;
    let rw = roi.width as usize;
    let pattern = raw.cfa.shifted(
        i64::from(roi.x).rem_euclid(2) as usize,
        i64::from(roi.y).rem_euclid(2) as usize,
    );
    // 列の折り返しと色の番号は行によらない（偶奇は鏡映で保たれる）ので先に求める。
    let cols: Vec<usize> = (0..roi.width)
        .map(|lx| mirror_index(i64::from(roi.x) + i64::from(lx), w) as usize)
        .collect();
    let mut data = vec![0.0f32; len];
    let mut saturated = vec![false; len];
    data.par_chunks_mut(rw)
        .zip(saturated.par_chunks_mut(rw))
        .enumerate()
        .for_each(|(ly, (drow, srow))| {
            let sy = mirror_index(i64::from(roi.y) + ly as i64, h) as usize;
            let src = &raw.data[sy * w as usize..(sy + 1) * w as usize];
            for (lx, &sx) in cols.iter().enumerate() {
                let k = raw.cfa.color_at(sx, sy).index();
                let (v, s) = levels.normalize(src[sx], k);
                drow[lx] = v;
                srow[lx] = s;
            }
        });
    CfaImage::new(roi, pattern, data, saturated)
}

#[cfg(test)]
mod tests {
    use genzo_model::PhotoMetadata;
    use genzo_raw::{CfaColor, CfaPattern};

    use super::*;

    fn raw(width: u32, height: u32, cfa: CfaPattern, f: impl Fn(u32, u32) -> u16) -> RawImage {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.push(f(x, y));
            }
        }
        RawImage {
            width,
            height,
            cfa,
            data,
            black_level: [512.0, 500.0, 520.0, 510.0],
            white_level: 16383.0,
            as_shot_wb: [2.0, 1.0, 1.5, 1.0],
            cam_xyz: None,
            metadata: PhotoMetadata::default(),
        }
    }

    #[test]
    fn black_and_white_levels_per_position() {
        let r = raw(4, 4, CfaPattern::RGGB, |_, _| 0);
        let lv = SensorLevels::from_raw(&r).unwrap();
        // 黒レベルちょうどは 0、白レベルの手前は 1 未満、白レベル以上は 1 で飽和。
        for k in 0..4 {
            let black = lv.black_level[k] as u16;
            assert_eq!(lv.normalize(black, k), (0.0, false));
            let (v, s) = lv.normalize(16382, k);
            assert!(!s && v < 1.0 && v > 0.9999, "{k}: {v}");
            assert_eq!(lv.normalize(16383, k), (1.0, true));
            assert_eq!(lv.normalize(u16::MAX, k), (1.0, true));
            // 黒レベルより下は 0 に切り詰める。
            assert_eq!(lv.normalize(0, k), (0.0, false));
        }
        // 中間の値: (DN − 黒) / (白 − 黒)。
        let (v, _) = lv.normalize(8447, CfaColor::Red.index());
        let expect = (8447.0 - 512.0) / (16383.0 - 512.0);
        assert!((f64::from(v) - expect).abs() < 1e-7);
        let (v, _) = lv.normalize(8447, CfaColor::Blue.index());
        let expect = (8447.0 - 520.0) / (16383.0 - 520.0);
        assert!((f64::from(v) - expect).abs() < 1e-7);
    }

    #[test]
    fn levels_are_validated() {
        let mut r = raw(2, 2, CfaPattern::RGGB, |_, _| 0);
        r.black_level[3] = 16383.0;
        assert!(SensorLevels::from_raw(&r).is_err());
        r.black_level[3] = f32::NAN;
        assert!(SensorLevels::from_raw(&r).is_err());
    }

    #[test]
    fn whole_image_uses_the_black_level_of_each_position() {
        for cfa in CfaPattern::ALL {
            let r = raw(6, 4, cfa, |x, y| 1000 + (y * 6 + x) as u16);
            let lv = SensorLevels::from_raw(&r).unwrap();
            let img = normalize(&r, &lv, Roi::full(6, 4)).unwrap();
            assert_eq!(img.pattern(), cfa);
            for y in 0..4 {
                for x in 0..6 {
                    let k = cfa.color_at(x as usize, y as usize).index();
                    let dn = f64::from(1000 + (y * 6 + x) as u16);
                    let black = f64::from(r.black_level[k]);
                    let expect = (dn - black) / (16383.0 - black);
                    let v = img.value(x, y).unwrap();
                    assert!((f64::from(v) - expect).abs() < 1e-7, "{cfa} ({x}, {y})");
                }
            }
            assert_eq!(img.saturated_count(), 0);
        }
    }

    #[test]
    fn saturation_is_marked() {
        let r = raw(
            4,
            2,
            CfaPattern::RGGB,
            |x, _| if x == 1 { 16383 } else { 600 },
        );
        let lv = SensorLevels::from_raw(&r).unwrap();
        let img = normalize(&r, &lv, Roi::full(4, 2)).unwrap();
        for y in 0..2 {
            for x in 0..4 {
                assert_eq!(img.is_saturated(x, y), x == 1);
                if x == 1 {
                    assert_eq!(img.value(x, y), Some(1.0));
                }
            }
        }
        assert_eq!(img.saturated_count(), 2);
    }

    #[test]
    fn roi_outside_the_raw_is_mirrored() {
        let r = raw(4, 3, CfaPattern::GRBG, |x, y| 600 + (10 * y + x) as u16);
        let lv = SensorLevels::from_raw(&r).unwrap();
        let roi = Roi::new(-2, -1, 8, 5);
        let img = normalize(&r, &lv, roi).unwrap();
        assert_eq!(img.roi(), roi);
        // 左上 (−2, −1) は偶奇が (0, 1) なので、色の並びは GRBG を (0, 1) ずらしたもの。
        assert_eq!(img.pattern(), CfaPattern::GRBG.shifted(0, 1));
        let full = normalize(&r, &lv, Roi::full(4, 3)).unwrap();
        for ly in 0..5u32 {
            for lx in 0..8u32 {
                let sx = mirror_index(i64::from(lx) - 2, 4);
                let sy = mirror_index(i64::from(ly) - 1, 3);
                assert_eq!(img.value(lx, ly), full.value(sx, sy));
                // 鏡映しても色は同じ（偶奇を保つ）。
                assert_eq!(img.color_at(lx, ly), full.color_at(sx, sy));
            }
        }
    }

    #[test]
    fn tiny_or_inconsistent_raws_are_errors() {
        let r = raw(1, 4, CfaPattern::RGGB, |_, _| 600);
        let lv = SensorLevels::from_raw(&r).unwrap();
        assert!(normalize(&r, &lv, Roi::full(1, 4)).is_err());
        let mut r = raw(2, 2, CfaPattern::RGGB, |_, _| 600);
        r.data.pop();
        assert!(normalize(&r, &lv, Roi::full(2, 2)).is_err());
        let r = raw(2, 2, CfaPattern::RGGB, |_, _| 600);
        assert!(normalize(&r, &lv, Roi::new(0, 0, 0, 2)).is_err());
    }
}
