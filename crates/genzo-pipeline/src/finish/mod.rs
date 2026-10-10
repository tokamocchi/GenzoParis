//! 仕上げのステージ（docs/04_architecture.md の 2.1 節のステージ 9〜17。2.2 節の段階 C）。
//!
//! | ステージ | モジュール | 型 | ID | 契約（入力 → 出力） | 設計書 |
//! |---|---|---|---|---|---|
//! | 9. ジオメトリ（向き・回転・切り抜き） | [`geometry`] | [`GeometryStage`] | `finish.geometry` | B2（S の格子）→ B2（C の格子） | 2.1 節・2.7 節、DEV-15 |
//! | 10. 露光量 | [`exposure`] | [`ExposureStage`] | `finish.exposure` | B2 → B2 | DEV-04、IQ-02 |
//! | 10 の後. コントラスト | [`exposure`] | [`ContrastStage`] | `finish.contrast` | B2 → B2 | DEV-04（仮置き） |
//! | 11. トーン（ハイライト・シャドウ・白・黒） | [`tone`] | [`ToneStage`] | `finish.tone` | B2 → B2（ガイドを使う） | DEV-05、2.7 節、PoC-5 |
//! | 12. テクスチャ・明瞭度・かすみの除去（v1） | [`reserved`] | [`TextureStage`] | `finish.texture` | B2 → B2（予約） | DEV-11 |
//! | 13. 色（彩度・自然な彩度） | [`color`] | [`ColorStage`] | `finish.color` | B2 → B2 | DEV-08 |
//! | 14. シャープ（v1） | [`reserved`] | [`SharpenStage`] | `finish.sharpen` | B2 → B2（予約） | DEV-12 |
//! | 15. シーン → ディスプレイ変換 | [`scene_to_display`] | [`SceneToDisplayStage`] | `finish.scene_to_display` | B2 → B3 | 2.6 節「トーンの圧縮」 |
//! | 16. トーンカーブ | [`tone_curve`] | [`ToneCurveStage`] | `finish.tone_curve` | B3 → B3 | DEV-07、2.6 節 |
//! | 17a. 画面 | [`output`] | [`DisplayStage`] | `output.display` | B3 → B4a | 2.6 節、5 章 |
//! | 17b. 書き出し・キャッシュ | [`output`] | [`ExportStage`] | `output.export.*`・`output.cache.display_p3` | B3 → B4b / B5 | 2.6 節、IQ-06・IQ-08 |
//! | 17c. ヒストグラム | [`output`] | [`Histogram`] | — | B4a / B4b / B5 → 度数 | DEV-26 |
//! | 段階 B. ガイド | [`crate::guide`] | [`crate::guide::compute_guide`] | — | B2（A1）→ log2 の輝度 | 2.7 節 |
//!
//! どのステージも段階 C（genzo-model の `Phase::C`）。ガイドだけが段階 B（`Phase::B`）。
//!
//! # 格子と座標
//!
//! ステージ 9 だけが格子を変える: 入力は [`StageContext::sensor_grid`]（S の向きの格子。段階 A1 の
//! プレビューやフル解像度のタイル）で、出力は C の格子（[`GeometryPlan`]）。ステージ 10〜17 は
//! C の格子の上の画素ごとの処理（ステージ 11 はガイドを G の座標で引く）。タイルの範囲（[`Roi`]）は、
//! ステージ 9 の入力は S の格子、それより後は C の格子の座標。ステージ 9 を飛ばす場合（回転・
//! 切り抜きなし、向きが通常）は C の格子 = S の格子。
//!
//! # 色の契約（2.6 節）
//!
//! - B2（ステージ 9〜14）: リニア BT.2020・D65。上限なし、負の値（色域外）を保持する。
//! - B3（ステージ 15 の出力〜16）: リニア BT.2020・D65、ディスプレイの白 = 1。輝度は 0〜1、負の値を保持。
//! - B4a / B4b / B5: 0〜1 の符号化した値（[`output`]）。
//!
//! # GPU 版と一致させるための決めごと（2.3 節）
//!
//! - 画素の計算は f32、式の順序は各モジュールの doc のとおり（融合積和演算は使わない）。係数は f64 で
//!   求めて f32 に丸め、GPU 版にも同じ値を渡す（パラメータの型の公開されたフィールド）。
//! - 補間は自前の式（ステージ 9 のバイキュービック、ガイドの双線形、カーブの 1D LUT、画面の 3D LUT）。
//! - 対数の前に下限（2^−16。[`crate::sanitize::safe_log2`]）、指数の前に上限（[`LOG2_CEILING`]）を設ける。
//! - 画素ごとの計算は C の格子の全体での座標だけに依存し、タイルの分け方・スレッド数によらない。
//! - 出力の NaN・無限大は [`run_stage_cpu`] が 0 に置き換える（2.6 節）。

pub mod color;
pub mod exposure;
pub mod geometry;
pub mod output;
pub mod reserved;
pub mod scene_to_display;
pub mod tone;
pub mod tone_curve;

use genzo_color::RgbColorSpace;
use genzo_model::{DevelopSettings, RenderQuality};
use rayon::prelude::*;

pub use self::color::ColorStage;
pub use self::exposure::{ContrastStage, ExposureStage};
pub use self::geometry::{GeometryPlan, GeometryStage, constrain_crop};
pub use self::output::{
    DisplayStage, DisplayTransform, Dither, ExportStage, ExportTransform, Histogram, quantize_u8,
    quantize_u16,
};
pub use self::reserved::{
    SharpenStage, TextureStage, UnimplementedSetting, unimplemented_settings,
};
pub use self::scene_to_display::SceneToDisplayStage;
pub use self::tone::ToneStage;
pub use self::tone_curve::ToneCurveStage;

use crate::border::mirror_cover;
use crate::coords::{CoordinateFrames, SensorGrid};
use crate::error::{PipelineError, Result};
use crate::image::{ImageTile, Roi};
use crate::sensor::A1Image;
use crate::stage::{Stage, StageContext, StageParams, run_stage_cpu};
use crate::version::ProcessVersion;

const BT2020_LUMINANCE_F64: [f64; 3] = RgbColorSpace::Bt2020.luminance_coefficients();

/// BT.2020 の輝度の係数（genzo-color の RGB → XYZ の行列の 2 行目を f32 に丸めた値。
/// 約 0.2627・0.6780・0.0593）。
pub const BT2020_LUMINANCE: [f32; 3] = [
    BT2020_LUMINANCE_F64[0] as f32,
    BT2020_LUMINANCE_F64[1] as f32,
    BT2020_LUMINANCE_F64[2] as f32,
];

/// 中間グレー（シーンリニア）。
pub const MIDDLE_GRAY: f32 = 0.18;

/// `log2(0.18)`（= −2.4739311883…。f32 に丸めた値）。
pub const LOG2_MIDDLE_GRAY: f32 = -2.473_931_f32;

/// 対数の領域で明るさを変えるステージ（コントラスト・トーン）の結果の上限（log2）。`2^64` を超える
/// 値は抑え、`exp2` のあふれ（無限大 → 0 への置き換え）で明るい画素が黒くならないようにする。
pub const LOG2_CEILING: f32 = 64.0;

/// BT.2020 の輝度 `w0·R + w1·G + w2·B`（左から順に足す）。
#[inline]
pub fn luminance(c: [f32; 3]) -> f32 {
    BT2020_LUMINANCE[0] * c[0] + BT2020_LUMINANCE[1] * c[1] + BT2020_LUMINANCE[2] * c[2]
}

/// 画素ごとのステージの共通の処理: 出力の画素 (x, y)（格子の座標）ごとに `f(x, y, 入力の画素)` を
/// 書く。入力は出力の範囲を含むこと（[`run_stage_cpu`] が確かめる）。行ごとに並列化する。
pub(crate) fn map_pixels(
    input: &ImageTile,
    output: &mut ImageTile,
    f: impl Fn(i64, i64, [f32; 3]) -> [f32; 3] + Sync,
) -> Result<()> {
    let roi = output.roi;
    let Some((ox, oy)) = roi.offset_in(&input.roi) else {
        return Err(PipelineError::InvalidRoi {
            roi,
            reason: "入力のタイルが出力の範囲を覆っていない",
        });
    };
    let in_w = input.roi.width as usize;
    let w = roi.width as usize;
    let src = input.image.data();
    output
        .image
        .data_mut()
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(j, row)| {
            let start = (oy + j) * in_w + ox;
            let y = i64::from(roi.y) + j as i64;
            for (i, (dst, s)) in row.iter_mut().zip(&src[start..start + w]).enumerate() {
                *dst = f(i64::from(roi.x) + i as i64, y, *s);
            }
        });
    Ok(())
}

/// シーンの処理（ステージ 9〜16）の組み込みのステージ（処理の順）。
pub fn scene_stages() -> [&'static dyn Stage; 9] {
    [
        &GeometryStage,
        &ExposureStage,
        &ContrastStage,
        &ToneStage,
        &TextureStage,
        &ColorStage,
        &SharpenStage,
        &SceneToDisplayStage,
        &ToneCurveStage,
    ]
}

/// 段階 C（ステージ 9〜17）の文脈を作る。
///
/// - 処理バージョンは丸める前の値で確かめる（未知の版はエラー）。
/// - `frames`（S の寸法と向き）に、設定の角度補正（丸めた値）を回転として加える。
/// - `grid` はステージ 9 の入力の格子。`resolution_scale` は格子の長辺の倍率
///   （[`SensorGrid::resolution_scale`]）。
/// - ガイドは [`StageContext::with_guide`] で別に付ける。
pub fn finish_context<'a>(
    settings: &DevelopSettings,
    frames: CoordinateFrames,
    grid: SensorGrid,
    quality: RenderQuality,
) -> Result<StageContext<'a>> {
    let version = ProcessVersion::from_u32(settings.process_version)?;
    let s = settings.normalized();
    let frames = frames.with_rotation(f64::from(s.geometry.angle_deg))?;
    let grid = SensorGrid::new(grid.width, grid.height, grid.extent)?;
    Ok(StageContext::new(version, frames)
        .with_sensor_grid(grid)
        .with_resolution_scale(grid.resolution_scale())
        .with_quality(quality))
}

impl A1Image {
    /// この画像を入力にした段階 C の文脈（[`finish_context`]）。
    pub fn finish_context<'a>(&self, settings: &DevelopSettings) -> Result<StageContext<'a>> {
        finish_context(settings, self.frames(), self.sensor_grid(), self.quality)
    }
}

/// シーンの処理（ステージ 9〜16）の計画: 有効なステージとそのパラメータ（タイルに分けない基準の
/// 実行と、タイル処理の範囲の計算に使う）。
///
/// 出力は B3（C の格子）。画面・書き出し（ステージ 17）は、この結果に [`DisplayStage`]・
/// [`ExportStage`] を [`run_stage_cpu`] で適用する。キャッシュとタイルの並べ方はエンジン（3 段目）が
/// 決める。
#[derive(Clone)]
pub struct FinishPlan {
    stages: Vec<(&'static dyn Stage, StageParams)>,
    geometry: GeometryPlan,
}

impl std::fmt::Debug for FinishPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinishPlan")
            .field("stages", &self.active_stage_ids())
            .field("output_size", &self.output_size())
            .finish()
    }
}

impl FinishPlan {
    /// 現像設定と文脈からパラメータを求める（ガイドを使うステージがあれば、文脈にガイドが必要）。
    pub fn new(settings: &DevelopSettings, ctx: &StageContext<'_>) -> Result<Self> {
        let geometry = GeometryPlan::from_settings(settings, ctx)?;
        let mut stages = Vec::new();
        for stage in scene_stages() {
            if let Some(p) = stage.params(settings, ctx)? {
                stages.push((stage, p));
            }
        }
        Ok(Self { stages, geometry })
    }

    /// ジオメトリ（C の格子と S の格子の対応）。
    pub fn geometry(&self) -> &GeometryPlan {
        &self.geometry
    }

    /// 出力（C の格子）の寸法。
    pub fn output_size(&self) -> (u32, u32) {
        self.geometry.output_size()
    }

    /// 有効な（飛ばさない）ステージの ID（処理の順）。
    pub fn active_stage_ids(&self) -> Vec<&'static str> {
        self.stages.iter().map(|(s, _)| s.id()).collect()
    }

    /// 有効なステージとパラメータ（処理の順）。
    pub fn stages(&self) -> &[(&'static dyn Stage, StageParams)] {
        &self.stages
    }

    /// 各ステージの入力の範囲（出力側からたどる。`[0]` が最初のステージの入力、最後が `output_roi`）。
    pub fn stage_rois(&self, ctx: &StageContext<'_>, output_roi: Roi) -> Vec<Roi> {
        let mut rois = vec![output_roi; self.stages.len() + 1];
        for k in (0..self.stages.len()).rev() {
            let (stage, p) = &self.stages[k];
            rois[k] = stage.input_roi(rois[k + 1], p, ctx);
        }
        rois
    }

    /// 出力の範囲 `output_roi`（C の格子）に必要な、ステージ 9 の入力の格子の範囲（画像の外に
    /// はみ出してよい）。
    pub fn source_roi(&self, ctx: &StageContext<'_>, output_roi: Roi) -> Roi {
        self.stage_rois(ctx, output_roi)[0]
    }

    /// 出力の範囲 `output_roi`（C の格子）を計算する（B3）。`source` はステージ 9 の入力の格子
    /// （B2）のタイルで、[`source_roi`](Self::source_roi) を含むか、鏡映で折り返した位置を含む
    /// （画像全体を渡せばよい）こと。置き換えた NaN・無限大の数も返す。
    pub fn render_roi(
        &self,
        ctx: &StageContext<'_>,
        source: &ImageTile,
        output_roi: Roi,
    ) -> Result<(ImageTile, u64)> {
        let rois = self.stage_rois(ctx, output_roi);
        let grid = ctx.sensor_grid;
        let first = rois[0];
        let mut tile = if source.roi.contains_roi(&first) {
            source.crop(first)?
        } else {
            let cover =
                mirror_cover(first, grid.width, grid.height).ok_or(PipelineError::InvalidRoi {
                    roi: first,
                    reason: "入力の範囲が空",
                })?;
            if !source.roi.contains_roi(&cover) {
                return Err(PipelineError::InvalidRoi {
                    roi: source.roi,
                    reason: "入力のタイルが必要な範囲を覆っていない",
                });
            }
            source.gather_mirror(first, grid.width, grid.height)?
        };
        let mut replaced = 0;
        for ((stage, p), roi) in self.stages.iter().zip(&rois[1..]) {
            let (next, n) = run_stage_cpu(*stage, ctx, &tile, *roi, p)?;
            tile = next;
            replaced += n;
        }
        Ok((tile, replaced))
    }

    /// 出力全体を計算する（タイルに分けない基準の実行）。
    pub fn render_full(
        &self,
        ctx: &StageContext<'_>,
        source: &ImageTile,
    ) -> Result<(ImageTile, u64)> {
        let (w, h) = self.output_size();
        self.render_roi(ctx, source, Roi::full(w, h))
    }
}

#[cfg(test)]
mod tests {
    use genzo_model::Phase;

    use super::*;

    #[test]
    fn luminance_coefficients_sum_to_one() {
        let s: f32 = BT2020_LUMINANCE.iter().sum();
        assert!((s - 1.0).abs() < 1e-6);
        assert!((BT2020_LUMINANCE[0] - 0.2627).abs() < 1e-3);
        assert!((BT2020_LUMINANCE[1] - 0.6780).abs() < 1e-3);
        assert!((LOG2_MIDDLE_GRAY - 0.18f32.log2()).abs() < 1e-6);
        assert!((luminance([0.18; 3]) - 0.18).abs() < 1e-7);
    }

    #[test]
    fn scene_stages_are_ordered_and_in_phase_c() {
        let ids: Vec<_> = scene_stages().iter().map(|s| s.id()).collect();
        assert_eq!(
            ids,
            [
                "finish.geometry",
                "finish.exposure",
                "finish.contrast",
                "finish.tone",
                "finish.texture",
                "finish.color",
                "finish.sharpen",
                "finish.scene_to_display",
                "finish.tone_curve",
            ]
        );
        for s in scene_stages() {
            assert_eq!(s.phase(), Phase::C);
        }
        // 契約がつながっている。
        let st = scene_stages();
        for w in st.windows(2) {
            assert_eq!(
                w[0].output_contract(),
                w[1].input_contract(),
                "{}",
                w[1].id()
            );
        }
    }
}
