//! 書き出すファイルの Exif（EXP-04・SEC-03）。最小限の TIFF の構造を自前で書く。
//!
//! 書くタグ（Exif 2.32。CIPA DC-008-2019）:
//!
//! | IFD | タグ | 値 |
//! |---|---|---|
//! | 0th | Make (0x010F)・Model (0x0110) | カメラ |
//! | 0th | Orientation (0x0112) | **常に 1**（現像で向きを画素に反映済みのため） |
//! | 0th | XResolution・YResolution・ResolutionUnit | [`EXPORT_RESOLUTION_PPI`] ppi |
//! | 0th | Software (0x0131) | [`EXIF_SOFTWARE`] |
//! | Exif | ExposureTime・FNumber・PhotographicSensitivity（ISO）・FocalLength・LensModel | 撮影情報 |
//! | Exif | ExifVersion (0x9000) | "0232" |
//! | Exif | DateTimeOriginal・SubSecTimeOriginal・OffsetTimeOriginal | 撮影日時 |
//! | Exif | ColorSpace (0xA001) | sRGB なら 1、それ以外は 0xFFFF（未較正。ICC で色空間を示す） |
//! | Exif | PixelXDimension・PixelYDimension | 書き出した画像の寸法 |
//! | GPS | GPSVersionID・緯度・経度 | **`remove_gps = false` のときだけ** 書く |
//!
//! ASCII のタグは、7bit の印字可能な文字以外を `?` に置き換え、[`EXIF_MAX_ASCII_CHARS`] 文字で切る
//! （Exif の ASCII 型は 7bit の文字だけを許すため）。

use chrono::{FixedOffset, NaiveDateTime, Timelike};
use genzo_model::capture_time::{format_offset, parse_datetime, parse_offset};
use genzo_model::{CaptureTime, GpsCoord, OutputColorSpace, PhotoMetadata, TzSource};

use crate::error::{MediaError, Result};

/// Exif の Software タグに書く文字列。
pub const EXIF_SOFTWARE: &str = "GenzoParis";

/// 書き出すファイルの解像度（ppi。仮置き）。
///
/// 画素数には影響しない印刷用の目安。Exif の既定値（72 dpi）に合わせた。印刷の設定を作るときに
/// 見直す。
pub const EXPORT_RESOLUTION_PPI: u32 = 72;

/// ASCII のタグの最大の文字数（仮置き）。メーカー名・機種名・レンズ名には十分な長さ。
pub const EXIF_MAX_ASCII_CHARS: usize = 255;

/// Exif のバージョン（ExifVersion タグの値。OffsetTimeOriginal は 2.31 で追加された）。
const EXIF_VERSION: &[u8; 4] = b"0232";

/// GPS の IFD のバージョン（GPSVersionID）。
///
/// GPSVersionID の値は Exif の版によって決まる（Exif 2.2 は 2.2.0.0、Exif 2.3 以降は 2.3.0.0。
/// kamadak-exif もこのタグを「Exif の版に依存する」としている）。ExifVersion を 0232 と書くので、
/// 2.3.0.0 にそろえる（レビューで 2.2.0.0 から変更。CIPA DC-008 の原典での確認を推奨）。
const GPS_VERSION: [u8; 4] = [2, 3, 0, 0];

/// タグの番号。
pub(crate) mod tag {
    pub const MAKE: u16 = 0x010F;
    pub const MODEL: u16 = 0x0110;
    pub const ORIENTATION: u16 = 0x0112;
    pub const X_RESOLUTION: u16 = 0x011A;
    pub const Y_RESOLUTION: u16 = 0x011B;
    pub const RESOLUTION_UNIT: u16 = 0x0128;
    pub const SOFTWARE: u16 = 0x0131;
    pub const EXIF_IFD: u16 = 0x8769;
    pub const GPS_IFD: u16 = 0x8825;

    pub const EXPOSURE_TIME: u16 = 0x829A;
    pub const F_NUMBER: u16 = 0x829D;
    pub const PHOTOGRAPHIC_SENSITIVITY: u16 = 0x8827;
    pub const SENSITIVITY_TYPE: u16 = 0x8830;
    pub const ISO_SPEED: u16 = 0x8833;
    pub const EXIF_VERSION: u16 = 0x9000;
    pub const DATE_TIME_ORIGINAL: u16 = 0x9003;
    pub const OFFSET_TIME_ORIGINAL: u16 = 0x9011;
    pub const FOCAL_LENGTH: u16 = 0x920A;
    pub const SUB_SEC_TIME_ORIGINAL: u16 = 0x9291;
    pub const COLOR_SPACE: u16 = 0xA001;
    pub const PIXEL_X_DIMENSION: u16 = 0xA002;
    pub const PIXEL_Y_DIMENSION: u16 = 0xA003;
    pub const LENS_MODEL: u16 = 0xA434;

    pub const GPS_VERSION_ID: u16 = 0x0000;
    pub const GPS_LATITUDE_REF: u16 = 0x0001;
    pub const GPS_LATITUDE: u16 = 0x0002;
    pub const GPS_LONGITUDE_REF: u16 = 0x0003;
    pub const GPS_LONGITUDE: u16 = 0x0004;
}

/// 書き出すファイルに書く Exif の情報。
///
/// [`ExifData::from_photo_metadata`] で写真のメタデータから作る。書き出しの設定で GPS を削除する
/// （`remove_gps = true`）ときは、書き出しの関数が [`ExifData::without_gps`] を適用する。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExifData {
    /// カメラのメーカー。
    pub make: Option<String>,
    /// カメラの機種。
    pub model: Option<String>,
    /// レンズ名。
    pub lens_model: Option<String>,
    /// 撮影日時（現地時刻。小数秒は SubSecTimeOriginal にミリ秒で書く）。
    pub datetime_original: Option<NaiveDateTime>,
    /// 撮影日時のオフセット（OffsetTimeOriginal）。
    pub offset_time_original: Option<FixedOffset>,
    /// シャッター速度（秒）。
    pub exposure_time_s: Option<f64>,
    /// 絞り値（F 値）。
    pub f_number: Option<f64>,
    /// ISO 感度。
    pub iso: Option<u32>,
    /// 焦点距離（mm）。
    pub focal_length_mm: Option<f64>,
    /// GPS の座標。
    pub gps: Option<GpsCoord>,
}

impl ExifData {
    /// 写真のメタデータ（元のファイルから読んだ値）から作る。
    ///
    /// 撮影日時は、元の文字列（`capture.datetime`・`capture.offset`）を解析できたときだけ書く。
    /// ユーザーが補正した日時を書くときは [`with_capture_time`](Self::with_capture_time) を使う。
    pub fn from_photo_metadata(meta: &PhotoMetadata) -> Self {
        let parsed = meta
            .capture
            .datetime
            .as_deref()
            .and_then(|s| parse_datetime(s).ok().flatten());
        let (datetime_original, embedded_offset) = match parsed {
            Some((dt, off)) => (Some(dt), off),
            None => (None, None),
        };
        let offset_time_original = datetime_original.and(embedded_offset.or_else(|| {
            meta.capture
                .offset
                .as_deref()
                .and_then(|s| parse_offset(s).ok())
        }));
        Self {
            make: meta.make.clone(),
            model: meta.model.clone(),
            lens_model: meta.lens.clone(),
            datetime_original,
            offset_time_original,
            exposure_time_s: meta.shutter_s.map(f64::from),
            f_number: meta.aperture.map(f64::from),
            iso: meta.iso,
            focal_length_mm: meta.focal_mm.map(f64::from),
            gps: meta.gps,
        }
    }

    /// カタログの撮影日時（時計のずれ・タイムゾーンの補正後。LIB-16）で、撮影日時を置き換える。
    ///
    /// 補正後の UTC を、推定に使ったオフセットの現地時刻にして書く。オフセット
    /// （OffsetTimeOriginal）は、ファイルに記録されていたか、ユーザーが設定した場合だけ書く
    /// （既定のタイムゾーンで推定しただけのオフセットは、事実として書かない）。
    /// 日時が分からなければ、撮影日時を書かない。
    pub fn with_capture_time(mut self, ct: &CaptureTime) -> Self {
        match ct.local() {
            Some(local) => {
                self.datetime_original = Some(local.naive_local());
                self.offset_time_original =
                    (ct.tz_source != TzSource::UserDefault).then(|| *local.offset());
            }
            None => {
                self.datetime_original = None;
                self.offset_time_original = None;
            }
        }
        self
    }

    /// GPS の情報を除く（EXP-04・SEC-03）。
    pub fn without_gps(mut self) -> Self {
        self.gps = None;
        self
    }
}

/// 書き出す画像の情報（Exif のタグの一部を決める）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExifImageInfo {
    pub width: u32,
    pub height: u32,
    pub color_space: OutputColorSpace,
}

/// TIFF のバイト順。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ByteOrder {
    /// ビッグエンディアン（"MM"）。
    Big,
    /// リトルエンディアン（"II"）。
    Little,
}

impl ByteOrder {
    /// この環境のバイト順（tiff crate はこの順で書く）。
    pub fn native() -> Self {
        if cfg!(target_endian = "big") {
            Self::Big
        } else {
            Self::Little
        }
    }

    fn u16(self, v: u16) -> [u8; 2] {
        match self {
            Self::Big => v.to_be_bytes(),
            Self::Little => v.to_le_bytes(),
        }
    }

    fn u32(self, v: u32) -> [u8; 4] {
        match self {
            Self::Big => v.to_be_bytes(),
            Self::Little => v.to_le_bytes(),
        }
    }
}

/// IFD のエントリの値。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Byte(Vec<u8>),
    /// 終端の NUL は書くときに付ける。
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    Undefined(Vec<u8>),
}

impl Value {
    /// TIFF の型の番号。
    pub fn type_code(&self) -> u16 {
        match self {
            Self::Byte(_) => 1,
            Self::Ascii(_) => 2,
            Self::Short(_) => 3,
            Self::Long(_) => 4,
            Self::Rational(_) => 5,
            Self::Undefined(_) => 7,
        }
    }

    /// 要素の数（ASCII は終端の NUL を含む）。
    pub fn count(&self) -> u32 {
        let n = match self {
            Self::Byte(v) | Self::Undefined(v) => v.len(),
            Self::Ascii(s) => s.len() + 1,
            Self::Short(v) => v.len(),
            Self::Long(v) => v.len(),
            Self::Rational(v) => v.len(),
        };
        u32::try_from(n).expect("Exif の値は小さい")
    }

    /// 値のバイト列。
    pub fn bytes(&self, order: ByteOrder) -> Vec<u8> {
        match self {
            Self::Byte(v) | Self::Undefined(v) => v.clone(),
            Self::Ascii(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                b
            }
            Self::Short(v) => v.iter().flat_map(|&x| order.u16(x)).collect(),
            Self::Long(v) => v.iter().flat_map(|&x| order.u32(x)).collect(),
            Self::Rational(v) => v
                .iter()
                .flat_map(|&(n, d)| {
                    let mut b = order.u32(n).to_vec();
                    b.extend_from_slice(&order.u32(d));
                    b
                })
                .collect(),
        }
    }
}

/// IFD のエントリ。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub tag: u16,
    pub value: Value,
}

/// 0th IFD・Exif IFD・GPS IFD のエントリ（IFD へのポインタのエントリは含まない。番号順）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExifIfds {
    pub ifd0: Vec<Entry>,
    pub exif: Vec<Entry>,
    pub gps: Option<Vec<Entry>>,
}

/// ASCII のタグに書ける文字列にする。空なら `None`。
pub(crate) fn sanitize_ascii(s: &str) -> Option<String> {
    let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    let out: String = t
        .chars()
        .take(EXIF_MAX_ASCII_CHARS)
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .collect();
    let out = out.trim_end().to_owned();
    (!out.is_empty()).then_some(out)
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// 有理数を約分する。
fn reduce(n: u64, d: u64) -> Option<(u32, u32)> {
    let g = gcd(n, d).max(1);
    Some((u32::try_from(n / g).ok()?, u32::try_from(d / g).ok()?))
}

/// 正の値を、分母 `denom` で四捨五入した有理数にする（約分する）。0 以下・非有限・大きすぎる値は `None`。
pub(crate) fn to_rational(v: f64, denom: u32) -> Option<(u32, u32)> {
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    let n = (v * f64::from(denom)).round();
    if n < 1.0 || n > f64::from(u32::MAX) {
        return None;
    }
    reduce(n as u64, u64::from(denom))
}

/// シャッター速度（秒）を有理数にする。
///
/// 1 秒未満で `1/n` に近い値（相対誤差 0.1% 以内）は `1/n`（例: 1/250）、それ以外は 1 秒未満なら
/// 分母 1,000,000、1 秒以上なら分母 10 で表す（例: 0.3 → 3/10、2.5 → 5/2）。
pub(crate) fn exposure_rational(t: f64) -> Option<(u32, u32)> {
    if !t.is_finite() || t <= 0.0 {
        return None;
    }
    if t < 1.0 {
        let n = (1.0 / t).round();
        if n >= 1.0 && n <= f64::from(u32::MAX) && ((1.0 / n) - t).abs() / t < 1e-3 {
            return Some((1, n as u32));
        }
        return to_rational(t, 1_000_000);
    }
    to_rational(t, 10)
}

/// 10 進数の度を、度・分・秒（秒は 1/10000 単位）の有理数にする。
pub(crate) fn degrees_to_dms(deg: f64) -> [(u32, u32); 3] {
    // 0.0001 秒単位の整数にしてから分けるので、60 秒への繰り上がりの誤りが起きない。
    let total = (deg.abs() * 3600.0 * 10_000.0).round() as u64;
    let d = total / 36_000_000;
    let rem = total % 36_000_000;
    let m = rem / 600_000;
    let s = rem % 600_000;
    [(d as u32, 1), (m as u32, 1), (s as u32, 10_000)]
}

/// Exif の IFD のエントリを作る。
pub(crate) fn build_ifds(data: &ExifData, info: &ExifImageInfo) -> ExifIfds {
    let mut ifd0 = Vec::new();
    let ascii = |s: &Option<String>| s.as_deref().and_then(sanitize_ascii);
    if let Some(make) = ascii(&data.make) {
        ifd0.push(Entry {
            tag: tag::MAKE,
            value: Value::Ascii(make),
        });
    }
    if let Some(model) = ascii(&data.model) {
        ifd0.push(Entry {
            tag: tag::MODEL,
            value: Value::Ascii(model),
        });
    }
    ifd0.push(Entry {
        tag: tag::ORIENTATION,
        value: Value::Short(vec![1]),
    });
    for t in [tag::X_RESOLUTION, tag::Y_RESOLUTION] {
        ifd0.push(Entry {
            tag: t,
            value: Value::Rational(vec![(EXPORT_RESOLUTION_PPI, 1)]),
        });
    }
    ifd0.push(Entry {
        tag: tag::RESOLUTION_UNIT,
        // 2 = インチ。
        value: Value::Short(vec![2]),
    });
    ifd0.push(Entry {
        tag: tag::SOFTWARE,
        value: Value::Ascii(EXIF_SOFTWARE.to_owned()),
    });

    let mut exif = Vec::new();
    if let Some(r) = data.exposure_time_s.and_then(exposure_rational) {
        exif.push(Entry {
            tag: tag::EXPOSURE_TIME,
            value: Value::Rational(vec![r]),
        });
    }
    if let Some(r) = data.f_number.and_then(|f| to_rational(f, 100)) {
        exif.push(Entry {
            tag: tag::F_NUMBER,
            value: Value::Rational(vec![r]),
        });
    }
    if let Some(iso) = data.iso.filter(|&v| v > 0) {
        // 65535 を超える感度は、PhotographicSensitivity を 65535 にして、SensitivityType = 3
        // （ISO 感度）と ISOSpeed に実際の値を書く（Exif 2.32 の規定）。
        exif.push(Entry {
            tag: tag::PHOTOGRAPHIC_SENSITIVITY,
            value: Value::Short(vec![u16::try_from(iso).unwrap_or(u16::MAX)]),
        });
        if iso > u32::from(u16::MAX) {
            exif.push(Entry {
                tag: tag::SENSITIVITY_TYPE,
                value: Value::Short(vec![3]),
            });
            exif.push(Entry {
                tag: tag::ISO_SPEED,
                value: Value::Long(vec![iso]),
            });
        }
    }
    exif.push(Entry {
        tag: tag::EXIF_VERSION,
        value: Value::Undefined(EXIF_VERSION.to_vec()),
    });
    if let Some(dt) = data.datetime_original {
        exif.push(Entry {
            tag: tag::DATE_TIME_ORIGINAL,
            value: Value::Ascii(dt.format("%Y:%m:%d %H:%M:%S").to_string()),
        });
        if let Some(off) = data.offset_time_original {
            exif.push(Entry {
                tag: tag::OFFSET_TIME_ORIGINAL,
                value: Value::Ascii(format_offset(off)),
            });
        }
        let millis = dt.nanosecond() / 1_000_000;
        if millis > 0 {
            exif.push(Entry {
                tag: tag::SUB_SEC_TIME_ORIGINAL,
                value: Value::Ascii(format!("{millis:03}")),
            });
        }
    }
    if let Some(r) = data.focal_length_mm.and_then(|f| to_rational(f, 100)) {
        exif.push(Entry {
            tag: tag::FOCAL_LENGTH,
            value: Value::Rational(vec![r]),
        });
    }
    exif.push(Entry {
        tag: tag::COLOR_SPACE,
        value: Value::Short(vec![match info.color_space {
            OutputColorSpace::Srgb => 1,
            OutputColorSpace::DisplayP3 | OutputColorSpace::AdobeRgb => 0xFFFF,
        }]),
    });
    exif.push(Entry {
        tag: tag::PIXEL_X_DIMENSION,
        value: Value::Long(vec![info.width]),
    });
    exif.push(Entry {
        tag: tag::PIXEL_Y_DIMENSION,
        value: Value::Long(vec![info.height]),
    });
    if let Some(lens) = ascii(&data.lens_model) {
        exif.push(Entry {
            tag: tag::LENS_MODEL,
            value: Value::Ascii(lens),
        });
    }

    let gps = data.gps.filter(GpsCoord::is_valid).map(|g| {
        vec![
            Entry {
                tag: tag::GPS_VERSION_ID,
                value: Value::Byte(GPS_VERSION.to_vec()),
            },
            Entry {
                tag: tag::GPS_LATITUDE_REF,
                value: Value::Ascii(if g.lat < 0.0 { "S" } else { "N" }.to_owned()),
            },
            Entry {
                tag: tag::GPS_LATITUDE,
                value: Value::Rational(degrees_to_dms(g.lat).to_vec()),
            },
            Entry {
                tag: tag::GPS_LONGITUDE_REF,
                value: Value::Ascii(if g.lon < 0.0 { "W" } else { "E" }.to_owned()),
            },
            Entry {
                tag: tag::GPS_LONGITUDE,
                value: Value::Rational(degrees_to_dms(g.lon).to_vec()),
            },
        ]
    });

    for v in [&mut ifd0, &mut exif] {
        v.sort_by_key(|e| e.tag);
    }
    ExifIfds { ifd0, exif, gps }
}

/// IFD を書いたときの大きさ（エントリの数・エントリ・次の IFD へのオフセット・外に置く値）。
fn ifd_size(entries: &[Entry], order: ByteOrder) -> usize {
    let data: usize = entries
        .iter()
        .map(|e| {
            let n = e.value.bytes(order).len();
            if n > 4 { n + n % 2 } else { 0 }
        })
        .sum();
    2 + 12 * entries.len() + 4 + data
}

/// IFD を `out` に書く。`offset` は `out` の先頭が TIFF の先頭からどれだけ後ろか（ここでは 0）。
fn write_ifd(out: &mut Vec<u8>, entries: &[Entry], order: ByteOrder) -> Result<()> {
    let start = out.len();
    let count = u16::try_from(entries.len())
        .map_err(|_| MediaError::encode("Exif のエントリが多すぎる"))?;
    out.extend_from_slice(&order.u16(count));
    // 外に置く値の位置（IFD の直後）。
    let mut data_pos = start + 2 + 12 * entries.len() + 4;
    let mut data = Vec::new();
    for e in entries {
        let bytes = e.value.bytes(order);
        out.extend_from_slice(&order.u16(e.tag));
        out.extend_from_slice(&order.u16(e.value.type_code()));
        out.extend_from_slice(&order.u32(e.value.count()));
        if bytes.len() <= 4 {
            let mut inline = [0u8; 4];
            inline[..bytes.len()].copy_from_slice(&bytes);
            out.extend_from_slice(&inline);
        } else {
            let pos =
                u32::try_from(data_pos).map_err(|_| MediaError::encode("Exif が大きすぎる"))?;
            out.extend_from_slice(&order.u32(pos));
            data_pos += bytes.len() + bytes.len() % 2;
            data.extend_from_slice(&bytes);
            if bytes.len() % 2 == 1 {
                // 値はワード（2 バイト）境界から始める。
                data.push(0);
            }
        }
    }
    // 次の IFD はない（サムネイルの 1st IFD は書かない）。
    out.extend_from_slice(&order.u32(0));
    out.extend_from_slice(&data);
    Ok(())
}

/// Exif の TIFF の構造（JPEG の APP1 の "Exif\0\0" の後ろ、PNG の eXIf チャンクの中身）を作る
/// （ビッグエンディアン）。
///
/// `width`・`height` は書き出す画像の寸法（PixelXDimension・PixelYDimension）、`color_space` は
/// 出力の色空間（ColorSpace タグ）。GPS は `data.gps` があれば書く（設定の `remove_gps` は呼び出し側で
/// [`ExifData::without_gps`] として適用する）。
pub fn exif_tiff_bytes(
    data: &ExifData,
    width: u32,
    height: u32,
    color_space: OutputColorSpace,
) -> Result<Vec<u8>> {
    encode_exif_tiff(
        &build_ifds(
            data,
            &ExifImageInfo {
                width,
                height,
                color_space,
            },
        ),
        ByteOrder::Big,
    )
}

/// Exif の TIFF の構造（JPEG の APP1 の "Exif\0\0" の後ろ、PNG の eXIf チャンクの中身）を作る。
pub(crate) fn encode_exif_tiff(ifds: &ExifIfds, order: ByteOrder) -> Result<Vec<u8>> {
    // ポインタのエントリの値（4 バイト）はエントリの中に収まるので、IFD の大きさは値に依らない。
    let mut ifd0 = ifds.ifd0.clone();
    ifd0.push(Entry {
        tag: tag::EXIF_IFD,
        value: Value::Long(vec![0]),
    });
    if ifds.gps.is_some() {
        ifd0.push(Entry {
            tag: tag::GPS_IFD,
            value: Value::Long(vec![0]),
        });
    }
    ifd0.sort_by_key(|e| e.tag);
    let ifd0_offset = 8usize;
    let exif_offset = ifd0_offset + ifd_size(&ifd0, order);
    let gps_offset = exif_offset + ifd_size(&ifds.exif, order);
    let as_u32 = |v: usize| u32::try_from(v).map_err(|_| MediaError::encode("Exif が大きすぎる"));
    for e in &mut ifd0 {
        match e.tag {
            tag::EXIF_IFD => e.value = Value::Long(vec![as_u32(exif_offset)?]),
            tag::GPS_IFD => e.value = Value::Long(vec![as_u32(gps_offset)?]),
            _ => {}
        }
    }

    let mut out = Vec::with_capacity(512);
    match order {
        ByteOrder::Big => out.extend_from_slice(b"MM"),
        ByteOrder::Little => out.extend_from_slice(b"II"),
    }
    out.extend_from_slice(&order.u16(42));
    out.extend_from_slice(&order.u32(as_u32(ifd0_offset)?));
    write_ifd(&mut out, &ifd0, order)?;
    debug_assert_eq!(out.len(), exif_offset);
    // write_ifd が書くオフセットは out の先頭（= TIFF の先頭）からの位置。
    write_ifd(&mut out, &ifds.exif, order)?;
    debug_assert_eq!(out.len(), gps_offset);
    if let Some(gps) = &ifds.gps {
        write_ifd(&mut out, gps, order)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use genzo_model::CaptureInfo;

    fn sample_meta() -> PhotoMetadata {
        PhotoMetadata {
            make: Some("SONY".to_owned()),
            model: Some("ILCE-7M4".to_owned()),
            lens: Some("FE 24-70mm F2.8 GM II".to_owned()),
            iso: Some(800),
            aperture: Some(2.8),
            shutter_s: Some(1.0 / 250.0),
            focal_mm: Some(35.0),
            gps: GpsCoord::new(35.681_236, 139.767_125),
            capture: CaptureInfo {
                datetime: Some("2024:05:01 12:34:56.789".to_owned()),
                offset: Some("+09:00".to_owned()),
            },
            ..Default::default()
        }
    }

    fn info() -> ExifImageInfo {
        ExifImageInfo {
            width: 640,
            height: 480,
            color_space: OutputColorSpace::Srgb,
        }
    }

    fn parse(tiff: Vec<u8>) -> exif::Exif {
        exif::Reader::new().read_raw(tiff).unwrap()
    }

    fn ascii(e: &exif::Exif, t: exif::Tag) -> Option<String> {
        match &e.get_field(t, exif::In::PRIMARY)?.value {
            exif::Value::Ascii(v) => Some(String::from_utf8_lossy(&v[0]).into_owned()),
            _ => None,
        }
    }

    fn rational(e: &exif::Exif, t: exif::Tag) -> Vec<(u32, u32)> {
        match &e.get_field(t, exif::In::PRIMARY).unwrap().value {
            exif::Value::Rational(v) => v.iter().map(|r| (r.num, r.denom)).collect(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn written_tiff_is_parsed_by_kamadak_exif_in_both_byte_orders() {
        let data = ExifData::from_photo_metadata(&sample_meta());
        for order in [ByteOrder::Big, ByteOrder::Little] {
            let tiff = encode_exif_tiff(&build_ifds(&data, &info()), order).unwrap();
            let e = parse(tiff);
            assert_eq!(e.little_endian(), order == ByteOrder::Little);
            assert_eq!(ascii(&e, exif::Tag::Make).as_deref(), Some("SONY"));
            assert_eq!(ascii(&e, exif::Tag::Model).as_deref(), Some("ILCE-7M4"));
            assert_eq!(
                ascii(&e, exif::Tag::Software).as_deref(),
                Some(EXIF_SOFTWARE)
            );
            assert_eq!(
                ascii(&e, exif::Tag::LensModel).as_deref(),
                Some("FE 24-70mm F2.8 GM II")
            );
            assert_eq!(
                ascii(&e, exif::Tag::DateTimeOriginal).as_deref(),
                Some("2024:05:01 12:34:56")
            );
            assert_eq!(
                ascii(&e, exif::Tag::OffsetTimeOriginal).as_deref(),
                Some("+09:00")
            );
            assert_eq!(
                ascii(&e, exif::Tag::SubSecTimeOriginal).as_deref(),
                Some("789")
            );
            let get_uint = |t| {
                e.get_field(t, exif::In::PRIMARY)
                    .unwrap()
                    .value
                    .get_uint(0)
                    .unwrap()
            };
            assert_eq!(get_uint(exif::Tag::Orientation), 1);
            assert_eq!(get_uint(exif::Tag::PhotographicSensitivity), 800);
            assert_eq!(get_uint(exif::Tag::ColorSpace), 1);
            assert_eq!(get_uint(exif::Tag::PixelXDimension), 640);
            assert_eq!(get_uint(exif::Tag::PixelYDimension), 480);
            assert_eq!(rational(&e, exif::Tag::ExposureTime), vec![(1, 250)]);
            assert_eq!(rational(&e, exif::Tag::FNumber), vec![(14, 5)]);
            assert_eq!(rational(&e, exif::Tag::FocalLength), vec![(35, 1)]);
            assert_eq!(rational(&e, exif::Tag::XResolution), vec![(72, 1)]);
            match &e
                .get_field(exif::Tag::ExifVersion, exif::In::PRIMARY)
                .unwrap()
                .value
            {
                exif::Value::Undefined(v, _) => assert_eq!(v, b"0232"),
                other => panic!("{other:?}"),
            }
            // GPS の IFD の版は、ExifVersion（0232）に合わせて 2.3.0.0。
            match &e
                .get_field(exif::Tag::GPSVersionID, exif::In::PRIMARY)
                .unwrap()
                .value
            {
                exif::Value::Byte(v) => assert_eq!(v.as_slice(), &[2, 3, 0, 0]),
                other => panic!("{other:?}"),
            }
            // GPS: 35.681236° = 35° 40' 52.4496"、139.767125° = 139° 46' 1.65"。
            assert_eq!(ascii(&e, exif::Tag::GPSLatitudeRef).as_deref(), Some("N"));
            assert_eq!(
                rational(&e, exif::Tag::GPSLatitude),
                vec![(35, 1), (40, 1), (524_496, 10_000)]
            );
            assert_eq!(ascii(&e, exif::Tag::GPSLongitudeRef).as_deref(), Some("E"));
            assert_eq!(
                rational(&e, exif::Tag::GPSLongitude),
                vec![(139, 1), (46, 1), (16_500, 10_000)]
            );
        }
    }

    #[test]
    fn tag_numbers_match_an_independent_table() {
        // 自前のタグの番号を、kamadak-exif のタグの表（Exif 2.3x の番号）と照合する。
        use exif::{Context, Tag};
        let table: [(u16, Tag); 25] = [
            (tag::MAKE, Tag::Make),
            (tag::MODEL, Tag::Model),
            (tag::ORIENTATION, Tag::Orientation),
            (tag::X_RESOLUTION, Tag::XResolution),
            (tag::Y_RESOLUTION, Tag::YResolution),
            (tag::RESOLUTION_UNIT, Tag::ResolutionUnit),
            (tag::SOFTWARE, Tag::Software),
            (tag::EXIF_IFD, Tag::ExifIFDPointer),
            (tag::GPS_IFD, Tag::GPSInfoIFDPointer),
            (tag::EXPOSURE_TIME, Tag::ExposureTime),
            (tag::F_NUMBER, Tag::FNumber),
            (tag::PHOTOGRAPHIC_SENSITIVITY, Tag::PhotographicSensitivity),
            (tag::SENSITIVITY_TYPE, Tag::SensitivityType),
            (tag::ISO_SPEED, Tag::ISOSpeed),
            (tag::EXIF_VERSION, Tag::ExifVersion),
            (tag::DATE_TIME_ORIGINAL, Tag::DateTimeOriginal),
            (tag::OFFSET_TIME_ORIGINAL, Tag::OffsetTimeOriginal),
            (tag::FOCAL_LENGTH, Tag::FocalLength),
            (tag::SUB_SEC_TIME_ORIGINAL, Tag::SubSecTimeOriginal),
            (tag::COLOR_SPACE, Tag::ColorSpace),
            (tag::PIXEL_X_DIMENSION, Tag::PixelXDimension),
            (tag::PIXEL_Y_DIMENSION, Tag::PixelYDimension),
            (tag::LENS_MODEL, Tag::LensModel),
            (tag::GPS_VERSION_ID, Tag::GPSVersionID),
            (tag::GPS_LATITUDE, Tag::GPSLatitude),
        ];
        for (ours, theirs) in table {
            assert_eq!(ours, theirs.number(), "{theirs}");
        }
        for (ours, theirs) in [
            (tag::GPS_LATITUDE_REF, Tag::GPSLatitudeRef),
            (tag::GPS_LONGITUDE_REF, Tag::GPSLongitudeRef),
            (tag::GPS_LONGITUDE, Tag::GPSLongitude),
        ] {
            assert_eq!(ours, theirs.number(), "{theirs}");
            assert_eq!(theirs.context(), Context::Gps);
        }
        // 書く IFD も表と一致する（0th は Tiff、撮影情報は Exif、位置は GPS）。
        assert_eq!(Tag::Software.context(), Context::Tiff);
        assert_eq!(Tag::OffsetTimeOriginal.context(), Context::Exif);
        assert_eq!(Tag::LensModel.context(), Context::Exif);
    }

    #[test]
    fn gps_is_omitted_when_removed() {
        let data = ExifData::from_photo_metadata(&sample_meta()).without_gps();
        let ifds = build_ifds(&data, &info());
        assert!(ifds.gps.is_none());
        let e = parse(encode_exif_tiff(&ifds, ByteOrder::Big).unwrap());
        assert!(e.fields().all(|f| f.tag.context() != exif::Context::Gps));
        assert!(
            e.get_field(exif::Tag::GPSInfoIFDPointer, exif::In::PRIMARY)
                .is_none()
        );
        // GPS 以外は残る。
        assert_eq!(ascii(&e, exif::Tag::Make).as_deref(), Some("SONY"));
    }

    #[test]
    fn southern_and_western_hemispheres() {
        let data = ExifData {
            gps: GpsCoord::new(-33.856_784, -70.0),
            ..Default::default()
        };
        let e = parse(encode_exif_tiff(&build_ifds(&data, &info()), ByteOrder::Little).unwrap());
        assert_eq!(ascii(&e, exif::Tag::GPSLatitudeRef).as_deref(), Some("S"));
        assert_eq!(ascii(&e, exif::Tag::GPSLongitudeRef).as_deref(), Some("W"));
        // 33.856784° = 33° 51' 24.4224"。
        assert_eq!(
            rational(&e, exif::Tag::GPSLatitude),
            vec![(33, 1), (51, 1), (244_224, 10_000)]
        );
        assert_eq!(
            rational(&e, exif::Tag::GPSLongitude),
            vec![(70, 1), (0, 1), (0, 10_000)]
        );
    }

    #[test]
    fn minimal_data_has_only_fixed_tags() {
        let e = parse(
            encode_exif_tiff(&build_ifds(&ExifData::default(), &info()), ByteOrder::Big).unwrap(),
        );
        assert_eq!(ascii(&e, exif::Tag::Make), None);
        assert_eq!(ascii(&e, exif::Tag::DateTimeOriginal), None);
        assert_eq!(
            ascii(&e, exif::Tag::Software).as_deref(),
            Some(EXIF_SOFTWARE)
        );
        assert!(
            e.get_field(exif::Tag::ExposureTime, exif::In::PRIMARY)
                .is_none()
        );
    }

    #[test]
    fn rational_conversions() {
        assert_eq!(exposure_rational(1.0 / 250.0), Some((1, 250)));
        assert_eq!(
            exposure_rational(f64::from(1.0_f32 / 8000.0)),
            Some((1, 8000))
        );
        assert_eq!(exposure_rational(1.0 / 3.0), Some((1, 3)));
        assert_eq!(exposure_rational(0.3), Some((3, 10)));
        assert_eq!(exposure_rational(1.0), Some((1, 1)));
        assert_eq!(exposure_rational(2.5), Some((5, 2)));
        assert_eq!(exposure_rational(30.0), Some((30, 1)));
        assert_eq!(exposure_rational(0.0), None);
        assert_eq!(exposure_rational(-1.0), None);
        assert_eq!(exposure_rational(f64::NAN), None);
        assert_eq!(to_rational(2.8, 100), Some((14, 5)));
        assert_eq!(to_rational(f64::from(2.8_f32), 100), Some((14, 5)));
        assert_eq!(to_rational(1.4, 100), Some((7, 5)));
        assert_eq!(to_rational(0.001, 100), None);
        assert_eq!(to_rational(1e12, 100), None);
    }

    #[test]
    fn dms_conversion_carries_correctly() {
        // 59.99999 秒は 0.0001 秒単位で 60 秒に繰り上がる。
        let d = degrees_to_dms(10.0 + 59.0 / 60.0 + 59.999_99 / 3600.0);
        assert_eq!(d, [(11, 1), (0, 1), (0, 10_000)]);
        assert_eq!(degrees_to_dms(0.0), [(0, 1), (0, 1), (0, 10_000)]);
        assert_eq!(degrees_to_dms(-180.0), [(180, 1), (0, 1), (0, 10_000)]);
    }

    #[test]
    fn ascii_sanitization() {
        assert_eq!(sanitize_ascii("  SONY\0\0"), Some("SONY".to_owned()));
        assert_eq!(
            sanitize_ascii("Ｌｅｎｓ 50mm"),
            Some("???? 50mm".to_owned())
        );
        assert_eq!(sanitize_ascii("a\nb"), Some("a?b".to_owned()));
        assert_eq!(sanitize_ascii("   "), None);
        let long = "x".repeat(1000);
        assert_eq!(sanitize_ascii(&long).unwrap().len(), EXIF_MAX_ASCII_CHARS);
    }

    #[test]
    fn iso_above_65535_uses_iso_speed() {
        let data = ExifData {
            iso: Some(102_400),
            ..Default::default()
        };
        let e = parse(encode_exif_tiff(&build_ifds(&data, &info()), ByteOrder::Big).unwrap());
        let get = |t| {
            e.get_field(t, exif::In::PRIMARY)
                .unwrap()
                .value
                .get_uint(0)
                .unwrap()
        };
        assert_eq!(get(exif::Tag::PhotographicSensitivity), 65535);
        assert_eq!(get(exif::Tag::SensitivityType), 3);
        assert_eq!(get(exif::Tag::ISOSpeed), 102_400);
    }

    #[test]
    fn color_space_tag_follows_output_space() {
        for (space, expected) in [
            (OutputColorSpace::Srgb, 1),
            (OutputColorSpace::DisplayP3, 0xFFFF),
            (OutputColorSpace::AdobeRgb, 0xFFFF),
        ] {
            let i = ExifImageInfo {
                color_space: space,
                ..info()
            };
            let e = parse(
                encode_exif_tiff(&build_ifds(&ExifData::default(), &i), ByteOrder::Big).unwrap(),
            );
            let v = e
                .get_field(exif::Tag::ColorSpace, exif::In::PRIMARY)
                .unwrap()
                .value
                .get_uint(0);
            assert_eq!(v, Some(expected), "{space:?}");
        }
    }

    #[test]
    fn capture_time_override() {
        let meta = sample_meta();
        // 時計のずれを +60 秒補正した撮影日時（オフセットはファイルの値）。
        let ct = CaptureTime::from_capture_info(&meta.capture, FixedOffset::east_opt(0).unwrap())
            .with_correction(60)
            .unwrap();
        let data = ExifData::from_photo_metadata(&meta).with_capture_time(&ct);
        let expected = NaiveDate::from_ymd_opt(2024, 5, 1)
            .unwrap()
            .and_hms_milli_opt(12, 35, 56, 789)
            .unwrap();
        assert_eq!(data.datetime_original, Some(expected));
        assert_eq!(data.offset_time_original, FixedOffset::east_opt(9 * 3600));
        // 既定のタイムゾーンで推定しただけなら、オフセットは書かない。
        let no_offset = CaptureInfo {
            datetime: Some("2024:05:01 12:34:56".to_owned()),
            offset: None,
        };
        let ct = CaptureTime::from_capture_info(&no_offset, FixedOffset::east_opt(3600).unwrap());
        assert_eq!(ct.tz_source, TzSource::UserDefault);
        let data = ExifData::default().with_capture_time(&ct);
        assert!(data.datetime_original.is_some());
        assert_eq!(data.offset_time_original, None);
        // 日時が分からなければ書かない。
        let data = ExifData::from_photo_metadata(&meta).with_capture_time(&CaptureTime::unknown());
        assert_eq!(data.datetime_original, None);
    }

    #[test]
    fn from_metadata_handles_iso8601_and_bad_strings() {
        let mut meta = sample_meta();
        meta.capture.datetime = Some("2024-05-01T12:34:56-05:00".to_owned());
        meta.capture.offset = Some("+09:00".to_owned());
        let d = ExifData::from_photo_metadata(&meta);
        // 文字列に含まれるオフセットを優先する。
        assert_eq!(d.offset_time_original, FixedOffset::west_opt(5 * 3600));
        meta.capture.datetime = Some("garbage".to_owned());
        let d = ExifData::from_photo_metadata(&meta);
        assert_eq!(d.datetime_original, None);
        assert_eq!(d.offset_time_original, None);
    }

    #[test]
    fn odd_length_values_are_word_aligned() {
        // 5 文字 + NUL = 6、4 文字 + NUL = 5（奇数）。
        let data = ExifData {
            make: Some("ABCD".to_owned()),
            model: Some("ABCDEFG".to_owned()),
            ..Default::default()
        };
        let tiff = encode_exif_tiff(&build_ifds(&data, &info()), ByteOrder::Big).unwrap();
        // すべての外に置いた値のオフセットが偶数。
        let e = parse(tiff.clone());
        assert_eq!(ascii(&e, exif::Tag::Model).as_deref(), Some("ABCDEFG"));
        let count = u16::from_be_bytes([tiff[8], tiff[9]]) as usize;
        for i in 0..count {
            let p = 10 + 12 * i;
            let n = u32::from_be_bytes(tiff[p + 4..p + 8].try_into().unwrap());
            let ty = u16::from_be_bytes([tiff[p + 2], tiff[p + 3]]);
            let size = match ty {
                1 | 2 | 7 => 1,
                3 => 2,
                4 => 4,
                5 => 8,
                _ => unreachable!(),
            } * n;
            if size > 4 {
                let off = u32::from_be_bytes(tiff[p + 8..p + 12].try_into().unwrap());
                assert_eq!(off % 2, 0, "エントリ {i}");
            }
        }
    }
}
