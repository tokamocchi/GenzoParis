//! カメラプロファイルのルックテーブルと、独自のルック（3D LUT）（DEV-17。docs/04_architecture.md の 2.1 節。処理
//! バージョン 2）。
//!
//! # ステージ 10 の後: プロファイルのルックテーブル（[`ProfileLookTableStage`]。`finish.profile_look_table`）
//!
//! カメラプロファイル（DCP）の ProfileLookTable を、**露光量（ステージ 10）の後、コントラスト・トーンの前**に、
//! リニア ProPhoto RGB で当てる（DNG の仕様の順: 露光の後、トーンカーブの前。明度の分割のある表は、露光量を
//! 変えると明度の位置が変わる）。B2 → ProPhoto → 表（genzo-color の [`HsvTable::apply`]）→ B2。行列は
//! genzo-color の `dcp::working_to_prophoto`・`dcp::prophoto_to_working`（f64 で求めて f32 に丸めたもの）。RAW 以外の
//! 写真（ステージ 8 がない）・プロファイルにルックテーブルがない・何もしない表なら飛ばす。
//!
//! カメラの BaselineExposure は使わない（2.6 節）ので、明度の分割のある表は、Adobe の描画とは明度の位置がずれうる
//! （決定事項ログ）。
//!
//! # ステージ 15 の後: 独自のルック（[`LookStage`]。`finish.look`）
//!
//! 利用者の 3D LUT（.cube。genzo-color の [`CubeLut`]）を、**シーン → ディスプレイの変換（ステージ 15）の後、
//! トーンカーブ（ステージ 16）の前**の B3（ディスプレイ参照）に当てる（作品づくりの LUT はふつう表示用の値を
//! 前提にするため。トーンカーブ・効果はルックの後にかかる）。LUT の入力は **sRGB（BT.709 の原色・D65・
//! IEC 61966-2-1 の伝達関数）** とみなす。画素ごとに（f32。GPU 版も同じ式・順序）:
//!
//! ```text
//! l  = (B3 → リニア sRGB の行列) · c
//! lc = clamp(l, 0, 1)（チャンネルごと）
//! o  = srgb_decode(LUT(srgb_encode(lc)))      （LUT は DOMAIN で位置を決めて四面体補間）
//! l' = l + a · (o − lc)                        （a は適用量 / 100。色域の外の分（l − lc）は足し戻す）
//! c' = (リニア sRGB → B3 の行列) · l'
//! ```
//!
//! 適用量 0 なら何もせず（ステージを飛ばす）、100 で LUT のとおり（色域の外の分を除く）。LUT が 0〜1 の外の値を
//! 返すと、B3 の輝度が 1 をわずかに超えうる（ステージ 17 の色域の圧縮が 0〜1 に収める）。

use std::sync::Arc;

use genzo_color::dcp::{prophoto_to_working, working_to_prophoto};
use genzo_color::space::{to_working, working_to};
use genzo_color::transfer::{srgb_decode_f32, srgb_encode_f32};
use genzo_color::{CubeLut, DcpProfile, HsvTable, Mat3F32, RgbColorSpace};
use genzo_model::{DevelopSettings, Phase};

use super::map_pixels;
use crate::contract::ColorContract;
use crate::error::{PipelineError, Result};
use crate::image::{ImageTile, Roi};
use crate::stage::{InputKind, Stage, StageContext, StageParams};
use crate::version::ProcessVersion;

/// プロファイルのルックテーブルのパラメータ。
#[derive(Debug, Clone)]
pub struct LookTableParams {
    /// 作業色空間 → リニア ProPhoto RGB（f32）。
    pub working_to_prophoto: Mat3F32,
    /// リニア ProPhoto RGB → 作業色空間（f32）。
    pub prophoto_to_working: Mat3F32,
    /// ルックテーブルを持つプロファイル（`look_table` は `Some`）。
    pub profile: Arc<DcpProfile>,
}

impl LookTableParams {
    /// ルックテーブル。
    pub fn table(&self) -> &HsvTable {
        self.profile
            .look_table
            .as_ref()
            .expect("ルックテーブルのあるプロファイルだけで作る")
    }

    /// 1 画素に当てる（モジュールの doc）。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let p = self.working_to_prophoto.apply(c);
        self.prophoto_to_working.apply(self.table().apply(p))
    }
}

/// ステージ 10 の後: カメラプロファイルのルックテーブル（モジュールの doc）。
#[derive(Debug, Clone, Copy, Default)]
pub struct ProfileLookTableStage;

impl ProfileLookTableStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.profile_look_table";
}

impl Stage for ProfileLookTableStage {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn phase(&self) -> Phase {
        Phase::C
    }
    fn input_contract(&self) -> ColorContract {
        ColorContract::B2Working
    }
    fn output_contract(&self) -> ColorContract {
        ColorContract::B2Working
    }
    fn params(
        &self,
        settings: &DevelopSettings,
        ctx: &StageContext<'_>,
    ) -> Result<Option<StageParams>> {
        match ctx.process_version {
            ProcessVersion::V1 => Ok(None),
            ProcessVersion::V2 => {
                if ctx.input_kind != InputKind::SceneReferred
                    || settings.normalized().profile.camera.is_none()
                {
                    return Ok(None);
                }
                let Some(profile) = ctx
                    .profiles
                    .and_then(|p| p.camera.as_ref())
                    .map(|c| Arc::clone(&c.data))
                else {
                    return Ok(None);
                };
                if profile
                    .look_table
                    .as_ref()
                    .is_none_or(HsvTable::is_identity)
                {
                    return Ok(None);
                }
                Ok(Some(StageParams::new(LookTableParams {
                    working_to_prophoto: working_to_prophoto()?.to_f32(),
                    prophoto_to_working: prophoto_to_working()?.to_f32(),
                    profile,
                })))
            }
        }
    }
    fn input_roi(&self, output_roi: Roi, _: &StageParams, _: &StageContext<'_>) -> Roi {
        output_roi
    }
    fn run_cpu(
        &self,
        ctx: &StageContext<'_>,
        input: &ImageTile,
        output: &mut ImageTile,
        params: &StageParams,
    ) -> Result<()> {
        let p = params.expect::<LookTableParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 => Err(PipelineError::PlanMismatch(
                "処理バージョン 1 ではカメラプロファイルのルックテーブルを適用しない",
            )),
            ProcessVersion::V2 => map_pixels(input, output, |_, _, c| p.apply(c)),
        }
    }
}

/// 独自のルック（3D LUT）のパラメータ。
#[derive(Debug, Clone)]
pub struct LookParams {
    /// 作業色空間（B3）→ リニア sRGB（f32）。
    pub working_to_srgb: Mat3F32,
    /// リニア sRGB → 作業色空間（f32）。
    pub srgb_to_working: Mat3F32,
    /// LUT。
    pub lut: Arc<CubeLut>,
    /// 適用量（0〜1。設定の % を 100 で割ったもの）。
    pub amount: f32,
}

impl LookParams {
    /// 1 画素に当てる（モジュールの doc の式）。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let l = self.working_to_srgb.apply(c);
        let lc = l.map(|v| v.clamp(0.0, 1.0));
        let o = self.lut.apply(lc.map(srgb_encode_f32)).map(srgb_decode_f32);
        let a = self.amount;
        let r = [
            l[0] + a * (o[0] - lc[0]),
            l[1] + a * (o[1] - lc[1]),
            l[2] + a * (o[2] - lc[2]),
        ];
        self.srgb_to_working.apply(r)
    }
}

/// ステージ 15 の後: 独自のルック（3D LUT。モジュールの doc）。
#[derive(Debug, Clone, Copy, Default)]
pub struct LookStage;

impl LookStage {
    /// ステージの ID。
    pub const ID: &'static str = "finish.look";
}

impl Stage for LookStage {
    fn id(&self) -> &'static str {
        Self::ID
    }
    fn phase(&self) -> Phase {
        Phase::C
    }
    fn input_contract(&self) -> ColorContract {
        ColorContract::B3DisplayReferred
    }
    fn output_contract(&self) -> ColorContract {
        ColorContract::B3DisplayReferred
    }
    fn params(
        &self,
        settings: &DevelopSettings,
        ctx: &StageContext<'_>,
    ) -> Result<Option<StageParams>> {
        match ctx.process_version {
            ProcessVersion::V1 => Ok(None),
            ProcessVersion::V2 => {
                let s = settings.normalized();
                if !s.profile.look_is_active() {
                    return Ok(None);
                }
                let Some(lut) = ctx
                    .profiles
                    .and_then(|p| p.look.as_ref())
                    .map(|l| Arc::clone(&l.data))
                else {
                    return Ok(None);
                };
                Ok(Some(StageParams::new(LookParams {
                    working_to_srgb: working_to(RgbColorSpace::Srgb).to_f32(),
                    srgb_to_working: to_working(RgbColorSpace::Srgb).to_f32(),
                    lut,
                    amount: (f64::from(s.profile.look_amount) / 100.0) as f32,
                })))
            }
        }
    }
    fn input_roi(&self, output_roi: Roi, _: &StageParams, _: &StageContext<'_>) -> Roi {
        output_roi
    }
    fn run_cpu(
        &self,
        ctx: &StageContext<'_>,
        input: &ImageTile,
        output: &mut ImageTile,
        params: &StageParams,
    ) -> Result<()> {
        let p = params.expect::<LookParams>(Self::ID)?;
        match ctx.process_version {
            ProcessVersion::V1 => Err(PipelineError::PlanMismatch(
                "処理バージョン 1 ではルックを適用しない",
            )),
            ProcessVersion::V2 => map_pixels(input, output, |_, _, c| p.apply(c)),
        }
    }
}

#[cfg(test)]
mod tests {
    use genzo_color::Lut3d;
    use genzo_model::DataRef;

    use super::*;
    use crate::coords::CoordinateFrames;
    use crate::profile::{ProfileResources, Resolved};

    fn ctx_with<'a>(res: &'a ProfileResources, version: ProcessVersion) -> StageContext<'a> {
        let frames = CoordinateFrames::new(8, 6, genzo_model::Orientation::Normal).unwrap();
        StageContext::new(version, frames).with_profiles(res)
    }

    fn inverting_lut() -> CubeLut {
        let lut = Lut3d::from_fn(5, |v| [1.0 - v[0], 1.0 - v[1], 1.0 - v[2]]).unwrap();
        CubeLut::new(None, 5, lut.data().to_vec(), [0.0; 3], [1.0; 3]).unwrap()
    }

    fn resources_with_look(lut: CubeLut) -> ProfileResources {
        ProfileResources {
            look: Some(Resolved {
                data: Arc::new(lut),
                data_ref: DataRef::new("cube:x", ""),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn look_amount_blends_and_keeps_out_of_gamut_parts() {
        let res = resources_with_look(inverting_lut());
        let mut s = DevelopSettings::default();
        s.profile.look = Some("x".into());
        let ctx = ctx_with(&res, ProcessVersion::V2);
        let p = LookStage.params(&s, &ctx).unwrap().unwrap();
        let p = p.get::<LookParams>().unwrap().clone();
        // 中間グレー（sRGB で 0.5）は反転して 0.5。
        let g = srgb_decode_f32(0.5);
        let out = p.apply([g; 3]);
        for v in out {
            assert!((v - g).abs() < 1e-5, "{out:?}");
        }
        // 黒は白になる。
        let out = p.apply([0.0; 3]);
        for v in out {
            assert!((v - 1.0).abs() < 1e-4, "{out:?}");
        }
        // 適用量 50 なら半分（リニアで）。
        s.profile.look_amount = 50.0;
        let p = LookStage.params(&s, &ctx).unwrap().unwrap();
        let p = p.get::<LookParams>().unwrap().clone();
        let out = p.apply([0.0; 3]);
        for v in out {
            assert!((v - 0.5).abs() < 1e-4, "{out:?}");
        }
        // 色域の外の分（sRGB で負の値）は足し戻す: 恒等の LUT なら変わらない。
        let id = CubeLut::new(
            None,
            9,
            Lut3d::identity(9).unwrap().data().to_vec(),
            [0.0; 3],
            [1.0; 3],
        )
        .unwrap();
        let res = resources_with_look(id);
        s.profile.look_amount = 100.0;
        let ctx = ctx_with(&res, ProcessVersion::V2);
        let p = LookStage.params(&s, &ctx).unwrap().unwrap();
        let p = p.get::<LookParams>().unwrap().clone();
        let c = to_working(RgbColorSpace::Srgb)
            .to_f32()
            .apply([0.6, -0.05, 0.2]);
        let out = p.apply(c);
        for k in 0..3 {
            assert!((out[k] - c[k]).abs() < 2e-3, "{out:?} {c:?}");
        }
    }

    #[test]
    fn stages_are_skipped_without_resources_or_in_version_1() {
        let res = resources_with_look(inverting_lut());
        let mut s = DevelopSettings::default();
        s.profile.look = Some("x".into());
        assert!(
            LookStage
                .params(&s, &ctx_with(&res, ProcessVersion::V1))
                .unwrap()
                .is_none()
        );
        let empty = ProfileResources::default();
        assert!(
            LookStage
                .params(&s, &ctx_with(&empty, ProcessVersion::V2))
                .unwrap()
                .is_none()
        );
        s.profile.look_amount = 0.0;
        assert!(
            LookStage
                .params(&s, &ctx_with(&res, ProcessVersion::V2))
                .unwrap()
                .is_none()
        );
        // ルックテーブルのないプロファイルでは飛ばす。
        let mut s = DevelopSettings::default();
        s.profile.camera = Some("cam".into());
        let res = ProfileResources {
            camera: Some(Resolved {
                data: Arc::new(DcpProfile::default()),
                data_ref: DataRef::new("dcp:cam", ""),
            }),
            ..Default::default()
        };
        assert!(
            ProfileLookTableStage
                .params(&s, &ctx_with(&res, ProcessVersion::V2))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn look_table_shifts_hue_and_keeps_neutrals() {
        let n = 6 * 2;
        let table = HsvTable::new([6, 2, 1], false, vec![[30.0, 1.0, 1.0]; n]).unwrap();
        let profile = DcpProfile {
            look_table: Some(table),
            ..Default::default()
        };
        let res = ProfileResources {
            camera: Some(Resolved {
                data: Arc::new(profile),
                data_ref: DataRef::new("dcp:cam", ""),
            }),
            ..Default::default()
        };
        let mut s = DevelopSettings::default();
        s.profile.camera = Some("cam".into());
        let ctx = ctx_with(&res, ProcessVersion::V2);
        let p = ProfileLookTableStage.params(&s, &ctx).unwrap().unwrap();
        let p = p.get::<LookTableParams>().unwrap().clone();
        // 中立色は変わらない。
        let out = p.apply([0.3; 3]);
        for v in out {
            assert!((v - 0.3).abs() < 1e-5, "{out:?}");
        }
        // 色は変わる（明度は保つ）。
        let c = [0.4, 0.2, 0.1];
        let out = p.apply(c);
        assert!(
            (out[0] - c[0]).abs() + (out[1] - c[1]).abs() > 0.01,
            "{out:?}"
        );
        // RAW 以外では飛ばす。
        let ctx = ctx.with_input_kind(InputKind::DisplayReferred);
        assert!(ProfileLookTableStage.params(&s, &ctx).unwrap().is_none());
    }
}
