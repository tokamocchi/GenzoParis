// 上の帯（カタログ名・表示モード・取り込み・書き出し・ジョブ・設定）。

import { JobsIndicator } from "../dialogs/JobsIndicator";
import { comboLabel } from "../keyboard";
import { actions, useApp } from "../state/app";

function baseName(p: string): string {
  return p.split(/[\\/]/).filter(Boolean).pop() ?? p;
}

export function TopBar() {
  const mode = useApp((s) => s.mode);
  const catalog = useApp((s) => s.catalog);
  const autoAdvance = useApp((s) => s.autoAdvance);
  const hasTargets = useApp((s) => s.selection.ids.length > 0 || s.selection.current !== null);
  return (
    <header className="topbar">
      <span className="brand">GenzoParis</span>
      {catalog && (
        <span className="catalog-name" title={catalog.catalog_path}>
          {baseName(catalog.catalog_path)}
        </span>
      )}
      <nav className="mode-tabs" aria-label="表示">
        <button className={mode === "grid" ? "on" : ""} onClick={() => actions.setMode("grid")} title={`グリッド（${comboLabel("g")}）`}>
          グリッド
        </button>
        <button className={mode === "loupe" ? "on" : ""} onClick={() => actions.setMode("loupe")} title={`ルーペ（${comboLabel("e")}）`}>
          ルーペ
        </button>
        <button className={mode === "develop" ? "on" : ""} onClick={() => actions.setMode("develop")} title={`現像（${comboLabel("d")}）`}>
          現像
        </button>
      </nav>
      <span className="spacer" />
      <label className="btn ghost small" title="評価・フラグを付けたら次の写真へ進む">
        <input type="checkbox" checked={autoAdvance} onChange={(e) => actions.setAutoAdvance(e.target.checked)} />
        自動で次へ
      </label>
      <JobsIndicator />
      <button className="btn" onClick={() => actions.openDialog({ kind: "import" })} title={`フォルダを取り込む（${comboLabel("mod+shift+i")}）`}>
        取り込み
      </button>
      <button
        className="btn"
        disabled={!hasTargets}
        onClick={() => actions.openDialog({ kind: "export", variantIds: actions.targets() })}
        title={`選んだ写真を書き出す（${comboLabel("mod+shift+e")}）`}
      >
        書き出し
      </button>
      <button className="btn ghost icon" onClick={() => actions.openDialog({ kind: "settings" })} title={`設定（${comboLabel("mod+,")}）`} aria-label="設定">
        ⚙
      </button>
    </header>
  );
}
