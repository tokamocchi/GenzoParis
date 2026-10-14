// 表示範囲の描画（ルーペ・現像の等倍表示 PRV-03、拡大とパン LIB-02。genzo-api の render_region）。
//
// - 現像のセッション（state/develop.ts）で開いた写真に対して描く（開いていなければ開く）。
// - 最新の要求だけを使う（コアも古い要求を cancelled で戻す。利用者には見せない）。
// - 範囲は出力の画像（C。切り抜いた後）のフル解像度の画素の座標（space: "output"）。倍率 1 が等倍。
//   画面の 1 画素に画像の 1 画素を出す（HiDPI で実ピクセル。UX-03）ときは、倍率に devicePixelRatio を
//   掛けずに、canvas の CSS の大きさを width / devicePixelRatio にする。
// - ビフォー / アフターの表示（DEV-31）のビフォーの範囲は renderBeforeRegion（コアの render_before_region。
//   アフターとは別の「最新の 1 件だけ」なので、両方を同時に描ける）。

import * as core from "../api/core";
import { isCancelled } from "../api/errors";
import { type Frame, fetchBeforeRegionFrame, fetchRegionFrame } from "../api/frames";
import type { RegionInfo, RegionRequest } from "../api/types";
import { openDevelopFor } from "./develop";

let seq = 0;

/** 範囲を描く（新しい要求で置き換えられたら null）。 */
export async function renderRegion(req: RegionRequest): Promise<Frame<RegionInfo> | null> {
  const my = ++seq;
  const state = await openDevelopFor(req.variant_id);
  if (!state || my !== seq) return null;
  try {
    const info = await core.renderRegion(req);
    if (my !== seq) return null;
    const frame = await fetchRegionFrame();
    if (my !== seq || !frame || frame.header.request !== info.request) return null;
    return frame;
  } catch (e) {
    if (isCancelled(e)) return null;
    throw e;
  }
}

let beforeSeq = 0;

/** ビフォーの範囲を描く（DEV-31。新しい要求で置き換えられたら null。写真を開いていて、ビフォーを設定していること）。 */
export async function renderBeforeRegion(req: RegionRequest): Promise<Frame<RegionInfo> | null> {
  const my = ++beforeSeq;
  try {
    const info = await core.renderBeforeRegion(req);
    if (my !== beforeSeq) return null;
    const frame = await fetchBeforeRegionFrame();
    if (my !== beforeSeq || !frame || frame.header.request !== info.request) return null;
    return frame;
  } catch (e) {
    if (isCancelled(e)) return null;
    throw e;
  }
}

/** 実行中の描画を取り消す（ルーペを閉じた・全体表示に戻したとき。ビフォーの範囲の描画も）。 */
export function cancelRegionRender(): void {
  seq++;
  beforeSeq++;
  void core.cancelRegion().catch(() => undefined);
}

/** 表示する範囲（C のフル解像度の画素）を、中心と倍率と表示の大きさ（CSS の画素）から求める。 */
export function regionFor(opts: {
  /** 出力の画像のフル解像度の大きさ。 */
  fullWidth: number;
  fullHeight: number;
  /** 表示の中心（C の画素）。 */
  centerX: number;
  centerY: number;
  /** 倍率（1 が等倍。画像の 1 画素が画面の 1 物理画素）。 */
  scale: number;
  /** 表示の大きさ（CSS の画素）。 */
  viewWidth: number;
  viewHeight: number;
  devicePixelRatio: number;
}): { x: number; y: number; width: number; height: number } {
  const w = Math.min(opts.fullWidth, (opts.viewWidth * opts.devicePixelRatio) / opts.scale);
  const h = Math.min(opts.fullHeight, (opts.viewHeight * opts.devicePixelRatio) / opts.scale);
  const x = Math.min(Math.max(0, opts.centerX - w / 2), opts.fullWidth - w);
  const y = Math.min(Math.max(0, opts.centerY - h / 2), opts.fullHeight - h);
  return { x: Math.floor(x), y: Math.floor(y), width: Math.ceil(w), height: Math.ceil(h) };
}
