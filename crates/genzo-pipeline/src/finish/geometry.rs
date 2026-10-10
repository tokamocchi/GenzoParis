//! ステージ 9: ジオメトリ（docs/04_architecture.md の 2.1 節・2.7 節。DEV-15）。
//!
//! 向き（RAW の Orientation）・角度補正（回転）・切り抜きを、**1 回のリサンプリング**で行う
//! （2.1 節の補足「リサンプリングを 1 回だけ行う」）。出力の画素ごとに C → G → O → S → 入力の格子と
//! 逆にたどり（逆写像）、入力の格子をバイキュービック補間で引く。歪曲補正（v1）は未実装。
//!
//! # 格子の決めごと
//!
//! - **入力の格子**: [`crate::stage::StageContext::sensor_grid`]（[`SensorGrid`]。段階 A1 のプレビュー、
//!   フル解像度のタイルなど。S の向き）。
//! - **G の格子**: G の外接矩形（[`CoordinateFrames::g_size_px`]）を、入力の格子と **同じ密度**
//!   （S の 1 画素あたりの格子の画素の数。向きで縦横が入れ替わる場合は入れ替える）で標本化した格子。
//!   左上は G の (0, 0) で、画素 p は G の `[p / d, (p + 1) / d)`。寸法は `max(1, round(外接矩形 × d))`。
//!   密度を入力の格子と同じにするのは、回転も切り抜きもなければ出力の画素の中心が入力の画素の中心と
//!   一致し、補間が恒等になるようにするため。
//! - **C の格子**（出力）: G の格子のうち切り抜きの範囲（[`GeometryPlan::crop_lattice`]）。C の画素
//!   (i, j) は G の格子の画素 `(x0 + i, y0 + j)`。切り抜きの端は G の格子に丸める
//!   （`x0 = round(left × 外接矩形の幅 × d)` など）。そのため、**切り抜きを変えても、切り抜いた
//!   範囲の画素は切り抜く前の同じ位置の画素と同じ計算になる**（2.7 節「切り抜きによって補正の結果は
//!   変わらない」）。
//!
//! # 回転で生じる外側の扱い（決めごと）
//!
//! 回転すると、G の外接矩形の四隅に画像のない部分ができる。ステージ 9 はここを鏡映で埋めず、
//! **切り抜きを画像の内側に制限する**（[`constrain_crop`]）: 切り抜きの四隅のどれかが回転した画像の
//! 外に出る場合は、切り抜きの中心と縦横比を保って、四隅が画像に収まる最大の大きさまで縮める
//! （中心が画像の外なら画像の中心を使う）。UI は角度補正のときに同じ関数で切り抜き枠を
//! 制限する（Lightroom の「画像に制限」に相当）想定で、パイプラインの制限は安全策。
//! 補間のタップが画像の端の外に出る部分（画像の端から 2 画素以内）は、2.7 節の決めごとどおり
//! 入力の格子の鏡映で拡張する。
//!
//! # 補間（GPU 版も同じ式。2.3 節）
//!
//! 出力の画素ごとの入力の格子の座標（連続座標。画素の中心が +0.5）を f32 で
//!
//! ```text
//! X = (x0 + i) as f32 + 0.5,  Y = (y0 + j) as f32 + 0.5      （G の格子の座標。i, j は C の画素）
//! x = a00·X + a01·Y + a02,     y = a10·X + a11·Y + a12        （左から順に足す）
//! ```
//!
//! とし（`a` は [`GeometryPlan::lattice_to_input_f32`]）、Keys の 3 次畳み込み（a = −0.5、
//! Catmull-Rom）で補間する:
//!
//! ```text
//! fx = x − 0.5,  ix = floor(fx),  t = fx − ix
//! w0 = ((−0.5·t + 1.0)·t − 0.5)·t      （画素 ix − 1）
//! w1 = (1.5·t − 2.5)·t·t + 1.0         （画素 ix）
//! w2 = ((−1.5·t + 2.0)·t + 0.5)·t      （画素 ix + 1）
//! w3 = (0.5·t − 0.5)·t·t               （画素 ix + 2）
//! 行 k（k = 0..3、画素の行 iy − 1 + k）: r_k = w0·p0 + w1·p1 + w2·p2 + w3·p3（左から順に）
//! 結果: v0·r0 + v1·r1 + v2·r2 + v3·r3（v は y 方向の重み）
//! ```
//!
//! `t = 0` では重みが厳密に (0, 1, 0, 0) なので、画素の中心ではその画素の値をそのまま返す
//! （回転 0 で補間が恒等）。Catmull-Rom は輪郭の近くでわずかに行き過ぎる（負の値も出る）が、B2 は
//! 負の値を保持する契約なので切り捨てない。縮小の用途（出力の密度が入力より低い）には向かない
//! ので、ステージ 9 は入力の格子と同じ密度で出力する（上の「G の格子」）。

use genzo_model::develop::MIN_CROP_SIZE;
use genzo_model::{CropRect, DevelopSettings, Geometry, Phase};
use genzo_raw::MAX_PIXELS;
use rayon::prelude::*;

use crate::border::mirror_index;
use crate::contract::ColorContract;
use crate::coords::{CoordinateFrames, SensorGrid};
use crate::error::{PipelineError, Result};
use crate::image::{ImageTile, Roi};
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// 切り抜きの四隅が画像の内側にあるかを判定するときの許容量（制限の倍率の 1 からの差）。
/// 丸めの誤差で、画像の端にちょうど接する切り抜きを縮めないため。
const CROP_SCALE_TOLERANCE: f64 = 1e-9;

/// [`GeometryPlan::input_roi`] で、補間のタップの範囲に加える余白（画素）。f32 の座標の丸めで
/// タップが 1 画素ずれても入力の範囲に収まるようにする。
const INPUT_ROI_SAFETY: i64 = 1;

/// 回転・切り抜き・向きをまとめた、C の格子 → 入力の格子の対応（ステージ 9 のパラメータ。
/// ステージ 11 もガイドを引くために使う）。
///
/// 作り方は [`GeometryPlan::new`]。モジュールの doc の「格子の決めごと」を参照。
#[derive(Debug, Clone, PartialEq)]
pub struct GeometryPlan {
    frames: CoordinateFrames,
    grid: SensorGrid,
    requested_crop: CropRect,
    crop: CropRect,
    density: (f64, f64),
    lattice_size: (u32, u32),
    crop_lattice: Roi,
    to_input: [[f64; 3]; 2],
    to_input_f32: [[f32; 3]; 2],
}

impl GeometryPlan {
    /// 現像設定のジオメトリ（丸めた設定。`geometry.angle_deg` は `frames.rotation_deg` と一致すること）
    /// と、座標系・入力の格子から作る。
    ///
    /// 角度は `frames` の値を使う（G は座標系の一部のため）。設定の角度と違えば
    /// [`PipelineError::InvalidArgument`]（文脈の作り方の誤り。[`super::finish_context`] を使う）。
    /// 出力（C の格子）の画素数が genzo-raw の `MAX_PIXELS` を超える場合（極端に細長い画像を
    /// 回転したときなど）は [`PipelineError::InvalidDimensions`]。
    pub fn new(geometry: &Geometry, frames: CoordinateFrames, grid: SensorGrid) -> Result<Self> {
        let grid = SensorGrid::new(grid.width, grid.height, grid.extent)?;
        if f64::from(geometry.angle_deg) != frames.rotation_deg {
            return Err(PipelineError::InvalidArgument(format!(
                "文脈の回転（{}°）と設定の角度補正（{}°）が違います",
                frames.rotation_deg, geometry.angle_deg
            )));
        }
        let requested_crop = geometry.crop;
        let crop = constrain_crop(&frames, &requested_crop);
        // G の格子の密度（向きで縦横が入れ替わる場合は入れ替える）。
        let (sx, sy) = grid.density();
        let density = if frames.orientation.swaps_dimensions() {
            (sy, sx)
        } else {
            (sx, sy)
        };
        let (gw, gh) = frames.g_size_px();
        let lw = lattice_len(gw * density.0)?;
        let lh = lattice_len(gh * density.1)?;
        let (x0, x1) = crop_span(crop.left, crop.right, gw * density.0, lw);
        let (y0, y1) = crop_span(crop.top, crop.bottom, gh * density.1, lh);
        let crop_lattice = Roi::new(x0 as i32, y0 as i32, x1 - x0, y1 - y0);
        // 出力（C の格子）の画素数の上限。回転で切り抜きを画像の内側に制限すれば、出力はふつう S の
        // 画素数以下だが、極端に細長い画像では制限した切り抜きが最小の大きさ（MIN_CROP_SIZE）で
        // 止まり、G の外接矩形の大きさに比例した巨大な出力になりうる。そのまま進むと、ルーペ・書き出し
        // の出力のバッファ（数 GB）を確保してから失敗する・長時間かかるので、ここでエラーにする。
        if crop_lattice.pixel_count() > MAX_PIXELS {
            return Err(PipelineError::InvalidDimensions {
                width: crop_lattice.width,
                height: crop_lattice.height,
            });
        }

        // G の格子 → G（px）→ S → 入力の格子。
        let g_to_s = frames.g_to_sensor_affine();
        let (inv_dx, inv_dy) = (1.0 / density.0, 1.0 / density.1);
        let mut to_input = [[0.0; 3]; 2];
        for (r, scale) in [(0usize, sx), (1, sy)] {
            to_input[r] = [
                g_to_s[r][0] * inv_dx * scale,
                g_to_s[r][1] * inv_dy * scale,
                g_to_s[r][2] * scale,
            ];
        }
        let to_input_f32 = to_input.map(|row| row.map(|v| v as f32));
        Ok(Self {
            frames,
            grid,
            requested_crop,
            crop,
            density,
            lattice_size: (lw, lh),
            crop_lattice,
            to_input,
            to_input_f32,
        })
    }

    /// 現像設定と文脈から作る（設定は丸めてから使う）。
    pub fn from_settings(settings: &DevelopSettings, ctx: &StageContext<'_>) -> Result<Self> {
        let s = settings.normalized();
        Self::new(&s.geometry, ctx.frames, ctx.sensor_grid)
    }

    /// 座標系。
    pub fn frames(&self) -> &CoordinateFrames {
        &self.frames
    }

    /// 入力の格子。
    pub fn sensor_grid(&self) -> &SensorGrid {
        &self.grid
    }

    /// 設定の切り抜き。
    pub fn requested_crop(&self) -> CropRect {
        self.requested_crop
    }

    /// 実際に使う切り抜き（画像の内側に制限したもの。[`constrain_crop`]）。
    pub fn crop(&self) -> CropRect {
        self.crop
    }

    /// G の格子の密度（G の 1 画素（S の画素の単位）あたりの格子の画素の数。横, 縦）。
    pub fn density(&self) -> (f64, f64) {
        self.density
    }

    /// G の格子の寸法（切り抜く前の全体）。
    pub fn lattice_size(&self) -> (u32, u32) {
        self.lattice_size
    }

    /// 切り抜きの範囲（G の格子の座標）。C の画素 (i, j) は G の格子の画素
    /// `(crop_lattice.x + i, crop_lattice.y + j)`。幅・高さは出力の寸法。
    pub fn crop_lattice(&self) -> Roi {
        self.crop_lattice
    }

    /// 出力（C の格子）の寸法。
    pub fn output_size(&self) -> (u32, u32) {
        (self.crop_lattice.width, self.crop_lattice.height)
    }

    /// G の格子の連続座標 → 入力の格子の連続座標のアフィン変換（f64。行ごとに `[a, b, t]`）。
    pub fn lattice_to_input(&self) -> [[f64; 3]; 2] {
        self.to_input
    }

    /// 画素の計算に使う f32 の係数（GPU 版にも同じ値を渡す）。
    pub fn lattice_to_input_f32(&self) -> [[f32; 3]; 2] {
        self.to_input_f32
    }

    /// C の連続座標 → G の画素の座標（S の画素の単位）。
    pub fn c_to_g_px(&self, cx: f64, cy: f64) -> (f64, f64) {
        (
            (cx + f64::from(self.crop_lattice.x)) / self.density.0,
            (cy + f64::from(self.crop_lattice.y)) / self.density.1,
        )
    }

    /// C の連続座標 → 入力の格子の連続座標（f64）。
    pub fn c_to_input(&self, cx: f64, cy: f64) -> (f64, f64) {
        let x = cx + f64::from(self.crop_lattice.x);
        let y = cy + f64::from(self.crop_lattice.y);
        let a = &self.to_input;
        (
            a[0][0] * x + a[0][1] * y + a[0][2],
            a[1][0] * x + a[1][1] * y + a[1][2],
        )
    }

    /// 何もしない（出力が入力の格子とまったく同じ）か。回転 0・切り抜きなし・向きが通常で、
    /// G の格子が入力の格子と一致する場合。このときステージ 9 を飛ばす。
    pub fn is_identity(&self) -> bool {
        self.crop_lattice == Roi::full(self.grid.width, self.grid.height)
            && self.to_input_f32 == [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
    }

    /// 出力の範囲 `output_roi`（C の格子）を計算するのに必要な入力の格子の範囲。
    ///
    /// 範囲の四隅を入力の格子に写した外接矩形に、補間のタップ（左右・上下に 1・2 画素）と
    /// 丸めの余白（1 画素）を加える。画像の外にはみ出してよい（鏡映で埋める）。
    pub fn input_roi(&self, output_roi: Roi) -> Roi {
        if output_roi.is_empty() {
            return Roi::new(0, 0, 0, 0);
        }
        let (x0, x1) = (f64::from(output_roi.x), output_roi.x_end() as f64);
        let (y0, y1) = (f64::from(output_roi.y), output_roi.y_end() as f64);
        let mut min = (f64::INFINITY, f64::INFINITY);
        let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (cx, cy) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            let (x, y) = self.c_to_input(cx, cy);
            min = (min.0.min(x), min.1.min(y));
            max = (max.0.max(x), max.1.max(y));
        }
        let lo = |v: f64| (v - 0.5).floor() as i64 - 1 - INPUT_ROI_SAFETY;
        let hi = |v: f64| (v - 0.5).floor() as i64 + 3 + INPUT_ROI_SAFETY;
        roi_from_span(lo(min.0), hi(max.0), lo(min.1), hi(max.1))
    }
}

/// G の格子の 1 軸の長さ（`max(1, round(v))`）。大きすぎればエラー。
fn lattice_len(v: f64) -> Result<u32> {
    let n = v.round();
    if !(n.is_finite() && n <= f64::from(u32::MAX)) {
        return Err(PipelineError::InvalidArgument(format!(
            "G の格子が大きすぎます（{v}）"
        )));
    }
    Ok((n as u32).max(1))
}

/// 切り抜きの 1 軸の範囲（G の格子の画素の番号 `[a, b)`）。`a < b ≤ len`。
fn crop_span(lo: f32, hi: f32, extent: f64, len: u32) -> (u32, u32) {
    let a = (f64::from(lo) * extent)
        .round()
        .clamp(0.0, f64::from(len - 1)) as u32;
    let b = (f64::from(hi) * extent)
        .round()
        .clamp(f64::from(a) + 1.0, f64::from(len)) as u32;
    (a, b)
}

/// `[x0, x1) × [y0, y1)` を i32 の範囲に収めた Roi にする。
fn roi_from_span(x0: i64, x1: i64, y0: i64, y1: i64) -> Roi {
    let lim = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    let (x0, x1, y0, y1) = (lim(x0), lim(x1), lim(y0), lim(y1));
    Roi::new(
        x0 as i32,
        y0 as i32,
        (x1 - x0).clamp(0, i64::from(u32::MAX)) as u32,
        (y1 - y0).clamp(0, i64::from(u32::MAX)) as u32,
    )
}

/// 切り抜きを回転した画像の内側に制限する（モジュールの doc の「回転で生じる外側の扱い」）。
///
/// - 回転が 0 なら G は画像そのものなので、そのまま返す。
/// - 四隅がすべて画像の内側（縮める倍率が 1 − 10^−9 以上。丸めの誤差で端に接する切り抜きを縮めない）なら
///   そのまま返す。
/// - そうでなければ、切り抜きの中心（画像の外なら画像の中心）を保ち、縦横比を保って、四隅が
///   画像に収まる最大の大きさに縮める。幅・高さは [`MIN_CROP_SIZE`] 以上にする。
///
/// 計算は f64（G の画素の単位）。`crop` は丸めた設定（各軸 0〜1、`left < right`）であること。
pub fn constrain_crop(frames: &CoordinateFrames, crop: &CropRect) -> CropRect {
    if frames.rotation_deg == 0.0 {
        return *crop;
    }
    let (gw, gh) = frames.g_size_px();
    let (ow, oh) = frames.oriented_size();
    let (ow, oh) = (f64::from(ow), f64::from(oh));
    let l = f64::from(crop.left) * gw;
    let r = f64::from(crop.right) * gw;
    let t = f64::from(crop.top) * gh;
    let b = f64::from(crop.bottom) * gh;
    let inside = |(u, v): (f64, f64)| (0.0..=ow).contains(&u) && (0.0..=oh).contains(&v);
    // 中心（画像の外なら画像の中心）。
    let mut c = ((l + r) * 0.5, (t + b) * 0.5);
    if !inside(frames.g_to_oriented(c.0, c.1)) {
        c = (gw * 0.5, gh * 0.5);
    }
    let (hw, hh) = ((r - l) * 0.5, (b - t) * 0.5);
    let oc = frames.g_to_oriented(c.0, c.1);
    // 四隅の中心からの差を O に移した向き（回転の線形部分 Rᵀ）。
    let (cos, sin) = frames.rotation_cos_sin();
    let mut scale: f64 = 1.0;
    for (dx, dy) in [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)] {
        let w = (cos * dx - sin * dy, sin * dx + cos * dy);
        for (o, wi, len) in [(oc.0, w.0, ow), (oc.1, w.1, oh)] {
            if wi > 0.0 {
                scale = scale.min((len - o) / wi);
            } else if wi < 0.0 {
                scale = scale.min(o / -wi);
            }
        }
    }
    let unchanged_center = c == ((l + r) * 0.5, (t + b) * 0.5);
    if scale >= 1.0 - CROP_SCALE_TOLERANCE && unchanged_center {
        return *crop;
    }
    let scale = scale.clamp(0.0, 1.0);
    let min_w = f64::from(MIN_CROP_SIZE) * gw;
    let min_h = f64::from(MIN_CROP_SIZE) * gh;
    let hw = (hw * scale).max(min_w * 0.5);
    let hh = (hh * scale).max(min_h * 0.5);
    let to_frac = |v: f64, len: f64| ((v / len).clamp(0.0, 1.0)) as f32;
    let mut out = CropRect {
        left: to_frac(c.0 - hw, gw),
        top: to_frac(c.1 - hh, gh),
        right: to_frac(c.0 + hw, gw),
        bottom: to_frac(c.1 + hh, gh),
    };
    // 0〜1 に収めた結果、幅・高さが最小値を下回る場合は内側へ広げ直す。
    if out.width() < MIN_CROP_SIZE {
        out.left = (out.right - MIN_CROP_SIZE).max(0.0);
        out.right = out.left + MIN_CROP_SIZE;
    }
    if out.height() < MIN_CROP_SIZE {
        out.top = (out.bottom - MIN_CROP_SIZE).max(0.0);
        out.bottom = out.top + MIN_CROP_SIZE;
    }
    out
}

/// Keys の 3 次畳み込み（a = −0.5）の 4 つの重み（モジュールの doc の式）。
#[inline]
pub fn cubic_weights(t: f32) -> [f32; 4] {
    [
        ((-0.5 * t + 1.0) * t - 0.5) * t,
        (1.5 * t - 2.5) * t * t + 1.0,
        ((-1.5 * t + 2.0) * t + 0.5) * t,
        (0.5 * t - 0.5) * t * t,
    ]
}

/// 補間のタップの左上の番号と、x・y の小数部。極端な座標は鏡映の周期より十分大きい範囲に切り詰める。
#[inline]
fn cubic_base(v: f32) -> (i64, f32) {
    let f = if v.is_finite() { v - 0.5 } else { -0.5 };
    let i = f.floor();
    let t = f - i;
    const LIMIT: f32 = (1u64 << 40) as f32;
    (i.clamp(-LIMIT, LIMIT) as i64, t)
}

/// 入力のタイルから画素 (x, y)（入力の格子の座標）を取る。タイルにない場合は、格子の鏡映で
/// 折り返した位置を取り、それもなければタイルの最も近い画素を取る（入力の範囲は
/// [`GeometryPlan::input_roi`] で確保しているので、通常は最初の分岐で取れる）。
#[inline]
fn fetch(tile: &ImageTile, grid: (u32, u32), x: i64, y: i64) -> [f32; 3] {
    let roi = tile.roi;
    let w = i64::from(roi.width);
    let local = |x: i64, y: i64| -> Option<[f32; 3]> {
        let lx = x - i64::from(roi.x);
        let ly = y - i64::from(roi.y);
        (lx >= 0 && ly >= 0 && lx < w && ly < i64::from(roi.height))
            .then(|| tile.image.data()[(ly * w + lx) as usize])
    };
    if let Some(p) = local(x, y) {
        return p;
    }
    let mx = i64::from(mirror_index(x, grid.0));
    let my = i64::from(mirror_index(y, grid.1));
    if let Some(p) = local(mx, my) {
        return p;
    }
    let cx = mx.clamp(i64::from(roi.x), roi.x_end() - 1);
    let cy = my.clamp(i64::from(roi.y), roi.y_end() - 1);
    local(cx, cy).unwrap_or([0.0; 3])
}

/// 入力の格子の連続座標 (x, y) をバイキュービックで補間する（モジュールの doc の式）。
#[inline]
pub(crate) fn sample_bicubic(tile: &ImageTile, grid: (u32, u32), x: f32, y: f32) -> [f32; 3] {
    let (ix, tx) = cubic_base(x);
    let (iy, ty) = cubic_base(y);
    let wx = cubic_weights(tx);
    let wy = cubic_weights(ty);
    let mut rows = [[0.0f32; 3]; 4];
    for (k, row) in rows.iter_mut().enumerate() {
        let yy = iy - 1 + k as i64;
        let p0 = fetch(tile, grid, ix - 1, yy);
        let p1 = fetch(tile, grid, ix, yy);
        let p2 = fetch(tile, grid, ix + 1, yy);
        let p3 = fetch(tile, grid, ix + 2, yy);
        *row =
            std::array::from_fn(|c| wx[0] * p0[c] + wx[1] * p1[c] + wx[2] * p2[c] + wx[3] * p3[c]);
    }
    std::array::from_fn(|c| {
        wy[0] * rows[0][c] + wy[1] * rows[1][c] + wy[2] * rows[2][c] + wy[3] * rows[3][c]
    })
}

/// ステージ 9: ジオメトリ（向き・回転・切り抜き。1 回のリサンプリング）。
///
/// - 入力: B2（入力の格子 = [`StageContext::sensor_grid`]、S の向き）。出力: B2（C の格子）。
/// - パラメータ: [`GeometryPlan`]。何もしない場合（[`GeometryPlan::is_identity`]）は `None`。
/// - 歪曲補正（v1）は未実装（`lens.profile_enabled` でも適用しない。
///   [`super::reserved::unimplemented_settings`] で警告できる）。
#[derive(Debug, Clone, Copy, Default)]
pub struct GeometryStage;

impl GeometryStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.geometry";
    /// 04 の 2.1 節のステージの番号。
    pub const NUMBER: u8 = 9;
}

impl Stage for GeometryStage {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn phase(&self) -> Phase {
        Phase::C
    }
    fn input_contract(&self) -> ColorContract {
        ColorContract::B2Working
    }
    fn output_contract(&self) -> ColorContract {
        ColorContract::B2Working
    }
    fn params(
        &self,
        settings: &DevelopSettings,
        ctx: &StageContext<'_>,
    ) -> Result<Option<StageParams>> {
        match ctx.process_version {
            ProcessVersion::V1 => {
                let plan = GeometryPlan::from_settings(settings, ctx)?;
                Ok((!plan.is_identity()).then(|| StageParams::new(plan)))
            }
        }
    }
    fn input_roi(&self, output_roi: Roi, params: &StageParams, _ctx: &StageContext<'_>) -> Roi {
        match params.get::<GeometryPlan>() {
            Some(plan) => plan.input_roi(output_roi),
            // 型が違う場合は run_cpu がエラーにする。ここでは出力の範囲を返す。
            None => output_roi,
        }
    }
    fn run_cpu(
        &self,
        ctx: &StageContext<'_>,
        input: &ImageTile,
        output: &mut ImageTile,
        params: &StageParams,
    ) -> Result<()> {
        let plan = params.expect::<GeometryPlan>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 => {}
        }
        let grid = (plan.grid.width, plan.grid.height);
        let a = plan.to_input_f32;
        let roi = output.roi;
        let ox = i64::from(plan.crop_lattice.x) + i64::from(roi.x);
        let oy = i64::from(plan.crop_lattice.y) + i64::from(roi.y);
        let width = roi.width as usize;
        output
            .image
            .data_mut()
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(j, row)| {
                let yy = (oy + j as i64) as f32 + 0.5;
                for (i, px) in row.iter_mut().enumerate() {
                    let xx = (ox + i as i64) as f32 + 0.5;
                    let x = a[0][0] * xx + a[0][1] * yy + a[0][2];
                    let y = a[1][0] * xx + a[1][1] * yy + a[1][2];
                    *px = sample_bicubic(input, grid, x, y);
                }
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use genzo_model::Orientation;

    use super::*;

    fn frames(w: u32, h: u32, o: Orientation, deg: f64) -> CoordinateFrames {
        CoordinateFrames::new(w, h, o)
            .unwrap()
            .with_rotation(deg)
            .unwrap()
    }

    fn geometry(deg: f32, crop: CropRect) -> Geometry {
        Geometry {
            crop,
            angle_deg: deg,
            aspect: None,
        }
    }

    #[test]
    fn cubic_weights_are_exact_at_zero_and_sum_to_one() {
        assert_eq!(cubic_weights(0.0), [0.0, 1.0, 0.0, 0.0]);
        for k in 0..=16 {
            let t = k as f32 / 16.0;
            let w = cubic_weights(t);
            let s: f32 = w.iter().sum();
            assert!((s - 1.0).abs() < 1e-6, "{t}: {w:?}");
        }
        // t = 0.5 は対称。
        let w = cubic_weights(0.5);
        assert!((w[0] - w[3]).abs() < 1e-7 && (w[1] - w[2]).abs() < 1e-7);
    }

    #[test]
    fn identity_geometry_is_detected() {
        let f = frames(64, 48, Orientation::Normal, 0.0);
        let p = GeometryPlan::new(
            &geometry(0.0, CropRect::FULL),
            f,
            SensorGrid::full_resolution(&f),
        )
        .unwrap();
        assert!(p.is_identity());
        assert_eq!(p.output_size(), (64, 48));
        // 向きが変わると恒等ではない（並べ替えが必要）。
        let f = frames(64, 48, Orientation::Rotate90Cw, 0.0);
        let p = GeometryPlan::new(
            &geometry(0.0, CropRect::FULL),
            f,
            SensorGrid::full_resolution(&f),
        )
        .unwrap();
        assert!(!p.is_identity());
        assert_eq!(p.output_size(), (48, 64));
    }

    #[test]
    fn angle_must_match_the_frames() {
        let f = frames(64, 48, Orientation::Normal, 0.0);
        let err = GeometryPlan::new(
            &geometry(5.0, CropRect::FULL),
            f,
            SensorGrid::full_resolution(&f),
        );
        assert!(matches!(err, Err(PipelineError::InvalidArgument(_))));
    }

    #[test]
    fn preview_grid_keeps_its_density() {
        // 7008 × 4672 の S を 2560 × 1707 のプレビューにした格子。
        let f = frames(7008, 4672, Orientation::Normal, 0.0);
        let grid = SensorGrid::new(2560, 1707, (7008.0, 4672.0)).unwrap();
        let p = GeometryPlan::new(&geometry(0.0, CropRect::FULL), f, grid).unwrap();
        assert!(p.is_identity());
        assert_eq!(p.lattice_size(), (2560, 1707));
        // 左半分の切り抜き。
        let crop = CropRect {
            left: 0.0,
            top: 0.0,
            right: 0.5,
            bottom: 1.0,
        };
        let p = GeometryPlan::new(&geometry(0.0, crop), f, grid).unwrap();
        assert_eq!(p.output_size(), (1280, 1707));
        assert_eq!(p.crop_lattice(), Roi::new(0, 0, 1280, 1707));
    }

    #[test]
    fn rotation_constrains_the_crop_inside_the_image() {
        let f = frames(600, 400, Orientation::Normal, 10.0);
        let (gw, gh) = f.g_size_px();
        assert!(gw > 600.0 && gh > 400.0);
        let c = constrain_crop(&f, &CropRect::FULL);
        assert!(c.width() < 1.0 && c.height() < 1.0);
        // 制限した切り抜きの四隅は画像の内側。
        for (x, y) in [
            (c.left, c.top),
            (c.right, c.top),
            (c.right, c.bottom),
            (c.left, c.bottom),
        ] {
            let (u, v) = f.g_to_oriented(f64::from(x) * gw, f64::from(y) * gh);
            assert!(
                (-1e-3..=600.001).contains(&u) && (-1e-3..=400.001).contains(&v),
                "{u} {v}"
            );
        }
        // 縦横比（画素の単位）は保たれる。
        let before = gw / gh;
        let after = f64::from(c.width()) * gw / (f64::from(c.height()) * gh);
        assert!((before - after).abs() < 1e-4, "{before} {after}");
        // 中心も保たれる。
        assert!((c.left + c.right - 1.0).abs() < 1e-5);
        // 内側に収まる切り抜きは変えない。
        let small = CropRect {
            left: 0.4,
            top: 0.4,
            right: 0.6,
            bottom: 0.6,
        };
        assert_eq!(constrain_crop(&f, &small), small);
        // 回転 0 なら何もしない。
        let f0 = frames(600, 400, Orientation::Normal, 0.0);
        assert_eq!(constrain_crop(&f0, &CropRect::FULL), CropRect::FULL);
    }

    #[test]
    fn crop_with_center_outside_uses_the_image_center() {
        let f = frames(600, 400, Orientation::Normal, 30.0);
        // 左上の隅（画像の外）だけの切り抜き。
        let corner = CropRect {
            left: 0.0,
            top: 0.0,
            right: 0.05,
            bottom: 0.05,
        };
        let c = constrain_crop(&f, &corner);
        assert!((c.left + c.right - 1.0).abs() < 1e-5, "{c:?}");
        assert!((c.top + c.bottom - 1.0).abs() < 1e-5, "{c:?}");
        assert!(c.width() >= MIN_CROP_SIZE && c.height() >= MIN_CROP_SIZE);
    }

    #[test]
    fn output_larger_than_the_pixel_limit_is_an_error() {
        // 極端に細長い画像（30,000,000 × 2）を 45° 回転すると、画像の内側に制限した切り抜きが
        // 最小の大きさ（G の外接矩形の 0.1 %）を下回り、最小の大きさの切り抜きでも C の格子が
        // 約 21213 × 21213（約 4.5 億画素）になる。上限（genzo-raw の MAX_PIXELS）を超える出力は、
        // 巨大なバッファを確保する前にエラーにする。
        let f = frames(30_000_000, 2, Orientation::Normal, 45.0);
        let r = GeometryPlan::new(
            &geometry(45.0, CropRect::FULL),
            f,
            SensorGrid::full_resolution(&f),
        );
        assert!(
            matches!(r, Err(PipelineError::InvalidDimensions { .. })),
            "{:?}",
            r.map(|p| p.output_size())
        );
        // 同じ画像でもプレビューの格子（長辺 2560）なら出力は小さいので作れる。
        let grid = SensorGrid::new(2560, 1, (30_000_000.0, 2.0)).unwrap();
        let p = GeometryPlan::new(&geometry(45.0, CropRect::FULL), f, grid).unwrap();
        assert!(p.output_size().0 * p.output_size().1 < 100_000);
        // 回転しなければ出力は S の寸法以下。
        let f0 = frames(30_000_000, 2, Orientation::Normal, 0.0);
        let p = GeometryPlan::new(
            &geometry(0.0, CropRect::FULL),
            f0,
            SensorGrid::full_resolution(&f0),
        )
        .unwrap();
        assert_eq!(p.output_size(), (30_000_000, 2));
    }

    #[test]
    fn input_roi_covers_every_tap() {
        let f = frames(50, 30, Orientation::Rotate90Cw, 7.0);
        let p = GeometryPlan::new(
            &geometry(7.0, CropRect::FULL),
            f,
            SensorGrid::full_resolution(&f),
        )
        .unwrap();
        let (w, h) = p.output_size();
        for roi in [
            Roi::full(w, h),
            Roi::new(3, 5, 4, 2),
            Roi::new(w as i32 - 1, h as i32 - 1, 1, 1),
        ] {
            let need = p.input_roi(roi);
            let a = p.lattice_to_input_f32();
            for j in roi.y..roi.y + roi.height as i32 {
                for i in roi.x..roi.x + roi.width as i32 {
                    let xx = (i64::from(p.crop_lattice().x) + i64::from(i)) as f32 + 0.5;
                    let yy = (i64::from(p.crop_lattice().y) + i64::from(j)) as f32 + 0.5;
                    let x = a[0][0] * xx + a[0][1] * yy + a[0][2];
                    let y = a[1][0] * xx + a[1][1] * yy + a[1][2];
                    let (ix, _) = cubic_base(x);
                    let (iy, _) = cubic_base(y);
                    assert!(need.contains(ix - 1, iy - 1) && need.contains(ix + 2, iy + 2));
                }
            }
        }
        assert!(p.input_roi(Roi::new(0, 0, 0, 3)).is_empty());
    }
}
