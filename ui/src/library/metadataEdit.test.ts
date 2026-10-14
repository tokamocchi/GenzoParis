import { describe, expect, it } from "vitest";

import { MOCK_IPTC, MOCK_METADATA_SUMMARY } from "../api/mock";
import type { MetadataField } from "../api/types";
import {
  COPYRIGHT_STATUS_OPTIONS,
  MAX_METADATA_CHARS,
  METADATA_FIELDS,
  METADATA_GROUPS,
  batchNote,
  emptyStates,
  fieldHint,
  filledCount,
  idsKey,
  normalizeDraft,
  statesFromMetadata,
  statesFromSummary,
  tooLong,
} from "./metadataEdit";

/** genzo-model の MetadataField::ALL と同じ並び。 */
const CORE_ORDER: MetadataField[] = [
  "title",
  "headline",
  "caption",
  "creator",
  "creator_job_title",
  "creator_address",
  "creator_city",
  "creator_region",
  "creator_postal_code",
  "creator_country",
  "creator_phone",
  "creator_email",
  "creator_url",
  "copyright",
  "copyright_status",
  "usage_terms",
  "copyright_url",
  "sublocation",
  "city",
  "province_state",
  "country",
  "country_code",
  "credit",
  "source",
  "instructions",
];

describe("説明のメタデータの項目", () => {
  it("コアの項目と同じものを同じ並びで持つ", () => {
    expect(METADATA_FIELDS.map((d) => d.field)).toEqual(CORE_ORDER);
    // 複数行・検索の対象はコアの multiline・searchable と同じ。
    expect(METADATA_FIELDS.filter((d) => d.multiline).map((d) => d.field)).toEqual([
      "caption",
      "creator_address",
      "usage_terms",
      "instructions",
    ]);
    expect(METADATA_FIELDS.filter((d) => d.searchable).map((d) => d.field)).toEqual([
      "title",
      "headline",
      "caption",
      "creator",
      "copyright",
      "sublocation",
      "city",
      "province_state",
      "country",
      "credit",
      "source",
    ]);
    expect(METADATA_GROUPS.map((g) => g.id)).toEqual(["content", "creator", "copyright", "location", "status"]);
    expect(COPYRIGHT_STATUS_OPTIONS.map((o) => o.value)).toEqual(["", "copyrighted", "public_domain"]);
    expect(fieldHint(METADATA_FIELDS[0])).toContain("テキスト検索");
    expect(fieldHint(METADATA_FIELDS.find((d) => d.field === "creator_email")!)).toBeUndefined();
  });

  it("1 枚の詳細と、複数のまとめから表示の状態を作る", () => {
    const one = statesFromMetadata(MOCK_IPTC, "古いキャプション");
    expect(one.title).toEqual({ value: "清水寺の朝", mixed: false });
    expect(one.headline).toEqual({ value: null, mixed: false });
    expect(one.caption.value).toBe("朝の清水の舞台\n京都");
    expect(one.instructions).toEqual({ value: null, mixed: false });
    // 古いコア（metadata がない）ではキャプションだけ。
    const old = statesFromMetadata(undefined, "夕焼け");
    expect(old.caption.value).toBe("夕焼け");
    expect(old.title.value).toBeNull();

    const many = statesFromSummary(MOCK_METADATA_SUMMARY);
    expect(many.title).toEqual({ value: null, mixed: true });
    expect(many.creator).toEqual({ value: "山田 太郎", mixed: false });
    // まとめにない項目は値なし。
    expect(many.source).toEqual({ value: null, mixed: false });
    expect(Object.keys(emptyStates())).toHaveLength(CORE_ORDER.length);
    expect(filledCount(many, METADATA_GROUPS[0])).toBe(2);
    expect(filledCount(one, METADATA_GROUPS[1])).toBe(2);
  });

  it("保存する値は前後の空白を除き、空なら削除", () => {
    expect(normalizeDraft("  清水寺  ")).toBe("清水寺");
    expect(normalizeDraft(" \n ")).toBeNull();
    expect(normalizeDraft("一行目\n二行目")).toBe("一行目\n二行目");
    expect(tooLong("あ".repeat(MAX_METADATA_CHARS))).toBe(false);
    expect(tooLong(` ${"あ".repeat(MAX_METADATA_CHARS)} `)).toBe(false);
    expect(tooLong("あ".repeat(MAX_METADATA_CHARS + 1))).toBe(true);
    // サロゲートペア（絵文字）は 1 文字に数える（コアの Unicode のスカラー値の数と同じ）。
    expect(tooLong("😀".repeat(MAX_METADATA_CHARS))).toBe(false);
  });

  it("対象の鍵と一括編集の説明", () => {
    expect(idsKey([1, 2, 3])).toBe(idsKey([1, 2, 3]));
    expect(idsKey([1, 2, 3])).not.toBe(idsKey([1, 3, 2]));
    expect(idsKey([])).toBe("0:::811c9dc5");
    const many = Array.from({ length: 10_000 }, (_, i) => i + 1);
    expect(idsKey(many).length).toBeLessThan(40);
    expect(batchNote(3, 3)).toBe("選択中の 3 枚に書きます。");
    expect(batchNote(4, 3)).toContain("元の写真 3 件");
    expect(batchNote(2, null)).toBe("選択中の 2 枚に書きます。");
  });
});
