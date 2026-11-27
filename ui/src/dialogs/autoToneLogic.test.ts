import { describe, expect, it } from "vitest";

import type { AutoToneBatchReport, JobInfo } from "../api/types";
import { AUTO_TONE_FAILURES_SHOWN, describeAutoToneJob, describeAutoToneReport } from "./autoToneLogic";

const report = (patch: Partial<AutoToneBatchReport> = {}): AutoToneBatchReport => ({
  applied: 0,
  unchanged: 0,
  skipped: 0,
  failed: [],
  cancelled: false,
  ...patch,
});

describe("一括の自動補正の結果の文（DEV-32）", () => {
  it("当てた枚数を知らせ、そのまま・動画は詳細に出す", () => {
    expect(describeAutoToneReport(report({ applied: 12 }))).toEqual({
      level: "success",
      message: "12 枚に自動補正を当てました",
      detail: undefined,
    });
    const n = describeAutoToneReport(report({ applied: 3, unchanged: 2, skipped: 1 }));
    expect(n.level).toBe("success");
    expect(n.detail).toBe("値が今と同じでそのままの写真: 2 枚\n飛ばした動画: 1 本");
  });

  it("失敗があれば警告にして、理由を上限まで出す", () => {
    const failed = Array.from({ length: AUTO_TONE_FAILURES_SHOWN + 2 }, (_, i) => ({
      variant_id: i + 1,
      reason: `理由 ${i + 1}`,
    }));
    const n = describeAutoToneReport(report({ applied: 1, failed }));
    expect(n.level).toBe("warning");
    const lines = n.detail?.split("\n") ?? [];
    expect(lines[0]).toBe(`失敗した写真: ${failed.length} 枚`);
    expect(lines).toContain("- 理由 1");
    expect(lines).not.toContain(`- 理由 ${AUTO_TONE_FAILURES_SHOWN + 1}`);
    expect(lines.at(-1)).toBe("ほか 2 件");
  });

  it("1 枚も当てなかったときは、失敗の有無で文を変える", () => {
    expect(describeAutoToneReport(report({ unchanged: 4 }))).toMatchObject({
      level: "info",
      message: "自動補正で値が変わる写真はありませんでした",
    });
    expect(describeAutoToneReport(report({ failed: [{ variant_id: 1, reason: "ファイルが見つかりません" }] }))).toMatchObject({
      level: "warning",
      message: "自動補正を当てられた写真はありませんでした",
    });
  });

  it("取り消しは、当てた枚数と戻し方を知らせる", () => {
    expect(describeAutoToneReport(report({ applied: 5, cancelled: true }))).toMatchObject({
      level: "info",
      message: "自動補正を取り消しました（5 枚には当てました。現像の画面の履歴から戻せます）",
    });
  });

  it("ジョブの失敗は二重に知らせない（null）。結果のない取り消し・成功も文にする", () => {
    const info = (patch: Partial<JobInfo>): JobInfo => ({
      job_id: 1,
      kind: "auto_tone",
      state: "succeeded",
      done: 0,
      total: 0,
      result: null,
      error: null,
      ...patch,
    });
    expect(describeAutoToneJob(info({ state: "failed" }))).toBeNull();
    expect(describeAutoToneJob(info({ state: "cancelled" }))).toEqual({ level: "info", message: "自動補正を取り消しました" });
    expect(describeAutoToneJob(info({}))).toEqual({ level: "success", message: "自動補正が終わりました" });
    expect(describeAutoToneJob(info({ result: { type: "auto_tone", ...report({ applied: 2 }) } }))?.message).toBe(
      "2 枚に自動補正を当てました",
    );
  });
});
