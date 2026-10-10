//! 回帰テストと計測の基盤。
//!
//! 合成テスト画像・合成 DNG の生成、基準画像との比較（許容誤差。IQ-07a / IQ-07b）、
//! 計測と記録のルール（docs/05_poc_and_roadmap.md の 1.8 節）に従った結果の出力を担当する
//! （M0 タスク 11・12、02 の MAINT-03・MAINT-07）。
//!
//! | モジュール | 内容 | 設計書 |
//! |---|---|---|
//! | [`image`] | 浮動小数点の画像（1・3・4 チャンネル） | 04 の 2.6 節（各境界のバッファ） |
//! | [`diff`] | 2 枚の画像の差: 8bit 換算の差（IQ-07a）と ΔE2000（IQ-07b）、許容差 | 05 の 1.8 節「指標の使い分け」、02 の IQ-07a / b |
//! | [`golden`] | 基準画像の保存・読み込み・比較・更新（`GENZO_UPDATE_GOLDEN=1`） | 05 の M0 タスク 11、02 の MAINT-03・IQ-08 |
//! | [`synth`] | 合成のシーンリニアの画像（グレーの階段・ゾーンプレート・細い線・エッジ・グラデーション） | 05 の PoC-3・PoC-5 |
//! | [`colorchecker`] | ColorChecker Classic の基準値（D50 → Bradford で D65）とパッチの画像 | 05 の PoC-4 |
//! | [`bayer`] | カメラ RGB からベイヤー配列の RAW（逆 WB・モザイク化・量子化）、合成 DNG | 04 の 2.1 節（ステージ 1〜3・8）・2.6 節 |
//! | [`bench`](mod@bench) | 計測（コールド・ウォーム、平均・95 パーセンタイル・最大、目標）、結果の記録と前回との比較 | 05 の M0 タスク 12・1.8 節、02 の MAINT-07 |
//! | [`env`](mod@env) | 計測の環境の情報（OS・CPU・ビルドの設定・GPU・ライブラリ） | 05 の 1.8 節「環境」 |
//! | [`stats`] | 平均・パーセンタイル（最近接順位法）の定義 | 05 の 1.8 節「報告する値」 |
//! | [`record`] | 入力の ID とハッシュ、使った現像設定の記録、ファイルの安全な書き込み | 05 の 1.8 節「入力」 |
//! | [`resample`] | 比較のための縮小（面積平均。IQ-07b の仮の基準） | 02 の IQ-07b、レビュー R-02 |
//! | [`preview`] | 確認用の PNG（16bit の画像、差分画像） | — |
//!
//! # 例: 合成画像を基準画像と比べる
//!
//! ```no_run
//! use genzo_color::RgbColorSpace;
//! use genzo_testkit::colorchecker::{ColorCheckerEdition, ColorCheckerLayout, color_checker_image};
//! use genzo_testkit::diff::Tolerance;
//! use genzo_testkit::golden::{GoldenSpec, GoldenStore};
//! use genzo_testkit::record::InputRef;
//!
//! let layout = ColorCheckerLayout { patch_size: 16, gap: 4 };
//! let scene = color_checker_image(
//!     ColorCheckerEdition::AfterNov2014,
//!     RgbColorSpace::Bt2020,
//!     layout,
//!     [0.02; 3],
//!     1.0,
//! )?;
//! let output = scene.clone(); // 実際には現像の処理の結果
//! // 合成の入力は、生成した画素ではなく生成のパラメータをハッシュにする（画素は OS の数学
//! // ライブラリで最下位のビットが変わりうるため。InputRef::from_float_image の注意を参照）。
//! let params = "colorchecker edition=after_nov2014 space=bt2020 patch=16 gap=4 background=0.02";
//! let spec = GoldenSpec::new("colorchecker_identity", Tolerance::iq07a())
//!     .with_input(InputRef::from_bytes("synthetic:colorchecker", params.as_bytes()));
//! GoldenStore::new(genzo_testkit::golden_dir!()).assert_matches(&spec, &output);
//! # Ok::<(), genzo_testkit::image::ImageError>(())
//! ```

pub mod bayer;
pub mod bench;
pub mod colorchecker;
pub mod diff;
pub mod env;
pub mod golden;
pub mod image;
pub mod preview;
pub mod record;
pub mod resample;
pub mod stats;
pub mod synth;

pub use bench::{Bench, BenchKind, BenchRecorder, BenchReport, BenchResult, Condition};
pub use diff::{
    DeltaEStats, Diff8Stats, LabConversion, MetricReport, Tolerance, delta_e2000_stats, diff_8bit,
};
pub use env::EnvironmentInfo;
pub use golden::{GoldenMode, GoldenOutcome, GoldenSpec, GoldenStore};
pub use image::{FloatImage, Rect};
pub use record::InputRef;
