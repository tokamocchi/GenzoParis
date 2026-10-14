//! プレビュー・書き出しの GPU 版と CPU 版の比較（02 の IQ-07a「最終出力では 8bit 換算で ±1 以内」、
//! docs/04_architecture.md の 2.2 節・2.4 節・6.1 節・6.3 節・7.1 節）。
//!
//! GPU のアダプターがない環境と CI のランナーではスキップする（`GENZO_REQUIRE_GPU=1` なら実行し、
//! アダプターがなければ失敗。tests/common の doc）。

mod common;

use std::sync::atomic::{AtomicU64, Ordering};

use genzo_gpu::{
    ExportBackend, FINAL_OUTPUT_MAX_DIFF_8BIT, GpuError, GpuPreviewOptions, GpuRenderer,
    GpuStageRegistry, StageTolerance, compare_pixels, export_with_fallback, max_diff_8bit,
};
use genzo_model::{
    CropRect, DevelopSettings, ExportFormat, ExportSettings, ExportSize, Orientation,
    OutputColorSpace, RenderQuality,
};
use genzo_pipeline::Stage;
use genzo_pipeline::engine::StageBackend;
use genzo_pipeline::finish::output::{Dither, Histogram, quantize_u8, quantize_u16};
use genzo_pipeline::{
    ColorContract, Engine, ExportOptions, ExportPixels, NeverCancel, OutputTarget, PhotoSource,
    PipelineError, PreviewRequest, RenderControl, RgbImage,
};
use genzo_raw::CfaPattern;

use common::*;

/// B3 どうしの比較の許容誤差（ステージ 9〜16 の差が積み重なったもの）。**仮置き**（このコンテナの
/// llvmpipe で測った差に余裕を持たせた値。PoC-3 で実機の差を測って決める）。
const B3_TOLERANCE: StageTolerance = StageTolerance::new(2e-4, 1e-3);

/// 段階 A1（センサー処理と縮小）の比較の許容誤差。**仮置き**（同上）。
const A1_TOLERANCE: StageTolerance = StageTolerance::new(1e-6, 1e-5);

fn rotated_strong() -> DevelopSettings {
    let mut s = strong_settings();
    rotate_and_crop(
        &mut s,
        -3.0,
        CropRect {
            left: 0.05,
            top: 0.08,
            right: 0.93,
            bottom: 0.95,
        },
    );
    s
}

/// CPU 版と GPU 版のプレビューを描いて比べる（最終出力は 8bit 換算で ±1 以内）。
fn compare_preview(
    g: &GpuRenderer,
    engine: &Engine,
    source: &PhotoSource,
    settings: &DevelopSettings,
    quality: RenderQuality,
    long_edge: u32,
    target: OutputTarget,
) -> genzo_gpu::GpuPreviewRender {
    let request = PreviewRequest::new(quality, long_edge)
        .with_output(target)
        .with_histogram();
    let cpu = engine
        .render_preview_with(source, settings, &request)
        .unwrap();
    let gpu = g
        .render_preview(
            engine,
            source,
            settings,
            &request,
            &GpuPreviewOptions::default(),
        )
        .unwrap();
    let b3 = gpu.b3.as_ref().unwrap();
    assert_eq!(b3.roi, cpu.b3.roi);
    assert_eq!(gpu.active_stages, cpu.active_stages);
    assert_eq!(gpu.used_guide, cpu.used_guide);
    let r = compare_pixels(cpu.b3.image.data(), b3.image.data(), B3_TOLERANCE);
    eprintln!("B3: {r}");
    assert!(r.passed(), "B3: {r}");
    let (co, go) = (cpu.output.unwrap(), gpu.output.clone().unwrap());
    assert_eq!(co.contract(), go.contract());
    let d = max_diff_8bit(co.as_flat(), go.as_flat());
    eprintln!("最終出力の 8bit 換算の差の最大: {d:.4}");
    assert!(d <= FINAL_OUTPUT_MAX_DIFF_8BIT, "{d}");
    assert!(go.as_flat().iter().all(|v| v.is_finite()));
    // ヒストグラム: 画素数は同じ。値のわずかな差で階級の境目の画素が隣に移ることはある。
    let (ch, gh) = (cpu.histogram.unwrap(), gpu.histogram.clone().unwrap());
    assert_eq!(ch.pixel_count, gh.pixel_count);
    let moved = histogram_moved(&ch, &gh);
    assert!(
        moved as f64 <= ch.pixel_count as f64 * 0.01,
        "ヒストグラムの差 {moved} 画素"
    );
    gpu
}

/// 2 つのヒストグラムで、階級を移った画素の数（チャンネルごとの差の和の半分の最大）。
fn histogram_moved(a: &Histogram, b: &Histogram) -> u64 {
    [
        (&a.red, &b.red),
        (&a.green, &b.green),
        (&a.blue, &b.blue),
        (&a.luma, &b.luma),
    ]
    .iter()
    .map(|(x, y)| {
        x.iter()
            .zip(y.iter())
            .map(|(p, q)| p.abs_diff(*q))
            .sum::<u64>()
            / 2
    })
    .max()
    .unwrap()
}

#[test]
fn preview_of_a_working_image_matches_the_cpu_version() {
    let g = gpu_or_skip!();
    let engine = engine(160, 64);
    let source = working_source(scene_b2(300, 200), Orientation::Rotate90Cw);
    for target in [
        OutputTarget::Export(OutputColorSpace::Srgb),
        OutputTarget::Export(OutputColorSpace::AdobeRgb),
        OutputTarget::CacheDisplayP3,
        OutputTarget::assumed_srgb_display().unwrap(),
    ] {
        let p = compare_preview(
            g,
            &engine,
            &source,
            &rotated_strong(),
            RenderQuality::Final,
            160,
            target,
        );
        // すべてのステージを GPU で処理した。
        assert!(
            p.backends.iter().all(|(_, b)| *b == StageBackend::Gpu),
            "{:?}",
            p.backends
        );
        assert!(!p.a1_on_gpu);
    }
}

#[test]
fn preview_of_a_raw_with_default_crop_matches_the_cpu_version() {
    let g = gpu_or_skip!();
    let engine = engine(90, 64);
    let source = raw_source_with_id(
        raw_with_crop_and_plateau(&camera_scene(180, 124), CfaPattern::RGGB),
        101,
    );
    let mut s = rotated_strong();
    custom_wb(&mut s, 5200.0, -6.0);
    for quality in [RenderQuality::Final, RenderQuality::Draft] {
        let p = compare_preview(
            g,
            &engine,
            &source,
            &s,
            quality,
            90,
            OutputTarget::Export(OutputColorSpace::Srgb),
        );
        assert_eq!(p.quality, quality);
    }
}

#[test]
fn preview_of_a_raw_matches_the_cpu_version_in_both_qualities() {
    let g = gpu_or_skip!();
    let engine = engine(90, 64);
    let raw = raw_from(&camera_scene(180, 124), CfaPattern::RGGB, true);
    let source = raw_source(raw);
    let mut s = rotated_strong();
    custom_wb(&mut s, 5200.0, -6.0);
    for quality in [RenderQuality::Final, RenderQuality::Draft] {
        let p = compare_preview(
            g,
            &engine,
            &source,
            &s,
            quality,
            90,
            OutputTarget::Export(OutputColorSpace::Srgb),
        );
        assert_eq!(p.a1_on_gpu, quality == RenderQuality::Draft);
        assert_eq!(p.quality, quality);
    }
}

#[test]
fn draft_a1_on_the_gpu_matches_the_cpu_version() {
    let g = gpu_or_skip!();
    // 奇数の寸法（2 × 2 の格子で最後の列・行を使わない）、4 つの CFA の並び、縮小あり・なし。
    for (w, h, long_edge) in [(67u32, 51u32, 24u32), (64, 48, 64), (90, 61, 40)] {
        for (cfa, crop) in CfaPattern::ALL
            .into_iter()
            .flat_map(|c| [(c, false), (c, true)])
        {
            // エンジンの A1 のキャッシュのキーは元ファイルの ID なので、RAW ごとにエンジンを作る。
            let engine = engine(64, 64);
            // 既定の切り抜き・閾値が白レベルより低い飽和の閾値（A-2・A-3）のある RAW も比べる。
            let raw = if crop {
                raw_with_crop_and_plateau(&camera_scene(w, h), cfa)
            } else {
                raw_from(&camera_scene(w, h), cfa, true)
            };
            let source = raw_source(raw);
            let mut s = DevelopSettings::default();
            custom_wb(&mut s, 3800.0, 4.0);
            let cpu = engine
                .a1(&source, &s, RenderQuality::Draft, long_edge)
                .unwrap();
            let genzo_pipeline::SourceImage::Raw(raw) = &source.image else {
                unreachable!()
            };
            let gpu = g
                .render_a1_draft(raw, &s, &engine.config().sensor, long_edge)
                .unwrap();
            assert_eq!(gpu.image.dimensions(), cpu.image.dimensions());
            assert_eq!(gpu.sensor_extent, cpu.sensor_extent);
            assert_eq!(gpu.sensor_size, cpu.sensor_size);
            assert_eq!(gpu.orientation, cpu.orientation);
            assert_eq!(gpu.default_crop, cpu.default_crop);
            assert_eq!(gpu.frames(), cpu.frames());
            assert_eq!(gpu.quality, RenderQuality::Draft);
            assert_eq!(gpu.warnings, cpu.warnings);
            let r = compare_pixels(cpu.image.data(), gpu.image.data(), A1_TOLERANCE);
            assert!(r.passed(), "{w}×{h} {cfa}: {r}");
        }
    }
}

#[test]
fn draft_a1_is_cached_and_reuses_the_raw_on_the_gpu() {
    let g = gpu_or_skip!();
    let engine = engine(48, 64);
    let source = raw_source(raw_from(&camera_scene(96, 64), CfaPattern::GRBG, false));
    let mut s = DevelopSettings::default();
    s.tone.shadows = 40.0;
    let request = PreviewRequest::new(RenderQuality::Draft, 48);
    let opts = GpuPreviewOptions::default();
    let before = g.cache_counters();
    let a = g
        .render_preview(&engine, &source, &s, &request, &opts)
        .unwrap();
    // 露光量だけを変えた: A1 とガイドはキャッシュから。
    s.exposure_ev = 0.7;
    let b = g
        .render_preview(&engine, &source, &s, &request, &opts)
        .unwrap();
    let after = g.cache_counters();
    assert!(after.draft_a1_misses > before.draft_a1_misses);
    assert!(after.draft_a1_hits > before.draft_a1_hits);
    assert!(after.draft_guide_hits > before.draft_guide_hits);
    assert!(a.used_guide && b.used_guide);
    assert_ne!(a.b3.as_ref().unwrap().image, b.b3.as_ref().unwrap().image);
    // 既定の設定の入口（B3 を読み出す）。
    let d = genzo_gpu::render_preview_gpu(g, &engine, &source, &s, &request).unwrap();
    assert!(d.b3.is_some() && d.a1_on_gpu);
    // B3 を読み出さない設定。
    let c = g
        .render_preview(
            &engine,
            &source,
            &s,
            &request,
            &GpuPreviewOptions {
                download_b3: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(c.b3.is_none());
    assert_eq!(c.b3_gpu.roi, a.b3.as_ref().unwrap().roi);
    assert_eq!(c.b3_gpu.contract, ColorContract::B3DisplayReferred);
    assert!(c.output_gpu.is_none());
    // 出力を読み出さない設定でも、GPU 上の出力とヒストグラムは返す。
    let with_output = PreviewRequest::new(RenderQuality::Draft, 48)
        .with_output(OutputTarget::CacheDisplayP3)
        .with_histogram();
    let o = g
        .render_preview(
            &engine,
            &source,
            &s,
            &with_output,
            &GpuPreviewOptions {
                download_output: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(o.output.is_none());
    let og = o.output_gpu.unwrap();
    assert_eq!(og.contract, ColorContract::B5Cache);
    assert_eq!(o.histogram.unwrap().pixel_count, og.roi.pixel_count());
    // ヒストグラムには出力先が必要（CPU 版と同じエラー）。
    let bad = PreviewRequest::new(RenderQuality::Final, 48).with_histogram();
    assert!(matches!(
        g.render_preview(&engine, &source, &s, &bad, &opts),
        Err(GpuError::Pipeline(PipelineError::InvalidArgument(_)))
    ));
}

#[test]
fn stages_without_a_gpu_version_run_on_the_cpu() {
    // 7.1 節: GPU 版のないステージは CPU 版で処理する（混在できる）。途中のステージ（トーン・色）と、
    // 最初のステージ（ジオメトリ）を CPU 版にした場合。
    let base = gpu_or_skip!();
    let engine = engine(120, 64);
    let source = working_source(scene_b2(160, 120), Orientation::Normal);
    for cpu_only in [
        &["finish.tone", "finish.color"][..],
        &["finish.geometry", "finish.tone_curve"][..],
    ] {
        let ctx =
            genzo_gpu::GpuContext::new(&genzo_gpu::GpuContextOptions::from_env_allowing_software())
                .unwrap()
                .unwrap();
        let mut registry = GpuStageRegistry::builtin();
        for id in cpu_only {
            registry = registry.without(id);
        }
        let g = GpuRenderer::new(ctx).unwrap().with_registry(registry);
        let p = compare_preview(
            &g,
            &engine,
            &source,
            &rotated_strong(),
            RenderQuality::Final,
            120,
            OutputTarget::Export(OutputColorSpace::DisplayP3),
        );
        assert_eq!(p.backends.len(), p.active_stages.len() + 1);
        for (id, b) in &p.backends {
            let expect = if cpu_only.contains(id) {
                StageBackend::Cpu
            } else {
                StageBackend::Gpu
            };
            assert_eq!(*b, expect, "{id}");
        }
        assert!(base.registry().ids().len() > g.registry().ids().len());
    }
}

#[test]
fn histogram_counts_match_the_cpu_version_on_the_same_input() {
    let g = gpu_or_skip!();
    let img = RgbImage::from_fn(123, 77, ColorContract::B4bExport, |x, y| {
        let h = x.wrapping_mul(2_654_435_761u32.wrapping_mul(y + 1)) % 1000;
        [
            h as f32 / 999.0,
            (x as f32 / 122.0) * 1.1 - 0.05,
            if (x + y) % 5 == 0 {
                1.0
            } else {
                y as f32 / 76.0
            },
        ]
    })
    .unwrap();
    let luma = Histogram::luma_coefficients(genzo_color::RgbColorSpace::Srgb);
    let cpu = Histogram::compute(&img, luma).unwrap();
    let gpu = g.histogram(&img, luma).unwrap();
    assert_eq!(gpu.red, cpu.red);
    assert_eq!(gpu.green, cpu.green);
    assert_eq!(gpu.blue, cpu.blue);
    assert_eq!(gpu.clipped_high, cpu.clipped_high);
    assert_eq!(gpu.clipped_low, cpu.clipped_low);
    assert_eq!(gpu.pixel_count, cpu.pixel_count);
    // 輝度は積和の融合で階級の境目の画素が移りうる（ごくわずか）。
    let moved: u64 = cpu
        .luma
        .iter()
        .zip(gpu.luma.iter())
        .map(|(a, b)| a.abs_diff(*b))
        .sum::<u64>()
        / 2;
    assert!(moved <= cpu.pixel_count / 1000, "{moved}");
    // リニアの値は扱わない。
    let lin = RgbImage::filled(2, 2, ColorContract::B3DisplayReferred, [0.5; 3]).unwrap();
    assert!(g.histogram(&lin, luma).is_err());
}

#[test]
fn quantization_matches_the_cpu_version() {
    let g = gpu_or_skip!();
    // 端数のある寸法（1 スレッド 4 画素・2 画素で割り切れない）。
    for (w, h) in [(37u32, 11u32), (5, 3), (64, 2)] {
        let img = RgbImage::from_fn(w, h, ColorContract::B4bExport, |x, y| {
            let v = ((x * 31 + y * 17) % 257) as f32 / 256.0;
            [v, 1.0 - v, if x == 0 { 0.0 } else { (v * 0.5) + 0.25 }]
        })
        .unwrap();
        for origin in [(0i64, 0i64), (1024, -7), (-33, 4096)] {
            for dither in [
                Dither::None,
                Dither::DEFAULT_8BIT,
                Dither::Triangular { seed: 9 },
            ] {
                let cpu = quantize_u8(&img, origin, dither).unwrap();
                let gpu = g.quantize_u8(&img, origin, dither).unwrap();
                assert_eq!(cpu.len(), gpu.len());
                let diff: Vec<_> = cpu
                    .iter()
                    .zip(&gpu)
                    .filter(|(a, b)| a != b)
                    .map(|(a, b)| (*a, *b))
                    .collect();
                assert!(diff.iter().all(|(a, b)| a.abs_diff(*b) <= 1), "{diff:?}");
                assert!(
                    diff.len() * 1000 <= cpu.len().max(1000),
                    "{w}×{h} {origin:?}: {diff:?}"
                );
                let cpu16 = quantize_u16(&img, origin, dither).unwrap();
                let gpu16 = g.quantize_u16(&img, origin, dither).unwrap();
                assert_eq!(cpu16.len(), gpu16.len());
                assert!(cpu16.iter().zip(&gpu16).all(|(a, b)| a.abs_diff(*b) <= 1));
            }
        }
    }
    let lin = RgbImage::filled(2, 2, ColorContract::B2Working, [0.5; 3]).unwrap();
    assert!(g.quantize_u8(&lin, (0, 0), Dither::None).is_err());
}

/// 書き出しの画素の 8bit 換算の差の最大。
fn export_diff_8bit(a: &ExportPixels, b: &ExportPixels) -> f64 {
    match (a, b) {
        (ExportPixels::Rgb8(x), ExportPixels::Rgb8(y)) => {
            assert_eq!(x.len(), y.len());
            x.iter()
                .zip(y)
                .map(|(p, q)| f64::from(p.abs_diff(*q)))
                .fold(0.0, f64::max)
        }
        (ExportPixels::Rgb16(x), ExportPixels::Rgb16(y)) => {
            assert_eq!(x.len(), y.len());
            x.iter()
                .zip(y)
                .map(|(p, q)| f64::from(p.abs_diff(*q)) / 257.0)
                .fold(0.0, f64::max)
        }
        _ => panic!("ビット数が違う"),
    }
}

/// ハイライト復元・ノイズ軽減（処理バージョン 2）のある RAW: GPU 版の簡易処理の A1（ステージ 4・6 を含む）・
/// プレビュー・書き出し（センサー処理は CPU 版）が CPU 版と一致する。
#[test]
fn highlight_recovery_and_noise_reduction_match_the_cpu_version() {
    let g = gpu_or_skip!();
    let raw = raw_from(&camera_scene(150, 104), CfaPattern::GRBG, true);
    let source = raw_source_with_id(raw, 78);
    let mut s = rotated_strong();
    s.highlight_recovery = genzo_model::HighlightRecovery {
        method: genzo_model::HighlightMethod::Reconstruct,
        amount: 80.0,
    };
    s.detail.noise_reduction = genzo_model::NoiseReduction {
        luminance: 35.0,
        color: 45.0,
        luminance_contrast: 20.0,
        ..Default::default()
    };
    let engine = engine(90, 64);
    let cpu = engine.a1(&source, &s, RenderQuality::Draft, 64).unwrap();
    let genzo_pipeline::SourceImage::Raw(r) = &source.image else {
        unreachable!()
    };
    let gpu = g
        .render_a1_draft(r, &s, &engine.config().sensor, 64)
        .unwrap();
    assert_eq!(gpu.frames(), cpu.frames());
    let d = compare_pixels(cpu.image.data(), gpu.image.data(), A1_TOLERANCE);
    assert!(d.passed(), "A1: {d}");
    // 設定を変えると結果も変わる（ステージ 4・6 が効いている）。
    let plain = g
        .render_a1_draft(r, &rotated_strong(), &engine.config().sensor, 64)
        .unwrap();
    assert_ne!(plain.image.data(), gpu.image.data());
    for quality in [RenderQuality::Final, RenderQuality::Draft] {
        compare_preview(
            g,
            &engine,
            &source,
            &s,
            quality,
            90,
            OutputTarget::Export(OutputColorSpace::Srgb),
        );
    }
    let export = ExportSettings::default();
    let c = engine
        .export(
            &source,
            &s,
            &export,
            &ExportOptions::default(),
            &NeverCancel,
        )
        .unwrap();
    let e = g
        .export(
            &engine,
            &source,
            &s,
            &export,
            &ExportOptions::default(),
            &NeverCancel,
        )
        .unwrap();
    let diff = export_diff_8bit(&c.pixels, &e.pixels);
    assert!(diff <= FINAL_OUTPUT_MAX_DIFF_8BIT, "{diff}");
}

/// レンズ補正（処理バージョン 2。埋め込みの情報と手動の補正）のある RAW: GPU 版の簡易処理の A1（ステージ 7
/// を含む）・プレビュー（ステージ 9 の歪曲補正を含む）・書き出しが CPU 版と一致する。
#[test]
fn lens_corrections_match_the_cpu_version() {
    let g = gpu_or_skip!();
    let mut raw = raw_with_crop_and_plateau(&camera_scene(150, 104), CfaPattern::RGGB);
    raw.lens_correction = Some(genzo_raw::LensCorrectionData::SonyArw(
        genzo_raw::SonyLensCorrection {
            distortion: (0..16).map(|i| (-50 * i) as i16).collect(),
            vignetting: (0..16).map(|i| (250 * i) as i16).collect(),
            ca_red: (0..16).map(|i| (600 * i) as i16).collect(),
            ca_blue: (0..16).map(|i| (-400 * i) as i16).collect(),
        },
    ));
    let source = raw_source_with_id(raw, 77);
    let mut s = rotated_strong();
    s.lens.profile_enabled = true;
    s.lens.remove_chromatic_aberration = true;
    s.lens.distortion = 25.0;
    s.lens.vignetting_amount = 30.0;
    s.lens.ca_red_cyan = 40.0;
    let engine = engine(90, 64);
    // 簡易処理の A1（GPU 版のステージ 7）。
    let cpu = engine.a1(&source, &s, RenderQuality::Draft, 64).unwrap();
    let genzo_pipeline::SourceImage::Raw(r) = &source.image else {
        unreachable!()
    };
    let gpu = g
        .render_a1_draft(r, &s, &engine.config().sensor, 64)
        .unwrap();
    assert_eq!(gpu.frames(), cpu.frames());
    assert!(gpu.lens_geometry.is_some());
    let d = compare_pixels(cpu.image.data(), gpu.image.data(), A1_TOLERANCE);
    assert!(d.passed(), "A1: {d}");
    for quality in [RenderQuality::Final, RenderQuality::Draft] {
        compare_preview(
            g,
            &engine,
            &source,
            &s,
            quality,
            90,
            OutputTarget::Export(OutputColorSpace::Srgb),
        );
    }
    let export = ExportSettings::default();
    let c = engine
        .export(
            &source,
            &s,
            &export,
            &ExportOptions::default(),
            &NeverCancel,
        )
        .unwrap();
    let e = g
        .export(
            &engine,
            &source,
            &s,
            &export,
            &ExportOptions::default(),
            &NeverCancel,
        )
        .unwrap();
    let diff = export_diff_8bit(&c.pixels, &e.pixels);
    assert!(diff <= FINAL_OUTPUT_MAX_DIFF_8BIT, "{diff}");
}

#[test]
fn color_mixer_grading_and_black_white_match_the_cpu_version() {
    // ステージ 13 の処理バージョン 2 の項目（カラーミキサー・カラーグレーディング・白黒。DEV-09・DEV-10・DEV-25）を
    // 含むプレビュー（RAW・RAW 以外）と書き出し。
    use genzo_model::{GradingWheel, HslAdjustment};
    let g = gpu_or_skip!();
    let engine = engine(80, 48);
    let sources = [
        working_source(scene_b2(150, 100), Orientation::Normal),
        raw_source(raw_from(&camera_scene(140, 96), CfaPattern::RGGB, true)),
    ];
    for bw in [false, true] {
        let mut s = rotated_strong();
        s.color.mixer.orange = HslAdjustment {
            hue: 30.0,
            saturation: -40.0,
            luminance: 40.0,
        };
        s.color.mixer.aqua.hue = -70.0;
        s.color.grading.shadows = GradingWheel {
            hue: 200.0,
            saturation: 40.0,
            luminance: 15.0,
        };
        s.color.grading.highlights = GradingWheel {
            hue: 45.0,
            saturation: 30.0,
            luminance: 0.0,
        };
        s.color.black_white.enabled = bw;
        s.color.black_white.mix.orange = 60.0;
        s.color.black_white.mix.blue = -80.0;
        for source in &sources {
            let p = compare_preview(
                g,
                &engine,
                source,
                &s,
                RenderQuality::Final,
                80,
                OutputTarget::Export(OutputColorSpace::Srgb),
            );
            assert!(p.active_stages.contains(&"finish.color"));
            let export = ExportSettings::default();
            let c = engine
                .export(source, &s, &export, &ExportOptions::default(), &NeverCancel)
                .unwrap();
            let e = g
                .export(
                    &engine,
                    source,
                    &s,
                    &export,
                    &ExportOptions::default(),
                    &NeverCancel,
                )
                .unwrap();
            let diff = export_diff_8bit(&c.pixels, &e.pixels);
            eprintln!("白黒 {bw}: 書き出しの 8bit 換算の差の最大 {diff:.3}");
            assert!(diff <= FINAL_OUTPUT_MAX_DIFF_8BIT, "{diff}");
        }
    }
}

#[test]
fn presence_sharpening_and_effects_match_the_cpu_version() {
    // 処理バージョン 2 のステージ 12（テクスチャ・明瞭度・かすみの除去）・14（シャープ）・16 の効果（周辺光量・粒子）を
    // 含むプレビュー（RAW・RAW 以外）と書き出し（タイルの余白・ガイドとかすみの推定・写真ごとの種）。
    let g = gpu_or_skip!();
    let engine = engine(80, 48);
    let sources = [
        working_source(scene_b2(150, 100), Orientation::Normal),
        raw_source(raw_from(&camera_scene(140, 96), CfaPattern::RGGB, true)),
    ];
    let mut s = rotated_strong();
    s.presence.texture = 50.0;
    s.presence.clarity = 40.0;
    s.presence.dehaze = 35.0;
    s.detail.sharpening.amount = 100.0;
    s.detail.sharpening.radius = 1.5;
    s.detail.sharpening.masking = 25.0;
    s.effects.vignette.amount = -50.0;
    s.effects.vignette.midpoint = 30.0;
    s.effects.grain.amount = 35.0;
    s.effects.grain.size = 60.0;
    for source in &sources {
        let p = compare_preview(
            g,
            &engine,
            source,
            &s,
            RenderQuality::Final,
            80,
            OutputTarget::Export(OutputColorSpace::Srgb),
        );
        for id in ["finish.texture", "finish.sharpen", "finish.effects"] {
            assert!(p.active_stages.contains(&id), "{id}");
        }
        assert!(
            p.backends
                .iter()
                .filter(
                    |(id, _)| ["finish.texture", "finish.sharpen", "finish.effects"].contains(id)
                )
                .all(|(_, b)| *b == StageBackend::Gpu),
            "{:?}",
            p.backends
        );
        let export = ExportSettings::default();
        let c = engine
            .export(source, &s, &export, &ExportOptions::default(), &NeverCancel)
            .unwrap();
        let e = g
            .export(
                &engine,
                source,
                &s,
                &export,
                &ExportOptions::default(),
                &NeverCancel,
            )
            .unwrap();
        let diff = export_diff_8bit(&c.pixels, &e.pixels);
        eprintln!("ステージ 12・14・16: 書き出しの 8bit 換算の差の最大 {diff:.3}");
        assert!(diff <= FINAL_OUTPUT_MAX_DIFF_8BIT, "{diff}");
    }
}

#[test]
fn export_matches_the_cpu_version() {
    let g = gpu_or_skip!();
    let engine = engine(80, 48);
    let sources = [
        working_source(scene_b2(150, 100), Orientation::Rotate180),
        raw_source(raw_from(&camera_scene(140, 96), CfaPattern::BGGR, true)),
        // 既定の切り抜き・向き 8・閾値が白レベルより低い飽和の閾値（A-2・A-3）。エンジンのキャッシュの
        // キーは元ファイルの ID なので、上の RAW と別の ID にする。
        raw_source_with_id(
            raw_with_crop_and_plateau(&camera_scene(140, 96), CfaPattern::GRBG),
            102,
        ),
    ];
    let exports = [
        ExportSettings::default(),
        ExportSettings {
            format: ExportFormat::Tiff16,
            color_space: OutputColorSpace::DisplayP3,
            ..Default::default()
        },
        ExportSettings {
            size: ExportSize::LongEdge(61),
            color_space: OutputColorSpace::AdobeRgb,
            ..Default::default()
        },
    ];
    for source in &sources {
        for export in &exports {
            let s = rotated_strong();
            let cpu = engine
                .export(source, &s, export, &ExportOptions::default(), &NeverCancel)
                .unwrap();
            let gpu = g
                .export(
                    &engine,
                    source,
                    &s,
                    export,
                    &ExportOptions::default(),
                    &NeverCancel,
                )
                .unwrap();
            assert_eq!((gpu.width, gpu.height), (cpu.width, cpu.height));
            assert_eq!(gpu.full_resolution_size, cpu.full_resolution_size);
            assert_eq!(gpu.color_space, cpu.color_space);
            assert_eq!(gpu.icc.as_bytes(), cpu.icc.as_bytes());
            assert_eq!(gpu.warnings, cpu.warnings);
            let d = export_diff_8bit(&cpu.pixels, &gpu.pixels);
            eprintln!("{export:?}: 8bit 換算の差の最大 {d:.3}");
            assert!(d <= FINAL_OUTPUT_MAX_DIFF_8BIT, "{export:?}: {d}");
            // 処理した側: センサーのステージは CPU 版、段階 C と 17b は GPU 版。
            let job = engine.full_resolution(source, &s).unwrap();
            let backends = g.export_backends(&job, export);
            for (id, b) in &backends {
                let expect = if id.starts_with("sensor.") {
                    StageBackend::Cpu
                } else {
                    StageBackend::Gpu
                };
                assert_eq!(*b, expect, "{id}");
            }
            assert_eq!(
                backends.last().unwrap().0,
                genzo_pipeline::finish::ExportStage::for_space(export.color_space).id()
            );
        }
    }
}

#[test]
fn gpu_export_does_not_depend_on_the_tile_size() {
    let g = gpu_or_skip!();
    let engine = engine(80, 64);
    let source = working_source(scene_b2(130, 90), Orientation::Normal);
    let s = rotated_strong();
    for export in [
        ExportSettings::default(),
        ExportSettings {
            size: ExportSize::LongEdge(50),
            ..Default::default()
        },
    ] {
        let mut results = Vec::new();
        for tile in [16u32, 37, 512] {
            let opts = ExportOptions {
                tile_size: Some(tile),
                ..Default::default()
            };
            results.push(
                g.export(&engine, &source, &s, &export, &opts, &NeverCancel)
                    .unwrap()
                    .pixels,
            );
        }
        assert!(results.windows(2).all(|w| w[0] == w[1]), "{export:?}");
    }
}

/// 進捗を数え、`cancel_after` タイルの後で取り消す。
struct CancelAfter {
    done: AtomicU64,
    cancel_after: u64,
}

impl RenderControl for CancelAfter {
    fn is_cancelled(&self) -> bool {
        self.done.load(Ordering::SeqCst) >= self.cancel_after
    }

    fn on_progress(&self, done: u64, _total: u64) {
        self.done.store(done, Ordering::SeqCst);
    }
}

#[test]
fn export_reports_progress_and_can_be_cancelled() {
    let g = gpu_or_skip!();
    let engine = engine(80, 32);
    let source = working_source(scene_b2(100, 70), Orientation::Normal);
    let s = DevelopSettings::default();
    let export = ExportSettings::default();
    let opts = ExportOptions::default();
    let all = CancelAfter {
        done: AtomicU64::new(0),
        cancel_after: u64::MAX,
    };
    g.export(&engine, &source, &s, &export, &opts, &all)
        .unwrap();
    // 100 × 70 をタイル 32 で: 4 × 3 = 12 タイル。
    assert_eq!(all.done.load(Ordering::SeqCst), 12);
    let cancel = CancelAfter {
        done: AtomicU64::new(0),
        cancel_after: 2,
    };
    let err = g
        .export(&engine, &source, &s, &export, &opts, &cancel)
        .unwrap_err();
    assert!(err.is_cancelled(), "{err}");
    // 取り消しは CPU 版に切り替えない。
    let err =
        export_with_fallback(Some(g), &engine, &source, &s, &export, &opts, &cancel).unwrap_err();
    assert!(matches!(err, PipelineError::Cancelled), "{err}");
    // 設定の誤りも切り替えない。
    let bad = ExportSettings {
        size: ExportSize::LongEdge(0),
        ..Default::default()
    };
    assert!(
        export_with_fallback(Some(g), &engine, &source, &s, &bad, &opts, &NeverCancel).is_err()
    );
}

#[test]
fn export_falls_back_to_the_cpu_version() {
    let g = gpu_or_skip!();
    let engine = engine(80, 64);
    let source = working_source(scene_b2(90, 60), Orientation::Normal);
    let s = strong_settings();
    let export = ExportSettings::default();
    let opts = ExportOptions::default();
    let cpu =
        export_with_fallback(None, &engine, &source, &s, &export, &opts, &NeverCancel).unwrap();
    assert_eq!(
        cpu.backend,
        ExportBackend::Cpu {
            gpu_error: None,
            gpu_failure: false
        }
    );
    let gpu =
        export_with_fallback(Some(g), &engine, &source, &s, &export, &opts, &NeverCancel).unwrap();
    assert_eq!(gpu.backend, ExportBackend::Gpu);
    assert!(export_diff_8bit(&cpu.image.pixels, &gpu.image.pixels) <= FINAL_OUTPUT_MAX_DIFF_8BIT);
    // デバイスを失った GPU（6.3 節）: CPU 版で書き出し、理由を返す。
    let lost = common::renderer().unwrap().unwrap();
    lost.context().destroy();
    assert!(lost.context().is_lost());
    let out = export_with_fallback(
        Some(&lost),
        &engine,
        &source,
        &s,
        &export,
        &opts,
        &NeverCancel,
    )
    .unwrap();
    match &out.backend {
        ExportBackend::Cpu {
            gpu_error: Some(e),
            gpu_failure: true,
        } => assert!(e.contains("失われ"), "{e}"),
        b => panic!("{b:?}"),
    }
    assert_eq!(out.image.pixels, cpu.image.pixels);
}

#[test]
fn a_lost_device_returns_errors_instead_of_panicking() {
    let _ = gpu_or_skip!();
    let r = common::renderer().unwrap().unwrap();
    let engine = engine(40, 64);
    let source = working_source(scene_b2(40, 30), Orientation::Normal);
    let s = strong_settings();
    let request = PreviewRequest::new(RenderQuality::Final, 40)
        .with_output(OutputTarget::Export(OutputColorSpace::Srgb));
    r.render_preview(
        &engine,
        &source,
        &s,
        &request,
        &GpuPreviewOptions::default(),
    )
    .unwrap();
    r.context().destroy();
    let err = r
        .render_preview(
            &engine,
            &source,
            &s,
            &request,
            &GpuPreviewOptions::default(),
        )
        .unwrap_err();
    assert!(err.is_device_lost(), "{err}");
    assert!(err.should_fall_back_to_cpu());
    let img = RgbImage::filled(4, 4, ColorContract::B4bExport, [0.5; 3]).unwrap();
    assert!(
        r.histogram(&img, [0.3, 0.6, 0.1])
            .unwrap_err()
            .is_device_lost()
    );
}

#[test]
fn display_profiles_with_unknown_matrices_run_on_the_cpu() {
    // 17a: 行列のわからないモニターの色域は CPU 版で処理し（混在）、プロファイルを登録すれば GPU 版で
    // 処理する。
    let _ = gpu_or_skip!();
    let r = common::renderer().unwrap().unwrap();
    let engine = engine(64, 64);
    let source = working_source(scene_b2(64, 48), Orientation::Normal);
    let s = strong_settings();
    let icc = genzo_color::IccProfile::standard(genzo_color::StandardProfile::DisplayP3).unwrap();
    let profile = genzo_color::DisplayProfile::resolve(Some(icc.as_bytes())).unwrap();
    assert!(!profile.is_assumed_srgb());
    let transform = std::sync::Arc::new(
        genzo_pipeline::finish::output::DisplayTransform::new(&profile, 17).unwrap(),
    );
    let target = OutputTarget::Display(transform);
    let before = compare_preview(
        &r,
        &engine,
        &source,
        &s,
        RenderQuality::Final,
        64,
        target.clone(),
    );
    assert_eq!(
        before.backends.last(),
        Some(&("output.display", StageBackend::Cpu))
    );
    assert!(r.register_display_profile(&profile).unwrap());
    let after = compare_preview(&r, &engine, &source, &s, RenderQuality::Final, 64, target);
    assert_eq!(
        after.backends.last(),
        Some(&("output.display", StageBackend::Gpu))
    );
}

#[test]
fn camera_profile_and_look_match_the_cpu_version() {
    // カメラプロファイル（DCP。ステージ 8 の色相彩度のマップ・段階 C のルックテーブル）と独自のルック（3D LUT）（DEV-17）を
    // 含むプレビュー（簡易処理の A1 は GPU 版のステージ 8・最終品質は CPU 版の A1）と書き出し。
    use std::sync::Arc;

    use genzo_color::{CubeLut, DcpProfile, HsvTable, Lut3d, Mat3};
    use genzo_pipeline::MemoryProfiles;
    let g = gpu_or_skip!();
    let m = typical_camera().map(|r| r.map(f64::from));
    // 表は、実際のプロファイルと同じく彩度 0 の点が色相によらない形にする（彩度 0 の近くでは色相が定まらないので、
    // 色相で明度の倍率が変わる表は、CPU 版と GPU 版のわずかな差で明るさが大きく変わりうる）。
    let mut hsm = Vec::new();
    for h in 0..8 {
        for s in 0..3 {
            hsm.push([(h as f32 - 4.0) * s as f32, 1.0 + 0.05 * s as f32, 1.0]);
        }
    }
    let hsm = HsvTable::new([8, 3, 1], false, hsm).unwrap();
    let mut look = Vec::new();
    for v in 0..3 {
        for h in 0..6 {
            for s in 0..3 {
                look.push([
                    3.0 * s as f32,
                    1.0 - 0.1 * s as f32 + 0.01 * (h * s) as f32,
                    1.0 - 0.05 * v as f32,
                ]);
            }
        }
    }
    let look = HsvTable::new([6, 3, 3], true, look).unwrap();
    let dcp = DcpProfile {
        calibration_illuminants: [Some(21), None],
        color_matrices: [Some(Mat3::from_rows(m)), None],
        hue_sat_maps: [Some(hsm), None],
        look_table: Some(look),
        ..Default::default()
    };
    let lut = Lut3d::from_fn(9, |v| [v[1], v[2], v[0]]).unwrap();
    let cube = CubeLut::new(None, 9, lut.data().to_vec(), [0.0; 3], [1.0; 3]).unwrap();
    let mut lib = MemoryProfiles::new();
    lib.insert_camera("dcp", dcp, &"aa".repeat(32));
    lib.insert_look("cube", cube, &"bb".repeat(32));
    let engine = engine(80, 48).with_profile_resolver(Arc::new(lib));
    let raw = raw_source(raw_from(&camera_scene(140, 96), CfaPattern::RGGB, true));
    let working = working_source(scene_b2(150, 100), Orientation::Normal);
    let mut s = rotated_strong();
    s.profile.camera = Some("dcp".into());
    s.profile.look = Some("cube".into());
    s.profile.look_amount = 65.0;
    for (source, quality) in [
        (&raw, RenderQuality::Final),
        (&raw, RenderQuality::Draft),
        (&working, RenderQuality::Final),
    ] {
        let p = compare_preview(
            g,
            &engine,
            source,
            &s,
            quality,
            80,
            OutputTarget::Export(OutputColorSpace::Srgb),
        );
        assert!(p.active_stages.contains(&"finish.look"));
        assert_eq!(
            p.active_stages.contains(&"finish.profile_look_table"),
            std::ptr::eq(source, &raw)
        );
        if std::ptr::eq(source, &raw) && quality == RenderQuality::Draft {
            assert!(
                p.a1_on_gpu,
                "簡易処理の A1 は GPU 版（ステージ 8 の色相彩度のマップを含む）"
            );
        }
    }
    let export = ExportSettings::default();
    let c = engine
        .export(&raw, &s, &export, &ExportOptions::default(), &NeverCancel)
        .unwrap();
    let e = g
        .export(
            &engine,
            &raw,
            &s,
            &export,
            &ExportOptions::default(),
            &NeverCancel,
        )
        .unwrap();
    let diff = export_diff_8bit(&c.pixels, &e.pixels);
    eprintln!("プロファイル・ルック: 書き出しの 8bit 換算の差の最大 {diff:.3}");
    assert!(diff <= FINAL_OUTPUT_MAX_DIFF_8BIT, "{diff}");
}

#[test]
fn masks_match_the_cpu_version() {
    // ローカル補正（マスク。DEV-18・DEV-19）を含むプレビュー（RAW・RAW 以外、最終品質・簡易処理）と書き出し（タイルの
    // 余白・局所の値の面をタイルごとに GPU で作る）。局所の値を使うステージは GPU 版で処理する。
    use genzo_model::{
        BrushStroke, LocalAdjustment, LocalDelta, MaskComponent, MaskMode, MaskShape,
    };
    let g = gpu_or_skip!();
    let engine = engine(80, 48);
    let sources = [
        working_source(scene_b2(150, 100), Orientation::Normal),
        raw_source(raw_from(&camera_scene(140, 96), CfaPattern::RGGB, true)),
    ];
    let mut s = rotated_strong();
    s.process_version = 2;
    let mut radial = LocalAdjustment {
        delta: LocalDelta {
            temperature: 30.0,
            exposure_ev: 0.6,
            highlights: -40.0,
            texture: 40.0,
            clarity: 30.0,
            dehaze: 25.0,
            saturation: -30.0,
            sharpness: 50.0,
            ..Default::default()
        },
        ..LocalAdjustment::new(MaskComponent::add(MaskShape::Radial {
            cx: 0.4,
            cy: 0.3,
            rx: 0.25,
            ry: 0.15,
            angle_deg: -15.0,
            feather: 60.0,
        }))
    };
    radial.components.push(MaskComponent {
        mode: MaskMode::Intersect,
        invert: false,
        shape: MaskShape::LuminanceRange {
            min: 10.0,
            max: 90.0,
            smoothness: 50.0,
        },
    });
    let brush = LocalAdjustment {
        delta: LocalDelta {
            exposure_ev: -0.5,
            contrast: 30.0,
            sharpness: -80.0,
            ..Default::default()
        },
        ..LocalAdjustment::new(MaskComponent::add(MaskShape::Brush {
            strokes: vec![BrushStroke {
                size: 0.08,
                feather: 50.0,
                flow: 80.0,
                points: vec![[0.6, 0.1], [0.8, 0.4]],
                ..Default::default()
            }],
        }))
    };
    s.masks = vec![radial, brush];
    for source in &sources {
        for quality in [RenderQuality::Final, RenderQuality::Draft] {
            let p = compare_preview(
                g,
                &engine,
                source,
                &s,
                quality,
                80,
                OutputTarget::Export(OutputColorSpace::Srgb),
            );
            let local_stages = [
                "finish.exposure",
                "finish.contrast",
                "finish.tone",
                "finish.texture",
                "finish.color",
                "finish.sharpen",
            ];
            assert!(
                p.backends
                    .iter()
                    .filter(|(id, _)| local_stages.contains(id))
                    .all(|(_, b)| *b == StageBackend::Gpu),
                "{:?}",
                p.backends
            );
        }
        let export = ExportSettings::default();
        let c = engine
            .export(source, &s, &export, &ExportOptions::default(), &NeverCancel)
            .unwrap();
        let e = g
            .export(
                &engine,
                source,
                &s,
                &export,
                &ExportOptions::default(),
                &NeverCancel,
            )
            .unwrap();
        let diff = export_diff_8bit(&c.pixels, &e.pixels);
        eprintln!("マスク: 書き出しの 8bit 換算の差の最大 {diff:.3}");
        assert!(diff <= FINAL_OUTPUT_MAX_DIFF_8BIT, "{diff}");
    }
}
