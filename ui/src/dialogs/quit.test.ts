import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MockBridge } from "../api/mock";
import type { DevelopSettings, DevelopState } from "../api/types";
import * as core from "../api/core";
import { appStore } from "../state/app";
import { applySettings, openDevelopFor } from "../state/develop";
import { registerFlush } from "../state/flush";
import { quitOnRequest } from "./catalogActions";

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

describe("終了の要求", () => {
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
        default:
          return { type: "ok" };
      }
    });
    mock.responses.set("develop_frame", new ArrayBuffer(0));
    mock.responses.set("ack_quit_request", undefined);
    mock.responses.set("quit_app", undefined);
  });

  it("受け取ったと答え、現像の送信待ちの最新の値を送り切ってから終了する", async () => {
    await openDevelopFor(1);
    mock.delay = (c) => ((c.args?.command as { type: string } | undefined)?.type === "set_settings" ? 20 : 0);
    mock.calls.length = 0;
    applySettings(settings(0.1), false);
    applySettings(settings(0.2), false); // 0.1 の送信中に来る（送信待ち）
    await quitOnRequest(7);
    const seq = mock.calls
      .map((c) => {
        if (c.command === "core_execute") {
          const cmd = c.args?.command as { type: string; settings?: { exposure_ev: number } };
          return cmd.type === "set_settings" ? `set:${cmd.settings?.exposure_ev}` : null;
        }
        if (c.command === "ack_quit_request") return `ack:${(c.args as { id: number }).id}`;
        if (c.command === "quit_app") return `quit:${(c.args as { discardUnsaved: boolean }).discardUnsaved}`;
        return null;
      })
      .filter((x) => x !== null);
    // 0.1 は終了の要求の前に送り始めている。0.2（送信待ち）は終了の前に送る。
    expect(seq).toEqual(["set:0.1", "ack:7", "set:0.2", "quit:false"]);
  });

  it("編集中のキャプションなど、登録された書き込みを済ませてから終了する", async () => {
    mock.responses.set("core_execute", () => ({ type: "updated", count: 1 }));
    mock.delay = (c) => ((c.args?.command as { type: string } | undefined)?.type === "set_caption" ? 20 : 0);
    const unregister = registerFlush(async () => {
      await core.setCaption([5], "夕焼け");
    });
    try {
      await quitOnRequest(9);
    } finally {
      unregister();
    }
    const seq = mock.calls
      .map((c) =>
        c.command === "core_execute" ? (c.args?.command as { type: string }).type : c.command,
      )
      .filter((x) => x === "set_caption" || x === "quit_app");
    expect(seq).toEqual(["set_caption", "quit_app"]);
  });

  it("登録された書き込みに失敗したら、終了しない（編集中の内容を黙って捨てない）", async () => {
    const unregister = registerFlush(async () => {
      throw new Error("書けません");
    });
    try {
      await quitOnRequest(11);
    } finally {
      unregister();
    }
    expect(mock.calls.some((c) => c.command === "quit_app")).toBe(false);
    expect(appStore.get().toasts.some((t) => t.message.includes("終了しません"))).toBe(true);
  });

  it("処理の途中に来た 2 つめの要求には答えるだけで、終了を 2 回は始めない", async () => {
    mock.delay = (c) => (c.command === "quit_app" ? 30 : 0);
    await Promise.all([quitOnRequest(1), quitOnRequest(2)]);
    expect(mock.calls.filter((c) => c.command === "ack_quit_request").length).toBe(2);
    expect(mock.calls.filter((c) => c.command === "quit_app").length).toBe(1);
  });
});
