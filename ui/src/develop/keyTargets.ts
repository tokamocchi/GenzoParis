// キーを受けた要素の判定（DOM の型に頼らない。node の vitest でも試せるように）。

/** 判定に使う要素の性質（HTMLElement の一部）。 */
interface ElementLike {
  tagName?: unknown;
  type?: unknown;
  getAttribute?: (name: string) => string | null;
}

/** Enter を押すと既定の動作をする input の種類（押す・選ぶ・送る）。 */
const ACTIVATING_INPUT_TYPES = new Set(["button", "submit", "reset", "checkbox", "radio", "image", "file"]);

/** Enter を押すと既定の動作をする役割（WAI-ARIA の role）。 */
const ACTIVATING_ROLES = new Set([
  "button",
  "link",
  "tab",
  "menuitem",
  "menuitemcheckbox",
  "menuitemradio",
  "option",
  "checkbox",
  "radio",
  "switch",
  "treeitem",
]);

/**
 * フォーカスのある要素が、Enter で自分の動作をする要素か（ボタン・リンク・チェックボックス・タブなど）。
 *
 * 切り抜きの編集中の Enter（完了）を window の capture で受けるとき、これらの要素では Enter を横取り
 * しない（パネルの「取り消し」などのボタンで Enter を押したのに、切り抜きを保存してしまわないように）。
 */
export function activatesOnEnter(target: EventTarget | null): boolean {
  const el = target as ElementLike | null;
  if (!el || typeof el.tagName !== "string") return false;
  const tag = el.tagName.toLowerCase();
  if (tag === "button" || tag === "a" || tag === "summary") return true;
  if (tag === "input") return typeof el.type === "string" && ACTIVATING_INPUT_TYPES.has(el.type.toLowerCase());
  const role = typeof el.getAttribute === "function" ? el.getAttribute("role") : null;
  return role !== null && ACTIVATING_ROLES.has(role.toLowerCase());
}
