import { describe, expect, it } from "vitest";

import { regionFor } from "./region";

describe("regionFor", () => {
  it("中心のまわりを、表示の物理画素の数だけ切り出す", () => {
    const r = regionFor({
      fullWidth: 6000,
      fullHeight: 4000,
      centerX: 3000,
      centerY: 2000,
      scale: 1,
      viewWidth: 800,
      viewHeight: 600,
      devicePixelRatio: 2,
    });
    expect(r).toEqual({ x: 2200, y: 1400, width: 1600, height: 1200 });
  });
  it("端では画像の内側に収める", () => {
    const r = regionFor({
      fullWidth: 6000,
      fullHeight: 4000,
      centerX: 10,
      centerY: 3990,
      scale: 1,
      viewWidth: 1000,
      viewHeight: 500,
      devicePixelRatio: 1,
    });
    expect(r).toEqual({ x: 0, y: 3500, width: 1000, height: 500 });
  });
  it("画像より表示が大きければ全体", () => {
    const r = regionFor({
      fullWidth: 300,
      fullHeight: 200,
      centerX: 150,
      centerY: 100,
      scale: 1,
      viewWidth: 1000,
      viewHeight: 800,
      devicePixelRatio: 1,
    });
    expect(r).toEqual({ x: 0, y: 0, width: 300, height: 200 });
  });
});
