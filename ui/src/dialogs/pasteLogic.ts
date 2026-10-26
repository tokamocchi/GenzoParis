// 現像設定の一括適用（DEV-30）のダイアログで選ぶ項目（DOM を使わない純粋な計算）。
//
// 画面で選ぶのは 12（genzo-api の SettingGroups のすべて）。マスク（ローカル補正。DEV-18・DEV-19）と変形（遠近補正。
// DEV-16）は、位置・傾きが写真ごとに違うことが多いので、切り抜きと角度と同じく既定では写さない（前の版の保存した選択に
// 無ければ写さない。AI の仮決定）。
// カメラプロファイル・ルック（DEV-17）も選べる（既定は写す。前の版の保存した選択に無ければ写す）。
// レンズ補正（DEV-14）とディテール（シャープ・ノイズ軽減。DEV-12・DEV-13）・外観（テクスチャ・明瞭度・かすみの除去。
// DEV-11）・効果（切り抜き後の周辺光量補正・粒子。DEV-24）は現像のパネルで扱うので選べる（既定は写す。前の版の
// 保存した選択に無ければ写す）。ハイライト復元（DEV-06）は基本補正に含める（genzo-api の basic_tone）。
//
// 処理バージョン（DATA-09）: 貼り付けは、貼り付け先の処理バージョンを変えない（版 1 と 2 の写真の間でも、選んだ
// 項目だけを写せる）。「処理バージョンが古い写真は更新する」を選んだときだけ、貼り付け元より古い写真を貼り付け元の
// 版に上げる（既定はオフ。保存しない）。版 1 の写真に写した v1 の機能の項目（レンズ補正・ハイライト復元・ノイズ軽減・
// カメラプロファイル・ルック・カラーミキサー・テクスチャ・明瞭度・シャープ・効果・マスクなど）は、更新するまで
// 使われないので、貼り付けた後に数を知らせる（describePasteReport）。

import type { PasteReport, SettingGroups, VariantId, VariantSummary } from "../api/types";

export type PasteGroupKey =
  | "white_balance"
  | "basic_tone"
  | "presence"
  | "tone_curve"
  | "color"
  | "detail"
  | "lens"
  | "effects"
  | "profile"
  | "masks"
  | "geometry"
  | "transform";

export type PasteSelection = Record<PasteGroupKey, boolean>;

export const PASTE_GROUP_ITEMS: { key: PasteGroupKey; label: string; detail: string }[] = [
  { key: "white_balance", label: "ホワイトバランス", detail: "色温度・色かぶり補正" },
  {
    key: "basic_tone",
    label: "基本補正",
    detail: "露光量・コントラスト・ハイライト・シャドウ・白レベル・黒レベル・ハイライト復元",
  },
  { key: "presence", label: "外観", detail: "テクスチャ・明瞭度・かすみの除去" },
  { key: "tone_curve", label: "トーンカーブ", detail: "パラメトリック・ポイントカーブ" },
  { key: "color", label: "色", detail: "彩度・自然な彩度・カラーミキサー・カラーグレーディング・白黒" },
  { key: "detail", label: "ディテール", detail: "シャープ・ノイズ軽減（輝度・カラー）" },
  { key: "lens", label: "レンズ補正", detail: "プロファイルによる補正・色収差の除去・手動の歪曲 / 周辺減光 / 色収差" },
  { key: "effects", label: "効果", detail: "切り抜き後の周辺光量補正・粒子" },
  { key: "profile", label: "プロファイル・ルック", detail: "カメラプロファイル・ルック（3D LUT）・ルックの適用量" },
  {
    key: "masks",
    label: "マスク",
    detail: "ローカル補正（ブラシ・グラデーション・色域・輝度範囲と調整）。写真ごとに位置が違うことが多いため、既定では写しません",
  },
  { key: "geometry", label: "切り抜きと角度", detail: "写真ごとに構図が違うことが多いため、既定では写しません" },
  {
    key: "transform",
    label: "変形",
    detail: "遠近補正（垂直・水平・縦横比・拡大縮小・ずらし）。写真ごとに傾きが違うため、既定では写しません",
  },
];

/** 既定の選択（マスクと、切り抜きと角度はオフ）。 */
export const DEFAULT_PASTE_SELECTION: PasteSelection = {
  white_balance: true,
  basic_tone: true,
  presence: true,
  tone_curve: true,
  color: true,
  detail: true,
  lens: true,
  effects: true,
  profile: true,
  masks: false,
  geometry: false,
  transform: false,
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

/** コアに渡す SettingGroups。 */
export function toSettingGroups(sel: PasteSelection): SettingGroups {
  return { ...sel };
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

/**
 * v1 の機能（レンズ補正・ハイライト復元・ノイズ軽減・カメラプロファイル・ルック・カラーミキサー・カラーグレーディング・
 * 白黒・テクスチャ・明瞭度・かすみの除去・シャープ・効果・マスク（部分補正））を使える最初の処理バージョン
 * （genzo-pipeline の ProcessVersion::V2）。
 */
export const V1_FEATURES_PROCESS_VERSION = 2;

/**
 * 「処理バージョンが古い写真は更新する」を出すか（貼り付け元の版が、v1 の機能を使える版以上。版 1 の設定を
 * 貼り付けるなら上げる先がない）。
 */
export function canOfferProcessUpgrade(sourceVersion: number): boolean {
  return sourceVersion >= V1_FEATURES_PROCESS_VERSION;
}

/** 「処理バージョンが古い写真は更新する」の文言と説明。 */
export function processUpgradeOption(sourceVersion: number): { label: string; detail: string } {
  return {
    label: `処理バージョンが古い写真は ${sourceVersion} に更新する`,
    detail:
      "オフなら、それぞれの写真の処理バージョンのまま貼り付けます。処理バージョン 1 の写真では、レンズ補正・" +
      "ハイライト復元・ノイズ軽減・カメラプロファイル・カラーミキサー・明瞭度・シャープ・効果・マスクなど" +
      "（処理バージョン 2 の項目）は使われません" +
      "（更新すると使われ、見え方が変わることがあります。履歴から元に戻せます）。",
  };
}

/** 貼り付けの結果の知らせ（成功の文と、処理バージョンが古いために使われない項目がある写真の警告）。 */
export function describePasteReport(
  r: PasteReport,
  sourceVersion: number,
): { message: string; warning: string | null } {
  const n = (v: number) => v.toLocaleString("ja-JP");
  const upgraded = r.upgraded > 0 ? `（${n(r.upgraded)} 枚の処理バージョンを ${sourceVersion} に更新）` : "";
  return {
    message: `${n(r.count)} 枚に現像設定を貼り付けました${upgraded}`,
    warning:
      r.needs_upgrade > 0
        ? `${n(r.needs_upgrade)} 枚は処理バージョンが古いため、レンズ補正・ハイライト復元・ノイズ軽減・カメラプロファイル・` +
          "カラーミキサー・明瞭度・シャープ・効果・マスクなど（処理バージョン 2 の項目）は使われません" +
          "（写真を右クリックして「処理バージョンを最新に更新…」で更新できます）"
        : null,
  };
}

/** 処理バージョンをまとめて更新した結果の知らせ。 */
export function describeProcessUpgrade(updated: number, targets: number): { level: "success" | "info"; message: string } {
  const n = (v: number) => v.toLocaleString("ja-JP");
  if (updated === 0) {
    return { level: "info", message: "更新が必要な写真はありませんでした（すでに最新の写真・動画はそのままです）" };
  }
  const rest = targets - updated;
  return {
    level: "success",
    message: `${n(updated)} 枚の処理バージョンを最新に更新しました${rest > 0 ? `（すでに最新の写真・動画 ${n(rest)} 枚はそのまま）` : ""}`,
  };
}
