// 写真と動画のタイムライン（ORG-01）の表示の状態: 切り替え（T）・イベントの区切りの間隔・まとまりの読み込み。
//
// - 表示の好み（タイムラインにするか・区切りの間隔）はブラウザに保存する（セルの大きさと同じ。library/prefs.ts）。
// - タイムラインは撮影日時の順のときだけ使う。切り替えて使い始めるときに、並べ替えが撮影日時の順でなければ
//   撮影日時の順（向きはそのまま）にする。後から並べ替えを変えたら、切り替えは残したまま、撮影日時の順に戻す
//   までふつうのグリッドで出す（active）。
// - まとまりは検索結果の世代ごとにコアに問い合わせる（core.timeline）。世代が変わったとき・撮影日時が変わった
//   とき（captureTimeVersion）・間隔を変えたときに読み直し、届くまでは前のまとまりで出す（行の配置が急に
//   変わらないように。件数が違えば gridMath の withTimeline が整える）。古い問い合わせの応答は捨てる。

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { SearchSort, Timeline } from "../api/types";
import { loadBoolean, loadNumber, save } from "../library/prefs";
import { DEFAULT_TIMELINE_GAP } from "../library/timelineLogic";
import { actions, appStore } from "./app";
import { createStore, useStore } from "./store";

export interface TimelineState {
  /** タイムラインにする（好み）。 */
  enabled: boolean;
  /** イベントの区切りの間隔（秒。0 はイベントで分けない）。 */
  gapSeconds: number;
  /** 最後に届いたまとまり（今の世代のものとは限らない）。 */
  data: Timeline | null;
  /** data のカタログ（カタログを切り替えたら前のカタログのまとまりを使わない）。 */
  dataCatalog: string | null;
  /** 読めなかった理由（読めたら null）。 */
  error: string | null;
}

export const timelineStore = createStore<TimelineState>({
  enabled: typeof localStorage === "undefined" ? false : loadBoolean("timeline", false),
  gapSeconds: typeof localStorage === "undefined" ? DEFAULT_TIMELINE_GAP : loadNumber("timelineGap", DEFAULT_TIMELINE_GAP, 0, 86_400),
  data: null,
  dataCatalog: null,
  error: null,
});

/** タイムラインを使えるか（切り替えが入っていて、撮影日時の順）。 */
export function timelineActive(enabled: boolean, sort: SearchSort): boolean {
  return enabled && sort.key === "capture_time";
}

/** タイムラインの表示を切り替える（使い始めるときは撮影日時の順にする）。 */
export function setTimelineEnabled(on: boolean): void {
  timelineStore.set({ enabled: on, error: null });
  save("timeline", on);
  const sort = appStore.get().sort;
  if (on && sort.key !== "capture_time") actions.setSort({ key: "capture_time", direction: sort.direction });
}

/** タイムラインの表示の切り替え（T）。 */
export function toggleTimeline(): void {
  const s = timelineStore.get();
  const active = timelineActive(s.enabled, appStore.get().sort);
  setTimelineEnabled(!active);
  actions.notify("info", active ? "グリッドの表示にしました" : "タイムラインの表示にしました（撮影日・イベントの見出し）");
}

/** イベントの区切りの間隔を変える。 */
export function setTimelineGap(seconds: number): void {
  timelineStore.set({ gapSeconds: seconds });
  save("timelineGap", seconds);
}

let seq = 0;

/**
 * 今の世代のまとまりを読む（使えないとき・世代がないときは何もしない）。応答が届くまでに次の問い合わせを
 * 始めたら、前の応答は捨てる。古い世代（stale）の応答は捨てる（新しい世代の問い合わせが続く）。
 */
export async function loadTimeline(generation: number, gapSeconds: number, catalog: string | null): Promise<void> {
  const my = ++seq;
  try {
    const t = await core.timeline(generation, gapSeconds);
    if (my !== seq) return;
    timelineStore.set({ data: t, dataCatalog: catalog, error: null });
  } catch (e) {
    if (my !== seq) return;
    const err = toApiError(e);
    if (err.info.kind === "stale") return;
    timelineStore.set({ error: err.info.message });
  }
}

/** タイムラインの状態を購読する。 */
export function useTimeline<T>(selector: (s: TimelineState) => T): T {
  return useStore(timelineStore, selector);
}

/** テスト用: 状態を初めに戻す。 */
export function resetTimelineForTest(over: Partial<TimelineState> = {}): void {
  seq++;
  timelineStore.set({
    enabled: false,
    gapSeconds: DEFAULT_TIMELINE_GAP,
    data: null,
    dataCatalog: null,
    error: null,
    ...over,
  });
}
