// ルーペ（1 枚表示と等倍表示。LIB-02・PRV-03・PERF-04/05・VID-02/03）。
//
// # 写真
//
// - 標準プレビュー（L1。長辺 2560）を画面に合わせて中央に出す。L1 が読めるまでは、サムネイル（L0）を同じ
//   大きさで出す（PERF-05: 先に埋め込み JPEG、後で現像結果）。
// - L1 がまだない（404）ときは、genzo:// の処理がコアに作る処理を予約する（genzo-app の protocol.rs）。
//   作り終えた知らせ（thumbnail_updated）で rev が変われば URI が変わって読み直す。rev が同じまま（L1 だけが
//   キャッシュの回収で消えていた）のときは、URI に印（retry）を付けて読み直す。
// - 前後の写真の L1 を先に読んでおく（PERF-04。prefetch.ts）。
// - 等倍表示（Z・クリックした点を中心に）: 現像のセッション（state/develop.ts。現像の画面と同じ）で写真を
//   開き、出力の画像のフル解像度の大きさを求めてから（outputSize.ts）、見えている範囲だけをコアに描かせて
//   canvas に出す（state/region.ts）。画像の 1 画素を画面の 1 物理画素にする（canvas の CSS の大きさは
//   画素 / devicePixelRatio。位置も物理画素の境界に合わせる。UX-03）。
// - ドラッグでパンし、離してから PAN_SETTLE_MS 後に見えている範囲を描き直す。描き直す間は、前の範囲と
//   拡大した L1 を出しておく。
// - Z・Esc・クリック（ドラッグでない）で全体表示に戻る。写真が変わったら全体表示に戻る（写真ごとに
//   PhotoView を作り直す）。開いた写真は閉じない（現像の画面に切り替えたときに、そのまま使える）。
// - I で情報（ファイル名・撮影日時・評価）の表示を切り替える。
//
// # 動画
//
// - サムネイル（代表フレーム。VID-02）を大きく出し、長さ・フレームレート・コーデック・解像度（VID-03）と
//   「OS のアプリで再生」（VID-04 は v1 のため OS のアプリを使う）を出す。

import { type PointerEvent as ReactPointerEvent, useEffect, useLayoutEffect, useRef, useState } from "react";

import * as appApi from "../api/app";
import * as core from "../api/core";
import { toApiError } from "../api/errors";
import { type Frame, drawFrame } from "../api/frames";
import type { RegionInfo, VariantDetails, VariantId, VariantSummary } from "../api/types";
import { previewUrl, thumbUrl } from "../api/uri";
import { FlagMark, LabelDot, Stars } from "../components/Marks";
import { formatCaptureTime, formatDuration } from "../format";
import { isTyping, useHotkeys } from "../keyboard";
import { actions, appStore, useApp } from "../state/app";
import { cancelRegionRender, regionFor, renderRegion } from "../state/region";
import { searchCache, useSearchVersion } from "../state/search";
import { useCoreEvent, useViewRect } from "./hooks";
import { resolveOutputSize } from "./outputSize";
import { prefetchImage } from "./prefetch";
import {
  DRAG_THRESHOLD_PX,
  MAX_REGION_PIXELS,
  PAN_SETTLE_MS,
  type Rect,
  clampCenter,
  expandRegion,
  fitRect,
  formatCodec,
  formatFps,
  formatResolution,
  limitRegion,
  normalizedPoint,
  orientedSize,
  pannedCenter,
  prefetchIndices,
  regionCovers,
  sameRect,
  snapToDevicePixels,
  withRetryParam,
  zoomOffset,
} from "./viewerMath";
import "./viewer.css";

/** L1 の読み直しの回数の上限（同じ rev のまま読めない場合。仮置き）。 */
const MAX_L1_RETRY = 5;

/** 一時的な知らせ（等倍表示を使えない理由など）を出しておく時間（ミリ秒。仮置き）。 */
const MESSAGE_MS = 5000;

/** 動画の詳細を読みに行くまでの待ち（ミリ秒。写真送りの間に読みに行かない。仮置き）。 */
const DETAILS_DELAY_MS = 100;

const SHOW_INFO_KEY = "genzo.loupe.showInfo";

function loadShowInfo(): boolean {
  try {
    return localStorage.getItem(SHOW_INFO_KEY) !== "0";
  } catch {
    return true;
  }
}

function saveShowInfo(v: boolean): void {
  try {
    localStorage.setItem(SHOW_INFO_KEY, v ? "1" : "0");
  } catch {
    // 保存できなくても、このセッションでは切り替える。
  }
}

// ---------------------------------------------------------------------------
// ルーペ
// ---------------------------------------------------------------------------

export function Loupe() {
  const current = useApp((s) => s.selection.current);
  const currentIndex = useApp((s) => s.selection.currentIndex);
  const count = useApp((s) => s.search?.count ?? 0);
  const hotkeysEnabled = useApp((s) => s.dialog === null && s.mode === "loupe");
  const detailsVersion = useApp((s) => s.detailsVersion);
  const revOverride = useApp((s) => (current !== null ? s.thumbRevs[current] : undefined));
  useSearchVersion();
  const [showInfo, setShowInfo] = useState(loadShowInfo);

  const listed = findSummary(current, currentIndex);
  // 動画の情報（VID-03）と、検索結果のキャッシュにない写真の概要のために、詳細を読む。
  const details = useVariantDetails(current, detailsVersion, !listed || listed.kind === "video");
  const summary = listed ?? (details && details.summary.variant_id === current ? details.summary : undefined);

  useHotkeys(
    {
      i: () => {
        saveShowInfo(!showInfo);
        setShowInfo(!showInfo);
      },
    },
    hotkeysEnabled,
  );
  usePrefetchNeighbors(currentIndex);

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
        <VideoView key={current} summary={summary} details={details} thumbRev={thumbRev} />
      ) : (
        <PhotoView key={current} summary={summary} thumbRev={thumbRev} hotkeysEnabled={hotkeysEnabled} />
      )}
      {showInfo && <InfoOverlay summary={summary} index={currentIndex} count={count} />}
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

/** 前後の写真の L1 を先読みする（PERF-04）。進む向きを覚えて、その向きを多めに読む。 */
function usePrefetchNeighbors(currentIndex: number | null): void {
  const version = useSearchVersion();
  const prev = useRef<number | null>(null);
  const direction = useRef<1 | -1>(1);
  /** 先読みの対象（id → URI）。作り終えた知らせで読み直す。 */
  const targets = useRef(new Map<VariantId, string>());
  useEffect(() => {
    if (currentIndex === null) return;
    if (prev.current !== null && prev.current !== currentIndex) direction.current = currentIndex > prev.current ? 1 : -1;
    prev.current = currentIndex;
    const revs = appStore.get().thumbRevs;
    const next = new Map<VariantId, string>();
    for (const i of prefetchIndices(currentIndex, searchCache.count, direction.current)) {
      const v = searchCache.get(i);
      // 動画には L1 がない（作る処理を予約しない）。
      if (!v || v.kind !== "photo") continue;
      const url = previewUrl(v.variant_id, revs[v.variant_id] ?? v.thumb_rev);
      next.set(v.variant_id, url);
      prefetchImage(url);
    }
    targets.current = next;
  }, [currentIndex, version]);
  useCoreEvent((e) => {
    if (e.type !== "thumbnail_updated" || !targets.current.has(e.variant_id)) return;
    const url = previewUrl(e.variant_id, e.rev);
    targets.current.set(e.variant_id, url);
    prefetchImage(url);
  });
}

// ---------------------------------------------------------------------------
// 画像の読み込み（読めるまで前の画像を出しておく）
// ---------------------------------------------------------------------------

interface LoadedImage {
  url: string;
  width: number;
  height: number;
}

function useImageSlot(url: string | null) {
  const [shown, setShown] = useState<LoadedImage | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  return {
    url,
    /** 最後に読めた画像（URI が変わっても、新しい画像が読めるまで出しておく）。 */
    shown,
    ready: url !== null && shown?.url === url,
    failed: url !== null && failed === url,
    onLoad: (u: string, img: HTMLImageElement) =>
      setShown({ url: u, width: img.naturalWidth, height: img.naturalHeight }),
    onError: (u: string) => setFailed(u),
  };
}

type ImageSlot = ReturnType<typeof useImageSlot>;

/** 読めた画像と、読み込み中の画像（隠す）。同じ URI の要素は作り直さない（key が URI）。 */
function SlotImages(props: { slot: ImageSlot; onError?: (url: string) => void }) {
  const { slot } = props;
  const urls: string[] = [];
  if (slot.shown) urls.push(slot.shown.url);
  if (slot.url && slot.url !== slot.shown?.url && !slot.failed) urls.push(slot.url);
  return (
    <>
      {urls.map((u) => (
        <img
          key={u}
          className="loupe-img"
          src={u}
          alt=""
          draggable={false}
          decoding="async"
          style={u === slot.shown?.url ? undefined : { visibility: "hidden" }}
          onLoad={(e) => slot.onLoad(u, e.currentTarget)}
          onError={() => {
            slot.onError(u);
            props.onError?.(u);
          }}
        />
      ))}
    </>
  );
}

// ---------------------------------------------------------------------------
// 写真
// ---------------------------------------------------------------------------

type Zoom =
  | { kind: "fit" }
  | { kind: "preparing" }
  /** 等倍。cx・cy は表示の中心（出力の画素。表示の大きさに合わせた制限の前の値）。 */
  | { kind: "zoomed"; fullW: number; fullH: number; cx: number; cy: number };

function PhotoView(props: { summary: VariantSummary; thumbRev: string | null; hotkeysEnabled: boolean }) {
  const { summary, thumbRev } = props;
  const id = summary.variant_id;
  const fileOk = summary.file_status === "ok";
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

  // --- 画像（L0・L1） ---
  const [retry, setRetry] = useState<{ rev: string | null; n: number }>({ rev: thumbRev, n: 0 });
  const retryCount = retry.rev === thumbRev ? retry.n : 0;
  const revRef = useRef(thumbRev);
  revRef.current = thumbRev;
  const bumpRetry = () =>
    setRetry((r) => {
      const n = r.rev === revRef.current ? r.n : 0;
      return n >= MAX_L1_RETRY ? r : { rev: revRef.current, n: n + 1 };
    });
  const l0 = useImageSlot(thumbRev ? thumbUrl(id, thumbRev) : null);
  const l1 = useImageSlot(withRetryParam(previewUrl(id, thumbRev), retryCount));
  const l1ReadyRef = useRef(false);
  l1ReadyRef.current = l1.ready;
  const [previewFailed, setPreviewFailed] = useState(false);

  // L1 が読めない: もう作ってあれば（知らせを取りこぼした）すぐ読み直す。なければ作る処理が予約される。
  const onL1Error = () => {
    if (!fileOk) return;
    core
      .requestPreview(id)
      .then((path) => {
        if (path && mounted.current) bumpRetry();
      })
      .catch(() => undefined);
  };
  useCoreEvent((e) => {
    if (e.type === "thumbnail_updated" && e.variant_id === id) {
      // rev が変われば URI が変わって読み直す。同じ rev なら印を付けて読み直す。
      if (!l1ReadyRef.current && e.rev === revRef.current) bumpRetry();
    } else if (e.type === "warning" && e.code === "preview_failed" && e.variant_id === id) {
      setPreviewFailed(true);
    }
  });

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
  /** 最後に要求した範囲（届いた結果が今の表示を含まなければ要求し直すが、同じ範囲を繰り返さない）。 */
  const lastRequest = useRef<Rect | null>(null);

  useEffect(() => {
    if (!message) return;
    const t = setTimeout(() => setMessage(null), MESSAGE_MS);
    return () => clearTimeout(t);
  }, [message]);

  const startZoom = (u: number, v: number) => {
    if (!fileOk) {
      setMessage("元ファイルを読めないため、等倍表示にできません");
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
    const have = region?.header;
    if (
      have &&
      have.full_width === zoomed.fullW &&
      have.full_height === zoomed.fullH &&
      regionCovers({ x: have.x, y: have.y, width: have.region_width, height: have.region_height }, need)
    ) {
      return;
    }
    // 同じ範囲を要求済み（描いている途中・描けた結果が届いた後）なら、もう一度は要求しない。
    if (lastRequest.current && sameRect(lastRequest.current, need)) return;
    const timer = setTimeout(
      () => {
        firstRegion.current = false;
        lastRequest.current = need;
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
  }, [id, zoomed?.fullW, zoomed?.fullH, center?.x, center?.y, dragging, regionFailed, view.width, view.height, view.dpr, region]);

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
  else if (zoom.kind === "fit" && !l1.shown && l1.failed && fileOk && !previewFailed) {
    status = "標準プレビューを作っています…";
  }
  const nothing = !l0.shown && !l1.shown;
  const noImage = nothing && (l0.url === null || l0.failed) && l1.failed;

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
            {!fileOk && <p className="hint">元ファイルが見つからないため、作れません</p>}
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
// 動画
// ---------------------------------------------------------------------------

function VideoView(props: { summary: VariantSummary; details: VariantDetails | null; thumbRev: string | null }) {
  const { summary, details } = props;
  const id = summary.variant_id;
  const viewRef = useRef<HTMLDivElement>(null);
  const view = useViewRect(viewRef);
  const slot = useImageSlot(props.thumbRev ? thumbUrl(id, props.thumbRev) : null);
  const tools = useApp((s) => s.catalog?.tools ?? null);
  // undefined: 詳細を読み込み中。null: 動画の情報がない（ffprobe がない・読めなかった）。
  const video = details && details.summary.variant_id === id ? details.video : undefined;
  const natural = slot.shown
    ? { width: slot.shown.width, height: slot.shown.height }
    : video?.width && video?.height
      ? { width: video.width, height: video.height }
      : (orientedSize(summary.width, summary.height, summary.orientation) ?? { width: 16, height: 9 });
  const box = fitRect(natural.width, natural.height, view.width, view.height);
  const noThumb = !slot.shown && (slot.url === null || slot.failed);
  const missing = summary.file_status === "missing";

  const play = () => {
    appApi.openVariantFile(id).catch((e) => actions.reportError(e, "動画を開けません"));
  };
  const openSettings = () => actions.openDialog({ kind: "settings", tab: "general" });

  return (
    <div className="loupe-video">
      <div ref={viewRef} className="loupe-view video">
        {!noThumb && (
          <div
            className="loupe-photo"
            style={{ left: box.x, top: box.y, width: box.width, height: box.height }}
            onDoubleClick={missing ? undefined : play}
            title={missing ? undefined : "ダブルクリックで OS のアプリで再生"}
          >
            <SlotImages slot={slot} />
            <span className="loupe-video-mark" aria-hidden="true">
              ▶
            </span>
          </div>
        )}
        {noThumb && (
          <div className="loupe-center-note">
            <p>動画のサムネイルがありません</p>
            {tools && !tools.ffmpeg && (
              <>
                <p className="hint">ffmpeg が見つからないため、サムネイルを作れません。</p>
                <button className="btn small" onClick={openSettings}>
                  設定で FFmpeg の場所を指定する
                </button>
              </>
            )}
          </div>
        )}
      </div>
      <div className="loupe-video-bar">
        <dl className="loupe-video-meta">
          <div>
            <dt>長さ</dt>
            <dd>{formatDuration(video?.duration_s)}</dd>
          </div>
          <div>
            <dt>フレームレート</dt>
            <dd>{formatFps(video?.fps)}</dd>
          </div>
          <div>
            <dt>コーデック</dt>
            <dd>{formatCodec(video?.codec, video?.bit_depth)}</dd>
          </div>
          <div>
            <dt>解像度</dt>
            <dd>{formatResolution(video?.width, video?.height)}</dd>
          </div>
        </dl>
        {video === null && (
          <span className="hint">
            {tools && !tools.ffprobe
              ? "ffprobe が見つからないため、動画の情報を読めません"
              : "動画の情報がありません"}
          </span>
        )}
        <span className="loupe-video-spacer" />
        <button className="btn" onClick={play} disabled={missing} title={missing ? "元ファイルが見つかりません" : undefined}>
          OS のアプリで再生
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// 情報の表示
// ---------------------------------------------------------------------------

function InfoOverlay(props: { summary: VariantSummary; index: number | null; count: number }) {
  const { summary } = props;
  return (
    <div className="loupe-info">
      <div className="loupe-info-name">
        {summary.file_name}
        {!summary.is_master && (
          <span className="muted">（仮想コピー{summary.variant_name ? `: ${summary.variant_name}` : ""}）</span>
        )}
      </div>
      <div>{formatCaptureTime(summary.capture)}</div>
      <div className="loupe-info-marks">
        <Stars rating={summary.rating} />
        <FlagMark flag={summary.flag} />
        <LabelDot label={summary.color_label} />
      </div>
      {props.index !== null && (
        <div className="muted">
          {(props.index + 1).toLocaleString()} / {props.count.toLocaleString()}
        </div>
      )}
    </div>
  );
}

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
    </div>
  );
}
