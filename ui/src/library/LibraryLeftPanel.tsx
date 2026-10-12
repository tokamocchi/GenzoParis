// ライブラリの左のパネル: カタログの件数・「すべての写真」・フォルダツリー（LIB-06）。
//
// - フォルダは core.folders()（取り込み・削除の後（foldersVersion）に読み直す）。parent_id で木にし、
//   ボリュームごとの根から並べる（folderTree.ts）。
// - フォルダを選ぶと、そのフォルダで絞り込む（actions.setFilter の folder_id・include_subfolders）。
//   ほかの絞り込みの条件はそのまま残す。
// - 行の開閉と「サブフォルダを含める」は、ブラウザに保存する（prefs.ts）。行の開閉はフォルダの ID に
//   結びつくので、カタログごとに分ける。

import { Fragment, useEffect, useMemo, useRef, useState } from "react";

import * as core from "../api/core";
import type { CatalogStats, FolderId, FolderInfo } from "../api/types";
import { Section } from "../components/Section";
import { actions, useApp } from "../state/app";
import { ancestorKeys, buildFolderTree, type FolderNode, visibleFolderRows } from "./folderTree";
import "./library.css";
import { catalogKey, loadBoolean, loadJson, save } from "./prefs";

/** 評価などの変更（detailsVersion）の後に、カタログの件数を読み直すまでの時間（ms。仮置き）。 */
const STATS_DELAY_MS = 1000;

type OpenMap = Record<string, boolean>;

function isOpenMap(v: unknown): v is OpenMap {
  return (
    typeof v === "object" &&
    v !== null &&
    !Array.isArray(v) &&
    Object.values(v as Record<string, unknown>).every((x) => typeof x === "boolean")
  );
}

/** 保存した行の開閉（鍵がない（カタログを開いていない）・読めないときは空）。 */
function loadOpen(key: string | null): OpenMap {
  return key ? loadJson(key, {}, isOpenMap) : {};
}

/** 行の開閉（保存した値がなければ、ボリュームの根の行だけを開く）。 */
function isOpenIn(open: OpenMap, key: FolderId, depth: number): boolean {
  return open[String(key)] ?? depth === 0;
}

function collectKeys(nodes: FolderNode[], out: Set<string>): Set<string> {
  for (const n of nodes) {
    out.add(String(n.key));
    collectKeys(n.children, out);
  }
  return out;
}

export function LibraryLeftPanel() {
  const folderId = useApp((s) => s.filter.folder_id ?? null);
  const includeSubfolders = useApp((s) => s.filter.include_subfolders ?? false);
  const foldersVersion = useApp((s) => s.foldersVersion);
  const detailsVersion = useApp((s) => s.detailsVersion);
  const catalogPath = useApp((s) => s.catalog?.catalog_path ?? null);
  const openKey = catalogKey("folderOpen", catalogPath);

  const [folders, setFolders] = useState<FolderInfo[] | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [stats, setStats] = useState<CatalogStats | null>(null);
  const [subPref, setSubPref] = useState(() => loadBoolean("includeSubfolders", true));
  const [openState, setOpenState] = useState(() => ({ key: openKey, open: loadOpen(openKey) }));
  if (openState.key !== openKey) {
    // カタログが変わった: そのカタログの開閉を読み直し、前のカタログのフォルダを出さない（描画中に直す）。
    setOpenState({ key: openKey, open: loadOpen(openKey) });
    setFolders(null);
    setStats(null);
  }
  const open = openState.open;

  // フォルダの一覧（古い応答は捨てる）。
  useEffect(() => {
    let alive = true;
    core
      .folders()
      .then((f) => {
        if (!alive) return;
        setFolders(f);
        setFolderError(null);
      })
      .catch((e: unknown) => {
        if (alive) setFolderError(actions.describeError(e));
      });
    return () => {
      alive = false;
    };
  }, [foldersVersion, reload]);

  // カタログの件数（取り込み・削除の後はすぐに、仮想コピーの追加などの後は落ち着いてから読み直す。取り込み中は
  // state/app.ts が合図（foldersVersion）を IMPORT_REFRESH_MS に 1 回にまとめる）。
  const statsFolders = useRef<number | null>(null);
  useEffect(() => {
    const immediate = statsFolders.current !== foldersVersion;
    let alive = true;
    const t = setTimeout(
      () => {
        // 要求を始めたときに覚える（取り消した実行では覚えない）。
        statsFolders.current = foldersVersion;
        core
          .catalogStats()
          .then((s) => {
            if (alive) setStats(s);
          })
          .catch((e: unknown) => console.warn("カタログの件数を読めません", e));
      },
      immediate ? 0 : STATS_DELAY_MS,
    );
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [foldersVersion, detailsVersion]);

  const tree = useMemo(() => buildFolderTree(folders ?? []), [folders]);
  const rows = visibleFolderRows(tree, (key, depth) => isOpenIn(open, key, depth));

  // 選んだフォルダ（外から変わった場合を含む）が見えるように、親の行を開く。
  useEffect(() => {
    if (folderId === null) return;
    const keys = ancestorKeys(tree, folderId);
    if (!keys) return;
    setOpenState((prev) => {
      const parents = keys.slice(0, -1);
      if (parents.every((k, depth) => isOpenIn(prev.open, k, depth))) return prev;
      const next = { ...prev.open };
      for (const k of parents) next[String(k)] = true;
      return { key: prev.key, open: next };
    });
  }, [folderId, tree]);

  const toggle = (key: FolderId, depth: number) => {
    const next: OpenMap = { ...open, [String(key)]: !isOpenIn(open, key, depth) };
    // 一覧にないフォルダ（削除したもの）の記録は捨てる。
    if (folders) {
      const known = collectKeys(tree, new Set());
      for (const k of Object.keys(next)) if (!known.has(k)) delete next[k];
    }
    setOpenState({ key: openKey, open: next });
    if (openKey) save(openKey, next);
  };

  const selectFolder = (id: FolderId) => actions.setFilter({ folder_id: id, include_subfolders: subPref });

  // フォルダを選んでいる間は、その条件を表示する（グリッドの「サブフォルダを含める」でも変わる）。
  const subChecked = folderId !== null ? includeSubfolders : subPref;
  const changeSub = (v: boolean) => {
    setSubPref(v);
    save("includeSubfolders", v);
    if (folderId !== null) actions.setFilter({ include_subfolders: v });
  };

  return (
    <div className="lib-left">
      <Section id="lib.catalog" title="カタログ">
        <div
          className={`lib-nav-row${folderId === null ? " on" : ""}`}
          role="button"
          tabIndex={0}
          onClick={() => actions.setFilter({ folder_id: null })}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              actions.setFilter({ folder_id: null });
            }
          }}
          title="フォルダで絞り込まない（ほかの条件は残す）"
        >
          <span>すべての写真</span>
          <span className="count" title="カタログの写真と動画の数（仮想コピーを含む）">
            {stats ? `${stats.variants.toLocaleString("ja-JP")} 件` : "—"}
          </span>
        </div>
      </Section>

      <Section id="lib.folders" title="フォルダ">
        {folderError && (
          <div>
            <p className="error-text">フォルダの一覧を読めません: {folderError}</p>
            <button type="button" className="btn small" onClick={() => setReload((n) => n + 1)}>
              読み直す
            </button>
          </div>
        )}
        {folders && folders.length === 0 && <p className="hint">取り込んだフォルダはまだありません。</p>}
        {rows.length > 0 && (
          <div className="lib-tree" role="tree" aria-label="フォルダ">
            {rows.map((r) => (
              <div
                key={r.node.key}
                className="lib-tree-row"
                role="treeitem"
                aria-expanded={r.hasChildren ? r.open : undefined}
                style={{ paddingLeft: 4 + r.depth * 12 }}
              >
                <button
                  type="button"
                  className="lib-tree-chev"
                  disabled={!r.hasChildren}
                  aria-label={r.hasChildren ? (r.open ? "閉じる" : "開く") : undefined}
                  onClick={() => toggle(r.node.key, r.depth)}
                >
                  {r.hasChildren ? (r.open ? "▾" : "▸") : ""}
                </button>
                <span className="lib-tree-segs">
                  {r.node.segments.map((seg, i) => (
                    <Fragment key={seg.id}>
                      {i > 0 && <span className="lib-tree-sep">›</span>}
                      <span
                        className={`lib-tree-seg${seg.id === folderId ? " on" : ""}`}
                        title={seg.fullPath}
                        onClick={() => selectFolder(seg.id)}
                      >
                        {seg.name}
                      </span>
                    </Fragment>
                  ))}
                </span>
              </div>
            ))}
          </div>
        )}
        <label className="check" title="選んだフォルダの下のフォルダの写真も表示する">
          <input type="checkbox" checked={subChecked} onChange={(e) => changeSub(e.target.checked)} />
          サブフォルダを含める
        </label>
        <div className="actions">
          <button type="button" className="btn small" onClick={() => actions.openDialog({ kind: "import" })}>
            フォルダを取り込む
          </button>
        </div>
      </Section>
    </div>
  );
}
