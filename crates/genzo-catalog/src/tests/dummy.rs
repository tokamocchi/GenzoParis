//! ダミーデータ（PoC-6 用）の動作確認。テストでは数千件で動くことだけを確かめる。

use std::time::Instant;

use super::Fixture;
use crate::{DummySpec, Filter, Sort, SortDirection, SortKey, populate_dummy};

#[test]
fn dummy_population_works_with_thousands_of_assets() {
    let mut f = Fixture::new();
    let spec = DummySpec {
        assets: 3000,
        batch_size: 1000,
        assets_per_folder: 400,
        ..Default::default()
    };
    let start = Instant::now();
    let report = populate_dummy(&mut f.cat, &spec).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(report.assets, 3000);
    let counts = f.cat.counts().unwrap();
    assert_eq!(counts.assets, 3000);
    assert_eq!(counts.variants, 3000 + report.virtual_copies as u64);
    assert!(counts.files > 3000, "RAW と JPEG のペアがある");
    // 仮想コピーは 10% 前後。
    assert!((150..450).contains(&report.virtual_copies), "{report:?}");
    assert!(report.history_entries > 0);
    assert!(report.keyword_tags > 0);

    // 代表的な検索が動く。
    let all = f.cat.search(&Filter::default(), &Sort::default()).unwrap();
    assert_eq!(all.len() as u64, counts.variants);
    let text = f
        .cat
        .search(
            &Filter {
                text: Some("京都".to_owned()),
                ..Default::default()
            },
            &Sort::new(SortKey::Rating, SortDirection::Descending),
        )
        .unwrap();
    assert!(!text.is_empty());
    let picked = f
        .cat
        .search(
            &Filter {
                rating_min: genzo_model::Rating::new(3),
                kind: Some(genzo_model::AssetKind::Photo),
                ..Default::default()
            },
            &Sort::default(),
        )
        .unwrap();
    assert!(!picked.is_empty());
    let sums = f.cat.variant_summaries(&all[..100]).unwrap();
    assert_eq!(sums.len(), 100);
    assert!(f.cat.check_integrity().unwrap().is_ok());

    // 同じ設定で 2 回目を呼んでも件数は増えない（登録の冪等性）。
    let again = populate_dummy(&mut f.cat, &spec).unwrap();
    assert_eq!(again.assets, 0);
    assert_eq!(f.cat.counts().unwrap().assets, 3000);
    eprintln!("ダミー 3000 件の登録: {elapsed:?}");
}

#[test]
fn dummy_spec_is_validated() {
    let mut f = Fixture::new();
    assert!(
        populate_dummy(
            &mut f.cat,
            &DummySpec {
                batch_size: 0,
                ..Default::default()
            }
        )
        .is_err()
    );
}

/// PoC-6 の計測用（通常のテストでは実行しない）。
///
/// 実行例: `GENZO_POC6_ASSETS=500000 cargo test -p genzo-catalog --release -- --ignored poc6 --nocapture`
/// （件数を省略すると 10 万件）。`GENZO_POC6_ANALYZE=1` を付けると、検索の前に `ANALYZE` で
/// 統計を集める（実行計画の比較用）。計測の結果は標準エラー出力に表示する。
#[test]
#[ignore = "大量のデータで計測するため、明示的に実行する"]
fn poc6_scale_measurement() {
    let assets: usize = std::env::var("GENZO_POC6_ASSETS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);
    let mut f = Fixture::new();
    let start = Instant::now();
    let report = populate_dummy(
        &mut f.cat,
        &DummySpec {
            assets,
            ..Default::default()
        },
    )
    .unwrap();
    eprintln!("登録 {assets} 件: {:?}（{report:?}）", start.elapsed());
    let size = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let db = f.catalog_path();
    let wal = db.with_extension("db-wal");
    eprintln!("DB {} バイト、WAL {} バイト", size(&db), size(&wal));

    if std::env::var("GENZO_POC6_ANALYZE").is_ok() {
        let t = Instant::now();
        f.cat
            .conn
            .execute_batch("PRAGMA analysis_limit = 1000; ANALYZE;")
            .unwrap();
        eprintln!("ANALYZE: {:?}", t.elapsed());
    }
    let jst = |y, m, d| chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, y, m, d, 0, 0, 0).unwrap();
    let cases: Vec<(&str, Filter, Sort)> = vec![
        ("全件・撮影日時順", Filter::default(), Sort::default()),
        (
            "全件・撮影日時の降順",
            Filter::default(),
            Sort::new(SortKey::CaptureTime, SortDirection::Descending),
        ),
        (
            "★3 以上",
            Filter {
                rating_min: genzo_model::Rating::new(3),
                ..Default::default()
            },
            Sort::default(),
        ),
        (
            "★3 以上 ＋ 期間 1 年",
            Filter {
                rating_min: genzo_model::Rating::new(3),
                captured_from: Some(jst(2020, 1, 1)),
                captured_until: Some(jst(2021, 1, 1)),
                ..Default::default()
            },
            Sort::default(),
        ),
        (
            "動画のみ",
            Filter {
                kind: Some(genzo_model::AssetKind::Video),
                ..Default::default()
            },
            Sort::default(),
        ),
        (
            "カメラ",
            Filter {
                cameras: Some(vec!["SONY ILCE-7M4".to_owned()]),
                ..Default::default()
            },
            Sort::default(),
        ),
        (
            "テキスト「京都」（2 文字・LIKE）",
            Filter {
                text: Some("京都".to_owned()),
                ..Default::default()
            },
            Sort::default(),
        ),
        (
            "テキスト「海」（1 文字・LIKE）",
            Filter {
                text: Some("海".to_owned()),
                ..Default::default()
            },
            Sort::default(),
        ),
        (
            "テキスト「夕焼け」（3 文字・FTS）",
            Filter {
                text: Some("夕焼け".to_owned()),
                ..Default::default()
            },
            Sort::default(),
        ),
        (
            "ファイル名順",
            Filter::default(),
            Sort::new(SortKey::FileName, SortDirection::Ascending),
        ),
        (
            "採用フラグ ＋ 評価順",
            Filter {
                flags: Some(vec![genzo_model::Flag::Picked]),
                ..Default::default()
            },
            Sort::new(SortKey::Rating, SortDirection::Descending),
        ),
    ];
    for (label, filter, sort) in &cases {
        // 1 回目はキャッシュの温め。2 回目を計測する。
        f.cat.search(filter, sort).unwrap();
        let t = Instant::now();
        let ids = f.cat.search(filter, sort).unwrap();
        let elapsed = t.elapsed();
        eprintln!("{label}: {} 件、{elapsed:?}", ids.len());
        for line in f.cat.explain_search(filter, sort).unwrap() {
            eprintln!("    {line}");
        }
    }
    let ids = f.cat.search(&Filter::default(), &Sort::default()).unwrap();
    let t = Instant::now();
    let mid = ids.len() / 2;
    let sums = f
        .cat
        .variant_summaries(&ids[mid..(mid + 200).min(ids.len())])
        .unwrap();
    eprintln!("表示範囲 {} 件の詳細: {:?}", sums.len(), t.elapsed());
    let t = Instant::now();
    f.cat
        .set_rating(
            &ids[..1000.min(ids.len())],
            genzo_model::Rating::new(2).unwrap(),
        )
        .unwrap();
    eprintln!("評価の一括変更 1000 件: {:?}", t.elapsed());
    let t = Instant::now();
    f.cat
        .save_develop(
            ids[0],
            &genzo_model::DevelopSettings {
                exposure_ev: 0.3,
                ..Default::default()
            },
            "露光量",
        )
        .unwrap();
    eprintln!(
        "現像設定の保存 1 件（synchronous = FULL）: {:?}",
        t.elapsed()
    );
    let t = Instant::now();
    let backup = f.dir.path().join("backup.db");
    f.cat.backup_to(&backup).unwrap();
    eprintln!("バックアップ: {:?}、{} バイト", t.elapsed(), size(&backup));
    let t = Instant::now();
    let report = f.cat.check_integrity().unwrap();
    eprintln!(
        "詳細チェック: {:?}（問題なし: {}）",
        t.elapsed(),
        report.is_ok()
    );
}
