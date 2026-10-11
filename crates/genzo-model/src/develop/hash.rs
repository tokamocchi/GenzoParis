//! 現像設定のハッシュ（キャッシュキーの `develop_hash`。docs/04_architecture.md の 4.1 節）。
//!
//! ハッシュは、丸めた設定（[`DevelopSettings::normalized`]）を正規化した JSON にして、
//! その SHA-256 を取る。JSON は serde_json が構造体のフィールドを定義の順に書き出すため、
//! 項目の順序は決まっている（設定の型には HashMap など順序の決まらない型を使わない）。
//! 浮動小数点数は最短の往復可能な表現で書かれ、-0.0 は丸めで 0.0 に、NaN は既定値に
//! しているため、同じ値なら同じバイト列になる。
//!
//! 注意: 数値の書式（`1e-7` か `0.0000001` か、など）は serde_json が使う書式化の実装に依存する。
//! 依存を更新して書式が変わると、同じ設定でもハッシュが変わる（キャッシュがすべて無効になる
//! だけで、誤ったキャッシュは使われない）。書式の変化はテスト（`float_text_format_is_pinned`）で
//! 検出する。
//!
//! 段階ごとの部分ハッシュ（2.2 節・2.7 節）:
//!
//! | 段階 | 含める項目 |
//! |---|---|
//! | A0 | process_version、RAW デコーダ（`render_deps.raw_decoder`） |
//! | A1 | process_version、render_deps 全体（カメラプロファイル・レンズデータ・デコーダ）、WB、ノイズ軽減、レンズ補正のうちステージ 7 の項目（`lens.profile_enabled`・`lens.vignetting_scale`・`lens.remove_chromatic_aberration`） |
//! | B | A1 の項目 ＋ 角度補正（回転）＋ 歪曲補正の適用量（`lens.distortion_scale`） |
//! | C | すべて（= [`DevelopSettings::develop_hash`]） |
//!
//! - 段階 A1 はステージ 2〜8（2.2 節）で、歪曲補正はステージ 9（ジオメトリ）に属する（2.1 節）。
//!   そのため歪曲補正の適用量は A1 に含めず、B に含める（2.7 節の表の「回転、歪曲補正」）。
//!   歪曲補正を有効にするかどうか（`lens.profile_enabled`）は周辺減光と共通なので A1 に含まれ、
//!   B にも A1 を通じて含まれる。
//! - 露光量・切り抜き・ハイライトやシャドウなどのスライダーは B に含めない（2.7 節の表）。
//! - ガイドのアルゴリズムのパラメータは process_version で固定されるため、設定には持たない。

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::DevelopSettings;
use super::params::{LensCorrection, NoiseReduction, RenderDeps, WhiteBalance};
use crate::Phase;

/// 部分ハッシュの入力の前に付ける、段階ごとの区別のための文字列。
const DOMAIN_A0: &[u8] = b"genzo.develop.A0\0";
const DOMAIN_A1: &[u8] = b"genzo.develop.A1\0";
const DOMAIN_B: &[u8] = b"genzo.develop.B\0";
const DOMAIN_RENDER_DEPS: &[u8] = b"genzo.render_deps\0";

/// バイト列を順につないだものの SHA-256。
pub(crate) fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// 構造体を JSON にする（設定の型は必ず JSON にできる）。
fn to_json_bytes<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("現像設定の型は必ず JSON にできる")
}

#[derive(Serialize)]
struct A0Input<'a> {
    process_version: u32,
    raw_decoder: &'a str,
}

/// レンズ補正のうち、段階 A1 に影響する項目（ステージ 7：周辺減光・倍率色収差。04 の 2.1 節）。
#[derive(Serialize)]
struct LensA1Input {
    profile_enabled: bool,
    vignetting_scale: f32,
    remove_chromatic_aberration: bool,
}

impl LensA1Input {
    fn new(lens: &LensCorrection) -> Self {
        Self {
            profile_enabled: lens.profile_enabled,
            vignetting_scale: lens.vignetting_scale,
            remove_chromatic_aberration: lens.remove_chromatic_aberration,
        }
    }
}

#[derive(Serialize)]
struct A1Input<'a> {
    process_version: u32,
    render_deps: &'a RenderDeps,
    white_balance: &'a WhiteBalance,
    lens: LensA1Input,
    noise_reduction: &'a NoiseReduction,
}

#[derive(Serialize)]
struct BInput<'a> {
    a1: A1Input<'a>,
    angle_deg: f32,
    /// 歪曲補正の適用量（ステージ 9。有効かどうかは `a1.lens.profile_enabled`）。
    distortion_scale: f32,
}

fn a1_input(s: &DevelopSettings) -> A1Input<'_> {
    A1Input {
        process_version: s.process_version,
        render_deps: &s.render_deps,
        white_balance: &s.white_balance,
        lens: LensA1Input::new(&s.lens),
        noise_reduction: &s.detail.noise_reduction,
    }
}

impl DevelopSettings {
    /// 丸めた設定の、正規化した JSON（ハッシュの入力）。
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(&self.normalized()).expect("丸めた現像設定は必ず JSON にできる")
    }

    /// 設定全体のハッシュ（正規化した JSON の SHA-256）。段階 C のハッシュと同じ。
    pub fn develop_hash(&self) -> [u8; 32] {
        sha256(&[self.canonical_json().as_bytes()])
    }

    /// 設定全体のハッシュの 16 進数（小文字 64 文字。`variant.develop_hash` の列に入れる）。
    pub fn develop_hash_hex(&self) -> String {
        hex::encode(self.develop_hash())
    }

    /// 段階 `phase` までの結果に影響する項目だけのハッシュ（2.2 節・2.7 節）。
    ///
    /// 含める項目はモジュールの説明の表のとおり。どの段階も丸めた設定から計算する。
    pub fn hash_for_phase(&self, phase: Phase) -> [u8; 32] {
        let s = self.normalized();
        match phase {
            Phase::A0 => {
                let input = A0Input {
                    process_version: s.process_version,
                    raw_decoder: &s.render_deps.raw_decoder,
                };
                sha256(&[DOMAIN_A0, &to_json_bytes(&input)])
            }
            Phase::A1 => sha256(&[DOMAIN_A1, &to_json_bytes(&a1_input(&s))]),
            Phase::B => {
                let input = BInput {
                    a1: a1_input(&s),
                    angle_deg: s.geometry.angle_deg,
                    distortion_scale: s.lens.distortion_scale,
                };
                sha256(&[DOMAIN_B, &to_json_bytes(&input)])
            }
            Phase::C => s.develop_hash(),
        }
    }
}

impl RenderDeps {
    /// 外部データの参照のハッシュ（キャッシュキーの `render_deps_hash`。4.1 節）。
    pub fn hash(&self) -> [u8; 32] {
        let mut deps = self.clone();
        deps.normalize();
        sha256(&[DOMAIN_RENDER_DEPS, &to_json_bytes(&deps)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::params::{CropRect, CurvePoint, DataRef, WbPreset};

    fn base() -> DevelopSettings {
        let mut s = DevelopSettings::default();
        s.render_deps.camera_profile = DataRef::new("sony-ilce-7m4", "0123456789abcdef".repeat(4));
        s.render_deps.raw_decoder = "libraw-0.21.2".to_owned();
        s
    }

    /// 変更前後で、どの段階のハッシュが変わったかを返す。
    fn changed_phases(a: &DevelopSettings, b: &DevelopSettings) -> Vec<Phase> {
        Phase::ALL
            .into_iter()
            .filter(|&p| a.hash_for_phase(p) != b.hash_for_phase(p))
            .collect()
    }

    #[test]
    fn same_settings_give_same_hash() {
        assert_eq!(base().develop_hash(), base().develop_hash());
        assert_eq!(base().develop_hash_hex().len(), 64);
        assert_eq!(base().hash_for_phase(Phase::C), base().develop_hash());
    }

    #[test]
    fn exposure_changes_only_phase_c() {
        let a = base();
        let mut b = base();
        b.exposure_ev = 0.5;
        assert_eq!(changed_phases(&a, &b), [Phase::C]);
    }

    #[test]
    fn crop_and_tone_sliders_do_not_change_guide() {
        let a = base();
        let mut b = base();
        b.geometry.crop = CropRect {
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
        };
        b.tone.highlights = -50.0;
        b.tone.shadows = 30.0;
        b.contrast = 20.0;
        b.tone_curve.luma = vec![CurvePoint::new(0.0, 0.1), CurvePoint::new(1.0, 1.0)];
        b.detail.sharpening.amount = 40.0;
        assert_eq!(changed_phases(&a, &b), [Phase::C]);
    }

    #[test]
    fn rotation_changes_b_and_c() {
        let a = base();
        let mut b = base();
        b.geometry.angle_deg = 1.5;
        assert_eq!(changed_phases(&a, &b), [Phase::B, Phase::C]);
    }

    #[test]
    fn distortion_changes_b_and_c_but_not_a1() {
        // 歪曲補正はステージ 9（ジオメトリ）なので、段階 A1 の中間キャッシュを作り直さない。
        let mut a = base();
        a.lens.profile_enabled = true;
        let mut b = a.clone();
        b.lens.distortion_scale = 50.0;
        assert_eq!(changed_phases(&a, &b), [Phase::B, Phase::C]);
    }

    #[test]
    fn phase_hashes_are_distinct_for_the_same_settings() {
        // 段階ごとに区別の文字列を付けているため、同じ設定でも段階が違えば値が違う。
        let s = base();
        let mut hashes: Vec<[u8; 32]> = Phase::ALL.map(|p| s.hash_for_phase(p)).to_vec();
        hashes.sort();
        hashes.dedup();
        assert_eq!(hashes.len(), Phase::ALL.len());
    }

    #[test]
    fn sensor_settings_change_a1_b_c() {
        let a = base();
        let mut wb = base();
        wb.white_balance = WhiteBalance::Preset(WbPreset::Cloudy);
        assert_eq!(changed_phases(&a, &wb), [Phase::A1, Phase::B, Phase::C]);

        let mut nr = base();
        nr.detail.noise_reduction.luminance = 20.0;
        assert_eq!(changed_phases(&a, &nr), [Phase::A1, Phase::B, Phase::C]);

        let mut lens = base();
        lens.lens.profile_enabled = true;
        assert_eq!(changed_phases(&a, &lens), [Phase::A1, Phase::B, Phase::C]);

        let mut vignetting = base();
        vignetting.lens.vignetting_scale = 50.0;
        assert_eq!(
            changed_phases(&a, &vignetting),
            [Phase::A1, Phase::B, Phase::C]
        );

        let mut ca = base();
        ca.lens.remove_chromatic_aberration = true;
        assert_eq!(changed_phases(&a, &ca), [Phase::A1, Phase::B, Phase::C]);

        let mut profile = base();
        profile.render_deps.camera_profile.sha256 = "1".repeat(64);
        assert_eq!(
            changed_phases(&a, &profile),
            [Phase::A1, Phase::B, Phase::C]
        );
    }

    #[test]
    fn decoder_and_process_version_change_every_phase() {
        let a = base();
        let mut dec = base();
        dec.render_deps.raw_decoder = "libraw-0.22.0".to_owned();
        assert_eq!(changed_phases(&a, &dec), Phase::ALL);

        // process_version は丸めで 1〜現在の値に収まるため、ここでは丸めの前後の
        // 等価性だけを確認する（値が 2 以上になったら変化のテストを追加する）。
        let mut pv = base();
        pv.process_version = 0;
        assert_eq!(changed_phases(&a, &pv), Vec::<Phase>::new());
    }

    #[test]
    fn equivalent_values_hash_equal() {
        // 丸めた結果が同じなら、ハッシュも同じ。
        let a = base();
        let mut b = base();
        b.exposure_ev = -0.0;
        b.render_deps.camera_profile.sha256 = "0123456789ABCDEF".repeat(4);
        assert_eq!(a.develop_hash(), b.develop_hash());

        let mut c = base();
        c.exposure_ev = 5.0;
        let mut d = base();
        d.exposure_ev = 7.0;
        assert_eq!(c.develop_hash(), d.develop_hash());
    }

    #[test]
    fn canonical_json_has_fixed_field_order() {
        let json = base().canonical_json();
        let keys = [
            "\"schema_version\"",
            "\"process_version\"",
            "\"white_balance\"",
            "\"exposure_ev\"",
            "\"contrast\"",
            "\"tone\"",
            "\"tone_curve\"",
            "\"color\"",
            "\"geometry\"",
            "\"lens\"",
            "\"detail\"",
            "\"masks\"",
            "\"render_deps\"",
        ];
        let positions: Vec<usize> = keys.iter().map(|k| json.find(k).unwrap()).collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{json}");
    }

    #[test]
    fn develop_hash_is_stable() {
        // 既定の設定のハッシュが意図せず変わっていないことを確かめる。
        // スキーマや型を変えて値が変わった場合は、キャッシュがすべて無効になることを
        // 確認したうえで期待値を更新する。
        let json = DevelopSettings::default().canonical_json();
        assert_eq!(
            json,
            concat!(
                r#"{"schema_version":1,"process_version":1,"white_balance":"as_shot","#,
                r#""exposure_ev":0.0,"contrast":0.0,"#,
                r#""tone":{"highlights":0.0,"shadows":0.0,"whites":0.0,"blacks":0.0},"#,
                r#""tone_curve":{"parametric":{"highlights":0.0,"lights":0.0,"darks":0.0,"shadows":0.0},"#,
                r#""luma":[],"red":[],"green":[],"blue":[]},"#,
                r#""color":{"saturation":0.0,"vibrance":0.0},"#,
                r#""geometry":{"crop":{"left":0.0,"top":0.0,"right":1.0,"bottom":1.0},"angle_deg":0.0,"aspect":null},"#,
                r#""lens":{"profile_enabled":false,"distortion_scale":100.0,"vignetting_scale":100.0,"remove_chromatic_aberration":false},"#,
                r#""detail":{"sharpening":{"amount":0.0,"radius":1.0,"detail":0.0,"masking":0.0},"#,
                r#""noise_reduction":{"luminance":0.0,"color":0.0}},"#,
                r#""masks":[],"#,
                r#""render_deps":{"camera_profile":{"id":"","sha256":""},"lens_profile":null,"raw_decoder":""}}"#
            )
        );
        let expected = sha256(&[json.as_bytes()]);
        assert_eq!(DevelopSettings::default().develop_hash(), expected);
    }

    #[test]
    fn float_text_format_is_pinned() {
        // ハッシュは浮動小数点数の JSON の書式に依存する（モジュールの説明を参照）。
        // 依存の更新でこのテストが失敗した場合は、すべてのキャッシュが無効になり、
        // variant.develop_hash の値も変わることを確認したうえで期待値を更新する。
        let mut s = DevelopSettings {
            exposure_ev: 0.1,
            contrast: 1e-7,
            ..Default::default()
        };
        s.tone.highlights = -33.333332;
        s.tone.shadows = 0.00001;
        s.detail.sharpening.radius = 2.9999998;
        let json = s.canonical_json();
        for part in [
            r#""exposure_ev":0.1,"contrast":1e-7,"#,
            r#""tone":{"highlights":-33.333332,"shadows":0.00001,"#,
            r#""radius":2.9999998,"#,
        ] {
            assert!(json.contains(part), "{part} が {json} にありません");
        }
    }

    #[test]
    fn negative_zero_and_nan_hash_like_their_normalized_values() {
        let mut a = base();
        a.tone_curve.luma = vec![CurvePoint::new(0.0, 0.0), CurvePoint::new(1.0, 1.0)];
        a.geometry.crop.left = 0.0;
        let mut b = a.clone();
        b.tone_curve.luma = vec![CurvePoint::new(-0.0, -0.0), CurvePoint::new(1.0, 1.0)];
        b.geometry.crop.left = -0.0;
        b.geometry.angle_deg = -0.0;
        b.color.vibrance = f32::NAN;
        b.detail.sharpening.radius = f32::NAN;
        for p in Phase::ALL {
            assert_eq!(a.hash_for_phase(p), b.hash_for_phase(p), "{p}");
        }
    }

    #[test]
    fn render_deps_hash_ignores_hex_case() {
        let a = base().render_deps;
        let mut b = a.clone();
        b.camera_profile.sha256 = b.camera_profile.sha256.to_ascii_uppercase();
        assert_eq!(a.hash(), b.hash());
        b.lens_profile = Some(DataRef::new("lens", ""));
        assert_ne!(a.hash(), b.hash());
    }
}
