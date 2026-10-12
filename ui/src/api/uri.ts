// genzo:// の URI（サムネイル・標準プレビュー。genzo-app の protocol.rs）。
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
