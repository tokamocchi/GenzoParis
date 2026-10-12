// 表示用の書式（日本語）。

import type { CaptureTime, Flag, Rating } from "./api/types";

/** 撮影日時（記録されていたオフセット、なければ推定に使ったオフセットの現地時刻）。 */
export function formatCaptureTime(c: CaptureTime | null | undefined): string {
  if (!c || !c.utc) return "日時不明";
  const offset = c.offset && c.offset !== "Z" ? c.offset : c.tz_assumed;
  const minutes = parseOffset(offset);
  const t = new Date(Date.parse(c.utc) + (minutes ?? 0) * 60_000);
  const p = (n: number) => String(n).padStart(2, "0");
  const s = `${t.getUTCFullYear()}-${p(t.getUTCMonth() + 1)}-${p(t.getUTCDate())} ${p(t.getUTCHours())}:${p(
    t.getUTCMinutes(),
  )}:${p(t.getUTCSeconds())}`;
  return minutes === null ? `${s}（UTC）` : s;
}

/** "+09:00" → 540（分）。 */
export function parseOffset(s: string | null | undefined): number | null {
  if (!s) return null;
  if (s === "Z") return 0;
  const m = /^([+-])(\d{2}):?(\d{2})$/.exec(s);
  if (!m) return null;
  const v = Number(m[2]) * 60 + Number(m[3]);
  return m[1] === "-" ? -v : v;
}

/** シャッター速度（1/250 秒・2.5 秒）。 */
export function formatShutter(s: number | null | undefined): string {
  if (!s || s <= 0) return "—";
  if (s >= 0.3) return `${Number(s.toFixed(1))} 秒`;
  return `1/${Math.round(1 / s)} 秒`;
}

export function formatAperture(f: number | null | undefined): string {
  return f ? `f/${Number(f.toFixed(1))}` : "—";
}

export function formatFocal(mm: number | null | undefined): string {
  return mm ? `${Number(mm.toFixed(1))} mm` : "—";
}

export function formatBytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${i === 0 ? v : v.toFixed(v < 10 ? 2 : 1)} ${units[i]}`;
}

/** 動画の長さ（1:23・1:02:03）。 */
export function formatDuration(s: number | null | undefined): string {
  if (s === null || s === undefined || !Number.isFinite(s)) return "—";
  const t = Math.round(s);
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const sec = t % 60;
  const p = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${p(m)}:${p(sec)}` : `${m}:${p(sec)}`;
}

export function formatEv(v: number): string {
  const s = v.toFixed(2);
  return v > 0 ? `+${s}` : s;
}

export function formatSigned(v: number, digits = 0): string {
  const s = v.toFixed(digits);
  return v > 0 ? `+${s}` : s;
}

export function ratingStars(r: Rating): string {
  return "★".repeat(r) + "☆".repeat(5 - r);
}

export function flagLabel(f: Flag): string {
  return f === 1 ? "採用" : f === -1 ? "不採用" : "なし";
}

export const COLOR_LABEL_NAMES = {
  red: "赤",
  yellow: "黄",
  green: "緑",
  blue: "青",
  purple: "紫",
} as const;
