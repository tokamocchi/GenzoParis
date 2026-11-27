import { describe, expect, it } from "vitest";

import type { GpxLocationPlan } from "../api/types";
import {
  DEFAULT_GPX_FORM,
  DEFAULT_GPX_OPTIONS,
  describeGpxPlan,
  describeGpxResult,
  describeGpxSkips,
  describeTrack,
  formatTrackTime,
  gpxOptionsOf,
} from "./gpxLogic";

const plan = (patch: Partial<GpxLocationPlan> = {}): GpxLocationPlan => ({
  track_points: 1200,
  track_skipped: 0,
  track_start: "2024-05-01T01:00:00Z",
  track_end: "2024-05-01T03:00:00Z",
  photos: 10,
  matched: 6,
  interpolated: 4,
  changed: 5,
  unchanged: 1,
  out_of_track: 2,
  no_capture_time: 0,
  skipped_located: 2,
  skipped_videos: 0,
  missing: 0,
  edit_id: null,
  ...patch,
});

describe("GPX のログからの位置付けの入力", () => {
  it("既定の入力は、既定の設定になる", () => {
    expect(gpxOptionsOf(DEFAULT_GPX_FORM)).toEqual({ options: DEFAULT_GPX_OPTIONS });
  });

  it("全角の数字・マイナスの記号も読む", () => {
    expect(gpxOptionsOf({ maxGap: "６００", offset: "−120", overwrite: true })).toEqual({
      options: { max_gap_seconds: 600, offset_seconds: -120, overwrite: true },
    });
    expect(gpxOptionsOf({ maxGap: " 60 ", offset: "+30", overwrite: false })).toEqual({
      options: { max_gap_seconds: 60, offset_seconds: 30, overwrite: false },
    });
  });

  it("範囲の外・整数でない入力は誤りにする", () => {
    for (const maxGap of ["0", "86401", "", "1.5", "abc"]) {
      expect(gpxOptionsOf({ ...DEFAULT_GPX_FORM, maxGap })).toHaveProperty("error");
    }
    for (const offset of ["", "1209601", "-1209601", "1e3"]) {
      expect(gpxOptionsOf({ ...DEFAULT_GPX_FORM, offset })).toHaveProperty("error");
    }
    expect(gpxOptionsOf({ ...DEFAULT_GPX_FORM, offset: "-1209600" })).toHaveProperty("options");
  });
});

describe("GPX のログからの位置付けの文", () => {
  it("ログの点の数と期間（画面の地域の時刻）を出す", () => {
    const text = describeTrack(plan({ track_skipped: 3 }));
    expect(text).toContain("ログ: 1,200 点（");
    expect(text).toContain(`${formatTrackTime("2024-05-01T01:00:00Z")} 〜 ${formatTrackTime("2024-05-01T03:00:00Z")}`);
    expect(text).toContain("読めなかった点 3");
    expect(formatTrackTime("2024-05-01T01:00:00Z")).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
    expect(formatTrackTime("よめない")).toBe("よめない");
  });

  it("見込み: 付ける数と、補間した数（一部のときだけ）", () => {
    expect(describeGpxPlan(plan())).toBe("5 枚に位置を付けます（対象 10 枚）（4 枚は 2 点の間を補間）");
    expect(describeGpxPlan(plan({ interpolated: 6 }))).toBe("5 枚に位置を付けます（対象 10 枚）");
    expect(describeGpxPlan(plan({ changed: 0 }))).toBe("位置を付けられる写真はありません（対象 10 枚）");
  });

  it("付けない写真は、理由ごとに数を出す（0 のものは出さない）", () => {
    expect(describeGpxSkips(plan(), false)).toEqual([
      "1 枚は、同じ位置がすでに付いているため変わりません",
      "2 枚は、すでに位置があるため付けません（「位置のある写真にも付ける」で付けられます）",
      "2 枚は、撮影日時がログの記録から離れているため付けられません（間隔・時計のずれを確かめてください）",
    ]);
    expect(
      describeGpxSkips(plan({ unchanged: 0, skipped_located: 0, out_of_track: 0, no_capture_time: 1, skipped_videos: 2, missing: 3 }), true),
    ).toEqual([
      "1 枚は、撮影日時がないため付けられません",
      "動画 2 本は対象外です（動画には位置を付けません）",
      "3 枚は見つかりませんでした（削除された写真など）",
    ]);
  });

  it("結果: 付けた数と取り消し方。付けられなければ知らせる", () => {
    const done = describeGpxResult(plan({ edit_id: 7 }), false);
    expect(done.level).toBe("success");
    expect(done.message).toBe("GPX のログから 5 枚に位置を付けました（位置を付けるダイアログの「元に戻す」で取り消せます）");
    expect(done.detail?.split("\n")).toHaveLength(4);
    expect(describeGpxResult(plan({ changed: 0 }), false)).toMatchObject({
      level: "info",
      message: "GPX のログから位置を付けられる写真はありませんでした",
    });
  });
});
