// 履歴の移動の計算（DEV-27。DOM を使わない純粋な計算）。

import type { HistoryEntryId, HistoryItem } from "../api/types";

/**
 * 今の位置（is_current）から `target` の項目まで動く回数（負は Undo の回数、正は Redo の回数、0 は
 * 動かない）。今の位置か `target` が見つからなければ null。
 *
 * `entries` はコアの history の応答（古い順）。Undo / Redo は 1 回で隣の項目へ動く（genzo-catalog の
 * step_history）。
 */
export function historySteps(entries: readonly HistoryItem[], target: HistoryEntryId): number | null {
  const cur = entries.findIndex((e) => e.is_current);
  const to = entries.findIndex((e) => e.id === target);
  if (cur < 0 || to < 0) return null;
  return to - cur;
}

/** 表示用に新しい順に並べ、今の位置より後（やり直せる項目）に印を付ける。 */
export function historyRows(
  entries: readonly HistoryItem[],
): { item: HistoryItem; current: boolean; undone: boolean }[] {
  const cur = entries.findIndex((e) => e.is_current);
  return entries.map((item, i) => ({ item, current: i === cur, undone: cur >= 0 && i > cur })).reverse();
}

/**
 * 履歴から、Undo / Redo できるか（今の位置より前・後に項目があるか）。保存が終わると後ろの項目は消える
 * （genzo-catalog の save_develop）ので、読み直した履歴で判断できる。履歴がなければ null。
 */
export function historyAvailability(
  entries: readonly HistoryItem[] | null,
): { canUndo: boolean; canRedo: boolean } | null {
  if (!entries) return null;
  const cur = entries.findIndex((e) => e.is_current);
  return { canUndo: cur > 0, canRedo: cur >= 0 && cur < entries.length - 1 };
}

/**
 * 「やり直す」を押せるか。コアの状態（can_redo。保存待ちがあれば偽）と履歴の両方がやり直せて、その後に
 * 画面で設定を変えていない（`unsaved` が偽）とき。保存待ちの変更があると、コアは Redo の前に保存して
 * やり直せる項目を消し、何もしない（ドラッグでない変更は保存まで約 1 秒かかり、その間コアの状態は古い）。
 */
export function redoAvailable(stateCanRedo: boolean, avail: { canRedo: boolean } | null, unsaved: boolean): boolean {
  return stateCanRedo && (avail ? avail.canRedo : true) && !unsaved;
}
