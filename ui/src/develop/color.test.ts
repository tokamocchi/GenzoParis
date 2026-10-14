// カラーミキサー（DEV-09）・カラーグレーディング（DEV-10）・白黒（DEV-25）の画面: 設定の組み立て、操作の可否（処理
// バージョン・白黒）、色相環の座標、パネルが代表的な状態で例外なく描けること（react-dom/server で文字列にする）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { mockDevelopSettings, mockDevelopState } from "../api/mock";
import { type DevelopSettings, type DevelopState, HUE_BANDS } from "../api/types";
import { SHORTCUT_GROUPS } from "../dialogs/shortcutsData";
import { appStore } from "../state/app";
import { developStore } from "../state/develop";
import {
  COLOR_PROCESS_VERSION,
  HUE_BAND_INFO,
  bwMixIsNeutral,
  colorControls,
  defaultColor,
  fullColor,
  gradingIsDefault,
  hsvCss,
  hueSatToWheelPoint,
  mixerIsNeutral,
  toggleBlackWhite,
  wheelBackground,
  wheelIsNeutral,
  wheelPointToHueSat,
  withBlackWhite,
  withBwMix,
  withGrading,
  withMixer,
  withWheel,
} from "./colorLogic";
import { DevelopPanel } from "./DevelopPanel";

describe("色の設定（カラーミキサー・カラーグレーディング・白黒）", () => {
  it("古いコアの応答で欠けた項目を既定値で補う", () => {
    const c = fullColor({ saturation: 12, vibrance: -5 });
    expect(c.saturation).toBe(12);
    expect(c.vibrance).toBe(-5);
    expect(mixerIsNeutral(c.mixer)).toBe(true);
    expect(c.grading.blending).toBe(50);
    expect(c.grading.balance).toBe(0);
    expect(c.black_white.enabled).toBe(false);
    expect(bwMixIsNeutral(c.black_white.mix)).toBe(true);
    // 一部だけある項目も、残りを補う。
    const p = fullColor({
      mixer: { blue: { luminance: -30 } },
      grading: { shadows: { hue: 220 } },
      black_white: { mix: { red: 10 } },
    } as never);
    expect(p.mixer.blue).toEqual({ hue: 0, saturation: 0, luminance: -30 });
    expect(p.mixer.red).toEqual({ hue: 0, saturation: 0, luminance: 0 });
    expect(p.grading.shadows).toEqual({ hue: 220, saturation: 0, luminance: 0 });
    expect(p.black_white.mix.red).toBe(10);
    expect(p.black_white.mix.magenta).toBe(0);
    expect(fullColor(null)).toEqual(defaultColor());
    // コアの既定値（mock の見本）と同じ。
    expect(defaultColor()).toEqual(mockDevelopSettings().color);
  });

  it("白黒とカラーを切り替える（V キー）。ほかの値は保つ", () => {
    let s = withMixer(mockDevelopSettings(), "orange", "luminance", 20);
    s = withBwMix(s, "blue", -40);
    const bw = toggleBlackWhite(s);
    expect(bw.color.black_white.enabled).toBe(true);
    expect(bw.color.mixer.orange.luminance).toBe(20);
    expect(bw.color.black_white.mix.blue).toBe(-40);
    expect(toggleBlackWhite(bw).color.black_white.enabled).toBe(false);
    expect(withBlackWhite(bw, true).color.black_white.enabled).toBe(true);
    // 古い設定（色の項目が欠けている）でも切り替えられる。
    const old = { ...mockDevelopSettings(), color: { saturation: 5, vibrance: 0 } } as unknown as DevelopSettings;
    const t = toggleBlackWhite(old);
    expect(t.color.black_white.enabled).toBe(true);
    expect(t.color.saturation).toBe(5);
    expect(t.color.grading.blending).toBe(50);
  });

  it("帯・範囲ごとの値を変える", () => {
    let s = withMixer(mockDevelopSettings(), "aqua", "hue", -15);
    expect(s.color.mixer.aqua).toEqual({ hue: -15, saturation: 0, luminance: 0 });
    expect(mixerIsNeutral(s.color.mixer)).toBe(false);
    s = withWheel(s, "highlights", { hue: 40, saturation: 25 });
    expect(s.color.grading.highlights).toEqual({ hue: 40, saturation: 25, luminance: 0 });
    expect(wheelIsNeutral(s.color.grading.highlights)).toBe(false);
    expect(wheelIsNeutral({ hue: 200, saturation: 0, luminance: 0 })).toBe(true);
    s = withGrading(s, { balance: -30 });
    expect(s.color.grading.balance).toBe(-30);
    expect(gradingIsDefault(s.color.grading)).toBe(false);
    expect(gradingIsDefault(defaultColor().grading)).toBe(true);
  });

  it("操作できる項目（処理バージョン・白黒・写真を開いていない）", () => {
    const v2 = mockDevelopSettings();
    expect(colorControls(v2, false)).toEqual({
      needsUpgrade: false,
      blackWhite: false,
      treatment: true,
      saturation: true,
      mixer: true,
      bwMix: false,
      grading: true,
    });
    // 白黒: 彩度・カラーミキサーは使われず、白黒ミキサーを使う。
    const bw = colorControls(withBlackWhite(v2, true), false);
    expect(bw.blackWhite).toBe(true);
    expect(bw.saturation).toBe(false);
    expect(bw.mixer).toBe(false);
    expect(bw.bwMix).toBe(true);
    expect(bw.grading).toBe(true);
    // 処理バージョン 1: v1 の項目は使えない（更新が必要）。彩度・自然な彩度は使える（白黒は効かないので）。
    const v1 = colorControls(withBlackWhite({ ...v2, process_version: 1 }, true), false);
    expect(v1.needsUpgrade).toBe(true);
    expect(v1.treatment || v1.mixer || v1.bwMix || v1.grading).toBe(false);
    expect(v1.saturation).toBe(true);
    expect(COLOR_PROCESS_VERSION).toBe(2);
    // 写真を開いていなければ何も操作できない。
    const off = colorControls(v2, true);
    expect(off.treatment || off.saturation || off.mixer || off.grading).toBe(false);
  });

  it("V のショートカットを一覧に載せる", () => {
    const develop = SHORTCUT_GROUPS.find((g) => g.title === "現像");
    expect(develop?.items.some((i) => i.keys.includes("v") && i.label.includes("白黒"))).toBe(true);
  });
});

describe("色相環と帯の色", () => {
  it("点の位置を色相・強さにする（右が 0、上が 90、縁が 100）", () => {
    expect(wheelPointToHueSat(50, 0, 50)).toEqual({ hue: 0, saturation: 100 });
    expect(wheelPointToHueSat(0, -25, 50)).toEqual({ hue: 90, saturation: 50 });
    expect(wheelPointToHueSat(-10, 0, 50)).toEqual({ hue: 180, saturation: 20 });
    expect(wheelPointToHueSat(0, 40, 50)).toEqual({ hue: 270, saturation: 80 });
    // 外は縁（100）。中心では色相を変えない。
    expect(wheelPointToHueSat(0, 80, 50).saturation).toBe(100);
    expect(wheelPointToHueSat(0, 0, 50, 123)).toEqual({ hue: 123, saturation: 0 });
    expect(wheelPointToHueSat(1, 1, 0, 7)).toEqual({ hue: 7, saturation: 0 });
    // 359.6° は 0° に丸める（360 にしない）。
    expect(wheelPointToHueSat(100, 0.7, 100).hue).toBe(0);
  });

  it("色相・強さと点の位置は行き来できる", () => {
    for (const [h, s] of [
      [0, 100],
      [45, 30],
      [200, 70],
      [330, 5],
    ]) {
      const p = hueSatToWheelPoint(h, s, 80);
      expect(wheelPointToHueSat(p.x, p.y, 80)).toEqual({ hue: h, saturation: s });
    }
    expect(hueSatToWheelPoint(90, 200, 10).y).toBeCloseTo(-10);
  });

  it("帯の色・色相環の背景", () => {
    expect(hsvCss(0)).toBe("rgb(255, 0, 0)");
    expect(hsvCss(120)).toBe("rgb(0, 255, 0)");
    expect(hsvCss(240)).toBe("rgb(0, 0, 255)");
    expect(hsvCss(30)).toBe("rgb(255, 128, 0)");
    expect(hsvCss(-60)).toBe("rgb(255, 0, 255)");
    expect(hsvCss(0, 0)).toBe("rgb(255, 255, 255)");
    expect(HUE_BANDS.map((b) => HUE_BAND_INFO[b].hue)).toEqual([0, 30, 60, 120, 180, 240, 270, 300]);
    const bg = wheelBackground();
    expect(bg).toContain("conic-gradient(from 90deg, rgb(230, 0, 0) 0deg");
    // 時計回りに 90°（下）は色相 270（パープル寄り）。
    expect(bg).toContain(`${hsvCss(270, 1, 0.9)} 90deg`);
  });
});

describe("カラーミキサー・カラーグレーディングのパネルを描く", () => {
  beforeEach(() => {
    const data = new Map<string, string>([
      ["genzo.section.develop.mixer", "1"],
      ["genzo.section.develop.grading", "1"],
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

  it("カラーの写真では、処理の切り替え・カラーミキサーの帯・色相環・ブレンドとバランスを出す", () => {
    const html = render(mockDevelopState());
    expect(html).toContain("処理（カラー / 白黒）");
    expect(html).toContain("カラーミキサー");
    for (const b of HUE_BANDS) expect(html).toContain(HUE_BAND_INFO[b].label);
    expect(html).toContain("色相");
    expect(html).toContain("カラーグレーディング");
    expect(html).toContain("シャドウ");
    expect(html).toContain("中間調");
    expect(html).toContain("ブレンド");
    expect(html).toContain("バランス");
    expect(html).toContain("dp-wheel");
    expect(html).not.toContain("白黒ミキサー");
    expect(html).not.toContain("カラーミキサー・白黒は使われません");
  });

  it("白黒の写真では、白黒ミキサーと、彩度が使われないことを出す", () => {
    const s = withBlackWhite(mockDevelopSettings(), true);
    const html = render(mockDevelopState({ settings: s }));
    expect(html).toContain("白黒ミキサー");
    expect(html).toContain("白黒のときは、彩度・自然な彩度・カラーミキサーは使われません");
  });

  it("処理バージョン 1 の写真では、使われないことと更新のボタンを出す", () => {
    const html = render(mockDevelopState({ settings: mockDevelopSettings({ process_version: 1 }) }));
    expect(html).toContain("カラーミキサー・白黒は使われません");
    expect(html).toContain("カラーグレーディングは使われません");
    expect(html).toContain("処理バージョンを 2 に更新");
  });

  it("古いコアの応答（色の項目が欠けている）でも描ける", () => {
    const old = { ...mockDevelopSettings(), color: { saturation: 0, vibrance: 0 } } as unknown as DevelopSettings;
    const html = render(mockDevelopState({ settings: old }));
    expect(html).toContain("カラーミキサー");
    expect(html).toContain("カラーグレーディング");
  });
});
