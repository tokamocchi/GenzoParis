// スナップショット（DEV-28）・現像プリセット（DEV-29）の画面: 名前の規則・フォルダごとのまとめ・含める項目の説明・
// 知らせの文（純粋な計算）、コアへの送信（MockBridge。適用・作成・名前の変更・削除・読み込み）、ビフォーへの設定と、
// パネル・ダイアログが代表的な状態で例外なく描けること（react-dom/server で文字列にする）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import {
  MOCK_TONE_GROUPS,
  MockBridge,
  mockDevelopPresets,
  mockDevelopState,
  mockPresetCommands,
} from "../api/mock";
import { DEFAULT_PASTE_SELECTION } from "../dialogs/pasteLogic";
import {
  ApplyPresetDialog,
  DevelopPresetDialog,
  PresetDeleteDialog,
  PresetNameDialog,
  SnapshotNameDialog,
} from "../dialogs/PresetDialogs";
import { appStore } from "../state/app";
import { developStore } from "../state/develop";
import {
  applyDevelopPreset,
  createSnapshot,
  deleteSnapshot,
  presetsStore,
  reloadPresets,
  renameSnapshot,
} from "../state/presets";
import { DevelopLeftPanel } from "./DevelopLeftPanel";
import {
  defaultSnapshotName,
  describeGroups,
  describePresetApply,
  describePresetImport,
  groupPresets,
  presetApplyTargets,
  presetFolders,
  presetTooltip,
  selectionOf,
  validateFolderName,
  validatePresetName,
  validateSnapshotName,
} from "./presetLogic";
import { showAsBefore, takeWantedBefore, viewStore } from "./viewState";

describe("名前・まとめ・説明（純粋な計算）", () => {
  it("名前の規則（プリセットは 100 文字・スナップショットは 200 文字・制御文字なし・フォルダは空でもよい）", () => {
    expect(validatePresetName(" 明るく ")).toBeNull();
    expect(validatePresetName("  ")).toBe("名前を入れてください");
    expect(validatePresetName("あ".repeat(100))).toBeNull();
    expect(validatePresetName("あ".repeat(101))).toBe("名前は 100 文字までです");
    expect(validatePresetName("a\tb")).toContain("改行・タブ");
    expect(validateFolderName("")).toBeNull();
    expect(validateFolderName("a\nb")).toContain("改行・タブ");
    expect(validateSnapshotName("あ".repeat(200))).toBeNull();
    expect(validateSnapshotName("あ".repeat(201))).toBe("名前は 200 文字までです");
    // サロゲートペア（絵文字）は 1 文字に数える（コアの Unicode のスカラー値の数と同じ）。
    expect(validatePresetName("😀".repeat(100))).toBeNull();
  });

  it("スナップショットの既定の名前は今の日時", () => {
    expect(defaultSnapshotName(new Date(2026, 9, 14, 9, 5, 3))).toBe("2026-10-14 09:05:03");
  });

  it("フォルダごとにまとめる（フォルダなしを先に）", () => {
    const list = mockDevelopPresets();
    const groups = groupPresets(list);
    expect(groups.map((g) => [g.folder, g.presets.map((p) => p.name)])).toEqual([
      ["", ["明るく"]],
      ["ルック", ["フィルム風", "新しい形式"]],
    ]);
    expect(presetFolders(list)).toEqual(["ルック"]);
    expect(groupPresets([])).toEqual([]);
  });

  it("含める項目の説明と、ダイアログの初めの選択", () => {
    expect(describeGroups(MOCK_TONE_GROUPS)).toBe("基本補正・外観");
    expect(describeGroups({ ...MOCK_TONE_GROUPS, profile: undefined })).toBe("基本補正・外観・プロファイル・ルック");
    const all = Object.fromEntries(Object.keys(DEFAULT_PASTE_SELECTION).map((k) => [k, true])) as typeof DEFAULT_PASTE_SELECTION;
    expect(describeGroups(all)).toBe("すべての項目");
    expect(selectionOf(MOCK_TONE_GROUPS, DEFAULT_PASTE_SELECTION)).toEqual({ ...MOCK_TONE_GROUPS });
    expect(selectionOf(undefined, DEFAULT_PASTE_SELECTION)).toEqual(DEFAULT_PASTE_SELECTION);
    const [ok, , broken] = mockDevelopPresets();
    expect(presetTooltip(ok)).toContain("含める項目: 基本補正・外観");
    expect(presetTooltip(broken)).toMatch(/^使えません: 新しい版のアプリ/);
  });

  it("適用先の数と動画の知らせ（動画には適用しない）", () => {
    expect(presetApplyTargets([1, 2, 2], 0)).toEqual({ photos: 2, videos: 0, note: null });
    const mixed = presetApplyTargets([1, 2, 3], 1);
    expect(mixed.photos).toBe(2);
    expect(mixed.note).toBe("選んだうち動画 1 件には適用しません（写真 2 枚に適用します）。");
    const onlyVideos = presetApplyTargets([5, 6], 2);
    expect(onlyVideos.photos).toBe(0);
    expect(onlyVideos.note).toContain("すべて動画です");
    // 数え違い（重なり）で負にしない。
    expect(presetApplyTargets([1], 3).photos).toBe(0);
  });

  it("適用・読み込みの知らせ", () => {
    expect(describePresetApply({ count: 1, upgraded: 0, needs_upgrade: 0 }, "明るく")).toEqual({
      message: "プリセット「明るく」を適用しました",
      warning: null,
    });
    const many = describePresetApply({ count: 3, upgraded: 1, needs_upgrade: 2 }, "明るく");
    expect(many.message).toBe("プリセット「明るく」を3 枚に適用しました（1 枚の処理バージョンを更新）");
    expect(many.warning).toContain("2 枚は処理バージョンが古いため");
    // 動画を除いた数（コアの skipped_videos）。動画だけなら適用した写真はない。
    expect(describePresetApply({ count: 2, upgraded: 0, needs_upgrade: 0, skipped_videos: 1 }, "明るく").message).toBe(
      "プリセット「明るく」を2 枚に適用しました（動画 1 件は除きました）",
    );
    expect(describePresetApply({ count: 0, upgraded: 0, needs_upgrade: 0, skipped_videos: 2 }, "明るく").message).toBe(
      "プリセット「明るく」を適用した写真はありません（動画 2 件は除きました）",
    );
    const [p] = mockDevelopPresets();
    expect(describePresetImport({ imported: [p], failed: [] })).toEqual({
      level: "success",
      message: "1 件のプリセットを読み込みました",
      detail: "読み込み: 明るく",
    });
    const mixed = describePresetImport({ imported: [p], failed: [{ path: "/x/a.txt", message: "読めません" }] });
    expect(mixed.level).toBe("warning");
    expect(mixed.detail).toContain("読み込めません: /x/a.txt（読めません）");
    expect(describePresetImport({ imported: [], failed: [{ path: "/x", message: "x" }] }).level).toBe("error");
  });
});

describe("コアへの送信", () => {
  let mock: MockBridge;
  let sent: Record<string, unknown>[];

  beforeEach(() => {
    mock = new MockBridge();
    installBridge(mock);
    mock.responses.set("core_execute", () => ({ type: "ok" }));
    ({ sent } = mockPresetCommands(mock));
    presetsStore.set({ develop: null, export: null, dir: null, errors: {}, snapshotsVersion: 0 });
    appStore.set({ toasts: [] });
    developStore.set({ variantId: null, state: null, settings: null });
  });

  it("一覧を読む（フォルダの場所も）", async () => {
    const list = await reloadPresets("develop");
    expect(list?.length).toBe(3);
    expect(presetsStore.get().develop?.map((p) => p.name)).toEqual(["明るく", "フィルム風", "新しい形式"]);
    expect(presetsStore.get().dir).toContain("presets");
    expect(sent[0]).toEqual({ type: "presets", kind: "develop" });
  });

  it("適用: 選んだ写真へ送り、結果を知らせる（重なりは 1 つ）", async () => {
    const [p] = mockDevelopPresets();
    await applyDevelopPreset(p, [3, 4, 3], "upgrade");
    expect(sent.find((c) => c.type === "apply_develop_preset")).toEqual({
      type: "apply_develop_preset",
      preset_id: p.id,
      targets: [3, 4],
      process_version: "upgrade",
    });
    const toasts = appStore.get().toasts;
    expect(toasts[toasts.length - 1].message).toBe("プリセット「明るく」を2 枚に適用しました");
  });

  it("適用: 写真と動画を混ぜて選ぶと、動画を除いて適用し、除いた数を知らせる", async () => {
    ({ sent } = mockPresetCommands(mock, { videos: [9] }));
    const [p] = mockDevelopPresets();
    await applyDevelopPreset(p, [3, 9, 4]);
    expect(sent.find((c) => c.type === "apply_develop_preset")?.targets).toEqual([3, 9, 4]);
    const toasts = appStore.get().toasts;
    expect(toasts[toasts.length - 1].message).toBe("プリセット「明るく」を2 枚に適用しました（動画 1 件は除きました）");
  });

  it("適用の失敗は投げる（ダイアログに出す）", async () => {
    mock.responses.set("core_execute", () => new Error("プリセットのファイルを読めません"));
    const [p] = mockDevelopPresets();
    await expect(applyDevelopPreset(p, [1])).rejects.toThrow("読めません");
  });

  it("スナップショット: 作る・名前の変更・削除で一覧の読み直しの合図が増える", async () => {
    const s = await createSnapshot(1, "途中", 42);
    expect(s.name).toBe("途中");
    expect(sent.find((c) => c.type === "create_snapshot")).toEqual({
      type: "create_snapshot",
      variant_id: 1,
      name: "途中",
      source: { kind: "history", entry_id: 42 },
    });
    await createSnapshot(1, "今");
    expect(sent.filter((c) => c.type === "create_snapshot")[1].source).toEqual({ kind: "current" });
    await renameSnapshot(s.id, "途中 2");
    await deleteSnapshot(s.id);
    expect(presetsStore.get().snapshotsVersion).toBe(4);
    expect(sent.map((c) => c.type)).toContain("delete_snapshot");
  });
});

describe("ビフォーに設定", () => {
  afterEach(() => {
    viewStore.set({ compare: "off" });
    takeWantedBefore();
  });

  it("表示がオフなら左右に並べて、表示を始めるときに選んだ出どころを使う", () => {
    viewStore.set({ compare: "off" });
    showAsBefore({ kind: "snapshot", snapshot_id: 11 });
    expect(viewStore.get().compare).toBe("side_by_side");
    expect(takeWantedBefore()).toEqual({ kind: "snapshot", snapshot_id: 11 });
    // 1 回だけ。
    expect(takeWantedBefore()).toBeNull();
  });
});

describe("パネル・ダイアログを描く", () => {
  beforeEach(() => {
    const data = new Map<string, string>();
    (globalThis as { localStorage?: unknown }).localStorage = {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, String(v)),
      removeItem: (k: string) => void data.delete(k),
    };
    presetsStore.set({ develop: mockDevelopPresets(), export: null, dir: null, errors: {}, snapshotsVersion: 0 });
  });
  afterEach(() => {
    delete (globalThis as { localStorage?: unknown }).localStorage;
    developStore.set({ variantId: null, state: null, settings: null });
    presetsStore.set({ develop: null, export: null, dir: null, errors: {}, snapshotsVersion: 0 });
  });

  const render = (el: ReturnType<typeof createElement>) => renderToString(el).replace(/<!-- -->/g, "");

  it("左のパネル: プリセット（フォルダ・使えないもの）・スナップショット・履歴の欄", () => {
    const state = mockDevelopState();
    appStore.set((s) => ({ selection: { ...s.selection, current: 1, currentIndex: 0, ids: [1] }, dialog: null }));
    developStore.set({ variantId: 1, state, settings: state.settings, opening: false, error: null });
    const html = render(createElement(DevelopLeftPanel));
    expect(html).toContain("プリセット");
    expect(html).toContain("明るく");
    expect(html).toContain("ルック");
    expect(html).toContain("新しい形式");
    expect(html).toContain("unusable");
    expect(html).toContain("スナップショット");
    expect(html).toContain("履歴");
  });

  it("プリセットがなければ案内を出す", () => {
    presetsStore.set({ develop: [] });
    const state = mockDevelopState();
    developStore.set({ variantId: 1, state, settings: state.settings, opening: false, error: null });
    appStore.set((s) => ({ selection: { ...s.selection, current: 1, currentIndex: 0, ids: [1] } }));
    expect(render(createElement(DevelopLeftPanel))).toContain("プリセットはまだありません");
  });

  it("ダイアログ", () => {
    const create = render(createElement(DevelopPresetDialog, { mode: "create", variantId: 1 }));
    expect(create).toContain("現像プリセットを作る");
    expect(create).toContain("含める項目");
    expect(create).toContain("ほかのカタログでも使えます");
    const [p] = mockDevelopPresets();
    const update = render(createElement(DevelopPresetDialog, { mode: "update", variantId: 1, presetId: p.id }));
    expect(update).toContain("今の設定で更新");
    const apply = render(createElement(ApplyPresetDialog, { targets: [1, 2, 2], presetId: p.id }));
    expect(apply).toContain("現像プリセットを適用（2 枚）");
    expect(apply).toContain("基本補正・外観");
    // 使えないプリセットは選べない。
    expect(apply).not.toContain("新しい形式");
    const snap = render(createElement(SnapshotNameDialog, { mode: "create", variantId: 1 }));
    expect(snap).toContain("スナップショットを作る");
    const fromHistory = render(
      createElement(SnapshotNameDialog, { mode: "create", variantId: 1, entryId: 5, currentName: "露光量 +0.50" }),
    );
    expect(fromHistory).toContain("履歴のこの段階");
    expect(fromHistory).toContain("露光量 +0.50");
    const rename = render(
      createElement(PresetNameDialog, {
        mode: "rename_folder",
        presetKind: "develop",
        folder: "ルック",
        currentName: "ルック",
      }),
    );
    expect(rename).toContain("フォルダの名前を変える");
    const del = render(createElement(PresetDeleteDialog, { presetKind: "export", presetId: "x", name: "Web 用" }));
    expect(del).toContain("書き出しプリセットを削除");
    expect(del).toContain("Web 用");
  });
});
