//! ステージ 8: 色変換（docs/04_architecture.md の 2.1 節、2.6 節「カメラ RGB から作業色空間への変換」）。
//!
//! カメラ RGB（B1。WB 適用済み）にカメラ行列から作った 3 × 3 の行列を掛けて、作業色空間
//! （リニア ITU-R BT.2020・D65。B2）にする。行列は genzo-color の
//! [`genzo_color::camera_to_working_matrix`]（「XYZ（D65）→ カメラ RGB」の行列 `cam_xyz` から、
//! 中立色が保たれるよう行ごとに正規化して逆行列を取ったもの）を f64 で作り、f32 に丸めて使う。
//!
//! 画素の計算は [`genzo_color::Mat3F32::apply`]（各成分を `m[i][0]·r + m[i][1]·g + m[i][2]·b` の順に
//! 足す。融合積和演算は使わない）。負の値（色域外）はそのまま残す（B2）。
//!
//! カメラ行列がない場合の扱いは [`crate::sensor`] のモジュールの doc を参照。

use genzo_color::{Mat3, Mat3F32, camera_to_working_matrix};
use rayon::prelude::*;

use crate::contract::ColorContract;
use crate::error::{PipelineError, Result};
use crate::image::RgbImage;

/// 1 回にまとめて処理する画素の数（並列化の単位。結果には影響しない）。
const PIXELS_PER_TASK: usize = 16 * 1024;

/// カメラ RGB → 作業色空間の行列（f32）。`cam_xyz` が不正（非有限・中立色に反応しない・特異）なら
/// エラー。
pub fn camera_to_working(cam_xyz: &Mat3) -> Result<Mat3F32> {
    Ok(camera_to_working_matrix(cam_xyz)?.to_f32())
}

/// B1 の画像に行列を掛けて B2 にする（その場で書き換える）。
pub fn apply_color_matrix(image: &mut RgbImage, matrix: &Mat3F32) -> Result<()> {
    if image.contract() != ColorContract::B1CameraRgb {
        return Err(PipelineError::ContractMismatch {
            stage: super::ColorMatrixStage::ID,
            expected: ColorContract::B1CameraRgb,
            found: image.contract(),
        });
    }
    image
        .data_mut()
        .par_chunks_mut(PIXELS_PER_TASK)
        .for_each(|chunk| {
            for p in chunk {
                *p = matrix.apply(*p);
            }
        });
    image.set_contract(ColorContract::B2Working);
    Ok(())
}

#[cfg(test)]
mod tests {
    use genzo_color::space::{XYZ_TO_BT2020, XYZ_TO_SRGB};

    use super::*;

    fn typical_cam_xyz() -> Mat3 {
        // テスト用の典型的な形の行列（実在の機種の値ではない）。
        Mat3::from_rows([
            [0.70, -0.20, -0.06],
            [-0.50, 1.30, 0.23],
            [-0.07, 0.13, 0.65],
        ])
    }

    #[test]
    fn bt2020_camera_is_identity() {
        let m = camera_to_working(&XYZ_TO_BT2020).unwrap();
        let mut img = RgbImage::from_fn(3, 2, ColorContract::B1CameraRgb, |x, y| {
            [x as f32 * 0.3, y as f32 * 0.7, 0.05]
        })
        .unwrap();
        let before = img.clone();
        apply_color_matrix(&mut img, &m).unwrap();
        assert_eq!(img.contract(), ColorContract::B2Working);
        for (a, b) in img.data().iter().zip(before.data()) {
            for k in 0..3 {
                assert!((a[k] - b[k]).abs() < 1e-6, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn neutral_stays_neutral_and_out_of_gamut_is_kept() {
        let m = camera_to_working(&typical_cam_xyz()).unwrap();
        let mut img = RgbImage::from_vec(
            3,
            1,
            ColorContract::B1CameraRgb,
            vec![[0.18; 3], [2.5; 3], [0.0, 1.0, 0.0]],
        )
        .unwrap();
        apply_color_matrix(&mut img, &m).unwrap();
        for (p, v) in img.data()[..2].iter().zip([0.18f32, 2.5]) {
            for k in 0..3 {
                assert!((p[k] - v).abs() < 1e-5 * v.max(1.0), "{p:?}");
            }
        }
        // 純粋な緑のカメラ RGB は、BT.2020 の外（負の成分）になりうる。切り詰めない。
        assert!(img.data()[2].iter().all(|v| v.is_finite()));
        let srgb = camera_to_working(&XYZ_TO_SRGB).unwrap();
        let mut g = RgbImage::filled(1, 1, ColorContract::B1CameraRgb, [0.0, 1.0, 0.0]).unwrap();
        apply_color_matrix(&mut g, &srgb).unwrap();
        // BT.709 の緑は BT.2020 の内側（成分は正）。
        assert!(g.data()[0].iter().all(|&v| v >= 0.0));
    }

    #[test]
    fn wrong_contract_or_matrix_is_an_error() {
        let m = camera_to_working(&XYZ_TO_BT2020).unwrap();
        let mut img = RgbImage::new(1, 1, ColorContract::B2Working).unwrap();
        assert!(matches!(
            apply_color_matrix(&mut img, &m),
            Err(PipelineError::ContractMismatch { .. })
        ));
        let singular = Mat3::from_rows([[1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!(matches!(
            camera_to_working(&singular),
            Err(PipelineError::Color(_))
        ));
    }
}
