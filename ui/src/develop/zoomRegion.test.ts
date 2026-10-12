// 現像の画面の等倍表示で要求する範囲（performance-6）のテスト。

import { describe, expect, it } from "vitest";

import { regionFor } from "../state/region";
import { MAX_REGION_PIXELS } from "../viewer/viewerMath";
import { zoomRegionRequest } from "./DevelopView";

/** 8K（7680 × 4320）・表示倍率 200% で、左右のパネル・上下の帯を除いた表示の大きさ（CSS の画素）。 */
const VIEW_8K = { viewWidth: 3840 - 240 - 300, viewHeight: 2160 - 40 - 96 - 26, devicePixelRatio: 2 };

describe("等倍表示で要求する範囲（performance-6）", () => {
  it("8K の画面でも、要求する画素数を上限（16 Mi 画素）までに抑え、中心を保つ", () => {
    const opts = { fullWidth: 7008, fullHeight: 4672, centerX: 3504, centerY: 2336, scale: 1, ...VIEW_8K };
    const visible = regionFor(opts);
    // そのまま要求すると上限を超える（コアは InvalidArgument を返し、等倍表示が閉じる）。
    expect(visible.width * visible.height).toBeGreaterThan(MAX_REGION_PIXELS);
    const r = zoomRegionRequest(opts);
    expect(r.width * r.height).toBeLessThanOrEqual(MAX_REGION_PIXELS);
    expect(r.x).toBeGreaterThanOrEqual(0);
    expect(r.y).toBeGreaterThanOrEqual(0);
    expect(r.x + r.width).toBeLessThanOrEqual(opts.fullWidth);
    expect(r.y + r.height).toBeLessThanOrEqual(opts.fullHeight);
    expect(Math.abs(r.x + r.width / 2 - opts.centerX)).toBeLessThanOrEqual(1);
    expect(Math.abs(r.y + r.height / 2 - opts.centerY)).toBeLessThanOrEqual(1);
  });

  it("上限に収まるときは、見えている範囲を 1 画素ずつ広げて要求する（ルーペと同じ）", () => {
    const opts = {
      fullWidth: 6000,
      fullHeight: 4000,
      centerX: 3000,
      centerY: 2000,
      scale: 1,
      viewWidth: 1500,
      viewHeight: 900,
      devicePixelRatio: 2,
    };
    const visible = regionFor(opts);
    expect(zoomRegionRequest(opts)).toEqual({
      x: visible.x - 1,
      y: visible.y - 1,
      width: visible.width + 2,
      height: visible.height + 2,
    });
  });
});
