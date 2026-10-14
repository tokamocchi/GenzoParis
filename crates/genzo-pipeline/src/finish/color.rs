//! ステージ 13: 色（彩度・自然な彩度（DEV-08）、HSL / カラーミキサー（DEV-09）、カラーグレーディング（DEV-10）、
//! 白黒（DEV-25）。docs/04_architecture.md の 2.1 節）。
//!
//! リニアな値（B2）を **OKLab**（genzo-color の [`linear_bt2020_to_oklab`]。知覚的な色空間）に変換し、明るさ L と
//! 無彩色からの差（色の成分）を調整して、リニアに戻す。色相は OKLCh の色相（無彩色の軸からの差の向き）で測る。
//!
//! | 項目 | 処理バージョン 1 | 処理バージョン 2 |
//! |---|---|---|
//! | 彩度・自然な彩度 | 適用する | 適用する（白黒のときは使わない） |
//! | カラーミキサー（`color.mixer`） | 適用しない（値があれば知らせる。[`super::unimplemented_settings`]） | 適用する（白黒のときは使わない） |
//! | 白黒（`color.black_white`） | 適用しない（同上） | 適用する（白黒ミキサー） |
//! | カラーグレーディング（`color.grading`） | 適用しない（同上） | 適用する（カラー・白黒のどちらでも） |
//!
//! v1 の機能の項目が既定値（ミキサー 0・白黒でない・グレーディングの強さと輝度 0）なら、版 2 は版 1 とビット単位で
//! 同じ結果になる（同じ式・同じ演算の順。`tests/engine_golden.rs` で確かめる）。
//!
//! # 計算（画素ごと。f32、この順序。値は仮置き）
//!
//! ```text
//! (L, a, b) = OKLab(RGB)                      （立方根は符号付き: 負の値（色域外）でも NaN にならない）
//! (da, db) = (a − L·na, b − L·nb)             （無彩色の軸からの差。下の「無彩色の軸」）
//!
//! 白黒でないとき:
//!   カラーミキサー（ある場合）: C = sqrt(da·da + db·db)。C > 0 なら
//!     g = ramp(C)、(k, s) = band(hue(da, db))                   （下の「色相の帯」）
//!     θ = mix(H, k, s)·g、ks = 1 + mix(S, k, s)·g、e = mix(Lm, k, s)·g
//!     θ ≠ 0 なら (da, db) を θ だけ回す（sin・cos は下の多項式）
//!     (da, db) = (da·ks, db·ks)
//!     e ≠ 0 なら m = exp2(e)、(L, da, db) = (L·m, da·m, db·m)   （その色の画素の露光量を変えるのと同じ）
//!   彩度・自然な彩度（どちらかが 0 でない場合）: k = chroma_gain(da, db)、(da, db) = (da·k, db·k)
//! 白黒のとき:
//!   白黒ミキサー（どれかが 0 でない場合）: C > 0 なら g・(k, s) を上と同じく求め、e = mix(W, k, s)·g、
//!     e ≠ 0 なら L = L·exp2(e)
//!   (da, db) = (0, 0)                         （同じ L の灰色）
//! カラーグレーディング（ある場合）: (ws, wm, wh) = zones(L)（下の「範囲の重み」）
//!   ta = ws·T[0].a + wm·T[1].a + wh·T[2].a + T[3].a、tb も同じ、e = ws·E[0] + wm·E[1] + wh·E[2] + E[3]
//!   (da, db) = (da + L·ta, db + L·tb)        （明るさに比例した色付け。黒は黒のまま）
//!   e ≠ 0 なら m = exp2(e)、(L, da, db) = (L·m, da·m, db·m)
//! RGB' = OKLab⁻¹(L, L·na + da, L·nb + db)
//! ```
//!
//! - `mix(v, k, s) = v[k] + (v[(k + 1) mod 8] − v[k])·s`。H は回す角度（ラジアン。スライダー / 100 ×
//!   [`HUE_SHIFT_MAX_DEG`]）、S はスライダー / 100、Lm・W・E は OKLab の L の倍率の 2 を底とする指数
//!   （スライダー / 100 × 最大の段数 / 3。L を `2^(EV/3)` 倍すると、リニアの値は `2^EV` 倍になる）。
//! - `ramp(C) = r·r·(3 − 2r)`（`r = min(C / HSL_CHROMA_RAMP, 1)`）: **無彩色に近い画素で色相が暴れないように**、
//!   彩度の低い画素ほどミキサーの効きを弱める（雑音の色相がばらばらでも、明るさのまだらにならない）。
//! - 彩度・自然な彩度の `chroma_gain` は DEV-08 の式（下の「彩度・自然な彩度」）。
//!
//! **色相の帯**: 8 つの帯の中心 `c[0..8]`（[`HUE_BAND_CENTERS_DEG`] をラジアンにした値。`c[8] = c[0] + 2π`）の
//! 間を、隣り合う 2 つの帯の重みの和が 1 になるように **smoothstep でつなぐ**（帯の境目で値も傾きも連続）。
//!
//! ```text
//! h = hue(da, db)（0〜2π）。h < c[0] なら h += 2π
//! k = c[k] ≤ h となる最大の k（0〜7）、t = clamp((h − c[k])·inv[k], 0, 1)（inv[k] = 1 / (c[k+1] − c[k])）
//! s = t·t·(3 − 2t)                            （帯 k の重み 1 − s、帯 k + 1 の重み s）
//! ```
//!
//! **色相の角度 `hue(da, db)`**: GPU の `atan2` の精度は環境によるので、CPU 版と GPU 版で同じ多項式を使う。
//!
//! ```text
//! (ax, ay) = (|da|, |db|)、t = min(ax, ay) / max(ax, ay)（max が 0 なら 0 を返す）
//! t > tan(π/12) なら u = (t·√3 − 1) / (t + √3)、b0 = π/6。そうでなければ u = t、b0 = 0
//! u2 = u·u、q = 1/9 − u2/11、q = u2·q − 1/7、q = u2·q + 1/5、q = u2·q − 1/3
//! a = b0 + (u + (u·u2)·q)                     （atan(t)。|u| ≤ tan(π/12) のテイラー展開で、誤差 1e−8 程度）
//! ay > ax なら a = π/2 − a、da < 0 なら a = π − a、db < 0 なら a = 2π − a
//! ```
//!
//! **回転の sin・cos**（|θ| ≤ [`HUE_SHIFT_MAX_DEG`] のテイラー展開。打ち切りの誤差は 1e−9 以下）:
//!
//! ```text
//! t2 = θ·θ
//! sin = θ + (θ·t2)·(−1/6 + t2·(1/120 + t2·(−1/5040 + t2·(1/362880))))
//! cos = 1 + t2·(−1/2 + t2·(1/24 + t2·(−1/720 + t2·(1/40320))))
//! (da', db') = (da·cos − db·sin, da·sin + db·cos)
//! ```
//!
//! **範囲の重み `zones(L)`**（カラーグレーディング）: 明るさの位置 `x`（0〜1）を、OKLab の L から
//! 「表示の中間グレーになる値の L（`Lp`）を 0.5、シーンの白（L ≒ 1）を 1」とする折れ線で求める（`Lp` は、RAW では
//! シーンの `0.18 / 2^1.3`（ステージ 15 で表示の 0.18 になる値）、RAW 以外では 0.18 の L）。境目 `x0 = 0.5 −
//! バランス / 100 × GRADING_BALANCE_SHIFT`、なめらかさ `σ = 1 / (GRADING_MIN_BLEND_WIDTH + ブレンド / 100 ×
//! (1 − GRADING_MIN_BLEND_WIDTH))` として:
//!
//! ```text
//! x = L·(0.5 / Lp)（L ≤ Lp）、0.5 + (L − Lp)·(0.5 / (Lw − Lp))（L > Lp）、clamp(x, 0, 1)
//! step(t) = u·u·(3 − 2u)、u = clamp((t − 0.5)·σ + 0.5, 0, 1)
//! x < x0: m = step(x / x0)、(ws, wm, wh) = (1 − m, m, 0)
//! x ≥ x0: h = step((x − x0) / (1 − x0))、(ws, wm, wh) = (0, 1 − h, h)
//! ```
//!
//! 3 つの重みの和は常に 1（シャドウ → 中間調 → ハイライトの順になめらかにつながる）。ブレンドが大きいほど隣の範囲と
//! 広く重なる。色付けの向き `T` は、色相環の色（sRGB の HSV の色相・彩度 1・明度 1 の色）の OKLCh の色相の
//! 単位ベクトルに、強さ / 100 × [`GRADING_CHROMA_MAX`] を掛けたもの（f64 で求めて f32 に丸める）。
//!
//! **無彩色の軸**: OKLab の行列（Ottosson の公表値）と BT.2020 の白（D65）の定義の丸めの違いで、
//! 作業色空間の無彩色 (g, g, g) の OKLab は a・b がちょうど 0 にならない（約 1e−4）。無彩色は
//! `(L, L·na, L·nb)`（`(na, nb) = (a, b) / L` を白 (1, 1, 1) で求めたもの。[`ColorStageParams::neutral`]）
//! の直線上にあるので、彩度・色相はこの軸からの差で測り、倍率もこの軸に向かって掛ける。こうすると、
//! 彩度 −100 と白黒の結果は R = G = B（丸めの誤差の範囲）になる。
//!
//! # 彩度・自然な彩度（DEV-08）
//!
//! ```text
//! C   = sqrt(da·da + db·db)                   （彩度。hypot は使わない: 結果が環境によらないように）
//! ks  = 1 + saturation                        （saturation は −1〜1。−1 で無彩色、+1 で 2 倍）
//! wc  = (1 − min(C / VIBRANCE_CHROMA_REF, 1))²          （彩度の低い色ほど 1 に近い）
//! skin = C > 0 かつ vibrance > 0 のとき:
//!          cosΔ = (da·cos h0 + db·sin h0) / C          （肌色の色相 h0 との角度の余弦）
//!          t = max(0, (cosΔ − cos W) / (1 − cos W))、 skin = t·t
//!        それ以外は 0
//! kv  = 1 + vibrance · VIBRANCE_MAX · wc · (1 − SKIN_PROTECTION · skin)
//! chroma_gain = ks · kv
//! ```
//!
//! - 自然な彩度は、彩度の低い色ほど強く効き（`wc`）、彩度の高い色（`C ≥ VIBRANCE_CHROMA_REF`）は
//!   変えない。上げるとき（vibrance > 0）は、肌色の色相（OKLab で 40〜75° 付近）の効きを弱める
//!   （`SKIN_PROTECTION`）。下げるときは肌色も同じように下げる。
//! - 倍率はどれも 0 以上なので、色相を反転させない。明るさ L を保つので、彩度を 0 にすると同じ
//!   L の無彩色になる（OKLab の L は知覚的な明るさで、輝度 Y とは少し違う）。

use std::f64::consts::TAU;

use genzo_color::oklab::{linear_bt2020_to_oklab, oklab_to_linear_bt2020, xyz_d65_to_oklab};
use genzo_color::space::{BT2020_TO_XYZ, SRGB_TO_XYZ};
use genzo_color::transfer::srgb_decode;
use genzo_model::{ColorGrading, ColorMixer, ColorParams, DevelopSettings, HueBands, Phase};

use super::map_pixels;
use super::scene_to_display::{SCENE_TO_DISPLAY_MIDDLE_GRAY, SCENE_TO_DISPLAY_MIDDLE_GRAY_EV};
use crate::contract::ColorContract;
use crate::error::Result;
use crate::image::{ImageTile, Roi};
use crate::stage::{InputKind, Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// 自然な彩度 +100 で、無彩色に近い色の彩度を何倍増やすか（`1 + VIBRANCE_MAX` 倍）。仮置き（PoC-4）。
pub const VIBRANCE_MAX: f32 = 1.0;
/// 自然な彩度が効かなくなる彩度（OKLab の C）。仮置き: sRGB の原色の C が 0.2〜0.32 程度なので、
/// それより少し低い 0.25 にした（鮮やかな色は変えない）。PoC-4 で決める。
pub const VIBRANCE_CHROMA_REF: f32 = 0.25;
/// 肌色の色相の中心（OKLab の色相。度）。仮置き: ColorChecker の dark skin・light skin（約 44〜46°）と、
/// 明るい黄みの肌（約 72°）の中間。
pub const SKIN_HUE_DEG: f64 = 55.0;
/// 肌色とみなす色相の半幅（度）。この角度で保護が 0 になる。仮置き。
pub const SKIN_HUE_HALF_WIDTH_DEG: f64 = 35.0;
/// 肌色の保護の強さ（0〜1。1 で肌色の中心では自然な彩度を上げない）。仮置き。
pub const SKIN_PROTECTION: f32 = 0.7;

/// カラーミキサー・白黒ミキサーの 8 つの色相の帯の中心（OKLCh の色相。度。genzo-model の `HueBand` の順）。
///
/// sRGB の色相環（HSV の色相）の 0°・30°・60°・120°・180°・240°・270°・300°（彩度 1・明度 1）の色の OKLCh の
/// 色相（29.2°・52.8°・109.8°・142.5°・194.8°・264.1°・293.8°・328.4°。テストで確かめる）を 1° に丸めた値。
/// 画面の帯の色（sRGB の色）と、効く色がそろうようにした。Lightroom Classic の帯の中心は公開されていない（仮置き）。
pub const HUE_BAND_CENTERS_DEG: [f64; 8] = [29.0, 53.0, 110.0, 142.0, 195.0, 264.0, 294.0, 328.0];

/// カラーミキサーの色相 ±100 で回す角度（OKLCh の色相。度）。仮置き: 隣の帯までの間隔（24〜69°）の半分前後。
pub const HUE_SHIFT_MAX_DEG: f64 = 30.0;
/// カラーミキサーの輝度 ±100 の明るさの変化（段。その色の画素のリニアの値を `2^(±1)` 倍）。仮置き。
pub const HSL_LUMINANCE_MAX_EV: f64 = 1.0;
/// 白黒ミキサー ±100 の明るさの変化（段）。仮置き（白黒の明暗を作る主な道具なので、カラーミキサーより広い）。
pub const BW_MIX_MAX_EV: f64 = 1.5;
/// カラーミキサー・白黒ミキサーが十分に効く彩度（OKLab の C）。これより彩度の低い画素は、無彩色に近いほど効きを
/// 弱める（`ramp`）。仮置き: 暗部の雑音の色の大きさ（C が 0.01 前後）より大きく、肌色（C が 0.05〜0.07）より小さい値。
pub const HSL_CHROMA_RAMP: f32 = 0.03;

/// カラーグレーディングの強さ 100 で足す色の大きさ（OKLab の C を L で割った値）。仮置き: sRGB のオレンジ（約 0.26）・
/// アクア（約 0.17）の C / L の間。
pub const GRADING_CHROMA_MAX: f64 = 0.2;
/// カラーグレーディングの輝度 ±100 の明るさの変化（段）。仮置き。
pub const GRADING_LUMINANCE_MAX_EV: f64 = 1.0;
/// カラーグレーディングのバランス ±100 で境目 `x0` を 0.5 から動かす幅（`x0` は 0.1〜0.9）。仮置き。
pub const GRADING_BALANCE_SHIFT: f64 = 0.4;
/// カラーグレーディングのブレンド 0 のときの、範囲の切り替わりの幅（範囲の間の長さに対する割合）。仮置き。
pub const GRADING_MIN_BLEND_WIDTH: f64 = 0.1;

/// `hue` の多項式の定数（f64 の値を f32 に丸めたもの。GPU 版にも同じ値を渡す）。
pub mod poly {
    /// `tan(π/12)`。
    pub const TAN_PI_12: f32 = 0.267_949_192_431_122_7_f64 as f32;
    /// `√3`。
    pub const SQRT3: f32 = 1.732_050_807_568_877_2_f64 as f32;
    /// `π/6`。
    pub const FRAC_PI_6: f32 = std::f64::consts::FRAC_PI_6 as f32;
    /// `π/2`。
    pub const FRAC_PI_2: f32 = std::f64::consts::FRAC_PI_2 as f32;
    /// `π`。
    pub const PI: f32 = std::f64::consts::PI as f32;
    /// `2π`。
    pub const TAU: f32 = std::f64::consts::TAU as f32;
    /// `1/3`。
    pub const INV3: f32 = (1.0f64 / 3.0) as f32;
    /// `1/5`。
    pub const INV5: f32 = (1.0f64 / 5.0) as f32;
    /// `1/7`。
    pub const INV7: f32 = (1.0f64 / 7.0) as f32;
    /// `1/9`。
    pub const INV9: f32 = (1.0f64 / 9.0) as f32;
    /// `1/11`。
    pub const INV11: f32 = (1.0f64 / 11.0) as f32;
    /// `1/6`。
    pub const INV6: f32 = (1.0f64 / 6.0) as f32;
    /// `1/120`。
    pub const INV120: f32 = (1.0f64 / 120.0) as f32;
    /// `1/5040`。
    pub const INV5040: f32 = (1.0f64 / 5040.0) as f32;
    /// `1/362880`。
    pub const INV362880: f32 = (1.0f64 / 362_880.0) as f32;
    /// `1/24`。
    pub const INV24: f32 = (1.0f64 / 24.0) as f32;
    /// `1/720`。
    pub const INV720: f32 = (1.0f64 / 720.0) as f32;
    /// `1/40320`。
    pub const INV40320: f32 = (1.0f64 / 40_320.0) as f32;
    /// `1 / HSL_CHROMA_RAMP`。
    pub const INV_CHROMA_RAMP: f32 = 1.0 / super::HSL_CHROMA_RAMP;
}

/// `atan(t)`（`t` は 0〜1。モジュールの doc の多項式）。
#[inline]
fn atan_unit(t: f32) -> f32 {
    let (u, base) = if t > poly::TAN_PI_12 {
        ((t * poly::SQRT3 - 1.0) / (t + poly::SQRT3), poly::FRAC_PI_6)
    } else {
        (t, 0.0)
    };
    let u2 = u * u;
    let mut q = poly::INV9 - u2 * poly::INV11;
    q = u2 * q - poly::INV7;
    q = u2 * q + poly::INV5;
    q = u2 * q - poly::INV3;
    base + (u + (u * u2) * q)
}

/// 無彩色の軸からの差 `(da, db)` の色相の角度（ラジアン。0〜2π。モジュールの doc の多項式。差が 0（と NaN）なら 0）。
#[inline]
pub fn hue_angle(da: f32, db: f32) -> f32 {
    let ax = da.abs();
    let ay = db.abs();
    let mx = ax.max(ay);
    let mn = ax.min(ay);
    // 差が 0（と NaN）なら 0。
    if mx.is_nan() || mx <= 0.0 {
        return 0.0;
    }
    let mut a = atan_unit(mn / mx);
    if ay > ax {
        a = poly::FRAC_PI_2 - a;
    }
    if da < 0.0 {
        a = poly::PI - a;
    }
    if db < 0.0 {
        a = poly::TAU - a;
    }
    a
}

/// 小さな角度 `theta`（ラジアン。|θ| ≤ [`HUE_SHIFT_MAX_DEG`]）の `(sin, cos)`（モジュールの doc の多項式）。
#[inline]
pub fn small_sin_cos(theta: f32) -> (f32, f32) {
    let t2 = theta * theta;
    let mut qs = poly::INV362880 * t2 - poly::INV5040;
    qs = qs * t2 + poly::INV120;
    qs = qs * t2 - poly::INV6;
    let sin = theta + (theta * t2) * qs;
    let mut qc = poly::INV40320 * t2 - poly::INV720;
    qc = qc * t2 + poly::INV24;
    qc = qc * t2 - 0.5;
    let cos = 1.0 + t2 * qc;
    (sin, cos)
}

/// `ramp(C)`: 彩度の低い画素ほど 0 に近い重み（モジュールの doc）。
#[inline]
pub fn chroma_ramp(c: f32) -> f32 {
    let r = (c * poly::INV_CHROMA_RAMP).min(1.0);
    r * r * (3.0 - 2.0 * r)
}

/// 色相の帯の表（中心と、隣の中心までの間隔の逆数。ラジアン）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HueBandTable {
    /// 中心 `c[0..8]` と `c[8] = c[0] + 2π`（f64 で求めて f32 に丸めた値）。
    pub centers: [f32; 9],
    /// `1 / (c[k+1] − c[k])`（f64 で求めて f32 に丸めた値）。
    pub inv_widths: [f32; 8],
}

impl HueBandTable {
    /// [`HUE_BAND_CENTERS_DEG`] の表。
    pub fn standard() -> Self {
        let c64: [f64; 9] = std::array::from_fn(|i| {
            if i < 8 {
                HUE_BAND_CENTERS_DEG[i].to_radians()
            } else {
                HUE_BAND_CENTERS_DEG[0].to_radians() + TAU
            }
        });
        Self {
            centers: c64.map(|v| v as f32),
            inv_widths: std::array::from_fn(|i| (1.0 / (c64[i + 1] - c64[i])) as f32),
        }
    }

    /// 色相 `h`（ラジアン。0〜2π）の帯 `k` と、帯 `k + 1` の重み `s`（モジュールの doc の「色相の帯」）。
    #[inline]
    pub fn locate(&self, h: f32) -> (usize, f32) {
        let c = &self.centers;
        let h = if h < c[0] { h + poly::TAU } else { h };
        let mut k = 0;
        for (i, ci) in c.iter().enumerate().take(8).skip(1) {
            if h >= *ci {
                k = i;
            }
        }
        let t = ((h - c[k]) * self.inv_widths[k]).clamp(0.0, 1.0);
        (k, t * t * (3.0 - 2.0 * t))
    }
}

/// 帯ごとの値 `v` を、帯 `k` と `k + 1` の間の重み `s` で混ぜる（`v[k] + (v[k+1] − v[k])·s`）。
#[inline]
pub fn band_mix(v: &[f32; 8], k: usize, s: f32) -> f32 {
    let a = v[k];
    a + (v[(k + 1) & 7] - a) * s
}

/// カラーミキサー（HSL）のパラメータ（帯の順。モジュールの doc の H・S・Lm）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixerParams {
    /// 色相の回す角度（ラジアン。スライダー / 100 × [`HUE_SHIFT_MAX_DEG`]）。
    pub hue: [f32; 8],
    /// 彩度の倍率から 1 を引いた値（スライダー / 100）。
    pub saturation: [f32; 8],
    /// L の倍率の 2 を底とする指数（スライダー / 100 × [`HSL_LUMINANCE_MAX_EV`] / 3）。
    pub luminance: [f32; 8],
}

impl MixerParams {
    /// 設定から作る（f64 で求めて f32 に丸める）。
    pub fn new(mixer: &ColorMixer) -> Self {
        let mut p = Self {
            hue: [0.0; 8],
            saturation: [0.0; 8],
            luminance: [0.0; 8],
        };
        for (i, a) in mixer.values().iter().enumerate() {
            p.hue[i] = (f64::from(a.hue) / 100.0 * HUE_SHIFT_MAX_DEG.to_radians()) as f32;
            p.saturation[i] = (f64::from(a.saturation) / 100.0) as f32;
            p.luminance[i] = (f64::from(a.luminance) / 100.0 * HSL_LUMINANCE_MAX_EV / 3.0) as f32;
        }
        p
    }
}

/// 白黒ミキサーのパラメータ（帯の順。L の倍率の 2 を底とする指数。スライダー / 100 × [`BW_MIX_MAX_EV`] / 3）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlackWhiteParams {
    /// 帯ごとの指数。
    pub mix: [f32; 8],
    /// どれかの帯が 0 でないか（すべて 0 なら色相を求めない）。
    pub active: bool,
}

impl BlackWhiteParams {
    /// 設定から作る。
    pub fn new(mix: &HueBands<f32>) -> Self {
        let mut m = [0.0f32; 8];
        for (i, v) in mix.values().iter().enumerate() {
            m[i] = (f64::from(*v) / 100.0 * BW_MIX_MAX_EV / 3.0) as f32;
        }
        Self {
            mix: m,
            active: m.iter().any(|v| *v != 0.0),
        }
    }
}

/// カラーグレーディングのパラメータ（モジュールの doc の「範囲の重み」と T・E）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradingParams {
    /// 色付けの向きと大きさ `(ta, tb)`（シャドウ・中間調・ハイライト・全体の順。L に掛けて足す）。
    pub tint: [[f32; 2]; 4],
    /// L の倍率の 2 を底とする指数（同じ順。スライダー / 100 × [`GRADING_LUMINANCE_MAX_EV`] / 3）。
    pub ev: [f32; 4],
    /// 表示の中間グレーになる値の L（`Lp`）。
    pub pivot: f32,
    /// `0.5 / Lp`。
    pub inv_lower: f32,
    /// `0.5 / (Lw − Lp)`（`Lw` はシーンの白 1 の L）。
    pub inv_upper: f32,
    /// 境目 `x0`。
    pub x0: f32,
    /// `1 / x0`。
    pub inv_x0: f32,
    /// `1 / (1 − x0)`。
    pub inv_x1: f32,
    /// なめらかさ `σ`。
    pub sharpness: f32,
}

/// 灰色（リニアの値 `y`）の OKLab の L（f64）。
fn gray_lightness(y: f64) -> f64 {
    xyz_d65_to_oklab(BT2020_TO_XYZ.apply([y; 3]))[0]
}

/// 色相環の色相（度。sRGB の HSV の色相）の色の、無彩色の軸からの向き（OKLCh の色相。ラジアン。f64）。
pub fn wheel_hue_to_oklab_angle(hue_deg: f64) -> f64 {
    let h = hue_deg.rem_euclid(360.0) / 60.0;
    // HSV（彩度 1・明度 1）→ sRGB の符号化した値。
    let x = 1.0 - ((h % 2.0) - 1.0).abs();
    let rgb = match h as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    };
    let lin = rgb.map(srgb_decode);
    let [l, a, b] = xyz_d65_to_oklab(SRGB_TO_XYZ.apply(lin));
    let [lw, aw, bw] = xyz_d65_to_oklab(SRGB_TO_XYZ.apply([1.0; 3]));
    let (na, nb) = (aw / lw, bw / lw);
    (b - l * nb).atan2(a - l * na)
}

impl GradingParams {
    /// 設定と入力の種類（表示の中間グレーになる値が違う）から作る（f64 で求めて f32 に丸める）。
    pub fn new(g: &ColorGrading, input: InputKind) -> Self {
        let mid = match input {
            InputKind::SceneReferred => {
                SCENE_TO_DISPLAY_MIDDLE_GRAY / 2f64.powf(SCENE_TO_DISPLAY_MIDDLE_GRAY_EV)
            }
            InputKind::DisplayReferred => SCENE_TO_DISPLAY_MIDDLE_GRAY,
        };
        let lp = gray_lightness(mid);
        let lw = gray_lightness(1.0);
        let mut tint = [[0.0f32; 2]; 4];
        let mut ev = [0.0f32; 4];
        for (i, w) in g.wheels().iter().enumerate() {
            let phi = wheel_hue_to_oklab_angle(f64::from(w.hue));
            let r = f64::from(w.saturation) / 100.0 * GRADING_CHROMA_MAX;
            tint[i] = [(r * phi.cos()) as f32, (r * phi.sin()) as f32];
            ev[i] = (f64::from(w.luminance) / 100.0 * GRADING_LUMINANCE_MAX_EV / 3.0) as f32;
        }
        let x0 = 0.5 - f64::from(g.balance) / 100.0 * GRADING_BALANCE_SHIFT;
        let width = GRADING_MIN_BLEND_WIDTH
            + f64::from(g.blending) / 100.0 * (1.0 - GRADING_MIN_BLEND_WIDTH);
        Self {
            tint,
            ev,
            pivot: lp as f32,
            inv_lower: (0.5 / lp) as f32,
            inv_upper: (0.5 / (lw - lp)) as f32,
            x0: x0 as f32,
            inv_x0: (1.0 / x0) as f32,
            inv_x1: (1.0 / (1.0 - x0)) as f32,
            sharpness: (1.0 / width) as f32,
        }
    }

    /// `step(t)`（モジュールの doc）。
    #[inline]
    fn step(&self, t: f32) -> f32 {
        let u = ((t - 0.5) * self.sharpness + 0.5).clamp(0.0, 1.0);
        u * u * (3.0 - 2.0 * u)
    }

    /// 明るさ `l` の範囲の重み `(ws, wm, wh)`（モジュールの doc の `zones(L)`）。
    #[inline]
    pub fn zones(&self, l: f32) -> [f32; 3] {
        let x = if l <= self.pivot {
            l * self.inv_lower
        } else {
            0.5 + (l - self.pivot) * self.inv_upper
        };
        let x = x.clamp(0.0, 1.0);
        if x < self.x0 {
            let m = self.step(x * self.inv_x0);
            [1.0 - m, m, 0.0]
        } else {
            let h = self.step((x - self.x0) * self.inv_x1);
            [0.0, 1.0 - h, h]
        }
    }
}

/// ステージ 13 のパラメータ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorStageParams {
    /// 彩度（−1〜1。スライダー / 100）。
    pub saturation: f32,
    /// 自然な彩度（−1〜1）。
    pub vibrance: f32,
    /// `cos h0`（f64 で計算して f32 に丸めた値）。
    pub skin_cos: f32,
    /// `sin h0`。
    pub skin_sin: f32,
    /// `cos W`。
    pub skin_cos_width: f32,
    /// 無彩色の軸 `(na, nb)`（白 (1, 1, 1) の OKLab の `(a / L, b / L)`。モジュールの doc）。
    pub neutral: (f32, f32),
    /// 色相の帯の表。
    pub bands: HueBandTable,
    /// カラーミキサー（処理バージョン 2 で、白黒でなく、どれかの帯が 0 でないとき）。
    pub mixer: Option<MixerParams>,
    /// 白黒（処理バージョン 2 で白黒のとき。彩度・自然な彩度・カラーミキサーは使わない）。
    pub black_white: Option<BlackWhiteParams>,
    /// カラーグレーディング（処理バージョン 2 で、どれかの範囲が何かするとき）。
    pub grading: Option<GradingParams>,
}

impl ColorStageParams {
    /// 彩度・自然な彩度のスライダーの値（−100〜100）から作る（カラーミキサー・白黒・カラーグレーディングなし）。
    pub fn from_sliders(saturation: f32, vibrance: f32) -> Self {
        let h0 = SKIN_HUE_DEG.to_radians();
        let [lw, aw, bw] = linear_bt2020_to_oklab([1.0; 3]);
        Self {
            saturation: saturation / 100.0,
            vibrance: vibrance / 100.0,
            skin_cos: h0.cos() as f32,
            skin_sin: h0.sin() as f32,
            skin_cos_width: SKIN_HUE_HALF_WIDTH_DEG.to_radians().cos() as f32,
            neutral: (aw / lw, bw / lw),
            bands: HueBandTable::standard(),
            mixer: None,
            black_white: None,
            grading: None,
        }
    }

    /// 丸めた設定の色の項目から作る。処理バージョン 1 では彩度・自然な彩度だけ（v1 の機能の項目は無視する）。
    /// `input` はカラーグレーディングの明るさの位置の基準に使う。
    pub fn from_color(color: &ColorParams, version: ProcessVersion, input: InputKind) -> Self {
        let mut p = Self::from_sliders(color.saturation, color.vibrance);
        match version {
            ProcessVersion::V1 => {}
            ProcessVersion::V2 => {
                if color.black_white.enabled {
                    p.black_white = Some(BlackWhiteParams::new(&color.black_white.mix));
                } else if !color.mixer.is_neutral() {
                    p.mixer = Some(MixerParams::new(&color.mixer));
                }
                if !color.grading.is_neutral() {
                    p.grading = Some(GradingParams::new(&color.grading, input));
                }
            }
        }
        p
    }

    /// 何もしないか。
    pub fn is_identity(&self) -> bool {
        self.saturation == 0.0
            && self.vibrance == 0.0
            && self.mixer.is_none()
            && self.black_white.is_none()
            && self.grading.is_none()
    }

    /// 彩度・自然な彩度を使うか（どちらかが 0 でなく、白黒でない）。
    #[inline]
    pub fn uses_saturation(&self) -> bool {
        self.black_white.is_none() && (self.saturation != 0.0 || self.vibrance != 0.0)
    }

    /// 無彩色からの差 `(da, db)` に掛ける倍率（モジュールの doc の `ks · kv`）。
    #[inline]
    pub fn chroma_gain(&self, da: f32, db: f32) -> f32 {
        let ks = 1.0 + self.saturation;
        if self.vibrance == 0.0 {
            return ks;
        }
        let c = (da * da + db * db).sqrt();
        let x = 1.0 - (c / VIBRANCE_CHROMA_REF).min(1.0);
        let wc = x * x;
        let skin = if self.vibrance > 0.0 && c > 0.0 {
            let cos_d = (da * self.skin_cos + db * self.skin_sin) / c;
            let t = ((cos_d - self.skin_cos_width) / (1.0 - self.skin_cos_width)).max(0.0);
            t * t
        } else {
            0.0
        };
        let kv = 1.0 + self.vibrance * VIBRANCE_MAX * wc * (1.0 - SKIN_PROTECTION * skin);
        ks * kv
    }

    /// 1 画素に適用する（モジュールの doc の順）。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let [mut l, a, b] = linear_bt2020_to_oklab(c);
        let (na, nb) = self.neutral;
        let mut da = a - l * na;
        let mut db = b - l * nb;
        if let Some(bw) = &self.black_white {
            if bw.active {
                let cc = (da * da + db * db).sqrt();
                if cc > 0.0 {
                    let g = chroma_ramp(cc);
                    let (k, s) = self.bands.locate(hue_angle(da, db));
                    let e = band_mix(&bw.mix, k, s) * g;
                    if e != 0.0 {
                        l *= e.exp2();
                    }
                }
            }
            da = 0.0;
            db = 0.0;
        } else {
            if let Some(m) = &self.mixer {
                let cc = (da * da + db * db).sqrt();
                if cc > 0.0 {
                    let g = chroma_ramp(cc);
                    let (k, s) = self.bands.locate(hue_angle(da, db));
                    let theta = band_mix(&m.hue, k, s) * g;
                    let gain = 1.0 + band_mix(&m.saturation, k, s) * g;
                    let e = band_mix(&m.luminance, k, s) * g;
                    if theta != 0.0 {
                        let (sn, cs) = small_sin_cos(theta);
                        let ra = da * cs - db * sn;
                        let rb = da * sn + db * cs;
                        da = ra;
                        db = rb;
                    }
                    da *= gain;
                    db *= gain;
                    if e != 0.0 {
                        let k = e.exp2();
                        l *= k;
                        da *= k;
                        db *= k;
                    }
                }
            }
            if self.saturation != 0.0 || self.vibrance != 0.0 {
                let k = self.chroma_gain(da, db);
                da *= k;
                db *= k;
            }
        }
        if let Some(gp) = &self.grading {
            let [ws, wm, wh] = gp.zones(l);
            let t = &gp.tint;
            let ta = ws * t[0][0] + wm * t[1][0] + wh * t[2][0] + t[3][0];
            let tb = ws * t[0][1] + wm * t[1][1] + wh * t[2][1] + t[3][1];
            let e = ws * gp.ev[0] + wm * gp.ev[1] + wh * gp.ev[2] + gp.ev[3];
            da += l * ta;
            db += l * tb;
            if e != 0.0 {
                let k = e.exp2();
                l *= k;
                da *= k;
                db *= k;
            }
        }
        oklab_to_linear_bt2020([l, l * na + da, l * nb + db])
    }
}

/// ステージ 13: 彩度・自然な彩度・カラーミキサー・カラーグレーディング・白黒（OKLab）。何もしなければ飛ばす。
#[derive(Debug, Clone, Copy, Default)]
pub struct ColorStage;

impl ColorStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.color";
    /// 04 の 2.1 節のステージの番号。
    pub const NUMBER: u8 = 13;
}

impl Stage for ColorStage {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn phase(&self) -> Phase {
        Phase::C
    }
    fn input_contract(&self) -> ColorContract {
        ColorContract::B2Working
    }
    fn output_contract(&self) -> ColorContract {
        ColorContract::B2Working
    }
    fn params(
        &self,
        settings: &DevelopSettings,
        ctx: &StageContext<'_>,
    ) -> Result<Option<StageParams>> {
        let c = settings.normalized().color;
        let p = ColorStageParams::from_color(&c, ctx.process_version, ctx.input_kind);
        Ok((!p.is_identity()).then(|| StageParams::new(p)))
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
        let p = *params.expect::<ColorStageParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 | ProcessVersion::V2 => {
                map_pixels(input, output, |_, _, c| p.apply(c))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use genzo_model::{HueBand, HueBands};

    use super::*;

    /// 角度の差（度。-180〜180）。
    fn angle_diff_deg(a: f64, b: f64) -> f64 {
        let d = (a - b).rem_euclid(360.0);
        if d > 180.0 { d - 360.0 } else { d }
    }

    /// 無彩色の軸からの差。
    fn delta(c: [f32; 3]) -> (f32, f32) {
        let n = ColorStageParams::from_sliders(0.0, 0.0).neutral;
        let [l, a, b] = linear_bt2020_to_oklab(c);
        (a - l * n.0, b - l * n.1)
    }

    fn chroma(c: [f32; 3]) -> f32 {
        let (a, b) = delta(c);
        (a * a + b * b).sqrt()
    }

    fn hue_deg(c: [f32; 3]) -> f64 {
        let (a, b) = delta(c);
        f64::from(b)
            .atan2(f64::from(a))
            .to_degrees()
            .rem_euclid(360.0)
    }

    /// sRGB の HSV の色相（彩度 1・明度 1）の色の作業色空間（BT.2020）のリニアの値。
    fn srgb_hue_color(hue_deg: f64) -> [f32; 3] {
        // 色相環の色そのもの（wheel_hue_to_oklab_angle と同じ変換）を作業色空間に移す。
        let h = hue_deg.rem_euclid(360.0) / 60.0;
        let x = 1.0 - ((h % 2.0) - 1.0).abs();
        let rgb = match h as u32 {
            0 => [1.0, x, 0.0],
            1 => [x, 1.0, 0.0],
            2 => [0.0, 1.0, x],
            3 => [0.0, x, 1.0],
            4 => [x, 0.0, 1.0],
            _ => [1.0, 0.0, x],
        };
        let xyz = SRGB_TO_XYZ.apply(rgb.map(srgb_decode));
        genzo_color::space::XYZ_TO_BT2020
            .apply(xyz)
            .map(|v| v as f32)
    }

    fn v2(color: &ColorParams) -> ColorStageParams {
        ColorStageParams::from_color(color, ProcessVersion::V2, InputKind::SceneReferred)
    }

    #[test]
    fn neutral_axis_is_close_to_zero_and_grays_are_unchanged() {
        let p = ColorStageParams::from_sliders(100.0, 100.0);
        assert!(p.neutral.0.abs() < 1e-3 && p.neutral.1.abs() < 1e-3);
        for g in [0.0f32, 1e-4, 0.18, 1.0, 5.0] {
            let out = p.apply([g; 3]);
            for v in out {
                assert!((v - g).abs() <= 1e-5 * g.max(1e-3), "{g} → {out:?}");
            }
        }
    }

    #[test]
    fn zero_saturation_gives_neutral_colors() {
        let p = ColorStageParams::from_sliders(-100.0, 0.0);
        for c in [
            [0.5, 0.2, 0.1],
            [0.05, 0.3, 0.6],
            [2.0, 1.0, 0.1],
            [0.2, -0.05, 0.1],
        ] {
            let out = p.apply(c);
            let m = out[1].abs().max(1e-3);
            assert!(
                (out[0] - out[1]).abs() < 1e-5 * m && (out[2] - out[1]).abs() < 1e-5 * m,
                "{c:?} → {out:?}"
            );
            // 明るさ L は保たれる。
            let l0 = linear_bt2020_to_oklab(c)[0];
            let l1 = linear_bt2020_to_oklab(out)[0];
            assert!((l0 - l1).abs() < 1e-4, "{l0} {l1}");
        }
    }

    #[test]
    fn saturation_scales_chroma_and_keeps_hue() {
        let p = ColorStageParams::from_sliders(50.0, 0.0);
        let c = [0.4, 0.2, 0.1];
        let out = p.apply(c);
        assert!((chroma(out) / chroma(c) - 1.5).abs() < 1e-3);
        let (a0, b0) = delta(c);
        let (a1, b1) = delta(out);
        assert!((a0 * b1 - a1 * b0).abs() < 1e-5, "色相が変わった");
        assert!(ColorStageParams::from_sliders(0.0, 0.0).is_identity());
    }

    #[test]
    fn vibrance_prefers_low_chroma_and_protects_skin() {
        let p = ColorStageParams::from_sliders(0.0, 100.0);
        // 彩度の低い青みのグレー（肌色の色相から遠い）。
        let dull = [0.2, 0.22, 0.26];
        // 鮮やかな色。
        let vivid = [0.8, 0.05, 0.05];
        let g_dull = chroma(p.apply(dull)) / chroma(dull);
        let g_vivid = chroma(p.apply(vivid)) / chroma(vivid);
        assert!(g_dull > 1.5, "{g_dull}");
        assert!((g_vivid - 1.0).abs() < 1e-3, "{g_vivid}");
        // 肌色（ColorChecker の light skin を BT.2020 にした程度の値）は、同じくらいの彩度の
        // 肌色でない色より効きが弱い。
        let skin = [0.40, 0.29, 0.23];
        let (a, b) = delta(skin);
        let c = (a * a + b * b).sqrt();
        let g_skin = p.chroma_gain(a, b);
        let g_other = p.chroma_gain(-c, 0.0);
        assert!(g_skin < g_other, "{g_skin} {g_other}");
        assert!(g_skin > 1.0);
        // 下げるときは保護しない（肌色も同じ割合で下がる）。
        let q = ColorStageParams::from_sliders(0.0, -100.0);
        assert!((q.chroma_gain(a, b) - q.chroma_gain(-c, 0.0)).abs() < 1e-6);
    }

    #[test]
    fn negative_values_never_produce_nan() {
        let vals = [-2.0f32, -0.5, -1e-6, 0.0, 1e-6, 0.3, 1.0, 5.0];
        let mut full = ColorParams {
            saturation: 30.0,
            vibrance: -60.0,
            ..Default::default()
        };
        full.mixer.red.hue = 100.0;
        full.mixer.blue.luminance = -100.0;
        full.mixer.green.saturation = 100.0;
        full.grading.shadows.saturation = 100.0;
        full.grading.highlights.luminance = 100.0;
        full.grading.global.luminance = -100.0;
        let mut bw = full;
        bw.black_white.enabled = true;
        bw.black_white.mix =
            HueBands::from_values([100.0, -100.0, 50.0, -50.0, 0.0, 100.0, -100.0, 20.0]);
        for p in [
            ColorStageParams::from_sliders(100.0, 100.0),
            ColorStageParams::from_sliders(-100.0, -100.0),
            ColorStageParams::from_sliders(30.0, -60.0),
            v2(&full),
            v2(&bw),
        ] {
            for &r in &vals {
                for &g in &vals {
                    for &b in &vals {
                        let out = p.apply([r, g, b]);
                        assert!(out.iter().all(|v| v.is_finite()), "{r} {g} {b} → {out:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn hue_band_centers_match_the_srgb_hue_wheel() {
        // 帯の中心は、sRGB の HSV の色相の色の OKLCh の色相を 1° に丸めた値（画面の帯の色とそろえる）。
        for (deg, center) in [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0]
            .into_iter()
            .zip(HUE_BAND_CENTERS_DEG)
        {
            let h = wheel_hue_to_oklab_angle(deg).to_degrees();
            eprintln!(
                "sRGB の色相 {deg}° → OKLCh の色相 {:.2}°",
                h.rem_euclid(360.0)
            );
            assert!(
                angle_diff_deg(h, center).abs() <= 0.5,
                "{deg}: {h} と {center}"
            );
            // 作業色空間に移した色の色相も同じ（無彩色の軸の差の範囲）。
            let c = srgb_hue_color(deg);
            assert!(angle_diff_deg(hue_deg(c), h).abs() < 0.05, "{deg}");
        }
        assert!(HUE_BAND_CENTERS_DEG.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(HueBand::ALL.len(), HUE_BAND_CENTERS_DEG.len());
    }

    #[test]
    fn hue_angle_matches_atan2() {
        let mut worst = 0.0f64;
        for i in 0..3600 {
            let t = f64::from(i) / 10.0;
            for r in [1e-6f64, 0.01, 0.3, 5.0] {
                let (a, b) = (
                    (t.to_radians().cos() * r) as f32,
                    (t.to_radians().sin() * r) as f32,
                );
                let h = f64::from(hue_angle(a, b));
                let e = f64::from(b).atan2(f64::from(a)).rem_euclid(TAU);
                let d = (h - e).abs().min(TAU - (h - e).abs());
                worst = worst.max(d);
                assert!((0.0..=TAU as f32 as f64).contains(&h), "{t} {r}: {h}");
            }
        }
        assert!(worst < 1e-6, "{worst}");
        // 軸の上・0・NaN。
        assert_eq!(hue_angle(1.0, 0.0), 0.0);
        assert!((hue_angle(0.0, 1.0) - poly::FRAC_PI_2).abs() < 1e-7);
        assert!((hue_angle(-1.0, 0.0) - poly::PI).abs() < 1e-7);
        assert!((hue_angle(0.0, -1.0) - 3.0 * poly::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(hue_angle(0.0, 0.0), 0.0);
        assert_eq!(hue_angle(f32::NAN, f32::NAN), 0.0);
        assert!(hue_angle(f32::NAN, 1.0).is_finite());
    }

    #[test]
    fn small_sin_cos_matches_the_library() {
        let max = HUE_SHIFT_MAX_DEG.to_radians();
        for i in -300..=300 {
            let t = (f64::from(i) / 300.0 * max) as f32;
            let (s, c) = small_sin_cos(t);
            assert!((f64::from(s) - f64::from(t).sin()).abs() < 2e-7, "{t}");
            assert!((f64::from(c) - f64::from(t).cos()).abs() < 2e-7, "{t}");
        }
        assert_eq!(small_sin_cos(0.0), (0.0, 1.0));
    }

    #[test]
    fn band_weights_are_a_smooth_partition() {
        let t = HueBandTable::standard();
        // 中心では、その帯の重みが 1。
        for (i, c) in HUE_BAND_CENTERS_DEG.iter().enumerate() {
            let (k, s) = t.locate(c.to_radians() as f32);
            let w = if k == i { 1.0 - s } else { s };
            assert!((w - 1.0).abs() < 1e-5, "{i}: k={k} s={s}");
        }
        // 0〜2π を回ると、重みは連続（隣の点との差が小さい）。
        let ones = |b: usize| {
            let mut v = [0.0f32; 8];
            v[b] = 1.0;
            v
        };
        for b in 0..8 {
            let v = ones(b);
            let mut prev: Option<f32> = None;
            for i in 0..=7200 {
                let h = (f64::from(i) / 7200.0 * TAU) as f32;
                let (k, s) = t.locate(h);
                let w = band_mix(&v, k, s);
                assert!((0.0..=1.0).contains(&w));
                if let Some(p) = prev {
                    assert!((w - p).abs() < 0.01, "帯 {b} の {h}: {p} → {w}");
                }
                prev = Some(w);
            }
        }
        // 重みの和は 1。
        for i in 0..1000 {
            let h = (f64::from(i) / 1000.0 * TAU) as f32;
            let (k, s) = t.locate(h);
            let sum: f32 = (0..8).map(|b| band_mix(&ones(b), k, s)).sum();
            assert!((sum - 1.0).abs() < 1e-6, "{h}: {sum}");
        }
    }

    #[test]
    fn mixer_changes_only_the_chosen_band() {
        let red = srgb_hue_color(0.0);
        let blue = srgb_hue_color(240.0);
        let gray = [0.18f32; 3];
        // ブルーの輝度 −100: 青は暗く（リニアで約 1/2）、赤と灰色は変わらない。
        let mut c = ColorParams::default();
        c.mixer.blue.luminance = -100.0;
        let p = v2(&c);
        let out = p.apply(blue);
        let y0 = super::super::luminance(blue);
        let y1 = super::super::luminance(out);
        assert!((y1 / y0 - 0.5).abs() < 0.01, "{y0} → {y1}");
        assert!((hue_deg(out) - hue_deg(blue)).abs() < 0.01);
        let r = p.apply(red);
        assert!(
            r.iter().zip(red).all(|(a, b)| (a - b).abs() < 1e-5),
            "赤は変わらない: {r:?}"
        );
        let g = p.apply(gray);
        assert!(g.iter().all(|v| (v - 0.18).abs() < 1e-6), "{g:?}");
        // レッドの色相 +100: 色相が +30° 回る（オレンジの側へ）。彩度・明るさは変わらない。
        let mut c = ColorParams::default();
        c.mixer.red.hue = 100.0;
        let out = v2(&c).apply(red);
        let d = angle_diff_deg(hue_deg(out), hue_deg(red));
        assert!((d - HUE_SHIFT_MAX_DEG).abs() < 0.2, "{d}");
        assert!((chroma(out) / chroma(red) - 1.0).abs() < 1e-3);
        // グリーンの彩度 −100: 緑は無彩色。
        let green = srgb_hue_color(120.0);
        let mut c = ColorParams::default();
        c.mixer.green.saturation = -100.0;
        let out = v2(&c).apply(green);
        assert!(chroma(out) < 3e-4, "{out:?}");
        // 帯の間（オレンジとイエローの間）では両方の帯が効く。
        let mid = srgb_hue_color(45.0);
        let mut c = ColorParams::default();
        c.mixer.orange.saturation = -100.0;
        let r1 = chroma(v2(&c).apply(mid)) / chroma(mid);
        c.mixer.yellow.saturation = -100.0;
        let r2 = chroma(v2(&c).apply(mid)) / chroma(mid);
        assert!(r1 > 0.05 && r1 < 0.95, "{r1}");
        assert!(r2 < 1e-3, "{r2}");
    }

    #[test]
    fn mixer_does_not_affect_near_neutral_pixels() {
        // 無彩色に近い画素（雑音のような小さい色の差）は、色相によらずほとんど変わらない（明るさのまだらにならない）。
        let mut c = ColorParams::default();
        for b in HueBand::ALL {
            *c.mixer.get_mut(b) = genzo_model::HslAdjustment {
                hue: 100.0,
                saturation: 100.0,
                luminance: if (b as usize).is_multiple_of(2) {
                    100.0
                } else {
                    -100.0
                },
            };
        }
        let p = v2(&c);
        for (i, d) in [
            [1e-4f32, 0.0, 0.0],
            [0.0, 1e-4, 0.0],
            [0.0, 0.0, 1e-4],
            [-1e-4, 1e-4, 0.0],
        ]
        .into_iter()
        .enumerate()
        {
            let px = [0.18 + d[0], 0.18 + d[1], 0.18 + d[2]];
            let out = p.apply(px);
            let y0 = super::super::luminance(px);
            let y1 = super::super::luminance(out);
            assert!((y1 / y0 - 1.0).abs() < 1e-3, "{i}: {y0} → {y1}");
        }
        assert!(chroma_ramp(0.0) == 0.0 && chroma_ramp(HSL_CHROMA_RAMP) == 1.0);
        assert!(chroma_ramp(HSL_CHROMA_RAMP * 0.1) < 0.03);
    }

    #[test]
    fn black_white_gives_grays_and_mix_changes_brightness_by_hue() {
        let mut c = ColorParams {
            saturation: 80.0,
            ..Default::default()
        };
        c.black_white.enabled = true;
        c.mixer.red.luminance = 100.0; // 白黒のときは使わない。
        let p = v2(&c);
        let red = srgb_hue_color(0.0);
        let blue = srgb_hue_color(240.0);
        for px in [red, blue, [0.3, 0.2, 0.1], [0.18; 3]] {
            let out = p.apply(px);
            let m = out[1].abs().max(1e-3);
            assert!(
                (out[0] - out[1]).abs() < 1e-5 * m && (out[2] - out[1]).abs() < 1e-5 * m,
                "{px:?} → {out:?}"
            );
            // 白黒ミキサーが 0 なら OKLab の L を保つ。
            let l0 = linear_bt2020_to_oklab(px)[0];
            let l1 = linear_bt2020_to_oklab(out)[0];
            assert!((l0 - l1).abs() < 1e-4, "{l0} {l1}");
        }
        // ブルー −100: 青い画素だけが暗い灰色になる（リニアで 2^−1.5 倍）。
        c.black_white.mix.blue = -100.0;
        let p = v2(&c);
        let base = v2(&ColorParams {
            black_white: genzo_model::BlackWhite {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let y = |px| super::super::luminance(px);
        let r = y(p.apply(blue)) / y(base.apply(blue));
        assert!((r - 2f32.powf(-1.5)).abs() < 0.01, "{r}");
        assert!((y(p.apply(red)) - y(base.apply(red))).abs() < 1e-6);
        assert!(!p.is_identity());
        // 白黒にしただけ（ミキサー 0）でも何かする。
        assert!(!base.is_identity());
    }

    #[test]
    fn grading_tints_shadows_and_highlights_and_keeps_black() {
        let mut c = ColorParams::default();
        c.grading.shadows = genzo_model::GradingWheel {
            hue: 240.0,
            saturation: 50.0,
            luminance: 0.0,
        };
        c.grading.highlights = genzo_model::GradingWheel {
            hue: 30.0,
            saturation: 50.0,
            luminance: 0.0,
        };
        let p = v2(&c);
        // 暗い灰色は青みに、明るい灰色はオレンジみになる。
        let dark = p.apply([0.005; 3]);
        let bright = p.apply([0.6; 3]);
        let blue_h = wheel_hue_to_oklab_angle(240.0).to_degrees();
        let orange_h = wheel_hue_to_oklab_angle(30.0).to_degrees();
        assert!(
            angle_diff_deg(hue_deg(dark), blue_h).abs() < 1.0,
            "{}",
            hue_deg(dark)
        );
        assert!(
            angle_diff_deg(hue_deg(bright), orange_h).abs() < 1.0,
            "{}",
            hue_deg(bright)
        );
        // 色の大きさは明るさに比例（C / L ≒ 強さ / 100 × 0.2 × 重み）。
        let l_bright = linear_bt2020_to_oklab(bright)[0];
        assert!(
            (chroma(bright) / l_bright - 0.1).abs() < 0.01,
            "{}",
            chroma(bright) / l_bright
        );
        // 黒は黒のまま。
        let black = p.apply([0.0; 3]);
        assert!(black.iter().all(|v| v.abs() < 1e-7), "{black:?}");
        // 強さ 0 の色相は効かない（何もしない）。
        let mut z = ColorParams::default();
        z.grading.midtones.hue = 120.0;
        assert!(v2(&z).is_identity());
    }

    #[test]
    fn grading_zones_partition_and_respond_to_balance_and_blending() {
        let g = ColorGrading::default();
        let p = GradingParams::new(&g, InputKind::SceneReferred);
        // 表示の中間グレーになる値の L（シーンの 0.0731 ≒ L 0.418）。
        assert!((p.pivot - 0.418).abs() < 0.002, "{}", p.pivot);
        let mut prev = [1.0f32, 0.0, 0.0];
        for i in 0..=1000 {
            let l = i as f32 / 1000.0 * 1.2;
            let w = p.zones(l);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6, "{l}: {w:?}");
            // シャドウは減り、ハイライトは増える。
            assert!(w[0] <= prev[0] + 1e-6 && w[2] >= prev[2] - 1e-6, "{l}");
            prev = w;
        }
        assert_eq!(p.zones(0.0), [1.0, 0.0, 0.0]);
        assert_eq!(p.zones(p.pivot), [0.0, 1.0, 0.0]);
        assert_eq!(p.zones(1.5), [0.0, 0.0, 1.0]);
        // バランスを正にすると、中間グレーの少し下でもハイライトが効く（境目が暗い側へ）。
        let q = GradingParams::new(
            &ColorGrading {
                balance: 100.0,
                ..g
            },
            InputKind::SceneReferred,
        );
        assert!(q.x0 < p.x0);
        assert!(q.zones(p.pivot)[2] > 0.2, "{:?}", q.zones(p.pivot));
        // ブレンド 0 は切り替わりが急、100 はなだらか。
        let sharp = GradingParams::new(
            &ColorGrading { blending: 0.0, ..g },
            InputKind::SceneReferred,
        );
        let soft = GradingParams::new(
            &ColorGrading {
                blending: 100.0,
                ..g
            },
            InputKind::SceneReferred,
        );
        let l = p.pivot * 0.2;
        assert_eq!(sharp.zones(l), [1.0, 0.0, 0.0]);
        assert!(soft.zones(l)[1] > 0.05);
        // RAW 以外は 0.18 が表示の中間グレー。
        let d = GradingParams::new(&g, InputKind::DisplayReferred);
        assert!((d.pivot - 0.565).abs() < 0.002, "{}", d.pivot);
    }

    #[test]
    fn grading_luminance_changes_exposure_of_the_zone() {
        let mut c = ColorParams::default();
        c.grading.global.luminance = 100.0;
        let p = v2(&c);
        for g in [0.01f32, 0.18, 0.8] {
            let out = p.apply([g; 3]);
            assert!((out[1] / g - 2.0).abs() < 1e-3, "{g} → {out:?}");
        }
        let mut c = ColorParams::default();
        c.grading.shadows.luminance = -100.0;
        let p = v2(&c);
        assert!((p.apply([0.001; 3])[1] / 0.001 - 0.5).abs() < 1e-3);
        assert!((p.apply([0.9; 3])[1] - 0.9).abs() < 1e-6);
    }

    #[test]
    fn version_1_ignores_v1_items() {
        let mut c = ColorParams {
            saturation: 20.0,
            ..Default::default()
        };
        c.mixer.red.hue = 50.0;
        c.grading.global.saturation = 30.0;
        c.black_white.enabled = true;
        let v1 = ColorStageParams::from_color(&c, ProcessVersion::V1, InputKind::SceneReferred);
        assert_eq!(v1, ColorStageParams::from_sliders(20.0, 0.0));
        let v2p = v2(&c);
        assert!(v2p.black_white.is_some() && v2p.grading.is_some() && v2p.mixer.is_none());
        // 既定の色の項目では、版 2 も版 1 と同じパラメータ（ビット単位で同じ結果）。
        let d = ColorParams {
            saturation: 40.0,
            vibrance: 30.0,
            ..Default::default()
        };
        assert_eq!(
            ColorStageParams::from_color(&d, ProcessVersion::V2, InputKind::SceneReferred),
            ColorStageParams::from_sliders(40.0, 30.0)
        );
        assert!(
            ColorStageParams::from_color(
                &ColorParams::default(),
                ProcessVersion::V2,
                InputKind::DisplayReferred
            )
            .is_identity()
        );
    }
}
