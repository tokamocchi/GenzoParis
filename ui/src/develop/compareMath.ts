// ビフォー / アフターの表示（DEV-31）の配置の計算（DOM を使わない純粋な計算）。
//
// - 形（viewState の CompareMode）ごとに、画像を出す面（ペイン）の位置と大きさを決める。左右に並べるときは
//   左がビフォー、右がアフター（Lightroom と同じ）。分割は 1 つの面に両方を重ね、左半分にビフォー、右半分に
//   アフターを見せる。
// - ビフォーの切り抜きはアフターと同じ（コアの effective_settings）なので、出力の画像の格子は同じ。拡大・パンの
//   中心はアフターの出力の画素で持ち、ビフォーの面には出力の大きさの比で写す（大きさが同じなら同じ値）。

import type { CompareMode } from "./viewState";

/** 面の対象。 */
export type PaneTarget = "after" | "before";

/** 面（表示の枠の中の位置と大きさ。CSS の画素）。 */
export interface Pane {
  target: PaneTarget;
  left: number;
  top: number;
  width: number;
  height: number;
}

/** 左右に並べるときの面の間の幅（CSS の画素。仮置き）。 */
export const PANE_GAP_PX = 4;

/**
 * 形ごとの面（`usable` が偽ならアフターだけ: 切り抜きの編集中など）。分割は 1 つの面で、ビフォーとアフターを
 * 重ねる（呼び出し側が splitX で切り分ける）。
 */
export function comparePanes(mode: CompareMode, width: number, height: number, usable = true): Pane[] {
  const full = (target: PaneTarget): Pane => ({ target, left: 0, top: 0, width, height });
  if (!usable || mode === "off") return [full("after")];
  if (mode === "before") return [full("before")];
  if (mode === "split") return [full("before"), full("after")];
  const w = Math.max(0, (width - PANE_GAP_PX) / 2);
  return [
    { target: "before", left: 0, top: 0, width: w, height },
    { target: "after", left: width - w, top: 0, width: w, height },
  ];
}

/** 拡大・パンで面の大きさとして使う値（左右に並べるときは半分の幅。どの面も同じ大きさ）。 */
export function paneSize(mode: CompareMode, width: number, height: number, usable = true): { width: number; height: number } {
  const p = comparePanes(mode, width, height, usable)[0];
  return { width: p.width, height: p.height };
}

/** 分割の境目（面の左端からの位置。CSS の画素。面の中央）。 */
export function splitX(paneWidth: number): number {
  return Math.round(paneWidth / 2);
}

/**
 * 拡大の中心（アフターの出力の画素）を、ビフォーの出力の画素にする（出力の大きさの比。大きさが分からなければ
 * そのまま）。
 */
export function mapCenter(
  center: { centerX: number; centerY: number },
  from: { width: number; height: number },
  to: { width: number; height: number },
): { centerX: number; centerY: number } {
  if (!(from.width > 0) || !(from.height > 0) || !(to.width > 0) || !(to.height > 0)) return center;
  if (from.width === to.width && from.height === to.height) return center;
  return { centerX: (center.centerX * to.width) / from.width, centerY: (center.centerY * to.height) / from.height };
}

/** 面の見出し（ビフォーは出どころの名前を添える）。 */
export function paneLabel(target: PaneTarget, beforeLabel: string | null): string {
  if (target === "after") return "アフター";
  return beforeLabel ? `ビフォー（${beforeLabel}）` : "ビフォー";
}

/** 形の名前（ツールバー・知らせ）。 */
export const COMPARE_MODES: { mode: CompareMode; label: string; key: string | null }[] = [
  { mode: "off", label: "アフター", key: null },
  { mode: "before", label: "ビフォー", key: "\\" },
  { mode: "side_by_side", label: "左右", key: "y" },
  { mode: "split", label: "分割", key: "shift+y" },
];
