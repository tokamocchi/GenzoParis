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
import type { CropRect, FrameInfo, RegionInfo } from "../api/types";
import { isTyping, useHotkeys } from "../keyboard";
import { actions, appStore, useApp } from "../state/app";
import { developStore, openDevelopFor, useDevelop } from "../state/develop";
import { regionFor, renderRegion } from "../state/region";
import { useDevicePixelRatio } from "../viewer/hooks";
import { expandRegion, limitRegion, MAX_REGION_PIXELS, type Rect } from "../viewer/viewerMath";
import { type CropHandle, cropFractionAspect, dragCrop, rotatedImageCorners } from "./cropMath";
import { useGeometryInfo, useLoadedSummary } from "./hooks";
import { activatesOnEnter } from "./keyTargets";
import { clippingOverlay } from "./pixels";
import {
  cancelCrop,
  cancelWbTool,
  commitCrop,
  constrainDraft,
  endAllTools,
  pickWhiteBalance,
  setDraftCrop,
  setZoom,
  startCrop,
  toggleClipping,
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

/** 画素を描く canvas（frame が変わったら描き直す）。 */
function FrameCanvas(props: { frame: Frame<unknown> | null; className?: string; style?: CSSProperties }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    if (ref.current && props.frame) drawFrame(ref.current, props.frame);
  }, [props.frame]);
  return <canvas ref={ref} className={props.className} style={props.style} />;
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

function Stage() {
  const rootRef = useRef<HTMLDivElement>(null);
  const fitCanvasRef = useRef<HTMLDivElement>(null);
  const size = useElementSize(rootRef);
  // モニター間の移動・表示倍率の変更でも読み直す（ルーペと同じ。viewer/hooks.ts）。
  const dpr = useDevicePixelRatio();
  const variantId = useDevelop((s) => s.variantId);
  const frame = useDevelop((s) => s.frame);
  const opening = useDevelop((s) => s.opening);
  const tool = useDevelopView((s) => s.tool);
  const zoom = useDevelopView((s) => s.zoom);
  const crop = useDevelopView((s) => s.crop);
  const showClipping = useDevelopView((s) => s.showClipping);
  const angle = useDevelop((s) => s.settings?.geometry.angle_deg ?? 0);
  const info = useGeometryInfo();
  const [region, setRegion] = useState<Frame<RegionInfo> | null>(null);

  const header: FrameInfo | null = frame?.header ?? null;
  const outW = header?.output_width ?? 0;
  const outH = header?.output_height ?? 0;
  const uncropped = !!header?.uncropped;
  const canZoom = outW > 0 && outH > 0 && !uncropped && !crop;
  const zoomed = !!zoom && canZoom;

  // 全体表示の配置（HiDPI: 画像の 1 画素を画面の 1 物理画素より大きくしない）。切り抜きの編集中は
  // まわりを空ける（画像が枠いっぱいだと、端のつまみが表示の外にかかってつかめないため）。
  const inset = crop ? CROP_INSET_PX : 0;
  const fitInner = frame
    ? fitBox(size.width - 2 * inset, size.height - 2 * inset, frame.width / dpr, frame.height / dpr)
    : null;
  const fit = fitInner ? { ...fitInner, left: fitInner.left + inset, top: fitInner.top + inset } : null;

  // 等倍表示の配置（画像の画素の単位）。
  const viewPxW = size.width * dpr;
  const viewPxH = size.height * dpr;
  const originX = zoomed ? zoomOrigin(outW, viewPxW, zoom.centerX) : 0;
  const originY = zoomed ? zoomOrigin(outH, viewPxH, zoom.centerY) : 0;

  // 等倍表示の範囲を描く（設定が変わったらプレビューの更新の後に少し待って、パン・大きさの変更でも描き直す）。
  const generation = header?.generation ?? null;
  const lastGen = useRef<number | null>(null);
  const zoomActive = useRef(false);
  zoomActive.current = zoomed;
  useEffect(() => {
    if (!zoomed || variantId === null || generation === null || !(size.width > 0) || !(size.height > 0)) {
      if (!zoomed) {
        setRegion(null);
        lastGen.current = null;
      }
      return;
    }
    const delay =
      lastGen.current !== null && lastGen.current !== generation ? REGION_REDRAW_DELAY_MS : REGION_PAN_DELAY_MS;
    lastGen.current = generation;
    const req = zoomRegionRequest({
      fullWidth: outW,
      fullHeight: outH,
      centerX: zoom.centerX,
      centerY: zoom.centerY,
      scale: 1,
      viewWidth: size.width,
      viewHeight: size.height,
      devicePixelRatio: dpr,
    });
    const timer = setTimeout(() => {
      renderRegion({ variant_id: variantId, space: "output", ...req, scale: 1 })
        .then((f) => {
          if (f && zoomActive.current && f.header.variant_id === developStore.get().variantId) setRegion(f);
        })
        .catch((e) => {
          actions.reportError(e, "等倍表示を描けません");
          setZoom(null);
        });
    }, delay);
    return () => clearTimeout(timer);
    // zoom のオブジェクトではなく中心の値で描き直す。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [zoomed, zoom?.centerX, zoom?.centerY, size.width, size.height, dpr, outW, outH, generation, variantId]);

  // マウスの操作（スポイト・等倍表示の切り替え・パン）。
  const press = useRef<{ x: number; y: number; cx: number; cy: number; moved: boolean; pointer: number } | null>(null);
  const [panning, setPanning] = useState(false);

  /** クリックした点の、出力の画像の中の割合（0〜1）。画像の外なら null。 */
  const pointToImage = (clientX: number, clientY: number): { fx: number; fy: number } | null => {
    const el = fitCanvasRef.current?.querySelector("canvas");
    if (!el) return null;
    const r = el.getBoundingClientRect();
    if (!(r.width > 0) || !(r.height > 0)) return null;
    const fx = (clientX - r.left) / r.width;
    const fy = (clientY - r.top) / r.height;
    if (fx < 0 || fx > 1 || fy < 0 || fy > 1) return null;
    return { fx, fy };
  };

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !frame) return;
    if (tool === "crop") return;
    if (tool === "wb") {
      const p = pointToImage(e.clientX, e.clientY);
      if (p) void pickWhiteBalance(p.fx, p.fy);
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

  // 表示する画素と配置。
  let fitStyle: CSSProperties | null = null;
  if (frame && fit) {
    fitStyle = zoomed
      ? // 等倍表示の下敷き: プレビューを等倍の大きさに引き伸ばす（範囲が届くまでの間に見せる）。
        { left: -originX / dpr, top: -originY / dpr, width: outW / dpr, height: outH / dpr }
      : { left: fit.left, top: fit.top, width: fit.width, height: fit.height };
  }
  const regionShown = zoomed && region && region.header.variant_id === variantId ? region : null;
  const regionStyle: CSSProperties | null = regionShown
    ? {
        left: (regionShown.header.x - originX) / dpr,
        top: (regionShown.header.y - originY) / dpr,
        width: regionShown.width / dpr,
        height: regionShown.height / dpr,
      }
    : null;

  const cropReady = !!crop && uncropped && !!fit;
  const k = crop ? cropFractionAspect(crop.aspect, info) : null;
  const corners = crop && info ? rotatedImageCorners(info.ow, info.oh, angle) : null;

  const classes = ["dv-stage"];
  if (tool === "wb") classes.push("tool-wb");
  else if (zoomed) classes.push(panning ? "panning" : "zoomed");
  else if (canZoom && !crop) classes.push("can-zoom");

  let hint: string | null = null;
  if (tool === "wb") hint = "スポイト: 中立（灰色）にしたい点をクリック（Esc で取り消し）";
  else if (crop && !cropReady) hint = "切り抜く前の全体を描いています…（Esc で取り消し）";
  else if (crop) hint = "切り抜き: 枠の四隅・四辺・中をドラッグ。Enter で完了、Esc で取り消し";
  else if (zoomed) hint = "等倍表示: ドラッグで移動。クリック・Z・Esc で全体表示";

  return (
    <div
      ref={rootRef}
      className={classes.join(" ")}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
    >
      <div ref={fitCanvasRef}>
        {fitStyle && <FrameCanvas frame={frame} className="dv-layer dv-image" style={fitStyle} />}
      </div>
      {regionShown && regionStyle && (
        <FrameCanvas frame={regionShown} className="dv-layer dv-image" style={regionStyle} />
      )}
      {showClipping && !crop && regionShown && regionStyle && (
        <ClippingCanvas frame={regionShown} style={regionStyle} />
      )}
      {showClipping && !crop && !regionShown && frame && fitStyle && <ClippingCanvas frame={frame} style={fitStyle} />}
      {cropReady && fit && <CropOverlay box={fit} crop={crop.crop} aspect={k} imageCorners={corners} />}
      {(opening || !frame || (crop && !cropReady) || (zoomed && !regionShown)) && (
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

/** 現像の画面のキー（W・R・Z・J と、道具を使っている間の Esc・Enter）。 */
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
    </div>
  );
}
