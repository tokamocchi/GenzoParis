// v1 の view の画面の部品（説明のメタデータの編集 LIB-15・撮影情報 LIB-17・ビフォー / アフター DEV-31・設定の
// 「表示」）が、代表的な状態で例外なく描けることの確認（react-dom/server で文字列にする。useEffect は動かない）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";

import { decodeFrame } from "../api/frames";
import { MOCK_BEFORE_INFO, MOCK_IPTC, MOCK_METADATA_SUMMARY, mockBeforeFrame } from "../api/mock";
import type { DevelopState, FrameInfo, VariantSummary } from "../api/types";
import { SettingsView } from "../dialogs/SettingsView";
import { DevelopView } from "../develop/DevelopView";
import { viewStore } from "../develop/viewState";
import { appStore } from "../state/app";
import { developStore } from "../state/develop";
import { InfoOverlay } from "../viewer/InfoOverlay";
import { INFO_DEFAULTS, infoStore } from "../viewer/infoItems";
import { MetadataEditor } from "./MetadataEditor";
import { batchNote, statesFromMetadata, statesFromSummary } from "./metadataEdit";

function render(el: ReturnType<typeof createElement>): string {
  return renderToString(el).replace(/<!-- -->/g, "");
}

const summary = {
  variant_id: 1,
  asset_id: 1,
  is_master: true,
  variant_name: null,
  kind: "photo",
  file_id: 1,
  file_name: "DSC00001.ARW",
  folder_id: 1,
  file_status: "ok",
  file_revision: 1,
  has_sidecar_jpeg: false,
  rating: 4,
  flag: 1,
  color_label: "red",
  capture: { raw: null, offset: null, tz_source: "user_default", tz_assumed: "+09:00", correction_s: 0, utc: null },
  camera: "SONY ILCE-7M4",
  lens: null,
  width: 6000,
  height: 4000,
  orientation: 1,
  develop_hash: "x",
  thumb_rev: null,
} as unknown as VariantSummary;

afterEach(() => {
  infoStore.set({ mode: 1, sets: INFO_DEFAULTS });
  viewStore.set({ compare: "off" });
});

describe("説明のメタデータの編集（LIB-15）", () => {
  it("1 枚: 値と節（値のある数）", () => {
    const html = render(
      createElement(MetadataEditor, { targets: [1], states: statesFromMetadata(MOCK_IPTC, null), disabled: false }),
    );
    expect(html).toContain("タイトルとキャプション（2）");
    expect(html).toContain("清水寺の朝");
    expect(html).toContain("山田 太郎");
    expect(html).toContain("著作権あり");
    expect(html).toContain("撮影場所（2）");
    expect(html).toContain('type="email"');
  });

  it("複数: 異なる値は「複数の値」、一括の説明", () => {
    const html = render(
      createElement(MetadataEditor, {
        targets: [1, 2, 3, 4],
        states: statesFromSummary(MOCK_METADATA_SUMMARY),
        disabled: false,
        note: batchNote(4, 3),
      }),
    );
    expect(html).toContain("（複数の値）");
    expect(html).toContain("元の写真 3 件");
    expect(html).toContain("山田 太郎");
  });
});

describe("撮影情報（LIB-17）", () => {
  it("情報 1 と 情報 2・なし", () => {
    const one = render(createElement(InfoOverlay, { summary, index: 0, count: 10 }));
    expect(one).toContain("DSC00001.ARW");
    expect(one).toContain("6,000 × 4,000");
    expect(one).toContain("1 / 10");
    infoStore.set({ mode: 2 });
    const two = render(createElement(InfoOverlay, { summary, index: 0, count: 10 }));
    expect(two).toContain("SONY ILCE-7M4");
    expect(two).toContain("…");
    infoStore.set({ mode: 0 });
    expect(render(createElement(InfoOverlay, { summary, index: 0, count: 10 }))).toBe("");
  });

  it("設定の「表示」に項目の表がある", () => {
    const html = render(createElement(SettingsView));
    expect(html).toContain("ルーペ上の撮影情報");
    expect(html).toContain("シャッター速度");
    expect(html).toContain("既定に戻す");
  });
});

describe("ビフォー / アフター（DEV-31）", () => {
  it("現像の画面の下のツールバーと、左右に並べた面", () => {
    const state = {
      variant_id: 1,
      settings: { geometry: { crop: { left: 0, top: 0, right: 1, bottom: 1 }, angle_deg: 0, aspect: null } },
      generation: 2,
      can_undo: false,
      can_redo: false,
      dirty: false,
      dragging: false,
      source: { is_raw: true, width: 40, height: 30, decoder: null, as_shot_white_balance: null },
      before: MOCK_BEFORE_INFO,
    } as unknown as DevelopState;
    appStore.set((s) => ({ selection: { ...s.selection, current: 1, currentIndex: 0 }, dialog: null }));
    developStore.set({
      variantId: 1,
      state,
      settings: state.settings,
      opening: false,
      error: null,
      before: MOCK_BEFORE_INFO,
      beforeFrame: decodeFrame<FrameInfo>(mockBeforeFrame(1, MOCK_BEFORE_INFO.generation)),
    });
    viewStore.set({ compare: "side_by_side" });
    const html = render(createElement(DevelopView));
    expect(html).toContain("ビフォー: 読み込み");
    expect(html).toContain("アフターの設定をビフォーに");
    expect(html).toContain("左右");
    expect(html).toContain("ビフォー（読み込み）");
    expect(html).toContain('data-target="before"');
    expect(html).toContain("撮影情報: 情報 1");
    viewStore.set({ compare: "off" });
    const off = render(createElement(DevelopView));
    expect(off).not.toContain("アフターの設定をビフォーに");
    expect(off).not.toContain('data-target="before"');
    developStore.set({ variantId: null, state: null, settings: null, before: null, beforeFrame: null });
    appStore.set((s) => ({ selection: { ...s.selection, current: null, currentIndex: null } }));
  });
});
