// オンラインの地図（位置情報の後付け LIB-21 の第 2 段階）の設定の保存とアプリのコマンドの引数の形のテスト
// （genzo-app の map.rs。コマンドは MockBridge で差し替える。本物の通信はしない）。

import { beforeEach, describe, expect, it } from "vitest";

import * as app from "../api/app";
import { installBridge } from "../api/bridge";
import { MOCK_MAP_CACHE, MOCK_PLACES, mockAppStatus, MockBridge, mockMapCommands } from "../api/mock";
import { DEFAULT_MAP_PREFERENCES } from "../library/geoMapLogic";
import { appStore } from "./app";
import { currentMapPreferences, openMapPage, saveMapPreferences } from "./map";

let mock: MockBridge;

beforeEach(() => {
  mock = new MockBridge();
  installBridge(mock);
  appStore.set({ status: mockAppStatus(), toasts: [] });
});

describe("オンラインの地図のアプリのコマンド（genzo-app の map.rs。引数の形）", () => {
  beforeEach(() => {
    mockMapCommands(mock, mockAppStatus().settings);
  });

  const args = (command: string) => mock.calls.filter((c) => c.command === command).map((c) => c.args);

  it("設定の保存は、今の設定に変える欄を重ねてすべて送り、返った設定でアプリの状態を置き換える", async () => {
    const tile = "https://tiles.example.org/{z}/{x}/{y}.png";
    appStore.set({ status: mockAppStatus({ ...DEFAULT_MAP_PREFERENCES, tile_url: tile }) });
    const saved = await saveMapPreferences({ online: true });
    expect(args("save_map_preferences")).toEqual([
      { preferences: { online: true, consented_at: null, tile_url: tile, search_url: null, tile_attribution: null } },
    ]);
    expect(saved.map?.online).toBe(true);
    expect(appStore.get().status?.settings.map).toEqual({
      online: true,
      consented_at: "2026-10-20T01:02:03Z",
      tile_url: tile,
      search_url: null,
      tile_attribution: null,
    });
    // 古いアプリの形（map が無い）からも、欄をすべて送る。
    appStore.set({ status: mockAppStatus() });
    await saveMapPreferences({ online: false });
    expect(args("save_map_preferences").at(-1)).toEqual({ preferences: DEFAULT_MAP_PREFERENCES });
  });

  it("有効・無効を切り替えても、設定した提供元の帰属の表示を消さない（コマンドは欠けた欄を既定値にする）", async () => {
    const prefs = {
      ...DEFAULT_MAP_PREFERENCES,
      tile_url: "https://tiles.example.org/{z}/{x}/{y}.png",
      tile_attribution: "Maps © 提供元",
    };
    appStore.set({ status: mockAppStatus(prefs) });
    await saveMapPreferences({ online: true });
    expect(args("save_map_preferences")).toEqual([{ preferences: { ...prefs, online: true } }]);
    expect(currentMapPreferences().tile_attribution).toBe("Maps © 提供元");
    await saveMapPreferences({ online: false });
    expect(args("save_map_preferences").at(-1)).toEqual({
      preferences: { ...prefs, online: false, consented_at: "2026-10-20T01:02:03Z" },
    });
  });

  it("検索・キャッシュ・ページを開く", async () => {
    expect(await app.searchPlaces("東京駅")).toEqual(MOCK_PLACES);
    expect(args("search_places")).toEqual([{ query: "東京駅" }]);
    expect(await app.mapCacheInfo()).toEqual(MOCK_MAP_CACHE);
    expect(await app.clearMapCache()).toEqual({ files: 0, bytes: 0 });
    expect(args("map_cache_info")).toEqual([undefined]);
    expect(args("clear_map_cache")).toEqual([undefined]);
    await app.openMapLink("fix_the_map");
    await app.openMapLink("copyright");
    expect(args("open_map_link")).toEqual([{ kind: "fix_the_map" }, { kind: "copyright" }]);
  });
});

describe("オンラインの地図の今の設定", () => {
  it("古いアプリ（map が無い）・状態がまだないときは既定値（無効）", () => {
    expect(currentMapPreferences()).toEqual(DEFAULT_MAP_PREFERENCES);
    appStore.set({ status: null });
    expect(currentMapPreferences()).toEqual(DEFAULT_MAP_PREFERENCES);
  });

  it("ページを開けなければ知らせる", async () => {
    mock.responses.set("open_map_link", () => new Error("既定のブラウザがありません"));
    openMapPage("tile_policy");
    await new Promise((r) => setTimeout(r, 0));
    expect(mock.calls.map((c) => c.args)).toEqual([{ kind: "tile_policy" }]);
    expect(appStore.get().toasts.length).toBe(1);
  });
});
