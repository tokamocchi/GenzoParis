// 書き出しのダイアログ（EXP-01・EXP-04）の入力の検証・ExportSettings への変換・前回の設定の復元・結果の
// まとめ（DOM を使わない純粋な計算）。
//
// ExportSettings の JSON の形は genzo-model の export.rs（`{"jpeg": {"quality": 90}}`・`"tiff16"`・
// `{"long_edge": 2048}` など）。値の範囲も同じところ（ExportSettings::validate）に合わせる。

import type {
  ConflictPolicy,
  ErrorInfo,
  ExportReport,
  ExportSettings,
  JobInfo,
  OutputColorSpace,
} from "../api/types";
import { type Notice, formatCount, groupErrors, groupReasons } from "./dialogFormat";

/** JPEG の品質の範囲（genzo-model の ExportSettings::validate）。 */
export const JPEG_QUALITY_MIN = 1;
export const JPEG_QUALITY_MAX = 100;
/** JPEG の品質の既定値（genzo-model の ExportFormat::DEFAULT_JPEG_QUALITY）。 */
export const DEFAULT_JPEG_QUALITY = 90;
/** 長辺の画素数の上限（genzo-model の ExportSize::MAX_LONG_EDGE）。 */
export const MAX_LONG_EDGE = 65_535;
/** 初めて「長辺を指定」を選んだときの値（仮置き: 2048 px）。 */
export const DEFAULT_LONG_EDGE = 2048;

export type FormatKind = "jpeg" | "tiff16" | "png8" | "png16";
export type SizeMode = "original" | "long_edge";

/** 画面の入力（数値は入力欄の文字列のまま持ち、書き出すときに検証する）。 */
export interface ExportForm {
  format: FormatKind;
  quality: string;
  colorSpace: OutputColorSpace;
  sizeMode: SizeMode;
  longEdge: string;
  removeGps: boolean;
  /** キーワード（LIB-09。XMP の dc:subject）を書かない（既定はオフ = 書く）。 */
  removeKeywords: boolean;
  onConflict: ConflictPolicy;
  destDir: string;
}

export const FORMAT_OPTIONS: { value: FormatKind; label: string }[] = [
  { value: "jpeg", label: "JPEG（8bit）" },
  { value: "tiff16", label: "TIFF（16bit）" },
  { value: "png8", label: "PNG（8bit）" },
  { value: "png16", label: "PNG（16bit）" },
];

export const COLOR_SPACE_OPTIONS: { value: OutputColorSpace; label: string }[] = [
  { value: "srgb", label: "sRGB" },
  { value: "display_p3", label: "Display P3" },
  { value: "adobe_rgb", label: "Adobe RGB (1998)" },
];

export const CONFLICT_OPTIONS: { value: ConflictPolicy; label: string }[] = [
  { value: "sequence", label: "連番を付けて別の名前にする" },
  { value: "overwrite", label: "上書きする" },
  { value: "skip", label: "書き出さない（飛ばす）" },
];

export function colorSpaceLabel(cs: OutputColorSpace): string {
  return COLOR_SPACE_OPTIONS.find((o) => o.value === cs)?.label ?? cs;
}

/** 既定の入力（GPS の削除は既定でオン。EXP-04・SEC-03）。 */
export function defaultExportForm(colorSpace: OutputColorSpace): ExportForm {
  return {
    format: "jpeg",
    quality: String(DEFAULT_JPEG_QUALITY),
    colorSpace,
    sizeMode: "original",
    longEdge: String(DEFAULT_LONG_EDGE),
    removeGps: true,
    removeKeywords: false,
    onConflict: "sequence",
    destDir: "",
  };
}

/** 整数の入力を読む（全角の数字・桁区切りも受け付ける。整数でなければ null）。 */
export function parseIntInput(s: string): number | null {
  const t = s
    .trim()
    .replace(/[０-９]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0))
    .replace(/[,，]/g, "");
  if (!/^\d+$/.test(t)) return null;
  const n = Number(t);
  return Number.isSafeInteger(n) ? n : null;
}

export interface ExportFormErrors {
  quality?: string;
  longEdge?: string;
  destDir?: string;
}

/** 入力を検証する（問題がなければ空のオブジェクト）。 */
export function validateExportForm(f: ExportForm): ExportFormErrors {
  const errors: ExportFormErrors = {};
  if (f.format === "jpeg") {
    const q = parseIntInput(f.quality);
    if (q === null || q < JPEG_QUALITY_MIN || q > JPEG_QUALITY_MAX) {
      errors.quality = `品質は ${JPEG_QUALITY_MIN}〜${JPEG_QUALITY_MAX} の整数で指定してください`;
    }
  }
  if (f.sizeMode === "long_edge") {
    const n = parseIntInput(f.longEdge);
    if (n === null || n < 1 || n > MAX_LONG_EDGE) {
      errors.longEdge = `長辺は 1〜${formatCount(MAX_LONG_EDGE)} px の整数で指定してください`;
    }
  }
  if (f.destDir.trim() === "") errors.destDir = "書き出し先のフォルダを選んでください";
  return errors;
}

export function hasErrors(e: ExportFormErrors): boolean {
  return Object.keys(e).length > 0;
}

/** コアに渡す ExportSettings（形式・大きさの値が不正なら null）。 */
export function toExportSettings(f: ExportForm): ExportSettings | null {
  const e = validateExportForm(f);
  if (e.quality || e.longEdge) return null;
  return {
    format: f.format === "jpeg" ? { jpeg: { quality: parseIntInput(f.quality) as number } } : f.format,
    color_space: f.colorSpace,
    size: f.sizeMode === "original" ? "original" : { long_edge: parseIntInput(f.longEdge) as number },
    remove_gps: f.removeGps,
    on_conflict: f.onConflict,
    remove_keywords: f.removeKeywords,
  };
}

// ---------------------------------------------------------------------------
// 前回の設定（localStorage）
// ---------------------------------------------------------------------------

/** 保存する形（`defaultColorSpace` は保存した時点のカタログの既定の色空間）。 */
export interface SavedExport {
  v: 1;
  form: ExportForm;
  defaultColorSpace: OutputColorSpace;
}

const FORMATS: readonly string[] = ["jpeg", "tiff16", "png8", "png16"];
const COLOR_SPACES: readonly string[] = ["srgb", "display_p3", "adobe_rgb"];
const CONFLICTS: readonly string[] = ["sequence", "overwrite", "skip"];

export function savedExport(form: ExportForm, coreDefault: OutputColorSpace): SavedExport {
  return { v: 1, form, defaultColorSpace: coreDefault };
}

/**
 * 前回の設定から入力を作る（壊れた項目は既定値）。
 *
 * 色空間は、保存した後に設定の画面で既定の色空間を変えていれば、新しい既定を使う（変えていなければ前回の
 * 色空間）。
 */
export function restoreExportForm(saved: unknown, coreDefault: OutputColorSpace): ExportForm {
  const base = defaultExportForm(coreDefault);
  if (typeof saved !== "object" || saved === null) return base;
  const s = saved as Partial<SavedExport>;
  const f = (typeof s.form === "object" && s.form !== null ? s.form : {}) as Partial<ExportForm>;
  const str = (v: unknown, fallback: string) => (typeof v === "string" ? v : fallback);
  const pick = <T extends string>(v: unknown, allowed: readonly string[], fallback: T): T =>
    typeof v === "string" && allowed.includes(v) ? (v as T) : fallback;
  const defaultChanged = s.defaultColorSpace !== coreDefault;
  return {
    format: pick(f.format, FORMATS, base.format),
    quality: str(f.quality, base.quality),
    colorSpace: defaultChanged ? coreDefault : pick(f.colorSpace, COLOR_SPACES, base.colorSpace),
    sizeMode: pick(f.sizeMode, ["original", "long_edge"], base.sizeMode),
    longEdge: str(f.longEdge, base.longEdge),
    removeGps: typeof f.removeGps === "boolean" ? f.removeGps : base.removeGps,
    removeKeywords: typeof f.removeKeywords === "boolean" ? f.removeKeywords : base.removeKeywords,
    onConflict: pick(f.onConflict, CONFLICTS, base.onConflict),
    destDir: str(f.destDir, base.destDir),
  };
}

// ---------------------------------------------------------------------------
// 結果のまとめ
// ---------------------------------------------------------------------------

/** 原本の保護（protected_destination）の分かりやすい文と案内。 */
const PROTECTED_MESSAGE = "原本と同じ場所・同じ名前になるため書き出しませんでした（原本は変更していません）";
const PROTECTED_HINT = "書き出し先のフォルダか、同じ名前があるときの扱い（連番）を変えてください";

/** 失敗のエラーを表示用にする（原本の保護は分かりやすい文にする）。 */
function displayError(e: ErrorInfo): ErrorInfo {
  if (e.kind !== "protected_destination") return e;
  return { ...e, message: PROTECTED_MESSAGE, hint: PROTECTED_HINT };
}

/**
 * 書き出しの結果の説明。失敗はエラーの種類ごと（件数・例の文・案内）、飛ばした理由と警告は文ごとにまとめる
 * （失敗の文にはファイルのパスが入るため。dialogFormat.ts の groupErrors）。
 */
export function describeExportReport(r: ExportReport): Notice {
  const parts = [`${formatCount(r.written)} 枚を書き出しました`];
  if (r.skipped > 0) parts.push(`${formatCount(r.skipped)} 枚は飛ばしました`);
  if (r.failed > 0) parts.push(`${formatCount(r.failed)} 枚は失敗しました`);
  const head = r.cancelled ? "書き出しを取り消しました" : "書き出し";
  const message = `${head}: ${parts.join("、")}`;

  const failures: ErrorInfo[] = [];
  const skippedReasons: string[] = [];
  const warnings: string[] = [];
  for (const item of r.items) {
    const o = item.outcome;
    if (o.status === "failed") failures.push(displayError(o.error));
    else if (o.status === "skipped") skippedReasons.push(o.reason);
    else warnings.push(...o.warnings);
  }
  const lines: string[] = [];
  if (failures.length > 0) lines.push("失敗:", ...groupErrors(failures, "枚"));
  if (skippedReasons.length > 0) lines.push("飛ばした理由:", ...groupReasons(skippedReasons, "枚"));
  if (warnings.length > 0) lines.push("警告:", ...groupReasons(warnings, "件", 3));
  lines.push(`書き出し先: ${r.dest_dir}`);

  let level: Notice["level"];
  if (r.failed > 0) level = r.written > 0 ? "warning" : "error";
  else if (r.skipped > 0 || r.cancelled) level = "warning";
  else level = "success";
  return { level, message, detail: lines.join("\n") };
}

/**
 * 書き出しのジョブの結果の説明。ジョブそのものの失敗（state が failed）は、state/app.ts が
 * job_finished のイベントで通知するので null（二重に知らせない）。
 */
export function describeExportJob(info: JobInfo): Notice | null {
  if (info.state === "failed") return null;
  const r = info.result;
  if (r && r.type === "export") return describeExportReport(r);
  if (info.state === "cancelled") return { level: "info", message: "書き出しを取り消しました" };
  return { level: "success", message: "書き出しが終わりました" };
}
