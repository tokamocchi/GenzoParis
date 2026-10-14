// 階層キーワード（LIB-09）の画面の計算（DOM を使わない純粋な関数）。
//
// - キーワードの一覧（core.keywordTree。名前の順の平らな並び）を parent_id で木にする。親が一覧にない
//   （循環した壊れたデータを含む）ものは最上位に置く。
// - 入力欄の書き方: 「場所 > 京都, 家族」のように、「,」「、」・改行で区切って複数を入れ、「>」（全角の「＞」・
//   「|」も）で階層を書く。前後の空白は除き、空の段は捨てる（parseKeywordInput）。そのため、キーワードの名前には
//   これらの区切りの文字と制御文字を使えず、200 文字まで（validateKeywordName。コアと同じ規則）。
// - 選択中の写真のキーワード（core.variantKeywords）の数が、選択の数と同じならすべてに付いている
//   （"all"）、少なければ一部（"some"）。

import type { KeywordId, KeywordInfo, VariantKeyword, VariantKeywords } from "../api/types";

/** キーワードの木の 1 つの点。 */
export interface KeywordNode {
  info: KeywordInfo;
  children: KeywordNode[];
}

/** 一覧を木にする（子の並びは一覧の順 = 名前の順）。 */
export function buildKeywordTree(list: readonly KeywordInfo[]): KeywordNode[] {
  const nodes = new Map<KeywordId, KeywordNode>();
  for (const k of list) nodes.set(k.keyword_id, { info: k, children: [] });
  const roots: KeywordNode[] = [];
  for (const k of list) {
    const node = nodes.get(k.keyword_id)!;
    const parent = k.parent_id === null ? undefined : nodes.get(k.parent_id);
    if (parent && !isAncestor(nodes, k.keyword_id, k.parent_id)) parent.children.push(node);
    else roots.push(node);
  }
  return roots;
}

/** `ancestorOf` が `id` の祖先（自分を含む）をたどると現れるか（循環の検出）。 */
function isAncestor(nodes: Map<KeywordId, KeywordNode>, id: KeywordId, start: KeywordId | null): boolean {
  let cur = start;
  for (let i = 0; cur !== null && i < 1000; i++) {
    if (cur === id) return true;
    cur = nodes.get(cur)?.info.parent_id ?? null;
  }
  return false;
}

/** 表示する行（開いている点の子だけ。深さつき）。`filter` があれば、名前に含むものとその祖先を開いて出す。 */
export interface KeywordRow {
  node: KeywordNode;
  depth: number;
  hasChildren: boolean;
  open: boolean;
}

export function visibleKeywordRows(
  tree: readonly KeywordNode[],
  isOpen: (id: KeywordId, depth: number) => boolean,
  filter = "",
): KeywordRow[] {
  const q = normalizeName(filter);
  const out: KeywordRow[] = [];
  const matches = (n: KeywordNode): boolean =>
    normalizeName(n.info.name).includes(q) || n.children.some(matches);
  const walk = (nodes: readonly KeywordNode[], depth: number) => {
    for (const n of nodes) {
      if (q !== "" && !matches(n)) continue;
      const hasChildren = n.children.length > 0;
      // 絞り込み中は、一致する子孫のある点を開く。
      const open = hasChildren && (q !== "" ? n.children.some(matches) : isOpen(n.info.keyword_id, depth));
      out.push({ node: n, depth, hasChildren, open });
      if (open) walk(n.children, depth + 1);
    }
  };
  walk(tree, 0);
  return out;
}

/** 名前の比較用（NFKC ＋ 小文字。コアの検索のキーと同じ考え方）。 */
export function normalizeName(s: string): string {
  return s.normalize("NFKC").toLowerCase().trim();
}

/** 階層のパスの表示（「場所 > 京都」）。 */
export function keywordPathText(path: readonly string[]): string {
  return path.join(" > ");
}

/** キーワードの最上位からのパス（一覧から求める。見つからなければ空）。 */
export function keywordPath(list: readonly KeywordInfo[], id: KeywordId): string[] {
  const byId = new Map(list.map((k) => [k.keyword_id, k]));
  const out: string[] = [];
  let cur = byId.get(id);
  for (let i = 0; cur && i < 1000; i++) {
    out.push(cur.name);
    cur = cur.parent_id === null ? undefined : byId.get(cur.parent_id);
  }
  return out.reverse();
}

/** 自分と子孫の ID（移動の先の候補から除く・削除の確認）。 */
export function descendantIds(list: readonly KeywordInfo[], id: KeywordId): Set<KeywordId> {
  const children = new Map<KeywordId, KeywordId[]>();
  for (const k of list) {
    if (k.parent_id === null) continue;
    const arr = children.get(k.parent_id) ?? [];
    arr.push(k.keyword_id);
    children.set(k.parent_id, arr);
  }
  const out = new Set<KeywordId>([id]);
  const stack = [id];
  while (stack.length > 0) {
    const cur = stack.pop()!;
    for (const c of children.get(cur) ?? []) {
      if (!out.has(c)) {
        out.add(c);
        stack.push(c);
      }
    }
  }
  return out;
}

/** 移動の先の候補（自分・子孫を除く。表示はパス）。null は最上位。 */
export function moveTargets(list: readonly KeywordInfo[], id: KeywordId): { id: KeywordId | null; label: string }[] {
  const excluded = descendantIds(list, id);
  const out: { id: KeywordId | null; label: string }[] = [{ id: null, label: "（最上位）" }];
  const items = list
    .filter((k) => !excluded.has(k.keyword_id))
    .map((k) => ({ id: k.keyword_id as KeywordId | null, label: keywordPathText(keywordPath(list, k.keyword_id)) }));
  items.sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));
  return out.concat(items);
}

/** 入力欄の区切り（「,」「、」「，」・改行）。 */
const ENTRY_SEPARATOR = /[,，、\n\r]+/;
/** 階層の区切り（「>」「＞」「|」）。 */
const LEVEL_SEPARATOR = /[>＞|]/;

/**
 * 入力欄の文字列をキーワードのパスの並びにする（「場所 > 京都, 家族」→ [["場所", "京都"], ["家族"]]）。
 * 空の段・空の項目は捨て、同じパス（比較は normalizeName）は 1 つにする。
 */
export function parseKeywordInput(text: string): string[][] {
  const out: string[][] = [];
  const seen = new Set<string>();
  for (const entry of text.split(ENTRY_SEPARATOR)) {
    const path = entry
      .split(LEVEL_SEPARATOR)
      .map((s) => s.trim())
      .filter((s) => s !== "");
    if (path.length === 0) continue;
    const key = path.map(normalizeName).join("\u0000");
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(path);
  }
  return out;
}

/** キーワードの名前の文字数の上限（コアの MAX_KEYWORD_NAME_CHARS と同じ。Unicode のコードポイントの数）。 */
export const MAX_KEYWORD_NAME_CHARS = 200;

/** キーワードの名前に使えない文字（入力欄の区切り。コアの KEYWORD_NAME_SEPARATORS と同じ）。 */
const NAME_SEPARATORS = [">", "＞", "|", ",", "，", "、"];

/**
 * キーワードの 1 つの名前（階層の 1 段）の検証（空・長すぎ・制御文字・区切りの文字。コアと同じ規則）。
 * 問題がなければ null。
 */
export function validateKeywordName(name: string): string | null {
  const t = name.trim();
  if (t === "") return "名前を入れてください";
  if ([...t].length > MAX_KEYWORD_NAME_CHARS) return `名前は ${MAX_KEYWORD_NAME_CHARS} 文字までにしてください`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(t)) return "改行・タブなどの制御文字は使えません";
  const sep = NAME_SEPARATORS.find((c) => t.includes(c));
  if (sep !== undefined) return `「${sep}」は使えません（階層や複数のキーワードの区切りに使う文字です）`;
  return null;
}

/** 入力中の最後の項目（補完の対象。「家族, 場所 > 京」なら「場所 > 京」）。 */
export function lastEntry(text: string): string {
  const parts = text.split(ENTRY_SEPARATOR);
  return parts[parts.length - 1].trim();
}

/** 補完の候補を選んだ後の入力欄の文字列（最後の項目を候補のパスに置き換え、次の区切りを足す）。 */
export function applyCompletion(text: string, path: readonly string[]): string {
  const m = /[,，、\n\r][^,，、\n\r]*$/.exec(text);
  const head = m ? text.slice(0, m.index + 1) + " " : "";
  return `${head}${keywordPathText(path)}, `;
}

/** 補完の検索語（最後の項目の、最後の段。「場所 > 京」なら「京」）。 */
export function completionQuery(text: string): string {
  const entry = lastEntry(text);
  const levels = entry.split(LEVEL_SEPARATOR);
  return levels[levels.length - 1].trim();
}

/** 選択中の写真への付き方。 */
export type TagState = "all" | "some" | "none";

export function tagState(summary: VariantKeywords | null, id: KeywordId): TagState {
  if (!summary || summary.variants === 0) return "none";
  const k = summary.keywords.find((x) => x.keyword_id === id);
  if (!k) return "none";
  return k.count >= summary.variants ? "all" : "some";
}

/** 選択中の写真のキーワードを、表示用に並べる（すべてに付いているものを先に、それぞれパスの順）。 */
export function sortedVariantKeywords(summary: VariantKeywords): VariantKeyword[] {
  const text = (k: VariantKeyword) => keywordPathText(k.path);
  return [...summary.keywords].sort((a, b) => {
    const fa = a.count >= summary.variants ? 0 : 1;
    const fb = b.count >= summary.variants ? 0 : 1;
    if (fa !== fb) return fa - fb;
    return text(a) < text(b) ? -1 : text(a) > text(b) ? 1 : 0;
  });
}

/** 削除の確認の文。 */
export function deleteKeywordMessage(
  name: string,
  usage: { photos_with_descendants: number; descendants: number; smart_collections: { name: string }[] },
): string[] {
  const lines = [`キーワード「${name}」を削除します（写真や元のファイルは削除しません）。`];
  if (usage.descendants > 0) lines.push(`下の階層のキーワード ${usage.descendants.toLocaleString("ja-JP")} 個も削除します。`);
  if (usage.photos_with_descendants > 0)
    lines.push(`${usage.photos_with_descendants.toLocaleString("ja-JP")} 枚の写真からキーワードが外れます。`);
  if (usage.smart_collections.length > 0)
    lines.push(
      `スマートコレクション ${usage.smart_collections.map((c) => `「${c.name}」`).join("・")} の条件がこのキーワードを使っています（削除すると、その条件の「含む」はどの写真にも一致しなくなり、「含まない」はすべての写真に一致するようになります）。`,
    );
  return lines;
}
