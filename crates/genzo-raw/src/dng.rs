//! 合成 DNG の書き出し（テスト用。機能フラグに関係なく使える）。
//!
//! [`RawImage`] から、非圧縮のベイヤー DNG（リトルエンディアンの TIFF）を作る。LibRaw の FFI の
//! テスト（PoC-2 の前に、寸法・CFA・黒レベル・白レベル・行列・WB・撮影情報が正しく取り出せるか）と、
//! 他の crate の回帰テスト（合成 DNG。M0 タスク 11・12）に使う。
//!
//! 書く主なタグ（Adobe の DNG Specification 1.4.0.0 と TIFF 6.0、EXIF 2.32）:
//!
//! | IFD | タグ |
//! |---|---|
//! | IFD0（RAW） | NewSubFileType = 0、ImageWidth / ImageLength、BitsPerSample = 16、Compression = 1、PhotometricInterpretation = 32803（CFA）、Make / Model、StripOffsets / RowsPerStrip / StripByteCounts、Orientation、SamplesPerPixel = 1、PlanarConfiguration = 1、Software、SubIFDs（プレビューがあるとき）、CFARepeatPatternDim = 2 × 2、CFAPattern、ExifIFD、GPSInfo、DNGVersion = 1.4.0.0、DNGBackwardVersion = 1.1.0.0、UniqueCameraModel、CFAPlaneColor、CFALayout = 1、BlackLevelRepeatDim = 2 × 2、BlackLevel（4 位置）、WhiteLevel、ColorMatrix1（/2）、AsShotNeutral、CalibrationIlluminant1（/2）、ActiveArea（余白があるとき） |
//! | EXIF IFD | ExposureTime、FNumber、ISOSpeedRatings（65535 を超えるときは RecommendedExposureIndex も）、ExifVersion、DateTimeOriginal、OffsetTimeOriginal、FocalLength、SubSecTimeOriginal、LensModel |
//! | GPS IFD | GPSVersionID、GPSLatitudeRef / GPSLatitude、GPSLongitudeRef / GPSLongitude |
//! | SubIFD（プレビュー） | NewSubFileType = 1、8bit の RGB（非圧縮）または JPEG（Compression = 7） |
//!
//! 制限（テスト用のため）:
//! - CFA の 2 つの緑の WB は区別できない（AsShotNeutral は R・G・B の 3 つ）。
//!   [`RawImage::as_shot_wb`] の G2 は書かない。
//! - 黒レベルは RATIONAL で書く（整数なら分母 1）。LibRaw 0.21 は CFA の DNG の黒レベルの
//!   パターンを整数（`unsigned`）に切り捨てて読む。白レベルは整数に丸めて書く。
//! - 文字列は ASCII だけ。撮影日時は EXIF の形式（`"YYYY:MM:DD HH:MM:SS"`、小数秒は
//!   `".123"` の形で付けてよい。小数秒は SubSecTimeOriginal に分けて書く）。
//! - [`RawImage::cam_xyz`] が `None` の場合は ColorMatrix1 を書かない（DNG の仕様では必須）。
//! - LibRaw で読む場合、幅と高さは 22 画素以上にする（LibRaw はそれ未満を RAW として扱わない）。
//! - 余白（[`DngOptions::margins`]）の左・上を奇数にすると、LibRaw は有効画素を 1 画素ずらして
//!   読む（`crate::decode` の「制限・注意」）。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use genzo_model::{GpsCoord, PhotoMetadata};

use crate::{CfaColor, RawError, RawImage};

/// EXIF の LightSource の D65（CalibrationIlluminant の既定値）。
pub const ILLUMINANT_D65: u16 = 21;
/// EXIF の LightSource の標準の光 A（テスト用）。
pub const ILLUMINANT_STANDARD_A: u16 = 17;

/// 書き出しの設定。
#[derive(Debug, Clone, PartialEq)]
pub struct DngOptions {
    /// 有効画素の外側に付ける余白（ActiveArea のテスト用。既定は 0）。
    pub margins: DngMargins,
    /// 余白の画素の値。
    pub margin_value: u16,
    /// UniqueCameraModel。`None` ならメーカーと機種から作る。
    pub unique_camera_model: Option<String>,
    /// ColorMatrix1（[`RawImage::cam_xyz`]）の CalibrationIlluminant1。既定は D65（21）。
    pub illuminant_1: u16,
    /// ColorMatrix2 と CalibrationIlluminant2（任意）。
    pub color_matrix_2: Option<([[f32; 3]; 3], u16)>,
    /// 埋め込みのプレビュー（SubIFD。任意）。
    pub preview: Option<DngPreview>,
}

impl Default for DngOptions {
    fn default() -> Self {
        Self {
            margins: DngMargins::default(),
            margin_value: 0,
            unique_camera_model: None,
            illuminant_1: ILLUMINANT_D65,
            color_matrix_2: None,
            preview: None,
        }
    }
}

/// 有効画素の外側の余白（画素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DngMargins {
    /// 左。
    pub left: u32,
    /// 上。
    pub top: u32,
    /// 右。
    pub right: u32,
    /// 下。
    pub bottom: u32,
}

impl DngMargins {
    /// すべて 0 か。
    pub fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

/// 埋め込みのプレビュー。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DngPreview {
    /// 8bit の RGB（行優先、`width * height * 3` バイト）。
    Rgb8 {
        /// 幅。
        width: u32,
        /// 高さ。
        height: u32,
        /// 画素の値。
        data: Vec<u8>,
    },
    /// JPEG のファイルのバイト列。
    Jpeg {
        /// 幅。
        width: u32,
        /// 高さ。
        height: u32,
        /// JPEG のバイト列。
        data: Vec<u8>,
    },
}

/// DNG の書き出しのエラー。
#[derive(Debug, thiserror::Error)]
pub enum DngError {
    /// 入力の RAW が不正（[`RawImage::validate`] のエラー）。
    #[error("入力の RAW が不正です: {0}")]
    InvalidImage(#[from] RawError),
    /// DNG に書けない値。
    #[error("DNG に書けない値です: {0}")]
    InvalidValue(String),
    /// ファイルが TIFF の上限（4 GiB）を超える。
    #[error("DNG の大きさが TIFF の上限（4 GiB）を超えます")]
    TooLarge,
    /// 入出力のエラー。
    #[error("DNG を書き出せません: {0}")]
    Io(#[from] std::io::Error),
}

/// TIFF のタグの番号。
mod tag {
    pub const NEW_SUBFILE_TYPE: u16 = 254;
    pub const IMAGE_WIDTH: u16 = 256;
    pub const IMAGE_LENGTH: u16 = 257;
    pub const BITS_PER_SAMPLE: u16 = 258;
    pub const COMPRESSION: u16 = 259;
    pub const PHOTOMETRIC: u16 = 262;
    pub const MAKE: u16 = 271;
    pub const MODEL: u16 = 272;
    pub const STRIP_OFFSETS: u16 = 273;
    pub const ORIENTATION: u16 = 274;
    pub const SAMPLES_PER_PIXEL: u16 = 277;
    pub const ROWS_PER_STRIP: u16 = 278;
    pub const STRIP_BYTE_COUNTS: u16 = 279;
    pub const PLANAR_CONFIGURATION: u16 = 284;
    pub const SOFTWARE: u16 = 305;
    pub const SUB_IFDS: u16 = 330;
    pub const CFA_REPEAT_PATTERN_DIM: u16 = 33421;
    pub const CFA_PATTERN: u16 = 33422;
    pub const EXPOSURE_TIME: u16 = 33434;
    pub const F_NUMBER: u16 = 33437;
    pub const EXIF_IFD: u16 = 34665;
    pub const GPS_IFD: u16 = 34853;
    pub const ISO_SPEED_RATINGS: u16 = 34855;
    pub const SENSITIVITY_TYPE: u16 = 34864;
    pub const RECOMMENDED_EXPOSURE_INDEX: u16 = 34866;
    pub const EXIF_VERSION: u16 = 36864;
    pub const DATE_TIME_ORIGINAL: u16 = 36867;
    pub const OFFSET_TIME_ORIGINAL: u16 = 36881;
    pub const FOCAL_LENGTH: u16 = 37386;
    pub const SUB_SEC_TIME_ORIGINAL: u16 = 37521;
    pub const LENS_MODEL: u16 = 42036;
    pub const DNG_VERSION: u16 = 50706;
    pub const DNG_BACKWARD_VERSION: u16 = 50707;
    pub const UNIQUE_CAMERA_MODEL: u16 = 50708;
    pub const CFA_PLANE_COLOR: u16 = 50710;
    pub const CFA_LAYOUT: u16 = 50711;
    pub const BLACK_LEVEL_REPEAT_DIM: u16 = 50713;
    pub const BLACK_LEVEL: u16 = 50714;
    pub const WHITE_LEVEL: u16 = 50717;
    pub const COLOR_MATRIX_1: u16 = 50721;
    pub const COLOR_MATRIX_2: u16 = 50722;
    pub const AS_SHOT_NEUTRAL: u16 = 50728;
    pub const CALIBRATION_ILLUMINANT_1: u16 = 50778;
    pub const CALIBRATION_ILLUMINANT_2: u16 = 50779;
    pub const ACTIVE_AREA: u16 = 50829;

    pub const GPS_VERSION_ID: u16 = 0;
    pub const GPS_LATITUDE_REF: u16 = 1;
    pub const GPS_LATITUDE: u16 = 2;
    pub const GPS_LONGITUDE_REF: u16 = 3;
    pub const GPS_LONGITUDE: u16 = 4;
}

/// PhotometricInterpretation の CFA（DNG の仕様）。
const PHOTOMETRIC_CFA: u16 = 32803;
/// PhotometricInterpretation の RGB。
const PHOTOMETRIC_RGB: u16 = 2;
/// PhotometricInterpretation の YCbCr（JPEG のプレビュー）。
const PHOTOMETRIC_YCBCR: u16 = 6;
/// Compression の「圧縮なし」。
const COMPRESSION_NONE: u16 = 1;
/// Compression の JPEG（DNG のプレビューで使う値）。
const COMPRESSION_JPEG: u16 = 7;
/// Software タグの値。LibRaw は "Adobe" や "dcraw" で始まる Software の DNG を RAW として
/// 扱わないため（LibRaw の `parse_tiff_ifd`）、それらで始まらない値にする。
const SOFTWARE: &str = "GenzoParis genzo-raw (synthetic DNG)";

/// TIFF の値。
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Byte(Vec<u8>),
    /// NUL を含まない ASCII の文字列（書くときに NUL を付ける）。
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    SRational(Vec<(i32, i32)>),
    Undefined(Vec<u8>),
}

impl Value {
    /// TIFF の型の番号。
    fn type_code(&self) -> u16 {
        match self {
            Value::Byte(_) => 1,
            Value::Ascii(_) => 2,
            Value::Short(_) => 3,
            Value::Long(_) => 4,
            Value::Rational(_) => 5,
            Value::Undefined(_) => 7,
            Value::SRational(_) => 10,
        }
    }

    /// 値の数（TIFF の Count）。
    fn count(&self) -> u32 {
        let n = match self {
            Value::Byte(v) | Value::Undefined(v) => v.len(),
            Value::Ascii(s) => s.len() + 1,
            Value::Short(v) => v.len(),
            Value::Long(v) => v.len(),
            Value::Rational(v) => v.len(),
            Value::SRational(v) => v.len(),
        };
        u32::try_from(n).expect("タグの値の数は u32 に収まる")
    }

    /// リトルエンディアンのバイト列。
    fn bytes(&self) -> Vec<u8> {
        match self {
            Value::Byte(v) | Value::Undefined(v) => v.clone(),
            Value::Ascii(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                b
            }
            Value::Short(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            Value::Long(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            Value::Rational(v) => v
                .iter()
                .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
                .collect(),
            Value::SRational(v) => v
                .iter()
                .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
                .collect(),
        }
    }
}

/// 2 バイト境界に切り上げる。
fn align2(n: usize) -> usize {
    n + (n & 1)
}

/// TIFF の IFD。
#[derive(Debug, Default)]
struct Ifd {
    entries: BTreeMap<u16, Value>,
}

impl Ifd {
    fn set(&mut self, tag: u16, value: Value) {
        self.entries.insert(tag, value);
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// IFD の全体（項目・次の IFD の位置・4 バイトを超える値）のバイト数。
    fn byte_size(&self) -> usize {
        let extra: usize = self
            .entries
            .values()
            .map(|v| v.bytes().len())
            .filter(|&n| n > 4)
            .map(align2)
            .sum();
        2 + 12 * self.entries.len() + 4 + extra
    }

    /// `offset` の位置に置く IFD を `out` に書く（`out.len()` は `offset` と同じであること）。
    fn write(&self, out: &mut Vec<u8>, offset: usize, next_ifd: u32) -> Result<(), DngError> {
        debug_assert_eq!(out.len(), offset);
        let count = u16::try_from(self.entries.len())
            .map_err(|_| DngError::InvalidValue("IFD の項目が多すぎます".to_owned()))?;
        out.extend_from_slice(&count.to_le_bytes());
        let mut data_pos = offset + 2 + 12 * self.entries.len() + 4;
        let mut data = Vec::new();
        for (&tag, value) in &self.entries {
            let bytes = value.bytes();
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&value.type_code().to_le_bytes());
            out.extend_from_slice(&value.count().to_le_bytes());
            if bytes.len() <= 4 {
                let mut inline = [0u8; 4];
                inline[..bytes.len()].copy_from_slice(&bytes);
                out.extend_from_slice(&inline);
            } else {
                out.extend_from_slice(&offset_u32(data_pos)?.to_le_bytes());
                data.extend_from_slice(&bytes);
                if bytes.len() % 2 == 1 {
                    data.push(0);
                }
                data_pos += align2(bytes.len());
            }
        }
        out.extend_from_slice(&next_ifd.to_le_bytes());
        out.extend_from_slice(&data);
        Ok(())
    }
}

/// ファイルの中の位置を TIFF の 32bit の位置にする。
fn offset_u32(pos: usize) -> Result<u32, DngError> {
    u32::try_from(pos).map_err(|_| DngError::TooLarge)
}

/// 最大公約数。
fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// 有理数の分母の最大値（10⁶。f32 の有効桁数（約 7 桁）に合わせた仮置きの精度）。
const MAX_DENOMINATOR: u64 = 1_000_000;

/// 0 以上の値を、分子が `max_num` 以下の分数（分子, 分母）にする。整数なら分母 1、それ以外は
/// 分母 10⁶ 以下（分子が収まるまで 10 分の 1 にする）で丸めて約分する。
fn ratio(v: f64, max_num: u64) -> Option<(u64, u64)> {
    if !(v.is_finite() && v >= 0.0) {
        return None;
    }
    let max = max_num as f64;
    if v.fract() == 0.0 && v <= max {
        return Some((v as u64, 1));
    }
    let mut den = MAX_DENOMINATOR;
    while den > 1 && v * den as f64 > max {
        den /= 10;
    }
    let num = (v * den as f64).round();
    if num > max {
        return None;
    }
    let num = num as u64;
    let g = gcd(num, den).max(1);
    Some((num / g, den / g))
}

/// 0 以上の値を RATIONAL にする（[`ratio`]）。
fn rational(v: f64) -> Result<(u32, u32), DngError> {
    ratio(v, u64::from(u32::MAX))
        .map(|(n, d)| (n as u32, d as u32))
        .ok_or_else(|| DngError::InvalidValue(format!("RATIONAL にできない値です: {v}")))
}

/// 値を SRATIONAL にする（[`ratio`]）。
fn srational(v: f64) -> Result<(i32, i32), DngError> {
    let (n, d) = ratio(v.abs(), i32::MAX as u64)
        .ok_or_else(|| DngError::InvalidValue(format!("SRATIONAL にできない値です: {v}")))?;
    let (n, d) = (n as i32, d as i32);
    Ok((if v < 0.0 { -n } else { n }, d))
}

/// シャッター速度（秒）を RATIONAL にする。1 秒未満で 1/n に近ければ 1/n の形にする。
fn exposure_time(seconds: f32) -> Result<(u32, u32), DngError> {
    let s = f64::from(seconds);
    if s > 0.0 && s < 1.0 {
        let inv = 1.0 / s;
        if (inv - inv.round()).abs() < 1e-3 && inv.round() <= f64::from(u32::MAX) {
            return Ok((1, inv.round() as u32));
        }
    }
    rational(s)
}

/// ASCII の文字列か確かめる（NUL を含まない）。
fn ascii(name: &str, s: &str) -> Result<String, DngError> {
    if s.is_ascii() && !s.contains('\0') {
        Ok(s.to_owned())
    } else {
        Err(DngError::InvalidValue(format!(
            "{name} は NUL を含まない ASCII の文字列にしてください: {s:?}"
        )))
    }
}

/// EXIF の日時（`"YYYY:MM:DD HH:MM:SS"` と任意の `".小数秒"`）を、DateTimeOriginal と
/// SubSecTimeOriginal に分ける。
fn split_exif_datetime(s: &str) -> Result<(String, Option<String>), DngError> {
    let invalid = || {
        DngError::InvalidValue(format!(
            "撮影日時は EXIF の形式（YYYY:MM:DD HH:MM:SS[.小数秒]）にしてください: {s:?}"
        ))
    };
    let b = s.as_bytes();
    if b.len() < 19 {
        return Err(invalid());
    }
    let pattern_ok = b[..19].iter().enumerate().all(|(i, &c)| match i {
        4 | 7 | 13 | 16 => c == b':',
        10 => c == b' ',
        _ => c.is_ascii_digit(),
    });
    if !pattern_ok {
        return Err(invalid());
    }
    let rest = &s[19..];
    if rest.is_empty() {
        return Ok((s.to_owned(), None));
    }
    let frac = rest.strip_prefix('.').ok_or_else(invalid)?;
    if frac.is_empty() || !frac.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    Ok((s[..19].to_owned(), Some(frac.to_owned())))
}

/// 度を度・分・秒の RATIONAL にする（0 以上の値）。
fn dms(degrees: f64) -> Result<Vec<(u32, u32)>, DngError> {
    let d = degrees.floor();
    let minutes_total = (degrees - d) * 60.0;
    let m = minutes_total.floor();
    let s = (minutes_total - m) * 60.0;
    Ok(vec![rational(d)?, rational(m)?, rational(s)?])
}

/// GPS IFD を作る。
fn gps_ifd(gps: &GpsCoord) -> Result<Ifd, DngError> {
    if !gps.is_valid() {
        return Err(DngError::InvalidValue(format!(
            "GPS の座標が不正です: {gps:?}"
        )));
    }
    let mut ifd = Ifd::default();
    ifd.set(tag::GPS_VERSION_ID, Value::Byte(vec![2, 3, 0, 0]));
    let lat_ref = if gps.lat < 0.0 { "S" } else { "N" };
    let lon_ref = if gps.lon < 0.0 { "W" } else { "E" };
    ifd.set(tag::GPS_LATITUDE_REF, Value::Ascii(lat_ref.to_owned()));
    ifd.set(tag::GPS_LATITUDE, Value::Rational(dms(gps.lat.abs())?));
    ifd.set(tag::GPS_LONGITUDE_REF, Value::Ascii(lon_ref.to_owned()));
    ifd.set(tag::GPS_LONGITUDE, Value::Rational(dms(gps.lon.abs())?));
    Ok(ifd)
}

/// EXIF IFD を作る（書く項目がなければ空）。
fn exif_ifd(m: &PhotoMetadata) -> Result<Ifd, DngError> {
    let mut ifd = Ifd::default();
    if let Some(s) = m.shutter_s {
        ifd.set(tag::EXPOSURE_TIME, Value::Rational(vec![exposure_time(s)?]));
    }
    if let Some(f) = m.aperture {
        ifd.set(
            tag::F_NUMBER,
            Value::Rational(vec![rational(f64::from(f))?]),
        );
    }
    if let Some(iso) = m.iso {
        match u16::try_from(iso) {
            Ok(v) if v < u16::MAX => ifd.set(tag::ISO_SPEED_RATINGS, Value::Short(vec![v])),
            _ => {
                // EXIF 2.3: 65535 以上は ISOSpeedRatings を 65535 にし、値は別のタグに書く。
                ifd.set(tag::ISO_SPEED_RATINGS, Value::Short(vec![u16::MAX]));
                // SensitivityType = 2（推奨露光指数）。
                ifd.set(tag::SENSITIVITY_TYPE, Value::Short(vec![2]));
                ifd.set(tag::RECOMMENDED_EXPOSURE_INDEX, Value::Long(vec![iso]));
            }
        }
    }
    if let Some(dt) = &m.capture.datetime {
        let (main, subsec) = split_exif_datetime(dt)?;
        ifd.set(tag::DATE_TIME_ORIGINAL, Value::Ascii(main));
        if let Some(subsec) = subsec {
            ifd.set(tag::SUB_SEC_TIME_ORIGINAL, Value::Ascii(subsec));
        }
    }
    if let Some(offset) = &m.capture.offset {
        ifd.set(
            tag::OFFSET_TIME_ORIGINAL,
            Value::Ascii(ascii("撮影日時のオフセット", offset)?),
        );
    }
    if let Some(f) = m.focal_mm {
        ifd.set(
            tag::FOCAL_LENGTH,
            Value::Rational(vec![rational(f64::from(f))?]),
        );
    }
    if let Some(lens) = &m.lens {
        ifd.set(tag::LENS_MODEL, Value::Ascii(ascii("レンズ名", lens)?));
    }
    if !ifd.is_empty() {
        ifd.set(tag::EXIF_VERSION, Value::Undefined(b"0232".to_vec()));
    }
    Ok(ifd)
}

/// CFAPattern の色の番号（DNG / TIFF-EP: 0 = 赤、1 = 緑、2 = 青）。
fn cfa_plane(c: CfaColor) -> u8 {
    match c {
        CfaColor::Red => 0,
        CfaColor::Green1 | CfaColor::Green2 => 1,
        CfaColor::Blue => 2,
    }
}

/// 3 × 3 の行列を SRATIONAL の 9 個（行優先）にする。
fn matrix_srational(m: &[[f32; 3]; 3]) -> Result<Value, DngError> {
    let v = m
        .iter()
        .flatten()
        .map(|&x| srational(f64::from(x)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::SRational(v))
}

/// プレビューの IFD とデータを作る。
fn preview_ifd(p: &DngPreview) -> Result<(Ifd, &[u8]), DngError> {
    let (width, height, data, jpeg) = match p {
        DngPreview::Rgb8 {
            width,
            height,
            data,
        } => {
            let expected = u64::from(*width) * u64::from(*height) * 3;
            if *width == 0 || *height == 0 || expected != data.len() as u64 {
                return Err(DngError::InvalidValue(format!(
                    "プレビューの長さ {} が寸法 {width} × {height} × 3 と一致しません",
                    data.len()
                )));
            }
            (*width, *height, data.as_slice(), false)
        }
        DngPreview::Jpeg {
            width,
            height,
            data,
        } => {
            if *width == 0 || *height == 0 || !data.starts_with(&[0xFF, 0xD8]) {
                return Err(DngError::InvalidValue(
                    "JPEG のプレビューが不正です（寸法が 0 か、SOI がありません）".to_owned(),
                ));
            }
            (*width, *height, data.as_slice(), true)
        }
    };
    let mut ifd = Ifd::default();
    ifd.set(tag::NEW_SUBFILE_TYPE, Value::Long(vec![1]));
    ifd.set(tag::IMAGE_WIDTH, Value::Long(vec![width]));
    ifd.set(tag::IMAGE_LENGTH, Value::Long(vec![height]));
    ifd.set(tag::BITS_PER_SAMPLE, Value::Short(vec![8, 8, 8]));
    ifd.set(
        tag::COMPRESSION,
        Value::Short(vec![if jpeg {
            COMPRESSION_JPEG
        } else {
            COMPRESSION_NONE
        }]),
    );
    ifd.set(
        tag::PHOTOMETRIC,
        Value::Short(vec![if jpeg {
            PHOTOMETRIC_YCBCR
        } else {
            PHOTOMETRIC_RGB
        }]),
    );
    // 位置は配置を決めてから書き換える。
    ifd.set(tag::STRIP_OFFSETS, Value::Long(vec![0]));
    ifd.set(tag::SAMPLES_PER_PIXEL, Value::Short(vec![3]));
    ifd.set(tag::ROWS_PER_STRIP, Value::Long(vec![height]));
    ifd.set(
        tag::STRIP_BYTE_COUNTS,
        Value::Long(vec![offset_u32(data.len())?]),
    );
    ifd.set(tag::PLANAR_CONFIGURATION, Value::Short(vec![1]));
    Ok((ifd, data))
}

/// [`RawImage`] を DNG のバイト列にする。
///
/// 入力は [`RawImage::validate`] で検証する。
pub fn encode(image: &RawImage, options: &DngOptions) -> Result<Vec<u8>, DngError> {
    image.validate()?;
    let m = &image.metadata;
    let mg = options.margins;
    let full_w = u64::from(image.width) + u64::from(mg.left) + u64::from(mg.right);
    let full_h = u64::from(image.height) + u64::from(mg.top) + u64::from(mg.bottom);
    let full_w = u32::try_from(full_w).map_err(|_| DngError::TooLarge)?;
    let full_h = u32::try_from(full_h).map_err(|_| DngError::TooLarge)?;
    let raw_bytes = u64::from(full_w) * u64::from(full_h) * 2;
    let raw_bytes = u32::try_from(raw_bytes).map_err(|_| DngError::TooLarge)?;

    let mut main = Ifd::default();
    main.set(tag::NEW_SUBFILE_TYPE, Value::Long(vec![0]));
    main.set(tag::IMAGE_WIDTH, Value::Long(vec![full_w]));
    main.set(tag::IMAGE_LENGTH, Value::Long(vec![full_h]));
    main.set(tag::BITS_PER_SAMPLE, Value::Short(vec![16]));
    main.set(tag::COMPRESSION, Value::Short(vec![COMPRESSION_NONE]));
    main.set(tag::PHOTOMETRIC, Value::Short(vec![PHOTOMETRIC_CFA]));
    if let Some(make) = &m.make {
        main.set(tag::MAKE, Value::Ascii(ascii("メーカー", make)?));
    }
    if let Some(model) = &m.model {
        main.set(tag::MODEL, Value::Ascii(ascii("機種", model)?));
    }
    main.set(tag::STRIP_OFFSETS, Value::Long(vec![0]));
    main.set(
        tag::ORIENTATION,
        Value::Short(vec![u16::from(m.orientation.to_exif())]),
    );
    main.set(tag::SAMPLES_PER_PIXEL, Value::Short(vec![1]));
    main.set(tag::ROWS_PER_STRIP, Value::Long(vec![full_h]));
    main.set(tag::STRIP_BYTE_COUNTS, Value::Long(vec![raw_bytes]));
    main.set(tag::PLANAR_CONFIGURATION, Value::Short(vec![1]));
    main.set(tag::SOFTWARE, Value::Ascii(SOFTWARE.to_owned()));
    main.set(tag::CFA_REPEAT_PATTERN_DIM, Value::Short(vec![2, 2]));
    // CFA の配列は ActiveArea の左上を原点とする（[`RawImage::cfa`] と同じ）。
    let colors = image.cfa.colors();
    let positions = [(0usize, 0usize), (1, 0), (0, 1), (1, 1)];
    main.set(
        tag::CFA_PATTERN,
        Value::Byte(
            positions
                .iter()
                .map(|&(x, y)| cfa_plane(colors[y][x]))
                .collect(),
        ),
    );
    main.set(tag::DNG_VERSION, Value::Byte(vec![1, 4, 0, 0]));
    main.set(tag::DNG_BACKWARD_VERSION, Value::Byte(vec![1, 1, 0, 0]));
    let unique = match &options.unique_camera_model {
        Some(u) => ascii("UniqueCameraModel", u)?,
        None => m
            .camera_name()
            .map(|n| ascii("UniqueCameraModel", &n))
            .transpose()?
            .unwrap_or_else(|| "GenzoParis Synthetic".to_owned()),
    };
    main.set(tag::UNIQUE_CAMERA_MODEL, Value::Ascii(unique));
    main.set(tag::CFA_PLANE_COLOR, Value::Byte(vec![0, 1, 2]));
    main.set(tag::CFA_LAYOUT, Value::Short(vec![1]));
    main.set(tag::BLACK_LEVEL_REPEAT_DIM, Value::Short(vec![2, 2]));
    let black = positions
        .iter()
        .map(|&(x, y)| rational(f64::from(image.black_level_at(x as u32, y as u32))))
        .collect::<Result<Vec<_>, _>>()?;
    // 行優先（(0,0)、(1,0)、(0,1)、(1,1)）。
    main.set(tag::BLACK_LEVEL, Value::Rational(black));
    main.set(
        tag::WHITE_LEVEL,
        Value::Long(vec![image.white_level.round() as u32]),
    );
    if let Some(cm) = &image.cam_xyz {
        main.set(tag::COLOR_MATRIX_1, matrix_srational(cm)?);
        main.set(
            tag::CALIBRATION_ILLUMINANT_1,
            Value::Short(vec![options.illuminant_1]),
        );
    }
    if let Some((cm2, ill2)) = &options.color_matrix_2 {
        main.set(tag::COLOR_MATRIX_2, matrix_srational(cm2)?);
        main.set(tag::CALIBRATION_ILLUMINANT_2, Value::Short(vec![*ill2]));
    }
    let wb = image.as_shot_wb;
    let neutral = [wb[0], wb[1], wb[2]]
        .iter()
        .map(|&c| rational(1.0 / f64::from(c)))
        .collect::<Result<Vec<_>, _>>()?;
    main.set(tag::AS_SHOT_NEUTRAL, Value::Rational(neutral));
    if !mg.is_zero() {
        // ActiveArea は 上・左・下・右。
        main.set(
            tag::ACTIVE_AREA,
            Value::Long(vec![
                mg.top,
                mg.left,
                mg.top + image.height,
                mg.left + image.width,
            ]),
        );
    }

    let exif = exif_ifd(m)?;
    let gps = m.gps.as_ref().map(gps_ifd).transpose()?;
    let preview = options.preview.as_ref().map(preview_ifd).transpose()?;
    if !exif.is_empty() {
        main.set(tag::EXIF_IFD, Value::Long(vec![0]));
    }
    if gps.is_some() {
        main.set(tag::GPS_IFD, Value::Long(vec![0]));
    }
    if preview.is_some() {
        main.set(tag::SUB_IFDS, Value::Long(vec![0]));
    }

    // 配置: ヘッダ → IFD0 → EXIF IFD → GPS IFD → プレビューの IFD → プレビューのデータ → RAW。
    // ポインタの値（LONG 1 つ）は IFD の大きさを変えないので、先に大きさを決めて後から書き換える。
    let main_off = 8usize;
    let mut pos = main_off + main.byte_size();
    let exif_off = pos;
    if !exif.is_empty() {
        pos += exif.byte_size();
    }
    let gps_off = pos;
    if let Some(g) = &gps {
        pos += g.byte_size();
    }
    let preview_off = pos;
    let mut preview_data_off = pos;
    if let Some((ifd, data)) = &preview {
        preview_data_off = align2(pos + ifd.byte_size());
        pos = preview_data_off + data.len();
    }
    let raw_off = align2(pos);
    let total = raw_off + raw_bytes as usize;
    offset_u32(total)?;

    if !exif.is_empty() {
        main.set(tag::EXIF_IFD, Value::Long(vec![offset_u32(exif_off)?]));
    }
    if gps.is_some() {
        main.set(tag::GPS_IFD, Value::Long(vec![offset_u32(gps_off)?]));
    }
    let mut preview = preview;
    if let Some((ifd, _)) = &mut preview {
        main.set(tag::SUB_IFDS, Value::Long(vec![offset_u32(preview_off)?]));
        ifd.set(
            tag::STRIP_OFFSETS,
            Value::Long(vec![offset_u32(preview_data_off)?]),
        );
    }
    main.set(tag::STRIP_OFFSETS, Value::Long(vec![offset_u32(raw_off)?]));

    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&offset_u32(main_off)?.to_le_bytes());
    main.write(&mut out, main_off, 0)?;
    if !exif.is_empty() {
        exif.write(&mut out, exif_off, 0)?;
    }
    if let Some(g) = &gps {
        g.write(&mut out, gps_off, 0)?;
    }
    if let Some((ifd, data)) = &preview {
        ifd.write(&mut out, preview_off, 0)?;
        out.resize(preview_data_off, 0);
        out.extend_from_slice(data);
    }
    out.resize(raw_off, 0);
    write_pixels(&mut out, image, options);
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

/// 余白を含む RAW の画素を書く（行優先、リトルエンディアンの u16）。
fn write_pixels(out: &mut Vec<u8>, image: &RawImage, options: &DngOptions) {
    let mg = options.margins;
    let fill = options.margin_value.to_le_bytes();
    let full_w = (image.width + mg.left + mg.right) as usize;
    let margin_row: Vec<u8> = fill.iter().copied().cycle().take(full_w * 2).collect();
    for _ in 0..mg.top {
        out.extend_from_slice(&margin_row);
    }
    let w = image.width as usize;
    for row in image.data.chunks_exact(w) {
        for _ in 0..mg.left {
            out.extend_from_slice(&fill);
        }
        for v in row {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for _ in 0..mg.right {
            out.extend_from_slice(&fill);
        }
    }
    for _ in 0..mg.bottom {
        out.extend_from_slice(&margin_row);
    }
}

/// [`encode`] した DNG をファイルに書く。
///
/// 既存のファイルは上書きしない（同じパスがあれば [`std::io::ErrorKind::AlreadyExists`]）。
pub fn write_file(
    path: impl AsRef<Path>,
    image: &RawImage,
    options: &DngOptions,
) -> Result<(), DngError> {
    let bytes = encode(image, options)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
