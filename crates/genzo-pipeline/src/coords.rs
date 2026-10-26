//! 座標系（docs/04_architecture.md の 2.7 節）と、RAW の向き（EXIF の Orientation）の扱い。
//!
//! | 名前 | 内容 |
//! |---|---|
//! | S：センサー座標 | CFA の画素の座標（フル解像度。RAW の有効画素の全体）。**センサー処理（ステージ 2〜8、段階 A1）の出力はこの向き**。デモザイクは有効画素の全体で行う |
//! | 既定の切り抜き | S の中の矩形（[`CoordinateFrames::default_crop`]。RAW の DefaultCrop。genzo-raw の `RawImage::default_crop`）。カメラの JPEG と同じ範囲で、O・G の基準の枠（「画像全体」）になる。ない場合は S の全体 |
//! | O：向きを反映した座標 | S の既定の切り抜きの範囲に RAW の向き（[`Orientation`]）を反映した画素の座標（左上は既定の切り抜きの左上の画素）。画素の並べ替えと平行移動だけ（補間なし） |
//! | D：歪曲補正後の枠（この crate の名前） | O にレンズの歪曲補正（v1。[`crate::lens::RadialDistortion`]）を適用した、回転の前の画像。枠は O と同じ大きさで、O の内側から取れる最大の範囲に自動で拡大・縮小する。歪曲補正がなければ O そのもの |
//! | P：遠近補正後の枠（この crate の名前。2026-10-26 追加。DEV-16） | D に変形（遠近補正。[`crate::perspective::Perspective`]。処理バージョン 2）を適用した、回転の前の画像。D と同じ中心・同じ尺度の座標で、D の四隅の像は凸な四角形（その外は画像のない部分）。変形がなければ D そのもの |
//! | G：ジオメトリ補正後の全体 | P（D に遠近補正）に回転（ステージ 9）を適用した、切り抜く前の画像全体。G の外接矩形の長辺を 1 とした値 |
//! | C：出力座標 | 切り抜いた後の画像。C → G（切り抜き）→ O → S の順にたどる |
//!
//! # 既定の切り抜き（実機の ARW の不具合 A-2）
//!
//! α7 IV の APS-C の ARW では、LibRaw の有効画素の右端 40 列が詰め物（2 列左の値の繰り返し）で、
//! 下から 3 行目の B が異常な値になっている。どちらもカメラが記録した既定の切り抜き（DefaultCrop）の
//! 外にある。そこで、S（センサー処理の範囲）は有効画素の全体のまま、O・G を既定の切り抜きの範囲に
//! する。
//!
//! - 利用者の切り抜き（genzo-model の `CropRect`）・回転の中心・空間的なパラメータの長さ
//!   （[`CoordinateFrames::g_long_side_px`]）・ガイド（[`crate::guide`]）は、既定の切り抜きの範囲を
//!   基準にする。書き出しの「元の寸法」も既定の切り抜きの寸法になる。
//! - 既定の切り抜きのすぐ外の画素（補間のタップ）は、S の実際の画素を使う（鏡映で埋めるのは S の
//!   外だけ）。α7 IV の APS-C では、既定の切り抜きと詰め物の列の間に実際の画像が 44 列、異常な行
//!   との間に 27 行あるので、デモザイク（RCD）とバイキュービックの近傍に入らない。
//! - 回転したときの切り抜きの制限（[`crate::finish::geometry::constrain_crop`]）も、既定の切り抜きの
//!   範囲（O）の内側に制限する。
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
//! - **歪曲補正**（v1。処理バージョン 2。DEV-14）: G → D（回転）の後の D → O（半径方向の写像。
//!   [`CoordinateFrames::distortion`]）。そのときは G → S がアフィン変換でなくなるので、ステージ 9 は
//!   画素ごとに「G の格子 → D（アフィン）→ 半径方向の倍率（表）→ 入力の格子（アフィン）」を評価する
//!   （[`crate::finish::geometry`]）。D は O と同じ大きさの枠で、画像のない部分を持たない（[`crate::lens`] の
//!   「歪曲の後の枠」）。そのため、回転の外接矩形・切り抜きの制限・切り抜く前の全体の表示の「画像の外」は
//!   D の枠で判定する（[`CoordinateFrames::g_to_undistorted`]）。プロファイルの歪曲（写真ごとに決まる）は
//!   [`CoordinateFrames::lens_geometry`] に入れて段階 A1 から段階 B・C へ渡し、適用量・手動の補正と合わせた
//!   写像は段階 B・C の文脈で作る（[`crate::finish::finish_context`]・[`crate::guide::compute_guide`]）。
//! - **遠近補正**（2026-10-26 追加。DEV-16。処理バージョン 2）: D → P の射影変換（[`crate::perspective`]）を回転の前に
//!   入れる。G は P を **D の中心のまわりに** 回転し、P の四角形（D の四隅の像）の外接矩形の左上を原点にしたもの
//!   （[`CoordinateFrames::with_perspective`]。D → G の対応は 3 × 3 の行列 [`CoordinateFrames::undistorted_to_g_homography`]
//!   に合成し、回転だけのときの式とは別に持つ）。外接矩形の中心は、ふつう D の中心に対応しない。遠近補正があるときは
//!   G → S が射影変換になるので、ステージ 9 は画素ごとに「G の格子 → D（射影）→ 半径方向の倍率（表）→ 入力の格子
//!   （アフィン）」を評価する（[`crate::finish::geometry::ProjectivePlan`]）。遠近補正がなければ、これまでの式を
//!   そのまま使う（処理バージョン 1 と、変形のない設定の結果は変えない）。
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
use genzo_raw::PixelRect;

use crate::error::{PipelineError, Result};
use crate::image::Roi;
use crate::lens::{ProfileDistortion, RadialDistortion};
use crate::perspective::{Homography, Perspective};

/// 遠近補正（D → P。[`crate::perspective`]）と、回転・外接矩形を含めて合成した D → G の対応
/// （[`CoordinateFrames::perspective`]。回転か既定の切り抜きを変えると作り直す）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerspectiveFrame {
    /// 遠近補正（D → P）。
    pub perspective: Perspective,
    /// D（O の画素の座標）→ G（外接矩形の左上が原点。S の画素の単位）。D の中心で分母が正。
    d_to_g: Homography,
    /// G → D。D の中心の像で分母が正。
    g_to_d: Homography,
    /// G の外接矩形の寸法（幅, 高さ）。
    g_size: (f64, f64),
}

impl PerspectiveFrame {
    /// D → G の射影変換。
    pub fn d_to_g(&self) -> &Homography {
        &self.d_to_g
    }

    /// G → D の射影変換。
    pub fn g_to_d(&self) -> &Homography {
        &self.g_to_d
    }

    /// G の外接矩形の寸法。
    pub fn g_size(&self) -> (f64, f64) {
        self.g_size
    }
}

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

/// 画像の座標系の情報（S の寸法・既定の切り抜き・向きと、G を作る回転）。
/// [`crate::stage::StageContext`] が持つ。
///
/// 既定の切り抜きは [`with_default_crop`](Self::with_default_crop)、回転は
/// [`with_rotation`](Self::with_rotation)、歪曲補正は [`with_distortion`](Self::with_distortion) で加える
/// （既定は S の全体と 0 となし。モジュールの doc の「G の定義」「既定の切り抜き」）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordinateFrames {
    /// S の幅（センサーのフル解像度。RAW 以外の入力では元の画像の幅）。
    pub sensor_width: u32,
    /// S の高さ。
    pub sensor_height: u32,
    /// 既定の切り抜き（S の画素の範囲。O・G の基準の枠）。S の中に収まること
    /// （[`with_default_crop`](Self::with_default_crop) で確かめる）。既定の切り抜きのない RAW・
    /// RAW 以外の入力では S の全体。
    pub default_crop: PixelRect,
    /// RAW の向き（S → O）。
    pub orientation: Orientation,
    /// 角度補正（度。genzo-model の `Geometry::angle_deg`）。正の値で画面上の反時計回り。
    /// G は D（歪曲補正の後。なければ O）をその中心のまわりにこの角度だけ回転したもの。センサー処理
    /// （段階 A1）の文脈では 0。
    pub rotation_deg: f64,
    /// RAW に埋め込まれたプロファイルの歪曲（写真ごとに決まる。適用量・手動の補正は含まない）。
    /// 段階 A1 から段階 B・C へ渡し、[`distortion`](Self::distortion) を作るのに使う。なければ `None`。
    pub lens_geometry: Option<ProfileDistortion>,
    /// ステージ 9 の歪曲補正（D → O。段階 B・C の文脈で設定から作る。版 1・補正なしは `None`）。
    pub distortion: Option<RadialDistortion>,
    /// 変形（遠近補正。D → P。段階 B・C の文脈で設定から作る。版 1・変形なしは `None`）と、回転を含めた
    /// D → G の対応（[`with_perspective`](Self::with_perspective) で作り、回転・既定の切り抜きを変えると作り直す）。
    pub perspective: Option<PerspectiveFrame>,
}

impl CoordinateFrames {
    /// 作る（既定の切り抜きは S の全体）。寸法が 0 ならエラー。
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
            default_crop: PixelRect::full(sensor_width, sensor_height),
            orientation,
            rotation_deg: 0.0,
            lens_geometry: None,
            distortion: None,
            perspective: None,
        })
    }

    /// 変形（遠近補正）を設定したもの（D → G の対応を今の回転・既定の切り抜きで合成する）。
    pub fn with_perspective(mut self, perspective: Option<Perspective>) -> Self {
        self.perspective = perspective.map(|p| self.perspective_frame(p));
        self
    }

    /// 遠近補正があるか（G → D が射影変換か）。
    pub fn has_perspective(&self) -> bool {
        self.perspective.is_some()
    }

    /// G → S がアフィン変換でないか（歪曲補正か遠近補正がある）。真なら、アフィン変換の係数
    /// （[`g_to_sensor_affine`](Self::g_to_sensor_affine)）は使えず、[`g_to_sensor`](Self::g_to_sensor) か
    /// ステージ 9 の計画の式を使う。
    pub fn is_nonaffine(&self) -> bool {
        self.distortion.is_some() || self.perspective.is_some()
    }

    /// 遠近補正 `p` と今の回転・O の寸法から、D → G の対応を合成する（モジュールの doc の「遠近補正」）。
    ///
    /// G の外接矩形は、形の変換（傾け直しと縦横比）を回転した四角形から決め、拡大縮小・ずらし（枠の中での位置）は
    /// その後に、外接矩形の中心のまわり・画面の軸の向きに当てる（[`crate::perspective`] の doc の 5）。
    fn perspective_frame(&self, p: Perspective) -> PerspectiveFrame {
        let (ow, oh) = self.oriented_size();
        let (ow, oh) = (f64::from(ow), f64::from(oh));
        let (cx, cy) = (ow * 0.5, oh * 0.5);
        let (c, s) = self.rotation_cos_sin();
        // D → P → D の中心のまわりに回転。
        let pre = p
            .d_to_p
            .then(&Homography::translation(-cx, -cy))
            .then(&Homography::rotation(c, s));
        let mut min = (f64::INFINITY, f64::INFINITY);
        let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (x, y) in [(0.0, 0.0), (ow, 0.0), (ow, oh), (0.0, oh)] {
            // 四隅は消失線の手前にある（Perspective が確かめている）。念のため有限に収めた値を使う。
            let (qx, qy) = pre.apply(x, y);
            min = (min.0.min(qx), min.1.min(qy));
            max = (max.0.max(qx), max.1.max(qy));
        }
        let g_size = ((max.0 - min.0).max(1e-9), (max.1 - min.1).max(1e-9));
        let (gcx, gcy) = (g_size.0 * 0.5, g_size.1 * 0.5);
        let d_to_g = pre
            .then(&Homography::translation(-min.0, -min.1))
            // 枠の中での位置: 外接矩形の中心のまわりに拡大縮小し、ずらす。
            .then(&Homography::translation(-gcx, -gcy))
            .then(&Homography::scaling(p.scale, p.scale))
            .then(&Homography::translation(
                gcx + p.offset_px.0,
                gcy + p.offset_px.1,
            ))
            .with_positive_w_at(cx, cy);
        let (dcx, dcy) = d_to_g.apply(cx, cy);
        let g_to_d = d_to_g
            .inverse()
            .unwrap_or(Homography::IDENTITY)
            .with_positive_w_at(dcx, dcy);
        PerspectiveFrame {
            perspective: p,
            d_to_g,
            g_to_d,
            g_size,
        }
    }

    /// 回転か既定の切り抜きを変えた後に、D → G の対応を作り直す。
    fn rebuild_perspective(&mut self) {
        if let Some(pf) = self.perspective {
            self.perspective = Some(self.perspective_frame(pf.perspective));
        }
    }

    /// プロファイルの歪曲（[`lens_geometry`](Self::lens_geometry)）を設定したもの。
    pub fn with_lens_geometry(mut self, lens_geometry: Option<ProfileDistortion>) -> Self {
        self.lens_geometry = lens_geometry;
        self
    }

    /// 歪曲補正（[`distortion`](Self::distortion)）を設定したもの。
    pub fn with_distortion(mut self, distortion: Option<RadialDistortion>) -> Self {
        self.distortion = distortion;
        self
    }

    /// 既定の切り抜き（S の画素の範囲）を設定したもの。`None` は S の全体。S からはみ出す・幅か
    /// 高さが 0 ならエラー。
    pub fn with_default_crop(mut self, crop: Option<PixelRect>) -> Result<Self> {
        let crop = crop.unwrap_or(PixelRect::full(self.sensor_width, self.sensor_height));
        if !crop.fits_within(self.sensor_width, self.sensor_height) {
            return Err(PipelineError::InvalidArgument(format!(
                "既定の切り抜き {crop:?} が S（{} × {}）からはみ出しています",
                self.sensor_width, self.sensor_height
            )));
        }
        self.default_crop = crop;
        self.rebuild_perspective();
        Ok(self)
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
        self.rebuild_perspective();
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

    /// O → S の変換（既定の切り抜きの寸法で向きを反映し、切り抜きの左上だけ平行移動する）。
    fn map(&self) -> OrientationMap {
        let c = self.default_crop;
        let mut map = orientation_map(self.orientation, f64::from(c.width), f64::from(c.height));
        map.t[0] += f64::from(c.x);
        map.t[1] += f64::from(c.y);
        map
    }

    /// O の寸法（既定の切り抜きの寸法。向きが 90 度・270 度の回転を含むなら幅と高さが入れ替わる）。
    pub fn oriented_size(&self) -> (u32, u32) {
        let c = self.default_crop;
        if self.orientation.swaps_dimensions() {
            (c.height, c.width)
        } else {
            (c.width, c.height)
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

    /// O の画素の範囲を、それが覆う S の画素の範囲に変換する（向きは画素の並べ替えと平行移動なので、
    /// 範囲は過不足なく対応する）。画像の外にはみ出した範囲もそのまま変換する。
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
    /// 回転した O の外接矩形の長辺（回転が 0 なら O、つまり既定の切り抜きの長辺）。歪曲補正（v1）は
    /// 含まない。遠近補正（DEV-16）があれば、P の四角形を回転した外接矩形の長辺。
    pub fn g_long_side_px(&self) -> f64 {
        let (w, h) = self.g_size_px();
        w.max(h)
    }

    /// G の外接矩形の寸法（幅, 高さ。S の画素の単位）。回転が 0 なら O の寸法そのもの。遠近補正があれば
    /// P の四角形を回転した外接矩形。
    pub fn g_size_px(&self) -> (f64, f64) {
        if let Some(pf) = &self.perspective {
            return pf.g_size;
        }
        let (ow, oh) = self.oriented_size();
        let (ow, oh) = (f64::from(ow), f64::from(oh));
        if self.rotation_deg == 0.0 {
            return (ow, oh);
        }
        let (c, s) = self.rotation_cos_sin();
        let (c, s) = (c.abs(), s.abs());
        (ow * c + oh * s, ow * s + oh * c)
    }

    /// 連続座標で O → G（歪曲補正の逆で D にし、D の中心のまわりに回転して、外接矩形の左上を原点に
    /// する）。歪曲補正がなければ D = O。
    pub fn oriented_to_g(&self, u: f64, v: f64) -> (f64, f64) {
        let (u, v) = match &self.distortion {
            Some(d) => d.oriented_to_undistorted(u, v),
            None => (u, v),
        };
        self.undistorted_to_g(u, v)
    }

    /// 連続座標で G → O（[`oriented_to_g`](Self::oriented_to_g) の逆）。
    pub fn g_to_oriented(&self, gx: f64, gy: f64) -> (f64, f64) {
        let (u, v) = self.g_to_undistorted(gx, gy);
        match &self.distortion {
            Some(d) => d.undistorted_to_oriented(u, v),
            None => (u, v),
        }
    }

    /// 連続座標で D → G（遠近補正があれば D → P、D の中心のまわりに回転し、外接矩形の左上を原点にする）。D は
    /// O と同じ大きさの枠（歪曲補正がなければ O）。遠近補正で消失線の向こう側に写る点は有限の値に収める
    /// （[`Homography::apply`]）。
    pub fn undistorted_to_g(&self, u: f64, v: f64) -> (f64, f64) {
        if let Some(pf) = &self.perspective {
            return pf.d_to_g.apply(u, v);
        }
        if self.rotation_deg == 0.0 {
            return (u, v);
        }
        let (ow, oh) = self.oriented_size();
        let (gw, gh) = self.g_size_px();
        let (c, s) = self.rotation_cos_sin();
        let (dx, dy) = (u - f64::from(ow) * 0.5, v - f64::from(oh) * 0.5);
        (c * dx + s * dy + gw * 0.5, -s * dx + c * dy + gh * 0.5)
    }

    /// 連続座標で G → D（[`undistorted_to_g`](Self::undistorted_to_g) の逆。回転と、あれば遠近補正の逆）。D の枠
    /// `[0, ow] × [0, oh]`（O の寸法）の内側が画像のある部分。遠近補正で消失線の向こう側の点（画像の外）は
    /// 有限の値に収める（画像の内側かの判定には [`g_to_undistorted_checked`](Self::g_to_undistorted_checked) を使う）。
    pub fn g_to_undistorted(&self, gx: f64, gy: f64) -> (f64, f64) {
        if let Some(pf) = &self.perspective {
            return pf.g_to_d.apply(gx, gy);
        }
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

    /// 連続座標で G → D。遠近補正で消失線の上・向こう側に当たる点（画像の外）は `None`。遠近補正がなければ
    /// [`g_to_undistorted`](Self::g_to_undistorted) と同じで、常に `Some`。
    pub fn g_to_undistorted_checked(&self, gx: f64, gy: f64) -> Option<(f64, f64)> {
        match &self.perspective {
            Some(pf) => pf.g_to_d.apply_checked(gx, gy),
            None => Some(self.g_to_undistorted(gx, gy)),
        }
    }

    /// G の点が画像（D の枠 `[0, ow] × [0, oh]`。許容量 `tolerance` 画素）の内側にあるか。遠近補正で消失線の
    /// 向こう側の点は外。
    pub fn g_point_in_image(&self, gx: f64, gy: f64, tolerance: f64) -> bool {
        let Some((u, v)) = self.g_to_undistorted_checked(gx, gy) else {
            return false;
        };
        let (ow, oh) = self.oriented_size();
        (-tolerance..=f64::from(ow) + tolerance).contains(&u)
            && (-tolerance..=f64::from(oh) + tolerance).contains(&v)
    }

    /// D → G の対応を 3 × 3 の射影変換として返す（遠近補正がなければ回転のアフィン変換
    /// [`undistorted_to_g_affine`](Self::undistorted_to_g_affine) を 3 行目 `[0, 0, 1]` で表したもの）。
    pub fn undistorted_to_g_homography(&self) -> Homography {
        match &self.perspective {
            Some(pf) => pf.d_to_g,
            None => Homography::from_affine(self.undistorted_to_g_affine()),
        }
    }

    /// G → D の対応を 3 × 3 の射影変換として返す（[`undistorted_to_g_homography`](Self::undistorted_to_g_homography)
    /// の逆。遠近補正がなければ [`g_to_undistorted_affine`](Self::g_to_undistorted_affine) と同じ）。
    pub fn g_to_undistorted_homography(&self) -> Homography {
        match &self.perspective {
            Some(pf) => pf.g_to_d,
            None => Homography::from_affine(self.g_to_undistorted_affine()),
        }
    }

    /// D → G の変換（回転だけ。**遠近補正は含まない**）をアフィン変換として返す（行ごとに
    /// `[[m00, m01, t0], [m10, m11, t1]]`）。回転が 0 なら厳密に恒等。
    pub fn undistorted_to_g_affine(&self) -> [[f64; 3]; 2] {
        if self.rotation_deg == 0.0 {
            return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        }
        let (c, s) = self.rotation_cos_sin();
        let (ow, oh) = self.oriented_size();
        let (gw, gh) = self.rotated_box_size();
        let (gcx, gcy) = (gw * 0.5, gh * 0.5);
        let (ocx, ocy) = (f64::from(ow) * 0.5, f64::from(oh) * 0.5);
        [
            [c, s, gcx - (c * ocx + s * ocy)],
            [-s, c, gcy - (-s * ocx + c * ocy)],
        ]
    }

    /// 回転した O の外接矩形の寸法（遠近補正を含まない。[`g_size_px`](Self::g_size_px) の回転だけの式）。
    fn rotated_box_size(&self) -> (f64, f64) {
        let (ow, oh) = self.oriented_size();
        let (ow, oh) = (f64::from(ow), f64::from(oh));
        if self.rotation_deg == 0.0 {
            return (ow, oh);
        }
        let (c, s) = self.rotation_cos_sin();
        let (c, s) = (c.abs(), s.abs());
        (ow * c + oh * s, ow * s + oh * c)
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

    /// G → D の変換（回転だけ。**遠近補正は含まない**。遠近補正があるときは
    /// [`g_to_undistorted_homography`](Self::g_to_undistorted_homography)）をアフィン変換 `[u, v] = m · [gx, gy] + t`
    /// として返す（行ごとに `[[m00, m01, t0], [m10, m11, t1]]`）。回転が 0 なら厳密に恒等。
    pub fn g_to_undistorted_affine(&self) -> [[f64; 3]; 2] {
        if self.rotation_deg == 0.0 {
            return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        }
        let (c, s) = self.rotation_cos_sin();
        let (ow, oh) = self.oriented_size();
        let (gw, gh) = self.rotated_box_size();
        let (gcx, gcy) = (gw * 0.5, gh * 0.5);
        let (ocx, ocy) = (f64::from(ow) * 0.5, f64::from(oh) * 0.5);
        [
            [c, -s, ocx - (c * gcx - s * gcy)],
            [s, c, ocy - (s * gcx + c * gcy)],
        ]
    }

    /// O → S の変換をアフィン変換として返す（向きと既定の切り抜きの位置。要素は −1・0・1 と整数）。
    pub fn oriented_to_sensor_affine(&self) -> [[f64; 3]; 2] {
        let m = self.map();
        [
            [m.m[0][0], m.m[0][1], m.t[0]],
            [m.m[1][0], m.m[1][1], m.t[1]],
        ]
    }

    /// G → S の変換をアフィン変換 `[x, y] = m · [gx, gy] + t` として返す（`[m, t]` を行ごとに
    /// `[[m00, m01, t0], [m10, m11, t1]]`）。**歪曲補正（[`distortion`](Self::distortion)）と遠近補正
    /// （[`perspective`](Self::perspective)）は含まない**（G → D → S。どちらかがあるとき（[`is_nonaffine`](Self::is_nonaffine)）
    /// の G → S はアフィン変換ではないので、[`g_to_sensor`](Self::g_to_sensor) か、ステージ 9 の計画の式を使う）。
    ///
    /// 行列は記号的に合成する（回転が 0 で向きが通常なら、厳密に単位行列と既定の切り抜きの左上）。
    pub fn g_to_sensor_affine(&self) -> [[f64; 3]; 2] {
        let (c, s) = self.rotation_cos_sin();
        let (ow, oh) = self.oriented_size();
        let (gw, gh) = self.rotated_box_size();
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

    /// 遠近補正（DEV-16）の座標系: D → G → D が元に戻り、G の外接矩形は四角形（D の四隅の像）の外接矩形で、
    /// 回転・既定の切り抜きを変えると作り直す。消失線の向こう側の点は画像の外。
    #[test]
    fn perspective_frames_round_trip_and_follow_rotation() {
        use crate::perspective::Perspective;
        let t = genzo_model::Transform {
            vertical: -40.0,
            horizontal: 15.0,
            ..Default::default()
        };
        for o in [Orientation::Normal, Orientation::Rotate90Cw] {
            let base = CoordinateFrames::new(600, 400, o).unwrap();
            let p = Perspective::for_settings(
                base.oriented_size(),
                crate::version::ProcessVersion::V2,
                &t,
            )
            .unwrap();
            let f = base.with_perspective(Some(p));
            assert!(f.has_perspective() && f.is_nonaffine());
            let (ow, oh) = f.oriented_size();
            let (ow, oh) = (f64::from(ow), f64::from(oh));
            let (gw, gh) = f.g_size_px();
            let mut min = (f64::INFINITY, f64::INFINITY);
            let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY);
            for (u, v) in [
                (0.0, 0.0),
                (ow, 0.0),
                (ow, oh),
                (0.0, oh),
                (ow * 0.3, oh * 0.7),
            ] {
                let (gx, gy) = f.undistorted_to_g(u, v);
                let (bu, bv) = f.g_to_undistorted_checked(gx, gy).unwrap();
                assert!(
                    (bu - u).abs() < 1e-6 && (bv - v).abs() < 1e-6,
                    "{o:?} ({u}, {v}) → ({gx}, {gy}) → ({bu}, {bv})"
                );
                assert!(f.g_point_in_image(gx, gy, 1e-6));
                min = (min.0.min(gx), min.1.min(gy));
                max = (max.0.max(gx), max.1.max(gy));
            }
            assert!(min.0.abs() < 1e-9 && min.1.abs() < 1e-9, "{min:?}");
            assert!(
                (max.0 - gw).abs() < 1e-9 && (max.1 - gh).abs() < 1e-9,
                "{max:?} vs {gw} {gh}"
            );
            // S との往復。
            let (sx, sy) = f.g_to_sensor(gw * 0.5, gh * 0.5);
            let (gx, gy) = f.sensor_to_g(sx, sy);
            assert!((gx - gw * 0.5).abs() < 1e-6 && (gy - gh * 0.5).abs() < 1e-6);
            // 回転を加えると外接矩形が変わり、D → G の対応も回転を含む。
            let r = f.with_rotation(10.0).unwrap();
            assert_ne!(r.g_size_px(), f.g_size_px());
            let (u, v) = (ow * 0.25, oh * 0.5);
            let (gx, gy) = r.undistorted_to_g(u, v);
            let (bu, bv) = r.g_to_undistorted(gx, gy);
            assert!((bu - u).abs() < 1e-6 && (bv - v).abs() < 1e-6);
            // 遠近補正の行列の 3 行目は [0, 0, 1] ではない（射影）。
            assert!(!r.undistorted_to_g_homography().is_affine());
            assert!(
                CoordinateFrames::new(600, 400, o)
                    .unwrap()
                    .with_rotation(10.0)
                    .unwrap()
                    .undistorted_to_g_homography()
                    .is_affine()
            );
        }
        // 消失線の向こう側: 大きく外れた G の点は画像の外。
        let base = CoordinateFrames::new(600, 400, Orientation::Normal).unwrap();
        let strong = genzo_model::Transform {
            vertical: -100.0,
            ..Default::default()
        };
        let p = Perspective::for_settings((600, 400), crate::version::ProcessVersion::V2, &strong)
            .unwrap();
        let f = base.with_perspective(Some(p));
        let (gw, gh) = f.g_size_px();
        assert!(!f.g_point_in_image(gw * 0.5, gh * 50.0, 0.0));
        assert!(!f.g_point_in_image(-gw * 50.0, gh * 0.5, 0.0));
        // g_to_undistorted は有限の値を返す。
        let (u, v) = f.g_to_undistorted(gw * 0.5, gh * 50.0);
        assert!(u.is_finite() && v.is_finite());
    }

    /// 拡大縮小・ずらし（DEV-16）は G の外接矩形を変えず、枠の中の画像を動かす（外接矩形を拡大縮小・ずらしの後の
    /// 四隅で決めると、ずらしは打ち消され、拡大縮小は出力の画素数が変わるだけになる。Codex のレビューの指摘）。
    #[test]
    fn scale_and_offset_move_the_image_inside_the_same_frame() {
        use crate::perspective::Perspective;
        let base = CoordinateFrames::new(600, 400, Orientation::Normal).unwrap();
        let with = |t: genzo_model::Transform| {
            let p = Perspective::for_settings((600, 400), crate::version::ProcessVersion::V2, &t)
                .unwrap();
            base.with_perspective(Some(p))
        };
        // ずらし: 枠は O のまま、D の中心が右・上へ動く（幅の半分の 50 %・高さの半分の 20 %）。
        let f = with(genzo_model::Transform {
            offset_x: 50.0,
            offset_y: -20.0,
            ..Default::default()
        });
        assert_eq!(f.g_size_px(), (600.0, 400.0));
        let (gx, gy) = f.undistorted_to_g(300.0, 200.0);
        assert!(
            (gx - 450.0).abs() < 1e-9 && (gy - 160.0).abs() < 1e-9,
            "{gx} {gy}"
        );
        // 枠の左の端は画像の外になる。
        assert!(!f.g_point_in_image(10.0, 200.0, 0.0));
        assert!(f.g_point_in_image(590.0, 200.0, 0.0));
        // 拡大: 枠は O のまま、中心のまわりに 1.5 倍（D の隅は枠の外へ出る）。
        let f = with(genzo_model::Transform {
            scale: 150.0,
            ..Default::default()
        });
        assert_eq!(f.g_size_px(), (600.0, 400.0));
        let (gx, gy) = f.undistorted_to_g(600.0, 400.0);
        assert!(
            (gx - 750.0).abs() < 1e-9 && (gy - 500.0).abs() < 1e-9,
            "{gx} {gy}"
        );
        let (u, v) = f.g_to_undistorted(600.0, 400.0);
        assert!(
            (u - 500.0).abs() < 1e-9 && (v - 333.333_333_333).abs() < 1e-6,
            "{u} {v}"
        );
        // 縮小: 枠の隅は画像の外になる。
        let f = with(genzo_model::Transform {
            scale: 80.0,
            ..Default::default()
        });
        assert_eq!(f.g_size_px(), (600.0, 400.0));
        assert!(!f.g_point_in_image(1.0, 1.0, 0.0));
        assert!(f.g_point_in_image(300.0, 200.0, 0.0));
        // 傾きと重ねても、外接矩形は傾き（形の変換）だけで決まる。
        let tilt = genzo_model::Transform {
            vertical: -40.0,
            ..Default::default()
        };
        let a = with(tilt);
        let b = with(genzo_model::Transform {
            scale: 120.0,
            offset_x: 30.0,
            ..tilt
        });
        assert_eq!(a.g_size_px(), b.g_size_px());
        assert_ne!(
            a.undistorted_to_g(300.0, 200.0),
            b.undistorted_to_g(300.0, 200.0)
        );
    }

    /// 画面（ui の cropMath の perspectiveFrame）が同じ式で G の外接矩形と D → G を計算しているか、同じ数値で固定する
    /// （ui の cropMath.test.ts の「変形の外接矩形はコアと同じ値」と同じ値。どちらかの式を変えたら両方を直す）。
    #[test]
    fn perspective_frame_matches_the_ui_values() {
        use crate::perspective::Perspective;
        let t = genzo_model::Transform {
            vertical: -40.0,
            horizontal: 15.0,
            aspect: 20.0,
            ..Default::default()
        };
        let base = CoordinateFrames::new(600, 400, Orientation::Normal)
            .unwrap()
            .with_rotation(7.0)
            .unwrap();
        let p =
            Perspective::for_settings((600, 400), crate::version::ProcessVersion::V2, &t).unwrap();
        let f = base.with_perspective(Some(p));
        let (gw, gh) = f.g_size_px();
        assert!(
            (gw - 716.487_918_941).abs() < 1e-6 && (gh - 570.003_459_435).abs() < 1e-6,
            "{gw} {gh}"
        );
        let t2 = genzo_model::Transform {
            scale: 120.0,
            offset_x: 30.0,
            offset_y: -10.0,
            ..t
        };
        let p =
            Perspective::for_settings((600, 400), crate::version::ProcessVersion::V2, &t2).unwrap();
        let f = base.with_perspective(Some(p));
        let (x, y) = f.undistorted_to_g(300.0, 200.0);
        assert!(
            (x - 437.430_696_754).abs() < 1e-6 && (y - 350.322_573_434).abs() < 1e-6,
            "{x} {y}"
        );
        let (x, y) = f.undistorted_to_g(0.0, 0.0);
        assert!(
            (x - 18.351_208_106).abs() < 1e-6 && (y - 113.540_434_291).abs() < 1e-6,
            "{x} {y}"
        );
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

    /// 既定の切り抜き（A-2）: O・G は S の中の既定の切り抜きの範囲で、O → S は平行移動を含む。
    #[test]
    fn default_crop_is_the_frame_of_o_and_g() {
        let crop = PixelRect::new(44, 30, 4608, 3072);
        for o in ALL {
            let f = CoordinateFrames::new(4736, 3132, o)
                .unwrap()
                .with_default_crop(Some(crop))
                .unwrap();
            let (ow, oh) = f.oriented_size();
            if o.swaps_dimensions() {
                assert_eq!((ow, oh), (3072, 4608));
            } else {
                assert_eq!((ow, oh), (4608, 3072));
            }
            assert_eq!(f.g_long_side_px(), 4608.0);
            // O の全体は S の既定の切り抜きの範囲をちょうど覆う。
            assert_eq!(
                f.oriented_roi_to_sensor(Roi::full(ow, oh)),
                Roi::new(44, 30, 4608, 3072),
                "{o:?}"
            );
            // O の四隅の画素は、既定の切り抜きの四隅の画素のどれか。
            let corners = [
                (44, 30),
                (44 + 4607, 30),
                (44, 30 + 3071),
                (44 + 4607, 30 + 3071),
            ];
            for (u, v) in [(0, 0), (ow - 1, 0), (0, oh - 1), (ow - 1, oh - 1)] {
                let p = f.oriented_pixel_to_sensor(u, v).unwrap();
                assert!(corners.contains(&p), "{o:?} ({u}, {v}) → {p:?}");
            }
            // 連続座標の往復と、アフィン変換の形の一致。
            let r = f.with_rotation(7.5).unwrap();
            let a = r.g_to_sensor_affine();
            let (gw, gh) = r.g_size_px();
            for (gx, gy) in [(0.0, 0.0), (gw * 0.3, gh * 0.8), (gw, gh)] {
                let (x, y) = r.g_to_sensor(gx, gy);
                let xa = a[0][0] * gx + a[0][1] * gy + a[0][2];
                let ya = a[1][0] * gx + a[1][1] * gy + a[1][2];
                assert!((x - xa).abs() < 1e-7 && (y - ya).abs() < 1e-7);
                let (gx2, gy2) = r.sensor_to_g(x, y);
                assert!((gx - gx2).abs() < 1e-7 && (gy - gy2).abs() < 1e-7);
            }
            // G の中心は既定の切り抜きの中心。
            let (cx, cy) = r.g_to_sensor(gw * 0.5, gh * 0.5);
            assert!((cx - (44.0 + 2304.0)).abs() < 1e-7 && (cy - (30.0 + 1536.0)).abs() < 1e-7);
        }
        // 回転 0・向きが通常なら、G → S は平行移動だけ。
        let f = CoordinateFrames::new(4736, 3132, Orientation::Normal)
            .unwrap()
            .with_default_crop(Some(crop))
            .unwrap();
        assert_eq!(f.g_to_sensor_affine(), [[1.0, 0.0, 44.0], [0.0, 1.0, 30.0]]);
        // None は全体。はみ出す範囲はエラー。
        let full = CoordinateFrames::new(10, 8, Orientation::Normal).unwrap();
        assert_eq!(full.with_default_crop(None).unwrap(), full);
        assert_eq!(full.default_crop, PixelRect::full(10, 8));
        assert!(
            full.with_default_crop(Some(PixelRect::new(5, 0, 6, 8)))
                .is_err()
        );
        assert!(
            full.with_default_crop(Some(PixelRect::new(0, 0, 0, 8)))
                .is_err()
        );
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
