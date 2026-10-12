// カタログの場所を選ぶ（新しく作る・既存を開く）。選んだパスを確かめてから、開き方を返す（開くのは呼び出し側）。
//
// 開く前に確かめるのは、カタログでないファイルを上書き・開こうとして、今のカタログを閉じてしまわないため。

import * as app from "../api/app";
import { describeError } from "../api/errors";
import { type CatalogIntent, decideCatalogPath } from "./catalogLogic";
import { ensureDbExtension } from "./dialogFormat";
import { chooseCatalogFile, chooseNewCatalogPath } from "./native";

export type CatalogPick =
  | { kind: "cancel" }
  | { kind: "ok"; path: string; create: boolean }
  | { kind: "error"; message: string };

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
  let chosen: string | null;
  try {
    chosen = await chooseNewCatalogPath("新しいカタログの場所と名前", defaultPath);
  } catch (e) {
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
