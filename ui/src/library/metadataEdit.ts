// 説明のメタデータ（LIB-15。タイトル・キャプション・作成者・著作権・撮影場所など）の編集の決まり
// （DOM を使わない純粋な計算）。
//
// - 項目は genzo-model の MetadataField と同じ（名前・並び）。画面はグループ（内容・作成者・著作権・撮影場所・
//   状態）ごとに折りたためる節に分けて出す。
// - 1 枚のときは詳細（core.variantDetails の metadata）、複数のときはまとめ（core.metadataSummary）から、
//   項目ごとの表示（値・「複数の値」）を作る。
// - 保存する値は前後の空白を除き、空なら null（削除）。コアも同じ決まりで整える（genzo-model の
//   normalize_metadata_value）。1 行の項目の改行はコアが空白にする。

import type { CopyrightStatus, IptcMetadata, MetadataField, MetadataSummary } from "../api/types";

/** 1 つの項目の定義。 */
export interface MetadataFieldDef {
  field: MetadataField;
  /** 節の中の短い名前。 */
  label: string;
  /** 複数行の入力欄にするか（コアの MetadataField::multiline と同じ）。 */
  multiline?: boolean;
  /** 入力欄の種類（メール・URL・電話は、OS の入力の補助を使う）。 */
  inputType?: "text" | "email" | "url" | "tel";
  /** 選択肢から選ぶ項目（著作権の状態）。 */
  select?: boolean;
  /** テキスト検索の対象か（コアの MetadataField::searchable と同じ。入力欄の説明に出す）。 */
  searchable?: boolean;
}

/** 項目のグループ（節）。 */
export interface MetadataGroup {
  id: string;
  title: string;
  fields: MetadataFieldDef[];
}

/** 項目のグループ（画面の並び。コアの MetadataField::ALL と同じ順）。 */
export const METADATA_GROUPS: MetadataGroup[] = [
  {
    id: "content",
    title: "タイトルとキャプション",
    fields: [
      { field: "title", label: "タイトル", searchable: true },
      { field: "headline", label: "見出し", searchable: true },
      { field: "caption", label: "キャプション", multiline: true, searchable: true },
    ],
  },
  {
    id: "creator",
    title: "作成者と連絡先",
    fields: [
      { field: "creator", label: "作成者", searchable: true },
      { field: "creator_job_title", label: "肩書き" },
      { field: "creator_address", label: "住所", multiline: true },
      { field: "creator_city", label: "市区町村" },
      { field: "creator_region", label: "都道府県・州" },
      { field: "creator_postal_code", label: "郵便番号" },
      { field: "creator_country", label: "国" },
      { field: "creator_phone", label: "電話", inputType: "tel" },
      { field: "creator_email", label: "メール", inputType: "email" },
      { field: "creator_url", label: "Web サイト", inputType: "url" },
    ],
  },
  {
    id: "copyright",
    title: "著作権",
    fields: [
      { field: "copyright", label: "著作権表示", searchable: true },
      { field: "copyright_status", label: "著作権の状態", select: true },
      { field: "usage_terms", label: "使用条件", multiline: true },
      { field: "copyright_url", label: "著作権情報の URL", inputType: "url" },
    ],
  },
  {
    id: "location",
    title: "撮影場所",
    fields: [
      { field: "sublocation", label: "場所", searchable: true },
      { field: "city", label: "市区町村", searchable: true },
      { field: "province_state", label: "都道府県・州", searchable: true },
      { field: "country", label: "国", searchable: true },
      { field: "country_code", label: "国コード" },
    ],
  },
  {
    id: "status",
    title: "クレジットと指示",
    fields: [
      { field: "credit", label: "クレジット", searchable: true },
      { field: "source", label: "提供元", searchable: true },
      { field: "instructions", label: "指示", multiline: true },
    ],
  },
];

/** すべての項目（画面の並び）。 */
export const METADATA_FIELDS: MetadataFieldDef[] = METADATA_GROUPS.flatMap((g) => g.fields);

/** 著作権の状態の選択肢（"" は不明 = null）。 */
export const COPYRIGHT_STATUS_OPTIONS: { value: CopyrightStatus | ""; label: string }[] = [
  { value: "", label: "不明" },
  { value: "copyrighted", label: "著作権あり" },
  { value: "public_domain", label: "パブリックドメイン" },
];

/** 1 つの項目の値の上限（文字数。コアの MAX_METADATA_CHARS と同じ）。 */
export const MAX_METADATA_CHARS = 2000;

/** 項目の表示の状態。 */
export interface FieldState {
  /** 値（なければ null。mixed なら null）。 */
  value: string | null;
  /** 写真によって値が異なる（「複数の値」）。 */
  mixed: boolean;
}

export type FieldStates = Record<MetadataField, FieldState>;

/** 空の状態（どの項目も値なし）。 */
export function emptyStates(): FieldStates {
  const out = {} as FieldStates;
  for (const d of METADATA_FIELDS) out[d.field] = { value: null, mixed: false };
  return out;
}

/** 1 枚の写真の説明のメタデータ（詳細の metadata。古いコアで無ければ caption だけ）から。 */
export function statesFromMetadata(metadata: IptcMetadata | undefined, caption: string | null): FieldStates {
  const out = emptyStates();
  for (const d of METADATA_FIELDS) {
    const v = metadata?.[d.field];
    out[d.field] = { value: typeof v === "string" && v !== "" ? v : null, mixed: false };
  }
  if (!metadata && caption) out.caption = { value: caption, mixed: false };
  return out;
}

/** 複数の写真のまとめから。 */
export function statesFromSummary(summary: MetadataSummary): FieldStates {
  const out = emptyStates();
  for (const f of summary.fields) {
    if (f.field in out) out[f.field] = { value: f.mixed ? null : f.value, mixed: f.mixed };
  }
  return out;
}

/** 保存する値（前後の空白を除き、空なら null）。 */
export function normalizeDraft(draft: string): string | null {
  const t = draft.trim();
  return t === "" ? null : t;
}

/** 入力欄の値が長すぎるか（コアと同じ上限。前後の空白を除いた文字数）。 */
export function tooLong(draft: string): boolean {
  return [...draft.trim()].length > MAX_METADATA_CHARS;
}

/** 入力欄の見出しの補足（テキスト検索の対象なら知らせる）。 */
export function fieldHint(d: MetadataFieldDef): string | undefined {
  return d.searchable ? "テキスト検索の対象になります" : undefined;
}

/** 値のある項目の数（節の見出しに出す）。 */
export function filledCount(states: FieldStates, group: MetadataGroup): number {
  return group.fields.filter((d) => states[d.field].value !== null || states[d.field].mixed).length;
}

/**
 * 対象の並びの鍵（欄を作り直すかの判定に使う。数が多くても短い文字列にする: 件数・先頭・末尾と、FNV-1a の
 * 32bit のハッシュ）。
 */
export function idsKey(ids: readonly number[]): string {
  let h = 0x811c9dc5;
  for (const id of ids) {
    h ^= id & 0xffffffff;
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return `${ids.length}:${ids[0] ?? ""}:${ids[ids.length - 1] ?? ""}:${h.toString(16)}`;
}

/** 複数の写真を編集するときの説明（何枚に書くか）。 */
export function batchNote(count: number, assets: number | null): string {
  const n = count.toLocaleString("ja-JP");
  if (assets !== null && assets !== count) {
    return `選択中の ${n} 枚（元の写真 ${assets.toLocaleString("ja-JP")} 件。仮想コピーは元の写真と同じ値）に書きます。`;
  }
  return `選択中の ${n} 枚に書きます。`;
}
