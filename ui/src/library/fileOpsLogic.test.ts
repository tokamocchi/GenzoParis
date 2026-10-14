import { describe, expect, it } from "vitest";

import {
  MOCK_FILE_MOVE_PLAN,
  MOCK_FILE_MOVE_REPORT,
  MOCK_FILE_RENAME_PLAN,
  MOCK_FOLDERS_FILES,
  MOCK_MISSING_REPORT,
  MOCK_RELINK_SEARCH,
  MOCK_RELOCATE_PLAN,
  MOCK_VOLUMES,
} from "../api/mock";
import type { FolderInfo, JobInfo } from "../api/types";
import {
  defaultRelinkChoice,
  describeCheckJob,
  describeCheckReport,
  describeMoveJob,
  describeMovePlan,
  describeMoveReport,
  describeFolderRemoval,
  describeRelinkReport,
  describeRelocatePlan,
  describeRenamePlan,
  folderChoices,
  folderMarks,
  folderSubtreeIds,
  isMissingOnly,
  missingOnlyPatch,
  offlineVolumesText,
  relinkRequests,
  splitExtension,
  validateName,
} from "./fileOpsLogic";
import { moreConditionCount } from "./filterLogic";

describe("名前の規則（コアの validate_name と同じ）", () => {
  it("どの OS でも使える名前だけを受け付ける", () => {
    expect(validateName("京都 2024")).toBeNull();
    expect(validateName("選別済み")).toBeNull();
    for (const bad of ["", "  ", "a/b", "a\\b", "a:b", "a?", "CON", "con.txt", "Lpt1", ".hidden", "末尾.", "末尾 ", " 先頭", "a\u0007b"]) {
      expect(validateName(bad), bad).not.toBeNull();
    }
    expect(validateName("あ".repeat(66))).toBeNull();
    expect(validateName("あ".repeat(67))).toContain("長すぎ");
  });

  it("拡張子を分ける（先頭の「.」は拡張子でない）", () => {
    expect(splitExtension("DSC01.ARW")).toEqual({ stem: "DSC01", ext: "ARW" });
    expect(splitExtension("a.b.jpg")).toEqual({ stem: "a.b", ext: "jpg" });
    expect(splitExtension(".hidden")).toEqual({ stem: ".hidden", ext: "" });
    expect(splitExtension("noext")).toEqual({ stem: "noext", ext: "" });
  });
});

describe("フォルダとボリュームの印", () => {
  it("オフライン・見つからないフォルダには落とせない", () => {
    const [online, offline, missing] = MOCK_FOLDERS_FILES;
    expect(folderMarks(online)).toMatchObject({ offline: false, missing: false, missingFiles: 2, droppable: true });
    expect(folderMarks(online).title).toContain("見つからない写真: 2 件");
    expect(folderMarks(offline)).toMatchObject({ offline: true, droppable: false });
    expect(folderMarks(offline).title).toContain("ドライブがつながっていません");
    expect(folderMarks(missing)).toMatchObject({ missing: true, droppable: false });
    // 古い応答（online などがない）は、つながっていて見つかるものとみなす。
    expect(folderMarks({ folder_id: 9, volume_id: 1, parent_id: null, rel_path: "", path: "/" }).droppable).toBe(true);
  });

  it("つながっていないドライブの案内", () => {
    expect(offlineVolumesText(MOCK_VOLUMES)).toBe("つながっていないドライブ: SD カード");
    expect(offlineVolumesText(MOCK_VOLUMES.filter((v) => v.online))).toBeNull();
  });
});

describe("移動・名前の変更の文", () => {
  it("予定: 移せる枚数・コピーの量・移せない理由", () => {
    const d = describeMovePlan(MOCK_FILE_MOVE_PLAN);
    expect(d.lines[0]).toBe("1 枚の写真を移します（1 枚は移せません）");
    expect(d.lines.join("\n")).toContain("RAW と同時記録の JPEG は一緒に動きます");
    expect(d.lines.join("\n")).toContain("元のファイルはゴミ箱へ移します");
    expect(d.problems).toEqual(["・移動先に同じ名前のもの（B.jpg）があります（1 枚）"]);
  });

  it("結果: ゴミ箱へ移せなかったものがあれば注意", () => {
    const n = describeMoveReport(MOCK_FILE_MOVE_REPORT, "移動");
    expect(n.level).toBe("warning");
    expect(n.message).toContain("1 枚の写真の移動をしました");
    expect(n.detail).toContain("ゴミ箱へ移せず、元の場所に残ったファイル");
    const ok = describeMoveReport({ ...MOCK_FILE_MOVE_REPORT, left_sources: [], skipped: [], bytes_copied: 0 }, "名前の変更");
    expect(ok).toEqual({ level: "success", message: "1 枚の写真の名前の変更をしました", detail: undefined });
  });
});

describe("見つからない写真・再リンク", () => {
  it("確認の結果: 自動の確認は変わりがなければ知らせない", () => {
    const n = describeCheckReport(MOCK_MISSING_REPORT, false);
    expect(n?.level).toBe("warning");
    expect(n?.message).toContain("新しく見つからなくなった写真 2 件");
    expect(n?.detail).toContain("見つからないフォルダ: 1 個");
    const quiet = { ...MOCK_MISSING_REPORT, newly_missing: 0, restored: 0 };
    expect(describeCheckReport(quiet, false)).toBeNull();
    expect(describeCheckReport(quiet, true)?.message).toContain("変わりはありません");
  });

  it("候補の既定の選択: 確からしい候補が 1 つだけなら選ぶ", () => {
    const [a, b, c] = MOCK_RELINK_SEARCH.targets;
    expect(defaultRelinkChoice(a)).toBe("/mnt/新しい/A.ARW");
    // 名前と撮影日時だけが同じ候補は選ばない。
    expect(defaultRelinkChoice(b)).toBeNull();
    // 候補なし。
    expect(defaultRelinkChoice(c)).toBeNull();
  });

  it("要求: 内容の違う候補は accept_changed を付ける", () => {
    const [a, b, c] = MOCK_RELINK_SEARCH.targets;
    const choices = new Map<number, string | null>([
      [a.file_id, a.candidates[0].path],
      [b.file_id, b.candidates[0].path],
      [c.file_id, null],
    ]);
    expect(relinkRequests(MOCK_RELINK_SEARCH.targets, choices)).toEqual([
      { file_id: a.file_id, path: a.candidates[0].path },
      { file_id: b.file_id, path: b.candidates[0].path, accept_changed: true },
    ]);
    // 候補にないパスは送らない。
    expect(relinkRequests([a], new Map([[a.file_id, "/別/x"]]))).toEqual([]);
  });

  it("再リンクの結果と、場所の指定し直しの文", () => {
    const n = describeRelinkReport({ linked: [{ file_id: 1, from: "/a", to: "/b" }], reread: ["/b"], failed: [] });
    expect(n.level).toBe("success");
    expect(n.message).toBe("1 個のファイルを結び付け直しました");
    expect(n.detail).toContain("読み直したファイル: 1 個");
    expect(describeRelocatePlan(MOCK_RELOCATE_PLAN)).toBe(
      "登録した写真のファイル 12 個のうち、11 個が新しい場所に同じ名前・大きさで見つかりました（配下のフォルダ 2 個を含む）",
    );
  });
});

describe("移動先の選択肢・ジョブの知らせ・絞り込み", () => {
  const f = (id: number, parent: number | null, rel: string, over: Partial<FolderInfo> = {}): FolderInfo => ({
    folder_id: id,
    volume_id: 1,
    parent_id: parent,
    rel_path: rel,
    path: `/${rel}`,
    online: true,
    missing: false,
    missing_files: 0,
    ...over,
  });
  const folders = [
    f(1, null, ""),
    f(2, 1, "photos"),
    f(3, 2, "photos/10"),
    f(4, 2, "photos/2"),
    f(5, 1, "work"),
    f(6, null, "", { volume_id: 2, path: "/media/SD", online: false }),
    f(7, 5, "work/gone", { missing: true }),
  ];

  it("つながっていて見つかるフォルダだけを、パスの自然な順に並べる", () => {
    expect(folderChoices(folders).map((c) => c.id)).toEqual([1, 2, 4, 3, 5]);
  });

  it("フォルダの移動: 自分とその配下を除き、同じボリュームだけにする", () => {
    expect(folderChoices(folders, { exclude: 2, volume: 1 }).map((c) => c.id)).toEqual([1, 5]);
    expect(folderChoices(folders, { volume: 2 })).toEqual([]);
  });

  it("フォルダをカタログから除く: 配下のフォルダと、見つからない写真の案内", () => {
    expect(folderSubtreeIds(folders, 2)).toEqual([2, 3, 4]);
    expect(folderSubtreeIds(folders, 7)).toEqual([7]);
    expect(folderSubtreeIds(folders, 99)).toEqual([]);
    expect(describeFolderRemoval(folders, 7)).toBe(
      "フォルダをカタログから除きます（ディスクのフォルダとファイルには触れません）。写真が登録されていれば除けません（先に写真をカタログから除くか、再リンクしてください）。",
    );
    const withMissing = folders.map((x) => (x.folder_id === 3 ? { ...x, missing_files: 4 } : x));
    expect(describeFolderRemoval(withMissing, 2)).toBe(
      "配下のフォルダ 2 個と一緒に、フォルダをカタログから除きます（ディスクのフォルダとファイルには触れません）。見つからない写真が 4 件登録されているため、除けません（先に写真をカタログから除くか、再リンクしてください）。",
    );
    expect(folderMarks(folders[6]).title).toContain("「カタログから除く…」");
  });

  const job = (over: Partial<JobInfo>): JobInfo => ({
    job_id: 1,
    kind: "move_files",
    state: "succeeded",
    done: 1,
    total: 1,
    result: null,
    error: null,
    ...over,
  });

  it("写真の移動のジョブの知らせ（失敗は別に知らせるので null）", () => {
    expect(describeMoveJob(job({ result: { type: "move_files", ...MOCK_FILE_MOVE_REPORT } }))?.level).toBe("warning");
    expect(describeMoveJob(job({ state: "cancelled" }))?.message).toBe("写真の移動を取り消しました");
    expect(describeMoveJob(job({ state: "failed" }))).toBeNull();
  });

  it("見つからない写真の確認のジョブの知らせ（自動のものは結果が読めなければ知らせない）", () => {
    const r = job({ kind: "check_missing", result: { type: "check_missing", ...MOCK_MISSING_REPORT } });
    expect(describeCheckJob(r, false)?.message).toContain("新しく見つからなくなった写真 2 件");
    expect(describeCheckJob(job({ kind: "check_missing" }), false)).toBeNull();
    expect(describeCheckJob(job({ kind: "check_missing", state: "cancelled" }), true)?.message).toContain("取り消しました");
  });

  it("名前の変更の予定の文", () => {
    const d = describeRenamePlan(MOCK_FILE_RENAME_PLAN);
    expect(d.lines).toEqual([
      "1 枚の写真の名前を変えます（1 枚は変えられません）",
      "ファイルは 2 個（RAW と同時記録の JPEG は同じ名前にそろえます）",
    ]);
    expect(d.problems).toEqual(["・今と同じ名前です（1 枚）"]);
  });

  it("見つからない写真だけの絞り込み（詳しい条件の数に入る）", () => {
    expect(isMissingOnly({ file_statuses: ["missing"] })).toBe(true);
    expect(isMissingOnly({ file_statuses: ["missing", "error"] })).toBe(false);
    expect(isMissingOnly({})).toBe(false);
    expect(missingOnlyPatch(true)).toEqual({ file_statuses: ["missing"] });
    expect(missingOnlyPatch(false)).toEqual({ file_statuses: null });
    expect(moreConditionCount({ file_statuses: ["missing"] })).toBe(1);
    expect(moreConditionCount({ file_statuses: null })).toBe(0);
  });
});
