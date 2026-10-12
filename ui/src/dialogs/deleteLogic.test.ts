import { describe, expect, it } from "vitest";

import type { DeletePlan, ErrorInfo } from "../api/types";
import {
  describeDeletePlan,
  describeDeleteReport,
  groupFileIssues,
  isOutdatedPlanError,
  planFileLines,
  planVariantLines,
  stripPathFromReason,
  summarizeDeletePlan,
} from "./deleteLogic";

const plan: DeletePlan = {
  plan_id: 7,
  kind: "trash",
  assets: [10, 11],
  variants: [
    { variant_id: 1, asset_id: 10, is_master: true, name: null },
    { variant_id: 2, asset_id: 10, is_master: false, name: "白黒" },
    { variant_id: 3, asset_id: 11, is_master: true, name: null },
  ],
  files: [
    { file_id: 100, asset_id: 10, role: "primary", path: "/p/a.ARW" },
    { file_id: 101, asset_id: 10, role: "sidecar_jpeg", path: "/p/a.JPG" },
    { file_id: 102, asset_id: 11, role: "primary", path: null },
  ],
};

const err = (kind: ErrorInfo["kind"], message: string): ErrorInfo => ({
  kind,
  message,
  user_actionable: false,
  retryable: false,
  hint: null,
});

describe("削除の計画", () => {
  it("要約", () => {
    const s = summarizeDeletePlan(plan, [1, 3]);
    expect(s).toEqual({
      variants: 3,
      masters: 2,
      virtualCopies: 1,
      assets: 2,
      files: 3,
      filesWithoutPath: 1,
      assetsWithoutPath: 1,
      trashableFiles: 2,
      includesUnselected: true,
    });
    expect(summarizeDeletePlan(plan, [1, 2, 3]).includesUnselected).toBe(false);
  });

  it("説明の文", () => {
    const s = summarizeDeletePlan(plan, [1, 3]);
    const trash = describeDeletePlan("trash", s);
    // 場所の分からないファイルを含む写真（asset 11）は移せずに残るので、移す数・除去する数に入れない。
    expect(trash[0]).toBe("2 個のファイルを OS のゴミ箱へ移します。");
    expect(trash[1]).toBe("1 枚の写真・動画をカタログから除去します。");
    expect(trash.join("\n")).toContain("まとめて対象");
    expect(trash.join("\n")).toContain(
      "1 枚は、場所の分からないファイル（1 個。ドライブが接続されていない可能性があります）を含むため、ゴミ箱へは移せず、失敗としてカタログに残します。",
    );
    // 場所がすべて分かれば、すべて移す。
    const known = summarizeDeletePlan({ ...plan, files: plan.files.map((f) => ({ ...f, path: f.path ?? "/p/b.ARW" })) }, [1, 3]);
    const all = describeDeletePlan("trash", known);
    expect(all[0]).toBe("3 個のファイルを OS のゴミ箱へ移します。");
    expect(all[1]).toBe("2 枚の写真・動画をカタログから除去します（仮想コピー 1 件を含む 3 件）。");
    expect(all.join("\n")).not.toContain("場所の分からない");
    const remove = describeDeletePlan("remove_from_catalog", s);
    expect(remove[1]).toBe("元のファイル 3 個は残ります。");
    expect(remove.join("\n")).not.toContain("場所の分からない");
    expect(describeDeletePlan("virtual_copies", { ...s, variants: 2 })[0]).toBe(
      "2 件の仮想コピーを削除します。元のファイルは変わりません。",
    );
  });

  it("一覧は上限まで（残りは件数）", () => {
    const f = planFileLines(plan, 2);
    expect(f.lines).toEqual(["/p/a.ARW", "/p/a.JPG（同時記録の JPEG）"]);
    expect(f.rest).toBe(1);
    expect(planFileLines(plan).lines[2]).toBe("場所不明のファイル（ID 102）");
    expect(planVariantLines(plan).lines[1]).toBe("仮想コピー「白黒」（ID 2）");
  });

  it("計画し直すエラー", () => {
    expect(isOutdatedPlanError(err("conflict", "確認の後にカタログが変わりました"))).toBe(true);
    expect(isOutdatedPlanError(err("not_found", "削除の計画 7 が見つかりません"))).toBe(true);
    expect(isOutdatedPlanError(err("not_found", "variant が見つかりません"))).toBe(false);
    expect(isOutdatedPlanError(err("trash_unavailable", "ゴミ箱へ移動できません"))).toBe(false);
  });
});

describe("削除の結果", () => {
  it("ゴミ箱: 移した・見つからなかった・失敗", () => {
    const n = describeDeleteReport("trash", {
      removed_variants: [1, 2],
      removed_assets: [10],
      trashed_files: ["/p/a.ARW"],
      skipped_missing: ["/p/a.JPG"],
      failed: [{ path: "/q/b.ARW", reason: "ゴミ箱へ移動できません" }],
    });
    expect(n.level).toBe("warning");
    expect(n.message).toBe("1 個のファイルをゴミ箱へ移し、1 枚をカタログから除去しました");
    expect(n.detail).toContain("見つからなかったファイル 1 個");
    expect(n.detail).toContain("・a.JPG");
    expect(n.detail).toContain("1 枚はゴミ箱へ移せなかったため、カタログに残しています");
    expect(n.detail).toContain("ゴミ箱へ移動できません（1 枚）");
  });

  it("失敗の理由はパスを除いてまとめる（同じ原因が別の理由に分かれない）", () => {
    expect(stripPathFromReason("ゴミ箱へ移動できません（/m/x/a.ARW）: 権限がありません", "/m/x/a.ARW")).toBe(
      "ゴミ箱へ移動できません: 権限がありません",
    );
    expect(stripPathFromReason("ファイルにアクセスできません（）: ボリュームのマウント先が分かりません", "")).toBe(
      "ファイルにアクセスできません: ボリュームのマウント先が分かりません",
    );
    const failed = Array.from({ length: 300 }, (_, i) => ({
      path: `/m/ext/IMG_${i}.ARW`,
      reason: `ファイルにアクセスできません（/m/ext/IMG_${i}.ARW）: ファイルのフォルダが見つかりません（ドライブが外れている可能性があります）。移していません`,
    }));
    const lines = groupFileIssues(failed, "枚", 3);
    expect(lines).toEqual([
      "・ファイルにアクセスできません: ファイルのフォルダが見つかりません（ドライブが外れている可能性があります）。移していません（300 枚）: IMG_0.ARW、IMG_1.ARW、IMG_2.ARW ほか 297 枚",
    ]);
    const n = describeDeleteReport("trash", {
      removed_variants: [],
      removed_assets: [],
      trashed_files: [],
      skipped_missing: [],
      failed,
    });
    expect(n.detail).not.toContain("種類の理由");
    expect(n.detail).toContain("ドライブの接続・権限を確かめてから");
  });

  it("すべて失敗ならエラー、問題がなければ成功", () => {
    const failed = describeDeleteReport("trash", {
      removed_variants: [],
      removed_assets: [],
      trashed_files: [],
      skipped_missing: [],
      failed: [{ path: "/q/b.ARW", reason: "x" }],
    });
    expect(failed.level).toBe("error");
    const ok = describeDeleteReport("remove_from_catalog", {
      removed_variants: [1],
      removed_assets: [10],
      trashed_files: [],
      skipped_missing: [],
      failed: [],
    });
    expect(ok).toEqual({
      level: "success",
      message: "1 枚をカタログから除去しました（元のファイルは残っています）",
      detail: undefined,
    });
    expect(
      describeDeleteReport("virtual_copies", {
        removed_variants: [2, 4],
        removed_assets: [],
        trashed_files: [],
        skipped_missing: [],
        failed: [],
      }).message,
    ).toBe("2 件の仮想コピーを削除しました");
  });
});
