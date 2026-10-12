// グリッドのセルのクリックで選ぶ位置と選び方（M1: Shift + クリックでも、セルに出していた写真を確かめる）。

import { describe, expect, it } from "vitest";

import { cellClickSelection } from "./Grid";

const none = { shiftKey: false, ctrlKey: false, metaKey: false };
const identity = (i: number) => i;

describe("セルのクリック", () => {
  it("修飾キーなし・Ctrl / ⌘ は、出していた写真を確かめて選ぶ", () => {
    expect(cellClickSelection(10, 110, none, identity)).toEqual({ index: 10, opts: { expect: 110 } });
    expect(cellClickSelection(10, 110, { ...none, ctrlKey: true }, identity)).toEqual({
      index: 10,
      opts: { toggle: true, expect: 110 },
    });
  });

  it("Shift + クリックも、出していた写真を確かめて範囲を選ぶ", () => {
    expect(cellClickSelection(10, 110, { ...none, shiftKey: true }, identity)).toEqual({
      index: 10,
      opts: { extend: true, expect: 110 },
    });
  });

  it("範囲の上限で移り先を変えたときは、その位置の写真を選ぶ（クリックした写真を確かめない）", () => {
    expect(cellClickSelection(20_000, 20_100, { ...none, shiftKey: true }, () => 10_001)).toEqual({
      index: 10_001,
      opts: { extend: true, expect: undefined },
    });
  });
});
