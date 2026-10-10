//! ディスプレイ（モニター）のプロファイルと、画面への出力の準備（04 の 5 章・2.6 節のステージ 17a、
//! IQ-05、CLR-01）。
//!
//! モニターの ICC プロファイルの取得は OS ごとに方法が違い、PoC-1 で実装する。ここでは取得した
//! 結果（バイト列、または取得できなかったこと）を受け取り、**取得できない・使えない場合は sRGB と
//! みなす**（IQ-05）。sRGB とみなしたことは [`DisplayProfile::is_assumed_srgb`] でわかるので、
//! UI はその旨を表示する。
//!
//! 画面への出力の手順（2.6 節 17a）:
//! 1. B3（リニア BT.2020）→ モニターの色域へ色域の圧縮（[`DisplayGamut::compress_working`]）。
//! 2. IEC 61966-2-1 の伝達関数で符号化（[`crate::transfer::srgb_encode_f32`]）。
//! 3. 3D LUT を適用（[`DisplayProfile::build_lut`] で作った [`Lut3d`]）。

use crate::error::{ColorError, Result};
use crate::gamut::{DEFAULT_GAMUT_POWER, DEFAULT_GAMUT_THRESHOLD, GamutCompressor};
use crate::icc::{IccProfile, IccTransform, RenderingIntent, StandardProfile};
use crate::lut::Lut3d;
use crate::matrix::{Mat3, Mat3F32};
use crate::space::{BT2020_TO_XYZ, RgbColorSpace, working_to};

/// sRGB とみなした理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayProfileFallbackReason {
    /// OS から取得できなかった（取得の失敗、未対応の環境、プロファイルが設定されていない）。
    NotAvailable,
    /// 取得したバイト列を ICC プロファイルとして読み込めなかった。
    Invalid(String),
    /// 読み込めたが、画面用には使えない（RGB でない、変換を作れないなど）。
    Unsupported(String),
}

/// 画面に使うプロファイルの出どころ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayProfileSource {
    /// OS から取得したモニターのプロファイル。
    Os,
    /// sRGB とみなした（IQ-05。UI にその旨を表示する）。
    AssumedSrgb(DisplayProfileFallbackReason),
}

/// 画面用 3D LUT の変換元（表示する画像の色の定義）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisplayLutSource {
    /// 現像結果（B3 を色域の圧縮の後に IEC 61966-2-1 で符号化した BT.2020）。
    /// 変換元は [`StandardProfile::Bt2020SrgbTransfer`]。
    Working,
    /// キャッシュ（B5、Display P3）。変換元は [`StandardProfile::DisplayP3`]。
    CacheDisplayP3,
}

impl DisplayLutSource {
    /// 変換元のプロファイルの種類。
    pub const fn standard_profile(self) -> StandardProfile {
        match self {
            Self::Working => StandardProfile::Bt2020SrgbTransfer,
            Self::CacheDisplayP3 => StandardProfile::DisplayP3,
        }
    }
}

/// 画面に使うプロファイル。
#[derive(Debug, Clone)]
pub struct DisplayProfile {
    profile: IccProfile,
    source: DisplayProfileSource,
}

impl DisplayProfile {
    /// sRGB とみなしたプロファイル。
    pub fn assumed_srgb(reason: DisplayProfileFallbackReason) -> Result<Self> {
        Ok(Self {
            profile: IccProfile::standard(StandardProfile::Srgb)?,
            source: DisplayProfileSource::AssumedSrgb(reason),
        })
    }

    /// OS から取得した結果から、画面に使うプロファイルを決める。
    ///
    /// `os_icc` が `None`（取得できない）、読み込めない、RGB でない、作業色空間からの変換を作れない
    /// 場合は sRGB とみなす。エラーになるのは、sRGB のプロファイル自体を作れない場合だけ。
    pub fn resolve(os_icc: Option<&[u8]>) -> Result<Self> {
        let Some(bytes) = os_icc else {
            return Self::assumed_srgb(DisplayProfileFallbackReason::NotAvailable);
        };
        let profile = match IccProfile::from_bytes(bytes) {
            Ok(p) => p,
            Err(e) => {
                return Self::assumed_srgb(DisplayProfileFallbackReason::Invalid(e.to_string()));
            }
        };
        if let Err(e) = Self::check_usable(&profile) {
            return Self::assumed_srgb(DisplayProfileFallbackReason::Unsupported(e.to_string()));
        }
        Ok(Self {
            profile,
            source: DisplayProfileSource::Os,
        })
    }

    /// 画面用に使えるか（RGB で、作業色空間のプロファイルからの変換を作れるか）。
    fn check_usable(profile: &IccProfile) -> Result<()> {
        if !profile.is_rgb() {
            return Err(ColorError::IccUnsupported {
                reason: "モニターのプロファイルの色空間が RGB でない".to_owned(),
            });
        }
        let src = IccProfile::standard(DisplayLutSource::Working.standard_profile())?;
        IccTransform::new(&src, profile, RenderingIntent::RelativeColorimetric).map(|_| ())
    }

    /// プロファイル。
    pub fn profile(&self) -> &IccProfile {
        &self.profile
    }

    /// 出どころ。
    pub fn source(&self) -> &DisplayProfileSource {
        &self.source
    }

    /// sRGB とみなしたか（UI に表示する。IQ-05）。
    pub fn is_assumed_srgb(&self) -> bool {
        matches!(self.source, DisplayProfileSource::AssumedSrgb(_))
    }

    /// 画面用の 3D LUT を作る（相対的な色域を維持。格子数は通常 [`crate::lut::DEFAULT_LUT_SIZE`]）。
    pub fn build_lut(&self, source: DisplayLutSource, size: usize) -> Result<Lut3d> {
        let src = IccProfile::standard(source.standard_profile())?;
        Lut3d::from_icc(&src, &self.profile, size)
    }

    /// モニターの色域への色域の圧縮。
    ///
    /// matrix-shaper のプロファイルならその原色を使う。LUT で表されたプロファイルなど、原色を
    /// 行列として取り出せない場合は `None`（そのときの扱いは PoC-1 で決める。暫定としては
    /// `DisplayGamut::for_space(RgbColorSpace::Srgb)` を使う）。
    pub fn gamut(&self) -> Option<DisplayGamut> {
        let m = self.profile.working_to_linear_rgb_matrix()?;
        DisplayGamut::from_working_to_display(&m, DEFAULT_GAMUT_THRESHOLD, DEFAULT_GAMUT_POWER).ok()
    }
}

/// 表示先の色域への色域の圧縮（作業色空間のまま入出力する）。
///
/// 作業色空間（リニア BT.2020）→ 表示先のリニア RGB に移し、[`GamutCompressor`] で圧縮して、
/// 作業色空間に戻す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayGamut {
    working_to_display: Mat3F32,
    display_to_working: Mat3F32,
    compressor: GamutCompressor,
}

impl DisplayGamut {
    /// 標準の色空間を表示先とする（膝とべき乗は既定値。仮置き）。
    pub fn for_space(space: RgbColorSpace) -> Self {
        let to = working_to(space);
        Self {
            working_to_display: to.to_f32(),
            display_to_working: crate::space::to_working(space).to_f32(),
            compressor: GamutCompressor::for_space(space),
        }
    }

    /// 作業色空間 → 表示先のリニア RGB の行列から作る。輝度の係数は、この行列から求めた
    /// 表示先の RGB → XYZ の Y の行。
    pub fn from_working_to_display(
        working_to_display: &Mat3,
        threshold: f32,
        power: f32,
    ) -> Result<Self> {
        let back = working_to_display.inverse()?;
        let luminance = (BT2020_TO_XYZ * back).row(1);
        Ok(Self {
            working_to_display: working_to_display.to_f32(),
            display_to_working: back.to_f32(),
            compressor: GamutCompressor::new(luminance, threshold, power)?,
        })
    }

    /// 圧縮に使う [`GamutCompressor`]。
    pub fn compressor(&self) -> &GamutCompressor {
        &self.compressor
    }

    /// 作業色空間の値（B3）を、表示先の色域に収めて作業色空間で返す。
    #[inline]
    pub fn compress_working(&self, rgb: [f32; 3]) -> [f32; 3] {
        let d = self.working_to_display.apply(rgb);
        let c = self.compressor.compress(d);
        self.display_to_working.apply(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lut::DEFAULT_LUT_SIZE;
    use crate::test_util::lab_d50_profile;

    #[test]
    fn missing_profile_is_assumed_srgb() {
        let d = DisplayProfile::resolve(None).unwrap();
        assert!(d.is_assumed_srgb());
        assert_eq!(
            d.source(),
            &DisplayProfileSource::AssumedSrgb(DisplayProfileFallbackReason::NotAvailable)
        );
        assert_eq!(
            d.profile(),
            &IccProfile::standard(StandardProfile::Srgb).unwrap()
        );
    }

    #[test]
    fn invalid_profile_is_assumed_srgb() {
        let d = DisplayProfile::resolve(Some(b"not an icc profile")).unwrap();
        assert!(d.is_assumed_srgb());
        assert!(matches!(
            d.source(),
            DisplayProfileSource::AssumedSrgb(DisplayProfileFallbackReason::Invalid(_))
        ));
    }

    #[test]
    fn non_rgb_profile_is_assumed_srgb() {
        let lab = lab_d50_profile().icc().unwrap();
        let d = DisplayProfile::resolve(Some(&lab)).unwrap();
        assert!(matches!(
            d.source(),
            DisplayProfileSource::AssumedSrgb(DisplayProfileFallbackReason::Unsupported(_))
        ));
    }

    #[test]
    fn valid_profile_is_used() {
        let p3 = IccProfile::standard(StandardProfile::DisplayP3).unwrap();
        let d = DisplayProfile::resolve(Some(p3.as_bytes())).unwrap();
        assert!(!d.is_assumed_srgb());
        assert_eq!(d.source(), &DisplayProfileSource::Os);
        assert_eq!(d.profile(), &p3);
        for source in [DisplayLutSource::Working, DisplayLutSource::CacheDisplayP3] {
            let lut = d.build_lut(source, DEFAULT_LUT_SIZE).unwrap();
            assert_eq!(lut.size(), DEFAULT_LUT_SIZE);
        }
        // P3 のキャッシュを P3 のモニターに表示する LUT は恒等。
        let lut = d
            .build_lut(DisplayLutSource::CacheDisplayP3, DEFAULT_LUT_SIZE)
            .unwrap();
        let o = lut.apply([0.25, 0.5, 0.75]);
        for (a, b) in o.iter().zip([0.25, 0.5, 0.75]) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn display_gamut_brings_bt2020_colors_inside_monitor_gamut() {
        let p3 = IccProfile::standard(StandardProfile::DisplayP3).unwrap();
        let d = DisplayProfile::resolve(Some(p3.as_bytes())).unwrap();
        let g = d.gamut().unwrap();
        let to_p3 = working_to(RgbColorSpace::DisplayP3).to_f32();
        for rgb in [
            [1.0_f32, 0.0, 0.0],
            [0.0, 0.8, 0.0],
            [0.0, 0.0, 0.9],
            [0.1, 0.6, 0.6],
        ] {
            let c = g.compress_working(rgb);
            let p = to_p3.apply(c);
            // ICC の行列の量子化の分だけ、わずかに外れることを許す。
            assert!(
                p.iter().all(|v| (-1e-3..=1.0 + 1e-3).contains(v)),
                "{rgb:?} → {p:?}"
            );
        }
        // 膝の内側の色は（行列の往復の誤差を除いて）変わらない。
        let gray = g.compress_working([0.3, 0.3, 0.3]);
        for v in gray {
            assert!((v - 0.3).abs() < 1e-5);
        }
        // 標準の色空間を表示先にした場合とほぼ同じ。
        let s = DisplayGamut::for_space(RgbColorSpace::DisplayP3);
        let a = s.compress_working([0.0, 0.8, 0.0]);
        let b = g.compress_working([0.0, 0.8, 0.0]);
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 2e-3, "{a:?} vs {b:?}");
        }
        assert_eq!(s.compressor().threshold(), DEFAULT_GAMUT_THRESHOLD);
    }

    #[test]
    fn display_gamut_rejects_singular_matrix() {
        let singular = Mat3::from_rows([[1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!(DisplayGamut::from_working_to_display(&singular, 0.8, 1.2).is_err());
    }

    #[test]
    fn assumed_srgb_has_gamut() {
        let d = DisplayProfile::resolve(None).unwrap();
        assert!(d.gamut().is_some());
    }
}
