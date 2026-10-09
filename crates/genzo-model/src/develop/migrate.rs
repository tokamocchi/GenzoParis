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

use super::DevelopError;
use crate::CURRENT_SCHEMA_VERSION;

/// 1 つ前のスキーマの JSON を、次のスキーマの JSON に変換する関数。
///
/// `schema_version` の書き換えは呼び出し側で行うため、関数の中では行わなくてよい。
pub type Migration = fn(Value) -> Result<Value, String>;

/// マイグレーション関数の表。`MIGRATIONS[i]` は schema_version `i + 1` → `i + 2` の変換。
///
/// 現在は v1 だけなので空。
pub const MIGRATIONS: &[Migration] = &[];

// 表の長さとスキーマのバージョンが食い違わないことをコンパイル時に確かめる。
const _: () = assert!(MIGRATIONS.len() as u32 + 1 == CURRENT_SCHEMA_VERSION);

/// `process_version` がない JSON に補う値（そのスキーマが作られたときの処理バージョン）。
///
/// 欠けている値を「現在の」処理バージョンで補うと、アプリの更新で結果が変わってしまう
/// （IQ-08・DATA-09）ため、スキーマのバージョンごとに固定の値を使う。
fn process_version_when_missing(schema_version: u32) -> u32 {
    // v1 のスキーマは処理バージョン 1 とともに作られた。
    // スキーマを上げて処理バージョンの既定が変わるときは、ここに分岐を追加する。
    debug_assert!(schema_version >= 1);
    1
}

/// JSON の値を現在のスキーマへ移行する。
pub(crate) fn migrate_to_current(value: Value) -> Result<Value, DevelopError> {
    migrate_with(value, CURRENT_SCHEMA_VERSION, MIGRATIONS)
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
