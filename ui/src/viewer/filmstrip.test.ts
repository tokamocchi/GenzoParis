// フィルムストリップの項目（速いスクロールの間はページ・サムネイルを要求しない。決定事項ログ No.66）の
// テスト（DOM を使わず、react-dom/server で文字列にする）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MockBridge } from "../api/mock";
import type { VariantSummary } from "../api/types";
import { searchCache } from "../state/search";
import { FilmstripCell, filmstripClickOptions, filmstripItem } from "./Filmstrip";

const summary = (i: number): VariantSummary =>
  ({
    variant_id: 100 + i,
    kind: "photo",
    file_name: `${i}.ARW`,
    rating: 0,
    flag: "none",
    color_label: null,
    file_status: "ok",
    thumb_rev: "r1",
    capture: null,
  }) as unknown as VariantSummary;

let mock: MockBridge;

beforeEach(() => {
  mock = new MockBridge();
  installBridge(mock);
  mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as { type: string; generation: number; start: number; len: number };
    const items: VariantSummary[] = [];
    for (let i = c.start; i < Math.min(1_000, c.start + c.len); i++) items.push(summary(i));
    return { type: "range", generation: c.generation, start: c.start, items };
  });
});

afterEach(() => {
  searchCache.reset(0, 0);
});

function ranges(): number {
  return mock.coreCommands().filter((t) => t === "range").length;
}

describe("フィルムストリップの項目（performance-5）", () => {
  it("速いスクロールの間は、読み込み済みの項目だけを出し、ページを要求しない", async () => {
    searchCache.reset(1, 1_000);
    expect(filmstripItem(450, true)).toBeUndefined();
    expect(ranges()).toBe(0);
    // 遅くなったら（止まったら）見えている範囲を要求する。
    expect(filmstripItem(450, false)).toBeUndefined();
    expect(ranges()).toBe(1);
    await searchCache.ensure(450, 451);
    // 読み込み済みなら、速い間も出す。
    expect(filmstripItem(450, true)?.variant_id).toBe(550);
    expect(ranges()).toBe(1);
  });

  it("速いスクロールの間は、新しくサムネイルを要求しない（memo の項目はその項目の値だけを受け取る）", () => {
    const props = {
      index: 3,
      item: summary(3),
      rev: "r2",
      left: 0,
      size: 85,
      current: false,
      selected: false,
    };
    const paused = renderToString(createElement(FilmstripCell, { ...props, paused: true }));
    expect(paused).not.toContain("<img");
    const shown = renderToString(createElement(FilmstripCell, { ...props, paused: false }));
    expect(shown).toContain("<img");
    expect(shown).toContain("r2");
  });
});

describe("項目のクリック（M1）", () => {
  it("Shift + クリックでも、出していた写真を確かめて選ぶ（表示中の写真は移さない）", () => {
    expect(filmstripClickOptions({ shiftKey: true, ctrlKey: false, metaKey: false }, 110)).toEqual({
      extend: true,
      toggle: false,
      keepCurrent: true,
      expect: 110,
    });
    expect(filmstripClickOptions({ shiftKey: false, ctrlKey: false, metaKey: true }, 110)).toMatchObject({
      toggle: true,
      expect: 110,
    });
  });
});
