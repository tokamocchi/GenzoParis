use genzo_model::NoiseReduction;
use genzo_raw::CfaPattern;

use super::*;
use crate::contract::ColorContract;
use crate::image::{CfaImage, RgbImage};
use crate::sensor::demosaic::{DemosaicMethod, demosaic};

/// テスト用の決まった乱数（splitmix64）。
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// 標準正規分布（Box–Muller）。
    fn gauss(&mut self) -> f32 {
        let (u, v) = (self.uniform(), self.uniform());
        ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
    }
}

fn tile(w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3]) -> ImageTile {
    let image = RgbImage::from_fn(w, h, ColorContract::B1CameraRgb, f).expect("画像");
    ImageTile::from_image(Roi::full(w, h), image).unwrap()
}

/// 範囲 `roi` の中の画素の値（チャンネル `c`）の平均と標準偏差。
fn stats(t: &ImageTile, roi: Roi, c: usize) -> (f64, f64) {
    let mut v = Vec::new();
    for y in roi.y..roi.y + roi.height as i32 {
        for x in roi.x..roi.x + roi.width as i32 {
            v.push(f64::from(t.at(i64::from(x), i64::from(y)).unwrap()[c]));
        }
    }
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    let var = v.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n;
    (mean, var.sqrt())
}

fn nr(luminance: f32, color: f32) -> NoiseReduction {
    NoiseReduction {
        luminance,
        color,
        ..Default::default()
    }
}

#[test]
fn transform_round_trips() {
    for c in [
        [0.0f32, 0.0, 0.0],
        [0.18, 0.18, 0.18],
        [1.0, 0.5, 0.25],
        [3.5, 2.0, 0.001],
        [0.0004, 0.02, 7.0],
    ] {
        let back = inverse(forward(c));
        for k in 0..3 {
            assert!(
                (back[k] - c[k]).abs() <= 2e-6 * c[k].max(1.0),
                "{c:?} → {back:?}"
            );
        }
    }
    // 負の値（B1 にはないはず）は 0 として扱う。
    assert_eq!(inverse(forward([-0.5, 0.2, 0.2]))[0], 0.0);
}

#[test]
fn garrote_shrinks_small_coefficients_and_keeps_large_ones() {
    assert_eq!(garrote(0.3, 0.0), 0.3);
    assert_eq!(garrote(0.05, 0.1), 0.0);
    assert_eq!(garrote(-0.1, 0.1), 0.0);
    assert!((garrote(1.0, 0.1) - 0.99).abs() < 1e-7);
    assert!((garrote(-1.0, 0.1) + 0.99).abs() < 1e-7);
    // 符号を変えない・大きさは単調。
    let mut last = 0.0;
    for i in 0..100 {
        let w = i as f32 * 0.01;
        let g = garrote(w, 0.2);
        assert!(g >= last && g <= w, "{w}: {g}");
        last = g;
    }
}

#[test]
fn plan_follows_the_settings_and_the_grid() {
    assert!(NoisePlan::new(&NoiseReduction::default(), DemosaicMethod::Rcd).is_none());
    let full = NoisePlan::new(&nr(50.0, 25.0), DemosaicMethod::Rcd).unwrap();
    assert_eq!(full.levels, 5);
    assert_eq!(full.margin(), 62);
    assert!(full.luma_active() && full.chroma_active());
    // 2 × 2 の簡易処理の格子は、S の段 1 から（最も細かい段は計算しない）。雑音の大きさは簡易処理の格子の値。
    let half = NoisePlan::new(&nr(50.0, 25.0), DemosaicMethod::Half2x2).unwrap();
    assert_eq!(half.levels, 4);
    assert_eq!(half.margin(), 30);
    for j in 0..4 {
        let k = j + 1;
        let l = full.luma[k] / NR_LUMA_LEVEL_NOISE[k] * NR_HALF_LUMA_LEVEL_NOISE[j];
        let c = full.chroma[k] / NR_CHROMA_LEVEL_NOISE[k] * NR_HALF_CHROMA_LEVEL_NOISE[j];
        assert!((half.luma[j] - l).abs() <= 1e-6 * l, "段 {j}");
        assert!((half.chroma[j] - c).abs() <= 1e-6 * c, "段 {j}");
    }
    assert_eq!(half.luma[4], 0.0);
    // 比べる用の双線形補間はフル解像度の格子（RCD と同じ値）。
    assert_eq!(
        NoisePlan::new(&nr(50.0, 25.0), DemosaicMethod::Bilinear),
        Some(full)
    );
    // 量に比例し、ディテールで細かい段・コントラストで粗い段のしきい値が下がる。
    let double = NoisePlan::new(&nr(100.0, 50.0), DemosaicMethod::Rcd).unwrap();
    for j in 0..5 {
        assert!((double.luma[j] - 2.0 * full.luma[j]).abs() <= 1e-6 * double.luma[j]);
    }
    let fine = NoisePlan::new(
        &NoiseReduction {
            luminance: 50.0,
            luminance_detail: 100.0,
            luminance_contrast: 100.0,
            ..Default::default()
        },
        DemosaicMethod::Rcd,
    )
    .unwrap();
    assert!(fine.luma[0] < full.luma[0] && fine.luma[4] < full.luma[4]);
    assert!(!fine.chroma_active());
    // 色だけ。
    let color = NoisePlan::new(&nr(0.0, 40.0), DemosaicMethod::Rcd).unwrap();
    assert!(!color.luma_active() && color.chroma_active());
}

/// 一様な灰色に雑音を足した画像（左半分と右半分で明るさが違う縁つき）。
fn noisy(w: u32, h: u32, sigma: f32, chroma_only: bool, seed: u64) -> ImageTile {
    let mut rng = Rng(seed);
    let mut data = Vec::with_capacity((w * h) as usize);
    for _y in 0..h {
        for x in 0..w {
            let base = if x < w / 2 { 0.1f32 } else { 0.4 };
            let px = if chroma_only {
                let n = rng.gauss() * sigma;
                [base + n, base - n * 0.5, base + n * 0.25]
            } else {
                [
                    base + rng.gauss() * sigma,
                    base + rng.gauss() * sigma,
                    base + rng.gauss() * sigma,
                ]
            };
            data.push(px.map(|v| v.max(0.0)));
        }
    }
    let image = RgbImage::from_vec(w, h, ColorContract::B1CameraRgb, data).unwrap();
    ImageTile::from_image(Roi::full(w, h), image).unwrap()
}

/// 鏡映で広げた入力（画像の外の余白）を作って、範囲 `roi` の結果を返す。
fn run(input: &ImageTile, plan: &NoisePlan, roi: Roi) -> ImageTile {
    let (w, h) = (input.roi.width, input.roi.height);
    let need = roi.expand(plan.margin());
    let padded = input.gather_mirror(need, w, h).unwrap();
    reduce_noise(&padded, plan, roi).unwrap()
}

#[test]
fn luminance_noise_is_reduced_and_the_edge_is_kept() {
    let (w, h) = (96u32, 64u32);
    let input = noisy(w, h, 0.01, false, 7);
    let plan = NoisePlan::new(&nr(60.0, 0.0), DemosaicMethod::Rcd).unwrap();
    let out = run(&input, &plan, Roi::full(w, h));
    // 平らなところ（縁から離れた右半分）の輝度（(R + 2G + B) / 4）の雑音が半分以下になる。色の雑音は残すので、
    // チャンネルごとの雑音はそれほど減らない。
    let flat = Roi::new(60, 10, 30, 44);
    let luma = |t: &ImageTile| {
        tile(flat.width, flat.height, |x, y| {
            let p = t
                .at(
                    i64::from(flat.x) + i64::from(x),
                    i64::from(flat.y) + i64::from(y),
                )
                .unwrap();
            [(p[0] + 2.0 * p[1] + p[2]) * 0.25; 3]
        })
    };
    let (l0, l1) = (luma(&input), luma(&out));
    let all = Roi::full(flat.width, flat.height);
    let (m0, s0) = stats(&l0, all, 0);
    let (m1, s1) = stats(&l1, all, 0);
    assert!(s1 < 0.5 * s0, "輝度の雑音: {s0} → {s1}");
    assert!((m1 - m0).abs() < 0.002, "平均: {m0} → {m1}");
    for c in 0..3 {
        let (_, c0) = stats(&input, flat, c);
        let (_, c1) = stats(&out, flat, c);
        assert!(c1 < c0, "チャンネル {c}: {c0} → {c1}");
    }
    // 縁（0.1 → 0.4）の段差は残る（縁の両側 4 画素の平均の差が 9 割以上）。
    let left = stats(&out, Roi::new(40, 10, 4, 44), 1).0;
    let right = stats(&out, Roi::new(52, 10, 4, 44), 1).0;
    assert!(right - left > 0.9 * 0.3, "{left} {right}");
}

#[test]
fn color_noise_is_reduced_without_moving_the_brightness() {
    let (w, h) = (96u32, 64u32);
    let input = noisy(w, h, 0.02, true, 11);
    let plan = NoisePlan::new(&nr(0.0, 60.0), DemosaicMethod::Rcd).unwrap();
    let out = run(&input, &plan, Roi::full(w, h));
    let flat = Roi::new(60, 10, 30, 44);
    // R − G の揺らぎが小さくなる。
    let diff = |t: &ImageTile| {
        let mut v = Vec::new();
        for y in flat.y..flat.y + flat.height as i32 {
            for x in flat.x..flat.x + flat.width as i32 {
                let p = t.at(i64::from(x), i64::from(y)).unwrap();
                v.push(f64::from(p[0] - p[1]));
            }
        }
        let n = v.len() as f64;
        let m = v.iter().sum::<f64>() / n;
        (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n).sqrt()
    };
    let (d0, d1) = (diff(&input), diff(&out));
    assert!(d1 < 0.5 * d0, "{d0} → {d1}");
    // 明るさ（G の平均）は変わらない。
    let (g0, _) = stats(&input, flat, 1);
    let (g1, _) = stats(&out, flat, 1);
    assert!((g1 - g0).abs() < 0.003, "{g0} → {g1}");
}

#[test]
fn tiles_match_the_whole_image_bitwise() {
    let (w, h) = (90u32, 70u32);
    let input = noisy(w, h, 0.015, false, 3);
    let plan = NoisePlan::new(&nr(45.0, 35.0), DemosaicMethod::Rcd).unwrap();
    let whole = run(&input, &plan, Roi::full(w, h));
    for roi in [
        Roi::new(0, 0, 17, 9),
        Roi::new(31, 22, 25, 31),
        Roi::new(70, 50, 20, 20),
        Roi::new(-5, -3, 12, 10),
    ] {
        let part = run(&input, &plan, roi);
        for y in 0..roi.height as i32 {
            for x in 0..roi.width as i32 {
                let (gx, gy) = (roi.x + x, roi.y + y);
                let mx = crate::border::mirror_index(i64::from(gx), w);
                let my = crate::border::mirror_index(i64::from(gy), h);
                // 画像の内側だけを比べる（外側は鏡映の入力から計算した値）。
                if gx < 0 || gy < 0 {
                    continue;
                }
                let a = whole.at(i64::from(mx), i64::from(my)).unwrap();
                let b = part.at(i64::from(gx), i64::from(gy)).unwrap();
                assert_eq!(
                    a.map(f32::to_bits),
                    b.map(f32::to_bits),
                    "({gx}, {gy}) {roi:?}"
                );
            }
        }
    }
}

#[test]
fn small_thresholds_barely_change_the_image() {
    let input = tile(40, 30, |x, y| {
        [
            0.05 + 0.01 * (x % 7) as f32,
            0.2 + 0.003 * y as f32,
            0.4 - 0.002 * (x + y) as f32,
        ]
    });
    let plan = NoisePlan::new(&nr(0.01, 0.01), DemosaicMethod::Rcd).unwrap();
    let out = run(&input, &plan, Roi::full(40, 30));
    for (a, b) in input.image.data().iter().zip(out.image.data()) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 2e-4, "{a:?} {b:?}");
        }
    }
}

#[test]
fn insufficient_input_is_rejected() {
    let input = tile(40, 30, |_, _| [0.2; 3]);
    let plan = NoisePlan::new(&nr(50.0, 0.0), DemosaicMethod::Rcd).unwrap();
    assert!(reduce_noise(&input, &plan, Roi::new(5, 5, 10, 10)).is_err());
}

/// CFA の各画素に独立な雑音を足して RCD でデモザイクした画像の、平方根の領域の Y・D1・D2 の段ごとの細部の標準偏差
/// （画素の雑音 1 あたり。[`NR_LUMA_LEVEL_NOISE`]・[`NR_CHROMA_LEVEL_NOISE`] の出どころ）。
fn demosaiced_noise_per_level(method: DemosaicMethod) -> [[f64; NR_LEVELS]; 3] {
    let (w, h) = (512u32, 512u32);
    let base = 0.2f32;
    let sigma = 0.01f32;
    let mut rng = Rng(5);
    let cfa = CfaImage::from_fn(Roi::full(w, h), CfaPattern::RGGB, |_, _| {
        (base + rng.gauss() * sigma).max(0.0)
    })
    .unwrap();
    let out_roi = match method {
        DemosaicMethod::Half2x2 => Roi::full(w / 2, h / 2),
        _ => {
            let m = method.margin() as i32;
            Roi::new(m, m, w - 2 * m as u32, h - 2 * m as u32)
        }
    };
    let rgb = demosaic(&cfa, method, out_roi).unwrap();
    // 平方根の領域の画素の雑音（1 チャンネル）。
    let s_pixel = f64::from(sigma) / (2.0 * f64::from(base + NR_EPSILON).sqrt());
    let (rw, rh) = (rgb.roi.width as usize, rgb.roi.height as usize);
    let planes: Vec<Vec<f64>> = (0..3)
        .map(|k| {
            rgb.image
                .data()
                .iter()
                .map(|c| f64::from(forward(*c)[k]))
                .collect()
        })
        .collect();
    let mut result = [[0.0; NR_LEVELS]; 3];
    let levels = NR_LEVELS - method.sensor_pixels_per_output().trailing_zeros() as usize;
    for (plane, out) in planes.iter().zip(result.iter_mut()) {
        let mut cur = plane.clone();
        for (k, slot) in out.iter_mut().enumerate().take(levels) {
            let hole = 1usize << k;
            let mut next = vec![0.0; rw * rh];
            let taps = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
            let at = |v: &[f64], x: i64, y: i64| {
                let x = x.clamp(0, rw as i64 - 1) as usize;
                let y = y.clamp(0, rh as i64 - 1) as usize;
                v[y * rw + x]
            };
            let mut tmp = vec![0.0; rw * rh];
            for y in 0..rh {
                for x in 0..rw {
                    tmp[y * rw + x] = (0..5)
                        .map(|t| {
                            taps[t] * at(&cur, x as i64 + (t as i64 - 2) * hole as i64, y as i64)
                        })
                        .sum();
                }
            }
            for y in 0..rh {
                for x in 0..rw {
                    next[y * rw + x] = (0..5)
                        .map(|t| {
                            taps[t] * at(&tmp, x as i64, y as i64 + (t as i64 - 2) * hole as i64)
                        })
                        .sum();
                }
            }
            // 端の影響を除いた内側。
            let mg = 2 * (2 * hole) + 24;
            let mut v = Vec::new();
            for y in mg..rh - mg {
                for x in mg..rw - mg {
                    v.push(cur[y * rw + x] - next[y * rw + x]);
                }
            }
            let n = v.len() as f64;
            let mean = v.iter().sum::<f64>() / n;
            let sd = (v.iter().map(|a| (a - mean) * (a - mean)).sum::<f64>() / n).sqrt();
            *slot = sd / s_pixel;
            cur = next;
        }
    }
    result
}

#[test]
fn demosaiced_noise_matches_the_level_tables() {
    let measured = demosaiced_noise_per_level(DemosaicMethod::Rcd);
    eprintln!("Y・D1・D2 の段ごとの標準偏差（画素の雑音 1 あたり）: {measured:.4?}");
    for (p, table) in [
        (0, NR_LUMA_LEVEL_NOISE),
        (1, NR_CHROMA_LEVEL_NOISE),
        (2, NR_CHROMA_LEVEL_NOISE),
    ] {
        for k in 0..NR_LEVELS {
            let (m, t) = (measured[p][k], f64::from(table[k]));
            assert!(
                (m - t).abs() <= 0.12 * t + 0.004,
                "面 {p} の段 {k}: {m} と {t}"
            );
        }
    }
}

#[test]
fn half_grid_noise_matches_the_level_tables() {
    let measured = demosaiced_noise_per_level(DemosaicMethod::Half2x2);
    eprintln!("簡易処理の Y・D1・D2 の段ごとの標準偏差（画素の雑音 1 あたり）: {measured:.4?}");
    for (p, table) in [
        (0, NR_HALF_LUMA_LEVEL_NOISE),
        (1, NR_HALF_CHROMA_LEVEL_NOISE),
        (2, NR_HALF_CHROMA_LEVEL_NOISE),
    ] {
        for j in 0..NR_LEVELS - 1 {
            let (m, t) = (measured[p][j], f64::from(table[j]));
            assert!(
                (m - t).abs() <= 0.12 * t + 0.004,
                "面 {p} の段 {j}: {m} と {t}"
            );
        }
    }
}

#[test]
#[ignore = "計測（手動で --release で実行する）"]
fn bench_reduce_noise_band() {
    let (w, h) = (7008u32 + 124, 452u32 + 124);
    let input = noisy(w, h, 0.01, false, 1);
    for (name, s) in [
        ("輝度と色", nr(30.0, 25.0)),
        ("色だけ", nr(0.0, 25.0)),
        ("輝度だけ", nr(30.0, 0.0)),
    ] {
        let plan = NoisePlan::new(&s, DemosaicMethod::Rcd).unwrap();
        let out = Roi::new(62, 62, w - 124, h - 124);
        let t = std::time::Instant::now();
        for _ in 0..3 {
            std::hint::black_box(reduce_noise(&input, &plan, out).unwrap());
        }
        println!(
            "{name}: {:.1} ms / 帯（7008 × 452）",
            t.elapsed().as_secs_f64() * 1000.0 / 3.0
        );
    }
}
