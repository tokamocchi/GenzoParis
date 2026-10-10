//! 原色・白色点・RGB ⇔ XYZ の行列・色順応（04 の 2.6 節）。
//!
//! - 作業色空間（B2・B3）は **リニア BT.2020（D65）**。出力の色空間（B4b・B5）は sRGB（BT.709）・
//!   Display P3・Adobe RGB (1998) で、白色点はすべて D65。そのため、作業色空間と出力の色空間の
//!   変換は XYZ を経由する行列の積だけで、色順応は要らない。
//! - XYZ は白（RGB = (1, 1, 1)）の Y を 1 とする相対値。
//! - D50 ⇔ D65 の色順応には Bradford 法を使う（PoC-4 のカラーチャートの基準値（D50）を D65 に
//!   順応させる。05 の PoC-4）。
//!
//! 行列は const fn で導出し、定数としても使えるようにしている（コンパイル時に計算される）。

use crate::error::{ColorError, Result};
use crate::matrix::Mat3;
use crate::transfer::TransferFunction;

/// CIE 1931 の色度座標 (x, y)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xy {
    /// x。
    pub x: f64,
    /// y。
    pub y: f64,
}

impl Xy {
    /// 作る。
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// 輝度 `big_y` の XYZ にする（`X = x·Y/y`、`Z = (1 − x − y)·Y/y`）。
    ///
    /// y ≤ 0 または有限でない値ならエラー。
    pub fn to_xyz(self, big_y: f64) -> Result<[f64; 3]> {
        if !(self.x.is_finite() && self.y.is_finite() && self.y > 0.0 && big_y.is_finite()) {
            return Err(ColorError::InvalidChromaticity {
                x: self.x,
                y: self.y,
            });
        }
        Ok(xy_to_xyz_const(self, big_y))
    }

    /// XYZ から色度座標を求める。X + Y + Z ≤ 0 または有限でない値ならエラー。
    pub fn from_xyz(xyz: [f64; 3]) -> Result<Xy> {
        let sum = xyz[0] + xyz[1] + xyz[2];
        if !(sum.is_finite() && sum > 0.0) {
            return Err(ColorError::InvalidChromaticity {
                x: f64::NAN,
                y: f64::NAN,
            });
        }
        Ok(Xy::new(xyz[0] / sum, xyz[1] / sum))
    }

    /// CIE 1960 UCS の (u, v) にする（`u = 4x / (−2x + 12y + 3)`、`v = 6y / (−2x + 12y + 3)`）。
    pub fn to_uv(self) -> Uv {
        let d = -2.0 * self.x + 12.0 * self.y + 3.0;
        Uv {
            u: 4.0 * self.x / d,
            v: 6.0 * self.y / d,
        }
    }
}

/// CIE 1960 UCS の色度座標 (u, v)。色温度と色かぶり補正（tint）の計算に使う
/// （[`crate::white_balance`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Uv {
    /// u。
    pub u: f64,
    /// v。
    pub v: f64,
}

impl Uv {
    /// 作る。
    pub const fn new(u: f64, v: f64) -> Self {
        Self { u, v }
    }

    /// CIE 1931 の (x, y) にする（`x = 3u / (2u − 8v + 4)`、`y = 2v / (2u − 8v + 4)`）。
    pub fn to_xy(self) -> Xy {
        let d = 2.0 * self.u - 8.0 * self.v + 4.0;
        Xy::new(3.0 * self.u / d, 2.0 * self.v / d)
    }
}

/// D50 の白色点（CIE 15 の 2° 視野の値を小数 4 桁にしたもの）。
pub const D50: Xy = Xy::new(0.3457, 0.3585);
/// D65 の白色点（ITU-R BT.709・BT.2020、IEC 61966-2-1 が使う値）。
pub const D65: Xy = Xy::new(0.3127, 0.3290);

/// RGB の 3 原色の色度座標。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RgbPrimaries {
    /// 赤。
    pub red: Xy,
    /// 緑。
    pub green: Xy,
    /// 青。
    pub blue: Xy,
}

/// ITU-R BT.2020 の原色。
pub const BT2020_PRIMARIES: RgbPrimaries = RgbPrimaries {
    red: Xy::new(0.708, 0.292),
    green: Xy::new(0.170, 0.797),
    blue: Xy::new(0.131, 0.046),
};
/// ITU-R BT.709（= sRGB、IEC 61966-2-1）の原色。
pub const BT709_PRIMARIES: RgbPrimaries = RgbPrimaries {
    red: Xy::new(0.640, 0.330),
    green: Xy::new(0.300, 0.600),
    blue: Xy::new(0.150, 0.060),
};
/// Display P3 の原色（DCI-P3 の原色。白色点は D65）。
pub const DISPLAY_P3_PRIMARIES: RgbPrimaries = RgbPrimaries {
    red: Xy::new(0.680, 0.320),
    green: Xy::new(0.265, 0.690),
    blue: Xy::new(0.150, 0.060),
};
/// Adobe RGB (1998) の原色。
pub const ADOBE_RGB_PRIMARIES: RgbPrimaries = RgbPrimaries {
    red: Xy::new(0.6400, 0.3300),
    green: Xy::new(0.2100, 0.7100),
    blue: Xy::new(0.1500, 0.0600),
};

/// y で割る（Y = 1 の XYZ）。const fn 用。y の検査は呼び出し側で行う。
const fn xy_to_xyz_const(xy: Xy, big_y: f64) -> [f64; 3] {
    [
        xy.x * big_y / xy.y,
        big_y,
        (1.0 - xy.x - xy.y) * big_y / xy.y,
    ]
}

/// 原色と白色点から RGB → XYZ の行列を導出する（const fn 版。導出できなければ `None`）。
///
/// 各原色の XYZ（Y = 1）を列に並べた行列 P について、`S = P⁻¹ · W`（W は白の XYZ、Y = 1）を求め、
/// `M = P · diag(S)` とする。RGB = (1, 1, 1) が白の XYZ（Y = 1）になる。
pub const fn rgb_to_xyz_matrix_const(primaries: RgbPrimaries, white: Xy) -> Option<Mat3> {
    let p = [primaries.red, primaries.green, primaries.blue];
    let mut i = 0;
    while i < 3 {
        if !(p[i].x.is_finite() && p[i].y.is_finite() && p[i].y > 0.0) {
            return None;
        }
        i += 1;
    }
    if !(white.x.is_finite() && white.y.is_finite() && white.y > 0.0) {
        return None;
    }
    let pm = Mat3::from_cols([
        xy_to_xyz_const(p[0], 1.0),
        xy_to_xyz_const(p[1], 1.0),
        xy_to_xyz_const(p[2], 1.0),
    ]);
    let Some(inv) = pm.checked_inverse() else {
        return None;
    };
    let s = inv.apply(xy_to_xyz_const(white, 1.0));
    Some(pm.mul_mat(&Mat3::diagonal(s)))
}

/// 原色と白色点から RGB → XYZ の行列を導出する。原色が一直線に並ぶなど導出できなければエラー。
pub fn rgb_to_xyz_matrix(primaries: RgbPrimaries, white: Xy) -> Result<Mat3> {
    // 不正な色度を先に検出して、わかりやすいエラーにする。
    for xy in [primaries.red, primaries.green, primaries.blue, white] {
        xy.to_xyz(1.0)?;
    }
    rgb_to_xyz_matrix_const(primaries, white).ok_or(ColorError::SingularMatrix {
        determinant: Mat3::from_cols([
            xy_to_xyz_const(primaries.red, 1.0),
            xy_to_xyz_const(primaries.green, 1.0),
            xy_to_xyz_const(primaries.blue, 1.0),
        ])
        .determinant(),
    })
}

/// const の文脈で `Option<Mat3>` を取り出す。導出に失敗する定数はコンパイルエラーになる。
const fn expect_const(m: Option<Mat3>) -> Mat3 {
    match m {
        Some(m) => m,
        None => panic!("色空間の行列を導出できない"),
    }
}

/// リニア BT.2020（D65）→ XYZ。
pub const BT2020_TO_XYZ: Mat3 = expect_const(rgb_to_xyz_matrix_const(BT2020_PRIMARIES, D65));
/// XYZ → リニア BT.2020（D65）。
pub const XYZ_TO_BT2020: Mat3 = expect_const(BT2020_TO_XYZ.checked_inverse());
/// リニア sRGB（BT.709、D65）→ XYZ。
pub const SRGB_TO_XYZ: Mat3 = expect_const(rgb_to_xyz_matrix_const(BT709_PRIMARIES, D65));
/// XYZ → リニア sRGB（BT.709、D65）。
pub const XYZ_TO_SRGB: Mat3 = expect_const(SRGB_TO_XYZ.checked_inverse());
/// リニア Display P3（D65）→ XYZ。
pub const DISPLAY_P3_TO_XYZ: Mat3 =
    expect_const(rgb_to_xyz_matrix_const(DISPLAY_P3_PRIMARIES, D65));
/// XYZ → リニア Display P3（D65）。
pub const XYZ_TO_DISPLAY_P3: Mat3 = expect_const(DISPLAY_P3_TO_XYZ.checked_inverse());
/// リニア Adobe RGB (1998)（D65）→ XYZ。
pub const ADOBE_RGB_TO_XYZ: Mat3 = expect_const(rgb_to_xyz_matrix_const(ADOBE_RGB_PRIMARIES, D65));
/// XYZ → リニア Adobe RGB (1998)（D65）。
pub const XYZ_TO_ADOBE_RGB: Mat3 = expect_const(ADOBE_RGB_TO_XYZ.checked_inverse());

/// RGB の色空間（原色・白色点・伝達関数の組）。
///
/// 作業色空間の [`RgbColorSpace::Bt2020`] はリニアで扱い、出力の色空間は 2.6 節の B4b の伝達関数を持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RgbColorSpace {
    /// ITU-R BT.2020 の原色・D65。作業色空間（B2・B3）。伝達関数はリニア。
    Bt2020,
    /// sRGB（BT.709 の原色・D65・IEC 61966-2-1 の伝達関数）。
    Srgb,
    /// Display P3（D65・IEC 61966-2-1 の伝達関数）。
    DisplayP3,
    /// Adobe RGB (1998)（D65・ガンマ 563/256）。
    AdobeRgb1998,
}

impl RgbColorSpace {
    /// すべての色空間。
    pub const ALL: [RgbColorSpace; 4] = [
        RgbColorSpace::Bt2020,
        RgbColorSpace::Srgb,
        RgbColorSpace::DisplayP3,
        RgbColorSpace::AdobeRgb1998,
    ];

    /// 原色。
    pub const fn primaries(self) -> RgbPrimaries {
        match self {
            Self::Bt2020 => BT2020_PRIMARIES,
            Self::Srgb => BT709_PRIMARIES,
            Self::DisplayP3 => DISPLAY_P3_PRIMARIES,
            Self::AdobeRgb1998 => ADOBE_RGB_PRIMARIES,
        }
    }

    /// 白色点（すべて D65）。
    pub const fn white_point(self) -> Xy {
        D65
    }

    /// 伝達関数（2.6 節の B2・B4b）。
    pub const fn transfer_function(self) -> TransferFunction {
        match self {
            Self::Bt2020 => TransferFunction::Linear,
            Self::Srgb | Self::DisplayP3 => TransferFunction::Srgb,
            Self::AdobeRgb1998 => TransferFunction::AdobeRgbGamma,
        }
    }

    /// リニア RGB → XYZ（D65、白の Y = 1）。
    pub const fn to_xyz(self) -> Mat3 {
        match self {
            Self::Bt2020 => BT2020_TO_XYZ,
            Self::Srgb => SRGB_TO_XYZ,
            Self::DisplayP3 => DISPLAY_P3_TO_XYZ,
            Self::AdobeRgb1998 => ADOBE_RGB_TO_XYZ,
        }
    }

    /// XYZ（D65）→ リニア RGB。
    pub const fn from_xyz(self) -> Mat3 {
        match self {
            Self::Bt2020 => XYZ_TO_BT2020,
            Self::Srgb => XYZ_TO_SRGB,
            Self::DisplayP3 => XYZ_TO_DISPLAY_P3,
            Self::AdobeRgb1998 => XYZ_TO_ADOBE_RGB,
        }
    }

    /// 輝度 Y の係数（RGB → XYZ の行列の 2 行目。和は 1）。
    pub const fn luminance_coefficients(self) -> [f64; 3] {
        self.to_xyz().row(1)
    }
}

/// 作業色空間（リニア BT.2020）→ `space` のリニア RGB の行列。
pub const fn working_to(space: RgbColorSpace) -> Mat3 {
    space.from_xyz().mul_mat(&BT2020_TO_XYZ)
}

/// `space` のリニア RGB → 作業色空間（リニア BT.2020）の行列。
pub const fn to_working(space: RgbColorSpace) -> Mat3 {
    XYZ_TO_BT2020.mul_mat(&space.to_xyz())
}

/// Bradford 法の錐体応答の行列（XYZ → 錐体応答に相当する RGB）。
pub const BRADFORD: Mat3 = Mat3::from_rows([
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
]);
const BRADFORD_INV: Mat3 = expect_const(BRADFORD.checked_inverse());

/// Bradford 法の色順応の行列（const fn 版。導出できなければ `None`）。
pub const fn bradford_adaptation_const(src_white: [f64; 3], dst_white: [f64; 3]) -> Option<Mat3> {
    let s = BRADFORD.apply(src_white);
    let d = BRADFORD.apply(dst_white);
    let mut i = 0;
    while i < 3 {
        if !(s[i].is_finite() && d[i].is_finite()) || s[i] == 0.0 {
            return None;
        }
        i += 1;
    }
    let scale = Mat3::diagonal([d[0] / s[0], d[1] / s[1], d[2] / s[2]]);
    Some(BRADFORD_INV.mul_mat(&scale.mul_mat(&BRADFORD)))
}

/// Bradford 法の色順応の行列（XYZ → XYZ）。白 `src_white`（XYZ）を `dst_white`（XYZ）に写す。
///
/// `M = B⁻¹ · diag(B·dst / B·src) · B`（B は [`BRADFORD`]）。
pub fn bradford_adaptation(src_white: [f64; 3], dst_white: [f64; 3]) -> Result<Mat3> {
    if !src_white
        .iter()
        .chain(dst_white.iter())
        .all(|v| v.is_finite())
    {
        return Err(ColorError::NonFinite {
            what: "白色点の XYZ",
        });
    }
    bradford_adaptation_const(src_white, dst_white).ok_or(ColorError::InvalidArgument {
        reason: "白色点の錐体応答が 0 になるため色順応の行列を作れない",
    })
}

/// D65 → D50 の Bradford 法の色順応（白色点は [`D65`]・[`D50`] の xy から求めた XYZ）。
pub const BRADFORD_D65_TO_D50: Mat3 = expect_const(bradford_adaptation_const(
    xy_to_xyz_const(D65, 1.0),
    xy_to_xyz_const(D50, 1.0),
));
/// D50 → D65 の Bradford 法の色順応（白色点は [`D50`]・[`D65`] の xy から求めた XYZ）。
pub const BRADFORD_D50_TO_D65: Mat3 = expect_const(bradford_adaptation_const(
    xy_to_xyz_const(D50, 1.0),
    xy_to_xyz_const(D65, 1.0),
));

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_mat_close(a: &Mat3, b: &Mat3, tol: f64) {
        let d = a.max_abs_diff(b);
        assert!(d <= tol, "差 {d:e} > {tol:e}\n{a:?}\n{b:?}");
    }

    #[test]
    fn srgb_matrix_matches_iec_61966_2_1() {
        // IEC 61966-2-1 の行列（小数 4 桁）。
        let published = Mat3::from_rows([
            [0.4124, 0.3576, 0.1805],
            [0.2126, 0.7152, 0.0722],
            [0.0193, 0.1192, 0.9505],
        ]);
        assert_mat_close(&SRGB_TO_XYZ, &published, 0.5e-4 + 1e-9);
    }

    #[test]
    fn bt2020_luminance_matches_itu_r_bt2020() {
        // BT.2020 の輝度の係数（0.2627, 0.6780, 0.0593）。
        let y = RgbColorSpace::Bt2020.luminance_coefficients();
        assert!((y[0] - 0.2627).abs() < 0.5e-4);
        assert!((y[1] - 0.6780).abs() < 0.5e-4);
        assert!((y[2] - 0.0593).abs() < 0.5e-4);
        // BT.709 の輝度の係数（0.2126, 0.7152, 0.0722）。
        let y = RgbColorSpace::Srgb.luminance_coefficients();
        assert!((y[0] - 0.2126).abs() < 0.5e-4);
        assert!((y[1] - 0.7152).abs() < 0.5e-4);
        assert!((y[2] - 0.0722).abs() < 0.5e-4);
    }

    #[test]
    fn display_p3_and_adobe_rgb_matrices() {
        // Display P3（D65）の RGB → XYZ（よく公表されている値。小数 4 桁）。
        let p3 = Mat3::from_rows([
            [0.4866, 0.2657, 0.1982],
            [0.2290, 0.6917, 0.0793],
            [0.0000, 0.0451, 1.0439],
        ]);
        assert_mat_close(&DISPLAY_P3_TO_XYZ, &p3, 0.5e-4 + 1e-9);
        // Adobe RGB (1998) の仕様の行列（小数 5 桁）。仕様は白を XYZ (0.95047, 1, 1.08883) としており、
        // xy (0.3127, 0.3290) から求めた白とは 1e-4 程度ずれるため、許容差を広げる。
        let adobe = Mat3::from_rows([
            [0.57667, 0.18556, 0.18823],
            [0.29734, 0.62736, 0.07529],
            [0.02703, 0.07069, 0.99134],
        ]);
        assert_mat_close(&ADOBE_RGB_TO_XYZ, &adobe, 2e-4);
    }

    #[test]
    fn bt709_to_bt2020_matches_itu_r_bt2087() {
        // ITU-R BT.2087 の BT.709 → BT.2020 の行列（小数 4 桁）。
        let published = Mat3::from_rows([
            [0.6274, 0.3293, 0.0433],
            [0.0691, 0.9195, 0.0114],
            [0.0164, 0.0880, 0.8956],
        ]);
        assert_mat_close(&to_working(RgbColorSpace::Srgb), &published, 0.5e-4 + 1e-9);
    }

    #[test]
    fn white_maps_to_white_and_y_is_one() {
        for space in RgbColorSpace::ALL {
            let xyz = space.to_xyz().apply([1.0, 1.0, 1.0]);
            let w = D65.to_xyz(1.0).unwrap();
            for k in 0..3 {
                assert!((xyz[k] - w[k]).abs() < 1e-12, "{space:?}");
            }
            let l = space.luminance_coefficients();
            assert!((l[0] + l[1] + l[2] - 1.0).abs() < 1e-12);
            // 作業色空間との間でも白は白。
            let rgb = working_to(space).apply([1.0, 1.0, 1.0]);
            for v in rgb {
                assert!((v - 1.0).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn working_conversions_are_inverse() {
        for space in RgbColorSpace::ALL {
            let p = working_to(space) * to_working(space);
            assert_mat_close(&p, &Mat3::IDENTITY, 1e-13);
            let q = space.from_xyz() * space.to_xyz();
            assert_mat_close(&q, &Mat3::IDENTITY, 1e-13);
        }
        assert_mat_close(&working_to(RgbColorSpace::Bt2020), &Mat3::IDENTITY, 1e-15);
    }

    #[test]
    fn primaries_have_expected_chromaticity() {
        // RGB の (1,0,0) を XYZ にすると、原色の xy に戻る。
        for space in RgbColorSpace::ALL {
            let m = space.to_xyz();
            let p = space.primaries();
            for (k, xy) in [p.red, p.green, p.blue].into_iter().enumerate() {
                let mut v = [0.0; 3];
                v[k] = 1.0;
                let c = Xy::from_xyz(m.apply(v)).unwrap();
                assert!((c.x - xy.x).abs() < 1e-12 && (c.y - xy.y).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn bradford_matches_lindbloom_published() {
        // Bruce Lindbloom が公表している Bradford 法の行列（白: D65 = (0.95047, 1, 1.08883)、
        // D50 = (0.96422, 1, 0.82521)。小数 7 桁）。
        let d65 = [0.95047, 1.0, 1.08883];
        let d50 = [0.96422, 1.0, 0.82521];
        let to50 = bradford_adaptation(d65, d50).unwrap();
        let published_to50 = Mat3::from_rows([
            [1.0478112, 0.0228866, -0.0501270],
            [0.0295424, 0.9904844, -0.0170491],
            [-0.0092345, 0.0150436, 0.7521316],
        ]);
        assert_mat_close(&to50, &published_to50, 0.5e-7 + 1e-10);
        let to65 = bradford_adaptation(d50, d65).unwrap();
        let published_to65 = Mat3::from_rows([
            [0.9555766, -0.0230393, 0.0631636],
            [-0.0282895, 1.0099416, 0.0210077],
            [0.0122982, -0.0204830, 1.3299098],
        ]);
        assert_mat_close(&to65, &published_to65, 0.5e-7 + 1e-10);
    }

    #[test]
    fn bradford_d50_d65_constants() {
        // 白を白に写す。
        let w65 = D65.to_xyz(1.0).unwrap();
        let w50 = D50.to_xyz(1.0).unwrap();
        let a = BRADFORD_D65_TO_D50.apply(w65);
        let b = BRADFORD_D50_TO_D65.apply(w50);
        for k in 0..3 {
            assert!((a[k] - w50[k]).abs() < 1e-12);
            assert!((b[k] - w65[k]).abs() < 1e-12);
        }
        // 互いに逆。
        assert_mat_close(
            &(BRADFORD_D50_TO_D65 * BRADFORD_D65_TO_D50),
            &Mat3::IDENTITY,
            1e-13,
        );
        // 同じ白どうしなら単位行列。
        assert_mat_close(
            &bradford_adaptation(w65, w65).unwrap(),
            &Mat3::IDENTITY,
            1e-14,
        );
        // xy から求めた白でも、公表値（白の定義が違う）と 3e-4 以内。
        let published_to50 = Mat3::from_rows([
            [1.0478112, 0.0228866, -0.0501270],
            [0.0295424, 0.9904844, -0.0170491],
            [-0.0092345, 0.0150436, 0.7521316],
        ]);
        assert_mat_close(&BRADFORD_D65_TO_D50, &published_to50, 3e-4);
    }

    #[test]
    fn bradford_rejects_invalid_white() {
        assert!(bradford_adaptation([f64::NAN, 1.0, 1.0], [1.0, 1.0, 1.0]).is_err());
        assert!(bradford_adaptation([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]).is_err());
    }

    #[test]
    fn invalid_primaries_are_error() {
        // y = 0 の原色。
        let bad = RgbPrimaries {
            red: Xy::new(0.7, 0.0),
            ..BT709_PRIMARIES
        };
        assert!(matches!(
            rgb_to_xyz_matrix(bad, D65),
            Err(ColorError::InvalidChromaticity { .. })
        ));
        // 3 原色が一直線上（行列が特異）。
        let collinear = RgbPrimaries {
            red: Xy::new(0.6, 0.3),
            green: Xy::new(0.4, 0.3),
            blue: Xy::new(0.2, 0.3),
        };
        assert!(matches!(
            rgb_to_xyz_matrix(collinear, D65),
            Err(ColorError::SingularMatrix { .. })
        ));
        assert!(rgb_to_xyz_matrix(BT2020_PRIMARIES, Xy::new(f64::NAN, 0.3)).is_err());
        // 正常なら定数と一致する。
        assert_eq!(
            rgb_to_xyz_matrix(BT2020_PRIMARIES, D65).unwrap(),
            BT2020_TO_XYZ
        );
    }

    #[test]
    fn xy_uv_roundtrip() {
        for xy in [D65, D50, Xy::new(0.4476, 0.4074), Xy::new(0.25, 0.25)] {
            let back = xy.to_uv().to_xy();
            assert!((back.x - xy.x).abs() < 1e-15 && (back.y - xy.y).abs() < 1e-15);
        }
        // D65 の uv（CIE 1960）は約 (0.1978, 0.3122)。
        let uv = D65.to_uv();
        assert!((uv.u - 0.1978).abs() < 1e-4 && (uv.v - 0.3122).abs() < 1e-4);
        assert!(Xy::from_xyz([0.0, 0.0, 0.0]).is_err());
        assert!(Xy::new(0.3, 0.0).to_xyz(1.0).is_err());
    }
}
