//! 変形（遠近補正。DEV-16。処理バージョン 2）: ステージ 9 の座標の変換に入れる射影変換。
//!
//! Lightroom Classic の「変形」のパネル（Upright の手動の項目）に相当する。設定は genzo-model の
//! `Transform`（垂直・水平・縦横比・拡大縮小・ずらし）で、歪曲補正の後の画像（D。[`crate::coords`]）を
//! **遠近補正の後の枠 P** に写し、その後に回転（`Geometry::angle_deg`）をかけて G にする（D → P → G）。
//! リサンプリングはステージ 9 の 1 回のまま（04 の 2.1 節）。
//!
//! # 式（AI の仮決定。値は仮置き）
//!
//! D の中心 `c = (ow / 2, oh / 2)` を原点、D の対角線の半分 `R` を 1 とした座標 `n = (d − c) / R` で考える
//! （レンズ補正の半径の基準と同じ）。
//!
//! 1. **遠近補正**: カメラを傾け直す回転 `R_ω` を、焦点距離 1（= 対角線の半分。フルサイズの約 43 mm に当たる）の
//!    ピンホールカメラの射影 `K · R_ω · K⁻¹`（`K = I`）として当てる: `[X, Y, W]ᵀ = R_ω · [x, y, 1]ᵀ`、
//!    `n' = (X / W, Y / W)`。回転のベクトルは `ω = (φ, ψ, 0)`（x 軸のまわり φ、y 軸のまわり ψ。z 軸のまわりの
//!    回転は角度補正が担う）で、`φ = −(垂直 / 100) · TILT_MAX_DEG`（負の値で上辺が広がる。見上げて撮った建物の
//!    すぼまりを直す向き）、`ψ = (水平 / 100) · TILT_MAX_DEG`（正の値で右辺が広がる）。合成した角度
//!    `|ω|` は [`TILT_TOTAL_MAX_DEG`] までに縮める（D の四隅（`|n| = 1`）で `W ≥ cos|ω| − sin|ω| > 0` を保ち、
//!    画像の中に消失線（`W = 0`）が入らないようにするため）。
//! 2. **中心をそろえる**: D の中心の像が P の中心に来るように平行移動する（傾け直すだけで画像が上下・左右に
//!    動かないように）。
//! 3. **縦横比・拡大縮小・ずらし**（アフィン変換）: 縦横比 `a`（−100〜+100）は幅を `k^−1/2`、高さを `k^1/2` 倍
//!    （`k = ASPECT_MAX_RATIO^(a / 100)`。正で縦に伸びる）、拡大縮小 `s`（50〜150 %）は両方を `s / 100` 倍、
//!    ずらし（−100〜+100）は幅・高さの半分に対する割合だけ平行移動する。
//! 4. 元の画素の単位に戻す: `p = c + R · n''`。
//!
//! P の枠は D と同じ中心・同じ尺度の座標で、D の四隅の像は（凸な）四角形になる。回転の後の G の外接矩形は
//! この四角形の外接矩形（[`crate::coords::CoordinateFrames::g_size_px`]）。四角形の外は画像のない部分で、
//! 回転と同じく切り抜きを画像の内側に制限し（[`crate::finish::constrain_crop`]）、切り抜く前の全体の表示では
//! 塗る。
//!
//! # 計算（GPU 版も同じ式。2.3 節）
//!
//! 行列は f64 で合成し、ステージ 9 は G の格子の座標から `[X, Y, W]` を f32 で求めて `W` で割る
//! （[`crate::finish::geometry::ProjectivePlan`]）。消失線の向こう側（`W ≤ 0`。G の外接矩形の四隅の、画像の外）は
//! `W` を [`W_MIN`] で下から止め、D の中心からの差を [`OUTSIDE_CLAMP_FACTOR`] × O の長辺に収める
//! （値に意味はないが、入力の範囲（`input_roi`）と鏡映の計算を有限に保つため）。
//!
//! # 自動の補正（Upright 相当）
//!
//! 画像の直線から垂直・水平・角度補正の値を求める処理は [`crate::upright`]。求めた値を設定に書き込むだけで、
//! 描画はこのモジュールの式で値だけを見る。

use genzo_model::Transform;

use crate::version::ProcessVersion;

/// 垂直・水平のスライダーの ±100 に当たる傾きの角度（度。**仮置き**）。
pub const TILT_MAX_DEG: f64 = 40.0;
/// 垂直と水平を合成した傾きの角度の上限（度。**仮置き**）。45 度未満なら、D の四隅で射影の分母が正に保たれる。
pub const TILT_TOTAL_MAX_DEG: f64 = 44.0;
/// 縦横比のスライダーの ±100 に当たる縦横比の倍率（**仮置き**）。
pub const ASPECT_MAX_RATIO: f64 = 1.5;
/// 射影の分母 `W` の下限（f32 の画素の計算。消失線の向こう側の点を有限に保つ）。
pub const W_MIN: f32 = 1e-4;
/// 消失線の向こう側の点の、D の中心からの差の上限（O の長辺に対する倍率）。
pub const OUTSIDE_CLAMP_FACTOR: f64 = 4.0;
/// 行列の逆を求めるときの行列式の下限（これより小さければ退化とみなす）。
const DET_MIN: f64 = 1e-18;

/// 3 × 3 の射影変換（同次座標。行ごとに `[[m00, m01, m02], [m10, m11, m12], [m20, m21, m22]]`）。
///
/// 点 `(x, y)` は `[X, Y, W]ᵀ = M · [x, y, 1]ᵀ`、`(X / W, Y / W)` に写る。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography(pub [[f64; 3]; 3]);

impl Homography {
    /// 恒等変換。
    pub const IDENTITY: Homography =
        Homography([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    /// 平行移動。
    pub fn translation(tx: f64, ty: f64) -> Self {
        Homography([[1.0, 0.0, tx], [0.0, 1.0, ty], [0.0, 0.0, 1.0]])
    }

    /// 拡大・縮小（原点のまわり）。
    pub fn scaling(sx: f64, sy: f64) -> Self {
        Homography([[sx, 0.0, 0.0], [0.0, sy, 0.0], [0.0, 0.0, 1.0]])
    }

    /// 原点のまわりの回転（画面の座標。y が下向きなので、`g = R·d`、`R = [[cos θ, sin θ], [−sin θ, cos θ]]` が
    /// 画面上の反時計回り。[`crate::coords`] の回転と同じ向き）。
    pub fn rotation(cos: f64, sin: f64) -> Self {
        Homography([[cos, sin, 0.0], [-sin, cos, 0.0], [0.0, 0.0, 1.0]])
    }

    /// アフィン変換 `[[a, b, t], [c, d, u]]`（行ごと）から。
    pub fn from_affine(a: [[f64; 3]; 2]) -> Self {
        Homography([a[0], a[1], [0.0, 0.0, 1.0]])
    }

    /// `self` を適用した後に `next` を適用する変換（`next · self`）。
    pub fn then(&self, next: &Homography) -> Homography {
        let (a, b) = (&next.0, &self.0);
        let mut m = [[0.0; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
            }
        }
        Homography(m)
    }

    /// 同次座標で適用する（`[X, Y, W]`）。
    #[inline]
    pub fn apply_h(&self, x: f64, y: f64) -> (f64, f64, f64) {
        let m = &self.0;
        (
            m[0][0] * x + m[0][1] * y + m[0][2],
            m[1][0] * x + m[1][1] * y + m[1][2],
            m[2][0] * x + m[2][1] * y + m[2][2],
        )
    }

    /// 点を写す。分母 `W` が正でなければ `None`（消失線の上・向こう側）。
    #[inline]
    pub fn apply_checked(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let (xx, yy, w) = self.apply_h(x, y);
        (w > 0.0 && w.is_finite()).then(|| (xx / w, yy / w))
    }

    /// 点を写す（分母は [`W_MIN`] で下から止める。有限の値が要る範囲の計算用）。
    #[inline]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let (xx, yy, w) = self.apply_h(x, y);
        let w = if w.is_finite() {
            w.max(f64::from(W_MIN))
        } else {
            f64::from(W_MIN)
        };
        (xx / w, yy / w)
    }

    /// 逆変換（余因子行列 ÷ 行列式）。行列式が小さすぎれば `None`。
    pub fn inverse(&self) -> Option<Homography> {
        let m = &self.0;
        let c00 = m[1][1] * m[2][2] - m[1][2] * m[2][1];
        let c01 = m[1][2] * m[2][0] - m[1][0] * m[2][2];
        let c02 = m[1][0] * m[2][1] - m[1][1] * m[2][0];
        let det = m[0][0] * c00 + m[0][1] * c01 + m[0][2] * c02;
        if !det.is_finite() || det.abs() < DET_MIN {
            return None;
        }
        let inv = 1.0 / det;
        Some(Homography([
            [
                c00 * inv,
                (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
                (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
            ],
            [
                c01 * inv,
                (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
                (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
            ],
            [
                c02 * inv,
                (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
                (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
            ],
        ]))
    }

    /// 点 `(x, y)` での分母 `W` が正になるように符号をそろえたもの（同次座標の尺度の自由度）。`W` が 0 なら
    /// そのまま。
    pub fn with_positive_w_at(&self, x: f64, y: f64) -> Homography {
        let (_, _, w) = self.apply_h(x, y);
        if w < 0.0 {
            Homography(self.0.map(|row| row.map(|v| -v)))
        } else {
            *self
        }
    }

    /// 3 行目が `[0, 0, 1]`（アフィン変換）か。
    pub fn is_affine(&self) -> bool {
        self.0[2] == [0.0, 0.0, 1.0]
    }

    /// f32 の係数（画素の計算用。GPU 版にも同じ値を渡す）。
    pub fn to_f32(&self) -> [[f32; 3]; 3] {
        self.0.map(|row| row.map(|v| v as f32))
    }
}

/// 傾きの角度（度）の組（x 軸のまわり・y 軸のまわり）を、スライダーの値から求める（合成の上限
/// [`TILT_TOTAL_MAX_DEG`] に縮める）。
pub fn tilt_angles_deg(vertical: f64, horizontal: f64) -> (f64, f64) {
    let phi = -(vertical / 100.0) * TILT_MAX_DEG;
    let psi = (horizontal / 100.0) * TILT_MAX_DEG;
    let total = phi.hypot(psi);
    if total > TILT_TOTAL_MAX_DEG {
        let k = TILT_TOTAL_MAX_DEG / total;
        (phi * k, psi * k)
    } else {
        (phi, psi)
    }
}

/// 回転のベクトル `ω = (φ, ψ, 0)`（度）の回転行列（Rodrigues の式）。
pub fn tilt_rotation(phi_deg: f64, psi_deg: f64) -> Homography {
    let (a, b) = (phi_deg.to_radians(), psi_deg.to_radians());
    let alpha = a.hypot(b);
    if alpha == 0.0 {
        return Homography::IDENTITY;
    }
    let (kx, ky) = (a / alpha, b / alpha);
    let (s, c) = alpha.sin_cos();
    let v = 1.0 - c;
    Homography([
        [1.0 - v * ky * ky, v * kx * ky, s * ky],
        [v * kx * ky, 1.0 - v * kx * kx, -s * kx],
        [-s * ky, s * kx, c],
    ])
}

/// 遠近補正（D → P。モジュールの doc の式）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Perspective {
    /// D（O の画素の座標）→ P（同じ中心・同じ尺度の座標）。D の中心で分母が正。
    pub d_to_p: Homography,
    /// P → D。P の中心（= D の中心）で分母が正。
    pub p_to_d: Homography,
}

impl Perspective {
    /// 丸めた設定から作る。版 1（遠近補正のない版）・何も変形しない設定なら `None`。
    pub fn for_settings(
        oriented_size: (u32, u32),
        version: ProcessVersion,
        transform: &Transform,
    ) -> Option<Self> {
        if !version.has_v1_features() || transform.is_identity() {
            return None;
        }
        Self::from_transform(oriented_size, transform)
    }

    /// 設定の値から作る（版は見ない。自動の補正の評価にも使う）。退化していれば `None`。
    pub fn from_transform(oriented_size: (u32, u32), t: &Transform) -> Option<Self> {
        let (phi, psi) = tilt_angles_deg(f64::from(t.vertical), f64::from(t.horizontal));
        Self::from_parts(
            oriented_size,
            (phi, psi),
            f64::from(t.aspect),
            f64::from(t.scale),
            (f64::from(t.offset_x), f64::from(t.offset_y)),
        )
    }

    /// 傾きの角度（度。x 軸・y 軸のまわり）と、縦横比（−100〜+100）・拡大縮小（%）・ずらし（−100〜+100）から
    /// 作る。
    pub fn from_parts(
        oriented_size: (u32, u32),
        tilt_deg: (f64, f64),
        aspect: f64,
        scale_percent: f64,
        offset: (f64, f64),
    ) -> Option<Self> {
        let (ow, oh) = (f64::from(oriented_size.0), f64::from(oriented_size.1));
        if !(ow > 0.0 && oh > 0.0) {
            return None;
        }
        let (cx, cy) = (ow * 0.5, oh * 0.5);
        let r = 0.5 * ow.hypot(oh);
        // 1. 正規化と傾け直し。
        let normalize =
            Homography::translation(-cx, -cy).then(&Homography::scaling(1.0 / r, 1.0 / r));
        let rot = tilt_rotation(tilt_deg.0, tilt_deg.1);
        // 2. 中心をそろえる。
        let (c0x, c0y) = rot.apply_checked(0.0, 0.0)?;
        let recenter = Homography::translation(-c0x, -c0y);
        // 3. 縦横比・拡大縮小・ずらし。
        let k = ASPECT_MAX_RATIO.powf(aspect / 100.0);
        let s = scale_percent / 100.0;
        let affine = Homography::scaling(s / k.sqrt(), s * k.sqrt()).then(
            &Homography::translation(offset.0 / 100.0 * (cx / r), offset.1 / 100.0 * (cy / r)),
        );
        // 4. 元の単位に戻す。
        let denormalize = Homography::scaling(r, r).then(&Homography::translation(cx, cy));
        let d_to_p = normalize
            .then(&rot)
            .then(&recenter)
            .then(&affine)
            .then(&denormalize)
            .with_positive_w_at(cx, cy);
        let p_to_d = d_to_p.inverse()?.with_positive_w_at(cx, cy);
        // D の四隅が消失線の手前にあること（傾きの上限で保証されるが、確かめる）。
        for (x, y) in [(0.0, 0.0), (ow, 0.0), (ow, oh), (0.0, oh)] {
            d_to_p.apply_checked(x, y)?;
        }
        Some(Self { d_to_p, p_to_d })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64), eps: f64) -> bool {
        (a.0 - b.0).abs() < eps && (a.1 - b.1).abs() < eps
    }

    #[test]
    fn homography_inverse_and_composition() {
        let h = Homography::translation(3.0, -2.0)
            .then(&Homography::scaling(2.0, 0.5))
            .then(&Homography::rotation(0.6_f64.cos(), 0.6_f64.sin()))
            .then(&Homography([
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.1, -0.05, 1.0],
            ]));
        let inv = h.inverse().unwrap();
        for (x, y) in [(0.0, 0.0), (1.5, -7.0), (100.0, 40.0)] {
            let p = h.apply_checked(x, y).unwrap();
            let back = inv.apply_checked(p.0, p.1).unwrap();
            assert!(close(back, (x, y), 1e-9), "{x} {y} → {p:?} → {back:?}");
        }
        assert!(!h.is_affine());
        assert!(
            Homography::translation(1.0, 2.0)
                .then(&Homography::scaling(3.0, 3.0))
                .is_affine()
        );
        // 退化した行列は逆を持たない。
        assert!(
            Homography([[1.0, 2.0, 0.0], [2.0, 4.0, 0.0], [0.0, 0.0, 1.0]])
                .inverse()
                .is_none()
        );
    }

    #[test]
    fn tilt_rotation_matches_the_single_axis_rotations() {
        let (s, c) = 20f64.to_radians().sin_cos();
        let rx = tilt_rotation(20.0, 0.0).0;
        let ry = tilt_rotation(0.0, 20.0).0;
        let eq = |a: f64, b: f64| (a - b).abs() < 1e-12;
        assert!(eq(rx[1][1], c) && eq(rx[1][2], -s) && eq(rx[2][1], s) && eq(rx[2][2], c));
        assert!(eq(rx[0][0], 1.0) && eq(rx[0][1], 0.0));
        assert!(eq(ry[0][0], c) && eq(ry[0][2], s) && eq(ry[2][0], -s) && eq(ry[2][2], c));
        assert!(eq(ry[1][1], 1.0));
        assert_eq!(tilt_rotation(0.0, 0.0), Homography::IDENTITY);
    }

    #[test]
    fn slider_signs_widen_the_expected_edges() {
        let size = (600u32, 400u32);
        // 垂直が負なら上辺が広がる（上の隅が中心から遠ざかる）。
        let p = Perspective::from_transform(
            size,
            &Transform {
                vertical: -50.0,
                ..Default::default()
            },
        )
        .unwrap();
        let top_left = p.d_to_p.apply_checked(0.0, 0.0).unwrap();
        let bottom_left = p.d_to_p.apply_checked(0.0, 400.0).unwrap();
        assert!(
            top_left.0 < 0.0 && top_left.0 < bottom_left.0 - 1.0,
            "{top_left:?} {bottom_left:?}"
        );
        assert!(bottom_left.0 > 0.0);
        // 中心は動かない。
        assert!(close(
            p.d_to_p.apply_checked(300.0, 200.0).unwrap(),
            (300.0, 200.0),
            1e-9
        ));
        // 水平が正なら右辺が広がる。
        let p = Perspective::from_transform(
            size,
            &Transform {
                horizontal: 50.0,
                ..Default::default()
            },
        )
        .unwrap();
        let top_right = p.d_to_p.apply_checked(600.0, 0.0).unwrap();
        let top_left = p.d_to_p.apply_checked(0.0, 0.0).unwrap();
        assert!(
            top_right.1 < 0.0 && top_left.1 > 0.0,
            "{top_right:?} {top_left:?}"
        );
        // 直線は直線のまま（左辺の中点は、両端の像の中点の近くにはないが、同じ直線の上）。
        let a = p.d_to_p.apply_checked(0.0, 0.0).unwrap();
        let b = p.d_to_p.apply_checked(0.0, 400.0).unwrap();
        let m = p.d_to_p.apply_checked(0.0, 100.0).unwrap();
        let cross = (b.0 - a.0) * (m.1 - a.1) - (b.1 - a.1) * (m.0 - a.0);
        assert!(cross.abs() < 1e-6, "{cross}");
    }

    #[test]
    fn affine_parts_scale_stretch_and_shift() {
        let size = (600u32, 400u32);
        let c = (300.0, 200.0);
        let p = Perspective::from_transform(
            size,
            &Transform {
                scale: 150.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(close(
            p.d_to_p.apply(600.0, 400.0),
            (c.0 + 450.0, c.1 + 300.0),
            1e-9
        ));
        let p = Perspective::from_transform(
            size,
            &Transform {
                aspect: 100.0,
                ..Default::default()
            },
        )
        .unwrap();
        let q = p.d_to_p.apply(600.0, 400.0);
        let ratio = ((q.1 - c.1) / 200.0) / ((q.0 - c.0) / 300.0);
        assert!((ratio - ASPECT_MAX_RATIO).abs() < 1e-9, "{ratio}");
        let p = Perspective::from_transform(
            size,
            &Transform {
                offset_x: 100.0,
                offset_y: -50.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(close(p.d_to_p.apply(300.0, 200.0), (600.0, 100.0), 1e-9));
        assert!(p.d_to_p.is_affine());
    }

    #[test]
    fn combined_tilt_is_limited_and_the_corners_stay_in_front_of_the_horizon() {
        let (phi, psi) = tilt_angles_deg(-100.0, 100.0);
        assert!((phi.hypot(psi) - TILT_TOTAL_MAX_DEG).abs() < 1e-9);
        assert!(phi > 0.0 && psi > 0.0);
        assert_eq!(tilt_angles_deg(-50.0, 0.0), (20.0, 0.0));
        for (w, h) in [(600u32, 400u32), (400, 600), (1000, 1000), (3000, 500)] {
            for (v, hz) in [
                (-100.0f32, 100.0f32),
                (100.0, -100.0),
                (100.0, 100.0),
                (-100.0, 0.0),
                (0.0, 100.0),
            ] {
                let t = Transform {
                    vertical: v,
                    horizontal: hz,
                    ..Default::default()
                };
                let p = Perspective::from_transform((w, h), &t).expect("作れる");
                let (ow, oh) = (f64::from(w), f64::from(h));
                for (x, y) in [(0.0, 0.0), (ow, 0.0), (ow, oh), (0.0, oh)] {
                    let q = p.d_to_p.apply_checked(x, y).unwrap();
                    let back = p.p_to_d.apply_checked(q.0, q.1).unwrap();
                    assert!(close(back, (x, y), 1e-6), "{w}×{h} {v} {hz}: {back:?}");
                }
            }
        }
        // 版 1・恒等の設定では作らない。
        assert!(
            Perspective::for_settings(
                (600, 400),
                ProcessVersion::V1,
                &Transform {
                    vertical: 10.0,
                    ..Default::default()
                }
            )
            .is_none()
        );
        assert!(
            Perspective::for_settings((600, 400), ProcessVersion::V2, &Transform::default())
                .is_none()
        );
        assert!(
            Perspective::for_settings(
                (600, 400),
                ProcessVersion::V2,
                &Transform {
                    vertical: 10.0,
                    ..Default::default()
                }
            )
            .is_some()
        );
    }
}
