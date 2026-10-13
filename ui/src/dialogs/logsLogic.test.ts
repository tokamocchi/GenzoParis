// ログ・クラッシュレポート（SYS-07）とカタログの構造（ORG-07）の表示の文言と、アプリのコマンドの呼び出しのテスト。

import { beforeEach, describe, expect, it } from "vitest";

import * as app from "../api/app";
import { installBridge } from "../api/bridge";
import { MOCK_CATALOG_SCHEMA, MOCK_CRASH_REPORT, MOCK_LOG_FOLDER, MockBridge, mockLogCommands } from "../api/mock";
import {
  crashNoticeTitle,
  describeClearReport,
  describeCrashReport,
  describeLogFolder,
  describeSchema,
  LOCAL_ONLY_NOTE,
  publicViewTables,
  readOnlyCommand,
  schemaSql,
  uiErrorText,
} from "./logsLogic";

describe("ログのフォルダの表示", () => {
  it("中身の要約", () => {
    expect(describeLogFolder(MOCK_LOG_FOLDER)).toBe("ログ 2 個・クラッシュレポート 1 件、合計 5.12 MB");
    expect(describeLogFolder({ ...MOCK_LOG_FOLDER, crash_reports: [], files: [MOCK_LOG_FOLDER.files[1]], total_bytes: 2048 })).toBe(
      "ログ 1 個、合計 2.00 KB",
    );
    expect(describeLogFolder({ ...MOCK_LOG_FOLDER, exists: false })).toBe("まだログはありません");
  });

  it("消した結果", () => {
    expect(describeClearReport({ removed: 2, truncated: 1, failed: [] })).toEqual({
      level: "success",
      message: "ログを消しました（3 個）",
    });
    expect(describeClearReport({ removed: 0, truncated: 0, failed: [] }).message).toBe("消すログはありませんでした");
    const partial = describeClearReport({ removed: 1, truncated: 0, failed: ["/l/cli.log: 使用中"] });
    expect(partial.level).toBe("warning");
    expect(partial.message).toContain("1 個は消せませんでした");
    expect(partial.detail).toBe("/l/cli.log: 使用中");
  });

  it("送信しないこととファイル名が含まれることを書く", () => {
    expect(LOCAL_ONLY_NOTE).toContain("外部へは送信しません");
    expect(LOCAL_ONLY_NOTE).toContain("ファイル名");
  });
});

describe("クラッシュの知らせ", () => {
  it("1 行の表示（現地時刻と内容）", () => {
    // 日本時間（+09:00）で表示する。
    expect(describeCrashReport(MOCK_CRASH_REPORT, 540)).toBe(
      "2026-10-12 10:02:03　index out of bounds: the len is 3 but the index is 7",
    );
    expect(describeCrashReport({ ...MOCK_CRASH_REPORT, message: null, created_at: null }, 540)).toBe(
      MOCK_CRASH_REPORT.file_name,
    );
  });

  it("見出し（件数）", () => {
    expect(crashNoticeTitle([MOCK_CRASH_REPORT])).toBe("前回、GenzoParis が異常終了しました");
    expect(crashNoticeTitle([MOCK_CRASH_REPORT, MOCK_CRASH_REPORT])).toBe("前回までに GenzoParis が 2 回、異常終了しました");
  });

  it("画面のエラーの文字列", () => {
    const e = new TypeError("x is undefined");
    const t = uiErrorText(e);
    expect(t.message).toBe("TypeError: x is undefined");
    expect(t.detail).toBe(e.stack ?? null);
    expect(uiErrorText("失敗")).toEqual({ message: "失敗", detail: null });
    expect(uiErrorText({ code: 1 })).toEqual({ message: '{"code":1}', detail: null });
    expect(uiErrorText(undefined)).toEqual({ message: "undefined", detail: null });
    // 循環する値でも例外にしない。
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(uiErrorText(cyclic).message).toBe("[object Object]");
  });
});

describe("カタログの構造", () => {
  it("要約・公開ビュー・SQL・読み取り専用で開く例", () => {
    expect(describeSchema(MOCK_CATALOG_SCHEMA)).toBe("表 1 個・ビュー 2 個・索引 1 個・トリガー 0 個");
    expect(publicViewTables(MOCK_CATALOG_SCHEMA).map((t) => t.name)).toEqual(["photos_v1", "photo_keywords_v1"]);
    // 一覧にない公開ビューは飛ばす（古いカタログなど）。
    expect(publicViewTables({ ...MOCK_CATALOG_SCHEMA, public_views: ["photos_v1", "none_v1"] }).map((t) => t.name)).toEqual([
      "photos_v1",
    ]);
    const sql = schemaSql(MOCK_CATALOG_SCHEMA);
    expect(sql.startsWith(`-- GenzoParis のカタログ（スキーマの版 ${MOCK_CATALOG_SCHEMA.schema_version}）\n`)).toBe(true);
    expect(MOCK_CATALOG_SCHEMA.schema_version).toBe(8);
    expect(sql).toContain("CREATE TABLE asset (id INTEGER PRIMARY KEY, kind TEXT NOT NULL);\n\nCREATE VIEW photos_v1");
    expect(readOnlyCommand("C:\\Users\\u\\Pictures\\Genzo Paris\\catalog.db")).toBe(
      'sqlite3 -readonly "C:\\Users\\u\\Pictures\\Genzo Paris\\catalog.db"',
    );
  });
});

describe("ログのコマンド", () => {
  let mock: MockBridge;
  beforeEach(() => {
    mock = new MockBridge();
    mockLogCommands(mock);
    installBridge(mock);
  });

  it("フォルダの中身・開く・消す", async () => {
    expect(await app.logFolderInfo()).toEqual(MOCK_LOG_FOLDER);
    await app.openLogFolder();
    expect((await app.clearLogs()).truncated).toBe(1);
    expect(mock.calls.map((c) => c.command)).toEqual(["log_folder_info", "open_log_folder", "clear_logs"]);
  });

  it("画面のエラーを送る（失敗しても例外にしない）", async () => {
    app.reportUiError("TypeError: x", "at f");
    expect(mock.calls).toEqual([{ command: "report_ui_error", args: { message: "TypeError: x", detail: "at f" } }]);
    mock.responses.set("report_ui_error", new Error("アプリが応答しない"));
    app.reportUiError("もう 1 件", null);
    // 失敗は握りつぶす（ログのためにエラーを重ねない）。
    await new Promise((r) => setTimeout(r, 0));
    expect(mock.calls.length).toBe(2);
  });
});
