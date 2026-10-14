import { describe, expect, it } from "vitest";

import { COMPARE_MODES, PANE_GAP_PX, comparePanes, mapCenter, paneLabel, paneSize, splitX } from "./compareMath";
import { nextCompareMode } from "./viewState";

describe("ビフォー / アフターの配置", () => {
  it("形ごとの面", () => {
    expect(comparePanes("off", 800, 600)).toEqual([{ target: "after", left: 0, top: 0, width: 800, height: 600 }]);
    expect(comparePanes("before", 800, 600)).toEqual([{ target: "before", left: 0, top: 0, width: 800, height: 600 }]);
    // 分割は 1 つの面に両方を重ねる（ビフォーが下）。
    expect(comparePanes("split", 800, 600).map((p) => [p.target, p.width])).toEqual([
      ["before", 800],
      ["after", 800],
    ]);
    // 左右: 左がビフォー、右がアフター、間に PANE_GAP_PX。
    const [l, r] = comparePanes("side_by_side", 800, 600);
    expect(l).toEqual({ target: "before", left: 0, top: 0, width: (800 - PANE_GAP_PX) / 2, height: 600 });
    expect(r.target).toBe("after");
    expect(r.left + r.width).toBe(800);
    expect(r.left - (l.left + l.width)).toBe(PANE_GAP_PX);
    // 使えないとき（切り抜きの編集中・ビフォーがない）はアフターだけ。
    expect(comparePanes("side_by_side", 800, 600, false)).toHaveLength(1);
    expect(paneSize("side_by_side", 800, 600)).toEqual({ width: (800 - PANE_GAP_PX) / 2, height: 600 });
    expect(paneSize("split", 800, 600)).toEqual({ width: 800, height: 600 });
    expect(comparePanes("side_by_side", 2, 10)[0].width).toBe(0);
  });

  it("分割の境目・中心の写し方・見出し", () => {
    expect(splitX(801)).toBe(401);
    expect(mapCenter({ centerX: 100, centerY: 50 }, { width: 400, height: 200 }, { width: 400, height: 200 })).toEqual({
      centerX: 100,
      centerY: 50,
    });
    expect(mapCenter({ centerX: 100, centerY: 50 }, { width: 400, height: 200 }, { width: 800, height: 400 })).toEqual({
      centerX: 200,
      centerY: 100,
    });
    expect(mapCenter({ centerX: 1, centerY: 2 }, { width: 0, height: 0 }, { width: 8, height: 4 })).toEqual({
      centerX: 1,
      centerY: 2,
    });
    expect(paneLabel("after", "読み込み")).toBe("アフター");
    expect(paneLabel("before", "読み込み")).toBe("ビフォー（読み込み）");
    expect(paneLabel("before", null)).toBe("ビフォー");
    expect(COMPARE_MODES.map((m) => m.key)).toEqual([null, "\\", "y", "shift+y"]);
  });

  it("キーで切り替える（同じ形ならアフターに戻る）", () => {
    expect(nextCompareMode("off", "before")).toBe("before");
    expect(nextCompareMode("before", "before")).toBe("off");
    expect(nextCompareMode("before", "side_by_side")).toBe("side_by_side");
    expect(nextCompareMode("split", "split")).toBe("off");
  });
});
