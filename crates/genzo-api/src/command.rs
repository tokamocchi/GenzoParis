//! コマンド（UI → コア。04 の 1.5 節）と応答。
//!
//! 案 A（Tauri）では JSON のコマンドとして受け取り、[`Core::execute`] に渡して応答を JSON で返す。
//! 案 B と CLI は、同じ機能を [`Core`] の型付きのメソッドで直接呼べる。
//!
//! JSON では `{"type": "set_rating", "variant_ids": [1, 2], "rating": 3}` のように、`type` で種類を表す。
//! 応答も `{"type": "updated", "count": 2}` の形。失敗は [`ApiError`]（UI には [`crate::ErrorInfo`]）。
//!
//! サムネイルのバイト列（[`Response::Thumbnail`]）は JSON では数値の配列になり大きいので、UI は
//! カスタム URI スキーム（`genzo://thumb/{variant_id}?rev={rev}`）の処理で [`Core::thumbnail`] を直接
//! 呼ぶこと。現像のプレビューのピクセルも、[`Core::develop_frame`] で受け取ってビューポートに直接描く。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use genzo_model::{
    CollectionId, CollectionKind, ColorLabel, CompositeSettings, CropRect, DevelopSettings,
    ExportFormat, ExportSettings, Flag, FolderId, GpsCoord, HistoryEntryId, KeywordId,
    MetadataField, Rating, RetouchSpot, SmartRule, SnapshotId, StackId, VariantId, Watermark,
};
use serde::{Deserialize, Serialize};

use crate::core::Core;
use crate::error::ApiError;
use crate::maintenance::RestorePlan;
use crate::naming::RenameRule;
use crate::presets::{ExportPresetSettings, PresetImportReport, PresetInfo, PresetKind};
use crate::types::{
    AutoStackReport, BackupEntry, BeforeInfo, BeforeSource, Capabilities, CaptureDevice,
    CaptureDeviceKey, CaptureTarget, CaptureTimeEdit, CaptureTimeEditInfo, CaptureTimePlan,
    CaptureTimeUndo, CatalogSchema, CatalogStats, CollectionInfo, ContentHashStatus, CopySettings,
    CoreSettings, DeleteKind, DeletePlan, DeleteReport, DevelopState, DisplayInfo,
    DuplicateGroupList, Facets, FileCheckReport, FileIdentity, FolderInfo, FrameInfo, HistoryItem,
    ImportOptions, IndexOf, JobInfo, KeywordHit, KeywordInfo, KeywordUsage, LocationEditInfo,
    LocationEditPlan, LocationUndo, MaskOverlayInfo, MetadataSummary, NamePreview, PasteReport,
    PasteVersion, PickedMaskColor, PickedWhiteBalance, RangeResult, RegionInfo, RegionRequest,
    SearchFilter, SearchResult, SearchSort, SettingGroups, SettingsUpdate, ShownVariants,
    StackInfo, StartupReport, ThumbnailData, Timeline, VariantDetails, VariantKeywords,
    VideoPlayback, VideoProxyCacheStatus, ViewOutputSize,
};
use crate::types::{
    FileMovePlan, FileMoveReport, FileRenamePlan, ProfileEntry, ProfileImportReport, ProfileKind,
    RelinkReport, RelinkRequest, RelocatePlan, RenameSpec, SnapshotInfo, SnapshotSource,
    VolumeInfo,
};

/// コマンド（UI → コア）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    // --- ジョブ ---
    /// フォルダを取り込む（P3 のジョブ）。→ [`Response::JobStarted`]
    ImportFolder {
        /// フォルダ。
        path: PathBuf,
        /// 配下のフォルダも取り込むか。
        recursive: bool,
        /// 重複と思われるもの・RAW と JPEG の扱い（IMP-04・IMP-06。省いた項目はカタログの設定に従う）。
        #[serde(default)]
        options: ImportOptions,
    },
    /// ジョブの情報。→ [`Response::Job`]
    Job {
        /// ジョブ。
        job_id: u64,
    },
    /// ジョブが終わるまで待つ（`timeout_ms` を過ぎたら、その時点の情報）。→ [`Response::Job`]
    WaitJob {
        /// ジョブ。
        job_id: u64,
        /// 待つ時間の上限（ミリ秒。`None` なら終わるまで）。
        timeout_ms: Option<u64>,
    },
    /// ジョブを取り消す。→ [`Response::Ok`]
    CancelJob {
        /// ジョブ。
        job_id: u64,
    },

    // --- 検索・選別 ---
    /// 検索する。→ [`Response::Search`]
    Search {
        /// 条件（JSON の形は `SearchFilter` と同じ。条件が多く大きいので、コマンドの大きさをそろえるため箱に
        /// 入れる）。
        #[serde(default)]
        filter: Box<SearchFilter>,
        /// 並べ替え。
        #[serde(default)]
        sort: SearchSort,
    },
    /// 表示範囲の詳細。→ [`Response::Range`]
    Range {
        /// 世代。
        generation: u64,
        /// 先頭の位置。
        start: u64,
        /// 件数。
        len: u64,
    },
    /// variant の位置。→ [`Response::IndexOf`]
    IndexOf {
        /// variant。
        variant_id: VariantId,
    },
    /// 写真のうち、表示中の検索結果に出ているもの（選択を表示している写真だけにする。折りたたんだスタックに
    /// 隠れた写真・絞り込みに合わない写真は含まない。LIB-12）。→ [`Response::ShownVariants`]
    ShownVariants {
        /// variant。
        variant_ids: Vec<VariantId>,
    },
    /// 絞り込みの候補（カメラ・レンズ・動画のコーデックと件数。[`crate::Core::facets`]）。
    /// → [`Response::Facets`]
    Facets,
    /// 評価を変える。→ [`Response::Updated`]
    SetRating {
        /// variant。
        variant_ids: Vec<VariantId>,
        /// 評価（0〜5）。
        rating: Rating,
    },
    /// フラグを変える。→ [`Response::Updated`]
    SetFlag {
        /// variant。
        variant_ids: Vec<VariantId>,
        /// フラグ（-1 / 0 / 1）。
        flag: Flag,
    },
    /// カラーラベルを変える。→ [`Response::Updated`]
    SetColorLabel {
        /// variant。
        variant_ids: Vec<VariantId>,
        /// ラベル（`null` でラベルなし）。
        label: Option<ColorLabel>,
    },
    /// キャプションを変える。→ [`Response::Updated`]
    SetCaption {
        /// variant。
        variant_ids: Vec<VariantId>,
        /// キャプション（`null` で削除）。
        caption: Option<String>,
    },
    /// 説明のメタデータ（タイトル・キャプション・作成者・著作権など。LIB-15）を一括で変える
    /// （[`crate::Core::set_metadata`]）。→ [`Response::Updated`]（変えた asset の数）
    SetMetadata {
        /// variant。
        variant_ids: Vec<VariantId>,
        /// 変える項目と値（`{"title": "清水寺", "creator": null}`。`null`・空の文字列で削除。並べない項目は
        /// 変えない）。
        changes: BTreeMap<MetadataField, Option<String>>,
    },
    /// 複数の写真の説明のメタデータのまとめ（[`crate::Core::metadata_summary`]）。→ [`Response::Metadata`]
    MetadataSummary {
        /// variant。
        variant_ids: Vec<VariantId>,
    },
    /// variant の詳細。→ [`Response::Details`]
    VariantDetails {
        /// variant。
        variant_id: VariantId,
    },
    /// フォルダの一覧。→ [`Response::Folders`]
    Folders,
    /// ファイルの状態を確かめる。→ [`Response::FileCheck`]
    CheckFiles {
        /// variant。
        variant_ids: Vec<VariantId>,
    },
    /// 条件に合う写真の数（表示中の検索結果は変えない。スマートコレクションの条件の編集中の件数など）。
    /// → [`Response::Count`]
    Count {
        /// 条件。
        #[serde(default)]
        filter: Box<SearchFilter>,
    },

    // --- キーワード（LIB-09） ---
    /// すべてのキーワードと、直接付いている写真の数。→ [`Response::Keywords`]
    KeywordTree,
    /// 名前でキーワードを探す（部分一致。入力欄の補完）。→ [`Response::KeywordHits`]
    FindKeywords {
        /// 検索語。
        query: String,
        /// 件数の上限（省略時は 20。[`crate::MAX_KEYWORD_HITS`] まで）。
        #[serde(default)]
        limit: Option<usize>,
    },
    /// 階層のパスでキーワードを確保する（なければ作る）。→ [`Response::KeywordCreated`]
    CreateKeyword {
        /// 親（`null` で最上位）。
        #[serde(default)]
        parent_id: Option<KeywordId>,
        /// 親の下からのパス（`["京都", "祇園"]`）。
        path: Vec<String>,
    },
    /// キーワードの名前を変える。→ [`Response::Ok`]
    RenameKeyword {
        /// キーワード。
        keyword_id: KeywordId,
        /// 新しい名前。
        name: String,
    },
    /// キーワードを別の親の下へ移す。→ [`Response::Ok`]
    MoveKeyword {
        /// キーワード。
        keyword_id: KeywordId,
        /// 新しい親（`null` で最上位）。
        #[serde(default)]
        parent_id: Option<KeywordId>,
    },
    /// キーワードを削除する（子のキーワードと付与も）。→ [`Response::Updated`]（削除したキーワードの数）
    DeleteKeyword {
        /// キーワード。
        keyword_id: KeywordId,
    },
    /// 書き出すファイルのメタデータにキーワードを書くかを変える。→ [`Response::Ok`]
    SetKeywordExport {
        /// キーワード。
        keyword_id: KeywordId,
        /// 書くか。
        include: bool,
    },
    /// キーワードの使われ方（削除の確認）。→ [`Response::KeywordUsage`]
    KeywordUsage {
        /// キーワード。
        keyword_id: KeywordId,
    },
    /// 写真にキーワードを付ける。→ [`Response::Updated`]（新しく付けた組の数）
    AddKeywords {
        /// 写真。
        variant_ids: Vec<VariantId>,
        /// 既存のキーワード。
        #[serde(default)]
        keyword_ids: Vec<KeywordId>,
        /// 階層のパス（なければ作る。`[["場所", "京都"]]`）。
        #[serde(default)]
        paths: Vec<Vec<String>>,
    },
    /// 写真からキーワードを外す。→ [`Response::Updated`]（外した組の数）
    RemoveKeywords {
        /// 写真。
        variant_ids: Vec<VariantId>,
        /// キーワード。
        keyword_ids: Vec<KeywordId>,
    },
    /// 写真に付いているキーワードと、それぞれが付いている写真の数。→ [`Response::VariantKeywords`]
    VariantKeywords {
        /// 写真。
        variant_ids: Vec<VariantId>,
    },

    // --- コレクション（LIB-10・LIB-11） ---
    /// すべてのコレクション。→ [`Response::Collections`]
    Collections {
        /// スマートコレクション・セットの写真の数も数えるか。
        #[serde(default)]
        with_counts: bool,
    },
    /// コレクションを作る。→ [`Response::CollectionCreated`]
    CreateCollection {
        /// 親のコレクションセット（`null` で最上位）。
        #[serde(default)]
        parent_id: Option<CollectionId>,
        /// 名前。
        name: String,
        /// 種類（`manual`・`smart`・`set`）。
        kind: CollectionKind,
        /// スマートコレクションの条件。
        #[serde(default)]
        rule: Option<SmartRule>,
        /// 手動のコレクションに入れる写真。
        #[serde(default)]
        variant_ids: Vec<VariantId>,
    },
    /// コレクションの名前を変える。→ [`Response::Ok`]
    RenameCollection {
        /// コレクション。
        collection_id: CollectionId,
        /// 新しい名前。
        name: String,
    },
    /// コレクションを別のコレクションセットへ移す。→ [`Response::Ok`]
    MoveCollection {
        /// コレクション。
        collection_id: CollectionId,
        /// 新しい親（`null` で最上位）。
        #[serde(default)]
        parent_id: Option<CollectionId>,
    },
    /// コレクションを削除する（セットなら中のものも。写真は消えない）。→ [`Response::Updated`]（削除した数）
    DeleteCollection {
        /// コレクション。
        collection_id: CollectionId,
    },
    /// スマートコレクションの条件を変える。→ [`Response::Ok`]
    SetSmartRule {
        /// コレクション。
        collection_id: CollectionId,
        /// 条件。
        rule: SmartRule,
    },
    /// 手動のコレクションに写真を入れる。→ [`Response::Updated`]（新しく入れた数）
    AddToCollection {
        /// コレクション。
        collection_id: CollectionId,
        /// 写真。
        variant_ids: Vec<VariantId>,
    },
    /// 手動のコレクションから写真を外す。→ [`Response::Updated`]（外した数）
    RemoveFromCollection {
        /// コレクション。
        collection_id: CollectionId,
        /// 写真。
        variant_ids: Vec<VariantId>,
    },
    /// 写真が入っている手動のコレクション。→ [`Response::VariantCollections`]
    VariantCollections {
        /// 写真。
        variant_id: VariantId,
    },

    // --- スタック（LIB-12） ---
    /// 写真をまとめて 1 つのスタックにする（既にスタックに入っている写真があれば、そのスタックもまとめる）。
    /// → [`Response::Stack`]
    CreateStack {
        /// 写真（2 枚以上）。
        variant_ids: Vec<VariantId>,
        /// 一番上にする写真（省略時は撮影日時の最も早いもの）。
        #[serde(default)]
        top: Option<VariantId>,
    },
    /// スタックに写真を足す（ほかのスタックの写真は移す）。→ [`Response::Updated`]（新しく足した数）
    AddToStack {
        /// スタック。
        stack_id: StackId,
        /// 写真。
        variant_ids: Vec<VariantId>,
    },
    /// 写真をスタックから外す（1 枚以下になったスタックは消える）。→ [`Response::Updated`]（外した数）
    RemoveFromStack {
        /// 写真。
        variant_ids: Vec<VariantId>,
    },
    /// 写真が入っているスタックを解除する（写真は消えない）。→ [`Response::Updated`]（解除したスタックの数）
    Unstack {
        /// 写真。
        variant_ids: Vec<VariantId>,
    },
    /// 写真をそのスタックの一番上にする。→ [`Response::Stack`]
    SetStackTop {
        /// 写真。
        variant_id: VariantId,
    },
    /// スタックの折りたたみの状態を変える。→ [`Response::Updated`]（状態の変わったスタックの数）
    SetStacksCollapsed {
        /// スタック（`null` ですべてのスタック）。
        #[serde(default)]
        stack_ids: Option<Vec<StackId>>,
        /// 折りたたむか（偽で展開）。
        collapsed: bool,
    },
    /// 写真が入っているスタック。→ [`Response::Stacks`]
    VariantStacks {
        /// 写真。
        variant_ids: Vec<VariantId>,
    },
    /// 撮影時刻の間隔で自動的にスタックを作る（[`crate::Core::auto_stack`]）。→ [`Response::AutoStack`]
    AutoStack {
        /// 対象（条件に合う写真のうち、スタックに入っていないもの）。
        #[serde(default)]
        filter: Box<SearchFilter>,
        /// 前の写真との間隔の上限（秒。0〜3,600）。
        gap_seconds: f64,
        /// 数えるだけ（何も変えない）か。
        #[serde(default)]
        dry_run: bool,
    },

    // --- 仮想コピー（LIB-13） ---
    /// 仮想コピーの名前を変える。→ [`Response::Ok`]
    RenameVirtualCopy {
        /// 仮想コピー。
        variant_id: VariantId,
        /// 新しい名前。
        name: String,
    },
    /// 仮想コピーをマスター（元の写真）にする（前のマスターは仮想コピーになる）。→ [`Response::MasterChanged`]
    SetAsMaster {
        /// 仮想コピー。
        variant_id: VariantId,
    },

    // --- サムネイル・プレビュー ---
    /// サムネイル（L0）。→ [`Response::Thumbnail`]
    Thumbnail {
        /// variant。
        variant_id: VariantId,
    },
    /// 標準プレビュー（L1）のファイル。→ [`Response::Path`]
    PreviewPath {
        /// variant。
        variant_id: VariantId,
    },
    /// 表示に使う標準プレビュー（L1）。なければ作る処理を予約して `null`（作ったら
    /// [`crate::Event::ThumbnailUpdated`]。[`crate::Core::request_preview`]）。→ [`Response::Path`]
    RequestPreview {
        /// variant。
        variant_id: VariantId,
    },
    /// サムネイル・プレビューを作り直す（P3 のジョブ）。→ [`Response::JobStarted`]
    RegeneratePreviews {
        /// variant。
        variant_ids: Vec<VariantId>,
    },

    // --- 現像 ---
    /// 写真を現像のために開く。→ [`Response::Develop`]
    OpenDevelop {
        /// variant。
        variant_id: VariantId,
    },
    /// 現像中の写真の状態。→ [`Response::DevelopOrNone`]
    DevelopState,
    /// 現像設定を変える（描画を投入して、すぐに返る）。→ [`Response::Generation`]
    SetSettings {
        /// 設定。
        settings: Box<DevelopSettings>,
        /// スライダーのドラッグ中か。
        dragging: bool,
    },
    /// ドラッグを終える（履歴に 1 件記録して保存）。→ [`Response::Develop`]
    EndDrag,
    /// 現像中の写真の処理バージョンを最新に更新する（DATA-09。履歴に 1 件記録して保存）。
    /// → [`Response::Develop`]
    UpgradeProcessVersion,
    /// 選んだ写真の処理バージョンをまとめて最新に更新する（DATA-09。[`crate::Core::upgrade_process_versions`]。
    /// すでに最新の写真・動画は飛ばす）。→ [`Response::Updated`]（更新した数）
    UpgradeProcessVersions {
        /// 対象。
        variant_ids: Vec<VariantId>,
    },
    /// 現在の設定で同期で描く。→ [`Response::Frame`]
    RenderDevelop,
    /// 現像中の写真の表示範囲を描く（ルーペの等倍表示・拡大とパン。P0 の最新の 1 件だけ。
    /// [`crate::Core::render_region`]）。画素は [`crate::Core::region_frame`] で取る。
    /// → [`Response::Region`]
    RenderRegion {
        /// 要求。
        request: RegionRequest,
    },
    /// 実行中の表示範囲の描画を取り消す（ビフォーの表示範囲も）。→ [`Response::Ok`]
    CancelRegion,
    /// ビフォー / アフターの表示（DEV-31）のビフォーを設定する（`null` でやめる。
    /// [`crate::Core::set_before`]）。→ [`Response::Before`]
    SetBefore {
        /// 出どころ（`{"kind": "initial"}` など。`null` でやめる）。
        source: Option<BeforeSource>,
    },
    /// 今のビフォーの設定で同期で描く（CLI・テスト用）。→ [`Response::Frame`]
    RenderBefore,
    /// 世代 `generation` 以降のビフォーのプレビューを待つ。→ [`Response::FrameOrNone`]
    WaitForBefore {
        /// 世代。
        generation: u64,
        /// 待つ時間の上限（ミリ秒）。
        timeout_ms: u64,
    },
    /// ビフォーの表示範囲を描く（[`crate::Core::render_before_region`]）。画素は
    /// [`crate::Core::before_region_frame`] で取る。→ [`Response::Region`]
    RenderBeforeRegion {
        /// 要求。
        request: RegionRequest,
    },
    /// 比較表示（LIB-03）の面 `pane` で使う写真の、出力の画像のフル解像度の大きさ（等倍表示の準備。
    /// [`crate::Core::view_output_size`]）。→ [`Response::ViewSize`]
    ViewOutputSize {
        /// 面（0 から [`crate::MAX_VIEW_PANES`] 未満）。
        pane: u32,
        /// 写真。
        variant_id: VariantId,
    },
    /// 比較表示の面 `pane` に、写真の表示範囲を描く（現像中でない写真も描ける。面ごとに P0 の最新の 1 件だけ。
    /// [`crate::Core::render_view_region`]）。画素は [`crate::Core::view_region_frame`] で取る。
    /// → [`Response::Region`]
    RenderViewRegion {
        /// 面（0 から [`crate::MAX_VIEW_PANES`] 未満）。
        pane: u32,
        /// 要求（`variant_id` は描く写真）。
        request: RegionRequest,
    },
    /// 比較表示のすべての面の描画を取り消す。→ [`Response::Ok`]
    CancelViewRegions,
    /// 比較表示をやめる（描画を取り消し、展開した入力と結果を捨てる。[`crate::Core::release_view`]）。
    /// → [`Response::Ok`]
    ReleaseView,
    /// 世代 `generation` 以降のプレビューを待つ。→ [`Response::FrameOrNone`]
    WaitForFrame {
        /// 世代。
        generation: u64,
        /// 待つ時間の上限（ミリ秒）。
        timeout_ms: u64,
    },
    /// 保存待ちの変更をすぐに保存する。→ [`Response::Saved`]
    FlushDevelop,
    /// 現像中の写真を閉じる。→ [`Response::Ok`]
    CloseDevelop,
    /// 取り消す（Undo）。→ [`Response::SettingsOrNone`]
    Undo {
        /// variant。
        variant_id: VariantId,
    },
    /// やり直す（Redo）。→ [`Response::SettingsOrNone`]
    Redo {
        /// variant。
        variant_id: VariantId,
    },
    /// 履歴の項目の位置へ 1 回で移る（[`crate::Core::go_to_history`]）。→ [`Response::SettingsOrNone`]
    GoToHistory {
        /// variant。
        variant_id: VariantId,
        /// 移る先の履歴の項目（[`HistoryItem::id`]）。
        entry_id: HistoryEntryId,
    },
    /// 履歴。→ [`Response::History`]
    History {
        /// variant。
        variant_id: VariantId,
    },
    /// スナップショットの一覧（DEV-28。[`crate::Core::snapshots`]）。→ [`Response::Snapshots`]
    Snapshots {
        /// variant。
        variant_id: VariantId,
    },
    /// スナップショットを作る（DEV-28。[`crate::Core::create_snapshot`]）。→ [`Response::Snapshot`]
    CreateSnapshot {
        /// variant。
        variant_id: VariantId,
        /// 名前。
        name: String,
        /// 出どころ（`{"kind": "current"}`（省略時）・`{"kind": "history", "entry_id": 12}`）。
        #[serde(default)]
        source: SnapshotSource,
    },
    /// スナップショットの名前を変える。→ [`Response::Snapshot`]
    RenameSnapshot {
        /// スナップショット。
        snapshot_id: SnapshotId,
        /// 新しい名前。
        name: String,
    },
    /// スナップショットを削除する。→ [`Response::Ok`]
    DeleteSnapshot {
        /// スナップショット。
        snapshot_id: SnapshotId,
    },
    /// スナップショットをその写真に適用する（履歴に 1 件記録する。[`crate::Core::apply_snapshot`]）。
    /// → [`Response::Settings`]
    ApplySnapshot {
        /// スナップショット。
        snapshot_id: SnapshotId,
    },
    /// 現像設定（コピー）。→ [`Response::Settings`]
    CopySettings {
        /// variant。
        variant_id: VariantId,
    },
    /// 設定を複数の variant に適用する（DEV-30。[`crate::Core::paste_settings_with`]）。→ [`Response::Pasted`]
    PasteSettings {
        /// 設定。
        settings: Box<DevelopSettings>,
        /// 適用先。
        targets: Vec<VariantId>,
        /// 写す項目（省略時はすべて）。
        #[serde(default)]
        groups: SettingGroups,
        /// 貼り付け先の処理バージョンの扱い（省略時は変えない。DATA-09）。
        #[serde(default)]
        process_version: PasteVersion,
    },
    /// 仮想コピーを作る。→ [`Response::VariantCreated`]
    CreateVirtualCopy {
        /// 元の variant。
        variant_id: VariantId,
        /// 名前（省略時は「コピー n」）。
        name: Option<String>,
    },
    /// WB のスポイト: プレビューの上の点を中立にする WB（適用はしない。[`crate::Core::white_balance_at`]）。
    /// → [`Response::WhiteBalance`]
    WhiteBalanceAt {
        /// 現像中の写真。
        variant_id: VariantId,
        /// 点の x（UI が表示しているプレビューの画像の幅に対する割合。0〜1）。
        x: f64,
        /// 点の y（高さに対する割合。0〜1）。
        y: f64,
        /// クリックした画像（UI が表示しているプレビュー）の `FrameInfo::uncropped`（省略可。省略時は
        /// 最新に公開したプレビューの表示とみなす。[`crate::Core::white_balance_at`]）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uncropped: Option<bool>,
    },
    /// 色域の範囲（DEV-19）のスポイト: プレビューの上の点の色（適用はしない。[`crate::Core::mask_color_at`]）。
    /// → [`Response::MaskColor`]
    MaskColorAt {
        /// 現像中の写真。
        variant_id: VariantId,
        /// 点の x（UI が表示しているプレビューの画像の幅に対する割合。0〜1）。
        x: f64,
        /// 点の y（高さに対する割合。0〜1）。
        y: f64,
        /// クリックした画像の `FrameInfo::uncropped`（省略可。[`Command::WhiteBalanceAt`] と同じ）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uncropped: Option<bool>,
    },
    /// スポット修正（DEV-21）のコピー元を自動で選ぶ（適用はしない。[`crate::Core::retouch_auto_source`]）。
    /// → [`Response::RetouchSource`]
    RetouchAutoSource {
        /// 現像中の写真。
        variant_id: VariantId,
        /// スポット（位置・大きさ。ずれは使わない）。
        spot: RetouchSpot,
    },
    /// マスクの重ねて表示（DEV-18）の画像の情報（[`crate::Core::mask_overlay`]。画素は UI ではアプリのバイナリの
    /// 応答で取る）。→ [`Response::MaskOverlay`]
    MaskOverlay {
        /// 現像中の写真。
        variant_id: VariantId,
        /// マスクの番号（設定の `masks` の位置）。
        index: usize,
        /// 表示（省略時は最新に公開したプレビューの表示）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uncropped: Option<bool>,
    },
    /// 切り抜きの編集中の表示を切り替える（`uncropped` の間は切り抜く前の全体を描く。
    /// [`crate::Core::set_develop_view`]）。→ [`Response::Ok`]
    SetDevelopView {
        /// 切り抜く前の全体を描くか。
        uncropped: bool,
    },
    /// 描画が実際に使う切り抜き（回転で画像の外に出ないよう制限したもの。[`crate::Core::constrain_crop`]）。
    /// → [`Response::Crop`]
    ConstrainCrop {
        /// 現像中の写真。
        variant_id: VariantId,
        /// 切り抜き（G の外接矩形の各軸の割合）。
        crop: CropRect,
        /// 角度補正（度）。
        angle_deg: f32,
    },
    /// 画面の ICC プロファイルを設定する。→ [`Response::Display`]
    SetDisplayProfile {
        /// ICC プロファイルのバイト列（`null` なら取得できないとして sRGB とみなす）。
        icc: Option<Vec<u8>>,
    },

    // --- カメラプロファイル・ルック（DEV-17） ---
    /// 取り込んだカメラプロファイル・ルックの一覧（[`crate::Core::list_profiles`]）。→ [`Response::Profiles`]
    ListProfiles {
        /// 機種（渡すと、カメラプロファイルがその機種用かを [`ProfileEntry::matches_camera`] に入れる）。
        #[serde(default)]
        camera: Option<String>,
    },
    /// カメラプロファイル（.dcp）・ルック（.cube）を取り込む（[`crate::Core::import_profiles`]）。
    /// → [`Response::ProfileImport`]
    ImportProfiles {
        /// ファイル。
        paths: Vec<PathBuf>,
        /// 同じ ID で内容の違うものを置き換えるか。
        #[serde(default)]
        replace: bool,
    },
    /// 取り込んだものを一覧から除く（[`crate::Core::remove_profile`]）。→ [`Response::Updated`]（除いた数）
    RemoveProfile {
        /// 種類。
        kind: ProfileKind,
        /// ID。
        id: String,
    },

    // --- プリセット（DEV-29・EXP-02。`presets` の doc） ---
    /// プリセットの一覧（[`crate::Core::presets`]）。→ [`Response::Presets`]
    Presets {
        /// 種類。
        kind: PresetKind,
    },
    /// 現像プリセットの設定（含めない項目は既定値。[`crate::Core::develop_preset_settings`]）。
    /// → [`Response::Settings`]
    DevelopPresetSettings {
        /// プリセット。
        preset_id: String,
    },
    /// 写真の今の設定から現像プリセットを作る（[`crate::Core::create_develop_preset`]）。→ [`Response::Preset`]
    CreateDevelopPreset {
        /// 名前。
        name: String,
        /// フォルダ（空ならフォルダなし）。
        #[serde(default)]
        folder: String,
        /// 含める項目。
        groups: SettingGroups,
        /// 元にする写真。
        variant_id: VariantId,
    },
    /// 現像プリセットを写真の今の設定で置き換える（[`crate::Core::update_develop_preset`]）。
    /// → [`Response::Preset`]
    UpdateDevelopPreset {
        /// プリセット。
        preset_id: String,
        /// 含める項目。
        groups: SettingGroups,
        /// 元にする写真。
        variant_id: VariantId,
    },
    /// 現像プリセットを選んだ写真に適用する（動画は除き、除いた数を返す。[`crate::Core::apply_develop_preset`]）。
    /// → [`Response::Pasted`]
    ApplyDevelopPreset {
        /// プリセット。
        preset_id: String,
        /// 適用先。
        targets: Vec<VariantId>,
        /// 適用先の処理バージョンの扱い（省略時は変えない。DATA-09）。
        #[serde(default)]
        process_version: PasteVersion,
    },
    /// 書き出しプリセットを作る（[`crate::Core::create_export_preset`]）。→ [`Response::Preset`]
    CreateExportPreset {
        /// 名前。
        name: String,
        /// フォルダ（空ならフォルダなし）。
        #[serde(default)]
        folder: String,
        /// 中身。
        settings: ExportPresetSettings,
    },
    /// 書き出しプリセットの中身を置き換える（[`crate::Core::update_export_preset`]）。→ [`Response::Preset`]
    UpdateExportPreset {
        /// プリセット。
        preset_id: String,
        /// 中身。
        settings: ExportPresetSettings,
    },
    /// プリセットの名前を変える。→ [`Response::Preset`]
    RenamePreset {
        /// 種類。
        kind: PresetKind,
        /// プリセット。
        preset_id: String,
        /// 新しい名前。
        name: String,
    },
    /// プリセットをフォルダへ移す（空の名前でフォルダなし）。→ [`Response::Preset`]
    MovePreset {
        /// 種類。
        kind: PresetKind,
        /// プリセット。
        preset_id: String,
        /// フォルダ。
        folder: String,
    },
    /// フォルダの名前を変える（そのフォルダのプリセットをすべて移す）。→ [`Response::Updated`]（移した数）
    RenamePresetFolder {
        /// 種類。
        kind: PresetKind,
        /// 今のフォルダ。
        folder: String,
        /// 新しい名前（空ならフォルダなし）。
        name: String,
    },
    /// プリセットを削除する。→ [`Response::Ok`]
    DeletePreset {
        /// 種類。
        kind: PresetKind,
        /// プリセット。
        preset_id: String,
    },
    /// プリセットのファイルを読み込む（[`crate::Core::import_presets`]）。→ [`Response::PresetImport`]
    ImportPresets {
        /// ファイル。
        paths: Vec<PathBuf>,
        /// 種類（指定するとその種類のファイルだけ）。
        #[serde(default)]
        kind: Option<PresetKind>,
    },
    /// プリセットをファイルに書き出す（[`crate::Core::export_preset_files`]）。→ [`Response::Paths`]
    ExportPresetFiles {
        /// 種類。
        kind: PresetKind,
        /// プリセット。
        preset_ids: Vec<String>,
        /// 書き出し先のフォルダ。
        dest_dir: PathBuf,
    },
    /// 書き出しのファイル名の例（見本の写真。[`crate::Core::export_name_preview`]）。→ [`Response::NamePreview`]
    ExportNamePreview {
        /// ファイル名の規則（`null` なら元のファイル名）。
        #[serde(default)]
        file_naming: Option<RenameRule>,
        /// 形式（拡張子に使う）。
        #[serde(default)]
        format: ExportFormat,
    },

    /// 透かしの見本（EXP-03。[`crate::Core::watermark_preview`]）。→ [`Response::WatermarkPreview`]
    WatermarkPreview {
        /// 透かし。
        watermark: Watermark,
        /// 見本の画像の幅（省略時は 600）。
        #[serde(default)]
        width: Option<u32>,
        /// 見本の画像の高さ（省略時は 400）。
        #[serde(default)]
        height: Option<u32>,
    },
    /// OS の既定のフォントのフォルダ（EXP-03。透かしのフォントを選ぶ最初の場所。[`crate::Core::system_font_dirs`]）。
    /// → [`Response::Paths`]
    SystemFontDirs,

    // --- 書き出し ---
    /// 書き出す（P3 のジョブ。動画は元のファイルのままコピーする）。→ [`Response::JobStarted`]
    Export {
        /// variant。
        variant_ids: Vec<VariantId>,
        /// 書き出しの設定。
        settings: ExportSettings,
        /// 書き出し先のフォルダ。
        dest_dir: PathBuf,
        /// ファイル名の規則（EXP-02。省略・`null` なら元のファイル名）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_naming: Option<RenameRule>,
    },

    // --- 比較明・比較暗合成（DEV-34） ---
    /// 合成する前の確認（[`crate::Core::composite_check`]）。→ [`Response::CompositeCheck`]
    CompositeCheck {
        /// variant（動画は除く）。
        variant_ids: Vec<VariantId>,
        /// 合成の設定（省いた項目は既定値）。
        #[serde(default)]
        settings: CompositeSettings,
        /// 保存先のフォルダ（省略・`null` なら 1 枚目の写真と同じフォルダ）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dest_dir: Option<PathBuf>,
    },
    /// 合成する（P3 のジョブ。[`crate::Core::composite`]）。→ [`Response::JobStarted`]
    Composite {
        /// variant（動画は除く）。
        variant_ids: Vec<VariantId>,
        /// 合成の設定（省いた項目は既定値）。
        #[serde(default)]
        settings: CompositeSettings,
        /// 保存先のフォルダ（省略・`null` なら 1 枚目の写真と同じフォルダ）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dest_dir: Option<PathBuf>,
    },

    // --- 削除 ---
    /// 削除の計画（影響の一覧）。→ [`Response::DeletePlan`]
    PlanDelete {
        /// 種類。
        kind: DeleteKind,
        /// variant。
        variant_ids: Vec<VariantId>,
    },
    /// 確認した削除を実行する。→ [`Response::Deleted`]
    ExecuteDelete {
        /// 計画の ID。
        plan_id: u64,
    },

    // --- 設定・保守 ---
    /// 設定。→ [`Response::CoreSettings`]
    GetSettings,
    /// 設定を変える。→ [`Response::CoreSettings`]
    UpdateSettings {
        /// 変更。
        update: SettingsUpdate,
    },
    /// 手動のバックアップ。→ [`Response::Backup`]
    BackupNow,
    /// バックアップの一覧。→ [`Response::Backups`]
    ListBackups,
    /// 詳細チェック（P3 のジョブ）。→ [`Response::JobStarted`]
    CheckIntegrity,
    /// キャッシュの回収（P3 のジョブ）。→ [`Response::JobStarted`]
    CollectCacheGarbage,
    /// バックアップからの復元の準備（別のファイルに復元し、差し替えの手順を返す）。→ [`Response::Restore`]
    PrepareRestore {
        /// バックアップ。
        backup: PathBuf,
    },
    /// 起動時の確認の結果。→ [`Response::Startup`]
    StartupReport,
    /// カタログの件数とスキーマの版。→ [`Response::CatalogStats`]
    CatalogStats,
    /// カタログの構造（表・列・索引・ビュー・トリガー。ORG-07）。→ [`Response::CatalogSchema`]
    CatalogSchema,
    /// 機能の有無。→ [`Response::Capabilities`]
    Capabilities,

    // --- 内容による同一性（ORG-03・IMP-04） ---
    /// 全体のハッシュのないファイルの内容のハッシュを計算する（専用のスレッドのジョブ。既に計算中なら、
    /// そのジョブ）。→ [`Response::JobStarted`]
    ComputeContentHashes {
        /// ネットワークのボリュームのファイルも読むか。
        #[serde(default)]
        include_network: bool,
    },
    /// 内容のハッシュの状況。→ [`Response::ContentHashStatus`]
    ContentHashStatus,
    /// 内容が同じファイルの組。→ [`Response::DuplicateGroups`]
    DuplicateGroups {
        /// 返す組の上限（最大 [`crate::DUPLICATE_GROUPS_MAX`]）。
        #[serde(default = "default_duplicate_groups_limit")]
        limit: usize,
    },
    /// ファイルの内容で、同じ内容の登録済みのファイルを探す。→ [`Response::FileIdentity`]
    IdentifyFile {
        /// 調べるファイル。
        path: PathBuf,
    },
    /// コピー取り込みの設定の、名前の例（IMP-03。見本の写真のコピー先。カタログ・ファイルは使わない）。
    /// → [`Response::NamePreview`]
    ImportNamePreview {
        /// コピーの設定（コピー先のフォルダは使わない）。
        copy: CopySettings,
    },

    // --- 動画の再生（VID-04） ---
    /// 動画の再生の情報（元のファイルを読めるか・再生用のファイル・作れない理由。
    /// [`crate::Core::video_playback`]）。→ [`Response::VideoPlayback`]
    VideoPlayback {
        /// 動画。
        variant_id: VariantId,
    },
    /// 再生用のファイルを作る（専用のスレッドのジョブ。同じ動画を作っている途中なら、そのジョブ。
    /// [`crate::Core::create_video_proxy`]）。→ [`Response::JobStarted`]
    CreateVideoProxy {
        /// 動画。
        variant_id: VariantId,
    },
    /// 再生用のファイルのキャッシュの状況。→ [`Response::VideoProxyCache`]
    VideoProxyCache,
    /// 再生用のファイルをすべて消す（作り直せる）。→ [`Response::VideoProxyCache`]
    ClearVideoProxies,

    // --- 撮影日時の一括修正（LIB-16）・タイムライン（ORG-01） ---
    /// 機材の一覧（[`crate::Core::capture_devices`]）。→ [`Response::CaptureDevices`]
    CaptureDevices,
    /// 写真の機材（[`crate::Core::capture_device_of`]）。→ [`Response::CaptureDevice`]
    CaptureDeviceOf {
        /// 写真。
        variant_id: VariantId,
    },
    /// 撮影日時の修正の見込み（何も変えない。[`crate::Core::plan_capture_time_edit`]）。
    /// → [`Response::CaptureTimePlan`]
    PlanCaptureTimeEdit {
        /// 対象。
        target: CaptureTarget,
        /// 修正。
        edit: CaptureTimeEdit,
    },
    /// 撮影日時を修正する（[`crate::Core::edit_capture_times`]）。→ [`Response::CaptureTimePlan`]
    EditCaptureTimes {
        /// 対象。
        target: CaptureTarget,
        /// 修正。
        edit: CaptureTimeEdit,
    },
    /// 撮影日時の修正の履歴。→ [`Response::CaptureTimeEdits`]
    CaptureTimeEdits,
    /// 最後の撮影日時の修正を取り消す。→ [`Response::CaptureTimeUndone`]
    UndoCaptureTimeEdit,
    /// 表示中の検索結果のタイムライン（[`crate::Core::timeline`]）。→ [`Response::Timeline`]
    Timeline {
        /// 世代。
        generation: u64,
        /// イベントの区切りの間隔（秒。0 ならイベントで分けない）。
        #[serde(default)]
        gap_seconds: u32,
    },

    // --- 位置情報の後付け（LIB-21） ---
    /// 位置の変更の見込み（何も変えない。[`crate::Core::plan_location_edit`]）。
    /// → [`Response::LocationEditPlan`]
    PlanLocationEdit {
        /// 写真（動画は除く。同じ写真の仮想コピーは位置を共有する）。
        variant_ids: Vec<VariantId>,
        /// 付ける位置（`{"lat": 35.681236, "lon": 139.767125}`）。`null` は付けた位置を消して、カメラの位置に
        /// 戻す。省略はできない（`required_option`）。
        #[serde(deserialize_with = "required_option")]
        location: Option<GpsCoord>,
    },
    /// 位置を付ける・付けた位置を消す（[`crate::Core::edit_locations`]）。→ [`Response::LocationEditPlan`]
    EditLocations {
        /// 写真（動画は除く）。
        variant_ids: Vec<VariantId>,
        /// 付ける位置（`null` は付けた位置を消す。省略はできない）。
        #[serde(deserialize_with = "required_option")]
        location: Option<GpsCoord>,
    },
    /// 位置の変更の履歴。→ [`Response::LocationEdits`]
    LocationEdits,
    /// 最後の位置の変更を取り消す。→ [`Response::LocationUndone`]
    UndoLocationEdit,

    // --- ボリューム・ファイル操作・見つからない写真（FILE-02〜FILE-04） ---
    /// ボリュームの一覧（`refresh` なら、つながりを確かめ直す。[`crate::Core::refresh_volumes`]）。
    /// → [`Response::Volumes`]
    Volumes {
        /// つながりを確かめ直すか。
        #[serde(default)]
        refresh: bool,
    },
    /// 写真の移動の予定（[`crate::Core::plan_move_files`]）。→ [`Response::MovePlan`]
    PlanMoveFiles {
        /// 写真。
        variant_ids: Vec<VariantId>,
        /// 移動先のフォルダ。
        dest_folder: FolderId,
    },
    /// 写真を移動する（ジョブ。[`crate::Core::move_files`]）。→ [`Response::JobStarted`]
    MoveFiles {
        /// 写真。
        variant_ids: Vec<VariantId>,
        /// 移動先のフォルダ。
        dest_folder: FolderId,
    },
    /// 写真の名前の変更の予定（[`crate::Core::plan_rename_files`]）。→ [`Response::RenamePlan`]
    PlanRenameFiles {
        /// 写真（テンプレートの連番はこの順）。
        variant_ids: Vec<VariantId>,
        /// 名前の付け方。
        spec: RenameSpec,
    },
    /// 写真の名前を変える（[`crate::Core::rename_files`]）。→ [`Response::FilesMoved`]
    RenameFiles {
        /// 写真。
        variant_ids: Vec<VariantId>,
        /// 名前の付け方。
        spec: RenameSpec,
    },
    /// フォルダを作る（[`crate::Core::create_folder`]）。→ [`Response::Folder`]
    CreateFolder {
        /// 親のフォルダ。
        parent_id: FolderId,
        /// 名前。
        name: String,
    },
    /// フォルダの名前を変える（[`crate::Core::rename_folder`]）。→ [`Response::Folder`]
    RenameFolder {
        /// フォルダ。
        folder_id: FolderId,
        /// 新しい名前。
        name: String,
    },
    /// フォルダを別のフォルダの中へ移す（[`crate::Core::move_folder`]）。→ [`Response::Folder`]
    MoveFolder {
        /// フォルダ。
        folder_id: FolderId,
        /// 移動先の親のフォルダ。
        parent_id: FolderId,
    },
    /// 写真が登録されていないフォルダ（と配下）をカタログから除く（[`crate::Core::remove_folder`]。ディスクには
    /// 触れない）。→ [`Response::Updated`]（除いたフォルダの数）
    RemoveFolder {
        /// フォルダ。
        folder_id: FolderId,
    },
    /// 見つからない写真を確かめる（ジョブ。[`crate::Core::check_missing`]）。→ [`Response::JobStarted`]
    CheckMissing {
        /// フォルダ（配下を含む。`None` ならすべて）。
        #[serde(default)]
        folder_id: Option<FolderId>,
    },
    /// 再リンクの候補を探す（ジョブ。[`crate::Core::find_relink_candidates`]）。→ [`Response::JobStarted`]
    FindRelinkCandidates {
        /// 写真（`None` ならすべての見つからないファイル）。
        #[serde(default)]
        variant_ids: Option<Vec<VariantId>>,
        /// 探すフォルダ。
        search_dir: PathBuf,
        /// 配下のフォルダも探すか。
        #[serde(default = "default_true")]
        recursive: bool,
    },
    /// 見つからないファイルを結び付け直す（[`crate::Core::relink_files`]）。→ [`Response::Relinked`]
    RelinkFiles {
        /// 結び付けるもの。
        links: Vec<RelinkRequest>,
    },
    /// フォルダの場所の指定し直しの予定（[`crate::Core::plan_relocate_folder`]）。→ [`Response::RelocatePlan`]
    PlanRelocateFolder {
        /// フォルダ。
        folder_id: FolderId,
        /// 新しい場所。
        path: PathBuf,
    },
    /// フォルダの場所を指定し直す（[`crate::Core::relocate_folder`]）。→ [`Response::RelocatePlan`]
    RelocateFolder {
        /// フォルダ。
        folder_id: FolderId,
        /// 新しい場所。
        path: PathBuf,
    },
}

fn default_true() -> bool {
    true
}

/// [`Command::DuplicateGroups`] の上限の既定値。
fn default_duplicate_groups_limit() -> usize {
    100
}

/// 省略できない `Option` の項目を読む（`null` は `None`、項目がなければエラー）。serde の既定では、省いた
/// `Option` の項目は `None` になる。位置（[`Command::EditLocations`]）の `null` は「付けた位置を消す」なので、
/// 項目の書き忘れを「消す」と読まないようにする。
fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// 応答（[`Command`] ごとの結果）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// 成功した（返す値がない）。
    Ok,
    /// ジョブを投入した。
    JobStarted {
        /// ジョブ。
        job_id: u64,
    },
    /// ジョブの情報。
    Job(JobInfo),
    /// 検索の結果。
    Search(SearchResult),
    /// 表示範囲の詳細。
    Range(RangeResult),
    /// variant の位置。
    IndexOf(IndexOf),
    /// 表示中の検索結果に出ている写真。
    ShownVariants(ShownVariants),
    /// 絞り込みの候補。
    Facets(Facets),
    /// 変更した数。
    Updated {
        /// 数。
        count: u64,
    },
    /// 設定の貼り付けの結果（[`Command::PasteSettings`]）。
    Pasted(PasteReport),
    /// variant の詳細。
    Details(Box<VariantDetails>),
    /// 説明のメタデータのまとめ。
    Metadata(MetadataSummary),
    /// フォルダの一覧。
    Folders {
        /// フォルダ。
        folders: Vec<FolderInfo>,
    },
    /// ファイルの状態の確認の結果。
    FileCheck(FileCheckReport),
    /// 写真の数。
    Count {
        /// 数。
        count: u64,
    },
    /// キーワードの一覧。
    Keywords {
        /// キーワード（名前の順）。
        keywords: Vec<KeywordInfo>,
    },
    /// 名前で探したキーワード。
    KeywordHits {
        /// キーワード（前方一致のものが先）。
        hits: Vec<KeywordHit>,
    },
    /// 確保したキーワード。
    KeywordCreated {
        /// キーワード。
        keyword_id: KeywordId,
    },
    /// キーワードの使われ方。
    KeywordUsage(KeywordUsage),
    /// 写真に付いているキーワード。
    VariantKeywords(VariantKeywords),
    /// コレクションの一覧。
    Collections {
        /// コレクション（名前の順）。
        collections: Vec<CollectionInfo>,
    },
    /// 作ったコレクション。
    CollectionCreated {
        /// コレクション。
        collection_id: CollectionId,
    },
    /// 写真が入っている手動のコレクション。
    VariantCollections {
        /// コレクション（名前の順）。
        collection_ids: Vec<CollectionId>,
    },
    /// スタック。
    Stack(StackInfo),
    /// スタックの一覧。
    Stacks {
        /// スタック。
        stacks: Vec<StackInfo>,
    },
    /// 自動スタックの結果。
    AutoStack(AutoStackReport),
    /// マスターを入れ替えた。
    MasterChanged {
        /// 前のマスター（仮想コピーになった。すでにマスターだったなら自分）。
        previous_master: VariantId,
    },
    /// サムネイル（なければ `null`）。
    Thumbnail {
        /// サムネイル。
        thumbnail: Option<ThumbnailData>,
    },
    /// ファイル（なければ `null`）。
    Path {
        /// パス。
        path: Option<PathBuf>,
    },
    /// 現像中の写真の状態。
    Develop(Box<DevelopState>),
    /// 現像中の写真の状態（開いていなければ `null`）。
    DevelopOrNone {
        /// 状態。
        state: Option<Box<DevelopState>>,
    },
    /// 設定の世代。
    Generation {
        /// 世代。
        generation: u64,
    },
    /// プレビューの情報。
    Frame(Box<FrameInfo>),
    /// 表示範囲の描画の情報（画素は [`crate::Core::region_frame`]。比較表示の面は
    /// [`crate::Core::view_region_frame`]）。
    Region(Box<RegionInfo>),
    /// 比較表示の面で使う写真の、出力の画像の大きさ。
    ViewSize(ViewOutputSize),
    /// ビフォーの状態（やめたら `null`）。
    Before {
        /// 状態。
        before: Option<BeforeInfo>,
    },
    /// プレビューの情報（まだなければ `null`）。
    FrameOrNone {
        /// 情報。
        frame: Option<Box<FrameInfo>>,
    },
    /// 保存したか。
    Saved {
        /// 保存したか。
        saved: bool,
    },
    /// 現像設定。
    Settings {
        /// 設定。
        settings: Box<DevelopSettings>,
    },
    /// 現像設定（なければ `null`。Undo / Redo できなかった）。
    SettingsOrNone {
        /// 設定。
        settings: Option<Box<DevelopSettings>>,
    },
    /// 履歴。
    History {
        /// 履歴（古い順）。
        entries: Vec<HistoryItem>,
    },
    /// スナップショットの一覧（DEV-28）。
    Snapshots {
        /// スナップショット（作った順）。
        snapshots: Vec<SnapshotInfo>,
    },
    /// スナップショット（作った・名前を変えたもの）。
    Snapshot {
        /// スナップショット。
        snapshot: SnapshotInfo,
    },
    /// プリセットの一覧（DEV-29・EXP-02）。
    Presets {
        /// プリセット（フォルダ → 名前の順）。
        presets: Vec<PresetInfo>,
        /// プリセットのフォルダ（分からなければ `null`）。
        #[serde(serialize_with = "crate::types::lossy_path")]
        dir: Option<PathBuf>,
    },
    /// プリセット（作った・変えたもの）。
    Preset {
        /// プリセット。
        preset: PresetInfo,
    },
    /// プリセットのファイルの読み込みの結果。
    PresetImport(PresetImportReport),
    /// 書いたファイル。
    Paths {
        /// ファイル。
        #[serde(serialize_with = "crate::types::lossy_path")]
        paths: Vec<PathBuf>,
    },
    /// 作った variant。
    VariantCreated {
        /// variant。
        variant_id: VariantId,
    },
    /// WB のスポイトの結果（色温度と tint）。
    WhiteBalance(PickedWhiteBalance),
    /// 色域の範囲のスポイトの結果（点の G 座標と OKLab）。
    MaskColor(PickedMaskColor),
    /// マスクの重ねて表示の画像の情報。
    MaskOverlay(MaskOverlayInfo),
    /// スポット修正のコピー元の自動の選択の結果（コピー元のずれ。O 座標。よい候補がなければ `None`）。
    RetouchSource {
        /// コピー元のずれ（`RetouchSpot::offset` にそのまま使える）。
        offset: Option<[f32; 2]>,
    },
    /// 切り抜き。
    Crop {
        /// 切り抜き（G の外接矩形の各軸の割合）。
        crop: CropRect,
    },
    /// 画面のプロファイルの情報。
    Display(DisplayInfo),
    /// 取り込んだカメラプロファイル・ルックの一覧。
    Profiles {
        /// 一覧（種類 → 表示の名前の順）。
        profiles: Vec<ProfileEntry>,
    },
    /// カメラプロファイル・ルックの取り込みの結果。
    ProfileImport(ProfileImportReport),
    /// 削除の計画。
    DeletePlan(DeletePlan),
    /// 削除の結果。
    Deleted(DeleteReport),
    /// 設定。
    CoreSettings(CoreSettings),
    /// バックアップ。
    Backup(BackupEntry),
    /// バックアップの一覧。
    Backups {
        /// バックアップ（新しい順）。
        backups: Vec<BackupEntry>,
    },
    /// 復元の計画。
    Restore(RestorePlan),
    /// 起動時の確認の結果。
    Startup(StartupReport),
    /// カタログの件数とスキーマの版。
    CatalogStats(CatalogStats),
    /// カタログの構造。
    CatalogSchema(CatalogSchema),
    /// 機能の有無。
    Capabilities(Capabilities),
    /// 内容のハッシュの状況。
    ContentHashStatus(ContentHashStatus),
    /// 内容が同じファイルの組。
    DuplicateGroups(DuplicateGroupList),
    /// ファイルの内容による識別の結果。
    FileIdentity(FileIdentity),
    /// コピー先の名前の例。
    NamePreview(NamePreview),
    /// 透かしの見本。
    WatermarkPreview(crate::watermark::WatermarkPreview),
    /// 合成する前の確認（DEV-34）。
    CompositeCheck(crate::types::CompositeCheck),
    /// 動画の再生の情報（VID-04）。
    VideoPlayback(VideoPlayback),
    /// 再生用のファイルのキャッシュの状況。
    VideoProxyCache(VideoProxyCacheStatus),
    /// 機材の一覧（LIB-16）。
    CaptureDevices {
        /// 機材。
        devices: Vec<CaptureDevice>,
    },
    /// 写真の機材。
    CaptureDevice(CaptureDeviceKey),
    /// 撮影日時の修正の見込み・結果。
    CaptureTimePlan(CaptureTimePlan),
    /// 撮影日時の修正の履歴（新しい順）。
    CaptureTimeEdits {
        /// 履歴。
        edits: Vec<CaptureTimeEditInfo>,
    },
    /// 撮影日時の修正を取り消した（履歴がなければ `null`）。
    CaptureTimeUndone {
        /// 取り消したもの。
        undone: Option<CaptureTimeUndo>,
    },
    /// タイムライン（ORG-01）。
    Timeline(Timeline),
    /// 位置の変更の見込み・結果（LIB-21）。
    LocationEditPlan(LocationEditPlan),
    /// 位置の変更の履歴（新しい順）。
    LocationEdits {
        /// 履歴。
        edits: Vec<LocationEditInfo>,
    },
    /// 位置の変更を取り消した（履歴がなければ `null`）。
    LocationUndone {
        /// 取り消したもの。
        undone: Option<LocationUndo>,
    },
    /// ボリュームの一覧（FILE-03）。
    Volumes {
        /// ボリューム。
        volumes: Vec<VolumeInfo>,
    },
    /// 写真の移動の予定（FILE-02）。
    MovePlan(FileMovePlan),
    /// 写真の名前の変更の予定（FILE-02）。
    RenamePlan(FileRenamePlan),
    /// 写真の名前の変更の結果。
    FilesMoved(FileMoveReport),
    /// フォルダ（作った・名前を変えた・移したもの）。
    Folder(FolderInfo),
    /// 再リンクの結果（FILE-04）。
    Relinked(RelinkReport),
    /// フォルダの場所の指定し直しの予定・結果（FILE-04）。
    RelocatePlan(RelocatePlan),
}

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

impl Core {
    /// コマンドを実行する（04 の 1.5 節）。ブロックしうるので、Tauri からは `spawn_blocking` で呼ぶ。
    pub fn execute(&self, command: Command) -> Result<Response, ApiError> {
        use Command as C;
        use Response as R;
        Ok(match command {
            C::ImportFolder {
                path,
                recursive,
                options,
            } => R::JobStarted {
                job_id: self.import_folder_with(&path, recursive, &options)?,
            },
            C::Job { job_id } => R::Job(self.job(job_id)?),
            C::WaitJob { job_id, timeout_ms } => R::Job(self.wait_job(job_id, timeout_ms.map(ms))?),
            C::CancelJob { job_id } => {
                self.cancel_job(job_id)?;
                R::Ok
            }
            C::Search { filter, sort } => R::Search(self.search(&filter, sort)?),
            C::Range {
                generation,
                start,
                len,
            } => R::Range(self.range(generation, start, len)?),
            C::IndexOf { variant_id } => R::IndexOf(self.index_of(variant_id)?),
            C::ShownVariants { variant_ids } => {
                R::ShownVariants(self.shown_variants(&variant_ids)?)
            }
            C::Facets => R::Facets(self.facets()?),
            C::SetRating {
                variant_ids,
                rating,
            } => R::Updated {
                count: self.set_rating(&variant_ids, rating)? as u64,
            },
            C::SetFlag { variant_ids, flag } => R::Updated {
                count: self.set_flag(&variant_ids, flag)? as u64,
            },
            C::SetColorLabel { variant_ids, label } => R::Updated {
                count: self.set_color_label(&variant_ids, label)? as u64,
            },
            C::SetCaption {
                variant_ids,
                caption,
            } => R::Updated {
                count: self.set_caption(&variant_ids, caption.as_deref())? as u64,
            },
            C::SetMetadata {
                variant_ids,
                changes,
            } => {
                let changes: Vec<(MetadataField, Option<String>)> = changes.into_iter().collect();
                R::Updated {
                    count: self.set_metadata(&variant_ids, &changes)? as u64,
                }
            }
            C::MetadataSummary { variant_ids } => R::Metadata(self.metadata_summary(&variant_ids)?),
            C::VariantDetails { variant_id } => {
                R::Details(Box::new(self.variant_details(variant_id)?))
            }
            C::Folders => R::Folders {
                folders: self.folders()?,
            },
            C::CheckFiles { variant_ids } => R::FileCheck(self.check_files(&variant_ids)?),
            C::Count { filter } => R::Count {
                count: self.count(&filter)?,
            },
            C::KeywordTree => R::Keywords {
                keywords: self.keyword_tree()?,
            },
            C::FindKeywords { query, limit } => R::KeywordHits {
                hits: self.find_keywords(&query, limit.unwrap_or(20))?,
            },
            C::CreateKeyword { parent_id, path } => R::KeywordCreated {
                keyword_id: self.create_keyword(parent_id, &path)?,
            },
            C::RenameKeyword { keyword_id, name } => {
                self.rename_keyword(keyword_id, &name)?;
                R::Ok
            }
            C::MoveKeyword {
                keyword_id,
                parent_id,
            } => {
                self.move_keyword(keyword_id, parent_id)?;
                R::Ok
            }
            C::DeleteKeyword { keyword_id } => R::Updated {
                count: self.delete_keyword(keyword_id)? as u64,
            },
            C::SetKeywordExport {
                keyword_id,
                include,
            } => {
                self.set_keyword_export(keyword_id, include)?;
                R::Ok
            }
            C::KeywordUsage { keyword_id } => R::KeywordUsage(self.keyword_usage(keyword_id)?),
            C::AddKeywords {
                variant_ids,
                keyword_ids,
                paths,
            } => R::Updated {
                count: self.add_keywords(&variant_ids, &keyword_ids, &paths)? as u64,
            },
            C::RemoveKeywords {
                variant_ids,
                keyword_ids,
            } => R::Updated {
                count: self.remove_keywords(&variant_ids, &keyword_ids)? as u64,
            },
            C::VariantKeywords { variant_ids } => {
                R::VariantKeywords(self.variant_keywords(&variant_ids)?)
            }
            C::Collections { with_counts } => R::Collections {
                collections: self.collections(with_counts)?,
            },
            C::CreateCollection {
                parent_id,
                name,
                kind,
                rule,
                variant_ids,
            } => R::CollectionCreated {
                collection_id: self.create_collection(
                    parent_id,
                    &name,
                    kind,
                    rule.as_ref(),
                    &variant_ids,
                )?,
            },
            C::RenameCollection {
                collection_id,
                name,
            } => {
                self.rename_collection(collection_id, &name)?;
                R::Ok
            }
            C::MoveCollection {
                collection_id,
                parent_id,
            } => {
                self.move_collection(collection_id, parent_id)?;
                R::Ok
            }
            C::DeleteCollection { collection_id } => R::Updated {
                count: self.delete_collection(collection_id)? as u64,
            },
            C::SetSmartRule {
                collection_id,
                rule,
            } => {
                self.set_smart_rule(collection_id, &rule)?;
                R::Ok
            }
            C::AddToCollection {
                collection_id,
                variant_ids,
            } => R::Updated {
                count: self.add_to_collection(collection_id, &variant_ids)? as u64,
            },
            C::RemoveFromCollection {
                collection_id,
                variant_ids,
            } => R::Updated {
                count: self.remove_from_collection(collection_id, &variant_ids)? as u64,
            },
            C::VariantCollections { variant_id } => R::VariantCollections {
                collection_ids: self.variant_collections(variant_id)?,
            },
            C::CreateStack { variant_ids, top } => R::Stack(self.create_stack(&variant_ids, top)?),
            C::AddToStack {
                stack_id,
                variant_ids,
            } => R::Updated {
                count: self.add_to_stack(stack_id, &variant_ids)? as u64,
            },
            C::RemoveFromStack { variant_ids } => R::Updated {
                count: self.remove_from_stack(&variant_ids)? as u64,
            },
            C::Unstack { variant_ids } => R::Updated {
                count: self.unstack(&variant_ids)? as u64,
            },
            C::SetStackTop { variant_id } => R::Stack(self.set_stack_top(variant_id)?),
            C::SetStacksCollapsed {
                stack_ids,
                collapsed,
            } => R::Updated {
                count: self.set_stacks_collapsed(stack_ids.as_deref(), collapsed)? as u64,
            },
            C::VariantStacks { variant_ids } => R::Stacks {
                stacks: self.variant_stacks(&variant_ids)?,
            },
            C::AutoStack {
                filter,
                gap_seconds,
                dry_run,
            } => R::AutoStack(self.auto_stack(&filter, gap_seconds, dry_run)?),
            C::RenameVirtualCopy { variant_id, name } => {
                self.rename_virtual_copy(variant_id, &name)?;
                R::Ok
            }
            C::SetAsMaster { variant_id } => R::MasterChanged {
                previous_master: self.set_as_master(variant_id)?,
            },
            C::Thumbnail { variant_id } => R::Thumbnail {
                thumbnail: self.thumbnail(variant_id)?,
            },
            C::PreviewPath { variant_id } => R::Path {
                path: self.preview_path(variant_id)?,
            },
            C::RequestPreview { variant_id } => R::Path {
                path: self.request_preview(variant_id)?,
            },
            C::RegeneratePreviews { variant_ids } => R::JobStarted {
                job_id: self.regenerate_previews(&variant_ids)?,
            },
            C::OpenDevelop { variant_id } => R::Develop(Box::new(self.open_develop(variant_id)?)),
            C::DevelopState => R::DevelopOrNone {
                state: self.develop_state()?.map(Box::new),
            },
            C::SetSettings { settings, dragging } => R::Generation {
                generation: self.set_settings(&settings, dragging)?,
            },
            C::EndDrag => R::Develop(Box::new(self.end_drag()?)),
            C::UpgradeProcessVersion => R::Develop(Box::new(self.upgrade_process_version()?)),
            C::UpgradeProcessVersions { variant_ids } => R::Updated {
                count: self.upgrade_process_versions(&variant_ids)? as u64,
            },
            C::RenderDevelop => R::Frame(Box::new(self.render_develop()?)),
            C::RenderRegion { request } => R::Region(Box::new(self.render_region(&request)?.info)),
            C::CancelRegion => {
                self.cancel_region()?;
                R::Ok
            }
            C::SetBefore { source } => R::Before {
                before: self.set_before(source)?,
            },
            C::RenderBefore => R::Frame(Box::new(self.render_before()?)),
            C::WaitForBefore {
                generation,
                timeout_ms,
            } => R::FrameOrNone {
                frame: self
                    .wait_for_before(generation, ms(timeout_ms))?
                    .map(Box::new),
            },
            C::ViewOutputSize { pane, variant_id } => {
                R::ViewSize(self.view_output_size(pane, variant_id)?)
            }
            C::RenderViewRegion { pane, request } => {
                R::Region(Box::new(self.render_view_region(pane, &request)?.info))
            }
            C::CancelViewRegions => {
                self.cancel_view_regions()?;
                R::Ok
            }
            C::ReleaseView => {
                self.release_view()?;
                R::Ok
            }
            C::RenderBeforeRegion { request } => {
                R::Region(Box::new(self.render_before_region(&request)?.info))
            }
            C::WaitForFrame {
                generation,
                timeout_ms,
            } => R::FrameOrNone {
                frame: self
                    .wait_for_frame(generation, ms(timeout_ms))?
                    .map(Box::new),
            },
            C::FlushDevelop => R::Saved {
                saved: self.flush_develop()?,
            },
            C::CloseDevelop => {
                self.close_develop()?;
                R::Ok
            }
            C::Undo { variant_id } => R::SettingsOrNone {
                settings: self.undo(variant_id)?.map(Box::new),
            },
            C::Redo { variant_id } => R::SettingsOrNone {
                settings: self.redo(variant_id)?.map(Box::new),
            },
            C::GoToHistory {
                variant_id,
                entry_id,
            } => R::SettingsOrNone {
                settings: self.go_to_history(variant_id, entry_id)?.map(Box::new),
            },
            C::History { variant_id } => R::History {
                entries: self.history(variant_id)?,
            },
            C::Snapshots { variant_id } => R::Snapshots {
                snapshots: self.snapshots(variant_id)?,
            },
            C::CreateSnapshot {
                variant_id,
                name,
                source,
            } => R::Snapshot {
                snapshot: self.create_snapshot(variant_id, &name, source)?,
            },
            C::RenameSnapshot { snapshot_id, name } => R::Snapshot {
                snapshot: self.rename_snapshot(snapshot_id, &name)?,
            },
            C::DeleteSnapshot { snapshot_id } => {
                self.delete_snapshot(snapshot_id)?;
                R::Ok
            }
            C::ApplySnapshot { snapshot_id } => R::Settings {
                settings: Box::new(self.apply_snapshot(snapshot_id)?),
            },
            C::CopySettings { variant_id } => R::Settings {
                settings: Box::new(self.develop_settings(variant_id)?),
            },
            C::PasteSettings {
                settings,
                targets,
                groups,
                process_version,
            } => R::Pasted(self.paste_settings_with(
                &settings,
                &targets,
                groups,
                process_version,
                crate::develop::HISTORY_LABEL_PASTE,
            )?),
            C::CreateVirtualCopy { variant_id, name } => R::VariantCreated {
                variant_id: self.create_virtual_copy(variant_id, name.as_deref())?,
            },
            C::WhiteBalanceAt {
                variant_id,
                x,
                y,
                uncropped,
            } => R::WhiteBalance(self.white_balance_at(variant_id, x, y, uncropped)?),
            C::MaskColorAt {
                variant_id,
                x,
                y,
                uncropped,
            } => R::MaskColor(self.mask_color_at(variant_id, x, y, uncropped)?),
            C::MaskOverlay {
                variant_id,
                index,
                uncropped,
            } => R::MaskOverlay(self.mask_overlay(variant_id, index, uncropped)?.info),
            C::RetouchAutoSource { variant_id, spot } => R::RetouchSource {
                offset: self.retouch_auto_source(variant_id, &spot)?,
            },
            C::SetDevelopView { uncropped } => {
                self.set_develop_view(uncropped)?;
                R::Ok
            }
            C::ConstrainCrop {
                variant_id,
                crop,
                angle_deg,
            } => R::Crop {
                crop: self.constrain_crop(variant_id, &crop, angle_deg)?,
            },
            C::SetDisplayProfile { icc } => R::Display(self.set_display_profile(icc.as_deref())?),
            C::ListProfiles { camera } => R::Profiles {
                profiles: self.list_profiles(camera.as_deref())?,
            },
            C::ImportProfiles { paths, replace } => {
                R::ProfileImport(self.import_profiles(&paths, replace)?)
            }
            C::RemoveProfile { kind, id } => {
                self.remove_profile(kind, &id)?;
                R::Updated { count: 1 }
            }
            C::Presets { kind } => R::Presets {
                presets: self.presets(kind)?,
                dir: self.presets_dir(),
            },
            C::DevelopPresetSettings { preset_id } => R::Settings {
                settings: Box::new(self.develop_preset_settings(&preset_id)?),
            },
            C::CreateDevelopPreset {
                name,
                folder,
                groups,
                variant_id,
            } => R::Preset {
                preset: self.create_develop_preset(&name, &folder, groups, variant_id)?,
            },
            C::UpdateDevelopPreset {
                preset_id,
                groups,
                variant_id,
            } => R::Preset {
                preset: self.update_develop_preset(&preset_id, groups, variant_id)?,
            },
            C::ApplyDevelopPreset {
                preset_id,
                targets,
                process_version,
            } => R::Pasted(self.apply_develop_preset(&preset_id, &targets, process_version)?),
            C::CreateExportPreset {
                name,
                folder,
                settings,
            } => R::Preset {
                preset: self.create_export_preset(&name, &folder, &settings)?,
            },
            C::UpdateExportPreset {
                preset_id,
                settings,
            } => R::Preset {
                preset: self.update_export_preset(&preset_id, &settings)?,
            },
            C::RenamePreset {
                kind,
                preset_id,
                name,
            } => R::Preset {
                preset: self.rename_preset(kind, &preset_id, &name)?,
            },
            C::MovePreset {
                kind,
                preset_id,
                folder,
            } => R::Preset {
                preset: self.move_preset(kind, &preset_id, &folder)?,
            },
            C::RenamePresetFolder { kind, folder, name } => R::Updated {
                count: self.rename_preset_folder(kind, &folder, &name)? as u64,
            },
            C::DeletePreset { kind, preset_id } => {
                self.delete_preset(kind, &preset_id)?;
                R::Ok
            }
            C::ImportPresets { paths, kind } => R::PresetImport(self.import_presets(&paths, kind)?),
            C::ExportPresetFiles {
                kind,
                preset_ids,
                dest_dir,
            } => R::Paths {
                paths: self.export_preset_files(kind, &preset_ids, &dest_dir)?,
            },
            C::ExportNamePreview {
                file_naming,
                format,
            } => R::NamePreview(self.export_name_preview(file_naming.as_ref(), format)),
            C::WatermarkPreview {
                watermark,
                width,
                height,
            } => {
                // 片方だけなら 3:2 の横長にする。
                let size = match (width, height) {
                    (Some(w), Some(h)) => Some((w, h)),
                    (Some(w), None) => Some((w, w.saturating_mul(2) / 3)),
                    (None, Some(h)) => Some((h.saturating_mul(3) / 2, h)),
                    (None, None) => None,
                };
                R::WatermarkPreview(self.watermark_preview(&watermark, size)?)
            }
            C::SystemFontDirs => R::Paths {
                paths: self.system_font_dirs(),
            },
            C::Export {
                variant_ids,
                settings,
                dest_dir,
                file_naming,
            } => R::JobStarted {
                job_id: self.export_with(
                    &variant_ids,
                    &settings,
                    &dest_dir,
                    file_naming.as_ref(),
                )?,
            },
            C::CompositeCheck {
                variant_ids,
                settings,
                dest_dir,
            } => R::CompositeCheck(self.composite_check(
                &variant_ids,
                &settings,
                dest_dir.as_deref(),
            )?),
            C::Composite {
                variant_ids,
                settings,
                dest_dir,
            } => R::JobStarted {
                job_id: self.composite(&variant_ids, &settings, dest_dir.as_deref())?,
            },
            C::PlanDelete { kind, variant_ids } => {
                R::DeletePlan(self.plan_delete(kind, &variant_ids)?)
            }
            C::ExecuteDelete { plan_id } => R::Deleted(self.execute_delete(plan_id)?),
            C::GetSettings => R::CoreSettings(self.settings()),
            C::UpdateSettings { update } => R::CoreSettings(self.update_settings(&update)?),
            C::BackupNow => R::Backup(self.backup_now()?),
            C::ListBackups => R::Backups {
                backups: self.list_backups()?,
            },
            C::CheckIntegrity => R::JobStarted {
                job_id: self.check_integrity()?,
            },
            C::CollectCacheGarbage => R::JobStarted {
                job_id: self.collect_cache_garbage()?,
            },
            C::PrepareRestore { backup } => R::Restore(crate::maintenance::prepare_restore(
                &backup,
                &self.inner.config.catalog_path,
            )?),
            C::StartupReport => R::Startup(self.startup_report()),
            C::CatalogStats => R::CatalogStats(self.catalog_stats()?),
            C::CatalogSchema => R::CatalogSchema(self.catalog_schema()?),
            C::Capabilities => R::Capabilities(self.capabilities()?),
            C::ComputeContentHashes { include_network } => R::JobStarted {
                job_id: self.compute_content_hashes(include_network)?,
            },
            C::ContentHashStatus => R::ContentHashStatus(self.content_hash_status()?),
            C::DuplicateGroups { limit } => R::DuplicateGroups(self.duplicate_groups(limit)?),
            C::IdentifyFile { path } => R::FileIdentity(self.identify_file(&path)?),
            C::ImportNamePreview { copy } => R::NamePreview(self.import_name_preview(&copy)),
            C::VideoPlayback { variant_id } => R::VideoPlayback(self.video_playback(variant_id)?),
            C::CreateVideoProxy { variant_id } => R::JobStarted {
                job_id: self.create_video_proxy(variant_id)?,
            },
            C::VideoProxyCache => R::VideoProxyCache(self.video_proxy_cache()?),
            C::ClearVideoProxies => R::VideoProxyCache(self.clear_video_proxies()?),
            C::CaptureDevices => R::CaptureDevices {
                devices: self.capture_devices()?,
            },
            C::CaptureDeviceOf { variant_id } => {
                R::CaptureDevice(self.capture_device_of(variant_id)?)
            }
            C::PlanCaptureTimeEdit { target, edit } => {
                R::CaptureTimePlan(self.plan_capture_time_edit(&target, &edit)?)
            }
            C::EditCaptureTimes { target, edit } => {
                R::CaptureTimePlan(self.edit_capture_times(&target, &edit)?)
            }
            C::CaptureTimeEdits => R::CaptureTimeEdits {
                edits: self.capture_time_edits()?,
            },
            C::UndoCaptureTimeEdit => R::CaptureTimeUndone {
                undone: self.undo_capture_time_edit()?,
            },
            C::Timeline {
                generation,
                gap_seconds,
            } => R::Timeline(self.timeline(generation, gap_seconds)?),
            C::PlanLocationEdit {
                variant_ids,
                location,
            } => R::LocationEditPlan(self.plan_location_edit(&variant_ids, location)?),
            C::EditLocations {
                variant_ids,
                location,
            } => R::LocationEditPlan(self.edit_locations(&variant_ids, location)?),
            C::LocationEdits => R::LocationEdits {
                edits: self.location_edits()?,
            },
            C::UndoLocationEdit => R::LocationUndone {
                undone: self.undo_location_edit()?,
            },
            C::Volumes { refresh } => R::Volumes {
                volumes: if refresh {
                    self.refresh_volumes()?
                } else {
                    self.volumes()?
                },
            },
            C::PlanMoveFiles {
                variant_ids,
                dest_folder,
            } => R::MovePlan(self.plan_move_files(&variant_ids, dest_folder)?),
            C::MoveFiles {
                variant_ids,
                dest_folder,
            } => R::JobStarted {
                job_id: self.move_files(&variant_ids, dest_folder)?,
            },
            C::PlanRenameFiles { variant_ids, spec } => {
                R::RenamePlan(self.plan_rename_files(&variant_ids, &spec)?)
            }
            C::RenameFiles { variant_ids, spec } => {
                R::FilesMoved(self.rename_files(&variant_ids, &spec)?)
            }
            C::CreateFolder { parent_id, name } => R::Folder(self.create_folder(parent_id, &name)?),
            C::RenameFolder { folder_id, name } => R::Folder(self.rename_folder(folder_id, &name)?),
            C::MoveFolder {
                folder_id,
                parent_id,
            } => R::Folder(self.move_folder(folder_id, parent_id)?),
            C::RemoveFolder { folder_id } => R::Updated {
                count: self.remove_folder(folder_id)?,
            },
            C::CheckMissing { folder_id } => R::JobStarted {
                job_id: self.check_missing(folder_id)?,
            },
            C::FindRelinkCandidates {
                variant_ids,
                search_dir,
                recursive,
            } => R::JobStarted {
                job_id: self.find_relink_candidates(variant_ids, &search_dir, recursive)?,
            },
            C::RelinkFiles { links } => R::Relinked(self.relink_files(&links)?),
            C::PlanRelocateFolder { folder_id, path } => {
                R::RelocatePlan(self.plan_relocate_folder(folder_id, &path)?)
            }
            C::RelocateFolder { folder_id, path } => {
                R::RelocatePlan(self.relocate_folder(folder_id, &path)?)
            }
        })
    }

    /// JSON のコマンドを実行し、JSON の結果を返す（案 A の Tauri のコマンドの入口の例）。
    ///
    /// 成功は `{"ok": <Response>}`、失敗は `{"error": <ErrorInfo>}`。
    pub fn execute_json(&self, command_json: &str) -> String {
        let result = serde_json::from_str::<Command>(command_json)
            .map_err(|e| ApiError::InvalidArgument(format!("コマンドを解釈できません: {e}")))
            .and_then(|c| self.execute(c));
        let value = match result {
            Ok(r) => serde_json::json!({ "ok": r }),
            Err(e) => serde_json::json!({ "error": e.info() }),
        };
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_have_plain_json_forms() {
        let c: Command =
            serde_json::from_str(r#"{"type":"set_rating","variant_ids":[1,2],"rating":3}"#)
                .unwrap();
        assert_eq!(
            c,
            Command::SetRating {
                variant_ids: vec![VariantId::new(1), VariantId::new(2)],
                rating: Rating::new(3).unwrap(),
            }
        );
        let c: Command = serde_json::from_str(r#"{"type":"search"}"#).unwrap();
        assert_eq!(
            c,
            Command::Search {
                filter: Box::default(),
                sort: SearchSort::default()
            }
        );
        let c: Command = serde_json::from_str(
            r#"{"type":"search","filter":{"rating_min":3,"kind":"video","text":"京都"},"sort":{"key":"capture_time","direction":"descending"}}"#,
        )
        .unwrap();
        let Command::Search { filter, .. } = c else {
            panic!()
        };
        assert_eq!(filter.rating_min, Rating::new(3));
        assert_eq!(filter.kind, Some(genzo_model::AssetKind::Video));
        let json = serde_json::to_value(Response::Updated { count: 2 }).unwrap();
        assert_eq!(json, serde_json::json!({"type": "updated", "count": 2}));
        let json = serde_json::to_value(Response::Search(SearchResult {
            generation: 3,
            count: 10,
            hidden: 0,
        }))
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type": "search", "generation": 3, "count": 10})
        );
        // 不正な評価は解釈の段階で弾く。
        assert!(
            serde_json::from_str::<Command>(
                r#"{"type":"set_rating","variant_ids":[1],"rating":6}"#
            )
            .is_err()
        );
    }

    /// カメラプロファイル・ルック（DEV-17）のコマンドと応答の JSON の形（ui/src/api/types.ts）。
    #[test]
    fn profile_commands_have_plain_json_forms() {
        let c: Command = serde_json::from_str(r#"{"type":"list_profiles"}"#).unwrap();
        assert_eq!(c, Command::ListProfiles { camera: None });
        let c: Command = serde_json::from_str(
            r#"{"type":"import_profiles","paths":["/x/a.dcp","/x/b.cube"],"replace":true}"#,
        )
        .unwrap();
        assert_eq!(
            c,
            Command::ImportProfiles {
                paths: vec![PathBuf::from("/x/a.dcp"), PathBuf::from("/x/b.cube")],
                replace: true,
            }
        );
        let c: Command =
            serde_json::from_str(r#"{"type":"remove_profile","kind":"look","id":"film"}"#).unwrap();
        assert_eq!(
            c,
            Command::RemoveProfile {
                kind: ProfileKind::Look,
                id: "film".into(),
            }
        );
        let json =
            serde_json::to_value(Response::ProfileImport(ProfileImportReport::default())).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type": "profile_import", "imported": [], "replaced": [], "repaired": [], "unchanged": [], "failed": []})
        );
        let json = serde_json::to_value(Response::Profiles { profiles: vec![] }).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type": "profiles", "profiles": []})
        );
    }

    /// 画面（ui/src/api/types.ts）が使う形: 絞り込みの候補・WB のスポイト・切り抜きの編集中の表示・
    /// 切り抜きの制限と、FrameInfo の追加の項目（古い形の JSON も読める）。
    #[test]
    fn develop_and_facet_commands_have_the_ui_forms() {
        use serde_json::json;
        let c: Command = serde_json::from_value(json!({"type": "facets"})).unwrap();
        assert_eq!(c, Command::Facets);
        let c: Command = serde_json::from_value(
            json!({"type": "white_balance_at", "variant_id": 1, "x": 0.5, "y": 0.25}),
        )
        .unwrap();
        assert_eq!(
            c,
            Command::WhiteBalanceAt {
                variant_id: VariantId::new(1),
                x: 0.5,
                y: 0.25,
                uncropped: None,
            }
        );
        // クリックした画像の表示（省略可）。省略したものは書き出さない（画面の型と同じ形）。
        let c: Command = serde_json::from_value(json!({
            "type": "white_balance_at", "variant_id": 1, "x": 0.5, "y": 0.25, "uncropped": true
        }))
        .unwrap();
        assert_eq!(
            c,
            Command::WhiteBalanceAt {
                variant_id: VariantId::new(1),
                x: 0.5,
                y: 0.25,
                uncropped: Some(true),
            }
        );
        assert_eq!(
            serde_json::to_value(Command::WhiteBalanceAt {
                variant_id: VariantId::new(1),
                x: 0.5,
                y: 0.25,
                uncropped: None,
            })
            .unwrap(),
            json!({"type": "white_balance_at", "variant_id": 1, "x": 0.5, "y": 0.25})
        );
        let c: Command =
            serde_json::from_value(json!({"type": "set_develop_view", "uncropped": true})).unwrap();
        assert_eq!(c, Command::SetDevelopView { uncropped: true });
        // 処理バージョンの更新（DATA-09）。引数はない。
        let c: Command =
            serde_json::from_value(json!({"type": "upgrade_process_version"})).unwrap();
        assert_eq!(c, Command::UpgradeProcessVersion);
        // まとめての更新（ライブラリの右クリックのメニュー）。
        let c: Command = serde_json::from_value(json!({
            "type": "upgrade_process_versions",
            "variant_ids": [3, 4]
        }))
        .unwrap();
        assert_eq!(
            c,
            Command::UpgradeProcessVersions {
                variant_ids: vec![VariantId::new(3), VariantId::new(4)],
            }
        );
        // 貼り付け: 処理バージョンの扱いは省略すると変えない（keep）。
        let c: Command = serde_json::from_value(json!({
            "type": "paste_settings",
            "settings": {},
            "targets": [1],
        }))
        .unwrap();
        let Command::PasteSettings {
            process_version, ..
        } = c
        else {
            panic!("{c:?}");
        };
        assert_eq!(process_version, PasteVersion::Keep);
        let c: Command = serde_json::from_value(json!({
            "type": "paste_settings",
            "settings": {},
            "targets": [1],
            "process_version": "upgrade",
        }))
        .unwrap();
        assert!(matches!(
            c,
            Command::PasteSettings {
                process_version: PasteVersion::Upgrade,
                ..
            }
        ));
        let r = serde_json::to_value(Response::Pasted(PasteReport {
            count: 3,
            upgraded: 1,
            needs_upgrade: 2,
            skipped_videos: 0,
        }))
        .unwrap();
        assert_eq!(
            r,
            json!({"type": "pasted", "count": 3, "upgraded": 1, "needs_upgrade": 2, "skipped_videos": 0})
        );
        let c: Command = serde_json::from_value(json!({
            "type": "constrain_crop",
            "variant_id": 1,
            "crop": {"left": 0, "top": 0, "right": 1, "bottom": 1},
            "angle_deg": 5.0
        }))
        .unwrap();
        assert_eq!(
            c,
            Command::ConstrainCrop {
                variant_id: VariantId::new(1),
                crop: CropRect::FULL,
                angle_deg: 5.0
            }
        );

        let facets = Response::Facets(Facets {
            cameras: vec![crate::types::FacetValue {
                value: "ILCE-7M4".to_owned(),
                count: 12,
            }],
            lenses: Vec::new(),
            codecs: Vec::new(),
        });
        assert_eq!(
            serde_json::to_value(&facets).unwrap(),
            json!({
                "type": "facets",
                "cameras": [{"value": "ILCE-7M4", "count": 12}],
                "lenses": [],
                "codecs": []
            })
        );
        let wb = Response::WhiteBalance(PickedWhiteBalance {
            temperature_k: 5321.0,
            tint: 4.5,
        });
        assert_eq!(
            serde_json::to_value(&wb).unwrap(),
            json!({"type": "white_balance", "temperature_k": 5321.0, "tint": 4.5})
        );
        let crop = Response::Crop {
            crop: CropRect::FULL,
        };
        assert_eq!(
            serde_json::to_value(&crop).unwrap(),
            json!({"type": "crop", "crop": {"left": 0.0, "top": 0.0, "right": 1.0, "bottom": 1.0}})
        );
        for r in [facets, wb, crop, Response::Ok] {
            let back: Response = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
            assert_eq!(back, r);
        }

        // FrameInfo: 追加の項目は省略できる（古い形）。
        let mut frame = json!({
            "variant_id": 1,
            "generation": 3,
            "quality": "final",
            "width": 120,
            "height": 80,
            "backend": "cpu",
            "histogram": crate::types::HistogramData::default(),
            "display_assumed_srgb": true,
            "warnings": []
        });
        let old: FrameInfo = serde_json::from_value(frame.clone()).unwrap();
        assert!(!old.uncropped);
        assert_eq!((old.output_width, old.output_height), (0, 0));
        frame["uncropped"] = json!(true);
        frame["output_width"] = json!(7008);
        frame["output_height"] = json!(4672);
        let new: FrameInfo = serde_json::from_value(frame.clone()).unwrap();
        assert!(new.uncropped);
        assert_eq!((new.output_width, new.output_height), (7008, 4672));
        assert_eq!(serde_json::to_value(&new).unwrap(), frame);
    }

    /// 画面（ui/src/api/types.ts）が使うスタック・仮想コピーの形（LIB-12・LIB-13）。
    #[test]
    fn stack_and_virtual_copy_commands_have_the_ui_forms() {
        use serde_json::json;
        let c: Command =
            serde_json::from_value(json!({"type": "create_stack", "variant_ids": [1, 2]})).unwrap();
        assert_eq!(
            c,
            Command::CreateStack {
                variant_ids: vec![VariantId::new(1), VariantId::new(2)],
                top: None,
            }
        );
        let c: Command = serde_json::from_value(
            json!({"type": "set_stacks_collapsed", "stack_ids": [4], "collapsed": true}),
        )
        .unwrap();
        assert_eq!(
            c,
            Command::SetStacksCollapsed {
                stack_ids: Some(vec![StackId::new(4)]),
                collapsed: true,
            }
        );
        let c: Command =
            serde_json::from_value(json!({"type": "auto_stack", "gap_seconds": 2})).unwrap();
        assert_eq!(
            c,
            Command::AutoStack {
                filter: Box::default(),
                gap_seconds: 2.0,
                dry_run: false,
            }
        );
        let r = Response::Stack(StackInfo {
            stack_id: StackId::new(4),
            top_variant_id: VariantId::new(2),
            collapsed: true,
            members: vec![VariantId::new(2), VariantId::new(1)],
        });
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"type": "stack", "stack_id": 4, "top_variant_id": 2, "collapsed": true, "members": [2, 1]})
        );
        let r = Response::MasterChanged {
            previous_master: VariantId::new(7),
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"type": "master_changed", "previous_master": 7})
        );
        // 検索の結果: 隠した写真がなければ hidden を書かない（前の形と同じ）。
        let r = Response::Search(SearchResult {
            generation: 1,
            count: 3,
            hidden: 2,
        });
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"type": "search", "generation": 1, "count": 3, "hidden": 2})
        );
        let back: SearchResult =
            serde_json::from_value(json!({"generation": 1, "count": 3})).unwrap();
        assert_eq!(back.hidden, 0);
    }

    /// 画面（ui/src/api/types.ts）が使う撮影日時の修正・タイムラインの形（LIB-16・ORG-01）。
    #[test]
    fn capture_time_and_timeline_commands_have_the_ui_forms() {
        use serde_json::json;
        let c: Command = serde_json::from_value(json!({
            "type": "edit_capture_times",
            "target": {"kind": "devices", "devices": [{"camera": null, "serial": null, "kind": "video"}]},
            "edit": {"kind": "set_zone", "offset_minutes": 540, "mode": "keep_local"}
        }))
        .unwrap();
        assert_eq!(
            c,
            Command::EditCaptureTimes {
                target: CaptureTarget::Devices {
                    devices: vec![CaptureDeviceKey {
                        camera: None,
                        serial: None,
                        kind: genzo_model::AssetKind::Video,
                    }],
                },
                edit: CaptureTimeEdit::SetZone {
                    offset_minutes: 540,
                    mode: genzo_model::ZoneMode::KeepLocal,
                },
            }
        );
        let c: Command = serde_json::from_value(json!({
            "type": "plan_capture_time_edit",
            "target": {"kind": "variants", "variant_ids": [5]},
            "edit": {"kind": "sync", "reference": 5, "correct": "2024-05-01T12:00:00"}
        }))
        .unwrap();
        assert!(matches!(
            c,
            Command::PlanCaptureTimeEdit {
                edit: CaptureTimeEdit::Sync {
                    offset_minutes: None,
                    ..
                },
                ..
            }
        ));
        let c: Command =
            serde_json::from_value(json!({"type": "timeline", "generation": 2})).unwrap();
        assert_eq!(
            c,
            Command::Timeline {
                generation: 2,
                gap_seconds: 0
            }
        );
        let r = Response::CaptureTimeUndone { undone: None };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"type": "capture_time_undone", "undone": null})
        );
        let r = Response::CaptureDevice(CaptureDeviceKey {
            camera: Some("DJI".to_owned()),
            serial: None,
            kind: genzo_model::AssetKind::Video,
        });
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"type": "capture_device", "camera": "DJI", "serial": null, "kind": "video"})
        );
    }

    /// 画面（ui/src/api/types.ts）が使う位置情報の後付けの形（LIB-21）。`location` の `null` は消す、省略は誤り。
    #[test]
    fn location_commands_have_the_ui_forms() {
        use serde_json::json;
        let tokyo = GpsCoord::new(35.681_236, 139.767_125).unwrap();
        let c: Command = serde_json::from_value(json!({
            "type": "edit_locations",
            "variant_ids": [1, 2],
            "location": {"lat": 35.681_236, "lon": 139.767_125}
        }))
        .unwrap();
        assert_eq!(
            c,
            Command::EditLocations {
                variant_ids: vec![VariantId::new(1), VariantId::new(2)],
                location: Some(tokyo),
            }
        );
        let c: Command = serde_json::from_value(json!({
            "type": "plan_location_edit", "variant_ids": [3], "location": null
        }))
        .unwrap();
        assert_eq!(
            c,
            Command::PlanLocationEdit {
                variant_ids: vec![VariantId::new(3)],
                location: None,
            }
        );
        // 位置の書き忘れを「消す」と読まない。
        for t in ["edit_locations", "plan_location_edit"] {
            let err = serde_json::from_value::<Command>(json!({"type": t, "variant_ids": [3]}))
                .unwrap_err();
            assert!(err.to_string().contains("location"), "{t}: {err}");
        }
        for (json, command) in [
            (json!({"type": "location_edits"}), Command::LocationEdits),
            (
                json!({"type": "undo_location_edit"}),
                Command::UndoLocationEdit,
            ),
        ] {
            assert_eq!(serde_json::from_value::<Command>(json).unwrap(), command);
        }
        let r = Response::LocationEditPlan(LocationEditPlan {
            photos: 2,
            changed: 1,
            unchanged: 1,
            with_camera_gps: 1,
            with_assigned: 0,
            skipped_videos: 1,
            missing: 0,
            edit_id: Some(4),
        });
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"type": "location_edit_plan", "photos": 2, "changed": 1, "unchanged": 1,
                   "with_camera_gps": 1, "with_assigned": 0, "skipped_videos": 1, "missing": 0,
                   "edit_id": 4})
        );
        let edit = LocationEditInfo {
            id: 4,
            kind: "set".to_owned(),
            location: Some(tokyo),
            asset_count: 1,
            created_at: chrono::DateTime::parse_from_rfc3339("2026-10-20T01:02:03Z")
                .unwrap()
                .to_utc(),
        };
        let r = Response::LocationEdits {
            edits: vec![edit.clone()],
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({"type": "location_edits", "edits": [{
                "id": 4, "kind": "set", "location": {"lat": 35.681_236, "lon": 139.767_125},
                "asset_count": 1, "created_at": "2026-10-20T01:02:03Z"
            }]})
        );
        let r = Response::LocationUndone {
            undone: Some(LocationUndo {
                edit: LocationEditInfo {
                    kind: "clear".to_owned(),
                    location: None,
                    ..edit
                },
                restored: 1,
                removed: 0,
            }),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["type"], "location_undone");
        assert_eq!(v["undone"]["edit"]["kind"], "clear");
        assert_eq!(v["undone"]["edit"]["location"], serde_json::Value::Null);
        assert_eq!(
            (
                v["undone"]["restored"].as_u64(),
                v["undone"]["removed"].as_u64()
            ),
            (Some(1), Some(0))
        );
        assert_eq!(
            serde_json::to_value(Response::LocationUndone { undone: None }).unwrap(),
            json!({"type": "location_undone", "undone": null})
        );
        // 詳細の位置の出どころ（LIB-21 より前の応答には gps_source・camera_gps が無い）。
        assert_eq!(
            serde_json::to_value(crate::types::GpsSource::Assigned).unwrap(),
            json!("assigned")
        );
        assert_eq!(
            serde_json::to_value(crate::types::GpsSource::Camera).unwrap(),
            json!("camera")
        );
    }

    #[test]
    fn every_command_round_trips() {
        let commands = vec![
            Command::ImportFolder {
                path: "/p".into(),
                recursive: true,
                options: ImportOptions::default(),
            },
            Command::ImportFolder {
                path: "/p".into(),
                recursive: false,
                options: ImportOptions {
                    skip_duplicates: Some(false),
                    separate_raw_jpeg: Some(true),
                    ..ImportOptions::default()
                },
            },
            Command::ComputeContentHashes {
                include_network: true,
            },
            Command::ContentHashStatus,
            Command::DuplicateGroups { limit: 5 },
            Command::IdentifyFile {
                path: "/p/a.ARW".into(),
            },
            Command::ImportFolder {
                path: "/sd".into(),
                recursive: true,
                options: ImportOptions {
                    copy: Some(CopySettings {
                        destination: "/photos".into(),
                        folder_rule: crate::FolderRule::YearMonthDay,
                        rename: Some(crate::RenameRule::default()),
                    }),
                    metadata: crate::ImportMetadata {
                        keywords: vec![vec!["場所".to_owned(), "京都".to_owned()]],
                        fields: [(genzo_model::MetadataField::Creator, "岡本".to_owned())]
                            .into_iter()
                            .collect(),
                    },
                    dry_run: true,
                    ..ImportOptions::default()
                },
            },
            Command::ImportNamePreview {
                copy: CopySettings {
                    destination: "/photos".into(),
                    folder_rule: crate::FolderRule::Single,
                    rename: None,
                },
            },
            Command::CaptureDevices,
            Command::CaptureDeviceOf {
                variant_id: VariantId::new(3),
            },
            Command::PlanCaptureTimeEdit {
                target: CaptureTarget::Variants {
                    variant_ids: vec![VariantId::new(1)],
                },
                edit: CaptureTimeEdit::Shift { seconds: -60 },
            },
            Command::EditCaptureTimes {
                target: CaptureTarget::Devices {
                    devices: vec![CaptureDeviceKey {
                        camera: Some("SONY ILCE-7M4".to_owned()),
                        serial: None,
                        kind: genzo_model::AssetKind::Photo,
                    }],
                },
                edit: CaptureTimeEdit::Sync {
                    reference: VariantId::new(2),
                    correct: "2024-05-01T12:00:00".to_owned(),
                    offset_minutes: Some(540),
                },
            },
            Command::CaptureTimeEdits,
            Command::UndoCaptureTimeEdit,
            Command::PlanLocationEdit {
                variant_ids: vec![VariantId::new(1), VariantId::new(2)],
                location: GpsCoord::new(-33.856_784, 151.215_297),
            },
            Command::EditLocations {
                variant_ids: vec![VariantId::new(1)],
                location: None,
            },
            Command::LocationEdits,
            Command::UndoLocationEdit,
            Command::Volumes { refresh: true },
            Command::PlanMoveFiles {
                variant_ids: vec![VariantId::new(1)],
                dest_folder: FolderId::new(4),
            },
            Command::MoveFiles {
                variant_ids: vec![VariantId::new(1), VariantId::new(2)],
                dest_folder: FolderId::new(4),
            },
            Command::PlanRenameFiles {
                variant_ids: vec![VariantId::new(1)],
                spec: RenameSpec::Template {
                    rule: crate::RenameRule::default(),
                },
            },
            Command::RenameFiles {
                variant_ids: vec![VariantId::new(1)],
                spec: RenameSpec::Name {
                    name: "京都の朝".to_owned(),
                },
            },
            Command::CreateFolder {
                parent_id: FolderId::new(2),
                name: "選別済み".to_owned(),
            },
            Command::RenameFolder {
                folder_id: FolderId::new(3),
                name: "2024 京都".to_owned(),
            },
            Command::MoveFolder {
                folder_id: FolderId::new(3),
                parent_id: FolderId::new(2),
            },
            Command::RemoveFolder {
                folder_id: FolderId::new(3),
            },
            Command::CheckMissing { folder_id: None },
            Command::FindRelinkCandidates {
                variant_ids: Some(vec![VariantId::new(5)]),
                search_dir: "/mnt/新しい場所".into(),
                recursive: true,
            },
            Command::RelinkFiles {
                links: vec![RelinkRequest {
                    file_id: genzo_model::FileId::new(7),
                    path: "/mnt/新しい場所/A.ARW".into(),
                    accept_changed: false,
                }],
            },
            Command::PlanRelocateFolder {
                folder_id: FolderId::new(3),
                path: "/media/me/SD/DCIM".into(),
            },
            Command::RelocateFolder {
                folder_id: FolderId::new(3),
                path: "/media/me/SD/DCIM".into(),
            },
            Command::Timeline {
                generation: 4,
                gap_seconds: 7_200,
            },
            Command::Range {
                generation: 1,
                start: 0,
                len: 10,
            },
            Command::SetSettings {
                settings: Box::default(),
                dragging: true,
            },
            Command::PasteSettings {
                settings: Box::default(),
                targets: vec![VariantId::new(1)],
                groups: SettingGroups::default(),
                process_version: PasteVersion::Upgrade,
            },
            Command::UpgradeProcessVersions {
                variant_ids: vec![VariantId::new(1), VariantId::new(2)],
            },
            Command::Export {
                variant_ids: vec![VariantId::new(1)],
                settings: ExportSettings::default(),
                dest_dir: "/out".into(),
                file_naming: None,
            },
            Command::Export {
                variant_ids: vec![VariantId::new(1)],
                settings: ExportSettings::default(),
                dest_dir: "/out".into(),
                file_naming: Some(crate::RenameRule::default()),
            },
            Command::Snapshots {
                variant_id: VariantId::new(1),
            },
            Command::CreateSnapshot {
                variant_id: VariantId::new(1),
                name: "明るめ".to_owned(),
                source: SnapshotSource::History {
                    entry_id: HistoryEntryId::new(4),
                },
            },
            Command::RenameSnapshot {
                snapshot_id: SnapshotId::new(2),
                name: "白黒".to_owned(),
            },
            Command::DeleteSnapshot {
                snapshot_id: SnapshotId::new(2),
            },
            Command::ApplySnapshot {
                snapshot_id: SnapshotId::new(2),
            },
            Command::Presets {
                kind: PresetKind::Develop,
            },
            Command::CreateDevelopPreset {
                name: "フィルム".to_owned(),
                folder: "ルック".to_owned(),
                groups: SettingGroups::ALL,
                variant_id: VariantId::new(1),
            },
            Command::ApplyDevelopPreset {
                preset_id: "0123456789abcdef".to_owned(),
                targets: vec![VariantId::new(1), VariantId::new(2)],
                process_version: PasteVersion::Keep,
            },
            Command::CreateExportPreset {
                name: "Web".to_owned(),
                folder: String::new(),
                settings: ExportPresetSettings::default(),
            },
            Command::RenamePresetFolder {
                kind: PresetKind::Export,
                folder: "旧".to_owned(),
                name: "新".to_owned(),
            },
            Command::ImportPresets {
                paths: vec!["/x/a.json".into()],
                kind: None,
            },
            Command::ExportPresetFiles {
                kind: PresetKind::Develop,
                preset_ids: vec!["0123456789abcdef".to_owned()],
                dest_dir: "/out".into(),
            },
            Command::ExportNamePreview {
                file_naming: None,
                format: ExportFormat::Png8,
            },
            Command::WatermarkPreview {
                watermark: Watermark::new(genzo_model::WatermarkContent::Image {
                    path: "/logo.png".into(),
                }),
                width: Some(300),
                height: None,
            },
            Command::SystemFontDirs,
            Command::Export {
                variant_ids: vec![VariantId::new(1), VariantId::new(2)],
                settings: ExportSettings {
                    output_sharpening: Some(genzo_model::OutputSharpening::default()),
                    watermark: Some(Watermark::new(genzo_model::WatermarkContent::Text {
                        text: "© 2026".into(),
                        font: "/fonts/a.ttf".into(),
                        font_index: 0,
                        color: genzo_model::RgbColor::WHITE,
                    })),
                    videos: genzo_model::VideoExport::Skip,
                    resolution_ppi: 300,
                    ..ExportSettings::default()
                },
                dest_dir: "/out".into(),
                file_naming: None,
            },
            Command::PlanDelete {
                kind: DeleteKind::Trash,
                variant_ids: vec![VariantId::new(2)],
            },
            Command::UpdateSettings {
                update: SettingsUpdate {
                    default_utc_offset_minutes: Some(540),
                    ..Default::default()
                },
            },
            Command::Folders,
            Command::Capabilities,
            Command::CatalogStats,
            Command::CatalogSchema,
            Command::Facets,
            Command::WhiteBalanceAt {
                variant_id: VariantId::new(1),
                x: 0.5,
                y: 0.25,
                uncropped: None,
            },
            Command::WhiteBalanceAt {
                variant_id: VariantId::new(1),
                x: 0.0,
                y: 1.0,
                uncropped: Some(false),
            },
            Command::SetDevelopView { uncropped: true },
            Command::UpgradeProcessVersion,
            Command::ViewOutputSize {
                pane: 1,
                variant_id: VariantId::new(4),
            },
            Command::RenderViewRegion {
                pane: 0,
                request: RegionRequest {
                    variant_id: VariantId::new(4),
                    space: crate::RegionSpace::Output,
                    x: 10.0,
                    y: 20.0,
                    width: 300.0,
                    height: 200.0,
                    scale: 1.0,
                },
            },
            Command::CancelViewRegions,
            Command::ReleaseView,
            Command::VideoPlayback {
                variant_id: VariantId::new(9),
            },
            Command::CreateVideoProxy {
                variant_id: VariantId::new(9),
            },
            Command::CompositeCheck {
                variant_ids: vec![VariantId::new(1), VariantId::new(2)],
                settings: CompositeSettings::default(),
                dest_dir: None,
            },
            Command::Composite {
                variant_ids: vec![VariantId::new(3), VariantId::new(4)],
                settings: CompositeSettings {
                    mode: genzo_model::CompositeMode::LighterColor,
                    format: ExportFormat::Jpeg { quality: 92 },
                    color_space: genzo_model::OutputColorSpace::DisplayP3,
                    remove_gps: true,
                    stack_with_sources: false,
                },
                dest_dir: Some("/out".into()),
            },
            Command::VideoProxyCache,
            Command::ClearVideoProxies,
            Command::CreateStack {
                variant_ids: vec![VariantId::new(1), VariantId::new(2)],
                top: Some(VariantId::new(2)),
            },
            Command::SetStacksCollapsed {
                stack_ids: None,
                collapsed: false,
            },
            Command::AutoStack {
                filter: Box::default(),
                gap_seconds: 1.5,
                dry_run: true,
            },
            Command::RenameVirtualCopy {
                variant_id: VariantId::new(3),
                name: "白黒".to_owned(),
            },
            Command::ConstrainCrop {
                variant_id: VariantId::new(1),
                crop: CropRect {
                    left: 0.1,
                    top: 0.2,
                    right: 0.9,
                    bottom: 0.8,
                },
                angle_deg: 5.0,
            },
        ];
        for c in commands {
            let json = serde_json::to_string(&c).unwrap();
            let back: Command = serde_json::from_str(&json).unwrap();
            assert_eq!(back, c, "{json}");
        }
    }
}
