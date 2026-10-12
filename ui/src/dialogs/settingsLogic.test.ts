import { describe, expect, it } from "vitest";

import type { CoreSettings } from "../api/types";
import {
  bytesToGbText,
  catalogSettingsForm,
  catalogSettingsUpdate,
  formatUtcOffset,
  parseCacheGb,
  utcOffsetOptions,
} from "./settingsLogic";

const settings: CoreSettings = {
  preview_cache_dir: "/c/previews",
  preview_cache_bytes: 20_000_000_000,
  default_export_color_space: "srgb",
  default_utc_offset_minutes: 540,
};

describe("タイムゾーン", () => {
  it("表示は genzo-api の offset_label と同じ形", () => {
    expect(formatUtcOffset(540)).toBe("UTC+09:00");
    expect(formatUtcOffset(0)).toBe("UTC+00:00");
    expect(formatUtcOffset(-210)).toBe("UTC-03:30");
    expect(formatUtcOffset(345)).toBe("UTC+05:45");
    expect(formatUtcOffset(-840)).toBe("UTC-14:00");
    expect(formatUtcOffset(840)).toBe("UTC+14:00");
  });

  it("選択肢は −14:00〜+14:00 の 15 分刻み", () => {
    const o = utcOffsetOptions();
    expect(o[0]).toBe(-840);
    expect(o[o.length - 1]).toBe(840);
    expect(o.length).toBe(113);
    expect(o).toContain(345);
    // 刻みに乗らない今の値は加える。範囲外は加えない。
    const withOdd = utcOffsetOptions(541);
    expect(withOdd.length).toBe(114);
    expect(withOdd.indexOf(541)).toBe(withOdd.indexOf(540) + 1);
    expect(utcOffsetOptions(900).length).toBe(113);
  });
});

describe("キャッシュの上限", () => {
  it("GB の表示", () => {
    expect(bytesToGbText(20_000_000_000)).toBe("20");
    expect(bytesToGbText(100_000_000)).toBe("0.1");
    expect(bytesToGbText(1_234_567_890)).toBe("1.23");
  });

  it("GB の入力（最小は 100,000,000 バイト）", () => {
    expect(parseCacheGb("20")).toEqual({ bytes: 20_000_000_000 });
    expect(parseCacheGb("0.1")).toEqual({ bytes: 100_000_000 });
    expect(parseCacheGb("１.５")).toEqual({ bytes: 1_500_000_000 });
    expect("error" in parseCacheGb("0.09")).toBe(true);
    expect("error" in parseCacheGb("abc")).toBe(true);
    expect("error" in parseCacheGb("-1")).toBe(true);
    expect("error" in parseCacheGb("1000001")).toBe(true);
  });
});

describe("カタログの設定の更新", () => {
  it("変えた項目だけを送る", () => {
    const form = catalogSettingsForm(settings);
    expect(catalogSettingsUpdate(settings, form)).toEqual({ update: {}, changed: false });
    const r = catalogSettingsUpdate(settings, { ...form, cacheGb: "50", utcOffsetMinutes: 0 });
    expect(r).toEqual({
      update: { preview_cache_bytes: 50_000_000_000, default_utc_offset_minutes: 0 },
      changed: true,
    });
    const r2 = catalogSettingsUpdate(settings, { ...form, cacheDir: " /d/previews ", exportColorSpace: "adobe_rgb" });
    expect(r2).toEqual({
      update: { preview_cache_dir: "/d/previews", default_export_color_space: "adobe_rgb" },
      changed: true,
    });
  });

  it("表示の丸めでは上限を変えない", () => {
    const odd = { ...settings, preview_cache_bytes: 1_234_567_890 };
    expect(catalogSettingsUpdate(odd, catalogSettingsForm(odd))).toEqual({ update: {}, changed: false });
  });

  it("不正な入力は理由を返す", () => {
    const form = catalogSettingsForm(settings);
    expect("error" in catalogSettingsUpdate(settings, { ...form, cacheDir: "" })).toBe(true);
    expect("error" in catalogSettingsUpdate(settings, { ...form, cacheGb: "0.01" })).toBe(true);
    expect("error" in catalogSettingsUpdate(settings, { ...form, utcOffsetMinutes: 900 })).toBe(true);
  });
});
