//! JPEG の書き出し（EXP-01・IQ-06）と、ヘッダーのセグメントの操作。
//!
//! 画素の符号化は image crate のエンコーダーで行い、できた JPEG の SOI の直後に、
//!
//! 1. APP1 の Exif（`"Exif\0\0"` ＋ TIFF の構造。[`crate::exif_write`]）
//! 2. APP2 の ICC プロファイル（`"ICC_PROFILE\0"` ＋ 通し番号 ＋ 総数 ＋ 最大 65,519 バイトの断片。
//!    ICC.1（ICC の仕様書）の付録 B「Embedding ICC profiles」の JFIF の節。節の番号は版によって
//!    違いうるので、引用するときは原典で確認すること）
//!
//! を自前で差し込む。エンコーダーが書く JFIF の APP0 は取り除く（Exif の規定では APP1 を SOI の
//! 直後に置き、JFIF の規定では APP0 を SOI の直後に置くため、両方は満たせない。カメラの JPEG と
//! 同じく Exif を優先する。3 成分の JPEG は JFIF がなくても YCbCr として扱われる）。

use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;

use crate::buffer::RgbImage8;
use crate::error::{MediaError, Result};

/// JPEG の寸法の上限（SOF の幅・高さは 16bit）。
pub const JPEG_MAX_DIMENSION: u32 = 65_535;

/// APP2 の 1 つのセグメントに入る ICC プロファイルの最大のバイト数。
///
/// セグメントの長さの上限 65,535 から、長さの欄（2）・`"ICC_PROFILE\0"`（12）・通し番号（1）・
/// 総数（1）を引いた値。
pub const ICC_CHUNK_MAX_BYTES: usize = 65_535 - 2 - 12 - 1 - 1;

/// ICC プロファイルを分割できる最大の数（通し番号は 1 バイト、1〜255）。
pub const ICC_MAX_CHUNKS: usize = 255;

/// APP1 の Exif に入る TIFF の構造の最大のバイト数（65,535 − 長さの欄 2 − `"Exif\0\0"` 6）。
pub const EXIF_MAX_BYTES: usize = 65_535 - 2 - 6;

const ICC_SIGNATURE: &[u8; 12] = b"ICC_PROFILE\0";
const EXIF_SIGNATURE: &[u8; 6] = b"Exif\0\0";
const JFIF_SIGNATURE: &[u8; 5] = b"JFIF\0";

const SOI: u8 = 0xD8;
const SOS: u8 = 0xDA;
const EOI: u8 = 0xD9;
const APP0: u8 = 0xE0;
const APP1: u8 = 0xE1;
const APP2: u8 = 0xE2;

/// JPEG のヘッダーのセグメント（SOS の前まで）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpegSegment {
    /// マーカー（0xFF の次のバイト。例: APP1 = 0xE1）。
    pub marker: u8,
    /// 長さの欄の後ろのデータ。
    pub data: Vec<u8>,
}

/// 解析したヘッダーと、SOS から後ろ（SOS のマーカーを含む）の位置。
struct ParsedJpeg {
    segments: Vec<JpegSegment>,
    scan_start: usize,
}

fn parse_header(jpeg: &[u8]) -> Result<ParsedJpeg> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != SOI {
        return Err(MediaError::decode("JPEG の SOI がない"));
    }
    let mut pos = 2;
    let mut segments = Vec::new();
    loop {
        // 0xFF の詰め物を飛ばす。
        if pos >= jpeg.len() || jpeg[pos] != 0xFF {
            return Err(MediaError::decode("JPEG のマーカーが見つからない"));
        }
        while pos < jpeg.len() && jpeg[pos] == 0xFF {
            pos += 1;
        }
        let Some(&marker) = jpeg.get(pos) else {
            return Err(MediaError::decode("JPEG が途中で終わっている"));
        };
        let marker_pos = pos - 1;
        pos += 1;
        match marker {
            SOS => {
                return Ok(ParsedJpeg {
                    segments,
                    scan_start: marker_pos,
                });
            }
            EOI | SOI | 0x01 | 0xD0..=0xD7 => {
                return Err(MediaError::decode(format!(
                    "JPEG のヘッダーに予期しないマーカー 0x{marker:02X}"
                )));
            }
            _ => {}
        }
        let Some(len_bytes) = jpeg.get(pos..pos + 2) else {
            return Err(MediaError::decode("JPEG のセグメントの長さがない"));
        };
        let len = usize::from(u16::from_be_bytes([len_bytes[0], len_bytes[1]]));
        if len < 2 || pos + len > jpeg.len() {
            return Err(MediaError::decode("JPEG のセグメントの長さが不正"));
        }
        segments.push(JpegSegment {
            marker,
            data: jpeg[pos + 2..pos + len].to_vec(),
        });
        pos += len;
    }
}

/// JPEG のヘッダーのセグメント（SOI の後ろから SOS の前まで）を返す。
pub fn jpeg_header_segments(jpeg: &[u8]) -> Result<Vec<JpegSegment>> {
    Ok(parse_header(jpeg)?.segments)
}

/// APP2 に分割して入れた ICC プロファイルを組み立てる。なければ `None`。
///
/// 通し番号の重複・欠け・総数の不一致があれば `None`（壊れたものとして扱う）。
pub fn extract_jpeg_icc(jpeg: &[u8]) -> Result<Option<Vec<u8>>> {
    let segments = jpeg_header_segments(jpeg)?;
    let chunks: Vec<&[u8]> = segments
        .iter()
        .filter(|s| s.marker == APP2 && s.data.starts_with(ICC_SIGNATURE))
        .map(|s| &s.data[ICC_SIGNATURE.len()..])
        .collect();
    if chunks.is_empty() {
        return Ok(None);
    }
    let total = chunks.len();
    let mut ordered: Vec<Option<&[u8]>> = vec![None; total];
    for c in &chunks {
        if c.len() < 2 || usize::from(c[1]) != total || c[0] == 0 || usize::from(c[0]) > total {
            return Ok(None);
        }
        let slot = &mut ordered[usize::from(c[0]) - 1];
        if slot.is_some() {
            return Ok(None);
        }
        *slot = Some(&c[2..]);
    }
    Ok(Some(
        ordered.into_iter().flatten().flatten().copied().collect(),
    ))
}

/// ICC プロファイルを APP2 のセグメントに分割する。
fn icc_segments(icc: &[u8]) -> Result<Vec<Vec<u8>>> {
    if icc.is_empty() {
        return Err(MediaError::invalid_argument("ICC プロファイルが空"));
    }
    let count = icc.len().div_ceil(ICC_CHUNK_MAX_BYTES);
    if count > ICC_MAX_CHUNKS {
        return Err(MediaError::TooLarge {
            what: "JPEG に埋め込む ICC プロファイル",
            actual: icc.len() as u64,
            max: (ICC_CHUNK_MAX_BYTES * ICC_MAX_CHUNKS) as u64,
        });
    }
    Ok(icc
        .chunks(ICC_CHUNK_MAX_BYTES)
        .enumerate()
        .map(|(i, chunk)| {
            let mut d = Vec::with_capacity(ICC_SIGNATURE.len() + 2 + chunk.len());
            d.extend_from_slice(ICC_SIGNATURE);
            d.push((i + 1) as u8);
            d.push(count as u8);
            d.extend_from_slice(chunk);
            d
        })
        .collect())
}

fn push_segment(out: &mut Vec<u8>, marker: u8, data: &[u8]) {
    let len = u16::try_from(data.len() + 2).expect("呼び出し側で長さを確認済み");
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(data);
}

/// JPEG に Exif と ICC プロファイルを入れ直す。
///
/// 既存の JFIF（APP0）・Exif（APP1）・ICC プロファイル（APP2）のセグメントを取り除き、SOI の直後に
/// Exif（`exif_tiff` が `Some` のとき）、続いて ICC プロファイル（`icc` が `Some` のとき）を置く。
/// ほかのセグメント（量子化テーブルなど）と画像のデータはそのまま残す。
pub fn insert_jpeg_metadata(
    jpeg: &[u8],
    icc: Option<&[u8]>,
    exif_tiff: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let parsed = parse_header(jpeg)?;
    let icc_segs = icc.map(icc_segments).transpose()?;
    if let Some(exif) = exif_tiff
        && exif.len() > EXIF_MAX_BYTES
    {
        return Err(MediaError::TooLarge {
            what: "JPEG に埋め込む Exif",
            actual: exif.len() as u64,
            max: EXIF_MAX_BYTES as u64,
        });
    }
    let mut out = Vec::with_capacity(jpeg.len() + icc.map_or(0, <[u8]>::len) + 1024);
    out.extend_from_slice(&[0xFF, SOI]);
    if let Some(exif) = exif_tiff {
        let mut d = Vec::with_capacity(EXIF_SIGNATURE.len() + exif.len());
        d.extend_from_slice(EXIF_SIGNATURE);
        d.extend_from_slice(exif);
        push_segment(&mut out, APP1, &d);
    }
    for seg in icc_segs.iter().flatten() {
        push_segment(&mut out, APP2, seg);
    }
    for seg in &parsed.segments {
        let drop = (seg.marker == APP0 && seg.data.starts_with(JFIF_SIGNATURE))
            || (seg.marker == APP1 && seg.data.starts_with(EXIF_SIGNATURE))
            || (seg.marker == APP2 && seg.data.starts_with(ICC_SIGNATURE));
        if !drop {
            push_segment(&mut out, seg.marker, &seg.data);
        }
    }
    out.extend_from_slice(&jpeg[parsed.scan_start..]);
    Ok(out)
}

/// 8bit の RGB 画像を JPEG にする（品質 1〜100）。Exif と ICC プロファイルを入れる。
pub fn encode_jpeg(
    img: &RgbImage8,
    quality: u8,
    icc: Option<&[u8]>,
    exif_tiff: Option<&[u8]>,
) -> Result<Vec<u8>> {
    if !(1..=100).contains(&quality) {
        return Err(MediaError::invalid_argument(format!(
            "JPEG の品質は 1〜100（{quality}）"
        )));
    }
    let (w, h) = img.dimensions();
    if w > JPEG_MAX_DIMENSION || h > JPEG_MAX_DIMENSION {
        return Err(MediaError::TooLarge {
            what: "JPEG の幅・高さ",
            actual: u64::from(w.max(h)),
            max: u64::from(JPEG_MAX_DIMENSION),
        });
    }
    let mut raw = Vec::new();
    JpegEncoder::new_with_quality(&mut raw, quality)
        .encode(img.as_raw(), w, h, ExtendedColorType::Rgb8)
        .map_err(|e| MediaError::encode(format!("JPEG: {e}")))?;
    insert_jpeg_metadata(&raw, icc, exif_tiff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_jpeg() -> Vec<u8> {
        let img = RgbImage8::from_fn(16, 8, |x, y| [(x * 16) as u8, (y * 32) as u8, 128]).unwrap();
        let mut raw = Vec::new();
        JpegEncoder::new_with_quality(&mut raw, 90)
            .encode(img.as_raw(), 16, 8, ExtendedColorType::Rgb8)
            .unwrap();
        raw
    }

    #[test]
    fn chunk_size_matches_the_icc_specification() {
        // ICC.1 の付録 B（JFIF への埋め込み）: 1 つのマーカーに入るのは 65,519 バイトまで
        // （65,535 − 長さ 2 − "ICC_PROFILE\0" 12 − 通し番号と総数 2）。
        assert_eq!(ICC_CHUNK_MAX_BYTES, 65_519);
        assert_eq!(EXIF_MAX_BYTES, 65_527);
    }

    #[test]
    fn large_icc_is_split_and_reassembled() {
        // 3 つに分かれる大きさ（65,519 × 2 + 1,000）。
        let icc: Vec<u8> = (0..(ICC_CHUNK_MAX_BYTES * 2 + 1000))
            .map(|i| (i * 31 % 251) as u8)
            .collect();
        let exif = b"MM\0\x2a\0\0\0\x08\0\0\0\0\0\0".to_vec();
        let out = insert_jpeg_metadata(&tiny_jpeg(), Some(&icc), Some(&exif)).unwrap();
        let segs = jpeg_header_segments(&out).unwrap();
        // SOI の直後が Exif、続いて ICC が 3 つ（番号 1〜3、総数 3）。
        assert_eq!(segs[0].marker, APP1);
        assert!(segs[0].data.starts_with(EXIF_SIGNATURE));
        assert_eq!(&segs[0].data[6..], exif.as_slice());
        for i in 0..3 {
            let s = &segs[1 + i];
            assert_eq!(s.marker, APP2);
            assert!(s.data.starts_with(ICC_SIGNATURE));
            assert_eq!(s.data[12], (i + 1) as u8);
            assert_eq!(s.data[13], 3);
        }
        assert_eq!(segs[1].data.len(), 14 + ICC_CHUNK_MAX_BYTES);
        assert_eq!(segs[3].data.len(), 14 + 1000);
        // JFIF は取り除かれている。
        assert!(
            !segs
                .iter()
                .any(|s| s.marker == APP0 && s.data.starts_with(JFIF_SIGNATURE))
        );
        assert_eq!(extract_jpeg_icc(&out).unwrap(), Some(icc.clone()));
        // 独立したデコーダー（zune-jpeg、image crate 経由）でも同じ ICC が取り出せ、画像も読める。
        use image::ImageDecoder;
        let mut dec = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(&out)).unwrap();
        assert_eq!(dec.icc_profile().unwrap(), Some(icc));
        assert_eq!(dec.exif_metadata().unwrap(), Some(exif));
        assert_eq!(dec.dimensions(), (16, 8));
        image::load_from_memory(&out).unwrap();
    }

    #[test]
    fn reinsertion_replaces_old_metadata() {
        let once = insert_jpeg_metadata(&tiny_jpeg(), Some(&[1, 2, 3]), Some(b"MM")).unwrap();
        let twice = insert_jpeg_metadata(&once, Some(&[9; 10]), None).unwrap();
        let segs = jpeg_header_segments(&twice).unwrap();
        assert_eq!(
            segs.iter()
                .filter(|s| s.marker == APP2 && s.data.starts_with(ICC_SIGNATURE))
                .count(),
            1
        );
        assert!(
            !segs
                .iter()
                .any(|s| s.marker == APP1 && s.data.starts_with(EXIF_SIGNATURE))
        );
        assert_eq!(extract_jpeg_icc(&twice).unwrap(), Some(vec![9; 10]));
        // 画像のデータ（SOS 以降）は変わらない。
        let a = parse_header(&tiny_jpeg()).unwrap().scan_start;
        let b = parse_header(&twice).unwrap().scan_start;
        assert_eq!(tiny_jpeg()[a..], twice[b..]);
    }

    #[test]
    fn too_large_metadata_is_rejected() {
        let huge_icc = vec![0u8; ICC_CHUNK_MAX_BYTES * ICC_MAX_CHUNKS + 1];
        assert!(matches!(
            insert_jpeg_metadata(&tiny_jpeg(), Some(&huge_icc), None),
            Err(MediaError::TooLarge { .. })
        ));
        let huge_exif = vec![0u8; EXIF_MAX_BYTES + 1];
        assert!(matches!(
            insert_jpeg_metadata(&tiny_jpeg(), None, Some(&huge_exif)),
            Err(MediaError::TooLarge { .. })
        ));
        // ちょうど上限の Exif は入る。
        let max_exif = vec![0u8; EXIF_MAX_BYTES];
        insert_jpeg_metadata(&tiny_jpeg(), None, Some(&max_exif)).unwrap();
        assert!(insert_jpeg_metadata(&tiny_jpeg(), Some(&[]), None).is_err());
    }

    #[test]
    fn broken_jpeg_does_not_panic() {
        let good = tiny_jpeg();
        for cut in [0, 1, 2, 3, 4, 10, 20, good.len() / 2] {
            let _ = insert_jpeg_metadata(&good[..cut], None, None);
            let _ = extract_jpeg_icc(&good[..cut]);
        }
        assert!(insert_jpeg_metadata(b"not a jpeg", None, None).is_err());
        // セグメントの長さが範囲外。
        let mut bad = good.clone();
        bad[4] = 0xFF;
        bad[5] = 0xFF;
        let _ = insert_jpeg_metadata(&bad, None, None);
    }

    #[test]
    fn broken_icc_chunks_are_ignored() {
        let out = insert_jpeg_metadata(&tiny_jpeg(), Some(&[5; 20]), None).unwrap();
        // 総数を 2 に書き換える（実際は 1 つ）。
        let mut bad = out.clone();
        let pos = bad
            .windows(ICC_SIGNATURE.len())
            .position(|w| w == ICC_SIGNATURE)
            .unwrap();
        bad[pos + 13] = 2;
        assert_eq!(extract_jpeg_icc(&bad).unwrap(), None);
        // 通し番号 0。
        let mut bad = out;
        bad[pos + 12] = 0;
        assert_eq!(extract_jpeg_icc(&bad).unwrap(), None);
    }

    #[test]
    fn encode_validates_arguments() {
        let img = RgbImage8::new(4, 4).unwrap();
        assert!(encode_jpeg(&img, 0, None, None).is_err());
        assert!(encode_jpeg(&img, 101, None, None).is_err());
        let ok = encode_jpeg(&img, 100, None, None).unwrap();
        assert!(
            jpeg_header_segments(&ok)
                .unwrap()
                .iter()
                .all(|s| s.marker != APP0)
        );
        let wide = RgbImage8::new(JPEG_MAX_DIMENSION + 1, 1).unwrap();
        assert!(matches!(
            encode_jpeg(&wide, 90, None, None),
            Err(MediaError::TooLarge { .. })
        ));
    }
}
