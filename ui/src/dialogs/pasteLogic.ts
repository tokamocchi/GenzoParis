// 現像設定の一括適用（DEV-30）のダイアログで選ぶ項目（DOM を使わない純粋な計算）。
//
// 画面で選ぶのは 5 つ。レンズ補正・ディテール・部分補正（genzo-api の SettingGroups の lens・detail・masks）は
// MVP の画面で扱わないので、常に写す（true）。

import type { SettingGroups, VariantId, VariantSummary } from "../api/types";

export type PasteGroupKey = "white_balance" | "basic_tone" | "tone_curve" | "color" | "geometry";

export type PasteSelection = Record<PasteGroupKey, boolean>;

export const PASTE_GROUP_ITEMS: { key: PasteGroupKey; label: string; detail: string }[] = [
  { key: "white_balance", label: "ホワイトバランス", detail: "色温度・色かぶり補正" },
  {
    key: "basic_tone",
    label: "基本補正",
    detail: "露光量・コントラスト・ハイライト・シャドウ・白レベル・黒レベル",
  },
  { key: "tone_curve", label: "トーンカーブ", detail: "パラメトリック・ポイントカーブ" },
  { key: "color", label: "色", detail: "彩度・自然な彩度" },
  { key: "geometry", label: "切り抜きと角度", detail: "写真ごとに構図が違うことが多いため、既定では写しません" },
];

/** 既定の選択（切り抜きと角度だけオフ）。 */
export const DEFAULT_PASTE_SELECTION: PasteSelection = {
  white_balance: true,
  basic_tone: true,
  tone_curve: true,
  color: true,
  geometry: false,
};

/** 保存した選択を読む（壊れた項目は既定値）。 */
export function restorePasteSelection(saved: unknown): PasteSelection {
  const out = { ...DEFAULT_PASTE_SELECTION };
  if (typeof saved !== "object" || saved === null) return out;
  for (const { key } of PASTE_GROUP_ITEMS) {
    const v = (saved as Record<string, unknown>)[key];
    if (typeof v === "boolean") out[key] = v;
  }
  return out;
}

/** コアに渡す SettingGroups（画面に出さない項目は true）。 */
export function toSettingGroups(sel: PasteSelection): SettingGroups {
  return { ...sel, lens: true, detail: true, masks: true };
}

export function anySelected(sel: PasteSelection): boolean {
  return PASTE_GROUP_ITEMS.some(({ key }) => sel[key]);
}

/** 読み込み済みの検索結果から種類を調べる件数の上限（仮置き: 1,000 件。多いと探すのに時間がかかる）。 */
export const KIND_LOOKUP_LIMIT = 1000;

/**
 * 貼り付け先を整える: コピー元の写真と、読み込み済みの検索結果で動画と分かったものを除く（動画には現像設定を
 * 保存しない。コアは動画が含まれていると何も保存せずにエラーにする）。
 *
 * `lookup` は読み込み済みの項目を id で探す関数（searchCache.getLoadedById）。件数が多いときは探さない
 * （動画が含まれていれば、コアのエラーで知らせる）。
 */
export function preparePasteTargets(
  targets: readonly VariantId[],
  from: VariantId | null,
  lookup: (id: VariantId) => Pick<VariantSummary, "kind"> | undefined,
): { ids: VariantId[]; skippedSource: boolean; skippedVideos: number } {
  const unique = [...new Set(targets)];
  const withoutSource = from === null ? unique : unique.filter((id) => id !== from);
  const skippedSource = withoutSource.length !== unique.length;
  if (withoutSource.length > KIND_LOOKUP_LIMIT) return { ids: withoutSource, skippedSource, skippedVideos: 0 };
  const ids = withoutSource.filter((id) => lookup(id)?.kind !== "video");
  return { ids, skippedSource, skippedVideos: withoutSource.length - ids.length };
}
