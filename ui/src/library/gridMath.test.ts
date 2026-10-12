import { describe, expect, it } from "vitest";

import {
  anchoredOffset,
  CELL_FOOTER,
  clampVirtual,
  firstVisibleIndex,
  GRID_GAP,
  gridLayout,
  isFastScroll,
  isFullyVisible,
  isScaled,
  MAX_SCROLL_PX,
  navigate,
  pageRows,
  revealOffset,
  rowTop,
  toPhysical,
  toVirtual,
  wheelPixels,
} from "./gridMath";

describe("配置", () => {
  it("幅とセルの大きさから列の数とセルの幅を決める", () => {
    // 幅 900・目安 160: 列は floor((900 - 6) / 166) = 5、セルの幅は floor(894 / 5) - 6 = 172。
    const l = gridLayout(900, 12, 160);
    expect(l.cols).toBe(5);
    expect(l.cellWidth).toBe(172);
    expect(l.cellHeight).toBe(172 + CELL_FOOTER);
    expect(l.rowHeight).toBe(172 + CELL_FOOTER + GRID_GAP);
    expect(l.rows).toBe(3);
    expect(l.contentHeight).toBe(GRID_GAP + 3 * l.rowHeight);
    expect(l.scrollHeight).toBe(l.contentHeight);
    expect(isScaled(l)).toBe(false);
    // セルの全体の幅が表示の幅に収まる。
    expect(l.cols * (l.cellWidth + GRID_GAP) + GRID_GAP).toBeLessThanOrEqual(900);
  });

  it("狭くても 1 列、件数が 0 なら行も 0", () => {
    const l = gridLayout(50, 0, 160);
    expect(l.cols).toBe(1);
    expect(l.rows).toBe(0);
    expect(l.contentHeight).toBe(0);
  });

  it("50 万件ではスクロールの領域の高さを上限までに抑える", () => {
    const l = gridLayout(900, 500_000, 320);
    // 列 2・行の高さ 469: 25 万行 × 469 + 6 = 1 億 1,725 万 6 px。
    expect(l.cols).toBe(2);
    expect(l.rowHeight).toBe(469);
    expect(l.contentHeight).toBe(117_250_006);
    expect(l.scrollHeight).toBe(MAX_SCROLL_PX);
    expect(isScaled(l)).toBe(true);
  });
});

describe("スクロールの位置の対応", () => {
  it("上限に収まるときは物理と仮想が同じ", () => {
    const l = gridLayout(900, 100, 160);
    expect(toVirtual(1234, l, 600)).toBe(1234);
    expect(toPhysical(1234, l, 600)).toBe(1234);
  });

  it("縮めているときは比例させ、端と端が対応する", () => {
    const l = gridLayout(900, 500_000, 320);
    const view = 800;
    const maxPhys = l.scrollHeight - view;
    const maxVirt = l.contentHeight - view;
    expect(toVirtual(0, l, view)).toBe(0);
    expect(toVirtual(maxPhys, l, view)).toBeCloseTo(maxVirt, 3);
    expect(toPhysical(maxVirt, l, view)).toBeCloseTo(maxPhys, 3);
    const v = 50_000_000;
    expect(toVirtual(toPhysical(v, l, view), l, view)).toBeCloseTo(v, 3);
    // 範囲の外は端に収める。
    expect(toPhysical(-5, l, view)).toBe(0);
    expect(clampVirtual(1e12, l, view)).toBe(maxVirt);
    expect(clampVirtual(Number.NaN, l, view)).toBe(0);
  });
});

describe("見えるようにする位置", () => {
  const l = gridLayout(900, 100, 160); // 5 列・行の高さ 200・セルの高さ 194
  const view = 500;

  it("見えていれば動かさず、上・下にはみ出していれば最小だけ動かす", () => {
    expect(revealOffset(0, l, view, 0)).toBeNull();
    // 行 5 の上端は 6 + 5 × 200 = 1006。下端 1200 ＋余白 6 が表示の下端に来るように 1206 - 500。
    expect(revealOffset(25, l, view, 0)).toBe(706);
    // 上にはみ出していれば、上端の余白から見せる。
    expect(revealOffset(0, l, view, 300)).toBe(0);
    expect(revealOffset(10, l, view, 900)).toBe(rowTop(l, 2) - GRID_GAP);
  });

  it("真ん中に置く", () => {
    const v = revealOffset(50, l, view, 0, true);
    // 行 10 の上端 2006 から (500 - 194) / 2 = 153 上。
    expect(v).toBe(2006 - 153);
  });

  it("全体が見えているかを判定する", () => {
    expect(isFullyVisible(l, 0, 0, view)).toBe(true);
    expect(isFullyVisible(l, 10, 0, view)).toBe(false); // 行 2 は 406〜600
    expect(isFullyVisible(l, 10, 100, view)).toBe(true);
  });

  it("表示の上端にかかっている行の先頭", () => {
    expect(firstVisibleIndex(l, 0, 100)).toBe(0);
    // 行 0 の下端は 200。仮想 199 ではまだ行 0 が見える。
    expect(firstVisibleIndex(l, 199, 100)).toBe(0);
    expect(firstVisibleIndex(l, 200, 100)).toBe(5);
    expect(firstVisibleIndex(l, 1e9, 100)).toBe(95);
    expect(firstVisibleIndex(gridLayout(900, 0, 160), 0, 0)).toBeNull();
  });

  it("配置が変わっても、目印のセルを表示の中の同じあたりに置く", () => {
    const small = gridLayout(900, 1000, 160); // 5 列・行の高さ 200
    const large = gridLayout(900, 1000, 320); // 2 列・行の高さ 469
    // セル 50（行 10。上端 2006）が表示の上から 106 の位置にある。
    const v = 1900;
    const next = anchoredOffset(small, large, v, 600, 50);
    // 大きいセルでは行 25（上端 6 + 25 × 469 = 11731）。セルの高さ 463 が収まる範囲で、上から 106 に置く。
    expect(next).toBe(11731 - 106);
    // 表示に収まらない位置にあったときは、セルの全体（下の余白 6 まで）が見えるようにずらす。
    expect(anchoredOffset(small, large, 2006 - 500, 600, 50)).toBe(11731 - (600 - 463 - 6));
  });
});

describe("キーボードの移動", () => {
  it("←→ は 1 件、↑↓ は列の数だけ動く", () => {
    expect(navigate("ArrowRight", 3, 20, 5, 2)).toBe(4);
    expect(navigate("ArrowLeft", 0, 20, 5, 2)).toBeNull();
    expect(navigate("ArrowDown", 3, 20, 5, 2)).toBe(8);
    expect(navigate("ArrowUp", 8, 20, 5, 2)).toBe(3);
    expect(navigate("ArrowUp", 3, 20, 5, 2)).toBeNull();
  });

  it("↓ で真下にセルがない短い最後の行には、最後のセルへ移る", () => {
    // 22 件・5 列: 最後の行は 20〜21。
    expect(navigate("ArrowDown", 18, 22, 5, 2)).toBe(21);
    expect(navigate("ArrowDown", 21, 22, 5, 2)).toBeNull();
  });

  it("Home・End・PageUp・PageDown", () => {
    expect(navigate("Home", 13, 20, 5, 2)).toBe(0);
    expect(navigate("End", 3, 20, 5, 2)).toBe(19);
    expect(navigate("PageDown", 3, 100, 5, 2)).toBe(13);
    expect(navigate("PageDown", 93, 100, 5, 2)).toBe(99);
    expect(navigate("PageUp", 13, 100, 5, 2)).toBe(3);
    expect(navigate("PageUp", 3, 100, 5, 2)).toBe(0);
  });

  it("選択がなければ先頭（End は最後）を選び、件数が 0 なら動かない", () => {
    expect(navigate("ArrowRight", null, 20, 5, 2)).toBe(0);
    expect(navigate("End", null, 20, 5, 2)).toBe(19);
    expect(navigate("ArrowRight", 99, 20, 5, 2)).toBe(0);
    expect(navigate("ArrowRight", null, 0, 5, 2)).toBeNull();
  });

  it("1 画面の行の数は 1 以上", () => {
    const l = gridLayout(900, 100, 160);
    expect(pageRows(l, 650)).toBe(3);
    expect(pageRows(l, 10)).toBe(1);
  });
});

describe("ホイール・速いスクロール", () => {
  it("ホイールの量を px にする", () => {
    expect(wheelPixels(100, 0, 600)).toBe(100);
    expect(wheelPixels(3, 1, 600)).toBe(120);
    expect(wheelPixels(1, 2, 600)).toBe(600);
  });

  it("表示の高さを 80 ms より短い間に動いたら速い", () => {
    expect(isFastScroll(600, 16, 600)).toBe(true);
    expect(isFastScroll(100, 16, 600)).toBe(false);
    expect(isFastScroll(600, 100, 600)).toBe(false);
    expect(isFastScroll(600, 0, 0)).toBe(false);
  });
});
