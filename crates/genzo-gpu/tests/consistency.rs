//! CPU 版と GPU 版の一致テスト（02 の IQ-07a、docs/04_architecture.md の 2.3 節・7.1 節「ステージを
//! 登録すると自動で対象になる」）。
//!
//! genzo-pipeline の登録表（`StageRegistry::builtin`）をたどり、GPU 版があるステージ（この crate の
//! `GpuStageRegistry` に ID があるもの）をすべて、同じ入力・同じパラメータで CPU 版と GPU 版の両方で
//! 実行し、ステージごとの許容誤差（`genzo_gpu::stage_tolerance`。仮置き）以内であることを確かめる。
//! 新しいステージの GPU 版を登録すると、このテストの対象になる（有効になる設定がテストになければ失敗
//! して知らせる）。
//!
//! GPU のアダプターがない環境と CI のランナーではスキップする（`GENZO_REQUIRE_GPU=1` なら実行し、
//! アダプターがなければ失敗。tests/common の doc）。

mod common;

use genzo_gpu::{GpuRenderer, compare, compare_pixels, stage_tolerance};
use genzo_model::{CropRect, DevelopSettings, Orientation, RenderQuality};
use genzo_pipeline::engine::StageBackend;
use genzo_pipeline::guide::{GuideSpec, compute_guide};
use genzo_pipeline::sensor::{SensorData, SensorPlan, run_sensor_stage};
use genzo_pipeline::{
    ColorContract, ImageTile, ProcessVersion, RgbImage, Roi, SaturationClip, SensorOptions, Stage,
    StageContext, StageRegistry, render_a1, run_stage_cpu,
};
use genzo_raw::CfaPattern;

use common::*;

/// RGB のステージを有効にする設定（回転・切り抜きでステージ 9 も有効にする）。
fn settings_for_all_stages() -> DevelopSettings {
    let mut s = strong_settings();
    rotate_and_crop(
        &mut s,
        4.0,
        CropRect {
            left: 0.08,
            top: 0.1,
            right: 0.9,
            bottom: 0.88,
        },
    );
    s
}

/// 段階 C の文脈（96 × 64 の B2 の画像を A1 とし、ガイドを付ける）を作って `f` を呼ぶ。
fn with_context(settings: &DevelopSettings, f: impl FnOnce(&StageContext<'_>, &RgbImage)) {
    let image = scene_b2(96, 64);
    let source = working_source(image, Orientation::Rotate90Cw).image;
    let a1 = render_a1(&source, settings, &SensorOptions::default(), 96).unwrap();
    let guide = compute_guide(&a1, &GuideSpec::from_settings(settings).unwrap()).unwrap();
    let ctx = a1.finish_context(settings).unwrap().with_guide(&guide);
    f(&ctx, &a1.image);
}

/// 画像全体のタイルから範囲 `roi`（外は鏡映）を取り出す。
fn tile_of(image: &RgbImage, roi: Roi) -> ImageTile {
    ImageTile::from_image(Roi::full(image.width(), image.height()), image.clone())
        .unwrap()
        .gather_mirror(roi, image.width(), image.height())
        .unwrap()
}

/// ステージの入力（契約に合わせた合成の画像）と出力の範囲。
fn case_for(
    stage: &dyn Stage,
    a1_image: &RgbImage,
    ctx: &StageContext<'_>,
    settings: &DevelopSettings,
) -> (ImageTile, Roi) {
    let params = stage.params(settings, ctx).unwrap().unwrap();
    if stage.id() == "finish.geometry" {
        // 出力（C の格子）の一部の範囲と、それに必要な入力（S の格子。外は鏡映）。
        let plan = params
            .get::<genzo_pipeline::finish::GeometryPlan>()
            .unwrap();
        let (w, h) = plan.output_size();
        let out = Roi::new(2, 3, w - 4, h - 5);
        let need = stage.input_roi(out, &params, ctx);
        return (tile_of(a1_image, need), out);
    }
    // 画素ごとのステージ: 出力の範囲を全体の格子の途中に置き、入力は少し広くする（位置の扱いの確認）。
    let out = Roi::new(5, 4, 70, 50);
    let input_roi = out.expand(2);
    let image = match stage.input_contract() {
        ColorContract::B2Working => scene_b2(80, 60),
        ColorContract::B3DisplayReferred => scene_b3(80, 60),
        c => panic!("{}: 想定していない入力の契約 {c}", stage.id()),
    };
    let tile = ImageTile::from_image(
        input_roi,
        image
            .crop(Roi::new(0, 0, input_roi.width, input_roi.height))
            .unwrap(),
    )
    .unwrap();
    (tile, out)
}

fn run_both(
    g: &GpuRenderer,
    stage: &dyn Stage,
    ctx: &StageContext<'_>,
    settings: &DevelopSettings,
    input: &ImageTile,
    out: Roi,
) -> (ImageTile, ImageTile, u64, u64) {
    let params = stage.params(settings, ctx).unwrap().unwrap();
    let (cpu, n_cpu) = run_stage_cpu(stage, ctx, input, out, &params).unwrap();
    let (gpu, n_gpu) = g
        .run_stage(stage, ctx, input, out, &params)
        .unwrap_or_else(|e| panic!("{}: {e}", stage.id()));
    (cpu, gpu, n_cpu, n_gpu)
}

#[test]
fn all_kernels_compile() {
    // CI のランナー（WARP・仮想の GPU）でも、アダプターがあればシェーダーのコンパイル（Metal・FXC / DXC）
    // だけは確かめる（値は比べない。common の doc）。
    let Some(g) = common::adapter() else {
        return;
    };
    g.compile_all().unwrap();
}

#[test]
fn ci_runners_skip_the_comparisons_unless_required() {
    // 04 の 2.3 節: GPU のない CI では CPU 版の回帰テストだけを行う（GPU 版の比較は開発者の PC）。
    assert!(skips_comparisons(Some("true"), false));
    assert!(skips_comparisons(Some("1"), false));
    assert!(
        !skips_comparisons(Some("true"), true),
        "GENZO_REQUIRE_GPU=1 なら比べる"
    );
    assert!(!skips_comparisons(None, false));
    assert!(!skips_comparisons(Some(""), false));
    assert!(!skips_comparisons(Some("false"), false));
    assert!(!skips_comparisons(Some("0"), false));
}

#[test]
fn every_registered_rgb_stage_matches_the_cpu_version() {
    let g = gpu_or_skip!();
    let registry = StageRegistry::builtin();
    let coverage = registry.gpu_coverage(g.registry(), ProcessVersion::V1);
    eprintln!("GPU 版のあるステージ: {:?}", coverage.with_gpu);
    eprintln!("CPU 版だけのステージ: {:?}", coverage.cpu_only);
    let settings = settings_for_all_stages();
    let mut tested = Vec::new();
    with_context(&settings, |ctx, a1_image| {
        for stage in registry.stages() {
            if g.backend_of(stage, ctx.process_version) != StageBackend::Gpu {
                continue;
            }
            let params = stage.params(&settings, ctx).unwrap();
            assert!(
                params.is_some(),
                "{}: GPU 版を登録したステージが、テストの設定で有効にならない（テストに設定を足すこと）",
                stage.id()
            );
            let (input, out) = case_for(stage, a1_image, ctx, &settings);
            let (cpu, gpu, n_cpu, n_gpu) = run_both(g, stage, ctx, &settings, &input, out);
            assert_eq!(gpu.roi, cpu.roi, "{}", stage.id());
            assert_eq!(gpu.contract(), cpu.contract(), "{}", stage.id());
            let tol = stage_tolerance(stage.id());
            let report = compare_pixels(cpu.image.data(), gpu.image.data(), tol);
            eprintln!("{}: {report}", stage.id());
            assert!(report.passed(), "{}: {report}", stage.id());
            assert_eq!(n_cpu, n_gpu, "{}: 置き換えた NaN・無限大の数", stage.id());
            tested.push(stage.id());
        }
    });
    // GPU 版のある RGB のステージはすべて比べた。
    for id in coverage.with_gpu {
        if registry.find(id).is_some() {
            assert!(tested.contains(&id), "{id} を比べていない");
        }
    }
    assert!(tested.len() >= 12, "{tested:?}");
}

/// `values` の 3 つ組をすべて並べた 1 行ずつの画像（R が最も速く変わる。`width = n²`、`height = n`）。
fn all_triples(values: &[f32], contract: ColorContract) -> RgbImage {
    let n = values.len() as u32;
    RgbImage::from_fn(n * n, n, contract, |x, y| {
        [
            values[(x % n) as usize],
            values[(x / n) as usize],
            values[y as usize],
        ]
    })
    .unwrap()
}

/// 境界の値（B2: ゼロ・負の値・2^−16（対数の下限）の前後・IEC 61966-2-1 の境目・膝（0.5）・1 の前後・
/// 大きな値）。
const B2_BOUNDARY_VALUES: [f32; 19] = [
    0.0,
    -0.0,
    1e-20,
    -1e-20,
    1.0 / 131_072.0,
    1.0 / 65_536.0,
    1.0 / 32_768.0,
    0.003_130_8,
    0.040_45,
    0.18,
    0.5,
    1.0,
    1.000_000_1,
    2.0,
    16.0,
    1e4,
    -1e-6,
    -0.5,
    -2.0,
];

/// 境界の値（B3: 各チャンネル 1 以下なので輝度は 0〜1。負の値（色域外）を含む）。
///
/// B3 の契約（2.6 節: 輝度は 0〜1）の外（輝度が 1 を大きく超える値）は含めない。ステージ 16 の比率の
/// 計算（`y2 + r × (c − y)`）で桁落ちが起き、`pow` の数 ULP の差が許容誤差を超えるため（比べない）。
const B3_BOUNDARY_VALUES: [f32; 15] = [
    0.0,
    -0.0,
    1e-20,
    1.0 / 131_072.0,
    1.0 / 65_536.0,
    1.0 / 32_768.0,
    0.003_130_8,
    0.040_45,
    0.18,
    0.5,
    0.999_999_9,
    1.0,
    -1e-6,
    -0.05,
    -0.5,
];

/// テストの設定に加える、別の分岐を通る設定（負の自然な彩度・彩度 −100・コントラスト ±100・トーンの
/// 両端・露光量の正負・チャンネル別のカーブ（G・B）・輝度のカーブなしのカーブ・パラメトリックだけのカーブ）。
fn other_settings() -> Vec<DevelopSettings> {
    use genzo_model::CurvePoint;
    let mut a = strong_settings();
    a.tone_curve.green = vec![
        CurvePoint::new(0.0, 0.1),
        CurvePoint::new(0.6, 0.5),
        CurvePoint::new(1.0, 0.9),
    ];
    a.tone_curve.blue = vec![
        CurvePoint::new(0.0, 0.0),
        CurvePoint::new(0.3, 0.45),
        CurvePoint::new(1.0, 1.0),
    ];
    a.color.vibrance = -60.0;
    a.color.saturation = -100.0;
    a.contrast = -100.0;
    a.exposure_ev = -3.0;
    a.tone.highlights = 100.0;
    a.tone.shadows = -100.0;
    a.tone.whites = -100.0;
    a.tone.blacks = 100.0;
    let mut b = DevelopSettings {
        exposure_ev: 4.0,
        contrast: 100.0,
        ..Default::default()
    };
    b.tone_curve.blue = vec![CurvePoint::new(0.0, 0.2), CurvePoint::new(1.0, 0.8)];
    b.color.vibrance = 100.0;
    b.tone.shadows = 100.0;
    let mut c = DevelopSettings::default();
    c.tone_curve.parametric.highlights = -100.0;
    c.tone_curve.parametric.lights = 100.0;
    c.color.saturation = 100.0;
    c.tone.blacks = -100.0;
    vec![settings_for_all_stages(), a, b, c]
}

#[test]
fn pixel_stages_match_on_boundary_values_and_other_settings() {
    // 画素ごとのステージ（GPU 版のあるもの）を、境界の値（ゼロ・負の値・下限の前後・伝達関数の境目）と、
    // 別の分岐を通る設定で比べる（2.3 節: NaN・無限大・負の値・ゼロ付近の値の扱いをそろえる）。
    let g = gpu_or_skip!();
    let registry = StageRegistry::builtin();
    let mut compared = 0;
    for settings in other_settings() {
        with_context(&settings, |ctx, _| {
            for stage in registry.stages() {
                if g.backend_of(stage, ctx.process_version) != StageBackend::Gpu
                    || stage.id() == "finish.geometry"
                {
                    continue;
                }
                let Some(params) = stage.params(&settings, ctx).unwrap() else {
                    continue;
                };
                let image = match stage.input_contract() {
                    ColorContract::B2Working => {
                        all_triples(&B2_BOUNDARY_VALUES, ColorContract::B2Working)
                    }
                    ColorContract::B3DisplayReferred => {
                        all_triples(&B3_BOUNDARY_VALUES, ColorContract::B3DisplayReferred)
                    }
                    c => panic!("{}: 想定していない入力の契約 {c}", stage.id()),
                };
                let roi = Roi::full(image.width(), image.height());
                let input = ImageTile::from_image(roi, image).unwrap();
                let (cpu, n_cpu) = run_stage_cpu(stage, ctx, &input, roi, &params).unwrap();
                let (gpu, n_gpu) = g
                    .run_stage(stage, ctx, &input, roi, &params)
                    .unwrap_or_else(|e| panic!("{}: {e}", stage.id()));
                let report = compare_pixels(
                    cpu.image.data(),
                    gpu.image.data(),
                    stage_tolerance(stage.id()),
                );
                assert!(report.passed(), "{}: {report}", stage.id());
                assert_eq!(n_cpu, n_gpu, "{}: 置き換えた NaN・無限大の数", stage.id());
                compared += 1;
            }
        });
    }
    // 4 つの設定で 12 ステージ前後ずつ（設定によっては無効になるステージがある）。
    assert!(compared >= 40, "{compared}");
}

#[test]
fn geometry_matches_over_the_whole_output_including_the_edges() {
    // ステージ 9 を、出力（C の格子）の端の画素を含む全体で比べる。補間のタップが画像の外に出る画素は
    // 鏡映で埋める（2.7 節）。GPU 版は 2 通り: CPU 版と同じく鏡映で取り出した入力を渡す場合と、入力の
    // 格子の画像全体を渡してシェーダーが自分で鏡映する場合（プレビューの経路）。
    let g = gpu_or_skip!();
    let registry = StageRegistry::builtin();
    let geometry = registry.find("finish.geometry").unwrap();
    let full_crop = CropRect::default();
    let cases = [
        (Orientation::Normal, 7.0f32, full_crop),
        (
            Orientation::Rotate90Cw,
            -11.0,
            CropRect {
                left: 0.0,
                top: 0.0,
                right: 0.5,
                bottom: 1.0,
            },
        ),
        (
            Orientation::Transverse,
            3.0,
            CropRect {
                left: 0.3,
                top: 0.2,
                right: 1.0,
                bottom: 1.0,
            },
        ),
        (Orientation::Rotate270Cw, 45.0, full_crop),
        // 回転なし（向きだけ）・画像の端に接する切り抜き。
        (
            Orientation::Rotate180,
            0.0,
            CropRect {
                left: 0.01,
                top: 0.0,
                right: 0.99,
                bottom: 0.97,
            },
        ),
        (Orientation::FlipVertical, 0.5, full_crop),
    ];
    for (orientation, angle, crop) in cases {
        let mut settings = DevelopSettings::default();
        rotate_and_crop(&mut settings, angle, crop);
        // 奇数の寸法（ワークグループ 8 × 8 で割り切れない）。
        let source = working_source(scene_b2(61, 43), orientation).image;
        let a1 = render_a1(&source, &settings, &SensorOptions::default(), 61).unwrap();
        let ctx = a1.finish_context(&settings).unwrap();
        let params = geometry
            .params(&settings, &ctx)
            .unwrap()
            .expect("回転・切り抜き・向きのどれかがあるのでステージ 9 は有効");
        let plan = params
            .get::<genzo_pipeline::finish::GeometryPlan>()
            .unwrap();
        let (w, h) = plan.output_size();
        let out = Roi::full(w, h);
        let (aw, ah) = a1.image.dimensions();
        let whole = ImageTile::from_image(Roi::full(aw, ah), a1.image.clone()).unwrap();
        let need = geometry.input_roi(out, &params, &ctx);
        assert!(
            !Roi::full(aw, ah).contains_roi(&need),
            "{orientation:?}: 端の外（鏡映）を使う場合を比べる"
        );
        let gathered = tile_of(&a1.image, need);
        let (cpu, _) = run_stage_cpu(geometry, &ctx, &gathered, out, &params).unwrap();
        let (gpu, _) = g
            .run_stage(geometry, &ctx, &gathered, out, &params)
            .unwrap();
        let (mirrored, _) = g.run_stage(geometry, &ctx, &whole, out, &params).unwrap();
        let tol = stage_tolerance(geometry.id());
        for (label, t) in [("鏡映した入力", &gpu), ("画像全体", &mirrored)] {
            assert_eq!(t.roi, out);
            let r = compare_pixels(cpu.image.data(), t.image.data(), tol);
            assert!(
                r.passed(),
                "{orientation:?} {angle} {crop:?}（{label}）: {r}"
            );
        }
    }
}

#[test]
fn pixelwise_stages_do_not_depend_on_the_tiling() {
    // 画素ごとの計算は全体での座標だけに依存する（タイルに分けても GPU 版の結果はビット単位で同じ）。
    let g = gpu_or_skip!();
    let registry = StageRegistry::builtin();
    let settings = settings_for_all_stages();
    with_context(&settings, |ctx, a1_image| {
        for id in ["finish.geometry", "finish.tone", "output.export.srgb"] {
            let stage = registry.find(id).unwrap();
            let params = stage.params(&settings, ctx).unwrap().unwrap();
            let (input, out) = case_for(stage, a1_image, ctx, &settings);
            let (whole, _) = g.run_stage(stage, ctx, &input, out, &params).unwrap();
            let half = out.width / 2;
            let left = Roi::new(out.x, out.y, half, out.height);
            let right = Roi::new(out.x + half as i32, out.y, out.width - half, out.height);
            for part in [left, right] {
                let (t, _) = g.run_stage(stage, ctx, &input, part, &params).unwrap();
                for y in 0..part.height {
                    for x in 0..part.width {
                        let gx = i64::from(part.x) + i64::from(x);
                        let gy = i64::from(part.y) + i64::from(y);
                        let a = whole.at(gx, gy).unwrap();
                        let b = t.at(gx, gy).unwrap();
                        assert_eq!(
                            a.map(f32::to_bits),
                            b.map(f32::to_bits),
                            "{id} ({gx}, {gy})"
                        );
                    }
                }
            }
        }
    });
}

#[test]
fn extreme_values_stay_finite() {
    // 途中で無限大になりうる値（1e30 など）でも、GPU 版の出力は有限（2.6 節: NaN・無限大は出力しない）。
    // 一致は比べない（WGSL は無限大の途中の結果を規定しない。tolerance の doc）。
    //
    // Metal のバックエンドは高速な数学でコンパイルされ（genzo-gpu の lib の doc）、コンパイラが
    // 「NaN・無限大はない」と仮定して置き換えの判定を消しうるので、Metal では数を表示するだけにする
    // （既知の制限。上流（wgpu）への要望と PoC-3 で確かめる）。
    let g = gpu_or_skip!();
    let metal = g.context().summary().backend == "Metal";
    let registry = StageRegistry::builtin();
    let settings = settings_for_all_stages();
    let extremes = [
        1e30f32, -1e30, 3e38, -3e38, 1e-40, -1e-40, 0.0, -0.0, 1e-20, 65504.0,
    ];
    with_context(&settings, |ctx, _| {
        for stage in registry.stages() {
            if g.backend_of(stage, ctx.process_version) != StageBackend::Gpu
                || stage.id() == "finish.geometry"
            {
                continue;
            }
            let n = extremes.len() as u32;
            let image = RgbImage::from_fn(n * n, n, stage.input_contract(), |x, y| {
                [
                    extremes[(x % n) as usize],
                    extremes[(x / n) as usize],
                    extremes[y as usize],
                ]
            })
            .unwrap();
            let roi = Roi::full(n * n, n);
            let input = ImageTile::from_image(roi, image).unwrap();
            let params = stage.params(&settings, ctx).unwrap().unwrap();
            let (out, _) = g.run_stage(stage, ctx, &input, roi, &params).unwrap();
            let non_finite = out
                .image
                .as_flat()
                .iter()
                .filter(|v| !v.is_finite())
                .count();
            if metal {
                eprintln!("{}（Metal）: 有限でない値 {non_finite} 個", stage.id());
            } else {
                assert_eq!(non_finite, 0, "{}: 有限でない値がある", stage.id());
            }
        }
    });
}

#[test]
fn gpu_stage_checks_inputs_like_the_cpu_version() {
    let g = gpu_or_skip!();
    let registry = StageRegistry::builtin();
    let settings = settings_for_all_stages();
    with_context(&settings, |ctx, _| {
        let stage = registry.find("finish.exposure").unwrap();
        let params = stage.params(&settings, ctx).unwrap().unwrap();
        let roi = Roi::new(0, 0, 8, 8);
        // 契約が違う。
        let wrong = ImageTile::from_image(roi, scene_b3(8, 8)).unwrap();
        assert!(g.run_stage(stage, ctx, &wrong, roi, &params).is_err());
        // 入力が出力の範囲を覆っていない。
        let small = ImageTile::from_image(Roi::new(0, 0, 4, 4), scene_b2(4, 4)).unwrap();
        assert!(g.run_stage(stage, ctx, &small, roi, &params).is_err());
        // GPU 版のないステージは Unsupported。
        let texture = registry.find("finish.texture").unwrap();
        let input = ImageTile::from_image(roi, scene_b2(8, 8)).unwrap();
        let err = g.run_stage(texture, ctx, &input, roi, &params).unwrap_err();
        assert!(
            matches!(err, genzo_gpu::GpuError::Unsupported { .. }),
            "{err}"
        );
    });
}

// ---------------------------------------------------------------------------
// センサーのステージ
// ---------------------------------------------------------------------------

/// センサーのステージ `id` を、CPU 版で前のステージまで進めた入力で CPU 版と GPU 版の両方で実行して比べる。
fn compare_sensor_stage(
    g: &GpuRenderer,
    id: &str,
    raw: &genzo_raw::RawImage,
    settings: &DevelopSettings,
    clip: SaturationClip,
    roi: Roi,
) {
    let options = SensorOptions {
        quality: RenderQuality::Draft,
        saturation_clip: clip,
        ..Default::default()
    };
    let plan = SensorPlan::new(raw, settings, &options).unwrap();
    let rois = plan.stage_rois(roi).unwrap();
    let ctx = plan.context();
    let mut data = SensorData::Raw(raw);
    for (k, stage) in plan.stages().iter().enumerate() {
        let out_roi = rois[k + 1];
        if stage.id() == id {
            let (cpu, n_cpu) =
                run_sensor_stage(*stage, &ctx, &plan, data.clone(), out_roi).unwrap();
            let (gpu, n_gpu) = g
                .run_sensor_stage(*stage, &plan, data, out_roi)
                .unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(n_cpu, n_gpu, "{id}");
            assert_eq!(cpu.roi(), gpu.roi(), "{id}");
            let tol = stage_tolerance(id);
            match (&cpu, &gpu) {
                (SensorData::Cfa(a), SensorData::Cfa(b)) => {
                    let r = compare(a.data(), b.data(), tol);
                    assert!(r.passed(), "{id} {clip:?}: {r}");
                    assert_eq!(a.saturated(), b.saturated(), "{id}: 飽和の印");
                    assert_eq!(a.pattern(), b.pattern(), "{id}: CFA の並び");
                }
                (SensorData::Rgb(a), SensorData::Rgb(b)) => {
                    assert_eq!(a.contract(), b.contract(), "{id}");
                    let r = compare_pixels(a.image.data(), b.image.data(), tol);
                    assert!(r.passed(), "{id} {clip:?}: {r}");
                }
                _ => panic!("{id}: 出力の種類が違う"),
            }
            return;
        }
        let (next, _) = run_sensor_stage(*stage, &ctx, &plan, data, out_roi).unwrap();
        data = next;
    }
    panic!("{id} は簡易処理のステージの列にない");
}

#[test]
fn every_registered_sensor_stage_matches_the_cpu_version() {
    let g = gpu_or_skip!();
    let registry = StageRegistry::builtin();
    let scene = camera_scene(66, 50);
    let mut settings = DevelopSettings::default();
    custom_wb(&mut settings, 4300.0, 8.0);
    let mut tested = Vec::new();
    for stage in registry.sensor_stages() {
        let has_gpu = stage
            .gpu(g.registry())
            .is_some_and(|s| s.supports(ProcessVersion::V1));
        if !has_gpu {
            continue;
        }
        for cfa in CfaPattern::ALL {
            let raw = raw_from(&scene, cfa, true);
            for clip in [
                SaturationClip::default(),
                SaturationClip::Off,
                SaturationClip::Global,
            ] {
                // 半分の格子の範囲（画像の外にはみ出す範囲と、内側の範囲）。
                for roi in [
                    Roi::new(-3, -2, 20, 15),
                    Roi::new(5, 4, 16, 12),
                    Roi::full(33, 25),
                ] {
                    compare_sensor_stage(g, stage.id(), &raw, &settings, clip, roi);
                }
            }
        }
        tested.push(stage.id());
    }
    eprintln!("比べたセンサーのステージ: {tested:?}");
    assert_eq!(tested.len(), 4, "{tested:?}");
}

#[test]
fn sensor_stage_without_gpu_version_is_unsupported() {
    let g = gpu_or_skip!();
    let raw = raw_from(&camera_scene(20, 16), CfaPattern::RGGB, false);
    let plan =
        SensorPlan::new(&raw, &DevelopSettings::default(), &SensorOptions::default()).unwrap();
    let rcd = plan.stages()[2];
    assert_eq!(rcd.id(), "sensor.demosaic.rcd");
    let cfa =
        genzo_pipeline::CfaImage::from_fn(Roi::new(-4, -4, 28, 24), CfaPattern::RGGB, |_, _| 0.2)
            .unwrap();
    let err = g
        .run_sensor_stage(rcd, &plan, SensorData::Cfa(cfa), Roi::full(20, 16))
        .unwrap_err();
    assert!(
        matches!(err, genzo_gpu::GpuError::Unsupported { .. }),
        "{err}"
    );
    // 入力の種類が違う。
    let norm = plan.stages()[0];
    let rgb = ImageTile::from_image(Roi::full(4, 4), scene_b2(4, 4)).unwrap();
    assert!(
        g.run_sensor_stage(norm, &plan, SensorData::Rgb(rgb), Roi::full(4, 4))
            .is_err()
    );
}

#[test]
fn sensor_stage_rejects_a_raw_that_does_not_match_the_plan_like_the_cpu_version() {
    // CPU 版（SensorPlan::check_raw）は、設定を作った RAW と CFA の並び・寸法が違う RAW をエラーにする。
    // GPU 版も同じエラーにする（以前は CFA の並びを確かめず、違う並びのまま正規化していた）。
    let g = gpu_or_skip!();
    let scene = camera_scene(20, 16);
    let rggb = raw_from(&scene, CfaPattern::RGGB, false);
    let options = SensorOptions {
        quality: RenderQuality::Draft,
        ..Default::default()
    };
    let plan = SensorPlan::new(&rggb, &DevelopSettings::default(), &options).unwrap();
    let norm = plan.stages()[0];
    let roi = Roi::full(20, 16);
    for other in [
        raw_from(&scene, CfaPattern::BGGR, false),
        raw_from(&camera_scene(22, 16), CfaPattern::RGGB, false),
    ] {
        let cpu = run_sensor_stage(norm, &plan.context(), &plan, SensorData::Raw(&other), roi)
            .map(|_| ())
            .unwrap_err();
        let gpu = g
            .run_sensor_stage(norm, &plan, SensorData::Raw(&other), roi)
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&gpu, genzo_gpu::GpuError::Pipeline(e) if e.to_string() == cpu.to_string()),
            "CPU: {cpu} / GPU: {gpu}"
        );
    }
    // 同じ RAW は通る。
    assert!(
        g.run_sensor_stage(norm, &plan, SensorData::Raw(&rggb), roi)
            .is_ok()
    );
}
