// 画面の部品が、代表的な状態で例外なく描けることの確認（DOM を使わず、react-dom/server で文字列にする。
// useEffect は動かないので、読み込みの後の表示は確かめない）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import {
  MOCK_CORE_SETTINGS,
  MOCK_CRASH_REPORT,
  MOCK_IMPORT_PREFERENCES,
  MOCK_LOG_STATUS,
  mockExportPresets,
} from "../api/mock";
import type { AppStatus, CatalogStatus, DevelopSettings, ErrorInfo } from "../api/types";
import { appStore } from "../state/app";
import { presetsStore } from "../state/presets";
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
  core_settings: MOCK_CORE_SETTINGS,
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
    // 重複と思われるもの（IMP-04）・RAW と JPEG（IMP-06）の選択。初めの値はカタログの設定。
    expect(html).toContain("重複と思われるものを取り込まない");
    expect(html).toContain("RAW と同時記録の JPEG を別々の写真として取り込む");
    expect(html).toMatch(/<input type="checkbox" checked=""\/><span>重複と思われるもの/);
    expect(html).toMatch(/<input type="checkbox"\/><span>RAW と同時記録の JPEG/);
  });

  it("取り込み（既定はその場で追加。コピーの欄は出さない）", () => {
    const html = render(createElement(ImportDialog));
    expect(html).toMatch(/<input type="radio" name="import-mode" checked=""\/><span class="title">その場で追加/);
    expect(html).toContain("コピーして追加");
    expect(html).not.toContain("import-folder-rule");
    expect(html).toContain("取り込み時に付けるキーワード・メタデータ");
    expect(html).toContain(">取り込む</button>");
  });

  it("取り込み（アプリに保存したコピーの設定: コピー先・規則・名前・付けるもの。IMP-02・03・05）", () => {
    appStore.set({ status: { ...status, settings: { ...status.settings, import: MOCK_IMPORT_PREFERENCES } } });
    const html = render(createElement(ImportDialog));
    expect(html).toMatch(/<input type="radio" name="import-mode" checked=""\/><span class="title">コピーして追加/);
    expect(html).toContain("/home/u/写真");
    expect(html).toContain('<option value="year_date" selected="">撮影日別（2026/2026-10-13）</option>');
    expect(html).toContain('value="{date}_{text}-{seq}"');
    expect(html).toContain("任意の文字");
    expect(html).toContain('value="京都"');
    expect(html).toContain("桁数");
    expect(html).toContain("撮影時刻");
    expect(html).toContain("取り込み時に付けるキーワード・メタデータ（4 件）");
    expect(html).toContain('value="場所 &gt; 京都, 家族"');
    expect(html).toContain('value="岡本"');
    expect(html).toContain("内容（ハッシュ）が同じことを確かめてから");
    expect(html).toContain(">コピーして取り込む</button>");
    // コピー先がなければ、理由を出して取り込めない。
    appStore.set({
      status: { ...status, settings: { ...status.settings, import: { ...MOCK_IMPORT_PREFERENCES, destination: null } } },
    });
    const empty = render(createElement(ImportDialog));
    expect(empty).toContain("コピー先のフォルダを選んでください");
    expect(empty).toMatch(/<button class="btn primary" disabled="">コピーして取り込む<\/button>/);
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
    // 後から付けた位置（LIB-21）も、この設定では書かない。
    expect(html).toContain("後から付けた位置も書きません");
    // 書き出しプリセット（EXP-02）とファイル名。
    expect(html).toContain("プリセット");
    expect(html).toContain("新しく保存…");
    expect(html).toContain("元のファイル名");
    // 解像度・出力シャープ・透かし（EXP-03）と動画（EXP-05）。
    expect(html).toContain("解像度");
    expect(html).toContain("出力シャープ");
    expect(html).toContain("透かし");
    expect(html).toContain("動画を書き出す");
  });

  it("書き出し: プリセットの一覧（フォルダ・使えないものは最後に理由付きで）", () => {
    const [web] = mockExportPresets();
    presetsStore.set({
      export: [
        web,
        { ...web, id: "00000000000000e2", name: "印刷", folder: "仕事" },
        { ...web, id: "00000000000000e3", name: "未来", export: undefined, problem: "新しい版のアプリで作ったプリセットです" },
      ],
    });
    try {
      const html = render(createElement(ExportDialog, { variantIds: [1] }));
      expect(html).toContain("Web 用");
      expect(html).toContain('label="仕事"');
      expect(html).toContain("印刷");
      // 使えないプリセットも一覧に出す（選ぶと削除だけできる。決定事項ログ No.162）。
      expect(html).toContain('label="使えないプリセット（削除だけできます）"');
      expect(html).toContain("未来（使えません）");
      expect(html).toContain('title="新しい版のアプリで作ったプリセットです"');
    } finally {
      presetsStore.set({ export: null });
    }
  });

  it("書き出し: 選んだプリセットが使えない・別の OS のパスを使わない", () => {
    const [web] = mockExportPresets();
    const data = new Map<string, string>();
    (globalThis as { localStorage?: unknown }).localStorage = {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, String(v)),
      removeItem: (k: string) => void data.delete(k),
    };
    const remember = (presetId: string) =>
      data.set("genzo.dialogs.export.last", JSON.stringify({ v: 1, form: {}, defaultColorSpace: "srgb", presetId }));
    presetsStore.set({
      export: [
        { ...web, warnings: ["書き出し先のフォルダ（C:\\Users\\a\\out）は、この PC のパスではないため使いません"] },
        { ...web, id: "00000000000000e3", name: "未来", export: undefined, problem: "新しい版のアプリで作ったプリセットです" },
      ],
    });
    try {
      // 前回選んだプリセットが、その後に使えなくなった: 理由と削除の案内を出す（入力は変えない）。
      remember("00000000000000e3");
      const broken = render(createElement(ExportDialog, { variantIds: [1] }));
      expect(broken).toContain("書き出しプリセット「未来」は使えません: 新しい版のアプリで作ったプリセットです");
      expect(broken).toContain("「削除…」で削除できます");
      // 別の OS で作ったプリセット: 使わない値を知らせる。
      remember(web.id);
      const foreign = render(createElement(ExportDialog, { variantIds: [1] }));
      expect(foreign).toContain("この PC のパスではないため使いません");
      expect(foreign).not.toContain("は使えません:");
    } finally {
      delete (globalThis as { localStorage?: unknown }).localStorage;
      presetsStore.set({ export: null });
    }
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

  it("設定のカタログ: 取り込みの既定・内容による同一性（IMP-04・IMP-06・ORG-03）", () => {
    const html = render(createElement(SettingsDialog, { tab: "catalog" }));
    expect(html).toContain("取り込みで、重複と思われるものを取り込まない");
    expect(html).toContain("RAW と同時記録の JPEG を別々の写真として取り込む");
    expect(html).toContain("内容のハッシュをバックグラウンドで計算する");
    expect(html).toContain("ネットワークのボリューム（NAS など）のファイルも計算する");
    expect(html).toContain("内容による同一性（重複の検出）");
    expect(html).toContain("今すぐ計算する");
    expect(html).toContain("内容が同じファイルを表示");
    // 状況は開いた後に読む（renderToString では効果が動かない）。
    expect(html).toContain("状況を読んでいます");
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
