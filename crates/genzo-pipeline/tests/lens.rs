//! レンズ補正（DEV-14。処理バージョン 2）の結合テスト: ステージ 7（周辺減光・倍率色収差）とステージ 9 の
//! 歪曲補正を、エンジンのプレビュー・フル解像度のタイル・ガイド・WB のスポイトで確かめる。

mod common;

use std::sync::Arc;

use common::*;
use genzo_color::RgbColorSpace;
use genzo_model::{DevelopSettings, FileId, Orientation, RenderQuality};
use genzo_pipeline::finish::{UnimplementedSetting, unimplemented_settings};
use genzo_pipeline::{
    ColorContract, Engine, EngineConfig, NeverCancel, PhotoSource, RgbImage, SensorWarning,
    SourceId, SourceImage,
};
use genzo_raw::{
    DngLensCorrection, DngVignetteRadial, DngWarpPlane, DngWarpRectilinear, LensCorrectionData,
    PixelRect, RawImage, SonyLensCorrection,
};
use genzo_testkit::diff::{LabConversion, delta_e2000_stats};
use genzo_testkit::image::FloatImage;

/// Sony の ARW の補正の情報（歪曲は外ほど負＝樽型の補正、周辺減光は外ほど大きい、色収差は少し）。
fn sony_data() -> LensCorrectionData {
    LensCorrectionData::SonyArw(SonyLensCorrection {
        distortion: (0..16).map(|i| (-40 * i) as i16).collect(),
        vignetting: (0..16).map(|i| (250 * i) as i16).collect(),
        ca_red: (0..16).map(|i| (300 * i) as i16).collect(),
        ca_blue: (0..16).map(|i| (-200 * i) as i16).collect(),
    })
}

fn dng_data() -> LensCorrectionData {
    let plane = |k0: f64| DngWarpPlane {
        kr: [k0, 0.03, -0.01, 0.0],
        kt: [0.0, 0.0],
    };
    LensCorrectionData::DngOpcode(DngLensCorrection {
        warp: Some(DngWarpRectilinear {
            planes: vec![plane(1.0005), plane(1.0), plane(0.9995)],
            center: [0.52, 0.48],
        }),
        vignette: Some(DngVignetteRadial {
            k: [0.4, 0.1, 0.0, 0.0, 0.0],
            center: [0.52, 0.48],
        }),
    })
}

/// 補正の情報を付けた合成の RAW（既定の切り抜き付き）。
fn raw_with_lens(scene: &FloatImage, lens: Option<LensCorrectionData>) -> RawImage {
    let mut raw = raw_from(scene);
    raw.default_crop = Some(PixelRect::new(4, 2, raw.width - 8, raw.height - 6));
    raw.lens_correction = lens;
    raw.validate().unwrap();
    raw
}

fn all_corrections() -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.lens.profile_enabled = true;
    s.lens.remove_chromatic_aberration = true;
    s.lens.distortion = 20.0;
    s.lens.vignetting_amount = 30.0;
    s.lens.ca_red_cyan = 40.0;
    s
}

#[test]
fn full_resolution_tiles_match_for_every_tile_size_with_lens_corrections() {
    for data in [sony_data(), dng_data()] {
        let source = raw_source(raw_with_lens(&busy_scene(150, 100), Some(data)));
        let mut s = all_corrections();
        s.geometry.angle_deg = 3.0;
        s.tone.shadows = 40.0;
        let reference = engine(80, 4096)
            .render_full_resolution(&source, &s, None, &NeverCancel)
            .unwrap();
        assert_eq!(reference.non_finite_replaced, 0);
        for tile in [32, 47, 64] {
            let r = engine(80, tile)
                .render_full_resolution(&source, &s, None, &NeverCancel)
                .unwrap();
            assert_bitwise_eq(&r.b3.image, &reference.b3.image, &format!("tile {tile}"));
        }
    }
}

#[test]
fn lens_corrections_change_the_result_and_are_ignored_in_version_1() {
    let source = raw_source(raw_with_lens(&busy_scene(120, 80), Some(sony_data())));
    let e = engine(60, 64);
    let none = DevelopSettings::default();
    let base = e
        .render_preview(&source, &none, RenderQuality::Final, 60)
        .unwrap();
    let s = all_corrections();
    let lens = e
        .render_preview(&source, &s, RenderQuality::Final, 60)
        .unwrap();
    assert!(lens.warnings.is_empty(), "{:?}", lens.warnings);
    assert!(lens.unimplemented.is_empty());
    assert!(lens.active_stages.contains(&"finish.geometry"));
    assert_ne!(base.b3.image.data(), lens.b3.image.data());
    // 版 1 の写真では無視して（結果は補正なしと同じ）、未実装として知らせる。
    let v1 = DevelopSettings {
        process_version: 1,
        ..s.clone()
    };
    let r1 = e
        .render_preview(&source, &v1, RenderQuality::Final, 60)
        .unwrap();
    let base1 = e
        .render_preview(
            &source,
            &DevelopSettings {
                process_version: 1,
                ..Default::default()
            },
            RenderQuality::Final,
            60,
        )
        .unwrap();
    assert_bitwise_eq(&r1.b3.image, &base1.b3.image, "版 1");
    assert_eq!(
        r1.unimplemented,
        [
            UnimplementedSetting::LensProfile,
            UnimplementedSetting::ChromaticAberration,
            UnimplementedSetting::LensManual,
        ]
    );
    assert_eq!(unimplemented_settings(&s), Vec::new());
}

#[test]
fn manual_vignetting_brightens_the_corners_of_a_flat_scene() {
    let scene = FloatImage::filled(96, 64, &[0.18; 3]).unwrap();
    let source = raw_source(raw_with_lens(&scene, None));
    let e = engine(96, 64);
    let mut s = DevelopSettings::default();
    s.lens.vignetting_amount = 100.0;
    s.lens.vignetting_midpoint = 0.0;
    let r = e
        .render_full_resolution(&source, &s, None, &NeverCancel)
        .unwrap();
    let (w, h) = r.b3.image.dimensions();
    let center = r.b3.image.pixel(w / 2, h / 2).unwrap()[1];
    let corner = r.b3.image.pixel(1, 1).unwrap()[1];
    assert!(corner > center * 1.4, "{corner} {center}");
}

#[test]
fn a1_is_reused_when_only_the_distortion_changes() {
    let source = raw_source(raw_with_lens(&busy_scene(120, 80), Some(sony_data())));
    let e = engine(60, 64);
    let mut s = DevelopSettings::default();
    s.lens.profile_enabled = true;
    e.render_preview(&source, &s, RenderQuality::Final, 60)
        .unwrap();
    let before = e.stats().a1;
    // 歪曲（ステージ 9）だけを変えても、段階 A1 は作り直さない。
    s.lens.distortion_scale = 60.0;
    s.lens.distortion = -30.0;
    e.render_preview(&source, &s, RenderQuality::Final, 60)
        .unwrap();
    let after = e.stats().a1;
    assert_eq!(after.misses, before.misses);
    assert_eq!(after.hits, before.hits + 1);
    // 周辺減光（ステージ 7）を変えると作り直す。
    s.lens.vignetting_scale = 50.0;
    e.render_preview(&source, &s, RenderQuality::Final, 60)
        .unwrap();
    assert_eq!(e.stats().a1.misses, before.misses + 1);
}

#[test]
fn preview_and_full_resolution_agree_with_distortion() {
    // IQ-07b と同じ基準（プレビューの各画素が覆うフル解像度の範囲の平均）で、歪曲補正のある場合も
    // プレビューとフル解像度の位置がそろう（滑らかなシーンで差が小さい）。
    let source = raw_source(raw_with_lens(&wavy_scene(300, 201), Some(sony_data())));
    let e = engine(100, 64);
    let lab = LabConversion::linear(RgbColorSpace::Bt2020);
    let stats = |s: &DevelopSettings| {
        let preview = e
            .render_preview(&source, s, RenderQuality::Final, 100)
            .unwrap();
        let reference = iq07b_reference(&e, &source, s, &preview);
        delta_e2000_stats(&reference, &to_float(&preview.b3.image), lab).unwrap()
    };
    let plain = stats(&DevelopSettings::default());
    let mut s = DevelopSettings::default();
    s.lens.profile_enabled = true;
    s.lens.distortion = 40.0;
    let warped = stats(&s);
    eprintln!(
        "IQ-07b の基準との ΔE2000: 補正なし 平均 {:.3} 最大 {:.3}、歪曲補正 平均 {:.3} 最大 {:.3}",
        plain.mean, plain.max, warped.mean, warped.max
    );
    // 歪曲補正で位置がずれていれば、傾きの大きいこのシーンでは差が大きく増える。
    assert!(warped.mean < plain.mean * 2.0 + 0.3, "{warped:?} {plain:?}");
}

#[test]
fn guide_and_white_balance_picker_follow_the_distortion() {
    let source = raw_source(raw_with_lens(&busy_scene(120, 80), Some(dng_data())));
    let e = engine(60, 64);
    let mut s = all_corrections();
    s.tone.highlights = -50.0;
    s.tone.shadows = 50.0;
    let preview = e
        .render_preview(&source, &s, RenderQuality::Final, 60)
        .unwrap();
    assert!(preview.used_guide);
    let job = e.full_resolution(&source, &s).unwrap();
    assert!(job.guide().is_some());
    assert!(job.context().frames.distortion.is_some());
    // 中央のスポイトは、歪曲補正があっても画像の中の点を指す。
    let pick = e.white_balance_at(&source, &s, (0.5, 0.5), false).unwrap();
    assert!(pick.samples > 0);
}

#[test]
fn missing_embedded_data_is_reported_and_manual_corrections_still_apply() {
    let source = raw_source(raw_with_lens(&busy_scene(120, 80), None));
    let e = engine(60, 64);
    let mut s = DevelopSettings::default();
    s.lens.profile_enabled = true;
    let r = e
        .render_preview(&source, &s, RenderQuality::Final, 60)
        .unwrap();
    assert_eq!(r.warnings, [SensorWarning::LensProfileMissing]);
    // 補正の情報がなく、手動の補正もなければ、結果は補正なしと同じ。
    let none = e
        .render_preview(
            &source,
            &DevelopSettings::default(),
            RenderQuality::Final,
            60,
        )
        .unwrap();
    assert_bitwise_eq(&r.b3.image, &none.b3.image, "補正の情報なし");
    s.lens.distortion = 50.0;
    let m = e
        .render_preview(&source, &s, RenderQuality::Final, 60)
        .unwrap();
    assert_ne!(m.b3.image.data(), none.b3.image.data());
}

#[test]
fn working_images_use_the_distortion_but_not_stage_7() {
    let image = RgbImage::from_fn(90, 60, ColorContract::B2Working, |x, y| {
        [
            0.05 + 0.004 * x as f32,
            0.1 + 0.005 * y as f32,
            0.2 + 0.001 * (x + y) as f32,
        ]
    })
    .unwrap();
    let source = PhotoSource::new(
        SourceId::new(FileId::new(2), 1),
        SourceImage::Working {
            image: Arc::new(image),
            orientation: Orientation::Normal,
        },
    )
    .unwrap();
    let e = Engine::new(EngineConfig {
        preview_long_edge: 90,
        tile_size: 64,
        ..Default::default()
    })
    .unwrap();
    let none = e
        .render_preview(
            &source,
            &DevelopSettings::default(),
            RenderQuality::Final,
            90,
        )
        .unwrap();
    let mut s = DevelopSettings::default();
    s.lens.distortion = -40.0;
    let d = e
        .render_preview(&source, &s, RenderQuality::Final, 90)
        .unwrap();
    assert!(d.warnings.is_empty());
    assert_ne!(d.b3.image.data(), none.b3.image.data());
    s.lens.vignetting_amount = 50.0;
    let v = e
        .render_preview(&source, &s, RenderQuality::Final, 90)
        .unwrap();
    assert_eq!(
        v.warnings,
        [SensorWarning::LensSensorCorrectionsNotApplicable]
    );
    // 周辺減光は使わないので、歪曲だけのときと同じ。
    assert_bitwise_eq(&v.b3.image, &d.b3.image, "RAW 以外");
    let job = e.full_resolution(&source, &s).unwrap();
    assert_eq!(
        job.warnings(),
        [SensorWarning::LensSensorCorrectionsNotApplicable]
    );
}
