//! 現像結果に影響する外部データ（`render_deps`。04 の 2.5 節、レビュー R-08）の記録。
//!
//! - RAW を展開したときに、実際に使った依存を求める（[`actual_render_deps`]）:
//!   - カメラ行列（`camera_profile`）: ID は行列の出どころと機種（`libraw_table:SONY ILCE-7M4`、
//!     `dng_color_matrix1_d65:...` など）、SHA-256 は行列の値（9 個の f32 のリトルエンディアン）。行列がなければ
//!     ID は `none`、SHA-256 は空。今の行列は、設計（2.5 節）のアプリのデータファイルではなく、LibRaw の
//!     内蔵の表か DNG の ColorMatrix（implementation_status No.25）。
//!   - RAW デコーダ（`raw_decoder`）: ワーカーが返す識別子（`libraw-0.21.2` など）。
//!   - レンズのデータ（`lens_profile`）: まだない（`None`）。
//!
//!   RAW 以外の入力では空のまま。
//! - 現像のセッション・書き出し・プレビューの作り直しは、保存された設定の `render_deps` を、実際に使った
//!   依存に置き換えた設定で描く（[`with_render_deps`]。段階 A0 / A1 のキャッシュのキー（`hash_for_phase`）
//!   に反映される）。`render_deps` はアプリが決める値で、利用者の設定（貼り付け・Undo・UI から送られた
//!   設定）では変えない。
//! - **保存**: 現像のセッションからの保存は、実際に使った依存を含めて記録する。保存された `render_deps` が
//!   空の写真（これまでの写真）は、**開いたときにメモリの中で今の値で埋め、次に保存したときに記録する**
//!   （開いただけでは履歴を増やさず、未調整の写真の設定を保存しない。SCL-02）。保存された値と今の値が
//!   違う（LibRaw を更新した・OS ごとに版が違うなど）ときは警告する（色が変わりうる。IQ-08）。
//! - **書き出し**: 使った依存を結果（[`crate::ExportOutcome::Written`] の `render_deps`）に記録する。
//! - **L0 / L1 のキャッシュキー**（`previews::rendered_key`）は、写真を展開せずに求める必要があるため、
//!   カメラ行列の内容のハッシュの代わりに RAW デコーダの識別子を使う（LibRaw の内蔵の表の行列は機種と
//!   デコーダの版で、DNG の ColorMatrix はファイルの内容（リビジョン）で決まるため）。LibRaw を更新すると
//!   キーが変わり、古い色のサムネイル・プレビューは使われなくなる。

use genzo_model::{DataRef, DevelopSettings, PhotoMetadata, RenderDeps};
use genzo_raw::CamXyzSource;
use sha2::{Digest, Sha256};

/// 展開した RAW で実際に使う依存（このモジュールの doc）。
pub(crate) fn actual_render_deps(
    cam_xyz: Option<&[[f32; 3]; 3]>,
    source: Option<CamXyzSource>,
    metadata: &PhotoMetadata,
    decoder_id: Option<&str>,
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
    RenderDeps {
        camera_profile,
        lens_profile: None,
        raw_decoder: decoder_id.unwrap_or_default().to_owned(),
    }
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

/// 設定の `render_deps` を `deps` に置き換えたもの（描画に使う設定）。
pub(crate) fn with_render_deps(settings: &DevelopSettings, deps: &RenderDeps) -> DevelopSettings {
    let mut s = settings.clone();
    s.render_deps = deps.clone();
    s
}

/// 保存された依存が、今の依存と違うか（空なら違うとしない。空は「まだ記録していない」）。
pub(crate) fn recorded_deps_differ(recorded: &RenderDeps, actual: &RenderDeps) -> bool {
    *recorded != RenderDeps::default() && recorded != actual
}

/// 保存された依存と今の依存が違うときの警告の文。
pub(crate) fn deps_changed_message(recorded: &RenderDeps, actual: &RenderDeps) -> String {
    format!(
        "前回保存したときと、カメラ行列または RAW デコーダが違います（保存: {} / {}、今: {} / {}）。同じ設定でも色が変わる可能性があります",
        recorded.camera_profile.id,
        recorded.raw_decoder,
        actual.camera_profile.id,
        actual.raw_decoder
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: [[f32; 3]; 3] = [
        [0.7424, -0.2329, -0.0466],
        [-0.4598, 1.2471, 0.2347],
        [-0.0715, 0.1505, 0.6066],
    ];

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
        );
        assert!(d.camera_profile.id.starts_with("libraw_table:"), "{d:?}");
        assert!(d.camera_profile.id.contains("ILCE-7M4"), "{d:?}");
        assert_eq!(d.camera_profile.sha256.len(), 64);
        assert_eq!(d.raw_decoder, "libraw-0.21.2");
        // 設定として正しい（検証を通る）。
        let s = with_render_deps(&DevelopSettings::default(), &d);
        s.validate().unwrap();
        // 行列が違えばハッシュも違う。
        let mut m2 = M;
        m2[0][0] += 0.03;
        let d2 = actual_render_deps(
            Some(&m2),
            Some(CamXyzSource::LibRawTable),
            &meta(),
            Some("libraw-0.21.2"),
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
        );
        assert!(
            dng.camera_profile
                .id
                .starts_with("dng_color_matrix1_illuminant21:")
        );
        assert_eq!(dng.raw_decoder, "");
        // 行列がない。
        let none = actual_render_deps(None, Some(CamXyzSource::None), &meta(), Some("x"));
        assert_eq!(none.camera_profile, DataRef::new("none", ""));
    }

    #[test]
    fn empty_recorded_deps_are_filled_without_warning() {
        let actual = actual_render_deps(Some(&M), None, &meta(), Some("libraw-0.21.2"));
        assert!(!recorded_deps_differ(&RenderDeps::default(), &actual));
        assert!(!recorded_deps_differ(&actual, &actual));
        let mut old = actual.clone();
        old.raw_decoder = "libraw-0.20.0".into();
        assert!(recorded_deps_differ(&old, &actual));
        assert!(deps_changed_message(&old, &actual).contains("libraw-0.20.0"));
    }
}
