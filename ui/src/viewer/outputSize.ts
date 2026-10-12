// 等倍表示の準備: 出力の画像（C。切り抜いた後）のフル解像度の大きさを求める（PRV-03）。
//
// クリックした点（全体表示の画像の中の割合）を出力の画素の座標にするのに、フル解像度の大きさが要る。
// 次の順に求める。
//
// 1. 現像のセッション（state/develop.ts。現像の画面と同じもの）で写真を開き、最新のプレビューの
//    FrameInfo の output_width・output_height を使う（まだ届いていなければ FRAME_WAIT_MS まで待つ）。
// 2. 待っても届かなければ、render_develop で描いて、その応答の値を使う。
// 3. 値がない（古いコア）・切り抜きの編集中なら、表示範囲の描画を 1 画素だけ要求し、応答の
//    full_width・full_height（genzo-api の RegionInfo。出力の画像全体の寸法）を使う。
// 4. それでも分からなければ、等倍表示は使えない（理由を返す）。
//
// 開いた写真は閉じない（現像の画面に切り替えたときに、そのまま使える）。

import * as core from "../api/core";
import { ApiError } from "../api/errors";
import type { FrameInfo, VariantId } from "../api/types";
import { developStore, openDevelopFor } from "../state/develop";
import { renderRegion } from "../state/region";
import { FRAME_WAIT_MS, outputSizeFromFrame } from "./viewerMath";

export type OutputSizeResult =
  | { kind: "size"; width: number; height: number }
  | { kind: "unavailable"; reason: string }
  /** 新しい要求に置き換えられた・取り消された（何もしない）。 */
  | { kind: "superseded" };

/** 等倍表示を使えないときの理由（コアが大きさを返さない）。 */
export const OUTPUT_SIZE_UNKNOWN =
  "出力の画像の大きさが分からないため、等倍表示を使えません（アプリの版が古い可能性があります）";

function latestFrameInfo(variantId: VariantId): FrameInfo | null {
  const f = developStore.get().frame;
  return f && f.header.variant_id === variantId ? f.header : null;
}

/** 現像のプレビューが届くのを待つ（`ms` を過ぎたら null）。 */
export function waitForFrame(variantId: VariantId, ms: number): Promise<FrameInfo | null> {
  const now = latestFrameInfo(variantId);
  if (now) return Promise.resolve(now);
  return new Promise((resolve) => {
    let done = false;
    const finish = (v: FrameInfo | null) => {
      if (done) return;
      done = true;
      unsubscribe();
      clearTimeout(timer);
      resolve(v);
    };
    const unsubscribe = developStore.subscribe(() => {
      const s = developStore.get();
      // 別の写真を開いた・閉じた: 待っても届かない。
      if (s.variantId !== variantId) finish(null);
      else {
        const info = latestFrameInfo(variantId);
        if (info) finish(info);
      }
    });
    const timer = setTimeout(() => finish(null), ms);
  });
}

/**
 * 写真を現像のセッションで開き、出力の画像のフル解像度の大きさを求める。
 *
 * @param isStale 呼び出し側の要求が古くなったか（等倍表示をやめた・写真が変わった）
 */
export async function resolveOutputSize(
  variantId: VariantId,
  isStale: () => boolean,
  frameWaitMs = FRAME_WAIT_MS,
): Promise<OutputSizeResult> {
  const state = await openDevelopFor(variantId);
  if (isStale()) return { kind: "superseded" };
  if (!state) {
    const d = developStore.get();
    if (d.variantId === variantId && d.error) throw new ApiError(d.error);
    return { kind: "superseded" };
  }

  // 1・2: 現像のプレビューの情報。
  let info = await waitForFrame(variantId, frameWaitMs);
  if (isStale()) return { kind: "superseded" };
  if (!info && developStore.get().variantId === variantId) {
    info = await core.renderDevelop();
    if (isStale()) return { kind: "superseded" };
  }
  const size = info && info.variant_id === variantId ? outputSizeFromFrame(info) : null;
  if (size) return { kind: "size", ...size };

  // 3: 表示範囲の描画の応答（出力の画像全体の寸法）。
  const probe = await renderRegion({ variant_id: variantId, space: "output", x: 0, y: 0, width: 1, height: 1, scale: 1 });
  if (isStale() || !probe) return { kind: "superseded" };
  const { full_width: width, full_height: height } = probe.header;
  if (width > 0 && height > 0) return { kind: "size", width, height };
  return { kind: "unavailable", reason: OUTPUT_SIZE_UNKNOWN };
}
