import { describe, expect, it } from "vitest";

import type { ImportSourceScan } from "../api/types";
import { importThumbUrl, setWindowsStyleForTest } from "../api/uri";
import {
  describePick,
  onlyOf,
  pickNotes,
  pickedCount,
  selectAll,
  selectKind,
  setPickRange,
  sizeText,
  togglePick,
} from "./importPickLogic";

const scan = (patch: Partial<ImportSourceScan> = {}): ImportSourceScan => ({
  scan_id: 7,
  root: "/card",
  files: [
    { index: 0, rel_path: "A.ARW", name: "A.ARW", kind: "photo", size: 30_000_000, modified: null },
    { index: 1, rel_path: "DCIM/B.jpg", name: "B.jpg", kind: "photo", size: 5_000, modified: null },
    { index: 2, rel_path: "DCIM/C.MP4", name: "C.MP4", kind: "video", size: null, modified: null },
    { index: 3, rel_path: "DCIM/D.jpg", name: "D.jpg", kind: "photo", size: 100, modified: null },
  ],
  total: 4,
  truncated: false,
  unreadable: 0,
  skipped_links: 0,
  ...patch,
});

describe("取り込む前の選択（IMP-02）", () => {
  it("すべて選んでいれば only を渡さない（フォルダの全体）。一部なら一覧の順の相対パス", () => {
    const s = scan();
    const all = selectAll(s);
    expect(onlyOf(s, all)).toBeUndefined();
    const some = togglePick(togglePick(all, "DCIM/B.jpg"), "A.ARW");
    expect(onlyOf(s, some)).toEqual(["DCIM/C.MP4", "DCIM/D.jpg"]);
    expect(onlyOf(s, new Set())).toEqual([]);
    // もう一度選ぶと戻る。
    expect(togglePick(some, "A.ARW").has("A.ARW")).toBe(true);
    // 一覧にないパスは数えない・渡さない。
    expect(pickedCount(s, new Set(["A.ARW", "ない.jpg"]))).toBe(1);
    expect(onlyOf(s, new Set(["A.ARW", "ない.jpg"]))).toEqual(["A.ARW"]);
  });

  it("切り詰めた一覧では選んで取り込めない（only を渡さない）", () => {
    const s = scan({ truncated: true, total: 9000 });
    expect(onlyOf(s, new Set(["A.ARW"]))).toBeUndefined();
    expect(pickNotes(s)[0]).toContain("9,000 件あり、多すぎるため先頭の 4 件だけを表示しています");
    expect(pickNotes(scan({ unreadable: 2 }))).toEqual(["読めなかったフォルダ・ファイルが 2 件あります（取り込みの結果で確かめられます）"]);
    expect(pickNotes(scan())).toEqual([]);
  });

  it("範囲をそろえる（順は問わない。両端を含む）・種類で選ぶ", () => {
    const s = scan();
    expect([...setPickRange(s, new Set(), 2, 1, true)]).toEqual(["DCIM/B.jpg", "DCIM/C.MP4"]);
    expect([...setPickRange(s, selectAll(s), 0, 2, false)]).toEqual(["DCIM/D.jpg"]);
    // 範囲の外の番号は、一覧の端までにする。
    expect(setPickRange(s, new Set(), -3, 99, true).size).toBe(4);
    expect([...selectKind(s, "video")]).toEqual(["DCIM/C.MP4"]);
    expect(selectKind(s, "photo").size).toBe(3);
  });

  it("一覧の説明と、大きさの表示", () => {
    const s = scan();
    expect(describePick(s, selectKind(s, "photo"))).toBe("3 / 4 件を取り込みます（写真 3・動画 1）");
    expect(describePick(scan({ files: s.files.slice(0, 2) }), new Set())).toBe("0 / 2 件を取り込みます（写真 2）");
    expect(sizeText(30_000_000)).toBe("28.6 MB");
    expect(sizeText(5_000)).toBe("4.9 KB");
    expect(sizeText(100)).toBe("100 B");
    expect(sizeText(300 * 1024 * 1024)).toBe("300 MB");
    expect(sizeText(null)).toBe("");
  });

  it("サムネイルの URI は、走査の番号と一覧の中の番号で作る（パスを入れない）", () => {
    setWindowsStyleForTest(false);
    expect(importThumbUrl(7, 2)).toBe("genzo://localhost/import-thumb/7/2");
    setWindowsStyleForTest(true);
    expect(importThumbUrl(7, 2)).toBe("http://genzo.localhost/import-thumb/7/2");
    setWindowsStyleForTest(null);
  });
});
