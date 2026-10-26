// スポット修正（DEV-21）の設定の組み立て・座標（O 座標の表）・道具の流れのテスト。

import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MOCK_RETOUCH_OFFSET, MockBridge, mockDevelopSettings, mockRetouchCommands, mockSourceInfo } from "../api/mock";
import type { DevelopSettings, DevelopState, RetouchSpot } from "../api/types";
import { ORIENTED_GRID_SIZE, RANGES, RETOUCH_LIMITS } from "../api/types";
import { SHORTCUT_GROUPS } from "../dialogs/shortcutsData";
import { appStore } from "../state/app";
import { applySettings, developStore, enqueue, openDevelopFor, undo } from "../state/develop";
import {
  DEFAULT_RETOUCH,
  addSpot,
  boxToO,
  clearSpots,
  fallbackOffset,
  fracToO,
  moveSource,
  moveSpot,
  newSpot,
  oToBox,
  oToFrac,
  orientedGrid,
  pixelsPerO,
  removeSpot,
  retouchControls,
  retouchSize,
  spotSummary,
  spotsOf,
  withSpot,
} from "./retouchLogic";
import { resetAll } from "./settingsLogic";
import {
  cancelRetouchTool,
  commitRetouchSpot,
  currentSpot,
  reselectSource,
  selectMask,
  selectSpot,
  toggleRetouchTool,
  viewStore,
} from "./viewState";

const spot = (points: [number, number][], offset: [number, number] = [0.05, 0]): RetouchSpot => ({
  ...newSpot(points, DEFAULT_RETOUCH),
  offset,
});

/** 角度 θ（度。反時計回り）で回転した、長辺 1・縦横比 3:2 の画像の O 座標の表（中心のまわり）。 */
function rotatedGrid(deg: number): [number, number][] {
  const t = (deg * Math.PI) / 180;
  const [cx, cy] = [0.5, 1 / 3];
  const out: [number, number][] = [];
  const n = ORIENTED_GRID_SIZE;
  for (let j = 0; j < n; j++) {
    for (let i = 0; i < n; i++) {
      // 画面の割合 → G（O と同じ大きさの枠とする）→ O（G = R·(O − 中心) の逆）。
      const gx = (i / (n - 1)) * 1 - cx;
      const gy = (j / (n - 1)) * (2 / 3) - cy;
      out.push([cx + Math.cos(t) * gx - Math.sin(t) * gy, cy + Math.sin(t) * gx + Math.cos(t) * gy]);
    }
  }
  return out;
}

describe("スポット修正の計算", () => {
  it("新しいスポットは道具の設定で作り、大きさ・点を丸める", () => {
    const s = newSpot([[0.3, 0.2]], { mode: "clone", size: 0.9, feather: 30.4, opacity: 120 });
    expect(s).toMatchObject({ enabled: true, mode: "clone", size: RANGES.retouchSize.max, feather: 30, opacity: 100 });
    expect(s.points).toEqual([[0.3, 0.2]]);
    expect(s.offset).toEqual([0, 0]);
    // 線の点は大きさの 1/4 より近い点を間引く（最後の点は残す）。
    const line = newSpot(
      [
        [0.1, 0.1],
        [0.1005, 0.1],
        [0.12, 0.1],
        [0.1201, 0.1],
      ],
      DEFAULT_RETOUCH,
    );
    expect(line.points).toEqual([
      [0.1, 0.1],
      [0.12, 0.1],
      [0.1201, 0.1],
    ]);
    expect(spotSummary(line)).toBe("線（3 点）");
    expect(spotSummary(s)).toBe("円");
    expect(retouchSize(-1)).toBe(RANGES.retouchSize.min);
  });

  it("選べなかったときのずれは、大きさの 2.5 倍だけ右（右半分なら左）", () => {
    expect(fallbackOffset(spot([[0.2, 0.3]]))).toEqual([0.025, 0]);
    expect(fallbackOffset(spot([[0.8, 0.3]]))).toEqual([-0.025, 0]);
    // 縦長の写真（O の幅 0.667）の右半分。
    expect(fallbackOffset(spot([[0.5, 0.3]]), 2 / 3)).toEqual([-0.025, 0]);
  });

  it("足す・変える・動かす・除く（上限はコアと同じ）", () => {
    const base = mockDevelopSettings();
    const r = addSpot(base, spot([[0.2, 0.2]]));
    expect("settings" in r).toBe(true);
    const s1 = "settings" in r ? r.settings : base;
    expect(spotsOf(s1)).toHaveLength(1);
    expect(addSpot(base, { ...spot([[0.2, 0.2]]), points: [] })).toEqual({ error: "スポットに点がありません" });
    const full = { ...base, retouch: Array.from({ length: RETOUCH_LIMITS.spots }, () => spot([[0.5, 0.5]])) };
    expect("error" in addSpot(full, spot([[0.2, 0.2]]))).toBe(true);
    const many = { ...base, retouch: [{ ...spot([[0.5, 0.5]]), points: Array.from({ length: RETOUCH_LIMITS.totalPoints }, () => [0.5, 0.5] as [number, number]) }] };
    expect(addSpot(many, spot([[0.2, 0.2]]))).toMatchObject({ error: expect.stringContaining("点が多すぎます") });
    // 変える（番号の外・同じ値なら同じ設定）。
    const s2 = withSpot(s1, 0, (x) => ({ ...x, opacity: 40 }));
    expect(spotsOf(s2)[0].opacity).toBe(40);
    expect(withSpot(s1, 3, (x) => ({ ...x, opacity: 40 }))).toBe(s1);
    expect(withSpot(s1, 0, (x) => x)).toBe(s1);
    // 動かす: 消したい場所はコピー元と一緒に、コピー元だけも動かせる。
    const moved = moveSpot(spotsOf(s1)[0], 0.1, -0.05);
    expect(moved.points).toEqual([[0.3, 0.15]]);
    expect(moved.offset).toEqual([0.05, 0]);
    expect(moveSource(spotsOf(s1)[0], 0.01, 0.02).offset).toEqual([0.06, 0.02]);
    expect(moveSource(spotsOf(s1)[0], 5, 0).offset).toEqual([RANGES.retouchOffset.max, 0]);
    // 除く。
    expect(spotsOf(removeSpot(s2, 0))).toEqual([]);
    expect(removeSpot(s2, 5)).toBe(s2);
    expect(spotsOf(clearSpots(s2))).toEqual([]);
    expect(clearSpots(base)).toBe(base);
    // すべてリセットで除く。
    expect(spotsOf(resetAll(s2))).toEqual([]);
  });

  it("処理バージョン 1 では使えない", () => {
    const s = mockDevelopSettings();
    expect(retouchControls(s, false)).toEqual({ needsUpgrade: false, editable: true, canAdd: true });
    expect(retouchControls({ ...s, process_version: 1 }, false)).toMatchObject({ needsUpgrade: true, editable: false });
    expect(retouchControls(s, true).editable).toBe(false);
  });

  it("O 座標の表がなければ G 座標と同じとみなし、画面の位置と行き来できる", () => {
    const g = orientedGrid(undefined, [0, 0, 1, 0.75]);
    expect(g.points).toHaveLength(ORIENTED_GRID_SIZE ** 2);
    expect(fracToO(g, 0.5, 0.5)).toEqual([0.5, 0.375]);
    const box = { left: 10, top: 20, width: 400, height: 300 };
    const o = boxToO(g, box, 110, 95);
    expect(o[0]).toBeCloseTo(0.25, 9);
    expect(o[1]).toBeCloseTo(0.1875, 9);
    const p = oToBox(g, box, o[0], o[1]);
    expect(p?.[0]).toBeCloseTo(110, 6);
    expect(p?.[1]).toBeCloseTo(95, 6);
    expect(pixelsPerO(g, box, 0.5, 0.3)).toBeCloseTo(400, 6);
    // 壊れた表（点の数が違う・NaN）は使わない。
    expect(orientedGrid([[0, 0]], [0, 0, 1, 1]).points.at(-1)).toEqual([1, 1]);
  });

  it("回転した画像の表でも、O 座標と画面の位置を行き来できる（画像の外の点も）", () => {
    const g = orientedGrid(rotatedGrid(7), [0, 0, 1, 2 / 3]);
    for (const [fx, fy] of [
      [0.5, 0.5],
      [0.1, 0.9],
      [0.95, 0.02],
      [-0.2, 1.3],
    ]) {
      const o = fracToO(g, fx, fy);
      const back = oToFrac(g, o[0], o[1]);
      expect(back?.[0]).toBeCloseTo(fx, 6);
      expect(back?.[1]).toBeCloseTo(fy, 6);
    }
    // 回転では倍率は向きによらない（長辺 1 が箱の幅の画素）。
    const box = { left: 0, top: 0, width: 600, height: 400 };
    expect(pixelsPerO(g, box, 0.5, 0.33)).toBeCloseTo(600, 3);
  });

  it("Q と Delete のショートカットを一覧に載せる", () => {
    const develop = SHORTCUT_GROUPS.find((g) => g.title === "現像");
    expect(develop?.items.some((i) => i.keys.includes("q") && i.label.includes("スポット修正"))).toBe(true);
    expect(develop?.items.some((i) => i.keys.includes("Delete") && i.label.includes("スポット"))).toBe(true);
  });
});

describe("スポット修正の道具（コアとのやり取り）", () => {
  let mock: MockBridge;
  let sent: Record<string, unknown>[];
  let settings: DevelopSettings;
  let offset: [number, number] | null;

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
    settings = mockDevelopSettings();
    offset = MOCK_RETOUCH_OFFSET;
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
    ({ sent } = mockRetouchCommands(mock, { offset: () => offset }));
    appStore.set({ toasts: [] });
    viewStore.set({ tool: "none", maskIndex: null, componentIndex: null, spotIndex: null, retouch: DEFAULT_RETOUCH });
    await openDevelopFor(Math.floor(Math.random() * 1e9) + 10);
  });

  const lastSettings = async (): Promise<DevelopSettings | undefined> => {
    await enqueue(async () => undefined);
    const last = mock.calls
      .map((c) => c.args?.command as { type: string; settings?: DevelopSettings } | undefined)
      .filter((c) => c?.type === "set_settings")
      .at(-1)?.settings;
    expect(last).toEqual(developStore.get().settings);
    return last;
  };

  it("Q で道具を使い始め・終える（マスクの選択は外す）。Esc は先にスポットの選択を外す", () => {
    selectMask(0);
    toggleRetouchTool();
    expect(viewStore.get()).toMatchObject({ tool: "retouch", maskIndex: null });
    selectSpot(2);
    cancelRetouchTool();
    expect(viewStore.get()).toMatchObject({ tool: "retouch", spotIndex: null });
    cancelRetouchTool();
    expect(viewStore.get().tool).toBe("none");
    toggleRetouchTool();
    toggleRetouchTool();
    expect(viewStore.get().tool).toBe("none");
  });

  it("コピー元を待つ間の Undo・スライダーの変更を消さず、その後の設定にスポットを足す", async () => {
    mock.delay = (call) => ((call.args?.command as { type?: string })?.type === "retouch_auto_source" ? 20 : 0);
    // Undo はコアの履歴を 1 つ戻し、露光量 -1 の設定を返す（見本）。
    const undone: DevelopSettings = { ...settings, exposure_ev: -1 };
    const base = mock.responses.get("core_execute") as (a?: Record<string, unknown>) => unknown;
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as { type: string };
      if (c.type === "undo") return { type: "settings_or_none", settings: undone };
      if (c.type === "develop_state") return { type: "develop_or_none", state: null };
      return base(args);
    });
    const sent = () =>
      mock.calls
        .map((c) => c.args?.command as { type: string; settings?: DevelopSettings } | undefined)
        .filter((c) => c?.type === "set_settings")
        .map((c) => c?.settings);
    // 待つ間にスライダーを動かしてから Undo を押す。
    const adding = commitRetouchSpot([[0.3, 0.2]]);
    applySettings({ ...(developStore.get().settings as DevelopSettings), exposure_ev: 0.5 }, false);
    const undoing = undo();
    await Promise.all([adding, undoing]);
    // スポットは Undo の後の設定に足す（Undo の前の値（露光量 0.5）を送り直して Undo を消さない）。
    const s = await lastSettings();
    expect(spotsOf(s)).toHaveLength(1);
    expect(s?.exposure_ev).toBe(-1);
    expect(spotsOf(sent().at(-1))).toHaveLength(1);
    expect(viewStore.get().spotIndex).toBe(0);
  });

  it("コピー元を待つ間に写真が変わったら足さない", async () => {
    mock.delay = (call) => ((call.args?.command as { type?: string })?.type === "retouch_auto_source" ? 20 : 0);
    const adding = commitRetouchSpot([[0.3, 0.2]]);
    await openDevelopFor(Math.floor(Math.random() * 1e9) + 10);
    await adding;
    expect(spotsOf(developStore.get().settings)).toHaveLength(0);
  });

  it("コピー元を選び直す間に手で動かしたコピー元は上書きしない", async () => {
    await commitRetouchSpot([[0.3, 0.2]]);
    mock.delay = (call) => ((call.args?.command as { type?: string })?.type === "retouch_auto_source" ? 20 : 0);
    offset = [0, 0.04];
    const reselecting = reselectSource(0);
    const moved = withSpot(developStore.get().settings as DevelopSettings, 0, (r) => moveSource(r, 0.01, 0));
    applySettings(moved, false);
    await reselecting;
    const s = await lastSettings();
    expect(spotsOf(s)[0].offset).toEqual(spotsOf(moved)[0].offset);
    expect(spotsOf(s)[0].offset).not.toEqual([0, 0.04]);
  });

  it("処理バージョン 1 の写真では使い始めずに知らせる", () => {
    developStore.set({ settings: { ...settings, process_version: 1 } });
    toggleRetouchTool();
    expect(viewStore.get().tool).toBe("none");
    expect(appStore.get().toasts.at(-1)?.message).toContain("スポット修正は使われません");
  });

  it("描いたスポットは、コアが選んだコピー元で足して選ぶ（選べなければ横にずらす）", async () => {
    toggleRetouchTool();
    await commitRetouchSpot([[0.3, 0.2]]);
    expect(sent[0]).toMatchObject({ type: "retouch_auto_source", spot: { points: [[0.3, 0.2]], offset: [0, 0] } });
    let s = await lastSettings();
    expect(spotsOf(s)).toHaveLength(1);
    expect(spotsOf(s)[0]).toMatchObject({ mode: "heal", size: DEFAULT_RETOUCH.size, offset: MOCK_RETOUCH_OFFSET });
    expect(viewStore.get().spotIndex).toBe(0);
    expect(currentSpot(developStore.get().settings, viewStore.get())?.index).toBe(0);
    // コアが候補を返さなければ、大きさの 2.5 倍だけ横に。線（2 点）。
    offset = null;
    await commitRetouchSpot([
      [0.8, 0.5],
      [0.9, 0.55],
    ]);
    s = await lastSettings();
    expect(spotsOf(s)[1].points).toHaveLength(2);
    expect(spotsOf(s)[1].offset).toEqual([-0.025, 0]);
    expect(viewStore.get().spotIndex).toBe(1);
    // コピー元を選び直す（候補がなければ知らせて変えない）。
    await reselectSource(1);
    await enqueue(async () => undefined);
    expect(appStore.get().toasts.at(-1)?.message).toContain("よいコピー元が見つかりません");
    offset = [0, 0.04];
    await reselectSource(1);
    s = await lastSettings();
    expect(spotsOf(s)[1].offset).toEqual([0, 0.04]);
  });
});
