// サムネイルのグリッド（LIB-01。選別の操作の対象を選ぶ（LIB-04・05）。動画の印（VID-03））。
//
// - 検索結果（searchCache の件数。id はコアのメモリにある）を行ごとに仮想スクロールする。描く行の範囲は
//   @tanstack/react-virtual が求める。50 万件で内容の高さがブラウザの要素の高さの上限を超える場合の
//   スクロールの位置の対応は ScrollMapper が行う（gridMath.ts の冒頭）。
// - 速いスクロール（スクロールバーのつまみのドラッグなど）の間は、検索結果のページとサムネイルを要求せず
//   （読み込み済みの項目と、出しているサムネイルはそのまま出す）、遅くなるか止まってから見えている範囲だけを
//   要求する（scrollMapper.ts の FastScrollDetector）。
// - 描いている範囲を検索結果のキャッシュに知らせる（searchCache.setView。読み直し・捨てるページの判断に
//   使う）。サムネイルの新しい rev は、描いているセルの分だけ引く（thumbRevs）。
// - キー（グリッドのモードで、ダイアログが開いていないとき）: ←→ で前後、↑↓ で列の数だけ、Shift で範囲を
//   広げる（アンカーから 10,000 件まで。gridMath の MAX_RANGE）、Home・End、PageUp・PageDown、Enter で
//   ルーペ。評価・フラグなどのキーは App.tsx が持つ。
// - selection.current が変わったら、そのセルが見えるようにスクロールする。グリッドを開き直したときは
//   真ん中に置く。絞り込み・並べ替えを変えたら先頭に戻し、操作の対象が残っていればそこを見せる。

import { useVirtualizer } from "@tanstack/react-virtual";
import { memo, type MouseEvent, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import type { SearchFilter, VariantSummary } from "../api/types";
import { thumbUrl } from "../api/uri";
import { FlagMark, LabelDot, Stars } from "../components/Marks";
import { formatCaptureTime } from "../format";
import { useHotkeys } from "../keyboard";
import { actions, appStore, type Selection, useApp } from "../state/app";
import { searchCache, thumbRevs, useSearchVersion } from "../state/search";
import { activeConditionCount, clearConditions } from "./filterLogic";
import {
  anchoredOffset,
  CELL_FOOTER,
  CELL_SIZE,
  clampRangeTarget,
  firstVisibleIndex,
  GRID_GAP,
  type GridLayout,
  gridLayout,
  isFullyVisible,
  MAX_RANGE,
  type NavKey,
  navigate,
  pageRows,
  revealOffset,
} from "./gridMath";
import "./library.css";
import { extensionMark, fileStatusTitle, virtualCopyLabel } from "./metadataLogic";
import { loadNumber, save } from "./prefs";
import { ScrollMapper } from "./scrollMapper";

/** セルの帯にファイル名を出す、印を言葉で書くセルの幅（px。仮置き）。 */
const WIDE_CELL = 140;

/** クリックの後、選択の変化でスクロールしない時間（ms。選択は検索結果の読み込みを待つので少し長め。仮置き）。 */
const CLICK_REVEAL_MS = 2000;

/** スライダーで値を変えるキー（スライダーにフォーカスがあるときは、グリッドの移動に使わない）。 */
const SLIDER_KEYS = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"]);

/**
 * Shift で広げる範囲の移り先を、アンカーから上限の件数までに収める（範囲の端（操作の対象）が選択に
 * 入るように。state/app.ts の selectIndex は範囲の id を 10,000 件までしか集めない）。収めたら知らせる。
 */
function clampRange(target: number): number {
  const next = clampRangeTarget(appStore.get().selection.anchorIndex, target);
  if (next !== target) actions.notify("info", `範囲の選択は ${MAX_RANGE.toLocaleString("ja-JP")} 件までです`);
  return next;
}

/** グリッドを閉じたときの位置（内容の位置。操作の対象がないときに戻す）。 */
let savedVirtual = 0;

type Pending =
  | { kind: "mount" }
  | { kind: "filter"; generation: number | null }
  | { kind: "relocate"; selection: Selection }
  | null;

export function Grid() {
  useSearchVersion();
  const count = searchCache.count;
  const searched = useApp((s) => s.search !== null);
  const generation = useApp((s) => s.search?.generation ?? null);
  const selection = useApp((s) => s.selection);
  // サムネイルの新しい rev が届いたら描き直す（rev は描いているセルの分だけ引く）。
  useApp((s) => s.thumbRevsVersion);
  const dialogOpen = useApp((s) => s.dialog !== null);
  const filter = useApp((s) => s.filter);
  const sort = useApp((s) => s.sort);

  const [cellSize, setCellSize] = useState(() =>
    loadNumber("cellSize", CELL_SIZE.default, CELL_SIZE.min, CELL_SIZE.max),
  );
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [fast, setFast] = useState(false);
  const [mapper] = useState(() => new ScrollMapper());
  const scrollRef = useRef<HTMLDivElement>(null);
  const layerRef = useRef<HTMLDivElement>(null);
  const renderShift = useRef(0);

  const ready = size.width > 0 && size.height > 0;
  const layout = useMemo(() => gridLayout(size.width, count, cellSize), [size.width, count, cellSize]);
  // スクロールのイベントは描画の合間に来るので、最新の配置を先に渡しておく。
  mapper.layout = layout;
  mapper.viewHeight = size.height;

  // 行の高さが変わったら、react-virtual に行の位置を計算し直させる（react-virtual は estimateSize の変化を
  // 見ず、getItemKey が変わったときに計算し直す）。
  const rowHeight = layout.rowHeight;
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const getItemKey = useCallback((i: number) => i, [rowHeight]);

  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: ready ? layout.rows : 0,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => layout.rowHeight,
    getItemKey,
    paddingStart: GRID_GAP,
    overscan: 2,
    // 位置を直すのは描画の後（useLayoutEffect）なので、flushSync を使わない。
    useFlushSync: false,
    initialOffset: () => mapper.virtual,
    observeElementRect: (instance, cb) => {
      const el = instance.scrollElement;
      if (!el) return;
      // スクロールバーを除いた幅と高さ（列の数と表示の高さに使う）。
      const read = () => {
        const rect = { width: el.clientWidth, height: el.clientHeight };
        mapper.viewHeight = rect.height;
        setSize((prev) => (prev.width === rect.width && prev.height === rect.height ? prev : rect));
        cb(rect);
      };
      read();
      const ro = new ResizeObserver(read);
      ro.observe(el);
      return () => ro.disconnect();
    },
    observeElementOffset: (instance, cb) =>
      instance.scrollElement ? mapper.attach(instance.scrollElement, cb) : undefined,
    scrollToFn: (offset, { adjustments }) => mapper.scrollTo(offset + (adjustments ?? 0)),
  });

  // 行の表示のずれ（縮めているときの物理と仮想の差）を、次の描画まで transform で直す。
  const applyShift = useCallback(() => {
    const layer = layerRef.current;
    if (!layer) return;
    const d = mapper.shift() - renderShift.current;
    layer.style.transform = Math.abs(d) < 0.01 ? "" : `translateY(${d}px)`;
  }, [mapper]);

  useEffect(() => {
    mapper.onFastChange = setFast;
    mapper.onMove = applyShift;
    return () => {
      mapper.onFastChange = null;
      mapper.onMove = null;
    };
  }, [mapper, applyShift]);

  useLayoutEffect(() => {
    applyShift();
  });

  const reveal = useCallback(
    (index: number, center: boolean) => {
      const v = revealOffset(index, mapper.layout, mapper.viewHeight, mapper.virtual, center);
      if (v !== null) mapper.scrollTo(v);
    },
    [mapper],
  );

  // 配置（幅・セルの大きさ・件数）や表示の高さが変わったら、物理と仮想の対応を合わせ直す。列の数・行の
  // 高さが変わったら、操作の対象（見えていれば）か上端のセルを、表示の中の同じあたりに置く。
  const prevLayout = useRef<{ layout: GridLayout; height: number } | null>(null);
  useLayoutEffect(() => {
    if (!ready) return;
    const prev = prevLayout.current;
    prevLayout.current = { layout, height: size.height };
    if (!prev) return;
    if (prev.layout.cols !== layout.cols || prev.layout.rowHeight !== layout.rowHeight) {
      const cur = appStore.get().selection.currentIndex;
      const anchor =
        cur !== null && isFullyVisible(prev.layout, cur, mapper.virtual, prev.height)
          ? cur
          : firstVisibleIndex(prev.layout, mapper.virtual, count);
      mapper.scrollTo(anchor === null ? 0 : anchoredOffset(prev.layout, layout, mapper.virtual, size.height, anchor));
    } else if (prev.layout !== layout || prev.height !== size.height) {
      mapper.resync();
    }
  }, [layout, size.height, ready, count, mapper]);

  // 操作の対象を見せる（開き直したとき・絞り込みを変えたとき・選択が動いたとき）。
  const pending = useRef<Pending>({ kind: "mount" });
  const lastIndex = useRef<number | null>(null);
  // クリックで選んだセルにはスクロールしない（見えているため）。Ctrl / ⌘ ＋クリックで外して操作の対象が
  // 別の写真に移ったときも、表示を飛ばさない。
  const clicked = useRef<{ index: number; toggle: boolean; at: number } | null>(null);
  // 閉じるときに位置を覚える（開いた直後の位置を決める前に閉じたとき（開発時の StrictMode の試しの
  // 取り外しを含む）は、前に覚えた位置を残す）。
  useEffect(
    () => () => {
      if (pending.current?.kind !== "mount") savedVirtual = mapper.virtual;
    },
    [mapper],
  );

  // 前の値と比べる（開いたときと、開発時の StrictMode の 2 回目の実行では何もしない）。
  const lastQuery = useRef({ filter, sort });
  useLayoutEffect(() => {
    if (lastQuery.current.filter === filter && lastQuery.current.sort === sort) return;
    lastQuery.current = { filter, sort };
    pending.current = { kind: "filter", generation: appStore.get().search?.generation ?? null };
  }, [filter, sort]);

  useLayoutEffect(() => {
    if (!ready) return;
    const p = pending.current;
    if (p?.kind === "mount") {
      pending.current = null;
      if (selection.currentIndex !== null) reveal(selection.currentIndex, true);
      else mapper.scrollTo(savedVirtual);
    } else if (p?.kind === "filter") {
      // 新しい検索結果が届くまで待つ。
      if (generation === p.generation) return;
      mapper.scrollTo(0);
      pending.current = selection.current !== null ? { kind: "relocate", selection } : null;
    } else if (p?.kind === "relocate") {
      // 操作の対象の位置を求め直す（relocateSelection）まで待つ。
      if (selection === p.selection) return;
      pending.current = null;
      if (selection.currentIndex !== null) reveal(selection.currentIndex, true);
    } else if (selection.currentIndex !== null && selection.currentIndex !== lastIndex.current) {
      const c = clicked.current;
      const byClick =
        c !== null &&
        performance.now() - c.at < CLICK_REVEAL_MS &&
        (c.toggle || c.index === selection.currentIndex);
      clicked.current = null;
      if (!byClick) reveal(selection.currentIndex, false);
    }
    lastIndex.current = selection.currentIndex;
  }, [selection, generation, ready, reveal, mapper]);

  // --- キーボード ---
  const navTarget = useRef<number | null>(null);
  const go = (key: NavKey, extend: boolean) => {
    const from = navTarget.current ?? appStore.get().selection.currentIndex;
    let next = navigate(key, from, searchCache.count, layout.cols, pageRows(layout, size.height));
    if (next !== null && extend) next = clampRange(next);
    if (next === null || next === from) return;
    navTarget.current = next;
    reveal(next, false);
    void actions.selectIndex(next, extend ? { extend: true } : {}).finally(() => {
      if (navTarget.current === next) navTarget.current = null;
    });
  };
  const nav = (key: NavKey) => ({
    [key]: () => go(key, false),
    [`shift+${key}`]: () => go(key, true),
  });
  useHotkeys(
    {
      ...nav("ArrowLeft"),
      ...nav("ArrowRight"),
      ...nav("ArrowUp"),
      ...nav("ArrowDown"),
      ...nav("Home"),
      ...nav("End"),
      ...nav("PageUp"),
      ...nav("PageDown"),
      Enter: () => {
        // ボタンにフォーカスがあるときは、ボタンを押したことにする（Enter の既定の動作を止めているため）。
        const el = document.activeElement;
        if (el instanceof HTMLButtonElement || el instanceof HTMLAnchorElement) {
          el.click();
          return;
        }
        if (appStore.get().selection.current !== null) actions.setMode("loupe");
      },
    },
    ready && !dialogOpen,
  );

  // --- マウス ---
  const cellOf = (e: MouseEvent) => {
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-index]");
    if (!el) return null;
    // セルに出していた写真（新しい世代のページが届くまでは古い世代の項目を出すので、選ぶときに確かめる）。
    const id = el.dataset.id;
    return { index: Number(el.dataset.index), expect: id === undefined ? undefined : Number(id) };
  };
  const onClick = (e: MouseEvent) => {
    const c = cellOf(e);
    if (c === null) return;
    const i = c.index;
    const toggle = !e.shiftKey && (e.metaKey || e.ctrlKey);
    clicked.current = { index: i, toggle, at: performance.now() };
    if (e.shiftKey) void actions.selectIndex(clampRange(i), { extend: true });
    else if (toggle) void actions.selectIndex(i, { toggle: true, expect: c.expect });
    else void actions.selectIndex(i, { expect: c.expect });
  };
  const onDoubleClick = (e: MouseEvent) => {
    const c = cellOf(e);
    if (c === null || e.shiftKey || e.metaKey || e.ctrlKey) return;
    void actions.selectIndex(c.index, { expect: c.expect }).then(() => actions.setMode("loupe"));
  };

  const changeCellSize = (v: number) => {
    setCellSize(v);
    save("cellSize", v);
  };

  // --- 描画 ---
  const selected = useMemo(() => new Set(selection.ids), [selection.ids]);
  const items = virtualizer.getVirtualItems();
  const shift = mapper.shift();
  renderShift.current = shift;
  const wide = layout.cellWidth >= WIDE_CELL;

  // 描いている範囲（位置）を検索結果のキャッシュに知らせる。
  const viewStart = items.length > 0 ? items[0].index * layout.cols : 0;
  const viewEnd = items.length > 0 ? Math.min(count, (items[items.length - 1].index + 1) * layout.cols) : 0;
  useLayoutEffect(() => {
    searchCache.setView("grid", viewStart, viewEnd);
  }, [viewStart, viewEnd]);
  useEffect(() => () => searchCache.clearView("grid"), []);

  return (
    <div className="lib-grid">
      <div
        ref={scrollRef}
        className="lib-grid-scroll"
        aria-label="サムネイルの一覧"
        onClick={onClick}
        onDoubleClick={onDoubleClick}
      >
        <div className="lib-grid-spacer" style={{ height: layout.scrollHeight }}>
          <div ref={layerRef} className="lib-grid-layer">
            {items.map((row) => {
              const first = row.index * layout.cols;
              const end = Math.min(count, first + layout.cols);
              const cells = [];
              for (let i = first; i < end; i++) {
                // 速い間は読み込み済みの項目だけを出す（ページを要求しない）。
                const item = fast ? searchCache.peek(i) : searchCache.get(i);
                cells.push(
                  <GridCell
                    key={item ? item.variant_id : `i${i}`}
                    index={i}
                    item={item}
                    rev={item ? thumbRevs.of(item) : null}
                    paused={fast}
                    selected={item ? selected.has(item.variant_id) : false}
                    current={item ? item.variant_id === selection.current : i === selection.currentIndex}
                    width={layout.cellWidth}
                    height={layout.cellHeight}
                    wide={wide}
                  />,
                );
              }
              return (
                <div
                  key={row.key}
                  className="lib-grid-row"
                  style={{ top: row.start + shift, height: layout.cellHeight, gap: GRID_GAP, paddingLeft: layout.insetLeft }}
                >
                  {cells}
                </div>
              );
            })}
          </div>
        </div>
        {searched && count === 0 && <EmptyState filter={filter} />}
      </div>
      <div className="lib-grid-toolbar">
        <span className="spacer" />
        <label htmlFor="lib-cell-size">サムネイルの大きさ</label>
        <input
          id="lib-cell-size"
          type="range"
          min={CELL_SIZE.min}
          max={CELL_SIZE.max}
          step={CELL_SIZE.step}
          value={cellSize}
          onChange={(e) => changeCellSize(Number(e.target.value))}
          // 矢印・Home・End などはスライダーの値を変える（window のグリッドのキーまで届かないようにする）。
          onKeyDown={(e) => {
            if (SLIDER_KEYS.has(e.key)) e.stopPropagation();
          }}
          onDoubleClick={() => changeCellSize(CELL_SIZE.default)}
          title="ダブルクリックで既定の大きさに戻す"
        />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// セル
// ---------------------------------------------------------------------------

interface CellProps {
  index: number;
  /**
   * 項目（読み込み中・速いスクロールの間に読み込んでいなければ undefined。新しい世代のページが届くまでは
   * 古い世代の項目）。
   */
  item: VariantSummary | undefined;
  rev: string | null;
  /** 速いスクロールの間（サムネイルを新しく要求しない）。 */
  paused: boolean;
  selected: boolean;
  current: boolean;
  width: number;
  height: number;
  wide: boolean;
}

const GridCell = memo(function GridCell(p: CellProps) {
  const { item } = p;
  const cls = `lib-cell${p.selected ? " selected" : ""}${p.current ? " current" : ""}${item ? "" : " loading"}`;
  // セルの枠（上下 1 px ずつ）を除いた高さを、サムネイルの枠と下の帯に分ける。
  const thumbHeight = Math.max(0, p.height - CELL_FOOTER - 2);
  return (
    <div
      className={cls}
      data-index={p.index}
      data-id={item?.variant_id}
      style={{ width: p.width, height: p.height }}
      title={item ? cellTitle(item) : undefined}
    >
      <div className="lib-cell-thumb" style={{ height: thumbHeight }}>
        {item && <Thumb item={item} rev={p.rev} paused={p.paused} />}
        {item && <Badges item={item} wide={p.wide} />}
      </div>
      <div className="lib-cell-foot" style={{ height: CELL_FOOTER }}>
        {item && (
          <>
            {item.rating > 0 && <Stars rating={item.rating} />}
            <FlagMark flag={item.flag} />
            <LabelDot label={item.color_label} />
            {p.wide && <span className="name">{item.file_name}</span>}
          </>
        )}
      </div>
    </div>
  );
});

function cellTitle(item: VariantSummary): string {
  const lines = [item.file_name, formatCaptureTime(item.capture)];
  const vc = virtualCopyLabel(item);
  if (vc) lines.push(vc);
  const status = fileStatusTitle(item.file_status);
  if (status) lines.push(status);
  return lines.join("\n");
}

/**
 * サムネイル（rev がない・読めないときは、ファイル名と種類の印）。速いスクロールの間（`paused`）は新しく
 * 要求せず、既に出しているものだけを出し続ける（セルは写真ごとに作るので、出していたのはこの写真のもの）。
 */
function Thumb({ item, rev, paused }: { item: VariantSummary; rev: string | null; paused: boolean }) {
  const [failed, setFailed] = useState<string | null>(null);
  const shown = useRef<string | null>(null);
  if (!paused) shown.current = rev ? thumbUrl(item.variant_id, rev) : null;
  const src = shown.current;
  if (!src || failed === src) {
    return (
      <div className="lib-cell-placeholder">
        <span className="ext">{extensionMark(item.file_name) || (item.kind === "video" ? "動画" : "写真")}</span>
        <span className="name">{item.file_name}</span>
      </div>
    );
  }
  return (
    <img src={src} alt="" loading="lazy" decoding="async" draggable={false} onError={() => setFailed(src)} />
  );
}

/** 動画・仮想コピー・元ファイルがないことの印。 */
function Badges({ item, wide }: { item: VariantSummary; wide: boolean }) {
  const vc = virtualCopyLabel(item);
  const status = fileStatusTitle(item.file_status);
  if (item.kind !== "video" && !vc && !status) return null;
  return (
    <div className="lib-cell-badges">
      {item.kind === "video" && (
        <span className="lib-badge" title="動画">
          {wide ? "▶ 動画" : "▶"}
        </span>
      )}
      {vc && (
        <span className="lib-badge" title={vc}>
          {wide ? "仮想コピー" : "仮想"}
        </span>
      )}
      <span className="spacer" />
      {status && (
        <span className="lib-badge danger" title={status}>
          {item.file_status === "missing" ? "!" : "×"}
        </span>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// 空の状態
// ---------------------------------------------------------------------------

function EmptyState({ filter }: { filter: SearchFilter }) {
  const conditions = activeConditionCount(filter);
  const hasFolder = filter.folder_id !== null && filter.folder_id !== undefined;
  let body;
  if (conditions > 0) {
    body = (
      <>
        <h2>条件に合う写真がありません</h2>
        <p>絞り込みの条件を変えるか、解除してください。</p>
        <div className="actions">
          <button className="btn" onClick={() => actions.replaceFilter(clearConditions(filter))}>
            条件を解除
          </button>
        </div>
      </>
    );
  } else if (hasFolder) {
    body = (
      <>
        <h2>このフォルダには写真がありません</h2>
        <div className="actions">
          {!filter.include_subfolders && (
            <button className="btn" onClick={() => actions.setFilter({ include_subfolders: true })}>
              サブフォルダを含める
            </button>
          )}
          <button className="btn" onClick={() => actions.setFilter({ folder_id: null })}>
            すべての写真を表示
          </button>
        </div>
      </>
    );
  } else {
    body = (
      <>
        <h2>カタログに写真がありません</h2>
        <p>写真や動画のフォルダを取り込むと、ここにサムネイルが並びます。</p>
        <div className="actions">
          <button className="btn primary" onClick={() => actions.openDialog({ kind: "import" })}>
            フォルダを取り込む
          </button>
        </div>
      </>
    );
  }
  return (
    <div className="lib-grid-empty">
      <div className="empty-state">{body}</div>
    </div>
  );
}
