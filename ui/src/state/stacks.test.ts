// スタック・仮想コピーの画面の操作（state/stacks.ts）のテスト（コアのコマンドは MockBridge で差し替える）と、
// 部品が代表的な状態で例外なく描けることの確認（react-dom/server。useEffect は動かない）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import * as core from "../api/core";
import { MOCK_COLLECTIONS, MOCK_STACK, MOCK_STACK_TOP, MOCK_VIRTUAL_COPY, MockBridge, mockSummary } from "../api/mock";
import type { CatalogStatus, StackInfo } from "../api/types";
import { AutoStackDialog, VirtualCopyNameDialog } from "../dialogs/StackDialogs";
import { StackBadgeButton } from "../library/StackBadge";
import { actions, appStore } from "./app";
import { searchCache } from "./search";
import {
  addTargetsToStack,
  createVirtualCopyOf,
  groupIntoStack,
  runPhotoMenu,
  stackToAddTo,
  toggleStack,
  unstackTargets,
} from "./stacks";

let mock: MockBridge;
let sent: Record<string, unknown>[];
/** スタック（variant_stacks の応答。set_stacks_collapsed で折りたたみを変える）。 */
let stacks: StackInfo[];
/** 検索し直した後の結果の位置（index_of・shown_variants・range の応答。id → 位置。なければ結果にない）。 */
let positions: Map<number, number>;
/** 検索し直した後の、折りたたんだスタックに隠れた写真の数（search の応答）。 */
let hidden: number;

function commands(type: string): Record<string, unknown>[] {
  return sent.filter((c) => c.type === type);
}

beforeEach(() => {
  mock = new MockBridge();
  installBridge(mock);
  sent = [];
  // 検索結果の世代を、この応答の世代（1 から）に合わせる。
  searchCache.reset(0, 0);
  stacks = [];
  hidden = 0;
  positions = new Map([
    [1, 0],
    [2, 1],
    [3, 2],
    [9, 3],
  ]);
  let generation = 0;
  mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as Record<string, unknown> & { type: string };
    sent.push(c);
    switch (c.type) {
      case "create_stack":
        return { type: "stack", ...MOCK_STACK };
      case "variant_stacks": {
        const ids = c.variant_ids as number[];
        return { type: "stacks", stacks: stacks.filter((st) => ids.some((id) => st.members.includes(id))) };
      }
      case "set_stacks_collapsed": {
        const which = c.stack_ids as number[] | null;
        stacks = stacks.map((st) =>
          which === null || which.includes(st.stack_id) ? { ...st, collapsed: c.collapsed as boolean } : st,
        );
        return { type: "updated", count: 1 };
      }
      case "unstack":
      case "set_rating":
        return { type: "updated", count: 1 };
      case "add_to_stack":
        return { type: "updated", count: (c.variant_ids as number[]).length };
      case "auto_stack":
        stacks = [{ stack_id: 7, top_variant_id: 1, collapsed: true, members: [1, 2, 3] }];
        return { type: "auto_stack", stacks: 1, photos: 3, considered: 4, skipped_in_stacks: 0, skipped_without_time: 0, dry_run: false };
      case "search":
        generation += 1;
        return { type: "search", generation, count: positions.size, hidden };
      case "index_of": {
        const id = c.variant_id as number;
        return { type: "index_of", generation, index: positions.get(id) ?? null };
      }
      case "shown_variants": {
        const ids = c.variant_ids as number[];
        return { type: "shown_variants", generation, variant_ids: [...new Set(ids.filter((id) => positions.has(id)))] };
      }
      case "range": {
        // 位置の表（positions）の順に並べた写真の項目。
        const order = [...positions.entries()].sort((a, b) => a[1] - b[1]).map(([id]) => id);
        const start = c.start as number;
        const items = order
          .slice(start, start + (c.len as number))
          .map((id) => mockSummary({ variant_id: id, file_name: `P${id}.jpg` }));
        return { type: "range", generation, start, items };
      }
      case "create_virtual_copy":
        positions.set(9, 3);
        return { type: "variant_created", variant_id: 9 };
      case "collections":
        return { type: "collections", collections: MOCK_COLLECTIONS };
      case "add_to_collection":
        return { type: "updated", count: 1 };
      default:
        return { type: "ok" };
    }
  });
  appStore.set({
    mode: "grid",
    filter: {},
    selection: { ids: [1, 2, 3], current: 2, currentIndex: 1, anchorIndex: 1, anchorId: 2 },
    dialog: null,
    toasts: [],
    catalog: { catalog_path: "/tmp/c.db", core_settings: { default_utc_offset_minutes: 540 } } as CatalogStatus,
  });
});

describe("スタックにまとめる（Ctrl / ⌘ + G）", () => {
  it("選んだ写真を表示中の写真を一番上にしてまとめ、検索し直して一番上を選ぶ", async () => {
    await groupIntoStack();
    expect(commands("create_stack")).toEqual([{ type: "create_stack", variant_ids: [1, 2, 3], top: 2 }]);
    expect(commands("search")).toHaveLength(1);
    const sel = appStore.get().selection;
    expect(sel.current).toBe(MOCK_STACK.top_variant_id);
    expect(sel.ids).toEqual([MOCK_STACK.top_variant_id]);
    expect(sel.currentIndex).toBe(1);
  });

  it("1 枚だけなら作らずに知らせる", async () => {
    appStore.set((s) => ({ selection: { ...s.selection, ids: [2] } }));
    await groupIntoStack();
    expect(commands("create_stack")).toHaveLength(0);
    expect(appStore.get().toasts.at(-1)?.message).toContain("2 枚以上");
  });

  it("解除は選んだ写真のスタックを解く", async () => {
    await unstackTargets();
    expect(commands("unstack")).toEqual([{ type: "unstack", variant_ids: [1, 2, 3] }]);
  });
});

/** 折りたたんだ後の結果（スタック MOCK_STACK の代表 2 と、スタックでない 9。隠れた写真は 1 と 3）。 */
function collapsedResult(): void {
  positions = new Map([
    [2, 0],
    [9, 1],
  ]);
  hidden = 2;
}

describe("スタックに足す", () => {
  it("表示中の写真のスタックに、一緒に選んだほかの写真を足す（作り直さない）", async () => {
    stacks = [MOCK_STACK];
    appStore.set((s) => ({ selection: { ...s.selection, ids: [2, 9], current: 2, currentIndex: 1 } }));
    await addTargetsToStack();
    expect(commands("add_to_stack")).toEqual([{ type: "add_to_stack", stack_id: MOCK_STACK.stack_id, variant_ids: [9] }]);
    expect(commands("create_stack")).toHaveLength(0);
    expect(commands("search")).toHaveLength(1);
    expect(appStore.get().toasts.at(-1)?.message).toBe("1 枚をスタックに足しました（折りたたみ中。S で展開）");
  });

  it("表示中の写真がスタックでなければ、選んだ写真のただ 1 つのスタックに足す。決められなければ知らせる", async () => {
    const other: StackInfo = { stack_id: 8, top_variant_id: 9, collapsed: false, members: [9, 10] };
    expect(stackToAddTo([MOCK_STACK], 4)).toBe(MOCK_STACK);
    expect(stackToAddTo([MOCK_STACK, other], 9)).toBe(other);
    expect(stackToAddTo([MOCK_STACK, other], 4)).toBeNull();
    expect(stackToAddTo([], 4)).toBeNull();
    // 1 枚だけ・スタックの写真がないときは足さない。
    appStore.set((s) => ({ selection: { ...s.selection, ids: [4], current: 4 } }));
    await addTargetsToStack();
    expect(appStore.get().toasts.at(-1)?.message).toContain("一緒に選んでください");
    appStore.set((s) => ({ selection: { ...s.selection, ids: [4, 9], current: 4 } }));
    await addTargetsToStack();
    expect(appStore.get().toasts.at(-1)?.message).toContain("スタックの写真がありません");
    expect(commands("add_to_stack")).toHaveLength(0);
  });

  it("メニューから「選んだ写真をスタックに足す」", async () => {
    stacks = [MOCK_STACK];
    appStore.set((s) => ({ selection: { ...s.selection, ids: [9, 2], current: 9, currentIndex: 3 } }));
    await runPhotoMenu("add_to_stack");
    expect(commands("add_to_stack")).toEqual([{ type: "add_to_stack", stack_id: MOCK_STACK.stack_id, variant_ids: [9] }]);
  });
});

describe("折りたたみ（S）", () => {
  it("展開しているスタックを折りたたむと、隠れる写真を選んでいれば一番上を選ぶ", async () => {
    stacks = [{ ...MOCK_STACK, collapsed: false }];
    collapsedResult();
    appStore.set((s) => ({ selection: { ...s.selection, ids: [3], current: 3, currentIndex: 2 } }));
    await toggleStack();
    expect(commands("set_stacks_collapsed")).toEqual([
      { type: "set_stacks_collapsed", stack_ids: [MOCK_STACK.stack_id], collapsed: true },
    ]);
    expect(appStore.get().selection.current).toBe(MOCK_STACK.top_variant_id);
  });

  it("ルーペで、隠れる写真を表示していたら、検索し直した後も近くの別の写真ではなく一番上を選ぶ", async () => {
    // 折りたたんだ後の結果では 3 は隠れる（index_of は null）。ルーペでは、選択の追跡が近い位置の写真に移る。
    stacks = [{ ...MOCK_STACK, collapsed: false }];
    collapsedResult();
    appStore.set((s) => ({ mode: "loupe", selection: { ...s.selection, ids: [3], current: 3, currentIndex: 1 } }));
    await toggleStack();
    expect(appStore.get().selection.current).toBe(MOCK_STACK.top_variant_id);
  });

  it("折りたたんだスタックを展開しても選択は変えない。スタックでなければ何もしない", async () => {
    stacks = [MOCK_STACK];
    await toggleStack(2);
    expect(commands("set_stacks_collapsed")[0]).toMatchObject({ collapsed: false });
    expect(appStore.get().selection.current).toBe(2);
    stacks = [];
    sent = [];
    await toggleStack(1);
    expect(commands("set_stacks_collapsed")).toHaveLength(0);
  });

  it("すべて折りたたむときも、隠れる写真を表示していたら一番上を選ぶ", async () => {
    stacks = [{ ...MOCK_STACK, collapsed: false }];
    collapsedResult();
    appStore.set((s) => ({ mode: "loupe", selection: { ...s.selection, ids: [1], current: 1, currentIndex: 1 } }));
    await runPhotoMenu("collapse_all");
    expect(commands("set_stacks_collapsed")).toEqual([{ type: "set_stacks_collapsed", stack_ids: null, collapsed: true }]);
    expect(appStore.get().selection.current).toBe(MOCK_STACK.top_variant_id);
  });

  it("複数選択で折りたたむと、隠れた写真を選択から外す（評価などは表示している写真だけに付く）", async () => {
    stacks = [{ ...MOCK_STACK, collapsed: false }];
    collapsedResult();
    // 展開中のスタック（[2, 1, 3]。一番上 2）を 3 枚とも選び、表示中は一番上。
    await toggleStack();
    const sel = appStore.get().selection;
    expect(sel.ids).toEqual([2]);
    expect(sel.current).toBe(2);
    expect(actions.targets()).toEqual([2]);
    await actions.setRating(3);
    expect(commands("set_rating")).toEqual([{ type: "set_rating", variant_ids: [2], rating: 3 }]);
  });

  it("隠れる写真を表示していて、ほかにも選んでいれば、結果に出ている写真の選択は残して代表を選ぶ", async () => {
    stacks = [{ ...MOCK_STACK, collapsed: false }];
    collapsedResult();
    appStore.set((s) => ({ selection: { ...s.selection, ids: [1, 3, 9], current: 3, currentIndex: 2 } }));
    await toggleStack();
    const sel = appStore.get().selection;
    expect(sel.current).toBe(2);
    expect(sel.currentIndex).toBe(0);
    expect(sel.ids).toEqual([9, 2]);
  });

  it("自動スタックの後も、隠れた写真を選択から外し、表示中の写真が隠れたら代表を選ぶ", async () => {
    // すべてを選んだ（Ctrl / ⌘ + A）状態で、1・2・3 を 1 つのスタック（一番上 1）にまとめる。
    positions = new Map([
      [1, 0],
      [9, 1],
    ]);
    hidden = 2;
    appStore.set((s) => ({ selection: { ...s.selection, ids: [1, 2, 3, 9], current: 3, currentIndex: 2 } }));
    await core.autoStack({}, 2, false);
    await actions.runSearch();
    const sel = appStore.get().selection;
    expect(sel.current).toBe(1);
    expect(actions.targets()).toEqual([1, 9]);
  });

  it("メニューから「すべてのスタックを展開」", async () => {
    await runPhotoMenu("expand_all");
    expect(commands("set_stacks_collapsed")).toEqual([{ type: "set_stacks_collapsed", stack_ids: null, collapsed: false }]);
    await runPhotoMenu("auto_stack");
    expect(appStore.get().dialog).toEqual({ kind: "auto_stack" });
  });
});

describe("仮想コピーを作る（Ctrl / ⌘ + '）", () => {
  it("手動のコレクションを表示していれば、そのコレクションにも入れて、作った仮想コピーを選ぶ", async () => {
    appStore.set({ filter: { collection_id: 13 } });
    const id = await createVirtualCopyOf();
    expect(id).toBe(9);
    expect(commands("create_virtual_copy")).toEqual([{ type: "create_virtual_copy", variant_id: 2, name: null }]);
    expect(commands("add_to_collection")).toEqual([{ type: "add_to_collection", collection_id: 13, variant_ids: [9] }]);
    expect(appStore.get().selection.current).toBe(9);
    expect(appStore.get().toasts.at(-1)?.message).toBe("仮想コピーを作りました");
  });

  it("スマートコレクションには入れない。結果に出なければそう知らせる", async () => {
    appStore.set({ filter: { collection_id: 12 } });
    positions.delete(9);
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as Record<string, unknown> & { type: string };
      sent.push(c);
      switch (c.type) {
        case "create_virtual_copy":
          return { type: "variant_created", variant_id: 9 };
        case "collections":
          return { type: "collections", collections: MOCK_COLLECTIONS };
        case "search":
          return { type: "search", generation: 50, count: 3 };
        case "index_of":
          return { type: "index_of", generation: 50, index: null };
        default:
          return { type: "ok" };
      }
    });
    await createVirtualCopyOf(1);
    expect(commands("add_to_collection")).toHaveLength(0);
    expect(appStore.get().toasts.at(-1)?.message).toContain("今の絞り込みでは表示されません");
  });
});

describe("部品の描画", () => {
  it("スタックの印・名前の変更・自動スタックのダイアログが例外なく描ける", () => {
    const badge = renderToString(createElement(StackBadgeButton, { id: 2, badge: MOCK_STACK_TOP.stack!, wide: true }));
    expect(badge).toContain("スタック 3");
    expect(badge).toContain("aria-expanded=\"false\"");
    const expanded = renderToString(createElement(StackBadgeButton, { id: 4, badge: MOCK_VIRTUAL_COPY.stack!, wide: false }));
    expect(expanded).toContain("2/2");
    expect(renderToString(createElement(VirtualCopyNameDialog, { variantId: 4 }))).toContain("仮想コピーの名前を変える");
    const auto = renderToString(createElement(AutoStackDialog));
    expect(auto).toContain("撮影時刻でスタック");
    expect(auto).toContain("間隔（秒）");
  });
});
