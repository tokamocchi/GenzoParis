import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { dispatchCoreEventForTest } from "../api/events";
import type { CatalogStatus } from "../api/types";
import { appStore } from "../state/app";
import { l1Status, prefetchL1, prefetchState } from "./prefetch";

/** 読めるかどうかを URI ごとに決める、Image の代わり（vitest の環境は node なので DOM がない）。 */
const requested: string[] = [];
const failing = new Set<string>();

class FakeImage {
  decoding = "auto";
  private url = "";
  set src(v: string) {
    this.url = v;
    requested.push(v);
  }
  get src(): string {
    return this.url;
  }
  decode(): Promise<void> {
    return failing.has(this.url) ? Promise.reject(new Error("404")) : Promise.resolve();
  }
}

const original = (globalThis as { Image?: unknown }).Image;
beforeAll(() => {
  (globalThis as { Image?: unknown }).Image = FakeImage;
});
afterAll(() => {
  (globalThis as { Image?: unknown }).Image = original;
});

const url = (id: number, rev: string) => `genzo://localhost/preview/${id}?rev=${rev}`;

/** 先読みの結果を待つ。 */
async function settle(u: string): Promise<void> {
  await prefetchState(u)?.done;
}

describe("prefetchL1（L1 の先読みと作成待ち）", () => {
  it("読めた画像は持っておき、同じ URI を要求し直さない", async () => {
    const u = url(1, "a");
    prefetchL1(1, "a", u);
    await settle(u);
    expect(prefetchState(u)?.ready).toBe(true);
    prefetchL1(1, "a", u);
    expect(requested.filter((r) => r === u)).toHaveLength(1);
  });

  it("404 になった写真は、作り終えた知らせまで要求しない", async () => {
    const u = url(7, "r1");
    failing.add(u);
    prefetchL1(7, "r1", u);
    await settle(u);
    expect(prefetchState(u)).toBeNull();
    expect(l1Status.state(7, "r1", Date.now())).toBe("waiting");
    // 写真の移動・検索結果の読み直しで、もう一度呼ばれても要求しない。
    prefetchL1(7, "r1", u);
    prefetchL1(7, "r1", u);
    expect(requested.filter((r) => r === u)).toHaveLength(1);

    // 作り終えた（rev が変わった）: 新しい URI を要求する。
    dispatchCoreEventForTest({ type: "thumbnail_updated", variant_id: 7, rev: "r2" });
    expect(l1Status.state(7, "r1", Date.now())).toBe("none");
    const u2 = url(7, "r2");
    prefetchL1(7, "r2", u2);
    await settle(u2);
    expect(prefetchState(u2)?.ready).toBe(true);
    expect(requested.filter((r) => r === u2)).toHaveLength(1);
  });

  it("作れなかった写真（preview_failed）は要求しない", () => {
    dispatchCoreEventForTest({
      type: "warning",
      code: "preview_failed",
      message: "プレビューを作れませんでした",
      variant_id: 9,
      path: null,
    });
    const u = url(9, "a");
    prefetchL1(9, "a", u);
    expect(requested).not.toContain(u);
  });

  it("別のカタログを開いたら忘れる", async () => {
    const u = url(11, "a");
    failing.add(u);
    prefetchL1(11, "a", u);
    await settle(u);
    expect(l1Status.state(11, "a", Date.now())).toBe("waiting");
    appStore.set({ catalog: { catalog_path: "/other/catalog.genzo" } as CatalogStatus });
    expect(l1Status.state(11, "a", Date.now())).toBe("none");
    appStore.set({ catalog: null });
  });
});
