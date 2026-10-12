// キーボードのショートカット（UX-02・LIB-05）。
//
// - 入力欄（input・textarea・select・contenteditable）にフォーカスがあるときと、IME の変換中は
//   ショートカットを処理しない（日本語の入力を妨げない。UX-01）。
// - キーの書き方: "g"、"shift+g"、"mod+z"（mod は macOS で ⌘、それ以外で Ctrl）、"ArrowRight"、"Delete"。

import { useEffect, useRef } from "react";

export type Bindings = Record<string, (e: KeyboardEvent) => void>;

const IS_MAC = typeof navigator !== "undefined" && /Mac|iPhone|iPad/i.test(navigator.platform || navigator.userAgent);

/** 入力欄にフォーカスがあるか。 */
export function isTyping(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || !el.tagName) return false;
  const tag = el.tagName.toLowerCase();
  if (tag === "textarea" || tag === "select") return true;
  if (tag === "input") {
    const type = (el as HTMLInputElement).type;
    return !["checkbox", "radio", "range", "button", "submit", "color"].includes(type);
  }
  return el.isContentEditable;
}

/** キーの組み合わせの文字列（"shift+mod+z" の順は mod・alt・shift・キー）。 */
export function comboOf(e: KeyboardEvent): string {
  const parts: string[] = [];
  const mod = IS_MAC ? e.metaKey : e.ctrlKey;
  if (mod) parts.push("mod");
  if (e.altKey) parts.push("alt");
  if (e.shiftKey) parts.push("shift");
  let key = e.key;
  if (key.length === 1) key = key.toLowerCase();
  // Shift で記号が変わるキー（数字）は、物理的なキーの数字で表す。
  if (e.shiftKey && /^Digit[0-9]$/.test(e.code)) key = e.code.slice(5);
  parts.push(key);
  return parts.join("+");
}

/** 書き方を正規化する（"Mod+Shift+Z" → "mod+shift+z"）。 */
export function normalizeCombo(s: string): string {
  const parts = s.split("+").map((p) => p.trim());
  const key = parts.pop() ?? "";
  const mods = new Set(parts.map((p) => p.toLowerCase()));
  const out: string[] = [];
  if (mods.has("mod")) out.push("mod");
  if (mods.has("alt")) out.push("alt");
  if (mods.has("shift")) out.push("shift");
  out.push(key.length === 1 ? key.toLowerCase() : key);
  return out.join("+");
}

/** ショートカットを登録する（`enabled` が偽の間は無効）。 */
export function useHotkeys(bindings: Bindings, enabled = true): void {
  const ref = useRef(bindings);
  ref.current = bindings;
  useEffect(() => {
    if (!enabled) return;
    const handler = (e: KeyboardEvent) => {
      if (e.isComposing || e.keyCode === 229) return;
      if (e.defaultPrevented) return;
      if (isTyping(e.target)) return;
      const combo = comboOf(e);
      for (const [k, fn] of Object.entries(ref.current)) {
        if (normalizeCombo(k) === combo) {
          e.preventDefault();
          fn(e);
          return;
        }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [enabled]);
}

/** ショートカットの表示用の文字（mod は ⌘ / Ctrl）。 */
export function comboLabel(s: string): string {
  return normalizeCombo(s)
    .split("+")
    .map((p) => {
      if (p === "mod") return IS_MAC ? "⌘" : "Ctrl";
      if (p === "shift") return IS_MAC ? "⇧" : "Shift";
      if (p === "alt") return IS_MAC ? "⌥" : "Alt";
      if (p === "ArrowLeft") return "←";
      if (p === "ArrowRight") return "→";
      if (p === "ArrowUp") return "↑";
      if (p === "ArrowDown") return "↓";
      return p.length === 1 ? p.toUpperCase() : p;
    })
    .join(IS_MAC ? "" : "+");
}
