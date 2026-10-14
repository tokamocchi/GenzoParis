// 撮影日時の一括修正のダイアログ（LIB-16。docs/04 の 3.13 節）。
//
// - 対象: 選んだ写真か、機材（カメラの機種・シリアル番号・写真か動画か）ごとのすべて（既定は表示中の写真の
//   機材）。同じ写真の仮想コピーは撮影日時を共有する。
// - 方法: 基準の写真（表示中の写真）の正しい時刻に合わせる（差を求めて対象のすべてをずらす。既定）・時刻を
//   ずらす（日・時間・分・秒）・タイムゾーンの指定（2 つの読み方）・指定の取り消し・ファイルの値に戻す。
// - 入力を変えるたびに、コアに見込み（変わる数・撮影日時の記録の内訳・例）を問い合わせる（何も変えない）。
// - 修正はカタログの中だけで、元のファイルには書かない。最後の修正から順に「直前の修正を取り消す」で戻せる。
//   修正した後は、コアが検索結果を作り直して知らせる（並び・タイムラインの日付が変わる）。

import { useEffect, useMemo, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type {
  CaptureDevice,
  CaptureDeviceKey,
  CaptureTarget,
  CaptureTimeEditInfo,
  CaptureTimePlan,
  ErrorInfo,
  VariantDetails,
  VariantId,
  ZoneMode,
} from "../api/types";
import { Dialog } from "../components/Dialog";
import { formatCaptureTime } from "../format";
import {
  buildEdit,
  CAPTURE_METHODS,
  type CaptureMethod,
  captureLocal,
  describeEditInfo,
  describePlan,
  deviceId,
  deviceLabel,
  deviceSummary,
  EMPTY_SHIFT,
  formatOffsetMinutes,
  sameDevice,
  type ShiftInput,
  shiftSeconds,
  sourceLines,
  ZONE_MODES,
  ZONE_OPTIONS,
  zoneLabel,
} from "../library/captureTimeLogic";
import { actions } from "../state/app";
import { Busy, ErrorBox } from "./parts";

/** 見込みを問い合わせるまでの時間（ms。入力の途中で問い合わせすぎない。仮置き）。 */
const PREVIEW_DELAY_MS = 300;

/** 撮影日時の表示（現地時刻とオフセット）。 */
function captureText(c: Parameters<typeof formatCaptureTime>[0]): string {
  if (!c?.utc) return "撮影日時なし";
  return `${formatCaptureTime(c)}${c.tz_assumed ? ` ${c.tz_assumed}` : ""}`;
}

export function CaptureTimeDialog(props: { variantIds: VariantId[]; current: VariantId | null }) {
  const [devices, setDevices] = useState<CaptureDevice[] | null>(null);
  const [ref, setRef] = useState<VariantDetails | null>(null);
  const [refDevice, setRefDevice] = useState<CaptureDeviceKey | null>(null);
  const [defaultOffset, setDefaultOffset] = useState<number | null>(null);
  const [history, setHistory] = useState<CaptureTimeEditInfo[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [targetMode, setTargetMode] = useState<"selection" | "devices">("devices");
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [method, setMethod] = useState<CaptureMethod>("sync");
  const [correct, setCorrect] = useState("");
  const [correctZone, setCorrectZone] = useState<number | null>(null);
  const [shift, setShift] = useState<ShiftInput>(EMPTY_SHIFT);
  const [zone, setZone] = useState(540);
  const [zoneMode, setZoneMode] = useState<ZoneMode>("keep_local");

  const [plan, setPlan] = useState<CaptureTimePlan | null>(null);
  const [planError, setPlanError] = useState<ErrorInfo | null>(null);
  const [counting, setCounting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const seq = useRef(0);

  const reloadHistory = () =>
    core
      .captureTimeEdits()
      .then(setHistory)
      .catch(() => setHistory([]));

  // 開いたとき: 機材・基準の写真・既定のタイムゾーン・履歴を読む。
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const [list, settings] = await Promise.all([core.captureDevices(), core.getSettings()]);
        if (!alive) return;
        setDevices(list);
        setDefaultOffset(settings.default_utc_offset_minutes);
        setZone(settings.default_utc_offset_minutes);
        if (props.current !== null) {
          const [details, device] = await Promise.all([
            core.variantDetails(props.current),
            core.captureDeviceOf(props.current),
          ]);
          if (!alive) return;
          setRef(details);
          setRefDevice(device);
          setChosen(new Set([deviceId(device)]));
          const local = captureLocal(details.summary.capture);
          if (local) setCorrect(local.replace("T", " "));
          else setMethod("shift");
        } else {
          setTargetMode("selection");
          setMethod("shift");
        }
      } catch (e) {
        if (alive) setLoadError(actions.describeError(e));
      }
    })();
    void reloadHistory();
    return () => {
      alive = false;
    };
  }, [props.current]);

  const target: CaptureTarget | null = useMemo(() => {
    if (targetMode === "selection") {
      return props.variantIds.length > 0 ? { kind: "variants", variant_ids: props.variantIds } : null;
    }
    const keys = (devices ?? []).filter((d) => chosen.has(deviceId(d))).map(({ camera, serial, kind }) => ({ camera, serial, kind }));
    return keys.length > 0 ? { kind: "devices", devices: keys } : null;
  }, [targetMode, props.variantIds, devices, chosen]);

  const edit = buildEdit(method, {
    reference: ref?.summary.variant_id ?? null,
    correct,
    correctZone,
    shift,
    zone,
    zoneMode,
  });
  const editKey = JSON.stringify(edit);
  const targetKey = JSON.stringify(target);

  // 入力が変わったら見込みを問い合わせる。
  useEffect(() => {
    const my = ++seq.current;
    setPlan(null);
    setPlanError(null);
    if (!target || !edit) {
      setCounting(false);
      return;
    }
    setCounting(true);
    const t = setTimeout(() => {
      core
        .planCaptureTimeEdit(target, edit)
        .then((p) => {
          if (my === seq.current) setPlan(p);
        })
        .catch((e: unknown) => {
          if (my === seq.current) setPlanError(toApiError(e).info);
        })
        .finally(() => {
          if (my === seq.current) setCounting(false);
        });
    }, PREVIEW_DELAY_MS);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editKey, targetKey]);

  const run = async () => {
    if (!target || !edit) return;
    setBusy(true);
    setError(null);
    try {
      const r = await core.editCaptureTimes(target, edit);
      actions.closeDialog();
      actions.notify(
        r.changed > 0 ? "success" : "info",
        describePlan(r, true).join(" "),
        r.changed > 0 ? "元のファイルは変えていません。撮影日時の修正のダイアログの「直前の修正を取り消す」で戻せます。" : undefined,
      );
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  const undo = async () => {
    setBusy(true);
    setError(null);
    try {
      const u = await core.undoCaptureTimeEdit();
      actions.notify(
        "info",
        u ? `撮影日時の修正（${describeEditInfo(u.edit)}）を取り消しました（${u.restored.toLocaleString("ja-JP")} 枚）` : "取り消す修正はありません",
      );
      await reloadHistory();
      setDevices(await core.captureDevices());
      if (props.current !== null) setRef(await core.variantDetails(props.current));
      // 見込みを問い合わせ直す。
      seq.current++;
      setPlan(null);
      if (target && edit) setPlan(await core.planCaptureTimeEdit(target, edit));
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  const toggleDevice = (d: CaptureDeviceKey) => {
    const id = deviceId(d);
    setChosen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const shiftInvalid = method === "shift" && shiftSeconds(shift) === null;
  const disabled = busy || !target || !edit || !plan || plan.changed === 0 || shiftInvalid;
  const defaultText = defaultOffset === null ? null : formatOffsetMinutes(defaultOffset);
  const refZone = ref?.summary.capture.tz_assumed ?? null;
  const n = (v: number) => v.toLocaleString("ja-JP");

  return (
    <Dialog
      title="撮影日時の修正"
      width={640}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button className="btn" onClick={() => void undo()} disabled={busy || history.length === 0} title={history[0] ? describeEditInfo(history[0]) : undefined}>
            直前の修正を取り消す
          </button>
          <span className="spacer" />
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => void run()} disabled={disabled}>
            {busy ? "修正しています…" : "修正する"}
          </button>
        </>
      }
    >
      {loadError && <p className="error-text">読み込めません: {loadError}</p>}
      <p className="hint" style={{ marginTop: 0 }}>
        カタログの中の撮影日時だけを変えます（元のファイルには書きません。書き出したファイルのメタデータには修正後の撮影日時が入ります）。並べ替え・絞り込み・タイムラインにも反映します。
      </p>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!disabled) void run();
        }}
      >
        <div className="dlg-section">
          <h3>対象</h3>
          <div className="dlg-options" role="radiogroup" aria-label="対象">
            <label className={`dlg-option${targetMode === "devices" ? " on" : ""}`}>
              <input type="radio" name="ct-target" checked={targetMode === "devices"} onChange={() => setTargetMode("devices")} />
              <span className="title">機材ごとのすべて</span>
              <span className="desc">選んだ機材（カメラの機種・シリアル番号・写真か動画か）で撮った、カタログのすべての写真・動画</span>
            </label>
            <label className={`dlg-option${targetMode === "selection" ? " on" : ""}`}>
              <input
                type="radio"
                name="ct-target"
                checked={targetMode === "selection"}
                onChange={() => setTargetMode("selection")}
                disabled={props.variantIds.length === 0}
              />
              <span className="title">選んだ写真（{n(props.variantIds.length)} 枚）</span>
              <span className="desc">グリッドで選んでいる写真だけ</span>
            </label>
          </div>
          {targetMode === "devices" && (
            <div className="dlg-list" role="group" aria-label="機材">
              {devices === null && !loadError && <Busy>機材を読んでいます…</Busy>}
              {devices?.length === 0 && <p className="hint">写真・動画がありません</p>}
              {devices?.map((d) => (
                <label key={deviceId(d)} className="dlg-check" style={{ padding: "2px 8px" }}>
                  <input type="checkbox" checked={chosen.has(deviceId(d))} onChange={() => toggleDevice(d)} />
                  <span>
                    {deviceLabel(d)}
                    {refDevice && sameDevice(d, refDevice) && <span className="hint">（表示中の写真の機材）</span>}
                    <span className="desc">{deviceSummary(d)}</span>
                  </span>
                </label>
              ))}
            </div>
          )}
        </div>

        <div className="dlg-section">
          <h3>方法</h3>
          <div className="dlg-row">
            <select
              className="dlg-grow"
              aria-label="方法"
              value={method}
              onChange={(e) => setMethod(e.target.value as CaptureMethod)}
            >
              {CAPTURE_METHODS.map((m) => (
                <option key={m.value} value={m.value} disabled={m.value === "sync" && !ref?.summary.capture.utc}>
                  {m.label}
                </option>
              ))}
            </select>
          </div>
          <p className="hint">{CAPTURE_METHODS.find((m) => m.value === method)?.hint}</p>

          {method === "sync" && ref && (
            <>
              <div className="dlg-note">
                基準の写真: {ref.summary.file_name}
                {"\n"}今の撮影日時: {captureText(ref.summary.capture)}
              </div>
              <div>
                <div className="field">
                  <label htmlFor="ct-correct">正しい撮影日時</label>
                  <input
                    id="ct-correct"
                    value={correct}
                    placeholder="2024-05-01 12:34:56"
                    onChange={(e) => setCorrect(e.target.value)}
                    aria-invalid={buildEdit("sync", { reference: 1, correct, correctZone, shift, zone, zoneMode }) === null}
                  />
                </div>
                <div className="field">
                  <label htmlFor="ct-correct-zone">タイムゾーン</label>
                  <select
                    id="ct-correct-zone"
                    value={correctZone === null ? "" : String(correctZone)}
                    onChange={(e) => setCorrectZone(e.target.value === "" ? null : Number(e.target.value))}
                  >
                    <option value="">基準の写真の表示（{refZone ?? "不明"}）</option>
                    {ZONE_OPTIONS.map((z) => (
                      <option key={z} value={z}>
                        {zoneLabel(z)}
                      </option>
                    ))}
                  </select>
                </div>
              </div>
              <p className="hint">時計の写真や、時刻の正しい別の機材（スマートフォンなど）で同じときに撮った写真の時刻を入れてください（「年-月-日 時:分:秒」）。</p>
            </>
          )}

          {method === "shift" && (
            <div className="dlg-row">
              <select
                aria-label="向き"
                value={shift.negative ? "-" : "+"}
                onChange={(e) => setShift({ ...shift, negative: e.target.value === "-" })}
              >
                <option value="+">後の時刻にする（+）</option>
                <option value="-">前の時刻にする（−）</option>
              </select>
              {(
                [
                  ["days", "日"],
                  ["hours", "時間"],
                  ["minutes", "分"],
                  ["seconds", "秒"],
                ] as const
              ).map(([k, label]) => (
                <label key={k} className="dlg-row" style={{ gap: 4 }}>
                  <input
                    className="dlg-inline-num"
                    style={{ width: 64 }}
                    inputMode="numeric"
                    value={shift[k]}
                    onChange={(e) => setShift({ ...shift, [k]: e.target.value })}
                    aria-label={label}
                  />
                  {label}
                </label>
              ))}
              {shiftInvalid && <p className="dlg-field-error">0 以上の整数を入れてください（合わせて 36,525 日まで）</p>}
            </div>
          )}

          {method === "set_zone" && (
            <>
              <div className="field">
                <label htmlFor="ct-zone">タイムゾーン</label>
                <select id="ct-zone" value={zone} onChange={(e) => setZone(Number(e.target.value))}>
                  {ZONE_OPTIONS.map((z) => (
                    <option key={z} value={z}>
                      {zoneLabel(z)}
                      {z === defaultOffset ? "・既定" : ""}
                    </option>
                  ))}
                </select>
              </div>
              <div className="dlg-options" role="radiogroup" aria-label="読み方">
                {ZONE_MODES.map((m) => (
                  <label key={m.value} className={`dlg-option${zoneMode === m.value ? " on" : ""}`}>
                    <input type="radio" name="ct-zone-mode" checked={zoneMode === m.value} onChange={() => setZoneMode(m.value)} />
                    <span className="title">{m.label}</span>
                    <span className="desc">{m.hint}</span>
                  </label>
                ))}
              </div>
            </>
          )}
        </div>
      </form>

      <div className="dlg-section">
        <h3>見込み</h3>
        {counting && <Busy>見込みを数えています…</Busy>}
        {!target && <p className="hint">対象を選んでください。</p>}
        {target && !edit && !counting && method === "shift" && !shiftInvalid && <p className="hint">ずらす量を入れてください。</p>}
        {target && !edit && !counting && method === "sync" && <p className="hint">正しい撮影日時を「年-月-日 時:分:秒」の形で入れてください。</p>}
        {plan && !counting && (
          <div className="dlg-note">
            {[...describePlan(plan, false), ...sourceLines(plan.sources, defaultText).map((l) => `・${l}`)].join("\n")}
            {plan.examples.length > 0 && "\n例:"}
            {plan.examples.map((e) => `\n  ${e.file_name}: ${captureText(e.before)} → ${captureText(e.after)}`).join("")}
          </div>
        )}
        <ErrorBox error={planError} />
      </div>
      {history.length > 0 && (
        <p className="hint">
          直前の修正: {describeEditInfo(history[0])}（{n(history[0].asset_count)} 枚）。修正の履歴は {n(history.length)} 件（新しい 20 件まで残します）。
        </p>
      )}
      <ErrorBox error={error} />
    </Dialog>
  );
}
