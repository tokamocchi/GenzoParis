//! OKLab と OKLCh（Björn Ottosson, "A perceptual color space for image processing", 2020）。
//!
//! 彩度・色相の調整（ステージ 13）を知覚的に均等な色空間で行うための候補（04 の 2.1 節。
//! どの色空間を使うかは PoC-4 で決める）。
//!
//! - 定義: XYZ（D65、白の Y = 1）に M1 を掛けて LMS にし、各成分の立方根を取って M2 を掛ける。
//!   M1・M2 は Ottosson の公表値（小数 10 桁）。
//! - 作業色空間（リニア BT.2020）からは、`M1 · (BT.2020 → XYZ)` を f64 で合成した行列を使う。
//! - **立方根は符号付き**（`cbrt(-x) = -cbrt(x)`）。色域外の色（負の値を含む RGB）でも NaN に
//!   ならず、逆変換で元に戻る。
//! - 画素ごとの変換（f32）は f32 のまま計算する（04 の 2.3 節）。非有限の入力（NaN・無限大）は
//!   そのまま伝わる。NaN の検出は各ステージの出力で行う（2.6 節）。
//! - OKLCh の色相は **度**（0 以上 360 未満）。彩度 C が 0 のときの色相は 0。
//! - **無彩色の軸のずれ**: M1・M2（小数 10 桁の公表値）と、この crate の BT.2020 の白（D65 の xy
//!   から求めた値）の丸めの違いで、作業色空間の無彩色 (g, g, g) の a・b はちょうど 0 にならない。
//!   この crate の行列では `(a / L, b / L)` ≒ (−2.2e−5, −1.23e−4) で、g によらずほぼ一定
//!   （f32 版・f64 版とも。2026-10-10 に計算で確認）。彩度を「a・b の大きさ」で測ると無彩色でも
//!   わずかに 0 でないので、無彩色からの差を使う場合は白 (1, 1, 1) の `(a / L, b / L)` を軸として
//!   引く（genzo-pipeline のステージ 13 がそうしている）。

use crate::matrix::{Mat3, Mat3F32};
use crate::space::BT2020_TO_XYZ;

/// OKLab の M1（XYZ（D65）→ LMS）。Ottosson の公表値。
pub const OKLAB_M1: Mat3 = Mat3::from_rows([
    [0.8189330101, 0.3618667424, -0.1288597137],
    [0.0329845436, 0.9293118715, 0.0361456387],
    [0.0482003018, 0.2643662691, 0.6338517070],
]);

/// OKLab の M2（立方根を取った LMS → Lab）。Ottosson の公表値。
pub const OKLAB_M2: Mat3 = Mat3::from_rows([
    [0.2104542553, 0.7936177850, -0.0040720468],
    [1.9779984951, -2.4285922050, 0.4505937099],
    [0.0259040371, 0.7827717662, -0.8086757660],
]);

const fn expect_const(m: Option<Mat3>) -> Mat3 {
    match m {
        Some(m) => m,
        None => panic!("OKLab の逆行列を導出できない"),
    }
}

const OKLAB_M1_INV: Mat3 = expect_const(OKLAB_M1.checked_inverse());
const OKLAB_M2_INV: Mat3 = expect_const(OKLAB_M2.checked_inverse());

/// リニア BT.2020 → LMS（`M1 · (BT.2020 → XYZ)`）。
pub const BT2020_TO_OKLAB_LMS: Mat3 = OKLAB_M1.mul_mat(&BT2020_TO_XYZ);
/// LMS → リニア BT.2020。
pub const OKLAB_LMS_TO_BT2020: Mat3 = expect_const(BT2020_TO_OKLAB_LMS.checked_inverse());

/// [`BT2020_TO_OKLAB_LMS`] の f32 版（画素ごとの変換で使う値そのもの。GPU 版も同じ値を使う）。
pub const BT2020_TO_OKLAB_LMS_F32: Mat3F32 = BT2020_TO_OKLAB_LMS.to_f32();
/// [`OKLAB_LMS_TO_BT2020`] の f32 版。
pub const OKLAB_LMS_TO_BT2020_F32: Mat3F32 = OKLAB_LMS_TO_BT2020.to_f32();
/// [`OKLAB_M2`] の f32 版。
pub const OKLAB_M2_F32: Mat3F32 = OKLAB_M2.to_f32();
/// [`OKLAB_M2`] の逆行列（f64 で求めたもの）の f32 版。
pub const OKLAB_M2_INV_F32: Mat3F32 = OKLAB_M2_INV.to_f32();

/// XYZ（D65、白の Y = 1）→ OKLab（f64）。
pub fn xyz_d65_to_oklab(xyz: [f64; 3]) -> [f64; 3] {
    let lms = OKLAB_M1.apply(xyz);
    OKLAB_M2.apply([lms[0].cbrt(), lms[1].cbrt(), lms[2].cbrt()])
}

/// OKLab → XYZ（D65、白の Y = 1）（f64）。
pub fn oklab_to_xyz_d65(lab: [f64; 3]) -> [f64; 3] {
    let c = OKLAB_M2_INV.apply(lab);
    OKLAB_M1_INV.apply([c[0] * c[0] * c[0], c[1] * c[1] * c[1], c[2] * c[2] * c[2]])
}

/// リニア BT.2020（作業色空間）→ OKLab（f32）。負の値を含む入力でも NaN にならない。
#[inline]
pub fn linear_bt2020_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    let lms = BT2020_TO_OKLAB_LMS_F32.apply(rgb);
    OKLAB_M2_F32.apply([lms[0].cbrt(), lms[1].cbrt(), lms[2].cbrt()])
}

/// OKLab → リニア BT.2020（作業色空間）（f32）。
#[inline]
pub fn oklab_to_linear_bt2020(lab: [f32; 3]) -> [f32; 3] {
    let c = OKLAB_M2_INV_F32.apply(lab);
    OKLAB_LMS_TO_BT2020_F32.apply([c[0] * c[0] * c[0], c[1] * c[1] * c[1], c[2] * c[2] * c[2]])
}

/// OKLab → OKLCh（L, C, h）。h は度で 0 以上 360 未満。C = 0 なら h = 0。
#[inline]
pub fn oklab_to_oklch(lab: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = lab;
    let c = a.hypot(b);
    let h = if c == 0.0 {
        0.0
    } else {
        let h = b.atan2(a).to_degrees();
        if h < 0.0 { h + 360.0 } else { h }
    };
    // 丸めで 360 ちょうどになった場合は 0 にそろえる。
    let h = if h >= 360.0 { 0.0 } else { h };
    [l, c, h]
}

/// OKLCh（L, C, h 度）→ OKLab。
#[inline]
pub fn oklch_to_oklab(lch: [f32; 3]) -> [f32; 3] {
    let [l, c, h] = lch;
    let (s, co) = h.to_radians().sin_cos();
    [l, c * co, c * s]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::{D65, RgbColorSpace};

    #[test]
    fn matches_ottosson_published_values() {
        // Ottosson のブログに掲載されている XYZ → OKLab の値（小数 3 桁）。
        let cases: [([f64; 3], [f64; 3]); 4] = [
            ([0.950, 1.000, 1.089], [1.000, 0.000, 0.000]),
            ([1.000, 0.000, 0.000], [0.450, 1.236, -0.019]),
            ([0.000, 1.000, 0.000], [0.922, -0.671, 0.263]),
            ([0.000, 0.000, 1.000], [0.153, -1.415, -0.449]),
        ];
        for (xyz, expected) in cases {
            let lab = xyz_d65_to_oklab(xyz);
            for k in 0..3 {
                assert!(
                    (lab[k] - expected[k]).abs() < 0.5e-3 + 1e-9,
                    "{xyz:?} → {lab:?}（公表値 {expected:?}）"
                );
            }
        }
    }

    #[test]
    fn white_is_l1_neutral() {
        let w = D65.to_xyz(1.0).unwrap();
        let lab = xyz_d65_to_oklab(w);
        assert!((lab[0] - 1.0).abs() < 1e-4 && lab[1].abs() < 1e-3 && lab[2].abs() < 1e-3);
        let lab32 = linear_bt2020_to_oklab([1.0, 1.0, 1.0]);
        assert!((lab32[0] - 1.0).abs() < 1e-4 && lab32[1].abs() < 1e-3 && lab32[2].abs() < 1e-3);
        // グレーは a, b ≒ 0（白と同じ程度）で、L は Y の立方根。
        let g = linear_bt2020_to_oklab([0.18, 0.18, 0.18]);
        assert!((g[0] - 0.18_f32.cbrt()).abs() < 1e-4);
        assert!(g[1].abs() < 1e-3 && g[2].abs() < 1e-3);
    }

    #[test]
    fn bt2020_path_matches_xyz_path() {
        let samples = [
            [0.2_f32, 0.5, 0.8],
            [1.0, 0.0, 0.0],
            [0.03, 0.02, 0.01],
            [2.0, 1.5, 0.5],
        ];
        for rgb in samples {
            let xyz = BT2020_TO_XYZ.apply([rgb[0] as f64, rgb[1] as f64, rgb[2] as f64]);
            let a = xyz_d65_to_oklab(xyz);
            let b = linear_bt2020_to_oklab(rgb);
            for k in 0..3 {
                assert!((a[k] - b[k] as f64).abs() < 1e-5, "{rgb:?}");
            }
        }
    }

    #[test]
    fn roundtrip_f64() {
        for xyz in [
            [0.3, 0.4, 0.5],
            [0.0, 0.0, 0.0],
            [-0.05, 0.1, 0.4],
            [1.5, 1.2, 0.2],
        ] {
            let back = oklab_to_xyz_d65(xyz_d65_to_oklab(xyz));
            for k in 0..3 {
                assert!((back[k] - xyz[k]).abs() < 1e-12, "{xyz:?} → {back:?}");
            }
        }
    }

    #[test]
    fn roundtrip_f32_including_out_of_gamut() {
        let samples = [
            [0.5_f32, 0.4, 0.3],
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [-0.1, 0.5, 0.2],
            [0.8, -0.05, -0.02],
            [-0.01, -0.02, -0.03],
            [4.0, 2.0, 0.0],
        ];
        for rgb in samples {
            let lab = linear_bt2020_to_oklab(rgb);
            assert!(lab.iter().all(|v| v.is_finite()), "{rgb:?} → {lab:?}");
            let back = oklab_to_linear_bt2020(lab);
            for k in 0..3 {
                assert!(
                    (back[k] - rgb[k]).abs() < 2e-5 * rgb[k].abs().max(1.0),
                    "{rgb:?} → {back:?}"
                );
            }
        }
    }

    #[test]
    fn negative_inputs_never_nan() {
        // 負の値を含む格子をくまなく調べる。
        let vals = [-2.0_f32, -0.5, -1e-6, 0.0, 1e-6, 0.3, 1.0, 3.0];
        for &r in &vals {
            for &g in &vals {
                for &b in &vals {
                    let lab = linear_bt2020_to_oklab([r, g, b]);
                    assert!(lab.iter().all(|v| v.is_finite()), "{:?}", [r, g, b]);
                    let lch = oklab_to_oklch(lab);
                    assert!(lch.iter().all(|v| v.is_finite()));
                    let back = oklab_to_linear_bt2020(oklch_to_oklab(lch));
                    assert!(back.iter().all(|v| v.is_finite()));
                }
            }
        }
    }

    #[test]
    fn oklch_roundtrip_and_hue_range() {
        let samples = [
            [0.7_f32, 0.1, 0.05],
            [0.5, -0.1, 0.02],
            [0.4, -0.03, -0.12],
            [0.6, 0.05, -0.2],
        ];
        for lab in samples {
            let lch = oklab_to_oklch(lab);
            assert!((0.0..360.0).contains(&lch[2]));
            assert!((lch[1] - lab[1].hypot(lab[2])).abs() < 1e-7);
            let back = oklch_to_oklab(lch);
            for k in 0..3 {
                assert!((back[k] - lab[k]).abs() < 1e-6);
            }
        }
        // 無彩色は色相 0。
        assert_eq!(oklab_to_oklch([0.5, 0.0, 0.0]), [0.5, 0.0, 0.0]);
        // 純粋な +b は 90 度、−a は 180 度。
        assert!((oklab_to_oklch([0.5, 0.0, 0.1])[2] - 90.0).abs() < 1e-5);
        assert!((oklab_to_oklch([0.5, -0.1, 0.0])[2] - 180.0).abs() < 1e-5);
    }

    #[test]
    fn srgb_red_hue_is_plausible() {
        // sRGB の赤（リニア (1,0,0)）を作業色空間に移して OKLCh にすると、色相は約 29 度
        // （OKLab の sRGB の赤は L ≒ 0.628, C ≒ 0.258, h ≒ 29.2°）。
        let red_2020 = crate::space::to_working(RgbColorSpace::Srgb).apply([1.0, 0.0, 0.0]);
        let lab =
            linear_bt2020_to_oklab([red_2020[0] as f32, red_2020[1] as f32, red_2020[2] as f32]);
        let lch = oklab_to_oklch(lab);
        assert!((lch[0] - 0.628).abs() < 2e-3, "{lch:?}");
        assert!((lch[1] - 0.258).abs() < 2e-3, "{lch:?}");
        assert!((lch[2] - 29.2).abs() < 0.3, "{lch:?}");
        // 逆方向の定数も正しく合成されている。
        let m = OKLAB_LMS_TO_BT2020 * BT2020_TO_OKLAB_LMS;
        assert!(m.max_abs_diff(&Mat3::IDENTITY) < 1e-12);
    }
}
