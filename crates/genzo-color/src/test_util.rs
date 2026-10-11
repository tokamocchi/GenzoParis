//! テスト用の共通の関数。

use lcms2::{
    CIExyY, CIExyYTRIPLE, GlobalContext, Intent, PixelFormat, Profile, ToneCurve, Transform,
};

use crate::icc::IccProfile;
use crate::lab::Lab;
use crate::space::{D65, RgbPrimaries};
use crate::transfer::srgb_decode;

/// ICC の PCS の白（D50 = XYZ (0.9642, 1, 0.8249)）の xyY。
pub(crate) fn icc_d50_xyy() -> CIExyY {
    let sum = 0.9642 + 1.0 + 0.8249;
    CIExyY {
        x: 0.9642 / sum,
        y: 1.0 / sum,
        Y: 1.0,
    }
}

/// lcms2 の Lab（D50）のプロファイル。
pub(crate) fn lab_d50_profile() -> Profile {
    Profile::new_lab4_context(GlobalContext::new(), &icc_d50_xyy()).unwrap()
}

/// RGB の値を、そのプロファイルの定義で Lab（D50）にする（相対的な色域を維持）。
pub(crate) fn rgb_to_lab(profile: &IccProfile, rgb: &[[f32; 3]]) -> Vec<Lab> {
    let lab = lab_d50_profile();
    let t: Transform<[f32; 3], [f32; 3]> = Transform::new(
        &profile.to_lcms().unwrap(),
        PixelFormat::RGB_FLT,
        &lab,
        PixelFormat::Lab_FLT,
        Intent::RelativeColorimetric,
    )
    .unwrap();
    let mut out = vec![[0.0_f32; 3]; rgb.len()];
    t.transform_pixels(rgb, &mut out);
    out.iter()
        .map(|v| Lab::new(f64::from(v[0]), f64::from(v[1]), f64::from(v[2])))
        .collect()
}

/// IEC 61966-2-1 の伝達関数を `points` 点の 16bit の表（'curv'）にした、ICC v2.1 の RGB の
/// matrix-shaper のプロファイル（白は D65）。
///
/// Windows の既定の「sRGB IEC61966-2.1」（v2.1・1024 点の 'curv' とされる。**実物とは未照合**）に
/// 似せたもの。実物は手元にないため、点数と版だけを合わせている。
pub(crate) fn tabulated_srgb_transfer_profile(
    primaries: RgbPrimaries,
    points: usize,
) -> IccProfile {
    let xyy = |x: f64, y: f64| CIExyY { x, y, Y: 1.0 };
    let white = xyy(D65.x, D65.y);
    let prim = CIExyYTRIPLE {
        Red: xyy(primaries.red.x, primaries.red.y),
        Green: xyy(primaries.green.x, primaries.green.y),
        Blue: xyy(primaries.blue.x, primaries.blue.y),
    };
    let n1 = (points - 1) as f64;
    let table: Vec<u16> = (0..points)
        .map(|i| (srgb_decode(i as f64 / n1) * 65535.0).round() as u16)
        .collect();
    let curve = ToneCurve::new_tabulated(&table);
    let mut p = Profile::new_rgb(&white, &prim, &[&curve, &curve, &curve]).unwrap();
    p.set_version(2.1);
    IccProfile::from_bytes(&p.icc().unwrap()).unwrap()
}
