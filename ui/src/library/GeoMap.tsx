// 位置を付けるダイアログの地図（位置情報の後付け LIB-21 の第 2 段階。docs/04 の 3.16 節）。
//
// - Leaflet は useEffect の中で動的に読む（library/leafletLoader.ts。node のテストでは読まない。テストはこの部品を
//   描くだけで、地図は作らない）。読み込むまでは「地図を読み込んでいます」を出す。
// - タイルは genzo:// の /tile/{z}/{x}/{y}（api/uri.ts の tileUrlTemplate）。genzo-app がオンラインの地図の提供元から
//   取り、ディスクにキャッシュして返す（WebView は外部と通信しない。CSP は変えない）。オンラインの地図が無効なら
//   genzo-app が 403 を返すので、この部品は有効なときだけ出す（GeotagDialog）。
// - タイルの層: 最大のズームは MAP_MAX_ZOOM。サブドメイン・detectRetina・crossOrigin は使わない（要求を増やさない）。
//   先読みはしない（Leaflet が取るのは、表示している範囲に一部でもかかるタイルだけ。keepBuffer（既定の 2）は、
//   地図を動かしたときに表示の外に出た読み終えたタイルを、捨てずに残しておく範囲で、新しくは取らない。Leaflet 1.9.4
//   の GridLayer の _update。版を上げたら確かめ直す）。
// - 帰属の表示「© OpenStreetMap contributors」は、Leaflet の帰属の表示（Leaflet へのリンクと旗が付き、リンクを押すと
//   WebView が外部のページへ移る）を使わず、この部品が地図の右下に常に出す（Leaflet の前から出し、畳まない）。押すと
//   既定のブラウザで OSM の著作権のページを開く（openMapLink）。「地図の誤りを報告」（fixthemap）も並べる。
// - ピン（付ける位置）と表示中の写真の位置の印は L.divIcon（CSS だけ。既定のアイコンの画像のパスの自動判定は Vite の
//   ビルドで壊れるので使わない）。地図をクリックするか、ピンをドラッグして離すと onPick に位置を渡す（親が座標の欄に
//   入れ、読んだ座標が pin として戻る）。表示中の写真の印を押すと、その位置を渡す。
// - 初めの表示は、付ける位置か表示中の写真の位置（なければ既定の範囲）。後から届いた位置も、利用者がまだ地図を
//   動かしていなければ、そこへ動かす。pin が表示の外に変わったら、そこまで動かす。
// - React の StrictMode の 2 回の初期化に備え、片付けで map.remove() し、読み込みの途中で外れたら作らない。
// - Leaflet のキーボードの操作は使わない（Dialog が keydown の伝わりを止めるので効かない）。マウスのドラッグ・
//   ホイール・ズームのボタンで動かす。

import type * as Leaflet from "leaflet";
import { useEffect, useRef, useState } from "react";

import { reportUiError } from "../api/app";
import type { GpsCoord } from "../api/types";
import { tileUrlTemplate } from "../api/uri";
import { openMapPage } from "../state/map";
import {
  initialMapView,
  MAP_LINK_LABELS,
  MAP_MAX_ZOOM,
  MAP_TILE_ERROR,
  type MapView,
  mapPointToGps,
  PIN_ZOOM,
} from "./geoMapLogic";
import "./geo.css";

/** 地図を動かす指示（検索の結果を選んだときなど。`seq` が変わったときだけ動かす）。 */
export interface MapFocus {
  seq: number;
  view: MapView;
}

/** 作った地図と印。 */
interface MapParts {
  L: typeof Leaflet;
  map: Leaflet.Map;
  pin: Leaflet.Marker | null;
  photo: Leaflet.Marker | null;
  /** 付ける位置・写真の位置・検索の結果へ動かしたか、利用者が動かしたか（後から届いた位置へ勝手に動かさない）。 */
  located: boolean;
}

/** ピンの印（geo.css の .geo-pin。先が位置）。 */
const pinIcon = (L: typeof Leaflet) =>
  L.divIcon({ className: "geo-pin", iconSize: [24, 24], iconAnchor: [12, 22] });
/** 表示中の写真の位置の印（geo.css の .geo-photo-mark。中心が位置）。 */
const photoIcon = (L: typeof Leaflet) =>
  L.divIcon({ className: "geo-photo-mark", iconSize: [14, 14], iconAnchor: [7, 7] });

export function GeoMap(props: {
  /** 付ける位置（座標の欄で読めた値。なければ null）。 */
  pin: GpsCoord | null;
  /** 表示中の写真の使う位置。 */
  photo: GpsCoord | null;
  focus: MapFocus | null;
  /** 地図で位置を選んだ（クリック・ピンのドラッグ・写真の印）。 */
  onPick: (g: GpsCoord) => void;
  /** 選べない（処理中）。 */
  disabled?: boolean;
}) {
  const container = useRef<HTMLDivElement>(null);
  const parts = useRef<MapParts | null>(null);
  // 地図を作るたびに増やす（印の effect を作り直した地図に当て直す）。0 はまだない。
  const [generation, setGeneration] = useState(0);
  const [failed, setFailed] = useState(false);
  const [tileError, setTileError] = useState(false);
  // 地図の作成・イベントの中から、最新の props を読む。
  const latest = useRef(props);
  latest.current = props;

  const pick = (lat: number, lng: number) => {
    if (!latest.current.disabled) latest.current.onPick(mapPointToGps(lat, lng));
  };

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
        const first = initialMapView(latest.current.pin, latest.current.photo);
        const map = L.map(el, {
          attributionControl: false,
          keyboard: false,
          maxZoom: MAP_MAX_ZOOM,
          worldCopyJump: true,
        });
        map.setView([first.center.lat, first.center.lon], first.zoom);
        L.tileLayer(tileUrlTemplate(), { maxZoom: MAP_MAX_ZOOM })
          .on("tileerror", () => setTileError(true))
          .on("tileload", () => setTileError(false))
          .addTo(map);
        map.on("click", (e: Leaflet.LeafletMouseEvent) => pick(e.latlng.lat, e.latlng.lng));
        made = { L, map, pin: null, photo: null, located: first.located };
        const m = made;
        map.on("dragstart", () => {
          m.located = true;
        });
        // ダイアログの大きさが変わったら（スクロールバーが出たときなど）、地図の大きさを測り直す。
        if (typeof ResizeObserver !== "undefined") {
          observer = new ResizeObserver(() => map.invalidateSize());
          observer.observe(el);
        }
        parts.current = made;
        setGeneration((g) => g + 1);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setFailed(true);
        // 読み込めない（ビルドの不具合など）ときは、アプリのログに残す。
        reportUiError("地図（Leaflet）を読み込めません", e instanceof Error ? (e.stack ?? e.message) : String(e));
      });
    return () => {
      cancelled = true;
      observer?.disconnect();
      made?.map.remove();
      parts.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 検索の結果などへ動かす（ピンより先に。動かした後の範囲でピンが見えるかを判定する）。
  const focus = props.focus;
  useEffect(() => {
    const p = parts.current;
    if (!p || !focus) return;
    const v = focus.view;
    if (v.kind === "bounds") {
      p.map.fitBounds(
        [
          [v.south, v.west],
          [v.north, v.east],
        ],
        { maxZoom: v.maxZoom, animate: false },
      );
    } else {
      p.map.setView([v.center.lat, v.center.lon], v.zoom, { animate: false });
    }
    p.located = true;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [generation, focus?.seq]);

  // ピン（付ける位置）。
  const pin = props.pin;
  useEffect(() => {
    const p = parts.current;
    if (!p) return;
    if (!pin) {
      p.pin?.remove();
      p.pin = null;
      return;
    }
    const at = p.L.latLng(pin.lat, pin.lon);
    if (p.pin) {
      p.pin.setLatLng(at);
    } else {
      const marker = p.L.marker(at, {
        icon: pinIcon(p.L),
        draggable: true,
        keyboard: false,
        zIndexOffset: 1000,
        title: "付ける位置（ドラッグで動かせます）",
      });
      marker.on("dragend", () => {
        const g = marker.getLatLng();
        pick(g.lat, g.lng);
      });
      p.pin = marker.addTo(p.map);
    }
    if (!p.located) {
      p.map.setView(at, PIN_ZOOM);
      p.located = true;
    } else if (!p.map.getBounds().contains(at)) {
      p.map.panTo(at);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [generation, pin?.lat, pin?.lon]);

  // 表示中の写真の位置の印。
  const photo = props.photo;
  useEffect(() => {
    const p = parts.current;
    if (!p) return;
    p.photo?.remove();
    p.photo = null;
    if (!photo) return;
    const at = p.L.latLng(photo.lat, photo.lon);
    const marker = p.L.marker(at, {
      icon: photoIcon(p.L),
      keyboard: false,
      title: "表示中の写真の位置（押すとこの位置を入れます）",
    });
    marker.on("click", () => pick(photo.lat, photo.lon));
    p.photo = marker.addTo(p.map);
    if (!p.located) {
      p.map.setView(at, PIN_ZOOM);
      p.located = true;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [generation, photo?.lat, photo?.lon]);

  return (
    <>
      <div className="geo-map">
        <div ref={container} className="geo-map-canvas" aria-label="地図" />
        {generation === 0 && !failed && (
          <div className="geo-map-status">
            <span className="dlg-busy">
              <span className="spinner" />
              地図を読み込んでいます…
            </span>
          </div>
        )}
        {failed && <div className="geo-map-status error-text">地図を読み込めません（画面を開き直してください）</div>}
        <div className="geo-attribution">
          <button
            type="button"
            className="geo-link"
            onClick={() => openMapPage("copyright")}
            title="OpenStreetMap の著作権とライセンスのページを既定のブラウザで開きます"
          >
            {MAP_LINK_LABELS.copyright}
          </button>
          <span className="geo-sep" aria-hidden="true">
            |
          </span>
          <button
            type="button"
            className="geo-link"
            onClick={() => openMapPage("fix_the_map")}
            title="OpenStreetMap の地図の誤りを報告するページを既定のブラウザで開きます"
          >
            {MAP_LINK_LABELS.fix_the_map}
          </button>
        </div>
      </div>
      {tileError && <p className="warn-text hint">{MAP_TILE_ERROR}</p>}
    </>
  );
}
