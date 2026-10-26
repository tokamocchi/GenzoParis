//! 現像設定のスキーマのマイグレーション（docs/04_architecture.md の 2.5 節）。
//!
//! 古い `schema_version` の JSON を読み込んだときは、マイグレーション関数を順番に適用して
//! 現在の形式（[`CURRENT_SCHEMA_VERSION`]）に変換する。未来のバージョン（このアプリより
//! 新しいアプリが保存したもの）は読めないため、エラーにする。
//!
//! # スキーマを変えるときの手順
//! 1. [`CURRENT_SCHEMA_VERSION`] を 1 上げる。
//! 2. 旧形式の JSON（[`Value`]）を新形式に変換する関数を書き、[`MIGRATIONS`] の末尾に追加する。
//! 3. 旧形式の JSON を読み込むテストを追加する（古いテストは消さない）。

use serde_json::Value;

use super::{DevelopError, Sharpening};
use crate::CURRENT_SCHEMA_VERSION;

/// 1 つ前のスキーマの JSON を、次のスキーマの JSON に変換する関数。
///
/// `schema_version` の書き換えは呼び出し側で行うため、関数の中では行わなくてよい。
pub type Migration = fn(Value) -> Result<Value, String>;

/// マイグレーション関数の表。`MIGRATIONS[i]` は schema_version `i + 1` → `i + 2` の変換。
///
/// - 1 → 2（`migrate_v1_to_v2`）: レンズ補正の手動の項目を加えた（DEV-14）。v1 の機能のパッケージの間
///   （配布の前）は、後のパッケージが足す項目も版 2 に入れる（決定事項ログ No.120）: ハイライト復元
///   （`highlight_recovery`。DEV-06）、ノイズ軽減のディテール・コントラスト（`detail.noise_reduction.
///   luminance_detail`・`luminance_contrast`・`color_detail`。DEV-13）、カラーミキサー・カラーグレーディング・白黒
///   （`color.mixer`・`color.grading`・`color.black_white`。DEV-09・DEV-10・DEV-25）、テクスチャ・明瞭度・かすみの除去
///   （`presence`。DEV-11）、効果（`effects` の切り抜き後の周辺光量補正・粒子。DEV-24）。
/// - 2 → 3（`migrate_v2_to_v3`）: 変形（遠近補正。`transform`。DEV-16）を加えた。
pub const MIGRATIONS: &[Migration] = &[migrate_v1_to_v2, migrate_v2_to_v3];

/// schema_version 1 → 2: レンズ補正の手動の項目（`lens.distortion`・`lens.vignetting_amount`・
/// `lens.vignetting_midpoint`・`lens.ca_red_cyan`・`lens.ca_blue_yellow`）、ハイライト復元
/// （`highlight_recovery`）、ノイズ軽減のディテール・コントラスト（`detail.noise_reduction` の
/// `luminance_detail`・`luminance_contrast`・`color_detail`）、カラーミキサー・カラーグレーディング・白黒（`color` の
/// `mixer`・`grading`・`black_white`）、テクスチャ・明瞭度・かすみの除去（`presence`）、効果（`effects`）を加えた。
/// シャープ（`detail.sharpening`）は版 1 からある項目で、ディテールの既定値を 0 から 25 にした（JSON にディテールが
/// なければ 25 で補う。旧版が書いた旧既定の 0 の読み替えは [`read_changed_defaults_in_v2`]）。
///
/// 新しい項目は JSON になければ既定値（何もしない値）で補うので、値は書き換えない。`lens` が
/// オブジェクトでない（型の誤り）ときは、そのまま残して型の読み込みでエラーにする。
fn migrate_v1_to_v2(value: Value) -> Result<Value, String> {
    Ok(value)
}

/// schema_version 2 → 3: 変形（遠近補正。`transform`: `vertical`・`horizontal`・`aspect`・`scale`・`offset_x`・
/// `offset_y`。DEV-16。決定事項ログ No.202）を加えた。JSON になければ既定値（何もしない値）で補うので、値は
/// 書き換えない。版 2 の JSON の `transform`（ない）以外の項目はそのまま。
fn migrate_v2_to_v3(value: Value) -> Result<Value, String> {
    Ok(value)
}

// 表の長さとスキーマのバージョンが食い違わないことをコンパイル時に確かめる。
const _: () = assert!(MIGRATIONS.len() as u32 + 1 == CURRENT_SCHEMA_VERSION);

/// `process_version` がない JSON に補う値（そのスキーマが作られたときの処理バージョン）。
///
/// 欠けている値を「現在の」処理バージョンで補うと、アプリの更新で結果が変わってしまう
/// （IQ-08・DATA-09）ため、スキーマのバージョンごとに固定の値を使う。
fn process_version_when_missing(schema_version: u32) -> u32 {
    // v1 のスキーマは処理バージョン 1 とともに、v2・v3 のスキーマは処理バージョン 2 とともに作られた。
    debug_assert!(schema_version >= 1);
    match schema_version {
        1 => 1,
        _ => 2,
    }
}

/// JSON の値を現在のスキーマへ移行する。
pub(crate) fn migrate_to_current(mut value: Value) -> Result<Value, DevelopError> {
    read_changed_defaults_in_v2(&mut value);
    read_provisional_masks_in_v2(&mut value);
    migrate_with(value, CURRENT_SCHEMA_VERSION, MIGRATIONS)
}

/// スキーマの版 2 のうちに形を変えたマスクの読み替え（masks のパッケージ。DEV-18。決定事項ログ No.150）。
///
/// それより前の版のマスク（`masks[i]`）は仮の形で、形を 1 つだけ `mask` に持ち、円形の反転は形の中の `invert`
/// だった（`{"enabled": true, "mask": {"radial": {..., "invert": true}}, "delta": {...}}`）。画面にはなく、処理
/// バージョン 2 でも適用しなかった。これを、加える部品 1 つの今の形（`{"components": [{"mode": "add", "invert":
/// true, "shape": {"radial": {...}}}]}`）に読み替える。差分の項目（`delta`）は今の形にもあるので、そのまま残す。
/// `components` があるマスク（今の形）と、型の誤り（オブジェクトでない）は書き換えない（型への読み込みでエラーに
/// する）。スキーマの版 1・2 の JSON に、マイグレーションの表の前に当てる。
fn read_provisional_masks_in_v2(value: &mut Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    let schema = obj.get("schema_version").and_then(Value::as_u64);
    if !matches!(schema, Some(1..=2)) {
        return;
    }
    let Some(masks) = obj.get_mut("masks").and_then(Value::as_array_mut) else {
        return;
    };
    for m in masks {
        let Some(m) = m.as_object_mut() else {
            continue;
        };
        if m.contains_key("components") {
            continue;
        }
        let Some(mut shape) = m.remove("mask") else {
            continue;
        };
        let invert = shape
            .as_object_mut()
            .and_then(|s| s.get_mut("radial"))
            .and_then(Value::as_object_mut)
            .and_then(|r| r.remove("invert"))
            .unwrap_or(Value::Bool(false));
        let mut component = serde_json::Map::new();
        component.insert("mode".to_owned(), Value::from("add"));
        component.insert("invert".to_owned(), invert);
        component.insert("shape".to_owned(), shape);
        m.insert(
            "components".to_owned(),
            Value::Array(vec![Value::Object(component)]),
        );
    }
}

/// スキーマの版 2 のうちに既定値を変えた項目の読み替え。v1 の機能のパッケージの間（配布の前）は、スキーマの版を
/// 上げずに版 2 に項目を足すため（決定事項ログ No.120・No.142）、マイグレーションの表の前に、スキーマの版 1・2 の
/// JSON に当てる（後でスキーマの版を上げても、版 1・2 の JSON にはこの読み替えを先に当てる）。
///
/// - シャープのディテール（`detail.sharpening.detail`）: 既定を 0 から 25 にした（finish-detail。No.138）。
///   それより前の版は `presence` を書かず、シャープの 4 つの値を必ず書いていたので、`presence` がなく、適用量が 0
///   （または項目がない）でディテールが 0 の JSON は、旧既定のままとみなして 25 に読み替える。適用量が 0 なら
///   ディテールは効かないので、画素の結果は変わらない。適用量が 0 でないときは、利用者が決めた値として保つ。
///
/// 型の誤り（オブジェクトでない・数でない）は書き換えずに残し、型への読み込みでエラーにする。
fn read_changed_defaults_in_v2(value: &mut Value) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    let schema = obj.get("schema_version").and_then(Value::as_u64);
    if !matches!(schema, Some(1..=2)) || obj.contains_key("presence") {
        return;
    }
    let Some(sharpening) = obj
        .get_mut("detail")
        .and_then(Value::as_object_mut)
        .and_then(|d| d.get_mut("sharpening"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let amount_is_zero = sharpening
        .get("amount")
        .is_none_or(|a| a.as_f64() == Some(0.0));
    if amount_is_zero && sharpening.get("detail").and_then(Value::as_f64) == Some(0.0) {
        sharpening.insert(
            "detail".to_owned(),
            Value::from(f64::from(Sharpening::DEFAULT_DETAIL)),
        );
    }
}

/// JSON の値を `target` のスキーマへ移行する（テストで表を差し替えられるように分けている）。
pub(crate) fn migrate_with(
    mut value: Value,
    target: u32,
    migrations: &[Migration],
) -> Result<Value, DevelopError> {
    debug_assert_eq!(migrations.len() as u32 + 1, target);
    let obj = value.as_object_mut().ok_or(DevelopError::NotAnObject)?;
    let version = match obj.get("schema_version") {
        None => return Err(DevelopError::MissingSchemaVersion),
        Some(v) => v
            .as_u64()
            .ok_or_else(|| DevelopError::InvalidSchemaVersion(v.to_string()))?,
    };
    if version == 0 {
        return Err(DevelopError::InvalidSchemaVersion(version.to_string()));
    }
    if version > u64::from(target) {
        return Err(DevelopError::FutureSchemaVersion {
            found: version,
            supported: target,
        });
    }
    // version は 1..=target なので u32 に収まる。
    let mut version = version as u32;
    if !obj.contains_key("process_version") {
        obj.insert(
            "process_version".to_owned(),
            Value::from(process_version_when_missing(version)),
        );
    }
    while version < target {
        let next = version + 1;
        let failed = |message: &str| DevelopError::Migration {
            from: version,
            to: next,
            message: message.to_owned(),
        };
        // 表の長さはコンパイル時に確かめているが、表を差し替えた場合も panic せずにエラーにする。
        let migrate = migrations
            .get((version - 1) as usize)
            .ok_or_else(|| failed("マイグレーション関数がありません"))?;
        value = migrate(value).map_err(|message| failed(&message))?;
        let obj = value
            .as_object_mut()
            .ok_or_else(|| failed("変換結果がオブジェクトではありません"))?;
        // process_version を落とすと、型への読み込みで「現在の」処理バージョンが補われて
        // しまう（IQ-08 に反する）ため、エラーにする。
        if !obj.contains_key("process_version") {
            return Err(failed("変換結果に process_version がありません"));
        }
        obj.insert("schema_version".to_owned(), Value::from(next));
        version = next;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// テスト用: v1 の `exposure` を v2 の `exposure_ev` に名前を変える。
    fn rename_exposure(mut v: Value) -> Result<Value, String> {
        let obj = v.as_object_mut().ok_or("not an object")?;
        if let Some(e) = obj.remove("exposure") {
            obj.insert("exposure_ev".to_owned(), e);
        }
        Ok(v)
    }

    /// テスト用: v2 → v3 で contrast を 2 倍にする。
    fn double_contrast(mut v: Value) -> Result<Value, String> {
        let obj = v.as_object_mut().ok_or("not an object")?;
        if let Some(c) = obj.get("contrast").and_then(Value::as_f64) {
            obj.insert("contrast".to_owned(), json!(c * 2.0));
        }
        Ok(v)
    }

    fn fail(_: Value) -> Result<Value, String> {
        Err("壊れています".to_owned())
    }

    #[test]
    fn migrations_are_applied_in_order() {
        let table: &[Migration] = &[rename_exposure, double_contrast];
        let v1 =
            json!({"schema_version": 1, "process_version": 1, "exposure": 0.5, "contrast": 10.0});
        let v3 = migrate_with(v1, 3, table).unwrap();
        assert_eq!(
            v3,
            json!({"schema_version": 3, "process_version": 1, "exposure_ev": 0.5, "contrast": 20.0})
        );
        // 途中のバージョンからも移行できる。
        let v2 = json!({"schema_version": 2, "process_version": 1, "contrast": 10.0});
        let v3 = migrate_with(v2, 3, table).unwrap();
        assert_eq!(v3["contrast"], json!(20.0));
        assert_eq!(v3["schema_version"], json!(3));
    }

    /// 実際の表: v1（MVP）の設定を v2 に移行すると、値はそのままで、処理バージョンも保つ
    /// （レンズ補正の手動の項目は読み込みで既定値になる）。
    #[test]
    fn v1_settings_migrate_to_v2_without_changing_values() {
        let v1 = json!({
            "schema_version": 1,
            "process_version": 1,
            "exposure_ev": 0.5,
            "lens": {"profile_enabled": true, "distortion_scale": 80.0}
        });
        let v2 = migrate_to_current(v1).unwrap();
        assert_eq!(v2["schema_version"], json!(crate::CURRENT_SCHEMA_VERSION));
        assert_eq!(v2["process_version"], json!(1));
        let s: crate::DevelopSettings = serde_json::from_value(v2).unwrap();
        assert_eq!(s.exposure_ev, 0.5);
        assert!(s.lens.profile_enabled);
        assert_eq!(s.lens.distortion_scale, 80.0);
        assert_eq!(s.lens.distortion, 0.0);
        assert_eq!(s.lens.vignetting_midpoint, 50.0);
        // ハイライト復元・ノイズ軽減の新しい項目も既定値（復元しない・ディテール 50）。値は保つ。
        assert_eq!(s.highlight_recovery, crate::HighlightRecovery::default());
        assert_eq!(s.detail.noise_reduction.luminance_detail, 50.0);
        assert_eq!(s.detail.noise_reduction.color_detail, 50.0);
        let v1 = json!({
            "schema_version": 1,
            "process_version": 1,
            "detail": {"noise_reduction": {"luminance": 20.0, "color": 30.0}}
        });
        let s: crate::DevelopSettings =
            serde_json::from_value(migrate_to_current(v1).unwrap()).unwrap();
        assert_eq!(s.process_version, 1);
        assert_eq!(s.detail.noise_reduction.luminance, 20.0);
        assert_eq!(s.detail.noise_reduction.color, 30.0);
        assert_eq!(s.detail.noise_reduction.luminance_contrast, 0.0);
        // process_version のない v1 は 1、v2 は 2 で補う（そのスキーマが作られたときの版。IQ-08）。
        let v1 = migrate_to_current(json!({"schema_version": 1})).unwrap();
        assert_eq!(v1["process_version"], json!(1));
        let v2 = migrate_to_current(json!({"schema_version": 2})).unwrap();
        assert_eq!(v2["process_version"], json!(2));
        let v3 = migrate_to_current(json!({"schema_version": 3})).unwrap();
        assert_eq!(v3["process_version"], json!(2));
    }

    /// 実際の表: v2（変形を書かない版）の設定を v3 に移行すると、値はそのままで、変形は既定値（何もしない）。
    #[test]
    fn v2_settings_migrate_to_v3_with_identity_transform() {
        let v2 = json!({
            "schema_version": 2,
            "process_version": 2,
            "geometry": {"angle_deg": 1.5},
            "lens": {"distortion": 10.0}
        });
        let v3 = migrate_to_current(v2).unwrap();
        assert_eq!(v3["schema_version"], json!(3));
        assert_eq!(v3["process_version"], json!(2));
        let s: crate::DevelopSettings = serde_json::from_value(v3).unwrap();
        assert_eq!(s.geometry.angle_deg, 1.5);
        assert_eq!(s.lens.distortion, 10.0);
        assert!(s.transform.is_identity());
        assert_eq!(s.transform.scale, 100.0);
        // 版 3 が書いた変形の値はそのまま読む。
        let v3 = json!({
            "schema_version": 3,
            "process_version": 2,
            "transform": {"vertical": -20.0, "scale": 110.0}
        });
        let s: crate::DevelopSettings =
            serde_json::from_value(migrate_to_current(v3).unwrap()).unwrap();
        assert_eq!(s.transform.vertical, -20.0);
        assert_eq!(s.transform.horizontal, 0.0);
        assert_eq!(s.transform.scale, 110.0);
        assert!(!s.transform.is_identity());
    }

    /// finish-detail のパッケージより前の版（`presence` を書かない版）が保存した設定では、シャープの旧既定
    /// （適用量 0・ディテール 0）を新しい既定（ディテール 25）に読み替える。適用量が 0 でなければ値を保つ。
    /// 新しい版が書いた設定（`presence` がある）は書き換えない。
    #[test]
    fn old_sharpening_detail_default_is_read_as_the_new_default() {
        let sharpening = |v: &Value| v["detail"]["sharpening"]["detail"].as_f64();
        // 旧版（スキーマの版 1・2）のシリアライザは、シャープの 4 つの値を必ず書いていた。
        for schema in [1, 2] {
            let old = json!({
                "schema_version": schema,
                "process_version": schema,
                "detail": {
                    "sharpening": {"amount": 0.0, "radius": 1.0, "detail": 0.0, "masking": 0.0},
                    "noise_reduction": {"luminance": 0.0, "color": 0.0}
                }
            });
            let v = migrate_to_current(old).unwrap();
            assert_eq!(sharpening(&v), Some(25.0), "schema {schema}");
            // ほかの値は変えない。
            assert_eq!(v["detail"]["sharpening"]["radius"], json!(1.0));
            assert_eq!(v["detail"]["sharpening"]["amount"], json!(0.0));
            assert_eq!(v["process_version"], json!(schema));
            // 適用量が 0 でなければ、利用者が決めた値として保つ。
            let old = json!({
                "schema_version": schema,
                "detail": {"sharpening": {"amount": 40.0, "radius": 1.0, "detail": 0.0, "masking": 0.0}}
            });
            assert_eq!(sharpening(&migrate_to_current(old).unwrap()), Some(0.0));
            // ディテールが 0 でなければ保つ。
            let old = json!({
                "schema_version": schema,
                "detail": {"sharpening": {"amount": 0.0, "detail": 10.0}}
            });
            assert_eq!(sharpening(&migrate_to_current(old).unwrap()), Some(10.0));
        }
        // 適用量のない旧版の JSON（既定の 0）も読み替える。
        let old = json!({"schema_version": 2, "detail": {"sharpening": {"detail": 0}}});
        assert_eq!(sharpening(&migrate_to_current(old).unwrap()), Some(25.0));
        // 新しい版が書いた設定（`presence` がある）は、適用量 0・ディテール 0 でも保つ。
        let new = json!({
            "schema_version": 2,
            "presence": {"texture": 0.0, "clarity": 0.0, "dehaze": 0.0},
            "detail": {"sharpening": {"amount": 0.0, "radius": 1.0, "detail": 0.0, "masking": 0.0}}
        });
        assert_eq!(sharpening(&migrate_to_current(new).unwrap()), Some(0.0));
        // 型の誤り（オブジェクトでない）はそのまま残して、型の読み込みでエラーにする。
        for broken in [
            json!({"schema_version": 2, "detail": 3}),
            json!({"schema_version": 2, "detail": {"sharpening": "x"}}),
            json!({"schema_version": 2, "detail": {"sharpening": {"amount": "0", "detail": 0}}}),
        ] {
            let v = migrate_to_current(broken.clone()).unwrap();
            assert_eq!(v["detail"], broken["detail"]);
        }
    }

    /// masks のパッケージより前の仮の形のマスク（形が 1 つの `mask`・円形の反転が形の中）は、加える部品 1 つの今の
    /// 形に読み替える。今の形のマスクは書き換えない。
    #[test]
    fn provisional_masks_are_read_as_one_component() {
        let old = json!({
            "schema_version": 2,
            "process_version": 2,
            "masks": [
                {"enabled": true,
                 "mask": {"radial": {"cx": 0.5, "cy": 0.4, "rx": 0.2, "ry": 0.1, "angle_deg": 10.0,
                                     "feather": 30.0, "invert": true}},
                 "delta": {"exposure_ev": 0.5, "saturation": -20.0}},
                {"mask": {"linear": {"x0": 0.0, "y0": 0.0, "x1": 0.0, "y1": 1.0}}},
                {"components": [{"mode": "subtract", "shape": {"linear": {"x0": 0, "y0": 0, "x1": 1, "y1": 0}}}]},
                7
            ]
        });
        let v = migrate_to_current(old).unwrap();
        let m = &v["masks"];
        assert!(m[0].get("mask").is_none());
        assert_eq!(
            m[0]["components"],
            json!([{"mode": "add", "invert": true, "shape": {"radial": {
                "cx": 0.5, "cy": 0.4, "rx": 0.2, "ry": 0.1, "angle_deg": 10.0, "feather": 30.0}}}])
        );
        assert_eq!(m[0]["delta"]["exposure_ev"], json!(0.5));
        assert_eq!(m[1]["components"][0]["invert"], json!(false));
        assert_eq!(m[2]["components"][0]["mode"], json!("subtract"));
        // 型の誤りはそのまま残す。
        assert_eq!(m[3], json!(7));
        // 読み替えた設定を型に読める（壊れた要素を除いて）。
        let mut v = v;
        v["masks"].as_array_mut().unwrap().pop();
        let s: crate::DevelopSettings = serde_json::from_value(v).unwrap();
        assert_eq!(s.masks.len(), 3);
        assert!(s.masks[0].components[0].invert);
        assert_eq!(s.masks[0].delta.saturation, -20.0);
        assert!(matches!(
            s.masks[1].components[0].shape,
            crate::MaskShape::Linear { y1, .. } if y1 == 1.0
        ));
    }

    #[test]
    fn future_version_is_rejected() {
        let err = migrate_with(json!({"schema_version": 2}), 1, &[]).unwrap_err();
        assert!(matches!(
            err,
            DevelopError::FutureSchemaVersion {
                found: 2,
                supported: 1
            }
        ));
    }

    #[test]
    fn missing_or_invalid_version_is_rejected() {
        assert!(matches!(
            migrate_with(json!({"exposure_ev": 1.0}), 1, &[]),
            Err(DevelopError::MissingSchemaVersion)
        ));
        assert!(matches!(
            migrate_with(json!({"schema_version": 0}), 1, &[]),
            Err(DevelopError::InvalidSchemaVersion(_))
        ));
        assert!(matches!(
            migrate_with(json!({"schema_version": "1"}), 1, &[]),
            Err(DevelopError::InvalidSchemaVersion(_))
        ));
        assert!(matches!(
            migrate_with(json!({"schema_version": -1}), 1, &[]),
            Err(DevelopError::InvalidSchemaVersion(_))
        ));
        assert!(matches!(
            migrate_with(json!([1, 2]), 1, &[]),
            Err(DevelopError::NotAnObject)
        ));
    }

    #[test]
    fn migration_failure_reports_versions() {
        let err = migrate_with(json!({"schema_version": 1}), 2, &[fail]).unwrap_err();
        match err {
            DevelopError::Migration { from, to, message } => {
                assert_eq!((from, to), (1, 2));
                assert_eq!(message, "壊れています");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn migration_must_keep_process_version() {
        fn drop_pv(mut v: Value) -> Result<Value, String> {
            v.as_object_mut().unwrap().remove("process_version");
            Ok(v)
        }
        let err = migrate_with(json!({"schema_version": 1}), 2, &[drop_pv]).unwrap_err();
        assert!(matches!(
            err,
            DevelopError::Migration { from: 1, to: 2, .. }
        ));
    }

    #[test]
    fn migration_result_must_be_an_object() {
        fn to_array(_: Value) -> Result<Value, String> {
            Ok(json!([]))
        }
        let err = migrate_with(json!({"schema_version": 1}), 2, &[to_array]).unwrap_err();
        assert!(matches!(
            err,
            DevelopError::Migration { from: 1, to: 2, .. }
        ));
    }

    #[test]
    fn non_integer_schema_version_is_rejected() {
        assert!(matches!(
            migrate_with(json!({"schema_version": 1.0}), 1, &[]),
            Err(DevelopError::InvalidSchemaVersion(_))
        ));
        assert!(matches!(
            migrate_with(json!({"schema_version": null}), 1, &[]),
            Err(DevelopError::InvalidSchemaVersion(_))
        ));
    }

    #[test]
    fn missing_process_version_is_filled_with_the_schema_default() {
        let v = migrate_with(json!({"schema_version": 1}), 1, &[]).unwrap();
        assert_eq!(v["process_version"], json!(1));
        // 既にある値は変えない。
        let v = migrate_with(json!({"schema_version": 1, "process_version": 7}), 1, &[]).unwrap();
        assert_eq!(v["process_version"], json!(7));
    }
}
