//! ローカル補正（マスク。DEV-18・DEV-19。処理バージョン 2）の結合テスト: 処理バージョン 1 での無視、マスクの外の画素が
//! マスクのない描画と同じこと、タイルの分け方・切り抜きによらないこと（G 座標で決まる）、画像全体を覆うマスクが全体の
//! 値と同じ結果になること、プレビューとフル解像度の差（IQ-07b）、重ねて表示・色域のスポイトを、エンジンの処理で
//! 確かめる。

mod common;

use common::*;
use genzo_color::RgbColorSpace;
use genzo_model::{
    BrushStroke, ColorSample, CropRect, DevelopSettings, LocalAdjustment, LocalDelta,
    MaskComponent, MaskMode, MaskShape, RenderQuality,
};
use genzo_pipeline::finish::{UnimplementedSetting, unimplemented_settings};
use genzo_pipeline::{NeverCancel, PhotoSource, RgbImage};
use genzo_testkit::diff::{LabConversion, delta_e2000_stats};

fn v2() -> DevelopSettings {
    DevelopSettings {
        process_version: 2,
        ..Default::default()
    }
}

fn mask(shape: MaskShape, delta: LocalDelta) -> LocalAdjustment {
    LocalAdjustment {
        delta,
        ..LocalAdjustment::new(MaskComponent::add(shape))
    }
}

fn radial(cx: f32, cy: f32, r: f32, feather: f32) -> MaskShape {
    MaskShape::Radial {
        cx,
        cy,
        rx: r,
        ry: r * 0.7,
        angle_deg: 20.0,
        feather,
    }
}

/// すべての項目を動かす差分。
fn every_delta() -> LocalDelta {
    LocalDelta {
        temperature: 30.0,
        tint: -20.0,
        exposure_ev: 0.7,
        contrast: 40.0,
        highlights: -50.0,
        shadows: 40.0,
        whites: 20.0,
        blacks: -20.0,
        texture: 40.0,
        clarity: 30.0,
        dehaze: 25.0,
        saturation: -40.0,
        sharpness: 60.0,
    }
}

/// 右側（G 座標の x ≥ 0.55）にだけ効くマスク（円形・ブラシ・範囲マスクの交差）。
fn right_side_masks() -> Vec<LocalAdjustment> {
    let mut a = mask(radial(0.78, 0.33, 0.12, 60.0), every_delta());
    a.components.push(MaskComponent {
        mode: MaskMode::Intersect,
        invert: false,
        shape: MaskShape::LuminanceRange {
            min: 10.0,
            max: 90.0,
            smoothness: 40.0,
        },
    });
    let b = mask(
        MaskShape::Brush {
            strokes: vec![BrushStroke {
                size: 0.04,
                feather: 50.0,
                flow: 80.0,
                density: 90.0,
                points: vec![[0.7, 0.15], [0.85, 0.2], [0.9, 0.5]],
                ..Default::default()
            }],
        },
        LocalDelta {
            exposure_ev: -0.6,
            sharpness: -80.0,
            saturation: 50.0,
            ..Default::default()
        },
    );
    vec![a, b]
}

/// 全体の調整（いろいろなステージが有効）。
fn global_features() -> DevelopSettings {
    let mut s = strong_settings();
    s.process_version = 2;
    s.presence.texture = 20.0;
    s.presence.clarity = 15.0;
    s.presence.dehaze = 10.0;
    s.detail.sharpening.amount = 40.0;
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
fn version_1_ignores_masks() {
    let source = raw_source(raw_from(&busy_scene(96, 64)));
    let base = DevelopSettings {
        process_version: 1,
        ..strong_settings()
    };
    let mut s = base.clone();
    s.masks = right_side_masks();
    assert_bitwise_eq(&full(&source, &base, 64), &full(&source, &s, 64), "版 1");
    assert_eq!(unimplemented_settings(&s), [UnimplementedSetting::Masks]);
    s.process_version = 2;
    assert!(unimplemented_settings(&s).is_empty());
}

#[test]
fn pixels_away_from_the_masks_are_unchanged() {
    // マスクは右側だけ。左側（周辺の画素を使うステージの半径より離れた画素）は、マスクのない描画とビット単位で同じ。
    let source = raw_source(raw_from(&busy_scene(160, 96)));
    for (name, base) in [("全体の調整あり", global_features()), ("マスクだけ", v2())] {
        let mut s = base.clone();
        s.masks = right_side_masks();
        let a = full(&source, &base, 64);
        let b = full(&source, &s, 64);
        let (w, h) = a.dimensions();
        let mut changed_right = 0;
        for y in 0..h {
            for x in 0..w {
                let (p, q) = (a.pixel(x, y).unwrap(), b.pixel(x, y).unwrap());
                if x < w / 2 - 8 {
                    assert!(
                        p.iter().zip(q).all(|(u, v)| u.to_bits() == v.to_bits()),
                        "{name} ({x}, {y}): {p:?} ≠ {q:?}"
                    );
                } else if p != q {
                    changed_right += 1;
                }
            }
        }
        assert!(changed_right > 500, "{name}: {changed_right}");
    }
}

#[test]
fn results_do_not_depend_on_the_tiling() {
    let source = raw_source(raw_from(&busy_scene(160, 96)));
    let mut s = global_features();
    s.masks = right_side_masks();
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
fn crop_does_not_move_the_masks() {
    // マスクは G 座標で決まるので、切り抜きを変えても同じ位置の画素は同じ（周辺光量補正は使わない）。
    let source = raw_source(raw_from(&busy_scene(160, 96)));
    let mut s = global_features();
    s.masks = right_side_masks();
    let e = engine(160, 64);
    let whole = e
        .render_preview(&source, &s, RenderQuality::Final, 160)
        .unwrap();
    let mut c = s.clone();
    c.geometry.crop = CropRect {
        left: 0.4,
        top: 0.1,
        right: 0.95,
        bottom: 0.8,
    };
    let part = e
        .render_preview(&source, &c, RenderQuality::Final, 160)
        .unwrap();
    let (ox, oy) = {
        let a = whole.geometry.crop_lattice();
        let b = part.geometry.crop_lattice();
        ((b.x - a.x) as u32, (b.y - a.y) as u32)
    };
    let (pw, ph) = part.b3.image.dimensions();
    for y in 0..ph {
        for x in 0..pw {
            let p = part.b3.image.pixel(x, y).unwrap();
            let q = whole.b3.image.pixel(x + ox, y + oy).unwrap();
            assert!(
                p.iter().zip(q).all(|(a, b)| a.to_bits() == b.to_bits()),
                "({x}, {y}): {p:?} ≠ {q:?}"
            );
        }
    }
}

#[test]
fn a_mask_over_the_whole_image_matches_the_global_values() {
    // 画像全体を覆うマスク（輝度の範囲 0〜100 だけの交差。重み 1）の差分は、同じ値の全体の調整と同じ結果になる
    // （露光量 +1 は 2 倍でちょうど、ほかの項目も同じ式・同じ値）。
    let source = raw_source(raw_from(&busy_scene(96, 64)));
    let everywhere = |delta: LocalDelta| LocalAdjustment {
        components: vec![MaskComponent {
            mode: MaskMode::Intersect,
            invert: false,
            shape: MaskShape::LuminanceRange {
                min: 0.0,
                max: 100.0,
                smoothness: 0.0,
            },
        }],
        delta,
        ..LocalAdjustment::new(MaskComponent::add(radial(0.0, 0.0, 0.0, 0.0)))
    };
    type Case = (&'static str, LocalDelta, fn(&mut DevelopSettings));
    let cases: [Case; 5] = [
        (
            "露光量",
            LocalDelta {
                exposure_ev: 1.0,
                ..Default::default()
            },
            |s| s.exposure_ev = 1.0,
        ),
        (
            "コントラスト",
            LocalDelta {
                contrast: 40.0,
                ..Default::default()
            },
            |s| s.contrast = 40.0,
        ),
        (
            "ハイライト・シャドウ",
            LocalDelta {
                highlights: -50.0,
                shadows: 30.0,
                ..Default::default()
            },
            |s| {
                s.tone.highlights = -50.0;
                s.tone.shadows = 30.0;
            },
        ),
        (
            "彩度",
            LocalDelta {
                saturation: -60.0,
                ..Default::default()
            },
            |s| s.color.saturation = -60.0,
        ),
        (
            "テクスチャ・明瞭度",
            LocalDelta {
                texture: 50.0,
                clarity: 40.0,
                ..Default::default()
            },
            |s| {
                s.presence.texture = 50.0;
                s.presence.clarity = 40.0;
            },
        ),
    ];
    for (name, delta, global) in cases {
        let mut local = v2();
        local.masks.push(everywhere(delta));
        let mut g = v2();
        global(&mut g);
        assert_bitwise_eq(&full(&source, &g, 64), &full(&source, &local, 64), name);
    }
}

#[test]
fn local_dehaze_builds_the_haze_estimate_and_local_values_add_to_the_global_ones() {
    let source = raw_source(raw_from(&busy_scene(96, 64)));
    let e = engine(96, 64);
    let mut s = v2();
    s.masks.push(mask(
        radial(0.5, 0.33, 0.3, 50.0),
        LocalDelta {
            dehaze: 60.0,
            ..Default::default()
        },
    ));
    let r = e
        .render_preview(&source, &s, RenderQuality::Final, 96)
        .unwrap();
    assert!(
        r.used_guide,
        "かすみの除去のマスクはガイドとかすみの推定を使う"
    );
    assert!(r.active_stages.contains(&"finish.texture"));
    // 局所の露光量は全体の露光量に足す（中心の画素はマスクの重み 1）。
    let mut a = v2();
    a.exposure_ev = 0.5;
    a.masks.push(mask(
        radial(0.5, 0.33, 0.3, 0.0),
        LocalDelta {
            exposure_ev: 0.5,
            ..Default::default()
        },
    ));
    let mut b = v2();
    b.exposure_ev = 1.0;
    let pa = e
        .render_preview(&source, &a, RenderQuality::Final, 96)
        .unwrap();
    let pb = e
        .render_preview(&source, &b, RenderQuality::Final, 96)
        .unwrap();
    assert_eq!(
        pa.b3.image.pixel(48, 32).unwrap(),
        pb.b3.image.pixel(48, 32).unwrap()
    );
}

#[test]
fn preview_and_full_resolution_differences_stay_small_with_masks() {
    // IQ-07b の計測値の回帰の検出（基準の判定ではない）。マスクは G 座標で決まり、ブラシのラスターもプレビューと
    // フル解像度で同じものを使うので、差は全体の調整のときと同じ程度（マスクの差分にシャープ・テクスチャがあり、
    // finish_detail.rs の「シャープ」「すべて」に近い）。表はこのコンテナで測った値（平均・95 パーセンタイル・最大）に
    // 10% と 0.1 の余裕を足した上限。
    let scene = wavy_scene(300, 201);
    let source = raw_source(raw_from(&scene));
    let e = engine(100, 128);
    let lab = LabConversion::linear(RgbColorSpace::Bt2020);
    let limits = |v: [f64; 3]| v.map(|x| x * 1.1 + 0.1);
    let mut masked = v2();
    masked.masks = right_side_masks();
    masked.masks.push(mask(
        MaskShape::Linear {
            x0: 0.1,
            y0: 0.0,
            x1: 0.1,
            y1: 0.5,
        },
        LocalDelta {
            exposure_ev: 0.8,
            highlights: -40.0,
            ..Default::default()
        },
    ));
    for (name, s, measured) in [
        ("マスクなし", v2(), [0.211, 0.734, 6.973]),
        ("マスク", masked, [0.287, 0.985, 8.301]),
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

#[test]
fn overlay_and_color_pick_use_the_preview_grid() {
    let source = raw_source(raw_from(&color_edges_scene(120, 80)));
    let e = engine(120, 64);
    let mut s = v2();
    s.masks.push(mask(
        radial(0.25, 0.2, 0.1, 0.0),
        LocalDelta::default(), // 差分 0 でも重ねて表示できる
    ));
    let preview = e
        .render_preview(&source, &s, RenderQuality::Final, 120)
        .unwrap();
    let o = e.mask_overlay(&source, &s, 0, 120, false).unwrap();
    assert_eq!((o.width, o.height), preview.b3.image.dimensions());
    // G 座標 (0.25, 0.2) は画素 (30, 24) の近く（長辺 120 の画像の G の長辺は 1）。
    assert_eq!(o.weights[(24 * o.width + 30) as usize], 1.0);
    assert_eq!(o.weights[(70 * o.width + 110) as usize], 0.0);
    assert!(e.mask_overlay(&source, &s, 1, 120, false).is_err());
    // スポイト: 点の G 座標と、その色の OKLab。同じ色を見本にした色域の範囲は、その点を選ぶ。
    let pick = e
        .mask_color_at(&source, &s, (0.25, 0.3), 120, false)
        .unwrap();
    assert!(
        (pick.x - 0.25).abs() < 0.01 && (pick.y - 0.2).abs() < 0.01,
        "{pick:?}"
    );
    assert!(pick.samples > 0);
    s.masks[0] = mask(
        MaskShape::ColorRange {
            samples: vec![ColorSample {
                x: pick.x as f32,
                y: pick.y as f32,
                lab: pick.lab,
            }],
            amount: 30.0,
        },
        LocalDelta::default(),
    );
    let o = e.mask_overlay(&source, &s, 0, 120, false).unwrap();
    assert!(o.weights[(24 * o.width + 30) as usize] > 0.9);
    assert!(
        e.mask_color_at(&source, &s, (1.5, 0.3), 120, false)
            .is_err()
    );
}
