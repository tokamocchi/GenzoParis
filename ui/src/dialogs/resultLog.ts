// 処理の結果の記録（書き出し・削除などの結果の詳細）。
//
// 通知は少したつと消え、長い詳細は短くして出す（dialogFormat.ts の shortenDetail）。全体は、上の帯の
// ジョブの表示（JobsIndicator）の「結果」から、後で見直し・コピーできるようにする。記録はこの画面の
// メモリだけに持つ（アプリを終了する・カタログを閉じると消える）。

import { actions } from "../state/app";
import { createStore, useStore } from "../state/store";
import { type Notice, shortenDetail } from "./dialogFormat";

export interface ResultEntry {
  id: number;
  /** 記録した時刻（Date.now()）。 */
  at: number;
  level: Notice["level"];
  message: string;
  detail: string;
}

/** 残す件数（仮置き: 10 件。古いものから捨てる）。 */
export const MAX_RESULTS = 10;

const store = createStore<{ entries: ResultEntry[] }>({ entries: [] });
let seq = 0;

/** 結果を記録する（詳細がなければ記録しない）。 */
export function recordResult(n: Notice, now = Date.now()): void {
  if (!n.detail) return;
  const entry: ResultEntry = { id: ++seq, at: now, level: n.level, message: n.message, detail: n.detail };
  store.set((s) => ({ entries: [entry, ...s.entries].slice(0, MAX_RESULTS) }));
}

/** 結果を通知し、詳細があれば記録する（通知の詳細は短くする）。 */
export function notifyResult(n: Notice): void {
  recordResult(n);
  actions.notify(n.level, n.message, shortenDetail(n.detail));
}

/** 1 件を消す。 */
export function removeResult(id: number): void {
  store.set((s) => ({ entries: s.entries.filter((e) => e.id !== id) }));
}

/** すべて消す（カタログを閉じたときなど）。 */
export function clearResults(): void {
  if (store.get().entries.length > 0) store.set({ entries: [] });
}

/** 今の記録（新しい順）。 */
export function resultEntries(): ResultEntry[] {
  return store.get().entries;
}

/** 記録を購読する（新しい順）。 */
export function useResults(): ResultEntry[] {
  return useStore(store, (s) => s.entries);
}
