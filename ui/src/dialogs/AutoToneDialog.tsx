// 選んだ写真にまとめて自動補正（DEV-32）を当てる確認のダイアログ（写真の右クリックのメニューの「自動補正を
// 当てる…」）。
//
// コアのジョブ（auto_tone_variants）で 1 枚ずつ値を求めて保存する。始めたらダイアログを閉じ、進捗は上の帯の
// ジョブの表示に出す（取り消せる）。終わったら結果を知らせる。現像中の写真が含まれていれば、送信待ちの設定を
// 先に送ってから始め（コアは現像中の写真の今の設定から値を求める）、終わった後にコアの状態を読み直す。

import { useMemo, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { ErrorInfo, VariantId } from "../api/types";
import { Dialog } from "../components/Dialog";
import { actions } from "../state/app";
import { developStore, enqueue, reloadState } from "../state/develop";
import { describeAutoToneJob } from "./autoToneLogic";
import { waitJobUntilClosed } from "./catalogActions";
import { formatCount } from "./dialogFormat";
import { ErrorBox } from "./parts";
import { notifyResult } from "./resultLog";

/** ジョブが終わるのを待って、結果を知らせる（現像中の写真が含まれていれば、コアの状態を読み直す）。 */
async function followAutoToneJob(jobId: number, ids: VariantId[]): Promise<void> {
  try {
    const info = await waitJobUntilClosed(jobId);
    if (info === "closed") return;
    const developing = developStore.get().variantId;
    if (developing !== null && ids.includes(developing)) await reloadState();
    const notice = describeAutoToneJob(info);
    if (notice) notifyResult(notice);
  } catch (e) {
    actions.reportError(e, "自動補正の結果を受け取れません");
  }
}

export function AutoToneDialog(props: { targets: VariantId[] }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const ids = useMemo(() => [...new Set(props.targets)], [props.targets]);

  const ok = async () => {
    if (ids.length === 0) return;
    setBusy(true);
    setError(null);
    const developing = developStore.get().variantId;
    const includesDeveloping = developing !== null && ids.includes(developing);
    try {
      let jobId: number | null = null;
      let failure: unknown = null;
      const run = async () => {
        try {
          jobId = await core.autoToneVariants(ids);
        } catch (e) {
          failure = e;
        }
      };
      // 現像中の写真が含まれていれば、送信待ちの設定の後に始める（順序を保つ）。
      if (includesDeveloping) await enqueue(run);
      else await run();
      if (failure !== null) throw failure;
      if (jobId === null) return;
      actions.trackJob(jobId, "auto_tone", `自動補正: ${formatCount(ids.length)} 枚`);
      actions.closeDialog();
      void followAutoToneJob(jobId, ids);
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={`自動補正を当てる（${formatCount(ids.length)} 枚）`}
      width={520}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void ok()} disabled={busy || ids.length === 0}>
            {busy ? "始めています…" : "当てる"}
          </button>
        </>
      }
    >
      <p style={{ marginTop: 0 }}>
        選んだ写真のそれぞれに、現像の「基本補正」の「自動」と同じ方法で、露光量・コントラスト・ハイライト・シャドウ・
        白レベル・黒レベルを決めて入れます。ほかの項目（ホワイトバランス・彩度・切り抜きなど）は変えません。動画は
        飛ばします。
      </p>
      <div className="dlg-note">
        写真を 1 枚ずつ開いて値を求めるので、枚数が多いと時間がかかります。進み具合は上の帯に出て、途中で取り消せます
        （取り消すまでに当てた写真はそのまま残ります）。
      </div>
      <div className="dlg-note">
        それぞれの写真の履歴に 1 件（「自動補正」）記録します（現像の画面の履歴から 1 枚ずつ元に戻せます。まとめて元に戻す
        操作はありません）。元のファイルは変更しません。
      </div>
      <ErrorBox error={error} title="自動補正を始められません" />
    </Dialog>
  );
}
