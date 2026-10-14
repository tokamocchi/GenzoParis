// テクスチャ・明瞭度・かすみの除去（DEV-11）・シャープ（DEV-12）・効果（DEV-24）の画面: 操作の可否（処理バージョン・
// 量）、項目のそろえ方と変え方、パネルが代表的な状態で例外なく描けること（react-dom/server で文字列にする）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { mockDevelopSettings, mockDevelopState, mockSourceInfo } from "../api/mock";
import type { DevelopState } from "../api/types";
import { appStore } from "../state/app";
import { developStore } from "../state/develop";
import { DevelopPanel } from "./DevelopPanel";
import {
  DETAIL_PROCESS_VERSION,
  defaultEffects,
  defaultPresence,
  defaultSharpening,
  detailControls,
  fullEffects,
  fullPresence,
  fullSharpening,
  withGrain,
  withPresence,
  withSharpening,
  withVignette,
} from "./detailLogic";

describe("テクスチャ・明瞭度・かすみの除去・シャープ・効果の操作の可否", () => {
  const v2 = mockDevelopSettings();

  it("版 2 の写真では量を動かせ、ほかの項目は量が 0 でないときだけ", () => {
    expect(DETAIL_PROCESS_VERSION).toBe(2);
    expect(detailControls(v2, false)).toEqual({
      needsUpgrade: false,
      presence: true,
      sharpen: true,
      sharpenOptions: false,
      vignette: true,
      vignetteOptions: false,
      vignetteHighlights: false,
      grain: true,
      grainOptions: false,
      sharpenChanged: false,
      effectsChanged: false,
    });
    const s = withGrain(withVignette(withSharpening(v2, { amount: 40 }), { amount: -30 }), { amount: 20 });
    const c = detailControls(s, false);
    expect(c.sharpenOptions && c.vignetteOptions && c.vignetteHighlights && c.grainOptions).toBe(true);
    expect(c.sharpenChanged && c.effectsChanged).toBe(true);
    // ハイライトの保護は暗くするときだけ。
    expect(detailControls(withVignette(v2, { amount: 30 }), false).vignetteHighlights).toBe(false);
    expect(detailControls(withVignette(v2, { amount: 30 }), false).vignetteOptions).toBe(true);
  });

  it("処理バージョン 1・開いていないときは動かせない（RAW 以外の写真には使える）", () => {
    const v1 = detailControls({ ...v2, process_version: 1 }, false);
    expect(v1.needsUpgrade).toBe(true);
    expect(v1.presence || v1.sharpen || v1.vignette || v1.grain).toBe(false);
    const off = detailControls(v2, true);
    expect(off.presence || off.sharpen || off.grain).toBe(false);
    // RAW かどうかは見ない（仕上げの段階の処理）。
    expect(detailControls(v2, false).presence).toBe(true);
  });

  it("欠けた項目は既定値でそろえる（コアの serde の既定と同じ）", () => {
    expect(defaultPresence()).toEqual(mockDevelopSettings().presence);
    expect(defaultSharpening()).toEqual(mockDevelopSettings().detail.sharpening);
    expect(defaultEffects()).toEqual(mockDevelopSettings().effects);
    expect(fullPresence(undefined)).toEqual({ texture: 0, clarity: 0, dehaze: 0 });
    expect(fullPresence({ clarity: 20 })).toEqual({ texture: 0, clarity: 20, dehaze: 0 });
    // カラーミキサーのパッケージまでのコアの応答（presence・effects が無い）。
    expect(fullEffects(undefined)).toEqual(defaultEffects());
    expect(fullEffects({ grain: { amount: 10 } } as never).grain).toEqual({ amount: 10, size: 25, roughness: 50 });
    expect(fullSharpening({ amount: 40 })).toEqual({ amount: 40, radius: 1, detail: 25, masking: 0 });
    const { presence: _p, effects: _e, ...old } = mockDevelopSettings();
    const c = detailControls(old as never, false);
    expect(c.effectsChanged || c.sharpenChanged).toBe(false);
  });

  it("一部を変えた設定を作る（ほかの項目は保つ）", () => {
    const s = withPresence(mockDevelopSettings(), { dehaze: 35 });
    expect(s.presence).toEqual({ texture: 0, clarity: 0, dehaze: 35 });
    const t = withSharpening(s, { masking: 50 });
    expect(t.detail.sharpening).toEqual({ amount: 0, radius: 1, detail: 25, masking: 50 });
    expect(t.detail.noise_reduction).toEqual(s.detail.noise_reduction);
    expect(t.presence.dehaze).toBe(35);
    const u = withGrain(withVignette(t, { roundness: -40 }), { size: 70 });
    expect(u.effects.vignette.roundness).toBe(-40);
    expect(u.effects.vignette.midpoint).toBe(50);
    expect(u.effects.grain).toEqual({ amount: 0, size: 70, roughness: 50 });
  });
});

describe("外観・ディテール・効果のパネルを描く", () => {
  beforeEach(() => {
    const data = new Map<string, string>([
      ["genzo.section.develop.detail", "1"],
      ["genzo.section.develop.effects", "1"],
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

  it("基本補正の外観・ディテールのシャープ・効果の項目を出す", () => {
    const html = render(mockDevelopState());
    for (const label of [
      "外観",
      "テクスチャ",
      "明瞭度",
      "かすみの除去",
      "シャープ",
      "適用量",
      "半径",
      "マスク",
      "効果",
      "切り抜き後の周辺光量補正",
      "中点",
      "丸み",
      "ぼかし",
      "粒子",
      "大きさ",
      "粗さ",
    ]) {
      expect(html).toContain(label);
    }
    expect(html).toContain("等倍（100%）の表示で確かめてください");
    expect(html).not.toContain("処理バージョンを 2 に更新");
  });

  it("処理バージョン 1 の写真では、使われないことと更新のボタンを出す", () => {
    const html = render(mockDevelopState({ settings: mockDevelopSettings({ process_version: 1 }) }));
    expect(html).toContain("テクスチャ・明瞭度・かすみの除去は使われません");
    expect(html).toContain("シャープ・ノイズ軽減は使われません");
    expect(html).toContain("効果は使われません");
    expect(html).toContain("処理バージョンを 2 に更新");
  });

  it("RAW 以外の写真でも使える（使われないという知らせを出さない）", () => {
    const html = render(mockDevelopState({ source: mockSourceInfo({ is_raw: false, lens_correction: null }) }));
    expect(html).toContain("明瞭度");
    expect(html).not.toContain("RAW 以外の写真では、シャープ");
    expect(html).not.toContain("効果は使われません");
  });
});
