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
        for cfa in CfaPattern::ALL {
            // エンジンの A1 のキャッシュのキーは元ファイルの ID なので、RAW ごとにエンジンを作る。
            let engine = engine(64, 64);
            let raw = raw_from(&camera_scene(w, h), cfa, true);
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
        let ctx = genzo_gpu::GpuContext::from_env().unwrap().unwrap();
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

#[test]
fn export_matches_the_cpu_version() {
    let g = gpu_or_skip!();
    let engine = engine(80, 48);
    let sources = [
        working_source(scene_b2(150, 100), Orientation::Rotate180),
        raw_source(raw_from(&camera_scene(140, 96), CfaPattern::BGGR, true)),
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
    assert_eq!(cpu.backend, ExportBackend::Cpu { gpu_error: None });
    let gpu =
        export_with_fallback(Some(g), &engine, &source, &s, &export, &opts, &NeverCancel).unwrap();
    assert_eq!(gpu.backend, ExportBackend::Gpu);
    assert!(export_diff_8bit(&cpu.image.pixels, &gpu.image.pixels) <= FINAL_OUTPUT_MAX_DIFF_8BIT);
    // デバイスを失った GPU（6.3 節）: CPU 版で書き出し、理由を返す。
    let lost = GpuRenderer::from_env().unwrap().unwrap();
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
        ExportBackend::Cpu { gpu_error: Some(e) } => assert!(e.contains("失われ"), "{e}"),
        b => panic!("{b:?}"),
    }
    assert_eq!(out.image.pixels, cpu.image.pixels);
}

#[test]
fn a_lost_device_returns_errors_instead_of_panicking() {
    let _ = gpu_or_skip!();
    let r = GpuRenderer::from_env().unwrap().unwrap();
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
    let r = GpuRenderer::from_env().unwrap().unwrap();
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
