// 自動補正（DEV-32）の操作: コアに値を求め、設定に入れて送る（履歴に 1 件）。同じ値なら送らず知らせる。失敗は知らせる。

import { beforeEach, describe, expect, it, vi } from "vitest";

import { installBridge } from "../api/bridge";
import { ApiError } from "../api/errors";
import { MOCK_AUTO_TONE, MockBridge, mockAutoToneCommand, mockDevelopSettings } from "../api/mock";
import type { DevelopSettings, DevelopState } from "../api/types";
import { appStore } from "../state/app";
import { applySettings, developStore, openDevelopFor } from "../state/develop";
import { applyAutoTone } from "./viewState";

function state(id: number, settings: DevelopSettings): DevelopState {
  return {
    variant_id: id,
    settings,
    generation: 1,
    can_undo: false,
    can_redo: false,
    dirty: false,
    dragging: false,
    source: { is_raw: true, width: 10, height: 10, decoder: null, as_shot_white_balance: null },
  };
}

describe("自動補正（DEV-32）", () => {
  let mock: MockBridge;
  let settings: DevelopSettings;
  beforeEach(() => {
    mock = new MockBridge();
    installBridge(mock);
    settings = mockDevelopSettings({ exposure_ev: 0.1, contrast: 5 });
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as { type: string; variant_id?: number };
      switch (c.type) {
        case "open_develop":
          return { type: "develop", ...state(c.variant_id ?? 1, settings) };
        case "set_settings":
          return { type: "generation", generation: 2 };
        case "develop_state":
          return { type: "develop_or_none", state: state(1, settings) };
        default:
          return { type: "ok" };
      }
    });
    mock.responses.set("develop_frame", new ArrayBuffer(0));
    developStore.set({ variantId: null, state: null, settings: null });
    appStore.set({ toasts: [] });
  });

  it("送信待ちの設定の後に値を求め、6 つの値を入れた設定を送る（ドラッグではない変更として）", async () => {
    const { sent } = mockAutoToneCommand(mock);
    await openDevelopFor(3);
    applySettings({ ...settings, exposure_ev: 0.7 }, false);
    await applyAutoTone();
    expect(sent).toEqual([{ type: "auto_tone", variant_id: 3 }]);
    const calls = () =>
      mock.calls
        .filter((c) => c.command === "core_execute")
        .map((c) => c.args?.command as { type: string; dragging?: boolean; settings?: DevelopSettings })
        .filter((c) => c.type === "set_settings" || c.type === "auto_tone");
    // 設定の送信は列（pump）で後から行われる。
    await vi.waitFor(() => expect(calls().map((c) => c.type)).toEqual(["set_settings", "auto_tone", "set_settings"]));
    const last = calls()[2];
    expect(last.dragging).toBe(false);
    expect(last.settings?.exposure_ev).toBe(MOCK_AUTO_TONE.exposure_ev);
    expect(last.settings?.contrast).toBe(MOCK_AUTO_TONE.contrast);
    expect(last.settings?.tone).toEqual({ highlights: -40, shadows: 25, whites: 10, blacks: -15 });
    // ほかの項目はそのまま。
    expect(last.settings?.white_balance).toEqual(settings.white_balance);
    expect(developStore.get().settings?.tone.shadows).toBe(25);
  });

  it("今の値と同じなら送らず、知らせる", async () => {
    settings = mockDevelopSettings({
      exposure_ev: MOCK_AUTO_TONE.exposure_ev,
      contrast: MOCK_AUTO_TONE.contrast,
      tone: { highlights: -40, shadows: 25, whites: 10, blacks: -15 },
    });
    mockAutoToneCommand(mock);
    await openDevelopFor(1);
    mock.calls.length = 0;
    await applyAutoTone();
    expect(mock.coreCommands().filter((t) => t === "set_settings")).toEqual([]);
    expect(appStore.get().toasts.map((t) => t.message)).toContain("自動補正: 今の値と同じです");
  });

  it("求めている間に変えた露光量・トーンは上書きしない（後からの操作を優先し、知らせる）", async () => {
    mockAutoToneCommand(mock);
    await openDevelopFor(1);
    // auto_tone の応答を遅くして、その間に露光量を変える。
    mock.delay = (c) => ((c.args?.command as { type: string })?.type === "auto_tone" ? 20 : 0);
    mock.calls.length = 0;
    const running = applyAutoTone();
    await vi.waitFor(() => expect(mock.coreCommands()).toContain("auto_tone"));
    applySettings({ ...settings, exposure_ev: 1.25 }, false);
    await running;
    const sent = () =>
      mock.calls
        .filter((c) => c.command === "core_execute")
        .map((c) => c.args?.command as { type: string; settings?: DevelopSettings })
        .filter((c) => c.type === "set_settings");
    await vi.waitFor(() => expect(sent().length).toBe(1));
    expect(sent()[0].settings?.exposure_ev).toBe(1.25);
    expect(sent()[0].settings?.tone.shadows).toBe(settings.tone.shadows);
    expect(developStore.get().settings?.exposure_ev).toBe(1.25);
    expect(appStore.get().toasts.map((t) => t.message)).toContain(
      "自動補正: 求めている間に露光量・トーンが変わったため、適用しませんでした",
    );
  });

  it("求めている間にほかの項目を変えても、自動補正の値を入れ、その変更も保つ", async () => {
    mockAutoToneCommand(mock);
    await openDevelopFor(1);
    mock.delay = (c) => ((c.args?.command as { type: string })?.type === "auto_tone" ? 20 : 0);
    mock.calls.length = 0;
    const running = applyAutoTone();
    await vi.waitFor(() => expect(mock.coreCommands()).toContain("auto_tone"));
    applySettings({ ...settings, presence: { ...settings.presence, clarity: 40 } }, false);
    await running;
    await vi.waitFor(() => expect(developStore.get().settings?.exposure_ev).toBe(MOCK_AUTO_TONE.exposure_ev));
    expect(developStore.get().settings?.presence.clarity).toBe(40);
  });

  it("写真を開いていなければ何もしない。コアの失敗は知らせる", async () => {
    const { sent } = mockAutoToneCommand(mock);
    await applyAutoTone();
    expect(sent).toEqual([]);
    mockAutoToneCommand(
      mock,
      new ApiError({
        kind: "invalid_input",
        message: "画像の上の画素がないため、自動補正を求められません",
        user_actionable: true,
        retryable: false,
        hint: null,
      }),
    );
    await openDevelopFor(1);
    mock.calls.length = 0;
    await applyAutoTone();
    expect(mock.coreCommands().filter((t) => t === "set_settings")).toEqual([]);
    expect(appStore.get().toasts.some((t) => t.message.startsWith("自動補正を求められません"))).toBe(true);
  });
});
