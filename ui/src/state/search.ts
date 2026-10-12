// 検索結果（世代と件数。id はコアのメモリにある）の表示範囲の詳細のキャッシュ（04 の 3.7 節）。
//
// - グリッド・フィルムストリップは件数だけで仮想スクロールし、見えている範囲のページを要求する。
// - 新しい世代（search_updated のイベント・検索のし直し）では、キャッシュを捨てて要求し直す。
// - 古い世代の要求は stale のエラーになる（捨てる）。

import { useSyncExternalStore } from "react";

import * as core from "../api/core";
import { isStale } from "../api/errors";
import type { VariantId, VariantSummary } from "../api/types";

/** 1 回に要求する件数（genzo-api の MAX_RANGE_LEN は 2,000）。 */
export const PAGE_SIZE = 200;

type Page = VariantSummary[] | "loading";

class SearchCache {
  generation = 0;
  count = 0;
  private pages = new Map<number, Page>();
  private listeners = new Set<() => void>();
  /** 描画の更新の通し番号（ページが届くたびに増える）。 */
  version = 0;

  subscribe = (l: () => void) => {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  };

  private notify() {
    this.version++;
    for (const l of [...this.listeners]) l();
  }

  /** 新しい世代にする（キャッシュを捨てる）。 */
  reset(generation: number, count: number) {
    this.generation = generation;
    this.count = count;
    this.pages.clear();
    this.notify();
  }

  /** 同じ世代のまま、読み直す（評価などの変更の後）。表示中のものは描き直しまで古い値を出す。 */
  invalidate() {
    const old = new Map(this.pages);
    this.pages.clear();
    for (const [page, items] of old) {
      if (items !== "loading") void this.load(page, items);
    }
  }

  /** 位置 `index` の項目（なければページを要求して undefined）。 */
  get(index: number): VariantSummary | undefined {
    if (index < 0 || index >= this.count) return undefined;
    const page = Math.floor(index / PAGE_SIZE);
    const p = this.pages.get(page);
    if (p === undefined) {
      void this.load(page);
      return undefined;
    }
    if (p === "loading") return undefined;
    return p[index - page * PAGE_SIZE];
  }

  /** 位置 `index` の項目を、読み込みを起こさずに返す（読み込み済みでなければ undefined）。 */
  peek(index: number): VariantSummary | undefined {
    if (index < 0 || index >= this.count) return undefined;
    const page = Math.floor(index / PAGE_SIZE);
    const p = this.pages.get(page);
    return p === undefined || p === "loading" ? undefined : p[index - page * PAGE_SIZE];
  }

  /** 読み込み済みの項目を、読み込みを起こさずに順に返す（ページの順は決まっていない）。 */
  *loaded(): Generator<VariantSummary> {
    for (const items of this.pages.values()) {
      if (items !== "loading") yield* items;
    }
  }

  /** 範囲のページを読み込む（届いたら解決）。 */
  async ensure(start: number, end: number): Promise<void> {
    const first = Math.max(0, Math.floor(start / PAGE_SIZE));
    const last = Math.min(Math.ceil(this.count / PAGE_SIZE) - 1, Math.floor((end - 1) / PAGE_SIZE));
    const waits: Promise<void>[] = [];
    for (let p = first; p <= last; p++) {
      const cur = this.pages.get(p);
      if (cur === undefined || cur === "loading") waits.push(this.load(p));
    }
    await Promise.all(waits);
  }

  /** 位置 `index` の項目を読み込んで返す。 */
  async fetch(index: number): Promise<VariantSummary | undefined> {
    await this.ensure(index, index + 1);
    return this.get(index);
  }

  /** 読み込み中の要求（世代とページで引く。前の世代の読み込みの途中で世代が変わっても、新しい世代の
   * ページを要求し直せるように、世代をキーに含める）。 */
  private inflight = new Map<string, Promise<void>>();

  private load(page: number, keepWhileLoading?: VariantSummary[]): Promise<void> {
    const generation = this.generation;
    const key = `${generation}:${page}`;
    const existing = this.inflight.get(key);
    if (existing) return existing;
    if (keepWhileLoading) this.pages.set(page, keepWhileLoading);
    else this.pages.set(page, "loading");
    const p = core
      .range(generation, page * PAGE_SIZE, PAGE_SIZE)
      .then((r) => {
        if (this.generation !== generation) return;
        this.pages.set(page, r.items);
        this.notify();
      })
      .catch((e) => {
        if (this.generation !== generation) return;
        this.pages.delete(page);
        if (!isStale(e)) console.error("検索結果を読めません", e);
      })
      .finally(() => {
        this.inflight.delete(key);
      });
    this.inflight.set(key, p);
    return p;
  }

  /** 読み込み済みの項目から id を探す（なければ -1）。 */
  findLoaded(id: VariantId): number {
    for (const [page, items] of this.pages) {
      if (items === "loading") continue;
      const i = items.findIndex((v) => v.variant_id === id);
      if (i >= 0) return page * PAGE_SIZE + i;
    }
    return -1;
  }

  /** 読み込み済みの項目（id から）。 */
  getLoadedById(id: VariantId): VariantSummary | undefined {
    const i = this.findLoaded(id);
    return i >= 0 ? this.get(i) : undefined;
  }
}

/** アプリで 1 つの検索結果のキャッシュ。 */
export const searchCache = new SearchCache();


/** 検索結果のキャッシュの更新で再描画する（戻り値は更新の通し番号）。 */
export function useSearchVersion(): number {
  return useSyncExternalStore(searchCache.subscribe, () => searchCache.version);
}
