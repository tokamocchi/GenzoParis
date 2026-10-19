// 位置情報の後付けのダイアログ（LIB-21。ジオタグ付け。docs/04 の 3.16 節）。
//
// - 対象: 操作の対象の写真（グリッドでは選択中のすべて、ルーペでは表示中の 1 枚。actions.targets）。同じ写真の
//   仮想コピーと RAW＋JPEG の組は位置を共有する。動画は除く（コアが除いて数を返す）。
// - 座標を貼り付ける（Google マップで地点を右クリックしてコピーした「35.681236, 139.767125」・度分秒・北緯と東経・
//   Google マップの URL など。library/geotagLogic.ts の parseCoordinates。genzo-model と同じ規則）。入力するたびに
//   このパソコンの中で読み、読んだ位置か誤りを出す（外部には送らない）。
// - 読めたら、コアに見込み（変わる数・カメラの位置がある数・除く動画の数）を問い合わせる（何も変えない）。
// - 「位置を付ける」「付けた位置を消す（カメラの位置に戻す）」はカタログの中だけを変え、元のファイルには書かない。
//   最後の変更から順に「直前の変更を取り消す」で戻せる。
// - 地図（第 2 段階）は「位置」の節の座標の欄の上に置く（地図のクリック・ピンの移動で setCoordinate を呼び、座標の
//   欄に入れる。見込み・付ける操作は座標の欄の値をそのまま使う）。

import { useEffect, useRef, useState } from "react";

import * as core from "../api/core";
import { toApiError } from "../api/errors";
import type { ErrorInfo, GpsCoord, LocationEditInfo, LocationEditPlan, VariantDetails, VariantId } from "../api/types";
import { Dialog } from "../components/Dialog";
import {
  coordErrorMessage,
  describeLocationEdit,
  describeLocationPlan,
  describeLocationUndo,
  describeTargets,
  locationSummary,
  MAX_LOCATION_EDITS,
  parseCoordinates,
} from "../library/geotagLogic";
import { formatGps, gpsDecimal } from "../library/metadataLogic";
import { actions } from "../state/app";
import { Busy, ErrorBox } from "./parts";

/** 見込みを問い合わせるまでの時間（ms。入力の途中で問い合わせすぎない。コアへの問い合わせで、外部への通信ではない。仮置き）。 */
const PREVIEW_DELAY_MS = 300;

/** 貼り付けられる形の案内（library/geotagLogic.ts の冒頭の一覧の主なもの）。 */
const FORMATS_HINT =
  "貼り付けられる形: Google マップで地点を右クリックしてコピーした値（35.681236, 139.767125）・度分秒" +
  "（35°40'52.4\"N 139°46'01.7\"E）・北緯と東経（北緯 35.681236°、東経 139.767125°）・Google マップの URL" +
  "（@緯度,経度・q=緯度,経度）。";

export function GeotagDialog(props: { variantIds: VariantId[]; current: VariantId | null }) {
  const ids = props.variantIds;
  const idsKey = ids.join(",");
  const [text, setText] = useState("");
  const [ref, setRef] = useState<VariantDetails | null>(null);
  // 対象の内訳（付けた位置を消す見込み。「付けた位置を消す」の数にも使う）。
  const [targets, setTargets] = useState<LocationEditPlan | null>(null);
  const [history, setHistory] = useState<LocationEditInfo[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [plan, setPlan] = useState<LocationEditPlan | null>(null);
  const [planError, setPlanError] = useState<ErrorInfo | null>(null);
  const [counting, setCounting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const seq = useRef(0);
  const composing = useRef(false);

  const parsed = parseCoordinates(text);
  const coord = parsed.ok ? parsed.coord : null;
  const coordKey = coord ? `${coord.lat},${coord.lon}` : "";

  /** 座標の欄に入れる（第 2 段階の地図もこれを使う）。 */
  const setCoordinate = (g: GpsCoord) => setText(gpsDecimal(g));

  const reloadHistory = () =>
    core
      .locationEdits()
      .then(setHistory)
      .catch(() => setHistory([]));

  // 開いたとき: 表示中の写真の詳細・対象の内訳・履歴を読む。
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const [details, base] = await Promise.all([
          props.current !== null ? core.variantDetails(props.current) : null,
          ids.length > 0 ? core.planLocationEdit(ids, null) : null,
        ]);
        if (!alive) return;
        setRef(details);
        setTargets(base);
      } catch (e) {
        if (alive) setLoadError(actions.describeError(e));
      }
    })();
    void reloadHistory();
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.current, idsKey]);

  // 座標が変わったら見込みを問い合わせる。
  useEffect(() => {
    const my = ++seq.current;
    setPlan(null);
    setPlanError(null);
    if (!coord || ids.length === 0) {
      setCounting(false);
      return;
    }
    setCounting(true);
    const t = setTimeout(() => {
      core
        .planLocationEdit(ids, coord)
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
  }, [coordKey, idsKey]);

  /** 位置を付ける（location が null なら付けた位置を消す）。 */
  const apply = async (location: GpsCoord | null) => {
    setBusy(true);
    setError(null);
    try {
      const r = await core.editLocations(ids, location);
      actions.closeDialog();
      actions.notify(
        r.changed > 0 ? "success" : "info",
        describeLocationPlan(r, location ? "set" : "clear", true).join(" "),
        r.changed > 0 ? "元のファイルは変えていません。位置を付けるダイアログの「直前の変更を取り消す」で戻せます。" : undefined,
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
      const u = await core.undoLocationEdit();
      actions.notify("info", u ? describeLocationUndo(u) : "取り消す変更はありません");
      await reloadHistory();
      if (props.current !== null) setRef(await core.variantDetails(props.current));
      if (ids.length > 0) setTargets(await core.planLocationEdit(ids, null));
      // 見込みを問い合わせ直す。
      const my = ++seq.current;
      setPlan(null);
      setCounting(false);
      if (coord && ids.length > 0) {
        const p = await core.planLocationEdit(ids, coord);
        if (my === seq.current) setPlan(p);
      }
    } catch (e) {
      setError(toApiError(e).info);
    } finally {
      setBusy(false);
    }
  };

  const applyDisabled = busy || ids.length === 0 || !coord || !plan || counting || plan.changed === 0;
  const clearDisabled = busy || !targets || targets.changed === 0;
  const invalid = !parsed.ok && parsed.error.kind !== "empty";
  const n = (v: number) => v.toLocaleString("ja-JP");

  return (
    <Dialog
      title="位置を付ける"
      width={600}
      onClose={actions.closeDialog}
      busy={busy}
      actions={
        <>
          <button
            className="btn"
            onClick={() => void undo()}
            disabled={busy || history.length === 0}
            title={history[0] ? describeLocationEdit(history[0]) : undefined}
          >
            直前の変更を取り消す
          </button>
          <span className="spacer" />
          <button className="btn" onClick={actions.closeDialog} disabled={busy}>
            キャンセル
          </button>
          <button className="btn primary" onClick={() => coord && void apply(coord)} disabled={applyDisabled}>
            {busy ? "変えています…" : "位置を付ける"}
          </button>
        </>
      }
    >
      {loadError && <p className="error-text">読み込めません: {loadError}</p>}
      <p className="hint" style={{ marginTop: 0 }}>
        カタログの中の位置だけを変えます（元のファイルには書きません。カメラが記録した位置も残ります）。書き出したファイルの
        Exif には付けた位置が入ります（書き出しの「位置情報（GPS）を削除する」を選ぶと入りません）。動画は対象外です。
      </p>

      <div className="dlg-section">
        <h3>対象</h3>
        {ids.length === 0 && <p className="hint">写真を選んでください（グリッドで選ぶか、ルーペで表示してください）。</p>}
        {ids.length > 0 && !targets && !loadError && <Busy>対象を数えています…</Busy>}
        {targets && <div className="dlg-note">{describeTargets(targets).join("\n")}</div>}
        {ref && (
          <p className="hint">
            表示中の写真: {ref.summary.file_name}（{locationSummary(ref)}）
          </p>
        )}
      </div>

      <div className="dlg-section">
        <h3>位置</h3>
        <div className="field">
          <label htmlFor="geo-coord">座標（緯度, 経度）</label>
          <input
            id="geo-coord"
            value={text}
            placeholder="35.681236, 139.767125"
            spellCheck={false}
            autoComplete="off"
            aria-invalid={invalid}
            onChange={(e) => setText(e.target.value)}
            onCompositionStart={() => {
              composing.current = true;
            }}
            onCompositionEnd={() => {
              composing.current = false;
            }}
            onKeyDown={(e) => {
              // Enter で付ける（IME の変換の確定は除く）。
              if (composing.current || e.nativeEvent.isComposing || e.keyCode === 229) return;
              if (e.key === "Enter") {
                e.preventDefault();
                if (!applyDisabled && coord) void apply(coord);
              }
            }}
          />
        </div>
        {parsed.ok && <div className="dlg-note">読んだ位置: {formatGps(parsed.coord)}</div>}
        {invalid && !parsed.ok && <p className="dlg-field-error">{coordErrorMessage(parsed.error)}</p>}
        {ref?.gps && (
          <div className="dlg-row">
            <button type="button" className="btn small" onClick={() => ref.gps && setCoordinate(ref.gps)} disabled={busy}>
              表示中の写真の位置を入れる
            </button>
          </div>
        )}
        <p className="hint">
          {FORMATS_HINT}
          <br />
          座標はこのパソコンの中で読み、外部には送りません。
        </p>
      </div>

      <div className="dlg-section">
        <h3>見込み</h3>
        {ids.length > 0 && !coord && <p className="hint">座標を入れると、位置の変わる写真の数を出します。</p>}
        {counting && <Busy>見込みを数えています…</Busy>}
        {plan && !counting && <div className="dlg-note">{describeLocationPlan(plan, "set", false).join("\n")}</div>}
        <ErrorBox error={planError} />
      </div>

      <div className="dlg-section">
        <h3>付けた位置を消す</h3>
        <p className="hint">
          選んだ写真の付けた位置を消して、カメラの位置に戻します（カメラの位置がない写真は位置なしになります）。
        </p>
        {targets && targets.changed === 0 && targets.photos > 0 && <p className="hint">付けた位置のある写真はありません。</p>}
        <div className="dlg-row">
          <button type="button" className="btn" onClick={() => void apply(null)} disabled={clearDisabled}>
            付けた位置を消す（カメラの位置に戻す）{targets && targets.changed > 0 ? `: ${n(targets.changed)} 枚` : ""}
          </button>
        </div>
      </div>

      {history.length > 0 && (
        <p className="hint">
          直前の変更: {describeLocationEdit(history[0])}（{n(history[0].asset_count)} 枚）。変更の履歴は {n(history.length)} 件（新しい{" "}
          {n(MAX_LOCATION_EDITS)} 件まで残します）。
        </p>
      )}
      <ErrorBox error={error} />
    </Dialog>
  );
}
