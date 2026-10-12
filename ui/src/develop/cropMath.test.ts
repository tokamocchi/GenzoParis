import { describe, expect, it } from "vitest";

import type { CropRect } from "../api/types";
import {
  MIN_CROP_FRACTION,
  MODEL_MIN_CROP_SIZE,
  aspectKey,
  dragCrop,
  fractionAspect,
  gBoxSize,
  geometryInfo,
  matchAspect,
  maxAspectCrop,
  cropFractionAspect,
  orientedSize,
  pixelAspect,
  rotatedImageCorners,
  sanitizeCrop,
} from "./cropMath";

const rect = (left: number, top: number, right: number, bottom: number): CropRect => ({ left, top, right, bottom });
const w = (c: CropRect) => c.right - c.left;
const h = (c: CropRect) => c.bottom - c.top;
const inside = (c: CropRect) => c.left >= 0 && c.top >= 0 && c.right <= 1 + 1e-12 && c.bottom <= 1 + 1e-12;

describe("縦横比", () => {
  it("向きが 5〜8 なら幅と高さを入れ替える", () => {
    expect(orientedSize(6000, 4000, 1)).toEqual({ width: 6000, height: 4000 });
    expect(orientedSize(6000, 4000, 6)).toEqual({ width: 4000, height: 6000 });
    expect(orientedSize(6000, 4000, 8)).toEqual({ width: 4000, height: 6000 });
    expect(orientedSize(6000, 4000, 3)).toEqual({ width: 6000, height: 4000 });
  });
  it("G の外接矩形（genzo-pipeline の g_size_px）", () => {
    expect(gBoxSize(6000, 4000, 0)).toEqual({ width: 6000, height: 4000 });
    const g = gBoxSize(6000, 4000, 30);
    const c = Math.cos(Math.PI / 6);
    const s = Math.sin(Math.PI / 6);
    expect(g.width).toBeCloseTo(6000 * c + 4000 * s, 9);
    expect(g.height).toBeCloseTo(6000 * s + 4000 * c, 9);
    // 角度の符号では変わらない。
    expect(gBoxSize(6000, 4000, -30).width).toBeCloseTo(g.width, 9);
  });
  it("画素の縦横比を割合の縦横比にする", () => {
    expect(pixelAspect("original", 6000, 4000)).toBeCloseTo(1.5, 12);
    expect(pixelAspect({ ratio: { w: 4, h: 5 } }, 6000, 4000)).toBeCloseTo(0.8, 12);
    expect(pixelAspect(null, 6000, 4000)).toBeNull();
    // 3:2 の画像（角度 0）で元の比率なら、割合の比は 1（全体のまま）。
    expect(fractionAspect(1.5, 6000, 4000)).toBeCloseTo(1, 12);
    // 1:1 なら割合の幅は高さの 4000 / 6000。
    expect(fractionAspect(1, 6000, 4000)).toBeCloseTo(4000 / 6000, 12);
  });
  it("選択肢のキー", () => {
    expect(aspectKey(null)).toBe("free");
    expect(aspectKey("original")).toBe("original");
    expect(aspectKey({ ratio: { w: 16, h: 9 } })).toBe("16:9");
  });
  it("縦横比を選ぶと、その比で外接矩形に収まる最大の枠になる（中心は今の枠の中心）", () => {
    const k = fractionAspect(1, 6000, 4000)!;
    const c = maxAspectCrop(rect(0, 0, 1, 1), k);
    expect(w(c) / h(c)).toBeCloseTo(k, 12);
    expect(h(c)).toBeCloseTo(1, 12);
    expect((c.left + c.right) / 2).toBeCloseTo(0.5, 12);
    // 端に寄った枠でも最大の大きさにして、内側へずらす。
    const d = maxAspectCrop(rect(0.8, 0.1, 0.95, 0.3), 2);
    expect(w(d)).toBeCloseTo(1, 12);
    expect(h(d)).toBeCloseTo(0.5, 12);
    expect(d.left).toBeCloseTo(0, 12);
    expect(d.top).toBeCloseTo(0, 12);
    expect(inside(d)).toBe(true);
  });
  it("縦横比の計算に使う大きさ（元の画像と向き、なければ切り抜く前のプレビュー）", () => {
    const a = geometryInfo({ source: { width: 6000, height: 4000 }, orientation: 6, angleDeg: 0, uncropped: null })!;
    expect(a).toEqual({ ow: 4000, oh: 6000, gw: 4000, gh: 6000 });
    expect(cropFractionAspect("original", a)).toBeCloseTo(1, 12);
    // 向きが分からなければ、G の外接矩形から O を逆に求める。
    const g = gBoxSize(6000, 4000, 12);
    const b = geometryInfo({ source: null, orientation: null, angleDeg: 12, uncropped: g })!;
    expect(b.ow).toBeCloseTo(6000, 6);
    expect(b.oh).toBeCloseTo(4000, 6);
    expect(geometryInfo({ source: null, orientation: null, angleDeg: 45, uncropped: g })).toBeNull();
    expect(geometryInfo({ source: null, orientation: null, angleDeg: 0, uncropped: null })).toBeNull();
    expect(cropFractionAspect(null, a)).toBeNull();
  });
  it("角度を変えたときは幅を保って高さを合わせ、外に出るなら縮める", () => {
    const c = matchAspect(rect(0.1, 0.1, 0.9, 0.5), 1);
    expect(w(c)).toBeCloseTo(0.6, 12);
    expect(h(c)).toBeCloseTo(0.6, 12);
    expect((c.top + c.bottom) / 2).toBeCloseTo(0.3, 12);
    expect(inside(c)).toBe(true);
  });
});

describe("枠のドラッグ", () => {
  const start = rect(0.2, 0.2, 0.8, 0.8);
  it("移動は大きさを保ち、0〜1 に収める", () => {
    const c = dragCrop(start, "move", 0.5, -0.5, null);
    expect(c.left).toBeCloseTo(0.4, 12);
    expect(c.top).toBe(0);
    expect(w(c)).toBeCloseTo(0.6, 12);
    expect(h(c)).toBeCloseTo(0.6, 12);
  });
  it("自由: 辺はその辺だけ、角は 2 辺を動かし、最小の大きさを保つ", () => {
    expect(dragCrop(start, "e", 0.1, 0.3, null)).toEqual(rect(0.2, 0.2, 0.9, 0.8));
    const c = dragCrop(start, "nw", -0.5, 0.1, null);
    expect(c.left).toBe(0);
    expect(c.top).toBeCloseTo(0.3, 12);
    const tiny = dragCrop(start, "w", 0.9, 0, null);
    expect(w(tiny)).toBeCloseTo(MIN_CROP_FRACTION, 12);
    expect(tiny.right).toBe(0.8);
  });
  it("縦横比の固定: 角は反対の角を固定し、比を保つ", () => {
    const k = 1.5;
    const c = dragCrop(rect(0.1, 0.1, 0.4, 0.3), "se", 0.2, 0.05, k);
    expect(c.left).toBe(0.1);
    expect(c.top).toBe(0.1);
    expect(w(c) / h(c)).toBeCloseTo(k, 12);
    // 指した位置（0.6, 0.35）を覆う。
    expect(c.right).toBeGreaterThanOrEqual(0.6 - 1e-12);
    expect(c.bottom).toBeGreaterThanOrEqual(0.35 - 1e-12);
    expect(inside(c)).toBe(true);
  });
  it("縦横比の固定: 端に当たったら比を保って止まる", () => {
    const k = 1;
    const c = dragCrop(rect(0.5, 0.5, 0.6, 0.6), "se", 2, 0.1, k);
    expect(c.right).toBeCloseTo(1, 12);
    expect(c.bottom).toBeCloseTo(1, 12);
    expect(w(c) / h(c)).toBeCloseTo(k, 12);
    const d = dragCrop(rect(0.5, 0.5, 0.6, 0.6), "nw", -2, -0.1, 2);
    expect(d.left).toBeCloseTo(0, 12);
    expect(w(d) / h(d)).toBeCloseTo(2, 12);
    expect(inside(d)).toBe(true);
  });
  it("縦横比の固定: 辺は反対の辺を固定し、もう一方は中心を保つ", () => {
    const k = 1;
    const c = dragCrop(start, "e", -0.2, 0, k);
    expect(c.left).toBe(0.2);
    expect(w(c)).toBeCloseTo(0.4, 12);
    expect(h(c)).toBeCloseTo(0.4, 12);
    expect((c.top + c.bottom) / 2).toBeCloseTo(0.5, 12);
    const d = dragCrop(rect(0.2, 0.1, 0.4, 0.3), "s", 0, 0.5, k);
    expect(d.top).toBe(0.1);
    expect(w(d) / h(d)).toBeCloseTo(k, 12);
    expect(inside(d)).toBe(true);
  });
});

describe("コアに送る形", () => {
  it("0〜1 に収め、順序と最小の大きさを直す", () => {
    expect(sanitizeCrop(rect(-0.1, 0.2, 1.0000001, 0.8))).toEqual(rect(0, 0.2, 1, 0.8));
    expect(sanitizeCrop(rect(0.8, 0.9, 0.2, 0.1))).toEqual(rect(0.2, 0.1, 0.8, 0.9));
    const s = sanitizeCrop(rect(1, 0.5, 1, 0.5));
    expect(s.right - s.left).toBeGreaterThanOrEqual(MODEL_MIN_CROP_SIZE);
    expect(s.right).toBeLessThanOrEqual(1);
    expect(s.bottom - s.top).toBeGreaterThanOrEqual(MODEL_MIN_CROP_SIZE);
  });
});

describe("回転した元の画像の四隅", () => {
  it("角度 0 では外接矩形の四隅", () => {
    expect(rotatedImageCorners(6000, 4000, 0)).toEqual([
      [0, 0],
      [1, 0],
      [1, 1],
      [0, 1],
    ]);
  });
  it("回転しても四隅は外接矩形の辺に接する", () => {
    const c = rotatedImageCorners(6000, 4000, 10);
    const xs = c.map((p) => p[0]);
    const ys = c.map((p) => p[1]);
    expect(Math.min(...xs)).toBeCloseTo(0, 12);
    expect(Math.max(...xs)).toBeCloseTo(1, 12);
    expect(Math.min(...ys)).toBeCloseTo(0, 12);
    expect(Math.max(...ys)).toBeCloseTo(1, 12);
    // 正の角度は画面上の反時計回り: 左上の角（O の (0, 0)）は外接矩形の左の辺に来る。
    expect(c[0][0]).toBeCloseTo(0, 12);
  });
});
