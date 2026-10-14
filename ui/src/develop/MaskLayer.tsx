// 現像のプレビューの上のローカル補正（マスク。DEV-18・DEV-19）の表示と操作。
//
// - MaskLayer: 選んでいるマスクの線形・円形のつまみ（ドラッグで動かす。ドラッグの間は dragging で送り、離したら
//   end_drag で履歴に 1 件）、色域の見本の点の印、ブラシで描く（tool が brush の間。押してから離すまでを 1 本の
//   ストロークにして、離したときに設定に足す。Alt を押しながらなら消しゴム）。位置は G 座標で、画面の画像の箱との
//   対応は FrameInfo.g_bounds（maskLogic の gToBox・boxToG）。
// - MaskOverlayCanvas / useMaskOverlay: 重ねて表示（O）。コアの mask_overlay_frame（プレビューと同じ格子の重み）を
//   赤の半透明にして、画像の箱に重ねる。プレビューが更新されるたびに取り直す（取得中に更新が来たら、終わってから
//   もう一度。古い結果は捨てる）。

import { type PointerEvent as ReactPointerEvent, useEffect, useRef, useState } from "react";

import { type Frame, fetchMaskOverlay } from "../api/frames";
import type { LinearShape, LocalAdjustment, MaskOverlayInfo, RadialShape, VariantId } from "../api/types";
import { applySettings, developStore, endDrag } from "../state/develop";
import {
  type Box,
  type BrushOptions,
  type GBounds,
  type LinearHandle,
  type RadialHandle,
  boxToG,
  dragLinear,
  dragRadial,
  gToBox,
  overlayRgba,
  pixelsPerG,
  radialAxes,
  withShape,
} from "./maskLogic";
import { commitBrushStroke, selectComponent } from "./viewState";

/** つまみの半径（CSS の画素）。 */
const KNOB = 6;
/** 円形の回転のつまみを、縦の半径の外にどれだけ離すか（CSS の画素）。 */
const ROTATE_OFFSET = 24;

type Drag =
  | { kind: "linear"; j: number; handle: LinearHandle; start: LinearShape; p0: [number, number]; pointer: number }
  | { kind: "radial"; j: number; handle: RadialHandle; start: RadialShape; p0: [number, number]; pointer: number };

/** マスクの操作の層（ステージ全体を覆う svg。つまみとブラシのときだけポインターを受ける）。 */
export function MaskLayer(props: {
  width: number;
  height: number;
  box: Box;
  gb: GBounds;
  mask: LocalAdjustment;
  index: number;
  component: number | null;
  painting: boolean;
  brush: BrushOptions;
  editable: boolean;
}) {
  const { box, gb, mask, index, component, editable } = props;
  const svgRef = useRef<SVGSVGElement>(null);
  const drag = useRef<Drag | null>(null);
  const [stroke, setStroke] = useState<{ points: [number, number][]; erase: boolean; pointer: number } | null>(null);
  const [cursor, setCursor] = useState<{ x: number; y: number; erase: boolean } | null>(null);
  const ppg = pixelsPerG(gb, box);

  // ブラシをやめたら、描きかけのストロークと印を消す。
  useEffect(() => {
    if (!props.painting) {
      setStroke(null);
      setCursor(null);
    }
  }, [props.painting]);

  /** ポインターの位置（ステージの CSS の画素）。 */
  const local = (e: { clientX: number; clientY: number }): [number, number] => {
    const r = svgRef.current?.getBoundingClientRect();
    return r ? [e.clientX - r.left, e.clientY - r.top] : [e.clientX, e.clientY];
  };
  const toG = (e: { clientX: number; clientY: number }): [number, number] => {
    const [x, y] = local(e);
    return boxToG(gb, box, x, y);
  };

  const startDrag = (j: number, kind: "linear" | "radial", handle: LinearHandle | RadialHandle) =>
    (e: ReactPointerEvent<SVGElement>) => {
      if (e.button !== 0 || !editable || props.painting) return;
      e.stopPropagation();
      e.preventDefault();
      const c = mask.components[j];
      if (!c) return;
      svgRef.current?.setPointerCapture(e.pointerId);
      if (component !== j) selectComponent(j);
      const p0 = toG(e);
      if (kind === "linear" && "linear" in c.shape) {
        drag.current = { kind, j, handle: handle as LinearHandle, start: c.shape.linear, p0, pointer: e.pointerId };
      } else if (kind === "radial" && "radial" in c.shape) {
        drag.current = { kind, j, handle: handle as RadialHandle, start: c.shape.radial, p0, pointer: e.pointerId };
      }
    };

  const onPointerDown = (e: ReactPointerEvent<SVGSVGElement>) => {
    if (!props.painting || e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    svgRef.current?.setPointerCapture(e.pointerId);
    const erase = props.brush.erase !== e.altKey;
    setStroke({ points: [toG(e)], erase, pointer: e.pointerId });
  };
  const onPointerMove = (e: ReactPointerEvent<SVGSVGElement>) => {
    if (props.painting) {
      const [x, y] = local(e);
      setCursor({ x, y, erase: props.brush.erase !== e.altKey });
      if (stroke && stroke.pointer === e.pointerId) {
        const p = toG(e);
        setStroke((s) => (s ? { ...s, points: [...s.points, p] } : s));
      }
      return;
    }
    const d = drag.current;
    if (!d || d.pointer !== e.pointerId) return;
    const p = toG(e);
    const s = developStore.get().settings;
    if (!s) return;
    const next =
      d.kind === "linear"
        ? withShape(s, index, d.j, "linear", () => dragLinear(d.start, d.handle, p[0] - d.p0[0], p[1] - d.p0[1]))
        : withShape(s, index, d.j, "radial", () => dragRadial(d.start, d.handle, p, d.p0));
    if (next !== s) applySettings(next, true);
  };
  const onPointerUp = (e: ReactPointerEvent<SVGSVGElement>) => {
    if (stroke && stroke.pointer === e.pointerId) {
      const points = [...stroke.points, toG(e)];
      setStroke(null);
      commitBrushStroke(points, stroke.erase);
      return;
    }
    const d = drag.current;
    if (!d || d.pointer !== e.pointerId) return;
    drag.current = null;
    endDrag();
  };

  const at = (u: number, v: number) => gToBox(gb, box, u, v);
  const diag = Math.hypot(props.width, props.height) * 2;

  const shapes = mask.components.map((c, j) => {
    const selected = j === component;
    const cls = `dv-mask-shape${selected ? " selected" : ""}${c.mode === "subtract" ? " subtract" : ""}`;
    if ("linear" in c.shape) {
      const l = c.shape.linear;
      const [x0, y0] = at(l.x0, l.y0);
      const [x1, y1] = at(l.x1, l.y1);
      const len = Math.hypot(x1 - x0, y1 - y0);
      // 向きの垂直（長さが 0 なら横の線）。
      const [nx, ny] = len > 1e-6 ? [-(y1 - y0) / len, (x1 - x0) / len] : [1, 0];
      const line = (x: number, y: number) => ({
        x1: x - nx * diag,
        y1: y - ny * diag,
        x2: x + nx * diag,
        y2: y + ny * diag,
      });
      const mx = (x0 + x1) / 2;
      const my = (y0 + y1) / 2;
      return (
        <g key={j} className={cls}>
          <line className="dv-mask-line full" {...line(x0, y0)} />
          <line className="dv-mask-line zero" {...line(x1, y1)} />
          <line className="dv-mask-line mid" {...line(mx, my)} />
          <line className="dv-mask-grip move" {...line(mx, my)} onPointerDown={startDrag(j, "linear", "move")} />
          <line className="dv-mask-axis" x1={x0} y1={y0} x2={x1} y2={y1} />
          <circle className="dv-mask-knob" cx={x0} cy={y0} r={KNOB} onPointerDown={startDrag(j, "linear", "start")}>
            <title>100% の位置（ドラッグで動かす）</title>
          </circle>
          <circle className="dv-mask-knob zero" cx={x1} cy={y1} r={KNOB} onPointerDown={startDrag(j, "linear", "end")}>
            <title>0% の位置（ドラッグで動かす）</title>
          </circle>
        </g>
      );
    }
    if ("radial" in c.shape) {
      const r = c.shape.radial;
      const [cx, cy] = at(r.cx, r.cy);
      const rx = r.rx * ppg;
      const ry = r.ry * ppg;
      const inner = Math.max(0, 1 - r.feather / 100);
      const { ex, ey } = radialAxes(r.angle_deg);
      const hx: [number, number] = [cx + ex[0] * rx, cy + ex[1] * rx];
      const hy: [number, number] = [cx + ey[0] * ry, cy + ey[1] * ry];
      const top: [number, number] = [cx - ey[0] * ry, cy - ey[1] * ry];
      const rot: [number, number] = [cx - ey[0] * (ry + ROTATE_OFFSET), cy - ey[1] * (ry + ROTATE_OFFSET)];
      const transform = `rotate(${r.angle_deg} ${cx} ${cy})`;
      return (
        <g key={j} className={cls}>
          <ellipse className="dv-mask-line full" cx={cx} cy={cy} rx={rx} ry={ry} transform={transform} />
          {inner > 0 && inner < 1 && (
            <ellipse className="dv-mask-line mid" cx={cx} cy={cy} rx={rx * inner} ry={ry * inner} transform={transform} />
          )}
          <ellipse
            className="dv-mask-grip move"
            cx={cx}
            cy={cy}
            rx={rx}
            ry={ry}
            transform={transform}
            onPointerDown={startDrag(j, "radial", "move")}
          />
          <line className="dv-mask-axis" x1={top[0]} y1={top[1]} x2={rot[0]} y2={rot[1]} />
          <circle className="dv-mask-knob" cx={cx} cy={cy} r={KNOB - 2} onPointerDown={startDrag(j, "radial", "move")}>
            <title>中心（ドラッグで動かす）</title>
          </circle>
          <circle className="dv-mask-knob" cx={hx[0]} cy={hx[1]} r={KNOB} onPointerDown={startDrag(j, "radial", "rx")}>
            <title>横の半径</title>
          </circle>
          <circle className="dv-mask-knob" cx={hy[0]} cy={hy[1]} r={KNOB} onPointerDown={startDrag(j, "radial", "ry")}>
            <title>縦の半径</title>
          </circle>
          <circle
            className="dv-mask-knob rotate"
            cx={rot[0]}
            cy={rot[1]}
            r={KNOB}
            onPointerDown={startDrag(j, "radial", "rotate")}
          >
            <title>回転</title>
          </circle>
        </g>
      );
    }
    if ("color_range" in c.shape) {
      return (
        <g key={j} className={cls}>
          {c.shape.color_range.samples.map((sm, k) => {
            const [x, y] = at(sm.x, sm.y);
            return (
              <g key={k} className="dv-mask-sample">
                <circle cx={x} cy={y} r={5} />
                <line x1={x - 9} y1={y} x2={x - 5} y2={y} />
                <line x1={x + 5} y1={y} x2={x + 9} y2={y} />
                <line x1={x} y1={y - 9} x2={x} y2={y - 5} />
                <line x1={x} y1={y + 5} x2={x} y2={y + 9} />
              </g>
            );
          })}
        </g>
      );
    }
    return null;
  });

  const size = props.brush.size * ppg;
  const innerSize = size * Math.max(0, 1 - props.brush.feather / 100);
  return (
    <svg
      ref={svgRef}
      className={`dv-layer dv-mask${props.painting ? " painting" : ""}`}
      style={{ left: 0, top: 0, width: props.width, height: props.height }}
      width={props.width}
      height={props.height}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onPointerLeave={() => setCursor(null)}
      onClick={(e) => {
        if (props.painting) e.stopPropagation();
      }}
    >
      {shapes}
      {stroke && stroke.points.length > 0 && (
        <polyline
          className={`dv-mask-stroke${stroke.erase ? " erase" : ""}`}
          points={stroke.points.map(([u, v]) => at(u, v).join(",")).join(" ")}
          strokeWidth={Math.max(1, 2 * size)}
        />
      )}
      {props.painting && cursor && (
        <g className={`dv-mask-cursor${cursor.erase ? " erase" : ""}`}>
          <circle cx={cursor.x} cy={cursor.y} r={Math.max(1, size)} />
          {innerSize > 1 && innerSize < size && <circle className="inner" cx={cursor.x} cy={cursor.y} r={innerSize} />}
          {cursor.erase && <line x1={cursor.x - 4} y1={cursor.y} x2={cursor.x + 4} y2={cursor.y} />}
        </g>
      )}
    </svg>
  );
}

/** 重ねて表示の canvas（重みを赤の半透明にして描く）。 */
export function MaskOverlayCanvas(props: { frame: Frame<MaskOverlayInfo>; style: Box }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    const f = props.frame;
    if (!canvas) return;
    if (canvas.width !== f.width) canvas.width = f.width;
    if (canvas.height !== f.height) canvas.height = f.height;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.putImageData(new ImageData(overlayRgba(f.pixels), f.width, f.height), 0, 0);
  }, [props.frame]);
  return <canvas ref={ref} className="dv-layer dv-mask-overlay" style={props.style} />;
}

/** 重ねて表示に使う画素を取る（`req` が null なら取らない）。 */
export function useMaskOverlay(
  req: { variantId: VariantId; index: number; generation: number; uncropped: boolean } | null,
): Frame<MaskOverlayInfo> | null {
  const [frame, setFrame] = useState<Frame<MaskOverlayInfo> | null>(null);
  const key = req ? `${req.variantId}:${req.index}:${req.generation}:${req.uncropped}` : null;
  const latest = useRef<{ key: string | null; req: typeof req }>({ key, req });
  latest.current = { key, req };
  const busy = useRef(false);
  const again = useRef(false);
  useEffect(() => {
    if (key === null) {
      setFrame(null);
      return;
    }
    if (busy.current) {
      again.current = true;
      return;
    }
    busy.current = true;
    void (async () => {
      try {
        do {
          again.current = false;
          const { key: k, req: r } = latest.current;
          if (k === null || !r) break;
          try {
            const f = await fetchMaskOverlay(r.variantId, r.index, r.uncropped);
            if (latest.current.key !== null && f) setFrame(f);
          } catch (e) {
            console.warn("マスクの重ねて表示を取得できません", e);
          }
        } while (again.current);
      } finally {
        busy.current = false;
      }
    })();
  }, [key]);
  if (!req || !frame) return null;
  const h = frame.header;
  return h.variant_id === req.variantId && h.index === req.index && h.uncropped === req.uncropped ? frame : null;
}
