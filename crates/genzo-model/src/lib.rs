//! GenzoParis のドメインの型。
//!
//! - 現像設定のスキーマとバージョン管理（docs/04_architecture.md の 2.5 節）: [`DevelopSettings`]
//! - パイプラインの段階 A0 / A1 / B / C（2.2 節）と、段階ごとの設定のハッシュ（2.7 節）: [`Phase`]
//! - カタログの ID と列挙値（3.1 節の ER 図）: [`ids`]、[`catalog`]
//! - 撮影日時の扱い（3.1 節。レビュー R-18）: [`CaptureTime`]
//! - 写真・動画のメタデータ: [`PhotoMetadata`]、[`VideoMetadata`]
//! - 書き出しの設定（EXP-01・EXP-04、6.4 節）: [`ExportSettings`]
//! - キャッシュキー（4.1 節）: [`CacheKey`]
//! - ファイルの同期（fsync）の OS ごとの違い（6.4 節。指摘 F19）: [`fs_sync`]。書き出し
//!   （genzo-media）・バックアップと L1（genzo-catalog）の両方が使うため、共通のこの crate に置く
//!
//! この crate は他の内部 crate に依存しない（04 の 1.4 節）。

pub mod cache_key;
pub mod capture_time;
pub mod catalog;
pub mod develop;
pub mod export;
pub mod fs_sync;
pub mod ids;
pub mod metadata;
pub mod phase;

pub use cache_key::{CacheColorSpace, CacheFormat, CacheKey, CacheKind, RenderQuality};
pub use capture_time::{CaptureTime, CaptureTimeError};
pub use catalog::{
    AssetKind, CollectionKind, ColorLabel, FileOpKind, FileOpState, FileRole, FileStatus, Flag,
    ParseEnumError, Rating, TzSource,
};
pub use develop::{
    AspectRatio, ColorParams, CropRect, CurvePoint, DataRef, DetailParams, DevelopError,
    DevelopSettings, FieldIssue, Geometry, IssueKind, LensCorrection, LocalAdjustment, LocalDelta,
    MaskShape, NoiseReduction, ParametricCurve, RenderDeps, Sharpening, ToneCurve, ToneParams,
    ValidationError, WbPreset, WhiteBalance,
};
pub use export::{
    ConflictPolicy, ExportFormat, ExportSettings, ExportSettingsError, ExportSize, OutputColorSpace,
};
pub use ids::{
    AssetId, CollectionId, FileId, FileOpId, FolderId, HistoryEntryId, KeywordId, SnapshotId,
    StackId, VariantId, VolumeId,
};
pub use metadata::{CaptureInfo, GpsCoord, Orientation, PhotoMetadata, VideoMetadata};
pub use phase::Phase;

/// 現像設定のスキーマ（項目の形）の現在のバージョン（04 の 2.5 節）。
///
/// 項目の追加・名前の変更などで JSON の形が変わったときに上げ、
/// [`develop::migrate`] にマイグレーション関数を追加する。
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// 処理アルゴリズムの現在のバージョン（04 の 2.5 節。IQ-08・DATA-09）。
///
/// 同じ設定でも結果が変わるアルゴリズムの変更をしたときに上げる。
/// 新しく現像する写真にはこの値を使い、既存の写真の値は明示的な更新なしには変えない。
pub const CURRENT_PROCESS_VERSION: u32 = 1;
