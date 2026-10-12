// 切り抜きの確定（commitCrop）と、写真の切り替えの競合のテスト（PR #2 の Codex の指摘）。

import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MockBridge } from "../api/mock";
import type { DevelopSettings, DevelopState } from "../api/types";
import { openDevelopFor } from "../state/develop";
import { commitCrop, startCrop, viewStore } from "./viewState";

const crop = { left: 0, top: 0, right: 1, bottom: 1 };

function settings(): DevelopSettings {
  return { exposure_ev: 0, geometry: { crop, angle_deg: 0, aspect: null } } as unknown as DevelopSettings;
}

function state(id: number): DevelopState {
  return {
    variant_id: id,
    settings: settings(),
    generation: 1,
    can_undo: false,
    can_redo: false,
    dirty: false,
    dragging: false,
    source: { is_raw: true, width: 10, height: 10, decoder: null, as_shot_white_balance: null },
  };
}

describe("切り抜きの確定と写真の切り替え", () => {
  let mock: MockBridge;
  beforeEach(() => {
    mock = new MockBridge();
    installBridge(mock);
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as { type: string; variant_id?: number; crop?: unknown };
      switch (c.type) {
        case "open_develop":
          return { type: "develop", ...state(c.variant_id ?? 1) };
        case "constrain_crop":
          return { type: "crop", crop: c.crop };
        default:
          return { type: "ok" };
      }
    });
    mock.responses.set("develop_frame", new ArrayBuffer(0));
    viewStore.set({ tool: "none", crop: null, zoom: null });
  });

  it("確定の途中で別の写真に移って編集を始めたら、その写真の表示を切り抜いた表示に戻さない", async () => {
    await openDevelopFor(1);
    await startCrop();
    // 確定の制限（constrain_crop）の往復を遅くして、その間に写真を移る。
    mock.delay = (c) => ((c.args?.command as { type: string } | undefined)?.type === "constrain_crop" ? 30 : 0);
    const committing = commitCrop();
    await openDevelopFor(2);
    await startCrop();
    await committing;
    const views = mock.calls
      .map((c) => c.args?.command as { type: string; uncropped?: boolean } | undefined)
      .filter((c) => c?.type === "set_develop_view")
      .map((c) => c?.uncropped);
    // 写真 1 の編集の開始（true）、写真 2 の編集の開始（true）の後に、false（切り抜いた表示）が来ない。
    expect(views.at(-1)).toBe(true);
    expect(viewStore.get().crop?.variantId).toBe(2);
  });

  it("同じ写真のままなら、確定の後に切り抜いた表示に戻す", async () => {
    await openDevelopFor(1);
    await startCrop();
    await commitCrop();
    const views = mock.calls
      .map((c) => c.args?.command as { type: string; uncropped?: boolean } | undefined)
      .filter((c) => c?.type === "set_develop_view")
      .map((c) => c?.uncropped);
    expect(views).toEqual([true, false]);
  });
});
