// 小さな状態の入れ物（React の useSyncExternalStore で購読する）。

import { useRef, useSyncExternalStore } from "react";

export interface Store<S> {
  get(): S;
  set(patch: Partial<S> | ((s: S) => Partial<S>)): void;
  subscribe(listener: () => void): () => void;
}

export function createStore<S extends object>(initial: S): Store<S> {
  let state = initial;
  const listeners = new Set<() => void>();
  return {
    get: () => state,
    set(patch) {
      const p = typeof patch === "function" ? patch(state) : patch;
      state = { ...state, ...p };
      for (const l of [...listeners]) l();
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

/** 状態の一部を購読する（`equal` で同じとみなせば再描画しない）。 */
export function useStore<S extends object, T>(
  store: Store<S>,
  selector: (s: S) => T,
  equal: (a: T, b: T) => boolean = Object.is,
): T {
  // selector は props に依存しうるので、描画ごとに最新のものを使う。
  const selectorRef = useRef(selector);
  selectorRef.current = selector;
  const equalRef = useRef(equal);
  equalRef.current = equal;
  const last = useRef<{ value: T } | null>(null);
  const getSnapshot = () => {
    const next = selectorRef.current(store.get());
    if (last.current && equalRef.current(last.current.value, next)) return last.current.value;
    last.current = { value: next };
    return next;
  };
  return useSyncExternalStore(store.subscribe, getSnapshot, getSnapshot);
}

/** 配列・オブジェクトの浅い比較。 */
export function shallowEqual<T>(a: T, b: T): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  const ka = Object.keys(a as object);
  const kb = Object.keys(b as object);
  if (ka.length !== kb.length) return false;
  for (const k of ka) {
    if (!Object.is((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k])) return false;
  }
  return true;
}
