// 選別支援のダイアログ（ORG-06。写真の右クリックの「選別支援…」）。
//
// 開くと、コアのジョブ（culling_assist）で、選んだ写真から似た写真のグループと、その中でピントの甘い見込みの写真を
// 調べる（何も変えない）。結果を見て、「甘い見込みの写真を選ぶ」（グリッドで選んだ状態にする。不採用にするかは
// 利用者が決める）か、「グループをスタックにまとめる」（いちばんシャープな見込みの写真を一番上にする）を選ぶ。

import { useEffect, useMemo, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { CullingReport, ErrorInfo, VariantId } from "../api/types";
import { thumbUrl } from "../api/uri";
import { Dialog } from "../components/Dialog";
import { actions } from "../state/app";
import { searchCache } from "../state/search";
import { waitJobUntilClosed } from "./catalogActions";
import {
  DEFAULT_CULLING_OPTIONS,
  cullingSkips,
  describeCulling,
  describeGroup,
  softVariants,
  stackableGroups,
} from "./cullingLogic";
import { formatCount } from "./dialogFormat";
import { ErrorBox } from "./parts";

/** ダイアログに並べるグループの数の上限（残りは数だけ出す。仮置き）。 */
const GROUPS_SHOWN = 30;

export function CullingDialog(props: { variantIds: VariantId[] }) {
  const ids = useMemo(() => [...new Set(props.variantIds)], [props.variantIds]);
  const [report, setReport] = useState<CullingReport | null>(null);
  const [busy, setBusy] = useState(true);
  const [stacking, setStacking] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const jobRef = useRef<number | null>(null);

  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const jobId = await core.cullingAssist(ids, DEFAULT_CULLING_OPTIONS);
        jobRef.current = jobId;
        actions.trackJob(jobId, "culling_assist", `選別支援: ${formatCount(ids.length)} 枚`);
        const info = await waitJobUntilClosed(jobId);
        if (!alive || info === "closed") return;
        if (info.result && info.result.type === "culling_assist") setReport(info.result);
        else if (info.error) setError(info.error);
      } catch (e) {
        if (alive) setError(toApiError(e).info);
      } finally {
        if (alive) setBusy(false);
      }
    })();
    return () => {
      alive = false;
      // 閉じたら、調べている途中のジョブを取り消す。
      if (jobRef.current !== null) void core.cancelJob(jobRef.current).catch(() => undefined);
    };
  }, [ids]);

  const soft = report ? softVariants(report) : [];

  const selectSoft = async () => {
    if (soft.length === 0) return;
    actions.closeDialog();
    await actions.selectIds(soft, soft[0]);
    actions.notify("info", `ピントの甘い見込みの ${formatCount(soft.length)} 枚を選びました（不採用にするには X）`);
  };

  const stackGroups = async () => {
    if (!report || report.groups.length === 0) return;
    setStacking(true);
    setError(null);
    let done = 0;
    try {
      // すでにスタックに入っている写真を含むグループは、元のスタックを壊さないように、まとめない（stackableGroups）。
      const stacked = new Set((await core.variantStacks(report.groups.flatMap((g) => g.variant_ids))).flatMap((s) => s.members));
      const { groups, skipped } = stackableGroups(report, stacked);
      for (const g of groups) {
        await core.createStack(g.variant_ids, g.sharpest);
        done++;
      }
      const skippedNote = skipped > 0 ? `。すでにスタックに入っている写真を含む ${formatCount(skipped)} 組は、まとめていません` : "";
      if (done === 0) {
        actions.notify("info", `まとめたグループはありません${skippedNote}`);
        return;
      }
      actions.closeDialog();
      actions.notify("success", `${formatCount(done)} 組をスタックにまとめました（いちばんシャープな見込みの写真が一番上）${skippedNote}`);
    } catch (e) {
      setError(toApiError(e).info);
      if (done > 0) actions.notify("info", `${formatCount(done)} 組はスタックにまとめました`);
    } finally {
      setStacking(false);
    }
  };

  const rev = (id: VariantId) => searchCache.getLoadedById(id)?.thumb_rev ?? null;

  return (
    <Dialog
      title={`選別支援（${formatCount(ids.length)} 枚）`}
      width={680}
      onClose={actions.closeDialog}
      busy={stacking}
      actions={
        <>
          <button className="btn" onClick={() => void selectSoft()} disabled={busy || stacking || soft.length === 0}>
            甘い見込みの {formatCount(soft.length)} 枚を選ぶ
          </button>
          <button
            className="btn"
            onClick={() => void stackGroups()}
            disabled={busy || stacking || !report || report.groups.length === 0}
            title="グループごとに、いちばんシャープな見込みの写真を一番上にしてスタックにまとめます（スタックの解除で戻せます。すでにスタックに入っている写真を含むグループはまとめません）"
          >
            {stacking ? "まとめています…" : `${formatCount(report?.groups.length ?? 0)} 組をスタックにまとめる`}
          </button>
          <span className="spacer" />
          <button className="btn primary" onClick={actions.closeDialog} disabled={stacking}>
            閉じる
          </button>
        </>
      }
    >
      <p style={{ marginTop: 0 }}>
        続けて撮った似た写真をグループにし、その中でピントの甘い（ぼけ・ブレの）見込みの写真を探します。評価・フラグ・スタックは、
        ここでは変えません。
      </p>
      {busy && <div className="dlg-note">サムネイルを調べています…</div>}
      {report && (
        <>
          <div className="dlg-note" aria-live="polite">
            <div>{describeCulling(report)}</div>
            {cullingSkips(report).map((l) => (
              <div key={l}>{l}</div>
            ))}
          </div>
          <div className="dlg-cull" role="group" aria-label="似た写真のグループ">
            {report.groups.slice(0, GROUPS_SHOWN).map((g, i) => (
              <div key={g.variant_ids[0]} className="dlg-cull-group">
                <div className="dlg-cull-head">
                  グループ {i + 1}（{describeGroup(g)}）
                </div>
                <div className="dlg-cull-row">
                  {g.variant_ids.map((id) => {
                    const mark = id === g.sharpest ? "sharp" : g.soft.includes(id) ? "soft" : "";
                    const r = rev(id);
                    return (
                      <button
                        type="button"
                        key={id}
                        className={`dlg-cull-item ${mark}`}
                        title={
                          id === g.sharpest
                            ? "いちばんシャープな見込み（クリックでグリッドで選ぶ）"
                            : mark === "soft"
                              ? "ピントの甘い見込み（クリックでグリッドで選ぶ）"
                              : "クリックでグリッドで選ぶ"
                        }
                        onClick={() => {
                          actions.closeDialog();
                          void actions.selectById(id);
                        }}
                      >
                        {r ? <img src={thumbUrl(id, r)} alt="" loading="lazy" draggable={false} /> : <span className="none">#{id}</span>}
                        {mark !== "" && <span className="mark">{mark === "sharp" ? "シャープ" : "甘い"}</span>}
                      </button>
                    );
                  })}
                </div>
              </div>
            ))}
            {report.groups.length > GROUPS_SHOWN && (
              <p className="hint">ほか {formatCount(report.groups.length - GROUPS_SHOWN)} 組（操作はすべての組に当たります）</p>
            )}
          </div>
        </>
      )}
      <div className="dlg-note">
        カタログのサムネイル（長辺 320 画素）で判定するので、粗い目安です。大きなぼけ・ブレは見つかりますが、等倍で分かる程度の
        僅かなピントのずれは分かりません。最後は、比較表示（C）や等倍表示（Z）で確かめてください。
      </div>
      <ErrorBox error={error} title="選別支援を使えません" />
    </Dialog>
  );
}
