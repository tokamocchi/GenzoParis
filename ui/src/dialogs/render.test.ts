// 画面の部品が、代表的な状態で例外なく描けることの確認（DOM を使わず、react-dom/server で文字列にする。
// useEffect は動かないので、読み込みの後の表示は確かめない）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import { MOCK_CRASH_REPORT, MOCK_LOG_STATUS } from "../api/mock";
import type { AppStatus, CatalogStatus, DevelopSettings, ErrorInfo } from "../api/types";
import { appStore } from "../state/app";
import { CatalogErrorScreen } from "./CatalogErrorScreen";
import { CloseFailedDialog } from "./CloseFailedDialog";
import { CrashReportsDialog } from "./CrashReportsDialog";
import { DeleteDialog } from "./DeleteDialog";
import { Dialogs } from "./Dialogs";
import { ExportDialog } from "./ExportDialog";
import { ImportDialog } from "./ImportDialog";
import { offerImportFolders } from "./importDrop";
import { JobsIndicator } from "./JobsIndicator";
import { PasteSettingsDialog } from "./PasteSettingsDialog";
import { clearResults, recordResult } from "./resultLog";
import { SettingsDialog } from "./SettingsDialog";
import { SettingsSchema } from "./SettingsSchema";
import { ShortcutsDialog } from "./ShortcutsDialog";
import { oneDriveNotice, WelcomeScreen } from "./WelcomeScreen";

const status: AppStatus = {
  version: "0.1.0",
  platform: "linux",
  settings: {
    version: 1,
    last_catalog: null,
    recent_catalogs: ["/home/u/Pictures/GenzoParis/catalog.db", "/mnt/ext/old.db"],
    ffmpeg_dir: null,
    gpu: "auto",
    backup_on_close: false,
  },
  notice: null,
  default_catalog: "/home/u/Pictures/GenzoParis/catalog.db",
  default_catalog_in_onedrive: false,
  catalog: null,
  tools: { ffmpeg: null, ffprobe: null },
  logs: MOCK_LOG_STATUS,
  crash_reports: [],
};

const catalog: CatalogStatus = {
  catalog_path: "/home/u/Pictures/GenzoParis/catalog.db",
  data_dir: "/home/u/Pictures/GenzoParis/catalog-data",
  startup: { previous_shutdown: "unclean", migrated_from: null, recovered_file_ops: 0, backup: null },
  warnings: [],
  stats: { schema_version: 3, assets: 1200, variants: 1250, files: 1300, history_entries: 40 },
  capabilities: { raw_decoder: "LibRaw 0.21", worker_version: "0.1.0", gpu: null },
  worker_error: null,
  display: { assumed_srgb: false, reason: null, description: "Display P3" },
  tools: { ffmpeg: null, ffprobe: null },
  core_settings: {
    preview_cache_dir: "/home/u/Pictures/GenzoParis/catalog-data/previews",
    preview_cache_bytes: 20_000_000_000,
    default_export_color_space: "srgb",
    default_utc_offset_minutes: 540,
  },
};

const error = (kind: ErrorInfo["kind"]): ErrorInfo => ({
  kind,
  message: "カタログを開けません",
  user_actionable: true,
  retryable: false,
  hint: "案内",
});

/** 文字列にする（React が文字の区切りに入れる <!-- --> は除く）。 */
function render(el: ReturnType<typeof createElement>): string {
  return renderToString(el).replace(/<!-- -->/g, "");
}

beforeEach(() => {
  appStore.set({ status, catalog: null, catalogError: null, dialog: null, jobs: {}, clipboard: null });
});

describe("起動の画面", () => {
  it("カタログの選択", () => {
    const html = render(createElement(WelcomeScreen));
    expect(html).toContain("カタログを選んでください");
    expect(html).toContain("最近開いたカタログ");
    expect(html).toContain("old.db");
    expect(html).not.toContain("OneDrive");
  });

  it("既定の場所が OneDrive で同期されるフォルダの中なら注意を出す", () => {
    appStore.set({
      status: {
        ...status,
        platform: "windows",
        default_catalog: "C:\\Users\\u\\OneDrive\\Pictures\\GenzoParis\\catalog.db",
        default_catalog_in_onedrive: true,
      },
    });
    const html = render(createElement(WelcomeScreen));
    expect(html).toContain(oneDriveNotice(false));
    expect(html).toContain("最大 20 GB");
    // 案内するボタンの見出しは、画面にある文言と同じ。
    expect(html).toContain("別の場所に新しいカタログを作る");
    expect(oneDriveNotice(false)).toContain("「別の場所に新しいカタログを作る」");
    // 既にカタログがあるときは、作る場所の案内はしない。
    expect(oneDriveNotice(true)).toContain("最大 20 GB");
    expect(oneDriveNotice(true)).not.toContain("おすすめします");
  });

  it("開けないとき（理由ごと）", () => {
    for (const kind of ["catalog_corrupt", "conflict", "incompatible_version", "not_found", "storage"] as const) {
      appStore.set({ catalogError: { path: "/p/catalog.db", error: error(kind) } });
      const html = render(createElement(CatalogErrorScreen));
      expect(html).toContain("再試行");
      expect(html).toContain("別のカタログを開く");
    }
  });
});

describe("ダイアログ", () => {
  beforeEach(() => {
    appStore.set({ catalog });
  });

  it("取り込み（ffprobe がないときの注意）", () => {
    const html = render(createElement(ImportDialog));
    expect(html).toContain("サブフォルダも含める");
    expect(html).toContain("ffprobe");
  });

  it("取り込み（点を含む名前のフォルダに、名前だけで注意を出さない）", () => {
    // 種類はアプリの path_kinds で確かめてから注意を出す（useEffect は動かないので、ここでは確かめる前）。
    offerImportFolders(["/p/DCIM", "/p/2024.Kyoto"]);
    const html = render(createElement(ImportDialog));
    expect(html).toContain("フォルダ（2 個）");
    expect(html).toContain("2024.Kyoto");
    expect(html).not.toContain("フォルダではありません");
    expect(html).toContain("除く");
  });

  it("書き出し", () => {
    const html = render(createElement(ExportDialog, { variantIds: [1, 2, 3] }));
    expect(html).toContain("書き出し（3 枚）");
    expect(html).toContain("位置情報（GPS）を削除する");
  });

  it("削除", () => {
    const html = render(createElement(DeleteDialog, { deleteKind: "trash", variantIds: [1] }));
    expect(html).toContain("ゴミ箱に移す");
  });

  it("貼り付け（コピーがないとき・あるとき）", () => {
    expect(render(createElement(PasteSettingsDialog, { targets: [1, 2] }))).toContain("先に現像設定をコピー");
    appStore.set({ clipboard: { settings: {} as DevelopSettings, from: 9 } });
    const html = render(createElement(PasteSettingsDialog, { targets: [1, 2, 9] }));
    expect(html).toContain("切り抜きと角度");
    expect(html).toContain("コピー元の写真は除きます");
  });

  it("設定（3 つのタブ）", () => {
    const html = render(createElement(SettingsDialog, { tab: "backup" }));
    expect(html).toContain("FFmpeg");
    expect(html).toContain("プレビューのキャッシュ");
    expect(html).toContain("UTC+09:00");
    expect(html).toContain("詳細チェック");
    expect(html).toContain("前回は正常に終了しませんでした");
  });

  it("設定の一般: ログ・クラッシュレポート（SYS-07）", () => {
    const html = render(createElement(SettingsDialog, { tab: "general" }));
    expect(html).toContain("ログ・クラッシュレポート");
    expect(html).toContain("外部へは送信しません");
    expect(html).toContain(MOCK_LOG_STATUS.dir);
    expect(html).toContain("ログのフォルダを開く");
    expect(html).toContain("ログを消す");
    // ログのフォルダが分からない・ファイルに書かない・初期化の問題。
    appStore.set({
      status: { ...status, logs: { dir: null, file: null, file_enabled: false, problems: ["ログのファイルを作れません"] } },
    });
    const off = render(createElement(SettingsDialog, { tab: "general" }));
    expect(off).toContain("ログのフォルダが分かりません");
    expect(off).toContain("GENZO_LOG_FILE=off");
    expect(off).toContain("ログのファイルを作れません");
  });

  it("設定のカタログ: カタログの構造（ORG-07）", () => {
    const html = render(createElement(SettingsDialog, { tab: "catalog" }));
    expect(html).toContain("カタログの構造（SQL で直接読む）");
    expect(html).toContain("photos_v1");
    expect(html).toContain("読み取り専用");
    expect(html).toContain(`sqlite3 -readonly &quot;${catalog.catalog_path}&quot;`);
    expect(html).toContain("今のスキーマの版は 3 です");
    expect(render(createElement(SettingsSchema, { catalogPath: "/c.db", schemaVersion: null }))).toContain("構造を表示");
  });

  it("前回の異常終了の知らせ", () => {
    const html = render(createElement(CrashReportsDialog, { reports: [MOCK_CRASH_REPORT] }));
    expect(html).toContain("前回、GenzoParis が異常終了しました");
    expect(html).toContain("index out of bounds");
    expect(html).toContain("外部へは送信しません");
    expect(html).toContain("ログのフォルダを開く");
    const many = render(createElement(CrashReportsDialog, { reports: Array(7).fill(MOCK_CRASH_REPORT) }));
    expect(many).toContain("7 回、異常終了しました");
    expect(many).toContain("ほか 2 件");
    appStore.set({ dialog: { kind: "crash_reports", reports: [MOCK_CRASH_REPORT] } });
    expect(render(createElement(Dialogs))).toContain("前回、GenzoParis が異常終了しました");
  });

  it("ショートカット", () => {
    const html = render(createElement(ShortcutsDialog));
    expect(html).toContain("ショートカット");
    expect(html).toContain("ルーペで開く");
    expect(html).toContain("撮影情報の表示の切り替え（情報 1 → 情報 2 → なし。ルーペ・現像。項目は設定の「表示」）");
    expect(html).toContain("ビフォー / アフター: 左右に並べる");
    expect(html).toContain("選択を表示中の 1 枚だけにする");
  });

  it("閉じられなかったとき", () => {
    const failure = { error: error("storage"), still_open: true };
    const html = render(createElement(CloseFailedDialog, { failure, quitting: true }));
    expect(html).toContain("保存せずに終了");
    expect(html).toContain("再試行");
    const closed = render(createElement(CloseFailedDialog, { failure: { ...failure, still_open: false }, quitting: false }));
    expect(closed).toContain("カタログは閉じました");
  });

  it("切り替え（appStore.dialog）", () => {
    appStore.set({ dialog: { kind: "shortcuts" } });
    expect(render(createElement(Dialogs))).toContain("キーボードのショートカット");
    appStore.set({ dialog: null });
    expect(render(createElement(Dialogs))).toBe("");
  });
});

describe("ジョブの表示", () => {
  it("結果だけがあるとき", () => {
    clearResults();
    recordResult({ level: "warning", message: "書き出し: 1 枚は失敗しました", detail: "失敗:\n・x（1 枚）" });
    expect(render(createElement(JobsIndicator))).toContain("結果（1）");
    clearResults();
  });

  it("実行中のジョブ", () => {
    expect(render(createElement(JobsIndicator))).toBe("");
    appStore.set({
      jobs: {
        5: { jobId: 5, kind: "import", state: "running", done: 120, total: 500, message: null, label: "取り込み: DCIM" },
        6: { jobId: 6, kind: "export", state: "running", done: 0, total: 0, message: null, label: "書き出し: 3 枚" },
      },
    });
    const html = render(createElement(JobsIndicator));
    expect(html).toContain("書き出し…");
    expect(html).toContain("ほか 1 件");
  });
});
