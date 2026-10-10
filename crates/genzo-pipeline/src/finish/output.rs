//! ステージ 17: 出力（docs/04_architecture.md の 2.1 節・2.6 節、5 章。DEV-26）。
//!
//! | ステージ | 型 | 入力 → 出力 | 内容 |
//! |---|---|---|---|
//! | 17a 画面 | [`DisplayStage`] | B3 → B4a | モニターの色域へ色域の圧縮 → IEC 61966-2-1 で符号化 → 3D LUT（四面体補間） |
//! | 17b 書き出し | [`ExportStage`] | B3 → B4b | 出力の色空間への行列 → 色域の圧縮 → 伝達関数で符号化（0〜1 の f32） |
//! | 17b のキャッシュ | [`ExportStage::CACHE_DISPLAY_P3`] | B3 → B5 | 17b の Display P3 と同じ計算で、契約を B5 にしたもの（4 章） |
//! | 17b の量子化 | [`quantize_u8`]・[`quantize_u16`] | B4b / B5 → 8bit / 16bit | 決定的なディザリング（[`Dither`]）と量子化 |
//! | 17c ヒストグラム | [`Histogram::compute`] | B4a / B4b / B5 → 度数 | R・G・B・輝度の 256 階級と、白飛び・黒つぶれの画素数 |
//!
//! 色域の圧縮は genzo-color の [`GamutCompressor`]・[`DisplayGamut`]（仮実装。方式は PoC-4）。
//! 3D LUT は genzo-color の [`Lut3d`]（lcms2 で作り、四面体補間を自前で適用）。
//!
//! CPU 版と GPU 版の比較（IQ-07a）は、量子化の前の f32 のバッファ（B4a / B4b）で行う。量子化と
//! ディザリングは式が決まっている（整数の演算）ので、どちらで行っても同じ結果になる。

use std::sync::{Arc, OnceLock};

use genzo_color::space::working_to;
use genzo_color::transfer::srgb_encode_f32;
use genzo_color::{
    DEFAULT_LUT_SIZE, DisplayGamut, DisplayLutSource, DisplayProfile, DisplayProfileFallbackReason,
    GamutCompressor, Lut3d, Mat3F32, RgbColorSpace, TransferFunction,
};
use genzo_model::{DevelopSettings, OutputColorSpace, Phase};
use rayon::prelude::*;

use super::map_pixels;
use crate::contract::ColorContract;
use crate::error::{PipelineError, Result};
use crate::image::{ImageTile, RgbImage, Roi};
use crate::stage::{Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// 画面用の 3D LUT の格子数（genzo-color の既定 33³。2.6 節）。
pub const DISPLAY_LUT_SIZE: usize = DEFAULT_LUT_SIZE;

/// 書き出しの色空間に対応する genzo-color の色空間。
pub const fn output_space(space: OutputColorSpace) -> RgbColorSpace {
    match space {
        OutputColorSpace::Srgb => RgbColorSpace::Srgb,
        OutputColorSpace::DisplayP3 => RgbColorSpace::DisplayP3,
        OutputColorSpace::AdobeRgb => RgbColorSpace::AdobeRgb1998,
    }
}

// ---------------------------------------------------------------------------
// 17a 画面
// ---------------------------------------------------------------------------

/// 画面への出力の変換（ステージ 17a のパラメータ）。
///
/// 1. `gamut.compress_working`（B3 → モニターの色域に収めた BT.2020 のリニアな値）
/// 2. `srgb_encode_f32`（各チャンネル。IEC 61966-2-1）
/// 3. `lut.apply`（BT.2020・IEC 61966-2-1 → モニターの ICC。入力は 0〜1 に収めてから引く）
/// 4. 結果を 0〜1 に収める（`clamp`。2.6 節の B4a は 0〜1）
///
/// 手順 4 の理由: LUT の格子点のうちモニターの色域の外の点は、lcms2 の浮動小数点の変換で 0〜1 の外の
/// 値を持つ。1. で色域内に収めた色でも、そうした格子点との四面体補間で結果がわずかに外れる
/// （sRGB とみなしたモニターで、B3 の輝度 0〜1 の範囲で最小 −0.016 程度。シアンの境界の近くの
/// 鮮やかな色）。色域外の色の処理（1.）とは別の、補間の誤差の吸収なので、単純な切り捨て
/// （2.6 節で避けるもの）には当たらない。GPU 版も同じ順序で収めること。
#[derive(Debug, Clone)]
pub struct DisplayTransform {
    /// モニターの色域への色域の圧縮。
    pub gamut: DisplayGamut,
    /// 3D LUT。
    pub lut: Arc<Lut3d>,
    /// sRGB とみなしたか（UI に表示する。IQ-05）。
    pub assumed_srgb: bool,
}

impl DisplayTransform {
    /// モニターのプロファイルから作る。プロファイルの原色を行列で取り出せない（LUT 型など）場合の
    /// 色域は、genzo-color の doc の暫定どおり sRGB にする（PoC-1 で決める）。
    pub fn new(display: &DisplayProfile, lut_size: usize) -> Result<Self> {
        let gamut = display
            .gamut()
            .unwrap_or_else(|| DisplayGamut::for_space(RgbColorSpace::Srgb));
        let lut = display.build_lut(DisplayLutSource::Working, lut_size)?;
        Ok(Self {
            gamut,
            lut: Arc::new(lut),
            assumed_srgb: display.is_assumed_srgb(),
        })
    }

    /// モニターのプロファイルが取得できないとき（sRGB とみなす）の変換。作るのに lcms2 を使うので、
    /// 1 回だけ作って使い回す。
    pub fn assumed_srgb() -> Result<Arc<Self>> {
        static CELL: OnceLock<std::result::Result<Arc<DisplayTransform>, String>> = OnceLock::new();
        CELL.get_or_init(|| {
            DisplayProfile::assumed_srgb(DisplayProfileFallbackReason::NotAvailable)
                .map_err(PipelineError::from)
                .and_then(|p| Self::new(&p, DISPLAY_LUT_SIZE))
                .map(Arc::new)
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(PipelineError::InvalidArgument)
    }

    /// 1 画素に適用する。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let g = self.gamut.compress_working(c);
        self.lut
            .apply(g.map(srgb_encode_f32))
            .map(|v| v.clamp(0.0, 1.0))
    }
}

/// ステージ 17a: 画面への出力（B3 → B4a）。
///
/// 変換（モニターのプロファイル）は現像設定ではなく実行環境で決まるので、ステージが持つ。
/// [`DisplayStage::default`] はモニターのプロファイルが取得できない場合（sRGB とみなす）の変換を、
/// 最初に [`params`](Stage::params) を呼んだときに作る（登録表の一致テスト用）。エンジンは
/// [`DisplayStage::new`] でモニターごとに作る。
#[derive(Debug, Clone, Default)]
pub struct DisplayStage {
    transform: Option<Arc<DisplayTransform>>,
}

impl DisplayStage {
    /// ステージの ID。
    pub const ID: &'static str = "output.display";

    /// 変換を指定して作る。
    pub fn new(transform: Arc<DisplayTransform>) -> Self {
        Self {
            transform: Some(transform),
        }
    }

    /// 使う変換。
    pub fn transform(&self) -> Result<Arc<DisplayTransform>> {
        match &self.transform {
            Some(t) => Ok(Arc::clone(t)),
            None => DisplayTransform::assumed_srgb(),
        }
    }
}

impl Stage for DisplayStage {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn phase(&self) -> Phase {
        Phase::C
    }
    fn input_contract(&self) -> ColorContract {
        ColorContract::B3DisplayReferred
    }
    fn output_contract(&self) -> ColorContract {
        ColorContract::B4aDisplay
    }
    fn params(&self, _: &DevelopSettings, ctx: &StageContext<'_>) -> Result<Option<StageParams>> {
        match ctx.process_version {
            ProcessVersion::V1 => Ok(Some(StageParams::new(self.transform()?))),
        }
    }
    fn input_roi(&self, output_roi: Roi, _: &StageParams, _: &StageContext<'_>) -> Roi {
        output_roi
    }
    fn run_cpu(
        &self,
        ctx: &StageContext<'_>,
        input: &ImageTile,
        output: &mut ImageTile,
        params: &StageParams,
    ) -> Result<()> {
        let t = params.expect::<Arc<DisplayTransform>>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 => map_pixels(input, output, |_, _, c| t.apply(c)),
        }
    }
}

// ---------------------------------------------------------------------------
// 17b 書き出し
// ---------------------------------------------------------------------------

/// 書き出しの変換（ステージ 17b のパラメータ）。
///
/// 1. `matrix.apply`（BT.2020 → 出力の色空間のリニアな値。genzo-color の `working_to` を f32 に丸めた行列）
/// 2. `compressor.compress`（色域の圧縮。出力は 0〜1）
/// 3. `transfer.encode_f32`（IEC 61966-2-1、または Adobe RGB のガンマ 563/256）
/// 4. 丸めの誤差を 0〜1 に収める（`clamp`。2. で範囲内なので、切り捨ての処理ではない）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportTransform {
    /// 出力の色空間。
    pub space: RgbColorSpace,
    /// BT.2020 → 出力の色空間のリニア RGB。
    pub matrix: Mat3F32,
    /// 色域の圧縮。
    pub compressor: GamutCompressor,
    /// 伝達関数。
    pub transfer: TransferFunction,
}

impl ExportTransform {
    /// 出力の色空間から作る。
    pub fn new(space: RgbColorSpace) -> Self {
        Self {
            space,
            matrix: working_to(space).to_f32(),
            compressor: GamutCompressor::for_space(space),
            transfer: space.transfer_function(),
        }
    }

    /// 1 画素に適用する（0〜1 の符号化した値）。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let l = self.compressor.compress(self.matrix.apply(c));
        l.map(|v| self.transfer.encode_f32(v).clamp(0.0, 1.0))
    }
}

/// 17b の出力先。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportTarget {
    /// 書き出しのファイル（B4b）。
    File(OutputColorSpace),
    /// キャッシュ L0 / L1（B5。Display P3・IEC 61966-2-1）。
    CacheDisplayP3,
}

/// ステージ 17b: 書き出し（B3 → B4b、またはキャッシュの B5）。量子化は [`quantize_u8`]・
/// [`quantize_u16`] で別に行う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportStage {
    target: ExportTarget,
}

impl ExportStage {
    /// sRGB への書き出し。
    pub const SRGB: ExportStage = ExportStage {
        target: ExportTarget::File(OutputColorSpace::Srgb),
    };
    /// Display P3 への書き出し。
    pub const DISPLAY_P3: ExportStage = ExportStage {
        target: ExportTarget::File(OutputColorSpace::DisplayP3),
    };
    /// Adobe RGB (1998) への書き出し。
    pub const ADOBE_RGB: ExportStage = ExportStage {
        target: ExportTarget::File(OutputColorSpace::AdobeRgb),
    };
    /// キャッシュ（B5）。
    pub const CACHE_DISPLAY_P3: ExportStage = ExportStage {
        target: ExportTarget::CacheDisplayP3,
    };
    /// すべて（登録表の順）。
    pub const ALL: [ExportStage; 4] = [
        Self::SRGB,
        Self::DISPLAY_P3,
        Self::ADOBE_RGB,
        Self::CACHE_DISPLAY_P3,
    ];

    /// 書き出しの色空間のステージ。
    pub const fn for_space(space: OutputColorSpace) -> &'static ExportStage {
        match space {
            OutputColorSpace::Srgb => &Self::SRGB,
            OutputColorSpace::DisplayP3 => &Self::DISPLAY_P3,
            OutputColorSpace::AdobeRgb => &Self::ADOBE_RGB,
        }
    }

    /// 出力先。
    pub const fn target(&self) -> ExportTarget {
        self.target
    }

    /// 出力の色空間。
    pub const fn space(&self) -> RgbColorSpace {
        match self.target {
            ExportTarget::File(s) => output_space(s),
            ExportTarget::CacheDisplayP3 => RgbColorSpace::DisplayP3,
        }
    }
}

impl Stage for ExportStage {
    fn id(&self) -> &'static str {
        match self.target {
            ExportTarget::File(OutputColorSpace::Srgb) => "output.export.srgb",
            ExportTarget::File(OutputColorSpace::DisplayP3) => "output.export.display_p3",
            ExportTarget::File(OutputColorSpace::AdobeRgb) => "output.export.adobe_rgb",
            ExportTarget::CacheDisplayP3 => "output.cache.display_p3",
        }
    }
    fn phase(&self) -> Phase {
        Phase::C
    }
    fn input_contract(&self) -> ColorContract {
        ColorContract::B3DisplayReferred
    }
    fn output_contract(&self) -> ColorContract {
        match self.target {
            ExportTarget::File(_) => ColorContract::B4bExport,
            ExportTarget::CacheDisplayP3 => ColorContract::B5Cache,
        }
    }
    fn params(&self, _: &DevelopSettings, ctx: &StageContext<'_>) -> Result<Option<StageParams>> {
        match ctx.process_version {
            ProcessVersion::V1 => Ok(Some(StageParams::new(ExportTransform::new(self.space())))),
        }
    }
    fn input_roi(&self, output_roi: Roi, _: &StageParams, _: &StageContext<'_>) -> Roi {
        output_roi
    }
    fn run_cpu(
        &self,
        ctx: &StageContext<'_>,
        input: &ImageTile,
        output: &mut ImageTile,
        params: &StageParams,
    ) -> Result<()> {
        let t = *params.expect::<ExportTransform>(self.id())?;
        match ctx.process_version {
            ProcessVersion::V1 => map_pixels(input, output, |_, _, c| t.apply(c)),
        }
    }
}

// ---------------------------------------------------------------------------
// 17b 量子化とディザリング
// ---------------------------------------------------------------------------

/// 量子化のディザリング（IQ-08 のため、乱数の状態に依存しない決定的なもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dither {
    /// ディザリングしない（四捨五入）。
    #[default]
    None,
    /// 三角分布（TPDF、振幅 ±1 LSB）のノイズ。ノイズは画素の座標・チャンネル・種（`seed`）の
    /// ハッシュ（[`dither_noise`]）から作る。
    Triangular {
        /// 種（画像ごとに変えたい場合に使う。既定は 0）。
        seed: u32,
    },
}

impl Dither {
    /// 8bit の書き出しの既定（種 0 の三角分布）。
    pub const DEFAULT_8BIT: Dither = Dither::Triangular { seed: 0 };
}

/// 32bit の整数のハッシュ（lowbias32: Chris Wellons の公表した定数。乗算はラップする）。
#[inline]
pub const fn hash32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// 画素 (x, y)（出力の画像全体での座標）・チャンネル `c` の三角分布のノイズ（−1 より大きく 1 より小さい）。
///
/// ```text
/// h  = hash32(x ⊕ hash32(y ⊕ hash32(c ⊕ seed)))       （x, y は u32 にラップした値）
/// u1 = (hash32(h) >> 8) × 2^−24、 u2 = (hash32(h ⊕ 0x9e3779b9) >> 8) × 2^−24   （0 以上 1 未満）
/// n  = u1 + u2 − 1
/// ```
#[inline]
pub fn dither_noise(x: i64, y: i64, c: u32, seed: u32) -> f32 {
    let h = hash32((x as u32) ^ hash32((y as u32) ^ hash32(c ^ seed)));
    const SCALE: f32 = 1.0 / (1u32 << 24) as f32;
    let u1 = (hash32(h) >> 8) as f32 * SCALE;
    let u2 = (hash32(h ^ 0x9e37_79b9) >> 8) as f32 * SCALE;
    u1 + u2 - 1.0
}

/// 0〜1 の値 `v` を `0..=max` の整数にする。
///
/// `q = floor(v × max + 0.5 + n)`（n はディザリングのノイズ。なしなら 0）を `0..=max` に収める。
/// NaN は 0。`v ≤ 0` と `v ≥ 1`（黒と白）にはノイズを加えない（純粋な黒・白を保つ）。
#[inline]
pub fn quantize_value(v: f32, max: f32, noise: f32) -> u32 {
    if v.is_nan() || v <= 0.0 {
        return 0;
    }
    if v >= 1.0 {
        return max as u32;
    }
    (v * max + 0.5 + noise).floor().clamp(0.0, max) as u32
}

fn check_encoded(image: &RgbImage) -> Result<()> {
    if image.contract().is_linear() {
        return Err(PipelineError::InvalidArgument(format!(
            "量子化は符号化した値（B4a・B4b・B5）にだけ使えます（{}）",
            image.contract()
        )));
    }
    Ok(())
}

fn quantize<T: Send + Copy + Default>(
    image: &RgbImage,
    origin: (i64, i64),
    dither: Dither,
    max: f32,
    cast: impl Fn(u32) -> T + Sync,
) -> Result<Vec<T>> {
    check_encoded(image)?;
    let w = image.width() as usize;
    let mut out = vec![T::default(); image.data().len() * 3];
    out.par_chunks_mut(w * 3)
        .zip(image.data().par_chunks(w))
        .enumerate()
        .for_each(|(j, (dst, src))| {
            let y = origin.1 + j as i64;
            for (i, (d, p)) in dst.chunks_mut(3).zip(src).enumerate() {
                let x = origin.0 + i as i64;
                for c in 0..3 {
                    let n = match dither {
                        Dither::None => 0.0,
                        Dither::Triangular { seed } => dither_noise(x, y, c as u32, seed),
                    };
                    d[c] = cast(quantize_value(p[c], max, n));
                }
            }
        });
    Ok(out)
}

/// 8bit に量子化する（R, G, B, R, G, B, … の並び）。`origin` はこの画像の左上の、出力の画像全体での
/// 座標（タイルに分けてもディザリングのノイズが同じになるように）。
pub fn quantize_u8(image: &RgbImage, origin: (i64, i64), dither: Dither) -> Result<Vec<u8>> {
    quantize(image, origin, dither, 255.0, |q| q as u8)
}

/// 16bit に量子化する（並びは [`quantize_u8`] と同じ）。16bit では通常ディザリングしない
/// （[`Dither::None`]）。
pub fn quantize_u16(image: &RgbImage, origin: (i64, i64), dither: Dither) -> Result<Vec<u16>> {
    quantize(image, origin, dither, 65535.0, |q| q as u16)
}

// ---------------------------------------------------------------------------
// 17c ヒストグラム
// ---------------------------------------------------------------------------

/// 白飛び・黒つぶれの画素数（DEV-26 のクリッピングの警告用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClipCounts {
    /// R がその端の階級にある画素の数。
    pub red: u64,
    /// G。
    pub green: u64,
    /// B。
    pub blue: u64,
    /// どれか 1 つ以上のチャンネルが端の階級にある画素の数。
    pub any: u64,
    /// 3 つのチャンネルがすべて端の階級にある画素の数。
    pub all: u64,
}

impl ClipCounts {
    fn add(&mut self, other: &ClipCounts) {
        self.red += other.red;
        self.green += other.green;
        self.blue += other.blue;
        self.any += other.any;
        self.all += other.all;
    }
}

/// ヒストグラム（ステージ 17c。DEV-26）。
///
/// - 階級は符号化した値の 8bit の値: `bin(v) = clamp(round(v × 255), 0, 255)`（NaN は 0）。
/// - 輝度は符号化した値の重み付きの和 `w0·R' + w1·G' + w2·B'`（ルマ。係数は出力の色空間の輝度の
///   係数。[`Histogram::luma_coefficients`]）。
/// - 白飛びは階級 255、黒つぶれは階級 0 のチャンネルを数える（[`ClipCounts`]）。
/// - 度数は整数の和なので、分割や並列化の順序によらず同じ結果になる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    /// R の度数。
    pub red: [u64; 256],
    /// G の度数。
    pub green: [u64; 256],
    /// B の度数。
    pub blue: [u64; 256],
    /// 輝度（ルマ）の度数。
    pub luma: [u64; 256],
    /// 画素数。
    pub pixel_count: u64,
    /// 白飛び（階級 255）。
    pub clipped_high: ClipCounts,
    /// 黒つぶれ（階級 0）。
    pub clipped_low: ClipCounts,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            red: [0; 256],
            green: [0; 256],
            blue: [0; 256],
            luma: [0; 256],
            pixel_count: 0,
            clipped_high: ClipCounts::default(),
            clipped_low: ClipCounts::default(),
        }
    }
}

/// 符号化した値の階級。
#[inline]
pub fn histogram_bin(v: f32) -> usize {
    if v.is_nan() {
        return 0;
    }
    (v * 255.0).round().clamp(0.0, 255.0) as usize
}

impl Histogram {
    /// 色空間の輝度の係数（f32。和は約 1）。画面（B4a）でモニターの原色がわからない場合は
    /// sRGB の係数を使う。
    pub fn luma_coefficients(space: RgbColorSpace) -> [f32; 3] {
        space.luminance_coefficients().map(|w| w as f32)
    }

    /// 符号化した画像（B4a・B4b・B5）のヒストグラム。`luma` は輝度の係数。
    pub fn compute(image: &RgbImage, luma: [f32; 3]) -> Result<Histogram> {
        check_encoded(image)?;
        // 行ごとに 1 つの度数の表（約 8KB）を作って集めると、プレビュー（1707 行）で 1 コマごとに
        // 約 14MB を確保することになる。rayon の分割ごとに足し込み（fold）、分割どうしを足す（reduce）。
        // 度数は整数の和なので、分割の仕方・スレッド数によらず結果は同じ。
        // 途中の値はヒープに置く（Box）。度数の表（約 8KB）を値のまま fold / reduce で渡すと、rayon の
        // 分割の再帰の各段にその複製が積まれ、最適化のない debug ビルドでは 2MiB のスタック（rayon の
        // ワーカーとテストのスレッドの既定）を超えていた（genzo-gpu の報告。統合の確認で再現）。
        let w = image.width() as usize;
        let total = image
            .data()
            .par_chunks(w)
            .fold(
                || Box::new(Histogram::default()),
                |mut h, row| {
                    for p in row {
                        h.add_pixel(*p, luma);
                    }
                    h
                },
            )
            .reduce(
                || Box::new(Histogram::default()),
                |mut a, b| {
                    a.merge(&b);
                    a
                },
            );
        Ok(*total)
    }

    fn add_pixel(&mut self, p: [f32; 3], luma: [f32; 3]) {
        let b = [
            histogram_bin(p[0]),
            histogram_bin(p[1]),
            histogram_bin(p[2]),
        ];
        self.red[b[0]] += 1;
        self.green[b[1]] += 1;
        self.blue[b[2]] += 1;
        let y = luma[0] * p[0] + luma[1] * p[1] + luma[2] * p[2];
        self.luma[histogram_bin(y)] += 1;
        self.pixel_count += 1;
        for (edge, counts) in [
            (255usize, &mut self.clipped_high),
            (0, &mut self.clipped_low),
        ] {
            let hit = b.map(|v| v == edge);
            counts.red += u64::from(hit[0]);
            counts.green += u64::from(hit[1]);
            counts.blue += u64::from(hit[2]);
            counts.any += u64::from(hit.iter().any(|&h| h));
            counts.all += u64::from(hit.iter().all(|&h| h));
        }
    }

    /// 別のヒストグラムを足す（タイルごとに求めたものを合わせる）。
    pub fn merge(&mut self, other: &Histogram) {
        for (a, b) in [
            (&mut self.red, &other.red),
            (&mut self.green, &other.green),
            (&mut self.blue, &other.blue),
            (&mut self.luma, &other.luma),
        ] {
            for (x, y) in a.iter_mut().zip(b) {
                *x += *y;
            }
        }
        self.pixel_count += other.pixel_count;
        self.clipped_high.add(&other.clipped_high);
        self.clipped_low.add(&other.clipped_low);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_of_middle_gray_is_118_in_srgb() {
        let t = ExportTransform::new(RgbColorSpace::Srgb);
        let e = t.apply([0.18; 3]);
        assert!(e.iter().all(|v| (v - 0.461_356).abs() < 1e-4), "{e:?}");
        let img = RgbImage::filled(4, 4, ColorContract::B4bExport, e).unwrap();
        let q = quantize_u8(&img, (0, 0), Dither::None).unwrap();
        assert!(q.iter().all(|&v| v == 118), "{q:?}");
        // 白と黒。
        assert_eq!(t.apply([1.0; 3]).map(|v| (v * 255.0).round()), [255.0; 3]);
        assert_eq!(t.apply([0.0; 3]), [0.0; 3]);
    }

    #[test]
    fn display_output_is_within_zero_and_one() {
        // 2.6 節の B4a は 0〜1。LUT の補間でわずかに外れる鮮やかな色（例: B3 の (−0.1, 0.74, 1.01)。
        // 修正前は R が約 −0.016）も含めて、B3 の範囲（負の値・1 を超えるチャンネルを含む）を
        // 格子で調べる。
        let t = DisplayTransform::assumed_srgb().unwrap();
        let n = 40;
        let mut checked = 0;
        for r in 0..=n {
            for g in 0..=n {
                for b in 0..=n {
                    let c = [r, g, b].map(|v| v as f32 / n as f32 * 1.2 - 0.1);
                    let y = super::super::luminance(c);
                    if !(0.0..=1.0).contains(&y) {
                        continue;
                    }
                    let o = t.apply(c);
                    assert!(o.iter().all(|v| (0.0..=1.0).contains(v)), "{c:?} → {o:?}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 10_000, "{checked}");
        let o = t.apply([-0.1, 0.74, 1.01]);
        assert!(o.iter().all(|v| (0.0..=1.0).contains(v)), "{o:?}");
        // 色域の内側の色は変えない（白・黒・中間のグレーはそのまま）。
        let gray = t.apply([0.18; 3]);
        assert!(
            gray.iter().all(|v| (v - 0.461_356).abs() < 3e-3),
            "{gray:?}"
        );
    }

    #[test]
    fn export_output_is_within_zero_and_one() {
        for space in [
            RgbColorSpace::Srgb,
            RgbColorSpace::DisplayP3,
            RgbColorSpace::AdobeRgb1998,
        ] {
            let t = ExportTransform::new(space);
            let vals = [-1.0f32, -0.01, 0.0, 0.2, 0.7, 1.0, 1.5];
            for &r in &vals {
                for &g in &vals {
                    for &b in &vals {
                        let e = t.apply([r, g, b]);
                        assert!(e.iter().all(|v| (0.0..=1.0).contains(v)), "{space:?} {e:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn dither_is_deterministic_and_triangular() {
        assert_eq!(dither_noise(10, 20, 1, 0), dither_noise(10, 20, 1, 0));
        assert_ne!(dither_noise(10, 20, 1, 0), dither_noise(11, 20, 1, 0));
        assert_ne!(dither_noise(10, 20, 1, 0), dither_noise(10, 20, 1, 7));
        // 範囲と平均・分散（三角分布の分散は 1/6）。
        let n = 200_000;
        let (mut sum, mut sq) = (0.0f64, 0.0f64);
        for k in 0..n {
            let v = dither_noise(k % 1000, k / 1000, 0, 3);
            assert!(v > -1.0 && v < 1.0);
            sum += f64::from(v);
            sq += f64::from(v) * f64::from(v);
        }
        let mean = sum / n as f64;
        let var = sq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.01, "{mean}");
        assert!((var - 1.0 / 6.0).abs() < 0.01, "{var}");
        // 既知の値（ハッシュの式を変えたら気づくように）。
        assert_eq!(hash32(0), 0);
        assert_eq!(hash32(1), hash32(1));
    }

    #[test]
    fn dithered_quantization_keeps_the_mean_and_extremes() {
        let e = srgb_encode_f32(0.18);
        let img = RgbImage::filled(64, 64, ColorContract::B4bExport, [e, 0.0, 1.0]).unwrap();
        let a = quantize_u8(&img, (0, 0), Dither::DEFAULT_8BIT).unwrap();
        let b = quantize_u8(&img, (0, 0), Dither::DEFAULT_8BIT).unwrap();
        assert_eq!(a, b, "同じ入力なら同じ結果");
        let r: Vec<f64> = a.chunks(3).map(|p| f64::from(p[0])).collect();
        let mean = r.iter().sum::<f64>() / r.len() as f64;
        assert!((mean - f64::from(e) * 255.0).abs() < 0.1, "{mean}");
        assert!(r.iter().all(|&v| (117.0..=119.0).contains(&v)));
        // 純粋な黒と白は変えない。
        assert!(a.chunks(3).all(|p| p[1] == 0 && p[2] == 255));
        // タイルに分けても同じ（座標は全体での座標）。
        let left = RgbImage::filled(32, 64, ColorContract::B4bExport, [e, 0.0, 1.0]).unwrap();
        let qa = quantize_u8(&left, (32, 0), Dither::DEFAULT_8BIT).unwrap();
        for y in 0..64usize {
            for x in 0..32usize {
                assert_eq!(qa[(y * 32 + x) * 3], a[(y * 64 + 32 + x) * 3]);
            }
        }
        // 16bit。
        let q16 = quantize_u16(&img, (0, 0), Dither::None).unwrap();
        assert_eq!(q16[0], (e * 65535.0).round() as u16);
        assert_eq!(q16[2], 65535);
        // リニアな値は量子化しない。
        let lin = RgbImage::filled(2, 2, ColorContract::B3DisplayReferred, [0.5; 3]).unwrap();
        assert!(quantize_u8(&lin, (0, 0), Dither::None).is_err());
        assert_eq!(quantize_value(f32::NAN, 255.0, 0.5), 0);
        assert_eq!(quantize_value(0.5, 255.0, 0.0), 128);
    }

    #[test]
    fn histogram_counts_every_pixel() {
        let img = RgbImage::from_fn(17, 9, ColorContract::B4bExport, |x, y| {
            [
                x as f32 / 16.0,
                y as f32 / 8.0,
                if x == 0 { 1.0 } else { 0.0 },
            ]
        })
        .unwrap();
        let h =
            Histogram::compute(&img, Histogram::luma_coefficients(RgbColorSpace::Srgb)).unwrap();
        assert_eq!(h.pixel_count, 17 * 9);
        for ch in [&h.red, &h.green, &h.blue, &h.luma] {
            assert_eq!(ch.iter().sum::<u64>(), 17 * 9);
        }
        // R の 0 の階級は x = 0 の列（9 画素）、255 は x = 16 の列。
        assert_eq!(h.red[0], 9);
        assert_eq!(h.red[255], 9);
        assert_eq!(h.clipped_high.red, 9);
        assert_eq!(h.clipped_high.blue, 9);
        // 白飛び（どれか）: R が 255 の列（9）と B が 255 の列（9）と G が 255 の行（17）の和集合。
        assert_eq!(h.clipped_high.any, 9 + 9 + 17 - 2);
        // すべてのチャンネルが 255 の画素はない。
        assert_eq!(h.clipped_high.all, 0);
        // 黒つぶれ: (0, 0) は R・G が 0 だが B が 1。
        assert_eq!(h.clipped_low.all, 0);
        assert!(h.clipped_low.any > 0);
        // 分けて足しても同じ。
        let mut m = Histogram::compute(
            &img.crop(Roi::new(0, 0, 17, 4)).unwrap(),
            [0.2126, 0.7152, 0.0722],
        )
        .unwrap();
        m.merge(
            &Histogram::compute(
                &img.crop(Roi::new(0, 4, 17, 5)).unwrap(),
                [0.2126, 0.7152, 0.0722],
            )
            .unwrap(),
        );
        assert_eq!(m.red, h.red);
        assert_eq!(m.pixel_count, h.pixel_count);
        let lin = RgbImage::filled(2, 2, ColorContract::B2Working, [0.5; 3]).unwrap();
        assert!(Histogram::compute(&lin, [0.3, 0.6, 0.1]).is_err());
        assert_eq!(histogram_bin(f32::NAN), 0);
        assert_eq!(histogram_bin(-1.0), 0);
        assert_eq!(histogram_bin(2.0), 255);
    }

    #[test]
    fn histogram_does_not_depend_on_the_number_of_threads() {
        // 分割ごとの足し込み（fold / reduce）でも、スレッドの数によらず 1 画素ずつ数えた結果と同じ。
        let img = RgbImage::from_fn(61, 47, ColorContract::B5Cache, |x, y| {
            let h = hash32(x * 131 + y * 7919);
            [
                (h & 0xff) as f32 / 255.0,
                ((h >> 8) & 0xff) as f32 / 255.0,
                ((h >> 16) & 0x1ff) as f32 / 255.0 - 0.5,
            ]
        })
        .unwrap();
        let luma = Histogram::luma_coefficients(RgbColorSpace::DisplayP3);
        let mut serial = Histogram::default();
        for p in img.data() {
            serial.add_pixel(*p, luma);
        }
        for threads in [1usize, 3, 4] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let h = pool.install(|| Histogram::compute(&img, luma).unwrap());
            assert_eq!(h, serial, "{threads} スレッド");
        }
    }

    #[test]
    fn histogram_fits_in_a_small_stack() {
        // 度数の表を値のまま fold / reduce で渡していたときは、debug ビルドで 4 スレッド・123 × 77 の画像に
        // 2MiB を超えるスタックが必要だった（溢れるとプロセスごと異常終了する）。Box に置いた後は 256KiB
        // でも足りることを確かめたので、余裕を持たせて 1MiB のスレッドと rayon のワーカーで計算する。
        const STACK: usize = 1 << 20;
        let img = RgbImage::from_fn(123, 77, ColorContract::B4bExport, |x, y| {
            [(x % 256) as f32 / 255.0, (y % 256) as f32 / 255.0, 0.5]
        })
        .unwrap();
        let luma = Histogram::luma_coefficients(RgbColorSpace::Srgb);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .stack_size(STACK)
            .build()
            .unwrap();
        let h = std::thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || pool.install(|| Histogram::compute(&img, luma).unwrap()))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(h.pixel_count, 123 * 77);
    }
}
