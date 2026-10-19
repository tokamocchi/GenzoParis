// ローカル補正（マスク。DEV-18・DEV-19）の画面を描くテスト: 現像のパネルの「マスク」（MaskSection。一覧・部品・部品の
// 設定・調整）と、プレビューの上の層（MaskLayer のつまみ・見本の印・ブラシの描き方の印）、重ねて表示（MaskOverlayCanvas）。
// react-dom/server で HTML にして、出るものを確かめる（操作の流れは mask.test.ts）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { Frame } from "../api/frames";
import { mockDevelopSettings, mockDevelopState } from "../api/mock";
import type { DevelopSettings, LocalAdjustment, MaskOverlayInfo } from "../api/types";
import { appStore } from "../state/app";
import { developStore } from "../state/develop";
import { DevelopPanel } from "./DevelopPanel";
import { MaskLayer, MaskOverlayCanvas } from "./MaskLayer";
import { type Box, DEFAULT_BRUSH, type GBounds, defaultDelta, gToBox, newLuminanceShape, newMask, pixelsPerG } from "./maskLogic";
import { viewStore } from "./viewState";

/** 5 種類の部品（線形・円形（除く・反転）・ブラシ・色域・輝度範囲）と調整のあるマスクと、使わないマスク。 */
function fixtureMasks(): LocalAdjustment[] {
  return [
    {
      enabled: true,
      name: "空",
      components: [
        { mode: "add", invert: false, shape: { linear: { x0: 0.5, y0: 0.15, x1: 0.5, y1: 0.45 } } },
        {
          mode: "subtract",
          invert: true,
          shape: { radial: { cx: 0.25, cy: 0.375, rx: 0.2, ry: 0.1, angle_deg: 30, feather: 40 } },
        },
        {
          mode: "add",
          invert: false,
          shape: {
            brush: {
              strokes: [
                { erase: false, size: 0.05, feather: 50, flow: 100, density: 80, points: [[0.1, 0.1], [0.2, 0.15]] },
                { erase: true, size: 0.02, feather: 0, flow: 100, density: 100, points: [[0.15, 0.12]] },
              ],
            },
          },
        },
        {
          mode: "intersect",
          invert: false,
          shape: { color_range: { samples: [{ x: 0.75, y: 0.3, lab: [0.6, 0.1, -0.05] }], amount: 40 } },
        },
        { mode: "intersect", invert: false, shape: { luminance_range: { min: 20, max: 80, smoothness: 30 } } },
      ],
      delta: { ...defaultDelta(), exposure_ev: 0.5, temperature: -20 },
    },
    { ...newMask(newLuminanceShape()), enabled: false },
  ];
}

describe("マスクのパネルを描く", () => {
  beforeEach(() => {
    const data = new Map<string, string>([["genzo.section.develop.masks", "1"]]);
    (globalThis as { localStorage?: unknown }).localStorage = {
      getItem: (k: string) => data.get(k) ?? null,
      setItem: (k: string, v: string) => void data.set(k, String(v)),
      removeItem: (k: string) => void data.delete(k),
    };
  });
  afterEach(() => {
    delete (globalThis as { localStorage?: unknown }).localStorage;
    developStore.set({ variantId: null, state: null, settings: null });
    viewStore.set({ tool: "none", maskIndex: null, componentIndex: null, maskOverlay: false, brushOverlay: true, brush: DEFAULT_BRUSH, pickTarget: null });
  });

  /** マスク `mask`・部品 `component` を選んだ現像のパネル。 */
  function render(settings: DevelopSettings, mask: number | null, component: number | null): string {
    appStore.set((s) => ({ selection: { ...s.selection, current: 1, currentIndex: 0 }, dialog: null }));
    const state = mockDevelopState({ settings });
    developStore.set({ variantId: 1, state, settings, opening: false, error: null });
    viewStore.set({ maskIndex: mask, componentIndex: component });
    const html = renderToString(createElement(DevelopPanel)).replace(/<!-- -->/g, "");
    // マスクの節だけ（次の節（レンズ補正）の前まで）。
    const start = html.indexOf("マスク（");
    expect(start, "マスクの節がある").toBeGreaterThanOrEqual(0);
    return html.slice(start, html.indexOf("レンズ補正", start));
  }

  it("マスクがなければ案内と追加のボタンだけ", () => {
    appStore.set((s) => ({ selection: { ...s.selection, current: 1, currentIndex: 0 }, dialog: null }));
    const settings = mockDevelopSettings({ masks: [] });
    developStore.set({ variantId: 1, state: mockDevelopState({ settings }), settings, opening: false, error: null });
    const html = renderToString(createElement(DevelopPanel)).replace(/<!-- -->/g, "");
    expect(html).toContain("マスクを足すと、写真の一部だけを調整できます");
    expect(html).not.toContain("部品（上から順に合成）");
  });

  it("一覧・選んだマスクの部品・調整を出す（線形の部品はつまみの案内）", () => {
    const html = render(mockDevelopSettings({ masks: fixtureMasks() }), 0, 0);
    expect(html).toContain("マスク（2）");
    expect(html).toContain('aria-label="マスク"');
    expect(html).toContain("空");
    expect(html).toContain("マスク 2");
    expect(html).toContain("（調整なし）");
    expect(html).toContain('aria-label="マスクの名前"');
    expect(html).toContain("重ねて表示");
    expect(html).toContain("複製");
    expect(html).toContain("部品（上から順に合成）");
    for (const label of ["1. 線形グラデーション", "2. 円形グラデーション", "3. ブラシ", "4. 色域", "5. 輝度範囲"]) {
      expect(html).toContain(label);
    }
    expect(html).toContain('aria-label="部品 2 の合成"');
    expect(html).toMatch(/<option value="subtract" title="[^"]*" selected="">除く<\/option>/);
    expect(html).toContain("反転");
    expect(html).toContain("実線の丸（100%）と点線の丸（0%）をドラッグ");
    for (const label of ["ホワイトバランス", "色温度", "色かぶり補正", "露光量", "コントラスト", "テクスチャ", "かすみの除去", "彩度", "シャープ"]) {
      expect(html).toContain(label);
    }
    expect(html).toContain("調整をリセット");
    expect(html).not.toContain("処理バージョンを 2 に更新");
  });

  it("部品ごとの設定: 円形・ブラシ・色域・輝度範囲", () => {
    const settings = mockDevelopSettings({ masks: fixtureMasks() });
    const radial = render(settings, 0, 1);
    expect(radial).toContain("中心・縁の丸（半径）・外の丸（回転）をドラッグ");
    expect(radial).toContain("ぼかし");
    expect(radial).toContain("角度");
    expect(radial).toContain('value="30°"');

    const brush = render(settings, 0, 2);
    for (const label of ["ブラシで描く", "消しゴム", "大きさ", "ぼかし", "流量", "密度", "最後の 1 本を消す", "すべて消す"]) {
      expect(brush).toContain(label);
    }
    expect(brush).toContain("2 本");
    expect(brush).toContain('value="5.0%"');

    const color = render(settings, 0, 3);
    expect(color).toContain('aria-label="見本の色"');
    expect(color).toContain("dp-swatch");
    expect(color).toContain("見本を足す");
    expect(color).toContain("許容量");
    expect(color).toContain('value="40"');

    const lum = render(settings, 0, 4);
    expect(lum).toContain("dp-lum-bar");
    expect(lum).toContain("下端");
    expect(lum).toContain("上端");
    expect(lum).toContain("滑らかさ");
    expect(lum).toContain("transparent 20%");
  });

  it("処理バージョン 1 では更新を案内し、編集できなくする（削除はできる）", () => {
    const html = render(mockDevelopSettings({ process_version: 1, masks: fixtureMasks() }), 0, 0);
    expect(html).toContain("ローカル補正（マスク）は使われません");
    expect(html).toContain("処理バージョンを 2 に更新");
    expect(html).toMatch(/<input type="checkbox" disabled="" aria-label="空を使う"/);
    expect(html).toMatch(/<button type="button" class="btn small" title="[^"]*">削除<\/button>/);
    expect(html).toMatch(/<button type="button" class="btn small" disabled="" title="[^"]*">複製<\/button>/);
    expect(html).toMatch(/<select disabled="" aria-label="部品 1 の合成"/);
  });
});

describe("プレビューの上のマスクの層を描く", () => {
  const gb: GBounds = [0, 0, 1, 0.75];
  const box: Box = { left: 10, top: 20, width: 400, height: 300 };
  const layer = (over: Partial<Parameters<typeof MaskLayer>[0]> = {}) =>
    renderToString(
      createElement(MaskLayer, {
        width: 420,
        height: 340,
        box,
        gb,
        mask: fixtureMasks()[0],
        index: 0,
        component: 0,
        painting: false,
        brush: DEFAULT_BRUSH,
        editable: true,
        overlayOn: false,
        overlayGeneration: null,
        ...over,
      }),
    );
  const attr = (name: string, v: number) => `${name}="${v}"`;

  it("線形: 100% と 0% のつまみを G 座標から画面の位置に置く", () => {
    const html = layer();
    expect(html).toContain('class="dv-layer dv-mask"');
    expect(html).toContain('class="dv-mask-shape selected"');
    const [x0, y0] = gToBox(gb, box, 0.5, 0.15);
    const [x1, y1] = gToBox(gb, box, 0.5, 0.45);
    expect(html).toContain(`<circle class="dv-mask-knob" ${attr("cx", x0)} ${attr("cy", y0)} r="6">`);
    expect(html).toContain(`<circle class="dv-mask-knob zero" ${attr("cx", x1)} ${attr("cy", y1)} r="6">`);
    expect(html).toContain("<title>100% の位置（ドラッグで動かす）</title>");
    expect(html).toContain("<title>0% の位置（ドラッグで動かす）</title>");
    expect(html).toContain('class="dv-mask-grip move"');
  });

  it("円形: 回転・ぼかしの内側の楕円・半径と回転のつまみ。除く部品は印を変える", () => {
    const html = layer({ component: 1 });
    const [cx, cy] = gToBox(gb, box, 0.25, 0.375);
    const ppg = pixelsPerG(gb, box);
    expect(html).toContain('class="dv-mask-shape selected subtract"');
    expect(html).toContain(`transform="rotate(30 ${cx} ${cy})"`);
    expect(html).toContain(`<ellipse class="dv-mask-line full" ${attr("cx", cx)} ${attr("cy", cy)} ${attr("rx", 0.2 * ppg)} ${attr("ry", 0.1 * ppg)}`);
    expect(html).toContain(`<ellipse class="dv-mask-line mid" ${attr("cx", cx)} ${attr("cy", cy)} ${attr("rx", 0.2 * ppg * 0.6)}`);
    for (const t of ["中心（ドラッグで動かす）", "横の半径", "縦の半径", "回転"]) expect(html).toContain(`<title>${t}</title>`);
    expect(html).toContain('class="dv-mask-knob rotate"');
  });

  it("色域の見本の点に印を出す。輝度範囲・ブラシの部品には画面のつまみがない", () => {
    const html = layer({ component: 3 });
    const [x, y] = gToBox(gb, box, 0.75, 0.3);
    expect(html).toContain(`<g class="dv-mask-sample"><circle ${attr("cx", x)} ${attr("cy", y)} r="5">`);
    // 部品は 5 つあり、ブラシ・輝度範囲は描かない（線形・円形・色域の 3 つ）。
    expect(html.match(/class="dv-mask-shape/g)?.length).toBe(3);
  });

  it("ブラシで描いている間はポインターを受ける層にする", () => {
    const html = layer({ component: 2, painting: true });
    expect(html).toContain('class="dv-layer dv-mask painting"');
    // ポインターを動かす前は印（カーソル）も描きかけの線もない。
    expect(html).not.toContain("dv-mask-cursor");
    expect(html).not.toContain("dv-mask-stroke");
  });

  it("重ねて表示の canvas を画像の箱に置く", () => {
    const frame: Frame<MaskOverlayInfo> = {
      header: { variant_id: 1, generation: 3, index: 0, width: 2, height: 1, uncropped: false },
      width: 2,
      height: 1,
      pixels: new Uint8ClampedArray(new ArrayBuffer(8)),
    };
    const html = renderToString(createElement(MaskOverlayCanvas, { frame, style: box }));
    expect(html).toBe('<canvas class="dv-layer dv-mask-overlay" style="left:10px;top:20px;width:400px;height:300px"></canvas>');
  });
});
