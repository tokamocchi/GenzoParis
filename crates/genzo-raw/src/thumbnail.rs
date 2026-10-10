//! 埋め込みサムネイル（取り込み時のサムネイル。04 の 1.2 節のバッチ用ワーカーの
//! 「埋め込みサムネイル抽出」）。

use serde::{Deserialize, Serialize};

use crate::RawError;

/// 受け取る埋め込みサムネイルの大きさの上限（バイト）。
///
/// 仮置き: 64 MiB。対象機種（α7 IV / α7C）の埋め込み JPEG は数 MB 以下と想定し（実際の大きさは
/// PoC-2 で確認する）、壊れたファイルによる巨大な確保を防ぐために十分大きな値にした。
/// LibRaw 自身の上限（`LIBRAW_MAX_THUMBNAIL_MB` = 512 MB）より小さい。
pub const MAX_THUMBNAIL_BYTES: usize = 64 * 1024 * 1024;

/// 埋め込みサムネイルの形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThumbnailFormat {
    /// JPEG のファイルのバイト列。
    Jpeg,
    /// 8bit の RGB（行優先、1 画素 3 バイト）。
    Rgb8,
    /// 8bit のグレー（行優先、1 画素 1 バイト）。
    Gray8,
}

impl ThumbnailFormat {
    /// ビットマップの 1 画素のバイト数。JPEG は `None`。
    pub const fn bytes_per_pixel(self) -> Option<usize> {
        match self {
            ThumbnailFormat::Jpeg => None,
            ThumbnailFormat::Rgb8 => Some(3),
            ThumbnailFormat::Gray8 => Some(1),
        }
    }
}

/// RAW のファイルに埋め込まれたサムネイル（プレビュー）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedThumbnail {
    /// 形式。
    pub format: ThumbnailFormat,
    /// 幅（画素）。JPEG で寸法が分からない場合は 0。
    pub width: u32,
    /// 高さ（画素）。JPEG で寸法が分からない場合は 0。
    pub height: u32,
    /// バイト列（JPEG はファイルそのもの、ビットマップは画素の値）。
    pub data: Vec<u8>,
}

impl EmbeddedThumbnail {
    /// 大きさ・長さ・JPEG の先頭の印（SOI）を検証する。
    pub fn validate(&self) -> Result<(), RawError> {
        if self.data.is_empty() {
            return Err(RawError::Decode("埋め込みサムネイルが空です".to_owned()));
        }
        if self.data.len() > MAX_THUMBNAIL_BYTES {
            return Err(RawError::Decode(format!(
                "埋め込みサムネイルの大きさ {} バイトが上限 {MAX_THUMBNAIL_BYTES} を超えています",
                self.data.len()
            )));
        }
        match self.format.bytes_per_pixel() {
            None => {
                if !self.data.starts_with(&[0xFF, 0xD8]) {
                    return Err(RawError::Decode(
                        "埋め込みサムネイルが JPEG の形式ではありません（SOI がありません）"
                            .to_owned(),
                    ));
                }
            }
            Some(bpp) => {
                if self.width == 0 || self.height == 0 {
                    return Err(RawError::InvalidDimensions {
                        width: self.width,
                        height: self.height,
                    });
                }
                let expected = u64::from(self.width) * u64::from(self.height) * bpp as u64;
                if expected != self.data.len() as u64 {
                    return Err(RawError::DataLengthMismatch {
                        expected,
                        actual: self.data.len(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// JPEG のバイト列から寸法（幅, 高さ）を読む。SOF のマーカーが見つからなければ `None`。
///
/// 画素は展開しない。マーカーの並び（ITU-T T.81 の B.1.1）だけをたどる。
pub(crate) fn jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2usize;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        // 詰め物の 0xFF。
        if marker == 0xFF {
            i += 1;
            continue;
        }
        // 長さを持たないマーカー（TEM・RSTn・SOI）。
        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            i += 2;
            continue;
        }
        // EOI と SOS の後には SOF はない（ベースライン・プログレッシブとも SOF が先）。
        if marker == 0xD9 || marker == 0xDA {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        if len < 2 {
            return None;
        }
        // SOF0〜SOF15（DHT の C4、JPG の C8、DAC の CC を除く）。
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let seg = data.get(i + 4..i + 2 + len)?;
            if seg.len() < 5 {
                return None;
            }
            let height = u32::from(u16::from_be_bytes([seg[1], seg[2]]));
            let width = u32::from(u16::from_be_bytes([seg[3], seg[4]]));
            return (width > 0 && height > 0).then_some((width, height));
        }
        i += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SOF0 だけを含む最小の JPEG のマーカー列（画素のデータは持たない）。
    fn fake_jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        // APP0（長さ 16）。
        v.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        v.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        // SOF0: 長さ 17、精度 8、高さ、幅、成分 3。
        v.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        v.extend_from_slice(&height.to_be_bytes());
        v.extend_from_slice(&width.to_be_bytes());
        v.extend_from_slice(&[0x03, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        v.extend_from_slice(&[0xFF, 0xD9]);
        v
    }

    #[test]
    fn jpeg_dimensions_from_sof() {
        assert_eq!(jpeg_dimensions(&fake_jpeg(1616, 1080)), Some((1616, 1080)));
        assert_eq!(jpeg_dimensions(&fake_jpeg(1, 65535)), Some((1, 65535)));
        // 詰め物の 0xFF があっても読める。
        let mut padded = fake_jpeg(160, 120);
        padded.insert(2, 0xFF);
        assert_eq!(jpeg_dimensions(&padded), Some((160, 120)));
    }

    #[test]
    fn jpeg_dimensions_rejects_broken_data() {
        assert_eq!(jpeg_dimensions(&[]), None);
        assert_eq!(jpeg_dimensions(&[0xFF, 0xD8]), None);
        assert_eq!(jpeg_dimensions(b"not a jpeg"), None);
        // 途中で切れた SOF。
        let full = fake_jpeg(160, 120);
        for cut in 0..full.len() - 2 {
            let r = jpeg_dimensions(&full[..cut]);
            assert!(r.is_none() || r == Some((160, 120)), "cut = {cut}");
        }
        // 長さが 0 のセグメント。
        assert_eq!(jpeg_dimensions(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0]), None);
        // 寸法 0。
        assert_eq!(jpeg_dimensions(&fake_jpeg(0, 120)), None);
    }

    #[test]
    fn validate_checks_lengths_and_markers() {
        let jpeg = EmbeddedThumbnail {
            format: ThumbnailFormat::Jpeg,
            width: 160,
            height: 120,
            data: fake_jpeg(160, 120),
        };
        jpeg.validate().unwrap();
        let not_jpeg = EmbeddedThumbnail {
            data: vec![0, 1, 2],
            ..jpeg.clone()
        };
        assert!(matches!(not_jpeg.validate(), Err(RawError::Decode(_))));
        let empty = EmbeddedThumbnail {
            data: Vec::new(),
            ..jpeg
        };
        assert!(empty.validate().is_err());

        let rgb = EmbeddedThumbnail {
            format: ThumbnailFormat::Rgb8,
            width: 4,
            height: 2,
            data: vec![0; 24],
        };
        rgb.validate().unwrap();
        let short = EmbeddedThumbnail {
            data: vec![0; 23],
            ..rgb.clone()
        };
        assert!(matches!(
            short.validate(),
            Err(RawError::DataLengthMismatch {
                expected: 24,
                actual: 23
            })
        ));
        let zero = EmbeddedThumbnail {
            width: 0,
            ..rgb.clone()
        };
        assert!(matches!(
            zero.validate(),
            Err(RawError::InvalidDimensions { .. })
        ));
        let gray = EmbeddedThumbnail {
            format: ThumbnailFormat::Gray8,
            data: vec![0; 8],
            ..rgb
        };
        gray.validate().unwrap();
    }

    #[test]
    fn format_names_serialize() {
        assert_eq!(
            serde_json::to_string(&ThumbnailFormat::Rgb8).unwrap(),
            "\"rgb8\""
        );
        assert_eq!(ThumbnailFormat::Jpeg.bytes_per_pixel(), None);
        assert_eq!(ThumbnailFormat::Gray8.bytes_per_pixel(), Some(1));
    }
}
