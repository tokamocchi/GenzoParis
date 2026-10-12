// フィルムストリップ（ルーペ・現像の下の、検索結果のサムネイルの横の列。LIB-02）。
//
// - 件数だけで横に仮想スクロールする（@tanstack/react-virtual の horizontal）。見えている範囲のページを
//   検索結果のキャッシュ（state/search.ts）に要求し、描いている範囲を知らせる（searchCache.setView）。
// - 速いスクロール（つまみのドラッグ・トラックの連打など）の間は、グリッドと同じ判定（scrollMapper.ts の
//   FastScrollDetector）で、検索結果のページとサムネイルを要求せず、読み込み済みの項目だけを出す（決定事項
//   ログ No.66）。遅くなるか止まったら、見えている範囲を要求する。
// - 項目（FilmstripCell）は memo にし、その項目の値（サムネイルの rev・表示中か・選択中か）だけを渡す。
// - 50 万件では内容の幅がブラウザの要素の大きさの上限を超えうるので、スクロールの領域の幅を
//   MAX_FILMSTRIP_SCROLL_PX までに抑え、スクロールの位置（物理）と内容の位置（仮想）を比例させる
//   （react-virtual の observeElementOffset・scrollToFn の間に入る。上限に収まるときは同じ値）。
// - 表示中の写真（selection.current）を枠で強調し、見える位置に保つ（端に寄ったら中央に置く）。
// - クリックでその写真を表示し、選択をその 1 枚にする。Ctrl / ⌘ + クリックは選択に足す・外す、Shift + クリックは
//   表示中の写真からの範囲を選択に足す（どちらも表示中の写真は変えない。Lightroom と同じ。state/app.ts の
//   selectIndex の keepCurrent）。縦のホイールで横に送る。
// - 横のスクロールバーは常に場所を取る（viewer.css の overflow-x: scroll）。項目の大きさを内側の高さから
//   決めるので、スクロールバーが出入りすると大きさが変わり、内容の幅が変わって、またスクロールバーが
//   出入りする、を繰り返すため。

import { elementScroll, observeElementOffset, useVirtualizer } from "@tanstack/react-virtual";
import { type MouseEvent, type WheelEvent, memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import type { VariantId, VariantSummary } from "../api/types";
import { thumbUrl } from "../api/uri";
import { FlagMark, LabelDot } from "../components/Marks";
import { formatCaptureTime } from "../format";
import { FastScrollDetector } from "../library/scrollMapper";
import { actions, useApp } from "../state/app";
import { searchCache, thumbRevs, useSearchVersion } from "../state/search";
import {
  MAX_FILMSTRIP_SCROLL_PX,
  accumulateWheel,
  keepVisibleOffset,
  toRealOffset,
  toVirtualOffset,
} from "./viewerMath";
import "./viewer.css";

/**
 * 1 項目の幅の既定値（px。高さを測るまでの仮置き。--filmstrip-h の 96 px から、上の枠線 1 px と
 * base.css のスクロールバーの 10 px を除いた値）。
 */
const DEFAULT_CELL = 85;

/** 1 項目の幅の下限（px。仮置き）。 */
const MIN_CELL = 40;

/** 見えている範囲の外に余分に描く数（仮置き）。 */
const OVERSCAN = 6;

export function Filmstrip() {
  const count = useApp((s) => s.search?.count ?? 0);
  const currentIndex = useApp((s) => s.selection.currentIndex);
  const current = useApp((s) => s.selection.current);
  const ids = useApp((s) => s.selection.ids);
  // サムネイルの新しい rev が届いたら描き直す（rev は描いている項目の分だけ引く）。
  useApp((s) => s.thumbRevsVersion);
  useSearchVersion();

  const scrollRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ cell: DEFAULT_CELL, viewport: 0 });
  const { cell, viewport } = size;
  const viewportRef = useRef(viewport);
  viewportRef.current = viewport;

  // 速いスクロールの判定（グリッドと同じ）。プログラムで書いた位置のスクロールのイベントは、利用者の
  // スクロールとみなさない（programLeft）。
  const [fast, setFast] = useState(false);
  const [detector] = useState(() => new FastScrollDetector());
  const programLeft = useRef<number | null>(null);
  useEffect(() => {
    detector.onChange = setFast;
    return () => {
      detector.onChange = null;
      detector.reset();
    };
  }, [detector]);

  // 項目は正方形（高さはスクロールバーを除いた内側の高さ。スクロールバーは常に場所を取るので、件数で
  // 変わらない）。
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
      observeElementOffset(instance, (offset, isScrolling) => {
        const virtual = toVirtualOffset(offset, mapRef.current.realMax, mapRef.current.virtualMax);
        if (isScrolling) {
          // プログラムで書いた位置（丸めを許す）なら、利用者のスクロールではない。
          const program = programLeft.current;
          programLeft.current = null;
          detector.move(virtual, viewportRef.current, program === null || Math.abs(offset - program) >= 1);
        }
        cb(virtual, isScrolling);
      }),
    scrollToFn: (offset, options, instance) => {
      const left = toRealOffset(
        offset + (options.adjustments ?? 0),
        mapRef.current.realMax,
        mapRef.current.virtualMax,
      );
      programLeft.current = left;
      elementScroll(left, { behavior: options.behavior }, instance);
    },
  });

  // 項目の幅・対応が変わった: 測り直し、今のスクロールの位置を読み直させる。
  useEffect(() => {
    virtualizer.measure();
    // react-virtual はスクロールの知らせで flushSync するので、effect の中ではなく、終わった後に送る
    // （React が「lifecycle の中の flushSync」と警告するため）。
    let cancelled = false;
    queueMicrotask(() => {
      const el = scrollRef.current;
      if (cancelled || !el) return;
      programLeft.current = el.scrollLeft;
      el.dispatchEvent(new Event("scroll"));
    });
    return () => {
      cancelled = true;
    };
  }, [virtualizer, cell, realMax, virtualMax]);

  // 表示中の写真を見える位置に保つ。
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || currentIndex === null || viewport <= 0) return;
    const offset = toVirtualOffset(el.scrollLeft, realMax, virtualMax);
    const target = keepVisibleOffset({ index: currentIndex, cell, offset, viewport, count });
    if (target === null) return;
    const left = toRealOffset(target, realMax, virtualMax);
    programLeft.current = left;
    el.scrollLeft = left;
  }, [currentIndex, cell, viewport, count, realMax, virtualMax]);

  const selected = useMemo(() => new Set(ids), [ids]);

  // 縦のホイールで横に送る（内容の位置で 1 対 1 に動くよう、物理の量に直す）。スクロールの領域を抑えている
  // ときは 1 回の量が 1 px 未満になりうるので、端数を持ち越す（WebKit のスクロールの位置は整数）。
  const wheelRest = useRef(0);
  const onWheel = (e: WheelEvent<HTMLDivElement>) => {
    const el = scrollRef.current;
    if (!el || Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;
    const unit = e.deltaMode === 1 ? cell : e.deltaMode === 2 ? viewport : 1;
    const ratio = virtualMax > 0 ? realMax / virtualMax : 1;
    const { step, rest } = accumulateWheel(wheelRest.current, e.deltaY * unit * ratio);
    wheelRest.current = rest;
    if (step !== 0) el.scrollLeft += step;
  };

  const virtualOffset = virtualizer.scrollOffset ?? 0;
  const realOffset = toRealOffset(virtualOffset, realMax, virtualMax);
  const visible = virtualizer.getVirtualItems();

  // 描いている範囲（位置）を検索結果のキャッシュに知らせる。
  const viewStart = visible.length > 0 ? visible[0].index : 0;
  const viewEnd = visible.length > 0 ? visible[visible.length - 1].index + 1 : 0;
  useLayoutEffect(() => {
    searchCache.setView("filmstrip", viewStart, viewEnd);
  }, [viewStart, viewEnd]);
  useEffect(() => () => searchCache.clearView("filmstrip"), []);

  return (
    <div className="filmstrip">
      <div ref={scrollRef} className="filmstrip-scroll" onWheel={onWheel}>
        <div className="filmstrip-content" style={{ width: realTotal }}>
          {visible.map((vi) => {
            const item = filmstripItem(vi.index, fast);
            return (
              <FilmstripCell
                key={vi.key}
                index={vi.index}
                item={item}
                rev={item ? thumbRevs.of(item) : null}
                paused={fast}
                left={realOffset + (vi.start - virtualOffset)}
                size={cell}
                current={item ? item.variant_id === current : currentIndex === vi.index}
                selected={!!item && selected.has(item.variant_id)}
              />
            );
          })}
        </div>
      </div>
      {count === 0 && <div className="filmstrip-empty">表示する写真・動画がありません</div>}
    </div>
  );
}

/**
 * 位置 `index` の項目（表示用）。速いスクロールの間（`fast`）は読み込み済みのものだけを返し、ページを
 * 要求しない（決定事項ログ No.66）。
 */
export function filmstripItem(index: number, fast: boolean): VariantSummary | undefined {
  return fast ? searchCache.peek(index) : searchCache.get(index);
}

/** 項目（その項目の値だけを受け取る。サムネイルの rev などが変わった項目だけを描き直す）。 */
export const FilmstripCell = memo(function FilmstripCell(props: {
  index: number;
  /** 項目（読み込み中・速いスクロールの間に読み込んでいなければ undefined）。 */
  item: VariantSummary | undefined;
  rev: string | null;
  /** 速いスクロールの間（サムネイルを新しく要求しない）。 */
  paused: boolean;
  left: number;
  size: number;
  current: boolean;
  selected: boolean;
}) {
  const v = props.item;
  const [failed, setFailed] = useState<string | null>(null);
  // rev がなければサムネイルはまだない（要求しない。できたら thumbnail_updated で rev が来る）。速いスクロールの
  // 間は新しく要求せず、この写真に出していたものだけを出し続ける（項目は位置で作るので、写真を確かめる）。
  const shown = useRef<{ id: VariantId; url: string | null } | null>(null);
  if (v && !props.paused) {
    shown.current = { id: v.variant_id, url: props.rev ? thumbUrl(v.variant_id, props.rev) : null };
  }
  const url = v && shown.current?.id === v.variant_id ? shown.current.url : null;

  const onClick = (e: MouseEvent) => {
    // 修飾キー付きのクリックで表示中の写真を移さない（現像の画面が開き直し、同期のコピー元が入れ替わるため）。
    // 出していた写真を渡す（新しい世代のページが届くまでは古い世代の項目を出すので、選ぶときに確かめる）。
    void actions.selectIndex(props.index, {
      extend: e.shiftKey,
      toggle: e.ctrlKey || e.metaKey,
      keepCurrent: true,
      expect: e.shiftKey ? undefined : v?.variant_id,
    });
  };

  const cls = ["filmstrip-cell", props.current ? "current" : "", props.selected ? "selected" : ""].join(" ");
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
});
