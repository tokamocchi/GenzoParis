// カメラプロファイル・ルック（DEV-17）の画面: 操作の可否（RAW・処理バージョン）、選択肢（その機種用・ほかの機種用・
// 見つからない ID）、取り込みの結果の知らせ、一覧・取り込み・除くの送信（MockBridge）と、パネル・ダイアログが
// 代表的な状態で例外なく描けること（react-dom/server で文字列にする）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import {
  MockBridge,
  mockDevelopSettings,
  mockDevelopState,
  mockProfileCommands,
  mockProfileEntries,
  mockProfileEntry,
  mockSourceInfo,
} from "../api/mock";
import type { DevelopState, ProfileEntry } from "../api/types";
import { ProfileBrowserDialog } from "../dialogs/ProfileBrowserDialog";
import { appStore } from "../state/app";
import { developStore } from "../state/develop";
import { importProfilePaths, profilesStore, refreshProfiles, removeProfileEntry } from "../state/profiles";
import { DevelopPanel } from "./DevelopPanel";
import {
  cameraChoices,
  defaultProfile,
  describeProfileImport,
  fullProfile,
  hasReplaceableFailures,
  lookChoices,
  profileControls,
  profileIsDefault,
  selectsOtherCamera,
  withProfile,
} from "./profileLogic";
import { resetAll } from "./settingsLogic";

/** 機種「SONY ILCE-7M4」で読んだ一覧（その機種用・ほかの機種用・ルック）。 */
function entriesFor7M4(): ProfileEntry[] {
  return mockProfileEntries().map((e) => ({
    ...e,
    matches_camera: e.kind === "camera" ? e.camera_model === "Sony ILCE-7M4" : null,
  }));
}

describe("カメラプロファイル・ルックの設定", () => {
  it("既定は標準の色・ルックなし・適用量 100。欠けた項目は既定で補う", () => {
    expect(defaultProfile()).toEqual({ camera: null, look: null, look_amount: 100 });
    expect(fullProfile(undefined)).toEqual(defaultProfile());
    expect(fullProfile({ look: "film" })).toEqual({ camera: null, look: "film", look_amount: 100 });
    expect(profileIsDefault(undefined)).toBe(true);
    expect(profileIsDefault({ look_amount: 50 })).toBe(false);
    const s = withProfile(mockDevelopSettings(), { camera: "ILCE-7M4 標準" });
    expect(s.profile).toEqual({ camera: "ILCE-7M4 標準", look: null, look_amount: 100 });
    // すべてリセットで標準の色・ルックなしに戻る。
    expect(resetAll(withProfile(s, { look: "film", look_amount: 30 })).profile).toEqual(defaultProfile());
  });

  it("カメラプロファイルは RAW だけ、どちらも処理バージョン 2 から。適用量はルックを選んだときだけ", () => {
    const v2 = mockDevelopSettings();
    const raw = mockSourceInfo();
    expect(profileControls(v2, raw, false)).toEqual({
      needsUpgrade: false,
      notRaw: false,
      camera: true,
      look: true,
      lookAmount: false,
    });
    expect(profileControls(withProfile(v2, { look: "film" }), raw, false).lookAmount).toBe(true);
    const jpeg = profileControls(v2, mockSourceInfo({ is_raw: false }), false);
    expect(jpeg.notRaw).toBe(true);
    expect(jpeg.camera).toBe(false);
    expect(jpeg.look).toBe(true);
    const v1 = profileControls(mockDevelopSettings({ process_version: 1 }), raw, false);
    expect(v1.needsUpgrade).toBe(true);
    expect(v1.camera || v1.look).toBe(false);
    expect(profileControls(v2, raw, true).look).toBe(false);
  });

  it("選択肢: 標準とその機種用。ほかの機種用は「すべて表示」で。見つからない ID も残す", () => {
    const list = entriesFor7M4();
    const mine = cameraChoices(list, null, false);
    expect(mine.map((c) => c.id)).toEqual([null, "ILCE-7M4 標準"]);
    expect(mine[0].label).toContain("標準");
    const all = cameraChoices(list, null, true);
    expect(all.map((c) => c.id)).toEqual([null, "ILCE-7M4 標準", "EOS R5 風景"]);
    expect(all[2].label).toContain("Canon EOS R5 用");
    const missing = cameraChoices(list, "消したプロファイル", false);
    expect(missing[missing.length - 1]).toEqual({ id: "消したプロファイル", label: "消したプロファイル（見つかりません）" });
    // ほかの機種用を選んでいれば、すべて表示にしなくても選択肢に出す。
    expect(selectsOtherCamera(list, "EOS R5 風景")).toBe(true);
    expect(cameraChoices(list, "EOS R5 風景", false).some((c) => c.id === "EOS R5 風景")).toBe(true);
    expect(selectsOtherCamera(list, "ILCE-7M4 標準")).toBe(false);
    // 読めない写しは選べない。
    const broken = cameraChoices([{ ...list[0], usable: false }], null, false);
    expect(broken[1].disabled).toBe(true);
    expect(broken[1].label).toContain("読めません");
    const looks = lookChoices(list, null);
    expect(looks.map((c) => c.id)).toEqual([null, "film"]);
    expect(looks[1].label).toBe("フィルム風（film）");
    expect(lookChoices(list, "ない").pop()?.label).toBe("ない（見つかりません）");
  });

  it("取り込みの結果の知らせ", () => {
    const e = mockProfileEntry();
    const ok = describeProfileImport({ imported: [e], replaced: [], repaired: [], unchanged: [], failed: [] });
    expect(ok).toEqual({ level: "success", message: "1 個を取り込みました", detail: undefined });
    const mixed = describeProfileImport({
      imported: [],
      replaced: [e],
      repaired: [],
      unchanged: [e],
      failed: [{ path: "/x/a.icc", message: "対応していない形式です" }],
    });
    expect(mixed.level).toBe("warning");
    expect(mixed.message).toBe("1 個を置き換えました・1 個は同じ内容のものがありました・1 個は取り込めませんでした");
    expect(mixed.detail).toContain("/x/a.icc: 対応していない形式です");
    expect(mixed.detail).toContain("内容が違う");
    expect(describeProfileImport({ imported: [], replaced: [], repaired: [], unchanged: [e], failed: [] }).level).toBe("info");
    // 消えた・壊れた写しを書き直した（取り込み直しで直る）。
    const repaired = describeProfileImport({ imported: [], replaced: [], repaired: [e], unchanged: [], failed: [] });
    expect(repaired).toEqual({
      level: "success",
      message: "1 個は消えた・壊れた写しを書き直しました",
      detail: undefined,
    });
    expect(
      hasReplaceableFailures({
        imported: [],
        replaced: [],
        repaired: [],
        unchanged: [],
        failed: [{ path: "a.dcp", message: "同じ ID（a）のカメラプロファイルがすでにあります（内容が違います）" }],
      }),
    ).toBe(true);
  });
});

describe("一覧・取り込み・除く（コアへの送信）", () => {
  let mock: MockBridge;
  let sent: Record<string, unknown>[];

  beforeEach(() => {
    mock = new MockBridge();
    installBridge(mock);
    mock.responses.set("core_execute", () => ({ type: "ok" }));
    ({ sent } = mockProfileCommands(mock));
    profilesStore.set({ entries: null, camera: null, busy: false, error: null });
    appStore.set({ toasts: [] });
  });

  it("機種を渡して一覧を読み、その機種用かが入る", async () => {
    await refreshProfiles("SONY ILCE-7M4");
    expect(sent[0]).toEqual({ type: "list_profiles", camera: "SONY ILCE-7M4" });
    const entries = profilesStore.get().entries ?? [];
    expect(entries.find((e) => e.id === "ILCE-7M4 標準")?.matches_camera).toBe(true);
    expect(entries.find((e) => e.id === "EOS R5 風景")?.matches_camera).toBe(false);
    expect(entries.find((e) => e.id === "film")?.matches_camera).toBeNull();
  });

  it("取り込み: 結果を知らせて読み直す。同じ名前は置き換えを選んだときだけ", async () => {
    await refreshProfiles("SONY ILCE-7M4");
    const added = await importProfilePaths(["/x/新しい.dcp", "/x/memo.txt"], false);
    expect(added.map((e) => e.id)).toEqual(["新しい"]);
    expect(sent.find((c) => c.type === "import_profiles")).toEqual({
      type: "import_profiles",
      paths: ["/x/新しい.dcp", "/x/memo.txt"],
      replace: false,
    });
    const toasts = appStore.get().toasts;
    expect(toasts[toasts.length - 1].message).toBe("1 個を取り込みました・1 個は取り込めませんでした");
    expect(profilesStore.get().entries?.some((e) => e.id === "新しい")).toBe(true);
    // 同じ名前（大文字・小文字は区別しない）: 置き換えなければ失敗にして案内する。
    await importProfilePaths(["/y/FILM.cube"], false);
    const last = appStore.get().toasts[appStore.get().toasts.length - 1];
    expect(last.detail).toContain("置き換える");
    const replaced = await importProfilePaths(["/y/FILM.cube"], true);
    expect(replaced.map((e) => e.id)).toEqual(["film"]);
  });

  it("除く: 送って読み直す", async () => {
    await refreshProfiles(null);
    expect(await removeProfileEntry("look", "film")).toBe(true);
    expect(sent.find((c) => c.type === "remove_profile")).toEqual({ type: "remove_profile", kind: "look", id: "film" });
    expect(profilesStore.get().entries?.some((e) => e.id === "film")).toBe(false);
    expect(await removeProfileEntry("look", "film")).toBe(false);
  });
});

describe("プロファイルのパネル・ダイアログを描く", () => {
  beforeEach(() => {
    const data = new Map<string, string>([["genzo.section.develop.profile", "1"]]);
    (globalThis as { localStorage?: unknown }).localStorage = {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, String(v)),
      removeItem: (k: string) => void data.delete(k),
    };
    profilesStore.set({ entries: entriesFor7M4(), camera: "SONY ILCE-7M4", busy: false, error: null });
  });
  afterEach(() => {
    delete (globalThis as { localStorage?: unknown }).localStorage;
    developStore.set({ variantId: null, state: null, settings: null });
    profilesStore.set({ entries: null, camera: null, busy: false, error: null });
  });

  function render(state: DevelopState): string {
    appStore.set((s) => ({ selection: { ...s.selection, current: 1, currentIndex: 0 }, dialog: null }));
    developStore.set({ variantId: 1, state, settings: state.settings, opening: false, error: null });
    return renderToString(createElement(DevelopPanel)).replace(/<!-- -->/g, "");
  }

  it("RAW: その機種用のプロファイルとルック・適用量・取り込み・管理を出す", () => {
    const html = render(
      mockDevelopState({ settings: mockDevelopSettings({ profile: { camera: null, look: "film", look_amount: 60 } }) }),
    );
    expect(html).toContain("プロファイル");
    expect(html).toContain("標準（アプリのカメラ行列）");
    expect(html).toContain("ILCE-7M4 標準");
    expect(html).not.toContain("EOS R5 風景");
    expect(html).toContain("ほかの機種用も表示");
    expect(html).toContain("フィルム風（film）");
    expect(html).toContain("適用量");
    expect(html).toContain("取り込む…");
    expect(html).toContain("管理…");
    expect(html).not.toContain("処理バージョンを 2 に更新");
  });

  it("RAW 以外ではカメラプロファイルを使わないと知らせる。版 1 では更新を案内する", () => {
    const jpeg = render(mockDevelopState({ source: mockSourceInfo({ is_raw: false, camera: null }) }));
    expect(jpeg).toContain("RAW 以外の写真では、カメラプロファイルは使われません");
    const v1 = render(mockDevelopState({ settings: mockDevelopSettings({ process_version: 1 }) }));
    expect(v1).toContain("カメラプロファイル・ルックは使われません");
    expect(v1).toContain("処理バージョンを 2 に更新");
  });

  it("見つからない ID を選んでいれば、そのまま出す", () => {
    const html = render(
      mockDevelopState({
        settings: mockDevelopSettings({ profile: { camera: "消した", look: null, look_amount: 100 } }),
      }),
    );
    expect(html).toContain("消した（見つかりません）");
  });

  it("一覧のダイアログ: 種類ごとに並べ、説明を出す", () => {
    const html = renderToString(createElement(ProfileBrowserDialog)).replace(/<!-- -->/g, "");
    expect(html).toContain("カメラプロファイル・ルック");
    expect(html).toContain("ILCE-7M4 標準");
    expect(html).toContain("EOS R5 風景");
    expect(html).toContain("フィルム風（film）");
    expect(html).toContain("同じ名前のものを置き換える");
    expect(html).toContain("プロファイルは付属していません");
  });
});
