//! 性能の目安の計測（CI では実行しない。`#[ignore]`）。
//!
//! ```text
//! cargo test -p genzo-pipeline --release --test engine_bench -- --ignored --nocapture --test-threads 1
//! ```
//!
//! 合成のデータで、このコンテナ（CPU 版）の目安を測る。合否の判定は実機（M1・RTX 3080。05 の PoC-3）で
//! 行う。結果は標準出力に表示する。環境変数 `GENZO_BENCH_DIR` を設定したときだけ、genzo-testkit の
//! 記録の形式でそのディレクトリに書く（リポジトリの中にファイルを作らないため）。

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use genzo_model::{DevelopSettings, ExportSettings, FileId, Orientation, RenderQuality};
use genzo_pipeline::{
    ColorContract, ExportOptions, NeverCancel, PhotoSource, RgbImage, SourceId, SourceImage,
};
use genzo_testkit::bench::{Bench, BenchRecorder, BenchResult};

/// α7 IV の寸法（2.2 節）。
const A7IV: (u32, u32) = (7008, 4672);

fn report(result: &BenchResult) {
    println!("{result}");
    if std::env::var_os("GENZO_BENCH_DIR").is_some_and(|v| !v.is_empty()) {
        let r = BenchRecorder::from_env().record(result).expect("記録");
        println!("{r}");
    }
}

#[test]
#[ignore = "計測（手動で --release で実行する）"]
fn bench_stage_c_preview_2560() {
    // 段階 C（ステージ 9〜16 と 17b のキャッシュ用の変換）: 露光量のドラッグ（A1・ガイドはキャッシュ）。
    // 入力は B2 の 2560 × 1707（RAW 以外の経路なので A1 = 入力）。目標 33ms（PERF-01）は GPU の値。
    let (w, h) = (2560u32, 1707u32);
    let scene = busy_scene(w, h);
    let image = RgbImage::from_vec(
        w,
        h,
        ColorContract::B2Working,
        scene
            .data()
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .collect(),
    )
    .unwrap();
    let source = PhotoSource::new(
        SourceId::new(FileId::new(1), 1),
        SourceImage::Working {
            image: Arc::new(image),
            orientation: Orientation::Normal,
        },
    )
    .unwrap();
    let e = engine(2560, 1024);
    let mut s = strong_settings();
    e.render_preview(&source, &s, RenderQuality::Final, 2560)
        .unwrap();
    let mut ev = 0.0f32;
    let result = Bench::latency("engine_stage_c_preview_2560")
        .target(Duration::from_millis(33))
        .note("resolution", "2560x1707")
        .note("backend", "cpu")
        .develop_settings(&s)
        .run_warm(|| {
            ev = if ev > 1.0 { 0.0 } else { ev + 0.1 };
            s.exposure_ev = ev;
            e.render_preview(&source, &s, RenderQuality::Final, 2560)
                .unwrap()
        })
        .unwrap();
    report(&result);
    let st = e.stats();
    assert_eq!(st.a1.misses, 1, "A1 は作り直さない");
    assert_eq!(st.guide.misses, 1, "ガイドは作り直さない");
}

#[test]
#[ignore = "計測（手動で --release で実行する）"]
fn bench_a1_and_full_resolution_export_a7iv() {
    // α7 IV の寸法の合成の RAW で、段階 A1（最終品質・簡易処理）・ガイド・書き出し（タイル 1024）を測る。
    let (w, h) = A7IV;
    let source = raw_source(raw_from(&busy_scene(w, h)));
    let e = engine(2560, 1024);
    let mut s = DevelopSettings::default();
    s.tone.shadows = 30.0;
    for (name, quality) in [
        ("engine_a1_final_a7iv", RenderQuality::Final),
        ("engine_a1_draft_a7iv", RenderQuality::Draft),
    ] {
        let result = Bench::batch(name)
            .note("resolution", "7008x4672")
            .note("backend", "cpu")
            .run_cold(
                || e.clear_caches(),
                || e.a1(&source, &s, quality, 2560).unwrap(),
            )
            .unwrap();
        report(&result);
    }
    let result = Bench::batch("engine_guide_a7iv")
        .note("backend", "cpu")
        .run_cold(
            || {
                e.clear_caches();
                e.a1(&source, &s, RenderQuality::Final, 2560).unwrap();
            },
            || e.guide(&source, &s, RenderQuality::Final, 2560).unwrap(),
        )
        .unwrap();
    report(&result);
    let export = ExportSettings::default();
    let job = e.full_resolution(&source, &s).unwrap();
    println!(
        "タイル 1 つの作業領域の概算: {:.1} MB",
        job.estimate_tile_bytes(1024) as f64 / 1e6
    );
    let result = Bench::batch("engine_export_jpeg8_a7iv")
        .note("resolution", "7008x4672")
        .note("backend", "cpu")
        .note("tile", "1024")
        .run_warm(|| {
            job.export(&export, &ExportOptions::default(), &NeverCancel)
                .unwrap()
        })
        .unwrap();
    report(&result);
}

#[test]
#[ignore = "計測（手動で --release で実行する）"]
fn bench_a1_with_highlight_recovery_and_noise_reduction_a7iv() {
    // ハイライト復元（ステージ 4）・ノイズ軽減（ステージ 6。処理バージョン 2）を有効にした段階 A1（最終品質・簡易処理）と
    // 書き出し。シーンは busy_scene を 2.5 倍にして、明るい部分を白飛びさせる（白飛びの画素の割合は表示する）。
    use genzo_model::{HighlightMethod, HighlightRecovery, NoiseReduction};
    let (w, h) = A7IV;
    let scene = busy_scene(w, h);
    let bright = genzo_testkit::image::FloatImage::from_rgb_fn(w, h, |x, y| {
        let p = scene.pixel(x, y).unwrap();
        [p[0] * 2.5, p[1] * 2.5, p[2] * 2.5]
    })
    .unwrap();
    let raw = raw_from(&bright);
    let clipped = raw
        .data
        .iter()
        .filter(|&&v| f32::from(v) >= raw.saturation_level)
        .count();
    println!(
        "白飛びの画素の割合: {:.1} %",
        100.0 * clipped as f64 / raw.data.len() as f64
    );
    let source = raw_source(raw);
    let e = engine(2560, 1024);
    let base = DevelopSettings::default();
    let mut nr = DevelopSettings::default();
    nr.detail.noise_reduction = NoiseReduction {
        luminance: 30.0,
        color: 25.0,
        ..Default::default()
    };
    let mut color_only = DevelopSettings::default();
    color_only.detail.noise_reduction.color = 25.0;
    let hr = DevelopSettings {
        highlight_recovery: HighlightRecovery {
            method: HighlightMethod::Reconstruct,
            amount: 100.0,
        },
        ..Default::default()
    };
    let mut both = nr.clone();
    both.highlight_recovery = hr.highlight_recovery;
    for (name, s, quality) in [
        (
            "engine_a1_final_v2_default_a7iv",
            &base,
            RenderQuality::Final,
        ),
        ("engine_a1_final_hr_a7iv", &hr, RenderQuality::Final),
        (
            "engine_a1_final_nr_color_a7iv",
            &color_only,
            RenderQuality::Final,
        ),
        ("engine_a1_final_nr_a7iv", &nr, RenderQuality::Final),
        ("engine_a1_final_hr_nr_a7iv", &both, RenderQuality::Final),
        ("engine_a1_draft_hr_nr_a7iv", &both, RenderQuality::Draft),
    ] {
        let result = Bench::batch(name)
            .note("resolution", "7008x4672")
            .note("backend", "cpu")
            .develop_settings(s)
            .run_cold(
                || e.clear_caches(),
                || e.a1(&source, s, quality, 2560).unwrap(),
            )
            .unwrap();
        report(&result);
    }
    let export = ExportSettings::default();
    let job = e.full_resolution(&source, &both).unwrap();
    let result = Bench::batch("engine_export_jpeg8_hr_nr_a7iv")
        .note("resolution", "7008x4672")
        .note("backend", "cpu")
        .note("tile", "1024")
        .develop_settings(&both)
        .run_warm(|| {
            job.export(&export, &ExportOptions::default(), &NeverCancel)
                .unwrap()
        })
        .unwrap();
    report(&result);
}

#[test]
#[ignore = "計測（手動で --release で実行する）"]
fn bench_stage_c_preview_2560_with_presence_sharpening_and_effects() {
    // 処理バージョン 2 のステージ 12（テクスチャ・明瞭度・かすみの除去）・14（シャープ）・16 の効果（周辺光量・粒子）を
    // 足した段階 C（露光量のドラッグ。A1・ガイドはキャッシュ）。入力は B2 の 2560 × 1707（RAW 以外の経路）。
    // 項目ごとに足した時間を比べるため、strong_settings だけのものも同じ条件で測る。
    let (w, h) = (2560u32, 1707u32);
    let scene = busy_scene(w, h);
    let image = RgbImage::from_vec(
        w,
        h,
        ColorContract::B2Working,
        scene
            .data()
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .collect(),
    )
    .unwrap();
    let source = PhotoSource::new(
        SourceId::new(FileId::new(1), 1),
        SourceImage::Working {
            image: Arc::new(image),
            orientation: Orientation::Normal,
        },
    )
    .unwrap();
    let with = |f: &dyn Fn(&mut DevelopSettings)| {
        let mut s = strong_settings();
        f(&mut s);
        s
    };
    let all = with(&|s| {
        s.presence.texture = 40.0;
        s.presence.clarity = 30.0;
        s.presence.dehaze = 25.0;
        s.detail.sharpening.amount = 60.0;
        s.detail.sharpening.masking = 20.0;
        s.effects.vignette.amount = -30.0;
        s.effects.grain.amount = 25.0;
    });
    for (name, s) in [
        ("engine_stage_c_preview_2560_base", strong_settings()),
        (
            "engine_stage_c_preview_2560_texture",
            with(&|s| s.presence.texture = 40.0),
        ),
        (
            "engine_stage_c_preview_2560_clarity",
            with(&|s| s.presence.clarity = 30.0),
        ),
        (
            "engine_stage_c_preview_2560_dehaze",
            with(&|s| s.presence.dehaze = 25.0),
        ),
        (
            "engine_stage_c_preview_2560_sharpen",
            with(&|s| s.detail.sharpening.amount = 60.0),
        ),
        (
            "engine_stage_c_preview_2560_vignette",
            with(&|s| s.effects.vignette.amount = -30.0),
        ),
        (
            "engine_stage_c_preview_2560_grain",
            with(&|s| s.effects.grain.amount = 25.0),
        ),
        ("engine_stage_c_preview_2560_detail_all", all.clone()),
    ] {
        let e = engine(2560, 1024);
        let mut s = s;
        e.render_preview(&source, &s, RenderQuality::Final, 2560)
            .unwrap();
        let mut ev = 0.0f32;
        let result = Bench::latency(name)
            .target(Duration::from_millis(33))
            .note("resolution", "2560x1707")
            .note("backend", "cpu")
            .develop_settings(&s)
            .run_warm(|| {
                ev = if ev > 1.0 { 0.0 } else { ev + 0.1 };
                s.exposure_ev = ev;
                e.render_preview(&source, &s, RenderQuality::Final, 2560)
                    .unwrap()
            })
            .unwrap();
        report(&result);
        assert!(e.stats().guide.misses <= 1, "ガイドは作り直さない");
    }
    // ガイド（かすみの推定あり・なし）の作り直しの時間（回転を変えたときなど）。
    let e = engine(2560, 1024);
    for (name, s) in [
        ("engine_guide_2560", strong_settings()),
        ("engine_guide_with_haze_2560", all),
    ] {
        let result = Bench::batch(name)
            .note("backend", "cpu")
            .run_cold(
                || {
                    e.clear_caches();
                    e.a1(&source, &s, RenderQuality::Final, 2560).unwrap();
                },
                || e.guide(&source, &s, RenderQuality::Final, 2560).unwrap(),
            )
            .unwrap();
        report(&result);
    }
}

/// 画像の幅（G 座標の 0〜1）を `passes` 回なぞる（2 回目・4 回目… は右から左。なぞるごとに `step` だけ下へ）ストローク。
/// 点は大きさの 1/8 の間隔（画面の間引きと同じ。大きさ 0.05 で 1 回 161 点）。
fn zigzag_stroke(size: f32, passes: usize, y: f32, step: f32) -> genzo_model::BrushStroke {
    let spacing = size / 8.0;
    let n = (1.0 / spacing).round() as usize;
    let mut points = Vec::new();
    for k in 0..passes {
        for i in 0..=n {
            let x = i as f32 * spacing;
            points.push([if k % 2 == 0 { x } else { 1.0 - x }, y + step * k as f32]);
        }
    }
    genzo_model::BrushStroke {
        size,
        points,
        ..Default::default()
    }
}

#[test]
#[ignore = "計測（手動で --release で実行する）"]
fn bench_brush_rasterize() {
    // ブラシのラスター（G の長辺 2048 セル。genzo-pipeline の local::brush）を塗る時間。G の外接矩形 1.0 × 0.75。
    // 初めから塗る（写真を開いたとき・使い回しがないとき）と、1 本を足して塗り足す（ブラシで 1 本描いたとき）。
    use genzo_pipeline::local::brush::{clear_cache, raster_for, rasterize};
    let g_box = (1.0f32, 0.75f32);
    let fill = || zigzag_stroke(0.05, 6, 0.05, 0.11);
    let cases: Vec<(&str, Vec<genzo_model::BrushStroke>)> = vec![
        (
            "brush_rasterize_one_pass",
            vec![zigzag_stroke(0.05, 1, 0.3, 0.0)],
        ),
        ("brush_rasterize_fill", vec![fill()]),
        ("brush_rasterize_fill_x5", (0..5).map(|_| fill()).collect()),
        (
            "brush_rasterize_large_x10",
            (0..10).map(|_| zigzag_stroke(0.15, 3, 0.1, 0.25)).collect(),
        ),
    ];
    for (name, strokes) in &cases {
        let points: usize = strokes.iter().map(|s| s.points.len()).sum();
        let result = Bench::batch(*name)
            .note("strokes", strokes.len().to_string())
            .note("points", points.to_string())
            .note("backend", "cpu")
            .run_warm(|| rasterize(strokes, g_box).unwrap())
            .unwrap();
        report(&result);
    }
    // 塗りつぶし 5 本の後に 1 本を足す（前の 5 本のラスターから塗り足す）。
    let before: Vec<_> = (0..5).map(|_| fill()).collect();
    let mut after = before.clone();
    after.push(fill());
    let result = Bench::latency("brush_append_one_stroke")
        .note("strokes", "5+1")
        .note("backend", "cpu")
        .run_cold(
            || {
                clear_cache();
                raster_for(&before, g_box);
            },
            || raster_for(&after, g_box).unwrap(),
        )
        .unwrap();
    report(&result);
}
