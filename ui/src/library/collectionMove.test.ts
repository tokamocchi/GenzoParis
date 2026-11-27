import { describe, expect, it } from "vitest";

import { planCollectionMove } from "./collectionLogic";
import { COLLECTION_SORT_OPTION, SORT_OPTIONS, sortOptions } from "./filterLogic";

/** 画面の並び（位置 → 写真）。 */
const shown = [10, 20, 30, 40, 50];
const idAt = (i: number): number | undefined => shown[i];

/** 昇順の画面の並びに、コアの並べ替え（ids を before の前へ。null なら最後へ）を当てた結果。 */
function apply(order: number[], ids: number[], before: number | null): number[] {
  const rest = order.filter((v) => !ids.includes(v));
  const at = before === null ? rest.length : rest.indexOf(before);
  return [...rest.slice(0, at), ...ids, ...rest.slice(at)];
}

/** 画面の上で動かした後の、画面の並び。 */
function moved(move: Parameters<typeof planCollectionMove>[0], indexes: number[], descending: boolean): number[] | null {
  const plan = planCollectionMove(move, indexes, idAt, shown.length, descending);
  if (plan === null) return null;
  // 降順の画面の並びは、コレクションの順の逆。
  const order = descending ? [...shown].reverse() : shown;
  const next = apply(order, plan.ids, plan.before);
  return descending ? next.reverse() : next;
}

describe("手動のコレクションの中の並べ替え（LIB-10）", () => {
  it("昇順: 先頭・1 つ前・1 つ後ろ・最後へ移す", () => {
    expect(moved("first", [2], false)).toEqual([30, 10, 20, 40, 50]);
    expect(moved("up", [2], false)).toEqual([10, 30, 20, 40, 50]);
    expect(moved("down", [2], false)).toEqual([10, 20, 40, 30, 50]);
    expect(moved("last", [2], false)).toEqual([10, 20, 40, 50, 30]);
    // 最後から 2 番目を 1 つ後ろへ（最後へ）。
    expect(moved("down", [3], false)).toEqual([10, 20, 30, 50, 40]);
  });

  it("降順でも、画面の上の向きのとおりに動く", () => {
    expect(moved("first", [2], true)).toEqual([30, 10, 20, 40, 50]);
    expect(moved("up", [2], true)).toEqual([10, 30, 20, 40, 50]);
    expect(moved("down", [2], true)).toEqual([10, 20, 40, 30, 50]);
    expect(moved("last", [2], true)).toEqual([10, 20, 40, 50, 30]);
    expect(moved("up", [1], true)).toEqual([20, 10, 30, 40, 50]);
    expect(moved("down", [3], true)).toEqual([10, 20, 30, 50, 40]);
  });

  it("複数の写真は、並びを保ったままひとかたまりで動く", () => {
    expect(moved("up", [2, 3], false)).toEqual([10, 30, 40, 20, 50]);
    expect(moved("down", [1, 2], false)).toEqual([10, 40, 20, 30, 50]);
    // 離れた写真は、動かした先でひとかたまりになる。
    expect(moved("first", [1, 3], false)).toEqual([20, 40, 10, 30, 50]);
    expect(moved("last", [0, 2], true)).toEqual([20, 40, 50, 10, 30]);
    // 位置の順・重なりは問わない。
    expect(moved("up", [3, 2, 3], false)).toEqual([10, 30, 40, 20, 50]);
  });

  it("端にあって動かせないときは null", () => {
    expect(moved("first", [0], false)).toBeNull();
    expect(moved("up", [0, 1], false)).toBeNull();
    expect(moved("last", [3, 4], true)).toBeNull();
    expect(moved("down", [4], false)).toBeNull();
    expect(moved("up", [], false)).toBeNull();
    // 先頭を含んでいても、離れていれば先頭へまとめられる。
    expect(moved("first", [0, 2], false)).toEqual([10, 30, 20, 40, 50]);
  });

  it("隣の写真を読み込んでいなければ null", () => {
    const partial = (i: number) => (i === 1 ? undefined : shown[i]);
    expect(planCollectionMove("up", [2], partial, shown.length, false)).toBeNull();
    expect(planCollectionMove("up", [1], partial, shown.length, false)).toBeNull();
  });
});

describe("並べ替えの項目の選択肢", () => {
  it("コレクションを表示している間だけ「コレクションの順」を足す", () => {
    expect(sortOptions(false, "capture_time")).toEqual(SORT_OPTIONS);
    expect(sortOptions(true, "capture_time").at(-1)).toEqual(COLLECTION_SORT_OPTION);
    // すでに選んでいれば、コレクションを離れても選択肢に残す（表示が空欄にならないように）。
    expect(sortOptions(false, "collection_order")).toContainEqual(COLLECTION_SORT_OPTION);
  });
});
