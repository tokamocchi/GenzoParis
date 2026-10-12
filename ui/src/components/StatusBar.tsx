// 下の帯（件数・選択・検索中）。

import { useApp } from "../state/app";

export function StatusBar() {
  const search = useApp((s) => s.search);
  const searching = useApp((s) => s.searching);
  const selected = useApp((s) => s.selection.ids.length);
  const index = useApp((s) => s.selection.currentIndex);
  const mode = useApp((s) => s.mode);
  return (
    <footer className="statusbar">
      <span>{search ? `${search.count.toLocaleString()} 件` : "—"}</span>
      {mode === "grid" ? (
        selected > 1 && <span>{selected.toLocaleString()} 件を選択</span>
      ) : (
        index !== null && search && <span>{(index + 1).toLocaleString()} / {search.count.toLocaleString()}</span>
      )}
      {searching && <span className="muted">検索中…</span>}
    </footer>
  );
}
