// 画面の部品が、代表的な状態で例外なく描けることの確認（DOM を使わず、react-dom/server で文字列にする。
// useEffect は動かないので、読み込みの後の表示は確かめない）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import type { AppStatus, CatalogStatus, DevelopSettings, ErrorInfo } from "../api/types";
import { appStore } from "../state/app";
import { CatalogErrorScreen } from "./CatalogErrorScreen";
import { CloseFailedDialog } from "./CloseFailedDialog";
import { DeleteDialog } from "./DeleteDialog";
import { Dialogs } from "./Dialogs";
import { ExportDialog } from "./ExportDialog";
import { ImportDialog } from "./ImportDialog";
import { JobsIndicator } from "./JobsIndicator";
import { PasteSettingsDialog } from "./PasteSettingsDialog";
import { SettingsDialog } from "./SettingsDialog";
import { ShortcutsDialog } from "./ShortcutsDialog";
import { WelcomeScreen } from "./WelcomeScreen";

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
  catalog: null,
  tools: { ffmpeg: null, ffprobe: null },
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

  it("ショートカット", () => {
    expect(render(createElement(ShortcutsDialog))).toContain("ショートカット");
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
