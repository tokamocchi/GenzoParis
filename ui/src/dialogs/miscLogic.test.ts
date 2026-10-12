// 取り込み・一括適用・カタログの選択・ショートカットの一覧の純粋なロジックのテスト。

import { describe, expect, it } from "vitest";

import { normalizeCombo } from "../keyboard";
import type { ErrorInfo } from "../api/types";
import { catalogErrorGuide, decideCatalogPath, restoreDisplacedName } from "./catalogLogic";
import { splitDroppedPaths, videoToolsWarning } from "./importLogic";
import {
  DEFAULT_PASTE_SELECTION,
  KIND_LOOKUP_LIMIT,
  anySelected,
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

  it("コピー元と動画を除く", () => {
    const kinds: Record<number, "photo" | "video"> = { 1: "photo", 2: "video", 3: "photo" };
    const lookup = (id: number) => (kinds[id] ? { kind: kinds[id] } : undefined);
    expect(preparePasteTargets([1, 2, 3, 4, 3], 1, lookup)).toEqual({
      ids: [3, 4],
      skippedSource: true,
      skippedVideos: 1,
    });
    expect(preparePasteTargets([3], null, lookup)).toEqual({ ids: [3], skippedSource: false, skippedVideos: 0 });
    // 多いときは種類を探さない。
    const many = Array.from({ length: KIND_LOOKUP_LIMIT + 1 }, (_, i) => i + 10);
    expect(preparePasteTargets(many, null, () => ({ kind: "video" })).ids.length).toBe(KIND_LOOKUP_LIMIT + 1);
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
