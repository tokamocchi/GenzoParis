//! 3D LUT（画面への出力。04 の 2.6 節のステージ 17a、5 章）。
//!
//! - 作成: 変換元プロファイル（[`crate::icc::StandardProfile::Bt2020SrgbTransfer`]、キャッシュを
//!   表示するときは Display P3）→ 変換先プロファイル（モニター）の変換を、lcms2 の浮動小数点の
//!   変換（既定は相対的な色域を維持するインテント。色域の圧縮は前段で済んでいるため）で、格子
//!   （既定 33³）の各点に評価して作る。
//! - 適用: **四面体補間を自前で実装** する（GPU のハードウェア補間は精度が GPU によって違うため、
//!   GPU 版も同じ式で補間する。2.3 節）。
//!
//! ## 格子の約束（GPU 版と共有する）
//!
//! - 格子数を n とすると、格子点 (i, j, k) の入力は `(i / (n−1), j / (n−1), k / (n−1))`（f32 で計算）。
//! - データの並びは **R が最も速く変わる**: `data[(b·n + g)·n + r]`。3D テクスチャにするときは
//!   x = R、y = G、z = B。
//! - 入力の座標: 各成分を 0〜1 に収め（NaN は 0）、`x = v · (n − 1)`、`i = min(floor(x), n − 2)`、
//!   `f = x − i`。
//! - 補間: 小数部 (fr, fg, fb) の大小で 6 つの四面体のどれかを選び、立方体の角 c000 から c111 への
//!   経路の 4 点を、重み `(1 − f1, f1 − f2, f2 − f3, f3)`（f1 ≥ f2 ≥ f3 は小数部を大きい順に
//!   並べたもの）で足す。各成分は `w0·c0 + w1·c1 + w2·c2 + w3·c3` を左から順に計算する。
//!   どの四面体を選ぶかの条件（`>=` と `>` の使い分け）は [`Lut3d::apply`] のとおり。小数部が
//!   同じ値のときはどちらの四面体を選んでも結果は同じになる（隣り合う四面体は境界の面で一致する）。
//! - 出力は格子の値をそのまま補間した値で、0〜1 に収めない（量子化するときに収める）。格子の値に
//!   NaN・無限大があれば、作成時に 0 に置き換える（2.6 節）。
//!
//! ## 格子点の値（変換先の色域の外の点）
//!
//! 変換先（モニター）の色域の外の格子点は、0〜1 の外の値を持つ。色域の境界の近くの色は、色域の
//! 外の格子点との補間で求まるため、格子点が 0〜1 に収められていると、境界の近くの誤差が増える
//! （指摘 F36）。変換先のトーンカーブの種類ごとの扱い:
//!
//! - 区分関数（v4 の 'para'）・ガンマ値（1 点の 'curv'）: lcms2 の変換が範囲の外も計算する
//!   （[`IccTransform`] の doc。純粋なべき乗の負の値は 0 になる）。
//! - 表（v2 の 2 点以上の 'curv'。Windows の既定の sRGB のプロファイルもこの形とされる。**要確認**）:
//!   lcms2 は出力を 0〜1 に収める。変換先が matrix-shaper なら、トーンカーブをガンマ 1 にした
//!   プロファイルへの変換で範囲を切らないリニアな値を求め、0〜1 の外のチャンネルを、lcms2 の逆関数の
//!   端の値から端の傾き（0 の側は幅 1/1024、1 の側は幅 1/128 の差分で測る）で線形に延長する
//!   （`icc.rs` の `extend_tabulated_output`）。
//!   0 より下は v4 の区分関数と同じく線形の区間の延長になる。1 より上は v4 の冪の延長と値が違う
//!   （線形）が、精度は v4 とほぼ同じ（下の表）。
//! - LUT 型のプロファイル（出力に BToA・BToD のタグを使うもの）: lcms2 の値のまま（延長する手段が
//!   ない。既知の制約。実際のモニターのプロファイルでの誤差は PoC-1 で確認する）。
//!
//! ## 精度（lcms2 の直接変換との差。PoC-1 の合格基準「ΔE2000 で最大 1 以下」（仮置き）との関係）
//!
//! 変換元 (a)（BT.2020・IEC 61966-2-1）から各変換先への 33³ の LUT を、lcms2 の直接変換と
//! ΔE2000（変換先のプロファイルで Lab（D50）にして計算）で比べた最大値（このコンテナで測定。
//! `report_lut_accuracy` のテストで再現できる）:
//!
//! - 「色域内」「全体」: 変換元の値で一様な乱数 20 万点と、無彩色・暗部・原色の軸の点（色域の圧縮
//!   なし）。「色域内」は、変換先のリニアな RGB（ICC の行列で計算）と直接変換の出力がどちらも 0〜1 に
//!   収まる点。「全体」は色域の外の色も含む。
//! - 「圧縮後」: genzo-pipeline の `DisplayTransform` と同じ手順（モニターの色域への色域の圧縮 →
//!   符号化 → LUT → 0〜1 に収める）で、B3（リニア BT.2020）の 0〜1 の一様な乱数 30 万点。直接変換も
//!   同じ圧縮・符号化の後に行う。一様な乱数なので、暗部（L* < 5）の点は少ない。
//!
//! | 変換先 | 色域内 | 色域内で L* ≥ 5 | 全体 | 圧縮後 |
//! |---|---|---|---|---|
//! | sRGB（v4.3） | 0.48 | 0.48 | 0.48 | 0.50 |
//! | Display P3（v4.3） | 0.34 | 0.34 | 0.34 | 0.33 |
//! | リニア BT.2020 | 0.44 | 0.44 | 0.44 | 0.16 |
//! | Adobe RGB（ガンマ 563/256） | **1.21** | 0.61 | **1.85** | 0.48 |
//! | ガンマ 1.8 のモニターの例 | 0.92 | 0.55 | **1.31** | 0.35 |
//! | sRGB v2.4（4096 点の表） | 0.47 | 0.47 | 0.47 | 0.50 |
//! | sRGB v2.1（1024 点の表。Windows の既定に似せたもの） | 0.48 | 0.48 | 0.48 | 0.50 |
//! | Display P3 v2.4（4096 点の表） | 0.34 | 0.34 | 0.34 | 0.33 |
//! | Display P3 v2.1（1024 点の表） | 0.34 | 0.34 | 0.34 | 0.33 |
//!
//! 表のトーンカーブの変換先は、格子点を延長する前（指摘 F36 の修正前）は sRGB v2.4 で色域内 1.14・
//! 圧縮後 1.54、Display P3 v2.4 で色域内 1.05・圧縮後 1.38 だった（圧縮後の最大は、白に近い明るい
//! 暖色など、色域の境界の近くの明るい色）。
//!
//! 変換先の伝達関数が純粋なべき乗だと、0 の近くで傾きが無限大になり、最初の格子の区間
//! （L* < 5 程度の暗部）の補間誤差が大きくなる。格子数を 65 にすると Adobe RGB の色域内で 0.65
//! （全体で 0.84）で、暗部を除けば 33³ で基準を満たす。基準の適用範囲（暗部を除くか）、格子数、1D の
//! 後段カーブの追加のどれで対応するかは PoC-1 で判断する。
//!
//! 「色域内」の判定は、指摘 F36 の修正（2026-10-10）で、直接変換の出力だけでなく変換先のリニアな
//! RGB も見るようにした。それまでの値（Adobe RGB 1.85、ガンマ 1.8 の例 1.31、65³ の Adobe RGB 0.84）
//! は、lcms2 が負の値を 0 に収めた色域のわずかに外の暗い色を含んでいた（今の「全体」と同じ値）。

use crate::error::{ColorError, Result};
use crate::icc::{IccProfile, IccTransform, RenderingIntent, extend_tabulated_output};

/// 格子数の既定値（2.6 節・5 章）。
pub const DEFAULT_LUT_SIZE: usize = 33;
/// 格子数の下限。
pub const MIN_LUT_SIZE: usize = 2;
/// 格子数の上限（仮置き）。129³ × 12 バイト ≒ 26 MB で、これより細かい格子は必要ないと考えた。
pub const MAX_LUT_SIZE: usize = 129;

/// 3D LUT（RGB → RGB、f32）。
#[derive(Clone, PartialEq)]
pub struct Lut3d {
    size: usize,
    data: Vec<[f32; 3]>,
}

impl std::fmt::Debug for Lut3d {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lut3d")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

fn check_size(size: usize) -> Result<()> {
    if (MIN_LUT_SIZE..=MAX_LUT_SIZE).contains(&size) {
        Ok(())
    } else {
        Err(ColorError::InvalidLutSize {
            size,
            min: MIN_LUT_SIZE,
            max: MAX_LUT_SIZE,
        })
    }
}

fn sanitize(v: [f32; 3]) -> [f32; 3] {
    v.map(|x| if x.is_finite() { x } else { 0.0 })
}

impl Lut3d {
    /// 格子点 (r, g, b) の入力の値（`index / (n − 1)`）。
    ///
    /// LUT として使えない格子数（0・1）でもパニックや NaN にならないよう、`n − 1` は 1 以上に
    /// して計算する（格子数 1 なら index 0 は 0）。
    pub fn grid_input(size: usize, r: usize, g: usize, b: usize) -> [f32; 3] {
        let n1 = size.saturating_sub(1).max(1) as f32;
        [r as f32 / n1, g as f32 / n1, b as f32 / n1]
    }

    /// 関数を各格子点で評価して作る。非有限の値は 0 に置き換える。
    pub fn from_fn(size: usize, mut f: impl FnMut([f32; 3]) -> [f32; 3]) -> Result<Self> {
        check_size(size)?;
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push(sanitize(f(Self::grid_input(size, r, g, b))));
                }
            }
        }
        Ok(Self { size, data })
    }

    /// 格子の値（R が最も速く変わる並び）から作る。長さが n³ でなければエラー。非有限の値は 0 に
    /// 置き換える。
    pub fn from_data(size: usize, data: Vec<[f32; 3]>) -> Result<Self> {
        check_size(size)?;
        let expected = size * size * size;
        if data.len() != expected {
            return Err(ColorError::LengthMismatch {
                input: data.len(),
                output: expected,
            });
        }
        let data = data.into_iter().map(sanitize).collect();
        Ok(Self { size, data })
    }

    /// 恒等変換の LUT。
    pub fn identity(size: usize) -> Result<Self> {
        Self::from_fn(size, |v| v)
    }

    /// `src` → `dst` の lcms2 の浮動小数点の変換（相対的な色域を維持）から作る。
    ///
    /// 画面用には `src` を [`crate::icc::StandardProfile::Bt2020SrgbTransfer`]（キャッシュの表示では
    /// Display P3）、`dst` をモニターのプロファイルにする（2.6 節 17a）。
    ///
    /// `dst` の色域の外の格子点は、0〜1 の外の値を持つ（境界の近くの補間の誤差を小さくするため）。
    /// lcms2 は変換先のトーンカーブが表（v2 の 2 点以上の 'curv'）だと出力を 0〜1 に収めるので、
    /// matrix-shaper のプロファイルではその格子点を範囲の外へ線形に延長する（指摘 F36。
    /// 方法はモジュールの doc の「格子点の値」）。
    pub fn from_icc(src: &IccProfile, dst: &IccProfile, size: usize) -> Result<Self> {
        Self::from_icc_with_intent(src, dst, size, RenderingIntent::RelativeColorimetric)
    }

    /// インテントを指定して lcms2 の変換から作る（格子点の値は [`Lut3d::from_icc`] と同じ扱い）。
    pub fn from_icc_with_intent(
        src: &IccProfile,
        dst: &IccProfile,
        size: usize,
        intent: RenderingIntent,
    ) -> Result<Self> {
        check_size(size)?;
        let transform = IccTransform::new(src, dst, intent)?;
        let identity = Self::identity(size)?;
        let mut data = vec![[0.0_f32; 3]; identity.data.len()];
        transform.transform(&identity.data, &mut data)?;
        extend_tabulated_output(src, dst, intent, &identity.data, &mut data)?;
        Self::from_data(size, data)
    }

    /// 格子数 n。
    pub fn size(&self) -> usize {
        self.size
    }

    /// 格子の値（R が最も速く変わる並び。長さ n³）。
    pub fn data(&self) -> &[[f32; 3]] {
        &self.data
    }

    /// 格子の値を f32 の平らな配列として（長さ 3n³。GPU へのアップロード用）。
    pub fn as_flat(&self) -> &[f32] {
        self.data.as_flattened()
    }

    /// RGBA（A = 1）の f32 の配列（長さ 4n³）。`Rgba32Float` などの 3D テクスチャ用
    /// （RGB だけの浮動小数点のテクスチャ形式は GPU で使えないことが多いため）。
    pub fn to_rgba_f32(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.data.len() * 4);
        for c in &self.data {
            out.extend_from_slice(&[c[0], c[1], c[2], 1.0]);
        }
        out
    }

    #[inline]
    fn at(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.data[(b * self.size + g) * self.size + r]
    }

    /// 1 画素に適用する（四面体補間）。入力は 0〜1 に収めてから引く（NaN は 0）。
    #[inline]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n1 = (self.size - 1) as f32;
        let max_base = self.size - 2;
        let mut base = [0_usize; 3];
        let mut frac = [0.0_f32; 3];
        for k in 0..3 {
            let v = if rgb[k].is_nan() {
                0.0
            } else {
                rgb[k].clamp(0.0, 1.0)
            };
            let x = v * n1;
            let i = (x.floor() as usize).min(max_base);
            base[k] = i;
            frac[k] = x - i as f32;
        }
        let [r0, g0, b0] = base;
        let (r1, g1, b1) = (r0 + 1, g0 + 1, b0 + 1);
        let [fr, fg, fb] = frac;

        // 立方体の角 c000 から c111 への経路（2 番目と 3 番目の角）と重み。
        let (w0, p1, w1, p2, w2, w3);
        if fr >= fg {
            if fg >= fb {
                // fr ≥ fg ≥ fb: R → G → B
                (w0, p1, w1, p2, w2, w3) =
                    (1.0 - fr, (r1, g0, b0), fr - fg, (r1, g1, b0), fg - fb, fb);
            } else if fr >= fb {
                // fr ≥ fb > fg: R → B → G
                (w0, p1, w1, p2, w2, w3) =
                    (1.0 - fr, (r1, g0, b0), fr - fb, (r1, g0, b1), fb - fg, fg);
            } else {
                // fb > fr ≥ fg: B → R → G
                (w0, p1, w1, p2, w2, w3) =
                    (1.0 - fb, (r0, g0, b1), fb - fr, (r1, g0, b1), fr - fg, fg);
            }
        } else if fb >= fg {
            // fb ≥ fg > fr: B → G → R
            (w0, p1, w1, p2, w2, w3) = (1.0 - fb, (r0, g0, b1), fb - fg, (r0, g1, b1), fg - fr, fr);
        } else if fb >= fr {
            // fg > fb ≥ fr: G → B → R
            (w0, p1, w1, p2, w2, w3) = (1.0 - fg, (r0, g1, b0), fg - fb, (r0, g1, b1), fb - fr, fr);
        } else {
            // fg > fr > fb: G → R → B
            (w0, p1, w1, p2, w2, w3) = (1.0 - fg, (r0, g1, b0), fg - fr, (r1, g1, b0), fr - fb, fb);
        }
        let c0 = self.at(r0, g0, b0);
        let c1 = self.at(p1.0, p1.1, p1.2);
        let c2 = self.at(p2.0, p2.1, p2.2);
        let c3 = self.at(r1, g1, b1);
        [
            w0 * c0[0] + w1 * c1[0] + w2 * c2[0] + w3 * c3[0],
            w0 * c0[1] + w1 * c1[1] + w2 * c2[1] + w3 * c3[1],
            w0 * c0[2] + w1 * c1[2] + w2 * c2[2] + w3 * c3[2],
        ]
    }

    /// 画素の列にその場で適用する。
    pub fn apply_in_place(&self, pixels: &mut [[f32; 3]]) {
        for p in pixels {
            *p = self.apply(*p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{DisplayLutSource, DisplayProfile};
    use crate::icc::{IccVersion, StandardProfile};
    use crate::lab::delta_e2000;
    use crate::matrix::Mat3;
    use crate::space::{BT709_PRIMARIES, DISPLAY_P3_PRIMARIES};
    use crate::test_util::{rgb_to_lab, tabulated_srgb_transfer_profile};
    use crate::transfer::srgb_encode_f32;
    use lcms2::{CIExyY, CIExyYTRIPLE, Profile, ToneCurve};

    /// 決定的な疑似乱数（線形合同法）で 0〜1 の値を作る。
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
        }
        fn rgb(&mut self) -> [f32; 3] {
            [self.next(), self.next(), self.next()]
        }
    }

    /// 四面体補間の別の書き方（小数部を大きい順に並べて、角から 1 軸ずつ進む）。
    fn reference_tetrahedral(lut: &Lut3d, rgb: [f32; 3]) -> [f32; 3] {
        let n1 = (lut.size - 1) as f32;
        let mut base = [0usize; 3];
        let mut frac = [0.0f32; 3];
        for k in 0..3 {
            let x = rgb[k].clamp(0.0, 1.0) * n1;
            let i = (x.floor() as usize).min(lut.size - 2);
            base[k] = i;
            frac[k] = x - i as f32;
        }
        let mut order = [0usize, 1, 2];
        // 安定な並べ替え（同じ値なら R, G, B の順）。
        order.sort_by(|&a, &b| frac[b].partial_cmp(&frac[a]).unwrap());
        let mut pos = base;
        let mut out = [0.0f64; 3];
        let mut prev = 1.0f32;
        let add = |out: &mut [f64; 3], w: f32, p: [usize; 3]| {
            let c = lut.at(p[0], p[1], p[2]);
            for k in 0..3 {
                out[k] += w as f64 * c[k] as f64;
            }
        };
        for &axis in &order {
            add(&mut out, prev - frac[axis], pos);
            prev = frac[axis];
            pos[axis] += 1;
        }
        add(&mut out, prev, pos);
        out.map(|v| v as f32)
    }

    #[test]
    fn grid_input_with_degenerate_size_is_finite() {
        // 格子数 1 では 0 / 0 で NaN、格子数 0 では usize の桁あふれ（デバッグビルドではパニック）に
        // なっていた。公開の関数なので、どの格子数でも有限の値を返す。
        assert_eq!(Lut3d::grid_input(1, 0, 0, 0), [0.0; 3]);
        let v = Lut3d::grid_input(0, 0, 0, 0);
        assert!(v.iter().all(|x| x.is_finite()), "{v:?}");
        // 通常の格子数は index / (n − 1)。
        assert_eq!(Lut3d::grid_input(2, 1, 0, 1), [1.0, 0.0, 1.0]);
        assert_eq!(Lut3d::grid_input(33, 32, 16, 0), [1.0, 0.5, 0.0]);
    }

    #[test]
    fn size_limits() {
        assert!(matches!(
            Lut3d::identity(1),
            Err(ColorError::InvalidLutSize { .. })
        ));
        assert!(matches!(
            Lut3d::identity(MAX_LUT_SIZE + 1),
            Err(ColorError::InvalidLutSize { .. })
        ));
        assert!(Lut3d::identity(2).is_ok());
        assert!(matches!(
            Lut3d::from_data(3, vec![[0.0; 3]; 26]),
            Err(ColorError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn layout_is_r_fastest() {
        let lut = Lut3d::identity(4).unwrap();
        assert_eq!(lut.data().len(), 64);
        assert_eq!(lut.data()[1], [1.0 / 3.0, 0.0, 0.0]);
        assert_eq!(lut.data()[4], [0.0, 1.0 / 3.0, 0.0]);
        assert_eq!(lut.data()[16], [0.0, 0.0, 1.0 / 3.0]);
        assert_eq!(lut.as_flat().len(), 192);
        let rgba = lut.to_rgba_f32();
        assert_eq!(rgba.len(), 256);
        assert_eq!(&rgba[4..8], &[1.0 / 3.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn grid_points_are_exact() {
        let mut rng = Lcg(7);
        let lut = Lut3d::from_fn(9, |_| rng.rgb()).unwrap();
        for b in 0..9 {
            for g in 0..9 {
                for r in 0..9 {
                    let v = lut.apply(Lut3d::grid_input(9, r, g, b));
                    assert_eq!(v, lut.at(r, g, b), "({r},{g},{b})");
                }
            }
        }
    }

    #[test]
    fn affine_functions_are_reproduced_exactly() {
        // 四面体補間は 1 次関数（アフィン変換）を正確に再現する。
        let m = Mat3::from_rows([[0.9, 0.2, -0.1], [-0.3, 1.1, 0.2], [0.05, -0.2, 1.3]]).to_f32();
        let f = |v: [f32; 3]| {
            let o = m.apply(v);
            [o[0] + 0.01, o[1] - 0.02, o[2] + 0.03]
        };
        let lut = Lut3d::from_fn(17, f).unwrap();
        let mut rng = Lcg(42);
        for _ in 0..10_000 {
            let v = rng.rgb();
            let a = lut.apply(v);
            let e = f(v);
            for k in 0..3 {
                assert!((a[k] - e[k]).abs() < 2e-6, "{v:?}: {a:?} vs {e:?}");
            }
        }
    }

    #[test]
    fn matches_reference_formulation_including_ties() {
        let mut rng = Lcg(3);
        let lut = Lut3d::from_fn(5, |_| rng.rgb()).unwrap();
        let mut rng = Lcg(99);
        let mut inputs: Vec<[f32; 3]> = (0..5000).map(|_| rng.rgb()).collect();
        // 小数部が同じ値になる点（四面体の境界）と端。
        inputs.extend([
            [0.3, 0.3, 0.3],
            [0.3, 0.3, 0.1],
            [0.1, 0.3, 0.3],
            [0.3, 0.1, 0.3],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.5],
            [0.125, 0.125, 0.9],
        ]);
        for v in inputs {
            let a = lut.apply(v);
            let e = reference_tetrahedral(&lut, v);
            for k in 0..3 {
                assert!((a[k] - e[k]).abs() < 1e-5, "{v:?}: {a:?} vs {e:?}");
            }
        }
    }

    #[test]
    fn continuous_across_cells() {
        // 格子の境界の両側で値がつながる。
        let mut rng = Lcg(11);
        let lut = Lut3d::from_fn(5, |_| rng.rgb()).unwrap();
        let x = 0.5_f32; // 格子点 2/4
        let eps = 1e-6;
        for (g, b) in [(0.1_f32, 0.7_f32), (0.6, 0.2), (0.33, 0.33)] {
            let lo = lut.apply([x - eps, g, b]);
            let hi = lut.apply([x + eps, g, b]);
            for k in 0..3 {
                assert!((lo[k] - hi[k]).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn out_of_range_and_nan_inputs_are_clamped() {
        let mut rng = Lcg(5);
        let lut = Lut3d::from_fn(6, |_| rng.rgb()).unwrap();
        assert_eq!(lut.apply([-1.0, 2.0, 0.0]), lut.apply([0.0, 1.0, 0.0]));
        assert_eq!(lut.apply([f32::NAN, 0.5, 0.5]), lut.apply([0.0, 0.5, 0.5]));
        assert_eq!(
            lut.apply([f32::INFINITY, f32::NEG_INFINITY, 0.5]),
            lut.apply([1.0, 0.0, 0.5])
        );
        let v = lut.apply([f32::NAN; 3]);
        assert!(v.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn non_finite_grid_values_become_zero() {
        let lut = Lut3d::from_fn(2, |v| if v[0] > 0.5 { [f32::NAN; 3] } else { v }).unwrap();
        assert_eq!(lut.at(1, 0, 0), [0.0; 3]);
        let lut = Lut3d::from_data(2, vec![[f32::INFINITY, 0.0, 0.0]; 8]).unwrap();
        assert_eq!(lut.at(0, 0, 0), [0.0; 3]);
    }

    #[test]
    fn in_place_matches_single() {
        let mut rng = Lcg(8);
        let lut = Lut3d::from_fn(7, |_| rng.rgb()).unwrap();
        let src: Vec<[f32; 3]> = (0..100).map(|_| rng.rgb()).collect();
        let mut buf = src.clone();
        lut.apply_in_place(&mut buf);
        for (s, d) in src.iter().zip(&buf) {
            assert_eq!(lut.apply(*s), *d);
        }
    }

    #[test]
    fn srgb_to_srgb_lut_is_identity() {
        let srgb = IccProfile::standard(StandardProfile::Srgb).unwrap();
        let lut = Lut3d::from_icc(&srgb, &srgb, DEFAULT_LUT_SIZE).unwrap();
        let mut rng = Lcg(1);
        let mut max = 0.0_f32;
        for _ in 0..20_000 {
            let v = rng.rgb();
            let o = lut.apply(v);
            for k in 0..3 {
                max = max.max((o[k] - v[k]).abs());
            }
        }
        assert!(max < 1e-4, "最大の差 {max:e}");
        // 変換元 (a) どうしも恒等。
        let a = IccProfile::standard(StandardProfile::Bt2020SrgbTransfer).unwrap();
        let lut = Lut3d::from_icc(&a, &a, DEFAULT_LUT_SIZE).unwrap();
        for _ in 0..5_000 {
            let v = rng.rgb();
            let o = lut.apply(v);
            for k in 0..3 {
                assert!((o[k] - v[k]).abs() < 1e-4);
            }
        }
    }

    /// モニターのプロファイルの例（D50 寄りの白・独自の原色・ガンマ 1.8、ICC v2）。
    fn monitor_like_profile() -> IccProfile {
        let white = CIExyY {
            x: 0.3400,
            y: 0.3550,
            Y: 1.0,
        };
        let prim = CIExyYTRIPLE {
            Red: CIExyY {
                x: 0.660,
                y: 0.330,
                Y: 1.0,
            },
            Green: CIExyY {
                x: 0.250,
                y: 0.680,
                Y: 1.0,
            },
            Blue: CIExyY {
                x: 0.152,
                y: 0.065,
                Y: 1.0,
            },
        };
        let curve = ToneCurve::new(1.8);
        let mut p = Profile::new_rgb(&white, &prim, &[&curve, &curve, &curve]).unwrap();
        p.set_version(2.4);
        IccProfile::from_bytes(&p.icc().unwrap()).unwrap()
    }

    /// LUT と lcms2 の直接変換の差（ΔE2000）の統計。
    struct DeltaEStats {
        /// 直接変換の出力が 0〜1 に収まる（変換先の色域内の）点での最大。
        max_in_gamut: f64,
        /// そのうち L* ≥ 5 の点での最大。
        max_in_gamut_l5: f64,
        /// すべての点での最大。
        max_all: f64,
        /// 色域内の点の数。
        n_in_gamut: usize,
    }

    fn delta_e_vs_direct(
        src: &IccProfile,
        dst: &IccProfile,
        size: usize,
        n_random: usize,
    ) -> DeltaEStats {
        let lut = Lut3d::from_icc(src, dst, size).unwrap();
        let t = IccTransform::new(src, dst, RenderingIntent::RelativeColorimetric).unwrap();
        let mut rng = Lcg(2024);
        let mut inputs: Vec<[f32; 3]> = (0..n_random).map(|_| rng.rgb()).collect();
        // 暗部・無彩色・端を重点的に足す。
        for i in 0..=96 {
            let v = i as f32 / 96.0;
            inputs.push([v, v, v]);
            inputs.push([v * 0.1, v * 0.05, v * 0.08]);
            inputs.push([v, 0.0, 0.0]);
            inputs.push([0.0, v, 0.0]);
            inputs.push([0.0, 0.0, v]);
            inputs.push([0.0, v * 0.5, v]);
        }
        let mut direct = vec![[0.0_f32; 3]; inputs.len()];
        t.transform(&inputs, &mut direct).unwrap();
        let via_lut: Vec<[f32; 3]> = inputs.iter().map(|v| lut.apply(*v)).collect();
        let lab_d = rgb_to_lab(dst, &direct);
        let lab_l = rgb_to_lab(dst, &via_lut);
        // 色域内の判定には、変換先のリニアな RGB（ICC の行列で計算。変換元の伝達関数は
        // IEC 61966-2-1）も使う。変換先のトーンカーブが表（v2）だと直接変換の出力は 0〜1 に
        // 収められるため、出力だけでは色域の外の色を見分けられない（指摘 F36）。
        let src_to_dst_linear = dst.rgb_to_pcs_xyz_matrix().unwrap().inverse().unwrap()
            * src.rgb_to_pcs_xyz_matrix().unwrap();
        let mut st = DeltaEStats {
            max_in_gamut: 0.0,
            max_in_gamut_l5: 0.0,
            max_all: 0.0,
            n_in_gamut: 0,
        };
        for i in 0..inputs.len() {
            let de = delta_e2000(lab_d[i], lab_l[i]);
            st.max_all = st.max_all.max(de);
            let linear = src_to_dst_linear
                .apply(inputs[i].map(|v| crate::transfer::srgb_decode(f64::from(v))));
            if direct[i].iter().all(|v| (0.0..=1.0).contains(v))
                && linear.iter().all(|v| (0.0..=1.0).contains(v))
            {
                st.n_in_gamut += 1;
                st.max_in_gamut = st.max_in_gamut.max(de);
                if lab_d[i].l >= 5.0 {
                    st.max_in_gamut_l5 = st.max_in_gamut_l5.max(de);
                }
            }
        }
        st
    }

    #[test]
    fn display_lut_matches_direct_lcms_within_delta_e_1() {
        // PoC-1 の合格基準（仮置き）: 3D LUT と lcms2 の直接変換の差が ΔE2000 で最大 1 以下。
        // 変換先の伝達関数が IEC 61966-2-1 型（0 の近くが線形）なら、33³ ですべての色が満たす。
        // v2 の表のトーンカーブ（'curv'）の変換先も含める（指摘 F36）。
        let src = IccProfile::standard(StandardProfile::Bt2020SrgbTransfer).unwrap();
        let mut dsts = vec![
            ("sRGB", IccProfile::standard(StandardProfile::Srgb).unwrap()),
            (
                "Display P3",
                IccProfile::standard(StandardProfile::DisplayP3).unwrap(),
            ),
            (
                "リニア BT.2020",
                IccProfile::standard(StandardProfile::LinearBt2020).unwrap(),
            ),
        ];
        dsts.extend(tabulated_trc_destinations());
        for (name, dst) in &dsts {
            let st = delta_e_vs_direct(&src, dst, DEFAULT_LUT_SIZE, 20_000);
            assert!(
                st.n_in_gamut > 1000,
                "{name}: 比べた点が少ない（{}）",
                st.n_in_gamut
            );
            assert!(
                st.max_in_gamut <= 1.0,
                "{name}: 色域内の最大 ΔE2000 = {}",
                st.max_in_gamut
            );
            // 色域外も含めたすべての入力でも満たす（色域の圧縮をしない場合の目安）。
            assert!(
                st.max_all <= 1.0,
                "{name}: 全体の最大 ΔE2000 = {}",
                st.max_all
            );
        }
    }

    #[test]
    fn display_lut_pure_gamma_destination_known_limitation() {
        // 変換先の伝達関数が純粋なべき乗（Adobe RGB のガンマ 563/256、ガンマ 1.8 のモニター）だと、
        // 0 の近くで傾きが無限大になるため、最初の格子の区間の補間誤差が大きい。33³ では L* < 5 の
        // 暗部で ΔE2000 が 1 を超える（このコンテナでの測定値: 色域内で Adobe RGB 1.21、キャッシュ →
        // モニターの例 1.22、モニターの例 0.92。色域のわずかに外の暗い色も含めると 1.85・1.31）。
        // L* ≥ 5 では 1 以下。PoC-1 の合格基準をそのまま満たさない既知の制約として、モジュールの
        // doc に書いている。
        let src = IccProfile::standard(StandardProfile::Bt2020SrgbTransfer).unwrap();
        let p3 = IccProfile::standard(StandardProfile::DisplayP3).unwrap();
        let cases = [
            (
                "Adobe RGB",
                &src,
                IccProfile::standard(StandardProfile::AdobeRgb1998).unwrap(),
            ),
            ("モニターの例（ガンマ 1.8）", &src, monitor_like_profile()),
            // キャッシュ（B5、Display P3）を表示するときの LUT も同じ仕組みで作れる。
            ("キャッシュ → モニターの例", &p3, monitor_like_profile()),
        ];
        for (name, s, dst) in &cases {
            let st = delta_e_vs_direct(s, dst, DEFAULT_LUT_SIZE, 20_000);
            assert!(st.n_in_gamut > 1000, "{name}");
            assert!(
                st.max_in_gamut_l5 <= 1.0,
                "{name}: L* ≥ 5 の最大 ΔE2000 = {}",
                st.max_in_gamut_l5
            );
            // 暗部を含めた最大の上限（劣化の検出用）。
            assert!(
                st.max_in_gamut <= 2.5,
                "{name}: 色域内の最大 ΔE2000 = {}",
                st.max_in_gamut
            );
        }
    }

    /// v2 の表のトーンカーブ（'curv'）を持つモニターのプロファイルの例（指摘 F36）。
    fn tabulated_trc_destinations() -> Vec<(&'static str, IccProfile)> {
        vec![
            (
                "sRGB v2.4（4096 点の表）",
                IccProfile::standard_with_version(StandardProfile::Srgb, IccVersion::V2_4).unwrap(),
            ),
            (
                "sRGB v2.1（1024 点の表）",
                tabulated_srgb_transfer_profile(BT709_PRIMARIES, 1024),
            ),
            (
                "Display P3 v2.4（4096 点の表）",
                IccProfile::standard_with_version(StandardProfile::DisplayP3, IccVersion::V2_4)
                    .unwrap(),
            ),
            (
                "Display P3 v2.1（1024 点の表）",
                tabulated_srgb_transfer_profile(DISPLAY_P3_PRIMARIES, 1024),
            ),
        ]
    }

    /// genzo-pipeline の `DisplayTransform` と同じ手順（モニターの色域への色域の圧縮 →
    /// IEC 61966-2-1 で符号化 → LUT → 0〜1 に収める）の結果と、同じく色域の圧縮と符号化の後に
    /// LUT を通さず lcms2 で直接変換した結果の ΔE2000（変換先のプロファイルで Lab（D50）にして
    /// 計算）の最大。入力は B3（リニア BT.2020）の 0〜1 の一様な乱数。
    fn display_path_max_delta_e(dst: &IccProfile, size: usize, n_random: usize) -> f64 {
        let display = DisplayProfile::resolve(Some(dst.as_bytes())).unwrap();
        assert!(!display.is_assumed_srgb());
        let gamut = display.gamut().unwrap();
        let lut = display.build_lut(DisplayLutSource::Working, size).unwrap();
        let src = IccProfile::standard(StandardProfile::Bt2020SrgbTransfer).unwrap();
        let t = IccTransform::new(&src, dst, RenderingIntent::RelativeColorimetric).unwrap();
        let mut rng = Lcg(0x1234_5678);
        let encoded: Vec<[f32; 3]> = (0..n_random)
            .map(|_| gamut.compress_working(rng.rgb()).map(srgb_encode_f32))
            .collect();
        let via_lut: Vec<[f32; 3]> = encoded
            .iter()
            .map(|v| lut.apply(*v).map(|x| x.clamp(0.0, 1.0)))
            .collect();
        let mut direct = vec![[0.0_f32; 3]; encoded.len()];
        t.transform(&encoded, &mut direct).unwrap();
        let lab_d = rgb_to_lab(dst, &direct);
        let lab_l = rgb_to_lab(dst, &via_lut);
        lab_d
            .iter()
            .zip(&lab_l)
            .map(|(a, b)| delta_e2000(*a, *b))
            .fold(0.0, f64::max)
    }

    #[test]
    fn tabulated_trc_destination_grid_points_extend_outside_0_1() {
        // 指摘 F36: v2 の表のトーンカーブ（'curv'）の変換先では、lcms2 の変換の出力は 0〜1 に
        // 収められる（IccTransform の doc）。LUT の格子点まで収められると、モニターの色域の境界の
        // 近くの補間の誤差が増える。格子点は v4（'para'）の変換先と同じく 0〜1 の外の値を持つこと。
        let src = IccProfile::standard(StandardProfile::Bt2020SrgbTransfer).unwrap();
        // tabulated_trc_destinations の並びに対応する、v4（'para'）の標準のプロファイル。
        let kinds = [
            StandardProfile::Srgb,
            StandardProfile::Srgb,
            StandardProfile::DisplayP3,
            StandardProfile::DisplayP3,
        ];
        for ((name, dst), kind) in tabulated_trc_destinations().into_iter().zip(kinds) {
            let v4 = Lut3d::from_icc(&src, &IccProfile::standard(kind).unwrap(), 33).unwrap();
            let lut = Lut3d::from_icc(&src, &dst, 33).unwrap();
            // 格子点 (0, 1, 0)（BT.2020 の緑）は変換先の色域の外で、R・B が負になる。
            let g = lut.at(0, 32, 0);
            let g4 = v4.at(0, 32, 0);
            assert!(g[0] < -1.0 && g[2] < -0.1, "{name}: {g:?}（v4 は {g4:?}）");
            let mut outside = 0;
            for (a, b) in lut.data().iter().zip(v4.data()) {
                for k in 0..3 {
                    if b[k] < 0.0 {
                        // 0 より下: v4 の区分関数も表の延長も、線形の区間（傾き 12.92）の延長
                        // （1024 点の表は 16bit の量子化で、傾きが 0.7% ほど大きい）。
                        assert!(
                            (a[k] - b[k]).abs() <= 0.01 * b[k].abs() + 2e-3,
                            "{name}: {a:?} vs {b:?}"
                        );
                    } else if b[k] > 1.0 {
                        // 1 より上: v4 は冪の延長、表は 1 での傾きで線形に延長する（冪は上に凸なので、
                        // 線形の延長は v4 以上になる）。
                        assert!(a[k] >= b[k] - 2e-3, "{name}: {a:?} vs {b:?}");
                    } else {
                        assert!((a[k] - b[k]).abs() <= 2e-3, "{name}: {a:?} vs {b:?}");
                    }
                    if !(0.0..=1.0).contains(&a[k]) {
                        outside += 1;
                    }
                }
            }
            assert!(outside > 10_000, "{name}: 0〜1 の外の値 {outside}");
        }
    }

    #[test]
    fn display_path_matches_direct_lcms_for_tabulated_trc_destinations() {
        // 指摘 F36: DisplayTransform と同じ手順で、PoC-1 の合格基準（仮置き。ΔE2000 で最大 1 以下）を
        // 表のトーンカーブのモニターでも満たす。修正前は、格子点が 0〜1 に収められていたため、
        // 明るい色の色域の境界の近く（B3 = [0.998, 0.927, 0.697] など）で 1 を超えていた。
        // v4（'para'）の変換先も比べる（修正の前後で変わらない）。
        let mut dsts = tabulated_trc_destinations();
        dsts.push((
            "sRGB v4.3",
            IccProfile::standard(StandardProfile::Srgb).unwrap(),
        ));
        dsts.push((
            "Display P3 v4.3",
            IccProfile::standard(StandardProfile::DisplayP3).unwrap(),
        ));
        for (name, dst) in &dsts {
            let max = display_path_max_delta_e(dst, DEFAULT_LUT_SIZE, 60_000);
            assert!(max <= 1.0, "{name}: 最大 ΔE2000 = {max}");
        }
    }

    /// 格子数ごとの誤差の一覧を表示する（PoC-1 の検討用。`cargo test -p genzo-color --release
    /// report_lut_accuracy -- --ignored --nocapture`）。最後の列は、DisplayTransform と同じ手順
    /// （色域の圧縮 → 符号化 → LUT）で B3 の一様な乱数 30 万点を比べた最大（指摘 F36）。
    #[test]
    #[ignore]
    fn report_lut_accuracy() {
        let src = IccProfile::standard(StandardProfile::Bt2020SrgbTransfer).unwrap();
        let mut dsts = vec![
            ("sRGB", IccProfile::standard(StandardProfile::Srgb).unwrap()),
            (
                "Display P3",
                IccProfile::standard(StandardProfile::DisplayP3).unwrap(),
            ),
            (
                "Adobe RGB",
                IccProfile::standard(StandardProfile::AdobeRgb1998).unwrap(),
            ),
            (
                "リニア BT.2020",
                IccProfile::standard(StandardProfile::LinearBt2020).unwrap(),
            ),
            ("モニターの例（ガンマ 1.8）", monitor_like_profile()),
        ];
        dsts.extend(tabulated_trc_destinations());
        println!(
            "| 変換先 | 格子数 | 色域内の最大 | 色域内（L* ≥ 5）の最大 | 全体の最大 | 色域を圧縮した入力の最大 |"
        );
        for (name, dst) in &dsts {
            for size in [17, 33, 49, 65] {
                let st = delta_e_vs_direct(&src, dst, size, 200_000);
                let display = display_path_max_delta_e(dst, size, 300_000);
                println!(
                    "| {name} | {size} | {:.3} | {:.3} | {:.3} | {display:.3} |",
                    st.max_in_gamut, st.max_in_gamut_l5, st.max_all
                );
            }
        }
    }
}
