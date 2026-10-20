// Leaflet 1.9.4（BSD-2-Clause。docs/third_party.md）と、その CSS を読む（位置情報の後付け LIB-21 の地図）。
//
// このファイルは library/GeoMap.tsx の useEffect の中から動的 import（import("./leafletLoader")）でだけ読む。
// Leaflet は読み込んだ時点で document・window を触るので、静的に import すると node のテスト（render.test.ts →
// Dialogs.tsx → GeotagDialog.tsx → GeoMap.tsx の経路）が落ちる。動的に読むと、ビルドでは Leaflet とその CSS が
// 別のチャンクになり、地図を開くまで読み込まない。
//
// - CSS: leaflet.css の url()（images/layers.png など。どれも 4 KB 未満）は Vite が data: の URL にして埋め込む
//   （CSP の img-src は data: を許している）。既定のマーカーの画像は使わない（GeoMap.tsx は L.divIcon）。
// - geoLeaflet.css（Leaflet の部品の暗い配色への上書き）は leaflet.css の **後** に読む（同じ詳細度の規則を
//   後のもので上書きするため。順を変えない）。

import * as leaflet from "leaflet";
import "leaflet/dist/leaflet.css";
import "./geoLeaflet.css";

export type Leaflet = typeof leaflet;

/** Leaflet（UMD を ESM として読むので、ビルドの形によっては default の下にある）。 */
const L: Leaflet = (leaflet as unknown as { default?: Leaflet }).default ?? leaflet;

export default L;
