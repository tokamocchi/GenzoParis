// 比較表示・選別表示の操作（state/compare.ts・state/viewRegion.ts）のテスト（コアのコマンドは MockBridge で
// 差し替える）と、画面が代表的な状態で例外なく描けることの確認（react-dom/server。useEffect は動かない）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MockBridge, mockCompareCommands, mockSummary } from "../api/mock";
import type { CatalogStatus } from "../api/types";
import { SHORTCUT_GROUPS } from "../dialogs/shortcutsData";
import { CompareView } from "../viewer/CompareView";
import { SurveyView } from "../viewer/SurveyView";
import { actions, appStore } from "./app";
import {
  compareStore,
  focusSurvey,
  makeCandidateSelect,
  openCompare,
  openSurvey,
  removeSurveyPhoto,
  setCompareActive,
  setCompareLinked,
  stepCandidate,
  stepSurvey,
  swapCompare,
} from "./compare";
import { searchCache } from "./search";
import { cancelViewRegions, releaseView, renderViewRegion, viewOutputSize } from "./viewRegion";

let mock: MockBridge;
let sent: Record<string, unknown>[];
/** 検索結果の並び（位置の順の id）。 */
let order: number[];

function commands(type: string): Record<string, unknown>[] {
  return sent.filter((c) => c.type === type);
}

/** テストの実行環境（node）には localStorage がないので、Map で代わりを置く。 */
function installLocalStorage(): void {
  const data = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => void data.set(k, String(v)),
    removeItem: (k: string) => void data.delete(k),
    clear: () => data.clear(),
    key: (i: number) => [...data.keys()][i] ?? null,
    get length() {
      return data.size;
    },
  };
}

/** 待っている Promise を進める。 */
async function settle(): Promise<void> {
  for (let i = 0; i < 20; i++) await Promise.resolve();
  await new Promise((r) => setTimeout(r, 0));
}

function selection() {
  return appStore.get().selection;
}

beforeEach(async () => {
  installLocalStorage();
  mock = new MockBridge();
  installBridge(mock);
  sent = [];
  order = [11, 12, 13, 14, 15];
  searchCache.reset(0, 0);
  let generation = 0;
  mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as Record<string, unknown> & { type: string };
    sent.push(c);
    switch (c.type) {
      case "search":
        generation += 1;
        return { type: "search", generation, count: order.length };
      case "index_of": {
        const i = order.indexOf(c.variant_id as number);
        return { type: "index_of", generation, index: i >= 0 ? i : null };
      }
      case "shown_variants":
        return {
          type: "shown_variants",
          generation,
          variant_ids: [...new Set((c.variant_ids as number[]).filter((id) => order.includes(id)))],
        };
      case "range": {
        const start = c.start as number;
        const items = order
          .slice(start, start + (c.len as number))
          .map((id) => mockSummary({ variant_id: id, file_name: `P${id}.jpg` }));
        return { type: "range", generation, start, items };
      }
      case "set_rating":
      case "set_flag":
        return { type: "updated", count: 1 };
      default:
        return { type: "ok" };
    }
  });
  mockCompareCommands(mock);
  appStore.set({
    mode: "loupe",
    filter: {},
    selection: { ids: [12], current: 12, currentIndex: 1, anchorIndex: 1, anchorId: 12 },
    autoAdvance: false,
    dialog: null,
    toasts: [],
    catalog: { catalog_path: "/tmp/c.db", core_settings: { default_utc_offset_minutes: 540 } } as CatalogStatus,
  });
  await actions.runSearch();
  // runSearch は選択を求め直す。
  await settle();
  appStore.set({ selection: { ids: [12], current: 12, currentIndex: 1, anchorIndex: 1, anchorId: 12 } });
  compareStore.set({ pair: { select: null, candidate: null }, active: "select", linked: true });
});

describe("比較表示を始める（C）", () => {
  it("1 枚だけ選んでいれば、次の写真を候補にする。選択は [選択, 候補]、表示中の写真は選択", async () => {
    await openCompare();
    await settle();
    expect(appStore.get().mode).toBe("compare");
    expect(compareStore.get().pair).toEqual({ select: 12, candidate: 13 });
    expect(selection().ids).toEqual([12, 13]);
    expect(selection().current).toBe(12);
  });

  it("2 枚選んでいれば、表示中の写真が選択、もう 1 枚が候補。最後の写真なら前の写真を候補にする", async () => {
    appStore.set({ selection: { ids: [14, 11], current: 11, currentIndex: 0, anchorIndex: 0, anchorId: 11 } });
    await openCompare();
    expect(compareStore.get().pair).toEqual({ select: 11, candidate: 14 });
    actions.setMode("loupe");
    appStore.set({ selection: { ids: [15], current: 15, currentIndex: 4, anchorIndex: 4, anchorId: 15 } });
    await openCompare();
    expect(compareStore.get().pair).toEqual({ select: 15, candidate: 14 });
  });

  it("比較表示の間にもう一度押しても組を作り直さない", async () => {
    await openCompare();
    await setCompareActive("candidate");
    await openCompare();
    expect(compareStore.get().pair).toEqual({ select: 12, candidate: 13 });
    expect(compareStore.get().active).toBe("candidate");
  });
});

describe("比較表示の操作", () => {
  beforeEach(async () => {
    await openCompare();
    await settle();
  });

  it("←・→ は候補を送り、選択の写真を飛ばす。端では動かない", async () => {
    await stepCandidate(1);
    expect(compareStore.get().pair).toEqual({ select: 12, candidate: 14 });
    await stepCandidate(-1);
    await stepCandidate(-1);
    // 13 → 選択の 12 を飛ばして 11。
    expect(compareStore.get().pair.candidate).toBe(11);
    await stepCandidate(-1);
    expect(compareStore.get().pair.candidate).toBe(11);
    expect(selection().ids).toEqual([12, 11]);
  });

  it("↑ は候補を選択にして次の写真を候補に、↓ は入れ替える（作業中の写真は同じまま）", async () => {
    await makeCandidateSelect();
    expect(compareStore.get().pair).toEqual({ select: 13, candidate: 14 });
    expect(selection().current).toBe(13);
    await swapCompare();
    expect(compareStore.get().pair).toEqual({ select: 14, candidate: 13 });
    expect(compareStore.get().active).toBe("candidate");
    expect(selection().current).toBe(13);
  });

  it("作業中の面を変えると、評価はその写真に付き、「自動で次へ」でも進まない", async () => {
    actions.setAutoAdvance(true);
    await setCompareActive("candidate");
    expect(selection().current).toBe(13);
    await actions.setRating(4);
    expect(commands("set_rating").at(-1)).toEqual({ type: "set_rating", variant_ids: [13], rating: 4 });
    expect(selection().current).toBe(13);
    expect(compareStore.get().pair).toEqual({ select: 12, candidate: 13 });
  });

  it("フィルムストリップの普通のクリックは作業中の面の写真を替え、Ctrl / ⌘ + クリックは候補にする", async () => {
    await actions.selectIndex(4);
    await settle();
    expect(compareStore.get().pair).toEqual({ select: 15, candidate: 13 });
    expect(selection().ids).toEqual([15, 13]);
    await actions.selectIndex(0, { toggle: true, keepCurrent: true });
    await settle();
    expect(compareStore.get().pair).toEqual({ select: 15, candidate: 11 });
    expect(selection().current).toBe(15);
  });

  it("ズーム・パンの同期の入り切りを覚える", () => {
    setCompareLinked(false);
    expect(compareStore.get().linked).toBe(false);
    expect(localStorage.getItem("genzo.compare.linked")).toBe("0");
    setCompareLinked(true);
    expect(localStorage.getItem("genzo.compare.linked")).toBe("1");
  });
});

describe("比較表示の面の描画（state/viewRegion.ts）", () => {
  it("面ごとに出力の大きさを求め、範囲を描いて画素を取る。取り消し・やめるときはコアに伝える", async () => {
    expect(await viewOutputSize(1, 13)).toEqual({ variant_id: 13, width: 730, height: 530 });
    const req = { variant_id: 13, space: "output" as const, x: 10, y: 20, width: 40, height: 30, scale: 1 };
    const f = await renderViewRegion(1, req);
    expect(f?.header.pane).toBe(1);
    expect(f?.header.variant_id).toBe(13);
    expect([f?.width, f?.height]).toEqual([40, 30]);
    const call = mock.calls.find((c) => c.command === "view_region_frame");
    expect(call?.args).toEqual({ pane: 1 });
    cancelViewRegions();
    releaseView();
    await settle();
    expect(mock.coreCommands().filter((t) => t === "cancel_view_regions")).toHaveLength(1);
    expect(mock.coreCommands().filter((t) => t === "release_view")).toHaveLength(1);
  });

  it("同じ面の新しい要求に置き換えられた結果は使わない（別の面の結果は使う）", async () => {
    const req = (id: number) => ({ variant_id: id, space: "output" as const, x: 0, y: 0, width: 8, height: 8, scale: 1 });
    const older = renderViewRegion(0, req(12));
    const other = renderViewRegion(1, req(13));
    const newer = renderViewRegion(0, req(14));
    expect(await older).toBeNull();
    expect((await other)?.header.variant_id).toBe(13);
    expect((await newer)?.header.variant_id).toBe(14);
  });
});

describe("選別表示（N）", () => {
  beforeEach(() => {
    appStore.set({ selection: { ids: [11, 13, 15], current: 13, currentIndex: 2, anchorIndex: 2, anchorId: 13 } });
    openSurvey();
  });

  it("作業中の写真を動かし、クリックで選ぶ。選択の集合は変えない", async () => {
    expect(appStore.get().mode).toBe("survey");
    stepSurvey(1);
    await settle();
    expect(selection().current).toBe(15);
    stepSurvey(1);
    await settle();
    expect(selection().current).toBe(15);
    focusSurvey(11);
    await settle();
    expect(selection().current).toBe(11);
    expect(selection().ids).toEqual([11, 13, 15]);
  });

  it("表示から外すと選択から外れ、作業中なら次の写真に移る。最後の 1 枚は外さない", async () => {
    expect(removeSurveyPhoto(13)).toBe(true);
    await settle();
    expect(selection().ids).toEqual([11, 15]);
    expect(selection().current).toBe(15);
    expect(removeSurveyPhoto(15)).toBe(true);
    await settle();
    expect(selection().ids).toEqual([11]);
    expect(removeSurveyPhoto(11)).toBe(false);
  });

  it("評価は作業中の写真に付き、「自動で次へ」でも進まない", async () => {
    actions.setAutoAdvance(true);
    await actions.setFlag(1);
    expect(commands("set_flag").at(-1)).toEqual({ type: "set_flag", variant_ids: [13], flag: 1 });
    expect(selection().current).toBe(13);
  });
});

describe("画面とショートカットの一覧", () => {
  it("比較表示・選別表示を描ける", async () => {
    await openCompare();
    expect(renderToString(createElement(CompareView))).toContain("ズーム・パンを同期");
    appStore.set({ selection: { ids: [11, 12], current: 11, currentIndex: 0, anchorIndex: 0, anchorId: 11 } });
    openSurvey();
    const html = renderToString(createElement(SurveyView));
    expect(html).toContain("2 枚を表示しています");
  });

  it("C・N と比較・選別のキーが一覧に載っている", () => {
    const keys = SHORTCUT_GROUPS.flatMap((g) => g.items.flatMap((i) => i.keys));
    for (const k of ["c", "n", "ArrowUp", "ArrowDown", "/"]) expect(keys).toContain(k);
  });
});
