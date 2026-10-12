// 設定の「バックアップ」（SYS-04・DATA-04・DATA-05）: 今すぐバックアップ、バックアップの一覧と復元、詳細チェック。

import { useCallback, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { ErrorInfo } from "../api/types";
import { actions, useApp } from "../state/app";
import { waitJobUntilClosed } from "./catalogActions";
import { describeIntegrity, formatLocalDateTime } from "./dialogFormat";
import { Busy, ErrorBox } from "./parts";
import { RestorePanel } from "./RestorePanel";

export function SettingsBackup() {
  const catalog = useApp((s) => s.catalog);
  const [reloadToken, setReloadToken] = useState(0);
  const [backingUp, setBackingUp] = useState(false);
  const [checking, setChecking] = useState(false);
  const [integrity, setIntegrity] = useState<{ ok: boolean; lines: string[] } | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const load = useCallback(() => core.listBackups(), []);

  if (!catalog) return <p className="muted">カタログが開いていません。</p>;

  const backupNow = async () => {
    setBackingUp(true);
    setError(null);
    try {
      const b = await core.backupNow();
      actions.notify("success", `バックアップを作りました（${formatLocalDateTime(b.created_at)}）`);
      setReloadToken((n) => n + 1);
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBackingUp(false);
    }
  };

  const check = async () => {
    setChecking(true);
    setIntegrity(null);
    setError(null);
    try {
      const jobId = await core.checkIntegrity();
      actions.trackJob(jobId, "integrity_check");
      const info = await waitJobUntilClosed(jobId);
      // 終わる前にカタログを閉じた（取り消したことは閉じる処理が知らせる）。
      if (info === "closed") return;
      const r = info.result;
      if (r && r.type === "integrity_check") {
        const d = describeIntegrity(r);
        setIntegrity(d);
        if (d.ok) actions.notify("success", "カタログの詳細チェック: 問題は見つかりませんでした");
        else actions.notify("error", "カタログの詳細チェックで問題が見つかりました", "設定の「バックアップ」から復元できます");
      } else if (info.state === "cancelled") {
        actions.notify("info", "カタログの詳細チェックを取り消しました");
      } else if (info.error) {
        setError(info.error);
      }
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setChecking(false);
    }
  };

  return (
    <div>
      {catalog.startup.previous_shutdown === "unclean" && (
        <div className="dlg-warn">前回は正常に終了しませんでした。詳細チェックをおすすめします。</div>
      )}
      <div className="dlg-section">
        <h3>バックアップを作る</h3>
        <div className="dlg-row">
          <button className="btn" onClick={() => void backupNow()} disabled={backingUp}>
            今すぐバックアップ
          </button>
          {backingUp && <Busy>作っています…</Busy>}
        </div>
        <p className="hint">
          バックアップはデータのフォルダの backups に作ります。前回から 1 日以上たっていれば、カタログを開いたときと終了時にも自動で作ります（新しいものから 5 世代を残します）。
        </p>
      </div>

      <div className="dlg-section">
        <h3>詳細チェック</h3>
        <div className="dlg-row">
          <button className="btn" onClick={() => void check()} disabled={checking}>
            詳細チェックを行う
          </button>
          {checking && <Busy>確かめています（大きなカタログでは時間がかかります）…</Busy>}
        </div>
        <p className="hint">カタログのデータベースの整合性・参照・テキスト検索の索引を確かめます（カタログは変更しません）。</p>
        {integrity && (
          <div className={integrity.ok ? "dlg-note" : "dlg-error"}>
            {integrity.lines.join("\n")}
            {!integrity.ok && <div className="hint">下の一覧から、問題が起きる前のバックアップを選んで復元してください。</div>}
          </div>
        )}
      </div>

      <ErrorBox error={error} />

      <div className="dlg-section">
        <h3>バックアップから復元</h3>
        <RestorePanel catalogPath={catalog.catalog_path} load={load} reloadToken={reloadToken} />
      </div>
    </div>
  );
}
