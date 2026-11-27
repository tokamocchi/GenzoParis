// 撮影の統計のダイアログ（左のパネルの「カタログ」の「統計」）: 写真・動画を、カメラ・レンズ・焦点距離・ISO・絞り・
// 撮影年ごとに数えて、棒で見せる。対象は「表示中の条件に合うもの」か「カタログ全体」を選べる。
//
// 数えるのはコア（shooting_stats。読み取り専用の接続で、表示中の検索結果は変えない）。開いている間にカタログが
// 変わっても読み直さない（「更新」で読み直す）。

import { useCallback, useEffect, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { ErrorInfo, ShootingStats } from "../api/types";
import { Dialog } from "../components/Dialog";
import { actions, appStore } from "../state/app";
import { ErrorBox } from "./parts";
import { statSections, statsSummary } from "./statsLogic";

type Scope = "shown" | "all";

export function StatsDialog() {
  const [scope, setScope] = useState<Scope>("shown");
  const [stats, setStats] = useState<ShootingStats | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  const load = useCallback(async (which: Scope) => {
    setBusy(true);
    setError(null);
    try {
      setStats(await core.shootingStats(which === "shown" ? appStore.get().filter : {}));
    } catch (e) {
      setStats(null);
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load(scope);
  }, [load, scope]);

  const sections = stats ? statSections(stats) : [];

  return (
    <Dialog
      title="撮影の統計"
      width={640}
      onClose={actions.closeDialog}
      actions={
        <>
          <button className="btn" onClick={() => void load(scope)} disabled={busy}>
            {busy ? "数えています…" : "更新"}
          </button>
          <button className="btn primary" onClick={actions.closeDialog}>
            閉じる
          </button>
        </>
      }
    >
      <div className="dlg-row" role="radiogroup" aria-label="数える対象">
        <label>
          <input type="radio" name="stats-scope" checked={scope === "shown"} onChange={() => setScope("shown")} />
          表示中の条件に合う写真
        </label>
        <label>
          <input type="radio" name="stats-scope" checked={scope === "all"} onChange={() => setScope("all")} />
          カタログ全体
        </label>
      </div>
      {stats && <p className="dlg-stats-summary">{statsSummary(stats)}</p>}
      {!stats && busy && <p className="dlg-stats-summary">数えています…</p>}
      <div className="dlg-stats">
        {sections.map((sec) => (
          <section key={sec.key} aria-label={sec.title}>
            <h3>
              {sec.title}
              <span className="dlg-stats-hint">{sec.hint}</span>
            </h3>
            <table>
              <tbody>
                {sec.rows.map((r) => (
                  <tr key={r.label} className={r.note ? "note" : undefined}>
                    <th scope="row" title={r.label}>
                      {r.label}
                    </th>
                    <td className="bar" aria-hidden="true">
                      <span style={{ width: `${Math.max(r.fraction * 100, r.count > 0 ? 1 : 0)}%` }} />
                    </td>
                    <td className="num">{r.count.toLocaleString("ja-JP")}</td>
                    <td className="pct">{r.percent.toFixed(1)}%</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </section>
        ))}
      </div>
      {stats && sections.length > 0 && (
        <div className="dlg-note">
          折りたたんだスタックの中の写真も数えます。撮影年は、補正後の撮影日時（UTC）の年です。焦点距離は、35mm 判に換算しない
          実焦点距離です。
        </div>
      )}
      <ErrorBox error={error} title="統計を数えられません" />
    </Dialog>
  );
}
