// ハイライト復元（DEV-06）・ノイズ軽減（DEV-13）の画面: 操作の可否（RAW・処理バージョン・量）、項目のそろえ方、
// パネルが代表的な状態で例外なく描けること（react-dom/server で文字列にする）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { mockDevelopSettings, mockDevelopState, mockSourceInfo } from "../api/mock";
import type { DevelopState } from "../api/types";
import { appStore } from "../state/app";
import { developStore } from "../state/develop";
import { DevelopPanel } from "./DevelopPanel";
import {
  defaultHighlightRecovery,
  defaultNoiseReduction,
  fullHighlightRecovery,
  fullNoiseReduction,
  SENSOR_PROCESS_VERSION,
  sensorControls,
} from "./settingsLogic";

describe("ハイライト復元・ノイズ軽減の操作の可否", () => {
  const v2 = mockDevelopSettings();
  const raw = mockSourceInfo();

  it("RAW の版 2 の写真では方式と量を動かせ、ディテール・コントラストは量が 0 でないときだけ", () => {
    expect(SENSOR_PROCESS_VERSION).toBe(2);
    expect(sensorControls(v2, raw, false)).toEqual({
      needsUpgrade: false,
      notRaw: false,
      highlight: true,
      highlightAmount: false,
      noise: true,
      luminanceOptions: false,
      colorOptions: false,
    });
    const s = {
      ...v2,
      highlight_recovery: { method: "reconstruct" as const, amount: 100 },
      detail: { ...v2.detail, noise_reduction: { ...defaultNoiseReduction(), luminance: 20, color: 30 } },
    };
    const c = sensorControls(s, raw, false);
    expect(c.highlightAmount).toBe(true);
    expect(c.luminanceOptions).toBe(true);
    expect(c.colorOptions).toBe(true);
  });

  it("RAW 以外・処理バージョン 1・開いていないときは動かせない", () => {
    const notRaw = sensorControls(v2, mockSourceInfo({ is_raw: false, lens_correction: null }), false);
    expect(notRaw.notRaw).toBe(true);
    expect(notRaw.highlight || notRaw.noise).toBe(false);
    const v1 = sensorControls({ ...v2, process_version: 1 }, raw, false);
    expect(v1.needsUpgrade).toBe(true);
    expect(v1.highlight || v1.noise).toBe(false);
    expect(sensorControls(v2, null, false).noise).toBe(false);
    expect(sensorControls(v2, raw, true).highlight).toBe(false);
  });

  it("欠けた項目は既定値でそろえる（コアの serde の既定と同じ）", () => {
    expect(defaultHighlightRecovery()).toEqual(mockDevelopSettings().highlight_recovery);
    expect(defaultNoiseReduction()).toEqual(mockDevelopSettings().detail.noise_reduction);
    expect(fullHighlightRecovery(undefined)).toEqual({ method: "clip", amount: 100 });
    expect(fullHighlightRecovery({ method: "neutral" })).toEqual({ method: "neutral", amount: 100 });
    // レンズ補正のパッケージまでのコアの応答（ディテール・コントラストが無い）。
    expect(fullNoiseReduction({ luminance: 10, color: 20 })).toEqual({
      luminance: 10,
      luminance_detail: 50,
      luminance_contrast: 0,
      color: 20,
      color_detail: 50,
    });
    // 古いコアの設定（highlight_recovery が無い）でも、操作の可否を求められる。
    const { highlight_recovery: _, ...old } = mockDevelopSettings();
    expect(sensorControls(old as never, mockSourceInfo(), false).highlightAmount).toBe(false);
  });
});

describe("ハイライト復元・ディテールのパネルを描く", () => {
  beforeEach(() => {
    const data = new Map<string, string>([["genzo.section.develop.detail", "1"]]);
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

  it("RAW の写真では、ハイライト復元の方式とノイズ軽減の項目を出す", () => {
    const html = render(mockDevelopState());
    expect(html).toContain("ハイライト復元");
    expect(html).toContain("白で切りそろえる");
    expect(html).toContain("色を保って復元");
    expect(html).toContain("ディテール");
    expect(html).toContain("ノイズ軽減");
    expect(html).toContain("輝度");
    expect(html).toContain("カラー");
    expect(html).toContain("コントラスト");
    expect(html).not.toContain("処理バージョンを 2 に更新");
  });

  it("処理バージョン 1 の写真では、使われないことと更新のボタンを出す", () => {
    const html = render(mockDevelopState({ settings: mockDevelopSettings({ process_version: 1 }) }));
    expect(html).toContain("ハイライト復元は使われません");
    expect(html).toContain("ノイズ軽減は使われません");
    expect(html).toContain("処理バージョンを 2 に更新");
  });

  it("RAW 以外の写真では、使われないことを知らせる", () => {
    const html = render(mockDevelopState({ source: mockSourceInfo({ is_raw: false, lens_correction: null }) }));
    expect(html).toContain("RAW 以外の写真では、ハイライト復元は使われません");
    expect(html).toContain("RAW 以外の写真では、ノイズ軽減は使われません");
  });
});
