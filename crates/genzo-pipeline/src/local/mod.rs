//! ローカル補正（マスク。DEV-18・DEV-19。処理バージョン 2。docs/04_architecture.md の 2.1 節「ローカル補正」）。
//!
//! マスク（genzo-model の `LocalAdjustment`）ごとに、画素の **重み** `m`（0〜1）を部品の合成で求め、調整値の差分
//! `δ` を掛けてすべてのマスクについて足した **局所の値** `Σ m·δ` を、項目ごとの面（[`LocalField`]）にする。
//! ステージ 10〜14 は、画素ごとに「全体の値 ＋ 局所の値」をそのステージのパラメータとして使う（[`LocalParam`] の表）。
//!
//! # 流れ（段階 C）
//!
//! 1. [`crate::finish::FinishPlan::new`] が、丸めた設定から計画（[`LocalPlan`]）を作る。有効なマスク（有効・部品が
//!    ある・差分が 0 でない）がなければ作らない。面を作る項目（[`LocalSlots`]）は、どれかの有効なマスクで差分が 0 で
//!    ない項目（決まった順に面の番号を振る）。ブラシの部品は G 座標のラスターにする（[`brush`]）。
//! 2. ステージ 9 の出力（ステージ 9 を飛ばすときは段階 A1 の画像。**露光量の前**の B2）を **基準の画像** として、
//!    画素ごとに重みと局所の値を求める（[`LocalPlan::field`]）。範囲マスクの色・明るさはこの基準の画像で決める
//!    （ステージ 10〜14 の結果で選択範囲が変わらない）。
//! 3. ステージ 10〜14 は、文脈の [`crate::StageContext::local`] から画素の局所の値を引く。
//!
//! GPU 版（genzo-gpu の `local.wgsl`）も同じ式・同じ順序で面を作り、ステージ 10〜14 のカーネルが同じように引く。
//!
//! # 座標
//!
//! C の画素 (x, y)（格子の番号）の G 座標は `u = ((ox + x) as f32 + 0.5) · sx`、`v = ((oy + y) as f32 + 0.5) · sy`
//! （`(ox, oy)` は切り抜きの範囲の G の格子での位置、`sx = 1 / (G の格子の密度 × G の長辺)`）。ガイドの引き方
//! （[`crate::guide::GuideLookup`]）と同じ形。マスクは G 座標で決まるので、解像度・切り抜き・タイルの分け方によらない。
//!
//! # 部品の値（f32、この順序）
//!
//! ```text
//! 線形:   t = ((u − x0)·dx + (v − y0)·dy) · inv_len2、 c = 1 − smoothstep(0, 1, t)     （長さ 0 なら c = 0）
//! 円形:   qx = (u − cx)·cos θ + (v − cy)·sin θ、 qy = (v − cy)·cos θ − (u − cx)·sin θ
//!         ρ = sqrt((qx·inv_rx)² + (qy·inv_ry)²)、 c = 1 − step(i0, 1, ρ)                （i0 = 1 − ぼかし / 100）
//! ブラシ: c = ラスターを双線形で引いた値（[`brush`]）
//! 色域:   (L, a, b) = OKLab(基準の画素)、 q = (a, b) / max(L, COLOR_L_FLOOR)
//!         d_i = |q − t_i|（t_i は見本の色の (a, b) / max(L, COLOR_L_FLOOR)）、 c = max_i (1 − step(lo, hi, d_i))
//!         hi = COLOR_RANGE_MIN + (COLOR_RANGE_MAX − COLOR_RANGE_MIN) × 許容量 / 100、 lo = COLOR_RANGE_INNER × hi
//! 輝度:   e = srgb_encode(clamp(f15(Y × 2^EV), 0, 1))                                    （f15 はステージ 15 の輝度の
//!         カーブ、EV は全体の露光量。表示の明るさの近似）
//!         滑らかさ s = 滑らかさ / 100 × LUMA_RANGE_SMOOTH_MAX、 lo = 下端 / 100、 hi = 上端 / 100
//!         s = 0 なら c = (lo ≤ e ≤ hi)、それ以外は c = smoothstep(lo − s, lo, e) · (1 − smoothstep(hi, hi + s, e))
//! step(e0, e1, x) = e1 > e0 なら smoothstep(e0, e1, x)、それ以外は x ≥ e1 なら 1、それ以外は 0
//! ```
//!
//! 反転なら `c = 1 − c`。合成は genzo-model の `MaskMode::combine`（加える `max`・除く `m·(1 − c)`・交差 `m·c`。初めの値は
//! 最初の部品が加えるなら 0、それ以外は 1）。局所の値は、項目の面ごとに `Σ m·δ` をマスクの順に左から足す。
//!
//! # 値の範囲（仮置き。決定事項ログ No.152）
//!
//! 全体の値と局所の値の和は、露光量は ±[`LOCAL_EXPOSURE_LIMIT`] EV、色温度・色かぶりは ±100、彩度は −100〜+200、
//! かすみの除去は ±100、シャープは −100〜+150（負はぼかす）、ほかのスライダーは ±200 に収める（ステージごとの式は
//! 各モジュールの doc）。

pub mod brush;

use std::sync::Arc;

use genzo_color::oklab::linear_bt2020_to_oklab;
use genzo_color::transfer::srgb_encode_f32;
use genzo_model::{DevelopSettings, LocalAdjustment, LocalDelta, MaskMode, MaskShape};
use rayon::prelude::*;

use crate::error::{PipelineError, Result};
use crate::finish::geometry::GeometryPlan;
use crate::finish::luminance;
use crate::finish::scene_to_display::SceneToDisplayParams;
use crate::image::{ImageTile, Roi};
use crate::stage::StageContext;
use crate::version::ProcessVersion;

pub use self::brush::BrushRaster;

/// 色域の範囲の、明るさで割るときの下限（OKLab の L。とても暗い画素の色の成分を、無彩色に寄せて扱う）。仮置き。
pub const COLOR_L_FLOOR: f32 = 0.05;
/// 色域の範囲の許容量 0 のときの距離（`(a, b) / L` の距離）。仮置き。
pub const COLOR_RANGE_MIN: f32 = 0.02;
/// 色域の範囲の許容量 100 のときの距離。仮置き。
pub const COLOR_RANGE_MAX: f32 = 0.3;
/// 色域の範囲の、重み 1 の範囲の割合（距離がこの割合 × 許容の距離までは 1）。仮置き。
pub const COLOR_RANGE_INNER: f32 = 0.4;
/// 輝度の範囲の滑らかさ 100 のときの幅（符号化した明るさの 0〜1 の単位）。仮置き。
pub const LUMA_RANGE_SMOOTH_MAX: f32 = 0.4;
/// 全体と局所の露光量の和の範囲（EV）。
pub const LOCAL_EXPOSURE_LIMIT: f32 = 10.0;
/// 局所の色温度 ±100 での、R と B の倍率（段。R は +、B は −）。仮置き。
pub const LOCAL_TEMPERATURE_EV: f32 = 0.5;
/// 局所の色かぶり ±100 での、G の倍率（段。マゼンタ（+）で G を下げ、R・B を半分ずつ上げる）。仮置き。
pub const LOCAL_TINT_EV: f32 = 0.4;

/// 局所の値の項目（genzo-model の `LocalDelta::values` と同じ順）と、使うステージ。
///
/// | 項目 | 使うステージ |
/// |---|---|
/// | 色温度・色かぶり・露光量 | 10（露光量。色温度・色かぶりは作業色空間の RGB の倍率）。露光量は 11・12 の局所の明るさにも |
/// | コントラスト | 10 の後（コントラスト）。11・12 の局所の明るさにも |
/// | ハイライト・シャドウ・白・黒 | 11（トーン）。12 の局所の明るさにも |
/// | テクスチャ・明瞭度・かすみの除去 | 12 |
/// | 彩度 | 13 |
/// | シャープ | 14 |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalParam {
    /// 色温度。
    Temperature,
    /// 色かぶり補正。
    Tint,
    /// 露光量。
    Exposure,
    /// コントラスト。
    Contrast,
    /// ハイライト。
    Highlights,
    /// シャドウ。
    Shadows,
    /// 白レベル。
    Whites,
    /// 黒レベル。
    Blacks,
    /// テクスチャ。
    Texture,
    /// 明瞭度。
    Clarity,
    /// かすみの除去。
    Dehaze,
    /// 彩度。
    Saturation,
    /// シャープ。
    Sharpness,
}

/// 項目の数。
pub const LOCAL_PARAM_COUNT: usize = LocalDelta::LEN;

/// 1 画素の局所の値（[`LocalParam`] の順）。
pub type LocalValues = [f32; LOCAL_PARAM_COUNT];

impl LocalParam {
    /// すべての項目（番号の順）。
    pub const ALL: [LocalParam; LOCAL_PARAM_COUNT] = [
        LocalParam::Temperature,
        LocalParam::Tint,
        LocalParam::Exposure,
        LocalParam::Contrast,
        LocalParam::Highlights,
        LocalParam::Shadows,
        LocalParam::Whites,
        LocalParam::Blacks,
        LocalParam::Texture,
        LocalParam::Clarity,
        LocalParam::Dehaze,
        LocalParam::Saturation,
        LocalParam::Sharpness,
    ];

    /// 番号（[`LocalValues`] の位置）。
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// ステージ 10（露光量）の項目。
    pub const EXPOSURE_STAGE: [LocalParam; 3] = [
        LocalParam::Temperature,
        LocalParam::Tint,
        LocalParam::Exposure,
    ];
    /// ステージ 11（トーン）の項目。
    pub const TONE_STAGE: [LocalParam; 4] = [
        LocalParam::Highlights,
        LocalParam::Shadows,
        LocalParam::Whites,
        LocalParam::Blacks,
    ];
    /// ステージ 11・12 の局所の明るさに使う項目（露光量・コントラスト・トーン）。
    pub const LOCAL_BASE: [LocalParam; 6] = [
        LocalParam::Exposure,
        LocalParam::Contrast,
        LocalParam::Highlights,
        LocalParam::Shadows,
        LocalParam::Whites,
        LocalParam::Blacks,
    ];
    /// ステージ 12（テクスチャ・明瞭度・かすみの除去）の項目。
    pub const PRESENCE_STAGE: [LocalParam; 3] =
        [LocalParam::Texture, LocalParam::Clarity, LocalParam::Dehaze];
}

/// 局所の値の、どれかが 0 でないか。
#[inline]
pub fn any_nonzero(v: &LocalValues, params: &[LocalParam]) -> bool {
    params.iter().any(|p| v[p.index()] != 0.0)
}

/// スライダーの単位（スライダーの値 / 100）の全体の値 `unit` に、局所の値 `delta`（スライダーの値）を足して
/// `[lo, hi]`（単位）に収める（各ステージの共通の式: `clamp(unit + delta / 100, lo, hi)`）。
#[inline]
pub fn add_unit(unit: f32, delta: f32, lo: f32, hi: f32) -> f32 {
    (unit + delta / 100.0).clamp(lo, hi)
}

/// 項目 → 面の番号（局所の値の面を作る項目）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalSlots {
    planes: [u8; LOCAL_PARAM_COUNT],
    count: u8,
}

impl LocalSlots {
    /// 面のない項目の印。
    pub const NONE: u8 = u8::MAX;

    /// 有効なマスク（[`LocalSlots::is_active_mask`]）のどれかで差分が 0 でない項目に、番号の順に面を振る。処理
    /// バージョン 1、有効なマスクがない、差分がすべて 0 なら `None`。`settings` は丸めた設定であること。
    pub fn for_settings(settings: &DevelopSettings, version: ProcessVersion) -> Option<Self> {
        if !version.has_v1_features() {
            return None;
        }
        let mut used = [false; LOCAL_PARAM_COUNT];
        for m in settings.masks.iter().filter(|m| Self::is_active_mask(m)) {
            for (u, v) in used.iter_mut().zip(m.delta.values()) {
                *u |= v != 0.0;
            }
        }
        let mut planes = [Self::NONE; LOCAL_PARAM_COUNT];
        let mut count = 0u8;
        for (p, u) in planes.iter_mut().zip(used) {
            if u {
                *p = count;
                count += 1;
            }
        }
        (count > 0).then_some(Self { planes, count })
    }

    /// 描画に使うマスクか（有効・部品がある・差分が 0 でない）。
    pub fn is_active_mask(m: &LocalAdjustment) -> bool {
        !m.is_neutral()
    }

    /// 項目 `p` の面の番号（面がなければ `None`）。
    #[inline]
    pub fn plane(&self, p: LocalParam) -> Option<usize> {
        let k = self.planes[p.index()];
        (k != Self::NONE).then_some(usize::from(k))
    }

    /// 項目 `p` の面があるか。
    #[inline]
    pub fn uses(&self, p: LocalParam) -> bool {
        self.planes[p.index()] != Self::NONE
    }

    /// `params` のどれかの面があるか。
    pub fn uses_any(&self, params: &[LocalParam]) -> bool {
        params.iter().any(|p| self.uses(*p))
    }

    /// 面の数。
    pub fn count(&self) -> usize {
        usize::from(self.count)
    }

    /// 項目ごとの面の番号（GPU 版のパラメータ。面がなければ `u32::MAX`）。
    pub fn to_u32(&self) -> [u32; LOCAL_PARAM_COUNT] {
        self.planes.map(|k| {
            if k == Self::NONE {
                u32::MAX
            } else {
                u32::from(k)
            }
        })
    }
}

/// 文脈と丸めた設定から、局所の値の面の割り当て（[`LocalSlots::for_settings`]）。
pub fn local_slots(settings: &DevelopSettings, ctx: &StageContext<'_>) -> Option<LocalSlots> {
    LocalSlots::for_settings(settings, ctx.process_version)
}

/// 局所の値の面（[`LocalPlan::field`] の結果。範囲は C の格子の座標）。
#[derive(Debug, Clone, PartialEq)]
pub struct LocalField {
    /// 範囲（C の格子）。
    pub roi: Roi,
    /// 項目 → 面の番号。
    pub slots: LocalSlots,
    /// 面（面の番号の順に、`roi` の画素数ずつ。行優先）。
    pub planes: Vec<f32>,
}

impl LocalField {
    /// 画素 (x, y)（`roi` の中）の局所の値。
    #[inline]
    pub fn values(&self, x: i64, y: i64) -> LocalValues {
        let mut out = [0.0f32; LOCAL_PARAM_COUNT];
        let n = self.roi.pixel_count() as usize;
        let i = (y - i64::from(self.roi.y)) as usize * self.roi.width as usize
            + (x - i64::from(self.roi.x)) as usize;
        for (o, k) in out.iter_mut().zip(self.slots.planes) {
            if k != LocalSlots::NONE {
                *o = self.planes[usize::from(k) * n + i];
            }
        }
        out
    }

    /// 範囲 `roi` を覆うか。
    pub fn covers(&self, roi: &Roi) -> bool {
        self.roi.contains_roi(roi)
    }
}

/// ステージ `stage` が使う局所の値の面（`slots` があるときは文脈の面が出力の範囲 `out` を覆うこと）。
pub fn checked_field<'a>(
    ctx: &StageContext<'a>,
    stage: &'static str,
    slots: Option<LocalSlots>,
    out: Roi,
) -> Result<Option<&'a LocalField>> {
    let Some(slots) = slots else {
        return Ok(None);
    };
    let field = ctx.local.ok_or(PipelineError::LocalFieldMismatch {
        stage,
        reason: "文脈に局所の値の面がない（ステージ 9 の後に作る）",
    })?;
    if field.slots != slots {
        return Err(PipelineError::LocalFieldMismatch {
            stage,
            reason: "局所の値の面の割り当てがパラメータと違う",
        });
    }
    if !field.covers(&out) {
        return Err(PipelineError::LocalFieldMismatch {
            stage,
            reason: "局所の値の面が出力の範囲を覆っていない",
        });
    }
    Ok(Some(field))
}

/// 準備した部品の形（モジュールの doc の式の係数。GPU 版にも同じ値を渡す）。
#[derive(Debug, Clone, PartialEq)]
pub enum PreparedShape {
    /// 線形（`inv_len2` が 0 なら長さ 0 で、値は常に 0）。
    Linear {
        /// 始点。
        x0: f32,
        /// 始点。
        y0: f32,
        /// 終点 − 始点。
        dx: f32,
        /// 終点 − 始点。
        dy: f32,
        /// 1 / |終点 − 始点|²（長さ 0 なら 0）。
        inv_len2: f32,
    },
    /// 円形。
    Radial {
        /// 中心。
        cx: f32,
        /// 中心。
        cy: f32,
        /// cos θ（f64 で求めて f32 に丸めた値）。
        cos: f32,
        /// sin θ。
        sin: f32,
        /// 1 / 横の半径。
        inv_rx: f32,
        /// 1 / 縦の半径。
        inv_ry: f32,
        /// 重み 1 の内側の範囲（1 − ぼかし / 100）。
        inner: f32,
    },
    /// ブラシ（`None` はストロークがない・G の外で、値は常に 0）。
    Brush(Option<Arc<BrushRaster>>),
    /// 色域の範囲。
    Color {
        /// 見本の色の `(a, b) / max(L, COLOR_L_FLOOR)`。
        targets: Vec<[f32; 2]>,
        /// 重み 1 の距離。
        lo: f32,
        /// 重み 0 の距離。
        hi: f32,
    },
    /// 輝度の範囲。
    Luminance {
        /// 下端（0〜1）。
        lo: f32,
        /// 上端（0〜1）。
        hi: f32,
        /// 滑らかさの幅（0 なら境目でちょうど切る）。
        smooth: f32,
    },
}

/// 準備した部品。
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedComponent {
    /// 合成のしかた。
    pub mode: MaskMode,
    /// 反転。
    pub invert: bool,
    /// 形。
    pub shape: PreparedShape,
}

/// 準備したマスク（重みを求める部品と、面ごとの差分）。
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedMask {
    /// 重みの初めの値（最初の部品が加えるなら 0、それ以外は 1）。
    pub initial: f32,
    /// 部品（合成の順）。
    pub components: Vec<PreparedComponent>,
    /// 面ごとの差分（面の番号の順。[`LocalSlots`] の面の数だけ）。
    pub delta: Vec<f32>,
}

/// 0〜1 のなめらかな段（`e1 > e0` でなければ `x ≥ e1` で 1。モジュールの doc の `step`）。
#[inline]
pub fn step(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 > e0 {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    } else if x >= e1 {
        1.0
    } else {
        0.0
    }
}

/// 基準の画素から、範囲マスクが使う値（OKLab と符号化した明るさ）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeInputs {
    /// `(a, b) / max(L, COLOR_L_FLOOR)`。
    pub chroma: [f32; 2],
    /// 符号化した明るさ（0〜1）。
    pub luma: f32,
}

impl PreparedShape {
    /// G 座標 (u, v) と範囲マスクの値から、部品の値（反転の前）。
    #[inline]
    pub fn value(&self, u: f32, v: f32, range: &RangeInputs) -> f32 {
        match self {
            PreparedShape::Linear {
                x0,
                y0,
                dx,
                dy,
                inv_len2,
            } => {
                if *inv_len2 == 0.0 {
                    return 0.0;
                }
                let t = ((u - x0) * dx + (v - y0) * dy) * inv_len2;
                1.0 - step(0.0, 1.0, t)
            }
            PreparedShape::Radial {
                cx,
                cy,
                cos,
                sin,
                inv_rx,
                inv_ry,
                inner,
            } => {
                let du = u - cx;
                let dv = v - cy;
                let qx = (du * cos + dv * sin) * inv_rx;
                let qy = (dv * cos - du * sin) * inv_ry;
                let rho = (qx * qx + qy * qy).sqrt();
                1.0 - step(*inner, 1.0, rho)
            }
            PreparedShape::Brush(r) => r.as_ref().map_or(0.0, |r| r.sample(u, v)),
            PreparedShape::Color { targets, lo, hi } => {
                let mut c = 0.0f32;
                for t in targets {
                    let da = range.chroma[0] - t[0];
                    let db = range.chroma[1] - t[1];
                    let d = (da * da + db * db).sqrt();
                    c = c.max(1.0 - step(*lo, *hi, d));
                }
                c
            }
            PreparedShape::Luminance { lo, hi, smooth } => {
                let e = range.luma;
                if *smooth > 0.0 {
                    step(lo - smooth, *lo, e) * (1.0 - step(*hi, hi + smooth, e))
                } else if *lo <= e && e <= *hi {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

impl PreparedMask {
    /// 画素の重み（モジュールの doc の合成）。
    #[inline]
    pub fn weight(&self, u: f32, v: f32, range: &RangeInputs) -> f32 {
        let mut m = self.initial;
        for c in &self.components {
            let mut x = c.shape.value(u, v, range);
            if c.invert {
                x = 1.0 - x;
            }
            m = c.mode.combine(m, x);
        }
        m
    }

    /// 範囲マスクの部品があるか。
    pub fn uses_range(&self) -> bool {
        self.components.iter().any(|c| {
            matches!(
                c.shape,
                PreparedShape::Color { .. } | PreparedShape::Luminance { .. }
            )
        })
    }
}

/// 局所の値の面の計画（モジュールの doc の流れの 1）。
#[derive(Debug, Clone, PartialEq)]
pub struct LocalPlan {
    /// 項目 → 面の番号。
    pub slots: LocalSlots,
    /// C の格子の画素 → G 座標の原点（切り抜きの範囲の G の格子での位置）。
    pub origin: (i64, i64),
    /// C の格子の画素 → G 座標の倍率（`1 / (密度 × G の長辺)`）。
    pub scale: (f32, f32),
    /// 有効なマスク（設定の順）。
    pub masks: Vec<PreparedMask>,
    /// 範囲マスクの明るさに掛ける全体の露光量の倍率 `2^EV`（f64 で求めて f32 に丸めた値）。
    pub exposure_gain: f32,
    /// 範囲マスクの明るさのカーブ（ステージ 15 の輝度のカーブ。入力の種類による）。
    pub tone: SceneToDisplayParams,
    /// 範囲マスクの部品があるか（基準の画素の色・明るさを求めるか）。
    pub uses_range: bool,
}

impl LocalPlan {
    /// 丸めた設定 `s` と文脈・ジオメトリから作る（有効なマスクがなければ `None`）。
    pub fn new(
        s: &DevelopSettings,
        ctx: &StageContext<'_>,
        geometry: &GeometryPlan,
    ) -> Result<Option<Self>> {
        let Some(slots) = local_slots(s, ctx) else {
            return Ok(None);
        };
        let masks: Vec<PreparedMask> = s
            .masks
            .iter()
            .filter(|m| LocalSlots::is_active_mask(m))
            .map(|m| prepare_mask(m, geometry, Some(&slots)))
            .collect();
        let uses_range = masks.iter().any(PreparedMask::uses_range);
        let (origin, scale) = g_mapping(geometry);
        Ok(Some(Self {
            slots,
            origin,
            scale,
            masks,
            exposure_gain: f64::from(s.exposure_ev).exp2() as f32,
            tone: SceneToDisplayParams::for_input(ctx.input_kind),
            uses_range,
        }))
    }

    /// C の画素 (x, y) の G 座標。
    #[inline]
    pub fn g_coords(&self, x: i64, y: i64) -> (f32, f32) {
        g_coords(self.origin, self.scale, x, y)
    }

    /// 基準の画素 `c`（B2）から、範囲マスクの値。
    #[inline]
    pub fn range_inputs(&self, c: [f32; 3]) -> RangeInputs {
        range_inputs(c, self.exposure_gain, &self.tone)
    }

    /// 基準の画像 `reference`（ステージ 9 の出力。露光量の前の B2。範囲は C の格子）の範囲全体の、局所の値の面。
    pub fn field(&self, reference: &ImageTile) -> Result<LocalField> {
        let roi = reference.roi;
        let n = roi.pixel_count() as usize;
        let k = self.slots.count();
        let mut planes = vec![0.0f32; n * k];
        let w = roi.width as usize;
        let src = reference.image.data();
        // 行ごとに、面の数 × 幅の値を求めてから、面ごとの位置に写す（行の中の順序は決まっている）。
        let rows: Vec<Vec<f32>> = (0..roi.height as usize)
            .into_par_iter()
            .map(|j| {
                let y = i64::from(roi.y) + j as i64;
                let mut row = vec![0.0f32; w * k];
                for i in 0..w {
                    let x = i64::from(roi.x) + i as i64;
                    let (u, v) = self.g_coords(x, y);
                    let range = if self.uses_range {
                        self.range_inputs(src[j * w + i])
                    } else {
                        RangeInputs {
                            chroma: [0.0; 2],
                            luma: 0.0,
                        }
                    };
                    for m in &self.masks {
                        let wgt = m.weight(u, v, &range);
                        if wgt == 0.0 {
                            continue;
                        }
                        for (p, d) in m.delta.iter().enumerate() {
                            row[p * w + i] += wgt * d;
                        }
                    }
                }
                row
            })
            .collect();
        for (j, row) in rows.iter().enumerate() {
            for p in 0..k {
                planes[p * n + j * w..p * n + (j + 1) * w]
                    .copy_from_slice(&row[p * w..(p + 1) * w]);
            }
        }
        Ok(LocalField {
            roi,
            slots: self.slots,
            planes,
        })
    }
}

/// C の格子の画素 → G 座標の対応（原点と倍率。モジュールの doc の「座標」）。
pub fn g_mapping(geometry: &GeometryPlan) -> ((i64, i64), (f32, f32)) {
    let cl = geometry.crop_lattice();
    let (dx, dy) = geometry.density();
    let long = geometry.frames().g_long_side_px();
    (
        (i64::from(cl.x), i64::from(cl.y)),
        ((1.0 / (dx * long)) as f32, (1.0 / (dy * long)) as f32),
    )
}

/// 出力の画像（C の格子の全体）の縁の G 座標（左・上・右・下。f64）。画面がマスクの位置を出力の画像の上に描く
/// ための対応（出力の画像の割合 `fx` の G 座標は `左 + fx · (右 − 左)`）。
pub fn g_bounds(geometry: &GeometryPlan) -> [f64; 4] {
    let cl = geometry.crop_lattice();
    let (dx, dy) = geometry.density();
    let long = geometry.frames().g_long_side_px();
    let x = |v: i64| v as f64 / (dx * long);
    let y = |v: i64| v as f64 / (dy * long);
    [
        x(i64::from(cl.x)),
        y(i64::from(cl.y)),
        x(i64::from(cl.x) + i64::from(cl.width)),
        y(i64::from(cl.y) + i64::from(cl.height)),
    ]
}

/// C の画素 (x, y) の G 座標（[`g_mapping`] の原点と倍率で）。
#[inline]
pub fn g_coords(origin: (i64, i64), scale: (f32, f32), x: i64, y: i64) -> (f32, f32) {
    (
        ((origin.0 + x) as f32 + 0.5) * scale.0,
        ((origin.1 + y) as f32 + 0.5) * scale.1,
    )
}

/// 基準の画素 `c` から範囲マスクの値（モジュールの doc の色域・輝度の式）。
#[inline]
pub fn range_inputs(c: [f32; 3], exposure_gain: f32, tone: &SceneToDisplayParams) -> RangeInputs {
    let [l, a, b] = linear_bt2020_to_oklab(c);
    let d = l.max(COLOR_L_FLOOR);
    let y = luminance(c) * exposure_gain;
    let luma = srgb_encode_f32(tone.curve(y).clamp(0.0, 1.0));
    RangeInputs {
        chroma: [a / d, b / d],
        luma,
    }
}

/// 色域の範囲の見本の OKLab から、比べる値 `(a, b) / max(L, COLOR_L_FLOOR)`。
#[inline]
pub fn sample_chroma(lab: [f32; 3]) -> [f32; 2] {
    let d = lab[0].max(COLOR_L_FLOOR);
    [lab[1] / d, lab[2] / d]
}

/// マスク `m` を準備する（`slots` があれば、その面の順の差分も。なければ差分は空。重ねて表示するとき）。
pub fn prepare_mask(
    m: &LocalAdjustment,
    geometry: &GeometryPlan,
    slots: Option<&LocalSlots>,
) -> PreparedMask {
    let (gw, gh) = geometry.frames().g_size_px();
    let long = gw.max(gh);
    let g_box = ((gw / long) as f32, (gh / long) as f32);
    let components = m
        .components
        .iter()
        .map(|c| PreparedComponent {
            mode: c.mode,
            invert: c.invert,
            shape: prepare_shape(&c.shape, g_box),
        })
        .collect();
    let delta = match slots {
        Some(slots) => {
            let values = m.delta.values();
            let mut d = vec![0.0f32; slots.count()];
            for p in LocalParam::ALL {
                if let Some(k) = slots.plane(p) {
                    d[k] = values[p.index()];
                }
            }
            d
        }
        None => Vec::new(),
    };
    PreparedMask {
        initial: m.components.first().map_or(0.0, |c| c.mode.initial()),
        components,
        delta,
    }
}

fn prepare_shape(shape: &MaskShape, g_box: (f32, f32)) -> PreparedShape {
    match shape {
        MaskShape::Linear { x0, y0, x1, y1 } => {
            let (dx, dy) = (
                f64::from(*x1) - f64::from(*x0),
                f64::from(*y1) - f64::from(*y0),
            );
            let len2 = dx * dx + dy * dy;
            PreparedShape::Linear {
                x0: *x0,
                y0: *y0,
                dx: dx as f32,
                dy: dy as f32,
                inv_len2: if len2 > 1e-12 {
                    (1.0 / len2) as f32
                } else {
                    0.0
                },
            }
        }
        MaskShape::Radial {
            cx,
            cy,
            rx,
            ry,
            angle_deg,
            feather,
        } => {
            let t = f64::from(*angle_deg).to_radians();
            PreparedShape::Radial {
                cx: *cx,
                cy: *cy,
                cos: t.cos() as f32,
                sin: t.sin() as f32,
                inv_rx: (1.0 / f64::from(*rx).max(1e-6)) as f32,
                inv_ry: (1.0 / f64::from(*ry).max(1e-6)) as f32,
                inner: 1.0 - feather / 100.0,
            }
        }
        MaskShape::Brush { strokes } => PreparedShape::Brush(brush::raster_for(strokes, g_box)),
        MaskShape::ColorRange { samples, amount } => {
            let hi = COLOR_RANGE_MIN + (COLOR_RANGE_MAX - COLOR_RANGE_MIN) * amount / 100.0;
            PreparedShape::Color {
                targets: samples.iter().map(|s| sample_chroma(s.lab)).collect(),
                lo: COLOR_RANGE_INNER * hi,
                hi,
            }
        }
        MaskShape::LuminanceRange {
            min,
            max,
            smoothness,
        } => PreparedShape::Luminance {
            lo: min / 100.0,
            hi: max / 100.0,
            smooth: smoothness / 100.0 * LUMA_RANGE_SMOOTH_MAX,
        },
    }
}

/// マスク 1 つの重みの画像（重ねて表示（O キー）・CLI の確かめ用。差分・有効かによらない）。`reference` は基準の画像
/// （C の格子）、`exposure_ev` は全体の露光量、`input` は入力の種類。値は 0〜1（行優先）。
pub fn mask_weights(
    m: &LocalAdjustment,
    geometry: &GeometryPlan,
    reference: &ImageTile,
    exposure_ev: f32,
    input: crate::InputKind,
) -> Vec<f32> {
    let mask = prepare_mask(m, geometry, None);
    let (origin, scale) = g_mapping(geometry);
    let gain = f64::from(exposure_ev).exp2() as f32;
    let tone = SceneToDisplayParams::for_input(input);
    let uses_range = mask.uses_range();
    let roi = reference.roi;
    let w = roi.width as usize;
    let src = reference.image.data();
    let mut out = vec![0.0f32; roi.pixel_count() as usize];
    out.par_chunks_mut(w.max(1))
        .enumerate()
        .for_each(|(j, row)| {
            let y = i64::from(roi.y) + j as i64;
            for (i, o) in row.iter_mut().enumerate() {
                let (u, v) = g_coords(origin, scale, i64::from(roi.x) + i as i64, y);
                let range = if uses_range {
                    range_inputs(src[j * w + i], gain, &tone)
                } else {
                    RangeInputs {
                        chroma: [0.0; 2],
                        luma: 0.0,
                    }
                };
                *o = mask.weight(u, v, &range);
            }
        });
    out
}

/// 局所の色温度・色かぶり（スライダーの値。重なったマスクの和）の、作業色空間の RGB の倍率（中立色の輝度を保つ）。
/// 和は ±100 に収める（モジュールの doc の「値の範囲」。[`add_unit`] の式）。
///
/// ```text
/// t = clamp(色温度 / 100, −1, 1)、 k = clamp(色かぶり / 100, −1, 1)
/// l = (T·t + M/2·k, −M·k, −T·t + M/2·k)        （T = LOCAL_TEMPERATURE_EV、M = LOCAL_TINT_EV）
/// e = (exp2(l_R), exp2(l_G), exp2(l_B))、 y = 輝度(e)、 倍率 = e / y
/// ```
#[inline]
pub fn white_balance_gains(temperature: f32, tint: f32) -> [f32; 3] {
    let t = add_unit(0.0, temperature, -1.0, 1.0);
    let k = add_unit(0.0, tint, -1.0, 1.0);
    let half = 0.5 * LOCAL_TINT_EV * k;
    let e = [
        (LOCAL_TEMPERATURE_EV * t + half).exp2(),
        (-LOCAL_TINT_EV * k).exp2(),
        (-LOCAL_TEMPERATURE_EV * t + half).exp2(),
    ];
    let y = luminance(e);
    [e[0] / y, e[1] / y, e[2] / y]
}

#[cfg(test)]
mod tests;
