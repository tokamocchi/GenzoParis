// スポット修正（DEV-21。修復・コピースタンプ）の設定の組み立て・座標の変換（DOM を使わない純粋な計算）。
//
// - 位置・大きさ・コピー元のずれは O 座標（RAW の向きを反映した画像の長辺を 1 とする座標。genzo-model の
//   develop::retouch）。角度補正・歪曲補正・切り抜きを後から変えても、スポットは写っているもの（ゴミ・電線）の上から
//   ずれない。画面の画像（CSS の画素の箱）との対応は、その画像の FrameInfo.o_grid（画像の上の 33 × 33 点の O 座標）を
//   双線形補間して求める（fracToO・oToFrac）。古いコアで表が無ければ、G 座標（g_bounds）と同じとみなす（回転・歪曲
//   補正がなければ O と G は同じ）。
// - クリックで円、ドラッグで線（電線など）。コピー元のずれはコアに選ばせる（retouch_auto_source）。選べなければ、大きさの
//   2.5 倍だけ右（点が右半分にあれば左）にずらす。
// - 値の範囲・数の上限はコア（genzo-model の validate）と同じ。超える値はコアが受け付けないので、ここで丸める・断る。
// - 処理バージョン 1 の写真では使われない（コアは警告を出して無視する）。パネルは更新を勧める。

import type { DevelopSettings, RetouchMode, RetouchSpot } from "../api/types";
import { ORIENTED_GRID_SIZE, RANGES, RETOUCH_LIMITS } from "../api/types";
import { type Box, type GBounds, simplifyPoints } from "./maskLogic";

/** スポット修正を使える最初の処理バージョン（genzo-pipeline の ProcessVersion::V2）。 */
export const RETOUCH_PROCESS_VERSION = 2;

/** 直し方の名前。 */
export const MODE_LABELS: Record<RetouchMode, string> = {
  heal: "修復",
  clone: "コピースタンプ",
};

/** 直し方の説明。 */
export const MODE_TITLES: Record<RetouchMode, string> = {
  heal: "コピー元の模様を写し、明るさと色をまわりになじませます（ゴミ・電線など）",
  clone: "コピー元をそのまま写します（縁だけぼかします）",
};

/** 新しいスポットの設定（道具の設定）。 */
export interface RetouchOptions {
  mode: RetouchMode;
  /** 大きさ（半径。O 座標）。 */
  size: number;
  feather: number;
  opacity: number;
}

export const DEFAULT_RETOUCH: RetouchOptions = { mode: "heal", size: 0.01, feather: 50, opacity: 100 };

/** 自動で選べなかったときのずれ（大きさの倍数）。 */
export const FALLBACK_OFFSET_FACTOR = 2.5;

const round5 = (v: number) => Math.round(v * 100000) / 100000;
const clampTo = (v: number, r: { min: number; max: number }) => Math.min(r.max, Math.max(r.min, v));

/** 大きさを範囲に収める。 */
export function retouchSize(v: number): number {
  return round5(clampTo(v, RANGES.retouchSize));
}

/** ずれを範囲に収める。 */
export function retouchOffset(d: [number, number]): [number, number] {
  return [round5(clampTo(d[0], RANGES.retouchOffset)), round5(clampTo(d[1], RANGES.retouchOffset))];
}

// ---------------------------------------------------------------------------
// パネルの状態
// ---------------------------------------------------------------------------

export interface RetouchControls {
  /** 処理バージョンが 1 で使われない。 */
  needsUpgrade: boolean;
  /** 編集できる。 */
  editable: boolean;
  /** スポットを足せる（上限まで）。 */
  canAdd: boolean;
}

export function retouchControls(s: DevelopSettings, disabled: boolean): RetouchControls {
  const needsUpgrade = s.process_version < RETOUCH_PROCESS_VERSION;
  const editable = !disabled && !needsUpgrade;
  return { needsUpgrade, editable, canAdd: editable && spotsOf(s).length < RETOUCH_LIMITS.spots };
}

/** 設定のスポット（古いコア・空の設定では欠ける）。 */
export function spotsOf(s: DevelopSettings | null | undefined): RetouchSpot[] {
  return s?.retouch ?? [];
}

/** スポットの要約（「円」「線 12 点」）。 */
export function spotSummary(r: RetouchSpot): string {
  return r.points.length >= 2 ? `線（${r.points.length} 点）` : "円";
}

/** スポットの表示の名前（1 から数えた番号）。 */
export function spotLabel(index: number): string {
  return `スポット ${index + 1}`;
}

// ---------------------------------------------------------------------------
// 設定の組み立て
// ---------------------------------------------------------------------------

/** 新しいスポット（点は間引く。間引きの距離は大きさの 1/4）。ずれは後で入れる。 */
export function newSpot(points: [number, number][], o: RetouchOptions): RetouchSpot {
  const size = retouchSize(o.size);
  return {
    enabled: true,
    mode: o.mode,
    points: simplifyPoints(points, size / 4, RETOUCH_LIMITS.points),
    size,
    feather: clampTo(Math.round(o.feather), RANGES.amount),
    opacity: clampTo(Math.round(o.opacity), RANGES.amount),
    offset: [0, 0],
  };
}

/** 自動で選べなかったときのずれ（大きさの 2.5 倍だけ右。点が右半分にあれば左）。 */
export function fallbackOffset(spot: RetouchSpot, oWidth = 1): [number, number] {
  const d = spot.size * FALLBACK_OFFSET_FACTOR;
  const right = (spot.points[0]?.[0] ?? 0) > oWidth / 2;
  return retouchOffset([right ? -d : d, 0]);
}

/** スポットの点の数の合計。 */
export function totalRetouchPoints(s: DevelopSettings): number {
  return spotsOf(s).reduce((n, r) => n + r.points.length, 0);
}

/** 点を順に結んだ折れ線の長さ（O 座標。円なら 0）。 */
export function strokeLength(points: [number, number][]): number {
  let n = 0;
  for (let i = 1; i < points.length; i++) n += Math.hypot(points[i][0] - points[i - 1][0], points[i][1] - points[i - 1][1]);
  return n;
}

/** スポットを足す。足せなければ理由を返す（点がない・数・点の数・線の長さがコアの上限を超える）。 */
export function addSpot(s: DevelopSettings, spot: RetouchSpot): { settings: DevelopSettings } | { error: string } {
  const spots = spotsOf(s);
  if (spot.points.length === 0) return { error: "スポットに点がありません" };
  if (strokeLength(spot.points) > RETOUCH_LIMITS.strokeLength) {
    return { error: `線が長すぎます（写真の長辺の ${RETOUCH_LIMITS.strokeLength} 倍まで）` };
  }
  if (spots.length >= RETOUCH_LIMITS.spots) {
    return { error: `スポットが多すぎます（${RETOUCH_LIMITS.spots.toLocaleString("ja-JP")} 個まで）` };
  }
  if (totalRetouchPoints(s) + spot.points.length > RETOUCH_LIMITS.totalPoints) {
    return {
      error: `スポットの点が多すぎます（全体で ${RETOUCH_LIMITS.totalPoints.toLocaleString("ja-JP")} 点まで）`,
    };
  }
  return { settings: { ...s, retouch: [...spots, spot] } };
}

/** スポット `i` を変える（番号が範囲の外なら、そのままの設定を返す）。 */
export function withSpot(
  s: DevelopSettings,
  i: number,
  fn: (r: RetouchSpot) => RetouchSpot,
): DevelopSettings {
  const spots = spotsOf(s);
  if (i < 0 || i >= spots.length) return s;
  const next = fn(spots[i]);
  if (next === spots[i]) return s;
  return { ...s, retouch: spots.map((r, k) => (k === i ? next : r)) };
}

/** スポット `i` を除く。 */
export function removeSpot(s: DevelopSettings, i: number): DevelopSettings {
  const spots = spotsOf(s);
  if (i < 0 || i >= spots.length) return s;
  return { ...s, retouch: spots.filter((_, k) => k !== i) };
}

/** スポットをすべて除く。 */
export function clearSpots(s: DevelopSettings): DevelopSettings {
  return spotsOf(s).length === 0 ? s : { ...s, retouch: [] };
}

/** スポットを O 座標で (du, dv) だけ動かしたもの（ドラッグを始めたときのスポット `start` から。コピー元も一緒に動く）。 */
export function moveSpot(start: RetouchSpot, du: number, dv: number): RetouchSpot {
  const c = (v: number) => round5(clampTo(v, RANGES.retouchCoord));
  return { ...start, points: start.points.map(([x, y]) => [c(x + du), c(y + dv)]) };
}

/** コピー元を O 座標で (du, dv) だけ動かしたもの（ドラッグを始めたときのスポット `start` から）。 */
export function moveSource(start: RetouchSpot, du: number, dv: number): RetouchSpot {
  return { ...start, offset: retouchOffset([start.offset[0] + du, start.offset[1] + dv]) };
}

// ---------------------------------------------------------------------------
// 座標（O 座標 ↔ 画面の画像の箱）
// ---------------------------------------------------------------------------

/** 画像の上の点の O 座標の表（FrameInfo.o_grid。一辺 n 点）。 */
export interface OGrid {
  n: number;
  points: [number, number][];
}

/**
 * 画像の上の点の O 座標の表。コアの表が正しければそれ、無ければ（古いコア）G 座標の範囲 `gb` から作る（回転・歪曲補正が
 * なければ O は G と同じ）。
 */
export function orientedGrid(o: [number, number][] | null | undefined, gb: GBounds): OGrid {
  const n = ORIENTED_GRID_SIZE;
  if (o && o.length === n * n && o.every(([x, y]) => Number.isFinite(x) && Number.isFinite(y))) return { n, points: o };
  const points: [number, number][] = [];
  for (let j = 0; j < n; j++) {
    for (let i = 0; i < n; i++) {
      points.push([gb[0] + (i / (n - 1)) * (gb[2] - gb[0]), gb[1] + (j / (n - 1)) * (gb[3] - gb[1])]);
    }
  }
  return { n, points };
}

/** 画像の幅・高さに対する割合 (fx, fy) の O 座標（表の双線形補間。表の外は端の升目を延長する）。 */
export function fracToO(g: OGrid, fx: number, fy: number): [number, number] {
  const k = g.n - 1;
  const ci = Math.min(k - 1, Math.max(0, Math.floor(fx * k)));
  const cj = Math.min(k - 1, Math.max(0, Math.floor(fy * k)));
  const tx = fx * k - ci;
  const ty = fy * k - cj;
  const p = (i: number, j: number) => g.points[j * g.n + i];
  const [a, b, c, d] = [p(ci, cj), p(ci + 1, cj), p(ci, cj + 1), p(ci + 1, cj + 1)];
  const lerp = (u: number, v: number, t: number) => u + (v - u) * t;
  return [
    lerp(lerp(a[0], b[0], tx), lerp(c[0], d[0], tx), ty),
    lerp(lerp(a[1], b[1], tx), lerp(c[1], d[1], tx), ty),
  ];
}

/**
 * O 座標の点の、画像の幅・高さに対する割合（fracToO の逆。ニュートン法。画像の外の点も、端の升目の延長で求める）。
 * 求まらなければ（表がつぶれているなど）null。
 */
export function oToFrac(g: OGrid, u: number, v: number): [number, number] | null {
  // 最初の値: 表の左上・右上・左下の 3 点の平行四辺形で解く。
  const n = g.n;
  const p00 = g.points[0];
  const p10 = g.points[n - 1];
  const p01 = g.points[(n - 1) * n];
  let [fx, fy] = solve2(p10[0] - p00[0], p01[0] - p00[0], p10[1] - p00[1], p01[1] - p00[1], u - p00[0], v - p00[1]) ?? [0.5, 0.5];
  const h = 1e-4;
  for (let it = 0; it < 12; it++) {
    const [x, y] = fracToO(g, fx, fy);
    const ex = u - x;
    const ey = v - y;
    if (Math.hypot(ex, ey) < 1e-9) break;
    const [xa, ya] = fracToO(g, fx + h, fy);
    const [xb, yb] = fracToO(g, fx, fy + h);
    const step = solve2((xa - x) / h, (xb - x) / h, (ya - y) / h, (yb - y) / h, ex, ey);
    if (!step) return null;
    fx += step[0];
    fy += step[1];
  }
  return Number.isFinite(fx) && Number.isFinite(fy) ? [fx, fy] : null;
}

/** 2 × 2 の連立一次方程式 [[a, b], [c, d]]·[x, y] = [e, f] を解く（解けなければ null）。 */
function solve2(a: number, b: number, c: number, d: number, e: number, f: number): [number, number] | null {
  const det = a * d - b * c;
  if (!(Math.abs(det) > 1e-15)) return null;
  return [(e * d - b * f) / det, (a * f - e * c) / det];
}

/** 位置（CSS の画素）が画像の箱の中か（縁を含む）。 */
export function insideBox(box: Box, x: number, y: number): boolean {
  return x >= box.left && x <= box.left + box.width && y >= box.top && y <= box.top + box.height;
}

/** 箱の中の位置（CSS の画素）を O 座標にする。 */
export function boxToO(g: OGrid, box: Box, x: number, y: number): [number, number] {
  return fracToO(g, (x - box.left) / box.width, (y - box.top) / box.height);
}

/** O 座標の点を、箱の中の位置（CSS の画素）にする（求まらなければ null）。 */
export function oToBox(g: OGrid, box: Box, u: number, v: number): [number, number] | null {
  const f = oToFrac(g, u, v);
  return f ? [box.left + f[0] * box.width, box.top + f[1] * box.height] : null;
}

/**
 * O 座標の長さ 1 が、点 (u, v) のあたりで箱の中の何画素か（表の局所の倍率。回転では向きによらず同じで、歪曲補正では
 * 場所によって少し変わる）。求まらなければ 0。
 */
export function pixelsPerO(g: OGrid, box: Box, u: number, v: number): number {
  const a = oToBox(g, box, u, v);
  const b = oToBox(g, box, u + 0.01, v);
  const c = oToBox(g, box, u, v + 0.01);
  if (!a || !b || !c) return 0;
  return (Math.hypot(b[0] - a[0], b[1] - a[1]) + Math.hypot(c[0] - a[0], c[1] - a[1])) / 2 / 0.01;
}
