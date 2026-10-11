//! GPU 版のテストの共通の部品（GPU の取得・合成の入力・設定）。
//!
//! - GPU（アダプター）がない環境では、テストはスキップする（失敗にしない）。環境変数
//!   `GENZO_REQUIRE_GPU=1` のときは、GPU がなければ失敗にする（GPU のある開発者の PC・リリース前の確認で
//!   使う。04 の 2.3 節）。
//! - **CI のランナー（環境変数 `CI`。GitHub Actions は `CI=true`）では、CPU 版と GPU 版の比較をしない**
//!   （04 の 2.3 節「GPU のない CI では CPU 版の回帰テストだけを行い、GPU 版の比較は開発者の PC（RTX 3080 と
//!   M1）で定期的に実行する」）。CI の Windows のランナーには GPU がないが、wgpu は DX12 のソフトウェアの
//!   実装（WARP）を最後の候補として選び、macOS のランナーでも仮想の GPU（Metal）が見えうるので、アダプターの有無
//!   だけでは比較を避けられない（実機と違う実装の差で CI が失敗しうる）。`GENZO_REQUIRE_GPU=1` なら CI でも
//!   比較する。シェーダーのコンパイル（Metal・FXC / DXC）だけを確かめるテストは、CI でもアダプターがあれば
//!   実行する（[`adapter`]）。
//! - 合成の画像は整数の演算と定数の表だけで作る（OS の数学ライブラリの差で入力が変わらないように）。

#![allow(dead_code)]

use std::sync::{Arc, OnceLock};

use genzo_gpu::GpuRenderer;
use genzo_model::{CropRect, CurvePoint, DevelopSettings, FileId, Orientation, WhiteBalance};
use genzo_pipeline::{
    ColorContract, Engine, EngineConfig, PhotoSource, RgbImage, SensorOptions, SourceId,
    SourceImage,
};
use genzo_raw::{CfaPattern, RawImage};
use genzo_testkit::bayer::{BayerParams, mosaic};
use genzo_testkit::image::FloatImage;

/// GPU を必須にするか（`GENZO_REQUIRE_GPU=1`）。
pub fn gpu_required() -> bool {
    std::env::var("GENZO_REQUIRE_GPU").is_ok_and(|v| v.trim() == "1")
}

/// CI のランナーか（環境変数 `CI` が空でなく、`0`・`false` でない）。
pub fn on_ci() -> bool {
    skips_comparisons(std::env::var("CI").ok().as_deref(), false)
}

/// CPU 版と GPU 版の比較を CI だからとして飛ばすか（`ci` は環境変数 `CI` の値、`required` は
/// `GENZO_REQUIRE_GPU=1` か）。
pub fn skips_comparisons(ci: Option<&str>, required: bool) -> bool {
    let on_ci = ci.is_some_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        !v.is_empty() && v != "0" && v != "false"
    });
    on_ci && !required
}

/// CPU 版と GPU 版を比べるテストで共有する GPU。アダプターがない、または CI のランナー（モジュールの
/// doc）なら `None`（スキップ）。
pub fn gpu() -> Option<&'static GpuRenderer> {
    static SKIP_NOTICE: OnceLock<()> = OnceLock::new();
    if skips_comparisons(std::env::var("CI").ok().as_deref(), gpu_required()) {
        SKIP_NOTICE.get_or_init(|| {
            eprintln!(
                "CI のランナーなので、GPU 版の比較のテストをスキップする（04 の 2.3 節。GENZO_REQUIRE_GPU=1 なら実行する）"
            );
        });
        return None;
    }
    adapter()
}

/// 新しい GPU（テストでは、ソフトウェアの実装（llvmpipe など）も使う。環境変数
/// `GENZO_GPU_ALLOW_SOFTWARE=0` なら使わない。決定 No.20）。
pub fn renderer() -> genzo_gpu::Result<Option<GpuRenderer>> {
    GpuRenderer::from_options(&genzo_gpu::GpuContextOptions::from_env_allowing_software())
}

/// アダプターがあれば GPU（CI のランナーでも返す。シェーダーのコンパイルだけを確かめるテスト用）。
/// なければ `None`（スキップ）。
pub fn adapter() -> Option<&'static GpuRenderer> {
    static GPU: OnceLock<Option<GpuRenderer>> = OnceLock::new();
    GPU.get_or_init(|| {
        let r = renderer();
        match r {
            Ok(Some(g)) => {
                eprintln!("GPU: {}", g.context().summary());
                Some(g)
            }
            Ok(None) => {
                assert!(
                    !gpu_required(),
                    "GENZO_REQUIRE_GPU=1 だが GPU のアダプターがない"
                );
                eprintln!("GPU のアダプターがないので、GPU 版のテストをスキップする");
                None
            }
            Err(e) => {
                assert!(
                    !gpu_required(),
                    "GENZO_REQUIRE_GPU=1 だが GPU を初期化できない: {e}"
                );
                eprintln!("GPU を初期化できないので、GPU 版のテストをスキップする: {e}");
                None
            }
        }
    })
    .as_ref()
}

/// GPU を取り出す（なければテストを終える）。
#[macro_export]
macro_rules! gpu_or_skip {
    () => {
        match common::gpu() {
            Some(g) => g,
            None => return,
        }
    };
}

/// テスト用の典型的な形のカメラ行列（XYZ → カメラ RGB。実在の機種の値ではない。genzo-pipeline の
/// テストと同じ）。
pub fn typical_camera() -> [[f32; 3]; 3] {
    [
        [0.70, -0.20, -0.06],
        [-0.50, 1.30, 0.23],
        [-0.07, 0.13, 0.65],
    ]
}

/// 色のパッチの表（シーンリニアの値。負の値（色域外）と 1 を超える値を含む）。
const PATCHES: [[f32; 3]; 14] = [
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
    [2.50, 1.80, 0.90],
    [-0.04, 0.35, 0.60],
    [6.00, 0.40, 0.10],
];

/// いろいろな値を含む作業色空間（B2）の合成の画像。
///
/// 左から: 対数の段階のグレー（2^−14〜2^4）/ 色のパッチ / 細い線と市松 / 滑らかな傾き。
pub fn scene_b2(w: u32, h: u32) -> RgbImage {
    RgbImage::from_fn(w, h, ColorContract::B2Working, |x, y| {
        let zone = (x * 4 / w).min(3);
        match zone {
            0 => {
                // 2^(k − 14)（k = 0..18）。整数の比で作る。
                let k = (y * 19 / h).min(18) as i32;
                let v = if k >= 14 {
                    (1u32 << (k - 14)) as f32
                } else {
                    1.0 / (1u32 << (14 - k)) as f32
                };
                [v; 3]
            }
            1 => PATCHES[((x / 5 + y / 5 * 3) % 14) as usize],
            2 => {
                if (x + y) % 3 == 0 {
                    [0.9, 0.85, 0.8]
                } else if (x / 2 + y / 2) % 2 == 0 {
                    [0.02, 0.025, 0.03]
                } else {
                    [0.2, 0.1, 0.05]
                }
            }
            _ => {
                let t = (x % 64) as f32 / 64.0;
                let s = (y % 48) as f32 / 48.0;
                [0.01 + 1.2 * t, 0.02 + 0.8 * s, 0.05 + 0.5 * t * s]
            }
        }
    })
    .unwrap()
}

/// 表示参照（B3）の合成の画像（輝度 0〜1、負の値を含む）。
pub fn scene_b3(w: u32, h: u32) -> RgbImage {
    let b2 = scene_b2(w, h);
    let data = b2
        .data()
        .iter()
        .map(|p| {
            let m = p[0].max(p[1]).max(p[2]).max(1.0);
            [p[0] / m, p[1] / m, p[2] / m]
        })
        .collect();
    RgbImage::from_vec(w, h, ColorContract::B3DisplayReferred, data).unwrap()
}

/// カメラ RGB のシーン（B1。合成の RAW の元）。
pub fn camera_scene(w: u32, h: u32) -> FloatImage {
    FloatImage::from_rgb_fn(w, h, |x, y| {
        let zone = (x * 3 / w).min(2);
        match zone {
            0 => {
                let k = (y * 8 / h).min(7) as usize;
                [[0.004, 0.01, 0.025, 0.06, 0.12, 0.25, 0.5, 0.9][k]; 3]
            }
            1 => {
                let p = PATCHES[((x / 7 + y / 7 * 3) % 11) as usize];
                [p[0].min(0.95), p[1].min(0.95), p[2].min(0.95)]
            }
            _ => {
                if x % 5 == 0 || y % 7 == 3 {
                    [0.85, 0.8, 0.75]
                } else {
                    [0.02, 0.02, 0.025]
                }
            }
        }
    })
    .unwrap()
}

/// 合成の RAW（RGGB など。14bit・典型的なカメラ行列）。`saturate` なら一部の画素を白レベルにする。
pub fn raw_from(scene: &FloatImage, cfa: CfaPattern, saturate: bool) -> RawImage {
    let mut raw = mosaic(
        scene,
        &BayerParams {
            cfa,
            cam_xyz: Some(typical_camera()),
            ..Default::default()
        },
    )
    .unwrap();
    if saturate {
        let w = raw.width as usize;
        let white = raw.white_level as u16;
        for (i, v) in raw.data.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            if (x / 9 + y / 11) % 7 == 0 && x % 3 != 0 {
                *v = white;
            }
        }
    }
    raw
}

/// 実機の ARW の形を真似た合成の RAW（A-2・A-3）: 飽和の閾値が白レベルより低く、閾値と白レベルの
/// 間の値に張り付いた画素（Sony の圧縮 ARW の緑の 16372 と同じ形）があり、有効画素の端の外側に
/// 既定の切り抜きの外の帯（詰め物の値）がある。向きは 8（反時計回りに 90 度。α7 IV の縦位置）。
pub fn raw_with_crop_and_plateau(scene: &FloatImage, cfa: CfaPattern) -> RawImage {
    let mut raw = raw_from(scene, cfa, false);
    raw.saturation_level = genzo_raw::saturation_level_for(raw.black_level, raw.white_level);
    let plateau = (raw.white_level - 8.0) as u16;
    assert!(f32::from(plateau) >= raw.saturation_level);
    let (w, h) = (raw.width, raw.height);
    // 既定の切り抜き: 左 3・上 2・右 6・下 4 の外側（原点は奇数）。
    let crop = genzo_raw::PixelRect::new(3, 2, w - 9, h - 6);
    for (i, v) in raw.data.iter_mut().enumerate() {
        let (x, y) = ((i as u32) % w, (i as u32) / w);
        if (x / 7 + y / 5) % 6 == 0 && x % 2 == 0 {
            *v = plateau;
        }
        // 既定の切り抜きの外の右端の列は、実機の詰め物のように極端な値にする（使われないこと）。
        if x >= w - 2 {
            *v = raw.white_level as u16;
        }
    }
    raw.default_crop = Some(crop);
    raw.metadata.orientation = Orientation::Rotate270Cw;
    raw.metadata.width = Some(crop.width);
    raw.metadata.height = Some(crop.height);
    raw.validate().unwrap();
    raw
}

/// RAW の入力（元ファイルの ID を指定する。GPU 版のキャッシュは同じプロセスのテストで共有するので、
/// 寸法などが違う RAW には別の ID を使う）。
pub fn raw_source_with_id(raw: RawImage, file_id: i64) -> PhotoSource {
    PhotoSource::new(
        SourceId::new(FileId::new(file_id), 1),
        SourceImage::Raw(Arc::new(raw)),
    )
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

/// RAW 以外（B2）の入力。
pub fn working_source(image: RgbImage, orientation: Orientation) -> PhotoSource {
    PhotoSource::new(
        SourceId::new(FileId::new(2), 1),
        SourceImage::Working {
            image: Arc::new(image),
            orientation,
        },
    )
    .unwrap()
}

/// エンジン。
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
    s.tone_curve.parametric.shadows = 20.0;
    s.tone_curve.red = vec![
        CurvePoint::new(0.0, 0.05),
        CurvePoint::new(0.5, 0.55),
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
