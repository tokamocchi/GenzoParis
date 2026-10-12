import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { ApiError } from "../api/errors";
import { MockBridge } from "../api/mock";
import type { DevelopState, ErrorInfo } from "../api/types";
import { developStore } from "../state/develop";
import { OUTPUT_SIZE_UNKNOWN, resolveOutputSize } from "./outputSize";

/** genzo-app の frames.rs の形（[ヘッダーの長さ][ヘッダーの JSON][RGBA]）にする。 */
function encodeFrame(header: Record<string, unknown> & { width: number; height: number }): ArrayBuffer {
  const json = new TextEncoder().encode(JSON.stringify(header));
  const pixels = header.width * header.height * 4;
  const buf = new ArrayBuffer(4 + json.length + pixels);
  new DataView(buf).setUint32(0, json.length, true);
  new Uint8Array(buf, 4, json.length).set(json);
  return buf;
}

function state(id: number): DevelopState {
  return {
    variant_id: id,
    settings: {} as DevelopState["settings"],
    generation: 1,
    can_undo: false,
    can_redo: false,
    dirty: false,
    dragging: false,
    source: { is_raw: true, width: 7008, height: 4672, decoder: null, as_shot_white_balance: null },
  };
}

const frameHeader = { variant_id: 7, generation: 1, quality: "final", width: 1, height: 1 };

describe("等倍表示の出力の大きさ", () => {
  let mock: MockBridge;
  let developFrame: ArrayBuffer;
  let renderDevelopInfo: Record<string, unknown>;
  let regionFull: { full_width: number; full_height: number };
  let openError: ErrorInfo | null;

  beforeEach(() => {
    developStore.set({ variantId: null, state: null, settings: null, opening: false, error: null, frame: null });
    mock = new MockBridge();
    installBridge(mock);
    developFrame = new ArrayBuffer(0);
    renderDevelopInfo = { ...frameHeader };
    regionFull = { full_width: 7008, full_height: 4672 };
    openError = null;
    mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
      const c = args?.command as { type: string; variant_id?: number };
      switch (c.type) {
        case "open_develop":
          if (openError) return new ApiError(openError);
          return { type: "develop", ...state(c.variant_id ?? 1) };
        case "render_develop":
          return { type: "frame", ...renderDevelopInfo };
        case "render_region":
          return { type: "region", variant_id: 7, request: 1, ...regionFull };
        default:
          return { type: "ok" };
      }
    });
    mock.responses.set("develop_frame", () => developFrame);
    mock.responses.set("region_frame", () =>
      encodeFrame({ variant_id: 7, request: 1, ...regionFull, width: 1, height: 1 }),
    );
  });

  it("現像のプレビューに出力の大きさがあれば、それを使う", async () => {
    developFrame = encodeFrame({ ...frameHeader, output_width: 6000, output_height: 4000 });
    const r = await resolveOutputSize(7, () => false, 500);
    expect(r).toEqual({ kind: "size", width: 6000, height: 4000 });
    expect(mock.coreCommands()).not.toContain("render_region");
    expect(mock.coreCommands()).not.toContain("render_develop");
  });

  it("プレビューが届かなければ render_develop の応答を使う", async () => {
    renderDevelopInfo = { ...frameHeader, output_width: 5000, output_height: 3000 };
    const r = await resolveOutputSize(7, () => false, 20);
    expect(r).toEqual({ kind: "size", width: 5000, height: 3000 });
    expect(mock.coreCommands()).toContain("render_develop");
    expect(mock.coreCommands()).not.toContain("render_region");
  });

  it("古いコア（値がない）では、表示範囲の描画の応答の全体の寸法を使う", async () => {
    developFrame = encodeFrame({ ...frameHeader });
    const r = await resolveOutputSize(7, () => false, 500);
    expect(r).toEqual({ kind: "size", width: 7008, height: 4672 });
    expect(mock.coreCommands()).toContain("render_region");
  });

  it("切り抜きの編集中のプレビューの値は使わない", async () => {
    developFrame = encodeFrame({ ...frameHeader, output_width: 9000, output_height: 6000, uncropped: true });
    const r = await resolveOutputSize(7, () => false, 500);
    expect(r).toEqual({ kind: "size", width: 7008, height: 4672 });
  });

  it("どれでも分からなければ使えない理由を返す", async () => {
    developFrame = encodeFrame({ ...frameHeader });
    regionFull = { full_width: 0, full_height: 0 };
    const r = await resolveOutputSize(7, () => false, 500);
    expect(r).toEqual({ kind: "unavailable", reason: OUTPUT_SIZE_UNKNOWN });
  });

  it("写真を開けなければ、そのエラーにする", async () => {
    openError = {
      kind: "file_access",
      message: "ファイルを読めません",
      user_actionable: true,
      retryable: false,
      hint: null,
    };
    await expect(resolveOutputSize(7, () => false, 20)).rejects.toMatchObject({ info: { kind: "file_access" } });
  });

  it("要求が古くなったら何もしない", async () => {
    const r = await resolveOutputSize(7, () => true, 20);
    expect(r).toEqual({ kind: "superseded" });
    expect(mock.coreCommands()).not.toContain("render_region");
  });
});
