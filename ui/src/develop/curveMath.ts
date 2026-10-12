// トーンカーブの計算と点の編集（DEV-07。DOM を使わない純粋な計算）。
//
// - 曲線の形は genzo-pipeline の finish/tone_curve.rs と同じにする（画面に描く曲線と、実際の現像の結果を
//   一致させるため）:
//   - 点のカーブ: 点を通る単調な 3 次スプライン（Fritsch–Carlson。`MonotoneCubic`）。最初の点より左・最後の
//     点より右は端の点の y で一定。
//   - パラメトリック: (0, 0)・(0.125, …)・(0.375, …)・(0.625, …)・(0.875, …)・(1, 1) の 6 点を同じ
//     スプラインで結ぶ。中の 4 点はスライダー ±100 で ±PARAMETRIC_AMPLITUDE（0.1）だけ上下する。
//   - 輝度のカーブは「点のカーブ ∘ パラメトリック」（パラメトリックを先に適用する）。
//   - 空のカーブ、または 0 と 1 を端点に持ち、すべての点が y = x の上にあるカーブは恒等。
// - 点の約束（genzo-model の validate.rs）: 座標は 0〜1、x は厳密に増加、点は 2 点以上（1 点は不正）で、
//   最大 MAX_CURVE_POINTS（64）点。画面では、両端の点の x を固定し（新しく作るカーブは 0 と 1）、y だけを
//   動かせるようにする。

import type { CurvePoint, ParametricCurve, ToneCurve } from "../api/types";

/** 1 本のカーブの点の数の上限（genzo-model の MAX_CURVE_POINTS）。 */
export const MAX_CURVE_POINTS = 64;

/** パラメトリックのスライダー ±100 での、領域の中央の点の上下の量（genzo-pipeline の PARAMETRIC_AMPLITUDE）。 */
export const PARAMETRIC_AMPLITUDE = 0.1;

/** パラメトリックの 4 つの領域の中央（シャドウ・ダーク・ライト・ハイライトの順。PARAMETRIC_CENTERS）。 */
export const PARAMETRIC_CENTERS = [0.125, 0.375, 0.625, 0.875] as const;

/**
 * 隣の点との x の最小の間隔（仮置き）。コアは f32 で x の厳密な増加を確かめるので、f32 に丸めても
 * 重ならない十分な間隔にする。
 */
export const MIN_POINT_GAP = 0.002;

/** 点のカーブの種類（設定の tone_curve の項目の名前）。 */
export type CurveChannel = "luma" | "red" | "green" | "blue";

export const CURVE_CHANNELS: { key: CurveChannel; label: string }[] = [
  { key: "luma", label: "RGB" },
  { key: "red", label: "R" },
  { key: "green", label: "G" },
  { key: "blue", label: "B" },
];

const clamp01 = (v: number) => (Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0);

/** 単調な 3 次スプライン（Fritsch–Carlson。genzo-pipeline の MonotoneCubic と同じ式）。 */
export class MonotoneCubic {
  private readonly xs: number[];
  private readonly ys: number[];
  private readonly ms: number[];

  /** 点 [x, y] から作る（2 点以上で、x が厳密に増加し、すべて有限であること。そうでなければ null）。 */
  static create(points: readonly (readonly [number, number])[]): MonotoneCubic | null {
    const pts = points.map((p): [number, number] => [p[0], p[1]]);
    if (pts.length < 2) return null;
    for (const [x, y] of pts) if (!Number.isFinite(x) || !Number.isFinite(y)) return null;
    for (let i = 1; i < pts.length; i++) if (pts[i][0] <= pts[i - 1][0]) return null;
    return new MonotoneCubic(pts);
  }

  private constructor(pts: [number, number][]) {
    const xs = pts.map((p) => p[0]);
    const ys = pts.map((p) => p[1]);
    const n = xs.length;
    const delta: number[] = [];
    for (let k = 0; k < n - 1; k++) delta.push((ys[k + 1] - ys[k]) / (xs[k + 1] - xs[k]));
    const ms = new Array<number>(n).fill(0);
    ms[0] = delta[0];
    ms[n - 1] = delta[n - 2];
    for (let k = 1; k < n - 1; k++) {
      ms[k] = delta[k - 1] * delta[k] > 0 ? (delta[k - 1] + delta[k]) * 0.5 : 0;
    }
    for (let k = 0; k < n - 1; k++) {
      if (delta[k] === 0) {
        ms[k] = 0;
        ms[k + 1] = 0;
        continue;
      }
      let a = ms[k] / delta[k];
      let b = ms[k + 1] / delta[k];
      // 端の接線の符号が区間の傾きと逆なら 0 にする（単調性のため）。
      if (a < 0) ms[k] = 0;
      if (b < 0) ms[k + 1] = 0;
      a = Math.max(a, 0);
      b = Math.max(b, 0);
      const s = a * a + b * b;
      if (s > 9) {
        const tau = 3 / Math.sqrt(s);
        ms[k] = tau * a * delta[k];
        ms[k + 1] = tau * b * delta[k];
      }
    }
    this.xs = xs;
    this.ys = ys;
    this.ms = ms;
  }

  /** x での値（範囲の外は端の点の y）。 */
  eval(x: number): number {
    const { xs, ys, ms } = this;
    const n = xs.length;
    if (Number.isNaN(x) || x <= xs[0]) return ys[0];
    if (x >= xs[n - 1]) return ys[n - 1];
    let k = 0;
    while (k < n - 2 && x >= xs[k + 1]) k++;
    const h = xs[k + 1] - xs[k];
    const t = (x - xs[k]) / h;
    const t2 = t * t;
    const t3 = t2 * t;
    const h00 = 2 * t3 - 3 * t2 + 1;
    const h10 = t3 - 2 * t2 + t;
    const h01 = -2 * t3 + 3 * t2;
    const h11 = t3 - t2;
    return h00 * ys[k] + h10 * h * ms[k] + h01 * ys[k + 1] + h11 * h * ms[k + 1];
  }
}

/** 点のカーブが恒等か（genzo-model の ToneCurve::points_are_identity）。 */
export function pointsAreIdentity(points: readonly CurvePoint[]): boolean {
  if (points.length === 0) return true;
  const first = points[0];
  const last = points[points.length - 1];
  return first.x === 0 && last.x === 1 && points.every((p) => p.x === p.y);
}

/** パラメトリックのカーブの 6 点（すべて 0 なら null。genzo-pipeline の parametric_spline）。 */
export function parametricPoints(p: ParametricCurve): [number, number][] | null {
  if (p.highlights === 0 && p.lights === 0 && p.darks === 0 && p.shadows === 0) return null;
  const amounts = [p.shadows, p.darks, p.lights, p.highlights];
  const pts: [number, number][] = [[0, 0]];
  PARAMETRIC_CENTERS.forEach((c, i) => pts.push([c, c + (PARAMETRIC_AMPLITUDE * amounts[i]) / 100]));
  pts.push([1, 1]);
  return pts;
}

/** 曲線の関数（恒等なら x をそのまま返す）。 */
export type CurveFn = (x: number) => number;

const identity: CurveFn = (x) => x;

/** 点のカーブの関数（恒等・不正な点なら恒等）。 */
export function pointCurveFn(points: readonly CurvePoint[]): CurveFn {
  if (pointsAreIdentity(points)) return identity;
  const s = MonotoneCubic.create(points.map((p) => [p.x, p.y] as const));
  return s ? (x) => s.eval(x) : identity;
}

/** パラメトリックのカーブの関数。 */
export function parametricFn(p: ParametricCurve): CurveFn {
  const pts = parametricPoints(p);
  const s = pts ? MonotoneCubic.create(pts) : null;
  return s ? (x) => s.eval(x) : identity;
}

/** 輝度のカーブの全体（点のカーブ ∘ パラメトリック）。 */
export function lumaCurveFn(curve: ToneCurve): CurveFn {
  const p = parametricFn(curve.parametric);
  const q = pointCurveFn(curve.luma);
  return (x) => q(p(x));
}

/** 曲線を `n` 区間で標本化する（描画用。x は 0〜1 の等間隔）。 */
export function sampleCurve(fn: CurveFn, n = 128): [number, number][] {
  const out: [number, number][] = [];
  for (let i = 0; i <= n; i++) {
    const x = i / n;
    out.push([x, fn(x)]);
  }
  return out;
}

// ---------------------------------------------------------------------------
// 点の編集
// ---------------------------------------------------------------------------

/** 編集の土台（2 点未満のカーブは、恒等の 2 点 (0, 0)・(1, 1) から始める）。 */
export function editablePoints(points: readonly CurvePoint[]): CurvePoint[] {
  if (points.length < 2) {
    return [
      { x: 0, y: 0 },
      { x: 1, y: 1 },
    ];
  }
  return points.map((p) => ({ x: p.x, y: p.y }));
}

/** 端の点（x を固定する点）か。 */
export function isEndpoint(points: readonly CurvePoint[], index: number): boolean {
  return index === 0 || index === points.length - 1;
}

/**
 * x の位置に点を足す（y は `y`。省略時は今の曲線の値）。足せない（上限・既にある点に近い）ときは null。
 *
 * 両端の点の外には足さない（両端の x は固定）。
 */
export function addPoint(
  points: readonly CurvePoint[],
  x: number,
  y?: number,
): { points: CurvePoint[]; index: number } | null {
  const base = editablePoints(points);
  if (base.length >= MAX_CURVE_POINTS) return null;
  const first = base[0].x;
  const last = base[base.length - 1].x;
  if (!(x > first + MIN_POINT_GAP && x < last - MIN_POINT_GAP)) return null;
  if (base.some((p) => Math.abs(p.x - x) < MIN_POINT_GAP)) return null;
  const value = y ?? pointCurveFn(base)(x);
  const index = base.findIndex((p) => p.x > x);
  const next = [...base.slice(0, index), { x, y: clamp01(value) }, ...base.slice(index)];
  return { points: next, index };
}

/**
 * 点を動かす。両端の点は x を変えず y だけ、中の点は隣の点の間（MIN_POINT_GAP をあける）に収める。
 * y は 0〜1 に収める。
 */
export function movePoint(points: readonly CurvePoint[], index: number, x: number, y: number): CurvePoint[] {
  const next = points.map((p) => ({ x: p.x, y: p.y }));
  if (index < 0 || index >= next.length) return next;
  let nx = next[index].x;
  if (!isEndpoint(next, index)) {
    const lo = next[index - 1].x + MIN_POINT_GAP;
    const hi = next[index + 1].x - MIN_POINT_GAP;
    nx =
      lo <= hi ? Math.min(hi, Math.max(lo, Number.isFinite(x) ? x : nx)) : (next[index - 1].x + next[index + 1].x) / 2;
  }
  next[index] = { x: nx, y: clamp01(y) };
  return next;
}

/** 点を消す（両端の点は消さない。消せなければ同じ点の並びの写しを返す）。 */
export function removePoint(points: readonly CurvePoint[], index: number): CurvePoint[] {
  const next = points.map((p) => ({ x: p.x, y: p.y }));
  if (index < 0 || index >= next.length || isEndpoint(next, index)) return next;
  next.splice(index, 1);
  return next;
}

/** (x, y) の近くの点（距離 `radius` 以内で最も近いもの。なければ -1）。 */
export function hitPoint(points: readonly CurvePoint[], x: number, y: number, radius: number): number {
  let best = -1;
  let bestD = radius * radius;
  points.forEach((p, i) => {
    const d = (p.x - x) ** 2 + (p.y - y) ** 2;
    if (d <= bestD) {
      bestD = d;
      best = i;
    }
  });
  return best;
}

/** 編集の枠（0〜1）の外へ `margin` を超えて出たか（外へのドラッグで点を消す）。 */
export function isOutside(x: number, y: number, margin: number): boolean {
  return x < -margin || x > 1 + margin || y < -margin || y > 1 + margin;
}

/** トーンカーブの 1 本の点を差し替えた設定のカーブ。 */
export function withChannel(curve: ToneCurve, channel: CurveChannel, points: CurvePoint[]): ToneCurve {
  return { ...curve, [channel]: points };
}
