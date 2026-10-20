// 位置情報の後付け（LIB-21）の入口（右クリックのメニュー・右パネルから開くダイアログの状態）・ダイアログと右パネルの
// GPS の行の描画・コアの命令の形のテスト（コアのコマンドは MockBridge で差し替える。部品は react-dom/server で
// 描くので useEffect は動かず、読み込みの前の状態を確かめる）。第 2 段階のオンラインの地図（有効・無効の描画・
// 有効にする前の説明・設定の画面）も確かめる（アプリのコマンドの形は state/map.test.ts）。地図（library/GeoMap.tsx）は
// Leaflet を useEffect の中で読むので、描いても Leaflet は読まない（地図の部品そのものはテストから import しない）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import * as core from "../api/core";
import {
  MOCK_GEOTAGGED_DETAILS,
  MOCK_LOCATION_EDITS,
  MOCK_LOCATION_PLAN,
  MOCK_MAP_PREFERENCES,
  mockAppStatus,
  MockBridge,
  mockSummary,
} from "../api/mock";
import type { VariantDetails } from "../api/types";
import {
  DEFAULT_MAP_PREFERENCES,
  DEFAULT_TILE_URL,
  PUBLIC_NOMINATIM_LABEL,
  PUBLIC_NOMINATIM_NOTE,
  SEARCH_UNAVAILABLE_NOTE,
} from "../library/geoMapLogic";
import { GpsValue } from "../library/MetadataPanel";
import { appStore } from "../state/app";
import { openGeotagDialog, runPhotoMenu } from "../state/stacks";
import { formatLocalDateTime } from "./dialogFormat";
import { Dialogs } from "./Dialogs";
import { GeotagDialog } from "./GeotagDialog";
import { MapConsentPanel } from "./MapConsent";
import { SettingsDialog } from "./SettingsDialog";

let mock: MockBridge;
/** core_execute で送ったコマンド。 */
let sent: (Record<string, unknown> & { type: string })[];

/** 文字列にする（React が文字の区切りに入れる <!-- --> は除く）。 */
function render(el: ReturnType<typeof createElement>): string {
  return renderToString(el).replace(/<!-- -->/g, "");
}

beforeEach(() => {
  mock = new MockBridge();
  installBridge(mock);
  sent = [];
  appStore.set({
    mode: "grid",
    dialog: null,
    toasts: [],
    selection: { ids: [3, 4], current: 4, currentIndex: 1, anchorIndex: 0, anchorId: 3 },
  });
  mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as Record<string, unknown> & { type: string };
    sent.push(c);
    switch (c.type) {
      case "plan_location_edit":
        return { type: "location_edit_plan", ...MOCK_LOCATION_PLAN };
      case "edit_locations":
        return { type: "location_edit_plan", ...MOCK_LOCATION_PLAN, edit_id: 5 };
      case "location_edits":
        return { type: "location_edits", edits: MOCK_LOCATION_EDITS };
      case "undo_location_edit":
        return { type: "location_undone", undone: { edit: MOCK_LOCATION_EDITS[0], restored: 1, removed: 0 } };
      default:
        return { type: "ok" };
    }
  });
});

describe("位置を付けるダイアログの入口（LIB-21）", () => {
  it("右クリックのメニューから開く（対象は選択中のすべて、表示中の写真も渡す）", async () => {
    await runPhotoMenu("edit_location");
    expect(appStore.get().dialog).toEqual({ kind: "geotag", variantIds: [3, 4], current: 4 });
  });

  it("ルーペでは表示中の 1 枚が対象", () => {
    appStore.set({ mode: "loupe" });
    openGeotagDialog();
    expect(appStore.get().dialog).toEqual({ kind: "geotag", variantIds: [4], current: 4 });
  });

  it("ダイアログの切り替え（Dialogs.tsx の case）", () => {
    appStore.set({ dialog: { kind: "geotag", variantIds: [3, 4], current: 4 } });
    const html = render(createElement(Dialogs));
    expect(html).toContain("位置を付ける");
    expect(html).toContain("座標（緯度, 経度）");
  });
});

describe("位置を付けるダイアログの描画", () => {
  it("読み込みの前: 注意・座標の欄・貼り付けられる形・付けた位置を消す・取り消し", () => {
    const html = render(createElement(GeotagDialog, { variantIds: [3, 4], current: 4 }));
    expect(html).toContain("元のファイルには書きません");
    expect(html).toContain("「位置情報（GPS）を削除する」を選ぶと入りません");
    expect(html).toContain("動画は対象外です");
    expect(html).toContain('placeholder="35.681236, 139.767125"');
    expect(html).toContain("貼り付けられる形");
    expect(html).toContain("外部には送りません");
    expect(html).toContain("座標を入れると、位置の変わる写真の数を出します。");
    expect(html).toContain("付けた位置を消す（カメラの位置に戻す）");
    expect(html).toContain("直前の変更を取り消す");
    expect(html).toContain("対象を数えています");
  });

  it("写真を選んでいないとき", () => {
    const html = render(createElement(GeotagDialog, { variantIds: [], current: null }));
    expect(html).toContain("写真を選んでください");
    expect(html).not.toContain("対象を数えています");
  });
});

describe("右パネルの GPS の行（MetadataPanel の GpsValue）", () => {
  const draw = (details: VariantDetails, over: { stale?: boolean; multi?: boolean } = {}) =>
    render(createElement(GpsValue, { details, stale: over.stale ?? false, multi: over.multi ?? false }));
  const noLocation: VariantDetails = { ...MOCK_GEOTAGGED_DETAILS, gps: null, gps_source: null, camera_gps: null };

  it("後から付けた位置: 使う位置・注記（カメラの位置も出す）・コピー・位置を付ける", () => {
    const html = draw(MOCK_GEOTAGGED_DETAILS);
    expect(html).toContain("北緯 35.681236°、東経 139.767125°");
    expect(html).toContain("後から付けた位置（カメラの位置: 北緯 35.658600°、東経 139.745400°）");
    expect(html).toContain("コピー");
    expect(html).toContain("位置を付ける…");
  });

  it("カメラの位置と古いコア（gps_source が無い）では注記を出さない", () => {
    const camera: VariantDetails = { ...MOCK_GEOTAGGED_DETAILS, gps: { lat: 35.6586, lon: 139.7454 }, gps_source: "camera" };
    const html = draw(camera);
    expect(html).toContain("北緯 35.658600°、東経 139.745400°");
    expect(html).not.toContain("後から付けた位置");
    const old: VariantDetails = { ...MOCK_GEOTAGGED_DETAILS };
    delete old.gps_source;
    delete old.camera_gps;
    expect(draw(old)).not.toContain("後から付けた位置");
  });

  it("位置がなければ「なし」で、コピーは出さない", () => {
    const html = draw(noLocation);
    expect(html).toContain("なし");
    expect(html).not.toContain("コピー");
    expect(html).toContain("位置を付ける…");
  });

  it("動画 1 本では位置を付けない（まとめて選んでいれば付けられる。動画はコアが除く）", () => {
    const video: VariantDetails = { ...noLocation, summary: mockSummary({ kind: "video" }) };
    expect(draw(video)).not.toContain("位置を付ける…");
    expect(draw(video, { multi: true })).toContain("位置を付ける…");
  });

  it("前の写真の詳細を出している間（stale）は、コピーも位置を付けるも使えない", () => {
    expect(draw(MOCK_GEOTAGGED_DETAILS, { stale: true }).match(/disabled=""/g)).toHaveLength(2);
    expect(draw(MOCK_GEOTAGGED_DETAILS)).not.toContain('disabled=""');
  });
});

describe("位置情報の後付けのコアの命令（genzo-api の JSON の形）", () => {
  const tokyo = { lat: 35.681236, lon: 139.767125 };

  it("見込みと実行は location_edit_plan を返す", async () => {
    expect(await core.planLocationEdit([1, 2], tokyo)).toEqual(MOCK_LOCATION_PLAN);
    expect(sent.at(-1)).toEqual({ type: "plan_location_edit", variant_ids: [1, 2], location: tokyo });
    const r = await core.editLocations([1, 2], tokyo);
    expect(r.edit_id).toBe(5);
    expect(sent.at(-1)).toEqual({ type: "edit_locations", variant_ids: [1, 2], location: tokyo });
  });

  it("消すときも location を null で送る（省略するとコアが誤りにする）", async () => {
    await core.editLocations([7], null);
    await core.planLocationEdit([7], null);
    for (const c of sent) {
      expect(JSON.stringify(c)).toContain('"location":null');
    }
    expect(sent.map((c) => c.type)).toEqual(["edit_locations", "plan_location_edit"]);
  });

  it("履歴と取り消し", async () => {
    expect(await core.locationEdits()).toEqual(MOCK_LOCATION_EDITS);
    expect(await core.undoLocationEdit()).toEqual({ edit: MOCK_LOCATION_EDITS[0], restored: 1, removed: 0 });
    expect(sent.map((c) => c.type)).toEqual(["location_edits", "undo_location_edit"]);
    // 履歴がなければ null。
    mock.responses.set("core_execute", () => ({ type: "location_undone", undone: null }));
    expect(await core.undoLocationEdit()).toBeNull();
  });
});


describe("位置を付けるダイアログのオンラインの地図（LIB-21 の第 2 段階）", () => {
  const dialog = () => render(createElement(GeotagDialog, { variantIds: [3, 4], current: 4 }));
  /** 有効で、地名の検索のサービス（検索サービスの URL）を設定した形。 */
  const SEARCHABLE = { ...MOCK_MAP_PREFERENCES, search_url: "https://geocoder.example.org/search" };

  it("無効（既定）: 地図は出さず、1 行の案内だけ（通信しない。座標の貼り付けは使える）", () => {
    for (const status of [mockAppStatus(DEFAULT_MAP_PREFERENCES), mockAppStatus(), null]) {
      appStore.set({ status });
      const html = dialog();
      expect(html).toContain("今は無効で、外部と通信しません");
      expect(html).toContain("オンラインの地図について…");
      expect(html).not.toContain("geo-map");
      expect(html).not.toContain("地名の検索");
      expect(html).toContain("座標（緯度, 経度）");
      expect(html).toContain("座標はこのパソコンの中で読み、外部には送りません。");
    }
  });

  it("有効: 地名の検索・地図（読み込みの前）・右下の帰属の表示・地図の誤りの報告・印の説明", () => {
    appStore.set({ status: mockAppStatus(SEARCHABLE) });
    const html = dialog();
    expect(html).toContain('aria-label="地名の検索"');
    expect(html).not.toContain(SEARCH_UNAVAILABLE_NOTE);
    expect(html).toContain("地名・駅名・施設名で検索");
    expect(html).toContain(">検索</button>");
    expect(html).toContain('class="geo-map"');
    expect(html).toContain("地図を読み込んでいます");
    expect(html).toContain('class="geo-attribution"');
    expect(html).toContain("© OpenStreetMap contributors");
    expect(html).toContain("地図の誤りを報告");
    expect(html).toContain("付ける位置（地図をクリックするか、ピンをドラッグして選びます）");
    expect(html).not.toContain("オンラインの地図について…");
    // 座標の欄の下の注記: 有効なときは「外部には送りません」と言い切らず、地図に出すとタイルを要求すると書く。
    expect(html).not.toContain("外部には送りません");
    expect(html).toContain("付近の地図のタイルを提供元（tile.openstreetmap.org）に要求します");
    // 座標の欄は地図の後ろ（地図で選んだ位置がここに入る）。
    expect(html.indexOf('class="geo-map"')).toBeLessThan(html.indexOf("座標（緯度, 経度）"));
    // OSM の標準のタイルでは、提供元の帰属の表示は出さない。
    expect(html).not.toContain("geo-credit");
  });

  it("有効で地名の検索のサービスがない（既定）: 検索の欄は出さず、案内を出す（地図と座標の欄は使える）", () => {
    appStore.set({ status: mockAppStatus(MOCK_MAP_PREFERENCES) });
    const html = dialog();
    expect(html).not.toContain('aria-label="地名の検索"');
    expect(html).not.toContain('type="search"');
    expect(html).toContain(SEARCH_UNAVAILABLE_NOTE);
    expect(html).toContain('class="geo-map"');
    expect(html).toContain("座標（緯度, 経度）");
    // 公開の Nominatim を使うと選ぶと、検索の欄を出す。
    appStore.set({ status: mockAppStatus({ ...MOCK_MAP_PREFERENCES, public_nominatim: true }) });
    const pub = dialog();
    expect(pub).toContain('aria-label="地名の検索"');
    expect(pub).not.toContain(SEARCH_UNAVAILABLE_NOTE);
  });

  it("有効で提供元を変えた: 右下に提供元の帰属の表示を、OSM の帰属の表示と並べて文字だけで出す", () => {
    const tile_url = "https://tiles.example.org/{z}/{x}/{y}.png";
    appStore.set({
      status: mockAppStatus({ ...MOCK_MAP_PREFERENCES, tile_url, tile_attribution: "Maps © <b>提供元</b>" }),
    });
    const html = dialog();
    const box = html.match(/<div class="geo-attribution">[\s\S]*?<\/div>/)?.[0] ?? "";
    expect(box).toContain('<span class="geo-credit"');
    // HTML として読まない（React の文字として出す）。リンクにもしない。
    expect(box).toContain("Maps © &lt;b&gt;提供元&lt;/b&gt;");
    expect(box).not.toContain("<b>");
    expect(box).toContain("© OpenStreetMap contributors");
    expect(box).toContain("地図の誤りを報告");
    // 帰属の表示が空なら出さない（OSM の帰属の表示だけ）。
    appStore.set({ status: mockAppStatus({ ...MOCK_MAP_PREFERENCES, tile_url, tile_attribution: null }) });
    const plain = dialog();
    expect(plain).not.toContain("geo-credit");
    expect(plain).toContain("© OpenStreetMap contributors");
  });

  it("開いたときのフォーカスは座標の欄（有効なときは地名の検索の欄が先にあるが、座標を貼って Enter で検索に送らない）", () => {
    for (const prefs of [SEARCHABLE, MOCK_MAP_PREFERENCES, DEFAULT_MAP_PREFERENCES]) {
      appStore.set({ status: mockAppStatus(prefs) });
      const html = dialog();
      // Dialog は data-autofocus の付いた欄を先に探す（components/Dialog.tsx の initialFocusTarget）。
      const marked = html.match(/<input[^>]*data-autofocus[^>]*>/g) ?? [];
      expect(marked).toHaveLength(1);
      expect(marked[0]).toContain('id="geo-coord"');
    }
    appStore.set({ status: mockAppStatus(SEARCHABLE) });
    const html = dialog();
    expect(html.indexOf('type="search"')).toBeGreaterThan(-1);
    expect(html.indexOf('type="search"')).toBeLessThan(html.indexOf('id="geo-coord"'));
  });

  it("有効にする前の説明: 送るもの・送り先・守ること・注意・利用ポリシーのページ", () => {
    const html = render(
      createElement(MapConsentPanel, {
        preferences: DEFAULT_MAP_PREFERENCES,
        busy: false,
        error: null,
        onEnable: () => undefined,
        onCancel: () => undefined,
      }),
    );
    expect(html).toContain("オンラインの地図（OpenStreetMap）を有効にしますか");
    expect(html).toContain("tile.openstreetmap.org");
    // 既定では公開の Nominatim に送らないので、送り先にも、利用ポリシーのボタンにも出さない。
    expect(html).not.toContain("nominatim.openstreetmap.org");
    expect(html).not.toContain("地名の検索（Nominatim）の利用ポリシー");
    expect(html).toContain("検索サービスを設定していないので、検索語は送りません");
    expect(html).toContain("1 秒以上あけ");
    expect(html).toContain("入力の途中では送りません");
    expect(html).toContain("個人の情報を検索語にしないでください");
    expect(html).toContain("タイルの利用ポリシー");
    expect(html).toContain("オンラインの地図を有効にする");
    expect(html).toContain("やめる");
    // 有効にするボタンは .primary にしない（Dialog の初めのフォーカスと Enter で有効にしない）。
    expect(html).not.toContain("btn primary");
    const noCancel = render(
      createElement(MapConsentPanel, {
        preferences: DEFAULT_MAP_PREFERENCES,
        busy: true,
        error: null,
        onEnable: () => undefined,
      }),
    );
    expect(noCancel).not.toContain("やめる");
    expect(noCancel).toContain("有効にしています…");
    // 公開の Nominatim を使うと選んでいれば、送り先と利用ポリシーのボタンに出す。
    const pub = render(
      createElement(MapConsentPanel, {
        preferences: { ...DEFAULT_MAP_PREFERENCES, public_nominatim: true },
        busy: false,
        error: null,
        onEnable: () => undefined,
      }),
    );
    expect(pub).toContain("nominatim.openstreetmap.org（OpenStreetMap Foundation の公開の Nominatim");
    expect(pub).toContain("地名の検索（Nominatim）の利用ポリシー");
  });

  it("設定の一般: オンラインの地図の節（有効・無効、キャッシュ、詳細の URL、利用ポリシー）", () => {
    appStore.set({ status: mockAppStatus(), catalog: null });
    const off = render(createElement(SettingsDialog, { tab: "general" }));
    expect(off).toContain("オンラインの地図（OpenStreetMap）");
    expect(off).toContain("オンラインの地図を使う");
    expect(off).toContain("初めて有効にするときに、送るものと送り先を確かめてもらいます");
    expect(off).toContain("この節の設定はすぐに効きます");
    expect(off).toContain("tile.openstreetmap.org");
    expect(off).toContain("キャッシュを消す");
    expect(off).toContain("200 MB を超えたら古いものから消します");
    expect(off).toContain("詳細（地図の提供元・プロキシ）");
    expect(off).toContain(`placeholder="${DEFAULT_TILE_URL}"`);
    expect(off).toContain("最大のズームは 19");
    // 提供元は PNG のタイルを配るものだけ（genzo-geo は PNG でない応答を受け取らない）。拡張子でわかるものは、
    // 保存のときにアプリが断る（genzo-geo の validate_tile_url_template）。
    expect(off).toContain("PNG のタイルを配る提供元だけ");
    expect(off).toContain("URL の拡張子が .jpg・.webp などなら、保存のときに断ります");
    // プロキシは環境変数と Windows の手動の設定だけを読む（PAC・自動検出・Windows 以外のシステムの設定は読まない。
    // genzo-geo の proxy.rs）。SOCKS は使えない。
    expect(off).toContain("環境変数（HTTPS_PROXY・ALL_PROXY・HTTP_PROXY と NO_PROXY）");
    expect(off).toContain("Windows の手動のプロキシの設定");
    expect(off).toContain("自動構成スクリプト（PAC）・設定の自動検出");
    expect(off).toContain("SOCKS のプロキシには対応していません");
    expect(off).toContain("地図のタイルの要求");
    expect(off).toContain("タイルの利用ポリシー");
    // OSM の公開の地名の検索（Nominatim）の切り替え（既定は OFF）と、近くの説明・利用ポリシーのボタン。
    const publicToggle = (html: string) =>
      html.match(/<label class="dlg-check"><input type="checkbox"[^>]*>(?:(?!<\/label>).)*OSM の公開の地名の検索/)?.[0] ?? "";
    expect(off).toContain(PUBLIC_NOMINATIM_LABEL);
    expect(publicToggle(off)).not.toBe("");
    expect(publicToggle(off)).not.toContain("checked");
    expect(off).toContain("既定は OFF です。OFF で検索サービスの URL もないときは、地名の検索は使えません");
    expect(off).toContain(PUBLIC_NOMINATIM_NOTE);
    expect(off).toContain("地名の検索（Nominatim）の利用ポリシー");
    expect(off.indexOf(PUBLIC_NOMINATIM_LABEL)).toBeLessThan(off.indexOf("地名の検索（Nominatim）の利用ポリシー"));
    // 検索の URL の欄に、公開の Nominatim を例に出さない。地名の検索は既定でなし。
    expect(off).toContain('<label for="map-search-url">検索サービスの URL</label>');
    expect(off).not.toContain('placeholder="https://nominatim.openstreetmap.org/search"');
    expect(off).toContain("地名の検索: なし（検索サービスを設定していないので、検索語は送りません）");
    appStore.set({ status: mockAppStatus({ ...MOCK_MAP_PREFERENCES, public_nominatim: true }), catalog: null });
    const pub = render(createElement(SettingsDialog, { tab: "general" }));
    expect(publicToggle(pub)).toContain('checked=""');
    expect(pub).toContain("nominatim.openstreetmap.org（OpenStreetMap Foundation の公開の Nominatim");
    appStore.set({
      status: mockAppStatus({ ...MOCK_MAP_PREFERENCES, public_nominatim: true, search_url: "https://geo.example.net/search" }),
      catalog: null,
    });
    const custom = render(createElement(SettingsDialog, { tab: "general" }));
    expect(custom).toContain("検索サービスの URL（geo.example.net）を指定しているので、今はそちらを使います");

    const tile_url = "https://tiles.example.org/{z}/{x}/{y}.png";
    appStore.set({ status: mockAppStatus({ ...MOCK_MAP_PREFERENCES, tile_url }) });
    const on = render(createElement(SettingsDialog, { tab: "general" }));
    // 日時は実行環境の時間帯で出す（UTC の 01:02:03 は時間帯によって前の日になるので、同じ関数で作った値と比べる）。
    const consented = MOCK_MAP_PREFERENCES.consented_at;
    expect(consented).not.toBeNull();
    expect(on).toContain(`初めて有効にした日時: ${formatLocalDateTime(consented ?? "")}`);
    expect(on).toMatch(/初めて有効にした日時: \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}/);
    expect(on).toContain("tiles.example.org（設定で変えた提供元）");
    expect(on).toContain('value="https://tiles.example.org/{z}/{x}/{y}.png"');
  });

  it("設定の一般: 提供元の帰属の表示の欄（タイルの URL を変えたときだけ使う。空なら案内する）", () => {
    const field = (html: string) => html.match(/<input[^>]*id="map-tile-attribution"[^>]*>/)?.[0] ?? "";
    const warning = "タイルの提供元の帰属の表示が空です";

    // OSM の標準（タイルの URL が空）: 欄は入力できず、案内も出さない。
    appStore.set({ status: mockAppStatus(), catalog: null });
    const osm = render(createElement(SettingsDialog, { tab: "general" }));
    expect(osm).toContain('<label for="map-tile-attribution">地図の帰属の表示</label>');
    expect(field(osm)).toContain('disabled=""');
    expect(field(osm)).toContain('placeholder="タイルの URL を変えたときに入れます"');
    expect(osm).not.toContain(warning);
    // 帰属の表示は「OSM と同じとみなす」のではなく、OSM の帰属の表示に並べて出す。
    expect(osm).not.toContain("帰属の表示は「© OpenStreetMap contributors」とみなします");
    expect(osm).toContain("「© OpenStreetMap contributors」を常に出し");

    // タイルの URL を変えたのに帰属の表示が空: 案内する。
    const tile_url = "https://tiles.example.org/{z}/{x}/{y}.png";
    appStore.set({ status: mockAppStatus({ ...MOCK_MAP_PREFERENCES, tile_url }), catalog: null });
    const missing = render(createElement(SettingsDialog, { tab: "general" }));
    expect(field(missing)).not.toContain("disabled");
    expect(missing).toContain(warning);

    // 帰属の表示がある: 欄に出し、案内は出さない。
    appStore.set({
      status: mockAppStatus({ ...MOCK_MAP_PREFERENCES, tile_url, tile_attribution: "Maps © 提供元" }),
      catalog: null,
    });
    const credited = render(createElement(SettingsDialog, { tab: "general" }));
    expect(field(credited)).toContain('value="Maps © 提供元"');
    expect(field(credited)).not.toContain("disabled");
    expect(credited).not.toContain(warning);
  });
});
