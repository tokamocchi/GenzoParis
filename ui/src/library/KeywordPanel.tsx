// 左のパネルのキーワードの一覧（LIB-09）。
//
// - 一覧は core.keywordTree()（キーワード・コレクションの変更の後（organizeVersion）に読み直す）。木の開閉は
//   カタログごとにブラウザに保存する。上の欄で名前を絞り込める（一致するものと、その上の階層を開いて出す）。
// - 名前をクリックすると、そのキーワードで絞り込む（下の階層を含む。もう一度押すと外す）。
// - 行の左の印は、操作の対象の写真（グリッドでは選択中のすべて、ルーペでは表示中の 1 枚）への付き方
//   （✓ = すべて、− = 一部）。押すと付ける・外す。
// - 数は、そのキーワードが直接付いている写真の数（下の階層の分は数えない。Lightroom と同じ）。
// - 「…」・右クリックで、下にキーワードを作る・名前の変更・移動・書き出しに含める・削除。
// - グリッドの写真をキーワードの行に落とすと、そのキーワードを付ける（library/photoDrag.ts）。

import { useEffect, useMemo, useRef, useState } from "react";

import * as core from "../api/core";
import type { KeywordInfo, VariantKeywords } from "../api/types";
import { MENU_SEPARATOR, type MenuAt, MenuButton, type MenuEntry, PopupMenu } from "../components/Menu";
import { Section } from "../components/Section";
import { actions, useApp } from "../state/app";
import { setKeyword, toggleKeywordFilter } from "../state/organize";
import { buildKeywordTree, type KeywordNode, tagState, visibleKeywordRows } from "./keywordLogic";
import "./library.css";
import { usePhotoDrag } from "./photoDrag";
import { catalogKey, loadJson, save } from "./prefs";

/** 選択が変わってから、選択中の写真のキーワードを読むまでの時間（ms。仮置き）。 */
const SELECTION_DELAY_MS = 80;

type OpenMap = Record<string, boolean>;

function isOpenMap(v: unknown): v is OpenMap {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** 操作の対象の写真の ID の並びの鍵（選択が同じなら読み直さない）。 */
function targetsKey(): string {
  return actions.targets().join(",");
}

export function KeywordPanel() {
  const organizeVersion = useApp((s) => s.organizeVersion);
  const catalogPath = useApp((s) => s.catalog?.catalog_path ?? null);
  const filterKeywords = useApp((s) => s.filter.keywords ?? null);
  // 操作の対象（選択・表示モード）が変わったら、付き方を読み直す。
  const selKey = useApp(() => targetsKey());
  const openKey = catalogKey("keywordOpen", catalogPath);
  const [list, setList] = useState<KeywordInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<OpenMap>(() => (openKey ? loadJson(openKey, {}, isOpenMap) : {}));
  const [summary, setSummary] = useState<VariantKeywords | null>(null);
  const [menu, setMenu] = useState<{ at: MenuAt; node: KeywordInfo } | null>(null);
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
      .keywordTree()
      .then((l) => {
        if (!alive) return;
        setList(l);
        setError(null);
      })
      .catch((e: unknown) => alive && setError(actions.describeError(e)));
    return () => {
      alive = false;
    };
  }, [organizeVersion, reload, catalogPath]);

  useEffect(() => {
    let alive = true;
    const ids = selKey === "" ? [] : selKey.split(",").map(Number);
    if (ids.length === 0) {
      setSummary(null);
      return;
    }
    const t = setTimeout(() => {
      core
        .variantKeywords(ids)
        .then((s) => alive && setSummary(s))
        .catch(() => alive && setSummary(null));
    }, SELECTION_DELAY_MS);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [selKey, organizeVersion]);

  const tree = useMemo(() => buildKeywordTree(list ?? []), [list]);
  const rows = visibleKeywordRows(tree, (id, depth) => open[String(id)] ?? depth > 99, query);
  const toggle = (n: KeywordNode) => {
    const next = { ...open, [String(n.info.keyword_id)]: !(open[String(n.info.keyword_id)] ?? false) };
    setOpen(next);
    if (openKey) save(openKey, next);
  };
  const hasTargets = selKey !== "";

  const menuItems = (k: KeywordInfo): MenuEntry[] => [
    {
      label: "下にキーワードを作る…",
      onSelect: () => actions.openDialog({ kind: "keyword", mode: "create", parentId: k.keyword_id }),
    },
    {
      label: "名前を変える…",
      onSelect: () => actions.openDialog({ kind: "keyword", mode: "rename", keywordId: k.keyword_id }),
    },
    {
      label: "移動…",
      onSelect: () => actions.openDialog({ kind: "keyword", mode: "move", keywordId: k.keyword_id }),
    },
    {
      label: "書き出すファイルに含める",
      checked: k.include_on_export,
      onSelect: () =>
        void core
          .setKeywordExport(k.keyword_id, !k.include_on_export)
          .catch((e) => actions.reportError(e, "キーワードの設定を変えられません")),
    },
    MENU_SEPARATOR,
    {
      label: "このキーワードで絞り込む",
      onSelect: () => toggleKeywordFilter(k.keyword_id),
    },
    MENU_SEPARATOR,
    {
      label: "削除…",
      danger: true,
      onSelect: () => actions.openDialog({ kind: "keyword_delete", keywordId: k.keyword_id }),
    },
  ];

  return (
    <Section
      id="lib.keywords"
      title={`キーワード${list ? `（${list.length.toLocaleString("ja-JP")}）` : ""}`}
      right={
        <button
          type="button"
          className="btn ghost small"
          title="キーワードを作る"
          aria-label="キーワードを作る"
          onClick={() => actions.openDialog({ kind: "keyword", mode: "create", parentId: null })}
        >
          ＋
        </button>
      }
    >
      {error && (
        <div>
          <p className="error-text">キーワードの一覧を読めません: {error}</p>
          <button type="button" className="btn small" onClick={() => setReload((n) => n + 1)}>
            読み直す
          </button>
        </div>
      )}
      {list && list.length > 0 && (
        <input
          type="search"
          className="lib-organize-filter"
          placeholder="キーワードを探す"
          aria-label="キーワードを名前で探す"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      )}
      {list && list.length === 0 && (
        <p className="hint">キーワードはまだありません。右のパネルの「キーワード」の欄で写真に付けると、ここに並びます。</p>
      )}
      {rows.length > 0 && (
        <div className="lib-tree" role="tree" aria-label="キーワード">
          {rows.map((r) => {
            const k = r.node.info;
            const state = tagState(summary, k.keyword_id);
            const filtered = filterKeywords?.includes(k.keyword_id) ?? false;
            const dropOver = drag?.target?.kind === "keyword" && drag.target.id === k.keyword_id;
            return (
              <div
                key={k.keyword_id}
                className={`lib-tree-row lib-organize-row${dropOver ? " drop-over" : ""}`}
                role="treeitem"
                aria-expanded={r.hasChildren ? r.open : undefined}
                aria-selected={filtered}
                style={{ paddingLeft: 4 + r.depth * 12 }}
                data-drop-kind="keyword"
                data-drop-id={k.keyword_id}
                data-drop-ok="1"
                data-drop-label={k.name}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setMenu({ at: { x: e.clientX, y: e.clientY }, node: k });
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
                <button
                  type="button"
                  className={`lib-tag-box ${state}`}
                  disabled={!hasTargets}
                  role="checkbox"
                  aria-checked={state === "all" ? true : state === "some" ? "mixed" : false}
                  aria-label={`${k.name} を${state === "all" ? "外す" : "付ける"}`}
                  title={
                    hasTargets
                      ? state === "all"
                        ? "操作の対象の写真から外す"
                        : "操作の対象の写真に付ける"
                      : "写真を選ぶと、付ける・外すができます"
                  }
                  onClick={() => void setKeyword(actions.targets(), k.keyword_id, state !== "all")}
                >
                  {state === "all" ? "✓" : state === "some" ? "−" : ""}
                </button>
                <span
                  className={`lib-tree-seg lib-organize-name${filtered ? " on" : ""}`}
                  title={`${k.name}（クリックで絞り込み。下の階層を含む）${k.include_on_export ? "" : "・書き出すファイルには含めない"}`}
                  onClick={() => toggleKeywordFilter(k.keyword_id)}
                >
                  {k.name}
                  {!k.include_on_export && <span className="lib-organize-note"> ⊘</span>}
                </span>
                <span className="lib-organize-count">{k.photo_count > 0 ? k.photo_count.toLocaleString("ja-JP") : ""}</span>
                <MenuButton label="…" title={`${k.name} の操作`} items={() => menuItems(k)} />
              </div>
            );
          })}
        </div>
      )}
      {query !== "" && rows.length === 0 && list && list.length > 0 && <p className="hint">一致するキーワードはありません。</p>}
      {menu && <PopupMenu at={menu.at} items={menuItems(menu.node)} onClose={() => setMenu(null)} label="キーワードの操作" />}
    </Section>
  );
}
