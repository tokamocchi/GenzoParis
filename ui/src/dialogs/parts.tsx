// ダイアログの部品（エラーの表示・パスの表示・処理中の表示）。

import type { ReactNode } from "react";

import type { ErrorInfo } from "../api/types";

/** エラーの説明と案内（ErrorInfo）。 */
export function ErrorBox(props: { error: ErrorInfo | null; title?: string }) {
  if (!props.error) return null;
  return (
    <div className="dlg-error" role="alert">
      {props.title && <div style={{ fontWeight: 600 }}>{props.title}</div>}
      <div>{props.error.message}</div>
      {props.error.hint && <div className="hint">{props.error.hint}</div>}
    </div>
  );
}

/** パスの表示（選べる文字。なければ `empty` の文）。 */
export function PathBox(props: { path: string | null | undefined; empty?: string; box?: boolean }) {
  const cls = `dlg-path${props.box ? " box" : ""}${props.path ? "" : " empty"}`;
  return (
    <div className={cls} title={props.path ?? undefined}>
      {props.path || props.empty || "—"}
    </div>
  );
}

/** 処理中の表示（スピナーと文）。 */
export function Busy(props: { children: ReactNode }) {
  return (
    <span className="dlg-busy">
      <span className="spinner" />
      {props.children}
    </span>
  );
}
