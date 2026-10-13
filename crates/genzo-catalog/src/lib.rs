//! カタログ（catalog.db）とサムネイル DB（thumbs.db）。
//!
//! SQLite のスキーマと番号付きのマイグレーション、制約（docs/04_architecture.md の 3.5 節）、
//! 検索（3.2 節・3.6 節）、データ保全とバックアップ（3.4 節）、
//! ファイル操作の記録（6.4 節の `file_op`）、サムネイルとプレビューのキャッシュ（4 章）を担当する。
//!
//! カタログへの書き込みは本体の DB 書き込みスレッドだけが行う（04 の 1.2 節・1.3 節）。
//! 検索結果の世代（3.7 節）はコア API（genzo-api）がメモリ上で管理する。この crate は、
//! 条件に合う id をすべて順番どおりに返す関数（[`Catalog::search`]）と、表示範囲の詳細を返す
//! 関数（[`Catalog::variant_summaries`]）を提供する。
//!
//! 主な型と関数:
//! - [`Catalog`] … カタログへの接続。開く（[`Catalog::open`]）と、必要ならバックアップを作ってから
//!   移行する。閉じる（[`Catalog::close`]）と「正常に終了した」印を付ける。
//! - 登録: [`Catalog::ensure_volume`]、[`Catalog::ensure_folder`]、[`Catalog::register_batch`]、
//!   [`Catalog::find_file`]（登録済みのファイルの確認）
//! - 検索: [`Filter`]、[`Sort`]、[`Catalog::search`]、[`Catalog::variant_summaries`]、絞り込みの候補
//!   （[`Catalog::facets`]）
//! - 選別: [`Catalog::set_rating`]、[`Catalog::set_flag`]、[`Catalog::set_color_label`]
//! - 階層キーワード（LIB-09）: [`Catalog::keyword_tree`]、[`Catalog::add_keywords`]、
//!   [`Catalog::export_keywords`]
//! - コレクション（LIB-10）・スマートコレクション（LIB-11）: [`Catalog::collections`]、
//!   [`Catalog::create_collection`]、[`Catalog::add_to_collection`]、[`Filter::collection`]
//! - スタック（LIB-12）: [`Catalog::create_stack`]、[`Catalog::set_stacks_collapsed`]、
//!   [`Catalog::stack_layout`] と [`StackLayout::group`]（検索結果のまとめ方）、[`Catalog::auto_stack`]
//! - 仮想コピー（LIB-13）: [`Catalog::create_virtual_copy`]、[`Catalog::rename_virtual_copy`]、
//!   [`Catalog::set_as_master`]、[`Catalog::delete_virtual_copies`]
//! - 説明のメタデータ（LIB-15。タイトル・キャプション・作成者・著作権など）: [`Catalog::asset_metadata`]、
//!   [`Catalog::set_asset_metadata`]（一括編集）、[`Catalog::metadata_summary`]
//! - 現像設定・履歴: [`Catalog::save_develop`]、[`Catalog::apply_develop_to_many`]、
//!   [`Catalog::save_develop_batch`]（履歴の名前を variant ごとに: [`Catalog::save_develop_labeled`]）、
//!   [`Catalog::undo_develop`]、[`Catalog::redo_develop`]
//! - 削除・ファイル操作: [`Catalog::remove_assets`]、[`Catalog::delete_virtual_copies`]、
//!   [`Catalog::trash_plan`]、[`Catalog::plan_trash`]、移動・リネーム（FILE-02）: [`Catalog::plan_file_moves`]、
//!   [`Catalog::plan_folder_op`]、[`Catalog::complete_file_op_entries`]、[`Catalog::create_folder`]
//! - ボリューム（FILE-03）・見つからない写真（FILE-04）: [`Catalog::volume_records`]、
//!   [`Catalog::set_offline_volumes`]、[`Catalog::rebind_folders`]、[`Catalog::relink_file`]、
//!   [`Catalog::relocate_folder`]、[`Catalog::missing_files`]、[`Catalog::set_folder_status`]
//! - 書き出しの原本の照合（6.4 節）: [`Catalog::files_in_folder`]、[`Catalog::files_named`]
//! - バックアップ: [`Catalog::create_backup`]、[`restore_backup`]、[`Catalog::check_integrity`]
//! - 構造（ORG-07）: [`Catalog::describe_schema`]、公開ビュー（[`PUBLIC_VIEWS`]）。文書は
//!   docs/catalog_schema.md（実際のスキーマと食い違わないことをテストで確かめる）
//! - キャッシュ: [`ThumbStore`]（L0）、[`PreviewCache`]（L1）
//! - ハッシュ: [`FileFacts::read`]、[`quick_hash`]、[`full_hash`]
//! - 内容による同一性（ORG-03）・重複（IMP-04）・RAW と JPEG のペア（IMP-06）:
//!   [`Catalog::files_with_quick_hash`]、[`Catalog::set_full_hash_if_current`]、
//!   [`Catalog::files_without_full_hash`]、[`Catalog::duplicate_groups`]、[`Catalog::companion_files`]、
//!   [`RegisterOptions`]
//! - 撮影日時の一括修正（LIB-16）: [`Catalog::capture_devices`]（機材）、[`Catalog::plan_capture_adjustment`]、
//!   [`Catalog::adjust_capture_times`]、[`Catalog::undo_capture_time_edit`]。タイムライン（ORG-01）の撮影日時:
//!   [`Catalog::timeline_keys`]

mod backup;
mod capture;
mod catalog;
mod collection;
mod develop;
pub mod dummy;
mod error;
mod files;
mod folders;
mod hash;
mod identity;
mod iptc;
mod keyword;
mod library;
mod preview_cache;
mod register;
mod schema;
mod search;
mod smart_sql;
mod stack;
pub mod text;
mod thumbs;
mod util;

#[cfg(test)]
mod tests;

// 他の crate の結合テスト（機能フラグ `test-support`）と、この crate のテストが使う。
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod testing;

pub use backup::{
    BackupInfo, BackupOwner, DEFAULT_BACKUP_GENERATIONS, ForeignKeyViolation, IntegrityReport,
    RestoreReport, backup_catalog_file, create_backup, list_backups, list_backups_by_stem,
    prune_backups, restore_backup,
};
pub use capture::{
    CaptureAdjustOutcome, CaptureAdjustment, CaptureChange, CaptureEditKind, CaptureEditRecord,
    CapturePlan, CaptureUndoOutcome, DeviceInfo, DeviceKey, MAX_CAPTURE_SHIFT_SECONDS,
    MAX_CAPTURE_TIME_EDITS, TimelineKey, TzSourceCounts,
};
pub use catalog::{
    Catalog, ConnectionSettings, OpenOptions, OpenReport, PreviousShutdown,
    STATE_CAPTURE_TIME_RERESOLVE,
};
pub use collection::{Collection, MAX_COLLECTION_DEPTH, MAX_COLLECTION_NAME_CHARS};
pub use develop::{
    DevelopState, HISTORY_LABEL_IMPORT, HISTORY_LABEL_VIRTUAL_COPY, HistoryEntry,
    MAX_VARIANT_NAME_CHARS, Snapshot, VariantInfo,
};
pub use dummy::{DummyReport, DummySpec, populate_dummy};
pub use error::{CatalogError, Result};
pub use files::{
    AppliedEntry, FILES_NAMED_CHUNK, FactsChange, FileLocation, FileOpEntry, FileOpPath,
    FileOpPayload, FileOpRecord, FileRecord, FolderOpEntry, MoveMethod, MoveRequest, PlannedFile,
    RemovalReport, TrashPlan,
};
pub use folders::{
    FolderRebind, FolderRecord, FolderRemoval, RebindReport, VolumeRecord, rel_path_below,
};
pub use hash::{
    FileFacts, QUICK_HASH_CHUNK, full_hash, full_hash_reader, quick_hash, quick_hash_reader,
};
pub use identity::{ContentHashCounts, DuplicateGroup, DuplicateGroups};
pub use iptc::{MetadataFieldSummary, MetadataSummary};
pub use keyword::{
    KEYWORD_NAME_SEPARATORS, Keyword, KeywordMatch, KeywordNode, KeywordUsage, MAX_KEYWORD_DEPTH,
    MAX_KEYWORD_NAME_CHARS, check_keyword_path,
};
pub use library::{AssetRecord, CatalogCounts};
pub use preview_cache::{
    DEFAULT_PREVIEW_CAPACITY_BYTES, EvictionReport, PREVIEW_TEMP_FILE_MIN_AGE, PreviewCache,
    PreviewScan, ReconcileReport, scan_preview_dir,
};
pub use register::{
    Folder, ImportTags, JPEG_EXTENSIONS, MediaMetadata, PairClass, RAW_EXTENSIONS, RegisterFile,
    RegisterOptions, RegisterOutcome, RegisterStatus, TagOutcome, Volume, normalize_rel_path,
    pair_class, rel_path_from_path,
};
pub use schema::{
    BUSY_TIMEOUT, CACHE_SIZE_KIB, CAPTURE_TIME_EDITS_SCHEMA_VERSION, CATALOG_APPLICATION_ID,
    CATALOG_SCHEMA_VERSION, COLLECTIONS_SCHEMA_VERSION, CONTENT_IDENTITY_SCHEMA_VERSION,
    ColumnInfo, IPTC_SCHEMA_VERSION, PUBLIC_VIEWS, PUBLIC_VIEWS_SCHEMA_VERSION,
    STABLE_IDS_SCHEMA_VERSION, STACKS_SCHEMA_VERSION, SchemaDescription, SchemaObject,
    THUMBS_APPLICATION_ID, THUMBS_SCHEMA_VERSION, TableInfo, UTC_ONLY_SCHEMA_VERSION,
    VOLUMES_SCHEMA_VERSION,
};
pub use search::{Facets, Filter, FolderFilter, Sort, SortDirection, SortKey, VariantSummary};
pub use stack::{
    AutoStackReport, GroupedIds, MAX_AUTO_STACK_GAP, StackBadge, StackInfo, StackLayout,
};
pub use thumbs::{ThumbStore, Thumbnail};
