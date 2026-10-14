//! 透かしの見本と、OS のフォントのフォルダ（EXP-03。docs/04_architecture.md の 2.4 節。画面・CLI の確認用）。
//!
//! - 見本（[`Core::watermark_preview`]）: 灰色の濃淡（左が暗く右が明るい）の見本の画像（既定 600 × 400。sRGB）に、
//!   書き出しと同じ計算（大きさ・余白・位置・不透明度。`export::watermark_request`・`apply_watermark`）で透かしを
//!   重ね、PNG（8bit）にして返す。透かしはワーカーで作る（SEC-05）。フォントにない文字・縮めたことは警告で知らせる。
//! - フォントのフォルダ（[`system_font_dirs`]）: 文字の透かしのフォントを選ぶダイアログの最初の場所（フォントは
//!   同梱しない・ダウンロードしない。決定事項ログ No.167）。OS ごとの既定の場所のうち、あるものだけ。

use std::path::PathBuf;

use genzo_media::RgbImage8;
use genzo_model::{OutputColorSpace, Watermark};
use genzo_pipeline::finish::output::{Dither, requantize_u16_to_u8};
use genzo_worker::{Lane, WatermarkImage, WatermarkRequest};
use serde::{Deserialize, Serialize};

use crate::core::Core;
use crate::error::ApiError;
use crate::export::{apply_watermark, watermark_request, watermark_warnings};

/// 見本の画像の既定の寸法（横長の 3:2）。
pub const PREVIEW_DEFAULT_SIZE: (u32, u32) = (600, 400);
/// 見本の画像の長辺の上限（画素）。
pub const PREVIEW_MAX_EDGE: u32 = 2000;
/// 見本の画像の辺の下限（画素）。
pub const PREVIEW_MIN_EDGE: u32 = 16;

/// 透かしの見本（[`Core::watermark_preview`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatermarkPreview {
    /// 見本の画像の幅。
    pub width: u32,
    /// 見本の画像の高さ。
    pub height: u32,
    /// 見本の画像（PNG 8bit・sRGB）の base64（画面は `data:image/png;base64,` を付けて表示する）。
    pub png_base64: String,
    /// 透かしの幅。
    pub mark_width: u32,
    /// 透かしの高さ。
    pub mark_height: u32,
    /// 警告（フォントにない文字・写真に収まらないので縮めたなど）。
    pub warnings: Vec<String>,
}

/// OS の既定のフォントのフォルダ（あるものだけ。重ならない）。
pub fn system_font_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for d in candidate_font_dirs() {
        if d.is_dir() && !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

#[cfg(not(windows))]
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[cfg(windows)]
fn candidate_font_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for var in ["WINDIR", "SystemRoot"] {
        if let Some(w) = std::env::var_os(var).filter(|w| !w.is_empty()) {
            v.push(PathBuf::from(w).join("Fonts"));
        }
    }
    // 利用者ごとに入れたフォント（Windows 10 1809 以降）。
    if let Some(l) = std::env::var_os("LOCALAPPDATA").filter(|l| !l.is_empty()) {
        v.push(
            PathBuf::from(l)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    v
}

#[cfg(target_os = "macos")]
fn candidate_font_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = home() {
        v.push(h.join("Library").join("Fonts"));
    }
    v.push(PathBuf::from("/Library/Fonts"));
    v.push(PathBuf::from("/System/Library/Fonts"));
    v.push(PathBuf::from("/System/Library/Fonts/Supplemental"));
    v
}

#[cfg(not(any(windows, target_os = "macos")))]
fn candidate_font_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").filter(|x| !x.is_empty()) {
        Some(x) => v.push(PathBuf::from(x).join("fonts")),
        None => {
            if let Some(h) = home() {
                v.push(h.join(".local").join("share").join("fonts"));
            }
        }
    }
    if let Some(h) = home() {
        v.push(h.join(".fonts"));
    }
    v.push(PathBuf::from("/usr/local/share/fonts"));
    v.push(PathBuf::from("/usr/share/fonts"));
    v
}

/// base64（RFC 4648 の標準の文字・`=` で埋める）。
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(TABLE[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// 見本の背景（左が暗く右が明るい灰色。sRGB で符号化した 16bit）。
fn preview_background(width: u32, height: u32) -> Vec<u16> {
    let mut v = Vec::with_capacity((width * height * 3) as usize);
    for _ in 0..height {
        for x in 0..width {
            let t = if width > 1 {
                f64::from(x) / f64::from(width - 1)
            } else {
                0.5
            };
            let g = ((0.12 + 0.76 * t) * 65535.0).round() as u16;
            v.extend_from_slice(&[g, g, g]);
        }
    }
    v
}

/// 透かしの見本の PNG（8bit・sRGB）と情報を作る（モジュールの doc）。`render` は透かしを作る関数（本体はワーカーの組、
/// CLI の `genzo watermark preview` は自分で起動したワーカー）。`size` は見本の画像の寸法（省略時は
/// [`PREVIEW_DEFAULT_SIZE`]。[`PREVIEW_MIN_EDGE`]〜[`PREVIEW_MAX_EDGE`] に収める）。透かしの設定が正しくなければ
/// [`ApiError::InvalidArgument`]。
pub fn render_watermark_preview(
    watermark: &Watermark,
    size: Option<(u32, u32)>,
    render: impl FnOnce(&WatermarkRequest) -> Result<WatermarkImage, ApiError>,
) -> Result<(Vec<u8>, WatermarkPreview), ApiError> {
    if let Some(p) = watermark.problem() {
        return Err(ApiError::InvalidArgument(p));
    }
    let (w, h) = size.unwrap_or(PREVIEW_DEFAULT_SIZE);
    let (w, h) = (
        w.clamp(PREVIEW_MIN_EDGE, PREVIEW_MAX_EDGE),
        h.clamp(PREVIEW_MIN_EDGE, PREVIEW_MAX_EDGE),
    );
    let (request, geometry) = watermark_request(watermark, w, h, OutputColorSpace::Srgb);
    let image = render(&request)?;
    let mut data = preview_background(w, h);
    apply_watermark(&mut data, w, h, watermark, &geometry, &image)?;
    let rgb = RgbImage8::from_raw(w, h, requantize_u16_to_u8(&data, w, (0, 0), Dither::None))?;
    let png = genzo_media::png_io::encode_png(&rgb, None, None)?;
    let preview = WatermarkPreview {
        width: w,
        height: h,
        png_base64: base64_encode(&png),
        mark_width: image.bitmap.width(),
        mark_height: image.bitmap.height(),
        warnings: watermark_warnings(&image),
    };
    Ok((png, preview))
}

impl Core {
    /// 透かしの見本を作る（[`render_watermark_preview`]。透かしは対話用のワーカーで作る）。
    pub fn watermark_preview(
        &self,
        watermark: &Watermark,
        size: Option<(u32, u32)>,
    ) -> Result<WatermarkPreview, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        let pool = inner.pool()?;
        let token = genzo_jobs::CancellationToken::new();
        let (_, preview) = render_watermark_preview(watermark, size, |request| {
            Ok(pool.render_watermark(Lane::Interactive, request, &token)?)
        })?;
        Ok(preview)
    }

    /// OS の既定のフォントのフォルダ（[`system_font_dirs`]）。
    pub fn system_font_dirs(&self) -> Vec<PathBuf> {
        system_font_dirs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_examples() {
        // RFC 4648 の 10 章の例。
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(input.as_bytes()), want, "{input}");
        }
        assert_eq!(base64_encode(&[0xFF, 0xFE, 0xFD]), "//79");
    }

    #[test]
    fn background_is_a_gray_ramp() {
        let bg = preview_background(4, 2);
        assert_eq!(bg.len(), 4 * 2 * 3);
        assert!(bg[0] < bg[3] && bg[3] < bg[6] && bg[6] < bg[9]);
        assert_eq!(bg[0], bg[1]);
        assert_eq!(&bg[..12], &bg[12..]);
    }

    #[test]
    fn font_dirs_exist_and_are_unique() {
        let dirs = system_font_dirs();
        for d in &dirs {
            assert!(d.is_dir(), "{}", d.display());
        }
        let mut sorted = dirs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), dirs.len());
    }
}
