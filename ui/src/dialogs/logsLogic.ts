// ログ・クラッシュレポート（SYS-07）とカタログの構造（ORG-07）の表示の文言（画面の部品から分けて単体テストする）。

import type { CatalogSchema, ClearReport, CrashReportSummary, LogFolderInfo, SchemaTable } from "../api/types";
import { formatBytes } from "../format";
import { formatLocalDateTime } from "./dialogFormat";

/** 送信しないことの説明（設定・知らせのダイアログで同じ文言）。 */
export const LOCAL_ONLY_NOTE =
  "ログとクラッシュレポートは、この PC の中だけに保存しています（外部へは送信しません）。写真のファイル名・フォルダの場所が含まれます。";

/** ログのフォルダの中身の要約（「ログ 2 個・クラッシュレポート 1 件、合計 1.20 MB」）。 */
export function describeLogFolder(info: LogFolderInfo): string {
  if (!info.exists) return "まだログはありません";
  const parts = [`ログ ${info.files.length} 個`];
  if (info.crash_reports.length > 0) parts.push(`クラッシュレポート ${info.crash_reports.length} 件`);
  return `${parts.join("・")}、合計 ${formatBytes(info.total_bytes)}`;
}

/** ログを消した結果の知らせ。 */
export function describeClearReport(r: ClearReport): { level: "success" | "warning"; message: string; detail?: string } {
  const done = r.removed + r.truncated;
  if (r.failed.length > 0) {
    return {
      level: "warning",
      message: `ログを消しました（${done} 個）。${r.failed.length} 個は消せませんでした`,
      detail: r.failed.join("\n"),
    };
  }
  return { level: "success", message: done > 0 ? `ログを消しました（${done} 個）` : "消すログはありませんでした" };
}

/** クラッシュレポートの 1 行（「2026-10-13 10:02:03　index out of bounds」）。 */
export function describeCrashReport(r: CrashReportSummary, offsetMinutes?: number): string {
  const when = r.created_at ? formatLocalDateTime(r.created_at, offsetMinutes) : r.file_name;
  return r.message ? `${when}　${r.message}` : when;
}

/** 起動時のクラッシュの知らせのダイアログの見出し。 */
export function crashNoticeTitle(reports: CrashReportSummary[]): string {
  return reports.length > 1
    ? `前回までに GenzoParis が ${reports.length} 回、異常終了しました`
    : "前回、GenzoParis が異常終了しました";
}

/** 画面のエラー（window の error・unhandledrejection）を、ログに残す文字列にする。 */
export function uiErrorText(reason: unknown): { message: string; detail: string | null } {
  if (reason instanceof Error) {
    return { message: `${reason.name}: ${reason.message}`, detail: reason.stack ?? null };
  }
  if (typeof reason === "string") return { message: reason, detail: null };
  try {
    return { message: JSON.stringify(reason) ?? String(reason), detail: null };
  } catch {
    return { message: String(reason), detail: null };
  }
}

/** カタログの構造の要約（表・ビュー・索引・トリガーの数）。 */
export function describeSchema(schema: CatalogSchema): string {
  const count = (kind: string) => schema.objects.filter((o) => o.kind === kind).length;
  return `表 ${count("table")} 個・ビュー ${count("view")} 個・索引 ${count("index")} 個・トリガー ${count("trigger")} 個`;
}

/** 公開ビュー（約束どおりの順）と、その列。 */
export function publicViewTables(schema: CatalogSchema): SchemaTable[] {
  return schema.public_views
    .map((name) => schema.tables.find((t) => t.name === name))
    .filter((t): t is SchemaTable => t !== undefined);
}

/** すべてのオブジェクトの SQL をつないだもの（画面でまとめて見る・コピーする）。 */
export function schemaSql(schema: CatalogSchema): string {
  const head = `-- GenzoParis のカタログ（スキーマの版 ${schema.schema_version}）\n`;
  return head + schema.objects.map((o) => `${o.sql.trim()};`).join("\n\n") + "\n";
}

/** 読み取り専用で開く例（SQLite の CLI。パスに空白があってもよいように引用符で囲む）。 */
export function readOnlyCommand(catalogPath: string): string {
  return `sqlite3 -readonly "${catalogPath}"`;
}
