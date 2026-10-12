import { describe, expect, it } from "vitest";

import {
  activeConditionCount,
  choiceToList,
  clearConditions,
  facetOptions,
  flagOption,
  flagsFor,
  flipDirection,
  formatUtcOffset,
  isDateRangeReversed,
  localDateToUtc,
  MULTIPLE,
  normalizeText,
  ratingOption,
  ratingPatch,
  singleChoice,
  toggleLabel,
  utcToLocalDate,
} from "./filterLogic";

describe("有効な条件の数", () => {
  it("フォルダは数えず、範囲の条件は 1 つに数える", () => {
    expect(activeConditionCount({})).toBe(0);
    expect(activeConditionCount({ folder_id: 3, include_subfolders: true })).toBe(0);
    expect(activeConditionCount({ text: "  " })).toBe(0);
    expect(activeConditionCount({ text: "犬" })).toBe(1);
    expect(activeConditionCount({ rating_min: 3, rating_max: 5 })).toBe(1);
    expect(
      activeConditionCount({ captured_from: "2024-01-01T00:00:00Z", captured_until: "2024-02-01T00:00:00Z" }),
    ).toBe(1);
    expect(activeConditionCount({ cameras: [], lenses: ["FE 24-70mm"] })).toBe(1);
    expect(
      activeConditionCount({ flags: [1], color_labels: [null], kind: "video", masters_only: true, keywords: [] }),
    ).toBe(4);
  });

  it("条件の解除ではフォルダの選択を残す", () => {
    expect(clearConditions({ folder_id: 7, include_subfolders: true, rating_min: 3, text: "a" })).toEqual({
      folder_id: 7,
      include_subfolders: true,
    });
    expect(clearConditions({ rating_min: 3 })).toEqual({});
  });

  it("空白だけの文字は条件なし", () => {
    expect(normalizeText("  ")).toBeNull();
    expect(normalizeText(" 夕日 ")).toBe("夕日");
  });
});

describe("評価・フラグ・カラーラベル", () => {
  it("評価の選択肢と条件を行き来する", () => {
    expect(ratingPatch(3)).toEqual({ rating_min: 3, rating_max: null });
    expect(ratingPatch(0)).toEqual({ rating_min: null, rating_max: null });
    expect(ratingOption({ rating_min: 4 })).toBe(4);
    expect(ratingOption({})).toBe(0);
    // 上限のある条件は選択肢にない。
    expect(ratingOption({ rating_min: 1, rating_max: 3 })).toBe(-1);
  });

  it("フラグの選択肢と条件を行き来する", () => {
    expect(flagsFor("all")).toBeNull();
    expect(flagsFor("not_rejected")).toEqual([0, 1]);
    expect(flagOption(null)).toBe("all");
    expect(flagOption([1, 0])).toBe("not_rejected");
    expect(flagOption([-1])).toBe("rejected");
    expect(flagOption([-1, 1])).toBeNull();
  });

  it("カラーラベルを切り替え、決まった順に並べる", () => {
    expect(toggleLabel(null, "blue")).toEqual(["blue"]);
    expect(toggleLabel(["blue"], "red")).toEqual(["red", "blue"]);
    expect(toggleLabel(["red", "blue"], null)).toEqual(["red", "blue", null]);
    expect(toggleLabel(["red"], "red")).toBeNull();
  });
});

describe("カメラ・レンズの候補", () => {
  it("候補にない選択中の値を足す", () => {
    const values = [{ value: "ILCE-7M4", count: 10 }];
    expect(facetOptions(values, ["ILCE-1"])).toEqual([
      { value: "ILCE-7M4", count: 10 },
      { value: "ILCE-1", count: 0 },
    ]);
    expect(facetOptions(values, ["ILCE-7M4"])).toEqual(values);
    expect(facetOptions([], null)).toEqual([]);
  });

  it("1 つだけ選ぶ欄の値と条件を行き来する", () => {
    expect(singleChoice(null)).toBe("");
    expect(singleChoice(["A"])).toBe("A");
    expect(singleChoice(["A", "B"])).toBe(MULTIPLE);
    expect(choiceToList("")).toBeNull();
    expect(choiceToList(MULTIPLE)).toBeNull();
    expect(choiceToList("A")).toEqual(["A"]);
  });
});

describe("撮影日の範囲", () => {
  it("現地の日付を、既定のタイムゾーンで UTC の境界にする（終了日は翌日の 0 時。含まない）", () => {
    // genzo-catalog の search.rs: captured_from は含む、captured_until は含まない。
    expect(localDateToUtc("2024-05-01", 540, false)).toBe("2024-04-30T15:00:00Z");
    expect(localDateToUtc("2024-05-01", 540, true)).toBe("2024-05-01T15:00:00Z");
    expect(localDateToUtc("2024-05-01", -300, false)).toBe("2024-05-01T05:00:00Z");
    expect(localDateToUtc("2024-12-31", 0, true)).toBe("2025-01-01T00:00:00Z");
    expect(localDateToUtc("2024-02-29", 0, true)).toBe("2024-03-01T00:00:00Z");
  });

  it("空・ありえない日付・範囲の外は null", () => {
    expect(localDateToUtc("", 540, false)).toBeNull();
    expect(localDateToUtc("2023-02-29", 0, false)).toBeNull();
    expect(localDateToUtc("2024-13-01", 0, false)).toBeNull();
    expect(localDateToUtc("2024/05/01", 0, false)).toBeNull();
    expect(localDateToUtc("9999-12-31", 0, true)).toBeNull();
    expect(localDateToUtc("9999-12-31", 0, false)).toBe("9999-12-31T00:00:00Z");
    expect(localDateToUtc("0099-01-01", 0, false)).toBe("0099-01-01T00:00:00Z");
  });

  it("UTC の境界を現地の日付に戻す", () => {
    expect(utcToLocalDate("2024-04-30T15:00:00Z", 540, false)).toBe("2024-05-01");
    expect(utcToLocalDate("2024-05-01T15:00:00Z", 540, true)).toBe("2024-05-01");
    expect(utcToLocalDate("2024-05-01T05:00:00Z", -300, false)).toBe("2024-05-01");
    expect(utcToLocalDate(null, 540, false)).toBe("");
    expect(utcToLocalDate("壊れた値", 540, false)).toBe("");
  });

  it("開始日が終了日より後かを判定する", () => {
    expect(isDateRangeReversed("2024-05-02", "2024-05-01")).toBe(true);
    expect(isDateRangeReversed("2024-05-01", "2024-05-01")).toBe(false);
    expect(isDateRangeReversed("", "2024-05-01")).toBe(false);
  });

  it("タイムゾーンの表示", () => {
    expect(formatUtcOffset(540)).toBe("UTC+09:00");
    expect(formatUtcOffset(-330)).toBe("UTC−05:30");
    expect(formatUtcOffset(0)).toBe("UTC+00:00");
  });
});

describe("並べ替え", () => {
  it("向きを逆にする", () => {
    expect(flipDirection({ key: "rating", direction: "ascending" })).toEqual({ key: "rating", direction: "descending" });
    expect(flipDirection({ key: "rating", direction: "descending" })).toEqual({ key: "rating", direction: "ascending" });
  });
});
