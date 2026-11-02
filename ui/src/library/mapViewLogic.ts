// 地図の表示（LIB-18。表示モード「地図」。library/MapView.tsx）の計算（DOM・Leaflet を使わない純粋な計算）。
//
// - 地図の表示範囲（Leaflet の範囲。経度は世界の写し（左右に続く地図）の分だけ -180〜180 の外に出る）を、コアの
//   GeoBounds（経度は -180〜180。west > east は経度 180 度の線をまたぐ範囲）にする（viewBounds）。幅が 360 度
//   以上なら経度のすべて。緯度は -90〜90 に収める。
// - まとまりを問い合わせる範囲は、表示範囲を上下左右に MAP_QUERY_PADDING ずつ広げたもの（地図を少し動かしても
//   印が欠けないように。印の要求はコアへの問い合わせで、外部への通信ではない）。
// - 印は、地図の中心に最も近い世界の写しに置く（経度 180 度の線の近くを見ているとき、反対側の印が見えるように。
//   markerLon）。
// - 写真の全体を見せる表示（extentView）は、1 点なら PIN_ZOOM の点、広がりがあれば範囲（FIT_MAX_ZOOM まで）。

import type { GeoBounds, MapClusters } from "../api/types";
import { FIT_MAX_ZOOM, type MapView, PIN_ZOOM } from "./geoMapLogic";

/** まとまりを問い合わせる範囲の、表示範囲の外へ広げる割合（上下左右それぞれ。仮置き）。 */
export const MAP_QUERY_PADDING = 0.25;

/** 地図を動かした後、まとまりを問い合わせるまで待つ時間（ms。続けて動かしている間は問い合わせない。仮置き）。 */
export const MAP_QUERY_DELAY_MS = 150;

/** 「地図の範囲で絞り込む」で、地図を動かした後に絞り込みを変えるまで待つ時間（ms。検索を何度もやり直さない。仮置き）。 */
export const MAP_FILTER_DELAY_MS = 600;

/** 経度を -180〜180 に戻す（180 は 180 のまま。-0 は 0）。 */
export function wrapLon(lng: number): number {
  if (lng >= -180 && lng <= 180) return lng + 0;
  return ((((lng + 180) % 360) + 360) % 360) - 180 + 0;
}

/**
 * Leaflet の表示範囲（南・西・北・東。経度は -180〜180 の外に出ることがある）をコアの範囲にする。南北が逆・
 * 数でないなら null。
 */
export function viewBounds(south: number, west: number, north: number, east: number): GeoBounds | null {
  if (![south, west, north, east].every(Number.isFinite) || south > north || west > east) return null;
  const s = Math.max(-90, south) + 0;
  const n = Math.min(90, north) + 0;
  if (s > n) return null;
  if (east - west >= 360) return { south: s, west: -180, north: n, east: 180 };
  let w = wrapLon(west);
  let e = wrapLon(east);
  // 東の端がちょうど 180 度の線（-180 に戻る）なら 180 にする（またがない範囲のまま）。
  if (e === -180 && east > west) e = 180;
  // 西の端がちょうど 180 なら -180 にする（またがない範囲のまま）。
  if (w === 180 && east > west) w = -180;
  return { south: s, west: w, north: n, east: e };
}

/** 表示範囲を上下左右に `ratio` ずつ広げる（Leaflet の範囲のまま。経度は -180〜180 の外に出てよい）。 */
export function padView(
  south: number,
  west: number,
  north: number,
  east: number,
  ratio: number = MAP_QUERY_PADDING,
): [number, number, number, number] {
  const dy = (north - south) * ratio;
  const dx = (east - west) * ratio;
  return [south - dy, west - dx, north + dy, east + dx];
}

/** 印の経度: `lon` に 360 の倍数を足して、地図の中心の経度 `centerLng` に最も近くする。 */
export function markerLon(lon: number, centerLng: number): number {
  return lon + Math.round((centerLng - lon) / 360) * 360;
}

/** 写真の全体を見せる表示（位置のある写真がなければ null）。 */
export function extentView(extent: GeoBounds | null): MapView | null {
  if (!extent) return null;
  if (extent.south === extent.north && extent.west === extent.east) {
    return { kind: "point", center: { lat: extent.south, lon: extent.west }, zoom: PIN_ZOOM };
  }
  return { kind: "bounds", ...extent, maxZoom: FIT_MAX_ZOOM };
}

/** まとまりを押したときに寄せる表示（写真が 1 か所なら PIN_ZOOM の点。広がりがあれば範囲）。 */
export function clusterView(c: { bounds: GeoBounds; lat: number; lon: number }): MapView {
  return extentView(c.bounds) ?? { kind: "point", center: { lat: c.lat, lon: c.lon }, zoom: PIN_ZOOM };
}

/** 印の数の書式（1,000 以上は「1.2k」「12k」）。 */
export function formatClusterCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 10_000) return `${(Math.floor(n / 100) / 10).toFixed(1)}k`;
  return `${Math.floor(n / 1000)}k`;
}

/** 印の大きさの段階（geo.css の .map-cluster.s1〜s4）。 */
export function clusterSizeClass(n: number): "s1" | "s2" | "s3" | "s4" {
  if (n < 10) return "s1";
  if (n < 100) return "s2";
  if (n < 1000) return "s3";
  return "s4";
}

/** 地図の下の帯の数の文。 */
export function describeMapCounts(m: MapClusters): string {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const parts = [`位置のある写真 ${n(m.located)} 件（表示している付近に ${n(m.in_bounds)} 件）`];
  if (m.unlocated > 0) parts.push(`位置のない写真 ${n(m.unlocated)} 件（地図には出ません）`);
  if (m.truncated) parts.push("印が多いため、写真の多い場所だけを出しています（拡大すると全部出ます）");
  return parts.join("・");
}

/** 同じ範囲か（絞り込みの範囲を変えるかの判定。小数 6 桁（約 0.1 m）までで比べる）。 */
export function sameBounds(a: GeoBounds | null | undefined, b: GeoBounds | null | undefined): boolean {
  if (!a || !b) return !a && !b;
  const eq = (x: number, y: number) => Math.abs(x - y) < 1e-6;
  return eq(a.south, b.south) && eq(a.west, b.west) && eq(a.north, b.north) && eq(a.east, b.east);
}

/** 範囲の短い文（絞り込みの札の説明。南西・北東の角。小数 4 桁（約 10 m））。 */
export function describeBounds(b: GeoBounds): string {
  const f = (v: number) => v.toFixed(4);
  const cross = b.west > b.east ? "（経度 180 度の線をまたぐ）" : "";
  return `南西 ${f(b.south)}, ${f(b.west)} 〜 北東 ${f(b.north)}, ${f(b.east)}${cross}`;
}
