//! コア API。
//!
//! UI（案 A: Tauri / 案 B: Rust ネイティブ UI）と CLI が共通に使う、コマンドとイベントの型と
//! 各サービス（取り込み・カタログ・現像・書き出し・ファイル操作）を提供する
//! （docs/04_architecture.md の 1.1 節・1.5 節）。コアは UI を知らない（MAINT-01）。
//!
//! # 使い方
//!
//! ```no_run
//! use std::path::Path;
//!
//! use genzo_api::{Core, CoreConfig, SearchFilter, SearchSort, WorkerLaunch};
//!
//! let mut config = CoreConfig::new("/photos/catalog.db", "/photos/genzo-data");
//! // ワーカーを別の実行ファイルにしない構成（本体の main が `__worker` で genzo_worker::run_worker を呼ぶ）。
//! config.worker = WorkerLaunch::current_exe();
//! let core = Core::open(config)?;
//! let events = core.subscribe(); // 進捗・カタログの変更・プレビューの更新・警告
//!
//! // 取り込み（P3 のジョブ）。
//! let job = core.import_folder(Path::new("/photos/2024-05-01"), true)?;
//! let info = core.wait_job(job, None)?;
//!
//! // 検索（世代番号と件数。id はコアのメモリに保持）と、表示範囲の詳細。
//! let result = core.search(&SearchFilter::default(), SearchSort::default())?;
//! let page = core.range(result.generation, 0, 100)?;
//!
//! // 現像（最新の 1 件だけを描画。結果はイベントと develop_frame で受け取る）。
//! let v = page.items[0].variant_id;
//! let state = core.open_develop(v)?;
//! let mut s = state.settings.clone();
//! s.exposure_ev = 0.5;
//! let generation = core.set_settings(&s, true)?; // ドラッグ中
//! core.end_drag()?; // 履歴に 1 件記録して保存
//! core.close()?;
//! # let _ = (events, info, generation);
//! # Ok::<(), genzo_api::ApiError>(())
//! ```
//!
//! # 構成
//!
//! | モジュール | 内容 | 設計書 |
//! |---|---|---|
//! | [`Core`]（`core`） | 起動（カタログ・前回の終了の確認・ファイル操作の確定・自動バックアップ）と終了、実行モデル、ロックの順序 | 1.2 節・1.3 節、DATA-04・05・07 |
//! | [`config`] | [`CoreConfig`]（ワーカーの起動方法 [`WorkerLaunch`]、GPU の使用 [`GpuMode`]）と仮置きの値 | 1.2 節 |
//! | [`command`] | [`Command`]・[`Response`]・[`Core::execute`] | 1.5 節 |
//! | [`events`] | [`Event`]（ジョブの進捗・カタログの変更・プレビューの更新・検索結果の世代・警告） | 1.5 節 |
//! | [`error`] | [`ApiError`] と UI に見せる分類（[`ErrorInfo`]・[`ErrorKind`]） | 6.3 節 |
//! | [`types`] | 引数・応答の型（serde の素直な形） | 1.5 節 |
//! | `import` | 取り込み（[`Core::import_folder`]） | IMP-01・VID-01・PRV-01、3.5 節 |
//! | `search` | 検索と世代（[`Core::search`]・[`Core::range`]・[`Core::index_of`]） | LIB-07・08、3.2 節・3.7 節 |
//! | `library` | 選別（[`Core::set_rating`] など）・詳細・フォルダ・ファイルの状態（[`Core::check_files`]） | LIB-04・14、3.3 節 |
//! | `previews` | サムネイル（[`Core::thumbnail`]）・L1（[`Core::preview_path`]）と、現像結果からの作り直し | 4 章・4.1 節、PRV-01・02・04 |
//! | `develop` | 現像のセッション（[`Core::open_develop`]・[`Core::set_settings`]・Undo / Redo・一括適用・仮想コピー） | DEV-00〜08・15・26・27・30、6.2 節 |
//! | `render` | GPU → CPU の切り替え、画面のプロファイル（[`Core::set_display_profile`]） | 2.4 節・6.3 節、IQ-05 |
//! | `export` | 書き出し（[`Core::export`]） | EXP-01・04、2.4 節・6.4 節 |
//! | `delete` | 削除（[`Core::plan_delete`]・[`Core::execute_delete`]）とファイル操作の確定 | FILE-01、6.4 節、DATA-07 |
//! | `settings` | 設定（[`Core::settings`]・[`Core::update_settings`]） | SYS-05 |
//! | [`maintenance`] | バックアップ・詳細チェック・復元（[`prepare_restore`]・[`apply_restore`]）、ジョブの操作 | SYS-04、DATA-04・05 |
//!
//! # 設計からの逸脱（理由は各モジュールの doc）
//!
//! - **実行モデル**: 1.3 節の tokio の代わりに、同期の API ＋ 内部のスレッド（genzo-jobs のスケジューラ）
//!   ＋ イベントのチャネル。DB 書き込みスレッドの代わりにカタログの接続を `Mutex` で守る。GPU スレッドは
//!   置かない（`core` の doc）。
//! - **ボリューム**: OS のボリューム ID（FILE-03、v1）の代わりに、パスの先頭（`/`・`C:\`）をボリュームと
//!   する（`paths` の doc）。
//! - **検索結果の作り直し**: バックグラウンドではなく、カタログを変えた操作の直後に同期で行う（`search` の doc）。
//! - **書き出しの原本の照合**: カタログ全体ではなく、今回の入力、書き出し先のフォルダにあるカタログの
//!   ファイル、上書きの設定では書き出すファイルと同じ名前のカタログのファイルを保護の対象にする
//!   （`export` の doc）。
//! - **型の生成**: TypeScript の型の生成（ts-rs など）はまだ入れていない。型は serde の素直な形にした。

pub mod command;
pub mod config;
mod core;
mod delete;
mod develop;
pub mod error;
pub mod events;
mod export;
mod import;
mod jobs;
mod library;
pub mod maintenance;
mod paths;
mod previews;
mod render;
mod search;
mod settings;
pub mod types;

pub use command::{Command, Response};
pub use config::{CoreConfig, GpuMode, WorkerLaunch};
pub use core::{Core, CoreHooks};
pub use delete::{OsTrash, TrashBackend};
pub use develop::{HISTORY_LABEL_GENERIC, HISTORY_LABEL_PASTE, describe_change};
pub use error::{ApiError, ErrorInfo, ErrorKind};
pub use events::{CatalogChange, Event, WarningCode};
pub use import::{IMAGE_EXTENSIONS, RAW_EXTENSIONS, VIDEO_EXTENSIONS, media_kind};
pub use maintenance::{RestorePlan, apply_restore, prepare_restore};
pub use previews::{L0_FORMAT_VERSION, L1_FORMAT_VERSION};
pub use search::MAX_RANGE_LEN;
pub use settings::{
    KEY_DEFAULT_EXPORT_COLOR_SPACE, KEY_DEFAULT_UTC_OFFSET_MINUTES, KEY_PREVIEW_CACHE_BYTES,
    KEY_PREVIEW_CACHE_DIR, MIN_PREVIEW_CACHE_BYTES, UTC_OFFSET_RANGE_MINUTES,
};
pub use types::*;

// 型付きのメソッドの引数・応答で使う、依存先の crate の型（UI がまとめて使えるように）。
pub use genzo_model::{
    AssetId, AssetKind, ColorLabel, DevelopSettings, ExportFormat, ExportSettings, ExportSize,
    FileStatus, Flag, OutputColorSpace, Rating, RenderQuality, VariantId,
};
pub use genzo_worker::{Lane, TestHook, TestHookOutput};

/// コアの型がスレッド間で共有できること（Tauri の状態として持てること）。
#[allow(dead_code)]
fn assert_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<Core>();
    check::<Event>();
    check::<ApiError>();
}
