// モーダルのダイアログ（Esc で閉じる。開いたときに最初の入力にフォーカス）。

import { type ReactNode, useEffect, useRef } from "react";

/** 開いたときにフォーカスを移す先（data-autofocus がないとき。文書の順で最初に一致するもの）。 */
const FIRST_FIELD = "[autofocus], input:not([type=hidden]), select, textarea, button.primary";

/**
 * 開いたときにフォーカスを移す先: `data-autofocus` の付いた要素、なければ文書の順で最初の入力か .primary の
 * ボタン。最初の入力より後ろの欄にフォーカスを置くときは `data-autofocus` を付ける（React の autoFocus は DOM に
 * 属性を書かず、描いたときに focus() を呼ぶだけなので `[autofocus]` では見つからず、この後の effect が最初の
 * 入力へ移し直す。位置を付けるダイアログでは、地名の検索の欄より後ろにある座標の欄に付ける）。
 */
export function initialFocusTarget(root: ParentNode | null | undefined): HTMLElement | null {
  if (!root) return null;
  return root.querySelector<HTMLElement>("[data-autofocus]") ?? root.querySelector<HTMLElement>(FIRST_FIELD);
}

export function Dialog(props: {
  title: string;
  children: ReactNode;
  actions?: ReactNode;
  onClose?: () => void;
  width?: number;
  /** 閉じられない（処理中など）。 */
  busy?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const { onClose, busy } = props;
  useEffect(() => {
    initialFocusTarget(ref.current)?.focus();
  }, []);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.isComposing && onClose && !busy) {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose, busy]);
  return (
    <div className="dialog-backdrop" onMouseDown={(e) => e.target === e.currentTarget && !busy && onClose?.()}>
      <div
        ref={ref}
        className="dialog"
        role="dialog"
        aria-modal="true"
        aria-label={props.title}
        style={props.width ? { width: props.width } : undefined}
        onKeyDown={(e) => e.stopPropagation()}
      >
        <div className="dialog-title">{props.title}</div>
        <div className="dialog-body">{props.children}</div>
        {props.actions && <div className="dialog-actions">{props.actions}</div>}
      </div>
    </div>
  );
}
