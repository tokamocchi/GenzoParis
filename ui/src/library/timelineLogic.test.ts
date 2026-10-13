import { describe, expect, it } from "vitest";

import { MOCK_TIMELINE, mockTimelineGroup } from "../api/mock";
import {
  countsLabel,
  dayHeaderText,
  dayLabel,
  daySummary,
  eventHeaderText,
  monthIndex,
  monthsByYear,
  timeRange,
  TIMELINE_GAP_OPTIONS,
} from "./timelineLogic";

describe("見出しの文", () => {
  it("日付・時刻の範囲・数", () => {
    // 2024-05-01 は水曜日。
    expect(dayLabel("2024-05-01")).toBe("2024 年 5 月 1 日（水）");
    expect(dayLabel("2026-10-13")).toBe("2026 年 10 月 13 日（火）");
    expect(dayLabel("不明")).toBe("不明");
    expect(timeRange("2024-05-01T09:00:00", "2024-05-01T09:40:59")).toBe("09:00〜09:40");
    // 新しい順の並びでも、早いほうを先に書く。
    expect(timeRange("2024-05-01T18:05:00", "2024-05-01T17:10:00")).toBe("17:10〜18:05");
    expect(timeRange("2024-05-01T09:00:10", "2024-05-01T09:00:50")).toBe("09:00");
    expect(timeRange(null, null)).toBeNull();
    expect(countsLabel(12, 0)).toBe("12 枚");
    expect(countsLabel(0, 3)).toBe("動画 3 本");
    expect(countsLabel(1200, 3)).toBe("写真 1,200・動画 3");
  });

  it("日のまとめと見出し（イベントが 1 つなら時刻の範囲も）", () => {
    const g = MOCK_TIMELINE.groups;
    const dayStart = [1, 0, 1, 1];
    expect(daySummary(g, dayStart, 0)).toEqual({
      photos: 6,
      videos: 1,
      events: 2,
      first: "2024-05-01T09:00:00",
      last: "2024-05-01T18:05:00",
    });
    expect(dayHeaderText(g, dayStart, 0)).toEqual({ title: "2024 年 5 月 1 日（水）", meta: "写真 6・動画 1・2 のまとまり" });
    expect(dayHeaderText(g, dayStart, 2)).toEqual({ title: "2024 年 5 月 2 日（木）", meta: "10:00〜10:30・2 枚" });
    expect(dayHeaderText(g, dayStart, 3)).toEqual({ title: "撮影日時なし", meta: "1 枚" });
    expect(eventHeaderText(g[1])).toBe("17:10〜18:05・写真 2・動画 1");
  });
});

describe("年・月へのジャンプ", () => {
  it("最初に現れた順に、月ごとの数をまとめる", () => {
    const groups = [
      mockTimelineGroup({ start: 0, count: 3, day: "2025-01-02" }),
      mockTimelineGroup({ start: 3, count: 2, day: "2024-12-31" }),
      mockTimelineGroup({ start: 5, count: 4, day: "2024-12-01" }),
      mockTimelineGroup({ start: 9, count: 1, day: "2025-01-01" }),
      mockTimelineGroup({ start: 10, count: 1, day: null }),
    ];
    const months = monthIndex(groups);
    expect(months.map((m) => [m.key, m.group, m.count])).toEqual([
      ["2025-01", 0, 4],
      ["2024-12", 1, 6],
    ]);
    expect(monthsByYear(months).map((y) => [y.year, y.months.length])).toEqual([
      [2025, 1],
      [2024, 1],
    ]);
    expect(monthIndex([])).toEqual([]);
    expect(TIMELINE_GAP_OPTIONS.map((o) => o.value)).toContain(0);
  });
});
