// 比較明・比較暗合成（DEV-34。docs/composite_design.md の 6.2 節）のダイアログ。
//
// - 選んだ写真（動画はコアが除く）を、撮影日時の順に現像して画素ごとに比べ、1 枚にして保存する。保存したファイルは
//   カタログに登録し、既定では元の写真とスタックにまとめて一番上にする。
// - 入力を変えるたびに、確認（core.compositeCheck。写真の並び・設定の違い・保存先とファイル名・メモリの見込み）を
//   出す。合成できない理由（problems）があれば「合成」を押せない。時間の目安は出さない（計測していないため）。
// - 始めたらダイアログを閉じ、進み具合はジョブの一覧に出す。終わったら結果を知らせ、合成の結果を選ぶ。

import { useEffect, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { CompositeCheck, CompositeMode, CompositeSettings, ErrorInfo, VariantId } from "../api/types";
import { Dialog } from "../components/Dialog";
import { actions } from "../state/app";
import { waitJobUntilClosed } from "./catalogActions";
import {
  type CompositeForm,
  MODE_OPTIONS,
  defaultCompositeForm,
  describeCheck,
  describeCompositeJob,
  destOf,
  modeLabel,
  toCompositeSettings,
} from "./compositeLogic";
import { formatCount } from "./dialogFormat";
import { COLOR_SPACE_OPTIONS, FORMAT_OPTIONS, type FormatKind } from "./exportLogic";
import { chooseFolder } from "./native";
import { Busy, ErrorBox, PathBox } from "./parts";
import { notifyResult } from "./resultLog";

/** 確認を問い合わせるまでの時間（ms。入力の途中で問い合わせすぎない。仮置き）。 */
const CHECK_DELAY_MS = 250;

/** 合成のジョブの終わりを待って、結果を知らせ、合成の結果を選ぶ。 */
async function followComposite(jobId: number): Promise<void> {
  try {
    const info = await waitJobUntilClosed(jobId);
    if (info === "closed") return;
    const notice = describeCompositeJob(info);
    if (notice) notifyResult(notice);
    const r = info.result;
    if (r && r.type === "composite" && r.registered !== null) {
      await actions.runSearch();
      await actions.selectById(r.registered);
    }
  } catch (e) {
    actions.reportError(e, "合成の結果を受け取れません");
  }
}

export function CompositeDialog(props: { variantIds: VariantId[]; mode: CompositeMode }) {
  const [form, setForm] = useState<CompositeForm>(() => defaultCompositeForm(props.mode));
  const [advanced, setAdvanced] = useState(MODE_OPTIONS.find((o) => o.value === props.mode)?.advanced ?? false);
  const [check, setCheck] = useState<CompositeCheck | null>(null);
  const [checking, setChecking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const seq = useRef(0);
  const update = (patch: Partial<CompositeForm>) => setForm((f) => ({ ...f, ...patch }));
  const settings = toCompositeSettings(form);
  const dest = destOf(form);
  const key = settings ? JSON.stringify({ settings, dest }) : null;

  useEffect(() => {
    const my = ++seq.current;
    if (key === null) {
      setCheck(null);
      setChecking(false);
      return;
    }
    setChecking(true);
    const t = setTimeout(() => {
      const q = JSON.parse(key) as { settings: CompositeSettings; dest: string | null };
      core
        .compositeCheck(props.variantIds, q.settings, q.dest)
        .then((c) => {
          if (my === seq.current) {
            setCheck(c);
            setError(null);
          }
        })
        .catch((e: unknown) => {
          if (my === seq.current) setError(toApiError(e).info);
        })
        .finally(() => {
          if (my === seq.current) setChecking(false);
        });
    }, CHECK_DELAY_MS);
    return () => clearTimeout(t);
  }, [key, props.variantIds]);

  const chooseDest = async () => {
    try {
      const p = await chooseFolder("合成の結果の保存先を選ぶ", form.destDir || check?.dest_dir || null);
      if (p) update({ destDir: p });
    } catch (e) {
      actions.reportError(e, "フォルダを選べません");
    }
  };

  const ready = settings !== null && check !== null && !checking && check.problems.length === 0;
  const start = async () => {
    if (!ready || settings === null || check === null) return;
    setBusy(true);
    setError(null);
    let jobId: number;
    try {
      jobId = await core.composite(props.variantIds, settings, dest);
    } catch (e) {
      setError(toApiError(e).info);
      setBusy(false);
      return;
    }
    actions.trackJob(jobId, "composite", `${modeLabel(form.mode)}: ${formatCount(check.sources.length)} 枚`);
    actions.closeDialog();
    void followComposite(jobId);
  };

  const modes = MODE_OPTIONS.filter((o) => advanced || !o.advanced || o.value === form.mode);
  const modeHint = MODE_OPTIONS.find((o) => o.value === form.mode)?.hint;
  const lines = check && settings ? describeCheck(check, settings) : [];

  return (
    <Dialog
      title={`${modeLabel(form.mode)}（${formatCount(props.variantIds.length)} 枚を選択）`}
      width={560}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void start()} disabled={busy || !ready}>
            合成
          </button>
        </>
      }
    >
      <div className="field">
        <label htmlFor="composite-mode">比べ方</label>
        <div className="dlg-row">
          <select
            id="composite-mode"
            className="dlg-grow"
            value={form.mode}
            onChange={(e) => update({ mode: e.target.value as CompositeMode })}
          >
            {modes.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <label className="dlg-check" style={{ margin: 0 }}>
            <input type="checkbox" checked={advanced} onChange={(e) => setAdvanced(e.target.checked)} />
            詳しい設定
          </label>
        </div>
        {modeHint && <p className="hint">{modeHint}</p>}
      </div>

      <div className="field">
        <label>保存の形式</label>
        <div className="dlg-row">
          <select
            aria-label="形式"
            value={form.format}
            onChange={(e) => update({ format: e.target.value as FormatKind })}
          >
            {FORMAT_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          {form.format === "jpeg" && (
            <input
              aria-label="JPEG の品質"
              inputMode="numeric"
              style={{ width: 64 }}
              value={form.quality}
              onChange={(e) => update({ quality: e.target.value })}
              aria-invalid={settings === null}
            />
          )}
          <select
            aria-label="色空間"
            value={form.colorSpace}
            onChange={(e) => update({ colorSpace: e.target.value as CompositeForm["colorSpace"] })}
          >
            {COLOR_SPACE_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </div>
        {settings === null && <div className="dlg-field-error">JPEG の品質は 1〜100 で入れてください</div>}
        <p className="hint">比べるのも、この色空間の値です。合成の後でこのアプリで仕上げるなら、16bit の TIFF をおすすめします。</p>
      </div>

      <div className="field">
        <label>保存先</label>
        <div className="dlg-row">
          <div className="dlg-grow">
            <PathBox path={form.destDir || check?.dest_dir} empty="1 枚目の写真と同じフォルダ" box />
          </div>
          <button className="btn" onClick={() => void chooseDest()} disabled={busy}>
            参照…
          </button>
          {form.destDir !== "" && (
            <button className="btn" onClick={() => update({ destDir: "" })} disabled={busy}>
              元に戻す
            </button>
          )}
        </div>
      </div>

      <div className="field">
        <label className="dlg-check">
          <input
            type="checkbox"
            checked={form.stackWithSources}
            onChange={(e) => update({ stackWithSources: e.target.checked })}
          />
          元の写真とスタックにまとめる（合成の結果を一番上にする）
        </label>
        <label className="dlg-check">
          <input type="checkbox" checked={form.removeGps} onChange={(e) => update({ removeGps: e.target.checked })} />
          GPS 情報を書かない
        </label>
      </div>

      <div className="dlg-section">
        {checking && <Busy>確認しています…</Busy>}
        {!checking &&
          lines.map((l) =>
            l.level === "info" ? (
              <div key={l.text} className="dlg-note">
                {l.text}
              </div>
            ) : (
              <div key={l.text} className={l.level === "error" ? "dlg-error" : "dlg-warn"}>
                {l.text}
              </div>
            ),
          )}
      </div>
      <div className="dlg-note">
        各写真に保存した現像の設定をかけてから合成します（元のファイルは変更しません）。三脚で固定して撮った写真向けで、位置合わせはしません。
      </div>
      <ErrorBox error={error} title="合成を始められません" />
    </Dialog>
  );
}
