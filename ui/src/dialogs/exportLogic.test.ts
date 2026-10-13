import { describe, expect, it } from "vitest";

import type { ErrorInfo, ExportReport, JobInfo } from "../api/types";
import {
  defaultExportForm,
  describeExportJob,
  describeExportReport,
  parseIntInput,
  restoreExportForm,
  savedExport,
  toExportSettings,
  validateExportForm,
} from "./exportLogic";

const err = (kind: ErrorInfo["kind"], message: string): ErrorInfo => ({
  kind,
  message,
  user_actionable: true,
  retryable: false,
  hint: null,
});

describe("書き出しの入力", () => {
  it("既定は JPEG 品質 90・元の大きさ・GPS を削除・連番・キーワードを書く", () => {
    const f = { ...defaultExportForm("display_p3"), destDir: "/out" };
    expect(validateExportForm(f)).toEqual({});
    // genzo-model の export.rs の JSON の形と同じ。
    expect(JSON.stringify(toExportSettings(f))).toBe(
      '{"format":{"jpeg":{"quality":90}},"color_space":"display_p3","size":"original","remove_gps":true,"on_conflict":"sequence","remove_keywords":false}',
    );
  });

  it("TIFF・PNG と長辺の指定", () => {
    const f = {
      ...defaultExportForm("srgb"),
      format: "png16" as const,
      sizeMode: "long_edge" as const,
      longEdge: "2048",
      removeGps: false,
      removeKeywords: true,
      onConflict: "skip" as const,
      destDir: "/out",
    };
    expect(JSON.stringify(toExportSettings(f))).toBe(
      '{"format":"png16","color_space":"srgb","size":{"long_edge":2048},"remove_gps":false,"on_conflict":"skip","remove_keywords":true}',
    );
    expect(toExportSettings({ ...f, format: "tiff16" })?.format).toBe("tiff16");
    expect(toExportSettings({ ...f, format: "png8" })?.format).toBe("png8");
  });

  it("品質は 1〜100 の整数", () => {
    const base = { ...defaultExportForm("srgb"), destDir: "/out" };
    expect(validateExportForm({ ...base, quality: "1" })).toEqual({});
    expect(validateExportForm({ ...base, quality: "100" })).toEqual({});
    expect(validateExportForm({ ...base, quality: "0" }).quality).toBeDefined();
    expect(validateExportForm({ ...base, quality: "101" }).quality).toBeDefined();
    expect(validateExportForm({ ...base, quality: "9.5" }).quality).toBeDefined();
    expect(validateExportForm({ ...base, quality: "" }).quality).toBeDefined();
    expect(toExportSettings({ ...base, quality: "0" })).toBeNull();
    // JPEG 以外では品質を見ない。
    expect(validateExportForm({ ...base, format: "tiff16", quality: "0" })).toEqual({});
  });

  it("長辺は 1〜65535 の整数", () => {
    const base = { ...defaultExportForm("srgb"), destDir: "/out", sizeMode: "long_edge" as const };
    expect(validateExportForm({ ...base, longEdge: "1" })).toEqual({});
    expect(validateExportForm({ ...base, longEdge: "65535" })).toEqual({});
    expect(validateExportForm({ ...base, longEdge: "0" }).longEdge).toBeDefined();
    expect(validateExportForm({ ...base, longEdge: "65536" }).longEdge).toBeDefined();
    // 元の大きさのときは見ない。
    expect(validateExportForm({ ...base, sizeMode: "original", longEdge: "0" })).toEqual({});
  });

  it("書き出し先は必須", () => {
    expect(validateExportForm(defaultExportForm("srgb")).destDir).toBeDefined();
    expect(validateExportForm({ ...defaultExportForm("srgb"), destDir: "  " }).destDir).toBeDefined();
  });

  it("整数の入力は全角と桁区切りも読む", () => {
    expect(parseIntInput("２０４８")).toBe(2048);
    expect(parseIntInput("2,048")).toBe(2048);
    expect(parseIntInput(" 90 ")).toBe(90);
    expect(parseIntInput("-1")).toBeNull();
    expect(parseIntInput("1e3")).toBeNull();
  });
});

describe("前回の設定", () => {
  it("保存したものを戻す", () => {
    const f = { ...defaultExportForm("adobe_rgb"), format: "tiff16" as const, destDir: "/out" };
    const saved = JSON.parse(JSON.stringify(savedExport(f, "srgb")));
    expect(restoreExportForm(saved, "srgb")).toEqual(f);
  });

  it("既定の色空間を設定で変えていれば、新しい既定を使う", () => {
    const f = { ...defaultExportForm("adobe_rgb"), destDir: "/out" };
    const saved = savedExport(f, "srgb");
    expect(restoreExportForm(saved, "display_p3").colorSpace).toBe("display_p3");
    expect(restoreExportForm(saved, "display_p3").destDir).toBe("/out");
  });

  it("壊れた値は既定値", () => {
    expect(restoreExportForm(null, "srgb")).toEqual(defaultExportForm("srgb"));
    expect(restoreExportForm("x", "display_p3")).toEqual(defaultExportForm("display_p3"));
    const r = restoreExportForm(
      { v: 1, defaultColorSpace: "srgb", form: { format: "gif", removeGps: "yes", onConflict: "keep", quality: 80 } },
      "srgb",
    );
    expect(r.format).toBe("jpeg");
    expect(r.removeGps).toBe(true);
    // 保存した値にない項目（キーワードを書き出さない。v1 の organize-a で足した）は既定値。
    expect(r.removeKeywords).toBe(false);
    expect(r.onConflict).toBe("sequence");
    expect(r.quality).toBe("90");
  });
});

describe("書き出しの結果", () => {
  const report = (over: Partial<ExportReport>): ExportReport => ({
    dest_dir: "/out",
    items: [],
    written: 0,
    skipped: 0,
    failed: 0,
    cancelled: false,
    ...over,
  });

  it("すべて書けたら成功", () => {
    const n = describeExportReport(report({ written: 3 }));
    expect(n.level).toBe("success");
    expect(n.message).toBe("書き出し: 3 枚を書き出しました");
    expect(n.detail).toContain("書き出し先: /out");
  });

  it("原本の保護・飛ばしの理由をまとめる", () => {
    const n = describeExportReport(
      report({
        written: 1,
        skipped: 2,
        failed: 2,
        items: [
          { variant_id: 1, outcome: { status: "failed", error: err("protected_destination", "原本です") } },
          { variant_id: 2, outcome: { status: "failed", error: err("protected_destination", "原本です") } },
          { variant_id: 3, outcome: { status: "skipped", reason: "同じ名前のファイルがあるため、書き出しませんでした", existing: "/out/a.jpg" } },
          { variant_id: 4, outcome: { status: "skipped", reason: "動画の書き出しは対象外です（EXP-05 は v1）", existing: null } },
        ],
      }),
    );
    expect(n.level).toBe("warning");
    expect(n.message).toBe("書き出し: 1 枚を書き出しました、2 枚は飛ばしました、2 枚は失敗しました");
    expect(n.detail).toContain("・原本と同じ場所・同じ名前（2 枚）");
    expect(n.detail).toContain("例: 原本と同じ場所・同じ名前になるため書き出しませんでした（原本は変更していません）");
    expect(n.detail).toContain("同じ名前のファイルがあるため、書き出しませんでした（1 枚）");
    expect(n.detail).toContain("連番");
  });

  it("失敗はエラーの種類ごとにまとめる（パスの違う文を別の理由にしない）", () => {
    const items: ExportReport["items"] = Array.from({ length: 300 }, (_, i) => ({
      variant_id: i + 1,
      outcome: {
        status: "failed" as const,
        error: {
          ...err("file_access", `ファイルにアクセスできません（/m/ext/IMG_${i}.ARW）: 見つかりません`),
          hint: "ドライブの接続・権限を確かめてください",
        },
      },
    }));
    items.push({ variant_id: 999, outcome: { status: "failed", error: err("storage", "空き容量が足りません") } });
    const n = describeExportReport(report({ failed: 301, items }));
    expect(n.detail?.split("\n")).toEqual([
      "失敗:",
      "・ファイルにアクセスできない（300 枚）",
      "　例: ファイルにアクセスできません（/m/ext/IMG_0.ARW）: 見つかりません",
      "　例: ファイルにアクセスできません（/m/ext/IMG_1.ARW）: 見つかりません",
      "　→ ドライブの接続・権限を確かめてください",
      "・空き容量・書き込みの問題（1 枚）",
      "　例: 空き容量が足りません",
      "書き出し先: /out",
    ]);
  });

  it("1 枚も書けなければエラー、取り消しは警告", () => {
    expect(
      describeExportReport(
        report({ failed: 1, items: [{ variant_id: 1, outcome: { status: "failed", error: err("storage", "容量不足") } }] }),
      ).level,
    ).toBe("error");
    const c = describeExportReport(report({ written: 2, cancelled: true }));
    expect(c.level).toBe("warning");
    expect(c.message).toBe("書き出しを取り消しました: 2 枚を書き出しました");
  });

  it("ジョブの失敗は二重に知らせない", () => {
    const info = (over: Partial<JobInfo>): JobInfo => ({
      job_id: 1,
      kind: "export",
      state: "succeeded",
      done: 1,
      total: 1,
      result: null,
      error: null,
      ...over,
    });
    expect(describeExportJob(info({ state: "failed" }))).toBeNull();
    expect(describeExportJob(info({ state: "cancelled" }))?.message).toBe("書き出しを取り消しました");
    expect(describeExportJob(info({ result: { type: "export", ...report({ written: 1 }) } }))?.level).toBe("success");
  });
});
