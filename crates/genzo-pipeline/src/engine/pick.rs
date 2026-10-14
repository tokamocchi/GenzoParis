//! WB のスポイト（DEV-03。指定した点を中立にする WB を求める。[`Engine::white_balance_at`]）。
//!
//! # 方法
//!
//! 1. 段階 A1 のプレビュー（最終品質、長辺 [`crate::EngineConfig::preview_long_edge`]。プレビューの描画と
//!    同じもので、ふつうはエンジンのキャッシュにある）を今の設定で求める。A1 の値は
//!    B2 = ステージ 8 の行列 ×（WB の係数 ⊙ カメラ RGB）（[`crate::sensor`] の doc）。
//! 2. 指定した点（プレビューの出力の画像の幅・高さに対する割合）を、プレビューと同じ C の格子
//!    （[`GeometryPlan::from_settings`]。切り抜く前の全体の表示ならその格子）で C → G → O → S とたどり、
//!    A1 の画素の座標にする（ステージ 9 と同じ変換なので、切り抜き・回転・向きがあっても同じ被写体の点を
//!    指す）。点のある C の画素が、表示で塗る画素（切り抜く前の全体の表示で、回転で画像の外になった四隅。
//!    [`GeometryPlan::c_pixel_outside_image`]）ならエラー。そうでなければ、O の外にわずかにかかる点（端の
//!    画素の中心より外側。C の格子の丸め・切り抜きの制限で、出力の端が O の外にかかることがある）は、O の
//!    内側に収めてから進める（表示では、そこにも鏡映で埋めた画像が見えているため）。
//! 3. その点のまわりの正方形（S の軸にそろえる。半径は見える範囲（RAW の既定の切り抜き）の長辺の
//!    [`WB_PICK_RADIUS`]、A1 の 1 画素以上）にある A1 の画素のうち、中心が既定の切り抜きの内側にある
//!    ものを平均する。
//!    カメラプロファイル（DCP。DEV-17）を選んでいても、プロファイルを使わない A1（今のカメラ行列のステージ 8）を使う
//!    （色相彩度のマップは逆に戻せないため。プロファイルを選んだ写真で初めて使うときは、A1 を 1 回余分に作る）。
//! 4. 平均を、ステージ 8 の逆（genzo-color の `working_to_camera_normalized`）と今の WB の係数で割って、
//!    カメラ RGB（WB をかける前）に戻す。緑の係数は G1 の係数（1）を使う（G2 の係数は G1 と G2 の
//!    感度をそろえる値で、ステージ 3 の後の緑は G1 の尺度のため。[`crate::sensor::wb`] の doc）。
//! 5. その色を中立にする係数 `[G/R, 1, G/B]` を、ステージ 3 と同じカメラ行列（行列がなければ BT.2020
//!    とみなす行列）で色温度・tint に直す（genzo-color の `temperature_tint_from_wb_multipliers`）。
//!    この値を `WhiteBalance::Custom` に入れると、ステージ 3 はこの係数に戻す。
//!
//! # 決めごと
//!
//! - **範囲の外**: 推定した値が設定できる範囲（色温度 2000〜50000 K、tint ±150。genzo-model の
//!   `ranges`）の外なら、範囲に丸めて返す（[`WhiteBalancePick::clamped`]）。中立でない色の点を選んでも、
//!   UI がそのまま適用できる値を返すため（エラーにしない）。カメラで作れない値は、描画のときに作れる範囲に
//!   丸めて警告する（[`crate::sensor::wb::resolve_white_balance`]）。
//! - **求められない点**: 平均したカメラ RGB のどれかが 0 以下（真っ黒な点など）、または色が極端で白の
//!   色度を求められない（白の Y が 0 以下など）ならエラー。
//! - **白飛びした点**: ステージ 3 の飽和の切りそろえで値が今の WB の白に寄るので、今の WB に近い値に
//!   なる（正しい値は求められない。エラーにはしない）。
//! - **RAW 以外の入力**（WB の設定を使わない）はエラー。

use genzo_color::ColorError;
use genzo_color::camera::working_to_camera_normalized;
use genzo_color::white_balance::{
    WB_TEMPERATURE_MAX_K, WB_TEMPERATURE_MIN_K, WB_TINT_MAX, WB_TINT_MIN,
    temperature_tint_from_wb_multipliers,
};
use genzo_model::{DevelopSettings, RenderQuality};

use super::{Engine, PhotoSource};
use crate::error::{PipelineError, Result};
use crate::finish::geometry::GeometryPlan;
use crate::sensor::{SensorPlan, SourceImage};

/// WB のスポイトで平均する範囲の半径（見える範囲（RAW の既定の切り抜き）の長辺に対する割合）。
///
/// **仮置き**: 0.4%（長辺 7008 画素の写真で約 28 画素、長辺 2560 の A1 で約 10 画素。21 × 21 画素ほどを
/// 平均する）。ノイズを均しつつ、小さな灰色の被写体も選べる大きさとして置いた。実写で見直す。
pub const WB_PICK_RADIUS: f64 = 0.004;

/// WB のスポイトの結果（[`Engine::white_balance_at`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteBalancePick {
    /// 色温度（K。設定できる範囲に丸めた値）。
    pub temperature_k: f64,
    /// 色かぶり補正（tint。設定できる範囲に丸めた値）。
    pub tint: f64,
    /// 推定した値が設定できる範囲の外で、範囲に丸めたか。
    pub clamped: bool,
    /// 平均したカメラ RGB（WB をかける前。黒レベルを 0、白レベルを 1 とした値）。
    pub camera_rgb: [f64; 3],
    /// その点を中立にする WB の係数（R, G, B。G = 1）。
    pub multipliers: [f64; 3],
    /// 平均した A1 の画素の数。
    pub samples: u64,
}

impl Engine {
    /// 指定した点を中立にする WB を求める（モジュールの doc の「方法」）。設定は変えない。
    ///
    /// - `point`: プレビューの出力の画像（C。`uncropped` なら切り抜く前の全体）の幅・高さに対する割合
    ///   （各 0〜1。左上が (0, 0)）。
    /// - `uncropped`: プレビューが切り抜く前の全体の表示か（[`crate::PreviewRequest::uncropped`]）。
    ///
    /// エラー（[`PipelineError::InvalidArgument`]）: RAW 以外の入力、点が 0〜1 の外・有限でない、点が表示で
    /// 塗る画素（`uncropped` で、回転で画像の外になった四隅）にある、平均した色から係数を作れない。
    /// ふつうの表示（切り抜いた後）では、0〜1 の点はすべて画像の上として扱う（端の点も選べる）。
    pub fn white_balance_at(
        &self,
        source: &PhotoSource,
        settings: &DevelopSettings,
        point: (f64, f64),
        uncropped: bool,
    ) -> Result<WhiteBalancePick> {
        let SourceImage::Raw(raw) = &source.image else {
            return Err(PipelineError::InvalidArgument(
                "RAW 以外の写真（JPEG など）は WB の設定を使わないため、WB のスポイトは使えません"
                    .to_owned(),
            ));
        };
        let (px, py) = point;
        let unit = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
        if !(unit(px) && unit(py)) {
            return Err(PipelineError::InvalidArgument(format!(
                "点の位置は画像の幅・高さに対する 0〜1 の割合で指定してください（{px}, {py}）"
            )));
        }
        // カメラプロファイル（DCP）の色相彩度のマップは逆に戻せないので、プロファイルを使わない A1（今のカメラ行列の
        // ステージ 8）で求める（WB の係数の変換はプロファイルを使っても今のカメラ行列のまま。crate::profile）。
        let without_profile;
        let settings = if settings.profile.camera.is_some() {
            let mut s = settings.clone();
            s.profile.camera = None;
            without_profile = s;
            &without_profile
        } else {
            settings
        };
        let a1 = self.a1(
            source,
            settings,
            RenderQuality::Final,
            self.config.preview_long_edge,
        )?;
        let ctx = a1.finish_context(settings)?.with_uncropped(uncropped);
        let geometry = GeometryPlan::from_settings(settings, &ctx)?;
        let (cw, ch) = geometry.output_size();
        let (cx, cy) = (px * f64::from(cw), py * f64::from(ch));
        // 表示で塗る画素（切り抜く前の全体の表示で、回転で画像の外になった四隅）を指したらエラー。描画と
        // 同じ画素の単位で判定する（0・1 ちょうどの点は端の画素）。
        let pixel = |c: f64, n: u32| (c.floor() as i64).clamp(0, i64::from(n) - 1);
        if geometry.c_pixel_outside_image(pixel(cx, cw), pixel(cy, ch)) {
            return Err(PipelineError::InvalidArgument(
                "指定した点は画像の外です（回転で画像の外になった部分）".to_owned(),
            ));
        }
        // C → G → O。画像の内側に収める（端の画素の中心より外側は、C の格子の丸め・切り抜きの制限で
        // O の外にわずかにかかることがあるが、表示ではそこにも鏡映で埋めた画像が見えているため）。
        let (gx, gy) = geometry.c_to_g_px(cx, cy);
        let frames = geometry.frames();
        let (u, v) = frames.g_to_oriented(gx, gy);
        let (ow, oh) = frames.oriented_size();
        let (u, v) = (u.clamp(0.0, f64::from(ow)), v.clamp(0.0, f64::from(oh)));
        // O → S → A1 の画素の座標。
        let (sx, sy) = frames.oriented_to_sensor(u, v);
        let (kx, ky) = a1.sensor_pixels_per_pixel();
        let radius = WB_PICK_RADIUS * f64::from(ow.max(oh));
        let (w, h) = a1.image.dimensions();
        let xs = pixel_span(sx / kx, (radius / kx).max(1.0), w);
        let ys = pixel_span(sy / ky, (radius / ky).max(1.0), h);
        // 中心が既定の切り抜き（見える範囲）の内側にある画素だけを平均する（外側は詰め物のことがある。
        // coords の doc の「既定の切り抜き」）。
        let crop = frames.default_crop;
        let inside = |c: f64, start: u32, len: u32| {
            (f64::from(start)..=f64::from(start) + f64::from(len)).contains(&c)
        };
        let mut sum = [0.0f64; 3];
        let mut samples = 0u64;
        for y in ys {
            if !inside((f64::from(y) + 0.5) * ky, crop.y, crop.height) {
                continue;
            }
            let row = a1.image.row(y);
            for x in xs.clone() {
                if !inside((f64::from(x) + 0.5) * kx, crop.x, crop.width) {
                    continue;
                }
                let p = row[x as usize];
                for (s, v) in sum.iter_mut().zip(p) {
                    *s += f64::from(v);
                }
                samples += 1;
            }
        }
        if samples == 0 {
            return Err(PipelineError::InvalidArgument(
                "指定した点のまわりに画像の画素がありません".to_owned(),
            ));
        }
        let mean = sum.map(|v| v / samples as f64);
        // B2 → B1（ステージ 8 の逆）→ カメラ RGB（ステージ 3 の逆。A1 と同じ設定で係数を求める）。
        let plan = SensorPlan::new(raw, settings, &self.sensor_options(RenderQuality::Final))?;
        let b1 = working_to_camera_normalized(plan.cam_xyz())?.apply(mean);
        let m = plan.wb().multipliers;
        let camera_rgb = [
            b1[0] / f64::from(m[0]),
            b1[1] / f64::from(m[1]),
            b1[2] / f64::from(m[2]),
        ];
        if !camera_rgb.iter().all(|v| v.is_finite() && *v > 0.0) {
            return Err(PipelineError::InvalidArgument(
                "指定した点が暗すぎる（または色が極端な）ため、WB を求められません".to_owned(),
            ));
        }
        let multipliers = [
            camera_rgb[1] / camera_rgb[0],
            1.0,
            camera_rgb[1] / camera_rgb[2],
        ];
        let (temperature_k, tint, clamped) =
            match temperature_tint_from_wb_multipliers(plan.cam_xyz(), multipliers) {
                Ok(t) => (t.temperature_k, t.tint, false),
                Err(ColorError::WhiteBalanceEstimateOutOfRange {
                    temperature_k,
                    tint,
                }) if temperature_k.is_finite() && tint.is_finite() => (
                    temperature_k.clamp(WB_TEMPERATURE_MIN_K, WB_TEMPERATURE_MAX_K),
                    tint.clamp(WB_TINT_MIN, WB_TINT_MAX),
                    true,
                ),
                Err(
                    ColorError::InvalidWhiteBalance { .. } | ColorError::InvalidChromaticity { .. },
                ) => {
                    return Err(PipelineError::InvalidArgument(
                        "指定した点の色が極端なため、WB を求められません".to_owned(),
                    ));
                }
                Err(e) => return Err(e.into()),
            };
        Ok(WhiteBalancePick {
            temperature_k,
            tint,
            clamped,
            camera_rgb,
            multipliers,
            samples,
        })
    }
}

/// 中心 `c`・半径 `r`（画素の単位の連続座標）の範囲に中心がある画素の番号（`0..n` に収める）。
fn pixel_span(c: f64, r: f64, n: u32) -> std::ops::Range<u32> {
    let lo = (c - r - 0.5).ceil().max(0.0);
    let hi = (c + r - 0.5).floor() + 1.0;
    let hi = hi.clamp(0.0, f64::from(n));
    if hi <= lo {
        return 0..0;
    }
    lo as u32..hi as u32
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use genzo_color::Mat3;
    use genzo_model::{CropRect, FileId, WhiteBalance};
    use genzo_raw::{CfaPattern, RawImage};

    use super::*;
    use crate::engine::{EngineConfig, SourceId};

    /// テスト用の「典型的な形の」カメラ行列（実在の機種の値ではない）。
    const CAM_XYZ: [[f32; 3]; 3] = [
        [0.7424, -0.2329, -0.0466],
        [-0.4598, 1.2471, 0.2347],
        [-0.0715, 0.1505, 0.6066],
    ];
    const AS_SHOT: [f32; 4] = [2.0, 1.0, 1.5, 1.0];

    /// 4 つの象限で色の違う「灰色」の面を写した RAW（240 × 160、RGGB、14bit）。`cast(x, y)` は、撮影時の
    /// WB をかけたときの各面のカメラ RGB（左上は中立）。
    fn quadrant_cast(x: u32, y: u32) -> [f32; 3] {
        match (x < 120, y < 80) {
            (true, true) => [0.2, 0.2, 0.2],
            (false, true) => [0.26, 0.2, 0.16],
            (true, false) => [0.17, 0.2, 0.25],
            (false, false) => [0.2, 0.24, 0.19],
        }
    }

    fn source() -> PhotoSource {
        source_with(1, |_| {})
    }

    /// [`source`] の RAW を `edit` で変えたもの（エンジンのキャッシュで取り違えないよう、ファイルの id を
    /// `file_id` にする）。
    fn source_with(file_id: i64, edit: impl FnOnce(&mut RawImage)) -> PhotoSource {
        let (w, h) = (240u32, 160u32);
        let (black, white) = (512.0f32, 16383.0f32);
        let data = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let color = CfaPattern::RGGB.color_at(x as usize, y as usize);
                let b1 = quadrant_cast(x, y)[color.rgb_channel()];
                (black + b1 / AS_SHOT[color.index()] * (white - black)).round() as u16
            })
            .collect();
        let mut raw = RawImage {
            width: w,
            height: h,
            cfa: CfaPattern::RGGB,
            data,
            black_level: [black; 4],
            white_level: white,
            saturation_level: white,
            default_crop: None,
            as_shot_wb: AS_SHOT,
            cam_xyz: Some(CAM_XYZ),
            metadata: Default::default(),
            lens_correction: None,
        };
        edit(&mut raw);
        PhotoSource::new(
            SourceId::new(FileId::new(file_id), 1),
            SourceImage::Raw(Arc::new(raw)),
        )
        .unwrap()
    }

    fn engine() -> Engine {
        Engine::new(EngineConfig {
            preview_long_edge: 120,
            ..Default::default()
        })
        .unwrap()
    }

    fn cam() -> Mat3 {
        Mat3::from_rows(CAM_XYZ.map(|r| r.map(f64::from)))
    }

    /// 撮影時の WB の色温度・tint。
    fn as_shot() -> genzo_color::TemperatureTint {
        temperature_tint_from_wb_multipliers(
            &cam(),
            [f64::from(AS_SHOT[0]), 1.0, f64::from(AS_SHOT[2])],
        )
        .unwrap()
    }

    fn custom(temperature_k: f32, tint: f32) -> DevelopSettings {
        DevelopSettings {
            white_balance: WhiteBalance::Custom {
                temperature_k,
                tint,
            },
            ..Default::default()
        }
    }

    /// 灰色の面（撮影時の WB で中立）を選ぶと、今の WB によらず撮影時の値が返る。
    #[test]
    fn picking_a_neutral_patch_returns_the_as_shot_white_balance() {
        let (engine, source) = (engine(), source());
        let expected = as_shot();
        for settings in [
            DevelopSettings::default(),
            custom(3000.0, 20.0),
            custom(9000.0, -30.0),
        ] {
            let p = engine
                .white_balance_at(&source, &settings, (0.25, 0.25), false)
                .unwrap();
            assert!(!p.clamped, "{p:?}");
            assert!(p.samples >= 4, "{p:?}");
            assert!(
                (p.temperature_k - expected.temperature_k).abs() < 5.0
                    && (p.tint - expected.tint).abs() < 0.2,
                "{p:?} / {expected:?} / {:?}",
                settings.white_balance
            );
            for (a, b) in p.multipliers.iter().zip([2.0, 1.0, 1.5]) {
                assert!((a - b).abs() < 2e-3, "{p:?}");
            }
            // 返した値を Custom の WB に入れると、その面を中立にする係数に戻る。
            let back = crate::sensor::wb::resolve_multipliers(
                &WhiteBalance::Custom {
                    temperature_k: p.temperature_k as f32,
                    tint: p.tint as f32,
                },
                AS_SHOT,
                &cam(),
            )
            .unwrap();
            assert!((back[0] - 2.0).abs() < 2e-3 && (back[2] - 1.5).abs() < 2e-3);
        }
        // 色のかかった面では、その面を中立にする値（撮影時の値とは違う）。
        let p = engine
            .white_balance_at(&source, &DevelopSettings::default(), (0.75, 0.25), false)
            .unwrap();
        let c = quadrant_cast(200, 10);
        let want = [
            f64::from(AS_SHOT[0] * c[1] / c[0]),
            1.0,
            f64::from(AS_SHOT[2] * c[1] / c[2]),
        ];
        for (a, b) in p.multipliers.iter().zip(want) {
            assert!((a - b).abs() < 5e-3, "{p:?} / {want:?}");
        }
        // 光源が暖色寄りとみなすので、色温度は撮影時より低い。
        assert!(p.temperature_k < expected.temperature_k - 100.0, "{p:?}");
    }

    /// 切り抜き・回転・向きがあっても、出力の画像の上の点は同じ被写体の点を指す（ステージ 9 と同じ変換）。
    #[test]
    fn the_same_subject_point_is_picked_with_crop_rotation_and_orientation() {
        let engine = engine();
        // 各象限の中心（S の画素の座標）。
        let centers = [(60.0, 40.0), (180.0, 40.0), (60.0, 120.0), (180.0, 120.0)];
        for (id, orientation) in [
            (10, genzo_model::Orientation::Normal),
            (11, genzo_model::Orientation::Rotate90Cw),
            (12, genzo_model::Orientation::Transverse),
        ] {
            let source = source_with(id, |raw| raw.metadata.orientation = orientation);
            let base = DevelopSettings::default();
            let mut edited = base.clone();
            edited.geometry.angle_deg = 7.0;
            edited.geometry.crop = CropRect {
                left: 0.1,
                top: 0.15,
                right: 0.85,
                bottom: 0.9,
            };
            for &(sx, sy) in &centers {
                let picks: Vec<WhiteBalancePick> = [&base, &edited]
                    .iter()
                    .map(|s| {
                        let point = c_fraction_of_sensor_point(&engine, &source, s, (sx, sy));
                        engine.white_balance_at(&source, s, point, false).unwrap()
                    })
                    .collect();
                // その点の面を中立にする係数（向きを取り違えると別の面になる）。
                let c = quadrant_cast(sx as u32, sy as u32);
                let want = [
                    f64::from(AS_SHOT[0] * c[1] / c[0]),
                    1.0,
                    f64::from(AS_SHOT[2] * c[1] / c[2]),
                ];
                for p in &picks {
                    for (a, b) in p.multipliers.iter().zip(want) {
                        assert!(
                            (a - b).abs() < 5e-3,
                            "{orientation:?} {sx},{sy}: {picks:?} / {want:?}"
                        );
                    }
                }
                // 切り抜く前の全体の表示でも同じ点を指す。
                let point = {
                    let a1 = engine
                        .a1(&source, &edited, RenderQuality::Final, 120)
                        .unwrap();
                    let ctx = a1.finish_context(&edited).unwrap().with_uncropped(true);
                    let g = GeometryPlan::from_settings(&edited, &ctx).unwrap();
                    let (gx, gy) = g.frames().sensor_to_g(sx, sy);
                    let (gw, gh) = g.frames().g_size_px();
                    (gx / gw, gy / gh)
                };
                let p = engine
                    .white_balance_at(&source, &edited, point, true)
                    .unwrap();
                for (a, b) in picks[0].multipliers.iter().zip(p.multipliers) {
                    assert!((a - b).abs() < 5e-3, "{orientation:?} {sx},{sy}: {p:?}");
                }
            }
        }
    }

    /// S の点 `(sx, sy)` が、設定 `s` のプレビューの出力の画像の上のどこ（幅・高さに対する割合）にあるか。
    fn c_fraction_of_sensor_point(
        engine: &Engine,
        source: &PhotoSource,
        s: &DevelopSettings,
        (sx, sy): (f64, f64),
    ) -> (f64, f64) {
        let a1 = engine.a1(source, s, RenderQuality::Final, 120).unwrap();
        let ctx = a1.finish_context(s).unwrap();
        let g = GeometryPlan::from_settings(s, &ctx).unwrap();
        let (gx, gy) = g.frames().sensor_to_g(sx, sy);
        let (cx, cy) = g.g_px_to_c(gx, gy);
        let (cw, ch) = g.output_size();
        (cx / f64::from(cw), cy / f64::from(ch))
    }

    #[test]
    fn invalid_points_and_non_raw_inputs_are_rejected() {
        let (engine, source) = (engine(), source());
        let s = DevelopSettings::default();
        for p in [(-0.1, 0.5), (0.5, 1.5), (f64::NAN, 0.5)] {
            assert!(matches!(
                engine.white_balance_at(&source, &s, p, false),
                Err(PipelineError::InvalidArgument(_))
            ));
        }
        // 切り抜く前の全体の表示で、回転で画像の外になった角。
        let mut rotated = s.clone();
        rotated.geometry.angle_deg = 20.0;
        assert!(matches!(
            engine.white_balance_at(&source, &rotated, (0.01, 0.01), true),
            Err(PipelineError::InvalidArgument(_))
        ));
        // 切り抜きの範囲（画像の内側に制限される）では角でも使える。
        assert!(
            engine
                .white_balance_at(&source, &rotated, (0.01, 0.01), false)
                .is_ok()
        );
        // RAW 以外。
        let working = PhotoSource::new(
            SourceId::new(FileId::new(2), 1),
            SourceImage::Working {
                image: Arc::new(
                    crate::RgbImage::filled(8, 8, crate::ColorContract::B2Working, [0.18; 3])
                        .unwrap(),
                ),
                orientation: genzo_model::Orientation::Normal,
            },
        )
        .unwrap();
        assert!(matches!(
            engine.white_balance_at(&working, &s, (0.5, 0.5), false),
            Err(PipelineError::InvalidArgument(_))
        ));
        // 真っ黒な点。
        let dark = source_with(3, |raw| raw.data.fill(512));
        assert!(matches!(
            engine.white_balance_at(&dark, &s, (0.5, 0.5), false),
            Err(PipelineError::InvalidArgument(_))
        ));
    }

    /// ふつうの表示（切り抜いた後）では、角度補正があっても出力の画像の四隅・四辺（0・1 ちょうど）を
    /// 選べる。出力の端は、C の格子の丸め・切り抜きの制限（四隅が画像の辺にちょうど接する）で画像（O）の外に
    /// わずかにかかることがあるが、表示ではそこも鏡映の値で埋めた画像が見えているため。
    #[test]
    fn edges_of_the_cropped_output_can_be_picked_at_any_angle() {
        let engine = engine();
        let crops = [
            CropRect::FULL,
            CropRect {
                left: 0.1,
                top: 0.05,
                right: 0.97,
                bottom: 0.9,
            },
        ];
        let points = [
            (0.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (1.0, 1.0),
            (0.5, 0.0),
            (0.0, 0.5),
            (1.0, 0.5),
            (0.5, 1.0),
        ];
        for (id, orientation) in [
            (30, genzo_model::Orientation::Normal),
            (31, genzo_model::Orientation::Rotate90Cw),
        ] {
            let source = source_with(id, |raw| raw.metadata.orientation = orientation);
            for angle in [7.0f32, 15.0, 30.0, 44.0, -7.0, -44.0] {
                for crop in crops {
                    let mut s = DevelopSettings::default();
                    s.geometry.angle_deg = angle;
                    s.geometry.crop = crop;
                    for p in points {
                        let r = engine.white_balance_at(&source, &s, p, false);
                        assert!(r.is_ok(), "{orientation:?} {angle}° {crop:?} {p:?}: {r:?}");
                    }
                }
            }
        }
    }

    /// 切り抜く前の全体の表示では、表示で塗る画素（回転で画像の外になった四隅。
    /// [`GeometryPlan::c_pixel_outside_image`]）を指したときだけエラーになる。塗らない画素なら、端
    /// （0・1 ちょうど）でも選べる。
    #[test]
    fn only_the_filled_corners_of_the_uncropped_view_are_rejected() {
        let (engine, source) = (engine(), source());
        for angle in [0.0f32, 7.0, 30.0, -44.0] {
            let mut s = DevelopSettings::default();
            s.geometry.angle_deg = angle;
            // 切り抜く前の全体の表示では、設定の切り抜きは使わない。
            s.geometry.crop = CropRect {
                left: 0.2,
                top: 0.2,
                right: 0.8,
                bottom: 0.8,
            };
            let a1 = engine.a1(&source, &s, RenderQuality::Final, 120).unwrap();
            let ctx = a1.finish_context(&s).unwrap().with_uncropped(true);
            let g = GeometryPlan::from_settings(&s, &ctx).unwrap();
            let (cw, ch) = g.output_size();
            let (fw, fh) = (f64::from(cw), f64::from(ch));
            // 四辺の上の点（各画素の中心の位置で、辺にちょうど接する点）。
            let mut points = Vec::new();
            for i in 0..cw {
                let x = (f64::from(i) + 0.5) / fw;
                points.extend([(x, 0.0), (x, 1.0)]);
            }
            for j in 0..ch {
                let y = (f64::from(j) + 0.5) / fh;
                points.extend([(0.0, y), (1.0, y)]);
            }
            let mut rejected = 0;
            for (x, y) in points {
                let ix = ((x * fw).floor() as i64).min(i64::from(cw) - 1);
                let iy = ((y * fh).floor() as i64).min(i64::from(ch) - 1);
                let r = engine.white_balance_at(&source, &s, (x, y), true);
                if g.c_pixel_outside_image(ix, iy) {
                    assert!(
                        matches!(r, Err(PipelineError::InvalidArgument(_))),
                        "{angle}° ({x}, {y}): {r:?}"
                    );
                    rejected += 1;
                } else {
                    assert!(r.is_ok(), "{angle}° ({x}, {y}): {r:?}");
                }
            }
            if angle == 0.0 {
                assert_eq!(rejected, 0);
            } else {
                assert!(rejected > 0, "{angle}°");
            }
        }
    }

    /// 推定が設定できる範囲の外なら、範囲に丸めて返す。
    #[test]
    fn out_of_range_estimates_are_clamped() {
        let engine = engine();
        let filled = |id: i64, b1: [f32; 3]| {
            source_with(id, |raw| {
                let w = raw.width as usize;
                for (i, v) in raw.data.iter_mut().enumerate() {
                    let color = CfaPattern::RGGB.color_at(i % w, i / w);
                    let b1 = b1[color.rgb_channel()];
                    *v = (512.0 + b1 / AS_SHOT[color.index()] * (16383.0 - 512.0)).round() as u16;
                }
            })
        };
        let s = DevelopSettings::default();
        // 強い橙色の面（推定は 2000 K より低い）。
        let p = engine
            .white_balance_at(&filled(20, [0.5, 0.2, 0.04]), &s, (0.5, 0.5), false)
            .unwrap();
        assert!(p.clamped, "{p:?}");
        assert!(
            (WB_TEMPERATURE_MIN_K..=WB_TEMPERATURE_MAX_K).contains(&p.temperature_k)
                && (WB_TINT_MIN..=WB_TINT_MAX).contains(&p.tint),
            "{p:?}"
        );
        // 白の色度を求められないほど極端な色はエラー。
        assert!(matches!(
            engine.white_balance_at(&filled(21, [0.002, 0.05, 0.4]), &s, (0.5, 0.5), false),
            Err(PipelineError::InvalidArgument(_))
        ));
    }

    #[test]
    fn pixel_spans_cover_centers_within_the_radius() {
        assert_eq!(pixel_span(10.0, 2.0, 100), 8..12);
        assert_eq!(pixel_span(10.5, 1.0, 100), 9..12);
        assert_eq!(pixel_span(0.0, 1.0, 100), 0..1);
        assert_eq!(pixel_span(100.0, 1.0, 100), 99..100);
        assert_eq!(pixel_span(500.0, 1.0, 100), 0..0);
    }
}
