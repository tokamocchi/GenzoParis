// アプリ全体の状態（選択・カラーラベル・ジョブの完了・取り込み・検索）のテスト（コアのコマンドは MockBridge で
// 差し替える）。

import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { installBridge } from "../api/bridge";
import { ApiError } from "../api/errors";
import { dispatchCoreEventForTest } from "../api/events";
import { MockBridge } from "../api/mock";
import type { CatalogStatus, ColorLabel, ErrorInfo, JobInfo, VariantSummary } from "../api/types";
import { actions, appStore, nextColorLabel } from "./app";
import { searchCache } from "./search";

/** 検索結果の件数（variant_id は 100 + 位置）。 */
const COUNT = 30;
const idAt = (i: number) => 100 + i;

let mock: MockBridge;
let labels: (ColorLabel | null)[];
/** core_execute の type ごとの応答の差し替え（なければ既定の応答）。 */
let overrides: Record<string, (c: Record<string, unknown>) => unknown>;

function summary(i: number): VariantSummary {
  return { variant_id: idAt(i), kind: "photo", file_name: `${i}.ARW`, color_label: labels[i] } as VariantSummary;
}

const closedError: ErrorInfo = {
  kind: "internal",
  message: "ジョブの結果を JSON にできません",
  user_actionable: false,
  retryable: false,
  hint: null,
};

/** 検索結果を世代 `generation` で読み込み済みにする。 */
async function loadResults(generation = 1): Promise<void> {
  searchCache.reset(generation, COUNT);
  appStore.set({ search: { generation, count: COUNT } });
  await searchCache.ensure(0, COUNT);
}

/** 表示中の写真を位置 `index` にする（選択はその 1 枚）。 */
async function show(index: number): Promise<void> {
  await actions.selectIndex(index);
}

/** 待っている Promise の続きを進める。 */
async function flush(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

beforeAll(() => {
  actions.installEventHandlers();
});

beforeEach(() => {
  mock = new MockBridge();
  installBridge(mock);
  labels = Array.from({ length: COUNT }, () => null);
  overrides = {};
  mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as Record<string, unknown> & { type: string };
    const o = overrides[c.type];
    if (o) return o(c);
    switch (c.type) {
      case "range": {
        const start = c.start as number;
        const items: VariantSummary[] = [];
        for (let i = start; i < Math.min(COUNT, start + (c.len as number)); i++) items.push(summary(i));
        return { type: "range", generation: c.generation, start, items };
      }
      case "index_of":
        return { type: "index_of", generation: searchCache.generation, index: (c.variant_id as number) - 100 };
      case "set_color_label":
      case "set_rating":
        if (c.type === "set_color_label") {
          for (const id of c.variant_ids as number[]) labels[id - 100] = c.label as ColorLabel | null;
        }
        return { type: "updated", count: (c.variant_ids as number[]).length };
      case "search":
        return { type: "search", generation: searchCache.generation + 1, count: COUNT };
      default:
        return { type: "ok" };
    }
  });
  appStore.set({
    mode: "grid",
    autoAdvance: false,
    selection: { ids: [], current: null, currentIndex: null, anchorIndex: null },
    jobs: {},
    toasts: [],
    dialog: null,
    catalog: { catalog_path: "/p/catalog.db" } as unknown as CatalogStatus,
  });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("フィルムストリップの選択（表示中の写真を変えない）", () => {
  beforeEach(async () => {
    await loadResults();
    appStore.set({ mode: "develop" });
    await show(5);
  });

  it("Ctrl / ⌘ + クリックは選択に足すだけで、表示中の写真（current）は変わらない", async () => {
    await actions.selectIndex(8, { toggle: true, keepCurrent: true });
    const sel = appStore.get().selection;
    expect(sel.current).toBe(idAt(5));
    expect(sel.currentIndex).toBe(5);
    expect(sel.ids).toEqual([idAt(5), idAt(8)]);
    // 同期のコピー元（表示中の写真）が貼り付け先に入らない: 貼り付けの対象は表示中の写真と選んだ写真。
    expect(actions.pasteTargets()).toEqual([idAt(5), idAt(8)]);
  });

  it("もう一度 Ctrl / ⌘ + クリックすると外れる。表示中の写真は外さない", async () => {
    await actions.selectIndex(8, { toggle: true, keepCurrent: true });
    await actions.selectIndex(8, { toggle: true, keepCurrent: true });
    expect(appStore.get().selection.ids).toEqual([idAt(5)]);
    await actions.selectIndex(5, { toggle: true, keepCurrent: true });
    expect(appStore.get().selection).toMatchObject({ ids: [idAt(5)], current: idAt(5) });
  });

  it("Shift + クリックは、表示中の写真からの範囲を選択に足す（表示中の写真は変わらない）", async () => {
    await actions.selectIndex(9, { toggle: true, keepCurrent: true });
    await actions.selectIndex(2, { extend: true, keepCurrent: true });
    const sel = appStore.get().selection;
    expect(sel.current).toBe(idAt(5));
    expect(sel.currentIndex).toBe(5);
    expect([...sel.ids].sort((a, b) => a - b)).toEqual([2, 3, 4, 5, 9].map(idAt));
  });

  it("修飾キーなしのクリックは、その写真を表示し、選択をその 1 枚にする", async () => {
    await actions.selectIndex(8, { toggle: true, keepCurrent: true });
    await actions.selectIndex(11, { keepCurrent: true });
    expect(appStore.get().selection).toMatchObject({ ids: [idAt(11)], current: idAt(11), currentIndex: 11 });
  });

  it("グリッドの Ctrl / ⌘ + クリックは、従来どおり操作の対象も移る", async () => {
    appStore.set({ mode: "grid" });
    await actions.selectIndex(8, { toggle: true });
    expect(appStore.get().selection).toMatchObject({ ids: [idAt(5), idAt(8)], current: idAt(8) });
    expect(actions.pasteTargets()).toEqual(actions.targets());
  });
});

describe("カラーラベル（6〜9）", () => {
  beforeEach(async () => {
    await loadResults();
  });

  it("すべて同じラベルなら外し、混在・読み込んでいないものがあれば付ける", () => {
    expect(nextColorLabel("red", ["red", "red"])).toBeNull();
    expect(nextColorLabel("red", ["red", null])).toBe("red");
    expect(nextColorLabel("red", ["red", "blue"])).toBe("red");
    expect(nextColorLabel("red", ["red", undefined])).toBe("red");
    expect(nextColorLabel("red", [])).toBe("red");
  });

  it("同じキーで外れる（ルーペ）", async () => {
    appStore.set({ mode: "loupe" });
    labels[3] = "red";
    await loadResults(2);
    await show(3);
    await actions.toggleColorLabel("red");
    expect(labels[3]).toBeNull();
    await actions.toggleColorLabel("yellow");
    expect(labels[3]).toBe("yellow");
  });

  it("グリッドで複数を選び、混在するときは付ける", async () => {
    labels[1] = "green";
    await loadResults(2);
    await show(1);
    await actions.selectIndex(2, { toggle: true });
    await actions.toggleColorLabel("green");
    expect([labels[1], labels[2]]).toEqual(["green", "green"]);
  });

  it("「自動で次へ」がオンなら、付けた後・外した後に次の写真へ進む", async () => {
    appStore.set({ mode: "loupe", autoAdvance: true });
    await show(3);
    await actions.toggleColorLabel("blue");
    expect(labels[3]).toBe("blue");
    expect(appStore.get().selection.currentIndex).toBe(4);
    // 右のパネルのボタン（setColorLabel）も同じ。
    await actions.setColorLabel(null);
    expect(appStore.get().selection.currentIndex).toBe(5);
  });
});

describe("ジョブの完了", () => {
  it("ジョブの結果を読めなくても、イベントの状態で待ちを終わらせ、一覧から消す", async () => {
    vi.useFakeTimers();
    overrides.job = (c) => {
      // waitJob の最初の問い合わせ（実行中）には答え、終わった後は読めない。
      if (appStore.get().jobs[c.job_id as number]?.state === "running") {
        return { type: "job", job_id: c.job_id, kind: "import", state: "running", done: 1, total: 4, result: null, error: null };
      }
      return new ApiError(closedError);
    };
    actions.trackJob(7, "import", "取り込み: DCIM");
    appStore.set((s) => ({ jobs: { ...s.jobs, 7: { ...s.jobs[7], done: 3, total: 4 } } }));
    const waiting = actions.waitJob(7);
    await flush();
    dispatchCoreEventForTest({ type: "job_finished", job_id: 7, kind: "import", state: "succeeded", message: null });
    const info: JobInfo = await waiting;
    expect(info).toEqual({
      job_id: 7,
      kind: "import",
      state: "succeeded",
      done: 3,
      total: 4,
      result: null,
      error: closedError,
    });
    expect(appStore.get().jobs[7]?.state).toBe("succeeded");
    vi.advanceTimersByTime(6000);
    expect(appStore.get().jobs[7]).toBeUndefined();
  });
});

describe("取り込み", () => {
  beforeEach(async () => {
    await loadResults();
  });

  it("取り込みの途中でカタログを閉じたら、検索し直さず、残りのフォルダを送らない", async () => {
    let nextJob = 20;
    overrides.import_folder = () => ({ type: "job_started", job_id: nextJob++ });
    overrides.job = (c) => ({
      type: "job",
      job_id: c.job_id,
      kind: "import",
      state: "running",
      done: 0,
      total: 0,
      result: null,
      error: null,
    });
    const done = actions.importFolders(["/p/a", "/p/b", "/p/c"], true);
    await flush();
    expect(mock.coreCommands().filter((t) => t === "import_folder")).toHaveLength(1);
    // 切り替え・復元と同じく、閉じた後にすぐ別のカタログを開く。
    actions.catalogClosed();
    appStore.set({ catalog: { catalog_path: "/p/other.db" } as unknown as CatalogStatus });
    await done;
    expect(mock.coreCommands().filter((t) => t === "import_folder")).toHaveLength(1);
    expect(mock.coreCommands()).not.toContain("search");
    expect(appStore.get().toasts.map((t) => t.message)).toContain(
      "カタログを閉じたため、残りの 2 個のフォルダは取り込みませんでした",
    );
  });

  it("取り込みが取り消されたら、残りのフォルダを送らない", async () => {
    let nextJob = 30;
    overrides.import_folder = () => ({ type: "job_started", job_id: nextJob++ });
    overrides.job = (c) => ({
      type: "job",
      job_id: c.job_id,
      kind: "import",
      state: "cancelled",
      done: 0,
      total: 0,
      result: null,
      error: null,
    });
    await actions.importFolders(["/p/a", "/p/b"], true);
    expect(mock.coreCommands().filter((t) => t === "import_folder")).toHaveLength(1);
  });

  it("すべて終われば、フォルダを順に取り込む", async () => {
    let nextJob = 40;
    overrides.import_folder = () => ({ type: "job_started", job_id: nextJob++ });
    overrides.job = (c) => ({
      type: "job",
      job_id: c.job_id,
      kind: "import",
      state: "succeeded",
      done: 1,
      total: 1,
      result: null,
      error: null,
    });
    await actions.importFolders(["/p/a", "/p/b", "/p/c"], true);
    expect(mock.coreCommands().filter((t) => t === "import_folder")).toHaveLength(3);
  });
});

describe("検索", () => {
  it("応答より先に新しい世代の search_updated が届いていれば、古い世代に戻さない", async () => {
    await loadResults(5);
    overrides.search = () => {
      // 応答（世代 6）より先に、世代 7 のイベントが届く。
      dispatchCoreEventForTest({ type: "search_updated", generation: 7, count: 12 });
      return { type: "search", generation: 6, count: COUNT };
    };
    await actions.runSearch();
    expect(searchCache.generation).toBe(7);
    expect(searchCache.count).toBe(12);
    expect(appStore.get().search).toEqual({ generation: 7, count: 12 });
    expect(appStore.get().searching).toBe(false);
  });

  it("新しい世代の応答なら、その世代にする", async () => {
    await loadResults(5);
    await actions.runSearch();
    expect(searchCache.generation).toBe(6);
    expect(appStore.get().search).toEqual({ generation: 6, count: COUNT });
  });
});
