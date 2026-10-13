// 起動時のクラッシュの知らせ（SYS-07）: 前回までにアプリが異常終了して、クラッシュレポートを保存していたら、
// 1 回だけ知らせる（アプリの app_status の crash_reports）。レポートはこの PC に保存しただけで、送信はしない。

import { useState } from "react";

import * as app from "../api/app";
import { toApiError } from "../api/errors";
import type { CrashReportSummary, ErrorInfo } from "../api/types";
import { Dialog } from "../components/Dialog";
import { actions } from "../state/app";
import { crashNoticeTitle, describeCrashReport, LOCAL_ONLY_NOTE } from "./logsLogic";
import { ErrorBox } from "./parts";

/** 一覧に出すレポートの数の上限。 */
const SHOWN = 5;

export function CrashReportsDialog(props: { reports: CrashReportSummary[] }) {
  const { reports } = props;
  const [error, setError] = useState<ErrorInfo | null>(null);

  const open = async () => {
    setError(null);
    try {
      await app.openLogFolder();
    } catch (e) {
      setError(toApiError(e).info);
    }
  };

  return (
    <Dialog
      title={crashNoticeTitle(reports)}
      width={560}
      onClose={actions.closeDialog}
      actions={
        <>
          <button className="btn" onClick={() => void open()}>
            ログのフォルダを開く
          </button>
          <button className="btn primary" onClick={actions.closeDialog}>
            閉じる
          </button>
        </>
      }
    >
      <p>
        異常終了したときの報告（クラッシュレポート）を、ログのフォルダに保存しました。不具合を報告するときは、このファイルを添えてください。
      </p>
      <ul className="hint" style={{ paddingLeft: 18 }}>
        {reports.slice(0, SHOWN).map((r) => (
          <li key={r.file_name}>{describeCrashReport(r)}</li>
        ))}
        {reports.length > SHOWN && <li>ほか {reports.length - SHOWN} 件</li>}
      </ul>
      <p className="hint">
        {LOCAL_ONLY_NOTE}渡す前に中身を確かめてください。不要になったら、設定の「一般」から消せます。
      </p>
      <p className="hint">
        カタログを開いていた場合は、設定の「バックアップ」の詳細チェックで、カタログに問題がないかを確かめられます。
      </p>
      <ErrorBox error={error} title="ログのフォルダを開けません" />
    </Dialog>
  );
}
