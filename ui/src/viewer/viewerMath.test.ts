import { describe, expect, it } from "vitest";

import type { FrameInfo } from "../api/types";
import { regionFor } from "../state/region";
import {
  MAX_FILMSTRIP_SCROLL_PX,
  MAX_REGION_PIXELS,
  L1_SETTLE_MS,
  RecentMap,
  accumulateWheel,
  clampCenter,
  expandRegion,
  fitRect,
  formatCodec,
  formatFps,
  formatResolution,
  keepVisibleOffset,
  l1RequestDelay,
  limitRegion,
  normalizedPoint,
  normalizedToOutput,
  orientedSize,
  outputSizeFromFrame,
  pannedCenter,
  prefetchIndices,
  regionCovers,
  regionUsable,
  sameRect,
  snapToDevicePixels,
  toRealOffset,
  toVirtualOffset,
  withRetryParam,
  zoomOffset,
} from "./viewerMath";

describe("orientedSize", () => {
  it("向き 5〜8 は縦横を入れ替える", () => {
    expect(orientedSize(6000, 4000, 1)).toEqual({ width: 6000, height: 4000 });
    expect(orientedSize(6000, 4000, 3)).toEqual({ width: 6000, height: 4000 });
    expect(orientedSize(6000, 4000, 6)).toEqual({ width: 4000, height: 6000 });
    expect(orientedSize(6000, 4000, 8)).toEqual({ width: 4000, height: 6000 });
  });
  it("大きさが分からなければ null", () => {
    expect(orientedSize(null, 4000, 1)).toBeNull();
    expect(orientedSize(6000, 0, 1)).toBeNull();
  });
});

describe("fitRect（全体表示の倍率）", () => {
  it("横長の画像は幅に合わせて上下の中央に置く", () => {
    // 余白 12 → 使える 976 × 776。3:2 は幅で決まる（976 / 3000 < 776 / 2000）。
    const r = fitRect(3000, 2000, 1000, 800, 12);
    expect(r.width).toBeCloseTo(976);
    expect(r.height).toBeCloseTo(976 * (2000 / 3000));
    expect(r.x).toBeCloseTo(12);
    expect(r.y).toBeCloseTo((800 - r.height) / 2);
  });
  it("縦長の画像は高さに合わせて左右の中央に置く", () => {
    const r = fitRect(2000, 3000, 1000, 600, 0);
    expect(r.height).toBeCloseTo(600);
    expect(r.width).toBeCloseTo(400);
    expect(r.x).toBeCloseTo(300);
    expect(r.y).toBeCloseTo(0);
  });
  it("小さい画像も表示に合わせて拡大する（L0 と L1 を同じ大きさで出すため）", () => {
    const r = fitRect(320, 213, 1000, 1000, 0);
    expect(r.width).toBeCloseTo(1000);
  });
  it("表示の大きさが 0 なら大きさ 0", () => {
    expect(fitRect(3000, 2000, 0, 0).width).toBe(0);
    expect(fitRect(0, 2000, 1000, 600).width).toBe(0);
  });
});

describe("クリックした点から出力の座標への変換", () => {
  const box = { x: 100, y: 50, width: 800, height: 400 };
  it("矩形の中の点は割合になる", () => {
    expect(normalizedPoint(500, 250, box)).toEqual({ u: 0.5, v: 0.5, inside: true });
    expect(normalizedPoint(100, 50, box)).toEqual({ u: 0, v: 0, inside: true });
    expect(normalizedPoint(900, 450, box)).toEqual({ u: 1, v: 1, inside: true });
  });
  it("矩形の外は端に収めて inside が偽", () => {
    expect(normalizedPoint(50, 500, box)).toEqual({ u: 0, v: 1, inside: false });
  });
  it("割合を出力の画素にする（33 MP の 7008 × 4672）", () => {
    const n = normalizedPoint(300, 150, box); // u = 0.25, v = 0.25
    expect(normalizedToOutput(n.u, n.v, 7008, 4672)).toEqual({ x: 1752, y: 1168 });
  });
});

describe("パンの範囲の制限", () => {
  it("等倍・dpr 1: 中心は見える幅の半分から、全体 − 半分まで", () => {
    // 表示 800 CSS px = 800 出力の画素が見える。中心は 400〜5600。
    expect(clampCenter(0, 6000, 800, 1)).toBe(400);
    expect(clampCenter(3000, 6000, 800, 1)).toBe(3000);
    expect(clampCenter(9999, 6000, 800, 1)).toBe(5600);
  });
  it("dpr 2 では見える出力の画素が 2 倍", () => {
    expect(clampCenter(0, 6000, 800, 2)).toBe(800);
    expect(clampCenter(6000, 6000, 800, 2)).toBe(5200);
  });
  it("画像が表示より小さい向きは中央に固定", () => {
    expect(clampCenter(10, 600, 800, 1)).toBe(300);
    expect(clampCenter(10, 1000, 800, 1.5)).toBe(500); // 見えるのは 1200 画素
  });
  it("ドラッグで中心は指と逆に動く（物理画素で換算）", () => {
    expect(pannedCenter(3000, 100, 1)).toBe(2900);
    expect(pannedCenter(3000, -50, 2)).toBe(3100);
    expect(pannedCenter(3000, 30, 1.5)).toBe(2955);
  });
  it("拡大した画像の左上は、中心が表示の中央に来る位置", () => {
    expect(zoomOffset({ x: 3000, y: 2000 }, { width: 800, height: 600 }, 1)).toEqual({ x: -2600, y: -1700 });
    expect(zoomOffset({ x: 3000, y: 2000 }, { width: 800, height: 600 }, 2)).toEqual({ x: -1100, y: -700 });
    // 画像が表示より小さい: 中央に置かれる（左に余白）。
    const c = clampCenter(0, 600, 800, 1);
    expect(zoomOffset({ x: c, y: 300 }, { width: 800, height: 600 }, 1).x).toBe(100);
  });
  it("見える範囲（regionFor）と画面の位置が一致する", () => {
    const dpr = 1.5;
    const view = { width: 1000, height: 700 };
    const center = { x: clampCenter(5000, 7008, view.width, dpr), y: clampCenter(100, 4672, view.height, dpr) };
    const r = regionFor({
      fullWidth: 7008,
      fullHeight: 4672,
      centerX: center.x,
      centerY: center.y,
      scale: 1,
      viewWidth: view.width,
      viewHeight: view.height,
      devicePixelRatio: dpr,
    });
    const off = zoomOffset(center, view, dpr);
    // 範囲の左上は表示の左上（0, 0）に来る（切り捨ての 1 画素の差まで）。
    expect(off.x + r.x / dpr).toBeGreaterThan(-1);
    expect(off.x + r.x / dpr).toBeLessThanOrEqual(0.001);
    expect(off.y + r.y / dpr).toBeCloseTo(0, 5);
    expect(r.width).toBe(1500);
    expect(r.height).toBe(1050);
  });
});

describe("snapToDevicePixels", () => {
  it("要素の位置＋ずれが物理画素の境界になる", () => {
    const off = snapToDevicePixels(10.3, 240.5, 2);
    expect(((240.5 + off) * 2) % 1).toBe(0);
    expect(off).toBeCloseTo(10.5);
  });
  it("表示倍率 150% でも境界に合う", () => {
    const off = snapToDevicePixels(-1234.4, 100.3333, 1.5);
    const phys = (100.3333 + off) * 1.5;
    expect(Math.abs(phys - Math.round(phys))).toBeLessThan(1e-9);
    expect(Math.abs(off - -1234.4)).toBeLessThanOrEqual(1 / 1.5 / 2 + 1e-9);
  });
});

describe("limitRegion・regionCovers", () => {
  it("上限以下ならそのまま", () => {
    const r = { x: 10, y: 20, width: 3840, height: 2160 };
    expect(limitRegion(r)).toBe(r);
  });
  it("上限を超えたら中心を保って縮める", () => {
    const r = { x: 0, y: 0, width: 6016, height: 3384 }; // 約 2,036 万画素
    const l = limitRegion(r);
    expect(l.width * l.height).toBeLessThanOrEqual(MAX_REGION_PIXELS);
    expect(l.width / l.height).toBeCloseTo(6016 / 3384, 2);
    // 中心は切り捨ての 1 画素の差まで保つ。
    expect(Math.abs(l.x + l.width / 2 - 3008)).toBeLessThanOrEqual(1);
    expect(Math.abs(l.y + l.height / 2 - 1692)).toBeLessThanOrEqual(1);
  });
  it("含むかどうか", () => {
    const have = { x: 100, y: 100, width: 1000, height: 800 };
    expect(regionCovers(have, { x: 100, y: 100, width: 1000, height: 800 })).toBe(true);
    expect(regionCovers(have, { x: 200, y: 150, width: 500, height: 500 })).toBe(true);
    expect(regionCovers(have, { x: 99, y: 150, width: 500, height: 500 })).toBe(false);
    expect(regionCovers(have, { x: 200, y: 150, width: 1000, height: 500 })).toBe(false);
  });
  it("各辺を 1 画素広げて画像の内側に収める", () => {
    expect(expandRegion({ x: 1074, y: 10, width: 860, height: 700 }, 1, 6000, 4000)).toEqual({
      x: 1073,
      y: 9,
      width: 862,
      height: 702,
    });
    expect(expandRegion({ x: 0, y: 3300, width: 860, height: 700 }, 1, 6000, 4000)).toEqual({
      x: 0,
      y: 3299,
      width: 861,
      height: 701,
    });
  });
  it("見えている範囲を、位置の丸めの後も覆う（表示倍率 150%・中心が端数）", () => {
    const dpr = 1.5;
    const view = { width: 861, height: 701 };
    const center = { x: clampCenter(1923.37, 6000, view.width, dpr), y: clampCenter(1424.6, 4000, view.height, dpr) };
    const off = zoomOffset(center, view, dpr);
    const snapped = { x: snapToDevicePixels(off.x, 240.33, dpr), y: snapToDevicePixels(off.y, 40.67, dpr) };
    const need = expandRegion(
      regionFor({
        fullWidth: 6000,
        fullHeight: 4000,
        centerX: center.x,
        centerY: center.y,
        scale: 1,
        viewWidth: view.width,
        viewHeight: view.height,
        devicePixelRatio: dpr,
      }),
      1,
      6000,
      4000,
    );
    // 表示の左上・右下に当たる出力の画素の範囲（物理画素の単位）。
    const visible = {
      x0: -snapped.x * dpr,
      y0: -snapped.y * dpr,
      x1: (view.width - snapped.x) * dpr,
      y1: (view.height - snapped.y) * dpr,
    };
    expect(need.x).toBeLessThanOrEqual(visible.x0);
    expect(need.y).toBeLessThanOrEqual(visible.y0);
    expect(need.x + need.width).toBeGreaterThanOrEqual(visible.x1);
    expect(need.y + need.height).toBeGreaterThanOrEqual(visible.y1);
  });
  it("同じ矩形か", () => {
    expect(sameRect({ x: 1, y: 2, width: 3, height: 4 }, { x: 1, y: 2, width: 3, height: 4 })).toBe(true);
    expect(sameRect({ x: 1, y: 2, width: 3, height: 4 }, { x: 1, y: 2, width: 3, height: 5 })).toBe(false);
  });
});

describe("regionUsable（描いてある範囲をそのまま使えるか）", () => {
  const have = {
    x: 100,
    y: 100,
    region_width: 1000,
    region_height: 800,
    full_width: 7008,
    full_height: 4672,
    generation: 5,
  };
  const need = { x: 200, y: 150, width: 800, height: 600 };
  it("同じ大きさ・範囲を含む・世代が古くなければ使う", () => {
    expect(regionUsable(have, need, 7008, 4672, 5)).toBe(true);
    // 世代が分からなければ、世代では判断しない。
    expect(regionUsable(have, need, 7008, 4672, null)).toBe(true);
    // 描いた後に知った世代が古い（描いた方が新しい）なら使う。
    expect(regionUsable(have, need, 7008, 4672, 4)).toBe(true);
  });
  it("現像設定が外から変わった（世代が新しい）なら描き直す", () => {
    expect(regionUsable(have, need, 7008, 4672, 6)).toBe(false);
  });
  it("範囲を含まない・出力の大きさが違う・まだ描いていないなら描き直す", () => {
    expect(regionUsable(have, { ...need, x: 50 }, 7008, 4672, 5)).toBe(false);
    expect(regionUsable(have, need, 6000, 4000, 5)).toBe(false);
    expect(regionUsable(null, need, 7008, 4672, 5)).toBe(false);
  });
});

describe("outputSizeFromFrame", () => {
  const base = { variant_id: 1, generation: 1, width: 1600, height: 1067 } as FrameInfo;
  it("出力の大きさがあれば使う", () => {
    expect(outputSizeFromFrame({ ...base, output_width: 7008, output_height: 4672 })).toEqual({
      width: 7008,
      height: 4672,
    });
  });
  it("古いコア（値がない）・切り抜きの編集中は使わない", () => {
    expect(outputSizeFromFrame(base)).toBeNull();
    expect(outputSizeFromFrame({ ...base, output_width: 7008, output_height: 4672, uncropped: true })).toBeNull();
    expect(outputSizeFromFrame({ ...base, output_width: 0, output_height: 4672 })).toBeNull();
    expect(outputSizeFromFrame(null)).toBeNull();
  });
});

describe("prefetchIndices（先読みする位置）", () => {
  it("進む向きに 2 枚、戻る向きに 1 枚（近い順）", () => {
    expect(prefetchIndices(10, 100, 1)).toEqual([11, 9, 12]);
    expect(prefetchIndices(10, 100, -1)).toEqual([9, 11, 8]);
  });
  it("範囲の外は除く", () => {
    expect(prefetchIndices(0, 100, -1)).toEqual([1]);
    expect(prefetchIndices(99, 100, 1)).toEqual([98]);
    expect(prefetchIndices(0, 1, 1)).toEqual([]);
  });
  it("数を変えられる", () => {
    expect(prefetchIndices(5, 100, 1, 3, 0)).toEqual([6, 7, 8]);
  });
});

describe("l1RequestDelay（写真送りの途中は L1 の要求を待つ）", () => {
  it("前の写真から間がなければ待つ", () => {
    expect(l1RequestDelay(33)).toBe(L1_SETTLE_MS);
    expect(l1RequestDelay(0, 150)).toBe(150);
    expect(l1RequestDelay(149, 150)).toBe(150);
  });
  it("間があれば（1 枚ずつ送っている・最初の写真）すぐ要求する", () => {
    expect(l1RequestDelay(150, 150)).toBe(0);
    expect(l1RequestDelay(2000)).toBe(0);
    expect(l1RequestDelay(Number.POSITIVE_INFINITY)).toBe(0);
  });
});

describe("withRetryParam", () => {
  it("0 なら変えない（先読みと同じ URI）", () => {
    expect(withRetryParam("genzo://localhost/preview/3?rev=ab", 0)).toBe("genzo://localhost/preview/3?rev=ab");
  });
  it("rev の有無に合わせて付ける", () => {
    expect(withRetryParam("genzo://localhost/preview/3?rev=ab", 2)).toBe("genzo://localhost/preview/3?rev=ab&retry=2");
    expect(withRetryParam("genzo://localhost/preview/3", 1)).toBe("genzo://localhost/preview/3?retry=1");
  });
});

describe("RecentMap", () => {
  it("上限を超えたら古いものから外す", () => {
    const m = new RecentMap<string, number>(2);
    expect(m.set("a", 1)).toEqual([]);
    m.set("b", 2);
    expect(m.set("c", 3)).toEqual([1]);
    expect(m.keys()).toEqual(["b", "c"]);
  });
  it("使ったものは新しくなる", () => {
    const m = new RecentMap<string, number>(2);
    m.set("a", 1);
    m.set("b", 2);
    expect(m.touch("a")).toBe(true);
    expect(m.touch("x")).toBe(false);
    m.set("c", 3);
    expect(m.keys()).toEqual(["a", "c"]);
    m.delete("a");
    expect(m.size).toBe(1);
    expect(m.get("c")).toBe(3);
  });
});

describe("keepVisibleOffset（フィルムストリップ）", () => {
  const base = { cell: 100, viewport: 1000, count: 1000 };
  it("内側に見えていれば動かさない", () => {
    expect(keepVisibleOffset({ ...base, index: 5, offset: 0 })).toBeNull();
    expect(keepVisibleOffset({ ...base, index: 8, offset: 0 })).toBeNull();
  });
  it("端の 1 個ぶんに入ったら中央に置く", () => {
    // 9 番は 900〜1000（右端の余白の中）。中央 = 950 − 500 = 450。
    expect(keepVisibleOffset({ ...base, index: 9, offset: 0 })).toBe(450);
    // 左端の余白の中。
    expect(keepVisibleOffset({ ...base, index: 10, offset: 1000 })).toBe(550);
  });
  it("先頭・末尾では範囲に収める", () => {
    expect(keepVisibleOffset({ ...base, index: 0, offset: 5000 })).toBe(0);
    expect(keepVisibleOffset({ ...base, index: 999, offset: 0 })).toBe(99_000);
  });
  it("狭いときは余白なしで判定する", () => {
    expect(keepVisibleOffset({ cell: 100, viewport: 150, count: 10, index: 1, offset: 100 })).toBeNull();
  });
  it("範囲の外の位置は null", () => {
    expect(keepVisibleOffset({ ...base, index: 1000, offset: 0 })).toBeNull();
  });
});

describe("スクロールの位置の対応（50 万件）", () => {
  it("上限に収まれば同じ値", () => {
    expect(toVirtualOffset(1234, 9000, 9000)).toBe(1234);
    expect(toRealOffset(1234, 9000, 9000)).toBe(1234);
  });
  it("上限を超えたら比例させる（往復で戻る）", () => {
    const viewport = 1200;
    const virtualMax = 500_000 * 86 - viewport;
    const realMax = MAX_FILMSTRIP_SCROLL_PX - viewport;
    expect(toVirtualOffset(realMax, realMax, virtualMax)).toBe(virtualMax);
    expect(toVirtualOffset(0, realMax, virtualMax)).toBe(0);
    const v = 12_345_678;
    expect(toVirtualOffset(toRealOffset(v, realMax, virtualMax), realMax, virtualMax)).toBeCloseTo(v, 3);
  });
  it("範囲の外は端に収める", () => {
    expect(toVirtualOffset(-5, 100, 1000)).toBe(0);
    expect(toRealOffset(5000, 100, 1000)).toBe(100);
    expect(toRealOffset(10, 0, 0)).toBe(0);
  });
});

describe("accumulateWheel（ホイールの端数の持ち越し）", () => {
  it("1 px 未満の量でも、貯まれば動く", () => {
    // 50 万件（比 約 0.19）で deltaY = 2 を 20 回: 2 × 0.1882 × 20 ≈ 7.5 px。
    let rest = 0;
    let moved = 0;
    for (let i = 0; i < 20; i++) {
      const r = accumulateWheel(rest, 2 * 0.1882);
      rest = r.rest;
      moved += r.step;
    }
    expect(moved).toBe(7);
    expect(rest).toBeCloseTo(0.528, 6);
  });
  it("切り捨てで量が減らない（100 × 0.1882 を 5 回で 94 px）", () => {
    let rest = 0;
    let moved = 0;
    for (let i = 0; i < 5; i++) {
      const r = accumulateWheel(rest, 100 * 0.1882);
      rest = r.rest;
      moved += r.step;
    }
    expect(moved).toBe(94);
  });
  it("負の向きも同じ。向きが変わったら端数を捨てる", () => {
    expect(accumulateWheel(0, -0.6)).toEqual({ step: 0, rest: -0.6 });
    const r = accumulateWheel(-0.6, -0.6);
    expect(r.step).toBe(-1);
    expect(r.rest).toBeCloseTo(-0.2, 9);
    expect(accumulateWheel(-0.6, 0.5)).toEqual({ step: 0, rest: 0.5 });
  });
});

describe("書式", () => {
  it("フレームレート", () => {
    expect(formatFps(59.94)).toBe("59.94 fps");
    expect(formatFps(30000 / 1001)).toBe("29.97 fps");
    expect(formatFps(24000 / 1001)).toBe("23.976 fps");
    expect(formatFps(25)).toBe("25 fps");
    expect(formatFps(null)).toBe("—");
  });
  it("コーデック", () => {
    expect(formatCodec("hevc", 10)).toBe("H.265 / HEVC（10 bit）");
    expect(formatCodec("h264", null)).toBe("H.264 / AVC");
    expect(formatCodec("av1", 8)).toBe("av1（8 bit）");
    expect(formatCodec(null, 10)).toBe("—");
  });
  it("解像度", () => {
    expect(formatResolution(3840, 2160)).toBe("3840 × 2160");
    expect(formatResolution(null, 2160)).toBe("—");
  });
});
