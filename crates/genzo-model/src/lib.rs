//! GenzoParis のドメインの型。
//!
//! - 現像設定のスキーマとバージョン管理（docs/04_architecture.md の 2.5 節）: [`DevelopSettings`]
//! - パイプラインの段階 A0 / A1 / B / C（2.2 節）と、段階ごとの設定のハッシュ（2.7 節）: [`Phase`]
//! - カタログの ID と列挙値（3.1 節の ER 図）: [`ids`]、[`catalog`]
//! - 撮影日時の扱い（3.1 節。レビュー R-18）: [`CaptureTime`]
//! - 写真・動画のメタデータ: [`PhotoMetadata`]、[`VideoMetadata`]
//! - カタログの中で編集する説明のメタデータ（LIB-15。IPTC Core の主要な項目）: [`IptcMetadata`]
//! - スマートコレクションの条件（LIB-11。版付きの JSON）: [`SmartRule`]
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
pub mod iptc;
pub mod metadata;
pub mod phase;
pub mod smart;

pub use cache_key::{CacheColorSpace, CacheFormat, CacheKey, CacheKind, RenderQuality};
pub use capture_time::{CaptureTime, CaptureTimeError, UTC_ONLY_OFFSET, ZoneMode};
pub use catalog::{
    AssetKind, CollectionKind, ColorLabel, FileOpKind, FileOpState, FileRole, FileStatus, Flag,
    FolderStatus, ParseEnumError, Rating, TzSource,
};
pub use develop::{
    AspectRatio, ColorParams, CropRect, CurvePoint, DataRef, DetailParams, DevelopError,
    DevelopSettings, FieldIssue, Geometry, HighlightMethod, HighlightRecovery, IssueKind,
    LensCorrection, LocalAdjustment, LocalDelta, MaskShape, NoiseReduction, ParametricCurve,
    RenderDeps, Sharpening, ToneCurve, ToneParams, ValidationError, WbPreset, WhiteBalance,
};
pub use export::{
    ConflictPolicy, ExportFormat, ExportSettings, ExportSettingsError, ExportSize, OutputColorSpace,
};
pub use ids::{
    AssetId, CollectionId, FileId, FileOpId, FolderId, HistoryEntryId, KeywordId, SnapshotId,
    StackId, VariantId, VolumeId,
};
pub use iptc::{
    CopyrightStatus, IptcMetadata, MAX_METADATA_CHARS, MetadataField, MetadataValueError,
    normalize_metadata_value,
};
pub use metadata::{
    CaptureInfo, GpsCoord, Orientation, PhotoMetadata, VideoMetadata, camera_display_name,
    normalize_serial,
};
pub use phase::Phase;
pub use smart::{
    ContainsOp, DateOp, EqualityOp, KeywordOp, MAX_SMART_CONDITIONS, MAX_SMART_DAYS,
    MAX_SMART_DEPTH, MAX_SMART_TEXT_CHARS, MatchMode, MemberOp, NumberOp, SMART_RULE_VERSION,
    SmartCondition, SmartRule, SmartRuleError, TextOp,
};

/// 現像設定のスキーマ（項目の形）の現在のバージョン（04 の 2.5 節）。
///
/// 項目の追加・名前の変更などで JSON の形が変わったときに上げ、
/// [`develop::migrate`] にマイグレーション関数を追加する。
///
/// - 2（2026-10-13。DEV-14）: レンズ補正の手動の項目（`lens.distortion` など）を加えた。v1 の機能の
///   パッケージの間（配布の前）は、後のパッケージが足す項目も版 2 に入れる（決定事項ログ No.120）。
///   同日、ハイライト復元（`highlight_recovery`。DEV-06）とノイズ軽減のディテール・コントラスト（DEV-13）を
///   加えた（決定事項ログ No.126・127）。
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// 処理アルゴリズムの現在のバージョン（04 の 2.5 節。IQ-08・DATA-09）。
///
/// 同じ設定でも結果が変わるアルゴリズムの変更をしたときに上げる。
/// 新しく現像する写真にはこの値を使い、既存の写真の値は明示的な更新なしには変えない。
///
/// - 1: MVP（04 の 2.1 節のステージのうち、v1 の印のないもの）。
/// - 2（2026-10-13）: v1 の機能のステージ（最初はレンズ補正。DEV-14。同日、ハイライト復元（DEV-06）と
///   ノイズ軽減（DEV-13）も足した）。v1 の機能のパッケージの間（配布の前）は、後のパッケージも版 2 に足す。
///   既定の設定（v1 の機能の項目が既定値。ハイライト復元は「白で切りそろえる」、ノイズ軽減は量 0）では版 1 と
///   同じ結果になるので、新しい写真の既定を 2 にした（決定事項ログ No.120・126）。
pub const CURRENT_PROCESS_VERSION: u32 = 2;
