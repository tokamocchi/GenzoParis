import { describe, expect, it } from "vitest";

import type { DevelopSettings, HistogramData, HistoryItem } from "../api/types";
import { historyAvailability, historyRows, redoAvailable } from "./historyMath";
import { activatesOnEnter } from "./keyTargets";
import { clipIndicators, clippingOverlay, histogramHeights, histogramScale } from "./pixels";
import {
  formatAngle,
  formatEv,
  formatKelvin,
  formatSigned,
  parseNumber,
  positionToTemperature,
  sameShown,
  temperatureToPosition,
} from "./scales";
import { effectiveWb, resetAll, wbFromMode, wbMode } from "./settingsLogic";
import { clampCenter, fitBox, zoomOrigin } from "./zoomMath";

describe("色温度の目盛り（対数）", () => {
  it("両端と中央", () => {
    expect(temperatureToPosition(2000)).toBe(0);
    expect(temperatureToPosition(50000)).toBeCloseTo(1000, 9);
    // 2000 × √25 = 10000 K が中央。
    expect(temperatureToPosition(10000)).toBeCloseTo(500, 9);
    expect(positionToTemperature(0)).toBe(2000);
    expect(positionToTemperature(1000)).toBe(50000);
    expect(positionToTemperature(500)).toBe(10000);
  });
  it("範囲の外は端に収める", () => {
    expect(temperatureToPosition(100)).toBe(0);
    expect(temperatureToPosition(1e6)).toBeCloseTo(1000, 9);
    expect(positionToTemperature(-5)).toBe(2000);
    expect(positionToTemperature(2000)).toBe(50000);
  });
  it("位置 → 色温度 → 位置で戻る（1 K に丸める誤差の範囲）", () => {
    for (const k of [2850, 3800, 5500, 6500, 7500, 23456]) {
      expect(Math.abs(positionToTemperature(temperatureToPosition(k)) - k)).toBeLessThanOrEqual(1);
    }
  });
});

describe("書式", () => {
  it("符号つき", () => {
    expect(formatSigned(25)).toBe("+25");
    expect(formatSigned(-25)).toBe("−25");
    expect(formatSigned(0)).toBe("0");
    expect(formatSigned(-0.001, 2)).toBe("0.00");
    expect(formatEv(0.35)).toBe("+0.35 EV");
    expect(formatEv(-1.5)).toBe("−1.50 EV");
    expect(formatEv(0)).toBe("0.00 EV");
    expect(formatKelvin(5499.6)).toBe("5500 K");
    expect(formatAngle(-1.25)).toBe("−1.25°");
  });
  it("単位つきの入力を読む", () => {
    expect(parseNumber("5500 K")).toBe(5500);
    expect(parseNumber("−1.50 EV")).toBe(-1.5);
    expect(parseNumber("+0.35")).toBe(0.35);
    expect(parseNumber("abc")).toBeNaN();
  });
});

describe("クリッピングの表示", () => {
  it("白飛び（いずれかが 255）は赤、黒つぶれ（すべて 0）は青、他は透明", () => {
    // 1 画素ずつ: 白飛び（R）・黒つぶれ・どちらでもない・白飛び（G・B）。
    const px = new Uint8ClampedArray([255, 10, 10, 255, 0, 0, 0, 255, 0, 0, 1, 255, 10, 255, 255, 255]);
    const r = clippingOverlay(px, 2, 2);
    expect(r.high).toBe(2);
    expect(r.low).toBe(1);
    expect([...r.data.slice(0, 4)]).toEqual([255, 0, 0, 255]);
    expect(r.data[4 + 3]).toBe(255);
    expect(r.data[4 + 2]).toBeGreaterThan(r.data[4]);
    expect(r.data[8 + 3]).toBe(0);
    expect(r.data[12 + 3]).toBe(255);
  });
  it("印はクリップした画素があれば点く", () => {
    const zero = { red: 0, green: 0, blue: 0, any: 0, all: 0 };
    const h = (high: number, low: number): HistogramData => ({
      red: [],
      green: [],
      blue: [],
      luma: [],
      pixel_count: 100,
      clipped_high: { ...zero, any: high },
      clipped_low: { ...zero, any: low },
    });
    expect(clipIndicators(h(1, 0))).toEqual({ high: true, low: false });
    expect(clipIndicators(h(0, 3))).toEqual({ high: false, low: true });
    expect(clipIndicators(null)).toEqual({ high: false, low: false });
  });
});

describe("ヒストグラムの縮尺", () => {
  it("両端の階級を除いた最大値に合わせる", () => {
    const a = new Array(256).fill(0);
    a[0] = 10000;
    a[100] = 50;
    a[255] = 9000;
    const b = new Array(256).fill(0);
    b[200] = 80;
    expect(histogramScale([a, b])).toBe(80);
    const hgt = histogramHeights(a, 80);
    expect(hgt[0]).toBe(1);
    expect(hgt[100]).toBeCloseTo(50 / 80, 12);
  });
  it("内側がすべて 0 なら全体の最大値、それも 0 なら 1", () => {
    const a = new Array(256).fill(0);
    a[255] = 30;
    expect(histogramScale([a])).toBe(30);
    expect(histogramScale([new Array(256).fill(0)])).toBe(1);
  });
});

describe("履歴の移動", () => {
  const item = (id: number, current = false): HistoryItem => ({
    id,
    created_at: "2026-10-12T00:00:00Z",
    label: `操作 ${id}`,
    is_current: current,
  });
  const entries = [item(1), item(2), item(3, true), item(4), item(5)];
  it("今の位置の前後に項目があれば Undo / Redo できる", () => {
    expect(historyAvailability(entries)).toEqual({ canUndo: true, canRedo: true });
    expect(historyAvailability([item(1, true)])).toEqual({ canUndo: false, canRedo: false });
    expect(historyAvailability([item(1), item(2, true)])).toEqual({ canUndo: true, canRedo: false });
    expect(historyAvailability(null)).toBeNull();
  });
  it("やり直せるのは、コアの状態と履歴の両方がやり直せて、その後に画面で設定を変えていないとき", () => {
    expect(redoAvailable(true, { canRedo: true }, false)).toBe(true);
    // Undo の後にドラッグでない変更をして、まだ保存していない（コアの状態・履歴は古いまま）。
    expect(redoAvailable(true, { canRedo: true }, true)).toBe(false);
    // 保存の後に読み直した履歴にはやり直せる項目がない。
    expect(redoAvailable(true, { canRedo: false }, false)).toBe(false);
    // コアの状態（保存待ちがあれば偽）。
    expect(redoAvailable(false, { canRedo: true }, false)).toBe(false);
    // 履歴をまだ読んでいなければ、コアの状態で判断する。
    expect(redoAvailable(true, null, false)).toBe(true);
  });
  it("表示は新しい順で、今の位置より後はやり直せる項目", () => {
    const rows = historyRows(entries);
    expect(rows.map((r) => r.item.id)).toEqual([5, 4, 3, 2, 1]);
    expect(rows.map((r) => r.undone)).toEqual([true, true, false, false, false]);
    expect(rows.find((r) => r.current)?.item.id).toBe(3);
  });
});

describe("WB の選択とリセット", () => {
  const asShot = { temperature_k: 5100, tint: 8 };
  it("撮影時の設定では撮影時の値を出し、分からなければ仮の値", () => {
    expect(effectiveWb("as_shot", asShot)).toEqual({ temperature_k: 5100, tint: 8, known: true });
    expect(effectiveWb("as_shot", null).known).toBe(false);
    expect(effectiveWb({ preset: "tungsten" }, asShot)).toEqual({ temperature_k: 2850, tint: 0, known: true });
  });
  it("カスタムに切り替えると、今出している値から始める", () => {
    expect(wbFromMode("custom", "as_shot", asShot)).toEqual({ custom: { temperature_k: 5100, tint: 8 } });
    expect(wbFromMode("custom", { preset: "shade" }, asShot)).toEqual({ custom: { temperature_k: 7500, tint: 10 } });
    expect(wbFromMode("cloudy", "as_shot", asShot)).toEqual({ preset: "cloudy" });
    expect(wbMode({ custom: { temperature_k: 4000, tint: 0 } })).toBe("custom");
    expect(wbMode({ preset: "flash" })).toBe("flash");
  });
  it("すべてリセット: 版・外部データ・部分補正・レンズ・ディテールは保つ", () => {
    const s = {
      schema_version: 3,
      process_version: 2,
      white_balance: { custom: { temperature_k: 4000, tint: 5 } },
      exposure_ev: 1.2,
      contrast: 30,
      tone: { highlights: -50, shadows: 40, whites: 10, blacks: -10 },
      tone_curve: {
        parametric: { highlights: 10, lights: 0, darks: 0, shadows: 0 },
        luma: [
          { x: 0, y: 0 },
          { x: 1, y: 0.9 },
        ],
        red: [],
        green: [],
        blue: [],
      },
      color: { saturation: 20, vibrance: 10 },
      geometry: { crop: { left: 0.1, top: 0.1, right: 0.9, bottom: 0.9 }, angle_deg: 2, aspect: "original" },
      lens: { profile_enabled: true, distortion_scale: 100, vignetting_scale: 100, remove_chromatic_aberration: true },
      detail: {
        sharpening: { amount: 40, radius: 1, detail: 25, masking: 0 },
        noise_reduction: { luminance: 10, color: 25 },
      },
      masks: [{ any: 1 }],
      render_deps: { camera_profile: { id: "x", sha256: "y" }, lens_profile: null, raw_decoder: "z" },
    } as DevelopSettings;
    const r = resetAll(s);
    expect(r.white_balance).toBe("as_shot");
    expect(r.exposure_ev).toBe(0);
    expect(r.contrast).toBe(0);
    expect(r.tone).toEqual({ highlights: 0, shadows: 0, whites: 0, blacks: 0 });
    expect(r.tone_curve.luma).toEqual([]);
    expect(r.tone_curve.parametric.highlights).toBe(0);
    expect(r.color).toEqual({ saturation: 0, vibrance: 0 });
    expect(r.geometry).toEqual({ crop: { left: 0, top: 0, right: 1, bottom: 1 }, angle_deg: 0, aspect: null });
    expect(r.schema_version).toBe(3);
    expect(r.process_version).toBe(2);
    expect(r.render_deps).toBe(s.render_deps);
    expect(r.masks).toBe(s.masks);
    expect(r.lens).toBe(s.lens);
    expect(r.detail).toBe(s.detail);
  });
});

describe("全体表示と等倍表示の配置", () => {
  it("縮小だけで中央に置く", () => {
    expect(fitBox(1000, 500, 2000, 1000)).toEqual({ left: 0, top: 0, width: 1000, height: 500, scale: 0.5 });
    const b = fitBox(1000, 1000, 2000, 1000);
    expect(b.top).toBe(250);
    // 小さい画像は拡大しない。
    expect(fitBox(1000, 1000, 200, 100)).toEqual({ left: 400, top: 450, width: 200, height: 100, scale: 1 });
  });
  it("等倍表示の中心は見える範囲が画像の内側に収まるように制限する", () => {
    expect(clampCenter(6000, 1000, 100)).toBe(500);
    expect(clampCenter(6000, 1000, 5900)).toBe(5500);
    expect(clampCenter(800, 1000, 100)).toBe(400);
    expect(zoomOrigin(6000, 1000, 3000)).toBe(2500);
    expect(zoomOrigin(800, 1000, 100)).toBe(-100);
  });
});

describe("入力欄を編集せずに離れたとき", () => {
  it("表示が同じ値なら変えない（色温度は 1 K、tint・スライダーは整数、露光量は 0.01 EV の表示で比べる）", () => {
    const signed0 = (v: number) => formatSigned(v, 0);
    expect(sameShown(formatKelvin, 5124, 5123.6)).toBe(true);
    expect(sameShown(formatKelvin, 5125, 5123.6)).toBe(false);
    // 撮影時の tint が小数でも、表示（"−3"）を読み直した値は同じとみなす。
    expect(sameShown(signed0, parseNumber(signed0(-2.5)), -2.5)).toBe(true);
    expect(sameShown(signed0, 7, 7.4)).toBe(true);
    expect(sameShown(signed0, 8, 7.4)).toBe(false);
    expect(sameShown(formatEv, parseNumber(formatEv(0.333)), 0.333)).toBe(true);
    expect(sameShown(formatEv, 0.34, 0.333)).toBe(false);
  });
});

describe("Enter を横取りしない要素", () => {
  const el = (tagName: string, extra: { type?: string; role?: string } = {}) => ({
    tagName,
    type: extra.type,
    getAttribute: (name: string) => (name === "role" ? (extra.role ?? null) : null),
  });
  it("ボタン・リンク・チェックボックス・ボタンの役割の要素", () => {
    expect(activatesOnEnter(el("BUTTON") as unknown as EventTarget)).toBe(true);
    expect(activatesOnEnter(el("A") as unknown as EventTarget)).toBe(true);
    expect(activatesOnEnter(el("INPUT", { type: "checkbox" }) as unknown as EventTarget)).toBe(true);
    expect(activatesOnEnter(el("INPUT", { type: "submit" }) as unknown as EventTarget)).toBe(true);
    expect(activatesOnEnter(el("DIV", { role: "button" }) as unknown as EventTarget)).toBe(true);
    expect(activatesOnEnter(el("DIV", { role: "tab" }) as unknown as EventTarget)).toBe(true);
  });
  it("それ以外（本体・スライダー・ステージ）では切り抜きの完了に使う", () => {
    expect(activatesOnEnter(null)).toBe(false);
    expect(activatesOnEnter(el("BODY") as unknown as EventTarget)).toBe(false);
    expect(activatesOnEnter(el("INPUT", { type: "range" }) as unknown as EventTarget)).toBe(false);
    expect(activatesOnEnter(el("DIV") as unknown as EventTarget)).toBe(false);
    expect(activatesOnEnter({} as EventTarget)).toBe(false);
  });
});
