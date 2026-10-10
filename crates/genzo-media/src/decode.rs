//! JPEG / PNG / TIFF の読み込み（04 の 2.6 節「RAW 以外の入力」、LIB-14、PRV-01）。
//!
//! - 画素は 8bit / 16bit の RGB にする（グレーは RGB に広げ、アルファは捨てる。32bit 浮動小数点の
//!   TIFF は 0〜1 に収めて 16bit にする）。向き（Orientation）は **反映しない**（記録されたまま返す。
//!   反映は [`crate::orientation::apply_orientation`]）。
//! - 埋め込みの ICC プロファイルを返す。ない・読めない・RGB でない・変換に使えないときは sRGB と
//!   みなす（2.6 節）。その理由を [`SrgbAssumption`] で返す。
//! - Exif（kamadak-exif）から [`PhotoMetadata`] を作る。
//!
//! 信頼できない入力のデコードはワーカープロセスで行う（04 の 1.1 節・1.2 節、SEC-05）。
//! 画素数（[`MAX_IMAGE_PIXELS`]）・デコーダーの確保量（[`MAX_DECODE_ALLOC_BYTES`]）・
//! ファイルの大きさ（[`MAX_INPUT_FILE_BYTES`]）に上限を設ける。元のファイルは読み取り専用で開く
//! （DATA-01）。

use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read};
use std::path::Path;

use genzo_color::{IccProfile, IccTransform, IccVersion, RenderingIntent, StandardProfile};
use genzo_model::{Orientation, PhotoMetadata};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

use crate::buffer::{DynRgbImage, MAX_IMAGE_PIXELS, RgbImage, Sample};
use crate::error::{MediaError, Result};
use crate::exif_read::{parse_exif, photo_metadata_from_exif};
use crate::orientation::apply_orientation_dyn;

/// デコーダーが一度に確保してよいメモリの上限（バイト。仮置き）。
///
/// 上限の画素数（2 億画素）の 16bit RGB（1.2 GB）が収まる大きさ。メモリの予算（SCL-05、
/// 一括処理時 8 GB）の中で、ワーカー 1 つが使う量の目安として PoC-2 で見直す。
pub const MAX_DECODE_ALLOC_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// 読み込むファイルの大きさの上限（バイト。仮置き）。
///
/// 1 億画素の 16bit 無圧縮 TIFF（約 600 MB）より十分大きい値。ファイル全体をメモリに読むため、
/// 壊れたファイル・巨大なファイルで確保しすぎないようにする。
pub const MAX_INPUT_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// 対応する画像の形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFileFormat {
    /// JPEG。
    Jpeg,
    /// PNG。
    Png,
    /// TIFF（ベースラインの TIFF。RAW の TIFF 系の形式は genzo-raw で扱う）。
    Tiff,
}

impl ImageFileFormat {
    /// 先頭のバイト列（マジックナンバー）から形式を判定する。拡張子は見ない。
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
            Some(Self::Tiff)
        } else {
            None
        }
    }

    fn image_format(self) -> ImageFormat {
        match self {
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Png => ImageFormat::Png,
            Self::Tiff => ImageFormat::Tiff,
        }
    }
}

/// 埋め込みの ICC プロファイルを使わず、sRGB とみなした理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrgbAssumption {
    /// ICC プロファイルが埋め込まれていない。
    NoProfile,
    /// ICC プロファイルとして読めない（壊れている・大きすぎる）。
    Invalid(String),
    /// RGB のプロファイルではない（グレー・CMYK など。画素は RGB に変換済みのため合わない）。
    NotRgb,
    /// RGB だが、色の変換に使えない（デバイスリンクなど）。
    Unsupported(String),
}

/// 画像の色を解釈するプロファイル。
#[derive(Debug, Clone, PartialEq)]
pub enum SourceProfile {
    /// 埋め込みの ICC プロファイル（RGB で、変換に使えることを確認済み）。
    Embedded(IccProfile),
    /// sRGB とみなす（理由付き）。
    AssumedSrgb(SrgbAssumption),
}

impl SourceProfile {
    /// 色の変換に使うプロファイル（sRGB とみなす場合は sRGB のプロファイル）。
    pub fn to_icc(&self) -> Result<IccProfile> {
        match self {
            Self::Embedded(p) => Ok(p.clone()),
            Self::AssumedSrgb(_) => {
                crate::profiles::standard(StandardProfile::Srgb, IccVersion::V4_3)
            }
        }
    }

    /// 埋め込みのプロファイルを使うか。
    pub fn is_embedded(&self) -> bool {
        matches!(self, Self::Embedded(_))
    }
}

/// 埋め込みの ICC のバイト列を確認する。
pub(crate) fn resolve_profile(icc: Option<Vec<u8>>) -> SourceProfile {
    let Some(bytes) = icc.filter(|b| !b.is_empty()) else {
        return SourceProfile::AssumedSrgb(SrgbAssumption::NoProfile);
    };
    let profile = match IccProfile::from_bytes(&bytes) {
        Ok(p) => p,
        Err(e) => return SourceProfile::AssumedSrgb(SrgbAssumption::Invalid(e.to_string())),
    };
    if !profile.is_rgb() {
        return SourceProfile::AssumedSrgb(SrgbAssumption::NotRgb);
    }
    // 変換を作れることを確かめる（作業色空間への変換に使うため）。
    let check = crate::profiles::standard(StandardProfile::LinearBt2020, IccVersion::V4_3)
        .and_then(|lin| {
            Ok(IccTransform::new(
                &profile,
                &lin,
                RenderingIntent::RelativeColorimetric,
            )?)
        });
    match check {
        Ok(_) => SourceProfile::Embedded(profile),
        Err(e) => SourceProfile::AssumedSrgb(SrgbAssumption::Unsupported(e.to_string())),
    }
}

/// 画像の情報（画素をデコードせずに読める部分）。
#[derive(Debug, Clone, PartialEq)]
pub struct ImageInfo {
    /// 形式。
    pub format: ImageFileFormat,
    /// 色を解釈するプロファイル。
    pub profile: SourceProfile,
    /// メタデータ（幅・高さは向きを反映する前の寸法）。
    pub metadata: PhotoMetadata,
}

/// デコードした画像。
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedImage {
    /// 形式。
    pub format: ImageFileFormat,
    /// 画素（向きは反映していない。ファイルに記録されたまま）。
    pub pixels: DynRgbImage,
    /// 色を解釈するプロファイル。
    pub profile: SourceProfile,
    /// メタデータ（幅・高さは向きを反映する前の寸法、向きは Exif の値）。
    pub metadata: PhotoMetadata,
    /// アルファチャンネルを捨てたか。
    pub alpha_dropped: bool,
}

impl DecodedImage {
    /// 向き（Exif の Orientation）を反映した画素。
    pub fn oriented_pixels(&self) -> DynRgbImage {
        apply_orientation_dyn(&self.pixels, self.metadata.orientation)
    }

    /// Exif の向き。
    pub fn orientation(&self) -> Orientation {
        self.metadata.orientation
    }
}

fn decode_limits() -> Limits {
    let mut l = Limits::default();
    l.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    l
}

fn image_err(e: image::ImageError) -> MediaError {
    match e {
        image::ImageError::Unsupported(u) => MediaError::unsupported(u.to_string()),
        image::ImageError::Limits(l) => MediaError::decode(format!("上限を超えた: {l}")),
        other => MediaError::decode(other.to_string()),
    }
}

/// デコーダーを作り、寸法を確認する。
fn open_decoder(bytes: &[u8], format: ImageFileFormat) -> Result<impl ImageDecoder + '_> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format.image_format());
    reader.limits(decode_limits());
    let decoder = reader.into_decoder().map_err(image_err)?;
    let (w, h) = decoder.dimensions();
    if w == 0 || h == 0 {
        return Err(MediaError::decode(format!("寸法が不正（{w}×{h}）")));
    }
    let pixels = u64::from(w) * u64::from(h);
    if pixels > MAX_IMAGE_PIXELS {
        return Err(MediaError::TooLarge {
            what: "画素数",
            actual: pixels,
            max: MAX_IMAGE_PIXELS,
        });
    }
    Ok(decoder)
}

/// TIFF の ICC プロファイル（タグ 34675）を tiff crate で直接読む。
///
/// image crate（0.25.10）の TIFF のデコーダーは、上限（Limits）を設定すると、画像のデータより
/// 大きいタグの値を読めなくなる（小さな画像では ICC プロファイルが読めない）ため、これを避ける。
fn tiff_icc(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut limits = tiff::decoder::Limits::default();
    limits.ifd_value_size = genzo_color::icc::MAX_ICC_PROFILE_BYTES;
    limits.decoding_buffer_size = genzo_color::icc::MAX_ICC_PROFILE_BYTES;
    let mut dec = tiff::decoder::Decoder::new(Cursor::new(bytes))
        .ok()?
        .with_limits(limits);
    dec.get_tag_u8_vec(tiff::tags::Tag::IccProfile).ok()
}

/// ICC と Exif のバイト列を読む（読めなければ `None`。メタデータの破損で画像全体を失敗にしない）。
fn read_metadata_blobs(
    decoder: &mut impl ImageDecoder,
    format: ImageFileFormat,
    bytes: &[u8],
) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    match format {
        // TIFF の Exif は IFD の構造そのものなので、ファイル全体を別に解析する（呼び出し側）。
        ImageFileFormat::Tiff => (tiff_icc(bytes), None),
        _ => (
            decoder.icc_profile().ok().flatten(),
            decoder.exif_metadata().ok().flatten(),
        ),
    }
}

fn metadata_from(
    exif_blob: Option<Vec<u8>>,
    tiff_file: Option<Vec<u8>>,
    (w, h): (u32, u32),
) -> PhotoMetadata {
    let exif = exif_blob.or(tiff_file).and_then(parse_exif);
    let mut meta = exif
        .as_ref()
        .map(photo_metadata_from_exif)
        .unwrap_or_default();
    meta.width = Some(w);
    meta.height = Some(h);
    meta
}

/// image crate の画像を 8bit / 16bit の RGB にする。
fn to_rgb(img: DynamicImage) -> Result<(DynRgbImage, bool)> {
    let alpha = img.color().has_alpha();
    let (w, h) = (img.width(), img.height());
    let out = match img {
        DynamicImage::ImageRgb8(b) => DynRgbImage::Rgb8(RgbImage::from_raw(w, h, b.into_raw())?),
        DynamicImage::ImageRgb16(b) => DynRgbImage::Rgb16(RgbImage::from_raw(w, h, b.into_raw())?),
        DynamicImage::ImageLuma8(_)
        | DynamicImage::ImageLumaA8(_)
        | DynamicImage::ImageRgba8(_) => {
            DynRgbImage::Rgb8(RgbImage::from_raw(w, h, img.to_rgb8().into_raw())?)
        }
        DynamicImage::ImageLuma16(_)
        | DynamicImage::ImageLumaA16(_)
        | DynamicImage::ImageRgba16(_) => {
            DynRgbImage::Rgb16(RgbImage::from_raw(w, h, img.to_rgb16().into_raw())?)
        }
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => {
            // 浮動小数点は 0〜1 に収めて 16bit にする（HDR の TIFF の 1 を超える値は失われる）。
            // NaN は 0 にする（2.6 節）。image crate の `to_rgb16` は NaN を最大値（白）にするため、
            // 自前で量子化する（`Sample::from_unit_f32` は NaN を 0 にする）。
            let data = img
                .into_rgb32f()
                .into_raw()
                .into_iter()
                .map(u16::from_unit_f32)
                .collect();
            DynRgbImage::Rgb16(RgbImage::from_raw(w, h, data)?)
        }
        other => {
            return Err(MediaError::unsupported(format!(
                "色の型 {:?}",
                other.color()
            )));
        }
    };
    Ok((out, alpha))
}

fn detect_or_err(bytes: &[u8]) -> Result<ImageFileFormat> {
    ImageFileFormat::detect(bytes)
        .ok_or_else(|| MediaError::unsupported("JPEG / PNG / TIFF のいずれでもない"))
}

/// 画素をデコードせずに、形式・ICC・メタデータを読む（取り込み時のメタデータの読み取り）。
pub fn probe_image_bytes(bytes: &[u8]) -> Result<ImageInfo> {
    let (format, icc, exif, dims) = probe_parts(bytes)?;
    let tiff_file = (format == ImageFileFormat::Tiff).then(|| bytes.to_vec());
    Ok(ImageInfo {
        format,
        profile: resolve_profile(icc),
        metadata: metadata_from(exif, tiff_file, dims),
    })
}

/// ICC・Exif のバイト列（Exif の解析の前）と寸法。
type ProbeParts = (
    ImageFileFormat,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    (u32, u32),
);

fn probe_parts(bytes: &[u8]) -> Result<ProbeParts> {
    let format = detect_or_err(bytes)?;
    let mut decoder = open_decoder(bytes, format)?;
    let dims = decoder.dimensions();
    let (icc, exif) = read_metadata_blobs(&mut decoder, format, bytes);
    Ok((format, icc, exif, dims))
}

/// 画素をデコードした途中の結果（メタデータの解析の前）。
struct RawDecoded {
    format: ImageFileFormat,
    pixels: DynRgbImage,
    alpha_dropped: bool,
    icc: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
    dims: (u32, u32),
}

fn decode_pixels(bytes: &[u8]) -> Result<RawDecoded> {
    let format = detect_or_err(bytes)?;
    let mut decoder = open_decoder(bytes, format)?;
    let dims = decoder.dimensions();
    let (icc, exif) = read_metadata_blobs(&mut decoder, format, bytes);
    let img = DynamicImage::from_decoder(decoder).map_err(image_err)?;
    if (img.width(), img.height()) != dims {
        return Err(MediaError::decode("デコードした寸法がヘッダーと一致しない"));
    }
    let (pixels, alpha_dropped) = to_rgb(img)?;
    Ok(RawDecoded {
        format,
        pixels,
        alpha_dropped,
        icc,
        exif,
        dims,
    })
}

/// `tiff_file` は TIFF のときのファイル全体（Exif の解析に使う）。
fn finish_decoded(d: RawDecoded, tiff_file: Option<Vec<u8>>) -> DecodedImage {
    DecodedImage {
        format: d.format,
        pixels: d.pixels,
        profile: resolve_profile(d.icc),
        metadata: metadata_from(d.exif, tiff_file, d.dims),
        alpha_dropped: d.alpha_dropped,
    }
}

/// JPEG / PNG / TIFF のバイト列をデコードする。形式は先頭のバイト列で判定する。
pub fn decode_image_bytes(bytes: &[u8]) -> Result<DecodedImage> {
    let d = decode_pixels(bytes)?;
    let tiff_file = (d.format == ImageFileFormat::Tiff).then(|| bytes.to_vec());
    Ok(finish_decoded(d, tiff_file))
}

/// 元のファイルを読み取り専用で開く（DATA-01）。
pub fn open_read_only(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| MediaError::io(path, e))
}

/// ファイル全体を読み取り専用で読む。`max_bytes` を超えるファイルはエラー。
pub fn read_file_limited(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let file = open_read_only(path)?;
    let len = file.metadata().map_err(|e| MediaError::io(path, e))?.len();
    if len > max_bytes {
        return Err(MediaError::TooLarge {
            what: "ファイル",
            actual: len,
            max: max_bytes,
        });
    }
    let mut buf = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    // 読んでいる間にファイルが大きくなっても、上限 + 1 バイトまでしか読まない。
    file.take(max_bytes + 1)
        .read_to_end(&mut buf)
        .map_err(|e| MediaError::io(path, e))?;
    if buf.len() as u64 > max_bytes {
        return Err(MediaError::TooLarge {
            what: "ファイル",
            actual: buf.len() as u64,
            max: max_bytes,
        });
    }
    Ok(buf)
}

/// JPEG / PNG / TIFF のファイルをデコードする（読み取り専用で開く。DATA-01）。
///
/// ファイル全体をメモリに読んでからデコードする。TIFF の Exif はファイル全体を解析するので、
/// 読んだバッファをそのまま渡す（複製しない）。
pub fn decode_image_file(path: &Path) -> Result<DecodedImage> {
    let bytes = read_file_limited(path, MAX_INPUT_FILE_BYTES)?;
    let d = decode_pixels(&bytes)?;
    let tiff_file = (d.format == ImageFileFormat::Tiff).then_some(bytes);
    Ok(finish_decoded(d, tiff_file))
}

/// ファイルの形式・ICC・メタデータを読む（画素はデコードしない。読み取り専用で開く。DATA-01）。
pub fn probe_image_file(path: &Path) -> Result<ImageInfo> {
    let bytes = read_file_limited(path, MAX_INPUT_FILE_BYTES)?;
    let (format, icc, exif, dims) = probe_parts(&bytes)?;
    // TIFF の Exif の解析には、読んだバッファをそのまま渡す（複製しない）。
    let tiff_file = (format == ImageFileFormat::Tiff).then_some(bytes);
    Ok(ImageInfo {
        format,
        profile: resolve_profile(icc),
        metadata: metadata_from(exif, tiff_file, dims),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::RgbImage8;

    #[test]
    fn format_detection() {
        assert_eq!(
            ImageFileFormat::detect(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFileFormat::Jpeg)
        );
        assert_eq!(
            ImageFileFormat::detect(b"\x89PNG\r\n\x1a\n...."),
            Some(ImageFileFormat::Png)
        );
        assert_eq!(
            ImageFileFormat::detect(b"II*\0...."),
            Some(ImageFileFormat::Tiff)
        );
        assert_eq!(
            ImageFileFormat::detect(b"MM\0*...."),
            Some(ImageFileFormat::Tiff)
        );
        assert_eq!(ImageFileFormat::detect(b"GIF89a"), None);
        assert_eq!(ImageFileFormat::detect(&[]), None);
    }

    #[test]
    fn profile_resolution() {
        assert_eq!(
            resolve_profile(None),
            SourceProfile::AssumedSrgb(SrgbAssumption::NoProfile)
        );
        assert_eq!(
            resolve_profile(Some(vec![])),
            SourceProfile::AssumedSrgb(SrgbAssumption::NoProfile)
        );
        assert!(matches!(
            resolve_profile(Some(vec![1, 2, 3])),
            SourceProfile::AssumedSrgb(SrgbAssumption::Invalid(_))
        ));
        let p3 = IccProfile::standard(StandardProfile::DisplayP3).unwrap();
        assert_eq!(
            resolve_profile(Some(p3.as_bytes().to_vec())),
            SourceProfile::Embedded(p3.clone())
        );
        // デバイスリンクのクラスにしたもの（RGB だが変換に使えない）。
        let mut link = p3.as_bytes().to_vec();
        link[12..16].copy_from_slice(b"link");
        assert!(matches!(
            resolve_profile(Some(link)),
            SourceProfile::AssumedSrgb(SrgbAssumption::Unsupported(_))
        ));
        // 色空間を GRAY にしたもの。
        let mut gray = p3.as_bytes().to_vec();
        gray[16..20].copy_from_slice(b"GRAY");
        let r = resolve_profile(Some(gray));
        assert!(
            matches!(
                r,
                SourceProfile::AssumedSrgb(SrgbAssumption::NotRgb | SrgbAssumption::Invalid(_))
            ),
            "{r:?}"
        );
        let srgb = SourceProfile::AssumedSrgb(SrgbAssumption::NoProfile);
        assert!(!srgb.is_embedded());
        assert_eq!(
            srgb.to_icc().unwrap(),
            IccProfile::standard(StandardProfile::Srgb).unwrap()
        );
    }

    #[test]
    fn gray_and_alpha_png_become_rgb() {
        let gray = image::GrayImage::from_fn(3, 2, |x, _| image::Luma([x as u8 * 100]));
        let mut png = Vec::new();
        gray.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let d = decode_image_bytes(&png).unwrap();
        assert_eq!(d.format, ImageFileFormat::Png);
        assert!(!d.alpha_dropped);
        assert_eq!(d.pixels.to_rgb8().pixel(2, 1), Some([200, 200, 200]));
        assert_eq!(d.metadata.width, Some(3));
        assert_eq!(d.metadata.height, Some(2));

        let rgba = image::RgbaImage::from_fn(2, 2, |_, _| image::Rgba([10, 20, 30, 0]));
        let mut png = Vec::new();
        rgba.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let d = decode_image_bytes(&png).unwrap();
        assert!(d.alpha_dropped);
        assert_eq!(d.pixels.to_rgb8().pixel(0, 0), Some([10, 20, 30]));

        let rgba16 = image::ImageBuffer::<image::Rgba<u16>, _>::from_fn(2, 1, |_, _| {
            image::Rgba([1000u16, 2000, 3000, 65535])
        });
        let mut png = Vec::new();
        DynamicImage::ImageRgba16(rgba16)
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let d = decode_image_bytes(&png).unwrap();
        assert_eq!(d.pixels.bits_per_channel(), 16);
        assert_eq!(d.pixels.to_rgb16().pixel(1, 0), Some([1000, 2000, 3000]));
    }

    #[test]
    fn plain_jpeg_without_metadata_assumes_srgb() {
        let img = RgbImage8::from_fn(8, 8, |_, _| [100, 150, 200]).unwrap();
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 95)
            .encode(img.as_raw(), 8, 8, image::ExtendedColorType::Rgb8)
            .unwrap();
        let d = decode_image_bytes(&jpg).unwrap();
        assert_eq!(d.format, ImageFileFormat::Jpeg);
        assert_eq!(
            d.profile,
            SourceProfile::AssumedSrgb(SrgbAssumption::NoProfile)
        );
        assert_eq!(d.metadata.orientation, Orientation::Normal);
        assert_eq!(d.metadata.make, None);
        let px = d.pixels.to_rgb8().pixel(4, 4).unwrap();
        assert!(
            px.iter()
                .zip([100, 150, 200])
                .all(|(a, b)| a.abs_diff(b) <= 3),
            "{px:?}"
        );
        let info = probe_image_bytes(&jpg).unwrap();
        assert_eq!(info.metadata.width, Some(8));
    }

    #[test]
    fn broken_inputs_are_errors_not_panics() {
        assert!(matches!(
            decode_image_bytes(b"hello"),
            Err(MediaError::Unsupported { .. })
        ));
        assert!(decode_image_bytes(&[]).is_err());
        let img = RgbImage8::from_fn(32, 32, |x, y| [x as u8, y as u8, 0]).unwrap();
        for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Tiff] {
            let mut buf = Vec::new();
            image::RgbImage::from_raw(32, 32, img.as_raw().to_vec())
                .unwrap()
                .write_to(&mut Cursor::new(&mut buf), format)
                .unwrap();
            for cut in [3, 8, 16, 64, buf.len() / 2, buf.len() - 1] {
                let _ = decode_image_bytes(&buf[..cut]);
                let _ = probe_image_bytes(&buf[..cut]);
            }
            // 中身を壊したもの。
            let mut bad = buf.clone();
            for b in bad.iter_mut().skip(40).step_by(13) {
                *b ^= 0x5A;
            }
            let _ = decode_image_bytes(&bad);
        }
    }

    #[test]
    fn oversized_dimensions_are_rejected_before_decoding() {
        // IHDR だけ巨大な寸法の PNG（50000 × 50000 = 25 億画素）。
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&50_000u32.to_be_bytes());
        ihdr.extend_from_slice(&50_000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend_from_slice(&(ihdr.len() as u32).to_be_bytes());
        let mut crc_data = b"IHDR".to_vec();
        crc_data.extend_from_slice(&ihdr);
        png.extend_from_slice(&crc_data);
        png.extend_from_slice(&crc32(&crc_data).to_be_bytes());
        // ヘッダーの解析が IDAT まで進むよう、空の IDAT と IEND を付ける。
        for name in [b"IDAT", b"IEND"] {
            png.extend_from_slice(&0u32.to_be_bytes());
            png.extend_from_slice(name);
            png.extend_from_slice(&crc32(name).to_be_bytes());
        }
        let r = decode_image_bytes(&png);
        assert!(matches!(r, Err(MediaError::TooLarge { .. })), "{r:?}");
        let r = probe_image_bytes(&png);
        assert!(matches!(r, Err(MediaError::TooLarge { .. })), "{r:?}");
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn float_tiff_is_clamped_and_nan_becomes_zero() {
        // 32bit 浮動小数点の TIFF: 0〜1 に収めて 16bit にし、NaN は 0 にする（2.6 節「NaN・無限大は
        // 0 に置き換える」。再現: 修正前は image crate の変換で NaN が 65535（白）になった）。
        let px = [
            [0.5f32, 0.25, 1.0],
            [1.5, -0.5, f32::NAN],
            [f32::INFINITY, f32::NEG_INFINITY, 0.0],
        ];
        let buf = image::ImageBuffer::<image::Rgb<f32>, _>::from_fn(3, 1, |x, _| {
            image::Rgb(px[x as usize])
        });
        let mut tif = Vec::new();
        DynamicImage::ImageRgb32F(buf)
            .write_to(&mut Cursor::new(&mut tif), ImageFormat::Tiff)
            .unwrap();
        let d = decode_image_bytes(&tif).unwrap();
        assert_eq!(d.format, ImageFileFormat::Tiff);
        assert!(!d.alpha_dropped);
        let rgb = d.pixels.to_rgb16();
        assert_eq!(rgb.pixel(0, 0), Some([32768, 16384, 65535]));
        assert_eq!(rgb.pixel(1, 0), Some([65535, 0, 0]));
        // 正の無限大は 1 に収める（範囲外として扱う）。負の無限大は 0。
        assert_eq!(rgb.pixel(2, 0), Some([65535, 0, 0]));
    }

    #[test]
    fn read_file_limited_enforces_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        std::fs::write(&p, [1u8; 100]).unwrap();
        assert_eq!(read_file_limited(&p, 100).unwrap().len(), 100);
        assert!(matches!(
            read_file_limited(&p, 99),
            Err(MediaError::TooLarge { .. })
        ));
        assert!(matches!(
            read_file_limited(&dir.path().join("missing"), 10),
            Err(MediaError::Io { .. })
        ));
    }
}
