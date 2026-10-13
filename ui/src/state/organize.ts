// キーワード（LIB-09）・コレクション（LIB-10）・スマートコレクション（LIB-11）の画面の操作。
//
// - 写真への付与・外す、コレクションへの追加・削除は、コアがカタログを変えて catalog_changed（keywords・
//   collections）を送る。画面はそれで一覧（organizeVersion）・詳細（detailsVersion）を読み直す
//   （state/app.ts の handleEvent）。表示中の検索結果はコアが作り直す（search_updated）。
// - ターゲットコレクション（B キーの入れ先）は、カタログごとにブラウザに保存する。B は、選んだ写真が
//   すべて入っていれば外し、そうでなければ入れる（Lightroom と同じ。入れた数が 0 ならすべて入っていた
//   とみなして外す）。
// - 手動のコレクションを表示している間の Delete・Backspace は、カタログから除くのではなく、そのコレクションから
//   外す（写真は消えない。Lightroom と同じ。決定事項ログ No.82）。

import * as core from "../api/core";
import type { CollectionId, CollectionInfo, KeywordId, VariantId } from "../api/types";
import { parseKeywordInput } from "../library/keywordLogic";
import type { DropTarget } from "../library/photoDrag";
import { actions, appStore, targetCollectionKey } from "./app";

/** ターゲットコレクションを決める（null で外す）。カタログごとに保存する。 */
export function setTargetCollection(id: CollectionId | null): void {
  appStore.set({ targetCollection: id });
  const path = appStore.get().catalog?.catalog_path;
  if (!path) return;
  try {
    if (id === null) localStorage.removeItem(targetCollectionKey(path));
    else localStorage.setItem(targetCollectionKey(path), String(id));
  } catch {
    // 保存できなくても、このセッションでは使える。
  }
}

/** 写真をコレクションに入れる（結果を知らせる）。入れた数を返す（失敗は null）。 */
export async function addToCollection(id: CollectionId, ids: VariantId[], name?: string): Promise<number | null> {
  if (ids.length === 0) return 0;
  try {
    const n = await core.addToCollection(id, ids);
    const where = name ? `「${name}」` : "コレクション";
    actions.notify("success", n > 0 ? `${n.toLocaleString("ja-JP")} 枚を${where}に入れました` : `すでに${where}に入っています`);
    return n;
  } catch (e) {
    actions.reportError(e, "コレクションに入れられません");
    return null;
  }
}

/** 写真をコレクションから外す（写真は消えない）。外した数を返す（失敗は null）。 */
export async function removeFromCollection(id: CollectionId, ids: VariantId[], name?: string): Promise<number | null> {
  if (ids.length === 0) return 0;
  try {
    const n = await core.removeFromCollection(id, ids);
    actions.notify("info", `${n.toLocaleString("ja-JP")} 枚を${name ? `「${name}」` : "コレクション"}から外しました（写真は削除していません）`);
    return n;
  } catch (e) {
    actions.reportError(e, "コレクションから外せません");
    return null;
  }
}

/**
 * B キー: 操作の対象の写真をターゲットコレクションに入れる（すべて入っていれば外す）。ターゲットがなければ、
 * 入れる先を選ぶダイアログを開く。
 */
export async function toggleTargetCollection(): Promise<void> {
  const ids = actions.targets();
  if (ids.length === 0) return;
  const target = appStore.get().targetCollection;
  if (target === null) {
    actions.openDialog({ kind: "add_to_collection", variantIds: ids });
    return;
  }
  let name: string | undefined;
  try {
    const list = await core.collections(false);
    const c = list.find((x) => x.collection_id === target);
    if (!c || c.kind !== "manual") {
      // ターゲットにしていたコレクションが削除された・手動でなくなった。
      setTargetCollection(null);
      actions.openDialog({ kind: "add_to_collection", variantIds: ids });
      return;
    }
    name = c.name;
    const added = await core.addToCollection(target, ids);
    if (added > 0) {
      actions.notify("success", `${added.toLocaleString("ja-JP")} 枚を「${name}」に入れました（B）`);
    } else {
      const removed = await core.removeFromCollection(target, ids);
      actions.notify("info", `${removed.toLocaleString("ja-JP")} 枚を「${name}」から外しました（B）`);
    }
  } catch (e) {
    actions.reportError(e, name ? `「${name}」に入れられません` : "ターゲットコレクションに入れられません");
  }
}

/** 表示中のコレクション（手動のものだけ。なければ null）。 */
export async function shownManualCollection(): Promise<CollectionInfo | null> {
  const id = appStore.get().filter.collection_id ?? null;
  if (id === null) return null;
  try {
    const list = await core.collections(false);
    const c = list.find((x) => x.collection_id === id);
    return c && c.kind === "manual" ? c : null;
  } catch {
    return null;
  }
}

/**
 * Delete・Backspace: 手動のコレクションを表示していれば、操作の対象をそのコレクションから外す。そうでなければ
 * 削除のダイアログ（カタログから除く）を開く。
 */
export async function deleteKeyAction(): Promise<void> {
  const ids = actions.targets();
  if (ids.length === 0) return;
  const shown = await shownManualCollection();
  if (shown) {
    await removeFromCollection(shown.collection_id, ids, shown.name);
    return;
  }
  actions.openDialog({ kind: "delete", deleteKind: "remove_from_catalog", variantIds: ids });
}

/** 入力欄の文字列（「場所 > 京都, 家族」）のキーワードを写真に付ける。付けたパスの数を返す（失敗は null）。 */
export async function addKeywordText(ids: VariantId[], text: string): Promise<number | null> {
  const paths = parseKeywordInput(text);
  if (paths.length === 0 || ids.length === 0) return 0;
  try {
    await core.addKeywords(ids, [], paths);
    return paths.length;
  } catch (e) {
    actions.reportError(e, "キーワードを付けられません");
    return null;
  }
}

/** 既存のキーワードを写真に付ける・外す。 */
export async function setKeyword(ids: VariantId[], keywordId: KeywordId, on: boolean): Promise<void> {
  if (ids.length === 0) return;
  try {
    if (on) await core.addKeywords(ids, [keywordId]);
    else await core.removeKeywords(ids, [keywordId]);
  } catch (e) {
    actions.reportError(e, on ? "キーワードを付けられません" : "キーワードを外せません");
  }
}

/** キーワードでの絞り込みを切り替える（同じキーワードなら外す）。ほかの条件は残す。 */
export function toggleKeywordFilter(keywordId: KeywordId): void {
  const cur = appStore.get().filter.keywords ?? [];
  actions.setFilter({ keywords: cur.includes(keywordId) ? cur.filter((k) => k !== keywordId) : [keywordId] });
}

/** 入力欄（右のパネルのキーワード）にフォーカスを移す（Ctrl / ⌘ + K）。 */
export const KEYWORD_INPUT_ID = "lib-keyword-input";

export function focusKeywordInput(): boolean {
  const el = typeof document === "undefined" ? null : document.getElementById(KEYWORD_INPUT_ID);
  if (el instanceof HTMLInputElement) {
    el.focus();
    el.select();
    return true;
  }
  return false;
}

/** グリッドの写真を左のパネルのコレクション・キーワードに落としたとき（library/photoDrag.ts）。 */
export async function dropPhotos(ids: VariantId[], target: DropTarget): Promise<void> {
  if (!target.ok || ids.length === 0) return;
  if (target.kind === "collection") {
    await addToCollection(target.id, ids, target.label);
    return;
  }
  try {
    const n = await core.addKeywords(ids, [target.id]);
    actions.notify(
      "success",
      n > 0
        ? `${ids.length.toLocaleString("ja-JP")} 枚にキーワード「${target.label}」を付けました`
        : `キーワード「${target.label}」はすでに付いています`,
    );
  } catch (e) {
    actions.reportError(e, "キーワードを付けられません");
  }
}
