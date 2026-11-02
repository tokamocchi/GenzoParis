// 地図の表示（LIB-18。表示モード「地図」。Shift + M。docs/04 の 3.17 節）: 表示中の検索結果の写真を、使う位置
// （付けた位置があればそれ、なければカメラの位置。LIB-21）で地図に並べ、地図から探す。
//
// - オンラインの地図（LIB-21。既定で無効）が無効なら、地図を作らず（通信しない）、有効にする前の説明と確認
//   （dialogs/MapConsent.tsx）を出す。
// - Leaflet は useEffect の中で動的に読む（GeoMap.tsx と同じ。leafletLoader.ts）。タイルは genzo:// の /tile。
//   先読みはしない（表示している範囲のタイルだけ。GeoMap.tsx の doc）。帰属の表示は MapAttribution（右下に常に）。
// - 印は、コアが地図のズームに合わせてまとめたもの（core.mapClusters。写真ごとの位置は受け取らない）。地図を
//   動かしたら（moveend）、表示範囲を上下左右に広げた範囲で問い合わせ直す（MAP_QUERY_DELAY_MS 待ち、古い応答は
//   捨てる。コアへの問い合わせで、外部への通信ではない）。検索結果の世代・写真の位置（locationVersion）が変わった
//   ときも問い合わせ直す。印は地図の中心に近い世界の写しに置く（mapViewLogic.ts の markerLon）。
// - 印を押す: 2 枚以上のまとまりで、まだ寄れるなら、まとまりの範囲へ寄せる。1 枚（か、いちばん寄った後）なら、代表の
//   写真を選ぶ（フィルムストリップに出る）。1 枚の印のダブルクリックで、その写真をルーペで開く。印に重ねると
//   代表の写真のサムネイルと数を出す。
// - 表示中の写真の位置に印（.geo-photo-mark）を出す（地図は動かさない。「表示中の写真へ」で寄せる）。
// - 地図の表示は、このセッションの中だけ覚える（ブラウザに保存しない。見ていた場所を残さない）。初めて開いたとき
//   （このセッションでまだ見ていなければ）は、写真の全体（extent）が見えるように寄せてからタイルの層を足す（寄せる
//   前の既定の範囲のタイルを要求しない。showTiles）。
// - 「地図の範囲で絞り込む」: 入れると、表示している範囲を絞り込みの条件（location_bounds）にし、地図を動かすたびに
//   （MAP_FILTER_DELAY_MS 待って）変える。外すと条件を消す（絞り込みの帯の札でも外せる）。
// - Leaflet のキーボードの操作は使わない（全体のショートカット（←・→ など）と重ならないように）。

import type * as Leaflet from "leaflet";
import { useEffect, useRef, useState } from "react";

import { reportUiError } from "../api/app";
import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { ErrorInfo, GpsCoord, MapClusters, MapPreferences, VariantId } from "../api/types";
import { thumbUrl, tileUrlTemplate } from "../api/uri";
import { MapConsentPanel } from "../dialogs/MapConsent";
import { actions, appStore, useApp } from "../state/app";
import { saveMapPreferences } from "../state/map";
import { MapAttribution } from "./GeoMap";
import {
  DEFAULT_MAP_CENTER,
  DEFAULT_MAP_ZOOM,
  MAP_MAX_ZOOM,
  MAP_TILE_ERROR,
  type MapView as MapViewTarget,
  mapPreferencesOf,
  PIN_ZOOM,
  providerAttribution,
} from "./geoMapLogic";
import {
  clusterSizeClass,
  clusterView,
  describeMapCounts,
  extentView,
  formatClusterCount,
  MAP_FILTER_DELAY_MS,
  MAP_QUERY_DELAY_MS,
  markerLon,
  padView,
  sameBounds,
  viewBounds,
} from "./mapViewLogic";
import "./geo.css";

/** 印の大きさ（画素。geo.css の .map-cluster.s1〜s4 と同じ）。 */
const CLUSTER_PX = { s1: 28, s2: 34, s3: 40, s4: 46 } as const;

/** このセッションで最後に見ていた地図の表示（ブラウザに保存しない）。 */
let lastView: { lat: number; lng: number; zoom: number } | null = null;

/** テスト用: 覚えた地図の表示を消す。 */
export function resetMapViewForTest(): void {
  lastView = null;
}

export function MapView() {
  const settings = useApp((s) => s.status?.settings);
  const prefs = mapPreferencesOf(settings);
  return (
    <div className="map-view">
      {prefs.online ? <PhotoMap prefs={prefs} /> : <MapViewOffline prefs={prefs} />}
    </div>
  );
}

/** オンラインの地図が無効なとき: 説明と、有効にする前の確認。 */
function MapViewOffline(props: { prefs: MapPreferences }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const enable = async () => {
    setBusy(true);
    setError(null);
    try {
      await saveMapPreferences({ online: true });
      actions.notify("success", "オンラインの地図を有効にしました", "設定の「一般」でいつでも無効にできます。");
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="map-view-offline">
      <p className="hint">
        地図に写真を並べるには、オンラインの地図（OpenStreetMap）を有効にします（今は無効で、外部と通信しません）。
        地図を使わなくても、絞り込みの「詳しい条件」の「位置情報」で、位置のある写真・ない写真を探せます。
      </p>
      <MapConsentPanel preferences={props.prefs} busy={busy} error={error} onEnable={() => void enable()} />
    </div>
  );
}

/** 作った地図と印の層。 */
interface MapParts {
  L: typeof Leaflet;
  map: Leaflet.Map;
  clusters: Leaflet.LayerGroup;
  photo: Leaflet.Marker | null;
  /** 地図を、写真の全体か覚えた表示へ動かしたか・利用者が動かしたか（後から届いた全体へ勝手に動かさない）。 */
  located: boolean;
  /** まとまりの印（代表の写真ごと。表示中の写真の強調を付け替える）。 */
  markers: Map<VariantId, Leaflet.Marker>;
  /** タイルの層（表示する範囲が決まってから地図に足す。showTiles）。 */
  tiles: Leaflet.TileLayer;
}

/**
 * タイルの層を地図に足す（まだなら）。初めて開いたときは、写真の全体へ寄せてから足す（寄せる前の既定の範囲の
 * タイルを要求しない）。
 */
function showTiles(p: MapParts): void {
  if (!p.map.hasLayer(p.tiles)) p.tiles.addTo(p.map);
}

function PhotoMap(props: { prefs: MapPreferences }) {
  const container = useRef<HTMLDivElement>(null);
  const parts = useRef<MapParts | null>(null);
  // 地図を作るたびに増やす（印の effect を作り直した地図に当て直す）。0 はまだない。
  const [made, setMade] = useState(0);
  const [failed, setFailed] = useState(false);
  const [tileError, setTileError] = useState(false);
  // 地図を動かした回数（動かし終えるたびに増やす。まとまりの問い合わせ・絞り込みの範囲の更新に使う）。
  const [moved, setMoved] = useState(0);
  const [data, setData] = useState<MapClusters | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [photoGps, setPhotoGps] = useState<GpsCoord | null>(null);

  const generation = useApp((s) => s.search?.generation ?? null);
  const locationVersion = useApp((s) => s.locationVersion);
  const detailsVersion = useApp((s) => s.detailsVersion);
  const current = useApp((s) => s.selection.current);
  const filterBounds = useApp((s) => s.filter.location_bounds ?? null);
  const tracking = filterBounds !== null;

  // 地図を作る（1 回。Leaflet を読んでから）。
  useEffect(() => {
    let cancelled = false;
    let made: MapParts | null = null;
    let observer: ResizeObserver | null = null;
    setFailed(false);
    import("./leafletLoader")
      .then(({ default: L }) => {
        const el = container.current;
        if (cancelled || !el) return;
        const map = L.map(el, {
          attributionControl: false,
          keyboard: false,
          maxZoom: MAP_MAX_ZOOM,
          worldCopyJump: true,
        });
        const first = lastView;
        if (first) map.setView([first.lat, first.lng], first.zoom);
        else map.setView([DEFAULT_MAP_CENTER.lat, DEFAULT_MAP_CENTER.lon], DEFAULT_MAP_ZOOM);
        const tiles = L.tileLayer(tileUrlTemplate(), { maxZoom: MAP_MAX_ZOOM })
          .on("tileerror", () => setTileError(true))
          .on("tileload", () => setTileError(false));
        made = {
          L,
          map,
          clusters: L.layerGroup().addTo(map),
          photo: null,
          located: first !== null,
          tiles,
          markers: new Map(),
        };
        const m = made;
        if (first) showTiles(m);
        map.on("dragstart zoomstart", () => {
          m.located = true;
        });
        map.on("moveend", () => {
          // 利用者が動かした後は、タイルを出す（zoomstart は表示が変わる前に出るので、動かし終えてから足す）。
          if (m.located) showTiles(m);
          const c = map.getCenter();
          lastView = { lat: c.lat, lng: c.lng, zoom: map.getZoom() };
          setMoved((n) => n + 1);
        });
        // 中央の領域の大きさが変わったら（左右のパネルの開閉など）、地図の大きさを測り直す。
        if (typeof ResizeObserver !== "undefined") {
          observer = new ResizeObserver(() => map.invalidateSize());
          observer.observe(el);
        }
        parts.current = made;
        setMade((g) => g + 1);
        setMoved((n) => n + 1);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setFailed(true);
        reportUiError("地図（Leaflet）を読み込めません", e instanceof Error ? (e.stack ?? e.message) : String(e));
      });
    return () => {
      cancelled = true;
      observer?.disconnect();
      made?.map.remove();
      parts.current = null;
    };
  }, []);

  // まとまりを問い合わせる（地図を動かした・世代・写真の位置が変わった）。続けて動かしている間は待つ。
  const querySeq = useRef(0);
  useEffect(() => {
    const p = parts.current;
    if (!p) return;
    if (generation === null) {
      showTiles(p);
      return;
    }
    const my = ++querySeq.current;
    const timer = setTimeout(() => {
      const b = p.map.getBounds();
      const bounds = viewBounds(...padView(b.getSouth(), b.getWest(), b.getNorth(), b.getEast()));
      if (!bounds) return;
      core
        .mapClusters(generation, bounds, Math.round(p.map.getZoom()))
        .then((m) => {
          if (my !== querySeq.current) return;
          setData(m);
          setError(null);
        })
        .catch((e: unknown) => {
          if (my !== querySeq.current) return;
          const err = toApiError(e);
          // 古い世代は捨てる（新しい世代の問い合わせが続く）。読めなければ、寄せずに今の範囲のタイルを出す。
          if (err.info.kind !== "stale") {
            setError(err.info.message);
            showTiles(p);
          }
        });
    }, MAP_QUERY_DELAY_MS);
    return () => clearTimeout(timer);
  }, [made, moved, generation, locationVersion]);

  // 初めて開いたとき: 写真の全体が見えるように寄せてから（利用者が動かしていなければ、1 回だけ。位置のある写真が
  // なければ既定の範囲のまま）、タイルを出す。
  useEffect(() => {
    const p = parts.current;
    if (!p || !data) return;
    if (!p.located) {
      p.located = true;
      const v = extentView(data.extent);
      if (v) moveTo(p, v, false);
    }
    showTiles(p);
  }, [made, data]);

  // 印を描く（まとまりが変わったときだけ作り直す。表示中の写真が変わっても作り直さない: 1 回目のクリックで写真を
  // 選んだ後に印を作り直すと、ダブルクリックの 2 回目の相手の要素がなくなり、ルーペで開けないため。表示中の写真の
  // 強調は下の effect で付け替える）。
  const currentRef = useRef(current);
  currentRef.current = current;
  useEffect(() => {
    const p = parts.current;
    if (!p) return;
    p.clusters.clearLayers();
    p.markers.clear();
    if (!data) return;
    const L = p.L;
    const centerLng = p.map.getCenter().lng;
    for (const c of data.clusters) {
      const size = clusterSizeClass(c.count);
      const px = CLUSTER_PX[size];
      const isCurrent = c.variant_id === currentRef.current;
      const marker = L.marker([c.lat, markerLon(c.lon, centerLng)], {
        icon: L.divIcon({
          className: `map-cluster ${size}${isCurrent ? " current" : ""}`,
          html: `<span>${formatClusterCount(c.count)}</span>`,
          iconSize: [px, px],
          iconAnchor: [px / 2, px / 2],
        }),
        keyboard: false,
        riseOnHover: true,
      });
      // 重ねたときの説明（サムネイルと数。HTML に入れるのはアプリが作った URL と数だけ）。
      marker.bindTooltip(
        `<img class="map-tip-thumb" src="${thumbUrl(c.variant_id, null)}" alt="" />` +
          `<span class="map-tip-text">${c.count === 1 ? "1 枚" : `${c.count.toLocaleString("ja-JP")} 枚`}</span>`,
        { direction: "top", offset: [0, -px / 2], className: "map-tip" },
      );
      marker.on("click", () => {
        const canZoom = p.map.getZoom() < p.map.getMaxZoom();
        const spread = c.bounds.south !== c.bounds.north || c.bounds.west !== c.bounds.east;
        if (c.count > 1 && canZoom && spread) {
          p.located = true;
          moveTo(p, clusterView(c));
          return;
        }
        void actions.selectIndex(c.index, { expect: c.variant_id });
      });
      marker.on("dblclick", (e: Leaflet.LeafletMouseEvent) => {
        L.DomEvent.stopPropagation(e);
        if (c.count !== 1) return;
        void actions.selectIndex(c.index, { expect: c.variant_id }).then((ok) => {
          if (ok) actions.setMode("loupe");
        });
      });
      p.clusters.addLayer(marker);
      p.markers.set(c.variant_id, marker);
    }
  }, [made, data]);

  // 表示中の写真が代表の印を強調する（印の要素の class だけを付け替える）。
  useEffect(() => {
    const p = parts.current;
    if (!p) return;
    for (const [id, marker] of p.markers) {
      marker.getElement()?.classList.toggle("current", id === current);
    }
  }, [made, data, current]);

  // 表示中の写真の位置（使う位置）。
  useEffect(() => {
    if (current === null) {
      setPhotoGps(null);
      return;
    }
    let alive = true;
    core
      .variantDetails(current)
      .then((d) => alive && setPhotoGps(d.gps ?? null))
      .catch(() => alive && setPhotoGps(null));
    return () => {
      alive = false;
    };
  }, [current, detailsVersion, locationVersion]);

  useEffect(() => {
    const p = parts.current;
    if (!p) return;
    p.photo?.remove();
    p.photo = null;
    if (!photoGps) return;
    const centerLng = p.map.getCenter().lng;
    p.photo = p.L.marker([photoGps.lat, markerLon(photoGps.lon, centerLng)], {
      icon: p.L.divIcon({ className: "geo-photo-mark", iconSize: [14, 14], iconAnchor: [7, 7] }),
      keyboard: false,
      interactive: false,
      zIndexOffset: 1000,
    }).addTo(p.map);
  }, [made, photoGps?.lat, photoGps?.lon, moved]);

  // 「地図の範囲で絞り込む」の間は、地図を動かし終えたら（少し待って）絞り込みの範囲を表示範囲に合わせる。
  useEffect(() => {
    const p = parts.current;
    if (!p || !tracking) return;
    const timer = setTimeout(() => {
      const next = currentViewBounds(p.map);
      if (next && !sameBounds(next, appStore.get().filter.location_bounds)) {
        actions.setFilter({ location_bounds: next });
      }
    }, MAP_FILTER_DELAY_MS);
    return () => clearTimeout(timer);
  }, [made, moved, tracking]);

  const setTracking = (on: boolean) => {
    const p = parts.current;
    if (!on) {
      actions.setFilter({ location_bounds: null });
      return;
    }
    const next = p ? currentViewBounds(p.map) : null;
    if (next) actions.setFilter({ location_bounds: next });
  };

  const fitAll = () => {
    const p = parts.current;
    const v = extentView(data?.extent ?? null);
    if (p && v) {
      p.located = true;
      moveTo(p, v);
    }
  };
  const toPhoto = () => {
    const p = parts.current;
    if (p && photoGps) {
      p.located = true;
      moveTo(p, { kind: "point", center: photoGps, zoom: Math.max(p.map.getZoom(), PIN_ZOOM) });
    }
  };

  return (
    <>
      <div className="map-view-bar" role="toolbar" aria-label="地図">
        <label className="lib-filter-check" title="表示している地図の範囲に位置のある写真だけを表示する（地図を動かすと変わる）">
          <input type="checkbox" checked={tracking} disabled={made === 0} onChange={(e) => setTracking(e.target.checked)} />
          地図の範囲で絞り込む
        </label>
        <button type="button" className="btn small" disabled={!data?.extent} onClick={fitAll} title="位置のある写真がすべて見えるように寄せる">
          写真の全体を表示
        </button>
        <button type="button" className="btn small" disabled={!photoGps} onClick={toPhoto} title="表示中の写真の位置へ地図を動かす">
          表示中の写真へ
        </button>
        <span className="spacer" />
        {error ? (
          <span className="error-text">地図の印を読めません: {error}</span>
        ) : (
          data && <span className="hint">{describeMapCounts(data)}</span>
        )}
      </div>
      <div className="map-view-map">
        <div ref={container} className="geo-map-canvas" aria-label="写真の撮影地の地図" />
        {made === 0 && !failed && (
          <div className="geo-map-status">
            <span className="dlg-busy">
              <span className="spinner" />
              地図を読み込んでいます…
            </span>
          </div>
        )}
        {failed && <div className="geo-map-status error-text">地図を読み込めません（表示を切り替え直してください）</div>}
        <MapAttribution credit={providerAttribution(props.prefs)} />
      </div>
      {tileError && <p className="warn-text hint map-view-note">{MAP_TILE_ERROR}</p>}
    </>
  );
}

/** 地図の今の表示範囲（コアの範囲。読めなければ null）。 */
function currentViewBounds(map: Leaflet.Map) {
  const b = map.getBounds();
  return viewBounds(b.getSouth(), b.getWest(), b.getNorth(), b.getEast());
}

/** 地図を表示へ動かす（範囲なら収まるように。点なら中心とズーム）。`animate` が false なら一度に動かす。 */
function moveTo(p: MapParts, v: MapViewTarget, animate = true): void {
  if (v.kind === "bounds") {
    p.map.fitBounds(
      [
        [v.south, v.west],
        [v.north, v.east],
      ],
      { maxZoom: v.maxZoom, padding: [24, 24], animate },
    );
  } else {
    p.map.setView([v.center.lat, markerLon(v.center.lon, p.map.getCenter().lng)], v.zoom, { animate });
  }
}
