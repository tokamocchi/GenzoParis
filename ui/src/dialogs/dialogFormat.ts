// ダイアログ・起動の画面・ジョブの表示で使う書式（日時・パス・ジョブの進捗。DOM を使わない純粋な計算）。

import type { ErrorInfo, ErrorKind, IntegrityReport, JobKind, JobResult, JobState } from "../api/types";

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
 * フォルダとファイルの名前をつなぐ。区切りはフォルダの書き方に合わせる（"\" だけを使うパス（Windows）なら
 * "\"、それ以外は "/"）。フォルダが区切りで終わっていれば、そのままつなぐ。
 */
export function joinPath(dir: string, name: string): string {
  const d = dir.trim();
  if (d === "") return name;
  if (d.endsWith("/") || d.endsWith("\\")) return `${d}${name}`;
  const sep = d.includes("\\") && !d.includes("/") ? "\\" : "/";
  return `${d}${sep}${name}`;
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
  hash_files: "内容のハッシュの計算",
  move_files: "写真の移動",
  check_missing: "見つからない写真の確認",
  find_relink_candidates: "再リンクの候補の探索",
  video_proxy: "再生用のファイルの作成",
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
  if (job.kind === "video_proxy") return `${name} ${formatJobAmount(job)}`;
  return `${name} ${formatCount(Math.min(job.done, job.total))}/${formatCount(job.total)}`;
}

/**
 * 進み具合の量（"120 / 500"。動画の再生用のファイルの作成は、進み具合が動画の位置（ミリ秒）なので "16%"）。
 * 分からなければ ""。
 */
export function formatJobAmount(job: { kind: JobKind; done: number; total: number }): string {
  if (job.total <= 0) return "";
  const done = Math.min(job.done, job.total);
  if (job.kind === "video_proxy") return `${Math.floor((done / job.total) * 100)}%`;
  return `${formatCount(done)} / ${formatCount(job.total)}`;
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
  )} 件・動画の再生用のファイル ${formatCount(r.video_proxies_removed ?? 0)} 個・一時ファイル ${formatCount(
    r.temp_files_removed,
  )} 個を削除）`;
}

/** 通知の内容（actions.notify に渡す）。 */
export interface Notice {
  level: "info" | "success" | "warning" | "error";
  message: string;
  detail?: string;
}

/** エラーの種類の名前（失敗の理由を種類ごとにまとめるときの見出し。genzo-api の ErrorKind）。 */
export const ERROR_KIND_LABEL: Record<ErrorKind, string> = {
  invalid_input: "値が正しくない",
  not_found: "見つからない",
  stale: "情報が古い",
  cancelled: "取り消した",
  file_access: "ファイルにアクセスできない",
  unsupported_file: "対応していないファイル",
  corrupt_file: "ファイルが壊れている",
  protected_destination: "原本と同じ場所・同じ名前",
  conflict: "ほかの操作・ファイルとの競合",
  trash_unavailable: "ゴミ箱を使えない",
  storage: "空き容量・書き込みの問題",
  catalog_corrupt: "カタログの問題",
  incompatible_version: "版が合わない",
  worker: "画像の処理（ワーカー）のエラー",
  gpu: "GPU のエラー",
  closed: "カタログが閉じている",
  internal: "内部のエラー",
};

/**
 * 失敗をエラーの種類（ErrorInfo.kind）ごとにまとめた行（多い順。`limit` 種類まで、残りは「ほか N 種類」）。
 *
 * コアの説明の文にはファイルのパスが入るので、文ごとにまとめると、同じ原因（ドライブが外れたなど）の失敗が
 * 1 件ずつ別の理由になる。種類ごとに件数・例の文（`examples` 個まで。違う文だけ）・案内（最初に見つかった
 * hint）を出す。
 */
export function groupErrors(errors: readonly ErrorInfo[], unit: string, limit = 5, examples = 2): string[] {
  const groups = new Map<ErrorKind, { count: number; examples: string[]; hint: string | null }>();
  for (const e of errors) {
    const g = groups.get(e.kind) ?? { count: 0, examples: [], hint: null };
    g.count++;
    if (g.examples.length < examples && !g.examples.includes(e.message)) g.examples.push(e.message);
    if (!g.hint && e.hint) g.hint = e.hint;
    groups.set(e.kind, g);
  }
  const sorted = [...groups.entries()].sort((a, b) => b[1].count - a[1].count);
  const lines: string[] = [];
  for (const [kind, g] of sorted.slice(0, limit)) {
    lines.push(`・${ERROR_KIND_LABEL[kind] ?? kind}（${formatCount(g.count)} ${unit}）`);
    for (const ex of g.examples) lines.push(`　例: ${ex}`);
    if (g.hint) lines.push(`　→ ${g.hint}`);
  }
  if (sorted.length > limit) lines.push(`・ほか ${formatCount(sorted.length - limit)} 種類の理由`);
  return lines;
}

/** 通知に出す詳細の行の上限（仮置き: 8 行。全体は上の帯の「結果」で見られる）。 */
export const TOAST_DETAIL_LINES = 8;

/**
 * 通知に出す詳細を短くする（`maxLines` 行まで。切ったら、続きの見方を書き足す）。
 */
export function shortenDetail(detail: string | undefined, maxLines = TOAST_DETAIL_LINES): string | undefined {
  if (detail === undefined) return undefined;
  const lines = detail.split("\n");
  if (lines.length <= maxLines) return detail;
  return [...lines.slice(0, maxLines), "…（続きは上の帯の「結果」で見られます）"].join("\n");
}

/**
 * 理由ごとの件数の行（多い順。`limit` 種類まで、残りは「ほか N 種類」）。決まった文の理由（書き出しを
 * 飛ばした理由など）に使う。パスの入る文は groupErrors で種類ごとにまとめる。
 */
export function groupReasons(reasons: string[], unit: string, limit = 5): string[] {
  const counts = new Map<string, number>();
  for (const r of reasons) counts.set(r, (counts.get(r) ?? 0) + 1);
  const sorted = [...counts.entries()].sort((a, b) => b[1] - a[1]);
  const lines = sorted.slice(0, limit).map(([r, n]) => `・${r}（${formatCount(n)} ${unit}）`);
  if (sorted.length > limit) lines.push(`・ほか ${formatCount(sorted.length - limit)} 種類の理由`);
  return lines;
}
