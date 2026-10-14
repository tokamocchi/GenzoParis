// 設定の「一般」の「ログ・クラッシュレポート」（SYS-07）: ログのフォルダの場所と中身、フォルダを開く、ログを消す。
//
// ログとクラッシュレポートは、この PC のアプリのデータのフォルダにだけ保存する（外部へは送信しない。02 の
// SEC-02）。写真のファイル名・フォルダの場所が含まれるので、利用者が消せるようにする（確かめてから消す）。

import { useCallback, useEffect, useState } from "react";

import * as app from "../api/app";
import { toApiError } from "../api/errors";
import type { ErrorInfo, LogFolderInfo } from "../api/types";
import { actions, useApp } from "../state/app";
import { describeClearReport, describeCrashReport, describeLogFolder, LOCAL_ONLY_NOTE } from "./logsLogic";
import { Busy, ErrorBox, PathBox } from "./parts";

export function SettingsLogs() {
  const logs = useApp((s) => s.status?.logs ?? null);
  const [info, setInfo] = useState<LogFolderInfo | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const dir = logs?.dir ?? null;

  const reload = useCallback(async () => {
    if (!dir) return;
    try {
      setInfo(await app.logFolderInfo());
    } catch (e) {
      setError(toApiError(e).info);
    }
  }, [dir]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const open = async () => {
    setError(null);
    try {
      await app.openLogFolder();
    } catch (e) {
      setError(toApiError(e).info);
    }
  };

  const clear = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await app.clearLogs();
      const n = describeClearReport(r);
      actions.notify(n.level, n.message, n.detail);
      setConfirming(false);
      await reload();
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="dlg-section">
      <h3>ログ・クラッシュレポート</h3>
      <p className="hint" style={{ marginTop: 0 }}>
        不具合を調べるために、動作の記録（ログ）と、異常終了したときの報告（クラッシュレポート）を残します。{LOCAL_ONLY_NOTE}
      </p>
      <div className="dlg-row">
        <div className="dlg-grow">
          <PathBox path={dir} empty="ログのフォルダが分かりません（ログはファイルに残していません）" box />
        </div>
        <button className="btn" onClick={() => void open()} disabled={!dir || busy}>
          ログのフォルダを開く
        </button>
        <button className="btn" onClick={() => setConfirming(true)} disabled={!dir || busy || confirming}>
          ログを消す…
        </button>
      </div>
      {info && <div className="hint">{describeLogFolder(info)}</div>}
      {info && info.crash_reports.length > 0 && (
        <ul className="hint" style={{ paddingLeft: 18, margin: "4px 0" }}>
          {info.crash_reports.slice(0, 5).map((r) => (
            <li key={r.file_name}>{describeCrashReport(r)}</li>
          ))}
        </ul>
      )}
      {logs && !logs.file_enabled && (
        <div className="dlg-note">ログをファイルに残さない設定で起動しています（環境変数 GENZO_LOG_FILE=off）。</div>
      )}
      {logs && logs.problems.length > 0 && (
        <div className="dlg-note">
          {logs.problems.map((p) => (
            <div key={p}>{p}</div>
          ))}
        </div>
      )}
      {confirming && (
        <div className="dlg-confirm">
          <p>ログのフォルダのログとクラッシュレポートを消します。</p>
          <ul className="hint" style={{ paddingLeft: 18, margin: "4px 0 8px" }}>
            <li>今書いているログは空にします（これからの動作は、また記録します）。</li>
            <li>写真・カタログ・設定は変わりません。消したログは戻せません。</li>
          </ul>
          <div className="dlg-row end">
            <button className="btn" onClick={() => setConfirming(false)} disabled={busy}>
              やめる
            </button>
            <button className="btn danger" onClick={() => void clear()} disabled={busy}>
              {busy ? "消しています…" : "消す"}
            </button>
          </div>
        </div>
      )}
      {busy && <Busy>ログを消しています…</Busy>}
      <ErrorBox error={error} title="ログのフォルダを操作できません" />
    </div>
  );
}
