// 検索結果（世代と件数。id はコアのメモリにある）の表示範囲の詳細のキャッシュ（04 の 3.7 節）。
//
// - グリッド・フィルムストリップは件数だけで仮想スクロールし、見えている範囲のページを要求する。描いている
//   範囲（表示中の範囲）は setView で知らせる（どちらも表示していないときは、表示中の範囲は「分からない」と
//   して扱う。下の各処理の説明）。
// - ページの数は MAX_PAGES までにする（表示中の範囲の近くのページを残し、ほかは使った順の古いものから
//   捨てる）。range の同時の要求は MAX_INFLIGHT 件まで（残りは待たせ、表示中の範囲のページを先に送る。
//   表示中の範囲の近くから外れた、待つ処理のない初めての読み込み（通り過ぎたページ）は送らずに捨てる）。
// - 新しい世代（search_updated のイベント・検索のし直し）では、表示中の範囲の近くのページを、新しい世代の
//   ページが届くまで表示用に残す（読み込み中の表示に戻してちらつかせない）。ほかのページは捨てる。古い世代の
//   項目は表示（get・peek）だけに使い、選択・操作に使うもの（fetch・items・loaded・findLoaded・
//   getLoadedById）は今の世代の項目だけを返す（同じ位置でも別の写真でありうるため）。
// - 同じ世代の読み直し（invalidate。評価などの変更の後）は、変わった variant を含むページと表示中の範囲の
//   ページだけを読み直す（届くまでは古い値を出す）。影響が広い変更（all）では表示中の範囲のページだけを
//   読み直し、ほかは捨てる（次に見たときに読む）。
// - 古い世代の要求は stale のエラーになる（捨てる）。
// - ページが届いたときの知らせ（subscribe）は、1 フレームに 1 回にまとめる。
// - サムネイルの新しい rev（thumbnail_updated のイベント）は thumbRevs に持つ（ThumbRevs）。

import { useSyncExternalStore } from "react";

import * as core from "../api/core";
import { isStale } from "../api/errors";
import type { VariantId, VariantSummary } from "../api/types";

/** 1 回に要求する件数（genzo-api の MAX_RANGE_LEN は 2,000）。 */
export const PAGE_SIZE = 200;

/**
 * 持っておくページの数の上限（**仮置き**: 64 ページ = 12,800 件）。範囲の選択・全選択の上限（10,000 件 =
 * 50 ページ。state/app.ts の MAX_RANGE_SELECTION）を読んだ後も、選んだ写真の項目（カラーラベルのキーで今の
 * ラベルを調べる）と表示中の範囲の近くが残る数にする。1 ページは JSON で約 125 KiB（1 件 約 600 B）なので、
 * 全体で 8 MiB 程度。
 */
export const MAX_PAGES = 64;

/** 表示中の範囲の前後に、捨てずに残すページの数（**仮置き**。少し戻ったときに読み直さないように）。 */
export const NEAR_PAGES = 2;

/**
 * range の同時の要求数の上限（**仮置き**: 4）。コアは range を 1 件ごとに別のスレッドで処理するが、カタログの
 * 1 つの Mutex を順に取るので、多く並べても速くならず、ルーペの L1 などほかの要求を待たせる。表示中の範囲
 * （ふつう 1〜3 ページ）を 1 回で送れる数にする。
 */
export const MAX_INFLIGHT = 4;

/** 項目を読み込む途中で世代が変わったときに、新しい世代で読み直す回数（items）。 */
const MAX_RETRIES = 3;

/**
 * サムネイルの新しい rev を、まとめて捨てるか調べ始める数（**仮置き**）。超えたら、読み込み済みのページに
 * ない写真の rev を捨て、次はその時点の数の 2 倍で調べる（1 件あたりの手間を一定に保つ）。
 */
const THUMB_PRUNE_MIN = 2048;

/** 次のフレームで呼ぶ（requestAnimationFrame がなければ 16 ms 後。テストの Node など）。 */
function nextFrame(fn: () => void): void {
  if (typeof requestAnimationFrame === "function") requestAnimationFrame(() => fn());
  else setTimeout(fn, 16);
}

/** 同じ世代のままの変更（catalog_changed をまとめたもの。state/app.ts）。 */
export interface CatalogChangeSet {
  /** 変わった variant。 */
  ids: ReadonlySet<VariantId>;
  /** 影響が広い（表示中のものをすべて読み直す）。 */
  all: boolean;
  /**
   * 変更を受け取ったときの要求の通し番号（requestMark）。これより後に送った要求のページは、変更の後の値
   * なので読み直さない。
   */
  since: number;
}

function mergeChange(a: CatalogChangeSet | null, b: CatalogChangeSet): CatalogChangeSet {
  if (!a) return b;
  return { ids: new Set([...a.ids, ...b.ids]), all: a.all || b.all, since: Math.max(a.since, b.since) };
}

/** 読み込んだページ。 */
interface Entry {
  /** 読んだときの世代の印（reset のたびに新しくなる。今の印と違えば古い世代で、表示だけに使う）。 */
  token: object;
  items: VariantSummary[];
  /** 最後に使った順（大きいほど新しい）。 */
  used: number;
  /** 要求を送った通し番号。 */
  sent: number;
}

/** ページの要求（送る前・送った後）。 */
interface Job {
  token: object;
  generation: number;
  page: number;
  /** 届くのを待つ処理がある（ensure・items・fetch。送る前の列から捨てない）。 */
  wanted: boolean;
  /** 送った通し番号（送る前は 0）。 */
  sent: number;
  /** 送ったときのサムネイルの rev の印（ThumbRevs.mark）。 */
  thumbMark: number;
  /** 送った後に受け取った変更（届いたら、変更の後の扱いを決める）。 */
  change: CatalogChangeSet | null;
  /** 作ったときの表示中の範囲の通し番号（それより後に表示中の範囲が変わっていなければ、送る前に捨てない）。 */
  viewSeq: number;
  promise: Promise<VariantSummary[] | null>;
  resolve: (items: VariantSummary[] | null) => void;
}

/**
 * サムネイルの新しい rev（thumbnail_updated のイベント。読み込んだページの thumb_rev より新しいもの）。
 *
 * - 受け取るたびに状態を作り直さず（50 万件の取り込みで件数の 2 乗の手間にしない）、Map に入れる。知らせ
 *   （subscribe）は 1 フレームに 1 回にまとめ、変化の通し番号（version）を増やす。
 * - range の結果が届いたら、その要求を送る前に受け取った rev は捨てる（結果の thumb_rev の方が同じか
 *   新しい）。数が増えたら、読み込み済みのページにない写真の rev も捨てる（その写真のページを次に読むときは、
 *   新しい thumb_rev が入っている）。
 */
export class ThumbRevs {
  /** 変化の通し番号（まとめて知らせるたびに増える）。 */
  version = 0;
  private revs = new Map<VariantId, { rev: string; seq: number }>();
  /** 受け取った数（各 rev の印）。 */
  private received = 0;
  private listeners = new Set<() => void>();
  private scheduled = false;
  private pruneAt = THUMB_PRUNE_MIN;

  /**
   * @param inUse 捨てずに残す写真（読み込み済みのページにある id）と、送った後まだ届いていない要求の印の
   *   最小値（それより後に受け取った rev は、届く結果より新しいので残す）。
   */
  constructor(private inUse: () => { ids: ReadonlySet<VariantId>; since: number }) {}

  /** 持っている rev の数。 */
  get size(): number {
    return this.revs.size;
  }

  get(id: VariantId): string | undefined {
    return this.revs.get(id)?.rev;
  }

  /** 項目のサムネイルの rev（新しい rev を受け取っていればそれ）。 */
  of(item: VariantSummary): string | null {
    return this.revs.get(item.variant_id)?.rev ?? item.thumb_rev;
  }

  /** 新しい rev を受け取った（すぐ引けるようにし、知らせは次のフレームにまとめる）。 */
  set(id: VariantId, rev: string): void {
    this.revs.set(id, { rev, seq: ++this.received });
    if (this.scheduled) return;
    this.scheduled = true;
    nextFrame(() => this.flush());
  }

  /** 今までに受け取った数（range を送るときに覚える）。 */
  mark(): number {
    return this.received;
  }

  /** range の結果 `items` が届いた: 要求を送ったとき（`mark`）までに受け取った rev は捨てる。 */
  settle(items: readonly VariantSummary[], mark: number): void {
    if (this.revs.size === 0) return;
    for (const v of items) {
      const r = this.revs.get(v.variant_id);
      if (r && r.seq <= mark) this.revs.delete(v.variant_id);
    }
  }

  /** すべて捨てる（カタログを開いた・閉じたとき）。 */
  clear(): void {
    this.revs.clear();
    this.pruneAt = THUMB_PRUNE_MIN;
    this.version++;
    this.emit();
  }

  subscribe = (l: () => void) => {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  };

  /** まとめて知らせる（受け取った後の次のフレーム）。 */
  private flush(): void {
    this.scheduled = false;
    if (this.revs.size > this.pruneAt) this.prune();
    this.version++;
    this.emit();
  }

  private emit(): void {
    for (const l of [...this.listeners]) l();
  }

  private prune(): void {
    const { ids, since } = this.inUse();
    for (const [id, r] of this.revs) {
      if (r.seq <= since && !ids.has(id)) this.revs.delete(id);
    }
    this.pruneAt = Math.max(THUMB_PRUNE_MIN, this.revs.size * 2);
  }
}

class SearchCache {
  generation = 0;
  count = 0;
  /** 描画の更新の通し番号（ページが届く・捨てるたびに増える。知らせは 1 フレームにまとめる）。 */
  version = 0;
  /**
   * 要求が stale（古い世代）になったとき（state/app.ts が、待たせている新しい世代の知らせをすぐ使う。
   * 取り込み中は search_updated を間引くため）。
   */
  onStale: (() => void) | null = null;
  /** サムネイルの新しい rev。 */
  readonly thumbRevs = new ThumbRevs(() => this.thumbRevsInUse());

  private token: object = {};
  private pages = new Map<number, Entry>();
  /** 今の世代の要求（ページで引く。送る前・送った後）。 */
  private jobs = new Map<number, Job>();
  /** 送る前の要求。 */
  private queue: Job[] = [];
  /** 送った要求（どの世代も。同時の要求数に数える）。 */
  private running = new Set<Job>();
  private views = new Map<string, { start: number; end: number }>();
  /** 表示中の範囲が変わるたびに増える。 */
  private viewSeq = 0;
  private clock = 0;
  private sentSeq = 0;
  private listeners = new Set<() => void>();
  private notifyScheduled = false;

  subscribe = (l: () => void) => {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  };

  /** 変わったことを知らせる（version はすぐ増やし、知らせは次のフレームにまとめる）。 */
  private changed(): void {
    this.version++;
    if (this.notifyScheduled) return;
    this.notifyScheduled = true;
    nextFrame(() => {
      this.notifyScheduled = false;
      this.emit();
    });
  }

  private emit(): void {
    for (const l of [...this.listeners]) l();
  }

  // --- 表示中の範囲 ---

  /**
   * 表示中の範囲（位置の [start, end)。描いている行・項目）を知らせる。`key` ごとに 1 つ（"grid" など）。
   * 空の範囲（まだ大きさを測っていない・件数が 0）は、表示していないことにする。
   */
  setView(key: string, start: number, end: number): void {
    const prev = this.views.get(key);
    if (end <= start) {
      if (prev) this.clearView(key);
      return;
    }
    if (prev && prev.start === start && prev.end === end) return;
    this.views.set(key, { start, end });
    this.viewSeq++;
  }

  /** 表示をやめた（Grid・Filmstrip を閉じた）。 */
  clearView(key: string): void {
    if (this.views.delete(key)) this.viewSeq++;
  }

  /** ページが表示中の範囲（前後 `near` ページを含む）にかかるか。どこも表示していなければ null（分からない）。 */
  private inView(page: number, near = 0): boolean | null {
    if (this.views.size === 0) return null;
    for (const v of this.views.values()) {
      const first = Math.floor(v.start / PAGE_SIZE) - near;
      const last = Math.floor((v.end - 1) / PAGE_SIZE) + near;
      if (page >= first && page <= last) return true;
    }
    return false;
  }

  // --- 世代 ---

  /**
   * 新しい世代にする。表示中の範囲の近くのページは、新しい世代のページが届くまで表示用に残す（古い世代の
   * 項目は表示だけに使う）。ほかのページと、送る前の要求は捨てる。どこも表示していなければ、すべて捨てる。
   */
  reset(generation: number, count: number): void {
    this.generation = generation;
    this.count = count;
    this.token = {};
    const waiting = this.queue;
    this.queue = [];
    this.jobs.clear();
    for (const [page] of [...this.pages]) {
      if (page * PAGE_SIZE >= count || this.inView(page, NEAR_PAGES) !== true) this.pages.delete(page);
    }
    // 待っている処理（items）は、世代が変わったことを見て読み直す。
    for (const job of waiting) job.resolve(null);
    this.version++;
    this.emit();
  }

  /**
   * 同じ世代のまま、変更の後の値を読み直す（評価などの変更の後）。表示中の範囲のページと、変わった variant を
   * 含むページを読み直す（届くまでは古い値を出す）。`all` なら表示中の範囲のページだけを読み直し、ほかは
   * 捨てる。どこも表示していなければ、変わった variant を含むページだけを読み直す（`all` ならすべて捨てる）。
   */
  invalidate(change: CatalogChangeSet): void {
    let dropped = false;
    for (const [page, e] of [...this.pages]) {
      // 古い世代のページ（表示用）は、新しい世代のページで差し替える。変更の後に送った要求のページはそのまま。
      if (e.token !== this.token || e.sent > change.since) continue;
      const action = this.afterChange(page, e.items, change);
      if (action === "drop") {
        this.pages.delete(page);
        dropped = true;
      } else if (action === "reload") {
        void this.request(page, false);
      }
    }
    // 変更の前に送って、まだ届いていない要求: 届いたら同じように扱う。
    for (const job of this.running) {
      if (job.token === this.token && job.sent <= change.since) job.change = mergeChange(job.change, change);
    }
    if (dropped) this.changed();
  }

  /** 変更の後のページの扱い（読み直す・捨てる・そのまま）。 */
  private afterChange(
    page: number,
    items: readonly VariantSummary[],
    change: CatalogChangeSet,
  ): "reload" | "drop" | "keep" {
    if (this.inView(page) === true) return "reload";
    if (change.all) return "drop";
    return items.some((v) => change.ids.has(v.variant_id)) ? "reload" : "keep";
  }

  /** 今までに送った要求の通し番号（変更を受け取ったときに覚え、invalidate に渡す）。 */
  requestMark(): number {
    return this.sentSeq;
  }

  // --- 表示に使う（古い世代の項目を含む） ---

  /**
   * 位置 `index` の項目（表示用）。今の世代のページがなければ要求し、届くまでは古い世代の項目（あれば）を
   * 返す。選択・操作には fetch・items を使う。
   */
  get(index: number): VariantSummary | undefined {
    if (index < 0 || index >= this.count) return undefined;
    const page = Math.floor(index / PAGE_SIZE);
    const e = this.pages.get(page);
    if (!e || e.token !== this.token) void this.request(page, false);
    if (!e) return undefined;
    e.used = ++this.clock;
    return e.items[index - page * PAGE_SIZE];
  }

  /** 位置 `index` の項目（表示用）を、読み込みを起こさずに返す（読み込み済みでなければ undefined）。 */
  peek(index: number): VariantSummary | undefined {
    if (index < 0 || index >= this.count) return undefined;
    const page = Math.floor(index / PAGE_SIZE);
    const e = this.pages.get(page);
    if (!e) return undefined;
    e.used = ++this.clock;
    return e.items[index - page * PAGE_SIZE];
  }

  // --- 選択・操作に使う（今の世代の項目だけ） ---

  /** 今の世代の読み込み済みの項目を、読み込みを起こさずに順に返す（ページの順は決まっていない）。 */
  *loaded(): Generator<VariantSummary> {
    for (const e of this.pages.values()) {
      if (e.token === this.token) yield* e.items;
    }
  }

  /** 範囲のページを読み込む（届いたら解決）。 */
  async ensure(start: number, end: number): Promise<void> {
    await Promise.all(
      this.pageRange(start, end)
        .filter((p) => this.pages.get(p)?.token !== this.token)
        .map((p) => this.request(p, true)),
    );
  }

  /**
   * 範囲 [start, end) の今の世代の項目を読み込んで返す（選択・操作に使う。読み込み済みのページが上限で
   * 捨てられても、読んだ値を返す）。途中で世代が変わったら、新しい世代で読み直す。読めなかった位置は除く。
   */
  async items(start: number, end: number): Promise<VariantSummary[]> {
    for (let attempt = 0; ; attempt++) {
      const token = this.token;
      const pages = this.pageRange(start, end);
      const loaded = await Promise.all(
        pages.map((p) => {
          const e = this.pages.get(p);
          return e && e.token === token ? e.items : this.request(p, true);
        }),
      );
      if (this.token !== token) {
        if (attempt < MAX_RETRIES) continue;
        return [];
      }
      const out: VariantSummary[] = [];
      pages.forEach((p, k) => {
        const items = loaded[k];
        if (!items) return;
        const base = p * PAGE_SIZE;
        const to = Math.min(end, base + items.length);
        for (let i = Math.max(start, base); i < to; i++) out.push(items[i - base]);
      });
      return out;
    }
  }

  /** 位置 `index` の今の世代の項目を読み込んで返す。 */
  async fetch(index: number): Promise<VariantSummary | undefined> {
    if (index < 0 || index >= this.count) return undefined;
    return (await this.items(index, index + 1))[0];
  }

  /** 今の世代の読み込み済みの項目から id を探す（なければ -1）。 */
  findLoaded(id: VariantId): number {
    for (const [page, e] of this.pages) {
      if (e.token !== this.token) continue;
      const i = e.items.findIndex((v) => v.variant_id === id);
      if (i >= 0) return page * PAGE_SIZE + i;
    }
    return -1;
  }

  /** 今の世代の読み込み済みの項目（id から）。 */
  getLoadedById(id: VariantId): VariantSummary | undefined {
    for (const e of this.pages.values()) {
      if (e.token !== this.token) continue;
      const v = e.items.find((x) => x.variant_id === id);
      if (v) return v;
    }
    return undefined;
  }

  // --- 要求 ---

  /** 範囲 [start, end) にかかるページ（件数の内側）。 */
  private pageRange(start: number, end: number): number[] {
    const first = Math.max(0, Math.floor(start / PAGE_SIZE));
    const last = Math.min(Math.ceil(this.count / PAGE_SIZE) - 1, Math.floor((end - 1) / PAGE_SIZE));
    const out: number[] = [];
    for (let p = first; p <= last; p++) out.push(p);
    return out;
  }

  /**
   * 今の世代のページを要求する（同じページの要求はまとめる）。届いたら項目、読めなければ null で解決する。
   * `wanted` は届くのを待つ処理があること（送る前の列から捨てない）。
   */
  private request(page: number, wanted: boolean): Promise<VariantSummary[] | null> {
    const existing = this.jobs.get(page);
    if (existing) {
      if (wanted) existing.wanted = true;
      return existing.promise;
    }
    let resolve: (items: VariantSummary[] | null) => void = () => undefined;
    const promise = new Promise<VariantSummary[] | null>((r) => {
      resolve = r;
    });
    const job: Job = {
      token: this.token,
      generation: this.generation,
      page,
      wanted,
      sent: 0,
      thumbMark: 0,
      change: null,
      viewSeq: this.viewSeq,
      promise,
      resolve,
    };
    this.jobs.set(page, job);
    this.queue.push(job);
    this.pump();
    return promise;
  }

  /** 同時の要求数の上限まで送る。 */
  private pump(): void {
    while (this.running.size < MAX_INFLIGHT) {
      const job = this.takeNext();
      if (!job) return;
      this.send(job);
    }
  }

  /**
   * 次に送る要求（表示中の範囲のページを先に）。通り過ぎたページの初めての読み込み（作った後に表示中の範囲が
   * 変わり、その近くから外れたもの）は捨てる。描画の中で要求したページは、表示中の範囲を知らせる前に送る
   * ことがあるので、範囲が変わるまでは捨てない。
   */
  private takeNext(): Job | undefined {
    if (this.queue.length === 0) return undefined;
    this.queue = this.queue.filter((job) => {
      const e = this.pages.get(job.page);
      const firstLoad = !e || e.token !== this.token;
      const passed =
        !job.wanted && firstLoad && job.viewSeq < this.viewSeq && this.inView(job.page, NEAR_PAGES) === false;
      if (passed) {
        this.jobs.delete(job.page);
        job.resolve(null);
      }
      return !passed;
    });
    const k = this.queue.findIndex((job) => this.inView(job.page) !== false);
    return this.queue.splice(Math.max(0, k), 1)[0];
  }

  private send(job: Job): void {
    job.sent = ++this.sentSeq;
    job.thumbMark = this.thumbRevs.mark();
    this.running.add(job);
    core.range(job.generation, job.page * PAGE_SIZE, PAGE_SIZE).then(
      (r) => this.done(job, r.items, null),
      (e: unknown) => this.done(job, null, e),
    );
  }

  /** 要求の結果が届いた（`items` か `error`）。 */
  private done(job: Job, items: VariantSummary[] | null, error: unknown): void {
    this.running.delete(job);
    const current = job.token === this.token;
    if (current && this.jobs.get(job.page) === job) this.jobs.delete(job.page);
    if (current && items) {
      this.thumbRevs.settle(items, job.thumbMark);
      // 送った後に変更があった: 値は使い（持っていたものより新しい）、変更の後の扱いに従う。
      const action = job.change ? this.afterChange(job.page, items, job.change) : "keep";
      if (action === "drop") this.pages.delete(job.page);
      else this.pages.set(job.page, { token: job.token, items, used: ++this.clock, sent: job.sent });
      if (action === "reload") void this.request(job.page, false);
      this.evict();
      this.changed();
    } else if (current && error !== null) {
      // 今の世代の要求が stale: コアは次の世代に進んでいる（知らせを待たせていれば、すぐ使う）。
      if (isStale(error)) this.onStale?.();
      else console.error("検索結果を読めません", error);
    }
    job.resolve(current ? items : null);
    this.pump();
  }

  /** ページの数を上限までにする（表示中の範囲の近くは残し、古い世代のもの・使った順の古いものから捨てる）。 */
  private evict(): void {
    if (this.pages.size <= MAX_PAGES) return;
    const candidates = [...this.pages]
      .filter(([page]) => this.inView(page, NEAR_PAGES) !== true)
      .sort(([, a], [, b]) => Number(a.token === this.token) - Number(b.token === this.token) || a.used - b.used);
    for (const [page] of candidates) {
      if (this.pages.size <= MAX_PAGES) break;
      this.pages.delete(page);
    }
  }

  /** サムネイルの新しい rev のうち、残すもの（ThumbRevs の inUse）。 */
  private thumbRevsInUse(): { ids: ReadonlySet<VariantId>; since: number } {
    const ids = new Set<VariantId>();
    for (const e of this.pages.values()) for (const v of e.items) ids.add(v.variant_id);
    let since = Number.POSITIVE_INFINITY;
    for (const job of this.running) if (job.token === this.token) since = Math.min(since, job.thumbMark);
    return { ids, since };
  }

  /** 持っているページの数（古い世代の表示用を含む。テスト・調査用）。 */
  get pageCount(): number {
    return this.pages.size;
  }
}

/** アプリで 1 つの検索結果のキャッシュ。 */
export const searchCache = new SearchCache();

/** サムネイルの新しい rev（thumbnail_updated。searchCache.thumbRevs）。 */
export const thumbRevs = searchCache.thumbRevs;

/** 検索結果のキャッシュの更新で再描画する（戻り値は更新の通し番号）。 */
export function useSearchVersion(): number {
  return useSyncExternalStore(searchCache.subscribe, () => searchCache.version);
}
