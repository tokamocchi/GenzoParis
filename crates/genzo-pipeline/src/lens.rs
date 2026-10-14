//! レンズ補正（DEV-14。docs/04_architecture.md の 2.1 節のステージ 7・9、2.7 節）。処理バージョン 2 から。
//!
//! 利用者の決定（DEV-14）: **ARW / DNG に埋め込まれた補正の情報を使い、ない場合は手動のスライダーで
//! 補正する**。lensfun などの外部のデータは使わない。埋め込みの情報の読み取りは genzo-raw の `lens`。
//!
//! | 補正 | ステージ | 段階 | 型 |
//! |---|---|---|---|
//! | 周辺減光（プロファイル・手動） | 7（カメラ RGB。B1） | A1 | [`LensStagePlan`] |
//! | 倍率色収差（プロファイル・手動。R と B の像の大きさ） | 7（同上） | A1 | [`LensStagePlan`] |
//! | 歪曲（プロファイル・手動） | 9（ジオメトリの座標の変換。1 回のリサンプリング） | C（B にも影響） | [`RadialDistortion`] |
//!
//! # 座標と半径（2.7 節）
//!
//! どの補正も、中心からの距離 `r`（S の画素の単位。向きは距離を変えない）の関数で表す。
//!
//! - **Sony の ARW・手動の補正**: 中心は **既定の切り抜き（O）の中心**、半径の基準 `R` は既定の切り抜きの
//!   **対角線の半分**（カメラの JPEG と同じ範囲。darktable は RAW の切り抜き（rawprepare）の後の画像の
//!   対角線の半分を使う。既定の切り抜きと同じかは**実機で未確認**）。
//! - **DNG の opcode**: 中心は opcode の光学中心（S の全体を (0, 0)〜(1, 1) とした座標）、`R` は中心から
//!   S の最も遠い隅までの距離（DNG の仕様の `m`）。S は DNG の段階 3 の画像（ActiveArea の全体）に当たる
//!   とみなした（**未確認**）。
//! - 1 つのステージの中心は 1 つにする: ステージ 7 は DNG の FixVignetteRadial（なければ WarpRectilinear、
//!   どちらもなければ O の中心）、ステージ 9 は DNG の WarpRectilinear（なければ O の中心）。手動の補正も
//!   その中心のまわりに適用する（DNG の光学中心が画像の中心からずれていると、手動の補正の中心もずれる。
//!   ふつうはほぼ中央）。
//!
//! # 埋め込みの情報の値の意味（Sony）
//!
//! Sony の値の意味は公開の仕様がないため、**darktable の `src/iop/lens.cc`（埋め込みのメタデータの方式の
//! 版 2。`_init_coeffs_md_v2`）の解釈に従った**（2026-10-13 に、darktable の master の lens.cc（SHA-256
//! `5801e7dd1ef957a8fd243badb3fc28b579a919aa9abcb39c42c9dd2444577886`）を AI が読んで確かめた。式を写した
//! ものではないが、保守的に、参考にした実装の著作権表示をここに記す: darktable — Copyright (C) 2019-2026
//! darktable developers、GPL-3.0-or-later。GenzoParis と同じ GPL-3.0-or-later。docs/third_party.md）:
//!
//! - 節点: `n` 個の値の `i` 番目の位置は `ρ = (i + 0.5) / (n − 1)`（`ρ = r / R`）。節点の間は線形補間、
//!   範囲の外は端の値（[`interp_knots`]）。
//! - 歪曲: `k(ρ) = 1 + t · d(ρ) · 2^−14`（`t` は適用量 / 100）。出力の画素の位置 `p` の値を、入力の
//!   `c + k · (p − c)` から取る。
//! - 色収差: R・B の取る位置の倍率 `m(ρ) = 1 + a(ρ) · 2^−21`。
//! - 周辺減光: `v_i' = 2^(0.5 − 2^(t · v_i · 2^−13 − 1))` を節点で補間し、画素を `v'` で割る（利得 `1 / v'`）。
//!   darktable の版 1 は `v'` の 2 乗で割っていた（版 2 で 2 乗をやめた）。**どちらがカメラの JPEG に近いかは
//!   実機の ARW で未確認**（人の確認待ち）。
//!
//! # 埋め込みの情報の値の意味（DNG。Adobe の DNG Specification 1.4.0.0）
//!
//! 仕様書はこの調査では取得していない（式は AI の知識による）。darktable の lens.cc の DNG の扱いと
//! 照らした（2026-10-13）: 式は同じ（周辺減光は、実質 `1 + t · (…)` を掛ける）。ただし darktable は中心を
//! 画像の中心、半径の基準を画像の対角線の半分にしていて（opcode の中心を使わない）、ここでは opcode の中心と
//! 仕様の `m`（中心から最も遠い隅まで）を使う（**どちらが正しいかは実際の DNG で未確認**）。
//!
//! - WarpRectilinear: `k(ρ) = kr0 + kr1·ρ² + kr2·ρ⁴ + kr3·ρ⁶`、出力の位置 `p` の値を `c + k · (p − c)` から
//!   取る。適用量 `t` は `1 + t · (k − 1)`。**接線方向の係数（kt0・kt1）は使わない**（0 でなければ警告）。
//!   面が 3 つなら、歪曲は G の面（2 つ目）、色収差は `k_R / k_G`・`k_B / k_G`（同じ `ρ` で割る近似）。
//! - FixVignetteRadial: 利得 `1 + t · (k0·ρ² + k1·ρ⁴ + k2·ρ⁶ + k3·ρ⁸ + k4·ρ¹⁰)` を掛ける。
//!
//! # 手動の補正（AI の仮決定。値は仮置き）
//!
//! - 歪曲: `k(ρ) = 1 − (v / 100) · MANUAL_DISTORTION_MAX · ρ²`（[`MANUAL_DISTORTION_MAX`] = 0.08。正の値で
//!   四隅を中心へ寄せて取る＝樽型の歪曲を補正する）。プロファイルの歪曲の前（出力側）にかける:
//!   `k_total(r) = k_m(r) · k_p(k_m(r) · r)`。
//! - 周辺減光: 利得 `2^(v / 100 · MANUAL_VIGNETTING_MAX_EV · w(ρ))`、`w` は `ρ` が中点 `m0 = 中点 / 100 · 0.9`
//!   から 1 までの smoothstep（`ρ ≥ 1` は 1）。
//! - 色収差: R（B）の取る位置の倍率 `1 − (v / 100) · MANUAL_CA_MAX`（[`MANUAL_CA_MAX`] = 0.002。α7 IV の
//!   対角線の半分（約 4211 画素）で最大 約 8.4 画素）。プロファイルの倍率と掛けた値は
//!   [`MAX_CA_RATIO_DEVIATION`]（5 %）の中に収める（壊れた・細工した値で入力の範囲が巨大にならないため）。
//!
//! # 歪曲の後の枠（自動の拡大・縮小。AI の仮決定）
//!
//! 歪曲を補正した画像（2.7 節の G の回転の前。この crate では **D** と呼ぶ）の枠は O と同じ大きさにし、
//! **D の枠の全体が O の内側から取れる最大の範囲**になるように倍率 `z` を掛ける（darktable の自動の
//! 倍率と同じ考え方）: `o = c + z · k_total(z · r) · (d − c)`。樽型の補正（四隅を寄せて取る）では
//! `z > 1`（写る範囲を広げる）、糸巻き型の補正では `z < 1`（拡大）。D の枠の辺の上の点（各辺 [`ZOOM_EDGE_SAMPLES`]
//! 点と、中心に最も近い点）が O の内側に入る最大の `z` を二分法で求める（r · k(r) が単調に増える前提）。
//! そのため D には画像のない部分がなく、切り抜きの制限（[`crate::finish::constrain_crop`]）は回転だけを
//! 考えればよい。
//!
//! # 計算（GPU 版も同じ式。2.3 節）
//!
//! 補正の関数は f64 で、半径の 2 乗 `u = r² / R²` の等間隔の表（[`LUT_INTERVALS`] 区間の 1D LUT。
//! [`crate::interp::Lut1d`] と同じ式）にし、画素ごとには f32 で表を引く（平方根を使わない）。表の外
//! （S の隅より遠い）は端の値。

use genzo_model::LensCorrection;
use genzo_raw::{LensCorrectionData, PixelRect, RawImage};

use crate::coords::CoordinateFrames;
use crate::error::Result;
use crate::interp::Lut1d;
use crate::version::ProcessVersion;

/// 補正の表（1D LUT）の区間の数（点の数は 1 つ多い）。**仮置き**: 補正の関数は滑らかなので、
/// 256 区間の線形補間の誤差は α7 IV の隅で 0.01 画素より十分小さい見込み（テストで確かめた範囲）。
pub const LUT_INTERVALS: usize = 256;
/// 歪曲の表の点の数（[`RadialDistortion`] は `Copy` にするため固定の長さの配列で持つ）。
pub const DISTORTION_LUT_LEN: usize = LUT_INTERVALS + 1;

/// 手動の歪曲の最大（`ρ = 1` での取る位置の倍率の変化。**仮置き**）。
pub const MANUAL_DISTORTION_MAX: f64 = 0.08;
/// 手動の周辺減光の最大（隅での段数。**仮置き**）。
pub const MANUAL_VIGNETTING_MAX_EV: f64 = 1.0;
/// 手動の周辺減光の中点 100 に当たる `ρ`（**仮置き**）。
pub const MANUAL_VIGNETTING_MIDPOINT_MAX: f64 = 0.9;
/// 手動の色収差の最大（取る位置の倍率の変化。**仮置き**）。
pub const MANUAL_CA_MAX: f64 = 0.002;
/// ステージ 7 の色収差の取る位置の倍率（プロファイルと手動を掛けた値）の 1 からの差の上限（genzo-raw の
/// [`genzo_raw::MAX_CA_RATIO_DEVIATION`] と同じ 5 %。**仮置き**）。genzo-raw の検証を通った埋め込みの情報では
/// 届かない（Sony の値は i16 で最大 約 1.6 %）が、壊れた・細工した値でも入力の範囲の余白（[`LensStagePlan::margin`]）
/// を有限に保つため、表の値をこの範囲に収める（有限でない値は 1）。
pub const MAX_CA_RATIO_DEVIATION: f64 = genzo_raw::MAX_CA_RATIO_DEVIATION;
/// 自動の倍率を求めるときの、D の枠の 1 辺あたりの点の数。
pub const ZOOM_EDGE_SAMPLES: usize = 64;

/// Sony の歪曲の値の尺度（2^−14。darktable の解釈）。
const SONY_DISTORTION_UNIT: f64 = 1.0 / 16384.0;
/// Sony の色収差の値の尺度（2^−21）。
const SONY_CA_UNIT: f64 = 1.0 / 2_097_152.0;
/// Sony の周辺減光の値の尺度（2^−13）。
const SONY_VIGNETTING_UNIT: f64 = 1.0 / 8192.0;
/// 周辺減光の補間した値の下限（darktable と同じ 1e−4。0 での割り算を避ける）。
const VIGNETTING_FLOOR: f64 = 1e-4;
/// 自動の倍率の探索の範囲。
const ZOOM_RANGE: (f64, f64) = (0.25, 4.0);
/// 表の範囲の、最も遠い点の半径に対する余裕（`u` で 1.21 倍）。
const LUT_RADIUS_MARGIN: f64 = 1.1;
/// 自動の倍率で、D の枠の点が O の外に出てよい量（O の画素の単位。丸めの誤差）。
const ZOOM_TOLERANCE_PX: f64 = 1e-6;
/// 歪曲の倍率の表がこの範囲で 1 なら、補正なしとみなす（f32 の 1 ULP より小さい）。
const IDENTITY_TOLERANCE: f64 = 1e-8;

/// 節点 `knots`（昇順）の値 `values` を線形補間する（範囲の外は端の値。darktable の
/// `_interpolate_linear_spline` と同じ規則）。
pub fn interp_knots(knots: &[f64], values: &[f64], x: f64) -> f64 {
    debug_assert_eq!(knots.len(), values.len());
    let n = knots.len();
    if n == 0 {
        return 0.0;
    }
    if x < knots[0] {
        return values[0];
    }
    for i in 1..n {
        if x < knots[i] {
            let t = (x - knots[i - 1]) / (knots[i] - knots[i - 1]);
            return values[i - 1] + (values[i] - values[i - 1]) * t;
        }
    }
    values[n - 1]
}

/// Sony の節点の位置（`(i + 0.5) / (n − 1)`）。
fn sony_knots(n: usize) -> Vec<f64> {
    let d = (n.max(2) - 1) as f64;
    (0..n).map(|i| (i as f64 + 0.5) / d).collect()
}

/// 既定の切り抜き（O の枠）の中心（S の座標）と、対角線の半分（S の画素）。
fn crop_center_and_half_diagonal(crop: PixelRect) -> ((f64, f64), f64) {
    let (w, h) = (f64::from(crop.width), f64::from(crop.height));
    (
        (f64::from(crop.x) + w * 0.5, f64::from(crop.y) + h * 0.5),
        0.5 * w.hypot(h),
    )
}

/// 点 `c` から矩形 `[0, w] × [0, h]` の最も遠い隅までの距離。
fn farthest_corner(c: (f64, f64), w: f64, h: f64) -> f64 {
    [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
        .iter()
        .map(|&(x, y)| (x - c.0).hypot(y - c.1))
        .fold(0.0, f64::max)
}

/// DNG の光学中心（正規化）を S の座標にし、S の最も遠い隅までの距離（DNG の仕様の `m`）と返す。
fn dng_center(center: [f64; 2], sensor: (u32, u32)) -> ((f64, f64), f64) {
    let (w, h) = (f64::from(sensor.0), f64::from(sensor.1));
    let c = (center[0] * w, center[1] * h);
    (c, farthest_corner(c, w, h).max(1.0))
}

/// DNG の WarpRectilinear の面の半径方向の関数 `kr0 + kr1·ρ² + kr2·ρ⁴ + kr3·ρ⁶`。
fn dng_radial(kr: &[f64; 4], rho: f64) -> f64 {
    let r2 = rho * rho;
    kr[0] + r2 * (kr[1] + r2 * (kr[2] + r2 * kr[3]))
}

/// プロファイル（埋め込みの情報）の歪曲の部分（[`CoordinateFrames::lens_geometry`] に入れる。`Copy`）。
///
/// 適用量・手動の補正・回転とは独立で、写真ごとに決まる（段階 A1 の画像に付けて段階 B・C へ渡す）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ProfileDistortion {
    /// Sony の ARW（節点の値。数は `n`）。中心と半径の基準は O（既定の切り抜き）。
    Sony {
        /// 節点の数（2〜16）。
        n: u8,
        /// 節点ごとの値（先頭の `n` 個）。
        values: [i16; 16],
    },
    /// DNG の WarpRectilinear（歪曲に使う面）。
    Dng {
        /// 半径方向の係数。
        kr: [f64; 4],
        /// 光学中心（S の座標）。
        center: (f64, f64),
        /// 半径の基準（S の画素。中心から S の最も遠い隅まで）。
        radius: f64,
        /// 接線方向の係数が 0 でない（使わない。警告する）。
        has_tangential: bool,
    },
}

impl ProfileDistortion {
    /// RAW の埋め込みの情報から作る（歪曲の情報がなければ `None`）。
    pub fn from_raw(raw: &RawImage) -> Option<Self> {
        Self::from_data(raw.lens_correction.as_ref()?, (raw.width, raw.height))
    }

    /// 埋め込みの情報と S の寸法から作る。
    pub fn from_data(data: &LensCorrectionData, sensor: (u32, u32)) -> Option<Self> {
        match data {
            LensCorrectionData::SonyArw(s) => {
                let n = s.distortion.len();
                if !(2..=16).contains(&n) {
                    return None;
                }
                let mut values = [0i16; 16];
                values[..n].copy_from_slice(&s.distortion);
                Some(Self::Sony { n: n as u8, values })
            }
            LensCorrectionData::DngOpcode(d) => {
                let w = d.warp.as_ref()?;
                let plane = if w.planes.len() == 3 {
                    &w.planes[1]
                } else {
                    w.planes.first()?
                };
                let (center, radius) = dng_center(w.center, sensor);
                Some(Self::Dng {
                    kr: plane.kr,
                    center,
                    radius,
                    has_tangential: w.planes.iter().any(|p| p.kt.iter().any(|&k| k != 0.0)),
                })
            }
        }
    }

    /// 中心（S の座標）と半径の基準（S の画素）。Sony は既定の切り抜き（O）から。
    fn center_and_radius(&self, crop: PixelRect) -> ((f64, f64), f64) {
        match self {
            Self::Sony { .. } => crop_center_and_half_diagonal(crop),
            Self::Dng { center, radius, .. } => (*center, *radius),
        }
    }

    /// 半径 `rho`（`r / R`）での取る位置の倍率（適用量 `t`。0 なら 1）。
    fn ratio(&self, rho: f64, t: f64) -> f64 {
        match self {
            Self::Sony { n, values } => {
                let n = usize::from(*n);
                let knots = sony_knots(n);
                let v: Vec<f64> = values[..n].iter().map(|&x| f64::from(x)).collect();
                1.0 + t * interp_knots(&knots, &v, rho) * SONY_DISTORTION_UNIT
            }
            Self::Dng { kr, .. } => 1.0 + t * (dng_radial(kr, rho) - 1.0),
        }
    }

    /// 接線方向の係数を使わずに無視したか（DNG）。
    pub fn ignores_tangential(&self) -> bool {
        matches!(
            self,
            Self::Dng {
                has_tangential: true,
                ..
            }
        )
    }
}

/// ステージ 9 の歪曲補正（D → O の半径方向の写像。[`CoordinateFrames::distortion`]）。`Copy`。
///
/// D の点 `d`（O の画素の座標）の値を、O の `c + K(u) · (d − c)` から取る（`u = |d − c|² / R²`。
/// `K` は自動の倍率を含む）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadialDistortion {
    /// 中心（O の画素の座標）。
    pub center: (f64, f64),
    /// `1 / R²`（O の画素の単位）。
    pub inv_r2: f64,
    /// 自動の倍率 `z`（モジュールの doc）。
    pub zoom: f64,
    /// 表の `u` の上端。
    pub u_max: f32,
    /// `K(u)` の表（`u` = 0〜`u_max` の等間隔）。
    pub lut: [f32; DISTORTION_LUT_LEN],
}

impl RadialDistortion {
    /// 座標系（O の枠と、プロファイルの歪曲 [`CoordinateFrames::lens_geometry`]）と丸めた設定から作る。
    /// 歪曲の補正がなければ `None`（版 1 も `None`）。
    pub fn for_settings(
        frames: &CoordinateFrames,
        version: ProcessVersion,
        lens: &LensCorrection,
    ) -> Option<Self> {
        if !version.has_v1_features() {
            return None;
        }
        let t = if lens.profile_enabled {
            f64::from(lens.distortion_scale) / 100.0
        } else {
            0.0
        };
        let profile = frames.lens_geometry.filter(|_| t > 0.0);
        let a = f64::from(lens.distortion) / 100.0 * MANUAL_DISTORTION_MAX;
        if profile.is_none() && a == 0.0 {
            return None;
        }
        let (ow, oh) = frames.oriented_size();
        let (ow, oh) = (f64::from(ow), f64::from(oh));
        let (_, r_o) = crop_center_and_half_diagonal(frames.default_crop);
        // 中心（O の座標）と半径の基準。
        let (center, r) = match &profile {
            Some(p) => {
                let (cs, r) = p.center_and_radius(frames.default_crop);
                (frames.sensor_to_oriented(cs.0, cs.1), r)
            }
            None => ((ow * 0.5, oh * 0.5), r_o),
        };
        let k_total = |dist: f64| -> f64 {
            let km = 1.0 - a * (dist / r_o).powi(2);
            match &profile {
                Some(p) => km * p.ratio(km * dist / r, t),
                None => km,
            }
        };
        // 自動の倍率: D の枠の辺の上の点が、O の内側に入る最大の z。
        let mut samples = Vec::with_capacity(4 * ZOOM_EDGE_SAMPLES + 4);
        for i in 0..=ZOOM_EDGE_SAMPLES {
            let s = i as f64 / ZOOM_EDGE_SAMPLES as f64;
            samples.extend([(s * ow, 0.0), (s * ow, oh), (0.0, s * oh), (ow, s * oh)]);
        }
        let foot = |v: f64, len: f64| v.clamp(0.0, len);
        samples.extend([
            (foot(center.0, ow), 0.0),
            (foot(center.0, ow), oh),
            (0.0, foot(center.1, oh)),
            (ow, foot(center.1, oh)),
        ]);
        let inside = |z: f64| {
            samples.iter().all(|&(x, y)| {
                let (dx, dy) = (x - center.0, y - center.1);
                let k = z * k_total(z * dx.hypot(dy));
                let (ox, oy) = (center.0 + k * dx, center.1 + k * dy);
                (-ZOOM_TOLERANCE_PX..=ow + ZOOM_TOLERANCE_PX).contains(&ox)
                    && (-ZOOM_TOLERANCE_PX..=oh + ZOOM_TOLERANCE_PX).contains(&oy)
            })
        };
        let (mut lo, mut hi) = ZOOM_RANGE;
        if !inside(lo) {
            // 極端な設定（写像が単調でないなど）。拡大の下限で止める。
            hi = lo;
        }
        for _ in 0..60 {
            if hi - lo <= 1e-12 {
                break;
            }
            let mid = 0.5 * (lo + hi);
            if inside(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let zoom = lo;
        // 表: D の最も遠い隅（余裕つき）まで。
        let far = farthest_corner(center, ow, oh) * LUT_RADIUS_MARGIN;
        let u_max = ((far / r) * (far / r)).max(1e-6) as f32;
        let mut lut = [0f32; DISTORTION_LUT_LEN];
        for (i, v) in lut.iter_mut().enumerate() {
            let u = f64::from(u_max) * i as f64 / LUT_INTERVALS as f64;
            let dist = u.sqrt() * r;
            *v = (zoom * k_total(zoom * dist)) as f32;
        }
        // 補正が実質的にない（埋め込みの値が 0 など）なら、歪曲補正なしと同じにする（版 1 とビット単位で
        // 同じ結果を保つ）。
        if lut
            .iter()
            .all(|&v| (f64::from(v) - 1.0).abs() < IDENTITY_TOLERANCE)
        {
            return None;
        }
        Some(Self {
            center,
            inv_r2: 1.0 / (r * r),
            zoom,
            u_max,
            lut,
        })
    }

    /// 表の `(n − 1) / u_max`（f32。[`Lut1d`] と同じ値。GPU 版にも渡す）。
    pub fn inv_step(&self) -> f32 {
        (LUT_INTERVALS as f64 / f64::from(self.u_max)) as f32
    }

    /// 表を [`Lut1d`] にしたもの（ステージ 9 の計画・GPU 版に渡す）。
    pub fn to_lut1d(&self) -> Result<Lut1d> {
        Lut1d::from_values(0.0, self.u_max, self.lut.to_vec())
    }

    /// `u` での `K`（f64 で表を線形補間する。座標の変換・範囲の計算用）。
    pub fn eval_f64(&self, u: f64) -> f64 {
        let last = LUT_INTERVALS as f64;
        let t = (u * LUT_INTERVALS as f64 / f64::from(self.u_max)).clamp(0.0, last);
        let t = if t.is_nan() { 0.0 } else { t };
        let i = (t.floor() as usize).min(LUT_INTERVALS - 1);
        let f = t - i as f64;
        let a = f64::from(self.lut[i]);
        let b = f64::from(self.lut[i + 1]);
        a + (b - a) * f
    }

    /// D → O（連続座標。O の画素の座標）。
    pub fn undistorted_to_oriented(&self, x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = (x - self.center.0, y - self.center.1);
        let k = self.eval_f64((dx * dx + dy * dy) * self.inv_r2);
        (self.center.0 + k * dx, self.center.1 + k * dy)
    }

    /// O → D（[`undistorted_to_oriented`](Self::undistorted_to_oriented) の逆。同じ向きの上で、
    /// `K(r²/R²) · r` が O の距離になる `r` を二分法で求める）。
    pub fn oriented_to_undistorted(&self, x: f64, y: f64) -> (f64, f64) {
        let (ox, oy) = (x - self.center.0, y - self.center.1);
        let target = ox.hypot(oy);
        if target == 0.0 {
            return self.center;
        }
        let mapped = |r: f64| self.eval_f64(r * r * self.inv_r2) * r;
        let (mut lo, mut hi) = (0.0f64, target.max(1.0) * 4.0);
        while mapped(hi) < target && hi < 1e12 {
            hi *= 2.0;
        }
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if mapped(mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let r = 0.5 * (lo + hi);
        let s = r / target;
        (self.center.0 + ox * s, self.center.1 + oy * s)
    }
}

/// ステージ 7 の計画（周辺減光の利得と、R・B の取る位置の倍率。S の座標の中心のまわり）。
///
/// どの表も `u = |p − c|² / R²`（S の画素）の関数。色収差は R と B の両方を持つか、どちらも持たない。
#[derive(Debug, Clone, PartialEq)]
pub struct LensStagePlan {
    /// 中心（S の座標）。
    pub center: (f64, f64),
    /// `1 / R²`（S の画素の単位）。
    pub inv_r2: f64,
    /// 周辺減光の利得（なければ `None`）。
    pub vignetting: Option<Lut1d>,
    /// R・B の取る位置の倍率（なければ `None`）。
    pub chromatic_aberration: Option<(Lut1d, Lut1d)>,
    /// 色収差で取る位置が動く量の最大（S の画素）。入力の範囲の余白に使う。
    pub max_shift: f64,
}

/// ステージ 7 の画素ごとの計算に使う f32 の値（入力の格子ごと。GPU 版にも同じ値を渡す）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LensKernelParams {
    /// 中心（格子の連続座標）。
    pub center: [f32; 2],
    /// 格子の 1 画素あたりの S の画素の数（1 か 2）。
    pub sensor_per_grid: f32,
    /// `1 / R²`（S の画素の単位）。
    pub inv_r2: f32,
}

impl LensStagePlan {
    /// RAW（埋め込みの情報・既定の切り抜き）と丸めた設定から作る。補正がなければ `None`。
    pub fn new(raw: &RawImage, lens: &LensCorrection) -> Option<Self> {
        let data = raw.lens_correction.as_ref();
        let sensor = (raw.width, raw.height);
        let (o_center, r_o) = crop_center_and_half_diagonal(raw.visible_area());
        let t_v = if lens.profile_enabled {
            f64::from(lens.vignetting_scale) / 100.0
        } else {
            0.0
        };
        // プロファイルの周辺減光（中心と半径の基準、`ρ` → 利得）。
        type Gain = Box<dyn Fn(f64) -> f64>;
        let mut center = o_center;
        let mut radius = r_o;
        let mut vig_profile: Option<(Gain, f64)> = None;
        let mut ca_profile: Option<(Gain, Gain, f64)> = None;
        match data {
            Some(LensCorrectionData::SonyArw(s)) => {
                if t_v > 0.0 && !s.vignetting.is_empty() {
                    let knots = sony_knots(s.vignetting.len());
                    let v: Vec<f64> = s
                        .vignetting
                        .iter()
                        .map(|&x| {
                            2f64.powf(
                                0.5 - 2f64.powf(t_v * f64::from(x) * SONY_VIGNETTING_UNIT - 1.0),
                            )
                        })
                        .collect();
                    vig_profile = Some((
                        Box::new(move |rho| {
                            1.0 / interp_knots(&knots, &v, rho).max(VIGNETTING_FLOOR)
                        }),
                        r_o,
                    ));
                }
                if lens.remove_chromatic_aberration && !s.ca_red.is_empty() {
                    let knots = sony_knots(s.ca_red.len());
                    let to_f = |v: &[i16]| v.iter().map(|&x| f64::from(x)).collect::<Vec<f64>>();
                    let (red, blue) = (to_f(&s.ca_red), to_f(&s.ca_blue));
                    let k2 = knots.clone();
                    ca_profile = Some((
                        Box::new(move |rho| 1.0 + interp_knots(&knots, &red, rho) * SONY_CA_UNIT),
                        Box::new(move |rho| 1.0 + interp_knots(&k2, &blue, rho) * SONY_CA_UNIT),
                        r_o,
                    ));
                }
            }
            Some(LensCorrectionData::DngOpcode(d)) => {
                if let (true, Some(v)) = (t_v > 0.0, d.vignette) {
                    let (c, m) = dng_center(v.center, sensor);
                    center = c;
                    radius = m;
                    vig_profile = Some((
                        Box::new(move |rho| {
                            let r2 = rho * rho;
                            let poly = r2
                                * (v.k[0]
                                    + r2 * (v.k[1] + r2 * (v.k[2] + r2 * (v.k[3] + r2 * v.k[4]))));
                            1.0 + t_v * poly
                        }),
                        m,
                    ));
                }
                if let (true, Some(w)) = (lens.remove_chromatic_aberration, &d.warp)
                    && w.planes.len() == 3
                {
                    let (c, m) = dng_center(w.center, sensor);
                    if vig_profile.is_none() {
                        center = c;
                        radius = m;
                    }
                    let (kr, kg, kb) = (w.planes[0].kr, w.planes[1].kr, w.planes[2].kr);
                    ca_profile = Some((
                        Box::new(move |rho| dng_radial(&kr, rho) / dng_radial(&kg, rho)),
                        Box::new(move |rho| dng_radial(&kb, rho) / dng_radial(&kg, rho)),
                        m,
                    ));
                }
            }
            None => {}
        }
        let amount = f64::from(lens.vignetting_amount) / 100.0;
        let manual_ca = (
            1.0 - f64::from(lens.ca_red_cyan) / 100.0 * MANUAL_CA_MAX,
            1.0 - f64::from(lens.ca_blue_yellow) / 100.0 * MANUAL_CA_MAX,
        );
        let vig_active = vig_profile.is_some() || amount != 0.0;
        let ca_active = ca_profile.is_some() || manual_ca != (1.0, 1.0);
        if !vig_active && !ca_active {
            return None;
        }
        let m0 = f64::from(lens.vignetting_midpoint) / 100.0 * MANUAL_VIGNETTING_MIDPOINT_MAX;
        let manual_gain = |dist: f64| {
            if amount == 0.0 {
                return 1.0;
            }
            let rho = dist / r_o;
            let s = ((rho - m0) / (1.0 - m0)).clamp(0.0, 1.0);
            let w = s * s * (3.0 - 2.0 * s);
            2f64.powf(amount * MANUAL_VIGNETTING_MAX_EV * w)
        };
        // 表の範囲: S の最も遠い隅（余裕つき）まで。
        let far =
            farthest_corner(center, f64::from(sensor.0), f64::from(sensor.1)) * LUT_RADIUS_MARGIN;
        let u_max = ((far / radius) * (far / radius)).max(1e-6) as f32;
        let dist_at =
            |i: usize| (f64::from(u_max) * i as f64 / LUT_INTERVALS as f64).sqrt() * radius;
        let table = |f: &dyn Fn(f64) -> f64| -> Option<Lut1d> {
            let values = (0..=LUT_INTERVALS).map(|i| f(dist_at(i)) as f32).collect();
            Lut1d::from_values(0.0, u_max, values).ok()
        };
        let vignetting = if vig_active {
            table(&|dist| {
                let p = vig_profile
                    .as_ref()
                    .map_or(1.0, |(g, base)| g(dist / *base));
                p * manual_gain(dist)
            })
        } else {
            None
        };
        let mut max_shift = 0.0f64;
        let chromatic_aberration = if ca_active {
            let channel = |which: usize| {
                table(&|dist| {
                    let p = ca_profile.as_ref().map_or(1.0, |(r, b, base)| {
                        if which == 0 {
                            r(dist / *base)
                        } else {
                            b(dist / *base)
                        }
                    });
                    let m = p * if which == 0 { manual_ca.0 } else { manual_ca.1 };
                    if m.is_finite() {
                        m.clamp(1.0 - MAX_CA_RATIO_DEVIATION, 1.0 + MAX_CA_RATIO_DEVIATION)
                    } else {
                        1.0
                    }
                })
            };
            let (r, b) = (channel(0)?, channel(2)?);
            for lut in [&r, &b] {
                for (i, &m) in lut.values().iter().enumerate() {
                    max_shift = max_shift.max((f64::from(m) - 1.0).abs() * dist_at(i));
                }
            }
            Some((r, b))
        } else {
            None
        };
        Some(Self {
            center,
            inv_r2: 1.0 / (radius * radius),
            vignetting,
            chromatic_aberration,
            max_shift,
        })
    }

    /// 入力の格子（格子の 1 画素あたり `sensor_per_grid` 画素の S）の画素ごとの値。
    pub fn kernel_params(&self, sensor_per_grid: u32) -> LensKernelParams {
        let k = f64::from(sensor_per_grid.max(1));
        LensKernelParams {
            center: [(self.center.0 / k) as f32, (self.center.1 / k) as f32],
            sensor_per_grid: k as f32,
            inv_r2: self.inv_r2 as f32,
        }
    }

    /// 出力の範囲を計算するのに必要な入力の範囲の余白（格子の画素。補間のタップ 2 ＋ 丸めの余白 1 を含む）。
    /// 色収差の倍率は [`MAX_CA_RATIO_DEVIATION`] の中に収めているので有限だが、計算はあふれないよう飽和させる
    /// （f64 から u32 への変換は飽和し、NaN は 0）。
    pub fn margin(&self, sensor_per_grid: u32) -> u32 {
        if self.chromatic_aberration.is_none() {
            return 0;
        }
        ((self.max_shift / f64::from(sensor_per_grid.max(1))).ceil() as u32).saturating_add(3)
    }
}

/// 版 2 で、設定がステージ 7 の補正（周辺減光・色収差・プロファイル）を求めているか（RAW 以外の写真で
/// 使われないことを知らせるため）。
pub fn requests_sensor_corrections(version: ProcessVersion, lens: &LensCorrection) -> bool {
    version.has_v1_features()
        && (lens.profile_enabled
            || lens.remove_chromatic_aberration
            || lens.vignetting_amount != 0.0
            || lens.ca_red_cyan != 0.0
            || lens.ca_blue_yellow != 0.0)
}

#[cfg(test)]
mod tests;
