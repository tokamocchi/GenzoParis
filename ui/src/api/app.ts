// アプリのコマンド（genzo-app の commands.rs）。

import { bridge } from "./bridge";
import { toApiError } from "./errors";
import type {
  AppSettings,
  AppSettingsUpdate,
  AppStatus,
  BackupEntry,
  CatalogFileState,
  CatalogStatus,
  ClearReport,
  CloseFailure,
  DisplayInfo,
  DisplayTarget,
  ExternalTools,
  ImportPreferences,
  LogFolderInfo,
  MapCacheInfo,
  MapLink,
  MapPreferences,
  Place,
  RestoreOutcome,
  VariantId,
} from "./types";

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await bridge().invoke<T>(command, args);
  } catch (e) {
    throw toApiError(e);
  }
}

export const appStatus = () => call<AppStatus>("app_status");
export const inspectCatalogPath = (path: string) =>
  call<CatalogFileState>("inspect_catalog_path", { path });
export const openCatalog = (path: string, create: boolean) =>
  call<CatalogStatus>("open_catalog", { path, create });
export const setDisplayTarget = (target: DisplayTarget) =>
  call<DisplayInfo | null>("set_display_target", { target });
export const listCatalogBackups = (catalogPath: string) =>
  call<BackupEntry[]>("list_catalog_backups", { catalogPath });
export const restoreCatalog = (catalogPath: string, backup: string) =>
  call<RestoreOutcome>("restore_catalog", { catalogPath, backup });
export const updateAppSettings = (update: AppSettingsUpdate) =>
  call<AppSettings>("update_app_settings", { update });
/** 取り込みのダイアログの設定を保存する（次に開いたときの初めの値。IMP-02・IMP-03・IMP-05）。 */
export const saveImportPreferences = (preferences: ImportPreferences) =>
  call<AppSettings>("save_import_preferences", { preferences });
export const locateTools = (dir: string | null) => call<ExternalTools>("locate_tools", { dir });
/** パスの種類（フォルダ・ファイル・ない）。ドロップしたものからフォルダを選ぶのに使う。 */
export const pathKinds = (paths: string[]) => call<("dir" | "file" | "missing")[]>("path_kinds", { paths });
export const revealVariantFile = (variantId: VariantId) =>
  call<void>("reveal_variant_file", { variantId });
export const openVariantFile = (variantId: VariantId) =>
  call<void>("open_variant_file", { variantId });

// オンラインの地図（位置情報の後付け LIB-21 の第 2 段階。genzo-app の map.rs。既定で無効で、有効なときだけ
// genzo-app が外部のサーバーと通信する。地図のタイルは genzo:// の /tile/{z}/{x}/{y}（uri.ts の tileUrlTemplate））

/**
 * オンラインの地図の設定を保存する（すぐ効く）。欄はすべて送る（欠けた欄はアプリが既定値にするので、URL を
 * 消さないよう今の値に重ねて送る。state/map.ts の saveMapPreferences）。consented_at はアプリが入れる。
 */
export const saveMapPreferences = (preferences: MapPreferences) =>
  call<AppSettings>("save_map_preferences", { preferences });
/**
 * 地名を検索する（設定した検索サービスか、設定で選んだときの OSM の公開の Nominatim）。**Enter か「検索」の
 * ボタンのときだけ呼ぶ**（入力の途中で呼ばない。Nominatim のポリシーのオートコンプリートの禁止）。1 秒に 1 回の
 * 間隔と結果のキャッシュは genzo-geo も受け持つ（画面でも 1 秒あける）。オンラインの地図が無効なとき・検索の
 * サービスがない（既定）ときは、通信せずに invalid_input。
 */
export const searchPlaces = (query: string) => call<Place[]>("search_places", { query });
/** 地図のタイルのキャッシュの大きさ（通信しない）。 */
export const mapCacheInfo = () => call<MapCacheInfo>("map_cache_info");
/** 地図のタイルのキャッシュを消して、消した後の大きさを返す（通信しない）。 */
export const clearMapCache = () => call<MapCacheInfo>("clear_map_cache");
/** OSM のページ（帰属の表示・地図の誤りの報告・利用ポリシー）を既定のブラウザで開く（WebView の中では開かない）。 */
export const openMapLink = (kind: MapLink) => call<void>("open_map_link", { kind });

// ログ・クラッシュレポート（SYS-07。ローカルだけ。外部へは送信しない）
export const logFolderInfo = () => call<LogFolderInfo>("log_folder_info");
export const openLogFolder = () => call<void>("open_log_folder");
export const clearLogs = () => call<ClearReport>("clear_logs");
/** 画面のエラーをアプリのログに残す（失敗しても何もしない。ログのためにエラーを重ねない）。 */
export function reportUiError(message: string, detail: string | null): void {
  bridge()
    .invoke<void>("report_ui_error", { message, detail })
    .catch(() => undefined);
}

/** 閉じる・終了の結果（失敗は CloseFailure。カタログが開いたままなら still_open）。 */
export type CloseResult = { ok: true } | { ok: false; failure: CloseFailure };

function closeFailure(e: unknown): CloseFailure {
  if (typeof e === "object" && e !== null && "still_open" in e && "error" in e) {
    return e as CloseFailure;
  }
  return { error: toApiError(e).info, still_open: false };
}

export async function closeCatalog(discardUnsaved: boolean): Promise<CloseResult> {
  try {
    await bridge().invoke<void>("close_catalog", { discardUnsaved });
    return { ok: true };
  } catch (e) {
    return { ok: false, failure: closeFailure(e) };
  }
}

/** 終了の要求（番号 `id`）を受け取ったとアプリに答える（答えがないと、アプリは数秒後に画面を待たずに閉じる）。 */
export function ackQuitRequest(id: number): Promise<void> {
  return bridge().invoke<void>("ack_quit_request", { id });
}

export async function quitApp(discardUnsaved: boolean): Promise<CloseResult> {
  try {
    await bridge().invoke<void>("quit_app", { discardUnsaved });
    return { ok: true };
  } catch (e) {
    return { ok: false, failure: closeFailure(e) };
  }
}
