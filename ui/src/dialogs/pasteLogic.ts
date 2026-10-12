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

/** 貼り付け先・コピー元を調べるのに使う、読み込み済みの項目の一部。 */
export type LoadedItem = Pick<VariantSummary, "variant_id" | "kind" | "file_name">;

/**
 * 読み込み済みの項目を 1 回だけ順に見て、`ids` の項目を集める（全部見つかったら途中でやめる）。
 *
 * 項目ごとに検索結果のキャッシュ全体を探す（searchCache.getLoadedById）と、対象の数 × 読み込み済みの件数に
 * なり、大きなカタログを大きくスクロールした後では画面が止まる。1 回の走査なら、読み込み済みの件数 ＋
 * 対象の数で済む。
 */
export function collectLoaded(
  ids: ReadonlySet<VariantId>,
  loaded: Iterable<LoadedItem>,
): Map<VariantId, LoadedItem> {
  const found = new Map<VariantId, LoadedItem>();
  if (ids.size === 0) return found;
  for (const item of loaded) {
    if (ids.has(item.variant_id) && !found.has(item.variant_id)) {
      found.set(item.variant_id, item);
      if (found.size === ids.size) break;
    }
  }
  return found;
}

/**
 * 貼り付け先を整える: コピー元の写真と、読み込み済みの検索結果で動画と分かったものを除く（動画には現像設定を
 * 保存しない。コアは動画が含まれていると何も保存せずにエラーにする）。コピー元の概要（読み込み済みなら）も
 * 返す。
 *
 * `loaded` は読み込み済みの項目（loadedItems.ts の loadedSummaries）。読み込んでいない項目の種類は分からない
 * ので除かない（動画が含まれていれば、コアのエラーで知らせる）。
 */
export function preparePasteTargets(
  targets: readonly VariantId[],
  from: VariantId | null,
  loaded: Iterable<LoadedItem>,
): { ids: VariantId[]; skippedSource: boolean; skippedVideos: number; source: LoadedItem | undefined } {
  const unique = [...new Set(targets)];
  const withoutSource = from === null ? unique : unique.filter((id) => id !== from);
  const skippedSource = withoutSource.length !== unique.length;
  const wanted = new Set(withoutSource);
  if (from !== null) wanted.add(from);
  const found = collectLoaded(wanted, loaded);
  const ids = withoutSource.filter((id) => found.get(id)?.kind !== "video");
  return {
    ids,
    skippedSource,
    skippedVideos: withoutSource.length - ids.length,
    source: from === null ? undefined : found.get(from),
  };
}
