// 設定の「一般」の「オンラインの地図（OpenStreetMap）」（位置情報の後付け LIB-21 の第 2 段階。docs/04 の 3.16 節）:
// 有効・無効、タイルのキャッシュの大きさと消去、詳細（地図の提供元の URL）、利用ポリシーのページ。
//
// - 既定で無効（docs/02 の SEC-02）。初めて有効にするとき（consented_at がまだない）は、位置を付けるダイアログと
//   同じ説明と確認（MapConsent.tsx）を出す。
// - 保存は専用のコマンド（state/map.ts の saveMapPreferences）で、すぐ効く（この画面の「保存」のボタン・
//   「再起動すると有効」とは別）。
// - 提供元の URL（OSM のポリシーの「ソフトウェアの更新なしに切り替えられること」）は空なら OpenStreetMap の標準。
//   https の URL だけ（形はアプリが確かめ、使えなければ何も保存しない）。PNG のタイルを配る提供元だけ（genzo-geo は
//   Content-Type が image/png で PNG の署名で始まる応答だけを受け取る。保存のときには種類を確かめられないので、
//   説明に書く）。
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
  DEFAULT_SEARCH_URL,
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
} from "../library/geoMapLogic";
import { actions, useApp } from "../state/app";
import { openMapPage, saveMapPreferences } from "../state/map";
import { formatLocalDateTime } from "./dialogFormat";
import { MapConsentPanel } from "./MapConsent";
import { Busy, ErrorBox } from "./parts";

/** 節の説明（文の途中で JSX の行を分けると空白が入るので、文字列で組む）。 */
function sectionNote(prefs: MapPreferences): string {
  return (
    "写真に位置を付けるダイアログで、地図で場所を選んだり、地名で検索したりできます。有効にしたときだけ、地図の" +
    "タイルの要求（地図に出した場所が分かります）と検索した語を外部のサーバー" +
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
  "空なら OpenStreetMap の標準を使います。OpenStreetMap のサーバーが使えないときや、利用の量が多いときは、OSM の" +
  "データを使う別の提供元に切り替えられます（https の URL で、PNG のタイルを配る提供元だけ。JPEG・WebP などの" +
  "タイルは読み込めません。その提供元の利用条件を確かめてください。最大の" +
  `ズームは ${MAP_MAX_ZOOM} とみなします）。地図の右下には「${MAP_ATTRIBUTION}」を常に出し、その提供元の帰属の` +
  "表示は、下の「地図の帰属の表示」に入れると並べて出します。";

/** 提供元の欄の下の説明。 */
const PROVIDER_FIELDS_NOTE =
  "タイルの URL は {z}・{x}・{y} を 1 つずつ含めてください。地名の検索は Nominatim と同じ形の /search の API です。" +
  "地図の帰属の表示は、タイルの URL を変えたときだけ使い、地図の右下に文字だけで出します（リンクにはしません。" +
  `${MAX_TILE_ATTRIBUTION_CHARS} 文字まで）。`;

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
        <summary className="hint">詳細（地図の提供元）</summary>
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
          <label htmlFor="map-search-url">地名の検索の URL</label>
          <input
            id="map-search-url"
            type="text"
            value={fields.searchUrl}
            placeholder={DEFAULT_SEARCH_URL}
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
      </details>

      <div className="dlg-row" style={{ marginTop: 6 }}>
        <span className="hint">利用ポリシーなど（既定のブラウザで開きます）:</span>
        <button type="button" className="btn small" onClick={() => openMapPage("tile_policy")}>
          {MAP_LINK_LABELS.tile_policy}
        </button>
        <button type="button" className="btn small" onClick={() => openMapPage("nominatim_policy")}>
          {MAP_LINK_LABELS.nominatim_policy}
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
