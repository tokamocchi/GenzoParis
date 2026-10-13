import { describe, expect, it } from "vitest";

import { MOCK_CAPTURE_DEVICES, MOCK_CAPTURE_EDITS, MOCK_CAPTURE_PLAN } from "../api/mock";
import type { CaptureTime } from "../api/types";
import {
  buildEdit,
  captureLocal,
  describeEditInfo,
  describePlan,
  deviceId,
  deviceLabel,
  deviceSummary,
  EMPTY_SHIFT,
  formatOffsetMinutes,
  formatShift,
  MAX_SHIFT_SECONDS,
  parseLocalDateTime,
  sameDevice,
  shiftInputOf,
  shiftSeconds,
  sourceLines,
  ZONE_OPTIONS,
  zoneLabel,
} from "./captureTimeLogic";

const base = { reference: 5, correct: "", correctZone: null, shift: EMPTY_SHIFT, zone: 540, zoneMode: "keep_local" as const };

describe("ずらす量", () => {
  it("日・時間・分・秒を秒にし、戻せる", () => {
    expect(shiftSeconds({ negative: false, days: "1", hours: "2", minutes: "3", seconds: "4" })).toBe(93_784);
    expect(shiftSeconds({ negative: true, days: "", hours: "１", minutes: "", seconds: "" })).toBe(-3_600);
    expect(shiftSeconds(EMPTY_SHIFT)).toBe(0);
    expect(shiftSeconds({ ...EMPTY_SHIFT, minutes: "1.5" })).toBeNull();
    expect(shiftSeconds({ ...EMPTY_SHIFT, seconds: "-3" })).toBeNull();
    expect(shiftSeconds({ ...EMPTY_SHIFT, days: "36526" })).toBeNull();
    expect(shiftSeconds({ ...EMPTY_SHIFT, days: "36525" })).toBe(MAX_SHIFT_SECONDS);
    expect(shiftInputOf(-93_784)).toEqual({ negative: true, days: "1", hours: "2", minutes: "3", seconds: "4" });
    expect(shiftSeconds(shiftInputOf(3_723))).toBe(3_723);
  });

  it("量の表示（CLI と同じ文）", () => {
    expect(formatShift(93_784)).toBe("+1 日 2 時間 3 分 4 秒");
    expect(formatShift(-60)).toBe("-1 分");
    expect(formatShift(0)).toBe("±0 秒");
    expect(formatShift(-9 * 3600)).toBe("-9 時間");
  });
});

describe("正しい撮影日時の入力", () => {
  it("いくつかの書き方を読み、実在しない日時は断る", () => {
    expect(parseLocalDateTime("2024-05-01 12:34:56")).toBe("2024-05-01T12:34:56");
    expect(parseLocalDateTime(" 2024/5/1 9:05 ")).toBe("2024-05-01T09:05:00");
    expect(parseLocalDateTime("２０２４-０５-０１Ｔ１２：００")).toBeNull(); // 全角の T は読まない
    expect(parseLocalDateTime("２０２４-０５-０１ １２：００：３０")).toBe("2024-05-01T12:00:30");
    expect(parseLocalDateTime("2024:05:01 12:00:00")).toBe("2024-05-01T12:00:00");
    expect(parseLocalDateTime("2024-02-30 12:00")).toBeNull();
    expect(parseLocalDateTime("2024-05-01 24:00")).toBeNull();
    expect(parseLocalDateTime("2024-05-01")).toBeNull();
    expect(parseLocalDateTime("きのう")).toBeNull();
  });

  it("写真の今の現地時刻（表示のオフセット）", () => {
    const c: CaptureTime = {
      raw: "2024:05:01 12:00:00",
      offset: "+09:00",
      tz_source: "user_set",
      tz_assumed: "+02:00",
      basis: "+09:00",
      correction_s: 0,
      utc: "2024-05-01T03:00:00.000Z",
    };
    expect(captureLocal(c)).toBe("2024-05-01T05:00:00");
    expect(captureLocal({ ...c, utc: null })).toBeNull();
    expect(captureLocal(null)).toBeNull();
  });
});

describe("タイムゾーン", () => {
  it("選択肢と表示", () => {
    expect(ZONE_OPTIONS[0]).toBe(-720);
    expect(ZONE_OPTIONS[ZONE_OPTIONS.length - 1]).toBe(840);
    expect(ZONE_OPTIONS).toContain(540);
    expect(ZONE_OPTIONS).toContain(330);
    expect(ZONE_OPTIONS).toContain(-210);
    expect([...ZONE_OPTIONS].sort((a, b) => a - b)).toEqual(ZONE_OPTIONS);
    expect(new Set(ZONE_OPTIONS).size).toBe(ZONE_OPTIONS.length);
    expect(formatOffsetMinutes(540)).toBe("+09:00");
    expect(formatOffsetMinutes(-210)).toBe("-03:30");
    expect(formatOffsetMinutes(0)).toBe("+00:00");
    expect(zoneLabel(540)).toBe("+09:00（日本・韓国）");
    expect(zoneLabel(345)).toBe("+05:45");
  });
});

describe("機材", () => {
  it("名前・区別・数", () => {
    const [a7c, a7iv, video] = MOCK_CAPTURE_DEVICES;
    expect(deviceLabel(a7iv)).toBe("SONY ILCE-7M4（シリアル 4012345）の写真");
    expect(deviceLabel(a7c)).toBe("SONY ILCE-7Cの写真");
    expect(deviceLabel(video)).toBe("機材の情報のない動画");
    expect(deviceId(a7c)).not.toBe(deviceId(a7iv));
    expect(sameDevice(a7iv, { camera: "SONY ILCE-7M4", serial: "4012345", kind: "photo" })).toBe(true);
    expect(sameDevice(a7iv, { camera: "SONY ILCE-7M4", serial: null, kind: "photo" })).toBe(false);
    expect(deviceSummary(a7iv)).toBe("1,200 枚・2024-05-01〜2024-05-03・撮影日時なし 2");
    expect(deviceSummary(video)).toBe("42 本・2024-05-01〜2024-05-04");
  });
});

describe("見込みと履歴の文", () => {
  it("見込み・結果・記録の内訳", () => {
    expect(describePlan(MOCK_CAPTURE_PLAN, false)).toEqual([
      "ずらす量: +1 時間 2 分 3 秒",
      "1,198 枚の撮影日時が変わります（対象 1,200 枚）。",
      "撮影日時の分からない 2 枚は変えません。",
    ]);
    expect(describePlan({ ...MOCK_CAPTURE_PLAN, shift_seconds: null }, true)[0]).toBe(
      "1,198 枚の撮影日時を修正しました（対象 1,200 枚）。",
    );
    expect(describePlan({ ...MOCK_CAPTURE_PLAN, changed: 0, without_time: 0, shift_seconds: 0 }, false)).toEqual([
      "ずらす量: ±0 秒",
      "撮影日時の変わる写真はありません（対象 1,200 枚）。",
    ]);
    expect(describePlan({ ...MOCK_CAPTURE_PLAN, assets: 0, shift_seconds: null }, false)).toEqual(["対象の写真がありません。"]);
    expect(sourceLines({ exif: 1, utc_only: 2, user_default: 3, user_set: 0 }, "+09:00")).toEqual([
      "オフセットが記録されている: 1",
      "オフセットの記録がない（既定のタイムゾーン +09:00 で推定）: 3",
      "UTC の印だけが記録されている（動画など。既定のタイムゾーンで表示）: 2",
    ]);
    expect(describeEditInfo(MOCK_CAPTURE_EDITS[0])).toBe("タイムゾーン +09:00（記録の時刻をその地域の時刻として読む）");
    expect(describeEditInfo(MOCK_CAPTURE_EDITS[1])).toBe("基準の写真に合わせる +1 時間 2 分 3 秒");
  });

  it("修正の内容を作る（足りない・誤った入力は null）", () => {
    expect(buildEdit("sync", { ...base, correct: "2024-05-01 12:00" })).toEqual({
      kind: "sync",
      reference: 5,
      correct: "2024-05-01T12:00:00",
      offset_minutes: null,
    });
    expect(buildEdit("sync", { ...base, correct: "?" })).toBeNull();
    expect(buildEdit("sync", { ...base, reference: null, correct: "2024-05-01 12:00" })).toBeNull();
    expect(buildEdit("shift", { ...base, shift: { ...EMPTY_SHIFT, hours: "1" } })).toEqual({ kind: "shift", seconds: 3600 });
    expect(buildEdit("shift", base)).toBeNull();
    expect(buildEdit("set_zone", { ...base, zone: 120, zoneMode: "keep_instant" })).toEqual({
      kind: "set_zone",
      offset_minutes: 120,
      mode: "keep_instant",
    });
    expect(buildEdit("clear_zone", base)).toEqual({ kind: "clear_zone" });
    expect(buildEdit("reset", base)).toEqual({ kind: "reset" });
  });
});
