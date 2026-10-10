//! 伝達関数（リニアな値 ⇔ 符号化した値）。
//!
//! - IEC 61966-2-1（sRGB）の区分関数。sRGB・Display P3 の書き出し（B4b）、キャッシュ（B5）、
//!   トーンカーブの横軸（ステージ 16）、画面用 3D LUT の入力（ステージ 17a）で使う（04 の 2.6 節）。
//! - Adobe RGB (1998) のガンマ 563/256（≒ 2.2）。
//!
//! **負の値の扱い**: どの関数も、負の値には符号を保ったまま |x| に適用する
//! （`f(-x) = -f(x)`）。作業色空間・ディスプレイ参照（B2・B3）では色域外の色を負の値として
//! 保持するため（IQ-02）、符号化しても情報が失われないようにする。0〜1 に収めるのは
//! 色域の圧縮（[`crate::gamut`]）の役割で、ここでは切り捨てない。
//!
//! NaN を渡すと NaN を返す（NaN の検出と 0 への置き換えは各ステージの出力で行う。2.6 節）。
//!
//! f32 版は f32 のまま計算する（CPU 基準実装と GPU 版で同じ精度にするため。2.3 節）。

/// IEC 61966-2-1 の復号（符号化値 → リニア）で、線形の区間と冪の区間を分ける値。
pub const SRGB_DECODE_THRESHOLD: f64 = 0.04045;
/// IEC 61966-2-1 の符号化（リニア → 符号化値）で、線形の区間と冪の区間を分ける値。
pub const SRGB_ENCODE_THRESHOLD: f64 = 0.0031308;
/// IEC 61966-2-1 の線形の区間の傾き。
pub const SRGB_LINEAR_SLOPE: f64 = 12.92;
/// IEC 61966-2-1 の冪の区間のオフセット。
pub const SRGB_OFFSET: f64 = 0.055;
/// IEC 61966-2-1 の冪の区間の指数。
pub const SRGB_GAMMA: f64 = 2.4;
/// Adobe RGB (1998) のガンマ（563/256 = 2.19921875。仕様の値）。
pub const ADOBE_RGB_GAMMA: f64 = 563.0 / 256.0;

const SRGB_DECODE_THRESHOLD_F32: f32 = SRGB_DECODE_THRESHOLD as f32;
const SRGB_ENCODE_THRESHOLD_F32: f32 = SRGB_ENCODE_THRESHOLD as f32;
const SRGB_LINEAR_SLOPE_F32: f32 = SRGB_LINEAR_SLOPE as f32;
const SRGB_OFFSET_F32: f32 = SRGB_OFFSET as f32;
const SRGB_GAMMA_F32: f32 = SRGB_GAMMA as f32;
const SRGB_INV_GAMMA_F32: f32 = (1.0 / SRGB_GAMMA) as f32;
const ADOBE_RGB_GAMMA_F32: f32 = ADOBE_RGB_GAMMA as f32;
const ADOBE_RGB_INV_GAMMA_F32: f32 = (256.0 / 563.0) as f32;

/// 伝達関数の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransferFunction {
    /// リニア（変換しない）。作業色空間（B2・B3）とリニア BT.2020 のプロファイル。
    Linear,
    /// IEC 61966-2-1 の区分関数（sRGB・Display P3）。
    Srgb,
    /// Adobe RGB (1998) のガンマ 563/256。
    AdobeRgbGamma,
}

impl TransferFunction {
    /// リニア → 符号化値（f64）。負の値は符号を保つ。
    pub fn encode(self, linear: f64) -> f64 {
        match self {
            Self::Linear => linear,
            Self::Srgb => srgb_encode(linear),
            Self::AdobeRgbGamma => adobe_rgb_encode(linear),
        }
    }

    /// 符号化値 → リニア（f64）。負の値は符号を保つ。
    pub fn decode(self, encoded: f64) -> f64 {
        match self {
            Self::Linear => encoded,
            Self::Srgb => srgb_decode(encoded),
            Self::AdobeRgbGamma => adobe_rgb_decode(encoded),
        }
    }

    /// リニア → 符号化値（f32）。負の値は符号を保つ。
    pub fn encode_f32(self, linear: f32) -> f32 {
        match self {
            Self::Linear => linear,
            Self::Srgb => srgb_encode_f32(linear),
            Self::AdobeRgbGamma => adobe_rgb_encode_f32(linear),
        }
    }

    /// 符号化値 → リニア（f32）。負の値は符号を保つ。
    pub fn decode_f32(self, encoded: f32) -> f32 {
        match self {
            Self::Linear => encoded,
            Self::Srgb => srgb_decode_f32(encoded),
            Self::AdobeRgbGamma => adobe_rgb_decode_f32(encoded),
        }
    }
}

/// IEC 61966-2-1: リニア → 符号化値（f64）。
///
/// `|x| ≤ 0.0031308` なら `12.92·x`、それ以外は `1.055·|x|^(1/2.4) − 0.055`（符号は x に合わせる）。
pub fn srgb_encode(linear: f64) -> f64 {
    let a = linear.abs();
    let e = if a <= SRGB_ENCODE_THRESHOLD {
        SRGB_LINEAR_SLOPE * a
    } else {
        (1.0 + SRGB_OFFSET) * a.powf(1.0 / SRGB_GAMMA) - SRGB_OFFSET
    };
    e.copysign(linear)
}

/// IEC 61966-2-1: 符号化値 → リニア（f64）。
///
/// `|v| ≤ 0.04045` なら `v / 12.92`、それ以外は `((|v| + 0.055) / 1.055)^2.4`（符号は v に合わせる）。
pub fn srgb_decode(encoded: f64) -> f64 {
    let a = encoded.abs();
    let l = if a <= SRGB_DECODE_THRESHOLD {
        a / SRGB_LINEAR_SLOPE
    } else {
        ((a + SRGB_OFFSET) / (1.0 + SRGB_OFFSET)).powf(SRGB_GAMMA)
    };
    l.copysign(encoded)
}

/// IEC 61966-2-1: リニア → 符号化値（f32）。式は [`srgb_encode`] と同じ。
#[inline]
pub fn srgb_encode_f32(linear: f32) -> f32 {
    let a = linear.abs();
    let e = if a <= SRGB_ENCODE_THRESHOLD_F32 {
        SRGB_LINEAR_SLOPE_F32 * a
    } else {
        (1.0 + SRGB_OFFSET_F32) * a.powf(SRGB_INV_GAMMA_F32) - SRGB_OFFSET_F32
    };
    e.copysign(linear)
}

/// IEC 61966-2-1: 符号化値 → リニア（f32）。式は [`srgb_decode`] と同じ。
#[inline]
pub fn srgb_decode_f32(encoded: f32) -> f32 {
    let a = encoded.abs();
    let l = if a <= SRGB_DECODE_THRESHOLD_F32 {
        a / SRGB_LINEAR_SLOPE_F32
    } else {
        ((a + SRGB_OFFSET_F32) / (1.0 + SRGB_OFFSET_F32)).powf(SRGB_GAMMA_F32)
    };
    l.copysign(encoded)
}

/// Adobe RGB (1998): リニア → 符号化値（f64）。`|x|^(256/563)`（符号は x に合わせる）。
pub fn adobe_rgb_encode(linear: f64) -> f64 {
    linear.abs().powf(1.0 / ADOBE_RGB_GAMMA).copysign(linear)
}

/// Adobe RGB (1998): 符号化値 → リニア（f64）。`|v|^(563/256)`（符号は v に合わせる）。
pub fn adobe_rgb_decode(encoded: f64) -> f64 {
    encoded.abs().powf(ADOBE_RGB_GAMMA).copysign(encoded)
}

/// Adobe RGB (1998): リニア → 符号化値（f32）。
#[inline]
pub fn adobe_rgb_encode_f32(linear: f32) -> f32 {
    linear.abs().powf(ADOBE_RGB_INV_GAMMA_F32).copysign(linear)
}

/// Adobe RGB (1998): 符号化値 → リニア（f32）。
#[inline]
pub fn adobe_rgb_decode_f32(encoded: f32) -> f32 {
    encoded.abs().powf(ADOBE_RGB_GAMMA_F32).copysign(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [TransferFunction; 3] = [
        TransferFunction::Linear,
        TransferFunction::Srgb,
        TransferFunction::AdobeRgbGamma,
    ];

    #[test]
    fn srgb_known_values() {
        // 端点。
        assert_eq!(srgb_encode(0.0), 0.0);
        assert!((srgb_encode(1.0) - 1.0).abs() < 1e-15);
        assert!((srgb_decode(1.0) - 1.0).abs() < 1e-15);
        // 線形の区間: 0.04045 / 12.92 = 0.0031308049...
        assert!((srgb_decode(0.04045) - 0.04045 / 12.92).abs() < 1e-18);
        assert!((srgb_encode(0.002) - 0.02584).abs() < 1e-15);
        // 冪の区間: リニア 0.18 は符号化値 約 0.4614（中間のグレー）。
        assert!((srgb_encode(0.18) - 0.461_356).abs() < 1e-6);
        // 符号化値 0.5 はリニア 約 0.214041。
        assert!((srgb_decode(0.5) - 0.214_041).abs() < 1e-6);
        // 8bit の 128/255 はリニア 約 0.215861。
        assert!((srgb_decode(128.0 / 255.0) - 0.215_861).abs() < 1e-6);
        // リニア 0.5 は符号化値 約 0.735357（8bit で 188）。
        assert!((srgb_encode(0.5) - 0.735_357).abs() < 1e-6);
        assert_eq!((srgb_encode(0.5) * 255.0).round(), 188.0);
        // 8bit の 1/255 は線形の区間（1/255 ≤ 0.04045）: 1 / (255 · 12.92)。
        assert!((srgb_decode(1.0 / 255.0) - 1.0 / (255.0 * 12.92)).abs() < 1e-15);
    }

    #[test]
    fn srgb_is_nearly_continuous_at_thresholds() {
        // IEC 61966-2-1 の閾値では、2 つの区間の値の差は 1e-7 未満（規格の丸めによる小さな段差）。
        let lin = SRGB_DECODE_THRESHOLD / SRGB_LINEAR_SLOPE;
        let pow = ((SRGB_DECODE_THRESHOLD + SRGB_OFFSET) / (1.0 + SRGB_OFFSET)).powf(SRGB_GAMMA);
        assert!((lin - pow).abs() < 1e-7);
        let lin_e = SRGB_LINEAR_SLOPE * SRGB_ENCODE_THRESHOLD;
        let pow_e =
            (1.0 + SRGB_OFFSET) * SRGB_ENCODE_THRESHOLD.powf(1.0 / SRGB_GAMMA) - SRGB_OFFSET;
        assert!((lin_e - pow_e).abs() < 1e-7);
    }

    #[test]
    fn adobe_rgb_gamma_value() {
        assert_eq!(ADOBE_RGB_GAMMA, 2.199_218_75);
        assert!((adobe_rgb_decode(0.5) - 0.5_f64.powf(2.199_218_75)).abs() < 1e-15);
        assert!((adobe_rgb_encode(1.0) - 1.0).abs() < 1e-15);
        assert_eq!(adobe_rgb_encode(0.0), 0.0);
    }

    #[test]
    fn roundtrip_f64_including_negative_and_above_one() {
        for tf in ALL {
            let mut x = -4.0;
            while x <= 4.0 {
                let back = tf.decode(tf.encode(x));
                assert!(
                    (back - x).abs() <= 1e-12 * x.abs().max(1.0),
                    "{tf:?}: {x} → {back}"
                );
                x += 0.0137;
            }
        }
    }

    #[test]
    fn roundtrip_f32() {
        for tf in ALL {
            let mut x = -2.0_f32;
            while x <= 2.0 {
                let back = tf.decode_f32(tf.encode_f32(x));
                assert!(
                    (back - x).abs() <= 2e-6 * x.abs().max(1e-3),
                    "{tf:?}: {x} → {back}"
                );
                x += 0.0071;
            }
        }
    }

    #[test]
    fn odd_symmetry_preserves_sign() {
        for tf in ALL {
            for x in [1e-6, 0.001, 0.003, 0.01, 0.2, 0.9, 1.5, 10.0] {
                assert_eq!(tf.encode(-x), -tf.encode(x));
                assert_eq!(tf.decode(-x), -tf.decode(x));
                let xf = x as f32;
                assert_eq!(tf.encode_f32(-xf), -tf.encode_f32(xf));
                assert_eq!(tf.decode_f32(-xf), -tf.decode_f32(xf));
            }
        }
        // -0.0 は -0.0 のまま（符号を保つ）。
        assert!(srgb_encode(-0.0).is_sign_negative());
    }

    #[test]
    fn monotonic() {
        for tf in ALL {
            let mut prev_e = f64::NEG_INFINITY;
            let mut prev_d = f64::NEG_INFINITY;
            let mut x = -1.5;
            while x <= 1.5 {
                let e = tf.encode(x);
                let d = tf.decode(x);
                assert!(e >= prev_e, "{tf:?} encode not monotonic at {x}");
                assert!(d >= prev_d, "{tf:?} decode not monotonic at {x}");
                prev_e = e;
                prev_d = d;
                x += 1e-4;
            }
        }
    }

    #[test]
    fn f32_matches_f64() {
        for tf in ALL {
            let mut x = -1.2_f64;
            while x <= 1.2 {
                let e64 = tf.encode(x);
                let e32 = tf.encode_f32(x as f32) as f64;
                assert!((e64 - e32).abs() < 1e-6, "{tf:?} encode {x}");
                let d64 = tf.decode(x);
                let d32 = tf.decode_f32(x as f32) as f64;
                assert!((d64 - d32).abs() < 1e-6, "{tf:?} decode {x}");
                x += 0.001;
            }
        }
    }

    #[test]
    fn nan_propagates() {
        for tf in [TransferFunction::Srgb, TransferFunction::AdobeRgbGamma] {
            assert!(tf.encode(f64::NAN).is_nan());
            assert!(tf.decode_f32(f32::NAN).is_nan());
        }
    }
}
