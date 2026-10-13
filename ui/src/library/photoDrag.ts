// 写真のドラッグ＆ドロップ（グリッドの写真 → 左のパネルのコレクション・キーワード。LIB-09・LIB-10）。
//
// - アプリのウィンドウは、フォルダのドロップでの取り込みのために Tauri のネイティブのドラッグ＆ドロップを
//   有効にしている（tauri.conf.json の dragDropEnabled）。その設定では Windows の WebView2 で HTML5 の
//   ドラッグ＆ドロップのイベントが届かないため、ポインターのイベントで自作する（決定事項ログ No.82）。
// - ボタンを押したまま DRAG_THRESHOLD_PX より動かしたらドラッグを始める（クリック・ダブルクリックと
//   見分ける）。選択中の写真をつかんだら選択中のすべて、選んでいない写真をつかんだらその 1 枚を運ぶ。
// - 受け取る先は、要素の data 属性（data-drop-kind = "collection" | "keyword"、data-drop-id、
//   data-drop-ok = "1"（受け取れる）、data-drop-label）で示す。離した位置の要素から探す。
// - Esc でやめる。ドラッグの後のクリックは選択を変えない（Grid が consumeDragClick で確かめる）。

import type { CollectionId, KeywordId, VariantId } from "../api/types";
import { createStore, useStore } from "../state/store";

/** ドラッグを始めるまでに動かす距離（px。仮置き）。 */
export const DRAG_THRESHOLD_PX = 6;

export type DropTarget =
  | { kind: "collection"; id: CollectionId; ok: boolean; label: string }
  | { kind: "keyword"; id: KeywordId; ok: boolean; label: string };

export interface PhotoDrag {
  ids: VariantId[];
  x: number;
  y: number;
  target: DropTarget | null;
}

export const dragStore = createStore<{ drag: PhotoDrag | null }>({ drag: null });

export function usePhotoDrag(): PhotoDrag | null {
  return useStore(dragStore, (s) => s.drag);
}

/** 動かした距離がドラッグを始める距離を超えたか。 */
export function exceedsThreshold(dx: number, dy: number): boolean {
  return dx * dx + dy * dy > DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX;
}

/** 運ぶ写真（つかんだ写真が選択中ならすべて、そうでなければその 1 枚）。 */
export function dragIds(selected: readonly VariantId[], grabbed: VariantId): VariantId[] {
  return selected.includes(grabbed) ? [...selected] : [grabbed];
}

/** 要素の data 属性から受け取る先を読む（なければ null）。 */
export function parseDropTarget(data: DOMStringMap | Record<string, string | undefined>): DropTarget | null {
  const id = Number(data.dropId);
  if (!Number.isInteger(id) || id <= 0) return null;
  const ok = data.dropOk === "1";
  const label = data.dropLabel ?? "";
  if (data.dropKind === "collection") return { kind: "collection", id, ok, label };
  if (data.dropKind === "keyword") return { kind: "keyword", id, ok, label };
  return null;
}

/** 画面の位置の受け取る先。 */
export function dropTargetAt(x: number, y: number): DropTarget | null {
  if (typeof document === "undefined") return null;
  const el = document.elementFromPoint(x, y);
  const target = el instanceof HTMLElement ? el.closest<HTMLElement>("[data-drop-kind]") : null;
  return target ? parseDropTarget(target.dataset) : null;
}

/** ドラッグの直後のクリックを選択に使わないための印。 */
let suppressClickUntil = 0;

/** 直前にドラッグしていたか（していれば、その後のクリックは選択を変えない）。 */
export function consumeDragClick(): boolean {
  if (performance.now() < suppressClickUntil) {
    suppressClickUntil = 0;
    return true;
  }
  return false;
}

/**
 * ボタンを押したところから、ドラッグを見張る。`resolveIds` はドラッグを始めるときに呼ぶ（運ぶ写真を決め、
 * 必要なら選択を変える）。`onDrop` は受け取れる先で離したときに呼ぶ。
 */
export function watchPhotoDrag(
  start: { x: number; y: number },
  resolveIds: () => VariantId[],
  onDrop: (ids: VariantId[], target: DropTarget) => void,
): void {
  let dragging = false;
  let ids: VariantId[] = [];
  const cleanup = () => {
    window.removeEventListener("pointermove", move, true);
    window.removeEventListener("pointerup", up, true);
    window.removeEventListener("keydown", key, true);
    window.removeEventListener("blur", cancel);
    dragStore.set({ drag: null });
  };
  const move = (e: PointerEvent) => {
    if (!dragging) {
      if (!exceedsThreshold(e.clientX - start.x, e.clientY - start.y)) return;
      ids = resolveIds();
      if (ids.length === 0) {
        cleanup();
        return;
      }
      dragging = true;
    }
    e.preventDefault();
    dragStore.set({ drag: { ids, x: e.clientX, y: e.clientY, target: dropTargetAt(e.clientX, e.clientY) } });
  };
  const up = (e: PointerEvent) => {
    const target = dragging ? dropTargetAt(e.clientX, e.clientY) : null;
    if (dragging) suppressClickUntil = performance.now() + 300;
    cleanup();
    if (target && target.ok) onDrop(ids, target);
  };
  const key = (e: KeyboardEvent) => {
    if (e.key === "Escape" && dragging) {
      e.preventDefault();
      e.stopPropagation();
      suppressClickUntil = performance.now() + 300;
      cleanup();
    }
  };
  const cancel = () => cleanup();
  window.addEventListener("pointermove", move, true);
  window.addEventListener("pointerup", up, true);
  window.addEventListener("keydown", key, true);
  window.addEventListener("blur", cancel);
}
