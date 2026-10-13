// 削除のダイアログ（FILE-01）: 種類（カタログから除去・ゴミ箱に移す・仮想コピーだけを削除）を選ぶたびに
// 影響の一覧（計画）を見せ、確認してから実行する。計画の後にカタログが変わっていたら、計画し直して確かめ直す。
//
// 折りたたんだスタック（LIB-12）の代表を選んでいるときは、スタックの中に隠れている写真も含めるかを選べる
// （既定は含めない。選んで見えている写真だけを消す。決定事項ログ No.86）。仮想コピーだけの削除では選べない
// （隠れた写真にマスターが含まれうるため）。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { DeleteKind, DeletePlan, ErrorInfo, VariantId } from "../api/types";
import { Dialog } from "../components/Dialog";
import { hiddenStackMembers } from "../library/stackLogic";
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
import { notifyResult } from "./resultLog";

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
  const selectedIds = props.variantIds;
  const [kind, setKind] = useState<DeleteKind>(props.deleteKind);
  // 折りたたんだスタックの中に隠れている写真（選んだ代表のスタックの、選んでいない写真）。
  const [hidden, setHidden] = useState<VariantId[]>([]);
  const [includeHidden, setIncludeHidden] = useState(false);
  useEffect(() => {
    let alive = true;
    core
      .variantStacks(selectedIds)
      .then((stacks) => alive && setHidden(hiddenStackMembers(selectedIds, stacks)))
      .catch(() => alive && setHidden([]));
    return () => {
      alive = false;
    };
  }, [selectedIds]);
  const canIncludeHidden = hidden.length > 0 && kind !== "virtual_copies";
  const withHidden = canIncludeHidden && includeHidden;
  const ids = useMemo(() => (withHidden ? [...selectedIds, ...hidden] : selectedIds), [withHidden, selectedIds, hidden]);
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
  const shown = plan && plan.kind === kind ? plan : null;

  const execute = async () => {
    if (!shown) return;
    setBusy(true);
    setError(null);
    try {
      const report = await core.executeDelete(shown.plan_id);
      actions.closeDialog();
      // 詳細の全体は、上の帯の「結果」で後から見直せる。
      notifyResult(describeDeleteReport(kind, report));
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

  const summary = shown ? summarizeDeletePlan(shown, ids) : null;
  const option = DELETE_KIND_OPTIONS.find((o) => o.kind === kind);
  const danger = option?.danger ?? false;
  const list = shown ? (kind === "virtual_copies" ? planVariantLines(shown) : planFileLines(shown)) : null;

  return (
    <Dialog
      title={`削除（${formatCount(selectedIds.length)} 件を選択${withHidden ? `。スタックの中の ${formatCount(hidden.length)} 枚を含む` : ""}）`}
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
            disabled={busy || planning || !shown}
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

      {canIncludeHidden && (
        <label className="dlg-check">
          <input
            type="checkbox"
            checked={includeHidden}
            disabled={busy}
            onChange={(e) => setIncludeHidden(e.target.checked)}
          />
          <span>
            折りたたんだスタックの中の写真（ほか {formatCount(hidden.length)} 枚）も含める
            <span className="desc">
              含めなければ、選んで見えている写真（スタックの代表）だけが対象です。スタックのほかの写真は残ります。
            </span>
          </span>
        </label>
      )}

      <div className="dlg-section">
        <h3>影響の一覧</h3>
        {planning && <Busy>影響を受けるファイルを確かめています…</Busy>}
        {replanned && shown && (
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
        {shown && summary && (
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
