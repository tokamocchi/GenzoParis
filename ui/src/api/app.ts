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
  CloseFailure,
  DisplayInfo,
  DisplayTarget,
  ExternalTools,
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
export const locateTools = (dir: string | null) => call<ExternalTools>("locate_tools", { dir });
/** パスの種類（フォルダ・ファイル・ない）。ドロップしたものからフォルダを選ぶのに使う。 */
export const pathKinds = (paths: string[]) => call<("dir" | "file" | "missing")[]>("path_kinds", { paths });
export const revealVariantFile = (variantId: VariantId) =>
  call<void>("reveal_variant_file", { variantId });
export const openVariantFile = (variantId: VariantId) =>
  call<void>("open_variant_file", { variantId });

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

export async function quitApp(discardUnsaved: boolean): Promise<CloseResult> {
  try {
    await bridge().invoke<void>("quit_app", { discardUnsaved });
    return { ok: true };
  } catch (e) {
    return { ok: false, failure: closeFailure(e) };
  }
}
