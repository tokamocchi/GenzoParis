// オンラインの地図と地名の検索（位置情報の後付け LIB-21 の第 2 段階）の画面の計算のテスト（Leaflet を読まない）。

import { afterEach, describe, expect, it } from "vitest";

import { MOCK_PLACES } from "../api/mock";
import type { Place } from "../api/types";
import { setWindowsStyleForTest, tileUrlTemplate } from "../api/uri";
import {
  DEFAULT_MAP_CENTER,
  DEFAULT_MAP_PREFERENCES,
  DEFAULT_MAP_ZOOM,
  describeDestinations,
  describeMapCache,
  describePlaces,
  FIT_MAX_ZOOM,
  initialMapView,
  MAP_ATTRIBUTION,
  MAP_LINK_LABELS,
  MAP_MAX_ZOOM,
  mapConsentText,
  mapDestinations,
  mapPointToGps,
  mapPreferencesOf,
  MAX_QUERY_CHARS,
  normalizeSearchQuery,
  PIN_ZOOM,
  PLACE_ZOOM,
  placeLabel,
  placeView,
  SEARCH_INTERVAL_MS,
  searchQueryProblem,
  searchWaitMs,
  urlFieldValue,
  withMapPreferences,
} from "./geoMapLogic";
import { parseCoordinates } from "./geotagLogic";
import { gpsDecimal } from "./metadataLogic";

afterEach(() => setWindowsStyleForTest(null));

describe("地図のタイルの URL（genzo-app の protocol.rs の /tile/{z}/{x}/{y}）", () => {
  it("macOS・Linux は genzo://localhost、Windows は http://genzo.localhost。拡張子は付けない", () => {
    setWindowsStyleForTest(false);
    expect(tileUrlTemplate()).toBe("genzo://localhost/tile/{z}/{x}/{y}");
    setWindowsStyleForTest(true);
    expect(tileUrlTemplate()).toBe("http://genzo.localhost/tile/{z}/{x}/{y}");
  });

  it("値は genzo-geo の定数と同じ（最大のズーム 19・帰属の表示・検索の間隔 1 秒・検索語 200 文字）", () => {
    expect(MAP_MAX_ZOOM).toBe(19);
    expect(MAP_ATTRIBUTION).toBe("© OpenStreetMap contributors");
    expect(MAP_LINK_LABELS.copyright).toBe(MAP_ATTRIBUTION);
    expect(SEARCH_INTERVAL_MS).toBe(1000);
    expect(MAX_QUERY_CHARS).toBe(200);
  });
});

describe("オンラインの地図の設定", () => {
  it("古いアプリ（map が無い）・欠けた欄は既定値（無効・OSM の既定の提供元）", () => {
    expect(mapPreferencesOf(undefined)).toEqual(DEFAULT_MAP_PREFERENCES);
    expect(mapPreferencesOf({})).toEqual({ online: false, consented_at: null, tile_url: null, search_url: null });
    expect(mapPreferencesOf({ map: { ...DEFAULT_MAP_PREFERENCES, online: true } }).online).toBe(true);
  });

  it("変える欄だけを重ねる（有効にしても、設定した提供元の URL を消さない）", () => {
    const tile_url = "https://tiles.example.org/{z}/{x}/{y}.png";
    const cur = { online: false, consented_at: null, tile_url, search_url: null };
    expect(withMapPreferences(cur, { online: true })).toEqual({ ...cur, online: true });
    expect(withMapPreferences(cur, { tile_url: null })).toEqual({ ...cur, tile_url: null });
  });

  it("URL の欄は前後の空白を除き、空なら null（既定の提供元）", () => {
    expect(urlFieldValue("   ")).toBeNull();
    expect(urlFieldValue("")).toBeNull();
    expect(urlFieldValue(" https://a.example/{z}/{x}/{y} ")).toBe("https://a.example/{z}/{x}/{y}");
  });

  it("送り先: 既定は OSM Foundation のサーバー、変えた提供元はそのホスト", () => {
    expect(mapDestinations(DEFAULT_MAP_PREFERENCES)).toEqual({
      tiles: "tile.openstreetmap.org",
      search: "nominatim.openstreetmap.org",
      osm: true,
    });
    expect(describeDestinations(DEFAULT_MAP_PREFERENCES)).toBe(
      "OpenStreetMap Foundation のサーバー（地図のタイル: tile.openstreetmap.org、地名の検索: nominatim.openstreetmap.org）",
    );
    const custom = { ...DEFAULT_MAP_PREFERENCES, tile_url: "https://tiles.example.org:8443/osm/{z}/{x}/{y}.png" };
    expect(mapDestinations(custom)).toEqual({
      tiles: "tiles.example.org:8443",
      search: "nominatim.openstreetmap.org",
      osm: false,
    });
    expect(describeDestinations(custom)).toBe(
      "地図のタイル: tiles.example.org:8443（設定で変えた提供元）、地名の検索: nominatim.openstreetmap.org（OpenStreetMap Foundation）",
    );
    // 読めない URL（アプリが保存を断るので通常はない）はそのまま出す。
    expect(mapDestinations({ ...DEFAULT_MAP_PREFERENCES, search_url: "not a url" }).search).toBe("not a url");
  });

  it("有効にする前の説明: 送るもの・送り先・守ること（1 秒・入力の途中で送らない・キャッシュ・先読みしない）・注意", () => {
    const t = mapConsentText(DEFAULT_MAP_PREFERENCES);
    const all = [...t.sends, t.destination, ...t.rules, ...t.cautions].join("\n");
    expect(t.destination).toBe(describeDestinations(DEFAULT_MAP_PREFERENCES));
    expect(all).toContain("地図のタイルの要求");
    expect(all).toContain("地名の検索に入れた語");
    expect(all).toContain("IP アドレス");
    expect(all).toContain("入力の途中では送りません");
    expect(all).toContain("1 秒以上あけ");
    expect(all).toContain("問い合わせ直しません");
    expect(all).toContain("先読みや、まとめての取得はしません");
    expect(all).toContain("© OpenStreetMap contributors");
    expect(all).toContain("個人の情報を検索語にしないでください");
    expect(all).toContain("座標の貼り付けで位置を付けられます");
    const custom = mapConsentText({ ...DEFAULT_MAP_PREFERENCES, search_url: "https://geo.example.net/search" });
    expect(custom.destination).toContain("geo.example.net（設定で変えた提供元）");
  });
});

describe("地名の検索（Enter かボタンのときだけ。1 秒に 1 回）", () => {
  it("検索語は前後の空白を除き NFC にする（genzo-geo と同じ。結果のキャッシュのキーをそろえる）", () => {
    expect(normalizeSearchQuery("  東京駅 ")).toBe("東京駅");
    // 「が」を「か」＋濁点（U+3099）で書いたもの。
    expect(normalizeSearchQuery("が")).toBe("が");
  });

  it("空・長すぎる検索語は送らない（文字数は NFC にした後の Unicode の文字で数える）", () => {
    expect(searchQueryProblem("")).toBe("検索語が空です");
    expect(searchQueryProblem(" 　 ")).toBe("検索語が空です");
    expect(searchQueryProblem("あ".repeat(MAX_QUERY_CHARS))).toBeNull();
    expect(searchQueryProblem("あ".repeat(MAX_QUERY_CHARS + 1))).toBe("検索語が長すぎます（200 文字まで）");
    // サロゲートペア（UTF-16 では 2 つ）も 1 文字。
    expect(searchQueryProblem("𠮷".repeat(MAX_QUERY_CHARS))).toBeNull();
    // 濁点を分けて書いた 200 文字は、NFC にすると 200 文字。
    expect(searchQueryProblem("が".repeat(MAX_QUERY_CHARS))).toBeNull();
  });

  it("前の要求の開始から 1 秒あける", () => {
    expect(searchWaitMs(null, 5_000)).toBe(0);
    expect(searchWaitMs(1_000, 1_000)).toBe(1_000);
    expect(searchWaitMs(1_000, 1_400)).toBe(600);
    expect(searchWaitMs(1_000, 2_000)).toBe(0);
    expect(searchWaitMs(1_000, 9_000)).toBe(0);
    // 時計が戻っても 1 秒より長くは待たない。
    expect(searchWaitMs(1_000, 500)).toBe(1_000);
  });

  it("結果の文と、1 件の見出し（名前がなければ表示用の名前の最初の区切り）", () => {
    expect(describePlaces([])).toContain("見つかりませんでした");
    expect(describePlaces(MOCK_PLACES)).toBe("2 件見つかりました。選ぶと地図をその場所へ動かし、ピンを置きます。");
    expect(placeLabel(MOCK_PLACES[0])).toEqual({ title: "東京駅", detail: "東京駅, 丸の内一丁目, 千代田区, 東京都, 日本" });
    expect(placeLabel(MOCK_PLACES[1])).toEqual({ title: "東京駅前", detail: "東京駅前, 八重洲一丁目, 中央区, 東京都, 日本" });
    const same: Place = { ...MOCK_PLACES[0], name: "日本", display_name: "日本" };
    expect(placeLabel(same)).toEqual({ title: "日本", detail: "" });
  });

  it("結果へ動かす: 範囲があれば範囲（[最小緯度, 最大緯度, 最小経度, 最大経度] の並び）、なければ点", () => {
    expect(placeView(MOCK_PLACES[0])).toEqual({
      kind: "bounds",
      south: 35.6785,
      north: 35.6838,
      west: 139.7637,
      east: 139.7706,
      maxZoom: FIT_MAX_ZOOM,
    });
    expect(placeView(MOCK_PLACES[1])).toEqual({
      kind: "point",
      center: { lat: 35.6807, lon: 139.7702 },
      zoom: PLACE_ZOOM,
    });
    // 幅のない範囲は点。
    const dot: Place = { ...MOCK_PLACES[0], bbox: [35.68, 35.68, 139.76, 139.76] };
    expect(placeView(dot)).toEqual({ kind: "point", center: { lat: 35.681236, lon: 139.767125 }, zoom: PLACE_ZOOM });
  });
});

describe("地図の座標と表示", () => {
  const tokyo = { lat: 35.681236, lon: 139.767125 };
  const camera = { lat: 35.6586, lon: 139.7454 };

  it("初めの表示: 付ける位置、なければ表示中の写真の位置、どちらもなければ既定", () => {
    expect(initialMapView(tokyo, camera)).toEqual({ center: tokyo, zoom: PIN_ZOOM, located: true });
    expect(initialMapView(null, camera)).toEqual({ center: camera, zoom: PIN_ZOOM, located: true });
    expect(initialMapView(null, null)).toEqual({ center: DEFAULT_MAP_CENTER, zoom: DEFAULT_MAP_ZOOM, located: false });
  });

  it("クリックした位置: 経度は世界の写しの分を戻して -180〜180、緯度は -90〜90、-0 は 0", () => {
    expect(mapPointToGps(35.5, 139.5)).toEqual({ lat: 35.5, lon: 139.5 });
    expect(mapPointToGps(35.5, 139.5 + 360)).toEqual({ lat: 35.5, lon: 139.5 });
    expect(mapPointToGps(35.5, 139.5 - 720)).toEqual({ lat: 35.5, lon: 139.5 });
    expect(mapPointToGps(-33.5, -200)).toEqual({ lat: -33.5, lon: 160 });
    expect(mapPointToGps(0, 180)).toEqual({ lat: 0, lon: 180 });
    expect(mapPointToGps(0, -180)).toEqual({ lat: 0, lon: -180 });
    expect(mapPointToGps(95, 0).lat).toBe(90);
    expect(mapPointToGps(-95, 0).lat).toBe(-90);
    const zero = mapPointToGps(-0, -0);
    expect(Object.is(zero.lat, 0) && Object.is(zero.lon, 0)).toBe(true);
  });

  it("クリックした位置は、座標の欄に入れた文字（小数 6 桁）から同じ位置として読める", () => {
    for (const [lat, lng] of [
      [35.68123649, 139.76712551],
      [-33.85678, 151.21529 + 360],
      [0.0000004, -179.9999996],
      [85.0511287, 181.25],
    ]) {
      const g = mapPointToGps(lat, lng);
      const r = parseCoordinates(gpsDecimal(g));
      expect(r.ok).toBe(true);
      if (r.ok) {
        expect(Math.abs(r.coord.lat - g.lat)).toBeLessThan(1e-6);
        expect(Math.abs(r.coord.lon - g.lon)).toBeLessThan(1e-6);
      }
    }
  });

  it("キャッシュの大きさの文", () => {
    expect(describeMapCache({ files: 0, bytes: 0 })).toBe("キャッシュはありません");
    expect(describeMapCache({ files: 240, bytes: 3_276_800 })).toBe("3.13 MB（ファイル 240 個）");
    expect(describeMapCache({ files: 12_345, bytes: 512 })).toBe("512 B（ファイル 12,345 個）");
  });
});
