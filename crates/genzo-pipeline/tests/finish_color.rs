//! ステージ 13 のカラーミキサー（DEV-09）・カラーグレーディング（DEV-10）・白黒（DEV-25）の結合テスト（処理バージョン
//! 2）: 処理バージョン 1 での無視、タイルの分け方によらないこと、白黒が灰色になること、プレビューとフル解像度の一致
//! （IQ-07b）を、エンジンの処理で確かめる。

mod common;

use common::*;
use genzo_color::RgbColorSpace;
use genzo_model::{DevelopSettings, GradingWheel, HslAdjustment, RenderQuality};
use genzo_pipeline::finish::{UnimplementedSetting, unimplemented_settings};
use genzo_pipeline::{NeverCancel, PhotoSource, RgbImage};
use genzo_testkit::diff::{LabConversion, delta_e2000_stats};

/// v1 の機能の色の項目をすべて使う設定（処理バージョン 2）。
fn color_features(black_white: bool) -> DevelopSettings {
    let mut s = DevelopSettings {
        process_version: 2,
        ..Default::default()
    };
    s.color.saturation = 20.0;
    s.color.mixer.blue = HslAdjustment {
        hue: -30.0,
        saturation: 40.0,
        luminance: -60.0,
    };
    s.color.mixer.orange.luminance = 30.0;
    s.color.mixer.green.hue = 50.0;
    s.color.grading.shadows = GradingWheel {
        hue: 220.0,
        saturation: 30.0,
        luminance: 10.0,
    };
    s.color.grading.highlights = GradingWheel {
        hue: 40.0,
        saturation: 25.0,
        luminance: -10.0,
    };
    s.color.grading.balance = 20.0;
    s.color.black_white.enabled = black_white;
    s.color.black_white.mix.red = 40.0;
    s.color.black_white.mix.blue = -50.0;
    s
}

fn full(source: &PhotoSource, s: &DevelopSettings, tile: u32) -> RgbImage {
    let r = engine(64, tile)
        .render_full_resolution(source, s, None, &NeverCancel)
        .unwrap();
    assert_eq!(r.non_finite_replaced, 0);
    r.b3.image
}

#[test]
fn version_1_ignores_the_mixer_grading_and_black_white() {
    let source = raw_source(raw_from(&busy_scene(96, 64)));
    let mut s = color_features(true);
    s.process_version = 1;
    let base = DevelopSettings {
        process_version: 1,
        color: genzo_model::ColorParams {
            saturation: 20.0,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_bitwise_eq(&full(&source, &base, 64), &full(&source, &s, 64), "版 1");
    assert_eq!(
        unimplemented_settings(&s),
        [
            UnimplementedSetting::ColorMixer,
            UnimplementedSetting::ColorGrading,
            UnimplementedSetting::BlackWhite,
        ]
    );
    // 版 2 では適用する（結果が変わり、知らせない）。
    s.process_version = 2;
    assert!(unimplemented_settings(&s).is_empty());
    let a = full(&source, &base, 64);
    let b = full(&source, &s, 64);
    let changed = a
        .data()
        .iter()
        .zip(b.data())
        .filter(|(p, q)| p != q)
        .count();
    assert!(changed > a.data().len() / 2, "{changed}");
}

#[test]
fn results_do_not_depend_on_the_tiling() {
    let source = raw_source(raw_from(&busy_scene(160, 96)));
    for bw in [false, true] {
        let s = color_features(bw);
        let a = full(&source, &s, 32);
        let b = full(&source, &s, 256);
        assert_bitwise_eq(&a, &b, if bw { "白黒" } else { "カラー" });
    }
}

#[test]
fn black_white_without_grading_gives_grays() {
    let source = raw_source(raw_from(&busy_scene(96, 64)));
    let mut s = color_features(true);
    s.color.grading = Default::default();
    let out = full(&source, &s, 64);
    for (i, p) in out.data().iter().enumerate() {
        let m = p[1].abs().max(1e-3);
        assert!(
            (p[0] - p[1]).abs() <= 2e-5 * m && (p[2] - p[1]).abs() <= 2e-5 * m,
            "画素 {i}: {p:?}"
        );
    }
    // グレーディングを足すと色が付く（白黒の写真の色付け）。
    let toned = full(&source, &color_features(true), 64);
    let colored = toned
        .data()
        .iter()
        .filter(|p| (p[0] - p[2]).abs() > 1e-3)
        .count();
    // 中間調（色付けしていない範囲）は灰色のまま。
    assert!(colored > toned.data().len() / 5, "{colored}");
}

#[test]
fn preview_and_full_resolution_differences_do_not_regress() {
    // IQ-07b（プレビューとフル解像度の差）の計測値の回帰の検出（基準の判定ではない。engine.rs の
    // iq07b_measured_values_do_not_regress と同じ考え方）。ステージ 13 は画素ごとの処理だが、色相・彩度で効き方の
    // 変わる非線形の処理なので、縮小してからかけるプレビューと、かけてから縮小するフル解像度で差が出る（色の
    // 境界で、混ざった色に別の帯の調整がかかる）。表は、このコンテナで測った値（平均・95 パーセンタイル・最大）に
    // 10% と 0.1 の余裕を足した上限。
    let scene = wavy_scene(300, 201);
    let source = raw_source(raw_from(&scene));
    let e = engine(100, 128);
    let lab = LabConversion::linear(RgbColorSpace::Bt2020);
    let limits = |v: [f64; 3]| v.map(|x| x * 1.1 + 0.1);
    for (name, s, measured) in [
        ("既定", DevelopSettings::default(), [0.211, 0.734, 6.973]),
        ("カラー", color_features(false), [0.596, 2.051, 25.032]),
        ("白黒", color_features(true), [0.321, 1.710, 11.031]),
    ] {
        let preview = e
            .render_preview(&source, &s, RenderQuality::Final, 100)
            .unwrap();
        let reference = iq07b_reference(&e, &source, &s, &preview);
        let st = delta_e2000_stats(&reference, &to_float(&preview.b3.image), lab).unwrap();
        eprintln!(
            "IQ-07b wavy/{name}: 平均 {:.3}、95 パーセンタイル {:.3}、最大 {:.3}",
            st.mean, st.p95, st.max
        );
        let [m, p, x] = limits(measured);
        assert!(st.mean <= m && st.p95 <= p && st.max <= x, "{name}: {st:?}");
    }
}
