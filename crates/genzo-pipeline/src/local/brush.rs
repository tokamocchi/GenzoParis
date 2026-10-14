//! ブラシの部品のラスター（DEV-18。[`super`] の「ブラシ」）。
//!
//! ブラシの部品（genzo-model の `MaskShape::Brush`）のストロークを、**G 座標の決まった格子**（G の外接矩形の長辺を
//! [`BRUSH_RASTER_CELLS`] 個のセルに分けたもの）に塗ったラスターにしてから、画素ごとに双線形で引く。格子は G 座標で
//! 決まるので、プレビュー・等倍・書き出し（解像度・切り抜き・タイルの分け方）によらず同じラスターを使う（IQ-07b）。
//! 塗るのは CPU だけで、GPU 版（genzo-gpu）は同じラスターを補助のバッファで受け取って、同じ式で引く（2.3 節）。
//!
//! # 塗り方（f32、この順序）
//!
//! ストロークごとに、セルの中心 `p` と折れ線（点の列を順に結んだもの。1 点なら点）の距離 `d` から
//!
//! ```text
//! t   = d / 大きさ（半径）
//! i0  = 1 − ぼかし / 100
//! cov = t ≥ 1 なら 0、t ≤ i0 なら 1、それ以外は 1 − smoothstep(i0, 1, t)       （i0 = 1（ぼかし 0）なら t < 1 で 1）
//! a   = 流量 / 100 × cov
//! 描く:   m ← m + max(0, 密度 / 100 − m) · a                                   （密度までしか上がらない。下げない）
//! 消す:   m ← m · (1 − 密度 / 100 · a)
//! ```
//!
//! を、塗った順に当てる（`m` はセルの値。初めは 0）。距離は線分ごとの最短距離の最小（f32。線分 a–b への射影
//! `t = clamp(((p − a)·(b − a)) · (1 / |b − a|²), 0, 1)` の点 q との `d² = |p − q|²` の平方根）。ラスターの範囲は、
//! ストロークの点 ± 大きさの外接矩形を G の外接矩形（と 2 セルの余白）で切ったもの。範囲の外の値は 0
//! （[`BrushRaster::sample`]）。
//!
//! 距離は、すべてのセルですべての線分を見るのではなく、線分ごとの **帯**（線分に沿って幅 ± 大きさ）と点ごとの
//! **扇形**（半径は大きさ。両側の線分の帯の外。両端の点は半円）の中のセルだけで、その線分（扇形は両側の線分）に
//! ついて求める（`Region`。折れ線の最も近い点は、どれかの線分の内側か点なので、最も近い線分の距離を必ず求める）。
//! 帯・扇形は f32 の丸めのぶん少し広げる。結果は、すべての線分を見たときとビット単位で同じ（単体テストで確かめる）。
//! 計算量はストロークが覆うセルの数にほぼ比例し、点の間隔（画面は大きさの 1/8 まで間引く）によらない。
//! 塗るときは行のまとまりごとに並列にする（セルごとの値は並列の分け方によらない）。
//!
//! # 決めごと（**仮置き**。決定事項ログ No.151）
//!
//! - 格子の細かさ [`BRUSH_RASTER_CELLS`] = 2048（長辺 6000px の書き出しでは 1 セル ≒ 2.9 画素。ぼかし 0 のブラシの縁は
//!   約 3 画素でなめらかになる）。メモリは G の全体を覆っても 2048 × 1366 × 4 バイト ≒ 11 MB。
//! - 作ったラスターは、ストロークと G の外接矩形が同じなら使い回す（[`raster_for`]。最近の [`CACHE_ENTRIES`] 件・
//!   [`CACHE_BYTES`] まで）。スライダーを動かすたびに塗り直さない。ストロークを後ろに足しただけなら、前のラスター
//!   （先頭のストロークが一致するもの）に足したストロークだけを塗り足す（ブラシで 1 本描くたびに全部を塗り直さない）。
//!   塗り足した結果は、初めから塗ったものとビット単位で同じ（範囲が広がった部分は、前のストロークでは 0 のまま）。

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use genzo_model::BrushStroke;
use rayon::prelude::*;

/// ラスターの格子の細かさ（G の外接矩形の長辺あたりのセルの数。**仮置き**）。
pub const BRUSH_RASTER_CELLS: f32 = 2048.0;
/// 使い回すラスターの件数の上限。
pub const CACHE_ENTRIES: usize = 16;
/// 使い回すラスターのバイト数の上限（**仮置き**: 128 MiB）。
pub const CACHE_BYTES: usize = 128 << 20;

/// ブラシの部品のラスター（G 座標の格子。モジュールの doc）。
#[derive(Debug, Clone, PartialEq)]
pub struct BrushRaster {
    /// セル (0, 0) の左上の G 座標。
    pub origin: [f32; 2],
    /// セルの大きさの逆数（G 座標の 1 あたりのセルの数。[`BRUSH_RASTER_CELLS`]）。
    pub inv_cell: f32,
    /// 幅（セル）。
    pub width: u32,
    /// 高さ（セル）。
    pub height: u32,
    /// 値（0〜1。行優先）。
    pub values: Vec<f32>,
}

impl BrushRaster {
    /// G 座標 (u, v) の値（双線形。範囲の外のセルは 0）。
    ///
    /// ```text
    /// fx = (u − origin.x) · inv_cell − 0.5、 fy も同じ
    /// x0 = floor(fx)、 tx = fx − x0（y も同じ）
    /// 値 = 双線形（a + (b − a)·tx の行の後に列。genzo-pipeline の interp と同じ順）
    /// ```
    #[inline]
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        let fx = (u - self.origin[0]) * self.inv_cell - 0.5;
        let fy = (v - self.origin[1]) * self.inv_cell - 0.5;
        if !(fx.is_finite() && fy.is_finite()) {
            return 0.0;
        }
        let x0f = fx.floor();
        let y0f = fy.floor();
        let (w, h) = (self.width as f32, self.height as f32);
        // 範囲の外（隣のセルも外）なら 0。
        if x0f < -1.0 || y0f < -1.0 || x0f >= w || y0f >= h {
            return 0.0;
        }
        let tx = fx - x0f;
        let ty = fy - y0f;
        let x0 = x0f as i64;
        let y0 = y0f as i64;
        let at = |x: i64, y: i64| -> f32 {
            if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
                0.0
            } else {
                self.values[y as usize * self.width as usize + x as usize]
            }
        };
        let a = at(x0, y0);
        let b = at(x0 + 1, y0);
        let c = at(x0, y0 + 1);
        let d = at(x0 + 1, y0 + 1);
        let top = a + (b - a) * tx;
        let bottom = c + (d - c) * tx;
        top + (bottom - top) * ty
    }

    /// 値のバイト数。
    pub fn bytes(&self) -> usize {
        self.values.len() * 4
    }
}

/// 0〜1 のなめらかな段（[`crate::finish::frequency::smoothstep`] と同じ式）。
#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// ストロークの覆い方（モジュールの doc の `cov`。`t` は距離 / 大きさ、`i0` は 1 − ぼかし / 100）。
#[inline]
pub fn coverage(t: f32, i0: f32) -> f32 {
    if t >= 1.0 {
        0.0
    } else if t <= i0 {
        1.0
    } else {
        1.0 - smoothstep(i0, 1.0, t)
    }
}

/// 線分（1 点のストロークなら同じ点の線分）。
#[derive(Debug, Clone, Copy)]
struct Segment {
    a: [f32; 2],
    d: [f32; 2],
    /// `1 / |b − a|²`（長さ 0 なら 0）。
    inv_len2: f32,
}

impl Segment {
    fn new(a: [f32; 2], b: [f32; 2]) -> Self {
        let d = [b[0] - a[0], b[1] - a[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        Self {
            a,
            d,
            inv_len2: if len2 > 0.0 { 1.0 / len2 } else { 0.0 },
        }
    }

    /// 点 (px, py) との距離の 2 乗（f32。モジュールの doc の式）。
    #[inline]
    fn dist2(&self, px: f32, py: f32) -> f32 {
        let (qx, qy) = (px - self.a[0], py - self.a[1]);
        let t = ((qx * self.d[0] + qy * self.d[1]) * self.inv_len2).clamp(0.0, 1.0);
        let (ex, ey) = (qx - t * self.d[0], qy - t * self.d[1]);
        ex * ex + ey * ey
    }
}

/// 距離を求めるセルの領域（凸。円と半平面 `n·p ≤ h` の交わり）と、そこで距離を求める線分（1 つか 2 つ）。
///
/// 折れ線の最も近い点は、ある線分の内側（その線分の帯の中）か、点（その点の両側の線分の帯の外の扇形の中）なので、
/// 線分ごとの帯（線分に沿って幅 ± 大きさ）と、点ごとの扇形（半径は大きさ。両端の点は半円）で、それぞれの線分の距離を
/// 求めれば、すべてのセルで最も近い線分の距離を求めたことになる。帯・扇形は丸めのぶん [`REGION_SLACK`] だけ広げる。
#[derive(Debug, Clone, Copy)]
struct Region {
    /// 行の範囲（ラスターの中の番号）。
    y0: i64,
    y1: i64,
    /// 円（中心と半径の 2 乗）。
    disc: Option<([f32; 2], f32)>,
    /// 半平面 `n·p ≤ h` の `(n, h)`。
    planes: [([f32; 2], f32); 4],
    plane_count: usize,
    /// 距離を求める線分の番号。
    segs: [usize; 2],
    seg_count: usize,
}

impl Region {
    /// 中心 `cy` の行で、領域に入りうる G 座標の横の範囲（空なら `None`）。
    fn row_span(&self, cy: f32) -> Option<(f32, f32)> {
        let (mut lo, mut hi) = (f32::NEG_INFINITY, f32::INFINITY);
        if let Some((c, rr)) = self.disc {
            let dy = cy - c[1];
            let left = rr - dy * dy;
            if left < 0.0 {
                return None;
            }
            let half = left.sqrt();
            lo = c[0] - half;
            hi = c[0] + half;
        }
        for &(n, h) in &self.planes[..self.plane_count] {
            let q = h - n[1] * cy;
            if n[0] > 0.0 {
                hi = hi.min(q / n[0]);
            } else if n[0] < 0.0 {
                lo = lo.max(q / n[0]);
            } else if q < 0.0 {
                return None;
            }
        }
        (lo <= hi).then_some((lo, hi))
    }
}

/// ラスターの範囲（G の格子のセルの番号。左上のセルと幅・高さ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Extent {
    ix0: i64,
    iy0: i64,
    width: u32,
    height: u32,
}

impl Extent {
    /// ストローク `strokes` の範囲（モジュールの doc）。ストロークがない・G の外なら `None`。
    fn of(strokes: &[BrushStroke], g_box: (f32, f32)) -> Option<Self> {
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for s in strokes {
            for p in &s.points {
                for k in 0..2 {
                    lo[k] = lo[k].min(p[k] - s.size);
                    hi[k] = hi[k].max(p[k] + s.size);
                }
            }
        }
        if !(lo[0].is_finite() && hi[0].is_finite()) {
            return None;
        }
        // G の外接矩形（と 2 セルの余白）で切る。セルの格子は G 座標の 0 にそろえる（同じストロークなら位置によらず
        // 同じセルの中心を使う）。
        let margin = 2.0 / BRUSH_RASTER_CELLS;
        let x0 = lo[0].max(-margin);
        let y0 = lo[1].max(-margin);
        let x1 = hi[0].min(g_box.0 + margin);
        let y1 = hi[1].min(g_box.1 + margin);
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        let ix0 = (x0 * BRUSH_RASTER_CELLS).floor() as i64;
        let iy0 = (y0 * BRUSH_RASTER_CELLS).floor() as i64;
        let ix1 = (x1 * BRUSH_RASTER_CELLS).ceil() as i64;
        let iy1 = (y1 * BRUSH_RASTER_CELLS).ceil() as i64;
        Some(Self {
            ix0,
            iy0,
            width: (ix1 - ix0).max(1) as u32,
            height: (iy1 - iy0).max(1) as u32,
        })
    }

    fn len(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// `inner` をすべて含むか。
    fn contains(&self, inner: &Extent) -> bool {
        self.ix0 <= inner.ix0
            && self.iy0 <= inner.iy0
            && inner.ix0 + i64::from(inner.width) <= self.ix0 + i64::from(self.width)
            && inner.iy0 + i64::from(inner.height) <= self.iy0 + i64::from(self.height)
    }

    /// 値 `values` のラスター。
    fn raster(&self, values: Vec<f32>) -> BrushRaster {
        let cell = 1.0 / BRUSH_RASTER_CELLS;
        BrushRaster {
            origin: [self.ix0 as f32 * cell, self.iy0 as f32 * cell],
            inv_cell: BRUSH_RASTER_CELLS,
            width: self.width,
            height: self.height,
            values,
        }
    }
}

/// セルの番号 `i`（範囲の左上 `base` から）の中心の G 座標。
#[inline]
fn center(i: i64, base: i64) -> f32 {
    ((base + i) as f32 + 0.5) / BRUSH_RASTER_CELLS
}

/// 距離を求める行のまとまり（並列の単位）。
const BAND_ROWS: usize = 16;

/// 帯・扇形を広げる幅（G 座標。セルの 1/64。境の判定の f32 の丸め（1e−7 程度）より十分大きい）。
const REGION_SLACK: f32 = 1.0 / (64.0 * BRUSH_RASTER_CELLS);

/// G 座標 `v` のセルの番号（範囲の左上 `base` から）を、1 セル外へ広げて `[lo, hi]` に収めたもの
/// （`low` なら下側（floor − 1）、それ以外は上側（ceil + 1））。
#[inline]
fn cell_bound(v: f32, base: i64, low: bool, lo: i64, hi: i64) -> i64 {
    let c = v * BRUSH_RASTER_CELLS;
    let c = if low { c.floor() - 1.0 } else { c.ceil() + 1.0 };
    (c.clamp(-1e9, 1e9) as i64 - base).clamp(lo, hi)
}

/// G 座標 `v` 以上（`low`）・以下（それ以外）の中心を持つセルの番号の端（範囲の左上 `base` から。`low` なら最初の
/// 番号、それ以外は最後の番号 + 1）を `[0, len]` に収めたもの。
#[inline]
fn center_index(v: f32, base: i64, low: bool, len: i64) -> i64 {
    let c = v * BRUSH_RASTER_CELLS - 0.5;
    let c = if low { c.ceil() } else { c.floor() + 1.0 };
    (c.clamp(-1e9, 1e9) as i64 - base).clamp(0, len)
}

/// ストローク `s` の、距離を求める領域（[`Region`]）と線分。行が範囲 `e` の外の領域は除く。
fn regions(s: &BrushStroke, r: f32, e: Extent) -> (Vec<Segment>, Vec<Region>) {
    // 同じ点が続くところは 1 点にする（折れ線は変わらない）。
    let mut pts: Vec<[f32; 2]> = Vec::with_capacity(s.points.len());
    for p in &s.points {
        if pts.last() != Some(p) {
            pts.push(*p);
        }
    }
    // 帯の幅・円の半径（丸めのぶん広げる）。
    let reach = r + REGION_SLACK;
    let h = i64::from(e.height);
    let rows = |lo: f32, hi: f32| {
        (
            cell_bound(lo - reach, e.iy0, true, 0, h),
            cell_bound(hi + reach, e.iy0, false, 0, h),
        )
    };
    let dot = |n: [f32; 2], p: [f32; 2]| n[0] * p[0] + n[1] * p[1];
    let mut out = Vec::new();
    let mut push = |region: Region| {
        if region.y0 < region.y1 {
            out.push(region);
        }
    };
    if pts.len() == 1 {
        let v = pts[0];
        let (y0, y1) = rows(v[1], v[1]);
        push(Region {
            y0,
            y1,
            disc: Some((v, reach * reach)),
            planes: [([0.0; 2], 0.0); 4],
            plane_count: 0,
            segs: [0, 0],
            seg_count: 1,
        });
        return (vec![Segment::new(v, v)], out);
    }
    let segments: Vec<Segment> = pts.windows(2).map(|w| Segment::new(w[0], w[1])).collect();
    // 帯の端・扇形の縁も広げる（点 v で向き d の境は d·p ≥ d·v − |d|·REGION_SLACK）。
    let slack = |d: [f32; 2]| (d[0] * d[0] + d[1] * d[1]).sqrt() * REGION_SLACK;
    for (k, g) in segments.iter().enumerate() {
        let b = [g.a[0] + g.d[0], g.a[1] + g.d[1]];
        let len = (g.d[0] * g.d[0] + g.d[1] * g.d[1]).sqrt();
        let nrm = [-g.d[1] / len, g.d[0] / len];
        let neg = |n: [f32; 2]| [-n[0], -n[1]];
        let (y0, y1) = rows(g.a[1].min(b[1]), g.a[1].max(b[1]));
        // 帯: (p − a)·d ≥ 0、(p − b)·d ≤ 0、|nrm·(p − a)| ≤ 大きさ。
        push(Region {
            y0,
            y1,
            disc: None,
            planes: [
                (neg(g.d), -dot(g.d, g.a) + slack(g.d)),
                (g.d, dot(g.d, b) + slack(g.d)),
                (nrm, dot(nrm, g.a) + reach),
                (neg(nrm), -dot(nrm, g.a) + reach),
            ],
            plane_count: 4,
            segs: [k, k],
            seg_count: 1,
        });
    }
    // 点ごとの扇形: 前の線分の向き dp で (p − v)·dp ≥ 0、次の線分の向き dn で (p − v)·dn ≤ 0。
    for (i, &v) in pts.iter().enumerate() {
        let (y0, y1) = rows(v[1], v[1]);
        let mut planes = [([0.0f32; 2], 0.0f32); 4];
        let mut plane_count = 0;
        let mut segs = [0usize; 2];
        let mut seg_count = 0;
        if i > 0 {
            let dp = segments[i - 1].d;
            planes[plane_count] = ([-dp[0], -dp[1]], -dot(dp, v) + slack(dp));
            plane_count += 1;
            segs[seg_count] = i - 1;
            seg_count += 1;
        }
        if i < segments.len() {
            let dn = segments[i].d;
            planes[plane_count] = (dn, dot(dn, v) + slack(dn));
            plane_count += 1;
            segs[seg_count] = i;
            seg_count += 1;
        }
        push(Region {
            y0,
            y1,
            disc: Some((v, reach * reach)),
            planes,
            plane_count,
            segs,
            seg_count,
        });
    }
    (segments, out)
}

/// ストローク `s` を、範囲 `e` の値 `values` に塗る（モジュールの doc の式）。
///
/// 距離は、線分ごとの帯と点ごとの扇形（[`Region`]）の中のセルだけで、その線分について求める（すべてのセルで最も
/// 近い線分の距離を求めたことになり、領域の外のセルは距離が大きさ以上）。計算量は、ストロークが覆うセルの数に
/// ほぼ比例する（点の間隔が細かくても、1 つのセルの距離を求める回数はほぼ変わらない）。
fn paint(values: &mut [f32], e: Extent, s: &BrushStroke) {
    if s.points.is_empty() {
        return;
    }
    let r = s.size.max(1e-6);
    let i0 = 1.0 - s.feather / 100.0;
    let flow = s.flow / 100.0;
    let density = s.density / 100.0;
    let (segments, regions) = regions(s, r, e);
    if regions.is_empty() {
        return;
    }
    let w = i64::from(e.width);
    let wu = e.width as usize;
    values
        .par_chunks_mut(wu * BAND_ROWS)
        .enumerate()
        .for_each(|(band, chunk)| {
            let b0 = (band * BAND_ROWS) as i64;
            let b1 = b0 + (chunk.len() / wu) as i64;
            // セルごとの距離の 2 乗の最小（このまとまりの行）。
            let mut d2: Option<Vec<f32>> = None;
            for g in &regions {
                for j in g.y0.max(b0)..g.y1.min(b1) {
                    let cy = center(j, e.iy0);
                    let Some((lo, hi)) = g.row_span(cy) else {
                        continue;
                    };
                    // 中心が [lo, hi] に入るセル（中心は ((番号) + 0.5) / BRUSH_RASTER_CELLS）。
                    let i_lo = center_index(lo, e.ix0, true, w);
                    let i_hi = center_index(hi, e.ix0, false, w);
                    if i_lo >= i_hi {
                        continue;
                    }
                    let buf = d2.get_or_insert_with(|| vec![f32::INFINITY; chunk.len()]);
                    let row = &mut buf[(j - b0) as usize * wu..][..wu];
                    for i in i_lo..i_hi {
                        let cx = center(i, e.ix0);
                        let slot = &mut row[i as usize];
                        for &k in &g.segs[..g.seg_count] {
                            *slot = slot.min(segments[k].dist2(cx, cy));
                        }
                    }
                }
            }
            let Some(d2) = d2 else {
                return;
            };
            for (m, &dd) in chunk.iter_mut().zip(&d2) {
                if !dd.is_finite() {
                    continue;
                }
                let cov = coverage(dd.sqrt() / r, i0);
                if cov <= 0.0 {
                    continue;
                }
                let a = flow * cov;
                if s.erase {
                    *m *= 1.0 - density * a;
                } else {
                    *m += (density - *m).max(0.0) * a;
                }
            }
        });
}

/// ストロークを塗ったラスターを作る（モジュールの doc）。`g_box` は G の外接矩形の大きさ（G 座標。長辺が 1）。
/// ストロークがない・範囲が G の外なら `None`（部品の値は常に 0）。
pub fn rasterize(strokes: &[BrushStroke], g_box: (f32, f32)) -> Option<BrushRaster> {
    extend(None, strokes, g_box).map(|(r, _)| r)
}

/// `base`（`strokes` の先頭の `n` 本を塗ったラスターとその範囲）に、残りのストロークを塗り足したラスター。`base` が
/// `None` なら初めから塗る。結果は [`rasterize`] と同じ値になる（先頭の `n` 本の範囲の外のセルは、その `n` 本では
/// 0 のまま）。
fn extend(
    base: Option<(usize, &BrushRaster, Extent)>,
    strokes: &[BrushStroke],
    g_box: (f32, f32),
) -> Option<(BrushRaster, Extent)> {
    let e = Extent::of(strokes, g_box)?;
    let (done, mut values) = match base {
        Some((n, r, be)) if be == e => (n, r.values.clone()),
        Some((n, r, be)) if e.contains(&be) => {
            let mut v = vec![0.0f32; e.len()];
            let (bw, w) = (be.width as usize, e.width as usize);
            let dx = (be.ix0 - e.ix0) as usize;
            let dy = (be.iy0 - e.iy0) as usize;
            for (j, row) in r.values.chunks_exact(bw).enumerate() {
                let at = (dy + j) * w + dx;
                v[at..at + bw].copy_from_slice(row);
            }
            (n, v)
        }
        _ => (0, vec![0.0f32; e.len()]),
    };
    for s in &strokes[done..] {
        paint(&mut values, e, s);
    }
    Some((e.raster(values), e))
}

/// 使い回すラスター（ストローク・G の外接矩形 → ラスターとその範囲。新しいものを後ろに置く）。
type CacheEntry = (
    Vec<BrushStroke>,
    (u32, u32),
    Option<(Arc<BrushRaster>, Extent)>,
);

fn cache() -> &'static Mutex<Vec<CacheEntry>> {
    static CACHE: OnceLock<Mutex<Vec<CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

/// 使い回すラスターをすべて捨てる（計測のコールドの回など。次の描画で塗り直す）。
pub fn clear_cache() {
    cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// [`rasterize`] の結果を使い回す。同じストローク・同じ G の外接矩形なら前に作ったものを返す。ストロークを後ろに
/// 足しただけなら（前に作ったもののストロークが先頭に一致すれば、いちばん長いものから）、足したストロークだけを
/// 塗り足す（ブラシで 1 本描くたびに、それまでのストロークを塗り直さない）。
pub fn raster_for(strokes: &[BrushStroke], g_box: (f32, f32)) -> Option<Arc<BrushRaster>> {
    let key = (g_box.0.to_bits(), g_box.1.to_bits());
    let base = {
        let mut c = cache().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = c.iter().position(|(s, k, _)| *k == key && s == strokes) {
            let e = c.remove(i);
            let r = e.2.as_ref().map(|(r, _)| r.clone());
            c.push(e);
            return r;
        }
        c.iter()
            .filter(|(s, k, _)| *k == key && s.len() < strokes.len() && strokes.starts_with(s))
            .max_by_key(|(s, _, _)| s.len())
            .map(|(s, _, r)| (s.len(), r.clone()))
    };
    // 先頭のストロークのラスターがない（G の外）なら、初めから塗る（G の外のストロークは何も塗らない）。
    let entry = match &base {
        Some((n, Some((r, be)))) => extend(Some((*n, r, *be)), strokes, g_box),
        _ => extend(None, strokes, g_box),
    }
    .map(|(r, e)| (Arc::new(r), e));
    let raster = entry.as_ref().map(|(r, _)| r.clone());
    let mut c = cache().lock().unwrap_or_else(PoisonError::into_inner);
    c.push((strokes.to_vec(), key, entry));
    let bytes = |c: &Vec<CacheEntry>| -> usize {
        c.iter()
            .map(|(_, _, r)| r.as_ref().map_or(0, |(r, _)| r.bytes()))
            .sum()
    };
    while c.len() > CACHE_ENTRIES || (c.len() > 1 && bytes(&c) > CACHE_BYTES) {
        c.remove(0);
    }
    raster
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: Vec<[f32; 2]>, size: f32) -> BrushStroke {
        BrushStroke {
            size,
            feather: 0.0,
            points,
            ..Default::default()
        }
    }

    #[test]
    fn a_single_dab_is_a_disc_up_to_the_density() {
        let mut s = stroke(vec![[0.5, 0.3]], 0.05);
        s.density = 60.0;
        let r = rasterize(&[s], (1.0, 0.6)).unwrap();
        assert!((r.sample(0.5, 0.3) - 0.6).abs() < 1e-6);
        assert!((r.sample(0.53, 0.3) - 0.6).abs() < 1e-6);
        assert_eq!(r.sample(0.56, 0.3), 0.0);
        assert_eq!(r.sample(0.9, 0.5), 0.0, "範囲の外は 0");
        assert!(r.width < 300 && r.height < 300, "{}×{}", r.width, r.height);
    }

    #[test]
    fn flow_builds_up_and_erase_removes() {
        let mut s = stroke(vec![[0.2, 0.2], [0.4, 0.2]], 0.03);
        s.flow = 50.0;
        let one = rasterize(std::slice::from_ref(&s), (1.0, 1.0)).unwrap();
        assert!(
            (one.sample(0.3, 0.2) - 0.5).abs() < 1e-6,
            "{}",
            one.sample(0.3, 0.2)
        );
        // 同じストロークを 2 回塗ると 0.75（密度までの差の半分ずつ）。
        let two = rasterize(&[s.clone(), s.clone()], (1.0, 1.0)).unwrap();
        assert!((two.sample(0.3, 0.2) - 0.75).abs() < 1e-6);
        // 消しゴム（流量 100・密度 100）で 0 に戻る。描いていない部分は 0 のまま。
        let mut e = stroke(vec![[0.3, 0.2]], 0.02);
        e.erase = true;
        let erased = rasterize(&[s.clone(), e], (1.0, 1.0)).unwrap();
        assert_eq!(erased.sample(0.3, 0.2), 0.0);
        assert!((erased.sample(0.37, 0.2) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn feather_falls_off_smoothly_and_the_raster_is_cached() {
        let mut s = stroke(vec![[0.5, 0.5]], 0.1);
        s.feather = 100.0;
        let strokes = vec![s];
        let r = raster_for(&strokes, (1.0, 1.0)).unwrap();
        let near = r.sample(0.51, 0.5);
        let mid = r.sample(0.55, 0.5);
        let far = r.sample(0.59, 0.5);
        assert!(near > mid && mid > far && far > 0.0, "{near} {mid} {far}");
        assert!((coverage(0.5, 0.0) - 0.5).abs() < 1e-6);
        let again = raster_for(&strokes, (1.0, 1.0)).unwrap();
        assert!(Arc::ptr_eq(&r, &again), "同じストロークは使い回す");
        // G の外のストロークはラスターを作らない。
        assert!(rasterize(&[stroke(vec![[1.8, 1.8]], 0.05)], (1.0, 0.5)).is_none());
    }

    /// 比べるための素朴な塗り方（すべてのセルで、すべての線分の距離の最小を求める）。
    fn brute_force(strokes: &[BrushStroke], g_box: (f32, f32)) -> Option<BrushRaster> {
        let e = Extent::of(strokes, g_box)?;
        let mut values = vec![0.0f32; e.len()];
        for s in strokes {
            if s.points.is_empty() {
                continue;
            }
            let r = s.size.max(1e-6);
            let pairs: Vec<_> = if s.points.len() == 1 {
                vec![(s.points[0], s.points[0])]
            } else {
                s.points.windows(2).map(|p| (p[0], p[1])).collect()
            };
            let segs: Vec<Segment> = pairs.into_iter().map(|(a, b)| Segment::new(a, b)).collect();
            for j in 0..i64::from(e.height) {
                for i in 0..i64::from(e.width) {
                    let (cx, cy) = (center(i, e.ix0), center(j, e.iy0));
                    let d2 = segs
                        .iter()
                        .map(|g| g.dist2(cx, cy))
                        .fold(f32::INFINITY, f32::min);
                    let cov = coverage(d2.sqrt() / r, 1.0 - s.feather / 100.0);
                    if cov <= 0.0 {
                        continue;
                    }
                    let a = s.flow / 100.0 * cov;
                    let m = &mut values[(j * i64::from(e.width) + i) as usize];
                    if s.erase {
                        *m *= 1.0 - s.density / 100.0 * a;
                    } else {
                        *m += (s.density / 100.0 - *m).max(0.0) * a;
                    }
                }
            }
        }
        Some(e.raster(values))
    }

    /// 斜め・折り返し・1 点・消しゴム・ぼかしの違うストローク。
    fn sample_strokes() -> Vec<BrushStroke> {
        let mut a = stroke(
            vec![[0.1, 0.1], [0.13, 0.12], [0.16, 0.16], [0.12, 0.2]],
            0.02,
        );
        a.feather = 60.0;
        a.flow = 70.0;
        let mut b = stroke(vec![[0.15, 0.13]], 0.015);
        b.density = 40.0;
        let mut c = stroke(vec![[0.09, 0.18], [0.2, 0.11]], 0.01);
        c.erase = true;
        c.feather = 100.0;
        c.flow = 50.0;
        vec![a, b, c]
    }

    #[test]
    fn painting_only_near_the_segments_matches_checking_every_segment() {
        let strokes = sample_strokes();
        let fast = rasterize(&strokes, (1.0, 0.75)).unwrap();
        let slow = brute_force(&strokes, (1.0, 0.75)).unwrap();
        assert_eq!(fast, slow, "線分のまわりだけを見ても同じ値");
        assert!(fast.values.iter().any(|&v| v > 0.0));
        // G の縁にかかるストロークも同じ。
        let edge = vec![stroke(vec![[-0.01, 0.3], [0.02, 0.33], [0.0, 0.36]], 0.03)];
        assert_eq!(
            rasterize(&edge, (1.0, 0.75)),
            brute_force(&edge, (1.0, 0.75))
        );
        // 折り返し（180 度）・水平・垂直・同じ点の続き・ぼかし 0。
        let mut zig = stroke(
            vec![
                [0.3, 0.3],
                [0.36, 0.3],
                [0.3, 0.3],
                [0.3, 0.36],
                [0.3, 0.36],
                [0.33, 0.39],
                [0.36, 0.36],
            ],
            0.012,
        );
        zig.flow = 80.0;
        assert_eq!(
            rasterize(&[zig.clone()], (1.0, 0.75)),
            brute_force(&[zig], (1.0, 0.75))
        );
    }

    #[test]
    fn random_scribbles_match_checking_every_segment() {
        // 決まった種の乱数（線形合同法）で、点の間隔・曲がり方・大きさ・ぼかしの違う走り書き。
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..6 {
            let mut strokes = Vec::new();
            for _ in 0..3 {
                let size = 0.004 + 0.02 * next();
                let mut p = [0.2 + 0.1 * next(), 0.2 + 0.1 * next()];
                let mut heading = std::f32::consts::TAU * next();
                let mut points = vec![p];
                for _ in 0..(1 + (next() * 30.0) as usize) {
                    heading += (next() - 0.5) * 3.0;
                    let step = size * (0.05 + next() * 0.5);
                    p = [p[0] + step * heading.cos(), p[1] + step * heading.sin()];
                    points.push(p);
                }
                let mut s = stroke(points, size);
                s.feather = [0.0, 35.0, 100.0][(next() * 3.0) as usize % 3];
                s.flow = 30.0 + 70.0 * next();
                s.density = 50.0 + 50.0 * next();
                s.erase = next() < 0.25;
                strokes.push(s);
            }
            assert_eq!(
                rasterize(&strokes, (0.8, 1.0)),
                brute_force(&strokes, (0.8, 1.0)),
                "{strokes:?}"
            );
        }
    }

    #[test]
    fn appended_strokes_are_painted_on_the_previous_raster() {
        let g_box = (1.0, 0.6);
        let mut strokes = sample_strokes();
        // 先頭の 2 本を作っておき、範囲を広げるストローク・消しゴムを順に足す。
        let first = raster_for(&strokes[..2], g_box).unwrap();
        let (_, be) = extend(None, &strokes[..2], g_box).unwrap();
        let mut far = stroke(vec![[0.5, 0.4], [0.6, 0.45]], 0.04);
        far.feather = 30.0;
        strokes.push(far);
        let grown = extend(Some((2, &first, be)), &strokes, g_box).unwrap().0;
        assert!(grown.width > first.width && grown.height > first.height);
        assert_eq!(
            grown,
            rasterize(&strokes, g_box).unwrap(),
            "塗り足しは初めから塗るのと同じ"
        );
        // raster_for も、先頭の一致する前のラスターから同じ値を作る。
        let via_cache = raster_for(&strokes, g_box).unwrap();
        assert_eq!(*via_cache, grown);
        // 範囲が変わらないストロークを足しても同じ。
        let mut inner = stroke(vec![[0.14, 0.14]], 0.005);
        inner.erase = true;
        strokes.push(inner);
        let again = raster_for(&strokes, g_box).unwrap();
        assert_eq!(*again, rasterize(&strokes, g_box).unwrap());
    }
}
