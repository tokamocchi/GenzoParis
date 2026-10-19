// 現像のプレビューの上の、切り抜き後の周辺光量補正の中心の印（DEV-24）。
//
// 効果の区切りを開いていて量が 0 でないときに、中心（手動の中心か、切り抜きの中心）に印を出す。印をドラッグすると
// 中心をその位置にする（切り抜きの中心のときは手動に切り替わる）。ドラッグの間は dragging で送り、離したら end_drag で
// 履歴に 1 件（マスクのつまみと同じ）。位置は G 座標で求め、画面の画像の箱との対応は FrameInfo.g_bounds
// （maskLogic の gToBox・boxToG）。中心の値は切り抜きと同じ割合なので、G の外接矩形の大きさ（GeometryInfo）で直す。

import { type PointerEvent as ReactPointerEvent, useRef } from "react";

import type { DevelopSettings } from "../api/types";
import { applySettings, developStore, endDrag } from "../state/develop";
import { fullEffects, gToVignetteCenter, vignetteCenterOf, vignetteCenterToG, withVignette } from "./detailLogic";
import { type Box, type GBounds, boxToG, gToBox } from "./maskLogic";

/** 印の円の半径（CSS の画素）。 */
const KNOB = 7;
/** 十字の線の長さ（円の外側。CSS の画素）。 */
const ARM = 9;

/** 周辺光量補正の中心の印の層（ステージ全体を覆う svg。印だけがポインターを受ける）。 */
export function VignetteLayer(props: {
  width: number;
  height: number;
  box: Box;
  gb: GBounds;
  settings: DevelopSettings;
  /** G の外接矩形の大きさ（比だけを使う。GeometryInfo の gw・gh）。 */
  gw: number;
  gh: number;
}) {
  const { box, gb, gw, gh } = props;
  const svgRef = useRef<SVGSVGElement>(null);
  const drag = useRef<number | null>(null);
  const manual = fullEffects(props.settings.effects).vignette.center != null;
  const [u, v] = vignetteCenterToG(vignetteCenterOf(props.settings), gw, gh);
  const [cx, cy] = gToBox(gb, box, u, v);

  const toCenter = (e: { clientX: number; clientY: number }) => {
    const r = svgRef.current?.getBoundingClientRect();
    const [x, y] = r ? [e.clientX - r.left, e.clientY - r.top] : [e.clientX, e.clientY];
    const [pu, pv] = boxToG(gb, box, x, y);
    return gToVignetteCenter(pu, pv, gw, gh);
  };

  const onPointerDown = (e: ReactPointerEvent<SVGElement>) => {
    if (e.button !== 0) return;
    // ステージのクリック（等倍表示への切り替え）にしない。
    e.stopPropagation();
    e.preventDefault();
    svgRef.current?.setPointerCapture(e.pointerId);
    drag.current = e.pointerId;
  };
  const onPointerMove = (e: ReactPointerEvent<SVGSVGElement>) => {
    if (drag.current !== e.pointerId) return;
    const s = developStore.get().settings;
    if (!s) return;
    const c = toCenter(e);
    const cur = fullEffects(s.effects).vignette.center;
    if (cur && cur.x === c.x && cur.y === c.y) return;
    applySettings(withVignette(s, { center: c }), true);
  };
  const onPointerUp = (e: ReactPointerEvent<SVGSVGElement>) => {
    if (drag.current !== e.pointerId) return;
    drag.current = null;
    endDrag();
  };

  return (
    <svg
      ref={svgRef}
      className="dv-layer dv-vignette"
      style={{ left: 0, top: 0, width: props.width, height: props.height }}
      width={props.width}
      height={props.height}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
    >
      <g className={`dv-vignette-center${manual ? " manual" : ""}`}>
        <line x1={cx - KNOB - ARM} y1={cy} x2={cx - KNOB} y2={cy} />
        <line x1={cx + KNOB} y1={cy} x2={cx + KNOB + ARM} y2={cy} />
        <line x1={cx} y1={cy - KNOB - ARM} x2={cx} y2={cy - KNOB} />
        <line x1={cx} y1={cy + KNOB} x2={cx} y2={cy + KNOB + ARM} />
        <circle className="dv-vignette-knob" cx={cx} cy={cy} r={KNOB} onPointerDown={onPointerDown}>
          <title>
            {manual
              ? "周辺光量補正の中心（ドラッグで動かす）"
              : "周辺光量補正の中心（切り抜きの中心。ドラッグすると手動で動かせます）"}
          </title>
        </circle>
      </g>
    </svg>
  );
}
