// ダイアログ・起動の画面・ジョブの表示で使う書式（日時・パス・ジョブの進捗。DOM を使わない純粋な計算）。

import type { IntegrityReport, JobKind, JobResult, JobState } from "../api/types";

/** 件数の表示（3 桁区切り）。 */
export function formatCount(n: number): string {
  return n.toLocaleString("ja-JP");
}

/**
 * RFC 3339 の日時を、現地時刻の「2026-10-12 14:03:05」にする。
 *
 * `offsetMinutes` は UTC からのずれ（分。日本なら 540）。省くと、その日時での OS のずれを使う（テストでは
 * 決まった値を渡す）。読めない文字列はそのまま返す。
 */
export function formatLocalDateTime(iso: string, offsetMinutes?: number): string {
  const ms = Date.parse(iso);
  if (!Number.isFinite(ms)) return iso;
  const offset = offsetMinutes ?? -new Date(ms).getTimezoneOffset();
  const t = new Date(ms + offset * 60_000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${t.getUTCFullYear()}-${p(t.getUTCMonth() + 1)}-${p(t.getUTCDate())} ${p(t.getUTCHours())}:${p(
    t.getUTCMinutes(),
  )}:${p(t.getUTCSeconds())}`;
}

/** パスの最後の名前（"/a/b/c.db" → "c.db"。Windows の "\" も区切りとみなす）。 */
export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/** パスのフォルダの部分（"/a/b/c.db" → "/a/b"。区切りがなければ null）。 */
export function parentDir(path: string): string | null {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (i < 0) return null;
  if (i === 0) return path.slice(0, 1);
  // "C:\a.db" のフォルダは "C:\"。
  if (i === 2 && /^[A-Za-z]:/.test(path)) return path.slice(0, 3);
  return path.slice(0, i);
}

/**
 * カタログのファイル名に拡張子 .db を付ける（保存のダイアログで拡張子を付けない OS があるため）。
 * 既に .db（大文字・小文字を問わない）なら、そのまま返す。
 */
export function ensureDbExtension(path: string): string {
  const trimmed = path.trim();
  if (trimmed === "") return trimmed;
  return /\.db$/i.test(baseName(trimmed)) ? trimmed : `${trimmed}.db`;
}

/** ジョブの種類の名前（state/app.ts の JOB_LABEL と同じ。そちらが公開されたら置き換える）。 */
export const JOB_KIND_LABEL: Record<JobKind, string> = {
  import: "取り込み",
  export: "書き出し",
  regenerate_previews: "プレビューの作り直し",
  integrity_check: "カタログの詳細チェック",
  reresolve_capture_times: "撮影日時の推定し直し",
  collect_garbage: "キャッシュの整理",
};

export const JOB_STATE_LABEL: Record<JobState, string> = {
  running: "実行中",
  succeeded: "完了",
  failed: "失敗",
  cancelled: "取り消し",
};

/** ジョブの進み具合の短い表示（「取り込み 120/500」。総数が分からなければ「取り込み…」）。 */
export function formatJobProgress(job: { kind: JobKind; done: number; total: number }): string {
  const name = JOB_KIND_LABEL[job.kind];
  if (job.total <= 0) return `${name}…`;
  return `${name} ${formatCount(Math.min(job.done, job.total))}/${formatCount(job.total)}`;
}

/** ジョブの一覧の並び（実行中を先に、それぞれ新しい順）。 */
export function sortJobs<T extends { jobId: number; state: JobState }>(jobs: Record<number, T>): T[] {
  return Object.values(jobs).sort((a, b) => {
    const ra = a.state === "running" ? 0 : 1;
    const rb = b.state === "running" ? 0 : 1;
    if (ra !== rb) return ra - rb;
    return b.jobId - a.jobId;
  });
}

/** 詳細チェックの結果の説明（問題がなければ 1 行）。 */
export function describeIntegrity(r: IntegrityReport): { ok: boolean; lines: string[] } {
  if (r.ok) return { ok: true, lines: ["問題は見つかりませんでした"] };
  const lines: string[] = [];
  if (r.integrity_errors.length > 0) {
    lines.push(`データベースの整合性の問題: ${formatCount(r.integrity_errors.length)} 件`);
    for (const e of r.integrity_errors.slice(0, 5)) lines.push(`・${e}`);
  }
  if (r.foreign_key_violations.length > 0) {
    lines.push(`参照の不整合: ${formatCount(r.foreign_key_violations.length)} 件`);
    for (const e of r.foreign_key_violations.slice(0, 5)) lines.push(`・${e}`);
  }
  if (r.fts_error) lines.push(`テキスト検索の索引の問題: ${r.fts_error}`);
  if (lines.length === 0) lines.push("問題が見つかりました（内容は不明）");
  return { ok: false, lines };
}

/** キャッシュの整理の結果の説明。 */
export function describeGarbage(r: Extract<JobResult, { type: "collect_garbage" }>): string {
  return `キャッシュを整理しました（サムネイル ${formatCount(r.thumbnails_removed)} 件・プレビュー ${formatCount(
    r.previews_removed,
  )} 件・一時ファイル ${formatCount(r.temp_files_removed)} 個を削除）`;
}

/** 通知の内容（actions.notify に渡す）。 */
export interface Notice {
  level: "info" | "success" | "warning" | "error";
  message: string;
  detail?: string;
}

/**
 * 理由ごとの件数の行（多い順。`limit` 種類まで、残りは「ほか N 種類」）。
 */
export function groupReasons(reasons: string[], unit: string, limit = 5): string[] {
  const counts = new Map<string, number>();
  for (const r of reasons) counts.set(r, (counts.get(r) ?? 0) + 1);
  const sorted = [...counts.entries()].sort((a, b) => b[1] - a[1]);
  const lines = sorted.slice(0, limit).map(([r, n]) => `・${r}（${formatCount(n)} ${unit}）`);
  if (sorted.length > limit) lines.push(`・ほか ${formatCount(sorted.length - limit)} 種類の理由`);
  return lines;
}
