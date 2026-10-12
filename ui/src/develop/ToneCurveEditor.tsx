// ポイントカーブの編集（DEV-07）。
//
// - クリックで点を足し（今の曲線の上に置く）、そのままドラッグで動かす。点のドラッグも同じ。
// - 消す: ダブルクリック、枠の外へのドラッグ、選んだ点で Delete / Backspace。両端の点は消せない。
// - 両端の点は x を固定して y だけ動く。x は厳密に増加する（curveMath の約束）。点は最大 64。
// - ドラッグ中は onChange(points, true)、離したら onCommit()（コアの end_drag。履歴に 1 件）。
//   ダブルクリック・キーでの変更は onChange(points, false)。
// - 描く曲線は genzo-pipeline と同じ補間（curveMath）。輝度のタブでは、パラメトリックと合わせた全体の
//   曲線も点線で重ねる。背景に、そのチャンネルのヒストグラムを薄く描く。

import { type KeyboardEvent, type MouseEvent, type PointerEvent, useEffect, useRef, useState } from "react";

import type { CurvePoint, ParametricCurve } from "../api/types";
import {
  type CurveChannel,
  addPoint,
  editablePoints,
  hitPoint,
  isEndpoint,
  isOutside,
  movePoint,
  parametricFn,
  pointCurveFn,
  removePoint,
  sampleCurve,
} from "./curveMath";
import { histogramHeights, histogramScale } from "./pixels";

/** 枠のまわりの余白（曲線の座標の単位）。端の点をつかめるようにする。 */
const PAD = 0.04;
/** 点をつかめる距離（曲線の座標の単位。仮置き）。 */
const HIT = 0.035;
/** 枠の外へこれ以上ドラッグしたら点を消す（曲線の座標の単位。仮置き）。 */
const REMOVE_MARGIN = 0.08;
/** 矢印キーで点を動かす量（Shift で大きく。仮置き）。 */
const NUDGE = 0.004;
const NUDGE_SHIFT = 0.02;

const toPath = (pts: [number, number][]) =>
  pts.map(([x, y], i) => `${i === 0 ? "M" : "L"}${x.toFixed(4)} ${(1 - y).toFixed(4)}`).join(" ");

interface Drag {
  pointer: number;
  index: number;
  /** ドラッグを始めたときの点の並び。 */
  points: CurvePoint[];
  startX: number;
  startY: number;
  pointerX: number;
  pointerY: number;
  /** 何か送ったか（離したら end_drag を送る）。 */
  changed: boolean;
  /** 枠の外へ出して消したか。 */
  removed: boolean;
}

export function ToneCurveEditor(props: {
  channel: CurveChannel;
  points: CurvePoint[];
  parametric: ParametricCurve;
  /** 背景に描くヒストグラムの度数（なければ描かない）。 */
  histogram: number[] | null;
  disabled?: boolean;
  onChange: (points: CurvePoint[], dragging: boolean) => void;
  onCommit: () => void;
}) {
  const { channel, points, disabled } = props;
  const svgRef = useRef<SVGSVGElement>(null);
  const drag = useRef<Drag | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const shown = editablePoints(points);

  // 自分の操作で点を変えたか（その直後の点の変化では選択を外さない）。
  const ownEdit = useRef(false);
  const emit = (next: CurvePoint[], dragging: boolean) => {
    ownEdit.current = true;
    props.onChange(next, dragging);
  };
  // チャンネルを変えた・点が外から変わった（Undo・貼り付けなど）ときは選択を外す。
  useEffect(() => setSelected(null), [channel]);
  useEffect(() => {
    if (ownEdit.current) {
      ownEdit.current = false;
      return;
    }
    setSelected(null);
  }, [points]);

  const toCurve = (clientX: number, clientY: number): { x: number; y: number } => {
    const svg = svgRef.current;
    if (!svg) return { x: 0, y: 0 };
    const r = svg.getBoundingClientRect();
    const span = 1 + 2 * PAD;
    const x = ((clientX - r.left) / Math.max(1, r.width)) * span - PAD;
    const v = ((clientY - r.top) / Math.max(1, r.height)) * span - PAD;
    return { x, y: 1 - v };
  };

  const onPointerDown = (e: PointerEvent<SVGSVGElement>) => {
    if (disabled || e.button !== 0) return;
    const p = toCurve(e.clientX, e.clientY);
    let base = editablePoints(points);
    let index = hitPoint(base, p.x, p.y, HIT);
    let changed = false;
    if (index < 0) {
      if (isOutside(p.x, p.y, 0)) return;
      const added = addPoint(points, p.x);
      if (!added) return;
      base = added.points;
      index = added.index;
      changed = true;
      emit(base, true);
    }
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    e.currentTarget.focus();
    setSelected(index);
    drag.current = {
      pointer: e.pointerId,
      index,
      points: base,
      startX: base[index].x,
      startY: base[index].y,
      pointerX: p.x,
      pointerY: p.y,
      changed,
      removed: false,
    };
  };

  const onPointerMove = (e: PointerEvent<SVGSVGElement>) => {
    const d = drag.current;
    if (!d || d.pointer !== e.pointerId || d.removed) return;
    const p = toCurve(e.clientX, e.clientY);
    if (!isEndpoint(d.points, d.index) && isOutside(p.x, p.y, REMOVE_MARGIN)) {
      // 枠の外へのドラッグで消す。
      d.removed = true;
      d.changed = true;
      setSelected(null);
      emit(removePoint(d.points, d.index), true);
      return;
    }
    const next = movePoint(d.points, d.index, d.startX + (p.x - d.pointerX), d.startY + (p.y - d.pointerY));
    d.changed = true;
    emit(next, true);
  };

  const onPointerUp = (e: PointerEvent<SVGSVGElement>) => {
    const d = drag.current;
    if (!d || d.pointer !== e.pointerId) return;
    drag.current = null;
    if (d.changed) props.onCommit();
  };

  const onDoubleClick = (e: MouseEvent<SVGSVGElement>) => {
    if (disabled) return;
    const p = toCurve(e.clientX, e.clientY);
    const base = editablePoints(points);
    const i = hitPoint(base, p.x, p.y, HIT);
    if (i < 0 || isEndpoint(base, i)) return;
    setSelected(null);
    emit(removePoint(base, i), false);
  };

  const onKeyDown = (e: KeyboardEvent<SVGSVGElement>) => {
    if (disabled || selected === null || selected >= shown.length) return;
    const step = e.shiftKey ? NUDGE_SHIFT : NUDGE;
    let next: CurvePoint[] | null = null;
    switch (e.key) {
      case "Delete":
      case "Backspace":
        if (isEndpoint(shown, selected)) break;
        next = removePoint(shown, selected);
        setSelected(null);
        break;
      case "ArrowUp":
        next = movePoint(shown, selected, shown[selected].x, shown[selected].y + step);
        break;
      case "ArrowDown":
        next = movePoint(shown, selected, shown[selected].x, shown[selected].y - step);
        break;
      case "ArrowLeft":
        next = movePoint(shown, selected, shown[selected].x - step, shown[selected].y);
        break;
      case "ArrowRight":
        next = movePoint(shown, selected, shown[selected].x + step, shown[selected].y);
        break;
      default:
        return;
    }
    // 全体のショートカット（Delete で削除のダイアログ、←/→ で前後の写真）より先に処理する。
    e.preventDefault();
    e.stopPropagation();
    if (next) emit(next, false);
  };

  const pointFn = pointCurveFn(points);
  const curve = sampleCurve(pointFn, 128);
  const pc = props.parametric;
  const hasParametric = pc.highlights !== 0 || pc.lights !== 0 || pc.darks !== 0 || pc.shadows !== 0;
  let total: [number, number][] | null = null;
  if (channel === "luma" && hasParametric) {
    // 輝度のカーブの全体（点のカーブ ∘ パラメトリック）。
    const paramFn = parametricFn(pc);
    total = sampleCurve((x) => pointFn(paramFn(x)), 128);
  }

  let histoPath: string | null = null;
  if (props.histogram && props.histogram.length > 1) {
    const h = histogramHeights(props.histogram, histogramScale([props.histogram]));
    const n = h.length - 1;
    histoPath = `M0 1 ${h.map((v, i) => `L${(i / n).toFixed(4)} ${(1 - v * 0.9).toFixed(4)}`).join(" ")} L1 1 Z`;
  }

  const vb = `${-PAD} ${-PAD} ${1 + 2 * PAD} ${1 + 2 * PAD}`;
  return (
    <svg
      ref={svgRef}
      className={`dp-curve${disabled ? " disabled" : ""}`}
      viewBox={vb}
      tabIndex={disabled ? -1 : 0}
      role="application"
      aria-label="ポイントカーブ（クリックで点を追加、ドラッグで移動、ダブルクリック・外へのドラッグ・Delete で削除）"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={onDoubleClick}
      onKeyDown={onKeyDown}
    >
      {histoPath && <path className="histo" d={histoPath} />}
      {[0.25, 0.5, 0.75].map((t) => (
        <g key={t}>
          <line className="grid" x1={t} y1={0} x2={t} y2={1} />
          <line className="grid" x1={0} y1={t} x2={1} y2={t} />
        </g>
      ))}
      <rect className="grid" x={0} y={0} width={1} height={1} fill="none" />
      <line className="diag" x1={0} y1={1} x2={1} y2={0} />
      {total && <path className="total" d={toPath(total)} />}
      <path className={`line ${channel}`} d={toPath(curve)} />
      {shown.map((p, i) => (
        <circle key={i} className={`pt${selected === i ? " sel" : ""}`} cx={p.x} cy={1 - p.y} r={0.018} />
      ))}
    </svg>
  );
}
