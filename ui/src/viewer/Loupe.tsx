// ルーペ（1 枚表示と等倍表示。LIB-02・PRV-03・PERF-04/05・VID-02/03）。
//
// # 写真
//
// - 標準プレビュー（L1。長辺 2560）を画面に合わせて中央に出す。L1 が読めるまでは、サムネイル（L0）を同じ
//   大きさで出す（PERF-05: 先に埋め込み JPEG、後で現像結果）。L1 の作成待ち・読み直しは previewImages.tsx
//   （比較表示・選別表示と共通）。
// - 写真送りの途中（前の写真から L1_SETTLE_MS 未満で移った）は、L1 の要求と先読みを L1_SETTLE_MS 待つ
//   （通り過ぎる写真ごとに作る処理を予約しない）。
// - 前後の写真の L1 を先に読んでおく（PERF-04。prefetch.ts）。表示中の写真の L1 が読めた（または作成待ちに
//   なった）後で読む（表示中の写真の作る処理を先に予約する）。
// - 等倍表示（Z・クリックした点を中心に）: 現像のセッション（state/develop.ts。現像の画面と同じ）で写真を
//   開き、出力の画像のフル解像度の大きさを求めてから（outputSize.ts）、見えている範囲だけをコアに描かせて
//   canvas に出す（state/region.ts）。画像の 1 画素を画面の 1 物理画素にする（canvas の CSS の大きさは
//   画素 / devicePixelRatio。位置も物理画素の境界に合わせる。UX-03）。
// - ドラッグでパンし、離してから PAN_SETTLE_MS 後に見えている範囲を描き直す。描き直す間は、前の範囲と
//   拡大した L1 を出しておく。
// - Z・Esc・クリック（ドラッグでない）で全体表示に戻る。写真が変わったら全体表示に戻る（写真ごとに
//   PhotoView を作り直す）。開いた写真は閉じない（現像の画面に切り替えたときに、そのまま使える）。
// - I で撮影情報（LIB-17。情報 1 → 情報 2 → なし。項目は設定の「表示」で選ぶ）の表示を切り替える
//   （InfoOverlay。現像の画面と同じ表示）。
//
// # 動画
//
// - アプリの中で再生する（VID-04。VideoPlayer.tsx）。再生する前は、サムネイル（代表フレーム。VID-02）を出す。
//   長さ・フレームレート・コーデック・解像度（VID-03）と「OS のアプリで再生」も出す。

import { type PointerEvent as ReactPointerEvent, useEffect, useLayoutEffect, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import { type Frame, drawFrame } from "../api/frames";
import type { RegionInfo, VariantDetails, VariantId, VariantSummary } from "../api/types";
import { previewUrl } from "../api/uri";
import { isTyping, useHotkeys } from "../keyboard";
import { actions, appStore, useApp } from "../state/app";
import { useDevelop } from "../state/develop";
import { openRelink } from "../state/files";
import { cancelRegionRender, regionFor, renderRegion } from "../state/region";
import { searchCache, useSearchVersion } from "../state/search";
import { useCoreEvent, useViewRect } from "./hooks";
import { InfoOverlay } from "./InfoOverlay";
import { cycleInfoMode } from "./infoItems";
import { resolveOutputSize } from "./outputSize";
import { prefetchL1 } from "./prefetch";
import { SlotImages, usePreviewImages } from "./previewImages";
import {
  DRAG_THRESHOLD_PX,
  MAX_REGION_PIXELS,
  PAN_SETTLE_MS,
  type Rect,
  clampCenter,
  expandRegion,
  fitRect,
  l1RequestDelay,
  limitRegion,
  normalizedPoint,
  orientedSize,
  pannedCenter,
  prefetchIndices,
  regionUsable,
  sameRect,
  snapToDevicePixels,
  zoomOffset,
} from "./viewerMath";
import { VideoPlayer } from "./VideoPlayer";
import "./viewer.css";

/** 一時的な知らせ（等倍表示を使えない理由など）を出しておく時間（ミリ秒。仮置き）。 */
const MESSAGE_MS = 5000;

/** 動画の詳細を読みに行くまでの待ち（ミリ秒。写真送りの間に読みに行かない。仮置き）。 */
const DETAILS_DELAY_MS = 100;

// ---------------------------------------------------------------------------
// ルーペ
// ---------------------------------------------------------------------------

/** 表示する写真が変わったこと（写真送り）。 */
interface Navigation {
  /** 通し番号（表示する写真が変わるたびに増える）。 */
  seq: number;
  /** 変わった時刻（performance.now()）。 */
  at: number;
  /** L1 の要求と先読みを待つ時間（ミリ秒。写真送りの途中なら L1_SETTLE_MS。l1RequestDelay）。 */
  delay: number;
}

/** 待つ時間の残り（ミリ秒）。 */
function remainingDelay(nav: Navigation): number {
  return Math.max(0, nav.at + nav.delay - performance.now());
}

export function Loupe() {
  const current = useApp((s) => s.selection.current);
  const currentIndex = useApp((s) => s.selection.currentIndex);
  const count = useApp((s) => s.search?.count ?? 0);
  const hotkeysEnabled = useApp((s) => s.dialog === null && s.mode === "loupe");
  const detailsVersion = useApp((s) => s.detailsVersion);
  const revOverride = useApp((s) => (current !== null ? s.thumbRevs[current] : undefined));
  useSearchVersion();

  // 写真送りの間隔（表示する写真が変わったときだけ、描画の中で更新する。StrictMode の 2 回目の描画では
  // 同じ写真なので変わらない）。
  const navRef = useRef<Navigation & { id: VariantId | null }>({
    id: null,
    seq: 0,
    at: Number.NEGATIVE_INFINITY,
    delay: 0,
  });
  if (navRef.current.id !== current) {
    const now = performance.now();
    navRef.current = {
      id: current,
      seq: navRef.current.seq + 1,
      at: now,
      delay: l1RequestDelay(now - navRef.current.at),
    };
  }
  const nav = navRef.current;
  /** 表示中の写真の L1 が読めた（または作成待ちになった）写真送りの通し番号。先読みはその後で始める。 */
  const [settledSeq, setSettledSeq] = useState(0);

  const listed = findSummary(current, currentIndex);
  // 動画の情報（VID-03）と、検索結果のキャッシュにない写真の概要のために、詳細を読む。
  const details = useVariantDetails(current, detailsVersion, !listed || listed.kind === "video");
  const summary = listed ?? (details && details.summary.variant_id === current ? details.summary : undefined);

  useHotkeys(
    {
      i: () => {
        cycleInfoMode();
      },
    },
    hotkeysEnabled,
  );
  // 動画には L1 がないので、待たずに先読みする。概要が分からない間は先読みしない。
  usePrefetchNeighbors(currentIndex, nav, summary?.kind === "video" || settledSeq === nav.seq);

  if (current === null) {
    return (
      <div className="loupe">
        <div className="empty-state">{count === 0 ? "表示する写真・動画がありません" : "写真が選ばれていません"}</div>
      </div>
    );
  }
  if (!summary) {
    return (
      <div className="loupe">
        <div className="loupe-center-note">
          <div className="spinner" />
        </div>
      </div>
    );
  }
  const thumbRev = revOverride ?? summary.thumb_rev;
  return (
    <div className="loupe">
      {summary.kind === "video" ? (
        <VideoPlayer
          key={current}
          summary={summary}
          details={details}
          thumbRev={thumbRev}
          hotkeysEnabled={hotkeysEnabled}
        />
      ) : (
        <PhotoView
          key={current}
          summary={summary}
          thumbRev={thumbRev}
          hotkeysEnabled={hotkeysEnabled}
          nav={nav}
          onL1Settled={setSettledSeq}
        />
      )}
      <InfoOverlay
        summary={summary}
        details={details}
        index={currentIndex}
        count={count}
        version={detailsVersion}
      />
      <FileNotice summary={summary} />
    </div>
  );
}

/** 表示中の写真の概要（検索結果のキャッシュから）。 */
function findSummary(id: VariantId | null, index: number | null): VariantSummary | undefined {
  if (id === null) return undefined;
  if (index !== null) {
    const v = searchCache.get(index);
    if (v && v.variant_id === id) return v;
  }
  return searchCache.getLoadedById(id);
}

/** variant の詳細（`enabled` のときだけ。写真送りの間は読みに行かない）。 */
function useVariantDetails(id: VariantId | null, version: number, enabled: boolean): VariantDetails | null {
  const [loaded, setLoaded] = useState<VariantDetails | null>(null);
  useEffect(() => {
    if (id === null || !enabled) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      core
        .variantDetails(id)
        .then((d) => {
          if (!cancelled) setLoaded(d);
        })
        .catch((e) => {
          // 削除された直後（not_found）は知らせない（選択は検索のし直しで移る）。
          if (!cancelled && toApiError(e).kind !== "not_found") actions.reportError(e, "写真の情報を読めません");
        });
    }, DETAILS_DELAY_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [id, version, enabled]);
  return loaded && loaded.summary.variant_id === id ? loaded : null;
}

/**
 * 前後の写真の L1 を先読みする（PERF-04）。進む向きを覚えて、その向きを多めに読む。`ready`（表示中の写真の
 * L1 が読めた・作成待ちになった）になってから、写真送りの待ち（nav.delay）の残りを待って読む。
 */
function usePrefetchNeighbors(currentIndex: number | null, nav: Navigation, ready: boolean): void {
  const version = useSearchVersion();
  const prev = useRef<number | null>(null);
  const direction = useRef<1 | -1>(1);
  /** 先読みの対象（id）。作り終えた知らせで読み直す。 */
  const targets = useRef(new Set<VariantId>());
  useEffect(() => {
    if (currentIndex === null) return;
    if (prev.current !== null && prev.current !== currentIndex) direction.current = currentIndex > prev.current ? 1 : -1;
    prev.current = currentIndex;
  }, [currentIndex]);
  useEffect(() => {
    targets.current = new Set();
    if (currentIndex === null || !ready) return;
    const run = () => {
      const revs = appStore.get().thumbRevs;
      for (const i of prefetchIndices(currentIndex, searchCache.count, direction.current)) {
        const v = searchCache.get(i);
        // 動画には L1 がない（作る処理を予約しない）。
        if (!v || v.kind !== "photo") continue;
        const rev = revs[v.variant_id] ?? v.thumb_rev;
        targets.current.add(v.variant_id);
        prefetchL1(v.variant_id, rev, previewUrl(v.variant_id, rev));
      }
    };
    const wait = remainingDelay(nav);
    if (wait <= 0) {
      run();
      return;
    }
    const timer = setTimeout(run, wait);
    return () => clearTimeout(timer);
    // nav は写真送りの通し番号で比べる。
  }, [currentIndex, version, ready, nav.seq]);
  useCoreEvent((e) => {
    // 作成待ちの印は prefetch.ts が先に消している。
    if (e.type !== "thumbnail_updated" || !targets.current.has(e.variant_id)) return;
    prefetchL1(e.variant_id, e.rev, previewUrl(e.variant_id, e.rev));
  });
}

// ---------------------------------------------------------------------------
// 写真
// ---------------------------------------------------------------------------

type Zoom =
  | { kind: "fit" }
  | { kind: "preparing" }
  /** 等倍。cx・cy は表示の中心（出力の画素。表示の大きさに合わせた制限の前の値）。 */
  | { kind: "zoomed"; fullW: number; fullH: number; cx: number; cy: number };

function PhotoView(props: {
  summary: VariantSummary;
  thumbRev: string | null;
  hotkeysEnabled: boolean;
  nav: Navigation;
  /** L1 が読めた・作成待ちになった（先読みを始めてよい）。写真送りの通し番号を渡す。 */
  onL1Settled: (seq: number) => void;
}) {
  const { summary, thumbRev, nav } = props;
  const id = summary.variant_id;
  const fileOk = summary.file_status === "ok";
  const missing = summary.file_status === "missing";
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

  // --- 画像（L0・L1。previewImages.tsx） ---
  // 写真送りの途中は少し待ってから L1 を要求する。
  const { l0, l1, l1Failed, onL1Error, l1Settled, l1Gone } = usePreviewImages({
    summary,
    thumbRev,
    waitMs: () => remainingDelay(nav),
  });

  // 表示中の写真の L1 が決まった（読めた・作成待ち・作れなかった）: 前後の先読みを始めてよい。
  const { onL1Settled } = props;
  useEffect(() => {
    if (l1Settled) onL1Settled(nav.seq);
  }, [l1Settled, onL1Settled, nav.seq]);

  const shown = l1.shown ?? l0.shown;
  const natural = shown
    ? { width: shown.width, height: shown.height }
    : (orientedSize(summary.width, summary.height, summary.orientation) ?? { width: 3, height: 2 });
  const box = fitRect(natural.width, natural.height, view.width, view.height);

  // --- 等倍表示 ---
  const [zoom, setZoom] = useState<Zoom>({ kind: "fit" });
  const [region, setRegion] = useState<Frame<RegionInfo> | null>(null);
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [regionFailed, setRegionFailed] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const zoomSeq = useRef(0);
  const regionSeq = useRef(0);
  const firstRegion = useRef(true);
  /** 表示範囲の描画を要求したことがあるか（取り消しを送るかどうか）。 */
  const usedRegion = useRef(false);
  /**
   * 最後に要求した範囲と現像設定の世代（届いた結果が今の表示を含まなければ要求し直すが、同じ範囲・同じ世代を
   * 繰り返さない）。
   */
  const lastRequest = useRef<{ rect: Rect; generation: number | null } | null>(null);

  useEffect(() => {
    if (!message) return;
    const t = setTimeout(() => setMessage(null), MESSAGE_MS);
    return () => clearTimeout(t);
  }, [message]);

  const startZoom = (u: number, v: number) => {
    if (!fileOk) {
      setMessage(
        missing ? "元ファイルが見つからないため、等倍表示にできません" : "元ファイルを読めないため、等倍表示にできません",
      );
      return;
    }
    const my = ++zoomSeq.current;
    const stale = () => my !== zoomSeq.current || !mounted.current;
    usedRegion.current = true;
    firstRegion.current = true;
    lastRequest.current = null;
    setMessage(null);
    setRegion(null);
    setRegionFailed(false);
    setZoom({ kind: "preparing" });
    resolveOutputSize(id, stale)
      .then((r) => {
        if (stale()) return;
        if (r.kind === "size") {
          setZoom({ kind: "zoomed", fullW: r.width, fullH: r.height, cx: u * r.width, cy: v * r.height });
        } else {
          setZoom({ kind: "fit" });
          if (r.kind === "unavailable") setMessage(r.reason);
        }
      })
      .catch((e) => {
        if (stale()) return;
        setZoom({ kind: "fit" });
        actions.reportError(e, "等倍表示にできません");
      });
  };

  const exitZoom = () => {
    zoomSeq.current++;
    regionSeq.current++;
    lastRequest.current = null;
    setZoom({ kind: "fit" });
    setRegion(null);
    setBusy(false);
    setDragging(false);
    if (usedRegion.current) cancelRegionRender();
  };
  const exitZoomRef = useRef(exitZoom);
  exitZoomRef.current = exitZoom;

  // 写真が変わった・ルーペを閉じた: 描画を取り消す（写真は閉じない）。
  useEffect(
    () => () => {
      zoomSeq.current++;
      regionSeq.current++;
      if (usedRegion.current) cancelRegionRender();
    },
    [],
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

  const hover = useRef<{ u: number; v: number } | null>(null);
  useHotkeys(
    {
      z: () => {
        if (zoom.kind === "fit") startZoom(hover.current?.u ?? 0.5, hover.current?.v ?? 0.5);
        else exitZoom();
      },
    },
    props.hotkeysEnabled,
  );

  // 開いている写真の現像設定の、分かっている最新の世代（設定の貼り付け・Undo などで外から変わったら、
  // 描いてある範囲を描き直す）。コアの状態（reloadState などで更新）と現像のプレビューの大きい方。
  const devGeneration = useDevelop((s) => {
    if (s.variantId !== id) return null;
    const a = s.state && s.state.variant_id === id ? s.state.generation : null;
    const b = s.frame && s.frame.header.variant_id === id ? s.frame.header.generation : null;
    return a === null ? b : b === null ? a : Math.max(a, b);
  });

  const zoomed = zoom.kind === "zoomed" ? zoom : null;
  const center = zoomed
    ? {
        x: clampCenter(zoomed.cx, zoomed.fullW, view.width, view.dpr),
        y: clampCenter(zoomed.cy, zoomed.fullH, view.height, view.dpr),
      }
    : null;

  // 見えている範囲を描く（最初はすぐ、パン・大きさの変化の後は PAN_SETTLE_MS 待ってから）。
  useEffect(() => {
    if (!zoomed || !center || dragging || regionFailed || view.width <= 0 || view.height <= 0) return;
    const visible = regionFor({
      fullWidth: zoomed.fullW,
      fullHeight: zoomed.fullH,
      centerX: center.x,
      centerY: center.y,
      scale: 1,
      viewWidth: view.width,
      viewHeight: view.height,
      devicePixelRatio: view.dpr,
    });
    const need = limitRegion(expandRegion(visible, 1, zoomed.fullW, zoomed.fullH), MAX_REGION_PIXELS);
    if (regionUsable(region?.header, need, zoomed.fullW, zoomed.fullH, devGeneration)) return;
    // 同じ範囲・同じ世代を要求済み（描いている途中・描けた結果が届いた後）なら、もう一度は要求しない。
    const last = lastRequest.current;
    if (last && sameRect(last.rect, need) && last.generation === devGeneration) return;
    const timer = setTimeout(
      () => {
        firstRegion.current = false;
        lastRequest.current = { rect: need, generation: devGeneration };
        const my = ++regionSeq.current;
        setBusy(true);
        renderRegion({ variant_id: id, space: "output", ...need, scale: 1 })
          .then((f) => {
            if (my !== regionSeq.current || !mounted.current) return;
            setBusy(false);
            if (!f || f.header.variant_id !== id) return;
            setRegion(f);
            const { full_width: fw, full_height: fh } = f.header;
            // 現像設定（切り抜き）が変わって出力の大きさが変わった: 中心を割合で保って合わせる。
            setZoom((z) =>
              z.kind === "zoomed" && (z.fullW !== fw || z.fullH !== fh)
                ? { ...z, fullW: fw, fullH: fh, cx: (z.cx / z.fullW) * fw, cy: (z.cy / z.fullH) * fh }
                : z,
            );
          })
          .catch((e) => {
            if (my !== regionSeq.current || !mounted.current) return;
            setBusy(false);
            setRegionFailed(true);
            actions.reportError(e, "等倍表示を描けません");
          });
      },
      firstRegion.current ? 0 : PAN_SETTLE_MS,
    );
    return () => clearTimeout(timer);
    // center は zoom と view から求めた値なので、x・y の数で比べる（描画ごとに作り直すオブジェクトにしない）。
  }, [
    id,
    zoomed?.fullW,
    zoomed?.fullH,
    center?.x,
    center?.y,
    dragging,
    regionFailed,
    view.width,
    view.height,
    view.dpr,
    region,
    devGeneration,
  ]);

  // 描いた範囲を canvas に出す（位置・大きさは描画の中で決まる。同じフレームで描く）。
  useLayoutEffect(() => {
    const c = canvasRef.current;
    if (c && region) drawFrame(c, region);
  }, [region, zoomed !== null]);

  // --- マウス（クリックで等倍、ドラッグでパン） ---
  const drag = useRef<{ x0: number; y0: number; cx0: number; cy0: number; moved: boolean } | null>(null);
  const localPoint = (e: ReactPointerEvent) => {
    const r = e.currentTarget.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };
  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    drag.current = { x0: e.clientX, y0: e.clientY, cx0: center?.x ?? 0, cy0: center?.y ?? 0, moved: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (zoom.kind === "fit") {
      const p = localPoint(e);
      const n = normalizedPoint(p.x, p.y, box);
      hover.current = n.inside ? { u: n.u, v: n.v } : null;
    }
    const d = drag.current;
    if (!d || !zoomed) return;
    const dx = e.clientX - d.x0;
    const dy = e.clientY - d.y0;
    if (!d.moved) {
      if (Math.hypot(dx, dy) < DRAG_THRESHOLD_PX) return;
      d.moved = true;
      setDragging(true);
    }
    const { width, height, dpr } = view;
    setZoom((z) =>
      z.kind === "zoomed"
        ? {
            ...z,
            cx: clampCenter(pannedCenter(d.cx0, dx, dpr), z.fullW, width, dpr),
            cy: clampCenter(pannedCenter(d.cy0, dy, dpr), z.fullH, height, dpr),
          }
        : z,
    );
  };
  const onPointerUp = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (d.moved) {
      setDragging(false);
      return;
    }
    if (Math.hypot(e.clientX - d.x0, e.clientY - d.y0) >= DRAG_THRESHOLD_PX) return;
    if (zoom.kind === "fit") {
      const p = localPoint(e);
      const n = normalizedPoint(p.x, p.y, box);
      if (n.inside) startZoom(n.u, n.v);
    } else if (zoom.kind === "zoomed") {
      exitZoom();
    }
  };
  const onPointerCancel = () => {
    if (drag.current?.moved) setDragging(false);
    drag.current = null;
  };

  // --- 表示 ---
  const dpr = view.dpr;
  const layer =
    zoomed && center
      ? (() => {
          const raw = zoomOffset(center, view, dpr);
          return {
            x: snapToDevicePixels(raw.x, view.left, dpr),
            y: snapToDevicePixels(raw.y, view.top, dpr),
          };
        })()
      : null;
  const rh = region && region.header.variant_id === id ? region.header : null;
  const background = l1.shown?.url ?? l0.shown?.url ?? null;

  let status: string | null = null;
  if (zoom.kind === "preparing") status = "等倍表示を準備しています…";
  else if (zoomed && busy) status = rh ? "描き直しています…" : "等倍で描いています…";
  else if (zoom.kind === "fit" && !l1.shown && l1Gone && fileOk && !l1Failed) {
    status = "標準プレビューを作っています…";
  }
  const nothing = !l0.shown && !l1.shown;
  const noImage = nothing && (l0.url === null || l0.failed) && l1Gone;

  const viewClass = [
    "loupe-view",
    zoom.kind === "fit" ? "fit" : zoom.kind,
    dragging ? "dragging" : "",
  ].join(" ");

  return (
    <>
      <div
        ref={viewRef}
        className={viewClass}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerCancel}
        onPointerLeave={() => {
          hover.current = null;
        }}
      >
        <div
          className="loupe-photo"
          style={{
            left: box.x,
            top: box.y,
            width: box.width,
            height: box.height,
            display: zoomed ? "none" : undefined,
          }}
        >
          {!l1.shown && <SlotImages slot={l0} />}
          <SlotImages slot={l1} onError={onL1Error} />
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
              style={
                rh
                  ? { left: rh.x / dpr, top: rh.y / dpr, width: rh.width / dpr, height: rh.height / dpr }
                  : { display: "none" }
              }
            />
          </div>
        )}
        {nothing && !noImage && (
          <div className="loupe-center-note">
            <div className="spinner" />
          </div>
        )}
        {noImage && (
          <div className="loupe-center-note">
            <p>プレビューがありません</p>
            {!fileOk && (
              <p className="hint">
                {missing ? "元ファイルが見つからないため、作れません" : "元ファイルを読めないため、作れません"}
              </p>
            )}
          </div>
        )}
      </div>
      {zoomed && <div className="loupe-badge">等倍（Z・Esc で全体表示）</div>}
      {(status || message) && (
        <div className="loupe-status" role="status">
          {status && zoom.kind === "preparing" && <span className="spinner small" />}
          <span>{message ?? status}</span>
        </div>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// 情報の表示
// ---------------------------------------------------------------------------

function FileNotice(props: { summary: VariantSummary }) {
  const { summary } = props;
  if (summary.file_status === "ok") return null;
  const missing = summary.file_status === "missing";
  const photo = summary.kind === "photo";
  return (
    <div className="loupe-notice" role="status">
      <strong>{missing ? "元ファイルが見つかりません" : "元ファイルを読めません"}</strong>
      <div>
        {missing
          ? "移動・削除されたか、ドライブが接続されていない可能性があります。"
          : "ファイルが壊れているか、読み取れない形式の可能性があります。"}
        {photo ? "等倍表示はできません。" : missing ? "再生できません。" : ""}
      </div>
      {missing && (
        <div className="loupe-notice-actions">
          <button type="button" className="btn small" onClick={() => openRelink([summary.variant_id])}>
            探して結び付け直す…
          </button>
        </div>
      )}
    </div>
  );
}
