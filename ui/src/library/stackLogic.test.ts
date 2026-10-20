// スタック（LIB-12）・仮想コピー（LIB-13）の画面の計算（library/stackLogic.ts）のテスト。

import { describe, expect, it } from "vitest";

import { MOCK_AUTO_STACK_PREVIEW, MOCK_STACK, MOCK_STACK_TOP, MOCK_VIRTUAL_COPY, mockSummary } from "../api/mock";
import type { StackInfo } from "../api/types";
import { activeConditionCount, clearConditions, moreConditionCount } from "./filterLogic";
import {
  MAX_COPY_NAME_CHARS,
  PHOTO_MENU_SEPARATOR,
  type PhotoMenuEntry,
  defaultDeleteKind,
  describeAutoStack,
  hiddenStackMembers,
  parseGapSeconds,
  photoMenuEntries,
  stackBadgeLabel,
  stackBadgeTitle,
  validateCopyName,
  variantBriefLabel,
} from "./stackLogic";

describe("スタックの印", () => {
  it("折りたたみは写真の数、展開は何枚目 / 写真の数", () => {
    const collapsed = { stack_id: 1, position: 1, count: 12, collapsed: true };
    expect(stackBadgeLabel(collapsed, false)).toBe("12");
    expect(stackBadgeLabel(collapsed, true)).toBe("スタック 12");
    expect(stackBadgeTitle(collapsed)).toContain("12 枚を折りたたみ中");
    const expanded = { stack_id: 1, position: 3, count: 12, collapsed: false };
    expect(stackBadgeLabel(expanded, true)).toBe("3/12");
    expect(stackBadgeTitle(expanded)).toBe("スタックの 3 枚目（12 枚）。クリックか S で折りたたむ");
    expect(stackBadgeTitle({ ...expanded, position: 1 })).toContain("一番上の写真");
  });
});

describe("仮想コピーの名前", () => {
  it("空・長すぎる・制御文字は断る（コアと同じ規則）", () => {
    expect(validateCopyName("白黒")).toBeNull();
    expect(validateCopyName("  ")).toBe("名前を入れてください");
    expect(validateCopyName("あ".repeat(MAX_COPY_NAME_CHARS))).toBeNull();
    expect(validateCopyName("あ".repeat(MAX_COPY_NAME_CHARS + 1))).toContain("200 文字まで");
    expect(validateCopyName("a\tb")).toContain("制御文字");
    // 区切りの文字（キーワードと違い）は使える。
    expect(validateCopyName("夕景 > 強め, 2")).toBeNull();
  });

  it("一覧の名前（マスターと仮想コピー）", () => {
    expect(variantBriefLabel({ variant_id: 1, is_master: true, name: null })).toBe("マスター（元の写真）");
    expect(variantBriefLabel({ variant_id: 2, is_master: false, name: "白黒" })).toBe("仮想コピー: 白黒");
    expect(variantBriefLabel({ variant_id: 3, is_master: false, name: null })).toBe("仮想コピー: （名前なし）");
  });
});

describe("自動スタック", () => {
  it("間隔は 0〜3600 秒（小数・全角も読む）", () => {
    expect(parseGapSeconds("2")).toBe(2);
    expect(parseGapSeconds("0.5")).toBe(0.5);
    expect(parseGapSeconds("０")).toBe(0);
    expect(parseGapSeconds("3600")).toBe(3600);
    expect(parseGapSeconds("3601")).toBeNull();
    expect(parseGapSeconds("")).toBeNull();
    expect(parseGapSeconds("-1")).toBeNull();
    expect(parseGapSeconds("abc")).toBeNull();
  });

  it("見込み・結果の文", () => {
    expect(describeAutoStack(MOCK_AUTO_STACK_PREVIEW)).toEqual([
      "12 個のスタックを作ります（41 枚。対象 230 枚）。",
      "すでにスタックに入っている 6 枚は対象にしません。",
      "撮影日時の分からない 1 枚は対象にしません。",
    ]);
    expect(describeAutoStack({ ...MOCK_AUTO_STACK_PREVIEW, dry_run: false })[0]).toBe(
      "12 個のスタックを作りました（41 枚）。",
    );
    expect(
      describeAutoStack({ stacks: 0, photos: 0, considered: 1500, skipped_in_stacks: 0, skipped_without_time: 0, dry_run: true }),
    ).toEqual(["この間隔でまとめられる写真はありません（対象 1,500 枚）。"]);
  });
});

describe("選択とスタック", () => {
  const expanded: StackInfo = { stack_id: 6, top_variant_id: 10, collapsed: false, members: [10, 11] };
  it("折りたたんだスタックの代表を選んでいれば、ほかの写真が隠れている", () => {
    expect(hiddenStackMembers([2], [MOCK_STACK])).toEqual([1, 3]);
    // 隠れた写真も選んでいれば除く。展開したスタックの写真は見えているので含めない。
    expect(hiddenStackMembers([2, 3, 10], [MOCK_STACK, expanded])).toEqual([1]);
    expect(hiddenStackMembers([10], [expanded])).toEqual([]);
    expect(hiddenStackMembers([99], [MOCK_STACK])).toEqual([]);
  });

  it("Delete の既定は、すべて仮想コピーのときだけ仮想コピーの削除", () => {
    const known = new Map([
      [4, MOCK_VIRTUAL_COPY],
      [5, mockSummary({ variant_id: 5, is_master: false })],
      [2, MOCK_STACK_TOP],
    ]);
    const lookup = (id: number) => known.get(id);
    expect(defaultDeleteKind([4, 5], lookup)).toBe("virtual_copies");
    expect(defaultDeleteKind([4, 2], lookup)).toBe("remove_from_catalog");
    // 読み込んでいない写真があれば、仮想コピーと決めつけない。
    expect(defaultDeleteKind([4, 99], lookup)).toBe("remove_from_catalog");
    expect(defaultDeleteKind([], lookup)).toBe("remove_from_catalog");
  });

  it("「詳しい条件」の数と、条件の解除で残す表示の指定", () => {
    expect(moreConditionCount({ masters_only: true, expand_stacks: true, long_edge_min: 3000 })).toBe(3);
    // スタックをすべて展開するのは表示の指定なので、有効な条件には数えず、解除しても残す。
    expect(activeConditionCount({ expand_stacks: true })).toBe(0);
    expect(clearConditions({ expand_stacks: true, masters_only: true, folder_id: 3 })).toEqual({
      folder_id: 3,
      include_subfolders: false,
      expand_stacks: true,
    });
  });
});

describe("写真の右クリックのメニュー", () => {
  const byAction = (entries: ReturnType<typeof photoMenuEntries>) =>
    new Map(
      entries.filter((e): e is PhotoMenuEntry => e !== PHOTO_MENU_SEPARATOR).map((e) => [e.action, e] as const),
    );

  it("仮想コピーの操作は仮想コピーのとき、スタックの操作はスタックのときだけ", () => {
    const plain = byAction(photoMenuEntries(1, mockSummary()));
    expect(plain.get("create_copy")?.enabled).toBe(true);
    expect(plain.get("rename_copy")?.enabled).toBe(false);
    expect(plain.get("group")?.enabled).toBe(false);
    expect(plain.get("add_to_stack")?.enabled).toBe(false);
    expect(byAction(photoMenuEntries(2, mockSummary())).get("add_to_stack")?.enabled).toBe(true);
    expect(plain.get("toggle_stack")?.enabled).toBe(false);
    expect(plain.get("auto_stack")?.enabled).toBe(true);
    const copy = byAction(photoMenuEntries(1, MOCK_VIRTUAL_COPY));
    expect(copy.get("rename_copy")?.enabled).toBe(true);
    expect(copy.get("make_master")?.enabled).toBe(true);
    // 展開したスタックの 2 枚目: 折りたたむ・一番上にする。
    expect(copy.get("toggle_stack")?.label).toBe("スタックを折りたたむ");
    expect(copy.get("set_top")?.enabled).toBe(true);
    const top = byAction(photoMenuEntries(3, MOCK_STACK_TOP));
    expect(top.get("toggle_stack")?.label).toBe("スタックを展開");
    expect(top.get("set_top")?.enabled).toBe(false);
    expect(top.get("group")?.enabled).toBe(true);
    expect(top.get("unstack")?.keys).toBe("mod+shift+g");
  });

  it("動画・読み込んでいない写真では仮想コピーを作れない", () => {
    expect(byAction(photoMenuEntries(1, mockSummary({ kind: "video" }))).get("create_copy")?.enabled).toBe(false);
    expect(byAction(photoMenuEntries(1, undefined)).get("create_copy")?.enabled).toBe(false);
  });

  it("位置を付ける（LIB-21）は写真を選んでいれば使える（動画 1 本だけなら使えない）", () => {
    const entry = byAction(photoMenuEntries(2, undefined)).get("edit_location");
    expect(entry).toMatchObject({ label: "位置を付ける…", enabled: true });
    expect(byAction(photoMenuEntries(1, mockSummary())).get("edit_location")?.enabled).toBe(true);
    expect(byAction(photoMenuEntries(3, mockSummary({ kind: "video" }))).get("edit_location")?.enabled).toBe(true);
    expect(byAction(photoMenuEntries(1, mockSummary({ kind: "video" }))).get("edit_location")?.enabled).toBe(false);
    expect(byAction(photoMenuEntries(0, undefined)).get("edit_location")?.enabled).toBe(false);
  });

  it("処理バージョンの更新は写真を選んでいれば使える（動画 1 本だけなら使えない）", () => {
    expect(byAction(photoMenuEntries(1, mockSummary())).get("upgrade_process")?.enabled).toBe(true);
    expect(byAction(photoMenuEntries(3, mockSummary({ kind: "video" }))).get("upgrade_process")?.enabled).toBe(true);
    expect(byAction(photoMenuEntries(1, mockSummary({ kind: "video" }))).get("upgrade_process")?.enabled).toBe(false);
    expect(byAction(photoMenuEntries(0, undefined)).get("upgrade_process")?.enabled).toBe(false);
  });

  it("比較明・比較暗合成は 2 枚以上を選んでいるときだけ（DEV-34）", () => {
    const one = byAction(photoMenuEntries(1, mockSummary()));
    expect(one.get("composite_lighten")?.enabled).toBe(false);
    expect(one.get("composite_darken")?.enabled).toBe(false);
    const two = byAction(photoMenuEntries(2, mockSummary()));
    expect(two.get("composite_lighten")?.label).toBe("比較明合成…");
    expect(two.get("composite_lighten")?.enabled).toBe(true);
    expect(two.get("composite_darken")?.enabled).toBe(true);
  });
});
