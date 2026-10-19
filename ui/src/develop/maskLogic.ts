// ローカル補正（マスク。DEV-18・DEV-19）の設定の組み立て・座標の変換・ブラシの点の間引き（DOM を使わない純粋な計算）。
//
// - 位置・半径・ブラシの大きさは G 座標（ジオメトリ補正後・切り抜く前の全体の外接矩形の長辺を 1 とする座標。
//   genzo-model の develop::masks）。画面の画像（CSS の画素の箱）との対応は、その画像の FrameInfo.g_bounds（縁の
//   G 座標）で求める（gToBox・boxToG）。切り抜きの編集中（切り抜く前の全体）でも、等倍表示でも同じ式。
// - 部品の合成: 重みを、最初の部品が「加える」なら 0、それ以外なら 1（画像全体）から始め、順に当てる（加える: max、
//   除く: m · (1 − c)、交差: m · c）。範囲マスク（色域・輝度）は交差で使う（DEV-19。追加するときの既定）。
// - 値の範囲・数の上限はコア（genzo-model の validate）と同じ。超える値はコアが受け付けないので、ここで丸める・断る。
// - 処理バージョン 1 の写真では使われない（コアは警告を出して無視する）。パネルは更新を勧める。

import type {
  BrushStroke,
  ColorSample,
  DevelopSettings,
  LinearShape,
  LocalAdjustment,
  LocalDelta,
  MaskComponent,
  MaskMode,
  MaskShape,
  RadialShape,
} from "../api/types";
import { MASK_LIMITS, RANGES } from "../api/types";

/** ローカル補正を使える最初の処理バージョン（genzo-pipeline の ProcessVersion::V2）。 */
export const MASK_PROCESS_VERSION = 2;

export type ShapeKind = "linear" | "radial" | "brush" | "color_range" | "luminance_range";

/** 形の名前。 */
export const SHAPE_LABELS: Record<ShapeKind, string> = {
  brush: "ブラシ",
  linear: "線形グラデーション",
  radial: "円形グラデーション",
  color_range: "色域",
  luminance_range: "輝度範囲",
};

/** 合成のしかたの選択肢。 */
export const MODE_CHOICES: { mode: MaskMode; label: string; title: string }[] = [
  { mode: "add", label: "加える", title: "この部品の範囲をマスクに加えます" },
  { mode: "subtract", label: "除く", title: "この部品の範囲をマスクから除きます" },
  { mode: "intersect", label: "交差", title: "この部品と重なる範囲だけを残します（色域・輝度の範囲の既定）" },
];

/** 形の種類。 */
export function shapeKind(shape: MaskShape): ShapeKind {
  if ("linear" in shape) return "linear";
  if ("radial" in shape) return "radial";
  if ("brush" in shape) return "brush";
  if ("color_range" in shape) return "color_range";
  return "luminance_range";
}

/** 範囲マスク（画素の色・明るさで決まる部品）か。 */
export function isRangeKind(kind: ShapeKind): boolean {
  return kind === "color_range" || kind === "luminance_range";
}

// ---------------------------------------------------------------------------
// 差分の調整値
// ---------------------------------------------------------------------------

/** 何もしない差分（genzo-model の LocalDelta::default）。 */
export function defaultDelta(): LocalDelta {
  return {
    temperature: 0,
    tint: 0,
    exposure_ev: 0,
    contrast: 0,
    highlights: 0,
    shadows: 0,
    whites: 0,
    blacks: 0,
    texture: 0,
    clarity: 0,
    dehaze: 0,
    saturation: 0,
    sharpness: 0,
  };
}

/** 項目が欠けていても差分をそろえる。 */
export function fullDelta(d: Partial<LocalDelta> | null | undefined): LocalDelta {
  return { ...defaultDelta(), ...(d ?? {}) };
}

/** 差分がすべて 0 か。 */
export function deltaIsZero(d: Partial<LocalDelta> | null | undefined): boolean {
  return Object.values(fullDelta(d)).every((v) => v === 0);
}

/** 差分の項目の範囲（露光量は EV、ほかは −100〜100。genzo-model の validate）。 */
export function deltaRange(key: keyof LocalDelta): { min: number; max: number } {
  return key === "exposure_ev" ? RANGES.exposureEv : RANGES.slider;
}

// ---------------------------------------------------------------------------
// マスクの名前・状態
// ---------------------------------------------------------------------------

/** 表示の名前（名前がなければ「マスク 1」など。genzo-api の mask_display_name と同じ）。 */
export function maskDisplayName(m: Pick<LocalAdjustment, "name">, index: number): string {
  return m.name.trim() === "" ? `マスク ${index + 1}` : m.name;
}

/** 名前を整える（制御文字を除き、上限の文字数まで。コアの validate は超えたものを受け付けない）。 */
export function sanitizeMaskName(name: string): string {
  // eslint-disable-next-line no-control-regex
  const cleaned = name.replace(/[\u0000-\u001f\u007f-\u009f]/g, "");
  return [...cleaned].slice(0, MASK_LIMITS.nameChars).join("");
}

/** 部品の種類の要約（「線形グラデーション ＋ 輝度範囲」など。一覧に出す）。 */
export function maskSummary(m: LocalAdjustment): string {
  if (m.components.length === 0) return "部品なし";
  return m.components.map((c) => SHAPE_LABELS[shapeKind(c.shape)]).join(" ＋ ");
}

/** マスクの欄の操作の可否。 */
export interface MaskControls {
  /** 処理バージョンが古くて使われない。 */
  needsUpgrade: boolean;
  /** 編集できる。 */
  editable: boolean;
  /** マスクを足せる（数の上限まで）。 */
  canAdd: boolean;
}

export function maskControls(s: DevelopSettings, disabled: boolean): MaskControls {
  const needsUpgrade = s.process_version < MASK_PROCESS_VERSION;
  const editable = !disabled && !needsUpgrade;
  return { needsUpgrade, editable, canAdd: editable && (s.masks?.length ?? 0) < MASK_LIMITS.masks };
}

// ---------------------------------------------------------------------------
// 設定の組み立て
// ---------------------------------------------------------------------------

/** G 座標の範囲（左・上・右・下）。 */
export type GBounds = [number, number, number, number];

/** 画像の縁の G 座標（古いコアで無ければ、外接矩形の全体を幅 1 とみなす）。 */
export function boundsOrDefault(gb: GBounds | null | undefined): GBounds {
  if (gb && gb.every(Number.isFinite) && gb[2] > gb[0] && gb[3] > gb[1]) return gb;
  return [0, 0, 1, 1];
}

const round4 = (v: number) => Math.round(v * 10000) / 10000;
const clampTo = (v: number, r: { min: number; max: number }) => Math.min(r.max, Math.max(r.min, v));
const coord = (v: number) => round4(clampTo(v, RANGES.maskCoord));

/** 新しい線形グラデーション（見えている範囲の上から 20% で 100%、50% で 0%。空のように上から下へ）。 */
export function newLinearShape(gb: GBounds): MaskShape {
  const [l, t, r, b] = gb;
  const cx = (l + r) / 2;
  const h = b - t;
  return { linear: { x0: coord(cx), y0: coord(t + 0.2 * h), x1: coord(cx), y1: coord(t + 0.5 * h) } };
}

/** 新しい円形グラデーション（見えている範囲の中央。半径は幅・高さの 25%）。 */
export function newRadialShape(gb: GBounds): MaskShape {
  const [l, t, r, b] = gb;
  return {
    radial: {
      cx: coord((l + r) / 2),
      cy: coord((t + b) / 2),
      rx: round4(clampTo(0.25 * (r - l), RANGES.maskRadius)),
      ry: round4(clampTo(0.25 * (b - t), RANGES.maskRadius)),
      angle_deg: 0,
      feather: 50,
    },
  };
}

/** 新しい輝度範囲（genzo-model の既定。下端 0・上端 100・滑らかさ 50 で全体を選ぶ）。 */
export function newLuminanceShape(): MaskShape {
  return { luminance_range: { min: 0, max: 100, smoothness: 50 } };
}

/** 見本の色を丸める（コアの値の範囲）。 */
export function sanitizeSample(s: ColorSample): ColorSample {
  const ab = { min: -4, max: 4 };
  return {
    x: coord(s.x),
    y: coord(s.y),
    lab: [clampTo(s.lab[0], { min: 0, max: 10 }), clampTo(s.lab[1], ab), clampTo(s.lab[2], ab)],
  };
}

/** 新しい色域の範囲（見本の色が 1 つ要る。コアは見本のない色域を受け付けない）。 */
export function newColorShape(sample: ColorSample): MaskShape {
  return { color_range: { samples: [sanitizeSample(sample)], amount: 50 } };
}

/** 新しい部品（範囲マスクは交差、形は加える）。 */
export function newComponent(shape: MaskShape): MaskComponent {
  return { mode: isRangeKind(shapeKind(shape)) ? "intersect" : "add", invert: false, shape };
}

/** 新しいマスク（部品 1 つ、差分 0）。 */
export function newMask(shape: MaskShape): LocalAdjustment {
  return { enabled: true, name: "", components: [newComponent(shape)], delta: defaultDelta() };
}

/** マスク `i` を変えた設定（範囲の外なら変えない）。 */
export function withMask(
  s: DevelopSettings,
  i: number,
  fn: (m: LocalAdjustment) => LocalAdjustment,
): DevelopSettings {
  const masks = s.masks ?? [];
  if (i < 0 || i >= masks.length) return s;
  const next = masks.slice();
  next[i] = fn(masks[i]);
  return { ...s, masks: next };
}

/** マスクを足した設定（上限なら変えない）。 */
export function addMask(s: DevelopSettings, m: LocalAdjustment): DevelopSettings {
  const masks = s.masks ?? [];
  if (masks.length >= MASK_LIMITS.masks) return s;
  return { ...s, masks: [...masks, m] };
}

/** マスク `i` を除いた設定。 */
export function removeMask(s: DevelopSettings, i: number): DevelopSettings {
  const masks = s.masks ?? [];
  if (i < 0 || i >= masks.length) return s;
  return { ...s, masks: masks.filter((_, k) => k !== i) };
}

/** マスク `i` の写しを後ろに足した設定（名前に「のコピー」を付ける。ブラシの点の上限を超えるなら変えない）。 */
export function duplicateMask(s: DevelopSettings, i: number): DevelopSettings {
  const masks = s.masks ?? [];
  if (i < 0 || i >= masks.length || masks.length >= MASK_LIMITS.masks) return s;
  const m = masks[i];
  if (totalBrushPoints(s) + maskBrushPoints(m) > MASK_LIMITS.totalBrushPoints) return s;
  const copy: LocalAdjustment = structuredClone(m);
  copy.name = sanitizeMaskName(`${maskDisplayName(m, i)}のコピー`);
  return { ...s, masks: [...masks, copy] };
}

/** マスク `i` の差分の一部を変えた設定（範囲に丸める）。 */
export function withDelta(s: DevelopSettings, i: number, patch: Partial<LocalDelta>): DevelopSettings {
  return withMask(s, i, (m) => {
    const d = { ...fullDelta(m.delta) };
    for (const [k, v] of Object.entries(patch) as [keyof LocalDelta, number][]) {
      d[k] = clampTo(v, deltaRange(k));
    }
    return { ...m, delta: d };
  });
}

/** マスク `i` の部品 `j` を変えた設定。 */
export function withComponent(
  s: DevelopSettings,
  i: number,
  j: number,
  fn: (c: MaskComponent) => MaskComponent,
): DevelopSettings {
  return withMask(s, i, (m) => {
    if (j < 0 || j >= m.components.length) return m;
    const components = m.components.slice();
    components[j] = fn(m.components[j]);
    return { ...m, components };
  });
}

/** マスク `i` に部品を足した設定（上限なら変えない）。 */
export function addComponent(s: DevelopSettings, i: number, c: MaskComponent): DevelopSettings {
  return withMask(s, i, (m) =>
    m.components.length >= MASK_LIMITS.components ? m : { ...m, components: [...m.components, c] },
  );
}

/** マスク `i` の部品 `j` を除いた設定。 */
export function removeComponent(s: DevelopSettings, i: number, j: number): DevelopSettings {
  return withMask(s, i, (m) => ({ ...m, components: m.components.filter((_, k) => k !== j) }));
}

/** 部品の形を変えた設定（同じ種類の形のときだけ。違えば変えない）。 */
export function withShape<K extends ShapeKind>(
  s: DevelopSettings,
  i: number,
  j: number,
  kind: K,
  fn: (shape: Extract<MaskShape, Record<K, unknown>>[K]) => Extract<MaskShape, Record<K, unknown>>[K],
): DevelopSettings {
  return withComponent(s, i, j, (c) => {
    if (shapeKind(c.shape) !== kind) return c;
    const inner = (c.shape as Record<string, unknown>)[kind] as Extract<MaskShape, Record<K, unknown>>[K];
    return { ...c, shape: { [kind]: fn(inner) } as unknown as MaskShape };
  });
}

/** 色域の部品に見本の色を足した設定（上限なら最後の見本を置き換える）。 */
export function addColorSample(s: DevelopSettings, i: number, j: number, sample: ColorSample): DevelopSettings {
  return withShape(s, i, j, "color_range", (r) => {
    const samples = r.samples.slice(0, MASK_LIMITS.colorSamples);
    if (samples.length >= MASK_LIMITS.colorSamples) samples.pop();
    return { ...r, samples: [...samples, sanitizeSample(sample)] };
  });
}

/** 色域の部品の見本の色 `k` を除いた設定（最後の 1 つは除かない。見本のない色域はコアが受け付けない）。 */
export function removeColorSample(s: DevelopSettings, i: number, j: number, k: number): DevelopSettings {
  return withShape(s, i, j, "color_range", (r) =>
    r.samples.length <= 1 ? r : { ...r, samples: r.samples.filter((_, n) => n !== k) },
  );
}

/** 輝度範囲を変える（下端が上端を越えたら、もう一方を押して下端 ≤ 上端を保つ）。 */
export function withLuminance(
  s: DevelopSettings,
  i: number,
  j: number,
  patch: Partial<{ min: number; max: number; smoothness: number }>,
): DevelopSettings {
  return withShape(s, i, j, "luminance_range", (r) => {
    const a = RANGES.amount;
    let min = clampTo(patch.min ?? r.min, a);
    let max = clampTo(patch.max ?? r.max, a);
    if (min > max) {
      if (patch.min !== undefined) max = min;
      else min = max;
    }
    return { min, max, smoothness: clampTo(patch.smoothness ?? r.smoothness, a) };
  });
}

// ---------------------------------------------------------------------------
// 座標（G 座標 ↔ 画面の画像の箱）
// ---------------------------------------------------------------------------

/** 画面の画像の箱（CSS の画素。画像の左上・幅・高さ）。 */
export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** G 座標の点を、箱の中の位置（CSS の画素）にする。 */
export function gToBox(gb: GBounds, box: Box, u: number, v: number): [number, number] {
  return [
    box.left + ((u - gb[0]) / (gb[2] - gb[0])) * box.width,
    box.top + ((v - gb[1]) / (gb[3] - gb[1])) * box.height,
  ];
}

/** 箱の中の位置（CSS の画素）を G 座標にする。 */
export function boxToG(gb: GBounds, box: Box, x: number, y: number): [number, number] {
  return [
    gb[0] + ((x - box.left) / box.width) * (gb[2] - gb[0]),
    gb[1] + ((y - box.top) / box.height) * (gb[3] - gb[1]),
  ];
}

/** G 座標の長さ 1 が、箱の中の何画素か（横方向。G は縦横の尺度が同じ）。 */
export function pixelsPerG(gb: GBounds, box: Box): number {
  return box.width / (gb[2] - gb[0]);
}

// ---------------------------------------------------------------------------
// 線形・円形のつまみ
// ---------------------------------------------------------------------------

export type LinearHandle = "start" | "end" | "move";
export type RadialHandle = "move" | "rx" | "ry" | "rotate";

/** 線形のつまみを G 座標で (du, dv) だけ動かした形（ドラッグを始めたときの形 `start` から）。 */
export function dragLinear(start: LinearShape, handle: LinearHandle, du: number, dv: number): LinearShape {
  const moveStart = handle === "start" || handle === "move";
  const moveEnd = handle === "end" || handle === "move";
  return {
    x0: moveStart ? coord(start.x0 + du) : start.x0,
    y0: moveStart ? coord(start.y0 + dv) : start.y0,
    x1: moveEnd ? coord(start.x1 + du) : start.x1,
    y1: moveEnd ? coord(start.y1 + dv) : start.y1,
  };
}

/** 円形の軸の向き（ex: 横の半径の向き、ey: 縦の半径の向き。G 座標は下向きが正なので、角度は時計回り）。 */
export function radialAxes(angleDeg: number): { ex: [number, number]; ey: [number, number] } {
  const t = (angleDeg * Math.PI) / 180;
  const c = Math.cos(t);
  const s = Math.sin(t);
  return { ex: [c, s], ey: [-s, c] };
}

/** 角度を −180〜180 にする。 */
export function wrapAngle(deg: number): number {
  let a = ((((deg + 180) % 360) + 360) % 360) - 180;
  if (a === -180 && deg > 0) a = 180;
  return Math.round(a * 100) / 100;
}

/**
 * 円形のつまみを動かした形。`start` はドラッグを始めたときの形、`p` は今のポインターの G 座標、`p0` は始めたときの
 * ポインターの G 座標。move は中心を動かし、rx・ry は軸に沿った距離を半径にし、rotate は中心から見た向きを縦の軸の
 * 上（−ey）にする。
 */
export function dragRadial(
  start: RadialShape,
  handle: RadialHandle,
  p: [number, number],
  p0: [number, number],
): RadialShape {
  if (handle === "move") {
    return { ...start, cx: coord(start.cx + p[0] - p0[0]), cy: coord(start.cy + p[1] - p0[1]) };
  }
  const w: [number, number] = [p[0] - start.cx, p[1] - start.cy];
  if (handle === "rotate") {
    if (Math.hypot(w[0], w[1]) < 1e-9) return start;
    return { ...start, angle_deg: wrapAngle((Math.atan2(w[0], -w[1]) * 180) / Math.PI) };
  }
  const { ex, ey } = radialAxes(start.angle_deg);
  const axis = handle === "rx" ? ex : ey;
  const r = round4(clampTo(Math.abs(w[0] * axis[0] + w[1] * axis[1]), { min: 0.001, max: RANGES.maskRadius.max }));
  return handle === "rx" ? { ...start, rx: r } : { ...start, ry: r };
}

// ---------------------------------------------------------------------------
// ブラシ
// ---------------------------------------------------------------------------

/** ブラシの道具の設定（ストロークを描くときに使う。genzo-model の BrushStroke の既定）。 */
export interface BrushOptions {
  /** 大きさ（半径。G 座標）。 */
  size: number;
  feather: number;
  flow: number;
  density: number;
  erase: boolean;
}

export const DEFAULT_BRUSH: BrushOptions = { size: 0.05, feather: 50, flow: 100, density: 100, erase: false };

/** マスクのブラシの点の数。 */
export function maskBrushPoints(m: LocalAdjustment): number {
  let n = 0;
  for (const c of m.components) {
    if ("brush" in c.shape) for (const st of c.shape.brush.strokes) n += st.points.length;
  }
  return n;
}

/** 設定のすべてのブラシの点の数（コアの上限 MASK_LIMITS.totalBrushPoints）。 */
export function totalBrushPoints(s: DevelopSettings): number {
  return (s.masks ?? []).reduce((n, m) => n + maskBrushPoints(m), 0);
}

/**
 * ストロークの点を間引く（前に残した点から `minDist` 未満の点を除く。最後の点は残す）。さらに `max` 個を超えたら、
 * 最初と最後を含めて等間隔に選ぶ。
 */
export function simplifyPoints(points: [number, number][], minDist: number, max: number): [number, number][] {
  const finite = points.filter(([x, y]) => Number.isFinite(x) && Number.isFinite(y));
  if (finite.length <= 1) return finite.map(([x, y]) => [coord(x), coord(y)]);
  const kept: [number, number][] = [finite[0]];
  for (let i = 1; i < finite.length; i++) {
    const [x, y] = finite[i];
    const [px, py] = kept[kept.length - 1];
    if (Math.hypot(x - px, y - py) >= minDist) kept.push(finite[i]);
  }
  const last = finite[finite.length - 1];
  const tail = kept[kept.length - 1];
  if (tail !== last && (tail[0] !== last[0] || tail[1] !== last[1])) kept.push(last);
  let out = kept;
  if (max >= 2 && out.length > max) {
    const step = (out.length - 1) / (max - 1);
    out = Array.from({ length: max }, (_, k) => kept[Math.round(k * step)]);
  }
  return out.map(([x, y]) => [coord(x), coord(y)]);
}

/** ストロークにする（大きさ・ぼかしなどは道具の設定。点は間引く。間引きの距離は大きさの 1/8）。 */
export function makeStroke(points: [number, number][], o: BrushOptions): BrushStroke {
  const size = round4(clampTo(o.size, RANGES.brushSize));
  return {
    erase: o.erase,
    size,
    feather: clampTo(Math.round(o.feather), RANGES.amount),
    flow: clampTo(Math.round(o.flow), RANGES.amount),
    density: clampTo(Math.round(o.density), RANGES.amount),
    points: simplifyPoints(points, size / 8, MASK_LIMITS.strokePoints),
  };
}

/**
 * ブラシの部品にストロークを足す。足せなければ理由を返す（点がない・本数や点の数がコアの上限を超える・部品が
 * ブラシでない）。
 */
export function appendStroke(
  s: DevelopSettings,
  i: number,
  j: number,
  stroke: BrushStroke,
): { settings: DevelopSettings } | { error: string } {
  const m = s.masks?.[i];
  const c = m?.components[j];
  if (!m || !c || !("brush" in c.shape)) return { error: "ブラシの部品を選んでください" };
  if (stroke.points.length === 0) return { error: "ストロークに点がありません" };
  if (c.shape.brush.strokes.length >= MASK_LIMITS.strokes) {
    return { error: `ストロークが多すぎます（1 つのブラシで ${MASK_LIMITS.strokes.toLocaleString("ja-JP")} 本まで）` };
  }
  if (totalBrushPoints(s) + stroke.points.length > MASK_LIMITS.totalBrushPoints) {
    return {
      error: `ブラシの点が多すぎます（全体で ${MASK_LIMITS.totalBrushPoints.toLocaleString("ja-JP")} 点まで）。不要なストロークを消してください`,
    };
  }
  return {
    settings: withShape(s, i, j, "brush", (b) => ({ strokes: [...b.strokes, stroke] })),
  };
}

/** ブラシの最後のストロークを除いた設定。 */
export function undoStroke(s: DevelopSettings, i: number, j: number): DevelopSettings {
  return withShape(s, i, j, "brush", (b) => ({ strokes: b.strokes.slice(0, -1) }));
}

/** ブラシのストロークをすべて除いた設定。 */
export function clearStrokes(s: DevelopSettings, i: number, j: number): DevelopSettings {
  return withShape(s, i, j, "brush", () => ({ strokes: [] }));
}

// ---------------------------------------------------------------------------
// 表示
// ---------------------------------------------------------------------------

/** 離したストローク（コアに渡した設定の通し番号 `seq`。受け付けた世代 `generation` が分かるまでは null）。 */
export interface PendingStroke {
  points: [number, number][];
  erase: boolean;
  /** 大きさ（半径。G 座標）。 */
  size: number;
  seq: number;
  generation: number | null;
}

/**
 * 離したストロークのうち、まだ描いておくもの。送った設定の結果 `acks`（送った順）のうち、ストロークの番号以降で受け付けた
 * 最初のもので世代を埋め、その世代以降の重ねて表示 `overlayGeneration` が届いたら除く（重ねて表示をしていなければ
 * （`overlayOn` が false）、受け付けたら除く）。受け付けた結果がなく、番号以降の失敗のうちに、後の設定が渡されて
 * いなかった（`superseded` でない）ものがあれば、すぐに除く（後の設定が渡されていれば、その結果を待つ）。
 * 変わらなければ同じ配列を返す。
 */
export function settlePendingStrokes(
  pending: PendingStroke[],
  acks: readonly { seq: number; generation: number | null; superseded?: boolean }[],
  overlayOn: boolean,
  overlayGeneration: number | null,
): PendingStroke[] {
  let changed = false;
  const next: PendingStroke[] = [];
  for (const p of pending) {
    let generation = p.generation;
    if (generation === null) {
      const covering = acks.filter((a) => a.seq >= p.seq);
      generation = covering.find((a) => a.generation !== null)?.generation ?? null;
      if (generation === null && covering.some((a) => a.generation === null && !a.superseded)) {
        changed = true;
        continue;
      }
    }
    const done =
      generation !== null && (!overlayOn || (overlayGeneration !== null && overlayGeneration >= generation));
    if (done) {
      changed = true;
      continue;
    }
    if (generation !== p.generation) changed = true;
    next.push(generation === p.generation ? p : { ...p, generation });
  }
  return changed ? next : pending;
}

/** 重ねて表示の色（赤）と、重み 1 のときの不透明度。 */
export const OVERLAY_RGB: [number, number, number] = [255, 40, 40];
export const OVERLAY_ALPHA = 0.5;

/**
 * 重ねて表示の画素（コアの mask_overlay_frame の画素は R・G・B に重み 0〜255）を、赤の半透明の RGBA にする（重み 0 は
 * 透明）。
 */
export function overlayRgba(pixels: Uint8ClampedArray): Uint8ClampedArray<ArrayBuffer> {
  const out = new Uint8ClampedArray(pixels.length);
  for (let i = 0; i + 3 < pixels.length; i += 4) {
    out[i] = OVERLAY_RGB[0];
    out[i + 1] = OVERLAY_RGB[1];
    out[i + 2] = OVERLAY_RGB[2];
    out[i + 3] = Math.round(pixels[i] * OVERLAY_ALPHA);
  }
  return out;
}

/**
 * 見本の色（OKLab）の表示の色（CSS）。OKLab → 線形 sRGB にして、最も大きいチャンネルが 1 を超えたら 1 に縮め、sRGB の
 * 伝達関数で符号化する（見本の印に出すだけの近似）。
 */
export function labToCss(lab: [number, number, number]): string {
  const [L, a, b] = lab;
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  let rgb = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ].map((v) => (Number.isFinite(v) ? Math.max(0, v) : 0));
  const k = Math.max(...rgb);
  if (k > 1) rgb = rgb.map((v) => v / k);
  const enc = (v: number) => (v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055);
  const hex = rgb.map((v) =>
    Math.round(Math.min(1, Math.max(0, enc(v))) * 255)
      .toString(16)
      .padStart(2, "0"),
  );
  return `#${hex.join("")}`;
}
