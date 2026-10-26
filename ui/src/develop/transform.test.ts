// 変形（遠近補正。DEV-16）の画面: 操作の可否（処理バージョン）、項目のそろえ方、自動の補正（Upright 相当）の送信と
// 設定への反映、貼り付けの項目、パネルが例外なく描けること（react-dom/server で文字列にする）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MockBridge, mockDevelopSettings, mockDevelopState, mockSourceInfo } from "../api/mock";
import type { DevelopState } from "../api/types";
import { DEFAULT_PASTE_SELECTION, PASTE_GROUP_ITEMS, restorePasteSelection, toSettingGroups } from "../dialogs/pasteLogic";
import { appStore } from "../state/app";
import { developStore, openDevelopFor } from "../state/develop";
import { DevelopPanel } from "./DevelopPanel";
import {
  TRANSFORM_PROCESS_VERSION,
  defaultTransform,
  describeUpright,
  fullTransform,
  resetAll,
  sameTransform,
  transformControls,
  transformIsIdentity,
} from "./settingsLogic";
import { applyUpright, changeTransform } from "./viewState";

describe("変形の操作の可否と項目", () => {
  const v2 = mockDevelopSettings();
  const src = mockSourceInfo();

  it("処理バージョン 2 ならすべて動かせ、版 1 では自動の水平だけ", () => {
    expect(transformControls(v2, src, false)).toEqual({
      needsUpgrade: false,
      sliders: true,
      uprightLevel: true,
      uprightPerspective: true,
    });
    const v1 = transformControls({ ...v2, process_version: 1 }, src, false);
    expect(v1.needsUpgrade).toBe(true);
    expect(v1.sliders).toBe(false);
    expect(v1.uprightLevel).toBe(true);
    expect(v1.uprightPerspective).toBe(false);
    expect(TRANSFORM_PROCESS_VERSION).toBe(2);
    // RAW 以外にも使える。
    expect(transformControls(v2, mockSourceInfo({ is_raw: false }), false).sliders).toBe(true);
    // 開いていない・パネル全体が無効なら何も動かせない。
    expect(transformControls(v2, null, false).sliders).toBe(false);
    expect(transformControls(v2, src, true).uprightLevel).toBe(false);
  });

  it("欠けた項目は既定値でそろえ、恒等かどうかを判定する", () => {
    expect(fullTransform(undefined)).toEqual(defaultTransform());
    expect(fullTransform({ vertical: -20 })).toEqual({ ...defaultTransform(), vertical: -20 });
    expect(transformIsIdentity(undefined)).toBe(true);
    expect(transformIsIdentity({ scale: 100 })).toBe(true);
    expect(transformIsIdentity({ scale: 110 })).toBe(false);
    expect(sameTransform(defaultTransform(), { ...defaultTransform(), offset_x: 1 })).toBe(false);
    // すべてリセットで既定に戻る。
    const s = { ...v2, transform: { ...defaultTransform(), vertical: 30 } };
    expect(resetAll(s).transform).toEqual(defaultTransform());
  });

  it("自動の補正の知らせの文", () => {
    const t = { ...defaultTransform(), vertical: -18.5, horizontal: 3 };
    expect(describeUpright("level", 1.25, t, { vertical: 3, horizontal: 5 })).toBe(
      "自動の補正（水平）: 角度 +1.25°（垂直の線 3 本・水平の線 5 本）",
    );
    expect(describeUpright("vertical", -0.5, t, { vertical: 3, horizontal: 0 })).toBe(
      "自動の補正（垂直）: 角度 -0.50°・垂直 -18.5（垂直の線 3 本・水平の線 0 本）",
    );
    expect(describeUpright("full", 0, t, { vertical: 3, horizontal: 2 })).toContain("・水平 +3.0（");
  });

  it("貼り付けの項目に変形があり、既定では写さない", () => {
    expect(PASTE_GROUP_ITEMS.map((i) => i.key)).toContain("transform");
    expect(DEFAULT_PASTE_SELECTION.transform).toBe(false);
    expect(toSettingGroups(DEFAULT_PASTE_SELECTION).transform).toBe(false);
    // 前の版の保存した選択（項目がない）は既定（写さない）。
    expect(restorePasteSelection({ white_balance: true }).transform).toBe(false);
    expect(restorePasteSelection({ transform: true }).transform).toBe(true);
  });
});

describe("自動の遠近補正（Upright）の送信", () => {
  let mock: MockBridge;
  let fail: boolean;
  beforeEach(() => {
    fail = false;
    mock = new MockBridge();
    installBridge(mock);
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as { type: string; variant_id?: number; mode?: string };
      switch (c.type) {
        case "open_develop":
          return { type: "develop", ...mockDevelopState({ variant_id: c.variant_id ?? 1 }) };
        case "analyze_upright":
          if (fail) return new Error("垂直の直線が見つからないため、自動で遠近補正できません");
          return {
            type: "upright",
            mode: c.mode,
            angle_deg: -0.75,
            transform: { ...defaultTransform(), vertical: c.mode === "level" ? 0 : -22.5 },
            vertical_lines: 6,
            horizontal_lines: 3,
            residual_deg: 0.4,
          };
        case "set_settings":
          return { type: "develop", ...mockDevelopState({ variant_id: 1, can_undo: true }) };
        default:
          return { type: "ok" };
      }
    });
    mock.responses.set("develop_frame", new ArrayBuffer(0));
  });
  afterEach(() => {
    developStore.set({ variantId: null, state: null, settings: null });
  });

  it("求めた角度と垂直・水平を設定に入れて送る", async () => {
    await openDevelopFor(1);
    await applyUpright("vertical");
    const sent = mock.calls
      .filter((c) => c.command === "core_execute")
      .map((c) => c.args?.command as { type: string; mode?: string });
    expect(sent.some((c) => c.type === "analyze_upright" && c.mode === "vertical")).toBe(true);
    const s = developStore.get().settings;
    expect(s?.geometry.angle_deg).toBe(-0.75);
    expect(s?.transform?.vertical).toBe(-22.5);
    expect(s?.transform?.scale).toBe(100);
    expect(appStore.get().toasts.some((t) => t.message.includes("自動の補正（垂直）"))).toBe(true);
  });

  it("直線が見つからなければ知らせ、設定は変えない", async () => {
    await openDevelopFor(2);
    fail = true;
    const before = developStore.get().settings;
    await applyUpright("full");
    expect(developStore.get().settings).toBe(before);
    expect(appStore.get().toasts.some((t) => t.level === "error" && t.message.includes("自動で補正できません"))).toBe(
      true,
    );
  });

  it("スライダーの変更は変形だけを送る（同じ値なら送らない）", async () => {
    await openDevelopFor(3);
    const before = developStore.get().settings;
    changeTransform({ scale: 100 }, false);
    expect(developStore.get().settings).toBe(before);
    changeTransform({ horizontal: 15 }, true);
    expect(developStore.get().settings?.transform).toEqual({ ...defaultTransform(), horizontal: 15 });
    expect(developStore.get().settings?.geometry).toEqual(before?.geometry);
  });
});

describe("変形のパネルを描く", () => {
  beforeEach(() => {
    const data = new Map<string, string>([
      ["genzo.section.develop.transform", "1"],
      ["genzo.section.develop.info", "1"],
    ]);
    (globalThis as { localStorage?: unknown }).localStorage = {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, String(v)),
      removeItem: (k: string) => void data.delete(k),
    };
  });
  afterEach(() => {
    delete (globalThis as { localStorage?: unknown }).localStorage;
    developStore.set({ variantId: null, state: null, settings: null });
  });

  function render(state: DevelopState): string {
    appStore.set((s) => ({ selection: { ...s.selection, current: 1, currentIndex: 0 }, dialog: null }));
    developStore.set({ variantId: 1, state, settings: state.settings, opening: false, error: null });
    return renderToString(createElement(DevelopPanel)).replace(/<!-- -->/g, "");
  }

  it("自動の 3 つのボタンと手動の項目を出す", () => {
    const html = render(mockDevelopState());
    expect(html).toContain("自動（Upright）");
    expect(html).toContain(">水平<");
    expect(html).toContain(">垂直<");
    expect(html).toContain(">完全<");
    expect(html).toContain("X オフセット");
    expect(html).toContain("拡大・縮小");
    expect(html).not.toContain("変形（遠近補正）は使われません");
  });

  it("処理バージョン 1 の写真では、更新のボタンを出す", () => {
    const html = render(mockDevelopState({ settings: mockDevelopSettings({ process_version: 1 }) }));
    expect(html).toContain("変形（遠近補正）は使われません");
    expect(html).toContain("処理バージョンを 2 に更新");
  });

  it("古いコアの設定（変形の項目がない）でも描ける", () => {
    const s = mockDevelopSettings();
    delete (s as { transform?: unknown }).transform;
    const html = render(mockDevelopState({ settings: s }));
    expect(html).toContain("自動（Upright）");
  });
});
