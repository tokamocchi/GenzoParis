//! 回帰テスト（02 の MAINT-03・IQ-08）: 合成のベイヤー画像 ＋ 代表的な現像設定の書き出しの結果を、
//! 基準画像（`tests/golden/`）と比べる。
//!
//! - 入力: 整数の演算と定数の表だけで作った合成のシーン（`common::busy_scene`、192 × 128）を、
//!   testkit の合成ベイヤー（RGGB・14bit・撮影時の WB は testkit の既定・テスト用のカメラ行列）にした
//!   もの。公開してよい合成画像だけを使う（05 の 3 章）。入力の記録は RAW の画素のハッシュ
//!   （整数の演算だけなので OS によらない）。
//! - 出力: エンジンのフル解像度の処理（タイル 64 px）の、sRGB の書き出し（ステージ 17b の B4b。
//!   量子化の前の f32）。
//! - 許容差: IQ-07a（8bit 換算で ±1）。Windows / macOS の数学ライブラリの差（`exp2`・`log2`・`powf`・
//!   `sin`・`cos` の最下位のビット）はこれより十分小さい。
//! - 基準の作り直し: `GENZO_UPDATE_GOLDEN=1 cargo test -p genzo-pipeline --test engine_golden`
//!   （CI では作り直さない。genzo-testkit の golden の doc）。作り直したら git の差分を確認してから
//!   コミットする。

mod common;

use common::*;
use genzo_model::{CropRect, CurvePoint, DevelopSettings, OutputColorSpace};
use genzo_pipeline::{NeverCancel, OutputTarget, PhotoSource, SourceImage};
use genzo_testkit::diff::Tolerance;
use genzo_testkit::golden::{GoldenSpec, GoldenStore};
use genzo_testkit::record::InputRef;

/// 合成の RAW の大きさ。
const WIDTH: u32 = 192;
/// 合成の RAW の大きさ。
const HEIGHT: u32 = 128;

/// 代表的な現像設定（名前、説明、設定）。
fn variants() -> Vec<(&'static str, &'static str, DevelopSettings)> {
    let mut out = vec![("engine_default", "既定の設定", DevelopSettings::default())];
    out.push((
        "engine_exposure_plus1",
        "露光量 +1",
        DevelopSettings {
            exposure_ev: 1.0,
            ..Default::default()
        },
    ));
    let mut s = DevelopSettings::default();
    custom_wb(&mut s, 4300.0, 8.0);
    out.push(("engine_wb_custom", "WB 4300K・tint +8", s));
    let mut s = DevelopSettings {
        contrast: 20.0,
        ..Default::default()
    };
    s.tone.highlights = -60.0;
    s.tone.shadows = 50.0;
    s.tone.whites = 15.0;
    s.tone.blacks = -10.0;
    out.push((
        "engine_tone",
        "コントラスト・ハイライト・シャドウ・白・黒（ガイドを使う）",
        s,
    ));
    let mut s = DevelopSettings::default();
    s.tone_curve.luma = vec![
        CurvePoint::new(0.0, 0.05),
        CurvePoint::new(0.3, 0.22),
        CurvePoint::new(0.7, 0.8),
        CurvePoint::new(1.0, 0.97),
    ];
    s.tone_curve.parametric.lights = 20.0;
    s.tone_curve.red = vec![
        CurvePoint::new(0.0, 0.0),
        CurvePoint::new(0.5, 0.55),
        CurvePoint::new(1.0, 1.0),
    ];
    out.push((
        "engine_tone_curve",
        "トーンカーブ（輝度・パラメトリック・R）",
        s,
    ));
    let mut s = DevelopSettings::default();
    s.color.saturation = 40.0;
    s.color.vibrance = 30.0;
    out.push(("engine_saturation", "彩度 +40・自然な彩度 +30", s));
    let mut s = DevelopSettings::default();
    rotate_and_crop(
        &mut s,
        4.0,
        CropRect {
            left: 0.1,
            top: 0.1,
            right: 0.85,
            bottom: 0.9,
        },
    );
    out.push(("engine_crop_rotate", "回転 4°・切り抜き", s));
    out
}

/// 入力の記録（RAW の画素と、作り方のパラメータのハッシュ）。
fn input_ref(source: &PhotoSource) -> InputRef {
    let SourceImage::Raw(raw) = &source.image else {
        unreachable!("RAW の入力");
    };
    let mut bytes = format!(
        "busy_scene {}x{} cfa=RGGB black={:?} white={} wb={:?} cam_xyz={:?}\n",
        raw.width, raw.height, raw.black_level, raw.white_level, raw.as_shot_wb, raw.cam_xyz
    )
    .into_bytes();
    for v in &raw.data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    InputRef::from_bytes(format!("synthetic:busy_bayer:{WIDTH}x{HEIGHT}"), &bytes)
}

#[test]
fn export_results_match_the_golden_images() {
    let source = raw_source(raw_from(&busy_scene(WIDTH, HEIGHT)));
    let input = input_ref(&source);
    let e = engine(96, 64);
    let store = GoldenStore::new(genzo_testkit::golden_dir!());
    let srgb = OutputTarget::Export(OutputColorSpace::Srgb);
    let mut failures = Vec::new();
    for (name, description, settings) in variants() {
        let r = e
            .render_full_resolution(&source, &settings, Some(&srgb), &NeverCancel)
            .unwrap();
        assert_eq!(r.non_finite_replaced, 0, "{name}");
        let out = r.output.expect("出力を求めた");
        let spec = GoldenSpec::new(name, Tolerance::iq07a())
            .with_description(format!(
                "{description}（合成のベイヤー {WIDTH}x{HEIGHT}、sRGB の書き出しの B4b）"
            ))
            .with_input(input.clone())
            .with_develop_settings(&settings);
        if let Err(err) = store.check(&spec, &to_float(&out.image)) {
            failures.push(format!("{name}: {err}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
