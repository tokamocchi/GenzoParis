// 通知の一覧（右下）。

import { actions, useApp } from "../state/app";

export function Toasts() {
  const toasts = useApp((s) => s.toasts);
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={`toast ${t.level}`} onClick={() => actions.dismissToast(t.id)}>
          <div>{t.message}</div>
          {t.detail && <div className="detail">{t.detail}</div>}
        </div>
      ))}
    </div>
  );
}
