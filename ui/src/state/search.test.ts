import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { type Bridge, installBridge } from "../api/bridge";
import { ApiError } from "../api/errors";
import { MockBridge } from "../api/mock";
import type { VariantSummary } from "../api/types";
import { MAX_INFLIGHT, MAX_PAGES, NEAR_PAGES, PAGE_SIZE, searchCache, thumbRevs } from "./search";

function item(id: number) {
  return { variant_id: id } as never;
}

/** 待っている Promise の続きを進める。 */
async function flush(): Promise<void> {
  for (let i = 0; i < 50; i++) await Promise.resolve();
}

/** 次のフレーム（Node では 16 ms 後）まで待つ。 */
function nextFrame(): Promise<void> {
  return new Promise((r) => setTimeout(r, 20));
}

/** そのテストで使っている、応答を手で返す Bridge（終わったら残りに答え、次のテストに要求を残さない）。 */
let controlled: ControlledBridge | null = null;

afterEach(async () => {
  vi.useRealTimers();
  if (controlled) await drain(controlled);
  controlled = null;
  searchCache.clearView("grid");
  searchCache.clearView("filmstrip");
  searchCache.reset(0, 0);
});

describe("検索結果のキャッシュ", () => {
  let mock: MockBridge;
  const ranges: string[] = [];
  beforeEach(() => {
    ranges.length = 0;
    mock = new MockBridge();
    installBridge(mock);
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as { type: string; generation: number; start: number; len: number };
      ranges.push(`g${c.generation} s${c.start}`);
      return { type: "range", items: [item(c.generation * 100)] };
    });
  });

  it("前の世代のページの読み込み中に世代が変わっても、新しい世代のページを要求して埋める", async () => {
    // 世代 1 の応答だけを遅らせる。
    mock.delay = (c) => ((c.args?.command as { generation: number }).generation === 1 ? 30 : 0);
    searchCache.reset(1, 1);
    expect(searchCache.get(0)).toBeUndefined();
    searchCache.reset(2, 1);
    expect(searchCache.get(0)).toBeUndefined();
    await new Promise((r) => setTimeout(r, 60));
    expect([...ranges].sort()).toEqual(["g1 s0", "g2 s0"]);
    expect(searchCache.peek(0)).toEqual(item(200));
  });

  it("世代が変わった直後の fetch は、新しい世代の項目を返す", async () => {
    mock.delay = (c) => ((c.args?.command as { generation: number }).generation === 3 ? 30 : 0);
    searchCache.reset(3, 1);
    searchCache.get(0);
    searchCache.reset(4, 1);
    expect(await searchCache.fetch(0)).toEqual(item(400));
    // 遅らせた世代 3 の応答を待つ（次のテストに要求を残さない）。
    await new Promise((r) => setTimeout(r, 40));
  });
});

// ---------------------------------------------------------------------------
// 応答を手で返す Bridge（同時の要求数を数える）
// ---------------------------------------------------------------------------

/** 位置 `index` の項目（世代 `generation`）。id は世代ごとに変える（古い世代の項目と見分ける）。 */
function summary(generation: number, index: number, rev = "r0"): VariantSummary {
  return { variant_id: generation * 1_000_000 + index, thumb_rev: rev } as VariantSummary;
}

interface PendingRange {
  generation: number;
  start: number;
  resolve: (v: unknown) => void;
}

class ControlledBridge implements Bridge {
  pending: PendingRange[] = [];
  /** 同時に待っていた要求の数の最大。 */
  maxPending = 0;
  /** 送られた range の数。 */
  sent = 0;
  /** 世代ごとの件数（範囲の外は返さない）。 */
  counts = new Map<number, number>();
  /** 世代ごとのサムネイルの rev（なければ "r0"）。 */
  revs = new Map<number, string>();
  /** この世代より古い要求は stale にする（コアが次の世代に進んだ）。 */
  coreGeneration = 0;
  /** 世代・位置の写真の id（既定は世代ごとに別の id）。 */
  idOf: (generation: number, index: number) => number = (g, i) => summary(g, i).variant_id;

  invoke<T>(_command: string, args?: Record<string, unknown>): Promise<T> {
    const c = args?.command as { type: string; generation: number; start: number; len: number };
    if (c.type !== "range") return Promise.resolve({ type: "ok" } as T);
    this.sent++;
    return new Promise<T>((resolve) => {
      this.pending.push({ generation: c.generation, start: c.start, resolve: resolve as (v: unknown) => void });
      this.maxPending = Math.max(this.maxPending, this.pending.length);
    });
  }

  invokeBinary(): Promise<ArrayBuffer> {
    return Promise.resolve(new ArrayBuffer(0));
  }

  listen(): Promise<() => void> {
    return Promise.resolve(() => undefined);
  }

  /** 待っている要求の先頭から `n` 件に答える。 */
  respond(n = Number.POSITIVE_INFINITY): void {
    const list = this.pending.splice(0, Math.min(n, this.pending.length));
    for (const p of list) {
      if (p.generation < this.coreGeneration) {
        p.resolve(Promise.reject(new ApiError({ kind: "stale", message: "古い世代", user_actionable: false, retryable: true, hint: null })));
        continue;
      }
      const count = this.counts.get(p.generation) ?? 0;
      const items: VariantSummary[] = [];
      for (let i = p.start; i < Math.min(count, p.start + PAGE_SIZE); i++) {
        items.push({
          ...summary(p.generation, i, this.revs.get(p.generation) ?? "r0"),
          variant_id: this.idOf(p.generation, i),
        });
      }
      p.resolve({ type: "range", generation: p.generation, start: p.start, items });
    }
  }

  /** 待っている要求の先頭（start）。 */
  starts(): number[] {
    return this.pending.map((p) => p.start);
  }
}

/** 待っている要求がなくなるまで答える（答えた後に送られた要求にも答える）。 */
async function drain(b: ControlledBridge): Promise<void> {
  for (let i = 0; i < 1000; i++) {
    await flush();
    if (b.pending.length === 0) return;
    b.respond();
  }
}

describe("ページの上限と同時の要求数（performance-2）", () => {
  let b: ControlledBridge;
  beforeEach(() => {
    b = new ControlledBridge();
    controlled = b;
    installBridge(b);
  });

  it("多くのページを見ても、持つページは上限まで、同時の要求は上限までにする", async () => {
    const count = 550_000;
    b.counts.set(1, count);
    searchCache.reset(1, count);
    // 1 ページずつ表示の位置を進めながら、300 ページを見る（ゆっくりのスクロール）。
    for (let p = 0; p < 300; p++) {
      searchCache.setView("grid", p * PAGE_SIZE, p * PAGE_SIZE + PAGE_SIZE);
      searchCache.get(p * PAGE_SIZE);
      expect(b.pending.length).toBeLessThanOrEqual(MAX_INFLIGHT);
      await drain(b);
    }
    expect(searchCache.pageCount).toBeLessThanOrEqual(MAX_PAGES);
    expect(b.maxPending).toBeLessThanOrEqual(MAX_INFLIGHT);
    // 今見ているページは残っている。
    expect(searchCache.peek(299 * PAGE_SIZE)).toEqual(summary(1, 299 * PAGE_SIZE));
  });

  it("1 万件の読み込み（全選択）も、同時の要求は上限までで、読んだ値をすべて返す", async () => {
    b.counts.set(1, 20_000);
    searchCache.reset(1, 20_000);
    const done = searchCache.items(0, 10_000);
    await flush();
    expect(b.pending.length).toBe(MAX_INFLIGHT);
    await drain(b);
    const items = await done;
    expect(items).toHaveLength(10_000);
    expect(items[9_999]).toEqual(summary(1, 9_999));
    expect(b.maxPending).toBe(MAX_INFLIGHT);
    expect(b.sent).toBe(10_000 / PAGE_SIZE);
  });

  it("表示中の範囲のページを先に送り、通り過ぎたページは送らずに捨てる", async () => {
    b.counts.set(1, 100_000);
    searchCache.reset(1, 100_000);
    searchCache.setView("grid", 0, PAGE_SIZE);
    // 4 件を送っている間に、表示が 40 ページ先へ動いた（途中の 10 ページを要求していた）。
    for (let p = 0; p < 14; p++) searchCache.get(p * PAGE_SIZE);
    expect(b.pending.length).toBe(MAX_INFLIGHT);
    searchCache.setView("grid", 40 * PAGE_SIZE, 41 * PAGE_SIZE);
    searchCache.get(40 * PAGE_SIZE);
    await drain(b);
    // 送ったのは、初めの 4 件と、表示中の範囲（の近く）のページだけ。
    expect(b.sent).toBe(MAX_INFLIGHT + 1);
    expect(searchCache.peek(40 * PAGE_SIZE)).toEqual(summary(1, 40 * PAGE_SIZE));
  });

  it("invalidate は、変わった variant を含むページと表示中の範囲のページだけを読み直す", async () => {
    b.counts.set(1, 10_000);
    searchCache.reset(1, 10_000);
    const loading = searchCache.ensure(0, 10 * PAGE_SIZE);
    await drain(b);
    await loading;
    expect(searchCache.pageCount).toBe(10);
    searchCache.setView("grid", 0, 300);
    const sentBefore = b.sent;
    // ページ 5 の写真の評価が変わった。
    searchCache.invalidate({ ids: new Set([summary(1, 5 * PAGE_SIZE + 3).variant_id]), all: false, since: searchCache.requestMark() });
    await flush();
    expect(b.starts().sort((x, y) => x - y)).toEqual([0, PAGE_SIZE, 5 * PAGE_SIZE]);
    // 読み直す間も、前の値を出す。
    expect(searchCache.peek(5 * PAGE_SIZE + 3)).toEqual(summary(1, 5 * PAGE_SIZE + 3));
    await drain(b);
    expect(b.sent - sentBefore).toBe(3);
    expect(searchCache.pageCount).toBe(10);
  });

  it("影響が広い変更（all）では、表示中の範囲のページだけを読み直し、ほかは捨てる", async () => {
    b.counts.set(1, 10_000);
    searchCache.reset(1, 10_000);
    const loading = searchCache.ensure(0, 10 * PAGE_SIZE);
    await drain(b);
    await loading;
    searchCache.setView("grid", 0, 300);
    searchCache.invalidate({ ids: new Set(), all: true, since: searchCache.requestMark() });
    await flush();
    expect(b.starts().sort((x, y) => x - y)).toEqual([0, PAGE_SIZE]);
    await drain(b);
    expect(searchCache.pageCount).toBe(2);
    expect(searchCache.peek(5 * PAGE_SIZE)).toBeUndefined();
  });

  it("変更の後に送った要求のページは読み直さない", async () => {
    b.counts.set(1, 1_000);
    searchCache.reset(1, 1_000);
    searchCache.setView("grid", 0, 100);
    const since = searchCache.requestMark();
    searchCache.get(0);
    await drain(b);
    const sentBefore = b.sent;
    searchCache.invalidate({ ids: new Set([summary(1, 0).variant_id]), all: false, since });
    await drain(b);
    expect(b.sent).toBe(sentBefore);
  });

  it("ページが届いたときの知らせは、1 フレームに 1 回にまとめる", async () => {
    b.counts.set(1, 10_000);
    searchCache.reset(1, 10_000);
    let notified = 0;
    const off = searchCache.subscribe(() => notified++);
    try {
      const loading = searchCache.ensure(0, 8 * PAGE_SIZE);
      await drain(b);
      await loading;
      expect(notified).toBe(0);
      await nextFrame();
      expect(notified).toBe(1);
    } finally {
      off();
    }
  });
});

describe("新しい世代のときの表示（performance-4）", () => {
  let b: ControlledBridge;
  beforeEach(() => {
    b = new ControlledBridge();
    controlled = b;
    installBridge(b);
  });

  it("表示中の範囲のページは、新しい世代のページが届くまで古い世代の項目を出す（操作には使わない）", async () => {
    b.counts.set(1, 2_000);
    b.counts.set(2, 2_100);
    searchCache.reset(1, 2_000);
    searchCache.setView("grid", 0, 150);
    searchCache.get(0);
    // 表示から遠いページも読んでおく。
    const far = searchCache.ensure(8 * PAGE_SIZE, 8 * PAGE_SIZE + 1);
    await drain(b);
    await far;
    const old = summary(1, 10);

    searchCache.reset(2, 2_100);
    // 表示中の範囲は古い世代の項目を出し続け、新しい世代のページを要求する。
    expect(searchCache.get(10)).toEqual(old);
    expect(searchCache.peek(10)).toEqual(old);
    expect(b.starts()).toEqual([0]);
    // 表示から遠いページは捨てた。
    expect(searchCache.peek(8 * PAGE_SIZE)).toBeUndefined();
    // 選択・操作に使うものは、今の世代の項目だけ。
    expect(searchCache.getLoadedById(old.variant_id)).toBeUndefined();
    expect(searchCache.findLoaded(old.variant_id)).toBe(-1);
    expect([...searchCache.loaded()]).toEqual([]);
    const fetched = searchCache.fetch(10);
    await drain(b);
    expect(await fetched).toEqual(summary(2, 10));
    // 届いたら差し替える。
    expect(searchCache.get(10)).toEqual(summary(2, 10));
  });

  it("表示していなければ、新しい世代では前のページをすべて捨てる", async () => {
    b.counts.set(1, 500);
    searchCache.reset(1, 500);
    const loading = searchCache.ensure(0, 500);
    await drain(b);
    await loading;
    searchCache.reset(2, 500);
    expect(searchCache.pageCount).toBe(0);
  });

  it("読み込みの途中で世代が変わったら、新しい世代で読み直す（古い世代の項目を返さない）", async () => {
    b.counts.set(1, 1_000);
    b.counts.set(2, 1_000);
    searchCache.reset(1, 1_000);
    const loading = searchCache.items(0, 400);
    await flush();
    b.coreGeneration = 2;
    searchCache.reset(2, 1_000);
    await drain(b);
    const items = await loading;
    expect(items).toHaveLength(400);
    expect(items[0]).toEqual(summary(2, 0));
  });

  it("今の世代の要求が stale になったら知らせる（待たせている新しい世代を使うため）", async () => {
    b.counts.set(1, 1_000);
    searchCache.reset(1, 1_000);
    const onStale = vi.fn();
    searchCache.onStale = onStale;
    try {
      b.coreGeneration = 2;
      searchCache.get(0);
      await drain(b);
      expect(onStale).toHaveBeenCalledTimes(1);
    } finally {
      searchCache.onStale = null;
    }
  });

  it("ページの上限は、範囲の選択・全選択の上限（1 万件）と表示中の範囲の近くを持てる数", () => {
    expect(MAX_PAGES).toBeGreaterThanOrEqual(10_000 / PAGE_SIZE + 2 * NEAR_PAGES + 1);
  });
});

describe("古い世代と新しい世代のページが並ぶとき（M2）", () => {
  let b: ControlledBridge;
  beforeEach(() => {
    b = new ControlledBridge();
    controlled = b;
    installBridge(b);
  });

  it("同じ写真を 2 つのセルに出さない（古い世代の側は、今の世代のページにある写真を出さない）", async () => {
    // 世代 2 では先頭に 1 枚増え、位置が 1 つずつずれる。
    b.idOf = (g, i) => (g === 1 ? 1_000 + i : i === 0 ? 999 : 1_000 + i - 1);
    b.counts.set(1, 2_000);
    b.counts.set(2, 2_001);
    searchCache.reset(1, 2_000);
    // 7 列の行（位置 1196〜1202）がページ 5（〜1199）とページ 6（1200〜）にまたがる。
    searchCache.setView("grid", 1_190, 1_210);
    searchCache.get(1_196);
    searchCache.get(1_200);
    await drain(b);
    searchCache.reset(2, 2_001);
    for (let i = 1_196; i <= 1_202; i++) searchCache.get(i);
    expect(b.starts().sort((x, y) => x - y)).toEqual([1_000, 1_200]);
    // 新しい世代のページ 6 だけが先に届いた。
    b.pending.sort((x, y) => y.start - x.start);
    b.respond(1);
    await flush();
    const row: (number | undefined)[] = [];
    for (let i = 1_196; i <= 1_202; i++) row.push(searchCache.get(i)?.variant_id);
    const shown = row.filter((v) => v !== undefined);
    expect(new Set(shown).size).toBe(shown.length);
    // 古い世代の 1196〜1198 はそのまま出し（ちらつかせない）、今の世代のページにある 2199 は新しい位置にだけ出す。
    expect(row).toEqual([2_196, 2_197, 2_198, undefined, 2_199, 2_200, 2_201]);
    await drain(b);
    for (let i = 1_196; i <= 1_202; i++) expect(searchCache.get(i)?.variant_id).toBe(1_000 + i - 1);
  });

  it("表示用に残す古い世代は直前の 1 つだけ（2 つ前の世代のページは捨てる）", async () => {
    b.counts.set(1, 1_000);
    b.counts.set(2, 1_000);
    b.counts.set(3, 1_000);
    searchCache.reset(1, 1_000);
    searchCache.setView("grid", 0, 100);
    searchCache.get(0);
    await drain(b);
    // 世代 2 のページが届く前に、世代 3 になった。
    searchCache.reset(2, 1_000);
    searchCache.reset(3, 1_000);
    expect(searchCache.peek(0)).toBeUndefined();
  });
});

describe("サムネイルの新しい rev（performance-1）", () => {
  let b: ControlledBridge;
  beforeEach(() => {
    b = new ControlledBridge();
    controlled = b;
    installBridge(b);
    thumbRevs.clear();
  });

  it("5 万件を受け取っても、知らせは 1 フレームに 1 回で、読み込み済みのページにない写真の rev は残さない", async () => {
    b.counts.set(1, 100_000);
    searchCache.reset(1, 100_000);
    searchCache.setView("grid", 0, 100);
    searchCache.get(0);
    await drain(b);
    let notified = 0;
    const off = thumbRevs.subscribe(() => notified++);
    try {
      const version = thumbRevs.version;
      for (let i = 0; i < 50_000; i++) thumbRevs.set(summary(1, i).variant_id, `n${i}`);
      // 受け取った直後から引ける（知らせはまだ）。
      expect(notified).toBe(0);
      expect(thumbRevs.get(summary(1, 30_000).variant_id)).toBe("n30000");
      await nextFrame();
      expect(notified).toBe(1);
      expect(thumbRevs.version).toBe(version + 1);
      // 読み込み済みのページ（0〜199）の分だけ残す（ほかは、次に読むページに新しい rev が入る）。
      expect(thumbRevs.size).toBe(PAGE_SIZE);
      expect(thumbRevs.of(summary(1, 5))).toBe("n5");
      expect(thumbRevs.get(summary(1, 30_000).variant_id)).toBeUndefined();
    } finally {
      off();
    }
  });

  it("range の結果が届いたら、要求の前に受け取った rev は捨て、後に受け取った rev は残す", async () => {
    b.counts.set(1, 1_000);
    b.revs.set(1, "new");
    searchCache.reset(1, 1_000);
    const before = summary(1, 1).variant_id;
    const after = summary(1, 2).variant_id;
    thumbRevs.set(before, "new");
    searchCache.get(0);
    await flush();
    thumbRevs.set(after, "newer");
    await drain(b);
    expect(thumbRevs.get(before)).toBeUndefined();
    expect(thumbRevs.of(searchCache.peek(1) as VariantSummary)).toBe("new");
    expect(thumbRevs.get(after)).toBe("newer");
    expect(thumbRevs.of(searchCache.peek(2) as VariantSummary)).toBe("newer");
  });
});
