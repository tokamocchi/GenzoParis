//! 現像のプレビューと表示範囲の描画の画素を、WebView に渡すバイト列にする。
//!
//! JSON では画素が大きすぎるため、Tauri の IPC のバイナリの応答（`tauri::ipc::Response`）で次の形の
//! バイト列を返す（画面の `ui/src/api/frames.ts` が読む）。
//!
//! | 位置 | 内容 |
//! |---|---|
//! | 0〜3 | ヘッダーの JSON の長さ（u32、リトルエンディアン） |
//! | 4〜 | ヘッダーの JSON（UTF-8。`FrameInfo` または `RegionInfo`） |
//! | その後 | RGBA の 8bit の画素（幅 × 高さ × 4。不透明。canvas の `ImageData` にそのまま入れる） |
//!
//! 画素の色空間は [`crate::state::DisplayTarget`]（Display P3 か sRGB）。

use serde::Serialize;

/// ヘッダーと RGB の画素から、画面に渡すバイト列を作る。
///
/// `rgb` の長さが `width × height × 3` でなければ `None`（不具合。呼び出し側で記録する）。
pub fn encode<H: Serialize>(header: &H, width: u32, height: u32, rgb: &[u8]) -> Option<Vec<u8>> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    if rgb.len() != pixels.checked_mul(3)? {
        return None;
    }
    let json = serde_json::to_vec(header).ok()?;
    let len = u32::try_from(json.len()).ok()?;
    let mut out = Vec::with_capacity(4 + json.len() + pixels * 4);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&json);
    for px in rgb.chunks_exact(3) {
        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_length_header_then_rgba() {
        #[derive(Serialize)]
        struct H {
            width: u32,
        }
        let rgb = [1u8, 2, 3, 4, 5, 6];
        let out = encode(&H { width: 2 }, 2, 1, &rgb).unwrap();
        let len = u32::from_le_bytes(out[0..4].try_into().unwrap()) as usize;
        let header: serde_json::Value = serde_json::from_slice(&out[4..4 + len]).unwrap();
        assert_eq!(header["width"], 2);
        assert_eq!(&out[4 + len..], &[1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn wrong_length_is_rejected() {
        assert!(encode(&0u8, 2, 2, &[0; 11]).is_none());
        assert!(encode(&0u8, u32::MAX, u32::MAX, &[]).is_none());
    }
}
