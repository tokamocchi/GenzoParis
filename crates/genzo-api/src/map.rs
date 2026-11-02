//! 地図の表示（LIB-18。docs/04_architecture.md の 3.17 節）。
//!
//! - 表示中の検索結果（世代）の写真を、使う位置（付けた位置があればそれ、なければカメラの位置。LIB-21）で
//!   地図に並べるために、地図のズームに合わせたまとまり（クラスター）にして返す（[`Core::map_clusters`]）。
//!   写真ごとの位置は画面に送らない（50 万件でも、送る量は地図に出す印の数に比例する）。
//! - **まとめ方**: 地図のタイルと同じ Web メルカトルの、ズーム `zoom` の画素の座標で、[`MAP_CELL_PX`] 四方の
//!   格子に分け、同じ格子に入る写真を 1 つにまとめる。印の位置はまとまりの写真の位置の平均、範囲は写真の位置を
//!   囲む矩形（押したときに寄せる）、代表は検索結果の並びで最初の写真。格子は経度 180 度の線をまたがない。
//!   緯度は Web メルカトルで描ける ±[`MAX_MERCATOR_LAT`] 度に収めてから格子を決める（極に近い写真も端の格子に
//!   入れる。位置の値はそのまま）。
//! - 範囲 `bounds` の中の写真だけをまとめる（画面は表示している範囲より少し広い範囲を渡す）。まとまりが
//!   [`MAX_MAP_CLUSTERS`] を超えるときは、写真の多いものから残す（`truncated`）。
//! - 写真の位置は、世代ごとに 1 回だけ読んで覚えておく（`Inner::map`）。位置の変更・取り込み・ファイルの状態の
//!   変更（取り込み・内容の確認・再リンクでのファイルの読み直しで、カメラの位置が変わる）を知らせたら
//!   （[`crate::events::Event::CatalogChanged`]）、次の問い合わせで読み直す。読むのは、タイムラインと同じく
//!   読み取り専用の接続（50 万件で 1 秒近くかかることがあるため、カタログを変える操作を待たせない）。
//! - ログに座標を書かない（決定事項ログ No.75 の方針）。

use std::collections::HashMap;
use std::sync::Arc;

use genzo_model::{GeoBounds, GpsCoord, VariantId};

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::types::{MapCluster, MapClusters};

/// まとめる格子の大きさ（地図の画素。**仮置き**: 64。印の円の大きさ（画面の `geo.css`）より少し大きい）。
pub const MAP_CELL_PX: f64 = 64.0;

/// 1 回に返すまとまりの数の上限（**仮置き**: 2,000。大きな画面で格子を埋めても足りる数）。
pub const MAX_MAP_CLUSTERS: usize = 2_000;

/// まとめるズームの上限（計算の上限。地図のタイルの最大のズーム（OSM の標準は 19）より大きくてよい）。
pub const MAX_MAP_ZOOM: u8 = 24;

/// Web メルカトルで描ける緯度の上限（度。`atan(sinh(π))`）。
pub const MAX_MERCATOR_LAT: f64 = 85.051_128_779_806_59;

/// タイル 1 枚の画素の数（Web メルカトルの、ズーム 0 の世界の幅）。
const TILE_PX: f64 = 256.0;

/// 位置のある 1 枚（検索結果の中の位置と、使う位置）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct MapPoint {
    index: usize,
    gps: GpsCoord,
}

/// 世代ごとに読んだ写真の位置（`Inner::map`）。
pub(crate) struct MapCache {
    generation: u64,
    /// 読む前に見た、写真の位置が変わりうる変更の回数（`EventHub::location_changes`）。
    changes: u64,
    points: Arc<MapPoints>,
}

/// 検索結果の写真の位置。
#[derive(Debug, Default)]
struct MapPoints {
    /// 位置のある写真（検索結果の順）。
    points: Vec<MapPoint>,
    /// 位置のない写真の数。
    unlocated: u64,
    /// 位置のある写真を囲む範囲。
    extent: Option<GeoBounds>,
}

impl MapPoints {
    /// 検索結果の順の位置（位置のないものは `None`）から作る。
    fn new(locations: &[Option<GpsCoord>]) -> Self {
        let mut out = Self::default();
        let mut ext = Extent::default();
        for (index, g) in locations.iter().enumerate() {
            match g {
                Some(gps) => {
                    ext.add(*gps);
                    out.points.push(MapPoint { index, gps: *gps });
                }
                None => out.unlocated += 1,
            }
        }
        out.extent = ext.bounds();
        out
    }
}

/// 位置を囲む矩形（緯度・経度の最小と最大）。
#[derive(Debug, Clone, Copy, Default)]
struct Extent(Option<(f64, f64, f64, f64)>);

impl Extent {
    fn add(&mut self, g: GpsCoord) {
        self.0 = Some(match self.0 {
            None => (g.lat, g.lon, g.lat, g.lon),
            Some((s, w, n, e)) => (s.min(g.lat), w.min(g.lon), n.max(g.lat), e.max(g.lon)),
        });
    }

    fn bounds(self) -> Option<GeoBounds> {
        self.0.and_then(|(s, w, n, e)| GeoBounds::new(s, w, n, e))
    }
}

/// 位置の、ズーム `zoom` の Web メルカトルの画素の座標（x は西の端の 180 度の線から東へ、y は北の端から南へ）。
fn world_px(g: GpsCoord, zoom: u8) -> (f64, f64) {
    let size = TILE_PX * 2f64.powi(i32::from(zoom));
    let x = (g.lon + 180.0) / 360.0 * size;
    let lat = g
        .lat
        .clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT)
        .to_radians();
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * size;
    // 東の端（経度 180）・南の端は、最後の格子に入れる。
    (x.clamp(0.0, size - 1e-9), y.clamp(0.0, size - 1e-9))
}

/// 作っている途中のまとまり。
struct Building {
    /// 代表（最初の写真）。
    first: MapPoint,
    count: u64,
    lat_sum: f64,
    lon_sum: f64,
    extent: Extent,
}

/// 範囲の中の写真をズームに合わせてまとめる（モジュールの doc）。まとまり（代表の検索結果の中の位置の順）・
/// 範囲の中の写真の数・上限で切ったかを返す。
fn cluster(points: &[MapPoint], bounds: GeoBounds, zoom: u8) -> (Vec<Building>, u64, bool) {
    let mut cells: HashMap<(u64, u64), Building> = HashMap::new();
    let mut in_bounds = 0u64;
    for p in points {
        if !bounds.contains(p.gps) {
            continue;
        }
        in_bounds += 1;
        let (x, y) = world_px(p.gps, zoom);
        let key = ((x / MAP_CELL_PX) as u64, (y / MAP_CELL_PX) as u64);
        let b = cells.entry(key).or_insert_with(|| Building {
            first: *p,
            count: 0,
            lat_sum: 0.0,
            lon_sum: 0.0,
            extent: Extent::default(),
        });
        b.count += 1;
        b.lat_sum += p.gps.lat;
        b.lon_sum += p.gps.lon;
        b.extent.add(p.gps);
    }
    let mut out: Vec<Building> = cells.into_values().collect();
    let truncated = out.len() > MAX_MAP_CLUSTERS;
    if truncated {
        // 写真の多いものから残す（同じ数なら検索結果の並びで先のもの）。
        out.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then(a.first.index.cmp(&b.first.index))
        });
        out.truncate(MAX_MAP_CLUSTERS);
    }
    out.sort_by_key(|b| b.first.index);
    (out, in_bounds, truncated)
}

/// 表示中の検索結果（世代 `generation`）の写真の位置（覚えたものがなければ読む。モジュールの doc）。
fn points(
    inner: &Inner,
    generation: u64,
) -> Result<(Arc<Vec<VariantId>>, Arc<MapPoints>), ApiError> {
    // 変更の回数を、検索結果と位置を読む前に見る（読んだ内容はこの回数の時点より新しい）。
    let changes = inner.events.location_changes();
    let (ids, _) = crate::search::current_ids(inner, generation)?;
    if let Some(c) = inner.map.lock().as_ref()
        && c.generation == generation
        && c.changes == changes
    {
        return Ok((ids, Arc::clone(&c.points)));
    }
    let locations = match genzo_catalog::Catalog::open_read_only(&inner.config.catalog_path) {
        Ok(reader) => {
            let l = reader.map_locations(&ids);
            drop(reader);
            l?
        }
        Err(e) => {
            tracing::debug!(error = %e, "地図の位置の読み取りに本体の接続を使います");
            inner.with_catalog(|c| c.map_locations(&ids))?
        }
    };
    let points = Arc::new(MapPoints::new(&locations));
    *inner.map.lock() = Some(MapCache {
        generation,
        changes,
        points: Arc::clone(&points),
    });
    Ok((ids, points))
}

impl Core {
    /// 表示中の検索結果（世代 `generation`）の写真を、地図の範囲 `bounds`・ズーム `zoom` に合わせてまとめる
    /// （モジュールの doc）。古い世代なら [`ApiError::Stale`]、範囲・ズームが不正なら種類 `invalid_argument`。
    pub fn map_clusters(
        &self,
        generation: u64,
        bounds: GeoBounds,
        zoom: u8,
    ) -> Result<MapClusters, ApiError> {
        let inner = &self.inner;
        inner.check_open()?;
        if !bounds.is_valid() {
            return Err(ApiError::InvalidArgument(
                "地図の範囲が不正です（緯度は -90〜90 で南の端が北の端以下、経度は -180〜180 の値を指定してください）"
                    .to_owned(),
            ));
        }
        if zoom > MAX_MAP_ZOOM {
            return Err(ApiError::InvalidArgument(format!(
                "地図のズームは 0〜{MAX_MAP_ZOOM} で指定してください（{zoom}）"
            )));
        }
        let (ids, points) = points(inner, generation)?;
        let (built, in_bounds, truncated) = cluster(&points.points, bounds, zoom);
        let clusters = built
            .into_iter()
            .filter_map(|b| {
                let n = b.count as f64;
                Some(MapCluster {
                    count: b.count,
                    lat: b.lat_sum / n,
                    lon: b.lon_sum / n,
                    bounds: b.extent.bounds()?,
                    variant_id: *ids.get(b.first.index)?,
                    index: b.first.index as u64,
                })
            })
            .collect();
        Ok(MapClusters {
            generation,
            zoom,
            clusters,
            in_bounds,
            located: points.points.len() as u64,
            unlocated: points.unlocated,
            extent: points.extent,
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(lat: f64, lon: f64) -> GpsCoord {
        GpsCoord::new(lat, lon).unwrap()
    }

    fn pts(list: &[Option<GpsCoord>]) -> MapPoints {
        MapPoints::new(list)
    }

    const WORLD: GeoBounds = GeoBounds {
        south: -90.0,
        west: -180.0,
        north: 90.0,
        east: 180.0,
    };

    #[test]
    fn world_pixels_follow_web_mercator() {
        // ズーム 0 の世界は 256 画素。経度 0・緯度 0 は中央。
        let (x, y) = world_px(g(0.0, 0.0), 0);
        assert!((x - 128.0).abs() < 1e-9 && (y - 128.0).abs() < 1e-9);
        // 西の端は 0、東の端・南の端・北の端は世界の中に収める。
        assert_eq!(world_px(g(0.0, -180.0), 1).0, 0.0);
        let (x, y) = world_px(g(-90.0, 180.0), 1);
        assert!(x < 512.0 && x > 511.9 && y < 512.0 && y > 511.9);
        assert!(world_px(g(90.0, 0.0), 3).1 < 1e-6);
        // 東京駅（35.681236, 139.767125）はズーム 15 でタイル (29105, 12903) の中（OSM のタイルの番号の式）。
        let (x, y) = world_px(g(35.681236, 139.767125), 15);
        assert_eq!(((x / TILE_PX) as u64, (y / TILE_PX) as u64), (29105, 12903));
    }

    #[test]
    fn nearby_photos_are_clustered_by_zoom() {
        // 東京駅のあたりの 3 枚（数百 m 以内）、京都の 1 枚、位置なしの 2 枚。
        let p = pts(&[
            None,
            Some(g(35.6812, 139.7671)),
            Some(g(34.9949, 135.7850)),
            Some(g(35.6840, 139.7650)),
            None,
            Some(g(35.6790, 139.7700)),
        ]);
        assert_eq!(p.unlocated, 2);
        assert_eq!(p.points.len(), 4);
        let ext = p.extent.unwrap();
        assert_eq!(
            (ext.south, ext.west, ext.north, ext.east),
            (34.9949, 135.7850, 35.6840, 139.7700)
        );
        // ズーム 5: 東京と京都は別のまとまり（約 360 km 離れている）。東京の 3 枚は 1 つ。
        let (c, n, truncated) = cluster(&p.points, WORLD, 5);
        assert_eq!((n, truncated), (4, false));
        let summary: Vec<(usize, u64)> = c.iter().map(|b| (b.first.index, b.count)).collect();
        assert_eq!(summary, vec![(1, 3), (2, 1)]);
        let tokyo = &c[0];
        assert!((tokyo.lat_sum / 3.0 - 35.6814).abs() < 1e-4);
        let b = tokyo.extent.bounds().unwrap();
        assert_eq!(
            (b.south, b.west, b.north, b.east),
            (35.6790, 139.7650, 35.6840, 139.7700)
        );
        // ズーム 18: 東京の 3 枚も分かれる（格子は約 30 m 四方）。
        let (c, _, _) = cluster(&p.points, WORLD, 18);
        assert_eq!(c.len(), 4);
        assert!(c.windows(2).all(|w| w[0].first.index < w[1].first.index));
        // 範囲の外は数えない（京都の付近だけ）。
        let kyoto = GeoBounds::new(34.0, 135.0, 36.0, 136.5).unwrap();
        let (c, n, _) = cluster(&p.points, kyoto, 5);
        assert_eq!((c.len(), n), (1, 1));
        assert_eq!(c[0].first.index, 2);
    }

    #[test]
    fn clusters_do_not_cross_the_antimeridian_but_bounds_can() {
        // 経度 179.9 と -179.9 は近いが、格子は 180 度の線をまたがないので別のまとまり。
        let p = pts(&[Some(g(-17.0, 179.9)), Some(g(-17.0, -179.9))]);
        let (c, n, _) = cluster(&p.points, WORLD, 0);
        assert_eq!((c.len(), n), (2, 2));
        // 180 度の線をまたぐ範囲には、両方が入る。
        let pacific = GeoBounds::new(-20.0, 170.0, -10.0, -170.0).unwrap();
        let (_, n, _) = cluster(&p.points, pacific, 3);
        assert_eq!(n, 2);
        // 極に近い写真（緯度 89）も、北の端の格子に入れる。
        let p = pts(&[Some(g(89.0, 10.0)), Some(g(-89.5, 10.0))]);
        let (c, n, _) = cluster(&p.points, WORLD, 2);
        assert_eq!((c.len(), n), (2, 2));
    }

    #[test]
    fn too_many_clusters_keep_the_largest() {
        // 格子ごとに 1 枚（経度 0.5 度おき。ズーム 10 なら別々の格子）を上限より多く、最後の格子にだけ 3 枚。
        let mut list: Vec<Option<GpsCoord>> = (0..MAX_MAP_CLUSTERS + 10)
            .map(|i| Some(g(10.0, -170.0 + i as f64 * 0.15)))
            .collect();
        let last = list.last().copied().flatten().unwrap();
        list.push(Some(last));
        list.push(Some(last));
        let p = pts(&list);
        let (c, n, truncated) = cluster(&p.points, WORLD, 10);
        assert!(truncated);
        assert_eq!(n, list.len() as u64);
        assert_eq!(c.len(), MAX_MAP_CLUSTERS);
        // 3 枚のまとまりは残り、ほかは並びの先のものから残る。
        assert!(c.iter().any(|b| b.count == 3));
        assert_eq!(c[0].first.index, 0);
        assert!(c.windows(2).all(|w| w[0].first.index < w[1].first.index));
    }
}
