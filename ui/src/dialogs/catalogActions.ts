// カタログの切り替え・閉じる・復元（設定のダイアログ・カタログの選択の画面・カタログを開けないときの画面・
// 閉じられなかったときのダイアログが使う）。
//
// 土台の actions にない状態の変更（カタログを閉じた後の状態・開けなかった理由の消去・カタログの設定の更新）は、
// ここで appStore を直接変える。土台に同じ操作（actions.catalogClosed など）ができたら置き換える。

import * as app from "../api/app";
import { toApiError } from "../api/errors";
import type { CoreSettings, ErrorInfo } from "../api/types";
import { actions, appStore } from "../state/app";
import { closeDevelopSession } from "../state/develop";
import { searchCache } from "../state/search";
import { baseName } from "./dialogFormat";

/** カタログを閉じた後の状態にする（カタログの選択の画面に戻る）。 */
export function markCatalogClosed(): void {
  // 新しく開くカタログの検索の世代は 1 から数え直すので、古い世代を残さない（search_updated を捨てないため）。
  searchCache.reset(0, 0);
  appStore.set({
    catalog: null,
    search: null,
    searching: false,
    selection: { ids: [], current: null, currentIndex: null, anchorIndex: null },
    mode: "grid",
    dialog: null,
    thumbRevs: {},
    // コピーした設定の元の写真の ID・前のカタログのジョブは、別のカタログでは意味がない。
    clipboard: null,
    jobs: {},
  });
}

/** カタログを開けなかった理由を消す（カタログの選択の画面に戻る）。 */
export function clearCatalogError(): void {
  appStore.set({ catalogError: null });
}

function setCatalogError(path: string, error: ErrorInfo): void {
  appStore.set({ catalogError: { path, error } });
}

/** 開いているカタログの設定を、保存した値にする（書き出しの既定の色空間などを使う画面のため）。 */
export function setCoreSettings(settings: CoreSettings): void {
  appStore.set((s) => (s.catalog ? { catalog: { ...s.catalog, core_settings: settings } } : {}));
}

/** 現像の送信待ちの値を送り、現像の写真を閉じる（カタログを閉じる・復元する前。保存待ちはコアが保存する）。 */
async function settleDevelop(): Promise<void> {
  if (appStore.get().mode !== "grid") actions.setMode("grid");
  await closeDevelopSession();
}

/** カタログを閉じた後に続けること（切り替え先を開く）。閉じられなかったときのダイアログが使う。 */
let afterClose: (() => Promise<void>) | null = null;

/** 閉じられた: カタログの選択の画面に戻し、続き（切り替え先を開く）があれば行う。 */
export async function continueAfterClose(): Promise<void> {
  const next = afterClose;
  afterClose = null;
  markCatalogClosed();
  if (next) await next();
}

/** 閉じるのをやめた（続きを捨てる）。 */
export function cancelAfterClose(): void {
  afterClose = null;
}

/**
 * 今のカタログを閉じてから、別のカタログを開く（`create` なら、なければ作る）。
 *
 * 閉じるときに保存待ちの現像設定を保存できなければ、閉じられなかったときのダイアログを出す（決定事項ログ
 * No.61）。そこで閉じられたら、続けて開く。開けなければ、カタログを開けないときの画面になる（catalogError）。
 */
export async function switchCatalog(path: string, create: boolean): Promise<void> {
  const cur = appStore.get().catalog;
  if (cur && cur.catalog_path === path) {
    actions.notify("info", "このカタログは開いています");
    return;
  }
  if (cur) {
    await settleDevelop();
    const r = await app.closeCatalog(false);
    if (!r.ok) {
      if (r.failure.still_open) {
        afterClose = async () => {
          await actions.openCatalog(path, create);
        };
        actions.openDialog({ kind: "close_failed", failure: r.failure, quitting: false });
        return;
      }
      // 閉じたが、終了の処理の途中でエラーがあった（カタログは閉じている）。
      actions.notify(
        "warning",
        `カタログを閉じる処理でエラーがありました: ${r.failure.error.message}`,
        r.failure.error.hint ?? undefined,
      );
    }
    markCatalogClosed();
  }
  actions.notify("info", `${baseName(path)} を開いています…`);
  await actions.openCatalog(path, create);
}

/** 復元の結果。 */
export type RestoreResult =
  | { ok: true; opened: boolean; displaced: string[]; openError: ErrorInfo | null }
  | { ok: false; error: ErrorInfo };

/**
 * バックアップから復元して開き直す（SYS-04・DATA-04）。今のカタログは名前を変えて残る（アプリの
 * restore_catalog。genzo-api の apply_restore）。
 *
 * - 開き直せたら、その状態にする（ダイアログは閉じる）。
 * - 復元したが開けなければ、カタログを開けないときの画面にする。
 * - 失敗したとき、復元しようとしたカタログが開いていたのに閉じていれば（復元の前に閉じるため）、開き直す。
 */
export async function restoreFromBackup(catalogPath: string, backupPath: string): Promise<RestoreResult> {
  const open = appStore.get().catalog;
  const wasOpen = open !== null && open.catalog_path === catalogPath;
  if (wasOpen) await settleDevelop();
  try {
    const outcome = await app.restoreCatalog(catalogPath, backupPath);
    if (outcome.status) {
      markCatalogClosed();
      await actions.catalogOpened(outcome.status);
      return { ok: true, opened: true, displaced: outcome.displaced, openError: null };
    }
    const error: ErrorInfo = outcome.open_error ?? {
      kind: "internal",
      message: "復元したカタログを開けませんでした（理由は不明）",
      user_actionable: false,
      retryable: false,
      hint: null,
    };
    markCatalogClosed();
    setCatalogError(catalogPath, error);
    return { ok: true, opened: false, displaced: outcome.displaced, openError: error };
  } catch (e) {
    const error = toApiError(e).info;
    if (wasOpen) {
      const st = await app.appStatus().catch(() => null);
      if (st && !st.catalog) {
        markCatalogClosed();
        await actions.openCatalog(catalogPath, false);
      }
    }
    return { ok: false, error };
  }
}
