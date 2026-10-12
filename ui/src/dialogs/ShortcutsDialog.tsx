// ショートカットの一覧（UX-02）。

import { Dialog } from "../components/Dialog";
import { comboLabel } from "../keyboard";
import { actions } from "../state/app";
import { SHORTCUT_GROUPS } from "./shortcutsData";

export function ShortcutsDialog() {
  return (
    <Dialog
      title="キーボードのショートカット"
      width={720}
      onClose={actions.closeDialog}
      actions={
        <button className="btn primary" onClick={actions.closeDialog}>
          閉じる
        </button>
      }
    >
      <div className="shortcut-groups">
        {SHORTCUT_GROUPS.map((g) => (
          <div key={g.title} className="shortcut-group">
            <h3>{g.title}</h3>
            {g.items.map((item) => (
              <div key={`${g.title}-${item.label}`} className="shortcut-row">
                <span>{item.label}</span>
                <span className="shortcut-keys">
                  {item.keys.map((k) => (
                    <kbd key={k}>{comboLabel(k)}</kbd>
                  ))}
                </span>
              </div>
            ))}
          </div>
        ))}
      </div>
      <p className="hint">
        文字の入力中（検索・キャプションなど）と日本語の変換中は、ショートカットは働きません。ダイアログを開いている間も働きません。
      </p>
    </Dialog>
  );
}
