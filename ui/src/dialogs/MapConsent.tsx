// オンラインの地図（位置情報の後付け LIB-21 の第 2 段階）を有効にする前の説明と確認（docs/02 の SEC-02。既定で
// 無効で、利用者が明示的に有効にしたときだけ通信する）。位置を付けるダイアログと設定の画面（一般）で同じものを出す。
//
// 何をどこへ送るか・アプリが守る OSM の利用ポリシーの規則・気をつけること（文は library/geoMapLogic.ts の
// mapConsentText）と、利用ポリシーのページ（既定のブラウザで開く）を示す。有効にするボタンは .primary にしない
// （Dialog が開いたときに最初の入力か .primary のボタンにフォーカスを移すので、Enter で有効にしてしまわないため）。

import type { ErrorInfo, MapPreferences } from "../api/types";
import { MAP_LINK_LABELS, mapConsentText } from "../library/geoMapLogic";
import { openMapPage } from "../state/map";
import { ErrorBox } from "./parts";
import "../library/geo.css";

export function MapConsentPanel(props: {
  preferences: MapPreferences;
  busy: boolean;
  error: ErrorInfo | null;
  onEnable: () => void;
  onCancel?: () => void;
}) {
  const t = mapConsentText(props.preferences);
  return (
    <div className="dlg-confirm geo-consent">
      <p>
        <strong>オンラインの地図（OpenStreetMap）を有効にしますか</strong>
      </p>
      <p>有効にすると、地図の表示と地名の検索のために、次のものを外部のサーバーに送ります。</p>
      <ul className="hint">
        {t.sends.map((s) => (
          <li key={s}>{s}。</li>
        ))}
      </ul>
      <p>送り先: {t.destination}</p>
      <p>OpenStreetMap Foundation の利用ポリシーに沿って、アプリは次のことを守ります。</p>
      <ul className="hint">
        {t.rules.map((s) => (
          <li key={s}>{s}。</li>
        ))}
      </ul>
      <ul className="hint">
        {t.cautions.map((s) => (
          <li key={s}>{s}。</li>
        ))}
      </ul>
      <div className="dlg-row">
        <button type="button" className="btn small" onClick={() => openMapPage("tile_policy")}>
          {MAP_LINK_LABELS.tile_policy}
        </button>
        <button type="button" className="btn small" onClick={() => openMapPage("nominatim_policy")}>
          {MAP_LINK_LABELS.nominatim_policy}
        </button>
        <span className="hint">（英語のページを既定のブラウザで開きます）</span>
      </div>
      <ErrorBox error={props.error} title="有効にできません" />
      <div className="dlg-row end">
        {props.onCancel && (
          <button type="button" className="btn" onClick={props.onCancel} disabled={props.busy}>
            やめる
          </button>
        )}
        <button type="button" className="btn" onClick={props.onEnable} disabled={props.busy}>
          {props.busy ? "有効にしています…" : "オンラインの地図を有効にする"}
        </button>
      </div>
    </div>
  );
}
