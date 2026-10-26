//! 自動補正（DEV-32。[`genzo_pipeline::Engine::auto_tone`]）のエンジンの結合テスト: 今の値によらない、
//! 暗い写真は露光量を上げる、切り抜いた範囲で決める、適用すると中間調が表示の中間グレーに寄る、
//! RAW 以外の入力。

mod common;

use std::sync::Arc;

use common::{busy_scene, engine, raw_from, raw_source};
use genzo_model::{CropRect, DevelopSettings, FileId, Orientation, RenderQuality};
use genzo_pipeline::engine::{OutputTarget, PreviewRequest};
use genzo_pipeline::{PhotoSource, SourceId, SourceImage, working_image_from_encoded};
use genzo_testkit::image::FloatImage;

/// 中間調の広い、ふつうの明るさのシーン（カメラ RGB。中立）: 左から右へ暗→明の段階（中央値は 0.06 前後）、
/// 上の帯に黒と白の点。
fn ramp_scene(w: u32, h: u32, scale: f32) -> FloatImage {
    const STEPS: [f32; 10] = [0.006, 0.012, 0.025, 0.04, 0.06, 0.09, 0.14, 0.22, 0.36, 0.6];
    FloatImage::from_rgb_fn(w, h, |x, y| {
        let v = if y < h / 16 {
            if x < w / 2 { 0.002 } else { 0.9 }
        } else {
            STEPS[(x * 10 / w).min(9) as usize]
        };
        [(v * scale).min(0.98); 3]
    })
    .unwrap()
}

/// 表示（sRGB）のルマのヒストグラムの中央値の階級。
fn display_median(e: &genzo_pipeline::Engine, source: &PhotoSource, s: &DevelopSettings) -> usize {
    let r = e
        .render_preview_with(
            source,
            s,
            &PreviewRequest::new(RenderQuality::Final, 160)
                .with_output(OutputTarget::assumed_srgb_display().unwrap())
                .with_histogram(),
        )
        .unwrap();
    let h = r.histogram.unwrap();
    let half = h.pixel_count.div_ceil(2);
    let mut acc = 0;
    for (i, c) in h.luma.iter().enumerate() {
        acc += c;
        if acc >= half {
            return i;
        }
    }
    255
}

#[test]
fn auto_tone_does_not_depend_on_the_current_tone_values_and_lifts_dark_photos() {
    let e = engine(160, 64);
    let normal = raw_source(raw_from(&ramp_scene(240, 160, 1.0)));
    let dark = PhotoSource::new(
        SourceId::new(FileId::new(2), 1),
        SourceImage::Raw(Arc::new(raw_from(&ramp_scene(240, 160, 0.25)))),
    )
    .unwrap();
    let s = DevelopSettings::default();
    let a = e.auto_tone(&normal, &s, 160).unwrap();
    assert!(a.stats.samples > 0);
    assert!(
        a.stats.black_log2 < a.stats.median_log2 && a.stats.median_log2 < a.stats.white_log2,
        "{a:?}"
    );
    // 今の露光量・コントラスト・トーン（ローカル補正のない設定）によらず同じ値。
    let mut busy = s.clone();
    busy.exposure_ev = 2.0;
    busy.contrast = -60.0;
    busy.tone.highlights = -100.0;
    busy.tone.blacks = 70.0;
    assert_eq!(e.auto_tone(&normal, &busy, 160).unwrap(), a);
    // 2 段暗い写真は、露光量がおよそ 2 段多い（代表の明るさは 2 段暗い）。
    let d = e.auto_tone(&dark, &s, 160).unwrap();
    eprintln!("ふつう {a:?}\n暗い {d:?}");
    assert!(
        (d.stats.median_log2 - (a.stats.median_log2 - 2.0)).abs() < 0.2,
        "{a:?} / {d:?}"
    );
    assert!(
        (d.values.exposure_ev - (a.values.exposure_ev + 2.0)).abs() < 0.25,
        "{a:?} / {d:?}"
    );
    // 適用すると、どちらも表示のルマの中央値が中間グレー（sRGB の 8bit で 118）の近くになる。
    for (name, source, pick) in [("ふつう", &normal, a), ("暗い", &dark, d)] {
        let before = display_median(&e, source, &s);
        let mut applied = s.clone();
        pick.values.apply(&mut applied);
        let after = display_median(&e, source, &applied);
        eprintln!("{name}: 中央値の階級 {before} → {after}");
        assert!(
            (90..=150).contains(&after),
            "{name}: {before} → {after} ({pick:?})"
        );
    }
    assert!(display_median(&e, &dark, &s) < 90);
}

#[test]
fn auto_tone_uses_the_cropped_area() {
    let e = engine(160, 64);
    let source = raw_source(raw_from(&busy_scene(240, 160)));
    let s = DevelopSettings::default();
    let whole = e.auto_tone(&source, &s, 160).unwrap();
    // グレーの段階の区画（左の 1/4）だけに切り抜くと、代表の明るさが変わる。
    let mut cropped = s.clone();
    cropped.geometry.crop = CropRect {
        left: 0.0,
        top: 0.0,
        right: 0.25,
        bottom: 1.0,
    };
    let part = e.auto_tone(&source, &cropped, 160).unwrap();
    assert_ne!(whole.stats, part.stats);
    assert!(part.stats.samples < whole.stats.samples);
    // 回転して切り抜いても求められる（表示で塗る画素は数えない）。
    let mut rotated = s.clone();
    rotated.geometry.angle_deg = 10.0;
    let r = e.auto_tone(&source, &rotated, 160).unwrap();
    assert!(r.stats.samples > 0);
}

#[test]
fn auto_tone_works_for_display_referred_input() {
    let e = engine(64, 64);
    let (w, h) = (48u32, 32u32);
    let make = |level: u8| {
        let mut bytes = Vec::new();
        for y in 0..h {
            for x in 0..w {
                // 上の 2 行に黒と白、ほかは一定の明るさ。
                let v = if y < 2 {
                    if x < w / 2 { 4 } else { 250 }
                } else {
                    level
                };
                bytes.extend_from_slice(&[v, v, v]);
            }
        }
        let enc = genzo_pipeline::input::encoded_from_u8(&bytes).unwrap();
        let image = working_image_from_encoded(w, h, &enc, None).unwrap();
        PhotoSource::new(
            SourceId::new(FileId::new(i64::from(level)), 1),
            SourceImage::Working {
                image: Arc::new(image),
                orientation: Orientation::Normal,
            },
        )
        .unwrap()
    };
    let s = DevelopSettings::default();
    // 中間グレー（sRGB の 118）の JPEG は、露光量をほとんど変えない。
    let fine = e.auto_tone(&make(118), &s, 64).unwrap();
    assert!(fine.values.exposure_ev.abs() < 0.3, "{fine:?}");
    // 暗い JPEG（sRGB の 50）は上げ、明るい JPEG（sRGB の 200）は下げる。
    let dark = e.auto_tone(&make(50), &s, 64).unwrap();
    let bright = e.auto_tone(&make(200), &s, 64).unwrap();
    eprintln!("中間 {fine:?}\n暗い {dark:?}\n明るい {bright:?}");
    assert!(dark.values.exposure_ev > 0.5, "{dark:?}");
    assert!(bright.values.exposure_ev < -0.3, "{bright:?}");
}
