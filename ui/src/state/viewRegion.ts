// 比較表示（LIB-03）の面ごとの表示範囲の描画（genzo-api の view.rs の render_view_region）。
//
// - 現像のセッションを開かない（任意の写真を描ける。現像中の写真なら、コアがセッションの今の設定で描く）。
// - 面（pane。0 が選択、1 が候補）ごとに最新の要求だけを使う（コアも同じ面の古い要求を cancelled で戻す。利用者
//   には見せない）。別の面の要求は取り消さない。
// - 範囲は出力の画像（C。切り抜いた後）のフル解像度の画素の座標（space: "output"）。倍率 1 が等倍（state/region.ts と同じ）。
// - 比較表示をやめたら releaseView（コアが展開した写真と結果を捨てる。メモリを返す）。

import * as core from "../api/core";
import { isCancelled } from "../api/errors";
import { type Frame, fetchViewRegionFrame } from "../api/frames";
import type { RegionInfo, RegionRequest, VariantId, ViewOutputSize } from "../api/types";

/** 比較表示の面の数（genzo-api の MAX_VIEW_PANES と同じ値）。 */
export const MAX_VIEW_PANES = 2;

const seqs = Array.from({ length: MAX_VIEW_PANES }, () => 0);

/** 面 `pane` に範囲を描く（同じ面の新しい要求で置き換えられた・取り消されたら null）。 */
export async function renderViewRegion(pane: number, req: RegionRequest): Promise<Frame<RegionInfo> | null> {
  const my = ++seqs[pane];
  try {
    const info = await core.renderViewRegion(pane, req);
    if (my !== seqs[pane]) return null;
    const frame = await fetchViewRegionFrame(pane);
    if (my !== seqs[pane] || !frame || frame.header.request !== info.request || frame.header.pane !== pane) return null;
    return frame;
  } catch (e) {
    if (isCancelled(e)) return null;
    throw e;
  }
}

/** 面 `pane` で使う写真の、出力の画像のフル解像度の大きさ（取り消されたら null）。 */
export async function viewOutputSize(pane: number, id: VariantId): Promise<ViewOutputSize | null> {
  try {
    return await core.viewOutputSize(pane, id);
  } catch (e) {
    if (isCancelled(e)) return null;
    throw e;
  }
}

/** 実行中の描画を取り消す（全体表示に戻したとき）。 */
export function cancelViewRegions(): void {
  for (let i = 0; i < seqs.length; i++) seqs[i]++;
  void core.cancelViewRegions().catch(() => undefined);
}

/** 比較表示をやめる（描画を取り消し、コアが展開した写真と結果を捨てる）。 */
export function releaseView(): void {
  for (let i = 0; i < seqs.length; i++) seqs[i]++;
  void core.releaseView().catch(() => undefined);
}
