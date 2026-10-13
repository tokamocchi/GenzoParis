use genzo_raw::CfaPattern;

use super::*;
use crate::sensor::wb::SATURATION_CLIP_RADIUS;

/// WB の係数（R, G1, B, G2）。R の係数が大きく、B が中くらいのふつうの昼光の値。
const MULT: [f32; 4] = [2.0, 1.0, 1.5, 1.0];

/// WB を掛けた後（ステージ 3 の出力。切りそろえない）の CFA。`scene(x, y)` は S の座標の B1 の値（R, G, B）。
/// 白レベル（RAW の 1）で切れたチャンネルは、値を WB の係数 × 1 にして飽和の印を付ける。
fn wb_cfa(roi: Roi, pattern: CfaPattern, scene: impl Fn(i32, i32) -> [f32; 3]) -> CfaImage {
    let mut data = Vec::new();
    let mut sat = Vec::new();
    for ly in 0..roi.height {
        for lx in 0..roi.width {
            let (x, y) = (roi.x + lx as i32, roi.y + ly as i32);
            let color = pattern.color_at(lx as usize, ly as usize);
            let m = MULT[color.index()];
            let v = scene(x, y)[color.rgb_channel()];
            let raw = v / m;
            if raw >= 1.0 {
                data.push(m);
                sat.push(true);
            } else {
                data.push(raw * m);
                sat.push(false);
            }
        }
    }
    CfaImage::new(roi, pattern, data, sat).unwrap()
}

fn plan(method: HighlightMethod, amount: f32, baseline: SaturationClip) -> HighlightPlan {
    let wb = WbCoefficients::new(MULT).unwrap();
    HighlightPlan::new(&HighlightRecovery { method, amount }, &wb, baseline).unwrap()
}

/// 出力の CFA の、2 × 2 の組 `(qx, qy)`（S の座標の半分）の (R, G, B)（G は 2 つの平均）。
fn quad_rgb(cfa: &CfaImage, qx: i32, qy: i32) -> [f32; 3] {
    let roi = cfa.roi();
    let mut out = [0.0f32; 3];
    for dy in 0..2 {
        for dx in 0..2 {
            let lx = (2 * qx + dx - roi.x) as u32;
            let ly = (2 * qy + dy - roi.y) as u32;
            let color = cfa.color_at(lx, ly);
            let v = cfa.value(lx, ly).unwrap();
            if color.is_green() {
                out[1] += 0.5 * v;
            } else {
                out[color.rgb_channel()] = v;
            }
        }
    }
    out
}

/// 横方向の明るさの勾配（x = 0 で 0.6、x = w で 3.0）。
fn ramp(x: i32, w: u32) -> f32 {
    0.6 + 2.4 * x as f32 / w as f32
}

const W: u32 = 200;
const H: u32 = 24;

fn full_roi() -> Roi {
    Roi::new(-16, -16, W + 32, H + 32)
}

fn out_roi() -> Roi {
    Roi::new(0, 0, W, H)
}

#[test]
fn clip_and_zero_amount_are_not_planned() {
    let wb = WbCoefficients::new(MULT).unwrap();
    for hr in [
        HighlightRecovery::default(),
        HighlightRecovery {
            method: HighlightMethod::Reconstruct,
            amount: 0.0,
        },
        HighlightRecovery {
            method: HighlightMethod::Clip,
            amount: 100.0,
        },
    ] {
        assert!(HighlightPlan::new(&hr, &wb, SaturationClip::default()).is_none());
    }
    let p = plan(HighlightMethod::Neutral, 40.0, SaturationClip::default());
    assert_eq!(p.mode, HighlightMode::Neutral);
    assert!((p.amount - 0.4).abs() < 1e-7);
    assert_eq!(p.clip_level, 1.0);
    assert_eq!(p.margin(), 2 * HR_SEARCH_RADIUS + 2);
    assert!(p.margin() >= SATURATION_CLIP_RADIUS);
}

#[test]
fn neutral_highlights_are_recovered_exactly_up_to_the_last_channel() {
    // 無彩色の勾配: G は X = 1、B は 1.5、R は 2 で飽和する。どの方式でも、すべてのチャンネルが飽和するまでは
    // 本当の値（X, X, X）に戻り、それより明るいところは、すべてが飽和する最小の無彩色（2, 2, 2）になる。
    for method in [HighlightMethod::Neutral, HighlightMethod::Reconstruct] {
        for pattern in CfaPattern::ALL {
            let cfa = wb_cfa(full_roi(), pattern, |x, _| [ramp(x, W); 3]);
            let p = plan(method, 100.0, SaturationClip::default());
            let out = recover_highlights(&cfa, &p, out_roi()).unwrap();
            let slope = 2.4 / W as f32;
            for qy in 0..(H / 2) as i32 {
                for qx in 0..(W / 2) as i32 {
                    let xc = ramp(2 * qx, W) + 0.5 * slope;
                    let rgb = quad_rgb(&out, qx, qy);
                    for c in 0..3 {
                        // 組の中の画素の位置の違い（勾配の 1 画素分）だけずれる。
                        assert!(
                            (rgb[c] - xc.min(2.0)).abs() <= 1.5 * slope + 1e-5,
                            "{method:?} {pattern:?} ({qx}, {qy}) {rgb:?} {xc}"
                        );
                    }
                }
            }
        }
    }
}

/// 色の付いた勾配（本当の値は (0.5, 0.8, 1.0) · X。G は X = 1.25、B は 1.5 で飽和し、R は飽和しない）。
fn sky(x: i32, _y: i32) -> [f32; 3] {
    let v = ramp(x, W);
    [0.5 * v, 0.8 * v, v]
}

/// 色相の角度（度。反対色の簡単な軸: a = R − G、b = B − (R + G) / 2）。
fn hue(rgb: [f32; 3]) -> f32 {
    let a = rgb[0] - rgb[1];
    let b = rgb[2] - 0.5 * (rgb[0] + rgb[1]);
    b.atan2(a).to_degrees()
}

/// 方式ごとの出力の組の (R, G, B)（行の 1 つ目の組の並び）。`method` が `None` なら復元しない（ステージ 3 の
/// 切りそろえ `baseline`）。
fn sky_row(method: Option<HighlightMethod>, baseline: SaturationClip) -> Vec<[f32; 3]> {
    let cfa = wb_cfa(full_roi(), CfaPattern::RGGB, sky);
    let out = match method {
        Some(m) => recover_highlights(&cfa, &plan(m, 100.0, baseline), out_roi()).unwrap(),
        None => crate::sensor::wb::apply_white_balance(
            &cfa_raw_from_wb(&cfa),
            &WbCoefficients::new(MULT).unwrap(),
            baseline,
            out_roi(),
        )
        .unwrap(),
    };
    (0..(W / 2) as i32)
        .map(|qx| quad_rgb(&out, qx, 4))
        .collect()
}

/// WB を掛けた CFA を、WB を掛ける前（ステージ 2 の出力）に戻す（ステージ 3 の切りそろえと比べるため）。
fn cfa_raw_from_wb(cfa: &CfaImage) -> CfaImage {
    let mut out = cfa.clone();
    let pattern = out.pattern();
    let w = out.width() as usize;
    let (data, _) = out.parts_mut();
    for (i, v) in data.iter_mut().enumerate() {
        let k = pattern.color_at(i % w, i / w).index();
        *v /= MULT[k];
    }
    out
}

#[test]
fn reconstruction_keeps_the_hue_across_the_clip_boundary() {
    let rec = sky_row(
        Some(HighlightMethod::Reconstruct),
        SaturationClip::default(),
    );
    // G が飽和し始める組（X ≥ 1.25）から 3 組の間は、近傍の比で G を復元するので、本当の比 G / R = 1.6 に戻る。
    let first = (0..rec.len())
        .find(|&q| ramp(2 * q as i32 + 1, W) * 0.8 >= 1.0)
        .unwrap();
    for (q, rgb) in rec.iter().enumerate().skip(first).take(3) {
        let ratio = rgb[1] / rgb[0];
        assert!(
            (ratio - 1.6).abs() < 0.03,
            "組 {q}: {rgb:?}（G / R = {ratio}）"
        );
    }
    // 飽和の境界の色相の段差: 境界の前後 2 組の、隣の組との色相の差の最大。復元すると小さく、切りそろえると大きい。
    let jump_at_boundary = |row: &[[f32; 3]]| {
        (first - 2..first + 2)
            .map(|q| (hue(row[q + 1]) - hue(row[q])).abs())
            .fold(0.0f32, f32::max)
    };
    let clip = sky_row(None, SaturationClip::default());
    let (j_rec, j_clip) = (jump_at_boundary(&rec), jump_at_boundary(&clip));
    eprintln!("飽和の境界の色相の段差（度）: 復元 {j_rec:.2}、切りそろえ {j_clip:.2}");
    assert!(j_rec < 2.0, "復元: {j_rec}");
    assert!(j_clip > 10.0, "切りそろえ: {j_clip}");
    // 境界より奥（近傍に手がかりのない白飛びの中）は、明るさだけの復元へ数組かけて移る（1 組で 16 度より大きく
    // 変わらない）。
    for q in first..rec.len() - 1 {
        let d = (hue(rec[q + 1]) - hue(rec[q])).abs();
        assert!(
            d < 16.0,
            "組 {q}: {:?} → {:?}（{d} 度）",
            rec[q],
            rec[q + 1]
        );
    }
}

#[test]
fn recovered_highlights_never_get_darker_toward_the_center() {
    // 明るくなる勾配で、復元した値はどのチャンネルもほとんど下がらない（白飛びの中心が周りより暗くならない・2 つ目の
    // チャンネルが飽和したところで段差を作らない）。色の比の推定から明るさだけの復元へ移るところで、1 % 未満だけ
    // 下がることがある（手がかりの重みが下がる速さと、推定が上がる速さの釣り合い）。
    for method in [HighlightMethod::Neutral, HighlightMethod::Reconstruct] {
        let row = sky_row(Some(method), SaturationClip::default());
        for q in 0..row.len() - 1 {
            for c in 0..3 {
                assert!(
                    row[q + 1][c] >= row[q][c] * 0.99,
                    "{method:?} 組 {q}: {:?} → {:?}",
                    row[q],
                    row[q + 1]
                );
            }
        }
    }
}

#[test]
fn no_magenta_where_green_clips_first() {
    // 本当は G が R より明るい。G が先に飽和しても、出力の G は R より暗くならない（マゼンタにならない）。
    for method in [HighlightMethod::Neutral, HighlightMethod::Reconstruct] {
        for (q, rgb) in sky_row(Some(method), SaturationClip::default())
            .iter()
            .enumerate()
        {
            assert!(rgb[1] >= rgb[0], "{method:?} 組 {q}: {rgb:?}");
            assert!(rgb.iter().all(|v| v.is_finite() && *v >= 0.0));
        }
    }
    // 比べる相手: 切りそろえも復元もしないと、R が G を超えてマゼンタ寄りになる（このテストが検出できること）。
    let off = sky_row(None, SaturationClip::Off);
    assert!(off.iter().any(|rgb| rgb[0] > rgb[1]), "{off:?}");
}

#[test]
fn amount_blends_the_clipped_and_the_recovered_values() {
    let cfa = wb_cfa(full_roi(), CfaPattern::GRBG, sky);
    let full = recover_highlights(
        &cfa,
        &plan(
            HighlightMethod::Reconstruct,
            100.0,
            SaturationClip::default(),
        ),
        out_roi(),
    )
    .unwrap();
    let half = recover_highlights(
        &cfa,
        &plan(
            HighlightMethod::Reconstruct,
            50.0,
            SaturationClip::default(),
        ),
        out_roi(),
    )
    .unwrap();
    let clip = crate::sensor::wb::apply_white_balance(
        &cfa_raw_from_wb(&cfa),
        &WbCoefficients::new(MULT).unwrap(),
        SaturationClip::default(),
        out_roi(),
    )
    .unwrap();
    let mut changed = 0;
    for ((a, b), c) in full.data().iter().zip(half.data()).zip(clip.data()) {
        let expected = c + 0.5 * (a - c);
        assert!(
            (b - expected).abs() <= 1e-6 * expected.abs().max(1.0),
            "{a} {b} {c}"
        );
        if a != c {
            changed += 1;
        }
    }
    assert!(changed > 0);
    // 飽和の印は引き継ぐ。
    assert_eq!(full.saturated(), cfa.window(out_roi()).unwrap().saturated());
}

#[test]
fn tiles_match_the_whole_bitwise() {
    // 斑点状の白飛び（いくつかの円）と色のある背景。
    let scene = |x: i32, y: i32| {
        let mut v = [0.3 + 0.002 * x as f32, 0.5, 0.4 + 0.003 * y as f32];
        for (cx, cy, r, gain) in [(30, 20, 9, 3.0f32), (90, 40, 14, 2.2), (150, 15, 6, 4.0)] {
            let d2 = ((x - cx) * (x - cx) + (y - cy) * (y - cy)) as f32;
            if d2 < (r * r) as f32 {
                v = v.map(|c| c * gain * (1.0 - 0.5 * d2 / (r * r) as f32));
            }
        }
        v
    };
    let (w, h) = (180u32, 60u32);
    let roi = Roi::new(-20, -20, w + 40, h + 40);
    for pattern in CfaPattern::ALL {
        let cfa = wb_cfa(roi, pattern, scene);
        let p = plan(
            HighlightMethod::Reconstruct,
            70.0,
            SaturationClip::default(),
        );
        let whole = recover_highlights(&cfa, &p, Roi::new(0, 0, w, h)).unwrap();
        for t in [
            Roi::new(0, 0, 33, 17),
            Roi::new(21, 9, 40, 31),
            Roi::new(77, 25, 41, 35),
            Roi::new(141, 3, 39, 20),
        ] {
            let need = t.expand(p.margin());
            let part = recover_highlights(&cfa.window(need).unwrap(), &p, t).unwrap();
            assert_eq!(part.roi(), t);
            for ly in 0..t.height {
                for lx in 0..t.width {
                    let a = whole.value((t.x as u32) + lx, (t.y as u32) + ly).unwrap();
                    let b = part.value(lx, ly).unwrap();
                    assert_eq!(a.to_bits(), b.to_bits(), "{pattern:?} {t:?} ({lx}, {ly})");
                }
            }
        }
    }
}

#[test]
fn insufficient_input_is_rejected() {
    let cfa = wb_cfa(Roi::new(0, 0, 40, 40), CfaPattern::RGGB, |_, _| [1.0; 3]);
    let p = plan(HighlightMethod::Neutral, 100.0, SaturationClip::default());
    assert!(recover_highlights(&cfa, &p, Roi::new(2, 2, 10, 10)).is_err());
    assert!(recover_highlights(&cfa, &p, Roi::new(15, 15, 10, 10)).is_ok());
}
