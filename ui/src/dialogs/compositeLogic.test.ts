import { describe, expect, it } from "vitest";

import type { CompositeCheck, CompositeReport } from "../api/types";
import {
  defaultCompositeForm,
  describeCheck,
  describeCompositeJob,
  describeCompositeReport,
  destOf,
  parseQuality,
  toCompositeSettings,
} from "./compositeLogic";

const check = (patch: Partial<CompositeCheck> = {}): CompositeCheck => ({
  sources: [3, 1, 2],
  skipped_videos: 0,
  problems: [],
  missing: [],
  size_differs: [],
  geometry_differs: [],
  settings_differ: [],
  stacks: 0,
  estimated_buffer_bytes: 196_448_256,
  dest_dir: "/photos",
  file_name: "C-lighten-3.tif",
  ...patch,
});

const report = (patch: Partial<CompositeReport> = {}): CompositeReport => ({
  mode: "lighten",
  sources: [3, 1, 2],
  skipped_videos: 0,
  composited: 3,
  size: [7008, 4672],
  path: "/photos/C-lighten-3.tif",
  registered: 9,
  registration_error: null,
  stacked: true,
  stack_error: null,
  cpu_renders: 0,
  warnings: [],
  failure: null,
  cancelled: false,
  ...patch,
});

describe("合成の設定", () => {
  it("既定は TIFF 16bit・Adobe RGB・スタックにまとめる（コアの既定と同じ）", () => {
    const f = defaultCompositeForm("lighten");
    expect(toCompositeSettings(f)).toEqual({
      mode: "lighten",
      format: "tiff16",
      color_space: "adobe_rgb",
      remove_gps: false,
      stack_with_sources: true,
    });
    expect(destOf(f)).toBeNull();
    expect(destOf({ ...f, destDir: "  /out " })).toBe("/out");
  });

  it("JPEG は品質を確かめる（全角の数字も読む）", () => {
    const f = { ...defaultCompositeForm("darken"), format: "jpeg" as const };
    expect(toCompositeSettings({ ...f, quality: "８５" })?.format).toEqual({ jpeg: { quality: 85 } });
    expect(toCompositeSettings({ ...f, quality: "0" })).toBeNull();
    expect(toCompositeSettings({ ...f, quality: "101" })).toBeNull();
    expect(parseQuality("abc")).toBeNull();
    // JPEG でなければ品質は見ない。
    expect(toCompositeSettings({ ...defaultCompositeForm("darken"), quality: "x" })?.format).toBe("tiff16");
  });
});

describe("確認の結果", () => {
  const settings = toCompositeSettings(defaultCompositeForm("lighten"))!;

  it("枚数・保存するファイル・メモリの見込み", () => {
    const lines = describeCheck(check(), settings);
    expect(lines.every((l) => l.level === "info")).toBe(true);
    expect(lines[0].text).toBe("撮影日時の順に 3 枚を合成します");
    expect(lines.some((l) => l.text.includes("C-lighten-3.tif"))).toBe(true);
    expect(lines.some((l) => l.text.includes("187"))).toBe(true);
  });

  it("警告と合成できない理由", () => {
    const lines = describeCheck(
      check({
        skipped_videos: 1,
        geometry_differs: [2],
        settings_differ: [1, 2],
        size_differs: [2],
        stacks: 2,
        problems: ["合成するには 2 枚以上の写真を選んでください"],
      }),
      settings,
    );
    expect(lines.filter((l) => l.level === "warning")).toHaveLength(3);
    expect(lines.filter((l) => l.level === "error").map((l) => l.text)).toEqual(["合成するには 2 枚以上の写真を選んでください"]);
    expect(lines.some((l) => l.text.includes("動画 1 本"))).toBe(true);
    expect(lines.some((l) => l.text.includes("2 個のスタック"))).toBe(true);
    // スタックにまとめない設定では、スタックの行を出さない。
    expect(describeCheck(check({ stacks: 2 }), { ...settings, stack_with_sources: false }).some((l) => l.text.includes("スタック"))).toBe(false);
  });
});

describe("合成の結果", () => {
  it("成功", () => {
    const n = describeCompositeReport(report());
    expect(n.level).toBe("success");
    expect(n.message).toBe("比較明合成: 3 枚を 1 枚にしました（元の写真とスタックにまとめました）");
    expect(n.detail).toContain("/photos/C-lighten-3.tif");
    expect(n.detail).toContain("7008 × 4672");
  });

  it("失敗・取り消し・登録できない", () => {
    const failed = describeCompositeReport(
      report({
        path: null,
        registered: null,
        stacked: false,
        composited: 2,
        failure: { variant_id: 2, error: { kind: "invalid_input", message: "寸法が違います", hint: null } as never },
      }),
    );
    expect(failed.level).toBe("error");
    expect(failed.message).toContain("2 / 3 枚目まで");
    expect(failed.message).toContain("寸法が違います");

    const cancelled = describeCompositeReport(report({ cancelled: true, composited: 1, path: null, registered: null }));
    expect(cancelled.level).toBe("info");
    expect(cancelled.message).toContain("1 / 3 枚");

    const unregistered = describeCompositeReport(report({ registered: null, stacked: false, registration_error: "読めません" }));
    expect(unregistered.level).toBe("warning");
    expect(unregistered.detail).toContain("取り込み直せます");
  });

  it("ジョブ: 失敗はイベントの通知に任せる", () => {
    const base = { job_id: 1, kind: "composite" as const, done: 3, total: 3, error: null };
    expect(describeCompositeJob({ ...base, state: "failed", result: null } as never)).toBeNull();
    expect(describeCompositeJob({ ...base, state: "succeeded", result: { type: "composite", ...report() } } as never)?.level).toBe("success");
    expect(describeCompositeJob({ ...base, state: "cancelled", result: null } as never)?.message).toContain("取り消し");
  });
});
