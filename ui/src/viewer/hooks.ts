// ルーペ・フィルムストリップで使う React のフック（DOM の大きさ・コアのイベント）。

import { type RefObject, useEffect, useLayoutEffect, useRef, useState } from "react";

import { onCoreEvent } from "../api/events";
import type { CoreEvent } from "../api/types";

/** 要素の大きさとページの中の位置（CSS の画素）と devicePixelRatio。 */
export interface ViewRect {
  width: number;
  height: number;
  left: number;
  top: number;
  dpr: number;
}

function currentDpr(): number {
  return typeof window !== "undefined" && window.devicePixelRatio > 0 ? window.devicePixelRatio : 1;
}

function sameRect(a: ViewRect, b: ViewRect): boolean {
  return a.width === b.width && a.height === b.height && a.left === b.left && a.top === b.top && a.dpr === b.dpr;
}

/**
 * 要素の大きさ・位置と devicePixelRatio を追う（大きさの変化・ウィンドウの大きさの変化・モニター間の移動で
 * devicePixelRatio が変わったとき）。
 */
export function useViewRect(ref: RefObject<HTMLElement | null>): ViewRect {
  const [rect, setRect] = useState<ViewRect>(() => ({ width: 0, height: 0, left: 0, top: 0, dpr: currentDpr() }));
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => {
      const r = el.getBoundingClientRect();
      const next = { width: r.width, height: r.height, left: r.left, top: r.top, dpr: currentDpr() };
      setRect((prev) => (sameRect(prev, next) ? prev : next));
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    window.addEventListener("resize", update);
    // devicePixelRatio の変化（別のモニターへの移動・表示倍率の変更）。値ごとに問い合わせを作り直す。
    let mq: MediaQueryList | null = null;
    const onDprChange = () => {
      update();
      listenDpr();
    };
    const listenDpr = () => {
      mq?.removeEventListener("change", onDprChange);
      mq = window.matchMedia(`(resolution: ${currentDpr()}dppx)`);
      mq.addEventListener("change", onDprChange);
    };
    listenDpr();
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", update);
      mq?.removeEventListener("change", onDprChange);
    };
  }, [ref]);
  return rect;
}

/** コアのイベントを受け取る（描画ごとに最新の処理を使う）。 */
export function useCoreEvent(handler: (e: CoreEvent) => void): void {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    const off = onCoreEvent((e) => ref.current(e));
    return () => {
      off();
    };
  }, []);
}
