// グリッドの配置・スクロールの位置の対応・キーボードの移動先の計算（LIB-01。DOM を使わない純粋な計算）。
//
// - 行の高さは一定（セルは正方形のサムネイルの枠＋下の帯）。列の数は幅に合わせ、セルを幅いっぱいに広げる。
// - 50 万件のカタログでは、内容の高さがブラウザの要素の高さの上限（Chromium・WebKit の LayoutUnit で
//   約 3,355 万 px）を超えうる（例: 幅 900 px・セルの大きさ 320 なら列が 2・行の高さが 469 px で、
//   25 万行 × 469 px = 1 億 1,725 万 px）。そこで、スクロールの
//   領域の高さを MAX_SCROLL_PX までに抑え、スクロールの位置（物理）と内容の位置（仮想）を比例させて
//   対応させる。上限に収まるときは物理と仮想は同じ（ふつうのスクロール）。
// - 描く行の範囲は @tanstack/react-virtual が内容の位置（仮想）から求める。react-virtual はスクロールの位置を
//   要素の scrollTop から読み、scrollTo で書くため、この対応は Grid の ScrollMapper（scrollMapper.ts）が
//   その間に入って行う（react-virtual の observeElementOffset・scrollToFn を差し替える）。

/** セルの間と外側の余白（px。仮置き）。 */
export const GRID_GAP = 6;

/** セルの下の帯（評価・フラグなど）の高さ（px。仮置き）。 */
export const CELL_FOOTER = 22;

/**
 * スクロールの領域の高さの上限（px。仮置き）。Chromium・WebKit の要素の高さの上限（約 3,355 万 px）より
 * 十分小さくする。
 */
export const MAX_SCROLL_PX = 8_000_000;

/** セルの大きさ（サムネイルの枠の幅の目安。px）の範囲と既定値（仮置き。サムネイルは長辺 320）。 */
export const CELL_SIZE = { min: 96, max: 320, default: 160, step: 8 } as const;

export interface GridLayout {
  /** 列の数（1 以上）。 */
  cols: number;
  /** 行の数。 */
  rows: number;
  /** セルの幅（px）。 */
  cellWidth: number;
  /** セルの高さ（px。サムネイルの枠＋下の帯）。 */
  cellHeight: number;
  /** 行の間隔（セルの高さ＋余白。px）。 */
  rowHeight: number;
  /** 内容の高さ（仮想。px）。 */
  contentHeight: number;
  /** スクロールの領域の高さ（物理。px。MAX_SCROLL_PX まで）。 */
  scrollHeight: number;
}

/** 幅と件数とセルの大きさの目安から配置を決める。 */
export function gridLayout(width: number, count: number, targetSize: number): GridLayout {
  const w = Math.max(0, Math.floor(width));
  const size = Math.max(1, targetSize);
  const cols = Math.max(1, Math.floor((w - GRID_GAP) / (size + GRID_GAP)));
  const cellWidth = Math.max(16, Math.floor((w - GRID_GAP) / cols) - GRID_GAP);
  const cellHeight = cellWidth + CELL_FOOTER;
  const rowHeight = cellHeight + GRID_GAP;
  const rows = count > 0 ? Math.ceil(count / cols) : 0;
  const contentHeight = rows > 0 ? GRID_GAP + rows * rowHeight : 0;
  return {
    cols,
    rows,
    cellWidth,
    cellHeight,
    rowHeight,
    contentHeight,
    scrollHeight: Math.min(contentHeight, MAX_SCROLL_PX),
  };
}

/** 行の上端（仮想。px）。 */
export function rowTop(layout: GridLayout, row: number): number {
  return GRID_GAP + row * layout.rowHeight;
}

/** スクロールの位置を縮めているか（内容が上限を超える）。 */
export function isScaled(layout: GridLayout): boolean {
  return layout.contentHeight > layout.scrollHeight;
}

/** スクロールの位置（物理）→ 内容の位置（仮想）。 */
export function toVirtual(scrollTop: number, layout: GridLayout, viewHeight: number): number {
  if (!isScaled(layout)) return scrollTop;
  const maxPhys = layout.scrollHeight - viewHeight;
  const maxVirt = layout.contentHeight - viewHeight;
  if (maxPhys <= 0 || maxVirt <= 0) return 0;
  return (Math.min(Math.max(0, scrollTop), maxPhys) / maxPhys) * maxVirt;
}

/** 内容の位置（仮想）→ スクロールの位置（物理）。 */
export function toPhysical(virtual: number, layout: GridLayout, viewHeight: number): number {
  const maxVirt = maxVirtual(layout, viewHeight);
  const v = Math.min(Math.max(0, virtual), maxVirt);
  if (!isScaled(layout)) return v;
  const maxPhys = layout.scrollHeight - viewHeight;
  if (maxPhys <= 0 || maxVirt <= 0) return 0;
  return (v / maxVirt) * maxPhys;
}

/**
 * 位置 `index` のセルを見えるようにする内容の位置（仮想）。既に全体が見えていれば null。
 * `center` なら真ん中に置く（グリッドを開き直したときなど）。
 */
export function revealOffset(
  index: number,
  layout: GridLayout,
  viewHeight: number,
  virtual: number,
  center = false,
): number | null {
  if (layout.rows === 0 || index < 0) return null;
  const row = Math.min(layout.rows - 1, Math.floor(index / layout.cols));
  const top = rowTop(layout, row);
  const bottom = top + layout.cellHeight;
  const clamp = (v: number) => clampVirtual(v, layout, viewHeight);
  if (center) {
    const target = clamp(top - (viewHeight - layout.cellHeight) / 2);
    return Math.abs(target - virtual) < 1 ? null : target;
  }
  if (top - GRID_GAP < virtual) return clamp(top - GRID_GAP);
  if (bottom + GRID_GAP > virtual + viewHeight) {
    // 表示の高さよりセルが高いときは、上端を合わせる。
    if (layout.cellHeight + 2 * GRID_GAP > viewHeight) return clamp(top - GRID_GAP);
    return clamp(bottom + GRID_GAP - viewHeight);
  }
  return null;
}

/** 内容の位置（仮想）の上限。 */
export function maxVirtual(layout: GridLayout, viewHeight: number): number {
  return Math.max(0, layout.contentHeight - viewHeight);
}

/** 内容の位置（仮想）を範囲に収める。 */
export function clampVirtual(virtual: number, layout: GridLayout, viewHeight: number): number {
  const v = Number.isFinite(virtual) ? virtual : 0;
  return Math.min(maxVirtual(layout, viewHeight), Math.max(0, v));
}

/** 表示の上端にかかっている行の先頭のセルの位置（件数が 0 なら null）。 */
export function firstVisibleIndex(layout: GridLayout, virtual: number, count: number): number | null {
  if (layout.rows === 0 || count <= 0) return null;
  const row = Math.floor((Math.max(0, virtual) - GRID_GAP + layout.rowHeight - layout.cellHeight) / layout.rowHeight);
  const r = Math.min(layout.rows - 1, Math.max(0, row));
  return Math.min(count - 1, r * layout.cols);
}

/** 位置 `index` のセルの全体が見えているか。 */
export function isFullyVisible(layout: GridLayout, index: number, virtual: number, viewHeight: number): boolean {
  if (layout.rows === 0 || index < 0) return false;
  const top = rowTop(layout, Math.floor(index / layout.cols));
  return top >= virtual && top + layout.cellHeight <= virtual + viewHeight;
}

/**
 * 配置が変わったとき（幅・セルの大きさ）に、`anchor` のセルが表示の中の同じあたりに来る内容の位置（仮想）。
 * セルが大きくなって収まらなくなるときは、セルの全体（下の余白まで）が見えるようにずらす。
 */
export function anchoredOffset(
  prev: GridLayout,
  next: GridLayout,
  virtual: number,
  viewHeight: number,
  anchor: number,
): number {
  if (next.rows === 0 || anchor < 0) return 0;
  const oldTop = rowTop(prev, Math.floor(anchor / prev.cols));
  const newTop = rowTop(next, Math.min(next.rows - 1, Math.floor(anchor / next.cols)));
  const maxRel = Math.max(0, viewHeight - next.cellHeight - GRID_GAP);
  const rel = Math.min(maxRel, Math.max(0, oldTop - virtual));
  return clampVirtual(newTop - rel, next, viewHeight);
}

/** ホイールの量（WheelEvent の deltaY・deltaMode）を px にする（行は 40 px・ページは表示の高さとみなす。仮置き）。 */
export function wheelPixels(deltaY: number, deltaMode: number, viewHeight: number): number {
  if (deltaMode === 1) return deltaY * 40;
  if (deltaMode === 2) return deltaY * Math.max(1, viewHeight);
  return deltaY;
}

/**
 * 速いスクロール（スクロールバーのつまみのドラッグなど）か。表示の高さを 80 ms より短い間に動いたら速いと
 * みなす（仮置き）。速い間は、検索結果のページとサムネイルを要求しない（通り過ぎるだけのセルの要求で、
 * 止まった先の表示が遅れないように。LIB-01 の読み込みの優先度）。
 */
export function isFastScroll(distance: number, elapsedMs: number, viewHeight: number): boolean {
  if (viewHeight <= 0 || distance <= 0) return false;
  return distance * 80 > viewHeight * Math.max(1, elapsedMs);
}

/** 1 画面に収まる行の数（PageUp・PageDown の移動量。1 以上）。 */
export function pageRows(layout: GridLayout, viewHeight: number): number {
  return Math.max(1, Math.floor(viewHeight / layout.rowHeight));
}

/** グリッドのキーボードの移動。 */
export type NavKey =
  | "ArrowLeft"
  | "ArrowRight"
  | "ArrowUp"
  | "ArrowDown"
  | "Home"
  | "End"
  | "PageUp"
  | "PageDown";

/**
 * キーで移る先の位置（動かないなら null）。
 *
 * - ←→ は前後の 1 件、↑↓ は列の数だけ。↓ で次の行が短く、真下にセルがなければ最後のセルへ。
 * - 選択がない（`current` が null）ときは、End は最後、それ以外は先頭を選ぶ。
 */
export function navigate(
  key: NavKey,
  current: number | null,
  count: number,
  cols: number,
  pageSize: number,
): number | null {
  if (count <= 0) return null;
  const last = count - 1;
  if (current === null || current < 0 || current > last) {
    return key === "End" ? last : 0;
  }
  const c = Math.max(1, cols);
  let next: number;
  switch (key) {
    case "ArrowLeft":
      next = Math.max(0, current - 1);
      break;
    case "ArrowRight":
      next = Math.min(last, current + 1);
      break;
    case "ArrowUp":
      next = current - c >= 0 ? current - c : current;
      break;
    case "ArrowDown":
      if (current + c <= last) next = current + c;
      else if (Math.floor(current / c) < Math.floor(last / c)) next = last;
      else next = current;
      break;
    case "Home":
      next = 0;
      break;
    case "End":
      next = last;
      break;
    case "PageUp":
      next = Math.max(0, current - Math.max(1, pageSize) * c);
      break;
    case "PageDown":
      next = Math.min(last, current + Math.max(1, pageSize) * c);
      break;
  }
  return next === current ? null : next;
}
