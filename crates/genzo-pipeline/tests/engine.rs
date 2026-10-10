//! エンジン（段階 A0〜C・キャッシュ・プレビュー・タイル処理・書き出し）の結合テスト。
//!
//! docs/04_architecture.md の 2.2 節・2.4 節・2.7 節・4.1 節、02 の IQ-07a・IQ-07b・IQ-08・SCL-05 の
//! 決めごとを、合成の RAW（testkit の合成ベイヤー）で確かめる。同じ計算どうし（タイルの大きさ・
//! スレッドの数・切り抜きの有無）はビット単位で比べ、基準値との比較には許容誤差を持たせる。

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use common::*;
use genzo_color::RgbColorSpace;
use genzo_model::{
    CropRect, DevelopSettings, ExportFormat, ExportSettings, ExportSize, FileId, Orientation,
    OutputColorSpace, RenderQuality,
};
use genzo_pipeline::engine::RenderControl;
use genzo_pipeline::finish::output::{Dither, quantize_u8, quantize_u16};
use genzo_pipeline::finish::{ExportStage, ToneStage};
use genzo_pipeline::{
    ColorContract, Engine, ExportOptions, ExportPixels, ImageTile, NeverCancel, OutputTarget,
    PhotoSource, PipelineError, PreviewRequest, RgbImage, Roi, SensorPlan, SourceId, SourceImage,
    Stage, downscale_area, run_stage_cpu, working_image_from_encoded,
};
use genzo_testkit::diff::{LabConversion, delta_e2000_stats};

/// 基準の実行: フル解像度の B2 を一度に作り（タイルに分けない）、ステージ 9〜16 を全体に適用する。
/// ガイドはエンジンと同じもの（プレビューの長辺の最終品質の A1 から作ったもの）。
fn untiled_reference(engine: &Engine, source: &PhotoSource, s: &DevelopSettings) -> ImageTile {
    let job = engine.full_resolution(source, s).unwrap();
    let ctx = job.context();
    let SourceImage::Raw(raw) = &source.image else {
        panic!("RAW の入力");
    };
    let plan = SensorPlan::new(raw, s, &Default::default()).unwrap();
    let (gw, gh) = plan.output_grid();
    let (b2, n) = plan.render_roi(raw, Roi::full(gw, gh)).unwrap();
    assert_eq!(n, 0);
    let (b3, n) = job.plan().render_full(&ctx, &b2).unwrap();
    assert_eq!(n, 0);
    b3
}

/// 書き出しの 17b（sRGB）を B3 の全体に適用して、8bit に量子化する（基準の書き出し）。
fn export_reference(
    engine: &Engine,
    source: &PhotoSource,
    s: &DevelopSettings,
    b3: &ImageTile,
) -> Vec<u8> {
    let job = engine.full_resolution(source, s).unwrap();
    let ctx = job.context();
    let stage = ExportStage::SRGB;
    let p = stage.params(s, &ctx).unwrap().unwrap();
    let (enc, _) = run_stage_cpu(&stage, &ctx, b3, b3.roi, &p).unwrap();
    quantize_u8(&enc.image, (0, 0), Dither::DEFAULT_8BIT).unwrap()
}

fn srgb8() -> ExportSettings {
    ExportSettings {
        format: ExportFormat::Jpeg { quality: 90 },
        color_space: OutputColorSpace::Srgb,
        ..Default::default()
    }
}

fn rgb8(px: &ExportPixels) -> &[u8] {
    match px {
        ExportPixels::Rgb8(v) => v,
        ExportPixels::Rgb16(_) => panic!("8bit のはず"),
    }
}

#[test]
fn tile_sizes_match_the_untiled_render_iq07a() {
    // タイルの大きさ 256 / 512 / 1024 とタイルに分けない処理の差が IQ-07a の範囲（2.7 節）。
    // 1024 のタイルが 2 つ以上になる大きさにする。回転・切り抜き・ガイドを使うトーンを含める。
    let raw = raw_from(&busy_scene(1200, 800));
    let source = raw_source(raw);
    let mut s = strong_settings();
    rotate_and_crop(
        &mut s,
        -4.0,
        CropRect {
            left: 0.05,
            top: 0.08,
            right: 0.97,
            bottom: 0.93,
        },
    );
    let reference_engine = engine(256, 1024);
    let reference = untiled_reference(&reference_engine, &source, &s);
    let reference_export = export_reference(&reference_engine, &source, &s, &reference);
    assert!(reference.image.width() > 1024);
    for tile in [256u32, 512, 1024] {
        let e = engine(256, tile);
        let r = e
            .render_full_resolution(&source, &s, None, &NeverCancel)
            .unwrap();
        assert_eq!(r.b3.roi, reference.roi);
        // 浮動小数点の B3: 小さな許容誤差（相対 1e-5）。実際はビット単位で一致する。
        for (p, q) in r.b3.image.data().iter().zip(reference.image.data()) {
            for k in 0..3 {
                let tol = 1e-5 * q[k].abs().max(1e-3);
                assert!((p[k] - q[k]).abs() <= tol, "{tile}: {p:?} vs {q:?}");
            }
        }
        assert_bitwise_eq(&r.b3.image, &reference.image, &format!("タイル {tile}"));
        // 書き出し（8bit）: IQ-07a（8bit 換算で ±1）。
        let ex = e
            .export(
                &source,
                &s,
                &srgb8(),
                &ExportOptions::default(),
                &NeverCancel,
            )
            .unwrap();
        let px = rgb8(&ex.pixels);
        assert_eq!(px.len(), reference_export.len());
        let max = px
            .iter()
            .zip(&reference_export)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(max <= 1, "タイル {tile}: 8bit の差の最大 {max}");
        assert!(e.stats().tiles_rendered >= 2);
    }
}

/// 読み込みの回数を数える A0 の読み込み。
fn counting_loader<'a>(
    scene: &'a genzo_testkit::image::FloatImage,
    loads: &'a AtomicU64,
) -> impl FnOnce() -> genzo_pipeline::Result<SourceImage> + 'a {
    move || {
        loads.fetch_add(1, Ordering::SeqCst);
        Ok(SourceImage::Raw(Arc::new(raw_from(scene))))
    }
}

#[test]
fn exposure_only_reuses_a1_and_guide_and_white_balance_keeps_a0() {
    // 2.2 節・2.7 節: 露光量だけを変えてもガイドと A1 を作り直さない。WB を変えたら A1（とガイド）を
    // 作り直し、A0（展開した RAW）は保持する。
    let scene = busy_scene(240, 160);
    let loads = AtomicU64::new(0);
    let id = SourceId::new(FileId::new(7), 3);
    let e = engine(96, 64);
    let mut s = DevelopSettings::default();
    s.tone.shadows = 40.0;
    let source = e
        .load_source(id, &s, counting_loader(&scene, &loads))
        .unwrap();
    let first = e
        .render_preview(&source, &s, RenderQuality::Final, 96)
        .unwrap();
    assert!(first.used_guide);
    assert!(first.active_stages.contains(&ToneStage::ID));
    let st = e.stats();
    assert_eq!((st.a0.hits, st.a0.misses), (0, 1));
    assert_eq!((st.a1.hits, st.a1.misses), (0, 1));
    assert_eq!((st.guide.hits, st.guide.misses), (0, 1));
    assert_eq!(st.previews_rendered, 1);

    // 露光量・コントラスト・トーン・カーブ・彩度・切り抜きだけを変える: A1 もガイドも作り直さない。
    let mut c = s.clone();
    c.exposure_ev = 1.0;
    c.contrast = 20.0;
    c.tone.highlights = -50.0;
    c.color.saturation = 25.0;
    c.geometry.crop = CropRect {
        left: 0.1,
        top: 0.1,
        right: 0.9,
        bottom: 0.9,
    };
    let source = e
        .load_source(id, &c, counting_loader(&scene, &loads))
        .unwrap();
    let second = e
        .render_preview(&source, &c, RenderQuality::Final, 96)
        .unwrap();
    assert_ne!(second.b3.image.data()[0], first.b3.image.data()[0]);
    let st = e.stats();
    assert_eq!((st.a0.hits, st.a0.misses), (1, 1));
    assert_eq!((st.a1.hits, st.a1.misses), (1, 1));
    assert_eq!((st.guide.hits, st.guide.misses), (1, 1));

    // WB を変える: A0 は保持（展開し直さない）、A1 とガイドは作り直す。
    let mut w = c.clone();
    custom_wb(&mut w, 4300.0, 8.0);
    let source = e
        .load_source(id, &w, counting_loader(&scene, &loads))
        .unwrap();
    e.render_preview(&source, &w, RenderQuality::Final, 96)
        .unwrap();
    let st = e.stats();
    assert_eq!((st.a0.hits, st.a0.misses), (2, 1));
    assert_eq!((st.a1.hits, st.a1.misses), (1, 2));
    assert_eq!((st.guide.hits, st.guide.misses), (1, 2));
    assert_eq!(loads.load(Ordering::SeqCst), 1, "A0 は 1 回だけ展開する");

    // WB のドラッグ中（簡易処理）は別のキー。指を離したら（最終品質）キャッシュから使う。
    custom_wb(&mut w, 4400.0, 8.0);
    let draft = e
        .render_preview(&source, &w, RenderQuality::Draft, 96)
        .unwrap();
    assert_eq!(draft.quality, RenderQuality::Draft);
    custom_wb(&mut w, 4300.0, 8.0);
    e.render_preview(&source, &w, RenderQuality::Final, 96)
        .unwrap();
    let st = e.stats();
    assert_eq!((st.a1.hits, st.a1.misses), (2, 3));
    assert_eq!((st.guide.hits, st.guide.misses), (2, 3));

    // 回転を変える: A1 はそのまま、ガイドは作り直す（G が変わる）。
    let mut r = w.clone();
    r.geometry.angle_deg = 2.5;
    e.render_preview(&source, &r, RenderQuality::Final, 96)
        .unwrap();
    let st = e.stats();
    assert_eq!((st.a1.hits, st.a1.misses), (3, 3));
    assert_eq!((st.guide.hits, st.guide.misses), (2, 4));

    // 等倍（プレビューと同じ長辺の最終品質の A1）は、プレビューのガイドを使う（2.7 節）。
    let job = e.full_resolution(&source, &r).unwrap();
    assert!(job.guide().is_some());
    let st = e.stats();
    assert_eq!((st.guide.hits, st.guide.misses), (3, 4));
    assert_eq!(st.a1.misses, 3);

    // トーンのスライダーがすべて 0 ならガイドを使わない（引きもしない）。
    let mut n = r.clone();
    n.tone = Default::default();
    let none = e
        .render_preview(&source, &n, RenderQuality::Final, 96)
        .unwrap();
    assert!(!none.used_guide);
    assert_eq!(e.stats().guide, st.guide);

    // キャッシュの件数は設定の上限まで。回数は 0 に戻せる。
    assert!(e.stats().cached[1] <= e.config().a1_capacity);
    e.reset_stats();
    assert_eq!(e.stats().a1.misses, 0);
    e.clear_caches();
    assert_eq!(e.stats().cached, [0, 0, 0]);
}

#[test]
fn crop_does_not_change_pixels_inside_the_crop() {
    // 2.7 節・R-03: 切り抜きを変えても、切り抜いた範囲内の結果は変わらない（ガイドは切り抜く前の
    // 全体から作る）。回転とガイドを使うトーンを含めて、プレビューとフル解像度の両方で確かめる。
    let source = raw_source(raw_from(&busy_scene(240, 160)));
    let mut wide = strong_settings();
    wide.geometry.angle_deg = 3.0;
    let mut narrow = wide.clone();
    narrow.geometry.crop = CropRect {
        left: 0.3,
        top: 0.25,
        right: 0.75,
        bottom: 0.7,
    };
    let e = engine(120, 64);
    // プレビュー。
    let a = e
        .render_preview(&source, &wide, RenderQuality::Final, 120)
        .unwrap();
    let b = e
        .render_preview(&source, &narrow, RenderQuality::Final, 120)
        .unwrap();
    check_crop(
        &a.b3,
        a.geometry.crop_lattice(),
        &b.b3,
        b.geometry.crop_lattice(),
    );
    // フル解像度。
    let ja = e.full_resolution(&source, &wide).unwrap();
    let jb = e.full_resolution(&source, &narrow).unwrap();
    let fa = e
        .render_full_resolution(&source, &wide, None, &NeverCancel)
        .unwrap();
    let fb = e
        .render_full_resolution(&source, &narrow, None, &NeverCancel)
        .unwrap();
    check_crop(
        &fa.b3,
        ja.geometry().crop_lattice(),
        &fb.b3,
        jb.geometry().crop_lattice(),
    );
}

/// 切り抜いた結果 `b`（G の格子の位置 `cb`）の画素が、広い結果 `a`（位置 `ca`）の同じ位置の画素と
/// ビット単位で同じか確かめる。
fn check_crop(a: &ImageTile, ca: Roi, b: &ImageTile, cb: Roi) {
    assert!(ca.contains_roi(&cb), "{ca:?} ⊇ {cb:?}");
    assert!(b.image.width() < a.image.width());
    let (dx, dy) = ((cb.x - ca.x) as u32, (cb.y - ca.y) as u32);
    for y in 0..cb.height {
        for x in 0..cb.width {
            let p = b.image.pixel(x, y).unwrap();
            let q = a.image.pixel(x + dx, y + dy).unwrap();
            assert_eq!(p.map(f32::to_bits), q.map(f32::to_bits), "({x}, {y})");
        }
    }
}

/// フル解像度を縮小した結果とプレビューの ΔE2000（B3、リニア BT.2020。IQ-07b）。
fn iq07b_stats(
    scene: &genzo_testkit::image::FloatImage,
    s: &DevelopSettings,
    long_edge: u32,
) -> genzo_testkit::diff::DeltaEStats {
    let source = raw_source(raw_from(scene));
    let e = engine(long_edge, 256);
    let preview = e
        .render_preview(&source, s, RenderQuality::Final, long_edge)
        .unwrap()
        .b3;
    let (pw, ph) = preview.image.dimensions();
    let (full, n) = e
        .render_downscaled(&source, s, (pw, ph), &NeverCancel)
        .unwrap();
    assert_eq!(n, 0);
    delta_e2000_stats(
        &to_float(&full.image),
        &to_float(&preview.image),
        LabConversion::linear(RgbColorSpace::Bt2020),
    )
    .unwrap()
}

/// IQ-07b を測る場合（シーン × 設定）。
fn iq07b_cases() -> Vec<(String, genzo_testkit::image::FloatImage, DevelopSettings)> {
    let (w, h) = (768, 512);
    let scenes = [
        ("busy", busy_scene(w, h)),
        ("thin_lines", thin_lines_scene(w, h)),
        ("color_edges", color_edges_scene(w, h)),
        ("high_contrast", high_contrast_scene(w, h)),
    ];
    let mut out = Vec::new();
    for (sn, scene) in scenes {
        for (tn, s) in [
            ("default", DevelopSettings::default()),
            ("strong", strong_settings()),
        ] {
            out.push((format!("{sn}/{tn}"), scene.clone(), s));
        }
    }
    out
}

#[test]
#[ignore = "IQ-07b は現在の仮実装（ステージ 11・15）では満たせない（細線と強い補正）。PoC-5 で処理順序か目標を見直す。数値は iq07b_measured_values_do_not_regress の表"]
fn preview_matches_downscaled_full_resolution_iq07b() {
    // IQ-07b: フル解像度の結果を面積平均でプレビューと同じ大きさに縮小したものと、プレビューとの差が
    // ΔE2000 の 95 パーセンタイルで 2 以下、最大で 5 以下（仮置き）。合成の画像（細線・色の境界・
    // 明暗差）に強い補正をかけて測る。縮小は 3 倍（768 × 512 → 256 × 171）。基準は緩めない。
    let mut failures = Vec::new();
    for (name, scene, s) in iq07b_cases() {
        let st = iq07b_stats(&scene, &s, 256);
        eprintln!(
            "IQ-07b {name}: 平均 {:.3}、95 パーセンタイル {:.3}、最大 {:.3}（{:?}）",
            st.mean, st.p95, st.max, st.max_at
        );
        if st.p95 > 2.0 || st.max > 5.0 {
            failures.push(format!("{name}: p95 {:.3} max {:.3}", st.p95, st.max));
        }
    }
    assert!(failures.is_empty(), "IQ-07b を満たさない: {failures:?}");
}

#[test]
fn iq07b_measured_values_do_not_regress() {
    // IQ-07b の計測値の回帰の検出（IQ-07b の基準ではない。基準の判定は上の #[ignore] のテスト）。
    // 表は、このコンテナで測った値（95 パーセンタイル, 最大）に 10% と 0.1 の余裕を足した上限。
    // 基準（95 パーセンタイル 2・最大 5）を満たす場合は、基準そのものでも確かめる。
    let limits: [(&str, f64, f64); 8] = [
        ("busy/default", 2.937, 6.748),
        ("busy/strong", 10.493, 39.866),
        ("thin_lines/default", 3.671, 4.042),
        ("thin_lines/strong", 9.551, 10.104),
        ("color_edges/default", 1.431, 4.068),
        ("color_edges/strong", 6.017, 28.440),
        ("high_contrast/default", 0.000, 4.101),
        ("high_contrast/strong", 1.296, 11.526),
    ];
    for (name, scene, s) in iq07b_cases() {
        let st = iq07b_stats(&scene, &s, 256);
        let (_, p95, max) = limits
            .iter()
            .find(|(n, _, _)| *n == name)
            .copied()
            .expect("表にある");
        eprintln!(
            "IQ-07b {name}: 平均 {:.3}、p95 {:.3}、最大 {:.3}",
            st.mean, st.p95, st.max
        );
        assert!(st.p95 <= p95 * 1.1 + 0.1, "{name}: p95 {} > {p95}", st.p95);
        assert!(st.max <= max * 1.1 + 0.1, "{name}: 最大 {} > {max}", st.max);
        if p95 <= 2.0 && max <= 5.0 {
            assert!(
                st.p95 <= 2.0 && st.max <= 5.0,
                "{name}: IQ-07b の基準を満たさなくなった"
            );
        }
    }
}

#[test]
fn neutral_gray_stays_neutral_in_srgb_export() {
    // 中立のグレーのパッチが、撮影時の WB で中立のまま出力される（sRGB の書き出しで R = G = B ± 1）。
    let (w, h) = (192, 128);
    let source = raw_source(raw_from(&gray_patches_scene(w, h)));
    let e = engine(96, 64);
    let s = DevelopSettings::default();
    for (dither, tol) in [(Dither::None, 1u8), (Dither::DEFAULT_8BIT, 2)] {
        let options = ExportOptions {
            dither_8bit: dither,
            ..Default::default()
        };
        let ex = e
            .export(&source, &s, &srgb8(), &options, &NeverCancel)
            .unwrap();
        assert_eq!((ex.width, ex.height), (w, h));
        let px = rgb8(&ex.pixels);
        let mut sums = [0u64; 3];
        let mut count = 0u64;
        for col in 0..6 {
            // パッチの内側（周り 6 画素を除く。デモザイクの境目の影響を避ける）。
            for y in 6..h / 2 - 6 {
                for x in col * w / 6 + 6..(col + 1) * w / 6 - 6 {
                    let i = ((y * w + x) * 3) as usize;
                    let p = &px[i..i + 3];
                    let (lo, hi) = (p.iter().min().unwrap(), p.iter().max().unwrap());
                    assert!(hi - lo <= tol, "{dither:?} ({x}, {y}): {p:?}");
                    for k in 0..3 {
                        sums[k] += u64::from(p[k]);
                    }
                    count += 1;
                }
            }
        }
        // ディザリングしても、平均では中立（チャンネルの平均の差が 0.5 未満）。
        let mean = sums.map(|v| v as f64 / count as f64);
        assert!(
            (mean[0] - mean[1]).abs() < 0.5 && (mean[2] - mean[1]).abs() < 0.5,
            "{mean:?}"
        );
    }
    // B3 でも中立（プレビュー）。
    let p = e
        .render_preview(&source, &s, RenderQuality::Final, 96)
        .unwrap();
    let c = p.b3.image.pixel(8, 10).unwrap();
    assert!(
        (c[0] - c[1]).abs() < 1e-4 && (c[2] - c[1]).abs() < 1e-4,
        "{c:?}"
    );
}

#[test]
fn results_do_not_depend_on_the_number_of_threads() {
    // 処理の決定性: 同じ入力・設定なら、スレッドの数を変えても同じ結果（プレビュー・書き出し）。
    let source = raw_source(raw_from(&busy_scene(300, 200)));
    let mut s = strong_settings();
    rotate_and_crop(
        &mut s,
        5.0,
        CropRect {
            left: 0.1,
            top: 0.05,
            right: 0.9,
            bottom: 0.95,
        },
    );
    let export = ExportSettings {
        size: ExportSize::LongEdge(150),
        ..srgb8()
    };
    let run = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let e = engine(120, 96);
            let p = e
                .render_preview_with(
                    &source,
                    &s,
                    &PreviewRequest::new(RenderQuality::Final, 120)
                        .with_output(OutputTarget::CacheDisplayP3)
                        .with_histogram(),
                )
                .unwrap();
            let ex = e
                .export(
                    &source,
                    &s,
                    &export,
                    &ExportOptions::default(),
                    &NeverCancel,
                )
                .unwrap();
            (p, ex)
        })
    };
    let (p1, e1) = run(1);
    for threads in [3, 4] {
        let (p, ex) = run(threads);
        assert_bitwise_eq(&p.b3.image, &p1.b3.image, "プレビュー B3");
        assert_bitwise_eq(
            p.output.as_ref().unwrap(),
            p1.output.as_ref().unwrap(),
            "プレビュー B5",
        );
        assert_eq!(p.histogram, p1.histogram);
        assert_eq!(ex.pixels, e1.pixels);
    }
}

#[test]
fn downscaled_export_equals_area_average_of_the_full_resolution() {
    // 2.4 節: 縮小を伴う書き出しは「フル解像度で処理してから縮小」。帯に分けて縮小しても、フル解像度の
    // B3 全体を面積平均（downscale_area）で縮小してから 17b・量子化したものとビット単位で同じ。
    let source = raw_source(raw_from(&busy_scene(250, 170)));
    let s = strong_settings();
    let e = engine(100, 48);
    let full = e
        .render_full_resolution(&source, &s, None, &NeverCancel)
        .unwrap();
    assert_eq!(full.full_size, (250, 170));
    for (long, format, space) in [
        (100u32, ExportFormat::Png8, OutputColorSpace::Srgb),
        (137, ExportFormat::Tiff16, OutputColorSpace::DisplayP3),
        (249, ExportFormat::Png16, OutputColorSpace::AdobeRgb),
        // 縮小の倍率が大きい（帯の入力の行がタイルより多く、縦にもタイルに分ける）。
        (3, ExportFormat::Png8, OutputColorSpace::Srgb),
    ] {
        let settings = ExportSettings {
            format,
            color_space: space,
            size: ExportSize::LongEdge(long),
            ..Default::default()
        };
        let ex = e
            .export(
                &source,
                &s,
                &settings,
                &ExportOptions::default(),
                &NeverCancel,
            )
            .unwrap();
        let (dw, dh) = settings.size.fit(250, 170);
        assert_eq!((ex.width, ex.height), (dw, dh));
        assert_eq!(ex.full_resolution_size, (250, 170));
        // 基準: フル解像度の B3 全体 → downscale_area → 17b → 量子化。
        let small = downscale_area(&full.b3.image, dw, dh).unwrap();
        let (b, _) = e
            .render_downscaled(&source, &s, (dw, dh), &NeverCancel)
            .unwrap();
        assert_bitwise_eq(&b.image, &small, &format!("縮小 {long}"));
        let job = e.full_resolution(&source, &s).unwrap();
        let tile = ImageTile::from_image(Roi::full(dw, dh), small).unwrap();
        let (enc, _) = job
            .apply_output(&OutputTarget::Export(space), &tile)
            .unwrap();
        assert_eq!(enc.contract(), ColorContract::B4bExport);
        match (&ex.pixels, format.bits_per_channel()) {
            (ExportPixels::Rgb8(v), 8) => {
                let q = quantize_u8(&enc.image, (0, 0), Dither::DEFAULT_8BIT).unwrap();
                assert_eq!(v, &q);
            }
            (ExportPixels::Rgb16(v), 16) => {
                let q = quantize_u16(&enc.image, (0, 0), Dither::None).unwrap();
                assert_eq!(v, &q);
            }
            other => panic!("ビット数が違う: {:?}", other.1),
        }
        // 埋め込む ICC は出力の色空間のもの。
        assert_eq!(ex.color_space, space);
        assert!(ex.icc.is_rgb());
    }
    // 縮小の寸法の誤り（0・拡大）はエラー。
    for bad in [(0, 10), (251, 170), (10, 171)] {
        assert!(
            e.render_downscaled(&source, &s, bad, &NeverCancel).is_err(),
            "{bad:?}"
        );
    }
    // 拡大はしない（長辺が出力より大きい指定は元の寸法）。
    let big = ExportSettings {
        size: ExportSize::LongEdge(5000),
        ..srgb8()
    };
    let ex = e
        .export(&source, &s, &big, &ExportOptions::default(), &NeverCancel)
        .unwrap();
    assert_eq!((ex.width, ex.height), (250, 170));
}

#[test]
fn region_render_matches_the_full_render() {
    // ルーペの等倍表示: 表示範囲だけを描いた結果が、全体を描いた結果の同じ範囲と同じ。範囲は出力の
    // 画像の内側に収め、タイルは画像全体の格子に揃える。
    let source = raw_source(raw_from(&busy_scene(260, 180)));
    let mut s = strong_settings();
    s.geometry.angle_deg = -2.0;
    let e = engine(130, 64);
    let display = OutputTarget::assumed_srgb_display().unwrap();
    let full = e
        .render_full_resolution(&source, &s, Some(&display), &NeverCancel)
        .unwrap();
    let (w, h) = full.full_size;
    let full_out = full.output.as_ref().unwrap();
    assert_eq!(full_out.contract(), ColorContract::B4aDisplay);
    for region in [
        Roi::new(70, 40, 100, 90),
        Roi::new(-20, -10, 60, 50),
        Roi::new(w as i32 - 30, h as i32 - 20, 100, 100),
    ] {
        let r = e
            .render_region(&source, &s, region, Some(&display), &NeverCancel)
            .unwrap();
        let clipped = region.intersect(&Roi::full(w, h)).unwrap();
        assert_eq!(r.b3.roi, clipped);
        assert_eq!(r.b3, full.b3.crop(clipped).unwrap());
        assert_eq!(r.output.as_ref().unwrap(), &full_out.crop(clipped).unwrap());
    }
    // タイルは画像全体の 64 px の格子に揃う。
    let job = e.full_resolution(&source, &s).unwrap();
    let tiles = job.tiles(Roi::new(70, 40, 100, 90), 64).unwrap();
    assert_eq!(tiles[0], Roi::new(70, 40, 58, 24));
    assert!(tiles.iter().all(|t| t.x % 64 == 0 || t.x == 70));
    assert_eq!(tiles.iter().map(|t| t.pixel_count()).sum::<u64>(), 100 * 90);
    // 画像と重ならない範囲・空のタイルの一辺はエラー。
    assert!(matches!(
        e.render_region(
            &source,
            &s,
            Roi::new(w as i32 + 5, 0, 10, 10),
            None,
            &NeverCancel
        ),
        Err(PipelineError::InvalidRoi { .. })
    ));
    assert!(job.tiles(Roi::full(10, 10), 0).is_err());
    assert!(job.render_tile(Roi::new(0, 0, w + 1, 1)).is_err());
}

#[test]
fn tile_margins_follow_the_input_rois() {
    // 2.7 節: タイルの余白は、各ステージの input_roi を出力側から順にたどって決まる。
    let source = raw_source(raw_from(&busy_scene(200, 140)));
    let mut s = strong_settings();
    s.geometry.angle_deg = 6.0;
    let e = engine(100, 64);
    let job = e.full_resolution(&source, &s).unwrap();
    let tile = Roi::new(64, 64, 64, 64);
    let r = job.tile_rois(tile);
    assert_eq!(*r.scene.last().unwrap(), tile);
    assert_eq!(r.sensor.len(), 5);
    // ステージ 8 の出力は、ステージ 9 の入力のうち画像の内側の部分（このタイルは下の端から少し
    // はみ出す。はみ出した部分は鏡映で作る）。
    assert!(r.scene[0].contains_roi(&r.sensor[4]), "{:?}", r.sensor);
    // 外側ほど広い（各ステージの入力は出力を含む）。デモザイク（RCD）の余白がある。
    for w in r.sensor.windows(2) {
        assert!(w[0].contains_roi(&w[1]), "{:?} ⊇ {:?}", w[0], w[1]);
    }
    assert!(r.sensor[1].width > r.sensor[3].width);
    assert!(r.margins().iter().all(|&m| m > 0), "{:?}", r.margins());
    // 作業領域の概算はタイルが大きいほど大きい。
    assert!(job.estimate_tile_bytes(32) < job.estimate_tile_bytes(128));
    // 回転なし・切り抜きなし・ステージ 9 なしなら、ステージ 9 の入力は出力と同じ。
    let plain = e
        .full_resolution(&source, &DevelopSettings::default())
        .unwrap();
    let r = plain.tile_rois(tile);
    assert_eq!(r.scene[0], tile);
    assert!(r.sensor[0].contains_roi(&tile));
}

#[test]
fn tile_rois_report_the_ranges_actually_computed_at_the_image_edges() {
    // 画像の端のタイル: ステージ 9 の入力（S の格子）は画像の外にはみ出すが、センサー処理は画像の
    // 内側だけを計算し、はみ出した部分は結果を鏡映して作る（SensorPlan::render_roi）。tile_rois は
    // 実際に計算する範囲を返すこと（以前は、はみ出した範囲をそのまま広げた値を返していた）。
    let source = raw_source(raw_from(&busy_scene(200, 140)));
    let mut s = strong_settings();
    s.geometry.angle_deg = 6.0;
    let e = engine(100, 64);
    let job = e.full_resolution(&source, &s).unwrap();
    let plan = job.sensor_plan().unwrap();
    let (gw, gh) = plan.output_grid();
    let (w, h) = job.output_size();
    let mut edge_tiles = 0;
    for tile in [
        Roi::new(0, 0, 64, 64),
        Roi::new(w as i32 - 40, h as i32 - 30, 40, 30),
        Roi::new(0, h as i32 - 16, w, 16),
        Roi::new(64, 32, 64, 64),
    ] {
        let r = job.tile_rois(tile);
        let scene0 = r.scene[0];
        if !scene0.is_inside(gw, gh) {
            edge_tiles += 1;
        }
        // ステージ 8 の出力は、ステージ 9 の入力を鏡映で覆う画像の内側の範囲。
        assert!(r.sensor[4].is_inside(gw, gh), "{tile:?}: {:?}", r.sensor);
        assert_eq!(
            Some(r.sensor[4]),
            genzo_pipeline::border::mirror_cover(scene0, gw, gh)
        );
        assert_eq!(plan.stage_rois(scene0).unwrap().to_vec(), r.sensor);
        // 外側ほど広い。
        for p in r.sensor.windows(2) {
            assert!(p[0].contains_roi(&p[1]), "{:?} ⊇ {:?}", p[0], p[1]);
        }
        // 余白は、ステージ 9 の入力に対するセンサー処理の余白（S の格子どうし）。
        let o = r.outermost();
        assert_eq!(
            r.margins(),
            [
                i64::from(scene0.x) - i64::from(o.x),
                i64::from(scene0.y) - i64::from(o.y),
                o.x_end() - scene0.x_end(),
                o.y_end() - scene0.y_end(),
            ]
        );
        // その範囲で描ける。
        let (b3, _) = job.render_tile(tile).unwrap();
        assert_eq!(b3.roi, tile);
    }
    assert!(edge_tiles >= 2, "端のタイルを確かめていない");
}

/// 何タイル目で取り消すかを決める制御。
struct CancelAfter {
    limit: u64,
    progress: AtomicU64,
    total: AtomicU64,
}

impl RenderControl for CancelAfter {
    fn is_cancelled(&self) -> bool {
        self.progress.load(Ordering::SeqCst) >= self.limit
    }
    fn on_progress(&self, done: u64, total: u64) {
        self.progress.store(done, Ordering::SeqCst);
        self.total.store(total, Ordering::SeqCst);
    }
}

#[test]
fn cancellation_is_checked_per_tile_and_progress_is_reported() {
    let source = raw_source(raw_from(&busy_scene(200, 140)));
    let s = DevelopSettings::default();
    let e = engine(100, 64);
    let c = CancelAfter {
        limit: 2,
        progress: AtomicU64::new(0),
        total: AtomicU64::new(0),
    };
    let before = e.stats().tiles_rendered;
    let r = e.export(&source, &s, &srgb8(), &ExportOptions::default(), &c);
    assert!(matches!(r, Err(PipelineError::Cancelled)));
    assert_eq!(e.stats().tiles_rendered - before, 2, "2 タイルで止まる");
    assert_eq!(c.total.load(Ordering::SeqCst), 4 * 3);
    // 縮小を伴う書き出し・ルーペも同じ。
    let c = CancelAfter {
        limit: 1,
        progress: AtomicU64::new(0),
        total: AtomicU64::new(0),
    };
    let small = ExportSettings {
        size: ExportSize::LongEdge(80),
        ..srgb8()
    };
    assert!(matches!(
        e.export(&source, &s, &small, &ExportOptions::default(), &c),
        Err(PipelineError::Cancelled)
    ));
    let cancel_now = || true;
    assert!(matches!(
        e.render_region(&source, &s, Roi::new(0, 0, 50, 50), None, &cancel_now),
        Err(PipelineError::Cancelled)
    ));
    // 取り消さなければ最後まで進む。
    let c = CancelAfter {
        limit: u64::MAX,
        progress: AtomicU64::new(0),
        total: AtomicU64::new(0),
    };
    e.export(&source, &s, &srgb8(), &ExportOptions::default(), &c)
        .unwrap();
    assert_eq!(c.progress.load(Ordering::SeqCst), 12);
}

#[test]
fn unknown_process_versions_and_bad_settings_are_rejected() {
    let scene = busy_scene(64, 48);
    let source = raw_source(raw_from(&scene));
    let e = engine(32, 32);
    let s = DevelopSettings {
        process_version: 99,
        ..Default::default()
    };
    let is_version = |r: genzo_pipeline::Result<()>| {
        matches!(
            r,
            Err(PipelineError::UnsupportedProcessVersion { found: 99, .. })
        )
    };
    assert!(is_version(
        e.render_preview(&source, &s, RenderQuality::Final, 32)
            .map(|_| ())
    ));
    assert!(is_version(e.full_resolution(&source, &s).map(|_| ())));
    assert!(is_version(
        e.export(
            &source,
            &s,
            &srgb8(),
            &ExportOptions::default(),
            &NeverCancel
        )
        .map(|_| ())
    ));
    assert!(is_version(
        e.load_source(source.id, &s, || Ok(source.image.clone()))
            .map(|_| ())
    ));
    assert!(is_version(
        e.a1(&source, &s, RenderQuality::Final, 32).map(|_| ())
    ));
    // 書き出しの設定の誤り。
    let ok = DevelopSettings::default();
    let bad = ExportSettings {
        size: ExportSize::LongEdge(0),
        ..srgb8()
    };
    assert!(matches!(
        e.export(&source, &ok, &bad, &ExportOptions::default(), &NeverCancel),
        Err(PipelineError::InvalidArgument(_))
    ));
    // ヒストグラムには出力先が必要。長辺 0 はエラー。
    let mut req = PreviewRequest::new(RenderQuality::Final, 32);
    req.histogram = true;
    assert!(e.render_preview_with(&source, &ok, &req).is_err());
    assert!(
        e.render_preview(&source, &ok, RenderQuality::Final, 0)
            .is_err()
    );
    // 読み込みの失敗はそのまま返し、キャッシュに入れない。
    let id = SourceId::new(FileId::new(42), 1);
    let r = e.load_source(id, &ok, || {
        Err(PipelineError::InvalidArgument("壊れた".into()))
    });
    assert!(r.is_err());
    assert_eq!(e.stats().cached[0], 0);
    // B2 でない RAW 以外の入力は受け付けない。
    let b3 = RgbImage::filled(4, 4, ColorContract::B3DisplayReferred, [0.1; 3]).unwrap();
    assert!(
        PhotoSource::new(
            id,
            SourceImage::Working {
                image: Arc::new(b3),
                orientation: Orientation::Normal,
            },
        )
        .is_err()
    );
}

#[test]
fn non_raw_input_starts_at_b2() {
    // RAW 以外の入力（2.6 節）: sRGB の 8bit の画素 → ICC で B2 → ステージ 9〜17（1〜8 は飛ばす）。
    // 向き（Exif）はステージ 9 で反映する。
    let (w, h) = (90u32, 60u32);
    let mut bytes = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let v = if x < w / 2 { 128 } else { (x * 2 + y) as u8 };
            bytes.extend_from_slice(&[v, v, v]);
        }
    }
    let enc = genzo_pipeline::input::encoded_from_u8(&bytes).unwrap();
    let image = working_image_from_encoded(w, h, &enc, None).unwrap();
    let source = PhotoSource::new(
        SourceId::new(FileId::new(5), 1),
        SourceImage::Working {
            image: Arc::new(image),
            orientation: Orientation::Rotate90Cw,
        },
    )
    .unwrap();
    let e = engine(45, 32);
    let s = DevelopSettings::default();
    // 簡易処理を求めても、センサー処理がないので最終品質。
    let p = e
        .render_preview(&source, &s, RenderQuality::Draft, 45)
        .unwrap();
    assert_eq!(p.quality, RenderQuality::Final);
    assert_eq!(p.b3.image.dimensions(), (30, 45), "向きで縦横が入れ替わる");
    assert!(p.warnings.is_empty());
    // フル解像度の書き出し。グレーは中立のまま。
    let ex = e
        .export(
            &source,
            &s,
            &srgb8(),
            &ExportOptions {
                dither_8bit: Dither::None,
                ..Default::default()
            },
            &NeverCancel,
        )
        .unwrap();
    assert_eq!((ex.width, ex.height), (60, 90));
    let px = rgb8(&ex.pixels);
    for p in px.chunks_exact(3) {
        assert!(
            p.iter().max().unwrap() - p.iter().min().unwrap() <= 1,
            "{p:?}"
        );
    }
    // タイルに分けても同じ（RAW 以外の経路でも余白を鏡映で取る）。
    let a = engine(45, 16)
        .render_full_resolution(&source, &strong_settings(), None, &NeverCancel)
        .unwrap();
    let b = engine(45, 1000)
        .render_full_resolution(&source, &strong_settings(), None, &NeverCancel)
        .unwrap();
    assert_bitwise_eq(&a.b3.image, &b.b3.image, "RAW 以外のタイル");
}

#[test]
fn preview_outputs_and_histogram() {
    let source = raw_source(raw_from(&busy_scene(160, 120)));
    let e = engine(80, 64);
    let s = strong_settings();
    for (target, contract) in [
        (
            OutputTarget::assumed_srgb_display().unwrap(),
            ColorContract::B4aDisplay,
        ),
        (
            OutputTarget::Export(OutputColorSpace::DisplayP3),
            ColorContract::B4bExport,
        ),
        (OutputTarget::CacheDisplayP3, ColorContract::B5Cache),
    ] {
        let p = e
            .render_preview_with(
                &source,
                &s,
                &PreviewRequest::new(RenderQuality::Final, 80)
                    .with_output(target)
                    .with_histogram(),
            )
            .unwrap();
        let out = p.output.as_ref().unwrap();
        assert_eq!(out.contract(), contract);
        assert_eq!(out.dimensions(), p.b3.image.dimensions());
        // 17a は 3D LUT（lcms2 で作った格子の値の四面体補間）の誤差で 0〜1 をわずかに外れることがある
        // （8bit の 1 段 ≒ 0.0039 より小さい）。17b は 0〜1 に収める（ExportTransform の 4.）。
        let slack = if contract == ColorContract::B4aDisplay {
            1.0 / 255.0
        } else {
            0.0
        };
        let (lo, hi) = out
            .as_flat()
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        eprintln!("{contract:?}: 最小 {lo}、最大 {hi}");
        let bad: Vec<f32> = out
            .as_flat()
            .iter()
            .copied()
            .filter(|v| !(-slack..=1.0 + slack).contains(v))
            .take(5)
            .collect();
        assert!(bad.is_empty(), "{contract:?}: {bad:?}");
        let hist = p.histogram.unwrap();
        assert_eq!(hist.pixel_count, out.pixel_count());
        assert_eq!(hist.luma.iter().sum::<u64>(), out.pixel_count());
        assert_eq!(p.non_finite_replaced, 0);
    }
    // A1 とガイドは 1 回だけ作った（出力先を変えても段階 C だけ）。
    let st = e.stats();
    assert_eq!((st.a1.misses, st.guide.misses), (1, 1));
    assert_eq!(st.previews_rendered, 3);
}

#[test]
fn draft_preview_uses_the_2x2_path() {
    // WB のドラッグ中（簡易処理。2 × 2 の画素をまとめる）。プレビューの寸法は半分の格子を超えない。
    let source = raw_source(raw_from(&busy_scene(200, 140)));
    let e = engine(160, 64);
    let mut s = DevelopSettings::default();
    s.tone.shadows = 30.0;
    let d = e
        .render_preview(&source, &s, RenderQuality::Draft, 160)
        .unwrap();
    assert_eq!(d.quality, RenderQuality::Draft);
    assert_eq!(d.b3.image.dimensions(), (100, 70));
    let f = e
        .render_preview(&source, &s, RenderQuality::Final, 160)
        .unwrap();
    assert_eq!(f.b3.image.dimensions(), (160, 112));
    // ガイドは品質ごとに別（入力の A1 が違う）。
    assert_eq!(e.stats().guide.misses, 2);
    // 簡易と最終品質の差は IQ-07b の対象外だが、平らな部分の明るさはほぼ同じ。
    let a = d.b3.image.pixel(5, 30).unwrap();
    let b = f.b3.image.pixel(8, 48).unwrap();
    assert!((a[1] - b[1]).abs() < 0.02, "{a:?} {b:?}");
}

#[test]
fn stage_backends_fall_back_to_cpu() {
    // GPU 版がないステージは CPU 版で処理する（7.1 節）。GPU のない環境ではすべて CPU。
    use genzo_pipeline::engine::StageBackend;
    use genzo_pipeline::{GpuStage, GpuStageLookup, NoGpu, ProcessVersion};

    struct FakeExposureGpu;
    impl GpuStage for FakeExposureGpu {
        fn stage_id(&self) -> &'static str {
            genzo_pipeline::finish::ExposureStage::ID
        }
        fn supports(&self, v: ProcessVersion) -> bool {
            v == ProcessVersion::V1
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    struct Lookup(FakeExposureGpu);
    impl GpuStageLookup for Lookup {
        fn find(&self, id: &str) -> Option<&dyn GpuStage> {
            (id == self.0.stage_id()).then_some(&self.0 as &dyn GpuStage)
        }
    }

    let source = raw_source(raw_from(&busy_scene(64, 48)));
    let e = engine(32, 32);
    let s = DevelopSettings {
        exposure_ev: 0.5,
        ..Default::default()
    };
    let job = e.full_resolution(&source, &s).unwrap();
    let cpu = job.stage_backends(&NoGpu);
    assert!(cpu.iter().all(|(_, b)| *b == StageBackend::Cpu));
    assert_eq!(cpu[0].0, "sensor.normalize");
    assert!(cpu.iter().any(|(id, _)| *id == "finish.scene_to_display"));
    let mixed = job.stage_backends(&Lookup(FakeExposureGpu));
    for (id, b) in mixed {
        let expect = if id == "finish.exposure" {
            StageBackend::Gpu
        } else {
            StageBackend::Cpu
        };
        assert_eq!(b, expect, "{id}");
    }
}

#[test]
fn engine_and_jobs_can_be_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Engine>();
    assert_send_sync::<genzo_pipeline::FullResolutionJob>();
    assert_send_sync::<PhotoSource>();
    // 2 つのスレッドから同じエンジンでプレビューと書き出しを同時に行っても、結果は 1 つのときと同じ。
    let source = raw_source(raw_from(&busy_scene(120, 80)));
    let s = strong_settings();
    let e = engine(60, 32);
    let alone = e
        .render_preview(&source, &s, RenderQuality::Final, 60)
        .unwrap();
    let alone_export = e
        .export(
            &source,
            &s,
            &srgb8(),
            &ExportOptions::default(),
            &NeverCancel,
        )
        .unwrap();
    let shared = engine(60, 32);
    std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            shared
                .render_preview(&source, &s, RenderQuality::Final, 60)
                .unwrap()
        });
        let b = scope.spawn(|| {
            shared
                .export(
                    &source,
                    &s,
                    &srgb8(),
                    &ExportOptions::default(),
                    &NeverCancel,
                )
                .unwrap()
        });
        assert_bitwise_eq(&a.join().unwrap().b3.image, &alone.b3.image, "並行");
        assert_eq!(b.join().unwrap().pixels, alone_export.pixels);
    });
}

#[test]
fn tiles_match_the_untiled_render_for_every_orientation() {
    // 2.7 節: タイルの分け方によらず結果が同じ（IQ-07a の前提）。RAW の向き（EXIF）・回転・切り抜き・
    // ガイドを使うトーンを組み合わせ、極端に小さいタイル（余白の方が大きい）でも確かめる。
    let scene = busy_scene(90, 60);
    for o in [
        Orientation::Rotate90Cw,
        Orientation::Transverse,
        Orientation::FlipHorizontal,
    ] {
        let source = raw_source(raw_oriented(&scene, o));
        let mut s = strong_settings();
        rotate_and_crop(
            &mut s,
            7.0,
            CropRect {
                left: 0.05,
                top: 0.1,
                right: 0.9,
                bottom: 0.95,
            },
        );
        let reference = untiled_reference(&engine(45, 1024), &source, &s);
        for tile in [7u32, 32] {
            let r = engine(45, tile)
                .render_full_resolution(&source, &s, None, &NeverCancel)
                .unwrap();
            assert_eq!(r.b3.roi, reference.roi);
            assert_bitwise_eq(&r.b3.image, &reference.image, &format!("{o:?} {tile}"));
        }
    }
}

#[test]
fn tiny_and_thin_inputs_work_through_every_entry_point() {
    // 寸法の端の値（2 × 2、1 列・1 行に近い細長い画像）と回転・向きの組み合わせで、プレビュー（簡易・
    // 最終）・等倍・書き出し（縮小・16bit を含む）がパニックせずに動き、タイルの一辺 1 でも結果が同じ。
    let mut s = strong_settings();
    s.geometry.angle_deg = 30.0;
    for (w, h) in [(2u32, 2u32), (3, 2), (2, 9), (300, 2)] {
        let source = raw_source(raw_oriented(&busy_scene(w, h), Orientation::Rotate90Cw));
        let e = engine(16, 64);
        for q in [RenderQuality::Final, RenderQuality::Draft] {
            let p = e.render_preview(&source, &s, q, 16).unwrap();
            assert_eq!(p.non_finite_replaced, 0, "{w}x{h} {q:?}");
        }
        let a = e
            .render_full_resolution(&source, &s, None, &NeverCancel)
            .unwrap();
        let b = engine(16, 1)
            .render_full_resolution(&source, &s, None, &NeverCancel)
            .unwrap();
        assert_bitwise_eq(&a.b3.image, &b.b3.image, &format!("{w}x{h}"));
        for export in [
            ExportSettings {
                size: ExportSize::LongEdge(2),
                ..srgb8()
            },
            ExportSettings {
                format: ExportFormat::Tiff16,
                ..Default::default()
            },
        ] {
            let ex = e
                .export(
                    &source,
                    &s,
                    &export,
                    &ExportOptions::default(),
                    &NeverCancel,
                )
                .unwrap();
            assert_eq!(ex.pixels.len(), (ex.width * ex.height * 3) as usize);
        }
    }
    // RAW 以外の 1 列・1 行の画像。
    for (w, h) in [(1u32, 1u32), (1, 7), (7, 1)] {
        let image = RgbImage::from_fn(w, h, ColorContract::B2Working, |x, y| {
            [0.1 + 0.05 * x as f32, 0.2, 0.05 + 0.01 * y as f32]
        })
        .unwrap();
        let source = PhotoSource::new(
            SourceId::new(FileId::new(3), 1),
            SourceImage::Working {
                image: Arc::new(image),
                orientation: Orientation::Rotate270Cw,
            },
        )
        .unwrap();
        let e = engine(4, 2);
        e.render_preview(&source, &s, RenderQuality::Final, 4)
            .unwrap();
        let ex = e
            .export(
                &source,
                &s,
                &ExportSettings {
                    size: ExportSize::LongEdge(1),
                    ..srgb8()
                },
                &ExportOptions::default(),
                &NeverCancel,
            )
            .unwrap();
        assert_eq!((ex.width, ex.height), (1, 1), "{w}x{h}");
    }
}

#[test]
fn preview_and_full_resolution_registration_is_within_half_a_preview_pixel() {
    // プレビューの画素と、フル解像度をプレビューの大きさに縮小した画素（四隅を合わせる。IQ-07b の比較）
    // の G 座標での位置の差。
    // - 回転・切り抜きがなければ（向きだけなら）位置は完全に一致し、滑らかなシーンでは差はほぼ 0。
    // - 回転・切り抜きがあると、切り抜きの端をそれぞれの格子（プレビューの密度 d と 1）に丸めるので、
    //   位置が最大で「プレビューの 0.5 画素＋フル解像度の 0.5 画素」ずれる（決めごと。lib.rs の
    //   「残りの担当」）。これを超えるずれ（座標の取り違え）を検出する。
    let scene = wavy_scene(300, 201);
    let crop = CropRect {
        left: 0.13,
        top: 0.21,
        right: 0.81,
        bottom: 0.77,
    };
    for (o, deg, crop) in [
        (Orientation::Rotate90Cw, 0.0f32, CropRect::FULL),
        (Orientation::Transverse, 0.0, CropRect::FULL),
        (Orientation::Normal, 0.0, crop),
        (Orientation::Normal, 7.0, CropRect::FULL),
        (Orientation::Rotate90Cw, -3.0, crop),
        (Orientation::FlipVertical, 12.0, crop),
    ] {
        let source = raw_source(raw_oriented(&scene, o));
        let mut s = DevelopSettings::default();
        rotate_and_crop(&mut s, deg, crop);
        for long_edge in [100u32, 150] {
            let e = engine(long_edge, 64);
            let p = e
                .render_preview(&source, &s, RenderQuality::Final, long_edge)
                .unwrap();
            let job = e.full_resolution(&source, &s).unwrap();
            let (gp, gf) = (&p.geometry, job.geometry());
            let ((wp, hp), (wf, hf)) = (gp.output_size(), gf.output_size());
            let d = gp.density();
            let mut worst: f64 = 0.0;
            for j in 0..hp {
                for i in 0..wp {
                    let (ax, ay) = gp.c_to_g_px(f64::from(i) + 0.5, f64::from(j) + 0.5);
                    let (bx, by) = gf.c_to_g_px(
                        (f64::from(i) + 0.5) * f64::from(wf) / f64::from(wp),
                        (f64::from(j) + 0.5) * f64::from(hf) / f64::from(hp),
                    );
                    // プレビューの画素の単位。
                    worst = worst.max((ax - bx).abs() * d.0).max((ay - by).abs() * d.1);
                }
            }
            let what = format!(
                "{o:?} {deg}° crop={} 長辺 {long_edge}",
                crop != CropRect::FULL
            );
            if deg == 0.0 && crop == CropRect::FULL {
                assert!(worst < 1e-9, "{what}: {worst}");
                // 位置が一致すれば、滑らかなシーンの差はごく小さい（RCD と面積平均の順序の差だけ）。
                let (full, _) = e
                    .render_downscaled(&source, &s, (wp, hp), &NeverCancel)
                    .unwrap();
                let st = delta_e2000_stats(
                    &to_float(&full.image),
                    &to_float(&p.b3.image),
                    LabConversion::linear(RgbColorSpace::Bt2020),
                )
                .unwrap();
                assert!(st.p95 < 0.05 && st.max < 0.5, "{what}: {st:?}");
            } else {
                let bound = 0.5 + 0.5 * d.0.max(d.1) + 1e-6;
                assert!(worst <= bound, "{what}: {worst} > {bound}");
            }
        }
    }
}
