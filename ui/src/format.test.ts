import { describe, expect, it } from "vitest";

import type { CaptureTime } from "./api/types";
import { formatBytes, formatCaptureTime, formatDuration, formatShutter, parseOffset } from "./format";

const cap = (utc: string | null, offset: string | null, tz_assumed: string | null): CaptureTime => ({
  raw: null,
  offset,
  tz_source: "exif",
  tz_assumed,
  correction_s: 0,
  utc,
});

describe("format", () => {
  it("撮影日時は記録されたオフセットの現地時刻", () => {
    expect(formatCaptureTime(cap("2024-05-01T01:02:03Z", "+09:00", "+09:00"))).toBe("2024-05-01 10:02:03");
    expect(formatCaptureTime(cap("2024-05-01T01:02:03Z", "Z", "+09:00"))).toBe("2024-05-01 10:02:03");
    expect(formatCaptureTime(cap("2024-05-01T01:02:03Z", null, null))).toBe("2024-05-01 01:02:03（UTC）");
    expect(formatCaptureTime(cap(null, null, null))).toBe("日時不明");
  });
  it("利用者がタイムゾーンを指定した写真は、指定した表示のオフセットの現地時刻（LIB-16）", () => {
    // 記録は +09:00、表示はパリ（+02:00）。撮影した瞬間（UTC）は同じ。
    const set: CaptureTime = {
      ...cap("2024-05-01T03:00:00Z", "+09:00", "+02:00"),
      tz_source: "user_set",
      basis: "+09:00",
    };
    expect(formatCaptureTime(set)).toBe("2024-05-01 05:00:00");
    // 表示のオフセットがなければ、記録のオフセットで表す。
    expect(formatCaptureTime(cap("2024-05-01T01:02:03Z", "+09:00", null))).toBe("2024-05-01 10:02:03");
  });
  it("オフセットを読む", () => {
    expect(parseOffset("+09:00")).toBe(540);
    expect(parseOffset("-03:30")).toBe(-210);
    expect(parseOffset("Z")).toBe(0);
    expect(parseOffset("bad")).toBeNull();
  });
  it("シャッター速度・長さ・大きさ", () => {
    expect(formatShutter(1 / 250)).toBe("1/250 秒");
    expect(formatShutter(2.5)).toBe("2.5 秒");
    expect(formatDuration(83)).toBe("1:23");
    expect(formatDuration(3723)).toBe("1:02:03");
    expect(formatBytes(1536)).toBe("1.50 KB");
  });
});
