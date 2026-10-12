import { describe, expect, it } from "vitest";

import { comboOf, isTyping, normalizeCombo } from "./keyboard";

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
  });
  it("入力欄ではショートカットを処理しない", () => {
    expect(isTyping({ tagName: "INPUT", type: "text" } as unknown as EventTarget)).toBe(true);
    expect(isTyping({ tagName: "INPUT", type: "range" } as unknown as EventTarget)).toBe(false);
    expect(isTyping({ tagName: "TEXTAREA" } as unknown as EventTarget)).toBe(true);
    expect(isTyping({ tagName: "DIV", isContentEditable: true } as unknown as EventTarget)).toBe(true);
    expect(isTyping(null)).toBe(false);
  });
});
