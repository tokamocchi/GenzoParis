// カタログを閉じられなかったとき（終了・カタログの切り替えで、保存待ちの現像設定を保存できなかった）の
// ダイアログ（決定事項ログ No.61）: 「再試行」「保存せずに終了（閉じる）」「キャンセル」を選ばせる。
// 未保存の設定を黙って捨てない（DATA-03）。

import { useState } from "react";

import * as app from "../api/app";
import type { CloseFailure } from "../api/types";
import { Dialog } from "../components/Dialog";
import { actions } from "../state/app";
import { cancelAfterClose, continueAfterClose, markCatalogClosed } from "./catalogActions";
import { ErrorBox } from "./parts";

export function CloseFailedDialog(props: { failure: CloseFailure; quitting: boolean }) {
  const { failure, quitting } = props;
  const [busy, setBusy] = useState(false);

  /** 閉じる（終了する）。`discard` なら保存できなかった設定を捨てる。 */
  const attempt = async (discard: boolean) => {
    setBusy(true);
    const r = quitting ? await app.quitApp(discard) : await app.closeCatalog(discard);
    if (r.ok) {
      // 終了なら、アプリはこのまま終わる。閉じたなら、続き（切り替え先を開く）へ。
      if (!quitting) await continueAfterClose();
      return;
    }
    setBusy(false);
    // まだ閉じられない・閉じたがエラーがあった: 新しい理由で出し直す。
    actions.openDialog({ kind: "close_failed", failure: r.failure, quitting });
  };

  const cancel = () => {
    cancelAfterClose();
    actions.closeDialog();
  };

  // 閉じたが、終了の処理の途中でエラーがあった（カタログは閉じている）。終了しないなら、カタログの選択の
  // 画面に戻る。
  if (!failure.still_open) {
    const stayClosed = () => {
      cancelAfterClose();
      markCatalogClosed();
    };
    return (
      <Dialog
        title={quitting ? "終了の処理でエラーがありました" : "カタログを閉じる処理でエラーがありました"}
        width={520}
        busy={busy}
        actions={
          quitting ? (
            <>
              <button className="btn" onClick={stayClosed} disabled={busy}>
                終了しない
              </button>
              <button className="btn primary" onClick={() => void attempt(true)} disabled={busy}>
                終了
              </button>
            </>
          ) : (
            <button
              className="btn primary"
              disabled={busy}
              onClick={() => {
                setBusy(true);
                void continueAfterClose();
              }}
            >
              OK
            </button>
          )
        }
      >
        <p>カタログは閉じました。</p>
        <ErrorBox error={failure.error} />
      </Dialog>
    );
  }

  return (
    <Dialog
      title={quitting ? "現像設定を保存できないため、終了できません" : "現像設定を保存できないため、カタログを閉じられません"}
      width={560}
      onClose={cancel}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={cancel} disabled={busy}>
            キャンセル
          </button>
          <button className="btn danger" onClick={() => void attempt(true)} disabled={busy}>
            {quitting ? "保存せずに終了" : "保存せずに閉じる"}
          </button>
          <button className="btn primary" onClick={() => void attempt(false)} disabled={busy}>
            {busy ? "処理中…" : "再試行"}
          </button>
        </>
      }
    >
      <p>保存していない現像設定があります。カタログは開いたままです。</p>
      <ErrorBox error={failure.error} />
      <ul className="hint" style={{ paddingLeft: 18 }}>
        <li>再試行: 空き容量・カタログのフォルダの権限を確かめてから、もう一度保存して{quitting ? "終了" : "閉じ"}ます。</li>
        <li>
          {quitting ? "保存せずに終了" : "保存せずに閉じる"}: 最後にもう一度だけ保存を試し、できなければ保存していない現像設定を捨てます（元に戻せません）。
        </li>
        <li>キャンセル: {quitting ? "終了せずに" : "閉じずに"}作業を続けます。</li>
      </ul>
    </Dialog>
  );
}
