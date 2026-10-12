// テスト用の Bridge（invoke・listen の記録と応答の差し替え）。

import type { Bridge, Unlisten } from "./bridge";

export interface MockCall {
  command: string;
  args?: Record<string, unknown>;
}

export class MockBridge implements Bridge {
  calls: MockCall[] = [];
  handlers = new Map<string, Set<(p: unknown) => void>>();
  /** コマンドごとの応答（関数なら引数から作る）。 */
  responses = new Map<string, unknown | ((args?: Record<string, unknown>) => unknown)>();
  /** 応答を遅らせる（ミリ秒。順序のテスト用）。 */
  delay: (call: MockCall) => number = () => 0;

  async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    const call = { command, args };
    this.calls.push(call);
    const ms = this.delay(call);
    if (ms > 0) await new Promise((r) => setTimeout(r, ms));
    const r = this.responses.get(command);
    const v = typeof r === "function" ? (r as (a?: Record<string, unknown>) => unknown)(args) : r;
    if (v instanceof Error) throw v;
    return v as T;
  }

  async invokeBinary(command: string, args?: Record<string, unknown>): Promise<ArrayBuffer> {
    return this.invoke<ArrayBuffer>(command, args);
  }

  async listen<T>(event: string, handler: (payload: T) => void): Promise<Unlisten> {
    const set = this.handlers.get(event) ?? new Set();
    set.add(handler as (p: unknown) => void);
    this.handlers.set(event, set);
    return () => set.delete(handler as (p: unknown) => void);
  }

  emit(event: string, payload: unknown): void {
    for (const h of this.handlers.get(event) ?? []) h(payload);
  }

  /** core_execute で送ったコマンドの type の並び。 */
  coreCommands(): string[] {
    return this.calls
      .filter((c) => c.command === "core_execute")
      .map((c) => (c.args?.command as { type: string }).type);
  }
}
