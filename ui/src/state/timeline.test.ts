// タイムライン（ORG-01）の表示の状態（state/timeline.ts）と、撮影日時の修正（LIB-16）の入口のテスト（コアの
// コマンドは MockBridge で差し替える）。ダイアログが代表的な状態で例外なく描けることも確かめる
// （react-dom/server。useEffect は動かない）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MOCK_TIMELINE, MockBridge } from "../api/mock";
import { CaptureTimeDialog } from "../dialogs/CaptureTimeDialog";
import { SHORTCUT_GROUPS } from "../dialogs/shortcutsData";
import { photoMenuEntries } from "../library/stackLogic";
import { appStore, DEFAULT_SORT } from "./app";
import { openCaptureTimeDialog, runPhotoMenu } from "./stacks";
import {
  loadTimeline,
  resetTimelineForTest,
  setTimelineEnabled,
  setTimelineGap,
  timelineActive,
  timelineStore,
  toggleTimeline,
} from "./timeline";

let mock: MockBridge;
let sent: Record<string, unknown>[];
/** timeline の応答（関数ならコマンドから作る。Error なら失敗）。 */
let timelineReply: (c: Record<string, unknown>) => unknown;

/** テストの実行環境（node）には localStorage がないので、Map で代わりを置く（organize.test.ts と同じ）。 */
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

beforeEach(() => {
  installLocalStorage();
  mock = new MockBridge();
  installBridge(mock);
  sent = [];
  resetTimelineForTest();
  appStore.set({ sort: DEFAULT_SORT, dialog: null, toasts: [], selection: { ids: [3, 4], current: 4, currentIndex: 1, anchorIndex: 0, anchorId: 3 } });
  timelineReply = (c) => ({ type: "timeline", ...MOCK_TIMELINE, generation: c.generation });
  mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as Record<string, unknown> & { type: string };
    sent.push(c);
    switch (c.type) {
      case "search":
        return { type: "search", generation: 1, count: 0, hidden: 0 };
      case "range":
        return { type: "range", generation: c.generation, start: 0, items: [] };
      case "timeline": {
        const r = timelineReply(c);
        if (r instanceof Error) throw r;
        return r;
      }
      default:
        return { type: "ok" };
    }
  });
});

describe("タイムラインの切り替え", () => {
  it("使い始めるときに撮影日時の順にし、好みを覚える", () => {
    appStore.set({ sort: { key: "file_name", direction: "descending" } });
    expect(timelineActive(true, appStore.get().sort)).toBe(false);
    setTimelineEnabled(true);
    expect(timelineStore.get().enabled).toBe(true);
    // 向きはそのまま、撮影日時の順にして検索し直す。
    expect(appStore.get().sort).toEqual({ key: "capture_time", direction: "descending" });
    expect(sent.some((c) => c.type === "search")).toBe(true);
    expect(timelineActive(true, appStore.get().sort)).toBe(true);
    expect(localStorage.getItem("genzo.library.timeline")).toBe("1");
    // 並べ替えを変えると、切り替えは残したまま使わない。
    expect(timelineActive(true, { key: "rating", direction: "descending" })).toBe(false);
  });

  it("T で切り替え、知らせる", () => {
    toggleTimeline();
    expect(timelineStore.get().enabled).toBe(true);
    expect(appStore.get().toasts.at(-1)?.message).toContain("タイムラインの表示にしました");
    toggleTimeline();
    expect(timelineStore.get().enabled).toBe(false);
    expect(appStore.get().toasts.at(-1)?.message).toBe("グリッドの表示にしました");
    // 区切りの間隔を覚える。
    setTimelineGap(3600);
    expect(timelineStore.get().gapSeconds).toBe(3600);
    expect(localStorage.getItem("genzo.library.timelineGap")).toBe("3600");
  });

  it("T は「表示」のショートカットに載っている", () => {
    const view = SHORTCUT_GROUPS.find((g) => g.title === "表示")!;
    expect(view.items.some((i) => i.keys.includes("t"))).toBe(true);
  });
});

describe("まとまりの読み込み", () => {
  it("世代と間隔で問い合わせ、カタログを覚える", async () => {
    await loadTimeline(4, 7200, "/c/catalog.db");
    expect(sent.at(-1)).toEqual({ type: "timeline", generation: 4, gap_seconds: 7200 });
    expect(timelineStore.get().data?.generation).toBe(4);
    expect(timelineStore.get().dataCatalog).toBe("/c/catalog.db");
    expect(timelineStore.get().data?.groups).toHaveLength(4);
  });

  it("後から始めた問い合わせの応答を優先し、古い世代（stale）の失敗は捨てる", async () => {
    mock.delay = (call) => {
      const c = call.args?.command as { type: string; generation?: number } | undefined;
      return c?.type === "timeline" && c.generation === 1 ? 30 : 0;
    };
    const slow = loadTimeline(1, 0, null);
    await loadTimeline(2, 0, null);
    await slow;
    expect(timelineStore.get().data?.generation).toBe(2);

    timelineReply = () =>
      Object.assign(new Error("stale"), { kind: "stale", message: "検索結果が更新されました", user_actionable: false, retryable: true, hint: null });
    await loadTimeline(3, 0, null);
    expect(timelineStore.get().error).toBeNull();
    expect(timelineStore.get().data?.generation).toBe(2);

    timelineReply = () =>
      Object.assign(new Error("x"), {
        kind: "invalid_input",
        message: "タイムラインは撮影日時の順の表示でだけ使えます",
        user_actionable: true,
        retryable: false,
        hint: null,
      });
    await loadTimeline(5, 0, null);
    expect(timelineStore.get().error).toBe("タイムラインは撮影日時の順の表示でだけ使えます");
  });
});

describe("撮影日時の修正の入口（LIB-16）", () => {
  it("右クリックのメニューと、ダイアログを開く", async () => {
    const entry = photoMenuEntries(2, undefined).find((e) => e !== "separator" && e.action === "edit_capture_time");
    expect(entry).toMatchObject({ label: "撮影日時を修正…", enabled: true });
    await runPhotoMenu("edit_capture_time");
    expect(appStore.get().dialog).toEqual({ kind: "capture_time", variantIds: [3, 4], current: 4 });
    appStore.set({ dialog: null });
    openCaptureTimeDialog();
    expect(appStore.get().dialog?.kind).toBe("capture_time");
  });

  it("ダイアログを描ける（読み込み前の状態）", () => {
    const html = renderToString(createElement(CaptureTimeDialog, { variantIds: [3, 4], current: 4 })).replace(/<!-- -->/g, "");
    expect(html).toContain("撮影日時の修正");
    expect(html).toContain("機材ごとのすべて");
    expect(html).toContain("選んだ写真（2 枚）");
    expect(html).toContain("基準の写真の正しい時刻に合わせる");
    expect(html).toContain("元のファイルには書きません");
    // 写真を選んでいないときは、選んだ写真を対象にできない。
    const none = renderToString(createElement(CaptureTimeDialog, { variantIds: [], current: null }));
    expect(none).toContain("撮影日時の修正");
  });
});
