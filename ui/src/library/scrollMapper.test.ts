import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { gridLayout, MAX_SCROLL_PX, toPhysical } from "./gridMath";
import { SCROLL_IDLE_MS, ScrollMapper, type ScrollElement } from "./scrollMapper";

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
