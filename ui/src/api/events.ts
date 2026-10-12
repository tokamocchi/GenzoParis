// コアのイベント（genzo-api の Event。Tauri のイベント名 "core-event"）と、アプリのイベント。

import { bridge, type Unlisten } from "./bridge";
import type { CloseFailure, CoreEvent } from "./types";

export const CORE_EVENT = "core-event";
export const CLOSE_FAILED_EVENT = "app-close-failed";

type Handler = (e: CoreEvent) => void;

const handlers = new Set<Handler>();
let started: Promise<Unlisten> | null = null;

/** コアのイベントの受け取りを始める（何度呼んでも 1 回だけ listen する）。 */
export function startCoreEvents(): Promise<Unlisten> {
  if (!started) {
    started = bridge().listen<CoreEvent>(CORE_EVENT, (e) => {
      for (const h of [...handlers]) {
        try {
          h(e);
        } catch (err) {
          console.error("イベントの処理で例外", err);
        }
      }
    });
  }
  return started;
}

/** コアのイベントを受け取る（戻り値で解除）。 */
export function onCoreEvent(handler: Handler): () => void {
  handlers.add(handler);
  return () => handlers.delete(handler);
}

/** テスト用: 受け取ったことにしてイベントを流す。 */
export function dispatchCoreEventForTest(e: CoreEvent): void {
  for (const h of [...handlers]) h(e);
}

/** 終了のときに保存できなかったことを受け取る。 */
export function onCloseFailed(handler: (f: CloseFailure) => void): Promise<Unlisten> {
  return bridge().listen<CloseFailure>(CLOSE_FAILED_EVENT, handler);
}
