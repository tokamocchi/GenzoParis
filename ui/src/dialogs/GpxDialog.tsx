// GPX のログから位置を付けるダイアログ（LIB-21 の拡張。写真の右クリックの「GPX のログから位置を付ける…」）。
//
// GPX のファイルを選ぶと、コアが見込み（plan_gpx_locations。何も変えない）を返す。設定（記録の途切れとみなす間隔・
// 写真の時計のずれ・位置のある写真にも付けるか）を変えると見込みを読み直す。「付ける」で実行する
// （apply_gpx_locations。1 回の変更として履歴に残り、位置を付けるダイアログの「元に戻す」で取り消せる）。
// 通信はしない（ログのファイルを読むだけ）。

import { useEffect, useMemo, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { ErrorInfo, GpxLocationPlan, VariantId } from "../api/types";
import { Dialog } from "../components/Dialog";
import {
  DEFAULT_GPX_FORM,
  type GpxForm,
  describeGpxPlan,
  describeGpxResult,
  describeGpxSkips,
  describeTrack,
  gpxOptionsOf,
} from "../library/gpxLogic";
import { actions } from "../state/app";
import { formatCount } from "./dialogFormat";
import { chooseGpxFile } from "./native";
import { ErrorBox } from "./parts";
import { notifyResult } from "./resultLog";

/** 入力が落ち着いてから見込みを読むまでの時間（ms。**仮置き**）。 */
const PLAN_DELAY_MS = 300;

export function GpxDialog(props: { variantIds: VariantId[] }) {
  const ids = useMemo(() => [...new Set(props.variantIds)], [props.variantIds]);
  const [path, setPath] = useState<string | null>(null);
  const [form, setForm] = useState<GpxForm>(DEFAULT_GPX_FORM);
  const [plan, setPlan] = useState<GpxLocationPlan | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const seq = useRef(0);

  const parsed = gpxOptionsOf(form);
  const options = "options" in parsed ? parsed.options : null;
  const inputError = "error" in parsed ? parsed.error : null;
  const optionsKey = options ? JSON.stringify(options) : null;

  // ファイル・設定が変わったら、少し待ってから見込みを読み直す（古い応答は捨てる）。
  useEffect(() => {
    const mine = ++seq.current;
    setPlan(null);
    if (path === null || optionsKey === null || ids.length === 0) return;
    const opts = JSON.parse(optionsKey) as NonNullable<typeof options>;
    const timer = setTimeout(() => {
      core
        .planGpxLocations(ids, path, opts)
        .then((p) => {
          if (mine !== seq.current) return;
          setPlan(p);
          setError(null);
        })
        .catch((e: unknown) => {
          if (mine === seq.current) setError(toApiError(e).info);
        });
    }, PLAN_DELAY_MS);
    return () => clearTimeout(timer);
  }, [path, optionsKey, ids]);

  const choose = async () => {
    try {
      const p = await chooseGpxFile("GPX のログを選ぶ", path);
      if (p !== null) {
        setError(null);
        setPath(p);
      }
    } catch (e) {
      setError(toApiError(e).info);
    }
  };

  const apply = async () => {
    if (path === null || options === null) return;
    setBusy(true);
    setError(null);
    try {
      const done = await core.applyGpxLocations(ids, path, options);
      actions.closeDialog();
      notifyResult(describeGpxResult(done, options.overwrite));
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  const update = (patch: Partial<GpxForm>) => setForm((f) => ({ ...f, ...patch }));
  const skips = plan && options ? describeGpxSkips(plan, options.overwrite) : [];

  return (
    <Dialog
      title={`GPX のログから位置を付ける（${formatCount(ids.length)} 枚）`}
      width={600}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button
            className="btn primary"
            onClick={() => void apply()}
            disabled={busy || path === null || options === null || plan === null || plan.changed === 0}
          >
            {busy ? "付けています…" : "付ける"}
          </button>
        </>
      }
    >
      <p style={{ marginTop: 0 }}>
        GPS ロガーやスマートフォンのアプリが記録した GPX のログと、写真の撮影日時を照らし合わせて、撮った場所を写真ごとに
        付けます。位置はカタログの中だけに持ち、元のファイルは変更しません。
      </p>
      <div className="dlg-row">
        <button className="btn" onClick={() => void choose()} disabled={busy}>
          GPX のファイルを選ぶ…
        </button>
        <span className={`dlg-path${path === null ? " empty" : ""}`} title={path ?? undefined}>
          {path ?? "まだ選んでいません"}
        </span>
      </div>
      <div className="dlg-row">
        <label htmlFor="gpx-max-gap">記録の途切れとみなす間隔</label>
        <input
          id="gpx-max-gap"
          type="text"
          inputMode="numeric"
          style={{ width: 80 }}
          value={form.maxGap}
          disabled={busy}
          onChange={(e) => update({ maxGap: e.target.value })}
          title="撮影の前後の 2 点の間隔がこれ以内なら、その間を補間します。これより長く途切れているところでは、近いほうの点がこの秒数以内にあるときだけ、その点を使います"
        />
        <span>秒</span>
      </div>
      <div className="dlg-row">
        <label htmlFor="gpx-offset">写真の時計のずれ</label>
        <input
          id="gpx-offset"
          type="text"
          inputMode="numeric"
          style={{ width: 80 }}
          value={form.offset}
          disabled={busy}
          onChange={(e) => update({ offset: e.target.value })}
          title="撮影日時にこの秒数を足してから、ログと照らし合わせます（カメラの時計が 2 分進んでいたなら -120）。カタログの撮影日時は変えません"
        />
        <span>秒（カメラの時計が進んでいたならマイナス）</span>
      </div>
      <label className="dlg-row">
        <input type="checkbox" checked={form.overwrite} disabled={busy} onChange={(e) => update({ overwrite: e.target.checked })} />
        位置のある写真にも付ける（カメラの位置は消えません。付けた位置を優先して使います）
      </label>
      {inputError && <div className="dlg-warn">{inputError}</div>}
      {plan && (
        <div className="dlg-note" aria-live="polite">
          <div>{describeTrack(plan)}</div>
          <div>{describeGpxPlan(plan)}</div>
          {skips.map((l) => (
            <div key={l}>{l}</div>
          ))}
        </div>
      )}
      {path !== null && options !== null && plan === null && !error && <div className="dlg-note">ログを読んでいます…</div>}
      <div className="dlg-note">
        撮影日時は、カタログの撮影日時（タイムゾーンを当てた後の時刻）を使います。時刻が合わないときは、先に「撮影日時を修正…」で
        直すか、上の「写真の時計のずれ」で合わせてください。動画には位置を付けません。
      </div>
      <ErrorBox error={error} title="GPX のログを使えません" />
    </Dialog>
  );
}
