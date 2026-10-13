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
      {search && search.hidden > 0 && (
        <span className="muted" title="条件に合う写真のうち、折りたたんだスタックの中にあるもの（S で展開）">
          ほかに折りたたんだスタックの中に {search.hidden.toLocaleString()} 枚
        </span>
      )}
      {mode === "grid" ? (
        selected > 1 && <span>{selected.toLocaleString()} 件を選択</span>
      ) : (
        index !== null && search && <span>{(index + 1).toLocaleString()} / {search.count.toLocaleString()}</span>
      )}
      {searching && <span className="muted">検索中…</span>}
    </footer>
  );
}
