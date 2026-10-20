// 設定の「一般」の「オンラインの地図（OpenStreetMap）」（位置情報の後付け LIB-21 の第 2 段階。docs/04 の 3.16 節）:
// 有効・無効、タイルのキャッシュの大きさと消去、詳細（地図の提供元の URL・帰属の表示と、プロキシの説明）、利用ポリシーの
// ページ。
//
// - 既定で無効（docs/02 の SEC-02）。初めて有効にするとき（consented_at がまだない）は、位置を付けるダイアログと
//   同じ説明と確認（MapConsent.tsx）を出す。
// - 保存は専用のコマンド（state/map.ts の saveMapPreferences）で、すぐ効く（この画面の「保存」のボタン・
//   「再起動すると有効」とは別）。
// - 地名の検索は既定では使わない（geoMapLogic.ts の searchService）。「OSM の公開の地名の検索（Nominatim）を使う」
//   （public_nominatim。既定は OFF）は、オンラインの地図を有効にする同意とは別の切り替えで、近くに利用ポリシーの制限の
//   説明（PUBLIC_NOMINATIM_NOTE）と Nominatim の利用ポリシーのボタンを置く。検索サービスの URL を指定したときは、
//   切り替えによらずその URL を使う（その間は切り替えを押せるが、使わないと書く）。保存はすぐ効く。
// - 提供元の URL（OSM のポリシーの「ソフトウェアの更新なしに切り替えられること」）は、タイルは空なら OpenStreetMap の
//   標準、地名の検索は空なら上の切り替えに従う。https の URL だけ（形はアプリが確かめ、使えなければ何も保存しない）。PNG のタイルを配る提供元だけ（genzo-geo は
//   Content-Type が image/png で PNG の署名で始まる応答だけを受け取る。保存のときにアプリが断るのは、URL の拡張子で
//   わかる PNG でないもの（.jpg・.webp・.pbf など。genzo-geo の validate_tile_url_template。目安）だけで、拡張子のない
//   URL は応答で確かめるので、説明に書く）。
// - プロキシの説明: genzo-geo（proxy.rs）が読むのは、環境変数と Windows の手動のプロキシの設定だけ。PAC・設定の
//   自動検出・Windows 以外のシステムの設定は読まずに直接つなぎ、SOCKS は誤りにする。読まない設定は誤りにならない
//   （つながらないときに、通信の失敗としか分からない）ので、説明に書く。
// - 地図の帰属の表示（tile_attribution）: タイルの URL を変えたときだけ使い、地図の右下に「© OpenStreetMap
//   contributors」と並べて文字だけで出す（GeoMap.tsx。多くの提供元は自分の帰属の表示を利用の条件にしている）。
//   タイルの URL が空の間は入力できない（アプリも保存のときに null にする）。タイルの URL があるのに空なら案内する。
//   文言は提供元の利用条件で決まるので、利用者が入れる（例は出すが、決めた文は入れない）。
// - キャッシュの大きさ・消去・ページを開くことは、無効でも使える（外部と通信しない）。

import { useEffect, useState } from "react";

import * as app from "../api/app";
import { toApiError } from "../api/errors";
import type { ErrorInfo, MapCacheInfo, MapPreferences } from "../api/types";
import {
  DEFAULT_TILE_URL,
  describeDestinations,
  describeMapCache,
  MAP_ATTRIBUTION,
  MAP_CACHE_LIMIT_BYTES,
  MAP_LINK_LABELS,
  MAP_MAX_ZOOM,
  MAX_TILE_ATTRIBUTION_CHARS,
  mapPreferencesOf,
  type ProviderFields,
  providerAttributionMissing,
  providerFieldsOf,
  providerPatch,
  providersDirty,
  PUBLIC_NOMINATIM_LABEL,
  PUBLIC_NOMINATIM_NOTE,
  searchService,
} from "../library/geoMapLogic";
import { actions, useApp } from "../state/app";
import { openMapPage, saveMapPreferences } from "../state/map";
import { formatLocalDateTime } from "./dialogFormat";
import { MapConsentPanel } from "./MapConsent";
import { Busy, ErrorBox } from "./parts";

/** 節の説明（文の途中で JSX の行を分けると空白が入るので、文字列で組む）。 */
function sectionNote(prefs: MapPreferences): string {
  return (
    "写真に位置を付けるダイアログで、地図で場所を選べます。地名の検索は、検索サービスを設定した場合（下の切り替えか、" +
    "詳細の検索サービスの URL）だけ使えます。有効にしたときだけ、地図のタイルの要求（地図に出した場所が分かります）と、" +
    "検索サービスを設定した場合は検索した語を外部のサーバー" +
    `（${describeDestinations(prefs)}）に送ります。無効のときは通信しません` +
    "（座標の貼り付けで位置を付けることはできます）。この節の設定はすぐに効きます。"
  );
}

/** タイルのキャッシュの説明。 */
const CACHE_NOTE =
  "見た地図のタイルを、このパソコンのキャッシュのフォルダに保存します（提供元のキャッシュの指示に従います。" +
  `${MAP_CACHE_LIMIT_BYTES / 1024 / 1024} MB を超えたら古いものから消します）。`;

/** 提供元の URL の説明。 */
const PROVIDER_NOTE =
  "タイルの URL が空なら OpenStreetMap の標準のタイルを使います。OpenStreetMap のサーバーが使えないときや、利用の" +
  "量が多いときは、OSM のデータを使う別の提供元に切り替えられます（https の URL で、PNG のタイルを配る提供元だけ。JPEG・WebP・ベクトルの" +
  "タイルなどは読み込めません（URL の拡張子が .jpg・.webp などなら、保存のときに断ります）。その提供元の利用条件を" +
  "確かめてください。最大の" +
  `ズームは ${MAP_MAX_ZOOM} とみなします）。地図の右下には「${MAP_ATTRIBUTION}」を常に出し、その提供元の帰属の` +
  "表示は、下の「地図の帰属の表示」に入れると並べて出します。";

/** 提供元の欄の下の説明。 */
const PROVIDER_FIELDS_NOTE =
  "タイルの URL は {z}・{x}・{y} を 1 つずつ含めてください。検索サービスの URL は Nominatim と同じ形の /search の API" +
  "で、その提供元の利用条件を確かめてください（1 秒に 1 回まで・入力の途中で検索しない・結果を覚えておく、の規則は" +
  "どの提供元にも当てます）。空なら、上の「" +
  `${PUBLIC_NOMINATIM_LABEL}」が ON のときだけ公開のサーバーを使い、OFF なら地名の検索は使えません。` +
  "地図の帰属の表示は、タイルの URL を変えたときだけ使い、地図の右下に文字だけで出します（リンクにはしません。" +
  `${MAX_TILE_ATTRIBUTION_CHARS} 文字まで）。`;

/** 通信のプロキシの説明（genzo-geo の proxy.rs が読む設定）。 */
const PROXY_NOTE =
  "通信のプロキシは、環境変数（HTTPS_PROXY・ALL_PROXY・HTTP_PROXY と NO_PROXY）か、Windows の手動のプロキシの設定" +
  "（例外のアドレスを含む）を使います。自動構成スクリプト（PAC）・設定の自動検出と、Windows 以外のシステムの設定の" +
  "プロキシは読みません（それらだけが設定されているときは、直接つなぎます。プロキシが要るときは、環境変数" +
  " HTTPS_PROXY を設定してください）。" +
  "SOCKS のプロキシには対応していません。";

/** タイルの URL を変えたのに、地図の帰属の表示が空のときの案内。 */
const ATTRIBUTION_MISSING_NOTE =
  "タイルの提供元の帰属の表示が空です。多くの提供元は、自分の帰属の表示を利用の条件にしています。その提供元の" +
  "利用条件を確かめて、「地図の帰属の表示」に入れてください（空のままなら「" +
  `${MAP_ATTRIBUTION}」だけを出します）。`;

export function SettingsMap() {
  const saved = useApp((s) => s.status?.settings.map);
  const prefs = mapPreferencesOf({ map: saved });
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [cache, setCache] = useState<MapCacheInfo | null>(null);
  const [clearing, setClearing] = useState(false);
  const [cacheError, setCacheError] = useState<ErrorInfo | null>(null);
  const [fields, setFields] = useState<ProviderFields>(() => providerFieldsOf(prefs));
  const setField = (patch: Partial<ProviderFields>) => setFields((f) => ({ ...f, ...patch }));

  useEffect(() => {
    let alive = true;
    app
      .mapCacheInfo()
      .then((info) => {
        if (alive) setCache(info);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  /** 保存する（すぐ効く）。成功したら `done` の文を知らせる。 */
  const save = async (patch: Partial<MapPreferences>, done: string): Promise<boolean> => {
    setBusy(true);
    setError(null);
    try {
      const settings = await saveMapPreferences(patch);
      if ("tile_url" in patch || "search_url" in patch || "tile_attribution" in patch) {
        // 提供元を保存したら、アプリが整えた値（前後の空白を除いた URL・帰属の表示）を欄に出す。
        setFields(providerFieldsOf(mapPreferencesOf(settings)));
      }
      actions.notify("success", done);
      return true;
    } catch (e) {
      setError(toApiError(e).info);
      return false;
    } finally {
      setBusy(false);
    }
  };

  const togglePublicSearch = (on: boolean) => {
    void save(
      { public_nominatim: on },
      on ? "OSM の公開の地名の検索（Nominatim）を使うようにしました" : "OSM の公開の地名の検索（Nominatim）を使わないようにしました",
    );
  };

  const toggle = (on: boolean) => {
    if (on && !prefs.consented_at) {
      setConfirming(true);
      return;
    }
    void save({ online: on }, on ? "オンラインの地図を有効にしました" : "オンラインの地図を無効にしました");
  };

  const enable = async () => {
    if (await save({ online: true }, "オンラインの地図を有効にしました")) setConfirming(false);
  };

  const clearCache = async () => {
    setClearing(true);
    setCacheError(null);
    try {
      setCache(await app.clearMapCache());
      actions.notify("success", "地図のキャッシュを消しました");
    } catch (e) {
      setCacheError(toApiError(e).info);
    } finally {
      setClearing(false);
    }
  };

  // 提供元の欄から保存する値と、今の設定と違うか。
  const next = providerPatch(fields);
  const dirty = providersDirty(prefs, fields);
  // 地名の検索の送り先（検索の URL を指定しているときは、公開の Nominatim の切り替えを使わない）。
  const search = searchService(prefs);

  return (
    <div className="dlg-section">
      <h3>オンラインの地図（OpenStreetMap）</h3>
      <p className="hint" style={{ marginTop: 0 }}>
        {sectionNote(prefs)}
      </p>
      <label className="dlg-check">
        <input
          type="checkbox"
          checked={prefs.online}
          onChange={(e) => toggle(e.target.checked)}
          disabled={busy || confirming}
        />
        <span>
          オンラインの地図を使う
          <span className="desc">
            {prefs.consented_at
              ? `初めて有効にした日時: ${formatLocalDateTime(prefs.consented_at)}`
              : "初めて有効にするときに、送るものと送り先を確かめてもらいます"}
          </span>
        </span>
      </label>
      {confirming && (
        <MapConsentPanel
          preferences={prefs}
          busy={busy}
          error={error}
          onEnable={() => void enable()}
          onCancel={() => setConfirming(false)}
        />
      )}

      <label className="dlg-check">
        <input
          type="checkbox"
          checked={prefs.public_nominatim}
          onChange={(e) => togglePublicSearch(e.target.checked)}
          disabled={busy || confirming}
        />
        <span>
          {PUBLIC_NOMINATIM_LABEL}
          <span className="desc">
            {search.kind === "custom"
              ? `検索サービスの URL（${search.host}）を指定しているので、今はそちらを使います（この切り替えは使いません）`
              : "既定は OFF です。OFF で検索サービスの URL もないときは、地名の検索は使えません"}
          </span>
        </span>
      </label>
      <p className="hint">
        {PUBLIC_NOMINATIM_NOTE}
      </p>
      <div className="dlg-row">
        <button type="button" className="btn small" onClick={() => openMapPage("nominatim_policy")}>
          {MAP_LINK_LABELS.nominatim_policy}
        </button>
        <span className="hint">（英語のページを既定のブラウザで開きます）</span>
      </div>

      <div className="dlg-row" style={{ marginTop: 6 }}>
        <span>地図のキャッシュ: {cache ? describeMapCache(cache) : "—"}</span>
        <button
          type="button"
          className="btn small"
          onClick={() => void clearCache()}
          disabled={clearing || !cache || cache.files === 0}
        >
          {clearing ? "消しています…" : "キャッシュを消す"}
        </button>
      </div>
      <p className="hint">
        {CACHE_NOTE}
      </p>
      <ErrorBox error={cacheError} title="キャッシュを消せません" />

      <details>
        <summary className="hint">詳細（地図の提供元・プロキシ）</summary>
        <p className="hint">
          {PROVIDER_NOTE}
        </p>
        <div className="field">
          <label htmlFor="map-tile-url">タイルの URL</label>
          <input
            id="map-tile-url"
            type="text"
            value={fields.tileUrl}
            placeholder={DEFAULT_TILE_URL}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => setField({ tileUrl: e.target.value })}
          />
        </div>
        <div className="field">
          <label htmlFor="map-search-url">検索サービスの URL</label>
          <input
            id="map-search-url"
            type="text"
            value={fields.searchUrl}
            placeholder="例: https://geocoder.example.org/search（空なら、上の切り替えに従います）"
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => setField({ searchUrl: e.target.value })}
          />
        </div>
        <div className="field">
          <label htmlFor="map-tile-attribution">地図の帰属の表示</label>
          {/* タイルの URL が空（OSM の標準）の間は使わないので入力できない（アプリも保存のときに null にする）。 */}
          <input
            id="map-tile-attribution"
            type="text"
            value={fields.tileAttribution}
            placeholder={next.tile_url === null ? "タイルの URL を変えたときに入れます" : "例: Maps © 提供元の名前"}
            spellCheck={false}
            autoComplete="off"
            disabled={next.tile_url === null}
            onChange={(e) => setField({ tileAttribution: e.target.value })}
          />
        </div>
        <p className="hint">
          {PROVIDER_FIELDS_NOTE}
        </p>
        {providerAttributionMissing(fields) && <p className="warn-text hint">{ATTRIBUTION_MISSING_NOTE}</p>}
        <div className="dlg-row end">
          <button
            type="button"
            className="btn"
            onClick={() => setFields(providerFieldsOf(prefs))}
            disabled={busy || !dirty}
          >
            元に戻す
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => void save(next, "地図の提供元を保存しました")}
            disabled={busy || !dirty}
          >
            提供元を保存
          </button>
        </div>
        <p className="hint">
          {PROXY_NOTE}
        </p>
      </details>

      <div className="dlg-row" style={{ marginTop: 6 }}>
        <span className="hint">利用ポリシーなど（既定のブラウザで開きます）:</span>
        <button type="button" className="btn small" onClick={() => openMapPage("tile_policy")}>
          {MAP_LINK_LABELS.tile_policy}
        </button>
        <button type="button" className="btn small" onClick={() => openMapPage("copyright")}>
          {MAP_LINK_LABELS.copyright}
        </button>
      </div>
      {busy && !confirming && <Busy>保存しています…</Busy>}
      {!confirming && <ErrorBox error={error} title="保存できません" />}
    </div>
  );
}
