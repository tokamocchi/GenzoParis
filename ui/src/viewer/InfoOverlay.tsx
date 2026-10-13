// ルーペ上の撮影情報（LIB-17）: ルーペと現像の画面の写真の上に重ねる（項目と表示の切り替えは infoItems.ts）。
//
// - 概要（VariantSummary）にない項目（露出・タイトルなど）を出すときだけ、詳細（core.variantDetails）を少し
//   待ってから読む（写真送りの間に読みすぎない）。届くまでは「…」。
// - マウスの操作を邪魔しない（pointer-events: none）。

import { useEffect, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { VariantDetails, VariantSummary } from "../api/types";
import { FlagMark, LabelDot, Stars } from "../components/Marks";
import { activeItems, infoLines, needsDetails, useInfo } from "./infoItems";
import "./viewer.css";

/** 詳細を読みに行くまでの待ち（ミリ秒。写真送りの間に読みに行かない。仮置き）。 */
const DETAILS_DELAY_MS = 150;

/** 詳細（`enabled` のときだけ。渡された詳細があればそれを使う）。 */
function useOverlayDetails(
  summary: VariantSummary,
  given: VariantDetails | null | undefined,
  enabled: boolean,
  version: number,
): VariantDetails | null {
  const id = summary.variant_id;
  const [loaded, setLoaded] = useState<VariantDetails | null>(null);
  const usable = given && given.summary.variant_id === id ? given : null;
  useEffect(() => {
    if (!enabled || usable) return;
    let cancelled = false;
    const t = setTimeout(() => {
      core
        .variantDetails(id)
        .then((d) => {
          if (!cancelled) setLoaded(d);
        })
        .catch((e: unknown) => {
          // 表示の補足なので、読めなくても知らせない（削除された直後など）。
          if (!cancelled && toApiError(e).kind !== "not_found") console.warn("撮影情報を読めません", e);
        });
    }, DETAILS_DELAY_MS);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [id, enabled, usable, version]);
  if (usable) return usable;
  return loaded && loaded.summary.variant_id === id ? loaded : null;
}

export function InfoOverlay(props: {
  summary: VariantSummary;
  /** 呼び出し側が読んだ詳細（あれば使う）。 */
  details?: VariantDetails | null;
  index: number | null;
  count: number;
  /** 寸法の上書き（現像の画面では切り抜いた後の出力の寸法）。 */
  dimensions?: { width: number; height: number } | null;
  /** 詳細を読み直す合図（カタログの変更で増える）。 */
  version?: number;
}) {
  const items = useInfo((s) => activeItems(s));
  const details = useOverlayDetails(props.summary, props.details, needsDetails(items), props.version ?? 0);
  if (items.length === 0) return null;
  const lines = infoLines(items, {
    summary: props.summary,
    details,
    index: props.index,
    count: props.count,
    dimensions: props.dimensions,
  });
  if (lines.length === 0) return null;
  const s = props.summary;
  return (
    <div className="loupe-info" aria-label="撮影情報">
      {lines.map((l, i) =>
        l.kind === "marks" ? (
          <div key={`m${i}`} className="loupe-info-marks">
            <Stars rating={s.rating} />
            <FlagMark flag={s.flag} />
            <LabelDot label={s.color_label} />
          </div>
        ) : (
          <div
            key={`${l.item}${i}`}
            className={l.strong ? "loupe-info-name" : l.muted ? "muted" : l.item === "caption" ? "loupe-info-caption" : undefined}
          >
            {l.text}
          </div>
        ),
      )}
    </div>
  );
}
