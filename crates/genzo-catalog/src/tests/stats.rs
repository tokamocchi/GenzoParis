//! 撮影の統計（[`crate::Catalog::shooting_stats`]）のテスト。

use genzo_model::Rating;

use super::{Fixture, video_req};
use crate::{Filter, StatCount};

fn count(label: &str, count: u64) -> StatCount {
    StatCount {
        label: label.to_owned(),
        count,
    }
}

/// asset の撮影情報を書き換える（テストの写真はどれも同じ機材なので）。
fn set_exif(
    f: &Fixture,
    asset: i64,
    camera: Option<&str>,
    lens: Option<&str>,
    iso: Option<i64>,
    aperture: Option<f64>,
    focal: Option<f64>,
) {
    f.cat
        .conn
        .execute(
            "UPDATE asset SET camera = ?2, lens = ?3, iso = ?4, aperture = ?5, focal = ?6 WHERE id = ?1",
            rusqlite::params![asset, camera, lens, iso, aperture, focal],
        )
        .unwrap();
}

/// 写真・動画を asset ごとに 1 回だけ数え、値のないものは「不明」に入れる。条件で絞り込める。
#[test]
fn stats_count_assets_once_and_respect_the_filter() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, Some("2023-05-01 10:00:00"));
    let b = f.photo("B.ARW", 2, Some("2024-01-02 10:00:00"));
    let c = f.photo("C.ARW", 3, Some("2024-06-03 10:00:00"));
    let d = f.photo("D.ARW", 4, None);
    let v = f
        .cat
        .register_file(&video_req(
            f.folder,
            "V.MP4",
            5,
            Some("2024-06-03 11:00:00"),
        ))
        .unwrap();
    set_exif(
        &f,
        a.asset_id.get(),
        Some("ILCE-7M4"),
        Some("FE 24-70mm"),
        Some(100),
        Some(2.8),
        Some(24.0),
    );
    set_exif(
        &f,
        b.asset_id.get(),
        Some("ILCE-7M4"),
        Some("FE 24-70mm"),
        Some(1600),
        Some(2.8),
        Some(70.0),
    );
    set_exif(
        &f,
        c.asset_id.get(),
        Some("ILCE-7C"),
        Some("FE 85mm"),
        Some(6400),
        Some(1.8),
        Some(85.0),
    );
    set_exif(&f, d.asset_id.get(), None, Some(""), Some(0), None, None);
    set_exif(
        &f,
        v.asset_id.get(),
        Some("DJI Pocket"),
        None,
        None,
        None,
        None,
    );
    // 仮想コピーは重ねて数えない。
    f.cat
        .create_virtual_copy(a.master_variant_id, None)
        .unwrap();

    let s = f.cat.shooting_stats(&Filter::default()).unwrap();
    assert_eq!((s.photos, s.videos), (4, 1));
    assert_eq!(
        s.cameras.items,
        vec![
            count("ILCE-7M4", 2),
            count("DJI Pocket", 1),
            count("ILCE-7C", 1)
        ]
    );
    assert_eq!(s.cameras.unknown, 1);
    assert_eq!(
        s.lenses.items,
        vec![count("FE 24-70mm", 2), count("FE 85mm", 1)]
    );
    assert_eq!(s.lenses.unknown, 1, "動画は数えない");
    assert_eq!(
        s.focal_lengths.items,
        vec![
            count("24–34 mm", 1),
            count("50–84 mm", 1),
            count("85–134 mm", 1)
        ]
    );
    assert_eq!(
        s.isos.items,
        vec![
            count("ISO 100 以下", 1),
            count("ISO 801–1600", 1),
            count("ISO 3201–6400", 1)
        ]
    );
    assert_eq!(s.isos.unknown, 1);
    assert_eq!(
        s.apertures.items,
        vec![count("f/1.8", 1), count("f/2.8", 2)]
    );
    assert_eq!(s.years.items, vec![count("2023", 1), count("2024", 3)]);
    assert_eq!(s.years.unknown, 1);

    // 条件で絞り込む（評価 3 以上）。
    f.cat
        .set_rating(&[c.master_variant_id], Rating::new(4).unwrap())
        .unwrap();
    let s = f
        .cat
        .shooting_stats(&Filter {
            rating_min: Some(Rating::new(3).unwrap()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!((s.photos, s.videos), (1, 0));
    assert_eq!(s.cameras.items, vec![count("ILCE-7C", 1)]);
    assert_eq!(s.years.items, vec![count("2024", 1)]);
    // 何も合わなければ空。
    let s = f
        .cat
        .shooting_stats(&Filter {
            rating_min: Some(Rating::new(5).unwrap()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(s, crate::ShootingStats::default());
}
