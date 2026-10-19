// 位置情報の後付け（LIB-21）の第 2 段階（オンラインの地図と地名の検索）の画面の計算（DOM・Leaflet を使わない
// 純粋な計算。docs/04 の 3.16 節）。地図の部品は library/GeoMap.tsx（Leaflet を動的に読む。テストからは読まない）。
//
// - 設定（MapPreferences）の既定値と重ね方・送り先の説明・有効にする前の説明（同意の文）。
// - 地名の検索の間隔（画面でも前の要求の開始から 1 秒あける。genzo-geo もプロセス全体で守る）・検索語の検査・
//   結果の文。
// - 地図の座標（クリックした位置の経度を -180〜180 に戻す）・初めの表示・検索の結果へ動かすときの範囲。
//
// OSM Foundation の利用ポリシー（タイル: https://operations.osmfoundation.org/policies/tiles/ 、Nominatim:
// https://operations.osmfoundation.org/policies/nominatim/ ）のうち、画面が受け持つのは次のこと（通信の規則は
// genzo-geo、有効にするまで通信しないことは genzo-app）:
// - 地名の検索は Enter か「検索」のボタンのときだけ（入力の途中で検索しない。オートコンプリートの禁止）。
// - 帰属の表示「© OpenStreetMap contributors」を地図の右下に常に出す（リンクは WebView ではなく既定のブラウザで開く）。
// - 先読み・まとめての取得をしない（Leaflet の既定の、表示している範囲のタイルだけを取る）。

import type { AppSettings, GpsCoord, MapCacheInfo, MapLink, MapPreferences, Place } from "../api/types";
import { formatBytes } from "../format";

/** 地図の最大のズーム（genzo-geo の OSM_MAX_ZOOM と同じ。設定で変えた提供元も同じとみなす。仮置き）。 */
export const MAP_MAX_ZOOM = 19;

/** 地図の帰属の表示（genzo-geo の OSM_ATTRIBUTION と同じ。設定で変えた提供元も OSM のデータとみなす）。 */
export const MAP_ATTRIBUTION = "© OpenStreetMap contributors";

/** 地名の検索の間隔（ms。前の要求の開始から。Nominatim のポリシーの「1 秒に 1 回まで」）。 */
export const SEARCH_INTERVAL_MS = 1000;

/** 検索語の長さの上限（文字数。genzo-geo の MAX_QUERY_CHARS と同じ。仮置き）。 */
export const MAX_QUERY_CHARS = 200;

/** タイルのキャッシュの容量の上限（genzo-geo の DEFAULT_CACHE_LIMIT_BYTES と同じ 200 MiB。仮置き）。 */
export const MAP_CACHE_LIMIT_BYTES = 200 * 1024 * 1024;

/** 既定のタイルの URL（genzo-geo の OSM_TILE_URL。設定の欄の例に出す）。 */
export const DEFAULT_TILE_URL = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";

/** 既定の地名の検索の URL（genzo-geo の NOMINATIM_SEARCH_URL。設定の欄の例に出す）。 */
export const DEFAULT_SEARCH_URL = "https://nominatim.openstreetmap.org/search";

/** 位置がないときの初めの表示（日本の中ほど。仮置き）。 */
export const DEFAULT_MAP_CENTER: GpsCoord = { lat: 36.2, lon: 138.25 };
/** 位置がないときの初めのズーム（日本の本州が入る程度。仮置き）。 */
export const DEFAULT_MAP_ZOOM = 5;
/** 付ける位置・表示中の写真の位置を中心に開くときのズーム（仮置き）。 */
export const PIN_ZOOM = 15;
/** 範囲のない検索の結果へ動かすときのズーム（仮置き）。 */
export const PLACE_ZOOM = 16;
/** 検索の結果の範囲に合わせるときの最大のズーム（小さい範囲で寄りすぎない。仮置き）。 */
export const FIT_MAX_ZOOM = 17;

/** 既定のブラウザで開くページの見出し（genzo-app の MapLink）。 */
export const MAP_LINK_LABELS: Record<MapLink, string> = {
  copyright: MAP_ATTRIBUTION,
  fix_the_map: "地図の誤りを報告",
  tile_policy: "タイルの利用ポリシー",
  nominatim_policy: "地名の検索（Nominatim）の利用ポリシー",
};

/** 地図のタイルを読めないときの文。 */
export const MAP_TILE_ERROR =
  "地図のタイルを読み込めません。ネットワークの接続とプロキシの設定を確かめてください（続くときは、設定の地図の提供元を確かめてください）。";

// ---------------------------------------------------------------------------
// 設定
// ---------------------------------------------------------------------------

/** 設定の既定値（genzo-app の MapPreferences の Default。オンラインの地図は無効）。 */
export const DEFAULT_MAP_PREFERENCES: MapPreferences = {
  online: false,
  consented_at: null,
  tile_url: null,
  search_url: null,
};

/** アプリの設定の地図の欄（古いアプリでは無いので既定値。欠けた欄も既定値）。 */
export function mapPreferencesOf(settings: Pick<AppSettings, "map"> | null | undefined): MapPreferences {
  return { ...DEFAULT_MAP_PREFERENCES, ...(settings?.map ?? {}) };
}

/**
 * 今の設定に変える欄を重ねる（保存のコマンドは欄をすべて受け取り、欠けた欄を既定値にするので、有効にするときに
 * 設定した URL を消さないため）。
 */
export function withMapPreferences(current: MapPreferences, patch: Partial<MapPreferences>): MapPreferences {
  return { ...current, ...patch };
}

/** URL の欄の値（前後の空白を除き、空なら null（既定の提供元））。形はアプリが確かめる。 */
export function urlFieldValue(text: string): string | null {
  const t = text.trim();
  return t === "" ? null : t;
}

/** URL のホスト名（読めなければ URL のまま）。 */
function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

/** 送り先（ホスト名）。`osm` は、どちらも OpenStreetMap Foundation のサーバー（既定）か。 */
export interface MapDestinations {
  tiles: string;
  search: string;
  osm: boolean;
}

export function mapDestinations(p: MapPreferences): MapDestinations {
  return {
    tiles: hostOf(p.tile_url ?? DEFAULT_TILE_URL),
    search: hostOf(p.search_url ?? DEFAULT_SEARCH_URL),
    osm: p.tile_url === null && p.search_url === null,
  };
}

/** 送り先の文。 */
export function describeDestinations(p: MapPreferences): string {
  const d = mapDestinations(p);
  if (d.osm) return `OpenStreetMap Foundation のサーバー（地図のタイル: ${d.tiles}、地名の検索: ${d.search}）`;
  const tiles = p.tile_url === null ? `${d.tiles}（OpenStreetMap Foundation）` : `${d.tiles}（設定で変えた提供元）`;
  const search = p.search_url === null ? `${d.search}（OpenStreetMap Foundation）` : `${d.search}（設定で変えた提供元）`;
  return `地図のタイル: ${tiles}、地名の検索: ${search}`;
}

/** オンラインの地図を有効にする前の説明（位置を付けるダイアログと設定の画面で同じものを出す）。 */
export interface MapConsentText {
  /** 外部に送るもの。 */
  sends: string[];
  /** 送り先。 */
  destination: string;
  /** 利用ポリシーに沿って、アプリが守ること。 */
  rules: string[];
  /** 利用者に気をつけてほしいこと。 */
  cautions: string[];
}

export function mapConsentText(p: MapPreferences): MapConsentText {
  return {
    sends: [
      "地図に表示している範囲の地図のタイルの要求（ズームと位置から、見ている場所が分かります）",
      "地名の検索に入れた語",
      "通信のときに、この PC の IP アドレスと、アプリの名前と版（User-Agent）が送り先に伝わります。写真とカタログの中身は送りません（座標の貼り付けは、このパソコンの中だけで読みます）",
    ],
    destination: describeDestinations(p),
    rules: [
      "地名の検索は、Enter か「検索」のボタンを押したときだけ送ります（入力の途中では送りません）。続けて検索するときは 1 秒以上あけ、同じ語の結果は覚えておいて問い合わせ直しません",
      "地図のタイルは、表示している範囲の分だけを取り、このパソコンにキャッシュします（提供元のキャッシュの指示に従います）。先読みや、まとめての取得はしません",
      "地図の右下に「© OpenStreetMap contributors」を表示します",
    ],
    cautions: [
      "自宅の住所など、個人の情報を検索語にしないでください（OSM の利用ポリシーは、個人の情報を送らないよう求めています）",
      "OSM のサーバーは寄付と協賛で運営されていて容量に限りがあり、使えることの保証はありません（予告なく使えなくなることがあります）。使えないときは、設定の「一般」で地図の提供元を変えられます",
      "無効のままでも、座標の貼り付けで位置を付けられます。有効にした後も、設定の「一般」でいつでも無効にできます",
    ],
  };
}

// ---------------------------------------------------------------------------
// 地名の検索
// ---------------------------------------------------------------------------

/** 検索語を整える（前後の空白を除き NFC にする。genzo-geo と同じ。結果のキャッシュのキーをそろえる）。 */
export function normalizeSearchQuery(q: string): string {
  return q.trim().normalize("NFC");
}

/** 検索語の誤り（空・長すぎる。文字数は Unicode の文字で数える）。なければ null。genzo-geo も確かめる。 */
export function searchQueryProblem(q: string): string | null {
  const key = normalizeSearchQuery(q);
  if (key === "") return "検索語が空です";
  if ([...key].length > MAX_QUERY_CHARS) return `検索語が長すぎます（${MAX_QUERY_CHARS} 文字まで）`;
  return null;
}

/** 前の検索の開始（ms。なければ null）から SEARCH_INTERVAL_MS あけるために、今（`now`）から待つ時間（ms）。 */
export function searchWaitMs(lastStart: number | null, now: number): number {
  if (lastStart === null) return 0;
  return Math.max(0, Math.min(SEARCH_INTERVAL_MS, lastStart + SEARCH_INTERVAL_MS - now));
}

/** 検索の結果の数の文。 */
export function describePlaces(places: readonly Place[]): string {
  return places.length === 0
    ? "見つかりませんでした（別の語や、市区町村の名前を足して検索してください）。"
    : `${places.length} 件見つかりました。選ぶと地図をその場所へ動かし、ピンを置きます。`;
}

/** 検索の結果の 1 件の見出しと説明（名前がなければ表示用の名前の最初の区切りまで）。 */
export function placeLabel(p: Place): { title: string; detail: string } {
  const title = p.name.trim() || p.display_name.split(",")[0].trim() || p.display_name;
  return { title, detail: p.display_name === title ? "" : p.display_name };
}

// ---------------------------------------------------------------------------
// 地図の座標と表示
// ---------------------------------------------------------------------------

/** 地図の表示（範囲か、中心とズーム）。 */
export type MapView =
  | { kind: "bounds"; south: number; west: number; north: number; east: number; maxZoom: number }
  | { kind: "point"; center: GpsCoord; zoom: number };

/** 検索の結果へ動かすときの表示（範囲があれば範囲、なければ・幅がなければ点と PLACE_ZOOM）。 */
export function placeView(p: Place): MapView {
  const b = p.bbox;
  if (b) {
    const [south, north, west, east] = b;
    if (south < north || west < east) return { kind: "bounds", south, west, north, east, maxZoom: FIT_MAX_ZOOM };
  }
  return { kind: "point", center: { lat: p.lat, lon: p.lon }, zoom: PLACE_ZOOM };
}

/** 地図の初めの表示（付ける位置、なければ表示中の写真の位置を中心に。どちらもなければ既定）。 */
export function initialMapView(
  pin: GpsCoord | null,
  photo: GpsCoord | null,
): { center: GpsCoord; zoom: number; located: boolean } {
  const at = pin ?? photo;
  return at
    ? { center: at, zoom: PIN_ZOOM, located: true }
    : { center: DEFAULT_MAP_CENTER, zoom: DEFAULT_MAP_ZOOM, located: false };
}

/**
 * 地図の座標（Leaflet の緯度・経度）を位置にする。経度は世界の写し（左右に続く地図）の分を戻して -180〜180 に、
 * 緯度は -90〜90 に収める（-0 は 0）。
 */
export function mapPointToGps(lat: number, lng: number): GpsCoord {
  const lon = lng >= -180 && lng <= 180 ? lng : ((((lng + 180) % 360) + 360) % 360) - 180;
  return { lat: Math.min(90, Math.max(-90, lat)) + 0, lon: lon + 0 };
}

// ---------------------------------------------------------------------------
// キャッシュ
// ---------------------------------------------------------------------------

/** タイルのキャッシュの大きさの文。 */
export function describeMapCache(info: MapCacheInfo): string {
  if (info.files === 0) return "キャッシュはありません";
  return `${formatBytes(info.bytes)}（ファイル ${info.files.toLocaleString("ja-JP")} 個）`;
}
