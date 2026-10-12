// Tauri との境界（invoke・listen）。
//
// 画面のコードは Tauri の API を直接呼ばず、ここを通す。テストとブラウザでの確認では
// `installBridge` で差し替える（src/api/mock.ts）。

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";

export type Unlisten = () => void;

export interface Bridge {
  /** アプリのコマンド（genzo-app の commands.rs）を呼ぶ。失敗は ErrorInfo などを reject する。 */
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  /** バイナリを返すコマンド（develop_frame など）を呼ぶ。 */
  invokeBinary(command: string, args?: Record<string, unknown>): Promise<ArrayBuffer>;
  /** アプリのイベントを受け取る。 */
  listen<T>(event: string, handler: (payload: T) => void): Promise<Unlisten>;
}

const tauriBridge: Bridge = {
  invoke: (command, args) => tauriInvoke(command, args),
  invokeBinary: async (command, args) => {
    const r = await tauriInvoke<ArrayBuffer | Uint8Array | number[]>(command, args);
    if (r instanceof ArrayBuffer) return r;
    if (r instanceof Uint8Array) {
      return r.buffer.slice(r.byteOffset, r.byteOffset + r.byteLength) as ArrayBuffer;
    }
    return new Uint8Array(r).buffer;
  },
  listen: async (event, handler) => {
    const un = await tauriListen(event, (e) => handler(e.payload as never));
    return un;
  },
};

let current: Bridge = tauriBridge;

/** Tauri の中で動いているか。 */
export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function bridge(): Bridge {
  return current;
}

/** テスト・ブラウザでの確認用に差し替える。 */
export function installBridge(b: Bridge): void {
  current = b;
}
