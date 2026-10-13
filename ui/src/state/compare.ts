// 比較表示・選別表示（LIB-03）の状態と操作（計算は viewer/compareLogic.ts、画面は viewer/CompareView.tsx・
// viewer/SurveyView.tsx）。
//
// - 比較表示（C）: 選択（左）と候補（右）の 2 枚。作業中の面の写真をアプリの選択の「表示中の写真」にし、選択の
//   集合を [選択, 候補] にする（評価・フラグ・ラベルは作業中の面の写真に付く。actions.targets）。フィルムストリップ
//   で写真を選んだら、組に写す（compareLogic の pairAfterSelection。この状態の購読）。
// - 選別表示（N）: 選択中の写真を並べる。状態は持たず、アプリの選択（selection.ids・current）をそのまま使う。
// - 拡大・パンの同期の入り切り（linked）は、この PC のブラウザの保存領域に覚える。

import type { VariantId } from "../api/types";
import {
  type ComparePair,
  type CompareSide,
  initialPair,
  pairAfterSelection,
  removeFromSurvey,
  selectionOfPair,
  stepInList,
  stepIndex,
  surveyPhotos,
  swapPair,
} from "../viewer/compareLogic";
import { actions, appStore } from "./app";
import { searchCache } from "./search";
import { createStore, useStore } from "./store";

export interface CompareState {
  pair: ComparePair;
  /** 作業中の面（評価などの対象。アプリの選択の表示中の写真）。 */
  active: CompareSide;
  /** 拡大・パンを 2 つの面で同期するか。 */
  linked: boolean;
}

const LINKED_KEY = "genzo.compare.linked";

function loadLinked(): boolean {
  try {
    return localStorage.getItem(LINKED_KEY) !== "0";
  } catch {
    return true;
  }
}

export const compareStore = createStore<CompareState>({
  pair: { select: null, candidate: null },
  active: "select",
  linked: typeof localStorage === "undefined" ? true : loadLinked(),
});

export function useCompare<T>(selector: (s: CompareState) => T, equal?: (a: T, b: T) => boolean): T {
  return useStore(compareStore, selector, equal);
}

/** 組を選択にする（[選択, 候補]・表示中の写真は作業中の面の写真）。 */
async function applyPair(pair: ComparePair, active: CompareSide): Promise<void> {
  compareStore.set({ pair, active });
  const { ids, current } = selectionOfPair(pair, active);
  if (current === null) return;
  await actions.selectIds(ids, current);
}

/** 写真の今の世代の位置（読み込み済みならその位置、なければコアに問い合わせる。結果になければ null）。 */
async function indexOf(id: VariantId | null): Promise<number | null> {
  if (id === null) return null;
  const at = searchCache.findLoaded(id);
  if (at >= 0) return at;
  return actions.locate(id, null);
}

/**
 * 比較表示を始める（C）。選択は表示中の写真、候補は選択中のほかの写真、なければ検索結果の次（最後なら前）の写真。
 */
export async function openCompare(): Promise<void> {
  // 比較表示の間にもう一度押しても、組を作り直さない（作業中の面が入れ替わらないように）。
  if (appStore.get().mode === "compare") return;
  if (searchCache.count === 0) {
    actions.notify("info", "比較する写真がありません");
    return;
  }
  if (appStore.get().selection.current === null && !(await actions.selectIndex(0))) return;
  const sel = appStore.get().selection;
  const pair = initialPair(sel.ids, sel.current);
  if (pair.candidate === null) {
    const at = sel.currentIndex ?? (await indexOf(pair.select));
    const next = stepIndex(at, 1, searchCache.count, at) ?? stepIndex(at, -1, searchCache.count, at);
    if (next !== null) pair.candidate = (await searchCache.fetch(next))?.variant_id ?? null;
  }
  await applyPair(pair, "select");
  actions.setMode("compare");
}

/** 選別表示を始める（N）。選択中の写真を並べる（1 枚だけなら、その 1 枚）。 */
export function openSurvey(): void {
  if (searchCache.count === 0) {
    actions.notify("info", "並べる写真がありません");
    return;
  }
  actions.setMode("survey");
}

/** 候補を前・次の写真にする（←・→。選択の写真は飛ばす）。 */
export async function stepCandidate(delta: 1 | -1): Promise<void> {
  const { pair, active } = compareStore.get();
  const select = await indexOf(pair.select);
  const candidate = await indexOf(pair.candidate);
  const next = stepIndex(candidate, delta, searchCache.count, select);
  if (next === null) return;
  const item = await searchCache.fetch(next);
  if (!item || compareStore.get().pair !== pair) return;
  await applyPair({ ...pair, candidate: item.variant_id }, active);
}

/** 候補を選択にし、その次の写真を候補にする（↑）。 */
export async function makeCandidateSelect(): Promise<void> {
  const { pair } = compareStore.get();
  if (pair.candidate === null) return;
  const at = await indexOf(pair.candidate);
  const next = stepIndex(at, 1, searchCache.count, at);
  const item = next === null ? undefined : await searchCache.fetch(next);
  if (compareStore.get().pair !== pair) return;
  const candidate = item && item.variant_id !== pair.candidate ? item.variant_id : null;
  await applyPair({ select: pair.candidate, candidate }, "select");
}

/** 選択と候補を入れ替える（↓）。作業中の面は同じ写真のまま（反対の面に移る）。 */
export async function swapCompare(): Promise<void> {
  const { pair, active } = compareStore.get();
  if (pair.candidate === null) return;
  await applyPair(swapPair(pair), active === "select" ? "candidate" : "select");
}

/** 作業中の面を変える（面のクリック）。 */
export async function setCompareActive(side: CompareSide): Promise<void> {
  const { pair, active } = compareStore.get();
  if (side === active || pair[side] === null) return;
  await applyPair(pair, side);
}

/** 拡大・パンの同期を入れる・切る。 */
export function setCompareLinked(linked: boolean): void {
  try {
    localStorage.setItem(LINKED_KEY, linked ? "1" : "0");
  } catch {
    // 保存できなくても、このセッションでは使う。
  }
  compareStore.set({ linked });
}

// アプリの選択の変化（フィルムストリップのクリックなど）を組に写す（比較表示の間だけ）。
let lastSelection = appStore.get().selection;
appStore.subscribe(() => {
  const s = appStore.get();
  if (s.selection === lastSelection) return;
  lastSelection = s.selection;
  if (s.mode !== "compare") return;
  const { pair, active } = compareStore.get();
  const next = pairAfterSelection(pair, active, s.selection);
  if (!next) return;
  if (next.pair === pair) {
    compareStore.set({ active: next.active });
    return;
  }
  void applyPair(next.pair, next.active);
});

// 別のカタログを開いた・閉じた: id が別のものを指すので忘れる。
let catalogPath = appStore.get().catalog?.catalog_path ?? null;
appStore.subscribe(() => {
  const p = appStore.get().catalog?.catalog_path ?? null;
  if (p === catalogPath) return;
  catalogPath = p;
  compareStore.set({ pair: { select: null, candidate: null }, active: "select" });
});

// ---------------------------------------------------------------------------
// 選別表示
// ---------------------------------------------------------------------------

/** 選別表示に並べる写真（selection から。compareLogic の surveyPhotos）。 */
export function surveyOf(sel: { ids: readonly VariantId[]; current: VariantId | null }) {
  return surveyPhotos(sel.ids, sel.current);
}

/** 選別表示の作業中の写真を変える（クリック）。 */
export function focusSurvey(id: VariantId): void {
  const sel = appStore.get().selection;
  if (sel.current === id) return;
  void actions.selectIds(sel.ids, id);
}

/** 選別表示の作業中の写真を前・次へ（←・→）。 */
export function stepSurvey(delta: 1 | -1): void {
  const sel = appStore.get().selection;
  const next = stepInList(surveyOf(sel).shown, sel.current, delta);
  if (next !== null && next !== sel.current) void actions.selectIds(sel.ids, next);
}

/** 選別表示から外す（× と「/」。選択から外す。写真は消えない）。外したら true。 */
export function removeSurveyPhoto(id: VariantId): boolean {
  const sel = appStore.get().selection;
  const next = removeFromSurvey(surveyOf(sel).shown, sel.ids, sel.current, id);
  if (!next) return false;
  void actions.selectIds(next.ids, next.current);
  return true;
}
