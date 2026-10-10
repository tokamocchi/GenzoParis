//! 書き出し（01 の EXP-01、docs/04_architecture.md の 2.4 節・2.6 節の B4b、02 の IQ-06・IQ-08・SCL-05）。
//!
//! [`ExportSettings`] の寸法・出力の色空間・ビット深度に従って、フル解像度のタイル処理
//! （[`super::full`]）→（縮小）→ ステージ 17b → 量子化を行い、量子化したバッファと、埋め込む ICC
//! プロファイル（genzo-color の [`IccProfile`]）を返す。ファイルへの書き込み（JPEG / TIFF / PNG の
//! 符号化・Exif・安全な書き出し）は genzo-media・genzo-api の担当で、ここでは行わない。
//!
//! # 処理の順序
//!
//! | 寸法 | 順序 |
//! |---|---|
//! | `ExportSize::Original` | タイルごとに: ステージ 2〜16（B3）→ 17b（B4b）→ 量子化 → 出力のバッファの位置に書く |
//! | `ExportSize::LongEdge(n)`（出力より小さい） | **フル解像度で処理してから縮小**（2.4 節）: 出力の行の帯ごとに、ステージ 2〜16（B3。タイル）→ 面積平均で縮小（リニアの B3 に対して。[`crate::resample`] と同じ定義）→ 17b → 量子化 |
//!
//! - 縮小のフィルタは **面積平均**（出力の画素 x は入力の `[x·W/w, (x+1)·W/w)` を面積で重み付けした平均。
//!   genzo-testkit の `downscale_area` と同じ定義）。リニアの値（B3）に対して使い、符号化した値には使わない。
//!   縮小した寸法は `ExportSize::fit`（四捨五入、拡大はしない）。genzo-media の書き出しも同じ寸法の
//!   規則なので、ここで縮小したバッファを渡せば genzo-media は縮小し直さない。
//! - 縮小は 17b の色域の圧縮（非線形）の前に行う。結果は「フル解像度の B3 全体を縮小してから 17b」と
//!   ビット単位で同じ（帯に分けても変わらない。テストで確認する）。
//! - 量子化のディザリング（[`Dither`]）のノイズは、書き出す画像全体での座標で決まる（タイル・帯の
//!   分け方によらない。IQ-08）。8bit の既定は三角分布（[`Dither::DEFAULT_8BIT`]）、16bit の既定は
//!   なし。
//! - 向き（Orientation）はステージ 9 で画素に反映済み。書き出すファイルの Exif の向きは「通常」に
//!   すること（genzo-media の担当）。
//!
//! # メモリ（SCL-05）
//!
//! フル解像度の浮動小数点のバッファは持たない。持つのは、量子化した出力のバッファ（8bit で 1 画素
//! 3 バイト、16bit で 6 バイト）と、タイル 1 つ分の作業領域、縮小のときは帯 1 つ分の横方向の和だけ。

use genzo_color::{IccProfile, IccVersion, StandardProfile};
use genzo_model::{DevelopSettings, ExportSettings, OutputColorSpace};

use super::control::RenderControl;
use super::full::FullResolutionJob;
use super::{Engine, OutputTarget, PhotoSource, blit};
use crate::error::{PipelineError, Result};
use crate::finish::UnimplementedSetting;
use crate::finish::output::{Dither, quantize_u8, quantize_u16};
use crate::image::Roi;
use crate::sensor::SensorWarning;

/// 書き出すファイルに埋め込む ICC プロファイルのバージョン（genzo-color の
/// [`genzo_color::EXPORT_ICC_VERSION`]。genzo-media の `EXPORT_ICC_VERSION` と同じ定数で、理由は
/// genzo-color の doc。PoC-1 の色の比較（IQ-06）で見直す）。
pub const EXPORT_ICC_VERSION: IccVersion = genzo_color::EXPORT_ICC_VERSION;

/// 出力の色空間の ICC プロファイル（[`EXPORT_ICC_VERSION`]）。
pub fn export_icc_profile(space: OutputColorSpace) -> Result<IccProfile> {
    let kind = match space {
        OutputColorSpace::Srgb => StandardProfile::Srgb,
        OutputColorSpace::DisplayP3 => StandardProfile::DisplayP3,
        OutputColorSpace::AdobeRgb => StandardProfile::AdobeRgb1998,
    };
    Ok(IccProfile::standard_with_version(kind, EXPORT_ICC_VERSION)?)
}

/// 書き出しの量子化などの設定（[`ExportSettings`] にない、処理の側の設定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportOptions {
    /// 8bit のディザリング（既定は [`Dither::DEFAULT_8BIT`]）。
    pub dither_8bit: Dither,
    /// 16bit のディザリング（既定はなし）。
    pub dither_16bit: Dither,
    /// タイルの一辺（`None` ならエンジンの設定）。
    pub tile_size: Option<u32>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            dither_8bit: Dither::DEFAULT_8BIT,
            dither_16bit: Dither::None,
            tile_size: None,
        }
    }
}

/// 量子化した画素（R, G, B, R, G, B, … の並び。行優先）。
#[derive(Clone, PartialEq, Eq)]
pub enum ExportPixels {
    /// 8bit（JPEG・PNG 8bit）。
    Rgb8(Vec<u8>),
    /// 16bit（TIFF・PNG 16bit）。
    Rgb16(Vec<u16>),
}

impl std::fmt::Debug for ExportPixels {
    /// 値の列は表示しない（大きいため）。ビット数と値の数だけ。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ExportPixels {{ bits: {}, len: {} }}",
            self.bits_per_channel(),
            self.len()
        )
    }
}

impl ExportPixels {
    /// 1 チャンネルあたりのビット数。
    pub fn bits_per_channel(&self) -> u8 {
        match self {
            ExportPixels::Rgb8(_) => 8,
            ExportPixels::Rgb16(_) => 16,
        }
    }

    /// 値の数（画素数 × 3）。
    pub fn len(&self) -> usize {
        match self {
            ExportPixels::Rgb8(v) => v.len(),
            ExportPixels::Rgb16(v) => v.len(),
        }
    }

    /// 空か。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 書き出しの結果（ファイルに書く前のもの）。
#[derive(Debug, Clone)]
pub struct ExportedImage {
    /// 幅。
    pub width: u32,
    /// 高さ。
    pub height: u32,
    /// 量子化した画素（B4b を 8bit / 16bit にしたもの）。
    pub pixels: ExportPixels,
    /// 出力の色空間。
    pub color_space: OutputColorSpace,
    /// 埋め込む ICC プロファイル（IQ-06）。
    pub icc: IccProfile,
    /// 縮小する前の寸法（C の格子、フル解像度）。
    pub full_resolution_size: (u32, u32),
    /// センサー処理の警告。
    pub warnings: Vec<SensorWarning>,
    /// 値が入っているが適用しなかった項目（v1）。
    pub unimplemented: Vec<UnimplementedSetting>,
    /// 置き換えた NaN・無限大の数。
    pub non_finite_replaced: u64,
}

/// 量子化の出力のバッファ。
enum Buffer {
    U8(Vec<u8>, Dither),
    U16(Vec<u16>, Dither),
}

impl Buffer {
    /// B4b のタイル（範囲は書き出す画像の座標）を量子化して書く。
    fn put(&mut self, full: Roi, tile: &crate::image::ImageTile) -> Result<()> {
        let origin = (i64::from(tile.roi.x), i64::from(tile.roi.y));
        match self {
            Buffer::U8(buf, d) => {
                let q = quantize_u8(&tile.image, origin, *d)?;
                blit(buf, full, tile.roi, &q, 3);
            }
            Buffer::U16(buf, d) => {
                let q = quantize_u16(&tile.image, origin, *d)?;
                blit(buf, full, tile.roi, &q, 3);
            }
        }
        Ok(())
    }

    fn into_pixels(self) -> ExportPixels {
        match self {
            Buffer::U8(v, _) => ExportPixels::Rgb8(v),
            Buffer::U16(v, _) => ExportPixels::Rgb16(v),
        }
    }
}

impl FullResolutionJob {
    /// 書き出す（モジュールの doc の順序）。設定が不正（JPEG の品質・長辺の範囲）ならエラー。
    pub fn export(
        &self,
        export: &ExportSettings,
        options: &ExportOptions,
        control: &dyn RenderControl,
    ) -> Result<ExportedImage> {
        export
            .validate()
            .map_err(|e| PipelineError::InvalidArgument(e.to_string()))?;
        let tile_size = options.tile_size.unwrap_or(self.tile_size());
        let (w, h) = self.output_size();
        let (dw, dh) = export.size.fit(w, h);
        let full = Roi::full(dw, dh);
        let len = full.pixel_count() as usize * 3;
        let mut buffer = match export.format.bits_per_channel() {
            8 => Buffer::U8(vec![0; len], options.dither_8bit),
            _ => Buffer::U16(vec![0; len], options.dither_16bit),
        };
        let target = OutputTarget::Export(export.color_space);
        let mut extra = 0;
        let mut emit = |b3: crate::image::ImageTile| -> Result<()> {
            let (encoded, n) = self.apply_output(&target, &b3)?;
            extra += n;
            buffer.put(full, &encoded)
        };
        let replaced = if (dw, dh) == (w, h) {
            self.for_each_tile(full, tile_size, control, &mut emit)?
        } else {
            self.for_each_downscaled_band((dw, dh), tile_size, control, &mut emit)?
        };
        Ok(ExportedImage {
            width: dw,
            height: dh,
            pixels: buffer.into_pixels(),
            color_space: export.color_space,
            icc: export_icc_profile(export.color_space)?,
            full_resolution_size: (w, h),
            warnings: self.warnings().to_vec(),
            unimplemented: self.unimplemented().to_vec(),
            non_finite_replaced: replaced + extra,
        })
    }
}

impl Engine {
    /// 書き出す（EXP-01。[`FullResolutionJob::export`]）。
    pub fn export(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        export: &ExportSettings,
        options: &ExportOptions,
        control: &dyn RenderControl,
    ) -> Result<ExportedImage> {
        // 設定の誤りは、重い準備（ガイドなど）の前に知らせる。
        export
            .validate()
            .map_err(|e| PipelineError::InvalidArgument(e.to_string()))?;
        self.full_resolution(source, settings)?
            .export(export, options, control)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icc_profiles_for_each_space() {
        for (space, needle) in [
            (OutputColorSpace::Srgb, "sRGB"),
            (OutputColorSpace::DisplayP3, "Display P3"),
            (OutputColorSpace::AdobeRgb, "Adobe RGB"),
        ] {
            let p = export_icc_profile(space).unwrap();
            assert!(p.is_rgb());
            assert_eq!(p.version().0, 2);
            assert!(p.description().unwrap().contains(needle), "{space:?}");
        }
    }

    #[test]
    fn pixels_helpers() {
        let p = ExportPixels::Rgb16(vec![0; 6]);
        assert_eq!(p.bits_per_channel(), 16);
        assert_eq!(p.len(), 6);
        assert!(!p.is_empty());
        assert!(ExportPixels::Rgb8(Vec::new()).is_empty());
        assert_eq!(format!("{p:?}"), "ExportPixels { bits: 16, len: 6 }");
        let o = ExportOptions::default();
        assert_eq!(o.dither_8bit, Dither::DEFAULT_8BIT);
        assert_eq!(o.dither_16bit, Dither::None);
    }
}
