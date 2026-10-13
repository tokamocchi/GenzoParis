import { describe, expect, it } from "vitest";

import type { VariantDetails, VariantSummary } from "../api/types";
import {
  INFO_DEFAULTS,
  INFO_ITEMS,
  activeItems,
  cycleInfoMode,
  infoLines,
  infoModeLabel,
  infoStore,
  itemText,
  needsDetails,
  nextInfoMode,
  parseInfoMode,
  parseInfoSets,
  setInfoSets,
  toggleInfoItem,
} from "./infoItems";

function summary(over: Partial<VariantSummary> = {}): VariantSummary {
  return {
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
    rating: 3,
    flag: 0,
    color_label: null,
    capture: {
      raw: "2024:05:01 12:00:00",
      offset: "+09:00",
      tz_source: "exif",
      tz_assumed: "+09:00",
      correction_s: 0,
      utc: "2024-05-01T03:00:00.000Z",
    },
    camera: "SONY ILCE-7M4",
    lens: "FE 24-70mm F2.8 GM II",
    width: 7008,
    height: 4672,
    orientation: 1,
    develop_hash: "x",
    thumb_rev: null,
    ...over,
  } as VariantSummary;
}

function details(over: Partial<VariantDetails> = {}): VariantDetails {
  return {
    summary: summary(),
    iso: 800,
    aperture: 2.8,
    shutter_s: 1 / 250,
    focal_mm: 35,
    gps: null,
    caption: "夕焼け",
    metadata: { title: "清水寺", caption: "夕焼け" },
    video: null,
    files: [],
    ...over,
  };
}

describe("撮影情報の表示の切り替え", () => {
  it("I で 情報 1 → 情報 2 → なし → 情報 1", () => {
    expect(nextInfoMode(1)).toBe(2);
    expect(nextInfoMode(2)).toBe(0);
    expect(nextInfoMode(0)).toBe(1);
    expect(infoModeLabel(0)).toContain("なし");
    expect(infoModeLabel(2)).toContain("情報 2");
  });

  it("保存した表示を読む（MVP の表示・非表示も読み替える）", () => {
    expect(parseInfoMode("2", null)).toBe(2);
    expect(parseInfoMode("0", "1")).toBe(0);
    expect(parseInfoMode(null, "0")).toBe(0);
    expect(parseInfoMode(null, "1")).toBe(1);
    expect(parseInfoMode(null, null)).toBe(1);
    expect(parseInfoMode("x", null)).toBe(1);
  });

  it("保存した項目を読む（壊れた・知らない値は除く）", () => {
    expect(parseInfoSets(null)).toBe(INFO_DEFAULTS);
    expect(parseInfoSets("{")).toBe(INFO_DEFAULTS);
    expect(parseInfoSets(JSON.stringify({ info1: ["iso", "nope", "iso", 3], info2: "x" }))).toEqual({
      info1: ["iso"],
      info2: INFO_DEFAULTS.info2,
    });
    expect(toggleInfoItem(["iso", "file_name"], "camera", true)).toEqual(["file_name", "camera", "iso"]);
    expect(toggleInfoItem(["iso", "file_name"], "iso", false)).toEqual(["file_name"]);
    expect(needsDetails(["file_name", "camera"])).toBe(false);
    expect(needsDetails(["iso"])).toBe(true);
    expect(INFO_ITEMS.length).toBeGreaterThanOrEqual(13);
  });

  it("状態を切り替え、項目を変える", () => {
    infoStore.set({ mode: 1, sets: INFO_DEFAULTS });
    expect(activeItems(infoStore.get())).toBe(INFO_DEFAULTS.info1);
    expect(cycleInfoMode()).toBe(2);
    expect(activeItems(infoStore.get())).toBe(INFO_DEFAULTS.info2);
    expect(cycleInfoMode()).toBe(0);
    expect(activeItems(infoStore.get())).toEqual([]);
    setInfoSets({ info1: ["iso"], info2: [] });
    expect(infoStore.get().sets.info1).toEqual(["iso"]);
    infoStore.set({ mode: 1, sets: INFO_DEFAULTS });
  });
});

describe("撮影情報の行", () => {
  it("情報 1 の既定: 名前・日時・寸法・印・位置", () => {
    const lines = infoLines(INFO_DEFAULTS.info1, { summary: summary(), details: null, index: 4, count: 120 });
    expect(lines).toEqual([
      { kind: "text", item: "file_name", text: "DSC00001.ARW", strong: true, muted: false },
      { kind: "text", item: "capture_time", text: "2024-05-01 12:00:00", strong: false, muted: false },
      { kind: "text", item: "dimensions", text: "7,008 × 4,672", strong: false, muted: false },
      { kind: "marks" },
      { kind: "text", item: "position", text: "5 / 120", strong: false, muted: true },
    ]);
  });

  it("露出の項目は 1 行にまとめ、詳細が届くまでは …", () => {
    const items = INFO_DEFAULTS.info2;
    const waiting = infoLines(items, { summary: summary(), details: null, index: null, count: 0 });
    expect(waiting.map((l) => (l.kind === "text" ? l.text : "marks"))).toEqual([
      "DSC00001.ARW",
      "…",
      "SONY ILCE-7M4",
      "FE 24-70mm F2.8 GM II",
    ]);
    const ready = infoLines(items, { summary: summary(), details: details(), index: null, count: 0 });
    expect(ready[1]).toMatchObject({ kind: "text", text: "1/250 秒・f/2.8・ISO 800・35 mm" });
  });

  it("寸法は向きを反映し、現像の画面では切り抜いた後の大きさ", () => {
    const s = summary({ orientation: 6 });
    expect(itemText("dimensions", { summary: s, details: null, index: null, count: 0 })).toBe("4,672 × 7,008");
    expect(
      itemText("dimensions", { summary: s, details: null, index: null, count: 0, dimensions: { width: 3000, height: 2000 } }),
    ).toBe("3,000 × 2,000");
    expect(itemText("dimensions", { summary: summary({ width: null }), details: null, index: null, count: 0 })).toBeNull();
  });

  it("仮想コピーの名前・タイトル・キャプション・値のない項目は飛ばす", () => {
    const vc = summary({ is_master: false, variant_name: "白黒" });
    expect(itemText("file_name", { summary: vc, details: null, index: null, count: 0 })).toBe("DSC00001.ARW（仮想コピー: 白黒）");
    const src = { summary: summary(), details: details(), index: null, count: 0 };
    expect(itemText("title", src)).toBe("清水寺");
    expect(itemText("caption", src)).toBe("夕焼け");
    // 別の写真の詳細は使わない。
    const other = details({ summary: summary({ variant_id: 2 }) });
    expect(itemText("iso", { ...src, details: other })).toBe("…");
    // 値のない項目（レンズなし・ISO なし）は行にしない。
    const lines = infoLines(["lens", "iso"], {
      summary: summary({ lens: null }),
      details: details({ iso: null, shutter_s: null }),
      index: null,
      count: 0,
    });
    expect(lines).toEqual([]);
  });
});
