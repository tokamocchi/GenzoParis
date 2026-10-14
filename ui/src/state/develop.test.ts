import { beforeEach, describe, expect, it, vi } from "vitest";

import { installBridge } from "../api/bridge";
import { dispatchCoreEventForTest } from "../api/events";
import { MOCK_BEFORE_INFO, MockBridge, mockBeforeFrame } from "../api/mock";
import type { DevelopSettings, DevelopState } from "../api/types";
import {
  applySettings,
  beforePending,
  developStore,
  endDrag,
  installDevelopEvents,
  openDevelopFor,
  setBeforeSource,
  undo,
} from "./develop";

function settings(exposure: number): DevelopSettings {
  return { exposure_ev: exposure } as unknown as DevelopSettings;
}

function state(id: number): DevelopState {
  return {
    variant_id: id,
    settings: settings(0),
    generation: 1,
    can_undo: false,
    can_redo: false,
    dirty: false,
    dragging: false,
    source: { is_raw: true, width: 10, height: 10, decoder: null, as_shot_white_balance: null },
  };
}

describe("現像の設定の送信", () => {
  let mock: MockBridge;
  beforeEach(() => {
    mock = new MockBridge();
    installBridge(mock);
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as { type: string; variant_id?: number };
      switch (c.type) {
        case "open_develop":
          return { type: "develop", ...state(c.variant_id ?? 1) };
        case "set_settings":
          return { type: "generation", generation: 2 };
        case "end_drag":
          return { type: "develop", ...state(1) };
        case "undo":
          return { type: "settings_or_none", settings: settings(-1) };
        case "develop_state":
          return { type: "develop_or_none", state: state(1) };
        default:
          return { type: "ok" };
      }
    });
    mock.responses.set("develop_frame", new ArrayBuffer(0));
  });

  it("送信中の値は最新の 1 つだけを送り、順序を保つ", async () => {
    await openDevelopFor(1);
    // set_settings を遅くして、送信中に値が積み重なる状況にする。
    mock.delay = (c) => ((c.args?.command as { type: string })?.type === "set_settings" ? 20 : 0);
    applySettings(settings(0.1), true);
    applySettings(settings(0.2), true);
    applySettings(settings(0.3), true);
    endDrag();
    applySettings(settings(0.4), false);
    await undo();
    const sent = mock.calls
      .filter((c) => c.command === "core_execute")
      .map((c) => c.args?.command as { type: string; settings?: { exposure_ev: number } })
      .filter((c) => c.type !== "open_develop" && c.type !== "develop_state")
      .map((c) => (c.type === "set_settings" ? `set:${c.settings?.exposure_ev}` : c.type));
    // 0.2 は 0.3 に置き換えられる。end_drag は 0.3 の後、0.4 は end_drag の後、undo は最後。
    expect(sent).toEqual(["set:0.1", "set:0.3", "end_drag", "set:0.4", "undo"]);
    expect(developStore.get().settings?.exposure_ev).toBe(-1);
  });

  it("写真を切り替える前に、送信中に来た最新の値を前の写真に送る（捨てない）", async () => {
    await openDevelopFor(1);
    mock.delay = (c) => ((c.args?.command as { type: string })?.type === "set_settings" ? 20 : 0);
    mock.calls.length = 0;
    applySettings(settings(0.1), false);
    applySettings(settings(0.2), false); // 0.1 の送信中に来る（送信待ち）
    await openDevelopFor(2);
    const sent = mock.calls
      .filter((c) => c.command === "core_execute")
      .map((c) => c.args?.command as { type: string; variant_id?: number; settings?: { exposure_ev: number } })
      .filter((c) => c.type === "set_settings" || c.type === "open_develop")
      .map((c) => (c.type === "set_settings" ? `set:${c.settings?.exposure_ev}` : `open:${c.variant_id}`));
    expect(sent).toEqual(["set:0.1", "set:0.2", "open:2"]);
  });
});

describe("ビフォー / アフター（DEV-31）", () => {
  let mock: MockBridge;
  beforeEach(() => {
    mock = new MockBridge();
    installBridge(mock);
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as { type: string; variant_id?: number; source?: { kind: string } | null };
      switch (c.type) {
        case "open_develop":
          return { type: "develop", ...state(c.variant_id ?? 1), before: null };
        case "set_settings":
          return { type: "generation", generation: 2 };
        case "set_before":
          return {
            type: "before",
            before: c.source ? { ...MOCK_BEFORE_INFO, variant_id: developStore.get().variantId, source: c.source } : null,
          };
        default:
          return { type: "ok" };
      }
    });
    mock.responses.set("develop_frame", new ArrayBuffer(0));
    mock.responses.set("before_frame", () => mockBeforeFrame(developStore.get().variantId ?? 0, MOCK_BEFORE_INFO.generation));
    installDevelopEvents();
  });

  it("設定の送信の後にビフォーを設定し、描けたら画素を取る。やめると捨てる", async () => {
    await openDevelopFor(5);
    applySettings(settings(0.5), false);
    await setBeforeSource({ kind: "initial" });
    expect(mock.coreCommands().filter((t) => t !== "develop_state")).toEqual(["open_develop", "set_settings", "set_before"]);
    expect(developStore.get().before?.label).toBe("読み込み");
    await vi.waitFor(() => expect(developStore.get().beforeFrame).not.toBeNull());
    expect(developStore.get().beforeFrame?.header.generation).toBe(MOCK_BEFORE_INFO.generation);
    expect(beforePending(developStore.get())).toBe(false);
    // 描き直しの知らせで読み直す。古い世代では置き換えない。
    mock.responses.set("before_frame", () => mockBeforeFrame(5, 3));
    dispatchCoreEventForTest({ type: "before_updated", variant_id: 5, generation: 3, width: 4, height: 3 });
    await new Promise((r) => setTimeout(r, 10));
    expect(developStore.get().beforeFrame?.header.generation).toBe(MOCK_BEFORE_INFO.generation);
    mock.responses.set("before_frame", () => mockBeforeFrame(5, 12));
    dispatchCoreEventForTest({ type: "before_updated", variant_id: 5, generation: 12, width: 4, height: 3 });
    await vi.waitFor(() => expect(developStore.get().beforeFrame?.header.generation).toBe(12));
    // 別の写真の知らせでは読まない。
    const calls = mock.calls.filter((c) => c.command === "before_frame").length;
    dispatchCoreEventForTest({ type: "before_updated", variant_id: 9, generation: 13, width: 4, height: 3 });
    await new Promise((r) => setTimeout(r, 10));
    expect(mock.calls.filter((c) => c.command === "before_frame").length).toBe(calls);
    await setBeforeSource(null);
    expect(developStore.get().before).toBeNull();
    expect(developStore.get().beforeFrame).toBeNull();
  });

  it("写真を開き直すとビフォーはやめた状態に戻る。出どころを変えた直後は描画中", async () => {
    await openDevelopFor(5);
    await setBeforeSource({ kind: "initial" });
    await vi.waitFor(() => expect(developStore.get().beforeFrame).not.toBeNull());
    developStore.set({ before: { ...MOCK_BEFORE_INFO, variant_id: 5, generation: 99 } });
    expect(beforePending(developStore.get())).toBe(true);
    await openDevelopFor(6);
    expect(developStore.get().before).toBeNull();
    expect(developStore.get().beforeFrame).toBeNull();
    expect(beforePending(developStore.get())).toBe(false);
  });
});
