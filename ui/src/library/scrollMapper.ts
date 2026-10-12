// グリッドのスクロールの位置（物理: 要素の scrollTop）と内容の位置（仮想: 行の上端の座標）の対応を保つ。
//
// - 内容の高さがスクロールの領域の上限（gridMath の MAX_SCROLL_PX）を超えるときは、物理と仮想を比例させる
//   （gridMath の toVirtual・toPhysical）。超えないときは物理と仮想は同じ。
// - 縮めているときは、ホイールの量をそのまま物理の位置に足すと何倍も速く動くので、ホイールを受けて
//   仮想の位置に足す（物理の位置は書き直す）。
// - 自分で書いた物理の位置のスクロールのイベントでは、仮想の位置（小数を含む）をそのまま保つ
//   （物理の位置の丸めで、見せたいセルの位置がずれないように）。
// - @tanstack/react-virtual の observeElementOffset・scrollToFn に、この位置（仮想）を渡す（Grid.tsx）。

import {
  type GridLayout,
  clampVirtual,
  gridLayout,
  isFastScroll,
  isScaled,
  toPhysical,
  toVirtual,
  wheelPixels,
} from "./gridMath";

/** スクロールの要素のうち、ここで使うもの（テストでは置き換える）。 */
export interface ScrollElement {
  scrollTop: number;
  addEventListener(type: string, listener: (e: Event) => void, options?: AddEventListenerOptions): void;
  removeEventListener(type: string, listener: (e: Event) => void): void;
}

/** スクロールが止まったとみなすまでの時間（ms。react-virtual の isScrollingResetDelay の既定値と同じ）。 */
export const SCROLL_IDLE_MS = 150;

export class ScrollMapper {
  layout: GridLayout = gridLayout(0, 0, 1);
  viewHeight = 0;
  /** 内容の位置（仮想。小数を保つ）。 */
  virtual = 0;
  /** 速いスクロールが始まった・終わったとき。 */
  onFastChange: ((fast: boolean) => void) | null = null;
  /** 位置が変わったとき（react-virtual に知らせる前。行の表示のずれを直す）。 */
  onMove: (() => void) | null = null;

  private el: ScrollElement | null = null;
  private written: number | null = null;
  private listener: ((virtual: number, scrolling: boolean) => void) | null = null;
  private idleTimer: ReturnType<typeof setTimeout> | null = null;
  private last = { time: Number.NEGATIVE_INFINITY, virtual: 0 };
  private fast = false;
  private now: () => number;

  constructor(now: () => number = () => performance.now()) {
    this.now = now;
  }

  /** 今の物理の位置（仮想の位置から求めたもの）。 */
  physical(): number {
    return toPhysical(this.virtual, this.layout, this.viewHeight);
  }

  /**
   * 行の上端（仮想）を、スクロールの領域の中の位置（物理）にするときに足す量。要素の実際の scrollTop
   * （ブラウザが丸めた値）を使い、表示の中の位置が仮想の位置のとおりになるようにする。
   */
  shift(): number {
    return (this.el ? this.el.scrollTop : this.physical()) - this.virtual;
  }

  /** 要素のスクロールとホイールを受け取る（react-virtual の observeElementOffset。戻り値で解除）。 */
  attach(el: ScrollElement, listener: (virtual: number, scrolling: boolean) => void): () => void {
    this.release?.();
    this.el = el;
    this.listener = listener;
    const onScroll = () => this.handleScroll();
    const onWheel = (e: Event) => this.handleWheel(e as WheelEvent);
    el.addEventListener("scroll", onScroll, { passive: true });
    el.addEventListener("wheel", onWheel, { passive: false });
    const release = () => {
      el.removeEventListener("scroll", onScroll);
      el.removeEventListener("wheel", onWheel);
      if (this.release !== release) return;
      this.release = null;
      this.el = null;
      this.listener = null;
      this.written = null;
      if (this.idleTimer !== null) clearTimeout(this.idleTimer);
      this.idleTimer = null;
      this.setFast(false);
    };
    this.release = release;
    return release;
  }

  private release: (() => void) | null = null;

  /** 内容の位置（仮想）へ動かす（物理の位置を書き、知らせる）。 */
  scrollTo(virtual: number): void {
    this.virtual = clampVirtual(virtual, this.layout, this.viewHeight);
    const el = this.el;
    if (el) {
      const phys = this.physical();
      if (Math.abs(el.scrollTop - phys) >= 0.5) {
        this.written = phys;
        el.scrollTop = phys;
      }
    }
    this.emit();
  }

  /** 配置・表示の高さが変わった後に、物理の位置を仮想の位置に合わせ直す（知らせる）。 */
  resync(): void {
    this.scrollTo(this.virtual);
  }

  private handleScroll(): void {
    const el = this.el;
    if (!el) return;
    const phys = el.scrollTop;
    if (this.written === null || Math.abs(phys - this.written) >= 1) {
      this.virtual = clampVirtual(toVirtual(phys, this.layout, this.viewHeight), this.layout, this.viewHeight);
    }
    this.written = null;
    this.emit();
  }

  private handleWheel(e: WheelEvent): void {
    // Ctrl ＋ホイールは拡大縮小（ブラウザ）。横のスクロールは扱わない。
    if (!isScaled(this.layout) || e.ctrlKey || e.deltaY === 0) return;
    e.preventDefault();
    this.scrollTo(this.virtual + wheelPixels(e.deltaY, e.deltaMode, this.viewHeight));
  }

  private emit(): void {
    const t = this.now();
    const distance = Math.abs(this.virtual - this.last.virtual);
    const elapsed = t - this.last.time;
    this.last = { time: t, virtual: this.virtual };
    // 続けてスクロールしている間だけ速さを見る（止まった後の 1 回の移動（Home・End など）は速いとみなさない）。
    if (elapsed < SCROLL_IDLE_MS && isFastScroll(distance, elapsed, this.viewHeight)) this.setFast(true);
    this.onMove?.();
    this.listener?.(this.virtual, true);
    if (this.idleTimer !== null) clearTimeout(this.idleTimer);
    this.idleTimer = setTimeout(() => {
      this.idleTimer = null;
      this.setFast(false);
      this.listener?.(this.virtual, false);
    }, SCROLL_IDLE_MS);
  }

  private setFast(fast: boolean): void {
    if (this.fast === fast) return;
    this.fast = fast;
    this.onFastChange?.(fast);
  }
}
