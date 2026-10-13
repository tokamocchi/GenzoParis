import { describe, expect, it } from "vitest";

import { MOCK_CONTENT_HASH_STATUS, MOCK_CORE_SETTINGS, MOCK_DUPLICATE_GROUPS, MOCK_IMPORT_REPORT } from "../api/mock";
import type { HashReport, ImportReport } from "../api/types";
import {
  contentFileLine,
  describeHashReport,
  describeHashStatus,
  duplicateGroupTitle,
  duplicateListNote,
} from "./identityLogic";
import {
  describeImportReport,
  duplicateLines,
  importChoiceFrom,
  importChoiceRequest,
} from "./importLogic";

describe("取り込みの設定（IMP-04・IMP-06）", () => {
  it("初めの値はカタログの設定（古いコアでは genzo-api の既定）", () => {
    expect(importChoiceFrom(MOCK_CORE_SETTINGS)).toEqual({ skipDuplicates: true, separateRawJpeg: false });
    expect(importChoiceFrom({ ...MOCK_CORE_SETTINGS, import_skip_duplicates: false, import_separate_raw_jpeg: true })).toEqual({
      skipDuplicates: false,
      separateRawJpeg: true,
    });
    expect(importChoiceFrom(null)).toEqual({ skipDuplicates: true, separateRawJpeg: false });
    expect(importChoiceFrom({})).toEqual({ skipDuplicates: true, separateRawJpeg: false });
  });

  it("指定は省かずに渡し、変えた項目だけを既定として保存する", () => {
    const same = importChoiceRequest(MOCK_CORE_SETTINGS, { skipDuplicates: true, separateRawJpeg: false });
    expect(same).toEqual({ options: { skip_duplicates: true, separate_raw_jpeg: false }, update: null });
    const changed = importChoiceRequest(MOCK_CORE_SETTINGS, { skipDuplicates: false, separateRawJpeg: false });
    expect(changed).toEqual({
      options: { skip_duplicates: false, separate_raw_jpeg: false },
      update: { import_skip_duplicates: false },
    });
    const both = importChoiceRequest(MOCK_CORE_SETTINGS, { skipDuplicates: false, separateRawJpeg: true });
    expect(both.update).toEqual({ import_skip_duplicates: false, import_separate_raw_jpeg: true });
  });
});

describe("取り込みの結果の知らせ", () => {
  it("重複のため取り込まなかったもの・RAW と組にした JPEG を出す", () => {
    const n = describeImportReport(MOCK_IMPORT_REPORT);
    expect(n.level).toBe("success");
    expect(n.message).toBe("取り込み: 10 件を追加、0 件を更新、重複の 2 件は取り込まず");
    expect(n.detail).toContain("取り込まなかったファイル（2 件）");
    expect(n.detail).toContain("・/p/SD/DSC00001.ARW（同じ内容: /p/2024/DSC00001.ARW）");
    expect(n.detail).toContain("RAW と同時記録の JPEG 3 件を、RAW と 1 枚の写真にまとめました");
  });

  it("登録できなかったものがあれば注意。設定により登録した重複は別に挙げる", () => {
    const r: ImportReport = {
      ...MOCK_IMPORT_REPORT,
      unchanged: 4,
      not_registered: [{ path: "/p/x.jpg", reason: "読めません" }],
      duplicates: [{ path: "/p/b/A.jpg", existing_path: null, existing_variant_id: null, skipped: false }],
      paired: 0,
    };
    const n = describeImportReport(r);
    expect(n.level).toBe("warning");
    expect(n.message).toBe("取り込み: 10 件を追加、0 件を更新、4 件は変更なし");
    expect(n.detail?.split("\n")[0]).toBe("1 件のファイルを登録できませんでした");
    expect(n.detail).toContain("別の写真として登録したファイル（1 件）");
    expect(n.detail).toContain("・/p/b/A.jpg（同じ内容: 場所不明）");
    expect(n.detail).not.toContain("RAW と同時記録");
  });

  it("古いコア（重複・組の項目がない）と、何もない結果", () => {
    const old: ImportReport = { ...MOCK_IMPORT_REPORT, duplicates: undefined, paired: undefined };
    const n = describeImportReport(old);
    expect(n.message).toBe("取り込み: 10 件を追加、0 件を更新");
    expect(n.detail).toBeUndefined();
  });

  it("重複のファイルは、場所のすべてで全件を挙げる（結果の記録に残す）", () => {
    const many = Array.from({ length: 8 }, (_, i) => ({
      path: `/p/${i}.jpg`,
      existing_path: `/q/${i}.jpg`,
      existing_variant_id: i,
      skipped: true,
    }));
    const lines = duplicateLines(many);
    expect(lines).toHaveLength(8);
    expect(lines[7]).toBe("・/p/7.jpg（同じ内容: /q/7.jpg）");
    const n = describeImportReport({ ...MOCK_IMPORT_REPORT, duplicates: many });
    expect(n.detail).toContain("取り込まなかったファイル（8 件）");
    expect(n.detail).toContain("・/p/0.jpg（同じ内容: /q/0.jpg）");
    expect(n.detail).toContain("・/p/7.jpg（同じ内容: /q/7.jpg）");
    expect(n.detail).not.toContain("ほか");
  });
});

describe("内容のハッシュ（ORG-03）", () => {
  it("状況の文", () => {
    const lines = describeHashStatus(MOCK_CONTENT_HASH_STATUS);
    expect(lines[0]).toBe("1,250 / 1,300 個のファイルの内容のハッシュを記録済みです（計算の対象 50 個）。");
    expect(lines).toContain("内容が同じファイルの組: 2 組（記録済みのファイルの中で）");
    const running = describeHashStatus({ ...MOCK_CONTENT_HASH_STATUS, pending: 0, duplicate_groups: 0, job_id: 3 });
    expect(running[0]).toBe("1,250 / 1,300 個のファイルの内容のハッシュを記録済みです。");
    expect(running[1]).toContain("バックグラウンドで計算しています");
    expect(running[2]).toBe("内容が同じファイルはありません（記録済みのファイルの中で）。");
  });

  it("計算の結果の知らせ", () => {
    const r: HashReport = {
      hashed: 12,
      skipped_network: 3,
      skipped_unavailable: 0,
      changed: 1,
      failed_count: 0,
      failed: [],
      cancelled: false,
    };
    const n = describeHashReport(r);
    expect(n.level).toBe("success");
    expect(n.message).toBe("内容のハッシュを 12 個のファイルに記録しました");
    expect(n.detail).toContain("ネットワークのボリュームのため読まなかったファイル: 3 個");
    expect(n.detail).toContain("登録の後に内容が変わっていたファイル: 1 個");
    const failed = describeHashReport({ ...r, failed_count: 1, failed: [{ path: "/p/a.ARW", reason: "権限がありません" }] });
    expect(failed.level).toBe("warning");
    expect(failed.detail).toContain("・/p/a.ARW: 権限がありません");
    const cancelled = describeHashReport({ ...r, skipped_network: 0, changed: 0, cancelled: true });
    expect(cancelled).toEqual({
      level: "info",
      message: "内容のハッシュの計算を取り消しました（12 個は記録済み）",
      detail: undefined,
    });
  });

  it("内容が同じファイルの組の表示", () => {
    const [g1, g2] = MOCK_DUPLICATE_GROUPS.groups;
    expect(duplicateGroupTitle(0, g1.files.length, g1.full_hash)).toBe("1. 2 個のファイル（BLAKE3 aaaaaaaaaaaa…）");
    expect(contentFileLine(g1.files[0])).toBe("/p/2024/DSC00001.ARW");
    expect(contentFileLine(g2.files[0])).toBe("/p/2024/DSC00002.JPG（RAW と組の JPEG）");
    expect(contentFileLine(g2.files[1])).toBe("/p/書き出し前/DSC00002.JPG（見つからない）");
    expect(contentFileLine({ ...g1.files[0], path: null, status: "error" })).toBe("場所不明のファイル（ID 1）（読めない）");
    expect(duplicateListNote(MOCK_DUPLICATE_GROUPS)).toBeNull();
    expect(duplicateListNote({ total: 150, groups: MOCK_DUPLICATE_GROUPS.groups })).toBe(
      "全 150 組のうち、最初の 2 組を表示しています。",
    );
  });
});
