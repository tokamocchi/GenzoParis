// コア API（genzo-api の Command）の呼び出し。
//
// すべて genzo-app の `core_execute` を通す（JSON の Command → Response）。応答の `type` を確かめて、
// 型付きの値を返す。

import { bridge } from "./bridge";
import { ApiError, toApiError } from "./errors";
import type {
  AutoStackReport,
  BackupEntry,
  BeforeInfo,
  BeforeSource,
  CatalogSchema,
  CatalogStats,
  Capabilities,
  CaptureDevice,
  CaptureDeviceKey,
  CaptureTarget,
  CaptureTimeEdit,
  CaptureTimeEditInfo,
  CaptureTimePlan,
  CaptureTimeUndo,
  CollectionId,
  CollectionInfo,
  CollectionKind,
  ColorLabel,
  Command,
  ContentHashStatus,
  CopySettings,
  CoreSettings,
  CropRect,
  DeleteKind,
  DeletePlan,
  DeleteReport,
  DevelopSettings,
  DevelopState,
  DisplayInfo,
  DuplicateGroupList,
  ExportSettings,
  Facets,
  FileCheckReport,
  FileIdentity,
  Flag,
  FolderId,
  FolderInfo,
  FrameInfo,
  HistoryEntryId,
  HistoryItem,
  ImportOptions,
  IndexOf,
  IptcMetadata,
  JobInfo,
  KeywordHit,
  KeywordId,
  KeywordInfo,
  KeywordUsage,
  MetadataSummary,
  NamePreview,
  RangeResult,
  Rating,
  RegionInfo,
  RegionRequest,
  Response,
  RestorePlan,
  SearchFilter,
  SearchResult,
  SearchSort,
  SettingGroups,
  SettingsUpdate,
  ShownVariants,
  SmartRule,
  StackId,
  StackInfo,
  StartupReport,
  Timeline,
  VariantDetails,
  VariantId,
  VariantKeywords,
  VolumeInfo,
  FileMovePlan,
  FileMoveReport,
  FileRenamePlan,
  RelinkReport,
  RelinkRequest,
  RelocatePlan,
  RenameSpec,
} from "./types";

type ResponseOf<T extends Response["type"]> = Extract<Response, { type: T }>;

/** コマンドを実行する（応答の種類を確かめる）。 */
export async function execute<T extends Response["type"]>(
  command: Command,
  expect: T,
): Promise<ResponseOf<T>> {
  let r: Response;
  try {
    r = await bridge().invoke<Response>("core_execute", { command });
  } catch (e) {
    throw toApiError(e);
  }
  if (r.type !== expect) {
    throw new ApiError({
      kind: "internal",
      message: `コアの応答の種類が違います（${command.type}: ${r.type}。期待は ${expect}）`,
      user_actionable: false,
      retryable: false,
      hint: null,
    });
  }
  return r as ResponseOf<T>;
}

function strip<T extends { type: string }>(r: T): Omit<T, "type"> {
  const { type: _type, ...rest } = r;
  return rest;
}

// --- ジョブ ---
export const importFolder = async (path: string, recursive = true, options?: ImportOptions) =>
  (await execute(options ? { type: "import_folder", path, recursive, options } : { type: "import_folder", path, recursive }, "job_started"))
    .job_id;
export const job = async (job_id: number): Promise<JobInfo> =>
  strip(await execute({ type: "job", job_id }, "job"));
export const cancelJob = async (job_id: number) => {
  await execute({ type: "cancel_job", job_id }, "ok");
};

// --- 検索・選別 ---
export const search = async (filter: SearchFilter, sort: SearchSort): Promise<SearchResult> =>
  strip(await execute({ type: "search", filter, sort }, "search"));
export const range = async (generation: number, start: number, len: number): Promise<RangeResult> =>
  strip(await execute({ type: "range", generation, start, len }, "range"));
export const indexOf = async (variant_id: VariantId): Promise<IndexOf> =>
  strip(await execute({ type: "index_of", variant_id }, "index_of"));
/** 写真のうち、表示中の検索結果に出ているもの（選択を表示している写真だけにする。LIB-12）。 */
export const shownVariants = async (variant_ids: VariantId[]): Promise<ShownVariants> =>
  strip(await execute({ type: "shown_variants", variant_ids }, "shown_variants"));
export const setRating = async (variant_ids: VariantId[], rating: Rating) =>
  (await execute({ type: "set_rating", variant_ids, rating }, "updated")).count;
export const setFlag = async (variant_ids: VariantId[], flag: Flag) =>
  (await execute({ type: "set_flag", variant_ids, flag }, "updated")).count;
export const setColorLabel = async (variant_ids: VariantId[], label: ColorLabel | null) =>
  (await execute({ type: "set_color_label", variant_ids, label }, "updated")).count;
export const setCaption = async (variant_ids: VariantId[], caption: string | null) =>
  (await execute({ type: "set_caption", variant_ids, caption }, "updated")).count;
/** 説明のメタデータ（LIB-15）を一括で変える（並べた項目だけ。null・空で削除）。変えた元の写真の数を返す。 */
export const setMetadata = async (variant_ids: VariantId[], changes: IptcMetadata) =>
  (await execute({ type: "set_metadata", variant_ids, changes }, "updated")).count;
/** 複数の写真の説明のメタデータのまとめ（項目ごとに共通の値か「複数の値」か）。 */
export const metadataSummary = async (variant_ids: VariantId[]): Promise<MetadataSummary> =>
  strip(await execute({ type: "metadata_summary", variant_ids }, "metadata"));
export const variantDetails = async (variant_id: VariantId): Promise<VariantDetails> =>
  strip(await execute({ type: "variant_details", variant_id }, "details"));
export const folders = async (): Promise<FolderInfo[]> =>
  (await execute({ type: "folders" }, "folders")).folders;
export const facets = async (): Promise<Facets> => strip(await execute({ type: "facets" }, "facets"));
export const checkFiles = async (variant_ids: VariantId[]): Promise<FileCheckReport> =>
  strip(await execute({ type: "check_files", variant_ids }, "file_check"));

/** 条件に合う写真の数（表示中の検索結果は変えない。スマートコレクションの条件の編集中の件数など）。 */
export const count = async (filter: SearchFilter) => (await execute({ type: "count", filter }, "count")).count;

// --- キーワード（LIB-09） ---
export const keywordTree = async (): Promise<KeywordInfo[]> =>
  (await execute({ type: "keyword_tree" }, "keywords")).keywords;
/** 名前でキーワードを探す（部分一致。前方一致のものが先。入力欄の補完）。 */
export const findKeywords = async (query: string, limit = 20): Promise<KeywordHit[]> =>
  (await execute({ type: "find_keywords", query, limit }, "keyword_hits")).hits;
/** parent の下に階層のパスでキーワードを確保する（あれば使い、なければ作る）。 */
export const createKeyword = async (parent_id: KeywordId | null, path: string[]) =>
  (await execute({ type: "create_keyword", parent_id, path }, "keyword_created")).keyword_id;
export const renameKeyword = async (keyword_id: KeywordId, name: string) => {
  await execute({ type: "rename_keyword", keyword_id, name }, "ok");
};
export const moveKeyword = async (keyword_id: KeywordId, parent_id: KeywordId | null) => {
  await execute({ type: "move_keyword", keyword_id, parent_id }, "ok");
};
/** キーワードを削除する（子のキーワードと付与も）。削除したキーワードの数を返す。 */
export const deleteKeyword = async (keyword_id: KeywordId) =>
  (await execute({ type: "delete_keyword", keyword_id }, "updated")).count;
export const setKeywordExport = async (keyword_id: KeywordId, include: boolean) => {
  await execute({ type: "set_keyword_export", keyword_id, include }, "ok");
};
export const keywordUsage = async (keyword_id: KeywordId): Promise<KeywordUsage> =>
  strip(await execute({ type: "keyword_usage", keyword_id }, "keyword_usage"));
/** 写真にキーワードを付ける（既存のもの・階層のパスで作るもの）。新しく付けた組の数を返す。 */
export const addKeywords = async (variant_ids: VariantId[], keyword_ids: KeywordId[], paths: string[][] = []) =>
  (await execute({ type: "add_keywords", variant_ids, keyword_ids, paths }, "updated")).count;
export const removeKeywords = async (variant_ids: VariantId[], keyword_ids: KeywordId[]) =>
  (await execute({ type: "remove_keywords", variant_ids, keyword_ids }, "updated")).count;
/** 写真に付いているキーワードと、それぞれが付いている写真の数。 */
export const variantKeywords = async (variant_ids: VariantId[]): Promise<VariantKeywords> =>
  strip(await execute({ type: "variant_keywords", variant_ids }, "variant_keywords"));

// --- コレクション（LIB-10・LIB-11） ---
export const collections = async (with_counts = false): Promise<CollectionInfo[]> =>
  (await execute({ type: "collections", with_counts }, "collections")).collections;
export const createCollection = async (
  parent_id: CollectionId | null,
  name: string,
  kind: CollectionKind,
  rule: SmartRule | null = null,
  variant_ids: VariantId[] = [],
) =>
  (await execute({ type: "create_collection", parent_id, name, kind, rule, variant_ids }, "collection_created"))
    .collection_id;
export const renameCollection = async (collection_id: CollectionId, name: string) => {
  await execute({ type: "rename_collection", collection_id, name }, "ok");
};
export const moveCollection = async (collection_id: CollectionId, parent_id: CollectionId | null) => {
  await execute({ type: "move_collection", collection_id, parent_id }, "ok");
};
/** コレクションを削除する（セットなら中のものも。写真は消えない）。削除した数を返す。 */
export const deleteCollection = async (collection_id: CollectionId) =>
  (await execute({ type: "delete_collection", collection_id }, "updated")).count;
export const setSmartRule = async (collection_id: CollectionId, rule: SmartRule) => {
  await execute({ type: "set_smart_rule", collection_id, rule }, "ok");
};
export const addToCollection = async (collection_id: CollectionId, variant_ids: VariantId[]) =>
  (await execute({ type: "add_to_collection", collection_id, variant_ids }, "updated")).count;
export const removeFromCollection = async (collection_id: CollectionId, variant_ids: VariantId[]) =>
  (await execute({ type: "remove_from_collection", collection_id, variant_ids }, "updated")).count;
export const variantCollections = async (variant_id: VariantId): Promise<CollectionId[]> =>
  (await execute({ type: "variant_collections", variant_id }, "variant_collections")).collection_ids;

// --- スタック（LIB-12） ---
/** 写真をまとめて 1 つのスタックにする（既にスタックに入っている写真があれば、そのスタックもまとめる）。 */
export const createStack = async (variant_ids: VariantId[], top: VariantId | null = null): Promise<StackInfo> =>
  strip(await execute({ type: "create_stack", variant_ids, top }, "stack"));
/** スタックに写真を足す（ほかのスタックの写真は移す）。新しく足した数を返す。 */
export const addToStack = async (stack_id: StackId, variant_ids: VariantId[]) =>
  (await execute({ type: "add_to_stack", stack_id, variant_ids }, "updated")).count;
/** 写真をスタックから外す（1 枚以下になったスタックは消える）。外した数を返す。 */
export const removeFromStack = async (variant_ids: VariantId[]) =>
  (await execute({ type: "remove_from_stack", variant_ids }, "updated")).count;
/** 写真が入っているスタックを解除する（写真は消えない）。解除したスタックの数を返す。 */
export const unstack = async (variant_ids: VariantId[]) =>
  (await execute({ type: "unstack", variant_ids }, "updated")).count;
/** 写真をそのスタックの一番上にする。 */
export const setStackTop = async (variant_id: VariantId): Promise<StackInfo> =>
  strip(await execute({ type: "set_stack_top", variant_id }, "stack"));
/** スタックの折りたたみの状態を変える（stack_ids が null ならすべて）。状態の変わったスタックの数を返す。 */
export const setStacksCollapsed = async (stack_ids: StackId[] | null, collapsed: boolean) =>
  (await execute({ type: "set_stacks_collapsed", stack_ids, collapsed }, "updated")).count;
/** 写真が入っているスタック。 */
export const variantStacks = async (variant_ids: VariantId[]): Promise<StackInfo[]> =>
  (await execute({ type: "variant_stacks", variant_ids }, "stacks")).stacks;
/** 撮影時刻の間隔で自動的にスタックを作る（dry_run なら数えるだけ）。 */
export const autoStack = async (filter: SearchFilter, gap_seconds: number, dry_run: boolean): Promise<AutoStackReport> =>
  strip(await execute({ type: "auto_stack", filter, gap_seconds, dry_run }, "auto_stack"));

// --- 仮想コピー（LIB-13） ---
export const renameVirtualCopy = async (variant_id: VariantId, name: string) => {
  await execute({ type: "rename_virtual_copy", variant_id, name }, "ok");
};
/** 仮想コピーをマスターにする（前のマスターは仮想コピーになる）。前のマスターを返す。 */
export const setAsMaster = async (variant_id: VariantId) =>
  (await execute({ type: "set_as_master", variant_id }, "master_changed")).previous_master;

// --- サムネイル・プレビュー ---
export const requestPreview = async (variant_id: VariantId) =>
  (await execute({ type: "request_preview", variant_id }, "path")).path;
export const regeneratePreviews = async (variant_ids: VariantId[]) =>
  (await execute({ type: "regenerate_previews", variant_ids }, "job_started")).job_id;

// --- 現像 ---
export const openDevelop = async (variant_id: VariantId): Promise<DevelopState> =>
  strip(await execute({ type: "open_develop", variant_id }, "develop"));
export const developState = async (): Promise<DevelopState | null> =>
  (await execute({ type: "develop_state" }, "develop_or_none")).state;
export const setSettings = async (settings: DevelopSettings, dragging: boolean) =>
  (await execute({ type: "set_settings", settings, dragging }, "generation")).generation;
export const endDrag = async (): Promise<DevelopState> =>
  strip(await execute({ type: "end_drag" }, "develop"));
export const renderDevelop = async (): Promise<FrameInfo> =>
  strip(await execute({ type: "render_develop" }, "frame"));
export const renderRegion = async (request: RegionRequest): Promise<RegionInfo> =>
  strip(await execute({ type: "render_region", request }, "region"));
/** ビフォー / アフターの表示のビフォーを設定する（DEV-31。null でやめる）。 */
export const setBefore = async (source: BeforeSource | null): Promise<BeforeInfo | null> =>
  (await execute({ type: "set_before", source }, "before")).before;
/** ビフォーの表示範囲を描く（画素は before_region_frame）。 */
export const renderBeforeRegion = async (request: RegionRequest): Promise<RegionInfo> =>
  strip(await execute({ type: "render_before_region", request }, "region"));
export const cancelRegion = async () => {
  await execute({ type: "cancel_region" }, "ok");
};
export const flushDevelop = async () => (await execute({ type: "flush_develop" }, "saved")).saved;
export const closeDevelop = async () => {
  await execute({ type: "close_develop" }, "ok");
};
export const undo = async (variant_id: VariantId) =>
  (await execute({ type: "undo", variant_id }, "settings_or_none")).settings;
export const redo = async (variant_id: VariantId) =>
  (await execute({ type: "redo", variant_id }, "settings_or_none")).settings;
/** 履歴の項目の位置へ 1 回で移る（その項目がない・すでにその位置なら null）。 */
export const goToHistory = async (variant_id: VariantId, entry_id: HistoryEntryId) =>
  (await execute({ type: "go_to_history", variant_id, entry_id }, "settings_or_none")).settings;
export const history = async (variant_id: VariantId): Promise<HistoryItem[]> =>
  (await execute({ type: "history", variant_id }, "history")).entries;
export const copySettings = async (variant_id: VariantId): Promise<DevelopSettings> =>
  (await execute({ type: "copy_settings", variant_id }, "settings")).settings;
export const pasteSettings = async (
  settings: DevelopSettings,
  targets: VariantId[],
  groups: SettingGroups,
) => (await execute({ type: "paste_settings", settings, targets, groups }, "updated")).count;
export const createVirtualCopy = async (variant_id: VariantId, name: string | null = null) =>
  (await execute({ type: "create_virtual_copy", variant_id, name }, "variant_created")).variant_id;
/**
 * 表示中のプレビューの x・y（0〜1）の点を中立にする WB（適用はしない。DEV-03 のスポイト）。
 *
 * `uncropped` には、クリックした画像の FrameInfo.uncropped を渡す（切り抜く前の全体の表示なら、その全体に
 * 対する割合として解釈する。回転で画像の外になった四隅を指すと invalid_input）。
 */
export const whiteBalanceAt = async (variant_id: VariantId, x: number, y: number, uncropped?: boolean) => {
  const r = await execute(
    { type: "white_balance_at", variant_id, x, y, ...(uncropped === undefined ? {} : { uncropped }) },
    "white_balance",
  );
  return { temperature_k: r.temperature_k, tint: r.tint };
};
/** 切り抜きの編集中の表示（`uncropped` なら切り抜く前の全体を描く）。 */
export const setDevelopView = async (uncropped: boolean) => {
  await execute({ type: "set_develop_view", uncropped }, "ok");
};
/** 角度に合わせて、切り抜きを画像の内側に収める（コアが描画で使うのと同じ制限）。 */
export const constrainCrop = async (
  variant_id: VariantId,
  crop: CropRect,
  angle_deg: number,
): Promise<CropRect> =>
  (await execute({ type: "constrain_crop", variant_id, crop, angle_deg }, "crop")).crop;

// --- 書き出し・削除 ---
export const exportVariants = async (
  variant_ids: VariantId[],
  settings: ExportSettings,
  dest_dir: string,
) => (await execute({ type: "export", variant_ids, settings, dest_dir }, "job_started")).job_id;
export const planDelete = async (kind: DeleteKind, variant_ids: VariantId[]): Promise<DeletePlan> =>
  strip(await execute({ type: "plan_delete", kind, variant_ids }, "delete_plan"));
export const executeDelete = async (plan_id: number): Promise<DeleteReport> =>
  strip(await execute({ type: "execute_delete", plan_id }, "deleted"));

// --- 設定・保守 ---
export const getSettings = async (): Promise<CoreSettings> =>
  strip(await execute({ type: "get_settings" }, "core_settings"));
export const updateSettings = async (update: SettingsUpdate): Promise<CoreSettings> =>
  strip(await execute({ type: "update_settings", update }, "core_settings"));
export const backupNow = async (): Promise<BackupEntry> =>
  strip(await execute({ type: "backup_now" }, "backup"));
export const listBackups = async (): Promise<BackupEntry[]> =>
  (await execute({ type: "list_backups" }, "backups")).backups;
export const checkIntegrity = async () =>
  (await execute({ type: "check_integrity" }, "job_started")).job_id;
export const collectCacheGarbage = async () =>
  (await execute({ type: "collect_cache_garbage" }, "job_started")).job_id;
export const prepareRestore = async (backup: string): Promise<RestorePlan> =>
  strip(await execute({ type: "prepare_restore", backup }, "restore"));
export const startupReport = async (): Promise<StartupReport> =>
  strip(await execute({ type: "startup_report" }, "startup"));
export const catalogStats = async (): Promise<CatalogStats> =>
  strip(await execute({ type: "catalog_stats" }, "catalog_stats"));
/** カタログの構造（表・列・索引・ビュー・トリガー。ORG-07）。 */
export const catalogSchema = async (): Promise<CatalogSchema> =>
  strip(await execute({ type: "catalog_schema" }, "catalog_schema"));
export const capabilities = async (): Promise<Capabilities> =>
  strip(await execute({ type: "capabilities" }, "capabilities"));

// --- 内容による同一性（ORG-03・IMP-04） ---
/** 全体のハッシュのないファイルの内容のハッシュを計算する（既に計算中なら、そのジョブ）。 */
export const computeContentHashes = async (include_network = false) =>
  (await execute({ type: "compute_content_hashes", include_network }, "job_started")).job_id;
export const contentHashStatus = async (): Promise<ContentHashStatus> =>
  strip(await execute({ type: "content_hash_status" }, "content_hash_status"));
export const duplicateGroups = async (limit = 100): Promise<DuplicateGroupList> =>
  strip(await execute({ type: "duplicate_groups", limit }, "duplicate_groups"));
export const identifyFile = async (path: string): Promise<FileIdentity> =>
  strip(await execute({ type: "identify_file", path }, "file_identity"));

// ---------------------------------------------------------------------------
// コピー取り込みの名前（IMP-03）
// ---------------------------------------------------------------------------

/** コピー取り込みの設定の、名前の例（見本の写真のコピー先。設定の誤りは error）。 */
export const importNamePreview = async (copy: CopySettings): Promise<NamePreview> =>
  strip(await execute({ type: "import_name_preview", copy }, "name_preview"));

// ---------------------------------------------------------------------------
// 撮影日時の一括修正（LIB-16）・タイムライン（ORG-01）
// ---------------------------------------------------------------------------

/** 機材の一覧（カメラの表示名・シリアル番号・写真か動画か）。 */
export const captureDevices = async (): Promise<CaptureDevice[]> =>
  (await execute({ type: "capture_devices" }, "capture_devices")).devices;
/** 写真の機材。 */
export const captureDeviceOf = async (variant_id: VariantId): Promise<CaptureDeviceKey> =>
  strip(await execute({ type: "capture_device_of", variant_id }, "capture_device"));
/** 撮影日時の修正の見込み（何も変えない）。 */
export const planCaptureTimeEdit = async (target: CaptureTarget, edit: CaptureTimeEdit): Promise<CaptureTimePlan> =>
  strip(await execute({ type: "plan_capture_time_edit", target, edit }, "capture_time_plan"));
/** 撮影日時を修正する（履歴に残す）。 */
export const editCaptureTimes = async (target: CaptureTarget, edit: CaptureTimeEdit): Promise<CaptureTimePlan> =>
  strip(await execute({ type: "edit_capture_times", target, edit }, "capture_time_plan"));
/** 撮影日時の修正の履歴（新しい順）。 */
export const captureTimeEdits = async (): Promise<CaptureTimeEditInfo[]> =>
  (await execute({ type: "capture_time_edits" }, "capture_time_edits")).edits;
/** 最後の撮影日時の修正を取り消す（履歴がなければ null）。 */
export const undoCaptureTimeEdit = async (): Promise<CaptureTimeUndo | null> =>
  (await execute({ type: "undo_capture_time_edit" }, "capture_time_undone")).undone;
/** 表示中の検索結果のタイムライン（撮影日時の順のときだけ。古い世代なら stale）。 */
export const timeline = async (generation: number, gap_seconds: number): Promise<Timeline> =>
  strip(await execute({ type: "timeline", generation, gap_seconds }, "timeline"));

// ---------------------------------------------------------------------------
// ボリューム（FILE-03）・アプリの中でのファイル操作（FILE-02）・見つからない写真と再リンク（FILE-04）
// ---------------------------------------------------------------------------

/** ボリュームの一覧（refresh なら、つながりを確かめ直す）。 */
export const volumes = async (refresh = false): Promise<VolumeInfo[]> =>
  (await execute({ type: "volumes", refresh }, "volumes")).volumes;
/** 写真の移動の予定（何も変えない）。 */
export const planMoveFiles = async (variant_ids: VariantId[], dest_folder: FolderId): Promise<FileMovePlan> =>
  strip(await execute({ type: "plan_move_files", variant_ids, dest_folder }, "move_plan"));
/** 写真を移動する（ジョブ）。 */
export const moveFiles = async (variant_ids: VariantId[], dest_folder: FolderId): Promise<number> =>
  (await execute({ type: "move_files", variant_ids, dest_folder }, "job_started")).job_id;
/** 写真の名前の変更の予定（何も変えない）。 */
export const planRenameFiles = async (variant_ids: VariantId[], spec: RenameSpec): Promise<FileRenamePlan> =>
  strip(await execute({ type: "plan_rename_files", variant_ids, spec }, "rename_plan"));
/** 写真の名前を変える。 */
export const renameFiles = async (variant_ids: VariantId[], spec: RenameSpec): Promise<FileMoveReport> =>
  strip(await execute({ type: "rename_files", variant_ids, spec }, "files_moved"));
/** フォルダを作る。 */
export const createFolder = async (parent_id: FolderId, name: string): Promise<FolderInfo> =>
  strip(await execute({ type: "create_folder", parent_id, name }, "folder"));
/** フォルダの名前を変える。 */
export const renameFolder = async (folder_id: FolderId, name: string): Promise<FolderInfo> =>
  strip(await execute({ type: "rename_folder", folder_id, name }, "folder"));
/** フォルダを別のフォルダの中へ移す。 */
export const moveFolder = async (folder_id: FolderId, parent_id: FolderId): Promise<FolderInfo> =>
  strip(await execute({ type: "move_folder", folder_id, parent_id }, "folder"));
/** 写真が登録されていないフォルダ（と配下）をカタログから除く（ディスクには触れない）。除いたフォルダの数。 */
export const removeFolder = async (folder_id: FolderId): Promise<number> =>
  (await execute({ type: "remove_folder", folder_id }, "updated")).count;
/** 見つからない写真を確かめる（ジョブ。フォルダを与えれば、その配下だけ）。 */
export const checkMissing = async (folder_id: FolderId | null = null): Promise<number> =>
  (await execute({ type: "check_missing", folder_id }, "job_started")).job_id;
/** 再リンクの候補を探す（ジョブ。variant_ids が null なら、すべての見つからない写真）。 */
export const findRelinkCandidates = async (
  variant_ids: VariantId[] | null,
  search_dir: string,
  recursive = true,
): Promise<number> =>
  (await execute({ type: "find_relink_candidates", variant_ids, search_dir, recursive }, "job_started")).job_id;
/** 見つからないファイルを結び付け直す。 */
export const relinkFiles = async (links: RelinkRequest[]): Promise<RelinkReport> =>
  strip(await execute({ type: "relink_files", links }, "relinked"));
/** フォルダの場所の指定し直しの予定。 */
export const planRelocateFolder = async (folder_id: FolderId, path: string): Promise<RelocatePlan> =>
  strip(await execute({ type: "plan_relocate_folder", folder_id, path }, "relocate_plan"));
/** フォルダの場所を指定し直す。 */
export const relocateFolder = async (folder_id: FolderId, path: string): Promise<RelocatePlan> =>
  strip(await execute({ type: "relocate_folder", folder_id, path }, "relocate_plan"));
export type { DisplayInfo };
