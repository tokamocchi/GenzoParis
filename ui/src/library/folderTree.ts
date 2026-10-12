// フォルダツリー（LIB-06）の組み立て（DOM を使わない純粋な計算）。
//
// - core.folders() のフォルダ（parent_id で親をたどる）を、ボリュームごとの根から木にする。カタログには、
//   写真のあるフォルダの親がボリュームのルート（rel_path が ""）まですべて登録される（genzo-catalog の
//   register.rs の `ensure_folder`）。
// - 子が 1 つだけのフォルダは、子と同じ行にまとめる（"/ › Users › me › Pictures" のように、1 行に
//   複数の区切りを並べる）。どの区切りも選べるので、途中のフォルダに直接ある写真も選べる（フォルダごとの
//   写真の数はコア API から取れないため、写真のないフォルダだけを省くことはしない）。
// - 兄弟は名前の自然な順（"2" < "10"）に並べる。

import type { FolderId, FolderInfo, VolumeId } from "../api/types";

/** 行の中の 1 つのフォルダ。 */
export interface FolderSegment {
  id: FolderId;
  /** 表示名（ルートはボリュームのパス）。 */
  name: string;
  /** 絶対パス（分からなければ相対パス）。title に使う。 */
  fullPath: string;
}

/** ツリーの 1 行（子が 1 つのフォルダをまとめたもの）。 */
export interface FolderNode {
  /** 行の識別（最後の区切りのフォルダ。開閉の状態の鍵）。 */
  key: FolderId;
  volumeId: VolumeId;
  /** 先頭から順に。最後の区切りの子が `children`。 */
  segments: FolderSegment[];
  children: FolderNode[];
}

/** 表示する行（開いている行の子だけを並べたもの）。 */
export interface FolderRow {
  node: FolderNode;
  depth: number;
  hasChildren: boolean;
  open: boolean;
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** ボリュームのルートの表示名（パスの末尾の区切りを除く。Unix のルートは "/"）。 */
export function rootName(f: FolderInfo): string {
  if (f.path) {
    const trimmed = f.path.replace(/[\\/]+$/, "");
    return trimmed === "" ? "/" : trimmed;
  }
  return `ボリューム ${f.volume_id}`;
}

/** フォルダの表示名（相対パスの最後の部分。ルートはボリュームのパス）。 */
export function folderName(f: FolderInfo): string {
  if (f.parent_id === null || f.rel_path === "") return rootName(f);
  const parts = f.rel_path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? f.rel_path;
}

function segmentOf(f: FolderInfo): FolderSegment {
  return { id: f.folder_id, name: folderName(f), fullPath: f.path ?? f.rel_path };
}

/** フォルダの一覧から木を作る（親が一覧にないフォルダは根として扱う）。 */
export function buildFolderTree(folders: FolderInfo[]): FolderNode[] {
  const byId = new Map<FolderId, FolderInfo>();
  for (const f of folders) byId.set(f.folder_id, f);
  const children = new Map<FolderId, FolderInfo[]>();
  const roots: FolderInfo[] = [];
  for (const f of folders) {
    if (f.parent_id !== null && f.parent_id !== f.folder_id && byId.has(f.parent_id)) {
      const list = children.get(f.parent_id) ?? [];
      list.push(f);
      children.set(f.parent_id, list);
    } else {
      roots.push(f);
    }
  }
  const byName = (a: FolderInfo, b: FolderInfo) =>
    collator.compare(folderName(a), folderName(b)) || a.folder_id - b.folder_id;
  for (const list of children.values()) list.sort(byName);
  roots.sort((a, b) => a.volume_id - b.volume_id || byName(a, b));

  // 親をたどる輪（壊れたデータ）で止まらないよう、訪れたフォルダを覚える。
  const visited = new Set<FolderId>();
  const build = (start: FolderInfo): FolderNode => {
    const segments: FolderSegment[] = [];
    let cur = start;
    for (;;) {
      visited.add(cur.folder_id);
      segments.push(segmentOf(cur));
      const kids = (children.get(cur.folder_id) ?? []).filter((k) => !visited.has(k.folder_id));
      if (kids.length === 1) {
        cur = kids[0];
        continue;
      }
      return {
        key: cur.folder_id,
        volumeId: start.volume_id,
        segments,
        children: kids.filter((k) => !visited.has(k.folder_id)).map((k) => build(k)),
      };
    }
  };
  return roots.filter((r) => !visited.has(r.folder_id)).map((r) => build(r));
}

/**
 * 開いている行の子をたどって、表示する行を並べる。`isOpen` は行の鍵と深さから開閉を返す。
 */
export function visibleFolderRows(
  roots: FolderNode[],
  isOpen: (key: FolderId, depth: number) => boolean,
): FolderRow[] {
  const out: FolderRow[] = [];
  const walk = (nodes: FolderNode[], depth: number) => {
    for (const node of nodes) {
      const hasChildren = node.children.length > 0;
      const open = hasChildren && isOpen(node.key, depth);
      out.push({ node, depth, hasChildren, open });
      if (open) walk(node.children, depth + 1);
    }
  };
  walk(roots, 0);
  return out;
}

/** フォルダを含む行の鍵の一覧（根から順。選んだフォルダが見えるように開くのに使う）。 */
export function ancestorKeys(roots: FolderNode[], id: FolderId): FolderId[] | null {
  const path: FolderId[] = [];
  const find = (nodes: FolderNode[]): boolean => {
    for (const node of nodes) {
      path.push(node.key);
      if (node.segments.some((s) => s.id === id)) return true;
      if (find(node.children)) return true;
      path.pop();
    }
    return false;
  };
  return find(roots) ? path : null;
}
