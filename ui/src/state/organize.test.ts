// キーワード・コレクションの画面の操作（state/organize.ts）のテスト（コアのコマンドは MockBridge で差し替える）と、
// 部品が代表的な状態で例外なく描けることの確認（react-dom/server。useEffect は動かない）。

import { createElement } from "react";
import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";

import { installBridge } from "../api/bridge";
import { MOCK_COLLECTIONS, MockBridge } from "../api/mock";
import type { CatalogStatus } from "../api/types";
import { AddToCollectionDialog, CollectionDialog, KeywordDialog } from "../dialogs/OrganizeDialogs";
import { SmartCollectionDialog } from "../dialogs/SmartCollectionDialog";
import { CollectionPanel } from "../library/CollectionPanel";
import { KeywordEditor } from "../library/KeywordEditor";
import { KeywordPanel } from "../library/KeywordPanel";
import { appStore, targetCollectionKey } from "./app";
import {
  addKeywordText,
  deleteKeyAction,
  dropPhotos,
  setTargetCollection,
  toggleKeywordFilter,
  toggleTargetCollection,
} from "./organize";

let mock: MockBridge;
/** 送ったコアのコマンド（type ごとの引数）。 */
let sent: Record<string, unknown>[];
/** add_to_collection の応答の数（すでに入っていれば 0）。 */
let addResult: number;

function commands(type: string): Record<string, unknown>[] {
  return sent.filter((c) => c.type === type);
}

/** テストの実行環境（node）には localStorage がないので、Map で代わりを置く。 */
function installLocalStorage(): void {
  const data = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => void data.set(k, String(v)),
    removeItem: (k: string) => void data.delete(k),
    clear: () => data.clear(),
    key: (i: number) => [...data.keys()][i] ?? null,
    get length() {
      return data.size;
    },
  };
}

beforeEach(() => {
  installLocalStorage();
  mock = new MockBridge();
  installBridge(mock);
  sent = [];
  addResult = 2;
  mock.responses.set("core_execute", (args?: Record<string, unknown>) => {
    const c = args?.command as Record<string, unknown> & { type: string };
    sent.push(c);
    switch (c.type) {
      case "collections":
        return { type: "collections", collections: MOCK_COLLECTIONS };
      case "add_to_collection":
        return { type: "updated", count: addResult };
      case "remove_from_collection":
      case "add_keywords":
      case "remove_keywords":
        return { type: "updated", count: 2 };
      case "search":
        return { type: "search", generation: 1, count: 0 };
      default:
        return { type: "ok" };
    }
  });
  appStore.set({
    mode: "grid",
    filter: {},
    selection: { ids: [101, 102], current: 101, currentIndex: 0, anchorIndex: 0, anchorId: 101 },
    targetCollection: null,
    dialog: null,
    catalog: { catalog_path: "/tmp/c.db", core_settings: { default_utc_offset_minutes: 540 } } as CatalogStatus,
  });
});

describe("ターゲットコレクション（B）", () => {
  it("なければ入れる先を選ぶダイアログを開く", async () => {
    await toggleTargetCollection();
    expect(appStore.get().dialog).toEqual({ kind: "add_to_collection", variantIds: [101, 102] });
    expect(commands("add_to_collection")).toHaveLength(0);
  });

  it("入れる。すべて入っていれば（入れた数が 0）外す", async () => {
    setTargetCollection(11);
    expect(localStorage.getItem(targetCollectionKey("/tmp/c.db"))).toBe("11");
    await toggleTargetCollection();
    expect(commands("add_to_collection")).toEqual([{ type: "add_to_collection", collection_id: 11, variant_ids: [101, 102] }]);
    expect(commands("remove_from_collection")).toHaveLength(0);
    addResult = 0;
    await toggleTargetCollection();
    expect(commands("remove_from_collection")).toEqual([
      { type: "remove_from_collection", collection_id: 11, variant_ids: [101, 102] },
    ]);
    setTargetCollection(null);
    expect(localStorage.getItem(targetCollectionKey("/tmp/c.db"))).toBeNull();
  });

  it("ターゲットが手動のコレクションでなくなっていたら外して、選ぶダイアログを開く", async () => {
    setTargetCollection(12);
    await toggleTargetCollection();
    expect(appStore.get().targetCollection).toBeNull();
    expect(appStore.get().dialog?.kind).toBe("add_to_collection");
  });
});

describe("Delete キー", () => {
  it("手動のコレクションを表示している間は、そのコレクションから外す", async () => {
    appStore.set({ filter: { collection_id: 13 } });
    await deleteKeyAction();
    expect(commands("remove_from_collection")).toEqual([
      { type: "remove_from_collection", collection_id: 13, variant_ids: [101, 102] },
    ]);
    expect(appStore.get().dialog).toBeNull();
  });

  it("スマートコレクション・ほかの表示では、削除のダイアログ（カタログから除く）を開く", async () => {
    appStore.set({ filter: { collection_id: 12 } });
    await deleteKeyAction();
    expect(commands("remove_from_collection")).toHaveLength(0);
    expect(appStore.get().dialog).toEqual({ kind: "delete", deleteKind: "remove_from_catalog", variantIds: [101, 102] });
    appStore.set({ filter: {}, dialog: null });
    await deleteKeyAction();
    expect(appStore.get().dialog?.kind).toBe("delete");
  });
});

describe("キーワードの付与と絞り込み", () => {
  it("入力欄の文字列をパスにして付ける", async () => {
    expect(await addKeywordText([101], "場所 > 京都, 家族")).toBe(2);
    expect(commands("add_keywords")).toEqual([
      { type: "add_keywords", variant_ids: [101], keyword_ids: [], paths: [["場所", "京都"], ["家族"]] },
    ]);
    expect(await addKeywordText([101], " , ")).toBe(0);
    expect(commands("add_keywords")).toHaveLength(1);
  });

  it("写真を落とすと、手動のコレクションに入れる・キーワードを付ける（受け取れない先では何もしない）", async () => {
    await dropPhotos([101], { kind: "collection", id: 11, ok: true, label: "京都 2024" });
    await dropPhotos([101], { kind: "collection", id: 12, ok: false, label: "★3" });
    await dropPhotos([101, 102], { kind: "keyword", id: 2, ok: true, label: "京都" });
    expect(commands("add_to_collection")).toEqual([{ type: "add_to_collection", collection_id: 11, variant_ids: [101] }]);
    expect(commands("add_keywords")).toEqual([
      { type: "add_keywords", variant_ids: [101, 102], keyword_ids: [2], paths: [] },
    ]);
  });

  it("キーワードの絞り込みを切り替える（同じものなら外す）", () => {
    toggleKeywordFilter(2);
    expect(appStore.get().filter.keywords).toEqual([2]);
    toggleKeywordFilter(2);
    expect(appStore.get().filter.keywords).toEqual([]);
  });
});

describe("部品の描画", () => {
  const render = (el: ReturnType<typeof createElement>) => renderToString(el).replace(/<!-- -->/g, "");

  it("左のパネル・右の欄・ダイアログが例外なく描ける", () => {
    expect(render(createElement(KeywordPanel))).toContain("キーワード");
    expect(render(createElement(CollectionPanel))).toContain("コレクション");
    const editor = render(createElement(KeywordEditor, { targets: [101, 102] }));
    expect(editor).toContain("キーワードを入れて Enter");
    expect(editor).toContain("選択中の 2 枚に付けます");
    expect(render(createElement(KeywordDialog, { mode: "create", parentId: null }))).toContain("キーワードを作る");
    expect(render(createElement(CollectionDialog, { mode: "create", collectionKind: "set", parentId: null }))).toContain(
      "コレクションセットを作る",
    );
    expect(render(createElement(AddToCollectionDialog, { variantIds: [101] }))).toContain("コレクションに入れる（1 枚）");
    expect(render(createElement(SmartCollectionDialog, {}))).toContain("スマートコレクションを作る");
  });
});
