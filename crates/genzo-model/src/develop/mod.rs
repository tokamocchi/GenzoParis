//! 現像設定（docs/04_architecture.md の 2.5 節）。
//!
//! 1 つのバリアント（仮想コピーを含む）の現像設定 [`DevelopSettings`] と、その JSON への
//! 保存・読み込み（スキーマのマイグレーションを含む）、検証、ハッシュを扱う。
//! カタログには [`DevelopSettings::to_json`] の JSON を 1 列に保存する（`variant.develop_json`）。

mod hash;
mod masks;
pub mod migrate;
mod params;
mod validate;

use serde::{Deserialize, Serialize};

pub use masks::{
    BrushStroke, ColorSample, LocalAdjustment, LocalDelta, MAX_BRUSH_STROKES, MAX_COLOR_SAMPLES,
    MAX_MASK_COMPONENTS, MAX_MASK_NAME_CHARS, MAX_MASKS, MAX_STROKE_POINTS, MAX_TOTAL_BRUSH_POINTS,
    MaskComponent, MaskMode, MaskShape,
};
pub use params::{
    AspectRatio, BlackWhite, ColorGrading, ColorMixer, ColorParams, CropRect, CurvePoint, DataRef,
    DetailParams, EffectsParams, Geometry, GradingWheel, Grain, HighlightMethod, HighlightRecovery,
    HslAdjustment, HueBand, HueBands, LensCorrection, MAX_ASPECT_TERM, MAX_CURVE_POINTS,
    MAX_PROFILE_ID_CHARS, MIN_CROP_SIZE, NoiseReduction, ParametricCurve, PostCropVignette,
    PresenceParams, ProfileSettings, RenderDeps, Sharpening, ToneCurve, ToneParams, Transform,
    ValueRange, VignetteCenter, WbPreset, WhiteBalance, is_valid_profile_id, ranges,
};
pub use validate::{FieldIssue, IssueKind, ValidationError};

pub(crate) use hash::sha256;

use crate::{CURRENT_PROCESS_VERSION, CURRENT_SCHEMA_VERSION};

/// 1 つのバリアント（仮想コピーを含む）の現像設定（04 の 2.5 節）。
///
/// - JSON に欠けている項目は既定値（何もしない値）で補う（`#[serde(default)]`）。
///   ただし [`from_json`](Self::from_json) では `schema_version` は必須。
/// - 知らない項目は無視する（同じスキーマのバージョンの中では項目を消さない運用とする）。
/// - パイプラインとハッシュは、[`normalized`](Self::normalized) で丸めた設定を使う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevelopSettings {
    /// スキーマの形（項目の追加・名前の変更）のバージョン。
    pub schema_version: u32,
    /// 処理アルゴリズムのバージョン。値が同じなら、アプリを更新しても結果は変わらない（IQ-08）。
    pub process_version: u32,
    /// カメラプロファイル・ルック（DEV-17。v1。既定は標準でルックなし）。
    pub profile: ProfileSettings,
    /// ホワイトバランス（DEV-03）。
    pub white_balance: WhiteBalance,
    /// ハイライト復元（DEV-06。v1。既定は復元しない（白で切りそろえる））。
    pub highlight_recovery: HighlightRecovery,
    /// 露光量（EV。-5〜+5。DEV-04）。
    pub exposure_ev: f32,
    /// コントラスト（-100〜+100。DEV-04）。
    pub contrast: f32,
    /// ハイライト・シャドウ・白・黒（DEV-05）。
    pub tone: ToneParams,
    /// テクスチャ・明瞭度・かすみの除去（DEV-11。v1。既定は 0）。
    pub presence: PresenceParams,
    /// トーンカーブ（DEV-07）。
    pub tone_curve: ToneCurve,
    /// 彩度・自然な彩度（DEV-08）・カラーミキサー（DEV-09）・カラーグレーディング（DEV-10）・白黒（DEV-25）。
    pub color: ColorParams,
    /// 切り抜き・角度補正（DEV-15）。
    pub geometry: Geometry,
    /// レンズ補正（DEV-14。v1。既定は無効）。
    pub lens: LensCorrection,
    /// 変形（遠近補正。DEV-16。既定は何もしない）。
    pub transform: Transform,
    /// シャープ・ノイズ軽減（DEV-12・DEV-13。v1。既定は 0）。
    pub detail: DetailParams,
    /// 効果（切り抜き後の周辺光量補正・粒子。DEV-24。v1。既定は 0）。
    pub effects: EffectsParams,
    /// ローカル補正（DEV-18。v1。既定は空）。
    pub masks: Vec<LocalAdjustment>,
    /// 現像結果に影響する外部データの識別子と内容のハッシュ（レビュー R-08）。
    pub render_deps: RenderDeps,
}

impl Default for DevelopSettings {
    /// 何も調整していない設定。処理バージョンは現在の値（新しく現像する写真用）。
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            process_version: CURRENT_PROCESS_VERSION,
            profile: ProfileSettings::default(),
            white_balance: WhiteBalance::default(),
            highlight_recovery: HighlightRecovery::default(),
            exposure_ev: 0.0,
            contrast: 0.0,
            tone: ToneParams::default(),
            presence: PresenceParams::default(),
            tone_curve: ToneCurve::default(),
            color: ColorParams::default(),
            geometry: Geometry::default(),
            lens: LensCorrection::default(),
            transform: Transform::default(),
            detail: DetailParams::default(),
            effects: EffectsParams::default(),
            masks: Vec::new(),
            render_deps: RenderDeps::default(),
        }
    }
}

/// 現像設定の読み込み・保存のエラー。
#[derive(Debug, thiserror::Error)]
pub enum DevelopError {
    /// JSON として解析できない、または型が合わない。
    #[error("現像設定の JSON を解析できません: {0}")]
    Json(#[from] serde_json::Error),
    /// JSON の最上位がオブジェクトではない。
    #[error("現像設定の JSON の最上位がオブジェクトではありません")]
    NotAnObject,
    /// `schema_version` がない。
    #[error("現像設定に schema_version がありません")]
    MissingSchemaVersion,
    /// `schema_version` が正の整数ではない。
    #[error("現像設定の schema_version が不正です: {0}")]
    InvalidSchemaVersion(String),
    /// このアプリより新しいスキーマで保存されている。
    #[error(
        "現像設定の schema_version {found} はこのバージョンのアプリでは読めません（対応しているのは {supported} まで）"
    )]
    FutureSchemaVersion {
        /// JSON の schema_version。
        found: u64,
        /// このアプリが対応している最新の schema_version。
        supported: u32,
    },
    /// このアプリが知らない処理バージョン。
    #[error(
        "現像設定の process_version {found} には対応していません（対応しているのは 1〜{supported}）"
    )]
    UnsupportedProcessVersion {
        /// JSON の process_version。
        found: u32,
        /// このアプリが対応している最新の process_version。
        supported: u32,
    },
    /// マイグレーション関数が失敗した。
    #[error("現像設定を schema_version {from} から {to} へ移行できません: {message}")]
    Migration {
        /// 移行元のバージョン。
        from: u32,
        /// 移行先のバージョン。
        to: u32,
        /// 失敗の理由。
        message: String,
    },
    /// 値の検証に失敗した。
    #[error(transparent)]
    Invalid(#[from] ValidationError),
}

impl DevelopSettings {
    /// JSON から読み込む。
    ///
    /// `schema_version` を見てマイグレーション関数を順に適用し、現在のスキーマに変換して
    /// から型に読み込む。未来の `schema_version`、および対応していない `process_version`
    /// （0、または現在の値より大きい）はエラーにする。
    ///
    /// 値の範囲は検証しない（保存済みの写真を開けなくしないため）。必要に応じて
    /// [`validate`](Self::validate) や [`normalized`](Self::normalized) を使う。
    pub fn from_json(json: &str) -> Result<Self, DevelopError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        let value = migrate::migrate_to_current(value)?;
        let settings: DevelopSettings = serde_json::from_value(value)?;
        if settings.process_version == 0 || settings.process_version > CURRENT_PROCESS_VERSION {
            return Err(DevelopError::UnsupportedProcessVersion {
                found: settings.process_version,
                supported: CURRENT_PROCESS_VERSION,
            });
        }
        Ok(settings)
    }

    /// JSON に書き出す（カタログへの保存用）。
    ///
    /// 先に [`validate`](Self::validate) を行い、不正な値（NaN は JSON で `null` になり
    /// 読み戻せなくなる）を保存しないようにする。範囲外の値を保存したい場合は、先に
    /// [`normalized`](Self::normalized) で丸める。
    pub fn to_json(&self) -> Result<String, DevelopError> {
        self.validate()?;
        Ok(serde_json::to_string(self)?)
    }

    /// 何も調整していない状態か。
    ///
    /// バージョンと外部データの参照（render_deps）、縦横比の指定（切り抜き枠の制約で、
    /// 結果には影響しない）は見ない。WB は「撮影時の値」なら調整なしとみなす。
    pub fn is_neutral(&self) -> bool {
        let s = self.normalized();
        s.white_balance == WhiteBalance::AsShot
            && s.profile.is_neutral()
            && s.highlight_recovery.is_disabled()
            && s.exposure_ev == 0.0
            && s.contrast == 0.0
            && s.tone == ToneParams::default()
            && s.presence.is_neutral()
            && s.tone_curve.is_identity()
            && s.color.is_neutral()
            && s.geometry.crop.is_full()
            && s.geometry.angle_deg == 0.0
            && s.lens.is_disabled()
            && s.transform.is_identity()
            && s.detail.is_neutral()
            && s.effects.is_neutral()
            && s.masks.iter().all(LocalAdjustment::is_neutral)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DevelopSettings {
        let mut s = DevelopSettings {
            white_balance: WhiteBalance::Custom {
                temperature_k: 4800.0,
                tint: 5.0,
            },
            exposure_ev: 0.7,
            contrast: 15.0,
            ..Default::default()
        };
        s.tone.highlights = -40.0;
        s.tone.shadows = 25.0;
        s.tone_curve.luma = vec![
            CurvePoint::new(0.0, 0.0),
            CurvePoint::new(0.25, 0.2),
            CurvePoint::new(1.0, 1.0),
        ];
        s.color.vibrance = 10.0;
        s.geometry.angle_deg = -1.25;
        s.geometry.aspect = Some(AspectRatio::Ratio { w: 3, h: 2 });
        s.render_deps.camera_profile = DataRef::new("sony-ilce-7m4", "ab".repeat(32));
        s.render_deps.raw_decoder = "libraw-0.21.2".to_owned();
        s
    }

    #[test]
    fn json_round_trip() {
        let s = sample();
        let json = s.to_json().unwrap();
        let back = DevelopSettings::from_json(&json).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.develop_hash(), s.develop_hash());
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s = DevelopSettings::from_json(
            r#"{"schema_version": 2, "process_version": 1, "exposure_ev": 1.5, "tone": {"shadows": 20}}"#,
        )
        .unwrap();
        assert_eq!(s.exposure_ev, 1.5);
        assert_eq!(s.tone.shadows, 20.0);
        assert_eq!(s.tone.highlights, 0.0);
        assert_eq!(s.white_balance, WhiteBalance::AsShot);
        assert!(s.geometry.crop.is_full());
        assert_eq!(s.lens, LensCorrection::default());
    }

    #[test]
    fn missing_process_version_uses_the_schema_default_not_current() {
        // v1 のスキーマの JSON は処理バージョン 1（MVP の写真の結果を変えない。IQ-08）。
        let s = DevelopSettings::from_json(r#"{"schema_version": 1}"#).unwrap();
        assert_eq!(s.process_version, 1);
        assert_eq!(
            s,
            DevelopSettings {
                process_version: 1,
                ..Default::default()
            }
        );
        let s = DevelopSettings::from_json(r#"{"schema_version": 2}"#).unwrap();
        assert_eq!(s.process_version, 2);
        assert_eq!(s, DevelopSettings::default());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let s =
            DevelopSettings::from_json(r#"{"schema_version": 2, "future_option": true}"#).unwrap();
        assert_eq!(s, DevelopSettings::default());
    }

    #[test]
    fn current_versions() {
        // 新しく現像する写真は処理バージョン 2（v1 の機能のステージ。決定事項ログ No.120）。
        let s = DevelopSettings::default();
        assert_eq!(s.schema_version, 3);
        assert_eq!(s.process_version, 2);
        // 版 1 の設定も読める（既存の写真）。
        let v1 =
            DevelopSettings::from_json(r#"{"schema_version": 2, "process_version": 1}"#).unwrap();
        assert_eq!(v1.process_version, 1);
        v1.validate().unwrap();
    }

    /// 変形（DEV-16）はどの項目でも 0（拡大・縮小は 100）でなければ無調整ではない。範囲の外は丸める。
    #[test]
    fn transform_values_are_validated_normalized_and_not_neutral() {
        assert!(DevelopSettings::default().transform.is_identity());
        for f in [
            |t: &mut Transform| t.vertical = -15.0,
            |t: &mut Transform| t.horizontal = 3.0,
            |t: &mut Transform| t.aspect = 10.0,
            |t: &mut Transform| t.scale = 120.0,
            |t: &mut Transform| t.offset_x = -5.0,
            |t: &mut Transform| t.offset_y = 1.0,
        ] {
            let mut s = DevelopSettings::default();
            f(&mut s.transform);
            assert!(!s.transform.is_identity(), "{:?}", s.transform);
            assert!(!s.is_neutral(), "{:?}", s.transform);
            s.validate().unwrap();
        }
        let mut s = DevelopSettings::default();
        s.transform.vertical = 150.0;
        s.transform.scale = 10.0;
        s.transform.offset_y = f32::NAN;
        let err = s.validate().unwrap_err();
        assert!(err.has_field("transform.vertical"));
        assert!(err.has_field("transform.scale"));
        assert!(err.has_field("transform.offset_y"));
        let n = s.normalized();
        assert_eq!(n.transform.vertical, 100.0);
        assert_eq!(n.transform.scale, 50.0);
        assert_eq!(n.transform.offset_y, 0.0);
        // JSON に変形がなければ既定値で読む（版 2 までの設定）。
        let s =
            DevelopSettings::from_json(r#"{"schema_version": 2, "process_version": 2}"#).unwrap();
        assert!(s.transform.is_identity());
    }

    #[test]
    fn manual_lens_corrections_are_not_neutral() {
        assert!(DevelopSettings::default().lens.is_disabled());
        for f in [
            |l: &mut LensCorrection| l.distortion = 10.0,
            |l: &mut LensCorrection| l.vignetting_amount = -5.0,
            |l: &mut LensCorrection| l.ca_red_cyan = 1.0,
            |l: &mut LensCorrection| l.ca_blue_yellow = -1.0,
            |l: &mut LensCorrection| l.profile_enabled = true,
            |l: &mut LensCorrection| l.remove_chromatic_aberration = true,
        ] {
            let mut s = DevelopSettings::default();
            f(&mut s.lens);
            assert!(!s.is_neutral(), "{:?}", s.lens);
        }
        // 中点だけを変えても（量が 0 なので）補正はない。
        let mut s = DevelopSettings::default();
        s.lens.vignetting_midpoint = 10.0;
        assert!(s.lens.is_disabled() && s.is_neutral());
    }

    #[test]
    fn lens_values_are_validated_and_normalized() {
        let mut s = DevelopSettings::default();
        s.lens.distortion = 150.0;
        s.lens.vignetting_midpoint = f32::NAN;
        s.lens.ca_blue_yellow = -101.0;
        let err = s.validate().unwrap_err();
        assert!(err.has_field("lens.distortion"));
        assert!(err.has_field("lens.vignetting_midpoint"));
        assert!(err.has_field("lens.ca_blue_yellow"));
        let n = s.normalized();
        assert_eq!(n.lens.distortion, 100.0);
        assert_eq!(n.lens.vignetting_midpoint, 50.0);
        assert_eq!(n.lens.ca_blue_yellow, -100.0);
        n.validate().unwrap();
    }

    #[test]
    fn highlight_recovery_and_noise_reduction_defaults_and_json() {
        // 既定は復元しない・ノイズ軽減の量 0（処理バージョン 1 と同じ結果。決定事項ログ No.126・127）。
        let s = DevelopSettings::default();
        assert_eq!(s.highlight_recovery.method, HighlightMethod::Clip);
        assert_eq!(s.highlight_recovery.amount, 100.0);
        assert!(s.highlight_recovery.is_disabled());
        let nr = s.detail.noise_reduction;
        assert_eq!(
            (nr.luminance, nr.luminance_detail, nr.luminance_contrast),
            (0.0, 50.0, 0.0)
        );
        assert_eq!((nr.color, nr.color_detail), (0.0, 50.0));
        assert!(nr.is_disabled());
        // JSON の形。
        let hr = HighlightRecovery {
            method: HighlightMethod::Reconstruct,
            amount: 80.0,
        };
        assert_eq!(
            serde_json::to_string(&hr).unwrap(),
            r#"{"method":"reconstruct","amount":80.0}"#
        );
        // 項目のない JSON（レンズ補正のパッケージまでのスキーマの版 2）は既定値で補う。
        let s = DevelopSettings::from_json(
            r#"{"schema_version": 2, "detail": {"noise_reduction": {"luminance": 30}}, "highlight_recovery": {"method": "neutral"}}"#,
        )
        .unwrap();
        assert_eq!(s.detail.noise_reduction.luminance, 30.0);
        assert_eq!(s.detail.noise_reduction.luminance_detail, 50.0);
        assert_eq!(s.detail.noise_reduction.color_detail, 50.0);
        assert_eq!(s.highlight_recovery.method, HighlightMethod::Neutral);
        assert_eq!(s.highlight_recovery.amount, 100.0);
        assert!(!s.is_neutral());
        // 量が 0 なら復元しない（切りそろえと同じ）。
        let z = DevelopSettings {
            highlight_recovery: HighlightRecovery {
                method: HighlightMethod::Reconstruct,
                amount: 0.0,
            },
            ..Default::default()
        };
        assert!(z.highlight_recovery.is_disabled() && z.is_neutral());
        // ノイズ軽減のディテール・コントラストだけを変えても（量が 0 なので）何もしない。
        let mut d = DevelopSettings::default();
        d.detail.noise_reduction.luminance_detail = 10.0;
        d.detail.noise_reduction.luminance_contrast = 80.0;
        d.detail.noise_reduction.color_detail = 90.0;
        assert!(d.is_neutral());
    }

    #[test]
    fn highlight_and_noise_values_are_validated_and_normalized() {
        let mut s = DevelopSettings::default();
        s.highlight_recovery.amount = 150.0;
        s.detail.noise_reduction.luminance_detail = f32::NAN;
        s.detail.noise_reduction.luminance_contrast = -1.0;
        s.detail.noise_reduction.color_detail = 101.0;
        let err = s.validate().unwrap_err();
        for f in [
            "highlight_recovery.amount",
            "detail.noise_reduction.luminance_detail",
            "detail.noise_reduction.luminance_contrast",
            "detail.noise_reduction.color_detail",
        ] {
            assert!(err.has_field(f), "{f}: {err}");
        }
        let n = s.normalized();
        assert_eq!(n.highlight_recovery.amount, 100.0);
        assert_eq!(n.detail.noise_reduction.luminance_detail, 50.0);
        assert_eq!(n.detail.noise_reduction.luminance_contrast, 0.0);
        assert_eq!(n.detail.noise_reduction.color_detail, 100.0);
        n.validate().unwrap();
        // 量の NaN は既定値（100）に丸める。
        let mut h = DevelopSettings::default();
        h.highlight_recovery.amount = f32::NAN;
        assert_eq!(h.normalized().highlight_recovery.amount, 100.0);
        // 未知の方式は JSON の読み込みでエラー。
        assert!(matches!(
            DevelopSettings::from_json(
                r#"{"schema_version": 2, "highlight_recovery": {"method": "magic"}}"#
            ),
            Err(DevelopError::Json(_))
        ));
    }

    #[test]
    fn color_mixer_grading_and_black_white_defaults_and_json() {
        // 既定は何もしない（カラー・ミキサー 0・グレーディング 0。処理バージョン 1 と同じ結果）。
        let s = DevelopSettings::default();
        assert!(s.color.is_neutral());
        assert!(!s.color.black_white.enabled);
        assert_eq!(s.color.grading.blending, 50.0);
        assert_eq!(s.color.grading.balance, 0.0);
        assert!(s.color.mixer.is_neutral() && s.color.grading.is_neutral());
        // 帯の名前と順。
        let keys: Vec<_> = HueBand::ALL.iter().map(|b| b.key()).collect();
        assert_eq!(
            keys,
            [
                "red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta"
            ]
        );
        for b in HueBand::ALL {
            assert_eq!(HueBand::from_key(b.key()), Some(b));
            assert_eq!(
                serde_json::to_string(&b).unwrap(),
                format!("\"{}\"", b.key())
            );
        }
        assert_eq!(HueBand::from_key("cyan"), None);
        // HueBands の値の順と取り出し。
        let mut m = HueBands::<f32>::from_values([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        assert_eq!(m.get(HueBand::Aqua), 5.0);
        *m.get_mut(HueBand::Magenta) = -1.0;
        assert_eq!(m.values()[7], -1.0);
        assert_eq!(m.magenta, -1.0);
        // JSON の形（帯の名前のオブジェクト）。項目のない JSON（レンズ補正・ハイライト復元のパッケージまでの
        // スキーマの版 2）は既定値で補う。
        let s = DevelopSettings::from_json(
            r#"{"schema_version": 2, "color": {"saturation": 10,
                "mixer": {"blue": {"luminance": -30}},
                "grading": {"shadows": {"hue": 220, "saturation": 25}, "balance": -10},
                "black_white": {"mix": {"red": 15}}}}"#,
        )
        .unwrap();
        assert_eq!(s.color.saturation, 10.0);
        assert_eq!(s.color.mixer.blue.luminance, -30.0);
        assert_eq!(s.color.mixer.blue.hue, 0.0);
        assert!(s.color.mixer.red.is_zero());
        assert_eq!(s.color.grading.shadows.hue, 220.0);
        assert_eq!(s.color.grading.shadows.saturation, 25.0);
        assert_eq!(s.color.grading.blending, 50.0);
        assert_eq!(s.color.grading.balance, -10.0);
        assert!(!s.color.black_white.enabled);
        assert_eq!(s.color.black_white.mix.red, 15.0);
        assert!(!s.is_neutral());
        let back = DevelopSettings::from_json(&s.to_json().unwrap()).unwrap();
        assert_eq!(back, s);
        let json = serde_json::to_value(s.color).unwrap();
        assert_eq!(json["mixer"]["blue"]["luminance"], -30.0);
        assert_eq!(json["black_white"]["mix"]["red"], 15.0);
        // スキーマの版 1 の JSON（彩度だけ）も読める。
        let v1 = DevelopSettings::from_json(r#"{"schema_version": 1, "color": {"vibrance": 5}}"#)
            .unwrap();
        assert_eq!(v1.color.vibrance, 5.0);
        assert!(v1.color.mixer.is_neutral() && v1.color.grading.is_neutral());
        assert!(!v1.color.black_white.enabled);
    }

    #[test]
    fn color_neutral_detection_ignores_values_that_have_no_effect() {
        // 強さが 0 の色相・ブレンド・バランス、カラーのときの白黒ミキサーは効かない。
        let mut s = DevelopSettings::default();
        s.color.grading.shadows.hue = 200.0;
        s.color.grading.blending = 80.0;
        s.color.grading.balance = -50.0;
        s.color.black_white.mix.blue = 40.0;
        assert!(s.color.is_neutral() && s.is_neutral());
        for f in [
            |c: &mut ColorParams| c.mixer.orange.hue = 5.0,
            |c: &mut ColorParams| c.mixer.green.saturation = -5.0,
            |c: &mut ColorParams| c.mixer.magenta.luminance = 1.0,
            |c: &mut ColorParams| c.grading.global.saturation = 1.0,
            |c: &mut ColorParams| c.grading.highlights.luminance = -1.0,
            |c: &mut ColorParams| c.black_white.enabled = true,
        ] {
            let mut t = DevelopSettings::default();
            f(&mut t.color);
            assert!(!t.color.is_neutral() && !t.is_neutral(), "{:?}", t.color);
        }
    }

    #[test]
    fn color_values_are_validated_and_normalized() {
        let mut s = DevelopSettings::default();
        s.color.mixer.red.hue = 101.0;
        s.color.mixer.purple.luminance = f32::NAN;
        s.color.grading.midtones.hue = 361.0;
        s.color.grading.highlights.saturation = -1.0;
        s.color.grading.global.luminance = 150.0;
        s.color.grading.blending = f32::NAN;
        s.color.grading.balance = -101.0;
        s.color.black_white.mix.aqua = 200.0;
        let err = s.validate().unwrap_err();
        for f in [
            "color.mixer.red.hue",
            "color.mixer.purple.luminance",
            "color.grading.midtones.hue",
            "color.grading.highlights.saturation",
            "color.grading.global.luminance",
            "color.grading.blending",
            "color.grading.balance",
            "color.black_white.mix.aqua",
        ] {
            assert!(err.has_field(f), "{f}: {err}");
        }
        let n = s.normalized();
        assert_eq!(n.color.mixer.red.hue, 100.0);
        assert_eq!(n.color.mixer.purple.luminance, 0.0);
        assert_eq!(n.color.grading.midtones.hue, 360.0);
        assert_eq!(n.color.grading.highlights.saturation, 0.0);
        assert_eq!(n.color.grading.global.luminance, 100.0);
        // ブレンドの NaN は既定値（50）に丸める。
        assert_eq!(n.color.grading.blending, 50.0);
        assert_eq!(n.color.grading.balance, -100.0);
        assert_eq!(n.color.black_white.mix.aqua, 100.0);
        n.validate().unwrap();
    }

    #[test]
    fn presence_sharpening_and_effects_defaults_and_json() {
        // 既定は何もしない（処理バージョン 1 と同じ結果）。
        let s = DevelopSettings::default();
        assert!(s.presence.is_neutral() && s.effects.is_neutral() && s.detail.is_neutral());
        assert_eq!(s.detail.sharpening.radius, 1.0);
        assert_eq!(s.detail.sharpening.detail, 25.0);
        assert_eq!(s.effects.vignette.midpoint, 50.0);
        assert_eq!(s.effects.vignette.feather, 50.0);
        assert_eq!(s.effects.grain.size, 25.0);
        assert_eq!(s.effects.grain.roughness, 50.0);
        assert!(s.is_neutral());
        // 項目のない JSON（カラーミキサーのパッケージまでのスキーマの版 2）は既定値で補う。
        let s = DevelopSettings::from_json(
            r#"{"schema_version": 2, "presence": {"clarity": 35},
                "detail": {"sharpening": {"amount": 60, "masking": 40}},
                "effects": {"vignette": {"amount": -25, "roundness": 30}, "grain": {"amount": 20}}}"#,
        )
        .unwrap();
        assert_eq!(s.presence.clarity, 35.0);
        assert_eq!((s.presence.texture, s.presence.dehaze), (0.0, 0.0));
        assert_eq!(s.detail.sharpening.amount, 60.0);
        assert_eq!(s.detail.sharpening.detail, 25.0);
        assert_eq!(s.detail.sharpening.masking, 40.0);
        assert_eq!(s.effects.vignette.amount, -25.0);
        assert_eq!(s.effects.vignette.roundness, 30.0);
        assert_eq!(s.effects.vignette.midpoint, 50.0);
        assert_eq!(s.effects.grain.amount, 20.0);
        assert_eq!(s.effects.grain.size, 25.0);
        assert!(!s.is_neutral());
        let back = DevelopSettings::from_json(&s.to_json().unwrap()).unwrap();
        assert_eq!(back, s);
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["presence"]["clarity"], 35.0);
        assert_eq!(json["effects"]["vignette"]["amount"], -25.0);
        assert_eq!(json["effects"]["grain"]["roughness"], 50.0);
        // finish-detail より前の版の JSON（シャープの旧既定のディテール 0 を保存していたもの）は、適用量が 0 なら
        // 新しい既定の 25 に読み替える（migrate.rs の read_changed_defaults_in_v2）。適用量があれば値を保つ。
        for schema in [1, 2] {
            let old = DevelopSettings::from_json(&format!(
                r#"{{"schema_version": {schema}, "process_version": {schema},
                    "detail": {{"sharpening": {{"amount": 0.0, "radius": 1.0, "detail": 0.0, "masking": 0.0}}}}}}"#
            ))
            .unwrap();
            assert_eq!(old.detail.sharpening, Sharpening::default());
            assert!(old.presence.is_neutral() && old.effects.is_neutral());
            assert!(old.is_neutral());
            // 既定の設定と同じになる（カタログでは「変更なし」になる）。
            let default = DevelopSettings {
                process_version: schema,
                ..DevelopSettings::default()
            };
            assert_eq!(old, default);
        }
        let v1 = DevelopSettings::from_json(
            r#"{"schema_version": 1, "detail": {"sharpening": {"amount": 40, "radius": 1, "detail": 0, "masking": 0}}}"#,
        )
        .unwrap();
        assert_eq!(v1.detail.sharpening.detail, 0.0);
        assert_eq!(v1.detail.sharpening.amount, 40.0);
        // 新しい版が保存した値（`presence` がある）は、適用量 0・ディテール 0 でも保つ（読み直して変わらない）。
        let mut s0 = DevelopSettings::default();
        s0.detail.sharpening.detail = 0.0;
        let back = DevelopSettings::from_json(&s0.to_json().unwrap()).unwrap();
        assert_eq!(back, s0);
        // 量が 0 なら、ほかの値（中点・丸み・大きさなど）は効かない。
        let mut q = DevelopSettings::default();
        q.effects.vignette.roundness = 80.0;
        q.effects.vignette.highlights = 50.0;
        q.effects.grain.size = 90.0;
        q.detail.sharpening.radius = 2.5;
        assert!(q.is_neutral());
        for f in [
            |s: &mut DevelopSettings| s.presence.texture = 1.0,
            |s: &mut DevelopSettings| s.presence.clarity = -1.0,
            |s: &mut DevelopSettings| s.presence.dehaze = 1.0,
            |s: &mut DevelopSettings| s.detail.sharpening.amount = 1.0,
            |s: &mut DevelopSettings| s.effects.vignette.amount = -1.0,
            |s: &mut DevelopSettings| s.effects.grain.amount = 1.0,
        ] {
            let mut t = DevelopSettings::default();
            f(&mut t);
            assert!(!t.is_neutral(), "{t:?}");
        }
    }

    #[test]
    fn presence_sharpening_and_effects_are_validated_and_normalized() {
        let mut s = DevelopSettings::default();
        s.presence.texture = 101.0;
        s.presence.clarity = f32::NAN;
        s.presence.dehaze = -150.0;
        s.detail.sharpening.detail = f32::NAN;
        s.effects.vignette.amount = -101.0;
        s.effects.vignette.midpoint = f32::NAN;
        s.effects.vignette.roundness = 200.0;
        s.effects.vignette.feather = -1.0;
        s.effects.vignette.highlights = 101.0;
        s.effects.grain.amount = 120.0;
        s.effects.grain.size = f32::NAN;
        s.effects.grain.roughness = -5.0;
        let err = s.validate().unwrap_err();
        for f in [
            "presence.texture",
            "presence.clarity",
            "presence.dehaze",
            "detail.sharpening.detail",
            "effects.vignette.amount",
            "effects.vignette.midpoint",
            "effects.vignette.roundness",
            "effects.vignette.feather",
            "effects.vignette.highlights",
            "effects.grain.amount",
            "effects.grain.size",
            "effects.grain.roughness",
        ] {
            assert!(err.has_field(f), "{f}: {err}");
        }
        let n = s.normalized();
        assert_eq!(n.presence.texture, 100.0);
        assert_eq!(n.presence.clarity, 0.0);
        assert_eq!(n.presence.dehaze, -100.0);
        // NaN は既定値に丸める。
        assert_eq!(n.detail.sharpening.detail, 25.0);
        assert_eq!(n.effects.vignette.amount, -100.0);
        assert_eq!(n.effects.vignette.midpoint, 50.0);
        assert_eq!(n.effects.vignette.roundness, 100.0);
        assert_eq!(n.effects.vignette.feather, 0.0);
        assert_eq!(n.effects.vignette.highlights, 100.0);
        assert_eq!(n.effects.grain.amount, 100.0);
        assert_eq!(n.effects.grain.size, 25.0);
        assert_eq!(n.effects.grain.roughness, 0.0);
        n.validate().unwrap();
    }

    #[test]
    fn vignette_center_json_validation_and_hash() {
        // ないとき（切り抜きの中心）は JSON に書かない（項目を足す前の設定とハッシュが同じ）。
        let s = DevelopSettings::default();
        assert_eq!(s.effects.vignette.center, None);
        let json = serde_json::to_value(&s).unwrap();
        assert!(
            json["effects"]["vignette"].get("center").is_none(),
            "{json}"
        );
        // 手動の中心は書いて読み直せる。
        let s = DevelopSettings::from_json(
            r#"{"schema_version": 2,
                "effects": {"vignette": {"amount": -30, "center": {"x": 0.25, "y": 0.6}}}}"#,
        )
        .unwrap();
        assert_eq!(
            s.effects.vignette.center,
            Some(VignetteCenter { x: 0.25, y: 0.6 })
        );
        let back = DevelopSettings::from_json(&s.to_json().unwrap()).unwrap();
        assert_eq!(back, s);
        s.validate().unwrap();
        // null は切り抜きの中心。
        let auto = DevelopSettings::from_json(
            r#"{"schema_version": 2, "effects": {"vignette": {"amount": -30, "center": null}}}"#,
        )
        .unwrap();
        assert_eq!(auto.effects.vignette.center, None);
        // 中心を手動にすると段階 C のハッシュだけが変わる。
        assert_ne!(auto.develop_hash(), s.develop_hash());
        for phase in [crate::Phase::A1, crate::Phase::B] {
            assert_eq!(auto.hash_for_phase(phase), s.hash_for_phase(phase));
        }
        // 範囲の外は端に寄せ、NaN は切り抜きの中心に戻す。
        let mut bad = s.clone();
        bad.effects.vignette.center = Some(VignetteCenter { x: -0.5, y: 1.5 });
        let err = bad.validate().unwrap_err();
        assert!(err.has_field("effects.vignette.center.x"), "{err}");
        assert!(err.has_field("effects.vignette.center.y"), "{err}");
        assert_eq!(
            bad.normalized().effects.vignette.center,
            Some(VignetteCenter { x: 0.0, y: 1.0 })
        );
        bad.effects.vignette.center = Some(VignetteCenter {
            x: f32::NAN,
            y: 0.5,
        });
        assert!(bad.validate().is_err());
        assert_eq!(bad.normalized().effects.vignette.center, None);
        bad.normalized().validate().unwrap();
    }

    #[test]
    fn profile_defaults_json_and_validation() {
        // 既定は標準のプロファイル・ルックなし・適用量 100（何もしない）。
        let s = DevelopSettings::default();
        assert_eq!(s.profile, ProfileSettings::default());
        assert_eq!(s.profile.look_amount, 100.0);
        assert!(s.profile.is_neutral() && s.is_neutral());
        // 項目のない JSON（効果のパッケージまでのスキーマの版 2）は既定値で補う。
        let s = DevelopSettings::from_json(
            r#"{"schema_version": 2, "profile": {"camera": "Sony ILCE-7M4 テスト", "look": "film"}}"#,
        )
        .unwrap();
        assert_eq!(s.profile.camera.as_deref(), Some("Sony ILCE-7M4 テスト"));
        assert_eq!(s.profile.look.as_deref(), Some("film"));
        assert_eq!(s.profile.look_amount, 100.0);
        assert!(!s.is_neutral());
        let back = DevelopSettings::from_json(&s.to_json().unwrap()).unwrap();
        assert_eq!(back, s);
        // render_deps のプロファイル・ルックは書いて読み直せる（ないときは書かない）。
        let mut d = DevelopSettings::default();
        d.render_deps.profile = Some(DataRef::new("dcp:a", "AB".repeat(32)));
        d.render_deps.look = Some(DataRef::new("cube:b", ""));
        let n = d.normalized();
        assert_eq!(
            n.render_deps.profile.as_ref().unwrap().sha256,
            "ab".repeat(32)
        );
        let back = DevelopSettings::from_json(&n.to_json().unwrap()).unwrap();
        assert_eq!(back.render_deps, n.render_deps);
        let json = serde_json::to_value(DevelopSettings::default()).unwrap();
        assert!(json["render_deps"].get("profile").is_none());
        assert!(json["render_deps"].get("look").is_none());
        // 適用量 0 のルックは効かない。
        let mut z = DevelopSettings::default();
        z.profile.look = Some("film".into());
        z.profile.look_amount = 0.0;
        assert!(!z.profile.look_is_active() && z.is_neutral());
        // 不正な ID・範囲外の適用量は検証で見つけ、丸めると指定なし・範囲の端になる。
        let mut bad = DevelopSettings::default();
        bad.profile.camera = Some(String::new());
        bad.profile.look = Some(" 前後に空白 ".into());
        bad.profile.look_amount = 150.0;
        let err = bad.validate().unwrap_err();
        for f in ["profile.camera", "profile.look", "profile.look_amount"] {
            assert!(err.has_field(f), "{f}: {err}");
        }
        let n = bad.normalized();
        assert_eq!(n.profile.camera, None);
        assert_eq!(n.profile.look, None);
        assert_eq!(n.profile.look_amount, 100.0);
        n.validate().unwrap();
        let mut long = DevelopSettings::default();
        long.profile.camera = Some("あ".repeat(MAX_PROFILE_ID_CHARS + 1));
        assert!(long.validate().is_err());
        long.profile.camera = Some("あ".repeat(MAX_PROFILE_ID_CHARS));
        long.validate().unwrap();
        assert!(!is_valid_profile_id("a\u{1}b"));
        let mut nan = DevelopSettings::default();
        nan.profile.look_amount = f32::NAN;
        assert_eq!(nan.normalized().profile.look_amount, 100.0);
        let mut bad_hash = DevelopSettings::default();
        bad_hash.render_deps.look = Some(DataRef::new("cube:x", "xyz"));
        assert!(
            bad_hash
                .validate()
                .unwrap_err()
                .has_field("render_deps.look.sha256")
        );
    }

    #[test]
    fn version_errors() {
        assert!(matches!(
            DevelopSettings::from_json(r#"{"exposure_ev": 1.0}"#),
            Err(DevelopError::MissingSchemaVersion)
        ));
        assert!(matches!(
            DevelopSettings::from_json(r#"{"schema_version": 4}"#),
            Err(DevelopError::FutureSchemaVersion {
                found: 4,
                supported: 3
            })
        ));
        assert!(matches!(
            DevelopSettings::from_json(r#"{"schema_version": 2, "process_version": 3}"#),
            Err(DevelopError::UnsupportedProcessVersion { found: 3, .. })
        ));
        assert!(matches!(
            DevelopSettings::from_json(r#"{"schema_version": 1, "process_version": 0}"#),
            Err(DevelopError::UnsupportedProcessVersion { found: 0, .. })
        ));
        assert!(matches!(
            DevelopSettings::from_json("not json"),
            Err(DevelopError::Json(_))
        ));
        assert!(matches!(
            DevelopSettings::from_json(r#"{"schema_version": 1, "exposure_ev": "high"}"#),
            Err(DevelopError::Json(_))
        ));
        let msg = DevelopSettings::from_json(r#"{"schema_version": 9}"#)
            .unwrap_err()
            .to_string();
        assert!(msg.contains("schema_version 9"), "{msg}");
    }

    #[test]
    fn to_json_rejects_invalid_values() {
        let s = DevelopSettings {
            exposure_ev: f32::NAN,
            ..Default::default()
        };
        let err = s.to_json().unwrap_err();
        match err {
            DevelopError::Invalid(v) => assert!(v.has_field("exposure_ev")),
            other => panic!("unexpected: {other:?}"),
        }
        // 丸めれば保存できる。
        s.normalized().to_json().unwrap();
    }

    #[test]
    fn from_json_does_not_reject_out_of_range_values() {
        let s = DevelopSettings::from_json(r#"{"schema_version": 2, "exposure_ev": 9.0}"#).unwrap();
        assert!(s.validate().is_err());
        assert_eq!(s.normalized().exposure_ev, 5.0);
    }

    #[test]
    fn neutral_detection() {
        assert!(DevelopSettings::default().is_neutral());
        let mut s = DevelopSettings::default();
        s.render_deps.raw_decoder = "libraw-0.21.2".to_owned();
        s.geometry.aspect = Some(AspectRatio::Original);
        s.tone_curve.luma = vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)];
        assert!(s.is_neutral());
        assert!(!sample().is_neutral());

        let wb = DevelopSettings {
            white_balance: WhiteBalance::Preset(WbPreset::Daylight),
            ..Default::default()
        };
        assert!(!wb.is_neutral());

        let mut crop = DevelopSettings::default();
        crop.geometry.crop.left = 0.1;
        assert!(!crop.is_neutral());

        let mut nr = DevelopSettings::default();
        nr.detail.noise_reduction.color = 25.0;
        assert!(!nr.is_neutral());

        let mut mask = DevelopSettings::default();
        mask.masks.push(LocalAdjustment {
            delta: LocalDelta {
                exposure_ev: 0.3,
                ..Default::default()
            },
            ..LocalAdjustment::new(MaskComponent::add(MaskShape::Linear {
                x0: 0.0,
                y0: 0.0,
                x1: 0.0,
                y1: 1.0,
            }))
        });
        assert!(!mask.is_neutral());
        mask.masks[0].enabled = false;
        assert!(mask.is_neutral());
    }
}

#[cfg(test)]
mod proptests {
    use proptest::prelude::*;

    use super::*;

    /// NaN・無限大・範囲外を含む f32。
    fn any_f32() -> impl Strategy<Value = f32> {
        prop_oneof![
            -200.0f32..200.0,
            Just(f32::NAN),
            Just(f32::INFINITY),
            Just(f32::NEG_INFINITY),
            Just(-0.0f32),
            any::<f32>(),
        ]
    }

    fn any_points() -> impl Strategy<Value = Vec<CurvePoint>> {
        prop::collection::vec(
            (any_f32(), any_f32()).prop_map(|(x, y)| CurvePoint::new(x, y)),
            0..8,
        )
    }

    prop_compose! {
        fn any_settings()(
            exposure in any_f32(),
            contrast in any_f32(),
            tone in prop::array::uniform4(any_f32()),
            sat in any_f32(),
            angle in any_f32(),
            crop in prop::array::uniform4(any_f32()),
            temp in any_f32(),
            tint in any_f32(),
            luma in any_points(),
            red in any_points(),
            pv in 0u32..5,
            extra in prop::array::uniform6(any_f32()),
        ) -> DevelopSettings {
            let mut s = DevelopSettings {
                process_version: pv,
                white_balance: WhiteBalance::Custom { temperature_k: temp, tint },
                exposure_ev: exposure,
                contrast,
                ..Default::default()
            };
            s.tone = ToneParams { highlights: tone[0], shadows: tone[1], whites: tone[2], blacks: tone[3] };
            s.color.saturation = sat;
            s.geometry.angle_deg = angle;
            s.geometry.crop = CropRect { left: crop[0], top: crop[1], right: crop[2], bottom: crop[3] };
            s.tone_curve.luma = luma;
            s.tone_curve.red = red;
            s.highlight_recovery = HighlightRecovery { method: HighlightMethod::Reconstruct, amount: extra[5] };
            s.detail.noise_reduction = NoiseReduction {
                luminance: extra[0],
                luminance_detail: extra[1],
                luminance_contrast: extra[2],
                color: extra[3],
                color_detail: extra[4],
            };
            s
        }
    }

    prop_compose! {
        /// 色（カラーミキサー・カラーグレーディング・白黒）だけを変えた設定（上の戦略に足すとテストのスタックが
        /// あふれるため分けた）。
        fn any_color_settings()(
            col in prop::array::uniform8(any_f32()),
            bw in any::<bool>(),
        ) -> DevelopSettings {
            let mut s = DevelopSettings::default();
            s.color.mixer.red.hue = col[0];
            s.color.mixer.aqua.saturation = col[1];
            s.color.mixer.blue.luminance = col[2];
            s.color.grading.shadows.hue = col[3];
            s.color.grading.highlights.saturation = col[4];
            s.color.grading.blending = col[5];
            s.color.grading.balance = col[6];
            s.color.black_white = BlackWhite { enabled: bw, mix: HueBands::from_values([col[7]; 8]) };
            s
        }
    }

    prop_compose! {
        /// テクスチャ・明瞭度・かすみの除去・シャープ・効果だけを変えた設定（上の戦略に足すとテストのスタックが
        /// あふれるため分けた）。
        fn any_detail_settings()(
            v in prop::array::uniform12(any_f32()),
        ) -> DevelopSettings {
            let mut s = DevelopSettings {
                presence: PresenceParams { texture: v[0], clarity: v[1], dehaze: v[2] },
                ..Default::default()
            };
            s.detail.sharpening = Sharpening { amount: v[3], radius: v[4], detail: v[5], masking: v[6] };
            s.effects.vignette.amount = v[7];
            s.effects.vignette.midpoint = v[8];
            s.effects.vignette.roundness = v[9];
            s.effects.grain.amount = v[10];
            s.effects.grain.size = v[11];
            s
        }
    }

    proptest! {
        #[test]
        fn normalized_detail_settings_are_valid_and_round_trip(s in any_detail_settings()) {
            let n = s.normalized();
            prop_assert!(n.validate().is_ok(), "{:?}", n.validate());
            prop_assert_eq!(n.normalized(), n.clone());
            let back = DevelopSettings::from_json(&n.to_json().unwrap()).unwrap();
            prop_assert_eq!(back.develop_hash(), n.develop_hash());
            prop_assert_eq!(back, n);
            prop_assert_eq!(s.develop_hash(), s.normalized().develop_hash());
        }

        #[test]
        fn normalized_settings_are_always_valid(s in any_settings()) {
            let n = s.normalized();
            prop_assert!(n.validate().is_ok(), "{:?}", n.validate());
            prop_assert_eq!(n.normalized(), n.clone());
        }

        #[test]
        fn normalized_settings_round_trip_through_json(s in any_settings()) {
            let n = s.normalized();
            let back = DevelopSettings::from_json(&n.to_json().unwrap()).unwrap();
            prop_assert_eq!(back.develop_hash(), n.develop_hash());
            prop_assert_eq!(back, n);
        }

        #[test]
        fn normalized_color_settings_are_valid_and_round_trip(s in any_color_settings()) {
            let n = s.normalized();
            prop_assert!(n.validate().is_ok(), "{:?}", n.validate());
            prop_assert_eq!(n.normalized(), n.clone());
            let back = DevelopSettings::from_json(&n.to_json().unwrap()).unwrap();
            prop_assert_eq!(back.develop_hash(), n.develop_hash());
            prop_assert_eq!(back, n);
            prop_assert_eq!(s.develop_hash(), s.normalized().develop_hash());
        }

        #[test]
        fn hash_depends_only_on_normalized_settings(s in any_settings()) {
            prop_assert_eq!(s.develop_hash(), s.normalized().develop_hash());
            for p in crate::Phase::ALL {
                prop_assert_eq!(s.hash_for_phase(p), s.normalized().hash_for_phase(p));
            }
        }
    }
}
