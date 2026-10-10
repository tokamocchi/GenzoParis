//! 合成のベイヤー配列の RAW を合成 DNG にし、LibRaw で展開して元に戻ることを確かめる。
//!
//! シーン → カメラ RGB（ステージ 8 の逆）→ 逆 WB・モザイク化・量子化 → DNG → LibRaw（段階 A0）
//! の経路で、画素の値・CFA の配列・黒レベル・白レベル・WB が保たれることを確かめる。
//! 実機の ARW での確認は PoC-2 で行う。
//!
//! genzo-raw の LibRaw が無効な build（[`LIBRAW_ENABLED`] が偽）では何もしない。
//! `cargo test -p genzo-testkit --features libraw`、または workspace 全体を
//! `--features genzo-cli/libraw` で build したとき（機能フラグが統合される）に実行される。

use genzo_color::RgbColorSpace;
use genzo_raw::{CfaPattern, LIBRAW_ENABLED, decode_bytes};
use genzo_testkit::bayer::{BayerParams, encode_dng, mosaic, normalize_cfa, working_to_camera};
use genzo_testkit::colorchecker::{ColorCheckerEdition, ColorCheckerLayout, color_checker_image};
use genzo_testkit::diff::diff_8bit;

/// テスト用の「典型的な形の」カメラ行列（genzo-raw のテストと同じ値。特定の機種の値ではない）。
const CAM_XYZ: [[f32; 3]; 3] = [
    [0.7424, -0.2329, -0.0466],
    [-0.4598, 1.2471, 0.2347],
    [-0.0715, 0.1505, 0.6066],
];

#[test]
fn synthetic_bayer_survives_dng_and_libraw() {
    if !LIBRAW_ENABLED {
        eprintln!("LibRaw が無効な build のため、合成 DNG の展開の確認を省略します");
        return;
    }
    // 6 × 4 のパッチ（一辺 6 画素、余白 2）で 50 × 34 画素（LibRaw の最小 22 画素以上）。
    let layout = ColorCheckerLayout {
        patch_size: 6,
        gap: 2,
    };
    let scene = color_checker_image(
        ColorCheckerEdition::AfterNov2014,
        RgbColorSpace::Bt2020,
        layout,
        [0.03; 3],
        1.0,
    )
    .unwrap();
    let camera = working_to_camera(&scene, &CAM_XYZ).unwrap();
    for cfa in CfaPattern::ALL {
        let params = BayerParams {
            cfa,
            // LibRaw 0.21 は DNG の黒レベルのパターンを整数に切り捨てて読むため、整数にする
            // （genzo-raw の dng モジュールの制限）。
            black_level: [510.0, 512.0, 514.0, 516.0],
            white_level: 16383.0,
            wb: [2.25, 1.0, 1.625, 1.0],
            cam_xyz: Some(CAM_XYZ),
            ..BayerParams::default()
        };
        let raw = mosaic(&camera, &params).unwrap();
        let dng = encode_dng(&raw).unwrap();
        let decoded = decode_bytes(&dng).unwrap().into_verified_image().unwrap();
        assert_eq!((decoded.width, decoded.height), (raw.width, raw.height));
        assert_eq!(decoded.cfa, cfa, "{cfa}");
        assert_eq!(decoded.data, raw.data, "{cfa}");
        assert_eq!(decoded.black_level, raw.black_level, "{cfa}");
        assert_eq!(decoded.white_level, raw.white_level, "{cfa}");
        // DNG の AsShotNeutral は R・G・B の 3 つ（G2 は G と同じになる）。
        for k in 0..3 {
            assert!(
                (decoded.as_shot_wb[k] - raw.as_shot_wb[k]).abs() < 1e-3,
                "{cfa}: {:?}",
                decoded.as_shot_wb
            );
        }
        // B0 に正規化した結果も一致する。
        let a = normalize_cfa(&raw).unwrap();
        let b = normalize_cfa(&decoded).unwrap();
        assert_eq!(diff_8bit(&a, &b, 0.0).unwrap().max, 0.0);
    }
}
