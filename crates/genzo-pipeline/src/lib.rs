//! 現像パイプラインの CPU 基準実装。
//!
//! 処理ステージ（docs/04_architecture.md の 2.1 節・7.1 節の `Stage`）、段階 A0 / A1 / B / C
//! の分割と中間キャッシュ（2.2 節）、ガイドの計算と座標系（2.7 節）、タイル処理を担当する。
//!
//! CPU 版を正とし、GPU 版（`genzo-gpu`）はこの crate の結果との差が許容誤差以内であることを
//! テストで確認する（04 の 1.1 節・2.3 節、02 の MAINT-02・IQ-07a）。この crate は wgpu と
//! `genzo-gpu` に依存しない（`genzo-gpu` がこの crate に依存する。1.4 節の図との違いは
//! `genzo-gpu` の説明を参照）。GPU 版はステージの ID で対応付ける（[`stage::GpuStageLookup`]）。
//!
//! # 構成
//!
//! | モジュール | 内容 | 設計書 |
//! |---|---|---|
//! | [`image`] | 画像の型: [`Roi`]・[`RgbImage`]（f32 の RGB）・[`CfaImage`]（B0。飽和の印）・[`GrayImage`]・[`ImageTile`] | 2.1 節・2.6 節・7.1 節 |
//! | [`contract`] | 色の契約 B0〜B5（[`ColorContract`]） | 2.6 節 |
//! | [`version`] | 処理バージョン（[`ProcessVersion`]。未知の版はエラー） | 2.5 節・7.1 節 |
//! | [`stage`] | RGB のステージのトレイト [`Stage`]・[`StageParams`]・[`StageContext`]・[`Guide`]・GPU 版の検索（[`stage::GpuStageLookup`]）・[`run_stage_cpu`] | 7.1 節 |
//! | [`registry`] | ステージの登録表（[`StageRegistry`]。登録すると CPU / GPU の一致テストの対象） | 7.1 節 |
//! | [`sensor`] | センサー処理（ステージ 2〜8）: [`sensor::SensorStage`]・[`SensorPlan`]・段階 A1 のプレビュー（[`render_a1`]・[`A1Image`]）・RAW 以外の入口（[`SourceImage`]） | 2.1 節・2.2 節・2.6 節 |
//! | [`sensor::normalize`] | ステージ 2: 正規化（黒レベル・白レベル・飽和の印） | 2.1 節・2.6 節 B0 |
//! | [`sensor::wb`] | ステージ 3: ホワイトバランスと、飽和した画素の切りそろえ（仮の処理） | 2.1 節・2.6 節 B1 |
//! | [`sensor::demosaic`] | ステージ 5: RCD・双線形・2 × 2 の簡易処理 | 2.1 節・2.2 節 |
//! | [`sensor::color_matrix`] | ステージ 8: カメラ RGB → リニア BT.2020 | 2.6 節 |
//! | [`resample`] | 面積平均の縮小（決まった縮小フィルタ）とプレビューの寸法 | 2.2 節・IQ-07b |
//! | [`interp`] | 明示的な補間（1D LUT・双線形） | 2.3 節 |
//! | [`border`] | 画像の端の外の鏡映 | 2.7 節 |
//! | [`coords`] | 座標系 S / O / G / C・RAW の向き・回転（G）・ステージ 9 の入力の格子（[`SensorGrid`]） | 2.7 節 |
//! | [`sanitize`] | NaN・無限大の置き換え、対数の下限 | 2.3 節・2.6 節 |
//! | [`guide`] | 段階 B: ガイドの計算（[`compute_guide`]・ガイデッドフィルタ）とキャッシュのキーの項目（[`GuideSpec`]） | 2.2 節・2.7 節 |
//! | [`finish`] | 仕上げのステージ 9〜17 の一覧・段階 C の文脈（[`finish_context`]）・基準の実行とタイルの範囲（[`FinishPlan`]） | 2.1 節・2.2 節・2.6 節 |
//! | [`finish::geometry`] | ステージ 9: 向き・回転・切り抜き（1 回のリサンプリング。バイキュービック）と C の格子（[`finish::GeometryPlan`]） | 2.1 節・2.7 節 |
//! | [`finish::exposure`] | ステージ 10: 露光量・コントラスト | DEV-04・IQ-02 |
//! | [`finish::tone`] | ステージ 11: ハイライト・シャドウ・白・黒（ガイドを使うローカルトーンマッピング） | DEV-05・2.7 節・PoC-5 |
//! | [`finish::color`] | ステージ 13: 彩度・自然な彩度（OKLab） | DEV-08 |
//! | [`finish::scene_to_display`] | ステージ 15: シーン → ディスプレイ（B2 → B3） | 2.6 節 |
//! | [`finish::tone_curve`] | ステージ 16: トーンカーブ（単調な 3 次スプライン → 1D LUT） | DEV-07・2.6 節 |
//! | [`finish::output`] | ステージ 17a 画面・17b 書き出し（量子化とディザリング）・17c ヒストグラム | 2.6 節・DEV-26 |
//! | [`finish::reserved`] | v1 のステージ 6・7・12・14 の予約と、v1 で無視する設定の一覧 | 2.1 節 |
//! | [`engine`] | エンジン: 段階 A0 / A1 / B のキャッシュ（[`Engine`]・ヒット・ミスの回数）、プレビュー（[`Engine::render_preview`]）、等倍・書き出しのタイル処理（[`FullResolutionJob`]）、ルーペの範囲（[`Engine::render_region`]）、書き出し（[`Engine::export`]）、取り消し（[`RenderControl`]） | 2.2 節・2.4 節・2.7 節・4.1 節・6.1 節 |
//! | [`input`] | RAW 以外の入力: 符号化した RGB と ICC からリニア BT.2020（B2）へ（[`working_image_from_encoded`]） | 2.6 節 |
//!
//! # データの流れ
//!
//! ```text
//! SourceImage::Raw(RawImage, u16)                         SourceImage::Working（B2。JPEG など）
//!   │ SensorPlan::new(raw, settings, options)               │
//!   │  2. 正規化 → CfaImage（B0、飽和の印）                   │
//!   │  3. WB・飽和の切りそろえ → CfaImage（B1 の値）           │
//!   │  5. デモザイク（RCD / 2 × 2）→ RgbImage（B1）            │
//!   │  8. 色変換 → RgbImage（B2）                             │
//!   ├─ SensorPlan::render_roi(raw, roi)   … 等倍・書き出しのタイル（フル解像度、S 座標）
//!   └─ SensorPlan::render_preview(raw, 2560) / render_a1(source, …)
//!         → A1Image（B2、長辺 2560px、S の向き、向き・S との対応・警告を持つ）
//!
//! 段階 B（回転・A1 の項目が変わったとき）
//!   compute_guide(&a1, &GuideSpec::from_settings(settings)) → Guide（G 座標、長辺 512、log2 の輝度）
//!
//! 段階 C（スライダーを動かすたび）
//!   ctx = a1.finish_context(settings).with_guide(&guide)   … 回転・ステージ 9 の入力の格子・倍率
//!   FinishPlan::new(settings, &ctx).render_roi(&ctx, &source_tile, roi)
//!      9. ジオメトリ（S の格子 → C の格子）→ 10. 露光量・コントラスト → 11. トーン（ガイド）
//!      → 13. 色 → 15. シーン → ディスプレイ（B3）→ 16. トーンカーブ（B3）
//!   run_stage_cpu(&DisplayStage, …)        → 17a 画面（B4a）
//!   run_stage_cpu(&ExportStage::SRGB, …)   → 17b 書き出し（B4b）→ quantize_u8 / quantize_u16
//!   Histogram::compute(&b4, …)             → 17c ヒストグラム
//!
//! エンジン（上の流れをまとめ、段階の境目でキャッシュする。[`engine`]）
//!   Engine::load_source(id, settings, 展開) → PhotoSource（A0。RAW は Arc で保持）
//!   Engine::render_preview(source, settings, 品質, 長辺) → A1（キャッシュ）→ ガイド（必要なら・キャッシュ）
//!      → ステージ 9〜16（B3）〔→ 17a / 17b / 17c は PreviewRequest で〕
//!   Engine::full_resolution(source, settings) → FullResolutionJob（フル解像度・最終品質・プレビューのガイド）
//!      .render_tile / .render_region（ルーペ）/ .export（タイル → 〔縮小〕→ 17b → 量子化、ICC）
//! ```
//!
//! 段階 B・C の例（RAW 以外の入力から書き出しの 8bit まで）:
//!
//! ```
//! use std::sync::Arc;
//!
//! use genzo_model::{DevelopSettings, Orientation};
//! use genzo_pipeline::finish::output::{Dither, Histogram, quantize_u8};
//! use genzo_pipeline::finish::{ExportStage, FinishPlan};
//! use genzo_pipeline::guide::{GuideSpec, compute_guide};
//! use genzo_pipeline::{
//!     ColorContract, ImageTile, RgbImage, Roi, SensorOptions, SourceImage, Stage, render_a1,
//!     run_stage_cpu,
//! };
//!
//! // 中間のグレー（0.18）の 60 × 40 の画像（B2）。
//! let image = RgbImage::filled(60, 40, ColorContract::B2Working, [0.18; 3])?;
//! let source = SourceImage::Working { image: Arc::new(image), orientation: Orientation::Normal };
//! let mut settings = DevelopSettings::default();
//! settings.tone.shadows = 30.0;
//! let a1 = render_a1(&source, &settings, &SensorOptions::default(), 2560)?;
//! // 段階 B: ガイド（露光量・切り抜き・スライダーを変えても作り直さない）。
//! let guide = compute_guide(&a1, &GuideSpec::from_settings(&settings)?)?;
//! // 段階 C: ステージ 9〜16。
//! let ctx = a1.finish_context(&settings)?.with_guide(&guide);
//! let plan = FinishPlan::new(&settings, &ctx)?;
//! let source_tile = ImageTile::from_image(Roi::full(60, 40), a1.image.clone())?;
//! let (b3, _) = plan.render_full(&ctx, &source_tile)?;
//! assert_eq!(b3.contract(), ColorContract::B3DisplayReferred);
//! // ステージ 17b: sRGB への書き出しと 8bit への量子化（決定的なディザリング）。
//! let export = ExportStage::SRGB;
//! let params = export.params(&settings, &ctx)?.expect("書き出しは常に有効");
//! let (encoded, _) = run_stage_cpu(&export, &ctx, &b3, b3.roi, &params)?;
//! let bytes = quantize_u8(&encoded.image, (0, 0), Dither::DEFAULT_8BIT)?;
//! assert_eq!(bytes.len(), 60 * 40 * 3);
//! // ステージ 17c: ヒストグラム（度数の合計は画素数）。
//! let hist = Histogram::compute(&encoded.image, [0.2126, 0.7152, 0.0722])?;
//! assert_eq!(hist.luma.iter().sum::<u64>(), 60 * 40);
//! # Ok::<(), genzo_pipeline::PipelineError>(())
//! ```
//!
//! # 例
//!
//! ```
//! use std::sync::Arc;
//!
//! use genzo_model::{DevelopSettings, PhotoMetadata};
//! use genzo_pipeline::{ColorContract, SensorOptions, SourceImage, render_a1};
//! use genzo_raw::{CfaPattern, RawImage};
//!
//! // 中間のグレー（0.18）を写した 64 × 48 の RAW（RGGB、14bit、撮影時の WB は R 2・B 1.5）。
//! let wb = [2.0f32, 1.0, 1.5, 1.0];
//! let (black, white) = (512.0f32, 16383.0f32);
//! let data = (0..64 * 48)
//!     .map(|i| {
//!         let k = CfaPattern::RGGB.color_at(i % 64, i / 64).index();
//!         (black + 0.18 / wb[k] * (white - black)).round() as u16
//!     })
//!     .collect();
//! let raw = RawImage {
//!     width: 64,
//!     height: 48,
//!     cfa: CfaPattern::RGGB,
//!     data,
//!     black_level: [black; 4],
//!     white_level: white,
//!     as_shot_wb: wb,
//!     cam_xyz: None,
//!     metadata: PhotoMetadata::default(),
//! };
//! // 段階 A1: ステージ 2〜8 と、長辺 32px への縮小。
//! let source = SourceImage::Raw(Arc::new(raw));
//! let a1 = render_a1(&source, &DevelopSettings::default(), &SensorOptions::default(), 32)?;
//! assert_eq!(a1.image.dimensions(), (32, 24));
//! assert_eq!(a1.image.contract(), ColorContract::B2Working);
//! // カメラ行列がないので、BT.2020 とみなした警告が付く（色は正確ではない）。
//! assert_eq!(a1.warnings.len(), 1);
//! // 撮影時の WB で中立に戻る。
//! let p = a1.image.pixel(10, 10).unwrap();
//! assert!(p.iter().all(|v| (v - 0.18).abs() < 1e-3));
//! # Ok::<(), genzo_pipeline::PipelineError>(())
//! ```
//!
//! # 決めごと
//!
//! - **向き**: センサー処理（段階 A1）の出力は **S の向き**（RAW の向きを反映しない）。向きは
//!   [`A1Image::orientation`] で渡し、ステージ 9 が G 座標の最初の変換（S → O）として、回転・歪曲補正・
//!   切り抜きと 1 回のリサンプリングで反映する（[`coords`]）。
//! - **色の契約**: センサー処理の出力は B2（[`ColorContract::B2Working`]）。ステージは入力の契約を
//!   [`Stage::input_contract`] で宣言し、[`run_stage_cpu`] が確かめる。
//! - **端の外**: 各ステージの入力の端の外は鏡映（[`border::mirror_index`]。端の画素を重複させない反射）。
//! - **NaN・無限大**: 各ステージの出力で 0 に置き換える（[`run_stage_cpu`]・[`sensor::run_sensor_stage`]
//!   が行い、数を返す）。対数の前は [`sanitize::safe_log2`]（下限 2^−16）。
//! - **空間的なパラメータ**: G の長辺に対する割合で定義し、[`StageContext::length_px`] で画素に直す。
//! - **縮小**: 面積平均（[`resample::downscale_area`]）。リニアの値にだけ使う。
//! - **補間**: [`interp`] の明示的な式（1D LUT は既定 4096 点＋線形補間、3D LUT は genzo-color の四面体補間）。
//! - **処理バージョン**: [`ProcessVersion`] の `match` でアルゴリズムを切り替える（新しい版を足すと、
//!   すべてのステージでコンパイルエラーになり、扱いの書き忘れを防ぐ）。
//! - **並列化**: rayon で行・帯ごとに並列化してよいが、画素ごとの計算は分割によらないこと（スレッド数で
//!   結果が変わらない。テストで確認する）。
//! - **段階 C の格子**（2 段目）: ステージ 9 だけが格子を変える（S の格子 → C の格子。C の格子は
//!   G の格子のうち切り抜きの範囲で、密度は入力の格子と同じ。[`finish::geometry`]）。ステージ 10〜17 は
//!   C の格子の上の画素ごとの処理。タイルの範囲は [`FinishPlan::source_roi`] がステージ 9 の入力の
//!   格子の範囲を返す（画像の外は鏡映で埋めて渡す）。
//! - **回転**（2 段目）: 段階 B・C の文脈の [`CoordinateFrames::rotation_deg`] に入れる
//!   （[`finish_context`]）。ステージ 9・11 は設定の角度と一致するか確かめる。回転で生じる外側は
//!   鏡映で埋めず、切り抜きを画像の内側に制限する（[`finish::constrain_crop`]）。
//! - **ガイド**（2 段目）: 段階 A1 のプレビュー（切り抜く前の全体、露光量の前）から作る。ステージ 11 は
//!   文脈のガイドの座標系・処理バージョンが文脈と一致するか確かめる（違えば
//!   [`PipelineError::GuideMismatch`]）。キャッシュのキーの項目は [`guide`] の doc の表。
//! - **v1 の項目**（ノイズ軽減・レンズ補正・シャープ・マスク）: 処理バージョン v1 では値があっても
//!   適用しない（[`finish::reserved`]。[`finish::unimplemented_settings`] で警告できる）。
//!
//! - **エンジン**（3 段目）: 段階 A0 / A1 / B は最新の数件だけをキャッシュし（キーは genzo-model の
//!   `CacheKey`。[`engine::cache`]）、段階 C は毎回計算する。等倍・書き出しはタイル（既定
//!   [`DEFAULT_TILE_SIZE`]）ごとに段階 A1〜C を実行し、余白は各ステージの `input_roi` を出力側から
//!   たどる。ガイドはプレビューと同じもの（長辺 [`EngineConfig::preview_long_edge`] の最終品質の A1 から
//!   作ったもの）を使う。フル解像度のバッファは持たない（SCL-05）。
//! - **書き出しの縮小**（3 段目）: 2.4 節のとおり **フル解像度で処理してから縮小** する。縮小は B3（リニア）
//!   に対する面積平均（[`resample`] と同じ定義・同じ足し方）で、17b と量子化はその後。genzo-media には
//!   縮小済みの寸法で渡す（genzo-media の寸法の規則も `ExportSize::fit` なので縮小し直さない）。
//!
//! # 残りの担当（このファイルの一覧を更新すること）
//!
//! **genzo-gpu / genzo-api**: GPU 版のステージの実行（エンジンは CPU 版だけを実行する。どのステージを
//! GPU で処理できるかは [`FullResolutionJob::stage_backends`]）、ジョブの優先度とメモリの予算
//! （genzo-jobs。タイルの作業領域の概算は [`FullResolutionJob::estimate_tile_bytes`]）、取り消しの
//! トークンの受け渡し（[`RenderControl`]）、キャッシュ L0 / L1 の符号化と保存（B5 は
//! [`OutputTarget::CacheDisplayP3`] の出力を量子化したもの）、書き出しのファイル（genzo-media）。
//!
//! **v1 / PoC で決めること**: 歪曲補正（ステージ 9 の G → O を画素ごとの式にする）、ステージ 4・6・7・
//! 12・14、HSL・カラーグレーディング、トーンカーブの効果（周辺光量・粒子）。仮置きの式と定数
//! （コントラスト・トーン・ガイド・自然な彩度・ステージ 15・パラメトリックのカーブ）は PoC-4 / PoC-5
//! で見直す（各モジュールの doc に「仮置き」と書いた）。IQ-07b（縮小プレビューとフル解像度の一致）は、
//! 細線と強い補正の合成画像で満たせていない（tests/engine.rs の計測値。PoC-5 で処理の順序か目標を
//! 見直す）。タイルの大きさ・キャッシュの件数（[`engine`] の仮置きの定数）は PoC-3 で決める。
//!
//! **IQ-07b と切り抜き・回転の位置合わせ（PoC-5 で決める）**: C の格子は「G の格子（密度は入力の格子と
//! 同じ）のうち切り抜きの範囲」で、切り抜きの端をそれぞれの格子に丸める（[`finish::geometry`]）。この
//! ため、回転か切り抜きがあると、プレビューの画素と「フル解像度を四隅を合わせてプレビューの大きさに
//! 縮小した画素」（IQ-07b の比較）の位置が、最大で **プレビューの 0.5 画素＋フル解像度の 0.5 画素**
//! ずれる（向きだけなら一致する。tests/engine.rs の
//! `preview_and_full_resolution_registration_is_within_half_a_preview_pixel`）。見た目の差は小さいが、
//! 細線・境界では ΔE2000 が大きくなる（滑らかな三角波のシーン 300 × 201 → 長辺 100 でも、切り抜きだけで
//! 平均 約 2、回転 7° で平均 約 5。回転・切り抜きなしでは 0。このコンテナでの参考値）。IQ-07b を回転・
//! 切り抜きのある画像で測る前に、(a) プレビューの C の格子を書き出しの切り抜きの端（フル解像度の格子に
//! 丸めた端）に合わせる（プレビューでは「切り抜きを変えても範囲内の画素がビット単位で同じ」でなくなる）か、
//! (b) 比較の基準を、プレビューの画素が覆う G の範囲で平均したものにするかを決める。

pub mod border;
pub mod contract;
pub mod coords;
pub mod engine;
pub mod error;
pub mod finish;
pub mod guide;
pub mod image;
pub mod input;
pub mod interp;
pub mod registry;
pub mod resample;
pub mod sanitize;
pub mod sensor;
pub mod stage;
pub mod version;

pub use contract::ColorContract;
pub use coords::{CoordinateFrames, SensorGrid};
pub use engine::{
    CacheCounters, DEFAULT_TILE_SIZE, Engine, EngineConfig, EngineStats, ExportOptions,
    ExportPixels, ExportedImage, FullResolutionJob, NeverCancel, OutputTarget, PhotoSource,
    PreviewRender, PreviewRequest, RegionRender, RenderControl, SourceId,
};
pub use error::{PipelineError, Result};
pub use finish::{FinishPlan, finish_context};
pub use guide::{GuideSpec, compute_guide};
pub use image::{CfaImage, GrayImage, ImageTile, RgbImage, Roi};
pub use input::working_image_from_encoded;
pub use registry::{GpuCoverage, StageRegistry};
pub use resample::{DEFAULT_PREVIEW_LONG_EDGE, downscale_area, preview_size};
pub use sensor::demosaic::DemosaicMethod;
pub use sensor::wb::SaturationClip;
pub use sensor::{A1Image, SensorOptions, SensorPlan, SensorWarning, SourceImage, render_a1};
pub use stage::{
    GpuStage, GpuStageLookup, Guide, NoGpu, Stage, StageContext, StageParams, run_stage_cpu,
};
pub use version::ProcessVersion;
