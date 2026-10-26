// 現像のプレビューの上のスポット修正（DEV-21。修復・コピースタンプ）の表示と操作。
//
// - 道具（tool が retouch。Q）を使っている間だけ出す。スポットごとに、消したい場所（実線。円か線）とコピー元（点線）と、
//   その間を結ぶ線を描く。選んでいるスポットは強く描く。
// - 空いているところをクリックすると円、ドラッグすると線（電線など。押してから離すまで）のスポットを足す（コピー元は
//   コアが選ぶ。viewState の commitRetouchSpot）。足したスポットがコアに届くまで、描いた形を残す。
// - スポット（消したい場所）を押すと選び、そのままドラッグすると動かす（コピー元も一緒に動く）。コピー元をドラッグすると
//   コピー元だけを動かす。ドラッグの間は dragging で送り、離したら end_drag で履歴に 1 件。
// - 位置は O 座標で、画面の画像の箱との対応は FrameInfo.o_grid（retouchLogic の boxToO・oToBox）。

import { type PointerEvent as ReactPointerEvent, useEffect, useRef, useState } from "react";

import type { RetouchSpot } from "../api/types";
import { applySettings, developStore, endDrag } from "../state/develop";
import type { Box } from "./maskLogic";
import {
  type OGrid,
  boxToO,
  densify,
  densifyStep,
  insideBox,
  moveSource,
  moveSpot,
  oToBox,
  pixelsPerO,
  withSpot,
} from "./retouchLogic";
import { commitRetouchSpot, selectSpot } from "./viewState";

/** クリックとドラッグを分ける動きの量（CSS の画素。仮置き）。 */
const CLICK_SLOP_PX = 4;

type Drag = { kind: "dest" | "source"; index: number; start: RetouchSpot; p0: [number, number]; pointer: number };

/** 描いている途中・コアに届くまでの形（O 座標）。 */
interface Drawn {
  points: [number, number][];
  /** 押した位置（ステージの CSS の画素。クリックかドラッグかを分ける）。 */
  origin: [number, number];
  moved: boolean;
  pointer: number;
}

export function RetouchLayer(props: {
  width: number;
  height: number;
  box: Box;
  grid: OGrid;
  spots: RetouchSpot[];
  selected: number | null;
  editable: boolean;
  /** 新しいスポットを描けるか（数が上限に達していない）。 */
  canAdd: boolean;
  /** 次に足すスポットの大きさ（半径。O 座標）。 */
  size: number;
}) {
  const { box, grid, spots, selected, editable, canAdd } = props;
  const svgRef = useRef<SVGSVGElement>(null);
  const drag = useRef<Drag | null>(null);
  const [drawn, setDrawn] = useState<Drawn | null>(null);
  const [pending, setPending] = useState<{ points: [number, number][]; count: number } | null>(null);
  const [cursor, setCursor] = useState<[number, number] | null>(null);

  // 足したスポットが設定に入ったら（数が変わったら）、残していた形を消す。
  useEffect(() => {
    setPending((p) => (p && spots.length !== p.count ? null : p));
  }, [spots.length]);

  /** O の幅（長辺を 1 とする。表の右端の点の大きい方）。 */
  const oWidth = Math.max(...grid.points.map((p) => p[0]), 0);

  const local = (e: { clientX: number; clientY: number }): [number, number] => {
    const r = svgRef.current?.getBoundingClientRect();
    return r ? [e.clientX - r.left, e.clientY - r.top] : [e.clientX, e.clientY];
  };
  const toO = (e: { clientX: number; clientY: number }): [number, number] => {
    const [x, y] = local(e);
    return boxToO(grid, box, x, y);
  };

  const startDrag = (index: number, kind: "dest" | "source") => (e: ReactPointerEvent<SVGElement>) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    if (selected !== index) selectSpot(index);
    const spot = spots[index];
    if (!editable || !spot) return;
    svgRef.current?.setPointerCapture(e.pointerId);
    drag.current = { kind, index, start: spot, p0: toO(e), pointer: e.pointerId };
  };

  const onPointerDown = (e: ReactPointerEvent<SVGSVGElement>) => {
    if (e.button !== 0 || !editable || !canAdd) return;
    // 画像のまわりの余白では描き始めない（画像の外の見えないスポットを足さない）。
    const origin = local(e);
    if (!insideBox(box, origin[0], origin[1])) return;
    e.stopPropagation();
    e.preventDefault();
    svgRef.current?.setPointerCapture(e.pointerId);
    setDrawn({ points: [toO(e)], origin, moved: false, pointer: e.pointerId });
  };
  const onPointerMove = (e: ReactPointerEvent<SVGSVGElement>) => {
    setCursor(local(e));
    const d = drag.current;
    if (d && d.pointer === e.pointerId) {
      const p = toO(e);
      const s = developStore.get().settings;
      if (!s) return;
      const du = p[0] - d.p0[0];
      const dv = p[1] - d.p0[1];
      const next = withSpot(s, d.index, () => (d.kind === "dest" ? moveSpot(d.start, du, dv) : moveSource(d.start, du, dv)));
      if (next !== s) applySettings(next, true);
      return;
    }
    if (drawn && drawn.pointer === e.pointerId) {
      const [x, y] = local(e);
      const moved = drawn.moved || Math.hypot(x - drawn.origin[0], y - drawn.origin[1]) >= CLICK_SLOP_PX;
      setDrawn({ ...drawn, moved, points: moved ? [...drawn.points, toO(e)] : drawn.points });
    }
  };
  const onPointerUp = (e: ReactPointerEvent<SVGSVGElement>) => {
    const d = drag.current;
    if (d && d.pointer === e.pointerId) {
      drag.current = null;
      endDrag();
      return;
    }
    if (drawn && drawn.pointer === e.pointerId) {
      // クリックなら円（押した位置の 1 点）、ドラッグなら線。
      const points = drawn.moved ? [...drawn.points, toO(e)] : [drawn.points[0]];
      setDrawn(null);
      setPending({ points, count: spots.length });
      // 足せなかったら、残していた形を消す。
      void commitRetouchSpot(points, oWidth > 0 ? oWidth : 1).then((added) => {
        if (!added) setPending(null);
      });
    }
  };

  const at = (u: number, v: number) => oToBox(grid, box, u, v);
  // 線分は O の上で細かくしてから画面の位置にする（回転・歪曲補正で曲がる線を、直す場所に沿って描く）。
  const pts = (points: [number, number][], du = 0, dv = 0) =>
    densify(points, densifyStep(grid, oWidth))
      .map(([u, v]) => at(u + du, v + dv))
      .filter((p): p is [number, number] => p !== null)
      .map((p) => p.join(","))
      .join(" ");

  /** スポットの形（消したい場所かコピー元）。 */
  const shape = (spot: { points: [number, number][]; size: number }, cls: string, du: number, dv: number, onDown?: (e: ReactPointerEvent<SVGElement>) => void) => {
    const first = spot.points[0];
    if (!first) return null;
    const ppo = pixelsPerO(grid, box, first[0] + du, first[1] + dv);
    const r = Math.max(2, spot.size * ppo);
    if (spot.points.length === 1) {
      const c = at(first[0] + du, first[1] + dv);
      if (!c) return null;
      return <circle className={cls} cx={c[0]} cy={c[1]} r={r} onPointerDown={onDown} />;
    }
    return (
      // 太さは CSS の stroke-width より強い style で決める（線の太さがスポットの大きさ）。
      <polyline className={cls} points={pts(spot.points, du, dv)} style={{ strokeWidth: 2 * r }} onPointerDown={onDown} />
    );
  };

  const items = spots.map((spot, i) => {
    if (spot.points.length === 0) return null;
    const sel = i === selected;
    const [du, dv] = spot.offset;
    const a = at(spot.points[0][0], spot.points[0][1]);
    const b = at(spot.points[0][0] + du, spot.points[0][1] + dv);
    const stroke = spot.points.length >= 2;
    const cls = `dv-retouch${sel ? " selected" : ""}${spot.enabled ? "" : " disabled"}${stroke ? " stroke" : ""}`;
    return (
      <g key={i} className={cls}>
        {a && b && <line className="dv-retouch-link" x1={a[0]} y1={a[1]} x2={b[0]} y2={b[1]} />}
        {shape(spot, "dv-retouch-source", du, dv, startDrag(i, "source"))}
        {shape(spot, "dv-retouch-dest", 0, 0, startDrag(i, "dest"))}
      </g>
    );
  });

  const live = drawn ? drawn.points : pending?.points ?? null;
  const cursorR = cursor ? Math.max(2, props.size * pixelsPerO(grid, box, ...boxToO(grid, box, cursor[0], cursor[1]))) : 0;
  return (
    <svg
      ref={svgRef}
      className="dv-layer dv-retouch-layer"
      style={{ left: 0, top: 0, width: props.width, height: props.height }}
      width={props.width}
      height={props.height}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onPointerLeave={() => setCursor(null)}
      onClick={(e) => e.stopPropagation()}
    >
      {items}
      {live && live.length > 0 && <g className="dv-retouch drawing">{shape({ points: live, size: props.size }, "dv-retouch-dest", 0, 0)}</g>}
      {editable && canAdd && cursor && !drawn && insideBox(box, cursor[0], cursor[1]) && <circle className="dv-retouch-cursor" cx={cursor[0]} cy={cursor[1]} r={cursorR} />}
    </svg>
  );
}
