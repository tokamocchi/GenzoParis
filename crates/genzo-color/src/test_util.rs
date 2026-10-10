//! テスト用の共通の関数。

use lcms2::{CIExyY, GlobalContext, Intent, PixelFormat, Profile, Transform};

use crate::icc::IccProfile;
use crate::lab::Lab;

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
