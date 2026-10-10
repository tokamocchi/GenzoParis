//! CIELAB（CIE 1976 L\*a\*b\*）と色差（ΔE76・ΔE2000）。
//!
//! 色の正確さの評価（PoC-4・IQ-04）、解像度の違う結果の比較（IQ-07b）、3D LUT の検証
//! （PoC-1 の合格基準）に使う。評価用なので f64 で計算する。
//!
//! - XYZ ⇔ Lab の式は CIE 15:2004 の定義で、閾値は CIE の推奨どおり有理数の
//!   ε = 216/24389、κ = 24389/27 を使う。
//! - ΔE2000 は CIE 142-2001 の式を、Sharma, Wu, Dalal (2005)
//!   「The CIEDE2000 Color-Difference Formula: Implementation Notes, Supplementary Test Data,
//!   and Mathematical Observations」の注意点（色相の平均と差の扱い）どおりに実装する。

/// CIE の ε（216/24389 ≒ 0.008856）。
pub const CIE_EPSILON: f64 = 216.0 / 24389.0;
/// CIE の κ（24389/27 ≒ 903.3）。
pub const CIE_KAPPA: f64 = 24389.0 / 27.0;

/// CIELAB の値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lab {
    /// L\*（明度。白が 100）。
    pub l: f64,
    /// a\*。
    pub a: f64,
    /// b\*。
    pub b: f64,
}

impl Lab {
    /// 作る。
    pub const fn new(l: f64, a: f64, b: f64) -> Self {
        Self { l, a, b }
    }

    /// 彩度 C\*ab。
    pub fn chroma(&self) -> f64 {
        self.a.hypot(self.b)
    }
}

fn lab_f(t: f64) -> f64 {
    if t > CIE_EPSILON {
        t.cbrt()
    } else {
        (CIE_KAPPA * t + 16.0) / 116.0
    }
}

fn lab_f_inv(f: f64) -> f64 {
    let f3 = f * f * f;
    if f3 > CIE_EPSILON {
        f3
    } else {
        (116.0 * f - 16.0) / CIE_KAPPA
    }
}

/// XYZ → CIELAB。`white` は基準白の XYZ（例: D50 の白、Y = 1）。
///
/// XYZ と基準白は同じ尺度（どちらも白の Y = 1、またはどちらも 100）で渡す。
/// 負の値（色域外の色）は線形の区間の式で計算するため NaN にならない。
pub fn xyz_to_lab(xyz: [f64; 3], white: [f64; 3]) -> Lab {
    let fx = lab_f(xyz[0] / white[0]);
    let fy = lab_f(xyz[1] / white[1]);
    let fz = lab_f(xyz[2] / white[2]);
    Lab {
        l: 116.0 * fy - 16.0,
        a: 500.0 * (fx - fy),
        b: 200.0 * (fy - fz),
    }
}

/// CIELAB → XYZ。`white` は基準白の XYZ。
pub fn lab_to_xyz(lab: Lab, white: [f64; 3]) -> [f64; 3] {
    let fy = (lab.l + 16.0) / 116.0;
    let fx = fy + lab.a / 500.0;
    let fz = fy - lab.b / 200.0;
    let yr = if lab.l > CIE_KAPPA * CIE_EPSILON {
        fy * fy * fy
    } else {
        lab.l / CIE_KAPPA
    };
    [
        lab_f_inv(fx) * white[0],
        yr * white[1],
        lab_f_inv(fz) * white[2],
    ]
}

/// ΔE76（CIE 1976 の色差。Lab のユークリッド距離）。
pub fn delta_e76(x: Lab, y: Lab) -> f64 {
    let dl = x.l - y.l;
    let da = x.a - y.a;
    let db = x.b - y.b;
    (dl * dl + da * da + db * db).sqrt()
}

/// ΔE2000（CIEDE2000。パラメーター係数 kL = kC = kH = 1）。
pub fn delta_e2000(x: Lab, y: Lab) -> f64 {
    delta_e2000_weighted(x, y, 1.0, 1.0, 1.0)
}

/// 色相角（度、0 以上 360 未満）。a = b = 0 なら 0（Sharma ら 2005 の扱い）。
fn hue_deg(b: f64, a: f64) -> f64 {
    if a == 0.0 && b == 0.0 {
        return 0.0;
    }
    let h = b.atan2(a).to_degrees();
    if h < 0.0 { h + 360.0 } else { h }
}

/// ΔE2000（CIEDE2000）。パラメーター係数 `kl`・`kc`・`kh` を指定する版。
pub fn delta_e2000_weighted(x: Lab, y: Lab, kl: f64, kc: f64, kh: f64) -> f64 {
    const POW25_7: f64 = 6_103_515_625.0; // 25^7

    let c1 = x.a.hypot(x.b);
    let c2 = y.a.hypot(y.b);
    let c_bar = 0.5 * (c1 + c2);
    let c_bar7 = c_bar.powi(7);
    let g = 0.5 * (1.0 - (c_bar7 / (c_bar7 + POW25_7)).sqrt());

    let a1p = (1.0 + g) * x.a;
    let a2p = (1.0 + g) * y.a;
    let c1p = a1p.hypot(x.b);
    let c2p = a2p.hypot(y.b);
    let h1p = hue_deg(x.b, a1p);
    let h2p = hue_deg(y.b, a2p);

    let dl = y.l - x.l;
    let dc = c2p - c1p;
    let c_prod = c1p * c2p;
    let dh_deg = if c_prod == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let dh = 2.0 * c_prod.sqrt() * (dh_deg.to_radians() * 0.5).sin();

    let l_bar = 0.5 * (x.l + y.l);
    let cp_bar = 0.5 * (c1p + c2p);
    let h_bar = if c_prod == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        0.5 * (h1p + h2p)
    } else if h1p + h2p < 360.0 {
        0.5 * (h1p + h2p + 360.0)
    } else {
        0.5 * (h1p + h2p - 360.0)
    };

    let t = 1.0 - 0.17 * (h_bar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * h_bar).to_radians().cos()
        + 0.32 * (3.0 * h_bar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * h_bar - 63.0).to_radians().cos();
    let d_theta = 30.0 * (-((h_bar - 275.0) / 25.0).powi(2)).exp();
    let cp_bar7 = cp_bar.powi(7);
    let rc = 2.0 * (cp_bar7 / (cp_bar7 + POW25_7)).sqrt();
    let l50 = (l_bar - 50.0) * (l_bar - 50.0);
    let sl = 1.0 + 0.015 * l50 / (20.0 + l50).sqrt();
    let sc = 1.0 + 0.045 * cp_bar;
    let sh = 1.0 + 0.015 * cp_bar * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;

    let tl = dl / (kl * sl);
    let tc = dc / (kc * sc);
    let th = dh / (kh * sh);
    (tl * tl + tc * tc + th * th + rt * tc * th).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::{D50, D65};

    /// Sharma, Wu, Dalal (2005) の CIEDE2000 の検証用データ（全 34 組）。
    /// (L1, a1, b1, L2, a2, b2, ΔE2000)
    const SHARMA_2005: [[f64; 7]; 34] = [
        [50.0000, 2.6772, -79.7751, 50.0000, 0.0000, -82.7485, 2.0425],
        [50.0000, 3.1571, -77.2803, 50.0000, 0.0000, -82.7485, 2.8615],
        [50.0000, 2.8361, -74.0200, 50.0000, 0.0000, -82.7485, 3.4412],
        [
            50.0000, -1.3802, -84.2814, 50.0000, 0.0000, -82.7485, 1.0000,
        ],
        [
            50.0000, -1.1848, -84.8006, 50.0000, 0.0000, -82.7485, 1.0000,
        ],
        [
            50.0000, -0.9009, -85.5211, 50.0000, 0.0000, -82.7485, 1.0000,
        ],
        [50.0000, 0.0000, 0.0000, 50.0000, -1.0000, 2.0000, 2.3669],
        [50.0000, -1.0000, 2.0000, 50.0000, 0.0000, 0.0000, 2.3669],
        [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0009, 7.1792],
        [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0010, 7.1792],
        [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0011, 7.2195],
        [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0012, 7.2195],
        [50.0000, -0.0010, 2.4900, 50.0000, 0.0009, -2.4900, 4.8045],
        [50.0000, -0.0010, 2.4900, 50.0000, 0.0010, -2.4900, 4.8045],
        [50.0000, -0.0010, 2.4900, 50.0000, 0.0011, -2.4900, 4.7461],
        [50.0000, 2.5000, 0.0000, 50.0000, 0.0000, -2.5000, 4.3065],
        [50.0000, 2.5000, 0.0000, 73.0000, 25.0000, -18.0000, 27.1492],
        [50.0000, 2.5000, 0.0000, 61.0000, -5.0000, 29.0000, 22.8977],
        [50.0000, 2.5000, 0.0000, 56.0000, -27.0000, -3.0000, 31.9030],
        [50.0000, 2.5000, 0.0000, 58.0000, 24.0000, 15.0000, 19.4535],
        [50.0000, 2.5000, 0.0000, 50.0000, 3.1736, 0.5854, 1.0000],
        [50.0000, 2.5000, 0.0000, 50.0000, 3.2972, 0.0000, 1.0000],
        [50.0000, 2.5000, 0.0000, 50.0000, 1.8634, 0.5757, 1.0000],
        [50.0000, 2.5000, 0.0000, 50.0000, 3.2592, 0.3350, 1.0000],
        [
            60.2574, -34.0099, 36.2677, 60.4626, -34.1751, 39.4387, 1.2644,
        ],
        [
            63.0109, -31.0961, -5.8663, 62.8187, -29.7946, -4.0864, 1.2630,
        ],
        [61.2901, 3.7196, -5.3901, 61.4292, 2.2480, -4.9620, 1.8731],
        [35.0831, -44.1164, 3.7933, 35.0232, -40.0716, 1.5901, 1.8645],
        [
            22.7233, 20.0904, -46.6940, 23.0331, 14.9730, -42.5619, 2.0373,
        ],
        [36.4612, 47.8580, 18.3852, 36.2715, 50.5065, 21.2231, 1.4146],
        [90.8027, -2.0831, 1.4410, 91.1528, -1.6435, 0.0447, 1.4441],
        [90.9257, -0.5406, -0.9208, 88.6381, -0.8985, -0.7239, 1.5381],
        [6.7747, -0.2908, -2.4247, 5.8714, -0.0985, -2.2286, 0.6377],
        [2.0776, 0.0795, -1.1350, 0.9033, -0.0636, -0.5514, 0.9082],
    ];

    #[test]
    fn delta_e2000_matches_sharma_2005_to_4_decimals() {
        for (i, row) in SHARMA_2005.iter().enumerate() {
            let x = Lab::new(row[0], row[1], row[2]);
            let y = Lab::new(row[3], row[4], row[5]);
            let de = delta_e2000(x, y);
            // 公表値は小数 4 桁に丸めた値。丸めた結果が一致することを確認する。
            let rounded = (de * 1e4).round() / 1e4;
            assert!(
                (rounded - row[6]).abs() < 1e-9,
                "組 {}: 計算値 {de:.6}、公表値 {}",
                i + 1,
                row[6]
            );
            // 対称性（Sharma らの注意点: 色相の平均の扱いが正しければ入れ替えても同じ）。
            assert!(
                (delta_e2000(y, x) - de).abs() < 1e-12,
                "組 {} の対称性",
                i + 1
            );
        }
    }

    #[test]
    fn delta_e2000_zero_for_identical() {
        for lab in [
            Lab::new(50.0, 0.0, 0.0),
            Lab::new(0.0, 0.0, 0.0),
            Lab::new(75.0, -20.0, 33.0),
        ] {
            assert_eq!(delta_e2000(lab, lab), 0.0);
            assert_eq!(delta_e76(lab, lab), 0.0);
        }
    }

    #[test]
    fn delta_e76_is_euclidean() {
        let x = Lab::new(50.0, 10.0, -5.0);
        let y = Lab::new(53.0, 14.0, -5.0);
        assert!((delta_e76(x, y) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn delta_e2000_weights_scale_lightness_term() {
        // 明度だけが違う組では、kL を 2 倍にすると ΔE は半分になる。
        let x = Lab::new(50.0, 0.0, 0.0);
        let y = Lab::new(60.0, 0.0, 0.0);
        let d1 = delta_e2000_weighted(x, y, 1.0, 1.0, 1.0);
        let d2 = delta_e2000_weighted(x, y, 2.0, 1.0, 1.0);
        assert!((d1 - 2.0 * d2).abs() < 1e-12);
    }

    #[test]
    fn white_and_black() {
        let w = D65.to_xyz(1.0).unwrap();
        let lab = xyz_to_lab(w, w);
        assert!((lab.l - 100.0).abs() < 1e-12 && lab.a.abs() < 1e-12 && lab.b.abs() < 1e-12);
        let k = xyz_to_lab([0.0; 3], w);
        assert!(k.l.abs() < 1e-12 && k.a.abs() < 1e-12 && k.b.abs() < 1e-12);
    }

    #[test]
    fn known_lab_values() {
        // Y = 0.18（中間のグレー）は L* ≒ 49.4961。
        let w = D50.to_xyz(1.0).unwrap();
        let g = [w[0] * 0.18, 0.18, w[2] * 0.18];
        let lab = xyz_to_lab(g, w);
        assert!((lab.l - 49.4961).abs() < 1e-4, "{}", lab.l);
        // 線形の区間: Y = 0.005 は L* = κ·0.005 = 4.5165...
        let lab = xyz_to_lab([w[0] * 0.005, 0.005, w[2] * 0.005], w);
        assert!((lab.l - CIE_KAPPA * 0.005).abs() < 1e-10);
    }

    #[test]
    fn lab_roundtrip_including_negative_and_dark() {
        let w = D65.to_xyz(1.0).unwrap();
        let samples = [
            [0.5, 0.4, 0.3],
            [0.001, 0.002, 0.0005],
            [0.9, 1.2, 1.5],
            [-0.01, 0.02, 0.03],
            [0.3, -0.001, 0.2],
            [0.0, 0.0, 0.0],
        ];
        for xyz in samples {
            let lab = xyz_to_lab(xyz, w);
            assert!(lab.l.is_finite() && lab.a.is_finite() && lab.b.is_finite());
            let back = lab_to_xyz(lab, w);
            for k in 0..3 {
                assert!((back[k] - xyz[k]).abs() < 1e-12, "{xyz:?} → {back:?}");
            }
        }
    }

    #[test]
    fn chroma() {
        assert!((Lab::new(50.0, 3.0, 4.0).chroma() - 5.0).abs() < 1e-15);
    }
}
