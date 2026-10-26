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
//! | A1 | process_version、render_deps のうちルック以外（カメラ行列・カメラプロファイル（DCP）・レンズデータ・デコーダ）、カメラプロファイルの選択（`profile.camera`。ステージ 8）、WB、ハイライト復元（ステージ 4）、ノイズ軽減（ステージ 6。ディテール・コントラストを含む）、レンズ補正のうちステージ 7 の項目（`lens.profile_enabled`・`lens.vignetting_scale`・`lens.remove_chromatic_aberration`・手動の周辺減光（`lens.vignetting_amount`・`lens.vignetting_midpoint`）・手動の色収差（`lens.ca_red_cyan`・`lens.ca_blue_yellow`）） |
//! | B | A1 の項目 ＋ 角度補正（回転）＋ 歪曲補正の適用量（`lens.distortion_scale`）と手動の歪曲（`lens.distortion`）＋ 変形（`transform`。遠近補正。DEV-16） |
//! | C | すべて（= [`DevelopSettings::develop_hash`]） |
//!
//! - 段階 A1 はステージ 2〜8（2.2 節）で、歪曲補正はステージ 9（ジオメトリ）に属する（2.1 節）。
//!   そのため歪曲補正の適用量と手動の歪曲は A1 に含めず、B に含める（2.7 節の表の「回転、歪曲補正」）。
//!   歪曲補正を有効にするかどうか（`lens.profile_enabled`）は周辺減光と共通なので A1 に含まれ、
//!   B にも A1 を通じて含まれる。
//! - 変形（遠近補正。ステージ 9 の座標の変換で G を決める）は回転と同じく B に含める（2026-10-26 追加。DEV-16）。
//! - 露光量・切り抜き・ハイライトやシャドウなどのスライダーは B に含めない（2.7 節の表）。
//! - 色（ステージ 13。彩度・自然な彩度・カラーミキサー・カラーグレーディング・白黒）は段階 C だけに含まれる
//!   （A1・B のキャッシュを作り直さない）。
//! - テクスチャ・明瞭度・かすみの除去（ステージ 12）・シャープ（ステージ 14）・効果（ステージ 16 の周辺光量・粒子）も
//!   段階 C だけに含まれる。明瞭度・かすみの除去が使う全体の情報（段階 B のガイドと、かすみの推定）は、項目の値に
//!   よらず A1 の画像と回転・歪曲補正だけで決まる（値は段階 C で使う）ので、B に含めない。
//! - ガイドのアルゴリズムのパラメータは process_version で固定されるため、設定には持たない。
//! - ローカル補正（マスク。DEV-18・DEV-19）はステージ 10〜14 を画素ごとに重み付けするので C だけ。かすみの除去の差分の
//!   あるマスクがあると段階 B でかすみの推定を作るが、それはガイドのキャッシュのキー（genzo-pipeline の `GuideSpec`）の
//!   「かすみの推定を作るか」で区別する（設定の B のハッシュには入れない。全体のかすみの除去と同じ）。
//! - カメラプロファイル（DCP。DEV-17）はステージ 8 なので A1 に含める（選択の ID と、ファイルの内容の参照
//!   `render_deps.profile`）。ルック（`profile.look`・`look_amount`・`render_deps.look`）とプロファイルのルックテーブルの
//!   位置（段階 C）は C だけ。キャッシュキーの外部データのハッシュも、段階 A1・B ではルックを除く
//!   （[`RenderDeps::hash_for_phase`]）。ルックのない設定の A1 の入力の JSON は、この項目を足す前と同じ。

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::DevelopSettings;
use super::params::{
    DataRef, HighlightRecovery, LensCorrection, NoiseReduction, RenderDeps, Transform, WhiteBalance,
};
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
    vignetting_amount: f32,
    vignetting_midpoint: f32,
    ca_red_cyan: f32,
    ca_blue_yellow: f32,
}

impl LensA1Input {
    fn new(lens: &LensCorrection) -> Self {
        Self {
            profile_enabled: lens.profile_enabled,
            vignetting_scale: lens.vignetting_scale,
            remove_chromatic_aberration: lens.remove_chromatic_aberration,
            vignetting_amount: lens.vignetting_amount,
            vignetting_midpoint: lens.vignetting_midpoint,
            ca_red_cyan: lens.ca_red_cyan,
            ca_blue_yellow: lens.ca_blue_yellow,
        }
    }
}

/// 外部データのうち、段階 A1・B に影響するもの（ルックを除く。項目の順は [`RenderDeps`] と同じで、ルックがなければ
/// `RenderDeps` の JSON と同じ）。
#[derive(Serialize)]
struct RenderDepsA1<'a> {
    camera_profile: &'a DataRef,
    lens_profile: &'a Option<DataRef>,
    raw_decoder: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<&'a DataRef>,
}

impl<'a> RenderDepsA1<'a> {
    fn new(d: &'a RenderDeps) -> Self {
        Self {
            camera_profile: &d.camera_profile,
            lens_profile: &d.lens_profile,
            raw_decoder: &d.raw_decoder,
            profile: d.profile.as_ref(),
        }
    }
}

#[derive(Serialize)]
struct A1Input<'a> {
    process_version: u32,
    render_deps: RenderDepsA1<'a>,
    /// カメラプロファイル（DCP）の選択（ステージ 8。標準なら JSON に書かない）。
    #[serde(skip_serializing_if = "Option::is_none")]
    camera_profile: Option<&'a str>,
    white_balance: &'a WhiteBalance,
    highlight_recovery: &'a HighlightRecovery,
    lens: LensA1Input,
    noise_reduction: &'a NoiseReduction,
}

#[derive(Serialize)]
struct BInput<'a> {
    a1: A1Input<'a>,
    angle_deg: f32,
    /// 歪曲補正の適用量（ステージ 9。有効かどうかは `a1.lens.profile_enabled`）。
    distortion_scale: f32,
    /// 手動の歪曲（ステージ 9）。
    distortion: f32,
    /// 変形（遠近補正。ステージ 9。DEV-16）。
    transform: &'a Transform,
}

fn a1_input(s: &DevelopSettings) -> A1Input<'_> {
    A1Input {
        process_version: s.process_version,
        render_deps: RenderDepsA1::new(&s.render_deps),
        camera_profile: s.profile.camera.as_deref(),
        white_balance: &s.white_balance,
        highlight_recovery: &s.highlight_recovery,
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
                    distortion: s.lens.distortion,
                    transform: &s.transform,
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

    /// 段階 `phase` までの結果に影響する外部データの参照のハッシュ。ルック（[`RenderDeps::look`]。段階 C）は C だけに
    /// 含める（A0・A1・B ではルックを除いた [`hash`](Self::hash)。ルックがなければすべての段階で `hash` と同じ）。
    pub fn hash_for_phase(&self, phase: Phase) -> [u8; 32] {
        match phase {
            Phase::C => self.hash(),
            Phase::A0 | Phase::A1 | Phase::B => {
                if self.look.is_none() {
                    self.hash()
                } else {
                    RenderDeps {
                        look: None,
                        ..self.clone()
                    }
                    .hash()
                }
            }
        }
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

    /// 変形（遠近補正。DEV-16）はステージ 9 の座標の変換で G を決めるので、回転と同じく段階 B から変わる
    /// （A1 は変わらない）。
    #[test]
    fn transform_changes_the_guide_but_not_a1() {
        let a = base();
        let mut b = base();
        b.transform.vertical = -20.0;
        assert_eq!(changed_phases(&a, &b), [Phase::B, Phase::C]);
        let mut c = base();
        c.transform.scale = 110.0;
        assert_eq!(changed_phases(&a, &c), [Phase::B, Phase::C]);
        // 既定値（何もしない）の書き方の違い（-0.0）は丸めで同じになる。
        let mut d = base();
        d.transform.offset_x = -0.0;
        assert_eq!(changed_phases(&a, &d), []);
    }

    #[test]
    fn color_items_change_only_phase_c() {
        // カラーミキサー・カラーグレーディング・白黒はステージ 13（段階 C）。A1・ガイドを作り直さない。
        let a = base();
        let mut b = base();
        b.color.mixer.blue.luminance = -40.0;
        assert_eq!(changed_phases(&a, &b), [Phase::C]);
        let mut c = base();
        c.color.grading.shadows.saturation = 30.0;
        c.color.grading.balance = 20.0;
        assert_eq!(changed_phases(&a, &c), [Phase::C]);
        let mut d = base();
        d.color.black_white.enabled = true;
        d.color.black_white.mix.red = 25.0;
        assert_eq!(changed_phases(&a, &d), [Phase::C]);
    }

    #[test]
    fn presence_sharpening_and_effects_change_only_phase_c() {
        // テクスチャ・明瞭度・かすみの除去（ステージ 12）・シャープ（14）・効果（16）は段階 C。A1・ガイドを作り直さない。
        let a = base();
        for f in [
            |s: &mut DevelopSettings| s.presence.texture = 30.0,
            |s: &mut DevelopSettings| s.presence.clarity = -20.0,
            |s: &mut DevelopSettings| s.presence.dehaze = 40.0,
            |s: &mut DevelopSettings| s.detail.sharpening.masking = 60.0,
            |s: &mut DevelopSettings| s.effects.vignette.amount = -30.0,
            |s: &mut DevelopSettings| s.effects.grain.amount = 25.0,
            |s: &mut DevelopSettings| {
                s.effects.vignette.center = Some(crate::VignetteCenter { x: 0.3, y: 0.4 })
            },
        ] {
            let mut b = base();
            f(&mut b);
            assert_eq!(changed_phases(&a, &b), [Phase::C]);
        }
    }

    #[test]
    fn masks_change_only_phase_c() {
        // ローカル補正（DEV-18・DEV-19）はステージ 10〜14 の重み付けなので段階 C。かすみの除去の差分があっても、
        // ガイドのキャッシュのキーには「かすみの推定を作るか」だけが入る（genzo-pipeline の GuideSpec）。
        use crate::{BrushStroke, LocalAdjustment, MaskComponent, MaskShape};
        let a = base();
        let mut b = base();
        let mut m = LocalAdjustment::new(MaskComponent::add(MaskShape::Brush {
            strokes: vec![BrushStroke {
                points: vec![[0.2, 0.3], [0.4, 0.3]],
                ..Default::default()
            }],
        }));
        m.delta.exposure_ev = 0.7;
        m.delta.dehaze = 25.0;
        b.masks.push(m);
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
        // 手動の歪曲も同じ（DEV-14）。
        let mut c = a.clone();
        c.lens.distortion = 30.0;
        assert_eq!(changed_phases(&a, &c), [Phase::B, Phase::C]);
    }

    #[test]
    fn manual_vignetting_and_chromatic_aberration_change_a1() {
        // 手動の周辺減光・色収差はステージ 7（段階 A1）。
        let a = base();
        let mut v = base();
        v.lens.vignetting_amount = 40.0;
        assert_eq!(changed_phases(&a, &v), [Phase::A1, Phase::B, Phase::C]);
        let mut m = base();
        m.lens.vignetting_midpoint = 20.0;
        assert_eq!(changed_phases(&a, &m), [Phase::A1, Phase::B, Phase::C]);
        let mut r = base();
        r.lens.ca_red_cyan = -10.0;
        assert_eq!(changed_phases(&a, &r), [Phase::A1, Phase::B, Phase::C]);
        let mut b = base();
        b.lens.ca_blue_yellow = 10.0;
        assert_eq!(changed_phases(&a, &b), [Phase::A1, Phase::B, Phase::C]);
        let mut lp = base();
        lp.render_deps.lens_profile = Some(DataRef::new("sony_arw:x", "2".repeat(64)));
        assert_eq!(changed_phases(&a, &lp), [Phase::A1, Phase::B, Phase::C]);
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

        // ノイズ軽減のディテール・コントラスト（ステージ 6）とハイライト復元（ステージ 4）も段階 A1。
        let mut nd = base();
        nd.detail.noise_reduction.color_detail = 20.0;
        assert_eq!(changed_phases(&a, &nd), [Phase::A1, Phase::B, Phase::C]);
        let mut nc = base();
        nc.detail.noise_reduction.luminance_contrast = 20.0;
        assert_eq!(changed_phases(&a, &nc), [Phase::A1, Phase::B, Phase::C]);
        let mut hr = base();
        hr.highlight_recovery.method = crate::develop::params::HighlightMethod::Reconstruct;
        assert_eq!(changed_phases(&a, &hr), [Phase::A1, Phase::B, Phase::C]);
        let mut ha = base();
        ha.highlight_recovery.amount = 50.0;
        assert_eq!(changed_phases(&a, &ha), [Phase::A1, Phase::B, Phase::C]);

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

        // 処理バージョンが違えば、すべての段階が変わる（既定は現在の版 2）。
        let mut pv = base();
        pv.process_version = 1;
        assert_eq!(changed_phases(&a, &pv), Phase::ALL);
        // 丸め（0 → 1）の前後は同じ。
        let mut zero = base();
        zero.process_version = 0;
        assert_eq!(changed_phases(&pv, &zero), Vec::<Phase>::new());
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
            "\"profile\"",
            "\"white_balance\"",
            "\"highlight_recovery\"",
            "\"exposure_ev\"",
            "\"contrast\"",
            "\"tone\"",
            "\"presence\"",
            "\"tone_curve\"",
            "\"color\"",
            "\"geometry\"",
            "\"lens\"",
            "\"detail\"",
            "\"effects\"",
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
                r#"{"schema_version":3,"process_version":2,"#,
                r#""profile":{"camera":null,"look":null,"look_amount":100.0},"white_balance":"as_shot","#,
                r#""highlight_recovery":{"method":"clip","amount":100.0},"#,
                r#""exposure_ev":0.0,"contrast":0.0,"#,
                r#""tone":{"highlights":0.0,"shadows":0.0,"whites":0.0,"blacks":0.0},"#,
                r#""presence":{"texture":0.0,"clarity":0.0,"dehaze":0.0},"#,
                r#""tone_curve":{"parametric":{"highlights":0.0,"lights":0.0,"darks":0.0,"shadows":0.0},"#,
                r#""luma":[],"red":[],"green":[],"blue":[]},"#,
                r#""color":{"saturation":0.0,"vibrance":0.0,"#,
                r#""mixer":{"red":{"hue":0.0,"saturation":0.0,"luminance":0.0},"orange":{"hue":0.0,"saturation":0.0,"luminance":0.0},"yellow":{"hue":0.0,"saturation":0.0,"luminance":0.0},"green":{"hue":0.0,"saturation":0.0,"luminance":0.0},"aqua":{"hue":0.0,"saturation":0.0,"luminance":0.0},"blue":{"hue":0.0,"saturation":0.0,"luminance":0.0},"purple":{"hue":0.0,"saturation":0.0,"luminance":0.0},"magenta":{"hue":0.0,"saturation":0.0,"luminance":0.0}},"#,
                r#""grading":{"shadows":{"hue":0.0,"saturation":0.0,"luminance":0.0},"midtones":{"hue":0.0,"saturation":0.0,"luminance":0.0},"highlights":{"hue":0.0,"saturation":0.0,"luminance":0.0},"global":{"hue":0.0,"saturation":0.0,"luminance":0.0},"blending":50.0,"balance":0.0},"#,
                r#""black_white":{"enabled":false,"mix":{"red":0.0,"orange":0.0,"yellow":0.0,"green":0.0,"aqua":0.0,"blue":0.0,"purple":0.0,"magenta":0.0}}},"#,
                r#""geometry":{"crop":{"left":0.0,"top":0.0,"right":1.0,"bottom":1.0},"angle_deg":0.0,"aspect":null},"#,
                r#""lens":{"profile_enabled":false,"distortion_scale":100.0,"vignetting_scale":100.0,"remove_chromatic_aberration":false,"#,
                r#""distortion":0.0,"vignetting_amount":0.0,"vignetting_midpoint":50.0,"ca_red_cyan":0.0,"ca_blue_yellow":0.0},"#,
                r#""transform":{"vertical":0.0,"horizontal":0.0,"aspect":0.0,"scale":100.0,"offset_x":0.0,"offset_y":0.0},"#,
                r#""detail":{"sharpening":{"amount":0.0,"radius":1.0,"detail":25.0,"masking":0.0},"#,
                r#""noise_reduction":{"luminance":0.0,"luminance_detail":50.0,"luminance_contrast":0.0,"color":0.0,"color_detail":50.0}},"#,
                r#""effects":{"vignette":{"amount":0.0,"midpoint":50.0,"roundness":0.0,"feather":50.0,"highlights":0.0},"#,
                r#""grain":{"amount":0.0,"size":25.0,"roughness":50.0}},"#,
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
    fn camera_profile_changes_a1_b_c_and_look_only_c() {
        // カメラプロファイル（DCP）はステージ 8（段階 A1）。ルックは段階 C だけ（DEV-17）。
        let a = base();
        let mut b = base();
        b.profile.camera = Some("Sony ILCE-7M4 テスト".into());
        assert_eq!(changed_phases(&a, &b), [Phase::A1, Phase::B, Phase::C]);
        let mut c = base();
        c.render_deps.profile = Some(DataRef::new("dcp:x", "ab".repeat(32)));
        assert_eq!(changed_phases(&a, &c), [Phase::A1, Phase::B, Phase::C]);
        for f in [
            |s: &mut DevelopSettings| s.profile.look = Some("film".into()),
            |s: &mut DevelopSettings| s.profile.look_amount = 40.0,
            |s: &mut DevelopSettings| {
                s.render_deps.look = Some(DataRef::new("cube:film", "cd".repeat(32)))
            },
        ] {
            let mut d = base();
            f(&mut d);
            assert_eq!(changed_phases(&a, &d), [Phase::C]);
        }
        // 外部データのハッシュも、ルックは段階 C だけ。
        let mut e = base();
        e.render_deps.look = Some(DataRef::new("cube:film", "cd".repeat(32)));
        for p in [Phase::A0, Phase::A1, Phase::B] {
            assert_eq!(
                e.render_deps.hash_for_phase(p),
                a.render_deps.hash_for_phase(p),
                "{p}"
            );
        }
        assert_ne!(
            e.render_deps.hash_for_phase(Phase::C),
            a.render_deps.hash_for_phase(Phase::C)
        );
        assert_eq!(
            a.render_deps.hash_for_phase(Phase::A1),
            a.render_deps.hash()
        );
        // ルックとプロファイルのない設定の外部データの JSON は、項目を足す前と同じ（新しい項目を書かない）。
        assert_eq!(
            serde_json::to_string(&a.render_deps).unwrap(),
            r#"{"camera_profile":{"id":"sony-ilce-7m4","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"},"lens_profile":null,"raw_decoder":"libraw-0.21.2"}"#
        );
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
