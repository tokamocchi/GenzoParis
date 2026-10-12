// カタログを閉じる・再試行・復元の流れのテスト（アプリのコマンドは MockBridge で差し替える）。

import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MockBridge } from "../api/mock";
import type { CatalogStatus, ErrorInfo } from "../api/types";
import { appStore } from "../state/app";
import {
  markCatalogClosed,
  openCatalogAt,
  restoreDiscardingUnsaved,
  restoreFromBackup,
  retryOpenCatalog,
  waitJobUntilClosed,
} from "./catalogActions";
import { recordResult, resultEntries } from "./resultLog";

const info = (kind: ErrorInfo["kind"], message: string, hint: string | null = null): ErrorInfo => ({
  kind,
  message,
  user_actionable: true,
  retryable: false,
  hint,
});

const catalog = { catalog_path: "/p/catalog.db" } as unknown as CatalogStatus;

/** アプリの restore_catalog が、保存できずに閉じられなかったときに返すもの（state.rs の CloseError::info）。 */
const saveFailed = info(
  "storage",
  "現像設定を保存できません",
  "空き容量・カタログのフォルダの権限を確かめて再試行するか、保存していない現像設定を捨てて閉じてください",
);

let mock: MockBridge;

beforeEach(() => {
  mock = new MockBridge();
  installBridge(mock);
  mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as { type: string; job_id?: number };
    if (c.type === "job") {
      return { type: "job", job_id: c.job_id, kind: "export", state: "running", done: 0, total: 3, result: null, error: null };
    }
    if (c.type === "close_develop") return { type: "ok" };
    return { type: "ok" };
  });
  appStore.set({ catalog: null, catalogError: null, jobs: {}, toasts: [], dialog: null, mode: "grid" });
});

describe("カタログを閉じる", () => {
  it("ジョブの待ちを終わらせ、実行中だったジョブを知らせ、結果の記録を消す", async () => {
    appStore.set({
      catalog,
      jobs: {
        5: { jobId: 5, kind: "export", state: "running", done: 1, total: 3, message: null, label: "書き出し: 3 枚" },
        6: { jobId: 6, kind: "import", state: "succeeded", done: 3, total: 3, message: null, label: "取り込み: DCIM" },
      },
    });
    recordResult({ level: "warning", message: "書き出し", detail: "失敗: 1 枚" });
    const waiting = waitJobUntilClosed(5);
    markCatalogClosed();
    await expect(waiting).resolves.toBe("closed");
    const s = appStore.get();
    expect(s.catalog).toBeNull();
    expect(s.jobs).toEqual({});
    const toast = s.toasts.find((t) => t.message.includes("カタログを閉じたため"));
    expect(toast?.detail).toBe("・書き出し: 3 枚");
    expect(resultEntries()).toEqual([]);
  });

  it("ジョブが終われば、その結果を返す", async () => {
    mock.responses.set("core_execute", () => ({
      type: "job",
      job_id: 8,
      kind: "export",
      state: "cancelled",
      done: 0,
      total: 0,
      result: null,
      error: null,
    }));
    appStore.set({ jobs: { 8: { jobId: 8, kind: "export", state: "running", done: 0, total: 0, message: null, label: "x" } } });
    const r = await waitJobUntilClosed(8);
    expect(r).not.toBe("closed");
    expect(r !== "closed" && r.state).toBe("cancelled");
  });
});

describe("開けないときの再試行", () => {
  it("作るつもりで失敗したら、まだなければ作る", async () => {
    mock.responses.set("open_catalog", () => {
      throw info("file_access", "フォルダ /ro を作れません");
    });
    mock.responses.set("inspect_catalog_path", "missing");
    expect(await openCatalogAt("/ro/new.db", true)).toBe(false);
    expect(appStore.get().catalogError?.error.kind).toBe("file_access");
    await retryOpenCatalog("/ro/new.db");
    const opens = mock.calls.filter((c) => c.command === "open_catalog");
    expect(opens.map((c) => c.args)).toEqual([
      { path: "/ro/new.db", create: true },
      { path: "/ro/new.db", create: true },
    ]);
  });

  it("開くつもりだったら開く（作らない）", async () => {
    mock.responses.set("open_catalog", () => {
      throw info("not_found", "カタログ /x.db が見つかりません");
    });
    mock.responses.set("inspect_catalog_path", "missing");
    await openCatalogAt("/x.db", false);
    await retryOpenCatalog("/x.db");
    const opens = mock.calls.filter((c) => c.command === "open_catalog");
    expect(opens.map((c) => c.args)).toEqual([
      { path: "/x.db", create: false },
      { path: "/x.db", create: false },
    ]);
    // 開くつもりのときは、調べずに開く。
    expect(mock.calls.some((c) => c.command === "inspect_catalog_path")).toBe(false);
  });

  it("別のカタログの失敗のあとでは作らない", async () => {
    mock.responses.set("open_catalog", () => {
      throw info("file_access", "x");
    });
    mock.responses.set("inspect_catalog_path", "missing");
    await openCatalogAt("/a.db", true);
    await retryOpenCatalog("/b.db");
    const last = mock.calls.filter((c) => c.command === "open_catalog").pop();
    expect(last?.args).toEqual({ path: "/b.db", create: false });
  });
});

describe("復元", () => {
  it("保存できずに閉じられなかったことを見分ける（カタログは開いたまま）", async () => {
    appStore.set({ catalog });
    mock.responses.set("restore_catalog", () => {
      throw saveFailed;
    });
    mock.responses.set("app_status", { catalog: { catalog_path: "/p/catalog.db" } });
    const r = await restoreFromBackup("/p/catalog.db", "/p/catalog-data/backups/b.db");
    expect(r).toEqual({ ok: false, error: saveFailed, closeFailed: true });
  });

  it("使えないバックアップは、閉じられなかったとはみなさない", async () => {
    appStore.set({ catalog });
    const bad = info("catalog_corrupt", "バックアップが壊れています", "別のバックアップを選んでください");
    mock.responses.set("restore_catalog", () => {
      throw bad;
    });
    mock.responses.set("app_status", { catalog: { catalog_path: "/p/catalog.db" } });
    const r = await restoreFromBackup("/p/catalog.db", "/b.db");
    expect(r).toEqual({ ok: false, error: bad, closeFailed: false });
  });

  it("保存せずに閉じてから復元する（失敗したら元のカタログを開き直す）", async () => {
    appStore.set({ catalog });
    mock.responses.set("close_catalog", undefined);
    const bad = info("not_found", "バックアップ /b.db が見つかりません");
    mock.responses.set("restore_catalog", () => {
      throw bad;
    });
    mock.responses.set("app_status", { catalog: null });
    mock.responses.set("open_catalog", () => {
      throw info("conflict", "x");
    });
    const r = await restoreDiscardingUnsaved("/p/catalog.db", "/b.db");
    expect(r).toEqual({ ok: false, error: bad, closeFailed: false });
    const names = mock.calls.map((c) => c.command).filter((c) => c !== "core_execute");
    expect(names).toEqual(["close_catalog", "restore_catalog", "app_status", "open_catalog"]);
    expect(mock.calls.find((c) => c.command === "close_catalog")?.args).toEqual({ discardUnsaved: true });
    expect(mock.calls.find((c) => c.command === "open_catalog")?.args).toEqual({ path: "/p/catalog.db", create: false });
  });

  it("保存せずに閉じることもできなければ、閉じられなかったと返す", async () => {
    appStore.set({ catalog });
    mock.responses.set("close_catalog", () => {
      throw { error: saveFailed, still_open: true };
    });
    const r = await restoreDiscardingUnsaved("/p/catalog.db", "/b.db");
    expect(r).toEqual({ ok: false, error: saveFailed, closeFailed: true });
    expect(mock.calls.some((c) => c.command === "restore_catalog")).toBe(false);
  });
});
