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
//! | [`Core`]（`core`） | 起動（カタログ・前回の終了の確認・ファイル操作の確定・自動バックアップ・キャッシュの持ち主の確認と作り直し）と終了、実行モデル、ロックの順序 | 1.2 節・1.3 節、DATA-03b・04・05・07 |
//! | `lock` | カタログの排他（1 つのカタログは 1 つのアプリだけが開く。[`ApiError::CatalogInUse`]） | 3 章 |
//! | `deps` | 現像設定の外部データ（`render_deps`: カメラ行列の出どころとハッシュ・RAW デコーダ・埋め込みのレンズ補正の情報の出どころとハッシュ・カメラプロファイルとルックのファイルの ID とハッシュ）の記録と、変わったときの警告 | 2.5 節、R-08、DEV-14・17 |
//! | [`config`] | [`CoreConfig`]（ワーカーの起動方法 [`WorkerLaunch`]、GPU の使用 [`GpuMode`]）と仮置きの値 | 1.2 節 |
//! | [`command`] | [`Command`]・[`Response`]・[`Core::execute`] | 1.5 節 |
//! | [`events`] | [`Event`]（ジョブの進捗・カタログの変更・プレビューの更新・検索結果の世代・警告） | 1.5 節 |
//! | [`error`] | [`ApiError`] と UI に見せる分類（[`ErrorInfo`]・[`ErrorKind`]） | 6.3 節 |
//! | [`types`] | 引数・応答の型（serde の素直な形） | 1.5 節 |
//! | `import` | 取り込み（[`Core::import_folder`]・[`Core::import_folder_with`]。重複と思われるものを取り込まない・RAW と JPEG の扱い・コピー・まとめて付けるキーワードとメタデータの設定は [`ImportOptions`]） | IMP-01・04〜06・VID-01・PRV-01、3.5 節・3.11 節 |
//! | `copy_import` | コピー取り込み（[`CopySettings`]。一時ファイルに書いて全体のハッシュで照合してから上書きせずに公開し、失敗・取り消しではこのジョブが作ったファイルだけを片付ける）と、名前の例（[`Core::import_name_preview`]） | IMP-02・03、3.12 節・6.4 節 |
//! | [`naming`] | コピー先の名前: フォルダの規則（[`FolderRule`]）・リネームの規則（[`RenameRule`]）・OS で使えない名前の処理 | IMP-02・03、3.12 節 |
//! | `identity` | ファイルの内容による同一性: 内容のハッシュのバックグラウンドの計算（[`Core::compute_content_hashes`]）・状況（[`Core::content_hash_status`]）・重複の組（[`Core::duplicate_groups`]）・ファイルの識別（[`Core::identify_file`]）、取り込みの重複の確認 | ORG-03・IMP-04、3.3 節・3.11 節 |
//! | `search` | 検索と世代（[`Core::search`]・[`Core::range`]・[`Core::index_of`]・[`Core::shown_variants`]）、結果のバックグラウンドの作り直し、絞り込みの候補（[`Core::facets`]） | LIB-07・08、3.2 節・3.7 節 |
//! | `library` | 選別（[`Core::set_rating`] など）・詳細・フォルダ・ファイルの状態（[`Core::check_files`]） | LIB-04・14、3.3 節 |
//! | `organize` | 階層キーワード（[`Core::keyword_tree`]・[`Core::add_keywords`]）・コレクションとスマートコレクション（[`Core::collections`]・[`Core::add_to_collection`]・[`Core::set_smart_rule`]）。絞り込みは [`SearchFilter::collection_id`]・[`SearchFilter::keywords`] | LIB-09〜11、3.9 節 |
//! | `stacks` | スタック（[`Core::create_stack`]・[`Core::set_stacks_collapsed`]・[`Core::auto_stack`]。検索結果のまとめ方は `search`）と仮想コピーの名前・マスターの入れ替え（[`Core::rename_virtual_copy`]・[`Core::set_as_master`]） | LIB-12・13、3.10 節 |
//! | `previews` | サムネイル（[`Core::thumbnail`]）・L1（[`Core::preview_path`]）と、現像結果からの作り直し | 4 章・4.1 節、PRV-01・02・04 |
//! | `playback` | 動画の再生（[`Core::video_playback`]・配信するファイル [`Core::video_stream`]）と、WebView で再生できない動画の再生用のファイル（[`Core::create_video_proxy`]。利用者の ffmpeg で H.264 ＋ AAC の MP4 にする。上限容量の LRU のキャッシュ） | VID-04、4.2 節 |
//! | `develop` | 現像のセッション（[`Core::open_develop`]・[`Core::set_settings`]・Undo / Redo・一括適用・仮想コピー）、WB のスポイト（[`Core::white_balance_at`]）、切り抜きの編集の支え（[`Core::set_develop_view`]・[`Core::constrain_crop`]）、自動の遠近補正（Upright 相当。[`Core::analyze_upright`]。DEV-16）、処理バージョンの更新（[`Core::upgrade_process_version`]・まとめて [`Core::upgrade_process_versions`]。レンズ補正などの v1 の機能は処理バージョン 2 から。貼り付けは版を変えない: [`PasteVersion`]）。埋め込みのレンズ補正の情報の要約は [`SourceInfo::lens_correction`]。ローカル補正（マスク）の色域のスポイト（[`Core::mask_color_at`]。点の G 座標と OKLab）・重ねて表示の重み（[`Core::mask_overlay`]）、画面の画像とマスクの G 座標の対応（[`FrameInfo::g_bounds`]）、履歴の名前（[`describe_change`]・[`mask_display_name`]） | DEV-00〜08・14・15・18・19・26・27・30、DATA-09、6.2 節 |
//! | `render` | GPU → CPU の切り替え、画面のプロファイル（[`Core::set_display_profile`]） | 2.4 節・6.3 節、IQ-05 |
//! | `region` | 表示範囲の描画（[`Core::render_region`]。ルーペの等倍表示・拡大とパン。P0 の最新の 1 件だけ、取り消しあり） | PRV-03・LIB-02、2.7 節・6.1 節 |
//! | `view` | 比較表示の表示範囲の描画（[`Core::render_view_region`]・[`Core::view_output_size`]。面ごとに任意の写真を描く。展開した入力を上限まで持ち、[`Core::release_view`] で捨てる） | LIB-03、1.5 節・6.1 節 |
//! | `export` | 書き出し（[`Core::export`]・ファイル名の規則を指定する [`Core::export_with`]・名前の例 [`preview_export_names`]・書き出しの仕上げ（出力シャープ・透かし）・動画の再エンコードなしのコピー） | EXP-01〜05、2.4 節・6.4 節 |
//! | `watermark` | 透かしの見本（[`Core::watermark_preview`]。ワーカーで透かしを作り、見本の画像に重ねた PNG）と OS のフォントのフォルダ（[`system_font_dirs`]） | EXP-03、2.4 節 |
//! | `snapshots` | スナップショット（[`Core::snapshots`]・[`Core::create_snapshot`]（今の状態・履歴の段階）・[`Core::rename_snapshot`]・[`Core::delete_snapshot`]・[`Core::apply_snapshot`]（履歴に 1 件残す））。ビフォーには [`BeforeSource::Snapshot`] | DEV-28、3.1 節 |
//! | `presets` | 現像プリセット・書き出しプリセット（[`PresetStore`]。アプリのデータのフォルダ（[`default_presets_dir`]）に 1 件 1 ファイルの版付きの JSON。[`Core::create_develop_preset`]・[`Core::apply_develop_preset`]（設定の貼り付けと同じく処理バージョンを変えない）・[`Core::create_export_preset`]・名前・フォルダ・削除・ファイルの読み込みと書き出し） | DEV-29・EXP-02、2.5 節・6.4 節 |
//! | `capture_time` | 撮影日時の一括修正（[`Core::capture_devices`]・[`Core::plan_capture_time_edit`]・[`Core::edit_capture_times`]・[`Core::undo_capture_time_edit`]。ずらす・基準の写真に合わせる・タイムゾーンの指定。カタログの中だけで、元のファイルには書かない） | LIB-16、3.13 節 |
//! | `location` | 位置情報の後付け（[`Core::plan_location_edit`]・[`Core::edit_locations`]・[`Core::location_edits`]・[`Core::undo_location_edit`]。選んだ写真にまとめて同じ位置を付ける・付けた位置を消す。動画は除く。カタログの中だけで、元のファイルには書かない。使う位置は詳細の [`VariantDetails::gps`] と書き出しの Exif） | LIB-21、3.16 節 |
//! | `timeline` | 写真と動画の統合タイムライン（[`Core::timeline`]。表示中の検索結果を撮影日と、撮影の間隔によるイベントでまとめる） | ORG-01、3.13 節 |
//! | `volumes` | ボリュームの識別（OS のボリューム ID。取れなければパスの先頭。OS ごとの取り方は `volumes::os`）・つながりの確認（[`Core::volumes`]・[`Core::refresh_volumes`]）・以前の形式のボリュームの移し替え | FILE-03、3.3 節・3.14 節 |
//! | `fileops` | アプリの中でのファイル操作（[`Core::move_files`]・[`Core::rename_files`]・[`Core::create_folder`]・[`Core::rename_folder`]・[`Core::move_folder`]・[`Core::remove_folder`]）と、途中で終わった移動・名前の変更の確定 | FILE-02、6.4 節・3.14 節、DATA-07 |
//! | `relink` | 見つからない写真の確認（[`Core::check_missing`]）・再リンクの候補と結び付け（[`Core::find_relink_candidates`]・[`Core::relink_files`]）・フォルダの場所の指定し直し（[`Core::relocate_folder`]） | FILE-04、3.3 節・3.14 節 |
//! | `delete` | 削除（[`Core::plan_delete`]・[`Core::execute_delete`]）とファイル操作の確定 | FILE-01、6.4 節、DATA-07 |
//! | `profiles` | カメラプロファイル（DCP）・ルック（3D LUT の .cube）のライブラリ（[`Core::import_profiles`]・[`Core::list_profiles`]・[`Core::remove_profile`]。データのフォルダの `profiles` に内容の SHA-256 の名前で写しを置き、一覧を一時ファイルから置き換える。描画のエンジンが現像設定の ID から引く）。Adobe のプロファイルなどは同梱しない | DEV-17、2.1 節・2.5 節・6.4 節 |
//! | `settings` | 設定（[`Core::settings`]・[`Core::update_settings`]） | SYS-05 |
//! | [`maintenance`] | バックアップ・詳細チェック・復元（[`prepare_restore`]・[`apply_restore`]）、ジョブの操作 | SYS-04、DATA-04・05 |
//! | [`logging`] | ログのファイル（上限とローテーション）・パニックのフックとクラッシュレポート・ログのフォルダの確認と消去（アプリと CLI が起動時に [`logging::init`]）。外部へは送信しない | SYS-07、SEC-02 |
//!
//! カタログの構造（ORG-07）は [`Core::catalog_schema`]（[`Command::CatalogSchema`]）で返す。文書は
//! docs/catalog_schema.md。
//!
//! # 設計からの逸脱（理由は各モジュールの doc）
//!
//! - **実行モデル**: 1.3 節の tokio の代わりに、同期の API ＋ 内部のスレッド（genzo-jobs のスケジューラ）
//!   ＋ イベントのチャネル。DB 書き込みスレッドの代わりにカタログの接続を `Mutex` で守る。GPU スレッドは
//!   置かない（`core` の doc）。
//! - **ボリューム**: OS のボリューム ID（FILE-03。`volumes` の doc）を取れないとき（コンテナの中・tmpfs・
//!   Windows の UNC の共有と割り当てたネットワークドライブなど）は、MVP と同じくパスの先頭（`/`・`C:\`）を
//!   ボリュームとする（`paths` の doc）。Windows・macOS の ID の取り方は実機で未確認。
//! - **書き出しの原本の照合**: カタログ全体ではなく、今回の入力、書き出し先のフォルダにあるカタログの
//!   ファイル、上書きの設定では書き出すファイルと同じ名前のカタログのファイルを保護の対象にする
//!   （`export` の doc）。
//! - **型の生成**: TypeScript の型の生成（ts-rs など）はまだ入れていない。型は serde の素直な形にした。
//! - **L0 / L1 のキャッシュキーの外部データ**: 4.1 節の `render_deps_hash` は、保存された `render_deps` の
//!   ハッシュではなく、RAW デコーダの識別子だけを入れた値のハッシュにする（写真を展開せずにキーを求める
//!   ため。カメラ行列は機種とデコーダの版、またはファイルの内容で決まる。`deps` の doc）。キーには
//!   キャッシュの世代（カタログの `cache_generation`。復元の後に変わる）も入れる（`previews` の doc）。
//!
//! # M4（UI）のために足したもの
//!
//! - 絞り込みの候補（[`Core::facets`]・[`Command::Facets`]）: カメラ・レンズ・動画のコーデックと件数
//!   （件数は asset の数。値は [`SearchFilter`] にそのまま渡して一致する）。
//! - WB のスポイト（DEV-03。[`Core::white_balance_at`]・[`Command::WhiteBalanceAt`]）: プレビューの上の点を
//!   中立にする色温度・tint を返す（適用はしない）。点は UI が表示しているプレビューの割合で、その画像の
//!   表示（[`FrameInfo::uncropped`]。省略可）も送れる（表示を切り替えた直後に、前の表示の画像の上の点を
//!   取り違えないため）。撮影時の WB の色温度・tint は [`SourceInfo::as_shot_white_balance`] で返す。
//! - 自動補正（DEV-32。2026-10-26 追加。[`Core::auto_tone`]・[`Command::AutoTone`]）: 露光量・コントラスト・
//!   ハイライト・シャドウ・白レベル・黒レベルを写真に合わせて決めた値を返す（適用はしない。UI・CLI が設定に入れて
//!   保存する。genzo-pipeline の `engine::auto_tone`）。
//! - 切り抜きの編集中の表示（[`Core::set_develop_view`]・[`Command::SetDevelopView`]）: 切り抜く前の全体
//!   （G の外接矩形）を描く（[`FrameInfo::uncropped`]）。描画が実際に使う切り抜き
//!   （[`Core::constrain_crop`]・[`Command::ConstrainCrop`]）。
//! - [`FrameInfo::output_width`]・[`FrameInfo::output_height`]: フル解像度の出力の寸法（表示範囲の描画の
//!   座標の格子）。
//!
//! # まだないもの（implementation_status の進捗表 No.12）
//!
//! - 表示範囲の描画（[`Core::render_region`]・比較表示の面の [`Core::render_view_region`]）の GPU 版（今は CPU 版
//!   だけ）と、ビューポートへの直接の描画（04 の 1.5 節。UI の方式と合わせて決める）。
//! - 切り抜く前の全体の表示（[`Core::set_develop_view`]）の GPU 版（今は、その間のプレビューを CPU 版で
//!   描く。genzo-gpu の `render_preview` が [`genzo_pipeline::PreviewRequest::uncropped`] を見ないため）。

mod ai_masks;
mod before;
mod capture_time;
pub mod command;
mod composite;
pub mod config;
mod copy_import;
mod core;
mod delete;
mod deps;
mod develop;
pub mod error;
pub mod events;
mod export;
mod fileops;
mod identity;
mod import;
mod jobs;
mod library;
mod location;
mod lock;
pub mod logging;
pub mod maintenance;
pub mod naming;
mod organize;
mod paths;
mod playback;
mod presets;
mod previews;
mod profiles;
mod region;
mod relink;
mod render;
mod search;
mod settings;
mod snapshots;
mod stacks;
mod timeline;
pub mod types;
mod view;
mod volumes;
mod watermark;

pub use ai_masks::{
    ENV_MODELS_DIR, ENV_ONNXRUNTIME, MODELS_DIR, default_models_dir, onnxruntime_candidates,
};
pub use capture_time::{CAPTURE_TIME_EXAMPLES, CAPTURE_ZONE_RANGE_MINUTES};
pub use command::{Command, Response};
pub use config::{CoreConfig, GpuMode, IMPORT_BATCH_FILES, WorkerLaunch};
pub use core::{
    CopyImportHook, CopyImportStage, Core, CoreHooks, DevelopSaveStage, FileOpHook, FileOpStage,
    RegionRequestStage, SettingsUpdateStage,
};
pub use delete::{OsTrash, TrashBackend};
pub use develop::{
    HISTORY_LABEL_AUTO_TONE, HISTORY_LABEL_BLACK_WHITE, HISTORY_LABEL_COLOR, HISTORY_LABEL_GENERIC,
    HISTORY_LABEL_PASTE, HISTORY_LABEL_UPGRADE_PROCESS, describe_change, mask_display_name,
    upgrade_process_label,
};
pub use error::{ApiError, ErrorInfo, ErrorKind};
pub use events::{CatalogChange, Event, WarningCode};
pub use export::preview_export_names;
/// ONNX Runtime のテレメトリーを止める環境変数と、その設定（genzo-ml。アプリ・CLI の `main` の最初で呼ぶ）。
pub use genzo_ml::{
    ORT_DISABLE_TELEMETRY_ENV, disable_telemetry_before_threads as disable_onnxruntime_telemetry,
};
pub use identity::{DUPLICATE_GROUPS_MAX, HASH_FAILURES_KEPT, INTERACTIVE_MAX_WAIT};
pub use import::{IMAGE_EXTENSIONS, RAW_EXTENSIONS, VIDEO_EXTENSIONS, media_kind};
pub use maintenance::{RestorePlan, apply_restore, prepare_restore};
pub use naming::{FolderRule, RenameRule};
pub use organize::MAX_KEYWORD_HITS;
pub use playback::{
    DEFAULT_VIDEO_PROXY_CAPACITY_BYTES, MIN_VIDEO_PROXY_CACHE_BYTES, VIDEO_PROXY_DIR,
    video_content_type,
};
pub use presets::{
    ENV_PRESETS_DIR, ExportPresetSettings, MAX_PRESET_FILE_BYTES, MAX_PRESET_FILES_PER_CALL,
    MAX_PRESET_NAME_CHARS, MAX_PRESETS, PRESET_FORMAT, PRESET_FORMAT_VERSION, PRESETS_DIR,
    PresetFileFailure, PresetImportReport, PresetInfo, PresetKind, PresetStore, apply_preset_label,
    default_presets_dir, develop_preset_settings, is_valid_preset_id,
};
pub use previews::{L0_FORMAT_VERSION, L1_FORMAT_VERSION};
pub use profiles::{MAX_PROFILE_ENTRIES, PROFILES_DIR, inspect_profile_file, load_profile_files};
pub use region::{MAX_REGION_OUTPUT_PIXELS, MAX_REGION_SCALE, MAX_REGION_SOURCE_PIXELS};
pub use relink::{MAX_RELINK_CANDIDATES, MAX_RELINK_TARGETS};
pub use search::MAX_RANGE_LEN;
pub use settings::{
    KEY_DEFAULT_EXPORT_COLOR_SPACE, KEY_DEFAULT_UTC_OFFSET_MINUTES, KEY_HASH_IN_BACKGROUND,
    KEY_HASH_NETWORK_VOLUMES, KEY_IMPORT_SEPARATE_RAW_JPEG, KEY_IMPORT_SKIP_DUPLICATES,
    KEY_PREVIEW_CACHE_BYTES, KEY_PREVIEW_CACHE_DIR, KEY_VIDEO_PROXY_CACHE_BYTES,
    MIN_PREVIEW_CACHE_BYTES, UTC_OFFSET_RANGE_MINUTES,
};
pub use snapshots::apply_snapshot_label;
pub use stacks::MAX_AUTO_STACK_GAP_SECONDS;
pub use timeline::{MAX_TIMELINE_GAP_SECONDS, MAX_TIMELINE_GROUPS};
pub use types::*;
pub use view::{MAX_VIEW_PANES, MAX_VIEW_SOURCES};
pub use volumes::{OsVolumes, VolumeIdentity, VolumeProbe};
pub use watermark::{
    PREVIEW_DEFAULT_SIZE, PREVIEW_MAX_EDGE, PREVIEW_MIN_EDGE, WatermarkPreview,
    render_watermark_preview, system_font_dirs,
};

// 型付きのメソッドの引数・応答で使う、依存先の crate の型（UI がまとめて使えるように）。
pub use genzo_catalog::{
    MAX_CAPTURE_SHIFT_SECONDS, MAX_CAPTURE_TIME_EDITS, MAX_LOCATION_EDITS, MAX_SNAPSHOT_NAME_CHARS,
};
pub use genzo_model::{
    AssetId, AssetKind, CaptureTime, CollectionId, CollectionKind, ColorLabel, CompositeMode,
    CompositeSettings, DevelopSettings, ExportFormat, ExportSettings, ExportSize, FileStatus, Flag,
    GpsCoord, KeywordId, OutputColorSpace, OutputSharpening, Rating, RenderQuality, RgbColor,
    SharpenAmount, SharpenMedia, SmartCondition, SmartRule, StackId, VariantId, VideoExport,
    Watermark, WatermarkAnchor, WatermarkContent, ZoneMode,
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
