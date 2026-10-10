//! RAW 以外の入力（JPEG / TIFF / PNG）を作業色空間（B2）にする（docs/04_architecture.md の 2.6 節
//! 「RAW 以外の入力」: 埋め込みの ICC プロファイル（なければ sRGB とみなす）から、lcms2 で B2 に変換し、
//! ステージ 1〜8 を飛ばす）。
//!
//! デコード（genzo-media。信頼できない入力なのでワーカープロセスで行う）の結果の 8bit / 16bit の画素と
//! プロファイルを受け取り、[`working_image_from_encoded`] でリニア BT.2020（B2）の [`RgbImage`] にする。
//! 向きは反映しない（[`crate::SourceImage::Working`] の `orientation` で渡し、ステージ 9 が反映する）。
//!
//! # 変換の定義
//!
//! - 変換元: 埋め込みのプロファイル（RGB で、変換に使えるもの。genzo-media が確認する）。`None` なら
//!   genzo-color の sRGB（IEC 61966-2-1）。
//! - 変換先: genzo-color の [`StandardProfile::LinearBt2020`]（リニア BT.2020・D65。ガンマ 1 なので、
//!   lcms2 の浮動小数点の変換で 1 超え・負の値もそのまま保たれる）。
//! - レンダリングインテント: 相対的な色域を維持（[`RenderingIntent::RelativeColorimetric`]。白を作業色空間の
//!   白 (1, 1, 1) に合わせる。黒点の補正は使わない）。
//! - 符号化した値は `v / 255`・`v / 65535`（0〜1）。
//! - 変換の結果の NaN・無限大は 0 にする（2.6 節）。
//!
//! lcms2 の浮動小数点の変換は OS ごとに最下位のビットが違いうるので、比較には許容誤差を持たせる。

use std::sync::OnceLock;

use genzo_color::{IccProfile, IccTransform, IccVersion, RenderingIntent, StandardProfile};

use crate::contract::ColorContract;
use crate::error::{PipelineError, Result};
use crate::image::{RgbImage, checked_len};

/// 変換先（リニア BT.2020）と、変換元を省略したとき（sRGB）のプロファイル（1 回だけ作る）。
fn standard(kind: StandardProfile) -> Result<IccProfile> {
    static LINEAR: OnceLock<std::result::Result<IccProfile, String>> = OnceLock::new();
    static SRGB: OnceLock<std::result::Result<IccProfile, String>> = OnceLock::new();
    let cell = match kind {
        StandardProfile::LinearBt2020 => &LINEAR,
        StandardProfile::Srgb => &SRGB,
        other => {
            return IccProfile::standard_with_version(other, IccVersion::V4_3)
                .map_err(PipelineError::from);
        }
    };
    cell.get_or_init(|| {
        IccProfile::standard_with_version(kind, IccVersion::V4_3).map_err(|e| e.to_string())
    })
    .clone()
    .map_err(PipelineError::InvalidArgument)
}

/// 8bit の RGB（R, G, B, … の並び）を 0〜1 の値にする。長さが 3 の倍数でなければエラー。
pub fn encoded_from_u8(data: &[u8]) -> Result<Vec<[f32; 3]>> {
    to_rgb(data, |v| f32::from(v) / 255.0)
}

/// 16bit の RGB を 0〜1 の値にする。長さが 3 の倍数でなければエラー。
pub fn encoded_from_u16(data: &[u16]) -> Result<Vec<[f32; 3]>> {
    to_rgb(data, |v| f32::from(v) / 65535.0)
}

fn to_rgb<T: Copy>(data: &[T], f: impl Fn(T) -> f32) -> Result<Vec<[f32; 3]>> {
    if !data.len().is_multiple_of(3) {
        return Err(PipelineError::InvalidArgument(format!(
            "RGB の値の数 {} が 3 の倍数ではありません",
            data.len()
        )));
    }
    Ok(data
        .chunks_exact(3)
        .map(|c| [f(c[0]), f(c[1]), f(c[2])])
        .collect())
}

/// 符号化した RGB（0〜1。`profile` の色空間。`None` なら sRGB）を、リニア BT.2020（B2）の画像にする
/// （モジュールの doc の定義）。
pub fn working_image_from_encoded(
    width: u32,
    height: u32,
    encoded: &[[f32; 3]],
    profile: Option<&IccProfile>,
) -> Result<RgbImage> {
    let len = checked_len(width, height)?;
    if encoded.len() != len {
        return Err(PipelineError::DataLengthMismatch {
            expected: len,
            actual: encoded.len(),
        });
    }
    let src = match profile {
        Some(p) => p.clone(),
        None => standard(StandardProfile::Srgb)?,
    };
    let dst = standard(StandardProfile::LinearBt2020)?;
    let transform = IccTransform::new(&src, &dst, RenderingIntent::RelativeColorimetric)?;
    let mut data = vec![[0.0f32; 3]; len];
    transform.transform(encoded, &mut data)?;
    let mut image = RgbImage::from_vec(width, height, ColorContract::B2Working, data)?;
    image.sanitize();
    Ok(image)
}

#[cfg(test)]
mod tests {
    use genzo_color::space::to_working;
    use genzo_color::transfer::srgb_decode;
    use genzo_color::{RgbColorSpace, StandardProfile};

    use super::*;

    #[test]
    fn srgb_values_become_linear_bt2020() {
        let enc = encoded_from_u8(&[128, 128, 128, 255, 0, 0, 0, 0, 0, 255, 255, 255]).unwrap();
        let img = working_image_from_encoded(4, 1, &enc, None).unwrap();
        assert_eq!(img.contract(), ColorContract::B2Working);
        // 中間のグレーは中立のまま、リニアの値（約 0.2158）になる。
        let g = srgb_decode(128.0 / 255.0) as f32;
        let p = img.pixel(0, 0).unwrap();
        assert!(p.iter().all(|v| (v - g).abs() < 2e-3), "{p:?} vs {g}");
        // sRGB の赤は BT.2020 の行列で移した値。
        let red = to_working(RgbColorSpace::Srgb).apply([1.0, 0.0, 0.0]);
        let p = img.pixel(1, 0).unwrap();
        for k in 0..3 {
            assert!((f64::from(p[k]) - red[k]).abs() < 2e-3, "{p:?} vs {red:?}");
        }
        assert_eq!(img.pixel(2, 0).unwrap(), [0.0; 3]);
        let w = img.pixel(3, 0).unwrap();
        assert!(w.iter().all(|v| (v - 1.0).abs() < 2e-3), "{w:?}");
    }

    #[test]
    fn embedded_profiles_are_used() {
        // Display P3 の (1, 0, 0) は sRGB の (1, 0, 0) より彩度が高い（BT.2020 の G・B が負に近い）。
        let p3 = IccProfile::standard(StandardProfile::DisplayP3).unwrap();
        let enc = vec![[1.0, 0.0, 0.0]];
        let a = working_image_from_encoded(1, 1, &enc, Some(&p3)).unwrap();
        let expect = to_working(RgbColorSpace::DisplayP3).apply([1.0, 0.0, 0.0]);
        let p = a.pixel(0, 0).unwrap();
        for k in 0..3 {
            assert!(
                (f64::from(p[k]) - expect[k]).abs() < 2e-3,
                "{p:?} vs {expect:?}"
            );
        }
        let b = working_image_from_encoded(1, 1, &enc, None).unwrap();
        assert!(p[0] > b.pixel(0, 0).unwrap()[0]);
    }

    #[test]
    fn lengths_are_checked() {
        assert!(encoded_from_u8(&[1, 2]).is_err());
        assert!(encoded_from_u16(&[1, 2, 3, 4]).is_err());
        let v = encoded_from_u16(&[65535, 0, 32768]).unwrap();
        assert_eq!(v[0][0], 1.0);
        assert!(working_image_from_encoded(2, 2, &[[0.0; 3]; 3], None).is_err());
        assert!(working_image_from_encoded(0, 2, &[], None).is_err());
    }
}
