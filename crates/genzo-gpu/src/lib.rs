//! 現像パイプラインの GPU 版（wgpu・WGSL）。
//!
//! `genzo-pipeline` が定義する処理ステージの GPU 実装と、CPU 版との一致テスト
//! （docs/04_architecture.md の 2.3 節、02 の IQ-07a）を担当する。GPU を使う処理は本体プロセスだけで
//! 行い、GPU スレッド 1 本がデバイスとキューを専有する（04 の 1.2 節・1.3 節）。
//!
//! # 構成
//!
//! | モジュール | 内容 | 設計書 |
//! |---|---|---|
//! | [`context`] | ヘッドレスの初期化（[`GpuContext`]。アダプターがなければ `None`）、エラーのスコープ、デバイスの消失、完了の待ち | 1.3 節・6.3 節 |
//! | [`stages`] | GPU 版の登録表（[`GpuStageRegistry`]。genzo-pipeline の `GpuStageLookup` の実装。ステージの ID で対応付ける） | 7.1 節 |
//! | `kernels` | WGSL の組み立て（CPU 版と同じビット列の定数を生成）・コンパイル・実行 | 2.3 節 |
//! | [`renderer`] | [`GpuRenderer`]: ステージ単位の実行と CPU 版との混在（[`GpuRenderer::run_stages`]）、17c のヒストグラム、17b の量子化、センサーのステージ | 2.3 節・7.1 節 |
//! | [`a1`] | 段階 A1 の簡易処理（2 × 2）の GPU 版（[`GpuRenderer::render_a1_draft`]） | 2.2 節 |
//! | [`preview`] | プレビューの段階 C（[`render_preview_gpu`]・[`GpuRenderer::render_preview`]）。B3 または 17a / 17b の結果とヒストグラム | 2.2 節・6.2 節 |
//! | [`export`] | 書き出しのタイル処理（[`GpuRenderer::export`]。1 タイルずつ投入）と CPU 版への切り替え（[`export_with_fallback`]） | 2.4 節・6.1 節 |
//! | [`tolerance`] | ステージごとの許容誤差（仮置き）と比べ方 | IQ-07a |
//! | [`error`] | [`GpuError`] | 6.3 節 |
//!
//! WGSL は `src/shaders/` にある（`common.wgsl`・`pixel_io.wgsl`・`color_lib.wgsl` が共通の部分、
//! `finish.wgsl`（ステージ 10・11・13・15・16）・`output.wgsl`（17a・17b）・`geometry.wgsl`（9）・
//! `histogram.wgsl`（17c）・`quantize.wgsl`（17b の量子化）・`sensor.wgsl`（2・3・5 の簡易処理）・
//! `resample.wgsl`（8 と縮小））。
//!
//! # 依存の向き（04 の 1.4 節の図との違い）
//!
//! 1.4 節の図は `genzo-pipeline` → `genzo-gpu` の向きだが、この workspace では
//! **`genzo-gpu` が `genzo-pipeline` に依存する**。CPU 基準実装（`genzo-pipeline`）を先に作り、GPU 版と
//! CPU 版との一致テストをこの crate に置くため。`genzo-pipeline` は wgpu に依存せず、7.1 節の
//! `Stage::gpu()` は「ステージの ID で `GpuStageLookup` を引く」形で、この crate の
//! [`GpuStageRegistry`] がその実装になる。GPU 版を実行するのはこの crate の [`GpuRenderer`]
//! （エンジンの `Engine` は CPU 版だけを実行する）。
//!
//! # 2.3 節の決めごとの守り方
//!
//! - 浮動小数点は f32。CPU 版の式と同じ順序で書き、行列は行ごとに左から足す（`m * v` は使わない）。
//!   定数は CPU 版の f32 の値を 16 進の浮動小数点の表記で埋め込む（10 進への丸めの差が出ない）。
//! - 「高速だが不正確な演算」の書き方をしない（近似の関数、逆数の掛け算への置き換えなど）。WGSL に
//!   ない立方根だけは `pow(x, 1/3)` にニュートン法の補正を 1 回かけて作る。
//! - テクスチャのハードウェア補間は使わない。双線形（ガイド）・バイキュービック（ステージ 9）・1D LUT
//!   （ステージ 16）・3D LUT の四面体補間（17a）はすべてストレージバッファから自前で補間する。
//! - 端の外は CPU 版と同じ鏡映（`mirror_index`）。
//! - ヒストグラムは整数の原子的な加算（足す順序によらない）。縮小の和は CPU 版と同じ順序で足す。
//! - NaN・無限大は各ステージの出力で 0 に置き換え、原子的なカウンターで数える。判定はビット列で行う
//!   （コンパイラが「NaN・無限大はない」と仮定しても消えないように）。
//! - GPU 版のステージは **入力が有限であること** を前提にする（段階 A1 とすべてのステージの出力は NaN・
//!   無限大を 0 に置き換え済みなので、パイプラインの経路では常に満たす）。WGSL の `min`・`max`・`clamp` は
//!   NaN の扱いを規定しない（Rust の `f32::max` は NaN でない方を返す）ので、[`GpuRenderer::run_stage`] などに
//!   NaN を含む入力を直接渡すと、その画素の他のチャンネルの値が CPU 版と違いうる（出力は有限）。
//! - 一致テスト（`tests/consistency.rs`）は、登録したすべてのステージを、合成の画像・境界の値（ゼロ・負の値・
//!   対数の下限の前後・伝達関数の境目）・複数の設定で比べ、ステージ 9 は出力の端の画素（鏡映を使う）を含む
//!   全体で比べる。**CI のランナーでは比較を飛ばす**（2.3 節。`tests/common` の doc）。
//! - それでも WGSL は演算の結合・融合（積和演算）を許し、`exp2`・`log2`・`pow`・割り算の精度にも
//!   幅があるので、**完全な一致は保証できない**。差はステージごとの許容誤差（[`tolerance`]。仮置き）で
//!   確かめる。
//! - **wgpu の Metal のバックエンドは、シェーダーを Metal の既定（高速な数学）でコンパイルする**
//!   （wgpu-hal 30 の `MTLCompileOptions` に設定がなく、この crate からは変えられない）。M1 での差は
//!   PoC-3 で測る（上流（wgpu）への要望の候補）。
//!
//! # 性能について
//!
//! この環境（Linux のコンテナ）の GPU は Mesa の llvmpipe（Vulkan のソフトウェア実装）で、計測の数値
//! （`tests/bench.rs`）は **性能の判断に使えない**。PERF-01（露光量のドラッグが 33ms 以内）・PERF-10
//! （書き出し 3 秒以内）は実機（M1・RTX 3080）の PoC-3 で測る。段階 C はステージごとに 1 回のディスパッチ
//! （ステージの間は GPU のメモリを経由する）で、まとめて 1 つのカーネルにする最適化は PoC-3 の結果を
//! 見て決める。
//!
//! # 例
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use genzo_model::{DevelopSettings, FileId, Orientation, RenderQuality};
//! use genzo_pipeline::{
//!     ColorContract, Engine, EngineConfig, OutputTarget, PhotoSource, PreviewRequest, RgbImage,
//!     SourceId, SourceImage,
//! };
//! use genzo_gpu::{GpuPreviewOptions, GpuRenderer};
//!
//! // GPU がなければ CPU 版で処理する（ここでは終わる）。
//! let Some(gpu) = GpuRenderer::from_env()? else { return Ok(()) };
//! let image = RgbImage::filled(120, 80, ColorContract::B2Working, [0.18; 3])?;
//! let source = PhotoSource::new(
//!     SourceId::new(FileId::new(1), 1),
//!     SourceImage::Working { image: Arc::new(image), orientation: Orientation::Normal },
//! )?;
//! let engine = Engine::new(EngineConfig::default())?;
//! let mut settings = DevelopSettings::default();
//! settings.exposure_ev = 0.5;
//! let request = PreviewRequest::new(RenderQuality::Final, 2560)
//!     .with_output(OutputTarget::assumed_srgb_display()?)
//!     .with_histogram();
//! let preview = gpu.render_preview(&engine, &source, &settings, &request, &GpuPreviewOptions::default())?;
//! assert_eq!(preview.histogram.unwrap().pixel_count, 120 * 80);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # 未実装（v1・PoC で決めること）
//!
//! - RCD（最終品質のデモザイク）・双線形の GPU 版（CPU 版で処理する）。等倍・書き出しのセンサー処理
//!   （ステージ 2〜8）は CPU 版。
//! - 段階 B（ガイドの計算）の GPU 版（CPU 版で計算し、ステージ 11 が GPU で参照する）。
//! - ルーペの範囲（`render_region`）の GPU 版。
//! - 複数のステージを 1 つのカーネルにまとめる最適化（PoC-3 の計測の後）。

pub mod a1;
pub mod context;
pub mod error;
pub mod export;
mod kernels;
pub mod preview;
pub mod renderer;
pub mod stages;
pub mod tolerance;

pub use a1::A1_BAND_ROWS;
pub use context::{DEFAULT_WAIT_TIMEOUT, GpuAdapterSummary, GpuContext, GpuContextOptions};
pub use error::{GpuError, Result};
pub use export::{ExportBackend, GpuExportOutcome, export_with_fallback};
pub use preview::{
    DRAFT_CACHE_CAPACITY, GpuCacheCounters, GpuPreviewOptions, GpuPreviewRender, render_preview_gpu,
};
pub use renderer::{ChainOutput, GpuRenderer, GpuTile, StageStep};
pub use stages::{GpuStageEntry, GpuStageKind, GpuStageRegistry};
pub use tolerance::{
    DEFAULT_STAGE_TOLERANCE, DiffReport, FINAL_OUTPUT_MAX_DIFF_8BIT, StageTolerance, compare,
    compare_pixels, max_diff_8bit, stage_tolerance,
};
