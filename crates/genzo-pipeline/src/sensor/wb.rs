//! ステージ 3: ホワイトバランス（docs/04_architecture.md の 2.1 節、2.6 節の B1。01 の DEV-03）。
//!
//! CFA（B0）の位置ごとに WB の係数（G = 1）を掛けて、被写体の中立色が (1, 1, 1) になる B1 の値に
//! する（デモザイクの前に CFA へ適用する。2.1 節の補足）。
//!
//! # 係数の決め方（[`resolve_multipliers`]）
//!
//! | 設定 | 係数 |
//! |---|---|
//! | `AsShot` | RAW の撮影時の係数（`RawImage::as_shot_wb`。R, G1, B, G2）を G1 で割ったもの |
//! | `Preset` | プリセットの色温度・tint（genzo-model の `WbPreset::temperature_tint`）から下と同じ方法で |
//! | `Custom` | genzo-color の `wb_multipliers_from_temperature_tint`（カメラ行列で白の XYZ → カメラ RGB）で R・B を求め、G1 = 1 |
//!
//! G2（B と同じ行の緑）の係数は、どの設定でも撮影時の係数の G2 / G1 とする（**仮置き**: G1 と
//! G2 の感度の差はセンサーの性質で、光源によらないとみなす。実機の値は PoC-2 で確認する）。
//! カメラ行列がない場合の扱いは [`crate::sensor`] のモジュールの doc を参照（BT.2020 の原色の
//! カメラとみなし、警告を返す）。
//!
//! # 飽和した画素の扱い（仮の処理。dcraw の `-H 0` に相当する切りそろえ）
//!
//! ハイライト復元（ステージ 4）は v1 で実装する。MVP でも、飽和した部分がマゼンタに色かぶりする
//! のを避けるため、WB を掛けた後に、**飽和した画素とその近傍の値を「すべてのチャンネルが飽和する
//! 値」（共通の白 = 係数の最小値 × 1）で切りそろえる**（`v = min(v, 共通の白)`）。
//!
//! - 理由: 白レベルで切れたチャンネルは WB を掛けると係数の分だけ違う値になる（例: G は 1、R は
//!   2）。さらに、G が飽和して R・B が飽和していない明るい部分では、R・B だけが 1 を超えて残り、
//!   マゼンタに見える。共通の白で切りそろえると、その部分は中立（白）になる。
//! - dcraw の `-H 0` は画像全体を共通の白で切る。これは飽和していない明るい色（例: 日の当たった
//!   赤い花の R）まで切り、02 の IQ-02「途中でクリッピングしない」に反する。そのため既定では
//!   **飽和した画素から半径 [`SATURATION_CLIP_RADIUS`] 画素以内（CFA の画素）だけ** を切りそろえる
//!   （[`SaturationClip::Neighborhood`]）。dcraw と同じ全体の切りそろえ（[`SaturationClip::Global`]）と、
//!   切りそろえない設定（[`SaturationClip::Off`]。比較用）も選べる。どれにするかは PoC-3 で実写を
//!   見て決め、ステージ 4（v1）で置き換える。
//!
//! 計算の順序（GPU 版も同じ）: `v = v × 係数[位置]`、近傍に飽和した画素があれば `v = min(v, 共通の白)`。

use genzo_color::Mat3;
use genzo_color::white_balance::wb_multipliers_from_temperature_tint;
use genzo_model::WhiteBalance;
use rayon::prelude::*;

use crate::error::{PipelineError, Result};
use crate::image::{CfaImage, Roi};

/// 飽和の切りそろえをする範囲の半径の既定値（CFA の画素。**仮置き**）。
///
/// 半径 1（3 × 3）で、飽和した画素の周りの R・G・B のすべてが切りそろえの対象になる（ベイヤー
/// 配列の 3 × 3 の窓には必ず 3 色が入る）。もう 1 画素広げて、デモザイクが色差を補間するときに
/// 切りそろえた範囲の境目の値を拾いにくくした。PoC-3 で実写を見て決める。
pub const SATURATION_CLIP_RADIUS: u32 = 2;

/// WB の係数の上限（G = 1 として）。genzo-raw が撮影時の係数に設けている上限と同じ値。
/// 色温度・tint から求めた係数がこれを超えたら、カメラ行列か設定が不正とみなす。
pub const MAX_WB_MULTIPLIER: f32 = 1000.0;

/// 飽和した画素の切りそろえの方法（モジュールの doc を参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SaturationClip {
    /// 切りそろえない（比較用。飽和した部分がマゼンタになりうる）。
    Off,
    /// 飽和した画素から半径 `radius` 画素以内（チェビシェフ距離。CFA の画素）を切りそろえる。
    Neighborhood {
        /// 半径（CFA の画素）。
        radius: u32,
    },
    /// 画像全体を切りそろえる（dcraw の `-H 0` と同じ）。
    Global,
}

impl Default for SaturationClip {
    fn default() -> Self {
        SaturationClip::Neighborhood {
            radius: SATURATION_CLIP_RADIUS,
        }
    }
}

impl SaturationClip {
    /// 出力の範囲の外に必要な余白（近傍の飽和の印を見るため）。
    pub const fn margin(self) -> u32 {
        match self {
            SaturationClip::Neighborhood { radius } => radius,
            SaturationClip::Off | SaturationClip::Global => 0,
        }
    }
}

/// WB の係数（GPU 版にも同じ値を渡す）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WbCoefficients {
    /// CFA の位置ごとの係数（R, G1, B, G2 の順。genzo-raw の `CfaColor::index`）。G1 = 1。
    pub multipliers: [f32; 4],
    /// 共通の白（すべてのチャンネルが飽和する値 = 係数の最小値）。
    pub clip_level: f32,
}

impl WbCoefficients {
    /// 係数から作る。係数は正の有限の値で [`MAX_WB_MULTIPLIER`] 以下であること。
    pub fn new(multipliers: [f32; 4]) -> Result<Self> {
        if !multipliers
            .iter()
            .all(|&m| m.is_finite() && m > 0.0 && m <= MAX_WB_MULTIPLIER)
        {
            return Err(PipelineError::InvalidArgument(format!(
                "WB の係数が不正です（{multipliers:?}。0 より大きく {MAX_WB_MULTIPLIER} 以下が必要）"
            )));
        }
        let clip_level = multipliers.iter().copied().fold(f32::INFINITY, f32::min);
        Ok(Self {
            multipliers,
            clip_level,
        })
    }
}

/// 現像設定の WB から、CFA の位置ごとの係数（R, G1, B, G2。G1 = 1）を求める。
///
/// `as_shot` は RAW の撮影時の係数（[`genzo_raw::RawImage::as_shot_wb`]。検証済みであること）、
/// `cam_xyz` は「XYZ（D65）→ カメラ RGB」の行列（カメラ行列がないときは呼び出し側が代わりの行列を
/// 渡す）。色温度・tint は [`genzo_model::DevelopSettings::normalized`] で範囲に丸めておくこと。
pub fn resolve_multipliers(
    wb: &WhiteBalance,
    as_shot: [f32; 4],
    cam_xyz: &Mat3,
) -> Result<[f32; 4]> {
    let g1 = f64::from(as_shot[1]);
    if !(g1.is_finite() && g1 > 0.0) {
        return Err(PipelineError::InvalidArgument(format!(
            "撮影時の WB の係数が不正です（{as_shot:?}）"
        )));
    }
    let g2 = (f64::from(as_shot[3]) / g1) as f32;
    let (temperature_k, tint) = match *wb {
        WhiteBalance::AsShot => {
            let m = as_shot.map(|v| (f64::from(v) / g1) as f32);
            return Ok([m[0], 1.0, m[2], m[3]]);
        }
        WhiteBalance::Preset(p) => p.temperature_tint(),
        WhiteBalance::Custom {
            temperature_k,
            tint,
        } => (temperature_k, tint),
    };
    let m =
        wb_multipliers_from_temperature_tint(cam_xyz, f64::from(temperature_k), f64::from(tint))?;
    Ok([m[0] as f32, 1.0, m[2] as f32, g2])
}

/// 近傍（半径 `r`。チェビシェフ距離）に飽和した画素があるかの表（`w × h`）。
fn dilate(sat: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    if r == 0 {
        return sat.to_vec();
    }
    // 横方向。
    let mut row = vec![false; w * h];
    row.par_chunks_mut(w)
        .zip(sat.par_chunks(w))
        .for_each(|(out, src)| {
            for (x, o) in out.iter_mut().enumerate() {
                let lo = x.saturating_sub(r);
                let hi = (x + r + 1).min(w);
                *o = src[lo..hi].iter().any(|&s| s);
            }
        });
    // 縦方向。
    let mut out = vec![false; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, o)| {
        let lo = y.saturating_sub(r);
        let hi = (y + r + 1).min(h);
        for (x, v) in o.iter_mut().enumerate() {
            *v = (lo..hi).any(|yy| row[yy * w + x]);
        }
    });
    out
}

/// CFA（B0）に WB の係数を掛け、飽和した画素を切りそろえる。出力は範囲 `out_roi` の CFA（B1 の値）。
///
/// `input` の範囲は `out_roi` を [`SaturationClip::margin`] だけ広げた範囲を含むこと。飽和の印は
/// そのまま引き継ぐ（ステージ 4 が使う）。
pub fn apply_white_balance(
    input: &CfaImage,
    coeffs: &WbCoefficients,
    clip: SaturationClip,
    out_roi: Roi,
) -> Result<CfaImage> {
    let need = out_roi.expand(clip.margin());
    if !input.roi().contains_roi(&need) {
        return Err(PipelineError::InvalidRoi {
            roi: input.roi(),
            reason: "WB の入力が必要な範囲（飽和の近傍の余白）を覆っていない",
        });
    }
    let (iw, ih) = (input.width() as usize, input.height() as usize);
    let near = match clip {
        SaturationClip::Neighborhood { radius } => {
            Some(dilate(input.saturated(), iw, ih, radius as usize))
        }
        SaturationClip::Off | SaturationClip::Global => None,
    };
    let (ox, oy) = out_roi
        .offset_in(&input.roi())
        .expect("need が out_roi を含む");
    let mut out = input.window(out_roi)?;
    let ow = out.width() as usize;
    let pattern = out.pattern();
    let m = coeffs.multipliers;
    let clip_level = coeffs.clip_level;
    let (data, _) = out.parts_mut();
    data.par_chunks_mut(ow).enumerate().for_each(|(ly, row)| {
        for (lx, v) in row.iter_mut().enumerate() {
            let k = pattern.color_at(lx, ly).index();
            let x = *v * m[k];
            let clipped = match (&near, clip) {
                (_, SaturationClip::Global) => true,
                (Some(near), _) => near[(oy + ly) * iw + ox + lx],
                _ => false,
            };
            *v = if clipped { x.min(clip_level) } else { x };
        }
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use genzo_color::space::{XYZ_TO_BT2020, XYZ_TO_SRGB};
    use genzo_model::WbPreset;
    use genzo_raw::CfaPattern;

    use super::*;

    const AS_SHOT: [f32; 4] = [2.0, 1.0, 1.5, 1.02];

    #[test]
    fn as_shot_is_normalized_to_green() {
        let m = resolve_multipliers(&WhiteBalance::AsShot, AS_SHOT, &XYZ_TO_BT2020).unwrap();
        assert_eq!(m, AS_SHOT);
        let m = resolve_multipliers(
            &WhiteBalance::AsShot,
            [2.002, 1.001, 1.5015, 1.001],
            &XYZ_TO_BT2020,
        )
        .unwrap();
        assert_eq!(m[1], 1.0);
        assert!(
            (m[0] - 2.0).abs() < 1e-6 && (m[2] - 1.5).abs() < 1e-6 && (m[3] - 1.0).abs() < 1e-6
        );
        assert!(
            resolve_multipliers(&WhiteBalance::AsShot, [2.0, 0.0, 1.0, 1.0], &XYZ_TO_BT2020)
                .is_err()
        );
    }

    #[test]
    fn custom_and_preset_use_the_camera_matrix() {
        // D65 に近い色温度（6504 K、tint 0 付近）では、BT.2020 のカメラの係数はほぼ (1, 1, 1)。
        let wb = WhiteBalance::Custom {
            temperature_k: 6504.0,
            tint: 0.0,
        };
        let m = resolve_multipliers(&wb, AS_SHOT, &XYZ_TO_BT2020).unwrap();
        assert_eq!(m[1], 1.0);
        assert!(
            (m[0] - 1.0).abs() < 0.02 && (m[2] - 1.0).abs() < 0.02,
            "{m:?}"
        );
        // G2 は撮影時の G2 / G1。
        assert_eq!(m[3], 1.02);
        // 色温度が低い（赤い光源）ほど R の係数は小さく、B の係数は大きい。
        let warm = resolve_multipliers(
            &WhiteBalance::Custom {
                temperature_k: 3000.0,
                tint: 0.0,
            },
            AS_SHOT,
            &XYZ_TO_SRGB,
        )
        .unwrap();
        let cool = resolve_multipliers(
            &WhiteBalance::Custom {
                temperature_k: 9000.0,
                tint: 0.0,
            },
            AS_SHOT,
            &XYZ_TO_SRGB,
        )
        .unwrap();
        assert!(warm[0] < cool[0] && warm[2] > cool[2], "{warm:?} {cool:?}");
        // プリセットは色温度・tint の指定と同じ。
        let (t, tint) = WbPreset::Tungsten.temperature_tint();
        let preset = resolve_multipliers(
            &WhiteBalance::Preset(WbPreset::Tungsten),
            AS_SHOT,
            &XYZ_TO_SRGB,
        )
        .unwrap();
        let custom = resolve_multipliers(
            &WhiteBalance::Custom {
                temperature_k: t,
                tint,
            },
            AS_SHOT,
            &XYZ_TO_SRGB,
        )
        .unwrap();
        assert_eq!(preset, custom);
    }

    #[test]
    fn coefficients_are_validated() {
        let c = WbCoefficients::new([2.0, 1.0, 1.5, 0.98]).unwrap();
        assert_eq!(c.clip_level, 0.98);
        assert!(WbCoefficients::new([0.0, 1.0, 1.0, 1.0]).is_err());
        assert!(WbCoefficients::new([f32::NAN, 1.0, 1.0, 1.0]).is_err());
        assert!(WbCoefficients::new([2000.0, 1.0, 1.0, 1.0]).is_err());
    }

    fn cfa(pattern: CfaPattern, values: [f32; 4], sat: impl Fn(u32, u32) -> bool) -> CfaImage {
        let roi = Roi::full(8, 8);
        let mut img = CfaImage::from_fn(roi, pattern, |x, y| {
            values[pattern.color_at(x as usize, y as usize).index()]
        })
        .unwrap();
        let w = img.width();
        let (_, s) = img.parts_mut();
        for (i, v) in s.iter_mut().enumerate() {
            *v = sat(i as u32 % w, i as u32 / w);
        }
        img
    }

    #[test]
    fn gray_becomes_neutral_for_every_pattern() {
        let coeffs = WbCoefficients::new(AS_SHOT).unwrap();
        for p in CfaPattern::ALL {
            // 逆 WB をかけたグレー 0.18 の CFA。
            let img = cfa(p, AS_SHOT.map(|m| 0.18 / m), |_, _| false);
            let out = apply_white_balance(
                &img,
                &coeffs,
                SaturationClip::default(),
                Roi::new(2, 2, 4, 4),
            )
            .unwrap();
            assert_eq!(out.roi(), Roi::new(2, 2, 4, 4));
            assert_eq!(out.pattern(), p);
            for &v in out.data() {
                assert!((v - 0.18).abs() < 1e-6, "{p}: {v}");
            }
        }
    }

    #[test]
    fn saturated_neighborhood_is_clipped_to_the_common_white() {
        let coeffs = WbCoefficients::new(AS_SHOT).unwrap();
        // G だけが飽和した明るい部分（R・B は飽和していないが、WB 後は 1 を超える）。
        let values = [0.65, 1.0, 0.8, 1.0];
        let sat =
            |x: u32, y: u32| CfaPattern::RGGB.color_at(x as usize, y as usize).is_green() && x < 4;
        let img = cfa(CfaPattern::RGGB, values, sat);
        let roi = Roi::new(2, 2, 4, 4);

        let off = apply_white_balance(&img, &coeffs, SaturationClip::Off, roi).unwrap();
        // 切りそろえないと R = 1.3、B = 1.2、G = 1.0（マゼンタ）。
        assert!((off.value(0, 0).unwrap() - 1.3).abs() < 1e-6);

        let near = apply_white_balance(&img, &coeffs, SaturationClip::default(), roi).unwrap();
        // x = 2..6 の範囲は、どれも半径 2 以内に飽和した G（x < 4）がある。
        for y in 0..4 {
            for x in 0..4 {
                let v = near.value(x, y).unwrap();
                assert!(v <= coeffs.clip_level, "({x}, {y}) {v}");
                assert!((v - 1.0).abs() < 1e-6);
            }
        }
        // 半径 0 なら飽和した画素だけ（R・B は 1 を超えたまま）。
        let only = apply_white_balance(
            &img,
            &coeffs,
            SaturationClip::Neighborhood { radius: 0 },
            roi,
        )
        .unwrap();
        assert!((only.value(0, 0).unwrap() - 1.3).abs() < 1e-6);
        // 飽和の印は引き継ぐ。
        assert!(near.is_saturated(1, 0));
    }

    #[test]
    fn unsaturated_bright_colors_are_kept_except_in_global_mode() {
        let coeffs = WbCoefficients::new(AS_SHOT).unwrap();
        // 飽和していない明るい赤（R の生の値 0.9 → WB 後 1.8）。
        let img = cfa(CfaPattern::BGGR, [0.9, 0.2, 0.1, 0.2], |_, _| false);
        let roi = Roi::new(2, 2, 4, 4);
        let near = apply_white_balance(&img, &coeffs, SaturationClip::default(), roi).unwrap();
        let red = |c: &CfaImage| {
            (0..4)
                .flat_map(|y| (0..4).map(move |x| (x, y)))
                .find(|&(x, y)| c.color_at(x, y) == genzo_raw::CfaColor::Red)
                .map(|(x, y)| c.value(x, y).unwrap())
                .unwrap()
        };
        assert!((red(&near) - 1.8).abs() < 1e-6);
        let global = apply_white_balance(&img, &coeffs, SaturationClip::Global, roi).unwrap();
        assert_eq!(red(&global), coeffs.clip_level);
    }

    #[test]
    fn input_must_cover_the_margin() {
        let coeffs = WbCoefficients::new(AS_SHOT).unwrap();
        let img = cfa(CfaPattern::RGGB, [0.1; 4], |_, _| false);
        assert!(
            apply_white_balance(&img, &coeffs, SaturationClip::default(), Roi::full(8, 8)).is_err()
        );
        assert!(apply_white_balance(&img, &coeffs, SaturationClip::Off, Roi::full(8, 8)).is_ok());
        assert!(
            apply_white_balance(&img, &coeffs, SaturationClip::Global, Roi::new(1, 1, 8, 8))
                .is_err()
        );
    }

    #[test]
    fn dilation_matches_brute_force() {
        let (w, h) = (9usize, 7usize);
        let sat: Vec<bool> = (0..w * h).map(|i| i % 13 == 0 || i == 40).collect();
        for r in 0..4 {
            let d = dilate(&sat, w, h, r);
            for y in 0..h {
                for x in 0..w {
                    let mut any = false;
                    for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                        for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                            any |= sat[yy * w + xx];
                        }
                    }
                    assert_eq!(d[y * w + x], any, "r={r} ({x}, {y})");
                }
            }
        }
    }
}
