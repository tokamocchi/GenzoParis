//! 色温度・色かぶり補正（tint）⇔ カメラの WB の係数（DEV-03。04 の 2.1 節のステージ 3）。
//!
//! ## 色温度と tint の定義（Lightroom との互換は求めない）
//!
//! uv は CIE 1960 UCS。白の色度を、プランク軌跡の点 `P(T)` と、その点での軌跡の法線
//! （等色温度線の方向）`N(T)` で表す: `uv = P(T) + Duv · N(T)`。
//!
//! - **色温度 T**: 白の **相関色温度**（CIE の定義どおり、プランク軌跡の法線上にある点を同じ
//!   色温度とみなす）。`P(T)` は Krystek (1985) のプランク軌跡の uv の有理式近似
//!   （適用範囲は 1000〜15000 K。15000 K を超える範囲は外挿のため、高い色温度の値は近似的な
//!   目安になる）。
//! - **基準の白（tint = 0）**: 低い色温度ではプランク軌跡の点（Duv = 0）、高い色温度では
//!   **CIE の昼光軌跡**（CIE 15 の昼光の色度の式。適用範囲は 4000〜25000 K で、それを超える範囲は
//!   外挿）の点 `D(T)` の Duv（`(D(T) − P(T)) · N(T)`）とする。つまり、`D(T)` を色温度 T の
//!   等色温度線に射影した点で、`D(T)` とのずれは 10000 K 以下で 5e-5 未満（uv）。
//!   2 つの標準の式は 4000 K で一致しない（Duv の差は約 0.0028 = tint 約 8.3）ので、
//!   [`WB_REFERENCE_BLEND_START_K`]〜[`WB_REFERENCE_BLEND_END_K`]（3800〜4200 K。仮置き）の間で
//!   昼光の Duv に重み `w = s²(3 − 2s)`（`s` は区間の中の位置 0〜1。両端で傾き 0）を掛けて、なめらかに
//!   つなぐ（人の判断の表 No.17 の決定。以前は 4000 K で tint 約 8 に相当する色の跳びがあった）。
//!   3800〜4000 K では昼光の式を適用範囲の外へ少し外挿して使う。
//! - **tint**: 基準の白からの Duv のずれを [`TINT_DUV_PER_UNIT`] で割った値。**正の値は白を
//!   緑の側（Duv > 0、プランク軌跡より上）にずらす**。光源が緑に寄っているとみなして補正する
//!   ので、画像は緑が弱まりマゼンタ寄りになる（genzo-model の `WhiteBalance::Custom` の tint の
//!   向きと同じ）。
//! - 色温度を上げると光源を青寄りとみなすので、画像は暖色寄りになる。
//! - 低い色温度で tint を大きく正にすると、白の色度がスペクトル軌跡の外（実在しない色）に
//!   なりうる（概算では、2000 K では Duv が +0.01 程度でスペクトル軌跡に近づく）。白のカメラ RGB
//!   が 0 以下になる場合、係数は作れずエラーになる（[`wb_multipliers_from_temperature_tint`]）。
//!   [`wb_multipliers_from_temperature_tint_clamped`] は、そのような設定を係数を作れる範囲に丸める
//!   （人の判断の表 No.17。丸めたことは呼び出し側が警告で知らせる）。
//!
//! 法線をどの温度でもプランク軌跡のもの（等色温度線）にそろえているので、(T, tint) と白の色度は
//! 1 対 1 に対応し、逆算（係数 → 色温度・tint）は一意に決まる。
//!
//! ## 係数の計算
//!
//! 白の XYZ（Y = 1）を `cam_xyz`（XYZ（D65）→ カメラ RGB）でカメラ RGB にし、係数を
//! `1 / cam_rgb` とし、G が 1 になるよう正規化する。逆算は、係数の逆数（白のカメラ RGB）を
//! `cam_xyz` の逆行列で XYZ に戻し、色度から色温度（相関色温度、二分法）と tint を求める。

use crate::error::{ColorError, Result};
use crate::matrix::Mat3;
use crate::space::{Uv, Xy};

/// 色温度の下限（K）。genzo-model の `ranges::TEMPERATURE_K` と同じ値。
pub const WB_TEMPERATURE_MIN_K: f64 = 2000.0;
/// 色温度の上限（K）。genzo-model の `ranges::TEMPERATURE_K` と同じ値。
pub const WB_TEMPERATURE_MAX_K: f64 = 50000.0;
/// tint の下限。genzo-model の `ranges::TINT` と同じ値。
pub const WB_TINT_MIN: f64 = -150.0;
/// tint の上限。genzo-model の `ranges::TINT` と同じ値。
pub const WB_TINT_MAX: f64 = 150.0;
/// 基準の白をプランク軌跡から CIE の昼光軌跡へ切り替え始める色温度（K）。**仮置き**: CIE の昼光の
/// 式の適用範囲の下限（4000 K）の前後 200 K（人の判断の表 No.17 の例の値）。
pub const WB_REFERENCE_BLEND_START_K: f64 = 3800.0;
/// 基準の白が CIE の昼光軌跡になる色温度（K）。**仮置き**（[`WB_REFERENCE_BLEND_START_K`] を参照）。
pub const WB_REFERENCE_BLEND_END_K: f64 = 4200.0;
/// tint の 1 単位あたりの Duv（CIE 1960 uv）（仮置き）。
///
/// tint の範囲 ±150 が Duv ±0.05 に対応する。光源の色のずれを十分に含む広さとして選んだ
/// （実在の光源の Duv の分布は確認していない）。Lightroom の tint の目盛りとの対応も確認していない。
pub const TINT_DUV_PER_UNIT: f64 = 1.0 / 3000.0;

/// 逆算で色温度を探す範囲（K）。設定できる範囲（2000〜50000 K）より広く探し、範囲外の推定値も
/// エラーに含めて返せるようにする。
const SEARCH_MIN_K: f64 = 1000.0;
const SEARCH_MAX_K: f64 = 100_000.0;
/// 二分法の打ち切りの幅（mired）。
const BISECTION_TOLERANCE_MIRED: f64 = 1e-11;
/// 色温度の範囲の判定の許容差（相対値）。丸め誤差で範囲の端をわずかに越えた値を許す。
const TEMPERATURE_RANGE_SLACK: f64 = 1e-6;
/// tint の範囲の判定の許容差。丸め誤差と、CIE の昼光の式が 7000 K で 2 つの式に分かれる
/// ことによる小さな段差（x で 1e-6 程度、tint で 2e-5 程度）を許す。
const TINT_RANGE_SLACK: f64 = 1e-3;

/// 色温度と tint。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemperatureTint {
    /// 色温度（K）。
    pub temperature_k: f64,
    /// 色かぶり補正（tint）。
    pub tint: f64,
}

/// Krystek (1985) の係数: u(T) = (a0 + a1·T + a2·T²) / (1 + b1·T + b2·T²)。
const KRYSTEK_U: ([f64; 3], [f64; 2]) = (
    [0.860117757, 1.54118254e-4, 1.28641212e-7],
    [8.42420235e-4, 7.08145163e-7],
);
/// Krystek (1985) の係数: v(T) = (a0 + a1·T + a2·T²) / (1 + b1·T + b2·T²)。
const KRYSTEK_V: ([f64; 3], [f64; 2]) = (
    [0.317398726, 4.22806245e-5, 4.20481691e-8],
    [-2.89741816e-5, 1.61456053e-7],
);

/// 有理式 (a0 + a1 T + a2 T²) / (1 + b1 T + b2 T²) の値と T による微分。
fn rational(c: &([f64; 3], [f64; 2]), t: f64) -> (f64, f64) {
    let (a, b) = c;
    let n = a[0] + a[1] * t + a[2] * t * t;
    let d = 1.0 + b[0] * t + b[1] * t * t;
    let dn = a[1] + 2.0 * a[2] * t;
    let dd = b[0] + 2.0 * b[1] * t;
    (n / d, (dn * d - n * dd) / (d * d))
}

/// プランク軌跡の uv（Krystek 1985 の近似。適用範囲は 1000〜15000 K）。
pub fn planckian_locus_uv(temperature_k: f64) -> Uv {
    Uv::new(
        rational(&KRYSTEK_U, temperature_k).0,
        rational(&KRYSTEK_V, temperature_k).0,
    )
}

/// CIE の昼光軌跡の xy（CIE 15 の式。4000〜7000 K と 7000〜25000 K で式が分かれる。
/// 適用範囲外の値は式をそのまま外挿する）。
pub fn daylight_locus_xy(temperature_k: f64) -> Xy {
    let t = temperature_k;
    let x = if t <= 7000.0 {
        -4.6070e9 / (t * t * t) + 2.9678e6 / (t * t) + 0.09911e3 / t + 0.244063
    } else {
        -2.0064e9 / (t * t * t) + 1.9018e6 / (t * t) + 0.24748e3 / t + 0.237040
    };
    Xy::new(x, -3.000 * x * x + 2.870 * x - 0.275)
}

/// 色温度 T でのプランク軌跡の点と、単位法線（緑の側、v が増える向き）。
fn planckian_frame(temperature_k: f64) -> (Uv, [f64; 2]) {
    let (u, du) = rational(&KRYSTEK_U, temperature_k);
    let (v, dv) = rational(&KRYSTEK_V, temperature_k);
    // 接線（T が下がる向き = mired が増える向き）は (−du, −dv)。法線はそれを 90 度回したもの
    // (dv, −du) で、2000 K 以上では v の成分が正（緑の側）になる。
    let len = du.hypot(dv);
    let tangent = [-du / len, -dv / len];
    let normal = [-tangent[1], tangent[0]];
    (Uv::new(u, v), normal)
}

/// 昼光の Duv に掛ける重み（[`WB_REFERENCE_BLEND_START_K`] 以下で 0、[`WB_REFERENCE_BLEND_END_K`]
/// 以上で 1、その間は `s²(3 − 2s)`）。
pub fn daylight_reference_weight(temperature_k: f64) -> f64 {
    let s = ((temperature_k - WB_REFERENCE_BLEND_START_K)
        / (WB_REFERENCE_BLEND_END_K - WB_REFERENCE_BLEND_START_K))
        .clamp(0.0, 1.0);
    s * s * (3.0 - 2.0 * s)
}

/// 基準の白（tint = 0）の Duv（モジュールの doc: 低い色温度は 0、高い色温度は昼光軌跡の点の Duv、
/// その間はなめらかにつなぐ）。
fn reference_duv(temperature_k: f64, p: Uv, normal: [f64; 2]) -> f64 {
    let w = daylight_reference_weight(temperature_k);
    if w == 0.0 {
        return 0.0;
    }
    let d = daylight_locus_xy(temperature_k).to_uv();
    w * ((d.u - p.u) * normal[0] + (d.v - p.v) * normal[1])
}

fn check_temperature_tint(temperature_k: f64, tint: f64) -> Result<()> {
    if !(temperature_k.is_finite()
        && (WB_TEMPERATURE_MIN_K..=WB_TEMPERATURE_MAX_K).contains(&temperature_k))
    {
        return Err(ColorError::TemperatureOutOfRange {
            temperature_k,
            min: WB_TEMPERATURE_MIN_K,
            max: WB_TEMPERATURE_MAX_K,
        });
    }
    if !(tint.is_finite() && (WB_TINT_MIN..=WB_TINT_MAX).contains(&tint)) {
        return Err(ColorError::TintOutOfRange {
            tint,
            min: WB_TINT_MIN,
            max: WB_TINT_MAX,
        });
    }
    Ok(())
}

/// 色温度と tint から白の色度を求める。範囲外（[`WB_TEMPERATURE_MIN_K`]〜[`WB_TEMPERATURE_MAX_K`]、
/// [`WB_TINT_MIN`]〜[`WB_TINT_MAX`]）や有限でない値はエラー。
pub fn white_point_from_temperature_tint(temperature_k: f64, tint: f64) -> Result<Xy> {
    check_temperature_tint(temperature_k, tint)?;
    let (p, n) = planckian_frame(temperature_k);
    let duv = reference_duv(temperature_k, p, n) + tint * TINT_DUV_PER_UNIT;
    Ok(Uv::new(p.u + duv * n[0], p.v + duv * n[1]).to_xy())
}

/// 白の色度から色温度（相関色温度）と tint を求める（二分法）。
///
/// 推定値が範囲外ならエラー [`ColorError::WhiteBalanceEstimateOutOfRange`]（推定値を含む。
/// 探索範囲 1000〜100000 K の外なら色温度は探索範囲の端の値）。
pub fn temperature_tint_from_white_point(white: Xy) -> Result<TemperatureTint> {
    if !(white.x.is_finite() && white.y.is_finite() && white.y > 0.0) {
        return Err(ColorError::InvalidChromaticity {
            x: white.x,
            y: white.y,
        });
    }
    let w = white.to_uv();
    if !(w.u.is_finite() && w.v.is_finite()) {
        return Err(ColorError::InvalidChromaticity {
            x: white.x,
            y: white.y,
        });
    }
    // g(m) = (w − P(m)) · 接線(m)。接線は mired が増える向きなので、g は mired について減少する。
    // 法線 n = (−t_v, t_u) から、接線は t = (n_v, −n_u)。
    let g = |mired: f64| -> f64 {
        let (p, n) = planckian_frame(1e6 / mired);
        (w.u - p.u) * n[1] - (w.v - p.v) * n[0]
    };
    let mut lo = 1e6 / SEARCH_MAX_K;
    let mut hi = 1e6 / SEARCH_MIN_K;
    let mired = if g(lo) < 0.0 {
        lo
    } else if g(hi) > 0.0 {
        hi
    } else {
        while hi - lo > BISECTION_TOLERANCE_MIRED {
            let mid = 0.5 * (lo + hi);
            if g(mid) > 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    };
    let temperature_k = 1e6 / mired;
    let (p, n) = planckian_frame(temperature_k);
    let duv = (w.u - p.u) * n[0] + (w.v - p.v) * n[1];
    let tint = (duv - reference_duv(temperature_k, p, n)) / TINT_DUV_PER_UNIT;

    let t_ok = (WB_TEMPERATURE_MIN_K * (1.0 - TEMPERATURE_RANGE_SLACK)
        ..=WB_TEMPERATURE_MAX_K * (1.0 + TEMPERATURE_RANGE_SLACK))
        .contains(&temperature_k);
    let tint_ok = (WB_TINT_MIN - TINT_RANGE_SLACK..=WB_TINT_MAX + TINT_RANGE_SLACK).contains(&tint);
    if !(t_ok && tint_ok) {
        return Err(ColorError::WhiteBalanceEstimateOutOfRange {
            temperature_k,
            tint,
        });
    }
    Ok(TemperatureTint {
        temperature_k: temperature_k.clamp(WB_TEMPERATURE_MIN_K, WB_TEMPERATURE_MAX_K),
        tint: tint.clamp(WB_TINT_MIN, WB_TINT_MAX),
    })
}

/// 白の XYZ からカメラの WB の係数を求める（G = 1 に正規化）。
///
/// `cam_xyz` は「XYZ（D65）→ カメラ RGB」の行列。白のカメラ RGB が 0 以下・有限でないならエラー。
pub fn wb_multipliers_from_white_xyz(cam_xyz: &Mat3, white_xyz: [f64; 3]) -> Result<[f64; 3]> {
    if !cam_xyz.is_finite() {
        return Err(ColorError::NonFinite {
            what: "カメラ行列"
        });
    }
    if !white_xyz.iter().all(|v| v.is_finite()) {
        return Err(ColorError::NonFinite { what: "白の XYZ" });
    }
    let cam = cam_xyz.apply(white_xyz);
    if !cam.iter().all(|&c| c.is_finite() && c > 0.0) {
        return Err(ColorError::InvalidWhiteBalance {
            reason: "白のカメラ RGB に 0 以下の値がある（カメラ行列か白の色が不正）",
        });
    }
    // 係数 = 1 / cam を G = 1 に正規化したもの = cam_G / cam。
    let m = [cam[1] / cam[0], 1.0, cam[1] / cam[2]];
    if !m.iter().all(|v| v.is_finite() && *v > 0.0) {
        return Err(ColorError::InvalidWhiteBalance {
            reason: "係数が有限の正の値にならない",
        });
    }
    Ok(m)
}

/// 色温度と tint からカメラの WB の係数を求める（G = 1 に正規化）。
pub fn wb_multipliers_from_temperature_tint(
    cam_xyz: &Mat3,
    temperature_k: f64,
    tint: f64,
) -> Result<[f64; 3]> {
    let white = white_point_from_temperature_tint(temperature_k, tint)?;
    wb_multipliers_from_white_xyz(cam_xyz, white.to_xyz(1.0)?)
}

/// 係数を作れる範囲に丸めるときに、色温度を寄せていく先（K。D65 に近い色温度。tint 0）。
const CLAMP_TARGET_K: f64 = 6504.0;
/// 丸めの二分法の回数（区間の幅は 2^−48 倍になる）。
const CLAMP_BISECTION_STEPS: u32 = 48;

/// [`wb_multipliers_from_temperature_tint_clamped`] の結果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClampedWhiteBalance {
    /// 係数（G = 1）。
    pub multipliers: [f64; 3],
    /// 指定された色温度・tint。
    pub requested: TemperatureTint,
    /// 係数を作るのに実際に使った色温度・tint（丸めなければ `requested` と同じ）。
    pub used: TemperatureTint,
}

impl ClampedWhiteBalance {
    /// 作れる範囲に丸めたか。
    pub fn was_clamped(&self) -> bool {
        self.requested != self.used
    }
}

/// 色温度と tint からカメラの WB の係数を求める。係数を作れない（白のカメラ RGB が 0 以下になる）、
/// または係数が `max_multiplier` を超える設定は、**作れる範囲に丸める**（人の判断の表 No.17）。
///
/// 丸め方: まず色温度はそのままで tint を 0 に向かって縮め（二分法）、作れる範囲の端の値を使う。
/// tint 0 でも作れなければ、tint 0 のまま色温度を 6504 K（D65 の近く）に向かって寄せる（mired で
/// 二分法）。6504 K・tint 0 でも作れなければ（カメラ行列の誤り）エラー。範囲外・有限でない入力、
/// 不正なカメラ行列はエラー（[`wb_multipliers_from_temperature_tint`] と同じ）。
pub fn wb_multipliers_from_temperature_tint_clamped(
    cam_xyz: &Mat3,
    temperature_k: f64,
    tint: f64,
    max_multiplier: f64,
) -> Result<ClampedWhiteBalance> {
    check_temperature_tint(temperature_k, tint)?;
    let requested = TemperatureTint {
        temperature_k,
        tint,
    };
    // Ok(Some(係数))・Ok(None)（作れない）・Err（そのほかの誤り）。
    let attempt = |t: f64, tint: f64| -> Result<Option<[f64; 3]>> {
        match wb_multipliers_from_temperature_tint(cam_xyz, t, tint) {
            Ok(m) if m.iter().all(|v| *v <= max_multiplier) => Ok(Some(m)),
            Ok(_) | Err(ColorError::InvalidWhiteBalance { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    };
    let done = |m: [f64; 3], t: f64, tint: f64| ClampedWhiteBalance {
        multipliers: m,
        requested,
        used: TemperatureTint {
            temperature_k: t,
            tint,
        },
    };
    if let Some(m) = attempt(temperature_k, tint)? {
        return Ok(done(m, temperature_k, tint));
    }
    // tint を縮める（lo: 作れる、hi: 作れない。tint の倍率）。
    if let Some(m0) = attempt(temperature_k, 0.0)? {
        let (mut lo, mut hi, mut best) = (0.0f64, 1.0f64, m0);
        for _ in 0..CLAMP_BISECTION_STEPS {
            let mid = 0.5 * (lo + hi);
            match attempt(temperature_k, mid * tint)? {
                Some(m) => {
                    lo = mid;
                    best = m;
                }
                None => hi = mid,
            }
        }
        return Ok(done(best, temperature_k, lo * tint));
    }
    // tint 0 のまま、色温度を CLAMP_TARGET_K に寄せる（mired で。lo: 作れる側の位置）。
    let (m_req, m_target) = (1e6 / temperature_k, 1e6 / CLAMP_TARGET_K);
    let at = |u: f64| 1e6 / (m_target + u * (m_req - m_target));
    let Some(m_target_wb) = attempt(CLAMP_TARGET_K, 0.0)? else {
        return Err(ColorError::InvalidWhiteBalance {
            reason: "6504 K・tint 0 でも係数を作れない（カメラ行列が不正）",
        });
    };
    let (mut lo, mut hi, mut best) = (0.0f64, 1.0f64, m_target_wb);
    for _ in 0..CLAMP_BISECTION_STEPS {
        let mid = 0.5 * (lo + hi);
        match attempt(at(mid), 0.0)? {
            Some(m) => {
                lo = mid;
                best = m;
            }
            None => hi = mid,
        }
    }
    Ok(done(best, at(lo), 0.0))
}

/// カメラの WB の係数から、白の XYZ（Y = 1 に正規化）を求める。
///
/// 係数の尺度は任意（G = 1 でなくてよい）。係数が 0 以下・有限でない、カメラ行列が特異、
/// 白の Y が 0 以下ならエラー。
pub fn white_xyz_from_wb_multipliers(cam_xyz: &Mat3, multipliers: [f64; 3]) -> Result<[f64; 3]> {
    if !multipliers.iter().all(|&m| m.is_finite() && m > 0.0) {
        return Err(ColorError::InvalidWhiteBalance {
            reason: "係数は有限の正の値が必要",
        });
    }
    let inv = cam_xyz.inverse()?;
    let cam_white = [
        1.0 / multipliers[0],
        1.0 / multipliers[1],
        1.0 / multipliers[2],
    ];
    let xyz = inv.apply(cam_white);
    if !(xyz.iter().all(|v| v.is_finite()) && xyz[1] > 0.0) {
        return Err(ColorError::InvalidWhiteBalance {
            reason: "係数から求めた白の Y が 0 以下",
        });
    }
    Ok([xyz[0] / xyz[1], 1.0, xyz[2] / xyz[1]])
}

/// カメラの WB の係数（撮影時の値など）から、色温度と tint を推定する。
pub fn temperature_tint_from_wb_multipliers(
    cam_xyz: &Mat3,
    multipliers: [f64; 3],
) -> Result<TemperatureTint> {
    let xyz = white_xyz_from_wb_multipliers(cam_xyz, multipliers)?;
    let xy = Xy::from_xyz(xyz).map_err(|_| ColorError::InvalidWhiteBalance {
        reason: "係数から求めた白の色度を計算できない",
    })?;
    temperature_tint_from_white_point(xy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::{D50, D65, XYZ_TO_BT2020, XYZ_TO_SRGB};

    fn uv_dist(a: Xy, b: Xy) -> f64 {
        let (a, b) = (a.to_uv(), b.to_uv());
        (a.u - b.u).hypot(a.v - b.v)
    }

    /// テスト用の「典型的な形の」カメラ行列（実在の機種の値として使わない）。
    fn typical_cam_xyz() -> Mat3 {
        Mat3::from_rows([
            [7000.0, -2000.0, -600.0],
            [-5000.0, 13000.0, 2300.0],
            [-700.0, 1300.0, 6500.0],
        ])
        .scale(1.0 / 10000.0)
    }

    #[test]
    fn planckian_matches_illuminant_a() {
        // CIE 標準イルミナント A（黒体放射。c2 = 1.4388e-2 の温度目盛りで約 2856 K）。
        // xy = (0.44757, 0.40745)。近似式の誤差を見込んで uv で 2e-4 以内。
        let a = Xy::new(0.44757, 0.40745);
        let p = planckian_locus_uv(2856.0).to_xy();
        assert!(uv_dist(a, p) < 2e-4, "{p:?}");
    }

    #[test]
    fn daylight_locus_matches_d65_and_d50() {
        // D65 は c2 の改定により 6504 K、D50 は 5003 K に相当する。
        let d65 = daylight_locus_xy(6504.0);
        assert!(
            (d65.x - D65.x).abs() < 2e-4 && (d65.y - D65.y).abs() < 2e-4,
            "{d65:?}"
        );
        let d50 = daylight_locus_xy(5003.0);
        assert!(
            (d50.x - D50.x).abs() < 2e-4 && (d50.y - D50.y).abs() < 2e-4,
            "{d50:?}"
        );
        // CIE 15 の D55（5503 K）と D75（7504 K）の色度（小数 5 桁の公表値）。式は分光分布からの
        // 計算値と y で 1e-4 程度ずれる（式の近似）。
        for (t, x, y) in [(5503.0, 0.33242, 0.34743), (7504.0, 0.29902, 0.31485)] {
            let d = daylight_locus_xy(t);
            assert!(
                (d.x - x).abs() < 2e-4 && (d.y - y).abs() < 2e-4,
                "{t}: {d:?}"
            );
        }
        // 7000 K の式の切り替わりで連続。
        let a = daylight_locus_xy(7000.0);
        let b = daylight_locus_xy(7000.0 + 1e-9);
        assert!((a.x - b.x).abs() < 1e-6);
    }

    #[test]
    fn about_6500k_is_close_to_d65() {
        let w = white_point_from_temperature_tint(6504.0, 0.0).unwrap();
        assert!(
            (w.x - D65.x).abs() < 2e-4 && (w.y - D65.y).abs() < 2e-4,
            "{w:?}"
        );
        let w = white_point_from_temperature_tint(6500.0, 0.0).unwrap();
        assert!(uv_dist(w, D65) < 3e-4, "{w:?}");
        // D65 の白から逆算すると、色温度は約 6500 K（相関色温度 6504 K 前後）、tint は 0 付近。
        let tt = temperature_tint_from_white_point(D65).unwrap();
        assert!((tt.temperature_k - 6504.0).abs() < 15.0, "{tt:?}");
        assert!(tt.tint.abs() < 1.0, "{tt:?}");
    }

    /// 白の色度の、プランク軌跡からの Duv（法線方向の符号付きの距離）。
    fn duv_of(white: Xy, temperature_k: f64) -> f64 {
        let w = white.to_uv();
        let (p, n) = planckian_frame(temperature_k);
        (w.u - p.u) * n[0] + (w.v - p.v) * n[1]
    }

    #[test]
    fn cct_and_duv_match_published_values() {
        // よく引用される相関色温度と Duv の値（例: Ohno 2014, LEUKOS 10(1)）:
        // D65 は 6504 K・Duv +0.0032、標準イルミナント A は 2856 K・Duv 0（黒体放射）。
        // D50 は 5003 K（c2 の改定による。Duv は照合しない）。Krystek の近似式の誤差を見込んで、
        // 色温度は ±10 K、Duv は ±3e-4 とする。
        let a = Xy::new(0.44757, 0.40745);
        for (name, white, cct, duv) in [
            ("D65", D65, 6504.0, Some(0.0032)),
            ("D50", D50, 5003.0, None),
            ("A", a, 2856.0, Some(0.0)),
        ] {
            // tint の範囲内（A は 4000 K 未満、D50・D65 は昼光の基準に近い）なので推定できる。
            let tt = temperature_tint_from_white_point(white).unwrap();
            assert!((tt.temperature_k - cct).abs() < 10.0, "{name}: {tt:?}");
            if let Some(duv) = duv {
                let d = duv_of(white, tt.temperature_k);
                assert!((d - duv).abs() < 3e-4, "{name}: Duv {d}");
            }
        }
    }

    #[test]
    fn below_the_blend_is_on_planckian_locus() {
        for t in [2000.0, 2856.0, 3500.0, 3799.0, 3800.0] {
            let w = white_point_from_temperature_tint(t, 0.0).unwrap();
            let p = planckian_locus_uv(t).to_xy();
            assert!(uv_dist(w, p) < 1e-12, "{t}");
        }
    }

    #[test]
    fn above_the_blend_is_close_to_daylight_locus() {
        // 基準の白は、昼光軌跡の点を等色温度線に射影したもの。10000 K 以下では 5e-5 以内。
        for t in [4200.0, 5000.0, 5500.0, 6504.0, 7500.0, 10000.0] {
            let w = white_point_from_temperature_tint(t, 0.0).unwrap();
            let d = daylight_locus_xy(t);
            assert!(uv_dist(w, d) < 5e-5, "{t}: {}", uv_dist(w, d));
        }
    }

    #[test]
    fn reference_white_is_smooth_around_4000k() {
        // 人の判断 No.17: 以前は 4000 K の前後で基準の白が Duv 約 0.0028（tint 約 8.3）跳んでいた。
        // 3800〜4200 K で 2 つの式をなめらかにつなぐので、跳びはなく、傾きも連続。
        let duv = |t: f64| {
            let (p, n) = planckian_frame(t);
            reference_duv(t, p, n)
        };
        let jump = (duv(4000.0 + 1e-9) - duv(4000.0 - 1e-9)).abs();
        assert!(jump < 1e-12, "{jump}");
        // 区間の両端で値・傾きが連続（重みの傾きが 0）。
        for t in [WB_REFERENCE_BLEND_START_K, WB_REFERENCE_BLEND_END_K] {
            let h = 1e-3;
            let left = (duv(t) - duv(t - h)) / h;
            let right = (duv(t + h) - duv(t)) / h;
            assert!((left - right).abs() < 1e-7, "{t}: {left} {right}");
        }
        // 1 K ごとの基準の白の変化は、区間の中でも区間の外と同じ程度（tint の 0.05 未満の変化）。
        let mut prev = white_point_from_temperature_tint(3700.0, 0.0).unwrap();
        let mut t = 3701.0;
        while t <= 4300.0 {
            let w = white_point_from_temperature_tint(t, 0.0).unwrap();
            let d = uv_dist(prev, w);
            assert!(d < 1e-4, "{t}: {d}");
            prev = w;
            t += 1.0;
        }
        // 重み。
        assert_eq!(daylight_reference_weight(3800.0), 0.0);
        assert_eq!(daylight_reference_weight(4000.0), 0.5);
        assert_eq!(daylight_reference_weight(4200.0), 1.0);
    }

    #[test]
    fn tint_moves_perpendicular_toward_green() {
        for t in [2500.0, 4500.0, 6504.0, 20000.0] {
            let base = white_point_from_temperature_tint(t, 0.0).unwrap().to_uv();
            let plus = white_point_from_temperature_tint(t, 30.0).unwrap().to_uv();
            let (_, n) = planckian_frame(t);
            let d = [plus.u - base.u, plus.v - base.v];
            // 大きさは 30 × TINT_DUV_PER_UNIT、向きは法線（v が増える側）。
            assert!((d[0].hypot(d[1]) - 30.0 * TINT_DUV_PER_UNIT).abs() < 1e-12);
            assert!((d[0] * n[0] + d[1] * n[1] - 30.0 * TINT_DUV_PER_UNIT).abs() < 1e-12);
            assert!(d[1] > 0.0, "{t}: 正の tint は緑の側（v が増える）");
        }
    }

    #[test]
    fn multipliers_for_srgb_camera_at_d65_are_neutral() {
        // カメラが sRGB（BT.709）そのものなら、D65 の白で係数はほぼ (1, 1, 1)。
        let m = wb_multipliers_from_white_xyz(&XYZ_TO_SRGB, D65.to_xyz(1.0).unwrap()).unwrap();
        for v in m {
            assert!((v - 1.0).abs() < 1e-12);
        }
        let m = wb_multipliers_from_temperature_tint(&XYZ_TO_SRGB, 6504.0, 0.0).unwrap();
        for v in m {
            assert!((v - 1.0).abs() < 2e-3, "{m:?}");
        }
    }

    #[test]
    fn multiplier_directions() {
        let cam = typical_cam_xyz();
        // 色温度を上げる（光源が青寄り）と、R の係数は増え、B の係数は減る（4000 K の前後もなめらかに
        // つながる）。
        for temps in [&[
            2000.0, 2500.0, 3000.0, 3500.0, 3800.0, 3900.0, 3999.0, 4000.0, 4100.0, 4200.0, 5000.0,
            6500.0, 9000.0, 20000.0, 50000.0,
        ][..]]
        {
            let mut prev = wb_multipliers_from_temperature_tint(&cam, temps[0], 0.0).unwrap();
            for &t in &temps[1..] {
                let m = wb_multipliers_from_temperature_tint(&cam, t, 0.0).unwrap();
                assert_eq!(m[1], 1.0);
                assert!(m[0] > prev[0], "{t}: R {m:?} {prev:?}");
                assert!(m[2] < prev[2], "{t}: B {m:?} {prev:?}");
                prev = m;
            }
        }
        // tint を正にする（光源が緑寄り）と、G に対して R と B の係数が増える（マゼンタ寄り）。
        let m0 = wb_multipliers_from_temperature_tint(&cam, 5500.0, 0.0).unwrap();
        let mp = wb_multipliers_from_temperature_tint(&cam, 5500.0, 50.0).unwrap();
        assert!(mp[0] > m0[0] && mp[2] > m0[2], "{m0:?} {mp:?}");
        let mn = wb_multipliers_from_temperature_tint(&cam, 5500.0, -50.0).unwrap();
        assert!(mn[0] < m0[0] && mn[2] < m0[2], "{m0:?} {mn:?}");
    }

    #[test]
    fn roundtrip_temperature_tint_via_multipliers() {
        let cams = [typical_cam_xyz(), XYZ_TO_SRGB, XYZ_TO_BT2020];
        let temps = [
            2000.0, 2100.0, 2500.0, 2856.0, 3200.0, 3500.0, 3900.0, 3999.0, 4000.0, 4001.0, 4500.0,
            5000.0, 5500.0, 6504.0, 7000.0, 8000.0, 10000.0, 15000.0, 25000.0, 40000.0, 50000.0,
        ];
        let tints = [-150.0, -80.0, -10.0, 0.0, 0.5, 25.0, 100.0, 150.0];
        let mut ok = 0;
        let mut total = 0;
        for cam in &cams {
            for &t in &temps {
                for &tint in &tints {
                    total += 1;
                    // 低い色温度で tint を大きく正にすると、白がカメラの表せる範囲（ここでは
                    // sRGB・BT.2020 の色域）の外になり、係数を作れない（エラー）。それは除く。
                    let m = match wb_multipliers_from_temperature_tint(cam, t, tint) {
                        Ok(m) => m,
                        Err(ColorError::InvalidWhiteBalance { .. }) => continue,
                        Err(e) => panic!("T {t}, tint {tint}: {e}"),
                    };
                    ok += 1;
                    let est = temperature_tint_from_wb_multipliers(cam, m).unwrap();
                    assert!(
                        (est.temperature_k - t).abs() < 1e-6 * t,
                        "T {t}, tint {tint} → {est:?}"
                    );
                    assert!(
                        (est.tint - tint).abs() < 1e-4,
                        "T {t}, tint {tint} → {est:?}"
                    );
                }
            }
        }
        assert!(
            ok * 10 >= total * 9,
            "係数を作れた組が少なすぎる: {ok} / {total}"
        );
    }

    #[test]
    fn roundtrip_temperature_tint_via_white_point() {
        // 色度の段階では、範囲内のすべての組が往復する（カメラの色域に関係しない）。
        let mut t = WB_TEMPERATURE_MIN_K;
        while t <= WB_TEMPERATURE_MAX_K {
            let mut tint = WB_TINT_MIN;
            while tint <= WB_TINT_MAX {
                let w = white_point_from_temperature_tint(t, tint).unwrap();
                let est = temperature_tint_from_white_point(w).unwrap();
                assert!(
                    (est.temperature_k - t).abs() < 1e-6 * t,
                    "T {t}, tint {tint} → {est:?}"
                );
                assert!(
                    (est.tint - tint).abs() < 1e-4,
                    "T {t}, tint {tint} → {est:?}"
                );
                tint += 18.75;
            }
            t *= 1.07;
        }
    }

    #[test]
    fn multipliers_scale_does_not_matter() {
        let cam = typical_cam_xyz();
        let m = wb_multipliers_from_temperature_tint(&cam, 5200.0, 12.0).unwrap();
        let scaled = [m[0] * 1024.0, m[1] * 1024.0, m[2] * 1024.0];
        let a = temperature_tint_from_wb_multipliers(&cam, m).unwrap();
        let b = temperature_tint_from_wb_multipliers(&cam, scaled).unwrap();
        assert!((a.temperature_k - b.temperature_k).abs() < 1e-6);
        assert!((a.tint - b.tint).abs() < 1e-6);
    }

    #[test]
    fn out_of_range_inputs_are_errors() {
        for (t, tint) in [
            (1999.0, 0.0),
            (50001.0, 0.0),
            (f64::NAN, 0.0),
            (5000.0, 150.5),
            (5000.0, -151.0),
            (5000.0, f64::INFINITY),
        ] {
            let r = white_point_from_temperature_tint(t, tint);
            assert!(
                matches!(
                    r,
                    Err(ColorError::TemperatureOutOfRange { .. }
                        | ColorError::TintOutOfRange { .. })
                ),
                "{t} {tint}: {r:?}"
            );
        }
        // 範囲の端はエラーにしない。
        assert!(white_point_from_temperature_tint(2000.0, -150.0).is_ok());
        assert!(white_point_from_temperature_tint(50000.0, 150.0).is_ok());
    }

    #[test]
    fn estimate_out_of_range_is_error_with_estimate() {
        // 1500 K 相当の白（設定できる範囲より低い）。
        let w = planckian_locus_uv(1500.0).to_xy();
        match temperature_tint_from_white_point(w) {
            Err(ColorError::WhiteBalanceEstimateOutOfRange {
                temperature_k,
                tint,
            }) => {
                assert!((temperature_k - 1500.0).abs() < 1e-3, "{temperature_k}");
                assert!(tint.abs() < 1e-3);
            }
            other => panic!("{other:?}"),
        }
        // tint が大きすぎる（Duv 0.08）。
        let (p, n) = planckian_frame(3000.0);
        let w = Uv::new(p.u + 0.08 * n[0], p.v + 0.08 * n[1]).to_xy();
        match temperature_tint_from_white_point(w) {
            Err(ColorError::WhiteBalanceEstimateOutOfRange {
                temperature_k,
                tint,
            }) => {
                assert!((temperature_k - 3000.0).abs() < 1e-3, "{temperature_k}");
                assert!((tint - 240.0).abs() < 1e-3, "{tint}");
            }
            other => panic!("{other:?}"),
        }
        // 探索範囲（1000 K）よりさらに低い: 色温度は探索範囲の端の値。
        let w = Xy::new(0.65, 0.34);
        assert!(matches!(
            temperature_tint_from_white_point(w),
            Err(ColorError::WhiteBalanceEstimateOutOfRange { .. })
        ));
        // 不正な色度。
        assert!(temperature_tint_from_white_point(Xy::new(f64::NAN, 0.3)).is_err());
        assert!(temperature_tint_from_white_point(Xy::new(0.3, 0.0)).is_err());
    }

    #[test]
    fn unbuildable_white_balance_is_clamped_to_the_buildable_range() {
        // 人の判断 No.17: 低い色温度で tint を大きく正にすると、白のカメラ RGB が 0 以下になり係数を
        // 作れない。作れる範囲に丸める（色温度はそのまま、tint を 0 に向かって縮める）。
        let cam = XYZ_TO_SRGB;
        let (t, tint) = (2000.0, 150.0);
        assert!(matches!(
            wb_multipliers_from_temperature_tint(&cam, t, tint),
            Err(ColorError::InvalidWhiteBalance { .. })
        ));
        let r = wb_multipliers_from_temperature_tint_clamped(&cam, t, tint, 1000.0).unwrap();
        assert!(r.was_clamped());
        assert_eq!(r.used.temperature_k, t);
        assert!(r.used.tint > 0.0 && r.used.tint < tint, "{r:?}");
        assert!(
            r.multipliers
                .iter()
                .all(|v| v.is_finite() && *v > 0.0 && *v <= 1000.0)
        );
        // 丸めた値は作れる範囲の端（少しでも tint を大きくすると作れない、または上限を超える）。
        let beyond = r.used.tint + 0.01;
        let over = match wb_multipliers_from_temperature_tint(&cam, t, beyond) {
            Ok(m) => m.iter().any(|v| *v > 1000.0),
            Err(ColorError::InvalidWhiteBalance { .. }) => true,
            Err(e) => panic!("{e}"),
        };
        assert!(over, "{r:?}");
        // 作れる設定は丸めない（係数も同じ）。
        let ok = wb_multipliers_from_temperature_tint_clamped(&cam, 5000.0, 10.0, 1000.0).unwrap();
        assert!(!ok.was_clamped());
        assert_eq!(
            ok.multipliers,
            wb_multipliers_from_temperature_tint(&cam, 5000.0, 10.0).unwrap()
        );
        // 係数の上限を小さくすると、上限の内に収まるように丸める。
        let small = wb_multipliers_from_temperature_tint_clamped(&cam, 2000.0, 0.0, 2.0).unwrap();
        assert!(small.was_clamped());
        assert!(small.multipliers.iter().all(|v| *v <= 2.0), "{small:?}");
        // tint 0 でも作れない色温度は、色温度を 6504 K に向かって寄せる。
        assert!(small.used.temperature_k > 2000.0 && small.used.tint == 0.0);
        // 範囲外の入力はエラーのまま。
        assert!(wb_multipliers_from_temperature_tint_clamped(&cam, 1999.0, 0.0, 1000.0).is_err());
        // 6504 K でも作れないカメラ行列はエラー。
        let neg = Mat3::diagonal([-1.0, 1.0, 1.0]) * typical_cam_xyz();
        assert!(matches!(
            wb_multipliers_from_temperature_tint_clamped(&neg, 5000.0, 0.0, 1000.0),
            Err(ColorError::InvalidWhiteBalance { .. })
        ));
    }

    #[test]
    fn invalid_multipliers_and_matrices() {
        let cam = typical_cam_xyz();
        for m in [
            [0.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
            [1.0, f64::NAN, 1.0],
            [1.0, 1.0, f64::INFINITY],
        ] {
            assert!(matches!(
                temperature_tint_from_wb_multipliers(&cam, m),
                Err(ColorError::InvalidWhiteBalance { .. })
            ));
        }
        let singular = Mat3::from_rows([[1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!(matches!(
            temperature_tint_from_wb_multipliers(&singular, [1.0, 1.0, 1.0]),
            Err(ColorError::SingularMatrix { .. })
        ));
        // 白のカメラ RGB が負になる行列（R の行を反転）。
        let neg = Mat3::diagonal([-1.0, 1.0, 1.0]) * cam;
        assert!(matches!(
            wb_multipliers_from_temperature_tint(&neg, 5000.0, 0.0),
            Err(ColorError::InvalidWhiteBalance { .. })
        ));
        let mut rows = cam.rows();
        rows[0][0] = f64::NAN;
        assert!(matches!(
            wb_multipliers_from_temperature_tint(&Mat3::from_rows(rows), 5000.0, 0.0),
            Err(ColorError::NonFinite { .. })
        ));
    }
}
