import { describe, expect, it } from "vitest";

import { parseNumber } from "../develop/scales";
import { normalizeNumberText, parseSliderText } from "./Slider";

describe("スライダーの数値の入力欄", () => {
  it("IME の全角の数字・記号を半角にする", () => {
    expect(normalizeNumberText("１２")).toBe("12");
    expect(normalizeNumberText("－０．５")).toBe("-0.5");
    expect(normalizeNumberText("−0.5")).toBe("-0.5");
    expect(normalizeNumberText("＋２５")).toBe("25");
    expect(normalizeNumberText("+0.35 EV")).toBe("0.35 EV");
  });
  it("既定の読み方", () => {
    expect(parseSliderText("１２")).toBe(12);
    expect(parseSliderText("－０．５")).toBe(-0.5);
    expect(parseSliderText("−0.5")).toBe(-0.5);
    expect(parseSliderText("＋３")).toBe(3);
    expect(parseSliderText("　１２　")).toBe(12);
    // 空・読めない文字は NaN（元の値に戻す）。
    expect(parseSliderText("")).toBeNaN();
    expect(parseSliderText("　")).toBeNaN();
    expect(parseSliderText("abc")).toBeNaN();
  });
  it("現像のパネルの読み方（parseNumber）にも整えた文字を渡す", () => {
    expect(parseNumber(normalizeNumberText("－１．５０ EV"))).toBe(-1.5);
    expect(parseNumber(normalizeNumberText("５５００ K"))).toBe(5500);
  });
});
