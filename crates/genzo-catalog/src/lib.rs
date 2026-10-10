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
//! - 検索: [`Filter`]、[`Sort`]、[`Catalog::search`]、[`Catalog::variant_summaries`]
//! - 選別: [`Catalog::set_rating`]、[`Catalog::set_flag`]、[`Catalog::set_color_label`]
//! - 現像設定・履歴: [`Catalog::save_develop`]、[`Catalog::apply_develop_to_many`]、
//!   [`Catalog::save_develop_batch`]、[`Catalog::undo_develop`]、[`Catalog::redo_develop`]
//! - 削除・ファイル操作: [`Catalog::remove_assets`]、[`Catalog::delete_virtual_copies`]、
//!   [`Catalog::trash_plan`]、[`Catalog::plan_trash`]
//! - 書き出しの原本の照合（6.4 節）: [`Catalog::files_in_folder`]、[`Catalog::files_named`]
//! - バックアップ: [`Catalog::create_backup`]、[`restore_backup`]、[`Catalog::check_integrity`]
//! - キャッシュ: [`ThumbStore`]（L0）、[`PreviewCache`]（L1）
//! - ハッシュ: [`FileFacts::read`]、[`quick_hash`]、[`full_hash`]

mod backup;
mod catalog;
mod develop;
pub mod dummy;
mod error;
mod files;
mod hash;
mod keyword;
mod library;
mod preview_cache;
mod register;
mod schema;
mod search;
pub mod text;
mod thumbs;
mod util;

#[cfg(test)]
mod tests;

pub use backup::{
    BackupInfo, DEFAULT_BACKUP_GENERATIONS, ForeignKeyViolation, IntegrityReport, RestoreReport,
    backup_catalog_file, create_backup, list_backups, prune_backups, restore_backup,
};
pub use catalog::{Catalog, ConnectionSettings, OpenOptions, OpenReport, PreviousShutdown};
pub use develop::{
    DevelopState, HISTORY_LABEL_IMPORT, HISTORY_LABEL_VIRTUAL_COPY, HistoryEntry, Snapshot,
    VariantInfo,
};
pub use dummy::{DummyReport, DummySpec, populate_dummy};
pub use error::{CatalogError, Result};
pub use files::{
    FILES_NAMED_CHUNK, FactsChange, FileLocation, FileOpEntry, FileOpPath, FileOpPayload,
    FileOpRecord, FileRecord, PlannedFile, RemovalReport, TrashPlan,
};
pub use hash::{
    FileFacts, QUICK_HASH_CHUNK, full_hash, full_hash_reader, quick_hash, quick_hash_reader,
};
pub use keyword::{Keyword, KeywordMatch};
pub use library::{AssetRecord, CatalogCounts};
pub use preview_cache::{
    DEFAULT_PREVIEW_CAPACITY_BYTES, EvictionReport, PREVIEW_TEMP_FILE_MIN_AGE, PreviewCache,
    ReconcileReport,
};
pub use register::{
    Folder, JPEG_EXTENSIONS, MediaMetadata, PairClass, RAW_EXTENSIONS, RegisterFile,
    RegisterOutcome, RegisterStatus, Volume, normalize_rel_path, pair_class, rel_path_from_path,
};
pub use schema::{
    BUSY_TIMEOUT, CACHE_SIZE_KIB, CATALOG_APPLICATION_ID, CATALOG_SCHEMA_VERSION,
    THUMBS_APPLICATION_ID, THUMBS_SCHEMA_VERSION,
};
pub use search::{Filter, FolderFilter, Sort, SortDirection, SortKey, VariantSummary};
pub use thumbs::{ThumbStore, Thumbnail};
