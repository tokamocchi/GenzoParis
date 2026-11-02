// 地図の表示（LIB-18）の画面の計算のテスト（Leaflet を読まない）。

import { describe, expect, it } from "vitest";

import type { MapClusters } from "../api/types";
import { FIT_MAX_ZOOM, PIN_ZOOM } from "./geoMapLogic";
import {
  clusterSizeClass,
  clusterView,
  describeBounds,
  describeMapCounts,
  extentView,
  formatClusterCount,
  markerLon,
  padView,
  sameBounds,
  viewBounds,
  wrapLon,
} from "./mapViewLogic";

describe("地図の範囲", () => {
  it("経度を -180〜180 に戻す", () => {
    expect(wrapLon(139.7)).toBe(139.7);
    expect(wrapLon(180)).toBe(180);
    expect(wrapLon(-180)).toBe(-180);
    expect(wrapLon(190)).toBe(-170);
    expect(wrapLon(-190)).toBe(170);
    expect(wrapLon(540)).toBe(-180);
    expect(Object.is(wrapLon(-0), 0)).toBe(true);
  });

  it("Leaflet の表示範囲をコアの範囲にする", () => {
    expect(viewBounds(35, 139, 36, 140)).toEqual({ south: 35, west: 139, north: 36, east: 140 });
    // 右へ続く世界の写しを見ている（経度 170〜190 は、170〜180 と -180〜-170）。
    expect(viewBounds(-20, 170, -10, 190)).toEqual({ south: -20, west: 170, north: -10, east: -170 });
    // 左の写し（-190〜-170）も同じ。
    expect(viewBounds(-20, -190, -10, -170)).toEqual({ south: -20, west: 170, north: -10, east: -170 });
    // 1 つ右の写しの中だけ（200〜220 は -160〜-140）。
    expect(viewBounds(0, 200, 10, 220)).toEqual({ south: 0, west: -160, north: 10, east: -140 });
    // 東の端・西の端がちょうど 180 度の線（またがない）。
    expect(viewBounds(0, -190, 10, -180)).toEqual({ south: 0, west: 170, north: 10, east: 180 });
    expect(viewBounds(0, 180, 10, 200)).toEqual({ south: 0, west: -180, north: 10, east: -160 });
    // 幅が 360 度以上は経度のすべて。緯度は -90〜90 に収める。
    expect(viewBounds(-120, -400, 120, 400)).toEqual({ south: -90, west: -180, north: 90, east: 180 });
    expect(viewBounds(-85, -180, 85, 180)).toEqual({ south: -85, west: -180, north: 85, east: 180 });
    // 南北・東西が逆、数でないものは null。
    expect(viewBounds(10, 0, 5, 1)).toBeNull();
    expect(viewBounds(0, 10, 1, 5)).toBeNull();
    expect(viewBounds(Number.NaN, 0, 1, 1)).toBeNull();
    expect(viewBounds(95, 0, 96, 1)).toBeNull();
  });

  it("問い合わせる範囲を広げる", () => {
    expect(padView(10, 100, 20, 120)).toEqual([7.5, 95, 22.5, 125]);
    expect(padView(0, 0, 4, 8, 0.5)).toEqual([-2, -4, 6, 12]);
  });

  it("印は地図の中心に近い世界の写しに置く", () => {
    expect(markerLon(-172, 175)).toBe(188);
    expect(markerLon(178, -175)).toBe(-182);
    expect(markerLon(139.7, 135)).toBe(139.7);
    expect(markerLon(139.7, 135 + 720)).toBe(139.7 + 720);
  });

  it("同じ範囲か・範囲の文", () => {
    const a = { south: 35, west: 139, north: 36, east: 140 };
    expect(sameBounds(a, { ...a, east: 140.0000001 })).toBe(true);
    expect(sameBounds(a, { ...a, east: 140.001 })).toBe(false);
    expect(sameBounds(null, undefined)).toBe(true);
    expect(sameBounds(a, null)).toBe(false);
    expect(describeBounds(a)).toBe("南西 35.0000, 139.0000 〜 北東 36.0000, 140.0000");
    expect(describeBounds({ south: -20, west: 170, north: -10, east: -170 })).toContain("経度 180 度の線をまたぐ");
  });
});

describe("地図の表示と印", () => {
  it("写真の全体・まとまりへ寄せる表示", () => {
    expect(extentView(null)).toBeNull();
    expect(extentView({ south: 35, west: 139, north: 35, east: 139 })).toEqual({
      kind: "point",
      center: { lat: 35, lon: 139 },
      zoom: PIN_ZOOM,
    });
    expect(extentView({ south: 34, west: 135, north: 36, east: 140 })).toEqual({
      kind: "bounds",
      south: 34,
      west: 135,
      north: 36,
      east: 140,
      maxZoom: FIT_MAX_ZOOM,
    });
    const one = { lat: 35.1, lon: 139.2, bounds: { south: 35.1, west: 139.2, north: 35.1, east: 139.2 } };
    expect(clusterView(one)).toEqual({ kind: "point", center: { lat: 35.1, lon: 139.2 }, zoom: PIN_ZOOM });
  });

  it("印の数の書式と大きさ", () => {
    expect([0, 9, 999, 1000, 1234, 9999, 10_000, 123_456].map(formatClusterCount)).toEqual([
      "0",
      "9",
      "999",
      "1.0k",
      "1.2k",
      "9.9k",
      "10k",
      "123k",
    ]);
    expect([1, 9, 10, 99, 100, 999, 1000].map(clusterSizeClass)).toEqual(["s1", "s1", "s2", "s2", "s3", "s3", "s4"]);
  });

  it("数の文", () => {
    const m: MapClusters = {
      generation: 3,
      zoom: 5,
      clusters: [],
      in_bounds: 12,
      located: 1234,
      unlocated: 0,
      extent: null,
      truncated: false,
    };
    expect(describeMapCounts(m)).toBe("位置のある写真 1,234 件（表示している付近に 12 件）");
    expect(describeMapCounts({ ...m, unlocated: 5, truncated: true })).toBe(
      "位置のある写真 1,234 件（表示している付近に 12 件）・位置のない写真 5 件（地図には出ません）・" +
        "印が多いため、写真の多い場所だけを出しています（拡大すると全部出ます）",
    );
  });
});
