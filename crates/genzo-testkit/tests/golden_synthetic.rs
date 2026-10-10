//! 合成画像の基準画像による回帰テスト（05 の M0 タスク 11 の完了の条件「サンプル画像でテストが動く」）。
//!
//! 合成のテストデータ（ColorChecker・ゾーンプレートのベイヤー化）の結果を、
//! `tests/golden/` の基準画像と比べる。合成の手順（genzo-testkit・genzo-color・genzo-raw の型）を
//! 変えて結果が変わったら失敗する。意図した変更なら `GENZO_UPDATE_GOLDEN=1 cargo test -p genzo-testkit`
//! で基準を作り直し、差分を確認してからコミットする。
//!
//! 入力のハッシュには、生成した画像のバイト列ではなく **生成のパラメータの文字列** を使う。
//! 三角関数・累乗の結果は OS の数学ライブラリによって最下位のビットが違いうるため、生成した画像の
//! ハッシュは OS ごとに変わりうる（比較そのものは許容差で吸収する）。

use genzo_color::RgbColorSpace;
use genzo_color::transfer::srgb_encode_f32;
use genzo_raw::CfaPattern;
use genzo_testkit::bayer::{BayerParams, mosaic, normalize_cfa};
use genzo_testkit::colorchecker::{ColorCheckerEdition, ColorCheckerLayout, color_checker_image};
use genzo_testkit::diff::Tolerance;
use genzo_testkit::golden::{GoldenOutcome, GoldenSpec, GoldenStore};
use genzo_testkit::record::InputRef;
use genzo_testkit::synth::zone_plate;

/// 合成の手順の回帰を検出する許容差（8bit 換算で 0.01）。
///
/// 同じ手順なら OS の数学ライブラリの違い（f32 の 1 ulp 程度。8bit 換算で 1e-5 程度）しか差が出ない
/// ので、IQ-07a の ±1 よりずっと小さい値にした。
///
/// 注意: ゾーンプレートのベイヤー化は 14bit に量子化するので、OS の違いで量子化の境目をまたいで
/// 1 DN 変わると、B0 で 1 / (16383 − 516) ≒ 6.3e-5、8bit 換算で約 0.016 になり、この許容差を超える。
/// ただし cos（f64）の最下位のビットの違いは f32 に丸めた時点でほぼ消え、さらに量子化の境目を
/// またぐ必要があるため、起こる確率は非常に小さいと見積もった（未確認。macOS・Windows の CI で
/// この基準だけが 1 画素で失敗した場合はこれを疑う）。
const SYNTH_TOLERANCE_8BIT: f64 = 0.01;

fn store() -> GoldenStore {
    GoldenStore::new(genzo_testkit::golden_dir!())
}

fn check(spec: &GoldenSpec, image: &genzo_testkit::FloatImage) {
    match store().assert_matches(spec, image) {
        GoldenOutcome::Passed(report) => assert!(report.passed()),
        GoldenOutcome::Updated { .. } => {}
    }
}

#[test]
fn colorchecker_after_nov2014_srgb_encoded() {
    let params = "colorchecker edition=after_nov2014 space=srgb encoding=iec61966-2-1 patch=8 gap=2 background=0.02 exposure=1";
    let layout = ColorCheckerLayout {
        patch_size: 8,
        gap: 2,
    };
    let linear = color_checker_image(
        ColorCheckerEdition::AfterNov2014,
        RgbColorSpace::Srgb,
        layout,
        [0.02; 3],
        1.0,
    )
    .unwrap();
    let encoded = linear.map(srgb_encode_f32);
    let spec = GoldenSpec::new(
        "colorchecker_after_nov2014_srgb",
        Tolerance::diff_8bit(SYNTH_TOLERANCE_8BIT),
    )
    .with_description(
        "ColorChecker Classic（2014 年 11 月以降）を D65 に順応してリニア sRGB にし、IEC 61966-2-1 で符号化した画像（色域外は負の値のまま）",
    )
    .with_input(InputRef::from_bytes(
        "synthetic:colorchecker_after_nov2014",
        params.as_bytes(),
    ));
    check(&spec, &encoded);
}

#[test]
fn zone_plate_bayer_normalized() {
    let params = "zone_plate 48x40 max_frequency=0.5 low=0.02 high=0.9; bayer cfa=GRBG black=[510,512,514,516] white=16383 wb=[2.0,1.0,1.5,1.0]; normalize";
    let scene = zone_plate(48, 40, 0.5, 0.02, 0.9).unwrap();
    let p = BayerParams {
        cfa: CfaPattern::GRBG,
        black_level: [510.0, 512.0, 514.0, 516.0],
        white_level: 16383.0,
        wb: [2.0, 1.0, 1.5, 1.0],
        ..BayerParams::default()
    };
    let raw = mosaic(&scene, &p).unwrap();
    let b0 = normalize_cfa(&raw).unwrap();
    let spec = GoldenSpec::new(
        "zone_plate_bayer_b0",
        Tolerance::diff_8bit(SYNTH_TOLERANCE_8BIT),
    )
    .with_description(
        "ゾーンプレートを逆 WB・GRBG でモザイク化・14bit に量子化し、黒レベル 0・白レベル 1 に正規化した CFA（B0）",
    )
    .with_input(InputRef::from_bytes(
        "synthetic:zone_plate_bayer",
        params.as_bytes(),
    ));
    check(&spec, &b0);
}
