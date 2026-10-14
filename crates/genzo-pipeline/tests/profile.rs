//! カメラプロファイル（DCP）・ルック（3D LUT）の結合テスト（DEV-17。処理バージョン 2）: 標準の色との関係、色相彩度の
//! マップ・ルックテーブル・ルックが効くこと、処理バージョン 1 での無視、見つからないときの警告、キャッシュのキー、
//! タイルの分け方によらないことを、エンジンの処理で確かめる。テストの DCP・LUT はテストの中で作る。

mod common;

use std::sync::Arc;

use common::*;
use genzo_color::{CubeLut, DcpProfile, HsvTable, Lut3d, Mat3};
use genzo_model::{DevelopSettings, RenderQuality};
use genzo_pipeline::finish::{UnimplementedSetting, unimplemented_settings};
use genzo_pipeline::{Engine, MemoryProfiles, NeverCancel, PhotoSource, RgbImage, SensorWarning};

/// RAW の行列と同じ ColorMatrix（D65）のプロファイル（テストの値。実在の機種の値ではない）。
fn plain_profile() -> DcpProfile {
    let m = typical_camera().map(|r| r.map(f64::from));
    DcpProfile {
        name: Some("テスト標準".into()),
        unique_camera_model: Some("GenzoParis Synthetic".into()),
        calibration_illuminants: [Some(21), None],
        color_matrices: [Some(Mat3::from_rows(m)), None],
        ..Default::default()
    }
}

/// 色相を 40 度回す色相彩度のマップと、彩度を下げるルックテーブル（明度の分割 4・sRGB の符号化）を持つプロファイル。
fn mapped_profile() -> DcpProfile {
    let hsm = HsvTable::new([6, 3, 1], false, vec![[40.0, 1.0, 1.0]; 6 * 3]).unwrap();
    let mut look = Vec::new();
    for v in 0..4 {
        for _h in 0..6 {
            for _s in 0..3 {
                look.push([0.0, 0.6, 1.0 - 0.05 * v as f32]);
            }
        }
    }
    DcpProfile {
        hue_sat_maps: [Some(hsm), None],
        look_table: Some(HsvTable::new([6, 3, 4], true, look).unwrap()),
        ..plain_profile()
    }
}

fn inverting_look() -> CubeLut {
    let lut = Lut3d::from_fn(9, |v| [1.0 - v[0], 1.0 - v[1], 1.0 - v[2]]).unwrap();
    CubeLut::new(
        Some("反転".into()),
        9,
        lut.data().to_vec(),
        [0.0; 3],
        [1.0; 3],
    )
    .unwrap()
}

fn library() -> Arc<MemoryProfiles> {
    let mut lib = MemoryProfiles::new();
    lib.insert_camera("plain", plain_profile(), &"11".repeat(32));
    lib.insert_camera("mapped", mapped_profile(), &"22".repeat(32));
    lib.insert_look("invert", inverting_look(), &"33".repeat(32));
    Arc::new(lib)
}

fn engine_with(lib: Arc<MemoryProfiles>, preview: u32, tile: u32) -> Engine {
    engine(preview, tile).with_profile_resolver(lib)
}

fn full(e: &Engine, source: &PhotoSource, s: &DevelopSettings) -> RgbImage {
    let r = e
        .render_full_resolution(source, s, None, &NeverCancel)
        .unwrap();
    assert_eq!(r.non_finite_replaced, 0);
    r.b3.image
}

fn max_abs_diff(a: &RgbImage, b: &RgbImage) -> f32 {
    a.data()
        .iter()
        .zip(b.data())
        .flat_map(|(p, q)| (0..3).map(move |k| (p[k] - q[k]).abs()))
        .fold(0.0, f32::max)
}

#[test]
fn plain_profile_with_a_d65_white_is_close_to_the_standard_color() {
    // RAW の行列と同じ D65 の ColorMatrix だけのプロファイルは、白が D65 に近いと標準のステージ 8 に近い（色順応の
    // 方法が違う分だけ違う）。
    let source = raw_source(raw_from(&gray_patches_scene(96, 64)));
    let e = engine_with(library(), 64, 64);
    let mut std = DevelopSettings::default();
    custom_wb(&mut std, 6504.0, 0.0);
    let mut s = std.clone();
    s.profile.camera = Some("plain".into());
    let a = full(&e, &source, &std);
    let b = full(&e, &source, &s);
    // 上半分のグレーのパッチは、ほぼ同じ（中立は中立のまま、明るさも同じ）。色のパッチは色順応の方法の違いの分だけ
    // 違う（白は D65 に近いが同じではない）。
    let (w, h) = a.dimensions();
    let mut gray = 0.0f32;
    let mut color = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            let (p, q) = (a.pixel(x, y).unwrap(), b.pixel(x, y).unwrap());
            let d = (0..3).map(|k| (p[k] - q[k]).abs()).fold(0.0, f32::max);
            if y < h / 2 - 2 {
                gray = gray.max(d);
            } else if y > h / 2 + 2 {
                color = color.max(d);
            }
        }
    }
    assert!(gray < 5e-4, "グレーの差 {gray}");
    assert!(color < 0.02, "色の差 {color}");
    assert!(max_abs_diff(&a, &b) < 0.02);
    // 機種が違うことを知らせる（合成の RAW の機種はプロファイルの機種と違う）。
    let p = e
        .render_preview(&source, &s, RenderQuality::Final, 64)
        .unwrap();
    assert!(
        p.warnings
            .contains(&SensorWarning::CameraProfileOtherCamera),
        "{:?}",
        p.warnings
    );
}

#[test]
fn hue_sat_map_and_look_table_change_colors_but_keep_neutrals() {
    let source = raw_source(raw_from(&gray_patches_scene(96, 64)));
    let e = engine_with(library(), 64, 64);
    let mut plain = DevelopSettings::default();
    plain.profile.camera = Some("plain".into());
    let mut mapped = plain.clone();
    mapped.profile.camera = Some("mapped".into());
    let a = full(&e, &source, &plain);
    let b = full(&e, &source, &mapped);
    // 上半分はグレーのパッチ（中立。明度の倍率で明るさだけ変わりうる）、下半分は色のパッチ。
    let (w, h) = a.dimensions();
    let mut color_changed = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            let (p, q) = (a.pixel(x, y).unwrap(), b.pixel(x, y).unwrap());
            if y < h / 2 - 2 {
                // 中立は中立のまま（チャンネルの差は、デモザイクでパッチの境目に出るわずかな色より大きくならない）。
                let spread = |c: [f32; 3]| c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
                assert!(
                    spread(q) <= spread(p) * 1.5 + 2e-5,
                    "({x}, {y}) {p:?} {q:?}"
                );
            } else if y > h / 2 + 2 {
                color_changed = color_changed.max((p[0] - q[0]).abs() + (p[2] - q[2]).abs());
            }
        }
    }
    assert!(color_changed > 0.02, "{color_changed}");
    // 段階 C にルックテーブルのステージが入る。
    let p = e
        .render_preview(&source, &mapped, RenderQuality::Final, 64)
        .unwrap();
    assert!(p.active_stages.contains(&"finish.profile_look_table"));
    let p = e
        .render_preview(&source, &plain, RenderQuality::Final, 64)
        .unwrap();
    assert!(!p.active_stages.contains(&"finish.profile_look_table"));
}

#[test]
fn look_is_applied_after_scene_to_display_and_blends_with_the_amount() {
    let source = raw_source(raw_from(&busy_scene(96, 64)));
    let e = engine_with(library(), 64, 64);
    let base = DevelopSettings::default();
    let mut s = base.clone();
    s.profile.look = Some("invert".into());
    let p = e
        .render_preview(&source, &s, RenderQuality::Final, 64)
        .unwrap();
    let ids = &p.active_stages;
    let pos = |id: &str| ids.iter().position(|x| *x == id).unwrap();
    assert!(pos("finish.scene_to_display") < pos("finish.look"));
    let a = full(&e, &source, &base);
    let b = full(&e, &source, &s);
    assert!(max_abs_diff(&a, &b) > 0.3);
    // 適用量 0 は飛ばす（ビット単位で同じ）。
    s.profile.look_amount = 0.0;
    assert_bitwise_eq(&a, &full(&e, &source, &s), "適用量 0");
    // RAW 以外の写真にも使える（ルックは段階 C）。
    let working = genzo_pipeline::PhotoSource::new(
        genzo_pipeline::SourceId::new(genzo_model::FileId::new(9), 1),
        genzo_pipeline::SourceImage::Working {
            image: Arc::new(a.clone().with_contract_b2()),
            orientation: genzo_model::Orientation::Normal,
        },
    )
    .unwrap();
    s.profile.look_amount = 100.0;
    let p = e
        .render_preview(&working, &s, RenderQuality::Final, 64)
        .unwrap();
    assert!(p.active_stages.contains(&"finish.look"));
}

/// テスト用: B3 の画像を B2 として RAW 以外の入力にする。
trait WithB2 {
    fn with_contract_b2(self) -> RgbImage;
}
impl WithB2 for RgbImage {
    fn with_contract_b2(mut self) -> RgbImage {
        self.set_contract(genzo_pipeline::ColorContract::B2Working);
        self
    }
}

#[test]
fn version_1_ignores_profiles_and_missing_items_are_reported() {
    let source = raw_source(raw_from(&busy_scene(96, 64)));
    let e = engine_with(library(), 64, 64);
    let base = DevelopSettings {
        process_version: 1,
        ..Default::default()
    };
    let mut s = base.clone();
    s.profile.camera = Some("mapped".into());
    s.profile.look = Some("invert".into());
    assert_bitwise_eq(&full(&e, &source, &base), &full(&e, &source, &s), "版 1");
    assert_eq!(
        unimplemented_settings(&s),
        [
            UnimplementedSetting::CameraProfile,
            UnimplementedSetting::Look
        ]
    );
    // 版 2 で見つからなければ、標準の色・ルックなしで描いて知らせる。
    let base2 = DevelopSettings::default();
    let mut m = base2.clone();
    m.profile.camera = Some("ない".into());
    m.profile.look = Some("ない".into());
    assert_bitwise_eq(
        &full(&e, &source, &base2),
        &full(&e, &source, &m),
        "見つからない",
    );
    let p = e
        .render_preview(&source, &m, RenderQuality::Final, 64)
        .unwrap();
    assert!(p.warnings.contains(&SensorWarning::CameraProfileMissing));
    assert!(p.warnings.contains(&SensorWarning::LookMissing));
    let job = e.full_resolution(&source, &m).unwrap();
    assert!(job.warnings().contains(&SensorWarning::LookMissing));
    // 仕組みのないエンジンでも同じ（見つからない）。
    let plain_engine = engine(64, 64);
    let p = plain_engine
        .render_preview(&source, &m, RenderQuality::Final, 64)
        .unwrap();
    assert!(p.warnings.contains(&SensorWarning::CameraProfileMissing));
}

#[test]
fn cache_keys_follow_the_profile_contents() {
    let source = raw_source(raw_from(&busy_scene(96, 64)));
    let lib = library();
    let e = engine_with(Arc::clone(&lib), 64, 64);
    let mut s = DevelopSettings::default();
    s.profile.camera = Some("plain".into());
    e.render_preview(&source, &s, RenderQuality::Final, 64)
        .unwrap();
    assert_eq!(e.stats().a1.misses, 1);
    // ルックだけを変えても A1 は作り直さない。
    s.profile.look = Some("invert".into());
    e.render_preview(&source, &s, RenderQuality::Final, 64)
        .unwrap();
    s.profile.look_amount = 40.0;
    e.render_preview(&source, &s, RenderQuality::Final, 64)
        .unwrap();
    assert_eq!(e.stats().a1.misses, 1);
    // プロファイルを変えると作り直す。
    s.profile.camera = Some("mapped".into());
    e.render_preview(&source, &s, RenderQuality::Final, 64)
        .unwrap();
    assert_eq!(e.stats().a1.misses, 2);
    // 同じ ID でも中身（SHA-256）が違えば、キーが違う。
    let mut other = MemoryProfiles::new();
    other.insert_camera("mapped", plain_profile(), &"44".repeat(32));
    let e2 = engine(64, 64).with_profile_resolver(Arc::new(other));
    assert_ne!(
        e.a1_cache_key(&source, &s, RenderQuality::Final, 64),
        e2.a1_cache_key(&source, &s, RenderQuality::Final, 64)
    );
    // キーに使う設定は、引いたファイルの参照を持つ。
    let eff = e.effective_settings(&s);
    assert_eq!(eff.render_deps.profile.unwrap().id, "dcp:mapped");
    assert_eq!(eff.render_deps.look.unwrap().id, "cube:invert");
}

#[test]
fn results_do_not_depend_on_tiles_and_preview_matches_full_resolution() {
    let source = raw_source(raw_from(&busy_scene(160, 100)));
    let mut s = strong_settings();
    s.profile.camera = Some("mapped".into());
    s.profile.look = Some("invert".into());
    s.profile.look_amount = 60.0;
    let reference = full(&engine_with(library(), 64, 1024), &source, &s);
    for tile in [32, 48, 64] {
        let t = full(&engine_with(library(), 64, tile), &source, &s);
        assert_bitwise_eq(&reference, &t, &format!("タイル {tile}"));
    }
    // WB のスポイトはプロファイルを選んでいても使える（プロファイルなしの A1 で求める）。
    let e = engine_with(library(), 64, 64);
    let pick = e.white_balance_at(&source, &s, (0.1, 0.1), false).unwrap();
    let mut no_profile = s.clone();
    no_profile.profile.camera = None;
    let pick2 = e
        .white_balance_at(&source, &no_profile, (0.1, 0.1), false)
        .unwrap();
    assert_eq!(pick, pick2);
}
