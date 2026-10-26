//! AI マスク（DEV-20）のラスターを、ブラシと同じ G 座標の格子（[`BrushRaster`]）に写す。
//!
//! 設定の `MaskShape::Ai` は、ラスター（[`AiMaskRaster`]。8 ビットの重み、`width × height`）が G 座標の `bounds`
//! （左・上・右・下）を覆うことを表す。ここでは、
//!
//! - セルの大きさを横方向で決め（`cell = (右 − 左) / width`。ブラシと同じ正方のセル）、
//! - 縦方向はセルの数を `round((下 − 上) / cell)` にして、必要なら行を線形に補間して詰め直し（ふつうは
//!   ラスターの縦横比が `bounds` と同じで、行はそのまま）、
//! - 値を 0〜1 にする。
//!
//! 写したものは [`BrushRaster`] なので、引き方（双線形。範囲の外は 0）・GPU への受け渡し（genzo-gpu の `local.wgsl`
//! の種類 2）はブラシと同じ。同じラスター・同じ範囲なら使い回す（[`raster_for`]）。

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use super::brush::BrushRaster;
use crate::profile::AiMaskRaster;

/// 使い回す件数の上限。
pub const CACHE_ENTRIES: usize = 8;

/// [`AiMaskRaster`] を、G 座標の `bounds`（左・上・右・下）を覆う [`BrushRaster`] に写す（モジュールの doc）。
/// 範囲が空（右 ≤ 左・下 ≤ 上・有限でない）、または写した格子が上限（1 辺 genzo-model の `MAX_AI_MASK_SIDE`・
/// `MAX_AI_MASK_PIXELS` 画素。範囲の縦横比がラスターと極端に違う設定）を超えるなら `None`（重み 0。確保しない）。
pub fn to_brush_raster(raster: &AiMaskRaster, bounds: [f32; 4]) -> Option<BrushRaster> {
    let (w, h) = (raster.width as usize, raster.height as usize);
    if raster.values.len() != w * h {
        return None;
    }
    let rows = grid_rows(raster.width, bounds)?;
    let [left, top, right, _] = bounds;
    let cell = f64::from(right - left) / w as f64;
    let inv_cell = (1.0 / cell) as f32;
    let scale = 1.0 / 255.0;
    let mut values = Vec::with_capacity(w * rows);
    if rows == h {
        values.extend(raster.values.iter().map(|&v| f32::from(v) * scale));
    } else {
        // 行を線形に補間して詰め直す（元の行 y の中心は y + 0.5）。
        for j in 0..rows {
            let fy = ((j as f64 + 0.5) / rows as f64 * h as f64 - 0.5).max(0.0);
            let y0 = (fy.floor() as usize).min(h - 1);
            let y1 = (y0 + 1).min(h - 1);
            let t = (fy - y0 as f64) as f32;
            let a = &raster.values[y0 * w..y0 * w + w];
            let b = &raster.values[y1 * w..y1 * w + w];
            values.extend(a.iter().zip(b).map(|(&a, &b)| {
                let (a, b) = (f32::from(a), f32::from(b));
                (a + (b - a) * t) * scale
            }));
        }
    }
    Some(BrushRaster {
        origin: [left, top],
        inv_cell,
        width: raster.width,
        height: rows as u32,
        values,
    })
}

/// 幅 `width` のラスターを G 座標の `bounds` に写したときの行の数（[`to_brush_raster`]）。範囲が空（右 ≤ 左・下 ≤ 上・
/// 有限でない）、幅が 0、または格子が上限（1 辺 genzo-model の `MAX_AI_MASK_SIDE`・`MAX_AI_MASK_PIXELS` 画素）を超える
/// （読み込んだ設定の範囲が極端に細い、例: 幅 1e−6・高さ 1）なら `None`。描画の前に「使えない」を知らせるためにも使う
/// （[`crate::ProfileResources::resolve`]）。
pub fn grid_rows(width: u32, bounds: [f32; 4]) -> Option<usize> {
    let [left, top, right, bottom] = bounds;
    if width == 0 || !bounds.iter().all(|v| v.is_finite()) || right <= left || bottom <= top {
        return None;
    }
    let cell = f64::from(right - left) / f64::from(width);
    let rows = (f64::from(bottom - top) / cell).round().max(1.0);
    if !rows.is_finite()
        || rows > f64::from(genzo_model::MAX_AI_MASK_SIDE)
        || u64::from(width) * (rows as u64) > genzo_model::MAX_AI_MASK_PIXELS
    {
        return None;
    }
    Some(rows as usize)
}

type Key = (String, [u32; 4]);
type Entry = (Key, Option<Arc<BrushRaster>>);

fn cache() -> &'static Mutex<Vec<Entry>> {
    static CACHE: OnceLock<Mutex<Vec<Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

/// 使い回すものをすべて捨てる（計測のコールドの回など）。
pub fn clear_cache() {
    cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// [`to_brush_raster`] の結果を使い回す（キーは内容の SHA-256 `sha256` と `bounds`）。
pub fn raster_for(
    sha256: &str,
    raster: &AiMaskRaster,
    bounds: [f32; 4],
) -> Option<Arc<BrushRaster>> {
    let key: Key = (sha256.to_owned(), bounds.map(f32::to_bits));
    {
        let mut c = cache().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = c.iter().position(|(k, _)| *k == key) {
            let e = c.remove(i);
            let r = e.1.clone();
            c.push(e);
            return r;
        }
    }
    let r = to_brush_raster(raster, bounds).map(Arc::new);
    let mut c = cache().lock().unwrap_or_else(PoisonError::into_inner);
    c.push((key, r.clone()));
    while c.len() > CACHE_ENTRIES {
        c.remove(0);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> AiMaskRaster {
        let values = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| f(x, y));
        AiMaskRaster::new(w, h, values.collect()).unwrap()
    }

    #[test]
    fn maps_the_bounds_to_square_cells() {
        // 4 × 2 のラスターが G の [0, 0, 1, 0.5] を覆う: セルは 0.25。
        let r = raster(4, 2, |x, _| if x < 2 { 255 } else { 0 });
        let b = to_brush_raster(&r, [0.0, 0.0, 1.0, 0.5]).unwrap();
        assert_eq!((b.width, b.height), (4, 2));
        assert_eq!(b.origin, [0.0, 0.0]);
        assert!((b.inv_cell - 4.0).abs() < 1e-6);
        assert_eq!(b.values[0], 1.0);
        assert_eq!(b.values[3], 0.0);
        // セルの中心では値そのもの、境目では中間。
        assert!((b.sample(0.125, 0.125) - 1.0).abs() < 1e-6);
        assert!((b.sample(0.875, 0.375)).abs() < 1e-6);
        assert!((b.sample(0.5, 0.125) - 0.5).abs() < 1e-6);
        // 範囲の外は 0。
        assert_eq!(b.sample(-0.5, 0.1), 0.0);
        assert_eq!(b.sample(0.5, 1.0), 0.0);
    }

    #[test]
    fn resamples_rows_when_the_aspect_differs() {
        // 2 × 2 のラスターで、縦が横の 2 倍の範囲: 行は 4 になり、線形に補間される。
        let r = raster(2, 2, |_, y| if y == 0 { 0 } else { 200 });
        let b = to_brush_raster(&r, [0.0, 0.0, 1.0, 2.0]).unwrap();
        assert_eq!((b.width, b.height), (2, 4));
        let col: Vec<f32> = (0..4).map(|j| b.values[j * 2]).collect();
        assert_eq!(col[0], 0.0);
        assert!((col[3] - 200.0 / 255.0).abs() < 1e-6);
        assert!(col[1] > col[0] && col[2] > col[1] && col[3] > col[2]);
    }

    /// 範囲の縦横比がラスターと極端に違う設定（Codex のレビューの指摘）は、巨大な格子を確保せずに断る。
    #[test]
    fn rejects_bounds_that_would_need_a_huge_grid() {
        let r = raster(640, 480, |_, _| 255);
        assert!(to_brush_raster(&r, [0.0, 0.0, 0.000_001, 1.0]).is_none());
        // 上限ちょうどまでは写せる（1 辺 4096 行）。
        let narrow = raster(1, 4, |_, _| 255);
        let b = to_brush_raster(&narrow, [0.0, 0.0, 1.0 / 1024.0, 4.0]).unwrap();
        assert_eq!((b.width, b.height), (1, 4096));
        assert!(to_brush_raster(&narrow, [0.0, 0.0, 1.0 / 1024.0, 4.01]).is_none());
        assert_eq!(grid_rows(1, [0.0, 0.0, 1.0 / 1024.0, 4.0]), Some(4096));
        assert_eq!(grid_rows(640, [0.0, 0.0, 0.000_001, 1.0]), None);
        assert_eq!(grid_rows(0, [0.0, 0.0, 1.0, 1.0]), None);
    }

    #[test]
    fn rejects_empty_bounds_and_reuses_rasters() {
        let r = raster(2, 2, |_, _| 128);
        assert!(to_brush_raster(&r, [0.0, 0.0, 0.0, 1.0]).is_none());
        assert!(to_brush_raster(&r, [0.0, f32::NAN, 1.0, 1.0]).is_none());
        clear_cache();
        let a = raster_for("x", &r, [0.0, 0.0, 1.0, 1.0]).unwrap();
        let b = raster_for("x", &r, [0.0, 0.0, 1.0, 1.0]).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let c = raster_for("x", &r, [0.0, 0.0, 1.0, 0.5]).unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
        assert!(raster_for("y", &r, [0.0, 0.0, 1.0, 0.0]).is_none());
    }
}
