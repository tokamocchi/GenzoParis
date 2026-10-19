// ローカル補正（マスク。DEV-18・DEV-19）の設定の組み立て・座標・ブラシの点・道具の流れのテスト。

import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { fetchMaskOverlay } from "../api/frames";
import { MOCK_MASK_LAB, MockBridge, mockDevelopSettings, mockMaskCommands, mockSourceInfo } from "../api/mock";
import type { DevelopSettings, DevelopState, LocalAdjustment, MaskComponent } from "../api/types";
import { MASK_LIMITS } from "../api/types";
import { SHORTCUT_GROUPS } from "../dialogs/shortcutsData";
import { appStore } from "../state/app";
import { developStore, enqueue, openDevelopFor } from "../state/develop";
import {
  DEFAULT_BRUSH,
  addColorSample,
  addComponent,
  addMask,
  appendStroke,
  boxToG,
  deltaIsZero,
  dragLinear,
  dragRadial,
  duplicateMask,
  gToBox,
  labToCss,
  makeStroke,
  maskControls,
  maskDisplayName,
  maskSummary,
  newColorShape,
  newComponent,
  newLinearShape,
  newLuminanceShape,
  newMask,
  newRadialShape,
  overlayRgba,
  pixelsPerG,
  removeColorSample,
  sanitizeMaskName,
  settlePendingStrokes,
  shapeKind,
  simplifyPoints,
  totalBrushPoints,
  withDelta,
  withLuminance,
  wrapAngle,
} from "./maskLogic";
import {
  commitBrushStroke,
  currentMask,
  endAllTools,
  overlayShown,
  pickMaskColor,
  selectMask,
  startMaskColorPick,
  toggleBrushTool,
  toggleMaskOverlay,
  viewStore,
} from "./viewState";

const GB: [number, number, number, number] = [0, 0, 1, 0.75];

function withMasks(masks: LocalAdjustment[], over: Partial<DevelopSettings> = {}): DevelopSettings {
  return mockDevelopSettings({ masks, ...over });
}

function brushMask(): LocalAdjustment {
  return newMask({ brush: { strokes: [] } });
}

describe("マスクの設定の組み立て", () => {
  it("新しい形は見えている範囲に置き、範囲マスクは交差で足す", () => {
    const l = newLinearShape(GB);
    expect(l).toEqual({ linear: { x0: 0.5, y0: 0.15, x1: 0.5, y1: 0.375 } });
    const r = newRadialShape([0.2, 0.1, 0.6, 0.4]);
    expect(r).toEqual({ radial: { cx: 0.4, cy: 0.25, rx: 0.1, ry: 0.075, angle_deg: 0, feather: 50 } });
    expect(newComponent(l).mode).toBe("add");
    expect(newComponent(newLuminanceShape()).mode).toBe("intersect");
    expect(newComponent(newColorShape({ x: 0.1, y: 0.2, lab: [0.5, 0, 0] })).mode).toBe("intersect");
    expect(shapeKind(newLuminanceShape())).toBe("luminance_range");
    const m = newMask(l);
    expect(m.enabled).toBe(true);
    expect(deltaIsZero(m.delta)).toBe(true);
    expect(maskSummary({ ...m, components: [...m.components, newComponent(newLuminanceShape())] })).toBe(
      "線形グラデーション ＋ 輝度範囲",
    );
  });

  it("名前・処理バージョン・上限", () => {
    expect(maskDisplayName({ name: "" }, 0)).toBe("マスク 1");
    expect(maskDisplayName({ name: " 空 " }, 3)).toBe(" 空 ");
    expect(sanitizeMaskName("空\u0007の\n部分")).toBe("空の部分");
    expect([...sanitizeMaskName("あ".repeat(150))].length).toBe(MASK_LIMITS.nameChars);
    expect(maskControls(withMasks([]), false)).toEqual({ needsUpgrade: false, editable: true, canAdd: true });
    expect(maskControls(withMasks([], { process_version: 1 }), false)).toEqual({
      needsUpgrade: true,
      editable: false,
      canAdd: false,
    });
    const full = withMasks(Array.from({ length: MASK_LIMITS.masks }, brushMask));
    expect(maskControls(full, false).canAdd).toBe(false);
    expect(addMask(full, brushMask())).toBe(full);
    expect(duplicateMask(full, 0)).toBe(full);
  });

  it("差分は範囲に丸め、複製は名前に「のコピー」を付ける", () => {
    const s = withMasks([newMask(newLinearShape(GB))]);
    const t = withDelta(s, 0, { exposure_ev: 7, dehaze: -150, temperature: 20 });
    expect(t.masks[0].delta).toMatchObject({ exposure_ev: 5, dehaze: -100, temperature: 20 });
    expect(withDelta(s, 3, { exposure_ev: 1 })).toBe(s);
    const d = duplicateMask(t, 0);
    expect(d.masks).toHaveLength(2);
    expect(d.masks[1].name).toBe("マスク 1のコピー");
    expect(d.masks[1].delta).toEqual(t.masks[0].delta);
    expect(d.masks[1]).not.toBe(d.masks[0]);
  });

  it("色域の見本は 5 つまで（超えたら最後を置き換える）、最後の 1 つは除かない", () => {
    const sample = (x: number) => ({ x, y: 0.2, lab: [0.5, 0.1, -0.1] as [number, number, number] });
    let s = withMasks([newMask(newColorShape(sample(0.1)))]);
    for (let k = 2; k <= 6; k++) s = addColorSample(s, 0, 0, sample(k / 10));
    const shape = s.masks[0].components[0].shape;
    if (!("color_range" in shape)) throw new Error("色域");
    expect(shape.color_range.samples.map((x) => x.x)).toEqual([0.1, 0.2, 0.3, 0.4, 0.6]);
    let one = withMasks([newMask(newColorShape(sample(0.1)))]);
    one = removeColorSample(one, 0, 0, 0);
    const kept = one.masks[0].components[0].shape;
    expect("color_range" in kept && kept.color_range.samples).toHaveLength(1);
    // 値の範囲（OKLab の L は 0〜10、a・b は −4〜4）に丸める。
    const c = newColorShape({ x: 3, y: -2, lab: [12, 5, -5] });
    expect("color_range" in c && c.color_range.samples[0]).toEqual({ x: 2, y: -1, lab: [10, 4, -4] });
  });

  it("輝度範囲の下端が上端を越えたら、もう一方を押す", () => {
    const s = withMasks([newMask({ luminance_range: { min: 20, max: 60, smoothness: 50 } })]);
    const lum = (t: DevelopSettings) => {
      const sh = t.masks[0].components[0].shape;
      return "luminance_range" in sh ? sh.luminance_range : null;
    };
    expect(lum(withLuminance(s, 0, 0, { min: 80 }))).toEqual({ min: 80, max: 80, smoothness: 50 });
    expect(lum(withLuminance(s, 0, 0, { max: 10 }))).toEqual({ min: 10, max: 10, smoothness: 50 });
    expect(lum(withLuminance(s, 0, 0, { smoothness: 140 }))?.smoothness).toBe(100);
  });

  it("部品は 32 個まで", () => {
    let s = withMasks([brushMask()]);
    for (let k = 0; k < MASK_LIMITS.components + 3; k++) s = addComponent(s, 0, newComponent(newLuminanceShape()));
    expect(s.masks[0].components).toHaveLength(MASK_LIMITS.components);
  });
});

describe("G 座標と画面の画像の箱", () => {
  it("往復で同じ点に戻り、縦横の尺度は同じ", () => {
    const gb: [number, number, number, number] = [0.1, 0.05, 0.9, 0.65];
    const box = { left: 20, top: 10, width: 400, height: 300 };
    const [x, y] = gToBox(gb, box, 0.5, 0.35);
    expect([x, y]).toEqual([220, 160]);
    const [u, v] = boxToG(gb, box, x, y);
    expect(u).toBeCloseTo(0.5, 12);
    expect(v).toBeCloseTo(0.35, 12);
    expect(pixelsPerG(gb, box)).toBeCloseTo(500, 9);
    expect(box.height / (gb[3] - gb[1])).toBeCloseTo(500, 9);
  });

  it("線形のつまみ: 端だけ・全体を動かし、位置の範囲（−1〜2）に収める", () => {
    const l = { x0: 0.5, y0: 0.1, x1: 0.5, y1: 0.4 };
    expect(dragLinear(l, "start", 0.1, 0)).toEqual({ x0: 0.6, y0: 0.1, x1: 0.5, y1: 0.4 });
    expect(dragLinear(l, "end", 0, 0.2)).toEqual({ x0: 0.5, y0: 0.1, x1: 0.5, y1: 0.6 });
    expect(dragLinear(l, "move", 0, -0.05)).toEqual({ x0: 0.5, y0: 0.05, x1: 0.5, y1: 0.35 });
    expect(dragLinear(l, "move", 5, 0).x0).toBe(2);
  });

  it("円形のつまみ: 回転した軸に沿った半径・中心から見た向きの角度（時計回り）", () => {
    const r = { cx: 0.5, cy: 0.4, rx: 0.2, ry: 0.1, angle_deg: 0, feather: 50 };
    expect(dragRadial(r, "rx", [0.8, 0.45], [0.7, 0.4]).rx).toBeCloseTo(0.3, 9);
    expect(dragRadial(r, "ry", [0.55, 0.65], [0.5, 0.5]).ry).toBeCloseTo(0.25, 9);
    // 90° 回すと、横の半径は下向き（G 座標の y が増える向き）。
    const r90 = { ...r, angle_deg: 90 };
    expect(dragRadial(r90, "rx", [0.5, 0.75], [0.5, 0.6]).rx).toBeCloseTo(0.35, 9);
    // 回転のつまみ: 真上で 0°、右で 90°、左で −90°、真下で 180°。
    expect(dragRadial(r, "rotate", [0.5, 0.1], [0.5, 0.2]).angle_deg).toBe(0);
    expect(dragRadial(r, "rotate", [0.9, 0.4], [0.5, 0.2]).angle_deg).toBe(90);
    expect(dragRadial(r, "rotate", [0.1, 0.4], [0.5, 0.2]).angle_deg).toBe(-90);
    expect(dragRadial(r, "rotate", [0.5, 0.9], [0.5, 0.2]).angle_deg).toBe(180);
    expect(dragRadial(r, "move", [0.6, 0.5], [0.5, 0.4])).toMatchObject({ cx: 0.6, cy: 0.5 });
    expect(wrapAngle(190)).toBe(-170);
    expect(wrapAngle(-190)).toBe(170);
    expect(wrapAngle(180)).toBe(180);
  });
});

describe("ブラシ", () => {
  it("点を間引き（最初と最後は残す）、上限を超えたら等間隔に選ぶ", () => {
    const pts: [number, number][] = Array.from({ length: 101 }, (_, i) => [i / 1000, 0]);
    const s = simplifyPoints(pts, 0.01, 5000);
    expect(s[0]).toEqual([0, 0]);
    expect(s.at(-1)).toEqual([0.1, 0]);
    expect(s).toHaveLength(11);
    const many = simplifyPoints(pts, 0, 10);
    expect(many).toHaveLength(10);
    expect(many[0]).toEqual([0, 0]);
    expect(many.at(-1)).toEqual([0.1, 0]);
    expect(simplifyPoints([[Number.NaN, 0], [0.2, 0.3]], 0.01, 10)).toEqual([[0.2, 0.3]]);
  });

  it("ストロークは道具の設定で作り、大きさを範囲に丸める", () => {
    const st = makeStroke(
      [
        [0.1, 0.1],
        [0.1, 0.1001],
        [0.3, 0.1],
      ],
      { ...DEFAULT_BRUSH, size: 0.9, flow: 140, erase: true },
    );
    expect(st).toEqual({ erase: true, size: 0.5, feather: 50, flow: 100, density: 100, points: [[0.1, 0.1], [0.3, 0.1]] });
  });

  it("ブラシの部品にだけ足し、本数・点の数の上限を超えるなら断る", () => {
    const s = withMasks([brushMask(), newMask(newLinearShape(GB))]);
    const st = makeStroke([[0.2, 0.2]], DEFAULT_BRUSH);
    const r = appendStroke(s, 0, 0, st);
    expect("settings" in r && totalBrushPoints(r.settings)).toBe(1);
    expect(appendStroke(s, 1, 0, st)).toEqual({ error: "ブラシの部品を選んでください" });
    expect("error" in appendStroke(s, 0, 0, { ...st, points: [] })).toBe(true);
    const fullStrokes = withMasks([
      newMask({ brush: { strokes: Array.from({ length: MASK_LIMITS.strokes }, () => st) } }),
    ]);
    const e1 = appendStroke(fullStrokes, 0, 0, st);
    expect("error" in e1 && e1.error).toContain("ストローク");
    const big = { ...st, points: Array.from({ length: 5000 }, (_, i): [number, number] => [i / 5000, 0.5]) };
    const manyPoints = withMasks([
      newMask({ brush: { strokes: Array.from({ length: 20 }, () => big) } }),
    ]);
    expect(totalBrushPoints(manyPoints)).toBe(MASK_LIMITS.totalBrushPoints);
    const e2 = appendStroke(manyPoints, 0, 0, st);
    expect("error" in e2 && e2.error).toContain("100,000");
  });
});

describe("重ねて表示と見本の色", () => {
  it("重みを赤の半透明にする（重み 0 は透明）", () => {
    const px = new Uint8ClampedArray([255, 255, 255, 255, 0, 0, 0, 255, 128, 128, 128, 255]);
    const out = overlayRgba(px);
    expect([...out.slice(0, 4)]).toEqual([255, 40, 40, 128]);
    expect(out[7]).toBe(0);
    expect(out[11]).toBe(64);
  });

  it("離したストロークは、コアが受け付けた世代以降の重ねて表示が届くまで描いておく", () => {
    const p = (seq: number) => ({
      points: [[0.1, 0.1]] as [number, number][],
      erase: false,
      size: 0.05,
      seq,
      generation: null,
    });
    const pending = [p(3), p(4)];
    // まだ受け付けていない（前の設定の受け付けだけ）: 変わらない。
    expect(settlePendingStrokes(pending, [{ seq: 2, generation: 10 }], true, 10)).toBe(pending);
    // 3 番を世代 11 で受け付けた: 世代を埋めるが、重ねて表示は前の世代なので描いておく。
    const a = settlePendingStrokes(pending, [{ seq: 3, generation: 11 }], true, 10);
    expect(a.map((x) => [x.seq, x.generation])).toEqual([
      [3, 11],
      [4, null],
    ]);
    // 世代 11 の重ねて表示が届いた: 3 番を除く。
    const b = settlePendingStrokes(a, [{ seq: 3, generation: 11 }], true, 11);
    expect(b.map((x) => x.seq)).toEqual([4]);
    // 後の設定（4 番は前の設定を含む）を受け付け、その重ねて表示が届いた: すべて除く。
    expect(settlePendingStrokes(b, [{ seq: 5, generation: 13 }], true, 12).map((x) => x.generation)).toEqual([13]);
    expect(settlePendingStrokes(b, [{ seq: 5, generation: 13 }], true, 13)).toEqual([]);
    // 重ねて表示をしていない・まだ届いていない。
    expect(settlePendingStrokes(pending, [{ seq: 4, generation: 12 }], false, null)).toEqual([]);
    expect(settlePendingStrokes(pending, [{ seq: 4, generation: 12 }], true, null)).toHaveLength(2);
    // コアが受け付けなかった（送信が失敗した）: その番号までの、まだ世代の分からないストロークはすぐに除く。
    expect(settlePendingStrokes(pending, [{ seq: 3, generation: null }], true, null).map((x) => x.seq)).toEqual([4]);
    const failed = settlePendingStrokes(a, [{ seq: 4, generation: null }], true, 10);
    expect(failed.map((x) => [x.seq, x.generation])).toEqual([[3, 11]]);
    // 前の設定を受け付けた直後に、後の設定が失敗した（両方の結果を同時に見る）: 前のストロークは受け付けたものとして残す。
    const both = settlePendingStrokes(
      pending,
      [
        { seq: 3, generation: 11 },
        { seq: 4, generation: null },
      ],
      true,
      10,
    );
    expect(both.map((x) => [x.seq, x.generation])).toEqual([[3, 11]]);
    // 失敗したが、そのときに後の設定（失敗したストロークを含む）が渡されていた: 後の設定の結果を待つ。
    const carried = settlePendingStrokes(pending, [{ seq: 3, generation: null, superseded: true }], true, 10);
    expect(carried).toBe(pending);
    const later = settlePendingStrokes(
      pending,
      [
        { seq: 3, generation: null, superseded: true },
        { seq: 4, generation: 12 },
      ],
      true,
      10,
    );
    expect(later.map((x) => [x.seq, x.generation])).toEqual([
      [3, 12],
      [4, 12],
    ]);
  });

  it("OKLab の見本の色を CSS の色にする（無彩色は R = G = B）", () => {
    expect(labToCss([1, 0, 0])).toBe("#ffffff");
    expect(labToCss([0, 0, 0])).toBe("#000000");
    const gray = labToCss([0.6, 0, 0]);
    expect(gray.slice(1, 3)).toBe(gray.slice(3, 5));
    expect(gray.slice(3, 5)).toBe(gray.slice(5, 7));
    const red = labToCss([0.6, 0.2, 0.1]);
    expect(Number.parseInt(red.slice(1, 3), 16)).toBeGreaterThan(Number.parseInt(red.slice(3, 5), 16));
    // 1 を超える明るさは、比を保って縮める。
    expect(labToCss([3, 0, 0])).toBe("#ffffff");
  });

  it("O と [ ] のショートカットを一覧に載せる", () => {
    const develop = SHORTCUT_GROUPS.find((g) => g.title === "現像");
    expect(develop?.items.some((i) => i.keys.includes("o") && i.label.includes("マスク"))).toBe(true);
    expect(develop?.items.some((i) => i.keys.includes("[") && i.keys.includes("]"))).toBe(true);
  });
});

describe("マスクの道具（コアとのやり取り）", () => {
  let mock: MockBridge;
  let sent: Record<string, unknown>[];
  let settings: DevelopSettings;

  function state(id: number): DevelopState {
    return {
      variant_id: id,
      settings,
      generation: 1,
      can_undo: false,
      can_redo: false,
      dirty: false,
      dragging: false,
      source: mockSourceInfo(),
      before: null,
      latest_process_version: 2,
    };
  }

  beforeEach(async () => {
    mock = new MockBridge();
    installBridge(mock);
    settings = withMasks([]);
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as { type: string; variant_id?: number };
      switch (c.type) {
        case "open_develop":
          return { type: "develop", ...state(c.variant_id ?? 1) };
        case "set_settings":
          return { type: "generation", generation: 2 };
        default:
          return { type: "ok" };
      }
    });
    mock.responses.set("develop_frame", new ArrayBuffer(0));
    ({ sent } = mockMaskCommands(mock, { masks: () => developStore.get().settings?.masks.length ?? 0 }));
    appStore.set({ toasts: [] });
    viewStore.set({
      tool: "none",
      maskIndex: null,
      componentIndex: null,
      maskOverlay: false,
      brushOverlay: true,
      pickTarget: null,
    });
    // 前のテストで開いた写真と違う ID で開く（同じ写真なら開いているものを返すため）。
    await openDevelopFor(Math.floor(Math.random() * 1e9) + 10);
  });

  /** 送信の列が空になるのを待って、最後に送った設定を返す（画面の設定と同じであることも確かめる）。 */
  const lastSettings = async (): Promise<DevelopSettings | undefined> => {
    await enqueue(async () => undefined);
    const last = mock.calls
      .map((c) => c.args?.command as { type: string; settings?: DevelopSettings } | undefined)
      .filter((c) => c?.type === "set_settings")
      .at(-1)?.settings;
    expect(last).toEqual(developStore.get().settings);
    return last;
  };

  it("マスクがなければ O で知らせ、あれば最初のマスクを選んで重ねて表示する", () => {
    toggleMaskOverlay();
    expect(viewStore.get().maskOverlay).toBe(false);
    expect(appStore.get().toasts.at(-1)?.message).toContain("マスクがありません");
    developStore.set({ settings: withMasks([brushMask()]) });
    toggleMaskOverlay();
    expect(viewStore.get()).toMatchObject({ maskOverlay: true, maskIndex: 0, componentIndex: 0 });
    toggleMaskOverlay();
    expect(viewStore.get().maskOverlay).toBe(false);
  });

  it("ブラシで描いている間は既定で重ねて表示し、O は描いている間の表示だけを切り替える", () => {
    developStore.set({ settings: withMasks([brushMask()]) });
    selectMask(0);
    expect(overlayShown(viewStore.get())).toBe(false);
    toggleBrushTool();
    expect(viewStore.get().tool).toBe("brush");
    expect(overlayShown(viewStore.get())).toBe(true);
    toggleMaskOverlay();
    expect(viewStore.get()).toMatchObject({ brushOverlay: false, maskOverlay: false });
    expect(overlayShown(viewStore.get())).toBe(false);
    toggleMaskOverlay();
    expect(overlayShown(viewStore.get())).toBe(true);
    // 描くのをやめたら、ふだんの表示（O で切り替える）に戻る。
    toggleBrushTool();
    expect(viewStore.get().tool).toBe("none");
    expect(overlayShown(viewStore.get())).toBe(false);
  });

  it("色域のスポイトで、点の色を見本にした新しいマスクを作る（交差の部品 1 つ）", async () => {
    startMaskColorPick({ kind: "new_mask" });
    expect(viewStore.get().tool).toBe("mask_color");
    await pickMaskColor(0.5, 0.5);
    expect(sent[0]).toMatchObject({ type: "mask_color_at", x: 0.5, y: 0.5, uncropped: false });
    const s = await lastSettings();
    const comp = s?.masks[0].components[0] as MaskComponent;
    expect(comp.mode).toBe("intersect");
    expect(comp.shape).toEqual({ color_range: { samples: [{ x: 0.5, y: 0.375, lab: MOCK_MASK_LAB }], amount: 50 } });
    expect(viewStore.get()).toMatchObject({ tool: "none", maskIndex: 0, componentIndex: 0 });
    // 見本を足す。
    startMaskColorPick({ kind: "add_sample", mask: 0, component: 0 });
    await pickMaskColor(0.25, 0);
    const sh = (await lastSettings())?.masks[0].components[0].shape;
    expect(sh && "color_range" in sh && sh.color_range.samples.map((x) => [x.x, x.y])).toEqual([
      [0.5, 0.375],
      [0.25, 0],
    ]);
  });

  it("ブラシのストロークは、離したときに選んでいるブラシの部品に 1 本として足す", async () => {
    developStore.set({ settings: withMasks([brushMask()]) });
    selectMask(0);
    toggleBrushTool();
    expect(viewStore.get().tool).toBe("brush");
    const first = commitBrushStroke(
      [
        [0.1, 0.1],
        [0.2, 0.1],
      ],
      false,
    );
    const second = commitBrushStroke([[0.3, 0.3]], true);
    expect(first).not.toBeNull();
    expect(second).toBeGreaterThan(first ?? Infinity);
    const sh = (await lastSettings())?.masks[0].components[0].shape;
    expect(sh && "brush" in sh && sh.brush.strokes.map((x) => [x.erase, x.points.length])).toEqual([
      [false, 2],
      [true, 1],
    ]);
    // コアが受け付けた設定の番号と世代（離したストロークを、重ねて表示に入るまで描いておくのに使う）。
    expect(developStore.get().settingsAcks.at(-1)).toEqual({ seq: second, generation: 2 });
    // コアが受け付けなかったら、世代は null（離したストロークを残さない）。
    const base = mock.responses.get("core_execute") as (a?: Record<string, unknown>) => unknown;
    mock.responses.set("core_execute", (args?: Record<string, unknown>) =>
      (args?.command as { type: string }).type === "set_settings" ? new Error("受け付けない") : base(args),
    );
    const third = commitBrushStroke([[0.4, 0.4]], false);
    await enqueue(async () => undefined);
    expect(developStore.get().settingsAcks.at(-1)).toEqual({ seq: third, generation: null, superseded: false });
    mock.responses.set("core_execute", base);
    // ブラシでない部品を選んでいれば描き始めない。
    viewStore.set({ tool: "none" });
    developStore.set({ settings: withMasks([newMask(newLinearShape(GB))]) });
    selectMask(0);
    toggleBrushTool();
    expect(viewStore.get().tool).toBe("none");
  });

  it("処理バージョン 1 の写真では、ブラシ・スポイトを使い始めない", () => {
    developStore.set({ settings: withMasks([brushMask()], { process_version: 1 }) });
    selectMask(0);
    toggleBrushTool();
    startMaskColorPick({ kind: "new_mask" });
    expect(viewStore.get().tool).toBe("none");
  });

  it("選んでいる番号が設定の外なら選んでいないとみなし、写真を変えたら選択を外す", () => {
    const s = withMasks([brushMask()]);
    expect(currentMask(s, { maskIndex: 0, componentIndex: 3 })).toMatchObject({ index: 0, component: null });
    expect(currentMask(s, { maskIndex: 2, componentIndex: 0 })).toBeNull();
    viewStore.set({ maskIndex: 0, componentIndex: 0, maskOverlay: true });
    endAllTools();
    expect(viewStore.get()).toMatchObject({ maskIndex: null, componentIndex: null, maskOverlay: true });
  });

  it("重ねて表示の画素を読む（ヘッダーは MaskOverlayInfo）", async () => {
    developStore.set({ settings: withMasks([brushMask()]) });
    const f = await fetchMaskOverlay(7, 0, true);
    expect(f?.header).toEqual({ variant_id: 7, generation: 1, index: 0, width: 4, height: 3, uncropped: true });
    expect(f?.pixels[0]).toBe(255);
    expect(f?.pixels[3 * 4]).toBe(0);
    expect(sent.at(-1)).toMatchObject({ type: "mask_overlay_frame", variantId: 7, index: 0, uncropped: true });
    await expect(fetchMaskOverlay(7, 5)).rejects.toBeTruthy();
  });
});
