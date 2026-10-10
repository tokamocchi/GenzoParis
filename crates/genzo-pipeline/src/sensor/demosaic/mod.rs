//! ステージ 5: デモザイク（docs/04_architecture.md の 2.1 節、2.2 節。02 の IQ-03）。
//!
//! CFA（B1 の値。WB 適用済み）から RGB（B1。カメラ RGB）を作る。
//!
//! | 方式 | 用途 | 余白 | 出力の格子 |
//! |---|---|---|---|
//! | [`DemosaicMethod::Rcd`] | 最終品質（既定。[`rcd`]） | [`rcd::RCD_MARGIN`] 画素 | S（フル解像度） |
//! | [`DemosaicMethod::Bilinear`] | 比較用（[`bilinear`]） | 1 画素 | S |
//! | [`DemosaicMethod::Half2x2`] | WB のドラッグ中の簡易処理（[`half`]。2.2 節） | なし（入力は出力の 2 倍の範囲） | S の半分の解像度 |
//!
//! # 範囲と端の扱い
//!
//! デモザイクの関数は、出力の範囲 `out_roi` に必要な入力の範囲（[`DemosaicMethod::input_roi`]）を
//! 含む CFA を受け取り、その中だけを読む。画像の端の外は、入力の CFA を作るとき（ステージ 2 の
//! [`super::normalize::normalize`]）に鏡映で拡張しておく（2.7 節）。そのため、タイルに分けても
//! 分けなくても、各画素の結果は同じ（ビット単位で一致する）。CFA の画像だけがある場合は
//! [`pad_cfa_mirror`] で鏡映の余白を付けてから呼ぶ（[`demosaic_full`]）。

pub mod bilinear;
pub mod half;
pub mod rcd;

use genzo_model::RenderQuality;

use crate::border::mirror_index;
use crate::contract::ColorContract;
use crate::error::{PipelineError, Result};
use crate::image::{CfaImage, ImageTile, RgbImage, Roi, checked_len};
use crate::version::ProcessVersion;

/// デモザイクの方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DemosaicMethod {
    /// RCD（Ratio Corrected Demosaicing）。最終品質。
    Rcd,
    /// 双線形補間。比較用。
    Bilinear,
    /// 2 × 2 の画素をまとめる簡易処理（ベイヤー配列の 4 画素から 1 画素の RGB。G は 2 つの平均）。
    Half2x2,
}

impl DemosaicMethod {
    /// すべての方式。
    pub const ALL: [DemosaicMethod; 3] = [
        DemosaicMethod::Rcd,
        DemosaicMethod::Bilinear,
        DemosaicMethod::Half2x2,
    ];

    /// 処理バージョンと品質から決まる方式（v1: 最終品質は RCD、簡易は 2 × 2。2.2 節）。
    pub const fn for_quality(version: ProcessVersion, quality: RenderQuality) -> Self {
        match version {
            ProcessVersion::V1 => match quality {
                RenderQuality::Final => DemosaicMethod::Rcd,
                RenderQuality::Draft => DemosaicMethod::Half2x2,
            },
        }
    }

    /// ステージの ID（GPU 版の検索と一致テストのキー）。
    pub const fn stage_id(self) -> &'static str {
        match self {
            DemosaicMethod::Rcd => "sensor.demosaic.rcd",
            DemosaicMethod::Bilinear => "sensor.demosaic.bilinear",
            DemosaicMethod::Half2x2 => "sensor.demosaic.half2x2",
        }
    }

    /// 出力の範囲の外に必要な入力の余白（S の画素）。2 × 2 の簡易処理は 0。
    pub const fn margin(self) -> u32 {
        match self {
            DemosaicMethod::Rcd => rcd::RCD_MARGIN,
            DemosaicMethod::Bilinear => bilinear::BILINEAR_MARGIN,
            DemosaicMethod::Half2x2 => 0,
        }
    }

    /// 出力の範囲（出力の格子）を計算するのに必要な入力の CFA の範囲（S 座標）。
    pub fn input_roi(self, output_roi: Roi) -> Roi {
        match self {
            DemosaicMethod::Rcd | DemosaicMethod::Bilinear => output_roi.expand(self.margin()),
            DemosaicMethod::Half2x2 => output_roi.doubled(),
        }
    }

    /// S の寸法 `width × height` の画像に対する出力の格子の寸法（2 × 2 の簡易処理は半分。端数は
    /// 切り捨て）。
    pub const fn output_grid(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            DemosaicMethod::Rcd | DemosaicMethod::Bilinear => (width, height),
            DemosaicMethod::Half2x2 => (width / 2, height / 2),
        }
    }

    /// 出力の格子の 1 画素が覆う S の画素の数（一辺）。
    pub const fn sensor_pixels_per_output(self) -> u32 {
        match self {
            DemosaicMethod::Rcd | DemosaicMethod::Bilinear => 1,
            DemosaicMethod::Half2x2 => 2,
        }
    }
}

/// CFA から RGB（B1）を作る。`cfa` は `method.input_roi(out_roi)` を含むこと。
pub fn demosaic(cfa: &CfaImage, method: DemosaicMethod, out_roi: Roi) -> Result<ImageTile> {
    checked_len(out_roi.width, out_roi.height)?;
    let need = method.input_roi(out_roi);
    if !cfa.roi().contains_roi(&need) {
        return Err(PipelineError::InvalidRoi {
            roi: cfa.roi(),
            reason: "デモザイクの入力の CFA が必要な範囲を覆っていない",
        });
    }
    let data = match method {
        DemosaicMethod::Rcd => rcd::rcd(cfa, out_roi),
        DemosaicMethod::Bilinear => bilinear::bilinear(cfa, out_roi),
        DemosaicMethod::Half2x2 => half::half(cfa, out_roi),
    };
    ImageTile::from_image(
        out_roi,
        RgbImage::from_vec(
            out_roi.width,
            out_roi.height,
            ColorContract::B1CameraRgb,
            data,
        )?,
    )
}

/// CFA の画像の周りに `margin` 画素の余白を鏡映で付ける（[`crate::border`]。CFA の色の並びを保つ）。
///
/// 余白は `cfa` 自身の範囲を画像全体とみなして折り返す。`cfa` は 2 × 2 画素以上であること。
pub fn pad_cfa_mirror(cfa: &CfaImage, margin: u32) -> Result<CfaImage> {
    let (w, h) = (cfa.width(), cfa.height());
    if w < 2 || h < 2 {
        return Err(PipelineError::InvalidDimensions {
            width: w,
            height: h,
        });
    }
    let roi = cfa.roi().expand(margin);
    let len = checked_len(roi.width, roi.height)?;
    let mut data = Vec::with_capacity(len);
    let mut saturated = Vec::with_capacity(len);
    let m = i64::from(margin);
    for y in 0..i64::from(roi.height) {
        let sy = mirror_index(y - m, h) as usize;
        for x in 0..i64::from(roi.width) {
            let sx = mirror_index(x - m, w) as usize;
            let i = sy * w as usize + sx;
            data.push(cfa.data()[i]);
            saturated.push(cfa.saturated()[i]);
        }
    }
    // 余白の左上は元の左上から (−margin, −margin)。偶奇は鏡映で保たれるので、色の並びは
    // 元の並びを margin の偶奇だけずらしたもの。
    let shift = (margin % 2) as usize;
    CfaImage::new(roi, cfa.pattern().shifted(shift, shift), data, saturated)
}

/// CFA の画像全体をデモザイクする（端の外は鏡映。テストと比較用）。2 × 2 の簡易処理では、
/// `cfa` の左上の S 座標が偶数であること（出力の格子は S の半分）。
pub fn demosaic_full(cfa: &CfaImage, method: DemosaicMethod) -> Result<ImageTile> {
    let roi = cfa.roi();
    match method {
        DemosaicMethod::Half2x2 => {
            if roi.x % 2 != 0 || roi.y % 2 != 0 {
                return Err(PipelineError::InvalidRoi {
                    roi,
                    reason: "2 × 2 の簡易処理の CFA の左上は偶数の座標が必要",
                });
            }
            let (gw, gh) = method.output_grid(roi.width, roi.height);
            demosaic(cfa, method, Roi::new(roi.x / 2, roi.y / 2, gw, gh))
        }
        DemosaicMethod::Rcd | DemosaicMethod::Bilinear => {
            let padded = pad_cfa_mirror(cfa, method.margin())?;
            demosaic(&padded, method, roi)
        }
    }
}

/// CFA の色の並びを、R = 0・G = 1・B = 2 の 2 × 2 の表にする（`table[y & 1][x & 1]`）。
pub(crate) fn channel_table(pattern: genzo_raw::CfaPattern) -> [[u8; 2]; 2] {
    let c = |x, y| pattern.color_at(x, y).rgb_channel() as u8;
    [[c(0, 0), c(1, 0)], [c(0, 1), c(1, 1)]]
}

#[cfg(test)]
mod tests {
    use genzo_raw::CfaPattern;

    use super::*;

    #[test]
    fn method_properties() {
        assert_eq!(
            DemosaicMethod::for_quality(ProcessVersion::V1, RenderQuality::Final),
            DemosaicMethod::Rcd
        );
        assert_eq!(
            DemosaicMethod::for_quality(ProcessVersion::V1, RenderQuality::Draft),
            DemosaicMethod::Half2x2
        );
        let ids: std::collections::HashSet<_> =
            DemosaicMethod::ALL.iter().map(|m| m.stage_id()).collect();
        assert_eq!(ids.len(), 3);
        let r = Roi::new(4, 6, 10, 8);
        assert_eq!(DemosaicMethod::Bilinear.input_roi(r), r.expand(1));
        assert_eq!(DemosaicMethod::Rcd.input_roi(r), r.expand(rcd::RCD_MARGIN));
        assert_eq!(
            DemosaicMethod::Half2x2.input_roi(r),
            Roi::new(8, 12, 20, 16)
        );
        assert_eq!(DemosaicMethod::Half2x2.output_grid(7, 5), (3, 2));
        assert_eq!(DemosaicMethod::Rcd.output_grid(7, 5), (7, 5));
        assert_eq!(DemosaicMethod::Half2x2.sensor_pixels_per_output(), 2);
    }

    #[test]
    fn padding_mirrors_values_and_keeps_the_pattern() {
        for p in CfaPattern::ALL {
            let cfa =
                CfaImage::from_fn(Roi::new(4, 6, 5, 4), p, |x, y| (10 * y + x) as f32).unwrap();
            for margin in [1, 2, 3] {
                let padded = pad_cfa_mirror(&cfa, margin).unwrap();
                assert_eq!(padded.roi(), cfa.roi().expand(margin));
                for y in 0..padded.height() {
                    for x in 0..padded.width() {
                        let sx = mirror_index(i64::from(x) - i64::from(margin), 5);
                        let sy = mirror_index(i64::from(y) - i64::from(margin), 4);
                        assert_eq!(padded.value(x, y), cfa.value(sx, sy));
                        assert_eq!(padded.color_at(x, y), cfa.color_at(sx, sy), "{p} {margin}");
                    }
                }
            }
        }
        let tiny = CfaImage::from_fn(Roi::full(1, 4), CfaPattern::RGGB, |_, _| 0.0).unwrap();
        assert!(pad_cfa_mirror(&tiny, 2).is_err());
    }

    #[test]
    fn input_must_cover_the_needed_range() {
        let cfa = CfaImage::from_fn(Roi::full(8, 8), CfaPattern::RGGB, |_, _| 0.5).unwrap();
        for m in [DemosaicMethod::Rcd, DemosaicMethod::Bilinear] {
            assert!(matches!(
                demosaic(&cfa, m, Roi::full(8, 8)),
                Err(PipelineError::InvalidRoi { .. })
            ));
        }
        assert!(demosaic(&cfa, DemosaicMethod::Half2x2, Roi::full(4, 4)).is_ok());
        assert!(demosaic(&cfa, DemosaicMethod::Half2x2, Roi::full(5, 4)).is_err());
        let odd = CfaImage::from_fn(Roi::new(1, 0, 8, 8), CfaPattern::GRBG, |_, _| 0.5).unwrap();
        assert!(demosaic_full(&odd, DemosaicMethod::Half2x2).is_err());
    }

    #[test]
    fn channel_table_follows_the_pattern() {
        assert_eq!(channel_table(CfaPattern::RGGB), [[0, 1], [1, 2]]);
        assert_eq!(channel_table(CfaPattern::BGGR), [[2, 1], [1, 0]]);
        assert_eq!(channel_table(CfaPattern::GRBG), [[1, 0], [2, 1]]);
        assert_eq!(channel_table(CfaPattern::GBRG), [[1, 2], [0, 1]]);
    }
}
