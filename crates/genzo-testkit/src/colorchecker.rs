//! ColorChecker Classic（24 色）の基準値と、色のパッチの合成画像。
//!
//! # 出典
//!
//! - 値: X-Rite (2016)「New color specifications for ColorChecker SG and Classic Charts」の
//!   ColorChecker Classic の CIE L\*a\*b\*（D50・2° 視野）。顔料の変更に伴い、**2014 年 11 月以降**に
//!   製造されたチャートと、それより前のチャートで値が違う（チャートの裏面に製造日が印字されている）。
//!   PoC-4 では使うチャートの製造時期に対応した値を選ぶ（05 の PoC-4 の測定方法 ①）。
//! - 照合: colour-science 0.4.6（BSD-3-Clause）の `DATA_COLORCHECKER24_AFTER_NOV2014_CIE_LAB`・
//!   `DATA_COLORCHECKER24_BEFORE_NOV2014_CIE_LAB`（同じ X-Rite の資料を出典とする）と、全 24 色の
//!   値が一致することを 2026-10-10 に確認した。**X-Rite の公表資料の原本とは照合していない**ため、
//!   PoC-4 で使う前に人が原本（CGATS のファイル）と照合すること。
//!
//! # D65 への変換
//!
//! 05 の PoC-4 の測定方法 ① のとおり、D50 の Lab を **Bradford 法で D65 に色順応** させて使う。
//!
//! 1. Lab（D50）→ XYZ（D50）。基準白は ICC の D50（X = 0.9642、Y = 1、Z = 0.8249）。
//!    X-Rite の資料がどの D50 の XYZ を前提にしているかは未確認のため、colour-science と同じ
//!    ICC の値とする（xy = (0.3457, 0.3585) から求めた値との差は Z で約 0.0002）。
//! 2. Bradford 法で ICC の D50 → D65（xy = (0.3127, 0.3290)）に順応する（genzo-color の
//!    [`bradford_adaptation`]）。
//! 3. XYZ（D65）→ 指定の色空間のリニア RGB、または Lab（基準白 D65）。
//!
//! 白（L\* = 100）が XYZ の Y = 1 に対応する。シーンリニアの画像として使う場合は、反射率の
//! 被写体を露出 0 で撮った値に相当する（白 9.5 のパッチで Y ≒ 0.88）。

use genzo_color::lab::{Lab, lab_to_xyz, xyz_to_lab};
use genzo_color::space::bradford_adaptation;
use genzo_color::{D65, Mat3, RgbColorSpace};

use crate::image::{FloatImage, ImageError, Rect};

/// ICC の D50 の白の XYZ（ICC.1 の PCS の白。Y = 1）。
pub const ICC_D50_XYZ: [f64; 3] = [0.9642, 1.0, 0.8249];

/// パッチの数。
pub const PATCH_COUNT: usize = 24;

/// パッチの名前（チャートの左上から右へ、行ごと。番号は 1〜24）。
pub const PATCH_NAMES: [&str; PATCH_COUNT] = [
    "dark skin",
    "light skin",
    "blue sky",
    "foliage",
    "blue flower",
    "bluish green",
    "orange",
    "purplish blue",
    "moderate red",
    "purple",
    "yellow green",
    "orange yellow",
    "blue",
    "green",
    "red",
    "yellow",
    "magenta",
    "cyan",
    "white 9.5 (.05 D)",
    "neutral 8 (.23 D)",
    "neutral 6.5 (.44 D)",
    "neutral 5 (.70 D)",
    "neutral 3.5 (1.05 D)",
    "black 2 (1.5 D)",
];

/// 無彩色のパッチ（19〜24）の位置（0 始まり）。
pub const NEUTRAL_PATCHES: std::ops::Range<usize> = 18..24;

/// 2014 年 11 月以降のチャートの L\*a\*b\*（D50）。X-Rite (2016)。
const LAB_D50_AFTER_NOV2014: [[f64; 3]; PATCH_COUNT] = [
    [37.54, 14.37, 14.92],
    [64.66, 19.27, 17.50],
    [49.32, -3.82, -22.54],
    [43.46, -12.74, 22.72],
    [54.94, 9.61, -24.79],
    [70.48, -32.26, -0.37],
    [62.73, 35.83, 56.50],
    [39.43, 10.75, -45.17],
    [50.57, 48.64, 16.67],
    [30.10, 22.54, -20.87],
    [71.77, -24.13, 58.19],
    [71.51, 18.24, 67.37],
    [28.37, 15.42, -49.80],
    [54.38, -39.72, 32.27],
    [42.43, 51.05, 28.62],
    [81.80, 2.67, 80.41],
    [50.63, 51.28, -14.12],
    [49.57, -29.71, -28.32],
    [95.19, -1.03, 2.93],
    [81.29, -0.57, 0.44],
    [66.89, -0.75, -0.06],
    [50.76, -0.13, 0.14],
    [35.63, -0.46, -0.48],
    [20.64, 0.07, -0.46],
];

/// 2014 年 11 月より前のチャートの L\*a\*b\*（D50）。X-Rite (2016)。
const LAB_D50_BEFORE_NOV2014: [[f64; 3]; PATCH_COUNT] = [
    [37.986, 13.555, 14.059],
    [65.711, 18.130, 17.810],
    [49.927, -4.880, -21.905],
    [43.139, -13.095, 21.905],
    [55.112, 8.844, -25.399],
    [70.719, -33.397, -0.199],
    [62.661, 36.067, 57.096],
    [40.020, 10.410, -45.964],
    [51.124, 48.239, 16.248],
    [30.325, 22.976, -21.587],
    [72.532, -23.709, 57.255],
    [71.941, 19.363, 67.857],
    [28.778, 14.179, -50.297],
    [55.261, -38.342, 31.370],
    [42.101, 53.378, 28.190],
    [81.733, 4.039, 79.819],
    [51.935, 49.986, -14.574],
    [51.038, -28.631, -28.638],
    [96.539, -0.425, 1.186],
    [81.257, -0.638, -0.335],
    [66.766, -0.734, -0.504],
    [50.867, -0.153, -0.270],
    [35.656, -0.421, -1.231],
    [20.461, -0.079, -0.973],
];

/// チャートの版（製造時期）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorCheckerEdition {
    /// 2014 年 11 月より前に製造されたチャート。
    BeforeNov2014,
    /// 2014 年 11 月以降に製造されたチャート。
    AfterNov2014,
}

/// ICC の D50 → D65 の Bradford 法の色順応の行列。
fn d50_to_d65() -> Mat3 {
    bradford_adaptation(ICC_D50_XYZ, d65_white())
        .expect("ICC の D50 と D65 の白は定数で、錐体応答は 0 にならない")
}

fn d65_white() -> [f64; 3] {
    D65.to_xyz(1.0)
        .expect("D65 の色度座標は定数で、y > 0 なので必ず XYZ にできる")
}

/// Lab（D50、基準白は ICC の D50）を、Bradford 法で D65 に順応した XYZ（Y = 1 が白）にする。
pub fn lab_d50_to_xyz_d65(lab: Lab) -> [f64; 3] {
    d50_to_d65().apply(lab_to_xyz(lab, ICC_D50_XYZ))
}

impl ColorCheckerEdition {
    /// 公表値の L\*a\*b\*（D50）。
    pub fn lab_d50(self) -> [Lab; PATCH_COUNT] {
        let table = match self {
            ColorCheckerEdition::BeforeNov2014 => &LAB_D50_BEFORE_NOV2014,
            ColorCheckerEdition::AfterNov2014 => &LAB_D50_AFTER_NOV2014,
        };
        table.map(|[l, a, b]| Lab::new(l, a, b))
    }

    /// Bradford 法で D65 に順応した XYZ（Y = 1 が白）。
    pub fn xyz_d65(self) -> [[f64; 3]; PATCH_COUNT] {
        let m = d50_to_d65();
        self.lab_d50()
            .map(|lab| m.apply(lab_to_xyz(lab, ICC_D50_XYZ)))
    }

    /// Bradford 法で D65 に順応した L\*a\*b\*（基準白 D65）。PoC-4 の色差の基準値。
    pub fn lab_d65(self) -> [Lab; PATCH_COUNT] {
        let white = d65_white();
        self.xyz_d65().map(|xyz| xyz_to_lab(xyz, white))
    }

    /// 指定の色空間のリニア RGB（D65）。色域外の色（例: sRGB の cyan）は負の値になる（切り詰めない）。
    pub fn linear_rgb(self, space: RgbColorSpace) -> [[f64; 3]; PATCH_COUNT] {
        let m = space.from_xyz();
        self.xyz_d65().map(|xyz| m.apply(xyz))
    }
}

/// チャートの画像の配置（6 列 × 4 行。パッチの間と外周に `gap` 画素の余白）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorCheckerLayout {
    /// パッチの一辺（画素）。
    pub patch_size: u32,
    /// パッチの間と外周の余白（画素）。
    pub gap: u32,
}

impl ColorCheckerLayout {
    /// 列の数。
    pub const COLUMNS: u32 = 6;
    /// 行の数。
    pub const ROWS: u32 = 4;

    /// 画像の幅。
    pub fn width(&self) -> u32 {
        Self::COLUMNS * self.patch_size + (Self::COLUMNS + 1) * self.gap
    }

    /// 画像の高さ。
    pub fn height(&self) -> u32 {
        Self::ROWS * self.patch_size + (Self::ROWS + 1) * self.gap
    }

    /// パッチ `index`（0〜23。番号 − 1）の矩形。範囲外なら `None`。
    pub fn patch_rect(&self, index: usize) -> Option<Rect> {
        if index >= PATCH_COUNT {
            return None;
        }
        let col = index as u32 % Self::COLUMNS;
        let row = index as u32 / Self::COLUMNS;
        let step = self.patch_size + self.gap;
        Some(Rect::new(
            self.gap + col * step,
            self.gap + row * step,
            self.patch_size,
            self.patch_size,
        ))
    }
}

/// ColorChecker の画像（シーンリニア、指定の色空間のリニア RGB）を作る。
///
/// 各パッチの値は [`ColorCheckerEdition::linear_rgb`] に `exposure`（倍率。1 なら白 = Y 1）を
/// 掛けたもの。余白は `background`（例: チャートの枠の黒）。
pub fn color_checker_image(
    edition: ColorCheckerEdition,
    space: RgbColorSpace,
    layout: ColorCheckerLayout,
    background: [f32; 3],
    exposure: f32,
) -> Result<FloatImage, ImageError> {
    let rgb = edition.linear_rgb(space);
    let mut img = FloatImage::filled(layout.width(), layout.height(), &background)?;
    for (i, c) in rgb.iter().enumerate() {
        let r = layout.patch_rect(i).expect("i は 24 未満");
        let v = c.map(|x| (x * f64::from(exposure)) as f32);
        for y in r.y..r.y + r.height {
            for x in r.x..r.x + r.width {
                if let Some(p) = img.pixel_mut(x, y) {
                    p.copy_from_slice(&v);
                }
            }
        }
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use genzo_color::lab::delta_e2000;

    use super::*;
    use crate::diff::LabConversion;

    #[test]
    fn tables_have_expected_spot_values() {
        // 表の転記の確認（X-Rite (2016)。colour-science 0.4.6 と照合済み）。
        let after = ColorCheckerEdition::AfterNov2014.lab_d50();
        assert_eq!(after[0], Lab::new(37.54, 14.37, 14.92));
        assert_eq!(after[17], Lab::new(49.57, -29.71, -28.32));
        assert_eq!(after[23], Lab::new(20.64, 0.07, -0.46));
        let before = ColorCheckerEdition::BeforeNov2014.lab_d50();
        assert_eq!(before[0], Lab::new(37.986, 13.555, 14.059));
        assert_eq!(before[18], Lab::new(96.539, -0.425, 1.186));
        assert_eq!(PATCH_NAMES[21], "neutral 5 (.70 D)");
    }

    #[test]
    fn editions_differ_but_are_close() {
        // 版の違いは ΔE2000 で数以下（同じ色の配合の変更）。取り違えの検出用に、
        // すべてのパッチで 0 より大きく 3 未満であることを確かめる。
        let a = ColorCheckerEdition::AfterNov2014.lab_d50();
        let b = ColorCheckerEdition::BeforeNov2014.lab_d50();
        for i in 0..PATCH_COUNT {
            let de = delta_e2000(a[i], b[i]);
            assert!(de > 0.0 && de < 3.0, "{}: {de}", PATCH_NAMES[i]);
        }
    }

    #[test]
    fn icc_d50_matches_icc_pcs_illuminant() {
        // ICC.1 のプロファイルのヘッダの PCS の光源（D50）は s15Fixed16 で
        // X = 0x0000F6D6、Y = 0x00010000、Z = 0x0000D32D と符号化される。
        let enc = ICC_D50_XYZ.map(|v| (v * 65536.0).round() as u32);
        assert_eq!(enc, [0xF6D6, 0x1_0000, 0xD32D]);
        // colour-science の "ICC D50" の色度座標 (0.345702914918791, 0.358538596679933) と同じ白。
        let sum: f64 = ICC_D50_XYZ.iter().sum();
        assert!((ICC_D50_XYZ[0] / sum - 0.345_702_914_918_791).abs() < 1e-12);
        assert!((ICC_D50_XYZ[1] / sum - 0.358_538_596_679_933).abs() < 1e-12);
    }

    #[test]
    fn bradford_d50_to_d65_is_close_to_published_matrix() {
        // Lindbloom の Bradford 法の D50 → D65 の行列（白は D50 = (0.96422, 1, 0.82521)、
        // D65 = (0.95047, 1, 1.08883)。小数 7 桁）。ここでは白が少し違う（ICC の D50 と、
        // xy = (0.3127, 0.3290) から求めた D65）ため、要素の差は最大 8.0e-4 になる。
        // 向き（D65 → D50 の行列は対角が 1.048・0.993・0.752）の取り違えを検出できる精度で比べる。
        const LINDBLOOM: [[f64; 3]; 3] = [
            [0.955_576_6, -0.023_039_3, 0.063_163_6],
            [-0.028_289_5, 1.009_941_6, 0.021_007_7],
            [0.012_298_2, -0.020_483_0, 1.329_909_8],
        ];
        // 同じ式（Bradford の錐体応答の行列 0.8951 0.2664 −0.1614 / −0.7502 1.7135 0.0367 /
        // 0.0389 −0.0685 1.0296）で、この crate の白（ICC の D50 → xy から求めた D65）について
        // 別に（Python で、genzo-color を使わずに）計算した値（小数 7 桁。2026-10-10）。
        // 同じ計算で Lindbloom の白を使うと、上の公表値と 4.5e-8 以内で一致した。
        const INDEPENDENT: [[f64; 3]; 3] = [
            [0.955_512_6, -0.023_073_0, 0.063_309_1],
            [-0.028_324_8, 1.009_942_9, 0.021_054_4],
            [0.012_328_7, -0.020_535_3, 1.330_713_7],
        ];
        let m = d50_to_d65().rows();
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    (m[i][j] - LINDBLOOM[i][j]).abs() < 1e-3,
                    "({i}, {j}): {} と {}",
                    m[i][j],
                    LINDBLOOM[i][j]
                );
                assert!(
                    (m[i][j] - INDEPENDENT[i][j]).abs() < 1e-6,
                    "({i}, {j}): {} と {}",
                    m[i][j],
                    INDEPENDENT[i][j]
                );
            }
        }
    }

    #[test]
    fn adapted_values_match_independent_computation() {
        // 2014 年 11 月以降の版の 3 色（dark skin・cyan・white 9.5）を、CIE の式（ε = 216/24389、
        // κ = 24389/27）と上の Bradford 法で、別に（Python で、genzo-color を使わずに）計算した
        // XYZ（D65）と Lab（基準白 D65）（小数 6 桁。2026-10-10）。PoC-4 の基準値の計算の組み立て
        // （Lab → XYZ の基準白、順応の向き、Lab の基準白）を絶対値で確かめる。
        let cases: [(usize, [f64; 3], [f64; 3]); 3] = [
            (
                0,
                [0.109_341, 0.097_089, 0.062_982],
                [37.314_907, 13.370_534, 14.579_786],
            ),
            (
                17,
                [0.133_522, 0.185_005, 0.385_519],
                [50.097_601, -24.981_089, -27.518_565],
            ),
            (
                18,
                [0.829_761, 0.880_154, 0.915_403],
                [95.167_442, -1.299_456, 2.919_368],
            ),
        ];
        let xyz = ColorCheckerEdition::AfterNov2014.xyz_d65();
        let lab = ColorCheckerEdition::AfterNov2014.lab_d65();
        for (i, ex, el) in cases {
            for k in 0..3 {
                assert!((xyz[i][k] - ex[k]).abs() < 1e-5, "{i}: {:?}", xyz[i]);
            }
            for (v, e) in [lab[i].l, lab[i].a, lab[i].b].into_iter().zip(el) {
                assert!((v - e).abs() < 1e-5, "{i}: {:?}", lab[i]);
            }
        }
    }

    #[test]
    fn neutral_stays_neutral_through_adaptation() {
        // a* = b* = 0 の D50 の色は、Bradford 法で D65 にしても a* = b* = 0（白が白に移る）で、
        // L* も変わらない。
        let white = d65_white();
        for l in [20.0, 50.0, 96.0] {
            let lab = xyz_to_lab(lab_d50_to_xyz_d65(Lab::new(l, 0.0, 0.0)), white);
            assert!((lab.l - l).abs() < 1e-9, "{lab:?}");
            assert!(lab.a.abs() < 1e-9 && lab.b.abs() < 1e-9, "{lab:?}");
        }
        // 白 (100, 0, 0) は D65 の白の XYZ になる。
        let w = lab_d50_to_xyz_d65(Lab::new(100.0, 0.0, 0.0));
        for k in 0..3 {
            assert!((w[k] - white[k]).abs() < 1e-12);
        }
    }

    #[test]
    fn neutral_patches_are_nearly_neutral_and_have_known_luminance() {
        let lab = ColorCheckerEdition::AfterNov2014.lab_d65();
        let xyz = ColorCheckerEdition::AfterNov2014.xyz_d65();
        for i in NEUTRAL_PATCHES {
            // 元の a*・b* は ±1.1 以内（白 9.5 の b* = 2.93 を除く）なので、順応後も小さい。
            assert!(lab[i].chroma() < 3.5, "{}: {:?}", PATCH_NAMES[i], lab[i]);
        }
        // 中間のグレー（neutral 5、L* = 50.76）の Y は ((50.76 + 16) / 116)^3 ≒ 0.19062。
        // 無彩色に近いので、順応後の Y もほぼ同じ。
        let expected = ((50.76_f64 + 16.0) / 116.0).powi(3);
        assert!((expected - 0.190_62).abs() < 1e-5);
        assert!((xyz[21][1] - expected).abs() < 2e-3, "{}", xyz[21][1]);
    }

    #[test]
    fn rgb_roundtrips_to_lab_d65() {
        // リニア RGB → Lab（diff の変換）で、D65 の Lab に戻る（どの色空間でも）。
        for edition in [
            ColorCheckerEdition::AfterNov2014,
            ColorCheckerEdition::BeforeNov2014,
        ] {
            let lab = edition.lab_d65();
            for space in RgbColorSpace::ALL {
                let rgb = edition.linear_rgb(space);
                let conv = LabConversion::linear(space);
                for i in 0..PATCH_COUNT {
                    let c = rgb[i].map(|v| v as f32);
                    let back = conv.to_lab(c);
                    // f32 に丸めた分だけ誤差がある。
                    assert!(delta_e2000(back, lab[i]) < 1e-3, "{space:?} {i}");
                }
            }
        }
    }

    #[test]
    fn gamut_of_patches() {
        // BT.2020（作業色空間）ではすべて色域内（負の値なし）。sRGB では cyan（18 番）が色域外。
        let bt2020 = ColorCheckerEdition::AfterNov2014.linear_rgb(RgbColorSpace::Bt2020);
        assert!(bt2020.iter().flatten().all(|&v| v > 0.0));
        let srgb = ColorCheckerEdition::AfterNov2014.linear_rgb(RgbColorSpace::Srgb);
        assert!(srgb[17][0] < 0.0, "{:?}", srgb[17]);
        // 白 9.5 の Y は約 0.88（L* = 95.19）。
        let xyz = ColorCheckerEdition::AfterNov2014.xyz_d65();
        assert!((xyz[18][1] - 0.88).abs() < 0.01, "{}", xyz[18][1]);
    }

    #[test]
    fn layout_and_image() {
        let layout = ColorCheckerLayout {
            patch_size: 10,
            gap: 2,
        };
        assert_eq!(layout.width(), 6 * 10 + 7 * 2);
        assert_eq!(layout.height(), 4 * 10 + 5 * 2);
        assert_eq!(layout.patch_rect(0), Some(Rect::new(2, 2, 10, 10)));
        assert_eq!(layout.patch_rect(7), Some(Rect::new(14, 14, 10, 10)));
        assert_eq!(layout.patch_rect(23), Some(Rect::new(62, 38, 10, 10)));
        assert_eq!(layout.patch_rect(24), None);
        let img = color_checker_image(
            ColorCheckerEdition::AfterNov2014,
            RgbColorSpace::Bt2020,
            layout,
            [0.01; 3],
            2.0,
        )
        .unwrap();
        let rgb = ColorCheckerEdition::AfterNov2014.linear_rgb(RgbColorSpace::Bt2020);
        for i in [0usize, 11, 21] {
            let r = layout.patch_rect(i).unwrap().inset(2).unwrap();
            let m = img.region_mean(r).unwrap();
            for k in 0..3 {
                assert!((m[k] - rgb[i][k] * 2.0).abs() < 1e-6, "{i}");
            }
        }
        assert_eq!(img.pixel(0, 0), Some(&[0.01, 0.01, 0.01][..]));
        assert_eq!(img.pixel(12, 5), Some(&[0.01, 0.01, 0.01][..]));
    }
}
