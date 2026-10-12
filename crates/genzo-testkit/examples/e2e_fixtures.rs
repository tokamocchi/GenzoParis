//! アプリの動作確認（E2E）用の写真のフォルダを作る。
//!
//! `cargo run -p genzo-testkit --example e2e_fixtures -- <フォルダ>`
//!
//! - `チャート 01.dng`: ColorChecker の合成 DNG（ベイヤー配列。撮影時の WB は genzo-testkit の既定値。
//!   カメラ行列は genzo-api のテストと同じ仮の値。埋め込みのプレビューはない）。
//!   LibRaw を有効にしたワーカーで RAW として開ける。WB のスポイト（DEV-03）を灰色のパッチで試せる
//!   （灰色のパッチは撮影時の WB で中立）。
//! - `チャート 02.dng`: 同じチャートを 2 段暗く撮ったもの（露光量の補正を試す）。
//! - `グラデーション 01.jpg`〜`03.jpg`、`色の帯.png`: RAW 以外の入力。
//!
//! 写真に人物・位置情報は含まない（合成）。既にあるファイルは上書きしない。

use std::path::{Path, PathBuf};

use genzo_color::RgbColorSpace;
use genzo_testkit::bayer::{BayerParams, mosaic, working_to_camera, write_dng};
use genzo_testkit::colorchecker::{ColorCheckerEdition, ColorCheckerLayout, color_checker_image};

fn write_chart(path: &Path, exposure: f32) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        return Ok(());
    }
    let layout = ColorCheckerLayout {
        patch_size: 320,
        gap: 40,
    };
    // 作業色空間（BT.2020）のチャートを、カメラ行列でカメラ RGB に戻してから並べる（ステージ 8 で元に
    // 戻る）。カメラ行列がないと、撮影時の WB を色温度・色かぶり補正で表せない。
    const CAM_XYZ: [[f32; 3]; 3] = [
        [0.7424, -0.2329, -0.0466],
        [-0.4598, 1.2471, 0.2347],
        [-0.0715, 0.1505, 0.6066],
    ];
    let scene = color_checker_image(
        ColorCheckerEdition::AfterNov2014,
        RgbColorSpace::Bt2020,
        layout,
        [0.03, 0.03, 0.03],
        exposure,
    )?;
    let camera = working_to_camera(&scene, &CAM_XYZ)?;
    let params = BayerParams {
        cam_xyz: Some(CAM_XYZ),
        ..BayerParams::default()
    };
    let raw = mosaic(&camera, &params)?;
    write_dng(path, &raw)?;
    Ok(())
}

fn write_rgb8(
    path: &Path,
    w: u32,
    h: u32,
    f: impl Fn(u32, u32) -> [u8; 3],
) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        return Ok(());
    }
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb(f(x, y)));
    img.save(path)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir: PathBuf = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("出力先のフォルダを指定してください")?;
    std::fs::create_dir_all(&dir)?;
    write_chart(&dir.join("チャート 01.dng"), 0.9)?;
    write_chart(&dir.join("チャート 02.dng"), 0.225)?;
    for (i, (r, g, b)) in [(255u8, 120u8, 40u8), (40, 140, 255), (90, 200, 90)]
        .iter()
        .enumerate()
    {
        write_rgb8(
            &dir.join(format!("グラデーション {:02}.jpg", i + 1)),
            1600,
            1067,
            |x, y| {
                let t = x as f32 / 1600.0;
                let s = y as f32 / 1067.0;
                [
                    (f32::from(*r) * t + 20.0 * s) as u8,
                    (f32::from(*g) * (1.0 - s) + 30.0 * t) as u8,
                    (f32::from(*b) * (0.5 + 0.5 * t * s)) as u8,
                ]
            },
        )?;
    }
    write_rgb8(&dir.join("色の帯.png"), 1200, 800, |x, _| {
        let band = x * 6 / 1200;
        match band {
            0 => [230, 30, 30],
            1 => [230, 200, 30],
            2 => [30, 200, 60],
            3 => [30, 200, 220],
            4 => [40, 60, 230],
            _ => [200, 40, 200],
        }
    })?;
    println!("{}", dir.display());
    Ok(())
}
