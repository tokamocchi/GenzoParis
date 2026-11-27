import { describe, expect, it } from "vitest";

import type { JobInfo, VideoFrameReport } from "../api/types";
import { scrubUrl, setWindowsStyleForTest } from "../api/uri";
import { SCRUB_FRAMES, describeFrameJob, describeFrameReport, scrubIndex } from "./videoFrameLogic";

describe("動画のスクラブ表示（VID-05）", () => {
  it("カーソルの位置を、フレームの番号にする（端は範囲に収める）", () => {
    expect(scrubIndex(0, 200)).toBe(0);
    expect(scrubIndex(19, 200)).toBe(0);
    expect(scrubIndex(20, 200)).toBe(1);
    expect(scrubIndex(199, 200)).toBe(SCRUB_FRAMES - 1);
    expect(scrubIndex(200, 200)).toBe(SCRUB_FRAMES - 1);
    expect(scrubIndex(-5, 200)).toBe(0);
    expect(scrubIndex(50, 0)).toBe(0);
    expect(scrubIndex(Number.NaN, 200)).toBe(0);
  });

  it("フレームの URI は、動画と番号で作る", () => {
    setWindowsStyleForTest(false);
    expect(scrubUrl(12, 3)).toBe("genzo://localhost/scrub/12/3");
    setWindowsStyleForTest(null);
  });
});

describe("動画からの静止画の切り出し（VID-07）", () => {
  const report = (patch: Partial<VideoFrameReport> = {}): VideoFrameReport => ({
    source: 4,
    position_ms: 12_500,
    width: 3840,
    height: 2160,
    path: "/photos/2024/DJI_0012-frame-000012500.jpg",
    registered: 99,
    registration_error: null,
    ...patch,
  });

  it("保存して登録したことを知らせる", () => {
    expect(describeFrameReport(report())).toEqual({
      level: "success",
      message: "静止画を切り出して登録しました: DJI_0012-frame-000012500.jpg（3840 × 2160）",
      detail: "保存先: /photos/2024/DJI_0012-frame-000012500.jpg",
    });
  });

  it("登録できなかった・メタデータを読めなかったときは警告にする", () => {
    const failed = describeFrameReport(report({ registered: null, registration_error: "フォルダを登録できません" }));
    expect(failed.level).toBe("warning");
    expect(failed.message).toContain("カタログに登録できませんでした");
    expect(failed.detail).toContain("フォルダを登録できません");
    expect(describeFrameReport(report({ registration_error: "メタデータを読めませんでした" })).level).toBe("warning");
  });

  it("ジョブの失敗は二重に知らせない（null）", () => {
    const info = (patch: Partial<JobInfo>): JobInfo => ({
      job_id: 1,
      kind: "video_frame",
      state: "succeeded",
      done: 0,
      total: 0,
      result: null,
      error: null,
      ...patch,
    });
    expect(describeFrameJob(info({ state: "failed" }))).toBeNull();
    expect(describeFrameJob(info({ state: "cancelled" }))?.message).toBe("静止画の切り出しを取り消しました");
    expect(describeFrameJob(info({ result: { type: "video_frame", ...report() } }))?.level).toBe("success");
  });
});
