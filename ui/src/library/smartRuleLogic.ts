// スマートコレクションの条件（LIB-11。genzo-model の smart.rs の SmartRule）の編集の計算（DOM を使わない
// 純粋な関数）。
//
// - 条件は木（グループで入れ子）。編集の画面の行は、最上位からの位置の並び（パス。[2, 0] は最上位の
//   3 つ目のグループの 1 つ目）で指す。変更はすべて新しいオブジェクトを返す（元を変えない）。
// - 検証（ruleProblems）はコアの SmartRule::validate と同じ決まり（版・入れ子の深さ・数・各条件の値）で、
//   保存のボタンを止めるために使う（保存のときにコアも確かめる。循環する参照はコアだけが確かめる）。
// - 撮影日の範囲は、カタログの既定のタイムゾーンの日付を UTC にして保存する（絞り込みと同じ。
//   filterLogic.ts の localDateToUtc）。

import type {
  AssetKind,
  ColorLabel,
  Flag,
  MatchMode,
  Rating,
  SmartCondition,
  SmartRule,
} from "../api/types";
import { utcToLocalDate } from "./filterLogic";

/** 条件の JSON の版（コアの SMART_RULE_VERSION）。 */
export const SMART_RULE_VERSION = 1;
/** グループの入れ子の深さの上限（コアの MAX_SMART_DEPTH。最上位が 1）。 */
export const MAX_SMART_DEPTH = 8;
/** 条件の数の上限（コアの MAX_SMART_CONDITIONS）。 */
export const MAX_SMART_CONDITIONS = 200;
/** テキストの値の文字数の上限（コアの MAX_SMART_TEXT_CHARS）。 */
export const MAX_SMART_TEXT_CHARS = 200;
/** 「直近 N 日」の上限（コアの MAX_SMART_DAYS）。 */
export const MAX_SMART_DAYS = 36_500;

export type ConditionType = SmartCondition["type"];

/** 条件の種類の名前（選択肢の順）。 */
export const CONDITION_TYPES: { type: ConditionType; label: string }[] = [
  { type: "rating", label: "評価" },
  { type: "flag", label: "フラグ" },
  { type: "color_label", label: "カラーラベル" },
  { type: "keyword", label: "キーワード" },
  { type: "capture_time", label: "撮影日" },
  { type: "camera", label: "カメラ" },
  { type: "lens", label: "レンズ" },
  { type: "kind", label: "種類" },
  { type: "text", label: "テキスト" },
  { type: "folder", label: "フォルダ" },
  { type: "collection", label: "コレクション" },
  { type: "group", label: "条件のグループ" },
];

/** 演算子の選択肢（条件の種類ごと）。 */
export const OPS: Partial<Record<ConditionType, { value: string; label: string }[]>> = {
  rating: [
    { value: "gte", label: "が次の値以上" },
    { value: "lte", label: "が次の値以下" },
    { value: "eq", label: "が次の値" },
    { value: "ne", label: "が次の値でない" },
  ],
  flag: [
    { value: "is", label: "が次である" },
    { value: "is_not", label: "が次でない" },
  ],
  color_label: [
    { value: "is", label: "が次である" },
    { value: "is_not", label: "が次でない" },
  ],
  keyword: [
    { value: "has", label: "に次を含む（下の階層も）" },
    { value: "not_has", label: "に次を含まない（下の階層も）" },
    { value: "any", label: "が付いている" },
    { value: "none", label: "が付いていない" },
  ],
  capture_time: [
    { value: "range", label: "が次の期間" },
    { value: "in_last_days", label: "が直近の日数" },
    { value: "unknown", label: "が分からない" },
  ],
  camera: [
    { value: "is", label: "が次と同じ" },
    { value: "is_not", label: "が次と違う" },
    { value: "contains", label: "が次を含む" },
    { value: "unknown", label: "が分からない" },
  ],
  lens: [
    { value: "is", label: "が次と同じ" },
    { value: "is_not", label: "が次と違う" },
    { value: "contains", label: "が次を含む" },
    { value: "unknown", label: "が分からない" },
  ],
  text: [
    { value: "contains", label: "に次を含む" },
    { value: "not_contains", label: "に次を含まない" },
  ],
  folder: [
    { value: "in", label: "に含まれる" },
    { value: "not_in", label: "に含まれない" },
  ],
  collection: [
    { value: "in", label: "に含まれる" },
    { value: "not_in", label: "に含まれない" },
  ],
};

export const FLAG_CHOICES: { value: Flag; label: string }[] = [
  { value: 1, label: "採用" },
  { value: 0, label: "フラグなし" },
  { value: -1, label: "不採用" },
];

export const KIND_CHOICES: { value: AssetKind; label: string }[] = [
  { value: "photo", label: "写真" },
  { value: "video", label: "動画" },
];

export const MATCH_CHOICES: { value: MatchMode; label: string }[] = [
  { value: "all", label: "すべての条件に一致" },
  { value: "any", label: "いずれかの条件に一致" },
];

/** 種類ごとの既定の条件（値は空のことがある。保存の前に検証で知らせる）。 */
export function defaultCondition(type: ConditionType): SmartCondition {
  switch (type) {
    case "group":
      return { type: "group", match: "any", rules: [defaultCondition("rating")] };
    case "rating":
      return { type: "rating", op: "gte", value: 3 };
    case "flag":
      return { type: "flag", op: "is", value: 1 };
    case "color_label":
      return { type: "color_label", op: "is", value: "red" };
    case "keyword":
      return { type: "keyword", op: "any" };
    case "capture_time":
      return { type: "capture_time", op: "in_last_days", days: 30 };
    case "camera":
      return { type: "camera", op: "is", value: "" };
    case "lens":
      return { type: "lens", op: "is", value: "" };
    case "kind":
      return { type: "kind", value: "photo" };
    case "text":
      return { type: "text", op: "contains", value: "" };
    case "folder":
      return { type: "folder", op: "in", folder_id: 0, include_subfolders: true };
    case "collection":
      return { type: "collection", op: "in", collection_id: 0 };
  }
}

/** 新しい条件（★3 以上の 1 つ）。 */
export function emptyRule(): SmartRule {
  return { version: SMART_RULE_VERSION, match: "all", rules: [defaultCondition("rating")] };
}

/** 演算子を変える（値が要らなくなったものは消し、要るようになったものは既定の値を入れる）。 */
export function withOp(c: SmartCondition, op: string): SmartCondition {
  switch (c.type) {
    case "keyword":
      return op === "has" || op === "not_has"
        ? { type: "keyword", op, keyword_id: c.keyword_id ?? 0 }
        : { type: "keyword", op: op as "any" | "none" };
    case "capture_time":
      if (op === "range") return { type: "capture_time", op, from: c.from, until: c.until };
      if (op === "in_last_days") return { type: "capture_time", op, days: c.days ?? 30 };
      return { type: "capture_time", op: "unknown" };
    case "group":
    case "kind":
      return c;
    default:
      return { ...c, op } as SmartCondition;
  }
}

// ---------------------------------------------------------------------------
// 木の操作（パス = 最上位からの位置の並び）
// ---------------------------------------------------------------------------

/** パスの条件（なければ undefined）。 */
export function conditionAt(rule: SmartRule, path: readonly number[]): SmartCondition | undefined {
  let rules = rule.rules;
  let cur: SmartCondition | undefined;
  for (const i of path) {
    cur = rules[i];
    if (!cur) return undefined;
    rules = cur.type === "group" ? cur.rules : [];
  }
  return cur;
}

/** グループ（パスが空なら最上位）の条件の並びを変える。 */
function mapRules(
  rule: SmartRule,
  groupPath: readonly number[],
  f: (rules: SmartCondition[]) => SmartCondition[],
): SmartRule {
  const go = (rules: SmartCondition[], depth: number): SmartCondition[] => {
    if (depth === groupPath.length) return f(rules);
    const i = groupPath[depth];
    return rules.map((c, j) => (j === i && c.type === "group" ? { ...c, rules: go(c.rules, depth + 1) } : c));
  };
  return { ...rule, rules: go(rule.rules, 0) };
}

/** パスの条件を置き換える。 */
export function replaceAt(rule: SmartRule, path: readonly number[], c: SmartCondition): SmartRule {
  if (path.length === 0) return rule;
  const i = path[path.length - 1];
  return mapRules(rule, path.slice(0, -1), (rules) => rules.map((x, j) => (j === i ? c : x)));
}

/** パスの条件を消す。 */
export function removeAt(rule: SmartRule, path: readonly number[]): SmartRule {
  if (path.length === 0) return rule;
  const i = path[path.length - 1];
  return mapRules(rule, path.slice(0, -1), (rules) => rules.filter((_, j) => j !== i));
}

/** グループ（パスが空なら最上位）の最後に条件を足す。 */
export function appendTo(rule: SmartRule, groupPath: readonly number[], c: SmartCondition): SmartRule {
  return mapRules(rule, groupPath, (rules) => [...rules, c]);
}

/** グループのすべて / いずれかを変える（パスが空なら最上位）。 */
export function setMatch(rule: SmartRule, groupPath: readonly number[], match: MatchMode): SmartRule {
  if (groupPath.length === 0) return { ...rule, match };
  const g = conditionAt(rule, groupPath);
  if (!g || g.type !== "group") return rule;
  return replaceAt(rule, groupPath, { ...g, match });
}

/** 条件の数（グループを含む）と、入れ子の深さ（最上位が 1）。 */
export function ruleSize(rule: SmartRule): { count: number; depth: number } {
  let count = 0;
  let depth = 1;
  const walk = (rules: SmartCondition[], d: number) => {
    depth = Math.max(depth, d);
    for (const c of rules) {
      count++;
      if (c.type === "group") walk(c.rules, d + 1);
    }
  };
  walk(rule.rules, 1);
  return { count, depth };
}

// ---------------------------------------------------------------------------
// 検証
// ---------------------------------------------------------------------------

function textProblem(v: string, what: string): string | null {
  const t = v.trim();
  if (t === "") return `${what}の値を入れてください`;
  if ([...t].length > MAX_SMART_TEXT_CHARS) return `${what}の値は ${MAX_SMART_TEXT_CHARS} 文字までです`;
  return null;
}

/** 保存できない理由（なければ空）。コアの SmartRule::validate と同じ決まり。 */
export function ruleProblems(rule: SmartRule): string[] {
  const out: string[] = [];
  if (rule.version !== SMART_RULE_VERSION) out.push(`条件の版 ${rule.version} はこの画面で編集できません`);
  const { count, depth } = ruleSize(rule);
  if (depth > MAX_SMART_DEPTH) out.push(`条件のグループの入れ子は ${MAX_SMART_DEPTH} 段までです`);
  if (count > MAX_SMART_CONDITIONS) out.push(`条件は ${MAX_SMART_CONDITIONS} 個までです`);
  const walk = (rules: SmartCondition[]) => {
    for (const c of rules) {
      let p: string | null = null;
      switch (c.type) {
        case "group":
          walk(c.rules);
          break;
        case "keyword":
          if ((c.op === "has" || c.op === "not_has") && !c.keyword_id) p = "キーワードを選んでください";
          break;
        case "capture_time":
          if (c.op === "range") {
            if (!c.from && !c.until) p = "撮影日の期間の開始か終了を入れてください";
            else if (c.from && c.until && Date.parse(c.from) >= Date.parse(c.until))
              p = "撮影日の期間の終了は開始より後にしてください";
          } else if (c.op === "in_last_days") {
            const d = c.days ?? 0;
            if (!Number.isInteger(d) || d < 1 || d > MAX_SMART_DAYS)
              p = `直近の日数は 1〜${MAX_SMART_DAYS.toLocaleString("ja-JP")} 日で入れてください`;
          }
          break;
        case "camera":
        case "lens":
          if (c.op !== "unknown") p = textProblem(c.value, c.type === "camera" ? "カメラ" : "レンズ");
          break;
        case "text":
          p = textProblem(c.value, "テキスト");
          break;
        case "folder":
          if (!c.folder_id) p = "フォルダを選んでください";
          break;
        case "collection":
          if (!c.collection_id) p = "コレクションを選んでください";
          break;
        default:
          break;
      }
      if (p && !out.includes(p)) out.push(p);
    }
  };
  walk(rule.rules);
  return out;
}

// ---------------------------------------------------------------------------
// 説明（一覧のツールチップ）
// ---------------------------------------------------------------------------

const LABEL_NAMES: Record<ColorLabel, string> = {
  red: "赤",
  yellow: "黄",
  green: "緑",
  blue: "青",
  purple: "紫",
};

/** 名前の解決（キーワード・フォルダ・コレクションの ID → 表示名）。 */
export interface NameLookup {
  keyword(id: number): string | undefined;
  folder(id: number): string | undefined;
  collection(id: number): string | undefined;
}

function opLabel(type: ConditionType, op: string): string {
  return OPS[type]?.find((o) => o.value === op)?.label ?? op;
}

/** 条件の 1 つの説明（「評価が次の値以上 ★3」など）。 */
export function describeCondition(c: SmartCondition, names: NameLookup, offsetMinutes = 0): string {
  const field = CONDITION_TYPES.find((t) => t.type === c.type)?.label ?? c.type;
  const gone = "（削除済み）";
  switch (c.type) {
    case "group":
      return `（${describeRules(c.match, c.rules, names, offsetMinutes)}）`;
    case "rating":
      return `${field}${opLabel(c.type, c.op)} ★${c.value as Rating}`;
    case "flag":
      return `${field}${opLabel(c.type, c.op)} ${FLAG_CHOICES.find((f) => f.value === c.value)?.label ?? c.value}`;
    case "color_label":
      return `${field}${opLabel(c.type, c.op)} ${c.value ? LABEL_NAMES[c.value] : "ラベルなし"}`;
    case "keyword":
      return c.op === "has" || c.op === "not_has"
        ? `${field}${opLabel(c.type, c.op)} ${names.keyword(c.keyword_id ?? 0) ?? gone}`
        : `${field}${opLabel(c.type, c.op)}`;
    case "capture_time": {
      if (c.op === "in_last_days") return `${field}${opLabel(c.type, c.op)} ${c.days ?? 0} 日`;
      if (c.op === "unknown") return `${field}${opLabel(c.type, c.op)}`;
      const day = (iso: string | undefined, upper: boolean) => utcToLocalDate(iso, offsetMinutes, upper);
      return `${field}${opLabel(c.type, c.op)} ${day(c.from, false)}〜${day(c.until, true)}`;
    }
    case "camera":
    case "lens":
      return c.op === "unknown" ? `${field}${opLabel(c.type, c.op)}` : `${field}${opLabel(c.type, c.op)}「${c.value}」`;
    case "kind":
      return `${field}が${KIND_CHOICES.find((k) => k.value === c.value)?.label ?? c.value}`;
    case "text":
      return `${field}${opLabel(c.type, c.op)}「${c.value}」`;
    case "folder":
      return `${field}${opLabel(c.type, c.op)} ${names.folder(c.folder_id) ?? gone}${c.include_subfolders ? "（サブフォルダを含む）" : ""}`;
    case "collection":
      return `${field}${opLabel(c.type, c.op)} ${names.collection(c.collection_id) ?? gone}`;
  }
}

function describeRules(match: MatchMode, rules: SmartCondition[], names: NameLookup, offsetMinutes: number): string {
  if (rules.length === 0) return match === "all" ? "すべての写真" : "どの写真も一致しない";
  return rules.map((c) => describeCondition(c, names, offsetMinutes)).join(match === "all" ? " かつ " : " または ");
}

/** 条件全体の説明。 */
export function describeRule(rule: SmartRule, names: NameLookup, offsetMinutes = 0): string {
  return describeRules(rule.match, rule.rules, names, offsetMinutes);
}
