//! エンジンの結合テストの共通の部品（合成の RAW・現像設定・比較）。
//!
//! 合成の画像は整数の演算と定数の表だけで作る（Windows / macOS の数学ライブラリの差で入力が
//! 変わらないようにするため）。

#![allow(dead_code)]

use std::sync::Arc;

use genzo_model::{CropRect, CurvePoint, DevelopSettings, FileId, Orientation, WhiteBalance};
use genzo_pipeline::{
    Engine, EngineConfig, PhotoSource, RgbImage, SensorOptions, SourceId, SourceImage,
};
use genzo_raw::{CfaPattern, RawImage};
use genzo_testkit::bayer::{BayerParams, mosaic};
use genzo_testkit::image::FloatImage;

/// テスト用の典型的な形のカメラ行列（XYZ → カメラ RGB。実在の機種の値ではない。
/// tests/sensor_pipeline.rs と同じ）。
pub fn typical_camera() -> [[f32; 3]; 3] {
    [
        [0.70, -0.20, -0.06],
        [-0.50, 1.30, 0.23],
        [-0.07, 0.13, 0.65],
    ]
}

/// 色のパッチの表（カメラ RGB。B1 の値。G の飽和（1）より小さい）。
const PATCHES: [[f32; 3]; 12] = [
    [0.60, 0.08, 0.05],
    [0.05, 0.50, 0.07],
    [0.04, 0.06, 0.55],
    [0.70, 0.62, 0.05],
    [0.05, 0.45, 0.50],
    [0.55, 0.06, 0.48],
    [0.30, 0.22, 0.16],
    [0.12, 0.18, 0.30],
    [0.85, 0.85, 0.85],
    [0.18, 0.18, 0.18],
    [0.03, 0.03, 0.03],
    [0.40, 0.30, 0.10],
];

/// グレーの段階（カメラ RGB。中立）。
const GRAYS: [f32; 8] = [0.004, 0.01, 0.025, 0.06, 0.12, 0.25, 0.5, 0.9];

/// いろいろな模様を含む合成のシーン（カメラ RGB。B1）。
///
/// 横に 4 つの区画: グレーの段階 / 1 画素の細い線（縦・横・斜め）/ 色のパッチ（くっきりした境界）/
/// 明暗差（暗い背景に明るい窓と、明るい背景に暗い細線）。
pub fn busy_scene(w: u32, h: u32) -> FloatImage {
    FloatImage::from_rgb_fn(w, h, |x, y| {
        let zone = (x * 4 / w).min(3);
        match zone {
            0 => [GRAYS[(y * 8 / h).min(7) as usize]; 3],
            1 => {
                let line = x % 7 == 0 || y % 9 == 0 || (x + y) % 13 == 0;
                if line {
                    [0.8, 0.75, 0.7]
                } else {
                    [0.03, 0.035, 0.04]
                }
            }
            2 => PATCHES[((x / 11 + y / 11 * 5) % 12) as usize],
            _ => {
                let (cx, cy) = (x % 40, y % 40);
                if (10..30).contains(&cx) && (10..30).contains(&cy) {
                    if (cx + cy) % 6 == 0 {
                        [0.02; 3]
                    } else {
                        [0.9, 0.88, 0.86]
                    }
                } else if (cx + 2 * cy) % 17 == 0 {
                    [0.7; 3]
                } else {
                    [0.006, 0.005, 0.004]
                }
            }
        }
    })
    .unwrap()
}

/// 細い線だけのシーン（IQ-07b の厳しい場合。1 画素の明るい線が暗い背景に並ぶ）。
pub fn thin_lines_scene(w: u32, h: u32) -> FloatImage {
    FloatImage::from_rgb_fn(w, h, |x, y| {
        if x % 5 == 0 || y % 8 == 3 {
            [0.85, 0.8, 0.75]
        } else {
            [0.02, 0.02, 0.025]
        }
    })
    .unwrap()
}

/// 色の境界だけのシーン（大きめの色のパッチ）。
pub fn color_edges_scene(w: u32, h: u32) -> FloatImage {
    FloatImage::from_rgb_fn(w, h, |x, y| PATCHES[((x / 23 + y / 19 * 3) % 12) as usize]).unwrap()
}

/// 明暗差の大きいシーン（逆光のような: 明るい空と暗い前景、境目のぎざぎざ）。
pub fn high_contrast_scene(w: u32, h: u32) -> FloatImage {
    FloatImage::from_rgb_fn(w, h, |x, y| {
        let horizon = h / 2 + (x % 23) * h / 160;
        if y < horizon {
            [0.8, 0.85, 0.9]
        } else if (x / 6 + y / 6) % 5 == 0 {
            [0.05, 0.04, 0.03]
        } else {
            [0.008, 0.007, 0.006]
        }
    })
    .unwrap()
}

/// 中立のグレーのパッチ（明るさの違う 6 つ）と、その間の色のパッチ。
pub fn gray_patches_scene(w: u32, h: u32) -> FloatImage {
    FloatImage::from_rgb_fn(w, h, |x, y| {
        let col = x * 6 / w;
        if y < h / 2 {
            [[0.02f32, 0.06, 0.12, 0.18, 0.35, 0.6][col as usize]; 3]
        } else {
            PATCHES[(col % 6) as usize]
        }
    })
    .unwrap()
}

/// カメラ RGB のシーンから合成の RAW（RGGB・14bit・撮影時の WB は testkit の既定・典型的なカメラ行列）。
pub fn raw_from(scene: &FloatImage) -> RawImage {
    raw_with(scene, CfaPattern::RGGB)
}

/// CFA の配列を指定して合成の RAW を作る。
pub fn raw_with(scene: &FloatImage, cfa: CfaPattern) -> RawImage {
    mosaic(
        scene,
        &BayerParams {
            cfa,
            cam_xyz: Some(typical_camera()),
            ..Default::default()
        },
    )
    .unwrap()
}

/// RAW の向き（EXIF の Orientation）を指定して合成の RAW を作る（RGGB）。
pub fn raw_oriented(scene: &FloatImage, orientation: Orientation) -> RawImage {
    let mut metadata = genzo_testkit::bayer::synthetic_metadata();
    metadata.orientation = orientation;
    mosaic(
        scene,
        &BayerParams {
            cfa: CfaPattern::RGGB,
            cam_xyz: Some(typical_camera()),
            metadata,
            ..Default::default()
        },
    )
    .unwrap()
}

/// 滑らかだが傾きの大きいシーン（三角波。周期 40〜56 px）。位置のずれ（位置合わせ）を調べる用。
/// 値は整数の演算と IEEE 754 の除算だけで作る（OS の数学ライブラリによらない）。
pub fn wavy_scene(w: u32, h: u32) -> FloatImage {
    let tri = |v: u32, p: u32| {
        let m = v % p;
        let k = if m < p / 2 { m } else { p - m };
        k as f32 / (p / 2) as f32
    };
    FloatImage::from_rgb_fn(w, h, |x, y| {
        [
            0.04 + 0.5 * tri(x, 48),
            0.04 + 0.4 * tri(y, 40),
            0.05 + 0.3 * tri(x + y, 56),
        ]
    })
    .unwrap()
}

/// RAW の入力。
pub fn raw_source(raw: RawImage) -> PhotoSource {
    PhotoSource::new(
        SourceId::new(FileId::new(1), 1),
        SourceImage::Raw(Arc::new(raw)),
    )
    .unwrap()
}

/// エンジン（プレビューの長辺とタイルの一辺を指定）。
pub fn engine(preview_long_edge: u32, tile_size: u32) -> Engine {
    Engine::new(EngineConfig {
        preview_long_edge,
        tile_size,
        sensor: SensorOptions::default(),
        ..Default::default()
    })
    .unwrap()
}

/// いろいろなステージが有効になる設定（露光量・コントラスト・トーン・色・カーブ）。
pub fn strong_settings() -> DevelopSettings {
    let mut s = DevelopSettings {
        exposure_ev: 0.5,
        contrast: 30.0,
        ..Default::default()
    };
    s.tone.highlights = -70.0;
    s.tone.shadows = 70.0;
    s.tone.whites = 20.0;
    s.tone.blacks = -20.0;
    s.color.saturation = 30.0;
    s.color.vibrance = 30.0;
    s.tone_curve.luma = vec![
        CurvePoint::new(0.0, 0.0),
        CurvePoint::new(0.25, 0.2),
        CurvePoint::new(0.75, 0.82),
        CurvePoint::new(1.0, 1.0),
    ];
    s
}

/// 回転と切り抜き。
pub fn rotate_and_crop(s: &mut DevelopSettings, deg: f32, crop: CropRect) {
    s.geometry.angle_deg = deg;
    s.geometry.crop = crop;
}

/// 色温度・tint の WB。
pub fn custom_wb(s: &mut DevelopSettings, temperature_k: f32, tint: f32) {
    s.white_balance = WhiteBalance::Custom {
        temperature_k,
        tint,
    };
}

/// RgbImage を testkit の画像にする。
pub fn to_float(img: &RgbImage) -> FloatImage {
    FloatImage::new(img.width(), img.height(), 3, img.as_flat().to_vec()).unwrap()
}

/// 2 枚の画像がビット単位で同じか確かめる。
pub fn assert_bitwise_eq(a: &RgbImage, b: &RgbImage, what: &str) {
    assert_eq!(a.dimensions(), b.dimensions(), "{what}");
    for (i, (p, q)) in a.data().iter().zip(b.data()).enumerate() {
        assert!(
            p.iter().zip(q).all(|(x, y)| x.to_bits() == y.to_bits()),
            "{what}: 画素 {i}: {p:?} ≠ {q:?}"
        );
    }
}
