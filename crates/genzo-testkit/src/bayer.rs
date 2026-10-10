//! 合成のベイヤー配列の RAW と、合成 DNG。
//!
//! パイプラインのステージ 2・3（04 の 2.1 節）の逆をたどって、リニアのカメラ RGB の画像から
//! [`RawImage`] を作る。色の契約（2.6 節）の定義は次のとおり。
//!
//! - B1（カメラ RGB）: WB の係数で被写体の中立色が (1, 1, 1) になるよう正規化した値。
//! - B0（CFA）: 黒レベルを 0、白レベルを 1 とした値。B1 = B0 × WB の係数。
//!
//! [`mosaic`] は B1 の画像に **逆 WB**（係数で割る）をかけて B0 にし、CFA の配列に従って画素ごとに
//! 1 色を取り出し（モザイク化）、黒レベルと白レベルで 16bit の値に量子化する:
//!
//! `DN = round(黒レベル + B0 × (白レベル − 黒レベル))`、ただし 0〜⌊白レベル⌋ に切り詰める
//! （白レベルを超える値はセンサーの飽和として白レベルになる）。
//!
//! 作業色空間（リニア BT.2020）の画像から作るときは、先に [`working_to_camera`]（ステージ 8 の逆）で
//! カメラ RGB にする。

use std::path::Path;

use genzo_color::Mat3;
use genzo_color::camera::working_to_camera_normalized;
use genzo_model::PhotoMetadata;
use genzo_raw::dng::{self, DngError, DngOptions};
use genzo_raw::{CfaPattern, RawError, RawImage};

use crate::image::{FloatImage, ImageError};

/// 黒レベルの既定値。**仮置き**: 14bit のセンサーでよく見られる値（実機の値は PoC-2 で確認する）。
pub const DEFAULT_BLACK_LEVEL: f32 = 512.0;
/// 白レベルの既定値。**仮置き**: 14bit の最大値（2^14 − 1）。
pub const DEFAULT_WHITE_LEVEL: f32 = 16383.0;
/// WB の係数の既定値（R, G, B, G2。G を 1 とする）。**仮置き**: 昼光で R・B の感度が G より
/// 低いセンサーを想定した、典型的な大きさの値（特定の機種の値ではない）。
pub const DEFAULT_WB: [f32; 4] = [2.0, 1.0, 1.5, 1.0];
/// 合成の RAW のメーカー名（撮影情報）。
pub const SYNTHETIC_MAKE: &str = "GenzoParis";
/// 合成の RAW の機種名（撮影情報）。
pub const SYNTHETIC_MODEL: &str = "Synthetic Bayer";

/// 合成の RAW の作成のエラー。
#[derive(Debug, thiserror::Error)]
pub enum BayerError {
    /// 入力の画像が 3 チャンネルではない。
    #[error("入力の画像は 3 チャンネル（RGB）が必要です（{0} チャンネル）")]
    NotRgb(u32),
    /// 入力の画像に NaN・無限大がある。
    #[error("入力の画像に有限でない値があります: {0}")]
    NonFinite(crate::image::NonFiniteSample),
    /// 設定（黒レベル・白レベル・WB・行列）が不正。
    #[error("合成の RAW の設定が不正です: {0}")]
    InvalidParams(#[from] RawError),
    /// カメラ行列から変換の行列を作れない。
    #[error("カメラ行列から変換の行列を作れません: {0}")]
    Color(#[from] genzo_color::ColorError),
    /// 画像の寸法が不正。
    #[error(transparent)]
    Image(#[from] ImageError),
}

/// 合成の RAW の設定。
#[derive(Debug, Clone, PartialEq)]
pub struct BayerParams {
    /// CFA の配列。
    pub cfa: CfaPattern,
    /// 黒レベル（CFA の位置ごと。R, G1, B, G2 の順。[`RawImage::black_level`] と同じ）。
    pub black_level: [f32; 4],
    /// 白レベル。
    pub white_level: f32,
    /// WB の係数（R, G, B, G2。G = 1。[`RawImage::as_shot_wb`] になる）。
    pub wb: [f32; 4],
    /// XYZ（D65）→ カメラ RGB の行列（[`RawImage::cam_xyz`]）。
    pub cam_xyz: Option<[[f32; 3]; 3]>,
    /// 撮影情報。幅と高さは [`mosaic`] が画像の寸法で上書きする。
    pub metadata: PhotoMetadata,
}

impl Default for BayerParams {
    fn default() -> Self {
        Self {
            cfa: CfaPattern::RGGB,
            black_level: [DEFAULT_BLACK_LEVEL; 4],
            white_level: DEFAULT_WHITE_LEVEL,
            wb: DEFAULT_WB,
            cam_xyz: None,
            metadata: synthetic_metadata(),
        }
    }
}

/// 合成の RAW の撮影情報（メーカー・機種だけ。GPS などの個人情報は入れない）。
pub fn synthetic_metadata() -> PhotoMetadata {
    PhotoMetadata {
        make: Some(SYNTHETIC_MAKE.to_owned()),
        model: Some(SYNTHETIC_MODEL.to_owned()),
        ..Default::default()
    }
}

/// 量子化で切り詰めた値の数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClipCounts {
    /// 白レベルを超えて白レベルに切り詰めた画素の数（飽和）。
    pub above_white: u64,
    /// 0 未満で 0 に切り詰めた画素の数（負の値の入力）。
    pub below_zero: u64,
}

/// カメラ RGB（B1）の画像からベイヤー配列の RAW を作る。
pub fn mosaic(camera_rgb: &FloatImage, params: &BayerParams) -> Result<RawImage, BayerError> {
    mosaic_with_clip_counts(camera_rgb, params).map(|(raw, _)| raw)
}

/// [`mosaic`] と同じ。切り詰めた値の数も返す。
pub fn mosaic_with_clip_counts(
    camera_rgb: &FloatImage,
    params: &BayerParams,
) -> Result<(RawImage, ClipCounts), BayerError> {
    if camera_rgb.channels() != 3 {
        return Err(BayerError::NotRgb(camera_rgb.channels()));
    }
    if let Some(s) = camera_rgb.find_non_finite() {
        return Err(BayerError::NonFinite(s));
    }
    let (width, height) = (camera_rgb.width(), camera_rgb.height());
    let mut metadata = params.metadata.clone();
    metadata.width = Some(width);
    metadata.height = Some(height);
    let mut raw = RawImage {
        width,
        height,
        cfa: params.cfa,
        data: Vec::new(),
        black_level: params.black_level,
        white_level: params.white_level,
        as_shot_wb: params.wb,
        cam_xyz: params.cam_xyz,
        metadata,
    };
    // 寸法以外（黒レベル・白レベル・WB・行列）を先に検証する（データは後で入れる）。
    raw.data = vec![0; camera_rgb.pixel_count() as usize];
    raw.validate()?;

    let white = f64::from(params.white_level);
    let max_dn = white.floor();
    let mut clips = ClipCounts::default();
    for y in 0..height {
        for x in 0..width {
            let color = params.cfa.color_at(x as usize, y as usize);
            let idx = color.index();
            let b1 = f64::from(camera_rgb.pixel(x, y).expect("範囲内")[color.rgb_channel()]);
            let b0 = b1 / f64::from(params.wb[idx]);
            let black = f64::from(params.black_level[idx]);
            let dn = (black + b0 * (white - black)).round();
            let dn = if dn > max_dn {
                clips.above_white += 1;
                max_dn
            } else if dn < 0.0 {
                clips.below_zero += 1;
                0.0
            } else {
                dn
            };
            // 0〜⌊白レベル⌋（≤ 65535。validate 済み）に収めたので u16 に収まる。
            raw.data[y as usize * width as usize + x as usize] = dn as u16;
        }
    }
    Ok((raw, clips))
}

/// RAW を B0（黒レベル 0、白レベル 1）の 1 チャンネルの画像にする（ステージ 2 の参照実装。
/// テストの期待値用）。
pub fn normalize_cfa(raw: &RawImage) -> Result<FloatImage, BayerError> {
    raw.validate()?;
    let white = f64::from(raw.white_level);
    Ok(FloatImage::from_gray_fn(raw.width, raw.height, |x, y| {
        let black = f64::from(raw.black_level_at(x, y));
        let dn = f64::from(raw.get(x, y).expect("範囲内"));
        ((dn - black) / (white - black)) as f32
    })?)
}

/// 作業色空間（リニア BT.2020、B2）の画像をカメラ RGB（B1）にする（ステージ 8 の逆）。
///
/// genzo-color の [`working_to_camera_normalized`]（BT.2020 → カメラ RGB を中立色が (1, 1, 1) に
/// なるよう行ごとに正規化した行列）をかける。ステージ 8 の行列
/// （[`camera_to_working_matrix`](genzo_color::camera_to_working_matrix)）の
/// 逆行列と同じなので、ステージ 8 を通すと元の画像に戻る（f32 の誤差を除く）。
/// ステージ 8 で使えない行列（特異なもの）は、[`working_to_camera_normalized`] がエラーにする。
pub fn working_to_camera(
    working: &FloatImage,
    cam_xyz: &[[f32; 3]; 3],
) -> Result<FloatImage, BayerError> {
    let m = Mat3::from_rows(cam_xyz.map(|r| r.map(f64::from)));
    let to_cam = working_to_camera_normalized(&m)?.to_f32();
    working
        .map_rgb(|p| to_cam.apply(p))
        .ok_or(BayerError::NotRgb(working.channels()))
}

/// 合成 DNG のバイト列にする（genzo-raw の [`dng::encode`]。既定の設定）。
///
/// LibRaw で読む場合、幅と高さは 22 画素以上にする（genzo-raw の dng モジュールの制限）。
pub fn encode_dng(raw: &RawImage) -> Result<Vec<u8>, DngError> {
    dng::encode(raw, &DngOptions::default())
}

/// 合成 DNG をファイルに書く（既存のファイルは上書きしない。genzo-raw の [`dng::write_file`]）。
pub fn write_dng(path: impl AsRef<Path>, raw: &RawImage) -> Result<(), DngError> {
    dng::write_file(path, raw, &DngOptions::default())
}

#[cfg(test)]
mod tests {
    use genzo_color::camera_to_working_matrix;
    use genzo_raw::CfaColor;

    use super::*;
    use crate::synth::zone_plate;

    /// テスト用の「典型的な形の」カメラ行列（genzo-raw のテストと同じ値。特定の機種の値ではない）。
    const CAM_XYZ: [[f32; 3]; 3] = [
        [0.7424, -0.2329, -0.0466],
        [-0.4598, 1.2471, 0.2347],
        [-0.0715, 0.1505, 0.6066],
    ];

    fn params(cfa: CfaPattern) -> BayerParams {
        BayerParams {
            cfa,
            black_level: [500.0, 510.0, 520.0, 530.0],
            white_level: 16000.0,
            wb: [2.0, 1.0, 1.6, 1.25],
            cam_xyz: Some(CAM_XYZ),
            metadata: synthetic_metadata(),
        }
    }

    #[test]
    fn mosaic_follows_cfa_black_and_wb_for_all_patterns() {
        let rgb = [0.4_f32, 0.3, 0.2];
        let img = FloatImage::filled(6, 4, &rgb).unwrap();
        for cfa in CfaPattern::ALL {
            let p = params(cfa);
            let raw = mosaic(&img, &p).unwrap();
            assert_eq!(raw.cfa, cfa);
            assert_eq!((raw.width, raw.height), (6, 4));
            assert_eq!(raw.metadata.width, Some(6));
            assert_eq!(raw.as_shot_wb, p.wb);
            raw.validate().unwrap();
            for y in 0..4 {
                for x in 0..6 {
                    let color = cfa.color_at(x as usize, y as usize);
                    let i = color.index();
                    let b0 = f64::from(rgb[color.rgb_channel()]) / f64::from(p.wb[i]);
                    let black = f64::from(p.black_level[i]);
                    let expected = (black + b0 * (16000.0 - black)).round() as u16;
                    assert_eq!(raw.get(x, y), Some(expected), "{cfa} ({x}, {y}) {color:?}");
                }
            }
            // G1 と G2 は黒レベル・WB が違うので、別の値になる。
            let find = |c: CfaColor| {
                (0..2)
                    .flat_map(|y| (0..2).map(move |x| (x, y)))
                    .find(|&(x, y)| cfa.color_at(x, y) == c)
                    .map(|(x, y)| raw.get(x as u32, y as u32).unwrap())
                    .unwrap()
            };
            assert_ne!(find(CfaColor::Green1), find(CfaColor::Green2));
        }
    }

    #[test]
    fn rggb_layout_explicit() {
        // RGGB: (0,0) R、(1,0) G1、(0,1) G2、(1,1) B。WB 1、黒 0、白 1000 で値を直接確かめる。
        let img = FloatImage::filled(2, 2, &[0.1, 0.5, 0.9]).unwrap();
        let p = BayerParams {
            black_level: [0.0; 4],
            white_level: 1000.0,
            wb: [1.0; 4],
            ..BayerParams::default()
        };
        let raw = mosaic(&img, &p).unwrap();
        assert_eq!(raw.data, vec![100, 500, 500, 900]);
        let p = BayerParams {
            cfa: CfaPattern::BGGR,
            ..p
        };
        let raw = mosaic(&img, &p).unwrap();
        assert_eq!(raw.data, vec![900, 500, 500, 100]);
    }

    #[test]
    fn normalize_and_reapply_wb_recovers_input() {
        let img = zone_plate(32, 24, 0.4, 0.05, 0.9).unwrap();
        let p = params(CfaPattern::GRBG);
        let raw = mosaic(&img, &p).unwrap();
        let b0 = normalize_cfa(&raw).unwrap();
        for y in 0..24 {
            for x in 0..32 {
                let color = p.cfa.color_at(x as usize, y as usize);
                let b1 = b0.pixel(x, y).unwrap()[0] * p.wb[color.index()];
                let input = img.pixel(x, y).unwrap()[color.rgb_channel()];
                // 量子化の誤差は B0 で 0.5 / (白 − 黒) 以下、B1 ではその WB 倍。
                let tol = 0.5 / (16000.0 - 530.0) * 2.0 + 1e-6;
                assert!((b1 - input).abs() <= tol, "({x}, {y}): {b1} {input}");
            }
        }
    }

    #[test]
    fn saturation_and_negative_values_are_clipped() {
        // R の WB 2.0: B1 = 2.0 でちょうど白レベル、それを超えると飽和。
        let img = FloatImage::from_rgb_fn(2, 2, |x, _| {
            if x == 0 {
                [5.0, 5.0, 5.0]
            } else {
                [-1.0, -1.0, -1.0]
            }
        })
        .unwrap();
        let p = params(CfaPattern::RGGB);
        let (raw, clips) = mosaic_with_clip_counts(&img, &p).unwrap();
        assert_eq!(raw.get(0, 0), Some(16000));
        assert_eq!(raw.get(0, 1), Some(16000));
        // 負の値: 黒レベル − (白 − 黒) × 1/WB は 0 未満なので 0。
        assert_eq!(raw.get(1, 0), Some(0));
        assert_eq!(clips.above_white, 2);
        assert_eq!(clips.below_zero, 2);
        // 白レベルちょうどは切り詰めない（数えない）。
        let img = FloatImage::filled(1, 1, &[2.0, 1.0, 1.6]).unwrap();
        let (raw, clips) = mosaic_with_clip_counts(&img, &p).unwrap();
        assert_eq!(raw.get(0, 0), Some(16000));
        assert_eq!(clips, ClipCounts::default());
        // 黒レベルより小さい値（0 以上）はそのまま残る（センサーのノイズで起こる）。
        let img = FloatImage::filled(1, 1, &[-0.01, 0.0, 0.0]).unwrap();
        let raw = mosaic(&img, &p).unwrap();
        let expected = (500.0_f64 - 0.005 * 15500.0).round() as u16;
        assert_eq!(raw.get(0, 0), Some(expected));
    }

    #[test]
    fn invalid_params_and_inputs() {
        let img = FloatImage::filled(4, 4, &[0.5; 3]).unwrap();
        let mut p = params(CfaPattern::RGGB);
        p.white_level = 70000.0;
        assert!(matches!(
            mosaic(&img, &p),
            Err(BayerError::InvalidParams(RawError::InvalidWhiteLevel(_)))
        ));
        let mut p = params(CfaPattern::RGGB);
        p.black_level[2] = 16000.0;
        assert!(matches!(
            mosaic(&img, &p),
            Err(BayerError::InvalidParams(RawError::InvalidBlackLevel {
                index: 2,
                ..
            }))
        ));
        let mut p = params(CfaPattern::RGGB);
        p.wb = [2.0, 1.5, 1.0, 1.0];
        assert!(matches!(
            mosaic(&img, &p),
            Err(BayerError::InvalidParams(RawError::InvalidWhiteBalance(_)))
        ));
        let gray = FloatImage::filled(4, 4, &[0.5]).unwrap();
        assert!(matches!(
            mosaic(&gray, &params(CfaPattern::RGGB)),
            Err(BayerError::NotRgb(1))
        ));
        let mut nan = img.clone();
        nan.data_mut()[7] = f32::NAN;
        assert!(matches!(
            mosaic(&nan, &params(CfaPattern::RGGB)),
            Err(BayerError::NonFinite(_))
        ));
    }

    #[test]
    fn working_to_camera_inverts_stage_8() {
        let working = FloatImage::from_rgb_fn(4, 3, |x, y| {
            [0.1 + x as f32 * 0.2, 0.3 + y as f32 * 0.1, 0.25]
        })
        .unwrap();
        let cam = working_to_camera(&working, &CAM_XYZ).unwrap();
        let m = Mat3::from_rows(CAM_XYZ.map(|r| r.map(f64::from)));
        let stage8 = camera_to_working_matrix(&m).unwrap().to_f32();
        let back = cam.map_rgb(|p| stage8.apply(p)).unwrap();
        for (a, b) in working.data().iter().zip(back.data()) {
            assert!((a - b).abs() < 1e-5, "{a} {b}");
        }
        // 中立色は (1, 1, 1) のまま（行ごとの正規化）。
        let white = FloatImage::filled(1, 1, &[1.0; 3]).unwrap();
        let cw = working_to_camera(&white, &CAM_XYZ).unwrap();
        for v in cw.data() {
            assert!((v - 1.0).abs() < 1e-5);
        }
        // 特異な行列・中立色に反応しない行のある行列はエラー。
        let singular = [[1.0, 1.0, 1.0]; 3];
        assert!(matches!(
            working_to_camera(&white, &singular),
            Err(BayerError::Color(_))
        ));
        let mut dead_row = CAM_XYZ;
        dead_row[1] = [0.0; 3];
        assert!(matches!(
            working_to_camera(&white, &dead_row),
            Err(BayerError::Color(_))
        ));
        let gray = FloatImage::filled(1, 1, &[0.5]).unwrap();
        assert!(matches!(
            working_to_camera(&gray, &CAM_XYZ),
            Err(BayerError::NotRgb(1))
        ));
    }

    #[test]
    fn synthetic_dng_bytes() {
        let img = FloatImage::filled(24, 22, &[0.3, 0.4, 0.5]).unwrap();
        let raw = mosaic(&img, &params(CfaPattern::RGGB)).unwrap();
        let bytes = encode_dng(&raw).unwrap();
        // リトルエンディアンの TIFF。
        assert_eq!(&bytes[..4], &[b'I', b'I', 42, 0]);
        // 非圧縮の 16bit の画素がそのまま入っている（データの長さ以上）。
        assert!(bytes.len() > 24 * 22 * 2);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.dng");
        write_dng(&path, &raw).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        // 既存のファイルは上書きしない。
        assert!(write_dng(&path, &raw).is_err());
    }
}
