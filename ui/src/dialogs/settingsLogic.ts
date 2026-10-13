// 設定のダイアログ（SYS-05）の値の変換と検証（タイムゾーンの表示・キャッシュの上限の GB）。DOM を使わない
// 純粋な計算。

import type { CoreSettings, SettingsUpdate } from "../api/types";
import { formatCount } from "./dialogFormat";

/** 既定のタイムゾーンとして受け付ける範囲（分。genzo-api の UTC_OFFSET_RANGE_MINUTES: −14:00〜+14:00）。 */
export const UTC_OFFSET_MIN_MINUTES = -14 * 60;
export const UTC_OFFSET_MAX_MINUTES = 14 * 60;

/** タイムゾーンの選択肢の刻み（15 分。:30・:45 のタイムゾーンも選べる）。 */
export const UTC_OFFSET_STEP_MINUTES = 15;

/** オフセットの表示（"UTC+09:00"・"UTC-03:30"。genzo-api の settings.rs の offset_label と同じ形）。 */
export function formatUtcOffset(minutes: number): string {
  const sign = minutes < 0 ? "-" : "+";
  const m = Math.abs(Math.trunc(minutes));
  const p = (n: number) => String(n).padStart(2, "0");
  return `UTC${sign}${p(Math.floor(m / 60))}:${p(m % 60)}`;
}

/** タイムゾーンの選択肢（15 分刻み。今の値が刻みに乗っていなければ加える）。 */
export function utcOffsetOptions(current?: number | null): number[] {
  const out: number[] = [];
  for (let m = UTC_OFFSET_MIN_MINUTES; m <= UTC_OFFSET_MAX_MINUTES; m += UTC_OFFSET_STEP_MINUTES) out.push(m);
  if (
    current !== undefined &&
    current !== null &&
    Number.isInteger(current) &&
    current >= UTC_OFFSET_MIN_MINUTES &&
    current <= UTC_OFFSET_MAX_MINUTES &&
    !out.includes(current)
  ) {
    out.push(current);
    out.sort((a, b) => a - b);
  }
  return out;
}

/** L1 プレビューのキャッシュの上限の最小値（バイト。genzo-api の MIN_PREVIEW_CACHE_BYTES。仮置きの値）。 */
export const MIN_PREVIEW_CACHE_BYTES = 100_000_000;
/** 画面の GB（10 億バイト。genzo-catalog の既定の上限 20,000,000,000 バイトが「20 GB」になる単位）。 */
export const BYTES_PER_GB = 1_000_000_000;
/** 画面で受け付ける上限（仮置き: 1,000,000 GB。JavaScript の整数で正確に表せる範囲に収める）。 */
export const MAX_PREVIEW_CACHE_GB = 1_000_000;

/** バイト → GB の入力欄の文字列（小数第 2 位まで。末尾の 0 は省く）。 */
export function bytesToGbText(bytes: number): string {
  const gb = bytes / BYTES_PER_GB;
  return String(Number(gb.toFixed(2)));
}

/** 動画の再生用のファイルのキャッシュの上限の最小値（バイト。genzo-api の MIN_VIDEO_PROXY_CACHE_BYTES。仮置きの値）。 */
export const MIN_VIDEO_PROXY_CACHE_BYTES = 500_000_000;
/** 動画の再生用のファイルのキャッシュの上限の既定値（バイト。genzo-api の DEFAULT_VIDEO_PROXY_CAPACITY_BYTES）。 */
export const DEFAULT_VIDEO_PROXY_CACHE_BYTES = 5_000_000_000;

/** 大きさの表示（10 進の単位。上限の GB と同じく 1 GB = 10 億バイト。"12.3 MB"・"1.25 GB"）。 */
export function formatDecimalSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1000) return `${bytes} バイト`;
  const units: [number, string][] = [
    [BYTES_PER_GB, "GB"],
    [1_000_000, "MB"],
    [1_000, "KB"],
  ];
  for (const [unit, label] of units) {
    if (bytes >= unit) return `${Number((bytes / unit).toFixed(2))} ${label}`;
  }
  return `${bytes} バイト`;
}

/** GB の入力を読む（全角の数字も受け付ける）。範囲外（`minBytes` 未満など）・数でなければ理由を返す。 */
export function parseCacheGb(text: string, minBytes: number = MIN_PREVIEW_CACHE_BYTES): { bytes: number } | { error: string } {
  const t = text
    .trim()
    .replace(/[０-９]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0))
    .replace(/[．]/g, ".")
    .replace(/[,，]/g, "");
  if (!/^\d+(\.\d+)?$/.test(t)) return { error: "数で指定してください（GB）" };
  const gb = Number(t);
  const min = minBytes / BYTES_PER_GB;
  const bytes = Math.round(gb * BYTES_PER_GB);
  if (!Number.isFinite(gb) || bytes < minBytes) {
    return { error: `${min} GB（${formatCount(minBytes)} バイト）以上にしてください` };
  }
  if (gb > MAX_PREVIEW_CACHE_GB) return { error: `${formatCount(MAX_PREVIEW_CACHE_GB)} GB 以下にしてください` };
  return { bytes };
}

/** 画面の入力（カタログに保存する設定）。 */
export interface CatalogSettingsForm {
  cacheDir: string;
  cacheGb: string;
  /** 動画の再生用のファイルのキャッシュの上限（GB。VID-04）。 */
  proxyGb: string;
  exportColorSpace: CoreSettings["default_export_color_space"];
  utcOffsetMinutes: number;
  /** 取り込みの既定: 重複と思われるものを取り込まない（IMP-04）。 */
  importSkipDuplicates: boolean;
  /** 取り込みの既定: RAW と同時記録の JPEG を別々の写真として取り込む（IMP-06）。 */
  importSeparateRawJpeg: boolean;
  /** 内容のハッシュをバックグラウンドで計算する（ORG-03）。 */
  hashInBackground: boolean;
  /** バックグラウンドの計算で、ネットワークのボリュームのファイルも読む。 */
  hashNetworkVolumes: boolean;
}

export function catalogSettingsForm(s: CoreSettings): CatalogSettingsForm {
  return {
    cacheDir: s.preview_cache_dir,
    cacheGb: bytesToGbText(s.preview_cache_bytes),
    proxyGb: bytesToGbText(s.video_proxy_cache_bytes ?? DEFAULT_VIDEO_PROXY_CACHE_BYTES),
    exportColorSpace: s.default_export_color_space,
    utcOffsetMinutes: s.default_utc_offset_minutes,
    // 古いコア（値がない）では genzo-api の既定。
    importSkipDuplicates: s.import_skip_duplicates ?? true,
    importSeparateRawJpeg: s.import_separate_raw_jpeg ?? false,
    hashInBackground: s.hash_in_background ?? true,
    hashNetworkVolumes: s.hash_network_volumes ?? false,
  };
}

/**
 * 変わった項目だけの SettingsUpdate を作る（変わっていなければ null の項目は送らない）。入力が不正なら
 * 理由を返す。
 */
export function catalogSettingsUpdate(
  current: CoreSettings,
  form: CatalogSettingsForm,
): { update: SettingsUpdate; changed: boolean } | { error: string } {
  const update: SettingsUpdate = {};
  const dir = form.cacheDir.trim();
  if (dir === "") return { error: "キャッシュの場所を指定してください" };
  if (dir !== current.preview_cache_dir) update.preview_cache_dir = dir;
  const parsed = parseCacheGb(form.cacheGb);
  if ("error" in parsed) return { error: `キャッシュの上限: ${parsed.error}` };
  // 入力欄の表示（小数第 2 位）に丸めた値と同じなら、変えていない（丸めで上限が変わらないように）。
  if (form.cacheGb.trim() !== bytesToGbText(current.preview_cache_bytes) && parsed.bytes !== current.preview_cache_bytes) {
    update.preview_cache_bytes = parsed.bytes;
  }
  const proxyCurrent = current.video_proxy_cache_bytes ?? DEFAULT_VIDEO_PROXY_CACHE_BYTES;
  const proxy = parseCacheGb(form.proxyGb, MIN_VIDEO_PROXY_CACHE_BYTES);
  if ("error" in proxy) return { error: `動画の再生用のファイルの上限: ${proxy.error}` };
  if (form.proxyGb.trim() !== bytesToGbText(proxyCurrent) && proxy.bytes !== proxyCurrent) {
    update.video_proxy_cache_bytes = proxy.bytes;
  }
  if (form.exportColorSpace !== current.default_export_color_space) {
    update.default_export_color_space = form.exportColorSpace;
  }
  if (form.utcOffsetMinutes !== current.default_utc_offset_minutes) {
    if (form.utcOffsetMinutes < UTC_OFFSET_MIN_MINUTES || form.utcOffsetMinutes > UTC_OFFSET_MAX_MINUTES) {
      return { error: "タイムゾーンは UTC-14:00〜UTC+14:00 の範囲で選んでください" };
    }
    update.default_utc_offset_minutes = form.utcOffsetMinutes;
  }
  const before = catalogSettingsForm(current);
  if (form.importSkipDuplicates !== before.importSkipDuplicates) update.import_skip_duplicates = form.importSkipDuplicates;
  if (form.importSeparateRawJpeg !== before.importSeparateRawJpeg) {
    update.import_separate_raw_jpeg = form.importSeparateRawJpeg;
  }
  if (form.hashInBackground !== before.hashInBackground) update.hash_in_background = form.hashInBackground;
  if (form.hashNetworkVolumes !== before.hashNetworkVolumes) update.hash_network_volumes = form.hashNetworkVolumes;
  return { update, changed: Object.keys(update).length > 0 };
}
