// モーダルのダイアログ（Esc で閉じる。最初の入力にフォーカス）。

import { type ReactNode, useEffect, useRef } from "react";

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
    const el = ref.current;
    const first = el?.querySelector<HTMLElement>(
      "[autofocus], input:not([type=hidden]), select, textarea, button.primary",
    );
    first?.focus();
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
