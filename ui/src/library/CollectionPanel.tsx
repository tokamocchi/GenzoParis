// 左のパネルのコレクションの一覧（LIB-10・LIB-11）。
//
// - 一覧は core.collections()。まず数を数えずに出し、スマートコレクション・セットの数は後から数えて
//   入れる（50 万件では 1 つあたり 0.1〜0.4 秒ほどかかるため。コアは読み取り専用の接続で数える）。
//   キーワード・コレクション・評価などの変更（organizeVersion）の後に読み直す。
// - 名前をクリックすると、そのコレクションを表示する（フォルダの選択は外す。ほかの絞り込みの条件は残す）。
// - セットの開閉はカタログごとにブラウザに保存する。並びはセットを先に、それぞれ名前の順。
// - ターゲットコレクション（B キーの入れ先）には ＋ の印を付ける。
// - 「…」・右クリックで、ターゲットにする・選んだ写真を入れる・条件の編集・中に作る・名前の変更・移動・削除。
// - グリッドの写真を手動のコレクションの行に落とすと、そのコレクションに入れる（library/photoDrag.ts）。

import { useEffect, useMemo, useRef, useState } from "react";

import * as core from "../api/core";
import type { CollectionId, CollectionInfo } from "../api/types";
import { MENU_SEPARATOR, type MenuAt, MenuButton, type MenuEntry, PopupMenu } from "../components/Menu";
import { Section } from "../components/Section";
import { actions, useApp } from "../state/app";
import { addToCollection, setTargetCollection } from "../state/organize";
import {
  buildCollectionTree,
  COLLECTION_KIND_LABEL,
  type CollectionNode,
  countText,
  visibleCollectionRows,
} from "./collectionLogic";
import { keywordPath, keywordPathText } from "./keywordLogic";
import "./library.css";
import { usePhotoDrag } from "./photoDrag";
import { catalogKey, loadJson, save } from "./prefs";
import { describeRule, type NameLookup } from "./smartRuleLogic";

/** 一覧を出してから、スマートコレクション・セットの数を数えるまでの時間（ms。続けて変わるときにまとめる。仮置き）。 */
const COUNT_DELAY_MS = 400;

type OpenMap = Record<string, boolean>;

function isOpenMap(v: unknown): v is OpenMap {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

const KIND_ICON: Record<CollectionInfo["kind"], string> = { set: "▤", manual: "▣", smart: "⚙" };

export function CollectionPanel() {
  const organizeVersion = useApp((s) => s.organizeVersion);
  const catalogPath = useApp((s) => s.catalog?.catalog_path ?? null);
  const shown = useApp((s) => s.filter.collection_id ?? null);
  const target = useApp((s) => s.targetCollection);
  const offset = useApp((s) => s.catalog?.core_settings.default_utc_offset_minutes ?? 0);
  const openKey = catalogKey("collectionOpen", catalogPath);
  const [list, setList] = useState<CollectionInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [open, setOpen] = useState<OpenMap>(() => (openKey ? loadJson(openKey, {}, isOpenMap) : {}));
  const [menu, setMenu] = useState<{ at: MenuAt; node: CollectionInfo } | null>(null);
  const [keywordNames, setKeywordNames] = useState<Map<number, string>>(new Map());
  const [folderNames, setFolderNames] = useState<Map<number, string>>(new Map());
  const drag = usePhotoDrag();
  const lastOpenKey = useRef(openKey);
  if (lastOpenKey.current !== openKey) {
    lastOpenKey.current = openKey;
    setOpen(openKey ? loadJson(openKey, {}, isOpenMap) : {});
    setList(null);
  }

  useEffect(() => {
    let alive = true;
    core
      .collections(false)
      .then((l) => {
        if (!alive) return;
        // 数えた数は、数え直すまで前の値を残す（数字がちらつかないように）。
        setList((prev) => l.map((c) => (c.count !== null ? c : { ...c, count: prev?.find((p) => p.collection_id === c.collection_id)?.count ?? null })));
        setError(null);
      })
      .catch((e: unknown) => alive && setError(actions.describeError(e)));
    const t = setTimeout(() => {
      core
        .collections(true)
        .then((l) => alive && setList(l))
        .catch(() => undefined);
    }, COUNT_DELAY_MS);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [organizeVersion, reload, catalogPath]);

  // 条件の説明（ツールチップ）に使うキーワード・フォルダの名前。
  const hasSmart = list?.some((c) => c.kind === "smart") ?? false;
  useEffect(() => {
    if (!hasSmart) return;
    let alive = true;
    core
      .keywordTree()
      .then((ks) => alive && setKeywordNames(new Map(ks.map((k) => [k.keyword_id, keywordPathText(keywordPath(ks, k.keyword_id))]))))
      .catch(() => undefined);
    core
      .folders()
      .then((fs) => alive && setFolderNames(new Map(fs.map((f) => [f.folder_id, f.path ?? f.rel_path]))))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [hasSmart, organizeVersion]);

  const tree = useMemo(() => buildCollectionTree(list ?? []), [list]);
  const rows = visibleCollectionRows(tree, (id) => open[String(id)] ?? true);
  const toggle = (n: CollectionNode) => {
    const id = String(n.info.collection_id);
    const next = { ...open, [id]: !(open[id] ?? true) };
    setOpen(next);
    if (openKey) save(openKey, next);
  };
  const names: NameLookup = {
    keyword: (id) => keywordNames.get(id),
    folder: (id) => folderNames.get(id),
    collection: (id) => list?.find((c) => c.collection_id === id)?.name,
  };

  const createItems = (parentId: CollectionId | null): MenuEntry[] => [
    {
      label: "コレクションを作る…",
      onSelect: () =>
        actions.openDialog({
          kind: "collection",
          mode: "create",
          collectionKind: "manual",
          parentId,
          variantIds: actions.targets(),
        }),
    },
    { label: "スマートコレクションを作る…", onSelect: () => actions.openDialog({ kind: "smart_collection", parentId }) },
    {
      label: "コレクションセットを作る…",
      onSelect: () => actions.openDialog({ kind: "collection", mode: "create", collectionKind: "set", parentId }),
    },
  ];

  const menuItems = (c: CollectionInfo): MenuEntry[] => {
    const id = c.collection_id;
    const items: MenuEntry[] = [];
    if (c.kind === "manual") {
      items.push(
        {
          label: "ターゲットコレクションにする（B で入れる・外す）",
          checked: target === id,
          onSelect: () => setTargetCollection(target === id ? null : id),
        },
        {
          label: "操作の対象の写真を入れる",
          disabled: actions.targets().length === 0,
          onSelect: () => void addToCollection(id, actions.targets(), c.name),
        },
        MENU_SEPARATOR,
      );
    }
    if (c.kind === "smart") {
      items.push({ label: "条件を編集…", onSelect: () => actions.openDialog({ kind: "smart_collection", collectionId: id }) }, MENU_SEPARATOR);
    }
    if (c.kind === "set") items.push(...createItems(id), MENU_SEPARATOR);
    items.push(
      { label: "名前を変える…", onSelect: () => actions.openDialog({ kind: "collection", mode: "rename", collectionId: id }) },
      { label: "移動…", onSelect: () => actions.openDialog({ kind: "collection", mode: "move", collectionId: id }) },
      MENU_SEPARATOR,
      { label: "削除…", danger: true, onSelect: () => actions.openDialog({ kind: "collection_delete", collectionId: id }) },
    );
    return items;
  };

  return (
    <Section
      id="lib.collections"
      title={`コレクション${list ? `（${list.length.toLocaleString("ja-JP")}）` : ""}`}
      right={<MenuButton label="＋" title="コレクションを作る" items={() => createItems(null)} />}
    >
      {error && (
        <div>
          <p className="error-text">コレクションの一覧を読めません: {error}</p>
          <button type="button" className="btn small" onClick={() => setReload((n) => n + 1)}>
            読み直す
          </button>
        </div>
      )}
      {list && list.length === 0 && (
        <p className="hint">
          コレクションはまだありません。＋ から作るか、写真を選んで Ctrl / ⌘ + N で作れます。
        </p>
      )}
      {rows.length > 0 && (
        <div className="lib-tree" role="tree" aria-label="コレクション">
          {rows.map((r) => {
            const c = r.node.info;
            const on = shown === c.collection_id;
            const dropOver = drag?.target?.kind === "collection" && drag.target.id === c.collection_id;
            const title =
              c.kind === "smart"
                ? c.rule_error
                  ? `条件を読めません: ${c.rule_error}`
                  : c.rule
                    ? `条件: ${describeRule(c.rule, names, offset)}`
                    : ""
                : COLLECTION_KIND_LABEL[c.kind];
            return (
              <div
                key={c.collection_id}
                className={`lib-tree-row lib-organize-row${dropOver ? (c.kind === "manual" ? " drop-over" : " drop-no") : ""}`}
                role="treeitem"
                aria-expanded={r.hasChildren ? r.open : undefined}
                aria-selected={on}
                style={{ paddingLeft: 4 + r.depth * 12 }}
                data-drop-kind="collection"
                data-drop-id={c.collection_id}
                data-drop-ok={c.kind === "manual" ? "1" : "0"}
                data-drop-label={c.name}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setMenu({ at: { x: e.clientX, y: e.clientY }, node: c });
                }}
              >
                <button
                  type="button"
                  className="lib-tree-chev"
                  disabled={!r.hasChildren}
                  aria-label={r.hasChildren ? (r.open ? "閉じる" : "開く") : undefined}
                  onClick={() => toggle(r.node)}
                >
                  {r.hasChildren ? (r.open ? "▾" : "▸") : ""}
                </button>
                <span className="lib-organize-icon" aria-hidden="true">
                  {KIND_ICON[c.kind]}
                </span>
                <span
                  className={`lib-tree-seg lib-organize-name${on ? " on" : ""}`}
                  title={title}
                  onClick={() => actions.showCollection(c.collection_id)}
                >
                  {c.name}
                </span>
                {target === c.collection_id && (
                  <span className="lib-organize-target" title="ターゲットコレクション（B で入れる・外す）">
                    ＋
                  </span>
                )}
                <span className={`lib-organize-count${c.rule_error ? " warn" : ""}`} title={c.rule_error ?? undefined}>
                  {countText(c)}
                </span>
                <MenuButton label="…" title={`${c.name} の操作`} items={() => menuItems(c)} />
              </div>
            );
          })}
        </div>
      )}
      {menu && <PopupMenu at={menu.at} items={menuItems(menu.node)} onClose={() => setMenu(null)} label="コレクションの操作" />}
    </Section>
  );
}
