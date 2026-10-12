// グリッドのスクロールの位置（物理: 要素の scrollTop）と内容の位置（仮想: 行の上端の座標）の対応を保つ。
//
// - 内容の高さがスクロールの領域の上限（gridMath の MAX_SCROLL_PX）を超えるときは、物理と仮想を比例させる
//   （gridMath の toVirtual・toPhysical）。超えないときは物理と仮想は同じ。
// - 縮めているときは、ホイールの量をそのまま物理の位置に足すと何倍も速く動くので、ホイールを受けて
//   仮想の位置に足す（物理の位置は書き直す）。
// - 自分で書いた物理の位置のスクロールのイベントでは、仮想の位置（小数を含む）をそのまま保つ
//   （物理の位置の丸めで、見せたいセルの位置がずれないように）。
// - @tanstack/react-virtual の observeElementOffset・scrollToFn に、この位置（仮想）を渡す（Grid.tsx）。
// - 速いスクロールの判定（FastScrollDetector）はフィルムストリップ（viewer/Filmstrip.tsx）でも使う。速さが
//   閾値（gridMath の isFastScroll）を超えたら印を付け、閾値の半分を FAST_RELEASE_MS 続けて下回るか、
//   止まって FAST_IDLE_MS たったら外す（決定事項ログ No.66・PERF-06）。

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

/**
 * 止まってから速いスクロールの印を外すまでの時間（ms。**仮置き**）。PERF-06（キャッシュ済みのサムネイルは
 * 表示範囲に入ってから 0.1 秒以内）の予算の半分にし、残りを検索結果のページとサムネイルの読み込みに使う。
 * スクロールのイベントは 1 フレーム（約 16 ms）ごとに来るので、動いている間に外れないよう 3 フレーム分にする。
 */
export const FAST_IDLE_MS = 50;

/**
 * 速いスクロールの印を外す遅さが続く時間（ms。**仮置き**）。速さが閾値の半分をこの時間続けて下回ったら、
 * 止まるのを待たずに外す（トラックパッドの慣性で減速していくとき・速く動かしてからゆっくり探すとき）。
 * 閾値と半分の間では印を変えない（ヒステリシス。速さの揺れで付いたり外れたりしないように）。
 */
export const FAST_RELEASE_MS = 50;

/**
 * 速いスクロールの判定（グリッド・フィルムストリップ）。位置が変わるたびに move を呼ぶ。速い間は、検索結果の
 * ページとサムネイルを要求しない（決定事項ログ No.66）。
 */
export class FastScrollDetector {
  /** 速いスクロールが始まった・終わったとき。 */
  onChange: ((fast: boolean) => void) | null = null;

  private last = { time: Number.NEGATIVE_INFINITY, pos: 0 };
  /** 閾値の半分より遅い動きが続いている始まり（続いていなければ null）。 */
  private slowSince: number | null = null;
  private fast = false;
  private idleTimer: ReturnType<typeof setTimeout> | null = null;
  private now: () => number;

  constructor(now: () => number = () => performance.now()) {
    this.now = now;
  }

  /** 速いスクロールの間か。 */
  get isFast(): boolean {
    return this.fast;
  }

  /**
   * 内容の位置 `pos`（仮想。px）に動いた。`viewSize` は表示の大きさ（スクロールの向きの px）。`user` は
   * 利用者のスクロール（スクロールのイベント・ホイール）か。
   */
  move(pos: number, viewSize: number, user: boolean): void {
    const t = this.now();
    const distance = Math.abs(pos - this.last.pos);
    const elapsed = t - this.last.time;
    this.last = { time: t, pos };
    // 利用者が続けてスクロールしている間だけ速さを見る（止まった後の 1 回の移動と、キーボードの移動など
    // プログラムからの移動は速いとみなさない）。
    if (user && elapsed < SCROLL_IDLE_MS) {
      if (isFastScroll(distance, elapsed, viewSize)) {
        this.slowSince = null;
        this.set(true);
      } else if (this.fast && !isFastScroll(distance * 2, elapsed, viewSize)) {
        // 閾値の半分より遅い（前のイベントからの間）。
        this.slowSince ??= t - elapsed;
        if (t - this.slowSince >= FAST_RELEASE_MS) this.set(false);
      } else {
        this.slowSince = null;
      }
    }
    if (!this.fast) return;
    if (this.idleTimer !== null) clearTimeout(this.idleTimer);
    this.idleTimer = setTimeout(() => {
      this.idleTimer = null;
      this.set(false);
    }, FAST_IDLE_MS);
  }

  /** 速くない状態に戻す（スクロールの要素を外したときなど）。 */
  reset(): void {
    this.last = { time: Number.NEGATIVE_INFINITY, pos: 0 };
    this.set(false);
  }

  private set(fast: boolean): void {
    if (!fast) {
      this.slowSince = null;
      if (this.idleTimer !== null) clearTimeout(this.idleTimer);
      this.idleTimer = null;
    }
    if (this.fast === fast) return;
    this.fast = fast;
    this.onChange?.(fast);
  }
}

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
  private detector: FastScrollDetector;

  constructor(now: () => number = () => performance.now()) {
    this.detector = new FastScrollDetector(now);
    this.detector.onChange = (fast) => this.onFastChange?.(fast);
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
      this.detector.reset();
    };
    this.release = release;
    return release;
  }

  private release: (() => void) | null = null;

  /**
   * 内容の位置（仮想）へ動かす（物理の位置を書き、知らせる）。`source` が "program"（既定。キーボードの
   * 移動・操作の対象を見せるときなど）なら、速いスクロールとはみなさない（PageDown を押し続けている間も
   * サムネイルを出す）。利用者のスクロール（ホイール）は "user"。
   */
  scrollTo(virtual: number, source: "program" | "user" = "program"): void {
    this.virtual = clampVirtual(virtual, this.layout, this.viewHeight);
    const el = this.el;
    if (el) {
      const phys = this.physical();
      if (Math.abs(el.scrollTop - phys) >= 0.5) {
        this.written = phys;
        el.scrollTop = phys;
      }
    }
    this.emit(source === "user");
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
    this.emit(true);
  }

  private handleWheel(e: WheelEvent): void {
    // Ctrl ＋ホイールは拡大縮小（ブラウザ）。横のスクロールは扱わない。
    if (!isScaled(this.layout) || e.ctrlKey || e.deltaY === 0) return;
    e.preventDefault();
    this.scrollTo(this.virtual + wheelPixels(e.deltaY, e.deltaMode, this.viewHeight), "user");
  }

  /** 位置を知らせる。`user` は利用者のスクロール（スクロールのイベント・ホイール）か。 */
  private emit(user: boolean): void {
    this.detector.move(this.virtual, this.viewHeight, user);
    this.onMove?.();
    this.listener?.(this.virtual, true);
    if (this.idleTimer !== null) clearTimeout(this.idleTimer);
    this.idleTimer = setTimeout(() => {
      this.idleTimer = null;
      this.listener?.(this.virtual, false);
    }, SCROLL_IDLE_MS);
  }
}
