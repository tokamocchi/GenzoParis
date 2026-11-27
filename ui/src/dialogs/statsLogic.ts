// 撮影の統計（左のパネルの「カタログ」の「統計」）の、表示の行の計算（DOM を使わない）。
//
// コア（shooting_stats）は、条件に合う写真・動画を、カメラ・レンズ・焦点距離・ISO・絞り・撮影年ごとに数えて返す
// （仮想コピーは重ねて数えない。レンズ・焦点距離・ISO・絞りは写真だけ）。ここでは、項目ごとに上位だけを残して
// 「ほか」にまとめ、「不明」を足し、棒の長さ（項目の中で最も多い数を 1 とする割合）を付ける。

import type { ShootingStats, StatBreakdown } from "../api/types";

/** 項目ごとに出す行の数の上限（**仮置き**: 12。残りは「ほか」にまとめる）。 */
export const STATS_TOP = 12;

export interface StatRow {
  label: string;
  count: number;
  /** 棒の長さ（0〜1。項目の中で最も多い数を 1 とする）。 */
  fraction: number;
  /** 項目の合計に対する割合（%。小数第 1 位に丸めた値）。 */
  percent: number;
  /** 値そのものではない行（ほか・不明）。 */
  note?: boolean;
}

/**
 * 1 つの項目の表示の行。`byCount` なら数の多い順の上位 `top` 件を残して、残りを「ほか」にまとめる（カメラ・レンズ）。
 * そうでなければ、渡された順（区切り・年の順）のまますべて出す。値がなかった数は最後に「不明」として出す。
 */
export function statRows(b: StatBreakdown, byCount: boolean, top = STATS_TOP): StatRow[] {
  const total = b.items.reduce((n, c) => n + c.count, 0) + b.unknown;
  if (total <= 0) return [];
  const shown = byCount && top > 0 ? b.items.slice(0, top) : b.items;
  const rest = b.items.slice(shown.length);
  const rows: { label: string; count: number; note?: boolean }[] = shown.map((c) => ({ label: c.label, count: c.count }));
  if (rest.length > 0) {
    rows.push({
      label: `ほか ${rest.length.toLocaleString("ja-JP")} 種類`,
      count: rest.reduce((n, c) => n + c.count, 0),
      note: true,
    });
  }
  if (b.unknown > 0) rows.push({ label: "不明", count: b.unknown, note: true });
  const max = rows.reduce((m, r) => Math.max(m, r.count), 0);
  return rows.map((r) => ({
    ...r,
    fraction: max > 0 ? r.count / max : 0,
    percent: Math.round((r.count / total) * 1000) / 10,
  }));
}

export interface StatSection {
  key: string;
  title: string;
  /** 数える対象の説明（見出しの横に小さく出す）。 */
  hint: string;
  rows: StatRow[];
}

/** ダイアログに出す項目（行のない項目は出さない）。 */
export function statSections(s: ShootingStats, top = STATS_TOP): StatSection[] {
  const sections: StatSection[] = [
    { key: "cameras", title: "カメラ", hint: "写真と動画", rows: statRows(s.cameras, true, top) },
    { key: "lenses", title: "レンズ", hint: "写真", rows: statRows(s.lenses, true, top) },
    { key: "focal_lengths", title: "焦点距離", hint: "写真。実焦点距離", rows: statRows(s.focal_lengths, false) },
    { key: "isos", title: "ISO", hint: "写真", rows: statRows(s.isos, false) },
    { key: "apertures", title: "絞り", hint: "写真", rows: statRows(s.apertures, false) },
    { key: "years", title: "撮影年", hint: "写真と動画", rows: statRows(s.years, false) },
  ];
  return sections.filter((x) => x.rows.length > 0);
}

/** 見出しの文（写真・動画の数）。 */
export function statsSummary(s: ShootingStats): string {
  const n = (v: number) => v.toLocaleString("ja-JP");
  if (s.photos === 0 && s.videos === 0) return "条件に合う写真・動画はありません";
  return `写真 ${n(s.photos)} 枚、動画 ${n(s.videos)} 本（仮想コピーは重ねて数えません）`;
}
