//! カメラプロファイル・ルック（DEV-17。docs/04_architecture.md の 2.1 節のステージ 8 と、段階 C のプロファイルの
//! ルックテーブル・独自のルック。処理バージョン 2）の受け渡し。
//!
//! # 受け渡し
//!
//! - 現像設定はプロファイル・ルックを ID（`profile.camera`・`profile.look`）で指す。ファイルを解析したもの
//!   （genzo-color の [`DcpProfile`]・[`CubeLut`]）は、エンジンに渡した [`ProfileResolver`]（genzo-api のプロファイルの
//!   ライブラリ、CLI のファイル）が ID から引く（[`crate::Engine::with_profile_resolver`]）。
//! - エンジンは描画のたびに [`ProfileResources::resolve`] で引き、
//!   - ステージ 8: [`crate::SensorOptions::camera_profile`] に入れる（[`crate::SensorPlan::new`] が使う。
//!     [`CameraProfilePlan`]）。
//!   - 段階 C: [`crate::StageContext::profiles`] に入れる（ステージのパラメータを求めるときに、ルックテーブル
//!     （`finish::look::ProfileLookTableStage`）・ルック（`finish::look::LookStage`）が使う）。
//!   - キャッシュのキー: 設定の `render_deps.profile`・`render_deps.look` を、引いたファイルの参照（ID と内容の
//!     SHA-256）に置き換えた設定（[`ProfileResources::apply_render_deps`]）で求める。ファイルの中身が変われば、A1
//!     （プロファイル）・段階 C（ルック）を作り直す。
//! - 見つからない（ライブラリから除いた・別の PC で開いた）ときは、標準の色（ルックなし）で描き、警告する
//!   （[`SensorWarning::CameraProfileMissing`]・[`SensorWarning::LookMissing`]）。
//! - 処理バージョン 1 の写真では引かない（値は無視して知らせる。`finish::reserved`）。
//!
//! # AI マスクのラスター（DEV-20）
//!
//! AI マスク（genzo-model の `MaskShape::Ai`）のラスターも同じ仕組みで引く（[`ProfileResolver::ai_mask`]。genzo-api は
//! カタログの `ai_mask` の表から引く）。設定が指すラスターの参照（ID と内容の SHA-256）ごとに
//! [`ProfileResources::ai_masks`] に入れ、段階 C のローカル補正（[`crate::local`]）が G 座標のラスターに写して使う。
//! 見つからなければ重み 0 として描き、警告する（[`SensorWarning::AiMaskMissing`]）。参照の SHA-256 は設定の JSON
//! （段階 C のハッシュ）に入るので、`render_deps` には足さない。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use genzo_color::{CubeLut, DcpProfile, HsvTable, Mat3F32};
use genzo_model::{DataRef, DevelopSettings};

use crate::error::Result;
use crate::sensor::SensorWarning;
use crate::version::ProcessVersion;

/// 引いたプロファイル・ルック（解析したものと、記録用の参照）。
#[derive(Debug)]
pub struct Resolved<T> {
    /// 解析したもの。
    pub data: Arc<T>,
    /// 参照（ID は `dcp:<ID>`・`cube:<ID>`、SHA-256 はファイルの内容）。
    pub data_ref: DataRef,
}

impl<T> Clone for Resolved<T> {
    fn clone(&self) -> Self {
        Self {
            data: Arc::clone(&self.data),
            data_ref: self.data_ref.clone(),
        }
    }
}

/// カメラプロファイルの参照の ID（`dcp:<プロファイルの ID>`）。
pub fn dcp_ref_id(id: &str) -> String {
    format!("dcp:{id}")
}

/// ルックの参照の ID（`cube:<ルックの ID>`）。
pub fn cube_ref_id(id: &str) -> String {
    format!("cube:{id}")
}

/// AI マスクのラスター（DEV-20。genzo-model の `MaskShape::Ai` が参照する、推論で作った重み）。
///
/// 値は 0〜255（0〜1 の重み）、行優先。G 座標のどの範囲を覆うかは設定の側（`bounds`）が持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiMaskRaster {
    /// 幅（画素）。
    pub width: u32,
    /// 高さ（画素）。
    pub height: u32,
    /// 値（`width × height`）。
    pub values: Vec<u8>,
}

impl AiMaskRaster {
    /// 作る（`values` の長さが `width × height` でなければ `None`）。
    pub fn new(width: u32, height: u32, values: Vec<u8>) -> Option<Self> {
        (width > 0 && height > 0 && values.len() == width as usize * height as usize).then_some(
            Self {
                width,
                height,
                values,
            },
        )
    }

    /// 内容のアドレス（幅・高さと値の SHA-256 の小文字の 16 進数。genzo-model の `ai_mask_sha256`。`DataRef::sha256` に
    /// 入れる値）。
    pub fn sha256_hex(&self) -> String {
        genzo_model::ai_mask_sha256(self.width, self.height, &self.values)
    }
}

/// 現像設定の ID からプロファイル・ルック（と AI マスクのラスター）を引く（genzo-api のプロファイルのライブラリ・
/// カタログなどが実装する）。
///
/// 呼び出しは描画のたび（キャッシュのキーを求めるたび）なので、解析したものは実装の側で持っておくこと。
pub trait ProfileResolver: Send + Sync + fmt::Debug {
    /// ID `id` のカメラプロファイル（DCP）。ない・読めなければ `None`。
    fn camera_profile(&self, id: &str) -> Option<Resolved<DcpProfile>>;
    /// ID `id` のルック（3D LUT）。ない・読めなければ `None`。
    fn look(&self, id: &str) -> Option<Resolved<CubeLut>>;
    /// 参照 `data_ref`（ID は `ai-mask:<種類>`、SHA-256 は内容）の AI マスクのラスター（DEV-20。モジュールの doc）。
    /// ない・読めなければ `None`（既定）。
    fn ai_mask(&self, data_ref: &DataRef) -> Option<Arc<AiMaskRaster>> {
        let _ = data_ref;
        None
    }
}

/// メモリの中のプロファイル・ルック（CLI のファイル・テスト用の [`ProfileResolver`]）。
#[derive(Debug, Default)]
pub struct MemoryProfiles {
    cameras: HashMap<String, Resolved<DcpProfile>>,
    looks: HashMap<String, Resolved<CubeLut>>,
    ai_masks: HashMap<String, Arc<AiMaskRaster>>,
}

impl MemoryProfiles {
    /// 空。
    pub fn new() -> Self {
        Self::default()
    }

    /// カメラプロファイルを ID `id`・内容の SHA-256 `sha256` で加える。
    pub fn insert_camera(&mut self, id: &str, profile: DcpProfile, sha256: &str) {
        self.cameras.insert(
            id.to_owned(),
            Resolved {
                data: Arc::new(profile),
                data_ref: DataRef::new(dcp_ref_id(id), sha256),
            },
        );
    }

    /// ルックを ID `id`・内容の SHA-256 `sha256` で加える。
    pub fn insert_look(&mut self, id: &str, look: CubeLut, sha256: &str) {
        self.looks.insert(
            id.to_owned(),
            Resolved {
                data: Arc::new(look),
                data_ref: DataRef::new(cube_ref_id(id), sha256),
            },
        );
    }
}

impl MemoryProfiles {
    /// AI マスクのラスターを加える（参照は内容の SHA-256 で引く。ID は見ない）。SHA-256 を返す。
    pub fn insert_ai_mask(&mut self, raster: AiMaskRaster) -> String {
        let sha = raster.sha256_hex();
        self.ai_masks.insert(sha.clone(), Arc::new(raster));
        sha
    }
}

impl ProfileResolver for MemoryProfiles {
    fn camera_profile(&self, id: &str) -> Option<Resolved<DcpProfile>> {
        self.cameras.get(id).cloned()
    }
    fn look(&self, id: &str) -> Option<Resolved<CubeLut>> {
        self.looks.get(id).cloned()
    }
    fn ai_mask(&self, data_ref: &DataRef) -> Option<Arc<AiMaskRaster>> {
        self.ai_masks.get(&data_ref.sha256).cloned()
    }
}

/// 1 つの現像設定のために引いたプロファイル・ルック（[`ProfileResources::resolve`]）。
#[derive(Debug, Clone, Default)]
pub struct ProfileResources {
    /// カメラプロファイル（設定が指していて、引けたとき）。
    pub camera: Option<Resolved<DcpProfile>>,
    /// ルック（設定が指していて、適用量が 0 でなく、引けたとき）。
    pub look: Option<Resolved<CubeLut>>,
    /// 設定がカメラプロファイルを指しているのに、引けなかった。
    pub camera_missing: bool,
    /// 設定がルックを指しているのに、引けなかった。
    pub look_missing: bool,
    /// AI マスクのラスター（設定のマスクが指す参照の SHA-256 → ラスター。引けたものだけ。DEV-20）。
    pub ai_masks: HashMap<String, Arc<AiMaskRaster>>,
    /// 設定のマスクが指しているのに、引けなかった・範囲に写せなかった AI マスクのラスターの参照の SHA-256（重複なし。
    /// 設定の順）。
    pub ai_masks_missing: Vec<String>,
}

impl ProfileResources {
    /// 設定の ID から引く（処理バージョン 2 から。版 1・未知の版・解決する仕組みがないときは何も引かない。
    /// 仕組みがないときに設定が指していれば「見つからない」）。AI マスクのラスターも引く（描画のとき）。
    pub fn resolve(settings: &DevelopSettings, resolver: Option<&dyn ProfileResolver>) -> Self {
        Self::resolve_with(settings, resolver, true)
    }

    /// カメラプロファイル・ルックだけを引く（AI マスクのラスターは引かない）。キャッシュのキー・記録用の設定
    /// （[`crate::Engine::effective_settings`]）に使う: ラスターの参照は設定の JSON に入っていて `render_deps` には
    /// 入れないので、引く必要がなく、引く側（genzo-api のカタログ）のロックの中から呼ばれても困らない。
    pub fn resolve_profiles_only(
        settings: &DevelopSettings,
        resolver: Option<&dyn ProfileResolver>,
    ) -> Self {
        Self::resolve_with(settings, resolver, false)
    }

    fn resolve_with(
        settings: &DevelopSettings,
        resolver: Option<&dyn ProfileResolver>,
        with_ai_masks: bool,
    ) -> Self {
        let Ok(version) = ProcessVersion::from_u32(settings.process_version) else {
            return Self::default();
        };
        if !version.has_v1_features() {
            return Self::default();
        }
        let s = settings.normalized();
        let mut out = Self::default();
        if let Some(id) = &s.profile.camera {
            out.camera = resolver.and_then(|r| r.camera_profile(id));
            out.camera_missing = out.camera.is_none();
        }
        if s.profile.look_is_active()
            && let Some(id) = &s.profile.look
        {
            out.look = resolver.and_then(|r| r.look(id));
            out.look_missing = out.look.is_none();
        }
        if !with_ai_masks {
            return out;
        }
        for c in s.masks.iter().flat_map(|m| m.components.iter()) {
            let genzo_model::MaskShape::Ai { mask, bounds, .. } = &c.shape else {
                continue;
            };
            let sha = &mask.sha256;
            if out.ai_masks_missing.contains(sha) {
                continue;
            }
            let raster = match out.ai_masks.get(sha) {
                Some(r) => Some(r.clone()),
                None => resolver.and_then(|r| r.ai_mask(mask)),
            };
            match raster {
                // 引けても、範囲に写せない（読み込んだ設定の範囲が極端に細いなど）なら使えないとして知らせる
                // （重み 0 で黙って効かなくならないように）。
                Some(r) if crate::local::ai::grid_rows(r.width, *bounds).is_some() => {
                    out.ai_masks.insert(sha.clone(), r);
                }
                _ => {
                    out.ai_masks.remove(sha);
                    out.ai_masks_missing.push(sha.clone());
                }
            }
        }
        out
    }

    /// 参照 `data_ref` の AI マスクのラスター（引けていれば）。
    pub fn ai_mask(&self, data_ref: &DataRef) -> Option<&Arc<AiMaskRaster>> {
        self.ai_masks.get(&data_ref.sha256)
    }

    /// 設定の `render_deps.profile`・`render_deps.look` を、引いたものの参照にした設定（引けなかった・使わないなら
    /// `None`。キャッシュのキーと記録に使う）。
    pub fn apply_render_deps(&self, settings: &DevelopSettings) -> DevelopSettings {
        let mut s = settings.clone();
        s.render_deps.profile = self.camera.as_ref().map(|c| c.data_ref.clone());
        s.render_deps.look = self.look.as_ref().map(|l| l.data_ref.clone());
        s
    }

    /// 段階 C の警告（ルック・AI マスクのラスターが見つからない）。カメラプロファイルの警告はステージ 8
    /// （[`crate::SensorPlan::new`]）が出す。
    pub fn stage_c_warnings(&self) -> Vec<SensorWarning> {
        let mut out = Vec::new();
        if self.look_missing {
            out.push(SensorWarning::LookMissing);
        }
        if !self.ai_masks_missing.is_empty() {
            out.push(SensorWarning::AiMaskMissing);
        }
        out
    }
}

/// ステージ 8 のカメラプロファイルの計画（[`crate::SensorPlan::camera_profile`]。genzo-color の
/// [`DcpProfile::render`] の結果を f32 にしたもの。GPU 版にも同じ値を渡す）。
///
/// 色相彩度のマップがあれば、`camera_to_prophoto` → マップ（[`HsvTable::apply`]）→ `prophoto_to_working` の順に当てる。
/// なければステージ 8 は 1 つの行列（[`crate::SensorPlan::camera_to_working`]）で、今と同じ計算になる。
#[derive(Debug, Clone, PartialEq)]
pub struct CameraProfilePlan {
    /// WB 済みのカメラ RGB → リニア ProPhoto RGB。
    pub camera_to_prophoto: Mat3F32,
    /// リニア ProPhoto RGB → 作業色空間。
    pub prophoto_to_working: Mat3F32,
    /// 色相彩度のマップ（ない・何もしない表なら `None`）。
    pub hue_sat_map: Option<Arc<HsvTable>>,
    /// 白の相関色温度（K。表示用）。
    pub temperature_k: f64,
    /// 光源 1 の行列・マップの重み（表示用）。
    pub weight1: f64,
    /// ForwardMatrix を使ったか（表示用）。
    pub uses_forward_matrix: bool,
}

impl CameraProfilePlan {
    /// プロファイル `profile` を、カメラの中立色 `neutral`（WB の係数の逆数）で計算する（genzo-color の
    /// [`DcpProfile::render`]）。行列の f32 と、マップがないときの 1 つの行列（カメラ → 作業色空間）を返す。
    pub fn new(profile: &DcpProfile, neutral: [f64; 3]) -> Result<(Self, Mat3F32)> {
        let r = profile.render(neutral)?;
        let combined = r.camera_to_working().to_f32();
        Ok((
            Self {
                camera_to_prophoto: r.camera_to_prophoto.to_f32(),
                prophoto_to_working: r.prophoto_to_working.to_f32(),
                hue_sat_map: r.hue_sat_map.map(Arc::new),
                temperature_k: r.temperature_k,
                weight1: r.weight1,
                uses_forward_matrix: r.uses_forward_matrix,
            },
            combined,
        ))
    }

    /// 1 画素に当てる（マップがあるときのステージ 8 の CPU 版。`camera_to_prophoto` → マップ → `prophoto_to_working`）。
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let p = self.camera_to_prophoto.apply(c);
        let m = match &self.hue_sat_map {
            Some(map) => map.apply(p),
            None => p,
        };
        self.prophoto_to_working.apply(m)
    }
}

#[cfg(test)]
mod tests {
    use genzo_color::Mat3;

    use super::*;

    fn profile() -> DcpProfile {
        DcpProfile {
            name: Some("テスト".into()),
            calibration_illuminants: [Some(21), None],
            color_matrices: [
                Some(Mat3::from_rows([
                    [0.70, -0.20, -0.06],
                    [-0.50, 1.30, 0.23],
                    [-0.07, 0.13, 0.65],
                ])),
                None,
            ],
            ..Default::default()
        }
    }

    fn look() -> CubeLut {
        CubeLut::parse("LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n")
            .unwrap()
    }

    #[test]
    fn resolves_only_from_version_2_and_reports_missing_items() {
        let mut lib = MemoryProfiles::new();
        lib.insert_camera("cam", profile(), &"ab".repeat(32));
        lib.insert_look("film", look(), &"cd".repeat(32));
        let mut s = DevelopSettings::default();
        s.profile.camera = Some("cam".into());
        s.profile.look = Some("film".into());
        let r = ProfileResources::resolve(&s, Some(&lib));
        assert!(r.camera.is_some() && r.look.is_some());
        assert!(!r.camera_missing && !r.look_missing);
        assert!(r.stage_c_warnings().is_empty());
        let e = r.apply_render_deps(&s);
        assert_eq!(e.render_deps.profile.as_ref().unwrap().id, "dcp:cam");
        assert_eq!(e.render_deps.look.as_ref().unwrap().id, "cube:film");
        // 適用量 0 のルックは引かない。
        let mut z = s.clone();
        z.profile.look_amount = 0.0;
        let r = ProfileResources::resolve(&z, Some(&lib));
        assert!(r.look.is_none() && !r.look_missing);
        assert!(r.apply_render_deps(&z).render_deps.look.is_none());
        // 版 1 では引かない。
        let v1 = DevelopSettings {
            process_version: 1,
            ..s.clone()
        };
        let r = ProfileResources::resolve(&v1, Some(&lib));
        assert!(r.camera.is_none() && r.look.is_none() && !r.camera_missing);
        // 見つからない。
        let mut m = s.clone();
        m.profile.camera = Some("other".into());
        m.profile.look = Some("other".into());
        let r = ProfileResources::resolve(&m, Some(&lib));
        assert!(r.camera_missing && r.look_missing);
        assert_eq!(r.stage_c_warnings(), [SensorWarning::LookMissing]);
        let r = ProfileResources::resolve(&s, None);
        assert!(r.camera_missing && r.look_missing);
        // 何も指していなければ何もしない（render_deps も消す）。
        let mut d = DevelopSettings::default();
        d.render_deps.profile = Some(DataRef::new("dcp:stale", ""));
        let r = ProfileResources::resolve(&d, Some(&lib));
        assert!(r.apply_render_deps(&d).render_deps.profile.is_none());
    }

    #[test]
    fn plan_without_a_map_is_a_single_matrix() {
        let p = profile();
        let neutral = [0.6, 1.0, 0.8];
        let (plan, combined) = CameraProfilePlan::new(&p, neutral).unwrap();
        assert!(plan.hue_sat_map.is_none());
        let c = [0.3f32, 0.5, 0.2];
        let a = plan.apply(c);
        let b = combined.apply(c);
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 1e-5, "{a:?} {b:?}");
        }
        // 中立色は中立色のまま。
        let n = combined.apply([0.4; 3]);
        for v in n {
            assert!((v - 0.4).abs() < 1e-6, "{n:?}");
        }
    }
}
