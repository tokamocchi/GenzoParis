//! 現像結果に影響する外部データ（`render_deps`。04 の 2.5 節、レビュー R-08）の記録。
//!
//! - RAW を展開したときに、実際に使った依存を求める（[`actual_render_deps`]）:
//!   - カメラ行列（`camera_profile`）: ID は行列の出どころと機種（`libraw_table:SONY ILCE-7M4`、
//!     `dng_color_matrix1_d65:...` など）、SHA-256 は行列の値（9 個の f32 のリトルエンディアン）。行列がなければ
//!     ID は `none`、SHA-256 は空。今の行列は、設計（2.5 節）のアプリのデータファイルではなく、LibRaw の
//!     内蔵の表か DNG の ColorMatrix（implementation_status No.25）。
//!   - RAW デコーダ（`raw_decoder`）: ワーカーが返す識別子（`libraw-0.21.2` など）。
//!   - レンズのデータ（`lens_profile`。DEV-14）: RAW に埋め込まれたレンズ補正の情報（genzo-raw の `lens`）。
//!     ID は出どころと機種・レンズ（`sony_arw:SONY ILCE-7M4|FE 24-70mm F2.8 GM II` など）、SHA-256 は情報の
//!     内容（JSON にしたもの）。情報がなければ `None`。lensfun などの外部のデータは使わない。
//!
//!   RAW 以外の入力では空のまま。
//! - カメラプロファイル・ルック（`profile`・`look`。DEV-17）: 現像設定が指す ID で、取り込んだライブラリ
//!   （`profiles`）から引いたファイル。ID は `dcp:<ID>`・`cube:<ID>`、SHA-256 はファイルの内容。引けない・使わない
//!   （処理バージョン 1・ルックの適用量 0）なら `None`。描画のエンジンが引く（genzo-pipeline の
//!   `Engine::effective_settings`）ので、RAW の展開とは別に、設定を描くたびに求める（[`with_render_deps`]）。
//! - 現像のセッション・書き出し・プレビューの作り直しは、保存された設定の `render_deps` を、実際に使った
//!   依存に置き換えた設定で描く（[`with_render_deps`]。段階 A0 / A1 のキャッシュのキー（`hash_for_phase`）
//!   に反映される）。`render_deps` はアプリが決める値で、利用者の設定（貼り付け・Undo・UI から送られた
//!   設定）では変えない。
//! - **保存**: 現像のセッションからの保存は、実際に使った依存を含めて記録する。保存された `render_deps` が
//!   空の写真（これまでの写真）は、**開いたときにメモリの中で今の値で埋め、次に保存したときに記録する**
//!   （開いただけでは履歴を増やさず、未調整の写真の設定を保存しない。SCL-02）。保存された値と今の値が
//!   違う（LibRaw を更新した・OS ごとに版が違う・取り込んだプロファイルを置き換えたなど）ときは警告する（色が
//!   変わりうる。IQ-08）。プロファイル・ルックが見つからないときは、描画の警告（genzo-pipeline の
//!   `SensorWarning::CameraProfileMissing`・`LookMissing`）で知らせるので、ここでは比べない。
//! - **書き出し**: 使った依存を結果（[`crate::ExportOutcome::Written`] の `render_deps`）に記録する。
//! - **L0 / L1 のキャッシュキー**（`previews::rendered_key`）は、写真を展開せずに求める必要があるため、
//!   カメラ行列の内容のハッシュの代わりに RAW デコーダの識別子を使う（LibRaw の内蔵の表の行列は機種と
//!   デコーダの版で、DNG の ColorMatrix はファイルの内容（リビジョン）で決まるため）。LibRaw を更新すると
//!   キーが変わり、古い色のサムネイル・プレビューは使われなくなる。カメラプロファイル・ルックは、展開せずに
//!   引けるので内容の SHA-256 を使う（置き換えると L1 のキーが変わる）。

use genzo_model::{DataRef, DevelopSettings, PhotoMetadata, RenderDeps};
use genzo_pipeline::Engine;
use genzo_raw::{CamXyzSource, LensCorrectionData};
use sha2::{Digest, Sha256};

/// 展開した RAW で実際に使う依存（このモジュールの doc）。
pub(crate) fn actual_render_deps(
    cam_xyz: Option<&[[f32; 3]; 3]>,
    source: Option<CamXyzSource>,
    metadata: &PhotoMetadata,
    decoder_id: Option<&str>,
    lens: Option<&LensCorrectionData>,
) -> RenderDeps {
    let camera = metadata
        .camera_name()
        .unwrap_or_else(|| "unknown".to_owned());
    let camera_profile = match cam_xyz {
        Some(m) => {
            let origin = match source {
                Some(CamXyzSource::DngColorMatrix { index, illuminant }) => {
                    format!("dng_color_matrix{index}_illuminant{illuminant}")
                }
                Some(CamXyzSource::LibRawTable) => "libraw_table".to_owned(),
                // 出どころが分からない（古いワーカー）・行列がないのに値がある（起きない）。
                Some(CamXyzSource::None) | None => "raw_cam_xyz".to_owned(),
            };
            DataRef::new(format!("{origin}:{camera}"), matrix_sha256(m))
        }
        None => DataRef::new("none", ""),
    };
    let lens_profile = lens.map(|l| {
        let name = metadata.lens.as_deref().unwrap_or("unknown");
        DataRef::new(format!("{}:{camera}|{name}", l.source_id()), lens_sha256(l))
    });
    RenderDeps {
        camera_profile,
        lens_profile,
        raw_decoder: decoder_id.unwrap_or_default().to_owned(),
        // カメラプロファイル・ルックは描画のエンジンが入れる（[`with_render_deps`]）。
        profile: None,
        look: None,
    }
}

/// レンズ補正の情報の SHA-256（JSON にしたもの。小文字の 16 進数 64 文字）。
fn lens_sha256(l: &LensCorrectionData) -> String {
    let mut h = Sha256::new();
    h.update(b"genzo.lens_correction.v1\0");
    h.update(serde_json::to_vec(l).expect("レンズ補正の情報は JSON にできる"));
    h.finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

/// 行列の値の SHA-256（小文字の 16 進数 64 文字）。
fn matrix_sha256(m: &[[f32; 3]; 3]) -> String {
    let mut h = Sha256::new();
    h.update(b"genzo.camera_matrix.xyz_to_camera.v1\0");
    for row in m {
        for v in row {
            h.update(v.to_le_bytes());
        }
    }
    h.finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

/// 描画に使う設定: `render_deps` を `deps`（RAW の展開で求めた依存）に置き換え、カメラプロファイル・ルックの参照を
/// 今ライブラリから引けるものにしたもの（このモジュールの doc）。
pub(crate) fn with_render_deps(
    engine: &Engine,
    settings: &DevelopSettings,
    deps: &RenderDeps,
) -> DevelopSettings {
    let mut s = settings.clone();
    s.render_deps = deps.clone();
    engine.effective_settings(&s)
}

/// 記録したプロファイル・ルックの参照が、今引けるものと違うか（記録がない・今引けない（描画の警告で知らせる）なら
/// 違うとしない）。
fn data_ref_changed(recorded: Option<&DataRef>, actual: Option<&DataRef>) -> bool {
    matches!((recorded, actual), (Some(r), Some(a)) if r != a)
}

/// 保存された依存が、今の依存と違うか（空なら違うとしない。空は「まだ記録していない」）。
///
/// レンズのデータは、記録がない（`None`。レンズ補正の前に保存した写真）なら比べない（記録していない
/// だけで、結果に影響していないため）。カメラプロファイル・ルックは [`data_ref_changed`]。
pub(crate) fn recorded_deps_differ(recorded: &RenderDeps, actual: &RenderDeps) -> bool {
    if *recorded == RenderDeps::default() {
        return false;
    }
    raw_deps_differ(recorded, actual)
        || data_ref_changed(recorded.profile.as_ref(), actual.profile.as_ref())
        || data_ref_changed(recorded.look.as_ref(), actual.look.as_ref())
}

/// RAW の展開で求める依存（カメラ行列・RAW デコーダ・レンズのデータ）が違うか（それらを記録していなければ違うと
/// しない。展開せずに設定を貼り付けた写真は、プロファイル・ルックの参照だけを記録していることがある）。
fn raw_deps_differ(recorded: &RenderDeps, actual: &RenderDeps) -> bool {
    let empty = RenderDeps::default();
    let raw_recorded = recorded.camera_profile != empty.camera_profile
        || recorded.raw_decoder != empty.raw_decoder
        || recorded.lens_profile.is_some();
    raw_recorded
        && (recorded.camera_profile != actual.camera_profile
            || recorded.raw_decoder != actual.raw_decoder
            || (recorded.lens_profile.is_some() && recorded.lens_profile != actual.lens_profile))
}

/// 参照の ID から、表示する名前（`dcp:` などの前置きを除いたもの）。
fn ref_name(r: &DataRef) -> &str {
    r.id.split_once(':').map_or(r.id.as_str(), |(_, n)| n)
}

/// 保存された依存と今の依存が違うときの警告の文（違うものごとの文を「。」でつなぐ）。
pub(crate) fn deps_changed_message(recorded: &RenderDeps, actual: &RenderDeps) -> String {
    let mut parts = Vec::new();
    if raw_deps_differ(recorded, actual) {
        if recorded.camera_profile == actual.camera_profile
            && recorded.raw_decoder == actual.raw_decoder
        {
            let id = |d: &RenderDeps| {
                d.lens_profile
                    .as_ref()
                    .map_or_else(|| "なし".to_owned(), |l| l.id.clone())
            };
            parts.push(format!(
                "前回保存したときと、埋め込みのレンズ補正の情報が違います（保存: {}、今: {}）。レンズ補正の結果が変わる可能性があります",
                id(recorded),
                id(actual)
            ));
        } else {
            parts.push(format!(
                "前回保存したときと、カメラ行列または RAW デコーダが違います（保存: {} / {}、今: {} / {}）。同じ設定でも色が変わる可能性があります",
                recorded.camera_profile.id,
                recorded.raw_decoder,
                actual.camera_profile.id,
                actual.raw_decoder
            ));
        }
    }
    if let (Some(r), true) = (
        &recorded.profile,
        data_ref_changed(recorded.profile.as_ref(), actual.profile.as_ref()),
    ) {
        parts.push(format!(
            "前回保存したときと、カメラプロファイル「{}」のファイルの内容が違います（取り込み直した・置き換えたなど）。同じ設定でも色が変わる可能性があります",
            ref_name(r)
        ));
    }
    if let (Some(r), true) = (
        &recorded.look,
        data_ref_changed(recorded.look.as_ref(), actual.look.as_ref()),
    ) {
        parts.push(format!(
            "前回保存したときと、ルック「{}」のファイルの内容が違います（取り込み直した・置き換えたなど）。同じ設定でも色が変わる可能性があります",
            ref_name(r)
        ));
    }
    parts.join("。")
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: [[f32; 3]; 3] = [
        [0.7424, -0.2329, -0.0466],
        [-0.4598, 1.2471, 0.2347],
        [-0.0715, 0.1505, 0.6066],
    ];

    fn engine() -> Engine {
        Engine::new(genzo_pipeline::EngineConfig::default()).unwrap()
    }

    fn meta() -> PhotoMetadata {
        PhotoMetadata {
            make: Some("SONY".into()),
            model: Some("ILCE-7M4".into()),
            ..Default::default()
        }
    }

    #[test]
    fn deps_name_the_matrix_source_and_hash_its_values() {
        let d = actual_render_deps(
            Some(&M),
            Some(CamXyzSource::LibRawTable),
            &meta(),
            Some("libraw-0.21.2"),
            None,
        );
        assert!(d.camera_profile.id.starts_with("libraw_table:"), "{d:?}");
        assert!(d.camera_profile.id.contains("ILCE-7M4"), "{d:?}");
        assert_eq!(d.camera_profile.sha256.len(), 64);
        assert_eq!(d.raw_decoder, "libraw-0.21.2");
        // 設定として正しい（検証を通る）。
        let s = with_render_deps(&engine(), &DevelopSettings::default(), &d);
        s.validate().unwrap();
        // 行列が違えばハッシュも違う。
        let mut m2 = M;
        m2[0][0] += 0.03;
        let d2 = actual_render_deps(
            Some(&m2),
            Some(CamXyzSource::LibRawTable),
            &meta(),
            Some("libraw-0.21.2"),
            None,
        );
        assert_ne!(d.camera_profile.sha256, d2.camera_profile.sha256);
        // DNG の ColorMatrix。
        let dng = actual_render_deps(
            Some(&M),
            Some(CamXyzSource::DngColorMatrix {
                index: 1,
                illuminant: 21,
            }),
            &meta(),
            None,
            None,
        );
        assert!(
            dng.camera_profile
                .id
                .starts_with("dng_color_matrix1_illuminant21:")
        );
        assert_eq!(dng.raw_decoder, "");
        // 行列がない。
        let none = actual_render_deps(None, Some(CamXyzSource::None), &meta(), Some("x"), None);
        assert_eq!(none.camera_profile, DataRef::new("none", ""));
    }

    #[test]
    fn empty_recorded_deps_are_filled_without_warning() {
        let actual = actual_render_deps(Some(&M), None, &meta(), Some("libraw-0.21.2"), None);
        assert!(!recorded_deps_differ(&RenderDeps::default(), &actual));
        assert!(!recorded_deps_differ(&actual, &actual));
        let mut old = actual.clone();
        old.raw_decoder = "libraw-0.20.0".into();
        assert!(recorded_deps_differ(&old, &actual));
        assert!(deps_changed_message(&old, &actual).contains("libraw-0.20.0"));
    }

    #[test]
    fn lens_data_is_recorded_and_compared_only_when_recorded() {
        use genzo_raw::SonyLensCorrection;
        let lens = LensCorrectionData::SonyArw(SonyLensCorrection {
            distortion: vec![-5; 16],
            ..Default::default()
        });
        let mut m = meta();
        m.lens = Some("FE 24-70mm F2.8 GM II".into());
        let with = actual_render_deps(Some(&M), None, &m, Some("libraw-0.21.2"), Some(&lens));
        let lp = with.lens_profile.clone().unwrap();
        assert_eq!(lp.id, "sony_arw:SONY ILCE-7M4|FE 24-70mm F2.8 GM II");
        assert_eq!(lp.sha256.len(), 64);
        // 設定として正しい（検証を通る）。
        with_render_deps(&engine(), &DevelopSettings::default(), &with)
            .validate()
            .unwrap();
        // 記録がない（レンズ補正の前に保存した写真）なら警告しない。
        let mut old = with.clone();
        old.lens_profile = None;
        assert!(!recorded_deps_differ(&old, &with));
        // 記録した情報と違えば警告する。
        let other = LensCorrectionData::SonyArw(SonyLensCorrection {
            distortion: vec![-6; 16],
            ..Default::default()
        });
        let changed = actual_render_deps(Some(&M), None, &m, Some("libraw-0.21.2"), Some(&other));
        assert_ne!(changed.lens_profile, with.lens_profile);
        assert!(recorded_deps_differ(&with, &changed));
        assert!(deps_changed_message(&with, &changed).contains("レンズ補正"));
    }
    /// カメラプロファイル・ルックの参照（DEV-17）: 記録した内容と今の内容が違えば警告する。記録がない・今引けない
    /// （描画の警告で知らせる）なら警告しない。RAW の依存を記録していない（展開せずに貼り付けた）写真でも、
    /// カメラ行列の違いとはしない。
    #[test]
    fn profile_and_look_refs_are_compared_only_when_both_known() {
        let actual = actual_render_deps(Some(&M), None, &meta(), Some("libraw-0.21.2"), None);
        let p1 = DataRef::new("dcp:Sony 標準", "ab".repeat(32));
        let p2 = DataRef::new("dcp:Sony 標準", "cd".repeat(32));
        let l1 = DataRef::new("cube:film", "ef".repeat(32));
        let with = |d: &RenderDeps, p: Option<&DataRef>, l: Option<&DataRef>| RenderDeps {
            profile: p.cloned(),
            look: l.cloned(),
            ..d.clone()
        };
        let recorded = with(&actual, Some(&p1), Some(&l1));
        assert!(!recorded_deps_differ(&recorded, &recorded));
        // 置き換えた（内容が違う）。
        let now = with(&actual, Some(&p2), Some(&l1));
        assert!(recorded_deps_differ(&recorded, &now));
        let msg = deps_changed_message(&recorded, &now);
        assert!(msg.contains("カメラプロファイル「Sony 標準」"), "{msg}");
        assert!(!msg.contains("ルック"), "{msg}");
        assert!(!msg.contains("カメラ行列"), "{msg}");
        // 今引けない・記録がない。
        assert!(!recorded_deps_differ(&recorded, &with(&actual, None, None)));
        assert!(!recorded_deps_differ(&with(&actual, None, None), &now));
        // ルックだけが違う。
        let l2 = DataRef::new("cube:film", "01".repeat(32));
        let msg = deps_changed_message(&recorded, &with(&actual, Some(&p1), Some(&l2)));
        assert!(
            msg.starts_with("前回保存したときと、ルック「film」"),
            "{msg}"
        );
        // 展開せずに貼り付けた写真（RAW の依存は空・参照だけ）は、カメラ行列の違いとしない。
        let pasted = with(&RenderDeps::default(), Some(&p1), None);
        assert!(!recorded_deps_differ(
            &pasted,
            &with(&actual, Some(&p1), None)
        ));
        let msg = deps_changed_message(&pasted, &with(&actual, Some(&p2), None));
        assert!(!msg.contains("カメラ行列"), "{msg}");
        // 引けない（解決する仕組みのないエンジン）なら、保存されていた参照も残さない。
        let mut s = DevelopSettings::default();
        s.profile.camera = Some("Sony 標準".into());
        s.render_deps.profile = Some(p1.clone());
        let e = with_render_deps(&engine(), &s, &actual);
        assert_eq!(e.render_deps.profile, None);
        assert_eq!(e.render_deps.raw_decoder, "libraw-0.21.2");
    }
}
