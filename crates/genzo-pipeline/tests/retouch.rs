//! スポット修正（DEV-21。処理バージョン 2）の結合テスト: 処理バージョン 1 での無視、スポットから離れた画素が
//! スポットのない描画と同じこと、タイルの分け方によらないこと、角度補正・切り抜きを変えてもスポットが写っている
//! ものの上からずれないこと（O 座標）、スポットを足してもセンサー処理をやり直さないこと、プレビューとフル解像度の差
//! （IQ-07b）を、エンジンの処理で確かめる。

mod common;

use common::*;
use genzo_color::RgbColorSpace;
use genzo_model::{CropRect, DevelopSettings, RenderQuality, RetouchMode, RetouchSpot};
use genzo_pipeline::finish::{UnimplementedSetting, unimplemented_settings};
use genzo_pipeline::{NeverCancel, PhotoSource, RgbImage};
use genzo_testkit::diff::{LabConversion, delta_e2000_stats};

fn v2() -> DevelopSettings {
    DevelopSettings {
        process_version: 2,
        ..Default::default()
    }
}

/// 左上の円・中ほどの線（コピースタンプ）。O 座標（長辺 1）。
fn spots() -> Vec<RetouchSpot> {
    vec![
        RetouchSpot {
            size: 0.04,
            ..RetouchSpot::new(vec![[0.15, 0.2]], [0.12, 0.05])
        },
        RetouchSpot {
            mode: RetouchMode::Clone,
            size: 0.015,
            feather: 30.0,
            ..RetouchSpot::new(vec![[0.1, 0.45], [0.25, 0.5], [0.4, 0.42]], [0.0, -0.08])
        },
    ]
}

fn full(source: &PhotoSource, s: &DevelopSettings, tile: u32) -> RgbImage {
    let r = engine(64, tile)
        .render_full_resolution(source, s, None, &NeverCancel)
        .unwrap();
    assert_eq!(r.non_finite_replaced, 0);
    r.b3.image
}

#[test]
fn version_1_ignores_retouch() {
    let source = raw_source(raw_from(&busy_scene(96, 64)));
    let base = DevelopSettings {
        process_version: 1,
        ..strong_settings()
    };
    let mut s = base.clone();
    s.retouch = spots();
    assert_bitwise_eq(&full(&source, &base, 64), &full(&source, &s, 64), "版 1");
    assert_eq!(unimplemented_settings(&s), [UnimplementedSetting::Retouch]);
    s.process_version = 2;
    assert!(unimplemented_settings(&s).is_empty());
}

#[test]
fn pixels_away_from_the_spots_are_unchanged() {
    // スポットは左側（コピー元を含めて x < 0.55）。右側は、スポットのない描画とビット単位で同じ（ガイドを使う
    // ステージは画像全体の情報を使うので、ここでは使わない）。
    let source = raw_source(raw_from(&busy_scene(160, 96)));
    let base = v2();
    let mut s = base.clone();
    s.retouch = spots();
    let a = full(&source, &base, 64);
    let b = full(&source, &s, 64);
    let (w, h) = a.dimensions();
    let mut changed = 0;
    for y in 0..h {
        for x in 0..w {
            let (p, q) = (a.pixel(x, y).unwrap(), b.pixel(x, y).unwrap());
            if x > w * 6 / 10 {
                assert!(
                    p.iter().zip(q).all(|(u, v)| u.to_bits() == v.to_bits()),
                    "({x}, {y}): {p:?} ≠ {q:?}"
                );
            } else if p != q {
                changed += 1;
            }
        }
    }
    assert!(changed > 300, "{changed}");
}

#[test]
fn results_do_not_depend_on_the_tiling() {
    let source = raw_source(raw_from(&busy_scene(160, 96)));
    let mut s = strong_settings();
    s.process_version = 2;
    s.retouch = spots();
    let mut rotated = s.clone();
    rotate_and_crop(
        &mut rotated,
        3.0,
        CropRect {
            left: 0.05,
            top: 0.1,
            right: 0.9,
            bottom: 0.95,
        },
    );
    for (name, s) in [("回転なし", s), ("回転・切り抜き", rotated)] {
        let a = full(&source, &s, 24);
        let b = full(&source, &s, 256);
        assert_bitwise_eq(&a, &b, name);
    }
}

#[test]
fn spots_stay_on_the_content_when_rotating_or_cropping() {
    // スポットは O 座標なので、A1（スポットを当てた後）は角度補正・切り抜きによらない。
    let source = raw_source(raw_from(&busy_scene(160, 96)));
    let e = engine(160, 64);
    let mut s = v2();
    s.retouch = spots();
    let a = e.a1(&source, &s, RenderQuality::Final, 160).unwrap();
    let mut r = s.clone();
    rotate_and_crop(
        &mut r,
        -4.0,
        CropRect {
            left: 0.2,
            top: 0.0,
            right: 1.0,
            bottom: 0.8,
        },
    );
    let b = e.a1(&source, &r, RenderQuality::Final, 160).unwrap();
    assert_bitwise_eq(&a.image, &b.image, "回転・切り抜き");
    let plain = e.sensor_a1(&source, &s, RenderQuality::Final, 160).unwrap();
    assert_ne!(a.image, plain.image);
}

#[test]
fn adding_spots_does_not_redo_the_sensor_processing() {
    let source = raw_source(raw_from(&busy_scene(160, 96)));
    let e = engine(160, 64);
    let mut s = strong_settings(); // トーンを使うのでガイドを作る
    s.process_version = 2;
    let before = e
        .render_preview(&source, &s, RenderQuality::Final, 160)
        .unwrap();
    assert!(before.used_guide);
    let stats = e.stats();
    s.retouch = spots();
    let after = e
        .render_preview(&source, &s, RenderQuality::Final, 160)
        .unwrap();
    let now = e.stats();
    // センサー処理（A1）はキャッシュから、ガイドはスポットを当てた画像から作り直す。
    assert_eq!(now.a1.misses, stats.a1.misses);
    assert_eq!(now.guide.misses, stats.guide.misses + 1);
    assert_ne!(before.b3.image, after.b3.image);
    // 同じ設定をもう一度描けば、スポットを当てた A1 もキャッシュから使う（結果は同じ）。
    let again = e
        .render_preview(&source, &s, RenderQuality::Final, 160)
        .unwrap();
    assert_bitwise_eq(&after.b3.image, &again.b3.image, "2 回目");
    assert_eq!(e.stats().guide.misses, now.guide.misses);
}

#[test]
fn preview_and_full_resolution_differences_stay_small_with_retouch() {
    // IQ-07b の計測値の回帰の検出（基準の判定ではない）。表はこのコンテナで測った値（平均・95 パーセンタイル・最大）に
    // 10% と 0.1 の余裕を足した上限。スポットを当てた A1 はプレビューとフル解像度でほぼ同じ値になる（コピー元・境界の
    // 補正は同じ O の位置から取る）が、最大はコピースタンプの縁の画素で大きい: このシーンでは写した色が元の色とまったく
    // 違い、プレビューは縁の重みを画素の中心で決める一方、基準はフル解像度の 3 × 3 画素の平均なので、縁の 1 画素の
    // 色が大きく違う（マスクの縁と同じ種類の差）。平均・95 パーセンタイルはスポットなしと同じ程度。
    let scene = wavy_scene(300, 201);
    let source = raw_source(raw_from(&scene));
    let e = engine(100, 128);
    let lab = LabConversion::linear(RgbColorSpace::Bt2020);
    let limits = |v: [f64; 3]| v.map(|x| x * 1.1 + 0.1);
    let mut s = v2();
    s.retouch = spots();
    for (name, s, measured) in [
        ("スポットなし", v2(), [0.211, 0.734, 6.973]),
        ("スポット", s, [0.378, 0.962, 38.979]),
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
