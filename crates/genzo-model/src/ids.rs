//! カタログの行の ID（docs/04_architecture.md の 3.1 節の ER 図）。
//!
//! どれも SQLite の `INTEGER PRIMARY KEY`（i64）を包む newtype で、種類の違う ID を
//! 取り違えないようにする。JSON では数値そのもの（`serde(transparent)`）として表す。

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl $name {
            /// 値から ID を作る。
            pub const fn new(value: i64) -> Self {
                Self(value)
            }

            /// ID の値を返す。
            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl From<i64> for $name {
            fn from(value: i64) -> Self {
                Self(value)
            }
        }

        impl From<$name> for i64 {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

define_id!(
    /// ボリューム（`volume` テーブル）の ID。
    VolumeId
);
define_id!(
    /// フォルダ（`folder` テーブル）の ID。
    FolderId
);
define_id!(
    /// ファイル（`file` テーブル）の ID。
    FileId
);
define_id!(
    /// 実体（`asset` テーブル）の ID。RAW と JPEG のペアなど、複数のファイルをまとめる単位。
    AssetId
);
define_id!(
    /// 現像のバリエーション（`variant` テーブル）の ID。マスターと仮想コピーを含む。
    /// グリッドに表示し、レーティング・フラグ・キーワードを付ける単位。
    VariantId
);
define_id!(
    /// キーワード（`keyword` テーブル）の ID。
    KeywordId
);
define_id!(
    /// コレクション（`collection` テーブル）の ID。
    CollectionId
);
define_id!(
    /// 現像の履歴（`history_entry` テーブル）の ID。
    HistoryEntryId
);
define_id!(
    /// スナップショット（`snapshot` テーブル）の ID。
    SnapshotId
);
define_id!(
    /// スタック（`stack` テーブル）の ID。
    StackId
);
define_id!(
    /// ファイル操作の記録（`file_op` テーブル。04 の 6.4 節）の ID。
    FileOpId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_serializes_as_plain_number() {
        let id = VariantId::new(42);
        assert_eq!(serde_json::to_string(&id).unwrap(), "42");
        let back: VariantId = serde_json::from_str("42").unwrap();
        assert_eq!(back, id);
        assert_eq!(back.get(), 42);
        assert_eq!(i64::from(back), 42);
        assert_eq!(VariantId::from(42), id);
        assert_eq!(id.to_string(), "42");
    }

    #[test]
    fn ids_are_ordered_by_value() {
        assert!(AssetId::new(1) < AssetId::new(2));
        assert!(FileId::new(-1) < FileId::new(0));
    }
}
