// ライブラリの左のパネル: カタログの件数・「すべての写真」・フォルダツリー（LIB-06）・コレクション
// （LIB-10・LIB-11。CollectionPanel）・キーワード（LIB-09。KeywordPanel）。
//
// - どこを見るか（ソース）は、すべての写真・フォルダ・コレクションのどれか 1 つ（actions.showFolder・
//   showCollection・showAll。ほかの絞り込みの条件は残す）。
// - フォルダは core.folders()（取り込み・削除の後（foldersVersion）に読み直す）。parent_id で木にし、
//   ボリュームごとの根から並べる（folderTree.ts）。
// - フォルダを選ぶと、そのフォルダで絞り込む（actions.setFilter の folder_id・include_subfolders）。
//   ほかの絞り込みの条件はそのまま残す。
// - 行の開閉と「サブフォルダを含める」は、ブラウザに保存する（prefs.ts）。行の開閉はフォルダの ID に
//   結びつくので、カタログごとに分ける。
// - フォルダの印（FILE-03・FILE-04）: つながっていないドライブのフォルダは薄く「未接続」、見つからないフォルダは
//   「!」、見つからない写真のあるフォルダはその数を出す。つながっていないドライブの名前は一覧の下に出す。
//   ドライブのつながりは、ウィンドウに戻ったときに確かめ直す（state/files.ts の refreshVolumes）。
// - フォルダの右クリック（FILE-02・FILE-04）: 写真をここへ移す・フォルダを作る・名前の変更・移動・見つからない
//   写真の確認と表示・場所の指定し直し。写真をフォルダに落とすと、移動のダイアログを開く（library/photoDrag.ts）。

import { Fragment, useEffect, useMemo, useRef, useState } from "react";

import * as core from "../api/core";
import type { CatalogStats, FolderId, FolderInfo, VolumeInfo } from "../api/types";
import { MENU_SEPARATOR, type MenuAt, MenuButton, type MenuEntry, PopupMenu } from "../components/Menu";
import { Section } from "../components/Section";
import { actions, useApp } from "../state/app";
import { openMoveFiles, openRelink, refreshVolumes, showMissingOnly, startCheckMissing } from "../state/files";
import { CollectionPanel } from "./CollectionPanel";
import { folderMarks, offlineVolumesText } from "./fileOpsLogic";
import {
  ancestorKeys,
  buildFolderTree,
  folderName,
  type FolderNode,
  type FolderSegment,
  layoutSegments,
  visibleFolderRows,
} from "./folderTree";
import { KeywordPanel } from "./KeywordPanel";
import "./library.css";
import { usePhotoDrag } from "./photoDrag";
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
  const collectionId = useApp((s) => s.filter.collection_id ?? null);
  const includeSubfolders = useApp((s) => s.filter.include_subfolders ?? false);
  const foldersVersion = useApp((s) => s.foldersVersion);
  const detailsVersion = useApp((s) => s.detailsVersion);
  const catalogPath = useApp((s) => s.catalog?.catalog_path ?? null);
  const openKey = catalogKey("folderOpen", catalogPath);

  const [folders, setFolders] = useState<FolderInfo[] | null>(null);
  const [volumes, setVolumes] = useState<VolumeInfo[] | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ at: MenuAt; folder: FolderInfo } | null>(null);
  const [moreMenu, setMoreMenu] = useState<{ at: MenuAt; segments: FolderSegment[] } | null>(null);
  const drag = usePhotoDrag();
  const [reload, setReload] = useState(0);
  const [stats, setStats] = useState<CatalogStats | null>(null);
  const [subPref, setSubPref] = useState(() => loadBoolean("includeSubfolders", true));
  const [openState, setOpenState] = useState(() => ({ key: openKey, open: loadOpen(openKey) }));
  if (openState.key !== openKey) {
    // カタログが変わった: そのカタログの開閉を読み直し、前のカタログのフォルダを出さない（描画中に直す）。
    setOpenState({ key: openKey, open: loadOpen(openKey) });
    setFolders(null);
    setVolumes(null);
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

  // ボリュームの一覧（つながっていないドライブの案内。フォルダの一覧と一緒に読み直す）。
  useEffect(() => {
    let alive = true;
    core
      .volumes(false)
      .then((v) => alive && setVolumes(v))
      .catch((e: unknown) => console.warn("ボリュームの一覧を読めません", e));
    return () => {
      alive = false;
    };
  }, [foldersVersion, reload]);

  // ウィンドウに戻ったとき（ドライブをつないだ・外した後など）に、ドライブのつながりを確かめ直す。
  useEffect(() => {
    let alive = true;
    const onFocus = () =>
      void refreshVolumes().then((v) => {
        if (alive && v) setVolumes(v);
      });
    window.addEventListener("focus", onFocus);
    return () => {
      alive = false;
      window.removeEventListener("focus", onFocus);
    };
  }, []);

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
  const byId = useMemo(() => new Map((folders ?? []).map((f) => [f.folder_id, f])), [folders]);
  const offlineText = volumes ? offlineVolumesText(volumes) : null;
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

  const selectFolder = (id: FolderId) => actions.showFolder(id, subPref);

  // フォルダを選んでいる間は、その条件を表示する（グリッドの「サブフォルダを含める」でも変わる）。
  const subChecked = folderId !== null ? includeSubfolders : subPref;
  const changeSub = (v: boolean) => {
    setSubPref(v);
    save("includeSubfolders", v);
    if (folderId !== null) actions.setFilter({ include_subfolders: v });
  };

  const recheckVolumes = () =>
    void refreshVolumes(true).then((v) => {
      if (!v) return;
      setVolumes(v);
      const text = offlineVolumesText(v);
      actions.notify("info", text ? `ドライブのつながりを確かめました。${text}` : "ドライブのつながりを確かめました（すべてつながっています）");
    });

  // フォルダの操作（右クリック・行の「…」）。
  const folderMenu = (f: FolderInfo): MenuEntry[] => {
    const marks = folderMarks(f);
    const isRoot = f.parent_id === null || f.rel_path === "";
    const name = folderName(f);
    const targets = actions.targets().length;
    return [
      {
        label: targets > 0 ? `操作の対象の写真（${targets.toLocaleString("ja-JP")} 枚）をここへ移す…` : "操作の対象の写真をここへ移す…",
        disabled: targets === 0 || !marks.droppable,
        onSelect: () => openMoveFiles(f.folder_id),
      },
      MENU_SEPARATOR,
      {
        label: "フォルダを作る…",
        disabled: !marks.droppable,
        onSelect: () => actions.openDialog({ kind: "folder_name", mode: "create", folderId: f.folder_id }),
      },
      {
        label: "名前を変える…",
        disabled: isRoot || !marks.droppable,
        onSelect: () => actions.openDialog({ kind: "folder_name", mode: "rename", folderId: f.folder_id, currentName: name }),
      },
      {
        label: "移動…",
        disabled: isRoot || !marks.droppable,
        onSelect: () => actions.openDialog({ kind: "move_folder", folderId: f.folder_id }),
      },
      MENU_SEPARATOR,
      {
        label: "見つからない写真を確かめる",
        disabled: marks.offline,
        onSelect: () => void startCheckMissing(f.folder_id, name),
      },
      {
        label: "見つからない写真だけを表示",
        onSelect: () => showMissingOnly(f.folder_id),
      },
      {
        label: "場所を指定し直す…",
        disabled: isRoot,
        onSelect: () => actions.openDialog({ kind: "relocate_folder", folderId: f.folder_id }),
      },
      {
        label: "カタログから除く…",
        onSelect: () => actions.openDialog({ kind: "remove_folder", folderId: f.folder_id }),
      },
    ];
  };
  const allFoldersMenu = (): MenuEntry[] => [
    { label: "見つからない写真を確かめる（すべて）", onSelect: () => void startCheckMissing(null) },
    { label: "見つからない写真だけを表示", onSelect: () => showMissingOnly() },
    { label: "見つからない写真を探す…", onSelect: () => openRelink(null) },
    MENU_SEPARATOR,
    { label: "ドライブのつながりを確かめ直す", onSelect: recheckVolumes },
  ];

  /** 行の 1 つのフォルダ（選ぶ・右クリック・写真を落とす先・印）。`first` でなければ前に区切りを置く。 */
  const renderSeg = (seg: FolderSegment, first: boolean) => {
    const info = byId.get(seg.id);
    const marks = info ? folderMarks(info) : null;
    const over = drag?.target?.kind === "folder" && drag.target.id === seg.id;
    const cls = [
      "lib-tree-seg",
      seg.id === folderId ? "on" : "",
      marks?.offline ? "offline" : "",
      marks?.missing ? "missing" : "",
      over ? (marks?.droppable ? "drop-over" : "drop-no") : "",
    ]
      .filter(Boolean)
      .join(" ");
    return (
      <Fragment key={seg.id}>
        {!first && <span className="lib-tree-sep">›</span>}
        <span
          className={cls}
          title={marks?.title ?? seg.fullPath}
          onClick={() => selectFolder(seg.id)}
          onContextMenu={(e) => {
            if (!info) return;
            e.preventDefault();
            setMenu({ at: { x: e.clientX, y: e.clientY }, folder: info });
          }}
          data-drop-kind="folder"
          data-drop-id={seg.id}
          data-drop-ok={marks?.droppable ? "1" : "0"}
          data-drop-label={seg.name}
        >
          {seg.name}
        </span>
        {marks?.missing && (
          <span className="lib-tree-mark warn" aria-label="フォルダが見つかりません">
            !
          </span>
        )}
        {marks && !marks.missing && marks.missingFiles > 0 && (
          <span className="lib-tree-mark warn" aria-label={`見つからない写真 ${marks.missingFiles} 件`}>
            !{marks.missingFiles.toLocaleString("ja-JP")}
          </span>
        )}
      </Fragment>
    );
  };

  /** 行のフォルダ（区切りが多ければ、間を「…」にまとめ、「…」のメニューから選べるようにする）。 */
  const renderSegments = (node: FolderNode) => {
    const lay = layoutSegments(node.segments);
    if (lay.hidden.length === 0) return lay.head.map((seg, i) => renderSeg(seg, i === 0));
    const hidden = lay.hidden;
    // まとめたフォルダの「!」（見つからないフォルダ・写真）は見えなくなるので、「…」を注意の色にする。
    const warn = hidden.some((s) => {
      const info = byId.get(s.id);
      const marks = info ? folderMarks(info) : null;
      return !!marks && (marks.missing || marks.missingFiles > 0);
    });
    return (
      <>
        {lay.head.map((seg, i) => renderSeg(seg, i === 0))}
        <span className="lib-tree-sep">›</span>
        <button
          type="button"
          className={`lib-tree-more${hidden.some((s) => s.id === folderId) ? " on" : ""}${warn ? " warn" : ""}`}
          title={hidden.map((s) => s.name).join(" › ")}
          aria-label={`途中のフォルダ（${hidden.length} 個）を選ぶ`}
          onClick={(e) => {
            const b = e.currentTarget.getBoundingClientRect();
            setMoreMenu({ at: { x: b.left, y: b.bottom }, segments: hidden });
          }}
        >
          …
        </button>
        {lay.tail.map((seg) => renderSeg(seg, false))}
      </>
    );
  };

  return (
    <div className="lib-left">
      <Section id="lib.catalog" title="カタログ">
        <div
          className={`lib-nav-row${folderId === null && collectionId === null ? " on" : ""}`}
          role="button"
          tabIndex={0}
          onClick={() => actions.showAll()}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              actions.showAll();
            }
          }}
          title="フォルダ・コレクションで絞り込まない（ほかの条件は残す）"
        >
          <span>すべての写真</span>
          <span className="count" title="カタログの写真と動画の数（仮想コピーを含む）">
            {stats ? `${stats.variants.toLocaleString("ja-JP")} 件` : "—"}
          </span>
        </div>
      </Section>

      <Section id="lib.folders" title="フォルダ" right={<MenuButton label="…" title="フォルダの操作" items={allFoldersMenu} />}>
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
                <span className="lib-tree-segs">{renderSegments(r.node)}</span>
                {byId.get(r.node.key)?.online === false && <span className="lib-tree-mark muted">未接続</span>}
              </div>
            ))}
          </div>
        )}
        {offlineText && (
          <p className="hint" title="つなぐと、そのドライブの写真の表示・現像・書き出しができます">
            {offlineText}
          </p>
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

      {menu && <PopupMenu at={menu.at} items={folderMenu(menu.folder)} onClose={() => setMenu(null)} label="フォルダの操作" />}
      {moreMenu && (
        <PopupMenu
          at={moreMenu.at}
          items={moreMenu.segments.map((s) => ({
            label: s.name,
            checked: s.id === folderId,
            onSelect: () => selectFolder(s.id),
          }))}
          onClose={() => setMoreMenu(null)}
          label="途中のフォルダ"
        />
      )}
      <CollectionPanel />
      <KeywordPanel />
    </div>
  );
}
