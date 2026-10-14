// 現像のプレビュー・表示範囲の画素（genzo-app の frames.rs の形）を読み、canvas に描く。
//
// 形: [ヘッダーの JSON の長さ（u32 LE）][ヘッダーの JSON][RGBA 8bit]

import { bridge } from "./bridge";
import { toApiError } from "./errors";
import type { DisplayTarget, FrameInfo, MaskOverlayInfo, RegionInfo, VariantId } from "./types";

export interface Frame<H> {
  header: H;
  width: number;
  height: number;
  /** RGBA。 */
  pixels: Uint8ClampedArray<ArrayBuffer>;
}

/** バイト列を読む（空なら null）。 */
export function decodeFrame<H extends { width: number; height: number }>(
  buf: ArrayBuffer,
): Frame<H> | null {
  if (buf.byteLength === 0) return null;
  if (buf.byteLength < 4) throw new Error("画素のデータが短すぎます");
  const view = new DataView(buf);
  const len = view.getUint32(0, true);
  if (4 + len > buf.byteLength) throw new Error("画素のヘッダーが壊れています");
  const header = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 4, len))) as H;
  const offset = 4 + len;
  const expected = header.width * header.height * 4;
  if (buf.byteLength - offset !== expected) {
    throw new Error(`画素の長さが合いません（${buf.byteLength - offset} / ${expected}）`);
  }
  return {
    header,
    width: header.width,
    height: header.height,
    pixels: new Uint8ClampedArray(buf, offset, expected),
  };
}

async function fetchFrame<H extends { width: number; height: number }>(
  command: string,
  args?: Record<string, unknown>,
): Promise<Frame<H> | null> {
  try {
    return decodeFrame<H>(await bridge().invokeBinary(command, args));
  } catch (e) {
    throw toApiError(e);
  }
}

/** 現像中の写真の最新のプレビュー。 */
export const fetchDevelopFrame = () => fetchFrame<FrameInfo>("develop_frame");
/** 表示範囲の描画の最新の結果。 */
export const fetchRegionFrame = () => fetchFrame<RegionInfo>("region_frame");
/** ビフォー / アフターの表示のビフォーの最新のプレビュー（DEV-31）。 */
export const fetchBeforeFrame = () => fetchFrame<FrameInfo>("before_frame");
/** ビフォーの表示範囲の描画の最新の結果（DEV-31）。 */
export const fetchBeforeRegionFrame = () => fetchFrame<RegionInfo>("before_region_frame");
/** 比較表示（LIB-03）の面 `pane` の表示範囲の描画の最新の結果。 */
export const fetchViewRegionFrame = (pane: number) => fetchFrame<RegionInfo>("view_region_frame", { pane });

/**
 * マスク `index` の重ねて表示（DEV-18。画素の R・G・B に重み 0〜255。プレビューと同じ格子）。`uncropped` は
 * クリックした画像の FrameInfo.uncropped（省略時は最新に公開したプレビューの表示）。
 */
export const fetchMaskOverlay = (variant_id: VariantId, index: number, uncropped?: boolean) =>
  fetchFrame<MaskOverlayInfo>("mask_overlay_frame", {
    variantId: variant_id,
    index,
    ...(uncropped === undefined ? {} : { uncropped }),
  });

let p3Support: boolean | null = null;

/** canvas が Display P3 を使えるか（WebView2 は Chromium 111 以降、WKWebView は Safari 15.2 以降）。 */
export function canvasSupportsP3(): boolean {
  if (p3Support !== null) return p3Support;
  try {
    const c = document.createElement("canvas");
    const ctx = c.getContext("2d", { colorSpace: "display-p3" } as CanvasRenderingContext2DSettings);
    p3Support =
      !!ctx &&
      (ctx as CanvasRenderingContext2D & { getContextAttributes?: () => { colorSpace?: string } })
        .getContextAttributes?.()?.colorSpace === "display-p3";
  } catch {
    p3Support = false;
  }
  return p3Support;
}

/** 画面の色空間（コアに伝える値）。 */
export function displayTarget(): DisplayTarget {
  return canvasSupportsP3() ? "display_p3" : "srgb";
}

/** canvas の 2D コンテキスト（色空間をコアの出力に合わせる）。 */
export function context2d(canvas: HTMLCanvasElement): CanvasRenderingContext2D | null {
  const colorSpace = canvasSupportsP3() ? "display-p3" : "srgb";
  return canvas.getContext("2d", { colorSpace } as CanvasRenderingContext2DSettings);
}

/** 画素を canvas に描く（canvas の大きさを画素に合わせる）。 */
export function drawFrame(canvas: HTMLCanvasElement, frame: Frame<unknown>): void {
  if (canvas.width !== frame.width) canvas.width = frame.width;
  if (canvas.height !== frame.height) canvas.height = frame.height;
  const ctx = context2d(canvas);
  if (!ctx) return;
  const colorSpace = canvasSupportsP3() ? "display-p3" : "srgb";
  const image = new ImageData(frame.pixels, frame.width, frame.height, {
    colorSpace,
  } as ImageDataSettings);
  ctx.putImageData(image, 0, 0);
}
