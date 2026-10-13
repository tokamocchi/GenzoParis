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
// - スタック（LIB-12）: 検索結果はコアがスタックでまとめている（折りたたんだスタックは代表の 1 枚）。セルに
//   スタックの印（StackBadge.tsx。クリックで展開・折りたたみ）を出し、展開したスタックの写真には下端に帯を付ける。
// - 写真の右クリックのメニュー（仮想コピー・スタック・撮影日時・フォルダへの移動と名前の変更・再リンク。
//   library/stackLogic.ts の photoMenuEntries）。選んで
//   いない写真を右クリックしたら、その 1 枚を選んでから出す。下の帯の「スタック」のボタンからは、すべての
//   スタックの展開・折りたたみと、撮影時刻による自動スタック。
// - タイムライン（ORG-01。T・下の帯の「タイムライン」）: 撮影日時の順のとき、撮影日の見出しとイベント（撮影の
//   間隔で区切ったまとまり）の見出しの行をセルの行の間に入れる（まとまりはコアが作る。state/timeline.ts）。
//   見出しのクリックで、その日・イベントの写真を選ぶ。下の帯で区切りの間隔と、年・月への移動を選べる。
//   スクロールして日付の見出しが隠れたら、上端に今の日付を出す。

import { useVirtualizer } from "@tanstack/react-virtual";
import {
  memo,
  type MouseEvent,
  type PointerEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type { SearchFilter, VariantId, VariantSummary } from "../api/types";
import { thumbUrl } from "../api/uri";
import { FlagMark, LabelDot, Stars } from "../components/Marks";
import { MENU_SEPARATOR, type MenuAt, type MenuEntry, MenuButton, PopupMenu } from "../components/Menu";
import { formatCaptureTime } from "../format";
import { comboLabel, useHotkeys } from "../keyboard";
import { actions, appStore, type Selection, useApp } from "../state/app";
import { dropPhotos } from "../state/organize";
import { searchCache, thumbRevs, useSearchVersion } from "../state/search";
import { addTargetsToStack, runPhotoMenu, setAllStacksCollapsed } from "../state/stacks";
import {
  loadTimeline,
  setTimelineEnabled,
  setTimelineGap,
  timelineActive,
  toggleTimeline,
  useTimeline,
} from "../state/timeline";
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
  navigateLayout,
  pageRows,
  revealOffset,
  ROW_CELLS,
  ROW_DAY,
  rowAt,
  rowCells,
  rowSize,
  rowTop,
  withTimeline,
} from "./gridMath";
import "./library.css";
import { extensionMark, fileStatusTitle, pairLabel, virtualCopyLabel } from "./metadataLogic";
import { consumeDragClick, dragIds, watchPhotoDrag } from "./photoDrag";
import { loadNumber, save } from "./prefs";
import { ScrollMapper } from "./scrollMapper";
import { StackBadgeButton } from "./StackBadge";
import { PHOTO_MENU_SEPARATOR, photoMenuEntries } from "./stackLogic";
import {
  dayHeaderText,
  eventHeaderText,
  monthIndex,
  monthsByYear,
  TIMELINE_GAP_OPTIONS,
} from "./timelineLogic";

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

/**
 * セルのクリックで選ぶ位置と選び方: 修飾キーなしは選び直し、Ctrl / ⌘ は追加・解除、Shift は範囲（移り先は
 * アンカーから上限の件数まで。`clamp`）。どれも、セルに出していた写真 `expect` を確かめて選ぶ（新しい世代の
 * ページが届くまでは古い世代の項目を出すので、今の世代のその位置は別の写真でありうる）。範囲の上限で移り先を
 * 変えたときは、その位置の写真を選ぶ（クリックした写真ではない）。
 */
export function cellClickSelection(
  index: number,
  expect: VariantId | undefined,
  mods: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean },
  clamp: (target: number) => number,
): { index: number; opts: { extend?: boolean; toggle?: boolean; expect?: VariantId } } {
  if (mods.shiftKey) {
    const target = clamp(index);
    return { index: target, opts: { extend: true, expect: target === index ? expect : undefined } };
  }
  if (mods.metaKey || mods.ctrlKey) return { index, opts: { toggle: true, expect } };
  return { index, opts: { expect } };
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
  // タイムライン（ORG-01）。
  const captureTimeVersion = useApp((s) => s.captureTimeVersion);
  const catalogPath = useApp((s) => s.catalog?.catalog_path ?? null);
  const tlEnabled = useTimeline((t) => t.enabled);
  const tlGap = useTimeline((t) => t.gapSeconds);
  const tlData = useTimeline((t) => (t.dataCatalog === catalogPath ? t.data : null));
  const tlError = useTimeline((t) => t.error);
  const tlActive = timelineActive(tlEnabled, sort);
  useEffect(() => {
    if (tlActive && generation !== null) void loadTimeline(generation, tlGap, catalogPath);
  }, [tlActive, generation, tlGap, captureTimeVersion, catalogPath]);

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
  const layout = useMemo(() => {
    const base = gridLayout(size.width, count, cellSize);
    return tlActive && tlData ? withTimeline(base, tlData.groups, count) : base;
  }, [size.width, count, cellSize, tlActive, tlData]);
  // スクロールのイベントは描画の合間に来るので、最新の配置を先に渡しておく。
  mapper.layout = layout;
  mapper.viewHeight = size.height;

  // 行の高さ・タイムラインの行の表が変わったら、react-virtual に行の位置を計算し直させる（react-virtual は
  // estimateSize の変化を見ず、getItemKey が変わったときに計算し直す）。
  const rowHeight = layout.rowHeight;
  const rowsTable = layout.timeline;
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const getItemKey = useCallback((i: number) => i, [rowHeight, rowsTable]);

  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: ready ? layout.rows : 0,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => rowSize(layout, i),
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
    if (
      prev.layout.cols !== layout.cols ||
      prev.layout.rowHeight !== layout.rowHeight ||
      prev.layout.timeline !== layout.timeline
    ) {
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
    let next = navigateLayout(key, from, searchCache.count, layout, pageRows(layout, size.height));
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
      // タイムラインの表示の切り替え（ORG-01）。
      t: () => toggleTimeline(),
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
  // タイムラインの見出しのクリック: その日（日付の見出し）・イベントの写真を選ぶ（範囲の上限まで）。
  const selectGroupOf = (el: HTMLElement) => {
    const tl = layout.timeline;
    const g = Number(el.dataset.group);
    if (!tl || !Number.isInteger(g) || g < 0 || g >= tl.groups.length) return;
    let end = g + 1;
    if (el.dataset.day === "1") while (end < tl.groups.length && !tl.dayStart[end]) end++;
    const first = tl.groups[g].start;
    const last = tl.groups[end - 1].start + tl.groups[end - 1].count - 1;
    const to = clampRange(last);
    void (async () => {
      clicked.current = { index: first, toggle: false, at: performance.now() };
      if (!(await actions.selectIndex(first, {}))) return;
      if (to > first) {
        clicked.current = { index: to, toggle: false, at: performance.now() };
        await actions.selectIndex(to, { extend: true });
      }
    })();
  };
  const onClick = (e: MouseEvent) => {
    // 写真をドラッグした後のクリック（ボタンを離したとき）は、選択を変えない。
    if (consumeDragClick()) return;
    const head = (e.target as HTMLElement).closest<HTMLElement>("[data-group]");
    if (head) {
      selectGroupOf(head);
      return;
    }
    const c = cellOf(e);
    if (c === null) return;
    const toggle = !e.shiftKey && (e.metaKey || e.ctrlKey);
    clicked.current = { index: c.index, toggle, at: performance.now() };
    const { index, opts } = cellClickSelection(c.index, c.expect, e, clampRange);
    void actions.selectIndex(index, opts);
  };
  // 写真をつかんで動かすと、左のパネルのコレクション・キーワード・フォルダに落とせる（library/photoDrag.ts）。
  const onPointerDown = (e: PointerEvent) => {
    if (e.button !== 0 || e.shiftKey || e.metaKey || e.ctrlKey || e.altKey) return;
    const c = cellOf(e);
    if (c === null || c.expect === undefined) return;
    const grabbed = c.expect;
    watchPhotoDrag(
      { x: e.clientX, y: e.clientY },
      () => {
        const sel = appStore.get().selection;
        // 選んでいない写真をつかんだら、その 1 枚を選んでから運ぶ。
        if (!sel.ids.includes(grabbed)) void actions.selectIndex(c.index, { expect: grabbed });
        return dragIds(sel.ids, grabbed);
      },
      (ids, target) => void dropPhotos(ids, target),
    );
  };
  const onDoubleClick = (e: MouseEvent) => {
    const c = cellOf(e);
    if (c === null || e.shiftKey || e.metaKey || e.ctrlKey) return;
    // 選べなかったら（出していた写真が結果から外れたなど）、前の写真でルーペを開かない。
    void actions.openInLoupe(c.index, c.expect);
  };
  // 写真の右クリックのメニュー（選んでいない写真なら、その 1 枚を選んでから出す）。
  const [menuAt, setMenuAt] = useState<MenuAt | null>(null);
  const onContextMenu = (e: MouseEvent) => {
    const c = cellOf(e);
    if (c === null) return;
    e.preventDefault();
    const at = { x: e.clientX, y: e.clientY };
    const sel = appStore.get().selection;
    void (async () => {
      if (c.expect === undefined || !sel.ids.includes(c.expect)) {
        if (!(await actions.selectIndex(c.index, { expect: c.expect }))) return;
      }
      setMenuAt(at);
    })();
  };
  const menuItems = (): MenuEntry[] => {
    const cur = appStore.get().selection.current;
    const item = cur === null ? undefined : searchCache.getLoadedById(cur);
    return photoMenuEntries(actions.targets().length, item).map((m) =>
      m === PHOTO_MENU_SEPARATOR
        ? MENU_SEPARATOR
        : {
            label: m.label,
            hint: m.keys ? comboLabel(m.keys) : undefined,
            disabled: !m.enabled,
            onSelect: () => void runPhotoMenu(m.action),
          },
    );
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

  // 描いている範囲（位置）を検索結果のキャッシュに知らせる（タイムラインの見出しの行は飛ばす）。
  let viewStart = 0;
  let viewEnd = 0;
  for (const it of items) {
    const c = rowCells(layout, it.index, count);
    if (c) {
      viewStart = c.start;
      break;
    }
  }
  for (let k = items.length - 1; k >= 0; k--) {
    const c = rowCells(layout, items[k].index, count);
    if (c) {
      viewEnd = c.end;
      break;
    }
  }
  // タイムライン: 上端にかかっている日の見出しが隠れていれば、その日付を上に出す。
  const tl = layout.timeline;
  let sticky: string | null = null;
  if (tl && tl.count > 0) {
    const g = tl.group[rowAt(layout, mapper.virtual)];
    const dg = tl.dayGroup[g];
    if (tl.top[tl.groupFirstRow[dg]] < mapper.virtual - 1) sticky = dayHeaderText(tl.groups, tl.dayStart, dg).title;
  }
  const months = tl ? monthsByYear(monthIndex(tl.groups)) : [];
  const jumpToMonth = (key: string) => {
    if (!tl) return;
    const m = monthIndex(tl.groups).find((x) => x.key === key);
    if (!m) return;
    mapper.scrollTo(rowTop(layout, tl.groupFirstRow[m.group]) - GRID_GAP);
  };
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
        onPointerDown={onPointerDown}
        onContextMenu={onContextMenu}
      >
        <div className="lib-grid-spacer" style={{ height: layout.scrollHeight }}>
          <div ref={layerRef} className="lib-grid-layer">
            {items.map((row) => {
              if (tl && tl.kind[row.index] !== ROW_CELLS) {
                const g = tl.group[row.index];
                const day = tl.kind[row.index] === ROW_DAY;
                const text = day ? dayHeaderText(tl.groups, tl.dayStart, g) : null;
                return (
                  <div
                    key={row.key}
                    className={day ? "lib-tl-day" : "lib-tl-event"}
                    data-group={g}
                    data-day={day ? "1" : "0"}
                    title={day ? "クリックでこの日の写真を選ぶ" : "クリックでこのまとまりの写真を選ぶ"}
                    style={{ top: row.start + shift, height: rowSize(layout, row.index), paddingLeft: layout.insetLeft }}
                  >
                    {text ? (
                      <>
                        <span className="title">{text.title}</span>
                        <span className="meta">{text.meta}</span>
                      </>
                    ) : (
                      <span className="meta">{eventHeaderText(tl.groups[g])}</span>
                    )}
                  </div>
                );
              }
              const range = rowCells(layout, row.index, count);
              const first = range?.start ?? 0;
              const end = range?.end ?? 0;
              const cells = [];
              for (let i = first; i < end; i++) {
                // 速い間は読み込み済みの項目だけを出す（ページを要求しない）。
                const item = fast ? searchCache.peek(i) : searchCache.get(i);
                // key は写真（同じ写真が同じ位置に残れば、世代が変わってもセルと <img> を作り直さない）。
                // 古い世代の項目は、今の世代のページに入っている写真を出さない（search.ts の shown）ので、
                // 古い世代と新しい世代のページが同じ行に並んでも、同じ写真が 2 つのセルに出て key が重なる
                // ことはない。
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
      {sticky && (
        <div className="lib-tl-sticky" aria-hidden="true">
          {sticky}
        </div>
      )}
      {menuAt && <PopupMenu at={menuAt} items={menuItems()} onClose={() => setMenuAt(null)} label="写真の操作" />}
      <div className="lib-grid-toolbar">
        <MenuButton
          label="スタック"
          title="選んだ写真をスタックに足す・スタックの表示・撮影時刻による自動スタック"
          items={() => [
            {
              label: "選んだ写真をスタックに足す",
              disabled: actions.targets().length < 2,
              onSelect: () => void addTargetsToStack(),
            },
            MENU_SEPARATOR,
            { label: "すべてのスタックを展開", onSelect: () => void setAllStacksCollapsed(false) },
            { label: "すべてのスタックを折りたたむ", onSelect: () => void setAllStacksCollapsed(true) },
            MENU_SEPARATOR,
            { label: "撮影時刻でスタック…", onSelect: () => actions.openDialog({ kind: "auto_stack" }) },
          ]}
        >
          スタック ▾
        </MenuButton>
        <label className="lib-tl-toggle" title={sort.key === "capture_time" ? "撮影日・イベントの見出しを入れて並べる（T）" : "撮影日時の順に並べ替えて、撮影日・イベントの見出しを入れる（T）"}>
          <input type="checkbox" checked={tlActive} onChange={(e) => setTimelineEnabled(e.target.checked)} />
          タイムライン
        </label>
        {tlActive && (
          <>
            <label htmlFor="lib-tl-gap">イベントの区切り</label>
            <select
              id="lib-tl-gap"
              value={tlGap}
              onChange={(e) => setTimelineGap(Number(e.target.value))}
              title="前の写真との撮影の間隔がこれを超えたら、別のまとまりにする"
            >
              {TIMELINE_GAP_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
            {months.length > 0 && (
              <select aria-label="年・月へ移動" value="" onChange={(e) => jumpToMonth(e.target.value)}>
                <option value="">年・月へ移動…</option>
                {months.map((y) => (
                  <optgroup key={y.year} label={`${y.year} 年`}>
                    {y.months.map((m) => (
                      <option key={m.key} value={m.key}>
                        {`${m.year} 年 ${m.month} 月（${m.count.toLocaleString("ja-JP")}）`}
                      </option>
                    ))}
                  </optgroup>
                ))}
              </select>
            )}
            {tlData?.events_merged && <span className="hint">まとまりが多いため、日付だけでまとめています</span>}
            {tlError && <span className="hint error-text">タイムラインを読めません: {tlError}</span>}
          </>
        )}
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
  const stack = item?.stack ?? null;
  // 展開したスタックの写真は、下端の帯でまとまりを示す（一番上の写真は帯の左端を空ける）。
  const stackCls = stack && !stack.collapsed ? ` stack-member${stack.position === 1 ? " stack-first" : ""}` : "";
  const cls = `lib-cell${p.selected ? " selected" : ""}${p.current ? " current" : ""}${item ? "" : " loading"}${stackCls}`;
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
  const pair = pairLabel(item);
  if (pair) lines.push(pair);
  if (item.stack) {
    lines.push(
      item.stack.collapsed
        ? `スタック（${item.stack.count} 枚を折りたたみ中）`
        : `スタックの ${item.stack.position} 枚目（${item.stack.count} 枚）`,
    );
  }
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

/** 動画・仮想コピー・RAW と JPEG のペア・スタック・元ファイルがないことの印。 */
function Badges({ item, wide }: { item: VariantSummary; wide: boolean }) {
  const vc = virtualCopyLabel(item);
  const pair = pairLabel(item);
  const status = fileStatusTitle(item.file_status);
  const stack = item.stack ?? null;
  if (item.kind !== "video" && !vc && !pair && !status && !stack) return null;
  return (
    <div className="lib-cell-badges">
      {stack && <StackBadgeButton id={item.variant_id} badge={stack} wide={wide} />}
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
      {pair && (
        <span className="lib-badge" title={pair}>
          {wide ? "RAW+JPEG" : "+J"}
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
  const hasCollection = filter.collection_id !== null && filter.collection_id !== undefined;
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
  } else if (hasCollection) {
    body = (
      <>
        <h2>このコレクションには写真がありません</h2>
        <p>
          グリッドの写真を左のパネルのコレクションにドラッグするか、写真を選んで B（ターゲットコレクション）で入れられます。
          スマートコレクションは、条件に合う写真が自動で入ります。
        </p>
        <div className="actions">
          <button className="btn" onClick={() => actions.showAll()}>
            すべての写真を表示
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
