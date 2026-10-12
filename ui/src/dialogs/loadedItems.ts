// 検索結果のキャッシュ（state/search.ts の searchCache）に読み込み済みの項目を、読み込みを起こさずに順に返す。
//
// （searchCache.loaded の薄い包み。）

import type { VariantSummary } from "../api/types";
import { searchCache } from "../state/search";

/** 読み込み済みの項目（ページの順は決まっていない）。 */
export function* loadedSummaries(): Generator<VariantSummary> {
  yield* searchCache.loaded();
}
