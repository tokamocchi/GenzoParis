//! ステージ 12 のテクスチャ・明瞭度・かすみの除去（DEV-11）・ステージ 14 のシャープ（DEV-12）・ステージ 16 の効果
//! （切り抜き後の周辺光量補正・粒子。DEV-24）の結合テスト（処理バージョン 2）: 処理バージョン 1 での無視、タイルの
//! 分け方によらないこと、切り抜きを変えても範囲の中が変わらないこと、露光量を変えてもガイドを作り直さないこと、
//! 粒子の種、プレビューとフル解像度の差（IQ-07b）を、エンジンの処理で確かめる。

mod common;

use common::*;
use genzo_color::RgbColorSpace;
use genzo_model::{CropRect, DevelopSettings, FileId, RenderQuality};
use genzo_pipeline::finish::{UnimplementedSetting, unimplemented_settings};
use genzo_pipeline::{NeverCancel, PhotoSource, PreviewRequest, RgbImage, SourceId};
use genzo_testkit::diff::{LabConversion, delta_e2000_stats};

/// テクスチャ・明瞭度・かすみの除去・シャープ・効果をすべて使う設定（処理バージョン 2）。
fn detail_features() -> DevelopSettings {
    let mut s = DevelopSettings {
        process_version: 2,
        ..Default::default()
    };
    s.presence.texture = 40.0;
    s.presence.clarity = 35.0;
    s.presence.dehaze = 30.0;
    s.detail.sharpening.amount = 80.0;
    s.detail.sharpening.radius = 1.5;
    s.detail.sharpening.masking = 30.0;
    s.effects.vignette.amount = -40.0;
    s.effects.vignette.roundness = 20.0;
    s.effects.grain.amount = 30.0;
    s.effects.grain.size = 40.0;
    s
}

fn full(source: &PhotoSource, s: &DevelopSettings, tile: u32) -> RgbImage {
    let r = engine(64, tile)
        .render_full_resolution(source, s, None, &NeverCancel)
        .unwrap();
    assert_eq!(r.non_finite_replaced, 0);
    r.b3.image
}

#[test]
fn version_1_ignores_presence_sharpening_and_effects() {
    let source = raw_source(raw_from(&busy_scene(96, 64)));
    let mut s = detail_features();
    s.process_version = 1;
    let base = DevelopSettings {
        process_version: 1,
        ..Default::default()
    };
    assert_bitwise_eq(&full(&source, &base, 64), &full(&source, &s, 64), "版 1");
    assert_eq!(
        unimplemented_settings(&s),
        [
            UnimplementedSetting::Presence,
            UnimplementedSetting::Sharpening,
            UnimplementedSetting::PostCropVignette,
            UnimplementedSetting::Grain,
        ]
    );
    // 版 2 では適用する（結果が変わり、知らせない）。
    s.process_version = 2;
    assert!(unimplemented_settings(&s).is_empty());
    let a = full(&source, &base, 64);
    let b = full(&source, &s, 64);
    let changed = a
        .data()
        .iter()
        .zip(b.data())
        .filter(|(p, q)| p != q)
        .count();
    assert!(changed > a.data().len() * 9 / 10, "{changed}");
}

#[test]
fn results_do_not_depend_on_the_tiling() {
    let source = raw_source(raw_from(&busy_scene(160, 96)));
    let mut rotated = detail_features();
    rotate_and_crop(
        &mut rotated,
        3.0,
        CropRect {
            left: 0.05,
            top: 0.1,
            right: 0.9,
            bottom: 0.95,
        },
    );
    for (name, s) in [("回転なし", detail_features()), ("回転・切り抜き", rotated)] {
        let a = full(&source, &s, 24);
        let b = full(&source, &s, 256);
        assert_bitwise_eq(&a, &b, name);
    }
}

#[test]
fn crop_does_not_change_pixels_inside_except_the_vignette() {
    // テクスチャ・明瞭度・かすみの除去・シャープ・粒子は、切り抜く前の全体（G）の座標とガイドで決まるので、切り抜きを
    // 変えても同じ位置の画素は同じ（周辺光量補正は切り抜いた後の画像が基準なので外す）。
    let source = raw_source(raw_from(&busy_scene(160, 96)));
    let mut s = detail_features();
    s.effects.vignette.amount = 0.0;
    let e = engine(160, 64);
    let whole = e
        .render_preview(&source, &s, RenderQuality::Final, 160)
        .unwrap();
    let mut c = s.clone();
    c.geometry.crop = CropRect {
        left: 0.25,
        top: 0.2,
        right: 0.75,
        bottom: 0.9,
    };
    let part = e
        .render_preview(&source, &c, RenderQuality::Final, 160)
        .unwrap();
    let (ox, oy) = {
        let a = whole.geometry.crop_lattice();
        let b = part.geometry.crop_lattice();
        ((b.x - a.x) as u32, (b.y - a.y) as u32)
    };
    let (pw, ph) = part.b3.image.dimensions();
    for y in 0..ph {
        for x in 0..pw {
            let p = part.b3.image.pixel(x, y).unwrap();
            let q = whole.b3.image.pixel(x + ox, y + oy).unwrap();
            assert!(
                p.iter().zip(q).all(|(a, b)| a.to_bits() == b.to_bits()),
                "({x}, {y}): {p:?} ≠ {q:?}"
            );
        }
    }
}

#[test]
fn exposure_does_not_rebuild_the_guide_and_dehaze_adds_the_haze_estimate() {
    let source = raw_source(raw_from(&busy_scene(96, 64)));
    let e = engine(96, 64);
    let mut s = DevelopSettings::default();
    s.presence.clarity = 50.0;
    let r = e
        .render_preview(&source, &s, RenderQuality::Final, 96)
        .unwrap();
    assert!(r.used_guide);
    assert_eq!(e.stats().guide.misses, 1);
    s.exposure_ev = 1.0;
    s.presence.texture = -30.0;
    e.render_preview(&source, &s, RenderQuality::Final, 96)
        .unwrap();
    assert_eq!(
        e.stats().guide.misses,
        1,
        "露光量・テクスチャではガイドを作り直さない"
    );
    // かすみの除去を使うと、かすみの推定の付いたガイドを 1 回だけ作る（量を変えても作り直さない）。
    s.presence.dehaze = 40.0;
    e.render_preview(&source, &s, RenderQuality::Final, 96)
        .unwrap();
    assert_eq!(e.stats().guide.misses, 2);
    s.presence.dehaze = -20.0;
    e.render_preview(&source, &s, RenderQuality::Final, 96)
        .unwrap();
    assert_eq!(e.stats().guide.misses, 2);
    // 粒子・周辺光量補正・シャープだけならガイドを使わない。
    let mut t = DevelopSettings::default();
    t.effects.grain.amount = 50.0;
    t.effects.vignette.amount = 30.0;
    t.detail.sharpening.amount = 50.0;
    let r = e
        .render_preview(&source, &t, RenderQuality::Final, 96)
        .unwrap();
    assert!(!r.used_guide);
}

#[test]
fn dehaze_raises_the_contrast_of_the_hazy_part() {
    // 左右で同じ模様の、右だけにかすみ（白い膜）がかかったシーン。
    let scene = busy_scene(160, 96);
    let hazy = genzo_testkit::FloatImage::from_rgb_fn(160, 96, |x, y| {
        let p = scene.pixel(x, y).unwrap();
        let p = [p[0], p[1], p[2]];
        if x >= 80 {
            p.map(|v| v * 0.4 + 0.45 * 0.6)
        } else {
            p
        }
    })
    .unwrap();
    let source = raw_source(raw_from(&hazy));
    let spread = |img: &RgbImage, x0: u32| {
        let ys: Vec<f32> = (8..88)
            .flat_map(|y| (x0 + 8..x0 + 72).map(move |x| (x, y)))
            .map(|(x, y)| {
                let p = img.pixel(x, y).unwrap();
                0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]
            })
            .collect();
        let m = ys.iter().sum::<f32>() / ys.len() as f32;
        (ys.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / ys.len() as f32).sqrt()
    };
    let base = full(&source, &DevelopSettings::default(), 64);
    let mut s = DevelopSettings::default();
    s.presence.dehaze = 100.0;
    let out = full(&source, &s, 64);
    let gain_hazy = spread(&out, 80) / spread(&base, 80);
    assert!(gain_hazy > 1.3, "{gain_hazy}");
    // 負の値はかすみを足す（明暗の差が小さくなる）。
    s.presence.dehaze = -100.0;
    let added = full(&source, &s, 64);
    assert!(spread(&added, 0) < spread(&base, 0) * 0.9);
}

#[test]
fn grain_depends_on_the_photo_and_is_the_same_for_the_same_photo() {
    let raw = raw_from(&busy_scene(96, 64));
    let mut s = DevelopSettings::default();
    s.effects.grain.amount = 60.0;
    let with_id = |id: i64| {
        PhotoSource::new(
            SourceId::new(FileId::new(id), 1),
            genzo_pipeline::SourceImage::Raw(std::sync::Arc::new(raw.clone())),
        )
        .unwrap()
    };
    let a = full(&with_id(1), &s, 64);
    let b = full(&with_id(1), &s, 32);
    assert_bitwise_eq(&a, &b, "同じ写真");
    let c = full(&with_id(2), &s, 64);
    let differ = a
        .data()
        .iter()
        .zip(c.data())
        .filter(|(p, q)| p != q)
        .count();
    assert!(differ > a.data().len() / 2, "{differ}");
    // 内容のリビジョンが変わっても模様は同じ（元ファイルの ID だけで決まる）。
    assert_eq!(
        SourceId::new(FileId::new(5), 1).photo_seed(),
        SourceId::new(FileId::new(5), 9).photo_seed()
    );
    assert_ne!(
        SourceId::new(FileId::new(5), 1).photo_seed(),
        SourceId::new(FileId::new(6), 1).photo_seed()
    );
}

#[test]
fn sharpening_is_weak_in_the_downscaled_preview_and_strong_at_full_resolution() {
    // 半径はフル解像度の画素なので、縮小したプレビューでは σ が小さく効きが小さい（細かい明暗は平均される）。
    let source = raw_source(raw_from(&thin_lines_scene(192, 128)));
    let e = engine(64, 64);
    let mut s = DevelopSettings::default();
    s.detail.sharpening.amount = 150.0;
    let changed = |sharp: &RgbImage, base: &RgbImage| {
        sharp
            .data()
            .iter()
            .zip(base.data())
            .map(|(p, q)| (p[1] - q[1]).abs())
            .sum::<f32>()
            / sharp.data().len() as f32
    };
    let req = PreviewRequest::new(RenderQuality::Final, 64);
    let p0 = e
        .render_preview_with(&source, &DevelopSettings::default(), &req)
        .unwrap()
        .b3
        .image;
    let p1 = e.render_preview_with(&source, &s, &req).unwrap().b3.image;
    let f0 = full(&source, &DevelopSettings::default(), 64);
    let f1 = full(&source, &s, 64);
    let preview_change = changed(&p1, &p0);
    let full_change = changed(&f1, &f0);
    assert!(
        full_change > preview_change * 2.0,
        "{full_change} {preview_change}"
    );
}

#[test]
fn preview_and_full_resolution_differences_do_not_regress() {
    // IQ-07b（プレビューとフル解像度の差）の計測値の回帰の検出（基準の判定ではない。engine.rs の
    // iq07b_measured_values_do_not_regress と同じ考え方）。明瞭度・かすみの除去は段階 B のガイドとかすみの推定を
    // フル解像度でも使い、テクスチャ・シャープの半径・粒子の模様は G の長辺に対する割合で縮小率に合わせるので、
    // 差は既定の設定に近い。表は、このコンテナで測った値（平均・95 パーセンタイル・最大）に 10% と 0.1 の余裕を
    // 足した上限。このシーン（長辺 300px）ではテクスチャの σ が 1 画素より小さく、テクスチャは効かない（既定と同じ
    // 値。写真の大きさでの確認は texture_works_at_photo_size_and_the_preview_follows_the_area_average）。
    let scene = wavy_scene(300, 201);
    let source = raw_source(raw_from(&scene));
    let e = engine(100, 128);
    let lab = LabConversion::linear(RgbColorSpace::Bt2020);
    let limits = |v: [f64; 3]| v.map(|x| x * 1.1 + 0.1);
    let with = |f: &dyn Fn(&mut DevelopSettings)| {
        let mut s = DevelopSettings::default();
        f(&mut s);
        s
    };
    for (name, s, measured) in [
        ("既定", DevelopSettings::default(), [0.211, 0.734, 6.973]),
        (
            "テクスチャ",
            with(&|s| s.presence.texture = 60.0),
            [0.211, 0.734, 6.973],
        ),
        (
            "明瞭度",
            with(&|s| s.presence.clarity = 60.0),
            [0.303, 1.090, 6.837],
        ),
        (
            "かすみの除去",
            with(&|s| s.presence.dehaze = 60.0),
            [0.319, 1.065, 6.973],
        ),
        (
            "シャープ",
            with(&|s| s.detail.sharpening.amount = 100.0),
            [0.267, 0.981, 7.283],
        ),
        (
            "周辺光量",
            with(&|s| s.effects.vignette.amount = -60.0),
            [0.207, 0.720, 6.326],
        ),
        (
            "粒子",
            with(&|s| s.effects.grain.amount = 50.0),
            [0.249, 0.764, 7.024],
        ),
        ("すべて", detail_features(), [0.346, 1.214, 7.171]),
    ] {
        let preview = e
            .render_preview(&source, &s, RenderQuality::Final, 100)
            .unwrap();
        let reference = iq07b_reference(&e, &source, &s, &preview);
        let st = delta_e2000_stats(&reference, &to_float(&preview.b3.image), lab).unwrap();
        eprintln!(
            "IQ-07b wavy/{name}: 平均 {:.3}、95 パーセンタイル {:.3}、最大 {:.3}",
            st.mean, st.p95, st.max
        );
        let [m, p, x] = limits(measured);
        assert!(st.mean <= m && st.p95 <= p && st.max <= x, "{name}: {st:?}");
    }
}

#[test]
fn texture_works_at_photo_size_and_the_preview_follows_the_area_average() {
    // テクスチャの σ は G の長辺の 0.08 %（長辺 2000px で 1.6px）。細かい模様（周期 4px の格子と、ゆるやかな明暗）の
    // RAW 以外の入力で、フル解像度では細かい模様が強まり、長辺 500px のプレビュー（σ 0.4px）はフル解像度を画素の
    // 範囲で平均したものに近い（テクスチャは局所の平均をほぼ保つので、縮小すると差が小さい）。
    use std::sync::Arc;

    use genzo_model::Orientation;
    use genzo_pipeline::{ColorContract, SourceImage};
    let (w, h) = (2000u32, 1200u32);
    let image = RgbImage::from_fn(w, h, ColorContract::B2Working, |x, y| {
        let base = 0.05 + 0.3 * (x as f32 / w as f32) + 0.1 * (y as f32 / h as f32);
        let fine = if (x / 2 + y / 2) % 2 == 0 { 1.15 } else { 0.87 };
        [base * fine, base * fine * 0.9, base * fine * 0.8]
    })
    .unwrap();
    let source = PhotoSource::new(
        SourceId::new(FileId::new(3), 1),
        SourceImage::Working {
            image: Arc::new(image),
            orientation: Orientation::Normal,
        },
    )
    .unwrap();
    let e = engine(500, 512);
    let mut s = DevelopSettings::default();
    let base = full(&source, &s, 512);
    s.presence.texture = 100.0;
    let tex = full(&source, &s, 512);
    // 隣り合う画素の明るさの比（細かい模様の強さ）が大きくなる。
    let contrast = |img: &RgbImage| {
        let a = img.pixel(1001, 601).unwrap()[1];
        let b = img.pixel(1003, 601).unwrap()[1];
        (a / b).max(b / a)
    };
    assert!(
        contrast(&tex) > contrast(&base) * 1.05,
        "{} {}",
        contrast(&tex),
        contrast(&base)
    );
    // プレビューとフル解像度の画素の範囲の平均の差（IQ-07b）は、ΔE2000 で 1 より小さい（テクスチャは対数の領域の
    // 局所の平均を保つので、リニアの平均はわずかに変わる。このコンテナの計測で平均 0.469・最大 0.544）。
    let lab = LabConversion::linear(RgbColorSpace::Bt2020);
    let measure = |s: &DevelopSettings| {
        let preview = e
            .render_preview(&source, s, RenderQuality::Final, 500)
            .unwrap();
        let reference = iq07b_reference(&e, &source, s, &preview);
        delta_e2000_stats(&reference, &to_float(&preview.b3.image), lab).unwrap()
    };
    let plain = measure(&DevelopSettings::default());
    let with_texture = measure(&s);
    eprintln!("IQ-07b 細かい模様: テクスチャなし {plain:?}、テクスチャ +100 {with_texture:?}");
    assert!(plain.max < 0.01, "{plain:?}");
    assert!(with_texture.max < 1.0, "{with_texture:?}");
}
