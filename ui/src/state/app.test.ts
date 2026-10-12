// アプリ全体の状態（選択・カラーラベル・ジョブの完了・取り込み・検索）のテスト（コアのコマンドは MockBridge で
// 差し替える）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { installBridge } from "../api/bridge";
import { ApiError } from "../api/errors";
import { dispatchCoreEventForTest } from "../api/events";
import { MockBridge } from "../api/mock";
import type { CatalogStatus, ColorLabel, DevelopSettings, ErrorInfo, JobInfo, VariantSummary } from "../api/types";
import { PasteSettingsDialog } from "../dialogs/PasteSettingsDialog";
import { actions, appStore, IMPORT_REFRESH_MS, nextColorLabel } from "./app";
import { searchCache, thumbRevs } from "./search";

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
    selection: { ids: [], current: null, currentIndex: null, anchorIndex: null, anchorId: null },
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

describe("サムネイルの新しい rev（performance-1）", () => {
  it("5 万件の thumbnail_updated を受けても状態を作り直さず、知らせは 1 フレームに 1 回にまとめる", async () => {
    await loadResults();
    let notified = 0;
    const off = appStore.subscribe(() => {
      notified++;
    });
    try {
      const state = appStore.get();
      for (let i = 0; i < 50_000; i++) {
        dispatchCoreEventForTest({ type: "thumbnail_updated", variant_id: 10_000 + i, rev: `r${i}` });
      }
      dispatchCoreEventForTest({ type: "thumbnail_updated", variant_id: idAt(3), rev: "fresh" });
      // 受け取るたびに状態（と、全件の rev）を作り直さない。ルーペが読む見え方は、すぐ新しい rev を返す。
      expect(notified).toBe(0);
      expect(appStore.get()).toBe(state);
      expect(appStore.get().thumbRevs[idAt(3)]).toBe("fresh");
      await new Promise((r) => setTimeout(r, 20));
      expect(notified).toBe(1);
      expect(appStore.get().thumbRevsVersion).toBe(thumbRevs.version);
      // 読み込み済みのページの写真の rev は残す。
      expect(appStore.get().thumbRevs[idAt(3)]).toBe("fresh");
    } finally {
      off();
    }
  });
});

describe("取り込み中の読み直しの間引き（performance-4）", () => {
  const running = {
    9: { jobId: 9, kind: "import" as const, state: "running" as const, done: 0, total: 0, message: null, label: "取り込み" },
  };

  beforeEach(async () => {
    // 前のテストの間引きの間隔を数え直す（カタログを閉じると捨てる）。
    actions.catalogClosed();
    appStore.set({ catalog: { catalog_path: "/p/catalog.db" } as unknown as CatalogStatus });
    await loadResults(1);
    appStore.set({ jobs: running });
  });

  it("search_updated は IMPORT_REFRESH_MS に 1 回にまとめる（最初はすぐ、間の分は最後の世代を間隔の終わりに）", () => {
    vi.useFakeTimers();
    const reset = vi.spyOn(searchCache, "reset");
    try {
      dispatchCoreEventForTest({ type: "search_updated", generation: 2, count: 31 });
      expect(searchCache.generation).toBe(2);
      vi.advanceTimersByTime(100);
      dispatchCoreEventForTest({ type: "search_updated", generation: 3, count: 32 });
      vi.advanceTimersByTime(100);
      dispatchCoreEventForTest({ type: "search_updated", generation: 4, count: 33 });
      vi.advanceTimersByTime(IMPORT_REFRESH_MS - 201);
      expect(searchCache.generation).toBe(2);
      expect(appStore.get().search).toEqual({ generation: 2, count: 31 });
      vi.advanceTimersByTime(1);
      expect(searchCache.generation).toBe(4);
      expect(appStore.get().search).toEqual({ generation: 4, count: 33 });
      expect(reset).toHaveBeenCalledTimes(2);
    } finally {
      reset.mockRestore();
    }
  });

  it("取り込みが終わったら、待たせている世代をすぐ使う", () => {
    vi.useFakeTimers();
    dispatchCoreEventForTest({ type: "search_updated", generation: 2, count: 31 });
    dispatchCoreEventForTest({ type: "search_updated", generation: 3, count: 32 });
    expect(searchCache.generation).toBe(2);
    dispatchCoreEventForTest({ type: "job_finished", job_id: 9, kind: "import", state: "succeeded", message: null });
    expect(searchCache.generation).toBe(3);
  });

  it("待たせている間に選ぶと（今の世代の range が stale）、新しい世代にして選ぶ", async () => {
    let coreGeneration = 1;
    overrides.range = (c) => {
      if ((c.generation as number) < coreGeneration) {
        return new ApiError({ kind: "stale", message: "古い世代", user_actionable: false, retryable: true, hint: null });
      }
      const start = c.start as number;
      const items: VariantSummary[] = [];
      for (let i = start; i < Math.min(COUNT, start + (c.len as number)); i++) items.push(summary(i));
      return { type: "range", generation: c.generation, start, items };
    };
    coreGeneration = 2;
    dispatchCoreEventForTest({ type: "search_updated", generation: 2, count: COUNT });
    coreGeneration = 3;
    dispatchCoreEventForTest({ type: "search_updated", generation: 3, count: COUNT });
    expect(searchCache.generation).toBe(2);
    await actions.selectIndex(5);
    expect(searchCache.generation).toBe(3);
    expect(appStore.get().selection).toMatchObject({ current: idAt(5), currentIndex: 5 });
  });

  it("取り込みのカタログの変更で、フォルダ・件数・候補の読み直しの合図を間引く（評価などはすぐ）", () => {
    vi.useFakeTimers();
    const v0 = appStore.get().foldersVersion;
    const d0 = appStore.get().detailsVersion;
    for (let k = 0; k < 5; k++) {
      dispatchCoreEventForTest({ type: "catalog_changed", change: "imported", variant_ids: [idAt(k)], all: false });
      vi.advanceTimersByTime(150);
    }
    expect(appStore.get().foldersVersion).toBe(v0 + 1);
    vi.advanceTimersByTime(IMPORT_REFRESH_MS);
    expect(appStore.get().foldersVersion).toBe(v0 + 2);
    expect(appStore.get().detailsVersion).toBe(d0 + 2);
    dispatchCoreEventForTest({ type: "catalog_changed", change: "marks", variant_ids: [idAt(1)], all: false });
    expect(appStore.get().detailsVersion).toBe(d0 + 3);
  });
});

describe("古い世代の項目を出しているセルのクリック（performance-4）", () => {
  it("今の世代のその位置が別の写真なら、出していた写真の位置を求め直して選ぶ", async () => {
    await loadResults(1);
    // 世代 2 では先頭に 1 枚増え、位置が 1 つずつずれる。
    const idAt2 = (i: number) => (i === 0 ? 999 : idAt(i - 1));
    overrides.range = (c) => {
      const start = c.start as number;
      const items: VariantSummary[] = [];
      for (let i = start; i < Math.min(COUNT, start + (c.len as number)); i++) {
        items.push({ ...summary(i), variant_id: c.generation === 2 ? idAt2(i) : idAt(i) });
      }
      return { type: "range", generation: c.generation, start, items };
    };
    overrides.index_of = (c) => {
      const id = c.variant_id as number;
      return { type: "index_of", generation: searchCache.generation, index: id === 999 ? 0 : id - 100 + 1 };
    };
    searchCache.setView("grid", 0, COUNT);
    try {
      dispatchCoreEventForTest({ type: "search_updated", generation: searchCache.generation + 1, count: COUNT });
      // セルは、新しい世代のページが届くまで古い世代の項目（位置 5 は 105）を出す。
      expect(searchCache.get(5)?.variant_id).toBe(idAt(5));
      await actions.selectIndex(5, { expect: idAt(5) });
      expect(appStore.get().selection).toMatchObject({ current: idAt(5), currentIndex: 6 });
    } finally {
      searchCache.clearView("grid");
    }
  });
});

// ---------------------------------------------------------------------------
// 検証で見つかった選択の不具合（M1〜M5）
// ---------------------------------------------------------------------------

/** 世代 2 では先頭に 1 枚（999）増え、位置が 1 つずつずれる（世代 1 は idAt のまま）。 */
function shiftInGeneration2(): void {
  const idAt2 = (i: number) => (i === 0 ? 999 : idAt(i - 1));
  overrides.range = (c) => {
    const start = c.start as number;
    const items: VariantSummary[] = [];
    for (let i = start; i < Math.min(COUNT, start + (c.len as number)); i++) {
      items.push({ ...summary(i), variant_id: c.generation === 2 ? idAt2(i) : idAt(i) });
    }
    return { type: "range", generation: c.generation, start, items };
  };
  overrides.index_of = (c) => {
    const id = c.variant_id as number;
    const index = searchCache.generation === 2 ? (id === 999 ? 0 : id - 100 + 1) : id - 100;
    return { type: "index_of", generation: searchCache.generation, index };
  };
}

describe("Shift + クリックは、セルに出していた写真と範囲の起点の写真で選ぶ（M1）", () => {
  afterEach(() => {
    searchCache.clearView("grid");
    searchCache.clearView("filmstrip");
  });

  it("グリッド: 古い世代の項目を出している間でも、クリックした写真と起点の写真の間を選ぶ", async () => {
    await loadResults(1);
    shiftInGeneration2();
    searchCache.setView("grid", 0, COUNT);
    await actions.selectIndex(2);
    await actions.selectIndex(4, { extend: true });
    expect(appStore.get().selection).toMatchObject({ current: idAt(4), anchorIndex: 2 });
    dispatchCoreEventForTest({ type: "search_updated", generation: 2, count: COUNT });
    await flush();
    // セルは位置 10 に 110（古い世代）を出している。
    expect(searchCache.get(10)?.variant_id).toBe(idAt(10));
    await actions.selectIndex(10, { extend: true, expect: idAt(10) });
    const sel = appStore.get().selection;
    // 起点の 102 からクリックした 110 まで（今の世代の位置 3〜11）。
    expect([...sel.ids].sort((a, b) => a - b)).toEqual([2, 3, 4, 5, 6, 7, 8, 9, 10].map(idAt));
    expect(sel).toMatchObject({ current: idAt(10), currentIndex: 11, anchorId: idAt(2), anchorIndex: 3 });
  });

  it("フィルムストリップ: 起点（表示中の写真）の位置を今の世代で求める", async () => {
    await loadResults(1);
    shiftInGeneration2();
    appStore.set({ mode: "develop" });
    await show(5);
    searchCache.setView("filmstrip", 0, COUNT);
    // 表示中の写真の位置の求め直し（relocateSelection）がまだ届いていない間に、Shift + クリックする。
    mock.delay = (c) => {
      const cmd = c.args?.command as { type: string; variant_id?: number };
      return cmd.type === "index_of" && cmd.variant_id === idAt(5) ? 40 : 0;
    };
    dispatchCoreEventForTest({ type: "search_updated", generation: 2, count: COUNT });
    expect(appStore.get().selection.currentIndex).toBe(5);
    await actions.selectIndex(10, { extend: true, keepCurrent: true, expect: idAt(10) });
    const sel = appStore.get().selection;
    expect(sel.current).toBe(idAt(5));
    expect([...sel.ids].sort((a, b) => a - b)).toEqual([5, 6, 7, 8, 9, 10].map(idAt));
    expect(actions.pasteTargets().sort((a, b) => a - b)).toEqual([5, 6, 7, 8, 9, 10].map(idAt));
    await new Promise((r) => setTimeout(r, 60));
  });
});

describe("選べなかったときはルーペを開かない（M3）", () => {
  it("出していた写真が結果にないと selectIndex は false を返し、ダブルクリックでもルーペを開かない", async () => {
    await loadResults(1);
    await show(4);
    overrides.index_of = () => ({ type: "index_of", generation: searchCache.generation, index: null });
    expect(await actions.selectIndex(2, { expect: 777 })).toBe(false);
    expect(await actions.openInLoupe(2, 777)).toBe(false);
    expect(appStore.get().mode).toBe("grid");
    expect(appStore.get().selection.current).toBe(idAt(4));
    expect(await actions.openInLoupe(2, idAt(2))).toBe(true);
    expect(appStore.get().mode).toBe("loupe");
    expect(appStore.get().selection.current).toBe(idAt(2));
  });
});

describe("位置の求め直しと選び直しの競合（M4）", () => {
  it("位置を問い合わせている間に選び直したら、前の写真の位置を書かない", async () => {
    await loadResults(1);
    await show(3);
    mock.delay = (c) => ((c.args?.command as { type: string }).type === "index_of" ? 30 : 0);
    dispatchCoreEventForTest({ type: "search_updated", generation: 2, count: COUNT });
    await actions.selectIndex(20);
    await new Promise((r) => setTimeout(r, 50));
    expect(appStore.get().selection).toMatchObject({ current: idAt(20), currentIndex: 20 });
  });
});

describe("選んだ写真の種類（M5）", () => {
  const BIG = 20_000;
  const VIDEO = 3;
  const settings = { version: 1 } as unknown as DevelopSettings;

  beforeEach(async () => {
    overrides.range = (c) => {
      const start = c.start as number;
      const items: VariantSummary[] = [];
      for (let i = start; i < Math.min(BIG, start + (c.len as number)); i++) {
        items.push({ ...summary(Math.min(i, COUNT - 1)), variant_id: idAt(i), kind: i === VIDEO ? "video" : "photo", file_name: `${i}.MOV` });
      }
      return { type: "range", generation: c.generation, start, items };
    };
    searchCache.reset(20, BIG);
    appStore.set({ search: { generation: 20, count: BIG }, clipboard: null });
    // 動画（位置 3）と写真（位置 4）を選び、80 ページ先までスクロールする（位置 3 のページは上限で捨てる）。
    await actions.selectIndex(VIDEO);
    await actions.selectIndex(4, { toggle: true });
    searchCache.setView("grid", 80 * 200, 80 * 200 + 100);
    await searchCache.ensure(10 * 200, 81 * 200);
  });

  afterEach(() => {
    searchCache.clearView("grid");
  });

  it("ページが捨てられても、選んだ動画は動画と分かり、設定をコピーしない", async () => {
    expect(searchCache.getLoadedById(idAt(VIDEO))).toBeUndefined();
    appStore.set((s) => ({ selection: { ...s.selection, current: idAt(VIDEO) } }));
    expect(await actions.kindOf(idAt(VIDEO))).toBe("video");
    expect(await actions.copySettingsOf(idAt(VIDEO), null)).toBe(false);
    expect(appStore.get().clipboard).toBeNull();
    expect(appStore.get().toasts.map((t) => t.message)).toContain("動画の設定はコピーできません");
    expect(mock.coreCommands()).not.toContain("copy_settings");
    expect(mock.coreCommands()).not.toContain("variant_details");
  });

  it("読み込み済みでも選んでもいなければ、コアに種類を問い合わせる", async () => {
    overrides.variant_details = () => ({ type: "details", summary: { variant_id: 900_000, kind: "video" } });
    expect(await actions.kindOf(900_000)).toBe("video");
    expect(mock.coreCommands()).toContain("variant_details");
  });

  it("貼り付けのダイアログは、ページが捨てられた選択中の動画を貼り付け先から除く", () => {
    appStore.set({ clipboard: { settings, from: idAt(4) } });
    const html = renderToString(createElement(PasteSettingsDialog, { targets: [idAt(VIDEO), idAt(4)] })).replace(
      /<!-- -->/g,
      "",
    );
    expect(html).toContain("動画 1 本は除きます");
    expect(html).toContain("コピー元: 4.MOV");
  });
});
