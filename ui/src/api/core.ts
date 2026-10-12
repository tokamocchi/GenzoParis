// コア API（genzo-api の Command）の呼び出し。
//
// すべて genzo-app の `core_execute` を通す（JSON の Command → Response）。応答の `type` を確かめて、
// 型付きの値を返す。

import { bridge } from "./bridge";
import { ApiError, toApiError } from "./errors";
import type {
  BackupEntry,
  CatalogStats,
  Capabilities,
  ColorLabel,
  Command,
  CoreSettings,
  CropRect,
  DeleteKind,
  DeletePlan,
  DeleteReport,
  DevelopSettings,
  DevelopState,
  DisplayInfo,
  ExportSettings,
  Facets,
  FileCheckReport,
  Flag,
  FolderInfo,
  FrameInfo,
  HistoryEntryId,
  HistoryItem,
  IndexOf,
  JobInfo,
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
  StartupReport,
  VariantDetails,
  VariantId,
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
export const importFolder = async (path: string, recursive = true) =>
  (await execute({ type: "import_folder", path, recursive }, "job_started")).job_id;
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
export const setRating = async (variant_ids: VariantId[], rating: Rating) =>
  (await execute({ type: "set_rating", variant_ids, rating }, "updated")).count;
export const setFlag = async (variant_ids: VariantId[], flag: Flag) =>
  (await execute({ type: "set_flag", variant_ids, flag }, "updated")).count;
export const setColorLabel = async (variant_ids: VariantId[], label: ColorLabel | null) =>
  (await execute({ type: "set_color_label", variant_ids, label }, "updated")).count;
export const setCaption = async (variant_ids: VariantId[], caption: string | null) =>
  (await execute({ type: "set_caption", variant_ids, caption }, "updated")).count;
export const variantDetails = async (variant_id: VariantId): Promise<VariantDetails> =>
  strip(await execute({ type: "variant_details", variant_id }, "details"));
export const folders = async (): Promise<FolderInfo[]> =>
  (await execute({ type: "folders" }, "folders")).folders;
export const facets = async (): Promise<Facets> => strip(await execute({ type: "facets" }, "facets"));
export const checkFiles = async (variant_ids: VariantId[]): Promise<FileCheckReport> =>
  strip(await execute({ type: "check_files", variant_ids }, "file_check"));

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
export const capabilities = async (): Promise<Capabilities> =>
  strip(await execute({ type: "capabilities" }, "capabilities"));
export type { DisplayInfo };
