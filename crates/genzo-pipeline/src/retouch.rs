//! スポット修正（DEV-21。修復・コピースタンプ。処理バージョン 2）。docs/04_architecture.md の 2.1 節「スポット
//! 修正」、設計は docs/retouch_design.md。
//!
//! # どこに当てるか
//!
//! 段階 A1 の出力（ステージ 8 の後の B2。S の向きの格子）に当てる。スポットの位置は O 座標（genzo-model の
//! `RetouchSpot` の doc。O は S を並べ替えて平行移動しただけ）なので、角度補正・歪曲補正・切り抜きを後から
//! 変えても、スポットは写っているものの上からずれない。ガイド（段階 B）・段階 C は当てた後の画像を使う
//! （エンジンの [`crate::Engine::a1`] と、フル解像度のタイル（[`crate::FullResolutionJob`]））。
//!
//! # 1 つのスポットの計算
//!
//! 長さはすべて O の画素（S の画素と同じ大きさ）で扱う。スポットの点の列を `P`（1 点なら円、2 点以上なら
//! 折れ線）、半径を `r`、ぼかしを `f`（0〜1）、不透明度を `a`（0〜1）、コピー元のずれを `d` とする。
//!
//! 1. **重み**: 画素の中心 `p`（O）から `P` までの距離 `δ` で、`δ ≤ r(1 − f)` なら 1、`δ ≥ r` なら 0、間は
//!    `1 − smoothstep`（[`spot_weight`]）。
//! 2. **コピー元の値** `s(p)`: 元の画像（スポットを当てる前の A1）の `p + d` の値（格子の上の双線形補間。画像の
//!    外は鏡映）。
//! 3. **修復の補正**（修復のときだけ）: 境界（`P` から距離 `R = r + b/2` の線。`b = max(r/4, 2 画素)`）の上に
//!    標本の点 `q` をとり、`q` のまわり 3 × 3 点（間隔 `h = b/2`）の平均を、元の画像の `q` と `q + d` で求め、
//!    チャンネルごとの比の対数 `D(q) = log2(消したい側の平均) − log2(コピー元の側の平均)`（下限 2^−16、
//!    ±[`MAX_LOG_RATIO`] に制限）を作る。画素ごとの補正 `c(p)` は、
//!    - 円: 境界の円の上の `N` 点（[`circle_sample_count`]）の `D` を、円の Poisson 核
//!      `(R² − |p − 中心|²) / |p − q|²` で重み付けした平均（円の内側で調和関数になる補間）。
//!    - 線: 線に沿って間隔 `h·2` で両側（法線の正の側と負の側）に標本をとり、`p` に最も近い線上の点の弧長で
//!      線形に補間した両側の値を、`p` の符号付きの距離 `σ` で `(σ + R) / 2R` の割合で線形に補間する
//!      （線を横切る方向の 1 次元の調和関数。電線など細い線を想定）。
//!
//!    修復の値は `s(p) · 2^c(p)`（チャンネルごと）。コピースタンプは `s(p)`。
//! 4. **合成**: `出力 = 今の値 + (修復の値 − 今の値) · a · 重み`。
//!
//! スポットは設定の順に当てる。コピー元と境界の値は **スポットを当てる前の画像** から取る（前のスポットの結果を
//! 写さない。タイルの処理で、遠くのコピー元のために前のスポットを計算し直さないため）。重なったスポットは、後の
//! スポットが前の結果の上に重なる。
//!
//! # タイル・解像度との関係
//!
//! 画素ごとの計算は、格子の全体での座標と、元の画像の値だけで決まる（タイルの分け方・スレッド数によらない）。
//! 境界の補正は、スポットごとに 1 回だけ（[`RetouchPlan::prepare`]）求めて、すべてのタイルで使う。プレビュー
//! （長辺 2560px）とフル解像度では、同じ O の位置の値を使うので、補間の差の範囲で一致する（IQ-07b）。

use genzo_model::{DevelopSettings, RetouchMode, RetouchSpot};
use rayon::prelude::*;

use crate::border::{mirror_cover, mirror_index};
use crate::coords::{CoordinateFrames, SensorGrid};
use crate::error::{PipelineError, Result};
use crate::image::{ImageTile, RgbImage, Roi};
use crate::sanitize::safe_log2;
use crate::version::ProcessVersion;

/// 修復の補正の比の対数の上限（チャンネルごと。±8 段）。
pub const MAX_LOG_RATIO: f32 = 8.0;

/// 円の境界の標本の数の下限・上限。
const CIRCLE_SAMPLES_MIN: usize = 16;
const CIRCLE_SAMPLES_MAX: usize = 256;

/// 線の境界の標本の、線に沿った位置の数の上限（画像の外の端どうしを往復する点の列のような、とても長い線で、
/// 標本の数とメモリが際限なく増えないようにする。超える線では間隔を広げる。長辺 6000 画素の写真を斜めに横切る
/// 線は、最小の大きさでも間隔を広げずに収まる。**仮置き**）。
const STROKE_SAMPLES_MAX: usize = 4096;

/// 境界の標本を 1 回に取り出す数（取り出す範囲を小さく保つため）。
const SAMPLES_PER_FETCH: usize = 64;

/// 元の画像（スポットを当てる前の A1）の範囲 `roi`（格子の座標。画像の外にはみ出してよく、はみ出した部分は
/// 鏡映）を返す関数。
pub type Fetch<'a> = dyn Fn(Roi) -> Result<ImageTile> + Sync + 'a;

/// `smoothstep` で縁を 0 にする重み（モジュールの doc の 1）。`inner ≥ radius` なら縁で 1 から 0 に変わる。
#[inline]
pub fn spot_weight(distance: f64, radius: f64, inner: f64) -> f32 {
    if distance >= radius {
        return 0.0;
    }
    if distance <= inner {
        return 1.0;
    }
    let t = (distance - inner) / (radius - inner);
    (1.0 - t * t * (3.0 - 2.0 * t)) as f32
}

/// 半径 `boundary_radius`（O の画素）の円の境界の標本の数（標本の間隔が窓の幅 `2h` 程度になる数。16〜256）。
pub fn circle_sample_count(boundary_radius: f64, h: f64) -> usize {
    let n = (std::f64::consts::TAU * boundary_radius / (2.0 * h)).ceil();
    (n as usize).clamp(CIRCLE_SAMPLES_MIN, CIRCLE_SAMPLES_MAX)
}

/// スポットの形（O の画素）。
#[derive(Debug, Clone, PartialEq)]
enum Path {
    /// 円（中心）。
    Circle([f64; 2]),
    /// 折れ線（点と、各点までの弧長。点は 2 つ以上で、続く点は同じでない）。
    Stroke {
        points: Vec<[f64; 2]>,
        arc: Vec<f64>,
    },
}

/// 折れ線の上の最も近い点。
#[derive(Debug, Clone, Copy)]
struct Nearest {
    /// 距離の 2 乗。
    d2: f64,
    /// 弧長。
    s: f64,
    /// 符号付きの距離（左の法線 `(−dy, dx)` の側が正）。
    sigma: f64,
}

impl Path {
    fn bbox(&self) -> [f64; 4] {
        match self {
            Path::Circle(c) => [c[0], c[1], c[0], c[1]],
            Path::Stroke { points, .. } => points.iter().fold(
                [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ],
                |b, p| {
                    [
                        b[0].min(p[0]),
                        b[1].min(p[1]),
                        b[2].max(p[0]),
                        b[3].max(p[1]),
                    ]
                },
            ),
        }
    }

    /// 線分 `i` の上の、`p` に最も近い点。
    fn nearest_on_segment(points: &[[f64; 2]], arc: &[f64], i: usize, p: [f64; 2]) -> Nearest {
        let (a, b) = (points[i], points[i + 1]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        let (px, py) = (p[0] - a[0], p[1] - a[1]);
        let t = ((px * dx + py * dy) / len2).clamp(0.0, 1.0);
        let (qx, qy) = (px - t * dx, py - t * dy);
        let len = arc[i + 1] - arc[i];
        Nearest {
            d2: qx * qx + qy * qy,
            s: arc[i] + t * len,
            sigma: (px * -dy + py * dx) / len,
        }
    }

    /// 弧長 `s` の位置と、そこでの左の法線（単位ベクトル）。線分の境目では後の線分の法線。
    fn point_at(points: &[[f64; 2]], arc: &[f64], s: f64) -> ([f64; 2], [f64; 2]) {
        let last = points.len() - 2;
        let i = arc[..=last]
            .partition_point(|&v| v <= s)
            .saturating_sub(1)
            .min(last);
        let (a, b) = (points[i], points[i + 1]);
        let len = arc[i + 1] - arc[i];
        let t = ((s - arc[i]) / len).clamp(0.0, 1.0);
        let (dx, dy) = ((b[0] - a[0]) / len, (b[1] - a[1]) / len);
        (
            [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])],
            [-dy, dx],
        )
    }
}

/// 1 つのスポットの計画（O の画素の単位）。
#[derive(Debug, Clone, PartialEq)]
struct SpotPlan {
    mode: RetouchMode,
    path: Path,
    radius: f64,
    inner: f64,
    opacity: f32,
    offset: [f64; 2],
}

impl SpotPlan {
    fn new(spot: &RetouchSpot, long_side: f64) -> Self {
        let pts: Vec<[f64; 2]> = spot
            .points
            .iter()
            .map(|p| [f64::from(p[0]) * long_side, f64::from(p[1]) * long_side])
            .collect();
        // 続く同じ点を除く（長さ 0 の線分を作らない）。
        let mut points: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
        for p in pts {
            if points.last() != Some(&p) {
                points.push(p);
            }
        }
        let path = if points.len() >= 2 {
            let mut arc = Vec::with_capacity(points.len());
            arc.push(0.0);
            for w in points.windows(2) {
                let l = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
                arc.push(arc.last().copied().unwrap_or(0.0) + l);
            }
            Path::Stroke { points, arc }
        } else {
            Path::Circle(points[0])
        };
        let radius = f64::from(spot.size) * long_side;
        let feather = f64::from(spot.feather) / 100.0;
        Self {
            mode: spot.mode,
            path,
            radius,
            inner: radius * (1.0 - feather),
            opacity: spot.opacity / 100.0,
            offset: [
                f64::from(spot.offset[0]) * long_side,
                f64::from(spot.offset[1]) * long_side,
            ],
        }
    }

    /// 境界の帯の幅 `b`（O の画素）。`px` は格子の 1 画素の O の画素の大きさ。
    fn band(&self, px: f64) -> f64 {
        (self.radius / 4.0).max(2.0 * px)
    }

    /// 境界の半径 `R`（中心の線からの距離）。
    fn boundary_radius(&self, px: f64) -> f64 {
        self.radius + self.band(px) / 2.0
    }

    /// 消したい場所の外接矩形（O の画素。`margin` を足す）。
    fn dest_bbox(&self, margin: f64) -> [f64; 4] {
        let b = self.path.bbox();
        let m = self.radius + margin;
        [b[0] - m, b[1] - m, b[2] + m, b[3] + m]
    }

    /// 画素 `p`（O）での重みと、線のときは最も近い点。`seg_boxes` は線分ごとの外接矩形（半径を足したもの）で、
    /// `p` を含む線分だけを調べる（含まない線分までの距離は半径より遠く、重みは 0）。
    fn weight_at(&self, p: [f64; 2], seg_boxes: &[[f64; 4]]) -> (f32, Option<Nearest>) {
        match &self.path {
            Path::Circle(c) => {
                let d = (p[0] - c[0]).hypot(p[1] - c[1]);
                (spot_weight(d, self.radius, self.inner), None)
            }
            Path::Stroke { points, arc } => {
                let mut best: Option<Nearest> = None;
                for (i, b) in seg_boxes.iter().enumerate() {
                    if !(p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]) {
                        continue;
                    }
                    let n = Path::nearest_on_segment(points, arc, i, p);
                    if best.is_none_or(|b| n.d2 < b.d2) {
                        best = Some(n);
                    }
                }
                match best {
                    Some(n) => (spot_weight(n.d2.sqrt(), self.radius, self.inner), Some(n)),
                    None => (0.0, None),
                }
            }
        }
    }

    /// 線の境界の標本の弧長の間隔（窓の幅。線に沿った位置が [`STROKE_SAMPLES_MAX`] を超える長い線では広げる）。
    fn stroke_step(&self, px: f64) -> f64 {
        let step = self.band(px);
        match &self.path {
            Path::Stroke { arc, .. } => step.max(arc[arc.len() - 1] / STROKE_SAMPLES_MAX as f64),
            Path::Circle(_) => step,
        }
    }

    /// 境界の標本の点（O の画素。円は円周の上、線は両側に交互に `[+, −, +, −, …]`）。
    fn boundary_points(&self, px: f64) -> Vec<[f64; 2]> {
        let rb = self.boundary_radius(px);
        let h = self.band(px) / 2.0;
        match &self.path {
            Path::Circle(c) => {
                let n = circle_sample_count(rb, h);
                (0..n)
                    .map(|k| {
                        let t = std::f64::consts::TAU * k as f64 / n as f64;
                        [c[0] + rb * t.cos(), c[1] + rb * t.sin()]
                    })
                    .collect()
            }
            Path::Stroke { points, arc } => {
                let step = self.stroke_step(px);
                let total = arc[arc.len() - 1];
                let m = (total / step).ceil() as usize;
                let mut out = Vec::with_capacity(2 * (m + 1));
                for j in 0..=m {
                    let s = (j as f64 * step).min(total);
                    let (q, n) = Path::point_at(points, arc, s);
                    out.push([q[0] + rb * n[0], q[1] + rb * n[1]]);
                    out.push([q[0] - rb * n[0], q[1] - rb * n[1]]);
                }
                out
            }
        }
    }
}

/// スポットの境界の補正（チャンネルごとの比の対数。[`SpotPlan::boundary_points`] と同じ順）。
#[derive(Debug, Clone)]
struct Boundary {
    points: Vec<[f64; 2]>,
    values: Vec<[f32; 3]>,
}

/// O の画素と格子の座標の対応。
#[derive(Debug, Clone, Copy)]
struct GridMap {
    frames: CoordinateFrames,
    density: (f64, f64),
    size: (u32, u32),
}

impl GridMap {
    fn new(frames: CoordinateFrames, grid: SensorGrid) -> Self {
        Self {
            frames,
            density: grid.density(),
            size: (grid.width, grid.height),
        }
    }

    /// 格子の連続座標 → O。
    fn grid_to_o(&self, gx: f64, gy: f64) -> [f64; 2] {
        let (u, v) = self
            .frames
            .sensor_to_oriented(gx / self.density.0, gy / self.density.1);
        [u, v]
    }

    /// O → 格子の連続座標。
    fn o_to_grid(&self, p: [f64; 2]) -> (f64, f64) {
        let (x, y) = self.frames.oriented_to_sensor(p[0], p[1]);
        (x * self.density.0, y * self.density.1)
    }

    /// 格子の 1 画素の大きさ（O の画素。縦横の大きい方）。
    fn px(&self) -> f64 {
        (1.0 / self.density.0).max(1.0 / self.density.1)
    }

    /// O の矩形 `[x0, y0, x1, y1]` を覆う格子の画素の範囲（画像の外にはみ出してよい）。
    fn o_rect_to_grid(&self, r: [f64; 4]) -> Roi {
        let a = self.o_to_grid([r[0], r[1]]);
        let b = self.o_to_grid([r[2], r[3]]);
        let (x0, x1) = (a.0.min(b.0).floor(), a.0.max(b.0).ceil());
        let (y0, y1) = (a.1.min(b.1).floor(), a.1.max(b.1).ceil());
        Roi::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)
    }

    /// 双線形補間に要る格子の範囲（O の点の列を覆う）。
    fn bilinear_cover(&self, points: impl Iterator<Item = [f64; 2]>) -> Option<Roi> {
        let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
        for p in points {
            let (gx, gy) = self.o_to_grid(p);
            let (x0, y0) = ((gx - 0.5).floor() as i64, (gy - 0.5).floor() as i64);
            b = [
                b[0].min(x0),
                b[1].min(y0),
                b[2].max(x0 + 2),
                b[3].max(y0 + 2),
            ];
        }
        (b[0] < b[2]).then(|| {
            Roi::new(
                b[0] as i32,
                b[1] as i32,
                (b[2] - b[0]) as u32,
                (b[3] - b[1]) as u32,
            )
        })
    }
}

/// 格子の連続座標 (gx, gy)（画素 (i, j) の中心は (i + 0.5, j + 0.5)）の値を、タイルから双線形補間で求める。
/// タイルは [`GridMap::bilinear_cover`] の範囲を覆うこと（覆っていない画素は 0）。
#[inline]
fn bilinear(tile: &ImageTile, gx: f64, gy: f64) -> [f32; 3] {
    let (x, y) = (gx - 0.5, gy - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
    let (xi, yi) = (x0 as i64, y0 as i64);
    let p = |dx: i64, dy: i64| tile.at(xi + dx, yi + dy).unwrap_or([0.0; 3]);
    let (p00, p10, p01, p11) = (p(0, 0), p(1, 0), p(0, 1), p(1, 1));
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let top = p00[c] + (p10[c] - p00[c]) * fx;
        let bottom = p01[c] + (p11[c] - p01[c]) * fx;
        out[c] = top + (bottom - top) * fy;
    }
    out
}

/// 点 `q`（O）のまわり 3 × 3 点（間隔 `h`）の平均。
fn window_mean(tile: &ImageTile, map: &GridMap, q: [f64; 2], h: f64) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    for j in -1..=1 {
        for i in -1..=1 {
            let (gx, gy) = map.o_to_grid([q[0] + f64::from(i) * h, q[1] + f64::from(j) * h]);
            let v = bilinear(tile, gx, gy);
            for c in 0..3 {
                sum[c] += v[c];
            }
        }
    }
    sum.map(|v| v / 9.0)
}

/// 窓の点（[`window_mean`] が使う点）の列。
fn window_points(q: [f64; 2], h: f64) -> impl Iterator<Item = [f64; 2]> {
    (-1..=1).flat_map(move |j| {
        (-1..=1).map(move |i| [q[0] + f64::from(i) * h, q[1] + f64::from(j) * h])
    })
}

/// 消したい側とコピー元の側の平均の比の対数（チャンネルごと。±[`MAX_LOG_RATIO`]）。
fn log_ratio(dest: [f32; 3], src: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        out[c] = (safe_log2(dest[c]) - safe_log2(src[c])).clamp(-MAX_LOG_RATIO, MAX_LOG_RATIO);
    }
    out
}

/// スポット修正の計画（設定の結果に影響するスポットと、O の座標系）。
#[derive(Debug, Clone)]
pub struct RetouchPlan {
    frames: CoordinateFrames,
    spots: Vec<SpotPlan>,
}

impl RetouchPlan {
    /// 設定から作る。処理バージョン 1、または結果に影響するスポット（`RetouchSpot::is_active`）がなければ `None`。
    /// `frames` は段階 A1 の座標系（S の寸法・既定の切り抜き・向き。回転は使わない）。
    pub fn new(settings: &DevelopSettings, frames: CoordinateFrames) -> Result<Option<Self>> {
        if ProcessVersion::from_u32(settings.process_version)? == ProcessVersion::V1 {
            return Ok(None);
        }
        let s = settings.normalized();
        let (ow, oh) = frames.oriented_size();
        let long_side = f64::from(ow.max(oh));
        let spots: Vec<SpotPlan> = s
            .retouch
            .iter()
            .filter(|r| r.is_active())
            .map(|r| SpotPlan::new(r, long_side))
            .collect();
        Ok((!spots.is_empty()).then_some(Self { frames, spots }))
    }

    /// スポットの数。
    pub fn len(&self) -> usize {
        self.spots.len()
    }

    /// スポットがないか（[`new`](Self::new) は空の計画を作らない）。
    pub fn is_empty(&self) -> bool {
        self.spots.is_empty()
    }

    /// 格子 `grid` の上で使う準備（修復の境界の補正を、`fetch` で元の画像から求める）。
    pub fn prepare(&self, grid: SensorGrid, fetch: &Fetch<'_>) -> Result<PreparedRetouch> {
        let map = GridMap::new(self.frames, grid);
        let px = map.px();
        let mut boundaries = Vec::with_capacity(self.spots.len());
        for spot in &self.spots {
            if spot.mode == RetouchMode::Clone {
                boundaries.push(None);
                continue;
            }
            let h = spot.band(px) / 2.0;
            let points = spot.boundary_points(px);
            let mut values = Vec::with_capacity(points.len());
            for chunk in points.chunks(SAMPLES_PER_FETCH) {
                let windows = || chunk.iter().flat_map(|&q| window_points(q, h));
                let off = spot.offset;
                let (Some(dr), Some(sr)) = (
                    map.bilinear_cover(windows()),
                    map.bilinear_cover(windows().map(|p| [p[0] + off[0], p[1] + off[1]])),
                ) else {
                    // 点のある組なので起きない（起きても補正 0 として位置をそろえる）。
                    values.extend(chunk.iter().map(|_| [0.0f32; 3]));
                    continue;
                };
                let dest = fetch(dr)?;
                let src = fetch(sr)?;
                for &q in chunk {
                    let md = window_mean(&dest, &map, q, h);
                    let ms = window_mean(&src, &map, [q[0] + off[0], q[1] + off[1]], h);
                    values.push(log_ratio(md, ms));
                }
            }
            boundaries.push(Some(Boundary { points, values }));
        }
        Ok(PreparedRetouch {
            plan: self.clone(),
            map,
            boundaries,
        })
    }

    /// 画像全体（段階 A1 のプレビュー。格子 `grid`）に当てたもの。
    pub fn apply_to_image(&self, image: &RgbImage, grid: SensorGrid) -> Result<RgbImage> {
        let (w, h) = image.dimensions();
        if (grid.width, grid.height) != (w, h) {
            return Err(PipelineError::InvalidArgument(format!(
                "格子の寸法 {}×{} が画像の寸法 {w}×{h} と違います",
                grid.width, grid.height
            )));
        }
        let fetch = |roi: Roi| crate::engine::gather_mirror_from(image, roi);
        let prepared = self.prepare(grid, &fetch)?;
        let mut tile = ImageTile::from_image(Roi::full(w, h), image.clone())?;
        prepared.apply(&mut tile, &fetch)?;
        Ok(tile.image)
    }
}

/// 格子の上で使う準備のできたスポット修正（[`RetouchPlan::prepare`]）。
#[derive(Debug, Clone)]
pub struct PreparedRetouch {
    plan: RetouchPlan,
    map: GridMap,
    boundaries: Vec<Option<Boundary>>,
}

/// 1 つのスポットの、画素ごとの層（範囲 `roi` の格子の画素ごと）。
struct Layer {
    roi: Roi,
    alpha: Vec<f32>,
    value: Vec<[f32; 3]>,
}

impl PreparedRetouch {
    /// タイル `tile`（格子の座標。画像の外にはみ出してよく、はみ出した画素は鏡映した位置の値として扱う）に、
    /// スポットを順に当てる。`fetch` は元の画像（コピー元）を返す。
    pub fn apply(&self, tile: &mut ImageTile, fetch: &Fetch<'_>) -> Result<()> {
        let (gw, gh) = self.map.size;
        let Some(cover) = mirror_cover(tile.roi, gw, gh) else {
            return Ok(());
        };
        for (k, spot) in self.plan.spots.iter().enumerate() {
            let bbox = self.map.o_rect_to_grid(spot.dest_bbox(self.map.px()));
            let Some(region) = bbox
                .intersect(&cover)
                .and_then(|r| r.intersect(&Roi::full(gw, gh)))
            else {
                continue;
            };
            let Some(layer) = self.layer(spot, self.boundaries[k].as_ref(), region, fetch)? else {
                continue;
            };
            blend(tile, &layer, (gw, gh));
        }
        Ok(())
    }

    /// スポット `spot` の、範囲 `region`（画像の内側）の層。重みがすべて 0 なら `None`。
    fn layer(
        &self,
        spot: &SpotPlan,
        boundary: Option<&Boundary>,
        region: Roi,
        fetch: &Fetch<'_>,
    ) -> Result<Option<Layer>> {
        let map = &self.map;
        let w = region.width as usize;
        let n = region.pixel_count() as usize;
        // 線分ごとの外接矩形（O）。行ごとに、その行と重なる線分だけを調べる。
        let seg_boxes: Vec<[f64; 4]> = match &spot.path {
            Path::Stroke { points, .. } => points
                .windows(2)
                .map(|s| {
                    let m = spot.radius;
                    [
                        s[0][0].min(s[1][0]) - m,
                        s[0][1].min(s[1][1]) - m,
                        s[0][0].max(s[1][0]) + m,
                        s[0][1].max(s[1][1]) + m,
                    ]
                })
                .collect(),
            Path::Circle(_) => Vec::new(),
        };
        let mut alpha = vec![0.0f32; n];
        let mut nearest: Vec<Option<Nearest>> = vec![None; n];
        let mut o_pos = vec![[0.0f64; 2]; n];
        alpha
            .par_chunks_mut(w)
            .zip(nearest.par_chunks_mut(w))
            .zip(o_pos.par_chunks_mut(w))
            .enumerate()
            .for_each(|(j, ((a_row, n_row), o_row))| {
                let gy = f64::from(region.y) + j as f64 + 0.5;
                for i in 0..w {
                    let gx = f64::from(region.x) + i as f64 + 0.5;
                    let p = map.grid_to_o(gx, gy);
                    o_row[i] = p;
                    let (wt, nr) = spot.weight_at(p, &seg_boxes);
                    a_row[i] = wt * spot.opacity;
                    n_row[i] = nr;
                }
            });
        if alpha.iter().all(|&a| a <= 0.0) {
            return Ok(None);
        }
        let off = spot.offset;
        let src_points = o_pos
            .iter()
            .zip(&alpha)
            .filter(|(_, a)| **a > 0.0)
            .map(|(p, _)| [p[0] + off[0], p[1] + off[1]]);
        let Some(src_roi) = map.bilinear_cover(src_points) else {
            return Ok(None);
        };
        let src = fetch(src_roi)?;
        let px = map.px();
        let rb = spot.boundary_radius(px);
        let step = spot.stroke_step(px);
        let mut value = vec![[0.0f32; 3]; n];
        value.par_chunks_mut(w).enumerate().for_each(|(j, row)| {
            for (i, out) in row.iter_mut().enumerate() {
                let idx = j * w + i;
                if alpha[idx] <= 0.0 {
                    continue;
                }
                let p = o_pos[idx];
                let (gx, gy) = map.o_to_grid([p[0] + off[0], p[1] + off[1]]);
                let s = bilinear(&src, gx, gy);
                *out = match boundary {
                    None => s,
                    Some(b) => {
                        let c = match &spot.path {
                            Path::Circle(center) => poisson(b, *center, rb, p),
                            Path::Stroke { .. } => match nearest[idx] {
                                Some(nr) => across(b, step, rb, nr),
                                None => [0.0; 3],
                            },
                        };
                        [s[0] * c[0].exp2(), s[1] * c[1].exp2(), s[2] * c[2].exp2()]
                    }
                };
            }
        });
        Ok(Some(Layer {
            roi: region,
            alpha,
            value,
        }))
    }
}

/// 円の境界の値の、Poisson 核による補間（モジュールの doc の 3）。
fn poisson(b: &Boundary, center: [f64; 2], rb: f64, p: [f64; 2]) -> [f32; 3] {
    let r2 = (p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2);
    let num = (rb * rb - r2).max(0.0);
    let mut acc = [0.0f64; 3];
    let mut wsum = 0.0f64;
    for (q, v) in b.points.iter().zip(&b.values) {
        let d2 = (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2);
        let wt = num / d2.max(1e-12);
        wsum += wt;
        for c in 0..3 {
            acc[c] += wt * f64::from(v[c]);
        }
    }
    if wsum <= 0.0 {
        return [0.0; 3];
    }
    acc.map(|v| (v / wsum) as f32)
}

/// 線の両側の値の補間（モジュールの doc の 3）。標本は `[+, −]` の組が弧長 `step` ごとに並ぶ。
fn across(b: &Boundary, step: f64, rb: f64, nr: Nearest) -> [f32; 3] {
    let pairs = b.values.len() / 2;
    if pairs == 0 {
        return [0.0; 3];
    }
    let t = (nr.s / step).max(0.0);
    let j0 = (t.floor() as usize).min(pairs - 1);
    let j1 = (j0 + 1).min(pairs - 1);
    let f = (t - j0 as f64).clamp(0.0, 1.0) as f32;
    let side = |j: usize, k: usize| b.values[2 * j + k];
    let u = ((nr.sigma + rb) / (2.0 * rb)).clamp(0.0, 1.0) as f32;
    std::array::from_fn(|c| {
        let plus = side(j0, 0)[c] + (side(j1, 0)[c] - side(j0, 0)[c]) * f;
        let minus = side(j0, 1)[c] + (side(j1, 1)[c] - side(j0, 1)[c]) * f;
        minus + (plus - minus) * u
    })
}

/// 層をタイルに重ねる（タイルの画素は、鏡映した位置が層の範囲に入れば重ねる）。
fn blend(tile: &mut ImageTile, layer: &Layer, size: (u32, u32)) {
    let roi = tile.roi;
    let w = roi.width as usize;
    let lw = layer.roi.width as usize;
    tile.image
        .data_mut()
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(j, row)| {
            let my = i64::from(mirror_index(i64::from(roi.y) + j as i64, size.1));
            if !(i64::from(layer.roi.y)..layer.roi.y_end()).contains(&my) {
                return;
            }
            let ly = (my - i64::from(layer.roi.y)) as usize;
            for (i, px) in row.iter_mut().enumerate() {
                let mx = i64::from(mirror_index(i64::from(roi.x) + i as i64, size.0));
                if !(i64::from(layer.roi.x)..layer.roi.x_end()).contains(&mx) {
                    continue;
                }
                let idx = ly * lw + (mx - i64::from(layer.roi.x)) as usize;
                let a = layer.alpha[idx];
                if a <= 0.0 {
                    continue;
                }
                let v = layer.value[idx];
                for c in 0..3 {
                    px[c] += (v[c] - px[c]) * a;
                }
            }
        });
}

/// [`oriented_grid`] の格子の一辺の点の数。双線形補間の誤差は升目の大きさの 2 乗に比例し、歪曲補正 ±100（手動の
/// 最大）の長辺 6000 画素の写真で最大 0.64 画素（一辺 9 点では 9.3 画素で、最小のスポットの半径 3 画素より大きかった。
/// テストで確かめる）。
pub const ORIENTED_GRID_SIZE: usize = 33;

/// 出力の画像（C の格子の全体。`geometry` は段階 C の [`GeometryPlan`]）の上の点の O 座標（長辺を 1 とする。
/// スポットの位置の座標）の表。画面がスポットを出力の画像の上に描き、クリックした位置をスポットの位置にするための
/// 対応（回転・歪曲補正があると C と O の対応は線形でないので、表を双線形補間する）。
///
/// 点 `(i, j)`（`0 ≤ i, j < n`、`n` = [`ORIENTED_GRID_SIZE`]）は出力の画像の幅・高さに対する割合 `(i / (n − 1),
/// j / (n − 1))` の位置で、`j · n + i` 番目に入る。
pub fn oriented_grid(geometry: &crate::finish::GeometryPlan) -> Vec<[f32; 2]> {
    oriented_grid_of(geometry, ORIENTED_GRID_SIZE)
}

/// 一辺 `n` 点の [`oriented_grid`]（テストで、表の細かさと誤差を比べる）。
fn oriented_grid_of(geometry: &crate::finish::GeometryPlan, n: usize) -> Vec<[f32; 2]> {
    let frames = geometry.frames();
    let [l, t, r, b] = crate::local::g_bounds(geometry);
    let g_long = frames.g_long_side_px();
    let (ow, oh) = frames.oriented_size();
    let o_long = f64::from(ow.max(oh));
    let k = (n - 1) as f64;
    (0..n * n)
        .map(|idx| {
            let (fx, fy) = ((idx % n) as f64 / k, (idx / n) as f64 / k);
            let gx = (l + fx * (r - l)) * g_long;
            let gy = (t + fy * (b - t)) * g_long;
            let (u, v) = frames.g_to_oriented(gx, gy);
            [(u / o_long) as f32, (v / o_long) as f32]
        })
        .collect()
}

/// コピー元の候補のずれの距離（半径の倍数）。
const SOURCE_DISTANCES: [f64; 3] = [2.5, 3.5, 5.0];
/// コピー元の候補の向きの数。
const SOURCE_DIRECTIONS: usize = 16;
/// 線の重なりを調べる点（コピー元の線の上に弧長で等間隔にとる）の数の上限（長い線では間隔を広げる）。
const OVERLAP_POINTS: usize = 400;

/// スポット `spot`（ずれは使わない）の、よいコピー元のずれ（O 座標）を、画像 `image`（格子 `grid`、座標系
/// `frames`）から探す（DEV-21 の「コピー元の自動の選び方」）。
///
/// 候補は、向き 16 通り × 距離（半径の 2.5・3.5・5 倍）。コピー元が O の枠からはみ出す候補と、線のときに
/// コピー元が消したい線と重なる（線どうしの距離が直径より近い）候補を除き（重なりは、コピー元の線の上に弧長で
/// 間隔 `h`（半径の半分。点が [`OVERLAP_POINTS`] を超えるなら広げる）でとった点から消したい線までの距離が
/// `直径 + h/2` より近いかで調べる。点の間で線が交わっても見逃さない）、境界の帯の上の点（修復の境界の
/// 標本と同じ点）で、消したい側とコピー元の側の輝度の対数の差の 2 乗の平均が最も小さい候補を選ぶ（同じなら
/// 先の候補）。候補がなければ `None`。
pub fn find_source_offset(
    image: &RgbImage,
    grid: SensorGrid,
    frames: CoordinateFrames,
    spot: &RetouchSpot,
) -> Result<Option<[f32; 2]>> {
    if spot.points.is_empty() {
        return Ok(None);
    }
    let (ow, oh) = frames.oriented_size();
    let long_side = f64::from(ow.max(oh));
    let plan = SpotPlan::new(
        &RetouchSpot {
            offset: [0.0, 0.0],
            ..spot.clone()
        },
        long_side,
    );
    let map = GridMap::new(frames, grid);
    let px = map.px();
    let whole = ImageTile::from_image(Roi::full(grid.width, grid.height), image.clone())?;
    let ring = plan.boundary_points(px);
    let lum = |v: [f32; 3]| safe_log2(crate::finish::luminance(v));
    let dest: Vec<f32> = ring
        .iter()
        .map(|&q| {
            let (gx, gy) = map.o_to_grid(q);
            lum(bilinear(&whole, gx, gy))
        })
        .collect();
    let bbox = plan.dest_bbox(0.0);
    // 線の重なりを調べる点（弧長で等間隔。両端を含む）と、その間隔の半分だけ広げた距離の上限。
    let (probe, limit) = match &plan.path {
        Path::Circle(c) => (vec![*c], 0.0),
        Path::Stroke { points, arc } => {
            let total = arc[arc.len() - 1];
            let h = (plan.radius / 2.0).max(total / OVERLAP_POINTS as f64);
            let n = (total / h).ceil() as usize;
            let probe = (0..=n)
                .map(|j| Path::point_at(points, arc, (j as f64 * h).min(total)).0)
                .collect();
            (probe, 2.0 * plan.radius + h / 2.0)
        }
    };
    let mut best: Option<(f64, [f64; 2])> = None;
    for &m in &SOURCE_DISTANCES {
        for k in 0..SOURCE_DIRECTIONS {
            let t = std::f64::consts::TAU * k as f64 / SOURCE_DIRECTIONS as f64;
            let d = [m * plan.radius * t.cos(), m * plan.radius * t.sin()];
            // コピー元が O の枠の中にあること。
            if bbox[0] + d[0] < 0.0
                || bbox[1] + d[1] < 0.0
                || bbox[2] + d[0] > f64::from(ow)
                || bbox[3] + d[1] > f64::from(oh)
            {
                continue;
            }
            // 線のときは、コピー元の線が消したい線と重ならないこと。
            if let Path::Stroke { points, arc } = &plan.path {
                let limit2 = limit * limit;
                let overlaps = probe.iter().any(|p| {
                    let q = [p[0] + d[0], p[1] + d[1]];
                    (0..points.len() - 1).any(|i| {
                        // 線分の外接矩形から上限より遠い点は、距離を求めない。
                        let (a, b) = (points[i], points[i + 1]);
                        if q[0] < a[0].min(b[0]) - limit
                            || q[0] > a[0].max(b[0]) + limit
                            || q[1] < a[1].min(b[1]) - limit
                            || q[1] > a[1].max(b[1]) + limit
                        {
                            return false;
                        }
                        Path::nearest_on_segment(points, arc, i, q).d2 < limit2
                    })
                });
                if overlaps {
                    continue;
                }
            }
            let mut cost = 0.0f64;
            for (q, dv) in ring.iter().zip(&dest) {
                let (gx, gy) = map.o_to_grid([q[0] + d[0], q[1] + d[1]]);
                let diff = f64::from(dv - lum(bilinear(&whole, gx, gy)));
                cost += diff * diff;
            }
            cost /= ring.len().max(1) as f64;
            if best.is_none_or(|(c, _)| cost < c) {
                best = Some((cost, d));
            }
        }
    }
    Ok(best.map(|(_, d)| [(d[0] / long_side) as f32, (d[1] / long_side) as f32]))
}

#[cfg(test)]
mod tests;
