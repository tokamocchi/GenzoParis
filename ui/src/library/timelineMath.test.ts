// タイムライン（ORG-01）の行の表（gridMath の withTimeline）と、その上の位置・移動の計算。

import { describe, expect, it } from "vitest";

import { MOCK_TIMELINE, mockTimelineGroup } from "../api/mock";
import type { TimelineGroup } from "../api/types";
import {
  anchoredOffset,
  DAY_HEADER_HEIGHT,
  EVENT_HEADER_HEIGHT,
  firstVisibleIndex,
  GRID_GAP,
  gridLayout,
  isFullyVisible,
  MAX_SCROLL_PX,
  navigateLayout,
  revealOffset,
  ROW_CELLS,
  ROW_DAY,
  ROW_EVENT,
  rowAt,
  rowCells,
  rowOfIndex,
  rowSize,
  rowTop,
  withTimeline,
} from "./gridMath";

// 幅 900・目安 160: 5 列、行の間隔 200（セル 172 + 帯 22 + 間 6）。
const base = () => gridLayout(900, 10, 160);

describe("タイムラインの行の表", () => {
  it("日付・イベントの見出しとセルの行を並べる", () => {
    const l = withTimeline(base(), MOCK_TIMELINE.groups, 10);
    const tl = l.timeline!;
    const h = base().rowHeight;
    // 5/1（見出し）・朝のイベント（見出し・1 行）・夕方のイベント（見出し・1 行）・5/2（見出し・1 行）・撮影日時なし（見出し・1 行）。
    expect(Array.from(tl.kind)).toEqual([
      ROW_DAY,
      ROW_EVENT,
      ROW_CELLS,
      ROW_EVENT,
      ROW_CELLS,
      ROW_DAY,
      ROW_CELLS,
      ROW_DAY,
      ROW_CELLS,
    ]);
    expect(l.rows).toBe(9);
    expect(rowTop(l, 0)).toBe(GRID_GAP);
    expect(rowSize(l, 0)).toBe(DAY_HEADER_HEIGHT);
    expect(rowSize(l, 1)).toBe(EVENT_HEADER_HEIGHT);
    expect(rowSize(l, 2)).toBe(h);
    expect(rowTop(l, 2)).toBe(GRID_GAP + DAY_HEADER_HEIGHT + EVENT_HEADER_HEIGHT);
    expect(l.contentHeight).toBe(GRID_GAP + 3 * DAY_HEADER_HEIGHT + 2 * EVENT_HEADER_HEIGHT + 4 * h);
    expect(l.scrollHeight).toBe(l.contentHeight);
    // 同じ日にまとまりが 1 つなら、イベントの見出しは出さない（5/2）。撮影日時なしにも出さない。
    expect(Array.from(tl.eventHeader)).toEqual([1, 1, 0, 0]);
    expect(Array.from(tl.dayStart)).toEqual([1, 0, 1, 1]);
    expect(Array.from(tl.dayGroup)).toEqual([0, 0, 2, 3]);
    // セルの範囲。見出しの行は null。
    expect(rowCells(l, 2, 10)).toEqual({ start: 0, end: 4 });
    expect(rowCells(l, 4, 10)).toEqual({ start: 4, end: 7 });
    expect(rowCells(l, 8, 10)).toEqual({ start: 9, end: 10 });
    expect(rowCells(l, 0, 10)).toBeNull();
    // 位置 → 行、内容の位置 → 行。
    expect([0, 3, 4, 6, 7, 9].map((i) => rowOfIndex(l, i))).toEqual([2, 2, 4, 4, 6, 8]);
    expect(rowAt(l, 0)).toBe(0);
    expect(rowAt(l, rowTop(l, 4) + 1)).toBe(4);
    expect(rowAt(l, l.contentHeight + 100)).toBe(8);
  });

  it("列の数で行が分かれ、まとまりの先頭で行を改める", () => {
    // 2 列: 4 枚は 2 行、3 枚は 2 行（最後は 1 枚）。
    const l = withTimeline(gridLayout(400, 10, 160), MOCK_TIMELINE.groups, 10);
    expect(l.cols).toBe(2);
    expect(rowCells(l, 3, 10)).toEqual({ start: 2, end: 4 });
    expect(rowCells(l, 6, 10)).toEqual({ start: 6, end: 7 });
    expect(rowOfIndex(l, 6)).toBe(6);
  });

  it("件数と合わないまとまり（古い世代）を整える", () => {
    // 件数が減った: 足りる分だけ並べる。
    const fewer = withTimeline(base(), MOCK_TIMELINE.groups, 5).timeline!;
    expect(fewer.groups.map((g) => [g.start, g.count])).toEqual([
      [0, 4],
      [4, 1],
    ]);
    // 件数が増えた: 残りは見出しのないまとまり（日付なし）にする。
    const more = withTimeline(base(), MOCK_TIMELINE.groups, 13).timeline!;
    expect(more.groups.map((g) => [g.start, g.count, g.day])).toEqual([
      [0, 4, "2024-05-01"],
      [4, 3, "2024-05-01"],
      [7, 2, "2024-05-02"],
      [9, 1, null],
      [10, 3, null],
    ]);
    // 撮影日時なしの見出しは、続くまとまりにはもう出さない。
    expect(Array.from(more.dayStart)).toEqual([1, 0, 1, 1, 0]);
    expect(withTimeline(base(), [], 0).rows).toBe(0);
  });

  it("見せる位置・上端のセル・配置が変わったときの位置", () => {
    const l = withTimeline(base(), MOCK_TIMELINE.groups, 10);
    // 位置 4（夕方のイベントの最初の写真）を上から見せるときは、イベントの見出しも見せる。
    expect(revealOffset(4, l, 300, l.contentHeight)).toBe(rowTop(l, 3) - GRID_GAP);
    // 位置 9（撮影日時なし）を下から見せる。
    const bottom = rowTop(l, 8) + l.cellHeight + GRID_GAP - 300;
    expect(revealOffset(9, l, 300, 0)).toBe(bottom);
    expect(isFullyVisible(l, 0, 0, 1000)).toBe(true);
    expect(isFullyVisible(l, 9, 0, 300)).toBe(false);
    // 上端が見出しにかかっていれば、その下のセルの行の先頭。
    expect(firstVisibleIndex(l, rowTop(l, 3), 10)).toBe(4);
    expect(firstVisibleIndex(l, 0, 10)).toBe(0);
    // 列の数が変わっても、同じ写真が同じあたりに来る。
    const narrow = withTimeline(gridLayout(400, 10, 160), MOCK_TIMELINE.groups, 10);
    const v = anchoredOffset(l, narrow, rowTop(l, 4) - 10, 600, 4);
    expect(v).toBe(rowTop(narrow, rowOfIndex(narrow, 4)) - 10);
  });

  it("↑↓・PageUp・PageDown は見出しを飛ばして同じ列へ", () => {
    const l = withTimeline(base(), MOCK_TIMELINE.groups, 10);
    // 位置 3（朝の 4 枚目）から ↓ で夕方の行の同じ列 → 行が短いので最後（位置 6）。
    expect(navigateLayout("ArrowDown", 3, 10, l, 2)).toBe(6);
    expect(navigateLayout("ArrowDown", 1, 10, l, 2)).toBe(5);
    expect(navigateLayout("ArrowUp", 5, 10, l, 2)).toBe(1);
    expect(navigateLayout("ArrowUp", 2, 10, l, 2)).toBeNull();
    expect(navigateLayout("ArrowDown", 9, 10, l, 2)).toBeNull();
    // PageDown は 2 行分。
    expect(navigateLayout("PageDown", 0, 10, l, 2)).toBe(7);
    expect(navigateLayout("PageUp", 9, 10, l, 10)).toBe(0);
    // ←→・Home・End はふつうのグリッドと同じ。
    expect(navigateLayout("ArrowRight", 3, 10, l, 2)).toBe(4);
    expect(navigateLayout("End", 0, 10, l, 2)).toBe(9);
    expect(navigateLayout("Home", null, 10, l, 2)).toBe(0);
  });

  it("50 万件（3,667 日）でも行の表を作れ、内容の高さは上限までに縮める", () => {
    const groups: TimelineGroup[] = [];
    let start = 0;
    for (let d = 0; d < 3667; d++) {
      const day = new Date(Date.UTC(2016, 0, 1 + d)).toISOString().slice(0, 10);
      // 1 日 2 つのイベント。
      for (const [k, n] of [
        [0, 80],
        [1, 56],
      ]) {
        groups.push(mockTimelineGroup({ start, count: n, day, new_day: k === 0, photos: n }));
        start += n;
      }
    }
    const t0 = performance.now();
    const l = withTimeline(gridLayout(900, start, 160), groups, start);
    const ms = performance.now() - t0;
    expect(start).toBe(498_712);
    const tl = l.timeline!;
    // 日付の見出し 3,667・イベントの見出し 7,334・セルの行は 1 日 16 + 12 行。
    expect(l.rows).toBe(3667 * (1 + 2 + 16 + 12));
    expect(l.scrollHeight).toBe(MAX_SCROLL_PX);
    expect(rowCells(l, tl.count - 1, start)?.end).toBe(start);
    expect(rowOfIndex(l, start - 1)).toBe(tl.count - 1);
    // 参考（このコンテナの node で数十ミリ秒の見込み。時間では失敗にしない）。
    expect(ms).toBeGreaterThanOrEqual(0);
  });
});
