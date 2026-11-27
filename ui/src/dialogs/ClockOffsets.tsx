// 機材ごとの時計のずれの見込み（ORG-02）: 撮影日時の修正のダイアログの「対象」の下に出す。
//
// 基準の機材（時計が合っているとみなす機材）を選んで「調べる」を押すと、コア（detect_clock_offsets）が、同じ
// 時間帯に撮った写真・動画の撮影日時から、ほかの機材の時計のずれを推定して返す（何も変えない）。「このずれを
// 入れる」で、その機材を対象にして、方法を「ずらす」にし、見込みの秒数を入れる（修正するかは利用者が決める）。
// 値はおよそ（数十秒〜数分の誤差）なので、秒まで合わせるには「基準の写真に合わせる」を使う。

import { useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { CaptureDevice, CaptureDeviceKey, ClockOffsetReport, ErrorInfo } from "../api/types";
import {
  clockVerdict,
  describeClockOffset,
  deviceId,
  deviceLabel,
  formatShift,
  suggestedShift,
} from "../library/captureTimeLogic";
import { ErrorBox } from "./parts";

export function ClockOffsets(props: {
  devices: CaptureDevice[];
  /** 初めに選ぶ基準の機材（表示中の写真の機材。なければ一覧の先頭）。 */
  defaultReference: CaptureDeviceKey | null;
  disabled: boolean;
  /** 見込みのずれを、その機材の修正として入れる。 */
  onUse: (device: CaptureDeviceKey, seconds: number) => void;
}) {
  const { devices } = props;
  const [reference, setReference] = useState<string | null>(null);
  const [report, setReport] = useState<ClockOffsetReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  if (devices.length < 2) return null;
  const fallback = props.defaultReference ? deviceId(props.defaultReference) : deviceId(devices[0]);
  const selected = reference ?? fallback;
  const referenceDevice = devices.find((d) => deviceId(d) === selected) ?? devices[0];

  const detect = async () => {
    setBusy(true);
    setError(null);
    setReport(null);
    try {
      const { camera, serial, kind } = referenceDevice;
      setReport(await core.detectClockOffsets({ camera, serial, kind }));
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  return (
    <details className="dlg-details">
      <summary>機材の時計のずれを調べる</summary>
      <p className="hint">
        時計が合っている機材を基準に選ぶと、同じ時間帯に撮った写真・動画から、ほかの機材の時計のずれのおよその値を出します（何も
        変えません）。値には数十秒〜数分の誤差があります。秒まで合わせるには、方法の「基準の写真に合わせる」を使ってください。
      </p>
      <div className="dlg-row">
        <label htmlFor="ct-clock-reference">基準の機材</label>
        <select
          id="ct-clock-reference"
          className="dlg-grow"
          value={selected}
          disabled={busy || props.disabled}
          onChange={(e) => {
            setReference(e.target.value);
            setReport(null);
          }}
        >
          {devices.map((d) => (
            <option key={deviceId(d)} value={deviceId(d)}>
              {deviceLabel(d)}
            </option>
          ))}
        </select>
        <button type="button" className="btn" onClick={() => void detect()} disabled={busy || props.disabled}>
          {busy ? "調べています…" : "調べる"}
        </button>
      </div>
      {report && report.devices.length === 0 && <p className="hint">比べられるほかの機材がありません</p>}
      {report && report.devices.length > 0 && (
        <div className="dlg-list" role="group" aria-label="時計のずれの見込み">
          {report.devices.map((d) => {
            const shifted = clockVerdict(d) === "shifted";
            const seconds = suggestedShift(d);
            return (
              <div key={deviceId(d.device)} className="dlg-list-item column">
                <div className="dlg-row">
                  <span className="dlg-grow">{deviceLabel(d.device)}</span>
                  {shifted && (
                    <button
                      type="button"
                      className="btn small"
                      disabled={props.disabled}
                      title="この機材を対象にして、方法を「ずらす」にし、この秒数を入れます（修正はまだしません）"
                      onClick={() => props.onUse(d.device, seconds)}
                    >
                      このずれを入れる（{formatShift(seconds)}）
                    </button>
                  )}
                </div>
                <span className="desc">{describeClockOffset(d)}</span>
              </div>
            );
          })}
        </div>
      )}
      {report && report.skipped_small > 0 && (
        <p className="hint">撮影日時の分かる写真が少ない {report.skipped_small.toLocaleString("ja-JP")} 台の機材は調べていません</p>
      )}
      <ErrorBox error={error} title="時計のずれを調べられません" />
    </details>
  );
}
