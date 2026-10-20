// 位置情報の後付け（LIB-21）の第 2 段階（オンラインの地図と地名の検索）の画面の計算（DOM・Leaflet を使わない
// 純粋な計算。docs/04 の 3.16 節）。地図の部品は library/GeoMap.tsx（Leaflet を動的に読む。テストからは読まない）。
//
// - 設定（MapPreferences）の既定値と重ね方・設定の画面の提供元の欄・地名の検索の送り先（searchService。既定は
//   なし）・送り先の説明・有効にする前の説明（同意の文）・設定で変えたタイルの提供元の帰属の表示。
// - 地名の検索の間隔（画面でも前の要求の開始から 1 秒あける。genzo-geo もプロセス全体で守る）・検索語の検査・
//   結果の文。
// - 地図の座標（クリックした位置の経度を -180〜180 に戻す）・初めの表示・検索の結果へ動かすときの範囲・座標の欄と
//   地図を動かす先（打つ途中の値では動かさない。coordFieldReducer）。
//
// OSM Foundation の利用ポリシー（タイル: https://operations.osmfoundation.org/policies/tiles/ 、Nominatim:
// https://operations.osmfoundation.org/policies/nominatim/ ）のうち、画面が受け持つのは次のこと（通信の規則は
// genzo-geo、有効にするまで通信しないことは genzo-app）:
// - 地名の検索は既定では使わない。設定に検索サービス（Nominatim と同じ API）の URL を書いたときはその URL、書いて
//   いなくて「OSM の公開の地名の検索（Nominatim）を使う」を ON にしたときだけ OSM Foundation の公開の Nominatim に
//   送る（genzo-app の map.rs の geocoder_provider と同じ。公開の Nominatim のポリシーの上限はアプリの全利用者の
//   合計で、公開で配布するアプリでは数えられないので、既定は OFF。利用者（開発者本人）の決定。2026-10-20）。
// - 地名の検索は Enter か「検索」のボタンのときだけ（入力の途中で検索しない。オートコンプリートの禁止）。
// - 帰属の表示「© OpenStreetMap contributors」を地図の右下に常に出す（リンクは WebView ではなく既定のブラウザで開く）。
//   設定で変えたタイルの提供元の帰属の表示（tile_attribution）があれば並べて出す（文字だけ。リンクにしない。
//   providerAttribution）。
// - 先読み・まとめての取得をしない（Leaflet の既定の、表示している範囲のタイルだけを取る）。座標の欄に打つ途中の
//   値では地図を動かさない（確定したときだけ動かす。見るつもりのない場所のタイルを取らない）。

import type { AppSettings, GpsCoord, MapCacheInfo, MapLink, MapPreferences, Place } from "../api/types";
import { formatBytes } from "../format";
import { coordErrorMessage, parseCoordinates } from "./geotagLogic";

/** 地図の最大のズーム（genzo-geo の OSM_MAX_ZOOM と同じ。設定で変えた提供元も同じとみなす。仮置き）。 */
export const MAP_MAX_ZOOM = 19;

/**
 * 地図の帰属の表示（genzo-geo の OSM_ATTRIBUTION と同じ）。設定で変えた提供元も OSM のデータとみなして常に出し、
 * その提供元の帰属の表示（tile_attribution）は並べて出す（providerAttribution）。
 */
export const MAP_ATTRIBUTION = "© OpenStreetMap contributors";

/** 設定で変えたタイルの提供元の帰属の表示の文字数の上限（genzo-app の MAX_TILE_ATTRIBUTION_CHARS と同じ。仮置き）。 */
export const MAX_TILE_ATTRIBUTION_CHARS = 200;

/** 地名の検索の間隔（ms。前の要求の開始から。Nominatim のポリシーの「1 秒に 1 回まで」）。 */
export const SEARCH_INTERVAL_MS = 1000;

/** 検索語の長さの上限（文字数。genzo-geo の MAX_QUERY_CHARS と同じ。仮置き）。 */
export const MAX_QUERY_CHARS = 200;

/** タイルのキャッシュの容量の上限（genzo-geo の DEFAULT_CACHE_LIMIT_BYTES と同じ 200 MiB。仮置き）。 */
export const MAP_CACHE_LIMIT_BYTES = 200 * 1024 * 1024;

/** 既定のタイルの URL（genzo-geo の OSM_TILE_URL。設定の欄の例に出す）。 */
export const DEFAULT_TILE_URL = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";

/**
 * OSM Foundation の公開の Nominatim の検索の URL（genzo-geo の NOMINATIM_SEARCH_URL）。既定では使わない（設定の
 * public_nominatim を ON にし、検索の URL が空のときだけ。searchService）。
 */
export const PUBLIC_NOMINATIM_SEARCH_URL = "https://nominatim.openstreetmap.org/search";

/** 設定の「OSM の公開の地名の検索（Nominatim）を使う」の見出し。 */
export const PUBLIC_NOMINATIM_LABEL = "OSM の公開の地名の検索（Nominatim）を使う";

/**
 * 「OSM の公開の地名の検索（Nominatim）を使う」の説明（切り替えの近くに出す。利用ポリシーの制限を示す。AI の仮の案）。
 * オンラインの地図を有効にする同意とは別の切り替えで、ON にしたときだけ公開の Nominatim に検索語を送る。
 */
export const PUBLIC_NOMINATIM_NOTE =
  "ON にすると、地名の検索で OpenStreetMap Foundation の公開のサーバー（nominatim.openstreetmap.org）に検索語を" +
  "送ります。利用ポリシーにより、検索はこのアプリの利用者全員の合計で 1 秒に 1 回までで、入力の途中の自動の検索" +
  "（オートコンプリート）はしません（このアプリが数えられるのは、この PC で動いているアプリの中の間隔だけです）。" +
  "使えることの保証はなく、予告なく使えなくなることがあります。既定は OFF です。下の「詳細」で検索サービスの URL を" +
  "指定したときは、そちらを使います。";

/**
 * 地名の検索のサービスがない（既定）ときに、位置を付けるダイアログの検索の欄の代わりに出す案内（AI の仮の案）。
 */
export const SEARCH_UNAVAILABLE_NOTE =
  "地名の検索は、設定の「一般」のオンラインの地図で、OSM の公開の地名の検索（Nominatim）を使うか、検索サービス" +
  "（Nominatim と同じ API）の URL を指定すると使えます。座標や Google マップの場所の URL を貼り付けても位置を付け" +
  "られます。";

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

/** 地図のタイルを読めないときの文（genzo-geo は PNG でない応答を受け取らないので、提供元が配るタイルの種類も挙げる）。 */
export const MAP_TILE_ERROR =
  "地図のタイルを読み込めません。ネットワークの接続とプロキシの設定を確かめてください（続くときは、設定の地図の" +
  "提供元と、その提供元が PNG のタイルを配っているかを確かめてください）。";

// ---------------------------------------------------------------------------
// 設定
// ---------------------------------------------------------------------------

/** 設定の既定値（genzo-app の MapPreferences の Default。オンラインの地図は無効）。 */
export const DEFAULT_MAP_PREFERENCES: MapPreferences = {
  online: false,
  consented_at: null,
  tile_url: null,
  search_url: null,
  public_nominatim: false,
  tile_attribution: null,
};

/**
 * アプリの設定の地図の欄（古いアプリでは無いので既定値。欠けた欄（前のアプリの tile_attribution・public_nominatim
 * など）も既定値）。
 */
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

/** 欄の値（前後の空白を除き、空なら null）。 */
function trimmedOrNull(text: string): string | null {
  const t = text.trim();
  return t === "" ? null : t;
}

/** URL の欄の値（前後の空白を除き、空なら null（タイルは既定の提供元、地名の検索は searchService）。形はアプリが確かめる。 */
export function urlFieldValue(text: string): string | null {
  return trimmedOrNull(text);
}

/** 設定の画面の「詳細（地図の提供元・プロキシ）」の提供元の欄の文字。 */
export interface ProviderFields {
  tileUrl: string;
  searchUrl: string;
  /** 地図の帰属の表示（タイルの提供元）。 */
  tileAttribution: string;
}

/** 今の設定から、提供元の欄の初めの値を作る（null は空の欄）。 */
export function providerFieldsOf(p: MapPreferences): ProviderFields {
  return { tileUrl: p.tile_url ?? "", searchUrl: p.search_url ?? "", tileAttribution: p.tile_attribution ?? "" };
}

/**
 * 提供元の欄から保存する値（前後の空白を除き、空なら null）。帰属の表示は、タイルの URL が空（OSM の標準）なら null
 * （genzo-app の MapPreferences::normalized も同じに整える）。URL の形、帰属の表示の文字数・制御文字はアプリが
 * 確かめる（使えなければ何も保存しない）。
 */
export function providerPatch(f: ProviderFields): Pick<MapPreferences, "tile_url" | "search_url" | "tile_attribution"> {
  const tile_url = urlFieldValue(f.tileUrl);
  return {
    tile_url,
    search_url: urlFieldValue(f.searchUrl),
    tile_attribution: tile_url === null ? null : trimmedOrNull(f.tileAttribution),
  };
}

/** 提供元の欄で保存する値が、今の設定と違うか（違うときだけ「元に戻す」・「提供元を保存」を押せる）。 */
export function providersDirty(p: MapPreferences, f: ProviderFields): boolean {
  const next = providerPatch(f);
  return (
    next.tile_url !== p.tile_url ||
    next.search_url !== p.search_url ||
    next.tile_attribution !== (p.tile_attribution ?? null)
  );
}

/**
 * タイルの URL を変えたのに、帰属の表示が空か（設定の画面で案内する。多くの提供元は自分の帰属の表示を利用の条件に
 * している。空でも保存はできる）。
 */
export function providerAttributionMissing(f: ProviderFields): boolean {
  const next = providerPatch(f);
  return next.tile_url !== null && next.tile_attribution === null;
}

/**
 * 地図の右下に、OSM の帰属の表示と並べて出す、設定で変えたタイルの提供元の帰属の表示（なければ null）。タイルの URL を
 * 変えていない（OSM の標準）ときは使わない。文字だけで出す（HTML として読まない。任意の URL を開かないよう、
 * リンクにもしない）。
 */
export function providerAttribution(p: MapPreferences): string | null {
  if (p.tile_url === null) return null;
  return trimmedOrNull(p.tile_attribution ?? "");
}

/** URL のホスト名（読めなければ URL のまま）。 */
function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

/**
 * 地名の検索の送り先（genzo-app の map.rs の geocoder_provider と同じ）: 検索の URL があればそれ（custom）、なければ
 * public_nominatim が ON のときだけ OSM Foundation の公開の Nominatim（public）、どちらもなければなし（none。既定。
 * 位置を付けるダイアログは検索の欄を出さない）。
 */
export type SearchService =
  | { kind: "custom"; url: string; host: string }
  | { kind: "public"; url: string; host: string }
  | { kind: "none" };

export function searchService(p: MapPreferences): SearchService {
  if (p.search_url !== null) return { kind: "custom", url: p.search_url, host: hostOf(p.search_url) };
  if (p.public_nominatim) {
    return { kind: "public", url: PUBLIC_NOMINATIM_SEARCH_URL, host: hostOf(PUBLIC_NOMINATIM_SEARCH_URL) };
  }
  return { kind: "none" };
}

/** 送り先（ホスト名）。`search` は地名の検索のサービスがなければ null。 */
export interface MapDestinations {
  tiles: string;
  search: string | null;
}

export function mapDestinations(p: MapPreferences): MapDestinations {
  const search = searchService(p);
  return {
    tiles: hostOf(p.tile_url ?? DEFAULT_TILE_URL),
    search: search.kind === "none" ? null : search.host,
  };
}

/** 送り先の文。 */
export function describeDestinations(p: MapPreferences): string {
  const d = mapDestinations(p);
  const tiles = p.tile_url === null ? `${d.tiles}（OpenStreetMap Foundation）` : `${d.tiles}（設定で変えた提供元）`;
  const service = searchService(p);
  const search =
    service.kind === "custom"
      ? `${service.host}（設定した検索サービス）`
      : service.kind === "public"
        ? `${service.host}（OpenStreetMap Foundation の公開の Nominatim。設定で使うと選んだもの）`
        : "なし（検索サービスを設定していないので、検索語は送りません）";
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
      "地図に表示している範囲の地図のタイルの要求（ズームと位置から、見ている場所が分かります）。位置を付ける" +
        "ダイアログでは、表示中の写真の位置（カメラの位置か付けた位置）や、座標の欄で確定した位置を中心に地図を" +
        `開くので、その付近のタイルを要求します（ズーム ${PIN_ZOOM} で開くので、タイル 1 枚の大きさ（日本の付近では` +
        "およそ 1 km 四方）ほどの精度で場所が分かります）",
      searchService(p).kind === "none"
        ? "地名の検索に入れた語（検索サービスを設定した場合だけ。今は設定していないので送りません）"
        : "地名の検索に入れた語（設定した検索サービスに送ります）",
      "通信のときに、この PC の IP アドレスと、アプリの名前と版（User-Agent）が送り先に伝わります。写真のファイルや" +
        "カタログそのものは送りません（座標の読み取りは、このパソコンの中で行います。地図に出した位置は、上のとおり" +
        "タイルの要求から分かります）",
    ],
    destination: describeDestinations(p),
    rules: [
      "地名の検索は、Enter か「検索」のボタンを押したときだけ送ります（入力の途中では送りません）。続けて検索するときは 1 秒以上あけ、同じ語の結果は覚えておいて問い合わせ直しません",
      "地図のタイルは、表示している範囲の分だけを取り、このパソコンにキャッシュします（提供元のキャッシュの指示に従います）。先読みや、まとめての取得はしません（座標の欄に打つ途中の値では地図を動かしません）",
      attributionRule(p),
    ],
    cautions: [
      "自宅の住所など、個人の情報を検索語にしないでください（OSM の利用ポリシーは、個人の情報を送らないよう求めています）",
      "OSM のサーバーは寄付と協賛で運営されていて容量に限りがあり、使えることの保証はありません（予告なく使えなくなることがあります）。使えないときは、設定の「一般」で地図の提供元を変えられます",
      "無効のままでも、座標の貼り付けで位置を付けられます。有効にした後も、設定の「一般」でいつでも無効にできます",
    ],
  };
}

/** 有効にする前の説明の、帰属の表示の守ること（設定したタイルの提供元の帰属の表示があれば、それも書く）。 */
function attributionRule(p: MapPreferences): string {
  const credit = providerAttribution(p);
  return credit === null
    ? `地図の右下に「${MAP_ATTRIBUTION}」を表示します`
    : `地図の右下に「${MAP_ATTRIBUTION}」と、設定したタイルの提供元の帰属の表示（${credit}）を表示します`;
}

/**
 * 位置を付けるダイアログの座標の欄の下の注記。座標を読むのはこのパソコンの中だが、オンラインの地図が有効なら、
 * 確定した座標へ地図を動かし、その付近のタイルを提供元に要求する（タイルの位置から場所が分かる）ので、
 * 「外部には送りません」と言い切らない。
 */
export function coordinateNote(p: MapPreferences): string {
  if (!p.online) return "座標はこのパソコンの中で読み、外部には送りません。";
  return (
    "座標はこのパソコンの中で読みます。オンラインの地図が有効なので、座標を確定すると（貼り付けたとき・欄を離れた" +
    `ときなど）地図をその位置へ動かし、付近の地図のタイルを提供元（${mapDestinations(p).tiles}）に要求します` +
    "（タイルの位置から、おおよその場所が分かります）。"
  );
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

/** 地名の検索の欄の語の扱い（`searchInput`）。 */
export type SearchInput =
  /** 送らない（空・長すぎる・座標の形だが読めない）。`message` を欄の下に出す。 */
  | { kind: "problem"; message: string }
  /** 座標として読めた（外部に送らず、座標の欄に入れる）。 */
  | { kind: "coordinate"; coord: GpsCoord }
  /** 地名の検索に送る（整えた語）。 */
  | { kind: "search"; query: string };

/**
 * 地名の検索の欄の語の扱い。座標の形の語（座標の欄と同じ parseCoordinates で読めるもの・範囲外や座標のない URL
 * など、座標の形だが読めないもの）は地名の検索（外部）に送らない。座標の欄に貼るつもりの座標を、検索の欄に貼って
 * Enter を押した場合に送らないため（座標の読み取りはこのパソコンの中で行うと画面に書いている。Nominatim の
 * ポリシーも個人の情報を送らないよう求めている）。座標の判定は長さの検査より先（長い Google マップの URL も読む）。
 */
export function searchInput(q: string): SearchInput {
  const query = normalizeSearchQuery(q);
  if (query === "") return { kind: "problem", message: "検索語が空です" };
  const parsed = parseCoordinates(q);
  if (parsed.ok) return { kind: "coordinate", coord: parsed.coord };
  if (parsed.error.kind !== "unreadable" && parsed.error.kind !== "empty") {
    return { kind: "problem", message: `座標の形の語は地名の検索に送りません（${coordErrorMessage(parsed.error)}）` };
  }
  const problem = searchQueryProblem(query);
  return problem ? { kind: "problem", message: problem } : { kind: "search", query };
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

/** 地図の初めの表示（確定した付ける位置、なければ表示中の写真の位置を中心に。どちらもなければ既定）。 */
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
 * 位置を付けるダイアログの座標の欄の文字と、地図を動かす先（確定した付ける位置）。
 *
 * 打つ途中で読める値になるたびに地図を動かすと（"35.681236, 1" は経度 1 として読める）、見るつもりのない場所の
 * タイルを提供元に要求するので、地図を動かす先は確定したとき（Enter・欄を離れる・貼り付けた後）と、地図・検索の
 * 結果・写真の位置から入れたときだけ変える。ピン（付ける位置の印）は欄の値に合わせてすぐ動かす（印を動かすだけ
 * では、タイルを要求しない）。見込み・付ける操作は、今までどおり欄の値をそのまま読む。
 */
export interface CoordField {
  text: string;
  /** 地図を動かす先（確定したときに読めた座標。読めなければ null）。 */
  target: GpsCoord | null;
}

export type CoordFieldAction =
  /** 打つ・消す（地図を動かす先は変えない）。 */
  | { kind: "input"; text: string }
  /** 確定する（Enter・欄を離れる・貼り付けた後）。 */
  | { kind: "commit" }
  /** 地図・検索の結果・写真の位置から入れる（すぐ確定する）。 */
  | { kind: "set"; text: string };

export const EMPTY_COORD_FIELD: CoordField = { text: "", target: null };

export function coordFieldReducer(state: CoordField, action: CoordFieldAction): CoordField {
  switch (action.kind) {
    case "input":
      return action.text === state.text ? state : { ...state, text: action.text };
    case "commit":
      return committedField(state, state.text);
    case "set":
      return committedField(state, action.text);
  }
}

/** `text` で確定した欄（動かす先が同じ座標なら前のものを使い、何も変わらなければ同じ state を返す）。 */
function committedField(state: CoordField, text: string): CoordField {
  const parsed = parseCoordinates(text);
  const next = parsed.ok ? parsed.coord : null;
  const prev = state.target;
  const same = next === null || prev === null ? next === prev : next.lat === prev.lat && next.lon === prev.lon;
  if (same && text === state.text) return state;
  return { text, target: same ? prev : next };
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
