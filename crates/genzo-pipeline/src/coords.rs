//! 座標系（docs/04_architecture.md の 2.7 節）と、RAW の向き（EXIF の Orientation）の扱い。
//!
//! | 名前 | 内容 |
//! |---|---|
//! | S：センサー座標 | CFA の画素の座標（フル解像度）。**センサー処理（ステージ 2〜8、段階 A1）の出力はこの向き** |
//! | O：向きを反映した座標 | S に RAW の向き（[`Orientation`]）を反映した画素の座標。画素の並べ替えだけ（補間なし） |
//! | G：ジオメトリ補正後の全体 | O にレンズの歪曲補正と回転（ステージ 9）を適用した、切り抜く前の画像全体。G の外接矩形の長辺を 1 とした値 |
//! | C：出力座標 | 切り抜いた後の画像。C → G（切り抜き）→ O → S の順にたどる |
//!
//! # G の定義
//!
//! - **回転**: G は O を **O の中心のまわりに** [`CoordinateFrames::rotation_deg`] 度だけ回転したもの
//!   （genzo-model の `Geometry::angle_deg`。正の値で画面上の反時計回り）。画面の座標は y が下向き
//!   なので、O の中心からの差 `d` は `g = R·d`、`R = [[cos θ, sin θ], [−sin θ, cos θ]]` で移り、
//!   逆は `d = Rᵀ·g`（[`CoordinateFrames::oriented_to_g`]・[`CoordinateFrames::g_to_oriented`]）。
//! - **外接矩形**: G の画素の座標（S の画素の単位）は、回転した O の外接矩形の左上を (0, 0) とする。
//!   外接矩形の寸法は `(ow·|cos θ| + oh·|sin θ|, ow·|sin θ| + oh·|cos θ|)`
//!   （[`CoordinateFrames::g_size_px`]）。外接矩形の中心は O の中心に対応する。
//!   回転が 0 なら G は O そのもの（計算も丸めなしで一致させる）。
//! - **歪曲補正**（v1）: G → O の変換に加える。そのときはアフィン変換でなくなるので、ステージ 9 は
//!   画素ごとに式を評価する形に変える（[`crate::finish::geometry`]）。
//! - **切り抜き**: genzo-model の `CropRect`（G の外接矩形の各軸の割合）。C の格子との対応は
//!   [`crate::finish::geometry::GeometryPlan`] が決める。
//! - **回転は段階 B の項目**: 回転を変えると G が変わるのでガイドを作り直す（2.7 節の表）。
//!   センサー処理（段階 A1）は回転を知らない（A1 のキャッシュのキーに回転を含めないため。その
//!   文脈の [`CoordinateFrames::rotation_deg`] は 0）。
//!
//! # 向きの扱い（決めごと）
//!
//! RAW の向き（`PhotoMetadata::orientation`）は、**G 座標を作る最初の変換**（S → O）として扱う
//! （切り抜きの座標は G で定義されるため。2.7 節）。センサー処理は向きを反映しない（S の向きの
//! まま出力する）。向きは 90 度単位の回転と反転なので、ステージ 9 で回転・歪曲補正・切り抜きと
//! まとめて 1 回のリサンプリングで反映する（2.1 節の補足「リサンプリングを 1 回だけ行う」）。
//! RAW 以外の入力（[`crate::sensor::SourceImage::Working`]）も、向きを反映する前の画素と向きを
//! 渡す。
//!
//! 連続座標では、画素 (x, y) は `[x, x + 1) × [y, y + 1)` を覆い、中心は `(x + 0.5, y + 0.5)`。

use genzo_model::Orientation;

use crate::error::{PipelineError, Result};
use crate::image::Roi;

/// S（向きを反映する前）と O（向きを反映した後）の対応（連続座標のアフィン変換）。
///
/// `[x, y] = m · [u, v] + t`（(u, v) が O、(x, y) が S）。`m` の要素は −1・0・1。
#[derive(Debug, Clone, Copy, PartialEq)]
struct OrientationMap {
    m: [[f64; 2]; 2],
    t: [f64; 2],
}

/// 向き `o` の、O → S の変換（S の寸法 `w × h`）。
///
/// 画素の対応は EXIF の定義（Exif 2.32 の Orientation タグ。genzo-media の向きの反映と同じ）。
fn orientation_map(o: Orientation, w: f64, h: f64) -> OrientationMap {
    let (m, t) = match o {
        Orientation::Normal => ([[1.0, 0.0], [0.0, 1.0]], [0.0, 0.0]),
        Orientation::FlipHorizontal => ([[-1.0, 0.0], [0.0, 1.0]], [w, 0.0]),
        Orientation::Rotate180 => ([[-1.0, 0.0], [0.0, -1.0]], [w, h]),
        Orientation::FlipVertical => ([[1.0, 0.0], [0.0, -1.0]], [0.0, h]),
        Orientation::Transpose => ([[0.0, 1.0], [1.0, 0.0]], [0.0, 0.0]),
        Orientation::Rotate90Cw => ([[0.0, 1.0], [-1.0, 0.0]], [0.0, h]),
        Orientation::Transverse => ([[0.0, -1.0], [-1.0, 0.0]], [w, h]),
        Orientation::Rotate270Cw => ([[0.0, -1.0], [1.0, 0.0]], [w, 0.0]),
    };
    OrientationMap { m, t }
}

impl OrientationMap {
    fn apply(&self, u: f64, v: f64) -> (f64, f64) {
        let m = &self.m;
        (
            m[0][0] * u + m[0][1] * v + self.t[0],
            m[1][0] * u + m[1][1] * v + self.t[1],
        )
    }

    /// 逆変換（S → O）。`m` は直交行列なので逆行列は転置。
    fn invert(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.m;
        let (dx, dy) = (x - self.t[0], y - self.t[1]);
        (m[0][0] * dx + m[1][0] * dy, m[0][1] * dx + m[1][1] * dy)
    }
}

/// 画像の座標系の情報（S の寸法・向きと、G を作る回転）。[`crate::stage::StageContext`] が持つ。
///
/// 回転は [`with_rotation`](Self::with_rotation) で加える（既定は 0。モジュールの doc の「G の定義」）。
/// 歪曲補正（v1）はまだ持たない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordinateFrames {
    /// S の幅（センサーのフル解像度。RAW 以外の入力では元の画像の幅）。
    pub sensor_width: u32,
    /// S の高さ。
    pub sensor_height: u32,
    /// RAW の向き（S → O）。
    pub orientation: Orientation,
    /// 角度補正（度。genzo-model の `Geometry::angle_deg`）。正の値で画面上の反時計回り。
    /// G は O をその中心のまわりにこの角度だけ回転したもの。センサー処理（段階 A1）の文脈では 0。
    pub rotation_deg: f64,
}

impl CoordinateFrames {
    /// 作る。寸法が 0 ならエラー。
    pub fn new(sensor_width: u32, sensor_height: u32, orientation: Orientation) -> Result<Self> {
        if sensor_width == 0 || sensor_height == 0 {
            return Err(PipelineError::InvalidDimensions {
                width: sensor_width,
                height: sensor_height,
            });
        }
        Ok(Self {
            sensor_width,
            sensor_height,
            orientation,
            rotation_deg: 0.0,
        })
    }

    /// 回転（度）を設定したもの。有限でなければエラー。−0 は 0 にそろえる。
    pub fn with_rotation(mut self, rotation_deg: f64) -> Result<Self> {
        if !rotation_deg.is_finite() {
            return Err(PipelineError::InvalidArgument(format!(
                "回転の角度が有限ではありません（{rotation_deg}）"
            )));
        }
        self.rotation_deg = if rotation_deg == 0.0 {
            0.0
        } else {
            rotation_deg
        };
        Ok(self)
    }

    /// 回転の (cos θ, sin θ)。回転が 0 なら厳密に (1, 0)。
    pub fn rotation_cos_sin(&self) -> (f64, f64) {
        if self.rotation_deg == 0.0 {
            return (1.0, 0.0);
        }
        let (s, c) = self.rotation_deg.to_radians().sin_cos();
        (c, s)
    }

    fn map(&self) -> OrientationMap {
        orientation_map(
            self.orientation,
            f64::from(self.sensor_width),
            f64::from(self.sensor_height),
        )
    }

    /// O の寸法（向きが 90 度・270 度の回転を含むなら幅と高さが入れ替わる）。
    pub fn oriented_size(&self) -> (u32, u32) {
        if self.orientation.swaps_dimensions() {
            (self.sensor_height, self.sensor_width)
        } else {
            (self.sensor_width, self.sensor_height)
        }
    }

    /// 連続座標で S → O。
    pub fn sensor_to_oriented(&self, x: f64, y: f64) -> (f64, f64) {
        self.map().invert(x, y)
    }

    /// 連続座標で O → S。
    pub fn oriented_to_sensor(&self, u: f64, v: f64) -> (f64, f64) {
        self.map().apply(u, v)
    }

    /// O の画素 (u, v) に当たる S の画素。O の範囲外なら `None`。
    pub fn oriented_pixel_to_sensor(&self, u: u32, v: u32) -> Option<(u32, u32)> {
        let (ow, oh) = self.oriented_size();
        if u >= ow || v >= oh {
            return None;
        }
        let (x, y) = self.oriented_to_sensor(f64::from(u) + 0.5, f64::from(v) + 0.5);
        Some((x.floor() as u32, y.floor() as u32))
    }

    /// O の画素の範囲を、それが覆う S の画素の範囲に変換する（向きは画素の並べ替えなので、範囲は
    /// 過不足なく対応する）。画像の外にはみ出した範囲もそのまま変換する。
    pub fn oriented_roi_to_sensor(&self, roi: Roi) -> Roi {
        let map = self.map();
        let (u0, v0) = (f64::from(roi.x), f64::from(roi.y));
        let (u1, v1) = (roi.x_end() as f64, roi.y_end() as f64);
        let a = map.apply(u0, v0);
        let b = map.apply(u1, v1);
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        Roi::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    /// G の外接矩形の長辺の長さ（S の画素の単位）。空間的なパラメータ（ぼかしの半径など）は
    /// この長さに対する割合で定義する（04 の 1.1 節の 5、2.7 節）。
    ///
    /// 回転した O の外接矩形の長辺（回転が 0 なら O の長辺）。歪曲補正（v1）は含まない。
    pub fn g_long_side_px(&self) -> f64 {
        let (w, h) = self.g_size_px();
        w.max(h)
    }

    /// G の外接矩形の寸法（幅, 高さ。S の画素の単位）。回転が 0 なら O の寸法そのもの。
    pub fn g_size_px(&self) -> (f64, f64) {
        let (ow, oh) = self.oriented_size();
        let (ow, oh) = (f64::from(ow), f64::from(oh));
        if self.rotation_deg == 0.0 {
            return (ow, oh);
        }
        let (c, s) = self.rotation_cos_sin();
        let (c, s) = (c.abs(), s.abs());
        (ow * c + oh * s, ow * s + oh * c)
    }

    /// 連続座標で O → G（O の中心のまわりに回転し、外接矩形の左上を原点にする）。
    pub fn oriented_to_g(&self, u: f64, v: f64) -> (f64, f64) {
        if self.rotation_deg == 0.0 {
            return (u, v);
        }
        let (ow, oh) = self.oriented_size();
        let (gw, gh) = self.g_size_px();
        let (c, s) = self.rotation_cos_sin();
        let (dx, dy) = (u - f64::from(ow) * 0.5, v - f64::from(oh) * 0.5);
        (c * dx + s * dy + gw * 0.5, -s * dx + c * dy + gh * 0.5)
    }

    /// 連続座標で G → O（[`oriented_to_g`](Self::oriented_to_g) の逆）。
    pub fn g_to_oriented(&self, gx: f64, gy: f64) -> (f64, f64) {
        if self.rotation_deg == 0.0 {
            return (gx, gy);
        }
        let (ow, oh) = self.oriented_size();
        let (gw, gh) = self.g_size_px();
        let (c, s) = self.rotation_cos_sin();
        let (dx, dy) = (gx - gw * 0.5, gy - gh * 0.5);
        (
            c * dx - s * dy + f64::from(ow) * 0.5,
            s * dx + c * dy + f64::from(oh) * 0.5,
        )
    }

    /// 連続座標で G → S。
    pub fn g_to_sensor(&self, gx: f64, gy: f64) -> (f64, f64) {
        let (u, v) = self.g_to_oriented(gx, gy);
        self.oriented_to_sensor(u, v)
    }

    /// 連続座標で S → G。
    pub fn sensor_to_g(&self, x: f64, y: f64) -> (f64, f64) {
        let (u, v) = self.sensor_to_oriented(x, y);
        self.oriented_to_g(u, v)
    }

    /// G → S の変換をアフィン変換 `[x, y] = m · [gx, gy] + t` として返す（`[m, t]` を行ごとに
    /// `[[m00, m01, t0], [m10, m11, t1]]`）。歪曲補正がない間は G → S はアフィン変換。
    ///
    /// 行列は記号的に合成する（回転が 0 で向きが通常なら、厳密に単位行列と 0）。
    pub fn g_to_sensor_affine(&self) -> [[f64; 3]; 2] {
        let (c, s) = self.rotation_cos_sin();
        let (ow, oh) = self.oriented_size();
        let (gw, gh) = self.g_size_px();
        // G → O: o = Rᵀ·(g − gc) + oc = Rᵀ·g + (oc − Rᵀ·gc)。
        let rt = [[c, -s], [s, c]];
        let (gcx, gcy) = (gw * 0.5, gh * 0.5);
        let (ocx, ocy) = (f64::from(ow) * 0.5, f64::from(oh) * 0.5);
        let t_o = if self.rotation_deg == 0.0 {
            [0.0, 0.0]
        } else {
            [
                ocx - (rt[0][0] * gcx + rt[0][1] * gcy),
                ocy - (rt[1][0] * gcx + rt[1][1] * gcy),
            ]
        };
        // O → S: s = m·o + t。
        let om = self.map();
        let m = [
            [
                om.m[0][0] * rt[0][0] + om.m[0][1] * rt[1][0],
                om.m[0][0] * rt[0][1] + om.m[0][1] * rt[1][1],
            ],
            [
                om.m[1][0] * rt[0][0] + om.m[1][1] * rt[1][0],
                om.m[1][0] * rt[0][1] + om.m[1][1] * rt[1][1],
            ],
        ];
        let t = [
            om.m[0][0] * t_o[0] + om.m[0][1] * t_o[1] + om.t[0],
            om.m[1][0] * t_o[0] + om.m[1][1] * t_o[1] + om.t[1],
        ];
        [[m[0][0], m[0][1], t[0]], [m[1][0], m[1][1], t[1]]]
    }
}

/// ステージ 9 の入力の格子（S 座標を一定の倍率で標本化した格子。2.7 節）。
///
/// 格子は S の範囲 `[0, extent.0) × [0, extent.1)` を、四隅を合わせて `width × height` 画素で覆う
/// （画素 (i, j) は S の `[i·extent.0/width, (i+1)·extent.0/width) × …`）。
///
/// | 入力 | 寸法 | extent |
/// |---|---|---|
/// | フル解像度のタイル（等倍・書き出し） | S の寸法 | S の寸法 |
/// | 2 × 2 の簡易処理のタイル | S の半分（切り捨て） | 寸法の 2 倍（S の幅・高さが奇数なら最後の列・行を含まない） |
/// | 段階 A1 のプレビュー（[`crate::A1Image`]） | プレビューの寸法 | [`crate::A1Image::sensor_extent`] |
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorGrid {
    /// 格子の幅（画素）。
    pub width: u32,
    /// 格子の高さ（画素）。
    pub height: u32,
    /// 格子が覆う S の範囲の幅・高さ（S の画素の単位。左上は S の (0, 0)）。
    pub extent: (f64, f64),
}

impl SensorGrid {
    /// 作る。寸法が 0、または extent が正の有限の値でなければエラー。
    pub fn new(width: u32, height: u32, extent: (f64, f64)) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(PipelineError::InvalidDimensions { width, height });
        }
        let ok = |v: f64| v.is_finite() && v > 0.0;
        if !(ok(extent.0) && ok(extent.1)) {
            return Err(PipelineError::InvalidArgument(format!(
                "格子が覆う範囲が不正です（{} × {}）",
                extent.0, extent.1
            )));
        }
        Ok(Self {
            width,
            height,
            extent,
        })
    }

    /// フル解像度（S の画素がそのまま格子の画素）。
    pub fn full_resolution(frames: &CoordinateFrames) -> Self {
        Self {
            width: frames.sensor_width,
            height: frames.sensor_height,
            extent: (
                f64::from(frames.sensor_width),
                f64::from(frames.sensor_height),
            ),
        }
    }

    /// S の 1 画素あたりの格子の画素の数（横, 縦）。
    pub fn density(&self) -> (f64, f64) {
        (
            f64::from(self.width) / self.extent.0,
            f64::from(self.height) / self.extent.1,
        )
    }

    /// 長辺で見た、フル解像度に対する倍率（[`crate::stage::StageContext::resolution_scale`] の値）。
    pub fn resolution_scale(&self) -> f64 {
        f64::from(self.width.max(self.height)) / self.extent.0.max(self.extent.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Orientation; 8] = [
        Orientation::Normal,
        Orientation::FlipHorizontal,
        Orientation::Rotate180,
        Orientation::FlipVertical,
        Orientation::Transpose,
        Orientation::Rotate90Cw,
        Orientation::Transverse,
        Orientation::Rotate270Cw,
    ];

    /// 3×2 の S の画像（画素 (x, y) の値 10y + x）を、向きを反映して表示したときの値の並び。
    fn displayed(o: Orientation) -> Vec<Vec<u32>> {
        let f = CoordinateFrames::new(3, 2, o).unwrap();
        let (ow, oh) = f.oriented_size();
        (0..oh)
            .map(|v| {
                (0..ow)
                    .map(|u| {
                        let (x, y) = f.oriented_pixel_to_sensor(u, v).unwrap();
                        10 * y + x
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn each_orientation_matches_exif() {
        // S:
        //  0  1  2
        // 10 11 12
        assert_eq!(
            displayed(Orientation::Normal),
            [vec![0, 1, 2], vec![10, 11, 12]]
        );
        assert_eq!(
            displayed(Orientation::FlipHorizontal),
            [vec![2, 1, 0], vec![12, 11, 10]]
        );
        assert_eq!(
            displayed(Orientation::Rotate180),
            [vec![12, 11, 10], vec![2, 1, 0]]
        );
        assert_eq!(
            displayed(Orientation::FlipVertical),
            [vec![10, 11, 12], vec![0, 1, 2]]
        );
        // 5: 転置。
        assert_eq!(
            displayed(Orientation::Transpose),
            [vec![0, 10], vec![1, 11], vec![2, 12]]
        );
        // 6: 時計回りに 90 度（0 行目が右辺）。
        assert_eq!(
            displayed(Orientation::Rotate90Cw),
            [vec![10, 0], vec![11, 1], vec![12, 2]]
        );
        // 7: 右上と左下を結ぶ対角線で反転。
        assert_eq!(
            displayed(Orientation::Transverse),
            [vec![12, 2], vec![11, 1], vec![10, 0]]
        );
        // 8: 反時計回りに 90 度（0 行目が左辺）。
        assert_eq!(
            displayed(Orientation::Rotate270Cw),
            [vec![2, 12], vec![1, 11], vec![0, 10]]
        );
    }

    #[test]
    fn continuous_mapping_round_trips() {
        for o in ALL {
            let f = CoordinateFrames::new(7, 4, o).unwrap();
            for (x, y) in [(0.0, 0.0), (7.0, 4.0), (1.25, 3.5), (6.9, 0.1)] {
                let (u, v) = f.sensor_to_oriented(x, y);
                let (ow, oh) = f.oriented_size();
                assert!((0.0..=f64::from(ow)).contains(&u) && (0.0..=f64::from(oh)).contains(&v));
                let (x2, y2) = f.oriented_to_sensor(u, v);
                assert!((x - x2).abs() < 1e-12 && (y - y2).abs() < 1e-12, "{o:?}");
            }
        }
    }

    #[test]
    fn roi_mapping_covers_the_same_pixels() {
        for o in ALL {
            let f = CoordinateFrames::new(7, 4, o).unwrap();
            let (ow, oh) = f.oriented_size();
            let roi = Roi::new(1, 0, ow - 2, oh - 1);
            let s = f.oriented_roi_to_sensor(roi);
            assert_eq!(s.pixel_count(), roi.pixel_count(), "{o:?}");
            for v in roi.y..roi.y + roi.height as i32 {
                for u in roi.x..roi.x + roi.width as i32 {
                    let (x, y) = f.oriented_pixel_to_sensor(u as u32, v as u32).unwrap();
                    assert!(s.contains(i64::from(x), i64::from(y)), "{o:?}");
                }
            }
            assert_eq!(f.oriented_roi_to_sensor(Roi::full(ow, oh)), Roi::full(7, 4));
        }
    }

    #[test]
    fn rotation_maps_round_trip_and_bounding_box() {
        for o in ALL {
            for deg in [0.0, 10.0, -33.5, 45.0] {
                let f = CoordinateFrames::new(60, 40, o)
                    .unwrap()
                    .with_rotation(deg)
                    .unwrap();
                let (gw, gh) = f.g_size_px();
                // O の四隅は G の外接矩形の内側（辺の上を含む）にある。
                let (ow, oh) = f.oriented_size();
                let mut min = (f64::INFINITY, f64::INFINITY);
                let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY);
                for (u, v) in [
                    (0.0, 0.0),
                    (f64::from(ow), 0.0),
                    (0.0, f64::from(oh)),
                    (f64::from(ow), f64::from(oh)),
                ] {
                    let (gx, gy) = f.oriented_to_g(u, v);
                    min = (min.0.min(gx), min.1.min(gy));
                    max = (max.0.max(gx), max.1.max(gy));
                    let (u2, v2) = f.g_to_oriented(gx, gy);
                    assert!((u - u2).abs() < 1e-9 && (v - v2).abs() < 1e-9);
                }
                assert!(
                    min.0.abs() < 1e-9 && min.1.abs() < 1e-9,
                    "{o:?} {deg}: {min:?}"
                );
                assert!((max.0 - gw).abs() < 1e-9 && (max.1 - gh).abs() < 1e-9);
                // アフィン変換の形の G → S は、関数の形と一致する。
                let a = f.g_to_sensor_affine();
                for (gx, gy) in [(0.0, 0.0), (3.5, 17.25), (gw, gh)] {
                    let (x, y) = f.g_to_sensor(gx, gy);
                    let xa = a[0][0] * gx + a[0][1] * gy + a[0][2];
                    let ya = a[1][0] * gx + a[1][1] * gy + a[1][2];
                    assert!((x - xa).abs() < 1e-9 && (y - ya).abs() < 1e-9);
                    let (gx2, gy2) = f.sensor_to_g(x, y);
                    assert!((gx - gx2).abs() < 1e-9 && (gy - gy2).abs() < 1e-9);
                }
            }
        }
    }

    #[test]
    fn positive_rotation_is_counter_clockwise_on_screen() {
        let f = CoordinateFrames::new(100, 100, Orientation::Normal)
            .unwrap()
            .with_rotation(90.0)
            .unwrap();
        // 中心の右の点は、反時計回りに 90 度回すと中心の上（y が小さい）に来る。
        let (gx, gy) = f.oriented_to_g(80.0, 50.0);
        assert!(
            (gx - 50.0).abs() < 1e-9 && (gy - 20.0).abs() < 1e-9,
            "{gx} {gy}"
        );
        // 回転 0 は厳密に恒等。
        let f0 = CoordinateFrames::new(7, 3, Orientation::Normal).unwrap();
        assert_eq!(f0.g_to_sensor_affine(), [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(f0.g_size_px(), (7.0, 3.0));
        assert!(f0.with_rotation(f64::NAN).is_err());
        assert!(
            f0.with_rotation(-0.0)
                .unwrap()
                .rotation_deg
                .is_sign_positive()
        );
        // 外接矩形は回転で大きくなる。
        let r = f0.with_rotation(30.0).unwrap();
        let (c, s) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
        let (gw, gh) = r.g_size_px();
        assert!(
            (gw - (7.0 * c + 3.0 * s)).abs() < 1e-12 && (gh - (7.0 * s + 3.0 * c)).abs() < 1e-12
        );
        assert_eq!(r.g_long_side_px(), gw);
    }

    #[test]
    fn sensor_grid_density_and_scale() {
        let f = CoordinateFrames::new(7008, 4672, Orientation::Normal).unwrap();
        let full = SensorGrid::full_resolution(&f);
        assert_eq!(full.density(), (1.0, 1.0));
        assert_eq!(full.resolution_scale(), 1.0);
        let preview = SensorGrid::new(2560, 1707, (7008.0, 4672.0)).unwrap();
        assert!((preview.resolution_scale() - 2560.0 / 7008.0).abs() < 1e-12);
        assert!(SensorGrid::new(0, 1, (1.0, 1.0)).is_err());
        assert!(SensorGrid::new(1, 1, (0.0, 1.0)).is_err());
        assert!(SensorGrid::new(1, 1, (f64::NAN, 1.0)).is_err());
    }

    #[test]
    fn sizes_and_long_side() {
        let f = CoordinateFrames::new(6000, 4000, Orientation::Rotate90Cw).unwrap();
        assert_eq!(f.oriented_size(), (4000, 6000));
        assert_eq!(f.g_long_side_px(), 6000.0);
        assert_eq!(f.oriented_pixel_to_sensor(4000, 0), None);
        assert!(CoordinateFrames::new(0, 1, Orientation::Normal).is_err());
    }
}
