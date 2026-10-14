// 比較表示（LIB-03。C）: 選択（左）と候補（右）の 2 枚を並べ、拡大（等倍）とパンを同期する。
//
// - 各面は、全体表示では標準プレビュー（L1。読めるまでは L0）を出す（previewImages.tsx。ルーペと同じ）。
// - 等倍（Z・面のクリック）: 両方の面の写真の出力の画像の大きさをコアに問い合わせ（view_output_size）、面ごとに
//   見えている範囲だけを描かせて canvas に出す（state/viewRegion.ts。コアの面ごとの「最新の 1 件だけ」。現像の
//   セッションは開かない）。描き直す間は、前の範囲と拡大した L1 を出しておく。画像の 1 画素を画面の 1 物理画素に
//   する（ルーペと同じ。UX-03）。
// - 同期（既定。compareLogic の冒頭）: クリックした点の割合を両方の面の中心にし、ドラッグの量を両方の面に同じだけ
//   足す。同期を外すと、ドラッグした面だけが動く。「位置をそろえる」で、もう片方の面を作業中の面の割合に合わせる。
// - 候補を替えても（←・→・↑・↓）等倍のまま: 新しい写真の出力の大きさを求め直し、中心は、入れ替えなら同じ写真の
//   割合、同期していればもう片方の面の割合、そうでなければ前の写真の割合。
// - 面のクリック（ドラッグでない）は、その面を作業中にする（評価などの対象）。全体表示では、さらにその点を中心に
//   等倍にする。等倍の間は、クリック・Z・Esc で全体表示に戻る（ルーペと同じ）。
// - 動画・元ファイルが見つからない・読めない写真は拡大しない（理由を知らせる。もう片方の面は拡大する）。
// - 写真の現像設定・ファイルが変わったら（検索結果の develop_hash・file_revision）、見えている範囲を描き直す。
// - 次の候補（→ で送る先）の L1 を先読みする（prefetch.ts。PERF-04）。
// - 比較表示をやめたら、コアが展開した写真と結果を捨てる（releaseView）。

import { type PointerEvent as ReactPointerEvent, useEffect, useLayoutEffect, useRef, useState } from "react";

import { type Frame, drawFrame } from "../api/frames";
import type { RegionInfo, VariantSummary } from "../api/types";
import { previewUrl } from "../api/uri";
import { FlagMark, LabelDot, Stars } from "../components/Marks";
import { comboLabel, isTyping, useHotkeys } from "../keyboard";
import { actions, appStore, useApp } from "../state/app";
import {
  makeCandidateSelect,
  setCompareActive,
  setCompareLinked,
  stepCandidate,
  swapCompare,
  useCompare,
} from "../state/compare";
import { regionFor } from "../state/region";
import { searchCache, thumbRevs } from "../state/search";
import { cancelViewRegions, releaseView, renderViewRegion, viewOutputSize } from "../state/viewRegion";
import {
  COMPARE_GAP_PX,
  type CompareSide,
  type PaneZoom,
  clampPane,
  paneAt,
  paneOf,
  pannedPanes,
  relativeCenter,
  stepIndex,
} from "./compareLogic";
import { useViewRect } from "./hooks";
import { prefetchL1 } from "./prefetch";
import { SlotImages, usePreviewImages } from "./previewImages";
import { useVariantSummary } from "./useSummary";
import {
  DRAG_THRESHOLD_PX,
  L1_SETTLE_MS,
  MAX_REGION_PIXELS,
  PAN_SETTLE_MS,
  type Rect,
  expandRegion,
  fitRect,
  limitRegion,
  normalizedPoint,
  orientedSize,
  regionUsable,
  sameRect,
  snapToDevicePixels,
  zoomOffset,
} from "./viewerMath";
import "./viewer.css";

/** 一時的な知らせ（等倍表示を使えない理由など）を出しておく時間（ミリ秒。仮置き。ルーペと同じ）。 */
const MESSAGE_MS = 5000;

const SIDES: CompareSide[] = ["select", "candidate"];

const SIDE_LABEL: Record<CompareSide, string> = { select: "選択", candidate: "候補" };

type ZoomState =
  | { kind: "fit" }
  | { kind: "preparing" }
  /** 等倍。面ごとの写真・出力の大きさ・中心（拡大できない面は null）。 */
  | { kind: "zoomed"; panes: (PaneZoom | null)[] };

/** 等倍にできない理由（できるなら null）。 */
function zoomBlocker(s: VariantSummary | null): string | null {
  if (!s) return "写真がありません";
  if (s.kind === "video") return "動画は拡大できません";
  if (s.file_status === "missing") return "元ファイルが見つからないため、等倍表示にできません";
  if (s.file_status !== "ok") return "元ファイルを読めないため、等倍表示にできません";
  return null;
}

/** 出力の大きさの覚え（写真・現像設定・ファイルの内容ごと。切り抜きを変えると大きさが変わる）。 */
function sizeKey(s: VariantSummary): string {
  return `${s.variant_id}:${s.develop_hash}:${s.file_revision}`;
}

export function CompareView() {
  const pair = useCompare((s) => s.pair);
  const active = useCompare((s) => s.active);
  const linked = useCompare((s) => s.linked);
  const hotkeysEnabled = useApp((s) => s.dialog === null && s.mode === "compare");
  const summaries = [useVariantSummary(pair.select), useVariantSummary(pair.candidate)];
  const ids = [pair.select, pair.candidate];

  const panesRef = useRef<HTMLDivElement>(null);
  const area = useViewRect(panesRef);
  const paneView = { width: Math.max(0, (area.width - COMPARE_GAP_PX) / 2), height: area.height };
  const dpr = area.dpr;

  const [zoom, setZoom] = useState<ZoomState>({ kind: "fit" });
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;
  const [dragging, setDragging] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const zoomSeq = useRef(0);
  const sizes = useRef(new Map<string, { width: number; height: number }>());
  /** 全体表示に戻したときの中心の割合（面ごと。同期していないときに拡大し直すと、その位置に戻る）。 */
  const lastRel = useRef<({ u: number; v: number } | null)[]>([null, null]);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      zoomSeq.current++;
      // 比較表示をやめた: 描画を取り消し、コアが展開した写真と結果を捨てる。
      releaseView();
    };
  }, []);

  useEffect(() => {
    if (!message) return;
    const t = setTimeout(() => setMessage(null), MESSAGE_MS);
    return () => clearTimeout(t);
  }, [message]);

  /** 面 i の写真の出力の大きさ（覚えていればそれ。取り消されたら null）。 */
  const sizeOf = async (i: number, s: VariantSummary) => {
    const key = sizeKey(s);
    const known = sizes.current.get(key);
    if (known) return known;
    const r = await viewOutputSize(i, s.variant_id);
    if (!r) return null;
    const size = { width: r.width, height: r.height };
    sizes.current.set(key, size);
    return size;
  };

  /** 面 i の等倍の面（割合 rel を中心に）。拡大できない・求められなければ null（`required` なら例外）。 */
  const paneFor = async (i: number, rel: { u: number; v: number }, required: boolean): Promise<PaneZoom | null> => {
    const s = summaries[i];
    if (!s || zoomBlocker(s) !== null) return null;
    try {
      const size = await sizeOf(i, s);
      return size ? paneAt(s.variant_id, size, rel) : null;
    } catch (e) {
      if (required) throw e;
      return null;
    }
  };

  const startZoom = (k: number, u: number, v: number) => {
    const blocker = zoomBlocker(summaries[k]);
    if (blocker) {
      setMessage(blocker);
      return;
    }
    const my = ++zoomSeq.current;
    setMessage(null);
    setZoom({ kind: "preparing" });
    void (async () => {
      try {
        const panes = await Promise.all(
          SIDES.map((_, i) =>
            paneFor(i, i === k || linked ? { u, v } : (lastRel.current[i] ?? { u: 0.5, v: 0.5 }), i === k),
          ),
        );
        if (my !== zoomSeq.current || !mounted.current) return;
        if (!panes[k]) {
          setZoom({ kind: "fit" });
          return;
        }
        setZoom({ kind: "zoomed", panes });
      } catch (e) {
        if (my !== zoomSeq.current || !mounted.current) return;
        setZoom({ kind: "fit" });
        actions.reportError(e, "等倍表示にできません");
      }
    })();
  };

  const exitZoom = () => {
    zoomSeq.current++;
    const z = zoomRef.current;
    if (z.kind === "zoomed") lastRel.current = z.panes.map((p) => (p ? relativeCenter(p) : null));
    setZoom({ kind: "fit" });
    setDragging(false);
    cancelViewRegions();
  };
  const exitZoomRef = useRef(exitZoom);
  exitZoomRef.current = exitZoom;

  // 写真が替わった（候補の送り・入れ替え・フィルムストリップのクリック）: 等倍のまま、新しい写真の面を作る。
  const zoomable = summaries.map((s) => zoomBlocker(s) === null);
  useEffect(() => {
    const z = zoomRef.current;
    if (z.kind !== "zoomed") return;
    const cur = z.panes;
    const want = ids.map((id, i) => (zoomable[i] ? id : null));
    if (want.every((id, i) => (cur[i]?.id ?? null) === id)) return;
    const my = ++zoomSeq.current;
    void (async () => {
      const next = await Promise.all(
        SIDES.map((_, i) => {
          const id = want[i];
          if (id === null) return Promise.resolve(null);
          const same = cur[i];
          if (same && same.id === id) return Promise.resolve(same);
          // 入れ替え: 同じ写真の割合。同期: もう片方の面（替わっていない面）の割合。そうでなければ前の写真の割合。
          const moved = cur.find((p) => p?.id === id);
          const anchor = linked ? cur.find((p, j) => j !== i && p && p.id === want[j]) : undefined;
          const base = moved ?? anchor ?? same;
          return paneFor(i, base ? relativeCenter(base) : { u: 0.5, v: 0.5 }, false);
        }),
      );
      if (my !== zoomSeq.current || !mounted.current) return;
      if (next.every((p) => p === null)) {
        setZoom({ kind: "fit" });
        return;
      }
      setZoom({ kind: "zoomed", panes: next });
    })();
    // 写真と拡大できるかが変わったときだけ。
  }, [pair.select, pair.candidate, zoomable[0], zoomable[1], zoom.kind]);

  // 次の候補の L1 を先読みする（→ で送ったときにすぐ出す。PERF-04。候補を替えてから L1_SETTLE_MS 待つ）。
  useEffect(() => {
    const candidate = pair.candidate;
    if (candidate === null) return;
    const timer = setTimeout(() => {
      const at = searchCache.findLoaded(candidate);
      const sel = pair.select === null ? -1 : searchCache.findLoaded(pair.select);
      const next = at < 0 ? null : stepIndex(at, 1, searchCache.count, sel >= 0 ? sel : null);
      const v = next === null ? undefined : searchCache.get(next);
      if (!v || v.kind !== "photo") return;
      const rev = thumbRevs.of(v);
      prefetchL1(v.variant_id, rev, previewUrl(v.variant_id, rev));
    }, L1_SETTLE_MS);
    return () => clearTimeout(timer);
  }, [pair.select, pair.candidate]);

  // --- パン ---
  const dragStart = useRef<(PaneZoom | null)[] | null>(null);
  const onPanStart = () => {
    const z = zoomRef.current;
    if (z.kind !== "zoomed") return;
    dragStart.current = z.panes.map((p) => (p ? clampPane(p, paneView, dpr) : null));
    setDragging(true);
  };
  const onPan = (k: number, dx: number, dy: number) => {
    const start = dragStart.current;
    if (!start) return;
    setZoom((z) =>
      z.kind === "zoomed" ? { kind: "zoomed", panes: pannedPanes(start, k, linked, { dx, dy }, paneView, dpr) } : z,
    );
  };
  const onPanEnd = () => {
    dragStart.current = null;
    setDragging(false);
  };

  /** もう片方の面を、作業中の面の中心の割合に合わせる（位置をそろえる）。 */
  const align = () =>
    setZoom((z) => {
      if (z.kind !== "zoomed") return z;
      const ai = paneOf(active);
      const a = z.panes[ai];
      if (!a) return z;
      const rel = relativeCenter(clampPane(a, paneView, dpr));
      return {
        kind: "zoomed",
        panes: z.panes.map((p, i) => (p && i !== ai ? paneAt(p.id, { width: p.fullW, height: p.fullH }, rel) : p)),
      };
    });

  /** 描いた結果の出力の大きさが違った（現像設定が変わった）: 中心の割合を保って合わせる。 */
  const onFullSize = (i: number, fw: number, fh: number) =>
    setZoom((z) => {
      if (z.kind !== "zoomed") return z;
      const p = z.panes[i];
      if (!p || (p.fullW === fw && p.fullH === fh)) return z;
      const panes = [...z.panes];
      panes[i] = paneAt(p.id, { width: fw, height: fh }, relativeCenter(p));
      return { kind: "zoomed", panes };
    });

  // --- キーボード ---
  const hover = useRef<{ pane: number; u: number; v: number } | null>(null);
  const toggleZoom = () => {
    if (zoomRef.current.kind === "fit") {
      const h = hover.current;
      startZoom(h?.pane ?? paneOf(active), h?.u ?? 0.5, h?.v ?? 0.5);
    } else exitZoom();
  };
  useHotkeys(
    {
      ArrowLeft: () => void stepCandidate(-1),
      ArrowRight: () => void stepCandidate(1),
      ArrowUp: () => void makeCandidateSelect(),
      ArrowDown: () => void swapCompare(),
      z: toggleZoom,
    },
    hotkeysEnabled,
  );
  // Esc: 等倍表示の間は全体表示に戻す（App.tsx のグリッドに戻す動作より先に、capture で処理して止める）。
  useEffect(() => {
    if (zoom.kind === "fit") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.isComposing || appStore.get().dialog !== null || isTyping(e.target)) return;
      e.preventDefault();
      e.stopPropagation();
      exitZoomRef.current();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [zoom.kind]);

  const finish = () => {
    const select = pair.select;
    if (select === null) {
      actions.setMode("grid");
      return;
    }
    void actions.selectIds([select], select).then(() => actions.setMode("loupe"));
  };

  const zoomed = zoom.kind === "zoomed";
  return (
    <div className="compare">
      <div ref={panesRef} className="compare-panes">
        {SIDES.map((side, i) => (
          <ComparePane
            key={side}
            side={side}
            pane={i}
            summary={summaries[i]}
            active={active === side}
            zoom={zoom.kind === "zoomed" ? (zoom.panes[i] ?? null) : null}
            preparing={zoom.kind === "preparing"}
            dragging={dragging}
            onClick={(point) => {
              void setCompareActive(side);
              if (zoomRef.current.kind === "fit") {
                if (point) startZoom(i, point.u, point.v);
              } else if (zoomRef.current.kind === "zoomed") exitZoom();
            }}
            onHover={(point) => {
              if (point) hover.current = { pane: i, ...point };
              else if (hover.current?.pane === i) hover.current = null;
            }}
            onPanStart={onPanStart}
            onPan={(dx, dy) => onPan(i, dx, dy)}
            onPanEnd={onPanEnd}
            onFullSize={(fw, fh) => onFullSize(i, fw, fh)}
          />
        ))}
      </div>
      <div className="compare-bar">
        <label className="compare-check" title="拡大とパンを 2 つの面で同期する">
          <input type="checkbox" checked={linked} onChange={(e) => setCompareLinked(e.target.checked)} />
          ズーム・パンを同期
        </label>
        <button className="btn small" disabled={!zoomed} onClick={align} title="もう片方の面を、作業中の面と同じ位置にする">
          位置をそろえる
        </button>
        <span className="compare-sep" />
        <button className="btn small" onClick={() => void stepCandidate(-1)} title={`前の候補（${comboLabel("ArrowLeft")}）`}>
          ← 前の候補
        </button>
        <button className="btn small" onClick={() => void stepCandidate(1)} title={`次の候補（${comboLabel("ArrowRight")}）`}>
          次の候補 →
        </button>
        <button
          className="btn small"
          disabled={pair.candidate === null}
          onClick={() => void swapCompare()}
          title={`選択と候補を入れ替える（${comboLabel("ArrowDown")}）`}
        >
          入れ替え
        </button>
        <button
          className="btn small"
          disabled={pair.candidate === null}
          onClick={() => void makeCandidateSelect()}
          title={`候補を選択にして、次の写真を候補にする（${comboLabel("ArrowUp")}）`}
        >
          候補を選択に
        </button>
        <span className="loupe-video-spacer" />
        {(message || zoom.kind === "preparing") && (
          <span className="compare-message" role="status">
            {message ?? "等倍表示を準備しています…"}
          </span>
        )}
        <button className="btn small" onClick={toggleZoom} title={`等倍表示の切り替え（${comboLabel("z")}）`}>
          {zoom.kind === "fit" ? "等倍" : "全体"}
        </button>
        <button className="btn small primary" onClick={finish} title="選択の写真をルーペで表示する">
          完了
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// 面
// ---------------------------------------------------------------------------

interface PaneProps {
  side: CompareSide;
  pane: number;
  summary: VariantSummary | null;
  active: boolean;
  /** 等倍の面（全体表示・拡大できない面は null）。 */
  zoom: PaneZoom | null;
  preparing: boolean;
  dragging: boolean;
  /** クリック（ドラッグでない）。全体表示で写真の上なら、その点の割合。 */
  onClick: (point: { u: number; v: number } | null) => void;
  onHover: (point: { u: number; v: number } | null) => void;
  onPanStart: () => void;
  onPan: (dx: number, dy: number) => void;
  onPanEnd: () => void;
  onFullSize: (fw: number, fh: number) => void;
}

function ComparePane(props: PaneProps) {
  const { summary, side } = props;
  return (
    <div className={`compare-pane${props.active ? " active" : ""}`} aria-label={SIDE_LABEL[side]}>
      {summary ? (
        <PanePhoto key={summary.variant_id} {...props} summary={summary} />
      ) : (
        <div className="loupe-center-note">
          <p>{side === "candidate" ? "候補の写真がありません" : "写真がありません"}</p>
          {side === "candidate" && <p className="hint">← → で候補を選ぶか、フィルムストリップで Ctrl / ⌘ + クリックしてください</p>}
        </div>
      )}
      <div className="compare-label">
        <span className="compare-side">{SIDE_LABEL[side]}</span>
        {summary && (
          <>
            <span className="compare-name" title={summary.file_name}>
              {summary.file_name}
              {!summary.is_master && (summary.variant_name ? `（${summary.variant_name}）` : "（仮想コピー）")}
            </span>
            <Stars rating={summary.rating} size={11} />
            <FlagMark flag={summary.flag} />
            <LabelDot label={summary.color_label} />
          </>
        )}
      </div>
    </div>
  );
}

function PanePhoto(props: PaneProps & { summary: VariantSummary }) {
  const { summary, zoom } = props;
  const id = summary.variant_id;
  useApp((s) => s.thumbRevsVersion);
  const thumbRev = thumbRevs.of(summary);
  const viewRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const view = useViewRect(viewRef);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const video = summary.kind === "video";
  const { l0, l1, onL1Error } = usePreviewImages({ summary, thumbRev, waitMs: () => 0, wantL1: !video });
  const shown = l1.shown ?? l0.shown;
  const natural = shown
    ? { width: shown.width, height: shown.height }
    : (orientedSize(summary.width, summary.height, summary.orientation) ?? { width: 3, height: 2 });
  const box = fitRect(natural.width, natural.height, view.width, view.height);

  // --- 等倍の範囲 ---
  const zoomed = zoom && zoom.id === id ? zoom : null;
  const center = zoomed ? clampPane(zoomed, view, view.dpr) : null;
  const [region, setRegion] = useState<{ frame: Frame<RegionInfo>; key: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const lastRequest = useRef<{ rect: Rect; key: string } | null>(null);
  const firstRegion = useRef(true);
  const regionSeq = useRef(0);
  // 描き直しの鍵（現像設定・ファイルの内容が変わったら描き直す）。
  const renderKey = `${summary.develop_hash}:${summary.file_revision}`;
  useEffect(() => {
    if (zoomed) return;
    // 全体表示に戻った: 次に等倍にしたときは、すぐ描く。
    regionSeq.current++;
    firstRegion.current = true;
    lastRequest.current = null;
    setBusy(false);
    setFailed(false);
  }, [zoomed === null]);

  const { onFullSize } = props;
  useEffect(() => {
    if (!zoomed || !center || props.dragging || failed || view.width <= 0 || view.height <= 0) return;
    const visible = regionFor({
      fullWidth: zoomed.fullW,
      fullHeight: zoomed.fullH,
      centerX: center.cx,
      centerY: center.cy,
      scale: 1,
      viewWidth: view.width,
      viewHeight: view.height,
      devicePixelRatio: view.dpr,
    });
    const need = limitRegion(expandRegion(visible, 1, zoomed.fullW, zoomed.fullH), MAX_REGION_PIXELS);
    if (region && region.key === renderKey && regionUsable(region.frame.header, need, zoomed.fullW, zoomed.fullH, null)) {
      return;
    }
    const last = lastRequest.current;
    if (last && last.key === renderKey && sameRect(last.rect, need)) return;
    const timer = setTimeout(
      () => {
        firstRegion.current = false;
        lastRequest.current = { rect: need, key: renderKey };
        const my = ++regionSeq.current;
        const key = renderKey;
        setBusy(true);
        renderViewRegion(props.pane, { variant_id: id, space: "output", ...need, scale: 1 })
          .then((f) => {
            if (my !== regionSeq.current || !mounted.current) return;
            setBusy(false);
            if (!f || f.header.variant_id !== id) return;
            setRegion({ frame: f, key });
            onFullSize(f.header.full_width, f.header.full_height);
          })
          .catch((e) => {
            if (my !== regionSeq.current || !mounted.current) return;
            setBusy(false);
            setFailed(true);
            actions.reportError(e, "等倍表示を描けません");
          });
      },
      firstRegion.current ? 0 : PAN_SETTLE_MS,
    );
    return () => clearTimeout(timer);
    // center は zoom と view から求めた値なので、x・y の数で比べる。
  }, [
    id,
    zoomed?.fullW,
    zoomed?.fullH,
    center?.cx,
    center?.cy,
    props.dragging,
    failed,
    view.width,
    view.height,
    view.dpr,
    region,
    renderKey,
  ]);

  // 描いた範囲を canvas に出す。
  useLayoutEffect(() => {
    const c = canvasRef.current;
    if (c && region) drawFrame(c, region.frame);
  }, [region, zoomed !== null]);

  // --- マウス（クリックで作業中・等倍、ドラッグでパン） ---
  const drag = useRef<{ x0: number; y0: number; moved: boolean } | null>(null);
  const localPoint = (e: ReactPointerEvent) => {
    const r = e.currentTarget.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    drag.current = { x0: e.clientX, y0: e.clientY, moved: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!zoomed) {
      const p = localPoint(e);
      const n = normalizedPoint(p.x, p.y, box);
      props.onHover(n.inside ? { u: n.u, v: n.v } : null);
    }
    const d = drag.current;
    if (!d || !zoomed) return;
    const dx = e.clientX - d.x0;
    const dy = e.clientY - d.y0;
    if (!d.moved) {
      if (Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) return;
      d.moved = true;
      props.onPanStart();
    }
    props.onPan(dx, dy);
  };
  const onPointerUp = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (d.moved) {
      props.onPanEnd();
      return;
    }
    if (Math.hypot(e.clientX - d.x0, e.clientY - d.y0) >= DRAG_THRESHOLD_PX) return;
    const p = localPoint(e);
    const n = normalizedPoint(p.x, p.y, box);
    props.onClick(!zoomed && n.inside ? { u: n.u, v: n.v } : null);
  };
  const onPointerCancel = () => {
    if (drag.current?.moved) props.onPanEnd();
    drag.current = null;
  };

  // --- 表示 ---
  const dpr = view.dpr;
  const layer =
    zoomed && center
      ? (() => {
          const raw = zoomOffset({ x: center.cx, y: center.cy }, view, dpr);
          return { x: snapToDevicePixels(raw.x, view.left, dpr), y: snapToDevicePixels(raw.y, view.top, dpr) };
        })()
      : null;
  const rh = region && region.frame.header.variant_id === id ? region.frame.header : null;
  const background = l1.shown?.url ?? l0.shown?.url ?? null;
  const nothing = !l0.shown && !l1.shown;
  const viewClass = [
    "loupe-view",
    video ? "video" : zoomed ? "zoomed" : props.preparing ? "preparing" : "fit",
    props.dragging && zoomed ? "dragging" : "",
  ].join(" ");

  return (
    <div
      ref={viewRef}
      className={viewClass}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
      onPointerLeave={() => props.onHover(null)}
    >
      <div
        className="loupe-photo"
        style={{ left: box.x, top: box.y, width: box.width, height: box.height, display: zoomed ? "none" : undefined }}
      >
        {!l1.shown && <SlotImages slot={l0} />}
        <SlotImages slot={l1} onError={onL1Error} />
        {video && (
          <span className="loupe-video-mark" aria-hidden="true">
            ▶
          </span>
        )}
      </div>
      {zoomed && layer && (
        <div
          className="loupe-zoom"
          style={{
            width: zoomed.fullW / dpr,
            height: zoomed.fullH / dpr,
            transform: `translate(${layer.x}px, ${layer.y}px)`,
          }}
        >
          {background && <img className="loupe-img" src={background} alt="" draggable={false} />}
          <canvas
            ref={canvasRef}
            className="loupe-region"
            style={rh ? { left: rh.x / dpr, top: rh.y / dpr, width: rh.width / dpr, height: rh.height / dpr } : { display: "none" }}
          />
        </div>
      )}
      {nothing && !zoomed && (
        <div className="loupe-center-note">
          {(l0.url === null || l0.failed) && l1.url === null ? <p>プレビューがありません</p> : <div className="spinner" />}
        </div>
      )}
      {zoomed && busy && (
        <div className="loupe-status" role="status">
          <span>{rh ? "描き直しています…" : "等倍で描いています…"}</span>
        </div>
      )}
      {summary.file_status !== "ok" && (
        <div className="compare-file-note" role="status">
          {summary.file_status === "missing" ? "元ファイルが見つかりません" : "元ファイルを読めません"}
        </div>
      )}
    </div>
  );
}
