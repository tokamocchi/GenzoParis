// ルーペ上の撮影情報の表示（LIB-17）の決まり: 表示の切り替え（I）・項目の組み合わせ・書式（DOM を使わない
// 純粋な計算と、小さな状態）。
//
// - 表示は「情報 1」→「情報 2」→「なし」の順に I で切り替える（Lightroom のルーペの情報と同じ）。ルーペと
//   現像の画面で同じ表示を使う。
// - 情報 1・情報 2 に出す項目は設定の「表示」で選べる（既定は INFO_DEFAULTS）。表示と項目は、この PC の
//   ブラウザーの保存領域（localStorage）に覚える（保存できなくても、その起動の間は使える）。
// - MVP の「I で表示・非表示」の保存値（genzo.loupe.showInfo）があれば、"0" は「なし」、それ以外は「情報 1」に
//   読み替える。

import type { VariantDetails, VariantSummary } from "../api/types";
import { formatAperture, formatCaptureTime, formatFocal, formatShutter } from "../format";
import { createStore, useStore } from "../state/store";

/** 表示する項目。 */
export type InfoItem =
  | "file_name"
  | "capture_time"
  | "dimensions"
  | "marks"
  | "position"
  | "camera"
  | "lens"
  | "focal"
  | "aperture"
  | "shutter"
  | "iso"
  | "title"
  | "caption";

/** 項目の一覧（設定の画面の並び）と名前。 */
export const INFO_ITEMS: { item: InfoItem; label: string; details?: boolean }[] = [
  { item: "file_name", label: "ファイル名" },
  { item: "capture_time", label: "撮影日時" },
  { item: "dimensions", label: "寸法" },
  { item: "marks", label: "評価・フラグ・ラベル" },
  { item: "position", label: "何枚目か" },
  { item: "camera", label: "カメラ" },
  { item: "lens", label: "レンズ" },
  { item: "focal", label: "焦点距離", details: true },
  { item: "aperture", label: "絞り", details: true },
  { item: "shutter", label: "シャッター速度", details: true },
  { item: "iso", label: "ISO 感度", details: true },
  { item: "title", label: "タイトル", details: true },
  { item: "caption", label: "キャプション", details: true },
];

/** 表示（0 = なし、1 = 情報 1、2 = 情報 2）。 */
export type InfoMode = 0 | 1 | 2;

/** 情報 1・情報 2 の項目。 */
export interface InfoSets {
  info1: InfoItem[];
  info2: InfoItem[];
}

/** 既定の項目（情報 1: 名前・日時・寸法・印、情報 2: 名前・露出・機材。Lightroom の既定に近い組み合わせ）。 */
export const INFO_DEFAULTS: InfoSets = {
  info1: ["file_name", "capture_time", "dimensions", "marks", "position"],
  info2: ["file_name", "shutter", "aperture", "iso", "focal", "camera", "lens"],
};

const MODE_KEY = "genzo.loupe.infoMode";
const LEGACY_SHOW_KEY = "genzo.loupe.showInfo";
const ITEMS_KEY = "genzo.loupe.infoItems";

/** I を押したときの次の表示（情報 1 → 情報 2 → なし → 情報 1）。 */
export function nextInfoMode(mode: InfoMode): InfoMode {
  return mode === 1 ? 2 : mode === 2 ? 0 : 1;
}

/** 表示の名前（知らせ・ボタンの説明用）。 */
export function infoModeLabel(mode: InfoMode): string {
  return mode === 0 ? "撮影情報: なし" : mode === 1 ? "撮影情報: 情報 1" : "撮影情報: 情報 2";
}

/** 保存した表示を読む（なければ MVP の保存値、それもなければ情報 1）。 */
export function parseInfoMode(stored: string | null, legacy: string | null): InfoMode {
  if (stored === "0" || stored === "1" || stored === "2") return Number(stored) as InfoMode;
  if (legacy !== null) return legacy === "0" ? 0 : 1;
  return 1;
}

const KNOWN = new Set<string>(INFO_ITEMS.map((i) => i.item));

/** 保存した項目を読む（壊れた・知らない値は除き、読めなければ既定）。 */
export function parseInfoSets(stored: string | null): InfoSets {
  if (!stored) return INFO_DEFAULTS;
  try {
    const v = JSON.parse(stored) as Partial<Record<keyof InfoSets, unknown>>;
    const pick = (x: unknown, fallback: InfoItem[]): InfoItem[] => {
      if (!Array.isArray(x)) return fallback;
      const out: InfoItem[] = [];
      for (const i of x) if (typeof i === "string" && KNOWN.has(i) && !out.includes(i as InfoItem)) out.push(i as InfoItem);
      return out;
    };
    return { info1: pick(v.info1, INFO_DEFAULTS.info1), info2: pick(v.info2, INFO_DEFAULTS.info2) };
  } catch {
    return INFO_DEFAULTS;
  }
}

/** 項目を入れる・外す（並びは INFO_ITEMS の順にそろえる）。 */
export function toggleInfoItem(items: InfoItem[], item: InfoItem, on: boolean): InfoItem[] {
  const set = new Set(items);
  if (on) set.add(item);
  else set.delete(item);
  return INFO_ITEMS.map((i) => i.item).filter((i) => set.has(i));
}

/** 項目に詳細（core.variantDetails）が要るか（露出・タイトルなど。概要にない値）。 */
export function needsDetails(items: InfoItem[]): boolean {
  return items.some((i) => INFO_ITEMS.find((d) => d.item === i)?.details);
}

/** 表示する値の材料。 */
export interface InfoSource {
  summary: VariantSummary;
  /** 詳細（読めていなければ null。露出・タイトルなどは「…」）。 */
  details: VariantDetails | null;
  /** 何枚目か（0 から。分からなければ null）と全体の数。 */
  index: number | null;
  count: number;
  /** 寸法の上書き（現像の画面では切り抜いた後の出力の寸法）。 */
  dimensions?: { width: number; height: number } | null;
}

/** 1 行（ラベルは読み上げ用・説明用。marks は印として別に描く）。 */
export type InfoLine =
  | { kind: "text"; item: InfoItem; text: string; strong?: boolean; muted?: boolean }
  | { kind: "marks" };

/** 露出の項目（続けて並べたら 1 行にまとめる）。 */
const EXPOSURE: InfoItem[] = ["shutter", "aperture", "iso", "focal"];

/** 寸法（向きを反映した幅 × 高さ）。 */
function dims(s: InfoSource): string | null {
  if (s.dimensions && s.dimensions.width > 0 && s.dimensions.height > 0) {
    return `${s.dimensions.width.toLocaleString("ja-JP")} × ${s.dimensions.height.toLocaleString("ja-JP")}`;
  }
  const { width, height, orientation } = s.summary;
  if (!width || !height) return null;
  const [w, h] = orientation >= 5 ? [height, width] : [width, height];
  return `${w.toLocaleString("ja-JP")} × ${h.toLocaleString("ja-JP")}`;
}

/** 1 つの項目の文字（値がなければ null。詳細が要る項目で詳細がまだなければ "…"）。 */
export function itemText(item: InfoItem, s: InfoSource): string | null {
  const d = s.details && s.details.summary.variant_id === s.summary.variant_id ? s.details : null;
  const needDetails = INFO_ITEMS.find((i) => i.item === item)?.details;
  if (needDetails && !d) return "…";
  const v = (x: string) => (x === "—" ? null : x);
  switch (item) {
    case "file_name": {
      const s0 = s.summary;
      if (s0.is_master) return s0.file_name;
      return `${s0.file_name}（仮想コピー${s0.variant_name ? `: ${s0.variant_name}` : ""}）`;
    }
    case "capture_time":
      return formatCaptureTime(s.summary.capture);
    case "dimensions":
      return dims(s);
    case "position":
      return s.index !== null ? `${(s.index + 1).toLocaleString("ja-JP")} / ${s.count.toLocaleString("ja-JP")}` : null;
    case "camera":
      return s.summary.camera;
    case "lens":
      return s.summary.lens;
    case "focal":
      return v(formatFocal(d?.focal_mm));
    case "aperture":
      return v(formatAperture(d?.aperture));
    case "shutter":
      return v(formatShutter(d?.shutter_s));
    case "iso":
      return d?.iso ? `ISO ${d.iso.toLocaleString("ja-JP")}` : null;
    case "title":
      return d?.metadata?.title ?? null;
    case "caption":
      return d?.metadata?.caption ?? d?.caption ?? null;
    case "marks":
      return null;
  }
}

/** 表示する行（値のない項目は飛ばし、続けて並べた露出の項目は「・」でつないで 1 行にする）。 */
export function infoLines(items: InfoItem[], s: InfoSource): InfoLine[] {
  const out: InfoLine[] = [];
  let exposure: string[] = [];
  const flush = () => {
    if (exposure.length > 0) out.push({ kind: "text", item: "shutter", text: exposure.join("・") });
    exposure = [];
  };
  for (const item of items) {
    if (EXPOSURE.includes(item)) {
      const t = itemText(item, s);
      if (t !== null && !(t === "…" && exposure.includes("…"))) exposure.push(t);
      continue;
    }
    flush();
    if (item === "marks") {
      out.push({ kind: "marks" });
      continue;
    }
    const text = itemText(item, s);
    if (text === null || text === "") continue;
    out.push({ kind: "text", item, text, strong: item === "file_name", muted: item === "position" });
  }
  flush();
  return out;
}

// ---------------------------------------------------------------------------
// 状態（ルーペと現像の画面で共有）
// ---------------------------------------------------------------------------

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // 保存できなくても、この起動の間は使う。
  }
}

export interface InfoState {
  mode: InfoMode;
  sets: InfoSets;
}

export const infoStore = createStore<InfoState>({
  mode: typeof localStorage === "undefined" ? 1 : parseInfoMode(read(MODE_KEY), read(LEGACY_SHOW_KEY)),
  sets: typeof localStorage === "undefined" ? INFO_DEFAULTS : parseInfoSets(read(ITEMS_KEY)),
});

export function useInfo<T>(selector: (s: InfoState) => T, equal?: (a: T, b: T) => boolean): T {
  return useStore(infoStore, selector, equal);
}

/** I: 表示を切り替える。 */
export function cycleInfoMode(): InfoMode {
  const mode = nextInfoMode(infoStore.get().mode);
  infoStore.set({ mode });
  write(MODE_KEY, String(mode));
  return mode;
}

/** 情報 1・情報 2 の項目を変える（設定の「表示」）。 */
export function setInfoSets(sets: InfoSets): void {
  infoStore.set({ sets });
  write(ITEMS_KEY, JSON.stringify(sets));
}

/** 今の表示の項目（なしなら空）。 */
export function activeItems(s: InfoState): InfoItem[] {
  return s.mode === 1 ? s.sets.info1 : s.mode === 2 ? s.sets.info2 : [];
}
