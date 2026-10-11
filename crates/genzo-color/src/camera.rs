//! カメラ RGB → 作業色空間（リニア BT.2020）の行列（ステージ 8。04 の 2.6 節
//! 「カメラ RGB から作業色空間への変換」）。
//!
//! 手順（2.6 節のとおり）:
//! 1. 入力は「XYZ（D65）→ カメラ RGB」の行列 `cam_xyz`。
//! 2. `BT.2020 → XYZ` を右から掛けて「BT.2020 → カメラ RGB」を作る。
//! 3. 中立色 (1, 1, 1) が (1, 1, 1) に対応するよう、**行ごとに**行の和で割って正規化する。
//! 4. 逆行列を取り、「カメラ RGB → BT.2020」を得る。
//!
//! B1（カメラ RGB）では WB の係数で被写体の中立色を (1, 1, 1) に正規化しているため、撮影時の
//! 光源の白は D65 の白に対応する（カメラ RGB の空間での簡易的な色順応。光源が D65 から離れる
//! ほど誤差が大きくなる。DEV-17 の DCP 方式で改善する予定）。
//!
//! LibRaw のどの行列（`cam_xyz`・`rgb_cam` など）を使うか、行列の基準の光源は PoC-2 で確認する
//! （2.6 節）。ここでは「XYZ（D65）→ カメラ RGB」の 3×3 の行列が与えられる前提で計算する。

use crate::error::{ColorError, Result};
use crate::matrix::Mat3;
use crate::space::BT2020_TO_XYZ;

/// 正規化に使う行の和の下限（行の要素の絶対値の和に対する比。仮置き）。
///
/// 行の和（中立色に対するそのチャンネルの応答）がこれ以下なら、そのチャンネルは中立色に
/// ほとんど反応しないため、正規化すると値が発散する。カメラ行列として不正とみなす。
/// 実在のカメラ行列の比は 0.1 以上が普通と考えて、十分小さい値を選んだ（根拠のある値ではない）。
pub const MIN_NEUTRAL_RESPONSE_RATIO: f64 = 1e-6;

/// 「BT.2020 → カメラ RGB」の行列を作り、中立色が (1, 1, 1) になるよう行ごとに正規化する
/// （2.6 節の手順 2〜3）。
///
/// エラー: 有限でない要素、中立色に反応しない行（[`MIN_NEUTRAL_RESPONSE_RATIO`]）、正規化した
/// 行列が特異（逆行列を取れない）。特異な行列は手順 4（[`camera_to_working_matrix`]）に使えない
/// ため、この関数でも拒否する（合成のテストデータを作る側と現像する側で、使える行列をそろえる）。
pub fn working_to_camera_normalized(cam_xyz: &Mat3) -> Result<Mat3> {
    if !cam_xyz.is_finite() {
        return Err(ColorError::NonFinite {
            what: "カメラ行列"
        });
    }
    let bt2020_to_cam = cam_xyz.mul_mat(&BT2020_TO_XYZ);
    let mut rows = bt2020_to_cam.rows();
    for row in &mut rows {
        let sum = row[0] + row[1] + row[2];
        let magnitude = row[0].abs() + row[1].abs() + row[2].abs();
        // NaN を含む比較は偽になるので、「応答が十分ある」ことを肯定形で判定する。
        let responsive = magnitude > 0.0 && sum > MIN_NEUTRAL_RESPONSE_RATIO * magnitude;
        if !responsive {
            return Err(ColorError::InvalidCameraMatrix {
                reason: "中立色に対するチャンネルの応答（行の和）が 0 以下か小さすぎる",
            });
        }
        for v in row.iter_mut() {
            *v /= sum;
        }
    }
    let normalized = Mat3::from_rows(rows);
    if normalized.checked_inverse().is_none() {
        return Err(ColorError::InvalidCameraMatrix {
            reason: SINGULAR_REASON,
        });
    }
    Ok(normalized)
}

/// 正規化した行列が特異なときのエラーの理由。
const SINGULAR_REASON: &str = "正規化した「BT.2020 → カメラ RGB」の行列が特異で、逆行列を取れない";

/// カメラ RGB（WB 適用済み、B1）→ 作業色空間（リニア BT.2020、B2）の行列（2.6 節の手順 1〜4）。
///
/// `cam_xyz` は「XYZ（D65）→ カメラ RGB」の行列。行ごとの尺度（チャンネルの感度）は正規化で
/// 打ち消されるので、任意の尺度でよい。
///
/// エラー: 有限でない要素、中立色に反応しない行（[`MIN_NEUTRAL_RESPONSE_RATIO`]）、逆行列を
/// 取れない（特異な）行列。
pub fn camera_to_working_matrix(cam_xyz: &Mat3) -> Result<Mat3> {
    let normalized = working_to_camera_normalized(cam_xyz)?;
    normalized.inverse().map_err(|e| match e {
        ColorError::SingularMatrix { .. } => ColorError::InvalidCameraMatrix {
            reason: SINGULAR_REASON,
        },
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::{
        D65, RgbColorSpace, RgbPrimaries, XYZ_TO_BT2020, XYZ_TO_SRGB, Xy, rgb_to_xyz_matrix,
        to_working,
    };

    /// テスト用の「典型的な形の」カメラ行列（XYZ → カメラ RGB。dcraw の表と同じく 1/10000 の
    /// 整数で書いた形）。実在の機種の値として使わないこと。
    fn typical_cam_xyz() -> Mat3 {
        Mat3::from_rows([
            [7000.0, -2000.0, -600.0],
            [-5000.0, 13000.0, 2300.0],
            [-700.0, 1300.0, 6500.0],
        ])
        .scale(1.0 / 10000.0)
    }

    fn assert_vec_close(a: [f64; 3], b: [f64; 3], tol: f64) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() <= tol, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn neutral_stays_neutral() {
        let m = camera_to_working_matrix(&typical_cam_xyz()).unwrap();
        assert_vec_close(m.apply([1.0, 1.0, 1.0]), [1.0, 1.0, 1.0], 1e-12);
        assert_vec_close(m.apply([0.18, 0.18, 0.18]), [0.18, 0.18, 0.18], 1e-12);
        // 行の和はすべて 1。
        for row in m.rows() {
            assert!((row[0] + row[1] + row[2] - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn bt2020_camera_gives_identity() {
        // カメラが BT.2020 そのもの（cam_xyz = XYZ → BT.2020）なら単位行列。
        let m = camera_to_working_matrix(&XYZ_TO_BT2020).unwrap();
        assert!(m.max_abs_diff(&Mat3::IDENTITY) < 1e-13);
    }

    #[test]
    fn srgb_camera_gives_bt709_to_bt2020() {
        // カメラが sRGB（BT.709）なら、BT.709 → BT.2020 の行列（ITU-R BT.2087）になる。
        let m = camera_to_working_matrix(&XYZ_TO_SRGB).unwrap();
        assert!(m.max_abs_diff(&to_working(RgbColorSpace::Srgb)) < 1e-13);
        let published = Mat3::from_rows([
            [0.6274, 0.3293, 0.0433],
            [0.0691, 0.9195, 0.0114],
            [0.0164, 0.0880, 0.8956],
        ]);
        assert!(m.max_abs_diff(&published) < 0.5e-4 + 1e-9);
    }

    #[test]
    fn row_scaling_is_cancelled() {
        // チャンネルの感度（行の尺度）が違っても結果は同じ。
        let base = typical_cam_xyz();
        let scaled = Mat3::diagonal([0.37, 2.5, 11.0]) * base;
        let a = camera_to_working_matrix(&base).unwrap();
        let b = camera_to_working_matrix(&scaled).unwrap();
        assert!(a.max_abs_diff(&b) < 1e-12);
    }

    #[test]
    fn known_camera_primaries_roundtrip() {
        // 原色が既知の仮想カメラ（D65 の白を (1,1,1) とする）。XYZ → カメラ RGB の行列を
        // わざと行ごとに別の尺度にして渡し、結果が「カメラ RGB → BT.2020」の正解と一致するか。
        let prim = RgbPrimaries {
            red: Xy::new(0.72, 0.27),
            green: Xy::new(0.18, 0.79),
            blue: Xy::new(0.13, 0.05),
        };
        let cam_to_xyz = rgb_to_xyz_matrix(prim, D65).unwrap();
        let cam_xyz = Mat3::diagonal([0.5, 1.3, 0.8]) * cam_to_xyz.inverse().unwrap();
        let expected = XYZ_TO_BT2020 * cam_to_xyz;
        let m = camera_to_working_matrix(&cam_xyz).unwrap();
        assert!(m.max_abs_diff(&expected) < 1e-12, "{m:?}\n{expected:?}");
        // 往復: BT.2020 → カメラ（正規化）→ BT.2020 は恒等。
        let fwd = working_to_camera_normalized(&cam_xyz).unwrap();
        assert!((m * fwd).max_abs_diff(&Mat3::IDENTITY) < 1e-12);
        // ある色をカメラで撮った値（正規化後のカメラ RGB）から元の BT.2020 の値に戻る。
        let color = [0.3, 0.6, 0.1];
        let cam = fwd.apply(color);
        assert_vec_close(m.apply(cam), color, 1e-12);
    }

    #[test]
    fn typical_matrix_roundtrip() {
        let cam_xyz = typical_cam_xyz();
        let fwd = working_to_camera_normalized(&cam_xyz).unwrap();
        let inv = camera_to_working_matrix(&cam_xyz).unwrap();
        assert!((inv * fwd).max_abs_diff(&Mat3::IDENTITY) < 1e-12);
        assert!((fwd * inv).max_abs_diff(&Mat3::IDENTITY) < 1e-12);
    }

    #[test]
    fn invalid_matrices_are_errors() {
        // 非有限。
        let mut rows = typical_cam_xyz().rows();
        rows[1][1] = f64::NAN;
        assert!(matches!(
            camera_to_working_matrix(&Mat3::from_rows(rows)),
            Err(ColorError::NonFinite { .. })
        ));
        // 中立色に反応しない行（行の和が 0）: XYZ → BT.2020 の行と組み合わせて和を 0 にする。
        let zero_row = Mat3::from_rows([[0.0; 3], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!(matches!(
            camera_to_working_matrix(&zero_row),
            Err(ColorError::InvalidCameraMatrix { .. })
        ));
        // 行の和が負（中立色に負の応答）。
        let neg = Mat3::diagonal([-1.0, 1.0, 1.0]) * XYZ_TO_BT2020;
        assert!(matches!(
            camera_to_working_matrix(&neg),
            Err(ColorError::InvalidCameraMatrix { .. })
        ));
        // 特異: 2 行が同じ。
        let r = XYZ_TO_BT2020.rows();
        let singular = Mat3::from_rows([r[0], r[0], r[2]]);
        assert!(matches!(
            camera_to_working_matrix(&singular),
            Err(ColorError::InvalidCameraMatrix { .. })
        ));
    }

    #[test]
    fn forward_matrix_rejects_what_the_inverse_rejects() {
        // 「BT.2020 → カメラ RGB」の正規化でも、手順 4 で逆行列を取れない行列は拒否する
        // （genzo-testkit の合成データの作成と、ステージ 8 で使える行列をそろえる）。
        let r = XYZ_TO_BT2020.rows();
        // 2 行が同じ（行の和は正なので、行の和の判定は通る）。
        let singular = Mat3::from_rows([r[0], r[0], r[2]]);
        assert!(matches!(
            working_to_camera_normalized(&singular),
            Err(ColorError::InvalidCameraMatrix { .. })
        ));
        // 3 行目が 1 行目と 2 行目の和（線形従属）。
        let dependent = Mat3::from_rows([
            r[0],
            r[1],
            [r[0][0] + r[1][0], r[0][1] + r[1][1], r[0][2] + r[1][2]],
        ]);
        assert!(matches!(
            working_to_camera_normalized(&dependent),
            Err(ColorError::InvalidCameraMatrix { .. })
        ));
        assert!(camera_to_working_matrix(&dependent).is_err());
        // 正常な行列は、どちらの関数でも受け付ける。
        assert!(working_to_camera_normalized(&typical_cam_xyz()).is_ok());
        assert!(camera_to_working_matrix(&typical_cam_xyz()).is_ok());
    }
}
