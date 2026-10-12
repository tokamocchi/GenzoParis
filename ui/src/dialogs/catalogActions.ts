// カタログの切り替え・閉じる・復元（設定のダイアログ・カタログの選択の画面・カタログを開けないときの画面・
// 閉じられなかったときのダイアログが使う）。
//
// 土台の actions にない状態の変更（カタログを閉じた後の状態・開けなかった理由の消去・カタログの設定の更新）は、
// ここで appStore を直接変える。土台に同じ操作（actions.catalogClosed など）ができたら置き換える。

import * as app from "../api/app";
import { toApiError } from "../api/errors";
import type { CoreSettings, ErrorInfo, JobInfo } from "../api/types";
import { actions, appStore } from "../state/app";
import { closeDevelopSession } from "../state/develop";
import { searchCache } from "../state/search";
import { isCloseSaveFailure, retryShouldCreate } from "./catalogLogic";
import { baseName } from "./dialogFormat";
import { clearResults } from "./resultLog";

/** カタログを閉じたときに呼ぶもの（ジョブの待ちを終わらせる）。 */
const closeListeners = new Set<() => void>();

/**
 * カタログを閉じた後の状態にする（カタログの選択の画面に戻る）。
 *
 * コアは閉じるときに実行中のジョブを取り消すが、閉じた後はイベント（job_finished）を送らず、ジョブの状態も
 * 問い合わせられない。そのため、実行中だったジョブは取り消したと知らせ、この画面の部品の待ち
 * （waitJobUntilClosed）を終わらせる。土台の actions.waitJob の待ちは終わらない（土台への依頼）。
 */
export function markCatalogClosed(): void {
  const running = Object.values(appStore.get().jobs).filter((j) => j.state === "running");
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
  // 前のカタログの結果（書き出し・削除）は、別のカタログでは見直しても意味がないので消す。
  clearResults();
  for (const l of [...closeListeners]) l();
  if (running.length > 0) {
    actions.notify(
      "info",
      "カタログを閉じたため、実行中の処理を取り消しました",
      running.map((j) => `・${j.label}`).join("\n"),
    );
  }
}

/**
 * ジョブが終わるのを待つ（actions.waitJob）。終わる前にカタログを閉じたら、待つのをやめて "closed" を返す
 * （取り消したことは markCatalogClosed が知らせる）。
 */
export function waitJobUntilClosed(jobId: number): Promise<JobInfo | "closed"> {
  return new Promise((resolve, reject) => {
    const onClose = () => {
      closeListeners.delete(onClose);
      resolve("closed");
    };
    closeListeners.add(onClose);
    actions.waitJob(jobId).then(
      (info) => {
        closeListeners.delete(onClose);
        resolve(info);
      },
      (e) => {
        closeListeners.delete(onClose);
        reject(e);
      },
    );
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

/**
 * 最後に開こうとしたカタログと、作るつもりだったか（`create`）。
 *
 * 開けなかった理由（appStore.catalogError）には `create` が残らないので、「再試行」で同じ開き方をするために
 * 覚えておく（土台の catalogError に create を持たせたら置き換える）。
 */
let lastOpenAttempt: { path: string; create: boolean } | null = null;

/** カタログを開く（`create` なら、なければ作る）。失敗は catalogError に入る（actions.openCatalog）。 */
export async function openCatalogAt(path: string, create: boolean): Promise<boolean> {
  lastOpenAttempt = { path, create };
  return actions.openCatalog(path, create);
}

/**
 * 開けなかったカタログを、もう一度開く（開けないときの画面の「再試行」）。
 *
 * 作るつもりで失敗した（フォルダを作れないなど）ときは、まだなければ作る（「開く」で再試行すると、
 * 「見つかりません」という別のエラーになるため）。
 */
export async function retryOpenCatalog(path: string): Promise<boolean> {
  const wanted = lastOpenAttempt !== null && lastOpenAttempt.path === path && lastOpenAttempt.create;
  let create = false;
  if (wanted) {
    const state = await app.inspectCatalogPath(path).catch(() => null);
    create = retryShouldCreate(wanted, state);
  }
  return openCatalogAt(path, create);
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
          await openCatalogAt(path, create);
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
  await openCatalogAt(path, create);
}

/** 復元の結果。 */
export type RestoreResult =
  | { ok: true; opened: boolean; displaced: string[]; openError: ErrorInfo | null }
  /**
   * 復元できなかった。`closeFailed` は、開いているカタログの保存待ちの現像設定を保存できず、閉じられ
   * なかった（カタログは開いたまま。復元はしていない）。
   */
  | { ok: false; error: ErrorInfo; closeFailed: boolean };

/**
 * バックアップから復元して開き直す（SYS-04・DATA-04）。今のカタログは名前を変えて残る（アプリの
 * restore_catalog。genzo-api の apply_restore）。
 *
 * - 開き直せたら、その状態にする（ダイアログは閉じる）。
 * - 復元したが開けなければ、カタログを開けないときの画面にする。
 * - 失敗したとき: アプリはバックアップを検証してから閉じるので、使えないバックアップならカタログは開いた
 *   まま。閉じた後の差し替え（apply_restore）に失敗したときは閉じたままなので、開き直す。保存待ちの
 *   現像設定を保存できずに閉じられなかったときは `closeFailed` で知らせる（画面は、保存せずに閉じてから
 *   復元するかを選ばせる。決定事項ログ No.61）。
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
    lastOpenAttempt = { path: catalogPath, create: false };
    setCatalogError(catalogPath, error);
    return { ok: true, opened: false, displaced: outcome.displaced, openError: error };
  } catch (e) {
    const error = toApiError(e).info;
    let closeFailed = false;
    if (wasOpen) {
      const st = await app.appStatus().catch(() => null);
      if (st && !st.catalog) {
        markCatalogClosed();
        await openCatalogAt(catalogPath, false);
      } else if (st?.catalog && st.catalog.catalog_path === open?.catalog_path && isCloseSaveFailure(error)) {
        closeFailed = true;
      }
    }
    return { ok: false, error, closeFailed };
  }
}

/**
 * 保存待ちの現像設定を捨ててカタログを閉じてから、バックアップから復元する（復元の前に閉じられなかった
 * ときの「保存せずに閉じてから復元」。決定事項ログ No.61）。
 */
export async function restoreDiscardingUnsaved(catalogPath: string, backupPath: string): Promise<RestoreResult> {
  const r = await app.closeCatalog(true);
  if (!r.ok && r.failure.still_open) return { ok: false, error: r.failure.error, closeFailed: true };
  if (!r.ok) {
    actions.notify(
      "warning",
      `カタログを閉じる処理でエラーがありました: ${r.failure.error.message}`,
      r.failure.error.hint ?? undefined,
    );
  }
  markCatalogClosed();
  const res = await restoreFromBackup(catalogPath, backupPath);
  if (!res.ok) {
    // 閉じたまま復元できなかった: 元のカタログを開き直す（開けなければ、開けないときの画面になる）。
    const st = await app.appStatus().catch(() => null);
    if (st && !st.catalog) await openCatalogAt(catalogPath, false);
  }
  return res;
}
