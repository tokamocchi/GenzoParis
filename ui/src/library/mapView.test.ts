// 地図の表示（LIB-18。library/MapView.tsx）が、オンラインの地図の有効・無効で例外なく描けることの確認
// （react-dom/server で文字列にする。useEffect は動かないので、Leaflet は読まず、通信もしない）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";

import type { AppSettings, MapPreferences } from "../api/types";
import { appStore } from "../state/app";
import { DEFAULT_MAP_PREFERENCES, MAP_LINK_LABELS } from "./geoMapLogic";
import { MapView, resetMapViewForTest } from "./MapView";

function render(): string {
  return renderToString(createElement(MapView)).replace(/<!-- -->/g, "");
}

function setMap(map: Partial<MapPreferences>): void {
  const status = { settings: { map: { ...DEFAULT_MAP_PREFERENCES, ...map } } as unknown as AppSettings };
  appStore.set({ status: status as never });
}

afterEach(() => {
  appStore.set({ status: null });
  resetMapViewForTest();
});

describe("地図の表示（LIB-18）", () => {
  it("オンラインの地図が無効なら、地図を作らずに有効にする前の説明を出す", () => {
    setMap({ online: false });
    const html = render();
    expect(html).toContain("オンラインの地図（OpenStreetMap）を有効にしますか");
    expect(html).toContain("位置情報");
    expect(html).not.toContain("geo-map-canvas");
  });

  it("有効なら、地図の入れ物・操作の帯・帰属の表示を出す", () => {
    setMap({ online: true });
    const html = render();
    expect(html).toContain("geo-map-canvas");
    expect(html).toContain("地図の範囲で絞り込む");
    expect(html).toContain("写真の全体を表示");
    expect(html).toContain(MAP_LINK_LABELS.copyright);
    expect(html).toContain("地図を読み込んでいます");
  });
});
