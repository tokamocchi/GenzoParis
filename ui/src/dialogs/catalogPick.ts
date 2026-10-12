// カタログの場所を選ぶ（新しく作る・既存を開く）。選んだパスを確かめてから、開き方を返す（開くのは呼び出し側）。
//
// 開く前に確かめるのは、カタログでないファイルを上書き・開こうとして、今のカタログを閉じてしまわないため。
//
// 新しいカタログの場所は保存のダイアログ（@tauri-apps/plugin-dialog の save）で選ぶ。アプリの権限
// （crates/genzo-app/capabilities/default.json）に dialog:allow-save がなくて開けないときは、
// "no_save_dialog" を返す。呼び出し側は、フォルダ（開くのダイアログ）と名前（画面の入力）で選ばせる
// （NewCatalogPicker.tsx）。一度断られたら、このセッションでは保存のダイアログを試さない。

import * as app from "../api/app";
import { describeError } from "../api/errors";
import { type CatalogIntent, decideCatalogPath, isDialogPermissionError, newCatalogFileName } from "./catalogLogic";
import { ensureDbExtension, joinPath } from "./dialogFormat";
import { chooseCatalogFile, chooseNewCatalogPath } from "./native";

export type CatalogPick =
  | { kind: "cancel" }
  | { kind: "ok"; path: string; create: boolean }
  | { kind: "error"; message: string }
  /** 保存のダイアログを使えない（権限がない）。フォルダと名前で選ばせる。 */
  | { kind: "no_save_dialog" };

/** 保存のダイアログが権限で断られたか（このセッションでは、もう試さない）。 */
let saveDialogDenied = false;

/** 選んだパスを確かめる。 */
export async function checkCatalogPath(path: string, intent: CatalogIntent): Promise<CatalogPick> {
  try {
    const state = await app.inspectCatalogPath(path);
    const d = decideCatalogPath(state, intent);
    return "error" in d ? { kind: "error", message: `${path}: ${d.error}` } : { kind: "ok", path, create: d.create };
  } catch (e) {
    return { kind: "error", message: describeError(e) };
  }
}

/** 新しいカタログの場所を選ぶ（保存のダイアログ。拡張子 .db を付ける）。 */
export async function pickNewCatalog(defaultPath?: string | null): Promise<CatalogPick> {
  if (saveDialogDenied) return { kind: "no_save_dialog" };
  let chosen: string | null;
  try {
    chosen = await chooseNewCatalogPath("新しいカタログの場所と名前", defaultPath);
  } catch (e) {
    if (isDialogPermissionError(e)) {
      console.warn("保存のダイアログを開けない（権限）。フォルダと名前で選ばせる", e);
      saveDialogDenied = true;
      return { kind: "no_save_dialog" };
    }
    return { kind: "error", message: describeError(e) };
  }
  if (!chosen) return { kind: "cancel" };
  return checkCatalogPath(ensureDbExtension(chosen), "new");
}

/** 既存のカタログを選ぶ。 */
export async function pickExistingCatalog(defaultPath?: string | null): Promise<CatalogPick> {
  let chosen: string | null;
  try {
    chosen = await chooseCatalogFile("開くカタログ（.db）", defaultPath);
  } catch (e) {
    return { kind: "error", message: describeError(e) };
  }
  if (!chosen) return { kind: "cancel" };
  return checkCatalogPath(chosen, "existing");
}

/** フォルダと名前から、新しいカタログの場所を決めて確かめる（保存のダイアログを使えないとき）。 */
export async function checkNewCatalogInFolder(folder: string, name: string): Promise<CatalogPick> {
  if (folder.trim() === "") return { kind: "error", message: "カタログを置くフォルダを選んでください" };
  const n = newCatalogFileName(name);
  if ("error" in n) return { kind: "error", message: n.error };
  return checkCatalogPath(joinPath(folder, n.name), "new");
}
