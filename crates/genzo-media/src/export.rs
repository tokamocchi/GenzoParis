//! 書き出し（EXP-01・EXP-04、04 の 2.4 節・2.6 節の B4b・6.4 節）。
//!
//! 入力は、パイプラインのステージ 17b で **出力の色空間（`settings.color_space`）に変換・符号化
//! 済み** の 8bit / 16bit の画像。この関数群は、
//!
//! 1. 寸法（`settings.size`）: 画像が指定の長辺より大きければ、伝達関数でリニアに戻して面積平均で
//!    縮小し、符号化し直す（[`crate::resize::downscale_encoded`]）。縮小の後の量子化は四捨五入で、
//!    ディザリングはしない（8bit の書き出しでも、パイプラインから 16bit で渡せば、縮小の後に 1 回だけ
//!    8bit に丸めることになる）。
//! 2. ビット数を形式に合わせる（JPEG・PNG 8bit は 8bit、TIFF・PNG 16bit は 16bit）。
//! 3. 出力の色空間の ICC プロファイル（[`export_icc_profile`]）と Exif を埋め込む（IQ-06・EXP-04）。
//!    GPS は `settings.remove_gps = false` のときだけ書く（SEC-03）。
//! 4. 安全に書き出す（原本の照合・一時ファイル・名前の変更。[`crate::safe_write`]）。
//!
//! を行う。

use std::io::{BufWriter, Write};
use std::path::Path;

use genzo_color::{IccProfile, IccVersion, StandardProfile, TransferFunction};
use genzo_model::{ExportFormat, ExportSettings, OutputColorSpace};

use crate::buffer::DynRgbImage;
use crate::error::{MediaError, Result};
use crate::exif_write::{ByteOrder, ExifData, ExifImageInfo, build_ifds, encode_exif_tiff};
use crate::jpeg::encode_jpeg;
use crate::png_io::encode_png_to;
use crate::resize::downscale_encoded;
use crate::safe_write::{ProtectedFiles, WriteOutcome, write_atomically};
use crate::tiff_io::encode_tiff16_to;

/// 書き出すファイルに埋め込む ICC プロファイルのバージョン（仮置き。genzo-color の
/// [`genzo_color::EXPORT_ICC_VERSION`] と同じ値で、理由はそちらの doc。genzo-pipeline も同じ定数を使う）。
pub const EXPORT_ICC_VERSION: IccVersion = genzo_color::EXPORT_ICC_VERSION;

/// 出力の色空間に対応する標準のプロファイル。
pub const fn standard_profile_for(space: OutputColorSpace) -> StandardProfile {
    match space {
        OutputColorSpace::Srgb => StandardProfile::Srgb,
        OutputColorSpace::DisplayP3 => StandardProfile::DisplayP3,
        OutputColorSpace::AdobeRgb => StandardProfile::AdobeRgb1998,
    }
}

/// 出力の色空間の伝達関数（2.6 節の B4b）。
pub fn transfer_function_for(space: OutputColorSpace) -> TransferFunction {
    standard_profile_for(space).transfer_function()
}

/// 書き出すファイルに埋め込む ICC プロファイル（[`EXPORT_ICC_VERSION`]）。
pub fn export_icc_profile(space: OutputColorSpace) -> Result<IccProfile> {
    crate::profiles::standard(standard_profile_for(space), EXPORT_ICC_VERSION)
}

/// 書き出しの前処理（寸法とビット数）を済ませた画像を、形式に合わせて `w` に書く。
///
/// `exif` が `Some` なら Exif を書く。GPS を書くかどうかは `exif` の内容に従う（設定の
/// `remove_gps` の適用は [`encode_export`]・[`export_image`] が行う）。
pub fn encode_export_to<W: Write + std::io::Seek>(
    image: &DynRgbImage,
    format: ExportFormat,
    color_space: OutputColorSpace,
    exif: Option<&ExifData>,
    w: &mut W,
) -> Result<()> {
    let icc = export_icc_profile(color_space)?;
    let (width, height) = image.dimensions();
    let ifds = exif.map(|e| {
        build_ifds(
            e,
            &ExifImageInfo {
                width,
                height,
                color_space,
            },
        )
    });
    // JPEG・PNG の Exif はビッグエンディアンで書く（どちらでもよいが、固定して決定的にする）。
    let exif_tiff = ifds
        .as_ref()
        .map(|i| encode_exif_tiff(i, ByteOrder::Big))
        .transpose()?;
    match format {
        ExportFormat::Jpeg { quality } => {
            let bytes = encode_jpeg(
                &image.to_rgb8(),
                quality,
                Some(icc.as_bytes()),
                exif_tiff.as_deref(),
            )?;
            w.write_all(&bytes).map_err(MediaError::io_no_path)
        }
        ExportFormat::Png8 | ExportFormat::Png16 => {
            // BufWriter は drop のときの書き込みの失敗を無視するので、最後に明示的に flush する。
            let mut bw = BufWriter::new(w);
            if format == ExportFormat::Png8 {
                encode_png_to(
                    &image.to_rgb8(),
                    Some(icc.as_bytes()),
                    exif_tiff.as_deref(),
                    &mut bw,
                )?;
            } else {
                encode_png_to(
                    &image.to_rgb16(),
                    Some(icc.as_bytes()),
                    exif_tiff.as_deref(),
                    &mut bw,
                )?;
            }
            bw.flush().map_err(MediaError::io_no_path)
        }
        ExportFormat::Tiff16 => {
            let mut bw = BufWriter::new(w);
            encode_tiff16_to(
                &image.to_rgb16(),
                Some(icc.as_bytes()),
                ifds.as_ref(),
                &mut bw,
            )?;
            bw.flush().map_err(MediaError::io_no_path)
        }
    }
}

/// 設定の寸法（`settings.size`）に合わせて縮小した画像を返す（必要なければ複製しない）。
pub fn fit_to_export_size<'a>(
    image: &'a DynRgbImage,
    settings: &ExportSettings,
) -> Result<std::borrow::Cow<'a, DynRgbImage>> {
    let (w, h) = image.dimensions();
    let (dw, dh) = settings.size.fit(w, h);
    if (dw, dh) == (w, h) {
        return Ok(std::borrow::Cow::Borrowed(image));
    }
    Ok(std::borrow::Cow::Owned(downscale_encoded(
        image,
        transfer_function_for(settings.color_space),
        dw,
        dh,
    )?))
}

/// 書き出す Exif（`remove_gps` を適用したもの）。
fn exif_for(settings: &ExportSettings, exif: Option<&ExifData>) -> Option<ExifData> {
    exif.map(|e| {
        if settings.remove_gps {
            e.clone().without_gps()
        } else {
            e.clone()
        }
    })
}

/// 設定に従って画像を書き出し用のバイト列にする（寸法・ビット数・ICC・Exif。ファイルには書かない）。
pub fn encode_export(
    image: &DynRgbImage,
    settings: &ExportSettings,
    exif: Option<&ExifData>,
) -> Result<Vec<u8>> {
    settings.validate()?;
    let fitted = fit_to_export_size(image, settings)?;
    let exif = exif_for(settings, exif);
    let mut cur = std::io::Cursor::new(Vec::new());
    encode_export_to(
        &fitted,
        settings.format,
        settings.color_space,
        exif.as_ref(),
        &mut cur,
    )?;
    Ok(cur.into_inner())
}

/// 設定に従って画像をファイルに書き出す。
///
/// 書き出し先は `desired_path`（拡張子は呼び出し側で `settings.format.extension()` に合わせる）。
/// 既存のファイルとの衝突は `settings.on_conflict` に従い、書き出し先が `protected`（カタログに
/// 登録されたファイルと今回の入力）と同じファイルなら、上書きの設定でも中止する（6.4 節）。
/// 一時ファイルに書いてから名前を変更するので、途中で失敗しても不完全なファイルは残らない。
pub fn export_image(
    image: &DynRgbImage,
    settings: &ExportSettings,
    exif: Option<&ExifData>,
    desired_path: &Path,
    protected: &ProtectedFiles,
) -> Result<WriteOutcome> {
    settings.validate()?;
    let fitted = fit_to_export_size(image, settings)?;
    let exif = exif_for(settings, exif);
    write_atomically(desired_path, settings.on_conflict, protected, |file| {
        encode_export_to(
            &fitted,
            settings.format,
            settings.color_space,
            exif.as_ref(),
            file,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::RgbImage16;
    use genzo_model::{ExportSize, GpsCoord};

    #[test]
    fn profile_mapping() {
        assert_eq!(
            standard_profile_for(OutputColorSpace::AdobeRgb),
            StandardProfile::AdobeRgb1998
        );
        assert_eq!(
            transfer_function_for(OutputColorSpace::DisplayP3),
            TransferFunction::Srgb
        );
        assert_eq!(
            transfer_function_for(OutputColorSpace::AdobeRgb),
            TransferFunction::AdobeRgbGamma
        );
        for s in [
            OutputColorSpace::Srgb,
            OutputColorSpace::DisplayP3,
            OutputColorSpace::AdobeRgb,
        ] {
            let p = export_icc_profile(s).unwrap();
            assert_eq!(p.version(), (2, 4));
            assert_eq!(p.description(), Some(standard_profile_for(s).description()));
        }
    }

    #[test]
    fn invalid_settings_are_rejected() {
        let img = DynRgbImage::Rgb16(RgbImage16::new(4, 4).unwrap());
        let s = ExportSettings {
            format: ExportFormat::Jpeg { quality: 0 },
            ..Default::default()
        };
        assert!(matches!(
            encode_export(&img, &s, None),
            Err(MediaError::InvalidExportSettings(_))
        ));
    }

    #[test]
    fn size_is_applied_and_gps_follows_setting() {
        let img = DynRgbImage::Rgb16(
            RgbImage16::from_fn(40, 20, |x, _| [x as u16 * 1000, 30000, 0]).unwrap(),
        );
        let exif = ExifData {
            make: Some("SONY".to_owned()),
            gps: GpsCoord::new(35.0, 139.0),
            ..Default::default()
        };
        for remove_gps in [false, true] {
            let s = ExportSettings {
                format: ExportFormat::Png16,
                size: ExportSize::LongEdge(10),
                remove_gps,
                ..Default::default()
            };
            let bytes = encode_export(&img, &s, Some(&exif)).unwrap();
            let decoded = image::load_from_memory(&bytes).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (10, 5));
            let e = exif::Reader::new()
                .read_from_container(&mut std::io::Cursor::new(&bytes))
                .unwrap();
            let has_gps = e
                .get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY)
                .is_some();
            assert_eq!(has_gps, !remove_gps);
            assert_eq!(
                e.get_field(exif::Tag::PixelXDimension, exif::In::PRIMARY)
                    .unwrap()
                    .value
                    .get_uint(0),
                Some(10)
            );
        }
    }
}
