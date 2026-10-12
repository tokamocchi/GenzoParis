import { describe, expect, it } from "vitest";

import { comboOf, isRangeKey, isTyping, normalizeCombo } from "./keyboard";

function key(init: Partial<KeyboardEvent>): KeyboardEvent {
  return {
    key: "",
    code: "",
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...init,
  } as KeyboardEvent;
}

describe("keyboard", () => {
  it("組み合わせを正規化する", () => {
    expect(normalizeCombo("Mod+Shift+Z")).toBe("mod+shift+z");
    expect(normalizeCombo("shift+mod+z")).toBe("mod+shift+z");
    expect(normalizeCombo("ArrowRight")).toBe("ArrowRight");
  });
  it("押したキーを組み合わせにする", () => {
    expect(comboOf(key({ key: "P" }))).toBe("p");
    expect(comboOf(key({ key: "Z", ctrlKey: true, shiftKey: true }))).toBe("mod+shift+z");
    // Shift で記号になる数字は数字のまま。
    expect(comboOf(key({ key: "!", code: "Digit1", shiftKey: true }))).toBe("shift+1");
    // スペースは "Space" と書く。
    expect(comboOf(key({ key: " ", code: "Space" }))).toBe("Space");
    expect(normalizeCombo("shift+Space")).toBe("shift+Space");
  });
  it("mod に使わない側の修飾キーを押したままでは、どの割り当てにも一致しない", () => {
    // macOS 以外（テストの環境）: Windows キー・Super キー（metaKey）。
    expect(comboOf(key({ key: "z", metaKey: true }))).toBeNull();
    expect(comboOf(key({ key: "Z", metaKey: true, ctrlKey: true, shiftKey: true }))).toBeNull();
    expect(comboOf(key({ key: "ArrowRight", metaKey: true }))).toBeNull();
    // macOS: Control（Control+X を X（不採用）、Control+Z を Z（等倍）にしない）。
    expect(comboOf(key({ key: "x", ctrlKey: true }), true)).toBeNull();
    expect(comboOf(key({ key: "z", ctrlKey: true, metaKey: true }), true)).toBeNull();
    // mod の側は今までどおり。
    expect(comboOf(key({ key: "z", metaKey: true }), true)).toBe("mod+z");
    expect(comboOf(key({ key: "Z", metaKey: true, shiftKey: true }), true)).toBe("mod+shift+z");
    expect(comboOf(key({ key: "z", ctrlKey: true }), false)).toBe("mod+z");
    expect(comboOf(key({ key: "x" }), true)).toBe("x");
  });
  it("入力欄ではショートカットを処理しない", () => {
    expect(isTyping({ tagName: "INPUT", type: "text" } as unknown as EventTarget)).toBe(true);
    expect(isTyping({ tagName: "INPUT", type: "range" } as unknown as EventTarget)).toBe(false);
    expect(isTyping({ tagName: "TEXTAREA" } as unknown as EventTarget)).toBe(true);
    expect(isTyping({ tagName: "DIV", isContentEditable: true } as unknown as EventTarget)).toBe(true);
    expect(isTyping(null)).toBe(false);
  });
  it("スライダーが値を変えるキー（全体のショートカットに通さない）", () => {
    for (const k of ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"]) {
      expect(isRangeKey(k)).toBe(true);
    }
    // 評価の数字・モードの切り替えなどは全体に通す。
    for (const k of ["0", "5", "9", "p", "x", "d", "Escape", "Enter", " ", "Delete"]) {
      expect(isRangeKey(k)).toBe(false);
    }
  });
});
