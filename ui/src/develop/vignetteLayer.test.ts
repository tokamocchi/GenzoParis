// 切り抜き後の周辺光量補正の中心の印（VignetteLayer。DEV-24）と、中心と G 座標の変換のテスト。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { mockDevelopSettings } from "../api/mock";
import type { DevelopSettings } from "../api/types";
import { gToVignetteCenter, vignetteCenterToG, withVignette } from "./detailLogic";
import type { Box, GBounds } from "./maskLogic";
import { VignetteLayer } from "./VignetteLayer";

describe("周辺光量補正の中心と G 座標", () => {
  it("横長・縦長の外接矩形で、割合と G 座標（長辺 1）を行き来する", () => {
    // 横長（6000 × 4000）: G は 1 × 0.6667。
    expect(vignetteCenterToG({ x: 0.25, y: 0.75 }, 6000, 4000)).toEqual([0.25, 0.5]);
    expect(gToVignetteCenter(0.25, 0.5, 6000, 4000)).toEqual({ x: 0.25, y: 0.75 });
    // 縦長（4000 × 6000）: G は 0.6667 × 1。
    expect(vignetteCenterToG({ x: 0.75, y: 0.5 }, 4000, 6000)).toEqual([0.5, 0.5]);
    expect(gToVignetteCenter(0.5, 0.5, 4000, 6000)).toEqual({ x: 0.75, y: 0.5 });
  });

  it("画像の外は端に寄せ、小数 4 桁に丸める", () => {
    expect(gToVignetteCenter(-0.2, 2, 3000, 2000)).toEqual({ x: 0, y: 1 });
    expect(gToVignetteCenter(1 / 3, 0.1, 3000, 2000)).toEqual({ x: 0.3333, y: 0.15 });
  });
});

describe("周辺光量補正の中心の印を描く", () => {
  // 切り抜いた後の表示（G の [0.25, 0.1]〜[0.75, 0.6]）を、箱（左 10・上 20、500 × 500）に出している。
  const box: Box = { left: 10, top: 20, width: 500, height: 500 };
  const gb: GBounds = [0.25, 0.1, 0.75, 0.6];

  /** 印の円の中心（CSS の画素）。 */
  function knobAt(html: string): [number, number] {
    const m = /<circle class="dv-vignette-knob" cx="([^"]+)" cy="([^"]+)"/.exec(html);
    expect(m, html).not.toBeNull();
    return [Number(m![1]), Number(m![2])];
  }

  function render(settings: DevelopSettings): string {
    return renderToString(
      createElement(VignetteLayer, { width: 600, height: 600, box, gb, settings, gw: 4000, gh: 2000 }),
    );
  }

  it("切り抜きの中心（自動）は点線の印、手動の中心はその位置に塗った印", () => {
    const base = mockDevelopSettings();
    const cropped = withVignette(
      { ...base, geometry: { ...base.geometry, crop: { left: 0.25, top: 0.2, right: 0.75, bottom: 1 } } },
      { amount: -30 },
    );
    // 切り抜きの中心 (0.5, 0.6) → G (0.5, 0.3) → 箱の (10 + 250, 20 + 200)。
    const auto = render(cropped);
    expect(auto).toContain('class="dv-vignette-center"');
    const [ax, ay] = knobAt(auto);
    expect(ax).toBeCloseTo(260, 9);
    expect(ay).toBeCloseTo(220, 9);
    expect(auto).toContain("ドラッグすると手動で動かせます");
    // 手動の中心 (0.3, 0.4) → G (0.3, 0.2) → 箱の (10 + 50, 20 + 100)。
    const manual = render(withVignette(cropped, { center: { x: 0.3, y: 0.4 } }));
    expect(manual).toContain('class="dv-vignette-center manual"');
    const [mx, my] = knobAt(manual);
    expect(mx).toBeCloseTo(60, 9);
    expect(my).toBeCloseTo(120, 9);
  });
});
