// 設定の「表示」: ルーペ上の撮影情報（LIB-17）の項目（情報 1・情報 2）。I で「情報 1 → 情報 2 → なし」と切り替える。
//
// この PC のブラウザーの保存領域に覚える（viewer/infoItems.ts。カタログには保存しない）。

import { INFO_DEFAULTS, INFO_ITEMS, type InfoSets, setInfoSets, toggleInfoItem, useInfo } from "../viewer/infoItems";
import { comboLabel } from "../keyboard";

export function SettingsView() {
  const sets = useInfo((s) => s.sets);
  const set = (which: keyof InfoSets, item: (typeof INFO_ITEMS)[number]["item"], on: boolean) =>
    setInfoSets({ ...sets, [which]: toggleInfoItem(sets[which], item, on) });
  return (
    <div className="dlg-section">
      <h3>ルーペ上の撮影情報</h3>
      <p className="muted">
        ルーペと現像の画面で {comboLabel("i")} を押すと「情報 1 → 情報 2 → なし」の順に切り替わります。それぞれに出す項目を選んでください。
      </p>
      <table className="info-items">
        <thead>
          <tr>
            <th>項目</th>
            <th>情報 1</th>
            <th>情報 2</th>
          </tr>
        </thead>
        <tbody>
          {INFO_ITEMS.map((d) => (
            <tr key={d.item}>
              <td>{d.label}</td>
              {(["info1", "info2"] as const).map((which) => (
                <td key={which}>
                  <input
                    type="checkbox"
                    aria-label={`${which === "info1" ? "情報 1" : "情報 2"}: ${d.label}`}
                    checked={sets[which].includes(d.item)}
                    onChange={(e) => set(which, d.item, e.target.checked)}
                  />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      <div className="dlg-row end">
        <button type="button" className="btn small" onClick={() => setInfoSets(INFO_DEFAULTS)}>
          既定に戻す
        </button>
      </div>
    </div>
  );
}
