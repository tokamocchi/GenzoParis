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
    ColorContract, ImageTile, InputKind, MemoryProfiles, ProcessVersion, ProfileResolver,
    ProfileResources, RgbImage, Roi, SaturationClip, SensorOptions, Stage, StageContext,
    StageRegistry, render_a1, run_stage_cpu,
};
use genzo_raw::CfaPattern;

use common::*;

/// 処理バージョン 2 のステージ 12・14・16 の効果の項目（テクスチャ・明瞭度・かすみの除去・シャープ・周辺光量・粒子）。
fn with_detail_features(mut s: DevelopSettings) -> DevelopSettings {
    s.presence.texture = 45.0;
    s.presence.clarity = -35.0;
    s.presence.dehaze = 40.0;
    s.detail.sharpening.amount = 90.0;
    s.detail.sharpening.radius = 2.0;
    s.detail.sharpening.detail = 40.0;
    s.detail.sharpening.masking = 30.0;
    s.effects.vignette.amount = -45.0;
    s.effects.vignette.roundness = -40.0;
    s.effects.vignette.highlights = 50.0;
    s.effects.grain.amount = 40.0;
    s.effects.grain.size = 70.0;
    s
}

/// テストのカメラプロファイル（ルックテーブルの明度の分割 5・sRGB の符号化）とルック（3D LUT。17³・DOMAIN あり）
/// （DEV-17。値はテスト用で、実在のプロファイルではない）。
fn test_profiles() -> &'static ProfileResources {
    use std::sync::OnceLock;
    static RES: OnceLock<ProfileResources> = OnceLock::new();
    RES.get_or_init(|| {
        let mut lib = MemoryProfiles::new();
        lib.insert_camera("test", test_dcp(), &"aa".repeat(32));
        lib.insert_look("test", test_cube(), &"bb".repeat(32));
        lib.insert_ai_mask(test_ai_raster());
        let mut s = DevelopSettings {
            process_version: 2,
            ..Default::default()
        };
        s.profile.camera = Some("test".into());
        s.profile.look = Some("test".into());
        s.masks.push(genzo_model::LocalAdjustment::new(
            genzo_model::MaskComponent::add(test_ai_shape()),
        ));
        ProfileResources::resolve(&s, Some(&lib as &dyn ProfileResolver))
    })
}

/// AI マスク（DEV-20）のテスト用のラスター（24 × 16。斜めのグラデーション）。
fn test_ai_raster() -> genzo_pipeline::AiMaskRaster {
    let values = (0..16)
        .flat_map(|y| (0..24).map(move |x| ((x * 9 + y * 11) % 256) as u8))
        .collect();
    genzo_pipeline::AiMaskRaster::new(24, 16, values).unwrap()
}

/// [`test_ai_raster`] を G の [0.1, 0.05, 0.9, 0.6] に置く形（[`test_profiles`] で引ける）。
fn test_ai_shape() -> genzo_model::MaskShape {
    use genzo_model::{AiMaskKind, DataRef, MaskShape};
    MaskShape::Ai {
        kind: AiMaskKind::Sky,
        model: "test".into(),
        mask: DataRef::new(AiMaskKind::Sky.ref_id(), test_ai_raster().sha256_hex()),
        width: 24,
        height: 16,
        bounds: [0.1, 0.05, 0.9, 0.6],
    }
}

/// テストの DCP（色相彩度のマップは色相で変わるずれ・彩度で変わる倍率、ルックテーブルは明度でも変わる）。
fn test_dcp() -> genzo_color::DcpProfile {
    use genzo_color::{HsvTable, Mat3};
    let m = typical_camera().map(|r| r.map(f64::from));
    let table = |hue: u32, sat: u32, val: u32, k: f32| {
        let mut e = Vec::new();
        for v in 0..val {
            for h in 0..hue {
                for s in 0..sat {
                    // 彩度 0 の点は色相によらない（実際のプロファイルと同じ形。色相が定まらないため）。
                    let sf = s as f32;
                    e.push([
                        k * (h as f32 * 7.0 - 20.0) * sf,
                        1.0 + k * 0.1 * sf - 0.02 * h as f32 * sf,
                        1.0 - 0.04 * v as f32 + 0.01 * h as f32 * sf,
                    ]);
                }
            }
        }
        e
    };
    genzo_color::DcpProfile {
        calibration_illuminants: [Some(17), Some(21)],
        color_matrices: [Some(Mat3::from_rows(m)), Some(Mat3::from_rows(m))],
        hue_sat_maps: [
            Some(HsvTable::new([12, 4, 1], false, table(12, 4, 1, 1.0)).unwrap()),
            Some(HsvTable::new([12, 4, 1], false, table(12, 4, 1, -0.5)).unwrap()),
        ],
        look_table: Some(HsvTable::new([9, 5, 5], true, table(9, 5, 5, 0.7)).unwrap()),
        ..Default::default()
    }
}

/// テストのルック（色を回して暗部を持ち上げる 17³ の LUT。入力の範囲は 0〜1.25）。
fn test_cube() -> genzo_color::CubeLut {
    let lut = genzo_color::Lut3d::from_fn(17, |v| {
        [
            0.05 + 0.9 * v[1],
            0.03 + 0.8 * v[2] + 0.1 * v[0],
            0.9 * v[0] * v[0],
        ]
    })
    .unwrap();
    genzo_color::CubeLut::new(None, 17, lut.data().to_vec(), [0.0; 3], [1.25; 3]).unwrap()
}

/// RGB のステージを有効にする設定（回転・切り抜きでステージ 9 も有効にする）。
fn settings_for_all_stages() -> DevelopSettings {
    let mut s = with_detail_features(strong_settings());
    // カメラプロファイルのルックテーブルとルック（DEV-17。文脈に test_profiles を付ける）。
    s.profile.camera = Some("test".into());
    s.profile.look = Some("test".into());
    s.profile.look_amount = 70.0;
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

/// 段階 C の文脈（96 × 64 の B2 の画像を A1 とし、ガイドを付ける）を作って `f` を呼ぶ。入力の種類
/// （RAW のシーン参照・RAW 以外のディスプレイ参照。ステージ 15 の 2 つの分岐）ごとに 1 回ずつ呼ぶ。
fn with_context(settings: &DevelopSettings, mut f: impl FnMut(&StageContext<'_>, &RgbImage)) {
    let image = scene_b2(96, 64);
    let source = working_source(image, Orientation::Rotate90Cw).image;
    let a1 = render_a1(&source, settings, &SensorOptions::default(), 96).unwrap();
    let guide = compute_guide(&a1, &GuideSpec::from_settings(settings).unwrap()).unwrap();
    for kind in [InputKind::SceneReferred, InputKind::DisplayReferred] {
        let ctx = a1
            .finish_context(settings)
            .unwrap()
            .with_guide(&guide)
            .with_input_kind(kind)
            .with_profiles(test_profiles());
        f(&ctx, &a1.image);
    }
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
    // 周辺の画素を使うステージ（12・14）は、必要な範囲（ぼかしの余白）よりさらに少し広くする。
    let out = Roi::new(5, 4, 70, 50);
    let need = stage.input_roi(out, &params, ctx);
    let input_roi = need.expand(2);
    let (w, h) = if need == out {
        (80, 60)
    } else {
        (input_roi.width, input_roi.height)
    };
    let image = match stage.input_contract() {
        ColorContract::B2Working => scene_b2(w, h),
        ColorContract::B3DisplayReferred => scene_b3(w, h),
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
            // カメラプロファイルのルックテーブルは RAW（シーン参照）だけ。
            if params.is_none()
                && stage.id() == genzo_pipeline::finish::ProfileLookTableStage::ID
                && ctx.input_kind == InputKind::DisplayReferred
            {
                continue;
            }
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

/// 境界の値（B2: ゼロ・負の値・2^−16（対数の下限）の前後・IEC 61966-2-1 の境目・0.5・1 の前後・
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
    // 処理バージョン 2 のステージ 12・14・16 の別の分岐（テクスチャだけ・かすみを足す・マスクなしのシャープ・
    // 明るくする周辺光量・丸み +100・粗さ 0 の粒子）。
    let mut d = DevelopSettings::default();
    d.presence.texture = -80.0;
    d.presence.dehaze = -60.0;
    d.detail.sharpening.amount = 150.0;
    d.detail.sharpening.detail = 100.0;
    d.effects.vignette.amount = 70.0;
    d.effects.vignette.roundness = 100.0;
    d.effects.vignette.feather = 0.0;
    d.effects.grain.amount = 100.0;
    d.effects.grain.roughness = 0.0;
    d.effects.grain.size = 0.0;
    vec![
        settings_for_all_stages(),
        a,
        b,
        c,
        color_features(false),
        color_features(true),
        d,
    ]
}

/// ステージ 13 の処理バージョン 2 の項目（カラーミキサー・カラーグレーディング。`black_white` で白黒と白黒ミキサー）。
fn color_features(black_white: bool) -> DevelopSettings {
    use genzo_model::{GradingWheel, HslAdjustment, HueBands};
    let mut s = DevelopSettings::default();
    s.color.saturation = -20.0;
    s.color.vibrance = 40.0;
    s.color.mixer.red = HslAdjustment {
        hue: 100.0,
        saturation: -50.0,
        luminance: 30.0,
    };
    s.color.mixer.blue = HslAdjustment {
        hue: -100.0,
        saturation: 100.0,
        luminance: -100.0,
    };
    s.color.mixer.yellow.luminance = 60.0;
    s.color.mixer.magenta.hue = 40.0;
    s.color.grading.shadows = GradingWheel {
        hue: 210.0,
        saturation: 60.0,
        luminance: 20.0,
    };
    s.color.grading.midtones = GradingWheel {
        hue: 100.0,
        saturation: 20.0,
        luminance: 0.0,
    };
    s.color.grading.highlights = GradingWheel {
        hue: 35.0,
        saturation: 40.0,
        luminance: -30.0,
    };
    s.color.grading.global = GradingWheel {
        hue: 300.0,
        saturation: 10.0,
        luminance: 10.0,
    };
    s.color.grading.blending = 30.0;
    s.color.grading.balance = -40.0;
    s.color.black_white.enabled = black_white;
    s.color.black_white.mix =
        HueBands::from_values([80.0, -40.0, 20.0, 0.0, -60.0, -100.0, 50.0, 100.0]);
    s
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
                // 周辺の画素を使うステージ（12・14）は、余白を鏡映で足す。
                let need = stage.input_roi(roi, &params, ctx);
                let input = if need == roi {
                    ImageTile::from_image(roi, image).unwrap()
                } else {
                    tile_of(&image, need)
                };
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
    // 7 つの設定 × 2 つの入力の種類で 12 ステージ前後ずつ（設定によっては無効になるステージがある）。
    assert!(compared >= 100, "{compared}");
}

#[test]
fn color_stage_matches_around_the_hue_circle() {
    // ステージ 13 の色相の帯（自前の atan の多項式・帯の境目）・回転（sin / cos の多項式）・無彩色に近い画素を、
    // OKLCh の色相を 1° ずつ回した画像で比べる（L 4 段 × C 6 段 × 色相 360）。
    use genzo_color::oklab::oklab_to_linear_bt2020;
    use genzo_pipeline::finish::ColorStage;
    let g = gpu_or_skip!();
    let ls = [0.15f32, 0.45, 0.8, 1.1];
    let cs = [0.0f32, 1e-4, 0.01, 0.03, 0.12, 0.3];
    let image = RgbImage::from_fn(
        360,
        (ls.len() * cs.len()) as u32,
        ColorContract::B2Working,
        |x, y| {
            let l = ls[y as usize / cs.len()];
            let c = cs[y as usize % cs.len()];
            let h = (x as f32).to_radians();
            oklab_to_linear_bt2020([l, c * h.cos(), c * h.sin()])
        },
    )
    .unwrap();
    let roi = Roi::full(image.width(), image.height());
    let input = ImageTile::from_image(roi, image).unwrap();
    let mut compared = 0;
    for settings in [color_features(false), color_features(true)] {
        with_context(&settings, |ctx, _| {
            let params = ColorStage.params(&settings, ctx).unwrap().unwrap();
            let (cpu, n_cpu) = run_stage_cpu(&ColorStage, ctx, &input, roi, &params).unwrap();
            let (gpu, n_gpu) = g.run_stage(&ColorStage, ctx, &input, roi, &params).unwrap();
            let report = compare_pixels(
                cpu.image.data(),
                gpu.image.data(),
                stage_tolerance(ColorStage::ID),
            );
            eprintln!("{}: {report}", ColorStage::ID);
            assert!(report.passed(), "{report}");
            assert_eq!(n_cpu, n_gpu);
            compared += 1;
        });
    }
    assert_eq!(compared, 4);
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
        for id in [
            "finish.geometry",
            "finish.tone",
            "finish.texture",
            "finish.sharpen",
            "finish.effects",
            "output.export.srgb",
        ] {
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
            let Some(params) = stage.params(&settings, ctx).unwrap() else {
                continue;
            };
            let need = stage.input_roi(roi, &params, ctx);
            let input = if need == roi {
                ImageTile::from_image(roi, image).unwrap()
            } else {
                tile_of(&image, need)
            };
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
        // GPU 版を登録していないステージは Unsupported。
        let without = common::renderer()
            .unwrap()
            .unwrap()
            .with_registry(genzo_gpu::GpuStageRegistry::builtin().without("finish.texture"));
        let texture = registry.find("finish.texture").unwrap();
        let input = ImageTile::from_image(roi, scene_b2(8, 8)).unwrap();
        let err = without
            .run_stage(texture, ctx, &input, roi, &params)
            .unwrap_err();
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
    compare_sensor_stage_with(g, id, raw, settings, &options, roi);
}

/// [`compare_sensor_stage`] の設定を指定する版（カメラプロファイルを渡すときなど）。
fn compare_sensor_stage_with(
    g: &GpuRenderer,
    id: &str,
    raw: &genzo_raw::RawImage,
    settings: &DevelopSettings,
    options: &SensorOptions,
    roi: Roi,
) {
    let clip = options.saturation_clip;
    let plan = SensorPlan::new(raw, settings, options).unwrap();
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
                    eprintln!(
                        "差 {id}: 最大 {:e}、許容に対する比 {:.4}",
                        r.max_abs, r.max_ratio
                    );
                    assert!(r.passed(), "{id} {clip:?}: {r}");
                    assert_eq!(a.saturated(), b.saturated(), "{id}: 飽和の印");
                    assert_eq!(a.pattern(), b.pattern(), "{id}: CFA の並び");
                }
                (SensorData::Rgb(a), SensorData::Rgb(b)) => {
                    assert_eq!(a.contract(), b.contract(), "{id}");
                    let r = compare_pixels(a.image.data(), b.image.data(), tol);
                    eprintln!(
                        "差 {id}: 最大 {:e}、許容に対する比 {:.4}",
                        r.max_abs, r.max_ratio
                    );
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
    // 極端な WB（R の係数が 1 未満。飽和の切りそろえの値が緑の係数で決まる。人の判断 No.19）。
    let mut extreme = DevelopSettings::default();
    custom_wb(&mut extreme, 2000.0, 0.0);
    // ステージ 7（レンズ補正。処理バージョン 2）は、補正があるときだけ計画に入るので、埋め込みの情報と
    // 補正の設定を付けて比べる。
    let lens_id = genzo_pipeline::sensor::LensCorrectionStage::ID;
    let with_lens = |s: &DevelopSettings| {
        let mut s = s.clone();
        s.lens.profile_enabled = true;
        s.lens.remove_chromatic_aberration = true;
        s.lens.vignetting_amount = 40.0;
        s.lens.ca_red_cyan = 70.0;
        s.lens.ca_blue_yellow = -50.0;
        s
    };
    let lens_data = genzo_raw::LensCorrectionData::SonyArw(genzo_raw::SonyLensCorrection {
        distortion: Vec::new(),
        vignetting: (0..16).map(|i| (300 * i) as i16).collect(),
        ca_red: (0..16).map(|i| (900 * i) as i16).collect(),
        ca_blue: (0..16).map(|i| (-700 * i) as i16).collect(),
    });
    // ステージ 4（ハイライト復元）・6（ノイズ軽減）も、設定が何かをするときだけ計画に入る。方式・量・面の有無を変えて
    // 比べる（ハイライト復元の量 < 100 % は、切りそろえ（clip の 3 種類）と混ぜる経路）。
    let hr_id = genzo_pipeline::sensor::HighlightRecoveryStage::ID;
    let nr_id = genzo_pipeline::sensor::NoiseReductionStage::ID;
    let variants = |id: &str, s: &DevelopSettings| -> Vec<DevelopSettings> {
        if id == lens_id {
            vec![with_lens(s)]
        } else if id == hr_id {
            [
                (genzo_model::HighlightMethod::Reconstruct, 100.0),
                (genzo_model::HighlightMethod::Reconstruct, 60.0),
                (genzo_model::HighlightMethod::Neutral, 100.0),
            ]
            .into_iter()
            .map(|(method, amount)| {
                let mut s = s.clone();
                s.highlight_recovery = genzo_model::HighlightRecovery { method, amount };
                s
            })
            .collect()
        } else if id == nr_id {
            [(40.0, 50.0, 30.0), (70.0, 0.0, 0.0), (0.0, 80.0, 0.0)]
                .into_iter()
                .map(|(luminance, color, luminance_contrast)| {
                    let mut s = s.clone();
                    s.detail.noise_reduction = genzo_model::NoiseReduction {
                        luminance,
                        color,
                        luminance_contrast,
                        ..Default::default()
                    };
                    s
                })
                .collect()
        } else {
            vec![s.clone()]
        }
    };
    let mut tested = Vec::new();
    for stage in registry.sensor_stages() {
        let has_gpu = stage
            .gpu(g.registry())
            .is_some_and(|s| s.supports(ProcessVersion::V2));
        if !has_gpu {
            continue;
        }
        let is_lens = stage.id() == lens_id;
        for (settings, extreme) in variants(stage.id(), &settings)
            .into_iter()
            .zip(variants(stage.id(), &extreme))
        {
            for cfa in CfaPattern::ALL {
                // 飽和の閾値 = 白レベル の RAW と、閾値が白レベルより低い RAW（A-3）。
                for mut raw in [
                    raw_from(&scene, cfa, true),
                    raw_with_crop_and_plateau(&scene, cfa),
                ] {
                    if is_lens {
                        raw.lens_correction = Some(lens_data.clone());
                    }
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
                        compare_sensor_stage(
                            g,
                            stage.id(),
                            &raw,
                            &extreme,
                            clip,
                            Roi::new(-3, -2, 20, 15),
                        );
                    }
                }
            }
        }
        tested.push(stage.id());
    }
    eprintln!("比べたセンサーのステージ: {tested:?}");
    assert_eq!(tested.len(), 7, "{tested:?}");
    assert!(tested.contains(&lens_id));
    assert!(tested.contains(&hr_id));
    assert!(tested.contains(&nr_id));
}

/// ステージ 9 の歪曲補正（処理バージョン 2。手動の歪曲と回転・切り抜き）の CPU 版と GPU 版の一致。
#[test]
fn geometry_with_lens_distortion_matches_the_cpu_version() {
    let g = gpu_or_skip!();
    let mut compared = 0;
    for (amount, deg) in [(60.0f32, 0.0f32), (-80.0, 5.0), (100.0, -12.0)] {
        let mut settings = settings_for_all_stages();
        settings.lens.distortion = amount;
        settings.geometry.angle_deg = deg;
        with_context(&settings, |ctx, a1_image| {
            assert!(ctx.frames.distortion.is_some());
            let registry = StageRegistry::builtin();
            let stage = registry.find("finish.geometry").unwrap();
            let params = stage.params(&settings, ctx).unwrap().unwrap();
            let plan = params
                .get::<genzo_pipeline::finish::GeometryPlan>()
                .unwrap();
            assert!(plan.warp().is_some());
            let (w, h) = plan.output_size();
            for out in [Roi::new(2, 3, w - 4, h - 5), Roi::full(w, h)] {
                let need = stage.input_roi(out, &params, ctx);
                let input = tile_of(a1_image, need);
                let (cpu, gpu, n_cpu, n_gpu) = run_both(g, stage, ctx, &settings, &input, out);
                let report = compare_pixels(
                    cpu.image.data(),
                    gpu.image.data(),
                    stage_tolerance("finish.geometry"),
                );
                eprintln!("歪曲 {amount} 回転 {deg}: {report}");
                assert!(report.passed(), "{amount} {deg}: {report}");
                assert_eq!(n_cpu, n_gpu);
                compared += 1;
            }
        });
    }
    assert_eq!(compared, 3 * 2 * 2);
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

/// ステージ 8 のカメラプロファイル（DEV-17。色相彩度のマップがあれば HSV の表のカーネル）の CPU 版と GPU 版の一致。
#[test]
fn stage_8_with_a_camera_profile_matches_the_cpu_version() {
    let g = gpu_or_skip!();
    let scene = camera_scene(66, 50);
    let dcp = std::sync::Arc::new(test_dcp());
    for (temperature, tint) in [(3200.0, 0.0), (5000.0, 10.0), (6500.0, -15.0)] {
        let mut settings = DevelopSettings::default();
        custom_wb(&mut settings, temperature, tint);
        settings.profile.camera = Some("test".into());
        let options = SensorOptions {
            quality: RenderQuality::Draft,
            camera_profile: Some(std::sync::Arc::clone(&dcp)),
            ..Default::default()
        };
        for cfa in [CfaPattern::RGGB, CfaPattern::GBRG] {
            let raw = raw_from(&scene, cfa, true);
            let plan = SensorPlan::new(&raw, &settings, &options).unwrap();
            assert!(
                plan.camera_profile()
                    .is_some_and(|p| p.hue_sat_map.is_some()),
                "色相彩度のマップがある"
            );
            for roi in [Roi::new(-3, -2, 20, 15), Roi::full(33, 25)] {
                compare_sensor_stage_with(
                    g,
                    genzo_pipeline::sensor::ColorMatrixStage::ID,
                    &raw,
                    &settings,
                    &options,
                    roi,
                );
            }
        }
    }
}

/// HSV の表（ルックテーブル）を、色相環・彩度（色域の外を含む）・明度（1 を超える値を含む）の細かい格子で比べる。
#[test]
fn look_table_matches_around_the_hue_circle_and_values() {
    use genzo_color::dcp::{hsv_to_rgb, prophoto_to_working};
    use genzo_pipeline::finish::ProfileLookTableStage;
    let g = gpu_or_skip!();
    let to_working = prophoto_to_working().unwrap().to_f32();
    let sats = [0.0f32, 0.05, 0.3, 0.7, 1.0, 1.15];
    let vals = [0.001f32, 0.02, 0.18, 0.5, 0.9, 1.0, 1.7, 6.0];
    let hues: Vec<f32> = (0..240).map(|i| i as f32 * 0.025).collect();
    let w = hues.len() as u32;
    let h = (sats.len() * vals.len()) as u32;
    let image = RgbImage::from_fn(w, h, ColorContract::B2Working, |x, y| {
        let s = sats[y as usize % sats.len()];
        let v = vals[y as usize / sats.len()];
        to_working.apply(hsv_to_rgb(hues[x as usize], s, v))
    })
    .unwrap();
    let mut settings = DevelopSettings::default();
    settings.profile.camera = Some("test".into());
    let frames =
        genzo_pipeline::CoordinateFrames::new(w, h, genzo_model::Orientation::Normal).unwrap();
    let ctx = StageContext::new(ProcessVersion::V2, frames).with_profiles(test_profiles());
    let stage = ProfileLookTableStage;
    let params = stage.params(&settings, &ctx).unwrap().unwrap();
    let roi = Roi::full(w, h);
    let input = ImageTile::from_image(roi, image).unwrap();
    let (cpu, n_cpu) = run_stage_cpu(&stage, &ctx, &input, roi, &params).unwrap();
    let (gpu, n_gpu) = g.run_stage(&stage, &ctx, &input, roi, &params).unwrap();
    let report = compare_pixels(
        cpu.image.data(),
        gpu.image.data(),
        stage_tolerance(stage.id()),
    );
    eprintln!("{}: {report}", stage.id());
    assert!(report.passed(), "{report}");
    assert_eq!(n_cpu, n_gpu);
}

/// ローカル補正（マスク。DEV-18・DEV-19）のすべての形・合成・差分を使う設定（処理バージョン 2）。
fn masks_settings() -> DevelopSettings {
    use genzo_model::{
        BrushStroke, ColorSample, LocalAdjustment, LocalDelta, MaskComponent, MaskMode, MaskShape,
    };
    let mut s = with_detail_features(strong_settings());
    s.process_version = 2;
    let all = LocalDelta {
        temperature: 40.0,
        tint: -30.0,
        exposure_ev: 0.8,
        contrast: 35.0,
        highlights: -60.0,
        shadows: 45.0,
        whites: 25.0,
        blacks: -15.0,
        texture: 50.0,
        clarity: 40.0,
        dehaze: 30.0,
        saturation: -50.0,
        sharpness: 70.0,
    };
    let mut radial = LocalAdjustment {
        delta: all,
        ..LocalAdjustment::new(MaskComponent::add(MaskShape::Radial {
            cx: 0.35,
            cy: 0.3,
            rx: 0.3,
            ry: 0.18,
            angle_deg: 25.0,
            feather: 70.0,
        }))
    };
    radial.components.push(MaskComponent {
        mode: MaskMode::Subtract,
        invert: false,
        shape: MaskShape::Linear {
            x0: 0.6,
            y0: 0.0,
            x1: 0.2,
            y1: 0.4,
        },
    });
    radial.components.push(MaskComponent {
        mode: MaskMode::Intersect,
        invert: false,
        shape: MaskShape::LuminanceRange {
            min: 15.0,
            max: 85.0,
            smoothness: 60.0,
        },
    });
    let mut brush = LocalAdjustment {
        delta: LocalDelta {
            exposure_ev: -0.7,
            saturation: 60.0,
            sharpness: -90.0,
            clarity: -40.0,
            ..Default::default()
        },
        ..LocalAdjustment::new(MaskComponent::add(MaskShape::Brush {
            strokes: vec![
                BrushStroke {
                    size: 0.12,
                    feather: 60.0,
                    flow: 70.0,
                    density: 90.0,
                    points: vec![[0.1, 0.1], [0.5, 0.25], [0.9, 0.5]],
                    ..Default::default()
                },
                BrushStroke {
                    erase: true,
                    size: 0.05,
                    feather: 30.0,
                    points: vec![[0.5, 0.25]],
                    ..Default::default()
                },
            ],
        }))
    };
    brush.components.push(MaskComponent {
        mode: MaskMode::Intersect,
        invert: true,
        shape: MaskShape::ColorRange {
            samples: vec![
                ColorSample {
                    x: 0.2,
                    y: 0.2,
                    lab: genzo_color::oklab::linear_bt2020_to_oklab([0.3, 0.12, 0.05]),
                },
                ColorSample {
                    x: 0.3,
                    y: 0.2,
                    lab: genzo_color::oklab::linear_bt2020_to_oklab([0.05, 0.1, 0.3]),
                },
            ],
            amount: 60.0,
        },
    });
    // ほぼ全体を覆い（線形の始まりより手前は重み 1）、円形のマスクと重なって色温度・色かぶりの和が ±100 を超える
    // マスク（和は ±100 に収める）。
    let warm = LocalAdjustment {
        delta: LocalDelta {
            temperature: 100.0,
            tint: -100.0,
            ..Default::default()
        },
        ..LocalAdjustment::new(MaskComponent::add(MaskShape::Linear {
            x0: 0.95,
            y0: 0.95,
            x1: 1.0,
            y1: 1.0,
        }))
    };
    // AI マスク（DEV-20。ラスターはブラシと同じ形で GPU に渡る）。反転して除く部品も付ける。
    let mut ai = LocalAdjustment {
        delta: LocalDelta {
            exposure_ev: 0.4,
            dehaze: -20.0,
            highlights: 30.0,
            ..Default::default()
        },
        ..LocalAdjustment::new(MaskComponent::add(test_ai_shape()))
    };
    ai.components.push(MaskComponent {
        mode: MaskMode::Subtract,
        invert: true,
        shape: MaskShape::Radial {
            cx: 0.5,
            cy: 0.3,
            rx: 0.2,
            ry: 0.2,
            angle_deg: 0.0,
            feather: 30.0,
        },
    });
    // 全体の値が 0 で、マスクだけで有効なステージ（コントラストは全体の値なし）。
    s.contrast = 0.0;
    s.masks = vec![radial, brush, warm, ai];
    s
}

#[test]
fn local_field_and_local_stages_match_the_cpu_version() {
    // 局所の値の面（local.wgsl の local_main）と、局所の値を引くステージ 10〜14 のカーネル（全体の値あり・マスクだけ）。
    use genzo_pipeline::FinishPlan;
    let g = gpu_or_skip!();
    let settings = masks_settings();
    let mut tested = Vec::new();
    with_context(&settings, |ctx, a1_image| {
        let plan = FinishPlan::new(&settings, ctx).unwrap();
        let local = plan.local().expect("マスクがある").clone();
        assert_eq!(local.slots.count(), 13);
        for stage in genzo_pipeline::finish::scene_stages() {
            let Some(params) = stage.params(&settings, ctx).unwrap() else {
                continue;
            };
            if stage.id() == "finish.geometry" {
                continue;
            }
            let (input, out) = case_for(stage, a1_image, ctx, &settings);
            // 局所の値の面: 入力のタイルの範囲で作る（出力を覆う）。
            let cpu_field = local.field(&input).unwrap();
            let gpu_field = g.local_field(&local, &input).unwrap();
            assert_eq!(gpu_field.roi, cpu_field.roi);
            let r = compare(
                &cpu_field.planes,
                &gpu_field.planes,
                stage_tolerance("local.field"),
            );
            eprintln!("local.field（{}）: {r}", stage.id());
            assert!(r.passed(), "local.field: {r}");
            assert!(
                cpu_field.planes.iter().any(|v| *v != 0.0),
                "マスクが効く画素がある"
            );
            let lctx = ctx.with_local(&cpu_field);
            let (c, n_cpu) = run_stage_cpu(stage, &lctx, &input, out, &params).unwrap();
            let (gp, n_gpu) = g
                .run_stage(stage, &lctx, &input, out, &params)
                .unwrap_or_else(|e| panic!("{}: {e}", stage.id()));
            let report =
                compare_pixels(c.image.data(), gp.image.data(), stage_tolerance(stage.id()));
            eprintln!("{}（ローカル補正）: {report}", stage.id());
            assert!(report.passed(), "{}: {report}", stage.id());
            assert_eq!(n_cpu, n_gpu, "{}", stage.id());
            tested.push(stage.id());
        }
    });
    for id in [
        "finish.exposure",
        "finish.contrast",
        "finish.tone",
        "finish.texture",
        "finish.color",
        "finish.sharpen",
    ] {
        assert!(tested.contains(&id), "{id} を比べていない: {tested:?}");
    }
}

/// ステージ 9 の遠近補正（変形。処理バージョン 2。DEV-16）の CPU 版と GPU 版の一致（歪曲補正・回転・切り抜きと
/// 重ねた場合と、切り抜く前の全体の表示を含む）。
#[test]
fn geometry_with_perspective_matches_the_cpu_version() {
    let g = gpu_or_skip!();
    let mut compared = 0;
    for (vertical, horizontal, scale, deg, distortion) in [
        (-45.0f32, 0.0f32, 100.0f32, 0.0f32, 0.0f32),
        (30.0, -25.0, 115.0, 4.0, 0.0),
        (-70.0, 20.0, 90.0, -8.0, 60.0),
    ] {
        let mut settings = settings_for_all_stages();
        settings.transform.vertical = vertical;
        settings.transform.horizontal = horizontal;
        settings.transform.scale = scale;
        settings.transform.offset_y = -10.0;
        settings.geometry.angle_deg = deg;
        settings.lens.distortion = distortion;
        with_context(&settings, |ctx, a1_image| {
            assert!(ctx.frames.has_perspective());
            let registry = StageRegistry::builtin();
            let stage = registry.find("finish.geometry").unwrap();
            for uncropped in [false, true] {
                let ctx = (*ctx).with_uncropped(uncropped);
                let params = stage.params(&settings, &ctx).unwrap().unwrap();
                let plan = params
                    .get::<genzo_pipeline::finish::GeometryPlan>()
                    .unwrap();
                let pr = plan.projective().expect("遠近補正の写像");
                assert!(plan.warp().is_none());
                assert_eq!(pr.lut.is_some(), distortion != 0.0);
                let (w, h) = plan.output_size();
                for out in [Roi::new(2, 3, w - 4, h - 5), Roi::full(w, h)] {
                    let need = stage.input_roi(out, &params, &ctx);
                    let input = tile_of(a1_image, need);
                    let (cpu, gpu, n_cpu, n_gpu) = run_both(g, stage, &ctx, &settings, &input, out);
                    let report = compare_pixels(
                        cpu.image.data(),
                        gpu.image.data(),
                        stage_tolerance("finish.geometry"),
                    );
                    eprintln!(
                        "変形 {vertical}/{horizontal} 回転 {deg} 歪曲 {distortion} 全体 {uncropped}: {report}"
                    );
                    assert!(
                        report.passed(),
                        "{vertical} {horizontal} {deg} {distortion} {uncropped}: {report}"
                    );
                    assert_eq!(n_cpu, n_gpu);
                    compared += 1;
                }
            }
        });
    }
    assert_eq!(compared, 3 * 2 * 2);
}
