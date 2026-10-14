// キーワード（LIB-09）・コレクション（LIB-10）・スマートコレクション（LIB-11）の画面の計算のテスト。

import { describe, expect, it } from "vitest";

import { MOCK_COLLECTIONS, MOCK_KEYWORD_USAGE, MOCK_KEYWORDS, MOCK_SMART_RULE, MOCK_VARIANT_KEYWORDS } from "../api/mock";
import type { CollectionInfo, KeywordInfo, SmartRule } from "../api/types";
import {
  buildCollectionTree,
  collectionDescendants,
  collectionPathText,
  countText,
  deleteCollectionMessage,
  manualCollections,
  parentChoices,
  referencedCollections,
  validateCollectionName,
  visibleCollectionRows,
} from "./collectionLogic";
import {
  applyCompletion,
  buildKeywordTree,
  completionQuery,
  deleteKeywordMessage,
  descendantIds,
  keywordPath,
  keywordPathText,
  moveTargets,
  parseKeywordInput,
  sortedVariantKeywords,
  tagState,
  validateKeywordName,
  visibleKeywordRows,
} from "./keywordLogic";
import { dragIds, exceedsThreshold, parseDropTarget } from "./photoDrag";
import {
  appendTo,
  conditionAt,
  defaultCondition,
  describeRule,
  emptyRule,
  MAX_SMART_CONDITIONS,
  MAX_SMART_DEPTH,
  removeAt,
  replaceAt,
  ruleProblems,
  ruleSize,
  setMatch,
  withOp,
} from "./smartRuleLogic";

describe("キーワードの木（LIB-09）", () => {
  it("parent_id で木にし、開いた点の子だけを出す", () => {
    const tree = buildKeywordTree(MOCK_KEYWORDS);
    expect(tree.map((n) => n.info.name)).toEqual(["場所", "人物"]);
    expect(tree[0].children.map((n) => n.info.name)).toEqual(["京都", "東京"]);
    const closed = visibleKeywordRows(tree, () => false);
    expect(closed.map((r) => r.node.info.name)).toEqual(["場所", "人物"]);
    expect(closed[0].hasChildren).toBe(true);
    const open = visibleKeywordRows(tree, () => true);
    expect(open.map((r) => `${r.depth}:${r.node.info.name}`)).toEqual([
      "0:場所",
      "1:京都",
      "2:祇園",
      "1:東京",
      "0:人物",
      "1:家族",
    ]);
  });

  it("名前で絞り込むと、一致するものと上の階層を開いて出す（全角・半角を区別しない）", () => {
    const tree = buildKeywordTree([...MOCK_KEYWORDS, { keyword_id: 7, parent_id: 4, name: "Ｓｈｉｂｕｙａ", include_on_export: true, photo_count: 1 }]);
    const rows = visibleKeywordRows(tree, () => false, "祇");
    expect(rows.map((r) => r.node.info.name)).toEqual(["場所", "京都", "祇園"]);
    expect(visibleKeywordRows(tree, () => false, "shibuya").map((r) => r.node.info.name)).toEqual(["場所", "東京", "Ｓｈｉｂｕｙａ"]);
    expect(visibleKeywordRows(tree, () => false, "なし")).toEqual([]);
  });

  it("親が循環した壊れたデータでも止まり、最上位に出す", () => {
    const broken: KeywordInfo[] = [
      { keyword_id: 1, parent_id: 2, name: "a", include_on_export: true, photo_count: 0 },
      { keyword_id: 2, parent_id: 1, name: "b", include_on_export: true, photo_count: 0 },
    ];
    const tree = buildKeywordTree(broken);
    expect(tree.length).toBeGreaterThan(0);
    expect(keywordPath(broken, 1).length).toBeLessThanOrEqual(1000);
  });

  it("パス・子孫・移動の先（自分と子孫を除く）", () => {
    expect(keywordPath(MOCK_KEYWORDS, 3)).toEqual(["場所", "京都", "祇園"]);
    expect(keywordPathText(["場所", "京都"])).toBe("場所 > 京都");
    expect([...descendantIds(MOCK_KEYWORDS, 1)].sort()).toEqual([1, 2, 3, 4]);
    const targets = moveTargets(MOCK_KEYWORDS, 2);
    expect(targets[0]).toEqual({ id: null, label: "（最上位）" });
    const labels = targets.map((t) => t.label);
    expect(labels).toContain("場所");
    expect(labels).toContain("人物 > 家族");
    expect(labels).not.toContain("場所 > 京都");
    expect(labels).not.toContain("場所 > 京都 > 祇園");
  });

  it("入力欄の書き方（「,」「、」で複数、「>」「＞」「|」で階層。空と重複は捨てる）", () => {
    expect(parseKeywordInput("場所 > 京都, 家族")).toEqual([["場所", "京都"], ["家族"]]);
    expect(parseKeywordInput("場所＞京都、場所 > 京都 ,, ｆｏｏ|bar ")).toEqual([["場所", "京都"], ["ｆｏｏ", "bar"]]);
    expect(parseKeywordInput(" > , 、")).toEqual([]);
    expect(parseKeywordInput("a >> b")).toEqual([["a", "b"]]);
    // 大文字・小文字、全角・半角の違いは同じパス。
    expect(parseKeywordInput("Tokyo, ＴＯＫＹＯ")).toEqual([["Tokyo"]]);
  });

  it("補完: 最後の項目の最後の段で探し、選んだらパスに置き換える", () => {
    expect(completionQuery("家族, 場所 > 京")).toBe("京");
    expect(completionQuery("")).toBe("");
    expect(applyCompletion("家族, 場所 > 京", ["場所", "京都"])).toBe("家族, 場所 > 京都, ");
    expect(applyCompletion("京", ["場所", "京都"])).toBe("場所 > 京都, ");
  });

  it("選択中の写真への付き方と並び（すべてに付いているものが先）", () => {
    expect(tagState(MOCK_VARIANT_KEYWORDS, 2)).toBe("all");
    expect(tagState(MOCK_VARIANT_KEYWORDS, 6)).toBe("some");
    expect(tagState(MOCK_VARIANT_KEYWORDS, 3)).toBe("none");
    expect(tagState(null, 2)).toBe("none");
    expect(sortedVariantKeywords(MOCK_VARIANT_KEYWORDS).map((k) => k.name)).toEqual(["京都", "家族"]);
    // どちらもすべてに付いていれば、パスの文字列の順（「人物 > 家族」<「場所 > 京都」）。
    const both = { variants: 2, keywords: MOCK_VARIANT_KEYWORDS.keywords.map((k) => ({ ...k, count: 2 })) };
    expect(sortedVariantKeywords(both).map((k) => k.name)).toEqual(["家族", "京都"]);
  });

  it("削除の確認の文（下の階層・写真の数・スマートコレクション）", () => {
    const lines = deleteKeywordMessage("場所", MOCK_KEYWORD_USAGE);
    expect(lines[0]).toContain("写真や元のファイルは削除しません");
    expect(lines.join("\n")).toContain("下の階層のキーワード 3 個");
    expect(lines.join("\n")).toContain("23 枚");
    expect(lines.join("\n")).toContain("「★3 以上の場所」");
    expect(deleteKeywordMessage("x", { photos_with_descendants: 0, descendants: 0, smart_collections: [] })).toHaveLength(1);
    // 「含まない」の条件は削除の後にすべての写真に一致する（コアの動作。検証の指摘）。
    expect(lines.join("\n")).toContain("「含まない」はすべての写真に一致");
    expect(lines.join("\n")).not.toContain("その条件はどの写真にも一致しなくなります");
  });

  it("名前の検証（区切りの文字・制御文字・長さ。コアと同じ規則）", () => {
    expect(validateKeywordName("京都")).toBeNull();
    expect(validateKeywordName("  ")).not.toBeNull();
    for (const bad of ["a>b", "a＞b", "a|b", "a,b", "a，b", "a、b"]) {
      expect(validateKeywordName(bad)).toContain("区切り");
    }
    expect(validateKeywordName("a\nb")).toContain("制御文字");
    expect(validateKeywordName("a\u0085b")).toContain("制御文字");
    expect(validateKeywordName("あ".repeat(200))).toBeNull();
    expect(validateKeywordName("あ".repeat(201))).toContain("200 文字");
    // 文字数はコードポイントで数える（絵文字 1 つは 1 文字。コアの chars().count() と同じ）。
    expect(validateKeywordName("😀".repeat(200))).toBeNull();
  });
});

describe("コレクションの木（LIB-10）", () => {
  it("セットの中に入れ子にし、セットを先に並べる", () => {
    const tree = buildCollectionTree(MOCK_COLLECTIONS);
    expect(tree.map((n) => n.info.name)).toEqual(["旅行", "お気に入り"]);
    expect(tree[0].children.map((n) => n.info.name)).toEqual(["京都 2024", "★3 以上の場所"]);
    const rows = visibleCollectionRows(tree, () => true);
    expect(rows.map((r) => `${r.depth}:${r.node.info.name}`)).toEqual(["0:旅行", "1:京都 2024", "1:★3 以上の場所", "0:お気に入り"]);
    expect(visibleCollectionRows(tree, () => false)).toHaveLength(2);
  });

  it("手動のコレクションの下にある行（壊れたデータ）は最上位に出す", () => {
    const odd: CollectionInfo[] = [
      ...MOCK_COLLECTIONS,
      { collection_id: 20, parent_id: 13, name: "迷子", kind: "manual", rule: null, rule_error: null, count: 0 },
    ];
    expect(buildCollectionTree(odd).map((n) => n.info.name)).toContain("迷子");
  });

  it("パス・中のもの・親の候補（セットだけ。自分と中のセットを除く）・写真を入れられるもの", () => {
    expect(collectionPathText(MOCK_COLLECTIONS, 11)).toBe("旅行 > 京都 2024");
    expect([...collectionDescendants(MOCK_COLLECTIONS, 10)].sort()).toEqual([10, 11, 12]);
    expect(parentChoices(MOCK_COLLECTIONS).map((c) => c.label)).toEqual(["（最上位）", "旅行"]);
    expect(parentChoices(MOCK_COLLECTIONS, 10).map((c) => c.label)).toEqual(["（最上位）"]);
    expect(manualCollections(MOCK_COLLECTIONS).map((c) => c.label)).toEqual(["お気に入り", "旅行 > 京都 2024"]);
  });

  it("数の表示・名前の検証・削除の確認の文", () => {
    expect(countText(MOCK_COLLECTIONS[1])).toBe("25");
    expect(countText({ ...MOCK_COLLECTIONS[2], count: null })).toBe("");
    expect(countText({ ...MOCK_COLLECTIONS[2], rule_error: "新しい版" })).toBe("!");
    expect(validateCollectionName("  ")).not.toBeNull();
    expect(validateCollectionName("京都")).toBeNull();
    const lines = deleteCollectionMessage(MOCK_COLLECTIONS, 10).join("\n");
    expect(lines).toContain("写真や元のファイルは削除しません");
    expect(lines).toContain("中のコレクション 2 個");
    const refs: CollectionInfo[] = [
      ...MOCK_COLLECTIONS,
      {
        collection_id: 14,
        parent_id: null,
        name: "参照",
        kind: "smart",
        rule: { version: 1, match: "all", rules: [{ type: "collection", op: "in", collection_id: 13 }] },
        rule_error: null,
        count: null,
      },
    ];
    expect(deleteCollectionMessage(refs, 13).join("\n")).toContain("「参照」");
    expect(deleteCollectionMessage(refs, 13).join("\n")).toContain("「含まれない」はすべての写真に一致");
    expect(validateCollectionName("😀".repeat(200))).toBeNull();
    expect(validateCollectionName("あ".repeat(201))).toContain("200 文字");
    expect(validateCollectionName("a\tb")).toContain("制御文字");
  });

  it("削除の確認の参照の判定は ID の前方一致で誤らない（1 と 12・10〜19・100）", () => {
    const smart = (id: number, rule: SmartRule): CollectionInfo => ({
      collection_id: id,
      parent_id: null,
      name: `S${id}`,
      kind: "smart",
      rule,
      rule_error: null,
      count: null,
    });
    const manual = (id: number, parent: number | null = null): CollectionInfo => ({
      collection_id: id,
      parent_id: parent,
      name: `M${id}`,
      kind: "manual",
      rule: null,
      rule_error: null,
      count: 0,
    });
    const refTo = (id: number): SmartRule => ({
      version: 1,
      match: "all",
      rules: [{ type: "group", match: "any", rules: [{ type: "collection", op: "not_in", collection_id: id }] }],
    });
    const list = [manual(1), manual(12), manual(100), smart(200, refTo(12)), smart(201, refTo(100)), smart(202, refTo(1))];
    const one = deleteCollectionMessage(list, 1).join("\n");
    expect(one).toContain("「S202」");
    expect(one).not.toContain("「S200」");
    expect(one).not.toContain("「S201」");
    expect(deleteCollectionMessage(list, 12).join("\n")).not.toContain("「S202」");
    expect(referencedCollections(refTo(12))).toEqual([12]);
    expect(referencedCollections(null)).toEqual([]);
    // セットを削除すると中のコレクションも消えるので、中のものを参照する条件も挙げる。
    const set: CollectionInfo = { ...manual(50), kind: "set" };
    const nested = [set, manual(51, 50), smart(300, refTo(51))];
    expect(deleteCollectionMessage(nested, 50).join("\n")).toContain("「S300」");
  });
});

describe("スマートコレクションの条件の編集（LIB-11）", () => {
  it("木の操作（パスで指して、置き換え・削除・足す・すべて / いずれか）", () => {
    let r: SmartRule = emptyRule();
    expect(r.rules).toHaveLength(1);
    r = appendTo(r, [], defaultCondition("group"));
    expect(conditionAt(r, [1])?.type).toBe("group");
    r = appendTo(r, [1], defaultCondition("kind"));
    expect(conditionAt(r, [1, 1])).toEqual({ type: "kind", value: "photo" });
    r = replaceAt(r, [1, 1], { type: "kind", value: "video" });
    expect(conditionAt(r, [1, 1])).toEqual({ type: "kind", value: "video" });
    r = setMatch(r, [1], "all");
    expect((conditionAt(r, [1]) as { match: string }).match).toBe("all");
    r = setMatch(r, [], "any");
    expect(r.match).toBe("any");
    r = removeAt(r, [1, 0]);
    expect((conditionAt(r, [1]) as { rules: unknown[] }).rules).toHaveLength(1);
    expect(ruleSize(r)).toEqual({ count: 3, depth: 2 });
    // 元のオブジェクトは変えない。
    const before = JSON.stringify(emptyRule());
    const base = emptyRule();
    replaceAt(base, [0], { type: "kind", value: "video" });
    expect(JSON.stringify(base)).toBe(before);
  });

  it("演算子を変えると、要らない値を消し、要る値を入れる", () => {
    expect(withOp({ type: "keyword", op: "any" }, "has")).toEqual({ type: "keyword", op: "has", keyword_id: 0 });
    expect(withOp({ type: "keyword", op: "has", keyword_id: 5 }, "none")).toEqual({ type: "keyword", op: "none" });
    expect(withOp({ type: "capture_time", op: "in_last_days", days: 7 }, "unknown")).toEqual({ type: "capture_time", op: "unknown" });
    expect(withOp({ type: "capture_time", op: "unknown" }, "in_last_days")).toEqual({ type: "capture_time", op: "in_last_days", days: 30 });
    expect(withOp({ type: "rating", op: "gte", value: 3 }, "eq")).toEqual({ type: "rating", op: "eq", value: 3 });
  });

  it("検証（コアの SmartRule::validate と同じ決まり）", () => {
    expect(ruleProblems(MOCK_SMART_RULE)).toEqual([]);
    const one = (c: SmartRule["rules"][number]) => ruleProblems({ version: 1, match: "all", rules: [c] });
    expect(one({ type: "keyword", op: "has", keyword_id: 0 })).toHaveLength(1);
    expect(one({ type: "keyword", op: "any" })).toEqual([]);
    expect(one({ type: "capture_time", op: "range" })).toHaveLength(1);
    expect(one({ type: "capture_time", op: "range", from: "2025-01-01T00:00:00Z", until: "2024-01-01T00:00:00Z" })).toHaveLength(1);
    expect(one({ type: "capture_time", op: "range", from: "2024-01-01T00:00:00Z" })).toEqual([]);
    expect(one({ type: "capture_time", op: "in_last_days", days: 0 })).toHaveLength(1);
    expect(one({ type: "capture_time", op: "in_last_days", days: 36_501 })).toHaveLength(1);
    expect(one({ type: "camera", op: "is", value: " " })).toHaveLength(1);
    expect(one({ type: "camera", op: "unknown", value: "" })).toEqual([]);
    expect(one({ type: "text", op: "contains", value: "あ".repeat(201) })).toHaveLength(1);
    expect(one({ type: "folder", op: "in", folder_id: 0, include_subfolders: true })).toHaveLength(1);
    expect(one({ type: "collection", op: "in", collection_id: 0 })).toHaveLength(1);
    expect(ruleProblems({ version: 2, match: "all", rules: [] })).toHaveLength(1);
    // 深さと数の上限。
    let deep: SmartRule["rules"][number] = { type: "kind", value: "photo" };
    for (let i = 0; i < MAX_SMART_DEPTH; i++) deep = { type: "group", match: "all", rules: [deep] };
    expect(ruleProblems({ version: 1, match: "all", rules: [deep] }).join()).toContain(`${MAX_SMART_DEPTH} 段`);
    const many = Array.from({ length: MAX_SMART_CONDITIONS + 1 }, () => ({ type: "kind", value: "photo" }) as const);
    expect(ruleProblems({ version: 1, match: "any", rules: [...many] }).join()).toContain(`${MAX_SMART_CONDITIONS} 個`);
  });

  it("説明の文（一覧のツールチップ）", () => {
    const names = {
      keyword: (id: number) => (id === 1 ? "場所" : undefined),
      folder: () => undefined,
      collection: () => undefined,
    };
    expect(describeRule(MOCK_SMART_RULE, names)).toBe("評価が次の値以上 ★3 かつ キーワードに次を含む（下の階層も） 場所");
    expect(describeRule({ version: 1, match: "any", rules: [] }, names)).toBe("どの写真も一致しない");
    expect(
      describeRule(
        {
          version: 1,
          match: "any",
          rules: [
            { type: "keyword", op: "has", keyword_id: 9 },
            { type: "capture_time", op: "range", from: "2023-12-31T15:00:00Z", until: "2024-12-31T15:00:00Z" },
          ],
        },
        names,
        540,
      ),
    ).toBe("キーワードに次を含む（下の階層も） （削除済み） または 撮影日が次の期間 2024-01-01〜2024-12-31");
  });
});

describe("写真のドラッグ（LIB-09・LIB-10）", () => {
  it("動かした距離・運ぶ写真・落とす先の読み取り", () => {
    expect(exceedsThreshold(3, 4)).toBe(false);
    expect(exceedsThreshold(6, 1)).toBe(true);
    expect(dragIds([1, 2, 3], 2)).toEqual([1, 2, 3]);
    expect(dragIds([1, 2, 3], 9)).toEqual([9]);
    expect(parseDropTarget({ dropKind: "collection", dropId: "11", dropOk: "1", dropLabel: "京都" })).toEqual({
      kind: "collection",
      id: 11,
      ok: true,
      label: "京都",
    });
    expect(parseDropTarget({ dropKind: "collection", dropId: "12", dropOk: "0" })?.ok).toBe(false);
    expect(parseDropTarget({ dropKind: "keyword", dropId: "2", dropOk: "1", dropLabel: "京都" })?.kind).toBe("keyword");
    // フォルダ（FILE-02）も受け取る先。知らない種類は受け取らない。
    expect(parseDropTarget({ dropKind: "folder", dropId: "2" })).toEqual({ kind: "folder", id: 2, ok: false, label: "" });
    expect(parseDropTarget({ dropKind: "stack", dropId: "2" })).toBeNull();
    expect(parseDropTarget({ dropKind: "keyword", dropId: "x" })).toBeNull();
  });
});
