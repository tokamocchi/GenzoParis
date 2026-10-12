// ルーペとフィルムストリップの計算（LIB-02・PRV-03・PERF-04。DOM を使わない純粋な計算）。
//
// 座標の約束:
//
// - 「CSS の画素」は画面の要素の座標（devicePixelRatio を掛ける前）。「物理画素」は CSS の画素 ×
//   devicePixelRatio。
// - 「出力の画素」は出力の画像（C。切り抜いた後）のフル解像度の画素（genzo-api の RegionSpace::Output）。
// - 倍率 `scale` は出力の 1 画素を画面の何物理画素で表すか（1 が等倍。genzo-api の RegionRequest::scale と
//   同じ意味）。画面に出す大きさ（CSS の画素）は「出力の画素 × scale / devicePixelRatio」になる。

import type { FrameInfo, Orientation } from "../api/types";

/** 矩形（左上と大きさ）。 */
export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 全体表示で、画像のまわりに空ける余白（CSS の画素。仮置き）。 */
export const FIT_MARGIN_PX = 12;

/** パンを離してから、見えている範囲を描き直すまでの待ち（ミリ秒。仮置き）。 */
export const PAN_SETTLE_MS = 120;

/** クリックとドラッグを分ける動きの大きさ（CSS の画素。仮置き）。 */
export const DRAG_THRESHOLD_PX = 4;

/**
 * 1 回の表示範囲の描画で要求する画素数の上限。genzo-api の region.rs の MAX_REGION_SOURCE_PIXELS
 * （16 Mi 画素。仮置き）と同じ値にする（超えると InvalidArgument になる）。
 */
export const MAX_REGION_PIXELS = 16 * 1024 * 1024;

/** 先読みする標準プレビュー（L1）の数: 進む向きに 2 枚、戻る向きに 1 枚（仮置き。PERF-04）。 */
export const PREFETCH_AHEAD = 2;
export const PREFETCH_BEHIND = 1;

/** 先読みした画像を持っておく数（仮置き。L1 は長辺 2560 の JPEG）。 */
export const PREFETCH_KEEP = 6;

/**
 * 写真送りの途中とみなす間隔（ミリ秒。仮置き）。前の写真からこれより短い間で移ったら、L1 の要求と先読みを
 * この時間だけ待つ（←→ を押し続けている間に、通り過ぎる写真ごとに L1 を作る処理を予約しない）。
 */
export const L1_SETTLE_MS = 150;

/**
 * L1 の作成待ちのまま、作ってあるかを 1 回だけ確かめるまでの時間（ミリ秒。仮置き。知らせの取りこぼしへの
 * 備え。確かめるのは作る処理を予約しない preview_path）。
 */
export const L1_CHECK_MS = 5000;

/**
 * 等倍表示の準備で、現像のプレビュー（出力の大きさが入っている）が届くのを待つ時間（ミリ秒。仮置き）。
 * 過ぎたら render_develop で描いて求める。
 */
export const FRAME_WAIT_MS = 1500;

/**
 * フィルムストリップのスクロールの領域の幅の上限（px。仮置き）。Chromium・WebKit の要素の大きさの上限
 * （LayoutUnit で約 3,355 万 px）より十分小さくする（50 万件 × 96 px = 4,800 万 px は超える）。
 * library/gridMath.ts の MAX_SCROLL_PX と同じ値。
 */
export const MAX_FILMSTRIP_SCROLL_PX = 8_000_000;

// ---------------------------------------------------------------------------
// 全体表示
// ---------------------------------------------------------------------------

/** 向き（EXIF の 1〜8）を反映した大きさ（5〜8 は縦横が入れ替わる）。分からなければ null。 */
export function orientedSize(
  width: number | null | undefined,
  height: number | null | undefined,
  orientation: Orientation,
): { width: number; height: number } | null {
  if (!width || !height || width <= 0 || height <= 0) return null;
  return orientation >= 5 ? { width: height, height: width } : { width, height };
}

/** 画像（contentW × contentH）を表示（viewW × viewH）に収めて中央に置く矩形（CSS の画素）。 */
export function fitRect(
  contentW: number,
  contentH: number,
  viewW: number,
  viewH: number,
  margin = FIT_MARGIN_PX,
): Rect {
  const availW = Math.max(0, viewW - margin * 2);
  const availH = Math.max(0, viewH - margin * 2);
  if (contentW <= 0 || contentH <= 0 || availW <= 0 || availH <= 0) {
    return { x: viewW / 2, y: viewH / 2, width: 0, height: 0 };
  }
  const k = Math.min(availW / contentW, availH / contentH);
  const width = contentW * k;
  const height = contentH * k;
  return { x: (viewW - width) / 2, y: (viewH - height) / 2, width, height };
}

/**
 * 表示の中の点（CSS の画素）を、矩形の中の割合（0〜1）にする。矩形の外なら `inside` が偽で、値は
 * 矩形の端に収める。
 */
export function normalizedPoint(px: number, py: number, rect: Rect): { u: number; v: number; inside: boolean } {
  if (rect.width <= 0 || rect.height <= 0) return { u: 0.5, v: 0.5, inside: false };
  const u = (px - rect.x) / rect.width;
  const v = (py - rect.y) / rect.height;
  const inside = u >= 0 && u <= 1 && v >= 0 && v <= 1;
  return { u: clamp(u, 0, 1), v: clamp(v, 0, 1), inside };
}

/** 割合（0〜1）の点を、出力の画素の座標にする。 */
export function normalizedToOutput(u: number, v: number, fullW: number, fullH: number): { x: number; y: number } {
  return { x: clamp(u, 0, 1) * fullW, y: clamp(v, 0, 1) * fullH };
}

// ---------------------------------------------------------------------------
// 等倍表示（拡大とパン）
// ---------------------------------------------------------------------------

/**
 * 表示の中心（出力の画素）を、画像の外が見えないように収める。画像が表示より小さい向きは中央に置く。
 *
 * @param viewCss 表示の大きさ（その向きの CSS の画素）
 */
export function clampCenter(center: number, full: number, viewCss: number, dpr: number, scale = 1): number {
  const visible = (viewCss * dpr) / scale;
  if (full <= visible) return full / 2;
  return clamp(center, visible / 2, full - visible / 2);
}

/** ドラッグの動き（CSS の画素）から、新しい中心（出力の画素）を求める（画像は指の動きについてくる）。 */
export function pannedCenter(startCenter: number, dragCss: number, dpr: number, scale = 1): number {
  return startCenter - (dragCss * dpr) / scale;
}

/** 拡大した画像の左上の位置（表示の中の CSS の画素）。中心が表示の中央に来る。 */
export function zoomOffset(
  center: { x: number; y: number },
  view: { width: number; height: number },
  dpr: number,
  scale = 1,
): { x: number; y: number } {
  return {
    x: view.width / 2 - (center.x * scale) / dpr,
    y: view.height / 2 - (center.y * scale) / dpr,
  };
}

/**
 * 位置（CSS の画素）を物理画素の境界に合わせる（HiDPI・表示倍率 150% でも画像の 1 画素が画面の 1 物理画素に
 * 重なるようにする。UX-03）。`origin` は表示の要素のページの中の位置（CSS の画素）。
 */
export function snapToDevicePixels(offsetCss: number, originCss: number, dpr: number): number {
  if (!(dpr > 0)) return offsetCss;
  return Math.round((originCss + offsetCss) * dpr) / dpr - originCss;
}

/**
 * 範囲を各辺 `px` 画素ずつ広げる（画像の内側に収める）。位置を物理画素の境界に合わせる丸め
 * （snapToDevicePixels）と、regionFor の左上の切り捨てで、見えている範囲の端の 1 画素が範囲から外れない
 * ようにする。
 */
export function expandRegion(region: Rect, px: number, fullW: number, fullH: number): Rect {
  const x0 = Math.max(0, region.x - px);
  const y0 = Math.max(0, region.y - px);
  const x1 = Math.min(fullW, region.x + region.width + px);
  const y1 = Math.min(fullH, region.y + region.height + px);
  return { x: x0, y: y0, width: Math.max(1, x1 - x0), height: Math.max(1, y1 - y0) };
}

/**
 * 要求する範囲の画素数を上限までに抑える（中心を保って縦横を同じ割合で縮める。外れた部分は拡大した
 * 標準プレビューのまま見える）。値は整数（左上は切り捨て、大きさは 1 以上）。
 */
export function limitRegion(region: Rect, maxPixels = MAX_REGION_PIXELS): Rect {
  const area = region.width * region.height;
  if (area <= maxPixels) return region;
  const k = Math.sqrt(maxPixels / area);
  const width = Math.max(1, Math.floor(region.width * k));
  const height = Math.max(1, Math.floor(region.height * k));
  const cx = region.x + region.width / 2;
  const cy = region.y + region.height / 2;
  return { x: Math.floor(cx - width / 2), y: Math.floor(cy - height / 2), width, height };
}

/** 同じ矩形か。 */
export function sameRect(a: Rect, b: Rect): boolean {
  return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;
}

/**
 * 描いてある範囲（`have`。genzo-api の RegionInfo）を、そのまま使えるか。出力の大きさが同じで、要る範囲
 * `need` を含み、描いた現像設定の世代が `generation`（分かっている最新の世代。分からなければ null）より
 * 古くないこと。現像設定が外から変わった（設定の貼り付け・Undo など）ら描き直す。
 */
export function regionUsable(
  have:
    | {
        x: number;
        y: number;
        region_width: number;
        region_height: number;
        full_width: number;
        full_height: number;
        generation: number;
      }
    | null
    | undefined,
  need: Rect,
  fullW: number,
  fullH: number,
  generation: number | null,
): boolean {
  if (!have || have.full_width !== fullW || have.full_height !== fullH) return false;
  if (generation !== null && have.generation < generation) return false;
  return regionCovers({ x: have.x, y: have.y, width: have.region_width, height: have.region_height }, need);
}

/** 描いてある範囲 `have` が、要る範囲 `need` を含むか（含めば描き直さない）。 */
export function regionCovers(have: Rect, need: Rect): boolean {
  return (
    have.x <= need.x &&
    have.y <= need.y &&
    have.x + have.width >= need.x + need.width &&
    have.y + have.height >= need.y + need.height
  );
}

/**
 * 現像のプレビューの情報から、出力の画像（C）のフル解像度の大きさを取る。コアが返さない（古いコア）か、
 * 切り抜く前の全体を描いたもの（`uncropped`。切り抜きの編集中）なら null。
 */
export function outputSizeFromFrame(info: FrameInfo | null | undefined): { width: number; height: number } | null {
  if (!info || info.uncropped) return null;
  const w = info.output_width;
  const h = info.output_height;
  if (!w || !h || w <= 0 || h <= 0) return null;
  return { width: w, height: h };
}

// ---------------------------------------------------------------------------
// 写真送り（PERF-04・05）
// ---------------------------------------------------------------------------

/**
 * 先読みする位置（優先する順）。進む向き（`direction`）に `ahead` 枚、戻る向きに `behind` 枚。検索結果の
 * 範囲（0〜count-1）の外は除く。
 */
export function prefetchIndices(
  index: number,
  count: number,
  direction: 1 | -1,
  ahead = PREFETCH_AHEAD,
  behind = PREFETCH_BEHIND,
): number[] {
  const out: number[] = [];
  const add = (i: number) => {
    if (i >= 0 && i < count && i !== index && !out.includes(i)) out.push(i);
  };
  for (let k = 1; k <= Math.max(ahead, behind); k++) {
    if (k <= ahead) add(index + direction * k);
    if (k <= behind) add(index - direction * k);
  }
  return out;
}

/**
 * L1 の要求（と先読み）を待つ時間（ミリ秒）。前の写真から `sincePrevious` ミリ秒で移った。写真送りの途中
 * （`settleMs` より短い）なら `settleMs`、そうでなければ 0（すぐ要求する。PERF-04）。
 */
export function l1RequestDelay(sincePrevious: number, settleMs = L1_SETTLE_MS): number {
  return sincePrevious < settleMs ? settleMs : 0;
}

/** 読み直しのための印を URI に付ける（0 なら付けない。先読みした URI と同じにするため）。 */
export function withRetryParam(url: string, retry: number): string {
  if (retry <= 0) return url;
  return `${url}${url.includes("?") ? "&" : "?"}retry=${retry}`;
}

/** 新しく使ったものを残す、上限つきの対応表（先読みした画像を持っておく）。 */
export class RecentMap<K, V> {
  private map = new Map<K, V>();
  constructor(readonly capacity: number) {}

  get size(): number {
    return this.map.size;
  }

  get(key: K): V | undefined {
    return this.map.get(key);
  }

  /** あれば新しく使ったことにして true。 */
  touch(key: K): boolean {
    if (!this.map.has(key)) return false;
    const v = this.map.get(key) as V;
    this.map.delete(key);
    this.map.set(key, v);
    return true;
  }

  /** 入れる（上限を超えたら古いものから外し、外したものを返す）。 */
  set(key: K, value: V): V[] {
    this.map.delete(key);
    this.map.set(key, value);
    const evicted: V[] = [];
    while (this.map.size > this.capacity) {
      const oldest = this.map.keys().next().value as K;
      evicted.push(this.map.get(oldest) as V);
      this.map.delete(oldest);
    }
    return evicted;
  }

  delete(key: K): void {
    this.map.delete(key);
  }

  clear(): void {
    this.map.clear();
  }

  keys(): K[] {
    return [...this.map.keys()];
  }
}

// ---------------------------------------------------------------------------
// フィルムストリップ
// ---------------------------------------------------------------------------

/**
 * 選択中の項目が見えるようにするスクロールの位置（仮想。px）。両端の `margin` 個ぶんの内側に見えていれば
 * 動かさない（null）。外れていれば中央に置く（前後の写真も見えるように）。
 */
export function keepVisibleOffset(opts: {
  index: number;
  /** 1 項目の幅（px）。 */
  cell: number;
  /** 今のスクロールの位置（仮想。px）。 */
  offset: number;
  /** 見えている幅（px）。 */
  viewport: number;
  count: number;
  margin?: number;
}): number | null {
  const { index, cell, offset, viewport, count } = opts;
  if (cell <= 0 || viewport <= 0 || index < 0 || index >= count) return null;
  // 狭くて余白を取れないときは、余白なしで判定する。
  const margin = viewport >= cell * 3 ? (opts.margin ?? 1) : 0;
  const start = index * cell;
  const end = start + cell;
  if (start >= offset + margin * cell && end <= offset + viewport - margin * cell) return null;
  const total = count * cell;
  const target = start + cell / 2 - viewport / 2;
  return clamp(target, 0, Math.max(0, total - viewport));
}

/**
 * ホイールの量（物理の px。小数）を、端数を持ち越して整数にする。WebKit のスクロールの位置は整数なので、
 * スクロールの領域を抑えている（50 万件）ときに 1 回の量が 1 px 未満になると、足しても切り捨てで消える。
 * 戻り値の `step` を scrollLeft に足し、`rest` を次の回に渡す。向きが変わったら、前の向きの端数は捨てる。
 */
export function accumulateWheel(rest: number, delta: number): { step: number; rest: number } {
  const carried = rest * delta < 0 ? 0 : rest;
  const total = carried + delta;
  // -0 は 0 にする。
  const step = Math.trunc(total) || 0;
  return { step, rest: total - step };
}

/** スクロールの位置（物理）→ 内容の位置（仮想）。上限に収まるとき（realMax == virtualMax）は同じ値。 */
export function toVirtualOffset(real: number, realMax: number, virtualMax: number): number {
  if (realMax <= 0 || virtualMax <= 0) return 0;
  if (realMax === virtualMax) return clamp(real, 0, virtualMax);
  return clamp((real / realMax) * virtualMax, 0, virtualMax);
}

/** 内容の位置（仮想）→ スクロールの位置（物理）。 */
export function toRealOffset(virtual: number, realMax: number, virtualMax: number): number {
  if (realMax <= 0 || virtualMax <= 0) return 0;
  if (realMax === virtualMax) return clamp(virtual, 0, realMax);
  return clamp((virtual / virtualMax) * realMax, 0, realMax);
}

// ---------------------------------------------------------------------------
// 表示用の書式
// ---------------------------------------------------------------------------

/** フレームレート（小数第 3 位まで。末尾の 0 は省く。59.94 fps・23.976 fps・25 fps）。 */
export function formatFps(fps: number | null | undefined): string {
  if (fps === null || fps === undefined || !Number.isFinite(fps) || fps <= 0) return "—";
  return `${Number(fps.toFixed(3))} fps`;
}

/** コーデック（ffprobe の codec_name）とビット深度。よく使われる名前は規格の名前を添える。 */
export function formatCodec(codec: string | null | undefined, bitDepth: number | null | undefined): string {
  if (!codec) return "—";
  const known: Record<string, string> = { hevc: "H.265 / HEVC", h264: "H.264 / AVC", prores: "ProRes" };
  const name = known[codec.toLowerCase()] ?? codec;
  return bitDepth ? `${name}（${bitDepth} bit）` : name;
}

/** 解像度（3840 × 2160）。 */
export function formatResolution(w: number | null | undefined, h: number | null | undefined): string {
  return w && h ? `${w} × ${h}` : "—";
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}
