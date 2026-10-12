// 削除のダイアログ（FILE-01）: 種類（カタログから除去・ゴミ箱に移す・仮想コピーだけを削除）を選ぶたびに
// 影響の一覧（計画）を見せ、確認してから実行する。計画の後にカタログが変わっていたら、計画し直して確かめ直す。

import { useCallback, useEffect, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { DeleteKind, DeletePlan, ErrorInfo, VariantId } from "../api/types";
import { Dialog } from "../components/Dialog";
import { actions } from "../state/app";
import {
  DELETE_KIND_OPTIONS,
  deleteKindLabel,
  describeDeletePlan,
  describeDeleteReport,
  isOutdatedPlanError,
  planFileLines,
  planVariantLines,
  summarizeDeletePlan,
} from "./deleteLogic";
import { formatCount } from "./dialogFormat";
import { Busy, ErrorBox } from "./parts";

function PlanList(props: { lines: string[]; rest: number }) {
  if (props.lines.length === 0) return null;
  return (
    <div className="dlg-list">
      {props.lines.map((l, i) => (
        <div key={i} className="dlg-list-item dlg-path">
          {l}
        </div>
      ))}
      {props.rest > 0 && <div className="dlg-list-more">ほか {formatCount(props.rest)} 件</div>}
    </div>
  );
}

export function DeleteDialog(props: { deleteKind: DeleteKind; variantIds: VariantId[] }) {
  const ids = props.variantIds;
  const [kind, setKind] = useState<DeleteKind>(props.deleteKind);
  const [plan, setPlan] = useState<DeletePlan | null>(null);
  const [planError, setPlanError] = useState<ErrorInfo | null>(null);
  const [planning, setPlanning] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [replanned, setReplanned] = useState(false);
  const seq = useRef(0);

  const makePlan = useCallback(
    async (k: DeleteKind) => {
      const my = ++seq.current;
      setPlanning(true);
      setPlan(null);
      setPlanError(null);
      try {
        const p = await core.planDelete(k, ids);
        if (my === seq.current) setPlan(p);
      } catch (e) {
        if (my === seq.current) setPlanError(toApiError(e).info);
      } finally {
        if (my === seq.current) setPlanning(false);
      }
    },
    [ids],
  );

  useEffect(() => {
    setError(null);
    setReplanned(false);
    void makePlan(kind);
  }, [kind, makePlan]);

  // 選んでいる種類の計画だけを見せ・実行する（古い応答が残っても、別の種類の計画は使わない）。
  const current = plan && plan.kind === kind ? plan : null;

  const execute = async () => {
    const plan = current;
    if (!plan) return;
    setBusy(true);
    setError(null);
    try {
      const report = await core.executeDelete(plan.plan_id);
      actions.closeDialog();
      const n = describeDeleteReport(kind, report);
      actions.notify(n.level, n.message, n.detail);
      await actions.runSearch();
    } catch (e) {
      const err = toApiError(e).info;
      if (isOutdatedPlanError(err)) {
        // 計画の後にカタログが変わった。新しい計画を見せて、確かめ直してもらう。
        setReplanned(true);
        await makePlan(kind);
      } else {
        setError(err);
      }
    } finally {
      setBusy(false);
    }
  };

  const summary = current ? summarizeDeletePlan(current, ids) : null;
  const option = DELETE_KIND_OPTIONS.find((o) => o.kind === kind);
  const danger = option?.danger ?? false;
  const list = current ? (kind === "virtual_copies" ? planVariantLines(current) : planFileLines(current)) : null;

  return (
    <Dialog
      title={`削除（${formatCount(ids.length)} 件を選択）`}
      width={640}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button
            className={`btn ${danger ? "danger" : "primary"}`}
            onClick={() => void execute()}
            disabled={busy || planning || !current}
          >
            {busy ? "実行中…" : deleteKindLabel(kind)}
          </button>
        </>
      }
    >
      <div className="dlg-options" role="radiogroup" aria-label="削除の種類">
        {DELETE_KIND_OPTIONS.map((o) => (
          <label key={o.kind} className={`dlg-option${o.kind === kind ? " on" : ""}${o.danger ? " danger" : ""}`}>
            <input
              type="radio"
              name="delete-kind"
              checked={o.kind === kind}
              disabled={busy}
              onChange={() => setKind(o.kind)}
            />
            <span className="title">{o.label}</span>
            <span className="desc">{o.description}</span>
          </label>
        ))}
      </div>

      <div className="dlg-section">
        <h3>影響の一覧</h3>
        {planning && <Busy>影響を受けるファイルを確かめています…</Busy>}
        {replanned && current && (
          <div className="dlg-warn">
            確認の後にカタログが変わったため、もう一度確かめました。内容を確かめてから、もう一度実行してください。
          </div>
        )}
        {planError && (
          <ErrorBox
            error={planError}
            title={
              kind === "virtual_copies" && planError.kind === "invalid_input"
                ? "選んだ中に仮想コピーでない写真（マスター）があります。仮想コピーだけを選ぶか、「カタログから除去」を使ってください"
                : "影響を確かめられません"
            }
          />
        )}
        {current && summary && (
          <>
            <div className={danger ? "dlg-warn" : "dlg-note"}>{describeDeletePlan(kind, summary).join("\n")}</div>
            {list && <PlanList lines={list.lines} rest={list.rest} />}
          </>
        )}
      </div>
      <ErrorBox error={error} title="削除できません" />
    </Dialog>
  );
}
