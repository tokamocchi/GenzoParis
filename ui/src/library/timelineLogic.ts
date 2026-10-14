// 写真と動画のタイムライン（ORG-01）の見出しの文と、年・月へのジャンプの一覧（DOM を使わない純粋な計算）。
//
// - まとまり（撮影日の中のイベント）はコアが作る（genzo-api の timeline.rs）。画面はその並びを行の表にして
//   （gridMath の withTimeline）、日付の見出し・イベントの見出しを出す。
// - 日付の見出しは「2024 年 5 月 1 日（水）」と、その日の写真・動画の数（イベントが 1 つなら時刻の範囲も）。
//   イベントの見出しは時刻の範囲と数。時刻は写真ごとの表示のタイムゾーンの現地時刻。
// - 年・月へのジャンプ（monthIndex）は、まとまりの並びに最初に現れた順（新しい順の表示では新しい月から）。

import type { TimelineGroup } from "../api/types";

/** タイムラインの区切りの間隔の選択肢（秒。0 はイベントで分けない。**仮置き**）。 */
export const TIMELINE_GAP_OPTIONS: { value: number; label: string }[] = [
  { value: 0, label: "日付だけ" },
  { value: 30 * 60, label: "30 分" },
  { value: 60 * 60, label: "1 時間" },
  { value: 2 * 60 * 60, label: "2 時間" },
  { value: 4 * 60 * 60, label: "4 時間" },
  { value: 8 * 60 * 60, label: "8 時間" },
];

/** イベントの区切りの間隔の既定値（秒。**仮置き**: 2 時間）。 */
export const DEFAULT_TIMELINE_GAP = 2 * 60 * 60;

const WEEKDAYS = ["日", "月", "火", "水", "木", "金", "土"];

/** "2024-05-01" → "2024 年 5 月 1 日（水）"（読めなければそのまま）。 */
export function dayLabel(day: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  if (!m) return day;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  const w = new Date(Date.UTC(y, mo - 1, d)).getUTCDay();
  return `${y} 年 ${mo} 月 ${d} 日（${WEEKDAYS[w]}）`;
}

/** 時刻の範囲（"09:00〜11:40"。早いほうを先に。同じ分なら 1 つ）。分からなければ null。 */
export function timeRange(first: string | null, last: string | null): string | null {
  const a = first?.slice(11, 16);
  const b = last?.slice(11, 16);
  if (!a || !b) return a ?? b ?? null;
  const [lo, hi] = (first ?? "") <= (last ?? "") ? [a, b] : [b, a];
  return lo === hi ? lo : `${lo}〜${hi}`;
}

/** 写真と動画の数の文（"写真 12・動画 3"、どちらかだけなら "12 枚"・"3 本"）。 */
export function countsLabel(photos: number, videos: number): string {
  const n = (v: number) => v.toLocaleString("ja-JP");
  if (videos === 0) return `${n(photos)} 枚`;
  if (photos === 0) return `動画 ${n(videos)} 本`;
  return `写真 ${n(photos)}・動画 ${n(videos)}`;
}

/** 日（日付の見出しのまとまりから次の見出しの前まで）のまとめ。 */
export interface DaySummary {
  photos: number;
  videos: number;
  events: number;
  /** 最も早い・遅い現地時刻。 */
  first: string | null;
  last: string | null;
}

/** `groups[from]` から始まる日のまとめ（`dayStart` は日付の見出しを出すまとまりの印）。 */
export function daySummary(groups: readonly TimelineGroup[], dayStart: ArrayLike<number>, from: number): DaySummary {
  const out: DaySummary = { photos: 0, videos: 0, events: 0, first: null, last: null };
  for (let i = from; i < groups.length && (i === from || !dayStart[i]); i++) {
    const g = groups[i];
    out.photos += g.photos;
    out.videos += g.videos;
    out.events++;
    for (const t of [g.first, g.last]) {
      if (t === null) continue;
      if (out.first === null || t < out.first) out.first = t;
      if (out.last === null || t > out.last) out.last = t;
    }
  }
  return out;
}

/** 日付の見出しの文（日付と数。イベントが 1 つなら時刻の範囲も）。撮影日時のないまとまりは「撮影日時なし」。 */
export function dayHeaderText(groups: readonly TimelineGroup[], dayStart: ArrayLike<number>, g: number): {
  title: string;
  meta: string;
} {
  const grp = groups[g];
  const sum = daySummary(groups, dayStart, g);
  const counts = countsLabel(sum.photos, sum.videos);
  if (grp.day === null) return { title: "撮影日時なし", meta: counts };
  const range = sum.events === 1 ? timeRange(sum.first, sum.last) : null;
  return {
    title: dayLabel(grp.day),
    meta: range ? `${range}・${counts}` : `${counts}・${sum.events} のまとまり`,
  };
}

/** イベントの見出しの文（時刻の範囲と数）。 */
export function eventHeaderText(g: TimelineGroup): string {
  const range = timeRange(g.first, g.last);
  const counts = countsLabel(g.photos, g.videos);
  return range ? `${range}・${counts}` : counts;
}

/** 年・月へのジャンプの 1 件。 */
export interface MonthEntry {
  /** "2024-05"。 */
  key: string;
  year: number;
  month: number;
  /** 最初に現れたまとまり。 */
  group: number;
  /** その月の写真・動画の数（まとまりの並びの中の全部）。 */
  count: number;
}

/** 年・月の一覧（最初に現れた順。撮影日時のないまとまりは含めない）。 */
export function monthIndex(groups: readonly TimelineGroup[]): MonthEntry[] {
  const out: MonthEntry[] = [];
  const at = new Map<string, MonthEntry>();
  groups.forEach((g, i) => {
    if (!g.day) return;
    const key = g.day.slice(0, 7);
    const hit = at.get(key);
    if (hit) {
      hit.count += g.count;
      return;
    }
    const e = { key, year: Number(key.slice(0, 4)), month: Number(key.slice(5, 7)), group: i, count: g.count };
    at.set(key, e);
    out.push(e);
  });
  return out;
}

/** 年ごとにまとめた月の一覧（ジャンプの選択肢の optgroup）。年も最初に現れた順。 */
export function monthsByYear(months: readonly MonthEntry[]): { year: number; months: MonthEntry[] }[] {
  const out: { year: number; months: MonthEntry[] }[] = [];
  for (const m of months) {
    let y = out.find((x) => x.year === m.year);
    if (!y) {
      y = { year: m.year, months: [] };
      out.push(y);
    }
    y.months.push(m);
  }
  return out;
}
