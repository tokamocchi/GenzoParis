//! 撮影の統計（カタログの統計）: 条件に合う写真・動画を、カメラ・レンズ・焦点距離・ISO・絞り・撮影年ごとに数える
//! （[`Catalog::shooting_stats`]）。
//!
//! - **数えるのは asset（写真・動画 1 つ）**で、仮想コピー（variant）は重ねて数えない（[`crate::Facets`] と同じ）。
//!   条件（[`Filter`]）は検索と同じで、条件に合う variant が 1 つでもある asset を数える。
//! - カメラ・撮影年は写真と動画、レンズ・焦点距離・ISO・絞りは写真だけを数える（動画にはその情報がないことが多い）。
//! - 値がない（NULL・空・0 以下）ものは、項目ごとの「不明」の数に入れる。
//! - 焦点距離・ISO は決まった区切り（[`FOCAL_EDGES_MM`]・[`ISO_EDGES`]）でまとめる。絞りは小数第 1 位に丸めた
//!   値ごと。撮影年は、補正後の UTC の年（タイムゾーンは見ない。年の変わり目の数時間の写真は、現地の年と
//!   ずれることがある）。
//! - 条件に合う asset の行を 1 回読んで、Rust で数える（50 万件でも 1 回の走査）。

use std::collections::{BTreeMap, HashMap};

use rusqlite::params_from_iter;

use crate::catalog::Catalog;
use crate::error::Result;
use crate::search::{Filter, Sort, build_search_sql};

/// 焦点距離の区切り（mm。実焦点距離。`[下限, 次の下限)` でまとめる。最後は「以上」）。
pub const FOCAL_EDGES_MM: [f64; 9] = [0.0, 16.0, 24.0, 35.0, 50.0, 85.0, 135.0, 200.0, 400.0];

/// ISO の区切り（`(前の値, この値]` でまとめる。最後の値を超えるものは「より上」）。
pub const ISO_EDGES: [i64; 9] = [100, 200, 400, 800, 1600, 3200, 6400, 12800, 25600];

/// 1 つの値（まとまり）と、その数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatCount {
    /// 値の表示（カメラ・レンズの名前、`24–34 mm`、`ISO 101–200`、`f/2.8`、`2024` など）。
    pub label: String,
    /// asset の数。
    pub count: u64,
}

/// 1 つの項目の内訳。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatBreakdown {
    /// 値ごとの数（数が 0 のまとまりは入れない）。
    pub items: Vec<StatCount>,
    /// 値がなくて数えられなかった数。
    pub unknown: u64,
}

/// 撮影の統計（[`Catalog::shooting_stats`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShootingStats {
    /// 写真の数（asset）。
    pub photos: u64,
    /// 動画の数（asset）。
    pub videos: u64,
    /// カメラ（数の多い順。写真と動画）。
    pub cameras: StatBreakdown,
    /// レンズ（数の多い順。写真だけ）。
    pub lenses: StatBreakdown,
    /// 焦点距離（区切りの順。写真だけ）。
    pub focal_lengths: StatBreakdown,
    /// ISO（区切りの順。写真だけ）。
    pub isos: StatBreakdown,
    /// 絞り（小さい順。写真だけ）。
    pub apertures: StatBreakdown,
    /// 撮影年（古い順。写真と動画）。
    pub years: StatBreakdown,
}

/// 焦点距離のまとまりの番号（[`FOCAL_EDGES_MM`] の添字）。
fn focal_bucket(mm: f64) -> usize {
    FOCAL_EDGES_MM
        .iter()
        .rposition(|edge| mm >= *edge)
        .unwrap_or(0)
}

/// 焦点距離のまとまりの表示。
fn focal_label(bucket: usize) -> String {
    let lo = FOCAL_EDGES_MM[bucket];
    match FOCAL_EDGES_MM.get(bucket + 1) {
        Some(next) if bucket == 0 => format!("{} mm 未満", *next as i64),
        Some(next) => format!("{}–{} mm", lo as i64, *next as i64 - 1),
        None => format!("{} mm 以上", lo as i64),
    }
}

/// ISO のまとまりの番号（[`ISO_EDGES`] の添字。最後の値を超えるものは `ISO_EDGES.len()`）。
fn iso_bucket(iso: i64) -> usize {
    ISO_EDGES
        .iter()
        .position(|edge| iso <= *edge)
        .unwrap_or(ISO_EDGES.len())
}

/// ISO のまとまりの表示。
fn iso_label(bucket: usize) -> String {
    match (bucket, ISO_EDGES.get(bucket)) {
        (0, Some(hi)) => format!("ISO {hi} 以下"),
        (_, Some(hi)) => format!("ISO {}–{hi}", ISO_EDGES[bucket - 1] + 1),
        (_, None) => format!("ISO {} より上", ISO_EDGES[ISO_EDGES.len() - 1]),
    }
}

/// 名前ごとの数を、数の多い順（同じ数なら名前の順）に並べる。
fn by_count(map: HashMap<String, u64>) -> Vec<StatCount> {
    let mut items: Vec<StatCount> = map
        .into_iter()
        .map(|(label, count)| StatCount { label, count })
        .collect();
    items.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
    items
}

impl Catalog {
    /// 条件に合う写真・動画の撮影の統計（モジュールの doc）。
    pub fn shooting_stats(&self, filter: &Filter) -> Result<ShootingStats> {
        let q = build_search_sql(&self.conn, filter, &Sort::default())?;
        let sql = format!(
            "SELECT a.kind, a.camera, a.lens, a.iso, a.aperture, a.focal, substr(a.captured_at_utc, 1, 4)
             FROM asset a
             WHERE a.id IN (SELECT sv.asset_id FROM variant sv WHERE sv.id IN ({}))",
            q.sql
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(q.values.iter()))?;

        let mut out = ShootingStats::default();
        let mut cameras: HashMap<String, u64> = HashMap::new();
        let mut lenses: HashMap<String, u64> = HashMap::new();
        let mut focals = [0u64; FOCAL_EDGES_MM.len()];
        let mut isos = [0u64; ISO_EDGES.len() + 1];
        // 絞りは 10 倍して丸めた整数をキーにする（f/2.8 → 28）。
        let mut apertures: BTreeMap<i64, u64> = BTreeMap::new();
        let mut years: BTreeMap<String, u64> = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let kind: String = row.get(0)?;
            let camera: Option<String> = row.get(1)?;
            let lens: Option<String> = row.get(2)?;
            let iso: Option<i64> = row.get(3)?;
            let aperture: Option<f64> = row.get(4)?;
            let focal: Option<f64> = row.get(5)?;
            let year: Option<String> = row.get(6)?;
            let is_photo = kind != "video";
            if is_photo {
                out.photos += 1;
            } else {
                out.videos += 1;
            }
            match camera.filter(|c| !c.is_empty()) {
                Some(c) => *cameras.entry(c).or_default() += 1,
                None => out.cameras.unknown += 1,
            }
            match year.filter(|y| y.len() == 4) {
                Some(y) => *years.entry(y).or_default() += 1,
                None => out.years.unknown += 1,
            }
            if !is_photo {
                continue;
            }
            match lens.filter(|l| !l.is_empty()) {
                Some(l) => *lenses.entry(l).or_default() += 1,
                None => out.lenses.unknown += 1,
            }
            match focal.filter(|f| f.is_finite() && *f > 0.0) {
                Some(f) => focals[focal_bucket(f)] += 1,
                None => out.focal_lengths.unknown += 1,
            }
            match iso.filter(|i| *i > 0) {
                Some(i) => isos[iso_bucket(i)] += 1,
                None => out.isos.unknown += 1,
            }
            match aperture.filter(|a| a.is_finite() && *a > 0.0) {
                Some(a) => *apertures.entry((a * 10.0).round() as i64).or_default() += 1,
                None => out.apertures.unknown += 1,
            }
        }
        out.cameras.items = by_count(cameras);
        out.lenses.items = by_count(lenses);
        out.focal_lengths.items = focals
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(i, n)| StatCount {
                label: focal_label(i),
                count: *n,
            })
            .collect();
        out.isos.items = isos
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(i, n)| StatCount {
                label: iso_label(i),
                count: *n,
            })
            .collect();
        out.apertures.items = apertures
            .into_iter()
            .map(|(k, count)| StatCount {
                label: format!("f/{}.{}", k / 10, k % 10),
                count,
            })
            .collect();
        out.years.items = years
            .into_iter()
            .map(|(label, count)| StatCount { label, count })
            .collect();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focal_lengths_are_grouped_by_fixed_edges() {
        assert_eq!(focal_label(focal_bucket(12.0)), "16 mm 未満");
        assert_eq!(focal_label(focal_bucket(16.0)), "16–23 mm");
        assert_eq!(focal_label(focal_bucket(23.9)), "16–23 mm");
        assert_eq!(focal_label(focal_bucket(24.0)), "24–34 mm");
        assert_eq!(focal_label(focal_bucket(70.0)), "50–84 mm");
        assert_eq!(focal_label(focal_bucket(399.0)), "200–399 mm");
        assert_eq!(focal_label(focal_bucket(600.0)), "400 mm 以上");
    }

    #[test]
    fn isos_are_grouped_by_doubling_edges() {
        assert_eq!(iso_label(iso_bucket(50)), "ISO 100 以下");
        assert_eq!(iso_label(iso_bucket(100)), "ISO 100 以下");
        assert_eq!(iso_label(iso_bucket(101)), "ISO 101–200");
        assert_eq!(iso_label(iso_bucket(3200)), "ISO 1601–3200");
        assert_eq!(iso_label(iso_bucket(25600)), "ISO 12801–25600");
        assert_eq!(iso_label(iso_bucket(51200)), "ISO 25600 より上");
    }

    #[test]
    fn names_sort_by_count_then_name() {
        let map: HashMap<String, u64> = [("B", 2), ("A", 2), ("C", 5)]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        let labels: Vec<String> = by_count(map).into_iter().map(|c| c.label).collect();
        assert_eq!(labels, ["C", "A", "B"]);
    }
}
