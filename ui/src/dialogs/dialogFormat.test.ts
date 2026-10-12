import { describe, expect, it } from "vitest";

import {
  baseName,
  describeIntegrity,
  ensureDbExtension,
  formatJobProgress,
  formatLocalDateTime,
  groupReasons,
  parentDir,
  sortJobs,
} from "./dialogFormat";

describe("日時", () => {
  it("現地時刻で表示する", () => {
    expect(formatLocalDateTime("2026-10-12T05:03:04Z", 540)).toBe("2026-10-12 14:03:04");
    expect(formatLocalDateTime("2026-10-12T05:03:04.123456Z", 0)).toBe("2026-10-12 05:03:04");
    expect(formatLocalDateTime("2026-01-01T02:00:00Z", -300)).toBe("2025-12-31 21:00:00");
    expect(formatLocalDateTime("2026-10-12T05:03:04+09:00", 540)).toBe("2026-10-12 05:03:04");
  });

  it("読めなければそのまま", () => {
    expect(formatLocalDateTime("bad", 540)).toBe("bad");
  });

  it("ずれを省くと OS の現地時刻（形だけ確かめる）", () => {
    expect(formatLocalDateTime("2026-10-12T05:03:04Z")).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
  });
});

describe("パス", () => {
  it("名前とフォルダ", () => {
    expect(baseName("/a/b/catalog.db")).toBe("catalog.db");
    expect(baseName("C:\\Users\\me\\Pictures\\GenzoParis\\catalog.db")).toBe("catalog.db");
    expect(parentDir("/a/b/catalog.db")).toBe("/a/b");
    expect(parentDir("C:\\catalog.db")).toBe("C:\\");
    expect(parentDir("/catalog.db")).toBe("/");
    expect(parentDir("catalog.db")).toBeNull();
  });

  it(".db を付ける", () => {
    expect(ensureDbExtension("/a/main")).toBe("/a/main.db");
    expect(ensureDbExtension("/a/main.db")).toBe("/a/main.db");
    expect(ensureDbExtension("/a/main.DB")).toBe("/a/main.DB");
    expect(ensureDbExtension("/a.b/main.catalog")).toBe("/a.b/main.catalog.db");
    expect(ensureDbExtension("  ")).toBe("");
  });
});

describe("ジョブ", () => {
  it("進み具合の表示", () => {
    expect(formatJobProgress({ kind: "import", done: 120, total: 500 })).toBe("取り込み 120/500");
    expect(formatJobProgress({ kind: "export", done: 0, total: 0 })).toBe("書き出し…");
    expect(formatJobProgress({ kind: "import", done: 1500, total: 12000 })).toBe("取り込み 1,500/12,000");
    expect(formatJobProgress({ kind: "collect_garbage", done: 5, total: 3 })).toBe("キャッシュの整理 3/3");
  });

  it("実行中を先に、新しい順", () => {
    const jobs = {
      1: { jobId: 1, state: "running" as const },
      2: { jobId: 2, state: "succeeded" as const },
      3: { jobId: 3, state: "running" as const },
      4: { jobId: 4, state: "failed" as const },
    };
    expect(sortJobs(jobs).map((j) => j.jobId)).toEqual([3, 1, 4, 2]);
  });
});

describe("結果のまとめ", () => {
  it("詳細チェック", () => {
    expect(describeIntegrity({ ok: true, integrity_errors: [], foreign_key_violations: [], fts_error: null })).toEqual({
      ok: true,
      lines: ["問題は見つかりませんでした"],
    });
    const bad = describeIntegrity({
      ok: false,
      integrity_errors: ["row 1 missing"],
      foreign_key_violations: [],
      fts_error: "索引が壊れています",
    });
    expect(bad.ok).toBe(false);
    expect(bad.lines).toEqual([
      "データベースの整合性の問題: 1 件",
      "・row 1 missing",
      "テキスト検索の索引の問題: 索引が壊れています",
    ]);
  });

  it("理由ごとの件数（多い順・上限）", () => {
    expect(groupReasons(["a", "b", "a", "c", "a", "b"], "枚", 2)).toEqual([
      "・a（3 枚）",
      "・b（2 枚）",
      "・ほか 1 種類の理由",
    ]);
  });
});
