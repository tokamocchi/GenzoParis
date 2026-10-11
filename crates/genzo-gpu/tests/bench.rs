//! 段階 C の GPU 時間などの計測（PoC-3 の準備。CI では実行しない。`#[ignore]`）。
//!
//! ```text
//! cargo test -p genzo-gpu --release --test bench -- --ignored --nocapture --test-threads 1
//! ```
//!
//! **注意**: このコンテナの GPU は Mesa の llvmpipe（Vulkan のソフトウェア実装。CPU で動く）で、
//! ここで測った数値は **性能の判断に使えない**。PERF-01（露光量のドラッグが 95 パーセンタイルで 33ms
//! 以内）・PERF-01b（WB のドラッグが 15fps 以上）・PERF-10（書き出し 3 秒以内）の判定は、実機（M1・
//! RTX 3080）で同じ計測を実行して行う（05 の PoC-3）。測るのは呼び出しから結果が CPU に戻るまでの
//! 時間（GPU の処理・転送・CPU 側の準備を含む）で、入力から画面への表示までの時間ではない。
//!
//! 結果は標準出力に表示する。環境変数 `GENZO_BENCH_DIR` を設定したときだけ、genzo-testkit の記録の
//! 形式でそのディレクトリに書く（リポジトリの中にファイルを作らないため）。

mod common;

use std::time::Duration;

use genzo_gpu::GpuPreviewOptions;
use genzo_model::{DevelopSettings, ExportSettings, Orientation, RenderQuality};
use genzo_pipeline::{ExportOptions, NeverCancel, OutputTarget, PreviewRequest};
use genzo_raw::CfaPattern;
use genzo_testkit::bench::{Bench, BenchRecorder, BenchResult};

use common::*;

/// 段階 A1 のプレビューの寸法（α7 IV を長辺 2560px にしたもの。2.2 節）。
const PREVIEW: (u32, u32) = (2560, 1707);

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
#[ignore = "計測（手動で --release で実行する。llvmpipe の数値は性能の判断に使えない）"]
fn bench_stage_c_preview_2560_gpu() {
    // 露光量のドラッグ（段階 C。A1・ガイドはキャッシュ）: ステージ 9〜16 → 17a → 17c。B3 と 17a の出力は
    // 読み出さない（ビューポートは GPU 上の結果を使う想定。ヒストグラムだけ読み出す）。入力は B2 の 2560 × 1707（RAW 以外の経路なので A1 = 入力）。
    let g = gpu_or_skip!();
    g.compile_all().unwrap();
    let (w, h) = PREVIEW;
    let source = working_source(scene_b2(w, h), Orientation::Normal);
    let e = engine(2560, 1024);
    let mut s = strong_settings();
    let request = PreviewRequest::new(RenderQuality::Final, 2560)
        .with_output(OutputTarget::assumed_srgb_display().unwrap())
        .with_histogram();
    let options = GpuPreviewOptions {
        download_b3: false,
        download_output: false,
        ..Default::default()
    };
    g.render_preview(&e, &source, &s, &request, &options)
        .unwrap();
    let mut ev = 0.0f32;
    let result = Bench::latency("gpu_stage_c_preview_2560")
        .target(Duration::from_millis(33))
        .note("resolution", "2560x1707")
        .note("backend", "gpu")
        .note("adapter", g.context().summary().to_string())
        .develop_settings(&s)
        .run_warm(|| {
            ev = if ev > 1.0 { 0.0 } else { ev + 0.1 };
            s.exposure_ev = ev;
            g.render_preview(&e, &source, &s, &request, &options)
                .unwrap()
        })
        .unwrap();
    report(&result);
    let st = e.stats();
    assert_eq!(st.a1.misses, 1, "A1 は作り直さない");
    assert_eq!(st.guide.misses, 1, "ガイドは作り直さない");
}

#[test]
#[ignore = "計測（手動で --release で実行する。llvmpipe の数値は性能の判断に使えない）"]
fn bench_wb_drag_draft_a1_gpu_a7iv() {
    // WB のドラッグ（段階 A1 の簡易処理を GPU で → 段階 C）。RAW は GPU に 1 回だけ転送する。
    let g = gpu_or_skip!();
    g.compile_all().unwrap();
    let (w, h) = A7IV;
    let source = raw_source(raw_from(&camera_scene(w, h), CfaPattern::RGGB, false));
    let e = engine(2560, 1024);
    let mut s = DevelopSettings::default();
    let request = PreviewRequest::new(RenderQuality::Draft, 2560)
        .with_output(OutputTarget::assumed_srgb_display().unwrap());
    let options = GpuPreviewOptions {
        download_b3: false,
        download_output: false,
        ..Default::default()
    };
    let mut k = 4000.0f32;
    let result = Bench::latency("gpu_wb_drag_draft_a7iv")
        .target(Duration::from_millis(66))
        .note("resolution", "7008x4672 → 2560x1707")
        .note("backend", "gpu")
        .note("adapter", g.context().summary().to_string())
        .run_warm(|| {
            k = if k > 7000.0 { 4000.0 } else { k + 150.0 };
            custom_wb(&mut s, k, 0.0);
            g.render_preview(&e, &source, &s, &request, &options)
                .unwrap()
        })
        .unwrap();
    report(&result);
}

#[test]
#[ignore = "計測（手動で --release で実行する。llvmpipe の数値は性能の判断に使えない）"]
fn bench_export_jpeg8_gpu() {
    // 書き出し（タイル 1024。センサー処理は CPU 版、段階 C と 17b・量子化は GPU）。寸法は環境変数
    // GENZO_BENCH_EXPORT_FULL=1 で α7 IV、既定は 1/4 の面積（llvmpipe では時間がかかるため）。
    let g = gpu_or_skip!();
    g.compile_all().unwrap();
    let full = std::env::var("GENZO_BENCH_EXPORT_FULL").is_ok_and(|v| v == "1");
    let (w, h) = if full { A7IV } else { (A7IV.0 / 2, A7IV.1 / 2) };
    let source = raw_source(raw_from(&camera_scene(w, h), CfaPattern::RGGB, false));
    let e = engine(2560, 1024);
    let mut s = strong_settings();
    s.exposure_ev = 0.3;
    let export = ExportSettings::default();
    let result = Bench::batch("gpu_export_jpeg8")
        .note("resolution", format!("{w}x{h}"))
        .note("backend", "gpu")
        .note("tile", "1024")
        .note("adapter", g.context().summary().to_string())
        .iterations(3)
        .run_warm(|| {
            g.export(
                &e,
                &source,
                &s,
                &export,
                &ExportOptions::default(),
                &NeverCancel,
            )
            .unwrap()
        })
        .unwrap();
    report(&result);
}
