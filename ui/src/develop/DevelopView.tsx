// 現像のプレビュー（中央。DEV-01〜05・07・08・15・26、SYS-03、PRV-02・03）。
//
// - 選んだ写真（selection.current）が変わったら、現像のために開く（state/develop.ts の openDevelopFor）。
//   動画は現像できないので開かない。
// - コアが描いたプレビュー（developStore.frame。Display P3 か sRGB の 8bit）を canvas に描き、枠の中央に
//   全体を表示する（CSS の大きさで縮小する。HiDPI では画像の 1 画素を画面の 1 物理画素より大きくしない）。
// - クリッピングの表示（J）: 白飛びを赤、黒つぶれを青で、別の canvas に重ねる（表示中の画素から計算）。
// - 等倍表示（Z・クリック）: 見えている範囲だけをコアにフル解像度で描かせる（state/region.ts。画素数は上限
//   までに抑え、外れた部分はプレビューの拡大のまま見せる。zoomRegionRequest）。ドラッグで
//   パン、設定が変わったらプレビューの更新の後に描き直す。Esc・Z・クリックで全体表示に戻す。
// - スポイト（W）: クリックした点を中立にする WB にする。
// - 切り抜きの編集（R）: 切り抜く前の全体の上に枠を重ねる（四隅・四辺・中を動かせる、外側を暗く、三分割の
//   線）。Enter で完了、Esc で取り消し。
// - ビフォー / アフター（DEV-31。\・Y・Shift ＋ Y・下のツールバー）: ビフォー（既定は取り込んだ直後の設定。
//   切り抜き・角度はアフターと同じ）を、ビフォーだけ・左右に並べる・分割で出す。等倍表示の中心は両方で同じ
//   （面ごとに、アフターは render_region、ビフォーは render_before_region で描く）。compareMath.ts の doc。
// - 撮影情報（LIB-17。I）: ルーペと同じ表示を重ねる（寸法は切り抜いた後の出力の大きさ）。
// - ローカル補正（マスク。DEV-18・DEV-19）: 選んでいるマスクの線形・円形のつまみ・色域の見本の印を重ね、ブラシで描く
//   （MaskLayer.tsx）。重ねて表示（O）は選んでいるマスクの重みを赤で重ねる。色域のスポイトはクリックした点の色を
//   見本にする。ビフォー / アフターの表示・切り抜きの編集の間は出さない。全体表示でも等倍表示でも、画像の箱と
//   FrameInfo.g_bounds で位置を合わせる。
// - 画像のまわりは色みのない灰色だけにする（写真の色の判断に影響しないように）。

import {
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import { type Frame, drawFrame } from "../api/frames";
import type { BeforeSource, CropRect, FrameInfo, RegionInfo, VariantId } from "../api/types";
import { RANGES } from "../api/types";
import { comboLabel, isTyping, useHotkeys } from "../keyboard";
import { actions, appStore, useApp } from "../state/app";
import {
  applySettings,
  beforePending,
  developStore,
  openDevelopFor,
  setBeforeSource,
  useDevelop,
} from "../state/develop";
import { regionFor, renderBeforeRegion, renderRegion } from "../state/region";
import { useDevicePixelRatio } from "../viewer/hooks";
import { InfoOverlay } from "../viewer/InfoOverlay";
import { cycleInfoMode, infoModeLabel, useInfo } from "../viewer/infoItems";
import { expandRegion, limitRegion, MAX_REGION_PIXELS, type Rect } from "../viewer/viewerMath";
import {
  COMPARE_MODES,
  type Pane,
  type PaneTarget,
  comparePanes,
  mapCenter,
  paneLabel,
  paneSize,
  splitX,
} from "./compareMath";
import { colorControls, toggleBlackWhite } from "./colorLogic";
import { type CropHandle, cropFractionAspect, dragCrop, rotatedImageCorners } from "./cropMath";
import { useGeometryInfo, useLoadedSummary, useLoadedSummaryObject } from "./hooks";
import { activatesOnEnter } from "./keyTargets";
import { MaskLayer, MaskOverlayCanvas, useMaskOverlay } from "./MaskLayer";
import { type Box, boundsOrDefault, maskControls } from "./maskLogic";
import { clippingOverlay } from "./pixels";
import {
  cancelCrop,
  cancelMaskTool,
  cancelWbTool,
  commitCrop,
  constrainDraft,
  currentMask,
  endAllTools,
  pickMaskColor,
  pickWhiteBalance,
  setBrushOptions,
  setCompareMode,
  setDraftCrop,
  setZoom,
  startCrop,
  takeWantedBefore,
  toggleClipping,
  toggleCompareMode,
  toggleMaskOverlay,
  toggleWbTool,
  useDevelopView,
  viewStore,
} from "./viewState";
import { clampCenter, fitBox, zoomOrigin } from "./zoomMath";
import "./develop.css";

/** 設定が変わってプレビューが更新された後、等倍表示を描き直すまでの時間（ミリ秒。仮置き）。 */
const REGION_REDRAW_DELAY_MS = 250;
/** パン・大きさの変更の後、等倍表示を描き直すまでの時間（ミリ秒。仮置き）。 */
const REGION_PAN_DELAY_MS = 60;
/** クリックとドラッグを分ける動きの量（CSS の画素。仮置き）。 */
const CLICK_SLOP_PX = 4;
/** 切り抜きの編集中に画像のまわりに空ける幅（CSS の画素。仮置き）。枠の端のつまみをつかめるようにする。 */
const CROP_INSET_PX = 24;
/** ブラシの大きさを [ ・ ] で変える倍率（仮置き）。 */
const BRUSH_SIZE_STEP = 1.25;

/**
 * 等倍表示で要求する範囲（出力の画素）。見えている範囲を 1 画素ずつ広げ（端の丸めで外れないように）、
 * 画素数を上限（MAX_REGION_PIXELS。コアの region.rs の MAX_REGION_SOURCE_PIXELS）までに抑える。ルーペと
 * 同じ。大きな画面（8K など）では見えている範囲が上限を超え、そのまま要求するとコアがエラーにするので、
 * 中心を保って縮め、外れた部分は現像のプレビューの拡大のまま見せる。
 */
export function zoomRegionRequest(opts: Parameters<typeof regionFor>[0]): Rect {
  const visible = regionFor(opts);
  return limitRegion(expandRegion(visible, 1, opts.fullWidth, opts.fullHeight), MAX_REGION_PIXELS);
}

/** 要素の大きさ（CSS の画素）を見張る。 */
function useElementSize(ref: RefObject<HTMLElement | null>): { width: number; height: number } {
  const [size, setSize] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => {
      const r = el.getBoundingClientRect();
      setSize((s) => (s.width === r.width && s.height === r.height ? s : { width: r.width, height: r.height }));
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return size;
}

/** 画素を描く canvas（frame が変わったら描き直す）。`fit` は全体表示の画像の印（クリックした点を求めるのに使う）。 */
function FrameCanvas(props: { frame: Frame<unknown> | null; className?: string; style?: CSSProperties; fit?: boolean }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    if (ref.current && props.frame) drawFrame(ref.current, props.frame);
  }, [props.frame]);
  return <canvas ref={ref} className={props.className} style={props.style} data-fit={props.fit ? "" : undefined} />;
}

/** クリッピングの表示（frame の画素から計算して重ねる）。 */
function ClippingCanvas(props: { frame: Frame<unknown>; style: CSSProperties }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    const f = props.frame;
    if (!canvas) return;
    const overlay = clippingOverlay(f.pixels, f.width, f.height);
    if (canvas.width !== f.width) canvas.width = f.width;
    if (canvas.height !== f.height) canvas.height = f.height;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.putImageData(new ImageData(overlay.data, f.width, f.height), 0, 0);
  }, [props.frame]);
  return <canvas ref={ref} className="dv-layer dv-clip" style={props.style} />;
}

// ---------------------------------------------------------------------------
// 切り抜きの枠
// ---------------------------------------------------------------------------

const HANDLES: CropHandle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];
/** つまみのつかめる幅（CSS の画素。仮置き）。 */
const GRIP = 14;

function CropOverlay(props: {
  box: { left: number; top: number; width: number; height: number };
  crop: CropRect;
  aspect: number | null;
  imageCorners: [number, number][] | null;
}) {
  const { box, crop } = props;
  const drag = useRef<{ handle: CropHandle; start: CropRect; x: number; y: number; pointer: number } | null>(null);
  const W = box.width;
  const H = box.height;
  const x0 = crop.left * W;
  const x1 = crop.right * W;
  const y0 = crop.top * H;
  const y1 = crop.bottom * H;
  const cw = x1 - x0;
  const ch = y1 - y0;

  const onDown = (handle: CropHandle) => (e: ReactPointerEvent<SVGElement>) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    (e.currentTarget.ownerSVGElement ?? (e.currentTarget as unknown as SVGSVGElement)).setPointerCapture(e.pointerId);
    drag.current = { handle, start: { ...props.crop }, x: e.clientX, y: e.clientY, pointer: e.pointerId };
  };
  const onMove = (e: ReactPointerEvent<SVGSVGElement>) => {
    const d = drag.current;
    if (!d || d.pointer !== e.pointerId || !(W > 0) || !(H > 0)) return;
    const dx = (e.clientX - d.x) / W;
    const dy = (e.clientY - d.y) / H;
    setDraftCrop(dragCrop(d.start, d.handle, dx, dy, props.aspect));
  };
  const onUp = (e: ReactPointerEvent<SVGSVGElement>) => {
    const d = drag.current;
    if (!d || d.pointer !== e.pointerId) return;
    drag.current = null;
    // 回転した画像の外に出た部分を制限する（コアの constrain_crop）。
    void constrainDraft();
  };

  const handleRect = (h: CropHandle) => {
    const cx = h.includes("w") ? x0 : h.includes("e") ? x1 : (x0 + x1) / 2;
    const cy = h.includes("n") ? y0 : h.includes("s") ? y1 : (y0 + y1) / 2;
    const corner = h.length === 2;
    // 辺のつまみは辺に沿って長く、角のつまみは正方形。
    const gw = corner ? GRIP : h === "n" || h === "s" ? Math.max(GRIP, cw - 2 * GRIP) : GRIP;
    const gh = corner ? GRIP : h === "e" || h === "w" ? Math.max(GRIP, ch - 2 * GRIP) : GRIP;
    return { x: cx - gw / 2, y: cy - gh / 2, width: gw, height: gh };
  };

  // 端のつまみの外側の半分も当たるように、svg を画像より GRIP だけ広くして中身をずらす（svg の外に
  // はみ出した部分は、WebKit ではポインターが当たらないため）。
  const M = GRIP;
  return (
    <svg
      className="dv-layer dv-crop"
      style={{ left: box.left - M, top: box.top - M, width: W + 2 * M, height: H + 2 * M }}
      width={W + 2 * M}
      height={H + 2 * M}
      onPointerMove={onMove}
      onPointerUp={onUp}
      onPointerCancel={onUp}
      onClick={(e) => e.stopPropagation()}
    >
      <g transform={`translate(${M} ${M})`}>
        <path className="dv-crop-shade" fillRule="evenodd" d={`M0 0H${W}V${H}H0Z M${x0} ${y0}H${x1}V${y1}H${x0}Z`} />
        {props.imageCorners && (
          <polygon
            className="dv-crop-image"
            points={props.imageCorners.map(([u, v]) => `${u * W},${v * H}`).join(" ")}
          />
        )}
        {[1, 2].map((i) => (
          <g key={i} className="dv-crop-thirds">
            <line x1={x0 + (cw * i) / 3} y1={y0} x2={x0 + (cw * i) / 3} y2={y1} />
            <line x1={x0} y1={y0 + (ch * i) / 3} x2={x1} y2={y0 + (ch * i) / 3} />
          </g>
        ))}
        <rect className="dv-crop-move" x={x0} y={y0} width={cw} height={ch} onPointerDown={onDown("move")} />
        <rect className="dv-crop-frame" x={x0} y={y0} width={cw} height={ch} />
        {HANDLES.map((h) => {
          const r = handleRect(h);
          const corner = h.length === 2;
          return (
            <g key={h}>
              {corner && (
                <rect
                  className="dv-crop-knob"
                  x={(h.includes("w") ? x0 : x1) - 4}
                  y={(h.includes("n") ? y0 : y1) - 4}
                  width={8}
                  height={8}
                />
              )}
              <rect className={`dv-crop-grip grip-${h}`} {...r} onPointerDown={onDown(h)} />
            </g>
          );
        })}
      </g>
    </svg>
  );
}

// ---------------------------------------------------------------------------
// 表示
// ---------------------------------------------------------------------------

/** 等倍表示の範囲の描画の要求の材料（面ごと）。 */
interface ZoomTarget {
  target: PaneTarget;
  variantId: VariantId | null;
  /** 出力の画像のフル解像度の大きさ（この面の画像の）。 */
  outW: number;
  outH: number;
  /** 描き直しの合図（アフターは設定の世代、ビフォーはビフォーの画素の世代）。 */
  generation: number | null;
  /** 中心（この面の出力の画素）。 */
  center: { centerX: number; centerY: number } | null;
}

/**
 * 等倍表示の範囲を描く（設定が変わったらプレビューの更新の後に少し待って、パン・大きさの変更でも描き直す）。
 * アフターは renderRegion、ビフォーは renderBeforeRegion（DEV-31。互いに取り消さない）。
 */
function useZoomRegion(
  z: ZoomTarget,
  view: { width: number; height: number },
  dpr: number,
): Frame<RegionInfo> | null {
  const [region, setRegion] = useState<Frame<RegionInfo> | null>(null);
  const lastGen = useRef<number | null>(null);
  const active = useRef(false);
  const zoomed = z.center !== null && z.outW > 0 && z.outH > 0;
  active.current = zoomed;
  useEffect(() => {
    if (!zoomed || z.variantId === null || z.generation === null || !(view.width > 0) || !(view.height > 0)) {
      if (!zoomed) {
        setRegion(null);
        lastGen.current = null;
      }
      return;
    }
    const center = z.center as { centerX: number; centerY: number };
    const delay =
      lastGen.current !== null && lastGen.current !== z.generation ? REGION_REDRAW_DELAY_MS : REGION_PAN_DELAY_MS;
    lastGen.current = z.generation;
    const req = zoomRegionRequest({
      fullWidth: z.outW,
      fullHeight: z.outH,
      centerX: center.centerX,
      centerY: center.centerY,
      scale: 1,
      viewWidth: view.width,
      viewHeight: view.height,
      devicePixelRatio: dpr,
    });
    const variantId = z.variantId;
    const timer = setTimeout(() => {
      const request = { variant_id: variantId, space: "output" as const, ...req, scale: 1 };
      const run = z.target === "before" ? renderBeforeRegion(request) : renderRegion(request);
      run
        .then((f) => {
          if (f && active.current && f.header.variant_id === developStore.get().variantId) setRegion(f);
        })
        .catch((e) => {
          actions.reportError(e, z.target === "before" ? "ビフォーの等倍表示を描けません" : "等倍表示を描けません");
          setZoom(null);
        });
    }, delay);
    return () => clearTimeout(timer);
    // 中心は値で比べる。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [zoomed, z.center?.centerX, z.center?.centerY, view.width, view.height, dpr, z.outW, z.outH, z.generation, z.variantId, z.target]);
  return zoomed && region && region.header.variant_id === z.variantId ? region : null;
}

/** 1 つの面（ビフォーかアフター）の画像: 全体表示か等倍表示（下敷きのプレビューと範囲の描画）。 */
function PaneImage(props: {
  pane: Pane;
  frame: Frame<FrameInfo> | null;
  region: Frame<RegionInfo> | null;
  /** 全体表示の配置（面の中の CSS の画素）。 */
  fit: { left: number; top: number; width: number; height: number } | null;
  /** 等倍表示の左上の画像の位置（この面の出力の画素）。全体表示なら null。 */
  origin: { x: number; y: number; outW: number; outH: number } | null;
  dpr: number;
  showClipping: boolean;
  /** 分割の表示で見せる範囲（面の左端からの幅。CSS の画素。null なら全体）。 */
  clipRight?: number | null;
  clipLeft?: number | null;
}) {
  const { pane, frame, region, fit, origin, dpr } = props;
  let fitStyle: CSSProperties | null = null;
  if (frame && fit) {
    fitStyle = origin
      ? // 等倍表示の下敷き: プレビューを等倍の大きさに引き伸ばす（範囲が届くまでの間に見せる）。
        { left: -origin.x / dpr, top: -origin.y / dpr, width: origin.outW / dpr, height: origin.outH / dpr }
      : { left: fit.left, top: fit.top, width: fit.width, height: fit.height };
  }
  const regionStyle: CSSProperties | null =
    origin && region
      ? {
          left: (region.header.x - origin.x) / dpr,
          top: (region.header.y - origin.y) / dpr,
          width: region.width / dpr,
          height: region.height / dpr,
        }
      : null;
  let clipPath: string | undefined;
  if (props.clipRight !== null && props.clipRight !== undefined) {
    clipPath = `inset(0 ${Math.max(0, pane.width - props.clipRight)}px 0 0)`;
  } else if (props.clipLeft !== null && props.clipLeft !== undefined) {
    clipPath = `inset(0 0 0 ${Math.max(0, props.clipLeft)}px)`;
  }
  return (
    <div
      className="dv-pane"
      data-target={pane.target}
      style={{ left: pane.left, top: pane.top, width: pane.width, height: pane.height, clipPath }}
    >
      {fitStyle && <FrameCanvas frame={frame} className="dv-layer dv-image" style={fitStyle} fit />}
      {region && regionStyle && <FrameCanvas frame={region} className="dv-layer dv-image" style={regionStyle} />}
      {props.showClipping && region && regionStyle && <ClippingCanvas frame={region} style={regionStyle} />}
      {props.showClipping && !region && frame && fitStyle && <ClippingCanvas frame={frame} style={fitStyle} />}
    </div>
  );
}

function Stage() {
  const rootRef = useRef<HTMLDivElement>(null);
  const size = useElementSize(rootRef);
  // モニター間の移動・表示倍率の変更でも読み直す（ルーペと同じ。viewer/hooks.ts）。
  const dpr = useDevicePixelRatio();
  const variantId = useDevelop((s) => s.variantId);
  const frame = useDevelop((s) => s.frame);
  const beforeFrame = useDevelop((s) => s.beforeFrame);
  const before = useDevelop((s) => s.before);
  const beforeBusy = useDevelop((s) => beforePending(s));
  const opening = useDevelop((s) => s.opening);
  const tool = useDevelopView((s) => s.tool);
  const zoom = useDevelopView((s) => s.zoom);
  const crop = useDevelopView((s) => s.crop);
  const compare = useDevelopView((s) => s.compare);
  const showClipping = useDevelopView((s) => s.showClipping);
  const angle = useDevelop((s) => s.settings?.geometry.angle_deg ?? 0);
  const info = useGeometryInfo();
  const settings = useDevelop((s) => s.settings);
  const hasState = useDevelop((s) => s.state !== null);
  const maskIndex = useDevelopView((s) => s.maskIndex);
  const componentIndex = useDevelopView((s) => s.componentIndex);
  const maskOverlay = useDevelopView((s) => s.maskOverlay);
  const brush = useDevelopView((s) => s.brush);

  const header: FrameInfo | null = frame?.header ?? null;
  const outW = header?.output_width ?? 0;
  const outH = header?.output_height ?? 0;
  const uncropped = !!header?.uncropped;
  const canZoom = outW > 0 && outH > 0 && !uncropped && !crop;
  const zoomed = !!zoom && canZoom;
  // ビフォー / アフターの表示（DEV-31）: 切り抜きの編集中・ビフォーを設定していないときはアフターだけ。
  const comparing = compare !== "off" && !crop && before !== null;
  const panes = comparePanes(compare, size.width, size.height, comparing);
  const view = paneSize(compare, size.width, size.height, comparing);
  const bHeader = beforeFrame && beforeFrame.header.variant_id === variantId ? beforeFrame.header : null;
  const bFrame = bHeader ? beforeFrame : null;
  const bOutW = bHeader?.output_width ?? 0;
  const bOutH = bHeader?.output_height ?? 0;
  const showsBefore = comparing && panes.some((p) => p.target === "before");
  const showsAfter = panes.some((p) => p.target === "after");

  // 全体表示の配置（HiDPI: 画像の 1 画素を画面の 1 物理画素より大きくしない）。切り抜きの編集中は
  // まわりを空ける（画像が枠いっぱいだと、端のつまみが表示の外にかかってつかめないため）。
  const inset = crop ? CROP_INSET_PX : 0;
  const fitFor = (f: Frame<FrameInfo> | null) => {
    if (!f) return null;
    const b = fitBox(view.width - 2 * inset, view.height - 2 * inset, f.width / dpr, f.height / dpr);
    return { ...b, left: b.left + inset, top: b.top + inset };
  };
  const fit = fitFor(frame);
  const bFit = fitFor(bFrame);

  // 等倍表示の配置（画像の画素の単位。面の大きさで）。
  const viewPxW = view.width * dpr;
  const viewPxH = view.height * dpr;
  const afterCenter = zoomed ? zoom : null;
  const beforeCenter =
    zoomed && bOutW > 0 && bOutH > 0
      ? mapCenter(zoom, { width: outW, height: outH }, { width: bOutW, height: bOutH })
      : null;
  const originOf = (c: { centerX: number; centerY: number } | null, w: number, h: number) =>
    c && w > 0 && h > 0
      ? { x: zoomOrigin(w, viewPxW, c.centerX), y: zoomOrigin(h, viewPxH, c.centerY), outW: w, outH: h }
      : null;

  const afterRegion = useZoomRegion(
    {
      target: "after",
      variantId,
      outW,
      outH,
      generation: header?.generation ?? null,
      center: showsAfter ? afterCenter : null,
    },
    view,
    dpr,
  );
  const beforeRegion = useZoomRegion(
    {
      target: "before",
      variantId,
      outW: bOutW,
      outH: bOutH,
      generation: bHeader?.generation ?? null,
      center: showsBefore ? beforeCenter : null,
    },
    view,
    dpr,
  );

  // マウスの操作（スポイト・等倍表示の切り替え・パン）。
  const press = useRef<{ x: number; y: number; cx: number; cy: number; moved: boolean; pointer: number } | null>(null);
  const [panning, setPanning] = useState(false);

  /**
   * クリックした点の、出力の画像の中の割合（0〜1）。画像の外なら null。全体表示の画像（data-fit の canvas）の
   * うち、点を含むもので求める（左右に並べるときはどちらの面でもよい。ビフォーとアフターの格子は同じ）。
   */
  const pointToImage = (clientX: number, clientY: number): { fx: number; fy: number } | null => {
    const canvases = rootRef.current?.querySelectorAll<HTMLCanvasElement>("canvas[data-fit]") ?? [];
    for (const el of canvases) {
      const r = el.getBoundingClientRect();
      if (!(r.width > 0) || !(r.height > 0)) continue;
      const fx = (clientX - r.left) / r.width;
      const fy = (clientY - r.top) / r.height;
      if (fx >= 0 && fx <= 1 && fy >= 0 && fy <= 1) return { fx, fy };
    }
    return null;
  };

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !frame) return;
    if (tool === "crop" || tool === "brush") return;
    if (tool === "wb") {
      const p = pointToImage(e.clientX, e.clientY);
      if (p) void pickWhiteBalance(p.fx, p.fy);
      return;
    }
    if (tool === "mask_color") {
      const p = pointToImage(e.clientX, e.clientY);
      if (p) void pickMaskColor(p.fx, p.fy);
      return;
    }
    e.currentTarget.setPointerCapture(e.pointerId);
    press.current = {
      x: e.clientX,
      y: e.clientY,
      cx: zoom?.centerX ?? 0,
      cy: zoom?.centerY ?? 0,
      moved: false,
      pointer: e.pointerId,
    };
  };
  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const p = press.current;
    if (!p || p.pointer !== e.pointerId) return;
    const dx = e.clientX - p.x;
    const dy = e.clientY - p.y;
    if (!p.moved && Math.hypot(dx, dy) < CLICK_SLOP_PX) return;
    p.moved = true;
    if (zoomed) {
      setPanning(true);
      setZoom({
        centerX: clampCenter(outW, viewPxW, p.cx - dx * dpr),
        centerY: clampCenter(outH, viewPxH, p.cy - dy * dpr),
      });
    }
  };
  const onPointerUp = (e: ReactPointerEvent<HTMLDivElement>) => {
    const p = press.current;
    if (!p || p.pointer !== e.pointerId) return;
    press.current = null;
    setPanning(false);
    if (p.moved) return;
    // クリック: 全体表示ならその点を中心に等倍表示、等倍表示なら全体表示に戻す。
    if (zoomed) {
      setZoom(null);
      return;
    }
    if (!canZoom) return;
    const pt = pointToImage(e.clientX, e.clientY);
    if (!pt) return;
    setZoom({
      centerX: clampCenter(outW, viewPxW, pt.fx * outW),
      centerY: clampCenter(outH, viewPxH, pt.fy * outH),
    });
  };

  const cropReady = !!crop && uncropped && !!fit;
  const k = crop ? cropFractionAspect(crop.aspect, info) : null;
  const corners = crop && info ? rotatedImageCorners(info.ow, info.oh, angle) : null;

  // ローカル補正（マスク）の表示: アフターだけの表示（面は 1 つでステージの左上から）で、切り抜きの編集中でないとき。
  // 画像の箱は、全体表示なら配置、等倍表示なら下敷きのプレビューを等倍に引き伸ばした箱。
  const afterOrigin = originOf(afterCenter, outW, outH);
  const imageBox: Box | null =
    frame && fit ? (afterOrigin ? { left: -afterOrigin.x / dpr, top: -afterOrigin.y / dpr, width: outW / dpr, height: outH / dpr } : fit) : null;
  const sel = !crop && !comparing && hasState ? currentMask(settings, { maskIndex, componentIndex }) : null;
  const gb = boundsOrDefault(header?.g_bounds);
  const maskEditable = !!settings && maskControls(settings, !hasState || opening).editable;
  const overlay = useMaskOverlay(
    sel && maskOverlay && variantId !== null && header
      ? { variantId, index: sel.index, generation: header.generation, uncropped }
      : null,
  );

  const classes = ["dv-stage"];
  if (tool === "wb" || tool === "mask_color") classes.push("tool-wb");
  else if (tool === "brush") classes.push("tool-brush");
  else if (zoomed) classes.push(panning ? "panning" : "zoomed");
  else if (canZoom && !crop) classes.push("can-zoom");

  let hint: string | null = null;
  if (tool === "wb") hint = "スポイト: 中立（灰色）にしたい点をクリック（Esc で取り消し）";
  else if (tool === "mask_color") hint = "色域: 選びたい色の点をクリック（Esc で取り消し）";
  else if (tool === "brush")
    hint = `ブラシ${brush.erase ? "（消しゴム）" : ""}: ドラッグで描く。Alt を押しながらで${brush.erase ? "描く" : "消す"}・[ ] で大きさ・Esc で終わる`;
  else if (crop && !cropReady) hint = "切り抜く前の全体を描いています…（Esc で取り消し）";
  else if (crop) hint = "切り抜き: 枠の四隅・四辺・中をドラッグ。Enter で完了、Esc で取り消し";
  else if (zoomed) hint = comparing ? "等倍表示（ビフォーとアフターは同じ位置）: ドラッグで移動。クリック・Z・Esc で全体表示" : "等倍表示: ドラッグで移動。クリック・Z・Esc で全体表示";

  const afterBusy = opening || !frame || (crop && !cropReady) || (zoomed && showsAfter && !afterRegion);
  const busy = afterBusy || (showsBefore && (beforeBusy || (zoomed && !beforeRegion)));
  const split = comparing && compare === "split";
  const splitAt = splitX(view.width);
  const summary = useLoadedSummaryObject(variantId);
  const count = useApp((s) => s.search?.count ?? 0);
  const currentIndex = useApp((s) => s.selection.currentIndex);
  const detailsVersion = useApp((s) => s.detailsVersion);

  return (
    <div
      ref={rootRef}
      className={classes.join(" ")}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
    >
      {panes.map((pane) =>
        pane.target === "after" ? (
          <PaneImage
            key="after"
            pane={pane}
            frame={frame}
            region={afterRegion}
            fit={fit}
            origin={afterOrigin}
            dpr={dpr}
            showClipping={showClipping && !crop}
            clipLeft={split ? splitAt : null}
          />
        ) : (
          <PaneImage
            key="before"
            pane={pane}
            frame={bFrame}
            region={beforeRegion}
            fit={bFit}
            origin={originOf(beforeCenter, bOutW, bOutH)}
            dpr={dpr}
            showClipping={false}
            clipRight={split ? splitAt : null}
          />
        ),
      )}
      {cropReady && fit && <CropOverlay box={fit} crop={crop.crop} aspect={k} imageCorners={corners} />}
      {sel && overlay && imageBox && <MaskOverlayCanvas frame={overlay} style={imageBox} />}
      {sel && imageBox && (
        <MaskLayer
          width={size.width}
          height={size.height}
          box={imageBox}
          gb={gb}
          mask={sel.mask}
          index={sel.index}
          component={sel.component}
          painting={tool === "brush" && maskEditable}
          brush={brush}
          editable={maskEditable}
        />
      )}
      {comparing && split && <div className="dv-split-line" style={{ left: splitAt }} />}
      {comparing &&
        (split ? (
          <>
            <div className="dv-pane-label" style={{ left: splitAt / 2 }}>
              {paneLabel("before", before?.label ?? null)}
            </div>
            <div className="dv-pane-label" style={{ left: splitAt + (view.width - splitAt) / 2 }}>
              {paneLabel("after", null)}
            </div>
          </>
        ) : (
          panes.map((p) => (
            <div key={p.target} className="dv-pane-label" style={{ left: p.left + p.width / 2 }}>
              {paneLabel(p.target, before?.label ?? null)}
            </div>
          ))
        ))}
      {summary && !crop && (
        <InfoOverlay
          summary={summary}
          index={currentIndex}
          count={count}
          version={detailsVersion}
          dimensions={outW > 0 && outH > 0 && !uncropped ? { width: outW, height: outH } : null}
        />
      )}
      {busy && (
        <div className="dv-busy" aria-label="描画中">
          <div className="spinner" />
        </div>
      )}
      {hint && <div className="dv-hint">{hint}</div>}
    </div>
  );
}

/** 描画・操作の失敗の知らせ（写真は開いたまま）。 */
function OperationError() {
  const error = useDevelop((s) => (s.state ? s.error : null));
  if (!error) return null;
  return (
    <div className="dv-banner" role="alert">
      <div>
        <div>{error.message}</div>
        {error.hint && <div className="hint">{error.hint}</div>}
      </div>
      <button className="btn ghost small" onClick={() => developStore.set({ error: null })} aria-label="閉じる">
        ✕
      </button>
    </div>
  );
}

/**
 * 白黒とカラーを切り替える（V キー。DEV-25）。写真を開いていなければ何もしない。処理バージョン 1 の写真では使われない
 * ので、変えずに知らせる。
 */
function toggleBlackWhiteKey(): void {
  const d = developStore.get();
  if (!d.state || !d.settings || d.opening) return;
  if (colorControls(d.settings, false).needsUpgrade) {
    actions.notify(
      "info",
      `この写真の処理バージョン（${d.settings.process_version}）では、白黒は使われません（処理バージョンを更新すると使えます）`,
    );
    return;
  }
  applySettings(toggleBlackWhite(d.settings), false);
}

/** ブラシの大きさを範囲に収める（G 座標の半径。RANGES.brushSize）。 */
function brushSize(v: number): number {
  return Math.round(Math.min(RANGES.brushSize.max, Math.max(RANGES.brushSize.min, v)) * 10000) / 10000;
}

/** 現像の画面のキー（W・R・Z・J・I・V・O・[・]・\・Y・Shift ＋ Y と、道具を使っている間の Esc・Enter）。 */
function useDevelopKeys(enabled: boolean) {
  const tool = useDevelopView((s) => s.tool);
  const zoomed = useDevelopView((s) => s.zoom !== null);
  useHotkeys(
    {
      w: () => toggleWbTool(),
      r: () => {
        if (viewStore.get().crop) void commitCrop();
        else void startCrop();
      },
      z: () => {
        const s = viewStore.get();
        if (s.crop) return;
        if (s.zoom) {
          setZoom(null);
          return;
        }
        const h = developStore.get().frame?.header;
        const w = h?.output_width ?? 0;
        const hh = h?.output_height ?? 0;
        if (!h || h.uncropped || !(w > 0) || !(hh > 0)) {
          actions.notify("info", "等倍表示は使えません（出力の画像の大きさがコアから届いていません）");
          return;
        }
        setZoom({ centerX: w / 2, centerY: hh / 2 });
      },
      j: () => toggleClipping(),
      i: () => {
        cycleInfoMode();
      },
      v: () => toggleBlackWhiteKey(),
      o: () => toggleMaskOverlay(),
      "[": () => {
        if (viewStore.get().tool === "brush") setBrushOptions({ size: brushSize(viewStore.get().brush.size / BRUSH_SIZE_STEP) });
      },
      "]": () => {
        if (viewStore.get().tool === "brush") setBrushOptions({ size: brushSize(viewStore.get().brush.size * BRUSH_SIZE_STEP) });
      },
      "\\": () => toggleCompareMode("before"),
      // macOS の日本語配列の ¥ のキー（Option なしでは "¥" になる）。
      "¥": () => toggleCompareMode("before"),
      y: () => toggleCompareMode("side_by_side"),
      "shift+y": () => toggleCompareMode("split"),
    },
    enabled,
  );
  // Esc・Enter は App の Escape（グリッドに戻る）より先に処理する（window の capture で受けて止める）。
  useEffect(() => {
    if (!enabled || (tool === "none" && !zoomed)) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing || e.keyCode === 229) return;
      if (appStore.get().dialog !== null || isTyping(e.target)) return;
      const s = viewStore.get();
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        if (s.crop) void cancelCrop();
        else if (s.tool === "wb") cancelWbTool();
        else if (s.tool === "brush" || s.tool === "mask_color") cancelMaskTool();
        else if (s.zoom) setZoom(null);
      } else if (e.key === "Enter" && s.crop && !activatesOnEnter(e.target)) {
        // ボタンなどにフォーカスがあるときは、その要素の動作に任せる（横取りして保存しない）。
        e.preventDefault();
        e.stopPropagation();
        void commitCrop();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [enabled, tool, zoomed]);
}

export function DevelopView() {
  const current = useApp((s) => s.selection.current);
  const dialogOpen = useApp((s) => s.dialog !== null);
  const isVideo = useLoadedSummary(current, (s) => s.kind) === "video";
  const variantId = useDevelop((s) => s.variantId);
  const hasState = useDevelop((s) => s.state !== null);
  const opening = useDevelop((s) => s.opening);
  const openError = useDevelop((s) => (s.state ? null : s.error));

  // 写真が変わったら開く（道具は終える。切り抜きの編集は保存しない）。
  useEffect(() => {
    endAllTools();
    if (current !== null && !isVideo) void openDevelopFor(current);
  }, [current, isVideo]);
  // 画面を離れたら道具を終える（切り抜く前の全体の表示を戻す・等倍表示の描画を取り消す）。
  useEffect(() => () => endAllTools(), []);

  useDevelopKeys(!dialogOpen && hasState && !isVideo && variantId === current);
  useBeforeForCompare(hasState && variantId === current && !isVideo);

  if (current === null) {
    return (
      <div className="empty-state">
        <h2>写真がありません</h2>
        <p>グリッドで写真を選んでから現像を開いてください。</p>
      </div>
    );
  }
  if (isVideo) {
    return (
      <div className="empty-state">
        <h2>動画は現像できません</h2>
        <p>前後の写真へは ← / → で移れます。</p>
      </div>
    );
  }
  if (openError && !opening && variantId === current) {
    return (
      <div className="empty-state">
        <h2>写真を開けません</h2>
        <p className="error-text">{openError.message}</p>
        {openError.hint && <p className="hint">{openError.hint}</p>}
        <button className="btn" onClick={() => void openDevelopFor(current)}>
          もう一度開く
        </button>
      </div>
    );
  }
  return (
    <div className="dv-root">
      <OperationError />
      <Stage />
      <CompareBar />
    </div>
  );
}

/**
 * ビフォー / アフターの表示の間は、開いている写真のビフォーを設定する（写真を開き直すとコアのビフォーはやめた
 * 状態に戻るので、写真ごとに設定し直す。既定は取り込んだ直後の設定）。表示をやめたらビフォーもやめる（描き直しと
 * 画素の分のメモリを使わない）。
 */
function useBeforeForCompare(ready: boolean): void {
  const compare = useDevelopView((s) => s.compare);
  const variantId = useDevelop((s) => s.variantId);
  const hasBefore = useDevelop((s) => s.before !== null);
  useEffect(() => {
    if (!ready) return;
    if (compare === "off") {
      if (hasBefore) void setBeforeSource(null).catch(() => undefined);
      return;
    }
    if (!hasBefore) {
      // 「ビフォーに設定」（スナップショット・履歴。DEV-28）で表示を始めたときは、選んだ出どころにする。
      void setBeforeSource(takeWantedBefore() ?? { kind: "initial" }).catch((e) => {
        actions.reportError(e, "ビフォーを描けません");
        setCompareMode("off");
      });
    }
  }, [ready, compare, variantId, hasBefore]);
}

/** ビフォーの出どころを変える（ツールバーのボタン）。 */
function chooseBefore(source: BeforeSource): void {
  void setBeforeSource(source).catch((e) => actions.reportError(e, "ビフォーを変えられません"));
}

/**
 * 中央の下のツールバー: ビフォー / アフターの表示の形（アフター・ビフォー・左右・分割）と、ビフォーの出どころ
 * （取り込んだ直後に戻す・アフターの設定をビフォーにコピー）、撮影情報の表示（I）。
 */
function CompareBar() {
  const compare = useDevelopView((s) => s.compare);
  const cropping = useDevelopView((s) => s.crop !== null);
  const before = useDevelop((s) => s.before);
  const hasState = useDevelop((s) => s.state !== null);
  const infoMode = useInfo((s) => s.mode);
  return (
    <div className="dv-bar" role="toolbar" aria-label="ビフォー / アフター">
      <span className="dv-bar-label">表示</span>
      <span className="dv-seg">
        {COMPARE_MODES.map((m) => (
          <button
            key={m.mode}
            type="button"
            className={`btn small${compare === m.mode ? " active" : ""}`}
            aria-pressed={compare === m.mode}
            disabled={!hasState || cropping}
            title={m.key ? `${m.label}（${comboLabel(m.key)}。もう一度押すとアフターに戻る）` : "アフターだけ"}
            onClick={() => setCompareMode(m.mode)}
          >
            {m.label}
          </button>
        ))}
      </span>
      {compare !== "off" && (
        <>
          <span className="dv-bar-label">ビフォー: {before?.label ?? "…"}</span>
          <button
            type="button"
            className="btn small"
            disabled={!before}
            title="ビフォーを、今のアフターの設定にする（この後の調整と比べる）"
            onClick={() => chooseBefore({ kind: "current" })}
          >
            アフターの設定をビフォーに
          </button>
          <button
            type="button"
            className="btn small"
            disabled={!before || before.source.kind === "initial"}
            title="ビフォーを、取り込んだ直後（仮想コピーは作ったとき）の設定に戻す"
            onClick={() => chooseBefore({ kind: "initial" })}
          >
            取り込み直後に戻す
          </button>
        </>
      )}
      <span className="dv-bar-grow" />
      <button
        type="button"
        className="btn small"
        title={`${infoModeLabel(infoMode)}（${comboLabel("i")} で切り替え。項目は設定の「表示」）`}
        onClick={() => cycleInfoMode()}
      >
        {infoModeLabel(infoMode)}
      </button>
    </div>
  );
}
