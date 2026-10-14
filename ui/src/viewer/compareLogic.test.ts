// 比較表示・選別表示の計算（compareLogic.ts）のテスト。

import { describe, expect, it } from "vitest";

import {
  MAX_SURVEY_PHOTOS,
  clampPane,
  fitSize,
  initialPair,
  pairAfterSelection,
  paneAt,
  paneOf,
  pannedPanes,
  relativeCenter,
  removeFromSurvey,
  selectionOfPair,
  stepInList,
  stepIndex,
  surveyLayout,
  surveyPhotos,
  swapPair,
} from "./compareLogic";

describe("比較表示の組", () => {
  it("選択は表示中の写真、候補は選択中のほかの写真（選んだ順の最初）", () => {
    expect(initialPair([3, 5, 7], 5)).toEqual({ select: 5, candidate: 3 });
    expect(initialPair([5], 5)).toEqual({ select: 5, candidate: null });
    expect(initialPair([], null)).toEqual({ select: null, candidate: null });
    expect(initialPair([4, 8], null)).toEqual({ select: 4, candidate: 8 });
  });

  it("候補の送りは選択の写真を飛ばし、範囲の外では止まる", () => {
    expect(stepIndex(3, 1, 10, 4)).toBe(5);
    expect(stepIndex(5, -1, 10, 4)).toBe(3);
    expect(stepIndex(9, 1, 10, 4)).toBeNull();
    expect(stepIndex(0, -1, 10, 4)).toBeNull();
    // 候補がない（位置が分からない）ときは選択から送る。
    expect(stepIndex(null, 1, 10, 4)).toBe(5);
    expect(stepIndex(null, -1, 10, 0)).toBeNull();
    // 選択の隣が端なら送れない。
    expect(stepIndex(8, 1, 10, 9)).toBeNull();
    expect(stepIndex(null, 1, 10, null)).toBeNull();
  });

  it("入れ替えと選択の集合（作業中の面の写真が表示中）", () => {
    expect(swapPair({ select: 1, candidate: 2 })).toEqual({ select: 2, candidate: 1 });
    expect(selectionOfPair({ select: 1, candidate: 2 }, "candidate")).toEqual({ ids: [1, 2], current: 2 });
    expect(selectionOfPair({ select: 1, candidate: null }, "candidate")).toEqual({ ids: [1], current: 1 });
    expect(selectionOfPair({ select: 1, candidate: 1 }, "select")).toEqual({ ids: [1], current: 1 });
    expect(paneOf("select")).toBe(0);
    expect(paneOf("candidate")).toBe(1);
  });

  it("フィルムストリップの普通のクリックは作業中の面の写真を替え、Ctrl / ⌘ + クリックは候補にする", () => {
    const pair = { select: 1, candidate: 2 };
    // 作業中が選択の面: 選択が替わる。
    expect(pairAfterSelection(pair, "select", { ids: [7], current: 7 })).toEqual({
      pair: { select: 7, candidate: 2 },
      active: "select",
    });
    // 作業中が候補の面: 候補が替わる。
    expect(pairAfterSelection(pair, "candidate", { ids: [7], current: 7 })).toEqual({
      pair: { select: 1, candidate: 7 },
      active: "candidate",
    });
    // 表示中の写真は変えずに足した（Ctrl / ⌘ + クリック）: 最後に足した写真が候補。
    expect(pairAfterSelection(pair, "select", { ids: [1, 2, 8, 9], current: 1 })).toEqual({
      pair: { select: 1, candidate: 9 },
      active: "select",
    });
    // 表示中の写真が組の片方に移った: 作業中の面をそれに合わせる。
    expect(pairAfterSelection(pair, "select", { ids: [1, 2], current: 2 })).toEqual({ pair, active: "candidate" });
    // 変わらない・組の写真が選択から外れただけ（絞り込み）: そのまま。
    expect(pairAfterSelection(pair, "select", { ids: [1, 2], current: 1 })).toBeNull();
    expect(pairAfterSelection(pair, "select", { ids: [1], current: 1 })).toBeNull();
    // 選択の写真がなければ、選択に入れる。
    expect(pairAfterSelection({ select: null, candidate: null }, "candidate", { ids: [4], current: 4 })).toEqual({
      pair: { select: 4, candidate: null },
      active: "select",
    });
  });
});

describe("選別表示", () => {
  it("選んだ順に並べ、表示中の写真を含め、上限を超えた分は数える", () => {
    expect(surveyPhotos([3, 1, 2], 1)).toEqual({ shown: [3, 1, 2], hidden: 0 });
    expect(surveyPhotos([3, 3, 2], 2)).toEqual({ shown: [3, 2], hidden: 0 });
    expect(surveyPhotos([2], 5)).toEqual({ shown: [5, 2], hidden: 0 });
    const many = Array.from({ length: MAX_SURVEY_PHOTOS + 3 }, (_, i) => i + 1);
    const r = surveyPhotos(many, 1);
    expect(r.shown).toHaveLength(MAX_SURVEY_PHOTOS);
    expect(r.hidden).toBe(3);
    // 表示中の写真が上限の外なら、最後の枠に入れる。
    const last = many[many.length - 1];
    const r2 = surveyPhotos(many, last);
    expect(r2.shown[MAX_SURVEY_PHOTOS - 1]).toBe(last);
    expect(r2.shown).toHaveLength(MAX_SURVEY_PHOTOS);
  });

  it("枠に収めた大きさ", () => {
    expect(fitSize(1.5, 300, 300)).toEqual({ width: 300, height: 200 });
    expect(fitSize(0.5, 300, 300)).toEqual({ width: 150, height: 300 });
    expect(fitSize(0, 300, 300)).toEqual({ width: 0, height: 0 });
  });

  it("並べ方は面積の和が最も大きい列の数で、枠は内側に収まる", () => {
    // 横長 1 枚は 1 列。
    expect(surveyLayout([1.5], 900, 600, 8).cols).toBe(1);
    // 横長 2 枚: 横長の画面では横に、縦長の画面では縦に並べる。
    expect(surveyLayout([1.5, 1.5], 1600, 600, 8).cols).toBe(2);
    expect(surveyLayout([1.5, 1.5], 600, 1600, 8).cols).toBe(1);
    // 4 枚の 3:2 を 4:3 の領域に: 2 × 2。
    const l = surveyLayout([1.5, 1.5, 1.5, 1.5], 1200, 900, 8);
    expect([l.cols, l.rows]).toEqual([2, 2]);
    for (const c of l.cells) {
      expect(c.x).toBeGreaterThanOrEqual(0);
      expect(c.y).toBeGreaterThanOrEqual(0);
      expect(c.x + c.width).toBeLessThanOrEqual(1200 + 1e-9);
      expect(c.y + c.height).toBeLessThanOrEqual(900 + 1e-9);
    }
    // 3 枚を 2 列に並べたら、最後の行の 1 枚は中央に寄せる。
    const three = surveyLayout([1, 1, 1], 1000, 1000, 10);
    expect(three.cols).toBe(2);
    const bottom = three.cells[2];
    expect(bottom.x + bottom.width / 2).toBeCloseTo(500, 6);
    // 帯（写真の下の評価など）を空ける分だけ、写真の高さが減る。
    expect(surveyLayout([1.5], 900, 600, 8, 22).cells[0].height).toBeCloseTo(600 - 16, 6);
    expect(surveyLayout([], 900, 600).cells).toEqual([]);
    expect(surveyLayout([1.5], 0, 600).cells).toEqual([]);
  });

  it("作業中の写真を動かす・表示から外す", () => {
    expect(stepInList([1, 2, 3], 2, 1)).toBe(3);
    expect(stepInList([1, 2, 3], 3, 1)).toBe(3);
    expect(stepInList([1, 2, 3], 1, -1)).toBe(1);
    expect(stepInList([1, 2, 3], 9, 1)).toBe(1);
    expect(stepInList([], 1, 1)).toBeNull();
    // 作業中の写真を外すと次の写真（最後なら前の写真）が作業中になる。
    expect(removeFromSurvey([1, 2, 3], [1, 2, 3], 2, 2)).toEqual({ ids: [1, 3], current: 3 });
    expect(removeFromSurvey([1, 2, 3], [1, 2, 3], 3, 3)).toEqual({ ids: [1, 2], current: 2 });
    // 作業中でない写真を外しても、作業中の写真はそのまま。
    expect(removeFromSurvey([1, 2, 3], [1, 2, 3], 1, 3)).toEqual({ ids: [1, 2], current: 1 });
    // 1 枚だけ・並びにない写真は外さない。
    expect(removeFromSurvey([1], [1], 1, 1)).toBeNull();
    expect(removeFromSurvey([1, 2], [1, 2], 1, 9)).toBeNull();
  });
});

describe("比較表示の等倍とパン", () => {
  const view = { width: 400, height: 300 };

  it("割合の中心と、表示に合わせて収めた中心", () => {
    const p = paneAt(1, { width: 4000, height: 3000 }, { u: 0.25, v: 0.5 });
    expect([p.cx, p.cy]).toEqual([1000, 1500]);
    expect(relativeCenter(p)).toEqual({ u: 0.25, v: 0.5 });
    // 端に寄せた中心は、表示の外が見えないように収める（等倍・dpr 2 で 800 × 600 の出力の画素が見える）。
    const edge = clampPane(paneAt(1, { width: 4000, height: 3000 }, { u: 0, v: 1 }), view, 2);
    expect([edge.cx, edge.cy]).toEqual([400, 2700]);
    // 画像が表示より小さい向きは中央。
    const small = clampPane(paneAt(1, { width: 500, height: 3000 }, { u: 0.1, v: 0.5 }), view, 2);
    expect(small.cx).toBe(250);
  });

  it("同期していれば両方の面を同じ画素だけ動かし、外していればドラッグした面だけ", () => {
    const a = paneAt(1, { width: 4000, height: 3000 }, { u: 0.5, v: 0.5 });
    const b = paneAt(2, { width: 6000, height: 4000 }, { u: 0.5, v: 0.5 });
    // 右へ 50 CSS 画素（dpr 2 で 100 物理画素）ドラッグ: 中心は左へ 100 出力の画素。
    const linked = pannedPanes([a, b], 0, true, { dx: 50, dy: 0 }, view, 2);
    expect(linked[0]?.cx).toBe(1900);
    expect(linked[1]?.cx).toBe(2900);
    const solo = pannedPanes([a, b], 1, false, { dx: 50, dy: -10 }, view, 2);
    expect(solo[0]).toBe(a);
    expect([solo[1]?.cx, solo[1]?.cy]).toEqual([2900, 2020]);
    // 拡大できない面（null）はそのまま。
    expect(pannedPanes([a, null], 0, true, { dx: 1, dy: 1 }, view, 1)[1]).toBeNull();
  });
});
