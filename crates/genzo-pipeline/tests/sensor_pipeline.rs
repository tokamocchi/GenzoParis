//! センサー処理（ステージ 2〜8）と段階 A1 のプレビューの結合テスト。
//!
//! genzo-testkit の合成データ（ベイヤー配列の RAW・ゾーンプレート・ColorChecker など）を入力にして、
//! docs/04_architecture.md の 2.1 節・2.2 節・2.6 節・2.7 節の決めごとを確かめる。
//!
//! 浮動小数点の比較は、同じ計算どうし（タイルと全体、帯の大きさ、スレッドの数）はビット単位で、
//! 基準値との比較は許容誤差を持たせる（Windows / macOS の数学ライブラリの差を含めないよう、
//! 合成データの sin・cos などは入力側だけで使う）。

use std::sync::Arc;

use genzo_color::space::{BT2020_TO_XYZ, XYZ_TO_BT2020};
use genzo_color::white_balance::white_point_from_temperature_tint;
use genzo_color::{Mat3, RgbColorSpace};
use genzo_model::{DevelopSettings, Orientation, PhotoMetadata, RenderQuality, WhiteBalance};
use genzo_pipeline::sensor::{
    ColorMatrixStage, NormalizeStage, SensorData, SensorStage, WhiteBalanceStage, run_sensor_stage,
};
use genzo_pipeline::{
    A1Image, ColorContract, DemosaicMethod, PipelineError, ProcessVersion, RgbImage, Roi,
    SaturationClip, SensorOptions, SensorPlan, SensorWarning, SourceImage, downscale_area,
    render_a1,
};
use genzo_raw::{CfaPattern, RawImage};
use genzo_testkit::bayer::{BayerParams, DEFAULT_WB, mosaic, working_to_camera};
use genzo_testkit::colorchecker::{ColorCheckerEdition, ColorCheckerLayout, color_checker_image};
use genzo_testkit::diff::{LabConversion, delta_e2000_stats};
use genzo_testkit::image::FloatImage;
use genzo_testkit::synth::{EdgeShape, LineOrientation, line_pattern, step_edge, zone_plate};

/// XYZ → BT.2020（カメラ RGB を BT.2020 とみなすカメラ行列。ステージ 8 が単位行列になる）。
fn bt2020_camera() -> [[f32; 3]; 3] {
    XYZ_TO_BT2020.rows().map(|r| r.map(|v| v as f32))
}

/// テスト用の典型的な形のカメラ行列（XYZ → カメラ RGB。実在の機種の値ではない）。
fn typical_camera() -> [[f32; 3]; 3] {
    [
        [0.70, -0.20, -0.06],
        [-0.50, 1.30, 0.23],
        [-0.07, 0.13, 0.65],
    ]
}

fn raw_from(scene: &FloatImage, cfa: CfaPattern, cam_xyz: Option<[[f32; 3]; 3]>) -> RawImage {
    mosaic(
        scene,
        &BayerParams {
            cfa,
            cam_xyz,
            ..Default::default()
        },
    )
    .unwrap()
}

fn to_float(img: &RgbImage) -> FloatImage {
    FloatImage::new(img.width(), img.height(), 3, img.as_flat().to_vec()).unwrap()
}

fn options(method: DemosaicMethod) -> SensorOptions {
    SensorOptions {
        demosaic: Some(method),
        ..Default::default()
    }
}

fn render_full(raw: &RawImage, opts: &SensorOptions) -> RgbImage {
    let plan = SensorPlan::new(raw, &DevelopSettings::default(), opts).unwrap();
    let (gw, gh) = plan.output_grid();
    let (tile, replaced) = plan.render_roi(raw, Roi::full(gw, gh)).unwrap();
    assert_eq!(replaced, 0);
    assert_eq!(tile.contract(), ColorContract::B2Working);
    tile.image
}

/// 周り `m` 画素を除いた内側。
fn interior(img: &FloatImage, m: u32) -> FloatImage {
    FloatImage::from_rgb_fn(img.width() - 2 * m, img.height() - 2 * m, |x, y| {
        let p = img.pixel(x + m, y + m).unwrap();
        [p[0], p[1], p[2]]
    })
    .unwrap()
}

/// PSNR（ピークを 1 とする。リニアの値のまま）。
fn psnr(expected: &FloatImage, actual: &FloatImage) -> f64 {
    let mut se = 0.0;
    for (a, b) in expected.data().iter().zip(actual.data()) {
        let d = f64::from(*a) - f64::from(*b);
        se += d * d;
    }
    let mse = se / expected.data().len() as f64;
    10.0 * (1.0 / mse).log10()
}

fn assert_close(a: [f32; 3], b: [f32; 3], tol: f32, what: &str) {
    for k in 0..3 {
        assert!((a[k] - b[k]).abs() <= tol, "{what}: {a:?} vs {b:?}");
    }
}

#[test]
fn gray_is_neutral_after_white_balance_for_every_pattern() {
    // カメラ RGB（B1）の 0.18 のグレーを逆 WB でモザイク化した RAW。撮影時の WB で中立に戻る。
    let scene = FloatImage::filled(40, 30, &[0.18; 3]).unwrap();
    let mut results = Vec::new();
    for cfa in CfaPattern::ALL {
        let raw = raw_from(&scene, cfa, Some(typical_camera()));
        assert_eq!(raw.as_shot_wb, DEFAULT_WB);
        let out = render_full(&raw, &SensorOptions::default());
        for p in out.data() {
            // 量子化（14bit）の誤差の範囲で中立かつ 0.18。
            assert_close(*p, [0.18; 3], 3e-4, &format!("{cfa}"));
            assert!((p[0] - p[1]).abs() < 3e-4 && (p[2] - p[1]).abs() < 3e-4);
        }
        results.push(out);
    }
    // 4 種の配置で同じ結果（平らな画像なので、位置による量子化の差もない）。
    for r in &results[1..] {
        for (a, b) in r.data().iter().zip(results[0].data()) {
            assert_close(*a, *b, 1e-6, "配置の違い");
        }
    }
}

#[test]
fn four_cfa_layouts_give_the_same_result_through_the_whole_pipeline() {
    // RGGB の RAW を (dx, dy) ずらして切り出すと、GRBG・GBRG・BGGR の RAW になる。同じ S の位置の
    // 結果は、切り出しの端から十分離れた内側ではビット単位で同じ。
    let scene = zone_plate(80, 72, 0.3, 0.05, 0.7).unwrap();
    let colored = FloatImage::from_rgb_fn(80, 72, |x, y| {
        let g = scene.pixel(x, y).unwrap()[0];
        [g * 1.4, g, g * 0.6 + 0.02 * (x % 7) as f32]
    })
    .unwrap();
    let base = raw_from(&colored, CfaPattern::RGGB, Some(typical_camera()));
    let base_out = render_full(&base, &SensorOptions::default());
    let mut patterns = Vec::new();
    for (dx, dy) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
        let (w, h) = (base.width - 2, base.height - 2);
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.push(base.get(x + dx, y + dy).unwrap());
            }
        }
        let cfa = base.cfa.shifted(dx as usize, dy as usize);
        patterns.push(cfa);
        let sub = RawImage {
            width: w,
            height: h,
            cfa,
            data,
            ..base.clone()
        };
        let out = render_full(&sub, &SensorOptions::default());
        // 端から 16 画素（RCD の余白 10 ＋ 切りそろえの半径 2 より広く）離れた内側。
        for y in 16..h - 16 {
            for x in 16..w - 16 {
                assert_eq!(
                    out.pixel(x, y),
                    base_out.pixel(x + dx, y + dy),
                    "{cfa} ({x}, {y})"
                );
            }
        }
    }
    patterns.sort_by_key(|p| p.name());
    patterns.dedup();
    assert_eq!(patterns.len(), 4);
}

/// RCD と双線形の、シーンとの差（PSNR と ΔE2000 の平均）。
fn demosaic_quality(scene: &FloatImage, cfa: CfaPattern) -> [(f64, f64); 2] {
    let raw = raw_from(scene, cfa, Some(bt2020_camera()));
    let margin = 12;
    let truth = interior(scene, margin);
    [DemosaicMethod::Rcd, DemosaicMethod::Bilinear].map(|m| {
        let opts = SensorOptions {
            demosaic: Some(m),
            saturation_clip: SaturationClip::Off,
            ..Default::default()
        };
        let out = interior(&to_float(&render_full(&raw, &opts)), margin);
        let de =
            delta_e2000_stats(&truth, &out, LabConversion::linear(RgbColorSpace::Bt2020)).unwrap();
        (psnr(&truth, &out), de.mean)
    })
}

#[test]
fn rcd_is_more_accurate_than_bilinear_on_synthetic_bayer_data() {
    let layout = ColorCheckerLayout {
        patch_size: 16,
        gap: 4,
    };
    let scenes = [
        (
            "ゾーンプレート",
            zone_plate(160, 160, 0.25, 0.05, 0.6).unwrap(),
        ),
        (
            "ゾーンプレート（ナイキスト）",
            zone_plate(160, 160, 0.5, 0.05, 0.6).unwrap(),
        ),
        (
            "ColorChecker",
            color_checker_image(
                ColorCheckerEdition::AfterNov2014,
                RgbColorSpace::Bt2020,
                layout,
                [0.02; 3],
                1.0,
            )
            .unwrap(),
        ),
        (
            "エッジ",
            step_edge(
                96,
                96,
                EdgeShape::Vertical { x: 47 },
                [0.02; 3],
                [0.8, 0.6, 0.4],
            )
            .unwrap(),
        ),
        (
            "灰色の縦線",
            line_pattern(96, 96, LineOrientation::Vertical, 4, 2, [0.1; 3], [0.5; 3]).unwrap(),
        ),
        (
            "灰色の横線",
            line_pattern(
                96,
                96,
                LineOrientation::Horizontal,
                3,
                1,
                [0.1; 3],
                [0.5; 3],
            )
            .unwrap(),
        ),
    ];
    for (name, scene) in &scenes {
        for cfa in [CfaPattern::RGGB, CfaPattern::GBRG] {
            let [(psnr_rcd, de_rcd), (psnr_bil, de_bil)] = demosaic_quality(scene, cfa);
            assert!(
                psnr_rcd > psnr_bil + 2.0,
                "{name} {cfa}: PSNR RCD {psnr_rcd:.2} / 双線形 {psnr_bil:.2}"
            );
            assert!(
                de_rcd < de_bil,
                "{name} {cfa}: ΔE2000 の平均 RCD {de_rcd:.3} / 双線形 {de_bil:.3}"
            );
        }
    }
    // 方向の判定が効いていれば、灰色の縦線・横線はほぼ完全に復元できる（量子化の誤差だけ）。
    let lines = &scenes[4].1;
    let [(psnr_rcd, de_rcd), _] = demosaic_quality(lines, CfaPattern::RGGB);
    assert!(psnr_rcd > 70.0 && de_rcd < 0.1, "{psnr_rcd} {de_rcd}");
}

#[test]
fn colors_are_reproduced_through_the_camera_matrix() {
    // 作業色空間（BT.2020）の ColorChecker → カメラ RGB → RAW → ステージ 2〜8 で、パッチの中央は
    // 元の色に戻る（量子化の誤差だけ）。
    let layout = ColorCheckerLayout {
        patch_size: 24,
        gap: 4,
    };
    let scene = color_checker_image(
        ColorCheckerEdition::AfterNov2014,
        RgbColorSpace::Bt2020,
        layout,
        [0.02; 3],
        0.9,
    )
    .unwrap();
    let cam = typical_camera();
    let camera_rgb = working_to_camera(&scene, &cam).unwrap();
    let raw = raw_from(&camera_rgb, CfaPattern::RGGB, Some(cam));
    let out = to_float(&render_full(&raw, &SensorOptions::default()));
    for i in 0..24 {
        // パッチの端から 9 画素以上離れた中央（RCD の推定が端の影響を受けない範囲）。
        let rect = layout.patch_rect(i).unwrap().inset(9).unwrap();
        let mean_out = out.region_mean(rect).unwrap();
        let mean_in = scene.region_mean(rect).unwrap();
        for k in 0..3 {
            assert!(
                (mean_out[k] - mean_in[k]).abs() < 2e-3,
                "パッチ {i}: {mean_out:?} vs {mean_in:?}"
            );
        }
    }
}

/// G だけが飽和する明るい中立の部分（B1 で 1.3）を含む画像。
fn highlight_scene() -> FloatImage {
    FloatImage::from_rgb_fn(48, 40, |x, _| if x < 24 { [0.3; 3] } else { [1.3; 3] }).unwrap()
}

#[test]
fn saturated_highlights_do_not_turn_magenta() {
    // カメラ行列は BT.2020（ステージ 8 が単位行列）にして、B1 の値をそのまま確かめる。
    let raw = raw_from(&highlight_scene(), CfaPattern::RGGB, Some(bt2020_camera()));
    // 既定の WB（R 2、B 1.5）で、B1 の 1.3 は G だけが白レベルを超える。
    let render = |clip| {
        render_full(
            &raw,
            &SensorOptions {
                saturation_clip: clip,
                ..Default::default()
            },
        )
    };
    let magenta = render(SaturationClip::Off);
    let p = magenta.pixel(40, 20).unwrap();
    assert!(
        p[1] < p[0] - 0.1 && p[1] < p[2] - 0.05,
        "切りそろえないとマゼンタ: {p:?}"
    );

    for clip in [SaturationClip::default(), SaturationClip::Global] {
        let out = render(clip);
        // 境目（x = 24）から 8 画素以上離れた明るい部分。
        for y in 0..40 {
            for x in 32..48 {
                let p = out.pixel(x, y).unwrap();
                // 中立（共通の白 = 1）。
                assert_close(p, [1.0; 3], 1e-4, &format!("{clip:?} ({x}, {y})"));
            }
        }
        // 飽和から離れた暗い部分は変わらない。
        assert_close(out.pixel(5, 20).unwrap(), [0.3; 3], 3e-4, "暗い部分");
    }
}

#[test]
fn normalization_uses_black_and_white_levels() {
    // 黒レベルちょうどの画素は 0、白レベル以上は飽和（共通の白）。
    let scene =
        FloatImage::from_rgb_fn(32, 24, |x, _| if x < 16 { [0.0; 3] } else { [5.0; 3] }).unwrap();
    let mut raw = raw_from(&scene, CfaPattern::BGGR, Some(bt2020_camera()));
    raw.black_level = [600.0, 512.0, 400.0, 520.0];
    // 黒の部分を位置ごとの黒レベルに合わせる。
    for y in 0..raw.height {
        for x in 0..16 {
            let k = raw.cfa.color_at(x as usize, y as usize).index();
            raw.data[(y * raw.width + x) as usize] = raw.black_level[k] as u16;
        }
    }
    let out = render_full(&raw, &SensorOptions::default());
    assert_close(out.pixel(3, 10).unwrap(), [0.0; 3], 0.0, "黒");
    // 白レベル以上はすべてのチャンネルで共通の白（WB の係数の最小値 = 1）。
    assert_close(out.pixel(28, 10).unwrap(), [1.0; 3], 1e-5, "白");
}

#[test]
fn tiles_and_out_of_range_rois_match_the_whole_image() {
    let scene = zone_plate(70, 52, 0.35, 0.02, 0.9).unwrap();
    let raw = raw_from(&scene, CfaPattern::GRBG, Some(typical_camera()));
    for method in DemosaicMethod::ALL {
        let plan = SensorPlan::new(&raw, &DevelopSettings::default(), &options(method)).unwrap();
        let (gw, gh) = plan.output_grid();
        let (full, _) = plan.render_roi(&raw, Roi::full(gw, gh)).unwrap();
        // タイルに分けても同じ（ビット単位）。
        for size in [8, 13, 32] {
            for tile in Roi::tiles(gw, gh, size) {
                let (t, _) = plan.render_roi(&raw, tile).unwrap();
                assert_eq!(t, full.crop(tile).unwrap(), "{method:?} {size} {tile:?}");
            }
        }
        // 画像の外にはみ出した範囲は、結果を鏡映した値。
        for roi in [
            Roi::new(-5, -3, 20, 10),
            Roi::new(gw as i32 - 4, gh as i32 - 6, 9, 11),
        ] {
            let (t, _) = plan.render_roi(&raw, roi).unwrap();
            assert_eq!(
                t,
                full.gather_mirror(roi, gw, gh).unwrap(),
                "{method:?} {roi:?}"
            );
        }
        assert!(plan.render_roi(&raw, Roi::new(0, 0, 0, 4)).is_err());
    }
}

#[test]
fn draft_2x2_preview_dimensions_and_values() {
    // 各位置の色ごとに一定の値（B1 で R 0.8、G 0.4、B 0.2）。
    for (w, h) in [(40, 30), (41, 31)] {
        let scene = FloatImage::filled(w, h, &[0.8, 0.4, 0.2]).unwrap();
        let raw = raw_from(&scene, CfaPattern::GBRG, Some(bt2020_camera()));
        let opts = SensorOptions {
            quality: RenderQuality::Draft,
            ..Default::default()
        };
        let plan = SensorPlan::new(&raw, &DevelopSettings::default(), &opts).unwrap();
        assert_eq!(plan.demosaic(), DemosaicMethod::Half2x2);
        assert_eq!(plan.output_grid(), (w / 2, h / 2));
        // プレビューの長辺がこの画像より大きいので、半分の格子のまま。
        let a1 = plan.render_preview(&raw, 2560).unwrap();
        assert_eq!(a1.image.dimensions(), (w / 2, h / 2));
        assert_eq!(a1.quality, RenderQuality::Draft);
        assert_eq!(
            a1.sensor_extent,
            (f64::from(w / 2 * 2), f64::from(h / 2 * 2))
        );
        assert_eq!(a1.sensor_pixels_per_pixel(), (2.0, 2.0));
        for p in a1.image.data() {
            assert_close(*p, [0.8, 0.4, 0.2], 2e-4, "2 × 2");
        }
        // 長辺 10px に縮小する。
        let small = plan.render_preview(&raw, 10).unwrap();
        assert_eq!(small.image.width().max(small.image.height()), 10);
        for p in small.image.data() {
            assert_close(*p, [0.8, 0.4, 0.2], 2e-4, "縮小した 2 × 2");
        }
    }
}

#[test]
fn preview_equals_downscaled_full_render_and_matches_testkit() {
    let scene = color_checker_image(
        ColorCheckerEdition::AfterNov2014,
        RgbColorSpace::Bt2020,
        ColorCheckerLayout {
            patch_size: 14,
            gap: 3,
        },
        [0.02; 3],
        1.0,
    )
    .unwrap();
    let raw = raw_from(&scene, CfaPattern::RGGB, Some(typical_camera()));
    let plan =
        SensorPlan::new(&raw, &DevelopSettings::default(), &SensorOptions::default()).unwrap();
    let (full, _) = plan
        .render_roi(&raw, Roi::full(raw.width, raw.height))
        .unwrap();
    for long_edge in [37, 64, 100] {
        let a1 = plan.render_preview(&raw, long_edge).unwrap();
        let (pw, ph) = a1.image.dimensions();
        assert_eq!(pw.max(ph), long_edge);
        // 帯ごとの処理と、全体を処理してから縮小した結果はビット単位で同じ。
        let whole = downscale_area(&full.image, pw, ph).unwrap();
        assert_eq!(a1.image, whole, "{long_edge}");
        // genzo-testkit の面積平均（f64 で計算）と、f32 の丸めの範囲で一致する。
        let reference =
            genzo_testkit::resample::downscale_area(&to_float(&full.image), pw, ph).unwrap();
        for (a, b) in a1.image.as_flat().iter().zip(reference.data()) {
            assert!((a - b).abs() <= 1e-6 + 1e-5 * b.abs(), "{a} vs {b}");
        }
        assert_eq!(
            a1.sensor_extent,
            (f64::from(raw.width), f64::from(raw.height))
        );
    }
}

#[test]
fn results_do_not_depend_on_the_number_of_threads() {
    let scene = zone_plate(150, 110, 0.4, 0.05, 0.8).unwrap();
    let raw = raw_from(&scene, CfaPattern::BGGR, Some(typical_camera()));
    let run = |threads: usize| -> A1Image {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let plan =
                SensorPlan::new(&raw, &DevelopSettings::default(), &SensorOptions::default())
                    .unwrap();
            plan.render_preview(&raw, 64).unwrap()
        })
    };
    let one = run(1);
    assert_eq!(one, run(3));
    assert_eq!(one, run(4));
}

#[test]
fn missing_camera_matrix_falls_back_to_bt2020_with_a_warning() {
    let scene = FloatImage::filled(24, 20, &[0.5, 0.25, 0.125]).unwrap();
    let raw = raw_from(&scene, CfaPattern::RGGB, None);
    let plan =
        SensorPlan::new(&raw, &DevelopSettings::default(), &SensorOptions::default()).unwrap();
    assert_eq!(plan.warnings(), [SensorWarning::MissingCameraMatrix]);
    assert!(plan.warnings()[0].to_string().contains("カメラ行列"));
    let a1 = plan.render_preview(&raw, 2560).unwrap();
    assert_eq!(a1.warnings, [SensorWarning::MissingCameraMatrix]);
    // ステージ 8 は単位行列（カメラ RGB をそのまま BT.2020 とみなす）。
    for p in a1.image.data() {
        assert_close(*p, [0.5, 0.25, 0.125], 2e-4, "単位行列");
    }
    // 色温度の WB も同じ仮定で計算できる。
    let custom = DevelopSettings {
        white_balance: WhiteBalance::Custom {
            temperature_k: 5000.0,
            tint: 10.0,
        },
        ..Default::default()
    };
    let plan = SensorPlan::new(&raw, &custom, &SensorOptions::default()).unwrap();
    assert_eq!(plan.wb().multipliers[1], 1.0);
    assert_eq!(plan.warnings(), [SensorWarning::MissingCameraMatrix]);
    // アプリのデータファイルの行列を渡せば警告はない。
    let opts = SensorOptions {
        camera_matrix: Some(Mat3::from_rows(typical_camera().map(|r| r.map(f64::from)))),
        ..Default::default()
    };
    let plan = SensorPlan::new(&raw, &DevelopSettings::default(), &opts).unwrap();
    assert!(plan.warnings().is_empty());
}

#[test]
fn camera_matrix_override_is_used() {
    let scene = FloatImage::filled(16, 16, &[0.3; 3]).unwrap();
    let raw = raw_from(&scene, CfaPattern::RGGB, Some(typical_camera()));
    let opts = SensorOptions {
        camera_matrix: Some(XYZ_TO_BT2020),
        ..Default::default()
    };
    let plan = SensorPlan::new(&raw, &DevelopSettings::default(), &opts).unwrap();
    assert_eq!(*plan.cam_xyz(), XYZ_TO_BT2020);
    let m = plan.camera_to_working().rows();
    for (i, row) in m.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            let e = if i == j { 1.0 } else { 0.0 };
            assert!((v - e).abs() < 1e-6, "{m:?}");
        }
    }
    let bad = SensorOptions {
        camera_matrix: Some(Mat3::from_rows([[1.0, 0.0, 0.0]; 3])),
        ..Default::default()
    };
    assert!(matches!(
        SensorPlan::new(&raw, &DevelopSettings::default(), &bad),
        Err(PipelineError::Color(_))
    ));
}

#[test]
fn process_versions_are_checked() {
    let scene = FloatImage::filled(8, 8, &[0.2; 3]).unwrap();
    let raw = raw_from(&scene, CfaPattern::RGGB, Some(typical_camera()));
    let plan =
        SensorPlan::new(&raw, &DevelopSettings::default(), &SensorOptions::default()).unwrap();
    assert_eq!(plan.process_version(), ProcessVersion::V1);
    assert_eq!(plan.context().process_version, ProcessVersion::V1);
    for v in [0, 2, 1000] {
        let s = DevelopSettings {
            process_version: v,
            ..Default::default()
        };
        assert!(matches!(
            SensorPlan::new(&raw, &s, &SensorOptions::default()),
            Err(PipelineError::UnsupportedProcessVersion { found, .. }) if found == v
        ));
        let working = SourceImage::Working {
            image: Arc::new(RgbImage::new(4, 4, ColorContract::B2Working).unwrap()),
            orientation: Orientation::Normal,
        };
        assert!(matches!(
            render_a1(&working, &s, &SensorOptions::default(), 2560),
            Err(PipelineError::UnsupportedProcessVersion { .. })
        ));
    }
}

#[test]
fn working_input_skips_sensor_stages_and_sanitizes() {
    let mut img = RgbImage::from_fn(30, 20, ColorContract::B2Working, |x, y| {
        [x as f32 * 0.125, y as f32 * 0.25, -0.125]
    })
    .unwrap();
    *img.pixel_mut(3, 4).unwrap() = [f32::NAN, f32::INFINITY, 0.5];
    let source = SourceImage::Working {
        image: Arc::new(img.clone()),
        orientation: Orientation::Rotate90Cw,
    };
    assert_eq!(source.sensor_size(), (30, 20));
    assert_eq!(source.orientation(), Orientation::Rotate90Cw);
    let a1 = render_a1(
        &source,
        &DevelopSettings::default(),
        &SensorOptions::default(),
        2560,
    )
    .unwrap();
    assert_eq!(a1.non_finite_replaced, 2);
    assert_eq!(a1.image.pixel(3, 4), Some([0.0, 0.0, 0.5]));
    // 負の値（色域外）は残す。向きは反映しない（S の向き）。
    assert_eq!(a1.image.pixel(10, 5), Some([1.25, 1.25, -0.125]));
    assert_eq!(a1.orientation, Orientation::Rotate90Cw);
    assert_eq!(a1.image.dimensions(), (30, 20));
    // 縮小。
    let small = render_a1(
        &source,
        &DevelopSettings::default(),
        &SensorOptions::default(),
        15,
    )
    .unwrap();
    assert_eq!(small.image.dimensions(), (15, 10));
    assert_eq!(small.sensor_pixels_per_pixel(), (2.0, 2.0));
    // B2 以外は受け付けない。
    img.set_contract(ColorContract::B1CameraRgb);
    let wrong = SourceImage::Working {
        image: Arc::new(img),
        orientation: Orientation::Normal,
    };
    assert!(matches!(
        render_a1(
            &wrong,
            &DevelopSettings::default(),
            &SensorOptions::default(),
            2560
        ),
        Err(PipelineError::ContractMismatch { .. })
    ));
}

#[test]
fn raw_source_keeps_the_sensor_orientation() {
    let scene = FloatImage::filled(40, 20, &[0.2; 3]).unwrap();
    let mut raw = raw_from(&scene, CfaPattern::RGGB, Some(typical_camera()));
    raw.metadata.orientation = Orientation::Rotate270Cw;
    let source = SourceImage::Raw(Arc::new(raw));
    let a1 = render_a1(
        &source,
        &DevelopSettings::default(),
        &SensorOptions::default(),
        20,
    )
    .unwrap();
    // 向きは反映せず（横長のまま）、向きの情報を渡す。
    assert_eq!(a1.image.dimensions(), (20, 10));
    assert_eq!(a1.orientation, Orientation::Rotate270Cw);
    assert_eq!(a1.frames().oriented_size(), (20, 40));
    assert_eq!(a1.sensor_size, (40, 20));
}

#[test]
fn stage_outputs_are_sanitized() {
    // NaN を含む CFA を WB のステージに通すと、出力の NaN は 0 になり、数が返る。
    let scene = FloatImage::filled(8, 8, &[0.2; 3]).unwrap();
    let raw = raw_from(&scene, CfaPattern::RGGB, Some(typical_camera()));
    let opts = SensorOptions {
        saturation_clip: SaturationClip::Off,
        ..Default::default()
    };
    let plan = SensorPlan::new(&raw, &DevelopSettings::default(), &opts).unwrap();
    let ctx = plan.context();
    let (cfa, n) = run_sensor_stage(
        &NormalizeStage,
        &ctx,
        &plan,
        SensorData::Raw(&raw),
        Roi::full(8, 8),
    )
    .unwrap();
    assert_eq!(n, 0);
    let SensorData::Cfa(mut cfa) = cfa else {
        panic!("CFA が出るはず");
    };
    cfa.data_mut()[5] = f32::NAN;
    cfa.data_mut()[9] = f32::INFINITY;
    let (out, n) = run_sensor_stage(
        &WhiteBalanceStage,
        &ctx,
        &plan,
        SensorData::Cfa(cfa),
        Roi::full(8, 8),
    )
    .unwrap();
    assert_eq!(n, 2);
    let SensorData::Cfa(out) = out else {
        panic!("CFA が出るはず");
    };
    assert_eq!(out.data()[5], 0.0);
    assert_eq!(out.data()[9], 0.0);
    // 入力の種類が違う。
    assert!(matches!(
        run_sensor_stage(
            &ColorMatrixStage,
            &ctx,
            &plan,
            SensorData::Cfa(out),
            Roi::full(8, 8)
        ),
        Err(PipelineError::SensorInputMismatch { .. })
    ));
    assert_eq!(ColorMatrixStage.stage_number(), 8);
}

#[test]
fn plan_and_raw_must_match() {
    let scene = FloatImage::filled(8, 8, &[0.2; 3]).unwrap();
    let raw = raw_from(&scene, CfaPattern::RGGB, Some(typical_camera()));
    let other = raw_from(&scene, CfaPattern::BGGR, Some(typical_camera()));
    let bigger = raw_from(
        &FloatImage::filled(10, 8, &[0.2; 3]).unwrap(),
        CfaPattern::RGGB,
        Some(typical_camera()),
    );
    let plan =
        SensorPlan::new(&raw, &DevelopSettings::default(), &SensorOptions::default()).unwrap();
    for r in [&other, &bigger] {
        assert!(matches!(
            plan.render_roi(r, Roi::full(8, 8)),
            Err(PipelineError::PlanMismatch(_))
        ));
        assert!(plan.render_preview(r, 4).is_err());
    }
    // 1 画素幅の RAW は扱わない。
    let thin = raw_from(
        &FloatImage::filled(1, 8, &[0.2; 3]).unwrap(),
        CfaPattern::RGGB,
        Some(typical_camera()),
    );
    assert!(matches!(
        SensorPlan::new(
            &thin,
            &DevelopSettings::default(),
            &SensorOptions::default()
        ),
        Err(PipelineError::InvalidDimensions { .. })
    ));
}

/// 物理的なモデルで RAW を作る（genzo-testkit の合成とは独立に、WB の係数と行列の向きを確かめる
/// ため）: 画素ごとのシーンの XYZ（D65 の白の Y = 1）に「XYZ → カメラ RGB」の行列 `cam` を掛けて
/// センサーの値とし、黒レベル・白レベルで u16 にする。撮影時の WB は D65 の白に対する係数
/// （`cam · 白` の逆数を G = 1 に正規化したもの。dcraw 以来の昼光の係数と同じ定義）。
fn physical_raw(w: u32, h: u32, cam: &Mat3, xyz: impl Fn(u32, u32) -> [f64; 3]) -> RawImage {
    let (black, white) = (512.0f64, 16383.0f64);
    let cfa = CfaPattern::RGGB;
    let mut data = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let c = cam.apply(xyz(x, y));
            let k = cfa.color_at(x as usize, y as usize).rgb_channel();
            data.push((black + c[k].clamp(0.0, 1.0) * (white - black)).round() as u16);
        }
    }
    let d65 = BT2020_TO_XYZ.apply([1.0; 3]);
    let wc = cam.apply(d65);
    RawImage {
        width: w,
        height: h,
        cfa,
        data,
        black_level: [black as f32; 4],
        white_level: white as f32,
        as_shot_wb: [(wc[1] / wc[0]) as f32, 1.0, (wc[1] / wc[2]) as f32, 1.0],
        cam_xyz: Some(cam.rows().map(|r| r.map(|v| v as f32))),
        metadata: PhotoMetadata::default(),
    }
}

#[test]
fn physical_camera_model_gives_the_scene_colors_in_bt2020() {
    // シーンの色（BT.2020 のリニアな値）→ XYZ → カメラ（行列をそのまま掛ける。行の尺度も含めて
    // センサーの感度とみなす）→ RAW → ステージ 2〜8。結果は、元の BT.2020 の色の定数倍
    // （倍率は D65 の白に対するカメラの G の応答。露出の尺度）に戻る（2.6 節のステージ 8 の方式で、
    // 撮影時の光源が D65 のとき）。
    let cam = Mat3::from_rows(typical_camera().map(|r| r.map(f64::from)));
    let colors = [
        [0.30, 0.10, 0.05],
        [0.05, 0.20, 0.10],
        [0.10, 0.10, 0.30],
        [0.18, 0.18, 0.18],
        [0.02, 0.25, 0.25],
    ];
    let s = 0.4;
    let raw = physical_raw(16 * colors.len() as u32, 16, &cam, |x, _| {
        BT2020_TO_XYZ.apply(colors[(x / 16) as usize].map(|v| v * s))
    });
    let scale = cam.apply(BT2020_TO_XYZ.apply([1.0; 3]))[1];
    let out = render_full(&raw, &SensorOptions::default());
    for (i, c) in colors.iter().enumerate() {
        let p = out.pixel(i as u32 * 16 + 8, 8).unwrap();
        let expect = c.map(|v| (v * s * scale) as f32);
        // 14bit の量子化の誤差（相対で 1e−3 程度）。
        assert_close(p, expect, 2e-4, &format!("色 {i}"));
    }
}

#[test]
fn custom_white_balance_neutralizes_the_light_source() {
    // 色温度・tint の光源の下の中立の物体（XYZ = 光源の白の色度）は、同じ色温度・tint の WB で
    // 中立（R = G = B）になる。カメラ行列で白の XYZ をカメラ RGB にして係数を作る（2.1 節の補足）。
    let cam = Mat3::from_rows(typical_camera().map(|r| r.map(f64::from)));
    for (t, tint) in [(3000.0f32, 0.0f32), (5000.0, 20.0), (7500.0, -15.0)] {
        let white = white_point_from_temperature_tint(f64::from(t), f64::from(tint))
            .unwrap()
            .to_xyz(1.0)
            .unwrap();
        let raw = physical_raw(32, 16, &cam, |_, _| white.map(|v| v * 0.3));
        let settings = DevelopSettings {
            white_balance: WhiteBalance::Custom {
                temperature_k: t,
                tint,
            },
            ..Default::default()
        };
        let plan = SensorPlan::new(&raw, &settings, &SensorOptions::default()).unwrap();
        let (tile, _) = plan.render_roi(&raw, Roi::full(32, 16)).unwrap();
        let p = tile.image.pixel(16, 8).unwrap();
        let g = p[1];
        assert!(g > 0.05, "{t} K: {p:?}");
        assert!(
            (p[0] - g).abs() < 2e-3 * g && (p[2] - g).abs() < 2e-3 * g,
            "{t} K・tint {tint}: {p:?}"
        );
        // 撮影時の WB（D65）のままでは中立にならない（光源の色が残る）。
        let as_shot =
            SensorPlan::new(&raw, &DevelopSettings::default(), &SensorOptions::default()).unwrap();
        let (tile, _) = as_shot.render_roi(&raw, Roi::full(32, 16)).unwrap();
        let q = tile.image.pixel(16, 8).unwrap();
        assert!(
            (q[0] - q[2]).abs() > 0.02 * q[1],
            "{t} K: 撮影時の WB では色が残るはず {q:?}"
        );
    }
}
