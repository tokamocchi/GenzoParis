import { describe, expect, it } from "vitest";

import type { FolderInfo } from "../api/types";
import {
  ancestorKeys,
  buildFolderTree,
  type FolderSegment,
  folderName,
  layoutSegments,
  rootName,
  visibleFolderRows,
} from "./folderTree";

function folder(folder_id: number, parent_id: number | null, rel_path: string, volume_id = 1, mount = "/"): FolderInfo {
  const path = mount === "" ? null : rel_path === "" ? mount : `${mount.replace(/\/$/, "")}/${rel_path}`;
  return { folder_id, volume_id, parent_id, rel_path, path };
}

// / › home › me › Pictures の下に 2023・2024、2024 の下に 2・10。
const FOLDERS: FolderInfo[] = [
  folder(1, null, ""),
  folder(2, 1, "home"),
  folder(3, 2, "home/me"),
  folder(4, 3, "home/me/Pictures"),
  folder(5, 4, "home/me/Pictures/2024"),
  folder(6, 4, "home/me/Pictures/2023"),
  folder(7, 5, "home/me/Pictures/2024/10"),
  folder(8, 5, "home/me/Pictures/2024/2"),
];

describe("フォルダの名前", () => {
  it("ルートはボリュームのパス、それ以外は相対パスの最後", () => {
    expect(rootName(folder(1, null, ""))).toBe("/");
    expect(rootName({ folder_id: 9, volume_id: 2, parent_id: null, rel_path: "", path: "C:\\" })).toBe("C:");
    expect(rootName({ folder_id: 9, volume_id: 2, parent_id: null, rel_path: "", path: "/Volumes/SD/" })).toBe(
      "/Volumes/SD",
    );
    expect(rootName({ folder_id: 9, volume_id: 2, parent_id: null, rel_path: "", path: null })).toBe("ボリューム 2");
    expect(folderName(folder(5, 4, "home/me/Pictures/2024"))).toBe("2024");
  });
});

describe("フォルダの木", () => {
  it("子が 1 つのフォルダを 1 行にまとめ、兄弟を自然な順に並べる", () => {
    const roots = buildFolderTree(FOLDERS);
    expect(roots).toHaveLength(1);
    const root = roots[0];
    expect(root.segments.map((s) => s.name)).toEqual(["/", "home", "me", "Pictures"]);
    expect(root.key).toBe(4);
    expect(root.segments[3].fullPath).toBe("/home/me/Pictures");
    expect(root.children.map((c) => c.segments.map((s) => s.name))).toEqual([["2023"], ["2024"]]);
    expect(root.children[1].children.map((c) => c.segments[0].name)).toEqual(["2", "10"]);
  });

  it("ボリュームごとに根を作り、親が一覧にないフォルダ・自分を親とするフォルダは根にする", () => {
    const roots = buildFolderTree([
      folder(20, null, "", 2, "/Volumes/SD"),
      folder(21, 20, "DCIM", 2, "/Volumes/SD"),
      folder(1, null, ""),
      folder(30, 99, "lost/child"),
      { folder_id: 31, volume_id: 1, parent_id: 31, rel_path: "loop", path: "/loop" },
    ]);
    expect(roots.map((r) => r.segments.map((s) => s.name))).toEqual([
      ["/"],
      ["child"],
      ["loop"],
      ["/Volumes/SD", "DCIM"],
    ]);
  });

  it("開いている行の子だけを並べる", () => {
    const roots = buildFolderTree(FOLDERS);
    const closed = visibleFolderRows(roots, (_key, depth) => depth === 0);
    expect(closed.map((r) => [r.node.key, r.depth, r.hasChildren, r.open])).toEqual([
      [4, 0, true, true],
      [6, 1, false, false],
      [5, 1, true, false],
    ]);
    const open = visibleFolderRows(roots, () => true);
    expect(open.map((r) => r.node.key)).toEqual([4, 6, 5, 8, 7]);
  });

  it("フォルダを含む行の鍵を根から順に返す", () => {
    const roots = buildFolderTree(FOLDERS);
    expect(ancestorKeys(roots, 8)).toEqual([4, 5, 8]);
    // まとめた行の途中のフォルダ。
    expect(ancestorKeys(roots, 2)).toEqual([4]);
    expect(ancestorKeys(roots, 999)).toBeNull();
  });
});

describe("行の区切りの見せ方", () => {
  const segs = (names: string[]): FolderSegment[] =>
    names.map((name, i) => ({ id: i + 1, name, fullPath: names.slice(0, i + 1).join("\\") }));
  const names = (list: FolderSegment[]) => list.map((s) => s.name);

  it("区切りが多ければ、先頭と末尾の 2 つを残して間を「…」にまとめる", () => {
    const lay = layoutSegments(segs(["C:", "Users", "me", "Pictures", "DCIM", "2026", "2026-10-17"]));
    expect(names(lay.head)).toEqual(["C:"]);
    expect(names(lay.hidden)).toEqual(["Users", "me", "Pictures", "DCIM"]);
    expect(names(lay.tail)).toEqual(["2026", "2026-10-17"]);
  });

  it("まとめても 1 つしか減らない（4 つまで）ならまとめない", () => {
    for (const list of [["F:"], ["F:", "DCIM"], ["C:", "Users", "me", "Pictures"]]) {
      const lay = layoutSegments(segs(list));
      expect(names(lay.head)).toEqual(list);
      expect(lay.hidden).toEqual([]);
      expect(lay.tail).toEqual([]);
    }
    const five = layoutSegments(segs(["C:", "Users", "me", "Pictures", "DCIM"]));
    expect(names(five.hidden)).toEqual(["Users", "me"]);
    expect(names(five.tail)).toEqual(["Pictures", "DCIM"]);
  });
});
