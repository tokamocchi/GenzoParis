// フィルムストリップ（ルーペ・現像の下の、検索結果のサムネイルの横の列。LIB-02）。
//
// - 件数だけで横に仮想スクロールする（@tanstack/react-virtual の horizontal）。見えている範囲のページを
//   検索結果のキャッシュ（state/search.ts）に要求する。
// - 50 万件では内容の幅がブラウザの要素の大きさの上限を超えうるので、スクロールの領域の幅を
//   MAX_FILMSTRIP_SCROLL_PX までに抑え、スクロールの位置（物理）と内容の位置（仮想）を比例させる
//   （react-virtual の observeElementOffset・scrollToFn の間に入る。上限に収まるときは同じ値）。
// - 表示中の写真（selection.current）を枠で強調し、見える位置に保つ（端に寄ったら中央に置く）。
// - クリックで選ぶ（Shift で範囲、Ctrl / ⌘ で追加・解除）。縦のホイールで横に送る。

import { elementScroll, observeElementOffset, useVirtualizer } from "@tanstack/react-virtual";
import { type MouseEvent, type WheelEvent, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import type { VariantSummary } from "../api/types";
import { thumbUrl } from "../api/uri";
import { FlagMark, LabelDot } from "../components/Marks";
import { formatCaptureTime } from "../format";
import { actions, useApp } from "../state/app";
import { searchCache, useSearchVersion } from "../state/search";
import { MAX_FILMSTRIP_SCROLL_PX, keepVisibleOffset, toRealOffset, toVirtualOffset } from "./viewerMath";
import "./viewer.css";

/** 1 項目の幅の既定値（px。高さを測るまでの仮置き。--filmstrip-h の 96 px からスクロールバーを除いた目安）。 */
const DEFAULT_CELL = 84;

/** 1 項目の幅の下限（px。仮置き）。 */
const MIN_CELL = 40;

/** 見えている範囲の外に余分に描く数（仮置き）。 */
const OVERSCAN = 6;

export function Filmstrip() {
  const count = useApp((s) => s.search?.count ?? 0);
  const currentIndex = useApp((s) => s.selection.currentIndex);
  const current = useApp((s) => s.selection.current);
  const ids = useApp((s) => s.selection.ids);
  const thumbRevs = useApp((s) => s.thumbRevs);
  useSearchVersion();

  const scrollRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ cell: DEFAULT_CELL, viewport: 0 });
  const { cell, viewport } = size;

  // 項目は正方形（高さはスクロールバーを除いた内側の高さ）。
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const update = () => {
      const next = { cell: Math.max(MIN_CELL, el.clientHeight), viewport: el.clientWidth };
      setSize((prev) => (prev.cell === next.cell && prev.viewport === next.viewport ? prev : next));
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // スクロールの位置の対応（物理 ↔ 仮想）。
  const virtualTotal = count * cell;
  const realTotal = Math.min(virtualTotal, MAX_FILMSTRIP_SCROLL_PX);
  const realMax = Math.max(0, realTotal - viewport);
  const virtualMax = Math.max(0, virtualTotal - viewport);
  const mapRef = useRef({ realMax, virtualMax });
  mapRef.current = { realMax, virtualMax };

  const virtualizer = useVirtualizer({
    horizontal: true,
    count,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => cell,
    overscan: OVERSCAN,
    observeElementOffset: (instance, cb) =>
      observeElementOffset(instance, (offset, isScrolling) =>
        cb(toVirtualOffset(offset, mapRef.current.realMax, mapRef.current.virtualMax), isScrolling),
      ),
    scrollToFn: (offset, options, instance) =>
      elementScroll(
        toRealOffset(offset + (options.adjustments ?? 0), mapRef.current.realMax, mapRef.current.virtualMax),
        { behavior: options.behavior },
        instance,
      ),
  });

  // 項目の幅・対応が変わった: 測り直し、今のスクロールの位置を読み直させる。
  useEffect(() => {
    virtualizer.measure();
    scrollRef.current?.dispatchEvent(new Event("scroll"));
  }, [virtualizer, cell, realMax, virtualMax]);

  // 表示中の写真を見える位置に保つ。
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || currentIndex === null || viewport <= 0) return;
    const offset = toVirtualOffset(el.scrollLeft, realMax, virtualMax);
    const target = keepVisibleOffset({ index: currentIndex, cell, offset, viewport, count });
    if (target !== null) el.scrollLeft = toRealOffset(target, realMax, virtualMax);
  }, [currentIndex, cell, viewport, count, realMax, virtualMax]);

  const selected = useMemo(() => new Set(ids), [ids]);

  // 縦のホイールで横に送る（内容の位置で 1 対 1 に動くよう、物理の量に直す）。
  const onWheel = (e: WheelEvent<HTMLDivElement>) => {
    const el = scrollRef.current;
    if (!el || Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;
    const unit = e.deltaMode === 1 ? cell : e.deltaMode === 2 ? viewport : 1;
    const ratio = virtualMax > 0 ? realMax / virtualMax : 1;
    el.scrollLeft += e.deltaY * unit * ratio;
  };

  const virtualOffset = virtualizer.scrollOffset ?? 0;
  const realOffset = toRealOffset(virtualOffset, realMax, virtualMax);

  return (
    <div className="filmstrip">
      <div ref={scrollRef} className="filmstrip-scroll" onWheel={onWheel}>
        <div className="filmstrip-content" style={{ width: realTotal }}>
          {virtualizer.getVirtualItems().map((vi) => (
            <FilmstripCell
              key={vi.key}
              index={vi.index}
              left={realOffset + (vi.start - virtualOffset)}
              size={cell}
              currentId={current}
              currentIndex={currentIndex}
              selected={selected}
              thumbRevs={thumbRevs}
            />
          ))}
        </div>
      </div>
      {count === 0 && <div className="filmstrip-empty">表示する写真・動画がありません</div>}
    </div>
  );
}

function FilmstripCell(props: {
  index: number;
  left: number;
  size: number;
  currentId: number | null;
  currentIndex: number | null;
  selected: Set<number>;
  thumbRevs: Record<number, string>;
}) {
  const v: VariantSummary | undefined = searchCache.get(props.index);
  const [failed, setFailed] = useState<string | null>(null);
  const isCurrent = v
    ? v.variant_id === props.currentId
    : props.currentIndex !== null && props.currentIndex === props.index;
  const isSelected = !!v && props.selected.has(v.variant_id);
  const rev = v ? (props.thumbRevs[v.variant_id] ?? v.thumb_rev) : null;
  // rev がなければサムネイルはまだない（要求しない。できたら thumbnail_updated で rev が来る）。
  const url = v && rev ? thumbUrl(v.variant_id, rev) : null;

  const onClick = (e: MouseEvent) => {
    void actions.selectIndex(props.index, { extend: e.shiftKey, toggle: e.ctrlKey || e.metaKey });
  };

  const cls = ["filmstrip-cell", isCurrent ? "current" : "", isSelected ? "selected" : ""].join(" ");
  return (
    <div
      className={cls}
      style={{ transform: `translateX(${props.left}px)`, width: props.size, height: props.size }}
      onClick={onClick}
      title={v ? `${v.file_name}\n${formatCaptureTime(v.capture)}` : undefined}
    >
      <div className="filmstrip-frame">
        {url && failed !== url ? (
          <img src={url} alt="" draggable={false} decoding="async" onError={() => setFailed(url)} />
        ) : (
          <span className="filmstrip-ph">{v ? (v.kind === "video" ? "動画" : "") : ""}</span>
        )}
        {v && (
          <div className="filmstrip-marks">
            {v.rating > 0 && <span className="filmstrip-rating">{"★".repeat(v.rating)}</span>}
            <FlagMark flag={v.flag} />
            <LabelDot label={v.color_label} />
          </div>
        )}
        {v?.kind === "video" && (
          <span className="filmstrip-kind" title="動画">
            ▶
          </span>
        )}
        {v && v.file_status !== "ok" && (
          <span
            className="filmstrip-missing"
            title={v.file_status === "missing" ? "元ファイルが見つかりません" : "元ファイルを読めません"}
          >
            !
          </span>
        )}
      </div>
    </div>
  );
}
