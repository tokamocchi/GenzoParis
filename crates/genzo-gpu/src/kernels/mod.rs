//! WGSL のカーネル（シェーダー）の組み立て・コンパイル・実行（docs/04_architecture.md の 2.3 節）。
//!
//! # 構成
//!
//! - シェーダーのモジュール（[`Module`]）は、共通の部分（`shaders/common.wgsl` など）とステージの部分を
//!   つないだもの。先頭に Rust 側が生成した定数を付ける（[`generated_header`]）:
//!   - パラメータの位置（`P_*`）。パラメータは u32 の配列で渡し（[`Params`]）、f32・i32 はビット列のまま
//!     入れる。uniform の配置の規則（16 バイトの整列など）に左右されない。
//!   - CPU 版と同じ値の f32 の定数（BT.2020 の輝度の係数、IEC 61966-2-1 の定数、OKLab の行列、トーンの
//!     定数など）。**16 進の浮動小数点の表記**（`0x1.0d01c6p-2f`）で書くので、10 進への丸めの差が出ず、
//!     CPU 版の値とビット単位で同じになる。
//! - バインディングはすべてのカーネルで同じ配置（0: 入力、1: 出力、2: パラメータ、3: 原子的な
//!   カウンター（`[0]` は置き換えた NaN・無限大の数）、4: 補助の入力（LUT・ガイドなど）、5: 補助の出力）。
//!   使わないものは小さな空のバッファを結び付ける。
//! - パイプラインは最初に使うときにコンパイルして、[`KernelCache`] に保持する。
//!
//! # 2.3 節の決めごととの対応
//!
//! - 浮動小数点は f32。式と演算の順序は CPU 版（各ステージの doc）と同じに書く。行列は `m * v` を使わず、
//!   行ごとに左から足す（`row_dot`）。
//! - テクスチャのハードウェア補間は使わない（ストレージバッファと自前の補間）。
//! - WGSL は浮動小数点の演算の結合・融合（積和演算）を許し、`exp2`・`log2`・`pow` などの精度も数 ULP の
//!   幅があるので、完全な一致は保証できない（一致はステージごとの許容誤差で確かめる。[`crate::tolerance`]）。
//!   wgpu の Metal のバックエンドはシェーダーを Metal の既定（高速な数学）でコンパイルし、この crate から
//!   変えられない（M1 の PoC-3 で差を確かめる）。

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use genzo_color::oklab::{
    BT2020_TO_OKLAB_LMS_F32, OKLAB_LMS_TO_BT2020_F32, OKLAB_M2_F32, OKLAB_M2_INV_F32,
};
use genzo_color::transfer::{
    ADOBE_RGB_INV_GAMMA_F32, SRGB_DECODE_THRESHOLD_F32, SRGB_ENCODE_THRESHOLD_F32, SRGB_GAMMA_F32,
    SRGB_INV_GAMMA_F32, SRGB_LINEAR_SLOPE_F32, SRGB_OFFSET_F32,
};
use genzo_pipeline::Roi;
use genzo_pipeline::finish::color::{SKIN_PROTECTION, VIBRANCE_CHROMA_REF, VIBRANCE_MAX};
use genzo_pipeline::finish::scene_to_display::SCENE_TO_DISPLAY_T_MAX;
use genzo_pipeline::finish::tone::{
    BLACKS_CENTER_EV, BLACKS_MAX_EV, BLACKS_WIDTH_EV, HIGHLIGHTS_CENTER_EV, HIGHLIGHTS_MAX_EV,
    HIGHLIGHTS_WIDTH_EV, SHADOWS_CENTER_EV, SHADOWS_MAX_EV, SHADOWS_WIDTH_EV, WHITES_CENTER_EV,
    WHITES_MAX_EV, WHITES_WIDTH_EV,
};
use genzo_pipeline::finish::{BT2020_LUMINANCE, LOG2_CEILING, LOG2_MIDDLE_GRAY};
use genzo_pipeline::sanitize::LOG_FLOOR;

use crate::context::GpuContext;
use crate::error::{GpuError, Result};

const COMMON: &str = include_str!("../shaders/common.wgsl");
const PIXEL_IO: &str = include_str!("../shaders/pixel_io.wgsl");
const COLOR_LIB: &str = include_str!("../shaders/color_lib.wgsl");
const FINISH: &str = include_str!("../shaders/finish.wgsl");
const OUTPUT: &str = include_str!("../shaders/output.wgsl");
const GEOMETRY: &str = include_str!("../shaders/geometry.wgsl");
const HISTOGRAM: &str = include_str!("../shaders/histogram.wgsl");
const QUANTIZE: &str = include_str!("../shaders/quantize.wgsl");
const SENSOR: &str = include_str!("../shaders/sensor.wgsl");
const RESAMPLE: &str = include_str!("../shaders/resample.wgsl");

/// ステージ 15 の `t` の上限（genzo-pipeline の定数をそのまま使う）。
pub(crate) const S2D_T_MAX: f32 = SCENE_TO_DISPLAY_T_MAX;

/// 座標を i32 にするときの切り詰めの範囲（2^30。CPU 版は i64 で 2^40。極端な座標でだけ結果が違う）。
pub(crate) const COORD_LIMIT: f32 = (1u32 << 30) as f32;

/// 立方根のニュートン法の補正をかける範囲（3 乗があふれない・非正規化数にならない範囲）。
const CBRT_NEWTON_RANGE: (f32, f32) = (1e-30, 1e30);

/// Adobe RGB の符号化の指数（genzo-color の定数をそのまま使う）。
pub(crate) const ADOBE_INV_GAMMA: f32 = ADOBE_RGB_INV_GAMMA_F32;

/// ディザリングのノイズの尺度 2^−24（genzo-pipeline の `dither_noise` と同じ）。
pub(crate) const DITHER_SCALE: f32 = 1.0 / (1u32 << 24) as f32;

/// パラメータの先頭の共通の位置（入力・出力のタイルの範囲）。
const HEADER: [&str; 8] = [
    "IN_X", "IN_Y", "IN_W", "IN_H", "OUT_X", "OUT_Y", "OUT_W", "OUT_H",
];

/// シェーダーのモジュール（WGSL のファイルの組み合わせ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Module {
    Finish,
    Output,
    Geometry,
    Histogram,
    Quantize,
    Sensor,
    Resample,
}

impl Module {
    #[cfg(test)]
    pub(crate) const ALL: [Module; 7] = [
        Module::Finish,
        Module::Output,
        Module::Geometry,
        Module::Histogram,
        Module::Quantize,
        Module::Sensor,
        Module::Resample,
    ];

    fn label(self) -> &'static str {
        match self {
            Module::Finish => "genzo-gpu.finish",
            Module::Output => "genzo-gpu.output",
            Module::Geometry => "genzo-gpu.geometry",
            Module::Histogram => "genzo-gpu.histogram",
            Module::Quantize => "genzo-gpu.quantize",
            Module::Sensor => "genzo-gpu.sensor",
            Module::Resample => "genzo-gpu.resample",
        }
    }

    fn parts(self) -> &'static [&'static str] {
        match self {
            Module::Finish => &[COMMON, PIXEL_IO, COLOR_LIB, FINISH],
            Module::Output => &[COMMON, PIXEL_IO, COLOR_LIB, OUTPUT],
            Module::Geometry => &[COMMON, PIXEL_IO, GEOMETRY],
            Module::Histogram => &[COMMON, HISTOGRAM],
            Module::Quantize => &[COMMON, QUANTIZE],
            Module::Sensor => &[COMMON, SENSOR],
            Module::Resample => &[COMMON, PIXEL_IO, RESAMPLE],
        }
    }

    /// モジュールのパラメータの位置（名前と個数。共通の位置の後に、この順に並ぶ）。
    fn slots(self) -> &'static [(&'static str, u32)] {
        match self {
            Module::Finish => &[
                ("GAIN", 1),
                ("GAMMA", 1),
                ("TONE_H", 1),
                ("TONE_S", 1),
                ("TONE_W", 1),
                ("TONE_B", 1),
                ("TONE_EV", 1),
                ("GUIDE_OX", 1),
                ("GUIDE_OY", 1),
                ("GUIDE_SX", 1),
                ("GUIDE_SY", 1),
                ("GUIDE_W", 1),
                ("GUIDE_H", 1),
                ("SAT", 1),
                ("VIB", 1),
                ("SKIN_COS", 1),
                ("SKIN_SIN", 1),
                ("SKIN_COS_W", 1),
                ("NEUTRAL_A", 1),
                ("NEUTRAL_B", 1),
                ("KNEE", 1),
                ("CURVE_FLAGS", 1),
                ("LUT_OFF", 4),
                ("LUT_LEN", 4),
                ("LUT_LO", 4),
                ("LUT_INV", 4),
                ("BLACK_LIFT", 1),
                ("RATIO_MIN", 1),
            ],
            Module::Output => &[
                ("W2D", 9),
                ("D2W", 9),
                ("GC_LUMA", 3),
                ("GC_THR", 1),
                ("GC_POW", 1),
                ("GC_INV_POW", 1),
                ("LUT_N", 1),
                ("MAT", 9),
                ("TRANSFER", 1),
            ],
            Module::Geometry => &[
                ("AFF", 6),
                ("CROP_X", 1),
                ("CROP_Y", 1),
                ("GRID_W", 1),
                ("GRID_H", 1),
            ],
            Module::Histogram => &[("HLUMA", 3)],
            Module::Quantize => &[
                ("QBITS", 1),
                ("QORIGIN_X", 1),
                ("QORIGIN_Y", 1),
                ("QDITHER", 1),
                ("QSEED", 1),
                ("QSTRIDE", 1),
            ],
            Module::Sensor => &[
                ("CFA", 4),
                ("RAW_W", 1),
                ("RAW_H", 1),
                ("BLACK", 4),
                ("INV", 4),
                ("WHITE", 1),
                ("MULT", 4),
                ("CLIP_LEVEL", 1),
                ("CLIP_MODE", 1),
                ("RADIUS", 1),
                ("FLAGS_OUT", 1),
            ],
            Module::Resample => &[("MAT", 9), ("TAP_OFF", 1)],
        }
    }

    /// 名前のパラメータの位置（先頭と個数）。
    fn slot(self, name: &str) -> Option<(u32, u32)> {
        if let Some(i) = HEADER.iter().position(|h| *h == name) {
            return Some((i as u32, 1));
        }
        let mut at = HEADER.len() as u32;
        for (n, count) in self.slots() {
            if *n == name {
                return Some((at, *count));
            }
            at += count;
        }
        None
    }

    /// パラメータの語の数。
    fn param_len(self) -> u32 {
        HEADER.len() as u32 + self.slots().iter().map(|s| s.1).sum::<u32>()
    }

    /// 生成した定数を先頭に付けた WGSL のソース。
    pub(crate) fn source(self) -> String {
        let mut s = generated_header(self);
        for part in self.parts() {
            s.push_str(part);
            s.push('\n');
        }
        s
    }
}

/// カーネル（シェーダーの入口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Kernel {
    Exposure,
    Contrast,
    Tone,
    Color,
    SceneToDisplay,
    ToneCurve,
    Display,
    Export,
    Geometry,
    Histogram,
    Quantize,
    Normalize,
    WhiteBalance,
    Half2x2,
    ColorMatrix,
    DownscaleH,
    DownscaleV,
}

impl Kernel {
    pub(crate) const ALL: [Kernel; 17] = [
        Kernel::Exposure,
        Kernel::Contrast,
        Kernel::Tone,
        Kernel::Color,
        Kernel::SceneToDisplay,
        Kernel::ToneCurve,
        Kernel::Display,
        Kernel::Export,
        Kernel::Geometry,
        Kernel::Histogram,
        Kernel::Quantize,
        Kernel::Normalize,
        Kernel::WhiteBalance,
        Kernel::Half2x2,
        Kernel::ColorMatrix,
        Kernel::DownscaleH,
        Kernel::DownscaleV,
    ];

    pub(crate) fn module(self) -> Module {
        match self {
            Kernel::Exposure
            | Kernel::Contrast
            | Kernel::Tone
            | Kernel::Color
            | Kernel::SceneToDisplay
            | Kernel::ToneCurve => Module::Finish,
            Kernel::Display | Kernel::Export => Module::Output,
            Kernel::Geometry => Module::Geometry,
            Kernel::Histogram => Module::Histogram,
            Kernel::Quantize => Module::Quantize,
            Kernel::Normalize | Kernel::WhiteBalance | Kernel::Half2x2 => Module::Sensor,
            Kernel::ColorMatrix | Kernel::DownscaleH | Kernel::DownscaleV => Module::Resample,
        }
    }

    fn entry_point(self) -> &'static str {
        match self {
            Kernel::Exposure => "exposure_main",
            Kernel::Contrast => "contrast_main",
            Kernel::Tone => "tone_main",
            Kernel::Color => "color_main",
            Kernel::SceneToDisplay => "scene_to_display_main",
            Kernel::ToneCurve => "tone_curve_main",
            Kernel::Display => "display_main",
            Kernel::Export => "export_main",
            Kernel::Geometry | Kernel::Histogram | Kernel::Quantize => "main",
            Kernel::Normalize => "normalize_main",
            Kernel::WhiteBalance => "white_balance_main",
            Kernel::Half2x2 => "half_main",
            Kernel::ColorMatrix => "color_matrix_main",
            Kernel::DownscaleH => "downscale_h_main",
            Kernel::DownscaleV => "downscale_v_main",
        }
    }

    /// 出力の NaN・無限大を置き換えて数えるカーネルか（ステージの出力。2.6 節）。
    pub(crate) fn counts_non_finite(self) -> bool {
        !matches!(
            self,
            Kernel::Histogram | Kernel::Quantize | Kernel::DownscaleH | Kernel::DownscaleV
        )
    }

    /// ワークグループの大きさ（WGSL の `@workgroup_size` と同じ）。
    pub(crate) fn workgroup_size(self) -> (u32, u32) {
        match self {
            Kernel::Histogram => (16, 16),
            Kernel::Quantize => (64, 1),
            _ => (8, 8),
        }
    }

    /// 必ず設定するパラメータ（共通の位置を除く）。設定し忘れを [`Params::finish`] が検出する。
    fn required(self) -> &'static [&'static str] {
        match self {
            Kernel::Exposure => &["GAIN"],
            Kernel::Contrast => &["GAMMA"],
            Kernel::Tone => &[
                "TONE_H", "TONE_S", "TONE_W", "TONE_B", "TONE_EV", "GAMMA", "GUIDE_OX", "GUIDE_OY",
                "GUIDE_SX", "GUIDE_SY", "GUIDE_W", "GUIDE_H",
            ],
            Kernel::Color => &[
                "SAT",
                "VIB",
                "SKIN_COS",
                "SKIN_SIN",
                "SKIN_COS_W",
                "NEUTRAL_A",
                "NEUTRAL_B",
            ],
            Kernel::SceneToDisplay => &["KNEE"],
            Kernel::ToneCurve => &[
                "CURVE_FLAGS",
                "LUT_OFF",
                "LUT_LEN",
                "LUT_LO",
                "LUT_INV",
                "BLACK_LIFT",
                "RATIO_MIN",
            ],
            Kernel::Display => &[
                "W2D",
                "D2W",
                "GC_LUMA",
                "GC_THR",
                "GC_POW",
                "GC_INV_POW",
                "LUT_N",
            ],
            Kernel::Export => &[
                "MAT",
                "GC_LUMA",
                "GC_THR",
                "GC_POW",
                "GC_INV_POW",
                "TRANSFER",
            ],
            Kernel::Geometry => &["AFF", "CROP_X", "CROP_Y", "GRID_W", "GRID_H"],
            Kernel::Histogram => &["HLUMA"],
            Kernel::Quantize => &[
                "QBITS",
                "QORIGIN_X",
                "QORIGIN_Y",
                "QDITHER",
                "QSEED",
                "QSTRIDE",
            ],
            Kernel::Normalize => &["CFA", "RAW_W", "RAW_H", "BLACK", "INV", "WHITE"],
            Kernel::WhiteBalance => &[
                "CFA",
                "MULT",
                "CLIP_LEVEL",
                "CLIP_MODE",
                "RADIUS",
                "FLAGS_OUT",
            ],
            Kernel::Half2x2 => &["CFA"],
            Kernel::ColorMatrix => &["MAT"],
            Kernel::DownscaleH | Kernel::DownscaleV => &["TAP_OFF"],
        }
    }
}

/// f32 を WGSL の 16 進の浮動小数点の表記にする（値をビット単位で保つ）。有限の値であること。
pub(crate) fn wgsl_f32(v: f32) -> String {
    debug_assert!(v.is_finite(), "WGSL の定数は有限の値");
    let bits = v.to_bits();
    let sign = if bits >> 31 == 1 { "-" } else { "" };
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;
    if exp == 0 && mant == 0 {
        return format!("{sign}0.0f");
    }
    if exp == 0 {
        // 非正規化数: 0.m × 2^−126。
        return format!("{sign}0x0.{:06x}p-126f", mant << 1);
    }
    format!("{sign}0x1.{:06x}p{}f", mant << 1, exp - 127)
}

/// 行列の行を WGSL の `vec3<f32>` の定数にする。
fn wgsl_rows(out: &mut String, name: &str, rows: [[f32; 3]; 3]) {
    for (i, r) in rows.iter().enumerate() {
        out.push_str(&format!(
            "const {name}_R{i}: vec3<f32> = vec3<f32>({}, {}, {});\n",
            wgsl_f32(r[0]),
            wgsl_f32(r[1]),
            wgsl_f32(r[2])
        ));
    }
}

/// CPU 版と同じ値の f32 の定数（名前と値）。
pub(crate) fn shared_constants() -> Vec<(&'static str, f32)> {
    vec![
        ("LUMA_R", BT2020_LUMINANCE[0]),
        ("LUMA_G", BT2020_LUMINANCE[1]),
        ("LUMA_B", BT2020_LUMINANCE[2]),
        ("LOG_FLOOR", LOG_FLOOR),
        ("LOG2_MIDDLE_GRAY", LOG2_MIDDLE_GRAY),
        ("LOG2_CEILING", LOG2_CEILING),
        ("SRGB_DECODE_THRESHOLD", SRGB_DECODE_THRESHOLD_F32),
        ("SRGB_ENCODE_THRESHOLD", SRGB_ENCODE_THRESHOLD_F32),
        ("SRGB_LINEAR_SLOPE", SRGB_LINEAR_SLOPE_F32),
        ("SRGB_OFFSET", SRGB_OFFSET_F32),
        ("SRGB_ONE_PLUS_OFFSET", 1.0f32 + SRGB_OFFSET_F32),
        ("SRGB_GAMMA", SRGB_GAMMA_F32),
        ("SRGB_INV_GAMMA", SRGB_INV_GAMMA_F32),
        ("ADOBE_INV_GAMMA", ADOBE_INV_GAMMA),
        ("ONE_THIRD", (1.0f64 / 3.0) as f32),
        ("CBRT_NEWTON_MIN", CBRT_NEWTON_RANGE.0),
        ("CBRT_NEWTON_MAX", CBRT_NEWTON_RANGE.1),
        ("HIGHLIGHTS_MAX_EV", HIGHLIGHTS_MAX_EV),
        ("HIGHLIGHTS_CENTER_EV", HIGHLIGHTS_CENTER_EV),
        ("HIGHLIGHTS_WIDTH_EV", HIGHLIGHTS_WIDTH_EV),
        ("SHADOWS_MAX_EV", SHADOWS_MAX_EV),
        ("SHADOWS_CENTER_EV", SHADOWS_CENTER_EV),
        ("SHADOWS_WIDTH_EV", SHADOWS_WIDTH_EV),
        ("WHITES_MAX_EV", WHITES_MAX_EV),
        ("WHITES_CENTER_EV", WHITES_CENTER_EV),
        ("WHITES_WIDTH_EV", WHITES_WIDTH_EV),
        ("BLACKS_MAX_EV", BLACKS_MAX_EV),
        ("BLACKS_CENTER_EV", BLACKS_CENTER_EV),
        ("BLACKS_WIDTH_EV", BLACKS_WIDTH_EV),
        ("VIBRANCE_MAX", VIBRANCE_MAX),
        ("VIBRANCE_CHROMA_REF", VIBRANCE_CHROMA_REF),
        ("SKIN_PROTECTION", SKIN_PROTECTION),
        ("S2D_T_MAX", S2D_T_MAX),
        ("COORD_LIMIT", COORD_LIMIT),
        ("DITHER_SCALE", DITHER_SCALE),
    ]
}

/// OKLab の行列（f32。genzo-color の CPU 版が使う f32 の行列そのもの）:
/// (BT.2020 → LMS, M2, M2 の逆, LMS → BT.2020)。
pub(crate) fn oklab_matrices() -> [[[f32; 3]; 3]; 4] {
    [
        BT2020_TO_OKLAB_LMS_F32.rows(),
        OKLAB_M2_F32.rows(),
        OKLAB_M2_INV_F32.rows(),
        OKLAB_LMS_TO_BT2020_F32.rows(),
    ]
}

/// モジュールの先頭に付ける生成した定数（パラメータの位置と共通の定数）。
pub(crate) fn generated_header(module: Module) -> String {
    let mut s = String::from("// ---- genzo-gpu が生成した定数 ----\n");
    for (i, name) in HEADER.iter().enumerate() {
        s.push_str(&format!("const P_{name}: u32 = {i}u;\n"));
    }
    let mut at = HEADER.len() as u32;
    for (name, count) in module.slots() {
        s.push_str(&format!("const P_{name}: u32 = {at}u;\n"));
        at += count;
    }
    for (name, v) in shared_constants() {
        s.push_str(&format!("const {name}: f32 = {};\n", wgsl_f32(v)));
    }
    let [lms, m2, m2i, rgb] = oklab_matrices();
    wgsl_rows(&mut s, "OK_LMS", lms);
    wgsl_rows(&mut s, "OK_M2", m2);
    wgsl_rows(&mut s, "OK_M2I", m2i);
    wgsl_rows(&mut s, "OK_RGB", rgb);
    s.push_str("// ---- ここまで ----\n");
    s
}

/// カーネルのパラメータ（u32 の配列）を組み立てる。
#[derive(Debug, Clone)]
pub(crate) struct Params {
    kernel: Kernel,
    words: Vec<u32>,
    set: Vec<bool>,
    error: Option<String>,
}

impl Params {
    /// 入力・出力のタイルの範囲を共通の位置に入れて作る。
    pub(crate) fn new(kernel: Kernel, input: Roi, output: Roi) -> Self {
        let len = kernel.module().param_len() as usize;
        let mut p = Self {
            kernel,
            words: vec![0; len],
            set: vec![false; len],
            error: None,
        };
        p.roi("IN", input);
        p.roi("OUT", output);
        p
    }

    fn roi(&mut self, prefix: &str, roi: Roi) {
        self.i32(&format!("{prefix}_X"), roi.x);
        self.i32(&format!("{prefix}_Y"), roi.y);
        self.u32(&format!("{prefix}_W"), roi.width);
        self.u32(&format!("{prefix}_H"), roi.height);
    }

    fn put(&mut self, name: &str, values: &[u32]) {
        match self.kernel.module().slot(name) {
            Some((at, count)) if values.len() as u32 == count => {
                for (k, v) in values.iter().enumerate() {
                    self.words[at as usize + k] = *v;
                    self.set[at as usize + k] = true;
                }
            }
            Some((_, count)) => {
                self.error.get_or_insert(format!(
                    "パラメータ {name} の個数が違う（{} 個、必要 {count} 個）",
                    values.len()
                ));
            }
            None => {
                self.error
                    .get_or_insert(format!("パラメータ {name} は {:?} にない", self.kernel));
            }
        }
    }

    pub(crate) fn u32(&mut self, name: &str, v: u32) -> &mut Self {
        self.put(name, &[v]);
        self
    }

    pub(crate) fn i32(&mut self, name: &str, v: i32) -> &mut Self {
        self.put(name, &[v as u32]);
        self
    }

    pub(crate) fn f32(&mut self, name: &str, v: f32) -> &mut Self {
        self.put(name, &[v.to_bits()]);
        self
    }

    pub(crate) fn f32s(&mut self, name: &str, v: &[f32]) -> &mut Self {
        let words: Vec<u32> = v.iter().map(|x| x.to_bits()).collect();
        self.put(name, &words);
        self
    }

    pub(crate) fn u32s(&mut self, name: &str, v: &[u32]) -> &mut Self {
        self.put(name, v);
        self
    }

    /// 行列（行優先の 9 個）。
    pub(crate) fn mat(&mut self, name: &str, rows: [[f32; 3]; 3]) -> &mut Self {
        self.f32s(name, rows.as_flattened())
    }

    /// 組み立てた語の列。名前の誤り・必須のパラメータの設定し忘れはエラー（この crate の不具合）。
    pub(crate) fn finish(&self) -> Result<Vec<u32>> {
        if let Some(e) = &self.error {
            return Err(GpuError::Internal(e.clone()));
        }
        for name in self.kernel.required() {
            let (at, count) = self.kernel.module().slot(name).ok_or_else(|| {
                GpuError::Internal(format!("必須のパラメータ {name} の位置がない"))
            })?;
            if !(at..at + count).all(|i| self.set[i as usize]) {
                return Err(GpuError::Internal(format!(
                    "{:?} のパラメータ {name} が設定されていない",
                    self.kernel
                )));
            }
        }
        Ok(self.words.clone())
    }
}

/// 1 回の実行（ディスパッチ）の資源。
pub(crate) struct Dispatch<'a> {
    pub(crate) kernel: Kernel,
    pub(crate) params: Vec<u32>,
    pub(crate) src: &'a wgpu::Buffer,
    pub(crate) dst: &'a wgpu::Buffer,
    pub(crate) aux: Option<&'a wgpu::Buffer>,
    pub(crate) aux_out: Option<&'a wgpu::Buffer>,
    /// スレッドの数（x, y）。ワークグループの数はこれをワークグループの大きさで割って切り上げる。
    pub(crate) threads: (u32, u32),
}

/// コンパイルしたパイプラインと、共通のバインディングの配置。
pub(crate) struct KernelCache {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    modules: Mutex<HashMap<Module, wgpu::ShaderModule>>,
    pipelines: Mutex<HashMap<Kernel, wgpu::ComputePipeline>>,
    dummy_ro: wgpu::Buffer,
    dummy_rw: wgpu::Buffer,
}

impl KernelCache {
    pub(crate) fn new(gpu: &GpuContext) -> Result<Self> {
        let entry = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let device = gpu.device();
        let (layout, pipeline_layout) = gpu.scoped(|| {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("genzo-gpu.layout"),
                entries: &[
                    entry(0, true),
                    entry(1, false),
                    entry(2, true),
                    entry(3, false),
                    entry(4, true),
                    entry(5, false),
                ],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("genzo-gpu.pipeline_layout"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            (layout, pipeline_layout)
        })?;
        Ok(Self {
            layout,
            pipeline_layout,
            modules: Mutex::new(HashMap::new()),
            pipelines: Mutex::new(HashMap::new()),
            dummy_ro: gpu.create_storage("genzo-gpu.dummy_ro", 16)?,
            dummy_rw: gpu.create_storage("genzo-gpu.dummy_rw", 16)?,
        })
    }

    /// カーネルのパイプライン（なければコンパイルする）。
    pub(crate) fn pipeline(
        &self,
        gpu: &GpuContext,
        kernel: Kernel,
    ) -> Result<wgpu::ComputePipeline> {
        if let Some(p) = self
            .pipelines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&kernel)
        {
            return Ok(p.clone());
        }
        let module = self.module(gpu, kernel.module())?;
        let pipeline = gpu.scoped(|| {
            gpu.device()
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(kernel.entry_point()),
                    layout: Some(&self.pipeline_layout),
                    module: &module,
                    entry_point: Some(kernel.entry_point()),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: None,
                })
        })?;
        self.pipelines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(kernel, pipeline.clone());
        Ok(pipeline)
    }

    fn module(&self, gpu: &GpuContext, module: Module) -> Result<wgpu::ShaderModule> {
        if let Some(m) = self
            .modules
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&module)
        {
            return Ok(m.clone());
        }
        let source = module.source();
        let m = gpu.scoped(|| {
            gpu.device()
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(module.label()),
                    source: wgpu::ShaderSource::Wgsl(source.into()),
                })
        })?;
        self.modules
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(module, m.clone());
        Ok(m)
    }

    /// すべてのカーネルをコンパイルする（シェーダーの誤りを早く見つけるため・計測の前の準備）。
    pub(crate) fn compile_all(&self, gpu: &GpuContext) -> Result<()> {
        for k in Kernel::ALL {
            self.pipeline(gpu, k)?;
        }
        Ok(())
    }
}

/// 1 回の投入にまとめるコマンド（ディスパッチとコピー）と、置き換えた NaN・無限大の数のカウンター。
pub(crate) struct Batch<'g> {
    gpu: &'g GpuContext,
    kernels: &'g KernelCache,
    encoder: wgpu::CommandEncoder,
    stats: wgpu::Buffer,
    recorded: usize,
    /// NaN・無限大を数えるカーネルを記録したか（数えないなら投入の後にカウンターを読まない）。
    counts: bool,
}

impl<'g> Batch<'g> {
    pub(crate) fn new(gpu: &'g GpuContext, kernels: &'g KernelCache) -> Result<Self> {
        let stats = gpu.create_storage("genzo-gpu.stats", 16)?;
        Self::with_stats(gpu, kernels, stats)
    }

    /// カウンターのバッファを指定して作る（何回かの投入で 1 つのカウンターに数え、最後に 1 回だけ
    /// 読む場合。[`read_count`]）。
    pub(crate) fn with_stats(
        gpu: &'g GpuContext,
        kernels: &'g KernelCache,
        stats: wgpu::Buffer,
    ) -> Result<Self> {
        let encoder = gpu.scoped(|| {
            gpu.device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("genzo-gpu.batch"),
                })
        })?;
        Ok(Self {
            gpu,
            kernels,
            encoder,
            stats,
            recorded: 0,
            counts: false,
        })
    }

    /// カーネルを 1 回実行するコマンドを記録する。
    pub(crate) fn dispatch(&mut self, d: Dispatch<'_>) -> Result<()> {
        let (tx, ty) = d.threads;
        if tx == 0 || ty == 0 {
            return Ok(());
        }
        let (wx, wy) = d.kernel.workgroup_size();
        let groups = (tx.div_ceil(wx), ty.div_ceil(wy));
        let max = self.gpu.limits().max_compute_workgroups_per_dimension;
        if groups.0 > max || groups.1 > max {
            return Err(GpuError::TooLarge {
                what: format!("{:?} のワークグループの数", d.kernel),
                bytes: u64::from(groups.0.max(groups.1)),
                limit: u64::from(max),
            });
        }
        let pipeline = self.kernels.pipeline(self.gpu, d.kernel)?;
        let params = self
            .gpu
            .create_storage_init("genzo-gpu.params", bytemuck::cast_slice(&d.params))?;
        let aux = d.aux.unwrap_or(&self.kernels.dummy_ro);
        let aux_out = d.aux_out.unwrap_or(&self.kernels.dummy_rw);
        let gpu = self.gpu;
        let layout = &self.kernels.layout;
        let stats = &self.stats;
        let encoder = &mut self.encoder;
        gpu.scoped(|| {
            let bind = gpu.device().create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(d.kernel.entry_point()),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: d.src.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: d.dst.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: stats.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: aux.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: aux_out.as_entire_binding(),
                    },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(d.kernel.entry_point()),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(groups.0, groups.1, 1);
        })?;
        self.recorded += 1;
        self.counts |= d.kernel.counts_non_finite();
        Ok(())
    }

    /// バッファの間のコピーを記録する。
    pub(crate) fn copy(
        &mut self,
        src: &wgpu::Buffer,
        src_offset: u64,
        dst: &wgpu::Buffer,
        dst_offset: u64,
        size: u64,
    ) -> Result<()> {
        let encoder = &mut self.encoder;
        self.gpu.scoped(|| {
            encoder.copy_buffer_to_buffer(src, src_offset, dst, dst_offset, size);
        })?;
        self.recorded += 1;
        Ok(())
    }

    /// 投入して完了まで待ち、置き換えた NaN・無限大の数を返す。
    pub(crate) fn submit(self) -> Result<u64> {
        let (gpu, stats, counts) = (self.gpu, self.stats.clone(), self.counts);
        self.submit_only()?;
        if counts {
            read_count(gpu, &stats)
        } else {
            Ok(0)
        }
    }

    /// 投入して完了まで待つ（カウンターは読まない）。
    pub(crate) fn submit_only(self) -> Result<()> {
        if self.recorded == 0 {
            return Ok(());
        }
        let gpu = self.gpu;
        let commands = gpu.scoped(|| self.encoder.finish())?;
        gpu.submit_and_wait(commands)
    }
}

/// カウンターのバッファ（`[0]` は置き換えた NaN・無限大の数）を読む。
pub(crate) fn read_count(gpu: &GpuContext, stats: &wgpu::Buffer) -> Result<u64> {
    let bytes = gpu.read_buffer(stats, 4)?;
    Ok(u64::from(u32::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
    ])))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成した 16 進の表記を読み戻す（テスト用の簡単な解析）。
    fn parse_hex_f32(s: &str) -> f32 {
        let (neg, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
        let s = s.strip_suffix('f').expect("f の接尾辞");
        if s == "0.0" {
            return if neg { -0.0 } else { 0.0 };
        }
        let s = s.strip_prefix("0x").expect("0x");
        let (mant, exp) = s.split_once('p').expect("p");
        let (int, frac) = mant.split_once('.').expect(".");
        let m = u64::from_str_radix(&format!("{int}{frac}"), 16).unwrap();
        let e: i32 = exp.parse().unwrap();
        let v = (m as f64) * 2f64.powi(e - 4 * frac.len() as i32);
        let v = v as f32;
        if neg { -v } else { v }
    }

    fn constant(name: &str) -> f32 {
        shared_constants()
            .into_iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("定数 {name} がない"))
    }

    /// WGSL に埋め込む定数で、genzo-color の f32 の式を Rust で書き直したもの（シェーダーと同じ式）。
    fn srgb_encode_replica(x: f32) -> f32 {
        let a = x.abs();
        let e = if a <= constant("SRGB_ENCODE_THRESHOLD") {
            constant("SRGB_LINEAR_SLOPE") * a
        } else {
            constant("SRGB_ONE_PLUS_OFFSET") * a.powf(constant("SRGB_INV_GAMMA"))
                - constant("SRGB_OFFSET")
        };
        e.copysign(x)
    }

    fn srgb_decode_replica(v: f32) -> f32 {
        let a = v.abs();
        let l = if a <= constant("SRGB_DECODE_THRESHOLD") {
            a / constant("SRGB_LINEAR_SLOPE")
        } else {
            ((a + constant("SRGB_OFFSET")) / constant("SRGB_ONE_PLUS_OFFSET"))
                .powf(constant("SRGB_GAMMA"))
        };
        l.copysign(v)
    }

    #[test]
    fn embedded_constants_reproduce_the_cpu_formulas() {
        use genzo_color::oklab::{linear_bt2020_to_oklab, oklab_to_linear_bt2020};
        use genzo_color::transfer::{adobe_rgb_encode_f32, srgb_decode_f32, srgb_encode_f32};
        // 同じ式・同じ定数なら、CPU（同じ数学ライブラリ）ではビット単位で同じになる。
        for i in 0..=2000 {
            let x = (i as f32 / 1000.0 - 1.0) * 1.3;
            assert_eq!(
                srgb_encode_replica(x).to_bits(),
                srgb_encode_f32(x).to_bits(),
                "{x}"
            );
            assert_eq!(
                srgb_decode_replica(x).to_bits(),
                srgb_decode_f32(x).to_bits(),
                "{x}"
            );
            let adobe = x.abs().powf(constant("ADOBE_INV_GAMMA")).copysign(x);
            assert_eq!(adobe.to_bits(), adobe_rgb_encode_f32(x).to_bits(), "{x}");
        }
        let [lms, m2, m2i, rgb] = oklab_matrices();
        let apply = |m: [[f32; 3]; 3], v: [f32; 3]| {
            std::array::from_fn::<f32, 3, _>(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
        };
        for c in [
            [0.18f32; 3],
            [0.5, 0.2, 0.1],
            [-0.1, 0.4, 0.9],
            [2.0, 0.0, 0.3],
        ] {
            let l = apply(lms, c);
            let lab = apply(m2, l.map(f32::cbrt));
            assert_eq!(lab, linear_bt2020_to_oklab(c), "{c:?}");
            let back = apply(m2i, lab).map(|v| v * v * v);
            assert_eq!(apply(rgb, back), oklab_to_linear_bt2020(lab), "{c:?}");
        }
        // 定数の値の確認。
        assert_eq!(constant("DITHER_SCALE"), 2f32.powi(-24));
        assert_eq!(constant("LOG_FLOOR"), 2f32.powi(-16));
        assert_eq!(constant("ONE_THIRD"), (1.0f64 / 3.0) as f32);
    }

    #[test]
    fn hex_literals_round_trip() {
        for v in [
            1.0f32,
            -1.0,
            0.5,
            0.2627,
            -2.473_931,
            1e30,
            1e-30,
            f32::MIN_POSITIVE,
            f32::MIN_POSITIVE / 8.0,
            f32::MAX,
            0.0,
            12.92,
            1.0 / 2.4,
        ] {
            let s = wgsl_f32(v);
            assert_eq!(parse_hex_f32(&s).to_bits(), v.to_bits(), "{v} → {s}");
        }
        assert_eq!(wgsl_f32(1.0), "0x1.000000p0f");
        assert_eq!(wgsl_f32(-0.5), "-0x1.000000p-1f");
    }

    #[test]
    fn slots_are_unique_and_required_exist() {
        for m in Module::ALL {
            let mut names: Vec<&str> = HEADER.to_vec();
            names.extend(m.slots().iter().map(|s| s.0));
            let mut sorted = names.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), names.len(), "{m:?} に重複");
        }
        for k in Kernel::ALL {
            for name in k.required() {
                assert!(k.module().slot(name).is_some(), "{k:?} {name}");
            }
        }
    }

    #[test]
    fn params_detect_mistakes() {
        let roi = Roi::new(1, 2, 3, 4);
        let mut p = Params::new(Kernel::Exposure, roi, roi);
        assert!(p.finish().is_err(), "GAIN の設定し忘れ");
        p.f32("GAIN", 2.0);
        let w = p.finish().unwrap();
        assert_eq!(w[0], 1);
        assert_eq!(w[6], 3);
        assert_eq!(f32::from_bits(w[8]), 2.0);
        let mut q = Params::new(Kernel::Exposure, roi, roi);
        q.f32("GAIN", 1.0).f32("NOPE", 1.0);
        assert!(q.finish().is_err());
        let mut r = Params::new(Kernel::Display, roi, roi);
        r.f32s("W2D", &[1.0; 3]);
        assert!(r.finish().is_err(), "個数の誤り");
        // 負の座標は i32 のビット列。
        let n = Params::new(Kernel::Exposure, Roi::new(-5, 0, 1, 1), roi);
        assert_eq!(n.words[0] as i32, -5);
    }

    #[test]
    fn shaders_translate_to_metal_and_hlsl() {
        // wgpu は Metal では MSL、DirectX 12 では HLSL に変換してからコンパイルする。Linux のテストでも
        // naga の変換が通ることを確かめる（Metal・FXC / DXC のコンパイラ自体は、アダプターのある環境の
        // tests/consistency.rs の all_kernels_compile（CI のランナーの WARP・仮想の GPU を含む）と実機で確かめる）。
        use naga::valid::{Capabilities, ValidationFlags, Validator};
        for m in Module::ALL {
            let module = naga::front::wgsl::parse_str(&m.source())
                .unwrap_or_else(|e| panic!("{m:?}: {}", e.emit_to_string(&m.source())));
            let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{m:?}: {e:?}"));
            let msl_options = naga::back::msl::Options {
                lang_version: (2, 4),
                ..Default::default()
            };
            let (msl, _) = naga::back::msl::write_string(
                &module,
                &info,
                &msl_options,
                &naga::back::msl::PipelineOptions::default(),
            )
            .unwrap_or_else(|e| panic!("{m:?} MSL: {e}"));
            assert!(msl.contains("kernel"), "{m:?}");
            let hlsl_options = naga::back::hlsl::Options::default();
            let pipeline = naga::back::hlsl::PipelineOptions::default();
            let mut hlsl = String::new();
            naga::back::hlsl::Writer::new(&mut hlsl, &hlsl_options, &pipeline)
                .write(&module, &info, None)
                .unwrap_or_else(|e| panic!("{m:?} HLSL: {e}"));
            assert!(hlsl.contains("numthreads"), "{m:?}");
        }
    }

    #[test]
    fn workgroup_sizes_match_the_shaders() {
        // ディスパッチのワークグループの数は Kernel::workgroup_size で決める。WGSL の @workgroup_size と
        // 違うと、画像の端の画素を処理しない（小さい場合）か、ワークグループの数が余分になる。
        for k in Kernel::ALL {
            let source = k.module().source();
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{k:?}: {}", e.emit_to_string(&source)));
            let ep = module
                .entry_points
                .iter()
                .find(|e| e.name == k.entry_point())
                .unwrap_or_else(|| panic!("{k:?}: 入口 {} がない", k.entry_point()));
            let (x, y) = k.workgroup_size();
            assert_eq!(ep.workgroup_size, [x, y, 1], "{k:?}");
        }
    }

    #[test]
    fn header_declares_every_slot_and_constant() {
        for m in Module::ALL {
            let h = generated_header(m);
            for (name, _) in m.slots() {
                assert!(h.contains(&format!("const P_{name}: u32")), "{m:?} {name}");
            }
            assert!(h.contains("const LUMA_R: f32"));
            assert!(h.contains("const OK_M2I_R2: vec3<f32>"));
            assert!(m.source().contains("fn mirror_index"));
        }
    }
}
