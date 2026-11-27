// コレクション（LIB-10）・スマートコレクション（LIB-11）の画面の計算（DOM を使わない純粋な関数）。
//
// - 一覧（core.collections。名前の順の平らな並び）を parent_id で木にする。並びは、コレクションセットを先に、
//   それぞれ名前の順（Lightroom と同じ）。
// - 入れ子はコレクションセットの中だけ。移動の先の候補は、セットのうち自分・自分の中のセットを除いたもの。
// - 写真を入れられるのは手動のコレクションだけ（ターゲットコレクション・ドラッグ＆ドロップの先）。

import type { CollectionId, CollectionInfo, CollectionKind, SmartCondition, SmartRule, VariantId } from "../api/types";

export interface CollectionNode {
  info: CollectionInfo;
  children: CollectionNode[];
}

const KIND_ORDER: Record<CollectionKind, number> = { set: 0, smart: 1, manual: 1 };

/** 一覧を木にする（セットを先に、それぞれ一覧の順 = 名前の順）。 */
export function buildCollectionTree(list: readonly CollectionInfo[]): CollectionNode[] {
  const nodes = new Map<CollectionId, CollectionNode>();
  for (const c of list) nodes.set(c.collection_id, { info: c, children: [] });
  const roots: CollectionNode[] = [];
  for (const c of list) {
    const node = nodes.get(c.collection_id)!;
    const parent = c.parent_id === null ? undefined : nodes.get(c.parent_id);
    if (parent && parent.info.kind === "set" && !cyclic(nodes, c.collection_id, c.parent_id)) parent.children.push(node);
    else roots.push(node);
  }
  const order = (arr: CollectionNode[]) => {
    arr.sort((a, b) => KIND_ORDER[a.info.kind] - KIND_ORDER[b.info.kind]);
    for (const n of arr) order(n.children);
  };
  order(roots);
  return roots;
}

function cyclic(nodes: Map<CollectionId, CollectionNode>, id: CollectionId, start: CollectionId | null): boolean {
  let cur = start;
  for (let i = 0; cur !== null && i < 1000; i++) {
    if (cur === id) return true;
    cur = nodes.get(cur)?.info.parent_id ?? null;
  }
  return false;
}

export interface CollectionRow {
  node: CollectionNode;
  depth: number;
  hasChildren: boolean;
  open: boolean;
}

/** 表示する行（開いているセットの子だけ）。 */
export function visibleCollectionRows(
  tree: readonly CollectionNode[],
  isOpen: (id: CollectionId, depth: number) => boolean,
): CollectionRow[] {
  const out: CollectionRow[] = [];
  const walk = (nodes: readonly CollectionNode[], depth: number) => {
    for (const n of nodes) {
      const hasChildren = n.children.length > 0;
      const open = hasChildren && isOpen(n.info.collection_id, depth);
      out.push({ node: n, depth, hasChildren, open });
      if (open) walk(n.children, depth + 1);
    }
  };
  walk(tree, 0);
  return out;
}

/** 最上位のセットからの名前の並び。 */
export function collectionPath(list: readonly CollectionInfo[], id: CollectionId): string[] {
  const byId = new Map(list.map((c) => [c.collection_id, c]));
  const out: string[] = [];
  let cur = byId.get(id);
  for (let i = 0; cur && i < 1000; i++) {
    out.push(cur.name);
    cur = cur.parent_id === null ? undefined : byId.get(cur.parent_id);
  }
  return out.reverse();
}

/** 表示用のパス（「旅行 > 京都 2024」）。 */
export function collectionPathText(list: readonly CollectionInfo[], id: CollectionId): string {
  return collectionPath(list, id).join(" > ");
}

/** 自分と、自分の中のコレクションの ID。 */
export function collectionDescendants(list: readonly CollectionInfo[], id: CollectionId): Set<CollectionId> {
  const out = new Set<CollectionId>([id]);
  let grew = true;
  while (grew) {
    grew = false;
    for (const c of list) {
      if (c.parent_id !== null && out.has(c.parent_id) && !out.has(c.collection_id)) {
        out.add(c.collection_id);
        grew = true;
      }
    }
  }
  return out;
}

/** 親の候補（セットだけ。`movingId` があれば、自分と自分の中のセットを除く）。null は最上位。 */
export function parentChoices(
  list: readonly CollectionInfo[],
  movingId: CollectionId | null = null,
): { id: CollectionId | null; label: string }[] {
  const excluded = movingId === null ? new Set<CollectionId>() : collectionDescendants(list, movingId);
  const sets = list
    .filter((c) => c.kind === "set" && !excluded.has(c.collection_id))
    .map((c) => ({ id: c.collection_id as CollectionId | null, label: collectionPathText(list, c.collection_id) }));
  sets.sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));
  return [{ id: null, label: "（最上位）" }, ...sets];
}

/** 写真を入れられるコレクション（手動のコレクション。パスの順）。 */
export function manualCollections(list: readonly CollectionInfo[]): { id: CollectionId; label: string }[] {
  const out = list
    .filter((c) => c.kind === "manual")
    .map((c) => ({ id: c.collection_id, label: collectionPathText(list, c.collection_id) }));
  out.sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));
  return out;
}

/** 種類の名前。 */
export const COLLECTION_KIND_LABEL: Record<CollectionKind, string> = {
  manual: "コレクション",
  smart: "スマートコレクション",
  set: "コレクションセット",
};

/** 一覧の行の数の表示（数えていなければ空）。 */
export function countText(c: CollectionInfo): string {
  if (c.rule_error) return "!";
  return c.count === null ? "" : c.count.toLocaleString("ja-JP");
}

/** 削除の確認の文。 */
export function deleteCollectionMessage(list: readonly CollectionInfo[], id: CollectionId): string[] {
  const c = list.find((x) => x.collection_id === id);
  if (!c) return [];
  const lines = [`${COLLECTION_KIND_LABEL[c.kind]}「${c.name}」を削除します（写真や元のファイルは削除しません）。`];
  const removed = collectionDescendants(list, id);
  const inner = removed.size - 1;
  if (inner > 0) lines.push(`中のコレクション ${inner.toLocaleString("ja-JP")} 個も削除します。`);
  // 削除するもの（中のコレクションを含む）を条件で使っている、残るスマートコレクション。
  const refs = list.filter(
    (x) => x.kind === "smart" && !removed.has(x.collection_id) && referencedCollections(x.rule).some((r) => removed.has(r)),
  );
  if (refs.length > 0)
    lines.push(
      `スマートコレクション ${refs.map((r) => `「${r.name}」`).join("・")} の条件がこのコレクションを使っています（削除すると、その条件の「含まれる」はどの写真にも一致しなくなり、「含まれない」はすべての写真に一致するようになります）。`,
    );
  return lines;
}

/** スマートコレクションの条件が参照するコレクション（グループの中も。コアの referenced_collections と同じ）。 */
export function referencedCollections(rule: SmartRule | null | undefined): CollectionId[] {
  const out: CollectionId[] = [];
  const walk = (conds: readonly SmartCondition[]) => {
    for (const c of conds) {
      if (c.type === "collection" && !out.includes(c.collection_id)) out.push(c.collection_id);
      else if (c.type === "group") walk(c.rules);
    }
  };
  if (rule) walk(rule.rules);
  return out;
}

/** コレクションの名前の文字数の上限（コアの MAX_COLLECTION_NAME_CHARS と同じ。Unicode のコードポイントの数）。 */
export const MAX_COLLECTION_NAME_CHARS = 200;

/** 名前の入力の検証（空・長すぎ・制御文字。コアと同じ規則）。問題がなければ null。 */
export function validateCollectionName(name: string): string | null {
  const t = name.trim();
  if (t === "") return "名前を入れてください";
  if ([...t].length > MAX_COLLECTION_NAME_CHARS) return `名前は ${MAX_COLLECTION_NAME_CHARS} 文字までにしてください`;
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(t)) return "改行・タブなどの制御文字は使えません";
  return null;
}

// ---------------------------------------------------------------------------
// 手動のコレクションの中の並べ替え（LIB-10 の残り。写真の右クリックのメニュー）
// ---------------------------------------------------------------------------

/** 並べ替えの向き（画面の上で）: 先頭へ・1 つ前へ・1 つ後ろへ・最後へ。 */
export type CollectionMove = "first" | "up" | "down" | "last";

/** コアに送る並べ替え（reorder_collection）: `ids`（この順に並ぶ）を `before` の前へ（null なら最後へ）。 */
export interface CollectionReorder {
  ids: VariantId[];
  before: VariantId | null;
}

/**
 * 画面の並び（コレクションの順で並べた検索結果）の上での移動を、コアに送る並べ替えにする。
 *
 * - `indexes` は移す写真の、画面の並びの上の位置（重なり・順は問わない）。`idAt` は位置から写真（読み込んで
 *   いなければ undefined）。`count` は表示中の件数。`descending` は並べ替えの向きが降順か（画面の並びが
 *   コレクションの順の逆）。
 * - 動かせない（すでに端にある・隣の写真を読み込んでいない）なら null。
 * - 絞り込みで一部だけを表示しているときは、表示されている隣の写真を基準にする。
 */
export function planCollectionMove(
  move: CollectionMove,
  indexes: readonly number[],
  idAt: (index: number) => VariantId | undefined,
  count: number,
  descending: boolean,
): CollectionReorder | null {
  const sorted = [...new Set(indexes)].filter((i) => i >= 0 && i < count).sort((a, b) => a - b);
  if (sorted.length === 0) return null;
  const picked = new Set(sorted);
  const visual: VariantId[] = [];
  for (const i of sorted) {
    const id = idAt(i);
    if (id === undefined) return null;
    visual.push(id);
  }
  // コレクションの順（昇順）での並びにする。
  const ids = descending ? [...visual].reverse() : visual;
  const first = sorted[0];
  const last = sorted[sorted.length - 1];
  /** 画面の位置 `i` の写真の前（コレクションの順で）へ。範囲の外なら最後へ。 */
  const beforeAt = (i: number): CollectionReorder | null => {
    if (i < 0 || i >= count) return { ids, before: null };
    const id = idAt(i);
    return id === undefined ? null : { ids, before: id };
  };
  /** 移す写真でない、画面の上で最初（`fromEnd` なら最後）の写真の位置（なければ -1）。 */
  const edge = (fromEnd: boolean): number => {
    for (let k = 0; k < count; k++) {
      const i = fromEnd ? count - 1 - k : k;
      if (!picked.has(i)) return i;
    }
    return -1;
  };
  // 移す写真がひとかたまりで端にあれば、動かすところがない。
  const contiguous = last - first + 1 === sorted.length;
  switch (move) {
    case "first": {
      if (contiguous && first === 0) return null;
      // 昇順: 最初の写真の前へ。降順: 画面の先頭はコレクションの最後。
      return descending ? { ids, before: null } : beforeAt(edge(false));
    }
    case "last": {
      if (contiguous && last === count - 1) return null;
      return descending ? beforeAt(edge(true)) : { ids, before: null };
    }
    case "up": {
      if (first === 0) return null;
      // 昇順: 1 つ前の写真の前へ。降順: 1 つ前の写真（コレクションの順では次）の後ろ = 2 つ前の写真の前へ。
      return descending ? beforeAt(first - 2) : beforeAt(first - 1);
    }
    case "down": {
      if (last === count - 1) return null;
      // 昇順: 1 つ後ろの写真の後ろ = 2 つ後ろの写真の前へ。降順: 1 つ後ろの写真の前へ。
      return descending ? beforeAt(last + 1) : beforeAt(last + 2);
    }
  }
}
