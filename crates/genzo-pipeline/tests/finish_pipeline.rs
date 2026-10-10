//! 仕上げのステージ（9〜17）とガイド（段階 B）の結合テスト。
//!
//! docs/04_architecture.md の 2.1 節・2.6 節・2.7 節、02 の IQ-02・IQ-07a・IQ-08 の決めごとを確かめる。
//!
//! 浮動小数点の比較は、同じ計算どうし（タイルと全体、スレッドの数、切り抜きの有無）はビット単位で、
//! 基準値との比較（ICC・既知の値）は許容誤差を持たせる（Windows / macOS の数学ライブラリの差を
//! 含めないよう、入力の模様は整数の演算だけで作る）。

use std::sync::Arc;

use genzo_color::space::to_working;
use genzo_color::{IccProfile, IccTransform, RenderingIntent, RgbColorSpace, StandardProfile};
use genzo_model::{
    CropRect, CurvePoint, DevelopSettings, Orientation, OutputColorSpace, RenderQuality,
};
use genzo_pipeline::finish::output::{Dither, Histogram, quantize_u8};
use genzo_pipeline::finish::{
    ColorStage, DisplayStage, ExportStage, ExposureStage, FinishPlan, GeometryStage,
    SceneToDisplayStage, ToneStage, finish_context,
};
use genzo_pipeline::guide::{GuideSpec, compute_guide, compute_guide_from};
use genzo_pipeline::stage::Guide;
use genzo_pipeline::{
    ColorContract, CoordinateFrames, ImageTile, PipelineError, RgbImage, Roi, SensorGrid,
    SensorOptions, SourceImage, Stage, StageContext, StageRegistry, render_a1, run_stage_cpu,
};

/// 整数の演算だけで作る、なめらかでない B2 の模様（暗部〜明部、色付き）。右下の 4 分の 1 は 4 倍明るい
/// （ハイライトの圧縮を通すため）。
fn scene(w: u32, h: u32) -> RgbImage {
    RgbImage::from_fn(w, h, ColorContract::B2Working, |x, y| {
        let k = |a: u32, b: u32, m: u32| ((x * a + y * b) % m) as f32 / m as f32;
        let gain = if x >= w / 2 && y >= h / 2 { 4.0 } else { 1.0 };
        [
            gain * (0.002 + 0.6 * k(37, 101, 97)),
            gain * (0.002 + 0.5 * k(53, 17, 89)),
            gain * (0.001 + 0.4 * k(11, 71, 83)),
        ]
    })
    .unwrap()
}

fn frames(img: &RgbImage, o: Orientation) -> CoordinateFrames {
    CoordinateFrames::new(img.width(), img.height(), o).unwrap()
}

fn context<'a>(settings: &DevelopSettings, img: &RgbImage, o: Orientation) -> StageContext<'a> {
    let f = frames(img, o);
    finish_context(
        settings,
        f,
        SensorGrid::full_resolution(&f),
        RenderQuality::Final,
    )
    .unwrap()
}

fn guide_for(settings: &DevelopSettings, img: &RgbImage, o: Orientation) -> Guide {
    let f = frames(img, o);
    compute_guide_from(
        img,
        SensorGrid::full_resolution(&f),
        f,
        &GuideSpec::from_settings(settings).unwrap(),
    )
    .unwrap()
}

fn tile(img: &RgbImage) -> ImageTile {
    ImageTile::from_image(Roi::full(img.width(), img.height()), img.clone()).unwrap()
}

/// ステージを 1 つ実行する。入力は、ステージの必要な範囲（画像の外にはみ出す部分は鏡映。2.7 節）を
/// 画像全体から取り出して渡す（エンジンのタイル処理と同じ形）。
fn run_one(
    stage: &dyn Stage,
    ctx: &StageContext<'_>,
    img: &RgbImage,
    out_roi: Roi,
    params: &genzo_pipeline::StageParams,
) -> ImageTile {
    let need = stage.input_roi(out_roi, params, ctx);
    let input = tile(img)
        .gather_mirror(need, img.width(), img.height())
        .unwrap();
    let (out, n) = run_stage_cpu(stage, ctx, &input, out_roi, params).unwrap();
    assert_eq!(n, 0);
    out
}

/// ガイドを作って、ステージ 9〜16 を画像全体に適用する（B3）。
fn render(settings: &DevelopSettings, img: &RgbImage, o: Orientation) -> (RgbImage, FinishPlan) {
    let guide = guide_for(settings, img, o);
    let ctx = context(settings, img, o).with_guide(&guide);
    let plan = FinishPlan::new(settings, &ctx).unwrap();
    let (out, replaced) = plan.render_full(&ctx, &tile(img)).unwrap();
    assert_eq!(replaced, 0, "NaN・無限大が出た");
    assert_eq!(out.contract(), ColorContract::B3DisplayReferred);
    (out.image, plan)
}

/// いろいろなステージが有効になる設定。
fn busy_settings() -> DevelopSettings {
    let mut s = DevelopSettings {
        exposure_ev: 0.4,
        contrast: 25.0,
        ..Default::default()
    };
    s.tone.highlights = -60.0;
    s.tone.shadows = 45.0;
    s.tone.whites = 10.0;
    s.tone.blacks = -15.0;
    s.color.saturation = 20.0;
    s.color.vibrance = 30.0;
    s.tone_curve.luma = vec![
        CurvePoint::new(0.0, 0.0),
        CurvePoint::new(0.3, 0.25),
        CurvePoint::new(0.7, 0.78),
        CurvePoint::new(1.0, 1.0),
    ];
    s.tone_curve.parametric.lights = 20.0;
    s
}

fn assert_bitwise_eq(a: &RgbImage, b: &RgbImage) {
    assert_eq!(a.dimensions(), b.dimensions());
    for (i, (p, q)) in a.data().iter().zip(b.data()).enumerate() {
        assert!(
            p.iter().zip(q).all(|(x, y)| x.to_bits() == y.to_bits()),
            "画素 {i}: {p:?} ≠ {q:?}"
        );
    }
}

#[test]
fn exposure_round_trip_iq02() {
    // 露光量のステージ単体で +EV の後に −EV をかけると元に戻る（IQ-02。非線形の処理を含めない）。
    let img = RgbImage::from_fn(13, 7, ColorContract::B2Working, |x, y| {
        let v = (x * 7 + y * 3) as f32 * 0.37 - 4.0;
        [v.exp2(), -0.01 * v, 1e-6 * (x as f32 + 1.0)]
    })
    .unwrap();
    let roi = Roi::full(13, 7);
    for ev in [3.0f32, 0.7, 5.0, 1.0 / 3.0] {
        let up = DevelopSettings {
            exposure_ev: ev,
            ..Default::default()
        };
        let down = DevelopSettings {
            exposure_ev: -ev,
            ..Default::default()
        };
        let ctx = context(&up, &img, Orientation::Normal);
        let p_up = ExposureStage.params(&up, &ctx).unwrap().unwrap();
        let p_down = ExposureStage.params(&down, &ctx).unwrap().unwrap();
        let (a, _) = run_stage_cpu(&ExposureStage, &ctx, &tile(&img), roi, &p_up).unwrap();
        let (b, _) = run_stage_cpu(&ExposureStage, &ctx, &a, roi, &p_down).unwrap();
        for (p, q) in img.data().iter().zip(b.image.data()) {
            for k in 0..3 {
                let rel = (p[k] - q[k]).abs() / p[k].abs().max(f32::MIN_POSITIVE);
                assert!(rel <= 1e-6, "EV {ev}: {p:?} → {q:?}");
            }
        }
        if ev == 3.0 {
            // 2 の冪なら厳密に戻る。
            assert_bitwise_eq(&img, &b.image);
        }
    }
    // 露光量 0 は飛ばす。
    let ctx = context(&DevelopSettings::default(), &img, Orientation::Normal);
    assert!(
        ExposureStage
            .params(&DevelopSettings::default(), &ctx)
            .unwrap()
            .is_none()
    );
}

#[test]
fn neutral_settings_do_almost_nothing() {
    // 既定の設定では、ステージ 9〜16 のうち有効なのはステージ 15 だけで、膝（0.5）より暗い色は
    // ビット単位で変わらない。
    let img = RgbImage::from_fn(31, 17, ColorContract::B2Working, |x, y| {
        let k = ((x * 13 + y * 7) % 41) as f32 / 41.0;
        [
            0.45 * k,
            0.45 * (1.0 - k),
            if x % 5 == 0 { -0.02 } else { 0.2 * k },
        ]
    })
    .unwrap();
    let s = DevelopSettings::default();
    let (out, plan) = render(&s, &img, Orientation::Normal);
    assert_eq!(plan.active_stage_ids(), [SceneToDisplayStage::ID]);
    assert_eq!(plan.output_size(), (31, 17));
    assert_bitwise_eq(&out, &img);
}

#[test]
fn crop_keeps_pixels_at_the_same_position() {
    let img = scene(64, 48);
    let base = busy_settings();
    let (full, full_plan) = render(&base, &img, Orientation::Normal);
    let mut cropped = base.clone();
    cropped.geometry.crop = CropRect {
        left: 0.25,
        top: 0.125,
        right: 0.75,
        bottom: 0.875,
    };
    let (part, plan) = render(&cropped, &img, Orientation::Normal);
    assert_eq!(plan.output_size(), (32, 36));
    assert_eq!(plan.geometry().crop_lattice(), Roi::new(16, 6, 32, 36));
    assert_eq!(full_plan.output_size(), (64, 48));
    for y in 0..36 {
        for x in 0..32 {
            assert_eq!(
                part.pixel(x, y).unwrap().map(f32::to_bits),
                full.pixel(x + 16, y + 6).unwrap().map(f32::to_bits),
                "({x}, {y})"
            );
        }
    }
}

#[test]
fn crop_with_rotation_and_orientation_keeps_pixels() {
    let img = scene(60, 40);
    let mut a = busy_settings();
    a.geometry.angle_deg = 6.0;
    a.geometry.crop = CropRect {
        left: 0.3,
        top: 0.3,
        right: 0.7,
        bottom: 0.7,
    };
    let mut b = a.clone();
    b.geometry.crop = CropRect {
        left: 0.35,
        top: 0.25,
        right: 0.62,
        bottom: 0.66,
    };
    for o in [
        Orientation::Normal,
        Orientation::Rotate90Cw,
        Orientation::Transverse,
    ] {
        let (ia, pa) = render(&a, &img, o);
        let (ib, pb) = render(&b, &img, o);
        // 切り抜きは画像の内側なので制限されない。
        assert_eq!(pa.geometry().crop(), a.geometry.crop);
        let ca = pa.geometry().crop_lattice();
        let cb = pb.geometry().crop_lattice();
        let both = ca.intersect(&cb).expect("重なる");
        assert!(both.pixel_count() > 50);
        for gy in both.y..both.y + both.height as i32 {
            for gx in both.x..both.x + both.width as i32 {
                let pa = ia.pixel((gx - ca.x) as u32, (gy - ca.y) as u32).unwrap();
                let pb = ib.pixel((gx - cb.x) as u32, (gy - cb.y) as u32).unwrap();
                assert_eq!(
                    pa.map(f32::to_bits),
                    pb.map(f32::to_bits),
                    "{o:?} ({gx}, {gy})"
                );
            }
        }
    }
}

#[test]
fn zero_rotation_reorders_pixels_exactly() {
    // 回転 0・切り抜きなしなら、ステージ 9 は向きに合わせた画素の並べ替えだけ（補間が恒等）。
    let img = scene(9, 6);
    let s = DevelopSettings::default();
    for o in [
        Orientation::Normal,
        Orientation::FlipHorizontal,
        Orientation::Rotate180,
        Orientation::FlipVertical,
        Orientation::Transpose,
        Orientation::Rotate90Cw,
        Orientation::Transverse,
        Orientation::Rotate270Cw,
    ] {
        let ctx = context(&s, &img, o);
        let f = ctx.frames;
        let (ow, oh) = f.oriented_size();
        let roi = Roi::full(ow, oh);
        let out = match GeometryStage.params(&s, &ctx).unwrap() {
            None => {
                assert_eq!(o, Orientation::Normal);
                continue;
            }
            Some(p) => run_one(&GeometryStage, &ctx, &img, roi, &p),
        };
        for v in 0..oh {
            for u in 0..ow {
                let (x, y) = f.oriented_pixel_to_sensor(u, v).unwrap();
                assert_eq!(
                    out.image.pixel(u, v).unwrap().map(f32::to_bits),
                    img.pixel(x, y).unwrap().map(f32::to_bits),
                    "{o:?} ({u}, {v})"
                );
            }
        }
    }
}

#[test]
fn rotation_follows_the_inverse_mapping() {
    // 1 次関数の模様はバイキュービック（Catmull-Rom）で厳密に再現されるので、内側の画素は
    // 逆写像した位置の値と一致する。正の角度は画面上の反時計回り。
    let f_val = |x: f64, y: f64| 0.1 + 0.01 * x + 0.002 * y;
    let img = RgbImage::from_fn(80, 60, ColorContract::B2Working, |x, y| {
        let v = f_val(f64::from(x), f64::from(y)) as f32;
        [v, v * 0.5, v * 0.25]
    })
    .unwrap();
    for deg in [20.0f32, -20.0] {
        let mut s = DevelopSettings::default();
        s.geometry.angle_deg = deg;
        let ctx = context(&s, &img, Orientation::Normal);
        let p = GeometryStage.params(&s, &ctx).unwrap().unwrap();
        let plan = p
            .get::<genzo_pipeline::finish::GeometryPlan>()
            .unwrap()
            .clone();
        let (w, h) = plan.output_size();
        let out = run_one(&GeometryStage, &ctx, &img, Roi::full(w, h), &p);
        let mut checked = 0;
        for j in 0..h {
            for i in 0..w {
                let (x, y) = plan.c_to_input(f64::from(i) + 0.5, f64::from(j) + 0.5);
                // 補間のタップが画像の内側にある画素だけ（端は鏡映なので 1 次関数にならない）。
                if !(2.0..78.0).contains(&x) || !(2.0..58.0).contains(&y) {
                    continue;
                }
                let expect = f_val(x - 0.5, y - 0.5) as f32;
                let got = out.image.pixel(i, j).unwrap()[0];
                assert!(
                    (got - expect).abs() < 2e-5,
                    "{deg}: ({i}, {j}) {got} {expect}"
                );
                checked += 1;
            }
        }
        assert!(checked > (w * h / 2) as usize, "{checked}");
        // 向き: 正の角度では、画面の下へ進むと値が減る（右向きの勾配が反時計回りに回る）。
        let (cx, cy) = (w / 2, h / 2);
        let down = out.image.pixel(cx, cy + 3).unwrap()[0] - out.image.pixel(cx, cy).unwrap()[0];
        if deg > 0.0 {
            assert!(down < 0.0, "{down}");
        } else {
            assert!(down > 0.0, "{down}");
        }
    }
}

#[test]
fn full_crop_with_rotation_is_constrained_inside_the_image() {
    let img = scene(60, 40);
    let mut s = DevelopSettings::default();
    s.geometry.angle_deg = 15.0;
    let (out, plan) = render(&s, &img, Orientation::Normal);
    let c = plan.geometry().crop();
    assert!(c.width() < 1.0 && c.height() < 1.0, "{c:?}");
    assert_eq!(out.dimensions(), plan.output_size());
    // 出力の四隅の画素の中心は画像の内側に写る（外側を鏡映で埋めていない）。
    let (w, h) = plan.output_size();
    for (i, j) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
        let (x, y) = plan
            .geometry()
            .c_to_input(f64::from(i) + 0.5, f64::from(j) + 0.5);
        assert!(
            (-0.5..=60.5).contains(&x) && (-0.5..=40.5).contains(&y),
            "{x} {y}"
        );
    }
}

#[test]
fn guide_is_invariant_to_exposure_crop_and_sliders() {
    let source = SourceImage::Working {
        image: Arc::new(scene(120, 80)),
        orientation: Orientation::Rotate270Cw,
    };
    let base = DevelopSettings::default();
    let a1 = render_a1(&source, &base, &SensorOptions::default(), 64).unwrap();
    let g0 = compute_guide(&a1, &GuideSpec::from_settings(&base).unwrap()).unwrap();
    let mut changed = busy_settings();
    changed.exposure_ev = 2.5;
    changed.geometry.crop.left = 0.2;
    assert_eq!(
        GuideSpec::from_settings(&changed).unwrap(),
        GuideSpec::from_settings(&base).unwrap()
    );
    assert_eq!(
        GuideSpec::develop_hash(&changed),
        GuideSpec::develop_hash(&base)
    );
    let g1 = compute_guide(&a1, &GuideSpec::from_settings(&changed).unwrap()).unwrap();
    assert_eq!(g0, g1);
    // 向きが反映される（G は縦長）。G の長辺が 512 より短いので、G の画素数と同じ寸法。
    assert_eq!((g0.image.width(), g0.image.height()), (80, 120));
    // 回転を変えるとガイドもハッシュも変わる。
    let mut rotated = base.clone();
    rotated.geometry.angle_deg = 4.0;
    assert_ne!(
        GuideSpec::develop_hash(&rotated),
        GuideSpec::develop_hash(&base)
    );
    let g2 = compute_guide(&a1, &GuideSpec::from_settings(&rotated).unwrap()).unwrap();
    assert_ne!(g2, g0);
    assert_eq!(g2.frames.rotation_deg, 4.0);
}

#[test]
fn tone_requires_a_matching_guide() {
    let img = scene(32, 24);
    let mut s = DevelopSettings::default();
    s.tone.shadows = 50.0;
    // ガイドがない。
    let ctx = context(&s, &img, Orientation::Normal);
    assert!(matches!(
        ToneStage.params(&s, &ctx),
        Err(PipelineError::GuideMismatch { .. })
    ));
    // 回転の違うガイド。
    let mut rotated = s.clone();
    rotated.geometry.angle_deg = 3.0;
    let stale = guide_for(&rotated, &img, Orientation::Normal);
    let ctx = context(&s, &img, Orientation::Normal).with_guide(&stale);
    assert!(matches!(
        FinishPlan::new(&s, &ctx),
        Err(PipelineError::GuideMismatch { .. })
    ));
    // 文脈の回転と設定の角度が違う。
    let ctx = context(&DevelopSettings::default(), &img, Orientation::Normal);
    assert!(matches!(
        FinishPlan::new(&rotated, &ctx),
        Err(PipelineError::InvalidArgument(_))
    ));
}

#[test]
fn tiles_match_the_full_render() {
    // タイルに分けても、分けない処理とビット単位で一致する（画素ごとの計算は全体の座標だけに依存）。
    let img = scene(70, 50);
    let mut s = busy_settings();
    s.geometry.angle_deg = -7.0;
    s.geometry.crop = CropRect {
        left: 0.1,
        top: 0.15,
        right: 0.85,
        bottom: 0.9,
    };
    let guide = guide_for(&s, &img, Orientation::Rotate90Cw);
    let ctx = context(&s, &img, Orientation::Rotate90Cw).with_guide(&guide);
    let plan = FinishPlan::new(&s, &ctx).unwrap();
    assert!(plan.active_stage_ids().contains(&GeometryStage::ID));
    assert!(plan.active_stage_ids().contains(&ToneStage::ID));
    assert!(plan.active_stage_ids().contains(&ColorStage::ID));
    let source = tile(&img);
    let (full, _) = plan.render_full(&ctx, &source).unwrap();
    let (w, h) = plan.output_size();
    for size in [7u32, 16, 33] {
        for t in Roi::tiles(w, h, size) {
            // タイルに必要な入力だけを切り出して渡す（エンジンのタイル処理と同じ形）。
            let need = plan.source_roi(&ctx, t);
            let part = source
                .gather_mirror(need, img.width(), img.height())
                .unwrap();
            let (out, _) = plan.render_roi(&ctx, &part, t).unwrap();
            for y in 0..t.height {
                for x in 0..t.width {
                    let a = out.image.pixel(x, y).unwrap();
                    let b = full.image.pixel(t.x as u32 + x, t.y as u32 + y).unwrap();
                    assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits), "{size} {t:?}");
                }
            }
        }
    }
}

#[test]
fn thread_count_does_not_change_results() {
    let img = scene(90, 64);
    let mut s = busy_settings();
    s.geometry.angle_deg = 11.0;
    let run = || render(&s, &img, Orientation::Normal).0;
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(run);
    let four = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap()
        .install(run);
    assert_bitwise_eq(&one, &four);
}

#[test]
fn negative_and_extreme_values_flow_without_nan() {
    let img = RgbImage::from_fn(24, 16, ColorContract::B2Working, |x, y| match (x + y) % 6 {
        0 => [-0.3, 0.5, 0.1],
        1 => [0.0, 0.0, 0.0],
        2 => [1e6, 2e5, 1e4],
        3 => [-1e-9, -2e-9, 0.0],
        4 => [3.0, -0.5, 8.0],
        _ => [0.18, 0.18, 0.18],
    })
    .unwrap();
    let mut s = busy_settings();
    s.color.saturation = 100.0;
    s.color.vibrance = 100.0;
    s.contrast = 100.0;
    s.tone.whites = 100.0;
    let (out, _) = render(&s, &img, Orientation::Normal);
    assert!(out.as_flat().iter().all(|v| v.is_finite()));
    // B3: 輝度は 1 以下。
    for p in out.data() {
        let y = genzo_pipeline::finish::luminance(*p);
        assert!(y <= 1.0 + 1e-5, "{p:?}");
    }
    // 書き出しは 0〜1。
    let ctx = context(&s, &img, Orientation::Normal);
    let b3 = tile(&out);
    for stage in ExportStage::ALL {
        let p = stage.params(&s, &ctx).unwrap().unwrap();
        let (e, n) = run_stage_cpu(&stage, &ctx, &b3, b3.roi, &p).unwrap();
        assert_eq!(n, 0);
        assert!(e.image.as_flat().iter().all(|v| (0.0..=1.0).contains(v)));
    }
}

#[test]
fn export_matches_the_icc_profiles() {
    // 書き出した値を、その色空間の ICC プロファイル（lcms2）で作業色空間に戻すと元の色になる
    // （色域の圧縮の膝より内側の色。IQ-06 の前提）。
    let linear = RgbColorSpace::Bt2020;
    let back_profile = IccProfile::standard(StandardProfile::LinearBt2020).unwrap();
    for (space, profile, stage) in [
        (
            RgbColorSpace::Srgb,
            StandardProfile::Srgb,
            ExportStage::SRGB,
        ),
        (
            RgbColorSpace::DisplayP3,
            StandardProfile::DisplayP3,
            ExportStage::DISPLAY_P3,
        ),
        (
            RgbColorSpace::AdobeRgb1998,
            StandardProfile::AdobeRgb1998,
            ExportStage::ADOBE_RGB,
        ),
    ] {
        assert_eq!(stage.space(), space);
        // 出力の色空間で穏やかな色を作り、BT.2020 にする。
        let colors_out = [
            [0.2f32, 0.4, 0.3],
            [0.5, 0.45, 0.4],
            [0.05, 0.06, 0.1],
            [0.7, 0.6, 0.65],
        ];
        let m = to_working(space);
        let b3: Vec<[f32; 3]> = colors_out
            .iter()
            .map(|c| {
                let v = m.apply([f64::from(c[0]), f64::from(c[1]), f64::from(c[2])]);
                [v[0] as f32, v[1] as f32, v[2] as f32]
            })
            .collect();
        let img = RgbImage::from_vec(4, 1, ColorContract::B3DisplayReferred, b3.clone()).unwrap();
        let ctx = context(&DevelopSettings::default(), &img, Orientation::Normal);
        let p = stage
            .params(&DevelopSettings::default(), &ctx)
            .unwrap()
            .unwrap();
        let (enc, _) = run_stage_cpu(&stage, &ctx, &tile(&img), Roi::full(4, 1), &p).unwrap();
        assert_eq!(enc.contract(), ColorContract::B4bExport);
        let src = IccProfile::standard(profile).unwrap();
        let t =
            IccTransform::new(&src, &back_profile, RenderingIntent::RelativeColorimetric).unwrap();
        let mut back = vec![[0.0f32; 3]; 4];
        t.transform(enc.image.data(), &mut back).unwrap();
        for (orig, got) in b3.iter().zip(&back) {
            for k in 0..3 {
                assert!(
                    (orig[k] - got[k]).abs() < 2e-3,
                    "{space:?}: {orig:?} → {got:?}"
                );
            }
        }
        let _ = linear;
    }
}

#[test]
fn srgb_middle_gray_is_118() {
    // シーンの 18% グレーは、既定の設定でディスプレイの 0.18 になり（ステージ 15 の中間グレーの
    // 対応）、sRGB の 8bit で 118（IEC 61966-2-1: 0.18 → 0.4614 → 117.65 → 118）。
    let img = RgbImage::filled(16, 16, ColorContract::B2Working, [0.18; 3]).unwrap();
    let s = DevelopSettings::default();
    let (b3, _) = render(&s, &img, Orientation::Normal);
    let ctx = context(&s, &img, Orientation::Normal);
    let stage = ExportStage::for_space(OutputColorSpace::Srgb);
    let p = stage.params(&s, &ctx).unwrap().unwrap();
    let (enc, _) = run_stage_cpu(stage, &ctx, &tile(&b3), Roi::full(16, 16), &p).unwrap();
    let q = quantize_u8(&enc.image, (0, 0), Dither::None).unwrap();
    assert!(q.iter().all(|&v| v == 118), "{:?}", &q[..6]);
    let d = quantize_u8(&enc.image, (0, 0), Dither::DEFAULT_8BIT).unwrap();
    assert!(d.iter().all(|&v| (117..=119).contains(&v)));
    let mean = d.iter().map(|&v| f64::from(v)).sum::<f64>() / d.len() as f64;
    assert!((mean - 117.65).abs() < 0.2, "{mean}");
    // 同じ入力なら同じ結果（IQ-08）。
    assert_eq!(
        d,
        quantize_u8(&enc.image, (0, 0), Dither::DEFAULT_8BIT).unwrap()
    );
    // ヒストグラムの合計は画素数。
    let h = Histogram::compute(
        &enc.image,
        Histogram::luma_coefficients(RgbColorSpace::Srgb),
    )
    .unwrap();
    assert_eq!(h.pixel_count, 256);
    assert_eq!(h.red[118], 256);
    assert_eq!(h.luma.iter().sum::<u64>(), 256);
    assert_eq!(h.clipped_high.any, 0);
    assert_eq!(h.clipped_low.any, 0);
}

#[test]
fn display_stage_with_assumed_srgb() {
    let img = RgbImage::from_vec(
        3,
        1,
        ColorContract::B3DisplayReferred,
        vec![[0.18; 3], [1.0; 3], [0.0; 3]],
    )
    .unwrap();
    let s = DevelopSettings::default();
    let ctx = context(&s, &img, Orientation::Normal);
    let stage = DisplayStage::default();
    let p = stage.params(&s, &ctx).unwrap().unwrap();
    assert!(stage.transform().unwrap().assumed_srgb);
    let (out, _) = run_stage_cpu(&stage, &ctx, &tile(&img), Roi::full(3, 1), &p).unwrap();
    assert_eq!(out.contract(), ColorContract::B4aDisplay);
    let g = out.image.pixel(0, 0).unwrap();
    assert!(g.iter().all(|v| (v - 0.461_356).abs() < 3e-3), "{g:?}");
    let w = out.image.pixel(1, 0).unwrap();
    assert!(w.iter().all(|v| (v - 1.0).abs() < 3e-3), "{w:?}");
    let b = out.image.pixel(2, 0).unwrap();
    assert!(b.iter().all(|v| v.abs() < 3e-3), "{b:?}");
}

#[test]
fn a1_preview_end_to_end() {
    // JPEG などの入力（B2）→ 段階 A1（長辺 64）→ 段階 B（ガイド）→ 段階 C（9〜16）→ 17b → 量子化
    // → ヒストグラム。
    let source = SourceImage::Working {
        image: Arc::new(scene(150, 100)),
        orientation: Orientation::Rotate90Cw,
    };
    let mut s = busy_settings();
    s.geometry.angle_deg = 3.5;
    s.geometry.crop = CropRect {
        left: 0.1,
        top: 0.1,
        right: 0.9,
        bottom: 0.8,
    };
    let a1 = render_a1(&source, &s, &SensorOptions::default(), 64).unwrap();
    let guide = compute_guide(&a1, &GuideSpec::from_settings(&s).unwrap()).unwrap();
    let ctx = a1.finish_context(&s).unwrap().with_guide(&guide);
    assert_eq!(ctx.frames.rotation_deg, 3.5);
    assert!((ctx.resolution_scale - 64.0 / 150.0).abs() < 1e-12);
    let plan = FinishPlan::new(&s, &ctx).unwrap();
    let (b3, n) = plan
        .render_full(
            &ctx,
            &ImageTile::from_image(Roi::full(64, 43), a1.image.clone()).unwrap(),
        )
        .unwrap();
    assert_eq!(n, 0);
    let (w, h) = plan.output_size();
    // 縦長（向きで 90 度回る）で、切り抜きで小さくなる。
    assert!(h > w, "{w} {h}");
    assert!(w < 43 && h < 64);
    let stage = ExportStage::DISPLAY_P3;
    let p = stage.params(&s, &ctx).unwrap().unwrap();
    let (enc, _) = run_stage_cpu(&stage, &ctx, &b3, b3.roi, &p).unwrap();
    let q = quantize_u8(&enc.image, (0, 0), Dither::DEFAULT_8BIT).unwrap();
    assert_eq!(q.len(), (w * h * 3) as usize);
    let hist = Histogram::compute(
        &enc.image,
        Histogram::luma_coefficients(RgbColorSpace::DisplayP3),
    )
    .unwrap();
    assert_eq!(hist.pixel_count, u64::from(w * h));
    for ch in [&hist.red, &hist.green, &hist.blue, &hist.luma] {
        assert_eq!(ch.iter().sum::<u64>(), u64::from(w * h));
    }
    // キャッシュ（B5）も同じ値で、契約だけが違う。
    let cache = ExportStage::CACHE_DISPLAY_P3;
    let pc = cache.params(&s, &ctx).unwrap().unwrap();
    let (c, _) = run_stage_cpu(&cache, &ctx, &b3, b3.roi, &pc).unwrap();
    assert_eq!(c.contract(), ColorContract::B5Cache);
    assert_eq!(c.image.data(), enc.image.data());
}

#[test]
fn unknown_process_versions_are_rejected() {
    let img = scene(8, 8);
    let s = DevelopSettings {
        process_version: 2,
        ..Default::default()
    };
    let f = frames(&img, Orientation::Normal);
    assert!(matches!(
        finish_context(&s, f, SensorGrid::full_resolution(&f), RenderQuality::Final),
        Err(PipelineError::UnsupportedProcessVersion { found: 2, .. })
    ));
    assert!(GuideSpec::from_settings(&s).is_err());
}

#[test]
fn every_registered_stage_accepts_default_settings() {
    let img = scene(8, 8);
    let s = DevelopSettings::default();
    let ctx = context(&s, &img, Orientation::Normal);
    let r = StageRegistry::builtin();
    for stage in r.stages() {
        let p = stage.params(&s, &ctx).unwrap();
        // 既定の設定で有効なのは、ステージ 15 と出力（17a・17b）だけ。
        let expect = stage.id() == SceneToDisplayStage::ID || stage.id().starts_with("output.");
        assert_eq!(p.is_some(), expect, "{}", stage.id());
    }
}

#[test]
fn tiny_images_work_with_every_stage() {
    // 1 × 1・2 × 1・1 × 3 の画像でも、回転・切り抜き・ガイド・すべてのステージが動く（境界の丸め）。
    for (w, h) in [(1u32, 1u32), (2, 1), (1, 3), (3, 2)] {
        let img = scene(w, h);
        for o in [Orientation::Normal, Orientation::Rotate90Cw] {
            for deg in [0.0f32, 30.0, -45.0] {
                let mut s = busy_settings();
                s.geometry.angle_deg = deg;
                s.geometry.crop = CropRect {
                    left: 0.2,
                    top: 0.0,
                    right: 0.9,
                    bottom: 1.0,
                };
                let (out, plan) = render(&s, &img, o);
                let (ow, oh) = plan.output_size();
                assert!(ow >= 1 && oh >= 1);
                assert_eq!(out.dimensions(), (ow, oh));
                assert!(
                    out.as_flat().iter().all(|v| v.is_finite()),
                    "{w}x{h} {o:?} {deg}"
                );
            }
        }
    }
}
