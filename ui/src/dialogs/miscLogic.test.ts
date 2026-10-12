// 取り込み・一括適用・カタログの選択・ショートカットの一覧の純粋なロジックのテスト。

import { describe, expect, it } from "vitest";

import { normalizeCombo } from "../keyboard";
import type { ErrorInfo } from "../api/types";
import {
  catalogErrorGuide,
  decideCatalogPath,
  isCloseSaveFailure,
  isDialogPermissionError,
  newCatalogFileName,
  restoreDisplacedName,
  restoreTargetNote,
  retryShouldCreate,
} from "./catalogLogic";
import { likelyFileNames, splitDroppedPaths, videoToolsWarning } from "./importLogic";
import {
  DEFAULT_PASTE_SELECTION,
  type LoadedItem,
  anySelected,
  collectLoaded,
  preparePasteTargets,
  restorePasteSelection,
  toSettingGroups,
} from "./pasteLogic";
import { SHORTCUT_GROUPS } from "./shortcutsData";

const err = (kind: ErrorInfo["kind"]): ErrorInfo => ({
  kind,
  message: "x",
  user_actionable: false,
  retryable: false,
  hint: null,
});

describe("取り込み", () => {
  it("ドロップしたものをフォルダとファイルに分ける", () => {
    expect(
      splitDroppedPaths(["/p/2024.05.01", "/p/a.ARW", "C:\\photos\\DCIM", "/p/b.mov", "/p/catalog.db", "/p/Trip.Paris"]),
    ).toEqual({
      folders: ["/p/2024.05.01", "C:\\photos\\DCIM", "/p/Trip.Paris"],
      files: ["/p/a.ARW", "/p/b.mov", "/p/catalog.db"],
    });
    // よくあるほかの形式のファイルも、ファイルとみなす。
    expect(
      splitDroppedPaths(["/p/a.gif", "/p/clip.MTS", "/p/a.webp", "/p/a.psd", "/p/a.mkv", "/p/a.HIF", "/p/a.docx"]).files,
    ).toHaveLength(7);
  });

  it("フォルダではない可能性がある名前（数字だけの拡張子は除く）", () => {
    expect(
      likelyFileNames(["/p/2024.05.01", "/p/DCIM", "C:\\x\\clip.xyz", "/p/Trip.Paris", "/p/a.b-c", "/p/.hidden"]),
    ).toEqual(["C:\\x\\clip.xyz", "/p/Trip.Paris"]);
  });

  it("ffmpeg・ffprobe がないときの注意", () => {
    expect(videoToolsWarning({ ffmpeg: "/usr/bin/ffmpeg", ffprobe: "/usr/bin/ffprobe" })).toBeNull();
    expect(videoToolsWarning(null)).toBeNull();
    const w = videoToolsWarning({ ffmpeg: null, ffprobe: null });
    expect(w).toContain("ffprobe");
    expect(w).toContain("ffmpeg（動画のサムネイル）");
    expect(videoToolsWarning({ ffmpeg: "/x/ffmpeg", ffprobe: null })).not.toContain("ffmpeg（");
  });
});

describe("一括適用", () => {
  it("既定は切り抜きと角度だけオフ。画面に出さない項目は true", () => {
    expect(toSettingGroups(DEFAULT_PASTE_SELECTION)).toEqual({
      white_balance: true,
      basic_tone: true,
      tone_curve: true,
      color: true,
      geometry: false,
      lens: true,
      detail: true,
      masks: true,
    });
  });

  it("保存した選択を戻す（壊れた項目は既定）", () => {
    expect(restorePasteSelection({ geometry: true, color: false, lens: false })).toEqual({
      ...DEFAULT_PASTE_SELECTION,
      geometry: true,
      color: false,
    });
    expect(restorePasteSelection("x")).toEqual(DEFAULT_PASTE_SELECTION);
    expect(restorePasteSelection({ white_balance: "no" })).toEqual(DEFAULT_PASTE_SELECTION);
  });

  it("何も選ばなければ貼り付けない", () => {
    expect(
      anySelected({ white_balance: false, basic_tone: false, tone_curve: false, color: false, geometry: false }),
    ).toBe(false);
    expect(anySelected(DEFAULT_PASTE_SELECTION)).toBe(true);
  });

  it("コピー元と動画を除く（読み込み済みの項目から）", () => {
    const loaded: LoadedItem[] = [
      { variant_id: 1, kind: "photo", file_name: "a.ARW" },
      { variant_id: 2, kind: "video", file_name: "b.mov" },
      { variant_id: 3, kind: "photo", file_name: "c.ARW" },
    ];
    expect(preparePasteTargets([1, 2, 3, 4, 3], 1, loaded)).toEqual({
      ids: [3, 4],
      skippedSource: true,
      skippedVideos: 1,
      source: loaded[0],
    });
    expect(preparePasteTargets([3], null, loaded)).toEqual({
      ids: [3],
      skippedSource: false,
      skippedVideos: 0,
      source: undefined,
    });
    // 読み込んでいない項目の種類は分からないので除かない。
    expect(preparePasteTargets([7, 8], 9, []).ids).toEqual([7, 8]);
  });

  it("読み込み済みの項目は 1 回だけ見る（全部見つかったらやめる）", () => {
    let visited = 0;
    function* items(): Generator<LoadedItem> {
      for (let i = 0; i < 500_000; i++) {
        visited++;
        yield { variant_id: i, kind: i % 2 === 0 ? "photo" : "video", file_name: `${i}.ARW` };
      }
    }
    const targets = Array.from({ length: 1000 }, (_, i) => i + 100);
    const r = preparePasteTargets(targets, 50, items());
    expect(r.ids).toHaveLength(500);
    expect(r.skippedVideos).toBe(500);
    expect(r.source?.file_name).toBe("50.ARW");
    // 最後の対象（1099）を見つけたところでやめる。
    expect(visited).toBe(1100);
    expect(collectLoaded(new Set(), items()).size).toBe(0);
  });
});

describe("カタログの選択", () => {
  it("選んだパスの扱い", () => {
    expect(decideCatalogPath("missing", "new")).toEqual({ create: true });
    expect(decideCatalogPath("catalog", "new")).toEqual({ create: false });
    expect("error" in decideCatalogPath("not_catalog", "new")).toBe(true);
    expect(decideCatalogPath("catalog", "existing")).toEqual({ create: false });
    expect("error" in decideCatalogPath("missing", "existing")).toBe(true);
    expect("error" in decideCatalogPath("not_catalog", "existing")).toBe(true);
  });

  it("復元で退避する名前の形", () => {
    expect(restoreDisplacedName("/p/GenzoParis/catalog.db")).toBe("catalog-before-restore-（日時）.db");
    expect(restoreDisplacedName("C:\\x\\main")).toBe("main-before-restore-（日時）.db");
  });

  it("保存のダイアログの権限のエラー（開発版・配布版の文）", () => {
    expect(
      isDialogPermissionError(
        "dialog.save not allowed. Permissions associated with this command: dialog:allow-save, dialog:default",
      ),
    ).toBe(true);
    expect(isDialogPermissionError("Command plugin:dialog|save not allowed by ACL")).toBe(true);
    expect(isDialogPermissionError(new Error("Command plugin:dialog|save not allowed by ACL"))).toBe(true);
    expect(isDialogPermissionError({ kind: "internal", message: "dialog.save not allowed." })).toBe(true);
    expect(isDialogPermissionError("ファイルにアクセスできません")).toBe(false);
    expect(isDialogPermissionError(null)).toBe(false);
  });

  it("新しいカタログの名前", () => {
    expect(newCatalogFileName(" 旅行 ")).toEqual({ name: "旅行.db" });
    expect(newCatalogFileName("main.DB")).toEqual({ name: "main.DB" });
    expect(newCatalogFileName("2024.05")).toEqual({ name: "2024.05.db" });
    for (const bad of ["", "  ", "a/b", "a\\b", "a:b", "a*b", 'a"b', "a<b", "a|b", "a?", ".", "..", "a.", ".db", ".x", "con", "COM1.db", "lpt9"]) {
      expect("error" in newCatalogFileName(bad), bad).toBe(true);
    }
  });

  it("再試行で作るか（作るつもりだったときだけ、まだなければ作る）", () => {
    expect(retryShouldCreate(true, "missing")).toBe(true);
    expect(retryShouldCreate(true, null)).toBe(true);
    expect(retryShouldCreate(true, "catalog")).toBe(false);
    expect(retryShouldCreate(true, "not_catalog")).toBe(false);
    expect(retryShouldCreate(false, "missing")).toBe(false);
    expect(retryShouldCreate(false, null)).toBe(false);
  });

  it("復元の確認: 今のカタログの扱い", () => {
    expect(restoreTargetNote("/p/catalog.db", "catalog", "/p")).toBe(
      "今のカタログは「catalog-before-restore-（日時）.db」の名前で同じフォルダ（/p）に残します（削除しません）。",
    );
    expect(restoreTargetNote("/p/catalog.db", null, "/p")).toContain("の名前で同じフォルダ");
    const missing = restoreTargetNote("/p/catalog.db", "missing", "/p");
    expect(missing).toContain("退避するものはありません");
    expect(missing).not.toContain("今のカタログは");
  });

  it("復元の前に閉じられなかったエラー（アプリの案内の文で見分ける）", () => {
    expect(
      isCloseSaveFailure({
        ...err("storage"),
        hint: "空き容量・カタログのフォルダの権限を確かめて再試行するか、保存していない現像設定を捨てて閉じてください",
      }),
    ).toBe(true);
    expect(isCloseSaveFailure({ ...err("catalog_corrupt"), hint: "別のバックアップを選んでください" })).toBe(false);
    expect(isCloseSaveFailure(err("storage"))).toBe(false);
  });

  it("理由ごとの案内", () => {
    expect(catalogErrorGuide(err("catalog_corrupt")).restore).toBe("open");
    expect(catalogErrorGuide(err("not_found")).restore).toBe("open");
    expect(catalogErrorGuide(err("conflict")).restore).toBe("none");
    // 版が合わないときは、古いバックアップへの復元を案内しない（作業を失いうる）。
    expect(catalogErrorGuide(err("incompatible_version")).restore).toBe("none");
    expect(catalogErrorGuide(err("storage")).restore).toBe("collapsed");
    expect(catalogErrorGuide(err("internal")).title).toBe("カタログを開けません");
  });
});

describe("ショートカットの一覧", () => {
  it("同じ区分の中でキーが重ならない", () => {
    for (const g of SHORTCUT_GROUPS) {
      const seen = new Set<string>();
      for (const item of g.items) {
        for (const k of item.keys) {
          const c = normalizeCombo(k);
          expect(seen.has(c), `${g.title}: ${k}`).toBe(false);
          seen.add(c);
        }
      }
    }
  });
});
