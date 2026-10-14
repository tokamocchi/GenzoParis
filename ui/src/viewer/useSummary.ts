// 写真の概要（比較表示・選別表示の面・枠。LIB-03）: 検索結果のキャッシュ（今の世代の読み込み済みの項目）にあれば
// それ、なければ詳細を読む（variant_details。少し待ってから。続けて写真を替えたときに読みに行かない）。

import { useEffect, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { VariantId, VariantSummary } from "../api/types";
import { useLoadedSummaryObject } from "../develop/hooks";
import { actions, useApp } from "../state/app";

/** 詳細を読みに行くまでの待ち（ミリ秒。仮置き。ルーペの動画の詳細と同じ）。 */
const DETAILS_DELAY_MS = 100;

/** 写真の概要（分からない間は null）。サムネイルの新しい rev は呼び出し側が thumbRevs で引く。 */
export function useVariantSummary(id: VariantId | null): VariantSummary | null {
  const detailsVersion = useApp((s) => s.detailsVersion);
  // 読み込み済みの概要（キャッシュの更新までは同じオブジェクト。develop/hooks.ts）。
  const listed = useLoadedSummaryObject(id);
  const [loaded, setLoaded] = useState<VariantSummary | null>(null);
  const need = id !== null && !listed;
  useEffect(() => {
    if (id === null || !need) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      core
        .variantDetails(id)
        .then((d) => {
          if (!cancelled) setLoaded(d.summary);
        })
        .catch((e) => {
          // 削除された直後（not_found）は知らせない（選択は検索のし直しで移る）。
          if (!cancelled && toApiError(e).kind !== "not_found") actions.reportError(e, "写真の情報を読めません");
        });
    }, DETAILS_DELAY_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [id, need, detailsVersion]);
  return listed ?? (loaded && loaded.variant_id === id ? loaded : null);
}
