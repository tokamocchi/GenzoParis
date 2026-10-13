// 小さなメニュー（行の「…」のボタン・右クリック。キーワード・コレクションの一覧、グリッドの写真で使う）。
//
// - 外をクリックする・Esc・項目を選ぶと閉じる。画面の端からはみ出さないように位置を寄せる。
// - 項目は button（キーボードの Tab・Enter で選べる）。開いたら最初の項目にフォーカスを移す。

import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";

export interface MenuItem {
  label: string;
  onSelect: () => void;
  disabled?: boolean;
  /** チェックの印（書き出しに含める・ターゲットなど）。 */
  checked?: boolean;
  /** 危険な操作（削除）。 */
  danger?: boolean;
  /** 右に添える短い文字（ショートカットなど）。 */
  hint?: string;
}

/** 区切り線。 */
export const MENU_SEPARATOR = "separator" as const;
export type MenuEntry = MenuItem | typeof MENU_SEPARATOR;

/** 開いているメニューの位置（画面の座標）。 */
export interface MenuAt {
  x: number;
  y: number;
}

/** 位置を指定して出すメニュー（at が null なら出さない）。 */
export function PopupMenu(props: { at: MenuAt | null; items: MenuEntry[]; onClose: () => void; label?: string }) {
  const { at, onClose } = props;
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<MenuAt | null>(at);
  useLayoutEffect(() => {
    if (!at) {
      setPos(null);
      return;
    }
    const el = ref.current;
    const w = el?.offsetWidth ?? 200;
    const h = el?.offsetHeight ?? 200;
    const vw = typeof window === "undefined" ? 1e4 : window.innerWidth;
    const vh = typeof window === "undefined" ? 1e4 : window.innerHeight;
    setPos({ x: Math.max(4, Math.min(at.x, vw - w - 4)), y: Math.max(4, Math.min(at.y, vh - h - 4)) });
    el?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [at]);
  useEffect(() => {
    if (!at) return;
    const down = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("mousedown", down, true);
    window.addEventListener("keydown", key, true);
    return () => {
      window.removeEventListener("mousedown", down, true);
      window.removeEventListener("keydown", key, true);
    };
  }, [at, onClose]);
  if (!at) return null;
  return (
    <div
      ref={ref}
      className="menu"
      role="menu"
      aria-label={props.label}
      style={{ left: (pos ?? at).x, top: (pos ?? at).y }}
      onKeyDown={(e) => e.stopPropagation()}
    >
      {props.items.map((it, i) =>
        it === MENU_SEPARATOR ? (
          <div key={`sep${i}`} className="menu-sep" role="separator" />
        ) : (
          <button
            key={it.label}
            type="button"
            role={it.checked === undefined ? "menuitem" : "menuitemcheckbox"}
            aria-checked={it.checked}
            className={`menu-item${it.danger ? " danger" : ""}`}
            disabled={it.disabled}
            onClick={() => {
              onClose();
              it.onSelect();
            }}
          >
            <span className="menu-check">{it.checked ? "✓" : ""}</span>
            {it.label}
            {it.hint && <span className="menu-hint">{it.hint}</span>}
          </button>
        ),
      )}
    </div>
  );
}

/** 「…」などのボタンと、押すと出るメニュー。 */
export function MenuButton(props: { label: string; title: string; items: () => MenuEntry[]; children?: ReactNode }) {
  const [at, setAt] = useState<MenuAt | null>(null);
  return (
    <>
      <button
        type="button"
        className="btn ghost small menu-button"
        aria-label={props.title}
        title={props.title}
        aria-haspopup="menu"
        aria-expanded={at !== null}
        onClick={(e) => {
          e.stopPropagation();
          const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
          setAt(at ? null : { x: r.left, y: r.bottom + 2 });
        }}
      >
        {props.children ?? props.label}
      </button>
      {at && <PopupMenu at={at} items={props.items()} onClose={() => setAt(null)} label={props.title} />}
    </>
  );
}
