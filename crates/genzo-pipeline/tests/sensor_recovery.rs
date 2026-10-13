//! ハイライト復元（ステージ 4。DEV-06）とノイズ軽減（ステージ 6。DEV-13）の結合テスト（処理バージョン 2）:
//! センサー処理の出力（B2）・エンジンのフル解像度のタイル・プレビューとの一致（IQ-07b）・処理バージョン 1 での無視を
//! 確かめる。

mod common;

use std::sync::Arc;

use common::*;
use genzo_color::space::XYZ_TO_BT2020;
use genzo_model::{
    DevelopSettings, FileId, HighlightMethod, HighlightRecovery, NoiseReduction, Orientation,
    RenderQuality,
};
use genzo_pipeline::finish::{UnimplementedSetting, unimplemented_settings};
use genzo_pipeline::sensor::denoise::NR_EPSILON;
use genzo_pipeline::sensor::{BAND_MARGIN_RATIO, PREVIEW_BAND_ROWS};
use genzo_pipeline::{
    ColorContract, NeverCancel, PhotoSource, RgbImage, Roi, SensorOptions, SensorPlan,
    SensorWarning, SourceId, SourceImage,
};
use genzo_raw::{CfaPattern, RawImage};
use genzo_testkit::bayer::{BayerParams, mosaic};
use genzo_testkit::diff::{LabConversion, delta_e2000_stats};
use genzo_testkit::image::FloatImage;

/// カメラ RGB を BT.2020 とみなす RAW（ステージ 8 が恒等になり、B2 がカメラ RGB と同じ値になる）。
fn bt2020_raw(scene: &FloatImage, cfa: CfaPattern) -> RawImage {
    let m = XYZ_TO_BT2020.rows().map(|r| r.map(|v| v as f32));
    mosaic(
        scene,
        &BayerParams {
            cfa,
            cam_xyz: Some(m),
            ..Default::default()
        },
    )
    .unwrap()
}

fn v2() -> DevelopSettings {
    DevelopSettings {
        process_version: 2,
        ..Default::default()
    }
}

fn with_highlight(method: HighlightMethod) -> DevelopSettings {
    DevelopSettings {
        highlight_recovery: HighlightRecovery {
            method,
            amount: 100.0,
        },
        ..v2()
    }
}

fn with_noise(luminance: f32, color: f32) -> DevelopSettings {
    let mut s = v2();
    s.detail.noise_reduction = NoiseReduction {
        luminance,
        color,
        ..Default::default()
    };
    s
}

/// 色の付いた明るさの勾配（本当の値は (0.5, 0.8, 1.0) · X、X は左端 0.6 〜 右端 3.0。WB の係数 (2, 1, 1.5) で
/// G が X = 1.25、B が 1.5 で飽和する）。
fn sky_scene(w: u32, h: u32) -> FloatImage {
    FloatImage::from_rgb_fn(w, h, |x, _| {
        let v = 0.6 + 2.4 * x as f32 / w as f32;
        [0.5 * v, 0.8 * v, v]
    })
    .unwrap()
}

/// センサー処理（ステージ 2〜8）の全体の出力（B2。最終品質）。
fn sensor_output(raw: &RawImage, s: &DevelopSettings) -> RgbImage {
    let plan = SensorPlan::new(raw, s, &SensorOptions::default()).unwrap();
    let (tile, _) = plan
        .render_roi(raw, Roi::full(raw.width, raw.height))
        .unwrap();
    tile.image
}

/// 色相の角度（度。反対色の簡単な軸）。
fn hue(c: [f32; 3]) -> f32 {
    let a = c[0] - c[1];
    let b = c[2] - 0.5 * (c[0] + c[1]);
    b.atan2(a).to_degrees()
}

/// 行 `y` の 2 画素ごとの平均（CFA の組の幅で、デモザイクの細かい揺れをならす）。
fn row_pairs(img: &RgbImage, y: u32) -> Vec<[f32; 3]> {
    (0..img.width() / 2)
        .map(|i| {
            let a = img.pixel(2 * i, y).unwrap();
            let b = img.pixel(2 * i + 1, y).unwrap();
            [0, 1, 2].map(|k| 0.5 * (a[k] + b[k]))
        })
        .collect()
}

#[test]
fn highlight_recovery_keeps_the_hue_without_magenta_through_demosaicing() {
    let (w, h) = (200u32, 24u32);
    for cfa in CfaPattern::ALL {
        let raw = bt2020_raw(&sky_scene(w, h), cfa);
        let clip = sensor_output(&raw, &v2());
        let rec = sensor_output(&raw, &with_highlight(HighlightMethod::Reconstruct));
        let neutral = sensor_output(&raw, &with_highlight(HighlightMethod::Neutral));
        let y = h / 2;
        let (rc, rr, rn) = (
            row_pairs(&clip, y),
            row_pairs(&rec, y),
            row_pairs(&neutral, y),
        );
        // G が飽和し始めるところ（X = 1.25）。
        let first = (0..rr.len())
            .find(|&i| 0.6 + 2.4 * (2 * i + 1) as f32 / w as f32 >= 1.25)
            .unwrap();
        // マゼンタにならない: 本当は G > R なので、出力の G は R より暗くならない。
        for (i, (a, b)) in rr.iter().zip(&rn).enumerate() {
            assert!(a[1] >= a[0] * 0.999, "{cfa:?} 復元 {i}: {a:?}");
            assert!(b[1] >= b[0] * 0.999, "{cfa:?} 明るさだけ {i}: {b:?}");
        }
        // 色の段差: 飽和の境界の前後で、色相の変化が切りそろえより小さい。
        let jump = |row: &[[f32; 3]], range: std::ops::Range<usize>| {
            range
                .map(|i| (hue(row[i + 1]) - hue(row[i])).abs())
                .fold(0.0f32, f32::max)
        };
        let (jr, jc) = (
            jump(&rr, first - 3..first + 2),
            jump(&rc, first - 3..first + 2),
        );
        eprintln!("{cfa:?}: 飽和の境界の色相の段差（度）: 復元 {jr:.2}、切りそろえ {jc:.2}");
        assert!(
            jr < 3.0 && jc > 10.0,
            "{cfa:?}: 復元 {jr} 度、切りそろえ {jc} 度"
        );
        // 境界より奥（近傍に手がかりのない白飛びの中）は、明るさだけの復元へ数画素かけて移る。
        let inner = jump(&rr, first..rr.len() - 1);
        assert!(inner < 16.0, "{cfa:?}: 奥の色相の段差 {inner} 度");
        // 境界のすぐ内側は、本当の比（G / R = 1.6）に近い（デモザイクの補間で R が少し揺れる分を許す）。切りそろえは
        // G を 1 で止めるので、比は 1.6 より小さくなる。
        for p in &rr[first + 1..first + 3] {
            assert!((p[1] / p[0] - 1.6).abs() < 0.12, "{cfa:?}: {p:?}");
        }
        let (p, q) = (rr[first + 1], rc[first + 1]);
        assert!(
            (p[1] / p[0] - 1.6).abs() < (q[1] / q[0] - 1.6).abs(),
            "{cfa:?}: {p:?} {q:?}"
        );
        // 飽和していない部分は変えない（左端の方）。
        for i in 0..first.saturating_sub(8) {
            for k in 0..3 {
                assert_eq!(rr[i][k].to_bits(), rc[i][k].to_bits(), "{cfa:?} {i}");
            }
        }
    }
}

#[test]
fn version_2_defaults_and_version_1_ignore_the_new_settings() {
    let raw = bt2020_raw(&sky_scene(64, 40), CfaPattern::RGGB);
    let v1 = DevelopSettings {
        process_version: 1,
        ..Default::default()
    };
    let base = sensor_output(&raw, &v1);
    // 版 2 の既定（復元しない・量 0）は版 1 と同じ。
    assert_bitwise_eq(&base, &sensor_output(&raw, &v2()), "版 2 の既定");
    // 版 1 は、ハイライト復元・ノイズ軽減の値があっても無視して、知らせる。
    let mut s = with_highlight(HighlightMethod::Reconstruct);
    s.process_version = 1;
    s.detail.noise_reduction.luminance = 50.0;
    s.detail.noise_reduction.color = 40.0;
    assert_bitwise_eq(&base, &sensor_output(&raw, &s), "版 1");
    assert_eq!(
        unimplemented_settings(&s),
        [
            UnimplementedSetting::HighlightRecovery,
            UnimplementedSetting::NoiseReduction
        ]
    );
    // 版 2 では計画に入る（ステージ 4・6）。
    s.process_version = 2;
    let plan = SensorPlan::new(&raw, &s, &SensorOptions::default()).unwrap();
    let ids: Vec<&str> = plan.stages().iter().map(|st| st.id()).collect();
    assert_eq!(
        ids,
        [
            "sensor.normalize",
            "sensor.white_balance",
            "sensor.highlight_recovery",
            "sensor.demosaic.rcd",
            "sensor.noise_reduction",
            "sensor.color_matrix",
        ]
    );
    // ハイライトを復元するときは、ステージ 3 では切りそろえない。
    assert_eq!(plan.saturation_clip(), genzo_pipeline::SaturationClip::Off);
    assert!(unimplemented_settings(&s).is_empty());
}

/// 灰色の階段（暗い・中・明るい）に、CFA の画素ごとに独立な雑音を足した RAW（明るさに比例しない一定の雑音）。
fn noisy_raw(w: u32, h: u32, sigma: f32, seed: u64) -> RawImage {
    let mut state = seed;
    let mut gauss = move || {
        // splitmix64 と Box–Muller（テスト用）。
        let mut next = || {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            (((z ^ (z >> 31)) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (u, v) = (next(), next());
        ((-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()) as f32
    };
    let noise: Vec<f32> = (0..w * h * 3).map(|_| sigma * gauss()).collect();
    let noisy = FloatImage::from_rgb_fn(w, h, |x, y| {
        let v = [0.05f32, 0.18, 0.5][(x * 3 / w) as usize];
        let i = ((y * w + x) * 3) as usize;
        [
            (v + noise[i]).max(0.0),
            (v + noise[i + 1]).max(0.0),
            (v + noise[i + 2]).max(0.0),
        ]
    })
    .unwrap();
    bt2020_raw(&noisy, CfaPattern::RGGB)
}

/// 範囲の中の、輝度 `(R + 2G + B) / 4` と色 `R − G` の標準偏差。
fn noise_of(img: &RgbImage, roi: Roi) -> (f64, f64) {
    let mut l = Vec::new();
    let mut c = Vec::new();
    for y in roi.y..roi.y + roi.height as i32 {
        for x in roi.x..roi.x + roi.width as i32 {
            let p = img.pixel(x as u32, y as u32).unwrap();
            l.push(f64::from((p[0] + 2.0 * p[1] + p[2]) * 0.25));
            c.push(f64::from(p[0] - p[1]));
        }
    }
    let sd = |v: &[f64]| {
        let n = v.len() as f64;
        let m = v.iter().sum::<f64>() / n;
        (v.iter().map(|a| (a - m) * (a - m)).sum::<f64>() / n).sqrt()
    };
    (sd(&l), sd(&c))
}

#[test]
fn noise_reduction_reduces_luminance_and_color_noise() {
    let raw = noisy_raw(192, 96, 0.01, 1);
    let base = sensor_output(&raw, &v2());
    let lum = sensor_output(&raw, &with_noise(60.0, 0.0));
    let col = sensor_output(&raw, &with_noise(0.0, 60.0));
    // 中くらいの明るさ（0.18）の内側。
    let roi = Roi::new(80, 20, 32, 56);
    let (l0, c0) = noise_of(&base, roi);
    let (l1, _) = noise_of(&lum, roi);
    let (_, c2) = noise_of(&col, roi);
    eprintln!("輝度の雑音 {l0:.5} → {l1:.5}、色の雑音 {c0:.5} → {c2:.5}");
    assert!(l1 < 0.6 * l0, "輝度: {l0} → {l1}");
    assert!(c2 < 0.6 * c0, "色: {c0} → {c2}");
    // 平均の明るさは変えない（0.5 % 以内。平方根の領域で縮小するので、わずかに下がりうる）。
    let mean = |img: &RgbImage| {
        let mut s = 0.0f64;
        for y in roi.y..roi.y + roi.height as i32 {
            for x in roi.x..roi.x + roi.width as i32 {
                s += f64::from(img.pixel(x as u32, y as u32).unwrap()[1]);
            }
        }
        s / f64::from(roi.width * roi.height)
    };
    let (m0, m1) = (mean(&base), mean(&lum));
    assert!(
        (m1 - m0).abs() < 0.005 * m0 + f64::from(NR_EPSILON),
        "{m0} → {m1}"
    );
}

#[test]
fn full_resolution_tiles_match_for_every_tile_size_with_recovery_and_noise_reduction() {
    // 白飛びのある雑音の画像に、ハイライト復元とノイズ軽減をかける。
    let raw = {
        let mut r = noisy_raw(150, 100, 0.01, 4);
        let sky = bt2020_raw(&sky_scene(150, 100), CfaPattern::RGGB);
        // 右半分を白飛びのある勾配にする。
        for y in 0..r.height as usize {
            for x in 75..r.width as usize {
                r.data[y * r.width as usize + x] = sky.data[y * sky.width as usize + x];
            }
        }
        r
    };
    let source = raw_source(raw);
    let mut s = with_highlight(HighlightMethod::Reconstruct);
    s.detail.noise_reduction = NoiseReduction {
        luminance: 40.0,
        color: 50.0,
        luminance_contrast: 30.0,
        ..Default::default()
    };
    s.tone.shadows = 30.0;
    let reference = engine(80, 4096)
        .render_full_resolution(&source, &s, None, &NeverCancel)
        .unwrap();
    assert_eq!(reference.non_finite_replaced, 0);
    for tile in [32, 47, 64] {
        let tiled = engine(80, tile)
            .render_full_resolution(&source, &s, None, &NeverCancel)
            .unwrap();
        assert_bitwise_eq(
            &reference.b3.image,
            &tiled.b3.image,
            &format!("タイル {tile}"),
        );
    }
}

#[test]
fn preview_matches_the_full_resolution_with_noise_reduction_iq07b() {
    // プレビュー（長辺 80）は、ノイズ軽減をフル解像度の格子でかけてから縮小するので、書き出しとの差（IQ-07b）を
    // ノイズ軽減が大きくしない。
    let source = raw_source(noisy_raw(192, 128, 0.02, 9));
    let e = engine(80, 64);
    let stats = |s: &DevelopSettings| {
        let preview = e
            .render_preview(&source, s, RenderQuality::Final, 80)
            .unwrap();
        let reference = iq07b_reference(&e, &source, s, &preview);
        delta_e2000_stats(
            &to_float(&preview.b3.image),
            &reference,
            LabConversion::linear(genzo_color::RgbColorSpace::Bt2020),
        )
        .unwrap()
    };
    let without = stats(&v2());
    let mut s = with_noise(50.0, 50.0);
    s.highlight_recovery.method = HighlightMethod::Reconstruct;
    let with = stats(&s);
    eprintln!("IQ-07b（ΔE2000）: ノイズ軽減なし {without:?}、あり {with:?}");
    assert!(with.p95 <= without.p95 + 0.05, "{without:?} {with:?}");
    assert!(with.max <= without.max + 0.2, "{without:?} {with:?}");
    assert!(with.p95 <= 2.0 && with.max <= 5.0, "{with:?}");
}

#[test]
fn draft_noise_reduction_uses_the_sensor_scale() {
    // ドラッグ中の 2 × 2 の簡易処理でも、同じ量でノイズが減る（S の尺度でしきい値を決める。IQ-07b の対象外）。
    let raw = Arc::new(noisy_raw(256, 128, 0.02, 2));
    let source = PhotoSource::new(SourceId::new(FileId::new(1), 1), SourceImage::Raw(raw)).unwrap();
    let e = engine(128, 64);
    let a1 = |s: &DevelopSettings, q: RenderQuality| e.a1(&source, s, q, 128).unwrap();
    let roi = Roi::new(52, 10, 24, 44);
    for q in [RenderQuality::Draft, RenderQuality::Final] {
        let (l0, c0) = noise_of(&a1(&v2(), q).image, roi);
        let (l1, c1) = noise_of(&a1(&with_noise(70.0, 70.0), q).image, roi);
        eprintln!("{q:?}: 輝度 {l0:.5} → {l1:.5}、色 {c0:.5} → {c1:.5}");
        assert!(l1 < 0.6 * l0 && c1 < 0.6 * c0, "{q:?}");
    }
}

#[test]
fn preview_bands_grow_with_the_noise_reduction_margin() {
    let raw = noisy_raw(64, 64, 0.01, 3);
    let plain = SensorPlan::new(&raw, &v2(), &SensorOptions::default()).unwrap();
    let nr = SensorPlan::new(&raw, &with_noise(30.0, 0.0), &SensorOptions::default()).unwrap();
    // α7 IV の寸法の比（4672 行 → 1707 行）。
    assert_eq!(plain.preview_band_rows(4672, 1707), PREVIEW_BAND_ROWS);
    let rows = nr.preview_band_rows(4672, 1707);
    assert!(rows > PREVIEW_BAND_ROWS, "{rows}");
    // 帯の入力の行（約 2.74 倍）が、余白（62 行 ＋ デモザイクなど）の BAND_MARGIN_RATIO 倍以上。
    assert!(f64::from(rows) * 4672.0 / 1707.0 >= f64::from(62 * BAND_MARGIN_RATIO));
}

#[test]
fn non_raw_input_reports_that_the_sensor_stages_are_not_used() {
    let image = RgbImage::filled(40, 30, ColorContract::B2Working, [0.2; 3]).unwrap();
    let source = PhotoSource::new(
        SourceId::new(FileId::new(1), 1),
        SourceImage::Working {
            image: Arc::new(image),
            orientation: Orientation::Normal,
        },
    )
    .unwrap();
    let mut s = with_noise(30.0, 30.0);
    s.highlight_recovery.method = HighlightMethod::Neutral;
    let preview = engine(40, 64)
        .render_preview(&source, &s, RenderQuality::Final, 40)
        .unwrap();
    assert!(
        preview
            .warnings
            .contains(&SensorWarning::HighlightRecoveryNotApplicable)
    );
    assert!(
        preview
            .warnings
            .contains(&SensorWarning::NoiseReductionNotApplicable)
    );
    assert!(
        SensorWarning::NoiseReductionNotApplicable
            .to_string()
            .contains("RAW 以外")
    );
    // 既定の設定では知らせない。
    let quiet = engine(40, 64)
        .render_preview(&source, &v2(), RenderQuality::Final, 40)
        .unwrap();
    assert!(quiet.warnings.is_empty(), "{:?}", quiet.warnings);
}
