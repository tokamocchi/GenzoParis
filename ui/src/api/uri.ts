// genzo:// の URI（サムネイル・標準プレビュー・動画・地図のタイル。genzo-app の protocol.rs）。
//
// Windows（WebView2）は http://genzo.localhost/...、macOS・Linux は genzo://localhost/... の形。

import type { VariantId } from "./types";

let windowsStyle: boolean | null = null;

function base(): string {
  if (windowsStyle === null) {
    windowsStyle = typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);
  }
  return windowsStyle ? "http://genzo.localhost" : "genzo://localhost";
}

/** テスト用: URI の形を決める。 */
export function setWindowsStyleForTest(v: boolean | null): void {
  windowsStyle = v;
}

/** サムネイル（L0）。`rev` が変わると URI が変わる（ブラウザのキャッシュを効かせる）。 */
export function thumbUrl(id: VariantId, rev: string | null | undefined): string {
  return `${base()}/thumb/${id}${rev ? `?rev=${encodeURIComponent(rev)}` : ""}`;
}

/** 標準プレビュー（L1）。なければ 404 になり、作り終えたら thumbnail_updated のイベントが来る。 */
export function previewUrl(id: VariantId, rev: string | null | undefined): string {
  return `${base()}/preview/${id}${rev ? `?rev=${encodeURIComponent(rev)}` : ""}`;
}

/** 動画の元のファイル（VID-04。Range の要求に応える。カタログに登録された動画だけ）。 */
export function videoUrl(id: VariantId): string {
  return `${base()}/video/${id}`;
}

/**
 * 動画の再生用のファイル（H.264 ＋ AAC の MP4。なければ 404）。`rev`（作った再生用のファイルの大きさなど）を
 * 付けると、作り直した後に別の URI になる（WebView に古い読み込みを使わせない）。
 */
export function videoProxyUrl(id: VariantId, rev?: string | number | null): string {
  return `${base()}/video-proxy/${id}${rev !== undefined && rev !== null ? `?rev=${encodeURIComponent(String(rev))}` : ""}`;
}

/**
 * 地図のタイルの URL のひな形（Leaflet の TileLayer に渡す。{z}・{x}・{y} は Leaflet が埋める。位置情報の後付け
 * LIB-21 の第 2 段階）。genzo-app がオンラインの地図の提供元から取ってディスクにキャッシュし、返す（WebView は
 * 外部と通信しない）。拡張子は付けない（protocol.rs が受け付けるのは `/tile/{z}/{x}/{y}` だけ）。
 */
export function tileUrlTemplate(): string {
  return `${base()}/tile/{z}/{x}/{y}`;
}
