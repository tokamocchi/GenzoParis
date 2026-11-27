import { beforeEach, describe, expect, it } from "vitest";

import { SPOT_VISUAL_SENSITIVITY, spotVisualThreshold, spotVisualization } from "./pixels";
import { setSpotVisual, toggleSpotVisual, viewStore } from "./viewState";

/** 灰色（`base`）の画像に、明るさの違う点を置いた RGBA の画素。 */
function image(w: number, h: number, base: number, dots: { x: number; y: number; v: number }[] = []): Uint8Array {
  const px = new Uint8Array(w * h * 4);
  for (let i = 0; i < w * h; i++) px.set([base, base, base, 255], i * 4);
  for (const d of dots) px.set([d.v, d.v, d.v, 255], (d.y * w + d.x) * 4);
  return px;
}

/** 白い画素の位置（x,y）。 */
function whites(data: Uint8ClampedArray, w: number): string[] {
  const out: string[] = [];
  for (let i = 0; i < data.length / 4; i++) {
    if (data[i * 4] === 255) out.push(`${i % w},${Math.floor(i / w)}`);
    // 白か黒だけで、不透明。
    expect([0, 255]).toContain(data[i * 4]);
    expect(data[i * 4 + 3]).toBe(255);
  }
  return out;
}

describe("スポットの可視化（DEV-21）", () => {
  it("しきい値は、感度を上げるほど小さくなる（0% で 96、100% で 4）", () => {
    expect(spotVisualThreshold(0)).toBeCloseTo(96, 5);
    expect(spotVisualThreshold(100)).toBeCloseTo(4, 5);
    expect(spotVisualThreshold(50)).toBeLessThan(spotVisualThreshold(40));
    // 範囲の外・数でない値は、範囲に収める・既定にする。
    expect(spotVisualThreshold(-10)).toBeCloseTo(96, 5);
    expect(spotVisualThreshold(500)).toBeCloseTo(4, 5);
    expect(spotVisualThreshold(Number.NaN)).toBeCloseTo(spotVisualThreshold(SPOT_VISUAL_SENSITIVITY.default), 5);
  });

  it("一様な画像は真っ黒で、点のまわりだけが白くなる", () => {
    const w = 9;
    const h = 7;
    expect(whites(spotVisualization(image(w, h, 128), w, h, 100), w)).toEqual([]);
    // 淡い点（まわりより 24 暗い）: 感度が高ければ輪郭が出て、低ければ出ない。
    const faint = image(w, h, 128, [{ x: 4, y: 3, v: 104 }]);
    const high = whites(spotVisualization(faint, w, h, 100), w);
    expect(high.length).toBeGreaterThan(0);
    // 輪郭は点の隣の画素に出る（点そのものは左右・上下が同じ値なので勾配が 0）。
    expect(high).toContain("3,3");
    expect(high).toContain("5,3");
    expect(high).not.toContain("4,3");
    expect(high.every((p) => Math.abs(Number(p.split(",")[0]) - 4) <= 1 && Math.abs(Number(p.split(",")[1]) - 3) <= 1)).toBe(true);
    expect(whites(spotVisualization(faint, w, h, 0), w)).toEqual([]);
  });

  it("画像の縁は、外に同じ値が続くものとして扱う（縁に輪郭を作らない）", () => {
    const w = 5;
    const h = 4;
    expect(whites(spotVisualization(image(w, h, 200), w, h, 100), w)).toEqual([]);
  });

  it("画素が足りない・大きさが 0 なら、空（透明）の画像を返す", () => {
    expect(spotVisualization(new Uint8Array(8), 4, 4, 50)).toEqual(new Uint8ClampedArray(64));
    expect(spotVisualization(new Uint8Array(0), 0, 0, 50).length).toBe(0);
  });
});

describe("スポットの可視化の表示の切り替え", () => {
  beforeEach(() => {
    viewStore.set({ tool: "none", spotVisual: false, spotVisualSensitivity: SPOT_VISUAL_SENSITIVITY.default });
  });

  it("A は、スポット修正の道具を使っている間だけ切り替える", () => {
    toggleSpotVisual();
    expect(viewStore.get().spotVisual).toBe(false);
    viewStore.set({ tool: "retouch" });
    toggleSpotVisual();
    expect(viewStore.get().spotVisual).toBe(true);
    toggleSpotVisual();
    expect(viewStore.get().spotVisual).toBe(false);
  });

  it("感度は範囲に収める", () => {
    setSpotVisual({ on: true, sensitivity: 130 });
    expect(viewStore.get()).toMatchObject({ spotVisual: true, spotVisualSensitivity: 100 });
    setSpotVisual({ sensitivity: -5 });
    expect(viewStore.get()).toMatchObject({ spotVisual: true, spotVisualSensitivity: 0 });
  });
});
