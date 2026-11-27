import { describe, expect, it } from "vitest";

import type { ShootingStats, StatBreakdown } from "../api/types";
import { statRows, statSections, statsSummary } from "./statsLogic";

const breakdown = (items: [string, number][], unknown = 0): StatBreakdown => ({
  items: items.map(([label, count]) => ({ label, count })),
  unknown,
});

const empty: StatBreakdown = { items: [], unknown: 0 };

describe("撮影の統計の行", () => {
  it("数の多い順の項目は、上位だけを残して「ほか」にまとめ、不明を最後に足す", () => {
    const rows = statRows(
      breakdown(
        [
          ["A", 50],
          ["B", 30],
          ["C", 15],
          ["D", 5],
        ],
        100,
      ),
      true,
      2,
    );
    expect(rows.map((r) => [r.label, r.count, r.note ?? false])).toEqual([
      ["A", 50, false],
      ["B", 30, false],
      ["ほか 2 種類", 20, true],
      ["不明", 100, true],
    ]);
    // 棒は項目の中で最も多い数（不明の 100）を 1 とし、割合は合計（200）に対する %。
    expect(rows.map((r) => r.fraction)).toEqual([0.5, 0.3, 0.2, 1]);
    expect(rows.map((r) => r.percent)).toEqual([25, 15, 10, 50]);
  });

  it("区切り・年の順の項目は、渡された順のまますべて出す", () => {
    const rows = statRows(
      breakdown([
        ["2022", 1],
        ["2023", 3],
        ["2024", 2],
      ]),
      false,
      1,
    );
    expect(rows.map((r) => r.label)).toEqual(["2022", "2023", "2024"]);
    expect(rows[1].fraction).toBe(1);
    expect(rows[0].percent).toBe(16.7);
  });

  it("何もなければ行を出さない", () => {
    expect(statRows(empty, true)).toEqual([]);
  });
});

describe("撮影の統計の項目と見出し", () => {
  const stats: ShootingStats = {
    photos: 1234,
    videos: 5,
    cameras: breakdown([["ILCE-7M4", 1239]]),
    lenses: empty,
    focal_lengths: breakdown([["24–34 mm", 10]], 2),
    isos: empty,
    apertures: empty,
    years: breakdown([["2024", 1239]]),
  };

  it("行のない項目は出さない", () => {
    expect(statSections(stats).map((s) => s.key)).toEqual(["cameras", "focal_lengths", "years"]);
  });

  it("見出しに写真・動画の数を出す", () => {
    expect(statsSummary(stats)).toBe("写真 1,234 枚、動画 5 本（仮想コピーは重ねて数えません）");
    expect(statsSummary({ ...stats, photos: 0, videos: 0 })).toBe("条件に合う写真・動画はありません");
  });
});
