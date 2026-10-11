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
