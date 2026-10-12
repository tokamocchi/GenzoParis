// 現像の画面の共通の購読（写真の向き・切り抜きの縦横比の計算に使う大きさ）。

import { useEffect, useState, useSyncExternalStore } from "react";

import * as core from "../api/core";
import type { Orientation, VariantId, VariantSummary } from "../api/types";
import { searchCache } from "../state/search";
import { useDevelop } from "../state/develop";
import { type GeometryInfo, geometryInfo } from "./cropMath";

/** variant の詳細から読んだ向き（検索結果のキャッシュにないときに使う）。 */
const orientationCache = new Map<VariantId, Orientation>();
/** 問い合わせ中の詳細（同じ写真を何度も問い合わせない）。 */
const inflight = new Map<VariantId, Promise<void>>();

function loadOrientation(id: VariantId): Promise<void> {
  let p = inflight.get(id);
  if (!p) {
    p = core
      .variantDetails(id)
      .then((d) => {
        orientationCache.set(id, d.summary.orientation);
      })
      .finally(() => inflight.delete(id));
    inflight.set(id, p);
  }
  return p;
}

/**
 * 検索結果のキャッシュに読み込み済みの、写真の概要の一部（`pick` は数・文字などの値を返すこと）。
 * キャッシュのページが届くたびに全体を描き直さないよう、値が変わったときだけ再描画する。
 */
export function useLoadedSummary<T extends string | number | boolean | null>(
  id: VariantId | null,
  pick: (s: VariantSummary) => T,
): T | undefined {
  return useSyncExternalStore(searchCache.subscribe, () => {
    if (id === null) return undefined;
    const s = searchCache.getLoadedById(id);
    return s ? pick(s) : undefined;
  });
}

/**
 * 写真の EXIF の向き（1〜8）。検索結果のキャッシュから読み、なければ詳細を問い合わせる。分からなければ null。
 */
export function useOrientation(id: VariantId | null): Orientation | null {
  const [, setTick] = useState(0);
  const fromSearch = useLoadedSummary(id, (s) => s.orientation);
  useEffect(() => {
    if (id === null || fromSearch !== undefined || orientationCache.has(id)) return;
    let alive = true;
    loadOrientation(id)
      .then(() => {
        if (alive) setTick((n) => n + 1);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [id, fromSearch]);
  if (id === null) return null;
  return fromSearch ?? orientationCache.get(id) ?? null;
}

/** 開いている写真の、切り抜きの縦横比の計算に使う大きさ（今の設定の角度で）。 */
export function useGeometryInfo(): GeometryInfo | null {
  const variantId = useDevelop((s) => s.variantId);
  const source = useDevelop((s) => s.state?.source ?? null);
  const angle = useDevelop((s) => s.settings?.geometry.angle_deg ?? 0);
  const uncroppedW = useDevelop((s) =>
    s.frame?.header.uncropped ? (s.frame.header.output_width ?? s.frame.width) : 0,
  );
  const uncroppedH = useDevelop((s) =>
    s.frame?.header.uncropped ? (s.frame.header.output_height ?? s.frame.height) : 0,
  );
  const orientation = useOrientation(variantId);
  return geometryInfo({
    source: source ? { width: source.width, height: source.height } : null,
    orientation,
    angleDeg: angle,
    uncropped: uncroppedW > 0 && uncroppedH > 0 ? { width: uncroppedW, height: uncroppedH } : null,
  });
}
