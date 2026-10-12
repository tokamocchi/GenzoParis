import { afterEach, describe, expect, it, vi } from "vitest";

import { watchDevicePixelRatio } from "./hooks";

/** matchMedia の問い合わせの代わり（change の処理を覚えておき、呼び出せる）。 */
class FakeQuery {
  listeners = new Set<() => void>();
  constructor(readonly media: string) {}
  addEventListener(_type: string, fn: () => void) {
    this.listeners.add(fn);
  }
  removeEventListener(_type: string, fn: () => void) {
    this.listeners.delete(fn);
  }
  fire() {
    for (const fn of [...this.listeners]) fn();
  }
}

/** window の代わり（devicePixelRatio と matchMedia だけ）。 */
function fakeWindow(dpr: number) {
  const queries: FakeQuery[] = [];
  const win = {
    devicePixelRatio: dpr,
    matchMedia: (media: string) => {
      const q = new FakeQuery(media);
      queries.push(q);
      return q;
    },
  };
  vi.stubGlobal("window", win);
  return { win, queries };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("devicePixelRatio の見張り", () => {
  it("変わったら知らせ、新しい値で問い合わせを作り直す", () => {
    const { win, queries } = fakeWindow(1);
    const onChange = vi.fn();
    const stop = watchDevicePixelRatio(onChange);
    expect(queries.map((q) => q.media)).toEqual(["(resolution: 1dppx)"]);

    // 倍率の違うモニターへ移す。
    win.devicePixelRatio = 2;
    queries[0].fire();
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(queries.map((q) => q.media)).toEqual(["(resolution: 1dppx)", "(resolution: 2dppx)"]);
    // 古い問い合わせの見張りはやめる。
    expect(queries[0].listeners.size).toBe(0);
    expect(queries[1].listeners.size).toBe(1);

    // もう一度移す（1.5 倍）。
    win.devicePixelRatio = 1.5;
    queries[1].fire();
    expect(onChange).toHaveBeenCalledTimes(2);
    expect(queries[2].media).toBe("(resolution: 1.5dppx)");

    stop();
    expect(queries.every((q) => q.listeners.size === 0)).toBe(true);
  });
});
