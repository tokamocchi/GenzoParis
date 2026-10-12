import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MockBridge } from "../api/mock";
import type { DevelopSettings, DevelopState } from "../api/types";
import { applySettings, developStore, endDrag, openDevelopFor, undo } from "./develop";

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
