import { describe, expect, it } from "vitest";

import type { CurvePoint } from "../api/types";
import {
  MAX_CURVE_POINTS,
  MIN_POINT_GAP,
  MonotoneCubic,
  addPoint,
  editablePoints,
  hitPoint,
  isOutside,
  lumaCurveFn,
  movePoint,
  parametricFn,
  pointCurveFn,
  pointsAreIdentity,
  removePoint,
} from "./curveMath";

const pts = (...a: [number, number][]): CurvePoint[] => a.map(([x, y]) => ({ x, y }));

// genzo-pipeline の MonotoneCubic・parametric_spline で計算した値（Rust の f64。作業用の小さなプログラムで
// 出力したもの）。画面に描く曲線が現像の結果と同じ形であることを確かめる。
const XS = [0.0, 0.05, 0.13, 0.25, 0.31, 0.5, 0.64, 0.77, 0.9, 0.999, 1.0];
const RUST: { points: [number, number][]; values: number[] }[] = [
  {
    points: [
      [0, 0],
      [0.25, 0.2],
      [1, 1],
    ],
    values: [
      0.0, 0.038933333333333, 0.0996736, 0.2, 0.2572288, 0.451851851851852, 0.6040192, 0.748146251851852,
      0.891792592592592, 0.998933155792593, 1.0,
    ],
  },
  {
    points: [
      [0, 0.1],
      [0.3, 0.5],
      [0.5, 0.45],
      [0.8, 0.9],
      [1, 0.95],
    ],
    values: [
      0.1, 0.175925925925926, 0.315896296296296, 0.47962962962963, 0.4996375, 0.45, 0.627405507501653,
      0.869876226054246, 0.937877552870189, 0.949795864597745, 0.95,
    ],
  },
  {
    points: [
      [0, 0],
      [0.1, 0.6],
      [0.2, 0.62],
      [1, 1],
    ],
    values: [
      0.0, 0.367544057060598, 0.612679074092223, 0.62572973496452, 0.638694824882734, 0.714445959905387,
      0.792463552235264, 0.871430353720362, 0.948014956257836, 0.999524488064293, 1.0,
    ],
  },
  {
    points: [
      [0, 1],
      [0.5, 0.2],
      [1, 0],
    ],
    values: [1.0, 0.9173, 0.7769928, 0.5625, 0.4601784, 0.2, 0.1004544, 0.0577208, 0.0304, 0.0003988024, 0.0],
  },
];
// パラメトリック（ハイライト −40・ライト 25・ダーク −10・シャドウ 60）。
const RUST_PARAM = [
  0.0, 0.07856, 0.19040418, 0.2803125, 0.31547754, 0.5071875, 0.66350546, 0.75217222, 0.86336, 0.99867769856, 1.0,
];

describe("トーンカーブの曲線（genzo-pipeline と同じ形）", () => {
  it("単調な 3 次スプラインの値が Rust の実装と一致する", () => {
    for (const c of RUST) {
      const s = MonotoneCubic.create(c.points);
      expect(s).not.toBeNull();
      XS.forEach((x, i) => expect(s!.eval(x)).toBeCloseTo(c.values[i], 12));
    }
  });
  it("パラメトリックの値が Rust の実装と一致する", () => {
    const f = parametricFn({ highlights: -40, lights: 25, darks: -10, shadows: 60 });
    XS.forEach((x, i) => expect(f(x)).toBeCloseTo(RUST_PARAM[i], 12));
  });
  it("パラメトリック: 端点は動かず、シャドウ +100 で 0.125 が 0.225 になる", () => {
    const f = parametricFn({ highlights: 0, lights: 0, darks: 0, shadows: 100 });
    expect(f(0)).toBe(0);
    expect(f(1)).toBe(1);
    expect(f(0.125)).toBeCloseTo(0.225, 12);
    // すべて 0 は恒等。
    const id = parametricFn({ highlights: 0, lights: 0, darks: 0, shadows: 0 });
    expect(id(0.3)).toBe(0.3);
  });
  it("範囲の外は端の点の y、不正な点では作らない", () => {
    const s = MonotoneCubic.create([
      [0.2, 0.1],
      [0.8, 0.7],
    ])!;
    expect(s.eval(0)).toBe(0.1);
    expect(s.eval(1)).toBe(0.7);
    expect(MonotoneCubic.create([[0, 0]])).toBeNull();
    expect(
      MonotoneCubic.create([
        [0.5, 0],
        [0.5, 1],
      ]),
    ).toBeNull();
  });
  it("恒等の判定（genzo-model の points_are_identity）", () => {
    expect(pointsAreIdentity([])).toBe(true);
    expect(pointsAreIdentity(pts([0, 0], [0.5, 0.5], [1, 1]))).toBe(true);
    expect(pointsAreIdentity(pts([0, 0], [0.5, 0.6], [1, 1]))).toBe(false);
    expect(pointsAreIdentity(pts([0.1, 0.1], [1, 1]))).toBe(false);
    expect(pointCurveFn([])(0.42)).toBe(0.42);
  });
  it("輝度のカーブは点のカーブ ∘ パラメトリック", () => {
    const curve = {
      parametric: { highlights: 0, lights: 0, darks: 0, shadows: 100 },
      luma: pts([0, 0], [0.25, 0.2], [1, 1]),
      red: [],
      green: [],
      blue: [],
    };
    const p = parametricFn(curve.parametric);
    const q = pointCurveFn(curve.luma);
    expect(lumaCurveFn(curve)(0.125)).toBeCloseTo(q(p(0.125)), 15);
    expect(lumaCurveFn(curve)(0.125)).not.toBeCloseTo(p(q(0.125)), 6);
  });
});

describe("トーンカーブの点の編集", () => {
  it("空のカーブに点を足すと両端 (0, 0)・(1, 1) を含む 3 点になり、y は今の曲線の値", () => {
    const r = addPoint([], 0.4)!;
    expect(r.index).toBe(1);
    expect(r.points).toEqual(pts([0, 0], [0.4, 0.4], [1, 1]));
    const r2 = addPoint(pts([0, 0], [1, 1]), 0.3, 0.7)!;
    expect(r2.points[1]).toEqual({ x: 0.3, y: 0.7 });
  });
  it("x の昇順の位置に入れる", () => {
    const r = addPoint(pts([0, 0], [0.2, 0.3], [0.8, 0.7], [1, 1]), 0.5, 0.5)!;
    expect(r.index).toBe(2);
    expect(r.points.map((p) => p.x)).toEqual([0, 0.2, 0.5, 0.8, 1]);
  });
  it("既にある点に近い・両端の外・上限に達したときは足さない", () => {
    const base = pts([0, 0], [0.5, 0.5], [1, 1]);
    expect(addPoint(base, 0.5 + MIN_POINT_GAP / 2)).toBeNull();
    expect(addPoint(base, 0)).toBeNull();
    expect(addPoint(base, 1)).toBeNull();
    expect(addPoint(pts([0.1, 0], [0.9, 1]), 0.05)).toBeNull();
    const full: CurvePoint[] = Array.from({ length: MAX_CURVE_POINTS }, (_, i) => ({
      x: i / (MAX_CURVE_POINTS - 1),
      y: i / (MAX_CURVE_POINTS - 1),
    }));
    expect(addPoint(full, 0.5 + 0.5 / MAX_CURVE_POINTS)).toBeNull();
  });
  it("両端の点は x を固定して y だけ動く", () => {
    const base = pts([0, 0], [0.5, 0.5], [1, 1]);
    expect(movePoint(base, 0, 0.3, 0.2)[0]).toEqual({ x: 0, y: 0.2 });
    expect(movePoint(base, 2, 0.7, 1.4)[2]).toEqual({ x: 1, y: 1 });
  });
  it("中の点は隣の点の間に収め、x は厳密に増加する", () => {
    const base = pts([0, 0], [0.3, 0.3], [0.6, 0.6], [1, 1]);
    const a = movePoint(base, 1, 0.9, -0.2);
    expect(a[1].x).toBeCloseTo(0.6 - MIN_POINT_GAP, 12);
    expect(a[1].y).toBe(0);
    const b = movePoint(base, 2, 0.1, 0.5);
    expect(b[2].x).toBeCloseTo(0.3 + MIN_POINT_GAP, 12);
    for (const r of [a, b]) for (let i = 1; i < r.length; i++) expect(r[i].x).toBeGreaterThan(r[i - 1].x);
    // 元の並びは変えない。
    expect(base[1]).toEqual({ x: 0.3, y: 0.3 });
  });
  it("両端の点は消さない", () => {
    const base = pts([0, 0], [0.5, 0.6], [1, 1]);
    expect(removePoint(base, 0)).toEqual(base);
    expect(removePoint(base, 2)).toEqual(base);
    expect(removePoint(base, 1)).toEqual(pts([0, 0], [1, 1]));
  });
  it("近くの点を探す・枠の外の判定・編集の土台", () => {
    const base = pts([0, 0], [0.5, 0.6], [1, 1]);
    expect(hitPoint(base, 0.51, 0.59, 0.03)).toBe(1);
    expect(hitPoint(base, 0.3, 0.3, 0.03)).toBe(-1);
    expect(isOutside(1.1, 0.5, 0.05)).toBe(true);
    expect(isOutside(1.03, 0.5, 0.05)).toBe(false);
    expect(editablePoints(pts([0.4, 0.4]))).toEqual(pts([0, 0], [1, 1]));
  });
});
