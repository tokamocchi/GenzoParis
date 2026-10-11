//! 3×3 の行列（色空間の変換に使う）。
//!
//! 行列の導出・合成・逆行列は f64 で計算し（丸め誤差を小さくするため）、画素ごとの処理には
//! [`Mat3::to_f32`] で取り出した [`Mat3F32`] を使う（04 の 2.3 節: 浮動小数点は 32bit で統一）。
//!
//! 行列は **行優先**（`rows[i][j]` が i 行 j 列）で、列ベクトルに左から掛ける
//! （`v' = M · v`）。色空間の変換 A → B → C は `M_bc * M_ab` の順に合成する。

use std::ops::Mul;

use crate::error::{ColorError, Result};

/// 逆行列を計算するときの、特異とみなす相対的な閾値（仮置き）。
///
/// `|det| ≤ 閾値 × (各行の要素の絶対値の最大の積)` なら特異とみなす。Hadamard の不等式から
/// この比は 0〜3√3 の範囲になり、0 に近いほど特異に近い。f64 の丸め誤差（約 2.2e-16）より
/// 十分大きく、色空間の行列（比は 0.01〜1 程度）より十分小さい値として選んだ。根拠のある値ではない。
pub const SINGULAR_RELATIVE_EPSILON: f64 = 1e-12;

/// 3×3 の行列（f64、行優先）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat3 {
    rows: [[f64; 3]; 3],
}

impl Mat3 {
    /// 単位行列。
    pub const IDENTITY: Mat3 = Mat3::diagonal([1.0, 1.0, 1.0]);

    /// 行の配列から作る（`rows[i][j]` が i 行 j 列）。
    pub const fn from_rows(rows: [[f64; 3]; 3]) -> Self {
        Self { rows }
    }

    /// 列ベクトルの配列から作る（`cols[j][i]` が i 行 j 列）。
    pub const fn from_cols(cols: [[f64; 3]; 3]) -> Self {
        Self::from_rows(cols).transpose()
    }

    /// 対角行列。
    pub const fn diagonal(d: [f64; 3]) -> Self {
        Self {
            rows: [[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]],
        }
    }

    /// 行の配列。
    pub const fn rows(&self) -> [[f64; 3]; 3] {
        self.rows
    }

    /// i 行目。
    ///
    /// # Panics
    ///
    /// `i >= 3` のとき。
    pub const fn row(&self, i: usize) -> [f64; 3] {
        self.rows[i]
    }

    /// i 行 j 列の要素。
    ///
    /// # Panics
    ///
    /// `i >= 3` または `j >= 3` のとき。
    pub const fn get(&self, i: usize, j: usize) -> f64 {
        self.rows[i][j]
    }

    /// 行列の積 `self · rhs`（`rhs` を先に適用する変換）。
    pub const fn mul_mat(&self, rhs: &Mat3) -> Mat3 {
        let a = &self.rows;
        let b = &rhs.rows;
        let mut out = [[0.0; 3]; 3];
        let mut i = 0;
        while i < 3 {
            let mut j = 0;
            while j < 3 {
                // 足す順序を固定する（a_i0 b_0j + a_i1 b_1j + a_i2 b_2j）。
                out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
                j += 1;
            }
            i += 1;
        }
        Mat3 { rows: out }
    }

    /// 列ベクトルに適用する（`self · v`）。
    pub const fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        let m = &self.rows;
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    }

    /// 転置。
    pub const fn transpose(&self) -> Mat3 {
        let m = &self.rows;
        Mat3 {
            rows: [
                [m[0][0], m[1][0], m[2][0]],
                [m[0][1], m[1][1], m[2][1]],
                [m[0][2], m[1][2], m[2][2]],
            ],
        }
    }

    /// 各要素に `s` を掛ける。
    pub const fn scale(&self, s: f64) -> Mat3 {
        let m = &self.rows;
        Mat3 {
            rows: [
                [m[0][0] * s, m[0][1] * s, m[0][2] * s],
                [m[1][0] * s, m[1][1] * s, m[1][2] * s],
                [m[2][0] * s, m[2][1] * s, m[2][2] * s],
            ],
        }
    }

    /// 行列式。
    pub const fn determinant(&self) -> f64 {
        let m = &self.rows;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// すべての要素が有限か。
    pub const fn is_finite(&self) -> bool {
        let mut i = 0;
        while i < 3 {
            let mut j = 0;
            while j < 3 {
                if !self.rows[i][j].is_finite() {
                    return false;
                }
                j += 1;
            }
            i += 1;
        }
        true
    }

    /// 逆行列。const の文脈で使うための版で、特異なら `None`。
    ///
    /// 判定は [`SINGULAR_RELATIVE_EPSILON`] による（[`Mat3::inverse`] と同じ）。
    pub const fn checked_inverse(&self) -> Option<Mat3> {
        if !self.is_finite() {
            return None;
        }
        let det = self.determinant();
        let scale = row_norm(self.rows[0]) * row_norm(self.rows[1]) * row_norm(self.rows[2]);
        if !det.is_finite()
            || scale.is_nan()
            || scale <= 0.0
            || det.abs() <= SINGULAR_RELATIVE_EPSILON * scale
        {
            return None;
        }
        let m = &self.rows;
        // 余因子行列の転置（随伴行列）を行列式で割る。
        let adj = [
            [
                m[1][1] * m[2][2] - m[1][2] * m[2][1],
                m[0][2] * m[2][1] - m[0][1] * m[2][2],
                m[0][1] * m[1][2] - m[0][2] * m[1][1],
            ],
            [
                m[1][2] * m[2][0] - m[1][0] * m[2][2],
                m[0][0] * m[2][2] - m[0][2] * m[2][0],
                m[0][2] * m[1][0] - m[0][0] * m[1][2],
            ],
            [
                m[1][0] * m[2][1] - m[1][1] * m[2][0],
                m[0][1] * m[2][0] - m[0][0] * m[2][1],
                m[0][0] * m[1][1] - m[0][1] * m[1][0],
            ],
        ];
        let inv = Mat3 { rows: adj }.scale(1.0 / det);
        if inv.is_finite() { Some(inv) } else { None }
    }

    /// 逆行列。特異（または特異に近い・有限でない要素を含む）ならエラー。
    ///
    /// 特異の判定は [`SINGULAR_RELATIVE_EPSILON`] を参照。
    pub fn inverse(&self) -> Result<Mat3> {
        if !self.is_finite() {
            return Err(ColorError::NonFinite { what: "行列" });
        }
        self.checked_inverse().ok_or(ColorError::SingularMatrix {
            determinant: self.determinant(),
        })
    }

    /// 各要素の差の絶対値の最大（テスト・検証用）。
    pub fn max_abs_diff(&self, other: &Mat3) -> f64 {
        let mut max = 0.0_f64;
        for i in 0..3 {
            for j in 0..3 {
                max = max.max((self.rows[i][j] - other.rows[i][j]).abs());
            }
        }
        max
    }

    /// f32 の行列として取り出す（画素ごとの処理と GPU 用）。
    pub const fn to_f32(&self) -> Mat3F32 {
        let m = &self.rows;
        Mat3F32 {
            rows: [
                [m[0][0] as f32, m[0][1] as f32, m[0][2] as f32],
                [m[1][0] as f32, m[1][1] as f32, m[1][2] as f32],
                [m[2][0] as f32, m[2][1] as f32, m[2][2] as f32],
            ],
        }
    }
}

/// 行の大きさの目安（各要素の絶対値の最大）。
///
/// const fn では `f64::sqrt` が使えないため、ユークリッドノルムの代わりに使う。
const fn row_norm(r: [f64; 3]) -> f64 {
    let a = r[0].abs();
    let b = r[1].abs();
    let c = r[2].abs();
    let m = if a > b { a } else { b };
    if m > c { m } else { c }
}

impl Mul for Mat3 {
    type Output = Mat3;

    fn mul(self, rhs: Mat3) -> Mat3 {
        self.mul_mat(&rhs)
    }
}

impl Mul<[f64; 3]> for Mat3 {
    type Output = [f64; 3];

    fn mul(self, rhs: [f64; 3]) -> [f64; 3] {
        self.apply(rhs)
    }
}

/// 3×3 の行列（f32、行優先）。[`Mat3::to_f32`] で作る。
///
/// 画素ごとの処理の CPU 基準実装で使う。演算の順序は [`Mat3F32::apply`] の doc のとおりに固定し、
/// GPU 版（WGSL）でも同じ順序で計算できるようにする（04 の 2.3 節）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat3F32 {
    rows: [[f32; 3]; 3],
}

impl Mat3F32 {
    /// 単位行列。
    pub const IDENTITY: Mat3F32 = Mat3::IDENTITY.to_f32();

    /// 行の配列から作る。
    pub const fn from_rows(rows: [[f32; 3]; 3]) -> Self {
        Self { rows }
    }

    /// 行の配列。
    pub const fn rows(&self) -> [[f32; 3]; 3] {
        self.rows
    }

    /// 列ベクトルに適用する。
    ///
    /// 各成分は `m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2]` を左から順に計算する
    /// （融合積和演算は使わない）。
    #[inline]
    pub fn apply(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.rows;
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    }

    /// WGSL の `mat3x3<f32>` を uniform / storage バッファに置くときの配置（列優先、各列を
    /// vec4 の大きさ（16 バイト）にそろえて末尾を 0 で埋めた 12 個の f32）。
    ///
    /// WGSL 側で `m * v` とすれば [`Mat3F32::apply`] と同じ変換になる。
    pub const fn to_wgsl_mat3x3(&self) -> [f32; 12] {
        let m = &self.rows;
        [
            m[0][0], m[1][0], m[2][0], 0.0, //
            m[0][1], m[1][1], m[2][1], 0.0, //
            m[0][2], m[1][2], m[2][2], 0.0,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Mat3 = Mat3::from_rows([[2.0, -1.0, 0.5], [0.25, 3.0, -2.0], [1.0, 0.0, 4.0]]);

    #[test]
    fn identity_is_neutral() {
        assert_eq!(A * Mat3::IDENTITY, A);
        assert_eq!(Mat3::IDENTITY * A, A);
        assert_eq!(Mat3::IDENTITY.apply([1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn product_matches_hand_computation() {
        let b = Mat3::from_rows([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]]);
        let p = A * b;
        // 1 行目: [2,-1,0.5] · 各列
        assert_eq!(
            p.row(0),
            [
                2.0 - 4.0 + 3.5, // 列 0: 1,4,7
                4.0 - 5.0 + 4.0, // 列 1: 2,5,8
                6.0 - 6.0 + 4.5  // 列 2: 3,6,9
            ]
        );
        // 積の適用は、順に適用したものと一致する。
        let v = [0.3, -0.7, 1.1];
        let lhs = p.apply(v);
        let rhs = A.apply(b.apply(v));
        for k in 0..3 {
            assert!((lhs[k] - rhs[k]).abs() < 1e-14);
        }
    }

    #[test]
    fn transpose_and_from_cols() {
        let t = A.transpose();
        for i in 0..3 {
            for j in 0..3 {
                assert_eq!(t.get(i, j), A.get(j, i));
            }
        }
        assert_eq!(Mat3::from_cols(A.rows()), t);
        assert_eq!(t.transpose(), A);
    }

    #[test]
    fn determinant_known_value() {
        // 2*(12-0) - (-1)*(1+2) + 0.5*(0-3) = 24 + 3 - 1.5
        assert!((A.determinant() - 25.5).abs() < 1e-12);
        assert_eq!(Mat3::IDENTITY.determinant(), 1.0);
    }

    #[test]
    fn inverse_roundtrip() {
        let inv = A.inverse().unwrap();
        assert!((A * inv).max_abs_diff(&Mat3::IDENTITY) < 1e-14);
        assert!((inv * A).max_abs_diff(&Mat3::IDENTITY) < 1e-14);
        let d = Mat3::diagonal([2.0, 4.0, 0.5]).inverse().unwrap();
        assert_eq!(d, Mat3::diagonal([0.5, 0.25, 2.0]));
    }

    #[test]
    fn singular_matrix_is_error() {
        let s = Mat3::from_rows([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 1.0, 1.0]]);
        assert!(matches!(
            s.inverse(),
            Err(ColorError::SingularMatrix { .. })
        ));
        let zero = Mat3::from_rows([[0.0; 3]; 3]);
        assert!(matches!(
            zero.inverse(),
            Err(ColorError::SingularMatrix { .. })
        ));
        // ほぼ特異（2 行目がほぼ 1 行目の 2 倍）もエラー。
        let near = Mat3::from_rows([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0 + 1e-13], [0.0, 1.0, 1.0]]);
        assert!(near.inverse().is_err());
        // 行ごとの尺度が極端でも、行どうしが独立なら特異ではない（相対的な判定）。
        let scaled = Mat3::diagonal([1.0, 1.0, 1e-14]);
        assert!(scaled.inverse().is_ok());
        // 小さいが特異ではない（スケールの問題だけ）は計算できる。
        let small = Mat3::IDENTITY.scale(1e-6);
        assert!((small.inverse().unwrap()).max_abs_diff(&Mat3::IDENTITY.scale(1e6)) < 1e-6);
    }

    #[test]
    fn non_finite_is_error() {
        let n = Mat3::from_rows([[f64::NAN, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!(matches!(n.inverse(), Err(ColorError::NonFinite { .. })));
        let i = Mat3::from_rows([[f64::INFINITY, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!(i.inverse().is_err());
        assert!(n.checked_inverse().is_none());
    }

    #[test]
    fn f32_extraction_and_apply() {
        let f = A.to_f32();
        let v = [0.25_f32, 0.5, -1.0];
        let r = f.apply(v);
        let r64 = A.apply([0.25, 0.5, -1.0]);
        for k in 0..3 {
            assert!((r[k] as f64 - r64[k]).abs() < 1e-6);
        }
        assert_eq!(Mat3F32::IDENTITY.apply(v), v);
    }

    #[test]
    fn wgsl_layout_is_column_major_padded() {
        let f = Mat3::from_rows([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]]).to_f32();
        assert_eq!(
            f.to_wgsl_mat3x3(),
            [1.0, 4.0, 7.0, 0.0, 2.0, 5.0, 8.0, 0.0, 3.0, 6.0, 9.0, 0.0]
        );
    }
}
