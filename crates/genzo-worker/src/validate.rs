//! 本体が、ワーカーから受け取ったバッファを検証して組み立てる（docs/04_architecture.md の 1.2 節
//! 「本体は、受け取ったバッファの寸法・長さ・型・上限（画素数の上限など）を検証してから使います」）。
//!
//! 共有メモリのヘッダの形式・種類・共有メモリに収まることは [`crate::shm::ShmBuffer::open_payload`] で
//! 確かめてある。ここでは種類ごとに、寸法（0 でない・上限以下）、データの長さと寸法の一致、
//! ヘッダと付随情報（JSON）の一致を確かめてから、データを本体のメモリ（使う型の `Vec`）に直接
//! 読む（複製は 1 回）。RAW は最後に `RawImage::validate` で黒レベル・白レベル・WB・行列も確かめる。

use genzo_media::MAX_IMAGE_PIXELS;
use genzo_model::PhotoMetadata;
use genzo_raw::{MAX_PIXELS, RawImage};

use crate::protocol::{LinearImageInfo, RawFrameInfo, ThumbnailInfo};
use crate::shm::{BufferError, PayloadKind, ReceivedPayload};

/// 受け取るサムネイルの JPEG の大きさの上限（バイト）。
///
/// **仮置き**: 64 MiB。長辺 [`MAX_THUMBNAIL_EDGE`] の品質 100 の JPEG（数十 MB）より大きい値。
pub const MAX_THUMBNAIL_JPEG_BYTES: u64 = 64 * 1024 * 1024;

/// サムネイルの長辺の上限（画素）。
///
/// **仮置き**: 8192。キャッシュの L1（長辺 2560px。04 の 4 章）より十分大きい値。
pub const MAX_THUMBNAIL_EDGE: u32 = 8192;

/// 撮影情報から、有限でない値・範囲外の値を除く。
///
/// ワーカーは応答の前に（JSON にできない NaN を入れないため）、本体は受け取った後に（乗っ取られた
/// ワーカーが範囲外の GPS などを送っても、カタログに入れないため）呼ぶ。f32 の範囲を超える数は
/// serde_json が受け付けない。
pub(crate) fn sanitize_metadata(m: &mut PhotoMetadata) {
    for v in [&mut m.aperture, &mut m.shutter_s, &mut m.focal_mm] {
        if v.is_some_and(|x| !x.is_finite()) {
            *v = None;
        }
    }
    if m.gps.is_some_and(|g| !g.is_valid()) {
        m.gps = None;
    }
}

/// 寸法を確かめて画素数を返す。
fn checked_pixels(width: u32, height: u32, max: u64) -> Result<u64, BufferError> {
    if width == 0 || height == 0 {
        return Err(BufferError::InvalidDimensions { width, height });
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > max {
        return Err(BufferError::TooManyPixels { pixels, max });
    }
    Ok(pixels)
}

fn same_dimensions(payload: &ReceivedPayload<'_>, info: (u32, u32)) -> Result<(), BufferError> {
    let h = payload.header();
    if (h.width, h.height) != info {
        return Err(BufferError::DimensionMismatch {
            header: (h.width, h.height),
            info,
        });
    }
    Ok(())
}

fn expect_len(payload: &ReceivedPayload<'_>, expected: u64) -> Result<(), BufferError> {
    let actual = payload.header().data_len;
    if actual != expected {
        return Err(BufferError::LengthMismatch { expected, actual });
    }
    Ok(())
}

/// CFA（u16）と付随情報から [`RawImage`] を組み立てて検証する。
pub(crate) fn raw_image(
    payload: &ReceivedPayload<'_>,
    info: RawFrameInfo,
) -> Result<RawImage, BufferError> {
    let h = *payload.header();
    if h.kind != PayloadKind::CfaU16 {
        return Err(BufferError::UnexpectedKind {
            expected: PayloadKind::CfaU16,
            actual: h.kind,
        });
    }
    // 画素数の上限を、データの長さより先に確かめる（巨大な確保をしない）。
    checked_pixels(h.width, h.height, MAX_PIXELS)?;
    same_dimensions(payload, (info.width, info.height))?;
    let expected = PayloadKind::CfaU16.data_len_for(h.width, h.height).ok_or(
        BufferError::InvalidDimensions {
            width: h.width,
            height: h.height,
        },
    )?;
    expect_len(payload, expected)?;
    let data = read_u16_le(payload, expected)?;
    let mut metadata = info.metadata;
    sanitize_metadata(&mut metadata);
    let image = RawImage {
        width: info.width,
        height: info.height,
        cfa: info.cfa,
        data,
        black_level: info.black_level,
        white_level: info.white_level,
        as_shot_wb: info.as_shot_wb,
        cam_xyz: info.cam_xyz,
        metadata,
    };
    image
        .validate()
        .map_err(|e| BufferError::Invalid(e.to_string()))?;
    Ok(image)
}

/// リニアの RGB（f32）を複製する。NaN・無限大は 0 に置き換え（04 の 2.6 節）、置き換えた数を返す。
/// `info` の撮影情報も整える（[`sanitize_metadata`]）。
pub(crate) fn linear_pixels(
    payload: &ReceivedPayload<'_>,
    info: &mut LinearImageInfo,
) -> Result<(Vec<[f32; 3]>, u64), BufferError> {
    let h = *payload.header();
    if h.kind != PayloadKind::RgbF32 {
        return Err(BufferError::UnexpectedKind {
            expected: PayloadKind::RgbF32,
            actual: h.kind,
        });
    }
    checked_pixels(h.width, h.height, MAX_IMAGE_PIXELS)?;
    same_dimensions(payload, (info.width, info.height))?;
    let expected = PayloadKind::RgbF32.data_len_for(h.width, h.height).ok_or(
        BufferError::InvalidDimensions {
            width: h.width,
            height: h.height,
        },
    )?;
    expect_len(payload, expected)?;
    if !matches!(info.source_bits, 8 | 16) {
        return Err(BufferError::Invalid(format!(
            "元のビット数 {} が 8・16 のどちらでもない",
            info.source_bits
        )));
    }
    let pixels = read_rgb_f32_le(payload, expected)?;
    sanitize_metadata(&mut info.metadata);
    Ok(pixels)
}

/// JPEG のバイト列を複製する。
///
/// JPEG の中身はデコードしない（信頼できない入力のデコードを本体で行わないため。SEC-05）。
/// 先頭の印（SOI と次のマーカーの 0xFF）、大きさの上限、寸法（長辺が `max_edge` 以下）、
/// ヘッダと付随情報の一致だけを確かめる。
pub(crate) fn thumbnail_jpeg(
    payload: &ReceivedPayload<'_>,
    info: &ThumbnailInfo,
    max_edge: u32,
) -> Result<Vec<u8>, BufferError> {
    let h = *payload.header();
    if h.kind != PayloadKind::Jpeg {
        return Err(BufferError::UnexpectedKind {
            expected: PayloadKind::Jpeg,
            actual: h.kind,
        });
    }
    let pixels_max = u64::from(max_edge) * u64::from(max_edge);
    checked_pixels(h.width, h.height, pixels_max)?;
    if h.width.max(h.height) > max_edge {
        return Err(BufferError::Invalid(format!(
            "サムネイルの寸法 {} × {} が長辺 {max_edge} を超える",
            h.width, h.height
        )));
    }
    same_dimensions(payload, (info.width, info.height))?;
    if h.data_len > MAX_THUMBNAIL_JPEG_BYTES {
        return Err(BufferError::DataTooLarge {
            len: h.data_len,
            max: MAX_THUMBNAIL_JPEG_BYTES,
        });
    }
    expect_len(payload, info.byte_len)?;
    let data = payload.read_to_vec()?;
    if !data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Err(BufferError::Invalid(
            "JPEG の先頭の印（SOI）がない".to_owned(),
        ));
    }
    Ok(data)
}

/// データの長さ `len`（バイト。寸法との一致は確かめ済み）を `usize` にする。
fn len_usize(len: u64) -> Result<usize, BufferError> {
    usize::try_from(len).map_err(|_| BufferError::DataTooLarge {
        len,
        max: usize::MAX as u64,
    })
}

/// リトルエンディアンの u16 の列（`len` バイト）を、本体の `Vec<u16>` に直接読む。
fn read_u16_le(payload: &ReceivedPayload<'_>, len: u64) -> Result<Vec<u16>, BufferError> {
    let mut data = vec![0u16; len_usize(len)? / 2];
    payload.read_into(bytemuck::cast_slice_mut(&mut data))?;
    for v in &mut data {
        *v = u16::from_le(*v);
    }
    Ok(data)
}

/// リトルエンディアンの f32 の RGB の列（`len` バイト）を本体の `Vec` に直接読み、有限でない値を
/// 0 にする。置き換えた数も返す。
fn read_rgb_f32_le(
    payload: &ReceivedPayload<'_>,
    len: u64,
) -> Result<(Vec<[f32; 3]>, u64), BufferError> {
    let mut pixels = vec![[0.0f32; 3]; len_usize(len)? / 12];
    payload.read_into(bytemuck::cast_slice_mut(&mut pixels))?;
    let mut replaced = 0u64;
    for v in pixels.iter_mut().flatten() {
        let x = f32::from_bits(u32::from_le(v.to_bits()));
        *v = if x.is_finite() {
            x
        } else {
            replaced += 1;
            0.0
        };
    }
    Ok((pixels, replaced))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{PhotoFormat, ProfileSummary, ThumbnailSource};
    use crate::shm::{SHM_HEADER_LEN, ShmArena, ShmBuffer, ShmHeader, ShmWriter};
    use genzo_model::PhotoMetadata;
    use genzo_raw::CfaPattern;

    struct Fixture {
        _root: tempfile::TempDir,
        arena: ShmArena,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let arena = ShmArena::new_in(root.path()).unwrap();
            Self { _root: root, arena }
        }

        /// ヘッダ（データの長さは `data` の長さ）とデータを書いた共有メモリ。
        fn buffer(&self, kind: PayloadKind, w: u32, h: u32, data: &[u8]) -> ShmBuffer {
            let buf = self
                .arena
                .allocate((SHM_HEADER_LEN + data.len()) as u64)
                .unwrap();
            let mut writer = ShmWriter::open(&buf.shm_ref(false)).unwrap();
            writer.data_mut(data.len()).unwrap().copy_from_slice(data);
            writer
                .finish(
                    ShmHeader {
                        kind,
                        width: w,
                        height: h,
                        data_len: data.len() as u64,
                        checksum: None,
                    },
                    false,
                )
                .unwrap();
            buf
        }
    }

    fn raw_info(w: u32, h: u32) -> RawFrameInfo {
        RawFrameInfo {
            width: w,
            height: h,
            cfa: CfaPattern::RGGB,
            black_level: [512.0; 4],
            white_level: 16383.0,
            as_shot_wb: [2.0, 1.0, 1.5, 1.0],
            cam_xyz: None,
            metadata: PhotoMetadata::default(),
            decoder_id: Some("test".into()),
            cam_xyz_source: None,
        }
    }

    fn u16_bytes(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn raw_round_trip() {
        let f = Fixture::new();
        let values: Vec<u16> = (0..24).map(|i| 500 + i * 100).collect();
        let buf = f.buffer(PayloadKind::CfaU16, 6, 4, &u16_bytes(&values));
        let p = buf.open_payload(PayloadKind::CfaU16, false).unwrap();
        let img = raw_image(&p, raw_info(6, 4)).unwrap();
        assert_eq!(img.data, values);
        assert_eq!(img.cfa, CfaPattern::RGGB);
    }

    #[test]
    fn raw_checks_dimensions_length_and_levels() {
        let f = Fixture::new();
        let data = u16_bytes(&[600; 24]);
        let buf = f.buffer(PayloadKind::CfaU16, 6, 4, &data);
        let p = buf.open_payload(PayloadKind::CfaU16, false).unwrap();
        assert_eq!(
            raw_image(&p, raw_info(4, 6)).unwrap_err(),
            BufferError::DimensionMismatch {
                header: (6, 4),
                info: (4, 6)
            }
        );
        let mut bad = raw_info(6, 4);
        bad.white_level = 0.0;
        assert!(matches!(raw_image(&p, bad), Err(BufferError::Invalid(_))));

        let buf = f.buffer(PayloadKind::CfaU16, 6, 5, &data);
        let p = buf.open_payload(PayloadKind::CfaU16, false).unwrap();
        assert_eq!(
            raw_image(&p, raw_info(6, 5)).unwrap_err(),
            BufferError::LengthMismatch {
                expected: 60,
                actual: 48
            }
        );
        let buf = f.buffer(PayloadKind::CfaU16, 0, 4, &data);
        let p = buf.open_payload(PayloadKind::CfaU16, false).unwrap();
        assert!(matches!(
            raw_image(&p, raw_info(0, 4)),
            Err(BufferError::InvalidDimensions { .. })
        ));
        // 画素数の上限は、データの長さより先に確かめる。
        let buf = f.buffer(PayloadKind::CfaU16, 20_000, 10_001, &data);
        let p = buf.open_payload(PayloadKind::CfaU16, false).unwrap();
        assert_eq!(
            raw_image(&p, raw_info(20_000, 10_001)).unwrap_err(),
            BufferError::TooManyPixels {
                pixels: 200_020_000,
                max: MAX_PIXELS
            }
        );
        let buf = f.buffer(PayloadKind::Jpeg, 6, 4, &data);
        let p = buf.open_payload(PayloadKind::Jpeg, false).unwrap();
        assert!(matches!(
            raw_image(&p, raw_info(6, 4)),
            Err(BufferError::UnexpectedKind { .. })
        ));
    }

    /// 乗っ取られたワーカーは、範囲外の GPS（緯度 1000 度など）を送れる。本体でも除く
    /// （ワーカーでの除去だけに頼らない）。f32 の範囲外の数（`1e39`）は serde_json が拒む。
    #[test]
    fn forged_metadata_is_sanitized_by_the_host() {
        assert!(serde_json::from_str::<PhotoMetadata>(r#"{"aperture":1e39}"#).is_err());
        let forged: PhotoMetadata =
            serde_json::from_str(r#"{"aperture":2.8,"gps":{"lat":1000.0,"lon":0.0}}"#).unwrap();
        assert!(forged.gps.is_some());
        let f = Fixture::new();
        let buf = f.buffer(PayloadKind::CfaU16, 6, 4, &u16_bytes(&[600; 24]));
        let p = buf.open_payload(PayloadKind::CfaU16, false).unwrap();
        let mut info = raw_info(6, 4);
        info.metadata = forged.clone();
        let img = raw_image(&p, info).unwrap();
        assert_eq!(img.metadata.gps, None);
        assert_eq!(img.metadata.aperture, Some(2.8));

        let buf = f.buffer(PayloadKind::RgbF32, 1, 1, &[0u8; 12]);
        let p = buf.open_payload(PayloadKind::RgbF32, false).unwrap();
        let mut info = linear_info(1, 1);
        info.metadata = forged.clone();
        linear_pixels(&p, &mut info).unwrap();
        assert_eq!(info.metadata.gps, None);

        let mut m = forged;
        sanitize_metadata(&mut m);
        assert_eq!(m.gps, None);
    }

    fn linear_info(w: u32, h: u32) -> LinearImageInfo {
        LinearImageInfo {
            format: PhotoFormat::Jpeg,
            width: w,
            height: h,
            source_bits: 8,
            metadata: PhotoMetadata::default(),
            profile: ProfileSummary {
                embedded: false,
                description: None,
                assumed_srgb_reason: Some("なし".into()),
                assumed_adobe_rgb_reason: None,
            },
            alpha_dropped: false,
        }
    }

    #[test]
    fn linear_pixels_replace_non_finite_values() {
        let f = Fixture::new();
        let values = [0.5f32, -0.25, 2.0, f32::NAN, f32::INFINITY, 1.0];
        let data: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let buf = f.buffer(PayloadKind::RgbF32, 2, 1, &data);
        let p = buf.open_payload(PayloadKind::RgbF32, false).unwrap();
        let (px, replaced) = linear_pixels(&p, &mut linear_info(2, 1)).unwrap();
        assert_eq!(px, vec![[0.5, -0.25, 2.0], [0.0, 0.0, 1.0]]);
        assert_eq!(replaced, 2);
        let mut info = linear_info(2, 1);
        info.source_bits = 12;
        assert!(matches!(
            linear_pixels(&p, &mut info),
            Err(BufferError::Invalid(_))
        ));
        assert!(matches!(
            linear_pixels(&p, &mut linear_info(1, 2)),
            Err(BufferError::DimensionMismatch { .. })
        ));
        let buf = f.buffer(PayloadKind::RgbF32, 3, 1, &data);
        let p = buf.open_payload(PayloadKind::RgbF32, false).unwrap();
        assert!(matches!(
            linear_pixels(&p, &mut linear_info(3, 1)),
            Err(BufferError::LengthMismatch { .. })
        ));
    }

    fn thumb_info(w: u32, h: u32, len: u64) -> ThumbnailInfo {
        ThumbnailInfo {
            width: w,
            height: h,
            byte_len: len,
            source: ThumbnailSource::Image,
            video: None,
        }
    }

    #[test]
    fn thumbnail_checks() {
        let f = Fixture::new();
        let jpeg = b"\xFF\xD8\xFF\xE0fake-jpeg".to_vec();
        let n = jpeg.len() as u64;
        let buf = f.buffer(PayloadKind::Jpeg, 320, 200, &jpeg);
        let p = buf.open_payload(PayloadKind::Jpeg, false).unwrap();
        assert_eq!(
            thumbnail_jpeg(&p, &thumb_info(320, 200, n), 320).unwrap(),
            jpeg
        );
        // 長辺の上限を超える。
        assert!(matches!(
            thumbnail_jpeg(&p, &thumb_info(320, 200, n), 160),
            Err(BufferError::TooManyPixels { .. }) | Err(BufferError::Invalid(_))
        ));
        assert!(matches!(
            thumbnail_jpeg(&p, &thumb_info(320, 200, n + 1), 320),
            Err(BufferError::LengthMismatch { .. })
        ));
        assert!(matches!(
            thumbnail_jpeg(&p, &thumb_info(200, 320, n), 320),
            Err(BufferError::DimensionMismatch { .. })
        ));
        let buf = f.buffer(PayloadKind::Jpeg, 10, 10, b"not a jpeg");
        let p = buf.open_payload(PayloadKind::Jpeg, false).unwrap();
        assert!(matches!(
            thumbnail_jpeg(&p, &thumb_info(10, 10, 10), 320),
            Err(BufferError::Invalid(_))
        ));
        // 細長い寸法（長辺だけが上限を超える）。
        let buf = f.buffer(PayloadKind::Jpeg, 400, 1, &jpeg);
        let p = buf.open_payload(PayloadKind::Jpeg, false).unwrap();
        assert!(matches!(
            thumbnail_jpeg(&p, &thumb_info(400, 1, n), 320),
            Err(BufferError::Invalid(_))
        ));
    }
}
