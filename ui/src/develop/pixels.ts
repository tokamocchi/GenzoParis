// プレビューの画素とヒストグラムの計算（DEV-26。DOM を使わない純粋な計算）。

import type { HistogramData } from "../api/types";

/** 白飛び（いずれかのチャンネルが 255）を重ねる色（RGBA）。 */
export const CLIP_HIGH_COLOR = [255, 0, 0, 255] as const;
/** 黒つぶれ（すべてのチャンネルが 0）を重ねる色（RGBA）。 */
export const CLIP_LOW_COLOR = [0, 80, 255, 255] as const;

export interface ClippingOverlay {
  /** 重ねる画素（RGBA。クリップしていない画素は透明）。 */
  data: Uint8ClampedArray<ArrayBuffer>;
  /** 白飛びの画素の数。 */
  high: number;
  /** 黒つぶれの画素の数。 */
  low: number;
}

/**
 * プレビューの画素（RGBA 8bit。画面用の値）から、クリッピングの表示の画素を作る。
 *
 * - 白飛び: R・G・B のいずれかが 255 → 赤。
 * - 黒つぶれ: R・G・B のすべてが 0 → 青。
 * - それ以外は透明（A = 0）。
 *
 * コアのヒストグラムの clipped_high.any・clipped_low.all と同じ判定（階級 255・階級 0）。
 */
export function clippingOverlay(pixels: ArrayLike<number>, width: number, height: number): ClippingOverlay {
  const n = Math.max(0, Math.floor(width) * Math.floor(height));
  const data = new Uint8ClampedArray(n * 4);
  let high = 0;
  let low = 0;
  const len = Math.min(n * 4, pixels.length - (pixels.length % 4));
  for (let i = 0; i < len; i += 4) {
    const r = pixels[i];
    const g = pixels[i + 1];
    const b = pixels[i + 2];
    if (r === 255 || g === 255 || b === 255) {
      data[i] = CLIP_HIGH_COLOR[0];
      data[i + 1] = CLIP_HIGH_COLOR[1];
      data[i + 2] = CLIP_HIGH_COLOR[2];
      data[i + 3] = CLIP_HIGH_COLOR[3];
      high++;
    } else if (r === 0 && g === 0 && b === 0) {
      data[i] = CLIP_LOW_COLOR[0];
      data[i + 1] = CLIP_LOW_COLOR[1];
      data[i + 2] = CLIP_LOW_COLOR[2];
      data[i + 3] = CLIP_LOW_COLOR[3];
      low++;
    }
  }
  return { data, high, low };
}

/** スポットの可視化（DEV-21）の感度の範囲（%）と既定の値（**仮置き**: 合成の画像で決めた）。 */
export const SPOT_VISUAL_SENSITIVITY = { min: 0, max: 100, default: 50 } as const;

/**
 * スポットの可視化の、輪郭とみなす明るさの変化のしきい値（8bit の輝度の、Sobel の勾配の大きさ）。感度 0% で 96、
 * 100% で 4（指数で補間。感度を上げるほど、淡いゴミの輪郭も白くなる）。
 */
export function spotVisualThreshold(sensitivity: number): number {
  const r = SPOT_VISUAL_SENSITIVITY;
  const t = (Math.min(r.max, Math.max(r.min, Number.isFinite(sensitivity) ? sensitivity : r.default)) - r.min) / (r.max - r.min);
  return 96 * Math.pow(4 / 96, t);
}

/**
 * スポットの可視化（DEV-21。センサーのゴミ・小さな写り込みを見つけやすくする白黒の輪郭の表示）の画素を作る。
 *
 * プレビューの画素（RGBA 8bit。画面用の値）の輝度（Rec. 709 の重み）の勾配（3 × 3 の Sobel）の大きさが、感度から
 * 決めたしきい値（[`spotVisualThreshold`]）以上の画素を白、それ以外を黒にした、不透明な画像（プレビューに重ねて
 * 隠す）。画像の縁の 1 画素は、画像の中の最も近い画素の値を外にも使う。現像の結果は変えない（表示だけ）。
 */
export function spotVisualization(
  pixels: ArrayLike<number>,
  width: number,
  height: number,
  sensitivity: number,
): Uint8ClampedArray<ArrayBuffer> {
  const w = Math.max(0, Math.floor(width));
  const h = Math.max(0, Math.floor(height));
  const n = w * h;
  const data = new Uint8ClampedArray(n * 4);
  if (n === 0 || pixels.length < n * 4) return data;
  const lum = new Float32Array(n);
  for (let i = 0, p = 0; i < n; i++, p += 4) {
    lum[i] = 0.2126 * pixels[p] + 0.7152 * pixels[p + 1] + 0.0722 * pixels[p + 2];
  }
  const limit = spotVisualThreshold(sensitivity);
  const limit2 = limit * limit;
  for (let y = 0; y < h; y++) {
    const y0 = (y > 0 ? y - 1 : 0) * w;
    const y1 = y * w;
    const y2 = (y < h - 1 ? y + 1 : y) * w;
    for (let x = 0; x < w; x++) {
      const x0 = x > 0 ? x - 1 : 0;
      const x2 = x < w - 1 ? x + 1 : x;
      const a = lum[y0 + x0];
      const b = lum[y0 + x];
      const c = lum[y0 + x2];
      const d = lum[y1 + x0];
      const f = lum[y1 + x2];
      const g = lum[y2 + x0];
      const hh = lum[y2 + x];
      const k = lum[y2 + x2];
      const gx = c + 2 * f + k - (a + 2 * d + g);
      const gy = g + 2 * hh + k - (a + 2 * b + c);
      // Sobel の重みの和（4）で割って、隣の画素との差の大きさにそろえる。
      const v = (gx * gx + gy * gy) / 16 >= limit2 ? 255 : 0;
      const o = (y1 + x) * 4;
      data[o] = v;
      data[o + 1] = v;
      data[o + 2] = v;
      data[o + 3] = 255;
    }
  }
  return data;
}

/**
 * ヒストグラムの縦の縮尺（棒の高さ 1 に当たる度数）。
 *
 * 両端の階級（0 と 255）は白飛び・黒つぶれで極端に大きくなりやすく、それに合わせると他の階級が
 * 見えなくなるため、内側の階級（1〜254）の最大値に合わせる（両端は上で切れる）。内側がすべて 0 なら
 * 全体の最大値、それも 0 なら 1。
 */
export function histogramScale(channels: readonly (readonly number[])[]): number {
  let inner = 0;
  let all = 0;
  for (const ch of channels) {
    for (let i = 0; i < ch.length; i++) {
      const v = ch[i];
      if (!(v > 0)) continue;
      if (v > all) all = v;
      if (i > 0 && i < ch.length - 1 && v > inner) inner = v;
    }
  }
  return inner > 0 ? inner : all > 0 ? all : 1;
}

/** 階級ごとの棒の高さ（0〜1。`scale` で割って 1 で切る）。 */
export function histogramHeights(bins: readonly number[], scale: number): number[] {
  const s = scale > 0 ? scale : 1;
  return bins.map((v) => Math.min(1, Math.max(0, v / s)));
}

/** 白飛び・黒つぶれの印を点けるか（clipped_high.any・clipped_low.any が 0 でなければ）。 */
export function clipIndicators(h: HistogramData | null | undefined): { high: boolean; low: boolean } {
  return { high: !!h && h.clipped_high.any > 0, low: !!h && h.clipped_low.any > 0 };
}

/** 割合の表示（"0.12 %"。画素数が 0 なら "—"）。 */
export function formatRatio(count: number, total: number): string {
  if (!(total > 0)) return "—";
  return `${((count / total) * 100).toFixed(2)} %`;
}
