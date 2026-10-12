// 検索結果のキャッシュ（state/search.ts の searchCache）に読み込み済みの項目を、読み込みを起こさずに順に返す。
//
// SearchCache は、読み込み済みのページをまとめて見る方法を公開していない（id ごとの getLoadedById は、毎回
// 全ページを探す）。そのため、内部のページの表（pages: Map<ページ番号, 項目の配列 | "loading">）を読む。
// 形が違えば何も返さない（呼び出し側は、種類が分からないものとして扱う）。土台に読み込み済みの項目を順に返す
// 方法ができたら置き換える。

import type { VariantSummary } from "../api/types";
import { searchCache } from "../state/search";

type Pages = Map<number, VariantSummary[] | "loading">;

/** 読み込み済みの項目（ページの順は決まっていない）。 */
export function* loadedSummaries(): Generator<VariantSummary> {
  const pages = (searchCache as unknown as { pages?: unknown }).pages;
  if (!(pages instanceof Map)) return;
  for (const items of (pages as Pages).values()) {
    if (!Array.isArray(items)) continue;
    yield* items;
  }
}
