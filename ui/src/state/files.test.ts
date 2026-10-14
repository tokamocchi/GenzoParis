// アプリの中でのファイル操作（FILE-02）・ドライブのつながり（FILE-03）・見つからない写真（FILE-04）の画面の操作
// （state/files.ts・photoDrag・写真のメニュー・自動の確認の知らせ）のテスト（コアのコマンドは MockBridge で
// 差し替える）と、ダイアログ・左のパネルが代表的な状態で例外なく描けることの確認（react-dom/server。
// useEffect は動かない）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { dispatchCoreEventForTest } from "../api/events";
import { MOCK_MISSING_REPORT, MOCK_VOLUMES, MockBridge, mockSummary } from "../api/mock";
import type { CatalogStatus, JobInfo } from "../api/types";
import {
  FolderNameDialog,
  MoveFilesDialog,
  MoveFolderDialog,
  RelinkDialog,
  RelocateFolderDialog,
  RenameFilesDialog,
} from "../dialogs/FileDialogs";
import { clearResults, resultEntries } from "../dialogs/resultLog";
import { SHORTCUT_GROUPS } from "../dialogs/shortcutsData";
import { LibraryLeftPanel } from "../library/LibraryLeftPanel";
import { parseDropTarget } from "../library/photoDrag";
import { photoMenuEntries } from "../library/stackLogic";
import { actions, appStore } from "./app";
import { openRenameFiles, refreshVolumes, resetVolumeRefresh, VOLUME_REFRESH_MS } from "./files";
import { dropPhotos } from "./organize";
import { runPhotoMenu } from "./stacks";

let mock: MockBridge;
let sent: Record<string, unknown>[];
/** job の問い合わせへの応答（job_id ごと）。 */
let jobs: Map<number, JobInfo>;

function commands(type: string): Record<string, unknown>[] {
  return sent.filter((c) => c.type === type);
}

async function flush(): Promise<void> {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

beforeAll(() => {
  actions.installEventHandlers();
});

beforeEach(() => {
  mock = new MockBridge();
  installBridge(mock);
  sent = [];
  jobs = new Map();
  resetVolumeRefresh();
  clearResults();
  mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as Record<string, unknown> & { type: string };
    sent.push(c);
    switch (c.type) {
      case "volumes":
        return { type: "volumes", volumes: MOCK_VOLUMES };
      case "job": {
        const info = jobs.get(c.job_id as number);
        return info ? { type: "job", ...info } : { type: "ok" };
      }
      default:
        return { type: "ok" };
    }
  });
  appStore.set({
    mode: "grid",
    filter: {},
    selection: { ids: [101, 102], current: 101, currentIndex: 0, anchorIndex: 0, anchorId: 101 },
    dialog: null,
    toasts: [],
    jobs: {},
    catalog: { catalog_path: "/tmp/c.db", core_settings: { default_utc_offset_minutes: 540 } } as CatalogStatus,
  });
});

describe("写真をフォルダへ落とす・メニューから移す", () => {
  it("フォルダの受け取る先を data 属性から読む", () => {
    expect(parseDropTarget({ dropKind: "folder", dropId: "12", dropOk: "1", dropLabel: "旅行" })).toEqual({
      kind: "folder",
      id: 12,
      ok: true,
      label: "旅行",
    });
    expect(parseDropTarget({ dropKind: "folder", dropId: "12", dropOk: "0", dropLabel: "SD" })?.ok).toBe(false);
  });

  it("落とすと、すぐには移さずに移動のダイアログを開く（受け取れない先では何もしない）", async () => {
    await dropPhotos([101, 102], { kind: "folder", id: 31, ok: false, label: "SD" });
    expect(appStore.get().dialog).toBeNull();
    await dropPhotos([101, 102], { kind: "folder", id: 31, ok: true, label: "選別済み" });
    expect(appStore.get().dialog).toEqual({ kind: "move_files", variantIds: [101, 102], destFolder: 31 });
    expect(commands("move_files")).toHaveLength(0);
  });

  it("写真のメニュー: 移動・名前の変更は対象があれば、再リンクは見つからない写真だけ", async () => {
    const byAction = (entries: ReturnType<typeof photoMenuEntries>) =>
      new Map(entries.flatMap((e) => (e === "separator" ? [] : [[e.action, e] as const])));
    const ok = byAction(photoMenuEntries(2, mockSummary()));
    expect(ok.get("move_to_folder")?.enabled).toBe(true);
    expect(ok.get("rename_files")).toMatchObject({ enabled: true, keys: "F2" });
    expect(ok.get("relink")?.enabled).toBe(false);
    expect(byAction(photoMenuEntries(1, mockSummary({ file_status: "missing" }))).get("relink")?.enabled).toBe(true);
    await runPhotoMenu("move_to_folder");
    expect(appStore.get().dialog).toEqual({ kind: "move_files", variantIds: [101, 102], destFolder: null });
    await runPhotoMenu("relink");
    expect(appStore.get().dialog).toEqual({ kind: "relink", variantIds: [101, 102] });
  });

  it("F2: 操作の対象があれば名前の変更のダイアログ、なければ知らせる", () => {
    openRenameFiles();
    expect(appStore.get().dialog).toEqual({ kind: "rename_files", variantIds: [101, 102] });
    appStore.set({ dialog: null, selection: { ids: [], current: null, currentIndex: null, anchorIndex: null, anchorId: null } });
    openRenameFiles();
    expect(appStore.get().dialog).toBeNull();
    expect(appStore.get().toasts.at(-1)?.message).toBe("名前を変える写真を選んでください");
    const files = SHORTCUT_GROUPS.find((g) => g.title === "ファイル・そのほか")!;
    expect(files.items.some((i) => i.keys.includes("F2"))).toBe(true);
  });
});

describe("ドライブのつながり", () => {
  it("ウィンドウに戻るたびには確かめない（間隔の下限。force なら確かめる）", async () => {
    expect(await refreshVolumes(false, 1_000)).toEqual(MOCK_VOLUMES);
    expect(await refreshVolumes(false, 1_000 + VOLUME_REFRESH_MS - 1)).toBeNull();
    expect(await refreshVolumes(true, 1_000 + VOLUME_REFRESH_MS - 1)).toEqual(MOCK_VOLUMES);
    expect(commands("volumes")).toEqual([
      { type: "volumes", refresh: true },
      { type: "volumes", refresh: true },
    ]);
  });

  it("カタログを開いていなければ何もしない", async () => {
    appStore.set({ catalog: null });
    expect(await refreshVolumes(true)).toBeNull();
    expect(commands("volumes")).toHaveLength(0);
  });

  it("フォルダ・ファイルの場所の変更で、フォルダの一覧を読み直す", () => {
    const v0 = appStore.get().foldersVersion;
    dispatchCoreEventForTest({ type: "catalog_changed", change: "folders", variant_ids: [], all: true });
    dispatchCoreEventForTest({ type: "catalog_changed", change: "files", variant_ids: [101], all: false });
    expect(appStore.get().foldersVersion).toBe(v0 + 2);
    dispatchCoreEventForTest({ type: "catalog_changed", change: "marks", variant_ids: [101], all: false });
    expect(appStore.get().foldersVersion).toBe(v0 + 2);
  });
});

describe("自動の見つからない写真の確認", () => {
  const finished = (id: number, newly: number): JobInfo => ({
    job_id: id,
    kind: "check_missing",
    state: "succeeded",
    done: 10,
    total: 10,
    result: { type: "check_missing", ...MOCK_MISSING_REPORT, newly_missing: newly, restored: 0 },
    error: null,
  });

  it("画面が待っていない確認は、変わりがあったときだけ知らせる", async () => {
    jobs.set(5, finished(5, 0));
    dispatchCoreEventForTest({ type: "job_finished", job_id: 5, kind: "check_missing", state: "succeeded", message: null });
    await flush();
    expect(resultEntries()).toHaveLength(0);
    expect(appStore.get().toasts).toHaveLength(0);
    jobs.set(6, finished(6, 2));
    dispatchCoreEventForTest({ type: "job_finished", job_id: 6, kind: "check_missing", state: "succeeded", message: null });
    await flush();
    expect(appStore.get().toasts.at(-1)?.message).toContain("新しく見つからなくなった写真 2 件");
  });

  it("画面が待っている確認（利用者が始めたもの）は、待っている側が知らせる（重ねて知らせない）", async () => {
    jobs.set(7, finished(7, 2));
    const waiting = actions.waitJob(7);
    await flush();
    dispatchCoreEventForTest({ type: "job_finished", job_id: 7, kind: "check_missing", state: "succeeded", message: null });
    await waiting;
    await flush();
    expect(appStore.get().toasts).toHaveLength(0);
  });
});

describe("部品の描画", () => {
  const render = (el: ReturnType<typeof createElement>) => renderToString(el).replace(/<!-- -->/g, "");

  it("ダイアログと左のパネルが例外なく描ける", () => {
    expect(render(createElement(MoveFilesDialog, { variantIds: [101, 102], destFolder: null }))).toContain("フォルダへ移動（2 枚）");
    const rename = render(createElement(RenameFilesDialog, { variantIds: [101, 102] }));
    expect(rename).toContain("ファイルの名前を変える（2 枚）");
    expect(rename).toContain("テンプレート");
    expect(render(createElement(FolderNameDialog, { mode: "create", folderId: 11 }))).toContain("フォルダを作る");
    expect(render(createElement(FolderNameDialog, { mode: "rename", folderId: 11, currentName: "写真" }))).toContain(
      "フォルダの名前を変える",
    );
    expect(render(createElement(MoveFolderDialog, { folderId: 11 }))).toContain("フォルダを移す");
    expect(render(createElement(RelinkDialog, { variantIds: null }))).toContain("すべての見つからない写真");
    expect(render(createElement(RelocateFolderDialog, { folderId: 12 }))).toContain("フォルダの場所を指定し直す");
    expect(render(createElement(LibraryLeftPanel))).toContain("フォルダ");
  });
});
