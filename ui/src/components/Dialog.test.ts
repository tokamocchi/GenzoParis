// モーダルのダイアログ（components/Dialog.tsx）の、開いたときのフォーカスの先のテスト（node で動かすので DOM は
// 使わず、querySelector だけを持つ見本で、選択子を探す順を確かめる。文書の順で最初の一致を返すのはブラウザ）。

import { describe, expect, it } from "vitest";

import { initialFocusTarget } from "./Dialog";

/** 選択子ごとに見つかる要素を決めた見本（見つからない選択子は null）。 */
function fakeRoot(found: Record<string, { id: string }>): ParentNode {
  return { querySelector: (selector: string) => found[selector] ?? null } as unknown as ParentNode;
}

const FIRST_FIELD = "[autofocus], input:not([type=hidden]), select, textarea, button.primary";

describe("ダイアログを開いたときのフォーカスの先", () => {
  it("data-autofocus の付いた欄を、文書の順で先にある入力より優先する（位置を付けるダイアログの座標の欄）", () => {
    const coord = { id: "geo-coord" };
    const search = { id: "search" };
    const root = fakeRoot({ "[data-autofocus]": coord, [FIRST_FIELD]: search });
    expect(initialFocusTarget(root)).toBe(coord);
  });

  it("data-autofocus がなければ、今までどおり最初の入力か .primary のボタン", () => {
    const first = { id: "first" };
    expect(initialFocusTarget(fakeRoot({ [FIRST_FIELD]: first }))).toBe(first);
    expect(initialFocusTarget(fakeRoot({}))).toBeNull();
    expect(initialFocusTarget(null)).toBeNull();
  });
});
