// 取り込み・一括適用・カタログの選択・ショートカットの一覧の純粋なロジックのテスト。

import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import * as core from "../api/core";
import { MockBridge, mockDevelopSettings, mockPasteCommands } from "../api/mock";

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
import { type PathKind, nonFolderPaths, splitDroppedPaths, videoToolsWarning } from "./importLogic";
import {
  DEFAULT_PASTE_SELECTION,
  type LoadedItem,
  anySelected,
  canOfferProcessUpgrade,
  collectLoaded,
  describePasteReport,
  describeProcessUpgrade,
  preparePasteTargets,
  processUpgradeOption,
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

  it("注意を出すのは、アプリで確かめてファイル・見つからないと分かったものだけ（名前からは推定しない）", () => {
    const folders = ["/p/2024.Kyoto", "/p/Trip.Paris", "/p/clip.mp4", "/p/gone", "/p/unchecked.xyz"];
    const kinds = new Map<string, PathKind>([
      ["/p/2024.Kyoto", "dir"],
      ["/p/Trip.Paris", "dir"],
      ["/p/clip.mp4", "file"],
      ["/p/gone", "missing"],
      // "/p/unchecked.xyz" は確かめていない（「参照…」で選んだもの・確かめている途中・確かめられなかったもの）。
    ]);
    expect(nonFolderPaths(folders, kinds)).toEqual({ files: ["/p/clip.mp4"], missing: ["/p/gone"] });
    expect(nonFolderPaths(folders, new Map())).toEqual({ files: [], missing: [] });
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
  it("既定はマスクと、切り抜きと角度・変形がオフ（レンズ補正は写す）", () => {
    expect(toSettingGroups(DEFAULT_PASTE_SELECTION)).toEqual({
      white_balance: true,
      basic_tone: true,
      presence: true,
      tone_curve: true,
      color: true,
      geometry: false,
      lens: true,
      detail: true,
      effects: true,
      profile: true,
      masks: false,
      transform: false,
    });
    // マスク（DEV-18）を選べば写す。
    expect(toSettingGroups({ ...DEFAULT_PASTE_SELECTION, masks: true }).masks).toBe(true);
    // ディテール（ノイズ軽減）を外せば写さない。
    expect(toSettingGroups({ ...DEFAULT_PASTE_SELECTION, detail: false }).detail).toBe(false);
  });

  it("保存した選択を戻す（壊れた項目は既定）", () => {
    expect(restorePasteSelection({ geometry: true, color: false, lens: false, detail: false })).toEqual({
      ...DEFAULT_PASTE_SELECTION,
      geometry: true,
      color: false,
      lens: false,
      detail: false,
    });
    // レンズ補正・ディテールを選べるようになる前に保存した選択（lens・detail が無い）では、前と同じく写す。
    expect(restorePasteSelection({ geometry: true }).lens).toBe(true);
    expect(restorePasteSelection({ geometry: true }).detail).toBe(true);
    // プロファイル・ルック（DEV-17）を選べるようになる前に保存した選択でも写す。
    expect(restorePasteSelection({ geometry: true }).profile).toBe(true);
    expect(toSettingGroups({ ...DEFAULT_PASTE_SELECTION, profile: false }).profile).toBe(false);
    // 外観（テクスチャ・明瞭度・かすみの除去）・効果を選べるようになる前に保存した選択でも写す。
    expect(restorePasteSelection({ geometry: true }).presence).toBe(true);
    expect(restorePasteSelection({ geometry: true, effects: false }).effects).toBe(false);
    // マスクを選べるようになる前に保存した選択では写さない（既定）。選んで保存したものは戻す。
    expect(restorePasteSelection({ geometry: true }).masks).toBe(false);
    expect(restorePasteSelection({ masks: true }).masks).toBe(true);
    expect(restorePasteSelection("x")).toEqual(DEFAULT_PASTE_SELECTION);
    expect(restorePasteSelection({ white_balance: "no" })).toEqual(DEFAULT_PASTE_SELECTION);
  });

  it("何も選ばなければ貼り付けない", () => {
    const none = {
      white_balance: false,
      basic_tone: false,
      presence: false,
      tone_curve: false,
      color: false,
      detail: false,
      lens: false,
      effects: false,
      profile: false,
      masks: false,
      transform: false,
    };
    expect(anySelected({ ...none, geometry: false })).toBe(false);
    expect(anySelected({ ...none, transform: true, geometry: false })).toBe(true);
    expect(anySelected({ ...none, masks: true, geometry: false })).toBe(true);
    expect(anySelected({ ...none, profile: true, geometry: false })).toBe(true);
    expect(anySelected({ ...none, lens: true, geometry: false })).toBe(true);
    expect(anySelected({ ...none, detail: true, geometry: false })).toBe(true);
    expect(anySelected({ ...none, effects: true, geometry: false })).toBe(true);
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

  it("処理バージョン: 貼り付け元が版 2 以上のときだけ「古い写真は更新する」を出す", () => {
    expect(canOfferProcessUpgrade(1)).toBe(false);
    expect(canOfferProcessUpgrade(2)).toBe(true);
    const o = processUpgradeOption(2);
    expect(o.label).toBe("処理バージョンが古い写真は 2 に更新する");
    expect(o.detail).toContain("処理バージョンのまま貼り付けます");
  });

  it("処理バージョン: 貼り付けの結果の知らせ（上げた数・使われない項目の警告）", () => {
    expect(describePasteReport({ count: 3, upgraded: 0, needs_upgrade: 0 }, 2)).toEqual({
      message: "3 枚に現像設定を貼り付けました",
      warning: null,
    });
    const up = describePasteReport({ count: 1200, upgraded: 2, needs_upgrade: 0 }, 2);
    expect(up.message).toBe("1,200 枚に現像設定を貼り付けました（2 枚の処理バージョンを 2 に更新）");
    const old = describePasteReport({ count: 3, upgraded: 0, needs_upgrade: 2 }, 2);
    expect(old.warning).toContain("2 枚は処理バージョンが古いため");
    expect(old.warning).toContain("処理バージョンを最新に更新…");
  });

  describe("処理バージョンが混ざった写真への貼り付けのコマンド", () => {
    let mock: MockBridge;
    beforeEach(() => {
      mock = new MockBridge();
      installBridge(mock);
    });

    it("既定は版を変えず（keep）、選んだときだけ上げる（upgrade）", async () => {
      const versions = new Map([[1, 1]]);
      const { sent } = mockPasteCommands(mock, versions);
      const settings = mockDevelopSettings();
      settings.lens.distortion = -20;
      const groups = toSettingGroups(DEFAULT_PASTE_SELECTION);
      const kept = await core.pasteSettings(settings, [1, 2], groups);
      expect(kept).toEqual({ count: 2, upgraded: 0, needs_upgrade: 1 });
      expect(sent[0]).toMatchObject({ type: "paste_settings", process_version: "keep", targets: [1, 2] });
      expect(versions.get(1)).toBe(1);
      const up = await core.pasteSettings(settings, [1, 2], groups, "upgrade");
      expect(up).toEqual({ count: 2, upgraded: 1, needs_upgrade: 0 });
      expect(sent[1]).toMatchObject({ process_version: "upgrade" });
      // まとめての更新は、古い写真だけを数える。
      versions.set(3, 1);
      expect(await core.upgradeProcessVersions([1, 3, 3])).toBe(1);
      expect(sent[2]).toEqual({ type: "upgrade_process_versions", variant_ids: [1, 3, 3] });
    });
  });

  it("処理バージョン: まとめての更新の結果の知らせ", () => {
    expect(describeProcessUpgrade(0, 4).level).toBe("info");
    expect(describeProcessUpgrade(4, 4)).toEqual({ level: "success", message: "4 枚の処理バージョンを最新に更新しました" });
    expect(describeProcessUpgrade(3, 5).message).toBe(
      "3 枚の処理バージョンを最新に更新しました（すでに最新の写真・動画 2 枚はそのまま）",
    );
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
