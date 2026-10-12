import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { gridLayout, MAX_SCROLL_PX, toPhysical } from "./gridMath";
import {
  FAST_IDLE_MS,
  FAST_RELEASE_MS,
  FastScrollDetector,
  SCROLL_IDLE_MS,
  ScrollMapper,
  type ScrollElement,
} from "./scrollMapper";

/** スクロールの要素の代わり（scrollTop の書き込みは、ブラウザと同じく整数に丸めてイベントを出す）。 */
class FakeElement implements ScrollElement {
  private top = 0;
  listeners = new Map<string, Set<(e: Event) => void>>();
  get scrollTop() {
    return this.top;
  }
  set scrollTop(v: number) {
    this.top = Math.round(v);
  }
  addEventListener(type: string, fn: (e: Event) => void) {
    const set = this.listeners.get(type) ?? new Set();
    set.add(fn);
    this.listeners.set(type, set);
  }
  removeEventListener(type: string, fn: (e: Event) => void) {
    this.listeners.get(type)?.delete(fn);
  }
  fire(type: string, e: Partial<WheelEvent> = {}) {
    for (const fn of this.listeners.get(type) ?? []) fn(e as Event);
  }
  /** 利用者のスクロール（scrollTop が変わってイベントが来る）。 */
  userScroll(top: number) {
    this.top = top;
    this.fire("scroll");
  }
}

let clock = 0;

beforeEach(() => {
  vi.useFakeTimers();
  clock = 0;
});

afterEach(() => {
  vi.useRealTimers();
});

function mapper(count: number, size: number) {
  const m = new ScrollMapper(() => clock);
  m.layout = gridLayout(900, count, size);
  m.viewHeight = 800;
  const el = new FakeElement();
  const calls: [number, boolean][] = [];
  const release = m.attach(el, (v, s) => calls.push([v, s]));
  return { m, el, calls, release };
}

describe("ScrollMapper", () => {
  it("上限に収まるときは scrollTop をそのまま位置にし、止まったら知らせる", () => {
    const { m, el, calls } = mapper(1000, 160);
    el.userScroll(1234);
    expect(m.virtual).toBe(1234);
    expect(calls).toEqual([[1234, true]]);
    vi.advanceTimersByTime(SCROLL_IDLE_MS);
    expect(calls).toEqual([
      [1234, true],
      [1234, false],
    ]);
  });

  it("縮めているときは、書いた位置の丸めで仮想の位置を変えない", () => {
    const { m, el } = mapper(500_000, 320);
    expect(m.layout.scrollHeight).toBe(MAX_SCROLL_PX);
    m.scrollTo(50_000_123.25);
    expect(m.virtual).toBe(50_000_123.25);
    expect(el.scrollTop).toBe(Math.round(toPhysical(50_000_123.25, m.layout, 800)));
    // 書いた位置のイベント（丸めた値）。
    el.fire("scroll");
    expect(m.virtual).toBe(50_000_123.25);
    // 行の位置は、丸めた実際の scrollTop から求める（表示の中の位置が仮想の位置のとおりになる）。
    expect(m.shift()).toBe(el.scrollTop - 50_000_123.25);
    // 利用者が動かしたら、物理の位置から求め直す。
    el.userScroll(el.scrollTop + 100);
    expect(m.virtual).toBeGreaterThan(50_000_123.25 + 100);
  });

  it("縮めているときはホイールを仮想の位置に足す", () => {
    const { m, el } = mapper(500_000, 320);
    const preventDefault = vi.fn();
    el.fire("wheel", { deltaY: 120, deltaMode: 0, ctrlKey: false, preventDefault });
    expect(preventDefault).toHaveBeenCalled();
    expect(m.virtual).toBe(120);
    // Ctrl ＋ホイール（拡大縮小）は扱わない。
    const pd2 = vi.fn();
    el.fire("wheel", { deltaY: 120, deltaMode: 0, ctrlKey: true, preventDefault: pd2 });
    expect(pd2).not.toHaveBeenCalled();
  });

  it("縮めていなければホイールはブラウザに任せる", () => {
    const { el } = mapper(1000, 160);
    const preventDefault = vi.fn();
    el.fire("wheel", { deltaY: 120, deltaMode: 0, ctrlKey: false, preventDefault });
    expect(preventDefault).not.toHaveBeenCalled();
  });

  it("続けて速く動いたら速いスクロールとし、止まったら戻す", () => {
    const { m, el } = mapper(500_000, 160);
    const fast: boolean[] = [];
    m.onFastChange = (f) => fast.push(f);
    // 止まった後の 1 回の大きな移動は速いとみなさない。
    clock = 1000;
    el.userScroll(1_000_000);
    expect(fast).toEqual([]);
    clock += 16;
    el.userScroll(1_002_000);
    expect(fast).toEqual([true]);
    vi.advanceTimersByTime(SCROLL_IDLE_MS);
    expect(fast).toEqual([true, false]);
  });

  it("縮めているときのホイールも、続けて速く動いたら速いスクロールとする", () => {
    const { m, el } = mapper(500_000, 320);
    const fast: boolean[] = [];
    m.onFastChange = (f) => fast.push(f);
    clock = 1000;
    el.fire("wheel", { deltaY: 100, deltaMode: 0, ctrlKey: false, preventDefault: () => {} });
    clock += 16;
    // 16 ms で 1 ページ分（表示の高さ 800）を超えて動く。
    el.fire("wheel", { deltaY: 1, deltaMode: 2, ctrlKey: false, preventDefault: () => {} });
    expect(fast).toEqual([true]);
  });

  it("プログラムからの移動（キーボードの移動など）は、スクロールの直後でも速いとみなさない", () => {
    const { m, el } = mapper(500_000, 160);
    const fast: boolean[] = [];
    m.onFastChange = (f) => fast.push(f);
    clock = 1000;
    // ホイールで 100 px 動かした 50 ms 後に、末尾へ（End）。
    el.userScroll(100);
    clock += 50;
    m.scrollTo(1e12);
    expect(fast).toEqual([]);
    // 書いた位置のスクロールのイベントでも、速いとみなさない。
    clock += 5;
    el.fire("scroll");
    expect(fast).toEqual([]);
    // PageDown の押し続け（33 ms ごとに 681 px）でも、速いとみなさない。
    m.scrollTo(0);
    for (let i = 1; i <= 10; i++) {
      clock += 33;
      m.scrollTo(i * 681);
      el.fire("scroll");
    }
    expect(fast).toEqual([]);
    // その後の利用者のスクロールは、プログラムで動かした位置から測る。
    clock += 16;
    el.userScroll(el.scrollTop + 2000);
    expect(fast).toEqual([true]);
  });

  it("速く動いた後に遅くなったら、止まるのを待たずに速いスクロールの印を外す（performance-3）", () => {
    // 縮めていない配置（物理と仮想が同じ）。表示の高さ 800 px なので、速さの閾値は 10 px/ms。
    const { m, el } = mapper(1000, 160);
    const fast: boolean[] = [];
    m.onFastChange = (f) => fast.push(f);
    clock = 1000;
    el.userScroll(0);
    clock += 16;
    el.userScroll(2000);
    expect(fast).toEqual([true]);
    // 1 px/ms（閾値の半分より遅い）で動き続ける。タイマーは進めない（止まってはいない）。
    let top = 2000;
    let t = 0;
    while (fast.length === 1 && t < 1000) {
      clock += 16;
      t += 16;
      top += 16;
      el.userScroll(top);
    }
    expect(fast).toEqual([true, false]);
    // 遅い動きが FAST_RELEASE_MS 続いたところで外す（3〜4 回のイベント）。
    expect(t).toBeLessThanOrEqual(FAST_RELEASE_MS + 16);
    // 遅くなった後も動いていれば、見えている範囲を要求できる（速い印は付かない）。
    clock += 16;
    el.userScroll(top + 16);
    expect(fast).toEqual([true, false]);
  });

  it("閾値と半分の間の速さでは、速いスクロールの印を変えない（ヒステリシス）", () => {
    const { m, el } = mapper(1000, 160);
    const fast: boolean[] = [];
    m.onFastChange = (f) => fast.push(f);
    clock = 1000;
    el.userScroll(0);
    clock += 16;
    el.userScroll(2000);
    // 7.5 px/ms（閾値 10 と半分 5 の間）で 0.5 秒動く。
    let top = 2000;
    for (let i = 0; i < 30; i++) {
      clock += 16;
      top += 120;
      el.userScroll(top);
    }
    expect(fast).toEqual([true]);
    // 印が付いていない状態から同じ速さで動いても、付けない。
    vi.advanceTimersByTime(FAST_IDLE_MS);
    expect(fast).toEqual([true, false]);
    for (let i = 0; i < 10; i++) {
      clock += 16;
      top += 120;
      el.userScroll(top);
    }
    expect(fast).toEqual([true, false]);
  });

  it("止まってから PERF-06 の予算（0.1 秒）の内側で、速いスクロールの印を外す", () => {
    const { m, el } = mapper(500_000, 160);
    const fast: boolean[] = [];
    m.onFastChange = (f) => fast.push(f);
    clock = 1000;
    el.userScroll(1_000_000);
    clock += 16;
    el.userScroll(1_002_000);
    expect(fast).toEqual([true]);
    expect(FAST_IDLE_MS).toBeLessThan(100);
    vi.advanceTimersByTime(FAST_IDLE_MS - 1);
    expect(fast).toEqual([true]);
    vi.advanceTimersByTime(1);
    expect(fast).toEqual([true, false]);
  });

  it("範囲の外へは動かさず、解除した後はイベントを受けない", () => {
    const { m, el, calls, release } = mapper(10, 160);
    m.scrollTo(1e9);
    expect(m.virtual).toBe(0); // 内容が表示より低い
    release();
    calls.length = 0;
    el.userScroll(5);
    expect(calls).toEqual([]);
    expect(el.listeners.get("scroll")?.size).toBe(0);
  });
});

describe("FastScrollDetector（フィルムストリップの横のスクロール）", () => {
  it("表示の幅を 80 ms より短い間に動いたら速いとし、プログラムからの移動は速いとみなさない", () => {
    const d = new FastScrollDetector(() => clock);
    const fast: boolean[] = [];
    d.onChange = (f) => fast.push(f);
    clock = 1000;
    d.move(0, 1500, true);
    // プログラムからの移動（表示中の写真を見える位置に保つ）は、大きくても速いとみなさない。
    clock += 16;
    d.move(1_000_000, 1500, false);
    expect(fast).toEqual([]);
    // つまみのドラッグ（50 万件では 1 px で約 3 万 px 進む）。
    clock += 16;
    d.move(1_031_000, 1500, true);
    expect(d.isFast).toBe(true);
    vi.advanceTimersByTime(FAST_IDLE_MS);
    expect(fast).toEqual([true, false]);
  });

  it("reset で速くない状態に戻す", () => {
    const d = new FastScrollDetector(() => clock);
    const fast: boolean[] = [];
    d.onChange = (f) => fast.push(f);
    clock = 1000;
    d.move(0, 800, true);
    clock += 16;
    d.move(5000, 800, true);
    d.reset();
    expect(fast).toEqual([true, false]);
    vi.advanceTimersByTime(FAST_IDLE_MS);
    expect(fast).toEqual([true, false]);
  });
});
