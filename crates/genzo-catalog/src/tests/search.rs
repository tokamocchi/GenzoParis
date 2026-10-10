//! 検索（LIB-07・LIB-08）のテスト: フィルター、並べ替え（同じ撮影日時・NULL）、表示範囲の詳細。

use chrono::{TimeZone, Utc};
use genzo_model::{AssetKind, ColorLabel, FileStatus, Flag, Rating, VariantId};

use super::{Fixture, photo_req, video_req};
use crate::{Filter, FolderFilter, Sort, SortDirection, SortKey};

/// 並べ替えのテスト用のカタログ。
///
/// | 名前 | 撮影日時（JST） | 評価 |
/// |---|---|---|
/// | C.ARW | 2024-05-02 10:00 | 3 |
/// | A.ARW | 2024-05-01 10:00 | 5 |
/// | B.ARW | 2024-05-01 10:00（A と同じ） | 1 |
/// | N1.ARW | なし | 0 |
/// | D.ARW | 2024-04-30 10:00 | 3 |
/// | N2.ARW | なし | 2 |
struct SortFixture {
    f: Fixture,
    c: VariantId,
    a: VariantId,
    b: VariantId,
    n1: VariantId,
    d: VariantId,
    n2: VariantId,
}

fn sort_fixture() -> SortFixture {
    let mut f = Fixture::new();
    let mut add = |name: &str, n: u64, when: Option<&str>, rating: u8| {
        let o = f.photo(name, n, when);
        f.cat
            .set_rating(&[o.master_variant_id], Rating::new(rating).unwrap())
            .unwrap();
        o.master_variant_id
    };
    let c = add("C.ARW", 1, Some("2024:05:02 10:00:00"), 3);
    let a = add("A.ARW", 2, Some("2024:05:01 10:00:00"), 5);
    let b = add("B.ARW", 3, Some("2024:05:01 10:00:00"), 1);
    let n1 = add("N1.ARW", 4, None, 0);
    let d = add("D.ARW", 5, Some("2024:04:30 10:00:00"), 3);
    let n2 = add("N2.ARW", 6, None, 2);
    SortFixture {
        f,
        c,
        a,
        b,
        n1,
        d,
        n2,
    }
}

fn sorted(s: &SortFixture, key: SortKey, direction: SortDirection) -> Vec<VariantId> {
    s.f.cat
        .search(&Filter::default(), &Sort::new(key, direction))
        .unwrap()
}

#[test]
fn capture_time_sort_puts_null_last_and_ties_by_variant_id() {
    let s = sort_fixture();
    assert_eq!(
        sorted(&s, SortKey::CaptureTime, SortDirection::Ascending),
        vec![s.d, s.a, s.b, s.c, s.n1, s.n2]
    );
    // 降順でも、撮影日時のないものは最後、同じ日時は variant.id の順。
    assert_eq!(
        sorted(&s, SortKey::CaptureTime, SortDirection::Descending),
        vec![s.c, s.a, s.b, s.d, s.n1, s.n2]
    );
}

#[test]
fn virtual_copies_sort_next_to_their_master_by_id() {
    let mut s = sort_fixture();
    // A の仮想コピーは A と同じ撮影日時。B（A と同じ日時）より id が大きいので B の後。
    let vc = s.f.cat.create_virtual_copy(s.a, None).unwrap();
    assert_eq!(
        sorted(&s, SortKey::CaptureTime, SortDirection::Ascending),
        vec![s.d, s.a, s.b, vc, s.c, s.n1, s.n2]
    );
    let masters =
        s.f.cat
            .search(
                &Filter {
                    masters_only: true,
                    ..Default::default()
                },
                &Sort::default(),
            )
            .unwrap();
    assert!(!masters.contains(&vc));
    assert_eq!(masters.len(), 6);
}

#[test]
fn file_name_rating_and_import_order_sorts() {
    let s = sort_fixture();
    assert_eq!(
        sorted(&s, SortKey::FileName, SortDirection::Ascending),
        vec![s.a, s.b, s.c, s.d, s.n1, s.n2]
    );
    assert_eq!(
        sorted(&s, SortKey::FileName, SortDirection::Descending),
        vec![s.n2, s.n1, s.d, s.c, s.b, s.a]
    );
    // 評価が同じ（C と D は 3）ものは variant.id の順。
    assert_eq!(
        sorted(&s, SortKey::Rating, SortDirection::Descending),
        vec![s.a, s.c, s.d, s.n2, s.b, s.n1]
    );
    assert_eq!(
        sorted(&s, SortKey::Rating, SortDirection::Ascending),
        vec![s.n1, s.b, s.n2, s.c, s.d, s.a]
    );
    assert_eq!(
        sorted(&s, SortKey::ImportOrder, SortDirection::Ascending),
        vec![s.c, s.a, s.b, s.n1, s.d, s.n2]
    );
    assert_eq!(
        sorted(&s, SortKey::ImportOrder, SortDirection::Descending),
        vec![s.n2, s.d, s.n1, s.b, s.a, s.c]
    );
}

#[test]
fn rating_flag_and_label_filters() {
    let mut s = sort_fixture();
    let search = |s: &SortFixture, filter: Filter| {
        s.f.cat
            .search(
                &filter,
                &Sort::new(SortKey::ImportOrder, SortDirection::Ascending),
            )
            .unwrap()
    };
    // ★3 以上。
    assert_eq!(
        search(
            &s,
            Filter {
                rating_min: Rating::new(3),
                ..Default::default()
            }
        ),
        vec![s.c, s.a, s.d]
    );
    // ★1〜3（境界を含む）。
    assert_eq!(
        search(
            &s,
            Filter {
                rating_min: Rating::new(1),
                rating_max: Rating::new(3),
                ..Default::default()
            }
        ),
        vec![s.c, s.b, s.d, s.n2]
    );
    s.f.cat.set_flag(&[s.a, s.b], Flag::Picked).unwrap();
    s.f.cat.set_flag(&[s.c], Flag::Rejected).unwrap();
    assert_eq!(
        search(
            &s,
            Filter {
                flags: Some(vec![Flag::Picked]),
                ..Default::default()
            }
        ),
        vec![s.a, s.b]
    );
    assert_eq!(
        search(
            &s,
            Filter {
                flags: Some(vec![Flag::None, Flag::Rejected]),
                ..Default::default()
            }
        ),
        vec![s.c, s.n1, s.d, s.n2]
    );
    assert_eq!(
        search(
            &s,
            Filter {
                flags: Some(vec![]),
                ..Default::default()
            }
        ),
        vec![]
    );
    s.f.cat
        .set_color_label(&[s.a], Some(ColorLabel::Red))
        .unwrap();
    s.f.cat
        .set_color_label(&[s.b], Some(ColorLabel::Blue))
        .unwrap();
    assert_eq!(
        search(
            &s,
            Filter {
                color_labels: Some(vec![Some(ColorLabel::Red), Some(ColorLabel::Blue)]),
                ..Default::default()
            }
        ),
        vec![s.a, s.b]
    );
    assert_eq!(
        search(
            &s,
            Filter {
                color_labels: Some(vec![None]),
                ..Default::default()
            }
        ),
        vec![s.c, s.n1, s.d, s.n2]
    );
    assert_eq!(
        search(
            &s,
            Filter {
                color_labels: Some(vec![]),
                ..Default::default()
            }
        ),
        vec![]
    );
    // 一括変更の結果。
    let (r, fl, l) = s.f.cat.variant_marks(s.a).unwrap();
    assert_eq!((r.get(), fl, l), (5, Flag::Picked, Some(ColorLabel::Red)));
    s.f.cat.set_color_label(&[s.a], None).unwrap();
    assert_eq!(s.f.cat.variant_marks(s.a).unwrap().2, None);
    // 存在しない id は数えない。
    assert_eq!(
        s.f.cat
            .set_rating(&[s.a, VariantId::new(9999)], Rating::MIN)
            .unwrap(),
        1
    );
    assert_eq!(s.f.cat.set_rating(&[], Rating::MIN).unwrap(), 0);
    assert_eq!(s.f.cat.count(&Filter::default()).unwrap(), 6);
}

#[test]
fn capture_time_range_is_utc_and_half_open() {
    let s = sort_fixture();
    // A と B は 2024-05-01 10:00 JST = 01:00 UTC。
    let from = Utc.with_ymd_and_hms(2024, 5, 1, 1, 0, 0).unwrap();
    let until = Utc.with_ymd_and_hms(2024, 5, 2, 1, 0, 0).unwrap();
    let r =
        s.f.cat
            .search(
                &Filter {
                    captured_from: Some(from),
                    captured_until: Some(until),
                    ..Default::default()
                },
                &Sort::default(),
            )
            .unwrap();
    // 下限は含み、上限（C の時刻）は含まない。撮影日時のないものは除く。
    assert_eq!(r, vec![s.a, s.b]);
    let r =
        s.f.cat
            .search(
                &Filter {
                    captured_from: Some(from),
                    ..Default::default()
                },
                &Sort::default(),
            )
            .unwrap();
    assert_eq!(r, vec![s.a, s.b, s.c]);
    // 範囲外の年でもエラーにならない。
    let far = Utc.with_ymd_and_hms(20000, 1, 1, 0, 0, 0).unwrap();
    let r =
        s.f.cat
            .search(
                &Filter {
                    captured_until: Some(far),
                    ..Default::default()
                },
                &Sort::default(),
            )
            .unwrap();
    assert_eq!(r.len(), 4);
}

#[test]
fn camera_lens_kind_and_status_filters() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None);
    let mut req = photo_req(f.folder, "B.ARW", 2, None);
    req.metadata = crate::MediaMetadata::Photo(super::photo_meta("ILCE-7RM5", "FE 50mm F1.2 GM"));
    let b = f.cat.register_file(&req).unwrap();
    let v = f
        .cat
        .register_file(&video_req(f.folder, "C.MP4", 3, None))
        .unwrap();
    let all = Sort::new(SortKey::ImportOrder, SortDirection::Ascending);
    let search = |f: &Fixture, filter: Filter| f.cat.search(&filter, &all).unwrap();
    assert_eq!(
        search(
            &f,
            Filter {
                cameras: Some(vec!["SONY ILCE-7RM5".to_owned()]),
                ..Default::default()
            }
        ),
        vec![b.master_variant_id]
    );
    assert_eq!(
        search(
            &f,
            Filter {
                lenses: Some(vec![
                    "FE 24-70mm F2.8 GM II".to_owned(),
                    "FE 50mm F1.2 GM".to_owned()
                ]),
                ..Default::default()
            }
        ),
        vec![a.master_variant_id, b.master_variant_id]
    );
    assert_eq!(
        search(
            &f,
            Filter {
                kind: Some(AssetKind::Video),
                ..Default::default()
            }
        ),
        vec![v.master_variant_id]
    );
    assert_eq!(
        search(
            &f,
            Filter {
                kind: Some(AssetKind::Photo),
                ..Default::default()
            }
        )
        .len(),
        2
    );
    f.cat
        .set_file_status(b.file_id, FileStatus::Missing, Some("移動された"))
        .unwrap();
    assert_eq!(
        search(
            &f,
            Filter {
                file_statuses: Some(vec![FileStatus::Missing, FileStatus::Error]),
                ..Default::default()
            }
        ),
        vec![b.master_variant_id]
    );
    assert_eq!(
        f.cat.distinct_cameras().unwrap(),
        vec![
            ("SONY ILCE-7M4".to_owned(), 1),
            ("SONY ILCE-7RM5".to_owned(), 1)
        ]
    );
    assert_eq!(f.cat.distinct_lenses().unwrap().len(), 2);
}

#[test]
fn folder_filter_with_and_without_subfolders() {
    let mut f = Fixture::new();
    let top = f.photo("A.ARW", 1, None);
    let sub = f.cat.ensure_folder(f.volume, "2024/京都/day1").unwrap();
    let other = f.cat.ensure_folder(f.volume, "2024/大阪").unwrap();
    let in_sub = f
        .cat
        .register_file(&photo_req(sub, "B.ARW", 2, None))
        .unwrap();
    let in_other = f
        .cat
        .register_file(&photo_req(other, "C.ARW", 3, None))
        .unwrap();
    let all = Sort::new(SortKey::ImportOrder, SortDirection::Ascending);
    let only = |include_subfolders: bool, folder| {
        f.cat
            .search(
                &Filter {
                    folder: Some(FolderFilter {
                        folder_id: folder,
                        include_subfolders,
                    }),
                    ..Default::default()
                },
                &all,
            )
            .unwrap()
    };
    assert_eq!(only(false, f.folder), vec![top.master_variant_id]);
    assert_eq!(
        only(true, f.folder),
        vec![top.master_variant_id, in_sub.master_variant_id]
    );
    let root = f.cat.find_folder(f.volume, "").unwrap().unwrap();
    assert_eq!(only(false, root), vec![]);
    assert_eq!(
        only(true, root),
        vec![
            top.master_variant_id,
            in_sub.master_variant_id,
            in_other.master_variant_id
        ]
    );
}

#[test]
fn keyword_filter_includes_child_keywords() {
    let mut f = Fixture::new();
    let a = f.photo("A.ARW", 1, None).master_variant_id;
    let b = f.photo("B.ARW", 2, None).master_variant_id;
    let c = f.photo("C.ARW", 3, None).master_variant_id;
    let place = f.cat.ensure_keyword(None, "場所").unwrap();
    let kyoto = f.cat.ensure_keyword(Some(place), "京都").unwrap();
    let gion = f.cat.ensure_keyword(Some(kyoto), "祇園").unwrap();
    let family = f.cat.ensure_keyword(None, "家族").unwrap();
    f.cat.add_keyword(&[a], kyoto).unwrap();
    f.cat.add_keyword(&[b], gion).unwrap();
    f.cat.add_keyword(&[b, c], family).unwrap();
    let all = Sort::new(SortKey::ImportOrder, SortDirection::Ascending);
    let by = |kws: Vec<genzo_model::KeywordId>| {
        f.cat
            .search(
                &Filter {
                    keywords: kws,
                    ..Default::default()
                },
                &all,
            )
            .unwrap()
    };
    assert_eq!(by(vec![place]), vec![a, b]);
    assert_eq!(by(vec![kyoto]), vec![a, b]);
    assert_eq!(by(vec![gion]), vec![b]);
    // 複数のキーワードは AND。
    assert_eq!(by(vec![place, family]), vec![b]);
    assert_eq!(by(vec![family]), vec![b, c]);
}

#[test]
fn combined_filters_and_explain() {
    let s = sort_fixture();
    let filter = Filter {
        rating_min: Rating::new(1),
        kind: Some(AssetKind::Photo),
        text: Some("arw".to_owned()),
        cameras: Some(vec!["SONY ILCE-7M4".to_owned()]),
        ..Default::default()
    };
    let r = s.f.cat.search(&filter, &Sort::default()).unwrap();
    assert_eq!(r, vec![s.d, s.a, s.b, s.c, s.n2]);
    assert_eq!(s.f.cat.count(&filter).unwrap(), 5);
    let plan = s.f.cat.explain_search(&filter, &Sort::default()).unwrap();
    assert!(!plan.is_empty());
}

#[test]
fn summaries_follow_requested_order_and_skip_missing() {
    let mut s = sort_fixture();
    let jpg = s.f.photo("A.JPG", 99, Some("2024:05:01 10:00:00"));
    assert!(jpg.paired);
    let ids = vec![s.n2, VariantId::new(9999), s.a, s.c];
    let sums = s.f.cat.variant_summaries(&ids).unwrap();
    assert_eq!(
        sums.iter().map(|x| x.variant_id).collect::<Vec<_>>(),
        vec![s.n2, s.a, s.c]
    );
    let a = &sums[1];
    assert_eq!(a.file_name, "A.ARW");
    assert!(a.has_sidecar_jpeg);
    assert!(a.is_master);
    assert_eq!(a.kind, AssetKind::Photo);
    assert_eq!(a.rating.get(), 5);
    assert_eq!(a.flag, Flag::None);
    assert_eq!(a.color_label, None);
    assert_eq!(a.camera.as_deref(), Some("SONY ILCE-7M4"));
    assert_eq!(a.width, Some(7008));
    assert_eq!(a.file_status, FileStatus::Ok);
    assert_eq!(a.file_revision, 1);
    assert_eq!(a.folder_id, s.f.folder);
    assert_eq!(a.process_version, genzo_model::CURRENT_PROCESS_VERSION);
    assert_eq!(
        a.capture.utc_db_string().as_deref(),
        Some("2024-05-01T01:00:00.000Z")
    );
    assert_eq!(a.capture.to_string(), "2024-05-01 10:00:00 +09:00");
    assert!(!sums[0].has_sidecar_jpeg);
    assert!(sums[0].capture.utc.is_none());
    assert!(s.f.cat.variant_summaries(&[]).unwrap().is_empty());
    // 重複した id はそのまま返す。
    assert_eq!(s.f.cat.variant_summaries(&[s.a, s.a]).unwrap().len(), 2);
}

#[test]
fn search_returns_all_ids_for_thousands_of_rows() {
    let mut f = Fixture::new();
    let reqs: Vec<_> = (0..3000)
        .map(|i| {
            let when = if i % 100 == 0 {
                None
            } else {
                Some(format!("2024:05:01 10:{:02}:{:02}", (i / 60) % 60, i % 60))
            };
            photo_req(f.folder, &format!("IMG{i:05}.ARW"), i, when.as_deref())
        })
        .collect();
    f.cat.register_batch(&reqs).unwrap();
    let ids = f.cat.search(&Filter::default(), &Sort::default()).unwrap();
    assert_eq!(ids.len(), 3000);
    // 撮影日時のない 30 件は最後。
    let tail = f.cat.variant_summaries(&ids[2970..]).unwrap();
    assert!(tail.iter().all(|s| s.capture.utc.is_none()));
    let head = f.cat.variant_summaries(&ids[..2970]).unwrap();
    assert!(head.iter().all(|s| s.capture.utc.is_some()));
    // 並び順が撮影日時の昇順・同じ日時は id の順になっている。
    let keys: Vec<_> = head
        .iter()
        .map(|s| (s.capture.utc.unwrap(), s.variant_id))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}
