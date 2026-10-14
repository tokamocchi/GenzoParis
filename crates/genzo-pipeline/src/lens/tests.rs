//! レンズ補正（[`crate::lens`]）の単体テスト。

use genzo_model::{LensCorrection, Orientation, PhotoMetadata};
use genzo_raw::{
    CfaPattern, DngLensCorrection, DngVignetteRadial, DngWarpPlane, DngWarpRectilinear,
    LensCorrectionData, PixelRect, RawImage, SonyLensCorrection,
};

use super::*;
use crate::contract::ColorContract;
use crate::coords::CoordinateFrames;
use crate::image::{ImageTile, RgbImage, Roi};
use crate::sensor::apply_lens_correction;

fn raw(w: u32, h: u32, crop: Option<PixelRect>, lens: Option<LensCorrectionData>) -> RawImage {
    RawImage {
        width: w,
        height: h,
        cfa: CfaPattern::RGGB,
        data: vec![1000; (w * h) as usize],
        black_level: [512.0; 4],
        white_level: 16383.0,
        saturation_level: 16383.0,
        default_crop: crop,
        as_shot_wb: [2.0, 1.0, 1.5, 1.0],
        cam_xyz: None,
        metadata: PhotoMetadata::default(),
        lens_correction: lens,
    }
}

fn sony(distortion: Vec<i16>, vignetting: Vec<i16>, ca: i16) -> LensCorrectionData {
    let n = vignetting.len().max(distortion.len()).max(2);
    LensCorrectionData::SonyArw(SonyLensCorrection {
        distortion,
        vignetting,
        ca_red: vec![ca; n],
        ca_blue: vec![-ca; n],
    })
}

fn frames(w: u32, h: u32, o: Orientation) -> CoordinateFrames {
    CoordinateFrames::new(w, h, o).unwrap()
}

fn lens() -> LensCorrection {
    LensCorrection::default()
}

#[test]
fn knot_interpolation_follows_the_darktable_rules() {
    let k = [0.25, 0.75, 1.25];
    let v = [1.0, 3.0, 4.0];
    assert_eq!(interp_knots(&k, &v, 0.0), 1.0);
    assert_eq!(interp_knots(&k, &v, 0.25), 1.0);
    assert!((interp_knots(&k, &v, 0.5) - 2.0).abs() < 1e-12);
    assert!((interp_knots(&k, &v, 1.0) - 3.5).abs() < 1e-12);
    assert_eq!(interp_knots(&k, &v, 9.0), 4.0);
    // Sony の節点の位置は (i + 0.5) / (n − 1)。
    assert_eq!(sony_knots(3), vec![0.25, 0.75, 1.25]);
}

#[test]
fn no_correction_gives_no_distortion_and_no_stage_7() {
    let f = frames(600, 400, Orientation::Normal);
    assert_eq!(
        RadialDistortion::for_settings(&f, ProcessVersion::V2, &lens()),
        None
    );
    let r = raw(60, 40, None, None);
    assert_eq!(LensStagePlan::new(&r, &lens()), None);
    // 版 1 は値があっても作らない。
    let mut l = lens();
    l.distortion = 50.0;
    assert_eq!(
        RadialDistortion::for_settings(&f, ProcessVersion::V1, &l),
        None
    );
    // 埋め込みの値が 0 なら、プロファイルを有効にしても補正なしと同じ。
    let f = f.with_lens_geometry(ProfileDistortion::from_data(
        &sony(vec![0; 16], Vec::new(), 0),
        (600, 400),
    ));
    let mut l = lens();
    l.profile_enabled = true;
    assert_eq!(
        RadialDistortion::for_settings(&f, ProcessVersion::V2, &l),
        None
    );
}

/// D の枠の辺の上の点（各辺 33 点）。
fn edge_points(w: f64, h: f64) -> Vec<(f64, f64)> {
    let mut v = Vec::new();
    for i in 0..=32 {
        let t = f64::from(i) / 32.0;
        v.extend([(t * w, 0.0), (t * w, h), (0.0, t * h), (w, t * h)]);
    }
    v
}

#[test]
fn manual_distortion_fills_the_frame_without_empty_areas() {
    for o in [Orientation::Normal, Orientation::Rotate90Cw] {
        let f = frames(600, 400, o);
        let (ow, oh) = f.oriented_size();
        let (ow, oh) = (f64::from(ow), f64::from(oh));
        for amount in [-100.0f32, -30.0, 40.0, 100.0] {
            let mut l = lens();
            l.distortion = amount;
            let d = RadialDistortion::for_settings(&f, ProcessVersion::V2, &l).unwrap();
            // 樽型の補正（正）は写る範囲を広げ（z > 1）、糸巻き型の補正（負）は拡大する（z < 1）。
            assert_eq!(d.zoom > 1.0, amount > 0.0, "{amount}: {}", d.zoom);
            assert_eq!(d.center, (ow * 0.5, oh * 0.5));
            // D の枠の全体が O の内側から取れ、どこかの辺は O の端に届く。
            let mut touches = false;
            for (x, y) in edge_points(ow, oh) {
                let (u, v) = d.undistorted_to_oriented(x, y);
                assert!(
                    (-1e-3..=ow + 1e-3).contains(&u) && (-1e-3..=oh + 1e-3).contains(&v),
                    "{amount}: ({x}, {y}) → ({u}, {v})"
                );
                touches |= u < 0.5 || v < 0.5 || u > ow - 0.5 || v > oh - 0.5;
            }
            assert!(touches, "{amount}");
            // 中心は動かない。往復の変換は元に戻る。
            assert_eq!(d.undistorted_to_oriented(d.center.0, d.center.1), d.center);
            for (x, y) in [(10.0, 20.0), (ow - 3.0, oh - 7.0), (ow * 0.3, oh * 0.9)] {
                let (u, v) = d.undistorted_to_oriented(x, y);
                let (x2, y2) = d.oriented_to_undistorted(u, v);
                assert!((x - x2).abs() < 1e-6 && (y - y2).abs() < 1e-6, "{x} {y}");
            }
        }
    }
}

#[test]
fn positive_manual_distortion_pulls_the_corners_in_relative_to_the_edges() {
    // 樽型の補正: 四隅の取る位置は、辺の中央より中心に寄る（拡大・縮小の倍率を除いて比べる）。
    let f = frames(600, 400, Orientation::Normal);
    let mut l = lens();
    l.distortion = 60.0;
    let d = RadialDistortion::for_settings(&f, ProcessVersion::V2, &l).unwrap();
    let k_center = d.eval_f64(0.0);
    let k_corner = d.eval_f64(1.0);
    assert!(k_corner < k_center, "{k_corner} {k_center}");
}

#[test]
fn sony_profile_distortion_uses_the_default_crop_and_the_scale() {
    // 歪曲の値が外ほど負（2^−14 単位。隅で約 −5.5%）。中心は既定の切り抜きの中心。
    let crop = PixelRect::new(20, 10, 600, 400);
    let values: Vec<i16> = (0..16).map(|i| (-60 * i) as i16).collect();
    let data = sony(values, Vec::new(), 0);
    let geom = ProfileDistortion::from_data(&data, (640, 420)).unwrap();
    let f = frames(640, 420, Orientation::Normal)
        .with_default_crop(Some(crop))
        .unwrap()
        .with_lens_geometry(Some(geom));
    let mut l = lens();
    l.profile_enabled = true;
    let full = RadialDistortion::for_settings(&f, ProcessVersion::V2, &l).unwrap();
    assert_eq!(full.center, (300.0, 200.0));
    // 隅を中心へ寄せて取る（樽型の補正）ので、自動の倍率で写る範囲を広げる。
    assert!(full.zoom > 1.0, "{}", full.zoom);
    let bend = |d: &RadialDistortion| d.eval_f64(1.0) / d.eval_f64(0.0) - 1.0;
    // ρ = 1（節点 14.5〜15.5 の間）での倍率の変化は、中心（節点 0 の値 0）との差 ≒ −870 · 2^−14。
    let expect = -(60.0 * 14.5) / 16384.0;
    assert!(
        (bend(&full) - expect).abs() < 2e-3,
        "{} {expect}",
        bend(&full)
    );
    // 適用量 0 は補正なし、50% は曲がりがほぼ半分。
    l.distortion_scale = 0.0;
    assert_eq!(
        RadialDistortion::for_settings(&f, ProcessVersion::V2, &l),
        None
    );
    l.distortion_scale = 50.0;
    let half = RadialDistortion::for_settings(&f, ProcessVersion::V2, &l).unwrap();
    assert!((bend(&half) / bend(&full) - 0.5).abs() < 0.05);
    // プロファイルを無効にすれば使わない。一定の倍率の写像は、自動の倍率で打ち消される（補正なし）。
    l.profile_enabled = false;
    assert_eq!(
        RadialDistortion::for_settings(&f, ProcessVersion::V2, &l),
        None
    );
    let flat = ProfileDistortion::from_data(&sony(vec![-800; 16], Vec::new(), 0), (640, 420));
    let mut l = lens();
    l.profile_enabled = true;
    assert_eq!(
        RadialDistortion::for_settings(&f.with_lens_geometry(flat), ProcessVersion::V2, &l),
        None
    );
}

#[test]
fn dng_warp_uses_the_optical_center_and_the_farthest_corner() {
    let data = LensCorrectionData::DngOpcode(DngLensCorrection {
        warp: Some(DngWarpRectilinear {
            planes: vec![DngWarpPlane {
                kr: [1.0, -0.05, 0.0, 0.0],
                kt: [0.0, 0.001],
            }],
            center: [0.25, 0.5],
        }),
        vignette: None,
    });
    let geom = ProfileDistortion::from_data(&data, (800, 400)).unwrap();
    assert!(geom.ignores_tangential());
    let ProfileDistortion::Dng { center, radius, .. } = geom else {
        panic!()
    };
    assert_eq!(center, (200.0, 200.0));
    assert!((radius - (600.0f64).hypot(200.0)).abs() < 1e-9);
    let f = frames(800, 400, Orientation::Normal).with_lens_geometry(Some(geom));
    let mut l = lens();
    l.profile_enabled = true;
    let d = RadialDistortion::for_settings(&f, ProcessVersion::V2, &l).unwrap();
    assert_eq!(d.center, (200.0, 200.0));
    assert!((d.inv_r2 - 1.0 / (radius * radius)).abs() < 1e-15);
}

#[test]
fn stage_7_profile_and_manual_vignetting() {
    // Sony の周辺減光（値が外ほど大きい）: 中心の利得は 1 に近く、隅ほど大きい。
    let vig: Vec<i16> = (0..16).map(|i| (i * 300) as i16).collect();
    let r = raw(400, 300, None, Some(sony(Vec::new(), vig.clone(), 0)));
    let mut l = lens();
    l.profile_enabled = true;
    let plan = LensStagePlan::new(&r, &l).unwrap();
    assert!(plan.chromatic_aberration.is_none());
    assert_eq!(plan.margin(1), 0);
    let g = plan.vignetting.as_ref().unwrap();
    let at = |rho: f64| g.eval((rho * rho) as f32);
    // 中心の利得は 1 / v'(0)（v_0 = 0 なので 1）。
    assert!((at(0.0) - 1.0).abs() < 1e-6, "{}", at(0.0));
    assert!(at(0.5) > 1.0 && at(1.0) > at(0.5));
    // ρ = 1 の値（節点 14.5 / 15 … の補間）を式で確かめる: v' = 2^(0.5 − 2^(v · 2^−13 − 1))。
    let knots = sony_knots(16);
    let vp: Vec<f64> = vig
        .iter()
        .map(|&v| 2f64.powf(0.5 - 2f64.powf(f64::from(v) / 8192.0 - 1.0)))
        .collect();
    let expect = 1.0 / interp_knots(&knots, &vp, 1.0);
    assert!(
        (f64::from(at(1.0)) - expect).abs() < 1e-3,
        "{} {expect}",
        at(1.0)
    );
    // 適用量 0 は補正なし（ほかの補正もないので計画がない）。
    l.vignetting_scale = 0.0;
    assert_eq!(LensStagePlan::new(&r, &l), None);
    // 手動の周辺減光（+100、中点 0）: 隅（ρ = 1）で +1 段、中心で 0 段。
    let mut m = lens();
    m.vignetting_amount = 100.0;
    m.vignetting_midpoint = 0.0;
    let plan = LensStagePlan::new(&raw(400, 300, None, None), &m).unwrap();
    let g = plan.vignetting.as_ref().unwrap();
    assert!((g.eval(0.0) - 1.0).abs() < 1e-6);
    assert!((g.eval(1.0) - 2.0).abs() < 1e-3, "{}", g.eval(1.0));
    // 中点 100（ρ 0.9 から）では ρ = 0.8 はまだ 1。
    m.vignetting_midpoint = 100.0;
    let plan = LensStagePlan::new(&raw(400, 300, None, None), &m).unwrap();
    assert!((plan.vignetting.as_ref().unwrap().eval(0.64) - 1.0).abs() < 1e-6);
}

#[test]
fn stage_7_chromatic_aberration_profile_and_manual() {
    let r = raw(400, 300, None, Some(sony(Vec::new(), Vec::new(), 2000)));
    let mut l = lens();
    l.remove_chromatic_aberration = true;
    let plan = LensStagePlan::new(&r, &l).unwrap();
    let (mr, mb) = plan.chromatic_aberration.as_ref().unwrap();
    // 1 + 2000 · 2^−21（R）と 1 − 2000 · 2^−21（B）。
    assert!((f64::from(mr.eval(0.3)) - (1.0 + 2000.0 / 2_097_152.0)).abs() < 1e-6);
    assert!((f64::from(mb.eval(0.3)) - (1.0 - 2000.0 / 2_097_152.0)).abs() < 1e-6);
    assert!(plan.max_shift > 0.0 && plan.margin(1) >= 4);
    // 色収差の情報のない写真では、手動の値だけ。
    let mut m = lens();
    m.ca_red_cyan = 100.0;
    let plan = LensStagePlan::new(&raw(400, 300, None, None), &m).unwrap();
    let (mr, mb) = plan.chromatic_aberration.as_ref().unwrap();
    assert!((f64::from(mr.eval(0.5)) - (1.0 - MANUAL_CA_MAX)).abs() < 1e-7);
    assert_eq!(mb.eval(0.5), 1.0);
    // 最大の移動は対角線の半分 × 0.002 × 1.1（表の余裕）程度。
    let half_diag = 0.5 * 400f64.hypot(300.0);
    assert!(plan.max_shift > half_diag * MANUAL_CA_MAX * 0.99);
    assert!(plan.max_shift < half_diag * MANUAL_CA_MAX * 1.2);
}

/// 検証を通らない（細工した）DNG の係数でも、色収差の倍率と入力の範囲の余白は有限に収まる（検証の指摘:
/// G の面の係数が小さいと kR / kG が巨大になり、余白が巨大・`u32` の加算のあふれになっていた）。
#[test]
fn stage_7_margin_is_bounded_for_extreme_dng_coefficients() {
    for kg0 in [1e-5, 1e-12] {
        let plane = |k0: f64| DngWarpPlane {
            kr: [k0, 0.0, 0.0, 0.0],
            kt: [0.0; 2],
        };
        let data = LensCorrectionData::DngOpcode(DngLensCorrection {
            warp: Some(DngWarpRectilinear {
                planes: vec![plane(1.0), plane(kg0), plane(1.0)],
                center: [0.5, 0.5],
            }),
            vignette: None,
        });
        let r = raw(7040, 4688, None, Some(data));
        let mut l = lens();
        l.remove_chromatic_aberration = true;
        let plan = LensStagePlan::new(&r, &l).unwrap();
        let (mr, mb) = plan.chromatic_aberration.as_ref().unwrap();
        for m in mr.values().iter().chain(mb.values()) {
            assert!(
                (f64::from(*m) - 1.0).abs() <= MAX_CA_RATIO_DEVIATION + 1e-6,
                "{kg0}: {m}"
            );
        }
        let half_diag = 0.5 * 7040f64.hypot(4688.0);
        let bound = half_diag * LUT_RADIUS_MARGIN * MAX_CA_RATIO_DEVIATION;
        assert!(plan.max_shift <= bound * 1.01, "{kg0}: {}", plan.max_shift);
        assert!(
            plan.margin(1) <= bound.ceil() as u32 + 4,
            "{}",
            plan.margin(1)
        );
    }
    // 余白の計算そのものも、あふれない（飽和する）。
    let huge = LensStagePlan {
        max_shift: 1e30,
        ..LensStagePlan::new(
            &raw(400, 300, None, None),
            &LensCorrection {
                ca_red_cyan: 50.0,
                ..lens()
            },
        )
        .unwrap()
    };
    assert_eq!(huge.margin(1), u32::MAX);
}

#[test]
fn dng_vignette_and_three_plane_warp_for_stage_7() {
    let data = LensCorrectionData::DngOpcode(DngLensCorrection {
        warp: Some(DngWarpRectilinear {
            planes: vec![
                DngWarpPlane {
                    kr: [1.001, 0.0, 0.0, 0.0],
                    kt: [0.0; 2],
                },
                DngWarpPlane {
                    kr: [1.0, 0.0, 0.0, 0.0],
                    kt: [0.0; 2],
                },
                DngWarpPlane {
                    kr: [0.999, 0.0, 0.0, 0.0],
                    kt: [0.0; 2],
                },
            ],
            center: [0.5, 0.5],
        }),
        vignette: Some(DngVignetteRadial {
            k: [0.5, 0.0, 0.0, 0.0, 0.0],
            center: [0.5, 0.5],
        }),
    });
    let r = raw(400, 300, None, Some(data));
    let mut l = lens();
    l.profile_enabled = true;
    l.remove_chromatic_aberration = true;
    let plan = LensStagePlan::new(&r, &l).unwrap();
    assert_eq!(plan.center, (200.0, 150.0));
    let g = plan.vignetting.as_ref().unwrap();
    // 利得 1 + 0.5 · ρ²（ρ は最も遠い隅までの距離で割った値）。
    assert!((g.eval(1.0) - 1.5).abs() < 1e-5);
    let (mr, mb) = plan.chromatic_aberration.as_ref().unwrap();
    assert!((mr.eval(0.5) - 1.001).abs() < 1e-6);
    assert!((mb.eval(0.5) - 0.999).abs() < 1e-6);
}

/// 一様な画像（タイル）と、範囲 `roi` の出力。
fn uniform_tile(roi: Roi, value: [f32; 3]) -> ImageTile {
    ImageTile::from_image(
        roi,
        RgbImage::filled(roi.width, roi.height, ColorContract::B1CameraRgb, value).unwrap(),
    )
    .unwrap()
}

#[test]
fn applying_stage_7_brightens_corners_and_keeps_uniform_colors() {
    let mut m = lens();
    m.vignetting_amount = 100.0;
    m.ca_red_cyan = 60.0;
    m.ca_blue_yellow = -60.0;
    let plan = LensStagePlan::new(&raw(80, 60, None, None), &m).unwrap();
    let out_roi = Roi::full(80, 60);
    let input = uniform_tile(out_roi.expand(plan.margin(1)), [0.2, 0.3, 0.4]);
    let out = apply_lens_correction(&input, &plan, 1, (80, 60), out_roi).unwrap();
    assert_eq!(out.contract(), ColorContract::B1CameraRgb);
    let center = out.image.pixel(40, 30).unwrap();
    let corner = out.image.pixel(0, 0).unwrap();
    assert!(corner[1] > center[1] * 1.5, "{corner:?} {center:?}");
    // 一様な画像では、色収差の補正で色の比は変わらない（どこから取っても同じ値）。
    for p in [center, corner] {
        assert!((p[0] / p[1] - 0.2 / 0.3).abs() < 1e-5, "{p:?}");
        assert!((p[2] / p[1] - 0.4 / 0.3).abs() < 1e-5, "{p:?}");
    }
}

#[test]
fn stage_7_tiles_match_the_whole_image() {
    // 位置によって変わる画像で、タイルに分けて計算しても、全体で計算しても同じ値になる。
    let mut m = lens();
    m.vignetting_amount = -50.0;
    m.ca_red_cyan = 100.0;
    m.ca_blue_yellow = 100.0;
    let plan = LensStagePlan::new(&raw(90, 70, None, None), &m).unwrap();
    let margin = plan.margin(1);
    let whole_in = Roi::full(90, 70).expand(margin);
    let image = RgbImage::from_fn(
        whole_in.width,
        whole_in.height,
        ColorContract::B1CameraRgb,
        |x, y| {
            let (x, y) = (x as f32, y as f32);
            [
                0.1 + 0.01 * ((x * 0.37).sin() + 1.0),
                0.2 + 0.002 * y,
                0.3 + 0.01 * ((y * 0.29 + x * 0.1).cos() + 1.0),
            ]
        },
    )
    .unwrap();
    let input = ImageTile::from_image(whole_in, image).unwrap();
    let whole = apply_lens_correction(&input, &plan, 1, (90, 70), Roi::full(90, 70)).unwrap();
    for tile in Roi::tiles(90, 70, 32) {
        let sub_in = input.crop(tile.expand(margin)).unwrap();
        let part = apply_lens_correction(&sub_in, &plan, 1, (90, 70), tile).unwrap();
        for y in 0..tile.height {
            for x in 0..tile.width {
                let a = part.image.pixel(x, y).unwrap();
                let b = whole
                    .image
                    .pixel(x + tile.x as u32, y + tile.y as u32)
                    .unwrap();
                assert_eq!(a, b, "{tile:?} ({x}, {y})");
            }
        }
    }
}

#[test]
fn working_images_report_that_stage_7_is_not_used() {
    let mut s = genzo_model::DevelopSettings::default();
    assert!(crate::sensor::working_lens_warnings(&s).unwrap().is_empty());
    s.lens.vignetting_amount = 10.0;
    assert_eq!(
        crate::sensor::working_lens_warnings(&s).unwrap(),
        [crate::SensorWarning::LensSensorCorrectionsNotApplicable]
    );
    // 歪曲だけなら使える（警告しない）。版 1 では警告しない（未実装の項目として別に知らせる）。
    let mut d = genzo_model::DevelopSettings::default();
    d.lens.distortion = 10.0;
    assert!(crate::sensor::working_lens_warnings(&d).unwrap().is_empty());
    s.process_version = 1;
    assert!(crate::sensor::working_lens_warnings(&s).unwrap().is_empty());
}
