import { describe, expect, it } from "vitest";

import type { CullingReport } from "../api/types";
import { DEFAULT_CULLING_OPTIONS, cullingSkips, describeCulling, describeGroup, softVariants } from "./cullingLogic";

const report = (patch: Partial<CullingReport> = {}): CullingReport => ({
  options: DEFAULT_CULLING_OPTIONS,
  analyzed: 1200,
  groups: [
    { variant_ids: [1, 2, 3], sharpest: 3, soft: [2] },
    { variant_ids: [7, 8], sharpest: 7, soft: [] },
    { variant_ids: [10, 11, 12, 13], sharpest: 11, soft: [10, 13] },
  ],
  grouped: 9,
  soft: 3,
  skipped_videos: 0,
  without_time: 0,
  without_thumbnail: 0,
  missing: 0,
  cancelled: false,
  ...patch,
});

describe("選別支援の結果の文（ORG-06）", () => {
  it("調べた数・グループ・甘い見込みの数を出す", () => {
    expect(describeCulling(report())).toBe("1,200 枚を調べました: 似た写真のグループ 3 組（9 枚）、ピントの甘い見込み 3 枚");
    expect(describeCulling(report({ groups: [], grouped: 0, soft: 0 }))).toBe(
      "1,200 枚を調べました。似た写真のグループは見つかりませんでした",
    );
  });

  it("調べなかった写真は、理由ごとに出す（0 のものは出さない）", () => {
    expect(cullingSkips(report())).toEqual([]);
    expect(cullingSkips(report({ skipped_videos: 2, without_time: 1, without_thumbnail: 3, missing: 4, cancelled: true }))).toEqual([
      "動画 2 本は調べていません",
      "撮影日時のない 1 枚は調べていません",
      "サムネイルのない 3 枚は調べていません",
      "4 枚は見つかりませんでした",
      "途中で取り消されました",
    ]);
  });

  it("甘い見込みの写真をグループの順に集める。グループの文", () => {
    expect(softVariants(report())).toEqual([2, 10, 13]);
    expect(describeGroup(report().groups[0])).toBe("3 枚、甘い見込み 1 枚");
    expect(describeGroup(report().groups[1])).toBe("2 枚");
  });
});
