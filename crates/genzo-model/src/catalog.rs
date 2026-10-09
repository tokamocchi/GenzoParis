//! カタログの列挙値（docs/04_architecture.md の 3.1 節の ER 図・3.5 節の制約・6.4 節）。
//!
//! 文字列の列に入れるものは [`as_str`](AssetKind::as_str) と [`FromStr`] で DB の文字列表現
//! （例: `"photo"`、`"sidecar_jpeg"`）と相互に変換する。JSON での表現も同じ文字列にする。
//! 整数の列に入れるもの（[`Rating`]、[`Flag`]）は整数で表す。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 文字列や整数から列挙値への変換に失敗したときのエラー。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{type_name} として不正な値です: {value:?}")]
pub struct ParseEnumError {
    /// 変換しようとした型の名前。
    pub type_name: &'static str,
    /// 変換できなかった値。
    pub value: String,
}

impl ParseEnumError {
    fn new(type_name: &'static str, value: impl ToString) -> Self {
        Self {
            type_name,
            value: value.to_string(),
        }
    }
}

/// DB に文字列で保存する列挙型を定義する。
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $s:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $( $(#[$vmeta])* #[serde(rename = $s)] $variant, )+
        }

        impl $name {
            /// すべての値（定義の順）。
            pub const ALL: &'static [Self] = &[ $( Self::$variant ),+ ];

            /// DB に保存する文字列表現を返す。
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Self::$variant => $s, )+
                }
            }
        }

        impl FromStr for $name {
            type Err = ParseEnumError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $( $s => Ok(Self::$variant), )+
                    _ => Err(ParseEnumError::new(stringify!($name), s)),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(
    /// 実体の種類（`asset.kind`）。写真と動画を同じテーブルで扱う（04 の 3.1 節）。
    AssetKind {
        /// 写真（RAW・JPEG・TIFF など）。
        Photo => "photo",
        /// 動画。カタログ管理だけを行い、編集はしない。
        Video => "video",
    }
);

string_enum!(
    /// カラーラベル（`variant.color_label`）。ラベルなしは `Option::None` で表す。
    ColorLabel {
        /// 赤。
        Red => "red",
        /// 黄。
        Yellow => "yellow",
        /// 緑。
        Green => "green",
        /// 青。
        Blue => "blue",
        /// 紫。
        Purple => "purple",
    }
);

string_enum!(
    /// ファイルの役割（`file.role`）。
    FileRole {
        /// 主となるファイル（RAW、または RAW のない写真・動画そのもの）。
        Primary => "primary",
        /// RAW と同時に記録された JPEG。
        SidecarJpeg => "sidecar_jpeg",
    }
);

string_enum!(
    /// ファイルの状態（`file.status`。04 の 3.3 節）。
    FileStatus {
        /// 存在し、読める。
        Ok => "ok",
        /// 見つからない（移動・削除・ボリュームの取り外し）。
        Missing => "missing",
        /// 存在するが、読み取りや解析に失敗した。
        Error => "error",
    }
);

string_enum!(
    /// 撮影日時の UTC を推定するのに使ったオフセットの出どころ（`asset.tz_source`。レビュー R-18）。
    TzSource {
        /// ファイルのメタデータ（EXIF の OffsetTimeOriginal、動画の作成日時など）に記録されていた。
        Exif => "exif",
        /// オフセットが記録されていなかったため、ユーザーが設定した既定のタイムゾーンを使った。
        UserDefault => "user_default",
        /// ユーザーがこの写真・動画に明示的に設定した（LIB-16）。
        UserSet => "user_set",
    }
);

string_enum!(
    /// ファイル操作の種類（`file_op.kind`。04 の 6.4 節）。
    FileOpKind {
        /// 別のフォルダへの移動。
        Move => "move",
        /// 名前の変更。
        Rename => "rename",
        /// OS のゴミ箱への移動。
        Trash => "trash",
    }
);

string_enum!(
    /// ファイル操作の状態（`file_op.state`。04 の 6.4 節の状態遷移）。
    FileOpState {
        /// 予定を記録した（まだ OS の操作をしていない）。
        Planned => "planned",
        /// OS の操作を実行中。
        Executing => "executing",
        /// OS の操作が成功し、カタログも更新した。
        Done => "done",
        /// OS の操作が失敗した（元の状態のまま）。
        Failed => "failed",
    }
);

impl FileOpState {
    /// これ以上遷移しない状態（`done` / `failed`）か。
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }

    /// `self` から `next` へ遷移してよいか（04 の 6.4 節の状態遷移図）。
    ///
    /// 通常の遷移は planned → executing → done / failed。起動時の確認（DATA-07）では、
    /// planned または executing のまま残った記録を、実際のファイルの場所を見て done か
    /// failed に確定させるため、planned → done / failed も許す。
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Planned, Self::Executing)
                | (Self::Planned, Self::Done)
                | (Self::Planned, Self::Failed)
                | (Self::Executing, Self::Done)
                | (Self::Executing, Self::Failed)
        )
    }
}

string_enum!(
    /// コレクションの種類（`collection.kind`）。
    CollectionKind {
        /// 手動で variant を入れるコレクション。
        Manual => "manual",
        /// 条件を保存し、自動で更新されるコレクション（LIB-11）。
        Smart => "smart",
        /// コレクションをまとめるセット（フォルダのようなもの）。
        Set => "set",
    }
);

/// レーティング（★0〜5。`variant.rating`。04 の 3.5 節の CHECK 制約）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(try_from = "u8", into = "u8")]
pub struct Rating(u8);

impl Rating {
    /// 最小値（評価なし）。
    pub const MIN: Rating = Rating(0);
    /// 最大値（★5）。
    pub const MAX: Rating = Rating(5);

    /// 0〜5 ならレーティングを作る。
    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX.0 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// 値（0〜5）を返す。
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Rating {
    type Error = ParseEnumError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value).ok_or_else(|| ParseEnumError::new("Rating", value))
    }
}

impl TryFrom<i64> for Rating {
    type Error = ParseEnumError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u8::try_from(value)
            .ok()
            .and_then(Self::new)
            .ok_or_else(|| ParseEnumError::new("Rating", value))
    }
}

impl From<Rating> for u8 {
    fn from(rating: Rating) -> Self {
        rating.0
    }
}

impl From<Rating> for i64 {
    fn from(rating: Rating) -> Self {
        i64::from(rating.0)
    }
}

impl fmt::Display for Rating {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// フラグ（採用 / 不採用。`variant.flag`。04 の 3.5 節の CHECK 制約で -1 / 0 / 1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(try_from = "i8", into = "i8")]
#[repr(i8)]
pub enum Flag {
    /// 不採用。
    Rejected = -1,
    /// フラグなし。
    #[default]
    None = 0,
    /// 採用。
    Picked = 1,
}

impl Flag {
    /// DB に保存する整数値（-1 / 0 / 1）を返す。
    pub const fn as_i8(self) -> i8 {
        self as i8
    }
}

impl TryFrom<i8> for Flag {
    type Error = ParseEnumError;

    fn try_from(value: i8) -> Result<Self, Self::Error> {
        match value {
            -1 => Ok(Self::Rejected),
            0 => Ok(Self::None),
            1 => Ok(Self::Picked),
            _ => Err(ParseEnumError::new("Flag", value)),
        }
    }
}

impl TryFrom<i64> for Flag {
    type Error = ParseEnumError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        i8::try_from(value)
            .map_err(|_| ParseEnumError::new("Flag", value))
            .and_then(Self::try_from)
    }
}

impl From<Flag> for i8 {
    fn from(flag: Flag) -> Self {
        flag.as_i8()
    }
}

impl From<Flag> for i64 {
    fn from(flag: Flag) -> Self {
        i64::from(flag.as_i8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_enums_round_trip_through_db_strings() {
        fn check<T>(all: &[T])
        where
            T: Copy + PartialEq + fmt::Debug + FromStr<Err = ParseEnumError> + fmt::Display,
            T: Serialize + for<'de> Deserialize<'de>,
        {
            for &v in all {
                let s = v.to_string();
                assert_eq!(s.parse::<T>().unwrap(), v);
                // JSON でも同じ文字列を使う。
                assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{s}\""));
                assert_eq!(serde_json::from_str::<T>(&format!("\"{s}\"")).unwrap(), v);
            }
        }
        check(AssetKind::ALL);
        check(ColorLabel::ALL);
        check(FileRole::ALL);
        check(FileStatus::ALL);
        check(TzSource::ALL);
        check(FileOpKind::ALL);
        check(FileOpState::ALL);
        check(CollectionKind::ALL);
    }

    #[test]
    fn db_strings_match_the_er_diagram() {
        assert_eq!(AssetKind::Photo.as_str(), "photo");
        assert_eq!(AssetKind::Video.as_str(), "video");
        assert_eq!(FileRole::Primary.as_str(), "primary");
        assert_eq!(FileRole::SidecarJpeg.as_str(), "sidecar_jpeg");
        assert_eq!(FileStatus::Ok.as_str(), "ok");
        assert_eq!(FileStatus::Missing.as_str(), "missing");
        assert_eq!(FileStatus::Error.as_str(), "error");
        assert_eq!(TzSource::Exif.as_str(), "exif");
        assert_eq!(TzSource::UserDefault.as_str(), "user_default");
        assert_eq!(TzSource::UserSet.as_str(), "user_set");
        assert_eq!(FileOpKind::Trash.as_str(), "trash");
        assert_eq!(FileOpState::Executing.as_str(), "executing");
        assert_eq!(CollectionKind::Smart.as_str(), "smart");
        assert_eq!(ColorLabel::Purple.as_str(), "purple");
    }

    #[test]
    fn unknown_strings_are_rejected() {
        let err = "Photo".parse::<AssetKind>().unwrap_err();
        assert_eq!(err.type_name, "AssetKind");
        assert_eq!(err.value, "Photo");
        assert!("".parse::<FileStatus>().is_err());
        assert!(serde_json::from_str::<TzSource>("\"utc\"").is_err());
    }

    #[test]
    fn rating_accepts_only_zero_to_five() {
        for v in 0..=5u8 {
            assert_eq!(Rating::try_from(v).unwrap().get(), v);
        }
        assert!(Rating::try_from(6u8).is_err());
        assert!(Rating::try_from(-1i64).is_err());
        assert!(Rating::try_from(256i64).is_err());
        assert_eq!(Rating::default(), Rating::MIN);
        assert_eq!(serde_json::to_string(&Rating::MAX).unwrap(), "5");
        assert!(serde_json::from_str::<Rating>("6").is_err());
        assert_eq!(serde_json::from_str::<Rating>("3").unwrap().get(), 3);
    }

    #[test]
    fn flag_uses_minus_one_zero_one() {
        assert_eq!(Flag::Rejected.as_i8(), -1);
        assert_eq!(Flag::None.as_i8(), 0);
        assert_eq!(Flag::Picked.as_i8(), 1);
        assert_eq!(Flag::try_from(-1i8).unwrap(), Flag::Rejected);
        assert_eq!(Flag::try_from(1i64).unwrap(), Flag::Picked);
        assert!(Flag::try_from(2i8).is_err());
        assert!(Flag::try_from(1000i64).is_err());
        assert_eq!(Flag::default(), Flag::None);
        assert_eq!(serde_json::to_string(&Flag::Rejected).unwrap(), "-1");
        assert_eq!(serde_json::from_str::<Flag>("1").unwrap(), Flag::Picked);
        assert!(serde_json::from_str::<Flag>("2").is_err());
    }

    #[test]
    fn file_op_state_transitions_follow_the_state_diagram() {
        use FileOpState::*;
        assert!(Planned.can_transition_to(Executing));
        assert!(Executing.can_transition_to(Done));
        assert!(Executing.can_transition_to(Failed));
        // 起動時の確認で確定させる遷移。
        assert!(Planned.can_transition_to(Done));
        assert!(Planned.can_transition_to(Failed));
        // 終了状態からは遷移しない。
        for &next in FileOpState::ALL {
            assert!(!Done.can_transition_to(next));
            assert!(!Failed.can_transition_to(next));
        }
        assert!(!Executing.can_transition_to(Planned));
        assert!(Done.is_terminal() && Failed.is_terminal());
        assert!(!Planned.is_terminal() && !Executing.is_terminal());
    }
}
