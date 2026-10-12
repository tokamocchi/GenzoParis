// 絞り込み（LIB-07）・並べ替え（LIB-08）の画面の値と SearchFilter の対応（DOM を使わない純粋な計算）。
//
// 撮影日の範囲（genzo-catalog の search.rs の `Filter`）:
// - `captured_from` は「この時刻を含む」、`captured_until` は「この時刻を含まない」。
// - 画面の日付は、カタログの既定のタイムゾーン（CoreSettings の default_utc_offset_minutes。UTC より東が
//   正）の現地の日付とみなす（CLI の `--from`・`--to` の日付と同じ。genzo-cli の args.rs の `DateArg`）。
//   開始日はその日の 0 時、終了日は翌日の 0 時（含まない）を UTC にする（終了日の終わりまでを含める）。

import type { ColorLabel, Facets, FacetValue, Flag, Rating, SearchFilter, SortKey } from "../api/types";

// ---------------------------------------------------------------------------
// 有効な条件の数・解除
// ---------------------------------------------------------------------------

const nonEmpty = (v: unknown[] | null | undefined) => Array.isArray(v) && v.length > 0;
const isSet = (v: unknown) => v !== null && v !== undefined;

/**
 * 有効な条件の数（フォルダは「どこを見るか」なので数えない）。撮影日の範囲は開始・終了をまとめて 1 つ、
 * 評価の下限・上限もまとめて 1 つに数える。
 */
export function activeConditionCount(f: SearchFilter): number {
  let n = 0;
  if (typeof f.text === "string" && f.text.trim() !== "") n++;
  if (isSet(f.rating_min) || isSet(f.rating_max)) n++;
  if (isSet(f.flags)) n++;
  if (isSet(f.color_labels)) n++;
  if (isSet(f.captured_from) || isSet(f.captured_until)) n++;
  if (nonEmpty(f.cameras)) n++;
  if (nonEmpty(f.lenses)) n++;
  if (isSet(f.kind)) n++;
  if (isSet(f.long_edge_min) || isSet(f.long_edge_max)) n++;
  if (isSet(f.duration_min_s) || isSet(f.duration_max_s)) n++;
  if (isSet(f.fps_min) || isSet(f.fps_max)) n++;
  if (nonEmpty(f.codecs)) n++;
  if (nonEmpty(f.keywords)) n++;
  if (isSet(f.file_statuses)) n++;
  if (f.masters_only) n++;
  return n;
}

/** 条件をすべて解除する（フォルダの選択は残す）。 */
export function clearConditions(f: SearchFilter): SearchFilter {
  const out: SearchFilter = {};
  if (isSet(f.folder_id)) {
    out.folder_id = f.folder_id;
    out.include_subfolders = f.include_subfolders ?? false;
  }
  return out;
}

/** 空の文字列は条件なし（null）にする。 */
export function normalizeText(s: string): string | null {
  const t = s.trim();
  return t === "" ? null : t;
}

// ---------------------------------------------------------------------------
// 評価・フラグ・カラーラベル・種類
// ---------------------------------------------------------------------------

/** 評価の選択肢（0 は「すべて」。1〜5 は「★N 以上」。5 は ★5 だけ）。 */
export const RATING_OPTIONS: { value: number; label: string }[] = [
  { value: 0, label: "評価: すべて" },
  { value: 1, label: "★1 以上" },
  { value: 2, label: "★2 以上" },
  { value: 3, label: "★3 以上" },
  { value: 4, label: "★4 以上" },
  { value: 5, label: "★5" },
];

/** 評価の選択肢 → 条件。 */
export function ratingPatch(min: number): Pick<SearchFilter, "rating_min" | "rating_max"> {
  return { rating_min: min >= 1 && min <= 5 ? (min as Rating) : null, rating_max: null };
}

/** 条件 → 評価の選択肢（上限があるなど選択肢にない形は -1）。 */
export function ratingOption(f: SearchFilter): number {
  if (isSet(f.rating_max)) return -1;
  return f.rating_min ?? 0;
}

export type FlagOption = "all" | "picked" | "none" | "rejected" | "not_rejected";

export const FLAG_OPTIONS: { value: FlagOption; label: string }[] = [
  { value: "all", label: "フラグ: すべて" },
  { value: "picked", label: "採用" },
  { value: "none", label: "フラグなし" },
  { value: "rejected", label: "不採用" },
  { value: "not_rejected", label: "不採用以外" },
];

const FLAG_SETS: Record<Exclude<FlagOption, "all">, Flag[]> = {
  picked: [1],
  none: [0],
  rejected: [-1],
  not_rejected: [0, 1],
};

/** フラグの選択肢 → 条件。 */
export function flagsFor(option: FlagOption): Flag[] | null {
  return option === "all" ? null : [...FLAG_SETS[option]];
}

/** 条件 → フラグの選択肢（選択肢にない組み合わせは null）。 */
export function flagOption(flags: Flag[] | null | undefined): FlagOption | null {
  if (!flags) return "all";
  const set = [...new Set(flags)].sort((a, b) => a - b);
  for (const [k, v] of Object.entries(FLAG_SETS)) {
    if (v.length === set.length && v.every((x, i) => x === set[i])) return k as FlagOption;
  }
  return null;
}

/** カラーラベルの絞り込みの順（null は「ラベルなし」）。 */
export const LABEL_CHOICES: (ColorLabel | null)[] = ["red", "yellow", "green", "blue", "purple", null];

/** カラーラベルの選択を切り替える（空になったら条件なし）。順は LABEL_CHOICES に揃える。 */
export function toggleLabel(
  labels: (ColorLabel | null)[] | null | undefined,
  label: ColorLabel | null,
): (ColorLabel | null)[] | null {
  const cur = labels ?? [];
  const next = cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label];
  if (next.length === 0) return null;
  return LABEL_CHOICES.filter((l) => next.includes(l));
}

// ---------------------------------------------------------------------------
// カメラ・レンズ・コーデック（絞り込みの候補）
// ---------------------------------------------------------------------------

export const EMPTY_FACETS: Facets = { cameras: [], lenses: [], codecs: [] };

/**
 * 選択肢の一覧（候補に、選択中で候補にない値を足す。候補を読めないときも、選んだ値は表示できるように）。
 */
export function facetOptions(values: FacetValue[], selected: string[] | null | undefined): FacetValue[] {
  const out = [...values];
  for (const s of selected ?? []) {
    if (!out.some((v) => v.value === s)) out.push({ value: s, count: 0 });
  }
  return out;
}

// ---------------------------------------------------------------------------
// 撮影日の範囲
// ---------------------------------------------------------------------------

const DAY_MS = 86_400_000;

/** "2024-05-01" → [2024, 5, 1]（形が違う・ありえない日付なら null）。 */
function parseDate(s: string): [number, number, number] | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s.trim());
  if (!m) return null;
  const y = Number(m[1]);
  const mo = Number(m[2]);
  const d = Number(m[3]);
  if (y < 1 || mo < 1 || mo > 12 || d < 1) return null;
  const t = new Date(Date.UTC(2000, mo - 1, d));
  t.setUTCFullYear(y);
  if (t.getUTCMonth() !== mo - 1 || t.getUTCDate() !== d) return null;
  return [y, mo, d];
}

/** 年月日の UTC の 0 時（ミリ秒。0〜99 年も正しく扱う）。 */
function utcMidnight(y: number, mo: number, d: number): number {
  const t = new Date(Date.UTC(2000, mo - 1, d));
  t.setUTCFullYear(y);
  return t.getTime();
}

/** RFC 3339（UTC。秒まで）。 */
function rfc3339(ms: number): string {
  return new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z");
}

/**
 * 現地の日付（"YYYY-MM-DD"）→ UTC の境界（RFC 3339）。`upper` なら翌日の 0 時（含まない上限）。
 * `offsetMinutes` は UTC より東が正（+09:00 なら 540）。空・不正な日付なら null。
 */
export function localDateToUtc(date: string, offsetMinutes: number, upper: boolean): string | null {
  const p = parseDate(date);
  if (!p) return null;
  const local = utcMidnight(p[0], p[1], p[2]) + (upper ? DAY_MS : 0);
  return rfc3339(local - offsetMinutes * 60_000);
}

/**
 * UTC の境界（RFC 3339）→ 現地の日付（"YYYY-MM-DD"）。`upper` なら含まない上限として、その前の日を返す。
 * 読めなければ ""。
 */
export function utcToLocalDate(iso: string | null | undefined, offsetMinutes: number, upper: boolean): string {
  if (!iso) return "";
  const ms = Date.parse(iso);
  if (!Number.isFinite(ms)) return "";
  // 上限は「含まない」ので、1 ミリ秒前の日付が最後の日。
  const t = new Date(ms + offsetMinutes * 60_000 - (upper ? 1 : 0));
  const y = t.getUTCFullYear();
  if (y < 1 || y > 9999) return "";
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${p(y, 4)}-${p(t.getUTCMonth() + 1)}-${p(t.getUTCDate())}`;
}

/** UTC からのオフセット（分）の表示（540 → "UTC+09:00"）。 */
export function formatUtcOffset(minutes: number): string {
  const sign = minutes < 0 ? "−" : "+";
  const m = Math.abs(minutes);
  return `UTC${sign}${String(Math.floor(m / 60)).padStart(2, "0")}:${String(m % 60).padStart(2, "0")}`;
}

// ---------------------------------------------------------------------------
// 並べ替え
// ---------------------------------------------------------------------------

export const SORT_OPTIONS: { value: SortKey; label: string }[] = [
  { value: "capture_time", label: "撮影日時" },
  { value: "file_name", label: "ファイル名" },
  { value: "rating", label: "評価" },
  { value: "import_order", label: "取り込み順" },
];
